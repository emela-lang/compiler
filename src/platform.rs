use std::collections::BTreeSet;
use std::fmt;

use crate::ast::Capability;
use crate::error::{Error, Result};
use crate::external::ExternalRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Aarch64AppleDarwin,
    X86_64UnknownLinuxGnu,
    Wasm32UnknownUnknown,
    Wasm32Wasi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendKind {
    NativeAarch64Darwin,
    NativeX86_64LinuxGnu,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkerKind {
    HostCc,
    None,
}

#[derive(Debug)]
pub(crate) struct TargetSpec {
    pub(crate) target: Target,
    pub(crate) triple: &'static str,
    pub(crate) backend: BackendKind,
    pub(crate) linker: LinkerKind,
    capabilities: &'static [Capability],
}

#[derive(Debug, Clone)]
pub(crate) struct PlatformSpec {
    pub(crate) name: String,
    pub(crate) provided_capabilities: BTreeSet<Capability>,
    pub(crate) externs: ExternalRegistry,
}

const FULL_NATIVE_CAPABILITIES: &[Capability] = &[
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
];

const WASI_CAPABILITIES: &[Capability] = &[
    Capability::Stdout,
    Capability::Stdin,
    Capability::Stderr,
    Capability::FileRead,
    Capability::FileWrite,
    Capability::Clock,
    Capability::Random,
    Capability::Env,
];

const NO_CAPABILITIES: &[Capability] = &[];

const TARGET_SPECS: &[TargetSpec] = &[
    TargetSpec {
        target: Target::Aarch64AppleDarwin,
        triple: "aarch64-apple-darwin",
        backend: BackendKind::NativeAarch64Darwin,
        linker: LinkerKind::HostCc,
        capabilities: FULL_NATIVE_CAPABILITIES,
    },
    TargetSpec {
        target: Target::X86_64UnknownLinuxGnu,
        triple: "x86_64-unknown-linux-gnu",
        backend: BackendKind::NativeX86_64LinuxGnu,
        linker: LinkerKind::HostCc,
        capabilities: FULL_NATIVE_CAPABILITIES,
    },
    TargetSpec {
        target: Target::Wasm32UnknownUnknown,
        triple: "wasm32-unknown-unknown",
        backend: BackendKind::None,
        linker: LinkerKind::None,
        capabilities: NO_CAPABILITIES,
    },
    TargetSpec {
        target: Target::Wasm32Wasi,
        triple: "wasm32-wasi",
        backend: BackendKind::None,
        linker: LinkerKind::None,
        capabilities: WASI_CAPABILITIES,
    },
];

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
        TARGET_SPECS
            .iter()
            .find(|spec| spec.triple == value)
            .map(|spec| spec.target)
            .ok_or_else(|| Error::new(format!("unknown target `{value}`")))
    }

    pub(crate) fn spec(self) -> &'static TargetSpec {
        TARGET_SPECS
            .iter()
            .find(|spec| spec.target == self)
            .expect("every target has a spec")
    }

    pub(crate) fn provided_capabilities(self) -> BTreeSet<Capability> {
        self.spec().capabilities.iter().copied().collect()
    }

    pub(crate) fn supports_native_backend(self) -> bool {
        matches!(
            self.spec().backend,
            BackendKind::NativeAarch64Darwin | BackendKind::NativeX86_64LinuxGnu
        )
    }
}

impl PlatformSpec {
    pub(crate) fn native_for_target(target: Target) -> Self {
        Self {
            name: target.to_string(),
            provided_capabilities: target.provided_capabilities(),
            externs: ExternalRegistry::builtin_native(),
        }
    }

    pub(crate) fn js_runtime(runtime: &str) -> Self {
        Self {
            name: format!("js-{runtime}"),
            provided_capabilities: [Capability::Stdout, Capability::Stdin, Capability::Clock]
                .into_iter()
                .collect(),
            externs: ExternalRegistry::builtin_js(),
        }
    }

    pub(crate) fn wasm() -> Self {
        Self {
            name: "wasm32-unknown-unknown".to_string(),
            provided_capabilities: Target::Wasm32UnknownUnknown.provided_capabilities(),
            externs: ExternalRegistry::builtin_wasm(),
        }
    }

    pub(crate) fn wasi() -> Self {
        Self {
            name: "wasm32-wasi".to_string(),
            provided_capabilities: Target::Wasm32Wasi.provided_capabilities(),
            externs: ExternalRegistry::builtin_wasi(),
        }
    }
}

impl fmt::Display for PlatformSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.spec().triple)
    }
}
