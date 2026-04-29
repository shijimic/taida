#![allow(clippy::doc_overindented_list_items)]

//! C17-5: `taida ingot install --force-refresh` re-extracts the store entry even
//! when sidecar SHA matches the remote (fast-path would otherwise skip).
//!
//! Also verifies:
//! - `--force-refresh` + `--no-remote-check` is rejected by the CLI.
//! - `--force-refresh` on an existing sidecar-less install refreshes
//!   without the "unknown provenance" strong warning (force-refresh is
//!   the answer the warning points the user at).

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

fn write_consumer(project: &std::path::Path) {
    fs::write(
        project.join("packages.tdm"),
        ">>> alice/force@a.1\n<<<@a.1 test/consumer\n",
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").unwrap();
}

/// Parse the RFC-3339 `fetched_at` line from a sidecar into a string we
/// can compare lexically (the format `YYYY-MM-DDTHH:MM:SSZ` is already
/// lexicographically ordered by time).
fn sidecar_fetched_at(pkg_dir: &std::path::Path) -> String {
    let text = fs::read_to_string(pkg_dir.join("_meta.toml")).expect("sidecar exists");
    for line in text.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "fetched_at"
        {
            return value.trim().trim_matches('"').to_string();
        }
    }
    panic!("sidecar has no fetched_at line:\n{}", text);
}

fn run_install(
    project: &std::path::Path,
    home: &std::path::Path,
    base: &str,
    api: &str,
    extra: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(taida_bin());
    cmd.arg("ingot").arg("install");
    for a in extra {
        cmd.arg(a);
    }
    cmd.current_dir(project)
        .env("HOME", home)
        .env("TAIDA_GITHUB_BASE_URL", base)
        .env("TAIDA_GITHUB_API_URL", api)
        .env("GH_TOKEN", "unused");
    cmd.output().expect("run taida ingot install")
}

#[test]
fn c17_5_force_refresh_rewrites_store_entry_even_when_fresh() {
    let work = unique_temp_dir("c17_force_fresh");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/force\n" as &[u8]),
        ("main.td", b"stdout(\"same\")\n"),
        ("marker.td", b"// gen=1\n"),
    ]);

    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "force".into(),
        version: "a.1".into(),
        commit_sha: "1111111111111111111111111111111111111111".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());

    write_consumer(&project);

    // Install 1 (cold): sidecar is written with commit_sha="" because
    // Phase 2 does not do a remote lookup on the first install path.
    let out1 = run_install(&project, &home, &server.base_url(), &server.api_url(), &[]);
    assert!(
        out1.status.success(),
        "first install failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );

    let store_dir = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("force")
        .join("a.1");

    // Install 2: row 2b (sidecar has empty commit_sha + remote known) -> refresh.
    // After this install, sidecar's commit_sha is the real SHA.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out2 = run_install(&project, &home, &server.base_url(), &server.api_url(), &[]);
    assert!(
        out2.status.success(),
        "2nd install failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr2.contains("refreshing store"),
        "row 2b should trigger a refresh on install 2, got stderr:\n{}",
        stderr2
    );

    // Install 3: row 2 (sidecar SHA == remote SHA) -> fast path, no refresh.
    let fetched_at_2 = sidecar_fetched_at(&store_dir);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out3 = run_install(&project, &home, &server.base_url(), &server.api_url(), &[]);
    assert!(
        out3.status.success(),
        "3rd install failed: {}",
        String::from_utf8_lossy(&out3.stderr)
    );
    let stderr3 = String::from_utf8_lossy(&out3.stderr);
    assert!(
        !stderr3.contains("refreshing store"),
        "fast-path must not print a refresh line, got stderr:\n{}",
        stderr3
    );
    let fetched_at_3 = sidecar_fetched_at(&store_dir);
    assert_eq!(
        fetched_at_2, fetched_at_3,
        "fast-path skip: fetched_at must not move"
    );

    // Install 4: --force-refresh must re-extract even though fast-path would skip.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out4 = run_install(
        &project,
        &home,
        &server.base_url(),
        &server.api_url(),
        &["--force-refresh"],
    );
    assert!(
        out4.status.success(),
        "force-refresh install failed: {}",
        String::from_utf8_lossy(&out4.stderr)
    );
    let stderr4 = String::from_utf8_lossy(&out4.stderr);
    assert!(
        stderr4.contains("refreshing store"),
        "expected refresh log on stderr, got:\n{}",
        stderr4
    );
    let fetched_at_4 = sidecar_fetched_at(&store_dir);
    assert!(
        fetched_at_4 > fetched_at_3,
        "--force-refresh must re-extract: sidecar.fetched_at must advance ({} !> {})",
        fetched_at_4,
        fetched_at_3
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn c17_5_force_refresh_conflicts_with_no_remote_check() {
    let work = unique_temp_dir("c17_conflict");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    write_consumer(&project);

    // No server needed -- we want the CLI arg parser to reject before
    // it even looks at the network.
    let out = Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .arg("--force-refresh")
        .arg("--no-remote-check")
        .current_dir(&project)
        .env("HOME", &home)
        .output()
        .expect("run taida ingot install");
    assert!(
        !out.status.success(),
        "mutual-exclusion must exit non-zero, stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--force-refresh") && stderr.contains("--no-remote-check"),
        "error must mention both flags, got:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn c17_5_force_refresh_handles_sidecar_less_install() {
    // Simulate a pre-C17 install: `.taida_installed` is present but no
    // `_meta.toml`. Without --force-refresh this would print the "unknown
    // provenance" warning. With --force-refresh the user has opted in and
    // the install must succeed without that warning.
    let work = unique_temp_dir("c17_sidecar_less");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/legacy\n" as &[u8]),
        ("main.td", b"stdout(\"legacy\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "legacy".into(),
        version: "a.1".into(),
        commit_sha: "2222222222222222222222222222222222222222".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());

    // Pre-populate the store the way a pre-C17 install would have:
    let store_pkg = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("legacy")
        .join("a.1");
    fs::create_dir_all(&store_pkg).unwrap();
    fs::write(store_pkg.join(".taida_installed"), "").unwrap();
    fs::write(store_pkg.join("packages.tdm"), "<<<@a.1 alice/legacy\n").unwrap();
    fs::write(store_pkg.join("main.td"), "stdout(\"old\")\n").unwrap();
    // Deliberately no `_meta.toml`.

    fs::write(
        project.join("packages.tdm"),
        ">>> alice/legacy@a.1\n<<<@a.1 test/consumer\n",
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"consumer\")\n").unwrap();

    let out = run_install(
        &project,
        &home,
        &server.base_url(),
        &server.api_url(),
        &["--force-refresh"],
    );
    assert!(
        out.status.success(),
        "force-refresh install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unknown provenance"),
        "force-refresh must suppress the 'unknown provenance' warning, got:\n{}",
        stderr
    );
    assert!(
        stderr.contains("refreshing store"),
        "force-refresh must log the refresh, got:\n{}",
        stderr
    );

    // Sidecar must now exist with the real SHA.
    let meta = fs::read_to_string(store_pkg.join("_meta.toml")).expect("sidecar written");
    assert!(
        meta.contains("2222222222222222222222222222222222222222"),
        "sidecar must record new SHA, got:\n{}",
        meta
    );

    drop(server);
    let _ = fs::remove_dir_all(&work);
}
