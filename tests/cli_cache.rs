//! CLI `taida ingot cache` and WASM build cache tests.
//!
//! Covers: RC-8a WASM runtime cache hit, RC-8d cache clean command.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::{taida_bin, unique_temp_dir};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

// -----------------------------------------------------------------------
// T-1: WASM runtime cache -- second build should hit cache (RC-8a)
// -----------------------------------------------------------------------

#[test]
fn test_rc8a_wasm_cache_hit_on_second_build() {
    let td = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/01_hello.td");
    if !td.exists() {
        return; // skip if examples missing
    }

    let tmp = unique_temp_dir("rc8a_cache_hit");
    let _ = fs::create_dir_all(&tmp);
    let wasm_out = tmp.join("hello.wasm");

    // First build -- compiles runtime (cache miss)
    let out1 = Command::new(taida_bin())
        .args(["build", "wasm-min", "-o"])
        .arg(&wasm_out)
        .arg(&td)
        .output()
        .expect("first wasm build");
    assert!(
        out1.status.success(),
        "first build failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(
        wasm_out.exists(),
        "wasm output should exist after first build"
    );

    let _ = fs::remove_file(&wasm_out);

    // Second build -- should hit cache (faster, same result)
    let out2 = Command::new(taida_bin())
        .args(["build", "wasm-min", "-o"])
        .arg(&wasm_out)
        .arg(&td)
        .output()
        .expect("second wasm build");
    assert!(
        out2.status.success(),
        "second build (cache hit) failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        wasm_out.exists(),
        "wasm output should exist after cache hit build"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------------
// T-2: `taida ingot cache clean` (RC-8d)
// -----------------------------------------------------------------------

#[test]
fn test_rc8d_cache_clean_removes_files() {
    // Isolate from the shared `target/wasm-rt-cache/` directory used by other
    // wasm build tests (e.g. test_rc8a, tests/wasm_*.rs) by running the
    // subprocess in a unique temp dir so `target/wasm-rt-cache/` resolves
    // to `{tmp}/target/wasm-rt-cache/` instead of the project root cache.
    let tmp = unique_temp_dir("rc8d_cache_clean");
    let cache_dir = tmp.join("target").join("wasm-rt-cache");
    let _ = fs::create_dir_all(&cache_dir);

    let fake_o = cache_dir.join("test_clean.deadbeef.o");
    let fake_tmp = cache_dir.join("test_clean.deadbeef.42.0.tmp.o");
    let _ = fs::write(&fake_o, b"fake");
    let _ = fs::write(&fake_tmp, b"fake");

    let output = Command::new(taida_bin())
        .args(["ingot", "cache", "clean"])
        .current_dir(&tmp)
        .output()
        .expect("cache clean");
    assert!(
        output.status.success(),
        "cache clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cleaned") || stdout.contains("already clean"),
        "should report cleaning result, got: {}",
        stdout
    );

    assert!(!fake_o.exists(), "fake .o should be removed by cache clean");
    assert!(
        !fake_tmp.exists(),
        "fake .tmp.o should be removed by cache clean"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Regression: `taida ingot cache clean` in a project with `.taida/` + `packages.tdm`
/// must find the project-local cache (`.taida/cache/wasm-rt/`), not the fallback.
#[test]
fn test_cache_clean_finds_project_local_cache() {
    let tmp = unique_temp_dir("cache_proj_local");
    let _ = fs::create_dir_all(tmp.join(".taida").join("cache").join("wasm-rt"));
    let _ = fs::write(tmp.join("packages.tdm"), "");

    // Place a fake cached file in the project-local cache
    let fake_o = tmp
        .join(".taida")
        .join("cache")
        .join("wasm-rt")
        .join("fake.deadbeef.o");
    let _ = fs::write(&fake_o, b"fake");
    assert!(fake_o.exists(), "setup: fake .o should exist");

    let output = Command::new(taida_bin())
        .args(["ingot", "cache", "clean"])
        .current_dir(&tmp)
        .output()
        .expect("cache clean in project dir");
    assert!(
        output.status.success(),
        "cache clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !fake_o.exists(),
        "project-local cache .o should be removed by cache clean"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_rc8d_cache_unknown_subcommand() {
    let output = Command::new(taida_bin())
        .args(["ingot", "cache", "bogus"])
        .output()
        .expect("cache bogus");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown cache command"),
        "should reject unknown subcommand, got: {}",
        stderr
    );
}

#[test]
fn test_rc8d_cache_help() {
    let output = Command::new(taida_bin())
        .args(["ingot", "cache", "--help"])
        .output()
        .expect("cache --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("clean"),
        "cache help should mention 'clean', got: {}",
        stdout
    );
}

// -----------------------------------------------------------------------
// C17B-004: CLI integration tests for `taida ingot cache clean --store` etc.
// -----------------------------------------------------------------------
//
// These tests exercise the CLI wiring of the C17 store-prune path (the
// library-level `prune_store_root` / `prune_store_package` are covered
// by unit tests in `src/pkg/store.rs`). They run the real `taida` binary
// with `HOME` pointed at a temp directory so no real `~/.taida/store/`
// is touched.

/// Populate `<home>/.taida/store/<org>/<name>/<version>/` with a minimal
/// fake install (marker + sidecar + one file) so the prune summary has
/// something to report.
fn populate_store(
    home: &std::path::Path,
    packages: &[(&str, &str, &str)], // (org, name, version)
) {
    for (org, name, version) in packages {
        let dir = home
            .join(".taida")
            .join("store")
            .join(org)
            .join(name)
            .join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".taida_installed"), "").unwrap();
        fs::write(dir.join("main.td"), "// populated by test\n").unwrap();
        fs::write(
            dir.join("_meta.toml"),
            format!(
                "schema_version = 1\ncommit_sha = \"\"\ntarball_sha256 = \"abc\"\nfetched_at = \"2026-04-17T00:00:00Z\"\nsource = \"github:{}/{}\"\nversion = \"{}\"\n",
                org, name, version
            ),
        )
        .unwrap();
    }
}

fn store_entry_exists(home: &std::path::Path, org: &str, name: &str, version: &str) -> bool {
    home.join(".taida")
        .join("store")
        .join(org)
        .join(name)
        .join(version)
        .join(".taida_installed")
        .exists()
}

#[cfg(unix)]
#[test]
fn c17b_004_cache_clean_store_non_tty_without_yes_rejects() {
    // C17B-004 test #1: In a non-TTY context (piped stdin) `cache clean
    // --store` without `--yes` must exit non-zero with a clear message.
    let tmp = unique_temp_dir("c17b_004_nontty");
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    populate_store(&home, &[("alice", "http", "a.1")]);

    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--store"])
        .env("HOME", &home)
        // stdin is closed / not a tty because we don't inherit it
        .stdin(std::process::Stdio::null())
        .output()
        .expect("cache clean --store");
    assert!(
        !out.status.success(),
        "non-TTY without --yes must exit non-zero, stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Refusing to prune store"),
        "stderr must explain the non-TTY refusal, got:\n{}",
        stderr
    );
    // Package must still be intact.
    assert!(
        store_entry_exists(&home, "alice", "http", "a.1"),
        "non-TTY refusal must NOT delete anything"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn c17b_004_cache_clean_store_with_yes_removes_all_packages() {
    // C17B-004 test #2: `--store --yes` wipes the entire store.
    let tmp = unique_temp_dir("c17b_004_store_yes");
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    populate_store(&home, &[("alice", "http", "a.1"), ("bob", "rpc", "c.1")]);
    assert!(store_entry_exists(&home, "alice", "http", "a.1"));
    assert!(store_entry_exists(&home, "bob", "rpc", "c.1"));

    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--store", "--yes"])
        .env("HOME", &home)
        .output()
        .expect("cache clean --store --yes");
    assert!(
        out.status.success(),
        "clean --store --yes must succeed, stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Removed") || stdout.contains("package"),
        "stdout must report removal, got: {}",
        stdout
    );

    assert!(!store_entry_exists(&home, "alice", "http", "a.1"));
    assert!(!store_entry_exists(&home, "bob", "rpc", "c.1"));
    // The store root itself is kept.
    assert!(
        home.join(".taida").join("store").exists(),
        "store root should persist (empty)"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn c17b_004_cache_clean_store_pkg_targets_single_package() {
    // C17B-004 test #3: `--store-pkg <org>/<name>` removes only the
    // named package across all versions; other packages survive.
    let tmp = unique_temp_dir("c17b_004_store_pkg");
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    populate_store(
        &home,
        &[
            ("alice", "http", "a.1"),
            ("alice", "http", "a.2"),
            ("alice", "other", "a.1"),
            ("bob", "rpc", "c.1"),
        ],
    );

    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--store-pkg", "alice/http"])
        .env("HOME", &home)
        .output()
        .expect("cache clean --store-pkg alice/http");
    assert!(
        out.status.success(),
        "clean --store-pkg must succeed, stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(!store_entry_exists(&home, "alice", "http", "a.1"));
    assert!(!store_entry_exists(&home, "alice", "http", "a.2"));
    assert!(
        store_entry_exists(&home, "alice", "other", "a.1"),
        "sibling under same org must survive"
    );
    assert!(
        store_entry_exists(&home, "bob", "rpc", "c.1"),
        "unrelated org must survive"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn c17b_004_cache_clean_store_pkg_conflicts_with_store() {
    // C17B-004 test #4: `--store-pkg` + `--store` is rejected by the
    // CLI mutual-exclusion check with a clear error message.
    let tmp = unique_temp_dir("c17b_004_conflict_store");
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    populate_store(&home, &[("alice", "http", "a.1")]);

    let out = Command::new(taida_bin())
        .args([
            "ingot",
            "cache",
            "clean",
            "--store",
            "--store-pkg",
            "alice/http",
        ])
        .env("HOME", &home)
        .output()
        .expect("cache clean conflict");
    assert!(
        !out.status.success(),
        "--store + --store-pkg must exit non-zero, stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot be combined"),
        "stderr must explain the conflict, got: {}",
        stderr
    );
    // Package must still exist.
    assert!(store_entry_exists(&home, "alice", "http", "a.1"));

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn c17b_004_cache_clean_store_pkg_rejects_three_segments() {
    // C17B-004 test #5: `--store-pkg foo/bar/baz` -> "Invalid --store-pkg value"
    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--store-pkg", "foo/bar/baz"])
        .output()
        .expect("cache clean --store-pkg three segments");
    assert!(
        !out.status.success(),
        "three-segment value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Invalid --store-pkg value"),
        "stderr must be 'Invalid --store-pkg value ...', got: {}",
        stderr
    );
}

#[test]
fn c17b_004_cache_clean_store_pkg_rejects_trailing_slash() {
    // C17B-004 test #6: `--store-pkg foo/` (empty name) -> rejected.
    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--store-pkg", "foo/"])
        .output()
        .expect("cache clean --store-pkg trailing slash");
    assert!(
        !out.status.success(),
        "trailing-slash value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Invalid --store-pkg value"),
        "stderr must be 'Invalid --store-pkg value ...', got: {}",
        stderr
    );
}

#[test]
fn c17b_004_cache_clean_store_pkg_rejects_leading_slash() {
    // Additional C17B-004 coverage: `--store-pkg /foo` (empty org).
    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--store-pkg", "/foo"])
        .output()
        .expect("cache clean --store-pkg leading slash");
    assert!(
        !out.status.success(),
        "leading-slash value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Invalid --store-pkg value"),
        "stderr must be 'Invalid --store-pkg value ...', got: {}",
        stderr
    );
}

#[test]
fn c17b_004_cache_clean_all_with_yes_prunes_store_too() {
    // C17B-004 test #7: `--all --yes` must wipe WASM + addon-cache + store.
    let tmp = unique_temp_dir("c17b_004_all");
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();

    // Populate store under HOME.
    populate_store(&home, &[("alice", "http", "a.1")]);
    // Populate addon-cache under HOME so we can verify it is also cleaned.
    let addon_cache = home
        .join(".taida")
        .join("addon-cache")
        .join("alice")
        .join("http")
        .join("a.1");
    fs::create_dir_all(&addon_cache).unwrap();
    fs::write(addon_cache.join("libaddon.so"), b"fake").unwrap();

    // WASM cache lives under project CWD, so create one there.
    fs::create_dir_all(tmp.join("target").join("wasm-rt-cache")).unwrap();
    let wasm_o = tmp
        .join("target")
        .join("wasm-rt-cache")
        .join("fake.deadbeef.o");
    fs::write(&wasm_o, b"fake").unwrap();

    let out = Command::new(taida_bin())
        .args(["ingot", "cache", "clean", "--all", "--yes"])
        .env("HOME", &home)
        .current_dir(&tmp)
        .output()
        .expect("cache clean --all --yes");
    assert!(
        out.status.success(),
        "clean --all --yes must succeed, stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Store gone.
    assert!(!store_entry_exists(&home, "alice", "http", "a.1"));
    // Addon cache entry gone (cache tree may survive empty).
    assert!(
        !addon_cache.join("libaddon.so").exists(),
        "--all must also wipe addon-cache"
    );
    // WASM .o gone.
    assert!(!wasm_o.exists(), "--all must also wipe wasm cache");

    let _ = fs::remove_dir_all(&tmp);
}
