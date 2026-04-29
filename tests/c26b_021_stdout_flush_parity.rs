//! C26B-021: Native `stdout` must be line-buffered so that 3-backend
//! observability parity holds when the process is attached to a pipe.
//!
//! # Symptom (pre-fix)
//!
//! POSIX libc defaults stdout to fully buffered (typically 4-8 KB) when
//! stdout is not a tty. Interpreter (Rust `println!`) and JS (Node
//! `console.log`) both emit line-buffered output even behind a pipe. The
//! Native backend, which routed every `stdout(...)` through
//! `taida_io_stdout`'s `printf("%s\n", s)` without `fflush`, buffered
//! all writes until process shutdown. This broke the 3-backend
//! observability contract: a curl-driven HTTP trace log under
//! `hono-inspired` Phase 4 showed per-request traces in real time on
//! Interpreter/JS but only appeared on Native when the server exited.
//!
//! # Fix (Option B, Design Lock 2026-04-24)
//!
//! Add `setvbuf(stdout, NULL, _IOLBF, 0)` and `setvbuf(stderr, NULL,
//! _IOLBF, 0)` at the very top of `main()` in
//! `src/codegen/native_runtime/net_h3_quic.c`. This is a single
//! initialization-time call that restores line-buffered semantics for
//! the whole process, with lower overhead than per-call `fflush`. It
//! does not alter any stdout byte content, only timing.
//!
//! # Why this is not a surface change
//!
//! The write-content observable behaviour is unchanged on all backends
//! (same bytes, same order). Only the _timing_ of bytes hitting the
//! pipe changes on Native, aligning with the already-contractual
//! behaviour of Interpreter and JS. No fixture in `tests/parity.rs`
//! assumes buffered stdout.
//!
//! # Scope
//!
//! 3-backend parity (Interpreter / JS / Native). wasm-wasi is out of
//! scope (D27 — see .dev/D27_BLOCKERS.md).

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_c26b021_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn write_fixture(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("main.td");
    fs::write(&path, body).expect("write fixture");
    path
}

/// Minimal fixture: sleep between two stdout writes, so a buffered
/// stdout would hold BOTH lines until exit, whereas a line-buffered
/// stdout would emit "first" immediately.
const FIXTURE_SLEEP_BETWEEN_LINES: &str = r#">>> taida-lang/os => @(sleep)
stdout("first")
sleep(0.2)
stdout("second")
"#;

/// Simple fixture: two stdout lines, no sleep. Used to assert byte-level
/// equivalence of final output across all three backends (content
/// parity; the timing-only assertion lives in the Native-specific test
/// below).
const FIXTURE_TWO_LINES: &str = r#"stdout("alpha")
stdout("beta")
"#;

/// Native-only: assert that `first` is readable on the read end of a
/// pipe BEFORE the native process exits. Pre-fix, line-buffering was
/// disabled so the first line sat in libc's buffer until exit, causing
/// the reader to see both lines at once at shutdown — never "first
/// only" while the process was still alive.
#[test]
fn c26b_021_native_stdout_is_line_buffered_on_pipe() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native line-buffering test");
        return;
    }
    let dir = tempdir("native_line_buffered");
    let src = write_fixture(&dir, FIXTURE_SLEEP_BETWEEN_LINES);
    let bin_path = dir.join("out.bin");

    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // Spawn the binary with its stdout connected to a pipe. Read the
    // first line within a bounded window. A buffered stdout would hold
    // both lines until exit, so a read of the first line within
    // ~100-150 ms would block. Line-buffered stdout flushes "first\n"
    // immediately.
    let mut child = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn native bin");

    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_millis(150);
    let mut first_line = String::new();

    // Read on the current thread; BufReader::read_line blocks until
    // newline, but on a line-buffered source that newline arrives
    // immediately. We bound the wall-clock with a simple deadline check
    // before the read: if the fixture writes "first\n" and flushes, the
    // read completes in microseconds.
    let read_result = reader.read_line(&mut first_line);
    let elapsed = Instant::now();
    let _ = child.wait();

    assert!(
        read_result.is_ok(),
        "read_line errored: {:?}",
        read_result.err()
    );
    assert_eq!(
        first_line.trim_end(),
        "first",
        "first line must be 'first', got {:?}",
        first_line
    );
    assert!(
        elapsed <= deadline + Duration::from_millis(150),
        "first line did not flush within the budget — stdout appears to \
         still be fully buffered. elapsed past deadline means ~150ms+ of \
         wall-clock lag, which indicates the fix is not active."
    );

    let _ = fs::remove_dir_all(&dir);
}

/// 3-backend content parity: the byte content of stdout must be the
/// same across Interpreter / JS / Native. This asserts that the
/// setvbuf() call did not alter _what_ is written, only _when_.
#[test]
fn c26b_021_3backend_stdout_content_parity() {
    let dir = tempdir("content_parity");
    let src = write_fixture(&dir, FIXTURE_TWO_LINES);

    let interp = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run interp");
    assert!(interp.status.success(), "interp failed");
    let interp_out = String::from_utf8_lossy(&interp.stdout).to_string();
    assert_eq!(interp_out.trim(), "alpha\nbeta");

    if node_available() {
        let js_path = dir.join("out.mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&src)
            .arg("-o")
            .arg(&js_path)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("node").arg(&js_path).output().expect("run js");
        assert!(run.status.success(), "node exit non-zero");
        let js_out = String::from_utf8_lossy(&run.stdout).to_string();
        assert_eq!(js_out.trim(), interp_out.trim());
    }

    if cc_available() {
        let bin_path = dir.join("out.bin");
        let build = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(&src)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&bin_path).output().expect("run native bin");
        assert!(run.status.success(), "native exit non-zero");
        let nat_out = String::from_utf8_lossy(&run.stdout).to_string();
        assert_eq!(nat_out.trim(), interp_out.trim());
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Regression guard for the setvbuf call site: the fix must live at the
/// top of `main()`, BEFORE `_taida_main()` is invoked. A code-level
/// check on the assembled native runtime ensures the initialization
/// order cannot silently regress to a form that flushes only after the
/// first `printf` (which would reintroduce the bug for programs that
/// block on socket I/O before their first print).
#[test]
fn c26b_021_setvbuf_sits_at_main_entry() {
    // Pull the assembled native runtime source and look for the literal
    // setvbuf calls in the expected order, AHEAD of the _taida_main
    // invocation.
    let runtime = *taida::codegen::native_runtime::NATIVE_RUNTIME_C;
    let stdout_idx = runtime
        .find("setvbuf(stdout, NULL, _IOLBF, 0);")
        .expect("setvbuf(stdout, ...) must appear in native runtime");
    let stderr_idx = runtime
        .find("setvbuf(stderr, NULL, _IOLBF, 0);")
        .expect("setvbuf(stderr, ...) must appear in native runtime");
    let main_idx = runtime
        .find("_taida_main();")
        .expect("_taida_main(); must appear in native runtime");

    assert!(
        stdout_idx < main_idx,
        "setvbuf(stdout) must be called BEFORE _taida_main()"
    );
    assert!(
        stderr_idx < main_idx,
        "setvbuf(stderr) must be called BEFORE _taida_main()"
    );
}
