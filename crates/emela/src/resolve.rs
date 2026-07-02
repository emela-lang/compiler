//! Qualified-name resolution for top-level functions (spec 0018).
//!
//! After import expansion, every top-level function carries the import qualifier
//! it was brought in under (`Function::module_path`). Its *full path* is
//! `module_path + [name]`, and it may be called by any non-empty suffix of that
//! path ending at `name` — bare `f`, `mod.f`, `pkg.mod.f`, etc.
//!
//! [`FnTable`] indexes every function by all such suffixes so a call/reference
//! path resolves to exactly one function, errors as ambiguous when several
//! match, or is unknown when none match. The same table drives both the type
//! checker (which needs the resolved signature) and lowering (which needs the
//! backend emit name), so the two passes never disagree on what a path denotes.

use std::collections::HashMap;

use crate::ast::Program;

/// One resolvable top-level function and the path it can be called by.
pub(crate) struct FnEntry {
    /// Index into `Program::functions` (entries are built in that order, so
    /// `entries[i].index == i`).
    pub(crate) index: usize,
    /// The bare function name (the last path segment).
    pub(crate) name: String,
    /// The full qualified path: `module_path + [name]`. For a compilation-root
    /// function or a module-private helper this is just `[name]`.
    pub(crate) full_path: Vec<String>,
    /// Whether the function is local to the compilation root (no import
    /// qualifier). Local functions keep their bare emit name and shadow imports
    /// when a bare name is resolved (spec 0018 R6).
    pub(crate) is_local: bool,
    /// Whether the function is generic (spec 0014).
    pub(crate) is_generic: bool,
    /// The backend symbol name: the bare name when it is unique across all
    /// top-level functions, otherwise the mangled full path so that same-named
    /// functions from different modules coexist (spec 0018 Compilation Notes).
    pub(crate) emit_name: String,
}

/// The outcome of resolving a path against the table.
pub(crate) enum Resolved<'a> {
    /// No function matches the path.
    None,
    /// Exactly one function matches.
    One(&'a FnEntry),
    /// Several functions match — the call site must qualify further (spec 0018
    /// R5). Carries the candidates so the diagnostic can list them.
    Ambiguous(Vec<&'a FnEntry>),
}

pub(crate) struct FnTable {
    entries: Vec<FnEntry>,
    /// Every non-empty suffix of every function's full path → matching entry
    /// indices.
    by_suffix: HashMap<Vec<String>, Vec<usize>>,
}

impl FnTable {
    pub(crate) fn build(program: &Program) -> FnTable {
        // A bare name shared by more than one function collides and must be
        // mangled (for the imported side). Count occurrences first.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for function in &program.functions {
            *counts.entry(function.name.as_str()).or_default() += 1;
        }

        let mut entries = Vec::with_capacity(program.functions.len());
        for (index, function) in program.functions.iter().enumerate() {
            let is_local = function.module_path.is_empty();
            let mut full_path = function.module_path.clone();
            full_path.push(function.name.clone());
            let collides = counts.get(function.name.as_str()).copied().unwrap_or(0) > 1;
            // A unique bare name (or a local function) keeps its bare symbol so
            // single imports such as `std.io.print` emit unchanged. Only a
            // colliding imported function is mangled to its full path.
            let emit_name = if collides && !is_local {
                full_path.join("__")
            } else {
                function.name.clone()
            };
            entries.push(FnEntry {
                index,
                name: function.name.clone(),
                full_path,
                is_local,
                is_generic: !function.type_params.is_empty(),
                emit_name,
            });
        }

        let mut by_suffix: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            let path = &entry.full_path;
            for start in 0..path.len() {
                by_suffix.entry(path[start..].to_vec()).or_default().push(i);
            }
        }

        FnTable { entries, by_suffix }
    }

    /// The backend emit name of the function at `index` in `Program::functions`.
    pub(crate) fn emit_name(&self, index: usize) -> &str {
        &self.entries[index].emit_name
    }

    /// Resolves a (possibly qualified) call/reference path to a function
    /// (spec 0018). For a bare name, a local function shadows imported
    /// candidates (R6); otherwise a single suffix match resolves and several
    /// matches are ambiguous.
    pub(crate) fn resolve(&self, path: &[String]) -> Resolved<'_> {
        let Some(indices) = self.by_suffix.get(path) else {
            return Resolved::None;
        };
        if path.len() == 1 {
            // Bare name: an entry-local function shadows any imports of the same
            // name. Only a multi-segment full path can match a longer path, so
            // this special case only applies to bare names.
            let locals: Vec<&FnEntry> = indices
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|entry| entry.is_local)
                .collect();
            match locals.as_slice() {
                [only] => return Resolved::One(only),
                [_, _, ..] => return Resolved::Ambiguous(locals),
                [] => {}
            }
        }
        match indices.as_slice() {
            [] => Resolved::None,
            [only] => Resolved::One(&self.entries[*only]),
            many => Resolved::Ambiguous(many.iter().map(|&i| &self.entries[i]).collect()),
        }
    }
}

/// Renders a candidate's full path for an ambiguity diagnostic, e.g.
/// `std.int.to_string`.
pub(crate) fn display_path(segments: &[String]) -> String {
    segments.join(".")
}
