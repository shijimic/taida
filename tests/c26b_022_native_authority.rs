//! C26B-022 Step 2 (wS Round 6, 2026-04-24) — Native parity fixture for
//! HTTP wire-byte upper limits (method 16 / path 2048 / authority 256).
//!
//! # Scope
//!
//! The interpreter h1 parser already rejects over-limit method / path /
//! Host-header value with `400 Bad Request` at the parser boundary (see
//! `tests/parity.rs::test_net6_c26b022_*` — landed in wE Round 3 and
//! extended for authority in wJ Round 4). This file pins the **Native**
//! (`taida build --target native` + run the binary) side of the same
//! 3-backend parity contract by asserting that a Native-compiled HTTP/1
//! server also returns `HTTP/1.1 400 Bad Request` for:
//!
//! - a 17-byte method (limit = 16)
//! - a 2049-byte path   (limit = 2048)
//! - a 257-byte Host    (limit = 256)
//!
//! and accepts their at-limit (16 / 2048 / 256) counterparts with
//! `HTTP/1.1 200 OK`.
//!
//! Option confirmation: **Step 3 Option B** (parser-level reject) per
//! Phase 0 Design Lock. Option A (dynamic struct buffers) was discarded.
//!
//! # Why a separate file
//!
//! `tests/parity.rs` already landed interp-side fixtures in wE and wJ.
//! Putting the Native extension as a new top-level test file keeps the
//! diff clean, isolates the build-target invocation pattern from the
//! rest of `parity.rs`, and avoids touching any existing assertion.
//!
//! # D27 escalation checklist (3 points, all NO)
//!
//! 1. No public mold signature changes (internal parser behaviour only).
//! 2. No STABILITY-pinned error string altered (`400 Bad Request` is an
//!    industry-standard HTTP status line, not a Taida-pinned token).
//! 3. Append-only: new test file, no existing assertion modified.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
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

fn tempdir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "taida_c26b022_native_{}_{}_{}",
        name,
        std::process::id(),
        nanos
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

/// Find a free loopback TCP port. Minimal local version — we do not need
/// the full parity.rs cooldown machinery here because the Native binary
/// binds the port freshly inside its own process.
fn find_free_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Write a taida source file that starts an HTTP/1 server bound to
/// `port`, accepting exactly 1 request, with a 5 s shutdown deadline.
fn write_server_fixture(dir: &Path, port: u16, body_marker: &str) -> PathBuf {
    let src = format!(
        r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "{marker}")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 5000, 4)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
"#,
        marker = body_marker,
        port = port,
    );
    let path = dir.join("main.td");
    std::fs::write(&path, src).expect("write main.td");
    path
}

/// Build the fixture as a native binary and return the path.
fn build_native(src: &Path, bin: &Path) -> bool {
    let out = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(src)
        .arg("-o")
        .arg(bin)
        .output()
        .expect("spawn taida build --target native");
    if !out.status.success() {
        eprintln!(
            "native build failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        return false;
    }
    true
}

/// Spawn the binary and wait until the server accepts a TCP connection.
/// Returns the child handle for later cleanup.
fn spawn_and_wait_ready(bin: &Path, port: u16) -> Child {
    let child = Command::new(bin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn native binary");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return child;
        }
        if Instant::now() >= deadline {
            return child;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Send `request_bytes` to the server and read the whole reply (until EOF
/// or timeout).
fn round_trip(port: u16, request_bytes: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .expect("connect to native server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set write timeout");
    stream.write_all(request_bytes).expect("write request");
    let mut reply = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    reply
}

fn drain_and_cleanup(mut child: Child, dir: &Path) -> String {
    let _ = child.kill();
    let out = child.wait_with_output().unwrap_or_else(|_| std::process::Output {
        status: std::process::ExitStatus::default(),
        stdout: Vec::new(),
        stderr: Vec::new(),
    });
    let _ = std::fs::remove_dir_all(dir);
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ════════════════════════════════════════════════════════════════════
// Over-limit method: 17 bytes > 16 → must reject with 400
// ════════════════════════════════════════════════════════════════════

#[test]
fn c26b_022_native_oversized_method_rejects_400() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native C26B-022 method test");
        return;
    }
    let dir = tempdir("oversized_method");
    let port = find_free_loopback_port();
    let src = write_server_fixture(&dir, port, "should-never-reach-handler");
    let bin = dir.join("out.bin");
    if !build_native(&src, &bin) {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("native build failed for oversized_method fixture");
    }
    let child = spawn_and_wait_ready(&bin, port);

    // 17-byte method: "VERYLONGMETHODXYZ"
    let method = "VERYLONGMETHODXYZ";
    assert!(method.len() > 16, "fixture setup");
    let req = format!(
        "{} / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        method
    );
    let reply = round_trip(port, req.as_bytes());
    let reply_str = String::from_utf8_lossy(&reply).to_string();

    drain_and_cleanup(child, &dir);

    assert!(
        reply_str.starts_with("HTTP/1.1 400 "),
        "C26B-022 native method: expected '400 Bad Request' for 17-byte method, got: {:?}",
        reply_str
    );
    assert!(
        !reply_str.contains("should-never-reach-handler"),
        "C26B-022 native method: handler must NOT run for oversized method, reply: {:?}",
        reply_str
    );
}

// ════════════════════════════════════════════════════════════════════
// At-limit method: exactly 16 bytes → must accept with 200
// ════════════════════════════════════════════════════════════════════

#[test]
fn c26b_022_native_at_limit_method_accepts() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native C26B-022 at-limit method test");
        return;
    }
    let dir = tempdir("at_limit_method");
    let port = find_free_loopback_port();
    let src = write_server_fixture(&dir, port, "native-method-at-limit-ok");
    let bin = dir.join("out.bin");
    if !build_native(&src, &bin) {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("native build failed for at_limit_method fixture");
    }
    let child = spawn_and_wait_ready(&bin, port);

    // Exactly 16 bytes.
    let method = "OPTIONSOPTIONSXX";
    assert_eq!(method.len(), 16);
    let req = format!(
        "{} / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        method
    );
    let reply = round_trip(port, req.as_bytes());
    let reply_str = String::from_utf8_lossy(&reply).to_string();

    drain_and_cleanup(child, &dir);

    assert!(
        reply_str.starts_with("HTTP/1.1 200 "),
        "C26B-022 native method: expected '200 OK' for 16-byte method, got: {:?}",
        reply_str
    );
    assert!(
        reply_str.contains("native-method-at-limit-ok"),
        "C26B-022 native method: handler body must appear for at-limit method, reply: {:?}",
        reply_str
    );
}

// ════════════════════════════════════════════════════════════════════
// Over-limit path: 2049 bytes > 2048 → must reject with 400
// ════════════════════════════════════════════════════════════════════

#[test]
fn c26b_022_native_oversized_path_rejects_400() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native C26B-022 path test");
        return;
    }
    let dir = tempdir("oversized_path");
    let port = find_free_loopback_port();
    let src = write_server_fixture(&dir, port, "should-never-reach-handler");
    let bin = dir.join("out.bin");
    if !build_native(&src, &bin) {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("native build failed for oversized_path fixture");
    }
    let child = spawn_and_wait_ready(&bin, port);

    // 2049-byte path: '/' + 2048 'a'
    let mut path = String::from("/");
    path.push_str(&"a".repeat(2048));
    assert!(path.len() > 2048);
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        path
    );
    let reply = round_trip(port, req.as_bytes());
    let reply_str = String::from_utf8_lossy(&reply).to_string();

    drain_and_cleanup(child, &dir);

    assert!(
        reply_str.starts_with("HTTP/1.1 400 "),
        "C26B-022 native path: expected '400 Bad Request' for 2049-byte path, got: {:?}",
        reply_str
    );
    assert!(
        !reply_str.contains("should-never-reach-handler"),
        "C26B-022 native path: handler must NOT run for oversized path, reply: {:?}",
        reply_str
    );
}

// ════════════════════════════════════════════════════════════════════
// Over-limit Host header value: 257 bytes > 256 → must reject with 400
// ════════════════════════════════════════════════════════════════════

#[test]
fn c26b_022_native_oversized_host_rejects_400() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native C26B-022 host test");
        return;
    }
    let dir = tempdir("oversized_host");
    let port = find_free_loopback_port();
    let src = write_server_fixture(&dir, port, "should-never-reach-handler");
    let bin = dir.join("out.bin");
    if !build_native(&src, &bin) {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("native build failed for oversized_host fixture");
    }
    let child = spawn_and_wait_ready(&bin, port);

    let host = "a".repeat(257);
    assert!(host.len() > 256);
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host
    );
    let reply = round_trip(port, req.as_bytes());
    let reply_str = String::from_utf8_lossy(&reply).to_string();

    drain_and_cleanup(child, &dir);

    assert!(
        reply_str.starts_with("HTTP/1.1 400 "),
        "C26B-022 native host: expected '400 Bad Request' for 257-byte Host, got: {:?}",
        reply_str
    );
    assert!(
        !reply_str.contains("should-never-reach-handler"),
        "C26B-022 native host: handler must NOT run for oversized Host, reply: {:?}",
        reply_str
    );
}

// ════════════════════════════════════════════════════════════════════
// At-limit Host header value: exactly 256 bytes → must accept with 200
// ════════════════════════════════════════════════════════════════════

#[test]
fn c26b_022_native_at_limit_host_accepts() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native C26B-022 at-limit host test");
        return;
    }
    let dir = tempdir("at_limit_host");
    let port = find_free_loopback_port();
    let src = write_server_fixture(&dir, port, "native-host-at-limit-ok");
    let bin = dir.join("out.bin");
    if !build_native(&src, &bin) {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("native build failed for at_limit_host fixture");
    }
    let child = spawn_and_wait_ready(&bin, port);

    let host = "a".repeat(256);
    assert_eq!(host.len(), 256);
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host
    );
    let reply = round_trip(port, req.as_bytes());
    let reply_str = String::from_utf8_lossy(&reply).to_string();

    drain_and_cleanup(child, &dir);

    assert!(
        reply_str.starts_with("HTTP/1.1 200 "),
        "C26B-022 native host: expected '200 OK' for 256-byte Host, got: {:?}",
        reply_str
    );
    assert!(
        reply_str.contains("native-host-at-limit-ok"),
        "C26B-022 native host: handler body must appear for at-limit Host, reply: {:?}",
        reply_str
    );
}
