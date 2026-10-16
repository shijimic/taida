//! Regressions for `taida lsp`: what the server publishes and answers for a
//! document, and how it ends.
//!
//! The tests speak the protocol over the server's standard input and output.
#![cfg(feature = "lsp")]
mod common;

use common::{taida_bin, unique_temp_dir};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Lsp {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    pending: Vec<Value>,
    next_id: i64,
}

impl Lsp {
    fn start() -> Self {
        let home = unique_temp_dir("f65_lsp_home");
        let mut child = Command::new(taida_bin())
            .arg("lsp")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn taida lsp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let (tx, messages) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().ok();
                    }
                }
                let Some(length) = length else { return };
                let mut body = vec![0u8; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if let Ok(value) = serde_json::from_slice(&body)
                    && tx.send(value).is_err()
                {
                    return;
                }
            }
        });
        let mut lsp = Self {
            child,
            stdin,
            messages,
            pending: Vec::new(),
            next_id: 1,
        };
        lsp.request(
            "initialize",
            json!({"processId": null, "rootUri": null, "capabilities": {}}),
        );
        lsp.notify("initialized", json!({}));
        lsp
    }

    fn send_raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(bytes).expect("write");
        stdin.flush().expect("flush");
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        self.send_raw(format!("Content-Length: {}\r\n\r\n{}", body.len(), body).as_bytes());
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// The next message satisfying `wanted`, from those already read or those
    /// still to come.
    fn wait_for(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.pending.iter().position(&wanted) {
            return self.pending.remove(index);
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.messages.recv_timeout(left) {
                Ok(message) if wanted(&message) => return message,
                Ok(message) => self.pending.push(message),
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for a message"),
                Err(RecvTimeoutError::Disconnected) => panic!("the server closed its output"),
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.wait_for(|m| m["id"] == id && m.get("method").is_none())
    }

    fn open(&mut self, uri: &str, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "taida", "version": 1, "text": text}}),
        );
        let published = self.wait_for(|m| {
            m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri
        });
        published["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .clone()
    }

    fn hover(&mut self, uri: &str, line: u32, character: u32) -> String {
        let response = self.request(
            "textDocument/hover",
            json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}}),
        );
        response["result"]["contents"]["value"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    fn completion_labels(&mut self, uri: &str, line: u32, character: u32) -> Vec<String> {
        let response = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
                "context": {"triggerKind": 2, "triggerCharacter": "."},
            }),
        );
        let items = match &response["result"] {
            Value::Array(items) => items.clone(),
            other => other["items"].as_array().cloned().unwrap_or_default(),
        };
        items
            .iter()
            .map(|item| item["label"].as_str().unwrap_or("").to_string())
            .collect()
    }

    /// Wait for the process to end and return its exit code.
    fn wait_exit(&mut self) -> Option<i32> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn uri_of(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn severity(diagnostic: &Value) -> i64 {
    diagnostic["severity"].as_i64().unwrap_or(0)
}

// ── diagnostics ─────────────────────────────────────────────────────

#[test]
fn type_errors_are_errors_with_their_code() {
    let mut lsp = Lsp::start();
    let diagnostics = lsp.open(
        "file:///tmp/f65_lsp_types/a.td",
        "x: Int <= \"a\"\ny <= true + 1\nn <= 5\nn.hasValue()\n",
    );
    assert!(diagnostics.len() >= 3, "{diagnostics:?}");
    // Every type error stops the program, so every one is an Error; the
    // message's `[E####]` is also the diagnostic's code.
    for diagnostic in &diagnostics {
        assert_eq!(severity(diagnostic), 1, "{diagnostic}");
        let message = diagnostic["message"].as_str().unwrap_or("");
        if let Some(rest) = message.strip_prefix("[E") {
            let code = format!("E{}", &rest[..4]);
            assert_eq!(diagnostic["code"], code.as_str(), "{diagnostic}");
        }
    }
    let codes: Vec<&str> = diagnostics
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(
        codes.contains(&"E1619") && codes.contains(&"E1509"),
        "{codes:?}"
    );
}

#[test]
fn direct_non_tail_recursion_is_reported_like_way_check() {
    let mut lsp = Lsp::start();
    let diagnostics = lsp.open(
        "file:///tmp/f65_lsp_e0701/a.td",
        "fact n: Int =\n  | n < 2 |> 1\n  | _ |> n * fact(n - 1)\n=> :Int\nstdout(fact(5).toString())\n",
    );
    let e0701: Vec<&Value> = diagnostics
        .iter()
        .filter(|d| d["code"] == "E0701")
        .collect();
    assert_eq!(e0701.len(), 1, "{diagnostics:?}");
    assert_eq!(severity(e0701[0]), 1);
    assert_eq!(e0701[0]["range"]["start"]["line"], 2);
}

#[test]
fn imports_are_resolved_from_the_document_path() {
    let dir = unique_temp_dir("f65_lsp_imports");
    std::fs::write(
        dir.join("lib.td"),
        "add a: Int b: Int =\n  a + b\n=> :Int\nEnum => Color = :Red :Green\n<<< @(add, Color)\n",
    )
    .unwrap();
    let main = dir.join("main.td");
    let text = ">>> ./lib.td => @(add, nothingHere, Color)\nc <= Color:Red()\nstdout(add(\"a\", 2).toString())\n";
    std::fs::write(&main, text).unwrap();
    let mut lsp = Lsp::start();
    let diagnostics = lsp.open(&uri_of(&main), text);
    let codes: Vec<&str> = diagnostics
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(
        codes.contains(&"E1701"),
        "the missing symbol: {diagnostics:?}"
    );
    assert!(
        codes.contains(&"E1506"),
        "the argument type: {diagnostics:?}"
    );
    assert!(
        !codes.contains(&"E1608"),
        "an imported enum is known: {diagnostics:?}"
    );
}

#[test]
fn a_missing_module_is_reported() {
    let dir = unique_temp_dir("f65_lsp_missing_module");
    let main = dir.join("main.td");
    let text = ">>> ./nope.td => @(x)\nstdout(\"a\")\n";
    let mut lsp = Lsp::start();
    let diagnostics = lsp.open(&uri_of(&main), text);
    assert!(
        diagnostics.iter().any(|d| d["code"] == "E1701"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_byte_order_mark_does_not_shift_the_range() {
    let mut lsp = Lsp::start();
    let plain = lsp.open("file:///tmp/f65_lsp_bom/a.td", "x <= 10 / 2\n");
    let bom = lsp.open("file:///tmp/f65_lsp_bom/b.td", "\u{feff}x <= 10 / 2\n");
    let start = |d: &[Value]| d[0]["range"]["start"]["character"].as_i64().unwrap();
    let end = |d: &[Value]| d[0]["range"]["end"]["character"].as_i64().unwrap();
    assert_eq!((start(&plain), end(&plain)), (8, 9));
    // The editor counts the mark as a character of the line.
    assert_eq!((start(&bom), end(&bom)), (9, 10));
}

#[test]
fn a_very_deep_expression_does_not_take_the_server_down() {
    let mut lsp = Lsp::start();
    let terms = 6000;
    let text = format!(
        "x <= {}\nstdout(x.toString())\n",
        vec!["1"; terms].join(" + ")
    );
    let diagnostics = lsp.open("file:///tmp/f65_lsp_deep/a.td", &text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let hover = lsp.hover("file:///tmp/f65_lsp_deep/a.td", 1, 7);
    assert!(hover.contains("x: Int"), "{hover}");
}

// ── hover and completion ────────────────────────────────────────────

#[test]
fn hover_answers_for_the_token_under_the_cursor() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_hover/a.td";
    let text =
        "n <= 42\nstdout(n.toString())\np <= @(name <= \"a\")\nstdout(p.name)\ntotal <= n + 2\n";
    lsp.open(uri, text);
    // `n` (a variable) and `toString` (a method) on the same line.
    assert!(
        lsp.hover(uri, 1, 7).contains("n: Int"),
        "{}",
        lsp.hover(uri, 1, 7)
    );
    let method = lsp.hover(uri, 1, 9);
    assert!(
        method.contains("Int.toString") && method.contains("Receiver type: Int"),
        "{method}"
    );
    // `p` and `name` on the next.
    assert!(lsp.hover(uri, 3, 7).contains("p: @(name: Str)"));
    let field = lsp.hover(uri, 3, 9);
    assert!(field.contains(".name: Str"), "{field}");
    // Punctuation and the callee's parenthesis are not the variable.
    let paren = lsp.hover(uri, 1, 6);
    assert!(!paren.contains("n: Int"), "{paren}");
}

#[test]
fn dot_completion_offers_the_methods_of_the_receiver() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_complete/a.td";
    // The document is unfinished: each line ends in a dot.
    let text = "s <= \"abc\"\nxs <= @[1, 2]\nn <= 5\ns.\nxs.\nn.\n";
    lsp.open(uri, text);
    let on_str = lsp.completion_labels(uri, 3, 2);
    assert!(on_str.contains(&"length".to_string()), "{on_str:?}");
    assert!(on_str.contains(&"startsWith".to_string()), "{on_str:?}");
    let on_list = lsp.completion_labels(uri, 4, 3);
    assert!(on_list.contains(&"first".to_string()), "{on_list:?}");
    assert!(on_list.contains(&"isEmpty".to_string()), "{on_list:?}");
    let on_int = lsp.completion_labels(uri, 5, 2);
    assert!(on_int.contains(&"isPositive".to_string()), "{on_int:?}");
    assert!(
        !on_int.contains(&"hasValue".to_string()),
        "the checker rejects hasValue on an Int: {on_int:?}"
    );
}

#[test]
fn dot_completion_after_a_field_uses_the_type_of_the_field() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_complete_field/a.td";
    // A top-level `name` of another type must not be taken for the field.
    let text = "p <= @(name <= \"x\", age <= 3)\nname <= 42\nstdout(p.name.)\n";
    lsp.open(uri, text);
    let labels = lsp.completion_labels(uri, 2, 14);
    assert!(labels.contains(&"length".to_string()), "{labels:?}");
    assert!(!labels.contains(&"isPositive".to_string()), "{labels:?}");
}

#[test]
fn dot_completion_still_offers_to_string_when_the_type_is_unknown() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_complete_unknown/a.td";
    let text = "xs <= @[1, 2]\nys <= xs.map(_ x = x * 2)\nys.\n";
    lsp.open(uri, text);
    let labels = lsp.completion_labels(uri, 2, 3);
    assert!(labels.contains(&"toString".to_string()), "{labels:?}");
}

#[test]
fn hover_on_a_local_binding_shows_its_own_type() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_hover_local/a.td";
    // The local `x` shadows the top-level one.
    let text = "x <= \"top\"\nf n: Int =\n  x <= n + 1\n  x\n=> :Int\nstdout(f(1).toString())\n";
    lsp.open(uri, text);
    let hover = lsp.hover(uri, 2, 2);
    assert!(hover.contains("x: Int"), "{hover}");
}

// ── how the server ends ─────────────────────────────────────────────

fn frame(message: &Value) -> Vec<u8> {
    frame_of(&message.to_string())
}

fn frame_of(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

#[test]
fn exit_after_shutdown_ends_the_process_with_success() {
    let mut lsp = Lsp::start();
    // Both messages in one write: the client does not wait for the answer.
    let mut bytes = frame(&json!({"jsonrpc": "2.0", "id": 900, "method": "shutdown"}));
    bytes.extend(frame(&json!({"jsonrpc": "2.0", "method": "exit"})));
    lsp.send_raw(&bytes);
    assert_eq!(
        lsp.wait_exit(),
        Some(0),
        "the server ends without waiting for the pipe to close"
    );
}

#[test]
fn exit_without_shutdown_ends_the_process_with_failure() {
    let mut lsp = Lsp::start();
    lsp.notify("exit", json!(null));
    assert_eq!(lsp.wait_exit(), Some(1));
}

#[test]
fn undecodable_messages_get_an_error_response_and_the_server_carries_on() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_garbage/a.td";
    lsp.open(uri, "n <= 42\n");

    // Text that is not JSON.
    lsp.send_raw(&frame_of("{not json"));
    let response = lsp.wait_for(|m| m["error"]["code"] == -32700);
    assert_eq!(response["id"], Value::Null, "{response}");

    // JSON that is not a request, under its id.
    lsp.send_raw(&frame_of(r#"{"id": 5, "method": "textDocument/hover"}"#));
    let response = lsp.wait_for(|m| m["error"]["code"] == -32600 && m["id"] == 5);
    assert_eq!(response["error"]["message"], "Invalid request");

    // An id no request can have, and a JSON-RPC batch, which LSP has no use for.
    lsp.send_raw(&frame_of(r#"{"jsonrpc":"2.0","id":1.5,"method":"x"}"#));
    lsp.wait_for(|m| m["error"]["code"] == -32600 && m["id"].is_null());
    lsp.send_raw(&frame_of(r#"[{"jsonrpc":"2.0","method":"x"}]"#));
    lsp.wait_for(|m| m["error"]["code"] == -32600 && m["id"].is_null());

    // A header without a usable length; the next frame is still read.
    lsp.send_raw(b"Content-Length: nope\r\n\r\n");
    lsp.wait_for(|m| m["error"]["code"] == -32700);

    // The server still answers.
    let hover = lsp.hover(uri, 0, 0);
    assert!(hover.contains("n: Int"), "{hover}");
    assert!(
        lsp.child.try_wait().unwrap().is_none(),
        "the server is still running"
    );
}

#[test]
fn a_frame_cut_at_the_wrong_length_does_not_wedge_the_server() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_wedge/a.td";
    lsp.open(uri, "n <= 42\n");

    // One byte more than the header says: the extra byte lands in front of
    // the next header.
    let body = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    lsp.send_raw(format!("Content-Length: {}\r\n\r\n{}", body.len() - 1, body).as_bytes());
    lsp.wait_for(|m| m["error"]["code"] == -32700);
    // A header naming no usable length, its body coming in a read of its own.
    lsp.send_raw(b"Content-Length: nope\r\n\r\n");
    lsp.wait_for(|m| m["error"]["code"] == -32700);
    lsp.send_raw(br#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    std::thread::sleep(Duration::from_millis(100));

    // Frames sent afterwards, one at a time, are all answered.
    for _ in 0..3 {
        let hover = lsp.hover(uri, 0, 0);
        assert!(hover.contains("n: Int"), "{hover}");
    }
}

#[test]
fn headers_ended_by_bare_line_feeds_are_read() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_lf/a.td";
    // Larger than the space kept for a header block, and framed with bare
    // line feeds only.
    let text = format!("n <= 42\n// {}\n", "x".repeat(20_000));
    let body = json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "taida", "version": 1, "text": text}},
    })
    .to_string();
    lsp.send_raw(format!("Content-Length: {}\n\n{}", body.len(), body).as_bytes());
    lsp.wait_for(|m| m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri);
    let hover = lsp.hover(uri, 0, 0);
    assert!(hover.contains("n: Int"), "{hover}");
}

#[test]
fn messages_sent_behind_exit_do_not_keep_the_server_running() {
    let mut behind_exit = frame(&json!({"jsonrpc": "2.0", "method": "exit"}));
    behind_exit.extend(frame(
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    ));
    let mut lsp = Lsp::start();
    lsp.send_raw(&behind_exit);
    assert_eq!(
        lsp.wait_exit(),
        Some(1),
        "exit in the same write as a message"
    );

    // `exit` with an id is still `exit`.
    let mut lsp = Lsp::start();
    let mut bytes = frame(&json!({"jsonrpc": "2.0", "id": 800, "method": "shutdown"}));
    bytes.extend(frame(
        &json!({"jsonrpc": "2.0", "id": 801, "method": "exit"}),
    ));
    bytes.extend(frame(
        &json!({"jsonrpc": "2.0", "id": 802, "method": "textDocument/hover", "params": {}}),
    ));
    lsp.send_raw(&bytes);
    assert_eq!(lsp.wait_exit(), Some(0));
}

#[test]
fn a_header_line_ended_by_a_bare_line_feed_before_a_crlf_blank_line_is_read() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_mixed/a.td";
    lsp.open(uri, "n <= 42\n");
    let body = json!({
        "jsonrpc": "2.0",
        "id": 50,
        "method": "textDocument/hover",
        "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}},
    })
    .to_string();
    lsp.send_raw(format!("Content-Length: {}\n\r\n{}", body.len(), body).as_bytes());
    let response = lsp.wait_for(|m| m["id"] == 50);
    assert!(
        response["result"]["contents"]["value"]
            .as_str()
            .unwrap_or("")
            .contains("n: Int"),
        "{response}"
    );
}

#[test]
fn one_bad_header_gets_one_error_however_long_the_junk_after_it() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_junk/a.td";
    lsp.open(uri, "n <= 42\n");
    lsp.send_raw(b"Content-Length: nope\r\n\r\n");
    // A body of the bad frame that comes in many reads.
    for _ in 0..12 {
        lsp.send_raw(&[b'x'; 16 * 1024]);
    }
    let hover = lsp.hover(uri, 0, 0);
    assert!(hover.contains("n: Int"), "{hover}");
    let errors = lsp
        .pending
        .iter()
        .filter(|m| m["error"]["code"] == -32700)
        .count();
    assert_eq!(errors, 1, "{:?}", lsp.pending);
}

#[test]
fn a_flood_of_bad_frames_leaves_the_server_answering() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_flood/a.td";
    lsp.open(uri, "n <= 42\n");
    let flood = b"Content-Length: nope\r\n\r\n".repeat(2000);
    lsp.send_raw(&flood);
    let hover = lsp.hover(uri, 0, 0);
    assert!(hover.contains("n: Int"), "{hover}");
}

#[test]
fn hover_does_not_borrow_the_type_of_a_top_level_variable_for_an_unknown_local() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_hover_unknown/a.td";
    // The lambda's parameter `e` is not the top-level `e`.
    let text = "e <= 42\nf <= _ e = e\n";
    lsp.open(uri, text);
    let hover = lsp.hover(uri, 1, 13);
    assert!(!hover.contains("e: Int"), "{hover}");
}

#[test]
fn hover_on_the_target_of_an_annotated_unmold_shows_the_annotation() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_hover_unmold/a.td";
    let text =
        "y <= \"s\"\nf n: Int =\n  y: Int <=< Div[n, 2]()\n  y\n=> :Int\nstdout(f(4).toString())\n";
    lsp.open(uri, text);
    let hover = lsp.hover(uri, 2, 2);
    assert!(hover.contains("y: Int"), "{hover}");
}

#[test]
fn dot_completion_on_a_local_of_unknown_type_does_not_use_a_variable_of_the_same_name() {
    let mut lsp = Lsp::start();
    let uri = "file:///tmp/f65_lsp_complete_local/a.td";
    let text = "e <= 42\nf <= _ e = e.\n";
    lsp.open(uri, text);
    let labels = lsp.completion_labels(uri, 1, 13);
    assert!(labels.contains(&"toString".to_string()), "{labels:?}");
    assert!(!labels.contains(&"isPositive".to_string()), "{labels:?}");
}

#[test]
fn closing_the_input_ends_the_server_with_success() {
    let mut lsp = Lsp::start();
    drop(lsp.stdin.take());
    assert_eq!(lsp.wait_exit(), Some(0));
}
