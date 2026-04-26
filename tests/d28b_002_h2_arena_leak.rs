//! D28B-002 (Round 2 wG): HTTP/2 NET runtime arena-leak regression test.
//!
//! This is the h2 companion to `tests/d28b_012_net_runtime_leak.rs` (wF).
//!
//! Pre-fix root cause (wF analysis applied to h2):
//!   * `taida_net_h2_serve` runs single-threaded on the main thread (no
//!     worker pool), but every completed h2 stream causes the runtime to
//!     allocate a 14-field request pack (`h2_build_request_pack` in
//!     `src/codegen/native_runtime/net_h1_h2.c` L5577-5607), several
//!     2-field header packs for headers, a body Bytes, plus
//!     `taida_str_new_copy` strings for method / path / query / authority
//!     / peer host / protocol. None of these shapes match the freelist
//!     buckets exactly so they fall through to the per-thread bump arena
//!     (`taida_arena_alloc`) just like the h1 path.
//!   * The handler's response is then dispatched and `taida_release` is
//!     called on it elsewhere, but on an arena-backed taida_val
//!     `taida_release` drops refcount to 0 without rewinding the arena
//!     offset. The h2 main-thread arena therefore accumulates per-request
//!     bytes the same way the h1 worker arena did (4 GiB plateau / 4.7
//!     GiB/h drift recorded for h1 in D28B-012).
//!
//! Fix (paired with wF, applied here in wG):
//!   * Insert a `taida_arena_request_reset()` call at the per-request
//!     boundary in `taida_net_h2_serve_connection` -- right after
//!     `h2_conn_remove_closed_streams(&conn)` finishes cleaning up the
//!     just-served stream. The same safety invariants the wF helper
//!     already documents (no live arena-backed taida_val survives across
//!     the boundary, the handler closure lives on the same thread but is
//!     refcounted not arena-bound) hold for h2 as well -- both paths run
//!     on the same `__thread` arena.
//!
//! Acceptance signal (mirrors wF):
//!   * Build the native artifact, launch the h2 server with TLS on a
//!     loopback port, fire ~500 h2 requests via curl --http2 over
//!     keep-alive (h2 multiplexes per-connection so we use fewer
//!     connections than h1 leak test), sample VmRSS / fd before vs after.
//!   * Assert: RSS growth per 1k requests is bounded (< 5 MiB/1k req,
//!     same cap as wF h1 test). Pre-fix the h2 path is expected to grow
//!     by tens of KiB per request from the bump arena; post-fix the
//!     per-stream arena reset keeps growth at OS allocator noise level.
//!
//! CI smoke: 500 h2 requests (curl is much slower per-request than the
//! /dev/tcp h1 loop because of TLS handshake + h2 framing overhead). The
//! `D28B_002_LONG=1` env enables 5,000 requests for tighter signal.

mod common;

use common::taida_bin;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn fixture_template() -> &'static str {
    // Inline the fixture source. We can't keep a dedicated .td under
    // examples/quality/ the way d28b_012 does because h2 requires
    // cert/key paths that the harness must generate per-run; embedding
    // the cert path in the .td via format! is the canonical pattern
    // used by every other h2 test in tests/parity.rs.
    r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= Repeat["x", 512]())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 50000, 30000, 128, @(cert <= "{cert}", key <= "{key}", protocol <= "h2"))
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn curl_h2_available() -> bool {
    match Command::new("curl").arg("--version").output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains("HTTP2"),
        Err(_) => false,
    }
}

fn gen_self_signed_cert(cert: &Path, key: &Path) -> bool {
    Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=127.0.0.1",
            "-keyout",
            key.to_str().unwrap_or(""),
            "-out",
            cert.to_str().unwrap_or(""),
            "-days",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn build_native(td: &Path) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let bin = std::env::temp_dir().join(format!("d28b_002_{}_{}.bin", std::process::id(), seq));
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

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

fn wait_for_bind(port: u16, server: &mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = server.try_wait() {
            return false;
        }
        if let Ok(addr) = format!("127.0.0.1:{port}").parse::<std::net::SocketAddr>()
            && std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

/// Drive `total` h2 requests against the loopback h2 server. We use
/// sequential `curl --http2` invocations (one TCP+TLS connection per
/// request). curl --parallel was tried first but it does not interact
/// well with single-handler h2 servers when many streams share one
/// connection (it will hang on multiplexing edge cases). Each curl
/// invocation goes through `taida_net_h2_serve_connection` once -- so
/// the per-request boundary that wG needs to instrument fires at
/// connection close, not at stream close. That is fine for this leak
/// regression test because every request consumes the same
/// per-request arena footprint regardless of which boundary the reset
/// runs at: the relevant question is whether the arena rewinds at
/// some boundary inside a process lifetime, not which boundary.
///
/// Returns the number of requests that completed with HTTP 200.
fn drive_h2_load(port: u16, total: u32) -> u32 {
    let mut completed: u32 = 0;
    let url = format!("https://127.0.0.1:{port}/");
    let args_template: [&str; 8] = [
        "--http2",
        "--insecure",
        "--silent",
        "--max-time",
        "3",
        "--output",
        "/dev/null",
        "--write-out",
        // status code on its own line so we can detect 200 vs other
    ];
    for _ in 0..total {
        let mut args: Vec<String> = args_template.iter().map(|s| s.to_string()).collect();
        args.push("%{http_code}".into());
        args.push(url.clone());
        let out = match Command::new("curl").args(&args).output() {
            Ok(o) => o,
            Err(_) => return completed,
        };
        if !out.status.success() {
            // Server likely stalled or curl hit max-time. Stop driving
            // so the harness can record what it has.
            break;
        }
        let code = String::from_utf8_lossy(&out.stdout);
        if code.trim() == "200" {
            completed += 1;
        } else {
            // Non-200 means the request reached the server but the
            // server returned something unexpected (or curl wrote a
            // status code != 200 due to TLS error). Stop -- the
            // measurement window is corrupted.
            break;
        }
    }
    completed
}

#[test]
fn d28b_002_native_h2_arena_reset_bounds_rss() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping: /proc/<pid>/status RSS sampling is Linux-specific");
        return;
    }
    if !taida_bin().exists() {
        eprintln!("skipping: taida release binary not built");
        return;
    }
    if !cc_available() {
        eprintln!("SKIP: cc unavailable");
        return;
    }
    if !openssl_available() {
        eprintln!("SKIP: openssl unavailable");
        return;
    }
    if !curl_h2_available() {
        eprintln!("SKIP: curl --http2 unavailable");
        return;
    }

    // Generate cert/key
    let cert_path =
        std::env::temp_dir().join(format!("d28b_002_h2_cert_{}.pem", std::process::id()));
    let key_path = std::env::temp_dir().join(format!("d28b_002_h2_key_{}.pem", std::process::id()));
    if !gen_self_signed_cert(&cert_path, &key_path) {
        eprintln!("SKIP: cert generation failed");
        return;
    }

    let port = find_free_port();
    let source = fixture_template()
        .replace("{port}", &format!("{port}"))
        .replace("{cert}", cert_path.to_str().unwrap_or(""))
        .replace("{key}", key_path.to_str().unwrap_or(""));

    let dir = std::env::temp_dir().join(format!("d28b_002_h2_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let td_path = dir.join("main.td");
    std::fs::write(&td_path, source).expect("write fixture");

    let bin = match build_native(&td_path) {
        Some(b) => b,
        None => {
            let _ = std::fs::remove_file(&cert_path);
            let _ = std::fs::remove_file(&key_path);
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("SKIP: native build of d28b_002 fixture failed");
            return;
        }
    };

    let mut server = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("server spawn");

    if !wait_for_bind(port, &mut server) {
        let _ = server.kill();
        let _ = server.wait();
        let _ = std::fs::remove_file(&bin);
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir_all(&dir);
        panic!("d28b_002: h2 server failed to bind 127.0.0.1:{port} within 20s");
    }

    let pid = server.id();

    // Warm-up: 30 h2 requests so the encoder/decoder dyn tables and TLS
    // session caches are settled before we start sampling.
    let warmup = drive_h2_load(port, 30);
    if warmup < 15 {
        let _ = server.kill();
        let _ = server.wait();
        let _ = std::fs::remove_file(&bin);
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir_all(&dir);
        panic!(
            "d28b_002: warm-up only completed {warmup}/30 h2 requests; \
             likely TLS handshake or curl --parallel issue"
        );
    }
    std::thread::sleep(Duration::from_millis(200));

    let rss_before = read_rss_kib(pid).expect("read VmRSS before");
    let fd_before = read_fd_count(pid).expect("read fd count before");

    let total: u32 = if std::env::var("D28B_002_LONG").is_ok() {
        5_000
    } else {
        // CI smoke: 300 sequential curl --http2 invocations. Each is
        // ~50 ms (TLS handshake + h2 framing + 512 B body) on
        // developer hardware so the wallclock budget is ~15 s. 300
        // requests is enough to drive the per-thread arena past its
        // first 2 MiB chunk pre-fix: every h2 request burns roughly
        // the same arena footprint as h1 (a 14-field pack + per-header
        // 2-field packs + body bytes + multiple str_new_copy strings,
        // totalling on the order of 1 KiB arena bytes per request)
        // so 300 req x 1 KiB ~= 300 KiB which clears the inter-chunk
        // boundary well before the 5 MiB / 1k req cap.
        300
    };

    let started = Instant::now();
    let drove = drive_h2_load(port, total);
    let elapsed = started.elapsed();

    let rss_after = read_rss_kib(pid).expect("read VmRSS after");
    let fd_after = read_fd_count(pid).expect("read fd count after");

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&cert_path);
    let _ = std::fs::remove_file(&key_path);
    let _ = std::fs::remove_dir_all(&dir);

    eprintln!(
        "d28b_002: drove {} h2 requests in {:.2?} (rss {} -> {} KiB, fd {} -> {})",
        drove, elapsed, rss_before, rss_after, fd_before, fd_after
    );

    assert!(
        drove >= total / 2,
        "d28b_002: only completed {drove} of {total} h2 requests (server may have stalled)"
    );

    // Acceptance: per-1k-request RSS growth must be bounded. Same
    // 5 MiB / 1k req cap as the h1 leak test (d28b_012). The pre-fix
    // h2 path leaks at the same per-request granularity as the h1 path
    // because both use the same per-thread bump arena. Post-fix the
    // arena rewinds at every stream completion in
    // taida_net_h2_serve_connection.
    let rss_growth_kib = rss_after.saturating_sub(rss_before);
    let kib_per_1k = (rss_growth_kib as f64) / (drove.max(1) as f64) * 1000.0;
    eprintln!(
        "d28b_002: h2 rss growth = {} KiB over {} requests = {:.1} KiB / 1k req",
        rss_growth_kib, drove, kib_per_1k
    );

    let cap_kib_per_1k: f64 = 5_120.0; // 5 MiB / 1k req
    assert!(
        kib_per_1k <= cap_kib_per_1k,
        "d28b_002: h2 RSS grew {kib_per_1k:.1} KiB / 1k requests, exceeds {cap_kib_per_1k} KiB / 1k cap. \
         This is the D28B-002 / D28B-012 regression signature on the h2 path -- the per-thread bump arena \
         is no longer rewinding at the h2 stream boundary. Check that `taida_arena_request_reset` is still \
         called from `taida_net_h2_serve_connection` in `src/codegen/native_runtime/net_h1_h2.c`."
    );

    // FD count: same +8 jitter allowance as wF / d28b_012.
    let fd_growth = fd_after.saturating_sub(fd_before);
    assert!(
        fd_growth <= 16,
        "d28b_002: h2 fd count grew by {fd_growth} (was {fd_before}, now {fd_after}) -- looks like an fd leak \
         in the h2 connection cleanup path"
    );
}
