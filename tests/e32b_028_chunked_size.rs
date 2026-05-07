//! E32B-028: oversized HTTP chunk-size is a protocol error, not a panic.
//!
//! Process-survival regression: a malformed connection A
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

// E32B-053 follow-up: 2-arg streaming handler driving readBodyAll, which
// exercises the line-by-line streaming chunked decoder (Native
// NET4_CHUNKED_WAIT_SIZE in readBodyAll, JS __taida_net_readBodyAllImpl,
// Interpreter chunked_state transition in stream.rs). Distinct from the
// eager `chunked_in_place_compact` path that the existing 1-arg fixtures
// drive.
fn streaming_source(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe, readBodyAll, startResponse, writeChunk, endResponse)

handler req writer =
  body <= readBodyAll(req)
  startResponse(writer, 200, @[@(name <= "Content-Type", value <= "application/octet-stream")])
  writeChunk(writer, body)
  endResponse(writer)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
    )
}

// E32B-068: distinct from `streaming_source` (which uses readBodyAll), this
// 2-arg handler drives the streaming `readBodyChunk` API directly. The
// chunk-size guard must reject malformed framing on this path even when the
// handler only requests a single chunk before responding — otherwise an
// attacker could bypass the eager guarantees by attaching a chunk-by-chunk
// streaming handler. The handler unmolds the `Lax[Bytes]` via `]=>` so we
// stay clear of the compiler-internal `__value` accessor (rejected by
// E1960) while still exercising the readBodyChunk codepath end-to-end.
fn streaming_chunk_source(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe, readBodyChunk, startResponse, writeChunk, endResponse)

handler req writer =
  chunk <= readBodyChunk(req)
  chunk ]=> bytes
  startResponse(writer, 200, @[@(name <= "Content-Type", value <= "application/octet-stream")])
  writeChunk(writer, bytes)
  endResponse(writer)

asyncResult <= httpServe({port}, handler, 1)
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

/// Process-survival regression: two HTTP/1.1 connections drive the
/// same server (request limit = 2). A sends an oversized chunk-size and
/// must be rejected with HTTP 400 + close; B sends a well-formed
/// chunked body `hello` afterwards and must observe HTTP 200 + the
/// echoed body. The property under test is that A's malformed input
/// does not break the server's ability to serve B.
///
/// The workers are intentionally sequential: A finishes its full
/// request/response round-trip first, then B opens a fresh connection.
/// This test pins process-wide survival after malformed input; it does
/// not claim overlap between sibling connections.
#[test]
fn chunked_process_survival_three_backend() {
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

/// E32B-053: chunk-size with leading OWS (space before the hex digits) must
/// be rejected as malformed by all three backends — RFC 7230 §4.1 forbids
/// OWS within `chunk-size`. Reverse-proxy interpretation drift around OWS
/// is the canonical request-smuggling vector this test pins.
#[test]
fn e32b_053_chunk_size_leading_ows_rejected_three_backend() {
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
        let dir = setup_net_project(&eager_source(port), &format!("ows_{}", backend));
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("ows_{}", backend));

        // Leading SP before the hex chunk-size.
        let response = send_request(
            port,
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n 5\r\nhello\r\n0\r\n\r\n",
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
            "{} backend must reject leading-OWS chunk-size with HTTP 400, got: {}",
            backend,
            response
        );
        assert!(
            !response.contains("200 OK") && !response.ends_with("hello"),
            "{} backend must not deliver the body to the handler when chunk-size has OWS, got: {}",
            backend,
            response
        );
    }
}

/// E32B-053: chunk-size with 16 hex digits (one more than the 15-digit cap)
/// even when its magnitude fits in a `usize` must be rejected — leading
/// zeros count toward the cap. This pins the leading-zero policy across
/// the three backends.
#[test]
fn e32b_053_chunk_size_leading_zero_overflows_cap_three_backend() {
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
        let dir = setup_net_project(&eager_source(port), &format!("lzcap_{}", backend));
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("lzcap_{}", backend));

        // 15 zeros + `1` = 16 hex digits → over the 15-digit cap.
        let response = send_request(
            port,
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n0000000000000001\r\nx\r\n0\r\n\r\n",
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
            "{} backend must reject 16-digit chunk-size (leading zeros counted) with HTTP 400, got: {}",
            backend,
            response
        );
    }
}

/// E32B-053 follow-up: leading-OWS chunk-size must be rejected on the
/// streaming path (`readBodyAll`) too. The Codex review uncovered that
/// the 2026-05-07 fix only touched the eager helpers, so JS
/// `__taida_net_readBodyAllImpl` / `__taida_net_readBodyChunkChunkedSync`
/// and Native `readBodyAll` continued to silently strip OWS. This test
/// pins the streaming path closure.
#[test]
fn e32b_053_streaming_chunk_size_leading_ows_rejected_three_backend() {
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
        let dir = setup_net_project(&streaming_source(port), &format!("ows_stream_{}", backend));
        let (mut child, artifact) =
            spawn_backend(&dir, backend, &format!("ows_stream_{}", backend));

        // Leading SP before the hex chunk-size on a 2-arg / streaming handler.
        let response = send_request(
            port,
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n 5\r\nhello\r\n0\r\n\r\n",
        );

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        // The streaming path may either return HTTP 400 or close the
        // connection without a response (Native readBodyAll currently
        // calls `taida_net4_abort_connection` which `shutdown(SHUT_RDWR)`
        // the socket without writing anything). Either way the body must
        // not be echoed back.
        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.contains("200 OK") && !response.ends_with("hello"),
            "{} streaming backend must not deliver the OWS-prefixed body to the handler, got: {:?}",
            backend,
            response
        );
    }
}

/// E32B-051: chunk-ext flood. A single chunk-size line (`1;` followed by a
/// 2 MiB padding) must be rejected as malformed by the eager path on all
/// three backends. Without the per-line cap shared between Interpreter / JS /
/// Native this test would force unbounded CRLF scans on the smaller backends.
#[test]
fn e32b_051_chunk_extension_flood_rejected_three_backend() {
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

    // 2 MiB of `a=b;` padding inside the chunk-ext, well past the shared
    // 1 MiB per-line cap.
    let mut padding = String::with_capacity(2 * 1024 * 1024);
    while padding.len() < 2 * 1024 * 1024 {
        padding.push_str("a=b;");
    }

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(&eager_source(port), &format!("extflood_{}", backend));
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("extflood_{}", backend));

        let mut request = Vec::with_capacity(padding.len() + 256);
        request.extend_from_slice(
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1;",
        );
        request.extend_from_slice(padding.as_bytes());
        request.extend_from_slice(b"\r\nx\r\n0\r\n\r\n");

        let response = send_request(port, &request);

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        // Either an explicit HTTP 400 or a connection close with no body
        // delivered. The handler must never observe the chunk payload.
        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.contains("200 OK"),
            "{} backend must not 200 a 2 MiB chunk-ext flood, got: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
        assert!(
            !response.ends_with("\r\nx") && !response.contains("\r\n\r\nx"),
            "{} backend must not echo the chunk-data after a chunk-ext flood, got prefix: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
    }
}

/// E32B-052: trailer-count flood. After the terminator chunk a body that
/// emits 200 trailer lines (each `X-N: 1`) must be rejected on all three
/// backends — well past the shared 64-line cap.
#[test]
fn e32b_052_trailer_count_flood_rejected_three_backend() {
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

    // 200 trailer lines, each tiny (~10 bytes), so the count cap fires
    // before the byte cap.
    let mut trailers = String::new();
    for i in 0..200 {
        trailers.push_str(&format!("X-T-{}: 1\r\n", i));
    }
    trailers.push_str("\r\n");

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(&eager_source(port), &format!("trcnt_{}", backend));
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("trcnt_{}", backend));

        let mut request = Vec::with_capacity(trailers.len() + 256);
        request.extend_from_slice(
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n",
        );
        request.extend_from_slice(trailers.as_bytes());

        let response = send_request(port, &request);

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.contains("200 OK"),
            "{} backend must reject a 200-trailer flood, got prefix: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
    }
}

/// E32B-051 (streaming-path closure): a 2 MiB chunk-extension flood directed
/// at a 2-arg streaming handler (`readBodyAll`) must be rejected by every
/// backend. This guards the per-line cap at the streaming path, which is a
/// distinct line reader from the eager `chunked_body_complete` decoder. A
/// missing cap on the streaming path lets attackers bypass the eager
/// guarantees just by attaching a streaming handler.
#[test]
fn e32b_051_streaming_chunk_extension_flood_rejected_three_backend() {
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

    let mut padding = String::with_capacity(2 * 1024 * 1024);
    while padding.len() < 2 * 1024 * 1024 {
        padding.push_str("a=b;");
    }

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(
            &streaming_source(port),
            &format!("extflood_stream_{}", backend),
        );
        let (mut child, artifact) =
            spawn_backend(&dir, backend, &format!("extflood_stream_{}", backend));

        let mut request = Vec::with_capacity(padding.len() + 256);
        request.extend_from_slice(
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1;",
        );
        request.extend_from_slice(padding.as_bytes());
        request.extend_from_slice(b"\r\nx\r\n0\r\n\r\n");

        let response = send_request(port, &request);

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.contains("200 OK"),
            "{} streaming backend must not 200 a 2 MiB chunk-ext flood, got: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
        // The streaming decoder must not let the chunk-ext padding leak into
        // the response body (which is what readBodyAll would echo back).
        assert!(
            !response.contains("a=b;"),
            "{} streaming backend must not echo chunk-ext padding, got prefix: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
    }
}

/// E32B-052 (streaming-path closure): a 200-line trailer flood must be
/// rejected on the streaming path (`readBodyAll`) as malformed framing —
/// not silently consumed as success — on all three backends.
#[test]
fn e32b_052_streaming_trailer_count_flood_rejected_three_backend() {
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

    let mut trailers = String::new();
    for i in 0..200 {
        trailers.push_str(&format!("X-T-{}: 1\r\n", i));
    }
    trailers.push_str("\r\n");

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(
            &streaming_source(port),
            &format!("trcnt_stream_{}", backend),
        );
        let (mut child, artifact) =
            spawn_backend(&dir, backend, &format!("trcnt_stream_{}", backend));

        let mut request = Vec::with_capacity(trailers.len() + 256);
        request.extend_from_slice(
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n",
        );
        request.extend_from_slice(trailers.as_bytes());

        let response = send_request(port, &request);

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.ends_with("hello"),
            "{} streaming backend must not echo body after 200-trailer flood, got: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
    }
}

/// E32B-052: trailer-bytes flood. 32 trailer lines (well under the 64-count
/// cap), each carrying a 512-byte name+value pair, sum to ~16 KiB — twice
/// the 8 KiB total-bytes cap. All three backends must reject the message.
#[test]
fn e32b_052_trailer_bytes_flood_rejected_three_backend() {
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

    // Each trailer line is ~512 bytes (`X-T-NN: <500 chars>`); 32 lines
    // exceeds the 8 KiB shared cap while staying under the 64-count cap.
    let padding: String = std::iter::repeat('a').take(500).collect();
    let mut trailers = String::new();
    for i in 0..32 {
        trailers.push_str(&format!("X-T-{}: {}\r\n", i, padding));
    }
    trailers.push_str("\r\n");

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(&eager_source(port), &format!("trbyt_{}", backend));
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("trbyt_{}", backend));

        let mut request = Vec::with_capacity(trailers.len() + 256);
        request.extend_from_slice(
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n",
        );
        request.extend_from_slice(trailers.as_bytes());

        let response = send_request(port, &request);

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.contains("200 OK"),
            "{} backend must reject a 16 KiB trailer-bytes flood, got prefix: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
    }
}

/// E32B-068: oversized chunk-size on the readBodyChunk streaming path.
/// The eager-path test (`e32b_028_oversized_chunk_size_eager_400_three_backend`)
/// pins the 1-arg handler reject. This sibling pins the 2-arg streaming
/// `readBodyChunk` reject so the per-chunk path cannot bypass the cap by
/// reading chunks individually instead of via `readBodyAll`. All three
/// backends must refuse to deliver the chunk body to the handler, which
/// means the handler-side echo (`chunk.__value`) must never reach the wire.
#[test]
fn e32b_068_streaming_readbodychunk_oversized_chunk_size_three_backend() {
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
            &streaming_chunk_source(port),
            &format!("rbc_oversize_{}", backend),
        );
        let (mut child, artifact) =
            spawn_backend(&dir, backend, &format!("rbc_oversize_{}", backend));

        // Same oversized chunk-size as the eager test: 16 hex digits worth of
        // F's overflows SIZE_MAX on 64-bit systems. The runtime must reject
        // before the handler can observe the `x` byte chunk-data.
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

        // The streaming path may either return HTTP 400 or close the
        // connection without a response (Native's readBodyChunk calls
        // `taida_net4_abort_connection` which `shutdown(SHUT_RDWR)`s the
        // socket without writing anything). What must hold: the handler
        // never observes the chunk-data, so the response must NOT contain
        // the echoed `x` body, and must not be a 200 OK.
        let response = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        assert!(
            !response.contains("200 OK"),
            "{} streaming readBodyChunk backend must not 200 an oversized chunk-size, got: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
        // The handler-side echo (`writeChunk(writer, bytes)` after
        // unmolding the Lax) would emit the byte `x` either as a
        // chunked-encoding frame `\r\n1\r\nx\r\n` or as a raw trailing
        // `\r\nx`. Reject all three echo shapes to ensure no chunk-data
        // leaked through the cap. The Codex review for this batch
        // flagged that the chunked-encoding shape was not previously
        // covered by the assert.
        assert!(
            !response.ends_with("\r\nx")
                && !response.contains("\r\n\r\nx")
                && !response.contains("\r\n1\r\nx\r\n"),
            "{} streaming readBodyChunk backend must not deliver chunk-data after oversized chunk-size, got prefix: {:?}",
            backend,
            &response[..response.len().min(200)]
        );
    }
}
