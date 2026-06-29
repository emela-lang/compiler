use serde::Serialize;

use crate::ast::{Capability, PrimType, Type};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ExternalBindings {
    pub(crate) js_symbol: Option<String>,
    pub(crate) native: Option<NativeBinding>,
    pub(crate) wasm: Option<WasmBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NativeBinding {
    pub(crate) symbol: String,
    pub(crate) links: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WasmBinding {
    pub(crate) module: String,
    pub(crate) symbol: String,
    pub(crate) params: Vec<String>,
    pub(crate) result: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExternalFunction {
    pub(crate) path: Vec<String>,
    pub(crate) name: String,
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) effectful: bool,
    pub(crate) capabilities: Vec<Capability>,
    pub(crate) bindings: ExternalBindings,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ExternalRegistry {
    functions: Vec<ExternalFunction>,
}

impl ExternalRegistry {
    pub(crate) fn from_functions(functions: Vec<ExternalFunction>) -> Result<Self> {
        let registry = Self { functions };
        registry.check_duplicates()?;
        Ok(registry)
    }

    pub(crate) fn builtin_native() -> Self {
        Self {
            functions: vec![
                native_function(
                    &["platform", "io"],
                    "_write_stdout_utf8!",
                    vec![Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::Unit)),
                    vec![Capability::Stdout],
                    "emela_write_stdout_utf8",
                ),
                native_function(
                    &["platform", "io"],
                    "_read_stdin_utf8!",
                    Vec::new(),
                    result_type(Type::Prim(PrimType::String)),
                    vec![Capability::Stdin],
                    "emela_read_stdin_utf8",
                ),
                native_function(
                    &["platform", "clock"],
                    "_now_i32!",
                    Vec::new(),
                    Type::Prim(PrimType::I32),
                    vec![Capability::Clock],
                    "emela_now_i32",
                ),
            ],
        }
    }

    pub(crate) fn builtin_js() -> Self {
        Self {
            functions: vec![
                js_function(
                    &["platform", "io"],
                    "_write_stdout_utf8!",
                    vec![Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::Unit)),
                    vec![Capability::Stdout],
                    "__emela_write_stdout_utf8",
                ),
                js_function(
                    &["platform", "io"],
                    "_read_stdin_utf8!",
                    Vec::new(),
                    result_type(Type::Prim(PrimType::String)),
                    vec![Capability::Stdin],
                    "__emela_read_stdin_utf8",
                ),
                js_function(
                    &["platform", "clock"],
                    "_now_i32!",
                    Vec::new(),
                    Type::Prim(PrimType::I32),
                    vec![Capability::Clock],
                    "__emela_now_i32",
                ),
            ],
        }
    }

    pub(crate) fn builtin_wasm() -> Self {
        Self::builtin_wasi()
    }

    pub(crate) fn builtin_wasi() -> Self {
        Self {
            functions: vec![
                wasi_function(
                    &["platform", "io"],
                    "_write_stdout_utf8!",
                    vec![Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::Unit)),
                    vec![Capability::Stdout],
                ),
                wasi_function(
                    &["platform", "io"],
                    "_read_stdin_utf8!",
                    Vec::new(),
                    result_type(Type::Prim(PrimType::String)),
                    vec![Capability::Stdin],
                ),
                wasi_function(
                    &["platform", "io"],
                    "_write_stderr_utf8!",
                    vec![Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::Unit)),
                    vec![Capability::Stderr],
                ),
                wasi_function(
                    &["platform", "clock"],
                    "_now_i32!",
                    Vec::new(),
                    Type::Prim(PrimType::I32),
                    vec![Capability::Clock],
                ),
                wasi_function(
                    &["platform", "random"],
                    "_random_i32!",
                    Vec::new(),
                    Type::Prim(PrimType::I32),
                    vec![Capability::Random],
                ),
                wasi_function(
                    &["platform", "fs"],
                    "_read_file_utf8!",
                    vec![Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::String)),
                    vec![Capability::FileRead],
                ),
                wasi_function(
                    &["platform", "fs"],
                    "_write_file_utf8!",
                    vec![Type::Prim(PrimType::String), Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::Unit)),
                    vec![Capability::FileWrite],
                ),
                wasi_function(
                    &["platform", "env"],
                    "_get_env!",
                    vec![Type::Prim(PrimType::String)],
                    result_type(Type::Prim(PrimType::String)),
                    vec![Capability::Env],
                ),
            ],
        }
    }

    pub(crate) fn resolve_import(&self, path: &[String], name: &str) -> Option<&ExternalFunction> {
        self.functions.iter().find(|function| {
            function.name == name
                && function.path.len() == path.len()
                && function
                    .path
                    .iter()
                    .zip(path.iter())
                    .all(|(left, right)| left == right)
        })
    }

    #[cfg(test)]
    pub(crate) fn native_links(&self) -> Vec<&str> {
        let mut links = Vec::new();
        for function in &self.functions {
            let Some(binding) = &function.bindings.native else {
                continue;
            };
            for link in &binding.links {
                if !links.contains(&link.as_str()) {
                    links.push(link.as_str());
                }
            }
        }
        links
    }

    fn check_duplicates(&self) -> Result<()> {
        let mut seen = Vec::<String>::new();
        for function in &self.functions {
            let key = format_external_path(function);
            if seen.contains(&key) {
                return Err(Error::new(format!("duplicate external import `{key}`")));
            }
            seen.push(key);
        }
        Ok(())
    }
}

pub(crate) fn format_external_path(function: &ExternalFunction) -> String {
    let mut parts = function.path.clone();
    parts.push(function.name.clone());
    parts.join(".")
}

pub(crate) fn parse_manifest_type(name: &str) -> Result<Type> {
    let name = name.trim();
    match name {
        "I32" | "i32" => return Ok(Type::Prim(PrimType::I32)),
        "Bool" | "bool" => return Ok(Type::Prim(PrimType::Bool)),
        "String" | "string" => return Ok(Type::Prim(PrimType::String)),
        "Unit" | "unit" => return Ok(Type::Prim(PrimType::Unit)),
        "PlatformError" => return Ok(Type::Named("PlatformError".to_string())),
        _ => {}
    }
    if let Some(inner) = name
        .strip_prefix("Result<")
        .and_then(|value| value.strip_suffix('>'))
    {
        let args = split_manifest_type_args(inner)?;
        if args.len() != 2 {
            return Err(Error::new(format!(
                "Result manifest type expects 2 arguments, got {}",
                args.len()
            )));
        }
        return Ok(Type::Apply {
            name: "Result".to_string(),
            args: vec![parse_manifest_type(args[0])?, parse_manifest_type(args[1])?],
        });
    }
    Err(Error::new(format!(
        "unknown external manifest type `{name}`"
    )))
}

fn result_type(ok: Type) -> Type {
    Type::Apply {
        name: "Result".to_string(),
        args: vec![ok, Type::Named("PlatformError".to_string())],
    }
}

fn split_manifest_type_args(value: &str) -> Result<Vec<&str>> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in value.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::new(format!("invalid manifest type `{value}`")))?;
            }
            ',' if depth == 0 => {
                args.push(value[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    args.push(value[start..].trim());
    Ok(args)
}

fn native_function(
    path: &[&str],
    name: &str,
    params: Vec<Type>,
    ret: Type,
    capabilities: Vec<Capability>,
    symbol: &str,
) -> ExternalFunction {
    ExternalFunction {
        path: path.iter().map(|part| part.to_string()).collect(),
        name: name.to_string(),
        params,
        ret,
        effectful: true,
        capabilities,
        bindings: ExternalBindings {
            native: Some(NativeBinding {
                symbol: symbol.to_string(),
                links: vec!["emela_runtime".to_string()],
            }),
            ..ExternalBindings::default()
        },
    }
}

fn js_function(
    path: &[&str],
    name: &str,
    params: Vec<Type>,
    ret: Type,
    capabilities: Vec<Capability>,
    symbol: &str,
) -> ExternalFunction {
    ExternalFunction {
        path: path.iter().map(|part| part.to_string()).collect(),
        name: name.to_string(),
        params,
        ret,
        effectful: true,
        capabilities,
        bindings: ExternalBindings {
            js_symbol: Some(symbol.to_string()),
            ..ExternalBindings::default()
        },
    }
}

fn wasi_function(
    path: &[&str],
    name: &str,
    params: Vec<Type>,
    ret: Type,
    capabilities: Vec<Capability>,
) -> ExternalFunction {
    ExternalFunction {
        path: path.iter().map(|part| part.to_string()).collect(),
        name: name.to_string(),
        params,
        ret,
        effectful: true,
        capabilities,
        bindings: ExternalBindings::default(),
    }
}
