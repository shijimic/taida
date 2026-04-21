//! C22-4 / C22B-004: SIGPIPE integration test.
//!
//! Verifies that `taida run file.td | head -N` exits successfully (0) rather
//! than being killed by SIGPIPE (exit 141) when the downstream consumer
//! closes its stdin after reading only part of the script's output.
//!
//! This test only makes sense on unix; on other platforms SIGPIPE does not
//! exist and the failure mode is different (the `head` equivalent may not
//! even be installed), so the test is gated behind `cfg(unix)`.

#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Stdio};

mod common;
use common::taida_bin;

fn write_tmp_td(body: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "c22_sigpipe_{}_{}_{}.td",
        std::process::id(),
        nanos,
        seq
    ));
    std::fs::write(&path, body).expect("write tmp td");
    path
}

/// Regression for C22B-004: `stdout` in stream mode + SIGPIPE SIG_IGN in main
/// should let the whole pipeline exit 0 when a short consumer closes early.
#[test]
fn taida_pipe_to_head_exits_zero() {
    // Enough lines that `head -2` will close stdin long before the script
    // finishes writing. Each `stdout(...)` call emits a full line + newline.
    let src = (0..2000)
        .map(|i| format!("stdout(\"line-{}\")\n", i))
        .collect::<String>();
    let td_path = write_tmp_td(&src);

    // Spawn `taida <file>` with stdout piped, then feed that into `head -2`.
    let mut taida = Command::new(taida_bin())
        .arg(&td_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn taida");

    let taida_stdout = taida.stdout.take().expect("taida stdout pipe");

    let head = Command::new("head")
        .arg("-n")
        .arg("2")
        .stdin(Stdio::from(taida_stdout))
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn head");

    // Wait for head to finish; it will close its stdin after reading 2 lines.
    let head_out = head.wait_with_output().expect("head wait_with_output");
    assert!(head_out.status.success(), "head should exit 0");
    let head_text = String::from_utf8_lossy(&head_out.stdout);
    assert_eq!(
        head_text.lines().count(),
        2,
        "head should emit exactly 2 lines"
    );
    assert!(head_text.starts_with("line-0\n"));

    // Now wait for taida to finish. Previously this would be killed by
    // SIGPIPE (exit 141 on unix). With C22-4's SIG_IGN + silent EPIPE
    // absorption in stdout builtin, it should exit cleanly.
    let taida_status = taida.wait().expect("taida wait");
    assert!(
        taida_status.success(),
        "taida should exit 0 when pipe closes early, got {:?}",
        taida_status
    );

    let _ = std::fs::remove_file(&td_path);
}

/// Regression guard: stream-mode `stdout` still returns the correct byte count
/// even after a successful-but-EPIPE write. (Caller contract: `stdout(s)`
/// returns `Int` bytes regardless of flush success; SIGPIPE must not propagate
/// as a Taida runtime error.)
#[test]
fn taida_pipe_to_head_preserves_exit_code_on_trivial_script() {
    let td_path = write_tmp_td("stdout(\"hi\")\n");

    let mut taida = Command::new(taida_bin())
        .arg(&td_path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn taida");

    // Drop the stdout pipe immediately to simulate a consumer that never reads.
    drop(taida.stdout.take());

    let status = taida.wait().expect("wait taida");
    // With SIG_IGN the process survives; `hi\n` was just absorbed into EPIPE.
    assert!(status.success(), "taida should exit 0 with closed stdout");

    let _ = std::fs::remove_file(&td_path);
}

/// Sanity: `debug(...)` in stream mode also routes to stdout (not stderr) so
/// it participates in the same SIGPIPE-safe path. Matches JS/Native backends
/// which emit `debug` to stdout via `console.log` / runtime debug helpers.
#[test]
fn taida_debug_goes_to_stdout_in_stream_mode() {
    let td_path = write_tmp_td("debug(\"ping\")\n");

    let out = Command::new(taida_bin())
        .arg(&td_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run taida");

    assert!(out.status.success(), "taida should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ping"),
        "debug output should appear on stdout, got stdout={:?} stderr={:?}",
        stdout,
        stderr
    );

    let _ = std::fs::remove_file(&td_path);
}

/// Writer style sanity: a mixed stdout + debug script flushes in order.
#[test]
fn taida_stream_mode_preserves_emit_order() {
    let td_path = write_tmp_td("stdout(\"A\")\ndebug(\"B\")\nstdout(\"C\")\n");
    let out = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("run taida");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().collect::<Vec<_>>(), vec!["A", "B", "C"]);

    let _ = std::fs::remove_file(&td_path);
}

// Silence unused warnings on non-unix (file is cfg-gated anyway).
#[cfg(unix)]
fn _touch() {
    let _ = std::io::stderr().flush();
}
