//! C20-3: 3-backend parity harness for the existing `stdin` API.
//!
//! Before C20 the three backends disagreed on:
//!
//!   * **ROOT-8 (Native)** — `char[4096]` stack buffer truncated long
//!     input; the tail bled into the next `stdin` call.
//!   * **ROOT-9 (Interpreter)** — `read_line` `Err` was surfaced as a
//!     throw (`IoError`), while JS / Native silently returned `""`.
//!     Callers could not write portable code.
//!   * **ROOT-10 (JS)** — `readSync(fd, buf, 0, 1)` decoded one byte at
//!     a time via `Buffer.toString('utf-8', 0, 1)`, turning every
//!     continuation byte into U+FFFD.
//!
//! After C20:
//!
//!   * All three backends return `""` on EOF / read error.
//!   * `stdin` accepts arbitrarily long lines (dynamic buffer on Native
//!     via `getline` / realloc loop).
//!   * JS decodes with a streaming `TextDecoder('utf-8', { stream })` so
//!     multibyte codepoints survive chunk boundaries.
//!
//! The `stdin()` form without a prompt is also valid (ROOT-13:
//! checker arity moved from `(1, 1)` to `(0, 1)`).
//!
//! Red test ゼロ容認 — any divergence is a C20 regression.

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
    manifest_dir().join("examples/quality/c20_stdin")
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

/// Return the bytes that the parity test should feed through stdin for
/// the given fixture stem. Using an explicit match (rather than reading
/// a file) keeps CRLF literals, UTF-8 payloads, and long-line sizes
/// auditable in a single place.
fn stdin_payload(stem: &str) -> Vec<u8> {
    match stem {
        "stdin_eof" => Vec::new(),
        "stdin_long_line" => {
            // 8191 ASCII "a" + one sentinel "Z" + newline.
            let mut v = vec![b'a'; 8191];
            v.push(b'Z');
            v.push(b'\n');
            v
        }
        "stdin_crlf" => b"hello\r\n".to_vec(),
        "stdin_utf8" => "こんにちは\n".as_bytes().to_vec(),
        other => panic!("unknown stdin fixture '{}'", other),
    }
}

// ---- backend drivers ---------------------------------------------

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
        "C20-3 interpreter output mismatch for '{}'.\n--- expected ---\n{}\n--- got ---\n{}\n",
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
    let stdout = String::from_utf8_lossy(&node_out.stdout).to_string();
    let expected = read_expected(stem);
    assert!(
        outputs_equal(&stdout, &expected),
        "C20-3 JS output mismatch for '{}' (interpreter is reference).\n\
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
        "C20-3 native output mismatch for '{}' (interpreter is reference).\n\
         --- expected ---\n{}\n--- got ---\n{}\n",
        stem,
        expected,
        stdout
    );
}

// ── stdin EOF parity (ROOT-9) ──

#[test]
fn c20_stdin_eof_interpreter_returns_empty_str() {
    assert_interpreter("stdin_eof");
}

#[test]
fn c20_stdin_eof_js_matches_interpreter() {
    assert_js_matches("stdin_eof");
}

#[test]
fn c20_stdin_eof_native_matches_interpreter() {
    assert_native_matches("stdin_eof");
}

// ── stdin long-line parity (ROOT-8) ──

#[test]
fn c20_stdin_long_line_interpreter_not_truncated() {
    assert_interpreter("stdin_long_line");
}

#[test]
fn c20_stdin_long_line_js_matches_interpreter() {
    assert_js_matches("stdin_long_line");
}

#[test]
fn c20_stdin_long_line_native_matches_interpreter() {
    assert_native_matches("stdin_long_line");
}

// ── stdin CRLF stripping parity ──

#[test]
fn c20_stdin_crlf_interpreter_strips_crlf() {
    assert_interpreter("stdin_crlf");
}

#[test]
fn c20_stdin_crlf_js_matches_interpreter() {
    assert_js_matches("stdin_crlf");
}

#[test]
fn c20_stdin_crlf_native_matches_interpreter() {
    assert_native_matches("stdin_crlf");
}

// ── stdin UTF-8 parity (ROOT-10) ──

#[test]
fn c20_stdin_utf8_interpreter_preserves_multibyte() {
    assert_interpreter("stdin_utf8");
}

#[test]
fn c20_stdin_utf8_js_matches_interpreter() {
    assert_js_matches("stdin_utf8");
}

#[test]
fn c20_stdin_utf8_native_matches_interpreter() {
    assert_native_matches("stdin_utf8");
}

// ── ROOT-13: checker accepts `stdin()` no-prompt form ──

#[test]
fn c20_stdin_no_prompt_form_passes_check() {
    let script = "line <= stdin()\nstdout(\"ok=\" + line)\n";
    let src = unique_temp("c20_stdin_no_prompt", "td");
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
        "`taida way check` should accept `stdin()` no-prompt form after C20-3 ROOT-13 fix. stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── ROOT-14: JS tolerates non-Str prompt via display-string coercion ──

#[test]
fn c20_stdin_js_non_string_prompt_does_not_crash() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    // `stdin(1)` — Int prompt. Interpreter / Native stringify via
    // display helpers. Before C20 the JS runtime handed the raw value
    // to `process.stdout.write`, raising ERR_INVALID_ARG_TYPE outside
    // the try/catch. After C20 the prompt is wrapped in `String(...)`.
    let script = "line <= stdin(1)\nstdout(\"ok\")\n";
    let src = unique_temp("c20_stdin_js_int_prompt", "td");
    fs::write(&src, script).expect("write temp td");

    let mjs = unique_temp("c20_stdin_js_int_prompt", "mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&mjs)
        .output()
        .expect("js build");
    let _ = fs::remove_file(&src);
    assert!(
        build.status.success(),
        "js build for int prompt failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut cmd = Command::new("node");
    cmd.arg(&mjs);
    let run = run_with_stdin(&mut cmd, b"hello\n");
    let _ = fs::remove_file(&mjs);
    assert!(
        run.status.success(),
        "node runtime crashed on non-Str prompt. stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = String::from_utf8_lossy(&run.stdout).to_string();
    // Expect the prompt "1" to have been written to stdout before "ok".
    assert!(
        out.contains("ok"),
        "expected 'ok' in JS stdout, got={:?}",
        out
    );
}
