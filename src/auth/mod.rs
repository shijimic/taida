pub mod device_flow;
pub mod token;

use token::{delete_token, load_token, load_token_checked, save_token};

fn is_help_flag(raw: &str) -> bool {
    matches!(raw, "--help" | "-h")
}

fn print_auth_help() {
    println!(
        "\
Usage:
  taida auth <login|logout|status>

Examples:
  taida auth login
  taida auth status"
    );
}

fn print_auth_login_help() {
    println!(
        "\
Usage:
  taida auth login

Behavior:
  Start the GitHub Device Authorization Flow and store the resulting token locally."
    );
}

fn print_auth_logout_help() {
    println!(
        "\
Usage:
  taida auth logout

Behavior:
  Delete the locally stored authentication token, if present."
    );
}

fn print_auth_status_help() {
    println!(
        "\
Usage:
  taida auth status

Behavior:
  Print the currently authenticated user and token creation timestamp, if logged in."
    );
}

pub fn run_auth(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: taida auth <login|logout|status>");
        std::process::exit(1);
    }

    match args[0].as_str() {
        "--help" | "-h" => print_auth_help(),
        "login" => run_login(&args[1..]),
        "logout" => run_logout(&args[1..]),
        "status" => run_status(&args[1..]),
        other => {
            eprintln!("Unknown auth command: {}", other);
            eprintln!("Usage: taida auth <login|logout|status>");
            std::process::exit(1);
        }
    }
}

fn run_login(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_auth_login_help();
            return;
        }
        _ => {
            eprintln!("Usage: taida auth login");
            std::process::exit(1);
        }
    }

    // すでにログイン済みか確認
    if let Some(existing) = load_token() {
        println!(
            "Already logged in as {}. Run `taida auth logout` first to re-authenticate.",
            existing.username
        );
        return;
    }

    println!("Starting GitHub Device Authorization Flow...\n");

    // ステップ1: デバイスコードをリクエスト
    let flow = match device_flow::start_device_flow() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // ステップ2: ユーザーへの案内を表示 (可能ならブラウザも開く)
    println!("Open this URL in your browser:");
    println!("  {}", hyperlink(&flow.verification_uri));
    println!();
    println!("Enter code: {}", flow.user_code);
    println!();
    if try_open_browser(&flow.verification_uri) {
        println!("(opened in your browser)");
        println!();
    }
    println!("Waiting for authorization...");

    // ステップ3: トークンをポーリングで取得
    let access_token =
        match device_flow::poll_for_token(&flow.device_code, flow.interval, flow.expires_in) {
            Ok(t) => {
                println!(); // ドットの後の改行
                t
            }
            Err(e) => {
                println!();
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };

    // ステップ4: ユーザー名を取得
    let username = match device_flow::get_github_username(&access_token) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // ステップ5: GitHub トークンを taida.dev セッショントークンに交換する。
    // community API が受けるのはこのセッショントークンだけで、GitHub
    // トークンは GitHub への呼び出し (ingot install / publish) 専用に残る。
    println!("Exchanging the GitHub token for a taida.dev session...");
    let taida_token = match exchange_device_token(&access_token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // ステップ6: トークンを保存
    match save_token(&access_token, Some(&taida_token), &username) {
        Ok(()) => {
            println!("Logged in as {}.", username);
        }
        Err(e) => {
            eprintln!("Error saving token: {}", e);
            std::process::exit(1);
        }
    }
}

/// URL を OSC 8 ハイパーリンクで包む (stdout が TTY のときのみ)。
/// 対応ターミナルではクリックで開ける。非対応ターミナルはエスケープを
/// 無視してプレーンな URL を表示する。
fn hyperlink(url: &str) -> String {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        format!("\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\")
    } else {
        url.to_string()
    }
}

/// 既定ブラウザで URL を開くことを試みる。ヘッドレス環境や起動失敗は
/// false を返すだけ — 呼び出し側は URL 表示にフォールバック済みなので
/// ログインフローは止まらない。
fn try_open_browser(url: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return false;
        }
        std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = url;
        false
    }
}

/// `POST /auth/device` で GitHub アクセストークンを taida.dev のセッション
/// トークンに交換する。レスポンスは authCallback と同形の
/// `{token, username, expires_at}`。
fn exchange_device_token(github_token: &str) -> Result<String, String> {
    let empty_body = serde_json::json!({});
    let (status, body) =
        crate::community::api::api_post("/auth/device", &empty_body, Some(github_token))?;
    if status != 201 {
        return Err(format!(
            "taida.dev rejected the token exchange (HTTP {}): {}",
            status, body
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("Unexpected response from taida.dev: {}", e))?;
    parsed["token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Unexpected response from taida.dev: missing token field".to_string())
}

fn run_logout(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_auth_logout_help();
            return;
        }
        _ => {
            eprintln!("Usage: taida auth logout");
            std::process::exit(1);
        }
    }

    match load_token_checked() {
        Ok(Some(token)) => match delete_token() {
            Ok(()) => println!("Logged out (was {}).", token.username),
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
        Ok(None) => {
            println!("Not logged in.");
        }
        // The credentials file is there but unusable. Logging out is meant to
        // leave no credentials behind, so remove it and say so.
        Err(reason) => match delete_token() {
            Ok(()) => println!("Removed the unusable credentials file ({}).", reason),
            Err(e) => {
                eprintln!("Error: {}", reason);
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
    }
}

fn run_status(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_auth_status_help();
            return;
        }
        _ => {
            eprintln!("Usage: taida auth status");
            std::process::exit(1);
        }
    }

    match load_token_checked() {
        Ok(Some(token)) => {
            println!("Logged in as {}.", token.username);
            println!("Token created: {}", token.created_at);
            if token.community_token().is_none() {
                println!(
                    "taida.dev session: none (login predates the session exchange).\nRun `taida auth logout`, then `taida auth login` to refresh."
                );
            }
        }
        Ok(None) => {
            println!("Not logged in. Run `taida auth login` to authenticate.");
        }
        Err(reason) => {
            eprintln!("Error: {}", reason);
            eprintln!("Run `taida auth logout` to remove it, then `taida auth login`.");
            std::process::exit(1);
        }
    }
}
