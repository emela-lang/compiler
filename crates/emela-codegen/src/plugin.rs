//! The external-process backend protocol.
//!
//! A backend can live in another process (or another language). The compiler
//! serializes a [`PluginRequest`] (containing the IR) to the process's stdin as
//! JSON and reads a [`PluginResponse`] from its stdout. [`BackendDescriptor`]
//! mirrors the `backend.json` files under `backends/`; the host crate turns a
//! descriptor with a `command` into a [`crate::Backend`].

use serde::{Deserialize, Serialize};

use crate::backend::{ArtifactKind, EmitMode, Tier};
use crate::ir::IrProgram;

/// A backend descriptor, as found in `backends/<name>/backend.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendDescriptor {
    pub name: String,
    pub backend: String,
    pub abi_version: u32,
    /// In-process backends name their implementation here instead of a command.
    #[serde(default)]
    pub builtin: Option<String>,
    /// External backends are invoked as this command (argv).
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub externs: Vec<ExternDescriptor>,
}

/// A platform extern a backend provides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternDescriptor {
    pub path: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default, rename = "return")]
    pub ret: Option<String>,
    #[serde(default)]
    pub effectful: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub bindings: serde_json::Value,
}

/// The request sent to an external backend process on stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRequest {
    pub ir: IrProgram,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub mode: EmitMode,
}

/// The response an external backend writes to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PluginResponse {
    Ok { kind: ArtifactKind, bytes: Vec<u8> },
    Error { diagnostics: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IrExpr, IrFunction, Type};

    fn tiny_ir() -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret: Type::Int,
                throws: None,
                effects: Default::default(),
                body: IrExpr::Int(1),
            }],
        }
    }

    #[test]
    fn request_round_trips() {
        let request = PluginRequest {
            ir: tiny_ir(),
            target: Some("wasm32-wasi".into()),
            runtime: Some("iwasm".into()),
            mode: EmitMode::Default,
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: PluginRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.target.as_deref(), Some("wasm32-wasi"));
        assert_eq!(back.ir.functions.len(), 1);
    }

    #[test]
    fn response_uses_a_status_tag() {
        let ok = PluginResponse::Ok {
            kind: ArtifactKind::JsSource,
            bytes: b"hi".to_vec(),
        };
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("\"status\":\"ok\""), "{json}");
        let back: PluginResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, PluginResponse::Ok { .. }));

        let err: PluginResponse =
            serde_json::from_str(r#"{"status":"error","diagnostics":["boom"]}"#).unwrap();
        assert!(matches!(err, PluginResponse::Error { .. }));
    }

    #[test]
    fn descriptor_parses_builtin_form() {
        // Mirrors backends/js-node/backend.json.
        let json = r#"{
            "name": "js-node",
            "backend": "js",
            "abi_version": 1,
            "builtin": "js",
            "runtime": "node",
            "capabilities": ["Stdout"],
            "externs": [
              {
                "path": ["platform", "io"],
                "name": "_write_stdout_utf8!",
                "params": ["String"],
                "return": "Result<Unit, PlatformError>",
                "effectful": true,
                "capabilities": ["Stdout"],
                "bindings": { "js": { "symbol": "__emela_write_stdout_utf8" } }
              }
            ]
        }"#;
        let descriptor: BackendDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(descriptor.name, "js-node");
        assert_eq!(descriptor.builtin.as_deref(), Some("js"));
        assert!(descriptor.command.is_none());
        assert_eq!(descriptor.externs.len(), 1);
        assert_eq!(
            descriptor.externs[0].ret.as_deref(),
            Some("Result<Unit, PlatformError>")
        );
    }

    #[test]
    fn descriptor_parses_external_command_form() {
        let json = r#"{
            "name": "example",
            "backend": "wasm",
            "abi_version": 1,
            "command": ["example-emela-backend", "--flag"],
            "tier": "Tier3",
            "target": "wasm32-wasi"
        }"#;
        let descriptor: BackendDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(
            descriptor.command.as_deref(),
            Some(["example-emela-backend".to_string(), "--flag".to_string()].as_slice())
        );
        assert_eq!(descriptor.tier, Some(Tier::Tier3));
    }
}
