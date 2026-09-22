use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

const GITHUB_CLIENT_ID: &str = "Iv23lifup4LV7q6HEz05";

#[derive(Debug, Deserialize)]
pub struct DeviceFlowResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
}

fn default_expires_in() -> u64 {
    900
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
}

/// GitHub デバイス認可フローを開始する。
/// 表示用のユーザーコードを含むデバイスコード情報を返す。
pub fn start_device_flow() -> Result<DeviceFlowResponse, String> {
    let client = crate::util::http_client()?;

    let mut params = HashMap::new();
    params.insert("client_id", GITHUB_CLIENT_ID);
    params.insert("scope", "read:user public_repo");

    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .map_err(|e| format!("Failed to start device flow: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        // N-46: preserve response body for debugging; unwrap_or explains
        // why the body might be absent (network error after status read)
        let body = resp
            .text()
            .unwrap_or_else(|e| format!("<failed to read response body: {}>", e));
        return Err(format!("GitHub returned status {}: {}", status, body));
    }

    resp.json::<DeviceFlowResponse>()
        .map_err(|e| format!("Failed to parse device flow response: {}", e))
}

/// ユーザーがコードを入力した後、GitHub にアクセストークンをポーリングする。
/// 成功・期限切れ・エラーのいずれかまでブロックする。
pub fn poll_for_token(device_code: &str, interval: u64, expires_in: u64) -> Result<String, String> {
    let client = crate::util::http_client()?;
    let mut poll_interval = interval.clamp(1, 60);
    let deadline = Instant::now() + Duration::from_secs(expires_in.max(1));

    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("Device code expired. Please run `taida auth login` again.".to_string());
        }
        let remaining = deadline.saturating_duration_since(now);
        thread::sleep(Duration::from_secs(poll_interval).min(remaining));

        let mut params = HashMap::new();
        params.insert("client_id", GITHUB_CLIENT_ID);
        params.insert("device_code", device_code);
        params.insert("grant_type", "urn:ietf:params:oauth:grant-type:device_code");

        let resp = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .map_err(|e| format!("Failed to poll for token: {}", e))?;

        let token_resp: TokenResponse = resp
            .json()
            .map_err(|e| format!("Failed to parse token response: {}", e))?;

        if let Some(token) = token_resp.access_token {
            return Ok(token);
        }

        match token_resp.error.as_deref() {
            Some("authorization_pending") => {
                // ユーザーがまだコードを入力していない、ポーリングを継続
                print!(".");
                io::stdout().flush().ok();
            }
            Some("slow_down") => {
                // ポーリング間隔を延長
                poll_interval = token_resp
                    .interval
                    .unwrap_or(poll_interval + 5)
                    .clamp(1, 60);
            }
            Some("expired_token") => {
                return Err("Device code expired. Please run `taida auth login` again.".to_string());
            }
            Some("access_denied") => {
                return Err("Authorization was denied by the user.".to_string());
            }
            Some(err) => {
                return Err(format!("GitHub OAuth error: {}", err));
            }
            None => {
                return Err("Unexpected response from GitHub (no token and no error).".to_string());
            }
        }
    }
}

/// 指定されたアクセストークンに対応する GitHub ユーザー名を取得する。
pub fn get_github_username(token: &str) -> Result<String, String> {
    let client = crate::util::http_client()?;

    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "taida-cli")
        .header("Accept", "application/json")
        .send()
        .map_err(|e| format!("Failed to fetch GitHub user: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        // N-46: preserve response body for debugging
        let body = resp
            .text()
            .unwrap_or_else(|e| format!("<failed to read response body: {}>", e));
        return Err(format!("GitHub API returned status {}: {}", status, body));
    }

    let user: GitHubUser = resp
        .json()
        .map_err(|e| format!("Failed to parse GitHub user response: {}", e))?;

    Ok(user.login)
}
