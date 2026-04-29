//! CLI `taida way verify` tests.
//!
//! Covers: jsonl output format, format/check validation, missing path errors.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

#[test]
fn test_verify_jsonl_outputs_findings_and_summary_and_sets_exit_code() {
    let dir = unique_temp_dir("taida_verify_jsonl");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
risky x =
  Error(message <= "boom").throw()
=> :Str
"#,
    );

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("verify")
        .arg("--format")
        .arg("jsonl")
        .arg(&src)
        .output()
        .expect("failed to run taida way verify --format jsonl");

    assert!(
        !output.status.success(),
        "verify jsonl should exit non-zero when ERROR findings exist"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // Verify at least one diagnostic line exists and each line has the expected JSON structure
    assert!(
        !lines.is_empty(),
        "jsonl output should contain at least one diagnostic line"
    );
    for line in &lines {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("each jsonl line should be valid json");
        assert_eq!(value["schema"], "taida.diagnostic.v1");
        assert_eq!(value["stream"], "verify");
        assert!(value.get("code").is_some());
        assert!(value.get("message").is_some());
        assert!(value.get("location").is_some());
        assert!(value.get("suggestion").is_some());
    }
    let summary: serde_json::Value = serde_json::from_str(lines.last().copied().unwrap_or("{}"))
        .expect("summary line should be valid json");
    assert_eq!(summary["kind"], "summary");
    assert!(
        summary["summary"]["errors"].as_u64().unwrap_or(0) >= 1,
        "summary should include at least one error"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── RC-5b: verify format/check validation ──

#[test]
fn test_rc5b_verify_invalid_format_errors() {
    let output = Command::new(taida_bin())
        .args(["way", "verify", "--format", "xml", "examples/01_hello.td"])
        .output()
        .expect("verify with invalid format");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format 'xml'"),
        "should mention unknown format, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_invalid_check_errors() {
    let output = Command::new(taida_bin())
        .args([
            "way",
            "verify",
            "--check",
            "nonexistent",
            "examples/01_hello.td",
        ])
        .output()
        .expect("verify with invalid check");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown check 'nonexistent'"),
        "should mention unknown check, got: {}",
        stderr
    );
    assert!(
        stderr.contains("error-coverage"),
        "should list available checks, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("verify")
        .output()
        .expect("verify with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_valid_format_accepted() {
    for fmt in &["text", "json", "jsonl", "sarif"] {
        let output = Command::new(taida_bin())
            .args(["way", "verify", "--format", fmt, "examples/01_hello.td"])
            .output()
            .unwrap_or_else(|_| panic!("way verify --format {} should run", fmt));
        assert!(
            output.status.success(),
            "way verify --format {} should succeed, stderr: {}",
            fmt,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_rc5b_verify_valid_check_accepted() {
    let output = Command::new(taida_bin())
        .args([
            "way",
            "verify",
            "--check",
            "error-coverage",
            "examples/01_hello.td",
        ])
        .output()
        .expect("verify with valid check");
    assert!(
        output.status.success(),
        "way verify --check error-coverage should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_rc5_verify_format_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["way", "verify", "--format"])
        .output()
        .expect("way verify --format with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --format"),
        "should mention missing value, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_verify_check_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["way", "verify", "--check"])
        .output()
        .expect("way verify --check with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --check"),
        "should mention missing value, got: {}",
        stderr
    );
}
