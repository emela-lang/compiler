use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ast::{Extern, Function, Import, Program};
use crate::error::{Diagnostic, Error, Result};
use crate::parser::parse_program;

#[derive(Debug, Clone)]
pub(crate) struct PackageSource {
    name: String,
    source_root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct PackageManifest {
    name: String,
    source: String,
}

pub(crate) fn load_packages(paths: &[PathBuf]) -> Result<Vec<PackageSource>> {
    let mut packages = Vec::new();
    let mut names = HashSet::new();
    for path in paths {
        let manifest_path = path.join("emela-package.json");
        let manifest_source = fs::read_to_string(&manifest_path).map_err(|err| {
            Error::new(format!(
                "failed to read package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        let manifest: PackageManifest = serde_json::from_str(&manifest_source).map_err(|err| {
            Error::new(format!(
                "failed to parse package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        if !names.insert(manifest.name.clone()) {
            return Err(Error::new(format!(
                "duplicate package `{}` in --package arguments",
                manifest.name
            )));
        }
        packages.push(PackageSource {
            name: manifest.name,
            source_root: path.join(manifest.source),
        });
    }
    Ok(packages)
}

pub(crate) fn resolve_imports(
    input: &Path,
    program: Program,
    packages: &[PackageSource],
) -> Result<Program> {
    let mut resolver = ImportResolver {
        packages,
        loaded: HashMap::new(),
        resolving: HashSet::new(),
        emitted: HashSet::new(),
    };
    resolver.expand_program(input, program)
}

struct ImportResolver<'a> {
    packages: &'a [PackageSource],
    loaded: HashMap<PathBuf, Program>,
    resolving: HashSet<PathBuf>,
    emitted: HashSet<PathBuf>,
}

impl ImportResolver<'_> {
    fn expand_program(&mut self, source_path: &Path, mut program: Program) -> Result<Program> {
        let imports = std::mem::take(&mut program.imports);
        let mut imported_functions = Vec::new();
        let mut imported_externs = Vec::new();
        for import in imports {
            let (functions, externs) = self.resolve_import(source_path, &import)?;
            imported_functions.extend(functions);
            imported_externs.extend(externs);
        }
        imported_functions.extend(program.functions);
        program.functions = imported_functions;
        imported_externs.extend(program.externs);
        program.externs = imported_externs;
        Ok(program)
    }

    fn resolve_import(
        &mut self,
        source_path: &Path,
        import: &Import,
    ) -> Result<(Vec<Function>, Vec<Extern>)> {
        let Some((module_file, module_name, item_name)) =
            self.resolve_module_file(source_path, import)?
        else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown package").label(
                import.span.clone(),
                format!("cannot resolve `{}`", import.path[0]),
            )));
        };
        let module = self.load_module(&module_file)?;
        if module.module.as_deref() != Some(module_name.as_str()) {
            return Err(Error::diagnostic(Diagnostic::new("Module mismatch").label(
                import.span.clone(),
                format!(
                    "expected module `{module_name}` in `{}`",
                    module_file.display()
                ),
            )));
        }
        let imported = module
            .functions
            .iter()
            .find(|function| function.name == item_name);
        match imported {
            Some(function) if function.is_public => {
                let canonical = module_file.canonicalize().map_err(|err| {
                    Error::new(format!(
                        "failed to resolve module `{}`: {err}",
                        module_file.display()
                    ))
                })?;
                if self.emitted.insert(canonical) {
                    // Stamp each of this module's own public functions with the
                    // qualifier the user wrote (everything before the item name),
                    // e.g. `["std", "int"]` for `import std.int.to_string`. They
                    // then become callable as `int.to_string` / `std.int.to_string`
                    // as well as the bare name (spec 0018). Private helpers and
                    // already-stamped transitively-imported functions are left
                    // unqualified, so they keep only their bare-name behavior.
                    let qualifier = import.path[..import.path.len() - 1].to_vec();
                    let mut functions = module.functions.clone();
                    for function in &mut functions {
                        if function.is_public && function.module_path.is_empty() {
                            function.module_path = qualifier.clone();
                        }
                    }
                    Ok((functions, module.externs.clone()))
                } else {
                    Ok((Vec::new(), Vec::new()))
                }
            }
            Some(function) => Err(Error::diagnostic(Diagnostic::new("Private import").label(
                function.name_span.clone(),
                format!("`{item_name}` is not public"),
            ))),
            None => Err(Error::diagnostic(Diagnostic::new("Unknown import").label(
                import.span.clone(),
                format!("`{item_name}` is not defined"),
            ))),
        }
    }

    fn resolve_module_file(
        &self,
        source_path: &Path,
        import: &Import,
    ) -> Result<Option<(PathBuf, String, String)>> {
        let item_name = import.item_name().to_string();
        if let Some(package) = self
            .packages
            .iter()
            .find(|package| package.name == import.path[0])
        {
            if import.path.len() < 3 {
                return Err(Error::diagnostic(
                    Diagnostic::new("Invalid package import").label(
                        import.span.clone(),
                        "package imports must name a module and item",
                    ),
                ));
            }
            let module_parts = &import.path[1..import.path.len() - 1];
            let module_path = join_module_path(&package.source_root, module_parts);
            return Ok(Some((module_path, module_parts.join("."), item_name)));
        }

        let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
        let module_parts = &import.path[..import.path.len() - 1];
        let module_path = join_module_path(base_dir, module_parts);
        Ok(Some((module_path, module_parts.join("."), item_name)))
    }

    fn load_module(&mut self, path: &Path) -> Result<Program> {
        let canonical = path.canonicalize().map_err(|err| {
            Error::new(format!(
                "failed to resolve module `{}`: {err}",
                path.display()
            ))
        })?;
        if let Some(program) = self.loaded.get(&canonical) {
            return Ok(program.clone());
        }
        if !self.resolving.insert(canonical.clone()) {
            return Err(Error::new(format!(
                "cyclic import involving `{}`",
                canonical.display()
            )));
        }
        let source = fs::read_to_string(&canonical).map_err(|err| {
            Error::new(format!(
                "failed to read module `{}`: {err}",
                canonical.display()
            ))
        })?;
        let label = canonical.display().to_string();
        let program = parse_program(&label, &source)?;
        let program = self.expand_program(&canonical, program)?;
        self.resolving.remove(&canonical);
        self.loaded.insert(canonical.clone(), program.clone());
        Ok(program)
    }
}

fn join_module_path(root: &Path, parts: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path.set_extension("emel");
    path
}
