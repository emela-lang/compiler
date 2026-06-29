use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::ast::Program;
use crate::backend::{Backend, EmitOptions};
use crate::error::{Error, Result};
use crate::package::imports::expand_package_imports;
#[cfg(test)]
use crate::package::imports::mark_stdlib_origin;
use crate::package::manifest::GitDependency;
use crate::package::manifest::{ProjectIdentity, ProjectManifest};
use crate::package::resolve::{
    add_project_dependency_from_current_dir, fetch_project_dependencies_from_current_dir,
    resolve_package_sources,
};
#[cfg(test)]
use crate::package::PackageSource;
use crate::parser::parse_program;
#[cfg(test)]
use crate::platform::PlatformSpec;
use crate::platform::Target;
#[cfg(test)]
use crate::typecheck::TypedProgram;
use crate::typecheck::{CheckMode, TypeChecker};

#[cfg(test)]
static TEST_STDLIB_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(Debug)]
enum Command {
    Init,
    Check(CompileArgs),
    Build(CompileArgs),
    PackageFetch,
    PackageAdd(PackageAddArgs),
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

#[derive(Debug)]
struct PackageAddArgs {
    name: String,
    git: String,
    rev: String,
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
    let platform = match target {
        Target::Wasm32Wasi => PlatformSpec::wasi(),
        Target::Wasm32UnknownUnknown => PlatformSpec::wasm(),
        _ => PlatformSpec::native_for_target(target),
    };
    compile_source_for_platform(source, &platform)
}

#[cfg(test)]
pub(crate) fn compile_source_for_platform(
    source: &str,
    platform: &PlatformSpec,
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program("<test>", source)?;
    let packages = test_default_packages();
    expand_package_imports(&mut program, &packages)?;
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
    let mut resolved_packages = test_default_packages();
    for package in packages {
        if !resolved_packages
            .iter()
            .any(|source| source.name == package.name)
        {
            resolved_packages.push(package.clone());
        }
    }
    expand_package_imports(&mut program, &resolved_packages)?;
    let typed = TypeChecker::new_with_mode(&program, platform, mode).check()?;
    Ok((program, typed))
}

#[cfg(test)]
fn test_default_packages() -> Vec<PackageSource> {
    let id = TEST_STDLIB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let source_root = env::temp_dir()
        .join(format!("emela-test-stdlib-{}-{id}", std::process::id()))
        .join("std");
    fs::create_dir_all(&source_root).unwrap();
    fs::write(
        source_root.join("io.emel"),
        r#"import platform.io._write_stdout_utf8!
import platform.io._read_stdin_utf8!
import platform.io._write_stderr_utf8!

#[requires(Stdout)]
fn write_stdout_utf8!(value: String) -> Result<Unit, PlatformError> {
  _write_stdout_utf8!(value)
}

#[requires(Stdin)]
fn read_stdin_utf8!() -> Result<String, PlatformError> {
  _read_stdin_utf8!()
}

#[requires(Stderr)]
fn write_stderr_utf8!(value: String) -> Result<Unit, PlatformError> {
  _write_stderr_utf8!(value)
}
"#,
    )
    .unwrap();
    fs::write(
        source_root.join("clock.emel"),
        r#"import platform.clock._now_i32!

#[requires(Clock)]
fn now_i32!() -> I32 {
  _now_i32!()
}
"#,
    )
    .unwrap();
    fs::write(
        source_root.join("fs.emel"),
        r#"import platform.fs._read_file_utf8!
import platform.fs._write_file_utf8!

#[requires(FileRead)]
fn read_file_utf8!(path: String) -> Result<String, PlatformError> {
  _read_file_utf8!(path)
}

#[requires(FileWrite)]
fn write_file_utf8!(path: String, content: String) -> Result<Unit, PlatformError> {
  _write_file_utf8!(path, content)
}
"#,
    )
    .unwrap();
    fs::write(
        source_root.join("random.emel"),
        r#"import platform.random._random_i32!

#[requires(Random)]
fn random_i32!() -> I32 {
  _random_i32!()
}
"#,
    )
    .unwrap();
    fs::write(
        source_root.join("env.emel"),
        r#"import platform.env._get_env!

#[requires(Env)]
fn get_env!(key: String) -> Result<String, PlatformError> {
  _get_env!(key)
}
"#,
    )
    .unwrap();
    vec![PackageSource {
        name: "std".to_string(),
        source_root,
    }]
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    match args.command {
        Command::Init => run_init(),
        Command::Check(compile_args) => run_check(compile_args),
        Command::Build(compile_args) => run_build(compile_args),
        Command::PackageFetch => run_package_fetch(),
        Command::PackageAdd(args) => run_package_add(args),
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
    let packages = resolve_package_sources(&args.input, &args.packages)?;
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
        "init" => {
            if let Some(extra) = args.next() {
                return Err(Error::new(format!(
                    "init does not accept argument `{extra}`"
                )));
            }
            Command::Init
        }
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
        "add" => Ok(Command::PackageAdd(parse_package_add_args(args)?)),
        _ => Err(Error::new(format!("unknown package command `{command}`"))),
    }
}

fn parse_package_add_args<I>(args: I) -> Result<PackageAddArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut name = None;
    let mut git = None;
    let mut rev = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--git" => {
                let value = args
                    .next()
                    .ok_or_else(|| Error::new("--git requires a URL"))?;
                git = Some(value);
            }
            "--rev" => {
                let value = args
                    .next()
                    .ok_or_else(|| Error::new("--rev requires a revision"))?;
                rev = Some(value);
            }
            _ if arg.starts_with('-') => {
                return Err(Error::new(format!("unknown package add option `{arg}`")));
            }
            _ => {
                if name.replace(arg).is_some() {
                    return Err(Error::new("package add accepts only one dependency name"));
                }
            }
        }
    }
    Ok(PackageAddArgs {
        name: name.ok_or_else(|| Error::new("package add requires a dependency name"))?,
        git: git.ok_or_else(|| Error::new("package add requires --git"))?,
        rev: rev.ok_or_else(|| Error::new("package add requires --rev"))?,
    })
}

fn print_help() {
    eprintln!(
        "Usage:\n  emela init\n  emela check [--backend PROFILE|PATH] [--target TARGET] [--package DIR]... [--library] INPUT.emel\n  emela build --backend PROFILE|PATH [--target TARGET] [--package DIR]... [--library] [--artifact PATH | --output PATH] INPUT.emel\n  emela package fetch\n  emela package add NAME --git URL --rev REV\n  emela --version"
    );
}

fn run_init() -> Result<()> {
    let cwd = env::current_dir()
        .map_err(|err| Error::new(format!("failed to get current directory: {err}")))?;
    let manifest_path = cwd.join("emela.json");
    if manifest_path.exists() {
        return Err(Error::new(format!(
            "project manifest `{}` already exists",
            manifest_path.display()
        )));
    }

    let package_name = package_name_from_dir(&cwd);
    let manifest = ProjectManifest {
        package: ProjectIdentity {
            name: package_name,
            version: "0.1.0".to_string(),
        },
        dependencies: Default::default(),
    };
    manifest.write_to(&manifest_path)?;
    println!("created {}", manifest_path.display());
    Ok(())
}

fn package_name_from_dir(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("app")
        .to_string()
}

fn run_package_fetch() -> Result<()> {
    fetch_project_dependencies_from_current_dir()
}

fn run_package_add(args: PackageAddArgs) -> Result<()> {
    add_project_dependency_from_current_dir(
        args.name,
        GitDependency {
            git: args.git,
            rev: args.rev,
        },
    )
}
