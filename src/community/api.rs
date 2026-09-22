use std::env;

/// API のベース URL を返す。環境変数 TAIDA_API_URL が設定されていればそれを使い、
/// なければ "https://taida.dev" をデフォルトとする。
pub fn api_base_url() -> String {
    env::var("TAIDA_API_URL").unwrap_or_else(|_| "https://taida.dev".to_string())
}

/// taida.dev API に GET リクエストを送信する。
/// (ステータスコード, レスポンスボディ) を返す。
pub fn api_get(path: &str, token: Option<&str>) -> Result<(u16, String), String> {
    let url = format!("{}{}", api_base_url(), path);
    let client = crate::util::http_client()?;

    let mut req = client
        .get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", "taida-cli");

    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }

    let resp = req
        .send()
        .map_err(|e| format!("Failed to connect to taida.dev: {}", e))?;

    let status = resp.status().as_u16();
    let body = resp
        .text()
        .map_err(|e| format!("Failed to read response: {}", e))?;

    Ok((status, body))
}

/// taida.dev API に JSON ボディ付きで POST リクエストを送信する。
/// (ステータスコード, レスポンスボディ) を返す。
pub fn api_post(
    path: &str,
    body: &serde_json::Value,
    token: Option<&str>,
) -> Result<(u16, String), String> {
    let url = format!("{}{}", api_base_url(), path);
    let client = crate::util::http_client()?;

    let mut req = client
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", "taida-cli");

    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }

    let resp = req
        .json(body)
        .send()
        .map_err(|e| format!("Failed to connect to taida.dev: {}", e))?;

    let status = resp.status().as_u16();
    let body_text = resp
        .text()
        .map_err(|e| format!("Failed to read response: {}", e))?;

    Ok((status, body_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn api_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_api_base_url_default() {
        let _guard = api_env_lock().lock().unwrap();

        // このテスト用に環境変数を一時的に削除
        let original = env::var("TAIDA_API_URL").ok();
        unsafe {
            env::remove_var("TAIDA_API_URL");
        }

        assert_eq!(api_base_url(), "https://taida.dev");

        // 復元
        if let Some(val) = original {
            unsafe {
                env::set_var("TAIDA_API_URL", val);
            }
        }
    }

    #[test]
    fn test_api_base_url_override() {
        let _guard = api_env_lock().lock().unwrap();

        let original = env::var("TAIDA_API_URL").ok();
        unsafe {
            env::set_var("TAIDA_API_URL", "http://localhost:8787");
        }

        assert_eq!(api_base_url(), "http://localhost:8787");

        // 復元
        unsafe {
            env::remove_var("TAIDA_API_URL");
        }
        if let Some(val) = original {
            unsafe {
                env::set_var("TAIDA_API_URL", val);
            }
        }
    }
}
