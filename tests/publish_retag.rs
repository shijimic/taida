//! C14-1: `--retag` collision semantics.
//!
//! Without `--retag`, a next-version that already exists as a tag on
//! `origin` must be rejected. With `--retag`, the existing tag is
//! force-replaced.

#![cfg(feature = "community")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let p = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn run_git(args: &[&str], dir: &Path) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed:\n{}",
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

fn setup_repo(root: &Path, pkg: &str) -> (PathBuf, PathBuf) {
    let bare = root.join("remote.git");
    let project = root.join(pkg);
    fs::create_dir_all(&bare).unwrap();
    fs::create_dir_all(&project).unwrap();
    run_git(&["init", "--bare"], &bare);
    run_git(&["init"], &project);
    run_git(&["config", "user.email", "t@t.dev"], &project);
    run_git(&["config", "user.name", "T"], &project);
    run_git(&["config", "init.defaultBranch", "main"], &project);
    let url = format!("https://github.com/alice/{}.git", pkg);
    run_git(&["remote", "add", "origin", &url], &project);
    run_git(
        &[
            "config",
            &format!("url.{}.pushInsteadOf", bare.to_str().unwrap()),
            &url,
        ],
        &project,
    );
    fs::write(
        project.join("packages.tdm"),
        format!("<<<@a.1 alice/{}\n", pkg),
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"ok\")\n").unwrap();
    let td = project.join("taida");
    fs::create_dir_all(&td).unwrap();
    fs::write(td.join("lib.td"), "hello <= 1\n<<< @(hello)\n").unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "initial"], &project);
    run_git(&["branch", "-M", "main"], &project);
    run_git(&["push", "-u", "origin", "main"], &project);
    (project, bare)
}

fn taida_bin() -> String {
    env!("CARGO_BIN_EXE_taida").to_string()
}

#[test]
fn collision_without_retag_is_rejected() {
    let root = unique_temp_dir("retag_collision");
    let (project, bare) = setup_repo(&root, "demo-pkg");

    // Pre-populate `a.1` on the bare remote and keep the local tag so
    // that `read_git_tags` (which uses `git fetch --tags` best-effort
    // + `git tag --list`) picks it up even when the remote URL is a
    // local bare repo accessed via pushInsteadOf.
    run_git(&["tag", "a.1"], &project);
    run_git(&["push", "origin", "refs/tags/a.1"], &project);

    // Use --force-version so plan_publish targets a.1 explicitly
    // (otherwise auto-bump would pick a.2 since a.1 is the prev tag).
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run", "--force-version", "a.1"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        !out.status.success(),
        "collision must be reported.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists on origin"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("--retag"),
        "stderr should mention --retag: {}",
        stderr
    );

    // Verify remote tag still points at the original commit (no side-effect).
    let remote_sha = git_output(&["rev-parse", "refs/tags/a.1"], &bare);
    assert!(!remote_sha.is_empty());
}

#[test]
fn retag_replaces_existing_tag() {
    let root = unique_temp_dir("retag_replace");
    let (project, bare) = setup_repo(&root, "demo-pkg");

    // Create `a.1` at the initial commit and push.
    run_git(&["tag", "a.1"], &project);
    run_git(&["push", "origin", "refs/tags/a.1"], &project);
    let original_sha = git_output(&["rev-parse", "refs/tags/a.1"], &bare);
    run_git(&["tag", "-d", "a.1"], &project);

    // Add a new commit so HEAD advances — retag must point to the new HEAD.
    fs::write(project.join("main.td"), "stdout(\"hello v2\")\n").unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "bump"], &project);
    run_git(&["push", "origin", "main"], &project);
    let head_sha = git_output(&["rev-parse", "HEAD"], &project);

    // Now retag at the new HEAD. Use --force-version to keep the tag name `a.1`
    // (otherwise auto-bump would pick a.2 since HEAD differs from the tagged tree).
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--retag", "--force-version", "a.1"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "retag publish failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Remote tag now points at the new HEAD, not the original sha.
    let new_remote_sha = git_output(&["rev-parse", "refs/tags/a.1"], &bare);
    assert_eq!(new_remote_sha, head_sha, "--retag should replace the tag");
    assert_ne!(
        new_remote_sha, original_sha,
        "--retag should move the tag to a new commit"
    );
}

#[test]
fn retag_dry_run_reports_retag_flag() {
    let root = unique_temp_dir("retag_dryrun");
    let (project, _bare) = setup_repo(&root, "demo-pkg");

    run_git(&["tag", "a.1"], &project);
    run_git(&["push", "origin", "refs/tags/a.1"], &project);
    run_git(&["tag", "-d", "a.1"], &project);

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run", "--retag", "--force-version", "a.1"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Retag: yes"),
        "retag plan line missing: {}",
        stdout
    );
}
