use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{Block, BlockItem, Expr, ImportDecl, ImportOrigin, Program, TopLevelItem};
use crate::backend::{Backend, EmitOptions};
use crate::error::{Error, Result, SourceFile};
use crate::lexer::lex_with_file;
use crate::package::cache::git_cache_path;
use crate::package::fetch::fetch_git_dependency;
use crate::package::manifest::{PackageManifest, ProjectManifest};
use crate::parser::Parser;
#[cfg(test)]
use crate::platform::PlatformSpec;
use crate::platform::Target;
#[cfg(test)]
use crate::typecheck::TypedProgram;
use crate::typecheck::{CheckMode, TypeChecker};

#[derive(Debug)]
enum Command {
    Check(CompileArgs),
    Build(CompileArgs),
    PackageFetch,
}

#[derive(Debug)]
struct Args {
    command: Command,
}

#[derive(Debug)]
struct CompileArgs {
    input: PathBuf,
    output: Option<PathBuf>,
    artifact: Option<PathBuf>,
    library: bool,
    target: Option<Target>,
    backend: Option<String>,
    packages: Vec<PathBuf>,
}

#[derive(Clone)]
struct PackageSource {
    name: String,
    source_root: PathBuf,
}

#[cfg(test)]
pub(crate) fn compile_source(source: &str) -> Result<(Program, TypedProgram)> {
    compile_source_for_target(source, Target::host()?)
}

#[cfg(test)]
pub(crate) fn compile_source_for_target(
    source: &str,
    target: Target,
) -> Result<(Program, TypedProgram)> {
    let platform = PlatformSpec::native_for_target(target);
    compile_source_for_platform(source, &platform)
}

#[cfg(test)]
pub(crate) fn compile_source_for_platform(
    source: &str,
    platform: &PlatformSpec,
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program("<test>", source)?;
    expand_package_imports(&mut program, &[])?;
    let typed = TypeChecker::new(&program, platform).check()?;
    Ok((program, typed))
}

#[cfg(test)]
pub(crate) fn compile_internal_source_for_platform(
    source: &str,
    platform: &PlatformSpec,
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program("<test>", source)?;
    mark_stdlib_origin(&mut program);
    let typed = TypeChecker::new(&program, platform).check()?;
    Ok((program, typed))
}

#[cfg(test)]
pub(crate) fn compile_source_for_platform_with_mode(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
) -> Result<(Program, TypedProgram)> {
    compile_source_for_platform_with_packages(source, platform, mode, &[])
}

#[cfg(test)]
fn compile_source_for_platform_with_packages(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
    packages: &[PackageSource],
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program("<test>", source)?;
    expand_package_imports(&mut program, packages)?;
    let typed = TypeChecker::new_with_mode(&program, platform, mode).check()?;
    Ok((program, typed))
}

fn parse_program(label: &str, source: &str) -> Result<Program> {
    let file = SourceFile::new(label, source.to_string());
    let tokens = lex_with_file(source, file)?;
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    match args.command {
        Command::Check(compile_args) => run_check(compile_args),
        Command::Build(compile_args) => run_build(compile_args),
        Command::PackageFetch => run_package_fetch(),
    }
}

fn version() -> &'static str {
    option_env!("EMELA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn run_check(args: CompileArgs) -> Result<()> {
    run_compile(args, true)
}

fn run_build(args: CompileArgs) -> Result<()> {
    run_compile(args, false)
}

fn run_compile(args: CompileArgs, check_only: bool) -> Result<()> {
    let source = fs::read_to_string(&args.input).map_err(|err| {
        Error::new(format!(
            "failed to read input file `{}`: {err}",
            args.input.display()
        ))
    })?;

    let backend_name = match args.backend.as_deref() {
        Some(name) => name,
        None if check_only => "js-node",
        None => return Err(Error::new("build requires --backend")),
    };
    let backend = Backend::parse(backend_name)?;
    let backend_target = backend.target();
    if let (Some(explicit), Some(profile)) = (args.target, backend_target) {
        if explicit != profile {
            return Err(Error::new(format!(
                "backend profile `{backend_name}` requires target `{profile}`, got `{explicit}`"
            )));
        }
    }
    let target = backend_target.or(args.target);
    let platform = backend.platform();

    let mode = if args.library {
        CheckMode::Library
    } else {
        CheckMode::Executable
    };
    let packages = resolve_package_sources(&args.input, &args.packages, true)?;
    let mut program = parse_program(&args.input.display().to_string(), &source)?;
    expand_package_imports(&mut program, &packages)?;
    let typed = TypeChecker::new_with_mode(&program, &platform, mode).check()?;
    if !check_only {
        backend.emit(
            &platform,
            &program,
            &typed,
            EmitOptions {
                target,
                mode,
                input: &args.input,
                output: args.output.as_deref(),
                artifact: args.artifact.as_deref(),
            },
        )?;
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(Error::new("missing command"));
    };
    let command = match command.as_str() {
        "check" => Command::Check(parse_compile_args(args, true)?),
        "build" => Command::Build(parse_compile_args(args, false)?),
        "package" => parse_package_command(args)?,
        "-V" | "--version" => {
            println!("emela {}", version());
            std::process::exit(0);
        }
        "-h" | "--help" => {
            print_help();
            std::process::exit(0);
        }
        _ if command.starts_with('-') => {
            return Err(Error::new(format!(
                "expected a command before option `{command}`"
            )));
        }
        _ => return Err(Error::new(format!("unknown command `{command}`"))),
    };
    Ok(Args { command })
}

fn parse_compile_args<I>(args: I, check_only: bool) -> Result<CompileArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut input = None;
    let mut output = None;
    let mut library = false;
    let mut artifact = None;
    let mut target = None;
    let mut backend = None;
    let mut packages = Vec::new();

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => return Err(Error::new("use `emela check` instead of --check")),
            "--library" => library = true,
            "--artifact" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--artifact requires a path"))?;
                artifact = Some(PathBuf::from(path));
            }
            "--target" => {
                let value = args
                    .next()
                    .ok_or_else(|| Error::new("--target requires a target triple"))?;
                target = Some(Target::parse(&value)?);
            }
            "--backend" => {
                let value = args.next().ok_or_else(|| {
                    Error::new("--backend requires a backend profile name or manifest path")
                })?;
                backend = Some(value);
            }
            "--package" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--package requires a package directory path"))?;
                packages.push(PathBuf::from(path));
            }
            "--output" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--output requires a path"))?;
                output = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => {
                return Err(Error::new(format!("unknown option `{arg}`")));
            }
            _ => {
                if input.replace(PathBuf::from(arg)).is_some() {
                    return Err(Error::new("only one input file is supported"));
                }
            }
        }
    }

    let input = input.ok_or_else(|| Error::new("missing input file"))?;
    if input.extension().and_then(|ext| ext.to_str()) != Some("emel") {
        return Err(Error::new("input file extension must be .emel"));
    }
    if check_only && (artifact.is_some() || output.is_some()) {
        return Err(Error::new(
            "--check cannot be combined with --artifact or --output",
        ));
    }
    if artifact.is_some() && output.is_some() {
        return Err(Error::new("--artifact and --output are mutually exclusive"));
    }
    if library && !check_only && artifact.is_none() {
        return Err(Error::new("--library requires `emela check` or --artifact"));
    }
    Ok(CompileArgs {
        input,
        output,
        artifact,
        library,
        target,
        backend,
        packages,
    })
}

fn parse_package_command<I>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(Error::new("missing package command"));
    };
    match command.as_str() {
        "fetch" => {
            if let Some(extra) = args.next() {
                return Err(Error::new(format!(
                    "package fetch does not accept argument `{extra}`"
                )));
            }
            Ok(Command::PackageFetch)
        }
        _ => Err(Error::new(format!("unknown package command `{command}`"))),
    }
}

fn print_help() {
    eprintln!(
        "Usage:\n  emela check [--backend PROFILE|PATH] [--target TARGET] [--package DIR]... [--library] INPUT.emel\n  emela build --backend PROFILE|PATH [--target TARGET] [--package DIR]... [--library] [--artifact PATH | --output PATH] INPUT.emel\n  emela package fetch\n  emela --version"
    );
}

fn run_package_fetch() -> Result<()> {
    let cwd = env::current_dir()
        .map_err(|err| Error::new(format!("failed to get current directory: {err}")))?;
    let manifest_path = find_project_manifest_from(&cwd)
        .ok_or_else(|| Error::new("emela.json was not found from current directory"))?;
    let manifest = ProjectManifest::read_from(&manifest_path)?;
    for (name, dependency) in &manifest.dependencies {
        fetch_git_dependency(name, dependency)?;
    }
    Ok(())
}

fn resolve_package_sources(
    input: &Path,
    explicit_packages: &[PathBuf],
    fetch_missing: bool,
) -> Result<Vec<PackageSource>> {
    let mut packages = load_package_sources(explicit_packages)?;
    if let Some(project_manifest_path) = find_project_manifest(input)? {
        let manifest = ProjectManifest::read_from(&project_manifest_path)?;
        for (name, dependency) in &manifest.dependencies {
            if packages.iter().any(|package| package.name == *name) {
                return Err(Error::new(format!(
                    "package `{name}` is provided by both emela.json and --package"
                )));
            }
            let package_root = if fetch_missing {
                fetch_git_dependency(name, dependency)?
            } else {
                git_cache_path(dependency)
            };
            let manifest = PackageManifest::read_from(&package_root)?;
            if manifest.name != *name {
                return Err(Error::new(format!(
                    "dependency `{name}` resolved to package `{}` at `{}`",
                    manifest.name,
                    package_root.display()
                )));
            }
            packages.push(PackageSource {
                name: manifest.name,
                source_root: package_root.join(manifest.source),
            });
        }
    }
    Ok(packages)
}

fn find_project_manifest(input: &Path) -> Result<Option<PathBuf>> {
    let input = if input.is_absolute() {
        input.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| Error::new(format!("failed to get current directory: {err}")))?
            .join(input)
    };
    let start = input.parent().unwrap_or_else(|| Path::new("."));
    Ok(find_project_manifest_from(start))
}

fn find_project_manifest_from(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let manifest = dir.join("emela.json");
        if manifest.exists() {
            return Some(manifest);
        }
    }
    None
}

fn load_package_sources(paths: &[PathBuf]) -> Result<Vec<PackageSource>> {
    let mut packages = Vec::new();
    for path in paths {
        let manifest = PackageManifest::read_from(path)?;
        if packages
            .iter()
            .any(|package: &PackageSource| package.name == manifest.name)
        {
            return Err(Error::new(format!("duplicate package `{}`", manifest.name)));
        }
        packages.push(PackageSource {
            name: manifest.name,
            source_root: path.join(manifest.source),
        });
    }
    Ok(packages)
}

fn expand_package_imports(program: &mut Program, packages: &[PackageSource]) -> Result<()> {
    let mut loaded_modules = BTreeSet::new();
    expand_package_imports_with_loaded(program, packages, &mut loaded_modules)
}

fn expand_package_imports_with_loaded(
    program: &mut Program,
    packages: &[PackageSource],
    loaded_modules: &mut BTreeSet<String>,
) -> Result<()> {
    let mut source_imports = Vec::new();
    program.items.retain(|item| {
        let TopLevelItem::Import(import) = item else {
            return true;
        };
        if is_source_package_import(import, packages) {
            source_imports.push(import.clone());
            false
        } else {
            true
        }
    });

    for import in source_imports {
        let package_name = import.path.first().expect("import path is not empty");
        let module_path = std_module_path(&import)?;
        let module_key = module_path.join(".");
        let import_key = format!("{package_name}.{module_key}.{}", import.name);
        let mut module = load_source_package_module(package_name, packages, &module_path)?;
        if !module_exports(&module, &import.name) {
            return Err(Error::new(format!(
                "package module `{package_name}.{}` does not export `{}`",
                module_key, import.name
            )));
        }
        retain_stdlib_item_dependencies(&mut module, &import.name);
        if loaded_modules.insert(import_key) {
            expand_package_imports_with_loaded(&mut module, packages, loaded_modules)?;
            if package_name == "std" {
                mark_stdlib_origin(&mut module);
            }
            program.items.extend(module.items);
        }
    }

    Ok(())
}

fn mark_stdlib_origin(program: &mut Program) {
    for item in &mut program.items {
        if let TopLevelItem::Import(import) = item {
            import.origin = ImportOrigin::Stdlib;
        }
    }
}

fn is_source_package_import(import: &ImportDecl, packages: &[PackageSource]) -> bool {
    let Some(package) = import.path.first() else {
        return false;
    };
    package == "std" || packages.iter().any(|source| source.name == *package)
}

fn std_module_path(import: &ImportDecl) -> Result<Vec<String>> {
    if import.path.len() < 2 {
        return Err(Error::new(format!(
            "source package import `{}` must include a module path",
            format_import_path(&import.path, &import.name)
        )));
    }
    Ok(import.path[1..].to_vec())
}

fn load_source_package_module(
    package_name: &str,
    packages: &[PackageSource],
    module_path: &[String],
) -> Result<Program> {
    let (source, label) = if package_name == "std" {
        if let Some(package) = packages.iter().find(|package| package.name == "std") {
            load_package_module(package, module_path)?
        } else {
            load_embedded_stdlib_module(module_path)?
        }
    } else {
        let package = packages
            .iter()
            .find(|package| package.name == package_name)
            .ok_or_else(|| Error::new(format!("unknown package `{package_name}`")))?;
        load_package_module(package, module_path)?
    };
    parse_program(&label, &source)
}

fn load_package_module(
    package: &PackageSource,
    module_path: &[String],
) -> Result<(String, String)> {
    load_module_file(
        package.source_root.clone(),
        module_path,
        &format!("package `{}`", package.name),
    )
}

fn load_module_file(
    mut root: PathBuf,
    module_path: &[String],
    label: &str,
) -> Result<(String, String)> {
    for part in module_path {
        root.push(part);
    }
    root.set_extension("emel");
    let source = fs::read_to_string(&root).map_err(|err| {
        Error::new(format!(
            "failed to read {label} module `{}`: {err}",
            root.display()
        ))
    })?;
    Ok((source, root.display().to_string()))
}

fn load_embedded_stdlib_module(module_path: &[String]) -> Result<(String, String)> {
    match module_path {
        [module] if module == "io" => Ok((
            include_str!("../../stdlib/std/io.emel").to_string(),
            "<embedded std.io>".to_string(),
        )),
        [module] if module == "clock" => Ok((
            include_str!("../../stdlib/std/clock.emel").to_string(),
            "<embedded std.clock>".to_string(),
        )),
        _ => Err(Error::new(format!(
            "embedded stdlib module `std.{}` is not bundled",
            module_path.join(".")
        ))),
    }
}

fn module_exports(module: &Program, name: &str) -> bool {
    module.items.iter().any(|item| match item {
        TopLevelItem::Function(function) => function.name == name,
        TopLevelItem::Import(_) => false,
        TopLevelItem::Struct(decl) => decl.name == name,
        TopLevelItem::Enum(decl) => decl.name == name,
    })
}

fn retain_stdlib_item_dependencies(module: &mut Program, export_name: &str) {
    let mut needed = BTreeSet::from([export_name.to_string()]);
    loop {
        let before = needed.len();
        for function in module.functions() {
            if needed.contains(&function.name) {
                collect_block_dependencies(&function.body, &mut needed);
            }
        }
        if needed.len() == before {
            break;
        }
    }

    module.items.retain(|item| match item {
        TopLevelItem::Function(function) => needed.contains(&function.name),
        TopLevelItem::Import(import) => needed.contains(&import.name),
        TopLevelItem::Struct(_) | TopLevelItem::Enum(_) => true,
    });
}

fn collect_block_dependencies(block: &Block, needed: &mut BTreeSet<String>) {
    for item in &block.items {
        match item {
            BlockItem::Binding { expr, .. } | BlockItem::Expr(expr) => {
                collect_expr_dependencies(expr, needed);
            }
        }
    }
}

fn collect_expr_dependencies(expr: &Expr, needed: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(name, _) => {
            needed.insert(name.clone());
        }
        Expr::String(_, _) => {}
        Expr::Call { name, args, .. } => {
            needed.insert(name.clone());
            for arg in args {
                collect_expr_dependencies(arg, needed);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_dependencies(receiver, needed);
            for arg in args {
                collect_expr_dependencies(arg, needed);
            }
        }
        Expr::FieldAccess { receiver, .. } => collect_expr_dependencies(receiver, needed),
        Expr::StructLiteral { value, .. } => collect_expr_dependencies(value, needed),
        Expr::Binary { left, right, .. } => {
            collect_expr_dependencies(left, needed);
            collect_expr_dependencies(right, needed);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_dependencies(scrutinee, needed);
            for arm in arms {
                collect_expr_dependencies(&arm.expr, needed);
            }
        }
        Expr::Block(block, _) => collect_block_dependencies(block, needed),
        Expr::Int(_, _) | Expr::Bool(_, _) | Expr::Unit(_) => {}
    }
}

fn format_import_path(path: &[String], name: &str) -> String {
    let mut parts = path.to_vec();
    parts.push(name.to_string());
    parts.join(".")
}
