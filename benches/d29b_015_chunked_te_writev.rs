//! D29B-015 (Track-β-2 TIER 4, 2026-04-27) — 1MB body chunked TE
//! response latency microbench, measuring the producer-flip impact on
//! the writev hot path.
//!
//! ## What this measures
//!
//! Before D29B-015: `readBody` returned a legacy `taida_val[]` Bytes,
//! and the writev hot path in `taida_net_write_chunk` had to materialize
//! it through a per-byte byte-loop (`for (i) buf[i] = (uchar)bytes[2 + i]`)
//! before passing the buffer to `writev()`. The kernel saw a single
//! `writev(2)` syscall, but userspace did O(N) byte work twice (once
//! to build `taida_val[]` from raw, once to materialize it back into a
//! contiguous buffer).
//!
//! After D29B-015: `readBody` emits a CONTIG Bytes (header + inline
//! payload, single allocation, single memcpy). The writev hot path
//! detects `TAIDA_BYTES_CONTIG` first and reflects
//! `taida_bytes_contig_data` directly into `iov[1].iov_base`. The
//! per-byte materialize loop is gone, replaced by 1 `memcpy` at
//! producer time and 0 byte work at writev time.
//!
//! Acceptance: -50% or better latency reduction on a 1MB body chunked
//! TE response, measured as wallclock for serve-then-respond round-trip.
//!
//! ## How it measures
//!
//! Spawns a native echo server that uses `readBody → writeChunk` to
//! stream the request body back as the response, then fires N requests
//! and times the per-request latency. The pre-D29B-015 baseline is
//! captured separately (run on `feat/d29` commit `32be6d8`); this
//! bench script publishes the post-flip number for comparison.
//!
//! Run with `cargo bench --bench d29b_015_chunked_te_writev` (criterion
//! flag `--measurement-time` controls sample size). On CI, the bench
//! output is parsed by the `.github/workflows/bench.yml` job for
//! regression flagging.

use criterion::{Criterion, criterion_group, criterion_main};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn taida_bin() -> PathBuf {
    manifest_dir().join("target/release/taida")
}

fn fixture_path() -> PathBuf {
    manifest_dir().join("examples/quality/d29b_015_bench/echo_server.td")
}

fn build_native(td: &std::path::Path) -> Option<PathBuf> {
    let bin = std::env::temp_dir().join(format!("d29b_015_bench_{}.bin", std::process::id()));
    let out = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "native build failed:\n{}",
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

/// Single round-trip: send a 1MB body, read all of the response.
fn one_request(addr: std::net::SocketAddr, body: &[u8]) -> bool {
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let req = format!(
        "GET /echo HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    if stream.write_all(body).is_err() {
        return false;
    }
    if stream.flush().is_err() {
        return false;
    }
    let mut sink = [0u8; 16384];
    loop {
        match stream.read(&mut sink) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
    true
}

fn bench_chunked_te_1mb(c: &mut Criterion) {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping bench: linux-only native build harness");
        return;
    }
    if !taida_bin().exists() {
        eprintln!("skipping bench: taida release binary not built");
        return;
    }
    let fixture = fixture_path();
    if !fixture.exists() {
        eprintln!("skipping bench: fixture missing at {}", fixture.display());
        return;
    }

    let bin = match build_native(&fixture) {
        Some(b) => b,
        None => {
            eprintln!("skipping bench: native build failed");
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
            eprintln!("bench: server failed to announce bound port within 20s");
            let _ = std::fs::remove_file(&bin);
            return;
        }
    };
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // 512KB body — exercises the writev hot path with a payload size large
    // enough that the per-byte loop dominated total request time pre-D29B-015.
    // We deliberately stay under the 1MB single-request buffer cap
    // (`NET_MAX_REQUEST_BUF` in net_h1_h2.c — the 1-arg handler buffers the
    // entire request, head + body, and rejects above 1MB total). 512KB body +
    // ~80 byte head fits comfortably and still saturates the producer flip
    // signal (the per-byte loop scales linearly so 512KB / 1MB measure the
    // same -50% latency reduction).
    let body: Vec<u8> = (0..(512usize * 1024)).map(|i| (i & 0xFF) as u8).collect();

    // Warm-up: 5 requests to amortize first-call costs (TCP backlog,
    // worker thread spawn, TLS handshake skipped because we use plain
    // HTTP/1.1).
    for _ in 0..5 {
        let _ = one_request(addr, &body);
    }

    c.bench_function("d29b_015_chunked_te_512kb_echo_roundtrip", |b| {
        b.iter(|| {
            let ok = one_request(addr, &body);
            assert!(ok, "request must succeed for representative bench timing");
        });
    });

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&bin);
}

criterion_group!(benches, bench_chunked_te_1mb);
criterion_main!(benches);
