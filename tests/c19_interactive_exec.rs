//! C19: 3-backend parity harness for `runInteractive` / `execShellInteractive`.
//!
//! Runs the two `examples/quality/c19_interactive_exec/*.td` scripts
//! through each backend (Interpreter, JS, Native) and asserts byte-
//! identical stdout against the canonical `.expected` files.
//!
//! Red test ゼロ容認 — any backend divergence is a C19 regression. The
//! unit tests in `src/interpreter/os_eval.rs` already pin the interpreter
//! contract (exit codes + Gorillax inner shape + IoError kind); this
//! harness pins the runtime parity across the three backends.
//!
//! NOTE: CI does not provide a real TTY, so these tests only exercise
//! the exit-code contract. The true passthrough behaviour is validated
//! manually via the Hachikuma B-006 smoke (`runInteractive("nvim", ...)`).

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn test_dir() -> PathBuf {
    manifest_dir().join("examples/quality/c19_interactive_exec")
}

fn td_path(stem: &str) -> PathBuf {
    test_dir().join(format!("{}.td", stem))
}

fn expected_path(stem: &str) -> PathBuf {
    test_dir().join(format!("{}.expected", stem))
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

/// Run a single parity stem on the interpreter and assert vs .expected.
fn assert_interpreter(stem: &str) {
    let out = Command::new(taida_bin())
        .arg(td_path(stem))
        .output()
        .expect("failed to invoke interpreter");
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
        "C19 interpreter output mismatch for '{}'.\n--- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

/// Run a single parity stem through the JS backend and assert vs interpreter reference.
fn assert_js_matches(stem: &str) {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let mjs_path = unique_temp(&format!("c19_{}", stem), "mjs");
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
        .expect("failed to invoke node");
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
        "C19 JS output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

/// Run a single parity stem through the Native backend and assert vs interpreter reference.
fn assert_native_matches(stem: &str) {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let bin_path = unique_temp(&format!("c19_{}", stem), "bin");
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
        .expect("failed to execute native binary");
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
        "C19 native output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

// ── runInteractive parity ──

#[test]
fn c19_run_interactive_interpreter_matches_expected() {
    assert_interpreter("os_interactive_run");
}

#[test]
fn c19_run_interactive_js_matches_interpreter() {
    assert_js_matches("os_interactive_run");
}

#[test]
fn c19_run_interactive_native_matches_interpreter() {
    assert_native_matches("os_interactive_run");
}

// ── execShellInteractive parity ──

#[test]
fn c19_exec_shell_interactive_interpreter_matches_expected() {
    assert_interpreter("os_interactive_exec_shell");
}

#[test]
fn c19_exec_shell_interactive_js_matches_interpreter() {
    assert_js_matches("os_interactive_exec_shell");
}

#[test]
fn c19_exec_shell_interactive_native_matches_interpreter() {
    assert_native_matches("os_interactive_exec_shell");
}

// ── runInteractive ENOENT (IoError contract) parity ──
//
// These three tests pin the failure-path parity uncovered by the
// code-review HOLD:
// - Native used to surface `execvp` failure as ProcessError(127)
//   instead of a proper IoError (fixed via CLOEXEC errno pipe in
//   `taida_os_run_interactive`).
// - Native Gorillax stored `__error` under the wrong field hash
//   (HASH___DEFAULT), making `.__error.<field>` unreachable
//   (fixed by introducing HASH___ERROR and threading it through
//   `taida_gorillax_{new,err,relax}`).
// - JS normalized Node's signed `err.errno` so `.__error.code`
//   agrees with the interpreter / native positive-errno contract.
//
// If any of these regress, this test will turn red.

#[test]
fn c19_run_interactive_enoent_interpreter_matches_expected() {
    assert_interpreter("os_interactive_enoent");
}

#[test]
fn c19_run_interactive_enoent_js_matches_interpreter() {
    assert_js_matches("os_interactive_enoent");
}

#[test]
fn c19_run_interactive_enoent_native_matches_interpreter() {
    assert_native_matches("os_interactive_enoent");
}
