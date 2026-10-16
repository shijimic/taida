//! Regressions for the HTTP / socket runtime: HTTP/1.1 request-head parsing,
//! response framing, the socket handle table, the HTTP client's chunked
//! decoding and the HTTP/1.1 server's concurrency, compared between the
//! interpreter and the native backend; plus HTTP/2 flow control and trailers
//! in the interpreter.
//!
//! The net and socket APIs are rejected at build time on every WASM profile,
//! so the comparisons cover the interpreter and the native backend only.

mod common;

use common::{taida_bin, unique_temp_dir};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ── helpers ─────────────────────────────────────────────────────

/// A server process that is killed when the test ends (or panics).
struct Server {
    child: Child,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_program(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write program");
    path
}

#[cfg(feature = "native")]
fn build_native(td: &Path) -> PathBuf {
    let bin = td.with_extension("bin");
    let out = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run taida build native");
    assert!(
        out.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Commands that run `td` on every backend under test.
fn backend_commands(td: &Path) -> Vec<(&'static str, Command)> {
    let mut interp = Command::new(taida_bin());
    interp.arg(td);
    #[allow(unused_mut)]
    let mut commands = vec![("interp", interp)];
    #[cfg(feature = "native")]
    {
        commands.push(("native", Command::new(build_native(td))));
    }
    commands
}

/// Runs a command to completion with a deadline.
fn run_with_timeout(mut cmd: Command, secs: u64) -> Output {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn program");
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if child.try_wait().expect("wait program").is_some() {
            return child.wait_with_output().expect("collect output");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let out = child.wait_with_output().expect("collect output");
            panic!(
                "program did not finish within {secs}s; stdout={:?} stderr={:?}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stdout_of(cmd: Command) -> String {
    let out = run_with_timeout(cmd, 60);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Starts a server that listens on port 0 and announces the bound port.
fn start_server(mut cmd: Command) -> (Server, u16) {
    let mut child = cmd
        .env("TAIDA_NET_ANNOUNCE_PORT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    let stdout = child.stdout.take().expect("server stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        if let Some(Ok(line)) = lines.next() {
            let _ = tx.send(line);
        }
        // Keep draining so the server never blocks on a full pipe.
        for _ in lines {}
    });
    let server = Server { child };
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("server did not announce its port");
    let port = line
        .rsplit(':')
        .next()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or_else(|| panic!("unexpected announce line: {line:?}"));
    (server, port)
}

/// Sends `request` on a new connection and reads until EOF or `wait`.
fn exchange(port: u16, request: &[u8], wait: Duration) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(wait)).expect("read timeout");
    stream.write_all(request).expect("send request");
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Renders `s` as a Taida string literal.
fn taida_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f || (c as u32) >= 0x80 => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── HTTP/1.1 request head parsing ───────────────────────────────

const PARSE_PRELUDE: &str = r#">>> taida-lang/net => @(httpParseRequestHead)

Span = @(start: Int, len: Int)
Hdr = @(name: Span, value: Span)

b2s x: Bool =
  | x |> "T"
  | _ |> "F"
=> :Str

sp s: Span =
  s.start.toString() + "+" + s.len.toString()
=> :Str

hdrStr acc: Str h: Hdr =
  acc + "[" + sp(h.name) + ":" + sp(h.value) + "]"
=> :Str

describe s: Str =
  |== err: Error =
    "ERR " + err.message
  => :Str

  bytesLax <= Bytes[s]()
  bytesLax >=> bytes
  result <= httpParseRequestHead(bytes)
  result >=> r
  hs <= Fold[r.headers, "", hdrStr]()
  "c=" + b2s(r.complete) + " n=" + r.consumed.toString() + " m=" + sp(r.method) + " p=" + sp(r.path) + " q=" + sp(r.query) + " v=" + r.version.major.toString() + "." + r.version.minor.toString() + " cl=" + r.contentLength.toString() + " ch=" + b2s(r.chunked) + " h=" + hs
=> :Str

show label: Str s: Str =
  stdout(label + " " + describe(s))
=> :Int

"#;

fn parse_cases() -> Vec<(&'static str, String)> {
    let mut cases: Vec<(&'static str, String)> = vec![
        (
            "ok",
            "GET /a?b=1 HTTP/1.1\r\nHost: x\r\nX-Y:  z  \r\n\r\nBODY".into(),
        ),
        (
            "sp-before-colon",
            "POST / HTTP/1.1\r\nContent-Length : 5\r\n\r\n".into(),
        ),
        (
            "te-sp-before-colon",
            "POST / HTTP/1.1\r\nTransfer-Encoding : chunked\r\n\r\n".into(),
        ),
        ("empty-name", "GET / HTTP/1.1\r\n: x\r\n\r\n".into()),
        (
            "space-in-name",
            "GET / HTTP/1.1\r\nBad Header: x\r\n\r\n".into(),
        ),
        ("tab-before-name", "GET / HTTP/1.1\r\n\tX: v\r\n\r\n".into()),
        (
            "high-byte-name",
            "GET / HTTP/1.1\r\nX\u{e9}: a\r\n\r\n".into(),
        ),
        (
            "space-in-path",
            "GET /a b HTTP/1.1\r\nHost: x\r\n\r\n".into(),
        ),
        (
            "ctl-in-path",
            "GET /a\u{1}b HTTP/1.1\r\nHost: x\r\n\r\n".into(),
        ),
        ("bad-method", "G(T / HTTP/1.1\r\nHost: x\r\n\r\n".into()),
        ("two-spaces", "GET  / HTTP/1.1\r\nHost: x\r\n\r\n".into()),
        ("obs-fold", "GET / HTTP/1.1\r\nX: a\r\n b\r\n\r\n".into()),
        (
            "nul-in-value",
            "GET / HTTP/1.1\r\nX: a\u{0}b\r\n\r\n".into(),
        ),
        ("cr-in-value", "GET / HTTP/1.1\r\nX: a\rb\r\n\r\n".into()),
        ("lower-http", "GET / http/1.1\r\n\r\n".into()),
        ("http-2", "GET / HTTP/2.0\r\n\r\n".into()),
        (
            "trailing-sp-after-version",
            "GET / HTTP/1.1 \r\n\r\n".into(),
        ),
        ("no-headers-1.0", "GET / HTTP/1.0\r\n\r\n".into()),
        ("no-headers-1.1", "GET /y HTTP/1.1\r\n\r\n".into()),
        (
            "leading-empty-lines",
            "\r\n\r\nGET / HTTP/1.1\r\nHost: a\r\n\r\n".into(),
        ),
        ("bare-lf", "GET / HTTP/1.1\nHost: x\n\n".into()),
        (
            "empty-values",
            "GET / HTTP/1.1\r\nX:\r\nY:   \r\n\r\n".into(),
        ),
        (
            "cl-tab",
            "POST / HTTP/1.1\r\nContent-Length:\t5\t\r\n\r\n".into(),
        ),
        (
            "cl-plus",
            "POST / HTTP/1.1\r\nContent-Length: +5\r\n\r\n".into(),
        ),
        (
            "cl-nbsp",
            "POST / HTTP/1.1\r\nContent-Length: 5\u{a0}\r\n\r\n".into(),
        ),
        (
            "cl-duplicate",
            "POST / HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n".into(),
        ),
        (
            "te-chunked",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".into(),
        ),
        (
            "te-gzip",
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n".into(),
        ),
        (
            "te-gzip-chunked",
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n".into(),
        ),
        (
            "te-chunked-gzip",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked, gzip\r\n\r\n".into(),
        ),
        (
            "te-two-lines",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: gzip\r\n\r\n"
                .into(),
        ),
        (
            "te-identity-cl",
            "POST / HTTP/1.1\r\nTransfer-Encoding: identity\r\nContent-Length: 5\r\n\r\n".into(),
        ),
        (
            "te-gzip-partial",
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n".into(),
        ),
        (
            "te-cl",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n".into(),
        ),
        ("partial-method", "GE".into()),
        ("partial-target", "GET /abc".into()),
        ("partial-version", "GET /abc HTTP/1.".into()),
        ("partial-header", "GET / HTTP/1.1\r\nHost: exa".into()),
        (
            "partial-after-header",
            "GET / HTTP/1.1\r\nHost: exa\r\nX-A: 1\r\n".into(),
        ),
        ("method-16", "ABCDEFGHIJKLMNOP / HTTP/1.1\r\n\r\n".into()),
        ("method-17", "ABCDEFGHIJKLMNOPQ / HTTP/1.1\r\n\r\n".into()),
        (
            "path-2048",
            format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(2047)),
        ),
        (
            "path-2049",
            format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(2048)),
        ),
        (
            "host-256",
            format!("GET / HTTP/1.1\r\nHost: {}\r\n\r\n", "h".repeat(256)),
        ),
        (
            "host-257",
            format!("GET / HTTP/1.1\r\nhOsT: {}\r\n\r\n", "h".repeat(257)),
        ),
    ];
    let headers = |n: usize| -> String { (0..n).map(|i| format!("X{i}: v\r\n")).collect() };
    cases.push((
        "headers-64",
        format!("GET / HTTP/1.1\r\n{}\r\n", headers(64)),
    ));
    cases.push((
        "headers-65",
        format!("GET / HTTP/1.1\r\n{}\r\n", headers(65)),
    ));
    cases
}

/// The native parser follows the interpreter's (httparse) rules: the same
/// heads are accepted or rejected, with the same spans and messages.
#[test]
fn request_head_parse_matches_the_interpreter() {
    let dir = unique_temp_dir("f65_net_parse");
    let mut source = String::from(PARSE_PRELUDE);
    for (label, input) in parse_cases() {
        source.push_str(&format!(
            "show({}, {})\n",
            taida_str(label),
            taida_str(&input)
        ));
    }
    let td = write_program(&dir, "parse.td", &source);

    let mut outputs = Vec::new();
    for (label, cmd) in backend_commands(&td) {
        outputs.push((label, stdout_of(cmd)));
    }
    let interp = &outputs[0].1;
    let line = |label: &str| -> &str {
        interp
            .lines()
            .find(|l| l.split(' ').next() == Some(label))
            .unwrap_or_else(|| panic!("no output line for {label}: {interp}"))
    };
    assert_eq!(
        line("sp-before-colon"),
        "sp-before-colon ERR Malformed HTTP request: invalid header name"
    );
    assert_eq!(
        line("te-sp-before-colon"),
        "te-sp-before-colon ERR Malformed HTTP request: invalid header name"
    );
    assert_eq!(
        line("no-headers-1.0"),
        "no-headers-1.0 c=T n=18 m=0+3 p=4+1 q=0+0 v=1.0 cl=0 ch=F h="
    );
    assert_eq!(
        line("leading-empty-lines"),
        "leading-empty-lines c=T n=31 m=4+3 p=8+1 q=0+0 v=1.1 cl=0 ch=F h=[20+4:26+1]"
    );
    assert_eq!(
        line("te-gzip"),
        "te-gzip ERR Malformed HTTP request: Transfer-Encoding must end with chunked"
    );
    assert_eq!(
        line("te-chunked-gzip"),
        "te-chunked-gzip ERR Malformed HTTP request: Transfer-Encoding must end with chunked"
    );
    assert!(line("te-gzip-chunked").contains(" ch=T "));
    assert!(line("te-gzip-partial").starts_with("te-gzip-partial c=F "));
    assert_eq!(
        line("cl-nbsp"),
        "cl-nbsp ERR Malformed HTTP request: invalid Content-Length value"
    );
    assert_eq!(
        line("method-17"),
        "method-17 ERR Malformed HTTP request: method exceeds wire-byte limit"
    );
    assert_eq!(
        line("path-2049"),
        "path-2049 ERR Malformed HTTP request: path exceeds wire-byte limit"
    );
    assert_eq!(
        line("host-257"),
        "host-257 ERR Malformed HTTP request: authority exceeds wire-byte limit"
    );
    assert_eq!(
        line("partial-header"),
        "partial-header c=F n=0 m=0+3 p=4+1 q=0+0 v=1.1 cl=0 ch=F h="
    );
    assert_eq!(
        line("headers-65"),
        "headers-65 ERR Malformed HTTP request: too many headers"
    );
    for (label, out) in &outputs[1..] {
        assert_eq!(out, interp, "{label} differs from the interpreter");
    }
    let _ = std::fs::remove_dir_all(dir);
}

// ── HTTP/1.1 server ─────────────────────────────────────────────

const ECHO_SERVER: &str = r#">>> taida-lang/net => @(httpServe)

handler req: Request =
  m <= StrOf[req.method, req.raw]()
  p <= StrOf[req.path, req.raw]()
  b <= StrOf[req.body, req.raw]()
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "m=" + m + " p=" + p + " body=" + b)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(0, handler, 100, 3000) >=> result
result >=> summary
stdout("requests: " + summary.requests.toString())
"#;

/// A body that holds a second request is never run as a request when the
/// framing headers are malformed or undecidable, and a head without header
/// lines is accepted.
#[test]
fn server_framing_rejects_smuggled_requests() {
    let dir = unique_temp_dir("f65_net_smuggle");
    let td = write_program(&dir, "echo.td", ECHO_SERVER);
    let inner = "GET /smuggled HTTP/1.1\r\nHost: a\r\n\r\n";
    let smuggling = [
        format!(
            "POST /a HTTP/1.1\r\nHost: a\r\nContent-Length : {}\r\n\r\n{inner}",
            inner.len()
        ),
        format!("POST /a HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: gzip\r\n\r\n{inner}"),
        format!(
            "POST /a HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked, gzip\r\n\r\n0\r\n\r\n{inner}"
        ),
    ];
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        for request in &smuggling {
            let response = text(&exchange(port, request.as_bytes(), Duration::from_secs(2)));
            assert!(
                response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
                "{label}: {request:?} -> {response:?}"
            );
            assert!(
                !response.contains("smuggled"),
                "{label}: {request:?} -> {response:?}"
            );
            assert_eq!(
                response.matches("HTTP/1.1 ").count(),
                1,
                "{label}: {response:?}"
            );
        }
        let response = text(&exchange(
            port,
            b"GET /y HTTP/1.0\r\n\r\n",
            Duration::from_secs(2),
        ));
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n") && response.ends_with("m=GET p=/y body="),
            "{label}: {response:?}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

const CONTENT_LENGTH_SERVER: &str = r#">>> taida-lang/net => @(httpServe)

handler req: Request =
  p <= StrOf[req.path, req.raw]()
  | p == "/short" |> @(status <= 200, headers <= @[@(name <= "Content-Length", value <= "1")], body <= "xyz")
  | p == "/dup" |> @(status <= 200, headers <= @[@(name <= "content-length", value <= "5"), @(name <= "x-b", value <= "b"), @(name <= "Content-Length", value <= "100")], body <= "0123456789")
  | p == "/head" |> @(status <= 200, headers <= @[@(name <= "Content-Length", value <= "1234")], body <= "")
  | _ |> @(status <= 200, headers <= @[@(name <= "x-a", value <= "1")], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(0, handler, 100, 3000) >=> result
result >=> summary
stdout("requests: " + summary.requests.toString())
"#;

/// The response is framed by the real body length whatever Content-Length
/// the handler supplies, so a keep-alive connection stays in sync. A HEAD
/// response keeps the handler's value (it describes the GET body).
#[test]
fn server_response_is_framed_by_the_body_length() {
    let dir = unique_temp_dir("f65_net_cl");
    let td = write_program(&dir, "cl.td", CONTENT_LENGTH_SERVER);
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        let response = text(&exchange(
            port,
            b"GET /short HTTP/1.1\r\nHost: a\r\n\r\nGET /ok HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n",
            Duration::from_secs(3),
        ));
        assert_eq!(
            response,
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nxyz\
             HTTP/1.1 200 OK\r\nx-a: 1\r\nContent-Length: 2\r\n\r\nok",
            "{label}"
        );
        let response = text(&exchange(
            port,
            b"GET /dup HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n",
            Duration::from_secs(3),
        ));
        assert_eq!(
            response, "HTTP/1.1 200 OK\r\ncontent-length: 10\r\nx-b: b\r\n\r\n0123456789",
            "{label}"
        );
        let response = text(&exchange(
            port,
            b"HEAD /head HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n",
            Duration::from_secs(3),
        ));
        assert_eq!(
            response, "HTTP/1.1 200 OK\r\nContent-Length: 1234\r\n\r\n",
            "{label}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn encode_response_is_framed_by_the_body_length() {
    let dir = unique_temp_dir("f65_net_encode");
    let td = write_program(
        &dir,
        "encode.td",
        r#">>> taida-lang/net => @(httpEncodeResponse)

show r: @(status: Int, headers: @[@(name: Str, value: Str)], body: Str) =
  httpEncodeResponse(r) >=> e
  stdout(Utf8Decode[e.bytes]().getOrDefault("?"))
=> :Int

show(@(status <= 200, headers <= @[@(name <= "Content-Length", value <= "5")], body <= "hi"))
show(@(status <= 200, headers <= @[@(name <= "content-length", value <= "1"), @(name <= "x-a", value <= "1"), @(name <= "Content-Length", value <= "9")], body <= "abc"))
show(@(status <= 204, headers <= @[@(name <= "Content-Length", value <= "5")], body <= ""))
"#,
    );
    let expected = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi\n\
                    HTTP/1.1 200 OK\r\ncontent-length: 3\r\nx-a: 1\r\n\r\nabc\n\
                    HTTP/1.1 204 No Content\r\n\r\n\n";
    for (label, cmd) in backend_commands(&td) {
        assert_eq!(stdout_of(cmd), expected, "{label}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Connections that stay open (a slow head, an idle keep-alive) do not keep
/// the server from answering other clients.
#[test]
fn server_answers_while_other_connections_stay_open() {
    let dir = unique_temp_dir("f65_net_slow");
    let td = write_program(
        &dir,
        "slow.td",
        r#">>> taida-lang/net => @(httpServe)

handler req: Request =
  @(status <= 200, headers <= @[@(name <= "x-a", value <= "1")], body <= "fast")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(0, handler, 1, 20000) >=> result
result >=> summary
stdout("requests: " + summary.requests.toString())
"#,
    );
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        let mut held = Vec::new();
        for _ in 0..24 {
            let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            s.write_all(b"GET /slow HTTP/1.1\r\nX-Pad: ")
                .expect("partial head");
            held.push(s);
        }
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        let response = text(&exchange(
            port,
            b"GET /fast HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n",
            Duration::from_secs(8),
        ));
        assert!(response.ends_with("fast"), "{label}: {response:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{label}: answered after {:?}",
            started.elapsed()
        );
        drop(held);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// maxConnections values above the 32-bit range mean "no practical limit",
/// as on the interpreter (2^32 used to become a limit of 0 connections).
#[test]
fn server_accepts_max_connections_above_int_range() {
    let dir = unique_temp_dir("f65_net_maxconn");
    let td = write_program(
        &dir,
        "maxconn.td",
        r#">>> taida-lang/net => @(httpServe)

handler req: Request =
  @(status <= 200, headers <= @[@(name <= "x-a", value <= "1")], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(0, handler, 1, 3000, 4294967296) >=> result
result >=> summary
stdout("requests: " + summary.requests.toString())
"#,
    );
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        let response = text(&exchange(
            port,
            b"GET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n",
            Duration::from_secs(5),
        ));
        assert!(response.ends_with("\r\n\r\nok"), "{label}: {response:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

const STREAMING_SERVER: &str = r#">>> taida-lang/net => @(httpServe, readBodyAll, startResponse, writeChunk, endResponse, wsUpgrade, wsClose)

handler req: Request writer: Writer =
  p <= StrOf[req.path, req.raw]()
  | p == "/ws" |> upgrade(req, writer)
  | _ |> echo(req, writer)
=> :Int

echo req: Request writer: Writer =
  body <= readBodyAll(req)
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain")])
  writeChunk(writer, "len=" + body.length().toString())
  endResponse(writer)
=> :Int

upgrade req: Request writer: Writer =
  up <= wsUpgrade(req, writer)
  | up.has_value |> accepted(up)
  | _ |> rejected(writer)
=> :Int

accepted up: Lax[@(ws: WsConn)] =
  up >=> u
  wsClose(u.ws)
=> :Int

rejected writer: Writer =
  startResponse(writer, 400, @[@(name <= "content-type", value <= "text/plain")])
  writeChunk(writer, "rejected")
  endResponse(writer)
=> :Int

httpServe(0, handler, 100, 3000) >=> result
result >=> summary
stdout("requests: " + summary.requests.toString())
"#;

/// Streaming chunk-size and trailer lines follow the same 1 MiB cap as the
/// eager path (the native reader used to stop at 4 KiB).
#[test]
fn streaming_body_accepts_long_chunk_lines() {
    let dir = unique_temp_dir("f65_net_chunkline");
    let td = write_program(&dir, "stream.td", STREAMING_SERVER);
    let head =
        "POST /c HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    let requests = [
        format!("{head}5;{}\r\nhello\r\n0\r\n\r\n", "a".repeat(5000)),
        format!("{head}5\r\nhello\r\n0\r\nX-T: {}\r\n\r\n", "a".repeat(5000)),
    ];
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        for request in &requests {
            let response = text(&exchange(port, request.as_bytes(), Duration::from_secs(5)));
            assert!(
                response.starts_with("HTTP/1.1 200 OK\r\n") && response.contains("len=5"),
                "{label}: {response:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// wsUpgrade reads whole header values and decodes the key with canonical
/// base64 only, like the interpreter.
#[test]
fn ws_upgrade_checks_whole_values_and_canonical_keys() {
    let dir = unique_temp_dir("f65_net_ws");
    let td = write_program(&dir, "ws.td", STREAMING_SERVER);
    let base = "GET /ws HTTP/1.1\r\nHost: a\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\n";
    let key = "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
    let cases = [
        (
            "long Connection value",
            format!(
                "{base}Connection: {}, Upgrade\r\n{key}\r\n",
                "x".repeat(300)
            ),
            true,
        ),
        (
            "'=' inside the key",
            format!(
                "{base}Connection: Upgrade\r\nSec-WebSocket-Key: AA=AAAAAAAAAAAAAAAAAAA==\r\n\r\n"
            ),
            false,
        ),
        (
            "non-zero trailing bits",
            format!(
                "{base}Connection: Upgrade\r\nSec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAB==\r\n\r\n"
            ),
            false,
        ),
        (
            "valid",
            format!("{base}Connection: Upgrade\r\n{key}\r\n"),
            true,
        ),
    ];
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        for (case, request, upgraded) in &cases {
            let response = text(&exchange(port, request.as_bytes(), Duration::from_secs(3)));
            if *upgraded {
                assert!(
                    response.starts_with("HTTP/1.1 101 Switching Protocols\r\n")
                        && response
                            .contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"),
                    "{label} {case}: {response:?}"
                );
            } else {
                assert!(
                    response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
                    "{label} {case}: {response:?}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A chunk-size line cut by the read timeout is not read as a whole line
/// (`10` split as `1` + `0` used to frame a 1-byte chunk).
#[test]
fn streaming_body_rejects_a_chunk_line_cut_by_the_timeout() {
    let dir = unique_temp_dir("f65_net_cutline");
    let td = write_program(
        &dir,
        "cut.td",
        &STREAMING_SERVER.replace(
            "httpServe(0, handler, 100, 3000)",
            "httpServe(0, handler, 100, 700)",
        ),
    );
    for (label, cmd) in backend_commands(&td) {
        let (_server, port) = start_server(cmd);
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        stream
            .write_all(
                b"POST /c HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1",
            )
            .expect("send head");
        std::thread::sleep(Duration::from_millis(1200));
        // The chunk is 0x10 bytes. Read as `1` + `0`, its data would frame
        // a complete (wrong) body: "0" + "abc".
        let _ = stream.write_all(b"0\r\n3\r\nabc\r\n0\r\n\r\nXYZ\r\n0\r\n\r\n");
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response = text(&response);
        assert!(
            !response.contains("200 OK") && !response.contains("len="),
            "{label}: {response:?}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

// ── sockets ─────────────────────────────────────────────────────

/// The socket API only accepts handles it issued; a stray Int such as `1`
/// (stdout on the native backend) is an unknown handle on every backend.
#[test]
fn socket_api_rejects_handles_it_did_not_issue() {
    let dir = unique_temp_dir("f65_net_sock");
    let td = write_program(
        &dir,
        "sock.td",
        r#">>> taida-lang/os => @(socketClose, udpClose, listenerClose, socketSend, socketSendAll, socketRecv, tcpAccept, udpSendTo, tcpListen, tcpConnect)

b2s x: Bool =
  | x |> "true"
  | _ |> "false"
=> :Str

report label: Str e: Error =
  stdout(label + " " + e.type + " " + e.message)
=> :Int

closeS label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  socketClose(h) >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

closeU label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  udpClose(h) >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

closeL label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  listenerClose(h) >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

send label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  socketSend(h, "x") >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

sendAll label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  socketSendAll(h, "x") >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

accept label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  tcpAccept(h, 100) >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

udpSend label: Str h: Int =
  |== e: Error =
    report(label, e)
  => :Int
  udpSendTo(h, "127.0.0.1", 9, Bytes["x"]().unmold()) >=> res
  res >=> r
  stdout(label + " ok")
=> :Int

stdout("before")
closeS("socketClose(1)", 1)
closeS("socketClose(-5)", -5)
closeU("udpClose(1)", 1)
closeL("listenerClose(1)", 1)
send("socketSend(1)", 1)
sendAll("socketSendAll(2)", 2)
accept("tcpAccept(0)", 0)
udpSend("udpSendTo(1)", 1)
socketRecv(0, 100) >=> recv
stdout("socketRecv(0) has=" + b2s(recv.has_value))
stdout("after")
"#,
    );
    let expected = "before\n\
        socketClose(1) IoError socketClose: unknown socket handle\n\
        socketClose(-5) IoError socketClose: unknown socket handle\n\
        udpClose(1) IoError udpClose: unknown socket handle\n\
        listenerClose(1) IoError listenerClose: unknown listener handle\n\
        socketSend(1) IoError socketSend: unknown socket handle\n\
        socketSendAll(2) IoError socketSendAll: unknown socket handle\n\
        tcpAccept(0) IoError tcpAccept: unknown listener handle\n\
        udpSendTo(1) IoError udpSendTo: unknown socket handle\n\
        socketRecv(0) has=false\n\
        after\n";
    for (label, cmd) in backend_commands(&td) {
        assert_eq!(stdout_of(cmd), expected, "{label}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

// ── HTTP client ─────────────────────────────────────────────────

/// Serves canned responses by request path until the test ends.
fn start_canned_http_server(responses: Vec<(&'static str, &'static [u8])>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let path = text(&request).split(' ').nth(1).unwrap_or("/").to_string();
            let body = responses
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, r)| *r)
                .unwrap_or(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            let _ = stream.write_all(body);
        }
    });
    port
}

/// The plain-HTTP client decodes a chunked response body (the HTTPS path
/// already did, through curl).
#[test]
fn http_client_decodes_chunked_responses() {
    let port = start_canned_http_server(vec![
        (
            "/chunked",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        ),
        (
            "/ext",
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: CHUNKED\r\n\r\n5;x=1\r\nhello\r\n0\r\nX-T: 1\r\n\r\n",
        ),
        (
            "/gzip-chunked",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ),
        (
            "/chunked-gzip",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ),
        (
            "/bad",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "/truncated",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel",
        ),
        ("/plain", b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nplain"),
    ]);
    let dir = unique_temp_dir("f65_net_client");
    let td = write_program(
        &dir,
        "client.td",
        &format!(
            r#">>> taida-lang/os => @(HttpRequest)

b2s x: Bool =
  | x |> "true"
  | _ |> "false"
=> :Str

fetch path: Str =
  HttpRequest["GET", "http://127.0.0.1:{port}" + path]() >=> lax
  res <= lax.getOrDefault(@(status <= -1, body <= "<none>", headers <= @()))
  stdout(path + " " + b2s(lax.has_value) + " " + res.status.toString() + " [" + res.body + "]")
=> :Int

fetch("/chunked")
fetch("/ext")
fetch("/gzip-chunked")
fetch("/chunked-gzip")
fetch("/bad")
fetch("/truncated")
fetch("/plain")
"#
        ),
    );
    let expected = "/chunked true 200 [hello world]\n\
        /ext true 200 [hello]\n\
        /gzip-chunked true 200 [abc]\n\
        /chunked-gzip true 200 [3\r\nabc\r\n0\r\n\r\n]\n\
        /bad false -1 [<none>]\n\
        /truncated false -1 [<none>]\n\
        /plain true 200 [plain]\n";
    for (label, cmd) in backend_commands(&td) {
        assert_eq!(stdout_of(cmd), expected, "{label}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

// ── HTTP/2 (interpreter) ────────────────────────────────────────

mod h2 {
    use super::*;
    use std::sync::Arc;

    pub(super) struct Tls {
        pub cert: PathBuf,
        pub key: PathBuf,
        pub der: rustls::pki_types::CertificateDer<'static>,
    }

    pub(super) fn self_signed(dir: &Path) -> Tls {
        let certified =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("cert");
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        std::fs::write(&cert, certified.cert.pem()).expect("write cert");
        std::fs::write(&key, certified.key_pair.serialize_pem()).expect("write key");
        Tls {
            cert,
            key,
            der: certified.cert.der().clone(),
        }
    }

    pub(super) type Conn = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

    pub(super) fn connect(port: u16, tls: &Tls) -> Conn {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(tls.der.clone()).expect("root");
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let name = rustls::pki_types::ServerName::try_from("localhost").expect("name");
        let conn = rustls::ClientConnection::new(Arc::new(config), name).expect("client");
        let sock = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut stream = rustls::StreamOwned::new(conn, sock);
        stream
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
            .expect("preface");
        stream.write_all(&frame(4, 0, 0, &[])).expect("settings");
        stream
    }

    pub(super) fn frame(kind: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let mut out = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
        out.extend_from_slice(&stream_id.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// HPACK block: GET/POST, https, the given :path, :authority localhost.
    pub(super) fn request_headers(post: bool, path: &str) -> Vec<u8> {
        let mut block = vec![if post { 0x83 } else { 0x82 }, 0x87, 0x04, path.len() as u8];
        block.extend_from_slice(path.as_bytes());
        block.extend_from_slice(&[0x01, 9]);
        block.extend_from_slice(b"localhost");
        block
    }

    /// Reads one frame: (type, flags, stream id, payload).
    pub(super) fn read_frame(stream: &mut Conn) -> Option<(u8, u8, u32, Vec<u8>)> {
        let mut head = [0u8; 9];
        stream.read_exact(&mut head).ok()?;
        let len = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).ok()?;
        let id = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) & 0x7fff_ffff;
        Some((head[3], head[4], id, payload))
    }

    pub(super) const SERVER: &str = r#">>> taida-lang/net => @(httpServe, HttpProtocol)

noHeaders =
  e: @[@(name: Str, value: Str)] <= @[]
  e
=> :@[@(name: Str, value: Str)]

handler req: Request =
  p <= StrOf[req.path, req.raw]()
  | p == "/big" |> @(status <= 200, headers <= noHeaders(), body <= Repeat["A", 100000]())
  | _ |> @(status <= 200, headers <= noHeaders(), body <= "len=" + req.body.len.toString())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(0, handler, 100, 5000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= HttpProtocol:H2())) >=> result
result >=> summary
stdout("requests: " + summary.requests.toString())
"#;

    pub(super) fn start(dir: &Path, tls: &Tls) -> (Server, u16) {
        let source = SERVER
            .replace("{cert}", tls.cert.to_str().expect("cert path"))
            .replace("{key}", tls.key.to_str().expect("key path"));
        let td = write_program(dir, "h2.td", &source);
        let mut cmd = Command::new(taida_bin());
        cmd.arg(td);
        start_server(cmd)
    }
}

/// A response larger than the client's flow-control window waits for
/// WINDOW_UPDATE instead of resetting the stream, and a request that
/// completes in the meantime (its body larger than the server's own initial
/// window, so the server must replenish that window while it waits) is still
/// answered.
#[test]
fn h2_flow_control_waits_for_the_peer_window() {
    let dir = unique_temp_dir("f65_net_h2_flow");
    let tls = h2::self_signed(&dir);
    let (_server, port) = h2::start(&dir, &tls);
    let mut conn = h2::connect(port, &tls);
    conn.write_all(&h2::frame(1, 0x5, 1, &h2::request_headers(false, "/big")))
        .expect("request 1");

    let body3 = vec![b'x'; 70_000];
    let mut sent3 = 0usize;
    // 0: stream 1 fills our window, 1: stream 3 body goes out, 2: acking
    let mut phase = 0;
    let mut stream1_bytes = 0usize;
    let mut unacked1 = 0u32;
    let mut send_conn_window: i64 = 65_535;
    let mut send_stream_window: i64 = 65_535;
    let mut response3 = Vec::new();
    let mut ended = std::collections::HashSet::<u32>::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while ended.len() < 2 && Instant::now() < deadline {
        if phase == 1 {
            while sent3 < body3.len() {
                let n = (body3.len() - sent3)
                    .min(16_384)
                    .min(send_conn_window.max(0) as usize)
                    .min(send_stream_window.max(0) as usize);
                if n == 0 {
                    break;
                }
                let flags = if sent3 + n == body3.len() { 1 } else { 0 };
                conn.write_all(&h2::frame(0, flags, 3, &body3[sent3..sent3 + n]))
                    .expect("data");
                sent3 += n;
                send_conn_window -= n as i64;
                send_stream_window -= n as i64;
            }
            if sent3 == body3.len() {
                // Only now open our window for stream 1 again.
                let inc = unacked1.to_be_bytes();
                conn.write_all(&h2::frame(8, 0, 0, &inc))
                    .expect("window update");
                conn.write_all(&h2::frame(8, 0, 1, &inc))
                    .expect("window update");
                unacked1 = 0;
                phase = 2;
            }
        }
        let Some((kind, flags, id, payload)) = h2::read_frame(&mut conn) else {
            break;
        };
        match kind {
            0 => {
                if id == 1 {
                    stream1_bytes += payload.len();
                    if phase == 2 {
                        let inc = (payload.len() as u32).to_be_bytes();
                        conn.write_all(&h2::frame(8, 0, 0, &inc))
                            .expect("window update");
                        conn.write_all(&h2::frame(8, 0, 1, &inc))
                            .expect("window update");
                    } else {
                        unacked1 += payload.len() as u32;
                    }
                    if phase == 0 && stream1_bytes >= 65_535 {
                        // The server's send window is used up: ask for
                        // something else meanwhile.
                        conn.write_all(&h2::frame(1, 0x4, 3, &h2::request_headers(true, "/echo")))
                            .expect("request 2");
                        phase = 1;
                    }
                } else if id == 3 {
                    response3.extend_from_slice(&payload);
                }
                if flags & 1 != 0 {
                    ended.insert(id);
                }
            }
            1 if flags & 1 != 0 => {
                ended.insert(id);
            }
            3 => panic!("RST_STREAM on stream {id}: {payload:?}"),
            4 if flags & 1 == 0 => {
                for setting in payload.chunks(6) {
                    if setting.len() == 6 && setting[..2] == [0, 4] {
                        send_stream_window =
                            u32::from_be_bytes([setting[2], setting[3], setting[4], setting[5]])
                                as i64;
                    }
                }
                conn.write_all(&h2::frame(4, 1, 0, &[]))
                    .expect("settings ack");
            }
            7 => panic!("GOAWAY: {payload:?}"),
            8 => {
                let inc =
                    u32::from_be_bytes([payload[0] & 0x7f, payload[1], payload[2], payload[3]])
                        as i64;
                if id == 0 {
                    send_conn_window += inc;
                } else if id == 3 {
                    send_stream_window += inc;
                }
            }
            _ => {}
        }
    }
    assert_eq!(stream1_bytes, 100_000);
    assert_eq!(text(&response3), "len=70000");
    let _ = std::fs::remove_dir_all(dir);
}

/// A request body larger than the server's initial receive window (65535
/// bytes) completes: the server opens its window while the body arrives.
#[test]
fn h2_request_body_larger_than_the_initial_window() {
    let dir = unique_temp_dir("f65_net_h2_body");
    let tls = h2::self_signed(&dir);
    let (_server, port) = h2::start(&dir, &tls);
    let mut conn = h2::connect(port, &tls);
    conn.write_all(&h2::frame(1, 0x4, 1, &h2::request_headers(true, "/echo")))
        .expect("headers");
    let body = vec![b'x'; 200_000];
    let mut sent = 0usize;
    let mut conn_window: i64 = 65_535;
    let mut stream_window: i64 = 65_535;
    let mut response = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        while sent < body.len() {
            let n = (body.len() - sent)
                .min(16_384)
                .min(conn_window.max(0) as usize)
                .min(stream_window.max(0) as usize);
            if n == 0 {
                break;
            }
            let flags = if sent + n == body.len() { 1 } else { 0 };
            conn.write_all(&h2::frame(0, flags, 1, &body[sent..sent + n]))
                .expect("data");
            sent += n;
            conn_window -= n as i64;
            stream_window -= n as i64;
        }
        let Some((kind, flags, id, payload)) = h2::read_frame(&mut conn) else {
            break;
        };
        assert!(kind != 3 && kind != 7, "frame type {kind}: {payload:?}");
        if kind == 8 {
            let inc =
                u32::from_be_bytes([payload[0] & 0x7f, payload[1], payload[2], payload[3]]) as i64;
            if id == 0 {
                conn_window += inc;
            } else if id == 1 {
                stream_window += inc;
            }
        } else if kind == 0 && id == 1 {
            response.extend_from_slice(&payload);
            if flags & 1 != 0 {
                break;
            }
        }
    }
    assert_eq!(sent, body.len());
    assert_eq!(text(&response), "len=200000");
    let _ = std::fs::remove_dir_all(dir);
}

/// Request trailers (a HEADERS frame after the body) complete the request
/// instead of failing the whole connection.
#[test]
fn h2_request_trailers_complete_the_request() {
    let dir = unique_temp_dir("f65_net_h2_trailers");
    let tls = h2::self_signed(&dir);
    let (_server, port) = h2::start(&dir, &tls);
    let mut conn = h2::connect(port, &tls);
    let trailer = [
        0x00, 8, b'x', b'-', b'd', b'i', b'g', b'e', b's', b't', 3, b'a', b'b', b'c',
    ];
    conn.write_all(&h2::frame(1, 0x4, 1, &h2::request_headers(true, "/echo")))
        .expect("headers");
    conn.write_all(&h2::frame(0, 0, 1, b"hello")).expect("data");
    conn.write_all(&h2::frame(1, 0x5, 1, &trailer))
        .expect("trailers");
    let mut body = Vec::new();
    while let Some((kind, flags, id, payload)) = h2::read_frame(&mut conn) {
        assert!(kind != 3 && kind != 7, "frame type {kind} on stream {id}");
        if kind == 0 && id == 1 {
            body.extend_from_slice(&payload);
            if flags & 1 != 0 {
                break;
            }
        }
    }
    assert_eq!(text(&body), "len=5");
    let _ = std::fs::remove_dir_all(dir);
}
