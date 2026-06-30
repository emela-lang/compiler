use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-wasm-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds `source` to a wasm binary and asserts it is a well-formed module.
fn build_wasm(source: &str) {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    let output = dir.join("out.wasm");
    fs::write(&input, source).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output)
        .arg(&input)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output).unwrap();
    let _ = fs::remove_dir_all(&dir);
    // The wasm magic number and version 1.
    assert_eq!(&bytes[0..4], b"\0asm");
    assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
}

#[test]
fn builds_integer_functions() {
    build_wasm("fn add(x: Int, y: Int) -> Int { x + y }\nfn main() -> Int { add(20, 22) }\n");
}

#[test]
fn builds_floats_and_arrays() {
    build_wasm("fn main() -> Array<Float> {\n  let xs: Array<Float> = [1.5, 2.5]\n  xs\n}\n");
}

#[test]
fn builds_strings() {
    build_wasm("fn main() -> String {\n  let s: String = \"hi\"\n  s\n}\n");
}

#[test]
fn builds_closures_and_indirect_calls() {
    build_wasm(
        "fn make_adder(n: Int) -> (Int) -> Int {\n  fn (x: Int) -> Int { x + n }\n}\nfn main() -> Int {\n  let add10 = make_adder(10)\n  add10(32)\n}\n",
    );
}

#[test]
fn builds_higher_order_functions() {
    build_wasm(
        "fn apply(f: (Int) -> Int, x: Int) -> Int { f(x) }\nfn inc(x: Int) -> Int { x + 1 }\nfn main() -> Int { apply(inc, 41) }\n",
    );
}

#[test]
fn builds_enums_and_match() {
    build_wasm(
        "enum Color {\n  Red\n  Green\n  Blue\n}\nfn code(c: Color) -> Int {\n  match c {\n    Red -> 1\n    Green -> 2\n    Blue -> 3\n  }\n}\nfn main() -> Int { code(Color.Blue) }\n",
    );
}

#[test]
fn builds_option_match() {
    build_wasm(
        "fn unwrap(o: Option<Int>, fb: Int) -> Int {\n  match o {\n    Some(v) -> v\n    None -> fb\n  }\n}\nfn main() -> Int { unwrap(Some(7), 0) }\n",
    );
}

#[test]
fn builds_panic() {
    build_wasm("fn boom() -> Int { panic(\"x\") }\nfn main() -> Int { boom() }\n");
}

#[test]
fn builds_throws_and_try_catch() {
    build_wasm(
        "enum E {\n  Bad\n}\nfn risky() -> Int throws E { throw E.Bad }\nfn safe() -> Int {\n  try {\n    risky()\n  } catch {\n    Bad -> 9\n  }\n}\nfn main() -> Int { safe() }\n",
    );
}

#[test]
fn builds_question_propagation() {
    build_wasm(
        "enum E {\n  Bad\n}\nfn risky() -> Int throws E { throw E.Bad }\nfn chain() -> Int throws E {\n  let x = risky()?\n  x\n}\nfn run() -> Int {\n  try {\n    chain()\n  } catch {\n    e -> 1\n  }\n}\nfn main() -> Int { run() }\n",
    );
}

#[test]
fn builds_option_question() {
    build_wasm(
        "fn first() -> Option<Int> { Some(5) }\nfn chain() -> Option<Int> {\n  let x = first()?\n  Some(x)\n}\nfn main() -> Int {\n  match chain() {\n    Some(v) -> v\n    None -> 0\n  }\n}\n",
    );
}

#[test]
fn builds_if_expression() {
    build_wasm(
        "fn pick(n: Int) -> Int { if n < 0 { 0 - n } else { n } }\nfn main() -> Int { if pick(0 - 5) == 5 { 42 } else { 7 } }\n",
    );
}

#[test]
fn builds_integer_division_and_remainder() {
    build_wasm("fn main() -> Int { (7 / 2) + (7 % 2) }\n");
}

#[test]
fn builds_char_and_concat() {
    build_wasm(
        "fn digit(d: Int) -> String { String.from_char(Char.from_code(48 + d)) }\nfn main() -> String { \"x\" ++ digit(7) ++ String.from_char('Z') }\n",
    );
}
