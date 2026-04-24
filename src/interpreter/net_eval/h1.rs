//! HTTP/1.1 serve loop, connection dispatcher, and HTTP accessor helpers
//! for the Taida interpreter's net package, split out from
//! `net_eval/mod.rs` (C13-3).
//!
//! Owns the `impl Interpreter` methods that implement the HTTP/1.1 accept
//! path (`eval_http_serve`), the per-connection non-blocking request head
//! read (`try_read_request`), and the request-body + handler + response
//! dispatcher (`dispatch_request`) used by both 1-arg and 2-arg handlers.
//! Also hosts `eval_net_bytes_arg`, the tiny `Bytes`/`Str` coercion used
//! by the net dispatcher in `mod.rs`.
//!
//! The H2 / H3 forks live in `h2.rs` / `h3.rs`; when `protocol: "h2"` or
//! `protocol: "h3"` is negotiated, this module calls `self.serve_h2(...)`
//! or `self.serve_h3(...)` and returns.
//!
//! C13-3 note: pure mechanical move — no behavior change.

use super::super::eval::{Interpreter, RuntimeError, Signal};
use super::super::value::Value;
use super::helpers::{
    ChunkedBodyError, build_streaming_head, chunked_body_complete, chunked_in_place_compact,
    determine_keep_alive, extract_result_value, extract_result_value_owned, get_field_bool,
    get_field_int, get_field_value, make_fulfilled_async, make_result_failure_msg,
    make_result_success, make_span, parse_request_head, send_response_scatter,
};
use super::types::{
    ActiveStreamingWriter, ConnAction, ConnReadResult, ConnStream, HttpConnection,
    RequestBodyState, StreamingWriter, WriterState,
};
use crate::net_surface::http_protocol_ordinal_to_wire;
use crate::parser::Expr;

/// C26B-022 Step 2 (wE Round 3, 2026-04-24): HTTP wire byte upper
/// limits enforced at the parser boundary so that downstream Native
/// codegen fixed-size stack buffers (`char method[16]` / `char path[2048]`
/// / `char authority[256]`) can never silently truncate.
///
/// Option confirmation: **Step 3 Option B** (parser-level reject with
/// `400 Bad Request`). Option A (dynamic buffers) was discarded at
/// Phase 0 Design Lock because it conflicts with the clone-heavy
/// abstraction direction of C26B-018/020/024.
///
/// These constants are the authoritative limits for 3-backend parity;
/// the Native C codegen struct field sizes must match (see
/// `src/codegen/native_runtime/net_h1_h2.c`). Interpreter enforces
/// these here; any over-limit wire byte is rejected with HTTP 400
/// before the handler is ever invoked.
pub(crate) const HTTP_WIRE_MAX_METHOD_LEN: usize = 16;
pub(crate) const HTTP_WIRE_MAX_PATH_LEN: usize = 2048;
/// Authority (Host header value) wire-byte cap. Struct field size in
/// `net_h1_h2.c` (`char authority[256]`) and `net_h3_quic.c` matches this
/// value to guarantee no silent truncation on Native codegen.
pub(crate) const HTTP_WIRE_MAX_AUTHORITY_LEN: usize = 256;

/// Extract the `start` and `len` fields from a `@(start: Int, len: Int)`
/// span pack. Returns `(0, 0)` if the shape does not match (conservative:
/// zero-length never exceeds any limit, so malformed packs do not
/// fail-fast here; the normal parse path handles them).
fn span_start_len(v: &Value) -> (usize, usize) {
    if let Value::BuchiPack(fields) = v {
        let mut start = 0usize;
        let mut len = 0usize;
        for (k, vv) in fields.iter() {
            if let Value::Int(n) = vv {
                match k.as_str() {
                    "start" => start = (*n).max(0) as usize,
                    "len" => len = (*n).max(0) as usize,
                    _ => {}
                }
            }
        }
        return (start, len);
    }
    (0, 0)
}

fn span_len(v: &Value) -> usize {
    span_start_len(v).1
}

/// Compare a header name span against an ASCII-lowercase reference name
/// (case-insensitive). Returns true on match; returns false on any bound
/// violation so over-limit / malformed spans never claim to be "Host".
fn span_equals_ascii_ci(raw: &[u8], start: usize, len: usize, reference: &[u8]) -> bool {
    if len != reference.len() {
        return false;
    }
    let end = match start.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    if end > raw.len() {
        return false;
    }
    raw[start..end]
        .iter()
        .zip(reference.iter())
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// C26B-022 Step 2 (wJ Round 4, 2026-04-24 authority extension):
/// Check if method / path / Host-header-value exceeds its wire-limit.
///
/// Returns `Some(&'static str)` with a short descriptor of the violating
/// field when over-limit, `None` otherwise.
///
/// The `raw` buffer is required so that header name/value spans (which
/// are zero-copy references into the HTTP request buffer) can be
/// resolved to concrete byte slices. The `headers` field is iterated;
/// for each entry whose `name` span case-insensitively equals `b"host"`,
/// its `value` span length is compared against
/// [`HTTP_WIRE_MAX_AUTHORITY_LEN`].
///
/// Host is the HTTP/1.1 equivalent of the H2/H3 `:authority` pseudo-
/// header; the Native codegen struct field (`char authority[256]`) is
/// populated from whichever is present, so enforcement on `Host` here
/// keeps the 3-backend parity story consistent.
pub(crate) fn check_http_wire_limits(
    parsed_fields: &[(String, Value)],
    raw: &[u8],
) -> Option<&'static str> {
    for (k, v) in parsed_fields {
        match k.as_str() {
            "method" if span_len(v) > HTTP_WIRE_MAX_METHOD_LEN => {
                return Some("method");
            }
            "path" if span_len(v) > HTTP_WIRE_MAX_PATH_LEN => {
                return Some("path");
            }
            "headers" => {
                if let Value::List(items) = v {
                    for header in items.iter() {
                        if let Value::BuchiPack(hf) = header {
                            // Resolve name + value spans from header pack
                            let mut name_span: Option<&Value> = None;
                            let mut value_span: Option<&Value> = None;
                            for (hk, hv) in hf.iter() {
                                match hk.as_str() {
                                    "name" => name_span = Some(hv),
                                    "value" => value_span = Some(hv),
                                    _ => {}
                                }
                            }
                            if let (Some(ns), Some(vs)) = (name_span, value_span) {
                                let (n_start, n_len) = span_start_len(ns);
                                if span_equals_ascii_ci(raw, n_start, n_len, b"host") {
                                    let (_, v_len) = span_start_len(vs);
                                    if v_len > HTTP_WIRE_MAX_AUTHORITY_LEN {
                                        return Some("authority");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

impl Interpreter {
    // ── httpServe implementation ───────────────────────────────
    //
    // httpServe(port, handler, maxRequests <= 0, timeoutMs <= 5000, maxConnections <= 128, tls <= @())
    //   → Async[Result[@(ok: Bool, requests: Int), _]]
    //
    // v5: tls parameter added. When tls is non-empty @(cert: Str, key: Str),
    //   the server runs HTTPS. When tls is @() (default), plaintext HTTP.
    //   TLS implementation is Phase 2; Phase 1 only parses and validates the arg.
    //
    // v2 concurrency model (NET2-3):
    //   The Interpreter is single-threaded (!Send, &mut self for handler eval).
    //   Concurrency is achieved via non-blocking accept + connection pool:
    //   - Listener is set to non-blocking mode
    //   - Active connections are held in a bounded pool (maxConnections)
    //   - Each connection has its own dedicated buffer (no sharing)
    //   - Main loop: try_accept → poll each connection → process first ready request
    //   - Handler execution is serial (one at a time) since &mut self
    //   - This provides IO-level concurrency: multiple clients can be connected
    //     simultaneously, with requests dispatched in round-robin order.
    //
    // - Binds to 127.0.0.1:port (fixed, never 0.0.0.0)
    // - Keep-alive within each connection
    // - maxRequests > 0 → bounded shutdown after N total requests (across all connections)
    // - maxRequests = 0 → run indefinitely
    // - maxConnections limits simultaneous open connections
    // - No httpClose, no streaming

    pub(super) fn eval_http_serve(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // ── Arg 0: port (required, Int) ──
        let port: u16 = match args.first() {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => {
                    if !(0..=65535).contains(&n) {
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
        // NB-5: timeoutMs <= 0 falls back to 5000ms (v1 default).
        // Duration::ZERO is OS-undefined for set_read_timeout; 0 must not reach the OS.
        let timeout_ms: u64 = match args.get(3) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => {
                    if n <= 0 {
                        5000 // fallback to default
                    } else {
                        n as u64
                    }
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

        // ── Arg 4: maxConnections (optional, default 128) ──
        // NET2-3c: Bounds the number of simultaneous open connections.
        let max_connections: usize = match args.get(4) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => {
                    if n <= 0 {
                        128 // fallback to default
                    } else {
                        n as usize
                    }
                }
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("httpServe: maxConnections must be Int, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            },
            None => 128,
        };

        // ── Arg 5: tls (optional, default @() = plaintext) ──
        // v5: TLS configuration. @() means plaintext (v4 compat).
        // @(cert: "path", key: "path") means HTTPS (HTTP/1.1 over TLS).
        // @(cert: "path", key: "path", protocol: "h2") means HTTP/2 over TLS.
        // v6 NET6-1b: protocol field support for h2 opt-in.
        let tls_cert_path: Option<String>;
        let tls_key_path: Option<String>;
        let mut requested_protocol: Option<String> = None;
        match args.get(5) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(Value::BuchiPack(fields)) => {
                    if fields.is_empty() {
                        // @() → plaintext
                        tls_cert_path = None;
                        tls_key_path = None;
                    } else {
                        // v6 NET6-1b: Extract protocol field if present.
                        // NB6-10: Separate "field exists" from "field is Str".
                        // If protocol field exists but is not Str, reject immediately.
                        if let Some((_, proto_val)) = fields.iter().find(|(k, _)| k == "protocol") {
                            match proto_val {
                                Value::Str(proto) => {
                                    requested_protocol = Some(proto.as_string().clone());
                                }
                                Value::Int(ordinal) => {
                                    if let Some(protocol) = http_protocol_ordinal_to_wire(*ordinal)
                                    {
                                        requested_protocol = Some(protocol.to_string());
                                    } else {
                                        let result = make_result_failure_msg(
                                            "ProtocolError",
                                            format!(
                                                "httpServe: unknown HttpProtocol ordinal {}. Expected 0 (H1), 1 (H2), or 2 (H3).",
                                                ordinal
                                            ),
                                        );
                                        return Ok(Some(Signal::Value(make_fulfilled_async(
                                            result,
                                        ))));
                                    }
                                }
                                _ => {
                                    let result = make_result_failure_msg(
                                        "ProtocolError",
                                        format!(
                                            "httpServe: protocol must be HttpProtocol or Str, got {}",
                                            proto_val
                                        ),
                                    );
                                    return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                                }
                            }
                        }

                        // Extract cert and key fields.
                        let cert = fields
                            .iter()
                            .find(|(k, _)| k == "cert")
                            .map(|(_, v)| v.clone());
                        let key = fields
                            .iter()
                            .find(|(k, _)| k == "key")
                            .map(|(_, v)| v.clone());

                        match (cert, key) {
                            (Some(Value::Str(c)), Some(Value::Str(k))) => {
                                tls_cert_path = Some(Value::str_take(c));
                                tls_key_path = Some(Value::str_take(k));
                            }
                            (Some(Value::Str(_)), _) => {
                                // NB5-16: Return Result failure(TlsError) for startup config errors
                                // (NET5-0c: startup failure = Result failure), matching JS/Native parity.
                                let result = make_result_failure_msg(
                                    "TlsError",
                                    "httpServe: tls.key must be a Str (PEM file path)",
                                );
                                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                            }
                            (_, Some(Value::Str(_))) => {
                                let result = make_result_failure_msg(
                                    "TlsError",
                                    "httpServe: tls.cert must be a Str (PEM file path)",
                                );
                                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                            }
                            _ => {
                                // v6 NET6-1b: Allow @(protocol: "h2") without cert/key
                                // to still trigger protocol validation below.
                                if requested_protocol.is_some() {
                                    tls_cert_path = None;
                                    tls_key_path = None;
                                } else {
                                    let result = make_result_failure_msg(
                                        "TlsError",
                                        "httpServe: tls must be @(cert: Str, key: Str) or @()",
                                    );
                                    return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                                }
                            }
                        }
                    }
                }
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!(
                            "httpServe: tls must be a BuchiPack @(cert: Str, key: Str) or @(), got {}",
                            v
                        ),
                    });
                }
                other => return Ok(Some(other)),
            },
            None => {
                // Default: plaintext (v4 compat)
                tls_cert_path = None;
                tls_key_path = None;
            }
        }

        // v6 NET6-1b / NET6-2a / v7 NET7-1c: Protocol validation.
        // HTTP/2 and HTTP/3 are opt-in. Unknown protocol values are rejected immediately.
        // v7: "h3" is now a recognized protocol value but not yet implemented (Phase 2/3).
        let is_h2 = match requested_protocol.as_deref() {
            Some("h1.1") | Some("http/1.1") => false, // Explicit HTTP/1.1
            Some("h2") => {
                // v6 NET6-2a: HTTP/2 requires TLS (h2c is out of scope).
                if tls_cert_path.is_none() || tls_key_path.is_none() {
                    let result = make_result_failure_msg(
                        "ProtocolError",
                        "httpServe: HTTP/2 (protocol: \"h2\") requires TLS (cert + key). \
                         Cleartext HTTP/2 (h2c) is not supported.",
                    );
                    return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                }
                true
            }
            Some("h3") => {
                // v7 NET7-1c: HTTP/3 requires TLS (cert + key). Validate before
                // dispatching to the h3 serve path so the cert/key contract
                // is established from Phase 1 onward.
                if tls_cert_path.is_none() || tls_key_path.is_none() {
                    let result = make_result_failure_msg(
                        "ProtocolError",
                        "httpServe: HTTP/3 (protocol: \"h3\") requires TLS (cert + key).",
                    );
                    return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                }
                // v7 Phase 3 (NET7-3a): Dispatch to H3 serve path.
                // This mirrors the Native backend's taida_net_h3_serve():
                //   1. Run H3 protocol layer self-tests (QPACK round-trip, request validation)
                //   2. Gate on QUIC transport availability
                //   3. Return appropriate error/success
                return self.serve_h3(
                    tls_cert_path.clone().unwrap(),
                    tls_key_path.clone().unwrap(),
                    handler,
                    max_requests,
                    port,
                );
            }
            Some(other) => {
                let result = make_result_failure_msg(
                    "ProtocolError",
                    format!(
                        "httpServe: unknown protocol \"{}\". \
                         Supported values: \"h1.1\", \"h2\", \"h3\"",
                        other
                    ),
                );
                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
            }
            None => false,
        };

        // v5/v6: Load TLS config if cert/key provided.
        // NET6-2a: If h2 is requested, use ALPN-enabled config.
        let tls_config: Option<std::sync::Arc<rustls::ServerConfig>> =
            match (tls_cert_path.as_deref(), tls_key_path.as_deref()) {
                (Some(cert), Some(key)) => {
                    let load_result = if is_h2 {
                        super::super::net_transport::load_tls_config_h2(cert, key)
                    } else {
                        super::super::net_transport::load_tls_config(cert, key)
                    };
                    match load_result {
                        Ok(config) => Some(config),
                        Err(msg) => {
                            // cert/key load failure → startup Result failure (NET5-0c).
                            let result = make_result_failure_msg("TlsError", msg);
                            return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                        }
                    }
                }
                _ => None, // plaintext (v4 compat)
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

        // NET2-3a/3b: Set listener to non-blocking for concurrent accept.
        // This allows the main loop to poll for new connections without blocking
        // while also servicing existing connections.
        listener.set_nonblocking(true).map_err(|e| RuntimeError {
            message: format!("httpServe: failed to set non-blocking: {}", e),
        })?;

        let read_timeout = std::time::Duration::from_millis(timeout_ms);

        // ── NET6-2a: HTTP/2 serve loop (Interpreter reference implementation) ──
        if is_h2 {
            return self.serve_h2(
                listener,
                tls_config.expect("h2 requires TLS"),
                handler,
                max_requests,
                max_connections,
                read_timeout,
            );
        }

        // Short poll timeout for non-blocking read attempts on connections.
        // This controls how quickly we cycle through the connection pool.
        let poll_timeout = std::time::Duration::from_millis(10);

        // ── NET2-3: Concurrent connection pool ──
        //
        // Connection pool: Vec of active connections, each with its own buffer.
        // Main loop:
        //   1. Try to accept new connections (non-blocking) up to maxConnections
        //   2. Round-robin poll each connection for a ready request
        //   3. Process the first ready request (handler is &mut self, serial)
        //   4. Remove closed/errored connections
        //
        // This provides IO-level concurrency: multiple clients can connect and
        // send data simultaneously. Handler dispatch is serial (one at a time).

        let mut request_count: i64 = 0;
        let mut connections: Vec<HttpConnection> = Vec::new();
        // Round-robin index for fair scheduling across connections
        let mut poll_start: usize = 0;

        loop {
            // Check bounded shutdown (total requests across all connections)
            if max_requests > 0 && request_count >= max_requests {
                break;
            }

            // ── Step 1: Accept new connections (non-blocking) ──
            while connections.len() < max_connections {
                match listener.accept() {
                    Ok((tcp_stream, peer_addr)) => {
                        // v5: If TLS is configured, perform TLS handshake on the accepted TCP stream.
                        let conn_stream = if let Some(ref tls_cfg) = tls_config {
                            // Set blocking mode with handshake deadline.
                            let _ = tcp_stream.set_nonblocking(false);
                            let _ = tcp_stream.set_read_timeout(Some(read_timeout));
                            let _ = tcp_stream.set_write_timeout(Some(read_timeout));

                            let tls_conn = match rustls::ServerConnection::new(tls_cfg.clone()) {
                                Ok(c) => c,
                                Err(_) => continue, // TLS setup error, skip connection
                            };
                            let mut tls_transport = super::super::net_transport::TlsTransport::new(
                                tls_conn, tcp_stream,
                            );
                            match super::super::net_transport::complete_tls_handshake(
                                &mut tls_transport,
                                read_timeout,
                            ) {
                                Ok(()) => ConnStream::Tls(Box::new(tls_transport)),
                                Err(_) => {
                                    // NET5-0c: handshake failure = close + don't call handler.
                                    continue;
                                }
                            }
                        } else {
                            ConnStream::Plain(tcp_stream)
                        };

                        // Set short read timeout for polling readiness
                        let _ = conn_stream.set_read_timeout(Some(poll_timeout));
                        connections.push(HttpConnection {
                            stream: conn_stream,
                            peer_addr,
                            buf: vec![0u8; 8192],
                            total_read: 0,
                            conn_requests: 0,
                            last_activity: std::time::Instant::now(),
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break; // no pending connections
                    }
                    Err(e) => {
                        // Accept failure → return error result (fatal)
                        let result = make_result_failure_msg(
                            "AcceptError",
                            format!("httpServe: accept failed: {}", e),
                        );
                        return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                    }
                }
            }

            // If no connections and we need to wait, sleep briefly to avoid busy-spin
            if connections.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }

            // ── Step 2: Poll connections round-robin for a ready request ──
            // Try each connection once, starting from poll_start for fairness.
            let n = connections.len();
            let mut processed_idx: Option<usize> = None;
            let mut close_idx: Option<usize> = None;

            for offset in 0..n {
                let idx = (poll_start + offset) % n;
                let conn = &mut connections[idx];

                // Check idle timeout — applies to all connections.
                // For first-request connections (conn_requests == 0):
                //   - No data at all: clean close, no request budget consumed.
                //   - Partial data: send 400 (malformed), count as request.
                // For keep-alive connections (conn_requests > 0):
                //   - No partial data (true idle): clean close, no 400.
                //   - Partial data present: send 400 (malformed), count as request.
                if conn.last_activity.elapsed() > read_timeout {
                    if conn.total_read > 0 {
                        // Partial data present: bad request, counts toward budget
                        let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                        request_count += 1;
                    }
                    // else: no data at all — clean close, no request budget consumed
                    // After 400 or clean idle close: remove connection.
                    close_idx = Some(idx);
                    break;
                }

                // Try to read data (non-blocking due to short timeout)
                let read_result = Self::try_read_request(conn);
                match read_result {
                    ConnReadResult::Ready(
                        parsed_fields,
                        head_consumed,
                        content_length,
                        is_chunked,
                        had_content_length_header,
                    ) => {
                        // We have a complete request head. Process body + handler.
                        // Advance round-robin past this connection.
                        poll_start = (idx + 1) % n;
                        processed_idx = Some(idx);

                        // ── Body reading + handler dispatch ──
                        // Set blocking timeout for body read (need full body)
                        let _ = conn.stream.set_read_timeout(Some(read_timeout));

                        // ── C26B-022 Step 2 (wE Round 3 + wJ Round 4): ──
                        // Enforce HTTP wire byte upper limits at the parser
                        // boundary to prevent silent truncation when these
                        // fields reach the Native codegen fixed-size stack
                        // buffers (`char method[16]`, `char path[2048]`,
                        // `char authority[256]`). Reject with 400 before
                        // handler dispatch so that 3-backend parity is
                        // preserved and no handler sees a truncated value.
                        //
                        // wJ Round 4 (2026-04-24): extended to cover
                        // the Host header value (authority, 256 bytes),
                        // which is the HTTP/1.1 equivalent of the H2/H3
                        // `:authority` pseudo-header. The raw buffer
                        // slice is passed through so that the zero-copy
                        // header name span can be resolved to compare
                        // case-insensitively against "host".
                        //
                        // Additive widening (§ 6.2): this adds a reject
                        // path for previously-accepted oversized inputs
                        // that would have hit silent truncation downstream.
                        // No existing parity.rs assertion is altered.
                        if let Some(field_name) =
                            check_http_wire_limits(&parsed_fields, &conn.buf[..conn.total_read])
                        {
                            let _ = field_name; // kept for future logging
                            let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                            request_count += 1;
                            close_idx = processed_idx;
                            let _ = conn.stream.set_read_timeout(Some(poll_timeout));
                            break;
                        }

                        let dispatch_result = self.dispatch_request(
                            conn,
                            &handler,
                            parsed_fields,
                            head_consumed,
                            content_length,
                            is_chunked,
                            had_content_length_header,
                            &mut request_count,
                        );

                        // Restore short poll timeout
                        let _ = conn.stream.set_read_timeout(Some(poll_timeout));

                        match dispatch_result {
                            ConnAction::KeepAlive => {
                                conn.last_activity = std::time::Instant::now();
                                // Connection stays in pool
                            }
                            ConnAction::Close => {
                                close_idx = processed_idx;
                            }
                        }
                        break; // processed one request, loop back to accept + poll
                    }
                    ConnReadResult::Eof => {
                        if conn.total_read > 0 {
                            // Partial data received then EOF: bad request
                            let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                            request_count += 1;
                        }
                        // else: clean close with no data — no request budget consumed
                        close_idx = Some(idx);
                        break;
                    }
                    ConnReadResult::Timeout | ConnReadResult::NeedMore => {
                        // No complete request ready. The idle timeout check at the
                        // top of the loop handles the actual timeout logic.
                        // Just move on to the next connection.
                        continue;
                    }
                    ConnReadResult::Malformed => {
                        let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                        request_count += 1;
                        close_idx = Some(idx);
                        break;
                    }
                }
            }

            // ── Step 3: Remove closed connection ──
            if let Some(idx) = close_idx {
                connections.swap_remove(idx);
                // Adjust poll_start if needed
                if !connections.is_empty() {
                    poll_start %= connections.len();
                } else {
                    poll_start = 0;
                }
            }

            // If no connection was processed and none closed, all connections are
            // waiting for data. Brief sleep to avoid busy-spin.
            if processed_idx.is_none() && close_idx.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }

        // Server completed successfully
        let result_inner = Value::pack(vec![
            ("ok".into(), Value::Bool(true)),
            ("requests".into(), Value::Int(request_count)),
        ]);
        let result = make_result_success(result_inner);
        Ok(Some(Signal::Value(make_fulfilled_async(result))))
    }
    /// Try to read and parse a request head from a connection (non-blocking).
    /// Returns the parse state without modifying request_count.
    pub(super) fn try_read_request(conn: &mut HttpConnection) -> ConnReadResult {
        const MAX_REQUEST_BUF: usize = 1_048_576; // 1 MiB

        if conn.total_read >= MAX_REQUEST_BUF {
            return ConnReadResult::Malformed;
        }

        // Try to parse what we already have in the buffer
        if conn.total_read > 0 {
            let parse_result = parse_request_head(&conn.buf[..conn.total_read]);
            let completion_info = match extract_result_value(&parse_result) {
                None => return ConnReadResult::Malformed,
                Some(inner) => {
                    if get_field_bool(inner, "complete").unwrap_or(false) {
                        let consumed = get_field_int(inner, "consumed").unwrap_or(0) as usize;
                        let cl = get_field_int(inner, "contentLength").unwrap_or(0);
                        let is_chunked = get_field_bool(inner, "chunked").unwrap_or(false);
                        // C12B-032: internal-only presence bit; absent in
                        // legacy fixtures → conservative false.
                        let had_cl_header =
                            get_field_bool(inner, "__hasContentLengthHeader").unwrap_or(false);
                        Some((consumed, cl, is_chunked, had_cl_header))
                    } else {
                        None
                    }
                }
            };
            if let Some((consumed, cl, is_chunked, had_cl_header)) = completion_info {
                match extract_result_value_owned(parse_result) {
                    Some(fields) => {
                        return ConnReadResult::Ready(
                            fields,
                            consumed,
                            cl,
                            is_chunked,
                            had_cl_header,
                        );
                    }
                    None => return ConnReadResult::Malformed,
                }
            }
        }

        // Need more data -- try a non-blocking read
        if conn.total_read == conn.buf.len() {
            conn.buf
                .resize(std::cmp::min(conn.buf.len() * 2, MAX_REQUEST_BUF), 0);
        }
        match std::io::Read::read(&mut conn.stream, &mut conn.buf[conn.total_read..]) {
            Ok(0) => ConnReadResult::Eof,
            Ok(n) => {
                conn.total_read += n;
                // Update last_activity on successful byte reception so that
                // slow-but-active clients (sending data within each timeout
                // window) are not incorrectly timed out.
                conn.last_activity = std::time::Instant::now();
                // Re-check parse after new data
                let parse_result = parse_request_head(&conn.buf[..conn.total_read]);
                let completion_info = match extract_result_value(&parse_result) {
                    None => return ConnReadResult::Malformed,
                    Some(inner) => {
                        if get_field_bool(inner, "complete").unwrap_or(false) {
                            let consumed = get_field_int(inner, "consumed").unwrap_or(0) as usize;
                            let cl = get_field_int(inner, "contentLength").unwrap_or(0);
                            let is_chunked = get_field_bool(inner, "chunked").unwrap_or(false);
                            let had_cl_header =
                                get_field_bool(inner, "__hasContentLengthHeader").unwrap_or(false);
                            Some((consumed, cl, is_chunked, had_cl_header))
                        } else {
                            None
                        }
                    }
                };
                match completion_info {
                    Some((consumed, cl, is_chunked, had_cl_header)) => {
                        match extract_result_value_owned(parse_result) {
                            Some(fields) => ConnReadResult::Ready(
                                fields,
                                consumed,
                                cl,
                                is_chunked,
                                had_cl_header,
                            ),
                            None => ConnReadResult::Malformed,
                        }
                    }
                    None => ConnReadResult::NeedMore,
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Check if we have partial data that could be parsed
                if conn.total_read > 0 {
                    ConnReadResult::Timeout
                } else {
                    ConnReadResult::NeedMore // no data at all yet
                }
            }
            Err(_) => ConnReadResult::Eof,
        }
    }
    /// Dispatch a single request on a connection: read body, call handler, write response.
    /// Returns whether to keep the connection alive or close it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_request(
        &mut self,
        conn: &mut HttpConnection,
        handler: &super::super::value::FuncValue,
        parsed_fields: Vec<(String, Value)>,
        head_consumed: usize,
        content_length: i64,
        is_chunked: bool,
        had_content_length_header: bool,
        request_count: &mut i64,
    ) -> ConnAction {
        const MAX_REQUEST_BUF: usize = 1_048_576; // 1 MiB

        // ── NET3-1a / NET4-1a: Detect handler arity before body reading ──
        // 1-arg handler = v2 one-shot response path (eager body read)
        // 2-arg handler = v3/v4 streaming path (body read deferred for v4)
        let handler_arity = handler.params.len();

        if handler_arity >= 2 {
            // ── v4 2-arg handler path: body-deferred ──
            // Do NOT eagerly read body. Only read the head.
            // Body will be read on demand via readBodyChunk/readBodyAll.

            // NB5-20: Detach head bytes from scratch buffer (owned copy).
            // This to_vec() is necessary because `Value` requires `'static` lifetime —
            // the connection scratch buffer (`conn.buf`) is reused/overwritten on keep-alive,
            // so we cannot borrow from it into a `Value::Bytes` that outlives this scope.
            let raw_bytes = conn.buf[..head_consumed].to_vec();

            // Determine keep-alive from head bytes.
            let http_minor = match get_field_value(&parsed_fields, "version") {
                Some(Value::BuchiPack(ver_fields)) => {
                    get_field_int(ver_fields, "minor").unwrap_or(1)
                }
                _ => 1,
            };
            let keep_alive = match get_field_value(&parsed_fields, "headers") {
                Some(Value::List(headers)) => determine_keep_alive(&raw_bytes, headers, http_minor),
                _ => http_minor == 1,
            };

            // NB5-20: Capture any leftover body bytes already in conn.buf (beyond head).
            // Same as raw_bytes above — `Value` is `'static`, so we cannot borrow from
            // `conn.buf`. The leftover is typically 0–a few KB (body bytes that arrived
            // in the same TCP segment as the head), so this copy is acceptable.
            let leftover = if conn.total_read > head_consumed {
                conn.buf[head_consumed..conn.total_read].to_vec()
            } else {
                Vec::new()
            };

            // Create mutable StreamingWriter for this request scope.
            let mut writer = StreamingWriter::new();

            // v4: Create body streaming state for readBodyChunk/readBodyAll.
            // Must be created before request pack so we can embed the token.
            let mut body_state = RequestBodyState::new(
                is_chunked,
                content_length,
                had_content_length_header,
                leftover,
            );

            // Build request pack for handler (head only, body = empty span).
            let mut request_fields: Vec<(String, Value)> = Vec::new();
            request_fields.push(("raw".into(), Value::bytes(raw_bytes)));

            for key in &["method", "path", "query", "version", "headers"] {
                if let Some(v) = get_field_value(&parsed_fields, key) {
                    request_fields.push((key.to_string(), v.clone()));
                }
            }

            // v4: body span is empty (body not yet read).
            request_fields.push(("body".into(), make_span(0, 0)));
            request_fields.push(("bodyOffset".into(), Value::Int(head_consumed as i64)));
            request_fields.push(("contentLength".into(), Value::Int(content_length)));
            request_fields.push((
                "remoteHost".into(),
                Value::str(conn.peer_addr.ip().to_string()),
            ));
            request_fields.push((
                "remotePort".into(),
                Value::Int(conn.peer_addr.port() as i64),
            ));
            request_fields.push(("keepAlive".into(), Value::Bool(keep_alive)));
            request_fields.push(("chunked".into(), Value::Bool(is_chunked)));
            // v4: sentinel to identify this request pack as body-streaming capable.
            request_fields.push((
                "__body_stream".into(),
                Value::str("__v4_body_stream".into()),
            ));
            // NB4-7: Request-scoped token for identity verification.
            request_fields.push((
                "__body_token".into(),
                Value::Int(body_state.request_token as i64),
            ));

            let request_pack = Value::pack(request_fields);

            // Create writer BuchiPack with sentinel for identification.
            let writer_pack = Value::pack(vec![(
                "__writer_id".into(),
                Value::str("__v3_streaming_writer".into()),
            )]);

            // NET3-2 + NET4-1a: Install active_streaming_writer with body_state pointer.
            self.active_streaming_writer = Some(ActiveStreamingWriter {
                writer: &mut writer as *mut StreamingWriter,
                stream: &mut conn.stream as *mut ConnStream,
                borrowed: false,
                body_state: &mut body_state as *mut RequestBodyState,
                ws_closed: false,
                ws_token: 0,
                ws_close_code: 0, // v5: no close frame received yet
            });

            let handler_result =
                self.call_function_with_values(handler, &[request_pack, writer_pack]);

            // Save WebSocket close state before clearing active writer.
            let ws_was_closed = self
                .active_streaming_writer
                .as_ref()
                .is_some_and(|a| a.ws_closed);

            // Clear the active writer — handler execution is done.
            self.active_streaming_writer = None;

            let response_value = match handler_result {
                Ok(v) => v,
                Err(e) => {
                    // v4: WebSocket state — send close frame on error.
                    if writer.state == WriterState::WebSocket {
                        // Send close frame with 1011 (internal error) if not already closed.
                        if !ws_was_closed {
                            let close_payload = [0x03, 0xF3u8]; // 1011
                            let _ = Self::write_ws_frame(&mut conn.stream, 0x8, &close_payload);
                        }
                        // C12B-041: flush + graceful half-close + short drain so
                        // the client observes the 1011 frame + FIN cleanly.
                        Self::finalize_websocket_close(&mut conn.stream);
                        *request_count += 1;
                        return ConnAction::Close;
                    }
                    if writer.state == WriterState::Streaming {
                        let _ = std::io::Write::write_all(&mut conn.stream, b"0\r\n\r\n");
                        writer.state = WriterState::Ended;
                        *request_count += 1;
                        return ConnAction::Close;
                    }
                    if writer.state == WriterState::Ended {
                        *request_count += 1;
                        return ConnAction::Close;
                    }
                    writer.state = WriterState::Ended;
                    let error_body = format!("Internal Server Error: {}", e.message);
                    let error_response = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        error_body.len(),
                        error_body
                    );
                    let _ = std::io::Write::write_all(&mut conn.stream, error_response.as_bytes());
                    *request_count += 1;
                    return ConnAction::Close;
                }
            };

            // ── v4: WebSocket auto-close on handler return ──
            if writer.state == WriterState::WebSocket {
                // Auto-close if not already closed.
                if !ws_was_closed {
                    let close_payload = [0x03, 0xE8u8]; // 1000 normal closure
                    let _ = Self::write_ws_frame(&mut conn.stream, 0x8, &close_payload);
                }
                // C12B-041: flush + graceful half-close + short drain so the
                // client observes the data frames + close frame + FIN cleanly
                // before the process exits (fixes flaky
                // test_net4_ws_auto_close_on_return_interp).
                Self::finalize_websocket_close(&mut conn.stream);
                *request_count += 1;
                conn.conn_requests += 1;
                conn.total_read = 0;
                // WebSocket connections never return to keep-alive.
                return ConnAction::Close;
            }

            // ── NET3-1a: One-shot fallback for 2-arg handler ──
            if writer.state == WriterState::Idle {
                let is_response_pack = matches!(&response_value, Value::BuchiPack(fields)
                    if fields.iter().any(|(k, _)| k == "status" || k == "body"));
                let effective_response = if is_response_pack {
                    response_value
                } else {
                    Value::pack(vec![
                        ("status".into(), Value::Int(200)),
                        ("headers".into(), Value::list(vec![])),
                        ("body".into(), Value::str(String::new())),
                    ])
                };

                // NB6-1: Scatter-gather send — head and body as separate buffers.
                if send_response_scatter(&mut conn.stream, &effective_response).is_err() {
                    let fallback = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = std::io::Write::write_all(&mut conn.stream, fallback);
                    *request_count += 1;
                    return ConnAction::Close;
                }
            } else {
                // Streaming was started. The return value is ignored.
                // Auto-end if not already ended.
                if writer.state != WriterState::Ended {
                    if writer.state == WriterState::HeadPrepared {
                        let head_bytes =
                            build_streaming_head(writer.pending_status, &writer.pending_headers);
                        let _ = std::io::Write::write_all(&mut conn.stream, &head_bytes);
                    }
                    if !StreamingWriter::is_bodyless_status(writer.pending_status) {
                        let _ = std::io::Write::write_all(&mut conn.stream, b"0\r\n\r\n");
                    }
                    writer.state = WriterState::Ended;
                }
            }

            *request_count += 1;
            conn.conn_requests += 1;

            // v4 NET4-1g: If body was not fully read, do NOT return to keep-alive.
            // The socket read buffer may contain unread body bytes that would
            // corrupt the next request's head parse.
            let body_done = body_state.fully_read || (!is_chunked && content_length == 0);
            if !body_done || !keep_alive {
                // Reset conn buffer and close.
                conn.total_read = 0;
                return ConnAction::Close;
            }

            // NB5-24: Recover any trailing bytes from body_state leftover.
            // When a pipelined client sends the next request in the same TCP segment
            // as the current body, those bytes end up in body_state.leftover beyond
            // the body data. We must copy them back into conn.buf so the keep-alive
            // loop can parse the next request from them.
            let trailing = if body_state.leftover_pos < body_state.leftover.len() {
                body_state.leftover[body_state.leftover_pos..].to_vec()
            } else {
                Vec::new()
            };

            if !trailing.is_empty() {
                // Ensure conn.buf is large enough for the trailing bytes.
                let needed = trailing.len();
                if conn.buf.len() < needed {
                    conn.buf.resize(std::cmp::max(8192, needed), 0);
                }
                conn.buf[..needed].copy_from_slice(&trailing);
                conn.total_read = needed;
            } else {
                conn.total_read = 0;
                if conn.buf.len() < 8192 {
                    conn.buf.resize(8192, 0);
                }
            }

            ConnAction::KeepAlive
        } else {
            // ── v2 1-arg handler path (unchanged) ──
            // Eager body read as before.

            let body_result = if is_chunked {
                // ── NET2-2: Chunked Transfer Encoding ──
                let completeness = loop {
                    let check = chunked_body_complete(&conn.buf[..conn.total_read], head_consumed);
                    match check {
                        Ok(wire_used) => break Ok(wire_used),
                        Err(ChunkedBodyError::Incomplete(_)) => {
                            if conn.total_read >= MAX_REQUEST_BUF {
                                break Err("Chunked body exceeds buffer limit".to_string());
                            }
                            if conn.total_read == conn.buf.len() {
                                conn.buf
                                    .resize(std::cmp::min(conn.buf.len() * 2, MAX_REQUEST_BUF), 0);
                            }
                            match std::io::Read::read(
                                &mut conn.stream,
                                &mut conn.buf[conn.total_read..],
                            ) {
                                Ok(0) => {
                                    break Err("Chunked body incomplete: connection closed".into());
                                }
                                Ok(n) => conn.total_read += n,
                                Err(ref e)
                                    if e.kind() == std::io::ErrorKind::WouldBlock
                                        || e.kind() == std::io::ErrorKind::TimedOut =>
                                {
                                    break Err("Chunked body incomplete: timeout".into());
                                }
                                Err(_) => break Err("Chunked body incomplete: read error".into()),
                            }
                        }
                        Err(ChunkedBodyError::Malformed(msg)) => break Err(msg),
                    }
                };

                match completeness {
                    Ok(_scan_wire) => {
                        match chunked_in_place_compact(&mut conn.buf, head_consumed) {
                            Ok(compact) => {
                                let total_wire = head_consumed + compact.wire_consumed;
                                Ok((
                                    total_wire,
                                    head_consumed,
                                    compact.body_len,
                                    compact.body_len as i64,
                                    true,
                                ))
                            }
                            Err(_msg) => {
                                let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                                let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                                *request_count += 1;
                                Err(())
                            }
                        }
                    }
                    Err(_msg) => {
                        let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                        *request_count += 1;
                        Err(())
                    }
                }
            } else {
                // ── Content-Length path (v1 behavior) ──
                if head_consumed + content_length as usize > MAX_REQUEST_BUF {
                    let too_large = b"HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = std::io::Write::write_all(&mut conn.stream, too_large);
                    *request_count += 1;
                    return ConnAction::Close;
                }

                let body_needed = head_consumed + content_length as usize;
                let mut body_incomplete = false;
                while conn.total_read < body_needed && conn.total_read < MAX_REQUEST_BUF {
                    if conn.total_read == conn.buf.len() {
                        conn.buf
                            .resize(std::cmp::min(conn.buf.len() * 2, MAX_REQUEST_BUF), 0);
                    }
                    match std::io::Read::read(&mut conn.stream, &mut conn.buf[conn.total_read..]) {
                        Ok(0) => {
                            body_incomplete = true;
                            break;
                        }
                        Ok(n) => conn.total_read += n,
                        Err(ref e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            body_incomplete = true;
                            break;
                        }
                        Err(_) => {
                            body_incomplete = true;
                            break;
                        }
                    }
                }

                if content_length > 0 && (body_incomplete || conn.total_read < body_needed) {
                    let bad_request = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = std::io::Write::write_all(&mut conn.stream, bad_request);
                    *request_count += 1;
                    return ConnAction::Close;
                }

                Ok((
                    body_needed,
                    head_consumed,
                    content_length as usize,
                    content_length,
                    false,
                ))
            };

            let (wire_consumed, body_start, body_len, final_content_length, is_request_chunked) =
                match body_result {
                    Ok(tuple) => tuple,
                    Err(()) => return ConnAction::Close,
                };

            // Detach request-scoped raw from scratch buffer (owned copy).
            let raw_len = if is_request_chunked {
                head_consumed + body_len
            } else {
                wire_consumed
            };
            let raw_bytes = conn.buf[..raw_len].to_vec();

            let http_minor = match get_field_value(&parsed_fields, "version") {
                Some(Value::BuchiPack(ver_fields)) => {
                    get_field_int(ver_fields, "minor").unwrap_or(1)
                }
                _ => 1,
            };
            let keep_alive = match get_field_value(&parsed_fields, "headers") {
                Some(Value::List(headers)) => determine_keep_alive(&raw_bytes, headers, http_minor),
                _ => http_minor == 1,
            };

            // ── Build request pack for handler ──
            let mut request_fields: Vec<(String, Value)> = Vec::new();
            request_fields.push(("raw".into(), Value::bytes(raw_bytes)));

            for key in &["method", "path", "query", "version", "headers"] {
                if let Some(v) = get_field_value(&parsed_fields, key) {
                    request_fields.push((key.to_string(), v.clone()));
                }
            }

            request_fields.push(("body".into(), make_span(body_start, body_len)));
            request_fields.push(("bodyOffset".into(), Value::Int(head_consumed as i64)));
            request_fields.push(("contentLength".into(), Value::Int(final_content_length)));
            request_fields.push((
                "remoteHost".into(),
                Value::str(conn.peer_addr.ip().to_string()),
            ));
            request_fields.push((
                "remotePort".into(),
                Value::Int(conn.peer_addr.port() as i64),
            ));
            request_fields.push(("keepAlive".into(), Value::Bool(keep_alive)));
            request_fields.push(("chunked".into(), Value::Bool(is_request_chunked)));

            let request_pack = Value::pack(request_fields);

            let handler_result = self.call_function_with_values(handler, &[request_pack]);

            let response_value = match handler_result {
                Ok(v) => v,
                Err(e) => {
                    let error_body = format!("Internal Server Error: {}", e.message);
                    let error_response = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        error_body.len(),
                        error_body
                    );
                    let _ = std::io::Write::write_all(&mut conn.stream, error_response.as_bytes());
                    *request_count += 1;
                    return ConnAction::Close;
                }
            };

            // NB6-1: Scatter-gather send — head and body as separate buffers.
            if send_response_scatter(&mut conn.stream, &response_value).is_err() {
                let fallback = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = std::io::Write::write_all(&mut conn.stream, fallback);
                *request_count += 1;
                return ConnAction::Close;
            }

            *request_count += 1;
            conn.conn_requests += 1;

            // ── Buffer advance: remove consumed bytes, keep any leftover ──
            if wire_consumed < conn.total_read {
                conn.buf.copy_within(wire_consumed..conn.total_read, 0);
                conn.total_read -= wire_consumed;
            } else {
                conn.total_read = 0;
            }
            if conn.buf.len() < 8192 {
                conn.buf.resize(8192, 0);
            }

            // ── Keep-alive decision ──
            if !keep_alive {
                return ConnAction::Close;
            }

            ConnAction::KeepAlive
        }
    }
}
