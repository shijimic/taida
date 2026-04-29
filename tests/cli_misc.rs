//! CLI tests for `taida way todo`, removed `inspect`, `taida doc`, and feature gate commands.
//!
//! Groups smaller command tests that do not warrant their own file.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

// ── taida way todo ──

#[test]
fn test_taida_todo_json_reports_ids_and_stats() {
    let dir = unique_temp_dir("taida_todo_cli");
    let src = r#"
a <= TODO[Int](id <= "TASK-1", task <= "first")
b <= TODO[Int](id <= "TASK-1", task <= "second", unm <= 2)
c <= TODO[Stub["shape TBD"]](id <= "TASK-2", task <= "third")
"#;
    write_file(&dir.join("main.td"), src);

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("todo")
        .arg("--format")
        .arg("json")
        .arg(&dir)
        .output()
        .expect("failed to run taida way todo");

    assert!(
        output.status.success(),
        "taida way todo should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("todo output should be valid JSON");
    assert_eq!(json["total"].as_u64(), Some(3));

    let by_id = json["byId"]
        .as_array()
        .expect("byId should be an array")
        .iter()
        .map(|v| {
            (
                v["id"].as_str().unwrap_or("<null>").to_string(),
                v["count"].as_u64().unwrap_or(0),
            )
        })
        .collect::<std::collections::HashMap<String, u64>>();

    assert_eq!(by_id.get("TASK-1"), Some(&2));
    assert_eq!(by_id.get("TASK-2"), Some(&1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_rc5d_todo_invalid_format_errors() {
    let output = Command::new(taida_bin())
        .args(["way", "todo", "--format", "csv", "."])
        .output()
        .expect("todo with invalid format");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format 'csv'"),
        "should mention unknown format, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_todo_format_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["way", "todo", "--format"])
        .output()
        .expect("todo --format with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --format"),
        "should mention missing value, got: {}",
        stderr
    );
}

// ── taida inspect removed in E31 ──

#[test]
fn test_e31_inspect_removed_with_e1700() {
    let output = Command::new(taida_bin())
        .args(["inspect", "--format", "yaml", "examples/01_hello.td"])
        .output()
        .expect("inspect removed command");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[E1700]") && stderr.contains("taida graph summary"),
        "should mention E1700 graph summary migration, got: {}",
        stderr
    );
}

// ── taida doc ──

#[test]
fn test_rc5e_doc_without_generate_errors() {
    let output = Command::new(taida_bin())
        .arg("doc")
        .output()
        .expect("doc without subcommand");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("taida doc --help"),
        "should suggest --help, got: {}",
        stderr
    );
}

#[test]
fn test_rc5e_doc_invalid_subcommand_errors() {
    let output = Command::new(taida_bin())
        .args(["doc", "build"])
        .output()
        .expect("doc with invalid subcommand");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("taida doc --help"),
        "should suggest --help, got: {}",
        stderr
    );
}

// ── Feature gates ──

#[test]
fn test_rc5h_feature_gate_messages_consistent() {
    // auth and community (without feature) should mention 'community' feature.
    // With the feature enabled, they produce usage errors.
    // publish is excluded: with the feature enabled it proceeds to auth/manifest checks.
    for cmd in &["auth", "community"] {
        let output = Command::new(taida_bin())
            .arg(cmd)
            .output()
            .unwrap_or_else(|_| panic!("should run taida {}", cmd));
        assert!(
            !output.status.success(),
            "taida {} with no args should fail",
            cmd
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Without community feature: "requires the 'community' feature"
        // With community feature: usage error mentioning the subcommand
        assert!(
            stderr.contains("community") || stderr.contains("--help") || stderr.contains("Usage"),
            "taida {} stderr should mention 'community' feature or usage, got: {}",
            cmd,
            stderr
        );
    }
}
