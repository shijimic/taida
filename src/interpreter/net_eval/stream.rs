//! HTTP streaming response writer (v3) and request body streaming (v4)
//! implementations, split out from `net_eval/mod.rs` (C13-3).
//!
//! This module owns the `impl Interpreter` methods for the v3 streaming
//! API (`startResponse` / `writeChunk` / `endResponse` / `sseEvent`) and
//! the v4 body streaming API (`readBodyChunk` / `readBodyAll`) together
//! with the chunked-body helpers they depend on.
//!
//! C13-3 note: pure mechanical move — no behavior change. The `try_net_func`
//! dispatcher in `mod.rs` continues to route these calls; this file merely
//! hosts the implementations.

use super::super::eval::{Interpreter, RuntimeError, Signal};
use super::super::value::Value;
use super::helpers::{build_streaming_head, get_field_str, write_all_retry, write_vectored_all};
use super::types::{
    ActiveStreamingWriter, BodyEncoding, ChunkedDecoderState, ConnStream, RequestBodyState,
    StreamingWriter, WriterState,
};
use crate::parser::Expr;

impl Interpreter {
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
    pub(super) fn validate_writer_token(
        &mut self,
        args: &[Expr],
        api_name: &str,
    ) -> Result<(), RuntimeError> {
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
                    k == "__writer_id"
                        && matches!(v, Value::Str(s) if s.as_str() == "__v3_streaming_writer")
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

    fn active_streaming_writer_for(
        &self,
        api_name: &str,
    ) -> Result<&ActiveStreamingWriter, RuntimeError> {
        self.active_streaming_writer
            .as_ref()
            .ok_or_else(|| RuntimeError {
                message: format!(
                    "{}: active streaming writer disappeared during handler execution",
                    api_name
                ),
            })
    }

    /// `startResponse(writer, status <= 200, headers <= @[])`
    ///
    /// Updates pending status/headers on the StreamingWriter.
    /// Does NOT commit to wire — that happens on first writeChunk/endResponse.
    pub(super) fn eval_start_response(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // Check we're inside a 2-arg handler first (before token validation).
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "startResponse: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "startResponse")?;

        let active = self.active_streaming_writer_for("startResponse")?;

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
                    for item in items.iter() {
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
        let active = self.active_streaming_writer_for("startResponse")?;
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
    pub(super) fn eval_write_chunk(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "writeChunk: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "writeChunk")?;

        let active = self.active_streaming_writer_for("writeChunk")?;

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
        let active = self.active_streaming_writer_for("writeChunk")?;
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
    pub(super) fn eval_end_response(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "endResponse: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate writer token.
        self.validate_writer_token(args, "endResponse")?;

        let active = self.active_streaming_writer_for("endResponse")?;

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
    pub(super) fn eval_sse_event(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
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
                Signal::Value(Value::Str(s)) => Value::str_take(s),
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
                Signal::Value(Value::Str(s)) => Value::str_take(s),
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
        let active = self.active_streaming_writer_for("sseEvent")?;
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
    pub(super) fn make_lax_bytes_value(data: Vec<u8>) -> Value {
        Value::pack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__value".into(), Value::bytes(data)),
            ("__default".into(), Value::bytes(vec![])),
            ("__type".into(), Value::str("Lax".into())),
        ])
    }

    /// Build a Lax[Bytes] empty (hasValue = false).
    pub(super) fn make_lax_bytes_empty() -> Value {
        Value::pack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("__value".into(), Value::bytes(vec![])),
            ("__default".into(), Value::bytes(vec![])),
            ("__type".into(), Value::str("Lax".into())),
        ])
    }

    /// `readBodyChunk(req)` implementation.
    ///
    /// Reads the next chunk of request body from the TcpStream.
    /// Returns Lax[Bytes] with the chunk data, or Lax empty when body is done.
    ///
    /// Zero-copy contract: each chunk is returned independently; no aggregate buffer.
    pub(super) fn eval_read_body_chunk_impl(&mut self) -> Result<Option<Signal>, RuntimeError> {
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

        // C12-12 / FB-2: dispatch off the canonical `BodyEncoding`
        // enum rather than the redundant `is_chunked` / zero-length
        // pair. The `Empty` arm is already short-circuited above via
        // `body.fully_read`, so reaching here with `Empty` would be a
        // state-machine bug — we defensively fall through to the
        // Content-Length path where `remaining == 0` ends the stream
        // cleanly. The match shape also documents the eventual v2
        // addition (e.g. HTTP/2 DATA framing) without churning the
        // surrounding logic.
        match body.body_encoding {
            BodyEncoding::Chunked => {
                // ── NET4-1b: Chunked TE decode ──
                Self::read_body_chunk_chunked(body, stream)
            }
            BodyEncoding::ContentLength(_) | BodyEncoding::Empty { .. } => {
                // ── NET4-1c: Content-Length body ──
                Self::read_body_chunk_content_length(body, stream)
            }
        }
    }

    /// Read one chunk from a chunked transfer-encoded body.
    pub(super) fn read_body_chunk_chunked(
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
    pub(super) fn read_body_chunk_content_length(
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
    pub(super) fn drain_chunked_trailers(
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
    pub(super) fn read_line_from_body(
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
    pub(super) fn trim_bytes(data: &[u8]) -> &[u8] {
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
    pub(super) fn parse_chunk_size_bytes(line: &[u8]) -> Option<usize> {
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
    pub(super) fn read_exact_from_body(
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
    pub(super) fn eval_read_body_all_impl(
        &mut self,
        api_name: &str,
    ) -> Result<Option<Signal>, RuntimeError> {
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
            return Ok(Some(Signal::Value(Value::bytes(vec![]))));
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

        Ok(Some(Signal::Value(Value::bytes(all_bytes))))
    }
}
