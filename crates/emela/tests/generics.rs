//! End-to-end tests for generic functions (spec 0014): type-parameter
//! inference, the opaqueness rules, and monomorphization through to the wasm
//! and JavaScript backends.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-generics-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `emela <args> <source-file>` and returns the process output.
fn run(args: &[&str], source: &str) -> std::process::Output {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
    for arg in args {
        command.arg(arg);
    }
    let output = command.arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn check_ok(source: &str) {
    let output = run(&["check"], source);
    assert!(
        output.status.success(),
        "expected check to pass, but it failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Asserts `check` rejects the program, returning the diagnostics for matching.
fn check_err(source: &str) -> String {
    let output = run(&["check"], source);
    assert!(
        !output.status.success(),
        "expected check to fail, but it passed"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn ir(source: &str) -> String {
    let output = run(&["ir"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn js(source: &str) -> String {
    let output = run(&["build", "--backend", "js-node"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Builds to wasm and asserts a well-formed module (the compiler validates the
/// bytes before writing them, so a successful build is a valid module).
fn build_wasm_ok(source: &str) {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    let output_path = dir.join("out.wasm");
    fs::write(&input, source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output_path)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output_path).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(&bytes[0..4], b"\0asm");
    assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
}

const IDENTITY: &str = "fn identity<T>(x: T) -> T { x }\n";

#[test]
fn checks_simple_generic() {
    check_ok(&format!("{IDENTITY}fn main() -> Int {{ identity(42) }}\n"));
}

#[test]
fn checks_higher_order_generic() {
    check_ok(
        "fn apply<A, B>(f: (A) -> B, x: A) -> B { f(x) }\n\
         fn main() -> Int { apply(fn (n: Int) -> Int { n + 1 }, 41) }\n",
    );
}

#[test]
fn infers_nested_type_argument() {
    // `T` is inferred from inside `Array<T>`.
    check_ok(
        "fn count<T>(xs: Array<T>) -> Int { 0 }\n\
         fn main() -> Int { count([1, 2, 3]) }\n",
    );
}

#[test]
fn monomorphizes_each_type_argument() {
    // `identity` used at Int and String produces two specializations; the
    // generic template itself is never emitted, and no type variable leaks.
    let dump = ir(&format!(
        "{IDENTITY}fn main() -> Int {{\n  let s: String = identity(\"hi\")\n  identity(7)\n}}\n"
    ));
    assert!(
        dump.contains("fn identity__Int("),
        "missing Int specialization:\n{dump}"
    );
    assert!(
        dump.contains("fn identity__String("),
        "missing String specialization:\n{dump}"
    );
    assert!(
        !dump.contains("fn identity("),
        "generic template should not be emitted:\n{dump}"
    );
    assert!(
        !dump.contains("Var("),
        "type variable leaked into IR:\n{dump}"
    );
}

#[test]
fn specializes_transitively() {
    // `wrap` calls `identity`; specializing `wrap__Int` must pull in
    // `identity__Int` as well.
    let dump = ir(&format!(
        "{IDENTITY}fn wrap<T>(x: T) -> T {{ identity(x) }}\n\
         fn main() -> Int {{ wrap(1) }}\n"
    ));
    assert!(
        dump.contains("fn wrap__Int("),
        "missing wrap specialization:\n{dump}"
    );
    assert!(
        dump.contains("fn identity__Int("),
        "missing transitive specialization:\n{dump}"
    );
}

#[test]
fn builds_generic_to_wasm() {
    build_wasm_ok(&format!("{IDENTITY}fn main() -> Int {{ identity(42) }}\n"));
}

#[test]
fn emits_specialized_javascript() {
    let source = format!("{IDENTITY}fn main() -> Int {{ identity(42) }}\n");
    let code = js(&source);
    assert!(
        code.contains("identity__Int"),
        "expected the mangled call:\n{code}"
    );
}

#[test]
fn rejects_operation_on_opaque_parameter() {
    let diagnostics = check_err("fn bad<T>(x: T) -> T { x + x }\nfn main() -> Int { 0 }\n");
    assert!(diagnostics.contains("Type mismatch"), "{diagnostics}");
}

#[test]
fn rejects_uninferable_type_parameter() {
    let diagnostics = check_err("fn pick<T>() -> T { panic(\"x\") }\nfn main() -> Int { 0 }\n");
    assert!(
        diagnostics.contains("Uninferable type parameter"),
        "{diagnostics}"
    );
}

#[test]
fn rejects_generic_used_as_value() {
    let diagnostics = check_err(&format!(
        "{IDENTITY}fn main() -> Int {{\n  let f = identity\n  0\n}}\n"
    ));
    assert!(
        diagnostics.contains("Generic function used as a value"),
        "{diagnostics}"
    );
}

#[test]
fn rejects_conflicting_type_arguments() {
    // `T` cannot be both Int and Float.
    let diagnostics = check_err(
        "fn same<T>(a: T, b: T) -> T { a }\n\
         fn main() -> Int { same(1, 2.0) }\n",
    );
    assert!(
        diagnostics.contains("Conflicting type argument") || diagnostics.contains("Type mismatch"),
        "{diagnostics}"
    );
}
