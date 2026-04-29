#![allow(clippy::doc_overindented_list_items)]

//! C17-5: offline-path contract.
//!
//! When the remote is unreachable:
//! - sidecar present  -> `SkipWithOfflineWarning`: install succeeds, stderr
//!                       mentions "offline, cannot verify staleness".
//! - sidecar missing  -> `SkipUnknownProvenanceStrongWarn`: install
//!                       succeeds (so the user can keep working), stderr
//!                       mentions "unknown provenance" and directs the
//!                       user at `--force-refresh`.
//!
//! Also verifies `--no-remote-check` short-circuits the remote lookup
//! entirely: sidecar present alone is enough to skip, no warning is
//! printed.

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

/// URL pointing at a closed TCP port. `curl -fsSL` exits non-zero, which
/// `curl_get_optional` maps to `Ok(None)` -- the offline branch of the
/// decision table. Port 1 is privileged and typically never open to an
/// unprivileged user; if it happens to be open the test would flake, but
/// that is vanishingly rare on CI Linux.
const CLOSED_URL: &str = "http://127.0.0.1:1";

fn write_consumer(project: &std::path::Path, pkg: &str) {
    fs::write(
        project.join("packages.tdm"),
        format!(">>> alice/{}@a.1\n<<<@a.1 test/consumer\n", pkg),
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"c\")\n").unwrap();
}

fn run_install_with_env(
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
fn c17_5_offline_with_sidecar_prints_offline_warning() {
    let work = unique_temp_dir("c17_offline_with_sidecar");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // Step 1: do a successful install against the mock so the store
    // ends up with a sidecar.
    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/warm\n" as &[u8]),
        ("main.td", b"stdout(\"warm\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "warm".into(),
        version: "a.1".into(),
        commit_sha: "3333333333333333333333333333333333333333".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());
    write_consumer(&project, "warm");
    // Two warm-up installs so the sidecar records the real SHA (row 2b
    // upgrade from empty commit_sha, then fast-path).
    let _ = run_install_with_env(&project, &home, &server.base_url(), &server.api_url(), &[]);
    let _ = run_install_with_env(&project, &home, &server.base_url(), &server.api_url(), &[]);
    drop(server);

    // Step 2: run install with API pointing at a closed port -> the
    // decision table should see `(Some(sidecar), None)` and print the
    // offline warning, but still exit 0.
    let out = run_install_with_env(
        &project,
        &home,
        CLOSED_URL, // archive is never fetched on a fast-path skip
        CLOSED_URL,
        &[],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "offline install with sidecar must exit 0, stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    assert!(
        stderr.contains("offline, cannot verify staleness"),
        "stderr must include offline warning, got:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("refreshing store"),
        "offline + sidecar must NOT refresh, got:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn c17_5_offline_without_sidecar_prints_strong_warning() {
    let work = unique_temp_dir("c17_offline_bare");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // Simulate a pre-C17 install (no sidecar).
    let store_pkg = home
        .join(".taida")
        .join("store")
        .join("alice")
        .join("bare")
        .join("a.1");
    fs::create_dir_all(&store_pkg).unwrap();
    fs::write(store_pkg.join(".taida_installed"), "").unwrap();
    fs::write(store_pkg.join("packages.tdm"), "<<<@a.1 alice/bare\n").unwrap();
    fs::write(store_pkg.join("main.td"), "stdout(\"old\")\n").unwrap();

    write_consumer(&project, "bare");

    let out = run_install_with_env(&project, &home, CLOSED_URL, CLOSED_URL, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "offline install without sidecar must still exit 0, stderr=\n{}",
        stderr
    );
    assert!(
        stderr.contains("unknown provenance"),
        "stderr must include strong warning, got:\n{}",
        stderr
    );
    assert!(
        stderr.contains("--force-refresh"),
        "strong warning must point user at --force-refresh, got:\n{}",
        stderr
    );

    // Store must still be intact (we did not wipe it silently).
    assert!(
        store_pkg.join("main.td").exists(),
        "offline skip must not touch the existing store entry"
    );

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn c17_5_no_remote_check_skips_lookup_silently() {
    let work = unique_temp_dir("c17_no_remote");
    let home = work.join("home");
    let project = work.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();

    // Start with a warm install so a sidecar with a real SHA is in
    // place. Then verify that --no-remote-check skips the API hit even
    // when the API endpoint would have worked.
    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/noremote\n" as &[u8]),
        ("main.td", b"stdout(\"nr\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "noremote".into(),
        version: "a.1".into(),
        commit_sha: "4444444444444444444444444444444444444444".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());
    write_consumer(&project, "noremote");
    let _ = run_install_with_env(&project, &home, &server.base_url(), &server.api_url(), &[]);
    let _ = run_install_with_env(&project, &home, &server.base_url(), &server.api_url(), &[]);
    drop(server);

    // Now point the API at a closed port; with --no-remote-check the
    // lookup should be skipped entirely, so no offline warning is
    // emitted.
    let out = run_install_with_env(
        &project,
        &home,
        CLOSED_URL,
        CLOSED_URL,
        &["--no-remote-check"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--no-remote-check must exit 0, stderr=\n{}",
        stderr
    );
    assert!(
        !stderr.contains("offline, cannot verify staleness"),
        "--no-remote-check suppresses the offline warning; got stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("refreshing store"),
        "--no-remote-check with sidecar present must not refresh; got stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&work);
}
