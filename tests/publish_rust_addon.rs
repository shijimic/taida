//! RC2.6 Phase 1 integration tests for `taida publish --target rust-addon`.
//!
//! These tests exercise the orchestration rewrite done in
//! RC2.6-1f against the real `taida` CLI binary. They are deliberately
//! layered so the fast path (dry-run) runs on every `cargo test`
//! invocation while the slower real-build path is exercised once per
//! run to keep CI latency reasonable.
//!
//! Layers:
//!
//! 1. **Dry-run auto-detect** — creates a minimal project with
//!    `native/addon.toml` present and `taida publish --dry-run`
//!    verifies the orchestrator branches into addon mode without
//!    touching cargo. This is the cheapest sanity check and will
//!    catch 90% of regressions in CLI argument parsing / addon
//!    detection.
//!
//! 2. **Dry-run explicit flag** — passes `--target rust-addon`
//!    explicitly both with and without a manifest present, asserting
//!    the mismatch case is a hard error.
//!
//! 3. **Real build with TAIDA_PUBLISH_SKIP_RELEASE=1** — spins up a
//!    self-contained cdylib crate, runs a full `taida publish
//!    --target rust-addon`, and verifies packages.tdm, the lockfile,
//!    the git tag, the remote push, and the release placeholder line
//!    in stdout. The crate is made a standalone workspace (`[workspace]`
//!    at the root) so cargo does not try to attach it to the outer
//!    taida workspace, and has zero external dependencies so the
//!    build latency is minimal.
//!
//! Each test runs in its own temp directory (unique nanosecond suffix)
//! and cleans itself up on success.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

fn run_git(args: &[&str], dir: &Path) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(args: &[&str], dir: &Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Set up a bare remote + working project that can push to it.
fn setup_project_with_remote(root: &Path) -> (PathBuf, PathBuf) {
    let bare = root.join("remote.git");
    let project = root.join("addon-pkg");
    fs::create_dir_all(&bare).expect("create bare dir");
    fs::create_dir_all(&project).expect("create project dir");

    run_git(&["init", "--bare"], &bare);
    run_git(&["init"], &project);
    run_git(&["config", "user.email", "test@taida.dev"], &project);
    run_git(&["config", "user.name", "Test User"], &project);
    run_git(
        &["remote", "add", "origin", bare.to_str().unwrap()],
        &project,
    );
    (project, bare)
}

/// Write a fake taida auth token so `taida publish` does not bail on
/// `auth::token::load_token`.
fn write_fake_auth(root: &Path, username: &str) -> PathBuf {
    let fake_home = root.join("fake-home");
    fs::create_dir_all(fake_home.join(".taida")).expect("create fake home");
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        format!(
            "{{\"github_token\":\"gho_test\",\"username\":\"{}\",\"created_at\":\"2026-04-09T00:00:00Z\"}}",
            username
        ),
    )
    .expect("write auth.json");
    fake_home
}

/// Write a minimal `native/addon.toml` declaring one function. No
/// `[library.prebuild]` section because Phase 1 stores the per-host
/// SHA-256 in `native/addon.lock.toml` instead.
fn write_addon_toml(project: &Path, package: &str, library: &str) {
    fs::create_dir_all(project.join("native")).unwrap();
    fs::write(
        project.join("native").join("addon.toml"),
        format!(
            "abi = 1\n\
             entry = \"taida_addon_get_v1\"\n\
             package = \"{}\"\n\
             library = \"{}\"\n\
             [functions]\n\
             noop = 0\n",
            package, library
        ),
    )
    .expect("write addon.toml");
}

// ──────────────────────────────────────────────────────────────
// Layer 1: dry-run auto-detect
// ──────────────────────────────────────────────────────────────

#[test]
fn test_publish_rust_addon_dry_run_auto_detect() {
    let root = unique_temp_dir("taida_publish_addon_dry");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").expect("write packages.tdm");
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").expect("write main.td");
    write_addon_toml(&project_dir, "tester/pkg", "tester_pkg");

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--dry-run"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dry-run should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    // Addon auto-detection happened.
    assert!(
        stdout.contains("Target: rust-addon"),
        "dry-run should report rust-addon target: {}",
        stdout
    );
    assert!(
        stdout.contains("Addon manifest"),
        "dry-run should report addon manifest path: {}",
        stdout
    );
    assert!(
        stdout.contains("Addon lockfile"),
        "dry-run should mention addon lockfile: {}",
        stdout
    );
    assert!(
        stdout.contains("Cargo build: skipped"),
        "dry-run must announce cargo build is skipped: {}",
        stdout
    );

    // packages.tdm must NOT be modified and addon.lock.toml must NOT exist.
    let manifest = fs::read_to_string(project_dir.join("packages.tdm")).expect("read packages.tdm");
    assert_eq!(manifest, "<<<@a @(run)\n");
    assert!(
        !project_dir.join("native").join("addon.lock.toml").exists(),
        "dry-run must not create addon.lock.toml"
    );

    // No tags should exist.
    let tags = git_output(&["tag", "--list"], &project_dir);
    assert!(tags.is_empty(), "dry-run should create no tags: {}", tags);

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 2a: explicit `--target rust-addon` flag with manifest
// ──────────────────────────────────────────────────────────────

#[test]
fn test_publish_rust_addon_dry_run_explicit_flag_with_manifest() {
    let root = unique_temp_dir("taida_publish_addon_flag");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();
    write_addon_toml(&project_dir, "tester/pkg", "tester_pkg");

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--target", "rust-addon", "--dry-run"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "dry-run with --target rust-addon should succeed: {}",
        stdout
    );
    assert!(stdout.contains("Target: rust-addon"));

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 2b: explicit `--target rust-addon` without manifest is rejected
// ──────────────────────────────────────────────────────────────

#[test]
fn test_publish_rust_addon_flag_without_manifest_is_rejected() {
    let root = unique_temp_dir("taida_publish_addon_mismatch");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();
    // NO native/addon.toml deliberately.

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--target", "rust-addon", "--dry-run"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(
        !output.status.success(),
        "flag + missing manifest should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rust-addon") && stderr.contains("native/addon.toml"),
        "error should mention the missing manifest: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 2c: invalid `--target` value
// ──────────────────────────────────────────────────────────────

#[test]
fn test_publish_unknown_target_value_is_rejected() {
    let root = unique_temp_dir("taida_publish_bad_target");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--target", "js-addon", "--dry-run"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown --target value") && stderr.contains("js-addon"),
        "error should reject js-addon: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 3: real build with TAIDA_PUBLISH_SKIP_RELEASE=1
// ──────────────────────────────────────────────────────────────

/// The cdylib-producing Cargo.toml writes `[workspace]` at the top so
/// cargo does NOT try to attach the temp crate to the outer taida
/// workspace. That means the temp crate has a completely independent
/// target/ directory (no cross-contamination with the taida build
/// cache) and zero external dependencies beyond what rustc ships.
const MINIMAL_CDYLIB_CARGO_TOML: &str = r#"[workspace]

[package]
name = "taida_rc26_phase1_fixture"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]
"#;

/// A trivial `cdylib` source file that exports the expected entry
/// symbol. The publish flow only needs the cdylib to exist on disk —
/// it does NOT try to load it via dlopen, so the body of
/// `taida_addon_get_v1` is irrelevant as long as the symbol is
/// present and the crate compiles.
const MINIMAL_CDYLIB_LIB_RS: &str = r#"#![allow(non_snake_case)]

// RC2.6 Phase 1 fixture: the publish orchestrator only needs a
// buildable cdylib for SHA-256 computation. It never dlopens the
// output, so we export a harmless null-returning stub for the frozen
// entry symbol.
#[unsafe(no_mangle)]
pub extern "C" fn taida_addon_get_v1() -> *const core::ffi::c_void {
    core::ptr::null()
}
"#;

#[test]
fn test_publish_rust_addon_full_flow_skip_release() {
    // The fixture compiles a trivial cdylib with no external deps,
    // which takes ~2-3 seconds cold on a modern machine. We gate
    // this test behind `cfg(feature = "community")` at the crate
    // level (dev-dependencies do that automatically), and on
    // the presence of `cargo` in PATH.
    if Command::new("cargo").arg("--version").output().is_err() {
        eprintln!("cargo not available; skipping test_publish_rust_addon_full_flow_skip_release");
        return;
    }

    let root = unique_temp_dir("taida_publish_addon_full");
    let (project_dir, bare) = setup_project_with_remote(&root);

    // packages.tdm + main.td.
    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    // Cargo.toml + src/lib.rs producing a cdylib with a stem that
    // matches the library field below.
    fs::write(project_dir.join("Cargo.toml"), MINIMAL_CDYLIB_CARGO_TOML).unwrap();
    fs::create_dir_all(project_dir.join("src")).unwrap();
    fs::write(
        project_dir.join("src").join("lib.rs"),
        MINIMAL_CDYLIB_LIB_RS,
    )
    .unwrap();

    // native/addon.toml with library = "taida_rc26_phase1_fixture"
    // so the orchestrator can locate `target/release/libtaida_rc26_phase1_fixture.<ext>`.
    write_addon_toml(&project_dir, "tester/rc26", "taida_rc26_phase1_fixture");

    // .gitignore — keep target/ out of the initial commit so the
    // worktree is clean after `cargo build --release --lib` runs.
    fs::write(project_dir.join(".gitignore"), "target/\nCargo.lock\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--target", "rust-addon"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .env("TAIDA_PUBLISH_SKIP_RELEASE", "1")
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "full addon publish should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );

    // Orchestrator output lines (one per stage).
    assert!(
        stdout.contains("[build]"),
        "stdout should include build step: {}",
        stdout
    );
    assert!(
        stdout.contains("[sha256]"),
        "stdout should include sha256 step: {}",
        stdout
    );
    assert!(
        stdout.contains("[lockfile]"),
        "stdout should include lockfile step: {}",
        stdout
    );
    assert!(
        stdout.contains("[rewrite]"),
        "stdout should include rewrite step: {}",
        stdout
    );
    assert!(
        stdout.contains("[release]  skipped (TAIDA_PUBLISH_SKIP_RELEASE=1)"),
        "stdout should announce skipped release: {}",
        stdout
    );
    // The package name in the `Published ...` line is derived from
    // the Manifest (packages.tdm) rather than native/addon.toml,
    // and defaults to the project directory name when packages.tdm
    // does not explicitly declare one. Our fixture project directory
    // is `addon-pkg`, so the CLI prints `alice/addon-pkg@a.1`. We
    // assert the version portion plus the prefix so the test is
    // resilient to future manifest-driven naming changes.
    assert!(
        stdout.contains("Published alice/") && stdout.contains("@a.1"),
        "stdout should report published version with @a.1: {}",
        stdout
    );

    // packages.tdm was rewritten to <<<@a.1 (or similar).
    let manifest = fs::read_to_string(project_dir.join("packages.tdm")).unwrap();
    assert!(
        manifest.contains("<<<@a.1"),
        "packages.tdm should be rewritten to a.1: {}",
        manifest
    );

    // native/addon.lock.toml now exists and contains a target row.
    let lock_path = project_dir.join("native").join("addon.lock.toml");
    assert!(
        lock_path.exists(),
        "native/addon.lock.toml should have been created"
    );
    let lock = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock.contains("[targets]"),
        "lockfile should contain [targets] section: {}",
        lock
    );
    assert!(
        lock.contains("sha256:"),
        "lockfile should contain a sha256: entry: {}",
        lock
    );

    // Git tag created locally and pushed to bare remote.
    let local_tags = git_output(&["tag", "--list"], &project_dir);
    assert!(
        local_tags.contains("a.1"),
        "local tag a.1 should exist: {}",
        local_tags
    );
    let remote_tags = git_output(&["tag", "--list"], &bare);
    assert!(
        remote_tags.contains("a.1"),
        "remote tag a.1 should exist: {}",
        remote_tags
    );

    // The commit message follows the pre-RC2.6 format.
    let log = git_output(&["log", "--oneline", "-1"], &project_dir);
    assert!(
        log.contains("publish:"),
        "commit message should begin with 'publish:': {}",
        log
    );

    // The commit should include BOTH packages.tdm and
    // native/addon.lock.toml — this closes RC2.6B-015's
    // "addon.toml not staged" bug.
    let show = git_output(&["show", "--stat", "HEAD"], &project_dir);
    assert!(
        show.contains("packages.tdm"),
        "HEAD commit should touch packages.tdm: {}",
        show
    );
    assert!(
        show.contains("native/addon.lock.toml"),
        "HEAD commit should touch native/addon.lock.toml: {}",
        show
    );

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 4: RC2.6-2c dry-run=plan explicit (parser compatibility)
// ──────────────────────────────────────────────────────────────

/// Verify that `--dry-run=plan` is accepted and behaves identically
/// to the bare `--dry-run` flag (backward compatibility). The test
/// is intentionally parallel to `test_publish_rust_addon_dry_run_auto_detect`
/// but uses the explicit `=plan` syntax.
#[test]
fn test_publish_rust_addon_dry_run_plan_explicit() {
    let root = unique_temp_dir("taida_publish_addon_plan");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").expect("write packages.tdm");
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").expect("write main.td");
    write_addon_toml(&project_dir, "tester/pkg", "tester_pkg");

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--dry-run=plan"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dry-run=plan should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    // Same assertions as the bare `--dry-run` test.
    assert!(
        stdout.contains("Dry run: no changes made."),
        "dry-run=plan should report no changes: {}",
        stdout
    );
    assert!(
        stdout.contains("Target: rust-addon"),
        "dry-run=plan should report rust-addon target: {}",
        stdout
    );
    assert!(
        stdout.contains("Cargo build: skipped"),
        "dry-run=plan must announce cargo build is skipped: {}",
        stdout
    );

    // No filesystem mutations.
    let manifest = fs::read_to_string(project_dir.join("packages.tdm")).expect("read packages.tdm");
    assert_eq!(manifest, "<<<@a @(run)\n");
    assert!(
        !project_dir.join("native").join("addon.lock.toml").exists(),
        "dry-run=plan must not create addon.lock.toml"
    );
    let tags = git_output(&["tag", "--list"], &project_dir);
    assert!(
        tags.is_empty(),
        "dry-run=plan should create no tags: {}",
        tags
    );

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 4: RC2.6-2c dry-run=build (build + lockfile, no git)
// ──────────────────────────────────────────────────────────────

/// Verify that `--dry-run=build` executes `cargo build --release --lib`
/// and merges the lockfile, but does NOT create a git commit, tag,
/// or push. The mutated files (packages.tdm, addon.lock.toml) are
/// left on disk for inspection.
#[test]
fn test_publish_rust_addon_dry_run_build() {
    if Command::new("cargo").arg("--version").output().is_err() {
        eprintln!("cargo not available; skipping test_publish_rust_addon_dry_run_build");
        return;
    }

    let root = unique_temp_dir("taida_publish_addon_drbuild");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    // packages.tdm + main.td.
    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    // Cargo.toml + src/lib.rs producing a cdylib.
    fs::write(project_dir.join("Cargo.toml"), MINIMAL_CDYLIB_CARGO_TOML).unwrap();
    fs::create_dir_all(project_dir.join("src")).unwrap();
    fs::write(
        project_dir.join("src").join("lib.rs"),
        MINIMAL_CDYLIB_LIB_RS,
    )
    .unwrap();

    // native/addon.toml.
    write_addon_toml(
        &project_dir,
        "tester/rc26-build",
        "taida_rc26_phase1_fixture",
    );

    // .gitignore.
    fs::write(project_dir.join(".gitignore"), "target/\nCargo.lock\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--target", "rust-addon", "--dry-run=build"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dry-run=build should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );

    // The build + lockfile steps should have run.
    assert!(
        stdout.contains("[build]"),
        "stdout should include build step: {}",
        stdout
    );
    assert!(
        stdout.contains("[sha256]"),
        "stdout should include sha256 step: {}",
        stdout
    );
    assert!(
        stdout.contains("[lockfile]"),
        "stdout should include lockfile step: {}",
        stdout
    );
    assert!(
        stdout.contains("[rewrite]"),
        "stdout should include rewrite step: {}",
        stdout
    );
    assert!(
        stdout.contains("Dry run (build)"),
        "stdout should announce dry-run build mode: {}",
        stdout
    );
    assert!(
        stdout.contains("git/release skipped"),
        "stdout should say git/release skipped: {}",
        stdout
    );

    // Lockfile should exist on disk (build was real).
    let lock_path = project_dir.join("native").join("addon.lock.toml");
    assert!(
        lock_path.exists(),
        "dry-run=build should create addon.lock.toml on disk"
    );
    let lock = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock.contains("[targets]") && lock.contains("sha256:"),
        "lockfile should contain targets + sha256: {}",
        lock
    );

    // packages.tdm should be rewritten.
    let manifest = fs::read_to_string(project_dir.join("packages.tdm")).unwrap();
    assert!(
        manifest.contains("<<<@a.1"),
        "packages.tdm should be rewritten: {}",
        manifest
    );

    // But NO git tag should exist (commit/push were skipped).
    let tags = git_output(&["tag", "--list"], &project_dir);
    assert!(
        tags.is_empty(),
        "dry-run=build should not create tags: {}",
        tags
    );

    // HEAD should still be the initial commit.
    let log = git_output(&["log", "--oneline"], &project_dir);
    assert!(
        log.lines().count() == 1,
        "dry-run=build should not add commits: {}",
        log
    );

    let _ = fs::remove_dir_all(&root);
}

// ──────────────────────────────────────────────────────────────
// Layer 4: RC2.6-2c invalid --dry-run mode is rejected
// ──────────────────────────────────────────────────────────────

#[test]
fn test_publish_invalid_dry_run_mode_rejected() {
    let root = unique_temp_dir("taida_publish_bad_dryrun");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a @(run)\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = write_fake_auth(&root, "alice");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--dry-run=commit"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(!output.status.success(), "unknown dry-run mode should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown --dry-run mode") && stderr.contains("commit"),
        "error should mention the invalid mode: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}
