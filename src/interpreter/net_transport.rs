/// Transport abstraction for `taida-lang/net` v5.
///
/// Defines the `Transport` trait that unifies plaintext TCP and TLS connections
/// behind a common interface. All HTTP serve / WebSocket I/O paths use this
/// trait so that TLS can be introduced without changing handler dispatch or
/// streaming logic.
///
/// # Design (NET_DESIGN.md Phase 1)
///
/// Transport operations:
///   - `read`     — read bytes from the connection (plaintext or TLS-decrypted)
///   - `write`    — write bytes to the connection (plaintext or TLS-encrypted)
///   - `flush`    — flush any buffered write data
///   - `shutdown` — shutdown the write side (TCP shutdown or TLS close_notify + TCP shutdown)
///   - `set_read_timeout` / `set_write_timeout` — per-connection deadline control
///
/// TLS / plaintext branching happens once at `httpServe` startup:
///   - `tls <= @()` → `PlaintextTransport` wrapping `TcpStream`
///   - `tls <= @(cert: ..., key: ...)` → `TlsTransport` wrapping `rustls::ServerConnection` + `TcpStream`
///
/// The `TransportAcceptor` trait abstracts the listener-level accept, producing
/// either a plaintext or TLS transport for each incoming connection.
use std::io;
use std::io::Read as _;
use std::net::SocketAddr;
use std::time::Duration;

// ── Transport trait ──────────────────────────────────────────────────

/// Unified read/write interface for a single connection.
///
/// Implementors: `PlaintextTransport` (v4 path), `TlsTransport` (v5 Phase 2).
/// The trait uses `&mut self` because connections are not shared across threads
/// (the Interpreter is single-threaded, `!Send`).
pub(crate) trait Transport {
    /// Read bytes into `buf`. Returns the number of bytes read (0 = EOF).
    /// On WouldBlock/TimedOut the implementation should return the appropriate io::Error.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Write all bytes in `buf` to the connection.
    /// Equivalent to `write_all` — retries partial writes internally.
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;

    /// Flush any buffered data to the underlying stream.
    fn flush(&mut self) -> io::Result<()>;

    /// Shutdown the write side of the connection.
    /// For plaintext: `TcpStream::shutdown(Shutdown::Write)`.
    /// For TLS: send `close_notify` alert, then TCP shutdown.
    fn shutdown_write(&mut self) -> io::Result<()>;

    /// Set the read timeout for subsequent `read` calls.
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()>;

    /// Set the write timeout for subsequent `write` calls.
    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()>;

    /// Get the peer address of the connection.
    fn peer_addr(&self) -> io::Result<SocketAddr>;

    /// Get a mutable reference to the underlying `TcpStream`.
    /// Used by legacy code paths that need direct stream access (WebSocket frame I/O, etc.).
    /// This will be removed or deprecated once all I/O goes through the Transport trait.
    fn as_tcp_stream_mut(&mut self) -> &mut std::net::TcpStream;

    /// Whether this transport uses TLS.
    fn is_tls(&self) -> bool;
}

// ── PlaintextTransport ───────────────────────────────────────────────

/// Plaintext TCP transport. Wraps a `TcpStream` directly.
/// This is the v4-compatible path where no TLS is involved.
pub(crate) struct PlaintextTransport {
    stream: std::net::TcpStream,
}

impl PlaintextTransport {
    pub fn new(stream: std::net::TcpStream) -> Self {
        PlaintextTransport { stream }
    }
}

impl Transport for PlaintextTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.stream, buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        io::Write::write_all(&mut self.stream, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.stream)
    }

    fn shutdown_write(&mut self) -> io::Result<()> {
        self.stream.shutdown(std::net::Shutdown::Write)
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }

    fn as_tcp_stream_mut(&mut self) -> &mut std::net::TcpStream {
        &mut self.stream
    }

    fn is_tls(&self) -> bool {
        false
    }
}

// ── TlsTransport ────────────────────────────────────────────────────

/// TLS transport. Wraps a `rustls::ServerConnection` + `TcpStream`.
/// All read/write operations go through rustls for encryption/decryption.
///
/// NET5-2a: Phase 2 implementation. cert/key are loaded at httpServe startup.
/// TLS handshake is performed during accept (before the connection enters the pool).
pub(crate) struct TlsTransport {
    tls_conn: rustls::ServerConnection,
    stream: std::net::TcpStream,
}

impl TlsTransport {
    pub fn new(tls_conn: rustls::ServerConnection, stream: std::net::TcpStream) -> Self {
        TlsTransport { tls_conn, stream }
    }

    /// Get a reference to the underlying TcpStream.
    /// Used for timeout/shutdown/peer_addr operations on the raw socket.
    pub fn stream_ref(&self) -> &std::net::TcpStream {
        &self.stream
    }

    /// Get the ALPN-negotiated protocol after TLS handshake.
    /// Returns `Some(b"h2")` for HTTP/2, `Some(b"http/1.1")` for HTTP/1.1,
    /// or `None` if ALPN was not negotiated.
    ///
    /// NET6-2a: Used by httpServe to dispatch to the h2 or h1 connection loop.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        self.tls_conn.alpn_protocol()
    }

    /// Drain all pending TLS ciphertext to the TCP stream.
    ///
    /// rustls may buffer multiple TLS records internally (e.g. when a single
    /// `write_all` produces more ciphertext than one `write_tls` call can push,
    /// or when the underlying TCP write is partial). This helper loops until
    /// `wants_write()` returns false, matching the pattern used in
    /// `complete_tls_handshake`.
    ///
    /// NB5-10: Without this drain loop, HTTPS responses, streaming body chunks,
    /// and `close_notify` alerts can be partially sent and stall the client.
    fn drain_tls_write(&mut self) -> io::Result<()> {
        while self.tls_conn.wants_write() {
            self.tls_conn.write_tls(&mut self.stream)?;
        }
        Ok(())
    }
}

impl Transport for TlsTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Process any pending TLS data from the TCP stream into the rustls state.
        // Then read decrypted application data.
        loop {
            // First, try to read already-decrypted data from the rustls buffer.
            match self.tls_conn.reader().read(buf) {
                Ok(n) if n > 0 => return Ok(n),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // No decrypted data available yet; need to read more TLS records.
                }
                Ok(0) => {
                    // Clean TLS closure (close_notify received).
                    return Ok(0);
                }
                Err(e) => return Err(e),
                _ => {}
            }

            // Read raw TLS records from the TCP stream.
            match self.tls_conn.read_tls(&mut self.stream) {
                Ok(0) => return Ok(0), // TCP EOF
                Ok(_) => {
                    // Process the newly read TLS records.
                    let io_state = self.tls_conn.process_new_packets().map_err(|e| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("TLS error: {}", e))
                    })?;
                    if io_state.tls_bytes_to_write() > 0 {
                        // Flush any TLS protocol responses (e.g., handshake, alerts).
                        // NB5-10: drain all pending ciphertext, not just one write_tls call.
                        self.drain_tls_write()?;
                    }
                    // Loop back to try reading decrypted data.
                    continue;
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        // Write plaintext data into rustls, which encrypts it.
        io::Write::write_all(&mut self.tls_conn.writer(), buf)?;
        // NB5-10: drain all pending ciphertext (large payloads may produce
        // multiple TLS records that a single write_tls cannot push entirely).
        self.drain_tls_write()?;
        io::Write::flush(&mut self.stream)?;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.tls_conn.writer())?;
        // NB5-10: drain all pending ciphertext before flushing the TCP stream.
        self.drain_tls_write()?;
        io::Write::flush(&mut self.stream)
    }

    fn shutdown_write(&mut self) -> io::Result<()> {
        // Send TLS close_notify alert.
        self.tls_conn.send_close_notify();
        // NB5-10: drain all pending ciphertext (close_notify + any buffered data).
        self.drain_tls_write()?;
        io::Write::flush(&mut self.stream)?;
        // Then shutdown the TCP write side.
        self.stream.shutdown(std::net::Shutdown::Write)
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }

    fn as_tcp_stream_mut(&mut self) -> &mut std::net::TcpStream {
        &mut self.stream
    }

    fn is_tls(&self) -> bool {
        true
    }
}

// ── TLS cert/key loading ────────────────────────────────────────────

/// Load TLS server config from PEM cert and key file paths.
///
/// Returns a `rustls::ServerConfig` ready for use in a `TlsAcceptor`.
/// Errors are returned as descriptive strings for httpServe to wrap in Result failure.
///
/// NET5-0c: cert/key are PEM file paths. CA chain should be included in cert file.
/// Self-signed certs are accepted (validation is client's responsibility).
pub(crate) fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<std::sync::Arc<rustls::ServerConfig>, String> {
    use std::fs::File;
    use std::io::BufReader;

    // Ensure the rustls default CryptoProvider is installed (idempotent).
    // rustls 0.23 requires an explicit provider; aws-lc-rs is our default.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Read cert chain.
    let cert_file = File::open(cert_path)
        .map_err(|e| format!("httpServe: failed to open cert file '{}': {}", cert_path, e))?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                format!(
                    "httpServe: failed to parse cert file '{}': {}",
                    cert_path, e
                )
            })?;
    if certs.is_empty() {
        return Err(format!(
            "httpServe: cert file '{}' contains no certificates",
            cert_path
        ));
    }

    // Read private key.
    let key_file = File::open(key_path)
        .map_err(|e| format!("httpServe: failed to open key file '{}': {}", key_path, e))?;
    let mut key_reader = BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| format!("httpServe: failed to parse key file '{}': {}", key_path, e))?
        .ok_or_else(|| format!("httpServe: key file '{}' contains no private key", key_path))?;

    // Build server config.
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("httpServe: TLS config error: {}", e))?;

    Ok(std::sync::Arc::new(config))
}

/// Load TLS server config with ALPN protocol negotiation for HTTP/2.
///
/// NET6-2a: Configures ALPN to advertise ["h2", "http/1.1"] so that clients
/// can negotiate HTTP/2 during the TLS handshake. The negotiated protocol
/// is accessible via `TlsTransport::alpn_protocol()` after handshake.
///
/// h2c (cleartext HTTP/2) is out of scope per NET_DESIGN.md.
pub(crate) fn load_tls_config_h2(
    cert_path: &str,
    key_path: &str,
) -> Result<std::sync::Arc<rustls::ServerConfig>, String> {
    use std::fs::File;
    use std::io::BufReader;

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let cert_file = File::open(cert_path)
        .map_err(|e| format!("httpServe: failed to open cert file '{}': {}", cert_path, e))?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                format!(
                    "httpServe: failed to parse cert file '{}': {}",
                    cert_path, e
                )
            })?;
    if certs.is_empty() {
        return Err(format!(
            "httpServe: cert file '{}' contains no certificates",
            cert_path
        ));
    }

    let key_file = File::open(key_path)
        .map_err(|e| format!("httpServe: failed to open key file '{}': {}", key_path, e))?;
    let mut key_reader = BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| format!("httpServe: failed to parse key file '{}': {}", key_path, e))?
        .ok_or_else(|| format!("httpServe: key file '{}' contains no private key", key_path))?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("httpServe: TLS config error: {}", e))?;

    // NET6-2a: ALPN protocol negotiation for HTTP/2.
    // Advertise h2 first (preferred), then http/1.1 as fallback.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(std::sync::Arc::new(config))
}

// ── TransportAcceptor ────────────────────────────────────────────────

/// Result of accepting a new connection through a `TransportAcceptor`.
pub(crate) struct AcceptedTransport {
    pub transport: Box<dyn Transport>,
    pub peer_addr: SocketAddr,
}

/// Listener-level abstraction for accepting new connections.
///
/// `PlaintextAcceptor` wraps `TcpListener` directly.
/// `TlsAcceptor` wraps `TcpListener` + `rustls::ServerConfig` and performs TLS handshake.
pub(crate) trait TransportAcceptor {
    /// Try to accept a new connection (non-blocking).
    /// Returns `Ok(Some(...))` if a connection is ready, `Ok(None)` if WouldBlock,
    /// or `Err(...)` for fatal errors.
    fn try_accept(&self, handshake_timeout: Duration) -> io::Result<Option<AcceptedTransport>>;

    /// Whether this acceptor produces TLS connections.
    fn is_tls(&self) -> bool;
}

/// Plaintext TCP acceptor. Wraps a non-blocking `TcpListener`.
pub(crate) struct PlaintextAcceptor {
    listener: std::net::TcpListener,
}

impl PlaintextAcceptor {
    pub fn new(listener: std::net::TcpListener) -> Self {
        PlaintextAcceptor { listener }
    }
}

impl TransportAcceptor for PlaintextAcceptor {
    fn try_accept(&self, _handshake_timeout: Duration) -> io::Result<Option<AcceptedTransport>> {
        match self.listener.accept() {
            Ok((stream, peer_addr)) => Ok(Some(AcceptedTransport {
                transport: Box::new(PlaintextTransport::new(stream)),
                peer_addr,
            })),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn is_tls(&self) -> bool {
        false
    }
}

/// TLS acceptor. Wraps a non-blocking `TcpListener` + `rustls::ServerConfig`.
/// Performs TCP accept + blocking TLS handshake with a deadline.
///
/// NET5-0c: Handshake deadline uses `timeoutMs`. Handshake failure = connection close,
/// handler not called. Handshaking connections count toward `maxConnections`.
pub(crate) struct TlsAcceptor {
    listener: std::net::TcpListener,
    tls_config: std::sync::Arc<rustls::ServerConfig>,
}

impl TlsAcceptor {
    pub fn new(
        listener: std::net::TcpListener,
        tls_config: std::sync::Arc<rustls::ServerConfig>,
    ) -> Self {
        TlsAcceptor {
            listener,
            tls_config,
        }
    }
}

impl TransportAcceptor for TlsAcceptor {
    fn try_accept(&self, handshake_timeout: Duration) -> io::Result<Option<AcceptedTransport>> {
        let (stream, peer_addr) = match self.listener.accept() {
            Ok(pair) => pair,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        };

        // Set blocking mode with handshake deadline for TLS handshake.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(handshake_timeout))?;
        stream.set_write_timeout(Some(handshake_timeout))?;

        // Create TLS server connection.
        let tls_conn = rustls::ServerConnection::new(self.tls_config.clone())
            .map_err(|e| io::Error::other(format!("TLS server connection error: {}", e)))?;

        // Perform blocking TLS handshake.
        let mut tls_transport = TlsTransport::new(tls_conn, stream);
        if let Err(e) = complete_tls_handshake(&mut tls_transport, handshake_timeout) {
            // Handshake failure: close connection, don't call handler.
            // This is NET5-0c policy: handshake failure = connection close.
            let _ = tls_transport.stream.shutdown(std::net::Shutdown::Both);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                format!("TLS handshake failed: {}", e),
            ));
        }

        Ok(Some(AcceptedTransport {
            transport: Box::new(tls_transport),
            peer_addr,
        }))
    }

    fn is_tls(&self) -> bool {
        true
    }
}

/// Complete a TLS handshake by driving the rustls state machine until
/// it reports `is_handshaking() == false`.
///
/// Uses the stream's read/write timeouts for deadline enforcement.
pub(crate) fn complete_tls_handshake(
    transport: &mut TlsTransport,
    _timeout: Duration,
) -> io::Result<()> {
    while transport.tls_conn.is_handshaking() {
        // Read TLS records from the TCP stream.
        match transport.tls_conn.read_tls(&mut transport.stream) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed during TLS handshake",
                ));
            }
            Ok(_) => {}
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "TLS handshake timed out",
                ));
            }
            Err(e) => return Err(e),
        }

        // Process the TLS records.
        transport
            .tls_conn
            .process_new_packets()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("TLS error: {}", e)))?;

        // Write any pending TLS handshake data back.
        while transport.tls_conn.wants_write() {
            transport.tls_conn.write_tls(&mut transport.stream)?;
        }
        io::Write::flush(&mut transport.stream)?;
    }
    Ok(())
}

// ── Connection lifecycle state ───────────────────────────────────────

/// Connection state within the httpServe pool.
///
/// Replaces the previous `HttpConnection` struct to use transport abstraction.
/// Each connection owns its transport, buffer, and lifecycle metadata.
///
/// # Lifecycle
///
/// ```text
/// Accept → Handshake (TLS only) → ReadHead → ReadBody → Handler → WriteResponse → [KeepAlive | Close]
///          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
///          transport.read / transport.write
/// ```
pub(crate) struct TransportConnection {
    /// The transport (plaintext or TLS) for this connection.
    pub transport: Box<dyn Transport>,
    /// Peer address.
    pub peer_addr: SocketAddr,
    /// Per-connection scratch buffer (allocated once, reused).
    pub buf: Vec<u8>,
    /// How many bytes are valid in buf.
    pub total_read: usize,
    /// How many requests have been processed on this connection.
    pub conn_requests: i64,
    /// Last activity timestamp (for idle timeout detection).
    pub last_activity: std::time::Instant,
}

// ── Timeout / Shutdown / Backpressure State ──────────────────────────

/// Shared configuration for connection management in the httpServe loop.
/// Centralizes timeout, shutdown, and backpressure parameters so that
/// all connection lifecycle decisions reference one source of truth.
///
/// # Backpressure (NET5-0b)
///
/// v5 runtime internally manages write backpressure via EAGAIN retry.
/// No explicit backpressure API is exposed to users. The `write_buffer_cap`
/// field provides a future hook for limiting queued write data per connection.
///
/// # Shutdown (NET5-0b)
///
/// Graceful shutdown drains pending connections up to `drain_timeout`.
/// Connections exceeding the drain deadline are force-closed.
pub(crate) struct ConnectionConfig {
    /// Per-connection read timeout (also used as TLS handshake deadline).
    pub read_timeout: Duration,
    /// Short poll timeout for non-blocking read attempts during connection polling.
    pub poll_timeout: Duration,
    /// Maximum simultaneous connections.
    pub max_connections: usize,
    /// Maximum total requests (0 = unlimited).
    pub max_requests: i64,
    /// Graceful shutdown drain timeout.
    /// When maxRequests is reached, existing connections are drained for up to
    /// this duration before being force-closed.
    pub drain_timeout: Duration,
}

impl ConnectionConfig {
    /// Create a new config from httpServe arguments.
    pub fn new(timeout_ms: u64, max_connections: usize, max_requests: i64) -> Self {
        let read_timeout = Duration::from_millis(timeout_ms);
        ConnectionConfig {
            read_timeout,
            poll_timeout: Duration::from_millis(10),
            max_connections,
            max_requests,
            // Drain timeout = same as read timeout (connections get one full
            // timeout window to complete their in-progress request).
            drain_timeout: read_timeout,
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn test_plaintext_transport_read_write() {
        // Create a TCP listener and connect to it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let mut client = std::net::TcpStream::connect(addr).unwrap();
        let (server_stream, _) = listener.accept().unwrap();

        let mut transport = PlaintextTransport::new(server_stream);

        // Client writes, transport reads.
        client.write_all(b"hello").unwrap();
        client.flush().unwrap();

        let mut buf = [0u8; 16];
        transport
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let n = transport.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello");

        // Transport writes, client reads.
        transport.write_all(b"world").unwrap();
        transport.flush().unwrap();

        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut buf2 = [0u8; 16];
        let n2 = client.read(&mut buf2).unwrap();
        assert_eq!(&buf2[..n2], b"world");

        assert!(!transport.is_tls());
    }

    #[test]
    fn test_plaintext_transport_peer_addr() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let _client = std::net::TcpStream::connect(addr).unwrap();
        let (server_stream, peer) = listener.accept().unwrap();

        let transport = PlaintextTransport::new(server_stream);
        assert_eq!(transport.peer_addr().unwrap(), peer);
    }

    #[test]
    fn test_plaintext_transport_shutdown() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let mut client = std::net::TcpStream::connect(addr).unwrap();
        let (server_stream, _) = listener.accept().unwrap();

        let mut transport = PlaintextTransport::new(server_stream);
        transport.write_all(b"before shutdown").unwrap();
        transport.flush().unwrap();
        transport.shutdown_write().unwrap();

        // Client should be able to read the data and then get EOF.
        let mut buf = Vec::new();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"before shutdown");
    }

    #[test]
    fn test_plaintext_acceptor() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        let acceptor = PlaintextAcceptor::new(listener);

        // No client connected yet — should return None (WouldBlock).
        assert!(
            acceptor
                .try_accept(Duration::from_secs(5))
                .unwrap()
                .is_none()
        );
        assert!(!acceptor.is_tls());

        // Connect a client.
        let _client = std::net::TcpStream::connect(addr).unwrap();
        // Give the OS a moment to propagate the connection.
        std::thread::sleep(Duration::from_millis(50));

        let accepted = acceptor
            .try_accept(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(!accepted.transport.is_tls());
        assert_eq!(accepted.peer_addr.ip(), std::net::Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn test_connection_config_defaults() {
        let config = ConnectionConfig::new(5000, 128, 0);
        assert_eq!(config.read_timeout, Duration::from_millis(5000));
        assert_eq!(config.poll_timeout, Duration::from_millis(10));
        assert_eq!(config.max_connections, 128);
        assert_eq!(config.max_requests, 0);
        assert_eq!(config.drain_timeout, Duration::from_millis(5000));
    }

    #[test]
    fn test_connection_config_custom() {
        let config = ConnectionConfig::new(10000, 64, 100);
        assert_eq!(config.read_timeout, Duration::from_millis(10000));
        assert_eq!(config.max_connections, 64);
        assert_eq!(config.max_requests, 100);
    }

    #[test]
    fn test_transport_connection_lifecycle() {
        // Verify TransportConnection can be constructed with a PlaintextTransport.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let _client = std::net::TcpStream::connect(addr).unwrap();
        let (server_stream, peer_addr) = listener.accept().unwrap();

        let tc = TransportConnection {
            transport: Box::new(PlaintextTransport::new(server_stream)),
            peer_addr,
            buf: vec![0u8; 8192],
            total_read: 0,
            conn_requests: 0,
            last_activity: std::time::Instant::now(),
        };

        assert_eq!(tc.peer_addr.ip(), std::net::Ipv4Addr::LOCALHOST);
        assert_eq!(tc.total_read, 0);
        assert_eq!(tc.conn_requests, 0);
        assert!(!tc.transport.is_tls());
    }

    // ── TLS tests (Phase 2) ────────────────────────────────────

    #[test]
    fn test_load_tls_config_missing_cert_file() {
        let result = load_tls_config("/nonexistent/cert.pem", "/nonexistent/key.pem");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("failed to open cert file"));
    }

    #[test]
    fn test_load_tls_config_missing_key_file() {
        // Create a temporary cert file but no key file.
        let dir = std::env::temp_dir().join("taida_tls_test_cert_only");
        let _ = std::fs::create_dir_all(&dir);
        let cert_path = dir.join("cert.pem");
        std::fs::write(&cert_path, "not a real cert").unwrap();
        let result = load_tls_config(cert_path.to_str().unwrap(), "/nonexistent/key.pem");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("failed to open key file") || msg.contains("no certificates"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_tls_config_invalid_cert_content() {
        let dir = std::env::temp_dir().join("taida_tls_test_invalid_cert");
        let _ = std::fs::create_dir_all(&dir);
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, "not a real cert").unwrap();
        std::fs::write(&key_path, "not a real key").unwrap();
        let result = load_tls_config(cert_path.to_str().unwrap(), key_path.to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err();
        // Should fail because no valid certificates found.
        assert!(
            msg.contains("no certificates")
                || msg.contains("no private key")
                || msg.contains("TLS config error"),
            "unexpected error: {}",
            msg
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_tls_config_empty_cert_file() {
        // Empty PEM file should fail with "no certificates".
        let dir = std::env::temp_dir().join("taida_tls_test_empty_cert");
        let _ = std::fs::create_dir_all(&dir);
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, "").unwrap();
        std::fs::write(&key_path, "").unwrap();
        let result = load_tls_config(cert_path.to_str().unwrap(), key_path.to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("no certificates"), "unexpected error: {}", msg);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── NB5-10 regression: TLS drain_tls_write ─────────────────

    /// Ensure the rustls default CryptoProvider is installed (idempotent).
    fn ensure_crypto_provider() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }

    /// Helper: generate a self-signed cert and return (cert_der, key_der)
    /// for building rustls ServerConfig / ClientConfig in tests.
    fn generate_test_cert() -> (
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ) {
        ensure_crypto_provider();
        let cert_params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = cert_params.self_signed(&key_pair).unwrap();
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_pair.serialize_der()).unwrap();
        (vec![cert_der], key_der)
    }

    #[test]
    fn test_tls_transport_large_write_drains_completely() {
        // NB5-10 regression: verify that write_all + flush on TlsTransport
        // drains all pending ciphertext even when the payload produces
        // multiple TLS records. Before the fix, a single write_tls call
        // could leave ciphertext in the rustls buffer, truncating the
        // response seen by the client.

        let (certs, key) = generate_test_cert();

        // Build server TLS config.
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs.clone(), key)
            .unwrap();
        let server_config = std::sync::Arc::new(server_config);

        // Build client TLS config (trust our self-signed cert).
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(certs[0].clone()).unwrap();
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let client_config = std::sync::Arc::new(client_config);

        // TCP listener on ephemeral port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // Large payload: 64 KiB of data (well above TLS max record size of ~16 KiB).
        // This forces rustls to produce multiple TLS records.
        let payload: Vec<u8> = (0..65536u32).map(|i| (i % 256) as u8).collect();
        let payload_clone = payload.clone();

        let server_handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();

            let tls_conn = rustls::ServerConnection::new(server_config.clone()).unwrap();
            let mut transport = TlsTransport::new(tls_conn, stream);

            // Complete TLS handshake.
            complete_tls_handshake(&mut transport, Duration::from_secs(5)).unwrap();

            // Write the large payload through the Transport trait.
            transport.write_all(&payload_clone).unwrap();
            transport.flush().unwrap();

            // Shutdown write side (sends close_notify).
            transport.shutdown_write().unwrap();
        });

        // Client: connect and read all data through TLS.
        let client_stream = std::net::TcpStream::connect(addr).unwrap();
        client_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client_stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls_client = rustls::ClientConnection::new(client_config, server_name).unwrap();
        let mut sock = client_stream;

        // Drive the TLS handshake on the client side.
        let mut stream = rustls::Stream::new(&mut tls_client, &mut sock);

        // Read all decrypted data.
        let mut received = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => panic!("client read error: {}", e),
            }
        }

        server_handle.join().unwrap();

        // The client must receive the full 64 KiB payload without truncation.
        assert_eq!(
            received.len(),
            payload.len(),
            "NB5-10: TLS payload truncated: got {} bytes, expected {}",
            received.len(),
            payload.len()
        );
        assert_eq!(received, payload, "NB5-10: TLS payload content mismatch");
    }

    #[test]
    fn test_tls_transport_shutdown_sends_close_notify() {
        // NB5-10 regression: verify that shutdown_write drains the
        // close_notify alert fully. The client should observe a clean
        // TLS closure (read returns 0) rather than a TCP RST / error.

        let (certs, key) = generate_test_cert();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs.clone(), key)
            .unwrap();
        let server_config = std::sync::Arc::new(server_config);

        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(certs[0].clone()).unwrap();
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let client_config = std::sync::Arc::new(client_config);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();

            let tls_conn = rustls::ServerConnection::new(server_config.clone()).unwrap();
            let mut transport = TlsTransport::new(tls_conn, stream);
            complete_tls_handshake(&mut transport, Duration::from_secs(5)).unwrap();

            // Write a small payload then shutdown.
            transport.write_all(b"goodbye").unwrap();
            transport.flush().unwrap();
            transport.shutdown_write().unwrap();
        });

        let client_stream = std::net::TcpStream::connect(addr).unwrap();
        client_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client_stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls_client = rustls::ClientConnection::new(client_config, server_name).unwrap();
        let mut sock = client_stream;
        let mut stream = rustls::Stream::new(&mut tls_client, &mut sock);

        let mut received = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // Clean TLS closure (close_notify received).
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => panic!("client read error (expected clean closure): {}", e),
            }
        }

        server_handle.join().unwrap();

        assert_eq!(
            &received, b"goodbye",
            "NB5-10: close_notify test: payload mismatch"
        );
    }
}
