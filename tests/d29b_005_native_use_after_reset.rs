//! D29B-005 (Track-η Phase 6, 2026-04-27) — Native `req.raw` use-after-reset
//! contract pin.
//!
//! # Background
//!
//! C26B-020 established that `req.raw` (the underlying request byte buffer
//! exposed to handlers) is `taida_retain`'d at request-pack construction
//! time so that the buffer remains live across `taida_arena_request_reset()`
//! and is reachable from `taida_net_SpanEquals` / `SpanStartsWith` /
//! `SpanContains` regardless of when the next request resets the per-request
//! arena.
//!
//! The Track-η leak fix (`taida_net_raw_as_bytes` ABI rewrite, Lock-Phase6-A
//! Option D) reshapes the resolver around the same `req.raw` value but does
//! not change its retain/release lifetime. This test pins the surviving
//! contract: a Native-compiled handler that runs `SpanEquals` on `req.raw`
//! across **multiple sequential requests** must continue to observe the
//! correct request bytes — i.e. the resolver does not see freed memory or
//! a reused-arena slot from the previous request.
//!
//! # Methodology
//!
//! Build a Native HTTP/1 server that serves N=3 sequential requests using
//! a 4-byte method comparison (`SpanEquals[span, req.raw, "GET"]`). Each
//! response body echoes the parsed method string and a fixed marker. The
//! test sends 3 GET requests on the same connection (`Connection: keep-alive`
//! handled implicitly by the per-request loop), verifies all three return
//! `200 OK` with the marker, and asserts that none of the responses
//! contains a `500 Internal Server Error` or aborted-connection symptom
//! that would indicate the second / third `SpanEquals` evaluation walked
//! into a recycled arena chunk.
//!
//! Co-pinned with `tests/d29b_012_native_span_zero_alloc_no_leak.rs` which
//! drives the same shape under valgrind for the leak-side guarantee.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod common;

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

fn find_free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

fn tempdir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "taida_d29b005_useafter_{}_{}_{}",
        name,
        std::process::id(),
        nanos
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    dir
}

fn write_server(dir: &Path, port: u16) -> PathBuf {
    // 3 sequential requests; for each we evaluate SpanEquals against
    // req.raw to trigger the Track-η resolver path. The body returns a
    // marker that flips depending on the SpanEquals verdict so any
    // use-after-reset bug (e.g. resolver reading stale arena bytes)
    // would surface as the wrong marker.
    let src = format!(
        r#">>> taida-lang/net => @(httpServe)

handler req =
  isGet <= SpanEquals[req.method, req.raw, "GET"]()
  marker <= "ok-" + isGet.toString()
  @(status <= 200, headers <= @[], body <= marker)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 3, 5000, 4)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#,
        port = port
    );
    let path = dir.join("main.td");
    std::fs::write(&path, src).expect("write main.td");
    path
}

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

fn spawn_server(bin: &Path, port: u16) -> Child {
    let mut child = Command::new(bin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn native server");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", port).parse().unwrap(),
            Duration::from_millis(150),
        )
        .is_ok()
        {
            return child;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("native server did not bind port {} within 5s", port);
}

fn send_one_request(port: u16) -> String {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let req = b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    sock.write_all(req).expect("write request");
    let mut resp = Vec::new();
    let _ = sock.read_to_end(&mut resp);
    String::from_utf8_lossy(&resp).into_owned()
}

#[test]
fn native_req_raw_lives_across_arena_reset() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let dir = tempdir("seq3");
    let port = find_free_loopback_port();
    let src = write_server(&dir, port);
    let bin = dir.join("server.bin");
    if !build_native(&src, &bin) {
        panic!("taida build --target native failed");
    }

    let mut child = spawn_server(&bin, port);

    // Drive 3 sequential requests. Between each request the per-request
    // arena is reset; if `req.raw` were not retained / dispatched
    // independently from the arena reset, the second / third SpanEquals
    // would either segfault, return 0 (wrong marker), or 500 Internal
    // Server Error.
    let r1 = send_one_request(port);
    let r2 = send_one_request(port);
    let r3 = send_one_request(port);

    // Wait for child to drain.
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < exit_deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    for (i, resp) in [&r1, &r2, &r3].iter().enumerate() {
        assert!(
            resp.contains("200 OK"),
            "D29B-005 use-after-reset regression: request {} did not return 200 OK.\n\
             req.raw lifetime contract C26B-020 likely violated by Track-η resolver.\n\
             Response: {:?}",
            i + 1,
            resp
        );
        assert!(
            resp.contains("ok-true"),
            "D29B-005 use-after-reset regression: request {} returned wrong marker.\n\
             SpanEquals[req.method, req.raw, \"GET\"] should evaluate to true\n\
             on every request, but the resolver appears to have read stale or\n\
             freed memory after taida_arena_request_reset().\n\
             Response: {:?}",
            i + 1,
            resp
        );
        assert!(
            !resp.contains("500"),
            "D29B-005 use-after-reset regression: request {} hit 500 status.\n\
             Likely a use-after-free in taida_net_raw_as_bytes after arena reset.\n\
             Response: {:?}",
            i + 1,
            resp
        );
    }
}
