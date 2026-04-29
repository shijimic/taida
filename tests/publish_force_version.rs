//! C14-1: `--force-version` override tests.
//!
//! The auto-detected next version comes from the API diff. `--force-version`
//! bypasses the diff and uses the user-supplied value instead.
//! `--label` may be layered on top of `--force-version`.

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
    assert!(output.status.success(), "git {:?} failed", args);
}

fn setup_repo(root: &Path, pkg: &str) -> PathBuf {
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
    project
}

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

/// C26B-025: rewrite `packages.tdm` self-identity to match the
/// version about to be published, then amend the initial commit so
/// the working tree stays clean for `taida ingot publish`.
fn bump_manifest_to(project: &Path, pkg: &str, version: &str) {
    fs::write(
        project.join("packages.tdm"),
        format!("<<<@{} alice/{}\n", version, pkg),
    )
    .unwrap();
    run_git(&["add", "packages.tdm"], project);
    run_git(&["commit", "--amend", "--no-edit"], project);
    // Re-push the amended main so the bare remote agrees.
    run_git(&["push", "-f", "origin", "main"], project);
}

#[test]
fn force_version_overrides_auto_bump() {
    let root = unique_temp_dir("force_version_simple");
    let project = setup_repo(&root, "demo-pkg");
    // C26B-025: manifest must match the tag. --force-version a.5
    // requires packages.tdm to declare <<<@a.5.
    bump_manifest_to(&project, "demo-pkg", "a.5");

    // Auto-detection would yield `a.1` (initial release). We override
    // to `a.5`.
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["ingot", "publish", "--dry-run", "--force-version", "a.5"])
        .current_dir(&project)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "dry-run with force-version failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("Next version: a.5"),
        "force-version should override; got: {}",
        stdout
    );
    assert!(stdout.contains("Tag to push: a.5"));
}

#[test]
fn force_version_combined_with_label() {
    let root = unique_temp_dir("force_version_label");
    let project = setup_repo(&root, "demo-pkg");
    // C26B-025: Manifest declares the stable base version `a.5`;
    // `--label rc` attaches the label so the tag becomes `a.5.rc`.
    // This is the supported "label addendum" form — manifest does
    // not need to encode the label.
    bump_manifest_to(&project, "demo-pkg", "a.5");

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args([
            "ingot",
            "publish",
            "--dry-run",
            "--force-version",
            "a.5",
            "--label",
            "rc",
        ])
        .current_dir(&project)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Next version: a.5.rc"),
        "label must layer onto force-version; got: {}",
        stdout
    );
}

#[test]
fn force_version_rejects_non_taida_version() {
    let root = unique_temp_dir("force_version_bad");
    let project = setup_repo(&root, "demo-pkg");

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["ingot", "publish", "--dry-run", "--force-version", "1.0.0"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success(), "semver must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is not a valid Taida version"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn force_version_actually_pushes_the_forced_tag() {
    // End-to-end: --force-version a.7 → tag a.7 appears on remote.
    let root = unique_temp_dir("force_version_e2e");
    let bare = root.join("remote.git");
    let project = setup_repo(&root, "demo-pkg");
    // C26B-025: manifest must agree with the tag.
    bump_manifest_to(&project, "demo-pkg", "a.7");

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["ingot", "publish", "--force-version", "a.7"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "publish failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let tags = Command::new("git")
        .args(["tag", "--list"])
        .current_dir(&bare)
        .output()
        .unwrap();
    let tags_str = String::from_utf8_lossy(&tags.stdout);
    assert!(
        tags_str.lines().any(|l| l == "a.7"),
        "remote tags missing 'a.7': {}",
        tags_str
    );
}

/// C26B-025: publish must refuse when `packages.tdm` self-identity
/// disagrees with the tag about to be pushed. This is the primary
/// regression guard for the terminal `@a.7` incident.
#[test]
fn c26b_025_publish_rejects_stale_manifest_self_identity() {
    let root = unique_temp_dir("c26b_025_stale_manifest");
    let project = setup_repo(&root, "demo-pkg");
    // Manifest stays at <<<@a.1 but we try to publish as a.7 —
    // mismatched base version (not a label addendum).
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["ingot", "publish", "--dry-run", "--force-version", "a.7"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        !out.status.success(),
        "publish must reject when manifest self-identity is stale"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("packages.tdm self-identity")
            && stderr.contains("<<<@a.1")
            && stderr.contains("'a.7'"),
        "stderr must pinpoint both current manifest version and desired tag: {}",
        stderr
    );
    assert!(
        stderr.contains("Bump"),
        "stderr must instruct operator to bump manifest: {}",
        stderr
    );
}

/// C26B-025: label addendum ("manifest a.5" + "--label rc" = tag
/// "a.5.rc") is a legitimate match and must NOT be rejected. This
/// keeps the common RC workflow ergonomic.
#[test]
fn c26b_025_publish_accepts_label_addendum() {
    let root = unique_temp_dir("c26b_025_label_ok");
    let project = setup_repo(&root, "demo-pkg");
    bump_manifest_to(&project, "demo-pkg", "a.5");
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args([
            "ingot",
            "publish",
            "--dry-run",
            "--force-version",
            "a.5",
            "--label",
            "rc1",
        ])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "label addendum must pass:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Tag to push: a.5.rc1"));
}

#[test]
fn missing_force_version_value_errors() {
    let root = unique_temp_dir("force_version_missing");
    let project = setup_repo(&root, "demo-pkg");

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["ingot", "publish", "--force-version"])
        .current_dir(&project)
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing value for --force-version"));
}
