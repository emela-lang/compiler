//! The Emela compiler frontend and CLI driver.
//!
//! This crate lexes, parses, resolves imports, and type-checks Emela source,
//! then lowers it to the `emela-codegen` IR and hands that IR to a selected
//! [`emela_codegen::Backend`].

mod ast;
mod driver;
mod error;
mod external;
mod imports;
mod lexer;
mod lower;
mod parser;
mod typecheck;

pub use driver::run;
pub use error::{Error, Result};
