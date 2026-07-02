use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Lays out a `std` source package wrapping `io.write_stdout`, plus an app that
/// imports `std.io.print`. Returns (package dir, app file).
fn hello_project() -> (std::path::PathBuf, std::path::PathBuf) {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-platform-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("io.emel"),
        "module io\nextern fn write_stdout(s: String) -> Unit uses { io }\npub fn print(s: String) -> Unit uses { io } { write_stdout(s) }\n",
    )
    .unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "import std.io.print\nfn main() -> Unit uses { io } { print(\"Hello, Emela!\\n\") }\n",
    )
    .unwrap();
    (package, app)
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

#[test]
fn js_backend_resolves_platform_via_runtime() {
    let (package, app) = hello_project();
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    // The runtime bundles only the used platform function.
    assert!(
        js.contains("\"io.write_stdout\": (s) => process.stdout.write(s)"),
        "{js}"
    );
    assert!(!js.contains("write_stderr"), "{js}");
    // The wrapper calls the runtime; `main` passes the message to the wrapper.
    assert!(js.contains("__rt[\"io.write_stdout\"](s)"), "{js}");
    assert!(js.contains("print(\"Hello, Emela!\\n\")"), "{js}");
}

#[test]
fn wasm_backend_builds_a_valid_module() {
    let (package, app) = hello_project();
    let out = app.parent().unwrap().join("hello.wasm");
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--package")
        .arg(&package)
        .arg("-o")
        .arg(&out)
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&out).unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    // A WASI module with the wasm magic; instantiation needs an `io`-capable
    // runtime (WAMR `iwasm`, `wasmtime`, ...).
    assert_eq!(&bytes[0..4], b"\0asm");
}

/// Builds a program that uses the pure `std.int.to_string` (if + `/`/`%` +
/// `Char`/`++`) together with `std.io.print`, end to end to a wasm module.
#[test]
fn pure_to_string_builds_on_wasm() {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-tostring-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("io.emel"),
        "module io\nextern fn write_stdout(s: String) -> Unit uses { io }\npub fn print(s: String) -> Unit uses { io } { write_stdout(s) }\n",
    )
    .unwrap();
    fs::write(
        package.join("src").join("int.emel"),
        "module int\nfn digits(n: Int) -> String { if n == 0 { \"\" } else { digits(n / 10) ++ String.from_char(Char.from_code(48 + n % 10)) } }\npub fn to_string(n: Int) -> String { if n == 0 { \"0\" } else { if n < 0 { \"-\" ++ digits(0 - n) } else { digits(n) } } }\n",
    )
    .unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "import std.io.print\nimport std.int.to_string\nfn main() -> Unit uses { io } { print(to_string(42) ++ \"\\n\") }\n",
    )
    .unwrap();

    let out = dir.join("out.wasm");
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--package")
        .arg(&package)
        .arg("-o")
        .arg(&out)
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&out).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(&bytes[0..4], b"\0asm");
}
