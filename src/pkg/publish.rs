use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::manifest::{Manifest, is_valid_taida_version};

const DEFAULT_PROPOSALS_REPO: &str = "taida-community/proposals";

// ─────────────────────────────────────────────────────────────
// RC2.6 Phase 1: addon publish orchestration helpers
// ─────────────────────────────────────────────────────────────
//
// The helpers below are part of the addon publish flow described in
// `.dev/RC2_6_DESIGN.md`. They are deliberately kept **side-effect
// isolated** so that `prepare_publish` (which is a non-mutating,
// read-only function — it runs git subprocesses and fs walks but
// never writes to disk) can stay non-mutating: all disk writes,
// subprocess mutations and SHA computation happen in these helpers
// and are stitched together by `src/main.rs::run_publish` (the
// orchestrator).
//
// Non-negotiable invariants carried from RC2.6 v2 design:
//
//   * `prepare_publish` must not call any of the functions in this
//     block (it would break its non-mutating contract).
//   * `compute_cdylib_sha256` is ungated so it can be unit-tested on
//     any feature set. It operates on an arbitrary byte stream.
//   * `build_addon_artifacts` is `native`-gated because it both relies
//     on `cargo build` producing a `cdylib` for the current host and
//     on `addon::host_target::detect_host_target` which itself lives
//     behind `#[cfg(feature = "native")]`.

/// Result of invoking `cargo build --release --lib` for an addon package.
///
/// Returned by [`build_addon_artifacts`]. Carries exactly the information
/// the downstream pipeline needs: (1) the absolute path to the freshly
/// built `cdylib` so SHA-256 can be computed and the file can be attached
/// to a GitHub Release asset, (2) the library stem so the asset can be
/// renamed into the canonical `lib<stem>-<triple>.<ext>` form, and
/// (3) the current host triple so `addon.lock.toml` can be keyed on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "native")]
pub struct AddonBuildOutput {
    /// Absolute path to the freshly built `cdylib` under
    /// `<project>/target/release/`.
    pub cdylib_path: PathBuf,
    /// Library stem as declared in `native/addon.toml` (the string
    /// between `lib` and the platform extension). Used for canonical
    /// release asset naming.
    pub library_stem: String,
    /// Canonical host triple (e.g. `x86_64-unknown-linux-gnu`). Keyed
    /// into `native/addon.lock.toml::[targets]`.
    pub host_triple: String,
}

/// Build the Rust addon cdylib for the current host and return the
/// artifact location plus metadata.
///
/// This is Phase 1 task **RC2.6-1a**. The function:
///
///   1. Parses `native/addon.toml` to discover the declared library
///      stem (`[addon].library`).
///   2. Detects the current host triple via
///      [`crate::addon::host_target::detect_host_target`].
///   3. Invokes `cargo build --release --lib` in `project_dir` and
///      surfaces `cargo`'s full stderr on failure.
///   4. Probes `target/release/lib<stem>.<ext>` for the cdylib (where
///      `<ext>` is `so` / `dylib` / `dll` depending on the host).
///
/// The function returns an error string (never panics) so the
/// orchestrator in `src/main.rs` can convert it into a CLI diagnostic.
///
/// ## Contract
///
/// * **Not pure.** Invokes `cargo` as a subprocess and touches
///   `project_dir/target/`.
/// * Does **not** modify `packages.tdm`, `addon.toml` or `addon.lock.toml`.
///   Those writes are delegated to subsequent helpers (`1c`/`1e`).
/// * Must be called **after** `prepare_publish` (so that
///   `compute_publish_integrity` is re-evaluated afterwards; the
///   orchestrator handles the ordering).
/// * Only the currently running host is built. Cross-compile is a
///   CI responsibility (RC2.6 non-negotiable 5).
#[cfg(feature = "native")]
pub fn build_addon_artifacts(project_dir: &Path) -> Result<AddonBuildOutput, String> {
    use crate::addon::host_target::detect_host_target;
    use crate::addon::manifest::parse_addon_manifest;

    let addon_toml = project_dir.join("native").join("addon.toml");
    if !addon_toml.exists() {
        return Err(format!(
            "build_addon_artifacts: '{}' not found. `taida publish --target rust-addon` requires a native/addon.toml manifest.",
            addon_toml.display()
        ));
    }

    let manifest = parse_addon_manifest(&addon_toml).map_err(|e| e.to_string())?;
    let library_stem = manifest.library.clone();

    let host = detect_host_target().map_err(|e| {
        format!(
            "build_addon_artifacts: {} (cannot build a host-specific cdylib on this platform).",
            e
        )
    })?;
    let host_triple = host.as_triple().to_string();
    let cdylib_ext = host.cdylib_ext();

    // Invoke cargo build --release --lib. The --manifest-path flag
    // anchors the build at project_dir so the working directory the
    // caller is in does not leak into the cargo invocation.
    let cargo_toml = project_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(format!(
            "build_addon_artifacts: '{}' not found. Addon publish requires a Cargo project alongside packages.tdm.",
            cargo_toml.display()
        ));
    }

    let output = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--lib",
            "--manifest-path",
            cargo_toml
                .to_str()
                .ok_or_else(|| "Cargo.toml path contains non-UTF-8 bytes".to_string())?,
        ])
        .current_dir(project_dir)
        .output()
        .map_err(|e| {
            format!(
                "build_addon_artifacts: failed to invoke cargo build in '{}': {}",
                project_dir.display(),
                e
            )
        })?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "build_addon_artifacts: `cargo build --release --lib` failed in '{}':\n--- stdout ---\n{}\n--- stderr ---\n{}",
            project_dir.display(),
            stdout.trim_end(),
            stderr.trim_end()
        ));
    }

    let cdylib_prefix = host.cdylib_prefix();
    let cdylib_name = format!("{cdylib_prefix}{library_stem}.{cdylib_ext}");
    let cdylib_path = project_dir
        .join("target")
        .join("release")
        .join(&cdylib_name);
    if !cdylib_path.exists() {
        return Err(format!(
            "build_addon_artifacts: expected cdylib '{}' not found after `cargo build --release --lib`. \
             Check that Cargo.toml declares `crate-type = [\"rlib\", \"cdylib\"]` and that \
             `[package].name` produces the stem '{}' configured in native/addon.toml.",
            cdylib_path.display(),
            library_stem
        ));
    }

    Ok(AddonBuildOutput {
        cdylib_path,
        library_stem,
        host_triple,
    })
}

/// Compute the SHA-256 digest of a file and format it as the
/// `"sha256:<64-lowercase-hex>"` string that the addon manifest /
/// lockfile schemas expect.
///
/// This is Phase 1 task **RC2.6-1b**. It delegates to the in-house
/// streaming SHA-256 implementation in `crate::crypto` so no new
/// dependency (`sha2`, `ring`, ...) is added to the tree — consistent
/// with RC2.6 Should Fix S1 "no new TOML/hash crates".
///
/// ## Errors
///
/// Returns the underlying `io::Error` message on read failure. The
/// function is intentionally streaming-friendly (loads the file in
/// one shot via `fs::read`) because addon cdylibs are small (sub-MB).
/// If very large assets appear in a future release, this can be
/// replaced with a chunked loop feeding [`crate::crypto::Sha256`]
/// without changing the return format.
pub fn compute_cdylib_sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "compute_cdylib_sha256: cannot read '{}': {}",
            path.display(),
            e
        )
    })?;
    let hex = crate::crypto::sha256_hex_bytes(&bytes);
    Ok(format!("sha256:{hex}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaidaVersionParts {
    generation: String,
    number: Option<u64>,
    label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishPlan {
    pub version: String,
    pub generation: String,
    pub number: u64,
    pub label: Option<String>,
    pub previous_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishPreparation {
    pub package_name: String,
    pub version: String,
    pub integrity: String,
    pub previous_version: Option<String>,
    pub source_repo: Option<String>,
    pub updated_manifest_source: String,
}

pub fn proposals_repo() -> String {
    env::var("TAIDA_PUBLISH_PROPOSALS_REPO").unwrap_or_else(|_| DEFAULT_PROPOSALS_REPO.to_string())
}

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

pub fn read_git_tags(project_dir: &Path) -> Result<Vec<String>, String> {
    // Fetch remote tags to ensure local tag list is up to date.
    // Failure here is non-fatal (e.g. no remote configured, offline).
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

/// Rewrite the `[library.prebuild].url` in `addon.toml` so that the
/// GitHub org/name prefix matches the current git remote origin.
///
/// This is RC2.6B-004: addon.toml templates are generated with the
/// upstream org hardcoded (e.g. `taida-lang/terminal`), but fork
/// publishers need the URL to point at their own fork's releases.
///
/// The function reads `native/addon.toml`, extracts the existing URL
/// template, derives `(org, name)` from `git remote get-url origin`,
/// and replaces the `https://github.com/<old-org>/<old-name>/` prefix
/// with the origin-derived values. The file is written back to disk
/// only if the URL actually changed.
///
/// Returns `Ok(true)` if the file was rewritten, `Ok(false)` if no
/// change was needed, and `Err` on failure.
pub fn rewrite_prebuild_url_if_needed(project_dir: &Path) -> Result<bool, String> {
    let addon_toml_path = project_dir.join("native").join("addon.toml");
    if !addon_toml_path.exists() {
        return Ok(false);
    }

    let origin = match git_origin_url(project_dir) {
        Some(url) => url,
        None => return Ok(false), // no origin → nothing to rewrite
    };

    let (org, name) = match parse_github_repo(&origin) {
        Some(pair) => pair,
        None => return Ok(false), // non-GitHub remote → skip
    };

    let source = std::fs::read_to_string(&addon_toml_path)
        .map_err(|e| format!("Failed to read '{}': {}", addon_toml_path.display(), e))?;

    // Look for a line matching `url = "https://github.com/<...>/<...>/releases/download/..."`
    // and replace the org/name portion with the origin-derived values.
    let mut rewritten = String::with_capacity(source.len());
    let mut changed = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("url")
            && trimmed.contains("https://github.com/")
            && trimmed.contains("/releases/download/")
        {
            // Extract the current URL value
            if let Some(eq_pos) = trimmed.find('=') {
                let value_part = trimmed[eq_pos + 1..].trim();
                // Strip quotes
                let url_str = value_part.trim_matches('"').trim_matches('\'');
                if let Some(after_gh) = url_str.strip_prefix("https://github.com/") {
                    // Parse out old org/name from the URL
                    if let Some(releases_pos) = after_gh.find("/releases/download/") {
                        let old_org_name = &after_gh[..releases_pos];
                        let suffix = &after_gh[releases_pos..];
                        let new_url = format!("https://github.com/{}/{}{}", org, name, suffix);
                        if old_org_name != format!("{}/{}", org, name) {
                            // Preserve original indentation
                            let indent = &line[..line.len() - line.trim_start().len()];
                            rewritten.push_str(&format!("{}url = \"{}\"", indent, new_url));
                            rewritten.push('\n');
                            changed = true;
                            continue;
                        }
                    }
                }
            }
        }
        rewritten.push_str(line);
        rewritten.push('\n');
    }

    // Preserve trailing newline fidelity: if original had no trailing
    // newline, remove the extra one we added.
    if !source.ends_with('\n') && rewritten.ends_with('\n') {
        rewritten.pop();
    }

    if changed {
        std::fs::write(&addon_toml_path, &rewritten)
            .map_err(|e| format!("Failed to write '{}': {}", addon_toml_path.display(), e))?;
    }

    Ok(changed)
}

pub fn plan_publish_version(
    manifest_version: &str,
    git_tags: &[String],
    label: Option<&str>,
) -> Result<PublishPlan, String> {
    if let Some(label) = label {
        validate_label(label)?;
    }

    let desired = parse_taida_version(manifest_version);
    let all_versions: Vec<TaidaVersionParts> = git_tags
        .iter()
        .filter_map(|tag| parse_taida_tag(tag))
        .collect();
    let latest = all_versions
        .iter()
        .max_by_key(|version| version.number.unwrap_or(0));

    match latest {
        Some(latest) => {
            let latest_generation = latest.generation.clone();
            let desired_generation = desired
                .as_ref()
                .map(|version| version.generation.clone())
                .unwrap_or_else(|| latest_generation.clone());
            let next_generation_name = next_generation(&latest_generation);

            // Existing generation (patch) or next(latest) (new breaking change)
            let generation_exists = all_versions
                .iter()
                .any(|v| v.generation == desired_generation);

            if !generation_exists && desired_generation != next_generation_name {
                return Err(format!(
                    "Generation '{}' does not exist and is not the next generation '{}'. \
                     To patch an existing generation, use one of: {}. \
                     To introduce a breaking change, use '{}'.",
                    desired_generation,
                    next_generation_name,
                    all_versions
                        .iter()
                        .map(|v| v.generation.as_str())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", "),
                    next_generation_name,
                ));
            }

            let number = latest.number.unwrap_or(0) + 1;
            let label = label.map(ToOwned::to_owned);
            Ok(PublishPlan {
                version: format_version(&desired_generation, number, label.as_deref()),
                generation: desired_generation,
                number,
                label,
                previous_version: Some(format_version(
                    &latest.generation,
                    latest.number.unwrap_or(0),
                    latest.label.as_deref(),
                )),
            })
        }
        None => {
            let generation = desired
                .as_ref()
                .map(|version| version.generation.clone())
                .unwrap_or_else(|| "a".to_string());
            let number = desired
                .as_ref()
                .and_then(|version| version.number)
                .unwrap_or(1);
            let label = label
                .map(ToOwned::to_owned)
                .or_else(|| desired.as_ref().and_then(|version| version.label.clone()));
            Ok(PublishPlan {
                version: format_version(&generation, number, label.as_deref()),
                generation,
                number,
                label,
                previous_version: None,
            })
        }
    }
}

pub fn rewrite_export_version(source: &str, new_version: &str) -> Result<String, String> {
    let export_pos = source.find("<<<").ok_or_else(|| {
        "packages.tdm must contain an export (`<<<`) before publishing.".to_string()
    })?;
    let after_export = &source[export_pos + 3..];

    if let Some(rest) = after_export.strip_prefix('@') {
        let version_len = rest
            .chars()
            .take_while(|ch| {
                ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '.' || *ch == '-'
            })
            .count();
        let version_start = export_pos + 4;
        let version_end = version_start + version_len;
        if version_len == 0 {
            return Err("packages.tdm export version is malformed.".to_string());
        }
        let mut updated = String::with_capacity(source.len() + new_version.len());
        updated.push_str(&source[..version_start]);
        updated.push_str(new_version);
        updated.push_str(&source[version_end..]);
        Ok(updated)
    } else {
        let mut updated = String::with_capacity(source.len() + new_version.len() + 1);
        updated.push_str(&source[..export_pos + 3]);
        updated.push('@');
        updated.push_str(new_version);
        updated.push_str(&source[export_pos + 3..]);
        Ok(updated)
    }
}

pub fn compute_publish_integrity(project_dir: &Path) -> String {
    let mut files = Vec::new();
    collect_publish_files(project_dir, project_dir, &mut files);
    files.sort();

    let mut state: u64 = 0xcbf29ce484222325;
    for file in files {
        if let Ok(rel) = file.strip_prefix(project_dir) {
            for byte in rel.to_string_lossy().as_bytes() {
                state ^= *byte as u64;
                state = state.wrapping_mul(0x100000001b3);
            }
        }
        // Separator between filename and content to prevent collision
        // e.g. file "ab" + content "cd" vs file "abc" + content "d"
        state ^= 0xff;
        state = state.wrapping_mul(0x100000001b3);
        if let Ok(content) = std::fs::read(&file) {
            for byte in &content {
                state ^= *byte as u64;
                state = state.wrapping_mul(0x100000001b3);
            }
        }
        // Separator between files
        state ^= 0x00;
        state = state.wrapping_mul(0x100000001b3);
    }

    format!("fnv1a:{state:016x}")
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

/// RC2.6-1g: enforce invariants I2 and I3 of the addon publish flow.
///
/// `check_dirty_allowlist(project_dir, allowlist)` runs `git status
/// --porcelain` and asserts that every dirty entry corresponds to a
/// file in `allowlist`. It is used at two points in the addon
/// orchestrator:
///
/// * **I2 — "prepare → mutate" boundary**. After `prepare_publish`
///   runs, the orchestrator writes the new `packages.tdm` and merges
///   `native/addon.lock.toml`. Before proceeding to `git add + commit`
///   the orchestrator calls this helper with the allowed set so that
///   a rogue file (for example a stray `Cargo.lock` regeneration
///   triggered by `cargo build`) is caught before it silently ends up
///   in the commit.
///
/// * **I3 — "commit ready" precheck**. Called again just before
///   `git_commit_tag_push` so the invariant holds if Phase 1-f adds
///   more mutation steps in the future.
///
/// Untracked files are silently ignored **only** when they match the
/// allowlist. Untracked files outside the allowlist are reported as
/// dirty — a strict interpretation of RC2.6 non-negotiable 1 "do not
/// stage files the user did not intend to publish".
///
/// `target/` is always excluded because `cargo build --release --lib`
/// writes there and the directory is part of the ignore set used by
/// [`compute_publish_integrity`]. This is the only implicit exception;
/// every other file must be in `allowlist` or the worktree counts as
/// dirty.
pub fn check_dirty_allowlist(project_dir: &Path, allowlist: &[&Path]) -> Result<(), String> {
    // Use `-u` (`--untracked-files=all`) so that untracked directories
    // are expanded into individual file entries. Without this flag, git
    // rolls up untracked directories (e.g. `?? native/`) and a prefix
    // match against the allowlist would let stray files slip through.
    // `-u` is safe for small projects like taida where `target/` is
    // already in `.gitignore`.
    let status = run_git(project_dir, &["status", "--porcelain", "-u"])?;
    if status.is_empty() {
        return Ok(());
    }

    // Normalise the allowlist to forward-slash POSIX strings for
    // match purposes; `git status --porcelain` also uses `/`.
    let normalised: Vec<String> = allowlist
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();

    let mut violations: Vec<String> = Vec::new();
    for raw_line in status.lines() {
        if raw_line.len() < 3 {
            continue;
        }
        // `git status --porcelain` format: "XY <path>". Two status
        // code characters + separating space + path.
        //
        // `run_git` applies `.trim()` to the whole stdout buffer so a
        // single-line status like "\n M packages.tdm\n" arrives here
        // as "M packages.tdm" — the leading space of " M" has been
        // stripped away. We detect that case by checking whether the
        // second character is a space: "M p..." means the first space
        // was trimmed (start path at index 2), while " M p..." or
        // "?? n..." means the leading position is the code character
        // itself (start path at index 3).
        let bytes = raw_line.as_bytes();
        let path_start = if bytes.get(1).copied() == Some(b' ') {
            2
        } else {
            3
        };
        if path_start >= raw_line.len() {
            continue;
        }
        let path_str = raw_line[path_start..].trim();
        if path_str.is_empty() {
            continue;
        }
        // Strip optional "-> <new-path>" renames (rare for publish
        // workflow); take the destination side.
        let path_str = if let Some((_, dst)) = path_str.split_once(" -> ") {
            dst.trim()
        } else {
            path_str
        };
        // Strip quotes that git wraps paths with spaces/special chars in.
        let path_str = path_str.trim_matches('"');

        // Implicit exclusion: target/ (cargo build output).
        if path_str == "target" || path_str == "target/" || path_str.starts_with("target/") {
            continue;
        }

        // With `-u` (`--untracked-files=all`), git expands untracked
        // directories into individual file entries, so no directory
        // rollup handling is needed — every entry is a concrete path.
        let allowlist_match = normalised.iter().any(|p| p == path_str);

        if allowlist_match {
            continue;
        }

        violations.push(raw_line.to_string());
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Working tree has unexpected changes outside the publish allowlist:\n{}",
            violations.join("\n")
        ))
    }
}

pub fn prepare_publish(
    project_dir: &Path,
    manifest: &Manifest,
    packages_source: &str,
    _author: &str,
    label: Option<&str>,
) -> Result<PublishPreparation, String> {
    validate_package_name(&manifest.name)?;
    check_worktree_clean(project_dir)?;

    let git_tags = read_git_tags(project_dir)?;
    let plan = plan_publish_version(&manifest.version, &git_tags, label)?;
    let updated_manifest_source = rewrite_export_version(packages_source, &plan.version)?;
    let source_repo = git_origin_url(project_dir);

    if let Some(repo) = &source_repo
        && let Some((_owner, repo_name)) = parse_github_repo(repo)
        && repo_name != manifest.name
    {
        eprintln!(
            "Warning: package name '{}' does not match git remote repository '{}'. This is allowed for forks.",
            manifest.name, repo_name
        );
    }

    let integrity = compute_publish_integrity(project_dir);

    Ok(PublishPreparation {
        package_name: manifest.name.clone(),
        version: plan.version,
        integrity,
        previous_version: plan.previous_version,
        source_repo,
        updated_manifest_source,
    })
}

/// Git commit + tag + push を実行する。
///
/// Performs operations in a safe order with rollback on failure:
/// 1. Check if the tag already exists on remote (fail early)
/// 2. Stage `packages.tdm` plus any caller-supplied `extra_paths`
///    (for the addon flow this typically adds `native/addon.lock.toml`)
/// 3. Commit + tag locally
/// 4. Push commit + tag
/// 5. On push failure, rollback local commit and tag
///
/// ## `extra_paths` contract (RC2.6-1d)
///
/// Each entry is resolved **relative to `project_dir`** before being
/// passed to `git add`. Absolute paths are rejected because the
/// publish flow must never stage files outside the package tree.
/// Paths that do not exist are also rejected so the caller catches
/// typos early rather than silently producing a commit that is
/// missing the addon lockfile.
///
/// Existing source-only callers pass `&[]` so their behaviour is
/// byte-identical to pre-RC2.6 (non-negotiable condition 1).
pub fn git_commit_tag_push(
    project_dir: &Path,
    version: &str,
    package_name: &str,
    extra_paths: &[&Path],
) -> Result<(), String> {
    let tag = version.to_string();
    let tag_ref = format!("refs/tags/{tag}");

    // Pre-check: verify the tag does not already exist on remote
    if let Ok(output) = Command::new("git")
        .args(["ls-remote", "--tags", "origin", &tag_ref])
        .current_dir(project_dir)
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && !stdout.trim().is_empty() {
            return Err(format!(
                "Tag '{}' already exists on remote. Cannot publish duplicate version.",
                tag
            ));
        }
    }

    // Stage packages.tdm
    run_git(project_dir, &["add", "packages.tdm"])?;

    // Stage any extra paths (addon flow: native/addon.lock.toml).
    //
    // Resolution rules:
    //
    //   * Absolute paths are rejected outright: the publish flow must
    //     never stage files that live outside the package tree.
    //   * Relative paths are resolved against `project_dir` and must
    //     point at an existing file.
    //   * `git add` receives the **relative** form so git records a
    //     clean path that matches the rest of the commit.
    for extra in extra_paths {
        if extra.is_absolute() {
            return Err(format!(
                "git_commit_tag_push: extra_paths entry '{}' is absolute; only project-relative paths are allowed.",
                extra.display()
            ));
        }
        let abs = project_dir.join(extra);
        if !abs.exists() {
            return Err(format!(
                "git_commit_tag_push: extra_paths entry '{}' does not exist under project dir '{}'.",
                extra.display(),
                project_dir.display()
            ));
        }
        let rel_str = extra.to_str().ok_or_else(|| {
            "git_commit_tag_push: extra_paths entry is not valid UTF-8.".to_string()
        })?;
        run_git(project_dir, &["add", rel_str])?;
    }

    // Commit
    let message = format!("publish: {package_name}@{version}");
    run_git(project_dir, &["commit", "-m", &message])?;

    // Tag
    if let Err(e) = run_git(project_dir, &["tag", &tag]) {
        // Rollback: undo the commit
        let _ = run_git(project_dir, &["reset", "--soft", "HEAD~1"]);
        return Err(e);
    }

    // Push commit
    if let Err(e) = run_git(project_dir, &["push", "origin", "HEAD"]) {
        // Rollback: delete local tag, undo commit
        let _ = run_git(project_dir, &["tag", "-d", &tag]);
        let _ = run_git(project_dir, &["reset", "--soft", "HEAD~1"]);
        return Err(e);
    }

    // Push tag
    if let Err(e) = run_git(project_dir, &["push", "origin", &tag_ref]) {
        // Rollback: revert the pushed commit, delete local tag
        let _ = run_git(project_dir, &["tag", "-d", &tag]);
        let _ = run_git(project_dir, &["revert", "HEAD", "--no-edit"]);
        let _ = run_git(project_dir, &["push", "origin", "HEAD"]);
        return Err(format!(
            "Tag push failed (commit was pushed but tag was not): {}",
            e
        ));
    }

    Ok(())
}

/// RC2.6-1e: multi-file rollback snapshot.
///
/// The addon publish flow mutates at least two files on disk
/// (`packages.tdm` + `native/addon.lock.toml`) and may add more in the
/// future. The pre-RC2.6 rollback in `src/main.rs::run_publish` only
/// snapshotted `packages.tdm`, which silently left any other mutated
/// file in its post-failure state. `PublishRollback` generalises the
/// pattern so every touched file is captured once, and every
/// error-path restores the worktree atomically.
///
/// # Semantics
///
/// * [`PublishRollback::snapshot`] reads the file's **current** bytes
///   into memory. The snapshot is an in-memory copy — no temp
///   file is written to disk.
/// * If the target file does not exist yet (the lockfile on first
///   publish, for example), `snapshot` records its absence so that
///   [`PublishRollback::restore`] will delete the file on rollback
///   instead of re-creating a garbage placeholder.
/// * [`PublishRollback::restore`] is best-effort: it continues past
///   individual failures so a partial disk error does not leave
///   half the files stranded.
/// * [`PublishRollback::snapshots_count`] is exposed for tests and
///   for orchestrator diagnostics.
#[derive(Debug, Default)]
pub struct PublishRollback {
    entries: Vec<PublishRollbackEntry>,
}

#[derive(Debug)]
enum PublishRollbackEntry {
    /// File existed at snapshot time.
    Existing { path: PathBuf, original: Vec<u8> },
    /// File did not exist at snapshot time; if it now exists on
    /// restore we must delete it.
    Missing { path: PathBuf },
}

impl PublishRollback {
    /// Construct an empty rollback recorder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Snapshot the current bytes of `path` into the rollback buffer.
    ///
    /// If `path` does not exist, a `Missing` entry is recorded so
    /// that a subsequent `restore` will delete any file that the
    /// orchestrator created during the failed publish attempt.
    ///
    /// Returns an I/O error string on read failure; the caller
    /// decides whether to abort the publish flow. Callers typically
    /// wrap this with `?` at the earliest safe point.
    pub fn snapshot(&mut self, path: impl Into<PathBuf>) -> Result<(), String> {
        let path = path.into();
        if path.exists() {
            let original = std::fs::read(&path).map_err(|e| {
                format!(
                    "PublishRollback::snapshot: cannot read '{}': {}",
                    path.display(),
                    e
                )
            })?;
            self.entries
                .push(PublishRollbackEntry::Existing { path, original });
        } else {
            self.entries.push(PublishRollbackEntry::Missing { path });
        }
        Ok(())
    }

    /// Restore every recorded file to its pre-publish state.
    ///
    /// Best-effort: failures for individual entries are collected and
    /// reported as a combined diagnostic string, but every entry is
    /// visited even if an earlier one failed, so a partial disk
    /// error cannot strand part of the worktree.
    ///
    /// After restoring file contents, also resets the git staging area
    /// for every snapshotted path so that a failed publish that already
    /// ran `git add` does not leave orphaned staged changes. Existing
    /// files are unstaged with `git restore --staged`, and files that
    /// were created (Missing marker) are removed from the index with
    /// `git rm --cached --force`. Index restoration failures are
    /// collected but do not block content restoration.
    pub fn restore(&self) -> Result<(), String> {
        let mut errors: Vec<String> = Vec::new();
        // Reverse iteration: restore in LIFO order so files whose
        // creation depended on earlier ones come back consistently.
        for entry in self.entries.iter().rev() {
            match entry {
                PublishRollbackEntry::Existing { path, original } => {
                    if let Err(e) = std::fs::write(path, original) {
                        errors.push(format!("failed to restore '{}': {}", path.display(), e));
                    }
                }
                PublishRollbackEntry::Missing { path } => {
                    if path.exists()
                        && let Err(e) = std::fs::remove_file(path)
                    {
                        errors.push(format!(
                            "failed to remove synthesised file '{}': {}",
                            path.display(),
                            e
                        ));
                    }
                }
            }
        }

        // Phase 2: best-effort git index reset.
        //
        // This handles the scenario where `git add` succeeded but a
        // later step (cargo build, lockfile merge, gh release, ...)
        // failed. Without this, `git status` would still show the
        // file as staged even though its contents have been reverted.
        //
        // Git index restoration is strictly best-effort and never
        // contributes to the error list — we may be running in a temp
        // directory without a git repo (unit tests), or git may not
        // be on PATH. The file-content restoration above is the
        // critical path; the index cleanup is supplementary.
        let work_dir: Option<&Path> = self.entries.first().and_then(|e| match e {
            PublishRollbackEntry::Existing { path, .. }
            | PublishRollbackEntry::Missing { path } => path.parent(),
        });
        if let Some(cwd) = work_dir {
            for entry in &self.entries {
                let (path, is_missing) = match entry {
                    PublishRollbackEntry::Existing { path, .. } => (path, false),
                    PublishRollbackEntry::Missing { path } => (path, true),
                };
                let path_str = match path.to_str() {
                    Some(s) => s,
                    None => continue,
                };
                // Silently ignore all failures — see comment above.
                let _ = if is_missing {
                    Command::new("git")
                        .args(["rm", "--cached", "--force", "--ignore-unmatch", path_str])
                        .current_dir(cwd)
                        .output()
                } else {
                    Command::new("git")
                        .args(["restore", "--staged", path_str])
                        .current_dir(cwd)
                        .output()
                };
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "PublishRollback::restore: {} error(s): {}",
                errors.len(),
                errors.join("; ")
            ))
        }
    }

    /// Number of files currently snapshotted.
    pub fn snapshots_count(&self) -> usize {
        self.entries.len()
    }
}

// ─────────────────────────────────────────────────────────────
// RC2.6 Phase 2: GitHub Release helpers
// ─────────────────────────────────────────────────────────────
//
// The helpers below drive `gh release create` as a subprocess.
// No direct GitHub REST API calls — the `gh` CLI handles auth,
// pagination, asset upload, and error display. If the user does
// not have `gh` installed or authenticated, we give a clear
// error with action hints rather than silently failing.
//
// Design constraints (from RC2_6_DESIGN.md):
//
//   * `create_github_release` runs AFTER `git_commit_tag_push`, so
//     it is a post-push side-effect. There is no rollback: if the
//     release step fails, the commit and tag already exist on the
//     remote and the user must fix things manually (or re-run with
//     `gh release create` by hand).
//   * The `GH_BIN` environment variable overrides the path to `gh`
//     so integration tests can substitute a mock script.
//   * `TAIDA_PUBLISH_SKIP_RELEASE=1` is checked by the orchestrator
//     (not here) — this function is only called when the orchestrator
//     decides a release should happen.

/// A single asset to attach to a GitHub Release.
///
/// The `gh release create` command supports a rename syntax:
/// `<local_path>#<display_name>` so the asset appears with a
/// canonical name in the release even if the on-disk filename
/// differs (e.g. `target/release/libfoo.so#libfoo-x86_64-unknown-linux-gnu.so`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhReleaseAsset {
    /// Absolute (or project-relative) path to the file on disk.
    pub local_path: PathBuf,
    /// Display name for the asset in the GitHub Release. When this
    /// differs from the file's basename, the `#` rename syntax is
    /// used automatically.
    pub asset_name: String,
}

/// Create a GitHub Release for `tag` and attach the given assets.
///
/// This is Phase 2 task **RC2.6-2a**. The function:
///
///   1. Locates the `gh` binary (respects `GH_BIN` env var, otherwise
///      `gh` on PATH).
///   2. Runs `gh auth status` to verify the user is authenticated.
///   3. Invokes `gh release create <tag> --title <title> --notes <notes>
///      <asset1>#<name1> <asset2>#<name2> ...` from `project_dir`.
///
/// ## Error handling
///
/// All errors are returned as descriptive `String`s that the CLI
/// orchestrator can print directly. Error messages include action
/// hints (install `gh`, run `gh auth login`, check file paths).
///
/// ## Contract
///
/// * **Not pure.** Invokes `gh` as a subprocess and creates a GitHub
///   Release (irreversible network side-effect).
/// * Must be called AFTER `git_commit_tag_push` succeeds so the tag
///   exists on the remote.
/// * Does NOT attempt rollback on failure — the caller prints the
///   error and exits.
pub fn create_github_release(
    project_dir: &Path,
    tag: &str,
    title: &str,
    notes: &str,
    assets: &[GhReleaseAsset],
) -> Result<(), String> {
    let gh_bin = env::var("GH_BIN").unwrap_or_else(|_| "gh".to_string());

    // Pre-check 1: Is `gh` available at all?
    let version_check = Command::new(&gh_bin).args(["--version"]).output();
    match version_check {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "GitHub CLI (`gh`) not found.\n\
                 \n\
                 The release step requires `gh` to upload assets to GitHub Releases.\n\
                 Install it from https://cli.github.com/ and then run:\n\
                 \n\
                 \x20 gh auth login\n\
                 \n\
                 Alternatively, skip the release step with:\n\
                 \n\
                 \x20 TAIDA_PUBLISH_SKIP_RELEASE=1 taida publish --target rust-addon\n\
                 \n\
                 Or create the release manually:\n\
                 \n\
                 \x20 gh release create {tag} --title \"{title}\" \\\n\
                 \x20   <asset1> <asset2> ..."
            ));
        }
        Err(e) => {
            return Err(format!("Failed to invoke `{}`: {}", gh_bin, e));
        }
        Ok(out) if !out.status.success() => {
            return Err(format!(
                "`{} --version` exited with status {}.",
                gh_bin, out.status
            ));
        }
        Ok(_) => {}
    }

    // Pre-check 2: Is the user authenticated?
    let auth_output = Command::new(&gh_bin)
        .args(["auth", "status"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("Failed to run `{} auth status`: {}", gh_bin, e))?;

    if !auth_output.status.success() {
        let stderr = String::from_utf8_lossy(&auth_output.stderr);
        return Err(format!(
            "`gh auth status` indicates you are not authenticated:\n\
             {}\n\
             \n\
             Run `gh auth login` to authenticate, then retry `taida publish`.\n\
             \n\
             Alternatively, skip the release step with:\n\
             \n\
             \x20 TAIDA_PUBLISH_SKIP_RELEASE=1 taida publish --target rust-addon",
            stderr.trim()
        ));
    }

    // Validate that every asset file exists on disk.
    for asset in assets {
        if !asset.local_path.exists() {
            return Err(format!(
                "Release asset '{}' (display name '{}') does not exist on disk.",
                asset.local_path.display(),
                asset.asset_name
            ));
        }
    }

    // Build the `gh release create` argument list.
    //
    // The `gh` `#` syntax (`path#name`) only sets a **display label**
    // on the asset — the actual download URL uses the original
    // filename. To ensure the asset's download URL matches the
    // canonical name that `addon.toml`'s URL template expands to
    // (e.g. `libtaida_lang_terminal-x86_64-unknown-linux-gnu.so`),
    // we **copy** the file to a temp location with the canonical name
    // before uploading. This mirrors the approach used in the CI
    // release workflow template (Phase 4 hotfix 228267a).
    let mut cmd_args: Vec<String> = vec![
        "release".to_string(),
        "create".to_string(),
        tag.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--notes".to_string(),
        notes.to_string(),
    ];

    // Temp directory for renamed assets. Cleaned up after upload.
    let rename_dir = std::env::temp_dir().join(format!(
        "taida-publish-assets-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    for asset in assets {
        let basename = asset
            .local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if basename == asset.asset_name {
            // Name already matches — upload directly.
            let path_str = asset.local_path.to_str().ok_or_else(|| {
                format!(
                    "Asset path '{}' contains non-UTF-8 characters.",
                    asset.local_path.display()
                )
            })?;
            cmd_args.push(path_str.to_string());
        } else {
            // Name differs — copy to temp dir with canonical name so
            // the GitHub Release asset URL uses the canonical name.
            std::fs::create_dir_all(&rename_dir)
                .map_err(|e| format!("Cannot create temp dir for asset rename: {}", e))?;
            let dest = rename_dir.join(&asset.asset_name);
            std::fs::copy(&asset.local_path, &dest).map_err(|e| {
                format!(
                    "Cannot copy '{}' to '{}' for canonical rename: {}",
                    asset.local_path.display(),
                    dest.display(),
                    e
                )
            })?;
            let dest_str = dest.to_str().ok_or_else(|| {
                format!(
                    "Renamed asset path '{}' contains non-UTF-8 characters.",
                    dest.display()
                )
            })?;
            cmd_args.push(dest_str.to_string());
        }
    }

    let output = Command::new(&gh_bin)
        .args(&cmd_args)
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("Failed to invoke `{} release create`: {}", gh_bin, e))?;

    // Clean up renamed asset temp directory (best-effort).
    let _ = std::fs::remove_dir_all(&rename_dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "`gh release create` failed (exit {}):\n\
             --- stderr ---\n{}\n--- stdout ---\n{}\n\
             \n\
             You can retry the release manually:\n\
             \n\
             \x20 gh release create {} --title \"{}\" --notes \"...\" <assets...>",
            output.status,
            stderr.trim(),
            stdout.trim(),
            tag,
            title,
        ));
    }

    Ok(())
}

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

/// taida-community/proposals への Issue 作成用 pre-filled URL を生成する。
pub fn proposals_url(author: &str, package_name: &str, version: &str, integrity: &str) -> String {
    let title = format!("publish: {author}/{package_name}@{version}");
    let body = format!(
        "## Publish Request\n\n- author: `{author}`\n- package: `{package_name}`\n- version: `@{version}`\n- integrity: `{integrity}`\n"
    );
    let repo = proposals_repo();
    format!(
        "https://github.com/{repo}/issues/new?title={}&body={}",
        urlencoded(&title),
        urlencoded(&body),
    )
}

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

fn parse_taida_version(version: &str) -> Option<TaidaVersionParts> {
    if !is_valid_taida_version(version) {
        return None;
    }

    let mut parts = version.split('.');
    let generation = parts.next()?.to_string();
    let next = parts.next();
    let last = parts.next();

    match (next, last) {
        (None, None) => Some(TaidaVersionParts {
            generation,
            number: None,
            label: None,
        }),
        (Some(number), None) => Some(TaidaVersionParts {
            generation,
            number: number.parse().ok(),
            label: None,
        }),
        (Some(number), Some(label)) => Some(TaidaVersionParts {
            generation,
            number: number.parse().ok(),
            label: Some(label.to_string()),
        }),
        _ => None,
    }
}

fn parse_taida_tag(tag: &str) -> Option<TaidaVersionParts> {
    // Accept both "a.1" and legacy "va.1" tags
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let parsed = parse_taida_version(version)?;
    parsed.number?;
    Some(parsed)
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

fn format_version(generation: &str, number: u64, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("{generation}.{number}.{label}"),
        None => format!("{generation}.{number}"),
    }
}

/// Validate a Taida package name.
///
/// A package name is either:
///
///   * a **bare** name (e.g. `"my-pkg"`, `"http"`) that was accepted by
///     pre-RC2.6 legacy single-slot projects, or
///   * a **fully qualified** `<org>/<name>` pair (e.g. `"taida-lang/terminal"`,
///     `"shijimic/terminal"`) which is the canonical form across
///     `packages.tdm`, the registry resolver (`src/pkg/store.rs::fetch_and_cache`),
///     and `.taida/deps/<org>/<name>/` layout.
///
/// Both sides of the slash follow the same character rules:
/// `[a-z0-9-]+`, no leading or trailing hyphen, non-empty.
///
/// At most one `/` is allowed. Nested subpaths like `org/name/sub` are
/// not a package name — they are module paths inside a package and are
/// parsed separately by `src/pkg/resolver.rs::resolve_package_module`.
///
/// RC2.6B-012 closure (2026-04-09): the pre-RC2.6 implementation
/// rejected any `/` and only accepted bare names. That prevented
/// `taida publish` from ever validating an `org/name` manifest —
/// which is exactly what `native/addon.toml` and `packages.tdm` use
/// throughout the ecosystem. The fix lifts the constraint to the
/// slash-qualified form while preserving the bare form for backward
/// compatibility.
fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Package name must not be empty.".to_string());
    }
    // Split on '/'. At most one '/' is legal (org/name).
    let mut parts = name.split('/');
    let first = parts.next().unwrap_or("");
    let second = parts.next();
    if parts.next().is_some() {
        return Err(format!(
            "Invalid package name '{}'. Expected either a bare name or a single 'org/name' pair.",
            name
        ));
    }

    let validate_component = |component: &str, label: &str| -> Result<(), String> {
        if component.is_empty() {
            return Err(format!(
                "Invalid package name '{}'. {} must not be empty.",
                name, label
            ));
        }
        if component.starts_with('-') || component.ends_with('-') {
            return Err(format!(
                "Invalid package name '{}'. {} must not start or end with '-'.",
                name, label
            ));
        }
        if !component
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        {
            return Err(format!(
                "Invalid package name '{}'. {} must contain only lowercase letters, digits, and hyphens.",
                name, label
            ));
        }
        Ok(())
    };

    match second {
        None => {
            // Bare form: `my-pkg`, `http`, ...
            validate_component(first, "Package name")
        }
        Some(name_part) => {
            // Qualified form: `org/name`
            validate_component(first, "Org component")?;
            validate_component(name_part, "Name component")?;
            Ok(())
        }
    }
}

fn parse_github_repo(remote: &str) -> Option<(String, String)> {
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

fn collect_publish_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if should_skip_path(root, &path) {
            continue;
        }
        if path.is_dir() {
            collect_publish_files(root, &path, out);
        } else {
            out.push(path);
        }
    }
}

fn should_skip_path(root: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if path.is_dir() && matches!(name, ".git" | ".taida" | "target" | "node_modules") {
        return true;
    }
    if let Ok(relative) = path.strip_prefix(root) {
        return relative.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some(".git" | ".taida" | "target" | "node_modules")
            )
        });
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_publish_version_same_generation() {
        let plan = plan_publish_version("a.3", &["va.3".to_string()], None).unwrap();
        assert_eq!(plan.version, "a.4");
        assert_eq!(plan.previous_version, Some("a.3".to_string()));
    }

    #[test]
    fn test_plan_publish_version_allows_manual_generation_bump() {
        let plan = plan_publish_version("b", &["va.3".to_string()], Some("alpha")).unwrap();
        assert_eq!(plan.version, "b.4.alpha");
        assert_eq!(plan.generation, "b");
    }

    #[test]
    fn test_plan_publish_version_rejects_skipped_generation() {
        let err = plan_publish_version("c", &["va.3".to_string()], None).unwrap_err();
        assert!(err.contains("does not exist and is not the next generation"));
    }

    #[test]
    fn test_plan_publish_version_initial_publish_uses_explicit_version() {
        let plan = plan_publish_version("b.7.beta", &[], None).unwrap();
        assert_eq!(plan.version, "b.7.beta");
        assert_eq!(plan.previous_version, None);
    }

    #[test]
    fn test_rewrite_export_version_replaces_existing_version() {
        let source = ">>> taida-lang/os@a.1\n<<<@a.3 @(run)\n";
        let updated = rewrite_export_version(source, "a.4.rc").unwrap();
        assert!(updated.contains("<<<@a.4.rc @(run)"));
    }

    #[test]
    fn test_rewrite_export_version_adds_missing_version() {
        let source = ">>> taida-lang/os@a.1\n<<< @(run)\n";
        let updated = rewrite_export_version(source, "a.1").unwrap();
        assert!(updated.contains("<<<@a.1 @(run)"));
    }

    #[test]
    fn test_compute_publish_integrity_ignores_git_and_taida_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "taida_publish_hash_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".taida")).unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@(run)\n").unwrap();
        std::fs::write(dir.join(".git").join("config"), "secret").unwrap();
        std::fs::write(dir.join(".taida").join("taida.lock"), "lock").unwrap();

        let hash1 = compute_publish_integrity(&dir);
        std::fs::write(dir.join(".git").join("config"), "changed-secret").unwrap();
        std::fs::write(dir.join(".taida").join("taida.lock"), "changed-lock").unwrap();
        let hash2 = compute_publish_integrity(&dir);

        assert_eq!(hash1, hash2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_github_repo_https_and_ssh() {
        assert_eq!(
            parse_github_repo("https://github.com/taida-community/proposals.git"),
            Some(("taida-community".to_string(), "proposals".to_string()))
        );
        assert_eq!(
            parse_github_repo("git@github.com:taida-community/proposals.git"),
            Some(("taida-community".to_string(), "proposals".to_string()))
        );
    }

    // ── Layer 1: validate_package_name ──

    #[test]
    fn test_validate_package_name_valid_bare() {
        // Bare form (backward compat with pre-RC2.6 single-slot projects)
        assert!(validate_package_name("my-package").is_ok());
        assert!(validate_package_name("http").is_ok());
        assert!(validate_package_name("a1b2").is_ok());
    }

    #[test]
    fn test_validate_package_name_valid_qualified() {
        // RC2.6B-012: org/name form must be accepted now that addon
        // packages and the registry resolver canonicalise on it.
        assert!(validate_package_name("taida-lang/terminal").is_ok());
        assert!(validate_package_name("shijimic/terminal").is_ok());
        assert!(validate_package_name("org1/pkg-2").is_ok());
    }

    #[test]
    fn test_validate_package_name_empty() {
        assert!(validate_package_name("").is_err());
    }

    #[test]
    fn test_validate_package_name_leading_trailing_hyphen() {
        assert!(validate_package_name("-pkg").is_err());
        assert!(validate_package_name("pkg-").is_err());
        // Hyphen rule applies to both sides of a qualified name too.
        assert!(validate_package_name("-org/pkg").is_err());
        assert!(validate_package_name("org-/pkg").is_err());
        assert!(validate_package_name("org/-pkg").is_err());
        assert!(validate_package_name("org/pkg-").is_err());
    }

    #[test]
    fn test_validate_package_name_uppercase_rejected() {
        assert!(validate_package_name("MyPkg").is_err());
        assert!(validate_package_name("Org/pkg").is_err());
        assert!(validate_package_name("org/Pkg").is_err());
    }

    #[test]
    fn test_validate_package_name_special_chars_rejected() {
        assert!(validate_package_name("my_pkg").is_err());
        assert!(validate_package_name("my.pkg").is_err());
        assert!(validate_package_name("org/pkg_1").is_err());
    }

    #[test]
    fn test_validate_package_name_multiple_slashes_rejected() {
        // At most one slash. Nested module paths belong to the
        // resolver, not the package name validator.
        assert!(validate_package_name("a/b/c").is_err());
        assert!(validate_package_name("org/pkg/sub").is_err());
    }

    #[test]
    fn test_validate_package_name_empty_components_rejected() {
        assert!(validate_package_name("/pkg").is_err());
        assert!(validate_package_name("org/").is_err());
        assert!(validate_package_name("/").is_err());
    }

    // ── Layer 1: validate_label ──

    #[test]
    fn test_validate_label_valid() {
        assert!(validate_label("alpha").is_ok());
        assert!(validate_label("rc-1").is_ok());
        assert!(validate_label("beta2").is_ok());
    }

    #[test]
    fn test_validate_label_invalid() {
        assert!(validate_label("Alpha").is_err());
        assert!(validate_label("-bad").is_err());
        assert!(validate_label("bad-").is_err());
    }

    // ── Layer 1: integrity separator collision ──

    #[test]
    fn test_integrity_different_for_boundary_shift() {
        // "file_a" + content "bc" vs "file_ab" + content "c"
        // Without separator these could collide; with separator they must differ.
        let dir1 = std::env::temp_dir().join(format!(
            "taida_integrity_sep1_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir2 = std::env::temp_dir().join(format!(
            "taida_integrity_sep2_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();

        std::fs::write(dir1.join("ab"), "cd").unwrap();
        std::fs::write(dir2.join("a"), "bcd").unwrap();

        let h1 = compute_publish_integrity(&dir1);
        let h2 = compute_publish_integrity(&dir2);
        assert_ne!(
            h1, h2,
            "Different file name/content boundaries must produce different hashes"
        );

        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    // ── Layer 1: should_skip_path with node_modules ──

    #[test]
    fn test_integrity_ignores_node_modules() {
        let dir = std::env::temp_dir().join(format!(
            "taida_publish_nm_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("main.td"), "stdout(1)\n").unwrap();
        std::fs::write(dir.join("node_modules").join("dep.js"), "big blob").unwrap();

        let hash1 = compute_publish_integrity(&dir);
        std::fs::write(dir.join("node_modules").join("dep.js"), "changed blob").unwrap();
        let hash2 = compute_publish_integrity(&dir);

        assert_eq!(
            hash1, hash2,
            "node_modules changes must not affect integrity"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Layer 1: rewrite_export_version edge cases ──

    #[test]
    fn test_rewrite_export_version_no_export_errors() {
        let source = ">>> taida-lang/os@a.1\nsome code\n";
        assert!(rewrite_export_version(source, "a.2").is_err());
    }

    // ── Layer 1: next_generation ──

    #[test]
    fn test_next_generation() {
        assert_eq!(next_generation("a"), "b");
        assert_eq!(next_generation("z"), "aa");
        assert_eq!(next_generation("az"), "ba");
    }

    // ── Layer 1: plan_publish_version — generation does not reset number ──

    #[test]
    fn test_plan_publish_generation_bump_continues_number() {
        // a.10 → b should produce b.11, not b.1
        let plan = plan_publish_version("b", &["va.10".to_string()], None).unwrap();
        assert_eq!(plan.version, "b.11");
        assert_eq!(plan.number, 11);
    }

    // ── Manual tag interference scenarios ──

    #[test]
    fn test_manual_tag_same_generation_large_number() {
        // Someone manually tags b.100; next publish should be b.101
        let plan =
            plan_publish_version("b", &["b.3".to_string(), "b.100".to_string()], None).unwrap();
        assert_eq!(plan.version, "b.101");
        assert_eq!(plan.number, 101);
    }

    #[test]
    fn test_manual_tag_different_generation_patch_allowed() {
        // Tags: b.3, c.50; packages.tdm says @b
        // b is an existing generation → patch allowed, number = 51
        let plan =
            plan_publish_version("b", &["b.3".to_string(), "c.50".to_string()], None).unwrap();
        assert_eq!(plan.version, "b.51");
        assert_eq!(plan.generation, "b");
    }

    #[test]
    fn test_manual_tag_nonexistent_generation_rejected() {
        // Tags: a.1, c.50; packages.tdm says @d
        // d does not exist and next(c) = d → allowed (new breaking change)
        let plan =
            plan_publish_version("d", &["a.1".to_string(), "c.50".to_string()], None).unwrap();
        assert_eq!(plan.version, "d.51");

        // But @e is rejected (next(c) = d, not e)
        let err =
            plan_publish_version("e", &["a.1".to_string(), "c.50".to_string()], None).unwrap_err();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn test_manual_tag_non_taida_version_ignored() {
        // Non-Taida tags (semver, random) should be ignored by parse_taida_tag
        let plan = plan_publish_version(
            "b",
            &[
                "b.3".to_string(),
                "v99.0.0".to_string(),
                "release-2026".to_string(),
                "foo".to_string(),
            ],
            None,
        )
        .unwrap();
        assert_eq!(plan.version, "b.4");
    }

    // ── Label scenarios ──

    #[test]
    fn test_label_on_old_generation_patch() {
        // Tags: a.1, b.3; publish @a --label hotfix → a.4.hotfix
        let plan =
            plan_publish_version("a", &["a.1".to_string(), "b.3".to_string()], Some("hotfix"))
                .unwrap();
        assert_eq!(plan.version, "a.4.hotfix");
        assert_eq!(plan.generation, "a");
    }

    #[test]
    fn test_label_on_new_generation() {
        // Tags: a.3; publish @b --label alpha → b.4.alpha
        let plan = plan_publish_version("b", &["a.3".to_string()], Some("alpha")).unwrap();
        assert_eq!(plan.version, "b.4.alpha");
        assert_eq!(plan.generation, "b");
    }

    #[test]
    fn test_label_with_manual_tag_different_generation() {
        // Tags: a.1, c.50; publish @a --label patch → a.51.patch
        let plan =
            plan_publish_version("a", &["a.1".to_string(), "c.50".to_string()], Some("patch"))
                .unwrap();
        assert_eq!(plan.version, "a.51.patch");
        assert_eq!(plan.generation, "a");
    }

    // ── RC2.6-1b: compute_cdylib_sha256 ──────────────────────

    #[test]
    fn test_compute_cdylib_sha256_empty_file() {
        let dir = std::env::temp_dir().join(format!(
            "taida_sha256_empty_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("empty.bin");
        std::fs::write(&f, b"").unwrap();
        let got = compute_cdylib_sha256(&f).unwrap();
        assert_eq!(
            got,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compute_cdylib_sha256_known_vector() {
        // SHA-256("hello world") is the canonical test vector
        let dir = std::env::temp_dir().join(format!(
            "taida_sha256_vec_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hello.bin");
        std::fs::write(&f, b"hello world").unwrap();
        let got = compute_cdylib_sha256(&f).unwrap();
        assert_eq!(
            got,
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compute_cdylib_sha256_missing_file_errors() {
        let bogus = std::env::temp_dir().join(format!(
            "taida_sha256_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let err = compute_cdylib_sha256(&bogus).unwrap_err();
        assert!(
            err.starts_with("compute_cdylib_sha256: cannot read"),
            "error should carry helper prefix: {err}"
        );
    }

    // ── RC2.6-1a: build_addon_artifacts (negative paths) ─────
    //
    // We do not invoke a real `cargo build` in unit tests (it would
    // take seconds and requires a working Rust toolchain). The
    // integration test in `tests/publish_rust_addon.rs` covers the
    // positive path with a minimal on-disk fixture.

    #[cfg(feature = "native")]
    #[test]
    fn test_build_addon_artifacts_missing_addon_toml_errors() {
        let dir = std::env::temp_dir().join(format!(
            "taida_build_addon_no_toml_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = build_addon_artifacts(&dir).unwrap_err();
        assert!(
            err.contains("native/addon.toml"),
            "error should mention addon.toml: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RC2.6-1g: check_dirty_allowlist ───────────────────

    fn init_tmp_git_repo(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "taida_allowlist_{}_{}_{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@taida.dev"])
            .current_dir(&dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&dir)
            .status()
            .unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@a\n").unwrap();
        Command::new("git")
            .args(["add", "packages.tdm"])
            .current_dir(&dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init", "--quiet"])
            .current_dir(&dir)
            .status()
            .unwrap();
        dir
    }

    #[test]
    fn test_check_dirty_allowlist_clean_tree_is_ok() {
        let dir = init_tmp_git_repo("clean");
        let allowed: &[&Path] = &[];
        check_dirty_allowlist(&dir, allowed).expect("clean tree");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_dirty_allowlist_allows_listed_mutation() {
        let dir = init_tmp_git_repo("allow");
        // Mutate packages.tdm — inside the allowlist.
        std::fs::write(dir.join("packages.tdm"), "<<<@a.1\n").unwrap();
        let allowed = [Path::new("packages.tdm")];
        check_dirty_allowlist(&dir, &allowed).expect("allowed mutation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_dirty_allowlist_rejects_stray_file() {
        let dir = init_tmp_git_repo("stray");
        // Create an unrelated dirty file.
        std::fs::write(dir.join("stray.txt"), "oops\n").unwrap();
        let allowed = [Path::new("packages.tdm")];
        let err = check_dirty_allowlist(&dir, &allowed).unwrap_err();
        assert!(err.contains("stray.txt"), "should mention stray.txt: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_dirty_allowlist_ignores_target_dir() {
        let dir = init_tmp_git_repo("target");
        // Create target/ with a file (simulates cargo build output).
        std::fs::create_dir_all(dir.join("target").join("release")).unwrap();
        std::fs::write(
            dir.join("target").join("release").join("libtest.so"),
            b"binary",
        )
        .unwrap();
        let allowed = [Path::new("packages.tdm")];
        check_dirty_allowlist(&dir, &allowed).expect("target/ should be ignored");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_dirty_allowlist_allows_new_nested_file_in_allowlist() {
        let dir = init_tmp_git_repo("nested");
        // Simulate addon.lock.toml being created for the first time.
        std::fs::create_dir_all(dir.join("native")).unwrap();
        std::fs::write(dir.join("native").join("addon.lock.toml"), "[targets]\n").unwrap();
        let allowed = [Path::new("native/addon.lock.toml")];
        check_dirty_allowlist(&dir, &allowed).expect("nested allowlist match");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_dirty_allowlist_rejects_stray_in_untracked_dir() {
        // Regression: when `native/` is entirely untracked, `git status
        // --porcelain` (without `-u`) reports `?? native/` as a single
        // directory entry.  The old rollup logic would accept it if any
        // allowlist entry started with `native/`, letting stray files
        // like `native/extra.txt` slip through.  With `-u`, git expands
        // the directory into individual entries and each is checked
        // against the allowlist.
        let dir = init_tmp_git_repo("dir_rollup");
        std::fs::create_dir_all(dir.join("native")).unwrap();
        // Allowlisted file.
        std::fs::write(dir.join("native").join("addon.lock.toml"), "[targets]\n").unwrap();
        // Stray file NOT in the allowlist.
        std::fs::write(dir.join("native").join("extra.txt"), "oops\n").unwrap();
        let allowed = [Path::new("native/addon.lock.toml")];
        let err = check_dirty_allowlist(&dir, &allowed).unwrap_err();
        assert!(
            err.contains("native/extra.txt"),
            "should reject stray file inside untracked dir: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RC2.6-1e: PublishRollback ─────────────────────────

    #[test]
    fn test_publish_rollback_snapshots_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "taida_rollback_existing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("packages.tdm");
        std::fs::write(&p, b"original\n").unwrap();

        let mut rb = PublishRollback::new();
        rb.snapshot(&p).expect("snapshot");
        assert_eq!(rb.snapshots_count(), 1);

        // Simulate an in-place rewrite.
        std::fs::write(&p, b"mutated\n").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"mutated\n");

        rb.restore().expect("restore");
        assert_eq!(std::fs::read(&p).unwrap(), b"original\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_publish_rollback_deletes_files_that_did_not_exist_at_snapshot() {
        let dir = std::env::temp_dir().join(format!(
            "taida_rollback_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("addon.lock.toml");

        let mut rb = PublishRollback::new();
        rb.snapshot(&p).expect("snapshot missing file");
        assert_eq!(rb.snapshots_count(), 1);

        // Simulate the orchestrator creating the file.
        std::fs::write(&p, b"[targets]\n").unwrap();
        assert!(p.exists());

        rb.restore().expect("restore");
        assert!(!p.exists(), "file created after snapshot should be removed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_publish_rollback_handles_multiple_files_in_lifo_order() {
        let dir = std::env::temp_dir().join(format!(
            "taida_rollback_multi_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let tdm = dir.join("packages.tdm");
        let lock = dir.join("addon.lock.toml");
        std::fs::write(&tdm, b"<<<@a\n").unwrap();
        // lock does NOT exist yet.

        let mut rb = PublishRollback::new();
        rb.snapshot(&tdm).unwrap();
        rb.snapshot(&lock).unwrap();
        assert_eq!(rb.snapshots_count(), 2);

        // Orchestrator mutates both.
        std::fs::write(&tdm, b"<<<@a.1\n").unwrap();
        std::fs::write(&lock, b"[targets]\n").unwrap();

        rb.restore().unwrap();

        assert_eq!(std::fs::read(&tdm).unwrap(), b"<<<@a\n");
        assert!(!lock.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_publish_rollback_restore_is_idempotent_for_unchanged_files() {
        let dir = std::env::temp_dir().join(format!(
            "taida_rollback_idem_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("packages.tdm");
        std::fs::write(&p, b"unchanged\n").unwrap();

        let mut rb = PublishRollback::new();
        rb.snapshot(&p).unwrap();
        // Don't mutate.
        rb.restore().unwrap();
        // Second restore must also succeed.
        rb.restore().unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"unchanged\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RC2.6-1d: git_commit_tag_push extra_paths validation ──
    //
    // We only test the validator arms that can fire without a real
    // git repo. End-to-end `git commit + tag + push` coverage lives
    // in tests/publish_cli.rs and tests/publish_rust_addon.rs.

    #[test]
    fn test_git_commit_tag_push_rejects_absolute_extra_path() {
        let dir = std::env::temp_dir().join(format!(
            "taida_extra_abs_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // We cannot reach the extra_paths validator without clearing
        // the remote-tag precheck, so build a throwaway git repo.
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&dir)
                .status()
                .expect("git init")
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "t@taida.dev"])
            .current_dir(&dir)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&dir)
            .status();
        std::fs::write(dir.join("packages.tdm"), "<<<@a\n").unwrap();
        let _ = Command::new("git")
            .args(["add", "packages.tdm"])
            .current_dir(&dir)
            .status();
        let _ = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&dir)
            .status();

        let abs = dir.join("native").join("addon.lock.toml");
        let err = git_commit_tag_push(&dir, "a.1", "demo", &[&abs]).unwrap_err();
        assert!(
            err.contains("absolute"),
            "expected absolute-path rejection: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_git_commit_tag_push_rejects_nonexistent_extra_path() {
        let dir = std::env::temp_dir().join(format!(
            "taida_extra_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&dir)
                .status()
                .expect("git init")
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "t@taida.dev"])
            .current_dir(&dir)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&dir)
            .status();
        std::fs::write(dir.join("packages.tdm"), "<<<@a\n").unwrap();
        let _ = Command::new("git")
            .args(["add", "packages.tdm"])
            .current_dir(&dir)
            .status();
        let _ = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&dir)
            .status();

        let extra = Path::new("native/addon.lock.toml");
        let err = git_commit_tag_push(&dir, "a.1", "demo", &[extra]).unwrap_err();
        assert!(
            err.contains("does not exist"),
            "expected missing-file rejection: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_build_addon_artifacts_missing_cargo_toml_errors() {
        let dir = std::env::temp_dir().join(format!(
            "taida_build_addon_no_cargo_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("native")).unwrap();
        std::fs::write(
            dir.join("native").join("addon.toml"),
            "abi = 1\n\
             entry = \"taida_addon_get_v1\"\n\
             package = \"test/pkg\"\n\
             library = \"test_pkg\"\n\
             [functions]\n\
             noop = 0\n",
        )
        .unwrap();
        let err = build_addon_artifacts(&dir).unwrap_err();
        assert!(
            err.contains("Cargo.toml"),
            "error should mention Cargo.toml: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RC2.6-2a: create_github_release (negative paths) ─────
    //
    // We cannot invoke real `gh release create` in unit tests (it
    // would require a GitHub repo + auth). We test the pre-check
    // paths that fire before the subprocess: missing `gh` binary
    // and missing asset files.

    #[test]
    fn test_create_github_release_missing_gh_binary() {
        let dir = std::env::temp_dir().join(format!(
            "taida_gh_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Point GH_BIN at a non-existent binary to force NotFound.
        // Safety: this is a single-threaded test scope; we restore the
        // original value immediately after the call under test.
        let prev = std::env::var("GH_BIN").ok();
        unsafe { std::env::set_var("GH_BIN", "/nonexistent/gh-test-bin-rc26") };

        let err = create_github_release(&dir, "a.1", "test a.1", "notes", &[]).unwrap_err();

        // Restore env.
        match prev {
            Some(v) => unsafe { std::env::set_var("GH_BIN", v) },
            None => unsafe { std::env::remove_var("GH_BIN") },
        }

        assert!(
            err.contains("not found")
                || err.contains("Not Found")
                || err.contains("Failed to invoke"),
            "error should indicate gh is missing: {err}"
        );
        assert!(
            err.contains("gh auth login")
                || err.contains("cli.github.com")
                || err.contains("Failed"),
            "error should contain action hints: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_github_release_missing_asset_file() {
        // The test only reaches the asset-existence check if gh
        // --version + gh auth status pass. We use a mock script.
        let dir = std::env::temp_dir().join(format!(
            "taida_gh_asset_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Create a mock gh script that succeeds on --version and auth status.
        let mock_gh = dir.join("mock-gh");
        #[cfg(unix)]
        {
            std::fs::write(&mock_gh, "#!/bin/sh\nexit 0\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        {
            // On non-Unix, skip this test.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        // Safety: single-threaded test scope, restored immediately.
        let prev = std::env::var("GH_BIN").ok();
        unsafe { std::env::set_var("GH_BIN", mock_gh.to_str().unwrap()) };

        let bogus_asset = GhReleaseAsset {
            local_path: dir.join("nonexistent-lib.so"),
            asset_name: "libfoo-x86_64-unknown-linux-gnu.so".to_string(),
        };
        let err =
            create_github_release(&dir, "a.1", "test a.1", "notes", &[bogus_asset]).unwrap_err();

        match prev {
            Some(v) => unsafe { std::env::set_var("GH_BIN", v) },
            None => unsafe { std::env::remove_var("GH_BIN") },
        }

        assert!(
            err.contains("does not exist"),
            "error should mention missing asset: {err}"
        );
        assert!(
            err.contains("nonexistent-lib.so"),
            "error should name the missing file: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RC2.6B-004: rewrite_prebuild_url_if_needed ──

    #[test]
    fn test_rewrite_prebuild_url_no_addon_toml() {
        // If there is no native/addon.toml, the function should return Ok(false).
        let dir =
            std::env::temp_dir().join(format!("taida_test_b004_no_addon_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No native/addon.toml created.
        let result = rewrite_prebuild_url_if_needed(&dir);
        assert_eq!(result, Ok(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rewrite_prebuild_url_rewrites_org_name() {
        // RC2.6B-004: create a real git repo with a GitHub-style origin
        // and an addon.toml pointing to a different org. Verify that
        // rewrite_prebuild_url_if_needed rewrites the file on disk.
        let dir =
            std::env::temp_dir().join(format!("taida_test_b004_rewrite_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("native")).unwrap();

        // Initialise a git repo with a GitHub origin
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git failed")
        };
        run_git(&["init"]);
        run_git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/shijimic/terminal.git",
        ]);

        // Write addon.toml with a different org (taida-lang)
        let addon_toml = dir.join("native/addon.toml");
        std::fs::write(
            &addon_toml,
            r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_lang_terminal"

[functions]
terminalSize = 1

[library.prebuild]
url = "https://github.com/taida-lang/terminal/releases/download/{version}/lib{name}-{target}.{ext}"

[library.prebuild.targets]
"#,
        )
        .unwrap();

        // Call the real function
        let result = rewrite_prebuild_url_if_needed(&dir);
        assert_eq!(result, Ok(true), "should report that a rewrite happened");

        // Verify the file on disk was rewritten
        let content = std::fs::read_to_string(&addon_toml).unwrap();
        assert!(
            content.contains("https://github.com/shijimic/terminal/releases/download/"),
            "URL should point to shijimic/terminal: {}",
            content
        );
        assert!(
            !content.contains("https://github.com/taida-lang/terminal/releases/download/"),
            "old taida-lang URL should be gone: {}",
            content
        );
        // package field should NOT be rewritten (only the URL line)
        assert!(
            content.contains("package = \"taida-lang/terminal\""),
            "package field must be unchanged: {}",
            content
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rewrite_prebuild_url_no_change_when_origin_matches() {
        // When the addon.toml URL already matches the git origin,
        // the function should return Ok(false) and not modify the file.
        let dir = std::env::temp_dir().join(format!("taida_test_b004_noop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("native")).unwrap();

        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git failed")
        };
        run_git(&["init"]);
        run_git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/taida-lang/terminal.git",
        ]);

        let addon_toml = dir.join("native/addon.toml");
        let original = r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_lang_terminal"

[functions]
terminalSize = 1

[library.prebuild]
url = "https://github.com/taida-lang/terminal/releases/download/{version}/lib{name}-{target}.{ext}"

[library.prebuild.targets]
"#;
        std::fs::write(&addon_toml, original).unwrap();

        let result = rewrite_prebuild_url_if_needed(&dir);
        assert_eq!(result, Ok(false), "should report no change needed");

        // File content must be unchanged
        let content = std::fs::read_to_string(&addon_toml).unwrap();
        assert_eq!(content, original, "file must not be modified");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
