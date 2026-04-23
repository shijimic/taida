/// Free helper functions for net_eval (C12B-025 mechanical split).
///
/// This file contains all free functions extracted from net_eval.rs:
///   - HTTP response head builder / reason phrase map
///   - Scatter-gather write helpers (write_all_retry, write_vectored_all)
///   - Result BuchiPack helpers (make_result_*, extract_result_value*, get_field_*)
///   - HTTP request head parser + build_parse_result
///   - Chunked transfer-encoding helpers (chunked_in_place_compact, chunked_body_complete)
///   - Keep-alive determination
///   - HTTP response encoder
///   - Request body helpers (is_body_stream_request, extract_body_token, eval_read_body)
use super::super::eval::RuntimeError;
use super::super::value::{AsyncStatus, AsyncValue, ErrorValue, Value};
use super::types::{ConnStream, ResponseFields, StreamingWriter};

/// Build the HTTP response head bytes for a streaming response.
///
/// For normal status codes: appends `Transfer-Encoding: chunked`.
/// For bodyless status codes (1xx/204/205/304): omits `Transfer-Encoding`
/// since no message body is allowed.
///
/// This is the head commit function. Once called, status/headers are on the wire
/// and cannot be changed.
pub(crate) fn build_streaming_head(status: u16, headers: &[(String, String)]) -> Vec<u8> {
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
pub(crate) fn http_reason_phrase(status: u16) -> &'static str {
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
pub(crate) fn write_all_retry(stream: &mut ConnStream, data: &[u8]) -> Result<(), RuntimeError> {
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
pub(crate) fn write_vectored_all(
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

pub(crate) fn make_result_success(inner: Value) -> Value {
    Value::BuchiPack(vec![
        ("__value".into(), inner),
        ("throw".into(), Value::Unit),
        ("__predicate".into(), Value::Unit),
        ("__type".into(), Value::Str("Result".into())),
    ])
}

pub(crate) fn make_result_failure_msg(kind: &str, message: impl Into<String>) -> Value {
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

pub(crate) fn make_span(start: usize, len: usize) -> Value {
    Value::BuchiPack(vec![
        ("start".into(), Value::Int(start as i64)),
        ("len".into(), Value::Int(len as i64)),
    ])
}

// ── Async / value helpers ──────────────────────────────────

/// Wrap a value in a fulfilled Async envelope.
pub(crate) fn make_fulfilled_async(value: Value) -> Value {
    Value::Async(AsyncValue {
        status: AsyncStatus::Fulfilled,
        value: Box::new(value),
        error: Box::new(Value::Unit),
        task: None,
    })
}

/// Extract the __value from a Result BuchiPack, returning None on failure.
pub(crate) fn extract_result_value(result: &Value) -> Option<&Vec<(String, Value)>> {
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
pub(crate) fn extract_result_value_owned(result: Value) -> Option<Vec<(String, Value)>> {
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
pub(crate) fn get_field_bool(fields: &[(String, Value)], key: &str) -> Option<bool> {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Bool(b))) => Some(*b),
        _ => None,
    }
}

/// Get an Int field from a BuchiPack field list.
pub(crate) fn get_field_int(fields: &[(String, Value)], key: &str) -> Option<i64> {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Int(n))) => Some(*n),
        _ => None,
    }
}

/// Get a Str field from a BuchiPack field list.
pub(crate) fn get_field_str(fields: &[(String, Value)], key: &str) -> Option<String> {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Str(s))) => Some(s.clone()),
        _ => None,
    }
}

/// Get a reference to any field value from a BuchiPack field list.
pub(crate) fn get_field_value<'a>(fields: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

// ── httpParseRequestHead ────────────────────────────────────

/// Parse HTTP/1.1 request head from raw bytes.
/// Returns Result[@(complete, consumed, method, path, query, version, headers, bodyOffset, contentLength), _]
pub(crate) fn parse_request_head(bytes: &[u8]) -> Value {
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

pub(crate) fn build_parse_result(
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
        ("headers".into(), Value::list(headers_list)),
        ("bodyOffset".into(), Value::Int(consumed as i64)),
        ("contentLength".into(), Value::Int(content_length)),
        ("chunked".into(), Value::Bool(has_transfer_encoding_chunked)),
        // C12B-032 / FB-2: internal-only presence bit. The v1 Taida
        // surface flattens "header absent" and "Content-Length: 0"
        // into the single `contentLength: 0` field; this extra bit
        // lets the internal `BodyEncoding` preserve the distinction.
        // Handlers ignore the field (unused name, prefixed with `__`
        // by convention), so the handler-visible surface is unchanged.
        (
            "__hasContentLengthHeader".into(),
            Value::Bool(has_content_length),
        ),
    ]);

    make_result_success(parsed)
}

/// Trim leading/trailing ASCII whitespace from a byte slice.
pub(crate) fn trim_ascii(bytes: &[u8]) -> &[u8] {
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
pub(crate) struct ChunkedCompactResult {
    /// Total compacted body length (bytes written to body region).
    pub(crate) body_len: usize,
    /// Total wire bytes consumed from `body_offset` (including framing).
    /// Used by keep-alive `advance()` to skip the right amount.
    pub(crate) wire_consumed: usize,
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
pub(crate) fn chunked_in_place_compact(
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
pub(crate) fn find_crlf(data: &[u8]) -> Option<usize> {
    if data.len() < 2 {
        return None;
    }
    (0..data.len() - 1).find(|&i| data[i] == b'\r' && data[i + 1] == b'\n')
}

/// Check if the buffer contains a complete chunked body (read-only scan).
/// NB2-15: Typed error for chunked body parsing — avoids string prefix matching.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum ChunkedBodyError {
    /// Need more data (incomplete chunk framing)
    Incomplete(String),
    /// Malformed chunk data (reject immediately)
    Malformed(String),
}

///
/// Walks the chunk framing without modifying the buffer.
/// Returns `Ok(wire_consumed)` if the terminator chunk was found,
/// or `Err(ChunkedBodyError)` if the data is incomplete or malformed.
pub(crate) fn chunked_body_complete(
    buf: &[u8],
    body_offset: usize,
) -> Result<usize, ChunkedBodyError> {
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
pub(crate) fn determine_keep_alive(raw: &[u8], headers: &[Value], http_minor: i64) -> bool {
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
pub(crate) fn encode_response(response: &Value) -> Value {
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
pub(crate) fn send_response_scatter(
    stream: &mut ConnStream,
    response: &Value,
) -> Result<(), String> {
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

pub(crate) fn extract_response_fields(response: &Value) -> Result<ResponseFields, String> {
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

pub(crate) fn status_reason(code: i64) -> &'static str {
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
pub(crate) fn is_body_stream_request(req: &Value) -> bool {
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
pub(crate) fn extract_body_token(req: &Value) -> Option<u64> {
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
pub(crate) fn eval_read_body(req: &Value) -> Result<Value, RuntimeError> {
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
