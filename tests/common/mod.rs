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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Binary discovery helpers (RCB-26)
// ---------------------------------------------------------------------------

/// Get the path to the built `taida` binary.
///
/// Tries `CARGO_BIN_EXE_taida` first (set by `cargo test`), then falls back
/// to `target/debug/taida` relative to the manifest directory.
pub fn taida_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_taida"));
    if !path.exists() {
        path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("taida");
    }
    path
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
