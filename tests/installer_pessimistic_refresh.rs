#![allow(clippy::doc_overindented_list_items)]

//! C17B-003: decision-table Row 1 (sidecar missing, remote known, flag
//! absent) pure end-to-end test.
//!
//! The Phase 5 suite covered the `--force-refresh` short-circuit but did
//! not cover the "pre-C17 install upgraded to C17" migration path: a
//! legacy store entry has `.taida_installed` present but no `_meta.toml`.
//! On the next install, with remote reachable and no flags, the
//! `classify_stale` table must drive `StaleOutcome::Refresh(MissingSidecar)`
//! so the sidecar is filled in and the user's facade files are re-extracted
//! from the current remote tarball.

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

/// C17B-003: pre-C17 install (`.taida_installed` present, `_meta.toml`
/// absent) + online remote + NO flags -> pessimistic refresh.
///
/// Contract:
/// - stderr contains "missing sidecar; refreshing store"
/// - sidecar is written with the remote SHA
/// - the old pre-C17 content is replaced by the remote's current content
/// - install exits 0
#[test]
fn c17b_003_row1_sidecarless_online_refreshes_pessimistically() {
    let work = unique_temp_dir("c17_row1_pessimistic");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // Pre-populate a pre-C17 extraction: legacy facade only, no sidecar.
    let store_pkg = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("row1")
        .join("a.1");
    fs::create_dir_all(&store_pkg).unwrap();
    fs::write(store_pkg.join(".taida_installed"), "").unwrap();
    fs::write(store_pkg.join("packages.tdm"), "<<<@a.1 alice/row1\n").unwrap();
    fs::write(store_pkg.join("main.td"), "stdout(\"LEGACY_PRE_C17\")\n").unwrap();
    // Deliberately: NO _meta.toml
    assert!(
        !store_pkg.join("_meta.toml").exists(),
        "pre-condition: sidecar must be absent for Row 1"
    );

    // Remote tarball has a fresh content that differs from the legacy
    // facade. The refresh must replace the local content with this.
    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/row1\n" as &[u8]),
        ("main.td", b"stdout(\"FRESH_FROM_REMOTE\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "row1".into(),
        version: "a.1".into(),
        commit_sha: "abcdef1234567890abcdef1234567890abcdef12".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());

    fs::write(
        project.join("packages.tdm"),
        ">>> alice/row1@a.1\n<<<@a.1 test/consumer\n",
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").unwrap();

    let out = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "Row 1 install must exit 0, stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        stderr.contains("missing sidecar") && stderr.contains("refreshing store"),
        "Row 1 must log 'missing sidecar; refreshing store', got stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("unknown provenance"),
        "Row 1 (remote known) must NOT emit the strong offline warning, got:\n{}",
        stderr
    );

    // Sidecar must now exist with the remote SHA.
    let meta_path = store_pkg.join("_meta.toml");
    assert!(
        meta_path.exists(),
        "Row 1 refresh must write a sidecar; dir contents: {:?}",
        fs::read_dir(&store_pkg)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    let meta = fs::read_to_string(&meta_path).unwrap();
    assert!(
        meta.contains("abcdef1234567890abcdef1234567890abcdef12"),
        "sidecar must record the remote commit SHA, got:\n{}",
        meta
    );
    // Row 1 contract: content is replaced by remote's current tarball.
    let main_td = fs::read_to_string(store_pkg.join("main.td")).expect("main.td must exist");
    assert!(
        main_td.contains("FRESH_FROM_REMOTE"),
        "Row 1 refresh must replace legacy content with remote's, got:\n{}",
        main_td
    );
    assert!(
        !main_td.contains("LEGACY_PRE_C17"),
        "Row 1 refresh must not retain stale legacy content, got:\n{}",
        main_td
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}

/// C17B-001 e2e: force-refresh + offline must not lose the user's install.
///
/// Set up a working install with a sidecar. Run `taida ingot install
/// --force-refresh` while the remote and archive URLs point at closed
/// ports. The fetch must fail, but the backup-swap rollback must restore
/// the previous working install (main.td + sidecar intact).
#[test]
fn c17b_001_force_refresh_offline_rolls_back_to_previous_install() {
    let work = unique_temp_dir("c17b_001_rollback");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // Step 1: warm up with a real online install so a sidecar+content
    // are in place.
    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/warm2\n" as &[u8]),
        ("main.td", b"stdout(\"PRECIOUS_WORKING_CONTENT\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "warm2".into(),
        version: "a.1".into(),
        commit_sha: "5555555555555555555555555555555555555555".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());
    fs::write(
        project.join("packages.tdm"),
        ">>> alice/warm2@a.1\n<<<@a.1 test/consumer\n",
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"c\")\n").unwrap();

    let warmup = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install");
    assert!(
        warmup.status.success(),
        "warmup install must succeed, stderr={}",
        String::from_utf8_lossy(&warmup.stderr)
    );
    drop(server);

    let store_pkg = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("warm2")
        .join("a.1");
    let precious =
        fs::read_to_string(store_pkg.join("main.td")).expect("precious content must exist");
    assert!(precious.contains("PRECIOUS_WORKING_CONTENT"));

    // Step 2: run force-refresh with every network endpoint closed.
    // The fetch MUST fail, and the rollback MUST restore everything.
    let out = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .arg("--force-refresh")
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", "http://127.0.0.1:1")
        .env("TAIDA_GITHUB_API_URL", "http://127.0.0.1:1")
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install");
    // `taida ingot install` returns non-zero when any dep fails to resolve,
    // but the critical behaviour is the state of the store.
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        store_pkg.join(".taida_installed").exists(),
        "C17B-001: force-refresh + offline must preserve the marker; stderr=\n{}",
        stderr
    );
    assert!(
        store_pkg.join("main.td").exists(),
        "C17B-001: force-refresh + offline must preserve main.td; stderr=\n{}",
        stderr
    );
    let after = fs::read_to_string(store_pkg.join("main.td")).unwrap();
    assert!(
        after.contains("PRECIOUS_WORKING_CONTENT"),
        "C17B-001: rolled-back content must match the pre-refresh state, got:\n{}",
        after
    );
    assert!(
        store_pkg.join("_meta.toml").exists(),
        "C17B-001: sidecar must survive the rollback"
    );

    // No .refresh-staging-* detritus must remain.
    let parent = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("warm2");
    for entry in fs::read_dir(&parent).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(
            !name.contains(".refresh-staging-"),
            "C17B-001: abandoned staging dir detected: {}",
            name
        );
    }

    let _ = fs::remove_dir_all(&work);
}

/// C17B-015: malformed sidecar (`schema_version = 99`) triggers the
/// "unreadable; re-extracting" path and the sidecar is rewritten with a
/// fresh schema-1 entry recording the current remote SHA.
#[test]
fn c17b_015_corrupt_sidecar_re_extracts() {
    let work = unique_temp_dir("c17b_015_corrupt");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // Pre-populate a "corrupt" sidecar that the v1 parser will reject.
    let store_pkg = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("corrupt")
        .join("a.1");
    fs::create_dir_all(&store_pkg).unwrap();
    fs::write(store_pkg.join(".taida_installed"), "").unwrap();
    fs::write(store_pkg.join("packages.tdm"), "<<<@a.1 alice/corrupt\n").unwrap();
    fs::write(store_pkg.join("main.td"), "stdout(\"STALE\")\n").unwrap();
    // schema_version=99 triggers StoreError::UnknownMetaSchema on read.
    fs::write(
        store_pkg.join("_meta.toml"),
        "schema_version = 99\ncommit_sha = \"x\"\ntarball_sha256 = \"x\"\nfetched_at = \"x\"\nsource = \"x\"\nversion = \"a.1\"\n",
    )
    .unwrap();

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/corrupt\n" as &[u8]),
        ("main.td", b"stdout(\"FRESH_AFTER_RE_EXTRACT\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "corrupt".into(),
        version: "a.1".into(),
        commit_sha: "6666666666666666666666666666666666666666".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());

    fs::write(
        project.join("packages.tdm"),
        ">>> alice/corrupt@a.1\n<<<@a.1 test/consumer\n",
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"c\")\n").unwrap();

    let out = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .current_dir(&project)
        .env("HOME", &home)
        .env("TAIDA_GITHUB_BASE_URL", server.base_url())
        .env("TAIDA_GITHUB_API_URL", server.api_url())
        .env("GH_TOKEN", "unused")
        .output()
        .expect("run taida ingot install");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "install must succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    assert!(
        stderr.contains("unreadable"),
        "C17B-015: stderr must explain the sidecar was unreadable, got:\n{}",
        stderr
    );
    assert!(
        stderr.contains("re-extracting"),
        "C17B-015: stderr must signal re-extraction, got:\n{}",
        stderr
    );
    // C17B-019: for schema mismatches, stderr must surface a recovery hint.
    assert!(
        stderr.contains("hint:"),
        "C17B-019: stderr must include a user-facing hint, got:\n{}",
        stderr
    );

    // Sidecar must be rewritten with schema 1 and the real SHA.
    let meta = fs::read_to_string(store_pkg.join("_meta.toml")).unwrap();
    assert!(
        meta.contains("schema_version = 1"),
        "new sidecar must declare schema 1, got:\n{}",
        meta
    );
    assert!(
        meta.contains("6666666666666666666666666666666666666666"),
        "new sidecar must record the remote SHA, got:\n{}",
        meta
    );
    // Content must be replaced by remote's current content.
    let main_td = fs::read_to_string(store_pkg.join("main.td")).unwrap();
    assert!(
        main_td.contains("FRESH_AFTER_RE_EXTRACT"),
        "content must reflect remote after re-extract, got:\n{}",
        main_td
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}
