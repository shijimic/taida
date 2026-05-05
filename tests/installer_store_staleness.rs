#![allow(clippy::doc_overindented_list_items)]

//! C17-5: end-to-end test that `taida ingot install` detects a retag / delete+recreate
//! on the remote and auto-refreshes the cached store entry.
//!
//! Scenario:
//!   1. Mock server serves tarball v1 + tag SHA = "aaaa".
//!   2. `taida ingot install` populates `~/.taida/store/taida-lang/demo/a.1/` with
//!      `_meta.toml` recording commit_sha=aaaa.
//!   3. Mock server swaps its tarball to v2 + tag SHA = "bbbb" (retag).
//!   4. `taida ingot install` runs again -> E32B-016 source pin rejects the
//!      changed tarball and the previous store entry is restored.

#![cfg(unix)]

mod common;
mod mock;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use mock::{MockServer, TagState, make_tarball};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

#[test]
fn c17_5_retagged_source_tarball_rejected_by_source_pin() {
    let work = unique_temp_dir("c17_retag");
    let fake_home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // v1 tarball: self-manifest declares taida-lang/demo@a.1 and ships `demo_v1.td`.
    let tarball_v1 = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/demo\n" as &[u8]),
        ("main.td", b"stdout(\"v1\")\n"),
        ("demo_v1.td", b"// v1 marker\n"),
    ]);
    let integrity_v1 = format!("sha256:{}", taida::crypto::sha256_hex_bytes(&tarball_v1));

    // v2 tarball: same identity/version, different content (simulating a
    // retag after the maintainer fixed a bug in the facade).
    let tarball_v2 = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/demo\n" as &[u8]),
        ("main.td", b"stdout(\"v2\")\n"),
        ("demo_v2.td", b"// v2 marker\n"),
    ]);

    let state = Arc::new(Mutex::new(TagState {
        org: "taida-lang".to_string(),
        name: "demo".to_string(),
        version: "a.1".to_string(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        tarball: tarball_v1,
    }));

    let server = MockServer::start(state.clone());

    fs::write(
        project.join("packages.tdm"),
        format!(
            r#"[packages."taida-lang/demo"]
version = "a.1"
integrity = "{integrity_v1}"

<<<@a.1 test/consumer
"#
        ),
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").unwrap();

    // First install.
    let output1 = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .arg("--no-remote-check") // fallback; we'll toggle below to see both paths
        .current_dir(&project)
        .env("HOME", &fake_home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        // Ensure `gh` auth or network is never touched by other code paths.
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install");
    assert!(
        output1.status.success(),
        "first install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr)
    );

    // Verify first install placed `demo_v1.td` (from tarball v1) in the store.
    let store_dir = fake_home
        .join(".taida")
        .join("store")
        .join("taida-lang")
        .join("demo")
        .join("a.1");
    assert!(store_dir.join("demo_v1.td").exists(), "v1 content missing");
    assert!(
        !store_dir.join("demo_v2.td").exists(),
        "v2 content leaked in"
    );

    // Now swap the mock to v2 with a new SHA (simulate retag).
    {
        let mut s = state.lock().unwrap();
        s.commit_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        s.tarball = tarball_v2.clone();
    }

    // Second install WITHOUT --no-remote-check -> decision table detects the
    // tag SHA change, but E32B-016 rejects the changed source tarball because
    // packages.tdm still pins v1.
    let output2 = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .current_dir(&project)
        .env("HOME", &fake_home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install (2nd)");
    assert!(
        !output2.status.success(),
        "second install must reject retagged tarball: stdout={} stderr={}",
        String::from_utf8_lossy(&output2.stdout),
        String::from_utf8_lossy(&output2.stderr)
    );

    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(
        stderr2.contains("E32K3_SOURCE_INTEGRITY_MISMATCH"),
        "expected source pin mismatch on stderr, got:\n{}",
        stderr2
    );

    // v1 content must remain; the failed refresh must roll back.
    assert!(
        store_dir.join("demo_v1.td").exists(),
        "v1 content missing after failed refresh; store state=\n{:?}",
        fs::read_dir(&store_dir).map(|e| e.flatten().map(|d| d.file_name()).collect::<Vec<_>>())
    );
    assert!(
        !store_dir.join("demo_v2.td").exists(),
        "retagged v2 content must not survive a source-pin mismatch"
    );

    // Sidecar must not record the retagged commit SHA.
    let meta = fs::read_to_string(store_dir.join("_meta.toml")).expect("sidecar exists");
    assert!(
        !meta.contains("commit_sha = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\""),
        "sidecar must not update to the rejected retag, got:\n{}",
        meta
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}
