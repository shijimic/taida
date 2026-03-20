use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, ErrorValue, PendingState, Value};
use crate::parser::Expr;
use std::sync::atomic::Ordering;

/// Return a human-readable name for a Signal variant (for error diagnostics).
fn signal_name(sig: &Signal) -> &'static str {
    match sig {
        Signal::Value(_) => "Value",
        Signal::Throw(_) => "Throw",
        Signal::Gorilla => "Gorilla",
        Signal::TailCall(_) => "TailCall",
    }
}
/// OS package evaluation for the Taida interpreter.
///
/// Implements the 34 APIs of `taida-lang/os` (core-bundled):
///
/// Input molds (-> Lax/Bool):
///   Read[path](), ListDir[path](), Stat[path](), Exists[path](), EnvVar[name]()
///
/// Async input molds (-> Async[Lax[T]]):
///   ReadAsync[path](), HttpGet[url](), HttpPost[url, body](),
///   HttpRequest[method, url](headers, body)
///
/// Side-effect functions (-> Result):
///   writeFile(path, content), writeBytes(path, content), appendFile(path, content), remove(path),
///   createDir(path), rename(from, to)
///
/// Binary file query function:
///   readBytes(path) -> Lax[Bytes]
///
/// Dangerous side-effect functions (-> Gorillax):
///   run(program, args), execShell(command)
///
/// Async functions:
///   tcpConnect(host, port), tcpListen(port), tcpAccept(listener),
///   socketSend(socket, data), socketSendAll(socket, data), socketRecv(socket),
///   socketSendBytes(socket, data), socketRecvBytes(socket), socketRecvExact(socket, size),
///   udpBind(host, port), udpSendTo(socket, host, port, data), udpRecvFrom(socket),
///   socketClose(socket), listenerClose(listener), udpClose(socket)
///
/// Query functions:
///   allEnv() -> HashMap[Str, Str]
///   argv() -> List[Str] (user args; interpreter strips `taida` and script path)
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use std::sync::{Arc, Mutex};

/// Maximum file size for Read / ReadAsync (64 MB).
const MAX_READ_SIZE: u64 = 64 * 1024 * 1024;
/// Default timeout for network operations (connect/send/recv/listen).
const DEFAULT_NETWORK_TIMEOUT_MS: u64 = 30_000;

/// The 35 symbols exported by the os package.
pub(crate) const OS_SYMBOLS: &[&str] = &[
    "Read",
    "ListDir",
    "Stat",
    "Exists",
    "EnvVar",
    "readBytes",
    "writeFile",
    "writeBytes",
    "appendFile",
    "remove",
    "createDir",
    "rename",
    "run",
    "execShell",
    "allEnv",
    "argv",
    // Phase 2: async APIs
    "ReadAsync",
    "HttpGet",
    "HttpPost",
    "HttpRequest",
    "tcpConnect",
    "tcpListen",
    "tcpAccept",
    "socketSend",
    "socketSendAll",
    "socketRecv",
    "socketSendBytes",
    "socketRecvBytes",
    "socketRecvExact",
    "udpBind",
    "udpSendTo",
    "udpRecvFrom",
    "socketClose",
    "listenerClose",
    "udpClose",
];

// ── Helpers ─────────────────────────────────────────────────

/// Create a Lax[T] success value: hasValue=true, __value=val, __default inferred.
fn make_lax_success(val: Value) -> Value {
    let default_val = Interpreter::default_for_value(&val);
    Value::BuchiPack(vec![
        ("hasValue".into(), Value::Bool(true)),
        ("__value".into(), val),
        ("__default".into(), default_val),
        ("__type".into(), Value::Str("Lax".into())),
    ])
}

/// Create a Lax[T] failure value: hasValue=false, __value=default, __default=default.
fn make_lax_failure(default_val: Value) -> Value {
    Value::BuchiPack(vec![
        ("hasValue".into(), Value::Bool(false)),
        ("__value".into(), default_val.clone()),
        ("__default".into(), default_val),
        ("__type".into(), Value::Str("Lax".into())),
    ])
}

/// Create an os Result success value: @(ok=true, code=0, message="").
fn make_result_success(inner: Value) -> Value {
    Value::BuchiPack(vec![
        ("__value".into(), inner),
        ("throw".into(), Value::Unit),
        ("__predicate".into(), Value::Unit),
        ("__type".into(), Value::Str("Result".into())),
    ])
}

/// Create an os Result failure value with throw set to an IoError.
fn make_result_failure(err: &std::io::Error) -> Value {
    let code = err.raw_os_error().unwrap_or(-1) as i64;
    let message = err.to_string();
    let kind = classify_io_error_kind(err).to_string();
    let inner = Value::BuchiPack(vec![
        ("ok".into(), Value::Bool(false)),
        ("code".into(), Value::Int(code)),
        ("message".into(), Value::Str(message.clone())),
        ("kind".into(), Value::Str(kind.clone())),
    ]);
    let error_val = make_io_error(err);
    Value::BuchiPack(vec![
        ("__value".into(), inner),
        ("throw".into(), error_val),
        ("__predicate".into(), Value::Unit),
        ("__type".into(), Value::Str("Result".into())),
    ])
}

/// Create an os Result failure value with explicit kind/message (non-OS errors).
fn make_result_failure_with_kind(kind: &str, message: impl Into<String>) -> Value {
    let message = message.into();
    let inner = Value::BuchiPack(vec![
        ("ok".into(), Value::Bool(false)),
        ("code".into(), Value::Int(-1)),
        ("message".into(), Value::Str(message.clone())),
        ("kind".into(), Value::Str(kind.to_string())),
    ]);
    let error_val = Value::Error(ErrorValue {
        error_type: "IoError".to_string(),
        message,
        fields: vec![
            ("code".into(), Value::Int(-1)),
            ("kind".into(), Value::Str(kind.to_string())),
        ],
    });
    Value::BuchiPack(vec![
        ("__value".into(), inner),
        ("throw".into(), error_val),
        ("__predicate".into(), Value::Unit),
        ("__type".into(), Value::Str("Result".into())),
    ])
}

fn make_async_fulfilled(value: Value) -> Value {
    Value::Async(AsyncValue {
        status: AsyncStatus::Fulfilled,
        value: Box::new(value),
        error: Box::new(Value::Unit),
        task: None,
    })
}

/// Create a Gorillax success value: hasValue=true, __value=val, __error=Unit.
fn make_gorillax_success(val: Value) -> Value {
    Value::BuchiPack(vec![
        ("hasValue".into(), Value::Bool(true)),
        ("__value".into(), val),
        ("__error".into(), Value::Unit),
        ("__type".into(), Value::Str("Gorillax".into())),
    ])
}

/// Create a Gorillax failure value: hasValue=false, __error=err.
fn make_gorillax_failure(err: Value) -> Value {
    Value::BuchiPack(vec![
        ("hasValue".into(), Value::Bool(false)),
        ("__value".into(), Value::Unit),
        ("__error".into(), err),
        ("__type".into(), Value::Str("Gorillax".into())),
    ])
}

fn make_io_error(err: &std::io::Error) -> Value {
    let code = err.raw_os_error().unwrap_or(-1) as i64;
    let message = err.to_string();
    let kind = classify_io_error_kind(err).to_string();
    Value::Error(ErrorValue {
        error_type: "IoError".to_string(),
        message,
        fields: vec![
            ("code".into(), Value::Int(code)),
            ("kind".into(), Value::Str(kind)),
        ],
    })
}

fn classify_io_error_kind(err: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match err.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => return "timeout",
        ErrorKind::ConnectionRefused => return "refused",
        ErrorKind::ConnectionReset => return "reset",
        ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe
        | ErrorKind::UnexpectedEof
        | ErrorKind::WriteZero
        | ErrorKind::NotConnected => return "peer_closed",
        ErrorKind::NotFound => return "not_found",
        ErrorKind::InvalidInput | ErrorKind::InvalidData => return "invalid",
        _ => {}
    }

    if let Some(code) = err.raw_os_error() {
        match code {
            11 => return "timeout",
            110 | 60 => return "timeout",
            111 | 61 => return "refused",
            104 | 54 => return "reset",
            32 | 57 | 107 => return "peer_closed",
            _ => {}
        }
    }

    let message = err.to_string().to_ascii_lowercase();
    if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("connection refused") {
        "refused"
    } else if message.contains("connection reset") {
        "reset"
    } else if message.contains("peer closed")
        || message.contains("broken pipe")
        || message.contains("unexpected eof")
        || message.contains("socket hang up")
    {
        "peer_closed"
    } else if message.contains("lookup")
        || message.contains("getaddrinfo")
        || message.contains("name or service not known")
        || message.contains("dns")
    {
        "dns"
    } else if message.contains("invalid") {
        "invalid"
    } else {
        "other"
    }
}

fn make_process_error(message: String, code: i64) -> Value {
    Value::Error(ErrorValue {
        error_type: "ProcessError".to_string(),
        message,
        fields: vec![("code".into(), Value::Int(code))],
    })
}

/// Build the standard success inner BuchiPack: @(ok=true, code=0, message="").
fn ok_inner() -> Value {
    Value::BuchiPack(vec![
        ("ok".into(), Value::Bool(true)),
        ("code".into(), Value::Int(0)),
        ("message".into(), Value::Str(String::new())),
    ])
}

/// Build a process result inner BuchiPack: @(stdout, stderr, code).
fn process_inner(stdout: String, stderr: String, code: i64) -> Value {
    Value::BuchiPack(vec![
        ("stdout".into(), Value::Str(stdout)),
        ("stderr".into(), Value::Str(stderr)),
        ("code".into(), Value::Int(code)),
    ])
}

/// Format a SystemTime as RFC3339/UTC string (seconds precision).
fn format_rfc3339_utc(time: std::time::SystemTime) -> String {
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Manual UTC calendar calculation (no chrono dependency)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since 1970-01-01 to (year, month, day) — civil_from_days algorithm
    let (year, month, day) = civil_from_days(days as i64);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since 1970-01-01 to (year, month, day).
/// Based on Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month index [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // month [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ── HTTP helper (minimal HTTP/1.1 client using tokio TcpStream) ──

/// Build an HTTP response Value: Lax[@(status: Int, body: Str, headers: @(...))]
fn make_http_response(status: i64, body: String, headers: Vec<(String, String)>) -> Value {
    let header_fields: Vec<(String, Value)> = headers
        .into_iter()
        .map(|(k, v)| (k, Value::Str(v)))
        .collect();
    let response = Value::BuchiPack(vec![
        ("status".into(), Value::Int(status)),
        ("body".into(), Value::Str(body)),
        ("headers".into(), Value::BuchiPack(header_fields)),
    ]);
    make_lax_success(response)
}

fn make_http_failure() -> Value {
    let default_response = Value::BuchiPack(vec![
        ("status".into(), Value::Int(0)),
        ("body".into(), Value::Str(String::new())),
        ("headers".into(), Value::BuchiPack(vec![])),
    ]);
    make_lax_failure(default_response)
}

fn make_udp_recv_default_payload() -> Value {
    Value::BuchiPack(vec![
        ("host".into(), Value::Str(String::new())),
        ("port".into(), Value::Int(0)),
        ("data".into(), Value::Bytes(Vec::new())),
        ("truncated".into(), Value::Bool(false)),
    ])
}

/// Parse a URL into (host, port, path, use_tls).
fn parse_url(url: &str) -> Option<(String, u16, String, bool)> {
    let (scheme, rest) = if let Some(stripped) = url.strip_prefix("https://") {
        ("https", stripped)
    } else if let Some(stripped) = url.strip_prefix("http://") {
        ("http", stripped)
    } else {
        // Default to http
        ("http", url)
    };

    let use_tls = scheme == "https";
    let default_port: u16 = if use_tls { 443 } else { 80 };

    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    let (host, port) = match host_port.rfind(':') {
        Some(idx) => {
            let port_str = &host_port[idx + 1..];
            match port_str.parse::<u16>() {
                Ok(p) => (host_port[..idx].to_string(), p),
                Err(_) => (host_port.to_string(), default_port),
            }
        }
        None => (host_port.to_string(), default_port),
    };

    // RCB-304: Reject URLs with CR/LF in host or path to prevent CRLF injection
    if host.contains('\r') || host.contains('\n') || path.contains('\r') || path.contains('\n') {
        return None;
    }

    Some((host, port, path.to_string(), use_tls))
}

fn parse_http_response_text(response_text: &str) -> Option<Value> {
    let (head, body_str) = match response_text.find("\r\n\r\n") {
        Some(idx) => (&response_text[..idx], &response_text[idx + 4..]),
        None => return None,
    };

    let mut lines = head.lines();
    let status_line = lines.next()?;
    let status_code: i64 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_lowercase();
            let value = line[colon + 1..].trim().to_string();
            headers.push((key, value));
        }
    }

    Some(make_http_response(
        status_code,
        body_str.to_string(),
        headers,
    ))
}

async fn http_request_async_via_curl(
    method: &str,
    url: &str,
    extra_headers: &[(String, String)],
    body: &str,
) -> Option<Value> {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-i")
        .arg("--max-time")
        .arg("30")
        // RCB-306: Limit response size to prevent OOM
        .arg("--max-filesize")
        .arg("104857600") // 100 MB
        .arg("-X")
        .arg(method)
        .arg(url);
    for (k, v) in extra_headers {
        // RCB-304: Strip CR/LF from header values to prevent CRLF injection
        let safe_k = k.replace(['\r', '\n'], "");
        let safe_v = v.replace(['\r', '\n'], "");
        cmd.arg("-H").arg(format!("{}: {}", safe_k, safe_v));
    }
    if !body.is_empty() {
        cmd.arg("--data-raw").arg(body);
    }

    let output = cmd.output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_http_response_text(&text)
}

/// Perform an HTTP/1.1 request.
/// - HTTP: raw TCP implementation (existing behavior)
/// - HTTPS: curl transport with TLS
async fn http_request_async(
    method: &str,
    url: &str,
    extra_headers: &[(String, String)],
    body: &str,
) -> Value {
    let (host, port, path, use_tls) = match parse_url(url) {
        Some(parsed) => parsed,
        None => return make_http_failure(),
    };

    if use_tls {
        return http_request_async_via_curl(method, url, extra_headers, body)
            .await
            .unwrap_or_else(make_http_failure);
    }

    let addr = format!("{}:{}", host, port);
    let stream = match tokio::net::TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(_) => return make_http_failure(),
    };

    // Build HTTP request
    let mut request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        method, path, host
    );
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        request.push_str("Content-Type: text/plain\r\n");
    }
    for (k, v) in extra_headers {
        // RCB-304: Strip CR/LF from header values to prevent CRLF injection
        let safe_k = k.replace(['\r', '\n'], "");
        let safe_v = v.replace(['\r', '\n'], "");
        request.push_str(&format!("{}: {}\r\n", safe_k, safe_v));
    }
    request.push_str("\r\n");
    if !body.is_empty() {
        request.push_str(body);
    }

    // Send request
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (reader, mut writer) = stream.into_split();
    if writer.write_all(request.as_bytes()).await.is_err() {
        return make_http_failure();
    }
    if writer.shutdown().await.is_err() {
        // Ignore shutdown errors — we may still read the response
    }

    // Read response with size limit (RCB-306: prevent OOM from huge responses)
    const MAX_HTTP_RESPONSE_SIZE: usize = 100 * 1024 * 1024; // 100 MB
    let mut response_buf = Vec::new();
    if reader
        .take(MAX_HTTP_RESPONSE_SIZE as u64)
        .read_to_end(&mut response_buf)
        .await
        .is_err()
    {
        return make_http_failure();
    }

    let response_str = String::from_utf8_lossy(&response_buf);
    parse_http_response_text(&response_str).unwrap_or_else(make_http_failure)
}

// ── Mold evaluation (input APIs) ────────────────────────────

impl Interpreter {
    /// Try to evaluate an os input mold: Read, ListDir, Stat, Exists, EnvVar,
    /// ReadAsync, HttpGet, HttpPost, HttpRequest.
    /// Returns None if the name is not a recognized os mold.
    pub(crate) fn try_os_mold(
        &mut self,
        name: &str,
        type_args: &[Expr],
        fields: &[crate::parser::BuchiField],
    ) -> Result<Option<Signal>, RuntimeError> {
        match name {
            // ── Read[path]() → Lax[Str] ──────────────────────
            "Read" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Read requires 1 argument: Read[path]()".into(),
                    });
                }
                let path = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Read: path must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                // Check file size before reading (64MB limit)
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        if meta.len() > MAX_READ_SIZE {
                            return Ok(Some(Signal::Value(make_lax_failure(Value::Str(
                                String::new(),
                            )))));
                        }
                    }
                    Err(_) => {
                        return Ok(Some(Signal::Value(make_lax_failure(Value::Str(
                            String::new(),
                        )))));
                    }
                }

                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Some(Signal::Value(make_lax_success(Value::Str(content))))),
                    Err(_) => Ok(Some(Signal::Value(make_lax_failure(Value::Str(
                        String::new(),
                    ))))),
                }
            }

            // ── ListDir[path]() → Lax[@[Str]] ───────────────
            "ListDir" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "ListDir requires 1 argument: ListDir[path]()".into(),
                    });
                }
                let path = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ListDir: path must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut names: Vec<Value> = Vec::new();
                        for entry in entries {
                            if let Ok(e) = entry
                                && let Some(name) = e.file_name().to_str()
                            {
                                names.push(Value::Str(name.to_string()));
                            }
                        }
                        names.sort_by(|a, b| {
                            if let (Value::Str(a), Value::Str(b)) = (a, b) {
                                a.cmp(b)
                            } else {
                                std::cmp::Ordering::Equal
                            }
                        });
                        Ok(Some(Signal::Value(make_lax_success(Value::List(names)))))
                    }
                    Err(_) => Ok(Some(Signal::Value(make_lax_failure(Value::List(
                        Vec::new(),
                    ))))),
                }
            }

            // ── Stat[path]() → Lax[@(size: Int, modified: Str, isDir: Bool)] ──
            "Stat" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Stat requires 1 argument: Stat[path]()".into(),
                    });
                }
                let path = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Stat: path must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                let default_stat = Value::BuchiPack(vec![
                    ("size".into(), Value::Int(0)),
                    ("modified".into(), Value::Str(String::new())),
                    ("isDir".into(), Value::Bool(false)),
                ]);

                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let size = meta.len() as i64;
                        let modified = meta.modified().map(format_rfc3339_utc).unwrap_or_default();
                        let is_dir = meta.is_dir();
                        let stat_pack = Value::BuchiPack(vec![
                            ("size".into(), Value::Int(size)),
                            ("modified".into(), Value::Str(modified)),
                            ("isDir".into(), Value::Bool(is_dir)),
                        ]);
                        Ok(Some(Signal::Value(make_lax_success(stat_pack))))
                    }
                    Err(_) => Ok(Some(Signal::Value(make_lax_failure(default_stat)))),
                }
            }

            // ── Exists[path]() → Bool ────────────────────────
            "Exists" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Exists requires 1 argument: Exists[path]()".into(),
                    });
                }
                let path = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Exists: path must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                let exists = std::path::Path::new(&path).exists();
                Ok(Some(Signal::Value(Value::Bool(exists))))
            }

            // ── EnvVar[name]() → Lax[Str] ───────────────────
            "EnvVar" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "EnvVar requires 1 argument: EnvVar[name]()".into(),
                    });
                }
                let name = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("EnvVar: name must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                match std::env::var(&name) {
                    Ok(val) => Ok(Some(Signal::Value(make_lax_success(Value::Str(val))))),
                    Err(_) => Ok(Some(Signal::Value(make_lax_failure(Value::Str(
                        String::new(),
                    ))))),
                }
            }

            // ── ReadAsync[path]() → Async[Lax[Str]] ────────────
            "ReadAsync" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "ReadAsync requires 1 argument: ReadAsync[path]()".into(),
                    });
                }
                let path = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ReadAsync: path must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    // Check file size first
                    let meta_result = tokio::fs::metadata(&path).await;
                    let result = match meta_result {
                        Ok(meta) if meta.len() > MAX_READ_SIZE => {
                            Ok(make_lax_failure(Value::Str(String::new())))
                        }
                        Err(_) => Ok(make_lax_failure(Value::Str(String::new()))),
                        Ok(_) => match tokio::fs::read_to_string(&path).await {
                            Ok(content) => Ok(make_lax_success(Value::Str(content))),
                            Err(_) => Ok(make_lax_failure(Value::Str(String::new()))),
                        },
                    };
                    let _ = tx.send(result);
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── HttpGet[url]() → Async[Lax[@(status, body, headers)]] ──
            "HttpGet" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "HttpGet requires 1 argument: HttpGet[url]()".into(),
                    });
                }
                let url = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("HttpGet: url must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let result = http_request_async("GET", &url, &[], "").await;
                    let _ = tx.send(Ok(result));
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── HttpPost[url, body]() → Async[Lax[@(status, body, headers)]] ──
            "HttpPost" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "HttpPost requires 2 arguments: HttpPost[url, body]()".into(),
                    });
                }
                let url = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("HttpPost: url must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let body = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("HttpPost: body must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let result = http_request_async("POST", &url, &[], &body).await;
                    let _ = tx.send(Ok(result));
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── HttpRequest[method, url](headers, body) → Async[Lax[@(status, body, headers)]] ──
            "HttpRequest" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "HttpRequest requires at least 2 type arguments: HttpRequest[method, url]()".into(),
                    });
                }
                let method = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("HttpRequest: method must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let url = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("HttpRequest: url must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                // Extract optional headers and body from fields:
                // HttpRequest["GET", "url"](headers <= @(...), body <= "...")
                let mut extra_headers: Vec<(String, String)> = Vec::new();
                let mut body = String::new();
                for field in fields {
                    match field.name.as_str() {
                        "headers" => {
                            if let Signal::Value(Value::BuchiPack(hfields)) =
                                self.eval_expr(&field.value)?
                            {
                                for (k, v) in &hfields {
                                    if let Value::Str(vs) = v {
                                        extra_headers.push((k.clone(), vs.clone()));
                                    }
                                }
                            }
                        }
                        "body" => {
                            if let Signal::Value(Value::Str(s)) = self.eval_expr(&field.value)? {
                                body = s;
                            }
                        }
                        _ => {}
                    }
                }

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let result = http_request_async(&method, &url, &extra_headers, &body).await;
                    let _ = tx.send(Ok(result));
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            _ => Ok(None),
        }
    }

    // ── Function evaluation (side-effect + query APIs) ──────

    /// Try to handle an os built-in function call.
    /// Returns None if the name is not a recognized os function.
    pub(crate) fn try_os_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        match name {
            // ── readBytes(path) → Lax[Bytes] ────────────────
            "readBytes" => {
                let path = self.eval_os_str_arg(args, 0, "readBytes", "path")?;

                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        if meta.len() > MAX_READ_SIZE {
                            return Ok(Some(Signal::Value(make_lax_failure(Value::Bytes(
                                Vec::new(),
                            )))));
                        }
                    }
                    Err(_) => {
                        return Ok(Some(Signal::Value(make_lax_failure(Value::Bytes(
                            Vec::new(),
                        )))));
                    }
                }

                match std::fs::read(&path) {
                    Ok(content) => Ok(Some(Signal::Value(make_lax_success(Value::Bytes(content))))),
                    Err(_) => Ok(Some(Signal::Value(make_lax_failure(Value::Bytes(
                        Vec::new(),
                    ))))),
                }
            }

            // ── writeFile(path, content) → Result ────────────
            "writeFile" => {
                let path = self.eval_os_str_arg(args, 0, "writeFile", "path")?;
                let content = self.eval_os_str_arg(args, 1, "writeFile", "content")?;

                match std::fs::write(&path, &content) {
                    Ok(_) => Ok(Some(Signal::Value(make_result_success(ok_inner())))),
                    Err(e) => Ok(Some(Signal::Value(make_result_failure(&e)))),
                }
            }

            // ── writeBytes(path, content) → Result ───────────
            "writeBytes" => {
                let path = self.eval_os_str_arg(args, 0, "writeBytes", "path")?;
                let content = self.eval_os_bytes_arg(args, 1, "writeBytes", "content")?;

                match std::fs::write(&path, &content) {
                    Ok(_) => Ok(Some(Signal::Value(make_result_success(ok_inner())))),
                    Err(e) => Ok(Some(Signal::Value(make_result_failure(&e)))),
                }
            }

            // ── appendFile(path, content) → Result ───────────
            "appendFile" => {
                let path = self.eval_os_str_arg(args, 0, "appendFile", "path")?;
                let content = self.eval_os_str_arg(args, 1, "appendFile", "content")?;

                use std::io::Write;
                let result = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| f.write_all(content.as_bytes()));

                match result {
                    Ok(_) => Ok(Some(Signal::Value(make_result_success(ok_inner())))),
                    Err(e) => Ok(Some(Signal::Value(make_result_failure(&e)))),
                }
            }

            // ── remove(path) → Result ────────────────────────
            "remove" => {
                let path = self.eval_os_str_arg(args, 0, "remove", "path")?;

                let result = if std::path::Path::new(&path).is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };

                match result {
                    Ok(_) => Ok(Some(Signal::Value(make_result_success(ok_inner())))),
                    Err(e) => Ok(Some(Signal::Value(make_result_failure(&e)))),
                }
            }

            // ── createDir(path) → Result ─────────────────────
            "createDir" => {
                let path = self.eval_os_str_arg(args, 0, "createDir", "path")?;

                match std::fs::create_dir_all(&path) {
                    Ok(_) => Ok(Some(Signal::Value(make_result_success(ok_inner())))),
                    Err(e) => Ok(Some(Signal::Value(make_result_failure(&e)))),
                }
            }

            // ── rename(from, to) → Result ────────────────────
            "rename" => {
                let from = self.eval_os_str_arg(args, 0, "rename", "from")?;
                let to = self.eval_os_str_arg(args, 1, "rename", "to")?;

                match std::fs::rename(&from, &to) {
                    Ok(_) => Ok(Some(Signal::Value(make_result_success(ok_inner())))),
                    Err(e) => Ok(Some(Signal::Value(make_result_failure(&e)))),
                }
            }

            // ── run(program, args) → Gorillax[@(stdout, stderr, code)] ──
            "run" => {
                let program = self.eval_os_str_arg(args, 0, "run", "program")?;

                // Second argument: list of strings
                let cmd_args = if let Some(arg) = args.get(1) {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => {
                            let mut strs = Vec::new();
                            for item in &items {
                                if let Value::Str(s) = item {
                                    strs.push(s.clone());
                                } else {
                                    strs.push(item.to_display_string());
                                }
                            }
                            strs
                        }
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };

                match std::process::Command::new(&program)
                    .args(&cmd_args)
                    .output()
                {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let code = output.status.code().unwrap_or(-1) as i64;
                        let inner = process_inner(stdout, stderr, code);

                        if code == 0 {
                            Ok(Some(Signal::Value(make_gorillax_success(inner))))
                        } else {
                            let error_val = make_process_error(
                                format!("Process '{}' exited with code {}", program, code),
                                code,
                            );
                            Ok(Some(Signal::Value(make_gorillax_failure(error_val))))
                        }
                    }
                    Err(e) => Ok(Some(Signal::Value(make_gorillax_failure(make_io_error(
                        &e,
                    ))))),
                }
            }

            // ── execShell(command) → Gorillax[@(stdout, stderr, code)] ──
            "execShell" => {
                let command = self.eval_os_str_arg(args, 0, "execShell", "command")?;

                let result = if cfg!(target_os = "windows") {
                    std::process::Command::new("cmd")
                        .args(["/C", &command])
                        .output()
                } else {
                    std::process::Command::new("sh")
                        .args(["-c", &command])
                        .output()
                };

                match result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let code = output.status.code().unwrap_or(-1) as i64;

                        if code == 0 {
                            let inner = process_inner(stdout, stderr, code);
                            Ok(Some(Signal::Value(make_gorillax_success(inner))))
                        } else {
                            let error_val = make_process_error(
                                format!("Shell command exited with code {}: {}", code, command),
                                code,
                            );
                            Ok(Some(Signal::Value(make_gorillax_failure(error_val))))
                        }
                    }
                    Err(e) => Ok(Some(Signal::Value(make_gorillax_failure(make_io_error(
                        &e,
                    ))))),
                }
            }

            // ── allEnv() → HashMap[Str, Str] ─────────────────
            "allEnv" => {
                let entries: Vec<Value> = std::env::vars()
                    .map(|(k, v)| {
                        Value::BuchiPack(vec![
                            ("key".into(), Value::Str(k)),
                            ("value".into(), Value::Str(v)),
                        ])
                    })
                    .collect();

                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("__entries".into(), Value::List(entries)),
                    ("__type".into(), Value::Str("HashMap".into())),
                ]))))
            }

            // ── argv() → @[Str] (CLI user args) ──────────────
            "argv" => {
                // Interpreter mode is typically: taida <script.td> [args...]
                // Expose only user args to match JS/native runtime behavior.
                let argv: Vec<Value> = std::env::args().skip(2).map(Value::Str).collect();
                Ok(Some(Signal::Value(Value::List(argv))))
            }

            // ── dnsResolve(host[, timeoutMs]) → Async[Result[@(addresses: @[Str]), _]] ──
            "dnsResolve" => {
                let host = self.eval_os_str_arg(args, 0, "dnsResolve", "host")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 1, "dnsResolve")?;

                let rt = self.tokio_runtime.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                rt.spawn(async move {
                    let resolve_future = tokio::net::lookup_host((host.as_str(), 0));
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        resolve_future,
                    )
                    .await
                    {
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("dnsResolve: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Ok(addrs)) => {
                            let mut seen = std::collections::HashSet::new();
                            let mut out = Vec::new();
                            for addr in addrs {
                                let ip = addr.ip().to_string();
                                if seen.insert(ip.clone()) {
                                    out.push(Value::Str(ip));
                                }
                            }

                            if out.is_empty() {
                                let e = std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    format!("dnsResolve: no records for '{}'", host),
                                );
                                let _ = tx.send(Ok(make_result_failure(&e)));
                                return;
                            }

                            let inner =
                                Value::BuchiPack(vec![("addresses".into(), Value::List(out))]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── tcpConnect(host, port) → Async[Result[@(socket: Int, ...), _]] ──
            "tcpConnect" => {
                let host = self.eval_os_str_arg(args, 0, "tcpConnect", "host")?;
                let port = match args.get(1) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n as u16,
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!("tcpConnect: port must be an Int, got {}", v),
                            });
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "tcpConnect: missing argument 'port'".into(),
                        });
                    }
                };
                let timeout_ms = self.eval_os_timeout_arg(args, 2, "tcpConnect")?;

                let rt = self.tokio_runtime.clone();
                let socket_handles = self.socket_handles.clone();
                let next_socket_id = self.next_socket_id.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                rt.spawn(async move {
                    let connect_future = tokio::net::TcpStream::connect((host.as_str(), port));
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        connect_future,
                    )
                    .await
                    {
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("tcpConnect: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Ok(stream)) => {
                            let socket_id = next_socket_id.fetch_add(1, Ordering::Relaxed);
                            let stream_handle = Arc::new(tokio::sync::Mutex::new(stream));
                            match socket_handles.lock() {
                                Ok(mut table) => {
                                    table.insert(socket_id, stream_handle);
                                }
                                Err(_) => {
                                    let e = std::io::Error::other(
                                        "tcpConnect: socket handle table is unavailable",
                                    );
                                    let _ = tx.send(Ok(make_result_failure(&e)));
                                    return;
                                }
                            }

                            let inner = Value::BuchiPack(vec![
                                ("socket".into(), Value::Int(socket_id)),
                                ("host".into(), Value::Str(host)),
                                ("port".into(), Value::Int(port as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── tcpListen(port) → Async[Result[@(listener: Int, ...), _]] ──
            "tcpListen" => {
                let port = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n as u16,
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!("tcpListen: port must be an Int, got {}", v),
                            });
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "tcpListen: missing argument 'port'".into(),
                        });
                    }
                };
                let timeout_ms = self.eval_os_timeout_arg(args, 1, "tcpListen")?;

                let rt = self.tokio_runtime.clone();
                let listener_handles = self.listener_handles.clone();
                let next_listener_id = self.next_listener_id.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                rt.spawn(async move {
                    // RCB-305: Default to loopback (127.0.0.1) instead of all interfaces (0.0.0.0)
                    let addr = format!("127.0.0.1:{}", port);
                    let bind_future = tokio::net::TcpListener::bind(&addr);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        bind_future,
                    )
                    .await
                    {
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("tcpListen: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Ok(listener)) => {
                            let listener_id = next_listener_id.fetch_add(1, Ordering::Relaxed);
                            let listener_handle = Arc::new(tokio::sync::Mutex::new(listener));
                            match listener_handles.lock() {
                                Ok(mut table) => {
                                    table.insert(listener_id, listener_handle);
                                }
                                Err(_) => {
                                    let e = std::io::Error::other(
                                        "tcpListen: listener handle table is unavailable",
                                    );
                                    let _ = tx.send(Ok(make_result_failure(&e)));
                                    return;
                                }
                            }
                            let inner = Value::BuchiPack(vec![
                                ("listener".into(), Value::Int(listener_id)),
                                ("port".into(), Value::Int(port as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── tcpAccept(listener_fd) → Async[Result[@(socket: Int, host: Str, port: Int), _]] ──
            "tcpAccept" => {
                let listener_fd = self.eval_os_handle_arg(args, 0, "tcpAccept", "listener")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 1, "tcpAccept")?;
                let listener_handle = self
                    .listener_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&listener_fd).cloned());
                let socket_handles = self.socket_handles.clone();
                let next_socket_id = self.next_socket_id.clone();

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let Some(listener_handle) = listener_handle else {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "tcpAccept: unknown listener handle",
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                        return;
                    };

                    let listener = listener_handle.lock().await;
                    let accept_future = listener.accept();
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        accept_future,
                    )
                    .await
                    {
                        Ok(Ok((stream, peer_addr))) => {
                            let socket_id = next_socket_id.fetch_add(1, Ordering::Relaxed);
                            let stream_handle = Arc::new(tokio::sync::Mutex::new(stream));
                            match socket_handles.lock() {
                                Ok(mut table) => {
                                    table.insert(socket_id, stream_handle);
                                }
                                Err(_) => {
                                    let e = std::io::Error::other(
                                        "tcpAccept: socket handle table is unavailable",
                                    );
                                    let _ = tx.send(Ok(make_result_failure(&e)));
                                    return;
                                }
                            }

                            let inner = Value::BuchiPack(vec![
                                ("socket".into(), Value::Int(socket_id)),
                                ("host".into(), Value::Str(peer_addr.ip().to_string())),
                                ("port".into(), Value::Int(peer_addr.port() as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("tcpAccept: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketSend(socket_fd, data) → Async[Result[@(ok, ...), _]] ──
            "socketSend" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "socketSend", "socket")?;
                let data = self.eval_os_str_arg(args, 1, "socketSend", "data")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 2, "socketSend")?;
                let socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncWriteExt;

                    let Some(stream_handle) = socket_handle else {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "socketSend: unknown socket handle",
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                        return;
                    };

                    let mut stream = stream_handle.lock().await;
                    let write_future = stream.write_all(data.as_bytes());
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        write_future,
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            let inner = Value::BuchiPack(vec![
                                ("ok".into(), Value::Bool(true)),
                                ("bytesSent".into(), Value::Int(data.len() as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("socketSend: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketSendAll(socket_fd, data) → Async[Result[@(ok, ...), _]] ──
            "socketSendAll" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "socketSendAll", "socket")?;
                let data = self.eval_os_bytes_arg(args, 1, "socketSendAll", "data")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 2, "socketSendAll")?;
                let socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncWriteExt;

                    let Some(stream_handle) = socket_handle else {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "socketSendAll: unknown socket handle",
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                        return;
                    };

                    let mut stream = stream_handle.lock().await;
                    let write_future = stream.write_all(&data);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        write_future,
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            let inner = Value::BuchiPack(vec![
                                ("ok".into(), Value::Bool(true)),
                                ("bytesSent".into(), Value::Int(data.len() as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("socketSendAll: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketRecv(socket_fd) → Async[Lax[Str]] ──
            "socketRecv" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "socketRecv", "socket")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 1, "socketRecv")?;
                let socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncReadExt;

                    let Some(stream_handle) = socket_handle else {
                        let _ = tx.send(Ok(make_lax_failure(Value::Str(String::new()))));
                        return;
                    };

                    let mut stream = stream_handle.lock().await;
                    let mut buf = vec![0u8; 65536];
                    let read_future = stream.read(&mut buf);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        read_future,
                    )
                    .await
                    {
                        Ok(Ok(0)) => {
                            let _ = tx.send(Ok(make_lax_failure(Value::Str(String::new()))));
                        }
                        Ok(Ok(n)) => {
                            let data = String::from_utf8_lossy(&buf[..n]).to_string();
                            let _ = tx.send(Ok(make_lax_success(Value::Str(data))));
                        }
                        Ok(Err(_)) | Err(_) => {
                            let _ = tx.send(Ok(make_lax_failure(Value::Str(String::new()))));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketSendBytes(socket_fd, data) → Async[Result[@(ok, ...), _]] ──
            "socketSendBytes" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "socketSendBytes", "socket")?;
                let data = self.eval_os_bytes_arg(args, 1, "socketSendBytes", "data")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 2, "socketSendBytes")?;
                let socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncWriteExt;

                    let Some(stream_handle) = socket_handle else {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "socketSendBytes: unknown socket handle",
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                        return;
                    };

                    let mut stream = stream_handle.lock().await;
                    let write_future = stream.write_all(&data);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        write_future,
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            let inner = Value::BuchiPack(vec![
                                ("ok".into(), Value::Bool(true)),
                                ("bytesSent".into(), Value::Int(data.len() as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("socketSendBytes: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketRecvBytes(socket_fd) → Async[Lax[Bytes]] ──
            "socketRecvBytes" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "socketRecvBytes", "socket")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 1, "socketRecvBytes")?;
                let socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncReadExt;

                    let Some(stream_handle) = socket_handle else {
                        let _ = tx.send(Ok(make_lax_failure(Value::Bytes(Vec::new()))));
                        return;
                    };

                    let mut stream = stream_handle.lock().await;
                    let mut buf = vec![0u8; 65536];
                    let read_future = stream.read(&mut buf);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        read_future,
                    )
                    .await
                    {
                        Ok(Ok(0)) => {
                            let _ = tx.send(Ok(make_lax_failure(Value::Bytes(Vec::new()))));
                        }
                        Ok(Ok(n)) => {
                            let _ = tx.send(Ok(make_lax_success(Value::Bytes(buf[..n].to_vec()))));
                        }
                        Ok(Err(_)) | Err(_) => {
                            let _ = tx.send(Ok(make_lax_failure(Value::Bytes(Vec::new()))));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketRecvExact(socket_fd, size) → Async[Lax[Bytes]] ──
            "socketRecvExact" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "socketRecvExact", "socket")?;
                let size = match args.get(1) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) if n >= 0 => n as usize,
                        Signal::Value(Value::Int(n)) => {
                            return Err(RuntimeError {
                                message: format!(
                                    "socketRecvExact: size must be a non-negative Int, got {}",
                                    n
                                ),
                            });
                        }
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!("socketRecvExact: size must be an Int, got {}", v),
                            });
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "socketRecvExact: missing argument 'size'".into(),
                        });
                    }
                };
                let timeout_ms = self.eval_os_timeout_arg(args, 2, "socketRecvExact")?;
                let socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncReadExt;

                    let Some(stream_handle) = socket_handle else {
                        let _ = tx.send(Ok(make_lax_failure(Value::Bytes(Vec::new()))));
                        return;
                    };

                    let mut stream = stream_handle.lock().await;
                    let mut buf = vec![0u8; size];
                    let read_future = stream.read_exact(&mut buf);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        read_future,
                    )
                    .await
                    {
                        Ok(Ok(_)) => {
                            let _ = tx.send(Ok(make_lax_success(Value::Bytes(buf))));
                        }
                        Ok(Err(_)) | Err(_) => {
                            let _ = tx.send(Ok(make_lax_failure(Value::Bytes(Vec::new()))));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── udpBind(host, port) → Async[Result[@(socket: Int, host: Str, port: Int), _]] ──
            "udpBind" => {
                let host = self.eval_os_str_arg(args, 0, "udpBind", "host")?;
                let port = match args.get(1) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n as u16,
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!("udpBind: port must be an Int, got {}", v),
                            });
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "udpBind: missing argument 'port'".into(),
                        });
                    }
                };
                let timeout_ms = self.eval_os_timeout_arg(args, 2, "udpBind")?;

                let rt = self.tokio_runtime.clone();
                let udp_socket_handles = self.udp_socket_handles.clone();
                let next_socket_id = self.next_socket_id.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                rt.spawn(async move {
                    let addr = format!("{}:{}", host, port);
                    let bind_future = tokio::net::UdpSocket::bind(&addr);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        bind_future,
                    )
                    .await
                    {
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("udpBind: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Ok(Ok(socket)) => {
                            let socket_id = next_socket_id.fetch_add(1, Ordering::Relaxed);
                            let socket_handle = Arc::new(tokio::sync::Mutex::new(socket));
                            match udp_socket_handles.lock() {
                                Ok(mut table) => {
                                    table.insert(socket_id, socket_handle);
                                }
                                Err(_) => {
                                    let e = std::io::Error::other(
                                        "udpBind: udp socket handle table is unavailable",
                                    );
                                    let _ = tx.send(Ok(make_result_failure(&e)));
                                    return;
                                }
                            }
                            let inner = Value::BuchiPack(vec![
                                ("socket".into(), Value::Int(socket_id)),
                                ("host".into(), Value::Str(host)),
                                ("port".into(), Value::Int(port as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── udpSendTo(socket, host, port, data) → Async[Result[@(ok,bytesSent), _]] ──
            "udpSendTo" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "udpSendTo", "socket")?;
                let host = self.eval_os_str_arg(args, 1, "udpSendTo", "host")?;
                let port = match args.get(2) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n as u16,
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!("udpSendTo: port must be an Int, got {}", v),
                            });
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "udpSendTo: missing argument 'port'".into(),
                        });
                    }
                };
                let data = self.eval_os_bytes_arg(args, 3, "udpSendTo", "data")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 4, "udpSendTo")?;
                let udp_handle = self
                    .udp_socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let Some(socket_handle) = udp_handle else {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "udpSendTo: unknown socket handle",
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                        return;
                    };

                    let socket = socket_handle.lock().await;
                    let send_future = socket.send_to(&data, format!("{}:{}", host, port));
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        send_future,
                    )
                    .await
                    {
                        Ok(Ok(bytes_sent)) => {
                            let inner = Value::BuchiPack(vec![
                                ("ok".into(), Value::Bool(true)),
                                ("bytesSent".into(), Value::Int(bytes_sent as i64)),
                            ]);
                            let _ = tx.send(Ok(make_result_success(inner)));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                        Err(_) => {
                            let e = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("udpSendTo: timed out after {}ms", timeout_ms),
                            );
                            let _ = tx.send(Ok(make_result_failure(&e)));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── udpRecvFrom(socket) → Async[Lax[@(host,port,data,truncated)]] ──
            "udpRecvFrom" => {
                let socket_fd = self.eval_os_handle_arg(args, 0, "udpRecvFrom", "socket")?;
                let timeout_ms = self.eval_os_timeout_arg(args, 1, "udpRecvFrom")?;
                let udp_handle = self
                    .udp_socket_handles
                    .lock()
                    .ok()
                    .and_then(|table| table.get(&socket_fd).cloned());

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let Some(socket_handle) = udp_handle else {
                        let _ = tx.send(Ok(make_lax_failure(make_udp_recv_default_payload())));
                        return;
                    };

                    let socket = socket_handle.lock().await;
                    let mut buf = vec![0u8; 65_507];
                    let recv_future = socket.recv_from(&mut buf);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        recv_future,
                    )
                    .await
                    {
                        Ok(Ok((n, peer))) => {
                            let payload = Value::BuchiPack(vec![
                                ("host".into(), Value::Str(peer.ip().to_string())),
                                ("port".into(), Value::Int(peer.port() as i64)),
                                ("data".into(), Value::Bytes(buf[..n].to_vec())),
                                ("truncated".into(), Value::Bool(false)),
                            ]);
                            let _ = tx.send(Ok(make_lax_success(payload)));
                        }
                        Ok(Err(_)) | Err(_) => {
                            let _ = tx.send(Ok(make_lax_failure(make_udp_recv_default_payload())));
                        }
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── socketClose(socket_fd) / udpClose(socket_fd) → Async[Result[@(ok,code,message), _]] ──
            name @ ("socketClose" | "udpClose") => {
                let socket_fd = self.eval_os_handle_arg(args, 0, name, "socket")?;
                let tcp_socket_handle = self
                    .socket_handles
                    .lock()
                    .ok()
                    .and_then(|mut table| table.remove(&socket_fd));
                let udp_socket_handle = self
                    .udp_socket_handles
                    .lock()
                    .ok()
                    .and_then(|mut table| table.remove(&socket_fd));

                let op_name = name.to_string();
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    use tokio::io::AsyncWriteExt;

                    if let Some(stream_handle) = tcp_socket_handle {
                        let mut stream = stream_handle.lock().await;
                        match stream.shutdown().await {
                            Ok(()) => {
                                let _ = tx.send(Ok(make_result_success(ok_inner())));
                            }
                            Err(e) => {
                                let _ = tx.send(Ok(make_result_failure(&e)));
                            }
                        }
                        return;
                    }

                    if let Some(socket_handle) = udp_socket_handle {
                        drop(socket_handle);
                        let _ = tx.send(Ok(make_result_success(ok_inner())));
                        return;
                    }

                    {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("{}: unknown socket handle", op_name),
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                    }
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── listenerClose(listener_fd) → Async[Result[@(ok,code,message), _]] ──
            "listenerClose" => {
                let listener_fd = self.eval_os_handle_arg(args, 0, "listenerClose", "listener")?;
                let listener_handle = self
                    .listener_handles
                    .lock()
                    .ok()
                    .and_then(|mut table| table.remove(&listener_fd));

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    let Some(handle) = listener_handle else {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "listenerClose: unknown listener handle",
                        );
                        let _ = tx.send(Ok(make_result_failure(&e)));
                        return;
                    };
                    drop(handle);
                    let _ = tx.send(Ok(make_result_success(ok_inner())));
                });
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            _ => Ok(None),
        }
    }

    pub(crate) fn try_pool_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        match name {
            "poolCreate" => {
                let config = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "poolCreate: missing argument 'config'".into(),
                        });
                    }
                };

                let fields = match config {
                    Value::BuchiPack(fields) => fields,
                    other => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid",
                            format!("poolCreate: config must be a pack, got {}", other),
                        ))));
                    }
                };

                let read_int = |key: &str, default: i64| -> Result<i64, String> {
                    match fields.iter().find(|(k, _)| k == key) {
                        Some((_, Value::Int(n))) => Ok(*n),
                        Some((_, v)) => {
                            Err(format!("poolCreate: '{}' must be Int, got {}", key, v))
                        }
                        None => Ok(default),
                    }
                };

                let max_size = match read_int("maxSize", 10) {
                    Ok(v) if v > 0 => v,
                    Ok(v) => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid",
                            format!("poolCreate: maxSize must be > 0, got {}", v),
                        ))));
                    }
                    Err(msg) => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid", msg,
                        ))));
                    }
                };

                let mut max_idle = match read_int("maxIdle", max_size) {
                    Ok(v) if v >= 0 => v,
                    Ok(v) => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid",
                            format!("poolCreate: maxIdle must be >= 0, got {}", v),
                        ))));
                    }
                    Err(msg) => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid", msg,
                        ))));
                    }
                };
                if max_idle > max_size {
                    max_idle = max_size;
                }

                let acquire_timeout_ms = match read_int("acquireTimeoutMs", 30_000) {
                    Ok(v) if v > 0 => v,
                    Ok(v) => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid",
                            format!("poolCreate: acquireTimeoutMs must be > 0, got {}", v),
                        ))));
                    }
                    Err(msg) => {
                        return Ok(Some(Signal::Value(make_result_failure_with_kind(
                            "invalid", msg,
                        ))));
                    }
                };

                let pool_id = self.next_pool_id.fetch_add(1, Ordering::Relaxed);
                let state = super::eval::PoolState {
                    open: true,
                    max_size,
                    max_idle,
                    acquire_timeout_ms,
                    idle: Vec::new(),
                    in_use_tokens: std::collections::HashSet::new(),
                    next_token: 1,
                };
                self.pool_states
                    .lock()
                    .map_err(|_| RuntimeError {
                        message: "poolCreate: internal pool table lock error".to_string(),
                    })?
                    .insert(pool_id, state);

                let inner = Value::BuchiPack(vec![("pool".into(), Value::Int(pool_id))]);
                Ok(Some(Signal::Value(make_result_success(inner))))
            }

            "poolAcquire" => {
                let pool_id = self.eval_os_handle_arg(args, 0, "poolAcquire", "pool")?;

                let explicit_timeout = match args.get(1) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(ms)) if ms > 0 => Some(ms),
                        Signal::Value(Value::Int(ms)) => {
                            return Ok(Some(Signal::Value(make_async_fulfilled(
                                make_result_failure_with_kind(
                                    "invalid",
                                    format!("poolAcquire: timeoutMs must be > 0, got {}", ms),
                                ),
                            ))));
                        }
                        Signal::Value(v) => {
                            return Ok(Some(Signal::Value(make_async_fulfilled(
                                make_result_failure_with_kind(
                                    "invalid",
                                    format!("poolAcquire: timeoutMs must be Int, got {}", v),
                                ),
                            ))));
                        }
                        other => return Ok(Some(other)),
                    },
                    None => None,
                };

                let mut table = self.pool_states.lock().map_err(|_| RuntimeError {
                    message: "poolAcquire: internal pool table lock error".to_string(),
                })?;
                let Some(state) = table.get_mut(&pool_id) else {
                    return Ok(Some(Signal::Value(make_async_fulfilled(
                        make_result_failure_with_kind(
                            "invalid",
                            "poolAcquire: unknown pool handle",
                        ),
                    ))));
                };
                if !state.open {
                    return Ok(Some(Signal::Value(make_async_fulfilled(
                        make_result_failure_with_kind("closed", "poolAcquire: pool is closed"),
                    ))));
                }

                let timeout_ms = explicit_timeout.unwrap_or(state.acquire_timeout_ms);
                let (resource, token) = if let Some(entry) = state.idle.pop() {
                    (entry.resource, entry.token)
                } else if (state.in_use_tokens.len() as i64) < state.max_size {
                    let token = state.next_token;
                    state.next_token += 1;
                    (Value::Unit, token)
                } else {
                    return Ok(Some(Signal::Value(make_async_fulfilled(
                        make_result_failure_with_kind(
                            "timeout",
                            format!("poolAcquire: timed out after {}ms", timeout_ms),
                        ),
                    ))));
                };
                state.in_use_tokens.insert(token);

                let inner = Value::BuchiPack(vec![
                    ("resource".into(), resource),
                    ("token".into(), Value::Int(token)),
                ]);
                Ok(Some(Signal::Value(make_async_fulfilled(
                    make_result_success(inner),
                ))))
            }

            "poolRelease" => {
                let pool_id = self.eval_os_handle_arg(args, 0, "poolRelease", "pool")?;
                let token = match args.get(1) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n,
                        Signal::Value(v) => {
                            return Ok(Some(Signal::Value(make_result_failure_with_kind(
                                "invalid",
                                format!("poolRelease: token must be Int, got {}", v),
                            ))));
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "poolRelease: missing argument 'token'".into(),
                        });
                    }
                };
                let resource = match args.get(2) {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "poolRelease: missing argument 'resource'".into(),
                        });
                    }
                };

                let mut table = self.pool_states.lock().map_err(|_| RuntimeError {
                    message: "poolRelease: internal pool table lock error".to_string(),
                })?;
                let Some(state) = table.get_mut(&pool_id) else {
                    return Ok(Some(Signal::Value(make_result_failure_with_kind(
                        "invalid",
                        "poolRelease: unknown pool handle",
                    ))));
                };
                if !state.open {
                    return Ok(Some(Signal::Value(make_result_failure_with_kind(
                        "closed",
                        "poolRelease: pool is closed",
                    ))));
                }
                if !state.in_use_tokens.remove(&token) {
                    return Ok(Some(Signal::Value(make_result_failure_with_kind(
                        "invalid",
                        "poolRelease: token is not in-use",
                    ))));
                }

                let mut reused = false;
                if (state.idle.len() as i64) < state.max_idle {
                    state.idle.push(super::eval::PoolEntry { token, resource });
                    reused = true;
                }

                let inner = Value::BuchiPack(vec![
                    ("ok".into(), Value::Bool(true)),
                    ("reused".into(), Value::Bool(reused)),
                ]);
                Ok(Some(Signal::Value(make_result_success(inner))))
            }

            "poolClose" => {
                let pool_id = self.eval_os_handle_arg(args, 0, "poolClose", "pool")?;
                let mut table = self.pool_states.lock().map_err(|_| RuntimeError {
                    message: "poolClose: internal pool table lock error".to_string(),
                })?;
                let Some(state) = table.get_mut(&pool_id) else {
                    return Ok(Some(Signal::Value(make_async_fulfilled(
                        make_result_failure_with_kind("invalid", "poolClose: unknown pool handle"),
                    ))));
                };
                if !state.open {
                    return Ok(Some(Signal::Value(make_async_fulfilled(
                        make_result_failure_with_kind("closed", "poolClose: pool already closed"),
                    ))));
                }
                state.open = false;
                state.idle.clear();
                state.in_use_tokens.clear();

                let inner = Value::BuchiPack(vec![("ok".into(), Value::Bool(true))]);
                Ok(Some(Signal::Value(make_async_fulfilled(
                    make_result_success(inner),
                ))))
            }

            "poolHealth" => {
                let pool_id = self.eval_os_handle_arg(args, 0, "poolHealth", "pool")?;
                let table = self.pool_states.lock().map_err(|_| RuntimeError {
                    message: "poolHealth: internal pool table lock error".to_string(),
                })?;
                let Some(state) = table.get(&pool_id) else {
                    return Err(RuntimeError {
                        message: "poolHealth: unknown pool handle".to_string(),
                    });
                };
                let health = Value::BuchiPack(vec![
                    ("open".into(), Value::Bool(state.open)),
                    ("idle".into(), Value::Int(state.idle.len() as i64)),
                    ("inUse".into(), Value::Int(state.in_use_tokens.len() as i64)),
                    ("waiting".into(), Value::Int(0)),
                ]);
                Ok(Some(Signal::Value(health)))
            }

            _ => Ok(None),
        }
    }

    /// Helper: evaluate a socket/listener handle argument.
    /// Accepts either a raw Int handle or a pack with `{field_name: Int}`.
    fn eval_os_handle_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        func_name: &str,
        field_name: &str,
    ) -> Result<i64, RuntimeError> {
        let arg = args.get(index).ok_or_else(|| RuntimeError {
            message: format!("{}: missing argument '{}'", func_name, field_name),
        })?;
        match self.eval_expr(arg)? {
            Signal::Value(Value::Int(n)) => Ok(n),
            Signal::Value(Value::BuchiPack(fields)) => fields
                .iter()
                .find(|(name, _)| name == field_name)
                .and_then(|(_, v)| match v {
                    Value::Int(n) => Some(*n),
                    _ => None,
                })
                .ok_or_else(|| RuntimeError {
                    message: format!(
                        "{}: first argument must be an Int ({}) or @({}: Int, ...)",
                        func_name, field_name, field_name
                    ),
                }),
            Signal::Value(v) => Err(RuntimeError {
                message: format!(
                    "{}: first argument must be an Int ({}), got {}",
                    func_name, field_name, v
                ),
            }),
            other => Err(RuntimeError {
                message: format!(
                    "{}: unexpected signal evaluating '{}': {}",
                    func_name,
                    field_name,
                    signal_name(&other)
                ),
            }),
        }
    }

    /// Helper: evaluate a string argument at a given index for os functions.
    fn eval_os_str_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        func_name: &str,
        arg_name: &str,
    ) -> Result<String, RuntimeError> {
        let arg = args.get(index).ok_or_else(|| RuntimeError {
            message: format!("{}: missing argument '{}'", func_name, arg_name),
        })?;
        match self.eval_expr(arg)? {
            Signal::Value(Value::Str(s)) => Ok(s),
            Signal::Value(v) => Err(RuntimeError {
                message: format!("{}: {} must be a string, got {}", func_name, arg_name, v),
            }),
            other => Err(RuntimeError {
                message: format!(
                    "{}: unexpected signal evaluating '{}': {}",
                    func_name,
                    arg_name,
                    signal_name(&other)
                ),
            }),
        }
    }

    /// Helper: evaluate a bytes argument for os functions.
    /// Accepts Bytes directly. For backward compatibility, Str is accepted as UTF-8 bytes.
    fn eval_os_bytes_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        func_name: &str,
        arg_name: &str,
    ) -> Result<Vec<u8>, RuntimeError> {
        let arg = args.get(index).ok_or_else(|| RuntimeError {
            message: format!("{}: missing argument '{}'", func_name, arg_name),
        })?;
        match self.eval_expr(arg)? {
            Signal::Value(Value::Bytes(bytes)) => Ok(bytes),
            Signal::Value(Value::Str(s)) => Ok(s.into_bytes()),
            Signal::Value(v) => Err(RuntimeError {
                message: format!("{}: {} must be Bytes, got {}", func_name, arg_name, v),
            }),
            other => Err(RuntimeError {
                message: format!(
                    "{}: unexpected signal evaluating '{}': {}",
                    func_name,
                    arg_name,
                    signal_name(&other)
                ),
            }),
        }
    }

    /// Helper: evaluate optional timeout argument in milliseconds.
    /// If omitted, returns DEFAULT_NETWORK_TIMEOUT_MS.
    fn eval_os_timeout_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        func_name: &str,
    ) -> Result<u64, RuntimeError> {
        let Some(arg) = args.get(index) else {
            return Ok(DEFAULT_NETWORK_TIMEOUT_MS);
        };
        match self.eval_expr(arg)? {
            Signal::Value(Value::Int(ms)) if ms > 0 => Ok(ms as u64),
            Signal::Value(Value::Int(ms)) => Err(RuntimeError {
                message: format!(
                    "{}: timeoutMs must be a positive Int, got {}",
                    func_name, ms
                ),
            }),
            Signal::Value(v) => Err(RuntimeError {
                message: format!("{}: timeoutMs must be an Int, got {}", func_name, v),
            }),
            other => Err(RuntimeError {
                message: format!(
                    "{}: unexpected signal evaluating 'timeoutMs': {}",
                    func_name,
                    signal_name(&other)
                ),
            }),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper: extract Lax fields ──

    fn lax_has_value(val: &Value) -> bool {
        if let Value::BuchiPack(fields) = val {
            for (name, v) in fields {
                if name == "hasValue"
                    && let Value::Bool(b) = v
                {
                    return *b;
                }
            }
        }
        false
    }

    fn lax_value(val: &Value) -> &Value {
        if let Value::BuchiPack(fields) = val {
            for (name, v) in fields {
                if name == "__value" {
                    return v;
                }
            }
        }
        &Value::Unit
    }

    fn result_is_success(val: &Value) -> bool {
        if let Value::BuchiPack(fields) = val {
            for (name, v) in fields {
                if name == "throw" {
                    return matches!(v, Value::Unit);
                }
            }
        }
        false
    }

    fn result_inner(val: &Value) -> &Value {
        if let Value::BuchiPack(fields) = val {
            for (name, v) in fields {
                if name == "__value" {
                    return v;
                }
            }
        }
        &Value::Unit
    }

    fn pack_field<'a>(val: &'a Value, field: &str) -> &'a Value {
        if let Value::BuchiPack(fields) = val {
            for (name, v) in fields {
                if name == field {
                    return v;
                }
            }
        }
        &Value::Unit
    }

    // ── Helper functions tests ──

    #[test]
    fn test_make_lax_success() {
        let val = make_lax_success(Value::Str("hello".into()));
        assert!(lax_has_value(&val));
        assert_eq!(lax_value(&val).to_display_string(), "hello");
    }

    #[test]
    fn test_make_lax_failure() {
        let val = make_lax_failure(Value::Str(String::new()));
        assert!(!lax_has_value(&val));
    }

    #[test]
    fn test_make_result_success() {
        let val = make_result_success(ok_inner());
        assert!(result_is_success(&val));
        let inner = result_inner(&val);
        let Value::Bool(b) = pack_field(inner, "ok") else {
            unreachable!("Expected Bool for ok field");
        };
        assert!(b);
    }

    #[test]
    fn test_make_result_failure() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let val = make_result_failure(&err);
        assert!(!result_is_success(&val));
    }

    #[test]
    fn test_format_rfc3339_utc_epoch() {
        let epoch = std::time::UNIX_EPOCH;
        assert_eq!(format_rfc3339_utc(epoch), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn test_format_rfc3339_utc_known_date() {
        // 2025-01-01T00:00:00Z = 1735689600 seconds since epoch
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1735689600);
        let s = format_rfc3339_utc(t);
        assert_eq!(s, "2025-01-01T00:00:00Z");
    }

    // ── Integration tests using Interpreter ──

    fn run_code(code: &str) -> Vec<String> {
        let (program, errors) = crate::parser::parse(code);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interp = Interpreter::new();
        match interp.eval_program(&program) {
            Ok(_) => {}
            Err(e) => unreachable!("Unexpected runtime error: {}", e),
        }
        interp.output.clone()
    }

    fn run_code_result(code: &str) -> Result<Vec<String>, String> {
        let (program, errors) = crate::parser::parse(code);
        if !errors.is_empty() {
            return Err(format!("Parse errors: {:?}", errors));
        }
        let mut interp = Interpreter::new();
        match interp.eval_program(&program) {
            Ok(_) => Ok(interp.output.clone()),
            Err(e) => Err(format!("{}", e)),
        }
    }

    // ── Read tests ──

    #[test]
    fn test_read_existing_file() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_read");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.txt"), "Hello Taida").unwrap();

        let path = dir.join("test.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= Read["{}"]()
stdout(result.hasValue)
stdout(result.__value)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true", "Hello Taida"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_nonexistent_file() {
        let code = r#"result <= Read["/tmp/taida_nonexistent_file_xyz.txt"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── ListDir tests ──

    #[test]
    fn test_listdir() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_listdir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();

        let path = dir.to_string_lossy().to_string();
        let code = format!(
            r#"result <= ListDir["{}"]()
stdout(result.hasValue)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output[0], "true");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_listdir_nonexistent() {
        let code = r#"result <= ListDir["/tmp/taida_nonexistent_dir_xyz"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── Stat tests ──

    #[test]
    fn test_stat_file() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_stat");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data.txt"), "12345").unwrap();

        let path = dir.join("data.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= Stat["{}"]()
stdout(result.hasValue)
stdout(result.__value.size)
stdout(result.__value.isDir)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output[0], "true");
        assert_eq!(output[1], "5");
        assert_eq!(output[2], "false");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_stat_directory() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_stat_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.to_string_lossy().to_string();
        let code = format!(
            r#"result <= Stat["{}"]()
stdout(result.__value.isDir)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_stat_nonexistent() {
        let code = r#"result <= Stat["/tmp/taida_nonexistent_xyz"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── Exists tests ──

    #[test]
    fn test_exists_true() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_exists");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("yes.txt"), "").unwrap();

        let path = dir.join("yes.txt").to_string_lossy().to_string();
        let code = format!(r#"stdout(Exists["{}"]())"#, path);
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_exists_false() {
        let code = r#"stdout(Exists["/tmp/taida_nonexistent_xyz"]())"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── EnvVar tests ──

    #[test]
    fn test_envvar_exists() {
        // PATH should always exist
        let code = r#"result <= EnvVar["PATH"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["true"]);
    }

    #[test]
    fn test_envvar_missing() {
        let code = r#"result <= EnvVar["TAIDA_NONEXISTENT_VAR_XYZ"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── writeFile tests ──

    #[test]
    fn test_writefile() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_writefile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("out.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= writeFile("{}", "Hello!")
stdout(result.__value.ok)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);

        let content = std::fs::read_to_string(dir.join("out.txt")).unwrap();
        assert_eq!(content, "Hello!");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_readbytes_writebytes_roundtrip() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_bytes_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("bytes.bin").to_string_lossy().to_string();
        let code = format!(
            r#"payloadLax <= Bytes["hello"]()
payloadLax ]=> payload
writeRes <= writeBytes("{}", payload)
stdout(writeRes.__value.ok)
readRes <= readBytes("{}")
stdout(readRes.hasValue)
decoded <= Utf8Decode[readRes.__value]()
decoded ]=> txt
stdout(txt)"#,
            path, path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true", "true", "hello"]);

        let content = std::fs::read(dir.join("bytes.bin")).unwrap();
        assert_eq!(content, b"hello");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── appendFile tests ──

    #[test]
    fn test_appendfile() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_appendfile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("log.txt"), "Line1\n").unwrap();

        let path = dir.join("log.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= appendFile("{}", "Line2\n")
stdout(result.__value.ok)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);

        let content = std::fs::read_to_string(dir.join("log.txt")).unwrap();
        assert!(content.contains("Line1"));
        assert!(content.contains("Line2"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── remove tests ──

    #[test]
    fn test_remove_file() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_remove");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("del.txt"), "bye").unwrap();

        let path = dir.join("del.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= remove("{}")
stdout(result.__value.ok)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);
        assert!(!dir.join("del.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_remove_directory() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_remove_dir");
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "inside").unwrap();

        let path = sub.to_string_lossy().to_string();
        let code = format!(
            r#"result <= remove("{}")
stdout(result.__value.ok)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);
        assert!(!sub.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── createDir tests ──

    #[test]
    fn test_createdir() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_createdir/a/b/c");
        let _ = std::fs::remove_dir_all("/tmp/taida_test_os_createdir");

        let path = dir.to_string_lossy().to_string();
        let code = format!(
            r#"result <= createDir("{}")
stdout(result.__value.ok)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);
        assert!(dir.exists());

        let _ = std::fs::remove_dir_all("/tmp/taida_test_os_createdir");
    }

    // ── rename tests ──

    #[test]
    fn test_rename() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_rename");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.txt"), "data").unwrap();

        let from = dir.join("old.txt").to_string_lossy().to_string();
        let to = dir.join("new.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= rename("{}", "{}")
stdout(result.__value.ok)"#,
            from, to
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true"]);
        assert!(!dir.join("old.txt").exists());
        assert!(dir.join("new.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── run tests ──

    #[test]
    fn test_run_success() {
        let code = r#"result <= run("echo", @["hello"])
stdout(result.__value.stdout)"#;
        let output = run_code(code);
        assert_eq!(output[0].trim(), "hello");
    }

    #[test]
    fn test_run_failure() {
        let code = r#"result <= run("/nonexistent_program_xyz", @[])
stdout(result.hasValue)"#;
        // run() with nonexistent program should return failure Gorillax
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── execShell tests ──

    #[test]
    fn test_execshell_success() {
        let code = r#"result <= execShell("echo world")
stdout(result.__value.stdout)"#;
        let output = run_code(code);
        assert_eq!(output[0].trim(), "world");
    }

    #[test]
    fn test_execshell_failure() {
        let code = r#"result <= execShell("exit 7")
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    // ── allEnv tests ──

    #[test]
    fn test_allenv() {
        let code = r#"env <= allEnv()
stdout(typeof(env))"#;
        let output = run_code(code);
        assert_eq!(output, vec!["HashMap"]);
    }

    #[test]
    fn test_argv_returns_list_of_strings() {
        let output = run_code(
            r#"args <= argv()
stdout(typeof(args))"#,
        );
        assert_eq!(output, vec!["List"]);
    }

    // ── Error handling tests ──

    #[test]
    fn test_read_missing_arg() {
        let result = run_code_result(r#"Read[]()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_writefile_missing_args() {
        let result = run_code_result(r#"writeFile()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_readbytes_missing_args() {
        let result = run_code_result(r#"readBytes()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_writebytes_missing_args() {
        let result = run_code_result(r#"writeBytes()"#);
        assert!(result.is_err());
    }

    // ── Stat modified field format ──

    #[test]
    fn test_stat_modified_rfc3339() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_stat_modified");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ts.txt"), "time").unwrap();

        let path = dir.join("ts.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= Stat["{}"]()
stdout(result.__value.modified)"#,
            path
        );
        let output = run_code(&code);
        // Should be RFC3339/UTC: YYYY-MM-DDTHH:MM:SSZ
        assert!(
            output[0].ends_with('Z'),
            "modified should end with Z: {}",
            output[0]
        );
        assert!(
            output[0].contains('T'),
            "modified should contain T: {}",
            output[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Phase 2: Async API tests ──

    // ── ReadAsync tests ──

    #[test]
    fn test_readasync_existing_file() {
        let dir = std::path::PathBuf::from("/tmp/taida_test_os_readasync");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.txt"), "Async Hello").unwrap();

        let path = dir.join("test.txt").to_string_lossy().to_string();
        // ReadAsync returns Async[Lax[Str]], ]=> unwraps the Async to get Lax[Str]
        let code = format!(
            r#"result <= ReadAsync["{}"]()
result ]=> lax
stdout(lax.hasValue)
stdout(lax.__value)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(output, vec!["true", "Async Hello"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_readasync_nonexistent_file() {
        let code = r#"result <= ReadAsync["/tmp/taida_nonexistent_readasync_xyz.txt"]()
result ]=> lax
stdout(lax.hasValue)"#;
        let output = run_code(code);
        assert_eq!(output, vec!["false"]);
    }

    #[test]
    fn test_readasync_missing_arg() {
        let result = run_code_result(r#"ReadAsync[]()"#);
        assert!(result.is_err());
    }

    // ── HttpGet tests ──

    #[test]
    fn test_httpget_missing_arg() {
        let result = run_code_result(r#"HttpGet[]()"#);
        assert!(result.is_err());
    }

    // ── HttpPost tests ──

    #[test]
    fn test_httppost_missing_args() {
        let result = run_code_result(r#"HttpPost["http://example.com"]()"#);
        assert!(result.is_err());
    }

    // ── HttpRequest tests ──

    #[test]
    fn test_httprequest_missing_args() {
        let result = run_code_result(r#"HttpRequest["GET"]()"#);
        assert!(result.is_err());
    }

    // ── tcpConnect tests ──

    #[test]
    fn test_dnsresolve_missing_args() {
        let result = run_code_result(r#"dnsResolve()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_tcpconnect_missing_args() {
        let result = run_code_result(r#"tcpConnect()"#);
        assert!(result.is_err());
    }

    // ── tcpListen tests ──

    #[test]
    fn test_tcplisten_missing_args() {
        let result = run_code_result(r#"tcpListen()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_tcpaccept_missing_args() {
        let result = run_code_result(r#"tcpAccept()"#);
        assert!(result.is_err());
    }

    // ── socketSend tests ──

    #[test]
    fn test_socketsend_missing_args() {
        let result = run_code_result(r#"socketSend()"#);
        assert!(result.is_err());
    }

    // ── socketRecv tests ──

    #[test]
    fn test_socketrecv_missing_args() {
        let result = run_code_result(r#"socketRecv()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_socketsendall_missing_args() {
        let result = run_code_result(r#"socketSendAll()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_socketsendbytes_missing_args() {
        let result = run_code_result(r#"socketSendBytes()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_socketrecvbytes_missing_args() {
        let result = run_code_result(r#"socketRecvBytes()"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_socketrecvexact_missing_args() {
        let result = run_code_result(r#"socketRecvExact()"#);
        assert!(result.is_err());
    }

    // ── udpBind tests ──

    #[test]
    fn test_udpbind_missing_args() {
        let result = run_code_result(r#"udpBind()"#);
        assert!(result.is_err());
    }

    // ── udpSendTo tests ──

    #[test]
    fn test_udpsendto_missing_args() {
        let result = run_code_result(r#"udpSendTo()"#);
        assert!(result.is_err());
    }

    // ── udpRecvFrom tests ──

    #[test]
    fn test_udprecvfrom_missing_args() {
        let result = run_code_result(r#"udpRecvFrom()"#);
        assert!(result.is_err());
    }

    // ── socketClose tests ──

    #[test]
    fn test_socketclose_missing_args() {
        let result = run_code_result(r#"socketClose()"#);
        assert!(result.is_err());
    }

    // ── listenerClose tests ──

    #[test]
    fn test_listenerclose_missing_args() {
        let result = run_code_result(r#"listenerClose()"#);
        assert!(result.is_err());
    }

    // ── udpClose tests ──

    #[test]
    fn test_udpclose_missing_args() {
        let result = run_code_result(r#"udpClose()"#);
        assert!(result.is_err());
    }

    // ── OS_SYMBOLS test ──

    #[test]
    fn test_os_symbols_count() {
        // Verify all 35 symbols are registered
        assert_eq!(OS_SYMBOLS.len(), 35);
        assert!(OS_SYMBOLS.contains(&"readBytes"));
        assert!(OS_SYMBOLS.contains(&"writeBytes"));
        assert!(OS_SYMBOLS.contains(&"argv"));
        assert!(OS_SYMBOLS.contains(&"ReadAsync"));
        assert!(OS_SYMBOLS.contains(&"HttpGet"));
        assert!(OS_SYMBOLS.contains(&"HttpPost"));
        assert!(OS_SYMBOLS.contains(&"HttpRequest"));
        assert!(OS_SYMBOLS.contains(&"tcpConnect"));
        assert!(OS_SYMBOLS.contains(&"tcpListen"));
        assert!(OS_SYMBOLS.contains(&"tcpAccept"));
        assert!(OS_SYMBOLS.contains(&"socketSend"));
        assert!(OS_SYMBOLS.contains(&"socketSendAll"));
        assert!(OS_SYMBOLS.contains(&"socketRecv"));
        assert!(OS_SYMBOLS.contains(&"socketSendBytes"));
        assert!(OS_SYMBOLS.contains(&"socketRecvBytes"));
        assert!(OS_SYMBOLS.contains(&"socketRecvExact"));
        assert!(OS_SYMBOLS.contains(&"udpBind"));
        assert!(OS_SYMBOLS.contains(&"udpSendTo"));
        assert!(OS_SYMBOLS.contains(&"udpRecvFrom"));
        assert!(OS_SYMBOLS.contains(&"socketClose"));
        assert!(OS_SYMBOLS.contains(&"listenerClose"));
        assert!(OS_SYMBOLS.contains(&"udpClose"));
    }

    // ── HTTP helper tests ──

    #[test]
    fn test_parse_url_http() {
        let (host, port, path, tls) = parse_url("http://example.com/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/path");
        assert!(!tls);
    }

    #[test]
    fn test_parse_url_https() {
        let (host, port, path, tls) = parse_url("https://example.com/api/v1").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/api/v1");
        assert!(tls);
    }

    #[test]
    fn test_parse_url_custom_port() {
        let (host, port, path, tls) = parse_url("http://localhost:8080/test").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/test");
        assert!(!tls);
    }

    #[test]
    fn test_parse_url_no_path() {
        let (host, port, path, tls) = parse_url("http://example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
        assert!(!tls);
    }

    #[test]
    fn test_make_http_response() {
        let resp = make_http_response(
            200,
            "hello".to_string(),
            vec![("content-type".to_string(), "text/plain".to_string())],
        );
        assert!(lax_has_value(&resp));
        let inner = lax_value(&resp);
        if let Value::BuchiPack(_fields) = inner {
            let status = pack_field(inner, "status");
            assert_eq!(*status, Value::Int(200));
            let body = pack_field(inner, "body");
            assert_eq!(*body, Value::Str("hello".to_string()));
        } else {
            panic!("Expected BuchiPack");
        }
    }

    #[test]
    fn test_make_http_failure() {
        let resp = make_http_failure();
        assert!(!lax_has_value(&resp));
    }

    // ── BT-11: OS operation error path tests ──

    #[test]
    fn test_bt11_read_nonexistent_returns_lax_false() {
        let code = r#"result <= Read["/tmp/taida_bt11_nonexistent_xyz.txt"]()
stdout(result.hasValue)
stdout(result.__default)"#;
        let output = run_code(code);
        assert_eq!(
            output[0], "false",
            "Read nonexistent should return Lax(hasValue=false)"
        );
        assert_eq!(
            output[1], "",
            "Default for string Lax should be empty string"
        );
    }

    #[test]
    fn test_bt11_stat_nonexistent_returns_lax_false() {
        let code = r#"result <= Stat["/tmp/taida_bt11_nonexistent_xyz"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(
            output,
            vec!["false"],
            "Stat nonexistent should return Lax(hasValue=false)"
        );
    }

    #[test]
    fn test_bt11_exists_nonexistent() {
        let code = r#"stdout(Exists["/tmp/taida_bt11_nonexistent_xyz"]())"#;
        let output = run_code(code);
        assert_eq!(
            output,
            vec!["false"],
            "Exists nonexistent should return false"
        );
    }

    /// RAII guard for test directories — ensures cleanup even on panic.
    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new(path: &str) -> Self {
            let p = std::path::PathBuf::from(path);
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &std::path::PathBuf {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_bt11_read_empty_file() {
        let dir = TestDir::new("/tmp/taida_test_bt11_empty");
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();

        let path = dir.path().join("empty.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= Read["{}"]()
stdout(result.hasValue)
stdout(result.__value)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(
            output[0], "true",
            "Empty file should still have hasValue=true"
        );
        assert_eq!(output[1], "", "Empty file content should be empty string");
    }

    #[test]
    fn test_bt11_write_and_read_empty_file() {
        let dir = TestDir::new("/tmp/taida_test_bt11_write_empty");

        let path = dir.path().join("empty.txt").to_string_lossy().to_string();
        let code = format!(
            r#"writeFile("{path}", "")
result <= Read["{path}"]()
stdout(result.hasValue)
stdout(result.__value)"#,
            path = path
        );
        let output = run_code(&code);
        assert_eq!(output[0], "true", "Written empty file should be readable");
        assert_eq!(output[1], "", "Written empty file content should be empty");
    }

    #[test]
    fn test_bt11_path_with_spaces() {
        let dir = TestDir::new("/tmp/taida test bt11 spaces");
        std::fs::write(dir.path().join("test file.txt"), "hello spaces").unwrap();

        let path = dir
            .path()
            .join("test file.txt")
            .to_string_lossy()
            .to_string();
        let code = format!(
            r#"result <= Read["{}"]()
stdout(result.hasValue)
stdout(result.__value)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(
            output[0], "true",
            "File with spaces in path should be readable"
        );
        assert_eq!(output[1], "hello spaces");
    }

    #[test]
    fn test_bt11_path_with_unicode() {
        let dir = TestDir::new("/tmp/taida_test_bt11_unicode_\u{65E5}\u{672C}");
        std::fs::write(dir.path().join("test.txt"), "unicode path").unwrap();

        let path = dir.path().join("test.txt").to_string_lossy().to_string();
        let code = format!(
            r#"result <= Read["{}"]()
stdout(result.hasValue)
stdout(result.__value)"#,
            path
        );
        let output = run_code(&code);
        assert_eq!(
            output[0], "true",
            "File with Unicode path should be readable"
        );
        assert_eq!(output[1], "unicode path");
    }

    #[test]
    fn test_bt11_listdir_nonexistent_returns_lax_false() {
        let code = r#"result <= ListDir["/tmp/taida_bt11_nonexistent_dir_xyz"]()
stdout(result.hasValue)"#;
        let output = run_code(code);
        assert_eq!(
            output,
            vec!["false"],
            "ListDir nonexistent should return Lax(hasValue=false)"
        );
    }
}
