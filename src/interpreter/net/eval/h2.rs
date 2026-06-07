//! HTTP/2 serve loop for the Taida interpreter's net package, split out
//! from `net_eval/mod.rs`.
//!
//! Owns the `impl Interpreter` methods that run the HTTP/2 accept loop
//! (`serve_h2`), drive a single connection's frame state machine
//! (`h2_connection_loop`), and emit a completed HEADERS + DATA response
//! (`send_h2_response`). The underlying HPACK / frame codec lives in
//! `super::super::h2`; this module is the interpreter-side glue.
//!
//! note: pure mechanical move — no behavior change. The HTTP/1.1
//! `eval_http_serve` implementation in `h1.rs` delegates into
//! `self.serve_h2(...)` when `protocol: "h2"` is negotiated.

use super::helpers::{
    extract_response_fields, make_fulfilled_async, make_result_failure_msg, make_result_success,
    make_span,
};
use super::types::{ActiveStreamingWriter, ConnStream, RequestBodyState, StreamingWriter};
use crate::interpreter::eval::{Interpreter, RuntimeError, Signal};
use crate::interpreter::value::Value;

impl Interpreter {
    /// 2b/2c: HTTP/2 serve loop (Interpreter reference implementation).
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
    pub(super) fn serve_h2(
        &mut self,
        listener: std::net::TcpListener,
        tls_config: std::sync::Arc<rustls::ServerConfig>,
        handler: crate::interpreter::value::FuncValue,
        max_requests: i64,
        _max_connections: usize,
        read_timeout: std::time::Duration,
    ) -> Result<Option<Signal>, RuntimeError> {
        use super::super::h2::*;

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
            let mut tls_transport =
                super::super::transport::TlsTransport::new(tls_conn, tcp_stream);
            match super::super::transport::complete_tls_handshake(&mut tls_transport, read_timeout)
            {
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

        let result_inner = Value::pack(vec![
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
    pub(super) fn h2_connection_loop(
        &mut self,
        stream: &mut ConnStream,
        h2_conn: &mut super::super::h2::H2Connection,
        handler: &crate::interpreter::value::FuncValue,
        peer_addr: &std::net::SocketAddr,
        max_requests: i64,
        total_request_count: &mut i64,
    ) -> Result<(), RuntimeError> {
        use super::super::h2::*;

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
            if header.frame_type == super::super::h2::FRAME_SETTINGS
                && header.flags & super::super::h2::FLAG_ACK == 0
            {
                settings_ack_pending = true;
            }

            // Check for PING that needs response (NB6-38: minimal copy — PING is always 8 bytes)
            let is_ping_needing_ack = header.frame_type == super::super::h2::FRAME_PING
                && header.flags & super::super::h2::FLAG_ACK == 0;
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
                    // Drop the reset stream so its accumulated `request_body`
                    // is freed immediately (parity with native
                    // `h2_conn_remove_closed_streams`). Without this, a body
                    // accumulated up to MAX_REQUEST_BODY_SIZE before the cap
                    // tripped would linger in the `streams` map until the
                    // connection closed, undermining the OOM cap (G1).
                    h2_conn.streams.remove(&stream_id);
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

                // C26B-022 Step 2 (wJ Round 4, 2026-04-24): Enforce HTTP/2
                // wire byte upper limits at the parser boundary so that
                // downstream Native codegen fixed-size stack buffers cannot
                // silently truncate. RST_STREAM with REFUSED_STREAM (0x7)
                // when :method / :path / :authority exceeds its cap.
                // Limits mirror the H1 path (16 / 2048 / 256 bytes) and
                // the Native struct sizes in `net_h1_h2.c`.
                if method.len() > super::h1::HTTP_WIRE_MAX_METHOD_LEN
                    || path.len() > super::h1::HTTP_WIRE_MAX_PATH_LEN
                    || authority.len() > super::h1::HTTP_WIRE_MAX_AUTHORITY_LEN
                {
                    let _ = send_rst_stream(stream, stream_id, 0x7); // REFUSED_STREAM
                    continue;
                }

                // Parse query from path
                let (path_part, query_part) = match path.find('?') {
                    Some(pos) => (&path[..pos], &path[pos + 1..]),
                    None => (path.as_str(), ""),
                };

                // D29B-001 (Track-ζ Lock-H, 2026-04-27): build a per-request
                // arena that holds [body | method | path | query | header
                // name/value pairs ...] as a single contiguous Vec<u8>, then
                // expose every Str-shaped field (method/path/query/headers
                // name/value) as `@(start, len)` span packs that index into
                // the same arena. The arena becomes `req.raw`. This makes
                // h2 match the h1 reference shape (where parse_request_head
                // returns span packs against the head buffer) and lets
                // `SpanEquals[req.headers(0).name, req.raw, "host"]()`
                // succeed under h2 instead of silently returning false.
                //
                // Strategy V1-A (sub-Lock Phase-5_..._track-zeta_sub-Lock.md):
                // single arena, body first (offset 0, body span unchanged),
                // followed by header/pseudo strings. The HPACK dynamic table
                // is a moving target, so we cannot reuse its memory; the
                // arena copy is the only way to give Span* mold a stable
                // backing buffer.
                let body_len = body.len();
                let mut arena_cap = body_len + method.len() + path_part.len() + query_part.len();
                for (name, value) in &regular_headers {
                    arena_cap += name.len() + value.len();
                }
                if !authority.is_empty() {
                    arena_cap += 4 /* "host" */ + authority.len();
                }

                let mut arena: Vec<u8> = Vec::with_capacity(arena_cap);
                arena.extend_from_slice(&body);

                let method_start = arena.len();
                let method_len = method.len();
                arena.extend_from_slice(method.as_bytes());

                let path_start = arena.len();
                let path_len = path_part.len();
                arena.extend_from_slice(path_part.as_bytes());

                let query_start = arena.len();
                let query_len = query_part.len();
                arena.extend_from_slice(query_part.as_bytes());

                // Pre-allocate the header span tuples so we can build the
                // header list after the arena is final.
                let mut header_spans: Vec<(usize, usize, usize, usize)> =
                    Vec::with_capacity(regular_headers.len() + 1);
                for (name, value) in &regular_headers {
                    let n_start = arena.len();
                    let n_len = name.len();
                    arena.extend_from_slice(name.as_bytes());
                    let v_start = arena.len();
                    let v_len = value.len();
                    arena.extend_from_slice(value.as_bytes());
                    header_spans.push((n_start, n_len, v_start, v_len));
                }
                if !authority.is_empty() {
                    let n_start = arena.len();
                    arena.extend_from_slice(b"host");
                    let v_start = arena.len();
                    let v_len = authority.len();
                    arena.extend_from_slice(authority.as_bytes());
                    header_spans.push((n_start, 4, v_start, v_len));
                }

                let mut request_fields: Vec<(String, Value)> = vec![
                    ("method".into(), make_span(method_start, method_len)),
                    ("path".into(), make_span(path_start, path_len)),
                    ("query".into(), make_span(query_start, query_len)),
                    (
                        "version".into(),
                        Value::pack(vec![
                            ("major".into(), Value::Int(2)),
                            ("minor".into(), Value::Int(0)),
                        ]),
                    ),
                ];

                let mut header_values: Vec<Value> = Vec::with_capacity(header_spans.len());
                for (n_start, n_len, v_start, v_len) in &header_spans {
                    header_values.push(Value::pack(vec![
                        ("name".into(), make_span(*n_start, *n_len)),
                        ("value".into(), make_span(*v_start, *v_len)),
                    ]));
                }
                request_fields.push(("headers".into(), Value::list(header_values)));

                // F55 S2: branch on handler arity. The 1-arg path is the
                // pre-existing eager contract (body completed value, arena
                // shape pinned by the existing span regression tests). The
                // 2-arg path activates the streaming body observation
                // contract that H1 already implements: req.body span is
                // empty and the handler pulls bytes via readBody /
                // readBodyChunk / readBodyAll.
                //
                // Streaming form chosen: eager fill, streaming
                // observation. The DATA
                // frames for this stream have already been read to END_STREAM
                // by `process_frame` (which is what produces `body` here), so
                // the per-stream queue is pre-materialized. readBody* pops it
                // from `RequestBodyState.leftover` without ever touching the
                // raw socket — so no re-entrant frame reading is needed and
                // the 16 MiB cap (net_h2.rs MAX_REQUEST_BODY_SIZE, enforced on
                // accumulation) still bounds memory. The handler observes the
                // identical streaming contract to H1; only the supply timing
                // differs (eager fill, streaming observation).
                let handler_arity = handler.params.len();

                if handler_arity >= 2 {
                    // Pre-load the full (already capped) body into a
                    // Content-Length-style RequestBodyState. H2 has no chunked
                    // transfer encoding on the wire (DATA framing replaces it),
                    // so the supply source is modelled as a fixed-length body
                    // whose bytes all live in `leftover`. Built before the
                    // request pack so the pack can embed the real token.
                    let mut writer = StreamingWriter::new();
                    let mut body_state =
                        RequestBodyState::new(false, body_len as i64, true, body.clone());

                    let request_pack = Self::build_h2_streaming_request_pack(
                        &mut request_fields,
                        arena,
                        body_len,
                        peer_addr,
                        body_state.request_token,
                    );

                    let writer_pack = Value::pack(vec![(
                        "__writer_id".into(),
                        Value::str("__v3_streaming_writer".into()),
                    )]);

                    self.active_streaming_writer = Some(ActiveStreamingWriter {
                        writer: &mut writer as *mut StreamingWriter,
                        stream: stream as *mut ConnStream,
                        borrowed: false,
                        body_state: &mut body_state as *mut RequestBodyState,
                        ws_closed: false,
                        ws_token: 0,
                        ws_close_code: 0,
                    });

                    let handler_result =
                        self.call_function_with_values(handler, &[request_pack, writer_pack]);

                    // Handler done — tear down the active writer before any
                    // further borrow of `stream` / `h2_conn`.
                    self.active_streaming_writer = None;

                    *total_request_count += 1;
                    h2_conn.request_count += 1;

                    // 2-arg H2 handlers always return a one-shot response pack
                    // (the v3 chunked-streaming writer API targets H1 wire
                    // framing; H2 response DATA framing is handled by
                    // send_h2_response). Any unread body bytes still in
                    // `body_state.leftover` are simply dropped here — the
                    // stream is closed below, so the design's "drain remaining
                    // DATA, no RST" rule is satisfied trivially (all DATA was
                    // already consumed off the wire by process_frame).
                    match handler_result {
                        Ok(response) => {
                            if self
                                .send_h2_response(stream, h2_conn, stream_id, &response)
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                        Err(_) => {
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
                } else {
                    // Body span still references the leading `body_len` bytes of
                    // the arena (offset 0), preserving the existing contract.
                    request_fields.push(("body".into(), make_span(0, body_len)));
                    request_fields.push(("bodyOffset".into(), Value::Int(0)));
                    request_fields.push(("contentLength".into(), Value::Int(body_len as i64)));
                    // raw = arena (body + headers concat). Track-ε's Arc<BytesValue>
                    // interior wrapping keeps `req.raw` zero-copy on subsequent
                    // clones (handler dispatch retains via Arc::clone, no Vec
                    // re-allocation).
                    request_fields.push(("raw".into(), Value::bytes(arena)));
                    request_fields
                        .push(("remoteHost".into(), Value::str(peer_addr.ip().to_string())));
                    request_fields.push(("remotePort".into(), Value::Int(peer_addr.port() as i64)));
                    request_fields.push(("keepAlive".into(), Value::Bool(true)));
                    request_fields.push(("chunked".into(), Value::Bool(false)));
                    request_fields.push(("protocol".into(), Value::str("h2".into())));

                    let request_pack = Value::pack(request_fields);

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
                }

                // Mark stream as closed
                if let Some(s) = h2_conn.streams.get_mut(&stream_id) {
                    s.state = super::super::h2::StreamState::Closed;
                }

                // Clean up closed streams to prevent unbounded growth
                h2_conn
                    .streams
                    .retain(|_, s| s.state != super::super::h2::StreamState::Closed);
            }
        }
    }

    /// F55 S2: assemble the 2-arg (streaming) H2 request pack.
    ///
    /// Mirrors the H1 2-arg contract from `dispatch_request`: `body` is an
    /// empty span, `raw` is the per-request arena (body + headers, identical
    /// to the 1-arg arena so `req.raw` / `Slice[req.raw, ...]` stay valid),
    /// and the `__body_stream` sentinel marks the pack as streaming-capable so
    /// `readBody` / `readBodyChunk` / `readBodyAll` accept it. `body_token`
    /// must equal the `RequestBodyState.request_token` so the readBody*
    /// identity check in `net_eval/mod.rs` accepts this pack.
    fn build_h2_streaming_request_pack(
        request_fields: &mut Vec<(String, Value)>,
        arena: Vec<u8>,
        body_len: usize,
        peer_addr: &std::net::SocketAddr,
        body_token: u64,
    ) -> Value {
        // body span is empty — body not surfaced eagerly (H1 2-arg parity).
        request_fields.push(("body".into(), make_span(0, 0)));
        // bodyOffset points at the body region (offset 0 in the arena) so
        // addons that slice `req.raw` see the same origin as the 1-arg path.
        request_fields.push(("bodyOffset".into(), Value::Int(0)));
        request_fields.push(("contentLength".into(), Value::Int(body_len as i64)));
        request_fields.push(("raw".into(), Value::bytes(arena)));
        request_fields.push(("remoteHost".into(), Value::str(peer_addr.ip().to_string())));
        request_fields.push(("remotePort".into(), Value::Int(peer_addr.port() as i64)));
        request_fields.push(("keepAlive".into(), Value::Bool(true)));
        request_fields.push(("chunked".into(), Value::Bool(false)));
        request_fields.push(("protocol".into(), Value::str("h2".into())));
        // v4 sentinel + request-scoped token (matches RequestBodyState).
        request_fields.push((
            "__body_stream".into(),
            Value::str("__v4_body_stream".into()),
        ));
        request_fields.push(("__body_token".into(), Value::Int(body_token as i64)));
        Value::pack(std::mem::take(request_fields))
    }

    /// Send an HTTP/2 response (HEADERS + DATA frames) for a completed request.
    pub(super) fn send_h2_response(
        &self,
        stream: &mut ConnStream,
        h2_conn: &mut super::super::h2::H2Connection,
        stream_id: u32,
        response: &Value,
    ) -> Result<(), RuntimeError> {
        use super::super::h2::*;

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
}
