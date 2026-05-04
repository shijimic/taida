/// Type definitions for net_eval (C12B-025 mechanical split).
///
/// This file contains all type definitions extracted from net_eval.rs:
///   - Writer state machine types (WriterState, StreamingWriter, ActiveStreamingWriter)
///   - Body framing types (BodyEncoding, RequestBodyState, ChunkedDecoderState)
///   - WebSocket frame types (WsFrame)
///   - Connection stream types (ConnStream, HttpConnection, ConnReadResult, ConnAction)
///   - Internal helper types (ChunkedCompactResult, ChunkedBodyError, ResponseFields)
///   - Global atomic counters (NEXT_REQUEST_TOKEN, NEXT_WS_TOKEN)
use super::super::value::Value;
use crate::net_surface::NET_RUNTIME_BUILTIN_NAMES;

/// (status_code, headers, body_bytes)
pub(super) type ResponseFields = (i64, Vec<(String, String)>, Vec<u8>);

pub(crate) const STREAMING_HEADER_NAME_MAX_BYTES: usize = 8192;
pub(crate) const STREAMING_HEADER_VALUE_MAX_BYTES: usize = 65536;

/// All symbols exported by the net package.
/// HTTP v1 (3) + HTTP v2 (1) + HTTP v3 (4) + HTTP v4 (6) + v5 (1) = 15 symbols.
pub(crate) const NET_SYMBOLS: &[&str] = &NET_RUNTIME_BUILTIN_NAMES;

//
// State transitions (see NET_DESIGN.md):
//   Idle → HeadPrepared (via startResponse)
//   Idle → Streaming (via writeChunk/sseEvent, implicit 200/@[])
//   Idle → Ended (via endResponse, implicit 200/@[], empty chunked body)
//   Idle → One-shot fallback (2-arg handler returns response pack)
//   HeadPrepared → Streaming (via writeChunk/sseEvent)
//   HeadPrepared → Ended (via endResponse, empty chunked body)
//   Streaming → Streaming (via writeChunk/sseEvent)
//   Streaming → Ended (via endResponse)

/// Writer state for response streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterState {
    /// No streaming operation started yet.
    Idle,
    /// `startResponse` called; pending status/headers set but not committed to wire.
    HeadPrepared,
    /// Head committed to wire; body chunks can be written.
    Streaming,
    /// `endResponse` called or auto-ended; no more writes allowed.
    Ended,
    /// v4: WebSocket upgrade completed. HTTP streaming API is disabled.
    /// Only wsSend/wsReceive/wsClose are valid in this state.
    WebSocket,
}

/// Streaming writer state for a 2-arg handler.
/// The handler receives a Value::BuchiPack with a `__writer_id` sentinel field,
/// but the actual mutable state lives here on the dispatch_request stack.
/// During handler execution, the Interpreter's `active_streaming_writer` field
/// holds raw pointers to this struct and the connection's TcpStream.
pub(crate) struct StreamingWriter {
    pub state: WriterState,
    pub pending_status: u16,
    /// Response headers staged for head commit.
    /// Intentionally retained after commit so later logic can validate which
    /// headers were already put on the wire (for example, SSE checks after an
    /// earlier writeChunk committed the head).
    pub pending_headers: Vec<(String, String)>,
    /// Whether SSE auto-headers have been applied.
    pub sse_mode: bool,
}

impl StreamingWriter {
    pub(crate) fn new() -> Self {
        StreamingWriter {
            state: WriterState::Idle,
            pending_status: 200,
            pending_headers: Vec::new(),
            sse_mode: false,
        }
    }

    /// Check if a status code forbids a message body (1xx, 204, 205, 304).
    pub(crate) fn is_bodyless_status(status: u16) -> bool {
        matches!(status, 100..=199 | 204 | 205 | 304)
    }

    /// Validate that user-supplied headers are safe for the streaming path.
    pub(crate) fn validate_reserved_headers(headers: &[(String, String)]) -> Result<(), String> {
        for (i, (name, value)) in headers.iter().enumerate() {
            if name.len() > STREAMING_HEADER_NAME_MAX_BYTES {
                return Err(format!(
                    "startResponse: headers[{}].name exceeds {} bytes",
                    i, STREAMING_HEADER_NAME_MAX_BYTES
                ));
            }
            if value.len() > STREAMING_HEADER_VALUE_MAX_BYTES {
                return Err(format!(
                    "startResponse: headers[{}].value exceeds {} bytes",
                    i, STREAMING_HEADER_VALUE_MAX_BYTES
                ));
            }
            if name.contains('\r') || name.contains('\n') {
                return Err(format!("startResponse: headers[{}].name contains CR/LF", i));
            }
            if value.contains('\r') || value.contains('\n') {
                return Err(format!(
                    "startResponse: headers[{}].value contains CR/LF",
                    i
                ));
            }

            let lower = name.to_ascii_lowercase();
            if lower == "content-length" {
                return Err(
                    "startResponse: 'Content-Length' is not allowed in streaming response headers. \
                     The runtime manages Content-Length/Transfer-Encoding for streaming responses."
                        .to_string(),
                );
            }
            if lower == "transfer-encoding" {
                return Err(
                    "startResponse: 'Transfer-Encoding' is not allowed in streaming response headers. \
                     The runtime manages Transfer-Encoding for streaming responses."
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}

/// Active streaming writer context, stored on the Interpreter during 2-arg handler execution.
/// Holds raw pointers to stack-local variables in `dispatch_request`.
///
/// Safety invariants:
/// - The Interpreter is single-threaded (!Send).
/// - The pointers are set before `call_function_with_values` and cleared after it returns.
/// - The pointees (StreamingWriter, TcpStream) live on the stack in `dispatch_request`
///   and outlive the handler call.
/// - NB3-9: `borrowed` flag prevents re-entrant access to the raw pointers,
///   eliminating the theoretical UB from nested streaming API calls (e.g.
///   `writeChunk(writer, writeChunk(writer, "data"))`).
pub(crate) struct ActiveStreamingWriter {
    pub writer: *mut StreamingWriter,
    pub stream: *mut ConnStream,
    /// NB3-9: Re-entrancy guard. Set to true while a streaming API function
    /// holds `&mut *self.writer` / `&mut *self.stream`. If another streaming
    /// API call is attempted while this is true, it returns a RuntimeError
    /// instead of creating a second `&mut` to the same pointee.
    pub borrowed: bool,
    /// v4: Raw pointer to the body streaming state for this request.
    /// Only set for 2-arg handlers (body-deferred mode).
    /// Null when 1-arg handler (body already read eagerly).
    pub body_state: *mut RequestBodyState,
    /// v4: WebSocket close state. True after wsClose has been sent.
    pub ws_closed: bool,
    /// NB4-10: Connection-scoped WebSocket token for identity verification.
    /// Set when wsUpgrade succeeds; verified by wsSend/wsReceive/wsClose.
    pub ws_token: u64,
    /// v5: Received close code from peer's close frame.
    /// 0 = no close frame received yet.
    /// Set when a valid close frame is received in wsReceive.
    pub ws_close_code: i64,
}

// ── C12-12 / FB-2: Body encoding internal representation ─────
//
// `BodyEncoding` names the three ways an HTTP/1.1 request can frame
// its body. It is derived from the parsed headers at handshake time
// and consumed by the runtime (read path, keep-alive advance,
// streaming state transitions).
//
// Intentionally *not* exposed in the handler-visible Taida
// `Value::BuchiPack`. The Taida surface continues to expose only
// the flattened `contentLength: Int` and `chunked: Bool` pair for v1
// compatibility. The `BodyEncoding` enum is the single source of
// truth that the internal wiring promotes to when v2 chunked /
// trailers / HTTP/2 DATA frames gain first-class support — see
// `.dev/taida-logs/docs/design/net_v2_chunked.md` for the upgrade
// path.
//
// Representation rules (matches RFC 7230 § 3.3.3):
//   - `Transfer-Encoding: chunked` present (alone) → `Chunked`
//   - `Content-Length: N` with N > 0 and no TE:chunked → `ContentLength(N)`
//   - `Content-Length: 0` and no TE:chunked → `Empty { had_content_length_header: true }`
//   - neither header present → `Empty { had_content_length_header: false }`
//   - TE:chunked + any `Content-Length` → rejected at parse time
//     (predates `BodyEncoding` construction), so this enum never has
//     to represent the invalid combination.
//
// C12B-032 refinement: the `Empty` variant now carries a
// `had_content_length_header` bit so the internal layer can
// distinguish between "the client sent `Content-Length: 0`" and
// "the client sent no Content-Length / Transfer-Encoding at all".
// Both cases still produce identical behaviour in the runtime read
// loop, but downstream consumers (v2 chunked trailers, HTTP/2 DATA
// frame framing, TE:identity negotiation) need the distinction to
// implement RFC 7230 § 3.3.3 rule 6 correctly. The handler-visible
// `contentLength: 0` / `chunked: false` flattening is preserved for
// v1 compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyEncoding {
    /// No body bytes on the wire. `had_content_length_header` records
    /// whether the client explicitly sent `Content-Length: 0` (true) or
    /// omitted Content-Length entirely (false). The two cases produce
    /// the same read-loop behaviour but differ in RFC 7230 § 3.3.3
    /// framing semantics (trailers / body-less response encoding).
    Empty { had_content_length_header: bool },
    /// Fixed-length body. The inner value is the non-zero byte count
    /// declared in the Content-Length header and bounds the read loop.
    ContentLength(u64),
    /// `Transfer-Encoding: chunked`. Byte count is only known after
    /// the terminal chunk is seen.
    Chunked,
}

impl BodyEncoding {
    /// Classify a request body from the three signals extracted by
    /// `parse_request_head`. Contract: `has_chunked` and
    /// `has_content_length` are never both true (the parser rejects
    /// that combination before calling us). `content_length_val` is
    /// only meaningful when `has_content_length` is true.
    pub(crate) fn classify(
        has_chunked: bool,
        has_content_length: bool,
        content_length_val: i64,
    ) -> Self {
        if has_chunked {
            BodyEncoding::Chunked
        } else if has_content_length && content_length_val > 0 {
            // content_length_val is validated non-negative and <= 2^53-1
            // at parse time, so this cast is always safe.
            BodyEncoding::ContentLength(content_length_val as u64)
        } else {
            // Either `Content-Length: 0` or the header is absent — the
            // wire behaviour is identical (empty body) but we record
            // which case we are in so RFC 7230 framing can be
            // reconstructed without re-parsing the raw request.
            BodyEncoding::Empty {
                had_content_length_header: has_content_length,
            }
        }
    }

    /// Derive `BodyEncoding` from the Result BuchiPack returned by
    /// `parse_request_head`. Used by unit tests to confirm that the
    /// handler-visible fields agree with the internal representation.
    ///
    /// Note: the Taida surface flattens "header absent" and
    /// `Content-Length: 0` into a single `contentLength: 0` field, so
    /// this reverse path cannot distinguish them and conservatively
    /// assumes `had_content_length_header = false`. Callers that need
    /// the true presence bit must construct the enum via
    /// `BodyEncoding::classify` directly with the parser's signals.
    #[cfg(test)]
    pub(crate) fn from_parsed_result_value(parsed: &Value) -> Option<Self> {
        let inner = super::helpers::extract_result_value(parsed)?;
        let chunked = super::helpers::get_field_bool(inner, "chunked")?;
        let cl = super::helpers::get_field_int(inner, "contentLength")?;
        Some(BodyEncoding::classify(chunked, cl > 0, cl))
    }

    /// Convenience accessor for the fixed body length. Returns `None`
    /// for `Chunked` (length unknown) and `Empty` (caller does not
    /// need to read the stream).
    #[allow(dead_code)]
    pub(crate) fn fixed_length(&self) -> Option<u64> {
        match self {
            BodyEncoding::ContentLength(n) => Some(*n),
            BodyEncoding::Chunked | BodyEncoding::Empty { .. } => None,
        }
    }

    /// `true` if the body is known empty at parse time (no wire bytes
    /// to drain before the next request on a keep-alive connection).
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, BodyEncoding::Empty { .. })
    }

    /// C12B-032: `true` when the `Empty` classification came from an
    /// explicit `Content-Length: 0` header (as opposed to the header
    /// being absent). Only meaningful for the `Empty` variant — the
    /// other two variants return `false`. Used by the HTTP/2 / v2
    /// chunked promotion path to know whether to re-emit a
    /// `Content-Length: 0` framing header or leave it off.
    #[allow(dead_code)]
    pub(crate) fn had_content_length_header(&self) -> bool {
        matches!(
            self,
            BodyEncoding::Empty {
                had_content_length_header: true,
            } | BodyEncoding::ContentLength(_)
        )
    }
}

// ── v4 Request Body Streaming State ──────────────────────────

/// Per-request body streaming state for 2-arg handlers.
/// Lives on the `dispatch_request` stack frame. The `ActiveStreamingWriter`
/// holds a raw pointer to this struct during handler execution.
///
/// Tracks how much of the request body has been consumed so that
/// `readBodyChunk` can read incrementally from the TcpStream without
/// buffering the full body.
pub(crate) struct RequestBodyState {
    /// C12-12 / FB-2: canonical internal body framing.
    /// Derived from request headers at parse time and used by the
    /// runtime read loop. The sibling `is_chunked` / `content_length`
    /// fields remain on the struct so existing consumers across
    /// `net_eval.rs` (keep-alive advance, body read loop, streaming
    /// transitions) keep compiling without a mechanical rename — they
    /// are redundant projections of `body_encoding`.
    pub body_encoding: BodyEncoding,
    /// Whether the request uses chunked transfer encoding.
    pub is_chunked: bool,
    /// Content-Length value from the request head (0 if absent or chunked).
    pub content_length: i64,
    /// How many body bytes have been consumed so far (Content-Length path).
    pub bytes_consumed: i64,
    /// Whether the body has been fully read (terminal chunk seen or all CL bytes read).
    pub fully_read: bool,
    /// Whether any readBodyChunk / readBodyAll call has been made.
    pub any_read_started: bool,
    /// Leftover bytes from head parsing that belong to the body start.
    /// For 2-arg handler, after head parse, there may be unread bytes in
    /// conn.buf that are body bytes already received. We copy these out
    /// so that the first readBodyChunk can return them without re-reading.
    pub leftover: Vec<u8>,
    /// Current position within the leftover buffer.
    pub leftover_pos: usize,
    /// Chunked decoder state: pending partial chunk header bytes.
    pub chunked_state: ChunkedDecoderState,
    /// NB4-7: Request-scoped token to verify that readBody*/readBodyChunk/readBodyAll
    /// are called with the correct request pack (not a fake or stale pack).
    pub request_token: u64,
}

/// Global monotonic counter for generating unique request tokens (NB4-7).
pub(crate) static NEXT_REQUEST_TOKEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

/// NB4-10: Global monotonic counter for generating unique WebSocket connection tokens.
pub(crate) static NEXT_WS_TOKEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

// ── v4 WebSocket frame types ───────────────────────────────

/// Parsed WebSocket frame result.
pub(crate) enum WsFrame {
    /// Text (opcode 0x1) or Binary (opcode 0x2) data frame.
    Data { opcode: u8, payload: Vec<u8> },
    /// Ping frame (opcode 0x9) with payload for pong echo.
    Ping { payload: Vec<u8> },
    /// Pong frame (opcode 0xA) — unsolicited, ignored.
    Pong,
    /// Close frame (opcode 0x8).
    /// v5: carries the raw close payload for close code extraction.
    Close { payload: Vec<u8> },
    /// Protocol error (fragmented frame, unknown opcode, etc.).
    ProtocolError(String),
}

/// Chunked transfer-encoding decoder state.
/// Maintains state between readBodyChunk calls for incremental decoding.
#[derive(Debug)]
pub(crate) enum ChunkedDecoderState {
    /// Waiting for chunk-size line (hex digits + CRLF).
    WaitingChunkSize,
    /// In the middle of reading chunk data.
    /// `remaining` is how many bytes left in the current chunk.
    ReadingChunkData { remaining: usize },
    /// Waiting for CRLF after chunk data.
    WaitingChunkTrailer,
    /// Terminal chunk (size=0) has been seen. Body is done.
    Done,
}

impl RequestBodyState {
    /// Construct a `RequestBodyState` from parser signals.
    ///
    /// C12B-032 / FB-2: `had_content_length_header` distinguishes
    /// explicit `Content-Length: 0` (true) from an absent
    /// Content-Length header (false). Both produce identical wire
    /// behaviour on HTTP/1.1 (empty body) but the bit is preserved in
    /// `BodyEncoding::Empty { had_content_length_header }` so the v2
    /// chunked-trailers / HTTP/2 DATA promotion path can reconstruct
    /// the correct framing without re-parsing the raw request.
    ///
    /// Legacy callers that did not pre-C12B-032 thread the presence
    /// bit can use `RequestBodyState::new_legacy` below, which
    /// conservatively infers `had_content_length_header` from the
    /// numeric `content_length > 0` signal (the backward-compatible
    /// reading of RFC 7230 § 3.3.3 rule 6 as of @c.12.rc3).
    pub(crate) fn new(
        is_chunked: bool,
        content_length: i64,
        had_content_length_header: bool,
        leftover: Vec<u8>,
    ) -> Self {
        let token = NEXT_REQUEST_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // C12-12 / FB-2 / C12B-032: derive the internal encoding once
        // at state construction. `had_content_length_header` now comes
        // from the parser directly so the Empty variant preserves the
        // presence bit per RFC 7230 § 3.3.3. `fully_read` is derived
        // from `body_encoding` so the two stay in lock-step.
        let body_encoding =
            BodyEncoding::classify(is_chunked, had_content_length_header, content_length);
        let fully_read = body_encoding.is_empty();
        RequestBodyState {
            body_encoding,
            is_chunked,
            content_length,
            bytes_consumed: 0,
            fully_read,
            any_read_started: false,
            leftover,
            leftover_pos: 0,
            chunked_state: ChunkedDecoderState::WaitingChunkSize,
            request_token: token,
        }
    }

    /// C12B-032 / FB-2: Legacy 3-arg constructor retained for unit
    /// tests and legacy callers that do not yet thread the
    /// `had_content_length_header` presence bit. Conservative
    /// behaviour: infer the bit from `content_length > 0`, which
    /// matches the pre-C12B-032 classification.
    #[cfg(test)]
    pub(crate) fn new_legacy(is_chunked: bool, content_length: i64, leftover: Vec<u8>) -> Self {
        RequestBodyState::new(is_chunked, content_length, content_length > 0, leftover)
    }

    /// Check if there are leftover bytes available.
    pub(crate) fn has_leftover(&self) -> bool {
        self.leftover_pos < self.leftover.len()
    }

    /// Take remaining leftover bytes (consuming them).
    #[allow(dead_code)]
    pub(crate) fn take_leftover(&mut self) -> Vec<u8> {
        if self.leftover_pos >= self.leftover.len() {
            return Vec::new();
        }
        let data = self.leftover[self.leftover_pos..].to_vec();
        self.leftover_pos = self.leftover.len();
        data
    }
}

// ── NET2-3: Concurrent connection pool types ────────────────

// ── ConnStream: polymorphic stream for TLS / plaintext ──────────────

/// Polymorphic stream that wraps either a plain TcpStream (v4 compat)
/// or a TlsTransport (v5 HTTPS). Implements `std::io::Read` and `std::io::Write`
/// so existing streaming helpers work unchanged.
pub(crate) enum ConnStream {
    Plain(std::net::TcpStream),
    Tls(Box<super::super::net_transport::TlsTransport>),
}

impl std::io::Read for ConnStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            ConnStream::Plain(s) => std::io::Read::read(s, buf),
            ConnStream::Tls(t) => super::super::net_transport::Transport::read(t.as_mut(), buf),
        }
    }
}

impl std::io::Write for ConnStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            ConnStream::Plain(s) => std::io::Write::write(s, buf),
            ConnStream::Tls(t) => {
                super::super::net_transport::Transport::write_all(t.as_mut(), buf)?;
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => std::io::Write::flush(s),
            ConnStream::Tls(t) => super::super::net_transport::Transport::flush(t.as_mut()),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => std::io::Write::write_all(s, buf),
            ConnStream::Tls(t) => {
                super::super::net_transport::Transport::write_all(t.as_mut(), buf)
            }
        }
    }
}

impl ConnStream {
    pub(crate) fn set_read_timeout(&self, dur: Option<std::time::Duration>) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => s.set_read_timeout(dur),
            ConnStream::Tls(t) => t.stream_ref().set_read_timeout(dur),
        }
    }

    /// Graceful write-side shutdown.
    ///
    /// For TLS: send `close_notify`, flush all buffered ciphertext, then shutdown
    /// the TCP write half. For plaintext: shutdown TCP write half directly.
    ///
    /// C12B-028: called on H2 `max_requests`-bounded exit so the kernel sends a
    /// clean FIN instead of an RST when the process terminates with pending
    /// response bytes still in-flight on the socket.
    pub(crate) fn shutdown_write_graceful(&mut self) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => s.shutdown(std::net::Shutdown::Write),
            ConnStream::Tls(t) => {
                super::super::net_transport::Transport::shutdown_write(t.as_mut())
            }
        }
    }

    /// Read-until-EOF on the socket with a short timeout, so the peer has a
    /// chance to observe our FIN / `close_notify` and to send its own before
    /// we close. Keeps at most `max_bytes` worth of post-shutdown traffic out
    /// of the kernel backlog. Any error (timeout, EOF, reset) is swallowed —
    /// this is a best-effort linger.
    pub(crate) fn drain_after_shutdown(&mut self, max_bytes: usize) {
        // Use a short explicit timeout so we do not block indefinitely if the
        // peer never closes.
        let _ = self.set_read_timeout(Some(std::time::Duration::from_millis(200)));
        let mut scratch = [0u8; 1024];
        let mut drained = 0;
        while drained < max_bytes {
            match std::io::Read::read(self, &mut scratch) {
                Ok(0) => break,        // clean EOF
                Ok(n) => drained += n, // keep draining
                Err(_) => break,       // timeout or error → done
            }
        }
    }
}

/// Per-connection state for the concurrent httpServe pool.
/// Each connection owns its own scratch buffer (no sharing).
pub(crate) struct HttpConnection {
    pub(crate) stream: ConnStream,
    pub(crate) peer_addr: std::net::SocketAddr,
    /// Per-connection scratch buffer (allocated once, reused via advance)
    pub(crate) buf: Vec<u8>,
    /// How many bytes are valid in buf
    pub(crate) total_read: usize,
    /// How many requests have been processed on this connection
    pub(crate) conn_requests: i64,
    /// Last activity timestamp (for idle timeout detection)
    pub(crate) last_activity: std::time::Instant,
}

/// Result of a non-blocking read attempt on a connection.
pub(crate) enum ConnReadResult {
    /// Complete request head parsed: (fields, head_consumed, content_length, is_chunked, had_content_length_header).
    /// C12B-032 / FB-2: `had_content_length_header` distinguishes the two
    /// "empty body" sub-cases (explicit `Content-Length: 0` vs. absent
    /// header) so the internal `BodyEncoding` can preserve the bit.
    Ready(Vec<(String, Value)>, usize, i64, bool, bool),
    /// Need more data (no complete head yet, not an error)
    NeedMore,
    /// Client closed the connection (EOF)
    Eof,
    /// Read timed out (short poll timeout, not necessarily idle timeout)
    Timeout,
    /// Malformed request head
    Malformed,
}

/// Action to take after dispatching a request on a connection.
pub(crate) enum ConnAction {
    /// Keep connection alive for more requests
    KeepAlive,
    /// Close the connection
    Close,
}
