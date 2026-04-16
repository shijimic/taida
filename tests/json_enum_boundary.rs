//! C16 post-review regression: JSON Enum boundary + Lax[Enum] parity.
//!
//! Pins the contract surfaced by code-reviewer blockers:
//!   - C16B-001: Native `json_default_value_for_desc('T')` must produce pure
//!     defaults — TypeDef fields whose type is Enum go to `Int(0)`, never
//!     `Lax[Enum]`. Direct `r.__value.status.toString()` / `r.__default.status
//!     .toString()` access verifies this on the parse-error AND success paths.
//!   - C16B-003: Extended coverage for List[Enum], List[TypeDef{Enum}],
//!     multiple Enum fields, variant-name edges, mismatch edges, and JSON
//!     input-type variations.
//!   - C16B-004: `Lax[Enum].toString()` format must be byte-identical across
//!     Interpreter / JS / Native (pinned as `Lax(default: 0)` via the
//!     comprehensive fixture).
//!
//! Red test ゼロ容認 — if any backend diverges, the non-reference backend is
//! wrong (Interpreter is the reference).

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn td_path() -> PathBuf {
    manifest_dir().join("examples/quality/json_enum_boundary.td")
}

fn expected_path() -> PathBuf {
    manifest_dir().join("examples/quality/json_enum_boundary.expected")
}

fn read_expected() -> String {
    fs::read_to_string(expected_path())
        .expect("examples/quality/json_enum_boundary.expected must exist")
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

fn outputs_equal(a: &str, b: &str) -> bool {
    a.trim_end_matches('\n') == b.trim_end_matches('\n')
}

#[test]
fn c16_enum_boundary_interpreter_matches_expected() {
    let out = Command::new(taida_bin())
        .arg(td_path())
        .output()
        .expect("failed to invoke interpreter");
    assert!(
        out.status.success(),
        "interpreter exited with non-zero status: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C16 boundary interpreter mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

#[test]
fn c16_enum_boundary_js_matches_interpreter() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let js_out_path = unique_temp("c16_json_enum_boundary", "js");
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(td_path())
        .arg("-o")
        .arg(&js_out_path)
        .output()
        .expect("failed to invoke js build");
    assert!(
        build_out.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    let node_out = Command::new("node")
        .arg(&js_out_path)
        .output()
        .expect("failed to invoke node");
    let _ = fs::remove_file(&js_out_path);
    assert!(
        node_out.status.success(),
        "node exit failed: {}",
        String::from_utf8_lossy(&node_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&node_out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C16 boundary JS mismatch (interpreter is reference).\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

#[test]
fn c16_enum_boundary_native_matches_interpreter() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let bin_path = unique_temp("c16_json_enum_boundary", "bin");
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(td_path())
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("failed to invoke native build");
    assert!(
        build_out.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    let run_out = Command::new(&bin_path)
        .output()
        .expect("failed to execute native binary");
    let _ = fs::remove_file(&bin_path);
    assert!(
        run_out.status.success(),
        "native binary exit failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&run_out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C16 boundary native mismatch (interpreter is reference).\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}
