//! The Emela compiler frontend and CLI driver.
//!
//! This crate lexes, parses, resolves imports, and type-checks Emela source,
//! then lowers it to the `emela-codegen` IR and hands that IR to a selected
//! [`emela_codegen::Backend`].

mod api;
mod ast;
mod driver;
mod error;
mod external;
mod imports;
mod lexer;
mod lower;
mod parser;
mod resolve;
mod typecheck;

pub use api::{check_source, compile_source, ir_source};
pub use driver::run;
pub use error::{Error, Result};

// Re-exported so embedders can name backend outputs without depending on
// `emela-codegen` directly.
pub use emela_codegen::{Artifact, ArtifactKind, EmitMode};
