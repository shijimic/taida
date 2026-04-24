//! C14-1: `taida publish` tag-push-only integration tests.
//!
//! These tests pin the new CLI contract that `taida publish` is a
//! tag-only command:
//!
//! - `--dry-run` prints a deterministic plan and makes no git changes.
//! - Real publish creates exactly one tag on `origin` and pushes it —
//!   no commit on `main`, no `gh release create`.
//! - `taida publish` exits immediately after the tag push (does not
//!   wait for CI).
//!
//! The tests use a bare local repo accessed via `insteadOf` so
//! they do not touch any real GitHub remote.

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

/// Set up a bare remote + working project whose `origin` URL looks like
/// GitHub but whose push traffic is redirected to the local bare repo.
fn setup_project_with_remote(root: &Path, pkg: &str) -> (PathBuf, PathBuf) {
    let bare = root.join("remote.git");
    let project = root.join(pkg);
    fs::create_dir_all(&bare).expect("create bare dir");
    fs::create_dir_all(&project).expect("create project dir");

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

    (project, bare)
}

fn write_initial_commit(project: &Path, pkg_identity: &str) {
    fs::write(
        project.join("packages.tdm"),
        format!("<<<@a.1 alice/{}\n", pkg_identity),
    )
    .unwrap();
    fs::write(project.join("main.td"), "stdout(\"ok\")\n").unwrap();
    let taida_dir = project.join("taida");
    fs::create_dir_all(&taida_dir).unwrap();
    fs::write(taida_dir.join("lib.td"), "hello <= 1\n<<< @(hello)\n").unwrap();
    run_git(&["add", "."], project);
    run_git(&["commit", "-m", "initial"], project);
    run_git(&["branch", "-M", "main"], project);
    run_git(&["push", "-u", "origin", "main"], project);
}

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

// ───────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────

#[test]
fn dry_run_prints_plan_and_makes_no_git_changes() {
    let root = unique_temp_dir("taida_publish_dryrun");
    let (project, _bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    let head_before = git_output(&["rev-parse", "HEAD"], &project);
    let tags_before = git_output(&["tag", "--list"], &project);

    let output = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("taida publish --dry-run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "publish --dry-run must succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // Plan printout pins — order and wording are part of the contract.
    assert!(
        stdout.contains("Publish plan for alice/demo-pkg:"),
        "plan header missing; got: {}",
        stdout
    );
    assert!(
        stdout.contains("Last release tag: none"),
        "initial publish must say 'none'; got: {}",
        stdout
    );
    assert!(stdout.contains("API diff: initial"), "got: {}", stdout);
    assert!(stdout.contains("Next version: a.1"), "got: {}", stdout);
    assert!(stdout.contains("Tag to push: a.1"), "got: {}", stdout);
    assert!(stdout.contains("Remote: origin"), "got: {}", stdout);
    assert!(
        stdout.contains("Dry-run: no git changes performed."),
        "got: {}",
        stdout
    );

    // No side-effects.
    let head_after = git_output(&["rev-parse", "HEAD"], &project);
    let tags_after = git_output(&["tag", "--list"], &project);
    assert_eq!(head_before, head_after, "dry-run must not change HEAD");
    assert_eq!(tags_before, tags_after, "dry-run must not create tags");
}

#[test]
fn real_publish_pushes_tag_and_exits() {
    let root = unique_temp_dir("taida_publish_real");
    let (project, bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    let head_before = git_output(&["rev-parse", "HEAD"], &project);

    let output = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish"])
        .current_dir(&project)
        .output()
        .expect("taida publish");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "publish must succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // Report.
    assert!(
        stdout.contains("Pushed tag a.1 for alice/demo-pkg"),
        "missing tag-push report; stdout: {}",
        stdout
    );
    assert!(
        stdout.contains("CI (`.github/workflows/release.yml`)"),
        "must point at CI; stdout: {}",
        stdout
    );

    // Invariants:
    //  1. HEAD on `main` unchanged — publish must not commit.
    //  2. `a.1` tag exists locally.
    //  3. `a.1` tag exists on the bare remote.
    let head_after = git_output(&["rev-parse", "HEAD"], &project);
    assert_eq!(head_before, head_after, "publish must not create commits");

    let local_tags = git_output(&["tag", "--list"], &project);
    assert!(
        local_tags.contains("a.1"),
        "local tag missing: {}",
        local_tags
    );

    let remote_tags = git_output(&["tag", "--list"], &bare);
    assert!(
        remote_tags.contains("a.1"),
        "remote tag missing: {}",
        remote_tags
    );
}

#[test]
fn real_publish_does_not_push_main() {
    // Regression gate: the old `taida publish` used `git push origin
    // HEAD --follow-tags` which would be blocked by protected `main`.
    // The new flow only pushes `refs/tags/<tag>`, so HEAD on the bare
    // remote must be whatever main was before publish, unchanged.
    let root = unique_temp_dir("taida_publish_no_main_push");
    let (project, bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    let remote_main_before = git_output(&["rev-parse", "refs/heads/main"], &bare);

    let output = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish"])
        .current_dir(&project)
        .output()
        .expect("taida publish");
    assert!(
        output.status.success(),
        "publish failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let remote_main_after = git_output(&["rev-parse", "refs/heads/main"], &bare);
    assert_eq!(
        remote_main_before, remote_main_after,
        "publish must not move main on the remote"
    );
}

#[test]
fn removed_cli_flags_are_rejected_with_migration_hint() {
    // C14 removed --target, --dry-run=plan, --dry-run=build. Users
    // coming from C13 should get an actionable error, not silent
    // acceptance.
    let root = unique_temp_dir("taida_publish_legacy_flags");
    let (project, _bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    // --target rust-addon
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--target", "rust-addon"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`--target` was removed in @c.14.rc1"),
        "stderr: {}",
        stderr
    );

    // --dry-run=plan
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run=plan"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`--dry-run=<mode>` was removed in @c.14.rc1"),
        "stderr: {}",
        stderr
    );

    // --dry-run=build
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run=build"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("`--dry-run=<mode>` was removed"));
}

#[test]
fn dirty_worktree_is_rejected() {
    let root = unique_temp_dir("taida_publish_dirty");
    let (project, _bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    // Introduce an uncommitted change.
    fs::write(project.join("main.td"), "stdout(\"dirty\")\n").unwrap();

    let output = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        !output.status.success(),
        "dirty worktree must be rejected even in dry-run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uncommitted changes"),
        "stderr should explain why: {}",
        stderr
    );
}

#[test]
fn second_publish_uses_api_diff_for_next_version() {
    // First publish produces a.1 (initial). A subsequent commit that
    // adds a new export should be classified Additive → a.2.
    let root = unique_temp_dir("taida_publish_api_diff_next");
    let (project, _bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    // Round 1: initial publish.
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish"])
        .current_dir(&project)
        .output()
        .expect("publish 1");
    assert!(
        out.status.success(),
        "publish 1 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Round 2: add a new export symbol, commit, push main, dry-run.
    fs::write(
        project.join("taida").join("lib.td"),
        "hello <= 1\ngreet <= 2\n<<< @(hello, greet)\n",
    )
    .unwrap();
    // C26B-025: manifest self-identity must match the tag that will
    // be pushed. `--dry-run` still enforces the check.
    fs::write(
        project.join("packages.tdm"),
        "<<<@a.2 alice/demo-pkg\n",
    )
    .unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "add greet + bump manifest"], &project);
    run_git(&["push", "origin", "main"], &project);

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("publish 2 dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "publish 2 dry-run failed: {}", stdout);

    assert!(
        stdout.contains("Last release tag: a.1"),
        "should see previous tag; got: {}",
        stdout
    );
    assert!(stdout.contains("API diff: added 1"), "got: {}", stdout);
    assert!(stdout.contains("Next version: a.2"), "got: {}", stdout);
}

#[test]
fn breaking_change_bumps_generation() {
    let root = unique_temp_dir("taida_publish_breaking");
    let (project, _bare) = setup_project_with_remote(&root, "demo-pkg");
    write_initial_commit(&project, "demo-pkg");

    // Round 1.
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish"])
        .current_dir(&project)
        .output()
        .expect("publish 1");
    assert!(out.status.success());

    // Round 2: remove the existing export.
    fs::write(
        project.join("taida").join("lib.td"),
        "farewell <= 1\n<<< @(farewell)\n",
    )
    .unwrap();
    // C26B-025: breaking change bumps generation -> b.1, so manifest
    // must be bumped to match.
    fs::write(
        project.join("packages.tdm"),
        "<<<@b.1 alice/demo-pkg\n",
    )
    .unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "rename hello -> farewell + bump manifest"], &project);
    run_git(&["push", "origin", "main"], &project);

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run"])
        .current_dir(&project)
        .output()
        .expect("publish 2 dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());

    assert!(stdout.contains("API diff: removed 1"), "got: {}", stdout);
    // hello was removed, farewell added; because removed is non-empty,
    // classification is Breaking -> generation bump.
    assert!(stdout.contains("Next version: b.1"), "got: {}", stdout);
}
