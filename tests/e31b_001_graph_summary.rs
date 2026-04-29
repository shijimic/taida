mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn sample_source() -> PathBuf {
    let dir = unique_temp_dir("e31_graph_summary");
    let file = dir.join("main.td");
    write_file(&file, "add x y =\n  x + y\nx <= 42\n");
    file
}

#[test]
fn e31_graph_summary_default_outputs_structural_summary_only() {
    let file = sample_source();
    let output = Command::new(taida_bin())
        .args(["graph", "summary"])
        .arg(&file)
        .output()
        .expect("run taida graph summary");

    assert!(
        output.status.success(),
        "graph summary should succeed, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary: Value = serde_json::from_str(&stdout).expect("summary should be JSON");
    assert_eq!(summary["version"], "1.0");
    assert!(summary["stats"]["functions"].as_u64().unwrap_or(0) >= 1);
    assert!(!stdout.contains("verification"));
    assert!(!stdout.contains("Taida Inspect"));
    assert!(!stdout.contains("taida-verify"));
}

#[test]
fn e31_graph_summary_json_outputs_summary_without_verify_embedding() {
    let file = sample_source();
    let output = Command::new(taida_bin())
        .args(["graph", "summary", "--format", "json"])
        .arg(&file)
        .output()
        .expect("run taida graph summary --format json");

    assert!(
        output.status.success(),
        "graph summary json should succeed, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary: Value = serde_json::from_str(&stdout).expect("summary should be JSON");
    assert_eq!(summary["version"], "1.0");
    assert!(summary.get("verification").is_none());
    assert!(summary.get("runs").is_none());
}

#[test]
fn e31_graph_summary_sarif_wraps_summary_without_verify_results() {
    let file = sample_source();
    let output = Command::new(taida_bin())
        .args(["graph", "summary", "--format", "sarif"])
        .arg(&file)
        .output()
        .expect("run taida graph summary --format sarif");

    assert!(
        output.status.success(),
        "graph summary sarif should succeed, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let sarif: Value = serde_json::from_str(&stdout).expect("sarif should be JSON");
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(
        sarif["runs"][0]["tool"]["driver"]["name"],
        "taida-graph-summary"
    );
    assert!(
        sarif["runs"][0]["results"]
            .as_array()
            .expect("results array")
            .is_empty()
    );
    assert_eq!(sarif["runs"][0]["properties"]["summary"]["version"], "1.0");
    assert!(!stdout.contains("taida-verify"));
    assert!(!stdout.contains("verification"));
}

#[test]
fn e31_graph_summary_rejects_old_type_option() {
    let file = sample_source();
    let output = Command::new(taida_bin())
        .args(["graph", "summary", "--type", "dataflow"])
        .arg(&file)
        .output()
        .expect("run taida graph summary --type");

    assert!(!output.status.success(), "--type should be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option for graph summary: --type"),
        "unexpected stderr: {}",
        stderr
    );
}
