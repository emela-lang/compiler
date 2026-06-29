use std::env;
use std::fs;
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::imports;
use crate::ir;
use crate::js;
use crate::parser::parse_program;
use crate::typecheck;

pub(crate) fn run() -> Result<()> {
    match parse_args()? {
        Command::Check { input, packages } => {
            let (typed, _) = compile(&input, &packages)?;
            let _ = typed.function_count();
            let _ = typed.signature_summary();
            Ok(())
        }
        Command::Build {
            input,
            output,
            packages,
        } => {
            let (_, artifact) = compile(&input, &packages)?;
            if let Some(output) = output {
                fs::write(&output, artifact).map_err(|err| {
                    Error::new(format!("failed to write `{}`: {err}", output.display()))
                })?;
            } else {
                print!("{artifact}");
            }
            Ok(())
        }
        Command::Ir {
            input,
            output,
            packages,
        } => {
            let artifact = compile_ir(&input, &packages)?;
            if let Some(output) = output {
                fs::write(&output, artifact).map_err(|err| {
                    Error::new(format!("failed to write `{}`: {err}", output.display()))
                })?;
            } else {
                print!("{artifact}");
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

fn compile(
    input: &PathBuf,
    package_paths: &[PathBuf],
) -> Result<(typecheck::TypedProgram, String)> {
    let (program, typed) = compile_frontend(input, package_paths)?;
    let ir = ir::lower(&program, &typed);
    Ok((typed, js::emit(&ir)))
}

fn compile_ir(input: &PathBuf, package_paths: &[PathBuf]) -> Result<String> {
    let (program, typed) = compile_frontend(input, package_paths)?;
    let ir = ir::lower(&program, &typed);
    Ok(ir::emit_text(&ir))
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
    },
    Ir {
        input: PathBuf,
        output: Option<PathBuf>,
        packages: Vec<PathBuf>,
    },
    Version,
}

fn parse_args() -> Result<Command> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    match command.as_str() {
        "--version" | "-V" => Ok(Command::Version),
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
}

fn parse_compile_args(args: impl Iterator<Item = String>) -> Result<CompileArgs> {
    let mut input = None;
    let mut output = None;
    let mut packages = Vec::new();
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
                let Some(backend) = args.next() else {
                    return Err(Error::new("missing value for --backend"));
                };
                if backend != "js-node" && backend != "js" {
                    return Err(Error::new(
                        "minimal compiler only supports --backend js-node",
                    ));
                }
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
    })
}

fn usage() -> Error {
    Error::new("usage: emela check [--backend js-node] [--package DIR] FILE | emela build [--backend js-node] [--package DIR] [-o FILE] FILE | emela ir [--package DIR] [-o FILE] FILE")
}
