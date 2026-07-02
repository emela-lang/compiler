//! A registry of available backends, looked up by name.

use crate::backend::{Backend, Tier};

#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<Box<dyn Backend>>,
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, backend: Box<dyn Backend>) {
        self.backends.push(backend);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Backend> {
        self.backends
            .iter()
            .find(|backend| backend.name() == name)
            .map(|backend| backend.as_ref())
    }

    /// All registered backends as `(name, tier)`, in registration order.
    pub fn list(&self) -> Vec<(&str, Tier)> {
        self.backends
            .iter()
            .map(|backend| (backend.name(), backend.tier()))
            .collect()
    }
}
