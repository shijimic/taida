//! C14-2a: 5-pattern coverage for the public API diff detector.
//!
//! The detector classifies two snapshots as `Initial`, `None`,
//! `Additive`, or `Breaking`. These tests drive the actual
//! `snapshot_head` / `snapshot_at_tag` / `detect` trio against tiny
//! real git repositories so the git path is exercised end-to-end.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use taida::pkg::api_diff::{self, ApiDiff};

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

/// Initialise a local git repo at `path` with one commit + tag.
fn init_repo_with_tag(path: &Path, tag: &str, td_contents: &str) {
    fs::create_dir_all(path).unwrap();
    run_git(&["init"], path);
    run_git(&["config", "user.email", "t@t.dev"], path);
    run_git(&["config", "user.name", "T"], path);
    run_git(&["config", "init.defaultBranch", "main"], path);
    let taida_dir = path.join("taida");
    fs::create_dir_all(&taida_dir).unwrap();
    fs::write(taida_dir.join("lib.td"), td_contents).unwrap();
    run_git(&["add", "."], path);
    run_git(&["commit", "-m", "initial"], path);
    run_git(&["branch", "-M", "main"], path);
    run_git(&["tag", tag], path);
}

fn replace_taida_lib(path: &Path, td_contents: &str, commit_msg: &str) {
    fs::write(path.join("taida").join("lib.td"), td_contents).unwrap();
    run_git(&["add", "."], path);
    run_git(&["commit", "-m", commit_msg], path);
}

// ───────────────────────────────────────────────────────────
// Snapshot tests
// ───────────────────────────────────────────────────────────

#[test]
fn snapshot_head_reads_multiple_td_files() {
    let tmp = unique_temp_dir("api_diff_head_multi");
    let taida_dir = tmp.join("taida");
    fs::create_dir_all(&taida_dir).unwrap();
    fs::write(taida_dir.join("a.td"), "foo <= 1\n<<< @(foo)\n").unwrap();
    fs::write(taida_dir.join("b.td"), "bar <= 2\n<<< @(bar)\n").unwrap();
    let snap = api_diff::snapshot_head(&tmp).expect("snapshot");
    let got: Vec<String> = snap.exports.iter().cloned().collect();
    assert_eq!(got, vec!["bar".to_string(), "foo".to_string()]);
}

#[test]
fn snapshot_at_tag_reads_from_git_history() {
    let tmp = unique_temp_dir("api_diff_tag_read");
    init_repo_with_tag(&tmp, "a.1", "foo <= 1\n<<< @(foo)\n");
    // Add more exports to working tree but not to the tag.
    replace_taida_lib(
        &tmp,
        "foo <= 1\nnewsym <= 2\n<<< @(foo, newsym)\n",
        "add newsym",
    );

    let tagged = api_diff::snapshot_at_tag(&tmp, "a.1").expect("snapshot_at_tag");
    assert_eq!(
        tagged.exports.iter().cloned().collect::<Vec<_>>(),
        vec!["foo".to_string()]
    );

    let head = api_diff::snapshot_head(&tmp).expect("snapshot_head");
    assert_eq!(
        head.exports.iter().cloned().collect::<Vec<_>>(),
        vec!["foo".to_string(), "newsym".to_string()]
    );
}

#[test]
fn snapshot_at_tag_nonexistent_is_error() {
    let tmp = unique_temp_dir("api_diff_tag_missing");
    init_repo_with_tag(&tmp, "a.1", "foo <= 1\n<<< @(foo)\n");
    let result = api_diff::snapshot_at_tag(&tmp, "nope");
    assert!(result.is_err(), "missing tag should return an error");
}

// ───────────────────────────────────────────────────────────
// 5-pattern detect() tests (Phase 2a contract)
// ───────────────────────────────────────────────────────────

#[test]
fn pattern_initial_release() {
    // `Initial` is the caller-provided case when there is no previous
    // tag. We cover it directly: the caller synthesises an empty prev
    // set. `detect` itself has no concept of `Initial` (that is
    // orchestrated at the publish layer — see
    // `publish::next_version_from_diff`). This test documents that
    // and proves that two empty snapshots compare as `None`.
    let prev = api_diff::PublicApiSnapshot::default();
    let next = api_diff::PublicApiSnapshot::default();
    assert_eq!(api_diff::detect(&prev, &next), ApiDiff::None);
}

#[test]
fn pattern_symbol_added_is_additive() {
    let tmp = unique_temp_dir("api_diff_added");
    init_repo_with_tag(&tmp, "a.1", "foo <= 1\n<<< @(foo)\n");
    replace_taida_lib(&tmp, "foo <= 1\nbar <= 2\n<<< @(foo, bar)\n", "add bar");

    let prev = api_diff::snapshot_at_tag(&tmp, "a.1").expect("prev snapshot");
    let next = api_diff::snapshot_head(&tmp).expect("next snapshot");
    let diff = api_diff::detect(&prev, &next);
    match diff {
        ApiDiff::Additive { added } => {
            assert_eq!(added, vec!["bar".to_string()]);
        }
        other => panic!("expected Additive, got {:?}", other),
    }
}

#[test]
fn pattern_symbol_removed_is_breaking() {
    let tmp = unique_temp_dir("api_diff_removed");
    init_repo_with_tag(&tmp, "a.1", "foo <= 1\nbar <= 2\n<<< @(foo, bar)\n");
    replace_taida_lib(&tmp, "foo <= 1\n<<< @(foo)\n", "remove bar");

    let prev = api_diff::snapshot_at_tag(&tmp, "a.1").expect("prev");
    let next = api_diff::snapshot_head(&tmp).expect("next");
    let diff = api_diff::detect(&prev, &next);
    match diff {
        ApiDiff::Breaking { removed, changed } => {
            assert_eq!(removed, vec!["bar".to_string()]);
            assert!(changed.is_empty(), "Phase 2a has no `changed` yet");
        }
        other => panic!("expected Breaking, got {:?}", other),
    }
}

#[test]
fn pattern_symbol_renamed_is_breaking() {
    let tmp = unique_temp_dir("api_diff_renamed");
    init_repo_with_tag(&tmp, "a.1", "foo <= 1\n<<< @(foo)\n");
    replace_taida_lib(&tmp, "foo2 <= 1\n<<< @(foo2)\n", "rename foo -> foo2");

    let prev = api_diff::snapshot_at_tag(&tmp, "a.1").expect("prev");
    let next = api_diff::snapshot_head(&tmp).expect("next");
    let diff = api_diff::detect(&prev, &next);
    match diff {
        ApiDiff::Breaking { removed, .. } => {
            assert_eq!(removed, vec!["foo".to_string()]);
        }
        other => panic!("rename must be classified Breaking, got {:?}", other),
    }
}

#[test]
fn pattern_no_change_is_none() {
    let tmp = unique_temp_dir("api_diff_nochange");
    init_repo_with_tag(&tmp, "a.1", "foo <= 1\n<<< @(foo)\n");
    // Commit a comment-only change that does not affect exports.
    replace_taida_lib(
        &tmp,
        "// new comment\nfoo <= 1\n<<< @(foo)\n",
        "comment only",
    );

    let prev = api_diff::snapshot_at_tag(&tmp, "a.1").expect("prev");
    let next = api_diff::snapshot_head(&tmp).expect("next");
    assert_eq!(api_diff::detect(&prev, &next), ApiDiff::None);
}

// ───────────────────────────────────────────────────────────
// Boundary / robustness
// ───────────────────────────────────────────────────────────

#[test]
fn missing_taida_dir_yields_empty_snapshot() {
    let tmp = unique_temp_dir("api_diff_no_dir");
    // No `taida/` directory at all.
    let snap = api_diff::snapshot_head(&tmp).expect("must not fail");
    assert!(snap.exports.is_empty());
}

#[test]
fn multiple_files_combine_exports() {
    let tmp = unique_temp_dir("api_diff_multi");
    let td = tmp.join("taida");
    fs::create_dir_all(&td).unwrap();
    fs::write(td.join("a.td"), "foo <= 1\n<<< @(foo)\n").unwrap();
    fs::write(td.join("b.td"), "bar <= 2\n<<< @(bar)\n").unwrap();
    fs::write(td.join("c.td"), "baz <= 3\n<<< @(baz)\n").unwrap();

    let snap = api_diff::snapshot_head(&tmp).expect("snapshot");
    let got: Vec<String> = snap.exports.iter().cloned().collect();
    assert_eq!(
        got,
        vec!["bar".to_string(), "baz".to_string(), "foo".to_string()]
    );
}
