//! C14-3g: verify that the scaffolded addon `release.yml` is
//! structurally symmetric with the Taida core `release.yml`.
//!
//! "Symmetric" means they share the job contract
//! `prepare -> gate -> build -> publish`, both use the
//! `github-actions[bot]` identity via `github.token`, and both accept
//! the Taida version tag scheme (addon side: bare, core side:
//! `@`-prefixed).
//!
//! This test reads the core workflow from the current repository at
//! `.github/workflows/release.yml` and compares its structural
//! anchors with a freshly scaffolded addon workflow. The test is
//! *not* a diff of the YAML bodies — the build matrix entries and
//! step names legitimately differ. It pins the shared contract only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "taida_init_symmetry_{}_{}_{}",
        prefix,
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn taida_bin() -> String {
    env!("CARGO_BIN_EXE_taida").to_string()
}

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at the crate root when running
    // integration tests.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Declared jobs (in order) under the top-level `jobs:` mapping.
fn declared_job_order(content: &str) -> Vec<String> {
    let mut in_jobs = false;
    let mut jobs = Vec::new();
    for line in content.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if in_jobs {
            if !line.is_empty() && !line.starts_with(' ') {
                break;
            }
            if let Some(rest) = line.strip_prefix("  ")
                && !rest.starts_with(' ')
                && let Some(name) = rest.strip_suffix(':')
                && !name.is_empty()
            {
                jobs.push(name.to_string());
            }
        }
    }
    jobs
}

fn scaffold_addon(root: &Path, name: &str) -> String {
    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon", name])
        .current_dir(root)
        .output()
        .expect("taida init must run");
    assert!(
        output.status.success(),
        "taida init failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = root.join(name).join(".github/workflows/release.yml");
    fs::read_to_string(&workflow).expect("scaffolded release.yml must be readable")
}

#[test]
fn test_jobs_match_core_contract() {
    let core = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("core release.yml must exist");
    let core_jobs = declared_job_order(&core);
    assert_eq!(
        core_jobs,
        vec!["prepare", "gate", "build", "publish"],
        "core release.yml must use the 4-stage contract (regression guard): {:?}",
        core_jobs
    );

    let tmp = unique_temp_dir("jobs");
    let addon = scaffold_addon(&tmp, "sym-pkg");
    let addon_jobs = declared_job_order(&addon);
    assert_eq!(
        addon_jobs, core_jobs,
        "addon release.yml must match the core job order: {:?} vs {:?}",
        addon_jobs, core_jobs
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_release_author_is_github_actions_bot() {
    let core = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("core release.yml must exist");
    assert!(
        core.contains("GH_TOKEN: ${{ github.token }}"),
        "core release.yml must authenticate `gh` with github.token"
    );

    let tmp = unique_temp_dir("bot");
    let addon = scaffold_addon(&tmp, "bot-pkg");
    assert!(
        addon.contains("GH_TOKEN: ${{ github.token }}"),
        "addon release.yml must authenticate `gh` with github.token to \
         keep release author = github-actions[bot]"
    );
    // Must not introduce a PAT shortcut.
    assert!(
        !addon.contains("secrets.GH_PAT"),
        "addon release.yml must not depend on a personal access token"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_tag_regex_shape_matches_core() {
    // Core tags are `@`-prefixed (`@c.14.rc1`). Addon tags drop the
    // `@` (`a.1.rc1`). Both share the same regex body:
    //   ^<generation>\.<number>(\.<label>)?$
    let core = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("core release.yml must exist");
    assert!(
        core.contains(r#"^@[a-z]\.[0-9]+(\.[a-z0-9][a-z0-9-]*)?$"#),
        "core release.yml must validate @-prefixed Taida version tags"
    );

    let tmp = unique_temp_dir("regex");
    let addon = scaffold_addon(&tmp, "tag-pkg");
    // Addon accepts both 1-letter and 2-letter generation regexes.
    assert!(
        addon.contains(r#"^[a-z]\.[0-9]+(\.[a-z0-9][a-z0-9-]*)?$"#),
        "addon release.yml must validate 1-letter generation tags"
    );
    assert!(
        addon.contains(r#"^[a-z][a-z]\.[0-9]+(\.[a-z0-9][a-z0-9-]*)?$"#),
        "addon release.yml must validate 2-letter generation tags"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_permissions_grant_contents_write() {
    let core = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("core release.yml must exist");
    // Core grants contents: read top-level and contents: write on
    // the publish job. Addons grant contents: write top-level (they
    // do not gate anything else on repo contents).
    assert!(
        core.contains("contents: write"),
        "core release.yml must grant contents: write somewhere"
    );

    let tmp = unique_temp_dir("perms");
    let addon = scaffold_addon(&tmp, "perm-pkg");
    assert!(
        addon.contains("contents: write"),
        "addon release.yml must grant contents: write for release asset upload"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_prepare_exposes_release_tag_and_ref() {
    // Both workflows must expose the same two outputs from `prepare`
    // so `gate` / `build` / `publish` can reference them uniformly.
    let tmp = unique_temp_dir("prep");
    let addon = scaffold_addon(&tmp, "prep-pkg");
    assert!(
        addon.contains("release_tag:") && addon.contains("release_ref:"),
        "addon prepare job must expose release_tag and release_ref outputs: {addon}"
    );

    let core = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("core release.yml must exist");
    assert!(
        core.contains("release_tag:") && core.contains("release_ref:"),
        "core prepare job must also expose release_tag and release_ref"
    );
    let _ = fs::remove_dir_all(&tmp);
}
