use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::ast::{Capability, Program, TopLevelItem};
use crate::error::{Error, Result};
use crate::external::{ExternalFunction, ExternalRegistry};
use crate::platform::{PlatformSpec, Target};
use crate::typecheck::{CheckMode, TypedProgram};

use super::{EmitOptions, BACKEND_ABI_VERSION};

#[derive(Debug)]
pub(crate) struct ExternalBackend {
    manifest: ExternalBackendManifest,
    externs: ExternalRegistry,
    target: Option<Target>,
}

#[derive(Debug, Deserialize)]
struct ExternalBackendManifest {
    name: String,
    backend: String,
    abi_version: u32,
    command: Vec<String>,
    runtime: Option<String>,
    target: Option<String>,
    capabilities: Vec<Capability>,
    externs: Vec<ManifestExternalFunction>,
}

#[derive(Debug, Deserialize)]
struct ManifestExternalFunction {
    path: Vec<String>,
    name: String,
    params: Vec<String>,
    #[serde(rename = "return")]
    ret: String,
    effectful: bool,
    capabilities: Vec<Capability>,
    #[serde(default)]
    bindings: ManifestBindings,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestBindings {
    js: Option<ManifestJsBinding>,
    native: Option<ManifestNativeBinding>,
    wasm: Option<ManifestWasmBinding>,
}

#[derive(Debug, Deserialize)]
struct ManifestJsBinding {
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct ManifestNativeBinding {
    symbol: String,
    #[serde(default, rename = "link")]
    links: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestWasmBinding {
    module: String,
    symbol: String,
    params: Vec<String>,
    #[serde(default)]
    result: Option<String>,
}

#[derive(Serialize)]
struct BackendRequest<'a> {
    abi_version: u32,
    profile: &'a str,
    backend: &'a str,
    runtime: Option<&'a str>,
    target: Option<String>,
    mode: &'a str,
    program: &'a Program,
    typed: &'a TypedProgram,
    imports: Vec<&'a ExternalFunction>,
}

#[derive(Deserialize)]
struct BackendResponse {
    artifact: Option<String>,
    diagnostics: Option<Vec<String>>,
}

impl ExternalBackend {
    pub(super) fn from_manifest_path(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| {
            Error::new(format!(
                "failed to read backend manifest `{}`: {err}",
                path.display()
            ))
        })?;
        Self::from_manifest_json(&source)
    }

    pub(crate) fn from_manifest_json(source: &str) -> Result<Self> {
        let manifest: ExternalBackendManifest = serde_json::from_str(source)
            .map_err(|err| Error::new(format!("invalid backend manifest JSON: {err}")))?;
        if manifest.abi_version != BACKEND_ABI_VERSION {
            return Err(Error::new(format!(
                "backend `{}` uses ABI version {}, expected {}",
                manifest.name, manifest.abi_version, BACKEND_ABI_VERSION
            )));
        }
        if manifest.command.is_empty() {
            return Err(Error::new(format!(
                "backend `{}` manifest must include a non-empty command",
                manifest.name
            )));
        }
        if manifest.backend.is_empty() {
            return Err(Error::new(format!(
                "backend `{}` manifest must include a backend kind",
                manifest.name
            )));
        }
        let target = manifest.target.as_deref().map(Target::parse).transpose()?;
        let externs = ExternalRegistry::from_functions(
            manifest
                .externs
                .iter()
                .map(ManifestExternalFunction::to_external)
                .collect::<Result<Vec<_>>>()?,
        )?;
        Ok(Self {
            manifest,
            externs,
            target,
        })
    }

    pub(super) fn target(&self) -> Option<Target> {
        self.target
    }

    pub(super) fn platform(&self) -> PlatformSpec {
        PlatformSpec {
            name: self.manifest.name.clone(),
            provided_capabilities: self.manifest.capabilities.iter().copied().collect(),
            externs: self.externs.clone(),
        }
    }

    pub(super) fn emit(
        &self,
        platform: &PlatformSpec,
        program: &Program,
        typed: &TypedProgram,
        options: EmitOptions<'_>,
    ) -> Result<()> {
        if options.output.is_some() {
            return Err(Error::new("external backend does not support --output"));
        }
        let Some(path) = options.artifact else {
            return Err(Error::new("external backend requires --artifact"));
        };
        let mut child = Command::new(&self.manifest.command[0])
            .args(&self.manifest.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                Error::new(format!(
                    "failed to start backend `{}`: {err}",
                    self.manifest.name
                ))
            })?;
        let imports = collect_imports(platform, program);
        let request = BackendRequest {
            abi_version: BACKEND_ABI_VERSION,
            profile: &self.manifest.name,
            backend: &self.manifest.backend,
            runtime: self.manifest.runtime.as_deref(),
            target: options.target.map(|target| target.to_string()),
            mode: if options.mode == CheckMode::Library {
                "library"
            } else {
                "executable"
            },
            program,
            typed,
            imports,
        };
        let payload = serde_json::to_vec(&request)
            .map_err(|err| Error::new(format!("failed to encode backend request: {err}")))?;
        child
            .stdin
            .as_mut()
            .expect("backend stdin was piped")
            .write_all(&payload)
            .map_err(|err| Error::new(format!("failed to write backend request: {err}")))?;
        drop(child.stdin.take());

        let output = child
            .wait_with_output()
            .map_err(|err| Error::new(format!("failed to wait for backend: {err}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::new(format!(
                "backend `{}` failed: {}",
                self.manifest.name,
                stderr.trim()
            )));
        }
        let response: BackendResponse = serde_json::from_slice(&output.stdout).map_err(|err| {
            Error::new(format!(
                "backend `{}` returned invalid JSON: {err}",
                self.manifest.name
            ))
        })?;
        if let Some(diagnostics) = response.diagnostics.filter(|items| !items.is_empty()) {
            return Err(Error::new(diagnostics.join("\n")));
        }
        let artifact = response.artifact.ok_or_else(|| {
            Error::new(format!(
                "backend `{}` response missing `artifact`",
                self.manifest.name
            ))
        })?;
        fs::write(path, artifact).map_err(|err| {
            Error::new(format!(
                "failed to write backend output `{}`: {err}",
                path.display()
            ))
        })
    }
}

impl ManifestExternalFunction {
    fn to_external(&self) -> Result<ExternalFunction> {
        Ok(ExternalFunction {
            path: self.path.clone(),
            name: self.name.clone(),
            params: self
                .params
                .iter()
                .map(|ty| crate::external::parse_manifest_type(ty))
                .collect::<Result<Vec<_>>>()?,
            ret: crate::external::parse_manifest_type(&self.ret)?,
            effectful: self.effectful,
            capabilities: self.capabilities.clone(),
            bindings: crate::external::ExternalBindings {
                js_symbol: self
                    .bindings
                    .js
                    .as_ref()
                    .map(|binding| binding.symbol.clone()),
                native: self.bindings.native.as_ref().map(|binding| {
                    crate::external::NativeBinding {
                        symbol: binding.symbol.clone(),
                        links: binding.links.clone(),
                    }
                }),
                wasm: self
                    .bindings
                    .wasm
                    .as_ref()
                    .map(|binding| crate::external::WasmBinding {
                        module: binding.module.clone(),
                        symbol: binding.symbol.clone(),
                        params: binding.params.clone(),
                        result: binding.result.clone(),
                    }),
            },
        })
    }
}

fn collect_imports<'a>(platform: &'a PlatformSpec, program: &Program) -> Vec<&'a ExternalFunction> {
    let mut imports = Vec::new();
    for item in &program.items {
        let TopLevelItem::Import(import) = item else {
            continue;
        };
        if let Some(function) = platform.externs.resolve_import(&import.path, &import.name) {
            imports.push(function);
        }
    }
    imports
}
