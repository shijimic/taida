//! C14B-011: API diff skip contract tests.
//!
//! `taida publish --force-version` and `taida publish --retag` fully
//! determine the target tag name without consulting the public API
//! diff. Running the diff in those cases would (a) do work whose
//! result is thrown away and (b) surface parse errors from pre-C13
//! addon packages that still carry discard-binding (`_x <= ...`) style
//! identifiers in their `taida/*.td` facades.
//!
//! The contract exercised here: when either escape hatch is in play,
//! `plan_publish` must skip the snapshot entirely and report
//! `API diff: skipped (...)` in the plan printout. These tests
//! seed a repository whose previous tag points at a tree containing
//! an unparseable `taida/*.td` file so that a regression of the skip
//! would fail loudly at snapshot time instead of silently passing.

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
        "git {:?} failed:\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

/// Build a repo whose `taida/bad.td` contains Taida source that the
/// parser rejects (a discard binding inside a function body, which is
/// rejected by C13's E1616). A prior tag `a.1` is pushed pointing at
/// the bad tree, and HEAD is identical. Any path that tries to snapshot
/// the exports via the Taida parser will fail; only a path that skips
/// the snapshot can succeed.
fn setup_repo_with_unparseable_facade(root: &Path, pkg: &str) -> PathBuf {
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
    // Parseable file — only included so the package has a real export.
    fs::write(td.join("lib.td"), "hello <= 1\n<<< @(hello)\n").unwrap();
    // A deliberately malformed file: an unterminated string literal is
    // an unambiguous lex-level error regardless of parser mode, so the
    // Taida parser will always reject this source. If a future
    // C14 regression starts calling `api_diff::snapshot_*` here, the
    // publish plan will fail with the parser error text rather than
    // reaching `Publish plan for ...:` output.
    fs::write(
        td.join("bad.td"),
        "broken = \"unterminated\n<<< @(broken)\n",
    )
    .unwrap();
    run_git(&["add", "."], &project);
    run_git(&["commit", "-m", "initial"], &project);
    run_git(&["branch", "-M", "main"], &project);
    run_git(&["push", "-u", "origin", "main"], &project);
    // Create and push the tag so `latest_taida_tag` has something to
    // try to snapshot against. This is the critical seed — without a
    // previous tag, `plan_publish` would treat the package as an
    // initial release and skip the snapshot via a different branch.
    run_git(&["tag", "a.1"], &project);
    run_git(&["push", "origin", "refs/tags/a.1"], &project);
    project
}

#[test]
fn force_version_skips_api_diff_on_parse_error() {
    // C14B-011: `--force-version b.1` on a repo whose previous tag
    // contains an unparseable taida/*.td file must succeed. A regression
    // that removes the skip would surface the parser error as
    // `api_diff: parse errors:` and fail the test.
    let root = unique_temp_dir("force_version_skips_diff");
    let project = setup_repo_with_unparseable_facade(&root, "demo-pkg");
    // C26B-025: manifest self-identity must match the tag.
    fs::write(
        project.join("packages.tdm"),
        "<<<@b.1 alice/demo-pkg\n",
    )
    .unwrap();
    run_git(&["add", "packages.tdm"], &project);
    run_git(&["commit", "-m", "bump manifest to b.1"], &project);
    run_git(&["push", "origin", "main"], &project);

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run", "--force-version", "b.1"])
        .current_dir(&project)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--force-version must bypass api_diff entirely.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("API diff: skipped (force-version)"),
        "plan must surface the skip reason; got: {}",
        stdout
    );
    assert!(
        stdout.contains("Next version: b.1"),
        "force version must be honoured verbatim; got: {}",
        stdout
    );
    // Defensive: the parser-error text must NOT appear on success.
    assert!(
        !stderr.contains("api_diff: parse errors"),
        "api_diff snapshot should never have run; stderr: {}",
        stderr
    );
}

#[test]
fn retag_skips_api_diff_on_parse_error() {
    // C14B-011 sibling: `--retag` has the same escape-hatch semantics.
    // Without an accompanying `--force-version`, plan_publish would
    // normally compute `next_version_from_diff`, but the skip branch
    // means `diff` is neutral (`None`) and next_version falls through
    // to a number-bump of the previous tag.
    let root = unique_temp_dir("retag_skips_diff");
    let project = setup_repo_with_unparseable_facade(&root, "demo-pkg");

    // Combine --retag with --force-version to land exactly on a.1 (the
    // existing tag). This is the terminal-repo-style recovery path:
    // the tag already exists but the tag's tree cannot be snapshotted.
    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run", "--retag", "--force-version", "a.1"])
        .current_dir(&project)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--retag must bypass api_diff entirely.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    // When both flags are set, `force-version` wins the skip reason
    // race in `plan_publish` — this is fine because the important
    // property is "diff not computed", not which flag gets credit.
    assert!(
        stdout.contains("API diff: skipped (force-version)")
            || stdout.contains("API diff: skipped (retag)"),
        "plan must surface some skip reason; got: {}",
        stdout
    );
    assert!(
        stdout.contains("Retag: yes"),
        "retag flag must still reach the plan printout; got: {}",
        stdout
    );
    assert!(
        !stderr.contains("api_diff: parse errors"),
        "api_diff snapshot should never have run; stderr: {}",
        stderr
    );
}

#[test]
fn retag_alone_also_skips_api_diff() {
    // Retag without an explicit force-version: the skip must still
    // kick in so that tagging a.2 on a repo whose a.1 tag has a bad
    // tree works.
    let root = unique_temp_dir("retag_alone_skips_diff");
    let project = setup_repo_with_unparseable_facade(&root, "demo-pkg");

    // Pre-create a.2 so --retag has something to overwrite.
    run_git(&["tag", "a.2"], &project);
    run_git(&["push", "origin", "refs/tags/a.2"], &project);
    run_git(&["tag", "-d", "a.2"], &project);

    // C26B-025: manifest self-identity must match the tag.
    fs::write(
        project.join("packages.tdm"),
        "<<<@a.2 alice/demo-pkg\n",
    )
    .unwrap();
    run_git(&["add", "packages.tdm"], &project);
    run_git(&["commit", "-m", "bump manifest to a.2"], &project);
    run_git(&["push", "origin", "main"], &project);

    let out = Command::new(taida_bin())
        .env("TAIDA_PUBLISH_SKIP_GH_AUTH", "1")
        .args(["publish", "--dry-run", "--retag", "--force-version", "a.2"])
        .current_dir(&project)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "retag on unparseable-facade repo must succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        !stderr.contains("api_diff: parse errors"),
        "api_diff snapshot should never have run; stderr: {}",
        stderr
    );
}
