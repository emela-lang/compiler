mod ast;
mod codegen;
mod driver;
mod error;
mod lexer;
mod parser;
mod platform;
mod typecheck;

fn main() {
    if let Err(error) = driver::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::PrimType;
    use crate::codegen::emit_assembly;
    use crate::driver::{compile_source, compile_source_for_target};
    use crate::platform::Target;

    #[test]
    fn accepts_empty_main() {
        let (_, typed) = compile_source("fn main() {\n}\n").unwrap();
        assert_eq!(typed.functions[0].name, "main");
        assert_eq!(typed.functions[0].ret, PrimType::Unit);
    }

    #[test]
    fn infers_i32_function() {
        let (_, typed) = compile_source(
            r#"
fn add(x, y) {
  x + y
}

fn main() {
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
        assert_eq!(add.params, vec![PrimType::I32, PrimType::I32]);
        assert_eq!(add.ret, PrimType::I32);
    }

    #[test]
    fn accepts_return_annotation_and_exits_with_main_i32() {
        let source = r#"
fn add(x, y) -> i32 {
  x + y
}

fn main() {
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
        assert_eq!(main.ret, PrimType::I32);

        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains(".globl _main"));
    }

    #[test]
    fn emits_assembly_match_expression() {
        let (program, typed) = compile_source_for_target(
            "fn main() -> I32 { match true { true -> 1 false -> 0 } }",
            Target::Aarch64AppleDarwin,
        )
        .unwrap();
        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("mov w10, #1"));
        assert!(assembly.contains("mov w10, #0"));
        assert!(assembly.contains("cmp w9, w10"));
    }

    #[test]
    fn emits_x86_64_linux_assembly() {
        let (program, typed) = compile_source_for_target(
            r#"
fn add(x, y) -> I32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
"#,
            Target::X86_64UnknownLinuxGnu,
        )
        .unwrap();
        let assembly = emit_assembly(Target::X86_64UnknownLinuxGnu, &program, &typed).unwrap();
        assert!(assembly.contains(".globl main"));
        assert!(assembly.contains("add:"));
        assert!(assembly.contains("movl %edi, -4(%rbp)"));
        assert!(assembly.contains("call add"));
        assert!(assembly.contains("addl %r9d, %eax"));
    }

    #[test]
    fn rejects_match_pattern_type_mismatch() {
        let error = compile_source("fn main() { match 1 { true -> 2 false -> 3 } }").unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_effectful_call_from_pure_function() {
        let error = compile_source(
            r#"
fn tick!() {
  ()
}

fn main() {
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
fn print_i32!(value) -> Unit {
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

        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains(".globl _main"));
        assert!(assembly.contains("requires Stdout"));
    }

    #[test]
    fn rejects_requires_outside_declared_capabilities() {
        let error = compile_source(
            r#"
#[requires(Stdout)]
fn print_i32!(value) -> Unit {
  ()
}

#[requires()]
fn main!() {
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
        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("add w0, w9, w0"));
    }

    #[test]
    fn rejects_capability_missing_from_native_target() {
        let error = compile_source(
            r#"
#[requires(HostImport)]
fn host_call!() -> Unit {
  ()
}

fn main!() {
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
fn print_i32!(value) -> Unit {
  ()
}

fn main!() {
  print_i32!(42)
}
"#;
        compile_source_for_target(source, Target::Wasm32Wasi).unwrap();
        let error = compile_source_for_target(source, Target::Wasm32UnknownUnknown).unwrap_err();
        assert!(error
            .to_string()
            .contains("target `wasm32-unknown-unknown` does not provide"));
    }

    #[test]
    fn non_native_target_does_not_emit_assembly() {
        let (program, typed) =
            compile_source_for_target("fn main() {}", Target::Wasm32UnknownUnknown).unwrap();
        let error = emit_assembly(Target::Wasm32UnknownUnknown, &program, &typed).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not have a native assembly backend"));
    }
}
