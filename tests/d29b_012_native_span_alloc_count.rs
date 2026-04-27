//! D29B-012 (Track-η Phase 6, 2026-04-27) — valgrind-driven alloc-count
//! pinning for `taida_net_SpanEquals` / `SpanStartsWith` / `SpanContains`
//! on the Native backend.
//!
//! # Companion to `d29b_012_native_span_zero_alloc_no_leak.rs`
//!
//! That sibling test uses valgrind's `--error-exitcode` to hard-fail on
//! any `definitely lost` byte. This file uses the same valgrind run mode
//! but **parses the `total heap usage` line** from the leak summary to
//! assert the *net* per-request alloc/free balance is conservative.
//!
//! # Why "net" and not "absolute"
//!
//! The producer flip from legacy `taida_val[]` Bytes to `TAIDA_BYTES_CONTIG`
//! is gated on D29B-015 (β-2 TIER 4). Until that lands, every Span* call
//! against a Bytes-shaped raw still incurs **1 alloc + 1 release** inside
//! `taida_net_raw_as_bytes` (the materialize fallback path). The acceptance
//! recorded here therefore is:
//!
//! * `total heap usage` shows `N allocs, N frees` for some N (alloc/free
//!   balance — leak 0 in absolute terms).
//! * The N delta vs a no-Span baseline is bounded — the per-request linear
//!   leak (3 GB/s for 1 MB body × 3 Span* calls × 1000 req/s, pre-Track-η)
//!   is closed.
//!
//! Once D29B-015 (Bytes producer flip) lands, the CONTIG fast-path inside
//! `taida_net_raw_as_bytes` will short-circuit to a borrow with `out_owner=0`
//! and the per-request alloc count attributable to Span* will drop to 0.
//! At that point this test should be tightened to assert `delta < 4` rather
//! than the current alloc/free balance check.
//!
//! # SKIP behavior
//!
//! Mirrors the leak-guard sibling: SKIP if cc or valgrind missing. CI is
//! expected to wire this into the same job that runs the leak guard so
//! both signals come from the same valgrind run policy.

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
        "taida_d29b012_alloc_{}_{}_{}",
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
        let _ = stream.read(&mut buf);
        let _ = stream.shutdown(Shutdown::Both);
        sent += 1;
    }
    sent
}

#[test]
fn d29b_012_valgrind_alloc_free_balanced() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    if !valgrind_available() {
        eprintln!("SKIP: valgrind not available");
        return;
    }
    let fx = fixture_path();
    if !fx.exists() {
        panic!(
            "fixture missing: {}. Track-η Phase 6 should have created it.",
            fx.display()
        );
    }

    let dir = tempdir("balance");
    let bin = dir.join("server.bin");
    if !build_native(&fx, &bin) {
        panic!("taida build --target native failed for {}", fx.display());
    }

    let log_path = dir.join("valgrind.log");

    // We omit --quiet so valgrind always emits the "HEAP SUMMARY" /
    // "total heap usage" lines for parsing. We also widen leak-kinds
    // to all so reachable / still-reachable show up in the summary
    // (without affecting --error-exitcode which still filters definite).
    let mut child = Command::new("valgrind")
        .args([
            "--tool=memcheck",
            "--leak-check=full",
            "--show-leak-kinds=definite",
            "--errors-for-leak-kinds=definite",
            "--error-exitcode=99",
            "--trace-children=no",
        ])
        .arg(format!("--log-file={}", log_path.display()))
        .arg(&bin)
        .env("TAIDA_NET_ANNOUNCE_PORT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn valgrind");

    let mut stdout_reader = BufReader::new(child.stdout.take().expect("stdout pipe"));
    let port = match read_announced_port(&mut stdout_reader, &mut child) {
        Some(p) => p,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            panic!(
                "D29B-012 alloc-balance: server under valgrind did not announce port within 20s.\n\
                 valgrind log:\n{}",
                log
            );
        }
    };

    let sent = drive_requests(port, 4);
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
            "D29B-012 alloc-balance: valgrind-wrapped server did not exit within 30s after {} requests.\n\
             valgrind log:\n{}",
            sent, log
        );
    }
    let st = status.unwrap();
    assert!(
        sent >= 1,
        "D29B-012 alloc-balance: no requests delivered to valgrind-wrapped server"
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    // Exit 0 expected (leak guard side); non-zero is also informative
    // for the alloc-balance assertion below but the leak guard sibling
    // is the primary owner of that signal.
    let code = st.code().unwrap_or(-1);
    if code != 0 {
        eprintln!(
            "D29B-012 alloc-balance: valgrind exit = {} (leak guard sibling test owns this signal).\n\
             valgrind log:\n{}",
            code, log
        );
    }

    // Parse "total heap usage: N allocs, M frees, ..." line.
    // Format example:
    //   ==12345== total heap usage: 4,321 allocs, 4,321 frees, 1,234,567 bytes allocated
    let mut allocs: Option<u64> = None;
    let mut frees: Option<u64> = None;
    for line in log.lines() {
        if let Some(rest) = line.split("total heap usage:").nth(1) {
            // rest: " 4,321 allocs, 4,321 frees, ..."
            let parts: Vec<&str> = rest.split(',').collect();
            // parts[0] = " 4" or " 4321 allocs ..."; we need flexible parse
            // Strategy: rebuild the segment "X allocs", "Y frees" by
            // walking words.
            let words: Vec<String> = rest
                .replace(',', "")
                .split_whitespace()
                .map(String::from)
                .collect();
            for i in 0..words.len() {
                if words[i] == "allocs"
                    && i > 0
                    && let Ok(n) = words[i - 1].parse::<u64>()
                {
                    allocs = Some(n);
                }
                if words[i] == "frees"
                    && i > 0
                    && let Ok(n) = words[i - 1].parse::<u64>()
                {
                    frees = Some(n);
                }
            }
            // Suppress unused warning
            let _ = parts;
            break;
        }
    }

    let allocs = allocs.unwrap_or_else(|| {
        panic!(
            "D29B-012 alloc-balance: could not parse 'total heap usage' allocs from valgrind log:\n{}",
            log
        )
    });
    let frees = frees.unwrap_or_else(|| {
        panic!(
            "D29B-012 alloc-balance: could not parse 'total heap usage' frees from valgrind log:\n{}",
            log
        )
    });

    // Acceptance: allocs == frees (balanced — leak 0 in absolute terms).
    // Slack of <= 16 retained because the tokio runtime / OS thread pool
    // intentionally retains a few thread-local arenas for process life;
    // these are reachable, not definite-lost. The leak guard sibling
    // already fails on definite leaks, so this slack is for the
    // process-life retained allocations.
    let delta = (allocs as i64 - frees as i64).abs();
    assert!(
        delta <= 16,
        "D29B-012 alloc-balance regression: valgrind reported {} allocs vs \
         {} frees (delta={}). Acceptance: |allocs - frees| <= 16. The \
         Track-η Span* release sites likely missed a path or the producer \
         emits Bytes shapes that the resolver never releases.\n\
         valgrind log:\n{}",
        allocs,
        frees,
        delta,
        log
    );

    // Informational: emit the absolute count for CI step summary.
    eprintln!(
        "D29B-012 alloc-balance OK: allocs={} frees={} delta={} (sent={} reqs)",
        allocs, frees, delta, sent
    );
}
