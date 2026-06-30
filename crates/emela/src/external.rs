//! External-process backends, selected with `--backend PATH` where `PATH` is a
//! `backend.json` descriptor that declares a `command`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use emela_codegen::{
    Artifact, Backend, BackendDescriptor, BackendError, BackendOptions, IrProgram, PluginRequest,
    PluginResponse, Result as CodegenResult, Tier,
};

use crate::error::{Error, Result};

/// A backend that runs in another process, speaking the JSON IR protocol.
pub(crate) struct ExternalBackend {
    name: String,
    tier: Tier,
    command: Vec<String>,
    target: Option<String>,
    runtime: Option<String>,
}

/// Loads a descriptor file and builds an [`ExternalBackend`] from it.
pub(crate) fn load_backend(path: &Path) -> Result<ExternalBackend> {
    let source = std::fs::read_to_string(path).map_err(|err| {
        Error::new(format!(
            "failed to read backend `{}`: {err}",
            path.display()
        ))
    })?;
    let descriptor: BackendDescriptor = serde_json::from_str(&source)
        .map_err(|err| Error::new(format!("invalid backend `{}`: {err}", path.display())))?;
    let command = descriptor.command.ok_or_else(|| {
        Error::new(format!(
            "backend `{}` has no `command`; only external (command) backends can be loaded from a path",
            descriptor.name
        ))
    })?;
    if command.is_empty() {
        return Err(Error::new(format!(
            "backend `{}` has an empty `command`",
            descriptor.name
        )));
    }
    Ok(ExternalBackend {
        name: descriptor.name,
        tier: descriptor.tier.unwrap_or(Tier::Tier3),
        command,
        target: descriptor.target,
        runtime: descriptor.runtime,
    })
}

impl Backend for ExternalBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn tier(&self) -> Tier {
        self.tier
    }

    fn compile(&self, ir: &IrProgram, options: &BackendOptions) -> CodegenResult<Artifact> {
        let request = PluginRequest {
            ir: ir.clone(),
            target: options.target.clone().or_else(|| self.target.clone()),
            runtime: options.runtime.clone().or_else(|| self.runtime.clone()),
            mode: options.mode,
        };
        let payload = serde_json::to_vec(&request)
            .map_err(|err| BackendError::new(format!("failed to encode IR request: {err}")))?;

        let mut child = Command::new(&self.command[0])
            .args(&self.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                BackendError::new(format!(
                    "failed to start backend `{}` ({}): {err}",
                    self.name, self.command[0]
                ))
            })?;

        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(&payload)
            .map_err(|err| {
                BackendError::new(format!(
                    "failed to send IR to backend `{}`: {err}",
                    self.name
                ))
            })?;

        let output = child.wait_with_output().map_err(|err| {
            BackendError::new(format!("backend `{}` did not complete: {err}", self.name))
        })?;
        if !output.status.success() {
            return Err(BackendError::with(
                format!("backend `{}` exited with {}", self.name, output.status),
                vec![String::from_utf8_lossy(&output.stderr).into_owned()],
            ));
        }

        let response: PluginResponse = serde_json::from_slice(&output.stdout).map_err(|err| {
            BackendError::with(
                format!("backend `{}` returned invalid JSON: {err}", self.name),
                vec![String::from_utf8_lossy(&output.stderr).into_owned()],
            )
        })?;
        match response {
            PluginResponse::Ok { kind, bytes } => Ok(Artifact { kind, bytes }),
            PluginResponse::Error { diagnostics } => Err(BackendError::with(
                format!("backend `{}` reported errors", self.name),
                diagnostics,
            )),
        }
    }
}

/// Whether a `--backend` value names a descriptor file rather than a built-in.
pub(crate) fn is_descriptor_path(value: &str) -> bool {
    value.ends_with(".json") || Path::new(value).is_file()
}
