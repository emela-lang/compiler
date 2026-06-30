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
