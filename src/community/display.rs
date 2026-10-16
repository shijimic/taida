use serde_json::Value;

/// 投稿の一覧を人間が読みやすい形式で表示する。
/// サーバーは `{ "posts": [...] }` を返す。
pub fn display_posts(json: &str) {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(obj)) => {
            if let Some(Value::Array(posts)) = obj.get("posts") {
                if posts.is_empty() {
                    println!("No posts yet.");
                    return;
                }
                for post in posts {
                    print_post(post);
                }
            } else {
                println!("{}", raw_text(json));
            }
        }
        Ok(_) => {
            println!("{}", raw_text(json));
        }
        Err(_) => {
            println!("{}", raw_text(json));
        }
    }
}

/// 投稿作成後の単一オブジェクトを表示する。
pub fn display_post_created(json: &str) {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(obj)) => {
            let author = obj
                .get("author")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let tags = format_tags_from_map(&obj);
            let (author, content) = (single_line(author), multi_line(content));
            if tags.is_empty() {
                println!("Posted as @{}: {}", author, content);
            } else {
                println!("Posted as @{}: {} {}", author, content, tags);
            }
        }
        Ok(_) => {
            println!("{}", raw_text(json));
        }
        Err(_) => {
            println!("{}", raw_text(json));
        }
    }
}

/// メッセージの一覧を人間が読みやすい形式で表示する。
/// サーバーは `{ "messages": [...] }` を返す。
pub fn display_messages(json: &str) {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(obj)) => {
            if let Some(Value::Array(messages)) = obj.get("messages") {
                if messages.is_empty() {
                    println!("No messages.");
                    return;
                }
                for msg in messages {
                    let from = msg
                        .get("from")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let created = msg.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
                    println!(
                        "[{}] from @{}: {}",
                        single_line(created),
                        single_line(from),
                        multi_line(content)
                    );
                }
            } else {
                println!("{}", raw_text(json));
            }
        }
        Ok(_) => {
            println!("{}", raw_text(json));
        }
        Err(_) => {
            println!("{}", raw_text(json));
        }
    }
}

/// メッセージ送信後の単一オブジェクトを表示する。
pub fn display_message_sent(json: &str) {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(obj)) => {
            let to = obj.get("to").and_then(|v| v.as_str()).unwrap_or("unknown");
            let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");
            println!(
                "Message sent to @{}: {}",
                single_line(to),
                multi_line(content)
            );
        }
        Ok(_) => {
            println!("{}", raw_text(json));
        }
        Err(_) => {
            println!("{}", raw_text(json));
        }
    }
}

/// 著者プロフィールを人間が読みやすい形式で表示する。
pub fn display_author(json: &str) {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(obj)) => {
            let name = obj
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!("@{}", single_line(name));
            if let Some(bio) = obj.get("bio").and_then(|v| v.as_str())
                && !bio.is_empty()
            {
                println!("  {}", multi_line(bio));
            }
            if let Some(packages) = obj.get("packages").and_then(|v| v.as_array())
                && !packages.is_empty()
            {
                println!("  Packages:");
                for pkg in packages {
                    if let Some(name) = pkg.as_str() {
                        println!("    - {}", single_line(name));
                    } else if let Some(name) = pkg.get("name").and_then(|v| v.as_str()) {
                        println!("    - {}", single_line(name));
                    }
                }
            }
        }
        Ok(_) => {
            println!("{}", raw_text(json));
        }
        Err(_) => {
            println!("{}", raw_text(json));
        }
    }
}

fn print_post(post: &Value) {
    let author = post
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let content = post.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let created = post
        .get("created_at")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tags = format_tags(post);
    let (created, author, content) = (
        single_line(created),
        single_line(author),
        multi_line(content),
    );
    if tags.is_empty() {
        println!("[{}] @{}: {}", created, author, content);
    } else {
        println!("[{}] @{}: {} {}", created, author, content, tags);
    }
}

fn format_tags(obj: &Value) -> String {
    match obj.get("tags").and_then(|v| v.as_array()) {
        Some(tags) if !tags.is_empty() => {
            let tag_strs: Vec<String> = tags
                .iter()
                .filter_map(|t| t.as_str())
                .map(single_line)
                .collect();
            format!("[{}]", tag_strs.join(", "))
        }
        _ => String::new(),
    }
}

fn format_tags_from_map(obj: &serde_json::Map<String, Value>) -> String {
    match obj.get("tags").and_then(|v| v.as_array()) {
        Some(tags) if !tags.is_empty() => {
            let tag_strs: Vec<String> = tags
                .iter()
                .filter_map(|t| t.as_str())
                .map(single_line)
                .collect();
            format!("[{}]", tag_strs.join(", "))
        }
        _ => String::new(),
    }
}

// Everything shown here was written by other users of the server (posts,
// messages, names, bios, tags). Control characters in it are escaped
// before it reaches the terminal, so it cannot clear the screen, move the
// cursor, retitle the window or write to the clipboard (ESC / OSC
// sequences), and it cannot forge extra entries with embedded newlines.

/// A field shown on one line: every control character is escaped,
/// newlines included.
fn single_line(text: &str) -> String {
    escape_controls(text, false)
}

/// Free text (post and message bodies, bios): newlines are kept, and each
/// continuation line is indented so it cannot pass for a new entry.
fn multi_line(text: &str) -> String {
    escape_controls(text, true).replace('\n', "\n  ")
}

/// A server response shown as is because it was not the expected shape:
/// line breaks and tabs are kept, other control characters are escaped.
fn raw_text(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.split('\t')
                .map(|part| escape_controls(part, false))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_controls(text: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' if keep_newlines => out.push('\n'),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // C0, DEL and C1 controls, plus the bidirectional overrides and
            // isolates that reorder how the rest of the line is displayed.
            c if c.is_control()
                || ('\u{202a}'..='\u{202e}').contains(&c)
                || ('\u{2066}'..='\u{2069}').contains(&c) =>
            {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_characters_are_escaped() {
        let hostile = "hi\u{1b}]0;PWNED\u{7}\u{1b}[2J\u{9b}31m\u{202e}x";
        let shown = single_line(hostile);
        assert!(!shown.chars().any(|c| c.is_control()), "{shown:?}");
        assert_eq!(
            shown,
            "hi\\u{1b}]0;PWNED\\u{7}\\u{1b}[2J\\u{9b}31m\\u{202e}x"
        );
        assert_eq!(single_line("a\nb\r\tc"), "a\\nb\\r\\tc");
        assert_eq!(
            multi_line("line1\n[2026] @admin: fake"),
            "line1\n  [2026] @admin: fake"
        );
        assert_eq!(
            raw_text("{\n\t\"a\": \"\u{1b}[2J\"\n}"),
            "{\n\t\"a\": \"\\u{1b}[2J\"\n}"
        );
        assert_eq!(single_line("日本語 ok"), "日本語 ok");
    }

    #[test]
    fn test_display_posts_empty() {
        display_posts(r#"{"posts":[]}"#);
    }

    #[test]
    fn test_display_posts_wrapped() {
        let json = r#"{"posts":[{"author":"alice","content":"hello","tags":["parser"],"created_at":"2026-03-06"}]}"#;
        display_posts(json);
    }

    #[test]
    fn test_display_post_created() {
        let json =
            r#"{"author":"alice","content":"hello","tags":["mold"],"created_at":"2026-03-06"}"#;
        display_post_created(json);
    }

    #[test]
    fn test_display_messages_wrapped() {
        let json = r#"{"messages":[{"from":"bob","content":"hi","created_at":"2026-03-06"}]}"#;
        display_messages(json);
    }

    #[test]
    fn test_display_messages_empty() {
        display_messages(r#"{"messages":[]}"#);
    }

    #[test]
    fn test_display_message_sent() {
        let json = r#"{"to":"bob","content":"hi","created_at":"2026-03-06"}"#;
        display_message_sent(json);
    }

    #[test]
    fn test_display_author_object() {
        let json = r#"{"username":"alice","bio":"Taida dev","packages":["mylib"]}"#;
        display_author(json);
    }
}
