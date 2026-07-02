#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-plugin-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn external_backend_round_trip() {
    let dir = temp_dir();

    // A dummy plugin: consume the IR request, emit a fixed artifact ("PLUG").
    let script = dir.join("plugin.sh");
    fs::write(
        &script,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s' '{\"status\":\"ok\",\"kind\":\"JsSource\",\"bytes\":[80,76,85,71]}'\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let descriptor = dir.join("backend.json");
    fs::write(
        &descriptor,
        format!(
            "{{\"name\":\"dummy\",\"backend\":\"custom\",\"abi_version\":1,\"command\":[\"sh\",\"{}\"]}}",
            script.display()
        ),
    )
    .unwrap();

    let source = dir.join("main.emel");
    fs::write(&source, "fn main() -> Int { 1 }\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg(&descriptor)
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "PLUG");
}
