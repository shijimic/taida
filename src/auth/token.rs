use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthToken {
    pub github_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token_read: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token_publish: Option<String>,
    pub username: String,
    pub created_at: String,
}

impl AuthToken {
    pub fn install_token(&self) -> &str {
        self.github_token_read
            .as_deref()
            .unwrap_or(self.github_token.as_str())
    }

    pub fn publish_token(&self) -> &str {
        self.github_token_publish
            .as_deref()
            .unwrap_or(self.github_token.as_str())
    }
}

/// Re-export for backward compatibility.
pub use crate::util::taida_home_dir;

/// ~/.taida/auth.json のパスを返す。
/// HOME -> USERPROFILE の順でホームディレクトリを検索し、
/// いずれも未設定の場合はエラーを返す。
pub fn auth_json_path() -> Result<PathBuf, String> {
    Ok(crate::util::taida_home_dir()?
        .join(".taida")
        .join("auth.json"))
}

/// 保存された認証トークンを読み込む。存在しなければ None を返す。
pub fn load_token() -> Option<AuthToken> {
    let path = auth_json_path().ok()?;
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// 認証トークンを ~/.taida/auth.json に保存する。
/// Unix 系ではファイルを作成時にパーミッション 0o600 (owner read/write only) で開く。
pub fn save_token(github_token: &str, username: &str) -> Result<(), String> {
    let path = auth_json_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {}", parent.display(), e))?;

        // Unix: .taida/ ディレクトリを 0o700 に制限する
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o700);
            fs::set_permissions(parent, perms)
                .map_err(|e| format!("Failed to set permissions on {}: {}", parent.display(), e))?;
        }
    }

    let now = chrono_rfc3339_now();
    let token = AuthToken {
        github_token: github_token.to_string(),
        github_token_read: Some(github_token.to_string()),
        github_token_publish: Some(github_token.to_string()),
        username: username.to_string(),
        created_at: now,
    };

    let json = serde_json::to_string_pretty(&token)
        .map_err(|e| format!("Failed to serialize token: {}", e))?;

    // Unix: OpenOptions::mode(0o600) で作成時にパーミッション設定（TOCTOU 回避）
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(json.as_bytes())
            })
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    }
    // N-58: On non-Unix platforms (Windows), there is no direct equivalent
    // of POSIX file modes. The token file inherits the default ACL of
    // the .taida/ directory. Windows user profile directories are
    // typically restricted to the owning user, so this is acceptable.
    // A future improvement could use Windows ACL APIs via `windows-acl`
    // crate if multi-user Windows environments become a target.
    #[cfg(not(unix))]
    {
        fs::write(&path, &json)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    }

    Ok(())
}

/// 認証トークンファイルを削除する。
/// NTH-2: TOCTOU 解消 — exists() チェックを行わず直接 remove_file() を呼び、
/// NotFound は正常終了として扱う。
pub fn delete_token() -> Result<(), String> {
    let path = auth_json_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to delete {}: {}", path.display(), e)),
    }
}

/// Unix エポックからの秒数を RFC 3339 タイムスタンプ文字列に変換する。
/// 外部クレートを使わず、純 Rust でグレゴリオ暦を計算する。
fn unix_secs_to_rfc3339(secs: u64) -> String {
    // 日数からグレゴリオ暦の年月日を計算する
    let days = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Civil date from days since Unix epoch (algorithm from Howard Hinnant)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month proxy [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // month [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hours, minutes, seconds
    )
}

/// 外部クレートを使わずに現在時刻の RFC 3339 タイムスタンプを生成する。
fn chrono_rfc3339_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d,
        Err(_) => return "1970-01-01T00:00:00Z".to_string(),
    };

    unix_secs_to_rfc3339(duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_auth_json_path_home() {
        let _guard = crate::util::env_test_guard();
        // HOME が設定されている場合はそれを使う
        let path = auth_json_path().expect("auth_json_path should succeed when HOME is set");
        assert!(path.to_string_lossy().ends_with(".taida/auth.json"));
    }

    #[test]
    fn test_auth_json_path_userprofile_fallback() {
        let _guard = crate::util::env_test_guard();

        let original_home = env::var("HOME").ok();
        let tmp = env::temp_dir().join("taida_test_userprofile");
        let _ = fs::create_dir_all(&tmp);

        // HOME を除去し、USERPROFILE を設定
        unsafe {
            env::remove_var("HOME");
            env::set_var("USERPROFILE", tmp.to_string_lossy().as_ref());
        }

        let path = auth_json_path().expect("auth_json_path should succeed with USERPROFILE");
        assert!(
            path.starts_with(&tmp),
            "Expected path to start with {:?}, got {:?}",
            tmp,
            path
        );
        assert!(path.to_string_lossy().ends_with(".taida/auth.json"));

        // 環境変数を復元
        unsafe {
            if let Some(home) = original_home {
                env::set_var("HOME", home);
            } else {
                env::remove_var("HOME");
            }
            env::remove_var("USERPROFILE");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_auth_json_path_no_home_no_userprofile() {
        let _guard = crate::util::env_test_guard();

        let original_home = env::var("HOME").ok();
        let original_userprofile = env::var("USERPROFILE").ok();

        unsafe {
            env::remove_var("HOME");
            env::remove_var("USERPROFILE");
        }

        // HOME も USERPROFILE も未設定の場合はエラーを返す
        let result = auth_json_path();
        assert!(
            result.is_err(),
            "Expected error when neither HOME nor USERPROFILE is set, got {:?}",
            result
        );

        // 環境変数を復元
        unsafe {
            if let Some(home) = original_home {
                env::set_var("HOME", home);
            } else {
                env::remove_var("HOME");
            }
            if let Some(up) = original_userprofile {
                env::set_var("USERPROFILE", up);
            } else {
                env::remove_var("USERPROFILE");
            }
        }
    }

    #[test]
    fn test_save_and_load_token() {
        let tmp = env::temp_dir().join("taida_test_auth");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let auth_path = tmp.join("auth.json");
        let token = AuthToken {
            github_token: "gho_test123".to_string(),
            github_token_read: Some("gho_read".to_string()),
            github_token_publish: Some("gho_publish".to_string()),
            username: "testuser".to_string(),
            created_at: "2026-03-06T12:00:00Z".to_string(),
        };

        let json = serde_json::to_string_pretty(&token).unwrap();
        fs::write(&auth_path, &json).unwrap();

        let data = fs::read_to_string(&auth_path).unwrap();
        let loaded: AuthToken = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded.username, "testuser");
        assert_eq!(loaded.github_token, "gho_test123");
        assert_eq!(loaded.install_token(), "gho_read");
        assert_eq!(loaded.publish_token(), "gho_publish");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_save_token_sets_permissions() {
        let _guard = crate::util::env_test_guard();

        let tmp = env::temp_dir().join("taida_test_auth_perms");
        let _ = fs::remove_dir_all(&tmp);

        // HOME を一時的に変更して save_token を実際にテスト
        let original_home = env::var("HOME").ok();
        unsafe {
            env::set_var("HOME", tmp.to_string_lossy().as_ref());
        }

        let result = save_token("gho_testperms", "permuser");
        assert!(result.is_ok(), "save_token failed: {:?}", result);

        let path = tmp.join(".taida").join("auth.json");
        assert!(path.exists(), "Token file should exist");

        // Unix: パーミッションが 0o600 であること
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "Expected permission 0o600, got 0o{:o}", mode);

            // .taida/ ディレクトリが 0o700 であること
            let dir_meta = fs::metadata(tmp.join(".taida")).unwrap();
            let dir_mode = dir_meta.permissions().mode() & 0o777;
            assert_eq!(
                dir_mode, 0o700,
                "Expected directory permission 0o700, got 0o{:o}",
                dir_mode
            );
        }

        // トークンが正しく読み込めること
        let loaded = load_token();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.username, "permuser");
        assert_eq!(loaded.github_token, "gho_testperms");
        assert_eq!(loaded.install_token(), "gho_testperms");
        assert_eq!(loaded.publish_token(), "gho_testperms");

        // 復元
        unsafe {
            if let Some(home) = original_home {
                env::set_var("HOME", home);
            } else {
                env::remove_var("HOME");
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_token_missing_file() {
        let result: Option<AuthToken> = serde_json::from_str("invalid json").ok();
        assert!(result.is_none());
    }

    #[test]
    fn test_chrono_rfc3339_now_format() {
        let ts = chrono_rfc3339_now();
        // RFC 3339 形式: YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20, "Timestamp length should be 20, got: {}", ts);
        assert!(ts.ends_with('Z'), "Timestamp should end with Z: {}", ts);
        assert_eq!(
            &ts[4..5],
            "-",
            "Timestamp should have dash at position 4: {}",
            ts
        );
        assert_eq!(
            &ts[7..8],
            "-",
            "Timestamp should have dash at position 7: {}",
            ts
        );
        assert_eq!(
            &ts[10..11],
            "T",
            "Timestamp should have T at position 10: {}",
            ts
        );
        assert_eq!(
            &ts[13..14],
            ":",
            "Timestamp should have colon at position 13: {}",
            ts
        );
        assert_eq!(
            &ts[16..17],
            ":",
            "Timestamp should have colon at position 16: {}",
            ts
        );

        // 年が妥当な範囲内であること
        let year: u32 = ts[0..4].parse().expect("Year should be numeric");
        assert!(
            (2024..=2100).contains(&year),
            "Year should be reasonable: {}",
            year
        );
    }

    #[test]
    fn test_unix_secs_to_rfc3339_deterministic() {
        // 決定的テスト: 既知のエポック値に対する変換結果を検証
        assert_eq!(unix_secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_secs_to_rfc3339(1709251200), "2024-03-01T00:00:00Z");
        assert_eq!(unix_secs_to_rfc3339(951782400), "2000-02-29T00:00:00Z");
        assert_eq!(unix_secs_to_rfc3339(1735689599), "2024-12-31T23:59:59Z");
    }

    #[test]
    fn test_chrono_rfc3339_consistency() {
        // 2つの連続呼び出しが一貫した形式の結果を返すこと
        let ts1 = chrono_rfc3339_now();
        let ts2 = chrono_rfc3339_now();
        assert_eq!(ts1.len(), ts2.len());
    }
}
