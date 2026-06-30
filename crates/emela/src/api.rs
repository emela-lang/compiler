//! Filesystem-free, string-based entry points for embedding the Emela
//! compiler in-process — for example the WebAssembly playground that runs the
//! compiler in the browser.
//!
//! These mirror the `check`, `ir`, and `build` CLI commands but take the source
//! as a string and never touch the filesystem. Imports are not resolved (there
//! is no package search path), so a source that uses `import` will fail to
//! resolve; everything else compiles exactly as the CLI would.

use emela_codegen::{Artifact, EmitMode};

use crate::driver;
use crate::error::Result;

/// Type-checks `source`, returning `Ok(())` when it is well-typed.
///
/// `label` is the name shown in diagnostics (e.g. `"playground.emel"`).
pub fn check_source(label: &str, source: &str) -> Result<()> {
    driver::check_source(label, source)
}

/// Lowers `source` to the codegen IR and renders it as text.
pub fn ir_source(label: &str, source: &str) -> Result<String> {
    driver::ir_source(label, source)
}

/// Compiles `source` with the named built-in backend (e.g. `"js-node"`,
/// `"wasm-wasi"`). `mode` selects the primary artifact or its textual form
/// (e.g. WAT for the wasm backend).
pub fn compile_source(
    label: &str,
    source: &str,
    backend: &str,
    mode: EmitMode,
) -> Result<Artifact> {
    driver::compile_source(label, source, backend, mode)
}
