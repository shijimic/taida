//! HTTP/3 serve entry point for the Taida interpreter's net package,
//! split out from `net_eval/mod.rs` (C13-3).
//!
//! Owns the `impl Interpreter` method `serve_h3` which mirrors the
//! native backend's `taida_net_h3_serve()`: runs protocol self-tests,
//! wires up the handler closure that materializes a 14-field request
//! pack, and hands control to `super::super::super::net_h3::serve_h3_loop`.
//!
//! C13-3 note: pure mechanical move — no behavior change. The HTTP/1.1
//! `eval_http_serve` implementation in `h1.rs` delegates into
//! `self.serve_h3(...)` when `protocol: "h3"` is requested.

use super::super::eval::{Interpreter, RuntimeError, Signal};
use super::super::value::Value;
use super::helpers::{
    extract_response_fields, make_fulfilled_async, make_result_failure_msg, make_result_success,
    make_span,
};

impl Interpreter {
    /// HTTP/3 serve entry point (NET7-3a: Interpreter parity backend).
    ///
    /// Mirrors the Native backend's `taida_net_h3_serve()`:
    ///   1. Run H3 protocol layer self-tests (QPACK round-trip, request validation)
    ///   2. Build handler closure (H3RequestData -> 14-field request pack -> handler -> H3ResponseData)
    ///   3. Run serve_h3_loop with sequential accept + handler dispatch
    ///   4. Return @(ok: true, requests: N) on success
    ///
    /// NET7-12b: The handler closure builds the same 14-field request pack
    /// as h1/h2, calls the user function synchronously, and extracts the
    /// response. The serve loop alternates between async I/O and sync handler
    /// dispatch, matching the Interpreter's single-threaded serial model.
    ///
    /// Design contracts (NET_DESIGN.md):
    ///   - cert/key required (validated before reaching here)
    ///   - 0-RTT: default-off, not exposed
    ///   - Handler dispatch: same 14-field request pack as h1/h2
    ///   - request_count: incremented only on valid HEADERS + handler success
    ///   - Graceful shutdown: GOAWAY -> drain -> close
    ///   - Bounded-copy discipline: 1 packet = at most 1 materialization
    ///   - Transport I/O does NOT use the existing Transport trait (NB7-7)
    pub(super) fn serve_h3(
        &mut self,
        cert_path: String,
        key_path: String,
        handler: super::super::value::FuncValue,
        max_requests: i64,
        port: u16,
    ) -> Result<Option<Signal>, RuntimeError> {
        use super::super::net_h3;

        // NB7-9/NB7-10: Run embedded self-tests to validate QPACK round-trip
        // and H3 request pseudo-header validation, matching Native behavior.
        match net_h3::run_selftests() {
            net_h3::SelftestResult::Ok => {}
            net_h3::SelftestResult::QpackFailure(rc) => {
                let result = make_result_failure_msg(
                    "H3SelftestFailed",
                    format!(
                        "httpServe: HTTP/3 protocol layer self-test failed. \
                         QPACK encode/decode round-trip failed (code: {}).",
                        rc
                    ),
                );
                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
            }
            net_h3::SelftestResult::ValidationFailure(rc) => {
                let result = make_result_failure_msg(
                    "H3SelftestFailed",
                    format!(
                        "httpServe: HTTP/3 protocol layer self-test failed. \
                         Request pseudo-header validation failed (code: {}).",
                        rc
                    ),
                );
                return Ok(Some(Signal::Value(make_fulfilled_async(result))));
            }
        }

        // NET7-12b: Connect to the real QUIC transport loop with handler dispatch.
        //
        // The Interpreter H3 path uses quinn (pure Rust, tokio-native) as the
        // QUIC substrate. Unlike the Native backend which uses libquiche via
        // dlopen, the Interpreter compiles quinn in at build time -- no runtime
        // library gate is needed.
        //
        // serve_h3_loop() creates a single-threaded tokio runtime internally,
        // using per-step block_on() to alternate between async I/O and sync
        // handler dispatch. The handler closure builds the same 14-field request
        // pack as h1/h2, calls the user function, and extracts the response.
        //
        // request_count is incremented only on valid HEADERS decode + successful
        // handler completion (NET7-12b contract).

        // NET7-12b: Handler dispatch closure.
        // Converts H3RequestData -> 14-field request pack -> handler call -> H3ResponseData.
        // Returns None on handler error (serve loop sends 500).
        let mut h3_handler = |req: net_h3::H3RequestData| -> Option<net_h3::H3ResponseData> {
            // C26B-022 Step 2 (wJ Round 4, 2026-04-24): Enforce HTTP/3
            // wire byte upper limits at the handler boundary so that
            // downstream Native codegen fixed-size stack buffers cannot
            // silently truncate. On violation, synthesize a 400 Bad
            // Request response; the serve loop increments request_count
            // the same way it would for any handler-returned response.
            if req.method.len() > super::h1::HTTP_WIRE_MAX_METHOD_LEN
                || req.path.len() > super::h1::HTTP_WIRE_MAX_PATH_LEN
                || req.authority.len() > super::h1::HTTP_WIRE_MAX_AUTHORITY_LEN
            {
                return Some(net_h3::H3ResponseData {
                    status: 400,
                    headers: Vec::new(),
                    body: Vec::new(),
                });
            }

            // Parse query from path (matching h1/h2 pattern).
            let (path_part, query_part) = match req.path.find('?') {
                Some(pos) => (req.path[..pos].to_string(), req.path[pos + 1..].to_string()),
                None => (req.path.clone(), String::new()),
            };

            // D29B-011 (Track-ζ Lock-H, 2026-04-27): build a per-request arena
            // mirroring the h2 strategy in `h2.rs::serve_h2`. QPACK has the
            // same dynamic-table reallocation problem as HPACK, so the only
            // way to give Span* mold a stable backing buffer is to copy the
            // decoded pseudo / regular header bytes into a fresh arena
            // alongside the body. This brings h3 to byte-identical span
            // shape with h1 (parse_request_head -> span packs) and h2
            // (post-D29B-001 arena) and lets `SpanEquals[req.method,
            // req.raw, "GET"]()` succeed under h3 instead of silently
            // returning false.
            //
            // Arena layout (Strategy V1-A from
            // `Phase-5_..._track-zeta_sub-Lock.md`):
            //   [body | method | path | query | n1 v1 n2 v2 ... | "host" authority]
            // body lives at offset 0 so the existing `body` span and
            // `bodyOffset = 0` invariants are preserved.
            let body_len = req.body.len();
            let mut arena_cap = body_len + req.method.len() + path_part.len() + query_part.len();
            for (name, value) in &req.headers {
                arena_cap += name.len() + value.len();
            }
            if !req.authority.is_empty() {
                arena_cap += 4 /* "host" */ + req.authority.len();
            }

            let mut arena: Vec<u8> = Vec::with_capacity(arena_cap);
            arena.extend_from_slice(&req.body);

            let method_start = arena.len();
            let method_len = req.method.len();
            arena.extend_from_slice(req.method.as_bytes());

            let path_start = arena.len();
            let path_len = path_part.len();
            arena.extend_from_slice(path_part.as_bytes());

            let query_start = arena.len();
            let query_len = query_part.len();
            arena.extend_from_slice(query_part.as_bytes());

            let mut header_spans: Vec<(usize, usize, usize, usize)> =
                Vec::with_capacity(req.headers.len() + 1);
            for (name, value) in &req.headers {
                let n_start = arena.len();
                let n_len = name.len();
                arena.extend_from_slice(name.as_bytes());
                let v_start = arena.len();
                let v_len = value.len();
                arena.extend_from_slice(value.as_bytes());
                header_spans.push((n_start, n_len, v_start, v_len));
            }
            if !req.authority.is_empty() {
                let n_start = arena.len();
                arena.extend_from_slice(b"host");
                let v_start = arena.len();
                let v_len = req.authority.len();
                arena.extend_from_slice(req.authority.as_bytes());
                header_spans.push((n_start, 4, v_start, v_len));
            }

            // Build request pack matching h2 1-arg handler contract.
            let mut request_fields: Vec<(String, Value)> = vec![
                ("method".into(), make_span(method_start, method_len)),
                ("path".into(), make_span(path_start, path_len)),
                ("query".into(), make_span(query_start, query_len)),
                (
                    "version".into(),
                    Value::pack(vec![
                        ("major".into(), Value::Int(3)),
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

            // Body span still references the leading `body_len` bytes of the
            // arena (offset 0). bodyOffset = 0 keeps existing addons that
            // slice via `Slice[req.raw, bodyOffset, bodyOffset + contentLength]`
            // pointing at the body region.
            request_fields.push(("body".into(), make_span(0, body_len)));
            request_fields.push(("bodyOffset".into(), Value::Int(0)));
            request_fields.push(("contentLength".into(), Value::Int(body_len as i64)));
            // raw = arena (body + headers concat). Track-ε's Arc<BytesValue>
            // interior wrapping keeps `req.raw` zero-copy on subsequent
            // clones; the arena allocation cost is a single Vec::with_capacity
            // sized exactly for the request, no re-alloc during build.
            request_fields.push(("raw".into(), Value::bytes(arena)));
            request_fields.push((
                "remoteHost".into(),
                Value::str(req.remote_addr.ip().to_string()),
            ));
            request_fields.push((
                "remotePort".into(),
                Value::Int(req.remote_addr.port() as i64),
            ));
            request_fields.push(("keepAlive".into(), Value::Bool(true)));
            request_fields.push(("chunked".into(), Value::Bool(false)));
            request_fields.push(("protocol".into(), Value::str("h3".into())));

            let request_pack = Value::pack(request_fields);

            // Call handler with request pack (1-arg path, same as h2).
            let handler_result = self.call_function_with_values(&handler, &[request_pack]);

            match handler_result {
                Ok(response) => {
                    // Extract response fields using the same extractor as h1/h2.
                    match extract_response_fields(&response) {
                        Ok((status, headers, body)) => Some(net_h3::H3ResponseData {
                            status: status as u16,
                            headers,
                            body,
                        }),
                        Err(_) => None, // Invalid response from handler.
                    }
                }
                Err(_) => None, // Handler threw an error.
            }
        };

        match net_h3::serve_h3_loop(&cert_path, &key_path, port, max_requests, &mut h3_handler) {
            Ok(request_count) => {
                let result_inner = Value::pack(vec![
                    ("ok".into(), Value::Bool(true)),
                    ("requests".into(), Value::Int(request_count)),
                ]);
                let result = make_result_success(result_inner);
                Ok(Some(Signal::Value(make_fulfilled_async(result))))
            }
            Err(e) => {
                // Classify the error: quinn/rustls initialization failures
                // are ProtocolError; runtime transport failures are separate.
                let kind = if e.contains("failed to read cert")
                    || e.contains("failed to read key")
                    || e.contains("failed to parse")
                    || e.contains("TLS config failed")
                    || e.contains("unsupported key type")
                    || e.contains("no valid certificates")
                    || e.contains("no PEM items")
                    || e.contains("failed to create QUIC endpoint")
                    || e.contains("failed to parse bind address")
                {
                    "ProtocolError"
                } else {
                    "H3RuntimeError"
                };
                let result = make_result_failure_msg(kind, e);
                Ok(Some(Signal::Value(make_fulfilled_async(result))))
            }
        }
    }
}
