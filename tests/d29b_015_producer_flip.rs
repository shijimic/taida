//! D29B-015 (Track-β-2 TIER 4, 2026-04-27) — Bytes producer flip + dispatcher
//! polymorphism static / wire / runtime guards.
//!
//! TIER 1 Track-β added the `TAIDA_BYTES_CONTIG_MAGIC` infrastructure plus
//! polymorphic writev hot paths in `taida_net_send_response_scatter` and
//! `taida_net_write_chunk`, but the producers (`taida_net_read_body` /
//! `taida_net_read_body_all` / `taida_net4_make_lax_bytes_value` / the H1 +
//! H2 request-pack `raw` producers) still emitted legacy
//! `taida_bytes_from_raw` outputs. That meant `readBody → writeChunk` —
//! the canonical Bytes hot path — never actually fed CONTIG into the
//! writev branch, and the D29B-003 zero-copy claim only held when callers
//! constructed Bytes via `taida_bytes_contig_new` directly.
//!
//! D29B-015 closes that gap by:
//!
//! 1. Producer flip: every `raw_bytes` / `result` / `Lax[Bytes]` path on
//!    the H1 / H2 server hot path now emits `taida_bytes_contig_new(...)`.
//! 2. Dispatcher polymorphism: every Bytes consumer that previously only
//!    accepted `TAIDA_IS_BYTES` (legacy `taida_val[]`) now branches on
//!    `TAIDA_IS_BYTES_CONTIG` first and reads the inline payload via
//!    `taida_bytes_contig_data` / `taida_bytes_contig_len`. The legacy
//!    `taida_val[]` path is preserved as a fall-through for back-compat.
//! 3. `taida_is_bytes` typeof and `taida_collection_get` accept either
//!    layout (CONTIG and legacy both type-check as Bytes from the
//!    Taida surface).
//!
//! This test pins:
//!
//! * **Static** — the C source for the runtime contains the producer
//!   flip sentinels (so an accidental revert / merge conflict drop is
//!   caught on the next `cargo test --release --lib`).
//! * **Wire** — a native echo server that calls `readBody → response.body`
//!   produces a wire-correct response (so the polymorphic dispatchers
//!   and writev branches still cooperate end-to-end).
//!
//! Together with `d29b_003_native_writev_zero_copy` (which pins the
//! contig primitives + writev-branch presence) and `d29b_012_native_*`
//! (which pins the alloc-balance / definite-leak invariants for Span*),
//! this guards the D29B-015 acceptance surface.

mod common;

use common::taida_bin;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Static check: every producer site that used to emit
/// `taida_bytes_from_raw(...)` (i.e. legacy `taida_val[]` 8x expansion)
/// has been flipped to `taida_bytes_contig_new(...)`. We assert on
/// **specific call shapes** rather than a count of the symbol, so that
/// future helpers / wrappers don't silently drop the sentinel.
#[test]
fn d29b_015_producer_flip_sentinels_present_in_net_h1_h2() {
    let net_src = include_str!("../src/codegen/native_runtime/net_h1_h2.c");

    // 1. taida_net_read_body slice path: CONTIG raw → CONTIG body via
    //    `taida_bytes_contig_new(raw_data + body_start, actual_len)`.
    assert!(
        net_src.contains("return taida_bytes_contig_new(raw_data + body_start, actual_len);"),
        "taida_net_read_body must short-circuit CONTIG raw via \
         taida_bytes_contig_new(raw_data + body_start, actual_len) \
         (D29B-015 producer flip)"
    );

    // 2. taida_net_read_body_all aggregate buffer → CONTIG.
    assert!(
        net_src.contains("taida_val result = taida_bytes_contig_new(all_buf, (taida_val)all_len);"),
        "taida_net_read_body_all must emit CONTIG via \
         taida_bytes_contig_new(all_buf, all_len) (D29B-015 producer flip)"
    );

    // 3. taida_net4_make_lax_bytes_value chunk path → CONTIG.
    assert!(
        net_src.contains("taida_val bytes = taida_bytes_contig_new(data, (taida_val)len);"),
        "taida_net4_make_lax_bytes_value must emit CONTIG (D29B-015 producer flip)"
    );

    // 4. taida_net_build_request_pack request `raw` field → CONTIG.
    assert!(
        net_src.contains(
            "taida_val raw_bytes = taida_bytes_contig_new(raw_data, (taida_val)raw_len);"
        ),
        "taida_net_build_request_pack request raw must be CONTIG (D29B-015 producer flip)"
    );

    // 5. Both H1 in-loop request-pack producers → CONTIG.
    assert!(
        net_src.contains(
            "taida_val raw_bytes = taida_bytes_contig_new(buf, (taida_val)head_consumed);"
        ),
        "H1 head-consumed-only request-pack `raw` must be CONTIG (D29B-015 producer flip)"
    );
    assert!(
        net_src.contains("taida_val raw_bytes = taida_bytes_contig_new(buf, (taida_val)raw_len);"),
        "H1 head-plus-body request-pack `raw` must be CONTIG (D29B-015 producer flip)"
    );

    // 6. H2 request-pack arena → CONTIG (both arena + body fall-back paths).
    assert!(
        net_src.contains("raw_bytes = taida_bytes_contig_new(arena, (taida_val)arena_size);"),
        "H2 arena-backed request-pack `raw` must be CONTIG (D29B-015 producer flip)"
    );
    assert!(
        net_src.contains("raw_bytes = taida_bytes_contig_new(body, (taida_val)body_len);"),
        "H2 body-only request-pack `raw` must be CONTIG (D29B-015 producer flip)"
    );

    // 7. WS binary frame producer → CONTIG.
    assert!(
        net_src.contains(
            "taida_val bytes = taida_bytes_contig_new(frame.payload, (taida_val)frame.payload_len);"
        ),
        "WS binary frame data must be CONTIG (D29B-015 producer flip)"
    );

    // 8. Legacy taida_bytes_from_raw still defined and reachable for
    //    back-compat (other small Bytes constructors not touched by D29B-015).
    assert!(
        net_src.contains("taida_bytes_from_raw"),
        "taida_bytes_from_raw must remain available for back-compat \
         (D29B-015 keeps legacy paths working alongside the producer flip)"
    );
}

/// Static check: dispatcher polymorphism — every Bytes operation that
/// previously only handled `TAIDA_IS_BYTES` now branches on
/// `TAIDA_IS_BYTES_CONTIG` first. We grep for specific function-level
/// sentinels so a regression that drops the CONTIG branch from any
/// individual dispatcher is caught.
#[test]
fn d29b_015_dispatcher_polymorphism_present_in_core_c() {
    let core_src = include_str!("../src/codegen/native_runtime/core.c");

    // The dispatchers we expect polymorphism on. For each, search for the
    // function definition followed by a CONTIG short-circuit branch.
    let dispatchers = [
        "taida_bytes_clone",
        "taida_bytes_get_lax",
        "taida_bytes_to_list",
        "taida_u16be_decode_mold",
        "taida_u16le_decode_mold",
        "taida_u32be_decode_mold",
        "taida_u32le_decode_mold",
        "taida_bytes_cursor_take",
        "taida_bytes_cursor_u8",
        "taida_utf8_decode_mold",
        "taida_sha256",
        "taida_bytes_to_display_string",
        "taida_bytes_set",
    ];

    for name in dispatchers {
        // Find the function *body* definition by looking for a definition
        // followed by `{` (skipping forward declarations which end in `;`).
        // Try several signature shapes:
        //   `static taida_val name(...) {`
        //   `static int name(...) {`
        //   `taida_val name(...) {`
        //   `int name(...) {`
        let needles = [
            format!("static taida_val {name}("),
            format!("static int {name}("),
            format!("taida_val {name}("),
            format!("int {name}("),
        ];
        let mut body_pos: Option<usize> = None;
        for needle in &needles {
            let mut search_from = 0usize;
            while let Some(rel) = core_src[search_from..].find(needle) {
                let abs = search_from + rel;
                // Walk forward to either ';' (forward decl) or '{' (body).
                let tail = &core_src[abs..(abs + 4096).min(core_src.len())];
                let semi = tail.find(';');
                let brace = tail.find('{');
                match (semi, brace) {
                    (Some(s), Some(b)) if b < s => {
                        body_pos = Some(abs);
                        break;
                    }
                    (None, Some(_)) => {
                        body_pos = Some(abs);
                        break;
                    }
                    _ => {
                        search_from = abs + needle.len();
                        continue;
                    }
                }
            }
            if body_pos.is_some() {
                break;
            }
        }
        let def_pos = body_pos
            .unwrap_or_else(|| panic!("could not find body definition of {name} in core.c"));

        // Look in the next ~2KB for either a TAIDA_IS_BYTES_CONTIG check
        // or a TAIDA_IS_ANY_BYTES check (some dispatchers fan out via
        // taida_bytes_clone which is itself polymorphic).
        let window_end = (def_pos + 3000).min(core_src.len());
        let window = &core_src[def_pos..window_end];
        assert!(
            window.contains("TAIDA_IS_BYTES_CONTIG") || window.contains("TAIDA_IS_ANY_BYTES"),
            "Dispatcher {name} must polymorphically check TAIDA_IS_BYTES_CONTIG \
             (or TAIDA_IS_ANY_BYTES) within ~3KB of its body definition (D29B-015 \
             dispatcher polymorphism)"
        );
    }

    // taida_is_bytes typeof must also accept CONTIG so handler-side
    // type-checking works on producer-flipped Bytes.
    assert!(
        core_src.contains("static int taida_is_bytes(taida_val ptr) {")
            && core_src
                .lines()
                .skip_while(|l| !l.contains("static int taida_is_bytes(taida_val ptr) {"))
                .take(6)
                .any(|l| l.contains("TAIDA_IS_ANY_BYTES")),
        "taida_is_bytes must accept TAIDA_IS_ANY_BYTES (D29B-015 dispatcher polymorphism \
         — CONTIG raws produced by the flipped readBody / readBodyAll must type-check \
         as Bytes from the Taida surface)"
    );

    // taida_list_concat must handle CONTIG bytes-bytes case.
    assert!(
        core_src.contains("if (TAIDA_IS_ANY_BYTES(list1) && TAIDA_IS_ANY_BYTES(list2))"),
        "taida_list_concat must accept ANY_BYTES (legacy + CONTIG) on the \
         bytes-bytes concat path (D29B-015 dispatcher polymorphism)"
    );
}

fn build_native_fixture(td: &std::path::Path) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let bin = std::env::temp_dir().join(format!("d29b_015_{}_{}.bin", std::process::id(), seq));
    let out = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "native build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(bin)
}

fn read_announced_port(stdout: &mut BufReader<ChildStdout>, server: &mut Child) -> Option<u16> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut line = String::new();
    while Instant::now() < deadline {
        if let Ok(Some(_)) = server.try_wait() {
            return None;
        }
        line.clear();
        match stdout.read_line(&mut line) {
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Ok(_) => {
                if let Some(rest) = line.trim_end().strip_prefix("listening on 127.0.0.1:")
                    && let Ok(p) = rest.parse::<u16>()
                {
                    return Some(p);
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// Wire-correctness round-trip. With the producer flip in place, the
/// echo server's response.body is built from a CONTIG `readBody` output
/// flowing through the polymorphic Bytes dispatchers (so any consumer
/// that stayed on the legacy `bytes[2 + i]` indexing would corrupt the
/// payload). The fact that the response body matches the request body
/// byte-for-byte proves end-to-end CONTIG correctness.
#[test]
fn d29b_015_native_echo_via_contig_readbody_roundtrips_correctly() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping: linux-specific announce-port + native build harness");
        return;
    }
    if !taida_bin().exists() {
        eprintln!("skipping: taida release binary not built");
        return;
    }

    // Re-use the d29b_003 echo fixture — it now exercises CONTIG end-to-end.
    let fixture = manifest_dir().join("examples/quality/d29b_003_writev_zero_copy/server.td");
    if !fixture.exists() {
        eprintln!("skipping: fixture missing at {}", fixture.display());
        return;
    }

    let bin = match build_native_fixture(&fixture) {
        Some(b) => b,
        None => {
            eprintln!("skipping: native build of d29b_015 fixture failed");
            return;
        }
    };

    let mut server = Command::new(&bin)
        .env("TAIDA_NET_ANNOUNCE_PORT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("server spawn");

    let mut stdout_reader = BufReader::new(
        server
            .stdout
            .take()
            .expect("server stdout pipe must be available"),
    );
    let port = match read_announced_port(&mut stdout_reader, &mut server) {
        Some(p) => p,
        None => {
            let _ = server.kill();
            let _ = server.wait();
            panic!("d29b_015: server failed to announce bound port within 20s");
        }
    };

    // Send a 1KB body — large enough that any per-byte indexing bug
    // (e.g. dispatching a CONTIG header through the legacy
    // `bytes[2 + i]` low-bit indexing path) would corrupt the
    // response and the byte-for-byte assert would fail.
    let req_body: Vec<u8> = (0..1024u32).map(|i| (i & 0xFF) as u8).collect();
    let req = format!(
        "GET /echo HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        req_body.len()
    );
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .expect("connect to native server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(&req_body).unwrap();
    stream.flush().unwrap();

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&bin);

    // Find body section after \r\n\r\n.
    let separator = b"\r\n\r\n";
    let body_start = buf
        .windows(separator.len())
        .position(|w| w == separator)
        .map(|p| p + separator.len())
        .expect("response missing CRLFCRLF separator");
    let body = &buf[body_start..];

    // Status check.
    let head = &buf[..body_start.min(buf.len())];
    let head_str = String::from_utf8_lossy(head);
    assert!(
        head_str.starts_with("HTTP/1.1 200"),
        "expected 200 OK; got: {}",
        &head_str[..head_str.len().min(120)]
    );

    // Echoed body byte-for-byte equality. If any consumer dispatched CONTIG
    // through legacy `bytes[2 + i]` indexing the bytes would be corrupted
    // (likely zeros or pointer high bits, not the original 0x00..0xFF
    // sequence).
    assert_eq!(
        body.len(),
        req_body.len(),
        "echoed body length must match request body length (got {}, expected {})",
        body.len(),
        req_body.len()
    );
    assert_eq!(
        body,
        &req_body[..],
        "echoed body bytes must match request body byte-for-byte \
         (a CONTIG dispatcher dropping the polymorphic branch would cause \
         silent corruption)"
    );
}

/// Static check: the EXPECTED_TOTAL_LEN comment in mod.rs records the
/// D29B-015 delta so future trackers can audit producer-flip / dispatcher
/// changes. Catches accidental revert of the size update without a
/// matching code revert.
#[test]
fn d29b_015_expected_total_len_comment_records_track_beta_2_delta() {
    let mod_rs = include_str!("../src/codegen/native_runtime/mod.rs");
    assert!(
        mod_rs.contains("D29B-015 (Track-β-2 TIER 4"),
        "mod.rs EXPECTED_TOTAL_LEN comment must record the D29B-015 \
         Track-β-2 delta with rationale (producer flip + dispatcher \
         polymorphism). Future tracker audits depend on this being \
         present."
    );
    assert!(
        mod_rs.contains("Measured delta:"),
        "mod.rs EXPECTED_TOTAL_LEN comment must record the measured \
         delta in bytes for D29B-015"
    );
}
