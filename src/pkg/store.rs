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
/// ```
use std::path::{Path, PathBuf};

/// Base URL for GitHub archive downloads.
/// Override with `TAIDA_GITHUB_BASE_URL` for testing (e.g. local mock server).
fn github_base_url() -> String {
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
    /// extracts it to `~/.taida/store/{org}/{name}/{version}/`, and creates a `.taida_installed` marker.
    pub fn fetch_and_cache(&self, org: &str, name: &str, version: &str) -> Result<PathBuf, String> {
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

        // Use curl to download
        let curl_status = std::process::Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&archive_path)
            .arg(&url)
            .status()
            .map_err(|e| format!("Failed to run curl: {}", e))?;

        if !curl_status.success() {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "Failed to download package {}/{}@{} from {}",
                org, name, version, url
            ));
        }

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

        // Create completion marker
        std::fs::write(pkg_dir.join(".taida_installed"), "")
            .map_err(|e| format!("Cannot create install marker: {}", e))?;

        // Cleanup temp directory
        let _ = std::fs::remove_dir_all(&tmp_dir);

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
    /// Used by `taida update` to find the latest version even when an older version is cached.
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

        let output = std::process::Command::new("curl")
            .args(["-fsSL", "-H", "Accept: application/vnd.github.v3+json"])
            .arg(&url)
            .output()
            .map_err(|e| format!("Failed to query GitHub tags for {}/{}: {}", org, name, e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to fetch tags for {}/{}: HTTP error",
                org, name
            ));
        }

        let body = String::from_utf8_lossy(&output.stdout);
        let prefix_new = format!("{}.", generation);
        let prefix_legacy = format!("v{}.", generation);

        // Parse tag names from JSON (handles both pretty-printed and compressed JSON)
        // Tags can be "a.3", "a.3.alpha" (new) or "va.3" (legacy) — extract num and pick highest
        let mut best: Option<(u64, String)> = None; // (num, version_without_v)
        for tag in extract_json_name_values(&body) {
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
}
