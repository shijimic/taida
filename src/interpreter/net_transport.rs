/// Transport abstraction for `taida-lang/net` v5/v7.
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
///
/// # v7 QUIC Transport Contract (NET7-1a / NET7-1b)
///
/// v7 extends the transport layer with a QUIC substrate for HTTP/3.
/// The QUIC transport operates over UDP + TLS 1.3, distinct from the TCP-based
/// h1/h2 path. The transport contract is defined here; implementation will be
/// filled in Phase 2 (Native reference) and Phase 3 (Interpreter parity).
///
/// ## Transport Abstraction (NET7-1a) -- NB7-7 Design Decision
///
/// QUIC transport differs from TCP in several fundamental ways:
///   - **UDP-based**: No connection-oriented socket; uses `UdpSocket` for send/recv
///   - **TLS 1.3 mandatory**: Encryption is built into the QUIC handshake (not layered above)
///   - **Stream multiplexing**: A single QUIC connection carries multiple bidirectional streams
///   - **Connection-level vs stream-level I/O**: Read/write operate on streams, not on the connection
///
/// ### QUIC / TCP Transport Separation (NB7-7 resolution)
///
/// The existing `Transport` trait and `TransportAcceptor` / `TransportConnection` types
/// are designed around TCP semantics:
///   - `as_tcp_stream_mut()` returns `&mut TcpStream` (TCP-only)
///   - `TransportAcceptor::try_accept()` returns 1 connection per accept (no stream mux)
///   - `TransportConnection` bundles per-connection buffer and request count for a
///     single-stream-per-connection model
///
/// These cannot directly represent QUIC's connection-to-multi-stream relationship.
/// Rather than force-fitting QUIC into the TCP abstraction (which would require
/// breaking changes to h1/h2 code paths), v7 adopts a **separate QUIC transport
/// path**:
///
///   - The `Transport` trait, `TransportAcceptor`, and `TransportConnection` remain
///     **TCP-only** and continue to serve h1/h2 without modification.
///   - The h3 path (Phase 2 Native, Phase 3 Interpreter) will use a dedicated
///     QUIC accept/connection/stream model that does NOT implement `Transport`.
///   - The h3 handler dispatch will map QUIC streams to the existing
///     `httpServe` handler contract at the request/response level, not at the
///     transport I/O level.
///   - Interpreter parity (Phase 3) will mirror the Native QUIC path structure,
///     using the same dedicated types rather than wrapping `Transport`.
///
/// This separation is intentional: the handler contract is shared across h1/h2/h3,
/// but the transport layer beneath it is protocol-specific. Forcing a single
/// `Transport` trait to cover both TCP and QUIC would either break existing h1/h2
/// code or produce an abstraction that pappers over fundamental protocol differences.
///
/// Copy discipline (bounded-copy):
///   - 1 packet = at most 1 materialization (no aggregate buffer above packet boundary)
///   - Connection-local and stream-local buffers are reused
///   - No `true zero-copy` claim (QUIC includes AEAD encryption)
///
/// ## Lifecycle Contract (NET7-1b)
///
/// Stream lifecycle:
///   - Streams are created by the QUIC library on incoming request
///   - Each stream maps to one HTTP/3 request/response exchange
///   - Stream close is handled by the HTTP/3 layer (HEADERS + DATA + trailers)
///   - Half-close semantics: client can close send side while server writes response
///
/// Connection lifecycle:
///   - Connection established via QUIC handshake (includes TLS 1.3)
///   - Multiple streams share one connection
///   - Connection idle timeout follows existing `timeout` parameter from `httpServe`
///   - No QUIC-specific timeout knobs exposed to user-land (v7 design lock)
///
/// Shutdown lifecycle:
///   - Graceful: GOAWAY frame → drain in-flight streams → close connection
///   - Maps to existing `maxRequests` / server shutdown contract
///   - No user-facing QUIC drain period knob (v7 design lock)
///
/// Security boundaries (NET7-0g):
///   - 0-RTT: default-off, no user-facing opt-in in v7
///   - Resumption/stateless reset/connection migration: runtime-internal only
///   - None of these are exposed in the `Transport` trait or `httpServe` contract
///
/// ## Protocol Selection (NET7-1c)
///
/// The `protocol` field in the `tls` BuchiPack determines the transport:
///   - `"h1.1"` / `"http/1.1"` → TCP + optional TLS → HTTP/1.1
///   - `"h2"`                   → TCP + TLS (required) → HTTP/2
///   - `"h3"`                   → UDP + QUIC/TLS1.3 (required) → HTTP/3
///   - absent / `None`          → TCP + optional TLS → HTTP/1.1 (default)
///   - unknown value             → `ProtocolError` (immediate reject, no fallback)
///
/// Protocol selection happens at `httpServe` startup and is immutable for the
/// lifetime of the server. No mixed-protocol, no automatic Alt-Svc, no silent fallback.
use std::io;
use std::io::Read as _;
use std::net::SocketAddr;
use std::time::Duration;

// ── Protocol Selection (NET7-1c) ────────────────────────────────────

/// Protocol kind selected by the caller via `tls.protocol` field.
///
/// v7 NET7-1c: This enum formalizes the protocol selection contract.
/// It is determined once at `httpServe` startup and is immutable for the
/// lifetime of the server instance.
///
/// Variants:
///   - `H1` -- HTTP/1.1 (default, TCP, optionally over TLS)
///   - `H2` -- HTTP/2 (TCP + TLS required, ALPN negotiation)
///   - `H3` -- HTTP/3 (UDP + QUIC/TLS1.3, Native reference in Phase 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolKind {
    /// HTTP/1.1 over TCP (plaintext or TLS).
    H1,
    /// HTTP/2 over TCP + TLS. Requires cert/key. ALPN negotiation.
    H2,
    /// HTTP/3 over UDP + QUIC/TLS1.3. Requires cert/key.
    /// Phase 2: Native reference backend (QPACK, frames, stream state, handler mapping).
    /// Phase 3: Interpreter parity backend (unlock target).
    H3,
}

impl ProtocolKind {
    /// Parse the protocol string from the `tls.protocol` field.
    /// Returns `None` for unrecognized values (caller should reject with `ProtocolError`).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "h1.1" | "http/1.1" => Some(ProtocolKind::H1),
            "h2" => Some(ProtocolKind::H2),
            "h3" => Some(ProtocolKind::H3),
            _ => None,
        }
    }

    /// Whether this protocol requires TLS (cert + key).
    pub fn requires_tls(&self) -> bool {
        match self {
            ProtocolKind::H1 => false, // TLS is optional for h1
            ProtocolKind::H2 => true,  // h2c is out of scope
            ProtocolKind::H3 => true,  // QUIC mandates TLS 1.3
        }
    }
}

// ── Transport trait ──────────────────────────────────────────────────

/// Unified read/write interface for a single TCP connection (h1/h2 only).
///
/// Implementors: `PlaintextTransport` (v4 path), `TlsTransport` (v5 Phase 2).
///
/// NOTE (NB7-7): The h3/QUIC path does NOT implement this trait. QUIC streams
/// use a dedicated transport model introduced in Phase 2/3. See the module-level
/// NB7-7 design decision for rationale.
///
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
    ///
    /// NOTE (NB7-7): This method is TCP-only by design. The h3/QUIC transport path
    /// does NOT implement `Transport` and will never provide a `TcpStream`.
    /// See the module-level NB7-7 design decision for rationale.
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

/// Listener-level abstraction for accepting new TCP connections (h1/h2 only).
///
/// `PlaintextAcceptor` wraps `TcpListener` directly.
/// `TlsAcceptor` wraps `TcpListener` + `rustls::ServerConfig` and performs TLS handshake.
///
/// NOTE (NB7-7): This trait follows the TCP model of 1 accept = 1 connection.
/// QUIC's 1 connection = N streams model is handled by a separate h3-specific
/// acceptor in Phase 2/3. See the module-level NB7-7 design decision.
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

/// Connection state within the httpServe pool (TCP / h1/h2 only).
///
/// Replaces the previous `HttpConnection` struct to use transport abstraction.
/// Each connection owns its transport, buffer, and lifecycle metadata.
///
/// NOTE (NB7-7): This struct models a single TCP connection with one active
/// request stream. QUIC connections (h3) use a separate connection/stream model
/// in Phase 2/3. See the module-level NB7-7 design decision.
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

// ── QUIC Transport Contract (NET7-1a / NET7-1b) ────────────────────
//
// This section documents the QUIC transport abstraction that Phase 2 (Native)
// and Phase 3 (Interpreter) will implement. No actual QUIC library dependency
// is added in Phase 1 -- only the contract is established.
//
// IMPORTANT (NB7-7): The QUIC transport does NOT implement the `Transport`,
// `TransportAcceptor`, or `TransportConnection` traits defined above. Those
// are TCP-only and remain unchanged for h1/h2. The h3 path uses a dedicated
// QUIC accept/connection/stream model. See the module-level NB7-7 design
// decision and `.dev/NET_DESIGN.md` "QUIC / TCP Transport Separation" for
// full rationale.
//
// ## QuicStreamTransport (NET7-1a)
//
// Dedicated QUIC stream transport (does NOT implement the `Transport` trait).
// Each QuicStreamTransport wraps a single bidirectional QUIC stream and
// maps read/write/shutdown to QUIC stream operations.
//
// I/O semantics (parallel to TCP transports but on a separate type hierarchy):
//   - `read`: reads from the QUIC stream's receive buffer (decrypted by QUIC)
//   - `write_all`: writes to the QUIC stream's send buffer (encrypted by QUIC)
//   - `flush`: triggers QUIC packet assembly and UDP send
//   - `shutdown_write`: sends STREAM FIN on this stream
//   - Timeouts: per-stream deadlines map to QUIC idle timeout internally
//   - TLS is always active (QUIC mandates TLS 1.3)
//   - `peer_addr`: returns the UDP peer address of the QUIC connection
//
// ## QuicAcceptor (NET7-1a)
//
// Dedicated QUIC acceptor (does NOT implement the `TransportAcceptor` trait).
// Listens on a `UdpSocket`, manages QUIC connection state, and yields
// incoming streams as `QuicStreamTransport` instances.
//
// Key differences from TCP-based acceptors:
//   - Binds a `UdpSocket` instead of a `TcpListener`
//   - QUIC handshake (including TLS 1.3) happens as part of connection setup
//   - A single accepted QUIC connection can yield multiple streams
//   - Stream acceptance is per-stream, not per-connection (unlike TCP's
//     `try_accept` which returns one connection per call)
//   - TLS is always active
//
// ## Stream Lifecycle (NET7-1b)
//
// 1. Client initiates QUIC connection (Initial packet)
// 2. Server performs QUIC handshake (includes TLS 1.3 handshake)
// 3. Client opens a bidirectional stream and sends HTTP/3 HEADERS frame
// 4. Server receives stream, maps to handler via existing request dispatch
// 5. Handler returns response; server sends HEADERS + DATA on the stream
// 6. Stream is closed (FIN sent in both directions)
// 7. Connection persists for additional streams until idle timeout or GOAWAY
//
// ## Connection Lifecycle (NET7-1b)
//
// - Established: QUIC handshake complete, TLS 1.3 keys derived
// - Active: streams being created/served
// - Draining: GOAWAY sent, no new streams, in-flight streams complete
// - Closed: all streams done, connection resources released
//
// ## Shutdown Contract (NET7-1b)
//
// Graceful shutdown (triggered by maxRequests or external signal):
// 1. Stop accepting new QUIC connections
// 2. Send GOAWAY on all active connections
// 3. Wait for in-flight streams to complete (bounded by timeout)
// 4. Close all connections
//
// This maps to the existing shutdown contract from httpServe.
// No QUIC-specific drain knobs are exposed to user-land in v7.
//
// ## Timeout Contract (NET7-1b)
//
// - The `timeout` parameter from httpServe is used as the per-connection
//   idle timeout for QUIC connections
// - No QUIC-specific timeout knobs are added to the user-facing API in v7
// - QUIC idle timeout, max idle timeout, and keep-alive are runtime-internal
//
// ## Security Boundaries (NET7-0g, referenced from NET7-1a/1b)
//
// - 0-RTT: default-off, not exposed in Phase 1
// - Resumption tickets: runtime-internal, not exposed
// - Stateless reset: runtime-internal, not exposed
// - Connection migration: runtime-internal, not exposed

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    // ── NET7-1c: ProtocolKind unit tests ────────────────────────────

    #[test]
    fn test_protocol_kind_from_str_valid() {
        assert_eq!(ProtocolKind::from_str("h1.1"), Some(ProtocolKind::H1));
        assert_eq!(ProtocolKind::from_str("http/1.1"), Some(ProtocolKind::H1));
        assert_eq!(ProtocolKind::from_str("h2"), Some(ProtocolKind::H2));
        assert_eq!(ProtocolKind::from_str("h3"), Some(ProtocolKind::H3));
    }

    #[test]
    fn test_protocol_kind_from_str_unknown() {
        assert_eq!(ProtocolKind::from_str("h4"), None);
        assert_eq!(ProtocolKind::from_str("quic"), None);
        assert_eq!(ProtocolKind::from_str(""), None);
        assert_eq!(ProtocolKind::from_str("HTTP/2"), None); // case-sensitive
        assert_eq!(ProtocolKind::from_str("H3"), None); // case-sensitive
    }

    #[test]
    fn test_protocol_kind_requires_tls() {
        assert!(!ProtocolKind::H1.requires_tls()); // h1 TLS is optional
        assert!(ProtocolKind::H2.requires_tls()); // h2 requires TLS (h2c out of scope)
        assert!(ProtocolKind::H3.requires_tls()); // h3 mandates TLS 1.3 via QUIC
    }

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

    // ── NET7-2a: Phase 2 ProtocolKind contract tests ──────────────────

    #[test]
    fn test_protocol_kind_h3_properties() {
        // H3 requires TLS (QUIC mandates TLS 1.3)
        assert!(ProtocolKind::H3.requires_tls());
        // H3 is distinct from H1 and H2
        assert_ne!(ProtocolKind::H3, ProtocolKind::H1);
        assert_ne!(ProtocolKind::H3, ProtocolKind::H2);
        // H3 is parsed from "h3" only (case-sensitive)
        assert_eq!(ProtocolKind::from_str("h3"), Some(ProtocolKind::H3));
        assert_eq!(ProtocolKind::from_str("H3"), None);
        assert_eq!(ProtocolKind::from_str("http/3"), None);
        assert_eq!(ProtocolKind::from_str("h3.0"), None);
    }

    #[test]
    fn test_protocol_kind_copy_clone() {
        // ProtocolKind is Copy + Clone (used in immutable server config)
        let pk = ProtocolKind::H3;
        let pk2 = pk; // Copy
        let pk3 = pk.clone(); // Clone
        assert_eq!(pk, pk2);
        assert_eq!(pk, pk3);
    }

    #[test]
    fn test_protocol_kind_debug() {
        // ProtocolKind implements Debug (for error messages)
        let dbg = format!("{:?}", ProtocolKind::H3);
        assert!(dbg.contains("H3"), "Debug should contain H3: {}", dbg);
    }
}
