//! E32B-080: 3-backend RFC 7230 grammar E2E.
//!
//! Static checks in `tests/e32b_027_net_header_guards.rs` and
//! `tests/e32b_039_040_041_hardening.rs` pin that the validator code is
//! present in the runtime sources. This test goes one step further:
//! it spawns each backend (`interp` / `js` / `native`) as a real process
//! and drives the eager handler-return path with seven malformed-header
//! responses. The runtime must hard-fail (HTTP 500) on every case and
//! never leak the malformed bytes onto the wire.
//!
//! The seven cases mirror the reviewer's bypass demonstration recorded
//! against E32B-041:
//!
//!   1. `':' ` in name (token grammar bypass)
//!   2. NUL in name
//!   3. SP in name (token grammar bypass)
//!   4. control byte (0x01) in name
//!   5. DEL (0x7F) in value (field-value grammar bypass)
//!   6. underscore in name (CL.CL bypass via reverse-proxy normalisation)
//!   7. `Set-Cookie` reserved name
//!
//! E32B-079 follow-up: the WebSocket and chunked concurrent-isolation
//! E2E variants are colocated with `tests/e32b_029_ws_validation.rs` and
//! `tests/e32b_028_chunked_size.rs` respectively, so they can share the
//! per-suite WS / chunked fixtures without duplicating the framing code.

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
        "taida_e32b080_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("create project dir");
    fs::write(dir.join("main.td"), source).expect("write main.td");
    fs::write(dir.join("packages.tdm"), "// E32B-080 test project\n").expect("write packages.tdm");

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
            let js_path = unique_path("taida_e32b080", label, "mjs");
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
            let bin_path = unique_path("taida_e32b080", label, "bin");
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

/// Send a single HTTP/1.1 request on a fresh TCP connection, retrying the
/// initial connect until the spawned server accepts. Returns `None` if no
/// response was read within the retry budget.
fn send_request(port: u16, request: &[u8], wait_for_listener: bool) -> Option<Vec<u8>> {
    let attempts = if wait_for_listener { 80 } else { 20 };
    for _ in 0..attempts {
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

/// Taida program: 7 path-dispatched malformed-header responses driven by the
/// 1-arg eager handler return form. The runtime validator runs in
/// `extract_response_fields` (interp) / `__taida_net_encodeResponseScatter`
/// (JS) / `taida_net_send_response_scatter` (native), and rejects each one.
fn handler_source(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe)

handler req =
  isCase1 <= SpanEquals[req.path, req.raw, "/case1"]()
  isCase2 <= SpanEquals[req.path, req.raw, "/case2"]()
  isCase3 <= SpanEquals[req.path, req.raw, "/case3"]()
  isCase4 <= SpanEquals[req.path, req.raw, "/case4"]()
  isCase5 <= SpanEquals[req.path, req.raw, "/case5"]()
  isCase6 <= SpanEquals[req.path, req.raw, "/case6"]()
  isCase7 <= SpanEquals[req.path, req.raw, "/case7"]()
  badName <= (
    | isCase1 |> "X:Y"
    | isCase2 |> "X\x00Y"
    | isCase3 |> "X Y"
    | isCase4 |> "X\x01Y"
    | isCase6 |> "Content_Length"
    | isCase7 |> "Set-Cookie"
    | _ |> "X-OK"
  )
  badValue <= (
    | isCase5 |> "V\x7FX"
    | _ |> "ok"
  )
  @(status <= 200, headers <= @[@(name <= badName, value <= badValue)], body <= "")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 7)
asyncResult ]=> result
result ]=> r
stdout(r.requests.toString())
"#
    )
}

#[test]
fn e32b_080_grammar_seven_cases_three_backend() {
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

    // (path, human description, byte that should NEVER appear in the wire
    // response — picked from the malformed name/value to make the leak
    // detection unambiguous).
    let cases: [(&str, &str, &[u8]); 7] = [
        ("/case1", "':' in name (X:Y)", b"X:Y"),
        ("/case2", "NUL in name", b"X\x00Y"),
        ("/case3", "space in name", b"X Y"),
        ("/case4", "control 0x01 in name", b"X\x01Y"),
        ("/case5", "DEL 0x7F in value", b"V\x7FX"),
        ("/case6", "underscore in name (CL.CL)", b"Content_Length"),
        ("/case7", "Set-Cookie reserved", b"Set-Cookie"),
    ];

    for backend in backends {
        let port = common::find_free_loopback_port();
        let dir = setup_net_project(&handler_source(port), backend);
        let (mut child, artifact) = spawn_backend(&dir, backend, backend);

        let mut failures: Vec<String> = Vec::new();
        for (i, (path, label, leaked_bytes)) in cases.iter().enumerate() {
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                path
            );
            let response = send_request(
                port,
                request.as_bytes(),
                /* wait_for_listener = */ i == 0,
            );

            let Some(bytes) = response else {
                failures.push(format!("{} [{}]: no response", path, label));
                continue;
            };

            // Status line must be 500 — the runtime validator
            // rejected the handler-supplied bad header.
            let starts_500 = bytes.starts_with(b"HTTP/1.1 500");
            if !starts_500 {
                failures.push(format!(
                    "{} [{}]: expected HTTP/1.1 500, got: {}",
                    path,
                    label,
                    String::from_utf8_lossy(&bytes)
                        .chars()
                        .take(120)
                        .collect::<String>()
                ));
                continue;
            }

            // The malformed header bytes must NOT appear in the wire
            // response — proves the validator caught the input before it
            // could reach the socket.
            if memmem(&bytes, leaked_bytes) {
                failures.push(format!(
                    "{} [{}]: malformed bytes {:?} leaked into wire response: {:?}",
                    path,
                    label,
                    leaked_bytes,
                    String::from_utf8_lossy(&bytes)
                ));
                continue;
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        if !failures.is_empty() {
            panic!(
                "{} backend: E32B-080 grammar E2E failures:\n  - {}",
                backend,
                failures.join("\n  - ")
            );
        }
    }
}

/// `slice::contains` only works for `Vec<u8>`-of-`u8`, but we want to find a
/// multi-byte needle. This is the obvious sliding-window search.
fn memmem(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}
