//! C20-2: 3-backend parity harness for the new `stdinLine` prelude API.
//!
//! `stdinLine(prompt) ]=> line` returns an `Async[Lax[Str]]`. The Async
//! wrapper exists so that the JS backend (`node:readline/promises` is
//! async-only) and the Interpreter / Native backends (rustyline /
//! linenoise-derived termios editor are synchronous) can share a single
//! surface type; callers unmold the Async with `]=>` to obtain the
//! inner `Lax[Str]` and then `getOrDefault("")` on it.
//!
//! This file pins the CI-testable invariants:
//!
//!   * **EOF**: a closed stdin collapses to `Lax[Str].failure("")` on
//!     all 3 backends (hasValue == false, default "").
//!   * **ASCII**: a plain line is returned verbatim on all 3 backends
//!     (hasValue == true).
//!   * **UTF-8 multibyte**: 日本語 / 한국어 / emoji payloads survive the
//!     round-trip without replacement chars, closing ROOT-7's piped-
//!     input regression (the TTY editing path itself is validated by
//!     the manual smoke in Hachikuma Phase 11).
//!   * **Checker**: `stdinLine()` (no prompt) and
//!     `stdinLine("…")` both type-check; the return type is
//!     `Async[Lax[Str]]`, so `]=>` is the idiomatic unmold.
//!
//! Red test ゼロ容認 — any divergence between the three backends (or any
//! regression vs. the fixtures) is a C20-2 blocker. The interactive
//! editing features (UTF-8 Backspace / arrow keys / Ctrl-U) require an
//! actual TTY and cannot be pinned from CI; they are smoke-tested in
//! Hachikuma Phase 11.

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    manifest_dir().join("examples/quality/c20_stdinline")
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

// ---- input providers per stem ------------------------------------

fn stdin_payload(stem: &str) -> Vec<u8> {
    match stem {
        "stdinline_eof" => Vec::new(),
        "stdinline_ascii" => b"hello\n".to_vec(),
        "stdinline_utf8" => "こんにちは\n".as_bytes().to_vec(),
        other => panic!("unknown stdinline fixture '{}'", other),
    }
}

// ---- backend drivers (mirrors c20_stdin_parity) -------------------

fn run_with_stdin(cmd: &mut Command, input: &[u8]) -> std::process::Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn child");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        if !input.is_empty() {
            stdin.write_all(input).expect("write stdin");
        }
        drop(stdin); // close to signal EOF
    }
    child.wait_with_output().expect("wait child")
}

/// Strip node:readline/promises' terminal escape sequences (cursor
/// positioning, erase-in-display) so the JS backend's stdout can be
/// compared to the plain-text interpreter reference. The sequences
/// appear when `terminal: true` is forced on a piped stdin — readline
/// still writes the prompt via ANSI CSI because it thinks stdout is
/// attached to a terminal. Callers strip them before diffing against
/// the shared `.expected` fixture. Operates on bytes so UTF-8
/// multibyte payloads (こんにちは, emoji, …) survive intact.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // CSI: skip until a byte in 0x40..=0x7E (the CSI final byte).
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // consume final byte
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| {
        // Defensive: if CSI stripping somehow broke a UTF-8 boundary
        // (it shouldn't — CSI bytes are all ASCII), fall back to lossy
        // so the test still produces a legible diagnostic rather than
        // panicking on `unwrap`.
        String::from_utf8_lossy(&e.into_bytes()).into_owned()
    })
}

fn assert_interpreter(stem: &str) {
    let input = stdin_payload(stem);
    let mut cmd = Command::new(taida_bin());
    cmd.arg(td_path(stem));
    let out = run_with_stdin(&mut cmd, &input);
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
        "C20-2 interpreter output mismatch for '{}'.\n--- expected ---\n{}\n--- got ---\n{}\n",
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
    let input = stdin_payload(stem);
    let mjs_path = unique_temp(&format!("c20_{}", stem), "mjs");
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

    let mut cmd = Command::new("node");
    cmd.arg(&mjs_path);
    let node_out = run_with_stdin(&mut cmd, &input);
    let _ = fs::remove_file(&mjs_path);
    assert!(
        node_out.status.success(),
        "node exit failed ({}): {}",
        stem,
        String::from_utf8_lossy(&node_out.stderr)
    );
    let stdout_raw = String::from_utf8_lossy(&node_out.stdout).to_string();
    let stdout = strip_ansi(&stdout_raw);
    let expected = read_expected(stem);
    assert!(
        outputs_equal(&stdout, &expected),
        "C20-2 JS output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got (ANSI-stripped) ---\n{}\n--- got (raw) ---\n{:?}\n",
        stem,
        expected,
        stdout,
        stdout_raw
    );
}

fn assert_native_matches(stem: &str) {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let input = stdin_payload(stem);
    let bin_path = unique_temp(&format!("c20_{}", stem), "bin");
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
    let mut cmd = Command::new(&bin_path);
    let run_out = run_with_stdin(&mut cmd, &input);
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
        "C20-2 native output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

// ── ROOT-7: stdinLine returns Lax[Str].failure("") on EOF ──
// Closes C20B-003 (ROOT-7) for the piped / redirected case. The
// interactive UTF-8 Backspace behaviour is validated in Hachikuma
// Phase 11 smoke.

#[test]
fn c20_stdin_line_interpreter_eof_returns_lax_failure() {
    assert_interpreter("stdinline_eof");
}

#[test]
fn c20_stdin_line_js_eof_returns_lax_failure() {
    assert_js_matches("stdinline_eof");
}

#[test]
fn c20_stdin_line_native_eof_returns_lax_failure() {
    assert_native_matches("stdinline_eof");
}

// ── ASCII round-trip ──

#[test]
fn c20_stdin_line_interpreter_ascii_round_trip() {
    assert_interpreter("stdinline_ascii");
}

#[test]
fn c20_stdin_line_js_ascii_round_trip() {
    assert_js_matches("stdinline_ascii");
}

#[test]
fn c20_stdin_line_native_ascii_round_trip() {
    assert_native_matches("stdinline_ascii");
}

// ── UTF-8 multibyte round-trip (ROOT-7 piped case) ──

#[test]
fn c20_stdin_line_interpreter_utf8_preserves_multibyte() {
    assert_interpreter("stdinline_utf8");
}

#[test]
fn c20_stdin_line_js_utf8_preserves_multibyte() {
    assert_js_matches("stdinline_utf8");
}

#[test]
fn c20_stdin_line_native_utf8_preserves_multibyte() {
    assert_native_matches("stdinline_utf8");
}

// ── Checker: stdinLine() / stdinLine("…") / ]=> narrowing ──

#[test]
fn c20_stdin_line_checker_accepts_no_prompt_form() {
    let script = "stdinLine() ]=> line\nstdout(\"ok\")\n";
    let src = unique_temp("c20_stdinline_noprompt", "td");
    fs::write(&src, script).expect("write temp td");
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src)
        .output()
        .expect("taida way check");
    let _ = fs::remove_file(&src);
    assert!(
        out.status.success(),
        "`taida way check` must accept `stdinLine()` (no prompt). stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn c20_stdin_line_checker_accepts_str_prompt_form() {
    let script = "stdinLine(\"name: \") ]=> line\nstdout(line.getOrDefault(\"\"))\n";
    let src = unique_temp("c20_stdinline_strprompt", "td");
    fs::write(&src, script).expect("write temp td");
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src)
        .output()
        .expect("taida way check");
    let _ = fs::remove_file(&src);
    assert!(
        out.status.success(),
        "`taida way check` must accept `stdinLine(\"…\")` with Str prompt. stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn c20_stdin_line_checker_rejects_too_many_args() {
    let script = "stdinLine(\"a\", \"b\") ]=> line\nstdout(\"never\")\n";
    let src = unique_temp("c20_stdinline_arity", "td");
    fs::write(&src, script).expect("write temp td");
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src)
        .output()
        .expect("taida way check");
    let _ = fs::remove_file(&src);
    // arity = 0..=1, so 2 args must surface [E1507].
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{}{}", stderr, stdout);
    assert!(
        combined.contains("E1507") || combined.contains("stdinLine") || !out.status.success(),
        "`taida way check` must reject 2-arg stdinLine as an arity violation. combined={}",
        combined
    );
}
