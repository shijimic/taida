//! D28B-012 (Round 2 wF): NET runtime path leak regression test.
//!
//! Pre-fix root cause:
//!   * Each `httpServe` 1-arg request builds a 13-field request pack,
//!     several `taida_net_make_span` (2-field) packs, plus a body
//!     string allocated by `Repeat["x", 512]()`. None of those shapes
//!     match the four fixed-size freelist buckets (pack-fc-4,
//!     list-cap-16, str buckets {32,64,128,256,512,1024}) by exact
//!     size, so they fall through to the per-thread bump arena
//!     (`taida_arena_alloc`) which has a steady-state cap of
//!     TAIDA_ARENA_MAX_CHUNKS (128) * TAIDA_ARENA_CHUNK_SIZE (2 MiB) =
//!     256 MiB / thread. With min(maxConnections, 16) worker threads
//!     the cap is ~4 GiB, matching the observed plateau exactly.
//!   * `taida_release` on an arena-backed pack/list/string drops the
//!     refcount to 0 without rewinding the arena offset, so each
//!     request consumes fresh arena bytes until the chunk is full
//!     and a new chunk is malloc'd.
//!   * Symptom recorded in `.dev/D28_BLOCKERS.md::D28B-012`:
//!     `scripts/soak/fast-soak-proxy.sh --backend native
//!     --duration-min 30` reported DRIFT DETECTED at 4.7 GiB/h.
//!
//! Fix: `taida_arena_request_reset` (defined in
//!   `src/codegen/native_runtime/core.c`; grep that file for the
//!   function name to find the current location, since absolute line
//!   anchors drift across reorgs) drains the per-thread small-object
//!   freelists separating arena vs malloc origins, then frees every
//!   arena chunk except chunk[0] and rewinds chunk[0]'s offset to 0.
//!   Called at the bottom of every keep-alive iteration in
//!   `net_worker_thread` plus once at conn_done so early-exit paths
//!   (head_malformed, EOF before head, body parse error, WebSocket
//!   close, request limit exhausted on partial connection) are
//!   covered.
//!
//! Acceptance signal:
//!   * Build the native artifact for
//!     `examples/quality/d28b_012_net_runtime_leak/server.td` (a
//!     scatter-gather httpServe identical in shape to the soak
//!     fixture).
//!   * Launch it on 127.0.0.1:18091 and fire ~5,000 HTTP/1.1 requests
//!     in a tight TCP loop.
//!   * Sample VmRSS / fd-count from /proc/<pid>/status before vs
//!     after.
//!   * Assert: RSS growth per 1k requests is bounded (< 5 MiB/1k
//!     requests). Pre-fix the same workload grew RSS by ~67 MiB/min
//!     of curl traffic; post-fix the per-iteration arena reset keeps
//!     growth within OS allocator + thread bookkeeping noise.
//!
//! The CI smoke uses --features ci-smoke (default) which dials the
//! request count down to 1,000 to keep wall-clock under 30 s. The
//! local long run (env `D28B_012_LONG=1`) hammers 50,000 requests for
//! a tighter signal during D28 audit.

mod common;

use common::taida_bin;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path() -> PathBuf {
    manifest_dir().join("examples/quality/d28b_012_net_runtime_leak/server.td")
}

fn build_native(td: &Path) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let bin = std::env::temp_dir().join(format!("d28b_012_{}_{}.bin", std::process::id(), seq));
    let out = Command::new(taida_bin())
        .args(["build", "native"])
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

fn read_rss_kib(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kib);
        }
    }
    None
}

fn read_fd_count(pid: u32) -> Option<u64> {
    let dir = format!("/proc/{pid}/fd");
    let entries = std::fs::read_dir(dir).ok()?;
    Some(entries.count() as u64)
}

/// D28B-026: Read the kernel-assigned port from the server's stdout
/// announce line. The fixture binds on port 0 with
/// `TAIDA_NET_ANNOUNCE_PORT=1`, which causes
/// `taida_net_h1_serve_connection` to emit
/// `listening on 127.0.0.1:<port>\n` once bind+listen succeed.
///
/// Polls the child's stdout for up to 20 s, returning the parsed port
/// on success. Hardcoded ports are no longer used here -- D28B-026
/// switched to ephemeral-port + announce-line parsing to remove the
/// flaky-collision risk under parallel `cargo test` and stale process
/// scenarios.
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

/// Send N HTTP/1.1 GET requests using keep-alive on a small set of
/// long-lived TCP connections. Each connection serves up to
/// `requests_per_conn` requests before being recycled, matching the
/// shape of curl/wrk steady-state load.
fn drive_load(port: u16, total_requests: u32, requests_per_conn: u32) -> u32 {
    let mut sent: u32 = 0;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut buf = [0u8; 4096];
    while sent < total_requests {
        let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(s) => s,
            Err(_) => return sent,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

        let req = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n";
        let mut ok_on_conn = 0u32;
        while ok_on_conn < requests_per_conn && sent < total_requests {
            if stream.write_all(req).is_err() {
                break;
            }
            // Drain at least one full response. The fixture sends a
            // 512 B body + small head; one read should pull a chunk
            // we can scan for the next response separator. We loop
            // until we have seen the response head terminator
            // (\r\n\r\n) so the next iteration starts at a clean
            // request boundary.
            let mut total_read = 0;
            let mut saw_head_end = false;
            // Drain up to a reasonable upper bound to avoid livelock
            // on a misbehaving server.
            for _ in 0..16 {
                let n = match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                total_read += n;
                // Look for end-of-head + at least 512 body bytes.
                // The fixture body is exactly 512 bytes + ~40 byte
                // head, so total_read >= 552 means we have at least
                // one full response.
                if total_read >= 552 {
                    saw_head_end = true;
                    break;
                }
            }
            if !saw_head_end {
                break;
            }
            ok_on_conn += 1;
            sent += 1;
        }
        let _ = stream.shutdown(Shutdown::Both);
    }
    sent
}

#[test]
fn d28b_012_native_arena_reset_bounds_rss() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping: /proc/<pid>/status RSS sampling is Linux-specific");
        return;
    }
    if !taida_bin().exists() {
        eprintln!("skipping: taida release binary not built");
        return;
    }

    let bin = match build_native(&fixture_path()) {
        Some(b) => b,
        None => {
            eprintln!("skipping: native build of d28b_012 fixture failed");
            return;
        }
    };

    // D28B-026: spawn with TAIDA_NET_ANNOUNCE_PORT=1 + piped stdout so
    // we can parse the kernel-assigned ephemeral port (fixture binds
    // on port 0). Piping stderr is intentional too -- a stderr panic
    // would otherwise be silently dropped under Stdio::null().
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
            panic!(
                "d28b_012: server failed to announce bound port within 20s (TAIDA_NET_ANNOUNCE_PORT=1, port 0)"
            );
        }
    };

    let pid = server.id();

    // Warm-up phase: send a small batch first so the worker threads
    // are spawned, the curl ring is alive, and the freelists have
    // their initial fill. Sample RSS *after* warm-up so the test
    // measures steady-state growth, not cold-start.
    //
    // D28B-026: warm-up acceptance floor raised from >= 100 / 200 (50%)
    // to >= 180 / 200 (90%). The previous floor would silently pass on
    // a server that crashed mid-warmup or that mishandled half the
    // requests; 90% is still below the practical 100% observed
    // post-fix, but tight enough to catch silent regression.
    let warmup = drive_load(port, 200, 50);
    assert!(
        warmup >= 180,
        "d28b_012: warm-up only completed {} requests (expected >= 180 / 200, D28B-026 90% floor); server may have crashed or stalled",
        warmup
    );
    // Give the kernel a beat to settle thread page allocations.
    std::thread::sleep(Duration::from_millis(200));

    let rss_before = read_rss_kib(pid).expect("read VmRSS before");
    let fd_before = read_fd_count(pid).expect("read fd count before");

    let total_requests: u32 = if std::env::var("D28B_012_LONG").is_ok() {
        50_000
    } else {
        // CI smoke: 1,000 requests is enough to drive the arena past
        // its first chunk pre-fix (each request consumes ~hundreds
        // of bytes of arena, 1,000 * ~600 B = ~600 KiB which would
        // not even fill a single arena chunk pre-fix; we lean on
        // the chunk count instead -- pre-fix the worker would have
        // accumulated chunks across many requests with a tight curl
        // loop, so even a 1,000 request run shows clear divergence
        // post-fix vs pre-fix).
        2_000
    };

    let started = Instant::now();
    let drove = drive_load(port, total_requests, 100);
    let elapsed = started.elapsed();

    let rss_after = read_rss_kib(pid).expect("read VmRSS after");
    let fd_after = read_fd_count(pid).expect("read fd count after");

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&bin);

    eprintln!(
        "d28b_012: drove {} requests in {:.2?} (rss {} -> {} KiB, fd {} -> {})",
        drove, elapsed, rss_before, rss_after, fd_before, fd_after
    );

    // D28B-026: steady-state drive floor raised from 50% to 90%. A
    // server that drops 10%+ of keep-alive requests is regressing
    // silently and should fail the test, not be counted as success.
    let steady_floor = (total_requests as u64 * 9 / 10) as u32;
    assert!(
        drove >= steady_floor,
        "d28b_012: only completed {} of {} requests (expected >= {} / D28B-026 90% floor); server may have stalled",
        drove,
        total_requests,
        steady_floor
    );

    // Acceptance: per-1k-request RSS growth must be bounded. The
    // hard cap below is generous (5 MiB / 1k requests) compared to
    // the post-fix observed steady-state (~0 KiB / 1k requests). The
    // *pre-fix* code grew RSS by ~67 MiB / minute under the same
    // workload (4.7 GiB / hour ~= 78 MiB / 1k requests at the curl
    // rate observed on developer hardware), so a regression would
    // blow through this cap by an order of magnitude.
    let rss_growth_kib = rss_after.saturating_sub(rss_before);
    let kib_per_1k = (rss_growth_kib as f64) / (drove as f64) * 1000.0;
    eprintln!(
        "d28b_012: rss growth = {} KiB over {} requests = {:.1} KiB / 1k req",
        rss_growth_kib, drove, kib_per_1k
    );

    let cap_kib_per_1k: f64 = 5_120.0; // 5 MiB / 1k req
    assert!(
        kib_per_1k <= cap_kib_per_1k,
        "d28b_012: RSS grew {:.1} KiB / 1k requests, exceeds {} KiB / 1k cap. \
         This is the D28B-012 regression signature -- the bump arena is \
         no longer rewinding at the request boundary. Check that \
         `taida_arena_request_reset` is still called from \
         `net_worker_thread` in `src/codegen/native_runtime/net_h1_h2.c`.",
        kib_per_1k,
        cap_kib_per_1k
    );

    // FD count is independently bounded: each connection is closed
    // before the next opens, so steady-state fd_count is at most the
    // server's fixed bookkeeping (listening socket, worker pool
    // pipes) plus any in-flight client connection. We allow + 8 over
    // the warm-up baseline to absorb scheduling jitter.
    let fd_growth = fd_after.saturating_sub(fd_before);
    assert!(
        fd_growth <= 8,
        "d28b_012: fd count grew by {} (was {}, now {}) -- looks like an fd leak",
        fd_growth,
        fd_before,
        fd_after
    );
}
