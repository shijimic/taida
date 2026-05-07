//! E32B-016: source package SHA-256 pins from `packages.tdm`.

#![cfg(unix)]

mod common;
mod mock;

use common::{taida_bin, unique_temp_dir};
use mock::{MockServer, TagState, make_tarball};
use std::fs;
use std::process::Command;
use std::sync::{Arc, Mutex};

fn pinned_manifest(integrity: &str) -> String {
    format!(
        r#"[packages."taida-lang/demo"]
version = "a.1"
integrity = "{integrity}"

<<<@a.1 test/consumer
"#
    )
}

#[test]
#[ignore = "Pre-empted by project-root marker tightening; needs rooted fixture"]
fn e32b_016_source_pin_mismatch_rejects_first_install() {
    let work = unique_temp_dir("e32b_016_mismatch");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/demo\n" as &[u8]),
        ("main.td", b"stdout(\"demo\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "taida-lang".to_string(),
        name: "demo".to_string(),
        version: "a.1".to_string(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        tarball,
    }));
    let server = MockServer::start(state);

    fs::write(
        project.join("packages.tdm"),
        pinned_manifest("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    )
    .expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(
        !output.status.success(),
        "install must fail on source pin mismatch"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E32K3_SOURCE_INTEGRITY_MISMATCH")
            && stderr.contains("taida-lang/demo@a.1"),
        "expected source integrity mismatch, got: {}",
        stderr
    );
    assert!(
        !home
            .join(".taida/store/taida-lang/demo/a.1/.taida_installed")
            .exists(),
        "mismatched source package must not be marked installed"
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}

#[test]
#[ignore = "Pre-empted by project-root marker tightening; needs rooted fixture"]
fn e32b_016_source_pin_match_allows_first_install() {
    let work = unique_temp_dir("e32b_016_match");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/demo\n" as &[u8]),
        ("main.td", b"stdout(\"demo\")\n"),
    ]);
    let integrity = format!("sha256:{}", taida::crypto::sha256_hex_bytes(&tarball));
    let state = Arc::new(Mutex::new(TagState {
        org: "taida-lang".to_string(),
        name: "demo".to_string(),
        version: "a.1".to_string(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        tarball,
    }));
    let server = MockServer::start(state);

    fs::write(project.join("packages.tdm"), pinned_manifest(&integrity)).expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(
        output.status.success(),
        "install should accept matching source pin; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let store_dir = home.join(".taida/store/taida-lang/demo/a.1");
    assert!(store_dir.join(".taida_installed").exists());
    let meta = fs::read_to_string(store_dir.join("_meta.toml")).expect("read sidecar");
    assert!(
        meta.contains(integrity.trim_start_matches("sha256:")),
        "sidecar should record fetched tarball hash, got: {}",
        meta
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}

#[test]
#[ignore = "Pre-empted by project-root marker tightening; needs rooted fixture"]
fn e32b_016_cached_source_pin_mismatch_rejects_reuse() {
    let work = unique_temp_dir("e32b_016_cached_mismatch");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/demo\n" as &[u8]),
        ("main.td", b"stdout(\"demo\")\n"),
    ]);
    let integrity = format!("sha256:{}", taida::crypto::sha256_hex_bytes(&tarball));
    let state = Arc::new(Mutex::new(TagState {
        org: "taida-lang".to_string(),
        name: "demo".to_string(),
        version: "a.1".to_string(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        tarball,
    }));
    let server = MockServer::start(state);

    fs::write(project.join("packages.tdm"), pinned_manifest(&integrity)).expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let first = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run first install");
    assert!(
        first.status.success(),
        "first install should populate cache; stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );

    fs::write(
        project.join("packages.tdm"),
        pinned_manifest("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    )
    .expect("rewrite manifest with wrong pin");
    let second = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run second install");

    assert!(
        !second.status.success(),
        "cached source pin mismatch must fail"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("E32K3_SOURCE_INTEGRITY_MISMATCH")
            && stderr.contains("cached source package taida-lang/demo@a.1"),
        "expected cached source integrity mismatch, got: {}",
        stderr
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn e32b_016_missing_source_pin_rejects_before_network() {
    let work = unique_temp_dir("e32b_016_missing_pin");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");
    fs::write(
        project.join("packages.tdm"),
        ">>> taida-lang/demo@a.1\n<<<@a.1 test/consumer\n",
    )
    .expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(!output.status.success(), "missing source pin must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E32K3_SOURCE_INTEGRITY_MISSING"),
        "expected missing pin error, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn e32b_016_non_official_owner_rejected() {
    let work = unique_temp_dir("e32b_016_non_official");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");
    fs::write(
        project.join("packages.tdm"),
        r#"[packages."alice/demo"]
version = "a.1"
integrity = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

<<<@a.1 test/consumer
"#,
    )
    .expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(
        !output.status.success(),
        "third-party source owner must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E32K3_NON_OFFICIAL_SOURCE_REJECTED"),
        "expected non-official owner error, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn e32b_016_base_url_override_rejected_without_mock_gate() {
    let work = unique_temp_dir("e32b_016_base_url");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");
    fs::write(
        project.join("packages.tdm"),
        pinned_manifest("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    )
    .expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", "http://127.0.0.1:9")
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(!output.status.success(), "base URL override must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E32K3_GITHUB_BASE_URL_CONFINED"),
        "expected confined base URL error, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}

/// A `packages.tdm` containing two `[packages."<id>"]` tables for the same
/// package id must be rejected by the manifest parser before any network or
/// staging step runs. A silent overwrite would let a hidden second pin
/// override the first one undetected during code review.
#[test]
fn e32b_043_duplicate_package_table_rejected() {
    let work = unique_temp_dir("e32b_043_duplicate_table");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");
    fs::write(
        project.join("packages.tdm"),
        r#"[packages."taida-lang/demo"]
version = "a.1"
integrity = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[packages."taida-lang/demo"]
version = "a.1"
integrity = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

<<<@a.1 test/consumer
"#,
    )
    .expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(
        !output.status.success(),
        "duplicate package table must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E32K3_PACKAGES_TDM_DUPLICATE_TABLE") && stderr.contains("taida-lang/demo"),
        "expected duplicate-table diagnostic, got: {}",
        stderr
    );
    assert!(
        !home
            .join(".taida/store/taida-lang/demo/a.1/.taida_installed")
            .exists(),
        "manifest with duplicate table must not produce a store entry"
    );

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn e32b_016_relaxed_signature_policy_rejected() {
    let work = unique_temp_dir("e32b_016_relaxed_sig");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&project).expect("create project");
    fs::write(
        project.join("packages.tdm"),
        pinned_manifest("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    )
    .expect("write manifest");
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").expect("write main");

    let output = Command::new(taida_bin())
        .args(["ingot", "install", "--no-remote-check"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_VERIFY_SIGNATURES", "best-effort")
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run install");

    assert!(
        !output.status.success(),
        "relaxed source signature policy must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E32K3_VERIFY_SIGNATURES_RELAXED"),
        "expected relaxed signature policy error, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}
