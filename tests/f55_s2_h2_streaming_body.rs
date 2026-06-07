//! F55 S2 — HTTP/2 (and HTTP/3) request-body streaming for 2-arg
//! `httpServe` handlers (interpreter backend).
//!
//! # Background
//!
//! Before F55 S2 the H2 / H3 serve paths dispatched every request through
//! the **1-arg** contract (eager `req.body` completed value, 16 MiB cap,
//! END_STREAM-then-dispatch). A 2-arg `handler req writer` was silently
//! treated as 1-arg: `req.body` was empty and `readBody*` had no supply
//! source, diverging from the H1 streaming-observation contract that
//! `readBody` / `readBodyChunk` / `readBodyAll` already honour
//! (`tests/c26b_023_two_arg_handler_body.rs`, `tests/c27b_027_read_body_2arg.rs`).
//!
//! The streaming revision ("eager fill, streaming observation") activates
//! the same observation contract under H2 / H3: at HEADERS+END_HEADERS the
//! handler is dispatched with `req.body.len = 0`, and `readBody*` pulls
//! the body bytes that were already accumulated off the wire (DATA frames,
//! still capped at 16 MiB) from a pre-loaded per-stream queue.
//!
//! # Scope of this file
//!
//! * **H2 end-to-end** via `curl --http2 --insecure` against the
//!   interpreter binary:
//!   1. `readBodyAll` echoes the full POST body (Content-Length present).
//!   2. `readBodyChunk` called repeatedly concatenates to the full body.
//!   3. A 2-arg handler that never reads the body still leaves the
//!      connection healthy enough to serve the next request (2-request
//!      bounded server).
//!   4. **1-arg invariance regression**: a 1-arg H2 handler still sees the
//!      eager `req.contentLength` / `req.body` (the D29B-001 arena shape is
//!      untouched).
//! * **H3 source-pin**: the H3 serve path has no off-the-shelf wire client
//!   in CI (curl lacks HTTP/3 here, and a bespoke quinn H3 client would be
//!   brittle), so — mirroring the `tests/d29b_011_h3_headers_span_parity.rs`
//!   idiom for the analogous arena work — the 2-arg streaming branch and its
//!   transport-less supply are pinned by source inspection. The body-reading
//!   code itself is shared verbatim with H1/H2 (`net_eval/stream.rs`), which
//!   the H2 end-to-end cases above exercise on real bytes.
//!
//! # Acceptance
//!
//! `cargo test --release --test f55_s2_h2_streaming_body` GREEN (H2 cases
//! skip cleanly when `cc` / `openssl` / `curl --http2` are unavailable; the
//! H3 source-pin always runs).

mod common;

use common::{find_free_loopback_port, normalize, taida_bin};
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

// ── Environment capability probes ───────────────────────────────────

fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn curl_h2_available() -> bool {
    if !curl_available() {
        return false;
    }
    match Command::new("curl").arg("--version").output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains("HTTP2"),
        Err(_) => false,
    }
}

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Test project + cert scaffolding ─────────────────────────────────

fn unique_dir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("taida_f55_s2_{}_{}_{}", label, pid, nanos));
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

/// Write a net project whose `taida-lang/net` resolves to the workspace
/// bundle (same shape as `tests/parity.rs::setup_net_project`).
fn write_net_project(dir: &Path, source: &str) {
    fs::write(dir.join("main.td"), source).expect("write main.td");
    fs::write(
        dir.join("packages.tdm"),
        "// Minimal packages.tdm for f55_s2 net test project\n",
    )
    .expect("write packages.tdm");
    let deps_net = dir
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("net");
    fs::create_dir_all(&deps_net).expect("create deps/taida-lang/net");
    let net_stub = r#"// taida-lang/net — Core bundled network package
Enum => HttpProtocol = :H1 :H2 :H3

<<< @(httpServe, httpParseRequestHead, httpEncodeResponse, readBody, startResponse, writeChunk, endResponse, sseEvent, readBodyChunk, readBodyAll, wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode, HttpProtocol)
"#;
    fs::write(deps_net.join("main.td"), net_stub).expect("write net stub");
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Generate a self-signed cert+key pair (mirrors parity.rs gen_self_signed_cert).
fn gen_self_signed_cert(cert_path: &Path, key_path: &Path) -> bool {
    Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=127.0.0.1",
            "-keyout",
            key_path.to_str().unwrap_or(""),
            "-out",
            cert_path.to_str().unwrap_or(""),
            "-days",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawn the interpreter against `source`, wait until the TCP port is
/// accepting, then return the live child + the project dir for cleanup.
fn spawn_interp_h2_server(source: &str, port: u16, label: &str) -> Option<(Child, PathBuf)> {
    let dir = unique_dir(label);
    write_net_project(&dir, source);
    let td_path = dir.join("main.td");
    let child = Command::new(taida_bin())
        .arg(&td_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn interpreter h2 server");

    let mut ready = false;
    for _ in 0..60 {
        thread::sleep(Duration::from_millis(100));
        if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            ready = true;
            break;
        }
    }
    if !ready {
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        cleanup(&dir);
        return None;
    }
    Some((child, dir))
}

/// Check whether `cc` is available (needed for native compilation).
fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Compile `source` to a native binary, spawn it, wait until the TCP port is
/// accepting, then return the live child + project dir + binary path for
/// cleanup. Mirrors `spawn_interp_h2_server` but goes through
/// `taida build native` first (the native backend leg of the F55 S2 work).
fn spawn_native_h2_server(
    source: &str,
    port: u16,
    label: &str,
) -> Option<(Child, PathBuf, PathBuf)> {
    let dir = unique_dir(label);
    write_net_project(&dir, source);
    let td_path = dir.join("main.td");
    let bin_path = dir.join(format!("{}_bin", label));

    let compile = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("invoke native build");
    if !compile.status.success() {
        eprintln!(
            "f55_s2/{}: native compile failed: {}",
            label,
            String::from_utf8_lossy(&compile.stderr)
        );
        cleanup(&dir);
        return None;
    }

    let child = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn native h2 server");

    let mut ready = false;
    for _ in 0..60 {
        thread::sleep(Duration::from_millis(100));
        if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            ready = true;
            break;
        }
    }
    if !ready {
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        cleanup(&dir);
        return None;
    }
    Some((child, dir, bin_path))
}

/// Run a single `curl --http2` request with an optional POST body. Returns
/// the response body printed to stdout (curl `-o -`), plus the version:code
/// status written by `-w`.
fn curl_h2_post(port: u16, path: &str, body: Option<&str>) -> (String, String) {
    let url = format!("https://127.0.0.1:{}{}", port, path);
    let mut args: Vec<String> = vec![
        "--http2".into(),
        "--insecure".into(),
        "--silent".into(),
        "--max-time".into(),
        "8".into(),
        "-w".into(),
        "\n%{http_version}:%{http_code}".into(),
    ];
    if let Some(b) = body {
        args.push("--data-binary".into());
        args.push(b.into());
    }
    args.push(url);

    let out = Command::new("curl")
        .args(&args)
        .output()
        .expect("run curl --http2");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // The last line is the `-w` status; everything before is the body.
    match stdout.rsplit_once('\n') {
        Some((body_part, status)) => (body_part.to_string(), status.to_string()),
        None => (String::new(), stdout),
    }
}

fn h2_prereqs_ok(case: &str) -> bool {
    if !openssl_available() {
        eprintln!("SKIP f55_s2/{}: openssl not available", case);
        return false;
    }
    if !curl_h2_available() {
        eprintln!("SKIP f55_s2/{}: curl --http2 not available", case);
        return false;
    }
    true
}

/// Native legs additionally require `cc` for the `taida build native` step.
fn native_h2_prereqs_ok(case: &str) -> bool {
    if !cc_available() {
        eprintln!("SKIP f55_s2/{}: cc not available", case);
        return false;
    }
    h2_prereqs_ok(case)
}

// ── H2 end-to-end: readBodyAll echoes the full body ─────────────────

#[test]
fn f55_s2_h2_2arg_read_body_all_echoes_body() {
    if !h2_prereqs_ok("read_body_all") {
        return;
    }
    let cert = unique_dir("rba_cert").join("cert.pem");
    let key = unique_dir("rba_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/read_body_all: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    // 2-arg handler: read the entire body via readBodyAll, return it as a
    // one-shot response pack (H2 response framing is built by send_h2_response,
    // not the chunked writer API).
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, readBodyAll, HttpProtocol)

handler req: Request writer: Writer =
  bodyBytes <= readBodyAll(req)
  decoded <= Utf8Decode[bodyBytes]()
  decoded >=> bodyText
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= bodyText)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir)) = spawn_interp_h2_server(&source, port, "rba") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/read_body_all: interp h2 server did not bind on port {}",
            port
        );
    };

    let payload = "hello-streaming-h2-body";
    let (resp_body, status) = curl_h2_post(port, "/echo", Some(payload));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status.starts_with('2'),
        "f55_s2/read_body_all: expected HTTP/2 2xx, got status {:?} body {:?}",
        status,
        resp_body
    );
    assert!(
        resp_body.contains(payload),
        "f55_s2/read_body_all: expected echoed body to contain {:?}, got {:?}",
        payload,
        resp_body
    );
    assert_eq!(
        server_stdout, "1",
        "f55_s2/read_body_all: expected server to process exactly 1 request, got {:?}",
        server_stdout
    );
}

// ── H2 end-to-end: readBodyChunk loop concatenates the full body ────

#[test]
fn f55_s2_h2_2arg_read_body_chunk_concatenates() {
    if !h2_prereqs_ok("read_body_chunk") {
        return;
    }
    let cert = unique_dir("rbc_cert").join("cert.pem");
    let key = unique_dir("rbc_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/read_body_chunk: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    // 2-arg handler: drain the body via repeated readBodyChunk calls,
    // tail-recursively concatenating each unwrapped Lax[Bytes] chunk until
    // one comes back empty (has_value = false), then echo the reassembled
    // bytes. readBodyChunk reads at most 8 KiB per call, so a >8 KiB body
    // forces several non-empty pops off the per-stream queue.
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, readBodyChunk, HttpProtocol)

drain req: Request acc: Bytes =
  chunk <= readBodyChunk(req)
  chunk >=> chunkV
  | chunk.has_value |> drain(req, Concat[acc, chunkV]())
  | _ |> acc
=> :Bytes

handler req: Request writer: Writer =
  empty <= Bytes[""]()
  empty >=> emptyB
  collected <= drain(req, emptyB)
  decoded <= Utf8Decode[collected]()
  decoded >=> bodyText
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= bodyText)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir)) = spawn_interp_h2_server(&source, port, "rbc") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/read_body_chunk: interp h2 server did not bind on port {}",
            port
        );
    };

    // A body well over readBodyChunk's 8 KiB read unit so the handler makes
    // several non-empty pops before the empty terminator; the H2 client is
    // also free to split it across multiple DATA frames. The handler must
    // reassemble whatever granularity it receives (the design guarantees no
    // chunk-boundary contract).
    let payload = "ABCD".repeat(5000); // 20000 bytes
    let (resp_body, status) = curl_h2_post(port, "/chunked", Some(&payload));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status.starts_with('2'),
        "f55_s2/read_body_chunk: expected HTTP/2 2xx, got status {:?}",
        status
    );
    assert!(
        resp_body.contains(&payload),
        "f55_s2/read_body_chunk: reassembled body ({} bytes expected) did not match; got {} bytes",
        payload.len(),
        resp_body.len()
    );
    assert_eq!(
        server_stdout, "1",
        "f55_s2/read_body_chunk: expected exactly 1 request, got {:?}",
        server_stdout
    );
}

// ── H2 end-to-end: body unread → next request still healthy ─────────

#[test]
fn f55_s2_h2_2arg_body_unread_next_request_healthy() {
    if !h2_prereqs_ok("body_unread") {
        return;
    }
    let cert = unique_dir("unread_cert").join("cert.pem");
    let key = unique_dir("unread_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/body_unread: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    // 2-arg handler that deliberately never reads the body. The design
    // requires the connection / server to stay healthy: the leftover body
    // bytes were already drained off the wire by the frame loop, so the next
    // request must still be served cleanly. A bounded server of 2 requests
    // proves the second dispatch happens.
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, HttpProtocol)

handler req: Request writer: Writer =
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= "ignored-body")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir)) = spawn_interp_h2_server(&source, port, "unread") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/body_unread: interp h2 server did not bind on port {}",
            port
        );
    };

    // First request: POST a body the handler ignores.
    let (resp1, status1) = curl_h2_post(port, "/first", Some("first-body-ignored"));
    // Second request: a fresh connection (curl exits between calls). The
    // server's request budget is 2, so it must accept and dispatch this too.
    let (resp2, status2) = curl_h2_post(port, "/second", Some("second-body-ignored"));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status1.starts_with('2'),
        "f55_s2/body_unread: first request expected 2xx, got {:?} ({:?})",
        status1,
        resp1
    );
    assert!(
        status2.starts_with('2'),
        "f55_s2/body_unread: second request expected 2xx (connection healthy after unread body), got {:?} ({:?})",
        status2,
        resp2
    );
    assert_eq!(
        server_stdout, "2",
        "f55_s2/body_unread: expected server to process 2 requests, got {:?}",
        server_stdout
    );
}

// ── H2 1-arg invariance regression ──────────────────────────────────

#[test]
fn f55_s2_h2_1arg_handler_body_unchanged() {
    if !h2_prereqs_ok("1arg_invariant") {
        return;
    }
    let cert = unique_dir("onearg_cert").join("cert.pem");
    let key = unique_dir("onearg_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/1arg_invariant: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    // 1-arg handler: must still observe the eager body. We echo back the
    // declared contentLength so the assertion proves the 1-arg path saw the
    // full body (the D29B-001 arena / req.body span is unchanged by S2).
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, HttpProtocol)

handler req: Request =
  clText <= req.contentLength.toString()
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= clText)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir)) = spawn_interp_h2_server(&source, port, "onearg") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/1arg_invariant: interp h2 server did not bind on port {}",
            port
        );
    };

    let payload = "0123456789"; // 10 bytes
    let (resp_body, status) = curl_h2_post(port, "/onearg", Some(payload));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status.starts_with('2'),
        "f55_s2/1arg_invariant: expected HTTP/2 2xx, got {:?}",
        status
    );
    assert!(
        resp_body.contains(&payload.len().to_string()),
        "f55_s2/1arg_invariant: 1-arg handler must see eager contentLength={} (D29B-001 invariant), got body {:?}",
        payload.len(),
        resp_body
    );
    assert_eq!(
        server_stdout, "1",
        "f55_s2/1arg_invariant: expected exactly 1 request, got {:?}",
        server_stdout
    );
}

// ════════════════════════════════════════════════════════════════════
//  Native backend leg (F55 S2 step 4)
// ════════════════════════════════════════════════════════════════════
//
// Each native case compiles the same handler source the interpreter cases
// use (so the observation contract is byte-identical) through
// `taida build native`, spawns the binary, and drives it with the same
// `curl --http2` requests. The native readBody* machinery (net_h1_h2.c
// Net4BodyState) is shared verbatim with H1, and the H2 2-arg branch
// pre-loads the already-accumulated DATA-frame body into that state's
// `leftover` supply (eager fill, streaming observation), so the bytes
// the handler observes match the interpreter / wire exactly.

// ── Native H2 end-to-end: readBodyAll echoes the full body ──────────

#[test]
fn f55_s2_native_h2_2arg_read_body_all_echoes_body() {
    if !native_h2_prereqs_ok("native_read_body_all") {
        return;
    }
    let cert = unique_dir("nrba_cert").join("cert.pem");
    let key = unique_dir("nrba_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/native_read_body_all: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, readBodyAll, HttpProtocol)

handler req: Request writer: Writer =
  bodyBytes <= readBodyAll(req)
  decoded <= Utf8Decode[bodyBytes]()
  decoded >=> bodyText
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= bodyText)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir, _bin)) = spawn_native_h2_server(&source, port, "nrba") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/native_read_body_all: native h2 server did not bind on port {}",
            port
        );
    };

    let payload = "hello-streaming-h2-body-native";
    let (resp_body, status) = curl_h2_post(port, "/echo", Some(payload));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status.starts_with('2'),
        "f55_s2/native_read_body_all: expected HTTP/2 2xx, got status {:?} body {:?}",
        status,
        resp_body
    );
    assert!(
        resp_body.contains(payload),
        "f55_s2/native_read_body_all: expected echoed body to contain {:?}, got {:?}",
        payload,
        resp_body
    );
    assert_eq!(
        server_stdout, "1",
        "f55_s2/native_read_body_all: expected server to process exactly 1 request, got {:?}",
        server_stdout
    );
}

// ── Native H2 end-to-end: readBodyChunk loop concatenates the body ──

#[test]
fn f55_s2_native_h2_2arg_read_body_chunk_concatenates() {
    if !native_h2_prereqs_ok("native_read_body_chunk") {
        return;
    }
    let cert = unique_dir("nrbc_cert").join("cert.pem");
    let key = unique_dir("nrbc_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/native_read_body_chunk: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, readBodyChunk, HttpProtocol)

drain req: Request acc: Bytes =
  chunk <= readBodyChunk(req)
  chunk >=> chunkV
  | chunk.has_value |> drain(req, Concat[acc, chunkV]())
  | _ |> acc
=> :Bytes

handler req: Request writer: Writer =
  empty <= Bytes[""]()
  empty >=> emptyB
  collected <= drain(req, emptyB)
  decoded <= Utf8Decode[collected]()
  decoded >=> bodyText
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= bodyText)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir, _bin)) = spawn_native_h2_server(&source, port, "nrbc") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/native_read_body_chunk: native h2 server did not bind on port {}",
            port
        );
    };

    let payload = "ABCD".repeat(5000); // 20000 bytes — forces multiple pops
    let (resp_body, status) = curl_h2_post(port, "/chunked", Some(&payload));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status.starts_with('2'),
        "f55_s2/native_read_body_chunk: expected HTTP/2 2xx, got status {:?}",
        status
    );
    assert!(
        resp_body.contains(&payload),
        "f55_s2/native_read_body_chunk: reassembled body ({} bytes expected) did not match; got {} bytes",
        payload.len(),
        resp_body.len()
    );
    assert_eq!(
        server_stdout, "1",
        "f55_s2/native_read_body_chunk: expected exactly 1 request, got {:?}",
        server_stdout
    );
}

// ── Native H2 end-to-end: body unread → next request still healthy ──

#[test]
fn f55_s2_native_h2_2arg_body_unread_next_request_healthy() {
    if !native_h2_prereqs_ok("native_body_unread") {
        return;
    }
    let cert = unique_dir("nunread_cert").join("cert.pem");
    let key = unique_dir("nunread_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/native_body_unread: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, HttpProtocol)

handler req: Request writer: Writer =
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= "ignored-body")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir, _bin)) = spawn_native_h2_server(&source, port, "nunread") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/native_body_unread: native h2 server did not bind on port {}",
            port
        );
    };

    let (resp1, status1) = curl_h2_post(port, "/first", Some("first-body-ignored"));
    let (resp2, status2) = curl_h2_post(port, "/second", Some("second-body-ignored"));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status1.starts_with('2'),
        "f55_s2/native_body_unread: first request expected 2xx, got {:?} ({:?})",
        status1,
        resp1
    );
    assert!(
        status2.starts_with('2'),
        "f55_s2/native_body_unread: second request expected 2xx (connection healthy after unread body), got {:?} ({:?})",
        status2,
        resp2
    );
    assert_eq!(
        server_stdout, "2",
        "f55_s2/native_body_unread: expected server to process 2 requests, got {:?}",
        server_stdout
    );
}

// ── Native H2 1-arg invariance regression ───────────────────────────

#[test]
fn f55_s2_native_h2_1arg_handler_body_unchanged() {
    if !native_h2_prereqs_ok("native_1arg_invariant") {
        return;
    }
    let cert = unique_dir("nonearg_cert").join("cert.pem");
    let key = unique_dir("nonearg_key").join("key.pem");
    if !gen_self_signed_cert(&cert, &key) {
        eprintln!("SKIP f55_s2/native_1arg_invariant: cert gen failed");
        return;
    }

    let port = find_free_loopback_port();
    let source = format!(
        r#">>> taida-lang/net => @(httpServe, HttpProtocol)

handler req: Request =
  clText <= req.contentLength.toString()
  @(status <= 200, headers <= @[@(name <= "Content-Type", value <= "text/plain")], body <= clText)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 10000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2()))
asyncResult >=> result
result >=> r
stdout(r.requests)
"#,
        port = port,
        cert = cert.display(),
        key = key.display(),
    );

    let Some((child, dir, _bin)) = spawn_native_h2_server(&source, port, "nonearg") else {
        let _ = fs::remove_file(&cert);
        let _ = fs::remove_file(&key);
        panic!(
            "f55_s2/native_1arg_invariant: native h2 server did not bind on port {}",
            port
        );
    };

    let payload = "0123456789"; // 10 bytes
    let (resp_body, status) = curl_h2_post(port, "/onearg", Some(payload));

    let output = child.wait_with_output().expect("wait for server");
    let server_stdout = normalize(&String::from_utf8_lossy(&output.stdout));

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
    cleanup(&dir);

    assert!(
        status.starts_with('2'),
        "f55_s2/native_1arg_invariant: expected HTTP/2 2xx, got {:?}",
        status
    );
    assert!(
        resp_body.contains(&payload.len().to_string()),
        "f55_s2/native_1arg_invariant: 1-arg handler must see eager contentLength={} (D29B-001 invariant), got body {:?}",
        payload.len(),
        resp_body
    );
    assert_eq!(
        server_stdout, "1",
        "f55_s2/native_1arg_invariant: expected exactly 1 request, got {:?}",
        server_stdout
    );
}

// ── H3 source-pin: 2-arg streaming branch exists ────────────────────
//
// No HTTP/3 wire client is available in CI (curl lacks HTTP3 here, and a
// bespoke quinn H3 client driving the spawned binary would be brittle). The
// H3 body-reading code is shared verbatim with H1/H2 (net_eval/stream.rs),
// which the H2 end-to-end cases above exercise on real bytes. Following the
// `tests/d29b_011_h3_headers_span_parity.rs` idiom, we pin the H3 2-arg
// streaming branch by source inspection so a future revert that drops it
// (silently re-treating 2-arg H3 handlers as 1-arg) flips this assertion.

fn read_interp_h3_source() -> String {
    std::fs::read_to_string("src/interpreter/net/eval/h3.rs")
        .expect("read src/interpreter/net/eval/h3.rs")
}

#[test]
fn f55_s2_interp_h3_serves_2arg_streaming_branch() {
    let src = read_interp_h3_source();

    assert!(
        src.contains("F55 S2"),
        "Interpreter h3 must keep the F55 S2 streaming-body branch banner; \
         a revert would drop it along with the 2-arg dispatch."
    );
    // Arity branch on the handler.
    assert!(
        src.contains("handler.params.len() >= 2"),
        "Interpreter h3 must branch on handler arity (>= 2 = streaming body \
         observation contract, identical to H1/H2)."
    );
    // 2-arg pack uses an empty body span (body not surfaced eagerly).
    assert!(
        src.contains("// body span is empty (H1/H2 2-arg parity).")
            && src.contains("request_fields.push((\"body\".into(), make_span(0, 0)));"),
        "Interpreter h3 2-arg path must publish an empty body span so the \
         handler reads via readBody* (matching H1/H2)."
    );
    // The body is pre-loaded into a RequestBodyState (option (b) supply).
    assert!(
        src.contains("RequestBodyState::new(false, body_len as i64, true, req.body.clone())"),
        "Interpreter h3 2-arg path must pre-load the full body into a \
         fixed-length RequestBodyState (option (b) supply source)."
    );
    // The streaming writer is installed against a transport-less stream.
    assert!(
        src.contains("ConnStream::Detached"),
        "Interpreter h3 2-arg path must install the ActiveStreamingWriter \
         against ConnStream::Detached (no socket; body fully buffered)."
    );
    // The __body_stream sentinel + matching token wire readBody* identity.
    assert!(
        src.contains("__body_stream") && src.contains("__body_token"),
        "Interpreter h3 2-arg request pack must carry the __body_stream \
         sentinel and a __body_token matching the RequestBodyState token so \
         readBody* accepts the pack."
    );
}

#[test]
fn f55_s2_interp_h2_serves_2arg_streaming_branch() {
    let src = std::fs::read_to_string("src/interpreter/net/eval/h2.rs")
        .expect("read src/interpreter/net/eval/h2.rs");

    assert!(
        src.contains("F55 S2"),
        "Interpreter h2 must keep the F55 S2 streaming-body branch banner."
    );
    assert!(
        src.contains("let handler_arity = handler.params.len();"),
        "Interpreter h2 must branch on handler arity for the streaming body path."
    );
    assert!(
        src.contains("build_h2_streaming_request_pack"),
        "Interpreter h2 must build a dedicated streaming request pack for 2-arg handlers."
    );
    assert!(
        src.contains("RequestBodyState::new(false, body_len as i64, true, body.clone())"),
        "Interpreter h2 2-arg path must pre-load the already-capped body into \
         a fixed-length RequestBodyState (option (b) supply source; the 16 MiB \
         cap is enforced during DATA accumulation in net_h2.rs)."
    );
    // The 1-arg path must remain intact (eager body span at offset 0).
    assert!(
        src.contains("request_fields.push((\"body\".into(), make_span(0, body_len)));"),
        "Interpreter h2 1-arg path must keep the eager body span (D29B-001 \
         arena shape) unchanged."
    );
}

// ── Native source-pin: 2-arg streaming branch exists (H2 + H3) ──────
//
// The native H2 cases above only run when `cc` + `openssl` + `curl --http2`
// are all present, and the native H3 path has no wire client at all (the
// transport is QUIC/quiche). These source-pins guard the native C 2-arg
// branch so a revert that silently re-treats 2-arg H2/H3 handlers as 1-arg
// flips an assertion even in a minimal CI environment. The native readBody*
// machinery (Net4BodyState) is shared verbatim between H1/H2/H3.

#[test]
fn f55_s2_native_h2_serves_2arg_streaming_branch() {
    let src = std::fs::read_to_string("src/codegen/native_runtime/net_h1_h2.c")
        .expect("read src/codegen/native_runtime/net_h1_h2.c");

    assert!(
        src.contains("F55 S2"),
        "Native net_h1_h2.c must keep the F55 S2 streaming-body branch banner."
    );
    // Arity branch on the handler inside the H2 dispatch path.
    assert!(
        src.contains("if (ctx->handler_arity >= 2)"),
        "Native h2 must branch on handler arity for the streaming body path."
    );
    // 2-arg dispatch goes through callback2 (request + writer).
    assert!(
        src.contains("taida_invoke_callback2(ctx->handler, req_pack, writer_token)"),
        "Native h2 2-arg path must dispatch via taida_invoke_callback2 \
         (request pack + writer token)."
    );
    // Option (b) supply: the already-accumulated body is pre-loaded into the
    // Net4BodyState leftover buffer (the shared H1 readBody* supply source).
    assert!(
        src.contains("net_h2_v4_body_supply") && src.contains("body_state.leftover = supply;"),
        "Native h2 2-arg path must pre-load the already-capped body into the \
         Net4BodyState leftover supply (option (b); the 16 MiB cap is enforced \
         during DATA accumulation)."
    );
    // The streaming request pack carries the __body_stream sentinel + token so
    // readBody* accepts it; 1-arg packs must not.
    assert!(
        src.contains("if (streaming) {")
            && src.contains("SET_FIELD(\"__body_stream\", (taida_val)\"__v4_body_stream\","),
        "Native h2 streaming request pack must carry __body_stream + \
         __body_token only on the 2-arg path."
    );
    // The 1-arg eager body span must remain intact (D29B-001 arena shape).
    assert!(
        src.contains("streaming ? (taida_val)0 : (taida_val)body_len"),
        "Native h2 1-arg path must keep the eager body span (only the 2-arg \
         path empties it)."
    );
}

#[test]
fn f55_s2_native_h3_serves_2arg_streaming_branch() {
    let src = std::fs::read_to_string("src/codegen/native_runtime/net_h3_quic.c")
        .expect("read src/codegen/native_runtime/net_h3_quic.c");

    assert!(
        src.contains("F55 S2"),
        "Native net_h3_quic.c must keep the F55 S2 streaming-body branch banner."
    );
    // Arity branch on the handler inside the H3 dispatch path.
    assert!(
        src.contains("int h3_streaming = (pool->handler_arity >= 2) ? 1 : 0;"),
        "Native h3 must branch on handler arity for the streaming body path."
    );
    // 2-arg dispatch goes through callback2 (request + writer).
    assert!(
        src.contains("taida_invoke_callback2(pool->handler, request_pack, writer_token)"),
        "Native h3 2-arg path must dispatch via taida_invoke_callback2."
    );
    // Option (b) supply via the shared Net4BodyState leftover buffer.
    assert!(
        src.contains("net_h3_v4_body_supply") && src.contains("body_state.leftover = supply;"),
        "Native h3 2-arg path must pre-load the collected body into the \
         Net4BodyState leftover supply (option (b))."
    );
    // The streaming request pack carries __body_stream + __body_token.
    assert!(
        src.contains("SET_FIELD_H3(\"__body_stream\", (taida_val)\"__v4_body_stream\","),
        "Native h3 streaming request pack must carry __body_stream + \
         __body_token on the 2-arg path."
    );
}
