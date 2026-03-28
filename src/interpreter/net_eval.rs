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
/// Legacy (16) + HTTP v1 (3) + HTTP v2 (1) + HTTP v3 (4) = 24 symbols.
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
    pub stream: *mut std::net::TcpStream,
    /// NB3-9: Re-entrancy guard. Set to true while a streaming API function
    /// holds `&mut *self.writer` / `&mut *self.stream`. If another streaming
    /// API call is attempted while this is true, it returns a RuntimeError
    /// instead of creating a second `&mut` to the same pointee.
    pub borrowed: bool,
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
    let reason = http_reason_phrase(status);
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", status, reason).as_bytes());
    for (name, value) in headers {
        buf.extend_from_slice(format!("{}: {}\r\n", name, value).as_bytes());
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

/// Write all bytes to a std::net::TcpStream, retrying on partial writes.
fn write_all_retry(stream: &mut std::net::TcpStream, data: &[u8]) -> Result<(), RuntimeError> {
    use std::io::Write;
    stream.write_all(data).map_err(|e| RuntimeError {
        message: format!("streaming write error: {}", e),
    })
}

/// Write multiple IoSlice buffers to a stream without creating an aggregate buffer.
/// Uses vectored write (writev) where available, falling back to sequential writes.
fn write_vectored_all(
    stream: &mut std::net::TcpStream,
    bufs: &[std::io::IoSlice<'_>],
) -> Result<(), RuntimeError> {
    use std::io::Write;
    // std::net::TcpStream supports write_vectored. We need to handle partial writes.
    // For simplicity and correctness, write each buffer sequentially via write_all.
    // This still avoids aggregate buffer creation — each buffer is sent as-is.
    // The kernel may coalesce these via Nagle or TCP_CORK.
    for buf in bufs {
        stream.write_all(buf).map_err(|e| RuntimeError {
            message: format!("streaming write error: {}", e),
        })?;
    }
    Ok(())
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
            buf.extend_from_slice(format!("Content-Length: {}\r\n", body_bytes.len()).as_bytes());
        }
    }

    buf.extend_from_slice(b"\r\n");
    if !no_body {
        buf.extend_from_slice(&body_bytes);
    }

    let result = Value::BuchiPack(vec![("bytes".into(), Value::Bytes(buf))]);
    make_result_success(result)
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

/// Per-connection state for the concurrent httpServe pool.
/// Each connection owns its own scratch buffer (no sharing).
struct HttpConnection {
    stream: std::net::TcpStream,
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
                Ok(Some(Signal::Value(eval_read_body(&req)?)))
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

        // State check: writeChunk is not valid after endResponse.
        if writer.state == WriterState::Ended {
            return Err(RuntimeError {
                message: "writeChunk: response already ended.".into(),
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

        // State check: sseEvent is not valid after endResponse.
        if writer.state == WriterState::Ended {
            return Err(RuntimeError {
                message: "sseEvent: response already ended.".into(),
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

    // ── httpServe implementation ───────────────────────────────
    //
    // httpServe(port, handler, maxRequests <= 0, timeoutMs <= 5000, maxConnections <= 128)
    //   → Async[Result[@(ok: Bool, requests: Int), _]]
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
                    Ok((stream, peer_addr)) => {
                        // Set short read timeout for polling readiness
                        let _ = stream.set_read_timeout(Some(poll_timeout));
                        connections.push(HttpConnection {
                            stream,
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

        // ── Body reading: Content-Length vs Chunked Transfer-Encoding ──
        let body_result = if is_chunked {
            // ── NET2-2: Chunked Transfer Encoding ──
            let completeness = loop {
                let check = chunked_body_complete(&conn.buf[..conn.total_read], head_consumed);
                match check {
                    Ok(wire_used) => break Ok(wire_used),
                    // NB2-15: Use typed enum instead of string prefix matching
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
                            Ok(0) => break Err("Chunked body incomplete: connection closed".into()),
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
                Ok(_scan_wire) => match chunked_in_place_compact(&mut conn.buf, head_consumed) {
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
                },
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
                let bad_request =
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
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

        // ── Determine keep-alive (NET2-1a/1b/1c) ──
        let http_minor = match get_field_value(&parsed_fields, "version") {
            Some(Value::BuchiPack(ver_fields)) => get_field_int(ver_fields, "minor").unwrap_or(1),
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

        // ── NET3-1a: Detect handler arity (1-arg vs 2-arg) ──
        // 1-arg handler = v2 one-shot response path (unchanged)
        // 2-arg handler = streaming writer path (v3)
        let handler_arity = handler.params.len();

        if handler_arity >= 2 {
            // ── v3 2-arg handler path ──
            // Create a writer BuchiPack with a sentinel for identification.
            // The actual mutable StreamingWriter state is held on the stack here;
            // the writer Value is an opaque token passed to the handler.
            let writer_pack = Value::BuchiPack(vec![(
                "__writer_id".into(),
                Value::Str("__v3_streaming_writer".into()),
            )]);

            // Create a mutable StreamingWriter for this request scope.
            let mut writer = StreamingWriter::new();

            // NET3-2: Install active_streaming_writer so that startResponse/writeChunk/
            // endResponse can access the writer state and TcpStream during handler execution.
            // Safety: writer and conn.stream live on this stack frame and outlive the
            // call_function_with_values call. The interpreter is single-threaded.
            self.active_streaming_writer = Some(ActiveStreamingWriter {
                writer: &mut writer as *mut StreamingWriter,
                stream: &mut conn.stream as *mut std::net::TcpStream,
                borrowed: false,
            });

            let handler_result =
                self.call_function_with_values(handler, &[request_pack, writer_pack]);

            // Clear the active writer — handler execution is done.
            self.active_streaming_writer = None;

            let response_value = match handler_result {
                Ok(v) => v,
                Err(e) => {
                    if writer.state == WriterState::Streaming {
                        // Head already committed — we cannot send a new HTTP response
                        // on this connection. Send chunk terminator and drop the connection.
                        let _ = std::io::Write::write_all(&mut conn.stream, b"0\r\n\r\n");
                        writer.state = WriterState::Ended;
                        *request_count += 1;
                        return ConnAction::Close;
                    }
                    if writer.state == WriterState::Ended {
                        // Response already fully sent — just drop the connection.
                        *request_count += 1;
                        return ConnAction::Close;
                    }
                    // Head not yet committed (Idle / HeadPrepared) — safe to send a
                    // full 500 error response.
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

            // ── NET3-1a: One-shot fallback for 2-arg handler ──
            // If the handler never touched the writer (state is still Idle),
            // fall back to v2 one-shot response path using the return value.
            if writer.state == WriterState::Idle {
                // One-shot fallback: use the response_value as a v2-style response pack.
                // If it's Unit or not a response pack (handler returned nothing useful),
                // send 200 + empty body.
                let is_response_pack = matches!(&response_value, Value::BuchiPack(fields)
                    if fields.iter().any(|(k, _)| k == "status" || k == "body"));
                let effective_response = if is_response_pack {
                    response_value
                } else {
                    // Unit, Int, Str, or any non-response value → 200 + empty body
                    Value::BuchiPack(vec![
                        ("status".into(), Value::Int(200)),
                        ("headers".into(), Value::List(vec![])),
                        ("body".into(), Value::Str(String::new())),
                    ])
                };

                let encoded = encode_response(&effective_response);
                match extract_result_value(&encoded) {
                    Some(inner) => {
                        if let Some(Value::Bytes(wire_bytes)) = get_field_value(inner, "bytes") {
                            let _ = std::io::Write::write_all(&mut conn.stream, wire_bytes);
                        }
                    }
                    None => {
                        let fallback = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = std::io::Write::write_all(&mut conn.stream, fallback);
                        *request_count += 1;
                        return ConnAction::Close;
                    }
                }
            } else {
                // Streaming was started. The return value is ignored.
                // Auto-end if not already ended.
                if writer.state != WriterState::Ended {
                    // Auto endResponse: commit head if needed, send terminator.
                    if writer.state == WriterState::HeadPrepared {
                        // Commit head first.
                        let head_bytes =
                            build_streaming_head(writer.pending_status, &writer.pending_headers);
                        let _ = std::io::Write::write_all(&mut conn.stream, &head_bytes);
                    }
                    // Send chunked terminator — only for status codes that allow a body.
                    if !StreamingWriter::is_bodyless_status(writer.pending_status) {
                        let _ = std::io::Write::write_all(&mut conn.stream, b"0\r\n\r\n");
                    }
                    writer.state = WriterState::Ended;
                }
            }
        } else {
            // ── v2 1-arg handler path (unchanged) ──
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

            // ── Encode response and write back ──
            let encoded = encode_response(&response_value);
            match extract_result_value(&encoded) {
                Some(inner) => {
                    if let Some(Value::Bytes(wire_bytes)) = get_field_value(inner, "bytes") {
                        let _ = std::io::Write::write_all(&mut conn.stream, wire_bytes);
                    }
                }
                None => {
                    let fallback = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = std::io::Write::write_all(&mut conn.stream, fallback);
                    *request_count += 1;
                    return ConnAction::Close;
                }
            }
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
        // 16 legacy + 3 HTTP v1 + 1 HTTP v2 + 4 HTTP v3 = 24
        assert_eq!(NET_SYMBOLS.len(), 24);
        assert!(NET_SYMBOLS.contains(&"dnsResolve"));
        assert!(NET_SYMBOLS.contains(&"httpServe"));
        assert!(NET_SYMBOLS.contains(&"httpParseRequestHead"));
        assert!(NET_SYMBOLS.contains(&"httpEncodeResponse"));
        assert!(NET_SYMBOLS.contains(&"readBody"));
        // v3 streaming
        assert!(NET_SYMBOLS.contains(&"startResponse"));
        assert!(NET_SYMBOLS.contains(&"writeChunk"));
        assert!(NET_SYMBOLS.contains(&"endResponse"));
        assert!(NET_SYMBOLS.contains(&"sseEvent"));
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
}
