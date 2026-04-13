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
    let project = root.join("demo-pkg");
    fs::create_dir_all(&bare).expect("create bare dir");
    fs::create_dir_all(&project).expect("create project dir");

    // Create bare remote
    run_git(&["init", "--bare"], &bare);

    // Init project + add remote + set local git identity
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

#[test]
fn test_publish_commits_tags_and_pushes() {
    let root = unique_temp_dir("taida_publish_cli");
    let (project_dir, bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n")
        .expect("write packages.tdm");
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").expect("write main.td");

    // Initial commit so we have a branch to push
    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = root.join("fake-home");
    fs::create_dir_all(fake_home.join(".taida")).expect("create fake home");
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_test_token","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .expect("write auth.json");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .arg("publish")
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "taida publish should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("Published alice/demo-pkg@a.1"),
        "stdout: {}",
        stdout
    );
    assert!(stdout.contains("Tag: a.1"), "stdout: {}", stdout);
    assert!(
        stdout.contains("taida-community"),
        "should show proposals URL: {}",
        stdout
    );

    // Verify packages.tdm was updated
    let updated_manifest =
        fs::read_to_string(project_dir.join("packages.tdm")).expect("read updated packages.tdm");
    assert!(
        updated_manifest.contains("<<<@a.1"),
        "packages.tdm should be updated: {}",
        updated_manifest
    );

    // Verify git tag exists locally
    let tags = git_output(&["tag", "--list"], &project_dir);
    assert!(tags.contains("a.1"), "tag a.1 should exist: {}", tags);

    // Verify tag was pushed to remote
    let remote_tags = git_output(&["tag", "--list"], &bare);
    assert!(
        remote_tags.contains("a.1"),
        "tag a.1 should be in remote: {}",
        remote_tags
    );

    // Verify commit message
    let log = git_output(&["log", "--oneline", "-1"], &project_dir);
    assert!(
        log.contains("publish: alice/demo-pkg@a.1"),
        "commit message should contain publish info: {}",
        log
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_with_label() {
    let root = unique_temp_dir("taida_publish_label");
    let (project_dir, bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n")
        .expect("write packages.tdm");
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").expect("write main.td");

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = root.join("fake-home");
    fs::create_dir_all(fake_home.join(".taida")).expect("create fake home");
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_test","username":"bob","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .expect("write auth.json");

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--label", "alpha"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "publish with label should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(stdout.contains("@a.1.alpha"), "stdout: {}", stdout);
    assert!(stdout.contains("a.1.alpha"), "stdout: {}", stdout);

    let remote_tags = git_output(&["tag", "--list"], &bare);
    assert!(
        remote_tags.contains("a.1.alpha"),
        "remote should have tag: {}",
        remote_tags
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_second_version_increments_number() {
    let root = unique_temp_dir("taida_publish_incr");
    let (project_dir, bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = root.join("fake-home");
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_t","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    // First publish
    let output1 = Command::new(env!("CARGO_BIN_EXE_taida"))
        .arg("publish")
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("first publish");
    assert!(
        output1.status.success(),
        "first publish failed: {}",
        String::from_utf8_lossy(&output1.stderr)
    );

    // Make a change for second publish
    fs::write(project_dir.join("main.td"), "stdout(\"v2\")\n").unwrap();
    run_git(&["add", "main.td"], &project_dir);
    run_git(&["commit", "-m", "update"], &project_dir);
    run_git(&["push", "origin", "HEAD"], &project_dir);

    // Second publish
    let output2 = Command::new(env!("CARGO_BIN_EXE_taida"))
        .arg("publish")
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("second publish");

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(
        output2.status.success(),
        "second publish should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout2,
        stderr2
    );
    assert!(
        stdout2.contains("@a.2"),
        "second publish should be a.2: {}",
        stdout2
    );

    let remote_tags = git_output(&["tag", "--list"], &bare);
    assert!(remote_tags.contains("a.1"), "remote tags: {}", remote_tags);
    assert!(remote_tags.contains("a.2"), "remote tags: {}", remote_tags);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_dry_run_makes_no_changes() {
    let root = unique_temp_dir("taida_publish_dry");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    let fake_home = root.join("fake-home");
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_t","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .args(["publish", "--dry-run"])
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("dry run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Dry run"), "stdout: {}", stdout);

    // packages.tdm should NOT be modified
    let manifest = fs::read_to_string(project_dir.join("packages.tdm")).unwrap();
    assert_eq!(
        manifest, "<<<@a alice/demo-pkg\n",
        "dry-run should not modify packages.tdm"
    );

    // No tags should exist
    let tags = git_output(&["tag", "--list"], &project_dir);
    assert!(tags.is_empty(), "dry-run should create no tags: {}", tags);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_publish_rejects_dirty_worktree() {
    let root = unique_temp_dir("taida_publish_dirty");
    let (project_dir, _bare) = setup_project_with_remote(&root);

    fs::write(project_dir.join("packages.tdm"), "<<<@a alice/demo-pkg\n").unwrap();
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").unwrap();

    run_git(&["add", "."], &project_dir);
    run_git(&["commit", "-m", "initial"], &project_dir);
    run_git(&["push", "-u", "origin", "HEAD"], &project_dir);

    // Create an uncommitted change
    fs::write(project_dir.join("main.td"), "stdout(\"dirty\")\n").unwrap();

    let fake_home = root.join("fake-home");
    fs::create_dir_all(fake_home.join(".taida")).unwrap();
    fs::write(
        fake_home.join(".taida").join("auth.json"),
        r#"{"github_token":"gho_t","username":"alice","created_at":"2026-03-07T00:00:00Z"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_taida"))
        .arg("publish")
        .current_dir(&project_dir)
        .env("HOME", &fake_home)
        .output()
        .expect("run taida publish");

    assert!(!output.status.success(), "should fail with dirty worktree");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uncommitted changes"),
        "should mention uncommitted changes: {}",
        stderr
    );

    // packages.tdm should NOT be modified
    let manifest = fs::read_to_string(project_dir.join("packages.tdm")).unwrap();
    assert_eq!(
        manifest, "<<<@a alice/demo-pkg\n",
        "dirty worktree should not modify packages.tdm"
    );

    let _ = fs::remove_dir_all(&root);
}
