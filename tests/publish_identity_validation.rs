//! C14-1 identity validation tests.
//!
//! `taida ingot publish` requires the manifest to declare a qualified
//! `owner/name` identity via `<<<@version owner/name`. Bare names (that
//! fall back to the directory name) must be rejected.

#![cfg(feature = "community")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
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

fn setup_with_manifest(root: &Path, pkg: &str, manifest: &str) -> PathBuf {
    let bare = root.join("remote.git");
    let project = root.join(pkg);
    fs::create_dir_all(&bare).unwrap();
    fs::create_dir_all(&project).unwrap();

    run_git(&["init", "--bare"], &bare);
    run_git(&["init"], &project);
    run_git(&["config", "user.email", "test@taida.dev"], &project);
    run_git(&["config", "user.name", "Test User"], &project);
    run_git(&["config", "init.defaultBranch", "main"], &project);

    let github_url = format!("https://github.com/alice/{}.git", pkg);
    run_git(&["remote", "add", "origin", &github_url], &project);
    run_git(
        &[
            "config",
            &format!("url.{}.pushInsteadOf", bare.to_str().unwrap()),
            &github_url,
        ],
        &project,
    );

    fs::write(project.join("packages.tdm"), manifest).unwrap();
    fs::write(project.join("main.td"), "stdout(\"ok\")\n").unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "initial"], &project);
    run_git(&["branch", "-M", "main"], &project);
    run_git(&["push", "-u", "origin", "main"], &project);

    project
}

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

#[test]
fn bare_identity_is_rejected() {
    // No `owner/name` in the <<< line → Manifest::name falls back to
    // the directory basename. plan_publish must reject this.
    let root = unique_temp_dir("taida_ident_bare");
    let project = setup_with_manifest(&root, "demo-pkg", "<<<@a.1\n");

    let output = Command::new(taida_bin())
        .args(["ingot", "publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        !output.status.success(),
        "publish must reject a bare identity"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not qualified"),
        "stderr should explain why; got: {}",
        stderr
    );
}

#[test]
fn qualified_identity_is_accepted() {
    // A proper `<<<@a.1 alice/demo-pkg` line passes identity
    // validation. The initial-publish path with a matching origin
    // yields next version = a.1.
    let root = unique_temp_dir("taida_ident_qualified");
    let project = setup_with_manifest(&root, "demo-pkg", "<<<@a.1 alice/demo-pkg\n");

    let output = Command::new(taida_bin())
        .args(["ingot", "publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "qualified identity must pass.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(stdout.contains("Publish plan for alice/demo-pkg:"));
    assert!(stdout.contains("Next version: a.1"));
}

#[test]
fn identity_mismatch_with_remote_is_rejected() {
    // Manifest claims `alice/demo-pkg` but origin points at
    // `someone-else/demo-pkg` — must be rejected so that `taida
    // install` cannot be tricked into the wrong URL.
    let root = unique_temp_dir("taida_ident_mismatch");
    let bare = root.join("remote.git");
    let project = root.join("demo-pkg");
    fs::create_dir_all(&bare).unwrap();
    fs::create_dir_all(&project).unwrap();
    run_git(&["init", "--bare"], &bare);
    run_git(&["init"], &project);
    run_git(&["config", "user.email", "test@taida.dev"], &project);
    run_git(&["config", "user.name", "Test User"], &project);
    run_git(&["config", "init.defaultBranch", "main"], &project);
    let wrong_url = "https://github.com/someone-else/demo-pkg.git";
    run_git(&["remote", "add", "origin", wrong_url], &project);
    run_git(
        &[
            "config",
            &format!("url.{}.pushInsteadOf", bare.to_str().unwrap()),
            wrong_url,
        ],
        &project,
    );

    fs::write(project.join("packages.tdm"), "<<<@a.1 alice/demo-pkg\n").unwrap();
    fs::write(project.join("main.td"), "stdout(\"ok\")\n").unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "initial"], &project);
    run_git(&["branch", "-M", "main"], &project);
    run_git(&["push", "-u", "origin", "main"], &project);

    let out = Command::new(taida_bin())
        .args(["ingot", "publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success(), "mismatched remote must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not match git remote"),
        "stderr should mention the mismatch: {}",
        stderr
    );
}

#[test]
fn non_github_remote_is_rejected() {
    let root = unique_temp_dir("taida_ident_nongh");
    let bare = root.join("remote.git");
    let project = root.join("demo-pkg");
    fs::create_dir_all(&bare).unwrap();
    fs::create_dir_all(&project).unwrap();
    run_git(&["init", "--bare"], &bare);
    run_git(&["init"], &project);
    run_git(&["config", "user.email", "test@taida.dev"], &project);
    run_git(&["config", "user.name", "Test User"], &project);
    run_git(&["config", "init.defaultBranch", "main"], &project);
    let gitlab_url = "https://gitlab.com/alice/demo-pkg.git";
    run_git(&["remote", "add", "origin", gitlab_url], &project);
    run_git(
        &[
            "config",
            &format!("url.{}.pushInsteadOf", bare.to_str().unwrap()),
            gitlab_url,
        ],
        &project,
    );

    fs::write(project.join("packages.tdm"), "<<<@a.1 alice/demo-pkg\n").unwrap();
    fs::write(project.join("main.td"), "stdout(\"ok\")\n").unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "initial"], &project);
    run_git(&["branch", "-M", "main"], &project);
    run_git(&["push", "-u", "origin", "main"], &project);

    let out = Command::new(taida_bin())
        .args(["ingot", "publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success(), "non-GitHub remote must be rejected");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a GitHub URL"));
}
