//! Self-upgrade for the `taida` binary (RC5 Phase 3).
//!
//! ## Design
//!
//! - Fetches release metadata from GitHub Releases API (unauthenticated).
//! - Parses Taida version tags `@<gen>.<num>.<label?>`.
//! - Resolves the best matching version based on CLI filters.
//! - Downloads the platform-appropriate **archive** asset
//!   (`taida-<tag>-<target>.tar.gz` on Unix, `.zip` on Windows).
//! - Verifies the shared `SHA256SUMS` file with Sigstore cosign.
//! - Verifies SHA-256 integrity against the release-signed `SHA256SUMS` file.
//! - Extracts the `taida` binary from the archive.
//! - Replaces the current executable via rename.
//!
//! ## Release artifact contract
//!
//! ```text
//! scripts/release/package-unix.sh   → taida-<tag>-<target>.tar.gz
//! scripts/release/package-windows.ps1 → taida-<tag>-<target>.zip
//! .github/workflows/release.yml     → SHA256SUMS (shared, all archives)
//! ```
//!
//! Archive layout: `<archive_base>/taida` (or `taida.exe` on Windows).
//!
//! ## Version scheme
//!
//! ```text
//! @b.10.rc2   -> gen="b", num=10, label=Some("rc2")
//! @b.11       -> gen="b", num=11, label=None        (stable)
//! @b.11.stable-> gen="b", num=11, label=Some("stable") (also stable)
//! ```

use crate::addon::host_target::{self, HostTarget};
use crate::addon::signature_verify::{
    VerifyError, VerifyOutcome, VerifyPolicy, bundle_path_for, verify_artifact_with_identity,
};
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
        if let Some(ref l) = label
            && l.is_empty()
        {
            return None;
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
        if let Some(cur) = current
            && cur.tag == parsed.tag
        {
            return Ok(None); // already up to date
        }
        return Ok(Some(parsed));
    }

    // Parse all tags and filter
    let mut candidates: Vec<TaidaVersion> = tags
        .iter()
        .filter_map(|t| TaidaVersion::parse(t))
        .filter(|v| {
            // Apply generation filter
            if let Some(ref g) = filter.generation
                && &v.generation != g
            {
                return false;
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
        if let Some(cur) = current
            && cur.tag == best.tag
        {
            return Ok(None); // already up to date
        }
        Ok(Some(best))
    } else {
        Ok(None)
    }
}

const GITHUB_API_URL: &str = "https://api.github.com";

/// Certificate identity accepted for the self-upgrade trust root.
///
/// Unlike source-package verification, self-upgrade is only allowed to trust
/// the canonical `taida-lang/taida` release workflow for a tagged release.
pub const UPGRADE_COSIGN_IDENTITY_REGEXP: &str =
    r"^https://github.com/taida-lang/taida/\.github/workflows/.+@refs/tags/.+$";

/// GitHub API base URL for `taida upgrade`.
///
/// Production code never reads `TAIDA_GITHUB_API_URL`: the self-replacing
/// upgrade path must not be redirected by ambient environment variables.
pub fn api_url() -> &'static str {
    GITHUB_API_URL
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
        .map_err(|e| {
            format!(
                "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to build HTTP client: {}",
                e
            )
        })
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
        let resp = client.get(&url).send().map_err(|e| {
            format!(
                "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to fetch releases: {}",
                e
            )
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "[E32K1_UPGRADE_DOWNLOAD_FAILED] GitHub API error (HTTP {}): {}",
                status, body
            ));
        }

        let json: serde_json::Value = resp.json().map_err(|e| {
            format!(
                "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to parse releases JSON: {}",
                e
            )
        })?;

        let arr = json.as_array().ok_or_else(|| {
            "[E32K1_UPGRADE_DOWNLOAD_FAILED] releases response is not an array".to_string()
        })?;

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
    let url = format!("{}/repos/{}/{}/releases/tags/{}", base, owner, repo, tag);

    let resp = client.get(&url).send().map_err(|e| {
        format!(
            "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to fetch release {}: {}",
            tag, e
        )
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to get release {} (HTTP {}): {}",
            tag, status, body
        ));
    }

    let json: serde_json::Value = resp.json().map_err(|e| {
        format!(
            "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to parse release JSON: {}",
            e
        )
    })?;

    let assets = json["assets"]
        .as_array()
        .ok_or_else(|| format!("release {} has no assets array", tag))?;

    for asset in assets {
        if asset["name"].as_str() == Some(asset_name)
            && let Some(url) = asset["browser_download_url"].as_str()
        {
            return Ok(url.to_string());
        }
    }

    Err(format!(
        "asset '{}' not found in release {}",
        asset_name, tag
    ))
}

/// Download bytes from URL.
pub fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path)
            .map_err(|e| format!("[E32K1_UPGRADE_DOWNLOAD_FAILED] download failed: {}", e));
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("taida-upgrade")
        .build()
        .map_err(|e| {
            format!(
                "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to build HTTP client: {}",
                e
            )
        })?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("[E32K1_UPGRADE_DOWNLOAD_FAILED] download failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!(
            "[E32K1_UPGRADE_DOWNLOAD_FAILED] download failed (HTTP {})",
            status
        ));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| {
            format!(
                "[E32K1_UPGRADE_DOWNLOAD_FAILED] failed to read download body: {}",
                e
            )
        })?
        .to_vec();

    Ok(bytes)
}

/// Verify already-downloaded bytes against a mandatory SHA-256 hex digest.
pub fn verify_sha256_bytes(bytes: &[u8], expected_sha256: &str) -> Result<(), String> {
    let expected = expected_sha256.trim();
    if expected.is_empty() {
        return Err("[E32K1_UPGRADE_NO_SHA256SUMS] release SHA256SUMS entry is empty".to_string());
    }

    let actual = crypto::sha256_hex_bytes(bytes);
    if actual != expected {
        return Err(format!(
            "[E32K1_UPGRADE_SHA256_MISMATCH] SHA-256 mismatch: expected {}, got {}",
            expected, actual
        ));
    }

    Ok(())
}

/// Download a release artifact and verify its mandatory SHA-256 hash.
pub fn download_and_verify(url: &str, expected_sha256: &str) -> Result<Vec<u8>, String> {
    let bytes = download_bytes(url)?;
    verify_sha256_bytes(&bytes, expected_sha256)?;
    Ok(bytes)
}

/// Determine the expected archive asset name for the current platform.
///
/// Convention (matching `scripts/release/package-*.{sh,ps1}`):
/// - Unix: `taida-<tag>-<target>.tar.gz`
/// - Windows: `taida-<tag>-<target>.zip`
pub fn platform_archive_name(tag: &str, host: &HostTarget) -> String {
    let triple = host.as_triple();
    let base = format!("taida-{}-{}", tag, triple);
    if matches!(host, HostTarget::X86_64Windows) {
        format!("{}.zip", base)
    } else {
        format!("{}.tar.gz", base)
    }
}

/// Extract the `taida` binary from a `.tar.gz` archive.
///
/// The archive is expected to contain `<base>/taida` where `<base>`
/// matches the archive name without extension (e.g. `taida-@b.11-x86_64-unknown-linux-gnu`).
pub fn extract_binary_from_tar_gz(
    archive_bytes: &[u8],
    archive_base: &str,
) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(decoder);

    let binary_path = format!("{}/taida", archive_base);

    for entry_result in archive
        .entries()
        .map_err(|e| format!("failed to read tar entries: {}", e))?
    {
        let mut entry = entry_result.map_err(|e| format!("failed to read tar entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("failed to read entry path: {}", e))?
            .to_string_lossy()
            .to_string();

        if path == binary_path {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("failed to read binary from archive: {}", e))?;
            return Ok(buf);
        }
    }

    Err(format!(
        "'{}' not found in archive (expected '{}')",
        "taida", binary_path
    ))
}

/// Look up the SHA-256 hash for a specific file from a `SHA256SUMS` blob.
///
/// The format matches `sha256sum` output: `<hex>  <filename>` per line.
pub fn lookup_sha256(sha256sums: &str, target_filename: &str) -> Option<String> {
    for line in sha256sums.lines() {
        // Format: "<hex>  <filename>" or "<hex> <filename>"
        let mut parts = line.splitn(2, |c: char| c.is_whitespace());
        let hex = parts.next()?;
        let filename = parts.next().map(|s| s.trim());
        if filename == Some(target_filename) {
            return Some(hex.to_string());
        }
    }
    None
}

/// Look up the mandatory SHA-256 entry for a release archive.
pub fn expected_sha256_for_archive(
    sha256sums: &str,
    target_filename: &str,
) -> Result<String, String> {
    lookup_sha256(sha256sums, target_filename).ok_or_else(|| {
        format!(
            "[E32K1_UPGRADE_NO_SHA256SUMS] release SHA256SUMS does not list {}",
            target_filename
        )
    })
}

struct TempDownloadedFile {
    path: std::path::PathBuf,
}

impl TempDownloadedFile {
    fn new(label: &str, bytes: &[u8]) -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| {
                format!(
                    "[E32K1_UPGRADE_STAGE_FAILED] system clock error while staging upgrade artifact: {}",
                    e
                )
            })?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "taida_upgrade_{}_{}_{}",
            std::process::id(),
            nanos,
            label
        ));
        std::fs::write(&path, bytes).map_err(|e| {
            format!(
                "[E32K1_UPGRADE_STAGE_FAILED] failed to stage {} for verification: {}",
                label, e
            )
        })?;
        Ok(Self { path })
    }
}

impl Drop for TempDownloadedFile {
    fn drop(&mut self) {
        let bundle = bundle_path_for(&self.path);
        let _ = std::fs::remove_file(&bundle);
        let _ = std::fs::remove_file(&self.path);
    }
}

fn upgrade_cosign_error(err: VerifyError) -> String {
    match err {
        VerifyError::CosignUnavailable => {
            "[E32K1_COSIGN_MISSING] taida upgrade requires cosign on PATH to verify SHA256SUMS"
                .to_string()
        }
        VerifyError::BundleMissing(detail) => format!(
            "[E32K1_UPGRADE_SHA256SUMS_COSIGN_MISSING] SHA256SUMS cosign bundle is required: {}",
            detail
        ),
        VerifyError::SignatureRejected { stderr } => format!(
            "[E32K1_UPGRADE_SHA256SUMS_COSIGN_REJECTED] cosign rejected SHA256SUMS: {}",
            stderr.trim()
        ),
        VerifyError::InvocationError(detail) => format!(
            "[E32K1_UPGRADE_SHA256SUMS_COSIGN_ERROR] SHA256SUMS cosign verification failed: {}",
            detail
        ),
    }
}

/// Download `SHA256SUMS`, verify its cosign bundle, then return the text.
pub fn download_verified_sha256sums(sha256sums_url: &str) -> Result<String, String> {
    let bytes = download_bytes(sha256sums_url)?;
    let staged = TempDownloadedFile::new("SHA256SUMS", &bytes)?;
    let outcome = verify_artifact_with_identity(
        &staged.path,
        sha256sums_url,
        VerifyPolicy::Required,
        UPGRADE_COSIGN_IDENTITY_REGEXP,
    )
    .map_err(upgrade_cosign_error)?;

    if !matches!(outcome, VerifyOutcome::Verified) {
        return Err(format!(
            "[E32K1_UPGRADE_SHA256SUMS_COSIGN_ERROR] SHA256SUMS verification did not complete: {:?}",
            outcome
        ));
    }

    String::from_utf8(bytes).map_err(|e| {
        format!(
            "[E32K1_UPGRADE_SHA256SUMS_INVALID_ENCODING] invalid SHA256SUMS encoding: {}",
            e
        )
    })
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

/// Canonical GitHub owner/repo for Taida releases.
///
/// `taida-lang/taida` is the source of truth. `shijimic/taida` is a
/// development fork and must NOT be used as a release source —
/// mirroring releases across the two caused `@c.14.rc3` to be invisible
/// to the C13 CLI's `taida upgrade` path. The fix is to point the
/// CLI at the canonical org and keep every external reference
/// (`install.sh`, docs, scaffolded `release.yml`) consistent.
const TAIDA_OWNER: &str = "taida-lang";
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
            return Err("upgrade requires the 'native' feature for platform detection".to_string());

            #[cfg(feature = "native")]
            {
                let archive_name = platform_archive_name(&version.tag, &host);
                println!("Downloading {} ...", archive_name);

                // Find archive download URL
                let download_url =
                    find_asset_url(TAIDA_OWNER, TAIDA_REPO, &version.tag, &archive_name)?;

                // Fetch release-signed SHA256SUMS and look up our archive's hash.
                let sha_url = find_asset_url(TAIDA_OWNER, TAIDA_REPO, &version.tag, "SHA256SUMS")
                    .map_err(|e| {
                    format!(
                        "[E32K1_UPGRADE_NO_SHA256SUMS] release {} must publish SHA256SUMS: {}",
                        version.tag, e
                    )
                })?;
                let sha_text = download_verified_sha256sums(&sha_url)?;
                let expected_sha = expected_sha256_for_archive(&sha_text, &archive_name)?;

                // Download archive with mandatory integrity check.
                let archive_bytes = download_and_verify(&download_url, &expected_sha)?;

                // Extract binary from archive
                let archive_base = archive_name
                    .strip_suffix(".tar.gz")
                    .or_else(|| archive_name.strip_suffix(".zip"))
                    .unwrap_or(&archive_name);

                println!("Extracting taida from {} ...", archive_name);

                let binary = if archive_name.ends_with(".tar.gz") {
                    extract_binary_from_tar_gz(&archive_bytes, archive_base)?
                } else {
                    // Windows .zip support (RC5B-505: deferred)
                    return Err(format!(
                        ".zip archive extraction not yet supported ({})",
                        archive_name
                    ));
                };

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
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_guard<F: FnOnce()>(f: F) {
        let _guard = match ENV_LOCK.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let prev_path = std::env::var("PATH").ok();
        let prev_api = std::env::var("TAIDA_GITHUB_API_URL").ok();
        f();
        unsafe {
            match prev_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
            match prev_api {
                Some(url) => std::env::set_var("TAIDA_GITHUB_API_URL", url),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
        }
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "e32b014_upgrade_{}_{}_{}",
                std::process::id(),
                nanos,
                label
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn install_fake_cosign(dir: &Path, log: &Path) {
        let bin = dir.join("fake-bin");
        fs::create_dir_all(&bin).unwrap();
        let cosign = bin.join("cosign");
        fs::write(
            &cosign,
            format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${{1:-}}" = "version" ]; then
  exit 0
fi
if [ "${{1:-}}" = "verify-blob" ]; then
  printf '%s\n' "$*" >> '{}'
  exit 0
fi
echo "unexpected fake cosign invocation: $*" >&2
exit 2
"#,
                log.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&cosign).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&cosign, perms).unwrap();
        }
        let old_path = std::env::var("PATH").unwrap_or_default();
        unsafe {
            std::env::set_var("PATH", format!("{}:{old_path}", bin.display()));
        }
    }

    // ── Canonical release source (security) ──

    /// SECURITY: the upgrade source MUST point at the canonical
    /// `taida-lang/taida` organization. Before this was enforced by a
    /// test, the constant was pinned to a personal fork (`shijimic`)
    /// which made the entire upgrade pipeline a single-account supply-
    /// chain target — if that account were compromised, sold, renamed,
    /// or deleted, every `taida upgrade` on the planet would either
    /// receive attacker-controlled binaries or silently break. The
    /// constants live alongside this test precisely so that any future
    /// edit hits the compiler and requires an explicit review, not a
    /// merge-by-mistake.
    #[test]
    fn canonical_release_source_is_taida_lang_org() {
        assert_eq!(
            TAIDA_OWNER, "taida-lang",
            "upgrade source must be the canonical org, not a personal fork"
        );
        assert_eq!(TAIDA_REPO, "taida");
    }

    #[test]
    fn e32b014_api_url_ignores_env_override() {
        with_env_guard(|| {
            unsafe {
                std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:9998");
            }
            assert_eq!(api_url(), "https://api.github.com");
        });
    }

    #[test]
    fn e32b014_missing_sha256sums_entry_is_hard_fail() {
        let err = expected_sha256_for_archive("abc123  other.tar.gz\n", "taida-@e.1-x.tar.gz")
            .expect_err("missing archive line must fail closed");
        assert!(
            err.contains("[E32K1_UPGRADE_NO_SHA256SUMS]") && err.contains("taida-@e.1-x.tar.gz"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn e32b014_sha256sums_requires_cosign_bundle() {
        let td = TestDir::new("missing_bundle");
        let sums = td.path().join("SHA256SUMS");
        fs::write(&sums, "abc123  taida-@e.1-x.tar.gz\n").unwrap();

        let err = download_verified_sha256sums(&format!("file://{}", sums.display()))
            .expect_err("SHA256SUMS without a cosign bundle must fail");
        assert!(
            err.contains("[E32K1_UPGRADE_SHA256SUMS_COSIGN_MISSING]"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn e32b014_sha256sums_cosign_uses_taida_release_identity() {
        with_env_guard(|| {
            let td = TestDir::new("identity");
            let sums = td.path().join("SHA256SUMS");
            let bundle = td.path().join("SHA256SUMS.cosign.bundle");
            let log = td.path().join("cosign.log");
            fs::write(&sums, "abc123  taida-@e.1-x.tar.gz\n").unwrap();
            fs::write(&bundle, "fake bundle").unwrap();
            install_fake_cosign(td.path(), &log);

            let text = download_verified_sha256sums(&format!("file://{}", sums.display()))
                .expect("fake cosign should verify SHA256SUMS");
            assert!(text.contains("taida-@e.1-x.tar.gz"));

            let log_text = fs::read_to_string(log).unwrap();
            assert!(
                log_text.contains("--certificate-identity-regexp")
                    && log_text.contains(UPGRADE_COSIGN_IDENTITY_REGEXP)
                    && log_text.contains("--certificate-oidc-issuer")
                    && log_text.contains("https://token.actions.githubusercontent.com"),
                "cosign invocation must pin taida-lang/taida workflow identity, got: {log_text}"
            );
        });
    }

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
        let tags = vec!["@a.7".to_string(), "@b.10".to_string(), "@b.11".to_string()];
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
        let tags = vec!["@b.10.rc2".to_string(), "@b.11".to_string()];
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

    // ── platform_archive_name ──

    #[test]
    fn archive_name_linux() {
        let name = platform_archive_name("@b.11", &HostTarget::X86_64LinuxGnu);
        assert_eq!(name, "taida-@b.11-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn archive_name_macos() {
        let name = platform_archive_name("@b.11", &HostTarget::Aarch64MacOs);
        assert_eq!(name, "taida-@b.11-aarch64-apple-darwin.tar.gz");
    }

    #[test]
    fn archive_name_windows() {
        let name = platform_archive_name("@b.11", &HostTarget::X86_64Windows);
        assert_eq!(name, "taida-@b.11-x86_64-pc-windows-msvc.zip");
    }

    #[test]
    fn archive_name_includes_tag() {
        let name = platform_archive_name("@b.10.rc2", &HostTarget::X86_64LinuxGnu);
        assert_eq!(name, "taida-@b.10.rc2-x86_64-unknown-linux-gnu.tar.gz");
    }

    // ── lookup_sha256 ──

    #[test]
    fn lookup_sha256_finds_match() {
        let sums = "\
abc123  taida-@b.11-x86_64-unknown-linux-gnu.tar.gz\n\
def456  taida-@b.11-aarch64-apple-darwin.tar.gz\n";
        let result = lookup_sha256(sums, "taida-@b.11-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn lookup_sha256_not_found() {
        let sums = "abc123  other-file.tar.gz\n";
        let result = lookup_sha256(sums, "taida-@b.11-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(result, None);
    }

    #[test]
    fn lookup_sha256_handles_double_space() {
        // sha256sum output uses two spaces between hash and filename
        let sums = "abc123  taida-@b.11-x86_64-unknown-linux-gnu.tar.gz\n";
        let result = lookup_sha256(sums, "taida-@b.11-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(result, Some("abc123".to_string()));
    }

    // ── extract_binary_from_tar_gz ──

    #[test]
    fn extract_binary_from_tar_gz_works() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let binary_content = b"fake taida binary";
        let archive_base = "taida-@b.11-x86_64-unknown-linux-gnu";

        // Build a tar.gz in memory
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar_builder = tar::Builder::new(&mut encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(binary_content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar_builder
                .append_data(
                    &mut header,
                    format!("{}/taida", archive_base),
                    &binary_content[..],
                )
                .unwrap();
            tar_builder.finish().unwrap();
        }
        let archive_bytes = encoder.finish().unwrap();

        let extracted = extract_binary_from_tar_gz(&archive_bytes, archive_base).unwrap();
        assert_eq!(extracted, binary_content);
    }

    #[test]
    fn extract_binary_from_tar_gz_missing_binary() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let archive_base = "taida-@b.11-x86_64-unknown-linux-gnu";

        // Build a tar.gz with only a README (no taida binary)
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar_builder = tar::Builder::new(&mut encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            tar_builder
                .append_data(
                    &mut header,
                    format!("{}/README.md", archive_base),
                    b"hello" as &[u8],
                )
                .unwrap();
            tar_builder.finish().unwrap();
        }
        let archive_bytes = encoder.finish().unwrap();

        let result = extract_binary_from_tar_gz(&archive_bytes, archive_base);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found in archive"));
    }

    // ── download_and_verify (sha mismatch) ──

    #[test]
    fn verify_sha_mismatch_is_error() {
        let data = b"hello world";
        let actual_sha = crypto::sha256_hex_bytes(data);
        let wrong_sha = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_ne!(actual_sha, wrong_sha);
        let err = verify_sha256_bytes(data, wrong_sha).unwrap_err();
        assert!(err.contains("[E32K1_UPGRADE_SHA256_MISMATCH]"));
    }
}
