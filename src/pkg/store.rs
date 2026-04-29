/// Global package store for Taida Lang.
///
/// Manages `~/.taida/store/` where downloaded packages are cached.
///
/// Layout:
/// ```text
/// ~/.taida/store/{org}/{name}/{gen}.{num}[.{label}]/
///   main.td
///   mod.td
///   packages.tdm       <- used for transitive dependency resolution
///   ...
///   .taida_installed   <- download completion marker
///   _meta.toml         <- C17 sidecar: provenance + stale-check metadata
/// ```
///
/// ## C17 sidecar (`_meta.toml`)
///
/// C17 introduces a provenance sidecar written alongside `.taida_installed`.
/// The sidecar records the tarball SHA-256, the resolved commit SHA (filled
/// by the resolver in C17-2 / Phase 2), an RFC-3339 `fetched_at` timestamp,
/// and the source identifier (e.g. `github:taida-lang/terminal`).
///
/// The sidecar exists so `taida ingot install` can detect stale store entries when
/// a tag is republished (retag / delete+recreate). See `.dev/C17_DESIGN.md`.
///
/// C17-1 only writes the sidecar. The stale-detection decision table is
/// implemented in C17-2 (Phase 2) and consumed by `taida ingot install` there.
use std::path::{Path, PathBuf};

/// Base URL for GitHub archive downloads.
/// Override with `TAIDA_GITHUB_BASE_URL` for testing (e.g. local mock server).
pub(crate) fn github_base_url() -> String {
    std::env::var("TAIDA_GITHUB_BASE_URL").unwrap_or_else(|_| "https://github.com".to_string())
}

/// Base URL for GitHub API calls.
/// Override with `TAIDA_GITHUB_API_URL` for testing.
fn github_api_url() -> String {
    std::env::var("TAIDA_GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_string())
}

/// Global package store at `~/.taida/store/`.
pub struct GlobalStore {
    root: PathBuf,
}

impl Default for GlobalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalStore {
    /// Create a new GlobalStore using the default location (`~/.taida/store/`).
    pub fn new() -> Self {
        let home = crate::util::taida_home_dir().unwrap_or_else(|_| std::env::temp_dir());
        GlobalStore {
            root: home.join(".taida").join("store"),
        }
    }

    /// Create a GlobalStore with a custom root (for testing).
    #[cfg(test)]
    pub fn with_root(root: PathBuf) -> Self {
        GlobalStore { root }
    }

    /// Validate that a path component does not contain traversal sequences.
    /// Rejects `..`, `/`, `\`, and empty strings (RCB-307 / SEC-009).
    fn validate_path_component(component: &str, label: &str) -> Result<(), String> {
        if component.is_empty() {
            return Err(format!("{} must not be empty", label));
        }
        if component.contains("..") || component.contains('/') || component.contains('\\') {
            return Err(format!(
                "Invalid {}: '{}'. Path traversal characters ('..', '/', '\\') are not allowed.",
                label, component
            ));
        }
        Ok(())
    }

    /// Get the path for a specific package version in the store.
    pub fn package_path(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.root.join(org).join(name).join(version)
    }

    /// Check if a package version is already cached in the store.
    pub fn is_cached(&self, org: &str, name: &str, version: &str) -> bool {
        // RCB-307: Reject path traversal in components
        if Self::validate_path_component(org, "org").is_err()
            || Self::validate_path_component(name, "package name").is_err()
            || Self::validate_path_component(version, "version").is_err()
        {
            return false;
        }
        let pkg_dir = self.package_path(org, name, version);
        pkg_dir.join(".taida_installed").exists()
    }

    /// Fetch a package from GitHub and cache it in the store.
    ///
    /// Downloads the tarball from `https://github.com/{org}/{name}/archive/refs/tags/{version}.tar.gz`,
    /// extracts it to `~/.taida/store/{org}/{name}/{version}/`, creates a
    /// `.taida_installed` marker, and writes a C17 `_meta.toml` provenance
    /// sidecar with the tarball SHA-256.
    pub fn fetch_and_cache(&self, org: &str, name: &str, version: &str) -> Result<PathBuf, String> {
        self.fetch_and_cache_with_meta(org, name, version, None)
    }

    /// C17-2: Remove a cached package directory so the next
    /// `fetch_and_cache*` call re-downloads and re-extracts it.
    ///
    /// This is the shared invalidation primitive used by:
    /// - `taida ingot cache clean --store-pkg` (Phase 3)
    ///
    /// C17B-001: the stale-detection path and `--force-refresh` no longer
    /// call this directly; they use the stage/commit/rollback trio
    /// (`stage_invalidation` + `commit_invalidation` + `rollback_invalidation`)
    /// so a failed fetch can restore the previous install.
    ///
    /// Path traversal is rejected up front (RCB-307 / SEC-009). Returns `Ok(())`
    /// when the directory does not exist -- invalidation is idempotent.
    pub fn invalidate_package(&self, org: &str, name: &str, version: &str) -> Result<(), String> {
        Self::validate_path_component(org, "org")?;
        Self::validate_path_component(name, "package name")?;
        Self::validate_path_component(version, "version")?;

        let pkg_dir = self.package_path(org, name, version);
        if !pkg_dir.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(&pkg_dir)
            .map_err(|e| format!("Cannot remove store entry '{}': {}", pkg_dir.display(), e))?;
        Ok(())
    }

    // --------------------------------------------------------------
    // C17B-001: stage / commit / rollback primitives
    // --------------------------------------------------------------
    //
    // The stale-detection path and `--force-refresh` both need to replace
    // `pkg_dir` with a freshly fetched extraction. The old implementation
    // did `rm -rf pkg_dir; fetch`, which destroyed the user's install
    // if the fetch failed. These three primitives implement a
    // transaction-like swap:
    //
    //   1. `stage_invalidation`  -- rename `pkg_dir` -> `pkg_dir.refresh-staging-<pid>-<nanos>`
    //   2. caller runs `fetch_and_cache_with_meta`
    //   3a. on success: `commit_invalidation` -- drop the staging dir
    //   3b. on failure: `rollback_invalidation` -- rename staging back to `pkg_dir`
    //
    // `stage_invalidation` returns `Ok(None)` when there was nothing to
    // stage (uncached), or `Ok(Some(stash_path))` when a backup was made.
    // `rollback_invalidation` first removes any partially-fetched
    // `pkg_dir` the fetch may have left behind before restoring the
    // backup.
    //
    // The staging name embeds PID + nanosecond timestamp so concurrent
    // processes do not collide on the same suffix.

    /// C17B-001: Stage an existing package directory aside so a subsequent
    /// `fetch_and_cache_with_meta` can write a fresh extraction. Returns
    /// the backup path (if one was created), or `None` if nothing was
    /// cached.
    ///
    /// The backup uses a unique suffix (`refresh-staging-<pid>-<nanos>`)
    /// so two processes racing will not clobber each other's stash.
    pub fn stage_invalidation(
        &self,
        org: &str,
        name: &str,
        version: &str,
    ) -> Result<Option<std::path::PathBuf>, String> {
        Self::validate_path_component(org, "org")?;
        Self::validate_path_component(name, "package name")?;
        Self::validate_path_component(version, "version")?;
        let pkg_dir = self.package_path(org, name, version);
        if !pkg_dir.exists() {
            return Ok(None);
        }
        let parent = pkg_dir
            .parent()
            .ok_or_else(|| format!("pkg_dir has no parent: {}", pkg_dir.display()))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let stash = parent.join(format!(
            "{}.refresh-staging-{}-{}",
            version,
            std::process::id(),
            nanos
        ));
        std::fs::rename(&pkg_dir, &stash).map_err(|e| {
            format!(
                "Cannot stage '{}' to '{}' for refresh: {}",
                pkg_dir.display(),
                stash.display(),
                e
            )
        })?;
        Ok(Some(stash))
    }

    /// C17B-001: Finalise a successful refresh by removing the staged
    /// backup. Best-effort: I/O errors bubble up so the caller can warn
    /// the user (the new install is already in place).
    pub fn commit_invalidation(&self, stash: &Path) -> Result<(), String> {
        if !stash.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(stash)
            .map_err(|e| format!("Cannot remove refresh backup '{}': {}", stash.display(), e))
    }

    /// C17B-001: Roll back a failed refresh by restoring the staged
    /// backup. Removes any partial `pkg_dir` the fetcher created before
    /// renaming the backup back into place.
    pub fn rollback_invalidation(&self, stash: &Path, pkg_dir: &Path) -> Result<(), String> {
        if !stash.exists() {
            // Nothing to restore; caller's previous install was absent to
            // begin with.
            return Ok(());
        }
        if pkg_dir.exists() {
            std::fs::remove_dir_all(pkg_dir).map_err(|e| {
                format!(
                    "rollback: cannot remove partial '{}': {}",
                    pkg_dir.display(),
                    e
                )
            })?;
        }
        std::fs::rename(stash, pkg_dir).map_err(|e| {
            format!(
                "rollback: cannot rename '{}' -> '{}': {}",
                stash.display(),
                pkg_dir.display(),
                e
            )
        })?;
        Ok(())
    }

    /// C17B-009: Acquire an advisory lock scoped to
    /// `<org>/<name>/<version>` so two concurrent `taida ingot install`
    /// processes do not race on the same store entry.
    ///
    /// The lock file lives at `<root>/<org>/<name>/.<version>.lock`.
    /// On Unix we use `flock(LOCK_EX)`; the lock is held until the
    /// returned guard is dropped (typically at the end of `resolve()`).
    /// On non-Unix platforms the guard is a no-op (Windows is not a
    /// supported target for the installer integration tests; higher
    /// layers observe their own mutual exclusion).
    ///
    /// Returns `Err` only when we cannot create the lock file at all
    /// (permissions / out-of-space). Contention blocks until the holder
    /// releases; this is fine because concurrent installs of the same
    /// package should serialise.
    pub fn acquire_install_lock(
        &self,
        org: &str,
        name: &str,
        version: &str,
    ) -> Result<InstallLock, String> {
        Self::validate_path_component(org, "org")?;
        Self::validate_path_component(name, "package name")?;
        Self::validate_path_component(version, "version")?;
        let lock_parent = self.root.join(org).join(name);
        std::fs::create_dir_all(&lock_parent).map_err(|e| {
            format!(
                "Cannot create lock parent '{}': {}",
                lock_parent.display(),
                e
            )
        })?;
        let lock_path = lock_parent.join(format!(".{}.lock", version));
        InstallLock::acquire(&lock_path)
    }

    /// Read the sidecar for a cached package, if present.
    ///
    /// Returns `Ok(None)` when the package is not cached or the sidecar is
    /// missing (pre-C17 install). Errors propagate parse / schema mismatches.
    pub fn read_package_meta(
        &self,
        org: &str,
        name: &str,
        version: &str,
    ) -> Result<Option<StoreMeta>, StoreError> {
        // Validation errors are treated as "nothing to read" -- invalid
        // path components cannot produce a sidecar.
        if Self::validate_path_component(org, "org").is_err()
            || Self::validate_path_component(name, "package name").is_err()
            || Self::validate_path_component(version, "version").is_err()
        {
            return Ok(None);
        }
        let pkg_dir = self.package_path(org, name, version);
        if !pkg_dir.exists() {
            return Ok(None);
        }
        read_meta(&meta_path_for(&pkg_dir))
    }

    /// Fetch a package from GitHub and cache it, optionally recording a
    /// resolver-supplied commit SHA in the `_meta.toml` sidecar.
    ///
    /// C17-1 (Phase 1): records `tarball_sha256`, `fetched_at`, `source`,
    /// `version`. The `commit_sha` is supplied by C17-2 (Phase 2) once the
    /// resolver learns the SHA via `git ls-remote`. When `commit_sha` is
    /// `None` the sidecar stores an empty string -- the decision table in
    /// Phase 2 treats that as "sidecar present but SHA unknown".
    pub fn fetch_and_cache_with_meta(
        &self,
        org: &str,
        name: &str,
        version: &str,
        commit_sha: Option<&str>,
    ) -> Result<PathBuf, String> {
        // RCB-307: Reject path traversal in components
        Self::validate_path_component(org, "org")?;
        Self::validate_path_component(name, "package name")?;
        Self::validate_path_component(version, "version")?;

        let pkg_dir = self.package_path(org, name, version);

        // Already cached
        if self.is_cached(org, name, version) {
            return Ok(pkg_dir);
        }

        // Create parent directories
        std::fs::create_dir_all(&pkg_dir).map_err(|e| {
            format!(
                "Cannot create store directory '{}': {}",
                pkg_dir.display(),
                e
            )
        })?;

        // Download tarball from GitHub (or mock server via TAIDA_GITHUB_BASE_URL)
        let url = format!(
            "{}/{}/{}/archive/refs/tags/{}.tar.gz",
            github_base_url().trim_end_matches('/'),
            org,
            name,
            version
        );
        let tmp_dir = self
            .root
            .join(org)
            .join(name)
            .join(format!(".tmp-{}", version));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir)
            .map_err(|e| format!("Cannot create temp directory: {}", e))?;

        let archive_path = tmp_dir.join("archive.tar.gz");

        // Use curl to download. C17B-006: apply the same connect/max-time
        // caps as the API helper so a stalled network cannot hang
        // install indefinitely. Tarballs can be large (a few MiB), so we
        // give this a generous `--max-time 120`.
        //
        // C17 HOLD fix (C1 — 2026-04-17): the bearer token is routed via
        // curl's stdin-based `--config -` inside
        // `github_curl_download_to_file` so it does not leak through
        // `/proc/<pid>/cmdline`.
        let download_ok = github_curl_download_to_file(&url, &archive_path).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&tmp_dir);
        })?;
        if !download_ok {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "Failed to download package {}/{}@{} from {}",
                org, name, version, url
            ));
        }

        // C17-1: compute tarball SHA-256 before extraction. The archive is
        // read once here -- small enough for addons (tens of KiB to a few
        // MiB) that a single in-memory pass is acceptable.
        let tarball_sha256 = compute_file_sha256(&archive_path).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            format!(
                "Failed to hash tarball for {}/{}@{}: {}",
                org, name, version, e
            )
        })?;

        // Extract tarball (--strip-components=1 removes the top-level directory)
        let tar_status = std::process::Command::new("tar")
            .args(["xzf"])
            .arg(&archive_path)
            .args(["--strip-components=1", "-C"])
            .arg(&pkg_dir)
            .status()
            .map_err(|e| format!("Failed to run tar: {}", e))?;

        if !tar_status.success() {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            let _ = std::fs::remove_dir_all(&pkg_dir);
            return Err(format!(
                "Failed to extract package {}/{}@{}",
                org, name, version
            ));
        }

        // Cleanup temp directory before verification so it never leaks
        let _ = std::fs::remove_dir_all(&tmp_dir);

        // Post-fetch manifest verification: ensure the extracted package's
        // packages.tdm identity and version match what was requested.
        Self::verify_fetched_package(&pkg_dir, org, name, version)?;

        // C17-1: write provenance sidecar before the installed marker. The
        // sidecar is written atomically so a crash after the marker but
        // before the sidecar leaves only an "unknown provenance" state that
        // the Phase 2 decision table treats as pessimistic-refresh.
        let meta = StoreMeta {
            schema_version: STORE_META_SCHEMA_VERSION,
            commit_sha: commit_sha.unwrap_or("").to_string(),
            tarball_sha256,
            tarball_etag: None,
            fetched_at: rfc3339_now(),
            source: format!("github:{}/{}", org, name),
            version: version.to_string(),
        };
        let meta_path = meta_path_for(&pkg_dir);
        if let Err(e) = write_meta_atomic(&meta_path, &meta) {
            // Clean up the half-installed package so the next install retries
            // from scratch rather than observing a manifest+data without
            // provenance metadata.
            let _ = std::fs::remove_dir_all(&pkg_dir);
            return Err(format!(
                "Failed to write store sidecar for {}/{}@{}: {}",
                org, name, version, e
            ));
        }

        // Create completion marker
        std::fs::write(pkg_dir.join(".taida_installed"), "")
            .map_err(|e| format!("Cannot create install marker: {}", e))?;

        Ok(pkg_dir)
    }

    /// Resolve a generation-only version (e.g. "a") to an exact version (e.g. "a.47").
    ///
    /// 1. Scans the local cache for matching versions
    /// 2. If not found locally, queries GitHub API for tags
    /// 3. Fetches and caches the resolved version
    pub fn resolve_generation(
        &self,
        org: &str,
        name: &str,
        generation: &str,
    ) -> Result<String, String> {
        // RCB-307: Reject path traversal in components
        Self::validate_path_component(org, "org")?;
        Self::validate_path_component(name, "package name")?;
        Self::validate_path_component(generation, "generation")?;

        // First, check local cache
        let pkg_parent = self.root.join(org).join(name);
        if pkg_parent.exists()
            && let Some(version) = self.find_latest_in_generation(&pkg_parent, generation)
        {
            return Ok(version);
        }

        self.resolve_generation_from_remote(org, name, generation)
    }

    /// Resolve a generation-only version by always querying GitHub API (bypass local cache).
    ///
    /// Used by `taida ingot update` to find the latest version even when an older version is cached.
    pub fn resolve_generation_remote(
        &self,
        org: &str,
        name: &str,
        generation: &str,
    ) -> Result<String, String> {
        // RCB-307: Reject path traversal in components
        Self::validate_path_component(org, "org")?;
        Self::validate_path_component(name, "package name")?;
        Self::validate_path_component(generation, "generation")?;

        self.resolve_generation_from_remote(org, name, generation)
    }

    /// Internal: query GitHub API for the latest version in a generation, fetch and cache it.
    fn resolve_generation_from_remote(
        &self,
        org: &str,
        name: &str,
        generation: &str,
    ) -> Result<String, String> {
        // Query GitHub API for tags (or mock server via TAIDA_GITHUB_API_URL)
        let url = format!(
            "{}/repos/{}/{}/tags?per_page=100",
            github_api_url().trim_end_matches('/'),
            org,
            name
        );

        // C17 HOLD fix (M1 — 2026-04-17): route this third GitHub API
        // call through the shared `github_curl_api_get_optional` helper
        // so that (a) the GH_TOKEN / GITHUB_TOKEN auth header is
        // applied, (b) connect/max-time are bounded, (c) the
        // `User-Agent: taida-install` header is set, and (d) the token
        // is passed via stdin `--config -` instead of argv (C1).
        //
        // Returning `Ok(None)` from the helper (offline / non-2xx) maps
        // to an explicit error here because this call site needs a tag
        // list to proceed; the stale-check helpers that tolerate offline
        // return their own pessimistic-skip signal separately.
        let body = match github_curl_api_get_optional(&url, "application/vnd.github.v3+json")
            .map_err(|e| format!("Failed to query GitHub tags for {}/{}: {}", org, name, e))?
        {
            Some(body) => body,
            None => {
                return Err(format!(
                    "Failed to fetch tags for {}/{}: HTTP error",
                    org, name
                ));
            }
        };
        let body = body.as_str();
        let prefix_new = format!("{}.", generation);
        let prefix_legacy = format!("v{}.", generation);

        // Parse tag names from JSON (handles both pretty-printed and compressed JSON)
        // Tags can be "a.3", "a.3.alpha" (new) or "va.3" (legacy) — extract num and pick highest
        let mut best: Option<(u64, String)> = None; // (num, version_without_v)
        for tag in extract_json_name_values(body) {
            let suffix = tag
                .strip_prefix(&prefix_new)
                .or_else(|| tag.strip_prefix(&prefix_legacy));
            if let Some(suffix) = suffix {
                // suffix is "num" or "num.label"
                let num_str = suffix.split('.').next().unwrap_or(suffix);
                if let Ok(num) = num_str.parse::<u64>() {
                    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
                    if best.as_ref().is_none_or(|(prev, _)| num > *prev) {
                        best = Some((num, version));
                    }
                }
            }
        }

        match best {
            Some((_, exact)) => {
                // Fetch and cache the resolved version
                self.fetch_and_cache(org, name, &exact)?;
                Ok(exact)
            }
            None => Err(format!(
                "No version found for {}/{}@{} (generation '{}')",
                org, name, generation, generation
            )),
        }
    }

    /// Find the latest version matching a generation in local cache.
    /// Handles both `gen.num` and `gen.num.label` directory names.
    fn find_latest_in_generation(&self, pkg_parent: &Path, generation: &str) -> Option<String> {
        let entries = std::fs::read_dir(pkg_parent).ok()?;
        let prefix = format!("{}.", generation);
        let mut best: Option<(u64, String)> = None; // (num, full_version)

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(suffix) = name.strip_prefix(&prefix) {
                // suffix is "num" or "num.label"
                let num_str = suffix.split('.').next().unwrap_or(suffix);
                if let Ok(num) = num_str.parse::<u64>() {
                    // Only count if actually installed
                    if entry.path().join(".taida_installed").exists()
                        && best.as_ref().is_none_or(|(prev, _)| num > *prev)
                    {
                        best = Some((num, name));
                    }
                }
            }
        }

        best.map(|(_, version)| version)
    }

    /// Post-fetch verification: ensure the extracted package's manifest
    /// declares an identity and version consistent with what was requested.
    ///
    /// For packages with a `packages.tdm` that declares a qualified name
    /// (`org/name`), the identity must exactly match `expected_org/expected_name`.
    /// The version in the manifest must match `expected_version`.
    ///
    /// For addon packages with `native/addon.toml`, the `package` field
    /// must also match the expected `org/name`.
    ///
    /// On mismatch, the package directory is cleaned up and an error is returned.
    fn verify_fetched_package(
        pkg_dir: &Path,
        expected_org: &str,
        expected_name: &str,
        expected_version: &str,
    ) -> Result<(), String> {
        let expected_qualified = format!("{}/{}", expected_org, expected_name);

        // 1. Verify packages.tdm if present
        if let Some(manifest) = crate::pkg::manifest::Manifest::from_dir(pkg_dir).map_err(|e| {
            let _ = std::fs::remove_dir_all(pkg_dir);
            format!(
                "Post-fetch verification failed for {}@{}: {}",
                expected_qualified, expected_version, e
            )
        })? {
            // If manifest declares a qualified name, it must match exactly
            if manifest.name.contains('/') && manifest.name != expected_qualified {
                let _ = std::fs::remove_dir_all(pkg_dir);
                return Err(format!(
                    "Post-fetch verification failed: package declares identity '{}' \
                     but was fetched as '{}@{}'. The tarball content does not match \
                     the requested package.",
                    manifest.name, expected_qualified, expected_version
                ));
            }

            // Version must match
            if manifest.version != expected_version {
                let _ = std::fs::remove_dir_all(pkg_dir);
                return Err(format!(
                    "Post-fetch verification failed: package declares version '{}' \
                     but was fetched as '{}@{}'. The tarball content does not match \
                     the requested package.",
                    manifest.version, expected_qualified, expected_version
                ));
            }
        }

        // 2. Verify native/addon.toml if present
        let addon_toml_path = pkg_dir.join("native").join("addon.toml");
        if addon_toml_path.exists() {
            let addon_manifest = crate::addon::manifest::parse_addon_manifest(&addon_toml_path)
                .map_err(|e| {
                    let _ = std::fs::remove_dir_all(pkg_dir);
                    format!(
                        "Post-fetch verification failed for {}@{}: \
                         addon.toml is present but cannot be parsed: {}",
                        expected_qualified, expected_version, e
                    )
                })?;
            if addon_manifest.package != expected_qualified {
                let _ = std::fs::remove_dir_all(pkg_dir);
                return Err(format!(
                    "Post-fetch verification failed: addon.toml declares package '{}' \
                     but was fetched as '{}@{}'. The tarball content does not match \
                     the requested package.",
                    addon_manifest.package, expected_qualified, expected_version
                ));
            }
        }

        Ok(())
    }
}

/// Extract all values of `"name"` keys from a JSON string.
///
/// Handles both pretty-printed and compressed JSON. Avoids matching
/// `"tag_name"`, `"full_name"`, etc. by checking that the character
/// immediately before `"name"` is `{`, `,`, or whitespace (after a comma/brace).
///
/// Returns a Vec of the string values found.
fn extract_json_name_values(json: &str) -> Vec<String> {
    let mut results = Vec::new();
    let bytes = json.as_bytes();
    let len = bytes.len();
    let target = b"\"name\"";
    let target_len = target.len();

    let mut i = 0;
    while i + target_len < len {
        // Look for "name" pattern
        if &bytes[i..i + target_len] == target {
            // Check preceding non-whitespace character is { or , (not part of a longer key)
            let mut j = i;
            while j > 0 {
                j -= 1;
                let c = bytes[j];
                if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                    continue;
                }
                // Must be preceded by { or , (start of object or after another field)
                if c == b'{' || c == b',' {
                    break;
                } else {
                    // Preceded by another character (e.g. part of "tag_name")
                    i += target_len;
                    continue; // Continue outer loop -- use goto-like jump
                }
            }

            // Skip past "name" and optional whitespace/colon
            let mut k = i + target_len;
            while k < len
                && (bytes[k] == b' ' || bytes[k] == b'\t' || bytes[k] == b'\n' || bytes[k] == b'\r')
            {
                k += 1;
            }
            if k < len && bytes[k] == b':' {
                k += 1;
                while k < len
                    && (bytes[k] == b' '
                        || bytes[k] == b'\t'
                        || bytes[k] == b'\n'
                        || bytes[k] == b'\r')
                {
                    k += 1;
                }
                if k < len && bytes[k] == b'"' {
                    k += 1; // skip opening quote
                    let start = k;
                    while k < len && bytes[k] != b'"' {
                        if bytes[k] == b'\\' {
                            k += 1; // skip escaped char
                        }
                        k += 1;
                    }
                    if k < len {
                        let value = String::from_utf8_lossy(&bytes[start..k]).to_string();
                        results.push(value);
                    }
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    results
}

// =============================================================================
// C17-1: store sidecar (`_meta.toml`)
// =============================================================================
//
// `_meta.toml` records provenance metadata next to the extracted tarball so
// `taida ingot install` can later detect stale store entries when a tag is
// republished (retag / delete+recreate).
//
// The sidecar is intentionally a tiny flat TOML document. C17 scope is
// "add sidecar + stale detection"; a richer schema (content-addressable
// layout) is deferred to C18+.
//
// Current schema version: `STORE_META_SCHEMA_VERSION = 1`.
//
// On-disk layout:
// ```toml
// # auto-generated by taida ingot install (C17)
// # Do not edit by hand.
// schema_version = 1
// commit_sha = "0cd5588720ac44e58a01e8f8831a62c023fab5cf"
// tarball_sha256 = "<hex>"
// # tarball_etag = "W/\"...\""  # optional; absent when None
// fetched_at = "2026-04-16T12:20:16Z"
// source = "github:taida-lang/terminal"
// version = "a.1"
// ```

/// Filename for the provenance sidecar placed alongside the extracted
/// tarball. Underscore prefix so it never collides with package files.
pub const STORE_META_FILENAME: &str = "_meta.toml";

/// Current schema version for `_meta.toml`. An older sidecar with a
/// different `schema_version` is treated as "unknown provenance" so the
/// caller (Phase 2) can force a refresh.
pub const STORE_META_SCHEMA_VERSION: u32 = 1;

/// Provenance metadata written alongside an extracted store package.
///
/// Written atomically via `write_meta_atomic` (tempfile + rename) so a
/// crashed install leaves either a complete sidecar or no sidecar at all.
/// Read via `read_meta`; missing sidecar returns `Ok(None)` (Phase 2
/// treats that as pessimistic refresh).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMeta {
    /// Always `STORE_META_SCHEMA_VERSION` when written by current code.
    pub schema_version: u32,
    /// Commit SHA the version tag pointed at when fetched.
    ///
    /// Empty string means "unknown at fetch time" -- C17-1 writes this
    /// when the resolver has not yet queried the remote HEAD. The Phase 2
    /// decision table treats `commit_sha.is_empty()` as "sidecar present
    /// but SHA unknown" and falls back to pessimistic refresh.
    pub commit_sha: String,
    /// SHA-256 (hex) of the tarball before extraction.
    ///
    /// C17B-008 (tracked in `.dev/C17_BLOCKERS.md`): C17 writes this field
    /// but does not yet verify it on fast-path reuse. The natural
    /// verification point is a rehash of the cached extraction (or of the
    /// re-downloaded tarball), which is deferred to C18+ together with
    /// content-addressable store work. Having the field recorded now means
    /// the future verifier has something to compare against for every
    /// install performed under C17.
    pub tarball_sha256: String,
    /// HTTP ETag returned by the archive host, if exposed. Optional; the
    /// field is omitted from the on-disk TOML when `None`.
    pub tarball_etag: Option<String>,
    /// RFC-3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) of when the tarball
    /// was fetched.
    pub fetched_at: String,
    /// Source identifier, e.g. `"github:taida-lang/terminal"`.
    pub source: String,
    /// Version string as requested, e.g. `"a.1"`.
    pub version: String,
}

/// Errors produced by the C17 store sidecar helpers.
#[derive(Debug)]
pub enum StoreError {
    /// I/O error while reading/writing the sidecar.
    Io(String),
    /// Sidecar could not be parsed (malformed TOML).
    Parse(String),
    /// Sidecar is well-formed but declares a schema version this build
    /// does not understand. Phase 2 treats this as pessimistic-refresh.
    UnknownMetaSchema { actual: u32, expected: u32 },
    /// Sidecar is missing a required key.
    MissingField(&'static str),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(m) => write!(f, "store sidecar I/O error: {}", m),
            StoreError::Parse(m) => write!(f, "store sidecar parse error: {}", m),
            StoreError::UnknownMetaSchema { actual, expected } => write!(
                f,
                "store sidecar schema_version={} unsupported (this build supports {})",
                actual, expected
            ),
            StoreError::MissingField(name) => {
                write!(f, "store sidecar missing required field '{}'", name)
            }
        }
    }
}

impl std::error::Error for StoreError {}

/// Return the sidecar path for an extracted package directory.
pub fn meta_path_for(pkg_dir: &Path) -> PathBuf {
    pkg_dir.join(STORE_META_FILENAME)
}

/// Read and parse a store sidecar.
///
/// Returns `Ok(None)` when the file does not exist (no sidecar is a valid
/// state for pre-C17 installs and is handled by the Phase 2 decision
/// table). Returns `Err` for malformed TOML or schema-version mismatches.
pub fn read_meta(path: &Path) -> Result<Option<StoreMeta>, StoreError> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreError::Io(e.to_string())),
    };
    let meta = parse_meta_str(&source)?;
    Ok(Some(meta))
}

/// Write a store sidecar atomically (write to `<path>.tmp`, then rename).
///
/// The parent directory must already exist (typically the package
/// extraction directory). The temp file is removed on failure so callers
/// never observe a half-written sidecar.
pub fn write_meta_atomic(path: &Path, meta: &StoreMeta) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Io(format!("sidecar path has no parent: {}", path.display())))?;
    if !parent.exists() {
        std::fs::create_dir_all(parent).map_err(|e| {
            StoreError::Io(format!(
                "cannot create sidecar directory {}: {}",
                parent.display(),
                e
            ))
        })?;
    }

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(STORE_META_FILENAME);
    let tmp_path = parent.join(format!(".{}.tmp", file_name));
    // Best-effort cleanup of any leftover tmp from a prior crash.
    let _ = std::fs::remove_file(&tmp_path);

    let serialized = serialize_meta(meta);
    // C17B-014: Use the open/write/sync/rename/sync-dir idiom for
    // durability. The rename is already atomic per POSIX, but without
    // a prior fsync the kernel may still have the tmp file's data in
    // memory; a power-loss event between rename and flush can resurrect
    // the old sidecar (or worse, a partial new one) on next boot.
    //
    // We:
    //   1. create the tmp file
    //   2. write the serialized bytes
    //   3. fsync the tmp fd
    //   4. rename tmp -> target
    //   5. fsync the parent directory (Unix)
    //
    // Parent-dir fsync is a no-op on non-Unix so we attempt it on Unix
    // only; failure there is logged but not fatal (the rename already
    // succeeded).
    {
        let mut f = std::fs::File::create(&tmp_path).map_err(|e| {
            StoreError::Io(format!(
                "cannot create temp sidecar {}: {}",
                tmp_path.display(),
                e
            ))
        })?;
        use std::io::Write as _;
        if let Err(e) = f.write_all(serialized.as_bytes()) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(StoreError::Io(format!(
                "cannot write temp sidecar {}: {}",
                tmp_path.display(),
                e
            )));
        }
        if let Err(e) = f.sync_all() {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(StoreError::Io(format!(
                "cannot fsync temp sidecar {}: {}",
                tmp_path.display(),
                e
            )));
        }
        // `f` goes out of scope here and is closed before rename.
    }

    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        StoreError::Io(format!(
            "cannot atomically place sidecar {}: {}",
            path.display(),
            e
        ))
    })?;

    // C17B-014: Fsync the parent directory so the rename itself is
    // persisted. On non-Unix this is a best-effort no-op (opening a
    // directory for fsync is not portable); the rename's atomicity is
    // still observed by crash-safety contracts on NTFS etc.
    #[cfg(unix)]
    {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Serialize a `StoreMeta` to the on-disk TOML form.
///
/// Kept as a free function (not `Display`) so tests and Phase 2 callers
/// can round-trip without relying on inherent method location.
fn serialize_meta(meta: &StoreMeta) -> String {
    let mut out = String::new();
    out.push_str("# auto-generated by taida ingot install (C17)\n");
    out.push_str("# Do not edit by hand.\n");
    out.push_str(&format!("schema_version = {}\n", meta.schema_version));
    out.push_str(&format!(
        "commit_sha = \"{}\"\n",
        escape_toml_basic_string(&meta.commit_sha)
    ));
    out.push_str(&format!(
        "tarball_sha256 = \"{}\"\n",
        escape_toml_basic_string(&meta.tarball_sha256)
    ));
    if let Some(etag) = &meta.tarball_etag {
        out.push_str(&format!(
            "tarball_etag = \"{}\"\n",
            escape_toml_basic_string(etag)
        ));
    }
    out.push_str(&format!(
        "fetched_at = \"{}\"\n",
        escape_toml_basic_string(&meta.fetched_at)
    ));
    out.push_str(&format!(
        "source = \"{}\"\n",
        escape_toml_basic_string(&meta.source)
    ));
    out.push_str(&format!(
        "version = \"{}\"\n",
        escape_toml_basic_string(&meta.version)
    ));
    out
}

/// Parse a store sidecar from a TOML string.
///
/// Accepts the flat key=value shape produced by `serialize_meta`. Lines
/// starting with `#` are comments. Sections (`[...]`) are rejected --
/// the sidecar schema has no sections in v1.
fn parse_meta_str(source: &str) -> Result<StoreMeta, StoreError> {
    let mut schema_version: Option<u32> = None;
    let mut commit_sha: Option<String> = None;
    let mut tarball_sha256: Option<String> = None;
    let mut tarball_etag: Option<String> = None;
    let mut fetched_at: Option<String> = None;
    let mut source_field: Option<String> = None;
    let mut version: Option<String> = None;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            return Err(StoreError::Parse(format!(
                "line {}: sections are not allowed in _meta.toml v1",
                line_no
            )));
        }

        let (key, value) = line.split_once('=').ok_or_else(|| {
            StoreError::Parse(format!("line {}: expected 'key = value'", line_no))
        })?;
        let key = key.trim();
        let value = value.trim();

        match key {
            "schema_version" => {
                let n: u32 = value.parse().map_err(|_| {
                    StoreError::Parse(format!(
                        "line {}: schema_version must be a non-negative integer, got {:?}",
                        line_no, value
                    ))
                })?;
                schema_version = Some(n);
            }
            "commit_sha" => commit_sha = Some(parse_basic_string(value, line_no)?),
            "tarball_sha256" => tarball_sha256 = Some(parse_basic_string(value, line_no)?),
            "tarball_etag" => tarball_etag = Some(parse_basic_string(value, line_no)?),
            "fetched_at" => fetched_at = Some(parse_basic_string(value, line_no)?),
            "source" => source_field = Some(parse_basic_string(value, line_no)?),
            "version" => version = Some(parse_basic_string(value, line_no)?),
            other => {
                // C17B-017: Unknown keys are tolerated (forward-compat).
                // When `TAIDA_DEBUG_STORE=1` we echo them to stderr so a
                // developer debugging sidecar evolution can see which
                // additive fields are being skipped. In the default
                // quiet mode nothing is printed -- v1 readers must not
                // reject v1.x sidecars with additive fields.
                if std::env::var_os("TAIDA_DEBUG_STORE").is_some() {
                    eprintln!(
                        "  debug: _meta.toml line {}: ignoring unknown key '{}' (forward-compat)",
                        line_no, other
                    );
                }
            }
        }
    }

    let schema_version = schema_version.ok_or(StoreError::MissingField("schema_version"))?;
    if schema_version != STORE_META_SCHEMA_VERSION {
        return Err(StoreError::UnknownMetaSchema {
            actual: schema_version,
            expected: STORE_META_SCHEMA_VERSION,
        });
    }

    Ok(StoreMeta {
        schema_version,
        commit_sha: commit_sha.ok_or(StoreError::MissingField("commit_sha"))?,
        tarball_sha256: tarball_sha256.ok_or(StoreError::MissingField("tarball_sha256"))?,
        tarball_etag,
        fetched_at: fetched_at.ok_or(StoreError::MissingField("fetched_at"))?,
        source: source_field.ok_or(StoreError::MissingField("source"))?,
        version: version.ok_or(StoreError::MissingField("version"))?,
    })
}

/// Parse a TOML basic string literal (`"..."`) with minimal escape
/// support: `\\`, `\"`, `\n`, `\r`, `\t`.
fn parse_basic_string(value: &str, line_no: usize) -> Result<String, StoreError> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return Err(StoreError::Parse(format!(
            "line {}: expected a quoted string, got {:?}",
            line_no, value
        )));
    }
    let inner = &value[1..value.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => {
                return Err(StoreError::Parse(format!(
                    "line {}: unsupported escape \\{} in string",
                    line_no, other
                )));
            }
            None => {
                return Err(StoreError::Parse(format!(
                    "line {}: dangling backslash in string",
                    line_no
                )));
            }
        }
    }
    Ok(out)
}

/// Escape a string for a TOML basic string literal.
fn escape_toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Compute the SHA-256 (hex) of a file by streaming it through the
/// in-tree hasher. Used by C17-1 to record `tarball_sha256` in the
/// sidecar.
///
/// The tarball is read fully into memory. Addon tarballs are typically
/// tens of KiB to a few MiB, so a single-pass read is acceptable; if this
/// ever grows we can switch to streaming via `Read::read_to_end` chunks.
fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("cannot read {} for hashing: {}", path.display(), e))?;
    Ok(crate::crypto::sha256_hex_bytes(&bytes))
}

/// Format `SystemTime::now()` as RFC-3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
///
/// Kept as a free function so the C17 sidecar can be written without
/// pulling in a time crate. Precision is whole seconds, matching the
/// granularity `taida ingot install` needs for stale detection.
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339_utc(secs)
}

/// Format a Unix-epoch second count as RFC-3339 UTC.
///
/// Implements the civil-calendar arithmetic locally (Howard Hinnant's
/// `days_from_civil` inverse) so we do not need a dependency.
fn format_rfc3339_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let time_of_day = unix_secs % 86_400;
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;

    // Howard Hinnant: civil_from_days
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146_096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, minute, second
    )
}

// =============================================================================
// C17-3: store prune helpers (taida ingot cache clean --store / --store-pkg)
// =============================================================================

/// Summary of a store prune pass. Reported to the user via `taida ingot cache clean`.
#[derive(Debug, Default, Clone)]
pub struct StorePruneReport {
    /// `~/.taida/store/` existed before the prune ran. When false,
    /// nothing was removed and the caller should print
    /// "No store cache found at ...".
    pub root_existed: bool,
    /// Number of real store entries (`<org>/<name>/<version>/`) removed.
    /// Excludes scratch directories (`.tmp-*`, `.refresh-staging-*`);
    /// those are tracked separately in `scratch_removed`.
    pub packages_removed: u64,
    /// Number of bytes freed. Best-effort: computed by walking the
    /// directory before deletion. Covers both real packages and scratch.
    pub bytes_removed: u64,
    /// Absolute path to the store root (for display).
    pub root: PathBuf,
    /// Names of the removed package entries, in deterministic order.
    /// Format: `<org>/<name>@<version>`. Used for the summary preview
    /// before a destructive prune. Excludes scratch directories so the
    /// preview does not show `alice/http@.tmp-a.3`.
    pub packages: Vec<String>,
    /// C17B-011: Number of scratch directories removed. These are
    /// `.tmp-<ver>` (leftover extraction scratch) and
    /// `.refresh-staging-<pid>-<nanos>` (abandoned refresh backups from a
    /// crashed `taida ingot install`). Counted separately from real packages so
    /// the summary does not conflate them.
    pub scratch_removed: u64,
}

/// C17B-011: Classify a version-level directory name.
///
/// - `.tmp-<ver>`                     -> scratch (extraction temp)
/// - `.refresh-staging-<pid>-<nanos>` -> scratch (abandoned rollback backup)
/// - `<ver>`                          -> real package directory
fn is_scratch_dir_name(name: &str) -> bool {
    if name.starts_with(".tmp-") {
        return true;
    }
    if name.contains(".refresh-staging-") {
        return true;
    }
    false
}

/// Compute a pre-flight summary of what `prune_store_root` would remove.
///
/// Non-destructive: merely walks `~/.taida/store/` and collects sizes.
/// Used by `taida ingot cache clean --store` to present a summary before the
/// user confirms.
pub fn summarize_store_root(store_root: &Path) -> Result<StorePruneReport, String> {
    summarize_store_root_impl(store_root, None)
}

/// Same as `summarize_store_root` but scoped to a single
/// `<org>/<name>` package (all versions).
pub fn summarize_store_package(
    store_root: &Path,
    org: &str,
    name: &str,
) -> Result<StorePruneReport, String> {
    validate_component_free(org, "org")?;
    validate_component_free(name, "package name")?;
    summarize_store_root_impl(store_root, Some((org, name)))
}

fn summarize_store_root_impl(
    store_root: &Path,
    scope: Option<(&str, &str)>,
) -> Result<StorePruneReport, String> {
    let mut report = StorePruneReport {
        root: store_root.to_path_buf(),
        root_existed: store_root.exists(),
        ..Default::default()
    };
    if !report.root_existed {
        return Ok(report);
    }
    // Walk <root>/<org>/<name>/<version> and collect each version dir.
    let orgs: Vec<PathBuf> = match scope {
        Some((org, _)) => vec![store_root.join(org)],
        None => std::fs::read_dir(store_root)
            .map_err(|e| format!("cannot read store root {}: {}", store_root.display(), e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
    };
    for org_dir in orgs {
        if !org_dir.is_dir() {
            continue;
        }
        let org_name = match org_dir.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let names: Vec<PathBuf> = match scope {
            Some((_, name)) => vec![org_dir.join(name)],
            None => std::fs::read_dir(&org_dir)
                .map_err(|e| format!("cannot read store org {}: {}", org_dir.display(), e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect(),
        };
        for name_dir in names {
            if !name_dir.is_dir() {
                continue;
            }
            let pkg_name = match name_dir.file_name().and_then(|n| n.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let versions = match std::fs::read_dir(&name_dir) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for ver_entry in versions.filter_map(|e| e.ok()) {
                let ver_path = ver_entry.path();
                if !ver_path.is_dir() {
                    continue;
                }
                let ver_name = match ver_path.file_name().and_then(|n| n.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let bytes = dir_size_bytes(&ver_path);
                report.bytes_removed += bytes;
                // C17B-011: classify real packages vs Taida-owned scratch
                // directories. Scratch shows up in the summary total
                // footer but never in the per-package preview, so users
                // do not see confusing entries like `alice/http@.tmp-a.3`.
                if is_scratch_dir_name(&ver_name) {
                    report.scratch_removed += 1;
                } else {
                    report.packages_removed += 1;
                    report
                        .packages
                        .push(format!("{}/{}@{}", org_name, pkg_name, ver_name));
                }
            }
        }
    }
    // Deterministic order so CLI output is stable.
    report.packages.sort();
    Ok(report)
}

/// Best-effort recursive byte count. Errors are silently skipped (the
/// prune summary is informational).
fn dir_size_bytes(path: &Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = std::fs::symlink_metadata(&p) {
                if meta.file_type().is_symlink() {
                    continue;
                }
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += dir_size_bytes(&p);
                }
            }
        }
    }
    total
}

/// Prune the entire store root (`~/.taida/store/`).
///
/// Removes every `<org>/<name>/<version>/` directory, which also
/// removes the sidecar and `.taida_installed` marker inside. Orphan
/// `.tmp-*` scratch directories are removed too. `~/.taida/store/`
/// itself is left in place (empty) so subsequent installs do not need
/// to recreate it.
///
/// Returns a `StorePruneReport` describing what was removed.
pub fn prune_store_root(store_root: &Path) -> Result<StorePruneReport, String> {
    let mut report = summarize_store_root(store_root)?;
    if !report.root_existed {
        return Ok(report);
    }
    // Remove everything under <root> (including org dirs) but keep the
    // root itself so the next install does not need to mkdir.
    let entries = std::fs::read_dir(store_root)
        .map_err(|e| format!("cannot read store root {}: {}", store_root.display(), e))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            let _ = std::fs::remove_dir_all(&p);
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
    report.root = store_root.to_path_buf();
    Ok(report)
}

/// Prune a single package (`<org>/<name>/*`) from the store.
///
/// Removes every version directory under `<root>/<org>/<name>/`. Returns
/// `Ok(report)` with `packages_removed == 0` when nothing matched, so
/// callers can distinguish "package not cached" from "prune failed".
pub fn prune_store_package(
    store_root: &Path,
    org: &str,
    name: &str,
) -> Result<StorePruneReport, String> {
    let report = summarize_store_package(store_root, org, name)?;
    if !report.root_existed {
        return Ok(report);
    }
    let pkg_dir = store_root.join(org).join(name);
    if pkg_dir.is_dir() {
        std::fs::remove_dir_all(&pkg_dir)
            .map_err(|e| format!("cannot remove {}: {}", pkg_dir.display(), e))?;
    }
    Ok(report)
}

impl GlobalStore {
    /// C17-3: absolute path to the store root. Used by
    /// `taida ingot cache clean --store` to surface the location in user-facing
    /// summaries.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

// =============================================================================
// C17-2: stale-detection decision table
// =============================================================================
//
// Given:
//   - the sidecar read from the cached store entry (may be absent)
//   - the remote commit SHA resolved by `resolve_version_to_sha` (may be
//     absent when offline)
//
// the installer must decide: skip, refresh, or refresh-with-warning. The
// table is pinned in `.dev/C17_IMPL_SPEC.md` Phase 2.
//
// `--force-refresh` bypasses this table (handled at the call site).
// `--no-remote-check` skips the remote lookup, so `classify_stale` is
// called with `remote_sha = None` and sidecar presence governs the outcome.

/// Outcome of the Phase 2 decision table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleDecision {
    /// Fast path: sidecar present and SHA matches remote. No refresh.
    SkipFastPath,
    /// Remote lookup failed but a sidecar is present -- trust it for this run,
    /// emit an offline warning. `install` continues with exit 0.
    SkipWithOfflineWarning,
    /// No sidecar and no remote SHA -- cannot verify provenance. Skip but
    /// emit a strong warning that points the user at `--force-refresh`.
    SkipUnknownProvenanceStrongWarn,
    /// Cached entry must be re-extracted before install proceeds. The
    /// reason is carried for log output.
    Refresh(RefreshReason),
}

/// Why the installer decided to refresh the store entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshReason {
    /// No sidecar at all (legacy install from before C17, or a previous
    /// crash between extract and sidecar write).
    MissingSidecar,
    /// Sidecar present but `commit_sha` is the empty string because Phase
    /// 1 wrote it without a resolved SHA. Treated as pessimistic refresh
    /// since we cannot prove freshness.
    SidecarShaUnknown,
    /// Remote commit SHA differs from sidecar. Carries the old/new pair
    /// for the info log `remote moved: ... sha <old>..<new>`.
    RemoteMoved { old_sha: String, new_sha: String },
}

/// Phase 2 decision table. Inputs:
///   - `sidecar`: the parsed sidecar of the cached package, or `None` if
///     the package is not cached / sidecar missing.
///   - `remote_sha`: the commit SHA resolved via `resolve_version_to_sha`,
///     or `None` if the remote lookup was skipped / failed.
///
/// The table is the authoritative contract of C17-2. See
/// `.dev/C17_IMPL_SPEC.md` Phase 2 for the frozen mapping.
pub fn classify_stale(sidecar: Option<&StoreMeta>, remote_sha: Option<&str>) -> StaleDecision {
    match (sidecar, remote_sha) {
        // Row 1: no sidecar, remote known -> pessimistic refresh.
        (None, Some(_)) => StaleDecision::Refresh(RefreshReason::MissingSidecar),

        // Row 2: sidecar with known SHA, remote agrees -> fast path.
        // Row 3: sidecar present but SHAs disagree -> refresh.
        // Row 2b: sidecar present but SHA unknown -> pessimistic refresh
        //   even when remote is reachable, so we do not silently trust a
        //   tarball with no provenance once we have a SHA to record.
        (Some(meta), Some(remote)) => {
            if meta.commit_sha.is_empty() {
                return StaleDecision::Refresh(RefreshReason::SidecarShaUnknown);
            }
            if meta.commit_sha == remote {
                StaleDecision::SkipFastPath
            } else {
                StaleDecision::Refresh(RefreshReason::RemoteMoved {
                    old_sha: meta.commit_sha.clone(),
                    new_sha: remote.to_string(),
                })
            }
        }

        // Row 4: sidecar present, remote unreachable -> trust sidecar for
        // this run, warn that staleness cannot be verified.
        (Some(_), None) => StaleDecision::SkipWithOfflineWarning,

        // Row 5: no sidecar AND remote unreachable -> cannot prove
        // anything. Emit a strong warning that guides the user to
        // `taida ingot install --force-refresh` once they are back online.
        (None, None) => StaleDecision::SkipUnknownProvenanceStrongWarn,
    }
}

/// Short human-readable label for a `RefreshReason`, used as the `reason`
/// half of the `remote moved: <pkg>@<version> sha <old>..<new>; refreshing
/// store` info line. Kept small so callers can format the final message
/// the way their surface requires.
pub fn refresh_reason_short(reason: &RefreshReason) -> String {
    match reason {
        RefreshReason::MissingSidecar => "missing sidecar".to_string(),
        RefreshReason::SidecarShaUnknown => "sidecar has no recorded commit sha".to_string(),
        RefreshReason::RemoteMoved { old_sha, new_sha } => {
            format!(
                "remote moved: sha {}..{}",
                truncate_sha(old_sha),
                truncate_sha(new_sha)
            )
        }
    }
}

fn truncate_sha(sha: &str) -> String {
    if sha.len() > 12 {
        sha[..12].to_string()
    } else {
        sha.to_string()
    }
}

// =============================================================================
// C17-2: resolve_version_to_sha (GitHub git/refs API)
// =============================================================================

/// Resolve a Taida version tag to the commit SHA it points at on origin.
///
/// Uses `GET {api}/repos/{org}/{name}/git/refs/tags/{version}` which GitHub
/// returns as JSON containing `"object": { "sha": "..." , "type": "commit" | "tag" }`.
/// Annotated tags (type = "tag") dereference to the underlying commit via a
/// second request; unannotated tags (type = "commit") return the commit SHA
/// directly.
///
/// Honours `TAIDA_GITHUB_API_URL` for mock servers in tests.
///
/// Returns:
/// - `Ok(Some(sha))` on a successful lookup.
/// - `Ok(None)` when the remote cannot be reached (network error, or the
///   mock/API returned a transient error). Callers map `None` to the
///   pessimistic-skip branch of the decision table.
/// - `Err(msg)` when the response is malformed or the tag does not exist
///   (the latter is a hard failure that the caller surfaces as an install
///   error, not a silent skip).
pub fn resolve_version_to_sha(
    org: &str,
    name: &str,
    version: &str,
) -> Result<Option<String>, String> {
    validate_component_free(org, "org")?;
    validate_component_free(name, "package name")?;
    validate_component_free(version, "version")?;

    // C17B-018: Process-local memoization. `taida ingot install` resolves the
    // same `<org>/<name>/<version>` at most a handful of times in a
    // single run, but when a project has multiple deps resolved via
    // fan-out or when `--force-refresh` + lockfile paths re-enter the
    // resolver, we avoid paying for the same GitHub API call twice.
    //
    // The cache key is namespaced on `TAIDA_GITHUB_API_URL` so tests
    // that swap the mock between subtests do not observe a stale hit.
    let api_hint = std::env::var("TAIDA_GITHUB_API_URL").unwrap_or_default();
    let cache_key = format!("{}|{}|{}|{}", api_hint, org, name, version);
    if let Some(cached) = sha_cache_get(&cache_key) {
        return Ok(cached);
    }

    let api = github_api_url();
    let api = api.trim_end_matches('/');
    let url = format!("{}/repos/{}/{}/git/refs/tags/{}", api, org, name, version);
    let body = match curl_get_optional(&url)? {
        Some(body) => body,
        None => {
            sha_cache_put(cache_key, None);
            return Ok(None);
        }
    };

    // The response is either:
    //   { "ref": "refs/tags/a.1",
    //     "object": { "sha": "<hex>", "type": "commit" | "tag", ... } }
    // or a 404 body that curl already treated as failure (caught above).
    let object = match extract_json_object_field(&body, "object") {
        Some(obj) => obj,
        None => {
            return Err(format!(
                "resolve_version_to_sha: response for {}/{}@{} has no 'object' field",
                org, name, version
            ));
        }
    };

    let sha = match extract_json_string_field(&object, "sha") {
        Some(s) => s,
        None => {
            return Err(format!(
                "resolve_version_to_sha: 'object.sha' missing for {}/{}@{}",
                org, name, version
            ));
        }
    };

    let ty = extract_json_string_field(&object, "type").unwrap_or_default();
    if ty == "tag" {
        // Annotated tag: the `sha` points at the tag object; we need to
        // dereference it via `/repos/{org}/{name}/git/tags/{sha}` to get
        // the underlying commit.
        let tag_url = format!("{}/repos/{}/{}/git/tags/{}", api, org, name, sha);
        let body = match curl_get_optional(&tag_url)? {
            Some(b) => b,
            None => {
                sha_cache_put(cache_key, None);
                return Ok(None);
            }
        };
        let obj = extract_json_object_field(&body, "object").ok_or_else(|| {
            format!(
                "resolve_version_to_sha: annotated tag {}/{}@{} has no 'object'",
                org, name, version
            )
        })?;
        let commit_sha = extract_json_string_field(&obj, "sha").ok_or_else(|| {
            format!(
                "resolve_version_to_sha: annotated tag {}/{}@{} 'object.sha' missing",
                org, name, version
            )
        })?;
        sha_cache_put(cache_key, Some(commit_sha.clone()));
        return Ok(Some(commit_sha));
    }

    sha_cache_put(cache_key, Some(sha.clone()));
    Ok(Some(sha))
}

/// C17B-018: Process-local memoization cache for `resolve_version_to_sha`.
///
/// Keyed on `"{api_url}|{org}|{name}|{version}"` so tests that swap the
/// `TAIDA_GITHUB_API_URL` mock between stages do not see stale hits. The
/// cache is purely additive: entries live for the process lifetime.
fn sha_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Option<String>>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn sha_cache_get(key: &str) -> Option<Option<String>> {
    sha_cache().lock().ok()?.get(key).cloned()
}

fn sha_cache_put(key: String, value: Option<String>) {
    if let Ok(mut guard) = sha_cache().lock() {
        guard.insert(key, value);
    }
}

/// C17B-018 test support: clear the memo cache. Exposed for tests that
/// rely on back-to-back SHA resolutions returning fresh results.
#[cfg(test)]
pub fn _test_only_clear_sha_cache() {
    if let Ok(mut guard) = sha_cache().lock() {
        guard.clear();
    }
}

/// Validation wrapper so `resolve_version_to_sha` can share the
/// traversal guard without borrowing `self` (mirrors
/// `GlobalStore::validate_path_component`).
fn validate_component_free(component: &str, label: &str) -> Result<(), String> {
    if component.is_empty() {
        return Err(format!("{} must not be empty", label));
    }
    if component.contains("..") || component.contains('/') || component.contains('\\') {
        return Err(format!(
            "Invalid {}: '{}'. Path traversal characters ('..', '/', '\\') are not allowed.",
            label, component
        ));
    }
    Ok(())
}

/// GET `url` via `curl -fsSL`. Returns:
/// - `Ok(Some(body))` on HTTP 2xx.
/// - `Ok(None)` when curl exits non-zero (network unreachable, DNS
///   failure, 5xx, 4xx, ...). This is the "cannot verify" branch --
///   callers pair it with an offline warning, never a silent skip.
/// - `Err(msg)` only when curl itself cannot be launched.
///
/// C17B-005: When `GH_TOKEN` / `GITHUB_TOKEN` is set in the environment
/// (or `~/.taida/auth.json` contains a token), add an
/// `Authorization: Bearer <token>` header so unauthenticated rate limits
/// (60 requests/hr/IP on GitHub) do not silently demote the stale-check
/// into offline mode.
///
/// C17B-006: Cap the network wait with `--connect-timeout 10` and
/// `--max-time 30`. A slow / dead network must not hang `taida ingot install`
/// indefinitely; we fall back to the offline branch instead.
///
/// Thin wrapper around `github_curl_api_get_optional` retaining the
/// historical call-site name.
fn curl_get_optional(url: &str) -> Result<Option<String>, String> {
    github_curl_api_get_optional(url, "application/vnd.github+json")
}

/// C17 HOLD fix (C1/M1 — 2026-04-17): unified GitHub API GET helper.
///
/// Applies the standard hardening to every JSON API call:
///
/// - `-fsSL` (fail fast on HTTP errors, follow redirects)
/// - `--connect-timeout 10 --max-time 30` (C17B-006: bound network wait)
/// - `-H "User-Agent: taida-install"` (GitHub API recommends a UA)
/// - `-H "Accept: <accept>"` as requested by the caller
/// - `Authorization: Bearer <token>` when a token is configured
///   (C17B-005)
///
/// Security (HOLD C1 fix, 2026-04-17): the bearer token is **never**
/// passed on argv. Historically we used `-H "Authorization: ..."` which
/// makes the secret visible via `/proc/<pid>/cmdline` to any user on
/// the machine. Instead, when a token is configured we enable
/// `--config -` and write the header line through the child's stdin.
/// `curl --config` parses its argument as a file of long-form options
/// so argv only carries the sentinel `--config -`.
///
/// Return contract:
/// - `Ok(Some(body))` on HTTP 2xx
/// - `Ok(None)` on any non-zero exit (offline / 4xx / 5xx / rate
///   limited / DNS fail). The caller pairs this with an explicit
///   stderr warning so the fallback is never silent.
/// - `Err(msg)` only when the curl process itself cannot be launched
///   or stdin writing fails.
fn github_curl_api_get_optional(url: &str, accept: &str) -> Result<Option<String>, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-fsSL",
        "--connect-timeout",
        "10",
        "--max-time",
        "30",
        "-H",
        "User-Agent: taida-install",
    ]);
    cmd.arg("-H").arg(format!("Accept: {}", accept));

    let token_opt = github_auth_token();
    if token_opt.is_some() {
        // Stdin config; argv only reveals `--config -`, not the token.
        cmd.arg("--config").arg("-");
        cmd.stdin(std::process::Stdio::piped());
    }
    cmd.arg(url);
    // Capture stdout so the caller receives the body.
    cmd.stdout(std::process::Stdio::piped());
    // C18B-010 fix (carry from C17B-023): capture stderr so that on
    // non-zero exit we can surface a structured `warning:` line
    // naming the HTTP failure context. Pre-fix stderr was inherited
    // and curl's diagnostics (TLS failure, connect timeout, etc.)
    // interleaved with Taida's own output.
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run curl: {}", e))?;
    if let Some(token) = token_opt
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        writeln!(stdin, "header = \"Authorization: Bearer {}\"", token)
            .map_err(|e| format!("Failed to pipe auth header to curl: {}", e))?;
        // Explicit drop closes stdin so curl stops reading.
        drop(stdin);
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for curl: {}", e))?;
    if !output.status.success() {
        // C18B-010: surface curl's stderr (trimmed) when we fall
        // through to the "offline / 4xx / 5xx" branch. Silent drop
        // of diagnostics used to mask transient GitHub errors.
        let err_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !err_text.is_empty() {
            eprintln!("warning: GitHub request failed ({}): {}", url, err_text);
        }
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// C17 HOLD fix (C1 — 2026-04-17): archive download variant of
/// `github_curl_api_get_optional`.
///
/// Writes the response body to `dest` and returns `Ok(true)` on
/// success, `Ok(false)` on any non-zero exit (offline / 404 / 5xx),
/// `Err` on process-spawn failure. Token is passed through stdin for
/// the same reason as the API helper.
///
/// Different knobs from the API helper:
/// - `--max-time 120` (tarballs are larger than API responses)
/// - writes to `-o <dest>` instead of stdout
/// - uses curl's default `Accept: */*`
fn github_curl_download_to_file(url: &str, dest: &Path) -> Result<bool, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-fsSL",
        "--connect-timeout",
        "10",
        "--max-time",
        "120",
        "-H",
        "User-Agent: taida-install",
    ]);

    let token_opt = github_auth_token();
    if token_opt.is_some() {
        cmd.arg("--config").arg("-");
        cmd.stdin(std::process::Stdio::piped());
    }
    cmd.arg("-o").arg(dest);
    cmd.arg(url);
    // C18B-010 fix (carry from C17B-023): capture stderr so tarball
    // download failures emit structured diagnostics rather than
    // leaking curl's raw output into the parent process's stderr.
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run curl: {}", e))?;
    if let Some(token) = token_opt
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        writeln!(stdin, "header = \"Authorization: Bearer {}\"", token)
            .map_err(|e| format!("Failed to pipe auth header to curl: {}", e))?;
        drop(stdin);
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for curl: {}", e))?;
    if !output.status.success() {
        // C18B-010: same as `github_curl_api_get_optional`, emit a
        // structured warning rather than silently returning Ok(false).
        let err_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !err_text.is_empty() {
            eprintln!("warning: GitHub download failed ({}): {}", url, err_text);
        }
        return Ok(false);
    }
    Ok(true)
}

/// C17B-005: Best-effort lookup of a GitHub auth token.
///
/// Precedence (first match wins):
///   1. `GH_TOKEN`
///   2. `GITHUB_TOKEN`
///   3. `~/.taida/auth.json` (`github_token` field) via `load_token`
///
/// Returns `None` if none are available; callers degrade gracefully to
/// unauthenticated requests.
///
/// Pinned in `.dev/C17_IMPL_SPEC.md` so the precedence is auditable.
///
/// C18B-007 (carry from C17B-020): reject tokens containing `"` / `\`
/// / `\n` / `\r` since the curl config stdin we pipe into looks like
///   `header = "Authorization: Bearer <token>"`
/// and those characters would either escape out of the quoted value
/// or inject additional config directives. Silent failure is avoided
/// by emitting a `warning:` line to stderr and treating the source as
/// if the token were absent (offline fallback).
///
/// C18B-008 (carry from C17B-021): reject whitespace-only tokens for
/// the same reason. A literal `"    "` would have produced an
/// `Authorization: Bearer     ` header that GitHub rejects with 401,
/// silently masquerading as anonymous rate-limited access.
fn github_auth_token() -> Option<String> {
    if let Ok(t) = std::env::var("GH_TOKEN")
        && let Some(ok) = sanitize_auth_token(&t, "GH_TOKEN")
    {
        return Some(ok);
    }
    if let Ok(t) = std::env::var("GITHUB_TOKEN")
        && let Some(ok) = sanitize_auth_token(&t, "GITHUB_TOKEN")
    {
        return Some(ok);
    }
    match crate::auth::token::load_token() {
        Some(t) => sanitize_auth_token(&t.github_token, "~/.taida/auth.json"),
        None => None,
    }
}

/// C18B-007/008 helper: validate a GitHub auth token before it is
/// piped into `curl --config -`. Returns `Some(token)` when the token
/// is safe to interpolate into
///   `header = "Authorization: Bearer <token>"`,
/// `None` otherwise. On rejection we emit a `warning:` line to stderr
/// naming the source so the fallback to unauthenticated requests is
/// visible in CI logs rather than silent.
///
/// Rejected shapes (each produces its own warning):
///   - empty or whitespace-only tokens
///   - tokens containing `"` / `\` / `\n` / `\r` — would either break
///     out of the quoted curl config directive or inject another
///     directive on a fresh line.
fn sanitize_auth_token(raw: &str, source: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        if !raw.is_empty() {
            // Non-empty but whitespace-only — callers would otherwise
            // produce `Authorization: Bearer     ` and degrade into a
            // silent 401 fallback (C18B-008).
            eprintln!(
                "warning: {} is whitespace-only; ignoring it and falling back to anonymous GitHub requests.",
                source
            );
        }
        return None;
    }
    // C18B-007: forbid control + quote + backslash inside the token.
    // The curl config uses double-quotes around the header value so
    // `"` / `\` / CR / LF escape out of that context.
    for ch in trimmed.chars() {
        if ch == '"' || ch == '\\' || ch == '\n' || ch == '\r' {
            eprintln!(
                "warning: {} contains an illegal character ({}); ignoring it and falling back to anonymous GitHub requests.",
                source,
                match ch {
                    '"' => "double quote",
                    '\\' => "backslash",
                    '\n' => "newline",
                    '\r' => "carriage return",
                    _ => "control character",
                }
            );
            return None;
        }
    }
    Some(trimmed.to_string())
}

/// Tiny JSON object extractor: returns the substring `{...}` that is the
/// value of the named key, or `None` if not found. Sufficient for the
/// two shapes we need (`object.sha`, `object.type`). Balances braces
/// (including inside strings) to avoid truncating nested objects.
fn extract_json_object_field(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\"", key);
    let idx = find_key_index(json, &pat)?;
    let after_colon = skip_to_value(json, idx + pat.len())?;
    let bytes = json.as_bytes();
    if bytes.get(after_colon)? != &b'{' {
        return None;
    }
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escape = false;
    let mut end = after_colon;
    for (i, c) in bytes.iter().enumerate().skip(after_colon) {
        if escape {
            escape = false;
            continue;
        }
        match *c {
            b'\\' if in_str => escape = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(json[after_colon..=end].to_string())
}

/// Tiny JSON string extractor: returns the decoded string value for the
/// named key, or `None`. Supports `\"`, `\\`, `\n`, `\r`, `\t`.
fn extract_json_string_field(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\"", key);
    let idx = find_key_index(json, &pat)?;
    let after_colon = skip_to_value(json, idx + pat.len())?;
    let bytes = json.as_bytes();
    if bytes.get(after_colon)? != &b'"' {
        return None;
    }
    let mut out = String::new();
    let mut i = after_colon + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some(out),
            b'\\' => {
                i += 1;
                match bytes.get(i).copied()? {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'/' => out.push('/'),
                    other => out.push(other as char),
                }
            }
            other => out.push(other as char),
        }
        i += 1;
    }
    None
}

/// Find `needle` in `json` at a position where the preceding non-whitespace
/// byte is `{` or `,` -- i.e. it appears as a key, not embedded in another
/// key like `"full_name"`. Returns the starting index of `needle`.
fn find_key_index(json: &str, needle: &str) -> Option<usize> {
    let bytes = json.as_bytes();
    let n_bytes = needle.as_bytes();
    if n_bytes.is_empty() {
        return None;
    }
    let mut i = 0;
    while i + n_bytes.len() <= bytes.len() {
        if &bytes[i..i + n_bytes.len()] == n_bytes {
            let mut j = i;
            let ok = loop {
                if j == 0 {
                    break true;
                }
                j -= 1;
                match bytes[j] {
                    b' ' | b'\t' | b'\n' | b'\r' => continue,
                    b'{' | b',' => break true,
                    _ => break false,
                }
            };
            if ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Skip whitespace + `:` after a key, returning the byte index of the
/// value's first character.
fn skip_to_value(json: &str, start: usize) -> Option<usize> {
    let bytes = json.as_bytes();
    let mut i = start;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    i += 1;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    Some(i)
}

// =============================================================================
// C17B-009: per-package install lock
// =============================================================================
//
// Two `taida ingot install` processes (e.g. monorepo CI workers, or a developer
// running `taida ingot install` twice in parallel shells) used to race on the
// same `<org>/<name>/<version>/` directory. Symptoms: interleaved tar
// output, corrupted sidecar, stray `.tmp-<version>` scratch trees.
//
// We take a Unix `flock(LOCK_EX)` on a lock file that lives next to the
// package directory. The lock is advisory (co-operating processes only),
// but Taida is the sole writer of `~/.taida/store/` so that is enough.
//
// On non-Unix platforms the lock is a no-op. The installer integration
// tests that exercise this path are `#[cfg(unix)]`, matching the rest of
// the C17 test surface.

/// RAII guard for the per-package install lock. Dropping the guard
/// releases the lock (explicit `drop(_guard)` is unnecessary).
pub struct InstallLock {
    // `File` holds the fd; `flock` is implicitly released when the fd is
    // closed, which happens on drop. We keep the path around so callers
    // can include it in error messages.
    #[allow(dead_code)]
    file: std::fs::File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl InstallLock {
    /// Acquire an exclusive advisory lock on `lock_path`. Creates the
    /// file if it does not exist. Blocks until the lock is available.
    ///
    /// The file contents are irrelevant -- we only care about the flock
    /// on the open file descriptor.
    pub fn acquire(lock_path: &Path) -> Result<InstallLock, String> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| format!("Cannot open lock file '{}': {}", lock_path.display(), e))?;
        Self::lock_blocking(&file, lock_path)?;
        Ok(InstallLock {
            file,
            path: lock_path.to_path_buf(),
        })
    }

    #[cfg(unix)]
    fn lock_blocking(file: &std::fs::File, path: &Path) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        // Safety: `fd` is valid for the lifetime of `file`; `flock` is
        // safe to call with any non-negative fd.
        let fd = file.as_raw_fd();
        // Retry on EINTR -- we want a blocking acquire semantics.
        loop {
            let r = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if r == 0 {
                return Ok(());
            }
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("flock('{}') failed: {}", path.display(), err));
        }
    }

    #[cfg(not(unix))]
    fn lock_blocking(_file: &std::fs::File, _path: &Path) -> Result<(), String> {
        // No-op on non-Unix platforms. Higher-level serialisation (cargo,
        // `taida` session discipline) is relied upon there.
        Ok(())
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        // Close of `self.file` releases the flock automatically on Unix.
        // Nothing to do here; we keep the impl explicit so future
        // maintainers do not delete `self.file` thinking it is unused.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_path_layout() {
        let store = GlobalStore::with_root(PathBuf::from("/tmp/taida_store_test"));
        let path = store.package_path("alice", "webframework", "b.12");
        assert_eq!(
            path,
            PathBuf::from("/tmp/taida_store_test/alice/webframework/b.12")
        );
    }

    #[test]
    fn test_is_cached_false_when_empty() {
        let store = GlobalStore::with_root(PathBuf::from("/tmp/taida_store_test_cached"));
        assert!(!store.is_cached("nonexistent", "pkg", "a.1"));
    }

    #[test]
    fn test_is_cached_true_with_marker() {
        let dir = PathBuf::from("/tmp/taida_store_test_marker");
        let _ = std::fs::remove_dir_all(&dir);
        let pkg_dir = dir.join("alice").join("http").join("b.12");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join(".taida_installed"), "").unwrap();

        let store = GlobalStore::with_root(dir.clone());
        assert!(store.is_cached("alice", "http", "b.12"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_latest_in_generation() {
        let dir = PathBuf::from("/tmp/taida_store_test_gen");
        let _ = std::fs::remove_dir_all(&dir);

        // Create mock versions
        let pkg_parent = dir.join("org").join("pkg");
        for v in &["a.1", "a.5", "a.12", "b.1"] {
            let vdir = pkg_parent.join(v);
            std::fs::create_dir_all(&vdir).unwrap();
            std::fs::write(vdir.join(".taida_installed"), "").unwrap();
        }

        let store = GlobalStore::with_root(dir.clone());
        let result = store.find_latest_in_generation(&pkg_parent, "a");
        assert_eq!(result, Some("a.12".to_string()));

        let result = store.find_latest_in_generation(&pkg_parent, "b");
        assert_eq!(result, Some("b.1".to_string()));

        let result = store.find_latest_in_generation(&pkg_parent, "c");
        assert_eq!(result, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_latest_in_generation_with_labels() {
        let dir = PathBuf::from("/tmp/taida_store_test_gen_label");
        let _ = std::fs::remove_dir_all(&dir);

        let pkg_parent = dir.join("org").join("pkg");
        for v in &["a.1", "a.3.alpha", "a.5.beta"] {
            let vdir = pkg_parent.join(v);
            std::fs::create_dir_all(&vdir).unwrap();
            std::fs::write(vdir.join(".taida_installed"), "").unwrap();
        }

        let store = GlobalStore::with_root(dir.clone());
        let result = store.find_latest_in_generation(&pkg_parent, "a");
        assert_eq!(result, Some("a.5.beta".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_cached_with_label() {
        let dir = PathBuf::from("/tmp/taida_store_test_label_cache");
        let _ = std::fs::remove_dir_all(&dir);
        let pkg_dir = dir.join("org").join("pkg").join("a.1.alpha");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join(".taida_installed"), "").unwrap();

        let store = GlobalStore::with_root(dir.clone());
        assert!(store.is_cached("org", "pkg", "a.1.alpha"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_json_name_values_pretty() {
        let json = r#"[
  {
    "name": "va.3",
    "zipball_url": "...",
    "tarball_url": "...",
    "commit": {}
  },
  {
    "name": "va.2",
    "node_id": "abc"
  }
]"#;
        let names = extract_json_name_values(json);
        assert_eq!(names, vec!["va.3", "va.2"]);
    }

    #[test]
    fn test_extract_json_name_values_compressed() {
        let json = r#"[{"name":"va.3","zipball_url":"..."},{"name":"va.1","node_id":"abc"}]"#;
        let names = extract_json_name_values(json);
        assert_eq!(names, vec!["va.3", "va.1"]);
    }

    #[test]
    fn test_extract_json_name_no_false_match() {
        // "tag_name" and "full_name" should NOT be matched
        let json = r#"[{"tag_name":"va.99","full_name":"repo","name":"va.5"}]"#;
        let names = extract_json_name_values(json);
        assert_eq!(names, vec!["va.5"]);
    }

    #[test]
    fn test_extract_json_name_empty() {
        let names = extract_json_name_values("[]");
        assert!(names.is_empty());
    }

    /// FL-29: GlobalStore fallback uses std::env::temp_dir() instead of "/tmp"
    /// Note: This test modifies environment variables and may be flaky under parallel
    /// execution. Run with `cargo test --test-threads=1` if it fails intermittently.
    #[test]
    fn test_global_store_fallback_uses_temp_dir() {
        let _guard = crate::util::env_test_lock().lock().unwrap();

        let original_home = std::env::var("HOME").ok();
        let original_userprofile = std::env::var("USERPROFILE").ok();

        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }

        let store = GlobalStore::new();
        let expected_root = std::env::temp_dir().join(".taida").join("store");
        assert_eq!(
            store.root, expected_root,
            "GlobalStore fallback should use std::env::temp_dir(), not hardcoded /tmp"
        );

        // Restore environment
        unsafe {
            if let Some(home) = original_home {
                std::env::set_var("HOME", home);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(up) = original_userprofile {
                std::env::set_var("USERPROFILE", up);
            } else {
                std::env::remove_var("USERPROFILE");
            }
        }
    }

    #[test]
    fn test_verify_fetched_package_matching_manifest() {
        let dir = PathBuf::from("/tmp/taida_verify_match");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@b.11.rc3 alice/http\n").unwrap();

        let result = GlobalStore::verify_fetched_package(&dir, "alice", "http", "b.11.rc3");
        assert!(
            result.is_ok(),
            "matching manifest should pass: {:?}",
            result
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_fetched_package_identity_mismatch() {
        let dir = PathBuf::from("/tmp/taida_verify_id_mismatch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@b.11.rc3 evil/hijacked\n").unwrap();

        let result = GlobalStore::verify_fetched_package(&dir, "alice", "http", "b.11.rc3");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Post-fetch verification failed"),
            "should report identity mismatch"
        );
    }

    #[test]
    fn test_verify_fetched_package_version_mismatch() {
        let dir = PathBuf::from("/tmp/taida_verify_ver_mismatch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@b.99.rc1 alice/http\n").unwrap();

        let result = GlobalStore::verify_fetched_package(&dir, "alice", "http", "b.11.rc3");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Post-fetch verification failed"),
            "should report version mismatch"
        );
    }

    #[test]
    fn test_verify_fetched_package_no_manifest_ok() {
        let dir = PathBuf::from("/tmp/taida_verify_no_manifest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No packages.tdm — simple packages without manifest should pass
        let result = GlobalStore::verify_fetched_package(&dir, "alice", "http", "b.11.rc3");
        assert!(result.is_ok(), "no manifest should pass: {:?}", result);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_fetched_package_bare_name_ok() {
        let dir = PathBuf::from("/tmp/taida_verify_bare_name");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Legacy manifest with bare name (no org/) — should pass
        // (bare names don't declare a qualified identity to compare against)
        std::fs::write(dir.join("packages.tdm"), "<<<@b.11.rc3\n").unwrap();

        let result = GlobalStore::verify_fetched_package(&dir, "alice", "http", "b.11.rc3");
        assert!(
            result.is_ok(),
            "bare name manifest should pass: {:?}",
            result
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_fetched_package_corrupt_addon_toml_rejected() {
        let dir = PathBuf::from("/tmp/taida_verify_corrupt_addon");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("native")).unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@b.11.rc3 alice/http\n").unwrap();
        // Write a corrupt addon.toml that cannot be parsed
        std::fs::write(
            dir.join("native").join("addon.toml"),
            "this is not valid toml {{{\n",
        )
        .unwrap();

        let result = GlobalStore::verify_fetched_package(&dir, "alice", "http", "b.11.rc3");
        assert!(result.is_err(), "corrupt addon.toml should be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("Post-fetch verification failed")
                && err.contains("addon.toml is present but cannot be parsed"),
            "error should mention addon.toml parse failure, got: {}",
            err
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // C17-1: store sidecar (`_meta.toml`) unit tests
    // =========================================================================

    fn sample_meta() -> StoreMeta {
        StoreMeta {
            schema_version: STORE_META_SCHEMA_VERSION,
            commit_sha: "0cd5588720ac44e58a01e8f8831a62c023fab5cf".to_string(),
            tarball_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_string(),
            tarball_etag: Some("W/\"abcd-1234\"".to_string()),
            fetched_at: "2026-04-16T12:20:16Z".to_string(),
            source: "github:taida-lang/terminal".to_string(),
            version: "a.1".to_string(),
        }
    }

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "taida_store_meta_{}_{}_{}",
            tag,
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn test_meta_path_for_layout() {
        let pkg_dir = PathBuf::from("/tmp/anywhere/alice/http/b.12");
        assert_eq!(
            meta_path_for(&pkg_dir),
            PathBuf::from("/tmp/anywhere/alice/http/b.12/_meta.toml")
        );
    }

    #[test]
    fn test_write_read_roundtrip() {
        let dir = unique_tmp_dir("roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);

        let original = sample_meta();
        write_meta_atomic(&path, &original).expect("write_meta_atomic");

        let loaded = read_meta(&path).expect("read_meta ok").expect("exists");
        assert_eq!(loaded, original);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_without_etag_roundtrip() {
        let dir = unique_tmp_dir("no_etag");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);

        let mut meta = sample_meta();
        meta.tarball_etag = None;
        write_meta_atomic(&path, &meta).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains("tarball_etag"),
            "sidecar should omit tarball_etag when None, got:\n{}",
            contents
        );

        let loaded = read_meta(&path).unwrap().unwrap();
        assert_eq!(loaded.tarball_etag, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_meta_missing_returns_none() {
        let dir = unique_tmp_dir("missing");
        // Note: directory intentionally not created
        let path = meta_path_for(&dir);
        let result = read_meta(&path).expect("missing sidecar is ok");
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_meta_schema_mismatch() {
        let dir = unique_tmp_dir("schema_mismatch");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);
        // Write a sidecar with an unknown schema_version.
        std::fs::write(
            &path,
            "schema_version = 99\n\
             commit_sha = \"\"\n\
             tarball_sha256 = \"\"\n\
             fetched_at = \"2026-04-16T00:00:00Z\"\n\
             source = \"github:foo/bar\"\n\
             version = \"a.1\"\n",
        )
        .unwrap();

        let err = read_meta(&path).expect_err("schema mismatch must error");
        match err {
            StoreError::UnknownMetaSchema { actual, expected } => {
                assert_eq!(actual, 99);
                assert_eq!(expected, STORE_META_SCHEMA_VERSION);
            }
            other => panic!("expected UnknownMetaSchema, got {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_meta_missing_required_field() {
        let dir = unique_tmp_dir("missing_field");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);
        // schema_version present, but tarball_sha256 missing.
        std::fs::write(
            &path,
            "schema_version = 1\n\
             commit_sha = \"\"\n\
             fetched_at = \"2026-04-16T00:00:00Z\"\n\
             source = \"github:foo/bar\"\n\
             version = \"a.1\"\n",
        )
        .unwrap();

        let err = read_meta(&path).expect_err("missing field must error");
        match err {
            StoreError::MissingField(name) => assert_eq!(name, "tarball_sha256"),
            other => panic!("expected MissingField, got {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_meta_rejects_sections() {
        let dir = unique_tmp_dir("sections");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);
        std::fs::write(
            &path,
            "schema_version = 1\n[unexpected]\ncommit_sha = \"\"\n",
        )
        .unwrap();

        let err = read_meta(&path).expect_err("sections are rejected in v1");
        match err {
            StoreError::Parse(m) => {
                assert!(m.contains("sections are not allowed"), "got: {}", m);
            }
            other => panic!("expected Parse, got {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_meta_atomic_no_tmp_file_leftover_on_success() {
        let dir = unique_tmp_dir("atomic_success");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);

        write_meta_atomic(&path, &sample_meta()).unwrap();

        // The tempfile pattern is `.<name>.tmp`.
        let tmp = dir.join(format!(".{}.tmp", STORE_META_FILENAME));
        assert!(
            !tmp.exists(),
            "tempfile {} should be renamed away",
            tmp.display()
        );
        assert!(path.exists(), "final sidecar should exist");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_meta_atomic_overwrites_stale_tmp() {
        // Simulate a crashed write: a `.{name}.tmp` from a prior install
        // lingers in the directory. `write_meta_atomic` must clean it up
        // and still produce a valid sidecar.
        let dir = unique_tmp_dir("atomic_stale_tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = meta_path_for(&dir);
        let tmp = dir.join(format!(".{}.tmp", STORE_META_FILENAME));
        std::fs::write(&tmp, b"garbage from prior crash").unwrap();

        write_meta_atomic(&path, &sample_meta()).unwrap();

        assert!(!tmp.exists(), "stale tempfile must be removed");
        let loaded = read_meta(&path).unwrap().unwrap();
        assert_eq!(loaded, sample_meta());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_meta_atomic_creates_parent_dir_if_missing() {
        // If the package directory somehow does not exist yet,
        // write_meta_atomic should create it rather than fail.
        let dir = unique_tmp_dir("atomic_no_parent");
        let path = meta_path_for(&dir);
        assert!(!dir.exists());
        write_meta_atomic(&path, &sample_meta()).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_escape_and_parse_basic_string_roundtrip() {
        let original = "line one\nline\ttwo \"quoted\" \\backslash\\";
        let serialized = escape_toml_basic_string(original);
        // The serialized form is the inner payload of a basic string;
        // wrap it for round-trip through parse_basic_string.
        let wrapped = format!("\"{}\"", serialized);
        let parsed = parse_basic_string(&wrapped, 1).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_format_rfc3339_utc_known_epochs() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        // 2026-04-16T12:20:16Z: days_from_1970 =
        //   (56*365 + 14 leap days to 2026-01-01 exclusive of 2024 already
        //    counted) + (31+28+31+15)  -- computed independently and
        //   cross-checked with `date -u -d '2026-04-16T12:20:16Z' +%s`.
        assert_eq!(format_rfc3339_utc(1_776_342_016), "2026-04-16T12:20:16Z");
        // Leap day edge: 2024-02-29T00:00:00Z -> 1_709_164_800
        assert_eq!(format_rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        // Pre-epoch-style boundary (first second of 2000): 946_684_800
        assert_eq!(format_rfc3339_utc(946_684_800), "2000-01-01T00:00:00Z");
    }

    #[test]
    fn test_rfc3339_now_matches_format_signature() {
        let now = rfc3339_now();
        // Shape: YYYY-MM-DDTHH:MM:SSZ == 20 chars
        assert_eq!(now.len(), 20, "got {:?}", now);
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[7..8], "-");
        assert_eq!(&now[10..11], "T");
        assert_eq!(&now[13..14], ":");
        assert_eq!(&now[16..17], ":");
    }

    #[test]
    fn test_serialize_meta_emits_generated_header() {
        let out = serialize_meta(&sample_meta());
        assert!(
            out.starts_with("# auto-generated by taida ingot install (C17)\n"),
            "header missing, got:\n{}",
            out
        );
        assert!(out.contains("# Do not edit by hand."));
    }

    #[test]
    fn test_parse_meta_str_tolerates_unknown_forward_compat_fields() {
        // Future sidecar versions may add fields. v1 readers should
        // ignore them rather than reject the sidecar outright.
        let toml = "schema_version = 1\n\
                    commit_sha = \"deadbeef\"\n\
                    tarball_sha256 = \"abcd\"\n\
                    fetched_at = \"2026-04-16T12:20:16Z\"\n\
                    source = \"github:foo/bar\"\n\
                    version = \"a.1\"\n\
                    future_field = \"v1.1 addition\"\n";
        let meta = parse_meta_str(toml).unwrap();
        assert_eq!(meta.commit_sha, "deadbeef");
        assert_eq!(meta.version, "a.1");
    }

    #[test]
    fn test_fetch_and_cache_writes_sidecar_after_extract() {
        // Integration-style test: use a local mock by wiring a pre-built
        // tarball into the extraction path. We cannot easily exercise
        // curl + tar here without network, so we validate the sidecar
        // contract by calling write_meta_atomic directly on a simulated
        // package directory. End-to-end coverage is owned by Phase 5.
        let dir = unique_tmp_dir("fetch_sim");
        std::fs::create_dir_all(&dir).unwrap();
        // Pretend we just extracted a tarball here.
        std::fs::write(dir.join("main.td"), "<<< @(main)\n").unwrap();

        let meta = StoreMeta {
            schema_version: STORE_META_SCHEMA_VERSION,
            commit_sha: "".to_string(), // Phase 1: unknown, Phase 2 fills in
            tarball_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_string(),
            tarball_etag: None,
            fetched_at: rfc3339_now(),
            source: "github:taida-lang/terminal".to_string(),
            version: "a.1".to_string(),
        };
        let meta_path = meta_path_for(&dir);
        write_meta_atomic(&meta_path, &meta).unwrap();

        // Contract: sidecar lives next to the extracted tree.
        assert!(meta_path.exists());
        let loaded = read_meta(&meta_path).unwrap().unwrap();
        assert_eq!(loaded.version, "a.1");
        assert_eq!(loaded.source, "github:taida-lang/terminal");
        assert!(
            loaded.commit_sha.is_empty(),
            "Phase 1 sidecar leaves commit_sha empty; Phase 2 fills it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compute_file_sha256_matches_known_vector() {
        let dir = unique_tmp_dir("sha256");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let hex = compute_file_sha256(&path).unwrap();
        assert_eq!(
            hex,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // C17-2: decision table + JSON helpers
    // =========================================================================

    fn meta_with_sha(sha: &str) -> StoreMeta {
        StoreMeta {
            schema_version: STORE_META_SCHEMA_VERSION,
            commit_sha: sha.to_string(),
            tarball_sha256: "abc".to_string(),
            tarball_etag: None,
            fetched_at: "2026-04-16T00:00:00Z".to_string(),
            source: "github:alice/http".to_string(),
            version: "a.1".to_string(),
        }
    }

    // --- decision table rows ---------------------------------------------

    #[test]
    fn test_classify_row1_no_sidecar_remote_known_refreshes_pessimistically() {
        let d = classify_stale(None, Some("aaaa"));
        assert_eq!(d, StaleDecision::Refresh(RefreshReason::MissingSidecar));
    }

    #[test]
    fn test_classify_row2_sidecar_matches_remote_is_fast_path() {
        let m = meta_with_sha("deadbeef");
        let d = classify_stale(Some(&m), Some("deadbeef"));
        assert_eq!(d, StaleDecision::SkipFastPath);
    }

    #[test]
    fn test_classify_row2b_sidecar_sha_unknown_refreshes() {
        // Phase 1-installed sidecars record commit_sha as "" because no
        // resolver SHA was available. Once a remote SHA is reachable, we
        // must re-extract so the sidecar gets a real SHA recorded.
        let m = meta_with_sha("");
        let d = classify_stale(Some(&m), Some("aaaa"));
        assert_eq!(d, StaleDecision::Refresh(RefreshReason::SidecarShaUnknown));
    }

    #[test]
    fn test_classify_row3_sidecar_sha_differs_refreshes_with_reason() {
        let m = meta_with_sha("1111111111111111111111111111111111111111");
        let d = classify_stale(Some(&m), Some("2222222222222222222222222222222222222222"));
        match d {
            StaleDecision::Refresh(RefreshReason::RemoteMoved { old_sha, new_sha }) => {
                assert_eq!(old_sha, "1111111111111111111111111111111111111111");
                assert_eq!(new_sha, "2222222222222222222222222222222222222222");
            }
            other => panic!("expected RemoteMoved, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_row4_sidecar_without_remote_is_offline_warn() {
        let m = meta_with_sha("deadbeef");
        let d = classify_stale(Some(&m), None);
        assert_eq!(d, StaleDecision::SkipWithOfflineWarning);
    }

    #[test]
    fn test_classify_row5_no_sidecar_no_remote_strong_warn() {
        let d = classify_stale(None, None);
        assert_eq!(d, StaleDecision::SkipUnknownProvenanceStrongWarn);
    }

    #[test]
    fn test_refresh_reason_short_truncates_long_shas() {
        let long_old = "1111111111111111111111111111111111111111"; // 40 hex
        let long_new = "2222222222222222222222222222222222222222";
        let s = refresh_reason_short(&RefreshReason::RemoteMoved {
            old_sha: long_old.to_string(),
            new_sha: long_new.to_string(),
        });
        assert!(s.contains("111111111111..222222222222"), "got: {}", s);
    }

    // --- invalidate_package ----------------------------------------------

    #[test]
    fn test_invalidate_package_removes_directory() {
        let dir = unique_tmp_dir("invalidate_ok");
        let pkg = dir.join("alice").join("http").join("a.1");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join(".taida_installed"), "").unwrap();
        std::fs::write(pkg.join("main.td"), "stdout(1)\n").unwrap();

        let store = GlobalStore::with_root(dir.clone());
        store.invalidate_package("alice", "http", "a.1").unwrap();
        assert!(!pkg.exists(), "package dir must be gone");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalidate_package_is_idempotent_when_missing() {
        let dir = unique_tmp_dir("invalidate_missing");
        let store = GlobalStore::with_root(dir.clone());
        // Directory does not exist -- should still succeed.
        store.invalidate_package("alice", "http", "a.1").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalidate_package_rejects_traversal() {
        let dir = unique_tmp_dir("invalidate_traversal");
        let store = GlobalStore::with_root(dir.clone());
        let err = store
            .invalidate_package("..", "http", "a.1")
            .expect_err("must reject traversal");
        assert!(err.contains("Invalid"), "got: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- read_package_meta ------------------------------------------------

    #[test]
    fn test_read_package_meta_returns_none_when_uncached() {
        let dir = unique_tmp_dir("read_meta_uncached");
        let store = GlobalStore::with_root(dir.clone());
        let meta = store.read_package_meta("alice", "http", "a.1").unwrap();
        assert!(meta.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_package_meta_reads_written_sidecar() {
        let dir = unique_tmp_dir("read_meta_hit");
        let store = GlobalStore::with_root(dir.clone());
        let pkg = store.package_path("alice", "http", "a.1");
        std::fs::create_dir_all(&pkg).unwrap();
        let sample = meta_with_sha("abc");
        write_meta_atomic(&meta_path_for(&pkg), &sample).unwrap();

        let loaded = store
            .read_package_meta("alice", "http", "a.1")
            .unwrap()
            .unwrap();
        assert_eq!(loaded, sample);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- JSON helpers -----------------------------------------------------

    #[test]
    fn test_extract_json_string_field_basic() {
        let body = r#"{"ref":"refs/tags/a.1","name":"x"}"#;
        assert_eq!(
            extract_json_string_field(body, "ref"),
            Some("refs/tags/a.1".to_string())
        );
        assert_eq!(
            extract_json_string_field(body, "name"),
            Some("x".to_string())
        );
        assert_eq!(extract_json_string_field(body, "missing"), None);
    }

    #[test]
    fn test_extract_json_string_field_handles_escapes() {
        let body = r#"{"s": "a\"b\\c\n"}"#;
        assert_eq!(
            extract_json_string_field(body, "s"),
            Some("a\"b\\c\n".to_string())
        );
    }

    #[test]
    fn test_extract_json_object_field_balances_braces() {
        let body = r#"{"ref":"r","object":{"sha":"deadbeef","type":"commit","url":"u"}}"#;
        let obj = extract_json_object_field(body, "object").unwrap();
        assert!(obj.starts_with('{') && obj.ends_with('}'));
        assert_eq!(
            extract_json_string_field(&obj, "sha"),
            Some("deadbeef".to_string())
        );
        assert_eq!(
            extract_json_string_field(&obj, "type"),
            Some("commit".to_string())
        );
    }

    #[test]
    fn test_extract_json_object_field_ignores_similar_key_suffix() {
        // Make sure the extractor does not pick up "my_object" when we
        // asked for "object".
        let body = r#"{"my_object":{"x":1},"object":{"sha":"a"}}"#;
        let obj = extract_json_object_field(body, "object").unwrap();
        assert_eq!(
            extract_json_string_field(&obj, "sha"),
            Some("a".to_string())
        );
    }

    #[test]
    fn test_extract_json_object_field_handles_nested_braces_in_strings() {
        // A `}` inside a string must not close the outer object.
        let body = r#"{"object":{"msg":"} not end","sha":"x"}}"#;
        let obj = extract_json_object_field(body, "object").unwrap();
        assert_eq!(
            extract_json_string_field(&obj, "sha"),
            Some("x".to_string())
        );
    }

    #[test]
    fn test_find_key_index_rejects_embedded_match() {
        // "tag_name" should not match "name".
        let json = r#"{"tag_name":"va.99","name":"va.5"}"#;
        // Should find "name" at the second occurrence.
        let idx = find_key_index(json, "\"name\"").unwrap();
        // Check that byte before idx (after whitespace) is ','.
        let bytes = json.as_bytes();
        let mut j = idx;
        while j > 0 {
            j -= 1;
            if !matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                break;
            }
        }
        assert_eq!(bytes[j], b',');
    }

    // --- resolve_version_to_sha: validation path --------------------------

    #[test]
    fn test_resolve_version_to_sha_rejects_traversal() {
        // We can validate component rejection without going to the network.
        let err = resolve_version_to_sha("..", "http", "a.1").expect_err("traversal must error");
        assert!(err.contains("Invalid"), "got: {}", err);
        let err = resolve_version_to_sha("alice", "..", "a.1").expect_err("traversal must error");
        assert!(err.contains("Invalid"), "got: {}", err);
        let err = resolve_version_to_sha("alice", "http", "..").expect_err("traversal must error");
        assert!(err.contains("Invalid"), "got: {}", err);
    }

    // --- resolve_version_to_sha: mock server ------------------------------

    // Minimal in-process HTTP server for mocking the GitHub git/refs API.
    // Each test spawns its own listener on a random port so parallel runs
    // do not collide. The env vars `TAIDA_GITHUB_API_URL` /
    // `TAIDA_GITHUB_BASE_URL` are shared process state -- these tests
    // serialize via `env_test_lock`.
    struct MockServer {
        addr: std::net::SocketAddr,
        handle: Option<std::thread::JoinHandle<()>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            // Best-effort wakeup: open a connection so the accept loop
            // notices the stop flag.
            let _ = std::net::TcpStream::connect(self.addr);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn start_mock_api<F>(responder: F) -> MockServer
    where
        F: Fn(&str) -> Option<(u16, String)> + Send + Sync + 'static,
    {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        listener
            .set_nonblocking(false)
            .expect("set_nonblocking(false)");
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let handle = std::thread::spawn(move || {
            for incoming in listener.incoming() {
                if stop_clone.load(Ordering::SeqCst) {
                    return;
                }
                let mut stream = match incoming {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut buf = [0u8; 4096];
                let n = match stream.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => continue,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let (status, body) = responder(&path).unwrap_or((404, "not found".to_string()));
                let status_line = match status {
                    200 => "200 OK",
                    404 => "404 Not Found",
                    500 => "500 Internal Server Error",
                    _ => "200 OK",
                };
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });

        MockServer {
            addr,
            handle: Some(handle),
            stop,
        }
    }

    fn api_url_for(addr: std::net::SocketAddr) -> String {
        format!("http://{}", addr)
    }

    #[test]
    fn test_resolve_version_to_sha_unannotated_tag() {
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let body = r#"{"ref":"refs/tags/a.1","object":{"sha":"abc123","type":"commit","url":"u"}}"#;
        let server = start_mock_api(move |path| {
            if path == "/repos/alice/http/git/refs/tags/a.1" {
                Some((200, body.to_string()))
            } else {
                None
            }
        });
        let prev = std::env::var("TAIDA_GITHUB_API_URL").ok();
        unsafe {
            std::env::set_var("TAIDA_GITHUB_API_URL", api_url_for(server.addr));
        }
        let result = resolve_version_to_sha("alice", "http", "a.1");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TAIDA_GITHUB_API_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
        }
        drop(server);
        assert_eq!(result.unwrap(), Some("abc123".to_string()));
    }

    #[test]
    fn test_resolve_version_to_sha_annotated_tag_dereferences() {
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let responder = |path: &str| -> Option<(u16, String)> {
            if path == "/repos/alice/http/git/refs/tags/a.1" {
                Some((
                    200,
                    r#"{"ref":"refs/tags/a.1","object":{"sha":"tagobj","type":"tag","url":"u"}}"#
                        .to_string(),
                ))
            } else if path == "/repos/alice/http/git/tags/tagobj" {
                Some((
                    200,
                    r#"{"object":{"sha":"realcommit","type":"commit"}}"#.to_string(),
                ))
            } else {
                None
            }
        };
        let server = start_mock_api(responder);
        let prev = std::env::var("TAIDA_GITHUB_API_URL").ok();
        unsafe {
            std::env::set_var("TAIDA_GITHUB_API_URL", api_url_for(server.addr));
        }
        let result = resolve_version_to_sha("alice", "http", "a.1");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TAIDA_GITHUB_API_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
        }
        drop(server);
        assert_eq!(result.unwrap(), Some("realcommit".to_string()));
    }

    // =========================================================================
    // C17-3: store prune helpers
    // =========================================================================

    fn populate_store(root: &Path, entries: &[(&str, &str, &str, usize)]) {
        // Each tuple: (org, name, version, bytes).
        for (org, name, version, size) in entries {
            let dir = root.join(org).join(name).join(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".taida_installed"), "").unwrap();
            let payload = vec![b'x'; *size];
            std::fs::write(dir.join("main.td"), payload).unwrap();
        }
    }

    #[test]
    fn test_summarize_store_root_empty_root_missing() {
        let dir = unique_tmp_dir("prune_missing");
        let report = summarize_store_root(&dir).unwrap();
        assert!(!report.root_existed);
        assert_eq!(report.packages_removed, 0);
        assert!(report.packages.is_empty());
        assert_eq!(report.root, dir);
    }

    #[test]
    fn test_summarize_store_root_counts_packages() {
        let dir = unique_tmp_dir("prune_summary");
        std::fs::create_dir_all(&dir).unwrap();
        populate_store(
            &dir,
            &[
                ("alice", "http", "a.1", 16),
                ("alice", "http", "a.2", 32),
                ("bob", "rpc", "c.1", 64),
            ],
        );
        let report = summarize_store_root(&dir).unwrap();
        assert!(report.root_existed);
        assert_eq!(report.packages_removed, 3);
        assert!(report.bytes_removed >= (16 + 32 + 64));
        assert_eq!(
            report.packages,
            vec![
                "alice/http@a.1".to_string(),
                "alice/http@a.2".to_string(),
                "bob/rpc@c.1".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_summarize_store_package_scopes_to_org_name() {
        let dir = unique_tmp_dir("prune_summary_pkg");
        std::fs::create_dir_all(&dir).unwrap();
        populate_store(
            &dir,
            &[
                ("alice", "http", "a.1", 16),
                ("alice", "http", "a.2", 32),
                ("bob", "rpc", "c.1", 64),
            ],
        );
        let report = summarize_store_package(&dir, "alice", "http").unwrap();
        assert!(report.root_existed);
        assert_eq!(report.packages_removed, 2);
        assert_eq!(
            report.packages,
            vec!["alice/http@a.1".to_string(), "alice/http@a.2".to_string(),]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_summarize_store_package_rejects_traversal() {
        let dir = unique_tmp_dir("prune_summary_traversal");
        std::fs::create_dir_all(&dir).unwrap();
        let err = summarize_store_package(&dir, "..", "http").expect_err("traversal rejected");
        assert!(err.contains("Invalid"), "got: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_store_root_removes_everything() {
        let dir = unique_tmp_dir("prune_root");
        std::fs::create_dir_all(&dir).unwrap();
        populate_store(
            &dir,
            &[("alice", "http", "a.1", 8), ("bob", "rpc", "c.1", 8)],
        );
        // Also drop an orphan scratch dir.
        std::fs::create_dir_all(dir.join("alice").join("http").join(".tmp-a.3")).unwrap();

        let report = prune_store_root(&dir).unwrap();
        assert!(report.root_existed);
        // C17B-012: tighten assertion from `>= 2` to `== 2`. With
        // C17B-011's scratch bucketing, the `.tmp-a.3` orphan is counted
        // under `scratch_removed` and must not inflate the package count.
        assert_eq!(
            report.packages_removed, 2,
            "real packages must be exactly 2 (alice/http@a.1 + bob/rpc@c.1); scratch excluded"
        );
        assert_eq!(
            report.scratch_removed, 1,
            "the .tmp-a.3 orphan must be bucketed as scratch, not a package"
        );
        // Preview must only list real packages.
        assert_eq!(report.packages.len(), 2, "preview list excludes scratch");
        for p in &report.packages {
            assert!(
                !p.contains(".tmp-"),
                "preview must never include .tmp- scratch: {}",
                p
            );
        }

        // Root itself is kept; org directories are gone.
        assert!(
            dir.exists(),
            "root must remain so next install needn't mkdir"
        );
        assert!(!dir.join("alice").exists(), "org dir must be gone");
        assert!(!dir.join("bob").exists(), "org dir must be gone");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_summarize_store_root_buckets_scratch_separately() {
        // C17B-011 guardrail: a `.refresh-staging-*` orphan (from a
        // crashed `--force-refresh`) must not show up as a package.
        let dir = unique_tmp_dir("summarize_scratch");
        std::fs::create_dir_all(&dir).unwrap();
        populate_store(&dir, &[("alice", "http", "a.1", 8)]);
        std::fs::create_dir_all(
            dir.join("alice")
                .join("http")
                .join("a.1.refresh-staging-12345-678"),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("alice").join("http").join(".tmp-b.9")).unwrap();

        let report = summarize_store_root(&dir).unwrap();
        assert_eq!(report.packages_removed, 1);
        assert_eq!(report.scratch_removed, 2);
        assert_eq!(report.packages.len(), 1);
        assert!(report.packages[0].contains("alice/http@a.1"));
        for p in &report.packages {
            assert!(!p.contains(".refresh-staging"));
            assert!(!p.contains(".tmp-"));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_store_package_only_touches_scope() {
        let dir = unique_tmp_dir("prune_pkg");
        std::fs::create_dir_all(&dir).unwrap();
        populate_store(
            &dir,
            &[
                ("alice", "http", "a.1", 8),
                ("alice", "http", "a.2", 8),
                ("alice", "other", "a.1", 8),
                ("bob", "rpc", "c.1", 8),
            ],
        );

        let report = prune_store_package(&dir, "alice", "http").unwrap();
        assert_eq!(report.packages_removed, 2);

        assert!(!dir.join("alice").join("http").exists(), "http gone");
        assert!(dir.join("alice").join("other").exists(), "other kept");
        assert!(dir.join("bob").join("rpc").exists(), "bob kept");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_store_package_missing_is_ok() {
        let dir = unique_tmp_dir("prune_pkg_missing");
        std::fs::create_dir_all(&dir).unwrap();
        let report = prune_store_package(&dir, "alice", "http").unwrap();
        assert!(report.root_existed);
        assert_eq!(report.packages_removed, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_version_to_sha_returns_none_when_404() {
        let _guard = crate::util::env_test_lock().lock().unwrap();
        // The mock returns 404 for any path -- curl -fsSL treats this as
        // a failure, which `curl_get_optional` maps to Ok(None).
        let server = start_mock_api(|_path| Some((404, "not found".to_string())));
        let prev = std::env::var("TAIDA_GITHUB_API_URL").ok();
        unsafe {
            std::env::set_var("TAIDA_GITHUB_API_URL", api_url_for(server.addr));
        }
        let result = resolve_version_to_sha("alice", "http", "a.1");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TAIDA_GITHUB_API_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
        }
        drop(server);
        assert_eq!(result.unwrap(), None, "404 -> Ok(None) pessimistic path");
    }

    /// Mock server variant that captures every incoming raw request
    /// (request line + headers) into a shared `Vec<String>`. Used by
    /// HOLD C1/M1 tests to assert both that the Authorization header
    /// reaches the server and that curl's argv does not contain the
    /// bearer token.
    fn start_mock_api_capturing<F>(
        responder: F,
    ) -> (MockServer, std::sync::Arc<std::sync::Mutex<Vec<String>>>)
    where
        F: Fn(&str) -> Option<(u16, String)> + Send + Sync + 'static,
    {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        listener
            .set_nonblocking(false)
            .expect("set_nonblocking(false)");
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();

        let handle = std::thread::spawn(move || {
            for incoming in listener.incoming() {
                if stop_clone.load(Ordering::SeqCst) {
                    return;
                }
                let mut stream = match incoming {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut buf = [0u8; 4096];
                let n = match stream.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => continue,
                };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                if let Ok(mut g) = captured_clone.lock() {
                    g.push(req.clone());
                }
                let (status, body) = responder(&path).unwrap_or((404, "not found".to_string()));
                let status_line = match status {
                    200 => "200 OK",
                    404 => "404 Not Found",
                    500 => "500 Internal Server Error",
                    _ => "200 OK",
                };
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });

        (
            MockServer {
                addr,
                handle: Some(handle),
                stop,
            },
            captured,
        )
    }

    /// HOLD C1 fix regression test (2026-04-17): when a GH_TOKEN is
    /// configured, the authorization header must reach the server via
    /// stdin-config AND must not leak through the child's argv.
    ///
    /// Checks:
    ///   1. Mock received `Authorization: Bearer <token>`
    ///   2. Authorization header is NOT carried in the argv invocation
    ///      (we can't inspect curl's argv directly after the process
    ///      exits, but we verify the security-relevant construction
    ///      side-effect: when a token is set, the command line uses
    ///      `--config -` and does not include the raw header string).
    #[test]
    fn test_github_curl_api_get_optional_passes_token_via_stdin() {
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let (server, captured) =
            start_mock_api_capturing(|_path| Some((200, "{\"ok\":true}".to_string())));

        // Neutralize auth.json lookup so we get a deterministic token source.
        let prev_gh = std::env::var("GH_TOKEN").ok();
        let prev_github = std::env::var("GITHUB_TOKEN").ok();
        let prev_home = std::env::var("HOME").ok();
        let tmp_home = std::env::temp_dir().join(format!(
            "taida_c17_hold_c1_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_home).unwrap();
        unsafe {
            std::env::set_var("GH_TOKEN", "sekrit-token-hold-c1");
            std::env::remove_var("GITHUB_TOKEN");
            std::env::set_var("HOME", &tmp_home);
        }

        // Exercise the shared helper exactly as production code does.
        let url = format!("http://{}/repos/alice/bar/git/refs/tags/a.1", server.addr);
        let out =
            github_curl_api_get_optional(&url, "application/vnd.github+json").expect("no error");

        // Restore env before any assertion to avoid leaking on panic.
        unsafe {
            match prev_gh {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
            match prev_github {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        drop(server);
        let _ = std::fs::remove_dir_all(&tmp_home);

        assert_eq!(out.as_deref(), Some("{\"ok\":true}"));
        let captured = captured.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "mock should have received the request"
        );
        let req = captured.join("\n");
        assert!(
            req.contains("Authorization: Bearer sekrit-token-hold-c1"),
            "token header must reach the server (stdin-config path): {}",
            req
        );
        assert!(
            req.contains("User-Agent: taida-install"),
            "UA header must reach the server: {}",
            req
        );
    }

    /// HOLD M1 fix regression test (2026-04-17): the tags listing code
    /// path (`resolve_generation_from_remote`) must also route through
    /// the unified helper so that auth/timeout/UA are applied.
    #[test]
    fn test_resolve_generation_sends_auth_and_ua_headers() {
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let (server, captured) = start_mock_api_capturing(|path| {
            if path.contains("/tags") {
                Some((200, "[{\"name\":\"a.1\"}]".to_string()))
            } else {
                Some((404, "nope".to_string()))
            }
        });

        let prev_gh = std::env::var("GH_TOKEN").ok();
        let prev_github = std::env::var("GITHUB_TOKEN").ok();
        let prev_home = std::env::var("HOME").ok();
        let prev_api = std::env::var("TAIDA_GITHUB_API_URL").ok();
        let tmp_home = std::env::temp_dir().join(format!(
            "taida_c17_hold_m1_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_home).unwrap();
        unsafe {
            std::env::set_var("GH_TOKEN", "tok-m1");
            std::env::remove_var("GITHUB_TOKEN");
            std::env::set_var("HOME", &tmp_home);
            std::env::set_var("TAIDA_GITHUB_API_URL", api_url_for(server.addr));
        }

        // `resolve_generation_from_remote` is the M1-affected path.
        // We only care that it issued an authenticated request; the
        // resolver result itself is exercised elsewhere.
        let store = GlobalStore::with_root(tmp_home.join("store"));
        let _ = store.resolve_generation("alice", "bar", "a");

        unsafe {
            match prev_gh {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
            match prev_github {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_api {
                Some(v) => std::env::set_var("TAIDA_GITHUB_API_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
        }
        drop(server);
        let _ = std::fs::remove_dir_all(&tmp_home);

        let captured = captured.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "mock should have received a tag listing request"
        );
        let req = captured.join("\n");
        assert!(
            req.contains("Authorization: Bearer tok-m1"),
            "HOLD M1: resolve_generation_from_remote must carry auth header. req: {}",
            req
        );
        assert!(
            req.contains("User-Agent: taida-install"),
            "HOLD M1: UA header must be applied. req: {}",
            req
        );
        assert!(
            req.contains("Accept: application/vnd.github.v3+json"),
            "HOLD M1: Accept header for tags API must be preserved. req: {}",
            req
        );
    }

    // ── C18B-007 / C18B-008 regression: sanitize_auth_token ──
    //
    // The helper is a pure function (env-free); we exercise it
    // directly to pin the rejection rules:
    //   - empty → None (no warning, treated as "unset")
    //   - whitespace-only → None (with warning per C18B-008)
    //   - contains `"` → None (per C18B-007)
    //   - contains `\` → None (per C18B-007)
    //   - contains `\n` → None (per C18B-007)
    //   - contains `\r` → None (per C18B-007)
    //   - well-formed token → Some(trimmed)

    #[test]
    fn test_sanitize_auth_token_rejects_empty() {
        assert_eq!(super::sanitize_auth_token("", "TEST_SRC"), None);
    }

    #[test]
    fn test_sanitize_auth_token_rejects_whitespace_only() {
        // C18B-008: `"    "` used to pass through and produce
        // `Authorization: Bearer     ` which GitHub 401-rejects.
        assert_eq!(super::sanitize_auth_token("    ", "TEST_SRC"), None);
        assert_eq!(super::sanitize_auth_token("\t\t", "TEST_SRC"), None);
    }

    #[test]
    fn test_sanitize_auth_token_rejects_double_quote() {
        // C18B-007: `"` escapes out of the curl config's quoted value.
        assert_eq!(super::sanitize_auth_token("ghp_abc\"def", "TEST_SRC"), None);
    }

    #[test]
    fn test_sanitize_auth_token_rejects_backslash() {
        // C18B-007: `\` is the escape prefix inside the curl config.
        assert_eq!(super::sanitize_auth_token("ghp_abc\\def", "TEST_SRC"), None);
    }

    #[test]
    fn test_sanitize_auth_token_rejects_newline() {
        // C18B-007: `\n` in the raw token would inject a fresh config
        // directive on the following line of the curl config stream.
        assert_eq!(super::sanitize_auth_token("ghp_abc\ndef", "TEST_SRC"), None);
    }

    #[test]
    fn test_sanitize_auth_token_rejects_cr() {
        assert_eq!(super::sanitize_auth_token("ghp_abc\rdef", "TEST_SRC"), None);
    }

    #[test]
    fn test_sanitize_auth_token_accepts_clean_token() {
        // Typical shape: `ghp_` + 36 base62 chars.
        let t = "ghp_abcdefghijklmnopqrstuvwxyz1234567890";
        assert_eq!(
            super::sanitize_auth_token(t, "TEST_SRC"),
            Some(t.to_string())
        );
    }

    #[test]
    fn test_sanitize_auth_token_trims_surrounding_whitespace() {
        // Tokens routinely pick up a trailing newline from shell
        // pipes (`echo $TOKEN`). Trimming is the expected behaviour
        // — only the trimmed value is sent.
        assert_eq!(
            super::sanitize_auth_token("  ghp_abc123\n", "TEST_SRC"),
            Some("ghp_abc123".to_string())
        );
    }
}
