//! Codegen errors.
//!
//! Unlike the frontend `Error`, these carry no source spans: the IR has no
//! span information, so a backend can only report messages and diagnostics.

use std::fmt;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, BackendError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendError {
    pub message: String,
    pub diagnostics: Vec<String>,
}

impl BackendError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }

    pub fn with(message: impl Into<String>, diagnostics: Vec<String>) -> Self {
        Self {
            message: message.into(),
            diagnostics,
        }
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        for diagnostic in &self.diagnostics {
            write!(f, "\n  {diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BackendError {}
