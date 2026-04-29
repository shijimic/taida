#![allow(clippy::doc_overindented_list_items)]

//! C17-5: end-to-end test that `taida ingot install` detects a retag / delete+recreate
//! on the remote and auto-refreshes the cached store entry.
//!
//! Scenario:
//!   1. Mock server serves tarball v1 + tag SHA = "aaaa".
//!   2. `taida ingot install` populates `~/.taida/store/alice/demo/a.1/` with
//!      `_meta.toml` recording commit_sha=aaaa.
//!   3. Mock server swaps its tarball to v2 + tag SHA = "bbbb" (retag).
//!   4. `taida ingot install` runs again -> store entry is re-extracted; the
//!      new sidecar records commit_sha=bbbb; exit code 0; the new
//!      tarball content is now visible in the store.

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
fn c17_5_install_autorefreshes_when_tag_retags() {
    let work = unique_temp_dir("c17_retag");
    let fake_home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // v1 tarball: self-manifest declares alice/demo@a.1 and ships `demo_v1.td`.
    let tarball_v1 = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/demo\n" as &[u8]),
        ("main.td", b"stdout(\"v1\")\n"),
        ("demo_v1.td", b"// v1 marker\n"),
    ]);

    // v2 tarball: same identity/version, different content (simulating a
    // retag after the maintainer fixed a bug in the facade).
    let tarball_v2 = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/demo\n" as &[u8]),
        ("main.td", b"stdout(\"v2\")\n"),
        ("demo_v2.td", b"// v2 marker\n"),
    ]);

    let state = Arc::new(Mutex::new(TagState {
        org: "alice".to_string(),
        name: "demo".to_string(),
        version: "a.1".to_string(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        tarball: tarball_v1,
    }));

    let server = MockServer::start(state.clone());

    // packages.tdm -- new format:
    //   >>> <org>/<name>@<version>    (dependency line)
    //   <<<@<version> <name>          (self-identity)
    fs::write(
        project.join("packages.tdm"),
        ">>> alice/demo@a.1\n<<<@a.1 test/consumer\n",
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
        .join("alice")
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

    // Second install WITHOUT --no-remote-check -> decision table should
    // detect the SHA change and auto-refresh.
    let output2 = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .current_dir(&project)
        .env("HOME", &fake_home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install (2nd)");
    assert!(
        output2.status.success(),
        "second install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output2.stdout),
        String::from_utf8_lossy(&output2.stderr)
    );

    // The stale-detection info must appear on stderr. Previous sidecar had
    // commit_sha=aaaa (row 2b initially, then row 3 on the second install
    // after sidecar has real SHA written in).
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(
        stderr2.contains("refreshing store"),
        "expected refresh log on stderr, got:\n{}",
        stderr2
    );

    // v2 content must now be present; v1 must be gone.
    assert!(
        store_dir.join("demo_v2.td").exists(),
        "v2 content missing after refresh; store state=\n{:?}",
        fs::read_dir(&store_dir).map(|e| e.flatten().map(|d| d.file_name()).collect::<Vec<_>>())
    );
    assert!(
        !store_dir.join("demo_v1.td").exists(),
        "stale v1 content survived refresh"
    );

    // Sidecar must now record bbbb.
    let meta = fs::read_to_string(store_dir.join("_meta.toml")).expect("sidecar exists");
    assert!(
        meta.contains("commit_sha = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\""),
        "sidecar did not update, got:\n{}",
        meta
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}
