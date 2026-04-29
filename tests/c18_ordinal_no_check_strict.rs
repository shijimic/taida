//! C18B-005 regression: `Ordinal[]` must reject non-Enum arguments
//! under `--no-check` on every backend.
//!
//! Pre-fix JS / Native silently returned the input (identity) under
//! `--no-check` while the interpreter raised a `RuntimeError`. The fix
//! introduces `__taida_enumOrdinalStrict` (JS) and a compile-time
//! emitter for `taida_runtime_panic` (Native) so all three backends
//! now agree:
//!   - stderr contains the canonical RuntimeError message, AND
//!   - the process exits with a non-zero status.

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

fn unique_temp(prefix: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        ext
    ))
}

fn write_source(src: &str) -> PathBuf {
    let p = unique_temp("c18b_005_source", "td");
    fs::write(&p, src).expect("write source");
    p
}

const NON_ENUM_SOURCE: &str = "stdout(Ordinal[1]().toString())\n";

const EXPECTED_ERROR_SUBSTRING: &str = "Ordinal: argument must be an Enum value";

#[test]
fn c18b_005_interpreter_rejects_non_enum_under_no_check() {
    let src = write_source(NON_ENUM_SOURCE);
    let out = Command::new(taida_bin())
        .arg("--no-check")
        .arg(&src)
        .output()
        .expect("failed to invoke interpreter");
    let _ = fs::remove_file(&src);
    assert!(
        !out.status.success(),
        "interpreter should reject non-Enum Ordinal argument with non-zero status"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains(EXPECTED_ERROR_SUBSTRING),
        "interpreter stderr missing expected message.\n--- stderr ---\n{}\n",
        stderr
    );
}

#[test]
fn c18b_005_js_rejects_non_enum_under_no_check() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let src = write_source(NON_ENUM_SOURCE);
    let out_path = unique_temp("c18b_005_js", "mjs");
    let build = Command::new(taida_bin())
        .arg("--no-check")
        .arg("build")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("failed to invoke js build");
    let _ = fs::remove_file(&src);
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&out_path)
        .output()
        .expect("failed to invoke node");
    let _ = fs::remove_file(&out_path);
    assert!(
        !run.status.success(),
        "node should exit non-zero for non-Enum Ordinal argument"
    );
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    assert!(
        stderr.contains(EXPECTED_ERROR_SUBSTRING),
        "js stderr missing expected message.\n--- stderr ---\n{}\n",
        stderr
    );
}

#[test]
fn c18b_005_native_rejects_non_enum_under_no_check() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let src = write_source(NON_ENUM_SOURCE);
    let out_path = unique_temp("c18b_005_native", "bin");
    let build = Command::new(taida_bin())
        .arg("--no-check")
        .arg("build")
        .arg("native")
        .arg(&src)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("failed to invoke native build");
    let _ = fs::remove_file(&src);
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&out_path)
        .output()
        .expect("failed to execute native binary");
    let _ = fs::remove_file(&out_path);
    assert!(
        !run.status.success(),
        "native binary should exit non-zero for non-Enum Ordinal argument"
    );
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    assert!(
        stderr.contains(EXPECTED_ERROR_SUBSTRING),
        "native stderr missing expected message.\n--- stderr ---\n{}\n",
        stderr
    );
}
