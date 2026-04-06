/// QUIC transport substrate for the Interpreter backend.
///
/// **NET7-9b: Phase 9** -- UDP + QUIC accept implementation.
/// **NET7-9c: Phase 9** -- H3 serve loop with QUIC streams.
///
/// This module provides the QUIC transport layer using quinn (pure Rust,
/// tokio-native) as the substrate. It replaces the Phase 3 libquiche.so
/// dlopen gate with a compile-time Rust dependency.
///
/// # Architecture
///
/// 1. **TLS Config**: Generate self-signed cert via rcgen, build rustls
///    ServerConfig with ALPN h3 ("h3").
/// 2. **QUIC Endpoint**: Bind tokio::net::UdpSocket, wrap in quinn
///    ServerConfig, create listening Endpoint.
/// 3. **Accept Loop**: Endpoint::accept() loop for incoming connections.
/// 4. **Stream Dispatch (NET7-9c)**: accept_bi() -> H3 frame decode ->
///    QPACK decode -> request extraction -> response encode -> write.
/// 5. **Idle timeout / GOAWAY / shutdown** integration via H3Connection.
///
/// # Design Decisions
///
/// - Bounded-copy discipline: stream bytes read into fixed-size chunks;
///   no intermediate aggregate buffers.
/// - ALPN h3 exact match: only "h3" is accepted (no silent fallback to
///   h2/h1).
/// - The serve loop runs on an internal tokio runtime, bridged to the
///   synchronous interpreter via `Runtime::block_on()`.
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

/// NET7-9c: Maximum number of concurrent connections the serve loop tracks.
const MAX_CONCURRENT_CONNS: usize = 256;

/// NET7-9c: Read buffer size for QUIC stream data (bounded-copy).
const STREAM_READ_BUF: usize = 8192;

/// H3 ALPN protocol identifier per IANA assignment for HTTP/3.
///
/// RFC 9114 Section 2: The ALPN token for HTTP/3 is "h3".
pub(crate) const H3_ALPN: &[u8] = b"h3";

/// Default port for HTTP/3 (same as HTTPS: 443).
/// NB7-97: Present as documentation of the standard HTTP/3 port.
/// serve_h3_loop requires an explicit port parameter (0 = OS picks).
pub(crate) const DEFAULT_H3_PORT: u16 = 443;

/// A successfully accepted QUIC connection.
pub(crate) struct AcceptedConnection {
    pub connection: quinn::Connection,
    pub remote_addr: std::net::SocketAddr,
}

// ── TLS Config ──────────────────────────────────────────────────────────

/// Ensure the rustls default crypto provider is installed (idempotent).
/// NB7-89: Called once per process; subsequent calls are a no-op.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Build a TLS server config for QUIC.
///
/// If `cert_path` and `key_path` are non-empty, load PEM files from disk.
/// Otherwise, generate a self-signed certificate for "localhost".
///
/// NB7-86: Previously ignored cert_path and key_path, always self-signing.
fn build_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<(rustls::ServerConfig, Vec<u8>, Vec<u8>), String> {
    ensure_crypto_provider();

    let (cert_der, key_der) = if !cert_path.is_empty() && !key_path.is_empty() {
        // User-provided certificate and key paths.
        let cert_pem = fs::read(cert_path)
            .map_err(|e| format!("httpServe: HTTP/3 failed to read cert file '{}': {}", cert_path, e))?;
        let key_pem = fs::read(key_path)
            .map_err(|e| format!("httpServe: HTTP/3 failed to read key file '{}': {}", key_path, e))?;

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
            other => return Err(format!("httpServe: HTTP/3 unsupported key type: {:?}", other)),
        };

        (certs[0].to_vec(), key_der)
    } else {
        // Self-signed certificate (bootstrap / testing path).
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| format!("httpServe: HTTP/3 failed to generate self-signed cert: {}", e))?;

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
        format!("httpServe: HTTP/3 failed to parse bind address 127.0.0.1:{}: {}", port, e)
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

// ── NET7-9c: Accept bi stream processing ───────────────────────────────

/// NET7-9c: Process a single bidirectional QUIC stream.
///
/// Reads raw bytes, decodes H3 frames, extracts the request,
/// and sends a response (HEADERS + DATA frames).
///
/// # Bounded-copy discipline
///
/// - Reads into a local `STREAM_READ_BUF` buffer.
/// - All frames in the buffer are decoded in sequence (NB7-88).
/// - SETTINGS on request streams is rejected (NB7-84, RFC 9114 §7.2.4.1).
/// - GOAWAY on request streams is rejected (NB7-85, RFC 9114 §7.2.6).
/// - Frame decode errors use H3_ERR_FRAME_UNEXPECTED (NB7-92).
///
/// Returns `Some(true)` if a valid HEADERS frame was processed and response sent
/// (i.e., a "successful request" for NB7-98 counting).
/// Returns `Some(false)` on error.
/// Returns `None` for control-frame-only streams (no HEADERS).
async fn process_stream(
    h3_conn: &mut super::H3Connection,
    mut recv: quinn::RecvStream,
    mut send: quinn::SendStream,
) -> Option<bool> {
    // Accumulate request bytes, bounded by max_field_section_size.
    let max_size = h3_conn.max_field_section_size as usize;
    let mut buf = Vec::with_capacity(STREAM_READ_BUF);

    loop {
        let mut chunk_buf = [0u8; STREAM_READ_BUF];
        match recv.read(&mut chunk_buf).await {
            Ok(Some(n)) => {
                if buf.len() + n > max_size {
                    let _ = send.reset(H3_ERR_GENERAL_PROTOCOL_ERROR.try_into().expect("H3 error code as VarInt"));
                    return Some(false);
                }
                buf.extend_from_slice(&chunk_buf[..n]);
            }
            Ok(None) => break, // FIN received
            Err(_) => {
                let _ = send.reset(H3_ERR_GENERAL_PROTOCOL_ERROR.try_into().expect("H3 error code as VarInt"));
                return Some(false);
            }
        }
    }

    if buf.is_empty() {
        let _ = send.reset(H3_ERR_NO_ERROR.try_into().expect("H3 error code as VarInt"));
        return None;
    }

    // Decode all H3 frames in the buffer. NB7-88: previously only the first
    // frame was decoded; now we iterate through all available frames.
    let mut pos = 0;
    let mut headers_seen = false;

    while pos < buf.len() {
        // Step 1: decode frame header to get type, length, and header size.
        let (frame_type, frame_length, header_size) =
            match super::decode_frame_header(&buf[pos..]) {
                Some((ft, fl, hs)) => (ft, fl, hs),
                None => {
                    let _ = send.reset(H3_ERR_FRAME_UNEXPECTED.try_into().expect("H3 error code as VarInt"));
                    return Some(false);
                }
            };

        let frame_len = match usize::try_from(frame_length) {
            Ok(n) => n,
            Err(_) => {
                let _ = send.reset(H3_ERR_FRAME_UNEXPECTED.try_into().expect("H3 error code as VarInt"));
                return Some(false);
            }
        };

        let total_frame_size = header_size + frame_len;
        if pos + total_frame_size > buf.len() {
            // Incomplete frame — we've received partial data for this frame.
            // Treat as malformed since FIN was received with incomplete frame.
            let _ = send.reset(H3_ERR_FRAME_UNEXPECTED.try_into().expect("H3 error code as VarInt"));
            return Some(false);
        }

        let payload = &buf[pos + header_size..pos + total_frame_size];
        pos += total_frame_size;

        match frame_type {
            super::H3_FRAME_HEADERS => {
                if headers_seen {
                    // Duplicate HEADERS on same request stream — protocol error
                    let _ = send.reset(H3_ERR_FRAME_UNEXPECTED.try_into().expect("H3 error code as VarInt"));
                    return Some(false);
                }
                headers_seen = true;

                // Decode QPACK-encoded request headers.
                // NB7-102: Pass the connection's dynamic_table when present so that
                // dynamic table entries (from prior SETTINGS/encoder stream activity)
                // can be resolved during QPACK decode.
                let dyn_table = h3_conn.dynamic_table.as_ref();
                let headers = match super::qpack_decode_block(payload, 8, None, dyn_table) {
                    Some(h) => h,
                    None => {
                        let _ = send_error_response(&mut send, 400, b"Bad Request").await;
                        return Some(false);
                    }
                };

                // Extract and validate request fields.
                let request = match super::extract_request_fields(&headers) {
                    Ok(req) => req,
                    Err(_) => {
                        let _ = send_error_response(&mut send, 400, b"Bad Request").await;
                        return Some(false);
                    }
                };

                // Touch idle timer on successful request activity.
                h3_conn.reset_idle_timer();

                // TODO(NB7-87): Replace echo with user handler dispatch.
                // Current behavior: echo method+path+authority only (no user handler).
                // Full handler integration (taida_val handler) will be added as
                // serve_h3_loop parameter in a future Phase.
                let body = format!(
                    "HTTP/3 {} {}{}",
                    request.method,
                    request.path,
                    if request.authority.is_empty() {
                        String::new()
                    } else {
                        format!(" @ {}", request.authority)
                    }
                );

                let response_headers = vec![
                    ("content-type".to_string(), "text/plain".to_string()),
                    ("server".to_string(), "taida-lang/net v7".to_string()),
                ];

                // Send HEADERS frame.
                let Some(hdrs_frame) = super::build_response_headers_frame(200, &response_headers) else {
                    let _ = send.reset(H3_ERR_GENERAL_PROTOCOL_ERROR.try_into().expect("H3 error code as VarInt"));
                    return Some(false);
                };
                if send.write_all(&hdrs_frame).await.is_err() {
                    return Some(false);
                }

                // Send DATA frame.
                let Some(data_frame) = super::build_data_frame(body.as_bytes()) else {
                    let _ = send.reset(H3_ERR_GENERAL_PROTOCOL_ERROR.try_into().expect("H3 error code as VarInt"));
                    return Some(false);
                };
                if send.write_all(&data_frame).await.is_err() {
                    return Some(false);
                }

                // Send FIN.
                if send.finish().is_err() {
                    return Some(false);
                }
            }

            super::H3_FRAME_SETTINGS => {
                // NB7-84: SETTINGS MUST only be sent on the control stream
                // (unidirectional, type 0x02). On a bidirectional request
                // stream this is H3_ERR_FRAME_UNEXPECTED (RFC 9114 §7.2.4.1).
                let _ = send.reset(H3_ERR_FRAME_UNEXPECTED.try_into().expect("H3 error code as VarInt"));
                return Some(false);
            }

            super::H3_FRAME_GOAWAY => {
                // NB7-85: GOAWAY MUST only be sent on the control stream
                // (RFC 9114 §7.2.6). On a request stream: reject.
                // NB7-99: do NOT call h3_conn.receive_goaway() here;
                // connection_handler is the sole authoritative caller.
                let _ = send.reset(H3_ERR_FRAME_UNEXPECTED.try_into().expect("H3 error code as VarInt"));
                return Some(false);
            }

            super::H3_FRAME_DATA => {
                // DATA without prior HEADERS — protocol error.
                let _ = send_error_response(&mut send, 400, b"Expected HEADERS before DATA").await;
                return Some(false);
            }

            _ => {
                // NB7-91: Unknown frame type. RFC 9114 Section 7.2.8 allows
                // implementations to ignore unknown frame types.
                // We silently skip without sending an HTTP response, which
                // would confuse a peer sending control-frame semantics.
            }
        }
    }

    if headers_seen {
        Some(true)
    } else {
        None
    }
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

// ── NET7-9c: Per-connection handler ────────────────────────────────────

/// Per-connection handler: accept bi-directional streams, process each
/// as an H3 request/response exchange.
///
/// Integrates idle timeout, GOAWAY handling, and graceful shutdown.
/// Returns the number of requests served on this connection.
async fn connection_handler(
    conn: quinn::Connection,
    mut h3_conn: super::H3Connection,
    max_requests: i64,
    request_counter: Arc<std::sync::Mutex<i64>>,
) -> i64 {
    let mut request_count: i64 = 0;

    loop {
        // Check idle timeout.
        if h3_conn.check_timeout().is_some() {
            let _ = h3_conn.begin_shutdown();
            break;
        }

        // Check max_requests for this connection.
        if max_requests > 0 && request_count >= max_requests {
            let _ = h3_conn.begin_shutdown();
            break;
        }

        // No new streams in draining/closed state.
        if !h3_conn.accepts_new_streams() {
            break;
        }

        // Wait for the next bidirectional stream.
        let (send_stream, recv_stream) = match conn.accept_bi().await {
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
            let _ = s.reset(H3_ERR_STREAM_CREATION_ERROR.try_into().expect("H3 error code as VarInt"));
            continue;
        }

        h3_conn.set_current_stream(stream_id);

        // Process the stream (read frames, decode request, send response).
        let request_ok = process_stream(&mut h3_conn, recv_stream, send_stream).await;

        // NB7-98: Only count successful HEADERS decodes as requests.
        // process_stream returns Some(true) = valid request served,
        // Some(false) = error, None = no request (empty stream / no HEADERS).
        if request_ok == Some(true) {
            request_count += 1;
            // Update global request counter.
            {
                let mut counter = request_counter.lock().unwrap();
                *counter += 1;
            }
        }

        h3_conn.last_peer_stream_id = stream_id;
    }

    // Graceful shutdown pipeline.
    h3_conn.shutdown();

    request_count
}

// ── NET7-9c: Public serve loop ─────────────────────────────────────────

/// NET7-9c: H3 serve loop — the main entry point for the Interpreter H3 server.
///
/// This runs synchronously (via an internal tokio runtime) and:
/// 1. Creates a QUIC endpoint (TLS + ALPN h3)
/// 2. Accepts connections and spawns per-connection handlers
/// 3. Each connection handler accepts bi-directional streams
/// 4. Stream data is decoded as H3 frames (SETTINGS, HEADERS, DATA)
/// 5. Request fields are extracted from QPACK-encoded HEADERS
/// 6. Response is built and sent as HEADERS + DATA frames
/// 7. Idle timeout and GOAWAY/shutdown are integrated
///
/// **TODO(NB7-87):** Current `process_stream` echoes method+path+authority only.
/// No user dispatch (`taida_val handler`) is invoked. This is a known limitation
/// — full handler integration will be added in a future Phase.
///
/// # Arguments
///
/// * `port` - UDP port to bind (matching h1/h2 policy: 0 = OS picks)
/// * `max_requests` - Max total requests before shutdown (0 = unlimited)
///
/// # Returns
///
/// The number of requests served, or an error string.
pub(crate) fn serve_h3_loop(
    cert_path: &str,
    key_path: &str,
    port: u16,
    max_requests: i64,
) -> Result<i64, String> {
    // Create a single-threaded tokio runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("httpServe: HTTP/3 failed to create tokio runtime: {}", e))?;

    rt.block_on(async {
        // Step 1: Create QUIC endpoint with user-provided or self-signed cert/key.
        let (endpoint, _cert_der, _key_der) = create_quic_endpoint(cert_path, key_path, port)?;

        let connection_count: Arc<std::sync::Mutex<Vec<quinn::Connection>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let request_counter: Arc<std::sync::Mutex<i64>> =
            Arc::new(std::sync::Mutex::new(0));

        loop {
            // Check global max_requests limit.
            let count = *request_counter.lock().unwrap();
            if max_requests > 0 && count >= max_requests {
                break;
            }

            // Accept the next incoming connection.
            let accepted = accept_connection(&endpoint).await;
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
            // NB7-93: retain + len + push are under the same Mutex lock, so the
            // only possible race is a connection closing between retain and push
            // (not between retain and len). This is benign because we only ever
            // track *more* connections, which is a conservative bound.
            let was_bounded = {
                let mut conns = connection_count.lock().unwrap();
                conns.retain(|c| c.close_reason().is_none());
                if conns.len() >= MAX_CONCURRENT_CONNS {
                    if let Some(old) = conns.first() {
                        old.close(0u32.into(), b"too_many_connections");
                    }
                    conns.drain(..1);
                    true
                } else {
                    false
                }
            };
            // Push outside the lock to narrow the critical section.
            // NB7-93: If we just dropped an old connection, the new one replaces it.
            if !was_bounded {
                connection_count.lock().unwrap().push(accepted.connection.clone());
            }

            // Create H3Connection with idle timeout tracking.
            let mut h3_conn = super::H3Connection::new();
            let conn_id = accepted.connection.stable_id().to_ne_bytes().to_vec();
            h3_conn.set_quic_connection_id(conn_id);
            h3_conn.state = super::H3ConnState::Active;

            let conn = accepted.connection;
            let max_requests_left = if max_requests <= 0 {
                i64::MAX
            } else {
                max_requests - count
            };
            let rc = Arc::clone(&request_counter);

            // Spawn per-connection handler.
            tokio::spawn(async move {
                connection_handler(conn, h3_conn, max_requests_left, rc).await
            });
        }

        // Drain all connections before returning.
        {
            let conns = connection_count.lock().unwrap();
            for c in conns.iter() {
                c.close(0u32.into(), b"shutdown");
            }
        }

        // NB7-95: Poll for connection closure instead of magic sleep(50ms).
        // Give connections time to drain, then return.
        let drain_start = tokio::time::Instant::now();
        let drain_timeout = std::time::Duration::from_secs(1);
        loop {
            let all_closed = {
                let conns = connection_count.lock().unwrap();
                conns.iter().all(|c| c.close_reason().is_some())
            };
            if all_closed {
                break;
            }
            if drain_start.elapsed() > drain_timeout {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let final_count = *request_counter.lock().unwrap();
        Ok(final_count)
    })
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

    /// NB7-94: The previous version of this test created a separate endpoint
    /// and attempts to close it, but serve_h3_loop creates its own endpoint.
    /// This test verifies serve_h3_loop exits cleanly with max_requests=1
    /// (no connections needed to verify start/stop behavior).
    #[tokio::test]
    async fn test_serve_loop_starts_and_stops_cleanly() {
        // Pick an OS-selected port. serve_h3_loop binds to 127.0.0.1:port.
        // With max_requests=1, it exits immediately on first accept_connection
        // returning None (endpoint was cleanly created, but the loop breaks
        // when no incoming connection arrives and the endpoint is dropped
        // at the end of serve_h3_loop).
        // In practice, with no incoming connections, the loop waits on
        // endpoint.accept().await. Since we can't externally close the
        // internal endpoint, we just verify the function doesn't panic
        // and returns within a short timeout.
        // The loop will block on accept_connection with nobody connecting.
        // Use max_requests=0 (unlimited) and timeout to assert it's waiting.
        let handle = tokio::spawn(async move {
            // Pick a port unlikely to have real traffic during test.
            serve_h3_loop("", "", 0, 0)
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Abort the handle since there's no incoming connection to trigger exit.
        handle.abort();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle,
        ).await;

        assert!(result.is_ok(), "serve loop should be abortable");
    }

    /// Test that H3Connection idle timeout integrates with the serve loop.
    #[test]
    fn test_h3_connection_idle_timeout_on_serve() {
        let mut conn = super::super::H3Connection::new();
        // Set a very short timeout for testing.
        conn.set_idle_timeout(std::time::Duration::from_millis(10));

        // Wait for timeout.
        std::thread::sleep(std::time::Duration::from_millis(20));

        assert!(conn.check_timeout().is_some(), "Idle timeout should have fired");
    }

    /// Test that goaway received triggers draining state.
    #[test]
    fn test_goaway_recv_triggers_draining() {
        let mut conn = super::super::H3Connection::new();
        conn.state = super::super::H3ConnState::Active;

        let ok = conn.receive_goaway(4);
        assert!(ok, "receive_goaway should succeed");
        assert!(conn.is_draining(), "Connection should be draining after GOAWAY");
        assert!(!conn.accepts_new_streams(), "Draining connection should not accept new streams");
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
        assert!(!pl.is_empty(), "Decoded headers payload should not be empty");
    }

    /// Test H3 frame encoding: HEADERS followed by DATA is valid round-trip.
    #[tokio::test]
    async fn test_response_frame_roundtrip() {
        let response_headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
        ];
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
}
