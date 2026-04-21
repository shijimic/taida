//! C22-2 / C22B-002: stream vs buffered mode stdout parity.
//!
//! Drives the `Interpreter::new()` (buffered mode, default — used by REPL,
//! in-process tests, JS codegen embedding) and `Interpreter::new_streaming()`
//! (stream mode — used by `taida run <file>` CLI) against the same Taida
//! source and asserts that, once you account for where the bytes land, the
//! two modes produce the same observable output.
//!
//! Also asserts the Taida-surface contract that neither `stdout` nor `debug`
//! dropped their byte-count return or their implicit trailing newline.

use std::io::Write;
use std::process::{Command, Stdio};

mod common;
use common::taida_bin;

fn run_streaming(source: &str) -> Vec<String> {
    // stream mode == CLI execution path
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "c22_stream_{}_{}.td",
        std::process::id(),
        fastrand_like_suffix()
    ));
    std::fs::write(&path, source).expect("write tmp");
    let out = Command::new(taida_bin())
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run taida");
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "taida should exit 0, stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn run_buffered(source: &str) -> Vec<String> {
    // buffered mode == `Interpreter::new()` in-process, exactly as the
    // `eval_with_output` test helper does.
    let (program, errors) = taida::parser::parse(source);
    assert!(errors.is_empty(), "parse errors: {:?}", errors);
    let mut interp = taida::interpreter::Interpreter::new();
    interp.eval_program(&program).expect("eval ok");
    interp.output.clone()
}

fn fastrand_like_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Fundamental contract: both modes must observe `stdout("a"); stdout("b")` as
/// two separate lines `["a", "b"]` in that order.
#[test]
fn stdout_two_lines_parity() {
    let src = "stdout(\"a\")\nstdout(\"b\")\n";
    assert_eq!(run_streaming(src), vec!["a".to_string(), "b".to_string()]);
    assert_eq!(run_buffered(src), vec!["a".to_string(), "b".to_string()]);
}

/// Interleaved stdout + debug must preserve relative order in both modes. In
/// stream mode both writers target stdout so order is observed directly.
#[test]
fn stdout_debug_interleave_parity() {
    let src = "stdout(\"x\")\ndebug(\"y\")\nstdout(\"z\")\n";
    assert_eq!(
        run_streaming(src),
        vec!["x".to_string(), "y".to_string(), "z".to_string()]
    );
    assert_eq!(
        run_buffered(src),
        vec!["x".to_string(), "y".to_string(), "z".to_string()]
    );
}

/// `stdout(s)` must return the payload byte count (excluding the implicit
/// `\n`). This is a long-standing C12-5 / FB-18 contract — C22 must not
/// silently drop it. We verify via a script that prints the returned count.
#[test]
fn stdout_returns_byte_count() {
    // `n <= stdout("hello")` then print n; expect 5 for ASCII "hello".
    let src = "n <= stdout(\"hello\")\nstdout(n)\n";
    let lines = run_streaming(src);
    assert_eq!(lines, vec!["hello".to_string(), "5".to_string()]);

    // Same expectation from buffered mode (for REPL / test consumers).
    let buf = run_buffered(src);
    assert_eq!(buf, vec!["hello".to_string(), "5".to_string()]);
}

/// Empty script with a final non-Unit expression auto-displays that value in
/// CLI stream mode — matches the legacy buffered behavior where the final
/// value was printed when `output.is_empty()`. We keep that ergonomics via
/// the new `stdout_emissions` counter.
#[test]
fn pure_expression_final_value_still_printed() {
    let src = "42\n";
    assert_eq!(run_streaming(src), vec!["42".to_string()]);
}

/// After an explicit `stdout(...)`, the final-value auto-display is suppressed
/// (same as the old `output.is_empty()` check).
#[test]
fn pure_expression_suppressed_after_stdout() {
    let src = "stdout(\"explicit\")\n42\n";
    assert_eq!(run_streaming(src), vec!["explicit".to_string()]);
}

// Silence: `Write` import is only used indirectly via process piping above.
#[allow(dead_code)]
fn _touch_write() {
    let _ = std::io::stderr().flush();
}
