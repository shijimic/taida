//! C27B-006 (@c.27 Round 2, wE) — HTTP parity retry shim absence invariant.
//!
//! # Background
//!
//! C26B-006 wJ Round 4 (2026-04-24) removed the test-side connect-with-retry
//! shim from `test_tcp_accept_sendall_recvexact_three_way_parity` and
//! `test_udp_send_recv_loopback_parity` after C26B-003 (port allocator
//! race-free) + C26B-026 (HPACK) closed the underlying flakiness window.
//!
//! C27B-006 confirms the shim is gone and pins it as an invariant: any
//! reintroduction (e.g. someone wrapping the naive 1-shot in a
//! `for _ in 0..N { match TcpStream::connect ...` pattern) flips this
//! test RED, forcing the author to either (a) document why the shim is
//! truly necessary or (b) apply a root-cause fix elsewhere.
//!
//! This is intentionally a **source-text invariant test** rather than a
//! behavioral test. Behavior is already covered by parity.rs's 3-way
//! parity assertions; what we lock here is the explicit non-existence of
//! the workaround pattern in the affected fixtures.
//!
//! # D28 escalation checklist (3 points, all NO → C27 scope-in)
//!
//!  1. **Public mold signature unchanged.** Source-text grep only.
//!  2. **No STABILITY-pinned error string altered.** No code change.
//!  3. **Append-only with respect to existing fixtures.** New test crate.
//!
//! # Scope of the invariant
//!
//! For the two functions named below, assert:
//!
//!  1. The function body contains the C26B-006 撤廃 marker comment.
//!  2. The function body does NOT contain any client-connect retry loop
//!     pattern (`for _ in 0..N { ... TcpStream::connect ... }` where N
//!     is small).
//!  3. The function body contains exactly one `tcpListen(` invocation
//!     (the naive 1-shot path).
//!
//! These together prove the retry shim has not silently re-appeared.

mod common;

use std::fs;

const PARITY_PATH: &str = "tests/parity.rs";

const FUNCTIONS_TO_VERIFY: &[&str] = &[
    "test_tcp_accept_sendall_recvexact_three_way_parity",
    "test_udp_send_recv_loopback_parity",
];

fn read_parity_source() -> String {
    fs::read_to_string(PARITY_PATH).expect("read tests/parity.rs")
}

/// Extract the body of `fn <name>() { ... }` by brace-matching.
/// Returns None if the function is not found.
fn extract_function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("fn {}(", name);
    let start = source.find(&needle)?;
    // Find the opening brace after the signature.
    let body_start = source[start..].find('{')?;
    let abs_body_start = start + body_start;
    let bytes = source.as_bytes();
    let mut depth: i32 = 0;
    let mut i = abs_body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&source[abs_body_start..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[test]
fn c27b_006_retry_shim_marker_present() {
    // Each function MUST carry the explicit C26B-006 撤廃 marker, which
    // reviewers see when they are tempted to re-add the shim.
    let source = read_parity_source();
    for fn_name in FUNCTIONS_TO_VERIFY {
        let body = extract_function_body(&source, fn_name)
            .unwrap_or_else(|| panic!("c27b_006: function `{}` not found in parity.rs", fn_name));
        assert!(
            body.contains("C26B-006") && body.contains("retry shim撤廃"),
            "c27b_006: function `{}` is missing the C26B-006 撤廃 marker. \
             If the shim is re-added the marker comment must be removed too — \
             do NOT silently bypass this guard.",
            fn_name
        );
    }
}

#[test]
fn c27b_006_naive_one_shot_preserved() {
    // Each function MUST contain exactly one `tcpListen(` (TCP test) or
    // `udpBind(` (UDP test) invocation in the inlined Taida source.
    // Re-adding a retry-shim usually duplicates the listener bind block,
    // so this asymmetric guard catches re-addition cleanly.
    let source = read_parity_source();

    let tcp_body = extract_function_body(&source, FUNCTIONS_TO_VERIFY[0])
        .expect("tcp accept fn body");
    let tcp_listen_count = tcp_body.matches("tcpListen(").count();
    assert_eq!(
        tcp_listen_count, 1,
        "c27b_006: TCP fn should contain exactly 1 `tcpListen(`, found {}. \
         (More than 1 likely means a retry shim has been re-added.)",
        tcp_listen_count
    );

    let udp_body = extract_function_body(&source, FUNCTIONS_TO_VERIFY[1])
        .expect("udp echo fn body");
    let udp_bind_count = udp_body.matches("udpBind(").count();
    assert_eq!(
        udp_bind_count, 1,
        "c27b_006: UDP fn should contain exactly 1 `udpBind(`, found {}. \
         (More than 1 likely means a retry shim has been re-added.)",
        udp_bind_count
    );
}

#[test]
fn c27b_006_no_outer_connect_retry_loop() {
    // Catch the specific anti-pattern: an outer `for _ in 0..N` loop
    // wrapping `TcpStream::connect`. The inner client-side code paths
    // already do single-shot connect.
    let source = read_parity_source();
    for fn_name in FUNCTIONS_TO_VERIFY {
        let body = extract_function_body(&source, fn_name)
            .expect("fn body for retry-loop scan");
        // Heuristic: the bad pattern is `for _ in 0..N` where N is a
        // small literal, with a `TcpStream::connect` somewhere inside.
        // We forbid both the `for _ in 0..` loop AND any `tcp_connect_retry`
        // helper invocation in these bodies.
        for forbidden in &["tcp_connect_retry", "TcpStream_connect_retry"] {
            assert!(
                !body.contains(forbidden),
                "c27b_006: function `{}` reintroduced banned helper `{}`",
                fn_name, forbidden
            );
        }
        // Conservative check: a `for _ in 0..` followed within ~200
        // chars by `TcpStream::connect`.
        let mut search_from = 0usize;
        while let Some(idx) = body[search_from..].find("for _ in 0..") {
            let abs = search_from + idx;
            let window_end = (abs + 240).min(body.len());
            let window = &body[abs..window_end];
            assert!(
                !window.contains("TcpStream::connect"),
                "c27b_006: function `{}` contains a `for _ in 0..` loop \
                 wrapping `TcpStream::connect` — this is the C26B-006 \
                 retry shim being re-introduced. Apply a root-cause fix \
                 to the underlying flakiness instead.",
                fn_name
            );
            search_from = abs + 1;
        }
    }
}
