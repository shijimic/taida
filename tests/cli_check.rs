//! CLI `taida way check` tests.
//!
//! Covers: `--format json` output schema, error codes E1501-E1504, file vs directory
//! consistency, regression tests, and quality example validation.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn test_check_json_outputs_machine_readable_summary() {
    let dir = unique_temp_dir("taida_check_json");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
x <= 1
stdout(x.toString())
"#,
    );

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("failed to run taida way check --format json");

    assert!(
        output.status.success(),
        "way check --format json should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("way check --format json output should be valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    assert!(value["diagnostics"].is_array());
    assert_eq!(value["summary"]["files"].as_u64(), Some(1));
    assert_eq!(value["summary"]["errors"].as_u64(), Some(0));

    let _ = fs::remove_dir_all(&dir);
}

// ── C-8a: taida way check --format json emits E1501/E1502/E1503/E1504 ──

#[test]
fn test_check_json_e1501_same_scope_redefinition() {
    let dir = unique_temp_dir("taida_check_e1501");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\n");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("failed to run taida way check --format json");

    assert!(
        !output.status.success(),
        "way check --format json should fail for E1501"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("way check --format json output should be valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1501"),
        "Expected E1501 in diagnostics, got: {:?}",
        diags
    );
    assert_eq!(value["summary"]["errors"].as_u64(), Some(1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_e1502_old_placeholder_partial_application() {
    let dir = unique_temp_dir("taida_check_e1502");
    let src = dir.join("main.td");
    write_file(&src, "add x y = x\n=> :Int\nresult <= add(5, _)\n");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("failed to run taida way check --format json");

    assert!(
        !output.status.success(),
        "way check --format json should fail for E1502"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("way check --format json output should be valid json");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1502"),
        "Expected E1502 in diagnostics, got: {:?}",
        diags
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_e1503_typedef_partial_application() {
    let dir = unique_temp_dir("taida_check_e1503");
    let src = dir.join("main.td");
    write_file(&src, "Point => @(x, y)\np <= Point(1, )\n");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("failed to run taida way check --format json");

    assert!(
        !output.status.success(),
        "way check --format json should fail for E1503"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("way check --format json output should be valid json");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1503"),
        "Expected E1503 in diagnostics, got: {:?}",
        diags
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_e1504_mold_placeholder_outside_pipeline() {
    let dir = unique_temp_dir("taida_check_e1504");
    let src = dir.join("main.td");
    write_file(&src, "x <= Str[_]()\n");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("failed to run taida way check --format json");

    assert!(
        !output.status.success(),
        "way check --format json should fail for E1504"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("way check --format json output should be valid json");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1504"),
        "Expected E1504 in diagnostics, got: {:?}",
        diags
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── C-8b: file/dir produce same format, summary, exit code ──

#[test]
fn test_check_json_file_vs_dir_format_consistency() {
    let dir = unique_temp_dir("taida_check_file_dir");
    let single_file = dir.join("single.td");
    let sub_dir = dir.join("sub");
    fs::create_dir_all(&sub_dir).expect("create sub dir");
    write_file(&single_file, "x <= 1\nx <= 2\n");
    write_file(&sub_dir.join("a.td"), "y <= 1\ny <= 2\n");

    // File input
    let file_out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&single_file)
        .output()
        .expect("way check --format json file");

    // Dir input
    let dir_out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&sub_dir)
        .output()
        .expect("way check --format json dir");

    // Both should fail with exit code != 0
    assert!(!file_out.status.success(), "file check should fail");
    assert!(!dir_out.status.success(), "dir check should fail");

    // Both should produce valid JSON with same schema
    let file_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&file_out.stdout))
            .expect("file output should be valid json");
    let dir_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&dir_out.stdout))
            .expect("dir output should be valid json");

    assert_eq!(file_json["schema"], "taida.check.v1");
    assert_eq!(dir_json["schema"], "taida.check.v1");
    assert!(file_json["diagnostics"].is_array());
    assert!(dir_json["diagnostics"].is_array());
    assert!(file_json["summary"].is_object());
    assert!(dir_json["summary"].is_object());

    // Both JSON outputs should have the same field set in diagnostics
    let file_diag = &file_json["diagnostics"][0];
    let dir_diag = &dir_json["diagnostics"][0];
    for field in &[
        "stage",
        "severity",
        "code",
        "message",
        "location",
        "suggestion",
    ] {
        assert!(
            file_diag.get(*field).is_some(),
            "file diagnostic missing field: {}",
            field
        );
        assert!(
            dir_diag.get(*field).is_some(),
            "dir diagnostic missing field: {}",
            field
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_file_vs_dir_success_exit_code() {
    let dir = unique_temp_dir("taida_check_success_exit");
    let single_file = dir.join("ok.td");
    let sub_dir = dir.join("sub");
    fs::create_dir_all(&sub_dir).expect("create sub dir");
    write_file(&single_file, "x <= 1\nstdout(x.toString())\n");
    write_file(&sub_dir.join("ok.td"), "y <= 2\nstdout(y.toString())\n");

    let file_out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&single_file)
        .output()
        .expect("check file");

    let dir_out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&sub_dir)
        .output()
        .expect("check dir");

    assert!(file_out.status.success(), "file check should succeed");
    assert!(dir_out.status.success(), "dir check should succeed");

    let _ = fs::remove_dir_all(&dir);
}

// ── C-11c: taida way check --format json regression tests ──

#[test]
fn test_check_json_regression_clean_file() {
    // C-11c: Clean file produces no diagnostics
    let dir = unique_temp_dir("taida_c11c_clean");
    let src = dir.join("main.td");
    write_file(&src, "x <= 42\nstdout(x.toString())\n");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("way check --format json");

    assert!(
        output.status.success(),
        "way check --format json clean file should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    assert_eq!(value["summary"]["errors"].as_u64(), Some(0));
    assert!(
        value["diagnostics"]
            .as_array()
            .expect("diagnostics should be a JSON array")
            .is_empty()
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_regression_multiple_errors() {
    // C-11c: Multiple errors produce correct count
    let dir = unique_temp_dir("taida_c11c_multi");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\ny <= 3\ny <= 4\n");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&src)
        .output()
        .expect("way check --format json");

    assert!(
        !output.status.success(),
        "way check --format json should fail with errors"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.len() >= 2,
        "Expected at least 2 diagnostics, got {}",
        diags.len()
    );
    assert!(
        diags.iter().all(|d| d["code"] == "E1501"),
        "All diagnostics should be E1501"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── C-11d: examples/quality/ checker regression ──

#[test]
fn test_quality_e2d_mold_partial_direct_is_rejected() {
    // C-11d: e2d_mold_partial_direct.td should be rejected by checker (E1504)
    let path = "examples/quality/e2d_mold_partial_direct.td";
    if !Path::new(path).exists() {
        return; // Skip if quality examples not present
    }
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(path)
        .output()
        .expect("check quality file");

    assert!(
        !output.status.success(),
        "e2d should be rejected by checker"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1504"),
        "Expected E1504 in e2d diagnostics, got: {:?}",
        diags
    );
}

#[test]
fn test_quality_e2f_duplicate_variable_is_rejected() {
    // C-11d: e2f_duplicate_variable_defs.td should be rejected by checker (E1501)
    let path = "examples/quality/e2f_duplicate_variable_defs.td";
    if !Path::new(path).exists() {
        return;
    }
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(path)
        .output()
        .expect("check quality file");

    assert!(
        !output.status.success(),
        "e2f should be rejected by checker"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1501"),
        "Expected E1501 in e2f diagnostics, got: {:?}",
        diags
    );
}

#[test]
fn test_quality_e3a_name_collision_passes() {
    // C-11d: e3a_name_collision_check.td should PASS (demonstrates valid shadowing)
    let path = "examples/quality/e3a_name_collision_check.td";
    if !Path::new(path).exists() {
        return;
    }
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(path)
        .output()
        .expect("check quality file");

    assert!(
        output.status.success(),
        "e3a should pass checker (valid shadowing), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── RC-5a: check missing path ──

#[test]
fn test_rc5a_check_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .output()
        .expect("check with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
    assert!(
        stderr.contains("taida way check --help"),
        "should suggest --help, got: {}",
        stderr
    );
}
