//! HTTP/2 serve loop for the Taida interpreter's net package, split out
//! from `net_eval/mod.rs` (C13-3).
//!
//! Owns the `impl Interpreter` methods that run the HTTP/2 accept loop
//! (`serve_h2`), drive a single connection's frame state machine
//! (`h2_connection_loop`), and emit a completed HEADERS + DATA response
//! (`send_h2_response`). The underlying HPACK / frame codec lives in
//! `super::super::net_h2`; this module is the interpreter-side glue.
//!
//! C13-3 note: pure mechanical move — no behavior change. The HTTP/1.1
//! `eval_http_serve` implementation in `h1.rs` delegates into
//! `self.serve_h2(...)` when `protocol: "h2"` is negotiated.

use super::super::eval::{Interpreter, RuntimeError, Signal};
use super::super::value::Value;
use super::helpers::{
    extract_response_fields, make_fulfilled_async, make_result_failure_msg, make_result_success,
    make_span,
};
use super::types::ConnStream;

impl Interpreter {
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
    pub(super) fn serve_h2(
        &mut self,
        listener: std::net::TcpListener,
        tls_config: std::sync::Arc<rustls::ServerConfig>,
        handler: super::super::value::FuncValue,
        max_requests: i64,
        _max_connections: usize,
        read_timeout: std::time::Duration,
    ) -> Result<Option<Signal>, RuntimeError> {
        use super::super::net_h2::*;

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
                super::super::net_transport::TlsTransport::new(tls_conn, tcp_stream);
            match super::super::net_transport::complete_tls_handshake(
                &mut tls_transport,
                read_timeout,
            ) {
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
    pub(super) fn h2_connection_loop(
        &mut self,
        stream: &mut ConnStream,
        h2_conn: &mut super::super::net_h2::H2Connection,
        handler: &super::super::value::FuncValue,
        peer_addr: &std::net::SocketAddr,
        max_requests: i64,
        total_request_count: &mut i64,
    ) -> Result<(), RuntimeError> {
        use super::super::net_h2::*;

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
            if header.frame_type == super::super::net_h2::FRAME_SETTINGS
                && header.flags & super::super::net_h2::FLAG_ACK == 0
            {
                settings_ack_pending = true;
            }

            // Check for PING that needs response (NB6-38: minimal copy — PING is always 8 bytes)
            let is_ping_needing_ack = header.frame_type == super::super::net_h2::FRAME_PING
                && header.flags & super::super::net_h2::FLAG_ACK == 0;
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
                request_fields.push(("headers".into(), Value::list(header_values)));

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
                    s.state = super::super::net_h2::StreamState::Closed;
                }

                // Clean up closed streams to prevent unbounded growth
                h2_conn
                    .streams
                    .retain(|_, s| s.state != super::super::net_h2::StreamState::Closed);
            }
        }
    }

    /// Send an HTTP/2 response (HEADERS + DATA frames) for a completed request.
    pub(super) fn send_h2_response(
        &self,
        stream: &mut ConnStream,
        h2_conn: &mut super::super::net_h2::H2Connection,
        stream_id: u32,
        response: &Value,
    ) -> Result<(), RuntimeError> {
        use super::super::net_h2::*;

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
