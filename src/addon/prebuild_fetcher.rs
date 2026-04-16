//! Prebuild addon binary fetcher (RC1.5 Phase 2).
//!
//! Downloads prebuild `.so`/`.dylib`/`.dll` binaries from a URL,
//! verifies SHA-256 integrity, and places them atomically into
//! the addon cache (`~/.taida/addon-cache/<org>/<name>/<version>/<target>/`).
//!
//! - HTTPS: uses `reqwest` blocking (production)
//! - `file://`: copies from local path (testing / dev)
//! - SHA-256: streaming verification during download
//! - Size limit: 100 MB
//! - Cache hit: re-verify SHA-256 before reuse

use std::fmt;
use std::path::{Path, PathBuf};

use crate::crypto::Sha256;

// ── Error ───────────────────────────────────────────────────────

/// Errors from fetching / verifying / caching a prebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchError {
    /// Target is not in the supported list.
    UnsupportedTarget {
        host: String,
        supported: Vec<String>,
    },
    /// Download / file copy failed.
    DownloadFailed { message: String },
    /// SHA-256 mismatch after download.
    IntegrityMismatch { expected: String, actual: String },
    /// Download exceeded 100 MB size limit.
    SizeLimitExceeded { max_bytes: u64, actual_bytes: u64 },
    /// Cache I/O error (directory creation, rename, write).
    CacheIoError { message: String },
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTarget { host, supported } => {
                writeln!(f, "addon is not available for your platform")?;
                writeln!(f, "  host target:    {host}")?;
                writeln!(f, "  supported targets:")?;
                for t in supported {
                    writeln!(f, "    - {t}")?;
                }
                write!(
                    f,
                    "  action: ask the addon author to upload a prebuild for {host}"
                )
            }
            Self::DownloadFailed { message } => {
                write!(f, "prebuild download failed: {message}")
            }
            Self::IntegrityMismatch { expected, actual } => {
                writeln!(f, "addon integrity check failed")?;
                writeln!(f, "  expected: {expected}")?;
                write!(f, "  actual:   {actual}")
            }
            Self::SizeLimitExceeded {
                max_bytes,
                actual_bytes,
            } => {
                write!(
                    f,
                    "prebuild binary exceeded the {} MB size limit (downloaded {actual_bytes} bytes)",
                    max_bytes / 1_048_576,
                )
            }
            Self::CacheIoError { message } => {
                write!(f, "cache I/O error: {message}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

fn download_fail(message: impl Into<String>) -> FetchError {
    FetchError::DownloadFailed {
        message: message.into(),
    }
}

fn cache_io(message: impl Into<String>) -> FetchError {
    FetchError::CacheIoError {
        message: message.into(),
    }
}

// ── Constants ───────────────────────────────────────────────────

/// Maximum allowed prebuild binary size (100 MB).
const MAX_SIZE_BYTES: u64 = 100 * 1024 * 1024;

/// RC15B-106: HTTPS redirect limit.
///
/// `reqwest`'s default redirect policy follows up to 10 redirects. We pin
/// that limit explicitly so the contract is visible in source code and
/// the documentation can point at a concrete constant. Redirect chains
/// longer than this are treated as suspicious and the download is
/// aborted with a deterministic `DownloadFailed` error.
///
/// A redirect still requires the final URL to use `https://`; downgrades
/// to `http://` are rejected by reqwest's default policy (we do not
/// relax this).
const HTTPS_MAX_REDIRECTS: usize = 10;

// ── RC15B-002: progress reporting ───────────────────────────────

/// Callback invoked during prebuild downloads so the caller can render
/// a progress bar (or a simple byte-count log line). The first argument
/// is bytes-so-far, the second is the total size if known from the
/// `Content-Length` header. The callback is invoked:
///
/// - Once at the start with `(0, total)`.
/// - Periodically as bytes are appended (throttled so it fires at most
///   about 20 times per second regardless of chunk size).
/// - Once at the end with `(final_size, Some(final_size))`.
///
/// The callback must not panic; a panicking callback aborts the download.
pub type ProgressCallback<'a> = dyn FnMut(u64, Option<u64>) + 'a;

/// A no-op progress callback used by the legacy `fetch_prebuild` entry
/// point. Kept as a free function so closures can be expressed inline.
fn noop_progress(_: u64, _: Option<u64>) {}

// ── Cache paths ────────────────────────────────────────────────

/// Returns the addon cache root (`~/.taida/addon-cache`).
///
/// Exposed to the crate so that CLI commands like `taida cache clean`
/// (RC15B-001) can locate the directory without duplicating the path
/// logic.
pub(crate) fn cache_root() -> Result<PathBuf, FetchError> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| cache_io("cannot determine home directory ($HOME not set)"))?;
    Ok(home.join(".taida/addon-cache"))
}

// ── RC15B-001: addon cache cleanup ──────────────────────────────

/// Summary of a [`clean_addon_cache`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddonCacheCleanSummary {
    /// Number of `lib*.so` / `lib*.dylib` / `lib*.dll` files removed.
    pub binaries_removed: usize,
    /// Number of `.manifest-sha256` sidecar files removed.
    pub sidecars_removed: usize,
    /// Total bytes freed (best-effort — files missing metadata count as 0).
    pub bytes_freed: u64,
    /// Whether the cache root existed at the time of the call.
    pub root_existed: bool,
    /// Resolved cache root path for diagnostics.
    pub root: PathBuf,
}

/// Remove every cached addon binary under `~/.taida/addon-cache`.
///
/// This is the `taida cache clean --addons` implementation for
/// RC15B-001. Unlike the WASM runtime cache cleaner, addon binaries are
/// keyed by `<org>/<name>/<version>/<target>/` so the directory tree is
/// walked recursively. The walk is conservative: it only removes files
/// whose basename matches the cached layout (`lib<name>.<ext>` or
/// `.manifest-sha256`), leaving anything unexpected alone so a confused
/// user can inspect the directory manually without losing state.
///
/// On a fresh machine where the directory does not yet exist, the call
/// succeeds and reports `root_existed = false`.
pub fn clean_addon_cache() -> Result<AddonCacheCleanSummary, FetchError> {
    let root = cache_root()?;
    let mut summary = AddonCacheCleanSummary {
        root: root.clone(),
        ..Default::default()
    };

    if !root.exists() {
        return Ok(summary);
    }
    summary.root_existed = true;

    fn walk(dir: &Path, summary: &mut AddonCacheCleanSummary) -> Result<(), FetchError> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| cache_io(format!("cannot read cache dir {}: {}", dir.display(), e)))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                walk(&path, summary)?;
                // After processing children, try to prune empty directories.
                let _ = std::fs::remove_dir(&path);
            } else if meta.is_file() {
                let fname = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or_default();
                let is_sidecar = fname == ".manifest-sha256";
                let is_binary = fname.starts_with("lib")
                    && (fname.ends_with(".so")
                        || fname.ends_with(".dylib")
                        || fname.ends_with(".dll"));
                // Also sweep leftover temp files from aborted installs.
                let is_temp = fname.ends_with(".tmp") || fname.contains(".tmp.");
                if is_sidecar || is_binary || is_temp {
                    let size = meta.len();
                    if std::fs::remove_file(&path).is_ok() {
                        summary.bytes_freed = summary.bytes_freed.saturating_add(size);
                        if is_sidecar {
                            summary.sidecars_removed += 1;
                        } else if is_binary {
                            summary.binaries_removed += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    walk(&root, &mut summary)?;
    Ok(summary)
}

/// Cache path for a specific prebuild version+target.
///
/// `~/.taida/addon-cache/<org>/<name>/<version>/<target>/`
fn cache_dir_for(
    org: &str,
    name: &str,
    version: &str,
    target: &str,
) -> Result<PathBuf, FetchError> {
    let root = cache_root()?;
    Ok(root.join(org).join(name).join(version).join(target))
}

/// Sidecar file storing the expected SHA-256 for cheap lookup.
fn sha256_sidecar(cache_dir: &Path) -> PathBuf {
    cache_dir.join(".manifest-sha256")
}

// ── SHA-256 placeholder detection (C14B-012) ─────────────────

/// Returns `true` when `value` is an all-zero placeholder digest of the
/// shape `"sha256:0{64}"` (case-insensitive). `taida init --target
/// rust-addon` seeds `native/addon.toml [library.prebuild.targets]` with
/// this placeholder so the scaffolded package type-checks before the
/// first CI release has computed real digests, and an addon author who
/// forgets to move the handback row after first release leaves the
/// placeholder frozen in the tag's tree forever (the tarball is
/// immutable — see [C14B-012]).
///
/// The install resolver uses this check to route to the
/// `addon.lock.toml` release-asset fallback when an addon.toml row is
/// still a placeholder, rather than failing with an unfixable integrity
/// mismatch against the CI-computed digest. Without the placeholder
/// detection, initial releases that ship a placeholder `addon.toml`
/// become permanently unusable to consumers.
///
/// Rejects:
/// - values without a `sha256:` prefix
/// - digests whose hex body is not 64 chars long
/// - digests containing any non-zero hex digit
///
/// Matches:
/// - `"sha256:0000000000000000000000000000000000000000000000000000000000000000"`
///   (terminal `@a.1` shape)
/// - case-insensitive: both lowercase and uppercase hex zeros.
pub fn is_placeholder_sha(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    if hex.len() != 64 {
        return false;
    }
    hex.chars().all(|c| c == '0')
}

// ── SHA-256 verification of an existing file ───────────────────

fn verify_sha256(path: &Path, expected: &str) -> Result<(), FetchError> {
    let data = std::fs::read(path)
        .map_err(|e| cache_io(format!("cannot read cached file {}: {}", path.display(), e)))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual = hasher.finalize_hex();
    if actual == expected {
        Ok(())
    } else {
        Err(FetchError::IntegrityMismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}

// ── Parse org/name from package id ─────────────────────────────

/// Splits `"taida-lang/terminal"` into `("taida-lang", "terminal")`.
///
/// # Security (RC15B-102)
///
/// Both `org` and `name` must match `[a-zA-Z0-9._-]+` to prevent
/// cache directory traversal (e.g. `"../../../malicious"`).
fn split_package_id(package_id: &str) -> Option<(&str, &str)> {
    let (org, name) = package_id.split_once('/')?;
    if org.is_empty() || name.is_empty() {
        return None;
    }
    let valid = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    };
    if !valid(org) || !valid(name) {
        return None;
    }
    Some((org, name))
}

// ── Fetcher ────────────────────────────────────────────────────

/// Fetch a prebuild addon binary.
///
/// 1. Checks if the file is already cached and verifies its SHA-256.
/// 2. If not, downloads/copies from the URL with streaming SHA-256.
/// 3. Places the file atomically (temp file -> rename).
/// 4. Writes a `.manifest-sha256` sidecar for cheap lookup.
///
/// `package_id`: e.g. `"taida-lang/terminal"` (used to build cache path)
/// `version`:    e.g. `"a.1"`
/// `target_triple`: canonical triple e.g. `"x86_64-unknown-linux-gnu"`
/// `lib_name`:    cdylib stem e.g. `"terminal"`
/// `ext`:         platform extension e.g. `"so"`
/// `url`:         full download URL or `file://` path
/// `expected_sha256`: `sha256:` prefix + 64 hex chars (the exact hex part)
pub fn fetch_prebuild(
    package_id: &str,
    version: &str,
    target_triple: &str,
    lib_name: &str,
    ext: &str,
    url: &str,
    expected_sha256: &str,
) -> Result<PathBuf, FetchError> {
    // Delegate to the progress-aware variant with a no-op callback so
    // the two entry points can never drift.
    fetch_prebuild_with_progress(
        package_id,
        version,
        target_triple,
        lib_name,
        ext,
        url,
        expected_sha256,
        &mut noop_progress,
    )
}

/// RC15B-002: progress-aware variant of [`fetch_prebuild`].
///
/// The CLI calls this directly so it can render a byte-count progress
/// indicator during long downloads. The `progress` callback is invoked
/// with `(bytes_so_far, total_bytes_if_known)`; see [`ProgressCallback`]
/// for the invocation contract.
///
/// Behaviour is otherwise identical to [`fetch_prebuild`]: cache-first,
/// streaming SHA-256 verification, atomic placement.
#[allow(clippy::too_many_arguments)]
pub fn fetch_prebuild_with_progress(
    package_id: &str,
    version: &str,
    target_triple: &str,
    lib_name: &str,
    ext: &str,
    url: &str,
    expected_sha256: &str,
    progress: &mut ProgressCallback<'_>,
) -> Result<PathBuf, FetchError> {
    let (org, name) = split_package_id(package_id)
        .ok_or_else(|| download_fail(format!("invalid package id: {package_id}")))?;

    let cache_dir = cache_dir_for(org, name, version, target_triple)?;
    let lib_filename = format!("lib{lib_name}.{ext}");
    let dest_path = cache_dir.join(&lib_filename);

    // 1. Cache hit: verify SHA-256 and return.
    if dest_path.exists() {
        if let Err(e) = verify_sha256(&dest_path, expected_sha256) {
            // Corrupted cache, remove and re-download.
            let _ = std::fs::remove_file(&dest_path);
            let _ = std::fs::remove_file(sha256_sidecar(&cache_dir));
            return Err(e);
        }
        return Ok(dest_path);
    }

    // 2. Download / copy with streaming SHA-256.
    let data = download_from_url(url, expected_sha256, progress)?;

    // 3. Atomic placement.
    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        cache_io(format!(
            "cannot create cache dir {}: {}",
            cache_dir.display(),
            e
        ))
    })?;

    let temp_path = cache_dir.join(format!("{lib_filename}.tmp"));
    std::fs::write(&temp_path, &data).map_err(|e| {
        cache_io(format!(
            "cannot write temp file {}: {}",
            temp_path.display(),
            e
        ))
    })?;

    std::fs::rename(&temp_path, &dest_path).map_err(|e| {
        cache_io(format!(
            "cannot atomically place {}: {}",
            dest_path.display(),
            e
        ))
    })?;

    // 4. Write sidecar.
    std::fs::write(sha256_sidecar(&cache_dir), expected_sha256)
        .map_err(|e| cache_io(format!("cannot write sha256 sidecar: {}", e)))?;

    Ok(dest_path)
}

/// Download from URL with streaming SHA-256 verification.
fn download_from_url(
    url: &str,
    expected_sha256: &str,
    progress: &mut ProgressCallback<'_>,
) -> Result<Vec<u8>, FetchError> {
    if let Some(path) = url.strip_prefix("file://") {
        download_from_file(path, expected_sha256, progress)
    } else if url.starts_with("https://") {
        download_from_https(url, expected_sha256, progress)
    } else {
        Err(download_fail(format!(
            "unsupported URL scheme (only https:// and file:// are allowed): {url}"
        )))
    }
}

/// Copy a file from local path with SHA-256 verification.
///
/// # Security (RC15B-101)
///
/// - Absolute paths are rejected to prevent reading arbitrary system files.
/// - Path traversal components (`..`) are rejected before any filesystem
///   access to prevent attacker-controlled manifest paths from escaping
///   the intended directory scope.
/// - Only relative paths from the project root are allowed.
fn download_from_file(
    file_path: &str,
    expected_sha256: &str,
    progress: &mut ProgressCallback<'_>,
) -> Result<Vec<u8>, FetchError> {
    // Reject absolute paths to prevent arbitrary file reads.
    if std::path::Path::new(file_path).is_absolute() || file_path.starts_with('/') {
        return Err(download_fail(format!(
            "file:// URL with absolute path is not allowed (use relative path from project root): {file_path}"
        )));
    }

    // Reject path traversal components before any filesystem access.
    if file_path.contains("..") {
        return Err(download_fail(format!(
            "file:// URL contains path traversal component: {file_path}"
        )));
    }

    let data = std::fs::read(file_path)
        .map_err(|e| download_fail(format!("cannot read file {}: {}", file_path, e)))?;

    if data.len() as u64 > MAX_SIZE_BYTES {
        return Err(FetchError::SizeLimitExceeded {
            max_bytes: MAX_SIZE_BYTES,
            actual_bytes: data.len() as u64,
        });
    }

    // Report progress once with both start and end states so the CLI can
    // still draw a bar for file:// reads (useful for local sample addons
    // during development).
    let total = data.len() as u64;
    progress(0, Some(total));
    progress(total, Some(total));

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual = hasher.finalize_hex();

    if actual != expected_sha256 {
        return Err(FetchError::IntegrityMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    Ok(data)
}

/// Download from HTTPS URL with streaming SHA-256 verification.
///
/// ## RC15B-106: redirect limits
///
/// The client is built with an explicit
/// [`reqwest::redirect::Policy::limited`] ceiling of
/// [`HTTPS_MAX_REDIRECTS`] (10). Longer chains are aborted with a
/// deterministic error rather than silently followed forever — both to
/// defend against redirect loops and to keep the contract between
/// manifest author and the fetcher obvious.
///
/// ## RC15B-002: progress callback
///
/// The supplied `progress` callback is invoked at the start of the
/// download with the content length (if known), every ~64 KiB while
/// streaming, and once more at the end. Progress throttling keeps the
/// terminal UI quiet for small binaries.
#[cfg(feature = "community")]
fn download_from_https(
    url: &str,
    expected_sha256: &str,
    progress: &mut ProgressCallback<'_>,
) -> Result<Vec<u8>, FetchError> {
    use reqwest::blocking::Client;
    use reqwest::redirect::Policy;
    use std::io::Read;

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(Policy::limited(HTTPS_MAX_REDIRECTS))
        .build()
        .map_err(|e| download_fail(format!("cannot create HTTP client: {e}")))?;

    let mut response = client
        .get(url)
        .send()
        .map_err(|e| download_fail(format!("request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(download_fail(format!(
            "HTTP {} for {url}",
            response.status()
        )));
    }

    // Check Content-Length upper bound.
    let total_len = response.content_length();
    if let Some(len) = total_len
        && len > MAX_SIZE_BYTES
    {
        return Err(FetchError::SizeLimitExceeded {
            max_bytes: MAX_SIZE_BYTES,
            actual_bytes: len,
        });
    }

    // Kick the progress callback once with the known total so the CLI
    // can paint an empty bar immediately.
    progress(0, total_len);

    let mut hasher = Sha256::new();
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    // Throttle progress updates so very small chunks don't flood the
    // callback (which typically calls into `eprint!`).
    const PROGRESS_TICK: u64 = 64 * 1024;
    let mut last_reported: u64 = 0;

    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| download_fail(format!("read failed: {e}")))?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if data.len() as u64 + chunk.len() as u64 > MAX_SIZE_BYTES {
            return Err(FetchError::SizeLimitExceeded {
                max_bytes: MAX_SIZE_BYTES,
                actual_bytes: data.len() as u64 + chunk.len() as u64,
            });
        }
        hasher.update(chunk);
        data.extend_from_slice(chunk);

        let so_far = data.len() as u64;
        if so_far - last_reported >= PROGRESS_TICK {
            progress(so_far, total_len);
            last_reported = so_far;
        }
    }

    // Final progress tick with the definitive size.
    let final_size = data.len() as u64;
    progress(final_size, Some(final_size));

    let actual = hasher.finalize_hex();
    if actual != expected_sha256 {
        return Err(FetchError::IntegrityMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    Ok(data)
}

/// HTTPS download for when `community` feature is not enabled (test mode).
#[cfg(not(feature = "community"))]
fn download_from_https(
    url: &str,
    _expected_sha256: &str,
    _progress: &mut ProgressCallback<'_>,
) -> Result<Vec<u8>, FetchError> {
    Err(download_fail(format!(
        "HTTPS downloads require the 'community' feature: {url}"
    )))
}

// ── RC2.6B-019: addon.lock.toml release asset fetcher ─────────

/// Download `addon.lock.toml` from a GitHub Release asset.
///
/// This is a lightweight text fetch (no SHA-256 streaming verification —
/// the lockfile itself *is* the source of SHA-256 values, not a verified
/// payload). Size-limited to 1 MB as a sanity check.
///
/// `package_name`: `"org/name"` form, e.g. `"shijimic/terminal"`
/// `version`:      exact tag, e.g. `"a.1"` (NO `v` prefix — Taida tags
///                 are `a.1`, not `va.1`)
#[cfg(feature = "community")]
pub fn fetch_release_lockfile(package_name: &str, version: &str) -> Result<String, String> {
    let (org, name) = package_name
        .split_once('/')
        .ok_or_else(|| format!("Cannot parse package name '{}' as org/name", package_name))?;

    let base_url = crate::pkg::store::github_base_url();
    let url = format!(
        "{}/{}/{}/releases/download/{}/addon.lock.toml",
        base_url.trim_end_matches('/'),
        org,
        name,
        version,
    );

    let response = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", "taida-install")
        .send()
        .map_err(|e| {
            format!(
                "Failed to download addon.lock.toml for '{}@{}' from {}: {}",
                package_name, version, url, e
            )
        })?;

    if !response.status().is_success() {
        return Err(format!(
            "addon.lock.toml not found for '{}@{}' (HTTP {})\n\
             \x20 url: {}\n\
             \x20 hint: the addon author may not have published a prebuild yet.\n\
             \x20       Run `taida publish --target rust-addon` in the addon repository.",
            package_name,
            version,
            response.status(),
            url
        ));
    }

    let body = response
        .text()
        .map_err(|e| format!("Failed to read addon.lock.toml response body: {}", e))?;

    // Sanity check: lockfile should be small text
    if body.len() > 1_048_576 {
        return Err(format!(
            "addon.lock.toml for '{}@{}' is too large ({} bytes, limit 1 MB)",
            package_name,
            version,
            body.len()
        ));
    }

    Ok(body)
}

/// Stub for when `community` feature is not enabled.
#[cfg(not(feature = "community"))]
pub fn fetch_release_lockfile(package_name: &str, version: &str) -> Result<String, String> {
    Err(format!(
        "HTTPS downloads require the 'community' feature (addon.lock.toml for '{}@{}')",
        package_name, version
    ))
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_file(data: &[u8]) -> (PathBuf, String) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        use std::time::SystemTime;

        let mut hasher = Sha256::new();
        hasher.update(data);
        let sha = hasher.finalize_hex();

        // Unique filename with timestamp + hash to avoid collisions between tests
        let mut h = DefaultHasher::new();
        h.write_u64(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
        );
        h.write(sha.as_bytes());
        let unique = h.finish();

        let dir = std::env::temp_dir();
        let path = dir.join(format!("taida_fetcher_test_{}_{}", unique, &sha[..16]));
        std::fs::write(&path, data).unwrap();

        (path, sha)
    }

    /// RAII guard that removes a per-test temp directory when dropped.
    ///
    /// We cannot use `tempfile::TempDir` directly because
    /// `download_from_file` enforces a relative-path-only policy on
    /// `file://` URLs (RC15B-101), but `tempfile::TempDir` canonicalizes
    /// its path to an absolute form. Instead we pick a unique relative
    /// directory name under CWD and clean it up on drop.
    struct RelativeTempDir {
        path: PathBuf,
    }

    impl Drop for RelativeTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Create a temp file inside a per-test isolated directory, returned
    /// as a relative path so it can be used with the `file://`
    /// relative-path-only constraint enforced by `download_from_file`.
    ///
    /// C12-8 (FB-24): each test gets its own unique directory rooted at
    /// the crate CWD. The returned `RelativeTempDir` guard must be kept
    /// alive for the duration of the test; dropping it removes the
    /// whole directory, so sibling tests never share a parent and cannot
    /// race on `create_dir_all` / `remove_*` ordering.
    ///
    /// Replaces the previous `make_relative_temp_file` which stored all
    /// test files under a shared `.taida-test-temp/` directory.
    fn make_relative_temp_file(data: &[u8]) -> (RelativeTempDir, PathBuf, String) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        use std::time::SystemTime;

        let mut hasher = Sha256::new();
        hasher.update(data);
        let sha = hasher.finalize_hex();

        // Build a globally unique directory name. Combining process id,
        // thread id, nanosecond timestamp, and the data hash gives
        // enough entropy that parallel tests never collide even when
        // invoked in rapid succession.
        let mut h = DefaultHasher::new();
        h.write_u64(std::process::id() as u64);
        h.write_u64(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
        );
        let tid = format!("{:?}", std::thread::current().id());
        h.write(tid.as_bytes());
        h.write(sha.as_bytes());
        let unique = h.finish();

        let dir_rel = PathBuf::from(format!("taida-fetcher-{:016x}", unique));
        std::fs::create_dir_all(&dir_rel).unwrap();

        let path = dir_rel.join("fetcher_test_bin");
        std::fs::write(&path, data).unwrap();

        (RelativeTempDir { path: dir_rel }, path, sha)
    }

    #[test]
    fn file_scheme_happy_path() {
        let data = b"test binary content";
        // C12-8 (FB-24): `_guard` owns a per-test tempdir under CWD;
        // its Drop cleans up the whole directory, so parallel tests
        // cannot race on a shared parent.
        let (_guard, path, sha) = make_relative_temp_file(data);

        // file:// URL with relative path: file://<tempdir>/fetcher_test_bin
        let url = format!("file://{}", path.display());
        let result = download_from_url(&url, &sha, &mut noop_progress);
        if let Err(ref e) = result {
            panic!("download failed: {:?}\nurl: {}", e, url);
        }
        assert_eq!(result.unwrap(), *data);
    }

    #[test]
    fn file_scheme_integrity_mismatch() {
        let data = b"test binary content";
        // C12-8 (FB-24): per-test isolated tempdir (see
        // `file_scheme_happy_path`).
        let (_guard, path, _) = make_relative_temp_file(data);

        let url = format!("file://{}", path.display());
        let wrong_sha = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let err = download_from_url(&url, wrong_sha, &mut noop_progress).unwrap_err();
        assert!(matches!(err, FetchError::IntegrityMismatch { .. }));
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        let err = download_from_url(
            "http://example.com/binary.so",
            "sha256:aa",
            &mut noop_progress,
        )
        .unwrap_err();
        assert!(matches!(err, FetchError::DownloadFailed { .. }));
        assert!(err.to_string().contains("unsupported URL scheme"));
    }

    #[test]
    fn file_scheme_progress_is_reported() {
        // RC15B-002: file:// reads should still fire the progress
        // callback so the CLI bar works for local sample addons.
        let data = b"ABCDEFGH";
        // C12-8 (FB-24): per-test isolated tempdir (see
        // `file_scheme_happy_path`).
        let (_guard, path, sha) = make_relative_temp_file(data);
        let url = format!("file://{}", path.display());

        let mut calls: Vec<(u64, Option<u64>)> = Vec::new();
        {
            let mut cb = |so_far: u64, total: Option<u64>| calls.push((so_far, total));
            download_from_url(&url, &sha, &mut cb).unwrap();
        }

        // At least two calls: start (0) and end (total).
        assert!(calls.len() >= 2, "calls: {:?}", calls);
        assert_eq!(calls.first().unwrap().0, 0);
        assert_eq!(calls.last().unwrap().0, data.len() as u64);
        assert_eq!(calls.last().unwrap().1, Some(data.len() as u64));
    }

    // ── C14B-012: placeholder SHA detection ──────────────────

    #[test]
    fn placeholder_sha_matches_all_zero_digest() {
        // Canonical shape emitted by `taida init --target rust-addon`
        // and persisted by terminal `@a.1`. Must be detected so the
        // resolver can reroute to addon.lock.toml.
        assert!(is_placeholder_sha(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn placeholder_sha_rejects_non_zero_digest() {
        // A real CI-computed digest must never be treated as a
        // placeholder, otherwise we would silently skip integrity
        // verification for legitimately-signed assets.
        assert!(!is_placeholder_sha(
            "sha256:d9c093feaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        // A single non-zero digit anywhere is enough to disqualify.
        assert!(!is_placeholder_sha(
            "sha256:0000000000000000000000000000000000000000000000000000000000000001"
        ));
    }

    #[test]
    fn placeholder_sha_rejects_missing_prefix() {
        // Without the `sha256:` prefix we cannot distinguish this
        // from e.g. a bare hex blob in a future signature scheme.
        assert!(!is_placeholder_sha(
            "0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn placeholder_sha_rejects_wrong_length() {
        // SHA-256 hex is exactly 64 chars; short or long values are
        // structurally invalid and should not trigger fallback.
        assert!(!is_placeholder_sha("sha256:0"));
        assert!(!is_placeholder_sha(&format!("sha256:{}", "0".repeat(63))));
        assert!(!is_placeholder_sha(&format!("sha256:{}", "0".repeat(65))));
    }

    #[test]
    fn placeholder_sha_rejects_empty_and_unrelated() {
        assert!(!is_placeholder_sha(""));
        assert!(!is_placeholder_sha("sha256:"));
        // Other digest algorithms are structurally disjoint — not our
        // problem (but must not be accepted as a placeholder either).
        assert!(!is_placeholder_sha(&format!("md5:{}", "0".repeat(64))));
    }

    #[test]
    fn split_package_id_valid() {
        assert_eq!(
            split_package_id("taida-lang/terminal"),
            Some(("taida-lang", "terminal"))
        );
    }

    #[test]
    fn split_package_id_invalid() {
        assert_eq!(split_package_id("no-slash"), None);
    }

    #[test]
    fn split_package_id_rejects_path_traversal_in_org() {
        assert_eq!(split_package_id("../etc/passwd/name"), None);
    }

    #[test]
    fn split_package_id_rejects_path_traversal_in_name() {
        assert_eq!(split_package_id("org/../../../etc/passwd"), None);
    }

    #[test]
    fn split_package_id_empty_org_or_name() {
        assert_eq!(split_package_id("/name"), None);
        assert_eq!(split_package_id("org/"), None);
    }

    #[test]
    fn file_scheme_path_traversal_is_rejected() {
        // RC15B-101: `..` component is rejected before filesystem access.
        let err = download_from_file("../../../../etc/passwd", "sha256:aa", &mut noop_progress)
            .unwrap_err();
        assert!(matches!(err, FetchError::DownloadFailed { .. }));
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn file_scheme_absolute_path_is_rejected() {
        let err = download_from_file("/tmp/../../../etc/passwd", "sha256:aa", &mut noop_progress)
            .unwrap_err();
        assert!(matches!(err, FetchError::DownloadFailed { .. }));
        assert!(err.to_string().contains("absolute path"));
    }

    #[test]
    fn file_scheme_path_traversal_leading_dots_is_rejected() {
        // After stripping `/`, a relative path with `..` is also rejected.
        let err = download_from_file("tmp/../../../etc/passwd", "sha256:aa", &mut noop_progress)
            .unwrap_err();
        assert!(matches!(err, FetchError::DownloadFailed { .. }));
        assert!(err.to_string().contains("path traversal"));
    }

    // ── RC15B-001: addon cache cleanup ──────────────────────────

    #[test]
    fn clean_addon_cache_on_empty_root() {
        // RC2B-209: serialize HOME mutation against every other
        // test (in any module of this crate) that touches HOME via
        // the shared `crate::util::env_test_lock()`. Cargo runs
        // unit tests in parallel and `cache_root()` reads `HOME`,
        // so without this lock two clean_addon_cache tests race
        // and one walks the other test's fixture root.
        let _guard = crate::util::env_test_lock().lock().unwrap();
        // Redirect HOME to a temp dir so we don't touch the real cache.
        let saved_home = std::env::var("HOME").ok();
        let tmp = std::env::temp_dir().join(format!("taida_clean_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Safety: serialised by `env_test_lock()` above.
        unsafe {
            std::env::set_var("HOME", &tmp);
        }

        let summary = clean_addon_cache().expect("empty root must succeed");
        assert!(!summary.root_existed);
        assert_eq!(summary.binaries_removed, 0);
        assert_eq!(summary.sidecars_removed, 0);

        // Restore
        if let Some(h) = saved_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn clean_addon_cache_removes_binaries_and_sidecars() {
        // RC2B-209: serialize HOME mutation. See sibling test
        // `clean_addon_cache_on_empty_root` for the rationale.
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let saved_home = std::env::var("HOME").ok();
        let tmp = std::env::temp_dir().join(format!("taida_clean_nonempty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Safety: serialised by `env_test_lock()` above.
        unsafe {
            std::env::set_var("HOME", &tmp);
        }

        // Seed a fake cache entry.
        let pkg_dir =
            tmp.join(".taida/addon-cache/taida-lang/terminal/a.1/x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("libterminal.so"), b"FAKEBINARYDATA").unwrap();
        std::fs::write(pkg_dir.join(".manifest-sha256"), b"sha256:dead").unwrap();
        // Unrelated file that must survive.
        std::fs::write(pkg_dir.join("README.txt"), b"dont-touch-me").unwrap();

        let summary = clean_addon_cache().expect("clean must succeed");
        assert!(summary.root_existed);
        assert_eq!(summary.binaries_removed, 1);
        assert_eq!(summary.sidecars_removed, 1);
        assert!(summary.bytes_freed > 0);
        // Binary removed, README preserved.
        assert!(!pkg_dir.join("libterminal.so").exists());
        assert!(!pkg_dir.join(".manifest-sha256").exists());
        assert!(pkg_dir.join("README.txt").exists());

        if let Some(h) = saved_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn fetch_error_display_unsupported_target() {
        let err = FetchError::UnsupportedTarget {
            host: "x86_64-unknown-freebsd".to_string(),
            supported: vec!["x86_64-unknown-linux-gnu".to_string()],
        };
        let msg = err.to_string();
        assert!(msg.contains("not available for your platform"));
        assert!(msg.contains("x86_64-unknown-freebsd"));
        assert!(msg.contains("supported targets"));
    }

    #[test]
    fn fetch_error_display_integrity_mismatch() {
        let err = FetchError::IntegrityMismatch {
            expected: "sha256:aaaa".to_string(),
            actual: "sha256:bbbb".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("integrity check failed"));
        assert!(msg.contains("expected"));
        assert!(msg.contains("actual"));
    }

    #[test]
    fn verify_sha256_happy() {
        let data = b"hello world";
        let (path, sha) = make_temp_file(data);
        assert!(verify_sha256(&path, &sha).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn verify_sha256_mismatch() {
        let data = b"hello world";
        let (path, _) = make_temp_file(data);
        let wrong_sha = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        assert!(verify_sha256(&path, wrong_sha).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
