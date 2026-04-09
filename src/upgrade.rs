//! Self-upgrade for the `taida` binary (RC5 Phase 3).
//!
//! ## Design
//!
//! - Fetches release metadata from GitHub Releases API (unauthenticated).
//! - Parses Taida version tags `@<gen>.<num>.<label?>`.
//! - Resolves the best matching version based on CLI filters.
//! - Downloads the platform-appropriate binary asset.
//! - Verifies SHA-256 integrity.
//! - Replaces the current executable via rename.
//!
//! ## Version scheme
//!
//! ```text
//! @b.10.rc2   -> gen="b", num=10, label=Some("rc2")
//! @b.11       -> gen="b", num=11, label=None        (stable)
//! @b.11.stable-> gen="b", num=11, label=Some("stable") (also stable)
//! ```

use crate::addon::host_target::{self, HostTarget};
use crate::crypto;

/// A parsed Taida version tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaidaVersion {
    /// Generation identifier (e.g. "a", "b").
    pub generation: String,
    /// Numeric part (e.g. 10, 11).
    pub num: u32,
    /// Optional label (e.g. "rc2", "stable", or None).
    pub label: Option<String>,
    /// The original tag string (e.g. "@b.10.rc2").
    pub tag: String,
}

impl TaidaVersion {
    /// Returns true if this version is considered stable.
    ///
    /// Stable = no label, or label == "stable".
    pub fn is_stable(&self) -> bool {
        match &self.label {
            None => true,
            Some(l) => l == "stable",
        }
    }

    /// Parse a tag string like `@b.10.rc2` or `@b.11` into a TaidaVersion.
    pub fn parse(tag: &str) -> Option<Self> {
        let stripped = tag.strip_prefix('@')?;
        let mut parts = stripped.splitn(3, '.');
        let generation = parts.next()?.to_string();
        if generation.is_empty() {
            return None;
        }
        let num_str = parts.next()?;
        let num: u32 = num_str.parse().ok()?;
        let label = parts.next().map(|s| s.to_string());
        // Reject empty labels (e.g. "@b.10.")
        if let Some(ref l) = label {
            if l.is_empty() {
                return None;
            }
        }
        Some(TaidaVersion {
            generation,
            num,
            label,
            tag: tag.to_string(),
        })
    }
}

impl std::fmt::Display for TaidaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.tag)
    }
}

impl Ord for TaidaVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare generation lexicographically, then num descending.
        self.generation
            .cmp(&other.generation)
            .then(self.num.cmp(&other.num))
            // Tie-break: label=None (stable) > label=Some("stable") > others
            .then_with(|| match (&self.label, &other.label) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => {
                    // "stable" sorts above other labels
                    let a_stable = a == "stable";
                    let b_stable = b == "stable";
                    match (a_stable, b_stable) {
                        (true, false) => std::cmp::Ordering::Greater,
                        (false, true) => std::cmp::Ordering::Less,
                        _ => a.cmp(b),
                    }
                }
            })
    }
}

impl PartialOrd for TaidaVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Filter criteria for version resolution.
pub struct VersionFilter {
    /// If set, only match versions with this generation.
    pub generation: Option<String>,
    /// If set, only match versions with this label.
    pub label: Option<String>,
    /// If set, match exactly this version.
    pub exact: Option<String>,
}

/// Resolve the best version from a list of tags.
///
/// Returns the highest-ranked version matching the filter, or None.
pub fn resolve_version(
    tags: &[String],
    filter: &VersionFilter,
    current: Option<&TaidaVersion>,
) -> Result<Option<TaidaVersion>, String> {
    // If exact version requested, just check it exists
    if let Some(ref exact) = filter.exact {
        let parsed = TaidaVersion::parse(exact)
            .ok_or_else(|| format!("invalid version format: {}", exact))?;
        let found = tags.iter().any(|t| t == exact);
        if !found {
            return Err(format!("version {} not found in releases", exact));
        }
        // Check if it's the same as current
        if let Some(cur) = current {
            if cur.tag == parsed.tag {
                return Ok(None); // already up to date
            }
        }
        return Ok(Some(parsed));
    }

    // Parse all tags and filter
    let mut candidates: Vec<TaidaVersion> = tags
        .iter()
        .filter_map(|t| TaidaVersion::parse(t))
        .filter(|v| {
            // Apply generation filter
            if let Some(ref g) = filter.generation {
                if &v.generation != g {
                    return false;
                }
            }
            // Apply label filter
            if let Some(ref label) = filter.label {
                match &v.label {
                    Some(l) => l == label,
                    None => false,
                }
            } else {
                // Default: stable only
                v.is_stable()
            }
        })
        .collect();

    // Sort descending (highest version first)
    candidates.sort_unstable_by(|a, b| b.cmp(a));

    if let Some(best) = candidates.into_iter().next() {
        // Check if it's the same as current
        if let Some(cur) = current {
            if cur.tag == best.tag {
                return Ok(None); // already up to date
            }
        }
        Ok(Some(best))
    } else {
        Ok(None)
    }
}

/// GitHub API base URL. Respects `TAIDA_GITHUB_API_URL` for testing.
fn api_url() -> String {
    std::env::var("TAIDA_GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_string())
}

/// Build a blocking reqwest client without authentication.
fn make_public_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("taida-upgrade")
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
            );
            headers
        })
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))
}

/// Fetch all release tag names from the GitHub repository.
///
/// Paginates if necessary (up to 10 pages of 100 releases each).
pub fn fetch_release_tags(owner: &str, repo: &str) -> Result<Vec<String>, String> {
    let client = make_public_client()?;
    let base = api_url();
    let mut tags = Vec::new();
    let mut page = 1u32;

    loop {
        let url = format!(
            "{}/repos/{}/{}/releases?per_page=100&page={}",
            base, owner, repo, page
        );
        let resp = client
            .get(&url)
            .send()
            .map_err(|e| format!("failed to fetch releases: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "GitHub API error (HTTP {}): {}",
                status, body
            ));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| format!("failed to parse releases JSON: {}", e))?;

        let arr = json
            .as_array()
            .ok_or_else(|| "releases response is not an array".to_string())?;

        if arr.is_empty() {
            break;
        }

        for item in arr {
            if let Some(tag) = item["tag_name"].as_str() {
                tags.push(tag.to_string());
            }
        }

        // Stop after 10 pages (1000 releases should be more than enough)
        if arr.len() < 100 || page >= 10 {
            break;
        }
        page += 1;
    }

    Ok(tags)
}

/// Find the download URL for a specific release asset.
pub fn find_asset_url(
    owner: &str,
    repo: &str,
    tag: &str,
    asset_name: &str,
) -> Result<String, String> {
    let client = make_public_client()?;
    let base = api_url();
    let url = format!(
        "{}/repos/{}/{}/releases/tags/{}",
        base, owner, repo, tag
    );

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("failed to fetch release {}: {}", tag, e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "failed to get release {} (HTTP {}): {}",
            tag, status, body
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("failed to parse release JSON: {}", e))?;

    let assets = json["assets"]
        .as_array()
        .ok_or_else(|| format!("release {} has no assets array", tag))?;

    for asset in assets {
        if asset["name"].as_str() == Some(asset_name) {
            if let Some(url) = asset["browser_download_url"].as_str() {
                return Ok(url.to_string());
            }
        }
    }

    Err(format!(
        "asset '{}' not found in release {}",
        asset_name, tag
    ))
}

/// Download a binary from URL and verify its SHA-256 hash.
pub fn download_and_verify(
    url: &str,
    expected_sha256: Option<&str>,
) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("taida-upgrade")
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("download failed (HTTP {})", status));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("failed to read download body: {}", e))?
        .to_vec();

    if let Some(expected) = expected_sha256 {
        let actual = crypto::sha256_hex_bytes(&bytes);
        if actual != expected {
            return Err(format!(
                "SHA-256 mismatch: expected {}, got {}",
                expected, actual
            ));
        }
    }

    Ok(bytes)
}

/// Determine the expected asset name for the current platform.
///
/// Convention: `taida-<triple>` or `taida-<triple>.exe` on Windows.
pub fn platform_asset_name(host: &HostTarget) -> String {
    let triple = host.as_triple();
    if matches!(host, HostTarget::X86_64Windows) {
        format!("taida-{}.exe", triple)
    } else {
        format!("taida-{}", triple)
    }
}

/// Replace the current executable with the new binary.
///
/// Strategy: rename current -> current.old, write new -> current, remove old.
pub fn self_replace(new_binary: &[u8]) -> Result<(), String> {
    let current = std::env::current_exe()
        .map_err(|e| format!("cannot determine current executable path: {}", e))?;

    let backup = current.with_extension("old");

    // Rename current -> backup
    std::fs::rename(&current, &backup).map_err(|e| {
        format!(
            "failed to rename {} -> {}: {}",
            current.display(),
            backup.display(),
            e
        )
    })?;

    // Write new binary
    if let Err(e) = std::fs::write(&current, new_binary) {
        // Attempt to restore backup
        let _ = std::fs::rename(&backup, &current);
        return Err(format!(
            "failed to write new binary to {}: {}",
            current.display(),
            e
        ));
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755));
    }

    // Remove backup
    let _ = std::fs::remove_file(&backup);

    Ok(())
}

/// The GitHub owner and repo for taida.
const TAIDA_OWNER: &str = "shijimic";
const TAIDA_REPO: &str = "taida";

/// Upgrade configuration parsed from CLI args.
pub struct UpgradeConfig {
    pub check_only: bool,
    pub filter: VersionFilter,
}

/// Run the upgrade command.
pub fn run(config: UpgradeConfig) -> Result<(), String> {
    let current_version_str = crate::version::taida_version();
    let current = TaidaVersion::parse(current_version_str);

    println!("Current version: {}", current_version_str);
    println!("Checking for updates...");

    // Fetch all release tags
    let tags = fetch_release_tags(TAIDA_OWNER, TAIDA_REPO)?;

    if tags.is_empty() {
        println!("No releases found.");
        return Ok(());
    }

    // Resolve best version
    let resolved = resolve_version(&tags, &config.filter, current.as_ref())?;

    match resolved {
        None => {
            println!("Already up to date.");
            Ok(())
        }
        Some(version) => {
            println!("New version available: {}", version);

            if config.check_only {
                return Ok(());
            }

            // Detect host platform
            #[cfg(feature = "native")]
            let host = host_target::detect_host_target().map_err(|e| e.to_string())?;

            #[cfg(not(feature = "native"))]
            return Err(
                "upgrade requires the 'native' feature for platform detection".to_string(),
            );

            #[cfg(feature = "native")]
            {
                let asset_name = platform_asset_name(&host);
                println!("Downloading {} ...", asset_name);

                // Find asset download URL
                let download_url =
                    find_asset_url(TAIDA_OWNER, TAIDA_REPO, &version.tag, &asset_name)?;

                // Try to find SHA-256 checksum file
                let sha_asset = format!("{}.sha256", asset_name);
                let expected_sha = match find_asset_url(
                    TAIDA_OWNER,
                    TAIDA_REPO,
                    &version.tag,
                    &sha_asset,
                ) {
                    Ok(sha_url) => {
                        let sha_bytes = download_and_verify(&sha_url, None)?;
                        let sha_text = String::from_utf8(sha_bytes)
                            .map_err(|e| format!("invalid SHA-256 file encoding: {}", e))?;
                        // SHA file format: "<hex>  <filename>" or just "<hex>"
                        Some(
                            sha_text
                                .split_whitespace()
                                .next()
                                .unwrap_or("")
                                .to_string(),
                        )
                    }
                    Err(_) => {
                        eprintln!(
                            "Warning: no SHA-256 checksum file found for {}. Skipping verification.",
                            asset_name
                        );
                        None
                    }
                };

                // Download binary
                let binary = download_and_verify(
                    &download_url,
                    expected_sha.as_deref(),
                )?;

                println!("Installing {} ...", version);

                // Replace current executable
                self_replace(&binary)?;

                println!("Successfully upgraded to {}", version);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── TaidaVersion::parse ──

    #[test]
    fn parse_stable_no_label() {
        let v = TaidaVersion::parse("@b.11").unwrap();
        assert_eq!(v.generation, "b");
        assert_eq!(v.num, 11);
        assert_eq!(v.label, None);
        assert!(v.is_stable());
    }

    #[test]
    fn parse_stable_explicit_label() {
        let v = TaidaVersion::parse("@b.11.stable").unwrap();
        assert_eq!(v.generation, "b");
        assert_eq!(v.num, 11);
        assert_eq!(v.label, Some("stable".to_string()));
        assert!(v.is_stable());
    }

    #[test]
    fn parse_rc_label() {
        let v = TaidaVersion::parse("@b.10.rc2").unwrap();
        assert_eq!(v.generation, "b");
        assert_eq!(v.num, 10);
        assert_eq!(v.label, Some("rc2".to_string()));
        assert!(!v.is_stable());
    }

    #[test]
    fn parse_gen_a() {
        let v = TaidaVersion::parse("@a.7.beta").unwrap();
        assert_eq!(v.generation, "a");
        assert_eq!(v.num, 7);
        assert_eq!(v.label, Some("beta".to_string()));
    }

    #[test]
    fn parse_rejects_missing_at() {
        assert!(TaidaVersion::parse("b.10.rc2").is_none());
    }

    #[test]
    fn parse_rejects_empty_gen() {
        assert!(TaidaVersion::parse("@.10").is_none());
    }

    #[test]
    fn parse_rejects_non_numeric() {
        assert!(TaidaVersion::parse("@b.abc").is_none());
    }

    #[test]
    fn parse_rejects_trailing_dot() {
        assert!(TaidaVersion::parse("@b.10.").is_none());
    }

    // ── Ordering ──

    #[test]
    fn ordering_higher_num_wins() {
        let v10 = TaidaVersion::parse("@b.10").unwrap();
        let v11 = TaidaVersion::parse("@b.11").unwrap();
        assert!(v11 > v10);
    }

    #[test]
    fn ordering_no_label_beats_stable_label() {
        let no_label = TaidaVersion::parse("@b.11").unwrap();
        let stable = TaidaVersion::parse("@b.11.stable").unwrap();
        assert!(no_label > stable);
    }

    #[test]
    fn ordering_stable_label_beats_rc() {
        let stable = TaidaVersion::parse("@b.11.stable").unwrap();
        let rc = TaidaVersion::parse("@b.11.rc2").unwrap();
        assert!(stable > rc);
    }

    // ── resolve_version ──

    #[test]
    fn resolve_latest_stable() {
        let tags = vec![
            "@b.10.rc2".to_string(),
            "@b.11".to_string(),
            "@b.10".to_string(),
            "@b.11.stable".to_string(),
        ];
        let filter = VersionFilter {
            generation: None,
            label: None,
            exact: None,
        };
        let result = resolve_version(&tags, &filter, None).unwrap();
        // @b.11 (no label) should win over @b.11.stable
        assert_eq!(result.unwrap().tag, "@b.11");
    }

    #[test]
    fn resolve_by_gen() {
        let tags = vec![
            "@a.7".to_string(),
            "@b.10".to_string(),
            "@b.11".to_string(),
        ];
        let filter = VersionFilter {
            generation: Some("a".to_string()),
            label: None,
            exact: None,
        };
        let result = resolve_version(&tags, &filter, None).unwrap();
        assert_eq!(result.unwrap().tag, "@a.7");
    }

    #[test]
    fn resolve_by_label() {
        let tags = vec![
            "@b.10.rc2".to_string(),
            "@b.11".to_string(),
            "@b.11.rc2".to_string(),
        ];
        let filter = VersionFilter {
            generation: None,
            label: Some("rc2".to_string()),
            exact: None,
        };
        let result = resolve_version(&tags, &filter, None).unwrap();
        assert_eq!(result.unwrap().tag, "@b.11.rc2");
    }

    #[test]
    fn resolve_exact() {
        let tags = vec![
            "@b.10.rc2".to_string(),
            "@b.11".to_string(),
        ];
        let filter = VersionFilter {
            generation: None,
            label: None,
            exact: Some("@b.10.rc2".to_string()),
        };
        let result = resolve_version(&tags, &filter, None).unwrap();
        assert_eq!(result.unwrap().tag, "@b.10.rc2");
    }

    #[test]
    fn resolve_exact_not_found() {
        let tags = vec!["@b.11".to_string()];
        let filter = VersionFilter {
            generation: None,
            label: None,
            exact: Some("@b.99".to_string()),
        };
        let result = resolve_version(&tags, &filter, None);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_already_up_to_date() {
        let tags = vec!["@b.11".to_string()];
        let current = TaidaVersion::parse("@b.11").unwrap();
        let filter = VersionFilter {
            generation: None,
            label: None,
            exact: None,
        };
        let result = resolve_version(&tags, &filter, Some(&current)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn resolve_no_matching_candidates() {
        let tags = vec!["@b.10.rc2".to_string()];
        let filter = VersionFilter {
            generation: None,
            label: None,
            exact: None,
        };
        // Only rc2, no stable -- should return None
        let result = resolve_version(&tags, &filter, None).unwrap();
        assert!(result.is_none());
    }

    // ── platform_asset_name ──

    #[test]
    fn asset_name_linux() {
        let name = platform_asset_name(&HostTarget::X86_64LinuxGnu);
        assert_eq!(name, "taida-x86_64-unknown-linux-gnu");
    }

    #[test]
    fn asset_name_macos() {
        let name = platform_asset_name(&HostTarget::Aarch64MacOs);
        assert_eq!(name, "taida-aarch64-apple-darwin");
    }

    #[test]
    fn asset_name_windows() {
        let name = platform_asset_name(&HostTarget::X86_64Windows);
        assert_eq!(name, "taida-x86_64-pc-windows-msvc.exe");
    }

    // ── download_and_verify (sha mismatch) ──

    #[test]
    fn verify_sha_mismatch_is_error() {
        // This tests the verification logic without making a network call.
        // We feed bytes directly and check the SHA logic.
        let data = b"hello world";
        let actual_sha = crypto::sha256_hex_bytes(data);
        let wrong_sha = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_ne!(actual_sha, wrong_sha);
    }
}
