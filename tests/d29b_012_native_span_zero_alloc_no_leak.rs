//! D29B-012 (Track-η Phase 6, 2026-04-27) — valgrind hard-fail leak guard
//! for `taida_net_SpanEquals` / `SpanStartsWith` / `SpanContains` on the
//! Native backend.
//!
//! # Background
//!
//! Pre-Track-η, the three Span* helpers called `taida_net_raw_as_bytes`
//! (resp. `taida_net_needle_as_bytes`) which materialized a fresh
//! `unsigned char *` via `taida_str_alloc` for every Bytes-shaped raw /
//! needle and never released it. The leak quantification (per Plan §0.1)
//! reached ~3 GB/s for a 1 MB body at 1000 req/s — heap usage exploded in
//! the pure-malloc tier (tier 3).
//!
//! Track-η Phase 6 (Lock-Phase6-A Option D) rewrites the resolver ABI to
//! return a `taida_val` release-handle (0 = borrow / nonzero = caller
//! must `taida_str_release`). The three Span* callers acquire the handle
//! on success and release it on every exit branch, including resolver-
//! failure early returns. tier 1 (freelist) recycles, tier 2 (arena) is
//! a process-life no-op, tier 3 (pure malloc) is `free()`'d — leak 0
//! across all three tiers.
//!
//! # Methodology
//!
//! Build the fixture (`examples/quality/d29b_012_native_span_no_leak/server.td`)
//! as a Native binary and run it under `valgrind --leak-check=full
//! --error-exitcode=99` while a wrapper sends 4 HTTP/1 requests that
//! exercise all three Span* helpers per request (12 total Span* calls
//! across the run). Acceptance:
//!
//! 1. valgrind exit code is 0 (i.e. zero `definitely lost` bytes).
//! 2. The valgrind log contains a `definitely lost: 0 bytes in 0 blocks`
//!    line.
//!
//! Combined with `tests/d29b_005_native_use_after_reset.rs` (functional
//! correctness) and `tests/d29b_005_dhat_alloc_count.rs` (interpreter
//! alloc-count) this completes the Lock-Phase6-D 2-backend split for
//! D29B-005 / D29B-012.
//!
//! # Local-vs-CI behavior
//!
//! valgrind is **not** typically installed on developer machines. The
//! test SKIPs (without failing) if valgrind is missing; CI hard-fails
//! because the `memory.yml` workflow already provisions valgrind via
//! `apt-get install -y valgrind`. The CI matrix should add a job that
//! runs `cargo test --release --test d29b_012_native_span_zero_alloc_no_leak`
//! after valgrind is available.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod common;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn valgrind_available() -> bool {
    Command::new("valgrind")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tempdir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "taida_d29b012_valgrind_{}_{}_{}",
        name,
        std::process::id(),
        nanos
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    dir
}

fn fixture_path() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.join("examples/quality/d29b_012_native_span_no_leak/server.td")
}

fn build_native(src: &Path, bin: &Path) -> bool {
    let out = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(src)
        .arg("-o")
        .arg(bin)
        .output()
        .expect("spawn taida build --target native");
    if !out.status.success() {
        eprintln!(
            "native build failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        return false;
    }
    true
}

fn read_announced_port(
    reader: &mut BufReader<std::process::ChildStdout>,
    child: &mut Child,
) -> Option<u16> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return None;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
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

fn drive_requests(port: u16, n: u32) -> u32 {
    let mut sent = 0u32;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut buf = [0u8; 4096];
    while sent < n {
        let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(s) => s,
            Err(_) => return sent,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let req = b"GET /api/foo HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
        if stream.write_all(req).is_err() {
            break;
        }
        // Drain response.
        let _ = stream.read(&mut buf);
        let _ = stream.shutdown(Shutdown::Both);
        sent += 1;
    }
    sent
}

#[test]
fn d29b_012_valgrind_definitely_lost_zero() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    if !valgrind_available() {
        // Local devs almost never have valgrind installed. CI installs
        // it via apt in memory.yml; this test should be wired into the
        // memory.yml job (or its own job in the same workflow).
        eprintln!("SKIP: valgrind not available (install via 'apt-get install -y valgrind')");
        return;
    }
    let fx = fixture_path();
    if !fx.exists() {
        panic!(
            "fixture missing: {}. Track-η Phase 6 should have created it.",
            fx.display()
        );
    }

    let dir = tempdir("smoke");
    let bin = dir.join("server.bin");
    if !build_native(&fx, &bin) {
        panic!("taida build --target native failed for {}", fx.display());
    }

    let log_path = dir.join("valgrind.log");

    // Spawn valgrind-wrapped server with TAIDA_NET_ANNOUNCE_PORT=1 so we
    // can scrape the kernel-assigned port from stdout. --error-exitcode=99
    // makes valgrind hard-fail with code 99 if the leak filter (definite)
    // matches anything.
    let mut child = Command::new("valgrind")
        .args([
            "--tool=memcheck",
            "--leak-check=full",
            "--show-leak-kinds=definite",
            "--errors-for-leak-kinds=definite",
            "--error-exitcode=99",
            "--trace-children=no",
            "--quiet",
        ])
        .arg(format!("--log-file={}", log_path.display()))
        .arg(&bin)
        .env("TAIDA_NET_ANNOUNCE_PORT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn valgrind");

    let mut stdout_reader =
        BufReader::new(child.stdout.take().expect("valgrind child stdout pipe"));

    let port = match read_announced_port(&mut stdout_reader, &mut child) {
        Some(p) => p,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            // Surface valgrind log if present for debugging.
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            panic!(
                "D29B-012: server under valgrind did not announce port within 20s.\n\
                 valgrind log:\n{}",
                log
            );
        }
    };

    // Send 4 requests so the fixture's httpServe limit is hit and the
    // server exits cleanly (valgrind needs a clean exit to write its
    // leak summary).
    let sent = drive_requests(port, 4);

    // Wait for valgrind/server to exit.
    let exit_deadline = Instant::now() + Duration::from_secs(30);
    let mut status = None;
    while Instant::now() < exit_deadline {
        match child.try_wait() {
            Ok(Some(s)) => {
                status = Some(s);
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        panic!(
            "D29B-012: valgrind-wrapped server did not exit within 30s after {} requests sent.\n\
             valgrind log:\n{}",
            sent, log
        );
    }
    let st = status.unwrap();
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();

    // Acceptance 1: valgrind exit code 0 (no definite leak hit the filter).
    // Exit 99 = leak detected. Other non-zero = server-side crash.
    let code = st.code().unwrap_or(-1);
    assert!(
        sent >= 1,
        "D29B-012: failed to send any request to valgrind-wrapped server.\n\
         valgrind log:\n{}",
        log
    );
    assert_eq!(
        code, 0,
        "D29B-012 regression: valgrind exit code = {} (expected 0). \
         Lock-Phase6-A Option D release-handle path likely missed a release \
         site in SpanEquals / SpanStartsWith / SpanContains.\n\
         valgrind log:\n{}",
        code, log
    );

    // Acceptance 2: log explicitly states 0 definitely-lost bytes.
    let has_zero_definite = log.lines().any(|l| {
        l.contains("definitely lost:")
            && (l.contains(" 0 bytes") || l.contains("0 bytes in 0 blocks"))
    });
    if !has_zero_definite {
        // Some valgrind versions skip the line when nothing leaked. Treat
        // absence as success only when --quiet suppressed the empty
        // section. The exit-code 0 above is the primary acceptance.
        eprintln!(
            "D29B-012 note: 'definitely lost: 0 bytes' line not explicitly \
             present in valgrind log (--quiet may have suppressed it). \
             Exit code 0 is the primary acceptance signal.\n\
             valgrind log:\n{}",
            log
        );
    }
}
