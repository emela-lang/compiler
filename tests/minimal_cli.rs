use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-minimal-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_source(name: &str, source: &str) -> std::path::PathBuf {
    let dir = temp_dir();
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
fn check_resolves_local_module_import() {
    let dir = temp_dir();
    fs::write(
        dir.join("math.emel"),
        r#"
module math

pub fn add_one(x: Int) -> Int {
  x + 1
}
"#,
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        r#"
import math.add_one

fn main() -> Int {
  add_one(41)
}
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_rejects_private_import() {
    let dir = temp_dir();
    fs::write(
        dir.join("math.emel"),
        r#"
module math

fn hidden(x: Int) -> Int {
  x + 1
}
"#,
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        r#"
import math.hidden

fn main() -> Int {
  hidden(41)
}
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Private import"));
}

#[test]
fn check_resolves_package_import() {
    let dir = temp_dir();
    let package = dir.join("math-pkg");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"math","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("ops.emel"),
        r#"
module ops

pub fn add_one(x: Int) -> Int {
  x + 1
}
"#,
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        r#"
import math.ops.add_one

fn main() -> Int {
  add_one(41)
}
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_multiple_imports_from_same_module_once() {
    let dir = temp_dir();
    fs::write(
        dir.join("math.emel"),
        r#"
module math

pub fn add_one(x: Int) -> Int {
  x + 1
}

pub fn add_two(x: Int) -> Int {
  x + 2
}
"#,
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        r#"
import math.add_one
import math.add_two

fn main() -> Int {
  add_two(add_one(39))
}
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
