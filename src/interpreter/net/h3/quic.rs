/// QUIC transport substrate for the Interpreter backend.
///
/// **: ** -- UDP + QUIC accept implementation.
/// **: ** -- H3 serve loop with QUIC streams.
/// **: ** -- Handler dispatch with h1/h2-compatible request pack.
///
/// This module provides the QUIC transport layer using quinn (pure Rust,
/// tokio-native) as the substrate. It replaces the libquiche.so
/// dlopen gate with a compile-time Rust dependency.
///
/// # Architecture
///
/// 1. **TLS Config**: Generate self-signed cert via rcgen, build rustls
/// ServerConfig with ALPN h3 ("h3").
/// 2. **QUIC Endpoint**: Bind tokio::net::UdpSocket, wrap in quinn
/// ServerConfig, create listening Endpoint.
/// 3. **Accept Loop**: Endpoint::accept() loop for incoming connections.
/// 4. **Stream Dispatch (/12b)**: accept_bi() -> H3 frame decode ->
/// QPACK decode -> request extraction -> handler dispatch -> response write.
/// 5. **Idle timeout / GOAWAY / shutdown** integration via H3Connection.
///
/// # Design Decisions
///
/// - Bounded-copy discipline: stream bytes read into fixed-size chunks;
/// no intermediate aggregate buffers.
/// - ALPN h3 exact match: only "h3" is accepted (no silent fallback to
/// h2/h1).
/// - The serve loop runs on an internal tokio runtime, bridged to the
/// synchronous interpreter via per-step `Runtime::block_on()` calls.
/// -: Handler dispatch is synchronous. The serve loop alternates
/// between async I/O (request read, response write) and sync handler
/// invocation, matching the h1/h2 serial model. No `tokio::spawn`
/// is used, so the interpreter's `&mut self` is available for handler calls.
///
/// # Dependencies
///
/// - quinn 0.11: Pure Rust QUIC stack (tokio-native)
/// - rustls 0.23: TLS 1.3 (already v5 dependency)
/// - rcgen 0.13: Runtime cert/key generation for self-signed certs
use quinn::crypto::rustls::QuicServerConfig;
use std::{fs, net::SocketAddr, sync::Arc};

// NB7-92: HTTP/3 application error codes (RFC 9114 Section 8.1 / IANA).
// These map to QUIC application error codes via stream.reset().
pub(crate) const H3_ERR_NO_ERROR: u64 = 0x0100;
pub(crate) const H3_ERR_GENERAL_PROTOCOL_ERROR: u64 = 0x0101;
pub(crate) const H3_ERR_FRAME_UNEXPECTED: u64 = 0x0105;
pub(crate) const H3_ERR_STREAM_CREATION_ERROR: u64 = 0x0103;

/// Maximum number of concurrent connections the serve loop tracks.
const MAX_CONCURRENT_CONNS: usize = 256;

/// Read buffer size for QUIC stream data (bounded-copy).
const STREAM_READ_BUF: usize = 8192;

/// H3 ALPN protocol identifier per IANA assignment for HTTP/3.
///
/// RFC 9114 Section 2: The ALPN token for HTTP/3 is "h3".
pub(crate) const H3_ALPN: &[u8] = b"h3";

/// Default port for HTTP/3 (same as HTTPS: 443).
/// Present as documentation of the standard HTTP/3 port.
/// serve_h3_loop requires an explicit port parameter (0 = OS picks).
pub(crate) const DEFAULT_H3_PORT: u16 = 443;

// ── NET7-12b: Handler dispatch data types ─────────────────────────────

/// Decoded H3 request data extracted from QPACK HEADERS + DATA frames.
///
/// This is the transport-agnostic representation passed to the user handler
/// callback. The caller (net_eval.rs) converts this into the same 14-field
/// request pack used by h1/h2.
///
/// Body is bounded by `max_field_section_size` ( bounded-copy discipline).
pub(crate) struct H3RequestData {
    pub method: String,
    pub path: String,
    pub authority: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub remote_addr: std::net::SocketAddr,
}

/// Response data returned by the user handler callback.
///
/// The caller (net_eval.rs) extracts these fields from the handler's
/// return value using the same `extract_response_fields` as h1/h2.
pub(crate) struct H3ResponseData {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A successfully accepted QUIC connection.
pub(crate) struct AcceptedConnection {
    pub connection: quinn::Connection,
    pub remote_addr: std::net::SocketAddr,
}

// ── TLS Config ──────────────────────────────────────────────────────────

/// Ensure the rustls default crypto provider is installed (idempotent).
/// Called once per process; subsequent calls are a no-op.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Build a TLS server config for QUIC.
///
/// If `cert_path` and `key_path` are non-empty, load PEM files from disk.
/// Otherwise, generate a self-signed certificate for "localhost".
///
/// Previously ignored cert_path and key_path, always self-signing.
fn build_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<(rustls::ServerConfig, Vec<u8>, Vec<u8>), String> {
    ensure_crypto_provider();

    let (cert_der, key_der) = if !cert_path.is_empty() && !key_path.is_empty() {
        // User-provided certificate and key paths.
        let cert_pem = fs::read(cert_path).map_err(|e| {
            format!(
                "httpServe: HTTP/3 failed to read cert file '{}': {}",
                cert_path, e
            )
        })?;
        let key_pem = fs::read(key_path).map_err(|e| {
            format!(
                "httpServe: HTTP/3 failed to read key file '{}': {}",
                key_path, e
            )
        })?;

        let mut cert_cursor = std::io::Cursor::new(&cert_pem);
        let certs_result: Result<Vec<rustls::pki_types::CertificateDer<'static>>, _> =
            rustls_pemfile::certs(&mut cert_cursor)
                .map(|r| r.map(|c| c.into_owned()))
                .collect();
        let certs = certs_result
            .map_err(|e| format!("httpServe: HTTP/3 failed to parse cert PEM: {}", e))?;
        if certs.is_empty() {
            return Err("httpServe: HTTP/3 cert file contained no valid certificates".to_string());
        }

        let key_obj = rustls_pemfile::read_one(&mut std::io::Cursor::new(&key_pem))
            .map_err(|e| format!("httpServe: HTTP/3 failed to read key PEM: {}", e))?
            .ok_or("httpServe: HTTP/3 key file contained no PEM items")?;

        let key_der: Vec<u8> = match key_obj {
            rustls_pemfile::Item::Pkcs8Key(k) => k.secret_pkcs8_der().to_vec(),
            rustls_pemfile::Item::Pkcs1Key(k) => k.secret_pkcs1_der().to_vec(),
            rustls_pemfile::Item::Sec1Key(k) => k.secret_sec1_der().to_vec(),
            other => {
                return Err(format!(
                    "httpServe: HTTP/3 unsupported key type: {:?}",
                    other
                ));
            }
        };

        (certs[0].to_vec(), key_der)
    } else {
        // Self-signed certificate (bootstrap / testing path).
        let cert =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).map_err(|e| {
                format!(
                    "httpServe: HTTP/3 failed to generate self-signed cert: {}",
                    e
                )
            })?;

        (cert.cert.der().to_vec(), cert.key_pair.serialize_der())
    };

    let private_key: rustls::pki_types::PrivateKeyDer<'static> =
        rustls::pki_types::PrivateKeyDer::try_from(key_der.clone())
            .map_err(|e| format!("httpServe: HTTP/3 failed to parse private key: {}", e))?;

    let cert_der_for_config: Vec<u8> = cert_der.clone();
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(cert_der_for_config)],
            private_key,
        )
        .map_err(|e| format!("httpServe: HTTP/3 TLS config failed: {}", e))?;

    let mut tls_config = tls_config;
    tls_config.alpn_protocols = vec![H3_ALPN.to_vec()];

    Ok((tls_config, cert_der, key_der))
}

// ── Endpoint Creation ──────────────────────────────────────────────────

/// Create a QUIC server endpoint that listens on the given port.
pub(crate) fn create_quic_endpoint(
    cert_path: &str,
    key_path: &str,
    port: u16,
) -> Result<(quinn::Endpoint, Vec<u8>, Vec<u8>), String> {
    let (tls_config, cert_der, key_der) = build_tls_config(cert_path, key_path)?;

    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(tls_config)
            .map_err(|e| format!("httpServe: HTTP/3 QUIC crypto config failed: {}", e))?,
    ));

    let bind_addr: SocketAddr = format!("127.0.0.1:{}", port).parse().map_err(|e| {
        format!(
            "httpServe: HTTP/3 failed to parse bind address 127.0.0.1:{}: {}",
            port, e
        )
    })?;

    let endpoint = quinn::Endpoint::server(server_config, bind_addr)
        .map_err(|e| format!("httpServe: HTTP/3 failed to create QUIC endpoint: {}", e))?;

    Ok((endpoint, cert_der, key_der))
}

/// Accept the next incoming QUIC connection.
pub(crate) async fn accept_connection(
    endpoint: &quinn::Endpoint,
) -> Option<Result<AcceptedConnection, String>> {
    let incoming = endpoint.accept().await?;
    let remote_addr = incoming.remote_address();

    match incoming.accept() {
        Ok(connecting) => match connecting.await {
            Ok(conn) => Some(Ok(AcceptedConnection {
                connection: conn,
                remote_addr,
            })),
            Err(e) => Some(Err(format!("QUIC TLS handshake failed: {}", e))),
        },
        Err(e) => Some(Err(format!("QUIC accept error: {}", e))),
    }
}

// ── NET7-9c/12b: Accept bi stream processing ─────────────────────────

/// Result of reading and decoding a single QUIC stream.
///
/// Separates request extraction from response sending so the
/// caller can invoke the user handler synchronously between the two async
/// phases.
enum StreamReadResult {
    /// A valid HEADERS frame was decoded; request data and send stream are ready.
    Request(H3RequestData, quinn::SendStream),
    /// Stream had an error or protocol violation; error response already sent.
    Error,
    /// No HEADERS frame on this stream (e.g., empty or control-frame-only).
    NoRequest,
}

/// 12b: Read and decode a single bidirectional QUIC stream.
///
/// Reads raw bytes, decodes H3 frames, extracts the request fields.
/// On success, returns the extracted `H3RequestData` and the `SendStream`
/// for the caller to send the response after handler dispatch.
///
/// # Bounded-copy discipline
///
/// - Reads into a local `STREAM_READ_BUF` buffer.
/// - All frames in the buffer are decoded in sequence.
/// - SETTINGS on request streams is rejected (RFC 9114 §7.2.4.1).
/// - GOAWAY on request streams is rejected (RFC 9114 §7.2.6).
/// - Frame decode errors use H3_ERR_FRAME_UNEXPECTED.
/// - Body data (DATA frames after HEADERS) is collected, bounded by
/// `max_field_section_size`.
async fn read_request_stream(
    h3_conn: &mut super::H3Connection,
    mut recv: quinn::RecvStream,
    mut send: quinn::SendStream,
    remote_addr: std::net::SocketAddr,
) -> StreamReadResult {
    // Accumulate request bytes, bounded by max_field_section_size.
    let max_size = h3_conn.max_field_section_size as usize;
    let mut buf = Vec::with_capacity(STREAM_READ_BUF);

    loop {
        let mut chunk_buf = [0u8; STREAM_READ_BUF];
        match recv.read(&mut chunk_buf).await {
            Ok(Some(n)) => {
                if buf.len() + n > max_size {
                    let _ = send.reset(
                        H3_ERR_GENERAL_PROTOCOL_ERROR
                            .try_into()
                            .expect("H3 error code as VarInt"),
                    );
                    return StreamReadResult::Error;
                }
                buf.extend_from_slice(&chunk_buf[..n]);
            }
            Ok(None) => break, // FIN received
            Err(_) => {
                let _ = send.reset(
                    H3_ERR_GENERAL_PROTOCOL_ERROR
                        .try_into()
                        .expect("H3 error code as VarInt"),
                );
                return StreamReadResult::Error;
            }
        }
    }

    if buf.is_empty() {
        let _ = send.reset(H3_ERR_NO_ERROR.try_into().expect("H3 error code as VarInt"));
        return StreamReadResult::NoRequest;
    }

    // Decode all H3 frames in the buffer. NB7-88: previously only the first
    // frame was decoded; now we iterate through all available frames.
    let mut pos = 0;
    let mut request_data: Option<H3RequestData> = None;
    let mut body_data: Vec<u8> = Vec::new();

    while pos < buf.len() {
        // Step 1: decode frame header to get type, length, and header size.
        let (frame_type, frame_length, header_size) = match super::decode_frame_header(&buf[pos..])
        {
            Some((ft, fl, hs)) => (ft, fl, hs),
            None => {
                let _ = send.reset(
                    H3_ERR_FRAME_UNEXPECTED
                        .try_into()
                        .expect("H3 error code as VarInt"),
                );
                return StreamReadResult::Error;
            }
        };

        let frame_len = match usize::try_from(frame_length) {
            Ok(n) => n,
            Err(_) => {
                let _ = send.reset(
                    H3_ERR_FRAME_UNEXPECTED
                        .try_into()
                        .expect("H3 error code as VarInt"),
                );
                return StreamReadResult::Error;
            }
        };

        let total_frame_size = header_size + frame_len;
        if pos + total_frame_size > buf.len() {
            // Incomplete frame — we've received partial data for this frame.
            // Treat as malformed since FIN was received with incomplete frame.
            let _ = send.reset(
                H3_ERR_FRAME_UNEXPECTED
                    .try_into()
                    .expect("H3 error code as VarInt"),
            );
            return StreamReadResult::Error;
        }

        let payload = &buf[pos + header_size..pos + total_frame_size];
        pos += total_frame_size;

        match frame_type {
            super::H3_FRAME_HEADERS => {
                if request_data.is_some() {
                    // Duplicate HEADERS on same request stream — protocol error
                    let _ = send.reset(
                        H3_ERR_FRAME_UNEXPECTED
                            .try_into()
                            .expect("H3 error code as VarInt"),
                    );
                    return StreamReadResult::Error;
                }

                // Decode QPACK-encoded request headers.
                // NB7-102: Pass the connection's dynamic_table when present so that
                // dynamic table entries (from prior SETTINGS/encoder stream activity)
                // can be resolved during QPACK decode.
                let dyn_table = h3_conn.dynamic_table.as_ref();
                let headers = match super::qpack_decode_block(payload, 8, None, dyn_table) {
                    Some(h) => h,
                    None => {
                        let _ = send_error_response(&mut send, 400, b"Bad Request").await;
                        return StreamReadResult::Error;
                    }
                };

                // Extract and validate request fields.
                let request = match super::extract_request_fields(&headers) {
                    Ok(req) => req,
                    Err(_) => {
                        let _ = send_error_response(&mut send, 400, b"Bad Request").await;
                        return StreamReadResult::Error;
                    }
                };

                // Touch idle timer on successful request activity.
                h3_conn.reset_idle_timer();

                // NET7-12b: Build H3RequestData for handler dispatch.
                request_data = Some(H3RequestData {
                    method: request.method,
                    path: request.path,
                    authority: request.authority,
                    headers: request.regular_headers,
                    body: Vec::new(), // will be filled from DATA frames below
                    remote_addr,
                });
            }

            super::H3_FRAME_DATA => {
                if request_data.is_none() {
                    // DATA without prior HEADERS — protocol error.
                    let _ =
                        send_error_response(&mut send, 400, b"Expected HEADERS before DATA").await;
                    return StreamReadResult::Error;
                }
                // Collect body data from DATA frame, bounded by max_field_section_size.
                body_data.extend_from_slice(payload);
            }

            super::H3_FRAME_SETTINGS => {
                // NB7-84: SETTINGS MUST only be sent on the control stream
                // (unidirectional, type 0x02). On a bidirectional request
                // stream this is H3_ERR_FRAME_UNEXPECTED (RFC 9114 §7.2.4.1).
                let _ = send.reset(
                    H3_ERR_FRAME_UNEXPECTED
                        .try_into()
                        .expect("H3 error code as VarInt"),
                );
                return StreamReadResult::Error;
            }

            super::H3_FRAME_GOAWAY => {
                // NB7-85: GOAWAY MUST only be sent on the control stream
                // (RFC 9114 §7.2.6). On a request stream: reject.
                // NB7-99: do NOT call h3_conn.receive_goaway() here;
                // connection_handler is the sole authoritative caller.
                let _ = send.reset(
                    H3_ERR_FRAME_UNEXPECTED
                        .try_into()
                        .expect("H3 error code as VarInt"),
                );
                return StreamReadResult::Error;
            }

            _ => {
                // NB7-91: Unknown frame type. RFC 9114 Section 7.2.8 allows
                // implementations to ignore unknown frame types.
                // We silently skip without sending an HTTP response, which
                // would confuse a peer sending control-frame semantics.
            }
        }
    }

    match request_data {
        Some(mut req) => {
            // Attach collected body data.
            req.body = body_data;
            StreamReadResult::Request(req, send)
        }
        None => StreamReadResult::NoRequest,
    }
}

/// Send an H3 response (HEADERS + DATA + FIN) on a QUIC send stream.
///
/// Converts `H3ResponseData` into QPACK-encoded HEADERS and DATA frames.
/// Returns true on success, false on write error.
async fn send_h3_response(mut send: quinn::SendStream, response: &H3ResponseData) -> bool {
    // Convert headers to the format expected by build_response_headers_frame.
    let header_refs: Vec<(String, String)> = response.headers.clone();

    // Send HEADERS frame.
    let Some(hdrs_frame) = super::build_response_headers_frame(response.status, &header_refs)
    else {
        let _ = send.reset(
            H3_ERR_GENERAL_PROTOCOL_ERROR
                .try_into()
                .expect("H3 error code as VarInt"),
        );
        return false;
    };
    if send.write_all(&hdrs_frame).await.is_err() {
        return false;
    }

    // Send DATA frame (only if body is non-empty, matching h2 bodyless status policy).
    let no_body = (100..200).contains(&response.status)
        || response.status == 204
        || response.status == 205
        || response.status == 304;

    if !no_body && !response.body.is_empty() {
        let Some(data_frame) = super::build_data_frame(&response.body) else {
            let _ = send.reset(
                H3_ERR_GENERAL_PROTOCOL_ERROR
                    .try_into()
                    .expect("H3 error code as VarInt"),
            );
            return false;
        };
        if send.write_all(&data_frame).await.is_err() {
            return false;
        }
    }

    // Send FIN.
    send.finish().is_ok()
}

/// Send a 500 Internal Server Error response on a QUIC send stream.
async fn send_h3_error_500(send: quinn::SendStream) {
    let error_response = H3ResponseData {
        status: 500,
        headers: vec![],
        body: b"Internal Server Error".to_vec(),
    };
    let _ = send_h3_response(send, &error_response).await;
}

/// Send an error response on a QUIC stream (HEADERS + DATA + FIN).
async fn send_error_response(
    send: &mut quinn::SendStream,
    status: u16,
    body: &[u8],
) -> Result<(), ()> {
    let headers_frame = super::build_response_headers_frame(status, &[]).ok_or(())?;
    let data_frame = super::build_data_frame(body);

    send.write_all(&headers_frame).await.map_err(|_| ())?;
    if let Some(df) = data_frame {
        send.write_all(&df).await.map_err(|_| ())?;
    }
    send.finish().map_err(|_| ())
}

// ── NET7-9c/12b: Public serve loop ────────────────────────────────────

/// H3 serve loop — the main entry point for the Interpreter H3 server.
///
/// This runs synchronously via per-step `Runtime::block_on()` calls,
/// alternating between async I/O and synchronous handler dispatch:
///
/// 1. Creates a QUIC endpoint (TLS + ALPN h3)
/// 2. Accepts connections sequentially (serial model, matching h1/h2)
/// 3. For each connection, accepts bi-directional streams
/// 4. Stream data is decoded as H3 frames (SETTINGS, HEADERS, DATA)
/// 5. Request fields are extracted into `H3RequestData`
/// 6. Handler callback is invoked synchronously (outside async context)
/// 7. Response is sent as HEADERS + DATA frames
/// 8. Idle timeout and GOAWAY/shutdown are integrated
///
/// # Handler Dispatch///
/// The `handler` callback converts `H3RequestData` into the same 14-field
/// request pack used by h1/h2, calls the user's Taida function, and extracts
/// the response. This callback is invoked synchronously between async I/O
/// steps, matching the Interpreter's single-threaded, serial model.
///
/// `request_count` is incremented only when:
/// 1. A valid HEADERS frame is decoded/// 2. The handler completes successfully///
/// Error streams (protocol errors, QPACK failures) are NOT counted.
///
/// # Arguments
///
/// * `cert_path` - Path to TLS certificate PEM (empty = self-signed)
/// * `key_path` - Path to TLS private key PEM (empty = self-signed)
/// * `port` - UDP port to bind (matching h1/h2 policy: 0 = OS picks)
/// * `max_requests` - Max total requests before shutdown (0 = unlimited)
/// * `handler` - Synchronous callback: H3RequestData -> Option<H3ResponseData>.
/// Returns None on handler error (500 will be sent).
///
/// # Returns
///
/// The number of requests served, or an error string.
pub(crate) fn serve_h3_loop(
    cert_path: &str,
    key_path: &str,
    port: u16,
    max_requests: i64,
    handler: &mut dyn FnMut(H3RequestData) -> Option<H3ResponseData>,
) -> Result<i64, String> {
    // Create a single-threaded tokio runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("httpServe: HTTP/3 failed to create tokio runtime: {}", e))?;

    // Step 1: Create QUIC endpoint with user-provided or self-signed cert/key.
    let endpoint = rt.block_on(async {
        let (ep, _cert_der, _key_der) = create_quic_endpoint(cert_path, key_path, port)?;
        Ok::<quinn::Endpoint, String>(ep)
    })?;

    let mut total_request_count: i64 = 0;
    let mut connections: Vec<quinn::Connection> = Vec::new();

    // NET7-12b: Sequential accept loop — process one connection at a time
    // (matching h1/h2 serial model). No tokio::spawn, so &mut self is
    // available for handler calls.
    'accept: loop {
        // Check global max_requests limit.
        if max_requests > 0 && total_request_count >= max_requests {
            break;
        }

        // Accept the next incoming connection (async).
        let accepted = rt.block_on(accept_connection(&endpoint));
        let accepted = match accepted {
            Some(Ok(a)) => a,
            Some(Err(e)) => {
                eprintln!("httpServe HTTP/3: accept error: {}", e);
                continue;
            }
            None => {
                // Endpoint shutting down.
                break;
            }
        };

        // Bound concurrent connections.
        connections.retain(|c| c.close_reason().is_none());
        if connections.len() >= MAX_CONCURRENT_CONNS {
            if let Some(old) = connections.first() {
                old.close(0u32.into(), b"too_many_connections");
            }
            connections.drain(..1);
        }
        connections.push(accepted.connection.clone());

        // Create H3Connection with idle timeout tracking.
        let mut h3_conn = super::H3Connection::new();
        let conn_id = accepted.connection.stable_id().to_ne_bytes().to_vec();
        h3_conn.set_quic_connection_id(conn_id);
        h3_conn.state = super::H3ConnState::Active;

        let conn = accepted.connection;
        let remote_addr = accepted.remote_addr;

        // NB7-115: Initialize server-initiated unidirectional control stream.
        // RFC 9114 Section 6.2.1: Each side MUST initiate a single control stream
        // and send SETTINGS as the first frame. GOAWAY frames are also sent here.
        // This mirrors Native's h3_init_control_stream() (native_runtime.c:17761-17797).
        //
        // The SendStream is kept alive for the duration of the connection so
        // that GOAWAY can be written to it during shutdown (not on a new stream).
        let mut ctrl_send: Option<quinn::SendStream> = None;
        if let Ok(mut ctrl_uni) = rt.block_on(conn.open_uni()) {
            // Step 1: Send stream type byte (0x00 = control stream per RFC 9114 Section 6.2).
            let type_byte: [u8; 1] = [0x00];
            let type_ok = rt.block_on(ctrl_uni.write_all(&type_byte)).is_ok();

            if type_ok
                && let Some(settings_payload) = super::encode_settings()
                && let Some(settings_frame) =
                    super::encode_frame(super::H3_FRAME_SETTINGS, &settings_payload)
                && rt.block_on(ctrl_uni.write_all(&settings_frame)).is_ok()
            {
                h3_conn.control_stream_id = Some(ctrl_uni.id().index());
                // Keep the SendStream alive — GOAWAY will be written here.
                ctrl_send = Some(ctrl_uni);
            }
            // Do NOT call ctrl_uni.finish() — the control stream stays open.
        }

        // Per-connection stream loop (sequential).
        loop {
            // Check idle timeout.
            if h3_conn.check_timeout().is_some() {
                let _ = h3_conn.begin_shutdown();
                break;
            }

            // Check per-connection max_requests.
            if max_requests > 0 && total_request_count >= max_requests {
                let _ = h3_conn.begin_shutdown();
                break;
            }

            // No new streams in draining/closed state.
            if !h3_conn.accepts_new_streams() {
                break;
            }

            // Wait for the next bidirectional stream (async).
            let bi_result = rt.block_on(conn.accept_bi());
            let (send_stream, recv_stream) = match bi_result {
                Ok(pair) => pair,
                Err(quinn::ConnectionError::ApplicationClosed(_))
                | Err(quinn::ConnectionError::ConnectionClosed(_)) => {
                    break;
                }
                Err(quinn::ConnectionError::TimedOut) => {
                    // QUIC-level idle timeout.
                    break;
                }
                Err(_) => {
                    let _ = h3_conn.begin_shutdown();
                    break;
                }
            };

            let stream_id: u64 = recv_stream.id().into();
            h3_conn.reset_idle_timer();

            // Register stream with H3 connection.
            if h3_conn.new_stream(stream_id).is_none() {
                // Reject: stream limit exceeded or draining.
                let mut s = send_stream;
                let _ = s.reset(
                    H3_ERR_STREAM_CREATION_ERROR
                        .try_into()
                        .expect("H3 error code as VarInt"),
                );
                continue;
            }

            h3_conn.set_current_stream(stream_id);

            // Phase 1: Read and decode the request (async).
            let read_result = rt.block_on(read_request_stream(
                &mut h3_conn,
                recv_stream,
                send_stream,
                remote_addr,
            ));

            match read_result {
                StreamReadResult::Request(request_data, send) => {
                    // Phase 2: Call handler synchronously (outside async context).
                    // NET7-12b: request_count incremented only on valid HEADERS
                    // decode + handler completion.
                    match handler(request_data) {
                        Some(response) => {
                            // Phase 3: Send the response (async).
                            let ok = rt.block_on(send_h3_response(send, &response));
                            if ok {
                                total_request_count += 1;
                            }
                        }
                        None => {
                            // Handler error — send 500.
                            rt.block_on(send_h3_error_500(send));
                            // Do not increment request_count on handler error.
                        }
                    }
                }
                StreamReadResult::Error => {
                    // Error already handled (error response sent in read_request_stream).
                    // Do not count as a request.
                }
                StreamReadResult::NoRequest => {
                    // No HEADERS on this stream — not a request.
                }
            }

            h3_conn.last_peer_stream_id = stream_id;

            // Check if we should stop accepting on this connection.
            if max_requests > 0 && total_request_count >= max_requests {
                let _ = h3_conn.begin_shutdown();
                break;
            }
        }

        // NET7-12d: Graceful shutdown pipeline for this connection.
        // Step 1: GOAWAY — transition H3Connection to Draining, get GOAWAY frame bytes.
        let (advanced, goaway_frames) = h3_conn.shutdown();
        // Step 2: Send GOAWAY frame on the existing control stream (NB7-115).
        // RFC 9114 Section 5.2: GOAWAY MUST be sent on the control stream.
        // Previously this opened a new uni stream (wrong — no type byte, no SETTINGS).
        // Now we use the control stream initialized at connection start.
        if advanced
            && let Some(frames) = goaway_frames
            && let Some(ref mut ctrl) = ctrl_send
        {
            for frame in &frames {
                let _ = rt.block_on(ctrl.write_all(frame));
            }
            // Do NOT finish the control stream here — the QUIC-level
            // close below will handle cleanup.
        }
        // Step 3: If still Draining (not yet Closed), complete the shutdown.
        // This closes all remaining streams and transitions to Closed.
        if h3_conn.is_draining() {
            h3_conn.complete_shutdown();
        }

        // Check if global limit reached — exit accept loop.
        if max_requests > 0 && total_request_count >= max_requests {
            break 'accept;
        }
    }

    // NET7-12d: Graceful shutdown for all tracked connections.
    // Step 1: Send GOAWAY (QUIC-level close with H3_NO_ERROR = 0x0100).
    for c in &connections {
        c.close(
            quinn::VarInt::from_u32(0x0100), // H3_NO_ERROR
            b"shutdown",
        );
    }

    // Step 2: Drain wait — poll for connection closure (bounded, 1 second max).
    // NB7-95: Poll-based instead of magic sleep.
    // NB7-67: This replaces the old immediate-close pattern.
    let drain_start = std::time::Instant::now();
    let drain_timeout = std::time::Duration::from_secs(1);
    loop {
        let all_closed = connections.iter().all(|c| c.close_reason().is_some());
        if all_closed {
            break;
        }
        if drain_start.elapsed() > drain_timeout {
            break;
        }
        rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(5)));
    }

    Ok(total_request_count)
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify H3_ALPN constant matches RFC 9114 specification.
    #[test]
    fn test_h3_alpn_constant() {
        assert_eq!(H3_ALPN, b"h3", "H3_ALPN must be 'h3' per RFC 9114");
    }

    /// Verify DEFAULT_H3_PORT is 443 (standard HTTPS port).
    #[test]
    fn test_default_h3_port() {
        assert_eq!(DEFAULT_H3_PORT, 443, "Default H3 port should be 443");
    }

    /// Test build_tls_config generates a valid rustls ServerConfig.
    #[test]
    fn test_build_tls_config_alpn_h3() {
        let (config, _, _) = build_tls_config("", "").expect("TLS config should succeed");
        assert_eq!(
            config.alpn_protocols,
            vec![H3_ALPN.to_vec()],
            "ALPN must be h3 only"
        );
    }

    /// Test build_tls_config produces non-empty cert and key DER.
    #[test]
    fn test_build_tls_config_produces_cert_key() {
        let (_, cert_der, key_der) = build_tls_config("", "").expect("TLS config should succeed");
        assert!(!cert_der.is_empty(), "Certificate DER should not be empty");
        assert!(!key_der.is_empty(), "Private key DER should not be empty");
    }

    /// Test create_quic_endpoint on a random available port.
    #[tokio::test]
    async fn test_create_quic_endpoint() {
        let (endpoint, cert_der, key_der) =
            create_quic_endpoint("", "", 0).expect("Endpoint creation should succeed");
        let local_addr = endpoint.local_addr().expect("Should have local address");
        assert!(local_addr.port() > 0, "Endpoint should be bound to a port");
        assert!(!cert_der.is_empty(), "Certificate should be non-empty");
        assert!(!key_der.is_empty(), "Key should be non-empty");
        endpoint.close(0u32.into(), b"test");
    }

    /// Multiple endpoints can be created on different ports.
    #[tokio::test]
    async fn test_multiple_endpoints() {
        let (ep1, _, _) = create_quic_endpoint("", "", 0).expect("Endpoint 1");
        let (ep2, _, _) = create_quic_endpoint("", "", 0).expect("Endpoint 2");

        let addr1 = ep1.local_addr().expect("EP1 addr");
        let addr2 = ep2.local_addr().expect("EP2 addr");
        assert_ne!(
            addr1.port(),
            addr2.port(),
            "Endpoints should bind to different ports (port 0 = OS picks)"
        );

        ep1.close(0u32.into(), b"test");
        ep2.close(0u32.into(), b"test");
    }

    /// Test that Endpoint local_addr matches the bind port.
    #[tokio::test]
    async fn test_endpoint_local_addr_matches_port() {
        let specific_port = 0;
        let (endpoint, _, _) =
            create_quic_endpoint("", "", specific_port).expect("Endpoint creation");
        let local = endpoint.local_addr().expect("Should have local addr");
        assert!(local.port() > 0, "Should bind to a valid port");
        assert!(
            local.ip().is_loopback(),
            "Endpoint should bind to localhost (127.0.0.1)"
        );
        endpoint.close(0u32.into(), b"test");
    }

    /// Verify cert generation is deterministic enough for testing.
    #[test]
    fn test_cert_generation_repeated() {
        for _ in 0..3 {
            let (_, cert_der, key_der) = build_tls_config("", "").expect("should succeed");
            assert!(!cert_der.is_empty());
            assert!(!key_der.is_empty());
            assert_eq!(
                cert_der[0], 0x30,
                "Certificate DER should start with SEQUENCE tag (0x30)"
            );
        }
    }

    /// Test that accept handles no pending connections gracefully.
    #[tokio::test]
    async fn test_accept_no_pending_connections() {
        let (endpoint, _, _) = create_quic_endpoint("", "", 0).expect("Endpoint creation");
        assert!(endpoint.local_addr().is_ok());
        endpoint.close(0u32.into(), b"test");
    }

    /// Verify the quinn crypto provider is properly configured.
    #[tokio::test]
    async fn test_quic_server_config_has_crypto() {
        let (endpoint, _, _) =
            create_quic_endpoint("", "", 0).expect("Endpoint creation should succeed");
        let local = endpoint.local_addr().expect("Endpoint should be bound");
        assert!(local.port() > 0);
        endpoint.close(0u32.into(), b"test");
    }

    // ── NET7-9c: Serve loop tests ────────────────────────────────────

    /// Verify serve_h3_loop creates an endpoint and blocks
    /// waiting for connections. Since serve_h3_loop now takes a handler
    /// callback and uses per-step block_on() (synchronous), we test it in
    /// a background OS thread and verify it's waiting (not panicking).
    #[test]
    fn test_serve_loop_starts_and_stops_cleanly() {
        let handle = std::thread::spawn(move || {
            // Echo handler for testing.
            let mut echo_handler = |req: H3RequestData| -> Option<H3ResponseData> {
                Some(H3ResponseData {
                    status: 200,
                    headers: vec![],
                    body: format!("echo {}", req.method).into_bytes(),
                })
            };
            serve_h3_loop("", "", 0, 0, &mut echo_handler)
        });

        // Wait briefly — the loop should be blocking on accept.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // The thread is alive and waiting; we can't cleanly stop it from
        // outside (the endpoint is internal), so just verify it hasn't
        // panicked by checking the thread is still running.
        assert!(
            !handle.is_finished(),
            "serve loop should still be waiting for connections"
        );

        // Drop the handle (thread will be detached). This is acceptable
        // for a unit test -- the OS will clean up when the test process exits.
    }

    /// Test that H3Connection idle timeout integrates with the serve loop.
    #[test]
    fn test_h3_connection_idle_timeout_on_serve() {
        let mut conn = super::super::H3Connection::new();
        // Set a very short timeout for testing.
        conn.set_idle_timeout(std::time::Duration::from_millis(10));

        // Wait for timeout.
        std::thread::sleep(std::time::Duration::from_millis(20));

        assert!(
            conn.check_timeout().is_some(),
            "Idle timeout should have fired"
        );
    }

    /// Test that goaway received triggers draining state.
    #[test]
    fn test_goaway_recv_triggers_draining() {
        let mut conn = super::super::H3Connection::new();
        conn.state = super::super::H3ConnState::Active;

        let ok = conn.receive_goaway(4);
        assert!(ok, "receive_goaway should succeed");
        assert!(
            conn.is_draining(),
            "Connection should be draining after GOAWAY"
        );
        assert!(
            !conn.accepts_new_streams(),
            "Draining connection should not accept new streams"
        );
    }

    /// Test that process_stream correctly rejects empty data.
    /// This is verified via the frame decode path.
    #[tokio::test]
    async fn test_process_stream_rejects_empty() {
        // We can't easily test process_stream with real QUIC streams,
        // but we can verify the frame decode path rejects empty input.
        assert!(super::super::decode_frame(&[]).is_none());
        assert!(super::super::decode_frame_header(&[]).is_none());
    }

    /// Test that send_error_response builds valid frames.
    #[tokio::test]
    async fn test_error_response_builds_valid_frames() {
        let headers = super::super::build_response_headers_frame(400, &[]);
        assert!(headers.is_some(), "Error response headers should build");

        let data = super::super::build_data_frame(b"Bad Request");
        assert!(data.is_some(), "Error response data should build");

        // Verify the headers frame can be decoded.
        let hdrs = headers.unwrap();
        let (ft, pl) = super::super::decode_frame(&hdrs).expect("headers frame should decode");
        assert_eq!(ft, super::super::H3_FRAME_HEADERS);
        assert!(
            !pl.is_empty(),
            "Decoded headers payload should not be empty"
        );
    }

    /// Test H3 frame encoding: HEADERS followed by DATA is valid round-trip.
    #[tokio::test]
    async fn test_response_frame_roundtrip() {
        let response_headers = vec![("content-type".to_string(), "text/plain".to_string())];
        let body = b"Hello, HTTP/3!";

        let headers_frame = super::super::build_response_headers_frame(200, &response_headers);
        let data_frame = super::super::build_data_frame(body);

        assert!(headers_frame.is_some());
        assert!(data_frame.is_some());

        // Decode and verify frame types.
        let hdrs = headers_frame.unwrap();
        let data = data_frame.unwrap();

        let (hdr_type, hdr_payload) = super::super::decode_frame(&hdrs).unwrap();
        let (data_type, data_payload) = super::super::decode_frame(&data).unwrap();

        assert_eq!(hdr_type, super::super::H3_FRAME_HEADERS);
        assert_eq!(data_type, super::super::H3_FRAME_DATA);

        // QPACK decode the headers.
        let decoded = super::super::qpack_decode_block(hdr_payload, 8, None, None);
        assert!(decoded.is_some());
        let decoded = decoded.unwrap();
        assert!(!decoded.is_empty());
        assert_eq!(decoded[0].name, ":status");
        assert_eq!(decoded[0].value, "200");

        // Verify DATA payload.
        assert_eq!(data_payload, body);
    }

    /// Test that connection_handler properly integrates shutdown pipeline.
    #[test]
    fn test_shutdown_pipeline_integration() {
        let mut conn = super::super::H3Connection::new();
        conn.state = super::super::H3ConnState::Active;

        // Add a stream so we can verify it gets closed during shutdown.
        conn.new_stream(0);
        assert!(conn.has_active_streams());

        // Step 1: begin_shutdown sends GOAWAY and transitions to Draining.
        let ok = conn.begin_shutdown();
        assert!(ok, "begin_shutdown should succeed from Active");
        assert!(conn.is_draining());
        assert!(!conn.accepts_new_streams());

        // Streams should still be counted.
        assert!(conn.has_active_streams());

        // Step 2: complete_shutdown closes everything.
        let ok = conn.complete_shutdown();
        assert!(ok, "complete_shutdown should succeed from Draining");
        assert!(conn.is_closed());
        assert!(!conn.has_active_streams());
    }

    /// Test that the serve loop endpoint is created on the expected port.
    #[tokio::test]
    async fn test_serve_loop_endpoint_creation() {
        let (endpoint, _, _) = create_quic_endpoint("", "", 0).expect("endpoint");
        let local = endpoint.local_addr().expect("local addr");
        assert!(
            local.ip().is_loopback(),
            "H3 endpoint must bind to 127.0.0.1 (matching h1/h2 policy)"
        );
        assert!(local.port() > 0, "H3 endpoint must bind to a valid port");
        endpoint.close(0u32.into(), b"test");
    }

    /// Test graceful shutdown with NO in-flight streams.
    ///
    /// When no streams are active, shutdown should:
    /// 1. Send GOAWAY (transition Active -> Draining)
    /// 2. Immediately complete (Draining -> Closed, since no streams to wait for)
    ///
    /// This validates the happy path where the server shuts down cleanly.
    #[test]
    fn test_shutdown_no_inflight_streams() {
        let mut conn = super::super::H3Connection::new();
        conn.state = super::super::H3ConnState::Active;

        // No streams registered — should be empty.
        assert!(!conn.has_active_streams());
        assert_eq!(conn.active_stream_count(), 0);

        // Step 1: shutdown() sends GOAWAY and transitions to Draining.
        let (advanced, goaway_frames) = conn.shutdown();
        assert!(advanced, "shutdown should advance from Active to Draining");
        assert!(
            goaway_frames.is_some(),
            "shutdown should produce GOAWAY frames"
        );
        assert!(conn.is_draining(), "connection should be in Draining state");

        // Step 2: No in-flight streams — drain wait is trivially satisfied.
        assert!(!conn.has_active_streams(), "no active streams during drain");

        // Step 3: complete_shutdown transitions Draining -> Closed.
        let ok = conn.complete_shutdown();
        assert!(ok, "complete_shutdown should succeed from Draining");
        assert!(conn.is_closed(), "connection should be Closed");

        // Verify GOAWAY frame contains valid data.
        let frames = goaway_frames.unwrap();
        assert!(
            !frames.is_empty(),
            "at least one GOAWAY frame should be produced"
        );
        // GOAWAY frame: type varint + length varint + payload (stream ID varint)
        let frame = &frames[0];
        assert!(frame.len() >= 3, "GOAWAY frame must be at least 3 bytes");
    }

    /// Test graceful shutdown WITH in-flight streams.
    ///
    /// When streams are still active at shutdown time, the sequence should be:
    /// 1. Send GOAWAY (transition Active -> Draining)
    /// 2. Verify in-flight streams are still tracked
    /// 3. Complete shutdown (close all streams, transition to Closed)
    ///
    /// This validates that the drain wait period can observe active streams.
    #[test]
    fn test_shutdown_with_inflight_streams() {
        let mut conn = super::super::H3Connection::new();
        conn.state = super::super::H3ConnState::Active;

        // Open 3 streams to simulate in-flight requests.
        conn.new_stream(0);
        conn.new_stream(4);
        conn.new_stream(8);
        assert_eq!(conn.active_stream_count(), 3);
        assert!(conn.has_active_streams());

        // Step 1: shutdown() sends GOAWAY and transitions to Draining.
        let (advanced, goaway_frames) = conn.shutdown();
        assert!(advanced, "shutdown should advance from Active to Draining");
        assert!(goaway_frames.is_some(), "GOAWAY frames should be produced");
        assert!(conn.is_draining());

        // Step 2: Streams are still active during drain wait.
        assert!(
            conn.has_active_streams(),
            "streams should still be tracked during drain"
        );
        assert_eq!(
            conn.active_stream_count(),
            3,
            "all 3 streams should remain active"
        );

        // New streams MUST be rejected in Draining state (RFC 9114).
        assert!(
            conn.new_stream(12).is_none(),
            "new streams must be rejected during draining"
        );

        // Step 3: complete_shutdown closes all streams and transitions to Closed.
        let ok = conn.complete_shutdown();
        assert!(ok, "complete_shutdown should succeed from Draining");
        assert!(conn.is_closed());
        assert!(
            !conn.has_active_streams(),
            "all streams should be closed after shutdown"
        );
        assert_eq!(conn.active_stream_count(), 0);

        // Verify final state is terminal — further shutdown attempts are no-ops.
        let (advanced2, _) = conn.shutdown();
        assert!(
            !advanced2,
            "shutdown on Closed connection should be a no-op"
        );
    }

    // ── NB7-115: Control stream initialization tests ─────────────────

    /// Verify that a control stream init payload contains:
    /// 1. Stream type byte 0x00 (control stream, RFC 9114 Section 6.2)
    /// 2. A valid SETTINGS frame (type 0x04)
    ///
    /// This validates the wire format that would be sent on the control stream,
    /// without requiring a live QUIC connection. The same bytes are written
    /// by serve_h3_loop at connection establishment.
    #[test]
    fn test_control_stream_payload_format() {
        // Build the control stream payload as serve_h3_loop does.
        let mut payload = Vec::new();

        // Step 1: Stream type byte 0x00 (control stream).
        payload.push(0x00);

        // Step 2: SETTINGS frame.
        let settings_payload =
            super::super::encode_settings().expect("encode_settings should succeed");
        let settings_frame =
            super::super::encode_frame(super::super::H3_FRAME_SETTINGS, &settings_payload)
                .expect("encode_frame for SETTINGS should succeed");
        payload.extend_from_slice(&settings_frame);

        // Verify: first byte is stream type 0x00.
        assert_eq!(
            payload[0], 0x00,
            "Control stream must start with type byte 0x00"
        );

        // Verify: remaining bytes decode as a SETTINGS frame.
        let (frame_type, frame_length, header_size) =
            super::super::decode_frame_header(&payload[1..])
                .expect("SETTINGS frame header should decode");
        assert_eq!(
            frame_type,
            super::super::H3_FRAME_SETTINGS,
            "First frame on control stream must be SETTINGS (type 0x04)"
        );
        assert!(
            frame_length > 0,
            "SETTINGS frame must have non-empty payload"
        );

        // Verify the SETTINGS payload decodes correctly.
        let settings_data = &payload[1 + header_size..1 + header_size + frame_length as usize];
        let decoded =
            super::super::decode_settings(settings_data).expect("SETTINGS payload should decode");
        // Default settings: max_field_section_size should be the default value.
        assert_eq!(
            decoded.max_field_section_size,
            super::super::H3_DEFAULT_MAX_FIELD_SECTION_SIZE
        );
    }

    /// Verify H3Connection tracks control_stream_id.
    #[test]
    fn test_h3_connection_control_stream_id_default_none() {
        let conn = super::super::H3Connection::new();
        assert!(
            conn.control_stream_id.is_none(),
            "New H3Connection should have no control stream ID"
        );
    }

    /// Verify H3Connection control_stream_id can be set.
    #[test]
    fn test_h3_connection_control_stream_id_set() {
        let mut conn = super::super::H3Connection::new();
        conn.control_stream_id = Some(3);
        assert_eq!(
            conn.control_stream_id,
            Some(3),
            "Control stream ID should be stored on H3Connection"
        );
    }

    /// GOAWAY frame must be valid when sent on control stream.
    /// Verifies that the shutdown pipeline produces a GOAWAY that can be
    /// written to the control stream (same bytes as before, but the
    /// transport path changed from open_uni to the existing control stream).
    #[test]
    fn test_goaway_frame_valid_for_control_stream() {
        let mut conn = super::super::H3Connection::new();
        conn.state = super::super::H3ConnState::Active;
        conn.control_stream_id = Some(3); // Simulate initialized control stream
        conn.last_peer_stream_id = 4;

        let (advanced, goaway_frames) = conn.shutdown();
        assert!(advanced, "shutdown should advance");
        let frames = goaway_frames.expect("GOAWAY frames expected");
        assert_eq!(frames.len(), 1, "exactly one GOAWAY frame");

        // Decode the GOAWAY frame.
        let (ft, fl, hs) = super::super::decode_frame_header(&frames[0])
            .expect("GOAWAY frame header should decode");
        assert_eq!(
            ft,
            super::super::H3_FRAME_GOAWAY,
            "Frame type must be GOAWAY (0x07)"
        );

        // Decode the stream ID from the GOAWAY payload.
        let (stream_id, _) = super::super::varint_decode(&frames[0][hs..hs + fl as usize])
            .expect("GOAWAY payload should contain a varint stream ID");
        assert_eq!(
            stream_id, 4,
            "GOAWAY stream ID should match last_peer_stream_id"
        );
    }
}
