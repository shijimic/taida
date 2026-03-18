//! CLI `taida graph` tests.
//!
//! Covers: unknown option/format errors, missing path/value errors, recursive flags.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::taida_bin;
use std::process::Command;

#[test]
fn test_graph_unknown_option_fails() {
    let output = Command::new(taida_bin())
        .arg("graph")
        .arg("--type")
        .arg("bad-view")
        .arg("examples/04_functions.td")
        .output()
        .expect("failed to run taida graph with unknown option");

    assert!(
        !output.status.success(),
        "graph should fail for unknown option --type"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option '--type'"),
        "unexpected stderr: {}",
        stderr
    );
}

#[test]
fn test_graph_unknown_format_option_fails() {
    let output = Command::new(taida_bin())
        .arg("graph")
        .arg("--format")
        .arg("bad-format")
        .arg("examples/04_functions.td")
        .output()
        .expect("failed to run taida graph with unknown option");

    assert!(
        !output.status.success(),
        "graph should fail for unknown option --format"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option '--format'"),
        "unexpected stderr: {}",
        stderr
    );
}

#[test]
fn test_graph_recursive_with_unknown_type_flag_errors() {
    // --type is no longer a valid option, should error
    let output = Command::new(taida_bin())
        .args([
            "graph",
            "--recursive",
            "--type",
            "dataflow",
            "examples/01_hello.td",
        ])
        .output()
        .expect("graph recursive with unknown type flag");

    assert!(
        !output.status.success(),
        "--type is an unknown option and should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option '--type'"),
        "error should mention unknown option, got: {}",
        stderr
    );
}

// ── RC-5: graph missing output value / path ──

#[test]
fn test_rc5_graph_missing_output_value_errors() {
    let output = Command::new(taida_bin())
        .args(["graph", "-o"])
        .output()
        .expect("graph with -o but no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for -o"),
        "should mention missing value, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_graph_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("graph")
        .output()
        .expect("graph with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
}
