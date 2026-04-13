//! Layer 2: Publish → Install round-trip test with local bare git remote.
//!
//! This test simulates the full cycle:
//! 1. Create a package project with packages.tdm + main.td
//! 2. `taida publish --label alpha` → commit, tag, push to local bare remote
//! 3. Package the project as tarball (simulating GitHub archive)
//! 4. Extract tarball with strip-components=1 (simulating store.rs fetch_and_cache)
//! 5. Verify the package is correctly extracted

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
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
        "git {:?} failed: {}",
        args,
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

fn setup_project_with_remote(root: &Path) -> (PathBuf, PathBuf) {
    let bare = root.join("remote.git");
    let project = root.join("demo-pkg");
    fs::create_dir_all(&bare).expect("create bare dir");
    fs::create_dir_all(&project).expect("create project dir");

    run_git(&["init", "--bare"], &bare);
    run_git(&["init"], &project);
    run_git(&["config", "user.email", "test@taida.dev"], &project);
    run_git(&["config", "user.name", "Test User"], &project);

    // Set origin to a GitHub-style URL so prepare_publish() accepts it.
    // pushInsteadOf redirects push traffic to the local bare repo while
    // git remote get-url origin still returns the GitHub URL.
    let github_url = "https://github.com/alice/demo-pkg.git";
    run_git(&["remote", "add", "origin", github_url], &project);
    run_git(
        &[
            "config",
            &format!("url.{}.pushInsteadOf", bare.to_str().unwrap()),
            github_url,
        ],
        &project,
    );

    (project, bare)
}

/// Create a tar.gz archive of `source_dir` with a top-level directory wrapper,
/// mimicking GitHub's archive format (strip-components=1 expected).
fn create_tarball(source_dir: &Path, output: &Path) {
    let parent = source_dir.parent().unwrap();
    let dir_name = source_dir.file_name().unwrap();
    let status = Command::new("tar")
        .args(["czf"])
        .arg(output)
        .arg("-C")
        .arg(parent)
        .arg(dir_name)
        .status()
        .expect("run tar");
    assert!(status.success(), "tar creation failed");
}

#[test]
fn test_publish_then_install_roundtrip() {
    let root = unique_temp_dir("taida_roundtrip");
    let (pkg_dir, bare) = setup_project_with_remote(&root);

    // ── Step 1: Create package project ──
    fs::write(pkg_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();
    fs::write(
        pkg_dir.join("main.td"),
        "greet name = stdout(\"Hello, \" + name + \"!\")\n",
    )
    .unwrap();

    run_git(&["add", "."], &pkg_dir);
    run_git(&["commit", "-m", "initial"], &pkg_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &pkg_dir);

    // ── Step 2: Setup fake auth ──
    let fake_home = root.join("home");
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_fake","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    // ── Step 3: Run taida publish --label alpha ──
    let publish_output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--label", "alpha"])
        .current_dir(&pkg_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let publish_stdout = String::from_utf8_lossy(&publish_output.stdout);
    let publish_stderr = String::from_utf8_lossy(&publish_output.stderr);
    assert!(
        publish_output.status.success(),
        "taida publish failed\nstdout: {}\nstderr: {}",
        publish_stdout,
        publish_stderr
    );
    assert!(
        publish_stdout.contains("@a.1.alpha"),
        "Expected version in output: {}",
        publish_stdout
    );

    // ── Step 4: Verify packages.tdm was updated ──
    let updated_manifest = fs::read_to_string(pkg_dir.join("packages.tdm")).unwrap();
    assert!(
        updated_manifest.contains("<<<@a.1.alpha"),
        "packages.tdm should contain updated version: {}",
        updated_manifest
    );

    // ── Step 5: Verify integrity hash is present in output ──
    assert!(
        publish_stdout.contains("fnv1a:"),
        "Expected integrity hash in output: {}",
        publish_stdout
    );

    // ── Step 6: Verify git tag exists in remote ──
    let remote_tags = git_output(&["tag", "--list"], &bare);
    assert!(
        remote_tags.contains("a.1.alpha"),
        "remote should have tag a.1.alpha: {}",
        remote_tags
    );

    // ── Step 7: Verify commit message ──
    let log = git_output(&["log", "--oneline", "-1"], &pkg_dir);
    assert!(
        log.contains("publish: alice/demo-pkg@a.1.alpha"),
        "commit message should contain publish info: {}",
        log
    );

    // ── Step 8: Verify proposals URL in output ──
    assert!(
        publish_stdout.contains("taida-community"),
        "should show proposals URL: {}",
        publish_stdout
    );

    // ── Step 9: Simulate install — create tarball and extract ──
    // (Full `taida install` E2E with mock server is in Layer 3.
    //  Here we verify the package files can be archived and extracted
    //  with strip-components=1, matching store.rs's fetch_and_cache behavior.)
    let tarball_source = root.join("demo-pkg-va.1.alpha");
    fs::create_dir_all(&tarball_source).unwrap();
    fs::write(tarball_source.join("packages.tdm"), &updated_manifest).unwrap();
    fs::write(
        tarball_source.join("main.td"),
        "greet name = stdout(\"Hello, \" + name + \"!\")\n",
    )
    .unwrap();
    let tarball_path = root.join("demo-pkg.tar.gz");
    create_tarball(&tarball_source, &tarball_path);

    let store_dir = root.join("store");
    let extract_dir = store_dir.join("alice").join("demo-pkg").join("a.1.alpha");
    fs::create_dir_all(&extract_dir).unwrap();
    let tar_status = Command::new("tar")
        .args(["xzf"])
        .arg(&tarball_path)
        .args(["--strip-components=1", "-C"])
        .arg(&extract_dir)
        .status()
        .unwrap();
    assert!(tar_status.success(), "tar extraction failed");
    assert!(
        extract_dir.join("main.td").exists(),
        "main.td should exist in extracted package"
    );
    assert!(
        extract_dir.join("packages.tdm").exists(),
        "packages.tdm should exist in extracted package"
    );
    let main_content = fs::read_to_string(extract_dir.join("main.td")).unwrap();
    assert!(
        main_content.contains("greet"),
        "main.td should contain greet function"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_dry_run_does_not_modify_anything() {
    let root = unique_temp_dir("taida_dryrun");
    let (pkg_dir, _bare) = setup_project_with_remote(&root);

    fs::write(pkg_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();
    fs::write(pkg_dir.join("main.td"), "stdout(1)\n").unwrap();

    run_git(&["add", "."], &pkg_dir);
    run_git(&["commit", "-m", "initial"], &pkg_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &pkg_dir);

    let fake_home = root.join("home");
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_fake","username":"bob","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--dry-run", "--label", "beta"])
        .current_dir(&pkg_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "dry-run should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Dry run"),
        "Expected dry-run output: {}",
        stdout
    );
    assert!(stdout.contains("@a.1.beta"), "Expected version: {}", stdout);
    assert!(stdout.contains("fnv1a:"), "Expected integrity: {}", stdout);

    // packages.tdm should NOT be modified
    let manifest = fs::read_to_string(pkg_dir.join("packages.tdm")).unwrap();
    assert_eq!(
        manifest, "<<<@a alice/demo-pkg\n",
        "dry-run should not modify packages.tdm"
    );

    // No tags should exist
    let tags = git_output(&["tag", "--list"], &pkg_dir);
    assert!(tags.is_empty(), "dry-run should create no tags: {}", tags);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_fails_without_auth() {
    let root = unique_temp_dir("taida_noauth");
    let pkg_dir = root.join("pkg");
    let fake_home = root.join("home");

    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    // No auth.json

    run_git(&["init"], &pkg_dir);
    fs::write(pkg_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish"])
        .current_dir(&pkg_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(!output.status.success(), "should fail without auth");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("taida auth login"),
        "should suggest login: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_fails_without_packages_td() {
    let root = unique_temp_dir("taida_nopkg");
    let empty_dir = root.join("empty");
    let fake_home = root.join("home");

    fs::create_dir_all(&empty_dir).unwrap();
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_fake","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish"])
        .current_dir(&empty_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(!output.status.success(), "should fail without packages.tdm");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("packages.tdm") || stderr.contains("taida init"),
        "should mention packages.tdm: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_fails_with_invalid_label() {
    let root = unique_temp_dir("taida_badlabel");
    let (pkg_dir, _bare) = setup_project_with_remote(&root);

    fs::write(pkg_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();
    fs::write(pkg_dir.join("main.td"), "stdout(1)\n").unwrap();

    run_git(&["add", "."], &pkg_dir);
    run_git(&["commit", "-m", "initial"], &pkg_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &pkg_dir);

    let fake_home = root.join("home");
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_fake","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--label", "UPPER"])
        .current_dir(&pkg_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(!output.status.success(), "should fail with invalid label");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid publish label"),
        "should report invalid label: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}
