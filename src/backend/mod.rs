use std::path::Path;

use crate::ast::Program;
use crate::error::{Error, Result};
use crate::platform::{PlatformSpec, Target};
use crate::typecheck::{CheckMode, TypedProgram};

mod bundled;
mod external;
mod js;
mod lowering;
mod native;
mod wasm;

pub(crate) use external::ExternalBackend;

#[cfg(test)]
pub(crate) use js::{emit_js_artifact, emit_js_library_artifact};

#[cfg(test)]
pub(crate) use wasm::{
    emit_wasi_artifact, emit_wasi_library_artifact, emit_wasm_artifact, emit_wasm_library_artifact,
};

#[cfg(test)]
pub(crate) use native::{
    emit_native_assembly, emit_native_assembly_for_platform, native_link_args,
};

pub(crate) const BACKEND_ABI_VERSION: u32 = 1;

pub(crate) enum Backend {
    Native(native::NativeBackendProfile),
    Js(js::JsBackendProfile),
    Wasm(wasm::WasmBackendProfile),
    External(ExternalBackend),
}

pub(crate) struct EmitOptions<'a> {
    pub(crate) mode: CheckMode,
    pub(crate) input: &'a Path,
    pub(crate) output: Option<&'a Path>,
    pub(crate) artifact: Option<&'a Path>,
    pub(crate) target: Option<Target>,
}

impl Backend {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "native" => Err(Error::new(
                "backend profile `native` is no longer supported; use `native-aarch64-apple-darwin` or `native-x86_64-unknown-linux-gnu`",
            )),
            "native-aarch64-apple-darwin" => Ok(Self::Native(native::NativeBackendProfile::new(
                Target::Aarch64AppleDarwin,
            ))),
            "native-x86_64-unknown-linux-gnu" => Ok(Self::Native(
                native::NativeBackendProfile::new(Target::X86_64UnknownLinuxGnu),
            )),
            "js" => Err(Error::new(
                "backend profile `js` is no longer supported; use `js-node` or `js-bun`",
            )),
            "js-node" => Ok(Self::Js(js::JsBackendProfile::node())),
            "js-bun" => Ok(Self::Js(js::JsBackendProfile::bun())),
            "wasm" => Ok(Self::Wasm(wasm::WasmBackendProfile::unknown_unknown())),
            "wasm-wasi" => Ok(Self::Wasm(wasm::WasmBackendProfile::wasi())),
            path => Ok(Self::External(ExternalBackend::from_manifest_path(
                Path::new(path),
            )?)),
        }
    }

    pub(crate) fn target(&self) -> Option<Target> {
        match self {
            Self::Native(backend) => Some(backend.target),
            Self::Wasm(backend) => backend.target(),
            Self::Js(_) => None,
            Self::External(backend) => backend.target(),
        }
    }

    pub(crate) fn platform(&self) -> PlatformSpec {
        match self {
            Self::Native(backend) => backend.platform(),
            Self::Wasm(backend) => backend.platform(),
            Self::Js(backend) => backend.platform(),
            Self::External(backend) => backend.platform(),
        }
    }

    pub(crate) fn emit(
        &self,
        platform: &PlatformSpec,
        program: &Program,
        typed: &TypedProgram,
        options: EmitOptions<'_>,
    ) -> Result<()> {
        match self {
            Self::Native(backend) => backend.emit(platform, program, typed, options),
            Self::Wasm(backend) => backend.emit(platform, program, typed, options),
            Self::Js(backend) => backend.emit(platform, program, typed, options),
            Self::External(backend) => backend.emit(platform, program, typed, options),
        }
    }
}
