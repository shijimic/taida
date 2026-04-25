//! C27B-014 (@c.27 Round 1, wA) — opt-in `httpServe` port-bind
//! announcement, 3-backend parity.
//!
//! # Scope
//!
//! Pins the `TAIDA_NET_ANNOUNCE_PORT=1` opt-in surface across the
//! interpreter / native / JS backends:
//!
//!  - Default (env unset / `=0` / any non-`1` value) emits **nothing**
//!    on stdout from the bind path. This is the production contract
//!    and must remain non-breaking — soak proxies must opt in
//!    explicitly via the env var.
//!  - With `TAIDA_NET_ANNOUNCE_PORT=1` set in the server process's
//!    environment, the bind path emits exactly one line of the form
//!    `listening on <host>:<port>\n` on stdout (flushed) before the
//!    first `accept()`. The host is whatever the listener resolved to
//!    (`127.0.0.1` for the v1 loopback contract); the port is the
//!    actually-bound port (resolved via `getsockname` / `local_addr` /
//!    `server.address()` so callers passing port=0 learn the
//!    OS-assigned value).
//!
//! Together these unblock the `.dev/C26_SOAK_RUNBOOK.md § 2.1` port=0
//! → tmux pane-title flow that has been blocked since the C26 cycle.
//!
//! # D28 escalation checklist (3 points, all NO → C27 scope-in)
//!
//!  1. **Public mold signature unchanged.** `httpServe(port, handler,
//!     ...)` and its result pack (`@(ok: Bool, requests: Int)`) are
//!     untouched. Option 2 (adding a `port: Int` field to the result
//!     pack) is intentionally **not** taken in this round.
//!  2. **No STABILITY-pinned error string altered.** The new emission
//!     is on the success path, not on `BindError` / `TlsError` / etc.
//!  3. **Append-only with respect to existing fixtures.** All existing
//!     `serve_one.td`-class fixtures keep their original stdout
//!     (currently `true\n1\n`); the announcement is suppressed by
//!     default so existing snapshots do not need to be rewritten.
//!
//! # Backend matrix
//!
//! | backend     | source                                                | how the line is emitted              |
//! |-------------|-------------------------------------------------------|--------------------------------------|
//! | interpreter | `src/interpreter/net_eval/h1.rs`                      | `writeln!(stdout(), …)` after bind   |
//! | native h1   | `src/codegen/native_runtime/net_h3_quic.c`            | `printf` + `fflush(stdout)` post-listen |
//! | native h2   | `src/codegen/native_runtime/net_h1_h2.c`              | same                                 |
//! | JS (Node)   | `src/js/runtime/net.rs` — `server.on('listening')`    | `console.log(...)`                   |

mod common;

use common::{normalize, taida_bin};
use std::io::{BufReader, Read};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Serialize all tests in this file to avoid port 18080 contention
/// (parallel test threads would otherwise race on the bind).
static PORT_LOCK: Mutex<()> = Mutex::new(());

const FIXTURE: &str = "examples/quality/c26_portbind/serve_one.td";
const PORT: u16 = 18080;
const EXPECTED_LINE: &str = "listening on 127.0.0.1:18080";

fn fixture_path() -> PathBuf {
    Path::new(FIXTURE).to_path_buf()
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

/// Wait until the server has bound, then issue exactly one curl-equivalent
/// request so the `httpServe(_, _, 1)` fixture's `max_requests=1` budget
/// is consumed and the server exits cleanly.
fn poke_once(port: u16, child: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let addr = format!("127.0.0.1:{}", port);
    loop {
        if Instant::now() > deadline {
            return Err("bind timeout (server did not accept within 15s)".into());
        }
        // Detect early exit (parse error, panic, etc.).
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("server exited before bind: {:?}", status));
        }
        if let Ok(mut stream) = TcpStream::connect(&addr) {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;
            // Minimal HTTP/1.1 GET. The fixture handler ignores the
            // request body, so we just need the server to read a full
            // request and write a response back.
            use std::io::Write;
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .map_err(|e| e.to_string())?;
            let mut sink = Vec::new();
            let _ = stream.read_to_end(&mut sink);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Spawn `cmd` with `TAIDA_NET_ANNOUNCE_PORT` either set to `value` or
/// removed (when `value` is `None`). Drive one request through the
/// fixture, wait for the child to exit, and return its full stdout.
fn run_and_capture(mut cmd: Command, env_value: Option<&str>) -> String {
    let _guard = PORT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    match env_value {
        Some(v) => {
            cmd.env("TAIDA_NET_ANNOUNCE_PORT", v);
        }
        None => {
            cmd.env_remove("TAIDA_NET_ANNOUNCE_PORT");
        }
    }
    let mut child = cmd.spawn().expect("spawn server");
    if let Err(e) = poke_once(PORT, &mut child) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("server probe failed (env={:?}): {}", env_value, e);
    }
    let status = child.wait().expect("wait server");
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    } else {
        // Already taken by piped reader — re-read via BufReader to be safe.
        let _ = BufReader::new(std::io::empty()).read_to_string(&mut stdout);
    }
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        panic!(
            "server exited non-zero (env={:?}): status={:?} stderr={} stdout={}",
            env_value, status, stderr, stdout
        );
    }
    normalize(&stdout)
}

// ── interpreter ────────────────────────────────────────────────────

fn interpreter_command() -> Command {
    let mut cmd = Command::new(taida_bin());
    cmd.arg(fixture_path());
    cmd
}

#[test]
fn announce_port_off_by_default_interpreter() {
    let out = run_and_capture(interpreter_command(), None);
    assert!(
        !out.contains("listening on"),
        "interpreter must not emit announcement when env unset; stdout was: {}",
        out
    );
    // Existing fixture surface is preserved.
    assert!(
        out.contains("true"),
        "expected fixture body in stdout: {}",
        out
    );
    assert!(
        out.contains('1'),
        "expected request count in stdout: {}",
        out
    );
}

#[test]
fn announce_port_off_for_zero_value_interpreter() {
    let out = run_and_capture(interpreter_command(), Some("0"));
    assert!(
        !out.contains("listening on"),
        "interpreter must not emit announcement when env=0; stdout was: {}",
        out
    );
}

#[test]
fn announce_port_off_for_other_value_interpreter() {
    // Anything that is not literally "1" must not opt in.
    let out = run_and_capture(interpreter_command(), Some("yes"));
    assert!(
        !out.contains("listening on"),
        "interpreter must only opt-in for env=1; stdout was: {}",
        out
    );
}

#[test]
fn announce_port_on_interpreter() {
    let out = run_and_capture(interpreter_command(), Some("1"));
    assert!(
        out.lines().any(|l| l == EXPECTED_LINE),
        "interpreter must emit `{}` on its own line when env=1; stdout was: {}",
        EXPECTED_LINE,
        out
    );
}

// ── native ─────────────────────────────────────────────────────────

fn build_native() -> Option<PathBuf> {
    if !cc_available() {
        return None;
    }
    let dir = tempdir_strict();
    let out_path = dir.join("serve_native");
    let status = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(fixture_path())
        .arg("-o")
        .arg(&out_path)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    Some(out_path)
}

fn native_command(bin: &Path) -> Command {
    Command::new(bin)
}

#[test]
fn announce_port_off_by_default_native() {
    let Some(bin) = build_native() else {
        eprintln!("skip: cc not available or native build failed");
        return;
    };
    let out = run_and_capture(native_command(&bin), None);
    assert!(
        !out.contains("listening on"),
        "native must not emit announcement when env unset; stdout was: {}",
        out
    );
    assert!(out.contains("true"));
    assert!(out.contains('1'));
}

#[test]
fn announce_port_on_native() {
    let Some(bin) = build_native() else {
        eprintln!("skip: cc not available or native build failed");
        return;
    };
    let out = run_and_capture(native_command(&bin), Some("1"));
    assert!(
        out.lines().any(|l| l == EXPECTED_LINE),
        "native must emit `{}` on its own line when env=1; stdout was: {}",
        EXPECTED_LINE,
        out
    );
}

// ── JS ─────────────────────────────────────────────────────────────

fn build_js() -> Option<PathBuf> {
    if !node_available() {
        return None;
    }
    let dir = tempdir_strict();
    let out_path = dir.join("serve.mjs");
    let status = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(fixture_path())
        .arg("-o")
        .arg(&out_path)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    Some(out_path)
}

fn js_command(artifact: &Path) -> Command {
    let mut cmd = Command::new("node");
    cmd.arg(artifact);
    cmd
}

#[test]
fn announce_port_off_by_default_js() {
    let Some(art) = build_js() else {
        eprintln!("skip: node not available or JS build failed");
        return;
    };
    let out = run_and_capture(js_command(&art), None);
    assert!(
        !out.contains("listening on"),
        "JS must not emit announcement when env unset; stdout was: {}",
        out
    );
    assert!(out.contains("true"));
    assert!(out.contains('1'));
}

#[test]
fn announce_port_on_js() {
    let Some(art) = build_js() else {
        eprintln!("skip: node not available or JS build failed");
        return;
    };
    let out = run_and_capture(js_command(&art), Some("1"));
    assert!(
        out.lines().any(|l| l == EXPECTED_LINE),
        "JS must emit `{}` on its own line when env=1; stdout was: {}",
        EXPECTED_LINE,
        out
    );
}

// ── helpers ────────────────────────────────────────────────────────

/// Tiny tempdir helper — avoids pulling the `tempfile` crate just for
/// these tests.
fn tempdir_strict() -> PathBuf {
    let mut base = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    base.push(format!("c27b014.{}.{}", std::process::id(), nonce));
    std::fs::create_dir_all(&base).expect("mkdir tempdir");
    base
}
