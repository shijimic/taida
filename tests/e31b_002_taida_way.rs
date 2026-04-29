mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

#[test]
fn e31_way_default_full_runs_check_lint_verify() {
    let dir = unique_temp_dir("e31_way_full");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
badName <= 1
stdout(badName.toString())
"#,
    );

    let output = Command::new(taida_bin())
        .arg("way")
        .arg(&src)
        .output()
        .expect("run taida way <PATH>");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[E1804]"),
        "default full gate should fail in lint stage: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e31_way_help_contract_requires_path_for_full_gate() {
    let help = Command::new(taida_bin())
        .args(["way", "--help"])
        .output()
        .expect("run taida way --help");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(
        stdout.contains("taida way <PATH>") && !stdout.contains("taida way [<PATH>]"),
        "help must show full gate path as required: {}",
        stdout
    );

    let missing_path = Command::new(taida_bin())
        .args(["way", "--format", "text"])
        .output()
        .expect("run taida way --format text");
    assert_eq!(missing_path.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&missing_path.stderr).contains("Missing <PATH> argument."),
        "missing path should be explicit: {}",
        String::from_utf8_lossy(&missing_path.stderr)
    );
}

#[test]
fn e31_way_full_invalid_format_points_at_way_help() {
    let output = Command::new(taida_bin())
        .args(["way", "--format", "yaml", "examples/01_hello.td"])
        .output()
        .expect("run taida way --format yaml");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format 'yaml'") && stderr.contains("taida way --help"),
        "full way format errors should point at hub help: {}",
        stderr
    );
    assert!(
        !stderr.contains("taida way check --help"),
        "full way format errors must not point at check help: {}",
        stderr
    );
}

#[test]
fn e31_way_check_is_parse_and_type_only() {
    let dir = unique_temp_dir("e31_way_check_only");
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
        .args(["way", "check", "--format", "json"])
        .arg(&src)
        .output()
        .expect("run taida way check");

    assert!(
        output.status.success(),
        "way check should not run structural verify: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("check output should be json");
    assert_eq!(value["summary"]["errors"].as_u64(), Some(0));
    assert!(
        !value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["stage"] == "verify"),
        "way check must be parse+type only: {}",
        value
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e31_way_verify_exits_nonzero_for_error_in_any_format() {
    let dir = unique_temp_dir("e31_way_verify_exit");
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
        .args(["way", "verify", "--format", "text"])
        .arg(&src)
        .output()
        .expect("run taida way verify");

    assert_eq!(
        output.status.code(),
        Some(1),
        "way verify text should fail on ERROR findings: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e31_way_verify_does_not_accept_naming_convention() {
    let output = Command::new(taida_bin())
        .args([
            "way",
            "verify",
            "--check",
            "naming-convention",
            "examples/01_hello.td",
        ])
        .output()
        .expect("run taida way verify --check naming-convention");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown check 'naming-convention'"),
        "naming-convention should belong to way lint only: {}",
        stderr
    );
}

#[test]
fn e31_way_rejects_no_check_global_and_local_forms() {
    for args in [
        &["--no-check", "way", "examples/01_hello.td"][..],
        &["way", "--no-check", "examples/01_hello.td"][..],
    ] {
        let output = Command::new(taida_bin())
            .args(args)
            .output()
            .unwrap_or_else(|_| panic!("run taida {}", args.join(" ")));

        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--no-check is not allowed"),
            "taida {} should reject --no-check under way: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
