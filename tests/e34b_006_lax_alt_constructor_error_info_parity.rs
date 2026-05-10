// Lax[T].errorInfo() must resolve cleanly on every Lax-shaped constructor,
// not only the bare `Lax(...)` factory. Two surfaces previously omitted the
// method on JS, so type-correct programs panicked at runtime:
//
//   1. `Bytes[<literal>]()` — frozen Lax pack created by `__taida_lax_from_bytes`.
//   2. JSON cast result — frozen Lax pack from `JSON_mold`.
//
// This fixture pins three-backend parity so future Lax-derived constructors
// stay aligned with the canonical accessor.

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
        "lax_alt_error_info_{}_{}_{}",
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

fn assert_three_backends_agree(results: &[(String, String); 3]) {
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    for (backend, out) in results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(out, &interp, "{} backend disagrees with interp", backend);
    }
}

#[test]
fn lax_error_info_on_bytes_constructor_is_empty_three_backends() {
    let dir = fixture_dir("bytes");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "raw <= Bytes[\"hello\"]()\ninfo <= raw.errorInfo()\nstdout(info.hasValue().toString())\n",
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
        "interp: errorInfo() on a Bytes-derived Lax must be empty"
    );
    assert_three_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lax_error_info_on_float_mold_failure_three_backends() {
    let dir = fixture_dir("float_failure");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "result <= Float[\"not_a_number\"]()\ninfo <= result.errorInfo()\nstdout(info.hasValue().toString())\n",
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
        "interp: errorInfo() on a failed Float mold Lax must be empty"
    );
    assert_three_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lax_error_info_on_list_get_out_of_bounds_three_backends() {
    let dir = fixture_dir("list_oob");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "items <= @[1, 2, 3]\nresult <= items.get(99)\ninfo <= result.errorInfo()\nstdout(info.hasValue().toString())\n",
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
        "interp: errorInfo() on a List.get out-of-bounds Lax must be empty"
    );
    assert_three_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}
