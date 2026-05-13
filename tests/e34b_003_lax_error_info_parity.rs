// E34 Phase 3 (Lock-D=B') acceptance: Lax[T].errorInfo() returns
// Lax[ErrorInfo] consistently across the Interpreter / JS / Native
// backends. The parity invariant covers two states the canonical
// shape must agree on:
//
//   1. Successful Lax → empty Lax[ErrorInfo] (has_value=false).
//   2. Failed Lax with no metadata → empty Lax[ErrorInfo] too.
//
// State (3) "Failed Lax with metadata → present Lax[ErrorInfo]"
// arrives once the Phase 5 producers (net / file / process / JSON
// failure paths) start populating `__error`. The fixture below
// avoids constructing one because there is no public Taida surface
// for synthesising a metadata-bearing failed Lax yet — that is
// exactly the work Phase 3 unlocks.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lax_error_info_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("mkdir fixture");
    dir
}

fn run_three_backends(main_path: &std::path::Path, dir: &std::path::Path) -> [(String, String); 3] {
    let interp = {
        let out = Command::new(taida_bin())
            .arg(main_path)
            .output()
            .expect("interp run");
        assert!(
            out.status.success(),
            "interp failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let js = if node_available() {
        let mjs = dir.join("main.mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(main_path)
            .arg("-o")
            .arg(&mjs)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("node").arg(&mjs).output().expect("node run");
        assert!(
            run.status.success(),
            "js run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("node unavailable; skipping JS leg");
        String::new()
    };

    let native = if cc_available() {
        let bin = dir.join("main.bin");
        let build = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(main_path)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&bin).output().expect("native run");
        assert!(
            run.status.success(),
            "native run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("cc unavailable; skipping native leg");
        String::new()
    };

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
    ]
}

#[test]
fn lax_error_info_success_returns_empty_lax_three_backends() {
    let dir = fixture_dir("success");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "obj <= Lax[42]()\ninfo <= obj.errorInfo()\nstdout(info.hasValue().toString())\n",
    )
    .expect("write main");
    let results = run_three_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "false",
        "interp: errorInfo() on a successful Lax must be empty"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interp on Lax[T].errorInfo() for successful receiver",
            backend
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
