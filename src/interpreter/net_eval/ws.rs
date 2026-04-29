//! WebSocket implementation (RFC 6455) for the Taida interpreter's net
//! package, split out from `net_eval/mod.rs` (C13-3).
//!
//! Owns the `impl Interpreter` methods for the WebSocket surface
//! (`wsUpgrade` / `wsSend` / `wsReceive` / `wsClose` / `wsCloseCode`),
//! as well as the frame-level reader/writer helpers and the
//! `finalize_websocket_close` hook invoked by the HTTP/1.1 dispatcher.
//!
//! C13-3 note: pure mechanical move — no behavior change. The
//! `try_net_func` dispatcher in `mod.rs` continues to route these calls;
//! this file merely hosts the implementations.

use super::super::eval::{Interpreter, RuntimeError, Signal};
use super::super::value::Value;
use super::helpers::{
    extract_body_token, get_field_int, get_field_value, write_all_retry, write_vectored_all,
};
use super::types::{ActiveStreamingWriter, ConnStream, NEXT_WS_TOKEN, WriterState, WsFrame};
use crate::parser::Expr;

impl Interpreter {
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

    fn active_ws_writer_for(&self, api_name: &str) -> Result<&ActiveStreamingWriter, RuntimeError> {
        self.active_streaming_writer
            .as_ref()
            .ok_or_else(|| RuntimeError {
                message: format!(
                    "{}: active streaming writer disappeared during handler execution",
                    api_name
                ),
            })
    }

    /// Compute Sec-WebSocket-Accept from Sec-WebSocket-Key (NET4-2b).
    /// SHA-1(key + GUID) → Base64.
    pub(super) fn compute_ws_accept(key: &str) -> String {
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
    pub(super) fn validate_ws_token(
        &mut self,
        args: &[Expr],
        api_name: &str,
    ) -> Result<(), RuntimeError> {
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
                    k == "__ws_id"
                        && matches!(v, Value::Str(s) if s.as_str() == "__v4_websocket_conn")
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
    pub(super) fn eval_ws_upgrade(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
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

        let active = self.active_ws_writer_for("wsUpgrade")?;
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
        let active = self.active_ws_writer_for("wsUpgrade")?;
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
        let ws_pack = Value::pack(vec![
            ("__ws_id".into(), Value::str("__v4_websocket_conn".into())),
            ("__ws_token".into(), Value::Int(ws_token as i64)),
        ]);

        // Return Lax with the ws connection.
        Ok(Some(Signal::Value(Self::make_lax_ws_value(ws_pack))))
    }

    /// Build Lax[@(ws: WsConn)] with value.
    pub(super) fn make_lax_ws_value(ws: Value) -> Value {
        let inner = Value::pack(vec![("ws".into(), ws)]);
        Value::pack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__value".into(), inner),
            ("__default".into(), Value::pack(vec![])),
            ("__type".into(), Value::str("Lax".into())),
        ])
    }

    /// Build Lax empty for failed wsUpgrade.
    pub(super) fn make_lax_ws_empty() -> Value {
        Value::pack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("__value".into(), Value::pack(vec![])),
            ("__default".into(), Value::pack(vec![])),
            ("__type".into(), Value::str("Lax".into())),
        ])
    }

    /// `wsSend(ws, data)` → Unit (NET4-2e).
    ///
    /// Sends a WebSocket text or binary frame.
    /// - Str → text frame (opcode 0x1)
    /// - Bytes → binary frame (opcode 0x2)
    ///
    /// Server-to-client: MASK=0.
    pub(super) fn eval_ws_send(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
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

        let active = self.active_ws_writer_for("wsSend")?;
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
    pub(super) fn write_ws_frame(
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
    pub(super) fn eval_ws_receive(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsReceive: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate ws token.
        self.validate_ws_token(args, "wsReceive")?;

        let active = self.active_ws_writer_for("wsReceive")?;
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
                        ("text", Value::str(text))
                    } else {
                        ("binary", Value::bytes(payload))
                    };
                    let inner = Value::pack(vec![
                        ("type".into(), Value::str(type_str.into())),
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
    pub(super) fn make_lax_ws_frame_value(inner: Value) -> Value {
        Value::pack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__value".into(), inner),
            ("__default".into(), Value::pack(vec![])),
            ("__type".into(), Value::str("Lax".into())),
        ])
    }

    /// Build Lax empty for close / end of stream.
    pub(super) fn make_lax_ws_frame_empty() -> Value {
        Value::pack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("__value".into(), Value::pack(vec![])),
            ("__default".into(), Value::pack(vec![])),
            ("__type".into(), Value::str("Lax".into())),
        ])
    }

    /// Read exactly `count` bytes from a TcpStream.
    pub(super) fn read_exact_bytes(
        stream: &mut ConnStream,
        count: usize,
    ) -> Result<Vec<u8>, RuntimeError> {
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
    pub(super) fn read_ws_frame(stream: &mut ConnStream) -> Result<WsFrame, RuntimeError> {
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
    pub(super) fn eval_ws_close(&mut self, args: &[Expr]) -> Result<Option<Signal>, RuntimeError> {
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
        let active = self.active_ws_writer_for("wsClose")?;
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
    pub(super) fn eval_ws_close_code(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        if self.active_streaming_writer.is_none() {
            return Err(RuntimeError {
                message: "wsCloseCode: can only be called inside a 2-argument httpServe handler"
                    .into(),
            });
        }

        // Validate ws token.
        self.validate_ws_token(args, "wsCloseCode")?;

        let active = self.active_ws_writer_for("wsCloseCode")?;
        let writer = unsafe { &*active.writer };

        if writer.state != WriterState::WebSocket {
            return Err(RuntimeError {
                message: "wsCloseCode: not in WebSocket state. Call wsUpgrade first.".into(),
            });
        }

        let close_code = active.ws_close_code;
        Ok(Some(Signal::Value(Value::Int(close_code))))
    }
    /// C12B-041: Finalize a WebSocket close by flushing and gracefully shutting
    /// down the TCP write half so the client observes the close frame + FIN
    /// before the process terminates.
    ///
    /// Applied at the two WS close confirmation points inside `dispatch_request`:
    ///   1. Success auto-close (handler returned normally from a WS handler)
    ///   2. Error-close 1011 (handler raised, WS still in WebSocket state)
    ///
    /// This mirrors the C12B-028 pattern applied to the H2 bounded-exit path.
    /// Without this sequence the kernel can send RST instead of FIN when the
    /// process exits with pending close-frame bytes still in the send buffer,
    /// causing `test_net4_ws_auto_close_on_return_interp` and similar WS
    /// regression tests to fail flakily.
    ///
    /// Note: we intentionally do NOT flush inside `write_ws_frame` itself.
    /// That would add a per-frame syscall on the hot path; doing it once at
    /// the close confirmation point has no such cost.
    pub(super) fn finalize_websocket_close(stream: &mut ConnStream) {
        // 1. Drain any buffered bytes (the close frame + preceding data frames)
        //    into the kernel send buffer so `shutdown_write` sees them first.
        let _ = std::io::Write::flush(stream);
        // 2. Half-close the write side so the peer observes FIN (or
        //    TLS close_notify + FIN) cleanly.
        let _ = stream.shutdown_write_graceful();
        // 3. Best-effort short drain so we absorb any peer bytes that arrive
        //    during the linger window and give the peer time to observe our
        //    FIN before the process terminates.
        stream.drain_after_shutdown(16 * 1024);
    }
}
