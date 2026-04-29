//! D28B-025 (Round 2 wG follow-up): HTTP/2 RFC 9113 no-body content-length parity.
//!
//! Pre-fix the h2 server response path passed `resp.headers` straight
//! through `h2_send_response_headers` when `!has_body` was true, which
//! includes status codes 1xx / 204 / 205 / 304. RFC 9113 §8.1.1 + RFC
//! 9110 §6.4 forbid content-length / transfer-encoding on those
//! responses; sending one is a protocol error that compliant h2
//! clients (curl --http2, hyper, h2spec) reject with PROTOCOL_ERROR.
//! The h1 path already strips these headers; the h2 path was missing
//! the symmetric guard until D28B-025.
//!
//! This regression test exercises the h2 server with a handler that
//! returns status 204 + an explicit `content-length: 5` header, then
//! checks that the wire response from `curl --http2 -i` does NOT
//! contain `content-length:` in the response head. We use 204 (the
//! most common no-body code in REST APIs) as the representative case;
//! the same fix path covers 1xx / 205 / 304 because the
//! `no_body` predicate in `taida_net_h2_serve_connection` covers all
//! four. Adding extra status codes here would only re-test the same
//! `if (no_body && needs_strip)` branch.
//!
//! Skip semantics match `tests/d28b_002_h2_arena_leak.rs`: requires
//! Linux + cc + openssl + curl with HTTP/2 support.

mod common;

use common::taida_bin;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn fixture_template() -> &'static str {
    // Handler returns status 204 + an explicit content-length header
    // that a compliant h2 server MUST strip before HPACK encoding.
    // The body is intentionally non-empty (5 bytes "hello") to make
    // the violation observable: pre-fix, `has_body` is false because
    // status is 204, so the `if (!has_body)` branch sends headers
    // verbatim including content-length.
    r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 204, headers <= @[@(name <= "content-length", value <= "5"), @(name <= "x-test", value <= "d28b025")], body <= "hello")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 16, 30000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= "h2"))
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn curl_h2_available() -> bool {
    match Command::new("curl").arg("--version").output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains("HTTP2"),
        Err(_) => false,
    }
}

fn gen_self_signed_cert(cert: &Path, key: &Path) -> bool {
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
            key.to_str().unwrap_or(""),
            "-out",
            cert.to_str().unwrap_or(""),
            "-days",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn build_native(td: &Path) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let bin = std::env::temp_dir().join(format!("d28b_025_{}_{}.bin", std::process::id(), seq));
    let out = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "native build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(bin)
}

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

fn wait_for_bind(port: u16, server: &mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = server.try_wait() {
            return false;
        }
        if let Ok(addr) = format!("127.0.0.1:{port}").parse::<std::net::SocketAddr>()
            && std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

#[test]
fn d28b_025_native_h2_no_body_strips_content_length() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping: harness uses TLS+h2 server which is exercised on Linux only");
        return;
    }
    if !taida_bin().exists() {
        eprintln!("skipping: taida release binary not built");
        return;
    }
    if !cc_available() {
        eprintln!("SKIP: cc unavailable");
        return;
    }
    if !openssl_available() {
        eprintln!("SKIP: openssl unavailable");
        return;
    }
    if !curl_h2_available() {
        eprintln!("SKIP: curl --http2 unavailable");
        return;
    }

    let cert_path =
        std::env::temp_dir().join(format!("d28b_025_h2_cert_{}.pem", std::process::id()));
    let key_path = std::env::temp_dir().join(format!("d28b_025_h2_key_{}.pem", std::process::id()));
    if !gen_self_signed_cert(&cert_path, &key_path) {
        eprintln!("SKIP: cert generation failed");
        return;
    }

    let port = find_free_port();
    let source = fixture_template()
        .replace("{port}", &format!("{port}"))
        .replace("{cert}", cert_path.to_str().unwrap_or(""))
        .replace("{key}", key_path.to_str().unwrap_or(""));

    let dir = std::env::temp_dir().join(format!("d28b_025_h2_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let td_path = dir.join("main.td");
    std::fs::write(&td_path, source).expect("write fixture");

    let bin = match build_native(&td_path) {
        Some(b) => b,
        None => {
            let _ = std::fs::remove_file(&cert_path);
            let _ = std::fs::remove_file(&key_path);
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("SKIP: native build of d28b_025 fixture failed");
            return;
        }
    };

    let mut server = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("server spawn");

    if !wait_for_bind(port, &mut server) {
        let _ = server.kill();
        let _ = server.wait();
        let _ = std::fs::remove_file(&bin);
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir_all(&dir);
        panic!("d28b_025: h2 server failed to bind 127.0.0.1:{port} within 20s");
    }

    // Issue one h2 request with -i (include response headers) and -k
    // (insecure). curl returns status + headers + (empty) body in the
    // response output; we scan the head for content-length.
    let url = format!("https://127.0.0.1:{port}/");
    let curl_out = Command::new("curl")
        .args([
            "--http2",
            "--insecure",
            "--silent",
            "--include",
            "--max-time",
            "5",
            &url,
        ])
        .output();

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&cert_path);
    let _ = std::fs::remove_file(&key_path);
    let _ = std::fs::remove_dir_all(&dir);

    let curl_out = curl_out.expect("curl invocation");
    if !curl_out.status.success() {
        // curl exit non-zero: that is the *pre-fix* protocol-error
        // signature (curl rejects content-length on 204 with an h2
        // PROTOCOL_ERROR). We deliberately allow this path to fail
        // here so the assertion message points at D28B-025 directly.
        let stderr = String::from_utf8_lossy(&curl_out.stderr);
        panic!(
            "d28b_025: curl --http2 failed (likely PROTOCOL_ERROR signature \
             of pre-fix D28B-025): {stderr}"
        );
    }

    let head_and_body = String::from_utf8_lossy(&curl_out.stdout).into_owned();
    eprintln!("d28b_025: curl response = {head_and_body:?}");

    // Status line must indicate 204.
    assert!(
        head_and_body.contains("HTTP/2 204")
            || head_and_body.contains("HTTP/2.0 204")
            || head_and_body.starts_with("HTTP/2 204"),
        "d28b_025: expected HTTP/2 204 status line, got: {head_and_body:?}"
    );

    // The response head must NOT contain content-length:. We split on
    // the head/body boundary to avoid a false positive if the body
    // happened to contain the literal token (the body is empty for
    // 204, but defence in depth).
    let head_end = head_and_body
        .find("\r\n\r\n")
        .unwrap_or(head_and_body.len());
    let head = &head_and_body[..head_end];
    assert!(
        !head.to_lowercase().contains("content-length"),
        "d28b_025: response head contains content-length on a 204 response, \
         which is a RFC 9113 §8.1.1 violation. The pre-fix h2 path was \
         passing user-supplied content-length straight through HPACK \
         encode in the `if (!has_body)` branch of \
         taida_net_h2_serve_connection. head = {head:?}"
    );

    // Sanity: the user-set x-test header should still be present
    // (the strip filter must only drop content-length, not arbitrary
    // headers).
    assert!(
        head.to_lowercase().contains("x-test"),
        "d28b_025: x-test header was stripped along with content-length. \
         The D28B-025 strip filter is over-eager. head = {head:?}"
    );
}
