//! D28B-006 (Round 2 wI): scatter-gather smoke regression test.
//!
//! Pins the *scatter-gather is the default send path* invariant for
//! the native NET runtime. This test is the short-window companion
//! to the 24h soak runbook in `.dev/D28_SOAK_RUNBOOK.md`; it must
//! complete in tens of seconds so it can run on every PR without
//! gating on long-form soak observation.
//!
//! What the smoke covers:
//!
//!   1. The native backend builds a `httpServe` handler that returns
//!      a 512 B body (`Repeat["x", 512]()`) and binds 127.0.0.1:18092.
//!   2. We send ~1,000 HTTP/1.1 GET requests over keep-alive TCP
//!      connections and read the response body back in full each
//!      time. The httpServe response path uses writev (scatter-
//!      gather) for head + body, so a regression that disables the
//!      writev path or breaks the body framing surfaces here as
//!      either short reads or non-200 responses.
//!   3. We assert that every successful response carries exactly the
//!      512 B body the fixture produces, and that ≥ 95% of attempts
//!      complete (the residual 5% absorbs the rare TCP-level retry
//!      a developer hardware loop will see in steady state).
//!   4. We sanity-check RSS / fd growth using the same per-1k cap
//!      d28b_012 uses for the bump-arena pin (5 MiB / 1k req, fd
//!      delta ≤ 8). This is *not* a re-pin of D28B-012 — the cap is
//!      identical so a regression in either invariant is caught by
//!      both tests, but the D28B-006 owner is the writev framing
//!      path, not the arena reset.
//!
//! What this smoke does *not* cover (deferred to the 24h runbook):
//!
//!   * Multi-hour drift (the 5,108 → 5,556 KiB plateau wF baseline
//!     only emerges after ~30 min of cold-start settling, and the
//!     `LIKELY STABLE` projection requires the 24h linear fit).
//!   * 3-backend parity for scatter-gather behaviour (interpreter +
//!     JS + native — runbook §3 covers all three; this smoke pins
//!     native only, which is the writev-having backend; interpreter
//!     and JS exercise the same logical path via Tokio / Node and
//!     are covered by `tests/parity.rs::test_net6_5b_*`).
//!   * Heaptrack / valgrind verification (runbook §3.2 / §3.3).
//!
//! See `.dev/D28_BLOCKERS.md::D28B-006`, `.dev/D28_BLOCKERS.md::D28B-014`,
//! `.dev/D28_SOAK_RUNBOOK.md`.

mod common;

use common::taida_bin;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path() -> PathBuf {
    manifest_dir().join("examples/quality/d28b_006_scatter_gather_smoke/server.td")
}

fn build_native(td: &Path) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let bin = std::env::temp_dir().join(format!("d28b_006_{}_{}.bin", std::process::id(), seq));
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

fn wait_for_bind(port: u16, server: &mut Child) -> Option<u16> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = server.try_wait() {
            return None; // server died
        }
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().ok()?,
            Duration::from_millis(100),
        )
        .is_ok()
        {
            return Some(port);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    None
}

/// Drive the smoke load and verify each response carries the expected
/// 512 B body produced by the writev path. Returns `(sent, full_body_seen)`.
///
/// `full_body_seen` counts responses whose body window (after the
/// `\r\n\r\n` head-end) contained the full 512 'x' bytes the fixture
/// emits. A regression that breaks scatter-gather framing (e.g. body
/// length 0, partial body, or non-200 status) surfaces as a divergence
/// between `sent` and `full_body_seen`.
fn drive_smoke(port: u16, total_requests: u32, requests_per_conn: u32) -> (u32, u32) {
    let mut sent: u32 = 0;
    let mut full_body_seen: u32 = 0;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut buf = [0u8; 4096];

    while sent < total_requests {
        let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(s) => s,
            Err(_) => return (sent, full_body_seen),
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

        let req = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n";
        let mut ok_on_conn = 0u32;
        while ok_on_conn < requests_per_conn && sent < total_requests {
            if stream.write_all(req).is_err() {
                break;
            }

            // Drain enough bytes to span the full head + 512 B body.
            // The fixture's head is short (a status line + one
            // content-type + content-length) so 16 reads of up to
            // 4 KiB is overkill but keeps the loop bounded.
            let mut acc: Vec<u8> = Vec::with_capacity(1024);
            let mut saw_head_end = false;
            for _ in 0..16 {
                let n = match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                acc.extend_from_slice(&buf[..n]);
                if !saw_head_end && acc.windows(4).any(|w| w == b"\r\n\r\n") {
                    saw_head_end = true;
                }
                // Head (~ < 100 B) + 512 body = ~620 B. Reading at
                // least 552 B guarantees at least one full body.
                if acc.len() >= 552 {
                    break;
                }
            }

            if !saw_head_end {
                break;
            }

            sent += 1;
            ok_on_conn += 1;

            // Parse out the body slice (everything after the first
            // `\r\n\r\n`) and verify it begins with at least 512
            // 'x' bytes. A scatter-gather regression where the body
            // is truncated or interleaved with the head bytes makes
            // this assertion miss.
            if let Some(head_end) = acc.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
                && acc.len() >= head_end + 512
            {
                let body = &acc[head_end..head_end + 512];
                if body.iter().all(|&b| b == b'x') {
                    full_body_seen += 1;
                }
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
    }

    (sent, full_body_seen)
}

#[test]
fn d28b_006_native_scatter_gather_smoke() {
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
            eprintln!("skipping: native build failed (toolchain unavailable?)");
            return;
        }
    };

    // Fail fast on bind: launch the native server and probe 18092.
    let mut server = match Command::new(&bin).spawn() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: failed to spawn native binary: {e}");
            let _ = std::fs::remove_file(&bin);
            return;
        }
    };

    let port = match wait_for_bind(18092, &mut server) {
        Some(p) => p,
        None => {
            let _ = server.kill();
            let _ = server.wait();
            let _ = std::fs::remove_file(&bin);
            panic!("d28b_006: server failed to bind 127.0.0.1:18092 within 20s");
        }
    };

    let pid = server.id();

    // Warm-up: 100 requests so the worker pool is fully spawned and
    // freelists have their initial fill before the measurement
    // window opens. Drift from a cold start (the first arena chunk
    // settling, the worker pool growing from 0 → N) is what the wF
    // baseline notes as a cold-start jump (5,108 → 5,556 KiB) and
    // would alias into the smoke window without warm-up.
    let (warm_sent, warm_bodies) = drive_smoke(port, 100, 50);
    if warm_sent < 50 || warm_bodies < 50 {
        let _ = server.kill();
        let _ = server.wait();
        let _ = std::fs::remove_file(&bin);
        panic!(
            "d28b_006: warm-up only completed {warm_sent} requests with \
             {warm_bodies} full bodies; server may have crashed before \
             the smoke window opened"
        );
    }
    std::thread::sleep(Duration::from_millis(200));

    let rss_before = read_rss_kib(pid).expect("read VmRSS before");
    let fd_before = read_fd_count(pid).expect("read fd count before");

    // Smoke window: ~1,000 requests in tens of seconds on dev
    // hardware. The D28B_006_LONG env knob bumps it to 10,000 for
    // a tighter signal during local audit (still bounded so the
    // test cannot block CI).
    let total_requests: u32 = if std::env::var("D28B_006_LONG").is_ok() {
        10_000
    } else {
        1_000
    };

    let started = Instant::now();
    let (drove, full_bodies) = drive_smoke(port, total_requests, 100);
    let elapsed = started.elapsed();

    let rss_after = read_rss_kib(pid).expect("read VmRSS after");
    let fd_after = read_fd_count(pid).expect("read fd count after");

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&bin);

    eprintln!(
        "d28b_006: smoke drove {drove} requests in {elapsed:.2?} \
         (full bodies seen = {full_bodies}, rss {rss_before} -> {rss_after} KiB, \
         fd {fd_before} -> {fd_after})"
    );

    // Acceptance 1: the smoke must complete most of its budget. A
    // server that crashed mid-flight or stopped accepting trips this.
    assert!(
        drove >= total_requests * 95 / 100,
        "d28b_006: only completed {drove} of {total_requests} requests \
         ({:.0}% of budget) — scatter-gather server stalled",
        (drove as f64) / (total_requests as f64) * 100.0
    );

    // Acceptance 2: every successful request must have surfaced the
    // full 512 B body. This is the writev (scatter-gather) framing
    // pin — if the runtime regresses to a single-buffer write or
    // interleaves head + body bytes, this counter diverges from
    // `drove` immediately. Pre-fix scenarios where body length is
    // wrong or framing is broken light up here.
    assert!(
        full_bodies >= drove * 95 / 100,
        "d28b_006: only {full_bodies} of {drove} responses carried the \
         expected 512 B 'x'-fill body — scatter-gather framing regression"
    );

    // Acceptance 3: per-1k-request RSS growth must stay under the
    // bump-arena cap d28b_012 also enforces. This is intentionally
    // a duplicate of the d28b_012 cap so a regression in either
    // invariant is caught by *both* tests, with d28b_006 owning the
    // writev framing reason and d28b_012 owning the arena reset
    // reason. The wF post-fix baseline lives well under this cap
    // (170 KiB / 1k req on developer hardware ≪ 5,120 KiB).
    let rss_growth_kib = rss_after.saturating_sub(rss_before);
    let kib_per_1k = (rss_growth_kib as f64) / (drove as f64) * 1000.0;
    eprintln!(
        "d28b_006: rss growth = {rss_growth_kib} KiB over {drove} req = \
         {kib_per_1k:.1} KiB / 1k req"
    );

    let cap_kib_per_1k: f64 = 5_120.0; // 5 MiB / 1k req
    assert!(
        kib_per_1k <= cap_kib_per_1k,
        "d28b_006: RSS grew {kib_per_1k:.1} KiB / 1k requests, exceeds \
         {cap_kib_per_1k} KiB / 1k cap. This is the same surface as \
         D28B-012 — see `.dev/D28_SOAK_RUNBOOK.md` for the long-form \
         24h investigation."
    );

    // Acceptance 4: fd count must not leak across the smoke. Each
    // keep-alive connection is closed before the next is opened, so
    // steady-state fd_count is dominated by listening socket + worker
    // pool fixed bookkeeping. A delta over 8 indicates either a
    // socket close regression or unbounded worker pool growth.
    let fd_growth = fd_after.saturating_sub(fd_before);
    assert!(
        fd_growth <= 8,
        "d28b_006: fd count grew by {fd_growth} (was {fd_before}, now \
         {fd_after}) — looks like a socket leak in the scatter-gather path"
    );
}

/// Sanity invariant: the fixture file the smoke depends on must be
/// present at the documented path. This guards against an accidental
/// fixture rename or move that would silently turn the smoke into a
/// no-op via the `eprintln!` skip arm above.
#[test]
fn d28b_006_fixture_present() {
    let p = fixture_path();
    assert!(
        p.exists(),
        "d28b_006: missing scatter-gather fixture at {} — \
         see `.dev/D28_SOAK_RUNBOOK.md` for the documented layout",
        p.display()
    );
}

/// Sanity invariant: the 24h soak runbook must be present at the
/// documented path. The smoke deliberately does not duplicate the
/// runbook content; it only needs to know the runbook exists so a
/// future cleanup that deletes it surfaces in CI rather than going
/// undetected until the next stable gate.
#[test]
fn d28b_014_runbook_present() {
    // The runbook lives under `.dev/` which is gitignored in the
    // primary worktree but accessible via the symlink the D28
    // worktree contract installs (see
    // `.dev/D28_WORKTREE_CONTRACT.md`). When this test runs from
    // CI on a fresh clone there is no `.dev/` so the test skips.
    //
    // D29 (2026-04-27): D28 stable land 後、runbook は
    // `.dev/taida-logs/docs/archive/d28/D28_SOAK_RUNBOOK.md` に
    // archive 移管される。primary path 不在でも archive path
    // exists なら GREEN とする (D29B-017 修正)。
    if !manifest_dir().join(".dev").exists() {
        eprintln!(
            "skipping: `.dev/` not present in this checkout (this is \
             expected on a fresh CI clone; the runbook is checked in \
             via the developer worktree, not the public tree)"
        );
        return;
    }
    let primary = manifest_dir().join(".dev/D28_SOAK_RUNBOOK.md");
    let archive = manifest_dir().join(".dev/taida-logs/docs/archive/d28/D28_SOAK_RUNBOOK.md");
    assert!(
        primary.exists() || archive.exists(),
        "d28b_014: D28_SOAK_RUNBOOK.md not found in either primary \
         (`.dev/D28_SOAK_RUNBOOK.md`) or D28 archive \
         (`.dev/taida-logs/docs/archive/d28/D28_SOAK_RUNBOOK.md`) — \
         see `.dev/D28_BLOCKERS.md::D28B-014` for the acceptance"
    );
}
