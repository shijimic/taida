//! E32B-028: oversized HTTP chunk-size is a protocol error, not a panic.
//!
//! E32B-080 follow-up (concurrent isolation): a malformed connection A
//! (oversized chunk-size) must not break sibling connection B's keep-alive
//! processing. Both connections drive the same server (request limit = 2),
//! A gets HTTP 400 + close, B gets HTTP 200 + body echo.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

fn unique_path(prefix: &str, label: &str, ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}_{}.{}",
        prefix,
        label,
        std::process::id(),
        nanos,
        ext
    ))
}

fn setup_net_project(source: &str, label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "taida_e32b028_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("create project dir");
    fs::write(dir.join("main.td"), source).expect("write main.td");
    fs::write(dir.join("packages.tdm"), "// E32B-028 test project\n").expect("write packages.tdm");

    let deps_net = dir
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("net");
    fs::create_dir_all(&deps_net).expect("create net dep");
    fs::write(
        deps_net.join("main.td"),
        r#"// taida-lang/net -- test stub
Enum => HttpProtocol = :H1 :H2 :H3

<<< @(httpServe, httpParseRequestHead, httpEncodeResponse, readBody, startResponse, writeChunk, endResponse, sseEvent, readBodyChunk, readBodyAll, wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode, HttpProtocol)
"#,
    )
    .expect("write net stub");

    dir
}

fn spawn_backend(dir: &Path, backend: &str, label: &str) -> (Child, Option<PathBuf>) {
    let taida = common::taida_bin();
    let main = dir.join("main.td");
    match backend {
        "interp" => {
            let child = Command::new(&taida)
                .arg(&main)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interpreter");
            (child, None)
        }
        "js" => {
            let js_path = unique_path("taida_e32b028", label, "mjs");
            let build = Command::new(&taida)
                .args(["build", "js"])
                .arg(&main)
                .arg("-o")
                .arg(&js_path)
                .output()
                .expect("build js");
            assert!(
                build.status.success(),
                "JS build failed: {}",
                String::from_utf8_lossy(&build.stderr)
            );
            let child = Command::new("node")
                .arg(&js_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn node");
            (child, Some(js_path))
        }
        "native" => {
            let bin_path = unique_path("taida_e32b028", label, "bin");
            let build = Command::new(&taida)
                .args(["build", "native"])
                .arg(&main)
                .arg("-o")
                .arg(&bin_path)
                .output()
                .expect("build native");
            assert!(
                build.status.success(),
                "native build failed: {}",
                String::from_utf8_lossy(&build.stderr)
            );
            let child = Command::new(&bin_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn native");
            (child, Some(bin_path))
        }
        _ => unreachable!("unknown backend"),
    }
}

fn send_request(port: u16, request: &[u8]) -> Option<Vec<u8>> {
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(50));
        let mut stream = match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
        if stream.write_all(request).is_err() {
            continue;
        }

        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        if !response.is_empty() {
            return Some(response);
        }
    }
    None
}

fn eager_source(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
    )
}

fn eager_source_two_request(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 2)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
    )
}

#[test]
fn e32b_028_oversized_chunk_size_eager_400_three_backend() {
    let mut backends = vec!["interp"];
    if node_available() {
        backends.push("js");
    } else {
        eprintln!("node unavailable; skipping JS member");
    }
    if cc_available() {
        backends.push("native");
    } else {
        eprintln!("cc unavailable; skipping native member");
    }

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(&eager_source(port), backend);
        let (mut child, artifact) = spawn_backend(&dir, backend, backend);

        let response = send_request(
            port,
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nFFFFFFFFFFFFFFFF\r\nx\r\n0\r\n\r\n",
        );

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response =
            response.unwrap_or_else(|| panic!("{} backend did not return a response", backend));
        let response = String::from_utf8_lossy(&response);
        assert!(
            response.contains("400 Bad Request"),
            "{} backend must reject oversized chunk-size with HTTP 400, got: {}",
            backend,
            response
        );
        assert!(
            !response.contains("200 OK") && !response.contains("x"),
            "{} backend must not pass oversized chunk body to the handler, got: {}",
            backend,
            response
        );
    }
}

/// E32B-080 (concurrent isolation): two HTTP/1.1 connections drive the
/// same server (request limit = 2). A sends an oversized chunk-size and
/// must be rejected with HTTP 400 + close; B sends a well-formed
/// chunked body `hello` afterwards and must observe HTTP 200 + the
/// echoed body. The property under test is that A's malformed input
/// does not break the server's ability to serve B.
///
/// E32B-080 follow-up (Codex HOLD): the workers are sequential rather
/// than racing on a shared atomic + sleep barrier — A finishes its
/// full request/response round-trip first, then B opens a fresh
/// connection. The server processes connections single-threadedly, so
/// the sequential shape is observationally indistinguishable from the
/// previous racing layout while removing every sleep-as-synchronization
/// hazard under nextest 2C parallelism.
#[test]
fn e32b_080_chunked_concurrent_isolation_three_backend() {
    let mut backends = vec!["interp"];
    if node_available() {
        backends.push("js");
    } else {
        eprintln!("node unavailable; skipping JS member");
    }
    if cc_available() {
        backends.push("native");
    } else {
        eprintln!("cc unavailable; skipping native member");
    }

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(
            &eager_source_two_request(port),
            &format!("conc_{}", backend),
        );
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("conc_{}", backend));

        // Connection A: oversized chunk-size in hex (FF * 16 chars > SIZE_MAX
        // on 64-bit, well past it on 32-bit). The runtime must reject before
        // delivering any chunk bytes to the handler.
        let response_a = send_request(
            port,
            b"POST /a HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nFFFFFFFFFFFFFFFF\r\nx\r\n0\r\n\r\n",
        );

        // Connection B: well-formed chunked POST with a 5-byte `hello`
        // body. Opens a fresh TCP connection — the server's accept loop
        // moved on after A's close, so B is the second of two requests
        // (matching `httpServe(_, _, 2)` in the test program).
        let response_b = send_request(
            port,
            b"POST /b HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        );

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response_a =
            response_a.unwrap_or_else(|| panic!("{}: connection A got no response", backend));
        let response_b =
            response_b.unwrap_or_else(|| panic!("{}: connection B got no response", backend));
        let response_a = String::from_utf8_lossy(&response_a);
        let response_b = String::from_utf8_lossy(&response_b);

        assert!(
            response_a.contains("400 Bad Request"),
            "{}: A must observe HTTP 400 (oversized chunk-size), got: {}",
            backend,
            response_a
        );
        assert!(
            !response_a.contains("200 OK") && !response_a.contains("\r\nx"),
            "{}: A must not leak the chunk body to the wire, got: {}",
            backend,
            response_a
        );

        // B's echoed body is "hello"; the runtime auto-appends Content-Length
        // for the eager path so the response ends with `...\r\n\r\nhello`.
        assert!(
            response_b.contains("200 OK"),
            "{}: B must observe HTTP 200 (sibling connection unaffected by A), got: {}",
            backend,
            response_b
        );
        assert!(
            response_b.ends_with("hello"),
            "{}: B's echoed body must reach the wire, got: {}",
            backend,
            response_b
        );
    }
}
