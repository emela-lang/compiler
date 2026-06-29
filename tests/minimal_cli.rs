use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn write_source(name: &str, source: &str) -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-minimal-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, source).unwrap();
    path
}

#[test]
fn check_accepts_minimal_program() {
    let source = write_source(
        "main.emel",
        r#"
fn add(x: Int, y: Int) -> Int uses {} {
  let n = x + y
  n
}

fn main() -> Int uses {} {
  add(40, 2)
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(source.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn build_emits_javascript() {
    let source = write_source(
        "main.emel",
        r#"
fn main() -> Int {
  42
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(source.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("function main()"));
    assert!(stdout.contains("console.log(__emela_result)"));
}

#[test]
fn ir_emits_lowered_program() {
    let source = write_source(
        "main.emel",
        r#"
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn main() -> Int {
  let value = add(40, 2)
  value
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("ir")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(source.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fn add(x, y) -> Int uses {}"));
    assert!(stdout.contains("return add.i32 %x, %y"));
    assert!(stdout.contains("let value = call @add(40, 2)"));
}

#[test]
fn check_accepts_spec_0001_types() {
    let source = write_source(
        "main.emel",
        r#"
fn keep_float(x: Float) -> Float {
  x + 0.5
}

fn keep_array(xs: Array<Int>) -> Array<Int> {
  xs
}

fn keep_record(value: Record) -> Record {
  value
}

fn keep_enum(value: Enum) -> Enum {
  value
}

fn keep_function(value: Function) -> Function {
  value
}

fn main() -> Unit {
  let n: Float = keep_float(1.5)
  let xs: Array<Int> = [1, 2, 3]
  let empty: Array<Int> = []
  ()
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(source.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ir_emits_float_and_array_values() {
    let source = write_source(
        "main.emel",
        r#"
fn main() -> Array<Float> {
  let first = 1.5 + 2.25
  [first, 4.0]
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("ir")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(source.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fn main() -> Array<Float> uses {}"));
    assert!(stdout.contains("let first = add.f64 1.5, 2.25"));
    assert!(stdout.contains("return [%first, 4]"));
}
