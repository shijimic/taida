/// Net package evaluation for the Taida interpreter.
///
/// Implements `taida-lang/net` (core-bundled):
///
/// HTTP surface:
///   httpServe, httpParseRequestHead, httpEncodeResponse, readBody
///   startResponse, writeChunk, endResponse, sseEvent
///   readBodyChunk, readBodyAll
///   wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
///
/// C12B-025 (2026-04-15): mechanical split from a single 12,591-line file
/// into a directory module:
///   - types.rs   — type definitions (Writer state, body framing, ConnStream)
///   - helpers.rs — free helper functions (parser / encoder / chunked / Result)
///   - mod.rs     — `impl Interpreter { ... }` (try_net_func dispatch + all evaluators)
///   - tests.rs   — `#[cfg(test)] mod tests` extracted verbatim
///
/// C13-3 (2026-04-16): further mechanical split of the 4,208-line `mod.rs`
/// (only the `impl Interpreter` evaluator block) into responsibility-aligned
/// sibling modules. Each sibling hosts its own `impl Interpreter { ... }`
/// block — Rust allows the same type to be extended across multiple
/// `impl` blocks in the same crate, so `Self::` / associated-item lookup
/// still resolves uniformly. The `try_net_func` dispatcher below is the
/// sole public-ish surface referenced by `module_eval.rs` / `eval.rs`.
///
///   - stream.rs — v3 streaming (startResponse/writeChunk/endResponse/sseEvent)
///                 + v4 body streaming (readBodyChunk/readBodyAll) and their
///                 chunked-body helpers
///   - ws.rs     — WebSocket implementation (wsUpgrade/wsSend/wsReceive/
///                 wsClose/wsCloseCode) + frame helpers + finalize_websocket_close
///   - h1.rs     — HTTP/1.1 accept loop (eval_http_serve) + per-connection
///                 head read (try_read_request) + dispatch_request
///   - h2.rs     — HTTP/2 serve loop (serve_h2 / h2_connection_loop /
///                 send_h2_response)
///   - h3.rs     — HTTP/3 serve entry point (serve_h3)
///
/// Public API surface (path-stable via re-exports):
///   - `pub(crate) const NET_SYMBOLS`           (re-exported from types.rs)
///   - `pub(crate) struct ActiveStreamingWriter` (re-exported from types.rs)
///   - `pub(crate) fn try_net_func`              (defined in this file)
pub(crate) mod h1;
pub(crate) mod h2;
pub(crate) mod h3;
pub(crate) mod helpers;
pub(crate) mod stream;
pub(crate) mod types;
pub(crate) mod ws;

#[cfg(test)]
mod tests;

// Re-exports to preserve the path `super::net_eval::ActiveStreamingWriter` /
// `super::net_eval::NET_SYMBOLS` used by sibling modules (eval.rs, module_eval.rs).
pub(crate) use types::{ActiveStreamingWriter, NET_SYMBOLS};

use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::Value;
use crate::parser::Expr;

use helpers::{
    encode_response, eval_read_body, extract_body_token, is_body_stream_request, parse_request_head,
};

// ── Dispatch ────────────────────────────────────────────────

impl Interpreter {
    /// Try to handle a net built-in function call.
    /// Returns None if the name is not a recognized net function
    /// or if the function was not imported from taida-lang/net (sentinel guard).
    ///
    /// Supports alias imports: `>>> taida-lang/net => @(httpServe: serve)`
    /// binds `serve = "__net_builtin_httpServe"`. The guard extracts the original
    /// function name from the `__net_builtin_` prefix rather than deriving it
    /// from the local call name.
    pub(crate) fn try_net_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // Sentinel guard: extract original function name from __net_builtin_ prefix.
        // This supports alias imports where the local name differs from the export name.
        let original_name = match self.env.get(name) {
            Some(Value::Str(tag)) if tag.starts_with("__net_builtin_") => {
                tag["__net_builtin_".len()..].to_string()
            }
            _ => return Ok(None),
        };

        match original_name.as_str() {
            // ── httpParseRequestHead(bytes) → Result[@(parsed), _] ──
            "httpParseRequestHead" => {
                let bytes = self.eval_net_bytes_arg(args, 0, "httpParseRequestHead")?;
                Ok(Some(Signal::Value(parse_request_head(&bytes))))
            }

            // ── httpEncodeResponse(response) → Result[@(bytes: Bytes), _] ──
            "httpEncodeResponse" => {
                let response = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "httpEncodeResponse: missing response argument".into(),
                        });
                    }
                };
                Ok(Some(Signal::Value(encode_response(&response))))
            }

            // ── httpServe(port, handler, maxRequests, timeoutMs) ──
            // → Async[Result[@(ok: Bool, requests: Int), _]]
            "httpServe" => self.eval_http_serve(args),

            // ── readBody(req) → Bytes ──
            // v4: In a 2-arg handler, readBody acts as readBodyAll alias.
            "readBody" => {
                let req = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "readBody: missing argument 'req'".into(),
                        });
                    }
                };
                // v4: If the request has __body_stream sentinel (2-arg handler),
                // delegate to readBodyAll to stream from socket.
                if is_body_stream_request(&req) {
                    // NB4-7: Verify token before delegating.
                    if let Some(ref active) = self.active_streaming_writer
                        && !active.body_state.is_null()
                    {
                        let body = unsafe { &*active.body_state };
                        let pack_token = extract_body_token(&req);
                        if pack_token != Some(body.request_token) {
                            return Err(RuntimeError {
                                    message: "readBody: request pack does not match the current active request. \
                                             The request may be stale or fabricated.".into(),
                                });
                        }
                    }
                    return self.eval_read_body_all_impl("readBody");
                }
                Ok(Some(Signal::Value(eval_read_body(&req)?)))
            }

            // ── v4 request body streaming API ──
            // readBodyChunk(req) → Lax[Bytes]
            // readBodyAll(req) → Bytes
            // Protected by the same re-entrancy guard as v3 streaming API.
            "readBodyChunk" | "readBodyAll" => {
                // Evaluate the req argument first (before re-entrancy guard).
                let req = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: format!("{}: missing argument 'req'", original_name),
                        });
                    }
                };

                // NET4-1f: 1-arg handler request packs do NOT have __body_stream sentinel.
                if !is_body_stream_request(&req) {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: can only be called in a 2-argument httpServe handler. \
                             In a 1-argument handler, the request body is already fully read. \
                             Use readBody(req) instead.",
                            original_name
                        ),
                    });
                }

                // NB4-7: Verify that the request pack's token matches the active body state.
                if let Some(ref active) = self.active_streaming_writer
                    && !active.body_state.is_null()
                {
                    let body = unsafe { &*active.body_state };
                    let pack_token = extract_body_token(&req);
                    if pack_token != Some(body.request_token) {
                        return Err(RuntimeError {
                            message: format!(
                                "{}: request pack does not match the current active request. \
                                     The request may be stale or fabricated.",
                                original_name
                            ),
                        });
                    }
                }

                // Re-entrancy guard (same pattern as v3 streaming API).
                if let Some(ref active) = self.active_streaming_writer
                    && active.borrowed
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: cannot be called while another streaming API call is in progress (re-entrant call detected)",
                            original_name
                        ),
                    });
                }
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = true;
                }

                let result = match original_name.as_str() {
                    "readBodyChunk" => self.eval_read_body_chunk_impl(),
                    "readBodyAll" => self.eval_read_body_all_impl(&original_name),
                    _ => unreachable!(),
                };

                // Clear re-entrancy guard.
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = false;
                }

                result
            }

            // ── v3 streaming API ──
            // These functions are only callable inside a 2-arg httpServe handler.
            // The active_streaming_writer field is set during handler execution
            // and provides access to the StreamingWriter state and TcpStream.
            //
            // NB3-9: Re-entrancy guard — prevent nested streaming API calls
            // (e.g. `writeChunk(writer, writeChunk(writer, "data"))`) from
            // creating overlapping &mut references to the same StreamingWriter.
            // The guard is set here at the dispatch level so every streaming
            // function is protected uniformly, and cleared after the call
            // returns (or errors).
            "startResponse" | "writeChunk" | "endResponse" | "sseEvent" => {
                // Check re-entrancy before dispatching.
                if let Some(ref active) = self.active_streaming_writer
                    && active.borrowed
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: cannot be called while another streaming API call is in progress (re-entrant call detected)",
                            name
                        ),
                    });
                }
                // Set the guard.
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = true;
                }

                let result = match name {
                    "startResponse" => self.eval_start_response(args),
                    "writeChunk" => self.eval_write_chunk(args),
                    "endResponse" => self.eval_end_response(args),
                    "sseEvent" => self.eval_sse_event(args),
                    _ => unreachable!(),
                };

                // Clear the guard after the call completes (success or error).
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = false;
                }

                result
            }

            // ── v4 WebSocket API + v5 WebSocket revision ──
            // These functions are only callable inside a 2-arg httpServe handler.
            // Protected by the same re-entrancy guard as v3 streaming API.
            "wsUpgrade" | "wsSend" | "wsReceive" | "wsClose" | "wsCloseCode" => {
                // Check re-entrancy before dispatching.
                if let Some(ref active) = self.active_streaming_writer
                    && active.borrowed
                {
                    return Err(RuntimeError {
                        message: format!(
                            "{}: cannot be called while another streaming API call is in progress (re-entrant call detected)",
                            original_name
                        ),
                    });
                }
                // Set the guard.
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = true;
                }

                let result = match original_name.as_str() {
                    "wsUpgrade" => self.eval_ws_upgrade(args),
                    "wsSend" => self.eval_ws_send(args),
                    "wsReceive" => self.eval_ws_receive(args),
                    "wsClose" => self.eval_ws_close(args),
                    "wsCloseCode" => self.eval_ws_close_code(args),
                    _ => unreachable!(),
                };

                // Clear the guard after the call completes (success or error).
                if let Some(ref mut active) = self.active_streaming_writer {
                    active.borrowed = false;
                }

                result
            }

            _ => Ok(None),
        }
    }

    /// Evaluate a positional argument as raw bytes, accepting either
    /// `Bytes` (zero-copy) or `Str` (UTF-8 → Vec<u8>). Used by the net
    /// dispatch entry points that take opaque byte payloads (e.g.
    /// `httpParseRequestHead`).
    ///
    /// C13-3: kept here (next to `try_net_func`) rather than moved into
    /// a sibling module, since the dispatcher is its only caller.
    fn eval_net_bytes_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        func_name: &str,
    ) -> Result<Vec<u8>, RuntimeError> {
        let arg = args.get(index).ok_or_else(|| RuntimeError {
            message: format!("{}: missing bytes argument", func_name),
        })?;
        match self.eval_expr(arg)? {
            Signal::Value(Value::Bytes(b)) => Ok(Value::bytes_take(b)),
            Signal::Value(Value::Str(s)) => Ok(Value::str_take(s).into_bytes()),
            Signal::Value(v) => Err(RuntimeError {
                message: format!("{}: argument must be Bytes or Str, got {}", func_name, v),
            }),
            other => Err(RuntimeError {
                message: format!(
                    "{}: unexpected signal: {:?}",
                    func_name,
                    std::mem::discriminant(&other)
                ),
            }),
        }
    }
}
