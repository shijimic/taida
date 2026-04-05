/// H3 connection, stream state, idle timeout, graceful shutdown.
///
/// This module contains:
/// - H3StreamState, H3Stream — per-stream lifecycle
/// - H3ConnState — connection-level QUIC transport state
/// - H3HandlerContext — H3-specific handler context (NET7-7c)
/// - H3Connection — protocol state with idle timeout, GOAWAY, shutdown
/// - QPACK dynamic table and decoder/encoder instruction processing

use super::qpack::{
    H3DecodeError, H3DynamicTable, H3EncoderInstruction,
    H3DecoderInstruction, H3DecoderState,
    apply_encoder_instruction, decode_encoder_instruction,
    decode_decoder_instruction, H3Header,
};
use super::frame::{encode_goaway, H3_FRAME_GOAWAY, varint_decode, H3_MAX_STREAMS, H3_DEFAULT_MAX_FIELD_SECTION_SIZE};


// ── H3 Stream State Machine ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum H3StreamState {
    Idle,
    Open,
    HalfClosedLocal,
    Closed,
}

#[derive(Debug)]
pub(crate) struct H3Stream {
    pub stream_id: u64,
    pub state: H3StreamState,
    pub request_headers: Vec<H3Header>,
    pub request_body: Vec<u8>,
}

impl H3Stream {
    pub fn new(stream_id: u64) -> Self {
        H3Stream {
            stream_id,
            state: H3StreamState::Idle,
            request_headers: Vec::new(),
            request_body: Vec::new(),
        }
    }
}

// ── H3 Connection State ──────────────────────────────────────────────────

/// Connection-level QUIC transport state (NB7-20, NB7-26, NB7-28).
///
/// These fields track QUIC-specific state and are **separate** from the
/// existing 14-field handler contract. They are surfaced via `H3HandlerContext`
/// for H3-specific requests and do not affect h1/h2 handler compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum H3ConnState {
    /// No active QUIC stream; awaiting peer frames.
    Idle,
    /// One or more streams are active; peer frames being processed.
    Active,
    /// GOAWAY has been sent or received; new streams rejected, existing streams drain.
    Draining,
    /// All streams closed; connection ready for cleanup.
    Closed,
}

/// H3-specific handler context, decoupled from the standard 14-field request pack.
///
/// NB7-20: H3 固有フィールド (quic_connection_id, quic_stream_id)
/// NB7-26: 既存 14 field に影響を与えずに分離
///
/// This struct carries QUIC transport metadata that only applies to HTTP/3
/// requests. Non-QUIC backends (h1, h2) leave these fields empty/default,
/// preserving the 14-field handler contract.
#[derive(Debug, Clone)]
pub(crate) struct H3HandlerContext {
    /// QUIC Connection ID bytes (opaque, length depends on transport config).
    /// Empty for non-QUIC connections.
    pub quic_connection_id: Vec<u8>,
    /// QUIC stream ID used for multiplexed routing.
    /// Default 0 for non-QUIC connections.
    pub quic_stream_id: u64,
}

impl Default for H3HandlerContext {
    fn default() -> Self {
        H3HandlerContext {
            quic_connection_id: Vec::new(),
            quic_stream_id: 0,
        }
    }
}

/// HTTP/3 protocol state per connection.
///
/// **NB7-22 / NB7-28: Responsibility boundary.**
/// `H3Connection` manages HTTP/3 protocol state only:
/// - QPACK encode/decode state (static table only in Phase 2/3)
/// - Stream lifecycle (open/close)
/// - Settings (max_field_section_size)
/// - GOAWAY tracking
/// - Idle timeout (NET7-6c): H3-layer deadline tracking
/// - QUIC transport state (NET7-7c): connection ID, stream ID, lifecycle
///
/// QUIC transport state (draining, loss_detection, congestion_control)
/// is managed by `net_transport.rs` / QUIC substrate (libquiche).
#[derive(Debug)]
pub(crate) struct H3Connection {
    pub streams: Vec<H3Stream>,
    pub max_field_section_size: u64,
    pub last_peer_stream_id: u64,
    pub goaway_sent: bool,
    pub goaway_received: bool,
    pub goaway_id: u64,
    // NET7-6c (NB7-22): idle timeout deadline implemented in Phase 6+.
    // Set on init, checked during polling, refreshed on peer activity.
    pub idle_timeout_at: std::time::Instant,
    // NET7-7c (NB7-20, NB7-26, NB7-28): QUIC transport state integration.
    // Connection-level QUIC Connection ID (bytes). Empty for non-QUIC.
    pub quic_connection_id: Vec<u8>,
    // Current QUIC stream being processed (0 if none / non-QUIC).
    pub current_quic_stream_id: u64,
    // Connection lifecycle state (Idle -> Active -> Draining -> Closed)
    pub state: H3ConnState,
    // H3-specific handler context for the current request
    pub handler_ctx: H3HandlerContext,
}

impl H3Connection {
    /// Create a new connection with the default idle timeout (30 seconds).
    pub fn new() -> Self {
        // NB7-22, NB7-23: Error scope comment — H3 protocol errors are stream errors
        // (H3_ERR_REQUEST_INCOMPLETE/400 equivalent). Connection errors
        // (H3_ERR_GENERAL_PROTOCOL_ERROR) apply only to framing violations. NB7-23
        H3Connection {
            streams: Vec::new(),
            max_field_section_size: H3_DEFAULT_MAX_FIELD_SECTION_SIZE,
            last_peer_stream_id: 0,
            goaway_sent: false,
            goaway_received: false,
            goaway_id: 0,
            idle_timeout_at: Self::default_idle_deadline(),
            quic_connection_id: Vec::new(),
            current_quic_stream_id: 0,
            state: H3ConnState::Idle,
            handler_ctx: H3HandlerContext::default(),
        }
    }

    /// Default idle timeout for an H3 connection.
    /// Per HTTP/3 common practice, 30 seconds is a reasonable default.
    /// This matches common HTTP server defaults (nginx, Caddy, etc.).
    pub const DEFAULT_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    fn default_idle_deadline() -> std::time::Instant {
        std::time::Instant::now() + Self::DEFAULT_IDLE_TIMEOUT
    }

    /// Check whether the idle timeout has elapsed.
    ///
    /// Returns `Some(H3DecodeError::IdleTimeout)` if the idle timeout has fired.
    /// Per RFC 9114, idle timeout is a clean close: maps to `H3_ERR_NO_ERROR` (0x0100).
    /// Per RFC 9000 §10.1, idle timeout fires when no frames are received
    /// within the idle timeout period.
    pub fn check_timeout(&self) -> Option<H3DecodeError> {
        if std::time::Instant::now() > self.idle_timeout_at {
            Some(H3DecodeError::IdleTimeout)
        } else {
            None
        }
    }

    /// Reset the idle timeout deadline on peer activity.
    /// Called when a new frame is received from the peer.
    /// NET7-6c: this is the "touch" mechanism for the idle timer.
    pub fn reset_idle_timer(&mut self) {
        self.idle_timeout_at = Self::default_idle_deadline();
    }

    /// Set a custom idle timeout duration. Useful for testing.
    pub fn set_idle_timeout(&mut self, duration: std::time::Duration) {
        self.idle_timeout_at = std::time::Instant::now() + duration;
    }

    pub fn find_stream(&self, stream_id: u64) -> Option<&H3Stream> {
        self.streams.iter().rev().find(|s| s.stream_id == stream_id)
    }

    #[allow(dead_code)]
    pub fn find_stream_mut(&mut self, stream_id: u64) -> Option<&mut H3Stream> {
        self.streams.iter_mut().rev().find(|s| s.stream_id == stream_id)
    }

    pub fn new_stream(&mut self, stream_id: u64) -> Option<&mut H3Stream> {
        if self.streams.len() >= H3_MAX_STREAMS {
            return None;
        }
        let mut s = H3Stream::new(stream_id);
        s.state = H3StreamState::Open;
        self.streams.push(s);
        self.streams.last_mut()
    }

    pub fn remove_closed_streams(&mut self) {
        self.streams.retain(|s| s.state != H3StreamState::Closed);
    }

    // ── NET7-7c: QUIC Transport State Integration ─────────────────────
    // NET7-7a: Error scope boundary — these methods manage connection-level
    // state. Stream-level errors are scoped per-stream; connection-level
    // errors (state transitions, GOAWAY) apply to the entire connection.

    /// Set the QUIC Connection ID from the transport layer.
    /// The ID is opaque bytes whose length depends on the QUIC config.
    /// NB7-20, NB7-26: stored on H3Connection, NOT on the 14-field handler pack.
    pub fn set_quic_connection_id(&mut self, id: Vec<u8>) {
        self.quic_connection_id = id;
        self.handler_ctx.quic_connection_id = self.quic_connection_id.clone();
    }

    /// Return a reference to the QUIC Connection ID bytes.
    pub fn quic_connection_id(&self) -> &[u8] {
        &self.quic_connection_id
    }

    /// Set the current QUIC stream being processed.
    /// Updates both the connection field and the handler context.
    pub fn set_current_stream(&mut self, stream_id: u64) {
        self.current_quic_stream_id = stream_id;
        self.handler_ctx.quic_stream_id = stream_id;
    }

    /// Get the handler context for the current H3 request.
    /// Returns a clone so the caller can embed it in the 14-field pack
    /// without breaking the existing handler contract.
    pub fn handler_context(&self) -> H3HandlerContext {
        self.handler_ctx.clone()
    }

    /// Transition the connection state. Returns false if the transition
    /// is illegal for the current state.
    ///
    /// State machine: Idle -> Active -> Draining -> Closed
    /// Active -> Closed (emergency close, skip draining)
    pub fn transition_state(&mut self, target: H3ConnState) -> bool {
        let valid = matches!(
            (self.state.clone(), target.clone()),
            (H3ConnState::Idle, H3ConnState::Active)
                | (H3ConnState::Idle, H3ConnState::Closed)
                | (H3ConnState::Active, H3ConnState::Draining)
                | (H3ConnState::Active, H3ConnState::Closed)
                | (H3ConnState::Draining, H3ConnState::Closed)
        );
        if valid {
            self.state = target;
        }
        valid
    }

    /// Check if the connection is in a state that accepts new streams.
    /// Only Active connections accept new streams.
    pub fn accepts_new_streams(&self) -> bool {
        self.state == H3ConnState::Active
    }

    /// Begin graceful shutdown: send GOAWAY with the last peer stream ID,
    /// then transition to Draining state. Returns false if already shutting down.
    pub fn begin_shutdown(&mut self) -> bool {
        if self.state != H3ConnState::Active || self.goaway_sent {
            return false;
        }
        self.goaway_sent = true;
        self.transition_state(H3ConnState::Draining)
    }

    /// Receive a GOAWAY frame from the peer. Returns false if already received.
    /// Sets `goaway_received = true` and records `goaway_id` as the last
    /// stream ID the peer will process.
    pub fn receive_goaway(&mut self, last_stream_id: u64) -> bool {
        if self.goaway_received {
            return false;
        }
        self.goaway_received = true;
        if self.state == H3ConnState::Active {
            self.goaway_id = last_stream_id;
            self.transition_state(H3ConnState::Draining);
        }
        true
    }

    /// Complete shutdown: close all remaining streams and transition to Closed.
    /// Uses `transition_state()` to validate the state transition.
    /// NB7-52/53: Previously bypassed the state machine — now properly guarded.
    pub fn complete_shutdown(&mut self) -> bool {
        // Close all remaining streams
        for stream in self.streams.iter_mut() {
            stream.state = H3StreamState::Closed;
        }
        self.remove_closed_streams();
        // Validate transition: Draining -> Closed or Active -> Closed (emergency close)
        let ok = self.transition_state(H3ConnState::Closed);
        ok
    }

    /// Execute the full shutdown pipeline: GOAWAY -> drain -> close.
    ///
    /// 1. If Active, send GOAWAY and transition to Draining
    /// 2. If Draining, close all streams and transition to Closed
    /// 3. If Already Closed, no-op
    ///
    /// Each call advances the state machine by at most one step,
    /// allowing the caller to implement a proper drain wait between steps.
    ///
    /// Returns `(true, Some(Vec<Vec<u8>>))` with GOAWAY frame bytes if GOAWAY was sent.
    /// Returns `(true, None)` if state advanced without GOAWAY (Draining -> Closed).
    /// Returns `(false, None)` if no state change was possible (already Closed).
    pub fn shutdown(&mut self) -> (bool, Option<Vec<Vec<u8>>>) {
        match self.state {
            // Step 1: Active -> Draining, send GOAWAY if not yet sent
            H3ConnState::Active => {
                if self.goaway_sent {
                    // GOAWAY sent but still Active — advance to Draining
                    let ok = self.transition_state(H3ConnState::Draining);
                    if !ok { return (false, None); }
                    (true, None)
                } else {
                    let last_id = self.last_peer_stream_id;
                    self.goaway_sent = true;
                    let ok = self.transition_state(H3ConnState::Draining);
                    if !ok { return (false, None); }
                    let mut frames = Vec::new();
                    if let Some(frame) = encode_goaway(last_id) {
                        frames.push(frame);
                    }
                    (true, Some(frames))
                }
            }
            // Step 2: Draining -> Closed (caller should have waited for streams to complete)
            H3ConnState::Draining => {
                for stream in self.streams.iter_mut() {
                    stream.state = H3StreamState::Closed;
                }
                self.remove_closed_streams();
                let ok = self.transition_state(H3ConnState::Closed);
                if ok { (true, None) } else { (false, None) }
            }
            // Step 3: Already closed
            H3ConnState::Closed | H3ConnState::Idle => {
                (false, None)
            }
        }
    }

    /// Check if the connection is in the draining state.
    /// A draining connection has sent or received a GOAWAY and is
    /// waiting for existing streams to complete.
    pub fn is_draining(&self) -> bool {
        self.state == H3ConnState::Draining
    }

    /// Check if the connection is fully closed.
    pub fn is_closed(&self) -> bool {
        self.state == H3ConnState::Closed
    }
}
