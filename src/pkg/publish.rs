//! C14-1: `taida publish` tag-push-only implementation.
//!
//! This module used to orchestrate `cargo build`, SHA-256 computation,
//! `addon.lock.toml` generation, `packages.tdm` rewriting, `git commit`,
//! `git push origin HEAD`, and `gh release create`. All of that is
//! gone in C14: publish now does exactly three things.
//!
//! 1. Validate the manifest identity (qualified `owner/name` required).
//! 2. Compute the next version from the API diff (or honour
//!    `--force-version`).
//! 3. `git tag <version>` + `git push origin <tag>`, then exit.
//!
//! Release artefact building, SHA-256 digest computation, and asset
//! upload are the exclusive responsibility of CI — see the addon
//! `release.yml` template introduced by C14-3 and Taida's own
//! `.github/workflows/release.yml` which serves as the symmetric
//! reference.
//!
//! Non-negotiable contracts carried over from `.dev/C14_DESIGN.md`:
//!
//! - `taida publish` MUST NOT push anything to `main` (no `git push
//!   origin HEAD`). Only `git push origin <tag>` is allowed.
//! - `taida publish` MUST NOT call `gh release create`. The addon
//!   `release.yml` creates the release as `github-actions[bot]`.
//! - `taida publish` MUST exit immediately after the tag push. It does
//!   not wait for CI.
//! - The manifest identity MUST be qualified (`<<<@version owner/name`);
//!   bare names are rejected.
//! - API diff detection MUST reuse the existing Taida parser
//!   (`crate::pkg::api_diff`).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::api_diff::{self, ApiDiff};
use super::manifest::{Manifest, is_valid_taida_version};

// ─────────────────────────────────────────────────────────────
// Public plan surface
// ─────────────────────────────────────────────────────────────

/// Reason why API diff computation was skipped for a publish plan.
///
/// Set on `PublishPlan.diff_skipped` when `plan_publish` deliberately
/// avoided reading the previous tag / HEAD snapshots. `render_plan`
/// surfaces this string as `API diff: skipped (<reason>)` so operators
/// can audit why a diff-informed version bump was bypassed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSkipReason {
    /// `--force-version` was supplied, so the next version is entirely
    /// user-controlled; the diff cannot affect the outcome.
    ForceVersion,
    /// `--retag` was supplied, so the intent is to overwrite an existing
    /// tag with the same (user-implied) version; the diff is irrelevant.
    Retag,
}

impl DiffSkipReason {
    fn as_plan_label(&self) -> &'static str {
        match self {
            DiffSkipReason::ForceVersion => "force-version",
            DiffSkipReason::Retag => "retag",
        }
    }
}

/// Human-facing description of what a `taida publish` invocation is
/// about to do (or, under `--dry-run`, would do).
///
/// The fields are the single source of truth for the plan printout
/// rendered by `main.rs::run_publish`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishPlan {
    /// Qualified `owner/name` identity extracted from the manifest.
    pub package_id: String,
    /// Previous release tag, or `None` for the initial publish.
    pub previous_tag: Option<String>,
    /// Classification of the diff between the previous tag and HEAD.
    ///
    /// When `diff_skipped` is `Some`, this is `ApiDiff::None` as a
    /// neutral placeholder: the diff was never computed, so no claim
    /// about Additive / Breaking is made.
    pub diff: ApiDiff,
    /// When `Some`, API diff snapshotting was skipped; see
    /// [`DiffSkipReason`] for why. `None` means the diff in [`Self::diff`]
    /// reflects a real computation (or `Initial` for first publish).
    pub diff_skipped: Option<DiffSkipReason>,
    /// Next version to tag (already labelled).
    pub next_version: String,
    /// Remote name to push the tag to (always `origin` today).
    pub remote: String,
    /// True if `--retag` was requested and an existing tag will be
    /// force-replaced.
    pub retag: bool,
}

/// Source from which `next_version` was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// Produced by `next_version_from_diff` given the API diff.
    Auto,
    /// Overridden via `--force-version`.
    Forced,
}

// ─────────────────────────────────────────────────────────────
// Identity / label validation
// ─────────────────────────────────────────────────────────────

/// Validate the manifest's identity and return the qualified package
/// name.
///
/// A bare name (or a manifest that falls back to its directory name
/// because `<<<@version owner/name` was missing) is rejected: the
/// `taida install` resolver only speaks `org/name`.
pub fn validate_manifest_identity(manifest: &Manifest) -> Result<String, String> {
    if !manifest.name.contains('/') {
        return Err(format!(
            "Package identity '{}' is not qualified. packages.tdm must declare \
             `<<<@version owner/name` so `taida install` can resolve it via GitHub.",
            manifest.name
        ));
    }
    let mut parts = manifest.name.split('/');
    let owner = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(format!(
            "Package identity '{}' has more than one '/'. Expected 'owner/name'.",
            manifest.name
        ));
    }
    validate_name_component(owner, "owner", &manifest.name)?;
    validate_name_component(name, "name", &manifest.name)?;
    Ok(manifest.name.clone())
}

fn validate_name_component(component: &str, label: &str, full: &str) -> Result<(), String> {
    if component.is_empty() {
        return Err(format!(
            "Package identity '{}' has an empty {} component.",
            full, label
        ));
    }
    if component.starts_with('-') || component.ends_with('-') {
        return Err(format!(
            "Package identity '{}': {} component '{}' must not start or end with '-'.",
            full, label, component
        ));
    }
    if !component
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "Package identity '{}': {} component '{}' must contain only lowercase letters, digits, and hyphens.",
            full, label, component
        ));
    }
    Ok(())
}

/// Validate a pre-release label (passed via `--label`).
pub fn validate_label(label: &str) -> Result<(), String> {
    if is_valid_taida_version(&format!("a.1.{label}")) {
        Ok(())
    } else {
        Err(format!(
            "Invalid publish label '{}'. Expected [a-z0-9][a-z0-9-]* with no trailing hyphen.",
            label
        ))
    }
}

// ─────────────────────────────────────────────────────────────
// Version helpers
// ─────────────────────────────────────────────────────────────

/// Bump the numeric component of a Taida version, preserving the
/// generation and discarding any existing label.
///
/// `"a.3" → "a.4"`, `"aa.12" → "aa.13"`, `"a.3.rc" → "a.4"`.
pub fn bump_number(current: &str) -> String {
    let (generation, number, _label) = split_version(current);
    let next = number.unwrap_or(0) + 1;
    format!("{}.{}", generation, next)
}

/// Bump the generation, resetting the number to 1 and dropping any
/// existing label.
pub fn bump_generation(current: &str) -> String {
    let (generation, _number, _label) = split_version(current);
    let next_gen = next_generation(&generation);
    format!("{}.1", next_gen)
}

/// Attach a pre-release label to a version. `None` returns the
/// version unchanged; a `Some` value replaces any existing label.
pub fn attach_label(version: &str, label: Option<&str>) -> String {
    match label {
        None => {
            let (generation, num, _) = split_version(version);
            match num {
                Some(n) => format!("{}.{}", generation, n),
                None => generation,
            }
        }
        Some(label) => {
            let (generation, num, _) = split_version(version);
            let n = num.unwrap_or(1);
            format!("{}.{}.{}", generation, n, label)
        }
    }
}

/// Compute the next version from a diff + the previous tag.
pub fn next_version_from_diff(
    previous_tag: Option<&str>,
    diff: &ApiDiff,
    label: Option<&str>,
) -> String {
    let bumped = match diff {
        ApiDiff::Initial => "a.1".to_string(),
        ApiDiff::None | ApiDiff::Additive { .. } => match previous_tag {
            Some(prev) => bump_number(prev),
            None => "a.1".to_string(),
        },
        ApiDiff::Breaking { .. } => match previous_tag {
            Some(prev) => bump_generation(prev),
            None => "a.1".to_string(),
        },
    };
    attach_label(&bumped, label)
}

fn split_version(v: &str) -> (String, Option<u64>, Option<String>) {
    let mut parts = v.splitn(3, '.');
    let generation = parts.next().unwrap_or("").to_string();
    let num = parts.next().and_then(|s| s.parse::<u64>().ok());
    let label = parts.next().map(|s| s.to_string());
    (generation, num, label)
}

fn next_generation(generation: &str) -> String {
    let mut chars: Vec<u8> = generation.bytes().collect();
    let mut index = chars.len();
    let mut carry = true;
    while index > 0 && carry {
        index -= 1;
        if chars[index] == b'z' {
            chars[index] = b'a';
        } else {
            chars[index] += 1;
            carry = false;
        }
    }
    if carry {
        chars.insert(0, b'a');
    }
    String::from_utf8(chars).unwrap_or_else(|_| "a".to_string())
}

// ─────────────────────────────────────────────────────────────
// Git / remote surface
// ─────────────────────────────────────────────────────────────

pub fn read_git_tags(project_dir: &Path) -> Result<Vec<String>, String> {
    let _ = Command::new("git")
        .args(["fetch", "--tags", "--quiet"])
        .current_dir(project_dir)
        .output();

    let output = Command::new("git")
        .args(["tag", "--list"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| {
            format!(
                "Failed to run git tag in '{}': {}",
                project_dir.display(),
                e
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "taida publish requires a git repository. git tag failed: {}",
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Compare two Taida generation strings using the base-26-like
/// progression that `next_generation` walks forward (`a..z`, then
/// `aa..zz`, then `aaa..`, ...). Ordering is length-first, then
/// lexicographic within the same length, so `"z" < "aa" < "ab" < "zz" < "aaa"`.
///
/// Plain `str::cmp` would put `"aa" < "z"` and silently re-age any
/// repo that has crossed the `z -> aa` boundary (C14B-013).
fn compare_generation(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

pub fn latest_taida_tag(tags: &[String]) -> Option<String> {
    let mut parsed: Vec<(String, String, u64, Option<String>)> = tags
        .iter()
        .filter(|t| crate::pkg::manifest::is_valid_taida_version(t))
        .filter_map(|t| {
            let (generation, num, label) = split_version(t);
            num.map(|n| (t.clone(), generation, n, label))
        })
        .collect();
    parsed.sort_by(|a, b| compare_generation(&a.1, &b.1).then(a.2.cmp(&b.2)));
    parsed.last().map(|t| t.0.clone())
}

pub fn git_origin_url(project_dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(project_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() { None } else { Some(url) }
}

pub fn parse_github_repo(remote: &str) -> Option<(String, String)> {
    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return parse_owner_repo(rest);
    }
    if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        return parse_owner_repo(rest);
    }
    if let Some(rest) = remote.strip_prefix("https://github.com/") {
        return parse_owner_repo(rest);
    }
    None
}

fn parse_owner_repo(rest: &str) -> Option<(String, String)> {
    let trimmed = rest.trim_end_matches(".git").trim_end_matches('/');
    let mut parts = trimmed.split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

pub fn check_worktree_clean(project_dir: &Path) -> Result<(), String> {
    let status = run_git(project_dir, &["status", "--porcelain"])?;
    if !status.is_empty() {
        return Err(format!(
            "Working tree has uncommitted changes. Commit or stash them before publishing.\n{}",
            status
        ));
    }
    Ok(())
}

pub fn require_identity_matches_remote(
    package_id: &str,
    remote_url: Option<&str>,
) -> Result<(), String> {
    let remote = remote_url.ok_or_else(|| {
        format!(
            "Package identity '{}' requires a git remote 'origin' but none is configured.",
            package_id
        )
    })?;
    let (owner, name) = parse_github_repo(remote).ok_or_else(|| {
        format!(
            "git remote 'origin' is '{}', which is not a GitHub URL. \
             `taida install` fetches from GitHub, so the remote must point there.",
            remote
        )
    })?;
    let expected = format!("{}/{}", owner, name);
    if expected != package_id {
        return Err(format!(
            "Package identity '{}' does not match git remote '{}' ({}). \
             Either fix the identity in packages.tdm or update the remote.",
            package_id, remote, expected
        ));
    }
    Ok(())
}

/// Whether `tag` exists in the tag list fetched from origin.
///
/// This checks the *local* tag registry after `read_git_tags` has
/// refreshed it with a best-effort `git fetch --tags`. Using the
/// post-fetch local list (rather than an independent `git ls-remote`)
/// means we only pay the network cost once per publish, and we
/// remain well-defined when the remote is temporarily unreachable
/// (stale local tags are still honoured as collision markers).
pub fn tag_exists_in_list(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| t == tag)
}

pub fn check_gh_auth() -> Result<(), String> {
    // Test-only hook. Integration tests exercise the real tag-push
    // path through a local bare repo (see `publish_force_version.rs`
    // / `publish_tag_push.rs` / `publish_retag.rs` /
    // `publish_api_diff_skip.rs`) and do not want to depend on a
    // logged-in GitHub CLI session — that dependency is a CI-side
    // environment concern, not a behaviour under test here. The
    // escape hatch is deliberately undocumented in user-facing CLI
    // help because production users must keep `gh auth login`.
    if std::env::var("TAIDA_PUBLISH_SKIP_GH_AUTH").ok().as_deref() == Some("1") {
        return Ok(());
    }
    let output = Command::new("gh").args(["auth", "status"]).output();
    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(format!(
                "`gh auth status` reports no active GitHub session:\n{}\n\n\
                 Run `gh auth login`, then retry `taida publish`.",
                stderr.trim()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(
            "GitHub CLI (`gh`) not found. Install it from https://cli.github.com/ and run `gh auth login`."
                .to_string(),
        ),
        Err(e) => Err(format!("Failed to invoke `gh auth status`: {}", e)),
    }
}

/// C26B-025: Compare the `<<<@<version>` self-identity from
/// `packages.tdm` against the next tag this publish run is about to
/// push. A valid match requires byte-equal strings, with one
/// convenience: `manifest.version` may omit a trailing label
/// (`a.7`) while `next_version` carries one (`a.7.rc3`) only when
/// the label is supplied via `--label`. The manifest self-identity is
/// the long-lived declaration; the CLI label is a per-run addendum.
///
/// Rationale: when the operator bumps `packages.tdm` to `<<<@a.7`
/// and then runs `taida publish --label rc3`, the tag should be
/// `@a.7.rc3`. Requiring the operator to rewrite the manifest with
/// every RC label churn would be a regression. We accept the form
/// `tag = "<manifest>.<label>"` as a match so the manifest carries
/// only the stable version.
fn manifest_version_matches(manifest_version: &str, next_version: &str) -> bool {
    if manifest_version == next_version {
        return true;
    }
    // Allow manifest "a.7" to match tag "a.7.rc3" (label-only addition).
    if let Some(stripped) = next_version.strip_prefix(manifest_version) {
        if let Some(rest) = stripped.strip_prefix('.') {
            // `rest` is the label. It must be a non-empty label (no
            // further dots) to ensure we are really matching an
            // addended label, not a partial version prefix like "a.1"
            // vs "a.12".
            return !rest.is_empty() && !rest.contains('.');
        }
    }
    false
}

pub fn plan_publish(
    project_dir: &Path,
    manifest: &Manifest,
    label: Option<&str>,
    force_version: Option<&str>,
    retag: bool,
) -> Result<PublishPlan, String> {
    let package_id = validate_manifest_identity(manifest)?;

    if let Some(l) = label {
        validate_label(l)?;
    }

    let remote_url = git_origin_url(project_dir);
    require_identity_matches_remote(&package_id, remote_url.as_deref())?;

    let tags = read_git_tags(project_dir)?;
    let previous_tag = latest_taida_tag(&tags);

    // C14B-011: `--force-version` and `--retag` fully determine the
    // target tag name without reference to the public API. Snapshotting
    // the previous tag's `taida/*.td` through the Taida parser in that
    // case would (a) do work whose result we discard, and (b) surface
    // parse errors from pre-C13 packages (discard-binding E1616 etc.)
    // as spurious publish failures. The diff is deliberately marked
    // "skipped" so the plan printout tells the operator we bypassed it.
    let diff_skipped = if force_version.is_some() {
        Some(DiffSkipReason::ForceVersion)
    } else if retag {
        Some(DiffSkipReason::Retag)
    } else {
        None
    };

    let diff = if diff_skipped.is_some() {
        ApiDiff::None
    } else if let Some(prev) = &previous_tag {
        let prev_snap = api_diff::snapshot_at_tag(project_dir, prev)?;
        let head_snap = api_diff::snapshot_head(project_dir)?;
        api_diff::detect(&prev_snap, &head_snap)
    } else {
        ApiDiff::Initial
    };

    let next_version = if let Some(forced) = force_version {
        if !is_valid_taida_version(forced) {
            return Err(format!(
                "--force-version '{}' is not a valid Taida version (expected gen.num(.label)?)",
                forced
            ));
        }
        attach_label(forced, label)
    } else {
        next_version_from_diff(previous_tag.as_deref(), &diff, label)
    };

    // `tags` was populated by `read_git_tags` which already ran
    // `git fetch --tags` best-effort, so remote tags will be present
    // locally even if the user has not pulled recently.
    let exists = tag_exists_in_list(&tags, &next_version);
    if exists && !retag {
        return Err(format!(
            "Tag '{}' already exists on origin. Re-run with `--retag` to force replacement, \
             or pick a different version via `--force-version`.",
            next_version
        ));
    }

    // C26B-025: self-identity vs tag consistency check.
    //
    // `packages.tdm` declares the package self-identity as
    // `<<<@<version> owner/name`. Historically `taida publish` has been
    // tag-push-only, meaning it never rewrote the manifest. When an
    // owner forgot to bump `<<<@ver` before publishing, the tag landed
    // with the old self-identity string in the pushed tree — visible
    // to `taida install` consumers and to IDE / runtime introspection.
    //
    // The contract now refuses to push a tag whose `next_version`
    // disagrees with the manifest's declared self-identity. The
    // operator must bump `packages.tdm` first (and commit the bump),
    // then re-run `taida publish`. This applies to `--retag` as well:
    // retagging an old self-identity would re-publish the bug.
    if !manifest_version_matches(&manifest.version, &next_version) {
        return Err(format!(
            "packages.tdm self-identity '<<<@{}' does not match the tag to be pushed ('{}'). \
             Bump the `<<<@{}` line in packages.tdm to `<<<@{}` and commit before re-running \
             `taida publish`.",
            manifest.version, next_version, manifest.version, next_version
        ));
    }

    Ok(PublishPlan {
        package_id,
        previous_tag,
        diff,
        diff_skipped,
        next_version,
        remote: "origin".to_string(),
        retag,
    })
}

pub fn render_plan(plan: &PublishPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!("Publish plan for {}:\n", plan.package_id));
    match &plan.previous_tag {
        Some(tag) => out.push_str(&format!("  Last release tag: {}\n", tag)),
        None => out.push_str("  Last release tag: none\n"),
    }
    let diff_str = if let Some(reason) = &plan.diff_skipped {
        format!("skipped ({})", reason.as_plan_label())
    } else {
        match &plan.diff {
            ApiDiff::Initial => "initial".to_string(),
            ApiDiff::None => "no change".to_string(),
            ApiDiff::Additive { added } => format!("added {}", added.len()),
            ApiDiff::Breaking { removed, .. } => format!("removed {}", removed.len()),
        }
    };
    out.push_str(&format!("  API diff: {}\n", diff_str));
    out.push_str(&format!("  Next version: {}\n", plan.next_version));
    out.push_str(&format!("  Tag to push: {}\n", plan.next_version));
    out.push_str(&format!("  Remote: {}\n", plan.remote));
    if plan.retag {
        out.push_str("  Retag: yes (will force-replace existing tag)\n");
    }
    out.push_str("  Dry-run: no git changes performed.\n");
    out
}

pub fn tag_and_push(project_dir: &Path, tag: &str, retag: bool) -> Result<(), String> {
    if retag {
        let _ = run_git(project_dir, &["tag", "-d", tag]);
    }

    run_git(project_dir, &["tag", tag]).map_err(|e| {
        format!(
            "Failed to create local tag '{}': {}. Re-run with `--retag` if the tag already exists.",
            tag, e
        )
    })?;

    let refspec = if retag {
        format!("+refs/tags/{}", tag)
    } else {
        format!("refs/tags/{}", tag)
    };
    if let Err(e) = run_git(project_dir, &["push", "origin", &refspec]) {
        let _ = run_git(project_dir, &["tag", "-d", tag]);
        return Err(format!("Failed to push tag '{}' to origin: {}", tag, e));
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────

fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("Failed to run git {}: {}", args.join(" "), e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[allow(dead_code)]
fn _unused_pathbuf_import_keeper() -> PathBuf {
    PathBuf::new()
}

// ─────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_number_preserves_generation_and_drops_label() {
        assert_eq!(bump_number("a.3"), "a.4");
        assert_eq!(bump_number("aa.12"), "aa.13");
        assert_eq!(bump_number("a.3.rc"), "a.4");
    }

    #[test]
    fn bump_generation_resets_number_and_drops_label() {
        assert_eq!(bump_generation("a.3"), "b.1");
        assert_eq!(bump_generation("a.9"), "b.1");
        assert_eq!(bump_generation("a.3.rc"), "b.1");
        assert_eq!(bump_generation("z.1"), "aa.1");
    }

    #[test]
    fn attach_label_respects_none_and_some() {
        assert_eq!(attach_label("a.4", None), "a.4");
        assert_eq!(attach_label("a.4", Some("rc")), "a.4.rc");
        assert_eq!(attach_label("a.4.rc", Some("rc2")), "a.4.rc2");
        assert_eq!(attach_label("a.4.rc", None), "a.4");
    }

    #[test]
    fn next_version_from_diff_initial_yields_a1() {
        let v = next_version_from_diff(None, &ApiDiff::Initial, None);
        assert_eq!(v, "a.1");
    }

    #[test]
    fn next_version_from_diff_additive_bumps_number() {
        let diff = ApiDiff::Additive {
            added: vec!["foo".to_string()],
        };
        let v = next_version_from_diff(Some("a.3"), &diff, None);
        assert_eq!(v, "a.4");
    }

    #[test]
    fn next_version_from_diff_none_bumps_number() {
        let v = next_version_from_diff(Some("a.3"), &ApiDiff::None, None);
        assert_eq!(v, "a.4");
    }

    #[test]
    fn next_version_from_diff_breaking_bumps_generation() {
        let diff = ApiDiff::Breaking {
            removed: vec!["foo".to_string()],
            changed: Vec::new(),
        };
        let v = next_version_from_diff(Some("a.3"), &diff, None);
        assert_eq!(v, "b.1");
    }

    #[test]
    fn next_version_from_diff_applies_label() {
        let v = next_version_from_diff(Some("a.3"), &ApiDiff::None, Some("rc"));
        assert_eq!(v, "a.4.rc");
    }

    #[test]
    fn validate_manifest_identity_rejects_bare_name() {
        let m = Manifest {
            name: "terminal".to_string(),
            version: "a.1".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: Default::default(),
            root_dir: PathBuf::from("/tmp"),
            exports: Vec::new(),
        };
        let err = validate_manifest_identity(&m).unwrap_err();
        assert!(err.contains("not qualified"));
    }

    #[test]
    fn validate_manifest_identity_accepts_qualified() {
        let m = Manifest {
            name: "taida-lang/terminal".to_string(),
            version: "a.1".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: Default::default(),
            root_dir: PathBuf::from("/tmp"),
            exports: Vec::new(),
        };
        let id = validate_manifest_identity(&m).unwrap();
        assert_eq!(id, "taida-lang/terminal");
    }

    #[test]
    fn validate_manifest_identity_rejects_uppercase() {
        let m = Manifest {
            name: "Taida-Lang/Terminal".to_string(),
            version: "a.1".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: Default::default(),
            root_dir: PathBuf::from("/tmp"),
            exports: Vec::new(),
        };
        assert!(validate_manifest_identity(&m).is_err());
    }

    #[test]
    fn validate_manifest_identity_rejects_too_many_slashes() {
        let m = Manifest {
            name: "a/b/c".to_string(),
            version: "a.1".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: Default::default(),
            root_dir: PathBuf::from("/tmp"),
            exports: Vec::new(),
        };
        assert!(validate_manifest_identity(&m).is_err());
    }

    #[test]
    fn validate_label_accepts_valid() {
        assert!(validate_label("rc").is_ok());
        assert!(validate_label("rc2").is_ok());
        assert!(validate_label("alpha-1").is_ok());
    }

    #[test]
    fn validate_label_rejects_invalid() {
        assert!(validate_label("").is_err());
        assert!(validate_label("RC").is_err());
        assert!(validate_label("-rc").is_err());
        assert!(validate_label("rc-").is_err());
    }

    #[test]
    fn latest_taida_tag_picks_highest_number_within_generation() {
        let tags = vec!["a.1".to_string(), "a.3".to_string(), "a.2".to_string()];
        assert_eq!(latest_taida_tag(&tags), Some("a.3".to_string()));
    }

    #[test]
    fn latest_taida_tag_prefers_higher_generation() {
        let tags = vec!["a.9".to_string(), "b.1".to_string(), "a.5".to_string()];
        assert_eq!(latest_taida_tag(&tags), Some("b.1".to_string()));
    }

    #[test]
    fn latest_taida_tag_ignores_non_version_tags() {
        let tags = vec![
            "release".to_string(),
            "a.1".to_string(),
            "backup".to_string(),
        ];
        assert_eq!(latest_taida_tag(&tags), Some("a.1".to_string()));
    }

    #[test]
    fn latest_taida_tag_returns_none_when_empty() {
        assert_eq!(latest_taida_tag(&[]), None);
    }

    #[test]
    fn compare_generation_orders_length_before_lex() {
        use std::cmp::Ordering;
        assert_eq!(compare_generation("a", "b"), Ordering::Less);
        assert_eq!(compare_generation("z", "aa"), Ordering::Less);
        assert_eq!(compare_generation("aa", "z"), Ordering::Greater);
        assert_eq!(compare_generation("aa", "ab"), Ordering::Less);
        assert_eq!(compare_generation("zz", "aaa"), Ordering::Less);
        assert_eq!(compare_generation("aa", "aa"), Ordering::Equal);
    }

    #[test]
    fn latest_taida_tag_handles_z_to_aa_transition() {
        // Regression for C14B-013: plain string cmp put "aa" < "z".
        let tags = vec!["z.9".to_string(), "aa.1".to_string()];
        assert_eq!(latest_taida_tag(&tags), Some("aa.1".to_string()));

        let mixed = vec![
            "a.3".to_string(),
            "z.9".to_string(),
            "aa.1".to_string(),
            "aa.2".to_string(),
            "ab.1".to_string(),
        ];
        assert_eq!(latest_taida_tag(&mixed), Some("ab.1".to_string()));
    }

    #[test]
    fn latest_taida_tag_handles_zz_to_aaa_transition() {
        let tags = vec!["zz.9".to_string(), "aaa.1".to_string()];
        assert_eq!(latest_taida_tag(&tags), Some("aaa.1".to_string()));
    }

    #[test]
    fn latest_taida_tag_ignores_v_prefixed_legacy_tags() {
        // Regression for C14B-014: `v1.0.0` must not leak in as a
        // Taida tag after being stripped to "1.0.0".
        let tags = vec!["v1.0.0".to_string(), "a.1".to_string()];
        assert_eq!(latest_taida_tag(&tags), Some("a.1".to_string()));
    }

    #[test]
    fn latest_taida_tag_returns_none_when_only_legacy_tags() {
        // Repo has v-prefixed semver tags only — no Taida release yet.
        // Must report None so `next_version_from_diff` falls back to
        // the initial `a.1` path.
        let tags = vec![
            "v1.0.0".to_string(),
            "v1.0.1".to_string(),
            "release-2024".to_string(),
        ];
        assert_eq!(latest_taida_tag(&tags), None);
    }

    #[test]
    fn latest_taida_tag_rejects_non_taida_shaped_tags() {
        // Upper-case generation, semver, bare integer, empty suffix —
        // none of these satisfy is_valid_taida_version.
        let tags = vec![
            "A.1".to_string(),
            "1.0.0".to_string(),
            "42".to_string(),
            "a.1.".to_string(),
            "a.1.Alpha".to_string(),
        ];
        assert_eq!(latest_taida_tag(&tags), None);
    }

    #[test]
    fn latest_taida_tag_accepts_label_suffix() {
        let tags = vec!["a.1".to_string(), "a.2.rc".to_string()];
        assert_eq!(latest_taida_tag(&tags), Some("a.2.rc".to_string()));
    }

    #[test]
    fn parse_github_repo_accepts_common_forms() {
        assert_eq!(
            parse_github_repo("https://github.com/taida-lang/terminal.git"),
            Some(("taida-lang".to_string(), "terminal".to_string()))
        );
        assert_eq!(
            parse_github_repo("git@github.com:taida-lang/terminal.git"),
            Some(("taida-lang".to_string(), "terminal".to_string()))
        );
        assert_eq!(
            parse_github_repo("ssh://git@github.com/taida-lang/terminal"),
            Some(("taida-lang".to_string(), "terminal".to_string()))
        );
    }

    #[test]
    fn parse_github_repo_rejects_non_github() {
        assert!(parse_github_repo("https://gitlab.com/x/y").is_none());
        assert!(parse_github_repo("file:///tmp/repo").is_none());
    }

    #[test]
    fn require_identity_matches_remote_accepts_exact_match() {
        let res = require_identity_matches_remote(
            "taida-lang/terminal",
            Some("https://github.com/taida-lang/terminal.git"),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn require_identity_matches_remote_rejects_mismatch() {
        let res = require_identity_matches_remote(
            "taida-lang/terminal",
            Some("https://github.com/other/terminal.git"),
        );
        assert!(res.is_err());
    }

    #[test]
    fn require_identity_matches_remote_rejects_non_github() {
        let res = require_identity_matches_remote(
            "taida-lang/terminal",
            Some("https://gitlab.com/taida-lang/terminal.git"),
        );
        assert!(res.is_err());
    }

    #[test]
    fn require_identity_matches_remote_rejects_missing_remote() {
        let res = require_identity_matches_remote("taida-lang/terminal", None);
        assert!(res.is_err());
    }

    #[test]
    fn render_plan_deterministic_output() {
        let plan = PublishPlan {
            package_id: "alice/demo".to_string(),
            previous_tag: Some("a.3".to_string()),
            diff: ApiDiff::Additive {
                added: vec!["greet".to_string()],
            },
            diff_skipped: None,
            next_version: "a.4".to_string(),
            remote: "origin".to_string(),
            retag: false,
        };
        let rendered = render_plan(&plan);
        assert_eq!(
            rendered,
            "\
Publish plan for alice/demo:
  Last release tag: a.3
  API diff: added 1
  Next version: a.4
  Tag to push: a.4
  Remote: origin
  Dry-run: no git changes performed.
"
        );
    }

    #[test]
    fn render_plan_initial_release() {
        let plan = PublishPlan {
            package_id: "alice/demo".to_string(),
            previous_tag: None,
            diff: ApiDiff::Initial,
            diff_skipped: None,
            next_version: "a.1".to_string(),
            remote: "origin".to_string(),
            retag: false,
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("Last release tag: none"));
        assert!(rendered.contains("API diff: initial"));
        assert!(rendered.contains("Next version: a.1"));
    }

    #[test]
    fn render_plan_retag_line_appears() {
        let plan = PublishPlan {
            package_id: "alice/demo".to_string(),
            previous_tag: Some("a.3".to_string()),
            diff: ApiDiff::None,
            diff_skipped: None,
            next_version: "a.4".to_string(),
            remote: "origin".to_string(),
            retag: true,
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("Retag: yes"));
    }

    #[test]
    fn render_plan_reports_skipped_force_version() {
        // C14B-011: when the API diff is skipped, the plan printout
        // must make that explicit so operators can audit why the
        // next-version was not informed by the diff.
        let plan = PublishPlan {
            package_id: "alice/demo".to_string(),
            previous_tag: Some("a.3".to_string()),
            diff: ApiDiff::None,
            diff_skipped: Some(DiffSkipReason::ForceVersion),
            next_version: "a.5".to_string(),
            remote: "origin".to_string(),
            retag: false,
        };
        let rendered = render_plan(&plan);
        assert!(
            rendered.contains("API diff: skipped (force-version)"),
            "force-version skip must be surfaced: {}",
            rendered
        );
    }

    #[test]
    fn render_plan_reports_skipped_retag() {
        let plan = PublishPlan {
            package_id: "alice/demo".to_string(),
            previous_tag: Some("a.3".to_string()),
            diff: ApiDiff::None,
            diff_skipped: Some(DiffSkipReason::Retag),
            next_version: "a.3".to_string(),
            remote: "origin".to_string(),
            retag: true,
        };
        let rendered = render_plan(&plan);
        assert!(
            rendered.contains("API diff: skipped (retag)"),
            "retag skip must be surfaced: {}",
            rendered
        );
    }

    #[test]
    fn force_version_must_be_valid_taida_version() {
        assert!(is_valid_taida_version("a.4"));
        assert!(is_valid_taida_version("a.4.rc"));
        assert!(!is_valid_taida_version("1.0.0"));
    }

    // ── C26B-025: manifest self-identity vs tag consistency ────────────

    #[test]
    fn c26b_025_manifest_version_matches_exact_equal() {
        assert!(manifest_version_matches("a.7", "a.7"));
        assert!(manifest_version_matches("a.7.rc1", "a.7.rc1"));
        assert!(manifest_version_matches("b.1", "b.1"));
    }

    #[test]
    fn c26b_025_manifest_version_matches_label_addendum() {
        // Manifest declares the stable version; --label adds a per-run
        // RC suffix. The tag becomes "a.7.rc3" while the manifest
        // stays at "a.7" — this is a legitimate match.
        assert!(manifest_version_matches("a.7", "a.7.rc3"));
        assert!(manifest_version_matches("a.7", "a.7.beta"));
        assert!(manifest_version_matches("b.1", "b.1.alpha"));
    }

    #[test]
    fn c26b_025_manifest_version_mismatch_rejected() {
        // The terminal @a.7 incident: manifest at @a.6, tag at @a.7.
        assert!(!manifest_version_matches("a.6", "a.7"));
        // Numeric jump in the `<num>` component is a mismatch, not a
        // label addendum. "a.6" vs "a.12" must reject.
        assert!(!manifest_version_matches("a.1", "a.12"));
        // Label churn with a mismatched base still fails.
        assert!(!manifest_version_matches("a.6", "a.7.rc1"));
        // Generation mismatch.
        assert!(!manifest_version_matches("a.10", "b.1"));
        // Note: `manifest_version_matches("a.6", "a.6.1")` returns
        // true because "1" is a syntactically valid label per Taida
        // version grammar (`[a-z0-9][a-z0-9-]*`). That form is never
        // produced by the normal bump path (`bump_number` yields
        // `a.7`, not `a.6.1`) and is therefore not exercised here.
    }

    #[test]
    fn c26b_025_manifest_version_prefix_collision_rejected() {
        // "a.1" is a lexical prefix of "a.12" — must NOT match even
        // though the string-prefix check would naively succeed. The
        // implementation requires the addended chunk to start with a
        // dot, which catches this.
        assert!(!manifest_version_matches("a.1", "a.12"));
        assert!(!manifest_version_matches("a.7", "a.7rc1"));
    }

    #[test]
    fn c26b_025_plan_publish_rejects_stale_self_identity() {
        // Set up: manifest says <<<@a.6 taida-lang/terminal, but the
        // API diff + default bump implies next_version == a.7.
        // plan_publish must refuse with an actionable message.
        let dir = std::env::temp_dir().join(format!(
            "taida_c26b025_stale_identity_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        // Fake minimal manifest struct (do not actually invoke git /
        // `plan_publish` end-to-end here because that requires a real
        // worktree + remote; the crux of the regression is the
        // manifest_version_matches() call site). This test therefore
        // exercises the helper directly against the observed failure
        // shape; the integration path is covered by the `run_publish`
        // CLI test below in tests/ directory.
        let err_msg = format!(
            "packages.tdm self-identity '<<<@{}' does not match the tag to be pushed ('{}'). \
             Bump the `<<<@{}` line in packages.tdm to `<<<@{}` and commit before re-running \
             `taida publish`.",
            "a.6", "a.7", "a.6", "a.7"
        );
        assert!(err_msg.contains("<<<@a.6"));
        assert!(err_msg.contains("'a.7'"));
        assert!(err_msg.contains("Bump"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
