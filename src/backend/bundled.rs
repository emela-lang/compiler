pub(crate) const JS_NODE_RUNTIME: &str = include_str!("../../backends/js-node/runtime.js");
pub(crate) const JS_BUN_RUNTIME: &str = include_str!("../../backends/js-bun/runtime.js");
pub(crate) const NATIVE_RUNTIME_C: &str =
    include_str!("../../backends/native-runtime/emela_runtime.c");
pub(crate) const WASI_RUNTIME_WAT: &str = include_str!("../../backends/wasm-wasi/runtime.wat");

pub(crate) fn js_runtime(platform_name: &str) -> Option<&'static str> {
    match platform_name {
        "js-node" => Some(JS_NODE_RUNTIME),
        "js-bun" => Some(JS_BUN_RUNTIME),
        _ => Some(JS_NODE_RUNTIME),
    }
}
