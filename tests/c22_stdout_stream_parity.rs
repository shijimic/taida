//! C22-2 / C22B-002: 3-backend parity harness for `examples/quality/c22_stdout_stream/`.
//!
//! Unlike `tests/c22_stdout_stream.rs` (which pins the Rust API contract
//! between `Interpreter::new()` and `Interpreter::new_streaming()`), this
//! file asserts that the Interpreter, JS, and Native backends produce
//! **byte-identical** stdout for the stream-mode fixtures. The parity
//! invariant is the design rationale behind the Phase 1 decision to
//! route `debug` to stdout instead of stderr: we refused to let the
//! interpreter diverge from JS / Native in the name of POSIX symmetry.
//!
//! Two fixtures:
//!
//! * `progress_loop.td` — four `stdout("step=N")` calls driven by
//!   `Map[..., emit]()` plus a final `stdout("done")`. Verifies that
//!   stream-mode line-by-line flush produces the same observable order
//!   on every backend, matching JS (`console.log`) and Native
//!   (`printf` via `taida_stdout_*`).
//! * `debug_stream.td` — single-argument `debug("...")` interleaved
//!   with `stdout(...)`. Pins the design decision that `debug` output
//!   goes to stdout on all three backends; a stderr routing on the
//!   interpreter side would break this harness.
//!
//! Red test ゼロ — any divergence is a C22 regression.

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    manifest_dir().join("examples/quality/c22_stdout_stream")
}

fn td_path(stem: &str) -> PathBuf {
    fixture_dir().join(format!("{}.td", stem))
}

fn expected_path(stem: &str) -> PathBuf {
    fixture_dir().join(format!("{}.expected", stem))
}

fn read_expected(stem: &str) -> String {
    fs::read_to_string(expected_path(stem))
        .unwrap_or_else(|_| panic!("expected file for '{}' must exist", stem))
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

// ---- backend drivers ---------------------------------------------

fn assert_interpreter(stem: &str) {
    let out = Command::new(taida_bin())
        .arg(td_path(stem))
        .output()
        .expect("failed to spawn interpreter");
    assert!(
        out.status.success(),
        "interpreter ({}) exited non-zero: stderr={}",
        stem,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let expected = read_expected(stem);
    assert!(
        outputs_equal(&stdout, &expected),
        "C22 interpreter stream-mode output mismatch for '{}'.\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

fn assert_js_matches(stem: &str) {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let mjs_path = unique_temp(&format!("c22_{}", stem), "mjs");
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(td_path(stem))
        .arg("-o")
        .arg(&mjs_path)
        .output()
        .expect("failed to invoke js build");
    assert!(
        build_out.status.success(),
        "js build ({}) failed: {}",
        stem,
        String::from_utf8_lossy(&build_out.stderr)
    );

    let node_out = Command::new("node")
        .arg(&mjs_path)
        .output()
        .expect("failed to spawn node");
    let _ = fs::remove_file(&mjs_path);
    assert!(
        node_out.status.success(),
        "node exit failed ({}): {}",
        stem,
        String::from_utf8_lossy(&node_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&node_out.stdout).to_string();
    let expected = read_expected(stem);
    assert!(
        outputs_equal(&stdout, &expected),
        "C22 JS output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

fn assert_native_matches(stem: &str) {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let bin_path = unique_temp(&format!("c22_{}", stem), "bin");
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(td_path(stem))
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("failed to invoke native build");
    assert!(
        build_out.status.success(),
        "native build ({}) failed: {}",
        stem,
        String::from_utf8_lossy(&build_out.stderr)
    );
    let run_out = Command::new(&bin_path)
        .output()
        .expect("failed to spawn native binary");
    let _ = fs::remove_file(&bin_path);
    assert!(
        run_out.status.success(),
        "native binary ({}) exit failed: status={:?}, stderr={}",
        stem,
        run_out.status.code(),
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&run_out.stdout).to_string();
    let expected = read_expected(stem);
    assert!(
        outputs_equal(&stdout, &expected),
        "C22 native output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

// ── progress_loop.td: streaming stdout across 3 backends ──────────

#[test]
fn c22_progress_loop_interpreter_streams_each_line() {
    assert_interpreter("progress_loop");
}

#[test]
fn c22_progress_loop_js_matches_interpreter() {
    assert_js_matches("progress_loop");
}

#[test]
fn c22_progress_loop_native_matches_interpreter() {
    assert_native_matches("progress_loop");
}

// ── debug_stream.td: `debug` routes to stdout, not stderr ─────────

#[test]
fn c22_debug_stream_interpreter_writes_to_stdout() {
    assert_interpreter("debug_stream");
}

#[test]
fn c22_debug_stream_js_matches_interpreter() {
    assert_js_matches("debug_stream");
}

#[test]
fn c22_debug_stream_native_matches_interpreter() {
    assert_native_matches("debug_stream");
}
