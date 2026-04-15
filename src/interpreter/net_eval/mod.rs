/// Net package evaluation for the Taida interpreter.
///
/// Implements `taida-lang/net` (core-bundled):
///
/// HTTP surface:
///   httpServe, httpParseRequestHead, httpEncodeResponse, readBody
///   startResponse, writeChunk, endResponse, sseEvent
///   readBodyChunk, readBodyAll
///   wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
///
/// C12B-025 (2026-04-15): mechanical split from a single 12,591-line file
/// into a directory module:
///   - types.rs   — type definitions (Writer state, body framing, ConnStream)
///   - helpers.rs — free helper functions (parser / encoder / chunked / Result)
///   - mod.rs     — `impl Interpreter { ... }` (try_net_func dispatch + all evaluators)
///   - tests.rs   — `#[cfg(test)] mod tests` extracted verbatim
///
/// Public API surface (path-stable via re-exports):
///   - `pub(crate) const NET_SYMBOLS`           (re-exported from types.rs)
///   - `pub(crate) struct ActiveStreamingWriter` (re-exported from types.rs)
///   - `pub(crate) fn try_net_func`              (defined in this file)

pub(crate) mod helpers;
pub(crate) mod types;

#[cfg(test)]
mod tests;

// Re-exports to preserve the path `super::net_eval::ActiveStreamingWriter` /
// `super::net_eval::NET_SYMBOLS` used by sibling modules (eval.rs, module_eval.rs).
pub(crate) use types::{ActiveStreamingWriter, NET_SYMBOLS};

use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::Value;
use crate::net_surface::http_protocol_ordinal_to_wire;
use crate::parser::Expr;

use helpers::{
    build_streaming_head, chunked_body_complete, chunked_in_place_compact, determine_keep_alive,
    encode_response, eval_read_body, extract_body_token, extract_response_fields,
    extract_result_value, extract_result_value_owned, get_field_bool, get_field_int,
    get_field_str, get_field_value, is_body_stream_request,
    make_fulfilled_async, make_result_failure_msg, make_result_success, make_span,
    parse_request_head, send_response_scatter, write_all_retry,
    write_vectored_all, ChunkedBodyError,
};
use types::{
    BodyEncoding, ChunkedDecoderState, ConnAction, ConnReadResult, ConnStream, HttpConnection,
    RequestBodyState, StreamingWriter, WriterState, WsFrame, NEXT_WS_TOKEN,
};

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
                        had_content_length_header,
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

            // C12B-028: Graceful TLS / TCP close so the client receives the
            // full response before the process exits.
            //
            // Without this sequence, `max_requests == 1`-style bounded servers
            // race against curl: the process exits while response DATA frames
            // are still in the kernel send buffer, and because curl's POST
            // body lingers in the receive buffer, Linux sends RST instead of
            // FIN. The client then reports "Recv failure: connection reset"
            // and the response body is lost. See `tests/parity.rs` NB6-44 /
            // NET6-3a-3 for the regression surface.
            let _ = conn_stream.shutdown_write_graceful();
            // Short best-effort drain so curl sees our close_notify / FIN.
            conn_stream.drain_after_shutdown(16 * 1024);

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

    /// HTTP/3 serve entry point (NET7-3a: Interpreter parity backend).
    ///
    /// Mirrors the Native backend's `taida_net_h3_serve()`:
    ///   1. Run H3 protocol layer self-tests (QPACK round-trip, request validation)
    ///   2. Build handler closure (H3RequestData -> 14-field request pack -> handler -> H3ResponseData)
    ///   3. Run serve_h3_loop with sequential accept + handler dispatch
    ///   4. Return @(ok: true, requests: N) on success
    ///
    /// NET7-12b: The handler closure builds the same 14-field request pack
    /// as h1/h2, calls the user function synchronously, and extracts the
    /// response. The serve loop alternates between async I/O and sync handler
    /// dispatch, matching the Interpreter's single-threaded serial model.
    ///
    /// Design contracts (NET_DESIGN.md):
    ///   - cert/key required (validated before reaching here)
    ///   - 0-RTT: default-off, not exposed
    ///   - Handler dispatch: same 14-field request pack as h1/h2
    ///   - request_count: incremented only on valid HEADERS + handler success
    ///   - Graceful shutdown: GOAWAY -> drain -> close
    ///   - Bounded-copy discipline: 1 packet = at most 1 materialization
    ///   - Transport I/O does NOT use the existing Transport trait (NB7-7)
    fn serve_h3(
        &mut self,
        cert_path: String,
        key_path: String,
        handler: super::value::FuncValue,
        max_requests: i64,
        port: u16,
    ) -> Result<Option<Signal>, RuntimeError> {
        use super::net_h3;

        // NB7-9/NB7-10: Run embedded self-tests to validate QPACK round-trip
        // and H3 request pseudo-header validation, matching Native behavior.
        match net_h3::run_selftests() {
            net_h3::SelftestResult::Ok => {}
            net_h3::SelftestResult::QpackFailure(rc) => {
                let result = make_result_failure_msg(
                    "H3SelftestFailed",
                    format!(
                        "httpServe: HTTP/3 protocol layer self-test failed. \
                         QPACK encode/decode round-trip failed (code: {}).",
                        rc
                    ),
                );
                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
            }
            net_h3::SelftestResult::ValidationFailure(rc) => {
                let result = make_result_failure_msg(
                    "H3SelftestFailed",
                    format!(
                        "httpServe: HTTP/3 protocol layer self-test failed. \
                         Request pseudo-header validation failed (code: {}).",
                        rc
                    ),
                );
                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
            }
        }

        // NET7-12b: Connect to the real QUIC transport loop with handler dispatch.
        //
        // The Interpreter H3 path uses quinn (pure Rust, tokio-native) as the
        // QUIC substrate. Unlike the Native backend which uses libquiche via
        // dlopen, the Interpreter compiles quinn in at build time -- no runtime
        // library gate is needed.
        //
        // serve_h3_loop() creates a single-threaded tokio runtime internally,
        // using per-step block_on() to alternate between async I/O and sync
        // handler dispatch. The handler closure builds the same 14-field request
        // pack as h1/h2, calls the user function, and extracts the response.
        //
        // request_count is incremented only on valid HEADERS decode + successful
        // handler completion (NET7-12b contract).

        // NET7-12b: Handler dispatch closure.
        // Converts H3RequestData -> 14-field request pack -> handler call -> H3ResponseData.
        // Returns None on handler error (serve loop sends 500).
        let mut h3_handler = |req: net_h3::H3RequestData| -> Option<net_h3::H3ResponseData> {
            // Parse query from path (matching h1/h2 pattern).
            let (path_part, query_part) = match req.path.find('?') {
                Some(pos) => (req.path[..pos].to_string(), req.path[pos + 1..].to_string()),
                None => (req.path.clone(), String::new()),
            };

            // Build request pack matching h2 1-arg handler contract.
            let mut request_fields: Vec<(String, Value)> = vec![
                ("method".into(), Value::Str(req.method)),
                ("path".into(), Value::Str(path_part)),
                ("query".into(), Value::Str(query_part)),
                (
                    "version".into(),
                    Value::BuchiPack(vec![
                        ("major".into(), Value::Int(3)),
                        ("minor".into(), Value::Int(0)),
                    ]),
                ),
            ];

            // Convert H3 headers to the same format as h1/h2.
            let mut header_values: Vec<Value> = Vec::new();
            for (name, value) in &req.headers {
                header_values.push(Value::BuchiPack(vec![
                    ("name".into(), Value::Str(name.clone())),
                    ("value".into(), Value::Str(value.clone())),
                ]));
            }
            // Add :authority as host header for compatibility (same as h2).
            if !req.authority.is_empty() {
                header_values.push(Value::BuchiPack(vec![
                    ("name".into(), Value::Str("host".into())),
                    ("value".into(), Value::Str(req.authority.clone())),
                ]));
            }
            request_fields.push(("headers".into(), Value::List(header_values)));

            // Body.
            let raw_len = req.body.len();
            request_fields.push(("body".into(), make_span(0, raw_len)));
            request_fields.push(("bodyOffset".into(), Value::Int(0)));
            request_fields.push(("contentLength".into(), Value::Int(raw_len as i64)));
            request_fields.push(("raw".into(), Value::Bytes(req.body)));
            request_fields.push((
                "remoteHost".into(),
                Value::Str(req.remote_addr.ip().to_string()),
            ));
            request_fields.push((
                "remotePort".into(),
                Value::Int(req.remote_addr.port() as i64),
            ));
            request_fields.push(("keepAlive".into(), Value::Bool(true)));
            request_fields.push(("chunked".into(), Value::Bool(false)));
            request_fields.push(("protocol".into(), Value::Str("h3".into())));

            let request_pack = Value::BuchiPack(request_fields);

            // Call handler with request pack (1-arg path, same as h2).
            let handler_result = self.call_function_with_values(&handler, &[request_pack]);

            match handler_result {
                Ok(response) => {
                    // Extract response fields using the same extractor as h1/h2.
                    match extract_response_fields(&response) {
                        Ok((status, headers, body)) => Some(net_h3::H3ResponseData {
                            status: status as u16,
                            headers,
                            body,
                        }),
                        Err(_) => None, // Invalid response from handler.
                    }
                }
                Err(_) => None, // Handler threw an error.
            }
        };

        match net_h3::serve_h3_loop(&cert_path, &key_path, port, max_requests, &mut h3_handler) {
            Ok(request_count) => {
                let result_inner = Value::BuchiPack(vec![
                    ("ok".into(), Value::Bool(true)),
                    ("requests".into(), Value::Int(request_count)),
                ]);
                let result = make_result_success(result_inner);
                Ok(Some(Signal::Value(make_fulfilled_async(result))))
            }
            Err(e) => {
                // Classify the error: quinn/rustls initialization failures
                // are ProtocolError; runtime transport failures are separate.
                let kind = if e.contains("failed to read cert")
                    || e.contains("failed to read key")
                    || e.contains("failed to parse")
                    || e.contains("TLS config failed")
                    || e.contains("unsupported key type")
                    || e.contains("no valid certificates")
                    || e.contains("no PEM items")
                    || e.contains("failed to create QUIC endpoint")
                    || e.contains("failed to parse bind address")
                {
                    "ProtocolError"
                } else {
                    "H3RuntimeError"
                };
                let result = make_result_failure_msg(kind, e);
                Ok(Some(Signal::Value(make_fulfilled_async(result))))
            }
        }
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
                            fields, consumed, cl, is_chunked, had_cl_header,
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
                            let had_cl_header = get_field_bool(inner, "__hasContentLengthHeader")
                                .unwrap_or(false);
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
    fn dispatch_request(
        &mut self,
        conn: &mut HttpConnection,
        handler: &super::value::FuncValue,
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

