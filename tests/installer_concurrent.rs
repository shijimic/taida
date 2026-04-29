#![allow(clippy::doc_overindented_list_items)]

//! C17B-009: concurrent `taida ingot install` processes must not corrupt the
//! store.
//!
//! Contract: two `taida ingot install` processes racing on the same
//! `<org>/<name>/<version>/` may block on each other (flock LOCK_EX) but
//! must both eventually exit 0 with a well-formed sidecar and marker in
//! place. No `.tmp-*` or `.refresh-staging-*` scratch may remain.

#![cfg(unix)]

mod common;
mod mock;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

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
fn c17b_009_two_concurrent_installs_serialize_safely() {
    let work = unique_temp_dir("c17b_009_concurrent");
    let home = work.join("home");
    fs::create_dir_all(&home).unwrap();

    // Two separate project directories -- both request the same package
    // so the install lock scope is the same pkg_dir.
    let project_a = work.join("project_a");
    let project_b = work.join("project_b");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();

    let tarball = make_tarball(&[
        ("packages.tdm", b"<<<@a.1 alice/race\n" as &[u8]),
        ("main.td", b"stdout(\"raced\")\n"),
    ]);
    let state = Arc::new(Mutex::new(TagState {
        org: "alice".into(),
        name: "race".into(),
        version: "a.1".into(),
        commit_sha: "7777777777777777777777777777777777777777".into(),
        tarball,
    }));
    let server = MockServer::start(state.clone());
    let base = server.base_url();
    let api = server.api_url();

    for p in [&project_a, &project_b] {
        fs::write(
            p.join("packages.tdm"),
            ">>> alice/race@a.1\n<<<@a.1 test/consumer\n",
        )
        .unwrap();
        fs::write(p.join("main.td"), "stdout(\"c\")\n").unwrap();
    }

    let spawn_install = |project: PathBuf, home: PathBuf, base: String, api: String| {
        thread::spawn(move || {
            Command::new(taida_bin())
                .arg("ingot")
                .arg("install")
                .current_dir(&project)
                .env("HOME", &home)
                .env("TAIDA_GITHUB_BASE_URL", &base)
                .env("TAIDA_GITHUB_API_URL", &api)
                .env("GH_TOKEN", "unused")
                .output()
                .expect("run taida ingot install")
        })
    };

    let t1 = spawn_install(project_a.clone(), home.clone(), base.clone(), api.clone());
    let t2 = spawn_install(project_b.clone(), home.clone(), base.clone(), api.clone());
    let out1 = t1.join().unwrap();
    let out2 = t2.join().unwrap();

    assert!(
        out1.status.success(),
        "install 1 failed: stderr={}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(
        out2.status.success(),
        "install 2 failed: stderr={}",
        String::from_utf8_lossy(&out2.stderr)
    );

    // The store must end in a consistent state: real version dir is
    // present with both marker and sidecar, no scratch dirs remain.
    let pkg_parent = home.join(".taida").join("store").join("alice").join("race");
    let version_dir = pkg_parent.join("a.1");
    assert!(
        version_dir.join(".taida_installed").exists(),
        "race: marker must exist after both installs"
    );
    assert!(
        version_dir.join("_meta.toml").exists(),
        "race: sidecar must exist after both installs"
    );
    let main_td = fs::read_to_string(version_dir.join("main.td")).unwrap();
    assert!(
        main_td.contains("raced"),
        "race: content must match remote tarball"
    );

    for entry in fs::read_dir(&pkg_parent).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(
            !name.starts_with(".tmp-"),
            "race: .tmp-* scratch left behind: {}",
            name
        );
        assert!(
            !name.contains(".refresh-staging-"),
            "race: .refresh-staging-* detritus left behind: {}",
            name
        );
    }

    drop(server);
    let _ = fs::remove_dir_all(&work);
}
