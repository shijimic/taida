//! Interpreter-side networking, organised by layer:
//!
//! - [`eval`] — the `taida-lang/net` builtin dispatch surface
//!   (httpServe / readBody / ws* / sse* evaluation entry points per
//!   protocol, plus shared header/body helpers and types)
//! - [`h2`] — HTTP/2 protocol engine (HPACK, frames, server loop)
//! - [`h3`] — HTTP/3 protocol engine (QPACK, frames, QUIC plumbing)
//! - [`transport`] — TCP/TLS transport traits and QUIC stream
//!   transport used by the protocol engines

pub(crate) mod eval;
pub(crate) mod h2;
#[allow(dead_code)] // Phase 3: protocol layer ready, QUIC transport gated
pub(crate) mod h3;
#[allow(dead_code)] // Phase 1 defines interfaces consumed in Phase 2+
pub(crate) mod transport;
