use std::fs;
use std::path::Path;
use std::process::Command;

use crate::ast::Program;
use crate::error::{Error, Result};
use crate::platform::{PlatformSpec, Target};
use crate::typecheck::{CheckMode, TypedProgram};

use super::{lowering, EmitOptions};

pub(crate) enum WasmTarget {
    UnknownUnknown,
    Wasi,
}

pub(crate) struct WasmBackendProfile {
    target: WasmTarget,
}

impl WasmBackendProfile {
    pub(super) fn unknown_unknown() -> Self {
        Self {
            target: WasmTarget::UnknownUnknown,
        }
    }

    pub(super) fn wasi() -> Self {
        Self {
            target: WasmTarget::Wasi,
        }
    }

    pub(super) fn target(&self) -> Option<Target> {
        match self.target {
            WasmTarget::UnknownUnknown => Some(Target::Wasm32UnknownUnknown),
            WasmTarget::Wasi => Some(Target::Wasm32Wasi),
        }
    }

    pub(super) fn platform(&self) -> PlatformSpec {
        match self.target {
            WasmTarget::UnknownUnknown => PlatformSpec::wasm(),
            WasmTarget::Wasi => PlatformSpec::wasi(),
        }
    }

    pub(super) fn emit(
        &self,
        platform: &PlatformSpec,
        program: &Program,
        _typed: &TypedProgram,
        options: EmitOptions<'_>,
    ) -> Result<()> {
        let wat = match self.target {
            WasmTarget::UnknownUnknown => {
                if options.mode == CheckMode::Library {
                    emit_wasm_library_artifact(program)?
                } else {
                    emit_wasm_artifact(program)?
                }
            }
            WasmTarget::Wasi => {
                if options.mode == CheckMode::Library {
                    emit_wasi_library_artifact(program, platform)?
                } else {
                    emit_wasi_artifact(program, platform)?
                }
            }
        };
        if let Some(path) = options.artifact {
            return fs::write(path, wat).map_err(|err| {
                Error::new(format!(
                    "failed to write backend output `{}`: {err}",
                    path.display()
                ))
            });
        }
        if options.mode == CheckMode::Library {
            return Err(Error::new("--library requires --check or --artifact"));
        }
        if let Some(output) = options.output {
            return build_wasm_binary(output, &wat);
        }
        Err(Error::new("wasm backend requires --artifact or --output"))
    }
}

fn build_wasm_binary(output: &Path, wat: &str) -> Result<()> {
    let temp = std::env::temp_dir().join(format!(
        "emela-{}.wat",
        std::process::id()
    ));
    fs::write(&temp, wat).map_err(|err| {
        Error::new(format!(
            "failed to write temporary wat `{}`: {err}",
            temp.display()
        ))
    })?;

    let status = Command::new("wat2wasm")
        .arg(&temp)
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|err| Error::new(format!("failed to execute wat2wasm: {err}")))?;

    let _ = fs::remove_file(&temp);

    if !status.success() {
        return Err(Error::new(format!(
            "wat2wasm failed while building `{}`",
            output.display()
        )));
    }
    Ok(())
}

pub(crate) fn emit_wasm_artifact(program: &Program) -> Result<String> {
    lowering::emit_wasm(program)
}

pub(crate) fn emit_wasm_library_artifact(program: &Program) -> Result<String> {
    lowering::emit_wasm_library(program)
}

pub(crate) fn emit_wasi_artifact(program: &Program, platform: &PlatformSpec) -> Result<String> {
    lowering::emit_wasi(program, platform)
}

pub(crate) fn emit_wasi_library_artifact(program: &Program, platform: &PlatformSpec) -> Result<String> {
    lowering::emit_wasi_library(program, platform)
}
