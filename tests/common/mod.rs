//! Shared test cache utilities for wasm backend integration tests.
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
use std::sync::OnceLock;

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
    if std::fs::copy(wasm_path, &tmp_path).is_ok() {
        if std::fs::rename(&tmp_path, &cache_path).is_err() {
            // N-3: Clean up the .tmp file on rename failure.
            let _ = std::fs::remove_file(&tmp_path);
        }
    }
}

/// RC-8b: Try to load a cached .wasm file. Returns the path if the cache exists
/// and is not stale.
///
/// M-1: Compares the source file (.td) modification time against the cached .wasm.
/// If the source is newer than the cache, the cache is considered stale and `None`
/// is returned, forcing recompilation.
pub fn cached_wasm(profile: &str, stem: &str, td_path: &Path) -> Option<PathBuf> {
    let cache_path = wasm_test_cache_dir(profile).join(format!("{}.wasm", stem));
    if cache_path.exists() {
        // M-1: Invalidate if source is newer than cache.
        if let (Ok(cache_meta), Ok(src_meta)) =
            (std::fs::metadata(&cache_path), std::fs::metadata(td_path))
        {
            if let (Ok(cache_mtime), Ok(src_mtime)) = (cache_meta.modified(), src_meta.modified()) {
                if src_mtime > cache_mtime {
                    return None; // stale cache
                }
            }
        }
        Some(cache_path)
    } else {
        None
    }
}
