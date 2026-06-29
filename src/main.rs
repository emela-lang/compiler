mod ast;
mod backend;
mod driver;
mod error;
mod external;
mod lexer;
mod package;
mod parser;
mod platform;
mod typecheck;

fn main() {
    if let Err(error) = driver::run() {
        eprintln!("{}", error.render());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use serde::Deserialize;

    use crate::ast::{Capability, FunctionType, PrimType, Type};
    use crate::backend::{
        emit_js_artifact, emit_js_library_artifact, emit_native_assembly,
        emit_native_assembly_for_platform, emit_wasi_artifact, emit_wasi_library_artifact,
        emit_wasm_artifact, emit_wasm_library_artifact, native_link_args, Backend, EmitOptions,
        ExternalBackend,
    };
    use crate::driver::{
        compile_internal_source_for_platform, compile_source, compile_source_for_platform,
        compile_source_for_platform_with_mode, compile_source_for_target,
    };
    use crate::external::{ExternalBindings, ExternalFunction, ExternalRegistry};
    use crate::platform::PlatformSpec;
    use crate::platform::Target;
    use crate::typecheck::CheckMode;

    #[test]
    fn accepts_empty_main() {
        let (_, typed) = compile_source("fn main() -> Unit {\n}\n").unwrap();
        assert_eq!(typed.functions[0].name, "main");
        assert_eq!(typed.functions[0].ret, Type::Prim(PrimType::Unit));
    }

    #[test]
    fn infers_i32_function() {
        let (_, typed) = compile_source(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
"#,
        )
        .unwrap();
        let add = typed
            .functions
            .iter()
            .find(|function| function.name == "add")
            .unwrap();
        assert_eq!(
            add.params,
            vec![Type::Prim(PrimType::I32), Type::Prim(PrimType::I32)]
        );
        assert_eq!(add.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn supports_pipeline_function_calls() {
        let (_, typed) = compile_source(
            r#"
fn add(value: I32, by: I32) -> I32 {
  value + by
}

fn double(value: I32) -> I32 {
  value * 2
}

fn main() -> I32 {
  20
    |> add(1)
    |> double()
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn pipeline_example_compiles() {
        for backend_name in [
            "js-node",
            "native-aarch64-apple-darwin",
            "native-x86_64-unknown-linux-gnu",
        ] {
            let backend = Backend::parse(backend_name).unwrap();
            compile_source_for_platform(
                include_str!("../examples/pipeline.emel"),
                &backend.platform(),
            )
            .unwrap();
        }
    }

    #[test]
    fn rejects_pipeline_stage_without_call() {
        let error = compile_source(
            r#"
fn add_one(value: I32) -> I32 {
  value + 1
}

fn main() -> I32 {
  41 |> add_one
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("pipeline stage must be an explicit function call"));
    }

    #[test]
    fn parser_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  add(1,
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Expected an expression"));
        assert!(rendered.contains("--> <test>:3:9"));
        assert!(rendered.contains("3 |   add(1,"));
        assert!(rendered.contains("Hint: Add a value"));
    }

    #[test]
    fn accepts_return_annotation_and_exits_with_main_i32() {
        let source = r#"
fn add(x: i32, y: i32) -> i32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
"#;
        let (program, typed) =
            compile_source_for_target(source, Target::Aarch64AppleDarwin).unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));

        let assembly = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains(".globl _main"));
    }

    #[test]
    fn library_mode_allows_sources_without_main() {
        let source = r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}
"#;
        compile_source(source).unwrap_err();
        let platform = PlatformSpec::native_for_target(Target::Aarch64AppleDarwin);
        let (_, typed) =
            compile_source_for_platform_with_mode(source, &platform, CheckMode::Library).unwrap();
        assert_eq!(typed.functions.len(), 1);
        assert_eq!(typed.functions[0].name, "add");
    }

    #[test]
    fn supports_parameter_and_local_type_annotations() {
        let (_, typed) = compile_source(
            r#"
fn add(x: I32, y: I32) -> I32 {
  sum: I32 = x + y
  sum
}

fn main() -> I32 {
  add(20, 22)
}
"#,
        )
        .unwrap();
        let add = typed
            .functions
            .iter()
            .find(|function| function.name == "add")
            .unwrap();
        assert_eq!(
            add.params,
            vec![Type::Prim(PrimType::I32), Type::Prim(PrimType::I32)]
        );
        assert_eq!(add.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn rejects_mismatched_parameter_type_annotation() {
        let error = compile_source(
            r#"
fn negate(value: Bool) -> Bool {
  value == 0
}

fn main() -> Bool {
  negate(true)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_mismatched_local_type_annotation() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  value: Bool = 42
  0
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn type_mismatch_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  true
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Type mismatch"));
        assert!(rendered.contains("--> <test>:3:3"));
        assert!(rendered.contains("3 |   true"));
        assert!(rendered.contains("Expected `I32`, but found `Bool`"));
    }

    #[test]
    fn unknown_type_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn main() -> Missing {
  ()
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Unknown type"));
        assert!(rendered.contains("--> <test>:2:14"));
        assert!(rendered.contains("I cannot find a type named `Missing`"));
    }

    #[test]
    fn generic_type_arity_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn main() -> Result<I32> {
  Ok(1)
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Wrong number of type arguments"));
        assert!(rendered.contains("`Result` expects 2 type argument(s), but got 1"));
    }

    #[test]
    fn unknown_function_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  nope(1)
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Unknown function"));
        assert!(rendered.contains("I cannot find a function named `nope`"));
        assert!(rendered.contains("3 |   nope(1)"));
    }

    #[test]
    fn call_arity_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(1)
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Wrong number of arguments"));
        assert!(rendered.contains("function `add` expects 2 argument(s), got 1"));
    }

    #[test]
    fn non_exhaustive_match_diagnostic_includes_source_excerpt() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  match true {
    true -> 1
  }
}
"#,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("error: Match is not exhaustive"));
        assert!(rendered.contains("This match does not cover every possible value"));
    }

    #[test]
    fn rejects_missing_parameter_type_annotation() {
        let error = compile_source(
            r#"
fn add(x, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(1, 2)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must have a type annotation"));
    }

    #[test]
    fn rejects_missing_return_type_annotation() {
        let error = compile_source(
            r#"
fn main() {
  ()
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("must have a return type annotation"));
    }

    #[test]
    fn rejects_missing_local_type_annotation() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  value = 42
  value
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must have a type annotation"));
    }

    #[test]
    fn supports_function_values_in_bindings_and_parameters() {
        let (_, typed) = compile_source(
            r#"
fn add_one(value: I32) -> I32 {
  value + 1
}

fn apply(value: I32, f: fn(I32) -> I32) -> I32 {
  f(value)
}

fn main() -> I32 {
  op: fn(I32) -> I32 = add_one
  apply(41, op)
}
"#,
        )
        .unwrap();
        let apply = typed
            .functions
            .iter()
            .find(|function| function.name == "apply")
            .unwrap();
        assert_eq!(
            apply.params[1],
            Type::Function(FunctionType {
                params: vec![Type::Prim(PrimType::I32)],
                ret: Box::new(Type::Prim(PrimType::I32)),
                effectful: false,
            })
        );
    }

    #[test]
    fn supports_anonymous_function_arguments_with_capture() {
        let (_, typed) = compile_source(
            r#"
fn apply(value: I32, f: fn(I32) -> I32) -> I32 {
  f(value)
}

fn main() -> I32 {
  offset: I32 = 1
  apply(41, fn(value: I32) -> value + offset)
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn anonymous_function_can_use_generic_outer_type_parameter() {
        let (_, typed) = compile_source(
            r#"
enum Option<T> {
  Some(T)
  None
}

fn fold<T, U>(opt: Option<T>, default: U, f: fn(T) -> U) -> U {
  match opt {
    Some(value) -> f(value)
    None -> default
  }
}

fn map<T, U>(opt: Option<T>, f: fn(T) -> U) -> Option<U> {
  fold(opt, None, fn(value: T) -> Some(f(value)))
}

fn main() -> Option<I32> {
  map(Some(41), fn(value: I32) -> value + 1)
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(
            main.ret,
            Type::Apply {
                name: "Option".to_string(),
                args: vec![Type::Prim(PrimType::I32)],
            }
        );
    }

    #[test]
    fn js_backend_emits_anonymous_function() {
        let platform = PlatformSpec::js_runtime("node");
        let (program, typed) = compile_source_for_platform(
            r#"
fn apply(value: I32, f: fn(I32) -> I32) -> I32 {
  f(value)
}

fn main() -> I32 {
  offset: I32 = 1
  apply(41, fn(value: I32) -> value + offset)
}
"#,
            &platform,
        )
        .unwrap();
        let js = emit_js_artifact(&platform, &program, &typed).unwrap();
        assert!(js.contains("((value) => (value + offset))"));
    }

    #[test]
    fn native_backend_rejects_anonymous_functions() {
        let (program, typed) = compile_source(
            r#"
fn main() -> I32 {
  f: fn(I32) -> I32 = fn(value: I32) -> value + 1
  0
}
"#,
        )
        .unwrap();
        let error = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap_err();
        assert!(error.to_string().contains("anonymous functions"));
    }

    #[test]
    fn supports_functions_returning_functions() {
        let (_, typed) = compile_source(
            r#"
fn double(value: I32) -> I32 {
  value * 2
}

fn identity(value: I32) -> I32 {
  value
}

fn choose_transform(flag: Bool) -> fn(I32) -> I32 {
  match flag {
    true -> double
    false -> identity
  }
}

fn main() -> I32 {
  transform: fn(I32) -> I32 = choose_transform(true)
  transform(21)
}
"#,
        )
        .unwrap();
        let choose_transform = typed
            .functions
            .iter()
            .find(|function| function.name == "choose_transform")
            .unwrap();
        assert_eq!(
            choose_transform.ret,
            Type::Function(FunctionType {
                params: vec![Type::Prim(PrimType::I32)],
                ret: Box::new(Type::Prim(PrimType::I32)),
                effectful: false,
            })
        );
    }

    #[test]
    fn rejects_function_value_type_mismatch() {
        let error = compile_source(
            r#"
fn is_zero(value: I32) -> Bool {
  value == 0
}

fn main() -> I32 {
  op: fn(I32) -> I32 = is_zero
  op(41)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_effectful_function_value_call_from_pure_function() {
        let error = compile_source(
            r#"
fn tick!() -> Unit {
  ()
}

fn call(callback: fn!() -> Unit) -> Unit {
  callback()
}

fn main() -> Unit {
  call(tick!)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pure function"));
    }

    #[test]
    fn native_backend_rejects_function_values() {
        let (program, typed) = compile_source(
            r#"
fn add_one(value: I32) -> I32 {
  value + 1
}

fn main() -> I32 {
  op: fn(I32) -> I32 = add_one
  op(41)
}
"#,
        )
        .unwrap();
        let error = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not support function value"));
    }

    #[test]
    fn emits_assembly_match_expression() {
        let (program, typed) = compile_source_for_target(
            "fn main() -> I32 { match true { true -> 1 false -> 0 } }",
            Target::Aarch64AppleDarwin,
        )
        .unwrap();
        let assembly = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("mov w10, #1"));
        assert!(assembly.contains("mov w10, #0"));
        assert!(assembly.contains("cmp w9, w10"));
    }

    #[test]
    fn emits_x86_64_linux_assembly() {
        let (program, typed) = compile_source_for_target(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
"#,
            Target::X86_64UnknownLinuxGnu,
        )
        .unwrap();
        let assembly =
            emit_native_assembly(Target::X86_64UnknownLinuxGnu, &program, &typed).unwrap();
        assert!(assembly.contains(".globl main"));
        assert!(assembly.contains("add:"));
        assert!(assembly.contains("movq %rdi, -8(%rbp)"));
        assert!(assembly.contains("call add"));
        assert!(assembly.contains("addl %r9d, %eax"));
    }

    #[test]
    fn emits_wasm_add_function() {
        let (program, _typed) = compile_source(
            r#"
fn main() -> I32 {
  1.add(2)
}
"#,
        )
        .unwrap();
        let wasm = emit_wasm_artifact(&program).unwrap();
        assert!(wasm.contains("(module"));
        assert!(wasm.contains("(func $main"));
        assert!(wasm.contains("i32.const 1"));
        assert!(wasm.contains("i32.const 2"));
        assert!(wasm.contains("i32.add"));
        assert!(wasm.contains("(export \"main\" (func $main)"));
    }

    #[test]
    fn emits_wasm_match_expression() {
        let (program, _typed) =
            compile_source("fn main() -> I32 { match true { true -> 1 false -> 0 } }").unwrap();
        let wasm = emit_wasm_artifact(&program).unwrap();
        assert!(wasm.contains("(if (result i32)"));
        assert!(wasm.contains("i32.const 1"));
        assert!(wasm.contains("i32.eq"));
        assert!(wasm.contains("i32.const 0"));
    }

    #[test]
    fn emits_wasm_library_without_entrypoint_export() {
        let (program, _typed) = compile_source_for_platform_with_mode(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x.add(y)
}
"#,
            &PlatformSpec::wasm(),
            CheckMode::Library,
        )
        .unwrap();
        let wasm = emit_wasm_library_artifact(&program).unwrap();
        assert!(wasm.contains("(func $add"));
    }

    #[test]
    fn wasm_backend_parse_and_compile() {
        let backend = Backend::parse("wasm").unwrap();
        assert!(matches!(
            backend.target(),
            Some(Target::Wasm32UnknownUnknown)
        ));
        let platform = backend.platform();
        assert_eq!(platform.name, "wasm32-unknown-unknown");
    }

    #[test]
    fn wasm_backend_emits_enum_match() {
        let (program, _typed) = compile_source_for_target(
            r#"
fn main() -> I32 {
  match Ok(41) {
    Ok(value) -> value.add(1)
    Err(_) -> 0
  }
}
"#,
            Target::Wasm32UnknownUnknown,
        )
        .unwrap();
        let wasm = emit_wasm_artifact(&program).unwrap();
        assert!(wasm.contains("(if (result i32)"));
        assert!(wasm.contains("i64.const 4294967295"));
        assert!(wasm.contains("i64.and"));
        assert!(wasm.contains("i32.wrap_i64"));
    }

    #[test]
    fn wasm_backend_custom_enum_struct_payload() {
        let (program, _typed) = compile_source_for_target(
            r#"
struct Error {
  code: I32
}

enum Checked {
  Good(I32)
  Bad(Error)
}

fn check(value: I32) -> Checked {
  match value == 0 {
    true -> Bad(Error { code: 7 })
    false -> Good(value)
  }
}

fn main() -> I32 {
  match check(0) {
    Good(v) -> v
    Bad(error) -> error.code
  }
}
"#,
            Target::Wasm32UnknownUnknown,
        )
        .unwrap();
        let wasm = emit_wasm_artifact(&program).unwrap();
        assert!(wasm.contains("(module"));
        assert!(wasm.contains("(func $check"));
        assert!(wasm.contains("(func $main"));
        assert!(wasm.contains("(export \"check\" (func $check)"));
        assert!(wasm.contains("(export \"main\" (func $main)"));
        assert!(wasm.contains("i64.extend_i32_u"));
        assert!(wasm.contains("i64.const 32"));
        assert!(wasm.contains("i64.shl"));
        assert!(wasm.contains("i64.or"));
        assert!(wasm.contains("i64.const 4294967295"));
        assert!(wasm.contains("i64.and"));
        assert!(wasm.contains("i32.wrap_i64"));
        assert!(wasm.contains("i64.shr_u"));
    }

    #[test]
    fn wasm_backend_rejects_function_values() {
        let error = compile_source_for_target(
            r#"
fn main() -> I32 {
  add(1, 2)
}
"#,
            Target::Wasm32UnknownUnknown,
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Unknown function"));
    }

    #[test]
    fn wasm_backend_rejects_anonymous_functions() {
        let (program, _typed) = compile_source(
            r#"
fn main() -> I32 {
  f: fn(I32) -> I32 = fn(value: I32) -> value + 1
  0
}
"#,
        )
        .unwrap();
        let error = emit_wasm_artifact(&program).unwrap_err();
        assert!(error.to_string().contains("anonymous function"));
    }

    #[test]
    fn wasi_backend_parse_and_compile() {
        let backend = Backend::parse("wasm-wasi").unwrap();
        assert!(matches!(backend.target(), Some(Target::Wasm32Wasi)));
        let platform = backend.platform();
        assert_eq!(platform.name, "wasm32-wasi");
    }

    #[test]
    fn wasi_backend_emits_module_header() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) =
            compile_source_for_target("fn main() -> Unit { () }", Target::Wasm32Wasi).unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(module"));
        assert!(wat.contains("(memory (export \"memory\") 2)"));
        assert!(wat.contains("(export \"_start\" (func $_start))"));
    }

    #[test]
    fn wasi_backend_emits_fd_write_stdout() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_write\""));
        assert!(wat.contains("$emela_write_stdout_utf8"));
        assert!(wat.contains("$emela_strlen"));
    }

    #[test]
    fn wasi_backend_emits_fd_write_stderr() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.io.write_stderr_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stderr_utf8!("error")
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_write\""));
        assert!(wat.contains("$emela_write_stderr_utf8"));
    }

    #[test]
    fn wasi_backend_emits_fd_read_stdin() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.io.read_stdin_utf8!

fn main!() -> Result<String, PlatformError> {
  read_stdin_utf8!()
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_read\""));
        assert!(wat.contains("$emela_read_stdin_utf8"));
    }

    #[test]
    fn wasi_backend_emits_clock_time_get() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.clock.now_i32!

fn main!() -> I32 {
  now_i32!()
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"clock_time_get\""));
        assert!(wat.contains("$emela_now_i32"));
    }

    #[test]
    fn wasi_backend_emits_random_get() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.random.random_i32!

fn main!() -> I32 {
  random_i32!()
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"random_get\""));
        assert!(wat.contains("$emela_random_i32"));
    }

    #[test]
    fn wasi_backend_emits_path_open_read() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.fs.read_file_utf8!

fn main!() -> Result<String, PlatformError> {
  read_file_utf8!("test.txt")
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"path_open\""));
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_read\""));
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_close\""));
        assert!(wat.contains("$emela_read_file_utf8"));
    }

    #[test]
    fn wasi_backend_emits_path_open_write() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.fs.write_file_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_file_utf8!("test.txt", "content")
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"path_open\""));
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_write\""));
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"fd_close\""));
        assert!(wat.contains("$emela_write_file_utf8"));
    }

    #[test]
    fn wasi_backend_emits_environ_get() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_target(
            r#"
import std.env.get_env!

fn main!() -> Result<String, PlatformError> {
  get_env!("HOME")
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"environ_sizes_get\""));
        assert!(wat.contains("(import \"wasi_snapshot_preview1\" \"environ_get\""));
        assert!(wat.contains("$emela_get_env"));
    }

    #[test]
    fn wasi_backend_library_mode_has_no_start() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) = compile_source_for_platform_with_mode(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x.add(y)
}
"#,
            &platform,
            CheckMode::Library,
        )
        .unwrap();
        let wat = emit_wasi_library_artifact(&program, &platform).unwrap();
        assert!(!wat.contains("$_start"));
        assert!(wat.contains("(func $add"));
    }

    #[test]
    fn wasi_backend_emits_start_entrypoint() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) =
            compile_source_for_target("fn main() -> I32 { 42 }", Target::Wasm32Wasi).unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("$_start"));
        assert!(wat.contains("call $main"));
        assert!(wat.contains("$wasi_proc_exit"));
    }

    #[test]
    fn wasi_backend_stdlib_wrapper_matches_result() {
        let (_, typed) = compile_source_for_target(
            r#"
import std.io.write_stdout_utf8!

fn main!() -> I32 {
  match write_stdout_utf8!("hello") {
    Ok(_) -> 0
    Err(_) -> 1
  }
}
"#,
            Target::Wasm32Wasi,
        )
        .unwrap();
        let main = typed.functions.iter().find(|f| f.name == "main!").unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));
        assert!(main.capabilities.contains(&Capability::Stdout));
    }

    #[test]
    fn wasi_backend_no_externs_emits_basic_module() {
        let platform = PlatformSpec::wasi();
        let (program, _typed) =
            compile_source_for_target("fn main() -> I32 { 42 }", Target::Wasm32Wasi).unwrap();
        let wat = emit_wasi_artifact(&program, &platform).unwrap();
        assert!(wat.contains("(module"));
        assert!(wat.contains("(memory (export \"memory\") 2)"));
        assert!(wat.contains("(export \"_start\" (func $_start))"));
    }

    #[test]
    fn rejects_match_pattern_type_mismatch() {
        let error =
            compile_source("fn main() -> I32 { match 1 { true -> 2 false -> 3 } }").unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_effectful_call_from_pure_function() {
        let error = compile_source(
            r#"
fn tick!() -> Unit {
  ()
}

fn main() -> Unit {
  tick!()
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pure function"));
    }

    #[test]
    fn allows_main_effect_boundary_with_capability() {
        let (program, typed) = compile_source(
            r#"
#[requires(Stdout)]
fn print_i32!(value: I32) -> Unit {
  ()
}

fn main!() -> I32 {
  print_i32!(42)
  0
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main!")
            .unwrap();
        assert!(main.effectful);
        assert_eq!(main.capabilities.len(), 1);

        let assembly = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains(".globl _main"));
        assert!(assembly.contains("requires Stdout"));
    }

    #[test]
    fn rejects_requires_outside_declared_capabilities() {
        let error = compile_source(
            r#"
#[requires(Stdout)]
fn print_i32!(value: I32) -> Unit {
  ()
}

#[requires()]
fn main!() -> Unit {
  print_i32!(42)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside #[requires"));
    }

    #[test]
    fn lowers_primitive_method_calls() {
        let (program, typed) = compile_source(
            r#"
fn main() -> I32 {
  20.add(22)
}
"#,
        )
        .unwrap();
        let assembly = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("add w0, w9, w0"));
    }

    #[test]
    fn supports_struct_enum_and_result_pattern_matching() {
        let (program, typed) = compile_source(
            r#"
struct Error {
  code: I32
}

enum Checked {
  Good(I32)
  Bad(Error)
}

fn checked(value: I32) -> Checked {
  match value == 0 {
    true -> Bad(Error { code: 7 })
    false -> Good(value)
  }
}

fn main() -> I32 {
  match checked(0) {
    Good(value) -> value
    Bad(error) -> error.code
  }
}
"#,
        )
        .unwrap();
        let checked = typed
            .functions
            .iter()
            .find(|function| function.name == "checked")
            .unwrap();
        assert_eq!(checked.ret, Type::Named("Checked".to_string()));

        let assembly = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("orr x0, x0, #0"));
        assert!(assembly.contains("orr x0, x0, #1"));
        assert!(assembly.contains("lsr x9, x9, #32"));
    }

    #[test]
    fn rejects_capability_missing_from_native_target() {
        let error = compile_source(
            r#"
#[requires(HostImport)]
fn host_call!() -> Unit {
  ()
}

fn main!() -> Unit {
  host_call!()
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not provide"));
    }

    #[test]
    fn target_capability_set_is_checked() {
        let source = r#"
#[requires(Stdout)]
fn print_i32!(value: I32) -> Unit {
  ()
}

fn main!() -> Unit {
  print_i32!(42)
}
"#;
        compile_source_for_target(source, Target::Wasm32Wasi).unwrap();
        let error = compile_source_for_target(source, Target::Wasm32UnknownUnknown).unwrap_err();
        assert!(error
            .to_string()
            .contains("platform `wasm32-unknown-unknown` does not provide"));
    }

    #[test]
    fn imported_external_capability_set_is_checked() {
        let source = r#"
import std.io.write_stdout_utf8!

fn main!() -> Unit {
  write_stdout_utf8!("hello")
  ()
}
"#;
        compile_source_for_target(source, Target::Wasm32Wasi).unwrap();
        let error = compile_source_for_target(source, Target::Wasm32UnknownUnknown).unwrap_err();
        assert!(error
            .to_string()
            .contains("platform `wasm32-unknown-unknown` does not provide"));
    }

    #[test]
    fn rejects_imported_effectful_call_from_pure_function() {
        let error = compile_source_for_target(
            r#"
import std.io.write_stdout_utf8!

fn main() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"#,
            Target::Aarch64AppleDarwin,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pure function"));
    }

    #[test]
    fn imported_external_function_lowers_to_native_binding() {
        let platform = PlatformSpec {
            name: "native-runtime".to_string(),
            provided_capabilities: [Capability::Stdout].into_iter().collect(),
            externs: ExternalRegistry::builtin_native(),
        };
        let (program, typed) = compile_internal_source_for_platform(
            r#"
import platform.io._write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  "hello" |> _write_stdout_utf8!()
}
"#,
            &platform,
        )
        .unwrap();
        let darwin = emit_native_assembly_for_platform(
            Target::Aarch64AppleDarwin,
            &platform,
            &program,
            &typed,
        )
        .unwrap();
        assert!(darwin.contains("    str x0, [sp, #0]\n"));
        assert!(darwin.contains("    ldr x0, [sp, #0]\n"));
        assert!(darwin.contains("    bl _emela_write_stdout_utf8\n"));

        let linux = emit_native_assembly_for_platform(
            Target::X86_64UnknownLinuxGnu,
            &platform,
            &program,
            &typed,
        )
        .unwrap();
        assert!(linux.contains("    movq %rax, 0(%rsp)\n"));
        assert!(linux.contains("    movq 0(%rsp), %rdi\n"));
        assert!(linux.contains("    call emela_write_stdout_utf8\n"));
    }

    #[test]
    fn imports_stdlib_wrapper_for_native_codegen() {
        let (program, typed) = compile_source_for_target(
            r#"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  "hello" |> write_stdout_utf8!()
}
"#,
            Target::Aarch64AppleDarwin,
        )
        .unwrap();
        assert!(program
            .functions()
            .iter()
            .any(|function| function.name == "write_stdout_utf8!"));
        let assembly = emit_native_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("    bl _write_stdout_utf8_effect\n"));
        assert!(assembly.contains("    bl _emela_write_stdout_utf8\n"));
    }

    #[test]
    fn native_codegen_rejects_missing_binding() {
        let platform = platform_with_externs(
            "missing-native",
            [Capability::Stdout],
            vec![platform_extern(
                "_write_stdout_utf8!",
                ExternalBindings::default(),
            )],
        );
        let (program, typed) = compile_internal_source_for_platform(
            r#"
import platform.io._write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  _write_stdout_utf8!("hello")
}
"#,
            &platform,
        )
        .unwrap();
        let error = emit_native_assembly_for_platform(
            Target::Aarch64AppleDarwin,
            &platform,
            &program,
            &typed,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not have a native binding"));
    }

    #[test]
    fn non_native_target_does_not_emit_native_assembly() {
        let (program, typed) =
            compile_source_for_target("fn main() -> Unit {}", Target::Wasm32UnknownUnknown)
                .unwrap();
        let error =
            emit_native_assembly(Target::Wasm32UnknownUnknown, &program, &typed).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not have a native assembly backend"));
    }

    #[test]
    fn builtin_js_backend_exposes_platform_externs() {
        let platform = PlatformSpec::js_runtime("node");
        let function = platform
            .externs
            .resolve_import(
                &["platform".to_string(), "io".to_string()],
                "_write_stdout_utf8!",
            )
            .unwrap();
        assert_eq!(function.params, vec![Type::Prim(PrimType::String)]);
        assert_eq!(
            function.ret,
            Type::Apply {
                name: "Result".to_string(),
                args: vec![
                    Type::Prim(PrimType::Unit),
                    Type::Named("PlatformError".to_string())
                ]
            }
        );
        assert_eq!(
            function.bindings.js_symbol.as_deref(),
            Some("__emela_write_stdout_utf8")
        );
    }

    #[test]
    fn builtin_backend_descriptors_match_registry_surface() {
        let native = builtin_descriptor(include_str!(
            "../backends/native-aarch64-apple-darwin/backend.json"
        ));
        assert_eq!(native.name, "native-aarch64-apple-darwin");
        assert_eq!(native.backend, "native");
        assert_eq!(native.builtin.as_deref(), Some("native"));
        assert_eq!(native.abi_version, crate::backend::BACKEND_ABI_VERSION);
        assert_eq!(native.target.as_deref(), Some("aarch64-apple-darwin"));
        assert!(native.capabilities.contains(&Capability::Stdout));
        assert!(native
            .externs
            .iter()
            .any(|external| external.name == "_write_stdout_utf8!"));

        let js = builtin_descriptor(include_str!("../backends/js-node/backend.json"));
        assert_eq!(js.name, "js-node");
        assert_eq!(js.backend, "js");
        assert_eq!(js.builtin.as_deref(), Some("js"));
        assert_eq!(js.abi_version, crate::backend::BACKEND_ABI_VERSION);
        assert_eq!(js.runtime.as_deref(), Some("node"));
        assert_eq!(
            js.capabilities,
            vec![Capability::Stdout, Capability::Stdin, Capability::Clock]
        );
        assert!(js
            .externs
            .iter()
            .any(|external| external.name == "_now_i32!"));
    }

    #[test]
    fn builtin_native_backend_exposes_link_args() {
        let registry = ExternalRegistry::builtin_native();
        let function = registry
            .resolve_import(
                &["platform".to_string(), "io".to_string()],
                "_write_stdout_utf8!",
            )
            .unwrap();
        let native = function.bindings.native.as_ref().unwrap();
        assert_eq!(native.symbol, "emela_write_stdout_utf8");
        assert_eq!(native.links, vec!["emela_runtime"]);
        assert_eq!(registry.native_links(), vec!["emela_runtime"]);
        let platform = PlatformSpec {
            name: "native-runtime".to_string(),
            provided_capabilities: [Capability::Stdout].into_iter().collect(),
            externs: registry,
        };
        let (program, _) = compile_internal_source_for_platform(
            r#"
import platform.io._write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  _write_stdout_utf8!("hello")
}
"#,
            &platform,
        )
        .unwrap();
        let link_args = native_link_args(&platform, &program);
        assert_eq!(link_args.len(), 1);
        assert!(link_args[0].ends_with("backends/native-runtime/emela_runtime.c"));
    }

    #[test]
    fn backend_manifest_rejects_invalid_native_binding() {
        let error = ExternalBackend::from_manifest_json(
            r#"
{
	  "name": "bad",
  "backend": "native",
  "abi_version": 1,
  "command": ["backend"],
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "_write_stdout_utf8!",
      "params": ["String"],
      "return": "Result<Unit, PlatformError>",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "native": {
          "link": ["emela_runtime"]
        }
      }
    }
  ]
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing field `symbol`"));
    }

    #[test]
    fn backend_manifest_rejects_unknown_type() {
        let error = ExternalBackend::from_manifest_json(
            r#"
{
	  "name": "bad",
  "backend": "js",
  "abi_version": 1,
  "command": ["backend"],
  "capabilities": [],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print!",
      "params": ["Bytes"],
      "return": "Unit",
      "effectful": true,
      "capabilities": [],
      "bindings": {}
    }
  ]
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown external manifest type"));
    }

    #[test]
    fn backend_manifest_rejects_duplicate_import() {
        let error = ExternalBackend::from_manifest_json(
            r#"
{
	  "name": "bad",
  "backend": "js",
  "abi_version": 1,
  "command": ["backend"],
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "_write_stdout_utf8!",
      "params": ["String"],
      "return": "Result<Unit, PlatformError>",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {}
    },
    {
      "path": ["platform", "io"],
      "name": "_write_stdout_utf8!",
      "params": ["String"],
      "return": "Result<Unit, PlatformError>",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {}
    }
  ]
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate external import"));
    }

    #[test]
    fn backend_platform_capability_set_is_checked() {
        let source = r#"
import std.io.write_stdout_utf8!

fn main!() -> Unit {
  write_stdout_utf8!("hello")
  ()
}
"#;
        let node = js_platform_with_stdout("_write_stdout_utf8!", true);
        compile_source_for_platform(source, &node).unwrap();

        let no_stdout = platform_with_externs(
            "no-stdout",
            [],
            vec![js_stdout_extern("_write_stdout_utf8!")],
        );
        let error = compile_source_for_platform(source, &no_stdout).unwrap_err();
        assert!(error
            .to_string()
            .contains("platform `no-stdout` does not provide"));
    }

    #[test]
    fn emits_js_main_and_external_binding() {
        let platform = js_platform_with_stdout("_write_stdout_utf8!", true);
        let (program, typed) = compile_internal_source_for_platform(
            r#"
import platform.io._write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  _write_stdout_utf8!("hello")
}
"#,
            &platform,
        )
        .unwrap();
        let js = emit_js_artifact(&platform, &program, &typed).unwrap();
        assert!(js.contains("function main_effect()"));
        assert!(js.contains("__emela_write_stdout_utf8(\"hello\");"));
        assert!(js.contains("function __emela_write_stdout_utf8(value)"));
        assert!(js.contains("const __emela_result = main_effect();"));
    }

    #[test]
    fn emits_js_library_without_entrypoint() {
        let platform = PlatformSpec::native_for_target(Target::Aarch64AppleDarwin);
        let (program, typed) = compile_source_for_platform_with_mode(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}
"#,
            &platform,
            CheckMode::Library,
        )
        .unwrap();
        let js = emit_js_library_artifact(&platform, &program, &typed).unwrap();
        assert!(js.contains("function add(x, y)"));
        assert!(!js.contains("__emela_result"));
    }

    #[test]
    fn js_backend_lowers_builtin_result_variants() {
        let platform = PlatformSpec::js_runtime("node");
        let (program, typed) = compile_source_for_platform(
            r#"
fn main() -> I32 {
  match Ok(41) {
    Ok(value) -> value + 1
    Err(_) -> 0
  }
}
"#,
            &platform,
        )
        .unwrap();
        let js = emit_js_artifact(&platform, &program, &typed).unwrap();
        assert!(js.contains("{ tag: 0, value: 41 }"));
        assert!(js.contains("__match.tag === 0"));
        assert!(js.contains("__match.tag === 1"));
        assert!(!js.contains("Ok(41)"));
    }

    #[test]
    fn imports_stdlib_wrapper_for_js_codegen() {
        let platform = PlatformSpec::js_runtime("node");
        let (program, typed) = compile_source_for_platform(
            r#"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"#,
            &platform,
        )
        .unwrap();
        let js = emit_js_artifact(&platform, &program, &typed).unwrap();
        assert!(js.contains("function write_stdout_utf8_effect(value)"));
        assert!(js.contains("__emela_write_stdout_utf8(value);"));
        assert!(js.contains("function __emela_write_stdout_utf8(value)"));
        assert!(js.contains("write_stdout_utf8_effect(\"hello\");"));
    }

    #[test]
    fn stdlib_import_only_requires_used_platform_externs() {
        let platform = js_platform_with_stdout("_write_stdout_utf8!", true);
        let (program, typed) = compile_source_for_platform(
            r#"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"#,
            &platform,
        )
        .unwrap();
        assert!(program.items.iter().any(|item| matches!(
            item,
            crate::ast::TopLevelItem::Import(import) if import.name == "_write_stdout_utf8!"
        )));
        assert!(!program.items.iter().any(|item| matches!(
            item,
            crate::ast::TopLevelItem::Import(import) if import.name == "_read_stdin_utf8!"
        )));
        emit_js_artifact(&platform, &program, &typed).unwrap();
    }

    #[test]
    fn user_source_cannot_import_platform_package() {
        let error = compile_source_for_target(
            r#"
import platform.io._write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  _write_stdout_utf8!("hello")
}
"#,
            Target::Aarch64AppleDarwin,
        )
        .unwrap_err();
        assert!(error.to_string().contains("only available to stdlib"));
    }

    #[test]
    fn supports_result_based_platform_stdin_wrapper() {
        let platform = PlatformSpec::js_runtime("node");
        let (program, typed) = compile_source_for_platform(
            r#"
import std.io.read_stdin_utf8!

fn main!() -> Result<String, PlatformError> {
  read_stdin_utf8!()
}
"#,
            &platform,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main!")
            .unwrap();
        assert_eq!(
            main.ret,
            Type::Apply {
                name: "Result".to_string(),
                args: vec![
                    Type::Prim(PrimType::String),
                    Type::Named("PlatformError".to_string()),
                ],
            }
        );
        assert!(main.capabilities.contains(&Capability::Stdin));
        assert!(program.items.iter().any(|item| matches!(
            item,
            crate::ast::TopLevelItem::Import(import) if import.name == "_read_stdin_utf8!"
        )));
    }

    #[test]
    fn matches_generic_result_from_platform_call() {
        let platform = PlatformSpec::js_runtime("node");
        let (_, typed) = compile_source_for_platform(
            r#"
import std.io.write_stdout_utf8!

fn main!() -> I32 {
  match write_stdout_utf8!("hello") {
    Ok(_) -> 0
    Err(_) -> 1
  }
}
"#,
            &platform,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main!")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));
        assert!(main.capabilities.contains(&Capability::Stdout));
    }

    #[test]
    fn stdlib_import_rejects_missing_used_platform_extern() {
        let platform = js_platform_with_stdout("_write_stdout_utf8!", true);
        let error = compile_source_for_platform(
            r#"
import std.io.read_stdin_utf8!

fn main!() -> Result<String, PlatformError> {
  read_stdin_utf8!()
}
"#,
            &platform,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("unknown external import `platform.io._read_stdin_utf8!`"));
    }

    #[test]
    fn external_backend_process_emits_artifact() {
        let backend = Backend::External(
            ExternalBackend::from_manifest_json(
                r#"
{
	  "name": "fake",
  "backend": "js",
  "abi_version": 1,
  "command": ["/bin/sh", "-c", "cat >/dev/null; printf '{\"artifact\":\"plugin-ok\"}'"],
  "capabilities": [],
  "externs": []
}
"#,
            )
            .unwrap(),
        );
        let platform = backend.platform();
        let (program, typed) = compile_source_for_platform(
            r#"
fn main() -> Unit {
}
"#,
            &platform,
        )
        .unwrap();
        let output = env::temp_dir().join(format!("emela-plugin-test-{}.txt", std::process::id()));
        backend
            .emit(
                &platform,
                &program,
                &typed,
                EmitOptions {
                    mode: CheckMode::Executable,
                    input: std::path::Path::new("test.emel"),
                    output: None,
                    artifact: Some(&output),
                    target: Some(Target::Wasm32UnknownUnknown),
                },
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "plugin-ok");
        let _ = fs::remove_file(output);
    }

    #[test]
    fn external_backend_rejects_abi_mismatch() {
        let error = ExternalBackend::from_manifest_json(
            r#"
{
	  "name": "bad",
  "backend": "js",
  "abi_version": 2,
  "command": ["backend"],
  "capabilities": [],
  "externs": []
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("ABI version 2"));
    }

    #[test]
    fn js_codegen_rejects_missing_binding() {
        let platform = platform_with_externs(
            "missing-js",
            [Capability::Stdout],
            vec![platform_extern(
                "_write_stdout_utf8!",
                ExternalBindings::default(),
            )],
        );
        let (program, typed) = compile_internal_source_for_platform(
            r#"
import platform.io._write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  _write_stdout_utf8!("hello")
}
"#,
            &platform,
        )
        .unwrap();
        let error = emit_js_artifact(&platform, &program, &typed).unwrap_err();
        assert!(error.to_string().contains("does not have a js binding"));
    }

    #[test]
    fn generic_function_instantiates_per_call() {
        let (_, typed) = compile_source(
            r#"
fn id<T>(value: T) -> T {
  value
}

fn main() -> Bool {
  first: I32 = id(42)
  id(true)
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::Bool));
    }

    #[test]
    fn generic_function_reuses_repeated_type_parameter() {
        let (_, typed) = compile_source(
            r#"
fn choose<T>(left: T, right: T) -> T {
  left
}

fn main() -> I32 {
  choose(1, 2)
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn generic_function_rejects_conflicting_repeated_type_parameter() {
        let error = compile_source(
            r#"
fn choose<T>(left: T, right: T) -> T {
  left
}

fn main() -> I32 {
  choose(1, true)
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting type argument for `T`"));
    }

    #[test]
    fn generic_function_body_cannot_assume_concrete_type_without_bounds() {
        let error = compile_source(
            r#"
fn add_one<T>(value: T) -> T {
  value + 1
}

fn main() -> I32 {
  add_one(41)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn generic_function_return_context_infers_nested_type_argument() {
        let (_, typed) = compile_source(
            r#"
fn ok<T, E>(value: T) -> Result<T, E> {
  Ok(value)
}

fn main() -> Result<I32, PlatformError> {
  ok(42)
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(
            main.ret,
            Type::Apply {
                name: "Result".to_string(),
                args: vec![
                    Type::Prim(PrimType::I32),
                    Type::Named("PlatformError".to_string())
                ],
            }
        );
    }

    #[test]
    fn result_map_infers_constructor_types_from_match_arms() {
        let (_, typed) = compile_source_for_platform_with_mode(
            r#"
fn map<T, E, U>(result: Result<T, E>, f: fn(T) -> U) -> Result<U, E> {
  match result {
    Ok(value) -> Ok(f(value))
    Err(err) -> Err(err)
  }
}
"#,
            &PlatformSpec::js_runtime("node"),
            CheckMode::Library,
        )
        .unwrap();
        let map = typed
            .functions
            .iter()
            .find(|function| function.name == "map")
            .unwrap();
        assert_eq!(
            map.ret,
            Type::Apply {
                name: "Result".to_string(),
                args: vec![
                    Type::GenericParam("U".to_string()),
                    Type::GenericParam("E".to_string())
                ],
            }
        );
    }

    #[test]
    fn generic_function_accepts_explicit_multiple_type_arguments() {
        let (_, typed) = compile_source(
            r#"
fn ok<T, E>(value: T) -> Result<T, E> {
  Ok(value)
}

fn main() -> Result<I32, PlatformError> {
  ok<I32, PlatformError>(42)
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(
            main.ret,
            Type::Apply {
                name: "Result".to_string(),
                args: vec![
                    Type::Prim(PrimType::I32),
                    Type::Named("PlatformError".to_string())
                ],
            }
        );
    }

    #[test]
    fn generic_function_rejects_explicit_type_argument_arity_mismatch() {
        let error = compile_source(
            r#"
fn ok<T, E>(value: T) -> Result<T, E> {
  Ok(value)
}

fn main() -> Result<I32, PlatformError> {
  ok<I32>(42)
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("expects 2 type argument(s), got 1"));
    }

    #[test]
    fn generic_function_rejects_explicit_type_argument_conflict() {
        let error = compile_source(
            r#"
fn id<T>(value: T) -> T {
  value
}

fn main() -> I32 {
  id<Bool>(42)
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting type argument for `T`"));
    }

    #[test]
    fn stdlib_option_source_checks_as_library() {
        let platform = PlatformSpec::native_for_target(Target::Aarch64AppleDarwin);
        compile_source_for_platform_with_mode(
            r#"
enum Option<T> {
  Some(T)
  None
}

fn map<T, U>(opt: Option<T>, f: fn(T) -> U) -> Option<U> {
  match opt {
    Some(value) -> Some(f(value))
    None -> None
  }
}
"#,
            &platform,
            CheckMode::Library,
        )
        .unwrap();
    }

    #[test]
    fn option_apply_wrong_function_return_points_at_function_parameter() {
        let platform = PlatformSpec::native_for_target(Target::Aarch64AppleDarwin);
        let source = r#"
enum Option<T> {
  Some(T)
  None
}

fn map<T, U>(opt: Option<T>, f: fn(T) -> U) -> Option<U> {
  match opt {
    Some(value) -> Some(f(value))
    None -> None
  }
}

fn unwrap_or<T>(opt: Option<T>, default: T) -> T {
  match opt {
    Some(value) -> value
    None -> default
  }
}

fn flat<T>(opt: Option<Option<T>>) -> Option<T> {
  unwrap_or(opt, None)
}

fn apply<T, U>(opt: Option<T>, f: fn(T) -> Option<T>) -> Option<U> {
  map(opt, f) |> flat()
}
"#;
        let error = compile_source_for_platform_with_mode(source, &platform, CheckMode::Library)
            .unwrap_err();
        let rendered = error.to_string();
        assert!(
            rendered.contains("fn apply<T, U>(opt: Option<T>, f: fn(T) -> Option<T>) -> Option<U>")
        );
        assert!(rendered.contains("This is returned from `apply`"));
    }

    #[test]
    fn explicit_type_argument_parser_keeps_less_than_expression() {
        let (_, typed) = compile_source(
            r#"
fn main() -> Bool {
  1 < 2
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::Bool));
    }

    #[test]
    fn generic_function_return_context_rejects_wrong_outer_type() {
        let error = compile_source(
            r#"
struct Box<T> {
  value: T
}

fn ok<T, E>(value: T) -> Result<T, E> {
  Ok(value)
}

fn main() -> Box<I32> {
  ok(42)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn main_function_must_not_be_generic() {
        let error = compile_source(
            r#"
fn main<T>() -> Unit {
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must not be generic"));
    }

    fn js_platform_with_stdout(name: &str, provides_stdout: bool) -> PlatformSpec {
        let capabilities = if provides_stdout {
            vec![Capability::Stdout]
        } else {
            Vec::new()
        };
        platform_with_externs("js-test", capabilities, vec![js_stdout_extern(name)])
    }

    fn platform_with_externs(
        name: &str,
        capabilities: impl IntoIterator<Item = Capability>,
        functions: Vec<ExternalFunction>,
    ) -> PlatformSpec {
        PlatformSpec {
            name: name.to_string(),
            provided_capabilities: capabilities.into_iter().collect(),
            externs: ExternalRegistry::from_functions(functions).unwrap(),
        }
    }

    fn js_stdout_extern(name: &str) -> ExternalFunction {
        platform_extern(
            name,
            ExternalBindings {
                js_symbol: Some("__emela_write_stdout_utf8".to_string()),
                ..ExternalBindings::default()
            },
        )
    }

    fn platform_extern(name: &str, bindings: ExternalBindings) -> ExternalFunction {
        ExternalFunction {
            path: vec!["platform".to_string(), "io".to_string()],
            name: name.to_string(),
            params: vec![Type::Prim(PrimType::String)],
            ret: Type::Apply {
                name: "Result".to_string(),
                args: vec![
                    Type::Prim(PrimType::Unit),
                    Type::Named("PlatformError".to_string()),
                ],
            },
            effectful: true,
            capabilities: vec![Capability::Stdout],
            bindings,
        }
    }

    fn builtin_descriptor(source: &str) -> BuiltinDescriptor {
        serde_json::from_str(source).unwrap()
    }

    #[derive(Debug, Deserialize)]
    struct BuiltinDescriptor {
        name: String,
        backend: String,
        abi_version: u32,
        builtin: Option<String>,
        target: Option<String>,
        runtime: Option<String>,
        capabilities: Vec<Capability>,
        externs: Vec<BuiltinExternal>,
    }

    #[derive(Debug, Deserialize)]
    struct BuiltinExternal {
        name: String,
    }
}
