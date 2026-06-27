use std::collections::BTreeSet;
use std::fmt;

use crate::ast::Capability;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Aarch64AppleDarwin,
    X86_64UnknownLinuxGnu,
    Wasm32UnknownUnknown,
    Wasm32Wasi,
}

impl Target {
    pub(crate) fn host() -> Result<Self> {
        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("aarch64", "macos") => Ok(Self::Aarch64AppleDarwin),
            ("x86_64", "linux") => Ok(Self::X86_64UnknownLinuxGnu),
            (arch, os) => Err(Error::new(format!(
                "unsupported host target `{arch}-{os}`; pass --target explicitly for checking"
            ))),
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "aarch64-apple-darwin" => Ok(Self::Aarch64AppleDarwin),
            "x86_64-unknown-linux-gnu" => Ok(Self::X86_64UnknownLinuxGnu),
            "wasm32-unknown-unknown" => Ok(Self::Wasm32UnknownUnknown),
            "wasm32-wasi" => Ok(Self::Wasm32Wasi),
            _ => Err(Error::new(format!("unknown target `{value}`"))),
        }
    }

    pub(crate) fn provided_capabilities(self) -> BTreeSet<Capability> {
        match self {
            Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxGnu => [
                Capability::Stdout,
                Capability::Stdin,
                Capability::Stderr,
                Capability::FileRead,
                Capability::FileWrite,
                Capability::Clock,
                Capability::Random,
                Capability::Env,
                Capability::Process,
                Capability::Network,
            ]
            .into_iter()
            .collect(),
            Self::Wasm32UnknownUnknown => BTreeSet::new(),
            Self::Wasm32Wasi => [
                Capability::Stdout,
                Capability::Stdin,
                Capability::Stderr,
                Capability::FileRead,
                Capability::FileWrite,
                Capability::Clock,
                Capability::Random,
                Capability::Env,
            ]
            .into_iter()
            .collect(),
        }
    }

    pub(crate) fn supports_native_backend(self) -> bool {
        matches!(self, Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxGnu)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aarch64AppleDarwin => write!(f, "aarch64-apple-darwin"),
            Self::X86_64UnknownLinuxGnu => write!(f, "x86_64-unknown-linux-gnu"),
            Self::Wasm32UnknownUnknown => write!(f, "wasm32-unknown-unknown"),
            Self::Wasm32Wasi => write!(f, "wasm32-wasi"),
        }
    }
}
