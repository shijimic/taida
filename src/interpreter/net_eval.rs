/// Net package evaluation for the Taida interpreter.
///
/// Implements `taida-lang/net` (core-bundled):
///
/// Legacy surface (shared with os runtime dispatch):
///   dnsResolve, tcpConnect, tcpListen, tcpAccept,
///   socketSend, socketSendAll, socketRecv,
///   socketSendBytes, socketRecvBytes, socketRecvExact,
///   udpBind, udpSendTo, udpRecvFrom,
///   socketClose, listenerClose, udpClose
///
/// HTTP v1 surface (new):
///   httpServe, httpParseRequestHead, httpEncodeResponse
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.

use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, ErrorValue, Value};
use crate::parser::Expr;

/// All symbols exported by the net package.
/// Legacy (16) + HTTP v1 (3) = 19 symbols.
pub(crate) const NET_SYMBOLS: &[&str] = &[
    // Legacy surface (shared with os)
    "dnsResolve",
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
    // HTTP v1
    "httpServe",
    "httpParseRequestHead",
    "httpEncodeResponse",
];

// ── Result helpers ──────────────────────────────────────────

fn make_result_success(inner: Value) -> Value {
    Value::BuchiPack(vec![
        ("__value".into(), inner),
        ("throw".into(), Value::Unit),
        ("__predicate".into(), Value::Unit),
        ("__type".into(), Value::Str("Result".into())),
    ])
}

fn make_result_failure_msg(kind: &str, message: impl Into<String>) -> Value {
    let message = message.into();
    let inner = Value::BuchiPack(vec![
        ("ok".into(), Value::Bool(false)),
        ("code".into(), Value::Int(-1)),
        ("message".into(), Value::Str(message.clone())),
        ("kind".into(), Value::Str(kind.to_string())),
    ]);
    let error_val = Value::Error(ErrorValue {
        error_type: "HttpError".into(),
        message,
        fields: vec![("kind".into(), Value::Str(kind.to_string()))],
    });
    Value::BuchiPack(vec![
        ("__value".into(), inner),
        ("throw".into(), error_val),
        ("__predicate".into(), Value::Unit),
        ("__type".into(), Value::Str("Result".into())),
    ])
}

fn make_span(start: usize, len: usize) -> Value {
    Value::BuchiPack(vec![
        ("start".into(), Value::Int(start as i64)),
        ("len".into(), Value::Int(len as i64)),
    ])
}

// ── Async / value helpers ──────────────────────────────────

/// Wrap a value in a fulfilled Async envelope.
fn make_fulfilled_async(value: Value) -> Value {
    Value::Async(AsyncValue {
        status: AsyncStatus::Fulfilled,
        value: Box::new(value),
        error: Box::new(Value::Unit),
        task: None,
    })
}

/// Extract the __value from a Result BuchiPack, returning None on failure.
fn extract_result_value(result: &Value) -> Option<&Vec<(String, Value)>> {
    let fields = match result {
        Value::BuchiPack(f) => f,
        _ => return None,
    };
    // Check that throw is Unit (success)
    match fields.iter().find(|(k, _)| k == "throw") {
        Some((_, Value::Unit)) => {}
        _ => return None,
    }
    match fields.iter().find(|(k, _)| k == "__value") {
        Some((_, Value::BuchiPack(inner))) => Some(inner),
        _ => None,
    }
}

/// Get a Bool field from a BuchiPack field list.
fn get_field_bool(fields: &[(String, Value)], key: &str) -> Option<bool> {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Bool(b))) => Some(*b),
        _ => None,
    }
}

/// Get an Int field from a BuchiPack field list.
fn get_field_int(fields: &[(String, Value)], key: &str) -> Option<i64> {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Int(n))) => Some(*n),
        _ => None,
    }
}

/// Get a reference to any field value from a BuchiPack field list.
fn get_field_value<'a>(fields: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

// ── httpParseRequestHead ────────────────────────────────────

/// Parse HTTP/1.1 request head from raw bytes.
/// Returns Result[@(complete, consumed, method, path, query, version, headers, bodyOffset, contentLength), _]
fn parse_request_head(bytes: &[u8]) -> Value {
    let mut header_buf = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut header_buf);

    match req.parse(bytes) {
        Ok(httparse::Status::Complete(consumed)) => {
            build_parse_result(&req, bytes, consumed, true)
        }
        Ok(httparse::Status::Partial) => {
            // Incomplete: try to extract what we can, but mark complete=false
            // Re-parse to get partial data (httparse populates fields even on Partial)
            build_parse_result(&req, bytes, 0, false)
        }
        Err(e) => make_result_failure_msg("ParseError", format!("Malformed HTTP request: {}", e)),
    }
}

fn build_parse_result(
    req: &httparse::Request,
    bytes: &[u8],
    consumed: usize,
    complete: bool,
) -> Value {
    let base = bytes.as_ptr() as usize;

    // method span
    let method_span = if let Some(method) = req.method {
        let start = method.as_ptr() as usize - base;
        make_span(start, method.len())
    } else {
        make_span(0, 0)
    };

    // path + query spans (split on '?')
    let (path_span, query_span) = if let Some(full_path) = req.path {
        let path_start = full_path.as_ptr() as usize - base;
        if let Some(q_pos) = full_path.find('?') {
            (
                make_span(path_start, q_pos),
                make_span(path_start + q_pos + 1, full_path.len() - q_pos - 1),
            )
        } else {
            (make_span(path_start, full_path.len()), make_span(0, 0))
        }
    } else {
        (make_span(0, 0), make_span(0, 0))
    };

    // version
    let version = Value::BuchiPack(vec![
        ("major".into(), Value::Int(1)),
        (
            "minor".into(),
            Value::Int(req.version.unwrap_or(1) as i64),
        ),
    ]);

    // headers as list of @(name: span, value: span)
    // On Partial parse, req.headers contains EMPTY_HEADER entries beyond parsed ones.
    // Stop at the first empty header name to avoid pointer arithmetic on unrelated memory.
    let mut content_length: i64 = 0;
    let mut cl_count: usize = 0;
    let mut headers_list = Vec::new();
    for header in req.headers.iter() {
        if header.name.is_empty() {
            break;
        }
        let name_start = header.name.as_ptr() as usize - base;
        let value_start = header.value.as_ptr() as usize - base;
        headers_list.push(Value::BuchiPack(vec![
            ("name".into(), make_span(name_start, header.name.len())),
            ("value".into(), make_span(value_start, header.value.len())),
        ]));
        if header.name.eq_ignore_ascii_case("content-length") {
            cl_count += 1;
            if cl_count > 1 {
                return make_result_failure_msg(
                    "ParseError",
                    "Malformed HTTP request: duplicate Content-Length header",
                );
            }
            let raw_val = match std::str::from_utf8(header.value) {
                Ok(s) => s.trim(),
                Err(_) => {
                    return make_result_failure_msg(
                        "ParseError",
                        "Malformed HTTP request: invalid Content-Length value",
                    );
                }
            };
            // Strict: entire trimmed value must be ASCII digits only (no leading +/-, no mixed chars).
            // This matches the JS backend's /^\d+$/ validation for cross-backend parity.
            if raw_val.is_empty() || !raw_val.bytes().all(|b| b.is_ascii_digit()) {
                return make_result_failure_msg(
                    "ParseError",
                    "Malformed HTTP request: invalid Content-Length value",
                );
            }
            // Safe to parse: we already validated all-digits, so parse::<i64>() cannot fail
            // (unless the number overflows i64, which we still want to reject).
            match raw_val.parse::<i64>() {
                Ok(len) => content_length = len,
                Err(_) => {
                    return make_result_failure_msg(
                        "ParseError",
                        "Malformed HTTP request: invalid Content-Length value",
                    );
                }
            }
        }
    }

    let parsed = Value::BuchiPack(vec![
        ("complete".into(), Value::Bool(complete)),
        ("consumed".into(), Value::Int(consumed as i64)),
        ("method".into(), method_span),
        ("path".into(), path_span),
        ("query".into(), query_span),
        ("version".into(), version),
        ("headers".into(), Value::List(headers_list)),
        ("bodyOffset".into(), Value::Int(consumed as i64)),
        ("contentLength".into(), Value::Int(content_length)),
    ]);

    make_result_success(parsed)
}

// ── httpEncodeResponse ──────────────────────────────────────

/// Encode a response BuchiPack into HTTP/1.1 wire bytes.
/// Input: @(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes | Str)
/// Returns Result[@(bytes: Bytes), _]
fn encode_response(response: &Value) -> Value {
    let (status, headers, body_bytes) = match extract_response_fields(response) {
        Ok(fields) => fields,
        Err(msg) => return make_result_failure_msg("EncodeError", msg),
    };

    // RFC 9110: 1xx, 204, 205, 304 MUST NOT contain a message body
    let no_body = (100..200).contains(&status) || status == 204 || status == 205 || status == 304;
    if no_body && !body_bytes.is_empty() {
        return make_result_failure_msg(
            "EncodeError",
            format!(
                "httpEncodeResponse: status {} must not have a body",
                status
            ),
        );
    }

    let reason = status_reason(status);
    let mut buf = Vec::with_capacity(256 + body_bytes.len());

    // Status line
    buf.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", status, reason).as_bytes());

    // User headers (skip Content-Length for no-body statuses)
    for (name, value) in &headers {
        if no_body && name.eq_ignore_ascii_case("Content-Length") {
            continue;
        }
        buf.extend_from_slice(format!("{}: {}\r\n", name, value).as_bytes());
    }

    // Auto-append Content-Length for statuses that allow a body
    if !no_body {
        let has_content_length = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("Content-Length"));
        if !has_content_length {
            buf.extend_from_slice(
                format!("Content-Length: {}\r\n", body_bytes.len()).as_bytes(),
            );
        }
    }

    buf.extend_from_slice(b"\r\n");
    if !no_body {
        buf.extend_from_slice(&body_bytes);
    }

    let result = Value::BuchiPack(vec![("bytes".into(), Value::Bytes(buf))]);
    make_result_success(result)
}

fn extract_response_fields(
    response: &Value,
) -> Result<(i64, Vec<(String, String)>, Vec<u8>), String> {
    let fields = match response {
        Value::BuchiPack(fields) => fields,
        _ => return Err("httpEncodeResponse: argument must be a BuchiPack @(...)".into()),
    };

    // status (required, must be Int)
    let status = match fields.iter().find(|(k, _)| k == "status") {
        Some((_, Value::Int(n))) => *n,
        Some((_, v)) => {
            return Err(format!(
                "httpEncodeResponse: status must be Int, got {}",
                v
            ))
        }
        None => return Err("httpEncodeResponse: missing required field 'status'".into()),
    };
    if !(100..=999).contains(&status) {
        return Err(format!(
            "httpEncodeResponse: status must be 100-999, got {}",
            status
        ));
    }

    // headers (required, must be List of @(name: Str, value: Str))
    let header_list = match fields.iter().find(|(k, _)| k == "headers") {
        Some((_, Value::List(list))) => list,
        Some((_, v)) => {
            return Err(format!(
                "httpEncodeResponse: headers must be a List, got {}",
                v
            ))
        }
        None => return Err("httpEncodeResponse: missing required field 'headers'".into()),
    };
    let mut headers = Vec::new();
    for (i, h) in header_list.iter().enumerate() {
        let hf = match h {
            Value::BuchiPack(f) => f,
            _ => {
                return Err(format!(
                    "httpEncodeResponse: headers[{}] must be @(name, value)",
                    i
                ))
            }
        };
        let name = match hf.iter().find(|(k, _)| k == "name") {
            Some((_, Value::Str(s))) => s.clone(),
            _ => {
                return Err(format!(
                    "httpEncodeResponse: headers[{}].name must be Str",
                    i
                ))
            }
        };
        let value = match hf.iter().find(|(k, _)| k == "value") {
            Some((_, Value::Str(s))) => s.clone(),
            _ => {
                return Err(format!(
                    "httpEncodeResponse: headers[{}].value must be Str",
                    i
                ))
            }
        };
        // Reject CRLF in header name/value to prevent response splitting
        if name.contains('\r') || name.contains('\n') {
            return Err(format!(
                "httpEncodeResponse: headers[{}].name contains CR/LF",
                i
            ));
        }
        if value.contains('\r') || value.contains('\n') {
            return Err(format!(
                "httpEncodeResponse: headers[{}].value contains CR/LF",
                i
            ));
        }
        headers.push((name, value));
    }

    // body (required, must be Bytes or Str)
    let body_bytes = match fields.iter().find(|(k, _)| k == "body") {
        Some((_, Value::Bytes(b))) => b.clone(),
        Some((_, Value::Str(s))) => s.as_bytes().to_vec(),
        Some((_, v)) => {
            return Err(format!(
                "httpEncodeResponse: body must be Bytes or Str, got {}",
                v
            ))
        }
        None => return Err("httpEncodeResponse: missing required field 'body'".into()),
    };

    Ok((status, headers, body_bytes))
}

fn status_reason(code: i64) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        418 => "I'm a Teapot",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

// ── Dispatch ────────────────────────────────────────────────

impl Interpreter {
    /// Try to handle a net built-in function call.
    /// Returns None if the name is not a recognized net function
    /// or if the function was not imported from taida-lang/net (sentinel guard).
    ///
    /// Supports alias imports: `>>> taida-lang/net => @(httpServe: serve)`
    /// binds `serve = "__net_builtin_httpServe"`. The guard extracts the original
    /// function name from the `__net_builtin_` prefix rather than deriving it
    /// from the local call name.
    pub(crate) fn try_net_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // Sentinel guard: extract original function name from __net_builtin_ prefix.
        // This supports alias imports where the local name differs from the export name.
        let original_name = match self.env.get(name) {
            Some(Value::Str(tag)) if tag.starts_with("__net_builtin_") => {
                tag["__net_builtin_".len()..].to_string()
            }
            _ => return Ok(None),
        };

        match original_name.as_str() {
            // ── Legacy surface — delegate to os_eval implementations ──
            // Note: these symbols are also reachable via the unguarded try_os_func()
            // when imported from taida-lang/os. That is known debt, not a NET-0 scope fix.
            "dnsResolve" | "tcpConnect" | "tcpListen" | "tcpAccept"
            | "socketSend" | "socketSendAll" | "socketRecv"
            | "socketSendBytes" | "socketRecvBytes" | "socketRecvExact"
            | "udpBind" | "udpSendTo" | "udpRecvFrom"
            | "socketClose" | "listenerClose" | "udpClose" => {
                self.try_os_func(&original_name, args)
            }

            // ── httpParseRequestHead(bytes) → Result[@(parsed), _] ──
            "httpParseRequestHead" => {
                let bytes = self.eval_net_bytes_arg(args, 0, "httpParseRequestHead")?;
                Ok(Some(Signal::Value(parse_request_head(&bytes))))
            }

            // ── httpEncodeResponse(response) → Result[@(bytes: Bytes), _] ──
            "httpEncodeResponse" => {
                let response = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "httpEncodeResponse: missing response argument".into(),
                        })
                    }
                };
                Ok(Some(Signal::Value(encode_response(&response))))
            }

            // ── httpServe(port, handler, maxRequests, timeoutMs) ──
            // → Async[Result[@(ok: Bool, requests: Int), _]]
            "httpServe" => self.eval_http_serve(args),

            _ => Ok(None),
        }
    }

    // ── httpServe implementation ───────────────────────────────
    //
    // httpServe(port, handler, maxRequests <= 0, timeoutMs <= 5000)
    //   → Async[Result[@(ok: Bool, requests: Int), _]]
    //
    // - Binds to 127.0.0.1:port (fixed, never 0.0.0.0)
    // - 1 connection = 1 request, response then close
    // - Sequential processing (no concurrent handler dispatch)
    // - maxRequests > 0 → bounded shutdown after N requests
    // - maxRequests = 0 → run indefinitely
    // - No httpClose, no keep-alive, no chunked, no streaming

    fn eval_http_serve(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // ── Arg 0: port (required, Int) ──
        let port: u16 = match args.first() {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => {
                    if n < 0 || n > 65535 {
                        return Err(RuntimeError {
                            message: format!("httpServe: port must be 0-65535, got {}", n),
                        });
                    }
                    n as u16
                }
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("httpServe: port must be Int, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            },
            None => {
                return Err(RuntimeError {
                    message: "httpServe: missing argument 'port'".into(),
                });
            }
        };

        // ── Arg 1: handler (required, Function) ──
        let handler = match args.get(1) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Function(f)) => f,
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("httpServe: handler must be a Function, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            },
            None => {
                return Err(RuntimeError {
                    message: "httpServe: missing argument 'handler'".into(),
                });
            }
        };

        // ── Arg 2: maxRequests (optional, default 0 = unlimited) ──
        let max_requests: i64 = match args.get(2) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => n,
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("httpServe: maxRequests must be Int, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            },
            None => 0,
        };

        // ── Arg 3: timeoutMs (optional, default 5000) ──
        let timeout_ms: u64 = match args.get(3) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => {
                    if n < 0 {
                        return Err(RuntimeError {
                            message: format!(
                                "httpServe: timeoutMs must be non-negative, got {}",
                                n
                            ),
                        });
                    }
                    n as u64
                }
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("httpServe: timeoutMs must be Int, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            },
            None => 5000,
        };

        // ── Bind to 127.0.0.1:port ──
        // v1 contract: always bind to loopback, never 0.0.0.0
        let addr = format!("127.0.0.1:{}", port);
        let listener = match std::net::TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                // Bind failure → immediate failure result
                let result = make_result_failure_msg(
                    "BindError",
                    format!("httpServe: failed to bind to {}: {}", addr, e),
                );
                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
            }
        };

        // Set read timeout on accepted connections
        let read_timeout = std::time::Duration::from_millis(timeout_ms);

        // ── Accept loop ──
        let mut request_count: i64 = 0;
        loop {
            // Check bounded shutdown
            if max_requests > 0 && request_count >= max_requests {
                break;
            }

            // Accept one connection
            let (mut stream, peer_addr) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) => {
                    // Accept failure → return error result
                    let result = make_result_failure_msg(
                        "AcceptError",
                        format!("httpServe: accept failed: {}", e),
                    );
                    return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                }
            };

            // Set read timeout on the connection
            let _ = stream.set_read_timeout(Some(read_timeout));

            // ── Read until head is complete + full body arrives ──
            // v1 max buffer: 1 MiB (protects against memory exhaustion)
            const MAX_REQUEST_BUF: usize = 1_048_576;
            let mut buf = vec![0u8; 8192];
            let mut total_read: usize = 0;

            // Phase 1: read until the HTTP head is complete
            enum HeadResult {
                Complete(Vec<(String, Value)>, usize, i64), // (parsed_fields, head_consumed, content_length)
                Malformed,
                Incomplete, // EOF / timeout before head finished
            }

            let head_result = loop {
                if total_read >= MAX_REQUEST_BUF {
                    break HeadResult::Incomplete;
                }
                if total_read == buf.len() {
                    buf.resize(std::cmp::min(buf.len() * 2, MAX_REQUEST_BUF), 0);
                }
                match std::io::Read::read(&mut stream, &mut buf[total_read..]) {
                    Ok(0) => break HeadResult::Incomplete,
                    Ok(n) => total_read += n,
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break HeadResult::Incomplete;
                    }
                    Err(_) => break HeadResult::Incomplete,
                }

                let parse_result = parse_request_head(&buf[..total_read]);
                match extract_result_value(&parse_result) {
                    None => break HeadResult::Malformed,
                    Some(inner) => {
                        if get_field_bool(inner, "complete").unwrap_or(false) {
                            let consumed =
                                get_field_int(inner, "consumed").unwrap_or(0) as usize;
                            let cl =
                                get_field_int(inner, "contentLength").unwrap_or(0);
                            break HeadResult::Complete(inner.clone(), consumed, cl);
                        }
                    }
                }
            };

            let (parsed_fields, head_consumed, content_length) = match head_result {
                HeadResult::Complete(fields, consumed, cl) => (fields, consumed, cl),
                HeadResult::Malformed | HeadResult::Incomplete => {
                    let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = std::io::Write::write_all(&mut stream, bad_request);
                    request_count += 1;
                    continue;
                }
            };

            // Phase 2: read until the full body arrives (Content-Length bytes after head)
            let body_needed = head_consumed + content_length as usize;
            while total_read < body_needed && total_read < MAX_REQUEST_BUF {
                if total_read == buf.len() {
                    buf.resize(std::cmp::min(buf.len() * 2, MAX_REQUEST_BUF), 0);
                }
                match std::io::Read::read(&mut stream, &mut buf[total_read..]) {
                    Ok(0) => break,
                    Ok(n) => total_read += n,
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            buf.truncate(total_read);

            // Reject if body is incomplete (EOF / timeout / buffer limit before full body)
            if content_length > 0 && total_read < body_needed {
                let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = std::io::Write::write_all(&mut stream, bad_request);
                request_count += 1;
                continue;
            }

            // ── Build request pack for handler ──
            let raw_bytes = buf;
            let body_offset = head_consumed as i64;
            let body_start = head_consumed;
            let body_len = content_length as usize;

            let mut request_fields: Vec<(String, Value)> = Vec::new();
            request_fields.push(("raw".into(), Value::Bytes(raw_bytes)));

            for key in &["method", "path", "query", "version", "headers"] {
                if let Some(v) = get_field_value(&parsed_fields, key) {
                    request_fields.push((key.to_string(), v.clone()));
                }
            }

            request_fields.push(("body".into(), make_span(body_start, body_len)));
            request_fields.push(("bodyOffset".into(), Value::Int(body_offset)));
            request_fields.push(("contentLength".into(), Value::Int(content_length)));
            request_fields.push(("remoteHost".into(), Value::Str(peer_addr.ip().to_string())));
            request_fields.push(("remotePort".into(), Value::Int(peer_addr.port() as i64)));

            let request_pack = Value::BuchiPack(request_fields);

            // ── Call handler with request ──
            let handler_result = self.call_function_with_values(&handler, &[request_pack]);

            let response_value = match handler_result {
                Ok(v) => v,
                Err(e) => {
                    // Handler error → send 500 Internal Server Error
                    let error_body = format!("Internal Server Error: {}", e.message);
                    let error_response = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        error_body.len(),
                        error_body
                    );
                    let _ = std::io::Write::write_all(&mut stream, error_response.as_bytes());
                    request_count += 1;
                    continue;
                }
            };

            // ── Encode response and write back ──
            let encoded = encode_response(&response_value);
            match extract_result_value(&encoded) {
                Some(inner) => {
                    if let Some(Value::Bytes(wire_bytes)) = get_field_value(inner, "bytes") {
                        let _ = std::io::Write::write_all(&mut stream, wire_bytes);
                    }
                }
                None => {
                    // Encode failed → send 500
                    let fallback = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = std::io::Write::write_all(&mut stream, fallback);
                }
            }

            // v1: 1 connection = 1 request, close after response
            // (stream drops here, closing the connection)
            request_count += 1;
        }

        // Server completed successfully
        let result_inner = Value::BuchiPack(vec![
            ("ok".into(), Value::Bool(true)),
            ("requests".into(), Value::Int(request_count)),
        ]);
        let result = make_result_success(result_inner);
        Ok(Some(Signal::Value(make_fulfilled_async(result))))
    }

    fn eval_net_bytes_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        func_name: &str,
    ) -> Result<Vec<u8>, RuntimeError> {
        let arg = args.get(index).ok_or_else(|| RuntimeError {
            message: format!("{}: missing bytes argument", func_name),
        })?;
        match self.eval_expr(arg)? {
            Signal::Value(Value::Bytes(b)) => Ok(b),
            Signal::Value(Value::Str(s)) => Ok(s.into_bytes()),
            Signal::Value(v) => Err(RuntimeError {
                message: format!("{}: argument must be Bytes or Str, got {}", func_name, v),
            }),
            other => Err(RuntimeError {
                message: format!(
                    "{}: unexpected signal: {:?}",
                    func_name,
                    std::mem::discriminant(&other)
                ),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_net_symbols_count() {
        // 16 legacy + 3 HTTP v1 = 19
        assert_eq!(NET_SYMBOLS.len(), 19);
        assert!(NET_SYMBOLS.contains(&"dnsResolve"));
        assert!(NET_SYMBOLS.contains(&"httpServe"));
        assert!(NET_SYMBOLS.contains(&"httpParseRequestHead"));
        assert!(NET_SYMBOLS.contains(&"httpEncodeResponse"));
    }

    // ── Sentinel guard tests ──

    #[test]
    fn test_sentinel_guard_blocks_without_import() {
        let mut interp = Interpreter::new();
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_sentinel_guard_passes_with_correct_sentinel() {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err());
    }

    #[test]
    fn test_sentinel_guard_with_alias() {
        // >>> taida-lang/net => @(httpServe: serve)
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("serve", Value::Str("__net_builtin_httpServe".into()));
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("serve", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("httpServe"));
    }

    #[test]
    fn test_sentinel_guard_blocks_wrong_sentinel() {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__os_builtin_httpServe".into()));
        let args: Vec<Expr> = vec![];
        assert!(interp.try_net_func("httpServe", &args).unwrap().is_none());
    }

    #[test]
    fn test_sentinel_guard_blocks_user_function() {
        let mut interp = Interpreter::new();
        interp.env.define_force("httpServe", Value::Int(42));
        let args: Vec<Expr> = vec![];
        assert!(interp.try_net_func("httpServe", &args).unwrap().is_none());
    }

    // ── httpParseRequestHead tests ──

    #[test]
    fn test_parse_complete_get() {
        let raw = b"GET /path?x=1 HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        let fields = match &result {
            Value::BuchiPack(f) => f,
            _ => panic!("expected BuchiPack"),
        };
        // Result success: __type = "Result", throw = Unit
        assert!(matches!(
            fields.iter().find(|(k, _)| k == "__type"),
            Some((_, Value::Str(s))) if s == "Result"
        ));
        assert!(matches!(
            fields.iter().find(|(k, _)| k == "throw"),
            Some((_, Value::Unit))
        ));
        // Inner value
        let inner = match fields.iter().find(|(k, _)| k == "__value") {
            Some((_, v)) => v,
            _ => panic!("no __value"),
        };
        let inner_fields = match inner {
            Value::BuchiPack(f) => f,
            _ => panic!("expected BuchiPack"),
        };
        // complete = true
        assert!(matches!(
            inner_fields.iter().find(|(k, _)| k == "complete"),
            Some((_, Value::Bool(true)))
        ));
        // method span: "GET" starts at 0, len 3
        let method = match inner_fields.iter().find(|(k, _)| k == "method") {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("no method"),
        };
        assert!(matches!(
            method.iter().find(|(k, _)| k == "start"),
            Some((_, Value::Int(0)))
        ));
        assert!(matches!(
            method.iter().find(|(k, _)| k == "len"),
            Some((_, Value::Int(3)))
        ));
        // path span: "/path" starts at 4, len 5
        let path = match inner_fields.iter().find(|(k, _)| k == "path") {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("no path"),
        };
        assert!(matches!(
            path.iter().find(|(k, _)| k == "start"),
            Some((_, Value::Int(4)))
        ));
        assert!(matches!(
            path.iter().find(|(k, _)| k == "len"),
            Some((_, Value::Int(5)))
        ));
        // query span: "x=1" starts at 10, len 3
        let query = match inner_fields.iter().find(|(k, _)| k == "query") {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("no query"),
        };
        assert!(matches!(
            query.iter().find(|(k, _)| k == "start"),
            Some((_, Value::Int(10)))
        ));
        assert!(matches!(
            query.iter().find(|(k, _)| k == "len"),
            Some((_, Value::Int(3)))
        ));
    }

    #[test]
    fn test_parse_post_with_body() {
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 5\r\nHost: localhost\r\n\r\nhello";
        let result = parse_request_head(raw);
        let inner = extract_result_inner(&result);
        assert_eq!(get_bool(&inner, "complete"), true);
        assert_eq!(get_int(&inner, "contentLength"), 5);
        // bodyOffset should equal consumed (end of headers)
        let consumed = get_int(&inner, "consumed");
        assert!(consumed > 0);
        assert_eq!(get_int(&inner, "bodyOffset"), consumed);
    }

    #[test]
    fn test_parse_incomplete() {
        let raw = b"GET / HTTP/1.1\r\nHost: local";
        let result = parse_request_head(raw);
        let inner = extract_result_inner(&result);
        assert_eq!(get_bool(&inner, "complete"), false);
    }

    #[test]
    fn test_parse_malformed() {
        let raw = b"INVALID\x00\x01\x02";
        let result = parse_request_head(raw);
        let fields = match &result {
            Value::BuchiPack(f) => f,
            _ => panic!("expected BuchiPack"),
        };
        // Result failure: throw is not Unit
        assert!(!matches!(
            fields.iter().find(|(k, _)| k == "throw"),
            Some((_, Value::Unit))
        ));
    }

    #[test]
    fn test_parse_no_query() {
        let raw = b"GET /path HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        let inner = extract_result_inner(&result);
        // query span should have len=0
        let query = match inner
            .iter()
            .find(|(k, _)| k == "query")
            .map(|(_, v)| v)
        {
            Some(Value::BuchiPack(f)) => f,
            _ => panic!("no query"),
        };
        assert!(matches!(
            query.iter().find(|(k, _)| k == "len"),
            Some((_, Value::Int(0)))
        ));
    }

    // ── httpEncodeResponse tests ──

    #[test]
    fn test_encode_200_text() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("content-type".into())),
                    ("value".into(), Value::Str("text/plain".into())),
                ])]),
            ),
            ("body".into(), Value::Str("Hello".into())),
        ]);
        let result = encode_response(&response);
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("content-type: text/plain\r\n"));
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(text.ends_with("\r\n\r\nHello"));
    }

    #[test]
    fn test_encode_404_empty() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(404)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("Content-Length: 0\r\n"));
    }

    #[test]
    fn test_encode_binary_body() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Bytes(vec![0x00, 0xFF, 0x42])),
        ]);
        let result = encode_response(&response);
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        assert!(bytes.ends_with(&[0x00, 0xFF, 0x42]));
    }

    #[test]
    fn test_encode_user_content_length_preserved() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("Content-Length".into())),
                    ("value".into(), Value::Str("99".into())),
                ])]),
            ),
            ("body".into(), Value::Str("Hi".into())),
        ]);
        let result = encode_response(&response);
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        // User's Content-Length should be preserved, no auto-append
        assert!(text.contains("Content-Length: 99\r\n"));
        assert_eq!(text.matches("Content-Length").count(), 1);
    }

    // ── Test helpers ──

    fn extract_result_inner(result: &Value) -> &Vec<(String, Value)> {
        let fields = match result {
            Value::BuchiPack(f) => f,
            _ => panic!("expected Result BuchiPack"),
        };
        match fields.iter().find(|(k, _)| k == "__value") {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("no __value BuchiPack"),
        }
    }

    fn get_bool(fields: &[(String, Value)], key: &str) -> bool {
        match fields.iter().find(|(k, _)| k == key) {
            Some((_, Value::Bool(b))) => *b,
            _ => panic!("missing bool field: {}", key),
        }
    }

    fn get_int(fields: &[(String, Value)], key: &str) -> i64 {
        match fields.iter().find(|(k, _)| k == key) {
            Some((_, Value::Int(n))) => *n,
            _ => panic!("missing int field: {}", key),
        }
    }

    fn is_result_failure(result: &Value) -> bool {
        match result {
            Value::BuchiPack(f) => !matches!(
                f.iter().find(|(k, _)| k == "throw"),
                Some((_, Value::Unit))
            ),
            _ => false,
        }
    }

    fn get_failure_message(result: &Value) -> String {
        let fields = match result {
            Value::BuchiPack(f) => f,
            _ => panic!("expected BuchiPack"),
        };
        match fields.iter().find(|(k, _)| k == "throw") {
            Some((_, Value::Error(e))) => e.message.clone(),
            _ => panic!("no Error in throw"),
        }
    }

    // ── Content-Length validation tests ──

    #[test]
    fn test_parse_invalid_content_length_non_numeric() {
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: abc\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_invalid_content_length_negative() {
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: -5\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_invalid_content_length_leading_plus() {
        // "+5" is accepted by Rust's parse::<i64>() but must be rejected for JS parity.
        // Both backends must use strict digits-only validation (/^\d+$/ equivalent).
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: +5\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_invalid_content_length_trailing_chars() {
        // "5abc" must be rejected (not silently parsed as 5).
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 5abc\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_invalid_content_length_empty() {
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: \r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_duplicate_content_length() {
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 10\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("duplicate Content-Length"));
    }

    #[test]
    fn test_parse_valid_content_length_zero() {
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 0\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 0);
    }

    #[test]
    fn test_parse_content_length_i64_overflow() {
        // Value exceeds i64::MAX (9223372036854775807). Interpreter rejects via parse::<i64>().
        // JS must also reject (string-length guard) for cross-backend parity.
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 999999999999999999999999\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_content_length_i64_max_boundary() {
        // Exactly i64::MAX = 9223372036854775807 (19 digits) — should succeed.
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 9223372036854775807\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 9223372036854775807);
    }

    #[test]
    fn test_parse_content_length_i64_max_plus_one() {
        // i64::MAX + 1 = 9223372036854775808 — must be rejected.
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 9223372036854775808\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    // ── Encode strict validation tests ──

    #[test]
    fn test_encode_missing_status() {
        let response = Value::BuchiPack(vec![
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("Hello".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("missing required field 'status'"));
    }

    #[test]
    fn test_encode_wrong_type_status() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Str("200".into())),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("Hello".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("status must be Int"));
    }

    #[test]
    fn test_encode_status_out_of_range() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(99)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("Hello".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("status must be 100-999"));
    }

    #[test]
    fn test_encode_missing_headers() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            ("body".into(), Value::Str("Hello".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("missing required field 'headers'"));
    }

    #[test]
    fn test_encode_missing_body() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            ("headers".into(), Value::List(vec![])),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("missing required field 'body'"));
    }

    #[test]
    fn test_encode_crlf_in_header_name() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("Bad\r\nHeader".into())),
                    ("value".into(), Value::Str("ok".into())),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("CR/LF"));
    }

    #[test]
    fn test_encode_crlf_in_header_value() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("X-Test".into())),
                    ("value".into(), Value::Str("inject\r\nEvil: header".into())),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("CR/LF"));
    }

    #[test]
    fn test_encode_wrong_type_body() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Int(42)),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("body must be Bytes or Str"));
    }

    #[test]
    fn test_encode_header_name_not_str() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Int(42)),
                    ("value".into(), Value::Str("ok".into())),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("headers[0].name must be Str"));
    }

    // ── No-body status tests ──

    #[test]
    fn test_encode_204_empty_body_ok() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(204)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 204 No Content\r\n"));
        // No Content-Length for 204
        assert!(!text.contains("Content-Length"));
        // No body after final CRLF
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn test_encode_204_with_body_rejected() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(204)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("oops".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("must not have a body"));
    }

    #[test]
    fn test_encode_304_with_body_rejected() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(304)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("cached".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("must not have a body"));
    }

    #[test]
    fn test_encode_205_with_body_rejected() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(205)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("data".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("must not have a body"));
    }

    #[test]
    fn test_encode_205_empty_body_ok() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(205)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 205 Reset Content\r\n"));
        assert!(!text.contains("Content-Length"));
    }

    #[test]
    fn test_encode_1xx_with_body_rejected() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(100)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str("data".into())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("must not have a body"));
    }

    #[test]
    fn test_encode_204_content_length_stripped() {
        // User-provided Content-Length should be silently dropped for 204
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(204)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("Content-Length".into())),
                    ("value".into(), Value::Str("0".into())),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("Content-Length"));
    }

    // ── Reason phrase tests ──

    #[test]
    fn test_encode_429_reason_phrase() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(429)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
    }

    #[test]
    fn test_encode_unknown_status_no_fake_reason() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(599)),
            ("headers".into(), Value::List(vec![])),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        // Should NOT say "OK" for unknown status
        assert!(text.starts_with("HTTP/1.1 599 \r\n"));
    }

    // ── Helper function tests ──

    #[test]
    fn test_make_fulfilled_async() {
        let inner = Value::Int(42);
        let async_val = make_fulfilled_async(inner);
        match async_val {
            Value::Async(a) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                assert!(matches!(*a.value, Value::Int(42)));
            }
            _ => panic!("expected Async"),
        }
    }

    #[test]
    fn test_extract_result_value_success() {
        let result = make_result_success(Value::BuchiPack(vec![
            ("ok".into(), Value::Bool(true)),
        ]));
        let inner = extract_result_value(&result);
        assert!(inner.is_some());
    }

    #[test]
    fn test_extract_result_value_failure() {
        let result = make_result_failure_msg("TestError", "test failed");
        let inner = extract_result_value(&result);
        assert!(inner.is_none());
    }

    #[test]
    fn test_get_field_helpers() {
        let fields = vec![
            ("complete".into(), Value::Bool(true)),
            ("count".into(), Value::Int(42)),
            ("name".into(), Value::Str("test".into())),
        ];
        assert_eq!(get_field_bool(&fields, "complete"), Some(true));
        assert_eq!(get_field_int(&fields, "count"), Some(42));
        assert!(get_field_value(&fields, "name").is_some());
        assert!(get_field_value(&fields, "missing").is_none());
    }

    // ── httpServe integration tests ──

    use crate::lexer::Span;
    use crate::parser::{BuchiField, Param};

    fn dummy_span() -> Span {
        Span::new(0, 0, 1, 1)
    }

    /// Build a simple handler lambda expression that returns 200 OK with a given body.
    fn make_handler_expr(body_text: &str) -> Expr {
        Expr::Lambda(
            vec![Param {
                name: "req".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            }],
            Box::new(Expr::BuchiPack(
                vec![
                    BuchiField {
                        name: "status".into(),
                        value: Expr::IntLit(200, dummy_span()),
                        span: dummy_span(),
                    },
                    BuchiField {
                        name: "headers".into(),
                        value: Expr::ListLit(
                            vec![Expr::BuchiPack(
                                vec![
                                    BuchiField {
                                        name: "name".into(),
                                        value: Expr::StringLit("content-type".into(), dummy_span()),
                                        span: dummy_span(),
                                    },
                                    BuchiField {
                                        name: "value".into(),
                                        value: Expr::StringLit("text/plain".into(), dummy_span()),
                                        span: dummy_span(),
                                    },
                                ],
                                dummy_span(),
                            )],
                            dummy_span(),
                        ),
                        span: dummy_span(),
                    },
                    BuchiField {
                        name: "body".into(),
                        value: Expr::StringLit(body_text.into(), dummy_span()),
                        span: dummy_span(),
                    },
                ],
                dummy_span(),
            )),
            dummy_span(),
        )
    }

    #[test]
    fn test_http_serve_bind_failure_returns_fulfilled_async() {
        // Bind a listener to grab a port, then try httpServe on same port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let mut interp = Interpreter::new();
        interp.env.define_force(
            "httpServe",
            Value::Str("__net_builtin_httpServe".into()),
        );
        let args = vec![
            Expr::IntLit(port as i64, dummy_span()),
            make_handler_expr("ok"),
            Expr::IntLit(1, dummy_span()),
        ];

        let result = interp.try_net_func("httpServe", &args).unwrap().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                // The result should be a failure (bind error)
                let inner = extract_result_value(&a.value);
                assert!(inner.is_none(), "Expected bind failure, but got success");
            }
            _ => panic!("expected Async value"),
        }
    }

    #[test]
    fn test_http_serve_max_requests_1_self_terminates() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18100);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("Hello from Taida!"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        // Wait for server to start
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send an HTTP request
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(&mut client, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

        // Read response
        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.contains("HTTP/1.1 200 OK"), "Expected 200 OK, got: {}", response_str);
        assert!(response_str.contains("Hello from Taida!"), "Expected body in response");

        // Server should have terminated
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value);
                assert!(inner.is_some(), "Expected success result");
                let inner = inner.unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(1));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    #[test]
    fn test_http_serve_request_pack_has_all_fields() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18200);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("ok"),
                Expr::IntLit(1, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send POST request with body
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(&mut client, b"POST /data?key=val HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello").unwrap();

        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.contains("200 OK"), "Expected 200 OK");

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(1));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    #[test]
    fn test_http_serve_max_requests_3() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18300);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("ok"),
                Expr::IntLit(3, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send 3 requests
        for _ in 0..3 {
            let mut client =
                std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
            std::io::Write::write_all(&mut client, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
            let mut response = Vec::new();
            let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            loop {
                let mut buf = [0u8; 4096];
                match std::io::Read::read(&mut client, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => response.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            let resp = String::from_utf8_lossy(&response);
            assert!(resp.contains("200 OK"));
        }

        // Server should terminate
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_int(inner, "requests"), Some(3));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    #[test]
    fn test_http_serve_malformed_request_returns_400() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18400);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("ok"),
                Expr::IntLit(1, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send malformed request
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(&mut client, b"NOT_HTTP\x00\x01\x02\r\n\r\n").unwrap();

        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        let resp = String::from_utf8_lossy(&response);
        assert!(resp.contains("400 Bad Request"), "Expected 400, got: {}", resp);

        server_handle.join().unwrap();
    }

    #[test]
    fn test_http_serve_missing_args() {
        let mut interp = Interpreter::new();
        interp.env.define_force(
            "httpServe",
            Value::Str("__net_builtin_httpServe".into()),
        );
        let result = interp.try_net_func("httpServe", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("missing argument 'port'"));
    }

    #[test]
    fn test_http_serve_missing_handler() {
        let mut interp = Interpreter::new();
        interp.env.define_force(
            "httpServe",
            Value::Str("__net_builtin_httpServe".into()),
        );
        let args = vec![Expr::IntLit(8080, dummy_span())];
        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("missing argument 'handler'"));
    }

    #[test]
    fn test_http_serve_port_validation() {
        let mut interp = Interpreter::new();
        interp.env.define_force(
            "httpServe",
            Value::Str("__net_builtin_httpServe".into()),
        );
        let handler = make_handler_expr("ok");
        let args = vec![Expr::IntLit(99999, dummy_span()), handler];
        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("port must be 0-65535"));
    }

    /// TCP fragmentation: head split across two writes must still succeed
    #[test]
    fn test_http_serve_split_head() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18500);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("split-ok"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client =
            std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Send head in two fragments with a small delay between them
        std::io::Write::write_all(&mut client, b"GET / HT").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::io::Write::write_all(&mut client, b"TP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();

        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        assert!(
            response_str.contains("200 OK"),
            "Split head should succeed, got: {}",
            response_str
        );
        assert!(response_str.contains("split-ok"));

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// TCP fragmentation: body arriving after head in a separate write.
    /// 200 OK proves the server waited for the full body (incomplete bodies get 400).
    #[test]
    fn test_http_serve_split_body() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18600);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("body-ok"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client =
            std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Send complete head with Content-Length, but body arrives in a separate write
        std::io::Write::write_all(
            &mut client,
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 11\r\n\r\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::io::Write::write_all(&mut client, b"hello world").unwrap();

        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        // 200 OK proves the server waited for the full 11-byte body;
        // an incomplete body would have resulted in 400.
        assert!(
            response_str.contains("200 OK"),
            "Split body should succeed (200 proves full body arrived), got: {}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(1));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// Incomplete body: Content-Length declares 100 bytes but client sends only 5 then closes.
    /// Server must return 400, not pass truncated body to handler.
    #[test]
    fn test_http_serve_incomplete_body_returns_400() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18700);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("should-not-reach"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client =
            std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Send head claiming 100-byte body, but only send 5 bytes then close
        std::io::Write::write_all(
            &mut client,
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\nhello",
        )
        .unwrap();
        // Shut down the write side to signal EOF to the server
        let _ = std::net::TcpStream::shutdown(&client, std::net::Shutdown::Write);

        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        assert!(
            response_str.contains("400 Bad Request"),
            "Incomplete body must be rejected with 400, got: {}",
            response_str
        );
        // Response must NOT contain handler's body (handler should never be called)
        assert!(
            !response_str.contains("should-not-reach"),
            "Handler must not be called for incomplete body"
        );

        let _ = server_handle.join();
    }

    /// EOF during head: client connects then immediately closes without sending any data.
    /// Server must count it as a request and not hang.
    #[test]
    fn test_http_serve_eof_during_head_counts_request() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18800);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("should-not-reach"),
                Expr::IntLit(1, dummy_span()), // maxRequests=1
                Expr::IntLit(3000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connect and immediately close (EOF before any HTTP data)
        let client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        drop(client); // close immediately

        // Server should terminate because maxRequests=1 is reached
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(1));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// Close after partial head: client sends an incomplete request line then closes.
    /// Server must return 400 and count it.
    #[test]
    fn test_http_serve_close_after_partial_head() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18810);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                "httpServe",
                Value::Str("__net_builtin_httpServe".into()),
            );
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("should-not-reach"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(3000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client =
            std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Send partial HTTP request (no \r\n\r\n terminator) then close
        std::io::Write::write_all(&mut client, b"GET /hello HTTP/1.1\r\nHost: loc").unwrap();
        let _ = std::net::TcpStream::shutdown(&client, std::net::Shutdown::Write);

        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        // Server should respond with 400 for incomplete head
        let resp = String::from_utf8_lossy(&response);
        assert!(
            resp.contains("400 Bad Request"),
            "Partial head should get 400, got: {}",
            resp
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(1));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }
}
