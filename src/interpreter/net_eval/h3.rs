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
            // Parse query from path (matching h1/h2 pattern).
            let (path_part, query_part) = match req.path.find('?') {
                Some(pos) => (req.path[..pos].to_string(), req.path[pos + 1..].to_string()),
                None => (req.path.clone(), String::new()),
            };

            // Build request pack matching h2 1-arg handler contract.
            let mut request_fields: Vec<(String, Value)> = vec![
                ("method".into(), Value::Str(req.method)),
                ("path".into(), Value::Str(path_part)),
                ("query".into(), Value::Str(query_part)),
                (
                    "version".into(),
                    Value::BuchiPack(vec![
                        ("major".into(), Value::Int(3)),
                        ("minor".into(), Value::Int(0)),
                    ]),
                ),
            ];

            // Convert H3 headers to the same format as h1/h2.
            let mut header_values: Vec<Value> = Vec::new();
            for (name, value) in &req.headers {
                header_values.push(Value::BuchiPack(vec![
                    ("name".into(), Value::Str(name.clone())),
                    ("value".into(), Value::Str(value.clone())),
                ]));
            }
            // Add :authority as host header for compatibility (same as h2).
            if !req.authority.is_empty() {
                header_values.push(Value::BuchiPack(vec![
                    ("name".into(), Value::Str("host".into())),
                    ("value".into(), Value::Str(req.authority.clone())),
                ]));
            }
            request_fields.push(("headers".into(), Value::list(header_values)));

            // Body.
            let raw_len = req.body.len();
            request_fields.push(("body".into(), make_span(0, raw_len)));
            request_fields.push(("bodyOffset".into(), Value::Int(0)));
            request_fields.push(("contentLength".into(), Value::Int(raw_len as i64)));
            request_fields.push(("raw".into(), Value::Bytes(req.body)));
            request_fields.push((
                "remoteHost".into(),
                Value::Str(req.remote_addr.ip().to_string()),
            ));
            request_fields.push((
                "remotePort".into(),
                Value::Int(req.remote_addr.port() as i64),
            ));
            request_fields.push(("keepAlive".into(), Value::Bool(true)));
            request_fields.push(("chunked".into(), Value::Bool(false)));
            request_fields.push(("protocol".into(), Value::Str("h3".into())));

            let request_pack = Value::BuchiPack(request_fields);

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
                let result_inner = Value::BuchiPack(vec![
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
