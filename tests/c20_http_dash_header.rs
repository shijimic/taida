//! C20-4 (C19B-007): 3-backend parity harness for the list-of-record
//! `HttpRequest` headers shape.
//!
//! Taida identifiers forbid `-`, so a buchi-pack header value like
//! `@(x-api-key <= "k")` is a parse error. The new shape
//! `@[@(name <= "x-api-key", value <= "k")]` unlocks arbitrary UTF-8
//! header names (which is what Anthropic's / OpenAI's / httpbin APIs
//! actually expect). The legacy shape keeps working.
//!
//! Parity contract: for the same Taida source, all three backends must
//! emit the same `-H 'Name: Value'` pairs to the wire — verified via a
//! TCP loopback echo server that captures the raw request.
//!
//! Plus:
//!   * `HttpRequest[method]()` with fewer than 2 type args must be a
//!     hard failure on **all** backends (ROOT-16 — JS used to emit
//!     syntactically invalid JS instead).

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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

/// Spawn a one-shot HTTP echo server on 127.0.0.1:0, return the port
/// plus a receiver that yields the raw request bytes once captured.
/// Copied-and-simplified from `tests/parity.rs::spawn_http_echo_server`.
fn spawn_http_echo_server() -> (u16, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind http loopback");
    listener.set_nonblocking(false).expect("set blocking");
    let port = listener.local_addr().expect("local addr").port();
    let (tx, rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        // Accept only the first client, read one request worth of
        // bytes (up to 16 KiB), then respond 201 and close.
        let (mut socket, _) = listener.accept().expect("accept http");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match socket.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    req.extend_from_slice(&buf[..n]);
                    // Stop once we've seen the header terminator and
                    // any declared body (best-effort; this test drives
                    // small GET / POST payloads).
                    if req.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let req_text = String::from_utf8_lossy(&req).to_string();
        let _ = tx.send(req_text);

        let body = "ok";
        let resp = format!(
            "HTTP/1.1 201 Created\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(resp.as_bytes());
    });

    (port, rx, handle)
}

fn write_source(label: &str, source: &str) -> PathBuf {
    let path = unique_temp(&format!("c20_http_{}", label), "td");
    fs::write(&path, source).expect("write td");
    path
}

fn run_interp(source: &str, label: &str) -> String {
    let src = write_source(label, source);
    let out = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("taida interp");
    let _ = fs::remove_file(&src);
    assert!(
        out.status.success(),
        "interpreter failed for {}: stderr={}",
        label,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn run_js(source: &str, label: &str) -> String {
    let src = write_source(label, source);
    let mjs = unique_temp(&format!("c20_http_{}", label), "mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&mjs)
        .output()
        .expect("taida js build");
    let _ = fs::remove_file(&src);
    assert!(
        build.status.success(),
        "js build failed for {}: {}",
        label,
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node").arg(&mjs).output().expect("node run");
    let _ = fs::remove_file(&mjs);
    assert!(
        run.status.success(),
        "node run failed for {}: {}",
        label,
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).trim_end().to_string()
}

fn run_native(source: &str, label: &str) -> String {
    let src = write_source(label, source);
    let bin = unique_temp(&format!("c20_http_{}", label), "bin");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("taida native build");
    let _ = fs::remove_file(&src);
    assert!(
        build.status.success(),
        "native build failed for {}: {}",
        label,
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("native run");
    let _ = fs::remove_file(&bin);
    assert!(
        run.status.success(),
        "native run failed for {}: status={:?}, stderr={}",
        label,
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).trim_end().to_string()
}

fn assert_header_on_wire(req: &str, name: &str, value: &str, backend: &str, label: &str) {
    // HTTP/1 headers are case-insensitive on name but we emit exactly
    // what the caller spelled, so pin the exact bytes.
    let needle = format!("{}: {}", name, value);
    assert!(
        req.contains(&needle),
        "[{}/{}] missing `{}: {}` in wire request. Full request:\n{}",
        backend,
        label,
        name,
        value,
        req
    );
}

fn assert_body(req: &str, body: &str, backend: &str, label: &str) {
    assert!(
        req.ends_with(body),
        "[{}/{}] wire body mismatch, expected to end with {:?}. Full request:\n{}",
        backend,
        label,
        body,
        req
    );
}

// ── list-of-record headers: 3-backend parity ──

fn run_list_of_record_backend(backend: &str) {
    let (port, rx, handle) = spawn_http_echo_server();
    let source = format!(
        r#"resp <= HttpRequest["POST", "http://127.0.0.1:{port}/echo"](
  headers <= @[
    @(name <= "x-api-key", value <= "secret-k"),
    @(name <= "anthropic-version", value <= "2023-06-01"),
  ],
  body <= "ping",
)
resp ]=> out
stdout(out.__value.status.toString())
stdout(out.__value.body)
"#
    );

    let label = format!("list_of_record_{}", backend);
    let out = match backend {
        "interp" => run_interp(&source, &label),
        "js" => run_js(&source, &label),
        "native" => run_native(&source, &label),
        other => panic!("unknown backend {}", other),
    };
    assert_eq!(
        out, "201\nok",
        "[{}/{}] taida-side response mismatch",
        backend, label
    );

    let req = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("[{}/{}] no request captured", backend, label));
    assert_header_on_wire(&req, "x-api-key", "secret-k", backend, &label);
    assert_header_on_wire(&req, "anthropic-version", "2023-06-01", backend, &label);
    assert_body(&req, "ping", backend, &label);
    handle.join().expect("join http server");
}

#[test]
fn c20_http_list_of_record_headers_interp_on_the_wire() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    run_list_of_record_backend("interp");
}

#[test]
fn c20_http_list_of_record_headers_js_on_the_wire() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    run_list_of_record_backend("js");
}

#[test]
fn c20_http_list_of_record_headers_native_on_the_wire() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    run_list_of_record_backend("native");
}

// ── legacy buchi-pack headers must still work (no regression) ──

fn run_legacy_buchi_pack_backend(backend: &str) {
    let (port, rx, handle) = spawn_http_echo_server();
    let source = format!(
        r#"resp <= HttpRequest["POST", "http://127.0.0.1:{port}/echo"](
  headers <= @(x_test <= "abc"),
  body <= "ping",
)
resp ]=> out
stdout(out.__value.status.toString())
"#
    );

    let label = format!("legacy_pack_{}", backend);
    let out = match backend {
        "interp" => run_interp(&source, &label),
        "js" => run_js(&source, &label),
        "native" => run_native(&source, &label),
        other => panic!("unknown backend {}", other),
    };
    assert_eq!(out, "201", "[{}/{}] response mismatch", backend, label);
    let req = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("[{}/{}] no request captured", backend, label));
    assert_header_on_wire(&req, "x_test", "abc", backend, &label);
    assert_body(&req, "ping", backend, &label);
    handle.join().expect("join http server");
}

#[test]
fn c20_http_legacy_buchi_pack_headers_interp_still_works() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    run_legacy_buchi_pack_backend("interp");
}

#[test]
fn c20_http_legacy_buchi_pack_headers_js_still_works() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    run_legacy_buchi_pack_backend("js");
}

#[test]
fn c20_http_legacy_buchi_pack_headers_native_still_works() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    run_legacy_buchi_pack_backend("native");
}

// ── ROOT-16: JS must reject malformed arity at compile time ──

#[test]
fn c20_http_request_missing_url_js_build_fails() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    // HttpRequest["GET"]() — type arg count 1; too few.
    // Before C20-4 the JS codegen emitted
    //   __taida_os_httpRequest(, null, null)
    // which is a JavaScript syntax error. After C20-4 the JS codegen
    // raises `HttpRequest requires at least 2 type arguments`
    // matching the Interpreter / Native rejection path.
    let source = "resp <= HttpRequest[\"GET\"]()\nstdout(\"never\")\n";
    let src = write_source("root16_arity", source);

    let mjs = unique_temp("c20_http_root16", "mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&mjs)
        .output()
        .expect("taida js build");
    let _ = fs::remove_file(&src);
    let _ = fs::remove_file(&mjs);
    assert!(
        !build.status.success(),
        "JS build unexpectedly succeeded for malformed HttpRequest arity"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("HttpRequest") && stderr.contains("at least 2"),
        "JS build stderr missing ROOT-16 arity diagnostic. stderr={}",
        stderr
    );
}
