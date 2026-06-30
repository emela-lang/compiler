use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use emela_codegen::{
    Artifact, Backend, BackendOptions, BackendRegistry, EmitMode, IrProgram, Tier, emit_text,
};

use crate::error::{Error, Result};
use crate::external;
use crate::imports;
use crate::lower;
use crate::parser::parse_program;
use crate::typecheck;

const DEFAULT_BACKEND: &str = "js-node";

/// The set of built-in backends, in display order.
fn registry() -> BackendRegistry {
    let mut registry = BackendRegistry::new();
    #[cfg(feature = "backend-wasm")]
    registry.register(Box::new(emela_backend_wasm::WasmBackend));
    #[cfg(feature = "backend-js")]
    registry.register(Box::new(emela_backend_js::JsBackend));
    registry
}

/// Canonicalize a user-facing backend name to a registered name.
fn canonical_backend(name: &str) -> &str {
    match name {
        "js" | "js-bun" => "js-node",
        "wasm" => "wasm-wasi",
        other => other,
    }
}

pub fn run() -> Result<()> {
    match parse_args()? {
        Command::Check { input, packages } => {
            let _ = compile_frontend(&input, &packages)?;
            Ok(())
        }
        Command::Build {
            input,
            output,
            packages,
            backend,
            mode,
        } => {
            let artifact = build(&input, &packages, backend.as_deref(), mode)?;
            write_artifact(artifact, output)
        }
        Command::Ir {
            input,
            output,
            packages,
        } => {
            let ir = compile_to_ir(&input, &packages)?;
            let text = emit_text(&ir);
            match output {
                Some(output) => fs::write(&output, text).map_err(|err| {
                    Error::new(format!("failed to write `{}`: {err}", output.display()))
                }),
                None => {
                    print!("{text}");
                    Ok(())
                }
            }
        }
        Command::Backends => {
            for (name, tier) in registry().list() {
                println!("{name}\t{}", tier.label());
            }
            Ok(())
        }
        Command::Version => {
            println!(
                "{}",
                option_env!("EMELA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
            );
            Ok(())
        }
    }
}

fn build(
    input: &PathBuf,
    package_paths: &[PathBuf],
    backend: Option<&str>,
    mode: EmitMode,
) -> Result<Artifact> {
    let ir = compile_to_ir(input, package_paths)?;
    let options = BackendOptions {
        mode,
        ..Default::default()
    };
    let requested = backend.unwrap_or(DEFAULT_BACKEND);

    // A `--backend PATH` pointing at a descriptor selects an external process.
    if external::is_descriptor_path(requested) {
        let backend = external::load_backend(Path::new(requested))?;
        note_tier(&backend);
        return backend
            .compile(&ir, &options)
            .map_err(|err| Error::new(err.to_string()));
    }

    let registry = registry();
    let name = canonical_backend(requested);
    let backend = registry.get(name).ok_or_else(|| {
        let available = registry
            .list()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        Error::new(format!(
            "unknown backend `{name}`; available backends: {available}"
        ))
    })?;
    note_tier(backend);
    backend
        .compile(&ir, &options)
        .map_err(|err| Error::new(err.to_string()))
}

/// Warns when building with a backend that is not fully supported (Tier 1).
fn note_tier(backend: &dyn Backend) {
    if backend.tier() != Tier::Tier1 {
        eprintln!(
            "note: backend `{}` is {} (build + smoke only)",
            backend.name(),
            backend.tier().label()
        );
    }
}

fn write_artifact(artifact: Artifact, output: Option<PathBuf>) -> Result<()> {
    match output {
        Some(output) => fs::write(&output, &artifact.bytes)
            .map_err(|err| Error::new(format!("failed to write `{}`: {err}", output.display()))),
        None => {
            if artifact.kind.is_text() {
                print!("{}", String::from_utf8_lossy(&artifact.bytes));
                Ok(())
            } else {
                Err(Error::new(
                    "binary artifact; pass -o FILE to write it to disk",
                ))
            }
        }
    }
}

fn compile_to_ir(input: &PathBuf, package_paths: &[PathBuf]) -> Result<IrProgram> {
    let (program, typed) = compile_frontend(input, package_paths)?;
    Ok(lower::lower(&program, &typed))
}

fn compile_frontend(
    input: &PathBuf,
    package_paths: &[PathBuf],
) -> Result<(crate::ast::Program, typecheck::TypedProgram)> {
    let source = fs::read_to_string(input)
        .map_err(|err| Error::new(format!("failed to read `{}`: {err}", input.display())))?;
    let label = input.display().to_string();
    let program = parse_program(&label, &source)?;
    let packages = imports::load_packages(package_paths)?;
    let program = imports::resolve_imports(input, program, &packages)?;
    let typed = typecheck::check(&program)?;
    Ok((program, typed))
}

enum Command {
    Check {
        input: PathBuf,
        packages: Vec<PathBuf>,
    },
    Build {
        input: PathBuf,
        output: Option<PathBuf>,
        packages: Vec<PathBuf>,
        backend: Option<String>,
        mode: EmitMode,
    },
    Ir {
        input: PathBuf,
        output: Option<PathBuf>,
        packages: Vec<PathBuf>,
    },
    Backends,
    Version,
}

fn parse_args() -> Result<Command> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    match command.as_str() {
        "--version" | "-V" => Ok(Command::Version),
        "backends" => Ok(Command::Backends),
        "check" => {
            let parsed = parse_compile_args(args)?;
            Ok(Command::Check {
                input: parsed.input,
                packages: parsed.packages,
            })
        }
        "build" => {
            let parsed = parse_compile_args(args)?;
            Ok(Command::Build {
                input: parsed.input,
                output: parsed.output,
                packages: parsed.packages,
                backend: parsed.backend,
                mode: parsed.mode,
            })
        }
        "ir" => {
            let parsed = parse_compile_args(args)?;
            Ok(Command::Ir {
                input: parsed.input,
                output: parsed.output,
                packages: parsed.packages,
            })
        }
        _ => Err(usage()),
    }
}

struct CompileArgs {
    input: PathBuf,
    output: Option<PathBuf>,
    packages: Vec<PathBuf>,
    backend: Option<String>,
    mode: EmitMode,
}

fn parse_compile_args(args: impl Iterator<Item = String>) -> Result<CompileArgs> {
    let mut input = None;
    let mut output = None;
    let mut packages = Vec::new();
    let mut backend = None;
    let mut mode = EmitMode::Default;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                let Some(path) = args.next() else {
                    return Err(Error::new("missing value for --output"));
                };
                output = Some(PathBuf::from(path));
            }
            "--backend" => {
                let Some(value) = args.next() else {
                    return Err(Error::new("missing value for --backend"));
                };
                backend = Some(value);
            }
            "--emit" => {
                let Some(value) = args.next() else {
                    return Err(Error::new("missing value for --emit"));
                };
                mode = match value.as_str() {
                    "default" => EmitMode::Default,
                    "text" => EmitMode::Text,
                    other => {
                        return Err(Error::new(format!(
                            "unknown --emit value `{other}` (expected `default` or `text`)"
                        )));
                    }
                };
            }
            "--package" => {
                let Some(path) = args.next() else {
                    return Err(Error::new("missing value for --package"));
                };
                packages.push(PathBuf::from(path));
            }
            flag if flag.starts_with('-') => {
                return Err(Error::new(format!("unsupported option `{flag}`")));
            }
            path => {
                if input.replace(PathBuf::from(path)).is_some() {
                    return Err(Error::new("multiple input files are not supported"));
                }
            }
        }
    }
    let input = input.ok_or_else(usage)?;
    Ok(CompileArgs {
        input,
        output,
        packages,
        backend,
        mode,
    })
}

fn usage() -> Error {
    Error::new(
        "usage: emela check [--backend NAME] [--package DIR] FILE \
         | emela build [--backend NAME] [--emit default|text] [--package DIR] [-o FILE] FILE \
         | emela ir [--package DIR] [-o FILE] FILE \
         | emela backends | emela --version",
    )
}
