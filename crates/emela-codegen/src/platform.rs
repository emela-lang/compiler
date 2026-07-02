//! The platform-function interface (spec 0013).
//!
//! Platform functions are the language-defined set of operations that produce
//! capability effects (spec 0009). The compiler implements none of them; a
//! backend supplies the implementation for the subset it provides. Emela source
//! references them with `extern fn` and may not name a backend, so source stays
//! backend-independent.

use crate::types::Type;

/// One entry of the platform interface: a qualified name, a signature, and the
/// capability effect it produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformFn {
    pub path: Vec<String>,
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    pub capability: String,
}

impl PlatformFn {
    /// The qualified name used as the ABI key, e.g. `io.write_stdout`.
    pub fn canonical(&self) -> String {
        let mut out = self.path.join(".");
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&self.name);
        out
    }
}

/// The normative platform interface. The MVP set returns `Unit` and reports no
/// failure (spec 0013); failing operations are added once `Result`/`Option`
/// land.
pub fn platform_interface() -> Vec<PlatformFn> {
    vec![
        PlatformFn {
            path: vec!["io".to_string()],
            name: "write_stdout".to_string(),
            params: vec![Type::String],
            ret: Type::Unit,
            capability: "io".to_string(),
        },
        PlatformFn {
            path: vec!["io".to_string()],
            name: "write_stderr".to_string(),
            params: vec![Type::String],
            ret: Type::Unit,
            capability: "io".to_string(),
        },
        PlatformFn {
            path: vec!["clock".to_string()],
            name: "monotonic_seconds".to_string(),
            params: vec![],
            ret: Type::Int,
            capability: "clock".to_string(),
        },
    ]
}

/// Looks a platform function up by its canonical name (e.g. `io.write_stdout`).
pub fn lookup(canonical: &str) -> Option<PlatformFn> {
    platform_interface()
        .into_iter()
        .find(|entry| entry.canonical() == canonical)
}
