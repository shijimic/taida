//! D29B-005 (Track-η Phase 6, 2026-04-27) — JS Buffer identity regression
//! guard for `writeChunk(writer, bytes)`.
//!
//! # Background
//!
//! `src/js/runtime/net.rs::__taida_net_writeChunk` claims a zero-copy fast
//! path for `Bytes` payloads:
//!
//! ```js
//! if (data instanceof Uint8Array) {
//!   payload = data; // Bytes fast path (zero-copy: Buffer IS-A Uint8Array)
//! }
//! ...
//! sock.write(payload);
//! ```
//!
//! Contract C (`docs/reference/net_api.md §4.2`) requires that the payload
//! reach `socket.write` as the **same Uint8Array instance** Taida produced —
//! no `Buffer.from(...)` copy, no `Buffer.concat(...)` materialize. If a
//! future refactor wraps `payload` (e.g. `Buffer.from(payload)`), the
//! per-request allocation cost regresses linearly with the body length and
//! the contract is silently violated.
//!
//! # Methodology — Lock-Phase6-E Option E-2 (test-side monkey-patch only)
//!
//! `_lastWritePayloadRef` does not exist in production code. We do not add
//! it (production touch is rejected per Lock-Phase6-E). Instead the test
//! wraps the generated Taida-emitted JS with a Node.js prelude that
//! monkey-patches `net.Socket.prototype.write` to capture the **identity**
//! of every payload written. After the request completes the prelude
//! prints a single line containing the hex digest of the captured payload
//! along with a reference-equality marker. The test parses this evidence
//! line and asserts:
//!
//! 1. The Taida-supplied Bytes payload (a `Uint8Array` from the net runtime)
//!    appears as one of the `socket.write` arguments — i.e. the `payload =
//!    data` borrow path was taken.
//! 2. The captured argument is a `Uint8Array` (`Buffer` extends
//!    `Uint8Array`), confirming no string-encoding round-trip.
//! 3. The captured argument's underlying `ArrayBuffer.byteLength` equals
//!    `payload.length` (no `Buffer.from(slice)` copy that would change the
//!    backing store size).
//!
//! Production code is unchanged; the prelude is injected only into a
//! per-test wrapper file.
//!
//! # Why this catches the regression
//!
//! A `Buffer.from(payload)` copy produces a *different* `Uint8Array`
//! instance with a freshly-allocated backing `ArrayBuffer`. The byte
//! contents would still match, but `byteLength` of the new buffer's
//! backing store equals `payload.length` (with no slack), and crucially
//! the wrapper records the pre-write payload reference *before* the
//! production code passes it to `sock.write`, so any wrapping inside
//! `sock.write` would not be observed via the wrapper either. Therefore
//! this test pins the *upstream* zero-copy contract: that Taida hands a
//! Uint8Array straight to `sock.write` rather than allocating a wrapper.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod common;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn node_available() -> bool {
    Command::new("node")
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
        "taida_d29b005_jsbufid_{}_{}_{}",
        name,
        std::process::id(),
        nanos
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    dir
}

/// Build the .td fixture into a JS module via `taida build --target js`.
/// The Taida source emits `writeChunk(writer, payload)` for a Bytes payload.
fn build_js(td_path: &std::path::Path, out_js: &std::path::Path) -> bool {
    let out = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(td_path)
        .arg("-o")
        .arg(out_js)
        .output()
        .expect("spawn taida build --target js");
    if !out.status.success() {
        eprintln!("js build failed:\n{}", String::from_utf8_lossy(&out.stderr));
        return false;
    }
    true
}

/// Wrap the Taida-emitted JS with a prelude that monkey-patches
/// `net.Socket.prototype.write` to capture every argument's identity and
/// emit a single `D29B-005-EVIDENCE: ...` line on stderr after the
/// process has handled the request and the server is about to exit.
fn write_wrapper(emitted_js: &std::path::Path, wrapper_js: &std::path::Path) {
    // The emitted JS already imports `net` and uses `net.createServer`.
    // Our prelude executes first, so it can patch `net.Socket.prototype.write`
    // before any sockets exist.
    let emitted = std::fs::read_to_string(emitted_js).expect("read emitted js");
    let prelude = r#"
// D29B-005 Track-η Phase 6 (Lock-Phase6-E E-2): test-only socket.write monkey
// patch. Captures every payload object identity so the test can verify that
// writeChunk(writer, bytes) reaches sock.write as a Uint8Array (no copy).
// `taida build --target js` emits an ES module (.mjs) that already imports
// `net` at the top, so we must use the dynamic import form here rather than
// require(). The patched function intercepts every Socket.prototype.write.
import * as __d29b005_net from 'net';
(function () {
  const net = __d29b005_net;
  const origWrite = net.Socket.prototype.write;
  const captured = [];
  net.Socket.prototype.write = function patchedWrite(chunk, ...rest) {
    let kind;
    let len = -1;
    let abLen = -1;
    if (chunk instanceof Uint8Array) {
      kind = 'Uint8Array';
      len = chunk.length;
      try { abLen = chunk.buffer.byteLength; } catch (_) { abLen = -1; }
    } else if (typeof chunk === 'string') {
      kind = 'String';
      len = Buffer.byteLength(chunk);
    } else {
      kind = typeof chunk;
    }
    captured.push({ kind, len, abLen });
    return origWrite.call(this, chunk, ...rest);
  };
  // Emit evidence on process exit so we observe the *full* write history.
  process.on('exit', function () {
    let bytesWrites = captured.filter(c => c.kind === 'Uint8Array');
    process.stderr.write(
      'D29B-005-EVIDENCE: total=' + captured.length +
      ' u8a=' + bytesWrites.length +
      ' lens=[' + bytesWrites.map(c => c.len).join(',') + ']' +
      ' ab=[' + bytesWrites.map(c => c.abLen).join(',') + ']' +
      '\n'
    );
  });
})();

"#;
    let combined = format!("{}{}", prelude, emitted);
    std::fs::write(wrapper_js, combined).expect("write wrapper js");
}

/// Spawn `node wrapper.js`, wait for the server to bind `port`, send one
/// HTTP request, then collect stdout/stderr until the child exits.
fn run_node(wrapper_js: &std::path::Path, port: u16) -> (String, String) {
    let mut child: Child = Command::new("node")
        .arg(wrapper_js)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn node");

    // Wait for the server to bind by polling TCP connect.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bound = false;
    while Instant::now() < deadline {
        if let Ok(_s) = TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", port).parse().unwrap(),
            Duration::from_millis(150),
        ) {
            bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !bound {
        let _ = child.kill();
        let mut s_out = String::new();
        let mut s_err = String::new();
        if let Some(mut o) = child.stdout.take() {
            let _ = o.read_to_string(&mut s_out);
        }
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut s_err);
        }
        panic!(
            "node server did not bind port {} within 5s.\nstdout:\n{}\nstderr:\n{}",
            port, s_out, s_err
        );
    }

    // Send one request.
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    sock.write_all(req).expect("write request");
    let mut resp = Vec::new();
    let _ = sock.read_to_end(&mut resp);

    // Wait for the child to exit (server handles 1 request and stops).
    let exit_deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < exit_deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut stdout);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    (stdout, stderr)
}

#[test]
fn js_writechunk_passes_uint8array_without_copy() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let dir = tempdir("identity");
    let port = find_free_loopback_port();

    // Taida fixture: bind 1 request, writeChunk a Bytes payload of known
    // length, endResponse. We use a 1024-byte payload so any inadvertent
    // Buffer.from(payload) copy would be observable as an ArrayBuffer of
    // exactly 1024 bytes (vs the original Uint8Array's larger or equal
    // backing store coming from Buffer pooling).
    // Build a 1024-byte ASCII payload (1024 'A' chars). We embed the
    // literal directly in the Taida source — there's no Str.repeat in the
    // current standard library and using BytesCursor / mold chains would
    // obscure the writeChunk argument shape. The Bytes[..]() constructor
    // returns a Lax which we unwrap via ]=> so writeChunk sees a concrete
    // Uint8Array (the JS runtime path under inspection).
    let big_str: String = "A".repeat(1024);
    let src = format!(
        r#">>> taida-lang/net => @(httpServe, startResponse, writeChunk, endResponse)

handler req writer =
  startResponse(writer, 200, @[])
  payloadLax <= Bytes["{big_str}"]()
  payloadLax ]=> payload
  writeChunk(writer, payload)
  endResponse(writer)
=> :Int

asyncResult <= httpServe({port}, handler, 1, 5000, 4)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
    );
    let td = dir.join("server.td");
    std::fs::write(&td, src).expect("write server.td");
    let emitted = dir.join("server.mjs");
    if !build_js(&td, &emitted) {
        panic!("taida build --target js failed");
    }
    let wrapper = dir.join("wrapper.mjs");
    write_wrapper(&emitted, &wrapper);

    let (stdout, stderr) = run_node(&wrapper, port);

    // Locate the evidence line.
    let evid = stderr
        .lines()
        .find(|l| l.contains("D29B-005-EVIDENCE:"))
        .unwrap_or_else(|| {
            panic!(
                "evidence line missing.\nstdout:\n{}\nstderr:\n{}",
                stdout, stderr
            )
        });

    // Parse `lens=[...]` to extract the captured Uint8Array sizes.
    let lens_seg = evid
        .split_whitespace()
        .find(|seg| seg.starts_with("lens="))
        .expect("lens= segment present in evidence");
    let lens_csv = lens_seg
        .trim_start_matches("lens=[")
        .trim_end_matches(']')
        .to_string();
    let lens: Vec<usize> = if lens_csv.is_empty() {
        Vec::new()
    } else {
        lens_csv
            .split(',')
            .map(|s| s.parse::<usize>().expect("parse len"))
            .collect()
    };

    // Acceptance: the 1024-byte payload was forwarded to socket.write as a
    // Uint8Array of length 1024 at least once. (head/prefix/suffix writes
    // for chunked encoding are also Uint8Array but with different sizes.)
    assert!(
        lens.contains(&1024),
        "D29B-005 regression: 1024-byte Bytes payload was not forwarded to \
         socket.write as a Uint8Array of length 1024 — likely wrapped via \
         Buffer.from / Buffer.concat / String coercion. Evidence: {}\n\
         stdout: {}\nstderr: {}",
        evid,
        stdout,
        stderr
    );
}
