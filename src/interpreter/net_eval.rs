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

/// (status_code, headers, body_bytes)
type ResponseFields = (i64, Vec<(String, String)>, Vec<u8>);

/// All symbols exported by the net package.
/// Legacy (16) + HTTP v1 (3) + HTTP v2 (1) + HTTP v3 (4) + HTTP v4 (6) + v5 (1) = 31 symbols.
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
    // HTTP v2
    "readBody",
    // HTTP v3 streaming
    "startResponse",
    "writeChunk",
    "endResponse",
    "sseEvent",
    // HTTP v4 request body streaming
    "readBodyChunk",
    "readBodyAll",
    // HTTP v4 WebSocket
    "wsUpgrade",
    "wsSend",
    "wsReceive",
    "wsClose",
    // v5 WebSocket revision
    "wsCloseCode",
];

// ── v3 Writer State Machine ───────────────────────────────────────
//
// State transitions (see NET_DESIGN.md):
//   Idle → HeadPrepared (via startResponse)
//   Idle → Streaming (via writeChunk/sseEvent, implicit 200/@[])
//   Idle → Ended (via endResponse, implicit 200/@[], empty chunked body)
//   Idle → One-shot fallback (2-arg handler returns response pack)
//   HeadPrepared → Streaming (via writeChunk/sseEvent)
//   HeadPrepared → Ended (via endResponse, empty chunked body)
//   Streaming → Streaming (via writeChunk/sseEvent)
//   Streaming → Ended (via endResponse)

/// Writer state for response streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterState {
    /// No streaming operation started yet.
    Idle,
    /// `startResponse` called; pending status/headers set but not committed to wire.
    HeadPrepared,
    /// Head committed to wire; body chunks can be written.
    Streaming,
    /// `endResponse` called or auto-ended; no more writes allowed.
    Ended,
    /// v4: WebSocket upgrade completed. HTTP streaming API is disabled.
    /// Only wsSend/wsReceive/wsClose are valid in this state.
    WebSocket,
}

/// Streaming writer state for a 2-arg handler.
/// The handler receives a Value::BuchiPack with a `__writer_id` sentinel field,
/// but the actual mutable state lives here on the dispatch_request stack.
/// During handler execution, the Interpreter's `active_streaming_writer` field
/// holds raw pointers to this struct and the connection's TcpStream.
pub(crate) struct StreamingWriter {
    pub state: WriterState,
    pub pending_status: u16,
    /// Response headers staged for head commit.
    /// Intentionally retained after commit so later logic can validate which
    /// headers were already put on the wire (for example, SSE checks after an
    /// earlier writeChunk committed the head).
    pub pending_headers: Vec<(String, String)>,
    /// Whether SSE auto-headers have been applied.
    pub sse_mode: bool,
}

impl StreamingWriter {
    fn new() -> Self {
        StreamingWriter {
            state: WriterState::Idle,
            pending_status: 200,
            pending_headers: Vec::new(),
            sse_mode: false,
        }
    }

    /// Check if a status code forbids a message body (1xx, 204, 205, 304).
    fn is_bodyless_status(status: u16) -> bool {
        matches!(status, 100..=199 | 204 | 205 | 304)
    }

    /// Validate that user-supplied headers do not contain reserved headers
    /// for the streaming path (Content-Length, Transfer-Encoding).
    fn validate_reserved_headers(headers: &[(String, String)]) -> Result<(), String> {
        for (name, _) in headers {
            let lower = name.to_ascii_lowercase();
            if lower == "content-length" {
                return Err(
                    "startResponse: 'Content-Length' is not allowed in streaming response headers. \
                     The runtime manages Content-Length/Transfer-Encoding for streaming responses."
                        .to_string(),
                );
            }
            if lower == "transfer-encoding" {
                return Err(
                    "startResponse: 'Transfer-Encoding' is not allowed in streaming response headers. \
                     The runtime manages Transfer-Encoding for streaming responses."
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}

/// Active streaming writer context, stored on the Interpreter during 2-arg handler execution.
/// Holds raw pointers to stack-local variables in `dispatch_request`.
///
/// Safety invariants:
/// - The Interpreter is single-threaded (!Send).
/// - The pointers are set before `call_function_with_values` and cleared after it returns.
/// - The pointees (StreamingWriter, TcpStream) live on the stack in `dispatch_request`
///   and outlive the handler call.
/// - NB3-9: `borrowed` flag prevents re-entrant access to the raw pointers,
///   eliminating the theoretical UB from nested streaming API calls (e.g.
///   `writeChunk(writer, writeChunk(writer, "data"))`).
pub(crate) struct ActiveStreamingWriter {
    pub writer: *mut StreamingWriter,
    pub stream: *mut ConnStream,
    /// NB3-9: Re-entrancy guard. Set to true while a streaming API function
    /// holds `&mut *self.writer` / `&mut *self.stream`. If another streaming
    /// API call is attempted while this is true, it returns a RuntimeError
    /// instead of creating a second `&mut` to the same pointee.
    pub borrowed: bool,
    /// v4: Raw pointer to the body streaming state for this request.
    /// Only set for 2-arg handlers (body-deferred mode).
    /// Null when 1-arg handler (body already read eagerly).
    pub body_state: *mut RequestBodyState,
    /// v4: WebSocket close state. True after wsClose has been sent.
    pub ws_closed: bool,
    /// NB4-10: Connection-scoped WebSocket token for identity verification.
    /// Set when wsUpgrade succeeds; verified by wsSend/wsReceive/wsClose.
    pub ws_token: u64,
    /// v5: Received close code from peer's close frame.
    /// 0 = no close frame received yet.
    /// Set when a valid close frame is received in wsReceive.
    pub ws_close_code: i64,
}

// ── v4 Request Body Streaming State ──────────────────────────

/// Per-request body streaming state for 2-arg handlers.
/// Lives on the `dispatch_request` stack frame. The `ActiveStreamingWriter`
/// holds a raw pointer to this struct during handler execution.
///
/// Tracks how much of the request body has been consumed so that
/// `readBodyChunk` can read incrementally from the TcpStream without
/// buffering the full body.
pub(crate) struct RequestBodyState {
    /// Whether the request uses chunked transfer encoding.
    pub is_chunked: bool,
    /// Content-Length value from the request head (0 if absent or chunked).
    pub content_length: i64,
    /// How many body bytes have been consumed so far (Content-Length path).
    pub bytes_consumed: i64,
    /// Whether the body has been fully read (terminal chunk seen or all CL bytes read).
    pub fully_read: bool,
    /// Whether any readBodyChunk / readBodyAll call has been made.
    pub any_read_started: bool,
    /// Leftover bytes from head parsing that belong to the body start.
    /// For 2-arg handler, after head parse, there may be unread bytes in
    /// conn.buf that are body bytes already received. We copy these out
    /// so that the first readBodyChunk can return them without re-reading.
    pub leftover: Vec<u8>,
    /// Current position within the leftover buffer.
    pub leftover_pos: usize,
    /// Chunked decoder state: pending partial chunk header bytes.
    pub chunked_state: ChunkedDecoderState,
    /// NB4-7: Request-scoped token to verify that readBody*/readBodyChunk/readBodyAll
    /// are called with the correct request pack (not a fake or stale pack).
    pub request_token: u64,
}

/// Global monotonic counter for generating unique request tokens (NB4-7).
static NEXT_REQUEST_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// NB4-10: Global monotonic counter for generating unique WebSocket connection tokens.
static NEXT_WS_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

// ── v4 WebSocket frame types ───────────────────────────────

/// Parsed WebSocket frame result.
enum WsFrame {
    /// Text (opcode 0x1) or Binary (opcode 0x2) data frame.
    Data { opcode: u8, payload: Vec<u8> },
    /// Ping frame (opcode 0x9) with payload for pong echo.
    Ping { payload: Vec<u8> },
    /// Pong frame (opcode 0xA) — unsolicited, ignored.
    Pong,
    /// Close frame (opcode 0x8).
    /// v5: carries the raw close payload for close code extraction.
    Close { payload: Vec<u8> },
    /// Protocol error (fragmented frame, unknown opcode, etc.).
    ProtocolError(String),
}

/// Chunked transfer-encoding decoder state.
/// Maintains state between readBodyChunk calls for incremental decoding.
#[derive(Debug)]
pub(crate) enum ChunkedDecoderState {
    /// Waiting for chunk-size line (hex digits + CRLF).
    WaitingChunkSize,
    /// In the middle of reading chunk data.
    /// `remaining` is how many bytes left in the current chunk.
    ReadingChunkData { remaining: usize },
    /// Waiting for CRLF after chunk data.
    WaitingChunkTrailer,
    /// Terminal chunk (size=0) has been seen. Body is done.
    Done,
}

impl RequestBodyState {
    fn new(is_chunked: bool, content_length: i64, leftover: Vec<u8>) -> Self {
        let fully_read = !is_chunked && content_length == 0;
        let token = NEXT_REQUEST_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        RequestBodyState {
            is_chunked,
            content_length,
            bytes_consumed: 0,
            fully_read,
            any_read_started: false,
            leftover,
            leftover_pos: 0,
            chunked_state: ChunkedDecoderState::WaitingChunkSize,
            request_token: token,
        }
    }

    /// Check if there are leftover bytes available.
    fn has_leftover(&self) -> bool {
        self.leftover_pos < self.leftover.len()
    }

    /// Take remaining leftover bytes (consuming them).
    #[allow(dead_code)]
    fn take_leftover(&mut self) -> Vec<u8> {
        if self.leftover_pos >= self.leftover.len() {
            return Vec::new();
        }
        let data = self.leftover[self.leftover_pos..].to_vec();
        self.leftover_pos = self.leftover.len();
        data
    }
}

/// Build the HTTP response head bytes for a streaming response.
///
/// For normal status codes: appends `Transfer-Encoding: chunked`.
/// For bodyless status codes (1xx/204/205/304): omits `Transfer-Encoding`
/// since no message body is allowed.
///
/// This is the head commit function. Once called, status/headers are on the wire
/// and cannot be changed.
fn build_streaming_head(status: u16, headers: &[(String, String)]) -> Vec<u8> {
    use std::io::Write as _;
    let reason = http_reason_phrase(status);
    let mut buf = Vec::with_capacity(256);
    // NB6-5: write!() directly into Vec<u8> to avoid intermediate String heap allocs.
    let _ = write!(buf, "HTTP/1.1 {} {}\r\n", status, reason);
    for (name, value) in headers {
        let _ = write!(buf, "{}: {}\r\n", name, value);
    }
    // NET3-1d: Auto-append Transfer-Encoding: chunked — but only for status codes
    // that allow a message body. Bodyless statuses (1xx/204/205/304) must NOT have
    // Transfer-Encoding (RFC 9110 §6.4.1).
    if !StreamingWriter::is_bodyless_status(status) {
        buf.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    }
    buf.extend_from_slice(b"\r\n");
    buf
}

/// Map HTTP status code to reason phrase.
fn http_reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        205 => "Reset Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

// ── v3 streaming write helpers ──────────────────────────────
//
// These avoid creating aggregate buffers. `write_vectored_all` uses IoSlice
// to send multiple disjoint buffers in a single syscall where supported.

/// Write all bytes to a ConnStream (plaintext or TLS), retrying on partial writes.
fn write_all_retry(stream: &mut ConnStream, data: &[u8]) -> Result<(), RuntimeError> {
    use std::io::Write;
    stream.write_all(data).map_err(|e| RuntimeError {
        message: format!("streaming write error: {}", e),
    })
}

/// Write multiple IoSlice buffers to a stream.
///
/// NB5-18: Plaintext path uses `write_vectored()` (writev syscall) to send
/// multiple buffers in a single syscall, avoiding Nagle-induced small packet
/// splitting. TLS path concatenates all IoSlices into one buffer before passing
/// to rustls writer — rustls `Writer` only implements `std::io::Write` (not
/// `write_vectored`), so a single `write_all` call produces one TLS record
/// instead of N records for N buffers (the previous per-buffer approach caused
/// 3 TLS records per chunked write: hex_prefix + payload + suffix).
fn write_vectored_all(
    stream: &mut ConnStream,
    bufs: &[std::io::IoSlice<'_>],
) -> Result<(), RuntimeError> {
    use std::io::Write;
    match stream {
        ConnStream::Plain(tcp) => {
            // Use writev to send all buffers in as few syscalls as possible.
            // write_vectored may not write all bytes in one call, so we track
            // which buffers (and partial offset within the current one) remain.
            let mut buf_idx = 0usize;
            let mut offset_in_buf = 0usize;
            while buf_idx < bufs.len() {
                if offset_in_buf > 0 {
                    // Partial write left us mid-buffer — finish it with write_all.
                    tcp.write_all(&bufs[buf_idx][offset_in_buf..])
                        .map_err(|e| RuntimeError {
                            message: format!("streaming write error: {}", e),
                        })?;
                    buf_idx += 1;
                    offset_in_buf = 0;
                    continue;
                }
                // Build IoSlice array for remaining buffers.
                let remaining: Vec<std::io::IoSlice<'_>> = bufs[buf_idx..]
                    .iter()
                    .map(|b| std::io::IoSlice::new(b))
                    .collect();
                match tcp.write_vectored(&remaining) {
                    Ok(0) => {
                        return Err(RuntimeError {
                            message: "streaming write error: write returned 0".into(),
                        });
                    }
                    Ok(mut n) => {
                        // Advance past fully written buffers.
                        for buf in &remaining {
                            if n >= buf.len() {
                                n -= buf.len();
                                buf_idx += 1;
                            } else {
                                // Partial write within this buffer.
                                offset_in_buf = n;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        return Err(RuntimeError {
                            message: format!("streaming write error: {}", e),
                        });
                    }
                }
            }
            Ok(())
        }
        ConnStream::Tls(_) => {
            // TLS: concatenate all IoSlices into one buffer, then write once.
            // This produces a single TLS record instead of N records.
            let total_len: usize = bufs.iter().map(|b| b.len()).sum();
            let mut combined = Vec::with_capacity(total_len);
            for buf in bufs {
                combined.extend_from_slice(buf);
            }
            stream.write_all(&combined).map_err(|e| RuntimeError {
                message: format!("streaming write error: {}", e),
            })
        }
    }
}

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

/// Extract the __value from a Result BuchiPack by consuming it, returning None on failure.
/// This avoids cloning the parsed fields when ownership can be transferred.
fn extract_result_value_owned(result: Value) -> Option<Vec<(String, Value)>> {
    let fields = match result {
        Value::BuchiPack(f) => f,
        _ => return None,
    };
    // Check that throw is Unit (success)
    match fields.iter().find(|(k, _)| k == "throw") {
        Some((_, Value::Unit)) => {}
        _ => return None,
    }
    // Find and move __value out
    for (k, v) in fields {
        if k == "__value"
            && let Value::BuchiPack(inner) = v
        {
            return Some(inner);
        }
    }
    None
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

/// Get a Str field from a BuchiPack field list.
fn get_field_str(fields: &[(String, Value)], key: &str) -> Option<String> {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Str(s))) => Some(s.clone()),
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
        Ok(httparse::Status::Complete(consumed)) => build_parse_result(&req, bytes, consumed, true),
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
        ("minor".into(), Value::Int(req.version.unwrap_or(1) as i64)),
    ]);

    // headers as list of @(name: span, value: span)
    // On Partial parse, req.headers contains EMPTY_HEADER entries beyond parsed ones.
    // Stop at the first empty header name to avoid pointer arithmetic on unrelated memory.
    let mut content_length: i64 = 0;
    let mut cl_count: usize = 0;
    let mut has_transfer_encoding_chunked = false;
    let mut has_content_length = false;
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
        // NET2-2a: Detect Transfer-Encoding: chunked
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            // Scan comma-separated tokens for "chunked"
            for token in header.value.split(|&b| b == b',') {
                let trimmed = trim_ascii(token);
                if trimmed.eq_ignore_ascii_case(b"chunked") {
                    has_transfer_encoding_chunked = true;
                }
            }
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
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
                Ok(len) => {
                    // Cap at Number.MAX_SAFE_INTEGER (2^53 - 1 = 9007199254740991) for
                    // cross-backend parity. JS Number loses precision beyond this value,
                    // so both backends must reject to keep contentLength identical.
                    if len > 9_007_199_254_740_991 {
                        return make_result_failure_msg(
                            "ParseError",
                            "Malformed HTTP request: invalid Content-Length value",
                        );
                    }
                    content_length = len;
                }
                Err(_) => {
                    return make_result_failure_msg(
                        "ParseError",
                        "Malformed HTTP request: invalid Content-Length value",
                    );
                }
            }
        }
    }

    // NET2-2e: Reject Content-Length + Transfer-Encoding: chunked (RFC 7230 section 3.3.3)
    if has_transfer_encoding_chunked && has_content_length {
        return make_result_failure_msg(
            "ParseError",
            "Malformed HTTP request: Content-Length and Transfer-Encoding: chunked are mutually exclusive",
        );
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
        ("chunked".into(), Value::Bool(has_transfer_encoding_chunked)),
    ]);

    make_result_success(parsed)
}

/// Trim leading/trailing ASCII whitespace from a byte slice.
fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |p| p + 1);
    &bytes[start..end]
}

// ── Chunked Transfer Encoding: in-place compaction (NET2-2b/2f/2g) ──

/// Result of chunked in-place compaction on a buffer.
#[derive(Debug)]
struct ChunkedCompactResult {
    /// Total compacted body length (bytes written to body region).
    body_len: usize,
    /// Total wire bytes consumed from `body_offset` (including framing).
    /// Used by keep-alive `advance()` to skip the right amount.
    wire_consumed: usize,
}

/// Perform in-place compaction of chunked transfer-encoded body data.
///
/// The buffer `buf[body_offset..]` contains raw chunked data:
///   chunk-size (hex) CRLF chunk-data CRLF ... 0 CRLF CRLF
///
/// After compaction, `buf[body_offset..body_offset + body_len]` contains
/// the reassembled body with all framing removed.
///
/// Uses `copy_within` (memmove-equivalent) for overlapping regions.
/// Never uses memcpy (which is undefined for overlapping regions).
///
/// Returns `Err(message)` on malformed chunks.
fn chunked_in_place_compact(
    buf: &mut [u8],
    body_offset: usize,
) -> Result<ChunkedCompactResult, String> {
    let data = &buf[body_offset..];
    let data_len = data.len();

    let mut read_pos: usize = 0;
    let mut write_pos: usize = 0;

    loop {
        // Find the end of the chunk-size line (CRLF)
        let size_line_end = match find_crlf(&buf[body_offset + read_pos..]) {
            Some(pos) => pos,
            None => {
                return Err("Malformed chunked body: missing CRLF after chunk-size".into());
            }
        };

        // Parse chunk-size (hex), ignoring chunk-ext after semicolon
        let size_line = &buf[body_offset + read_pos..body_offset + read_pos + size_line_end];
        let hex_part = match size_line.iter().position(|&b| b == b';') {
            Some(semi) => &size_line[..semi],
            None => size_line,
        };
        let hex_str = std::str::from_utf8(trim_ascii(hex_part))
            .map_err(|_| "Malformed chunked body: invalid chunk-size encoding".to_string())?;

        if hex_str.is_empty() {
            return Err("Malformed chunked body: empty chunk-size".into());
        }

        let chunk_size = usize::from_str_radix(hex_str, 16)
            .map_err(|_| format!("Malformed chunked body: invalid chunk-size '{}'", hex_str))?;

        // Advance read_pos past "chunk-size\r\n"
        read_pos += size_line_end + 2; // +2 for CRLF

        // NET2-2f: 0-length terminator chunk
        if chunk_size == 0 {
            // Skip optional trailer headers until final CRLF
            // Trailer format: (header-field CRLF)* CRLF
            loop {
                if body_offset + read_pos + 2 > buf.len() {
                    return Err("Malformed chunked body: missing final CRLF after 0 chunk".into());
                }
                // Check if the next two bytes are CRLF (end of trailers)
                if buf[body_offset + read_pos] == b'\r' && buf[body_offset + read_pos + 1] == b'\n'
                {
                    read_pos += 2;
                    break;
                }
                // Skip trailer line
                match find_crlf(&buf[body_offset + read_pos..]) {
                    Some(pos) => read_pos += pos + 2,
                    None => {
                        return Err("Malformed chunked body: incomplete trailer".into());
                    }
                }
            }

            return Ok(ChunkedCompactResult {
                body_len: write_pos,
                wire_consumed: read_pos,
            });
        }

        // Validate: enough data for chunk-data + CRLF
        if read_pos + chunk_size + 2 > data_len {
            return Err("Malformed chunked body: truncated chunk data".into());
        }

        // In-place compaction: copy chunk data to write position.
        // Use copy_within (memmove) because regions may overlap.
        if write_pos != read_pos {
            buf.copy_within(
                body_offset + read_pos..body_offset + read_pos + chunk_size,
                body_offset + write_pos,
            );
        }
        write_pos += chunk_size;
        read_pos += chunk_size;

        // Validate trailing CRLF after chunk data
        if buf[body_offset + read_pos] != b'\r' || buf[body_offset + read_pos + 1] != b'\n' {
            return Err("Malformed chunked body: missing CRLF after chunk data".into());
        }
        read_pos += 2; // skip CRLF
    }
}

/// Find the position of the first CRLF in a byte slice.
/// Returns the offset of '\r' (so the CRLF is at `pos` and `pos+1`).
fn find_crlf(data: &[u8]) -> Option<usize> {
    if data.len() < 2 {
        return None;
    }
    (0..data.len() - 1).find(|&i| data[i] == b'\r' && data[i + 1] == b'\n')
}

/// Check if the buffer contains a complete chunked body (read-only scan).
/// NB2-15: Typed error for chunked body parsing — avoids string prefix matching.
#[derive(Debug)]
#[allow(dead_code)]
enum ChunkedBodyError {
    /// Need more data (incomplete chunk framing)
    Incomplete(String),
    /// Malformed chunk data (reject immediately)
    Malformed(String),
}

///
/// Walks the chunk framing without modifying the buffer.
/// Returns `Ok(wire_consumed)` if the terminator chunk was found,
/// or `Err(ChunkedBodyError)` if the data is incomplete or malformed.
fn chunked_body_complete(buf: &[u8], body_offset: usize) -> Result<usize, ChunkedBodyError> {
    let data_len = buf.len() - body_offset;
    let mut read_pos: usize = 0;

    loop {
        // Need at least 1 byte to start scanning for chunk-size
        if read_pos >= data_len {
            return Err(ChunkedBodyError::Incomplete(
                "no data for next chunk-size".into(),
            ));
        }

        // Find the end of the chunk-size line (CRLF)
        let size_line_end = match find_crlf(&buf[body_offset + read_pos..]) {
            Some(pos) => pos,
            None => {
                return Err(ChunkedBodyError::Incomplete(
                    "missing CRLF after chunk-size".into(),
                ));
            }
        };

        // Parse chunk-size (hex), ignoring chunk-ext after semicolon
        let size_line = &buf[body_offset + read_pos..body_offset + read_pos + size_line_end];
        let hex_part = match size_line.iter().position(|&b| b == b';') {
            Some(semi) => &size_line[..semi],
            None => size_line,
        };
        let hex_str = std::str::from_utf8(trim_ascii(hex_part))
            .map_err(|_| ChunkedBodyError::Malformed("invalid chunk-size encoding".to_string()))?;

        if hex_str.is_empty() {
            return Err(ChunkedBodyError::Malformed("empty chunk-size".into()));
        }

        let chunk_size = usize::from_str_radix(hex_str, 16).map_err(|_| {
            ChunkedBodyError::Malformed(format!("invalid chunk-size '{}'", hex_str))
        })?;

        // Advance past "chunk-size\r\n"
        read_pos += size_line_end + 2;

        // Terminator chunk
        if chunk_size == 0 {
            // Skip optional trailer headers until final CRLF
            loop {
                if read_pos + 2 > data_len {
                    return Err(ChunkedBodyError::Incomplete(
                        "missing final CRLF after 0 chunk".into(),
                    ));
                }
                if buf[body_offset + read_pos] == b'\r' && buf[body_offset + read_pos + 1] == b'\n'
                {
                    read_pos += 2;
                    return Ok(read_pos);
                }
                match find_crlf(&buf[body_offset + read_pos..]) {
                    Some(pos) => read_pos += pos + 2,
                    None => {
                        return Err(ChunkedBodyError::Incomplete("incomplete trailer".into()));
                    }
                }
            }
        }

        // Check we have chunk-data + CRLF
        if read_pos + chunk_size + 2 > data_len {
            return Err(ChunkedBodyError::Incomplete("chunk data incomplete".into()));
        }

        // Skip chunk-data + CRLF
        read_pos += chunk_size;

        // Validate CRLF after data
        if buf[body_offset + read_pos] != b'\r' || buf[body_offset + read_pos + 1] != b'\n' {
            return Err(ChunkedBodyError::Malformed(
                "missing CRLF after chunk data".into(),
            ));
        }
        read_pos += 2;
    }
}

// ── Keep-Alive determination (NET2-1a/1b/1c) ───────────────

/// Determine whether the connection should be kept alive based on
/// HTTP version and the Connection header.
///
/// Rules (RFC 7230 §6.1):
/// - HTTP/1.1: keep-alive by default, `Connection: close` disables it
/// - HTTP/1.0: close by default, `Connection: keep-alive` enables it
///
/// `raw` is the request wire bytes. `headers` is the parsed header span
/// list from `parse_request_head`. `http_minor` is the minor version (0 or 1).
fn determine_keep_alive(raw: &[u8], headers: &[Value], http_minor: i64) -> bool {
    // Collect all Connection header values (RFC 7230 §6.1: token list,
    // multiple headers are merged as comma-separated).
    let mut has_close = false;
    let mut has_keep_alive = false;
    for header in headers {
        if let Value::BuchiPack(fields) = header {
            let name_start = get_field_int(fields, "start")
                .or_else(|| {
                    if let Some(Value::BuchiPack(name_span)) =
                        fields.iter().find(|(k, _)| k == "name").map(|(_, v)| v)
                    {
                        get_field_int(name_span, "start")
                    } else {
                        None
                    }
                })
                .unwrap_or(0) as usize;
            let name_len = get_field_int(fields, "len")
                .or_else(|| {
                    if let Some(Value::BuchiPack(name_span)) =
                        fields.iter().find(|(k, _)| k == "name").map(|(_, v)| v)
                    {
                        get_field_int(name_span, "len")
                    } else {
                        None
                    }
                })
                .unwrap_or(0) as usize;

            if name_start + name_len > raw.len() {
                continue;
            }
            let name_bytes = &raw[name_start..name_start + name_len];
            if name_bytes.eq_ignore_ascii_case(b"connection") {
                // Extract value span and scan comma-separated tokens
                if let Some(Value::BuchiPack(value_span)) =
                    fields.iter().find(|(k, _)| k == "value").map(|(_, v)| v)
                {
                    let val_start = get_field_int(value_span, "start").unwrap_or(0) as usize;
                    let val_len = get_field_int(value_span, "len").unwrap_or(0) as usize;
                    if val_start + val_len <= raw.len() {
                        let val_bytes = &raw[val_start..val_start + val_len];
                        for token in val_bytes.split(|&b| b == b',') {
                            let trimmed = trim_ascii(token);
                            if trimmed.eq_ignore_ascii_case(b"close") {
                                has_close = true;
                            } else if trimmed.eq_ignore_ascii_case(b"keep-alive") {
                                has_keep_alive = true;
                            }
                        }
                    }
                }
                // Don't break — merge multiple Connection headers
            }
        }
    }

    // RFC 7230 §6.1: `close` always wins over `keep-alive`
    if has_close {
        return false;
    }
    match http_minor {
        // HTTP/1.1: keep-alive by default
        1 => true,
        // HTTP/1.0: close by default, `keep-alive` enables
        _ => has_keep_alive,
    }
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
            format!("httpEncodeResponse: status {} must not have a body", status),
        );
    }

    use std::io::Write as _;
    let reason = status_reason(status);
    let mut buf = Vec::with_capacity(256 + body_bytes.len());

    // NB6-5: write!() directly into Vec<u8> to eliminate per-header intermediate String allocs.
    // Status line
    let _ = write!(buf, "HTTP/1.1 {} {}\r\n", status, reason);

    // User headers (skip Content-Length for no-body statuses)
    for (name, value) in &headers {
        if no_body && name.eq_ignore_ascii_case("Content-Length") {
            continue;
        }
        let _ = write!(buf, "{}: {}\r\n", name, value);
    }

    // Auto-append Content-Length for statuses that allow a body
    if !no_body {
        let has_content_length = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("Content-Length"));
        if !has_content_length {
            let _ = write!(buf, "Content-Length: {}\r\n", body_bytes.len());
        }
    }

    buf.extend_from_slice(b"\r\n");
    if !no_body {
        buf.extend_from_slice(&body_bytes);
    }

    let result = Value::BuchiPack(vec![("bytes".into(), Value::Bytes(buf))]);
    make_result_success(result)
}

/// NB6-1: Scatter-gather send for internal one-shot response path.
/// Builds head and body as separate buffers and sends them via vectored I/O,
/// avoiding the aggregate buffer concatenation of encode_response().
fn send_response_scatter(stream: &mut ConnStream, response: &Value) -> Result<(), String> {
    use std::io::Write as _;

    let (status, headers, body_bytes) = extract_response_fields(response)?;

    let no_body = (100..200).contains(&status) || status == 204 || status == 205 || status == 304;
    if no_body && !body_bytes.is_empty() {
        return Err(format!(
            "httpEncodeResponse: status {} must not have a body",
            status
        ));
    }

    let reason = status_reason(status);
    let mut head = Vec::with_capacity(256);
    let _ = write!(head, "HTTP/1.1 {} {}\r\n", status, reason);

    for (name, value) in &headers {
        if no_body && name.eq_ignore_ascii_case("Content-Length") {
            continue;
        }
        let _ = write!(head, "{}: {}\r\n", name, value);
    }

    if !no_body {
        let has_content_length = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("Content-Length"));
        if !has_content_length {
            let _ = write!(head, "Content-Length: {}\r\n", body_bytes.len());
        }
    }

    head.extend_from_slice(b"\r\n");

    // NB6-1: Send head and body as separate IoSlices — no aggregate buffer.
    if no_body || body_bytes.is_empty() {
        stream
            .write_all(&head)
            .map_err(|e| format!("response write error: {}", e))?;
    } else {
        let bufs = [
            std::io::IoSlice::new(&head),
            std::io::IoSlice::new(&body_bytes),
        ];
        write_vectored_all(stream, &bufs)
            .map_err(|e| format!("response write error: {}", e.message))?;
    }
    Ok(())
}

fn extract_response_fields(response: &Value) -> Result<ResponseFields, String> {
    let fields = match response {
        Value::BuchiPack(fields) => fields,
        _ => return Err("httpEncodeResponse: argument must be a BuchiPack @(...)".into()),
    };

    // status (required, must be Int)
    let status = match fields.iter().find(|(k, _)| k == "status") {
        Some((_, Value::Int(n))) => *n,
        Some((_, v)) => return Err(format!("httpEncodeResponse: status must be Int, got {}", v)),
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
            ));
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
                ));
            }
        };
        let name = match hf.iter().find(|(k, _)| k == "name") {
            Some((_, Value::Str(s))) => s.clone(),
            _ => {
                return Err(format!(
                    "httpEncodeResponse: headers[{}].name must be Str",
                    i
                ));
            }
        };
        let value = match hf.iter().find(|(k, _)| k == "value") {
            Some((_, Value::Str(s))) => s.clone(),
            _ => {
                return Err(format!(
                    "httpEncodeResponse: headers[{}].value must be Str",
                    i
                ));
            }
        };
        // NB-7: Enforce header name/value length limits (parity with Native)
        if name.len() > 8192 {
            return Err(format!(
                "httpEncodeResponse: headers[{}].name exceeds 8192 bytes",
                i
            ));
        }
        if value.len() > 65536 {
            return Err(format!(
                "httpEncodeResponse: headers[{}].value exceeds 65536 bytes",
                i
            ));
        }
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
    // NB5-22: `b.clone()` is necessary because `fields` is a shared reference to the
    // handler's returned BuchiPack — `Value` does not support destructive move from a
    // borrowed slice. This is the 1-arg eager path where the full body is already in
    // memory; the 2-arg streaming path avoids this clone by writing chunks directly.
    // A future `Value::into_bytes()` consuming method could eliminate this clone, but
    // would require changes to the Value type across the codebase.
    let body_bytes = match fields.iter().find(|(k, _)| k == "body") {
        Some((_, Value::Bytes(b))) => b.clone(),
        Some((_, Value::Str(s))) => s.as_bytes().to_vec(),
        Some((_, v)) => {
            return Err(format!(
                "httpEncodeResponse: body must be Bytes or Str, got {}",
                v
            ));
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

// ── v4 request body streaming helpers ────────────────────────

/// Check if a request Value has the `__body_stream` sentinel,
/// indicating it was created by a 2-arg handler with body-deferred semantics.
fn is_body_stream_request(req: &Value) -> bool {
    if let Value::BuchiPack(fields) = req {
        fields.iter().any(|(k, v)| {
            k == "__body_stream" && matches!(v, Value::Str(s) if s == "__v4_body_stream")
        })
    } else {
        false
    }
}

/// Extract the request body token from a body-stream request pack (NB4-7).
/// Returns None if the request is not a body-stream request or has no token.
fn extract_body_token(req: &Value) -> Option<u64> {
    if let Value::BuchiPack(fields) = req {
        for (k, v) in fields {
            if k == "__body_token"
                && let Value::Int(n) = v
            {
                return Some(*n as u64);
            }
        }
    }
    None
}

// ── readBody ─────────────────────────────────────────────────

/// `readBody(req)` — extract body bytes from a request pack.
///
/// Returns `Bytes` (owned copy of `req.raw[body.start .. body.start + body.len]`).
/// If body.len == 0 or body span is absent, returns empty Bytes.
fn eval_read_body(req: &Value) -> Result<Value, RuntimeError> {
    let fields = match req {
        Value::BuchiPack(f) => f,
        _ => {
            return Err(RuntimeError {
                message: format!(
                    "readBody: argument must be a request pack @(...), got {}",
                    req
                ),
            });
        }
    };

    // Extract raw: Bytes
    let raw = match get_field_value(fields, "raw") {
        Some(Value::Bytes(b)) => b,
        _ => {
            return Err(RuntimeError {
                message: "readBody: request pack missing 'raw: Bytes' field".into(),
            });
        }
    };

    // Extract body: @(start: Int, len: Int)
    let (body_start, body_len) = match get_field_value(fields, "body") {
        Some(Value::BuchiPack(span)) => {
            let start = get_field_int(span, "start").unwrap_or(0) as usize;
            let len = get_field_int(span, "len").unwrap_or(0) as usize;
            (start, len)
        }
        _ => (0, 0),
    };

    // Return body slice as Bytes
    if body_len == 0 {
        Ok(Value::Bytes(vec![]))
    } else {
        let end = body_start.saturating_add(body_len).min(raw.len());
        let start = body_start.min(end);
        Ok(Value::Bytes(raw[start..end].to_vec()))
    }
}

// ── NET2-3: Concurrent connection pool types ────────────────

// ── ConnStream: polymorphic stream for TLS / plaintext ──────────────

/// Polymorphic stream that wraps either a plain TcpStream (v4 compat)
/// or a TlsTransport (v5 HTTPS). Implements `std::io::Read` and `std::io::Write`
/// so existing streaming helpers work unchanged.
pub(crate) enum ConnStream {
    Plain(std::net::TcpStream),
    Tls(Box<super::net_transport::TlsTransport>),
}

impl std::io::Read for ConnStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            ConnStream::Plain(s) => std::io::Read::read(s, buf),
            ConnStream::Tls(t) => super::net_transport::Transport::read(t.as_mut(), buf),
        }
    }
}

impl std::io::Write for ConnStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            ConnStream::Plain(s) => std::io::Write::write(s, buf),
            ConnStream::Tls(t) => {
                super::net_transport::Transport::write_all(t.as_mut(), buf)?;
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => std::io::Write::flush(s),
            ConnStream::Tls(t) => super::net_transport::Transport::flush(t.as_mut()),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => std::io::Write::write_all(s, buf),
            ConnStream::Tls(t) => super::net_transport::Transport::write_all(t.as_mut(), buf),
        }
    }
}

impl ConnStream {
    fn set_read_timeout(&self, dur: Option<std::time::Duration>) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => s.set_read_timeout(dur),
            ConnStream::Tls(t) => t.stream_ref().set_read_timeout(dur),
        }
    }
}

/// Per-connection state for the concurrent httpServe pool.
/// Each connection owns its own scratch buffer (no sharing).
struct HttpConnection {
    stream: ConnStream,
    peer_addr: std::net::SocketAddr,
    /// Per-connection scratch buffer (allocated once, reused via advance)
    buf: Vec<u8>,
    /// How many bytes are valid in buf
    total_read: usize,
    /// How many requests have been processed on this connection
    conn_requests: i64,
    /// Last activity timestamp (for idle timeout detection)
    last_activity: std::time::Instant,
}

/// Result of a non-blocking read attempt on a connection.
enum ConnReadResult {
    /// Complete request head parsed: (fields, head_consumed, content_length, is_chunked)
    Ready(Vec<(String, Value)>, usize, i64, bool),
    /// Need more data (no complete head yet, not an error)
    NeedMore,
    /// Client closed the connection (EOF)
    Eof,
    /// Read timed out (short poll timeout, not necessarily idle timeout)
    Timeout,
    /// Malformed request head
    Malformed,
}

/// Action to take after dispatching a request on a connection.
enum ConnAction {
    /// Keep connection alive for more requests
    KeepAlive,
    /// Close the connection
    Close,
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
            "dnsResolve" | "tcpConnect" | "tcpListen" | "tcpAccept" | "socketSend"
            | "socketSendAll" | "socketRecv" | "socketSendBytes" | "socketRecvBytes"
            | "socketRecvExact" | "udpBind" | "udpSendTo" | "udpRecvFrom" | "socketClose"
            | "listenerClose" | "udpClose" => self.try_os_func(&original_name, args),

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
                        });
                    }
                };
                Ok(Some(Signal::Value(encode_response(&response))))
            }

            // ── httpServe(port, handler, maxRequests, timeoutMs) ──
            // → Async[Result[@(ok: Bool, requests: Int), _]]
            "httpServe" => self.eval_http_serve(args),

            // ── readBody(req) → Bytes ──
            // v4: In a 2-arg handler, readBody acts as readBodyAll alias.
            "readBody" => {
                let req = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "readBody: missing argument 'req'".into(),
                        });
                    }
                };
                // v4: If the request has __body_stream sentinel (2-arg handler),
                // delegate to readBodyAll to stream from socket.
                if is_body_stream_request(&req) {
                    // NB4-7: Verify token before delegating.
                    if let Some(ref active) = self.active_streaming_writer
                        && !active.body_state.is_null()
                    {
                        let body = unsafe { &*active.body_state };
                        let pack_token = extract_body_token(&req);
                        if pack_token != Some(body.request_token) {
                            return Err(RuntimeError {
                                    message: "readBody: request pack does not match the current active request. \
                                             The request may be stale or fabricated.".into(),
                                });
                        }
                    }
                    return self.eval_read_body_all_impl("readBody");
                }
                Ok(Some(Signal::Value(eval_read_body(&req)?)))
            }

            // ── v4 request body streaming API ──
            // readBodyChunk(req) → Lax[Bytes]
            // readBodyAll(req) → Bytes
            // Protected by the same re-entrancy guard as v3 streaming API.
            "readBodyChunk" | "readBodyAll" => {
                // Evaluate the req argument first (before re-entrancy guard).
                let req = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: format!("{}: missing argument 'req'", original_name),
                        });
                    }
                };

                // NET4-1f: 1-arg handler request packs do NOT have __body_stream sentinel.
                if !is_body_stream_request(&req) {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: can only be called in a 2-argument httpServe handler. \
                             In a 1-argument handler, the request body is already fully read. \
                             Use readBody(req) instead.",
                            original_name
                        ),
                    });
                }

                // NB4-7: Verify that the request pack's token matches the active body state.
                if let Some(ref active) = self.active_streaming_writer
                    && !active.body_state.is_null()
                {
                    let body = unsafe { &*active.body_state };
                    let pack_token = extract_body_token(&req);
                    if pack_token != Some(body.request_token) {
                        return Err(RuntimeError {
                            message: format!(
                                "{}: request pack does not match the current active request. \
                                     The request may be stale or fabricated.",
                                original_name
                            ),
                        });
                    }
                }

                // Re-entrancy guard (same pattern as v3 streaming API).
                if let Some(ref active) = self.active_streaming_writer
                    && active.borrowed
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: cannot be called while another streaming API call is in progress (re-entrant call detected)",
                            original_name
                        ),
                    });
                }
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = true;
                }

                let result = match original_name.as_str() {
                    "readBodyChunk" => self.eval_read_body_chunk_impl(),
                    "readBodyAll" => self.eval_read_body_all_impl(&original_name),
                    _ => unreachable!(),
                };

                // Clear re-entrancy guard.
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = false;
                }

                result
            }

            // ── v3 streaming API ──
            // These functions are only callable inside a 2-arg httpServe handler.
            // The active_streaming_writer field is set during handler execution
            // and provides access to the StreamingWriter state and TcpStream.
            //
            // NB3-9: Re-entrancy guard — prevent nested streaming API calls
            // (e.g. `writeChunk(writer, writeChunk(writer, "data"))`) from
            // creating overlapping &mut references to the same StreamingWriter.
            // The guard is set here at the dispatch level so every streaming
            // function is protected uniformly, and cleared after the call
            // returns (or errors).
            "startResponse" | "writeChunk" | "endResponse" | "sseEvent" => {
                // Check re-entrancy before dispatching.
                if let Some(ref active) = self.active_streaming_writer
                    && active.borrowed
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: cannot be called while another streaming API call is in progress (re-entrant call detected)",
                            name
                        ),
                    });
                }
                // Set the guard.
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = true;
                }

                let result = match name {
                    "startResponse" => self.eval_start_response(args),
                    "writeChunk" => self.eval_write_chunk(args),
                    "endResponse" => self.eval_end_response(args),
                    "sseEvent" => self.eval_sse_event(args),
                    _ => unreachable!(),
                };

                // Clear the guard after the call completes (success or error).
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = false;
                }

                result
            }

            // ── v4 WebSocket API + v5 WebSocket revision ──
            // These functions are only callable inside a 2-arg httpServe handler.
            // Protected by the same re-entrancy guard as v3 streaming API.
            "wsUpgrade" | "wsSend" | "wsReceive" | "wsClose" | "wsCloseCode" => {
                // Check re-entrancy before dispatching.
                if let Some(ref active) = self.active_streaming_writer
                    && active.borrowed
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: cannot be called while another streaming API call is in progress (re-entrant call detected)",
                            original_name
                        ),
                    });
                }
                // Set the guard.
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = true;
                }

                let result = match original_name.as_str() {
                    "wsUpgrade" => self.eval_ws_upgrade(args),
                    "wsSend" => self.eval_ws_send(args),
                    "wsReceive" => self.eval_ws_receive(args),
                    "wsClose" => self.eval_ws_close(args),
                    "wsCloseCode" => self.eval_ws_close_code(args),
                    _ => unreachable!(),
                };

                // Clear the guard after the call completes (success or error).
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = false;
                }

                result
            }

            _ => Ok(None),
        }
    }

    // ── v3 streaming API implementation ─────────────────────────
    //
    // These methods implement startResponse / writeChunk / endResponse.
    // They access the active_streaming_writer field which holds raw pointers
    // to the stack-local StreamingWriter and TcpStream in dispatch_request.
    //
    // Zero-copy contract:
    //   - Bytes payload is sent directly via IoSlice (no copy)
    //   - Str payload is encoded to UTF-8 bytes, then sent via IoSlice
    //   - Chunk framing uses small stack-local buffers for hex prefix
    //   - No aggregate buffer (prefix + payload + suffix) is ever created

    /// Validate that args[0] is the genuine writer token created by dispatch_request.
    fn validate_writer_token(&mut self, args: &[Expr], api_name: &str) -> Result<(), RuntimeError> {
        let arg0 = match args.first() {
            Some(a) => a,
            None => {
                return Err(RuntimeError {
                    message: format!("{}: missing writer argument", api_name),
                });
            }
        };
        match self.eval_expr(arg0)? {
            Signal::Value(Value::BuchiPack(fields)) => {
                let is_valid = fields.iter().any(|(k, v)| {
                    k == "__writer_id" && matches!(v, Value::Str(s) if s == "__v3_streaming_writer")
                });
                if !is_valid {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: first argument must be the writer provided by httpServe",
                            api_name
                        ),
                    });
                }
                Ok(())
            }
            _ => Err(RuntimeError {
                message: format!(
                    "{}: first argument must be the writer provided by httpServe",
                    api_name
                ),
            }),
        }
    }

    /// `startResponse(writer, status <= 200, headers <= @[])`
    ///
    /// Updates pending status/headers on the StreamingWriter.
    /// Does NOT commit to wire — that happens on first writeChunk/endResponse.
    fn eval_start_response(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        // Check we're inside a 2-arg handler first (before token validation).
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "startResponse: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "startResponse")?;

        let active = self.active_streaming_writer.as_ref().unwrap();

        // Safety: pointers are valid during handler execution (see ActiveStreamingWriter doc).
        let writer = unsafe { &mut *active.writer };

        // State check: startResponse is only valid in Idle state.
        match writer.state {
            WriterState::Idle => {}
            WriterState::HeadPrepared => {
                return Err(RuntimeError {
                    message: "startResponse: already called. Cannot call startResponse twice."
                        .into(),
                });
            }
            WriterState::Streaming => {
                return Err(RuntimeError {
                    message: "startResponse: head already committed (chunks are being written). Cannot change status/headers after writeChunk.".into(),
                });
            }
            WriterState::Ended => {
                return Err(RuntimeError {
                    message: "startResponse: response already ended.".into(),
                });
            }
            WriterState::WebSocket => {
                return Err(RuntimeError {
                    message:
                        "startResponse: cannot use HTTP streaming API after WebSocket upgrade."
                            .into(),
                });
            }
        }

        // Arg 0: writer (BuchiPack with __writer_id sentinel) — skip, already validated.
        // Arg 1: status (Int, default 200)
        let status: u16 = if let Some(arg) = args.get(1) {
            match self.eval_expr(arg)? {
                Signal::Value(Value::Int(n)) => {
                    if !(100..=599).contains(&n) {
                        return Err(RuntimeError {
                            message: format!("startResponse: status must be 100-599, got {}", n),
                        });
                    }
                    n as u16
                }
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("startResponse: status must be Int, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            }
        } else {
            200
        };

        // Arg 2: headers (List of @(name, value), default @[])
        let headers: Vec<(String, String)> = if let Some(arg) = args.get(2) {
            match self.eval_expr(arg)? {
                Signal::Value(Value::List(items)) => {
                    let mut out = Vec::new();
                    for item in &items {
                        if let Value::BuchiPack(fields) = item {
                            let name = get_field_str(fields, "name").unwrap_or_default();
                            let value = get_field_str(fields, "value").unwrap_or_default();
                            out.push((name, value));
                        }
                    }
                    out
                }
                Signal::Value(_) => Vec::new(),
                other => return Ok(Some(other)),
            }
        } else {
            Vec::new()
        };

        // Validate reserved headers (Content-Length, Transfer-Encoding).
        StreamingWriter::validate_reserved_headers(&headers)
            .map_err(|msg| RuntimeError { message: msg })?;

        // Update pending state.
        // Re-borrow writer since self was borrowed by eval_expr above.
        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &mut *active.writer };
        writer.pending_status = status;
        writer.pending_headers = headers;
        writer.state = WriterState::HeadPrepared;

        Ok(Some(Signal::Value(Value::Unit)))
    }

    /// `writeChunk(writer, data)`
    ///
    /// Sends one chunk of body data using chunked transfer encoding.
    /// If head is not yet committed, commits it first (implicit 200/@[] or pending state).
    ///
    /// Zero-copy: uses IoSlice to send [hex_prefix, payload, suffix] without
    /// creating an aggregate buffer.
    fn eval_write_chunk(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "writeChunk: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "writeChunk")?;

        let active = self.active_streaming_writer.as_ref().unwrap();

        let writer = unsafe { &mut *active.writer };

        // State check: writeChunk is not valid after endResponse or WebSocket upgrade.
        if writer.state == WriterState::Ended {
            return Err(RuntimeError {
                message: "writeChunk: response already ended.".into(),
            });
        }
        if writer.state == WriterState::WebSocket {
            return Err(RuntimeError {
                message: "writeChunk: cannot use HTTP streaming API after WebSocket upgrade."
                    .into(),
            });
        }

        // Arg 0: writer (skip)
        // Arg 1: data (Bytes or Str)
        let data = match args.get(1) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(v) => v,
                other => return Ok(Some(other)),
            },
            None => {
                return Err(RuntimeError {
                    message: "writeChunk: missing argument 'data'".into(),
                });
            }
        };

        // Extract payload bytes. Bytes = zero-copy fast path, Str = UTF-8 encode.
        let payload: &[u8] = match &data {
            Value::Bytes(b) => b.as_slice(),
            Value::Str(s) => s.as_bytes(),
            other => {
                return Err(RuntimeError {
                    message: format!("writeChunk: data must be Bytes or Str, got {}", other),
                });
            }
        };

        // Empty chunk is no-op (design contract: avoid colliding with terminator).
        if payload.is_empty() {
            return Ok(Some(Signal::Value(Value::Unit)));
        }

        // Re-borrow after eval_expr.
        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &mut *active.writer };
        let stream = unsafe { &mut *active.stream };

        // Bodyless status check: 1xx/204/205/304 cannot have body chunks.
        if StreamingWriter::is_bodyless_status(writer.pending_status) {
            return Err(RuntimeError {
                message: format!(
                    "writeChunk: status {} does not allow a message body",
                    writer.pending_status
                ),
            });
        }

        // Commit head if not yet committed.
        if writer.state == WriterState::Idle || writer.state == WriterState::HeadPrepared {
            let head_bytes = build_streaming_head(writer.pending_status, &writer.pending_headers);
            write_all_retry(stream, &head_bytes)?;
            writer.state = WriterState::Streaming;
        }

        // Send chunk using IoSlice (zero-copy for payload).
        // Wire format: <hex-size>\r\n<payload>\r\n
        let hex_prefix = format!("{:x}\r\n", payload.len());
        let suffix = b"\r\n";

        let bufs = &[
            std::io::IoSlice::new(hex_prefix.as_bytes()),
            std::io::IoSlice::new(payload),
            std::io::IoSlice::new(suffix),
        ];
        write_vectored_all(stream, bufs)?;

        Ok(Some(Signal::Value(Value::Unit)))
    }

    /// `endResponse(writer)`
    ///
    /// Terminates the chunked response by sending `0\r\n\r\n`.
    /// If head is not yet committed, commits it first (empty chunked body).
    /// Idempotent: second call is a no-op.
    fn eval_end_response(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "endResponse: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "endResponse")?;

        let active = self.active_streaming_writer.as_ref().unwrap();

        let writer = unsafe { &mut *active.writer };
        let stream = unsafe { &mut *active.stream };

        // Idempotent: no-op if already ended.
        if writer.state == WriterState::Ended {
            return Ok(Some(Signal::Value(Value::Unit)));
        }
        if writer.state == WriterState::WebSocket {
            return Err(RuntimeError {
                message: "endResponse: cannot use HTTP streaming API after WebSocket upgrade."
                    .into(),
            });
        }

        // Commit head if not yet committed.
        if writer.state == WriterState::Idle || writer.state == WriterState::HeadPrepared {
            let head_bytes = build_streaming_head(writer.pending_status, &writer.pending_headers);
            write_all_retry(stream, &head_bytes)?;
        }

        // Send chunked terminator — but only for status codes that allow a body.
        // Bodyless statuses (1xx/204/205/304) have head-only responses.
        if !StreamingWriter::is_bodyless_status(writer.pending_status) {
            write_all_retry(stream, b"0\r\n\r\n")?;
        }
        writer.state = WriterState::Ended;

        Ok(Some(Signal::Value(Value::Unit)))
    }

    /// `sseEvent(writer, event, data)`
    ///
    /// SSE convenience API. Sends one Server-Sent Event in wire format:
    ///   event: <event>\n
    ///   data: <line1>\n
    ///   data: <line2>\n
    ///   \n
    ///
    /// Auto-header behavior (NET3-3b, NET3-3c):
    ///   - If Content-Type is not already set in pending headers, sets
    ///     `text/event-stream; charset=utf-8`
    ///   - If Cache-Control is not already set, sets `no-cache`
    ///   - These are applied once (sse_mode flag prevents re-checking)
    ///
    /// Multiline data (NET3-3d):
    ///   - Splits `data` on `\n` and emits a `data: ` line for each
    ///
    /// Zero-copy note:
    ///   - Each SSE line is a separate small String; no aggregate String
    ///     is built for the entire event.
    ///   - All lines are sent as one chunked frame via vectored I/O
    ///     (IoSlice), so the event arrives atomically to the client.
    fn eval_sse_event(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "sseEvent: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "sseEvent")?;

        // Evaluate event name (arg 1).
        let event_name: String = if let Some(arg) = args.get(1) {
            match self.eval_expr(arg)? {
                Signal::Value(Value::Str(s)) => s,
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("sseEvent: event must be Str, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            }
        } else {
            return Err(RuntimeError {
                message: "sseEvent: missing argument 'event'".into(),
            });
        };

        // Evaluate data (arg 2).
        let data: String = if let Some(arg) = args.get(2) {
            match self.eval_expr(arg)? {
                Signal::Value(Value::Str(s)) => s,
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("sseEvent: data must be Str, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            }
        } else {
            return Err(RuntimeError {
                message: "sseEvent: missing argument 'data'".into(),
            });
        };

        // Re-borrow after eval_expr calls.
        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &mut *active.writer };
        let stream = unsafe { &mut *active.stream };

        // State check: sseEvent is not valid after endResponse or WebSocket.
        if writer.state == WriterState::Ended {
            return Err(RuntimeError {
                message: "sseEvent: response already ended.".into(),
            });
        }
        if writer.state == WriterState::WebSocket {
            return Err(RuntimeError {
                message: "sseEvent: cannot use HTTP streaming API after WebSocket upgrade.".into(),
            });
        }

        // Bodyless status check.
        if StreamingWriter::is_bodyless_status(writer.pending_status) {
            return Err(RuntimeError {
                message: format!(
                    "sseEvent: status {} does not allow a message body",
                    writer.pending_status
                ),
            });
        }

        // NET3-3b, NET3-3c: Auto-set SSE headers if not already in sse_mode.
        if !writer.sse_mode {
            // If head is already committed (Streaming state), check whether SSE
            // headers were already set by the user via startResponse. If not,
            // auto-headers cannot be retroactively added to the response.
            if writer.state == WriterState::Streaming {
                let has_sse_content_type = writer.pending_headers.iter().any(|(k, v)| {
                    k.eq_ignore_ascii_case("content-type")
                        && v.to_ascii_lowercase().contains("text/event-stream")
                });
                let has_cache_no_cache = writer.pending_headers.iter().any(|(k, v)| {
                    k.eq_ignore_ascii_case("cache-control")
                        && v.to_ascii_lowercase().contains("no-cache")
                });
                if !has_sse_content_type || !has_cache_no_cache {
                    return Err(RuntimeError {
                        message: "sseEvent: head already committed without SSE headers. \
                                  Call sseEvent before writeChunk, or use startResponse \
                                  with explicit Content-Type: text/event-stream and \
                                  Cache-Control: no-cache headers before writeChunk."
                            .into(),
                    });
                }
                // User already set both SSE headers; mark sse_mode and proceed.
                writer.sse_mode = true;
            } else {
                // Head not yet committed — safe to add auto-headers.
                // Check if Content-Type is already set.
                let has_content_type = writer
                    .pending_headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
                if !has_content_type {
                    writer.pending_headers.push((
                        "Content-Type".to_string(),
                        "text/event-stream; charset=utf-8".to_string(),
                    ));
                }

                // Check if Cache-Control is already set.
                let has_cache_control = writer
                    .pending_headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("cache-control"));
                if !has_cache_control {
                    writer
                        .pending_headers
                        .push(("Cache-Control".to_string(), "no-cache".to_string()));
                }

                writer.sse_mode = true;
            }
        }

        // Commit head if not yet committed.
        if writer.state == WriterState::Idle || writer.state == WriterState::HeadPrepared {
            let head_bytes = build_streaming_head(writer.pending_status, &writer.pending_headers);
            write_all_retry(stream, &head_bytes)?;
            writer.state = WriterState::Streaming;
        }

        // Send SSE event via zero-copy vectored I/O in one chunked frame.
        // Wire format:
        //   event: <event>\n      (omit if event is empty)
        //   data: <line1>\n
        //   data: <line2>\n
        //   \n                    (event terminator)
        //
        // We reference the original event_name and data strings directly via
        // IoSlice, interleaving static prefix/suffix byte slices. No per-line
        // String is allocated — payload data is never copied into a new buffer.
        // (NET_IMPL_GUIDE.md: "payload copy は最小限 / 1回まで",
        //  "巨大な1文字列を組み立てない")

        let event_prefix = b"event: ";
        let data_prefix = b"data: ";
        let newline = b"\n";
        let terminator = b"\n";

        // Split data into &str slices (zero-copy views into `data`).
        let data_lines: Vec<&str> = data.split('\n').collect();

        // First pass: compute total payload byte length for chunk header.
        let mut total_len: usize = 0;
        if !event_name.is_empty() {
            total_len += event_prefix.len() + event_name.len() + newline.len();
        }
        for line in &data_lines {
            total_len += data_prefix.len() + line.len() + newline.len();
        }
        total_len += terminator.len();

        // Build chunk frame: hex_prefix + payload slices + suffix.
        let hex_prefix = format!("{:x}\r\n", total_len);
        let suffix = b"\r\n";

        // Capacity: 1 (hex) + 3 (event line) + 3*n (data lines) + 1 (term) + 1 (suffix)
        let mut bufs: Vec<std::io::IoSlice<'_>> = Vec::with_capacity(3 + 3 * data_lines.len() + 3);
        bufs.push(std::io::IoSlice::new(hex_prefix.as_bytes()));

        if !event_name.is_empty() {
            bufs.push(std::io::IoSlice::new(event_prefix));
            bufs.push(std::io::IoSlice::new(event_name.as_bytes()));
            bufs.push(std::io::IoSlice::new(newline));
        }

        for line in &data_lines {
            bufs.push(std::io::IoSlice::new(data_prefix));
            bufs.push(std::io::IoSlice::new(line.as_bytes()));
            bufs.push(std::io::IoSlice::new(newline));
        }

        bufs.push(std::io::IoSlice::new(terminator));
        bufs.push(std::io::IoSlice::new(suffix));

        write_vectored_all(stream, &bufs)?;

        Ok(Some(Signal::Value(Value::Unit)))
    }

    // ── v4 request body streaming implementation ─────────────────
    //
    // readBodyChunk(req) → Lax[Bytes]
    //   Reads the next chunk of request body from the socket.
    //   - Chunked TE: decodes one chunk at a time
    //   - Content-Length: reads in 8KB increments
    //   - Body end: returns Lax empty (hasValue = false)
    //
    // readBodyAll(req) → Bytes
    //   Reads all remaining body bytes. This is the only aggregate-permitted path.

    /// Build a Lax[Bytes] with a value.
    fn make_lax_bytes_value(data: Vec<u8>) -> Value {
        Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__value".into(), Value::Bytes(data)),
            ("__default".into(), Value::Bytes(vec![])),
            ("__type".into(), Value::Str("Lax".into())),
        ])
    }

    /// Build a Lax[Bytes] empty (hasValue = false).
    fn make_lax_bytes_empty() -> Value {
        Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("__value".into(), Value::Bytes(vec![])),
            ("__default".into(), Value::Bytes(vec![])),
            ("__type".into(), Value::Str("Lax".into())),
        ])
    }

    /// `readBodyChunk(req)` implementation.
    ///
    /// Reads the next chunk of request body from the TcpStream.
    /// Returns Lax[Bytes] with the chunk data, or Lax empty when body is done.
    ///
    /// Zero-copy contract: each chunk is returned independently; no aggregate buffer.
    fn eval_read_body_chunk_impl(&mut self) -> Result<Option<Signal>, RuntimeError> {
        // Must be inside a 2-arg handler.
        let active = match self.active_streaming_writer.as_ref() {
            Some(a) => a,
            None => {
                return Err(RuntimeError {
                    message:
                        "readBodyChunk: can only be called inside a 2-argument httpServe handler"
                            .into(),
                });
            }
        };

        // v4: After WebSocket upgrade, readBodyChunk is not allowed.
        let writer = unsafe { &*active.writer };
        if writer.state == WriterState::WebSocket {
            return Err(RuntimeError {
                message: "readBodyChunk: cannot read HTTP body after WebSocket upgrade.".into(),
            });
        }

        if active.body_state.is_null() {
            return Err(RuntimeError {
                message: "readBodyChunk: no body streaming state available".into(),
            });
        }

        // Safety: body_state is valid during handler execution.
        let body = unsafe { &mut *active.body_state };
        let stream = unsafe { &mut *active.stream };

        body.any_read_started = true;

        // NET4-1d: If body is already fully read, return Lax empty.
        if body.fully_read {
            return Ok(Some(Signal::Value(Self::make_lax_bytes_empty())));
        }

        if body.is_chunked {
            // ── NET4-1b: Chunked TE decode ──
            Self::read_body_chunk_chunked(body, stream)
        } else {
            // ── NET4-1c: Content-Length body ──
            Self::read_body_chunk_content_length(body, stream)
        }
    }

    /// Read one chunk from a chunked transfer-encoded body.
    fn read_body_chunk_chunked(
        body: &mut RequestBodyState,
        stream: &mut ConnStream,
    ) -> Result<Option<Signal>, RuntimeError> {
        const READ_BUF_SIZE: usize = 8192;

        loop {
            match body.chunked_state {
                ChunkedDecoderState::Done => {
                    body.fully_read = true;
                    return Ok(Some(Signal::Value(Self::make_lax_bytes_empty())));
                }
                ChunkedDecoderState::WaitingChunkSize => {
                    // NB6-7: Read chunk-size line as bytes; parse hex directly.
                    let line = Self::read_line_from_body(body, stream)?;
                    let trimmed = Self::trim_bytes(&line);
                    if trimmed.is_empty() {
                        // Could be trailing CRLF; try again.
                        continue;
                    }
                    let chunk_size =
                        Self::parse_chunk_size_bytes(&line).ok_or_else(|| RuntimeError {
                            message: format!(
                                "readBodyChunk: invalid chunk-size '{}' in chunked body",
                                String::from_utf8_lossy(trimmed)
                            ),
                        })?;

                    if chunk_size == 0 {
                        // Terminal chunk. Drain all trailing headers + final CRLF (NB4-8).
                        body.chunked_state = ChunkedDecoderState::Done;
                        body.fully_read = true;
                        Self::drain_chunked_trailers(body, stream)?;
                        return Ok(Some(Signal::Value(Self::make_lax_bytes_empty())));
                    }

                    body.chunked_state = ChunkedDecoderState::ReadingChunkData {
                        remaining: chunk_size,
                    };
                }
                ChunkedDecoderState::ReadingChunkData { remaining } => {
                    if remaining == 0 {
                        body.chunked_state = ChunkedDecoderState::WaitingChunkTrailer;
                        continue;
                    }

                    // Read up to `remaining` bytes from leftover + stream.
                    let to_read = remaining.min(READ_BUF_SIZE);
                    let data = Self::read_exact_from_body(body, stream, to_read)?;
                    let actually_read = data.len();

                    // NB4-18: short read (EOF) in chunked data is a protocol error.
                    if actually_read == 0 {
                        return Err(RuntimeError {
                            message: format!(
                                "readBodyChunk: truncated chunked body — expected {} more chunk-data bytes but got EOF",
                                remaining
                            ),
                        });
                    }

                    let new_remaining = remaining - actually_read;
                    body.chunked_state = ChunkedDecoderState::ReadingChunkData {
                        remaining: new_remaining,
                    };

                    body.bytes_consumed += actually_read as i64;
                    return Ok(Some(Signal::Value(Self::make_lax_bytes_value(data))));
                }
                ChunkedDecoderState::WaitingChunkTrailer => {
                    // NB4-18: Read the CRLF after chunk data and validate it is
                    // exactly empty (only whitespace/CRLF). Non-empty content or
                    // EOF is a protocol error.
                    let line = Self::read_line_from_body(body, stream)?;
                    let trimmed = Self::trim_bytes(&line);
                    if !trimmed.is_empty() {
                        return Err(RuntimeError {
                            message: format!(
                                "readBodyChunk: malformed chunk trailer — expected CRLF after chunk data, \
                                 got {:?}",
                                String::from_utf8_lossy(&line)
                            ),
                        });
                    }
                    if line.is_empty() {
                        // EOF before CRLF — protocol error.
                        return Err(RuntimeError {
                            message:
                                "readBodyChunk: missing CRLF after chunk data (unexpected EOF)"
                                    .into(),
                        });
                    }
                    body.chunked_state = ChunkedDecoderState::WaitingChunkSize;
                }
            }
        }
    }

    /// Read one chunk from a Content-Length body.
    fn read_body_chunk_content_length(
        body: &mut RequestBodyState,
        stream: &mut ConnStream,
    ) -> Result<Option<Signal>, RuntimeError> {
        const READ_BUF_SIZE: usize = 8192;

        let remaining = body.content_length - body.bytes_consumed;
        if remaining <= 0 {
            body.fully_read = true;
            return Ok(Some(Signal::Value(Self::make_lax_bytes_empty())));
        }

        let to_read = (remaining as usize).min(READ_BUF_SIZE);
        let data = Self::read_exact_from_body(body, stream, to_read)?;
        if data.is_empty() {
            // NB4-18: EOF before Content-Length exhausted is a protocol error.
            return Err(RuntimeError {
                message: format!(
                    "readBodyChunk: truncated body — expected {} bytes (Content-Length) but got EOF after {} bytes",
                    body.content_length, body.bytes_consumed
                ),
            });
        }
        body.bytes_consumed += data.len() as i64;
        if body.bytes_consumed >= body.content_length {
            body.fully_read = true;
        }
        Ok(Some(Signal::Value(Self::make_lax_bytes_value(data))))
    }

    /// Consume all trailing headers after a chunked terminal chunk (size=0).
    /// RFC 7230 Section 4.1.2: After the terminal chunk, there may be
    /// trailer header fields followed by a final CRLF. We read lines
    /// until we see an empty line (just CRLF), which marks the end of
    /// the chunked message. This prevents leftover trailer bytes from
    /// corrupting the next request on a keep-alive connection.
    fn drain_chunked_trailers(
        body: &mut RequestBodyState,
        stream: &mut ConnStream,
    ) -> Result<(), RuntimeError> {
        // Read lines until we get an empty line (just whitespace/CRLF).
        // Safety limit: at most 64 trailer lines to prevent infinite loops.
        for _ in 0..64 {
            let line = Self::read_line_from_body(body, stream)?;
            // NB4-18: EOF (0 raw bytes) != valid empty line ("\r\n").
            if line.is_empty() {
                return Err(RuntimeError {
                    message: "chunked body error: missing final CRLF after terminal chunk"
                        .to_string(),
                });
            }
            let trimmed = Self::trim_bytes(&line);
            if trimmed.is_empty() {
                // Final empty line found; trailers fully consumed.
                return Ok(());
            }
            // Non-empty line: a trailer header. Continue reading.
        }
        // Too many trailer lines; treat as consumed (close will handle cleanup).
        Ok(())
    }

    /// Read a line (up to CRLF) from leftover buffer then stream.
    ///
    /// NB5-21: After leftover is exhausted, reads in 64-byte chunks from the
    /// stream instead of byte-by-byte. Excess bytes beyond the LF are pushed
    /// back into `body.leftover` so they are available for subsequent reads.
    /// This reduces syscall count from O(line_length) to O(1) for typical
    /// chunk-size lines (4-8 bytes), and avoids per-byte rustls overhead on TLS.
    ///
    /// NB6-7: Returns Vec<u8> instead of String to avoid per-line UTF-8 validation
    /// and String heap allocation. Chunk-size lines are always ASCII hex digits.
    ///
    /// NB6-8: Excess pushback now uses in-place splice (drain + insert) on
    /// body.leftover instead of allocating a new Vec per pushback.
    fn read_line_from_body(
        body: &mut RequestBodyState,
        stream: &mut ConnStream,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut line = Vec::new();

        // First consume from leftover.
        while body.has_leftover() {
            let b = body.leftover[body.leftover_pos];
            body.leftover_pos += 1;
            line.push(b);
            if b == b'\n' {
                return Ok(line);
            }
        }

        // Then read from stream in chunks until LF is found.
        let mut chunk_buf = [0u8; 64];
        loop {
            match std::io::Read::read(stream, &mut chunk_buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    // Scan the chunk for LF.
                    if let Some(lf_pos) = chunk_buf[..n].iter().position(|&b| b == b'\n') {
                        // Include everything up to and including the LF.
                        line.extend_from_slice(&chunk_buf[..=lf_pos]);
                        // NB6-8: Push excess bytes back into leftover using in-place
                        // splice instead of allocating a new Vec per pushback.
                        let excess = &chunk_buf[lf_pos + 1..n];
                        if !excess.is_empty() {
                            // Drain consumed portion and prepend excess.
                            body.leftover.drain(..body.leftover_pos);
                            body.leftover_pos = 0;
                            // Insert excess at the beginning.
                            body.leftover.splice(..0, excess.iter().copied());
                        }
                        break;
                    } else {
                        // No LF in this chunk — append all and continue reading.
                        line.extend_from_slice(&chunk_buf[..n]);
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => {
                    return Err(RuntimeError {
                        message: format!("readBodyChunk: read error: {}", e),
                    });
                }
            }
        }

        Ok(line)
    }

    /// NB6-7: Trim ASCII whitespace from a byte slice (equivalent to str::trim()).
    #[inline]
    fn trim_bytes(data: &[u8]) -> &[u8] {
        let start = data
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(data.len());
        let end = data
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(start, |p| p + 1);
        &data[start..end]
    }

    /// NB6-7: Parse hex chunk size directly from byte slice.
    /// Strips any chunk-extension after ';' and trims whitespace.
    #[inline]
    fn parse_chunk_size_bytes(line: &[u8]) -> Option<usize> {
        let trimmed = Self::trim_bytes(line);
        // Strip chunk-extension after ';'
        let hex_part = match trimmed.iter().position(|&b| b == b';') {
            Some(pos) => Self::trim_bytes(&trimmed[..pos]),
            None => trimmed,
        };
        if hex_part.is_empty() {
            return None;
        }
        // Parse hex digits directly from bytes.
        let mut result: usize = 0;
        for &b in hex_part {
            let digit = match b {
                b'0'..=b'9' => (b - b'0') as usize,
                b'a'..=b'f' => (b - b'a' + 10) as usize,
                b'A'..=b'F' => (b - b'A' + 10) as usize,
                _ => return None,
            };
            result = result.checked_mul(16)?.checked_add(digit)?;
        }
        Some(result)
    }

    /// Read up to `count` bytes from leftover buffer then stream.
    /// Returns a Vec of the bytes actually read (may be less than count on EOF).
    ///
    /// NB5-19: Single allocation — `result` is pre-sized to `count` and the stream
    /// reads directly into the unfilled tail, avoiding the previous intermediate
    /// `vec![0u8; remaining]` allocation per read call.
    fn read_exact_from_body(
        body: &mut RequestBodyState,
        stream: &mut ConnStream,
        count: usize,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut result = vec![0u8; count];
        let mut len = 0usize;

        // First, drain from leftover directly into result.
        while len < count && body.has_leftover() {
            result[len] = body.leftover[body.leftover_pos];
            body.leftover_pos += 1;
            len += 1;
        }

        // Then read from stream directly into the unfilled tail of result.
        while len < count {
            match std::io::Read::read(stream, &mut result[len..count]) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    len += n;
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if len == 0 {
                        // If we have nothing yet, this might be a real timeout.
                        // Retry one more time with a blocking read.
                        continue;
                    }
                    break; // Return what we have.
                }
                Err(e) => {
                    return Err(RuntimeError {
                        message: format!("readBodyChunk: read error: {}", e),
                    });
                }
            }
        }

        result.truncate(len);
        Ok(result)
    }

    /// `readBodyAll(req)` implementation.
    ///
    /// Reads all remaining body bytes by repeatedly calling readBodyChunk logic.
    /// This is the only path where aggregate buffering is permitted.
    fn eval_read_body_all_impl(&mut self, api_name: &str) -> Result<Option<Signal>, RuntimeError> {
        let active = match self.active_streaming_writer.as_ref() {
            Some(a) => a,
            None => {
                return Err(RuntimeError {
                    message: format!(
                        "{}: can only be called inside a 2-argument httpServe handler",
                        api_name
                    ),
                });
            }
        };

        // v4: After WebSocket upgrade, readBodyAll is not allowed.
        let writer = unsafe { &*active.writer };
        if writer.state == WriterState::WebSocket {
            return Err(RuntimeError {
                message: format!(
                    "{}: cannot read HTTP body after WebSocket upgrade.",
                    api_name
                ),
            });
        }

        if active.body_state.is_null() {
            return Err(RuntimeError {
                message: format!("{}: no body streaming state available", api_name),
            });
        }

        // Safety: body_state is valid during handler execution.
        let body = unsafe { &mut *active.body_state };
        let stream = unsafe { &mut *active.stream };

        body.any_read_started = true;

        // If body is already fully read, return empty Bytes.
        if body.fully_read {
            return Ok(Some(Signal::Value(Value::Bytes(vec![]))));
        }

        // Aggregate all remaining body bytes.
        // This is the only place where aggregate buffering is permitted.
        let mut all_bytes: Vec<u8> = Vec::new();

        if body.is_chunked {
            // Chunked path: read all chunks.
            loop {
                match body.chunked_state {
                    ChunkedDecoderState::Done => {
                        body.fully_read = true;
                        break;
                    }
                    ChunkedDecoderState::WaitingChunkSize => {
                        // NB6-7: Parse chunk size directly from byte slice.
                        let line = Self::read_line_from_body(body, stream)?;
                        let trimmed = Self::trim_bytes(&line);
                        if trimmed.is_empty() {
                            continue;
                        }
                        let chunk_size =
                            Self::parse_chunk_size_bytes(&line).ok_or_else(|| RuntimeError {
                                message: format!(
                                    "{}: invalid chunk-size '{}' in chunked body",
                                    api_name,
                                    String::from_utf8_lossy(trimmed)
                                ),
                            })?;

                        if chunk_size == 0 {
                            body.chunked_state = ChunkedDecoderState::Done;
                            body.fully_read = true;
                            // NB4-8: Drain all trailing headers + final CRLF.
                            Self::drain_chunked_trailers(body, stream)?;
                            break;
                        }

                        body.chunked_state = ChunkedDecoderState::ReadingChunkData {
                            remaining: chunk_size,
                        };
                    }
                    ChunkedDecoderState::ReadingChunkData { remaining } => {
                        if remaining == 0 {
                            body.chunked_state = ChunkedDecoderState::WaitingChunkTrailer;
                            continue;
                        }
                        let data = Self::read_exact_from_body(body, stream, remaining)?;
                        let n = data.len();
                        all_bytes.extend_from_slice(&data);
                        let new_remaining = remaining - n;
                        body.chunked_state = ChunkedDecoderState::ReadingChunkData {
                            remaining: new_remaining,
                        };
                    }
                    ChunkedDecoderState::WaitingChunkTrailer => {
                        // NB4-18: Validate CRLF after chunk data.
                        let line = Self::read_line_from_body(body, stream)?;
                        let trimmed = Self::trim_bytes(&line);
                        if !trimmed.is_empty() {
                            return Err(RuntimeError {
                                message: format!(
                                    "{}: malformed chunk trailer — expected CRLF after chunk data, \
                                     got {:?}",
                                    api_name,
                                    String::from_utf8_lossy(&line)
                                ),
                            });
                        }
                        if line.is_empty() {
                            return Err(RuntimeError {
                                message: format!(
                                    "{}: missing CRLF after chunk data (unexpected EOF)",
                                    api_name
                                ),
                            });
                        }
                        body.chunked_state = ChunkedDecoderState::WaitingChunkSize;
                    }
                }
            }
        } else {
            // Content-Length path: read remaining bytes.
            let remaining = (body.content_length - body.bytes_consumed) as usize;
            if remaining > 0 {
                let data = Self::read_exact_from_body(body, stream, remaining)?;
                body.bytes_consumed += data.len() as i64;
                all_bytes = data;
            }
            body.fully_read = true;
        }

        Ok(Some(Signal::Value(Value::Bytes(all_bytes))))
    }

    // ── v4 WebSocket implementation ─────────────────────────────
    //
    // WebSocket handshake + frame I/O per RFC 6455.
    //
    // Design constraints (from NET_DESIGN.md / NET_IMPL_GUIDE.md):
    //   - wsUpgrade failure → no wire write, Lax empty
    //   - wsUpgrade success → 101 response + WriterState::WebSocket
    //   - server→client frames: MASK=0
    //   - client→server frames: MASK=1, XOR decode in-place
    //   - fragmented frames (FIN=0) → protocol error → close
    //   - oversized payload (>16 MiB) → close
    //   - ping → auto pong → advance to next data frame
    //   - close frame → Lax empty
    //   - wsClose → send close frame, idempotent
    //   - auto close on handler return

    /// WebSocket opcodes.
    const WS_OPCODE_TEXT: u8 = 0x1;
    const WS_OPCODE_BINARY: u8 = 0x2;
    const WS_OPCODE_CLOSE: u8 = 0x8;
    #[allow(dead_code)]
    const WS_OPCODE_PING: u8 = 0x9;
    const WS_OPCODE_PONG: u8 = 0xA;

    /// Maximum WebSocket payload size: 16 MiB.
    const WS_MAX_PAYLOAD: u64 = 16 * 1024 * 1024;

    /// RFC 6455 magic GUID for Sec-WebSocket-Accept calculation.
    const WS_GUID: &'static str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

    /// Compute Sec-WebSocket-Accept from Sec-WebSocket-Key (NET4-2b).
    /// SHA-1(key + GUID) → Base64.
    fn compute_ws_accept(key: &str) -> String {
        use base64::Engine;
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(key.as_bytes());
        hasher.update(Self::WS_GUID.as_bytes());
        let hash = hasher.finalize();
        base64::engine::general_purpose::STANDARD.encode(hash)
    }

    /// Validate a WebSocket ws token argument (similar to validate_writer_token).
    /// NB4-10: Validate ws token — checks both sentinel AND connection-scoped token.
    fn validate_ws_token(&mut self, args: &[Expr], api_name: &str) -> Result<(), RuntimeError> {
        let arg0 = match args.first() {
            Some(a) => a,
            None => {
                return Err(RuntimeError {
                    message: format!("{}: missing ws argument", api_name),
                });
            }
        };
        match self.eval_expr(arg0)? {
            Signal::Value(Value::BuchiPack(fields)) => {
                // Check sentinel.
                let is_valid = fields.iter().any(|(k, v)| {
                    k == "__ws_id" && matches!(v, Value::Str(s) if s == "__v4_websocket_conn")
                });
                if !is_valid {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: first argument must be the WebSocket connection from wsUpgrade",
                            api_name
                        ),
                    });
                }
                // NB4-10: Verify connection-scoped token matches active ws_token.
                let pack_token = fields.iter().find_map(|(k, v)| {
                    if k == "__ws_token" {
                        match v {
                            Value::Int(n) => Some(*n as u64),
                            _ => None,
                        }
                    } else {
                        None
                    }
                });
                if let Some(ref active) = self.active_streaming_writer
                    && (active.ws_token == 0 || pack_token != Some(active.ws_token))
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: WebSocket connection does not match the current active connection. \
                                 The connection may be stale or fabricated.",
                            api_name
                        ),
                    });
                }
                Ok(())
            }
            _ => Err(RuntimeError {
                message: format!(
                    "{}: first argument must be the WebSocket connection from wsUpgrade",
                    api_name
                ),
            }),
        }
    }

    /// `wsUpgrade(req, writer)` → `Lax[@(ws: WsConn)]` (NET4-2a).
    ///
    /// Validates the WebSocket upgrade request, sends 101 response if valid,
    /// and transitions writer state to WebSocket.
    /// On failure: returns Lax empty, writes nothing to wire.
    fn eval_ws_upgrade(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsUpgrade: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Evaluate req argument.
        let req = match args.first() {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(v) => v,
                other => return Ok(Some(other)),
            },
            None => {
                return Err(RuntimeError {
                    message: "wsUpgrade: missing argument 'req'".into(),
                });
            }
        };

        // NB4-10: Verify that the request pack's token matches the active body state.
        // This prevents stale/fabricated request packs from triggering an upgrade.
        if let Some(ref active) = self.active_streaming_writer
            && !active.body_state.is_null()
        {
            let body = unsafe { &*active.body_state };
            let pack_token = extract_body_token(&req);
            if pack_token != Some(body.request_token) {
                return Err(RuntimeError {
                    message: "wsUpgrade: request pack does not match the current active request. \
                                 The request may be stale or fabricated."
                        .into(),
                });
            }
        }

        // Validate writer token (2nd arg).
        self.validate_writer_token(&args[1..], "wsUpgrade")?;

        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &mut *active.writer };

        // State check: wsUpgrade is only valid in Idle state (before any head commit).
        match writer.state {
            WriterState::Idle => {}
            WriterState::HeadPrepared | WriterState::Streaming => {
                return Err(RuntimeError {
                    message: "wsUpgrade: cannot upgrade after HTTP response has started. \
                             wsUpgrade must be called before startResponse/writeChunk."
                        .into(),
                });
            }
            WriterState::Ended => {
                return Err(RuntimeError {
                    message: "wsUpgrade: cannot upgrade after HTTP response has ended.".into(),
                });
            }
            WriterState::WebSocket => {
                return Err(RuntimeError {
                    message: "wsUpgrade: WebSocket upgrade already completed.".into(),
                });
            }
        }

        // Extract request fields for validation.
        let req_fields = match &req {
            Value::BuchiPack(f) => f,
            _ => {
                return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
            }
        };

        // Validate: must be GET.
        let method_ok = match get_field_value(req_fields, "method") {
            Some(Value::BuchiPack(method_span)) => {
                // Method is stored as a span in raw bytes.
                let raw = match get_field_value(req_fields, "raw") {
                    Some(Value::Bytes(b)) => b,
                    _ => return Ok(Some(Signal::Value(Self::make_lax_ws_empty()))),
                };
                let start = get_field_int(method_span, "start").unwrap_or(0) as usize;
                let len = get_field_int(method_span, "len").unwrap_or(0) as usize;
                let end = start.saturating_add(len).min(raw.len());
                let method_str = std::str::from_utf8(&raw[start..end]).unwrap_or("");
                method_str.eq_ignore_ascii_case("GET")
            }
            _ => false,
        };
        if !method_ok {
            return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
        }

        // Check request has no body (Content-Length must be 0 or absent, not chunked).
        let cl = get_field_int(req_fields, "contentLength").unwrap_or(0);
        let chunked = match get_field_value(req_fields, "chunked") {
            Some(Value::Bool(b)) => *b,
            _ => false,
        };
        if cl > 0 || chunked {
            return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
        }

        // Extract headers for WebSocket validation.
        let headers = match get_field_value(req_fields, "headers") {
            Some(Value::List(h)) => h,
            _ => return Ok(Some(Signal::Value(Self::make_lax_ws_empty()))),
        };

        // Extract raw bytes for header value extraction.
        let raw = match get_field_value(req_fields, "raw") {
            Some(Value::Bytes(b)) => b,
            _ => return Ok(Some(Signal::Value(Self::make_lax_ws_empty()))),
        };

        // Helper to extract header value from span.
        let get_header_value =
            |headers: &[Value], raw: &[u8], target_name: &str| -> Option<String> {
                for h in headers {
                    if let Value::BuchiPack(hf) = h {
                        let name_span = match get_field_value(hf, "name") {
                            Some(Value::BuchiPack(s)) => s,
                            _ => continue,
                        };
                        let n_start = get_field_int(name_span, "start").unwrap_or(0) as usize;
                        let n_len = get_field_int(name_span, "len").unwrap_or(0) as usize;
                        let n_end = n_start.saturating_add(n_len).min(raw.len());
                        let name_str = std::str::from_utf8(&raw[n_start..n_end]).unwrap_or("");
                        if name_str.eq_ignore_ascii_case(target_name) {
                            let val_span = match get_field_value(hf, "value") {
                                Some(Value::BuchiPack(s)) => s,
                                _ => continue,
                            };
                            let v_start = get_field_int(val_span, "start").unwrap_or(0) as usize;
                            let v_len = get_field_int(val_span, "len").unwrap_or(0) as usize;
                            let v_end = v_start.saturating_add(v_len).min(raw.len());
                            let val_str = std::str::from_utf8(&raw[v_start..v_end]).unwrap_or("");
                            return Some(val_str.to_string());
                        }
                    }
                }
                None
            };

        // Validate: Upgrade: websocket
        let upgrade_val = get_header_value(headers, raw, "Upgrade");
        if !upgrade_val
            .as_ref()
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        {
            return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
        }

        // Validate: Connection: Upgrade (may contain multiple values separated by comma)
        let conn_val = get_header_value(headers, raw, "Connection");
        let connection_ok = conn_val.as_ref().is_some_and(|v| {
            v.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("Upgrade"))
        });
        if !connection_ok {
            return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
        }

        // Validate: Sec-WebSocket-Version: 13
        let version_val = get_header_value(headers, raw, "Sec-WebSocket-Version");
        if version_val.as_ref().is_none_or(|v| v.trim() != "13") {
            return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
        }

        // Validate: Sec-WebSocket-Key (must be present, 24-char base64)
        let ws_key = match get_header_value(headers, raw, "Sec-WebSocket-Key") {
            Some(k) => k.trim().to_string(),
            None => {
                return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
            }
        };
        // NB4-11: RFC 6455: key must be a base64 encoded 16-byte value (= 24 chars with padding).
        // Validate both the length (24 chars) and that it decodes to exactly 16 bytes.
        if ws_key.len() != 24 {
            return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
        }
        match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &ws_key) {
            Ok(decoded) if decoded.len() == 16 => {} // Valid
            _ => {
                return Ok(Some(Signal::Value(Self::make_lax_ws_empty())));
            }
        }

        // All validations passed. Send 101 Switching Protocols response.
        let accept = Self::compute_ws_accept(&ws_key);
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {}\r\n\
             \r\n",
            accept
        );

        // Write the 101 response to wire.
        let active = self.active_streaming_writer.as_ref().unwrap();
        let stream = unsafe { &mut *active.stream };
        write_all_retry(stream, response.as_bytes())?;

        // Transition to WebSocket state.
        let writer = unsafe { &mut *active.writer };
        writer.state = WriterState::WebSocket;

        // NB4-10: Generate connection-scoped token for identity verification.
        let ws_token = NEXT_WS_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(ref mut active) = self.active_streaming_writer {
            active.ws_token = ws_token;
        }

        // Create WsConn BuchiPack with identity token.
        let ws_pack = Value::BuchiPack(vec![
            ("__ws_id".into(), Value::Str("__v4_websocket_conn".into())),
            ("__ws_token".into(), Value::Int(ws_token as i64)),
        ]);

        // Return Lax with the ws connection.
        Ok(Some(Signal::Value(Self::make_lax_ws_value(ws_pack))))
    }

    /// Build Lax[@(ws: WsConn)] with value.
    fn make_lax_ws_value(ws: Value) -> Value {
        let inner = Value::BuchiPack(vec![("ws".into(), ws)]);
        Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__value".into(), inner),
            ("__default".into(), Value::BuchiPack(vec![])),
            ("__type".into(), Value::Str("Lax".into())),
        ])
    }

    /// Build Lax empty for failed wsUpgrade.
    fn make_lax_ws_empty() -> Value {
        Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("__value".into(), Value::BuchiPack(vec![])),
            ("__default".into(), Value::BuchiPack(vec![])),
            ("__type".into(), Value::Str("Lax".into())),
        ])
    }

    /// `wsSend(ws, data)` → Unit (NET4-2e).
    ///
    /// Sends a WebSocket text or binary frame.
    /// - Str → text frame (opcode 0x1)
    /// - Bytes → binary frame (opcode 0x2)
    ///
    /// Server-to-client: MASK=0.
    fn eval_ws_send(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsSend: can only be called inside a 2-argument httpServe handler".into(),
            });
        }

        // Validate ws token.
        self.validate_ws_token(args, "wsSend")?;

        // Evaluate data argument.
        let data = match args.get(1) {
            Some(arg) => match self.eval_expr(arg)? {
                Signal::Value(v) => v,
                other => return Ok(Some(other)),
            },
            None => {
                return Err(RuntimeError {
                    message: "wsSend: missing argument 'data'".into(),
                });
            }
        };

        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &*active.writer };

        // Must be in WebSocket state.
        if writer.state != WriterState::WebSocket {
            return Err(RuntimeError {
                message: "wsSend: not in WebSocket state. Call wsUpgrade first.".into(),
            });
        }

        // Check if already closed.
        if active.ws_closed {
            return Err(RuntimeError {
                message: "wsSend: WebSocket connection is already closed.".into(),
            });
        }

        let stream = unsafe { &mut *active.stream };

        // Determine opcode and payload.
        let (opcode, payload): (u8, &[u8]) = match &data {
            Value::Str(s) => (Self::WS_OPCODE_TEXT, s.as_bytes()),
            Value::Bytes(b) => (Self::WS_OPCODE_BINARY, b.as_slice()),
            _ => {
                return Err(RuntimeError {
                    message: "wsSend: data must be Str (text frame) or Bytes (binary frame)".into(),
                });
            }
        };

        Self::write_ws_frame(stream, opcode, payload)?;

        Ok(Some(Signal::Value(Value::Unit)))
    }

    /// Write a WebSocket frame to the stream.
    /// Server-to-client: FIN=1, MASK=0.
    /// Uses vectored write (header on stack, payload direct).
    fn write_ws_frame(
        stream: &mut ConnStream,
        opcode: u8,
        payload: &[u8],
    ) -> Result<(), RuntimeError> {
        let payload_len = payload.len();

        // Build frame header on stack (max 10 bytes).
        let mut header = [0u8; 10];
        header[0] = 0x80 | opcode; // FIN=1, opcode
        let header_len;

        if payload_len < 126 {
            header[1] = payload_len as u8; // MASK=0
            header_len = 2;
        } else if payload_len <= 65535 {
            header[1] = 126;
            header[2] = (payload_len >> 8) as u8;
            header[3] = (payload_len & 0xFF) as u8;
            header_len = 4;
        } else {
            header[1] = 127;
            let len64 = payload_len as u64;
            header[2] = (len64 >> 56) as u8;
            header[3] = (len64 >> 48) as u8;
            header[4] = (len64 >> 40) as u8;
            header[5] = (len64 >> 32) as u8;
            header[6] = (len64 >> 24) as u8;
            header[7] = (len64 >> 16) as u8;
            header[8] = (len64 >> 8) as u8;
            header[9] = len64 as u8;
            header_len = 10;
        }

        // Vectored write: header + payload (no aggregate buffer).
        let bufs = [
            std::io::IoSlice::new(&header[..header_len]),
            std::io::IoSlice::new(payload),
        ];
        write_vectored_all(stream, &bufs)
    }

    /// `wsReceive(ws)` → `Lax[@(type: Str, data: Bytes)]` (NET4-2d).
    ///
    /// Receives the next WebSocket data frame.
    /// - ping: auto pong, advance to next frame
    /// - close: return Lax empty
    /// - text/binary: return @(type, data)
    /// - fragmented (FIN=0): protocol error → close
    /// - oversized (>16 MiB): close
    fn eval_ws_receive(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsReceive: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate ws token.
        self.validate_ws_token(args, "wsReceive")?;

        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &*active.writer };

        if writer.state != WriterState::WebSocket {
            return Err(RuntimeError {
                message: "wsReceive: not in WebSocket state. Call wsUpgrade first.".into(),
            });
        }

        if active.ws_closed {
            // Already closed — return Lax empty.
            return Ok(Some(Signal::Value(Self::make_lax_ws_frame_empty())));
        }

        let stream = unsafe { &mut *active.stream };

        // Loop to handle ping/pong transparently.
        loop {
            let frame = Self::read_ws_frame(stream)?;

            match frame {
                WsFrame::Data { opcode, payload } => {
                    let (type_str, data_val) = if opcode == Self::WS_OPCODE_TEXT {
                        // Text frames carry UTF-8: return Str so wsSend(ws, data) echoes as text.
                        let text = String::from_utf8(payload)
                            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                        ("text", Value::Str(text))
                    } else {
                        ("binary", Value::Bytes(payload))
                    };
                    let inner = Value::BuchiPack(vec![
                        ("type".into(), Value::Str(type_str.into())),
                        ("data".into(), data_val),
                    ]);
                    return Ok(Some(Signal::Value(Self::make_lax_ws_frame_value(inner))));
                }
                WsFrame::Ping { payload } => {
                    // Auto pong: send pong with same payload.
                    Self::write_ws_frame(stream, Self::WS_OPCODE_PONG, &payload)?;
                    // Continue to next frame.
                    continue;
                }
                WsFrame::Pong => {
                    // Unsolicited pong: ignore, continue.
                    continue;
                }
                WsFrame::Close { payload } => {
                    // v5 close code extraction (NET5-0d):
                    // - 0 bytes: no status code → wsCloseCode returns 1005, reply with empty close
                    // - 2+ bytes: extract 16-bit code, validate, echo code in reply
                    // - 1 byte or invalid code or invalid UTF-8 reason: protocol error → 1002
                    if payload.is_empty() {
                        // No status code: reply with empty close payload.
                        let _ = Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &[]);
                        if let Some(ref mut active) = self.active_streaming_writer {
                            active.ws_closed = true;
                            active.ws_close_code = 1005; // No Status Rcvd
                        }
                        return Ok(Some(Signal::Value(Self::make_lax_ws_frame_empty())));
                    } else if payload.len() == 1 {
                        // 1-byte close payload is malformed (RFC 6455 Section 5.5).
                        let close_payload = [0x03, 0xEA]; // 1002 protocol error
                        let _ = Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &close_payload);
                        if let Some(ref mut active) = self.active_streaming_writer {
                            active.ws_closed = true;
                            // ws_close_code NOT updated for protocol error
                        }
                        return Err(RuntimeError {
                            message:
                                "wsReceive: protocol error: malformed close frame (1-byte payload)"
                                    .into(),
                        });
                    } else {
                        // 2+ bytes: first 2 bytes are the close code (big-endian).
                        let code = ((payload[0] as u16) << 8) | (payload[1] as u16);
                        // Validate close code (RFC 6455 Section 7.4).
                        // 1000-1003: standard, 1007-1014: IANA-registered,
                        // 3000-4999: reserved for libraries/apps/private use.
                        let valid_code = matches!(code,
                            1000..=1003 | 1007..=1014 | 3000..=4999
                        );
                        if !valid_code {
                            let close_payload = [0x03, 0xEA]; // 1002
                            let _ =
                                Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &close_payload);
                            if let Some(ref mut active) = self.active_streaming_writer {
                                active.ws_closed = true;
                            }
                            return Err(RuntimeError {
                                message: format!(
                                    "wsReceive: protocol error: invalid close code {}",
                                    code
                                ),
                            });
                        }
                        // Validate reason UTF-8 if present.
                        if payload.len() > 2 && std::str::from_utf8(&payload[2..]).is_err() {
                            let close_payload = [0x03, 0xEA]; // 1002
                            let _ =
                                Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &close_payload);
                            if let Some(ref mut active) = self.active_streaming_writer {
                                active.ws_closed = true;
                            }
                            return Err(RuntimeError {
                                message: "wsReceive: protocol error: invalid UTF-8 in close reason"
                                    .into(),
                            });
                        }
                        // Valid close: echo the code in the reply.
                        let reply = [(code >> 8) as u8, (code & 0xFF) as u8];
                        let _ = Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &reply);
                        if let Some(ref mut active) = self.active_streaming_writer {
                            active.ws_closed = true;
                            active.ws_close_code = code as i64;
                        }
                        return Ok(Some(Signal::Value(Self::make_lax_ws_frame_empty())));
                    }
                }
                WsFrame::ProtocolError(msg) => {
                    // Send close frame with protocol error status code (1002).
                    let close_payload = [0x03, 0xEA]; // 1002 in big-endian
                    let _ = Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &close_payload);
                    if let Some(ref mut active) = self.active_streaming_writer {
                        active.ws_closed = true;
                    }
                    return Err(RuntimeError {
                        message: format!("wsReceive: protocol error: {}", msg),
                    });
                }
            }
        }
    }

    /// Build Lax[@(type, data)] with value.
    fn make_lax_ws_frame_value(inner: Value) -> Value {
        Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__value".into(), inner),
            ("__default".into(), Value::BuchiPack(vec![])),
            ("__type".into(), Value::Str("Lax".into())),
        ])
    }

    /// Build Lax empty for close / end of stream.
    fn make_lax_ws_frame_empty() -> Value {
        Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("__value".into(), Value::BuchiPack(vec![])),
            ("__default".into(), Value::BuchiPack(vec![])),
            ("__type".into(), Value::Str("Lax".into())),
        ])
    }

    /// Read exactly `count` bytes from a TcpStream.
    fn read_exact_bytes(stream: &mut ConnStream, count: usize) -> Result<Vec<u8>, RuntimeError> {
        use std::io::Read;
        let mut buf = vec![0u8; count];
        let mut pos = 0;
        while pos < count {
            match stream.read(&mut buf[pos..]) {
                Ok(0) => {
                    return Err(RuntimeError {
                        message: "wsReceive: connection closed unexpectedly".into(),
                    });
                }
                Ok(n) => pos += n,
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // Retry.
                    continue;
                }
                Err(e) => {
                    return Err(RuntimeError {
                        message: format!("wsReceive: read error: {}", e),
                    });
                }
            }
        }
        Ok(buf)
    }

    /// Read and parse one WebSocket frame from the stream (NET4-2c).
    fn read_ws_frame(stream: &mut ConnStream) -> Result<WsFrame, RuntimeError> {
        // Read first 2 bytes: FIN+opcode, MASK+payload_len7.
        let header = Self::read_exact_bytes(stream, 2)?;
        let byte0 = header[0];
        let byte1 = header[1];

        let fin = (byte0 & 0x80) != 0;
        let rsv = byte0 & 0x70;
        let opcode = byte0 & 0x0F;
        let masked = (byte1 & 0x80) != 0;
        let payload_len7 = (byte1 & 0x7F) as u64;

        // RSV bits must be 0 (no extensions in v4).
        if rsv != 0 {
            return Ok(WsFrame::ProtocolError("RSV bits must be 0".into()));
        }

        // NET4-2h: Fragmented frames (FIN=0) are not supported in v4.
        if !fin {
            return Ok(WsFrame::ProtocolError(
                "fragmented frames are not supported".into(),
            ));
        }

        // Continuation opcode (0x0) without fragmentation is also a protocol error.
        if opcode == 0x0 {
            return Ok(WsFrame::ProtocolError(
                "unexpected continuation frame".into(),
            ));
        }

        // NB4-11: Client-to-server frames MUST be masked (RFC 6455 Section 5.1).
        if !masked {
            return Ok(WsFrame::ProtocolError(
                "client frame must be masked (MASK=0 received)".into(),
            ));
        }

        // Determine actual payload length.
        let payload_len: u64 = if payload_len7 < 126 {
            payload_len7
        } else if payload_len7 == 126 {
            let ext = Self::read_exact_bytes(stream, 2)?;
            ((ext[0] as u64) << 8) | (ext[1] as u64)
        } else {
            // payload_len7 == 127
            let ext = Self::read_exact_bytes(stream, 8)?;
            let mut val: u64 = 0;
            for &b in &ext {
                val = (val << 8) | (b as u64);
            }
            // MSB must be 0 (unsigned).
            if val >> 63 != 0 {
                return Ok(WsFrame::ProtocolError(
                    "payload length MSB must be 0".into(),
                ));
            }
            val
        };

        // NET4-2h: Oversized payload check.
        if payload_len > Self::WS_MAX_PAYLOAD {
            return Ok(WsFrame::ProtocolError(format!(
                "payload too large ({} bytes, max {} bytes)",
                payload_len,
                Self::WS_MAX_PAYLOAD
            )));
        }

        // Read masking key (4 bytes) if masked.
        let mask_key = if masked {
            let key = Self::read_exact_bytes(stream, 4)?;
            Some([key[0], key[1], key[2], key[3]])
        } else {
            None
        };

        // Read payload.
        let mut payload = if payload_len > 0 {
            Self::read_exact_bytes(stream, payload_len as usize)?
        } else {
            Vec::new()
        };

        // NB6-6: Unmask payload in-place using word-at-a-time XOR.
        // Process 4 bytes at a time to eliminate modulo per byte.
        if let Some(key) = mask_key {
            let mask_word = u32::from_ne_bytes(key);
            let mut chunks = payload.chunks_exact_mut(4);
            for chunk in chunks.by_ref() {
                let word = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let unmasked = word ^ mask_word;
                chunk.copy_from_slice(&unmasked.to_ne_bytes());
            }
            // Handle remaining 1-3 bytes.
            for (i, byte) in chunks.into_remainder().iter_mut().enumerate() {
                *byte ^= key[i];
            }
        }

        // Dispatch by opcode.
        match opcode {
            0x1 | 0x2 => {
                // Text or binary data frame.
                Ok(WsFrame::Data { opcode, payload })
            }
            0x8 => {
                // Close frame. v5: carry raw payload for close code extraction.
                Ok(WsFrame::Close { payload })
            }
            0x9 => {
                // Ping.
                Ok(WsFrame::Ping { payload })
            }
            0xA => {
                // Pong (unsolicited).
                Ok(WsFrame::Pong)
            }
            _ => Ok(WsFrame::ProtocolError(format!(
                "unknown opcode 0x{:X}",
                opcode
            ))),
        }
    }

    /// `wsClose(ws)` → Unit (NET4-2f).
    ///
    /// Sends a close frame. Idempotent (second call is no-op).
    /// Handler return auto-close is handled in dispatch_request.
    /// `wsClose(ws)` or `wsClose(ws, code)` → Unit (NET4-2f, v5 revision).
    ///
    /// Sends a close frame. Optional close code (default 1000).
    /// v5: accepts an explicit close code in the range 1000-4999
    /// (excluding reserved codes 1004, 1005, 1006, 1015).
    /// Idempotent (second call is no-op).
    fn eval_ws_close(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsClose: can only be called inside a 2-argument httpServe handler".into(),
            });
        }

        // Validate ws token.
        self.validate_ws_token(args, "wsClose")?;

        // v5: Evaluate optional close code arg before taking borrows.
        let close_code: u16 = if let Some(code_arg) = args.get(1) {
            match self.eval_expr(code_arg)? {
                Signal::Value(Value::Int(n)) => {
                    // Validate close code per NET5-0d.
                    if !(1000..=4999).contains(&n) {
                        return Err(RuntimeError {
                            message: format!("wsClose: close code must be 1000-4999, got {}", n),
                        });
                    }
                    // Reserved codes that must not be sent.
                    if matches!(n, 1004 | 1005 | 1006 | 1015) {
                        return Err(RuntimeError {
                            message: format!(
                                "wsClose: close code {} is reserved and cannot be sent",
                                n
                            ),
                        });
                    }
                    n as u16
                }
                Signal::Value(v) => {
                    return Err(RuntimeError {
                        message: format!("wsClose: close code must be Int, got {}", v),
                    });
                }
                other => return Ok(Some(other)),
            }
        } else {
            1000 // default: Normal Closure
        };

        // Now take borrows after eval_expr is done.
        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &*active.writer };

        if writer.state != WriterState::WebSocket {
            return Err(RuntimeError {
                message: "wsClose: not in WebSocket state. Call wsUpgrade first.".into(),
            });
        }

        // Idempotent: no-op if already closed.
        if active.ws_closed {
            return Ok(Some(Signal::Value(Value::Unit)));
        }

        let stream = unsafe { &mut *active.stream };

        // Send close frame (opcode 0x8) with the specified close code.
        let close_payload = [(close_code >> 8) as u8, (close_code & 0xFF) as u8];
        let _ = Self::write_ws_frame(stream, Self::WS_OPCODE_CLOSE, &close_payload);

        // Mark as closed.
        if let Some(ref mut active) = self.active_streaming_writer {
            active.ws_closed = true;
        }

        Ok(Some(Signal::Value(Value::Unit)))
    }

    /// `wsCloseCode(ws)` → Int (v5 NET5-0d).
    ///
    /// Returns the close code received from the peer's close frame.
    /// - 0: no close frame received yet
    /// - 1005: close frame with no status code
    /// - 1000-4999: peer's close code
    fn eval_ws_close_code(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsCloseCode: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate ws token.
        self.validate_ws_token(args, "wsCloseCode")?;

        let active = self.active_streaming_writer.as_ref().unwrap();
        let writer = unsafe { &*active.writer };

        if writer.state != WriterState::WebSocket {
            return Err(RuntimeError {
                message: "wsCloseCode: not in WebSocket state. Call wsUpgrade first.".into(),
            });
        }

        let close_code = active.ws_close_code;
        Ok(Some(Signal::Value(Value::Int(close_code))))
    }

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

    fn eval_http_serve(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
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
                                    requested_protocol = Some(proto.clone());
                                }
                                _ => {
                                    let result = make_result_failure_msg(
                                        "ProtocolError",
                                        format!(
                                            "httpServe: protocol must be a Str, got {}",
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
                                tls_cert_path = Some(c);
                                tls_key_path = Some(k);
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

        // v6 NET6-1b / NET6-2a: Protocol validation.
        // HTTP/2 is opt-in. Unknown protocol values are rejected immediately.
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
            Some(other) => {
                let result = make_result_failure_msg(
                    "ProtocolError",
                    format!(
                        "httpServe: unknown protocol \"{}\". \
                         Supported values: \"h1.1\", \"h2\"",
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
                        super::net_transport::load_tls_config_h2(cert, key)
                    } else {
                        super::net_transport::load_tls_config(cert, key)
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
                            let mut tls_transport =
                                super::net_transport::TlsTransport::new(tls_conn, tcp_stream);
                            match super::net_transport::complete_tls_handshake(
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
                    ) => {
                        // We have a complete request head. Process body + handler.
                        // Advance round-robin past this connection.
                        poll_start = (idx + 1) % n;
                        processed_idx = Some(idx);

                        // ── Body reading + handler dispatch ──
                        // Set blocking timeout for body read (need full body)
                        let _ = conn.stream.set_read_timeout(Some(read_timeout));

                        let dispatch_result = self.dispatch_request(
                            conn,
                            &handler,
                            parsed_fields,
                            head_consumed,
                            content_length,
                            is_chunked,
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
        let result_inner = Value::BuchiPack(vec![
            ("ok".into(), Value::Bool(true)),
            ("requests".into(), Value::Int(request_count)),
        ]);
        let result = make_result_success(result_inner);
        Ok(Some(Signal::Value(make_fulfilled_async(result))))
    }

    /// NET6-2a/2b/2c: HTTP/2 serve loop (Interpreter reference implementation).
    ///
    /// Accepts TLS connections with ALPN h2 negotiation, validates the HTTP/2
    /// connection preface, then enters a frame-level protocol loop that:
    /// - Receives HEADERS/DATA frames and dispatches complete requests to the handler
    /// - Sends HPACK-encoded response HEADERS + DATA frames
    /// - Handles SETTINGS, PING, WINDOW_UPDATE, GOAWAY, RST_STREAM
    /// - Respects connection/stream flow control windows
    ///
    /// Design: serial handler dispatch (consistent with h1 path), but with
    /// stream multiplexing at the frame level within each connection.
    fn serve_h2(
        &mut self,
        listener: std::net::TcpListener,
        tls_config: std::sync::Arc<rustls::ServerConfig>,
        handler: super::value::FuncValue,
        max_requests: i64,
        _max_connections: usize,
        read_timeout: std::time::Duration,
    ) -> Result<Option<Signal>, RuntimeError> {
        use super::net_h2::*;

        let mut total_request_count: i64 = 0;
        let mut total_connection_count: i64 = 0;

        // Accept connections one at a time (serial, single-threaded model).
        // Each connection is fully serviced before accepting the next.
        // This is consistent with the Interpreter's blocking model while
        // supporting stream multiplexing within each connection.
        loop {
            if max_requests > 0 && total_request_count >= max_requests {
                break;
            }

            // Accept new connection (blocking).
            listener.set_nonblocking(false).map_err(|e| RuntimeError {
                message: format!("httpServe h2: failed to set blocking: {}", e),
            })?;
            let _ = listener.set_nonblocking(false);
            let (tcp_stream, peer_addr) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) => {
                    let result = make_result_failure_msg(
                        "AcceptError",
                        format!("httpServe h2: accept failed: {}", e),
                    );
                    return Ok(Some(Signal::Value(make_fulfilled_async(result))));
                }
            };

            // TLS handshake with ALPN.
            let _ = tcp_stream.set_nonblocking(false);
            let _ = tcp_stream.set_read_timeout(Some(read_timeout));
            let _ = tcp_stream.set_write_timeout(Some(read_timeout));

            let tls_conn = match rustls::ServerConnection::new(tls_config.clone()) {
                Ok(c) => c,
                Err(_) => continue, // TLS setup error, skip connection
            };
            let mut tls_transport = super::net_transport::TlsTransport::new(tls_conn, tcp_stream);
            match super::net_transport::complete_tls_handshake(&mut tls_transport, read_timeout) {
                Ok(()) => {}
                Err(_) => continue, // Handshake failure, skip connection
            }

            // Check ALPN negotiation result.
            let alpn = tls_transport.alpn_protocol();
            match alpn {
                Some(b"h2") => {} // HTTP/2 negotiated — proceed
                Some(b"http/1.1") | None => {
                    // Client didn't negotiate h2. Since protocol="h2" was explicitly
                    // requested, we don't fallback. Close the connection.
                    // This is the "no silent fallback" policy from NET_DESIGN.md.
                    continue;
                }
                Some(_) => continue, // Unknown ALPN protocol
            }

            // Read/write through a transport wrapper that implements Read+Write.
            let mut conn_stream = ConnStream::Tls(Box::new(tls_transport));
            let _ = conn_stream.set_read_timeout(Some(read_timeout));

            // Validate HTTP/2 connection preface from client.
            if let Err(_e) = validate_connection_preface(&mut conn_stream) {
                continue; // Invalid preface, close connection
            }

            // Initialize HTTP/2 connection state.
            let mut h2_conn = H2Connection::new();

            // Send our server SETTINGS frame.
            if let Err(_e) = send_settings(&mut conn_stream, &h2_conn.local_settings) {
                continue;
            }

            total_connection_count += 1;
            // NB6-47: emit connection count to stderr (side channel for benchmarks).
            // This keeps the public result pack contract clean (@(requests: Int) only).
            eprintln!("[h2-conn] {}", total_connection_count);

            // HTTP/2 connection frame loop.
            // Process frames until the connection is closed or max_requests is reached.
            let conn_result = self.h2_connection_loop(
                &mut conn_stream,
                &mut h2_conn,
                &handler,
                &peer_addr,
                max_requests,
                &mut total_request_count,
            );

            // Send GOAWAY on graceful shutdown.
            if !h2_conn.goaway_sent {
                let _ = send_goaway(
                    &mut conn_stream,
                    h2_conn.last_peer_stream_id,
                    0, // NO_ERROR
                    b"",
                );
            }

            if let Err(e) = conn_result {
                // Connection-level error — already handled via GOAWAY.
                // Log for debugging but don't propagate.
                let _ = e; // Suppress unused warning
            }

            // Check if we should stop accepting connections
            if max_requests > 0 && total_request_count >= max_requests {
                break;
            }
        }

        let result_inner = Value::BuchiPack(vec![
            ("ok".into(), Value::Bool(true)),
            ("requests".into(), Value::Int(total_request_count)),
        ]);
        let result = make_result_success(result_inner);
        Ok(Some(Signal::Value(make_fulfilled_async(result))))
    }

    /// HTTP/2 connection-level frame processing loop.
    ///
    /// Reads frames, processes them through the h2 state machine, and dispatches
    /// complete requests to the handler. Responses are sent as HEADERS + DATA frames.
    fn h2_connection_loop(
        &mut self,
        stream: &mut ConnStream,
        h2_conn: &mut super::net_h2::H2Connection,
        handler: &super::value::FuncValue,
        peer_addr: &std::net::SocketAddr,
        max_requests: i64,
        total_request_count: &mut i64,
    ) -> Result<(), RuntimeError> {
        use super::net_h2::*;

        let mut settings_ack_pending = false;
        // NB6-38: Reusable buffer for frame reading — avoids per-frame heap allocation
        // on the hot path. read_frame_reuse() writes into this buffer and returns a
        // borrowed slice, so no allocation occurs after the initial capacity is reached.
        let mut frame_buf: Vec<u8> = Vec::with_capacity(16_384);

        loop {
            // Check bounded shutdown
            if max_requests > 0 && *total_request_count >= max_requests {
                return Ok(());
            }

            // Read next frame (NB6-38: reuse frame_buf to avoid per-frame heap alloc)
            let (header, payload) = match read_frame_reuse(
                stream,
                h2_conn.local_settings.max_frame_size,
                &mut frame_buf,
            ) {
                Ok(frame) => frame,
                Err(H2Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof
                        || e.kind() == std::io::ErrorKind::ConnectionReset
                        || e.kind() == std::io::ErrorKind::BrokenPipe =>
                {
                    // Clean connection close
                    return Ok(());
                }
                Err(H2Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // Timeout — close connection
                    return Ok(());
                }
                Err(H2Error::Connection(error_code, ref msg)) => {
                    // Protocol error — send GOAWAY and close
                    let _ = send_goaway(
                        stream,
                        h2_conn.last_peer_stream_id,
                        error_code,
                        msg.as_bytes(),
                    );
                    h2_conn.goaway_sent = true;
                    return Ok(());
                }
                Err(_) => return Ok(()),
            };

            // Check if we need to send SETTINGS ACK after receiving client's SETTINGS
            if header.frame_type == super::net_h2::FRAME_SETTINGS
                && header.flags & super::net_h2::FLAG_ACK == 0
            {
                settings_ack_pending = true;
            }

            // Check for PING that needs response (NB6-38: minimal copy — PING is always 8 bytes)
            let is_ping_needing_ack = header.frame_type == super::net_h2::FRAME_PING
                && header.flags & super::net_h2::FLAG_ACK == 0;
            let ping_data = if is_ping_needing_ack {
                Some(payload.to_vec())
            } else {
                None
            };

            // Process frame through the h2 state machine
            let completed_request = match process_frame(h2_conn, &header, payload) {
                Ok(req) => req,
                Err(H2Error::Connection(error_code, ref msg)) => {
                    let _ = send_goaway(
                        stream,
                        h2_conn.last_peer_stream_id,
                        error_code,
                        msg.as_bytes(),
                    );
                    h2_conn.goaway_sent = true;
                    return Ok(());
                }
                Err(H2Error::Stream(stream_id, error_code, _)) => {
                    let _ = send_rst_stream(stream, stream_id, error_code);
                    continue;
                }
                Err(H2Error::Compression(ref msg)) => {
                    let _ = send_goaway(
                        stream,
                        h2_conn.last_peer_stream_id,
                        0x9, // COMPRESSION_ERROR
                        msg.as_bytes(),
                    );
                    h2_conn.goaway_sent = true;
                    return Ok(());
                }
                Err(H2Error::Io(_)) => return Ok(()),
            };

            // Send SETTINGS ACK if needed
            if settings_ack_pending {
                if send_settings_ack(stream).is_err() {
                    return Ok(());
                }
                settings_ack_pending = false;
            }

            // Send PING ACK if needed
            if ping_data
                .as_ref()
                .is_some_and(|data| send_ping_ack(stream, data).is_err())
            {
                return Ok(());
            }

            // If a request is complete, dispatch to handler
            if let Some((stream_id, h2_headers, body)) = completed_request {
                // NB6-42: Only send WINDOW_UPDATE when window drops below half of initial.
                // Avoids per-DATA-frame WINDOW_UPDATE overhead for small bodies.
                let data_received = body.len() as u32;
                if data_received > 0 {
                    let initial_window = DEFAULT_INITIAL_WINDOW_SIZE as i64;
                    // Connection window
                    if h2_conn.conn_recv_window < initial_window / 2 {
                        let replenish = initial_window - h2_conn.conn_recv_window;
                        let _ = send_window_update(stream, 0, replenish as u32);
                        h2_conn.conn_recv_window += replenish;
                    }
                    // Stream window
                    if let Some(s) = h2_conn.streams.get_mut(&stream_id)
                        && s.recv_window < initial_window / 2
                    {
                        let replenish = initial_window - s.recv_window;
                        let _ = send_window_update(stream, stream_id, replenish as u32);
                        s.recv_window += replenish;
                    }
                }

                // Convert h2 pseudo-headers + headers into request pack
                // NB6-36: pass actual stream_id so errors target the correct stream
                let (method, path, authority, regular_headers) =
                    match extract_request_fields_with_stream_id(&h2_headers, stream_id) {
                        Ok(fields) => fields,
                        Err(_) => {
                            let _ = send_rst_stream(stream, stream_id, 0x1); // PROTOCOL_ERROR
                            continue;
                        }
                    };

                // Parse query from path
                let (path_part, query_part) = match path.find('?') {
                    Some(pos) => (&path[..pos], &path[pos + 1..]),
                    None => (path.as_str(), ""),
                };

                // Build request pack for handler (h2 requests use 1-arg handler path).
                let mut request_fields: Vec<(String, Value)> = vec![
                    ("method".into(), Value::Str(method)),
                    ("path".into(), Value::Str(path_part.to_string())),
                    ("query".into(), Value::Str(query_part.to_string())),
                    (
                        "version".into(),
                        Value::BuchiPack(vec![
                            ("major".into(), Value::Int(2)),
                            ("minor".into(), Value::Int(0)),
                        ]),
                    ),
                ];

                // Convert h2 headers to the same format as h1
                let mut header_values: Vec<Value> = Vec::new();
                for (name, value) in &regular_headers {
                    header_values.push(Value::BuchiPack(vec![
                        ("name".into(), Value::Str(name.clone())),
                        ("value".into(), Value::Str(value.clone())),
                    ]));
                }
                // Add :authority as host header for compatibility
                if !authority.is_empty() {
                    header_values.push(Value::BuchiPack(vec![
                        ("name".into(), Value::Str("host".into())),
                        ("value".into(), Value::Str(authority.clone())),
                    ]));
                }
                request_fields.push(("headers".into(), Value::List(header_values)));

                // Body
                let raw_len = body.len();
                request_fields.push(("body".into(), make_span(0, raw_len)));
                request_fields.push(("bodyOffset".into(), Value::Int(0)));
                request_fields.push(("contentLength".into(), Value::Int(raw_len as i64)));
                request_fields.push(("raw".into(), Value::Bytes(body)));
                request_fields.push(("remoteHost".into(), Value::Str(peer_addr.ip().to_string())));
                request_fields.push(("remotePort".into(), Value::Int(peer_addr.port() as i64)));
                request_fields.push(("keepAlive".into(), Value::Bool(true)));
                request_fields.push(("chunked".into(), Value::Bool(false)));
                request_fields.push(("protocol".into(), Value::Str("h2".into())));

                let request_pack = Value::BuchiPack(request_fields);

                // Call handler with request pack (1-arg path for h2).
                let handler_result = self.call_function_with_values(handler, &[request_pack]);

                *total_request_count += 1;
                h2_conn.request_count += 1;

                // Extract response from handler result
                match handler_result {
                    Ok(response) => {
                        // Send h2 response
                        if self
                            .send_h2_response(stream, h2_conn, stream_id, &response)
                            .is_err()
                        {
                            // Write error — close connection
                            return Ok(());
                        }
                    }
                    Err(_) => {
                        // Handler error — send 500
                        let _ = send_response_headers(
                            stream,
                            &mut h2_conn.encoder,
                            stream_id,
                            500,
                            &[],
                            true,
                            h2_conn.peer_settings.max_frame_size,
                        );
                    }
                }

                // Mark stream as closed
                if let Some(s) = h2_conn.streams.get_mut(&stream_id) {
                    s.state = super::net_h2::StreamState::Closed;
                }

                // Clean up closed streams to prevent unbounded growth
                h2_conn
                    .streams
                    .retain(|_, s| s.state != super::net_h2::StreamState::Closed);
            }
        }
    }

    /// Send an HTTP/2 response (HEADERS + DATA frames) for a completed request.
    fn send_h2_response(
        &self,
        stream: &mut ConnStream,
        h2_conn: &mut super::net_h2::H2Connection,
        stream_id: u32,
        response: &Value,
    ) -> Result<(), RuntimeError> {
        use super::net_h2::*;

        let (status, headers, body_bytes) = match extract_response_fields(response) {
            Ok(fields) => fields,
            Err(msg) => {
                // Invalid response — send 500
                let _ = send_response_headers(
                    stream,
                    &mut h2_conn.encoder,
                    stream_id,
                    500,
                    &[],
                    true,
                    h2_conn.peer_settings.max_frame_size,
                );
                return Err(RuntimeError {
                    message: format!("httpServe h2: {}", msg),
                });
            }
        };

        let no_body =
            (100..200).contains(&status) || status == 204 || status == 205 || status == 304;

        if no_body || body_bytes.is_empty() {
            // Headers only, END_STREAM on HEADERS frame
            if let Err(e) = send_response_headers(
                stream,
                &mut h2_conn.encoder,
                stream_id,
                status as u16,
                &headers,
                true,
                h2_conn.peer_settings.max_frame_size,
            ) {
                return Err(RuntimeError {
                    message: format!("httpServe h2: failed to send headers: {}", e),
                });
            }
        } else {
            // Add content-length header
            let has_cl = headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-length"));
            let mut all_headers = headers.clone();
            if !has_cl {
                all_headers.push(("content-length".to_string(), body_bytes.len().to_string()));
            }

            // Send HEADERS (no END_STREAM yet)
            if let Err(e) = send_response_headers(
                stream,
                &mut h2_conn.encoder,
                stream_id,
                status as u16,
                &all_headers,
                false,
                h2_conn.peer_settings.max_frame_size,
            ) {
                return Err(RuntimeError {
                    message: format!("httpServe h2: failed to send headers: {}", e),
                });
            }

            // Send DATA with END_STREAM, respecting flow control windows.
            // We need mutable access to both conn_send_window and
            // stream.send_window. To satisfy the borrow checker, copy
            // the stream window out, call the function, then write it back.
            let mut stream_sw = h2_conn
                .streams
                .get(&stream_id)
                .map_or(i64::MAX, |s| s.send_window);
            let data_result = send_response_data(
                stream,
                stream_id,
                &body_bytes,
                true,
                h2_conn.peer_settings.max_frame_size,
                &mut h2_conn.conn_send_window,
                &mut stream_sw,
            );
            // Write back the updated stream send window.
            if let Some(s) = h2_conn.streams.get_mut(&stream_id) {
                s.send_window = stream_sw;
            }

            if let Err(e) = data_result {
                // Flow control window exhausted or I/O error.
                // In a full async implementation we would wait for WINDOW_UPDATE,
                // but this blocking interpreter cannot read frames during a
                // synchronous write path. Send RST_STREAM to cleanly abort the
                // stream so the peer does not hang waiting for the remaining body.
                let _ = send_rst_stream(stream, stream_id, ERROR_FLOW_CONTROL_ERROR);
                return Err(RuntimeError {
                    message: format!(
                        "httpServe h2: failed to send response body on stream {}: {}",
                        stream_id, e
                    ),
                });
            }
        }

        Ok(())
    }

    /// Try to read and parse a request head from a connection (non-blocking).
    /// Returns the parse state without modifying request_count.
    fn try_read_request(conn: &mut HttpConnection) -> ConnReadResult {
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
                        Some((consumed, cl, is_chunked))
                    } else {
                        None
                    }
                }
            };
            if let Some((consumed, cl, is_chunked)) = completion_info {
                match extract_result_value_owned(parse_result) {
                    Some(fields) => {
                        return ConnReadResult::Ready(fields, consumed, cl, is_chunked);
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
                            Some((consumed, cl, is_chunked))
                        } else {
                            None
                        }
                    }
                };
                match completion_info {
                    Some((consumed, cl, is_chunked)) => {
                        match extract_result_value_owned(parse_result) {
                            Some(fields) => ConnReadResult::Ready(fields, consumed, cl, is_chunked),
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
    fn dispatch_request(
        &mut self,
        conn: &mut HttpConnection,
        handler: &super::value::FuncValue,
        parsed_fields: Vec<(String, Value)>,
        head_consumed: usize,
        content_length: i64,
        is_chunked: bool,
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
            let mut body_state = RequestBodyState::new(is_chunked, content_length, leftover);

            // Build request pack for handler (head only, body = empty span).
            let mut request_fields: Vec<(String, Value)> = Vec::new();
            request_fields.push(("raw".into(), Value::Bytes(raw_bytes)));

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
                Value::Str(conn.peer_addr.ip().to_string()),
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
                Value::Str("__v4_body_stream".into()),
            ));
            // NB4-7: Request-scoped token for identity verification.
            request_fields.push((
                "__body_token".into(),
                Value::Int(body_state.request_token as i64),
            ));

            let request_pack = Value::BuchiPack(request_fields);

            // Create writer BuchiPack with sentinel for identification.
            let writer_pack = Value::BuchiPack(vec![(
                "__writer_id".into(),
                Value::Str("__v3_streaming_writer".into()),
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
                    Value::BuchiPack(vec![
                        ("status".into(), Value::Int(200)),
                        ("headers".into(), Value::List(vec![])),
                        ("body".into(), Value::Str(String::new())),
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
            request_fields.push(("raw".into(), Value::Bytes(raw_bytes)));

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
                Value::Str(conn.peer_addr.ip().to_string()),
            ));
            request_fields.push((
                "remotePort".into(),
                Value::Int(conn.peer_addr.port() as i64),
            ));
            request_fields.push(("keepAlive".into(), Value::Bool(keep_alive)));
            request_fields.push(("chunked".into(), Value::Bool(is_request_chunked)));

            let request_pack = Value::BuchiPack(request_fields);

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
        // 16 legacy + 3 HTTP v1 + 1 HTTP v2 + 4 HTTP v3 + 6 HTTP v4 + 1 v5 = 31
        assert_eq!(NET_SYMBOLS.len(), 31);
        assert!(NET_SYMBOLS.contains(&"dnsResolve"));
        assert!(NET_SYMBOLS.contains(&"httpServe"));
        assert!(NET_SYMBOLS.contains(&"httpParseRequestHead"));
        assert!(NET_SYMBOLS.contains(&"wsCloseCode"));
        assert!(NET_SYMBOLS.contains(&"httpEncodeResponse"));
        assert!(NET_SYMBOLS.contains(&"readBody"));
        // v3 streaming
        assert!(NET_SYMBOLS.contains(&"startResponse"));
        assert!(NET_SYMBOLS.contains(&"writeChunk"));
        assert!(NET_SYMBOLS.contains(&"endResponse"));
        assert!(NET_SYMBOLS.contains(&"sseEvent"));
        // v4 request body streaming
        assert!(NET_SYMBOLS.contains(&"readBodyChunk"));
        assert!(NET_SYMBOLS.contains(&"readBodyAll"));
        // v4 WebSocket
        assert!(NET_SYMBOLS.contains(&"wsUpgrade"));
        assert!(NET_SYMBOLS.contains(&"wsSend"));
        assert!(NET_SYMBOLS.contains(&"wsReceive"));
        assert!(NET_SYMBOLS.contains(&"wsClose"));
    }

    // ── WebSocket tests ──

    #[test]
    fn test_ws_accept_computation() {
        // RFC 6455 Section 4.2.2 example:
        // Key: "dGhlIHNhbXBsZSBub25jZQ==" → Accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        let accept = Interpreter::compute_ws_accept("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn test_ws_frame_write() {
        use std::io::Read;
        // Create a pair of connected streams to test frame write.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let (server_tcp, _) = listener.accept().unwrap();
        let mut server = ConnStream::Plain(server_tcp);

        // Write a text frame "Hello" from server to client.
        Interpreter::write_ws_frame(&mut server, 0x1, b"Hello").unwrap();

        // Read and verify the frame.
        let mut buf = [0u8; 64];
        let n = client.read(&mut buf).unwrap();
        assert!(n >= 7);
        // byte 0: FIN=1, opcode=0x1 → 0x81
        assert_eq!(buf[0], 0x81);
        // byte 1: MASK=0, len=5 → 0x05
        assert_eq!(buf[1], 0x05);
        // payload
        assert_eq!(&buf[2..7], b"Hello");
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
        assert!(get_bool(inner, "complete"));
        assert_eq!(get_int(inner, "contentLength"), 5);
        // bodyOffset should equal consumed (end of headers)
        let consumed = get_int(inner, "consumed");
        assert!(consumed > 0);
        assert_eq!(get_int(inner, "bodyOffset"), consumed);
    }

    #[test]
    fn test_parse_incomplete() {
        let raw = b"GET / HTTP/1.1\r\nHost: local";
        let result = parse_request_head(raw);
        let inner = extract_result_inner(&result);
        assert!(!get_bool(inner, "complete"));
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
        let query = match inner.iter().find(|(k, _)| k == "query").map(|(_, v)| v) {
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
            Value::BuchiPack(f) => {
                !matches!(f.iter().find(|(k, _)| k == "throw"), Some((_, Value::Unit)))
            }
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
    fn test_parse_content_length_max_safe_integer_boundary() {
        // Exactly Number.MAX_SAFE_INTEGER = 9007199254740991 (2^53 - 1) — should succeed.
        // This is the cross-backend upper limit (JS Number precision boundary).
        let raw =
            b"POST /data HTTP/1.1\r\nContent-Length: 9007199254740991\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 9_007_199_254_740_991);
    }

    #[test]
    fn test_parse_content_length_max_safe_integer_plus_one() {
        // Number.MAX_SAFE_INTEGER + 1 = 9007199254740992 — must be rejected.
        // Beyond this value, JS Number loses precision, breaking cross-backend parity.
        let raw =
            b"POST /data HTTP/1.1\r\nContent-Length: 9007199254740992\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_content_length_i64_max_rejected() {
        // i64::MAX = 9223372036854775807 — exceeds MAX_SAFE_INTEGER, must be rejected.
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 9223372036854775807\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    #[test]
    fn test_parse_content_length_i64_max_plus_one() {
        // i64::MAX + 1 = 9223372036854775808 — must be rejected.
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 9223372036854775808\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("invalid Content-Length"));
    }

    // ── Content-Length leading-zero tests (NB-20 parity fix) ──

    #[test]
    fn test_parse_content_length_leading_zeros_simple() {
        // "007" should be accepted as 7 (RFC 9110: Content-Length = 1*DIGIT, leading zeros valid).
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 007\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 7);
    }

    #[test]
    fn test_parse_content_length_leading_zeros_17_digits() {
        // "00000000000000005" (17 chars) should be accepted as 5.
        // JS must strip leading zeros before length check for parity.
        let raw =
            b"POST /data HTTP/1.1\r\nContent-Length: 00000000000000005\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 5);
    }

    #[test]
    fn test_parse_content_length_all_zeros_long() {
        // "00000000000000000" (17 zeros) should be accepted as 0.
        let raw =
            b"POST /data HTTP/1.1\r\nContent-Length: 00000000000000000\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 0);
    }

    #[test]
    fn test_parse_content_length_leading_zeros_0042() {
        // "0042" should be accepted as 42.
        let raw = b"POST /data HTTP/1.1\r\nContent-Length: 0042\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert_eq!(get_int(inner, "contentLength"), 42);
    }

    #[test]
    fn test_parse_content_length_leading_zeros_over_max_safe() {
        // Leading zeros + value > MAX_SAFE_INTEGER must still be rejected.
        // "009007199254740992" = 9007199254740992 > MAX_SAFE_INTEGER
        let raw =
            b"POST /data HTTP/1.1\r\nContent-Length: 009007199254740992\r\nHost: localhost\r\n\r\n";
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

    // ── NB-7: header name/value length limits ──

    #[test]
    fn test_encode_header_name_exceeds_limit() {
        let long_name = "X".repeat(8193);
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str(long_name)),
                    ("value".into(), Value::Str("ok".into())),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(
            get_failure_message(&result).contains("name exceeds 8192 bytes"),
            "Expected name length error, got: {}",
            get_failure_message(&result)
        );
    }

    #[test]
    fn test_encode_header_value_exceeds_limit() {
        let long_value = "V".repeat(65537);
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("X-Data".into())),
                    ("value".into(), Value::Str(long_value)),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(is_result_failure(&result));
        assert!(
            get_failure_message(&result).contains("value exceeds 65536 bytes"),
            "Expected value length error, got: {}",
            get_failure_message(&result)
        );
    }

    #[test]
    fn test_encode_header_name_at_limit_ok() {
        let name = "X".repeat(8192);
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str(name)),
                    ("value".into(), Value::Str("ok".into())),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
    }

    #[test]
    fn test_encode_header_value_at_limit_ok() {
        let value = "V".repeat(65536);
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::BuchiPack(vec![
                    ("name".into(), Value::Str("X-Data".into())),
                    ("value".into(), Value::Str(value)),
                ])]),
            ),
            ("body".into(), Value::Str(String::new())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
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
        let result = make_result_success(Value::BuchiPack(vec![("ok".into(), Value::Bool(true))]));
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

    // ── NB-23: Multiple header span verification ──

    /// Helper to extract span (start, len) from a header entry.
    fn get_header_span(headers: &[Value], idx: usize, field: &str) -> (i64, i64) {
        let entry = match &headers[idx] {
            Value::BuchiPack(f) => f,
            _ => panic!("header[{}] is not BuchiPack", idx),
        };
        let span = match entry.iter().find(|(k, _)| k == field) {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("header[{}].{} not found", idx, field),
        };
        let start = match span.iter().find(|(k, _)| k == "start") {
            Some((_, Value::Int(n))) => *n,
            _ => panic!("no start"),
        };
        let len = match span.iter().find(|(k, _)| k == "len") {
            Some((_, Value::Int(n))) => *n,
            _ => panic!("no len"),
        };
        (start, len)
    }

    #[test]
    fn test_parse_multiple_headers_span() {
        // "GET / HTTP/1.1\r\nHost: example.com\r\nContent-Type: text/plain\r\nX-Custom: value\r\n\r\n"
        let raw = b"GET / HTTP/1.1\r\nHost: example.com\r\nContent-Type: text/plain\r\nX-Custom: value\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);

        // Extract headers list
        let headers = match inner.iter().find(|(k, _)| k == "headers") {
            Some((_, Value::List(h))) => h,
            _ => panic!("no headers list"),
        };
        assert_eq!(headers.len(), 3, "expected 3 headers");

        // Verify each header's name/value span against raw bytes.
        // Header 0: "Host" / "example.com"
        let (name_start, name_len) = get_header_span(headers, 0, "name");
        assert_eq!(
            &raw[name_start as usize..(name_start + name_len) as usize],
            b"Host"
        );
        let (val_start, val_len) = get_header_span(headers, 0, "value");
        assert_eq!(
            &raw[val_start as usize..(val_start + val_len) as usize],
            b"example.com"
        );

        // Header 1: "Content-Type" / "text/plain"
        let (name_start, name_len) = get_header_span(headers, 1, "name");
        assert_eq!(
            &raw[name_start as usize..(name_start + name_len) as usize],
            b"Content-Type"
        );
        let (val_start, val_len) = get_header_span(headers, 1, "value");
        assert_eq!(
            &raw[val_start as usize..(val_start + val_len) as usize],
            b"text/plain"
        );

        // Header 2: "X-Custom" / "value"
        let (name_start, name_len) = get_header_span(headers, 2, "name");
        assert_eq!(
            &raw[name_start as usize..(name_start + name_len) as usize],
            b"X-Custom"
        );
        let (val_start, val_len) = get_header_span(headers, 2, "value");
        assert_eq!(
            &raw[val_start as usize..(val_start + val_len) as usize],
            b"value"
        );
    }

    #[test]
    fn test_parse_single_header_span() {
        let raw = b"GET / HTTP/1.1\r\nAccept: */*\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);

        let headers = match inner.iter().find(|(k, _)| k == "headers") {
            Some((_, Value::List(h))) => h,
            _ => panic!("no headers list"),
        };
        assert_eq!(headers.len(), 1);

        let (name_start, name_len) = get_header_span(headers, 0, "name");
        assert_eq!(
            &raw[name_start as usize..(name_start + name_len) as usize],
            b"Accept"
        );
        let (val_start, val_len) = get_header_span(headers, 0, "value");
        assert_eq!(
            &raw[val_start as usize..(val_start + val_len) as usize],
            b"*/*"
        );
    }

    #[test]
    fn test_parse_no_headers_empty_list() {
        // Minimal valid HTTP request with no headers (just terminator).
        let raw = b"GET / HTTP/1.1\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);

        let headers = match inner.iter().find(|(k, _)| k == "headers") {
            Some((_, Value::List(h))) => h,
            _ => panic!("no headers list"),
        };
        assert_eq!(headers.len(), 0);
    }

    // ── NB-24: HTTP version validation ──

    #[test]
    fn test_parse_http_version_1_0_accepted() {
        let raw = b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);

        // version.minor should be 0
        let version = match inner.iter().find(|(k, _)| k == "version") {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("no version"),
        };
        assert!(matches!(
            version.iter().find(|(k, _)| k == "minor"),
            Some((_, Value::Int(0)))
        ));
    }

    #[test]
    fn test_parse_http_version_1_1_accepted() {
        let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);

        let version = match inner.iter().find(|(k, _)| k == "version") {
            Some((_, Value::BuchiPack(f))) => f,
            _ => panic!("no version"),
        };
        assert!(matches!(
            version.iter().find(|(k, _)| k == "minor"),
            Some((_, Value::Int(1)))
        ));
    }

    #[test]
    fn test_parse_http_version_alpha_rejected() {
        // "HTTP/a.b" — httparse rejects non-digit version components
        let raw = b"GET / HTTP/a.b\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("Malformed"));
    }

    #[test]
    fn test_parse_http_version_multi_digit_rejected() {
        // "HTTP/12.34" — httparse rejects multi-digit version numbers
        let raw = b"GET / HTTP/12.34\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("Malformed"));
    }

    #[test]
    fn test_parse_http_version_2_0_rejected() {
        // "HTTP/2.0" — httparse rejects major version != 1
        let raw = b"GET / HTTP/2.0\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("Malformed"));
    }

    #[test]
    fn test_parse_http_version_1_9_rejected() {
        // "HTTP/1.9" — httparse only accepts HTTP/1.0 and HTTP/1.1
        let raw = b"GET / HTTP/1.9\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("Malformed"));
    }

    #[test]
    fn test_parse_http_version_0_9_rejected() {
        // "HTTP/0.9" — httparse rejects major version != 1
        let raw = b"GET / HTTP/0.9\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(is_result_failure(&result));
        assert!(get_failure_message(&result).contains("Malformed"));
    }

    // ── NB-25: Multiple header encode verification ──

    #[test]
    fn test_encode_multiple_headers() {
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("Content-Type".into())),
                        ("value".into(), Value::Str("application/json".into())),
                    ]),
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("X-Request-Id".into())),
                        ("value".into(), Value::Str("abc-123".into())),
                    ]),
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("Cache-Control".into())),
                        ("value".into(), Value::Str("no-cache".into())),
                    ]),
                ]),
            ),
            ("body".into(), Value::Str("{\"ok\":true}".into())),
        ]);
        let result = encode_response(&response);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
            Some((_, Value::Bytes(b))) => b.clone(),
            _ => panic!("no bytes"),
        };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(text.contains("X-Request-Id: abc-123\r\n"));
        assert!(text.contains("Cache-Control: no-cache\r\n"));
        assert!(text.contains("Content-Length: 11\r\n"));
        assert!(text.ends_with("\r\n\r\n{\"ok\":true}"));
    }

    #[test]
    fn test_encode_multiple_headers_order_preserved() {
        // Headers should appear in the order provided by the user.
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("X-First".into())),
                        ("value".into(), Value::Str("1".into())),
                    ]),
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("X-Second".into())),
                        ("value".into(), Value::Str("2".into())),
                    ]),
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("X-Third".into())),
                        ("value".into(), Value::Str("3".into())),
                    ]),
                ]),
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

        // Find positions to verify ordering
        let pos_first = text.find("X-First: 1\r\n").expect("X-First missing");
        let pos_second = text.find("X-Second: 2\r\n").expect("X-Second missing");
        let pos_third = text.find("X-Third: 3\r\n").expect("X-Third missing");
        assert!(
            pos_first < pos_second && pos_second < pos_third,
            "Headers not in order: first={}, second={}, third={}",
            pos_first,
            pos_second,
            pos_third
        );
    }

    #[test]
    fn test_encode_duplicate_header_names_preserved() {
        // Multiple headers with the same name should all appear (e.g. Set-Cookie).
        let response = Value::BuchiPack(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("Set-Cookie".into())),
                        ("value".into(), Value::Str("a=1".into())),
                    ]),
                    Value::BuchiPack(vec![
                        ("name".into(), Value::Str("Set-Cookie".into())),
                        ("value".into(), Value::Str("b=2".into())),
                    ]),
                ]),
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
        assert!(text.contains("Set-Cookie: a=1\r\n"));
        assert!(text.contains("Set-Cookie: b=2\r\n"));
        assert_eq!(text.matches("Set-Cookie").count(), 2);
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
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
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
        std::io::Write::write_all(&mut client, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();

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
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "Expected 200 OK, got: {}",
            response_str
        );
        assert!(
            response_str.contains("Hello from Taida!"),
            "Expected body in response"
        );

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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
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
        std::io::Write::write_all(
            &mut client,
            b"POST /data?key=val HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello",
        )
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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
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
            let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
            std::io::Write::write_all(&mut client, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
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
        assert!(
            resp.contains("400 Bad Request"),
            "Expected 400, got: {}",
            resp
        );

        server_handle.join().unwrap();
    }

    #[test]
    fn test_http_serve_missing_args() {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        let result = interp.try_net_func("httpServe", &[]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("missing argument 'port'")
        );
    }

    #[test]
    fn test_http_serve_missing_handler() {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        let args = vec![Expr::IntLit(8080, dummy_span())];
        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("missing argument 'handler'")
        );
    }

    #[test]
    fn test_http_serve_port_validation() {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("split-ok"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Send head in two fragments with a small delay between them
        std::io::Write::write_all(&mut client, b"GET / HT").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::io::Write::write_all(&mut client, b"TP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("body-ok"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("should-not-reach"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

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
    fn test_http_serve_eof_during_head_does_not_count_request() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18800);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("idle-ok"),
                Expr::IntLit(1, dummy_span()), // maxRequests=1
                Expr::IntLit(3000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connect and immediately close (EOF before any HTTP data).
        // This idle connection should NOT consume the request budget.
        let client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        drop(client); // close immediately

        std::thread::sleep(std::time::Duration::from_millis(200));

        // Now send a real request — this should succeed and consume the budget.
        let mut real = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = real.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        let req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let _ = std::io::Write::write_all(&mut real, req);
        let mut resp = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut real, &mut buf) {
                Ok(0) => break,
                Ok(n) => resp.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(
            resp_str.contains("200 OK"),
            "real request after idle close should get 200, got: {}",
            resp_str
        );

        // Server should terminate because maxRequests=1 is now reached.
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                // Only the real request counted, not the idle close.
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
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("should-not-reach"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(3000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

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

    /// NB-3: Content-Length under 1 MiB but head + body exceeds 1 MiB → 413
    /// The early reject condition must be `head_consumed + content_length > MAX_REQUEST_BUF`,
    /// not just `content_length > MAX_REQUEST_BUF`.
    #[test]
    fn test_nb3_head_plus_body_exceeds_limit_returns_413() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18900);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("should not reach"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Craft a request where Content-Length < MAX_REQUEST_BUF (1 MiB)
        // but head_consumed + Content-Length > MAX_REQUEST_BUF.
        // Header is ~60 bytes, so CL = 1048576 - 10 = 1048566 (under 1 MiB).
        // head (~60) + 1048566 > 1048576 → must trigger 413.
        let cl_value = 1_048_576usize - 10; // just under 1 MiB
        let request = format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            cl_value
        );

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = client.set_write_timeout(Some(std::time::Duration::from_secs(3)));
        std::io::Write::write_all(&mut client, request.as_bytes()).unwrap();

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

        let resp = String::from_utf8_lossy(&response);
        assert!(
            resp.contains("413 Content Too Large"),
            "NB-3: head + body > 1 MiB should get 413, got: {}",
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

    /// NB-3: Content-Length that exactly fits (head + body == MAX_REQUEST_BUF) → 200 OK, not 413
    #[test]
    fn test_nb3_head_plus_body_exactly_fits_returns_200() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18910);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        // Pre-calculate the header size to set Content-Length so head + body == 1 MiB exactly.
        // "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: NNNNNN\r\n\r\n"
        // We need to know consumed size; build the header template first.
        let header_template = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: ";
        let header_suffix = "\r\n\r\n";
        // CL digits: we'll target ~6 digits. head_consumed = template + digits + suffix
        // Try CL = 1048576 - 62 = 1048514 (6 digits). head = 48 + 7 + 4 = 59 => 59 + 1048514 = 1048573 < 1048576, fits.
        // Actually compute: template.len() = 48, digits of CL, suffix.len() = 4
        // Let's just compute iteratively.
        let max = 1_048_576usize;
        let template_len = header_template.len() + header_suffix.len(); // 48 + 4 = 52
        // head_consumed = template_len + cl_digits_len
        // We need head_consumed + cl_value == max
        // cl_value = max - head_consumed = max - template_len - cl_digits_len
        // For 6-digit CL: cl = max - 52 - 6 = 1048518, which is 7 digits → contradiction
        // For 7-digit CL: cl = max - 52 - 7 = 1048517, which is 7 digits ✓
        let cl_digits = 7;
        let cl_value = max - template_len - cl_digits;
        assert_eq!(
            cl_value.to_string().len(),
            cl_digits,
            "CL digit count mismatch"
        );

        let request_head = format!("{}{}{}", header_template, cl_value, header_suffix);
        let head_len = request_head.len();
        assert_eq!(
            head_len + cl_value,
            max,
            "head + body must equal MAX_REQUEST_BUF"
        );

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("ok"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = client.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        // Send head + exactly cl_value bytes of body
        std::io::Write::write_all(&mut client, request_head.as_bytes()).unwrap();
        let body = vec![b'A'; cl_value];
        std::io::Write::write_all(&mut client, &body).unwrap();

        // Read response
        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));
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

        let resp = String::from_utf8_lossy(&response);
        assert!(
            resp.contains("200 OK"),
            "NB-3: head + body == 1 MiB should get 200, got: {}",
            resp
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NB-28: Verify that timeoutMs causes the server to close an idle connection
    /// (connects but sends no data). With the "idle = no budget" rule, this idle
    /// connection should be cleanly closed without a 400 and without consuming
    /// the request budget.
    #[test]
    fn test_nb28_timeout_closes_connection() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(18920);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("timeout-ok"),
                Expr::IntLit(1, dummy_span()),   // maxRequests=1
                Expr::IntLit(500, dummy_span()), // timeoutMs=500 (short timeout)
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connect but send NO data — the server should timeout after ~500ms
        let start = std::time::Instant::now();
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Set a generous read timeout on the client side so we don't hang forever
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));

        // Wait for the server to close the connection (clean close, no 400)
        let mut response = Vec::new();
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
        let elapsed = start.elapsed();

        // Verify: idle connection with no data gets clean close (no 400)
        assert!(
            response.is_empty(),
            "NB-28: idle connection (no data) should get clean close, got: {}",
            String::from_utf8_lossy(&response)
        );

        // Verify: elapsed time should be at least ~400ms (timeout was 500ms)
        // but not more than 3s (proving it was the timeout, not client-side timeout)
        assert!(
            elapsed.as_millis() >= 400,
            "NB-28: elapsed {}ms is too short — timeout did not fire",
            elapsed.as_millis()
        );
        assert!(
            elapsed.as_millis() < 3000,
            "NB-28: elapsed {}ms is too long — timeout should be ~500ms",
            elapsed.as_millis()
        );

        // Idle close did not consume budget. Send a real request to terminate the server.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut real = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = real.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        let req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let _ = std::io::Write::write_all(&mut real, req);
        let mut resp2 = Vec::new();
        let mut buf2 = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut real, &mut buf2) {
                Ok(0) => break,
                Ok(n) => resp2.extend_from_slice(&buf2[..n]),
                Err(_) => break,
            }
        }
        let real_resp = String::from_utf8_lossy(&resp2);
        assert!(
            real_resp.contains("200 OK"),
            "NB-28: real request after idle timeout should get 200, got: {}",
            real_resp
        );

        // Verify: server terminates successfully (maxRequests=1 reached by real request)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                // Only the real request counted, not the idle timeout.
                assert_eq!(get_field_int(inner, "requests"), Some(1));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // ── NB-26: Content-Length absent GET should have contentLength=0 ──

    #[test]
    fn test_nb26_get_without_content_length_has_cl_zero() {
        // A GET request with no Content-Length header.
        // The parser must default contentLength to 0.
        let raw = b"GET /hello HTTP/1.1\r\nHost: localhost\r\nAccept: */*\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(!is_result_failure(&result));
        let inner = extract_result_inner(&result);
        assert!(get_bool(inner, "complete"));
        // contentLength must be 0 when Content-Length header is absent
        assert_eq!(
            get_int(inner, "contentLength"),
            0,
            "GET without Content-Length header must have contentLength=0"
        );
    }

    // ── NB-27: empty path parse ──

    #[test]
    fn test_nb27_empty_path_parse() {
        // "GET  HTTP/1.1\r\n..." — double space means the path token is empty.
        // httparse treats this as malformed because it cannot find a valid URI
        // between the method and version.
        let raw = b"GET  HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        // httparse rejects double-space as a malformed request line → ParseError.
        assert!(
            is_result_failure(&result),
            "NB-27: double-space path should be rejected as malformed, got success"
        );
        let msg = get_failure_message(&result);
        assert!(
            msg.contains("Malformed"),
            "NB-27: expected Malformed error, got: {}",
            msg
        );
    }

    // ── NB-29: sentinel shadow by unmold ──

    #[test]
    fn test_nb29_sentinel_shadow_by_unmold() {
        // Simulates: >>> taida-lang/net => @(httpServe)
        //            someResult ]=> httpServe
        // After unmold, httpServe is overwritten with a non-sentinel value.
        // try_net_func must return None (sentinel guard blocks dispatch).
        let mut interp = Interpreter::new();

        // Step 1: Set up sentinel (as if imported via >>> taida-lang/net)
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        // Verify sentinel is active
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args);
        assert!(
            result.is_err(),
            "Before shadow: sentinel should be active (httpServe requires args)"
        );

        // Step 2: Simulate unmold shadow (]=> httpServe overwrites with a value)
        interp.env.define_force("httpServe", Value::Int(99));

        // Step 3: try_net_func must return None — sentinel is gone
        let result = interp.try_net_func("httpServe", &args).unwrap();
        assert!(
            result.is_none(),
            "After unmold shadow: sentinel is gone, try_net_func must return None"
        );
    }

    // ── readBody tests ──

    #[test]
    fn test_read_body_content_length() {
        // Build a fake request pack with raw bytes containing a body
        let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\nhello".to_vec();
        let body_start = 35i64; // "hello" starts at offset 35
        let body_len = 5i64;
        let req = Value::BuchiPack(vec![
            ("raw".into(), Value::Bytes(raw)),
            (
                "body".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(body_start)),
                    ("len".into(), Value::Int(body_len)),
                ]),
            ),
        ]);
        let result = eval_read_body(&req).unwrap();
        assert_eq!(result, Value::Bytes(b"hello".to_vec()));
    }

    #[test]
    fn test_read_body_no_body() {
        // body.len == 0 should return empty Bytes
        let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec();
        let req = Value::BuchiPack(vec![
            ("raw".into(), Value::Bytes(raw)),
            (
                "body".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(35)),
                    ("len".into(), Value::Int(0)),
                ]),
            ),
        ]);
        let result = eval_read_body(&req).unwrap();
        assert_eq!(result, Value::Bytes(vec![]));
    }

    #[test]
    fn test_read_body_missing_raw() {
        // Request pack without 'raw' field should produce RuntimeError
        let req = Value::BuchiPack(vec![(
            "body".into(),
            Value::BuchiPack(vec![
                ("start".into(), Value::Int(0)),
                ("len".into(), Value::Int(5)),
            ]),
        )]);
        let result = eval_read_body(&req);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("raw"));
    }

    #[test]
    fn test_read_body_not_buchipack() {
        // Argument that is not a BuchiPack should produce RuntimeError
        let req = Value::Int(42);
        let result = eval_read_body(&req);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("request pack"));
    }

    // ── determine_keep_alive unit tests (NET2-1a/1b/1c) ──

    #[test]
    fn test_keep_alive_http11_default() {
        // HTTP/1.1 without Connection header → keep-alive
        let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let headers = vec![Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)), // "Host"
                    ("len".into(), Value::Int(4)),
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(22)), // "localhost"
                    ("len".into(), Value::Int(9)),
                ]),
            ),
        ])];
        assert!(determine_keep_alive(raw, &headers, 1));
    }

    #[test]
    fn test_keep_alive_http11_connection_close() {
        // HTTP/1.1 with Connection: close → not keep-alive
        let raw = b"GET / HTTP/1.1\r\nConnection: close\r\nHost: localhost\r\n\r\n";
        let headers = vec![
            Value::BuchiPack(vec![
                (
                    "name".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(16)),
                        ("len".into(), Value::Int(10)), // "Connection"
                    ]),
                ),
                (
                    "value".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(28)),
                        ("len".into(), Value::Int(5)), // "close"
                    ]),
                ),
            ]),
            Value::BuchiPack(vec![
                (
                    "name".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(35)),
                        ("len".into(), Value::Int(4)), // "Host"
                    ]),
                ),
                (
                    "value".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(41)),
                        ("len".into(), Value::Int(9)), // "localhost"
                    ]),
                ),
            ]),
        ];
        assert!(!determine_keep_alive(raw, &headers, 1));
    }

    #[test]
    fn test_keep_alive_http10_default() {
        // HTTP/1.0 without Connection header → not keep-alive
        let raw = b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n";
        let headers = vec![Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(4)), // "Host"
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(22)),
                    ("len".into(), Value::Int(9)), // "localhost"
                ]),
            ),
        ])];
        assert!(!determine_keep_alive(raw, &headers, 0));
    }

    #[test]
    fn test_keep_alive_http10_explicit() {
        // HTTP/1.0 with Connection: keep-alive → keep-alive
        let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive\r\nHost: localhost\r\n\r\n";
        let headers = vec![
            Value::BuchiPack(vec![
                (
                    "name".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(16)),
                        ("len".into(), Value::Int(10)), // "Connection"
                    ]),
                ),
                (
                    "value".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(28)),
                        ("len".into(), Value::Int(10)), // "keep-alive"
                    ]),
                ),
            ]),
            Value::BuchiPack(vec![
                (
                    "name".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(40)),
                        ("len".into(), Value::Int(4)), // "Host"
                    ]),
                ),
                (
                    "value".into(),
                    Value::BuchiPack(vec![
                        ("start".into(), Value::Int(46)),
                        ("len".into(), Value::Int(9)), // "localhost"
                    ]),
                ),
            ]),
        ];
        assert!(determine_keep_alive(raw, &headers, 0));
    }

    #[test]
    fn test_keep_alive_case_insensitive() {
        // Connection header name and value should be case-insensitive
        let raw = b"GET / HTTP/1.1\r\nCONNECTION: CLOSE\r\n\r\n";
        let headers = vec![Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)), // "CONNECTION"
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(5)), // "CLOSE"
                ]),
            ),
        ])];
        assert!(!determine_keep_alive(raw, &headers, 1));
    }

    #[test]
    fn test_keep_alive_token_list_close_with_upgrade() {
        // "Connection: close, upgrade" — HTTP/1.1 should NOT keep alive (close token present)
        let raw = b"GET / HTTP/1.1\r\nConnection: close, upgrade\r\n\r\n";
        let headers = vec![Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)), // "Connection"
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(14)), // "close, upgrade"
                ]),
            ),
        ])];
        assert!(!determine_keep_alive(raw, &headers, 1));
    }

    #[test]
    fn test_keep_alive_token_list_keep_alive_with_extra() {
        // "Connection: keep-alive, foo" — HTTP/1.0 should keep alive (keep-alive token present)
        let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive, foo\r\n\r\n";
        let headers = vec![Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)), // "Connection"
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(15)), // "keep-alive, foo"
                ]),
            ),
        ])];
        assert!(determine_keep_alive(raw, &headers, 0));
    }

    #[test]
    fn test_keep_alive_close_wins_over_keep_alive_same_header() {
        // "Connection: keep-alive, close" — close wins on both HTTP/1.0 and 1.1
        let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive, close\r\n\r\n";
        let headers = vec![Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)),
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(17)), // "keep-alive, close"
                ]),
            ),
        ])];
        assert!(!determine_keep_alive(raw, &headers, 0)); // HTTP/1.0
        assert!(!determine_keep_alive(raw, &headers, 1)); // HTTP/1.1
    }

    #[test]
    fn test_keep_alive_close_wins_across_duplicate_headers() {
        // Two Connection headers: one says keep-alive, the other says close
        let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive\r\nConnection: close\r\n\r\n";
        let h1 = Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)),
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(10)), // "keep-alive"
                ]),
            ),
        ]);
        let h2 = Value::BuchiPack(vec![
            (
                "name".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(40)),
                    ("len".into(), Value::Int(10)),
                ]),
            ),
            (
                "value".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(52)),
                    ("len".into(), Value::Int(5)), // "close"
                ]),
            ),
        ]);
        let headers = vec![h1, h2];
        assert!(!determine_keep_alive(raw, &headers, 0)); // HTTP/1.0
        assert!(!determine_keep_alive(raw, &headers, 1)); // HTTP/1.1
    }

    // ── Keep-Alive integration tests (NET2-1h) ──

    /// Helper: read all HTTP responses from a stream, splitting on double CRLF boundaries.
    /// Returns a Vec of raw response strings.
    fn read_responses(stream: &mut std::net::TcpStream, expected: usize) -> Vec<String> {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut all_data = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(stream, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    all_data.extend_from_slice(&buf[..n]);
                    // Count complete responses by looking for Content-Length pattern
                    let text = String::from_utf8_lossy(&all_data);
                    let responses_found = text.matches("HTTP/1.1 ").count();
                    if responses_found >= expected {
                        // Check if all bodies are complete
                        let mut complete = true;
                        let mut offset = 0;
                        for _ in 0..expected {
                            if let Some(pos) = text[offset..].find("HTTP/1.1 ") {
                                let resp_start = offset + pos;
                                // Find Content-Length
                                if let Some(cl_pos) = text[resp_start..]
                                    .to_ascii_lowercase()
                                    .find("content-length: ")
                                {
                                    let cl_start = resp_start + cl_pos + 16;
                                    if let Some(cl_end) = text[cl_start..].find("\r\n") {
                                        let cl: usize =
                                            text[cl_start..cl_start + cl_end].parse().unwrap_or(0);
                                        if let Some(body_start) =
                                            text[resp_start..].find("\r\n\r\n")
                                        {
                                            let body_offset = resp_start + body_start + 4;
                                            if body_offset + cl > all_data.len() {
                                                complete = false;
                                                break;
                                            }
                                            offset = body_offset + cl;
                                        } else {
                                            complete = false;
                                            break;
                                        }
                                    } else {
                                        complete = false;
                                        break;
                                    }
                                } else {
                                    complete = false;
                                    break;
                                }
                            } else {
                                complete = false;
                                break;
                            }
                        }
                        if complete {
                            break;
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        // Split into individual responses
        let text = String::from_utf8_lossy(&all_data).to_string();
        let mut results = Vec::new();
        let mut remaining = text.as_str();
        while let Some(pos) = remaining.find("HTTP/1.1 ") {
            let start = pos;
            // Find next response or end of data
            let next = remaining[start + 9..]
                .find("HTTP/1.1 ")
                .map(|p| start + 9 + p)
                .unwrap_or(remaining.len());
            results.push(remaining[start..next].to_string());
            remaining = &remaining[next..];
        }
        results
    }

    /// NET2-1h: 1 connection, 2 requests → 2 responses (keep-alive works)
    #[test]
    fn test_keep_alive_two_requests_one_connection() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19100);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("keep-alive-ok"),
                Expr::IntLit(2, dummy_span()), // maxRequests=2
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send 2 requests on the SAME connection (HTTP/1.1 default keep-alive)
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();

        // Read first response
        let responses = read_responses(&mut client, 1);
        assert!(!responses.is_empty(), "Should receive first response");
        assert!(
            responses[0].contains("200 OK"),
            "First response should be 200 OK, got: {}",
            responses[0]
        );
        assert!(
            responses[0].contains("keep-alive-ok"),
            "First response should contain body"
        );

        // Send second request on same connection
        std::io::Write::write_all(
            &mut client,
            b"GET /second HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();

        // Read second response
        let responses2 = read_responses(&mut client, 1);
        assert!(!responses2.is_empty(), "Should receive second response");
        assert!(
            responses2[0].contains("200 OK"),
            "Second response should be 200 OK, got: {}",
            responses2[0]
        );

        // Server should terminate (maxRequests=2 reached)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-1h: Connection: close → connection terminates after one request
    #[test]
    fn test_keep_alive_connection_close_terminates() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19200);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("close-ok"),
                Expr::IntLit(1, dummy_span()), // maxRequests=1
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send request with Connection: close
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
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

        let resp = String::from_utf8_lossy(&response);
        assert!(resp.contains("200 OK"), "Should get 200 OK, got: {}", resp);
        assert!(resp.contains("close-ok"), "Response body should be present");

        // Server terminates (maxRequests=1 reached after one request with Connection: close)
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

    /// NET2-1h: HTTP/1.0 + Connection: keep-alive → connection maintained
    #[test]
    fn test_keep_alive_http10_explicit_keep_alive() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19300);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("http10-ka-ok"),
                Expr::IntLit(2, dummy_span()), // maxRequests=2
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // HTTP/1.0 with explicit Connection: keep-alive
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET /first HTTP/1.0\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
        )
        .unwrap();

        // Read first response
        let responses = read_responses(&mut client, 1);
        assert!(
            !responses.is_empty(),
            "Should receive first HTTP/1.0 keep-alive response"
        );
        assert!(
            responses[0].contains("200 OK"),
            "First response should be 200 OK, got: {}",
            responses[0]
        );

        // Send second request (still HTTP/1.0 + keep-alive)
        std::io::Write::write_all(
            &mut client,
            b"GET /second HTTP/1.0\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
        )
        .unwrap();

        // Read second response
        let responses2 = read_responses(&mut client, 1);
        assert!(
            !responses2.is_empty(),
            "Should receive second HTTP/1.0 keep-alive response"
        );
        assert!(
            responses2[0].contains("200 OK"),
            "Second response should be 200 OK, got: {}",
            responses2[0]
        );

        // Server terminates (maxRequests=2)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-1h: HTTP/1.0 without Connection header → connection closes after one request
    #[test]
    fn test_keep_alive_http10_default_close() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19400);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            // maxRequests=2 but HTTP/1.0 should close after 1
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("http10-close-ok"),
                Expr::IntLit(2, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // HTTP/1.0 without Connection header → default close
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(&mut client, b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
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

        let resp = String::from_utf8_lossy(&response);
        assert!(resp.contains("200 OK"), "Should get 200, got: {}", resp);
        assert!(resp.contains("http10-close-ok"), "Body should be present");

        // Connection should be closed after this single request.
        // Server is still running (maxRequests=2, only used 1).
        // Send another connection to consume the second request and terminate.
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let mut response2 = Vec::new();
        let _ = client2.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client2, &mut buf) {
                Ok(0) => break,
                Ok(n) => response2.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        let resp2 = String::from_utf8_lossy(&response2);
        assert!(resp2.contains("200 OK"), "Second connection should get 200");

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-1h: maxRequests across connections — verify count is global
    #[test]
    fn test_keep_alive_max_requests_across_connections() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19500);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("max-req-ok"),
                Expr::IntLit(3, dummy_span()), // maxRequests=3 total
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connection 1: send 2 requests (keep-alive)
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client1,
            b"GET /req1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();
        let resp1 = read_responses(&mut client1, 1);
        assert!(!resp1.is_empty() && resp1[0].contains("200 OK"));

        std::io::Write::write_all(
            &mut client1,
            b"GET /req2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let resp2 = read_responses(&mut client1, 1);
        assert!(!resp2.is_empty() && resp2[0].contains("200 OK"));
        drop(client1);

        // Connection 2: send 1 request (this is the 3rd overall → triggers maxRequests)
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"GET /req3 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();
        let resp3 = read_responses(&mut client2, 1);
        assert!(!resp3.is_empty() && resp3[0].contains("200 OK"));

        // Server should terminate (3 total requests reached)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(3));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // ── Chunked Transfer Encoding unit tests (NET2-2) ──

    #[test]
    fn test_chunked_compact_basic() {
        // "4\r\nWiki\r\n7\r\npedia i\r\n0\r\n\r\n"
        // Should compact to "Wikipedia i"
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"4\r\nWiki\r\n7\r\npedia i\r\n0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
        assert_eq!(result.body_len, 11); // "Wikipedia i" = 11 bytes
        assert_eq!(
            &buf[body_offset..body_offset + result.body_len],
            b"Wikipedia i"
        );
        // wire_consumed should cover all chunked framing
        assert_eq!(result.wire_consumed, chunked_body.len());
    }

    #[test]
    fn test_chunked_compact_single_chunk() {
        // Single chunk: "5\r\nhello\r\n0\r\n\r\n"
        let head = b"POST /data HTTP/1.1\r\nHost: h\r\n\r\n";
        let chunked_body = b"5\r\nhello\r\n0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
        assert_eq!(result.body_len, 5);
        assert_eq!(&buf[body_offset..body_offset + result.body_len], b"hello");
    }

    #[test]
    fn test_chunked_compact_zero_only() {
        // Terminator only: "0\r\n\r\n" → empty body
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
        assert_eq!(result.body_len, 0);
        assert_eq!(result.wire_consumed, 5); // "0\r\n\r\n"
    }

    #[test]
    fn test_chunked_compact_hex_sizes() {
        // Hex chunk sizes: a (10) + 10 (16) = 26 bytes
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let data_a = b"0123456789"; // 10 bytes
        let data_10 = b"abcdefghijklmnop"; // 16 bytes
        let mut chunked = Vec::new();
        chunked.extend_from_slice(b"a\r\n");
        chunked.extend_from_slice(data_a);
        chunked.extend_from_slice(b"\r\n");
        chunked.extend_from_slice(b"10\r\n");
        chunked.extend_from_slice(data_10);
        chunked.extend_from_slice(b"\r\n");
        chunked.extend_from_slice(b"0\r\n\r\n");

        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(&chunked);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
        assert_eq!(result.body_len, 26);
        assert_eq!(
            &buf[body_offset..body_offset + result.body_len],
            b"0123456789abcdefghijklmnop"
        );
    }

    #[test]
    fn test_chunked_compact_chunk_ext_ignored() {
        // Chunk extensions should be ignored: "5;ext=val\r\nhello\r\n0\r\n\r\n"
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"5;ext=val\r\nhello\r\n0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
        assert_eq!(result.body_len, 5);
        assert_eq!(&buf[body_offset..body_offset + result.body_len], b"hello");
    }

    #[test]
    fn test_chunked_compact_trailers_skipped() {
        // Trailers after 0 chunk: "0\r\nTrailer: val\r\n\r\n"
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"5\r\nhello\r\n0\r\nTrailer: val\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
        assert_eq!(result.body_len, 5);
        assert_eq!(&buf[body_offset..body_offset + result.body_len], b"hello");
    }

    #[test]
    fn test_chunked_compact_malformed_chunk_size() {
        // Invalid hex in chunk size → error
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"XY\r\nhello\r\n0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid chunk-size"));
    }

    #[test]
    fn test_chunked_compact_truncated_data() {
        // Chunk promises 10 bytes but only 5 available → truncated
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"a\r\nhello";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("truncated"));
    }

    #[test]
    fn test_chunked_compact_missing_crlf_after_data() {
        // Data present but CRLF after it is wrong
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"5\r\nhelloXX0\r\n\r\n"; // XX instead of \r\n
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CRLF after chunk data"));
    }

    #[test]
    fn test_chunked_compact_empty_chunk_size() {
        // Empty chunk size line → error
        let head = b"GET / HTTP/1.1\r\n\r\n";
        let chunked_body = b"\r\nhello\r\n0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(head);
        buf.extend_from_slice(chunked_body);
        let body_offset = head.len();

        let result = chunked_in_place_compact(&mut buf, body_offset);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty chunk-size"));
    }

    // ── parse_request_head: Transfer-Encoding detection (NET2-2a) ──

    #[test]
    fn test_parse_head_detects_chunked() {
        let raw = b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n";
        let result = parse_request_head(raw);
        let inner = extract_result_value(&result).unwrap();
        assert_eq!(get_field_bool(inner, "chunked"), Some(true));
        assert_eq!(get_field_int(inner, "contentLength"), Some(0));
    }

    #[test]
    fn test_parse_head_no_chunked() {
        let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = parse_request_head(raw);
        let inner = extract_result_value(&result).unwrap();
        assert_eq!(get_field_bool(inner, "chunked"), Some(false));
    }

    #[test]
    fn test_parse_head_chunked_case_insensitive() {
        let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: Chunked\r\n\r\n";
        let result = parse_request_head(raw);
        let inner = extract_result_value(&result).unwrap();
        assert_eq!(get_field_bool(inner, "chunked"), Some(true));
    }

    #[test]
    fn test_parse_head_chunked_in_token_list() {
        // "Transfer-Encoding: gzip, chunked"
        let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: gzip, chunked\r\n\r\n";
        let result = parse_request_head(raw);
        let inner = extract_result_value(&result).unwrap();
        assert_eq!(get_field_bool(inner, "chunked"), Some(true));
    }

    // ── parse_request_head: Content-Length + chunked rejection (NET2-2e) ──

    #[test]
    fn test_parse_head_rejects_cl_and_chunked() {
        // RFC 7230 §3.3.3: Content-Length + Transfer-Encoding: chunked = reject
        let raw = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\nhello";
        let result = parse_request_head(raw);
        // Should be a failure result
        assert!(
            extract_result_value(&result).is_none(),
            "Should reject CL + TE:chunked"
        );
    }

    #[test]
    fn test_parse_head_cl_without_chunked_ok() {
        // Content-Length without chunked should work fine
        let raw = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello";
        let result = parse_request_head(raw);
        let inner = extract_result_value(&result).unwrap();
        assert_eq!(get_field_bool(inner, "chunked"), Some(false));
        assert_eq!(get_field_int(inner, "contentLength"), Some(5));
    }

    // ── readBody with chunked body (NET2-2h) ──

    #[test]
    fn test_read_body_chunked() {
        // After in-place compaction, raw contains head + compacted body.
        // The body span points to the compacted region.
        let head = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n";
        let compacted_body = b"Wikipedia i"; // Result of compaction
        let mut raw = Vec::new();
        raw.extend_from_slice(head);
        raw.extend_from_slice(compacted_body);

        let body_start = head.len() as i64;
        let body_len = compacted_body.len() as i64;

        let req = Value::BuchiPack(vec![
            ("raw".into(), Value::Bytes(raw)),
            (
                "body".into(),
                Value::BuchiPack(vec![
                    ("start".into(), Value::Int(body_start)),
                    ("len".into(), Value::Int(body_len)),
                ]),
            ),
        ]);
        let result = eval_read_body(&req).unwrap();
        assert_eq!(result, Value::Bytes(b"Wikipedia i".to_vec()));
    }

    // ── httpServe integration test: chunked body (NET2-2i) ──

    /// NET2-2i: httpServe with chunked request body
    #[test]
    fn test_http_serve_chunked_body() {
        use std::sync::atomic::{AtomicU16, Ordering};
        // NB2-11: Use distinct port range to avoid collision with test_keep_alive_max_requests_across_connections
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19700);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("chunked-echo"),
                Expr::IntLit(1, dummy_span()), // maxRequests=1
                Expr::IntLit(1000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();

        // Send a chunked request: "hello" + " world" = "hello world"
        let chunked_request = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        std::io::Write::write_all(&mut client, chunked_request).unwrap();

        let mut response = Vec::new();
        let _ = std::io::Read::read_to_end(&mut client, &mut response);
        let response_str = String::from_utf8_lossy(&response);

        assert!(
            response_str.contains("200 OK"),
            "Expected 200 OK for chunked body, got: {}",
            response_str
        );
        assert!(
            response_str.contains("chunked-echo"),
            "Expected handler body text in response, got: {}",
            response_str
        );

        server_handle.join().unwrap();
    }

    /// NET2-2i: httpServe rejects Content-Length + Transfer-Encoding: chunked
    #[test]
    fn test_http_serve_rejects_cl_and_chunked() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19510);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("reject-test"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(1000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();

        // Send request with both Content-Length and Transfer-Encoding: chunked
        let bad_request = b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\nhello";
        std::io::Write::write_all(&mut client, bad_request).unwrap();

        let mut response = Vec::new();
        let _ = std::io::Read::read_to_end(&mut client, &mut response);
        let response_str = String::from_utf8_lossy(&response);

        assert!(
            response_str.contains("400 Bad Request"),
            "Expected 400 for CL+TE:chunked, got: {}",
            response_str
        );

        server_handle.join().unwrap();
    }

    /// NET2-2i: httpServe with malformed chunk size → 400
    #[test]
    fn test_http_serve_malformed_chunk() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19520);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("malformed-test"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(1000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();

        // Malformed chunk: "XY" is not valid hex
        let bad_request = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nXY\r\nhello\r\n0\r\n\r\n";
        std::io::Write::write_all(&mut client, bad_request).unwrap();

        let mut response = Vec::new();
        let _ = std::io::Read::read_to_end(&mut client, &mut response);
        let response_str = String::from_utf8_lossy(&response);

        assert!(
            response_str.contains("400 Bad Request"),
            "Expected 400 for malformed chunk, got: {}",
            response_str
        );

        server_handle.join().unwrap();
    }

    /// NET2-2i: chunked body + keep-alive (chunked first, then Content-Length on same connection)
    #[test]
    fn test_http_serve_chunked_then_normal_keep_alive() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19530);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("mixed-test"),
                Expr::IntLit(2, dummy_span()), // maxRequests=2
                Expr::IntLit(2000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();

        // Request 1: chunked body (HTTP/1.1 keep-alive by default)
        let req1 = b"POST /first HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        std::io::Write::write_all(&mut client, req1).unwrap();

        let resp1 = read_responses(&mut client, 1);
        assert!(
            !resp1.is_empty() && resp1[0].contains("200 OK"),
            "First chunked request should succeed, got: {:?}",
            resp1
        );

        // Request 2: normal Content-Length body on same connection
        let req2 = b"POST /second HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nworld";
        std::io::Write::write_all(&mut client, req2).unwrap();

        let resp2 = read_responses(&mut client, 1);
        assert!(
            !resp2.is_empty() && resp2[0].contains("200 OK"),
            "Second normal request should succeed on same connection, got: {:?}",
            resp2
        );

        // Server should terminate (maxRequests=2)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-2i: large chunked body (multiple chunks totaling > 8KB)
    #[test]
    fn test_http_serve_chunked_large_body() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19540);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("large-chunked"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(2000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(1000)))
            .unwrap();

        // Build a chunked body with 3 chunks of 4096 bytes each (12KB total)
        let chunk_data = vec![b'A'; 4096];
        let mut chunked_body = Vec::new();
        for _ in 0..3 {
            chunked_body.extend_from_slice(format!("{:x}\r\n", chunk_data.len()).as_bytes());
            chunked_body.extend_from_slice(&chunk_data);
            chunked_body.extend_from_slice(b"\r\n");
        }
        chunked_body.extend_from_slice(b"0\r\n\r\n");

        let mut request = Vec::new();
        request.extend_from_slice(
            b"POST /large HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        );
        request.extend_from_slice(&chunked_body);

        std::io::Write::write_all(&mut client, &request).unwrap();

        let mut response = Vec::new();
        let _ = std::io::Read::read_to_end(&mut client, &mut response);
        let response_str = String::from_utf8_lossy(&response);

        assert!(
            response_str.contains("200 OK"),
            "Large chunked body should succeed, got: {}",
            response_str
        );

        server_handle.join().unwrap();
    }

    // ── NET2-3g: Concurrent handler dispatch tests ──

    /// NET2-3g: Two clients connect simultaneously, both get responses.
    #[test]
    fn test_concurrent_two_clients_both_get_responses() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19600);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("concurrent-ok"),
                Expr::IntLit(2, dummy_span()), // maxRequests=2 (one per client)
                Expr::IntLit(5000, dummy_span()), // timeoutMs
                Expr::IntLit(4, dummy_span()), // maxConnections=4
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connect two clients simultaneously before sending any data
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Both send requests
        std::io::Write::write_all(
            &mut client1,
            b"GET /client1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"GET /client2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        // Both should get responses
        let responses1 = read_responses(&mut client1, 1);
        let responses2 = read_responses(&mut client2, 1);

        assert!(!responses1.is_empty(), "Client 1 should receive a response");
        assert!(
            responses1[0].contains("200 OK"),
            "Client 1 should get 200 OK, got: {}",
            responses1[0]
        );
        assert!(
            responses1[0].contains("concurrent-ok"),
            "Client 1 should get body"
        );

        assert!(!responses2.is_empty(), "Client 2 should receive a response");
        assert!(
            responses2[0].contains("200 OK"),
            "Client 2 should get 200 OK, got: {}",
            responses2[0]
        );
        assert!(
            responses2[0].contains("concurrent-ok"),
            "Client 2 should get body"
        );

        // Server should terminate (maxRequests=2)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-3g: maxConnections limits simultaneous connections.
    /// Excess connections must wait for a slot or be processed later.
    #[test]
    fn test_concurrent_max_connections_limit() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19610);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("limited-ok"),
                Expr::IntLit(3, dummy_span()),    // maxRequests=3
                Expr::IntLit(5000, dummy_span()), // timeoutMs
                Expr::IntLit(2, dummy_span()),    // maxConnections=2
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connect 3 clients. maxConnections=2, so only 2 can be in the pool at once.
        // The third will be accepted after one of the first two closes.
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Send requests on first two
        std::io::Write::write_all(
            &mut client1,
            b"GET /c1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"GET /c2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        // Both get responses
        let r1 = read_responses(&mut client1, 1);
        let r2 = read_responses(&mut client2, 1);
        assert!(
            !r1.is_empty() && r1[0].contains("200 OK"),
            "Client 1 should get 200"
        );
        assert!(
            !r2.is_empty() && r2[0].contains("200 OK"),
            "Client 2 should get 200"
        );

        // Drop clients to free slots
        drop(client1);
        drop(client2);

        // Third client can now connect (after slots free up)
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut client3 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client3,
            b"GET /c3 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let r3 = read_responses(&mut client3, 1);
        assert!(
            !r3.is_empty() && r3[0].contains("200 OK"),
            "Client 3 should get 200"
        );

        // Server should terminate (maxRequests=3)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(3));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-3g: maxRequests counts across all connections.
    #[test]
    fn test_concurrent_max_requests_across_connections() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19620);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("counted"),
                Expr::IntLit(3, dummy_span()), // maxRequests=3 total
                Expr::IntLit(5000, dummy_span()),
                Expr::IntLit(4, dummy_span()), // maxConnections=4
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client 1: keep-alive, sends 2 requests
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(&mut client1, b"GET /r1 HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let r1 = read_responses(&mut client1, 1);
        assert!(!r1.is_empty() && r1[0].contains("200 OK"));

        std::io::Write::write_all(&mut client1, b"GET /r2 HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let r2 = read_responses(&mut client1, 1);
        assert!(!r2.is_empty() && r2[0].contains("200 OK"));

        // Client 2: sends 1 request (should be the 3rd total, hitting maxRequests)
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"GET /r3 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let r3 = read_responses(&mut client2, 1);
        assert!(!r3.is_empty() && r3[0].contains("200 OK"));

        // Server should terminate (maxRequests=3 reached)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(3));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-3g: Per-connection buffer isolation.
    /// Verify that data from one connection does not leak into another.
    #[test]
    fn test_concurrent_buffer_isolation() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19630);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("isolated"),
                Expr::IntLit(2, dummy_span()), // maxRequests=2
                Expr::IntLit(5000, dummy_span()),
                Expr::IntLit(4, dummy_span()), // maxConnections=4
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client 1: POST with body "AAAA"
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client1,
            b"POST /c1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\nAAAA",
        )
        .unwrap();

        // Client 2: POST with body "BBBB"
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"POST /c2 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\nBBBB",
        )
        .unwrap();

        // Both should get their own responses without data leakage
        let r1 = read_responses(&mut client1, 1);
        let r2 = read_responses(&mut client2, 1);

        assert!(
            !r1.is_empty() && r1[0].contains("200 OK"),
            "Client 1 should get 200"
        );
        assert!(
            !r2.is_empty() && r2[0].contains("200 OK"),
            "Client 2 should get 200"
        );

        // Server terminates
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-3g: maxConnections defaults to 128 when not specified (v1 compat).
    #[test]
    fn test_concurrent_max_connections_default() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19640);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            // No maxConnections arg — should default to 128
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("default-mc"),
                Expr::IntLit(1, dummy_span()), // maxRequests=1
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Single client should work fine with default maxConnections
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET /test HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let r = read_responses(&mut client, 1);
        assert!(!r.is_empty() && r[0].contains("200 OK"));
        assert!(r[0].contains("default-mc"));

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

    /// NET2-3g: Keep-alive works correctly in concurrent mode.
    /// One connection does keep-alive while another connects separately.
    #[test]
    fn test_concurrent_keep_alive_with_multiple_connections() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19650);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("ka-concurrent"),
                Expr::IntLit(3, dummy_span()), // maxRequests=3 total
                Expr::IntLit(5000, dummy_span()),
                Expr::IntLit(4, dummy_span()), // maxConnections=4
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client 1: keep-alive, 2 requests
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Client 2: single request with Connection: close
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();

        // Client 1 first request
        std::io::Write::write_all(
            &mut client1,
            b"GET /ka1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();
        let r1 = read_responses(&mut client1, 1);
        assert!(!r1.is_empty() && r1[0].contains("200 OK"));

        // Client 2 request
        std::io::Write::write_all(
            &mut client2,
            b"GET /close HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let r2 = read_responses(&mut client2, 1);
        assert!(!r2.is_empty() && r2[0].contains("200 OK"));

        // Client 1 second request (keep-alive, this is the 3rd total)
        std::io::Write::write_all(
            &mut client1,
            b"GET /ka2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let r3 = read_responses(&mut client1, 1);
        assert!(!r3.is_empty() && r3[0].contains("200 OK"));

        // Server should terminate (maxRequests=3)
        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(3));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// NET2-3g: Chunked body works with concurrent connections.
    #[test]
    fn test_concurrent_chunked_body() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19660);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("chunked-concurrent"),
                Expr::IntLit(2, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
                Expr::IntLit(4, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client 1: normal Content-Length request
        let mut client1 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client1,
            b"POST /normal HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        )
        .unwrap();

        // Client 2: chunked request
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client2,
            b"POST /chunked HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nworld\r\n0\r\n\r\n",
        )
        .unwrap();

        let r1 = read_responses(&mut client1, 1);
        let r2 = read_responses(&mut client2, 1);

        assert!(
            !r1.is_empty() && r1[0].contains("200 OK"),
            "Normal request should succeed"
        );
        assert!(
            !r2.is_empty() && r2[0].contains("200 OK"),
            "Chunked request should succeed"
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(get_field_int(inner, "requests"), Some(2));
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// Regression: keep-alive partial request timeout returns 400 (not silent close).
    ///
    /// Scenario: send a complete first request (200 OK, keep-alive), then send a
    /// partial second request and let it timeout. The server must respond with 400
    /// Bad Request (not silently close the connection) and count the request.
    #[test]
    fn test_keep_alive_partial_timeout_returns_400() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19670);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("partial-timeout-test"),
                Expr::IntLit(3, dummy_span()), // maxRequests=3 (1 real + 1 partial-timeout + 1 margin)
                Expr::IntLit(300, dummy_span()), // timeoutMs=300 (short for test speed)
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Phase 1: send a complete request on keep-alive connection
        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        std::io::Write::write_all(
            &mut client,
            b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();

        let resp1 = read_responses(&mut client, 1);
        assert!(
            !resp1.is_empty() && resp1[0].contains("200 OK"),
            "First request should get 200 OK, got: {:?}",
            resp1
        );

        // Phase 2: send a PARTIAL second request (incomplete head) and let it timeout
        std::io::Write::write_all(
            &mut client,
            b"GET /second HTTP/1.1\r\nHost: lo", // intentionally incomplete
        )
        .unwrap();

        // Wait for the server's 300ms timeout to fire
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Read whatever the server sends back (should be 400, not EOF)
        let _ = client.set_read_timeout(Some(std::time::Duration::from_millis(500)));
        let mut buf = [0u8; 4096];
        let mut all_data = Vec::new();
        loop {
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => all_data.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        let response = String::from_utf8_lossy(&all_data).to_string();
        assert!(
            response.contains("400 Bad Request"),
            "Partial request on keep-alive should get 400 Bad Request, got: {:?}",
            response
        );

        // Server should count the partial as a request (total=2).
        // Send one more request on a fresh connection to reach maxRequests=3.
        let mut client2 = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = client2.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        std::io::Write::write_all(
            &mut client2,
            b"GET /third HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let resp3 = read_responses(&mut client2, 1);
        assert!(
            !resp3.is_empty() && resp3[0].contains("200 OK"),
            "Third request should get 200 OK"
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                let inner = extract_result_value(&a.value).unwrap();
                assert_eq!(get_field_bool(inner, "ok"), Some(true));
                assert_eq!(
                    get_field_int(inner, "requests"),
                    Some(3),
                    "Server should count partial timeout as a request (1 OK + 1 partial-400 + 1 OK = 3)"
                );
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    /// Regression: slow-split request within timeout window is processed normally.
    ///
    /// Scenario: send a request head in two parts, each spaced under the timeout
    /// window. The server should reassemble and return 200 OK (not 400).
    #[test]
    fn test_slow_split_request_within_timeout_succeeds() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(19680);
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_handler_expr("slow-split-ok"),
                Expr::IntLit(1, dummy_span()),   // maxRequests=1
                Expr::IntLit(500, dummy_span()), // timeoutMs=500
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));

        // Send first half of the request head
        std::io::Write::write_all(&mut client, b"GET /split HTTP/1.1\r\nHost: localhost\r\n")
            .unwrap();

        // Wait 200ms — under the 500ms timeout, but long enough to trigger
        // a timeout if last_activity were not updated on byte reception.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Send the rest (completing the head with double CRLF)
        std::io::Write::write_all(&mut client, b"Connection: close\r\n\r\n").unwrap();

        let resp = read_responses(&mut client, 1);
        assert!(
            !resp.is_empty() && resp[0].contains("200 OK"),
            "Slow-split request should be processed normally (200 OK), got: {:?}",
            resp
        );
        assert!(
            resp[0].contains("slow-split-ok"),
            "Response body should contain handler output"
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

    // ── NET3 Phase 1 tests ──

    // NET3-1b: Writer state machine
    #[test]
    fn test_writer_state_initial() {
        let writer = StreamingWriter::new();
        assert_eq!(writer.state, WriterState::Idle);
        assert_eq!(writer.pending_status, 200);
        assert!(writer.pending_headers.is_empty());
        assert!(!writer.sse_mode);
    }

    #[test]
    fn test_writer_state_transitions() {
        let mut writer = StreamingWriter::new();
        assert_eq!(writer.state, WriterState::Idle);

        // Idle -> HeadPrepared (via startResponse)
        writer.state = WriterState::HeadPrepared;
        assert_eq!(writer.state, WriterState::HeadPrepared);

        // HeadPrepared -> Streaming (via writeChunk)
        writer.state = WriterState::Streaming;
        assert_eq!(writer.state, WriterState::Streaming);

        // Streaming -> Ended (via endResponse)
        writer.state = WriterState::Ended;
        assert_eq!(writer.state, WriterState::Ended);
    }

    // NET3-1c: startResponse pending state
    #[test]
    fn test_start_response_pending_state() {
        let mut writer = StreamingWriter::new();
        assert_eq!(writer.pending_status, 200);

        // Update pending status/headers
        writer.pending_status = 201;
        writer.pending_headers = vec![("X-Custom".to_string(), "value1".to_string())];
        writer.state = WriterState::HeadPrepared;

        assert_eq!(writer.pending_status, 201);
        assert_eq!(writer.pending_headers.len(), 1);
        assert_eq!(writer.pending_headers[0].0, "X-Custom");
        assert_eq!(writer.pending_headers[0].1, "value1");
    }

    // NET3-1d: Head commit builds correct wire format
    #[test]
    fn test_build_streaming_head_basic() {
        let headers = vec![("Content-Type".to_string(), "text/plain".to_string())];
        let head = build_streaming_head(200, &headers);
        let head_str = String::from_utf8(head).unwrap();

        assert!(head_str.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(head_str.contains("Content-Type: text/plain\r\n"));
        assert!(head_str.contains("Transfer-Encoding: chunked\r\n"));
        assert!(head_str.ends_with("\r\n\r\n"));
    }

    #[test]
    fn test_build_streaming_head_custom_status() {
        let head = build_streaming_head(404, &[]);
        let head_str = String::from_utf8(head).unwrap();
        assert!(head_str.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(head_str.contains("Transfer-Encoding: chunked\r\n"));
    }

    #[test]
    fn test_build_streaming_head_multiple_headers() {
        let headers = vec![
            ("Content-Type".to_string(), "text/html".to_string()),
            ("X-Foo".to_string(), "bar".to_string()),
            ("Cache-Control".to_string(), "no-cache".to_string()),
        ];
        let head = build_streaming_head(200, &headers);
        let head_str = String::from_utf8(head).unwrap();

        assert!(head_str.contains("Content-Type: text/html\r\n"));
        assert!(head_str.contains("X-Foo: bar\r\n"));
        assert!(head_str.contains("Cache-Control: no-cache\r\n"));
        assert!(head_str.contains("Transfer-Encoding: chunked\r\n"));
    }

    // NET3-1e: Reserved header rejection
    #[test]
    fn test_reserved_header_content_length_rejected() {
        let headers = vec![("Content-Length".to_string(), "42".to_string())];
        let result = StreamingWriter::validate_reserved_headers(&headers);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Content-Length"),
            "error should mention Content-Length: {}",
            msg
        );
    }

    #[test]
    fn test_reserved_header_transfer_encoding_rejected() {
        let headers = vec![("Transfer-Encoding".to_string(), "chunked".to_string())];
        let result = StreamingWriter::validate_reserved_headers(&headers);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Transfer-Encoding"),
            "error should mention Transfer-Encoding: {}",
            msg
        );
    }

    #[test]
    fn test_reserved_header_case_insensitive() {
        // content-length (lowercase)
        let headers = vec![("content-length".to_string(), "42".to_string())];
        assert!(StreamingWriter::validate_reserved_headers(&headers).is_err());

        // TRANSFER-ENCODING (uppercase)
        let headers = vec![("TRANSFER-ENCODING".to_string(), "chunked".to_string())];
        assert!(StreamingWriter::validate_reserved_headers(&headers).is_err());
    }

    #[test]
    fn test_non_reserved_headers_allowed() {
        let headers = vec![
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("X-Custom".to_string(), "value".to_string()),
            ("Cache-Control".to_string(), "no-cache".to_string()),
        ];
        assert!(StreamingWriter::validate_reserved_headers(&headers).is_ok());
    }

    #[test]
    fn test_empty_headers_allowed() {
        let headers: Vec<(String, String)> = vec![];
        assert!(StreamingWriter::validate_reserved_headers(&headers).is_ok());
    }

    // NET3-1f: Bodyless status validation
    #[test]
    fn test_bodyless_status_detection() {
        // 1xx
        assert!(StreamingWriter::is_bodyless_status(100));
        assert!(StreamingWriter::is_bodyless_status(101));
        assert!(StreamingWriter::is_bodyless_status(199));
        // 204, 205, 304
        assert!(StreamingWriter::is_bodyless_status(204));
        assert!(StreamingWriter::is_bodyless_status(205));
        assert!(StreamingWriter::is_bodyless_status(304));
        // Normal statuses are NOT bodyless
        assert!(!StreamingWriter::is_bodyless_status(200));
        assert!(!StreamingWriter::is_bodyless_status(201));
        assert!(!StreamingWriter::is_bodyless_status(301));
        assert!(!StreamingWriter::is_bodyless_status(302));
        assert!(!StreamingWriter::is_bodyless_status(400));
        assert!(!StreamingWriter::is_bodyless_status(404));
        assert!(!StreamingWriter::is_bodyless_status(500));
    }

    // NET3-1d: http_reason_phrase coverage
    #[test]
    fn test_http_reason_phrases() {
        assert_eq!(http_reason_phrase(200), "OK");
        assert_eq!(http_reason_phrase(201), "Created");
        assert_eq!(http_reason_phrase(204), "No Content");
        assert_eq!(http_reason_phrase(301), "Moved Permanently");
        assert_eq!(http_reason_phrase(400), "Bad Request");
        assert_eq!(http_reason_phrase(404), "Not Found");
        assert_eq!(http_reason_phrase(500), "Internal Server Error");
        assert_eq!(http_reason_phrase(999), "Unknown");
    }

    // NET3-1a: v3 streaming API sentinel guard
    #[test]
    fn test_v3_api_sentinel_without_import() {
        let mut interp = Interpreter::new();
        let args: Vec<Expr> = vec![];
        // Without sentinel, these should return None (not dispatched)
        assert!(
            interp
                .try_net_func("startResponse", &args)
                .unwrap()
                .is_none()
        );
        assert!(interp.try_net_func("writeChunk", &args).unwrap().is_none());
        assert!(interp.try_net_func("endResponse", &args).unwrap().is_none());
        assert!(interp.try_net_func("sseEvent", &args).unwrap().is_none());
    }

    #[test]
    fn test_v3_api_sentinel_with_import_errors_outside_handler() {
        let mut interp = Interpreter::new();
        // Set sentinel as if imported from taida-lang/net
        for sym in &["startResponse", "writeChunk", "endResponse", "sseEvent"] {
            interp
                .env
                .define_force(sym, Value::Str(format!("__net_builtin_{}", sym)));
        }
        let args: Vec<Expr> = vec![];
        // With sentinel but outside handler context, these should error
        for sym in &["startResponse", "writeChunk", "endResponse", "sseEvent"] {
            let result = interp.try_net_func(sym, &args);
            assert!(result.is_err(), "{} should error outside handler", sym);
            let msg = result.unwrap_err().message;
            assert!(
                msg.contains("2-argument httpServe handler"),
                "{} error should mention 2-arg handler: {}",
                sym,
                msg
            );
        }
    }

    // NET3-1a: 2-arg handler detection + one-shot fallback
    // Build a 2-arg handler lambda expression (req, writer) that returns a response pack.
    fn make_two_arg_handler_expr(body_text: &str) -> Expr {
        Expr::Lambda(
            vec![
                Param {
                    name: "req".into(),
                    type_annotation: None,
                    default_value: None,
                    span: dummy_span(),
                },
                Param {
                    name: "writer".into(),
                    type_annotation: None,
                    default_value: None,
                    span: dummy_span(),
                },
            ],
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

    /// Build a 2-arg handler that returns Unit (does nothing, no writer usage).
    fn make_two_arg_noop_handler_expr() -> Expr {
        Expr::Lambda(
            vec![
                Param {
                    name: "req".into(),
                    type_annotation: None,
                    default_value: None,
                    span: dummy_span(),
                },
                Param {
                    name: "writer".into(),
                    type_annotation: None,
                    default_value: None,
                    span: dummy_span(),
                },
            ],
            Box::new(Expr::IntLit(1, dummy_span())),
            dummy_span(),
        )
    }

    /// Allocate a unique, bindable port for tests using a monotonic counter.
    /// Same pattern as tests/parity.rs — avoids the TOCTOU race of bind(0)+drop.
    fn v3_free_port() -> u16 {
        use std::sync::Once;
        use std::sync::atomic::{AtomicU16, Ordering};

        static INIT: Once = Once::new();
        static COUNTER: AtomicU16 = AtomicU16::new(0);

        INIT.call_once(|| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let seed = listener.local_addr().unwrap().port();
            COUNTER.store(seed, Ordering::Relaxed);
        });

        for _ in 0..200 {
            let port = COUNTER.fetch_add(1, Ordering::Relaxed);
            if !(10000..=65000).contains(&port) {
                // Counter drifted out of usable range — reseed from OS.
                // The OS-assigned port is verified bindable (listener is alive).
                // Advance the counter past it so no other caller gets the same number.
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let fresh = listener.local_addr().unwrap().port();
                COUNTER.store(fresh.wrapping_add(1), Ordering::Relaxed);
                // Drop the listener, then re-verify the port is still bindable.
                drop(listener);
                if (10000..=65000).contains(&fresh)
                    && std::net::TcpListener::bind(("127.0.0.1", fresh)).is_ok()
                {
                    return fresh;
                }
                // Rare: OS gave an out-of-range or immediately-reclaimed port. Retry.
                continue;
            }
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
        panic!("v3_free_port: could not find a free port after 200 attempts");
    }

    #[test]
    fn test_v3_two_arg_handler_one_shot_fallback() {
        let port = v3_free_port();

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_two_arg_handler_expr("fallback-ok"),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

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
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "2-arg one-shot fallback should return 200 OK: {}",
            response_str
        );
        assert!(
            response_str.contains("fallback-ok"),
            "2-arg one-shot fallback should return handler body: {}",
            response_str
        );
        // One-shot uses Content-Length, not chunked
        assert!(
            response_str.contains("Content-Length"),
            "2-arg one-shot fallback should use Content-Length: {}",
            response_str
        );
        assert!(
            !response_str.contains("Transfer-Encoding: chunked"),
            "2-arg one-shot fallback should NOT use chunked TE: {}",
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

    #[test]
    fn test_v3_two_arg_handler_no_return_fallback() {
        let port = v3_free_port();

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                make_two_arg_noop_handler_expr(),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

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
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "2-arg no-return fallback should return 200: {}",
            response_str
        );
        // Empty body → Content-Length: 0
        assert!(
            response_str.contains("Content-Length: 0"),
            "2-arg no-return fallback should have Content-Length: 0: {}",
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

    // ── NET3 Phase 2 tests ──

    use crate::parser::Statement;
    use std::sync::Arc;

    /// Build a streaming handler FuncValue that calls the given sequence of
    /// streaming API calls. Each call is a Statement::Expr(Expr::FuncCall(...)).
    /// The handler has params (req, writer) and body is the statements.
    fn make_streaming_handler(stmts: Vec<Statement>) -> Value {
        Value::Function(super::super::value::FuncValue {
            name: "<streaming_handler>".to_string(),
            params: vec![
                Param {
                    name: "req".into(),
                    type_annotation: None,
                    default_value: None,
                    span: dummy_span(),
                },
                Param {
                    name: "writer".into(),
                    type_annotation: None,
                    default_value: None,
                    span: dummy_span(),
                },
            ],
            body: stmts,
            closure: Arc::new(std::collections::HashMap::new()),
            return_type: None,
        })
    }

    /// Build `writeChunk(writer, "data")` expression
    fn make_write_chunk_call(data: &str) -> Expr {
        Expr::FuncCall(
            Box::new(Expr::Ident("writeChunk".into(), dummy_span())),
            vec![
                Expr::Ident("writer".into(), dummy_span()),
                Expr::StringLit(data.into(), dummy_span()),
            ],
            dummy_span(),
        )
    }

    /// Build `writeChunk(writer, Bytes)` expression using a pre-set variable
    fn make_write_chunk_bytes_call(_data: Vec<u8>) -> Expr {
        Expr::FuncCall(
            Box::new(Expr::Ident("writeChunk".into(), dummy_span())),
            vec![
                Expr::Ident("writer".into(), dummy_span()),
                // Use a pre-evaluated Bytes value via a variable
                Expr::Ident("__test_bytes_data".into(), dummy_span()),
            ],
            dummy_span(),
        )
    }

    /// Build `endResponse(writer)` expression
    fn make_end_response_call() -> Expr {
        Expr::FuncCall(
            Box::new(Expr::Ident("endResponse".into(), dummy_span())),
            vec![Expr::Ident("writer".into(), dummy_span())],
            dummy_span(),
        )
    }

    /// Build `startResponse(writer, status, headers)` expression
    fn make_start_response_call(status: i64, headers: Vec<(String, String)>) -> Expr {
        let header_list: Vec<Expr> = headers
            .into_iter()
            .map(|(name, value)| {
                Expr::BuchiPack(
                    vec![
                        BuchiField {
                            name: "name".into(),
                            value: Expr::StringLit(name, dummy_span()),
                            span: dummy_span(),
                        },
                        BuchiField {
                            name: "value".into(),
                            value: Expr::StringLit(value, dummy_span()),
                            span: dummy_span(),
                        },
                    ],
                    dummy_span(),
                )
            })
            .collect();

        Expr::FuncCall(
            Box::new(Expr::Ident("startResponse".into(), dummy_span())),
            vec![
                Expr::Ident("writer".into(), dummy_span()),
                Expr::IntLit(status, dummy_span()),
                Expr::ListLit(header_list, dummy_span()),
            ],
            dummy_span(),
        )
    }

    /// Build `sseEvent(writer, event, data)` expression
    fn make_sse_event_call(event: &str, data: &str) -> Expr {
        Expr::FuncCall(
            Box::new(Expr::Ident("sseEvent".into(), dummy_span())),
            vec![
                Expr::Ident("writer".into(), dummy_span()),
                Expr::StringLit(event.into(), dummy_span()),
                Expr::StringLit(data.into(), dummy_span()),
            ],
            dummy_span(),
        )
    }

    /// Helper to set up streaming API sentinels on an interpreter
    fn setup_v3_sentinels(interp: &mut Interpreter) {
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        for sym in &["startResponse", "writeChunk", "endResponse", "sseEvent"] {
            interp
                .env
                .define_force(sym, Value::Str(format!("__net_builtin_{}", sym)));
        }
    }

    /// Read full HTTP response from a client stream
    fn read_full_response(client: &mut std::net::TcpStream) -> String {
        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(client, &mut buf) {
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
        String::from_utf8_lossy(&response).to_string()
    }

    // NET3-2b: writeChunk sends chunked body
    #[test]
    fn test_v3_write_chunk_basic() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("hello ")),
            Statement::Expr(make_write_chunk_call("world")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should have chunked transfer encoding
        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "chunked response should have Transfer-Encoding: chunked\n{}",
            response_str
        );
        // Should have HTTP/1.1 200 OK
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "chunked response should have 200 OK\n{}",
            response_str
        );
        // Should NOT have Content-Length
        assert!(
            !response_str.contains("Content-Length"),
            "chunked response should NOT have Content-Length\n{}",
            response_str
        );
        // Body should contain the chunked data
        assert!(
            response_str.contains("hello "),
            "response should contain 'hello '\n{}",
            response_str
        );
        assert!(
            response_str.contains("world"),
            "response should contain 'world'\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2c: endResponse sends chunked terminator
    #[test]
    fn test_v3_end_response_empty_body() {
        let port = v3_free_port();

        // endResponse without any writeChunk → empty chunked body
        let handler = make_streaming_handler(vec![Statement::Expr(make_end_response_call())]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "empty chunked response should have Transfer-Encoding: chunked\n{}",
            response_str
        );
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "empty chunked response should have 200 OK\n{}",
            response_str
        );
        // Should end with chunked terminator (0\r\n\r\n)
        assert!(
            response_str.contains("0\r\n\r\n"),
            "empty chunked response should have terminator\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2c: endResponse is idempotent (double call)
    #[test]
    fn test_v3_end_response_idempotent() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("data")),
            Statement::Expr(make_end_response_call()),
            Statement::Expr(make_end_response_call()), // second call is no-op
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "response should be chunked\n{}",
            response_str
        );
        assert!(
            response_str.contains("data"),
            "response should contain 'data'\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2d: Bytes fast path (zero-copy)
    #[test]
    fn test_v3_write_chunk_bytes_fast_path() {
        let port = v3_free_port();

        let bytes_data = b"binary\x00payload\xff".to_vec();
        let bytes_data_clone = bytes_data.clone();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_bytes_call(bytes_data.clone())),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            // Pre-set the bytes data in the environment
            interp
                .env
                .define_force("__test_bytes_data", Value::Bytes(bytes_data_clone));
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

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
        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "bytes response should be chunked\n{}",
            response_str
        );
        // Binary payload should be present in raw bytes
        // The chunk format is: <hex_len>\r\n<payload>\r\n
        // Our payload is b"binary\x00payload\xff" = 15 bytes → hex "f"
        assert!(
            response.windows(b"binary".len()).any(|w| w == b"binary"),
            "response should contain binary data"
        );
        assert!(
            response.windows(b"payload".len()).any(|w| w == b"payload"),
            "response should contain 'payload' binary data"
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2e: Str chunk without aggregate buffer
    #[test]
    fn test_v3_write_chunk_str_no_aggregate() {
        let port = v3_free_port();

        // Multiple string chunks to verify no aggregate buffer
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("chunk1")),
            Statement::Expr(make_write_chunk_call("chunk2")),
            Statement::Expr(make_write_chunk_call("chunk3")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "str chunks response should be chunked\n{}",
            response_str
        );
        // All chunks should be present
        assert!(
            response_str.contains("chunk1"),
            "response should contain chunk1\n{}",
            response_str
        );
        assert!(
            response_str.contains("chunk2"),
            "response should contain chunk2\n{}",
            response_str
        );
        assert!(
            response_str.contains("chunk3"),
            "response should contain chunk3\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2f: startResponse + writeChunk + endResponse full flow
    #[test]
    fn test_v3_start_response_write_chunk_end_response() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(
                201,
                vec![("X-Custom".into(), "streaming-test".into())],
            )),
            Statement::Expr(make_write_chunk_call("first")),
            Statement::Expr(make_write_chunk_call("second")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Custom status
        assert!(
            response_str.contains("HTTP/1.1 201 Created"),
            "response should have 201 Created\n{}",
            response_str
        );
        // Custom header
        assert!(
            response_str.contains("X-Custom: streaming-test"),
            "response should have custom header\n{}",
            response_str
        );
        // Chunked
        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "response should be chunked\n{}",
            response_str
        );
        // Body chunks
        assert!(
            response_str.contains("first"),
            "response should contain 'first'\n{}",
            response_str
        );
        assert!(
            response_str.contains("second"),
            "response should contain 'second'\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2f: writeChunk empty data is no-op
    #[test]
    fn test_v3_write_chunk_empty_is_noop() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("before")),
            Statement::Expr(make_write_chunk_call("")), // empty = no-op
            Statement::Expr(make_write_chunk_call("after")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        assert!(
            response_str.contains("before"),
            "response should contain 'before'\n{}",
            response_str
        );
        assert!(
            response_str.contains("after"),
            "response should contain 'after'\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2f: auto-end when handler returns without endResponse
    #[test]
    fn test_v3_auto_end_on_handler_return() {
        let port = v3_free_port();

        // Handler writes chunks but doesn't call endResponse → auto-end
        let handler = make_streaming_handler(vec![Statement::Expr(make_write_chunk_call(
            "auto-end-data",
        ))]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "auto-end response should be chunked\n{}",
            response_str
        );
        assert!(
            response_str.contains("auto-end-data"),
            "response should contain body\n{}",
            response_str
        );
        // Should have terminator (auto-ended)
        assert!(
            response_str.contains("0\r\n\r\n"),
            "auto-ended response should have terminator\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-2f: reserved header rejection in startResponse
    #[test]
    fn test_v3_reserved_header_rejection_in_handler() {
        let port = v3_free_port();

        // Handler calls startResponse with Content-Length → should error
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(
                200,
                vec![("Content-Length".into(), "42".into())],
            )),
            Statement::Expr(make_write_chunk_call("data")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should get 500 error because startResponse rejected Content-Length
        assert!(
            response_str.contains("500"),
            "reserved header should cause 500 error\n{}",
            response_str
        );
        assert!(
            response_str.contains("Content-Length"),
            "error should mention Content-Length\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-2f: bodyless status rejects writeChunk
    #[test]
    fn test_v3_bodyless_status_rejects_write_chunk() {
        let port = v3_free_port();

        // Handler calls startResponse(204) then writeChunk → should error
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(204, vec![])),
            Statement::Expr(make_write_chunk_call("body-not-allowed")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should get 500 error because 204 does not allow body
        assert!(
            response_str.contains("500"),
            "bodyless status should cause 500 error\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-2f: Verify chunked wire format correctness
    #[test]
    fn test_v3_chunked_wire_format() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("AB")), // 2 bytes → hex "2"
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

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
        // Verify exact chunked wire format:
        // "2\r\nAB\r\n0\r\n\r\n"
        assert!(
            response_str.contains("2\r\nAB\r\n"),
            "chunk should be '2\\r\\nAB\\r\\n', got:\n{}",
            response_str
        );
        assert!(
            response_str.ends_with("0\r\n\r\n"),
            "response should end with '0\\r\\n\\r\\n', got:\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-2f: implicit head commit on first writeChunk
    #[test]
    fn test_v3_implicit_head_commit() {
        let port = v3_free_port();

        // No startResponse → writeChunk should auto-commit 200/@[]
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("implicit-head")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should default to 200 OK
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "implicit head should default to 200 OK\n{}",
            response_str
        );
        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "implicit head should include chunked TE\n{}",
            response_str
        );
        assert!(
            response_str.contains("implicit-head"),
            "response should contain body\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // ── Phase 3: SSE tests ──────────────────────────────────────

    // NET3-3a: sseEvent sends SSE wire format
    #[test]
    fn test_v3_sse_event_basic() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_sse_event_call("message", "hello")),
            Statement::Expr(make_sse_event_call("message", "world")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should have SSE content type
        assert!(
            response_str.contains("Content-Type: text/event-stream; charset=utf-8"),
            "SSE response should have text/event-stream Content-Type\n{}",
            response_str
        );
        // Should have Cache-Control: no-cache
        assert!(
            response_str.contains("Cache-Control: no-cache"),
            "SSE response should have Cache-Control: no-cache\n{}",
            response_str
        );
        // Should have Transfer-Encoding: chunked
        assert!(
            response_str.contains("Transfer-Encoding: chunked"),
            "SSE response should have chunked TE\n{}",
            response_str
        );
        // Should contain SSE wire format
        assert!(
            response_str.contains("event: message\ndata: hello\n\n"),
            "SSE response should contain first event wire format\n{}",
            response_str
        );
        assert!(
            response_str.contains("event: message\ndata: world\n\n"),
            "SSE response should contain second event wire format\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-3b: Content-Type auto-setting
    #[test]
    fn test_v3_sse_auto_content_type() {
        let port = v3_free_port();

        // startResponse with no Content-Type header, then sseEvent → auto-set
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(200, vec![])),
            Statement::Expr(make_sse_event_call("ping", "pong")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        assert!(
            response_str.contains("Content-Type: text/event-stream; charset=utf-8"),
            "sseEvent should auto-set Content-Type\n{}",
            response_str
        );
        assert!(
            response_str.contains("Cache-Control: no-cache"),
            "sseEvent should auto-set Cache-Control\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3b: Content-Type is NOT overwritten if user already set it
    #[test]
    fn test_v3_sse_respects_user_content_type() {
        let port = v3_free_port();

        // startResponse with explicit Content-Type → sseEvent should NOT override
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(
                200,
                vec![("Content-Type".to_string(), "text/plain".to_string())],
            )),
            Statement::Expr(make_sse_event_call("test", "data")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should have the user-specified Content-Type, NOT the auto-set SSE one
        assert!(
            response_str.contains("Content-Type: text/plain"),
            "User Content-Type should be preserved\n{}",
            response_str
        );
        assert!(
            !response_str.contains("text/event-stream"),
            "Auto Content-Type should NOT override user-set Content-Type\n{}",
            response_str
        );
        // But Cache-Control should still be auto-set (user didn't set it)
        assert!(
            response_str.contains("Cache-Control: no-cache"),
            "Cache-Control should still be auto-set\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3d: multiline data splitting
    #[test]
    fn test_v3_sse_multiline_data() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_sse_event_call("update", "line1\nline2\nline3")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Multiline data should be split into multiple data: lines
        assert!(
            response_str.contains("event: update\ndata: line1\ndata: line2\ndata: line3\n\n"),
            "multiline data should be split into data: lines\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3d: empty event name omits event: line
    #[test]
    fn test_v3_sse_empty_event_name() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_sse_event_call("", "noname")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should NOT contain "event:" line when event name is empty
        assert!(
            !response_str.contains("event:"),
            "empty event name should omit event: line\n{}",
            response_str
        );
        // Should contain "data: noname\n\n"
        assert!(
            response_str.contains("data: noname\n\n"),
            "data should still be sent\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3f: sseEvent outside handler context
    #[test]
    fn test_v3_sse_event_outside_handler() {
        let mut interp = Interpreter::new();
        // Set sentinel but not inside handler
        interp
            .env
            .define_force("sseEvent", Value::Str("__net_builtin_sseEvent".into()));
        let args: Vec<Expr> = vec![
            Expr::StringLit("writer".into(), dummy_span()),
            Expr::StringLit("message".into(), dummy_span()),
            Expr::StringLit("hello".into(), dummy_span()),
        ];
        let result = interp.try_net_func("sseEvent", &args);
        assert!(result.is_err(), "sseEvent should error outside handler");
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("2-argument httpServe handler"),
            "error should mention 2-arg handler: {}",
            msg
        );
    }

    // NET3-3f: sseEvent after endResponse
    #[test]
    fn test_v3_sse_event_after_end() {
        let port = v3_free_port();

        // endResponse first, then sseEvent → should error
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_end_response_call()),
            Statement::Expr(make_sse_event_call("message", "after-end")),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            // This will result in a runtime error from sseEvent after endResponse,
            // but httpServe catches handler errors and returns an error response.
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // The server should still respond (handler error is caught by httpServe).
        // The response might be a 500 error or the chunked response that was already sent.
        assert!(
            !response_str.is_empty(),
            "server should respond even when handler has SSE error"
        );

        let _ = server_handle.join();
    }

    // NET3-3f: SSE with implicit head commit (no startResponse)
    #[test]
    fn test_v3_sse_implicit_head() {
        let port = v3_free_port();

        // No startResponse → sseEvent should auto-commit 200 with SSE headers
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_sse_event_call("message", "auto-head")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should have 200 OK
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "implicit head should default to 200 OK\n{}",
            response_str
        );
        // Should have SSE headers auto-set
        assert!(
            response_str.contains("Content-Type: text/event-stream; charset=utf-8"),
            "auto-head should include SSE Content-Type\n{}",
            response_str
        );
        assert!(
            response_str.contains("Cache-Control: no-cache"),
            "auto-head should include Cache-Control\n{}",
            response_str
        );
        // Should contain the SSE data
        assert!(
            response_str.contains("data: auto-head\n\n"),
            "response should contain SSE data\n{}",
            response_str
        );

        let result = server_handle.join().unwrap();
        match result {
            Signal::Value(Value::Async(a)) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
            }
            _ => panic!("expected fulfilled Async"),
        }
    }

    // NET3-3f: SSE mixed with writeChunk
    #[test]
    fn test_v3_sse_mixed_with_write_chunk() {
        let port = v3_free_port();

        // Mix sseEvent and writeChunk — both should work on the same chunked stream
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_sse_event_call("greet", "hi")),
            Statement::Expr(make_write_chunk_call("raw-chunk")),
            Statement::Expr(make_sse_event_call("bye", "bye")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Should contain SSE event
        assert!(
            response_str.contains("event: greet\ndata: hi\n\n"),
            "first SSE event should be present\n{}",
            response_str
        );
        // Should contain raw chunk
        assert!(
            response_str.contains("raw-chunk"),
            "raw writeChunk data should be present\n{}",
            response_str
        );
        // Should contain second SSE event
        assert!(
            response_str.contains("event: bye\ndata: bye\n\n"),
            "second SSE event should be present\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3f: writeChunk before sseEvent → error (head committed without SSE headers)
    #[test]
    fn test_v3_write_chunk_then_sse_event_errors() {
        let port = v3_free_port();

        // writeChunk commits head without SSE headers, then sseEvent should error
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_write_chunk_call("raw-first")),
            Statement::Expr(make_sse_event_call("message", "after-chunk")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // The handler should have errored on the sseEvent call — httpServe catches
        // runtime errors and returns a 500 or similar response.
        // The raw-first chunk was already sent before the error, so the response
        // will contain it. The SSE event should NOT be present.
        assert!(
            !response_str.contains("event: message"),
            "sseEvent should not succeed after writeChunk committed head\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3f: startResponse with Content-Type only (no Cache-Control) + writeChunk + sseEvent → error
    #[test]
    fn test_v3_content_type_only_then_write_chunk_then_sse_event_errors() {
        let port = v3_free_port();

        // User sets Content-Type but NOT Cache-Control, then writeChunk commits head.
        // sseEvent should still error because Cache-Control: no-cache is also required.
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(
                200,
                vec![(
                    "Content-Type".to_string(),
                    "text/event-stream; charset=utf-8".to_string(),
                )],
            )),
            Statement::Expr(make_write_chunk_call("raw-first")),
            Statement::Expr(make_sse_event_call("message", "after-chunk")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // sseEvent should fail — Content-Type alone is not enough, Cache-Control is also required
        assert!(
            !response_str.contains("event: message"),
            "sseEvent should not succeed when only Content-Type is set (Cache-Control missing)\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3f: startResponse with explicit SSE headers + writeChunk + sseEvent → OK
    #[test]
    fn test_v3_explicit_sse_headers_then_write_chunk_then_sse() {
        let port = v3_free_port();

        // User explicitly sets SSE headers via startResponse, then writeChunk, then sseEvent.
        // This should work because the user took responsibility for setting SSE headers.
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(
                200,
                vec![
                    (
                        "Content-Type".to_string(),
                        "text/event-stream; charset=utf-8".to_string(),
                    ),
                    ("Cache-Control".to_string(), "no-cache".to_string()),
                ],
            )),
            Statement::Expr(make_write_chunk_call("preamble")),
            Statement::Expr(make_sse_event_call("update", "data-after-chunk")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // Verify SSE headers are present in the response head
        assert!(
            response_str.contains("text/event-stream"),
            "explicit SSE Content-Type should be in response\n{}",
            response_str
        );
        assert!(
            response_str.contains("no-cache"),
            "explicit Cache-Control should be in response\n{}",
            response_str
        );

        // The preamble chunk should be present
        assert!(
            response_str.contains("preamble"),
            "preamble writeChunk should be present\n{}",
            response_str
        );

        // The SSE event should be present (sse_mode was set by explicit headers check)
        assert!(
            response_str.contains("event: update\ndata: data-after-chunk\n\n"),
            "sseEvent should succeed when SSE headers were explicitly set\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3f: SSE wire format exactness
    #[test]
    fn test_v3_sse_wire_format_exact() {
        let port = v3_free_port();

        let handler = make_streaming_handler(vec![
            Statement::Expr(make_sse_event_call("tick", "42")),
            Statement::Expr(make_end_response_call()),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

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

        // SSE event is sent as one chunked frame via vectored I/O.
        // The payload "event: tick\ndata: 42\n\n" is 23 bytes → hex "17"
        let expected_sse = "event: tick\ndata: 42\n\n";
        let expected_hex = format!("{:x}", expected_sse.len());
        let expected_chunk = format!("{}\r\n{}\r\n", expected_hex, expected_sse);
        assert!(
            response_str.contains(&expected_chunk),
            "SSE chunk wire format mismatch.\nExpected chunk: {:?}\nGot:\n{}",
            expected_chunk,
            response_str
        );
        // Should end with chunked terminator
        assert!(
            response_str.ends_with("0\r\n\r\n"),
            "response should end with chunked terminator\n{}",
            response_str
        );

        let _ = server_handle.join();
    }

    // NET3-3f: SSE bodyless status rejection
    #[test]
    fn test_v3_sse_bodyless_status_rejected() {
        let port = v3_free_port();

        // startResponse with 204 (No Content), then sseEvent → should error
        let handler = make_streaming_handler(vec![
            Statement::Expr(make_start_response_call(204, vec![])),
            Statement::Expr(make_sse_event_call("message", "should-fail")),
        ]);

        let server_port = port;
        let server_handle = std::thread::spawn(move || {
            let mut interp = Interpreter::new();
            setup_v3_sentinels(&mut interp);
            interp.env.define_force("__handler", handler);
            let args = vec![
                Expr::IntLit(server_port as i64, dummy_span()),
                Expr::Ident("__handler".into(), dummy_span()),
                Expr::IntLit(1, dummy_span()),
                Expr::IntLit(5000, dummy_span()),
            ];
            interp.try_net_func("httpServe", &args).unwrap().unwrap()
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        std::io::Write::write_all(
            &mut client,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .unwrap();

        let response_str = read_full_response(&mut client);

        // The handler error should be caught by httpServe — we should get some response
        assert!(
            !response_str.is_empty(),
            "server should respond even with bodyless status SSE error"
        );

        let _ = server_handle.join();
    }

    // ── v5 Phase 1 tests ──

    #[test]
    fn test_net_symbols_includes_ws_close_code() {
        assert!(
            NET_SYMBOLS.contains(&"wsCloseCode"),
            "NET_SYMBOLS must include wsCloseCode (v5)"
        );
    }

    #[test]
    fn test_ws_close_code_not_in_ws_state() {
        // wsCloseCode outside handler should fail.
        let mut interp = Interpreter::new();
        interp.env.define_force(
            "wsCloseCode",
            Value::Str("__net_builtin_wsCloseCode".into()),
        );
        let args = vec![Expr::Ident("dummy".into(), dummy_span())];
        let result = interp.try_net_func("wsCloseCode", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("wsCloseCode"));
    }

    #[test]
    fn test_ws_close_dispatch_with_code_arg() {
        // wsClose with 2 args (ws, code) dispatches correctly.
        // Out of handler context, the handler-context guard rejects the call
        // before reaching close code validation. This only verifies dispatch
        // routing — actual close code validation is covered by parity tests
        // (test_net5_5a_ws_close_reserved_codes_interp, test_net5_5a_ws_close_out_of_range_interp).
        for code in &[1004i64, 1005, 1006, 1015, 0, 999, 5000, -1] {
            let mut interp = Interpreter::new();
            interp
                .env
                .define_force("wsClose", Value::Str("__net_builtin_wsClose".into()));
            let args = vec![
                Expr::Ident("dummy".into(), dummy_span()),
                Expr::IntLit(*code, dummy_span()),
            ];
            let result = interp.try_net_func("wsClose", &args);
            assert!(
                result.is_err(),
                "wsClose({}) should fail outside handler context",
                code
            );
        }
    }

    #[test]
    fn test_http_serve_tls_arg_empty_pack_accepted() {
        // httpServe with tls <= @() should be accepted (plaintext mode).
        // Define test_handler as a non-Function value to trigger the handler type check.
        // This proves the tls arg parsing (index 5) succeeds without interference.
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        interp.env.define_force("test_handler", Value::Int(42));

        let args = vec![
            Expr::IntLit(0, dummy_span()),
            Expr::Ident("test_handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(100, dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::BuchiPack(vec![], dummy_span()), // tls = @()
        ];

        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("handler must be a Function"),
            "Should fail on handler, not on tls arg. Got: {}",
            msg
        );
    }

    #[test]
    fn test_http_serve_tls_arg_non_pack_rejected() {
        // httpServe with tls as non-BuchiPack should be rejected.
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));

        // Define a dummy handler function.
        interp.env.define_force(
            "test_handler",
            Value::Function(super::super::value::FuncValue {
                name: "test_handler".into(),
                params: vec![Param {
                    name: "req".into(),
                    default_value: None,
                    type_annotation: None,
                    span: dummy_span(),
                }],
                body: vec![],
                closure: std::sync::Arc::new(std::collections::HashMap::new()),
                return_type: None,
            }),
        );

        let args = vec![
            Expr::IntLit(0, dummy_span()),
            Expr::Ident("test_handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(100, dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(42, dummy_span()), // tls = 42 (not a BuchiPack)
        ];

        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("tls must be a BuchiPack"),
            "Should reject non-BuchiPack tls. Got: {}",
            msg
        );
    }

    #[test]
    fn test_http_serve_tls_cert_key_returns_phase2_error() {
        // httpServe with tls = @(cert: "x", key: "y") should return TlsError (Phase 2 not ready).
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));

        interp.env.define_force(
            "test_handler",
            Value::Function(super::super::value::FuncValue {
                name: "test_handler".into(),
                params: vec![Param {
                    name: "req".into(),
                    default_value: None,
                    type_annotation: None,
                    span: dummy_span(),
                }],
                body: vec![],
                closure: std::sync::Arc::new(std::collections::HashMap::new()),
                return_type: None,
            }),
        );

        let args = vec![
            Expr::IntLit(0, dummy_span()),
            Expr::Ident("test_handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(100, dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::BuchiPack(
                vec![
                    BuchiField {
                        name: "cert".into(),
                        value: Expr::StringLit("cert.pem".into(), dummy_span()),
                        span: dummy_span(),
                    },
                    BuchiField {
                        name: "key".into(),
                        value: Expr::StringLit("key.pem".into(), dummy_span()),
                        span: dummy_span(),
                    },
                ],
                dummy_span(),
            ),
        ];

        let result = interp.try_net_func("httpServe", &args);
        // Should succeed (return a Signal::Value with Async), but with a TlsError result.
        assert!(result.is_ok());
        let signal = result.unwrap();
        assert!(signal.is_some());
        match signal.unwrap() {
            Signal::Value(Value::Async(async_val)) => {
                // Should be a fulfilled Async with a Result failure.
                assert_eq!(async_val.status, AsyncStatus::Fulfilled);
                // The inner value should be a Result with TlsError.
                match &*async_val.value {
                    Value::BuchiPack(fields) => {
                        let kind =
                            fields
                                .iter()
                                .find(|(k, _)| k == "__value")
                                .and_then(|(_, v)| {
                                    if let Value::BuchiPack(inner) = v {
                                        inner
                                            .iter()
                                            .find(|(k, _)| k == "kind")
                                            .map(|(_, v)| v.clone())
                                    } else {
                                        None
                                    }
                                });
                        assert_eq!(
                            kind,
                            Some(Value::Str("TlsError".into())),
                            "Should have TlsError kind"
                        );
                    }
                    other => {
                        panic!("Expected BuchiPack (Result), got: {:?}", other);
                    }
                }
            }
            other => {
                panic!("Expected Async value, got: {:?}", other);
            }
        }
    }
}
