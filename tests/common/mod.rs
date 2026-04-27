//! Shared test utilities for integration tests.
//!
//! RCB-26: Common helpers (`taida_bin`, `wasmtime_bin`) extracted from 9 test files
//! to eliminate copy-paste duplication. Each test crate declares `mod common;` to
//! import these functions.
//!
//! RC-8b: Parity tests save compiled .wasm files to `target/wasm-test-cache/<profile>/`
//! so superset tests can reuse them without recompiling.
//!
//! The cache is a best-effort optimization that does not affect test correctness.
//! Tests never rely on cache ordering or presence -- a cache miss simply triggers
//! recompilation. Test execution order does not matter.

// Not all test crates use every function in this module.
#![allow(dead_code)]

pub mod fixture_lists;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, Once, OnceLock};

// ---------------------------------------------------------------------------
// Binary discovery helpers (RCB-26)
// ---------------------------------------------------------------------------

/// Get the path to the built `taida` binary.
///
/// Runtime lookup only. This intentionally avoids `env!("CARGO_BIN_EXE_taida")`
/// because nextest archive execution may run the test binary on a different
/// machine than the one that compiled it. In that mode the compile-time
/// absolute path baked into `env!` points at the archive-build runner and
/// fails with `ENOENT`, even though the archive itself contains the `taida`
/// host binary (`target/debug/taida`).
///
/// Search order:
///
/// 1. `TAIDA_BIN` runtime env override (used by ad-hoc local runs)
/// 2. Canonical `<manifest>/target/release/taida` (or `target/debug/taida`)
///    — chosen ahead of `CARGO_BIN_EXE_taida` because cargo can cache
///    multiple bin fingerprints under `target/<profile>/deps/taida-XXXX`
///    and silently point `CARGO_BIN_EXE_taida` at a stale one when test
///    builds race with bin builds (observed on `feat/c27` Round 1 fix
///    verification: a fresh `cargo build --release --bin taida` updated
///    the canonical path, but a subsequent `cargo test --release` ran
///    against a previously-cached bin via `CARGO_BIN_EXE_taida` and
///    re-linked `target/release/taida` to the older fingerprint).
/// 3. `CARGO_BIN_EXE_taida` runtime env (last-resort fallback)
/// 4. Relative candidates from the current working directory, current test
///    binary location, and the compile-time manifest dir:
///    - `<root>/taida`
///    - `<root>/debug/taida`
///    - `<root>/release/taida`
///    - `<root>/target/debug/taida`
///    - `<root>/target/release/taida`
pub fn taida_bin() -> PathBuf {
    #[cfg(windows)]
    const BIN_NAME: &str = "taida.exe";
    #[cfg(not(windows))]
    const BIN_NAME: &str = "taida";

    if let Some(path) = std::env::var_os("TAIDA_BIN").map(PathBuf::from)
        && path.exists()
    {
        return path;
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for canonical in [
        manifest.join("target").join("release").join(BIN_NAME),
        manifest.join("target").join("debug").join(BIN_NAME),
    ] {
        if canonical.exists() {
            return canonical;
        }
    }

    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_taida").map(PathBuf::from)
        && path.exists()
    {
        return path;
    }

    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        for ancestor in parent.ancestors() {
            roots.push(ancestor.to_path_buf());
        }
    }
    roots.push(manifest.clone());

    let mut candidates = Vec::new();
    for root in roots {
        for candidate in [
            root.join(BIN_NAME),
            root.join("debug").join(BIN_NAME),
            root.join("release").join(BIN_NAME),
            root.join("target").join("debug").join(BIN_NAME),
            root.join("target").join("release").join(BIN_NAME),
        ] {
            if !candidates.iter().any(|p: &PathBuf| p == &candidate) {
                candidates.push(candidate);
            }
        }
    }

    if let Some(path) = candidates.iter().find(|p| p.exists()) {
        return path.clone();
    }

    panic!(
        "could not locate taida binary; searched:\n{}",
        candidates
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Find the `wasmtime` binary for running compiled `.wasm` files.
///
/// Checks `$HOME/.wasmtime/bin/wasmtime` first, then falls back to `which wasmtime`.
/// Returns `None` if wasmtime is not installed.
pub fn wasmtime_bin() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home).join(".wasmtime/bin/wasmtime");
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("which").arg("wasmtime").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// Run a `.td` file with the interpreter and return its stdout.
///
/// Returns `None` if the interpreter exits with a non-zero status.
/// On failure, prints stderr to aid debugging.
///
/// Note: `parity.rs` and `crash_regression.rs` maintain their own local versions
/// because they require different semantics (per-line `normalize()` and `Result`
/// return type, respectively).
pub fn run_interpreter(td_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin()).arg(td_path).output().ok()?;
    if !output.status.success() {
        eprintln!(
            "run_interpreter failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// Output normalization (RCB-26)
// ---------------------------------------------------------------------------

/// Normalize output by trimming trailing whitespace on every line.
///
/// Used by parity and crash-regression tests to tolerate minor whitespace
/// differences between backends.
pub fn normalize(s: &str) -> String {
    s.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Like `run_interpreter`, but applies per-line `normalize()` to the output.
///
/// Preferred by parity tests where backends may differ in trailing whitespace.
pub fn run_interpreter_normalized(td_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin()).arg(td_path).output().ok()?;
    if !output.status.success() {
        eprintln!(
            "run_interpreter_normalized failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stdout)))
}

// ---------------------------------------------------------------------------
// Temp dir / file helpers (RCB-29)
// ---------------------------------------------------------------------------

/// Create a unique temporary directory with the given prefix.
///
/// The directory name includes the process ID and nanosecond timestamp to avoid
/// collisions when tests run in parallel.
pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

/// Write string content to a file, panicking on failure.
pub fn write_file(path: &Path, content: &str) {
    std::fs::write(path, content).expect("failed to write file");
}

/// Check whether `node` is available on the system PATH.
pub fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// WASM test cache (RC-8b)
// ---------------------------------------------------------------------------

/// N-1: Per-profile OnceLock to ensure `create_dir_all` runs at most once per profile.
/// We use a fixed set of known profiles instead of a dynamic map.
static CACHE_DIR_MIN: OnceLock<PathBuf> = OnceLock::new();
static CACHE_DIR_WASI: OnceLock<PathBuf> = OnceLock::new();
static CACHE_DIR_FULL: OnceLock<PathBuf> = OnceLock::new();

/// RC-8b: Directory for caching compiled .wasm files between parity and superset tests.
/// Parity tests write here; superset tests read first, falling back to recompilation on miss.
///
/// N-1: Uses `OnceLock` so `create_dir_all` is called at most once per profile per process.
pub fn wasm_test_cache_dir(profile: &str) -> PathBuf {
    let lock = match profile {
        "wasm-min" => &CACHE_DIR_MIN,
        "wasm-wasi" => &CACHE_DIR_WASI,
        "wasm-full" => &CACHE_DIR_FULL,
        _ => {
            // Unknown profile: fall back to creating every time (no OnceLock).
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("wasm-test-cache")
                .join(profile);
            let _ = std::fs::create_dir_all(&dir);
            return dir;
        }
    };

    lock.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("wasm-test-cache")
            .join(profile);
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
    .clone()
}

/// RC-8b: Save a compiled .wasm file to the test cache for reuse by superset tests.
///
/// N-3: If `rename` fails (e.g. cross-device move), the `.wasm.tmp` file is cleaned up
/// to prevent temporary file leaks.
pub fn cache_wasm(profile: &str, stem: &str, wasm_path: &Path) {
    let cache_path = wasm_test_cache_dir(profile).join(format!("{}.wasm", stem));
    // Atomic: write to .tmp then rename to avoid partial reads by concurrent tests.
    let tmp_path = cache_path.with_extension("wasm.tmp");
    if std::fs::copy(wasm_path, &tmp_path).is_ok()
        && std::fs::rename(&tmp_path, &cache_path).is_err()
    {
        // N-3: Clean up the .tmp file on rename failure.
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// RCB-55: Taida binary mtime, cached once per process via OnceLock.
/// Used by `cached_wasm` to invalidate cache when the compiler is rebuilt.
static BIN_MTIME: OnceLock<Option<std::time::SystemTime>> = OnceLock::new();

fn taida_bin_mtime() -> Option<std::time::SystemTime> {
    *BIN_MTIME.get_or_init(|| std::fs::metadata(taida_bin()).ok()?.modified().ok())
}

/// RC-8b: Try to load a cached .wasm file. Returns the path if the cache exists
/// and is not stale.
///
/// M-1: Compares the source file (.td) modification time against the cached .wasm.
/// RCB-55: Also compares the taida binary's mtime — if the compiler was rebuilt,
/// the cache is considered stale and `None` is returned, forcing recompilation.
/// If the source or binary is newer than the cache, `None` is returned.
/// Equal mtime is treated as valid (same-time writes are assumed identical).
pub fn cached_wasm(profile: &str, stem: &str, td_path: &Path) -> Option<PathBuf> {
    let cache_path = wasm_test_cache_dir(profile).join(format!("{}.wasm", stem));
    if cache_path.exists() {
        // L-3: If metadata/mtime cannot be read, treat as cache miss (safe side).
        let cache_meta = std::fs::metadata(&cache_path).ok()?;
        let cache_mtime = cache_meta.modified().ok()?;
        // M-1: Invalidate if source is newer than cache.
        let src_meta = std::fs::metadata(td_path).ok()?;
        let src_mtime = src_meta.modified().ok()?;
        if src_mtime > cache_mtime {
            return None; // stale: source changed
        }
        // RCB-55: Invalidate if taida binary is newer than cache.
        if let Some(bin_mtime) = taida_bin_mtime()
            && bin_mtime > cache_mtime
        {
            return None; // stale: compiler rebuilt
        }
        Some(cache_path)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// D29B-009 / Lock-F: shared NET test port allocator
// ---------------------------------------------------------------------------
//
// Mirror of `tests/parity.rs::find_free_loopback_port` (C25B-002 / C26B-003
// root-cause-fixed allocator). Hoisted into `common::` so any NET integration
// test can share the same probe/cooldown invariants without duplicating
// hard-coded port bands (e.g. the legacy `AtomicU16::new(17000)` allocator
// in `tests/c27b_027_read_body_2arg.rs` that this commit retires).
//
// Properties (inherited from parity.rs):
//   * Stays strictly below `/proc/sys/net/ipv4/ip_local_port_range` so the
//     kernel never re-hands an allocated port to an ephemeral client socket.
//   * PID-seeded counter: independent test binaries do not converge on the
//     same range under nextest 2C parallelism.
//   * 8-second cooldown list: the same port is never re-issued while the
//     intended consumer is still racing to bind it.
//   * Double-bind probe: rejects ports about to be claimed by an in-flight
//     ephemeral socket.
//
// Note: `parity.rs` keeps its own private copy because the `Mutex<...>` /
// `AtomicU16` statics there are tied to its `c25b_002_*` regression test
// (`crate::tests::parity::find_free_loopback_port` references). Two
// independent allocators are intentional — they target different test
// binaries and therefore live in different process address spaces.

const PORT_COOLDOWN_SECS: u64 = 8;
const ALLOC_PORT_MIN: u16 = 10000;

fn port_probe_lock() -> &'static Mutex<Vec<(u16, std::time::Instant)>> {
    static LOCK: OnceLock<Mutex<Vec<(u16, std::time::Instant)>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(Vec::with_capacity(64)))
}

fn ephemeral_port_min() -> u16 {
    if let Ok(s) = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range") {
        let mut parts = s.split_ascii_whitespace();
        if let Some(min_s) = parts.next()
            && let Ok(min) = min_s.parse::<u16>()
            && min >= 1024
        {
            return min;
        }
    }
    32768
}

/// Allocate a unique, bindable loopback port for NET integration tests.
///
/// Replaces the legacy `AtomicU16::new(17000)` per-binary allocators that
/// CI run 24935511811 / 24846315881 (D28 main) showed to be flaky under
/// nextest 2C parallelism. See `.dev/D29_BLOCKERS.md::D29B-009` for the
/// full failure inventory.
pub fn find_free_loopback_port() -> u16 {
    static INIT: Once = Once::new();
    static COUNTER: AtomicU16 = AtomicU16::new(0);

    let alloc_max = ephemeral_port_min().saturating_sub(1);
    let (band_min, band_max) = if alloc_max < ALLOC_PORT_MIN + 500 {
        (ALLOC_PORT_MIN, 65000u16)
    } else {
        (ALLOC_PORT_MIN, alloc_max)
    };
    let band_span = (band_max - band_min) as u32 + 1;

    INIT.call_once(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free loopback port");
        let seed = listener.local_addr().expect("local addr").port();
        let pid_bias = (std::process::id() as u16).wrapping_mul(37);
        let offset = (seed as u32).wrapping_add(pid_bias as u32) % band_span;
        let biased = band_min + offset as u16;
        COUNTER.store(biased, Ordering::Relaxed);
    });

    let mut cooldown = port_probe_lock().lock().expect("port_probe_lock poisoned");
    let now = std::time::Instant::now();
    cooldown.retain(|&(_, t)| now.duration_since(t).as_secs() < PORT_COOLDOWN_SECS);

    for _ in 0..400 {
        let raw = COUNTER.fetch_add(2, Ordering::Relaxed);
        let port = if (band_min..=band_max).contains(&raw) {
            raw
        } else {
            let wrapped = band_min + (raw % band_span.max(1) as u16);
            COUNTER.store(wrapped.wrapping_add(2), Ordering::Relaxed);
            wrapped
        };

        if cooldown.iter().any(|&(p, _)| p == port) {
            continue;
        }

        let Ok(listener1) = TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        drop(listener1);

        let Ok(listener2) = TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        drop(listener2);

        cooldown.push((port, std::time::Instant::now()));
        return port;
    }
    panic!("find_free_loopback_port: could not find a free port after 400 attempts");
}
