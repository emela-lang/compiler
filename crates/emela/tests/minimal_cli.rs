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

enum Choice {
  Yes
  No
}

fn keep_enum(value: Choice) -> Choice {
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

#[test]
fn check_accepts_spec_0003_function_values() {
    let source = write_source(
        "main.emel",
        r#"
fn apply(x: Int, f: (Int) -> Int uses {}) -> Int uses {} {
  f(x)
}

fn add1(x: Int) -> Int uses {} {
  x + 1
}

fn makeAdder(n: Int) -> ((Int) -> Int uses {}) uses {} {
  fn (x: Int) -> Int uses {} {
    x + n
  }
}

fn main() -> Int uses {} {
  let inc = add1
  let add10 = makeAdder(10)
  apply(41, inc) + add10(5)
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
fn check_rejects_effectful_function_where_pure_is_expected() {
    let source = write_source(
        "main.emel",
        r#"
fn applyPure(x: Int, f: (Int) -> Int uses {}) -> Int uses {} {
  f(x)
}

fn readThenAdd(x: Int) -> Int uses { fs } {
  x + 1
}

fn main() -> Int uses {} {
  applyPure(1, readThenAdd)
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
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Type mismatch"));
}

#[test]
fn ir_emits_function_value_calls() {
    let source = write_source(
        "main.emel",
        r#"
fn add1(x: Int) -> Int {
  x + 1
}

fn main() -> Int {
  let inc = add1
  inc(41)
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
    assert!(stdout.contains("let inc = @add1"));
    assert!(stdout.contains("return call %inc(41)"));
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

#[test]
fn build_lowers_error_handling_to_js() {
    let source = write_source(
        "main.emel",
        r#"
enum E {
  A
  B
}

fn f() -> Int throws E uses {} {
  throw E.A
}

fn g() -> Int uses {} {
  try {
    f()
  } catch {
    e -> 7
  }
}

fn pick(o: Option<Int>) -> Int uses {} {
  match o {
    Some(v) -> v
    None -> 0
  }
}

fn main() -> Int uses {} {
  g() + pick(Some(3))
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
    let js = String::from_utf8_lossy(&output.stdout);
    assert!(js.contains("EmelaError"), "missing error runtime: {js}");
    assert!(js.contains("tag:"), "missing enum tag: {js}");
}

#[test]
fn check_rejects_non_exhaustive_match() {
    let source = write_source(
        "main.emel",
        r#"
enum C {
  A
  B
}

fn f(c: C) -> Int uses {} {
  match c {
    A -> 1
  }
}

fn main() -> Int uses {} {
  f(C.A)
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
        !output.status.success(),
        "a non-exhaustive match should be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("exhaustive") || stderr.contains("missing"),
        "{stderr}"
    );
}

#[test]
fn check_accepts_main_throws_never() {
    let source = write_source(
        "main.emel",
        "fn main() -> Int throws Never uses {} {\n  0\n}\n",
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
fn check_rejects_main_declaring_throws() {
    let source = write_source(
        "main.emel",
        "enum E {\n  A\n}\nfn main() -> Int throws E uses {} {\n  0\n}\n",
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
        !output.status.success(),
        "`main` declaring a non-Never throws should be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Never"), "{stderr}");
}

/// Two modules export `id`. Both imports are accepted (spec 0018 R4), and the
/// qualified forms `math.id` / `phys.id` resolve to distinct, mangled symbols so
/// the same bare name can coexist.
#[test]
fn ir_resolves_qualified_calls_and_disambiguates_collisions() {
    let dir = temp_dir();
    fs::write(
        dir.join("math.emel"),
        "module math\npub fn id(x: Int) -> Int {\n  x\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("phys.emel"),
        "module phys\npub fn id(x: Int) -> Int {\n  x + 100\n}\n",
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        r#"
import math.id
import phys.id

fn main() -> Int {
  math.id(1) + phys.id(2)
}
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("ir")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fn math__id(x) -> Int"), "{stdout}");
    assert!(stdout.contains("fn phys__id(x) -> Int"), "{stdout}");
    assert!(stdout.contains("call @math__id(1)"), "{stdout}");
    assert!(stdout.contains("call @phys__id(2)"), "{stdout}");
}

/// When two imports bind the same bare name, an unqualified call is ambiguous
/// and must be qualified (spec 0018 R5).
#[test]
fn check_rejects_ambiguous_bare_call() {
    let dir = temp_dir();
    fs::write(
        dir.join("math.emel"),
        "module math\npub fn id(x: Int) -> Int {\n  x\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("phys.emel"),
        "module phys\npub fn id(x: Int) -> Int {\n  x + 100\n}\n",
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        "import math.id\nimport phys.id\nfn main() -> Int {\n  id(1)\n}\n",
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Ambiguous reference"), "{stderr}");
    assert!(stderr.contains("math.id"), "{stderr}");
    assert!(stderr.contains("phys.id"), "{stderr}");
}

/// A package import is callable bare, by its leaf module, and by its full path
/// (spec 0018 R2): `add_one`, `ops.add_one`, `math.ops.add_one`.
#[test]
fn check_resolves_qualified_package_import() {
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
        "module ops\npub fn add_one(x: Int) -> Int {\n  x + 1\n}\n",
    )
    .unwrap();
    let source = dir.join("main.emel");
    fs::write(
        &source,
        r#"
import math.ops.add_one

fn main() -> Int {
  add_one(0) + ops.add_one(1) + math.ops.add_one(2)
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
