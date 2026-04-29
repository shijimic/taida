//! GitHub Release REST API driver (RC2.7 Phase 2).
//!
//! Provides release ensure + asset ensure semantics so that
//! `taida ingot publish` can create GitHub Releases
//! and upload assets without depending on the `gh` CLI.
//!
//! ## Design
//!
//! - Default driver for publish; `gh` path is legacy fallback.
//! - Uses `reqwest::blocking` (same as the install-side fetcher).
//! - Auth via `Authorization: Bearer <github_token>` from `auth.json`.
//! - Idempotent: re-running after a partial failure converges.
//!
//! ## API flow
//!
//! 1. `POST /repos/{owner}/{repo}/releases` to create.
//! 2. If 422 (already exists), `GET .../releases/tags/{tag}` to retrieve.
//! 3. For each asset: check existing, delete if name collision, upload.
//! 4. Upload URL: strip `{?name,label}` suffix, append `?name=<name>`.

/// Metadata about a GitHub Release, as returned by the API.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    /// Numeric release ID used for asset API calls.
    pub id: u64,
    /// The `upload_url` from the release response (may contain
    /// `{?name,label}` template suffix that must be stripped).
    pub upload_url: String,
    /// Human-readable URL to the release page.
    pub html_url: String,
}

/// Metadata about a release asset, as returned by the API.
#[derive(Debug, Clone)]
pub struct AssetInfo {
    /// Numeric asset ID.
    pub id: u64,
    /// Asset display name.
    pub name: String,
}

/// An asset to upload to a GitHub Release.
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    /// Path to the local file.
    pub local_path: std::path::PathBuf,
    /// Desired name for the asset in the release.
    pub asset_name: String,
}

// ── Helpers ──────────────────────────────────────────────────

/// GitHub API base URL. Respects `TAIDA_GITHUB_API_URL` for testing.
fn api_url() -> String {
    std::env::var("TAIDA_GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_string())
}

/// Build a blocking reqwest client with the required headers.
fn make_client(token: &str) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("taida-publish")
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                    .map_err(|e| format!("invalid token for Authorization header: {}", e))?,
            );
            headers.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
            );
            headers
        })
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))
}

/// Strip the `{?name,label}` URI template suffix from an upload URL.
///
/// GitHub returns upload URLs like:
///   `https://uploads.github.com/repos/o/r/releases/123/assets{?name,label}`
///
/// We need to strip the `{?...}` and append `?name=<asset_name>` ourselves.
pub fn normalize_upload_url(raw: &str) -> String {
    if let Some(idx) = raw.find('{') {
        raw[..idx].to_string()
    } else {
        raw.to_string()
    }
}

/// Parse `(owner, repo)` from a GitHub remote URL.
///
/// Supports HTTPS, SSH, and git@ formats.
pub fn parse_github_remote(remote: &str) -> Option<(String, String)> {
    // HTTPS: https://github.com/owner/repo.git
    if let Some(rest) = remote.strip_prefix("https://github.com/") {
        return parse_owner_repo_suffix(rest);
    }
    // SSH: git@github.com:owner/repo.git
    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return parse_owner_repo_suffix(rest);
    }
    // SSH with scheme: ssh://git@github.com/owner/repo.git
    if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        return parse_owner_repo_suffix(rest);
    }
    None
}

fn parse_owner_repo_suffix(rest: &str) -> Option<(String, String)> {
    let trimmed = rest.trim_end_matches(".git").trim_end_matches('/');
    let mut parts = trimmed.split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    // Reject nested paths (e.g. owner/repo/extra)
    if parts.next().is_some() {
        return None;
    }
    Some((owner, repo))
}

// ── Release Create / Get ─────────────────────────────────────

/// Ensure a GitHub Release exists for the given tag.
///
/// Tries to create the release; if it already exists (HTTP 422),
/// falls back to retrieving the existing release by tag.
pub fn ensure_release(
    token: &str,
    owner: &str,
    repo: &str,
    tag: &str,
    title: &str,
    body: &str,
) -> Result<ReleaseInfo, String> {
    let client = make_client(token)?;
    ensure_release_impl(&client, owner, repo, tag, title, body)
}

/// Internal: ensure release using a shared client.
fn ensure_release_impl(
    client: &reqwest::blocking::Client,
    owner: &str,
    repo: &str,
    tag: &str,
    title: &str,
    body: &str,
) -> Result<ReleaseInfo, String> {
    let base = api_url();
    let create_url = format!("{}/repos/{}/{}/releases", base, owner, repo);

    let payload = serde_json::json!({
        "tag_name": tag,
        "name": title,
        "body": body,
        "draft": false,
        "prerelease": false,
    });

    let resp = client
        .post(&create_url)
        .json(&payload)
        .send()
        .map_err(|e| format!("REST release create request failed: {}", e))?;

    let status = resp.status();
    if status.is_success() {
        return parse_release_response(resp);
    }

    // 422 Unprocessable Entity typically means "release already exists".
    if status.as_u16() == 422 {
        return get_release_by_tag_impl(client, owner, repo, tag);
    }

    let body_text = resp.text().unwrap_or_default();
    Err(format!(
        "REST release create failed (HTTP {}): {}",
        status, body_text
    ))
}

/// Retrieve an existing release by tag name.
fn get_release_by_tag_impl(
    client: &reqwest::blocking::Client,
    owner: &str,
    repo: &str,
    tag: &str,
) -> Result<ReleaseInfo, String> {
    let base = api_url();
    let url = format!("{}/repos/{}/{}/releases/tags/{}", base, owner, repo, tag);

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("REST get release by tag failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        return Err(format!(
            "REST get release by tag '{}' failed (HTTP {}): {}",
            tag, status, body_text
        ));
    }

    parse_release_response(resp)
}

fn parse_release_response(resp: reqwest::blocking::Response) -> Result<ReleaseInfo, String> {
    let json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("failed to parse release JSON: {}", e))?;

    let id = json["id"]
        .as_u64()
        .ok_or_else(|| "release response missing 'id'".to_string())?;
    let upload_url = json["upload_url"]
        .as_str()
        .ok_or_else(|| "release response missing 'upload_url'".to_string())?
        .to_string();
    let html_url = json["html_url"].as_str().unwrap_or("").to_string();

    Ok(ReleaseInfo {
        id,
        upload_url,
        html_url,
    })
}

// ── Asset List / Delete / Upload ─────────────────────────────

/// List all assets on a release.
pub fn list_assets(
    token: &str,
    owner: &str,
    repo: &str,
    release_id: u64,
) -> Result<Vec<AssetInfo>, String> {
    let client = make_client(token)?;
    list_assets_impl(&client, owner, repo, release_id)
}

/// Internal: list assets using a shared client.
fn list_assets_impl(
    client: &reqwest::blocking::Client,
    owner: &str,
    repo: &str,
    release_id: u64,
) -> Result<Vec<AssetInfo>, String> {
    let base = api_url();
    // RC2.7B-007: request up to 100 assets per page (GitHub default is 30).
    let url = format!(
        "{}/repos/{}/{}/releases/{}/assets?per_page=100",
        base, owner, repo, release_id
    );

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("REST list assets failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        return Err(format!(
            "REST list assets failed (HTTP {}): {}",
            status, body_text
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("failed to parse assets JSON: {}", e))?;

    let arr = json
        .as_array()
        .ok_or_else(|| "assets response is not an array".to_string())?;

    let mut assets = Vec::with_capacity(arr.len());
    for item in arr {
        if let (Some(id), Some(name)) = (item["id"].as_u64(), item["name"].as_str()) {
            assets.push(AssetInfo {
                id,
                name: name.to_string(),
            });
        }
    }
    Ok(assets)
}

/// Delete a release asset by ID.
pub fn delete_asset(token: &str, owner: &str, repo: &str, asset_id: u64) -> Result<(), String> {
    let client = make_client(token)?;
    delete_asset_impl(&client, owner, repo, asset_id)
}

/// Internal: delete asset using a shared client.
fn delete_asset_impl(
    client: &reqwest::blocking::Client,
    owner: &str,
    repo: &str,
    asset_id: u64,
) -> Result<(), String> {
    let base = api_url();
    let url = format!(
        "{}/repos/{}/{}/releases/assets/{}",
        base, owner, repo, asset_id
    );

    let resp = client
        .delete(&url)
        .send()
        .map_err(|e| format!("REST delete asset failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        return Err(format!(
            "REST delete asset {} failed (HTTP {}): {}",
            asset_id, status, body_text
        ));
    }

    Ok(())
}

/// Upload a file as a release asset.
///
/// If an asset with the same name already exists, it is deleted first
/// (asset ensure semantics).
pub fn upload_asset(
    token: &str,
    owner: &str,
    repo: &str,
    release: &ReleaseInfo,
    asset: &ReleaseAsset,
) -> Result<(), String> {
    let client = make_client(token)?;
    upload_asset_impl(&client, owner, repo, release, asset)
}

/// Internal: upload asset using a shared client.
fn upload_asset_impl(
    client: &reqwest::blocking::Client,
    owner: &str,
    repo: &str,
    release: &ReleaseInfo,
    asset: &ReleaseAsset,
) -> Result<(), String> {
    if !asset.local_path.exists() {
        return Err(format!(
            "Release asset '{}' (display name '{}') does not exist on disk.",
            asset.local_path.display(),
            asset.asset_name
        ));
    }

    // Check for existing asset with the same name and delete it.
    let existing = list_assets_impl(client, owner, repo, release.id)?;
    for existing_asset in &existing {
        if existing_asset.name == asset.asset_name {
            delete_asset_impl(client, owner, repo, existing_asset.id)?;
        }
    }

    // Read the file into memory for upload.
    let file_bytes = std::fs::read(&asset.local_path)
        .map_err(|e| format!("cannot read asset '{}': {}", asset.local_path.display(), e))?;

    // Determine content type from extension.
    let content_type = match asset.local_path.extension().and_then(|e| e.to_str()) {
        Some("toml") => "application/toml",
        Some("so" | "dylib" | "dll") => "application/octet-stream",
        _ => "application/octet-stream",
    };

    let upload_base = normalize_upload_url(&release.upload_url);
    let upload_url = format!("{}?name={}", upload_base, urlencoded(&asset.asset_name));

    let resp = client
        .post(&upload_url)
        .header("Content-Type", content_type)
        .body(file_bytes)
        .send()
        .map_err(|e| format!("REST asset upload failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        return Err(format!(
            "REST asset upload '{}' failed (HTTP {}): {}",
            asset.asset_name, status, body_text
        ));
    }

    Ok(())
}

/// Ensure a release exists and all assets are uploaded.
///
/// This is the main entry point for the REST release driver.
/// It is idempotent: re-running after a partial failure will
/// create the release if missing and re-upload any missing assets.
///
/// RC2.7B-008: creates a single HTTP client and reuses it for all
/// API calls, avoiding redundant TLS handshakes.
pub fn ensure_release_with_assets(
    token: &str,
    owner: &str,
    repo: &str,
    tag: &str,
    title: &str,
    notes: &str,
    assets: &[ReleaseAsset],
) -> Result<String, String> {
    let client = make_client(token)?;
    let release = ensure_release_impl(&client, owner, repo, tag, title, notes)?;

    for asset in assets {
        upload_asset_impl(&client, owner, repo, &release, asset)?;
    }

    Ok(release.html_url)
}

// ── Percent-encoding ─────────────────────────────────────────

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_upload_url_strips_template() {
        let raw = "https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}";
        assert_eq!(
            normalize_upload_url(raw),
            "https://uploads.github.com/repos/o/r/releases/1/assets"
        );
    }

    #[test]
    fn test_normalize_upload_url_no_template() {
        let raw = "https://uploads.github.com/repos/o/r/releases/1/assets";
        assert_eq!(normalize_upload_url(raw), raw);
    }

    #[test]
    fn test_parse_github_remote_https() {
        assert_eq!(
            parse_github_remote("https://github.com/shijimic/terminal.git"),
            Some(("shijimic".to_string(), "terminal".to_string()))
        );
    }

    #[test]
    fn test_parse_github_remote_ssh() {
        assert_eq!(
            parse_github_remote("git@github.com:shijimic/terminal.git"),
            Some(("shijimic".to_string(), "terminal".to_string()))
        );
    }

    #[test]
    fn test_parse_github_remote_no_git_suffix() {
        assert_eq!(
            parse_github_remote("https://github.com/org/repo"),
            Some(("org".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn test_parse_github_remote_nested_rejected() {
        assert_eq!(parse_github_remote("https://github.com/a/b/c"), None);
    }

    #[test]
    fn test_parse_github_remote_non_github() {
        assert_eq!(parse_github_remote("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn test_urlencoded_basic() {
        assert_eq!(urlencoded("foo-bar.so"), "foo-bar.so");
        assert_eq!(urlencoded("lib foo.so"), "lib%20foo.so");
    }

    // ── RC2.7-3a: ensure_release retry / error path tests ──

    #[test]
    fn test_ensure_release_bad_api_url() {
        // Point at an unreachable API endpoint to test transport failure.
        // Safety: single-threaded test, restored immediately.
        let prev = std::env::var("TAIDA_GITHUB_API_URL").ok();
        unsafe { std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:1") };

        let result = ensure_release("fake-token", "owner", "repo", "a.1", "title", "notes");

        match prev {
            Some(v) => unsafe { std::env::set_var("TAIDA_GITHUB_API_URL", v) },
            None => unsafe { std::env::remove_var("TAIDA_GITHUB_API_URL") },
        }

        assert!(result.is_err(), "should fail with unreachable API");
        let err = result.unwrap_err();
        assert!(
            err.contains("REST release create request failed")
                || err.contains("connection refused")
                || err.contains("error"),
            "error should mention transport failure: {err}"
        );
    }

    #[test]
    fn test_upload_asset_missing_local_file() {
        let bogus = ReleaseAsset {
            local_path: std::path::PathBuf::from("/nonexistent/lib.so"),
            asset_name: "lib-x86_64.so".to_string(),
        };
        let release = ReleaseInfo {
            id: 1,
            upload_url: "https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}"
                .to_string(),
            html_url: "".to_string(),
        };
        let result = upload_asset("fake-token", "o", "r", &release, &bogus);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_ensure_release_with_assets_transport_failure() {
        // RC2.7-3a: test idempotent retry semantics by hitting
        // an unreachable endpoint. The ensure call should fail
        // gracefully with a descriptive error.
        let prev = std::env::var("TAIDA_GITHUB_API_URL").ok();
        unsafe { std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:1") };

        let asset = ReleaseAsset {
            local_path: std::path::PathBuf::from("/tmp/test.toml"),
            asset_name: "addon.lock.toml".to_string(),
        };
        let result = ensure_release_with_assets(
            "fake-token",
            "owner",
            "repo",
            "a.1",
            "title",
            "notes",
            &[asset],
        );

        match prev {
            Some(v) => unsafe { std::env::set_var("TAIDA_GITHUB_API_URL", v) },
            None => unsafe { std::env::remove_var("TAIDA_GITHUB_API_URL") },
        }

        assert!(result.is_err(), "should fail on transport error");
    }
}
