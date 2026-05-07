//! E32B-029: WebSocket control-frame and text UTF-8 validation parity.
//!
//! Process-survival regression: a malformed WS connection A
//! (invalid UTF-8 text frame) must not affect sibling WS connection B.
//! Both connections share a server with request limit = 2; A must observe
//! close-code 1007 and B must observe its frame echoed back + a normal
//! close (1000).

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
        "taida_e32b029_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("create project dir");
    fs::write(dir.join("main.td"), source).expect("write main.td");
    fs::write(dir.join("packages.tdm"), "// E32B-029 test project\n").expect("write packages.tdm");

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
            let js_path = unique_path("taida_e32b029", label, "mjs");
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
            let bin_path = unique_path("taida_e32b029", label, "bin");
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

fn ws_source(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe, wsUpgrade, wsSend, wsReceive, wsClose)

handler req writer =
  upgrade <= wsUpgrade(req, writer)
  upgrade ]=> accepted
  ws <= accepted.ws
  msg <= wsReceive(ws)
  msg ]=> received
  wsSend(ws, received.data)
  wsClose(ws)
=> :Unit

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
    )
}

fn ws_source_two_request(port: u16) -> String {
    format!(
        r#">>> taida-lang/net => @(httpServe, wsUpgrade, wsSend, wsReceive, wsClose)

handler req writer =
  upgrade <= wsUpgrade(req, writer)
  upgrade ]=> accepted
  ws <= accepted.ws
  msg <= wsReceive(ws)
  msg ]=> received
  wsSend(ws, received.data)
  wsClose(ws)
=> :Unit

asyncResult <= httpServe({port}, handler, 2)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
    )
}

fn connect_ws(port: u16) -> Option<TcpStream> {
    let request = format!(
        "GET /ws HTTP/1.1\r\n\
         Host: localhost:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );

    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(50));
        let mut stream = match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
        if stream.write_all(request.as_bytes()).is_err() {
            continue;
        }

        let mut response = Vec::new();
        let mut one = [0u8; 1];
        while !response.ends_with(b"\r\n\r\n") && response.len() < 4096 {
            match stream.read(&mut one) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&one[..n]),
                Err(_) => break,
            }
        }
        if String::from_utf8_lossy(&response).contains("101 Switching Protocols") {
            return Some(stream);
        }
    }
    None
}

fn masked_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask_key = [0x37, 0xfa, 0x21, 0x3d];
    let mut frame = Vec::new();
    frame.push(0x80 | opcode);
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else if payload.len() <= 65_535 {
        frame.push(0x80 | 126);
        frame.push((payload.len() >> 8) as u8);
        frame.push((payload.len() & 0xFF) as u8);
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask_key);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(*byte ^ mask_key[i % 4]);
    }
    frame
}

fn read_ws_bytes(stream: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if find_close_code(&out).is_some() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    out
}

fn find_close_code(bytes: &[u8]) -> Option<u16> {
    let mut pos = 0;
    while pos + 2 <= bytes.len() {
        let opcode = bytes[pos] & 0x0F;
        let len7 = bytes[pos + 1] & 0x7F;
        let mut header_len = 2usize;
        let payload_len = if len7 < 126 {
            len7 as usize
        } else if len7 == 126 {
            if pos + 4 > bytes.len() {
                return None;
            }
            header_len = 4;
            ((bytes[pos + 2] as usize) << 8) | bytes[pos + 3] as usize
        } else {
            if pos + 10 > bytes.len() {
                return None;
            }
            header_len = 10;
            let mut len = 0usize;
            for byte in &bytes[pos + 2..pos + 10] {
                len = (len << 8) | (*byte as usize);
            }
            len
        };
        let payload_start = pos + header_len;
        let payload_end = payload_start.saturating_add(payload_len);
        if payload_end > bytes.len() {
            return None;
        }
        if opcode == 0x8 && payload_len >= 2 {
            return Some(((bytes[payload_start] as u16) << 8) | bytes[payload_start + 1] as u16);
        }
        pos = payload_end;
    }
    None
}

fn run_reject_case(backend: &str, label: &str, opcode: u8, payload: &[u8], expected_code: u16) {
    run_reject_frame_case(
        backend,
        label,
        &masked_frame(opcode, payload),
        expected_code,
    );
}

/// Lower-level entry point used by E32B-068 to drive raw frames that the
/// `masked_frame` helper cannot express (RSV1=1, MASK=0, unknown opcode).
/// Identical lifecycle to `run_reject_case` — connect, send the frame,
/// observe the close frame, assert the RFC 6455 close code.
fn run_reject_frame_case(backend: &str, label: &str, frame: &[u8], expected_code: u16) {
    let port = common::find_free_loopback_port();
    let dir = setup_net_project(&ws_source(port), &format!("{}_{}", backend, label));
    let (mut child, artifact) = spawn_backend(&dir, backend, &format!("{}_{}", backend, label));

    let mut stream = connect_ws(port).unwrap_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        panic!("{} {}: WebSocket upgrade did not complete", backend, label);
    });
    stream.write_all(frame).expect("write websocket frame");
    let response = read_ws_bytes(&mut stream);

    let _ = child.kill();
    let _ = child.wait();
    if let Some(path) = artifact {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_dir_all(&dir);

    let close_code = find_close_code(&response).unwrap_or_else(|| {
        panic!(
            "{} {}: expected close code {}, got raw bytes {:?}",
            backend, label, expected_code, response
        )
    });
    assert_eq!(
        close_code, expected_code,
        "{} {}: close code mismatch, raw bytes {:?}",
        backend, label, response
    );
}

/// Build a client→server frame with the RSV1 reserved bit set. RFC 6455
/// §5.2 reserves RSV1/2/3 for extensions and requires endpoints to fail
/// the connection (1002 protocol error) when an unnegotiated reserved bit
/// is observed. The frame is otherwise a well-formed masked text frame
/// so that the close cause is the reserved bit, not a downstream parse
/// error.
fn rsv1_text_frame(payload: &[u8]) -> Vec<u8> {
    let mask_key = [0x37, 0xfa, 0x21, 0x3d];
    let mut frame = Vec::new();
    // FIN(0x80) | RSV1(0x40) | opcode(0x1=text)
    frame.push(0x80 | 0x40 | 0x1);
    // Masked payload (MASK=0x80) — short length only (≤125).
    frame.push(0x80 | payload.len() as u8);
    frame.extend_from_slice(&mask_key);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(*byte ^ mask_key[i % 4]);
    }
    frame
}

/// Build an UNMASKED client→server text frame. RFC 6455 §5.1 requires every
/// client-originated frame to be masked; servers MUST close with 1002 when
/// they receive an unmasked frame from a client.
fn unmasked_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0x80 | 0x1); // FIN | text
    frame.push(payload.len() as u8); // MASK bit cleared
    frame.extend_from_slice(payload);
    frame
}

/// Build a masked frame with an unassigned non-control opcode (0x3-0x7
/// reserved by RFC 6455 §5.2). Receiving an unknown opcode MUST fail the
/// connection with 1002 (protocol error).
fn unknown_opcode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    masked_frame(opcode, payload)
}

#[test]
fn e32b_029_websocket_validation_three_backend() {
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

    let ping_126 = vec![b'p'; 126];
    let pong_126 = vec![b'q'; 126];
    let mut close_126 = vec![0x03, 0xE8];
    close_126.extend(std::iter::repeat(b'c').take(124));

    for backend in backends {
        run_reject_case(backend, "ping_126", 0x9, &ping_126, 1002);
        run_reject_case(backend, "pong_126", 0xA, &pong_126, 1002);
        run_reject_case(backend, "close_126", 0x8, &close_126, 1002);
        run_reject_case(backend, "invalid_text_utf8", 0x1, &[0xFF], 1007);
    }
}

/// E32B-068 (RFC 6455 §5.1 / §5.2 protocol-violation closure):
///
/// The existing `e32b_029_websocket_validation_three_backend` covers control
/// payload >125 and invalid-UTF-8 text. Three additional protocol violations
/// must close with 1002 on every backend so that future runtime changes
/// cannot silently accept frames that the spec marks as connection-fatal:
///   - RSV1 = 1 with no negotiated extension (RFC 6455 §5.2)
///   - MASK = 0 on a client→server frame (RFC 6455 §5.1)
///   - unassigned non-control opcode 0x3 (RFC 6455 §5.2)
#[test]
fn e32b_068_websocket_protocol_violation_three_backend() {
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
        run_reject_frame_case(backend, "rsv1_set", &rsv1_text_frame(b"hi"), 1002);
        run_reject_frame_case(backend, "unmasked", &unmasked_text_frame(b"hi"), 1002);
        run_reject_frame_case(
            backend,
            "unknown_opcode_3",
            &unknown_opcode_frame(0x3, b"hi"),
            1002,
        );
    }
}

/// Locate the first text-frame (opcode 0x1) payload in a stream of unmasked
/// server-to-client frames. Returns `None` if no text frame is found.
///
/// Server-to-client frames are unmasked (RFC 6455 §5.1). Each frame begins
/// with the FIN+opcode byte and a payload-length byte (no mask key on this
/// direction). The helper walks frames sequentially until it lands on a
/// text frame and returns that frame's payload.
fn find_text_payload(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos + 2 <= bytes.len() {
        let opcode = bytes[pos] & 0x0F;
        let len7 = bytes[pos + 1] & 0x7F;
        let mut header_len = 2usize;
        let payload_len = if len7 < 126 {
            len7 as usize
        } else if len7 == 126 {
            if pos + 4 > bytes.len() {
                return None;
            }
            header_len = 4;
            ((bytes[pos + 2] as usize) << 8) | bytes[pos + 3] as usize
        } else {
            if pos + 10 > bytes.len() {
                return None;
            }
            header_len = 10;
            let mut len = 0usize;
            for byte in &bytes[pos + 2..pos + 10] {
                len = (len << 8) | (*byte as usize);
            }
            len
        };
        let payload_start = pos + header_len;
        let payload_end = payload_start.saturating_add(payload_len);
        if payload_end > bytes.len() {
            return None;
        }
        if opcode == 0x1 {
            return Some(bytes[payload_start..payload_end].to_vec());
        }
        pos = payload_end;
    }
    None
}

/// Process-survival regression: connection A sends a malformed
/// UTF-8 text frame and must observe close-code 1007. Sibling connection
/// B sends a valid text frame `hello` afterwards and must observe its
/// frame echoed back followed by a normal close (1000). Both run
/// against the same backend process (request limit = 2) so the test
/// demonstrates that A's malformed input did not impact B's echo path.
///
/// The workers are intentionally sequential: A finishes its upgrade +
/// frame round-trip first, then B opens a fresh WS handshake. This test
/// pins process-wide survival after malformed input; it does not claim
/// overlap between sibling connections.
#[test]
fn ws_process_survival_three_backend() {
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
        let dir = setup_net_project(&ws_source_two_request(port), &format!("conc_{}", backend));
        let (mut child, artifact) = spawn_backend(&dir, backend, &format!("conc_{}", backend));

        // Connection A: malformed UTF-8 text frame → expect close 1007.
        // `connect_ws` already polls the listener so we wait for the
        // backend to bind without an external barrier.
        let response_a = (|| -> Option<Vec<u8>> {
            let mut stream = connect_ws(port)?;
            stream
                .write_all(&masked_frame(0x1, &[0xFF]))
                .expect("write malformed text frame");
            Some(read_ws_bytes(&mut stream))
        })();

        // Connection B: valid text frame → expect echo + close 1000.
        // Runs after A has fully closed, so the server's accept loop
        // is guaranteed to have moved past A's slot.
        let response_b = (|| -> Option<Vec<u8>> {
            let mut stream = connect_ws(port)?;
            stream
                .write_all(&masked_frame(0x1, b"hello"))
                .expect("write valid text frame");
            Some(read_ws_bytes(&mut stream))
        })();

        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = artifact {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);

        let response_a = response_a
            .unwrap_or_else(|| panic!("{}: connection A could not complete WS upgrade", backend));
        let response_b = response_b
            .unwrap_or_else(|| panic!("{}: connection B could not complete WS upgrade", backend));

        let close_a = find_close_code(&response_a).unwrap_or_else(|| {
            panic!(
                "{}: connection A must observe a close frame, got: {:?}",
                backend, response_a
            )
        });
        assert_eq!(
            close_a, 1007,
            "{}: A must close with 1007 (invalid UTF-8 text frame), got close-code {} (raw {:?})",
            backend, close_a, response_a
        );

        let echo = find_text_payload(&response_b).unwrap_or_else(|| {
            panic!(
                "{}: B must observe an echoed text frame, got: {:?}",
                backend, response_b
            )
        });
        assert_eq!(
            echo, b"hello",
            "{}: B's echoed payload must be exactly `hello`, got {:?}",
            backend, echo
        );
        let close_b = find_close_code(&response_b).unwrap_or_else(|| {
            panic!(
                "{}: B must also observe a close frame, got: {:?}",
                backend, response_b
            )
        });
        assert_eq!(
            close_b, 1000,
            "{}: B must close with 1000 (normal closure), got close-code {} (raw {:?})",
            backend, close_b, response_b
        );
    }
}
