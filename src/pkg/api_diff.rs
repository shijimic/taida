//! C14-2a: Public API diff detector (symbol-level).
//!
//! This module compares the set of exported symbols between a package's
//! HEAD tree and a previous release tag, and reports whether the next
//! publish should bump the number (compat / additive) or the generation
//! (breaking).
//!
//! Non-negotiable (C14_DESIGN.md §4):
//!
//! - Must reuse the existing Taida parser (`crate::parser::parse`).
//!   No independent parser is allowed.
//! - Phase 2a covers the **symbol name set** only. Function signatures,
//!   type pack fields, and `native/addon.toml` `[functions]` arity are
//!   intentionally deferred to C14.rc2+ (C14-2b/c/d).
//! - `rename` is not detected as a distinct operation — `foo -> foo2`
//!   is reported as `removed + added` and therefore classified as
//!   Breaking.
//!
//! ## Snapshot sources
//!
//! - `snapshot_head(root)` — walks `root/taida/*.td` from the working
//!   tree (ignoring hidden / build directories).
//! - `snapshot_at_tag(root, tag)` — uses `git ls-tree` + `git show` to
//!   read the contents of `taida/*.td` as they existed at the tag,
//!   without checking out the tree.
//!
//! Both paths feed through the same parser + export extractor so the
//! behaviour is byte-identical modulo the file source.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::parser::{Statement, parse};

/// A snapshot of the public API of a Taida package at a specific
/// point in time (either HEAD on disk or a git tag).
///
/// Phase 2a only populates the `exports` set. Later phases will add
/// signature / pack / arity fields alongside without removing this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicApiSnapshot {
    /// Export symbol names harvested from `taida/*.td`.
    ///
    /// Canonical form: the symbol string as it appears in the
    /// `<<< @(...)` list of the source file. No normalisation is
    /// performed beyond deduplication (the `BTreeSet` semantics).
    pub exports: BTreeSet<String>,
}

/// The classified difference between two snapshots.
///
/// Mapping to next-version rules (applied by the caller, typically
/// `publish::next_version_from_diff`):
///
/// - `Initial` → `"a.1"` (no previous tag).
/// - `None` / `Additive` → number bump (`a.3` → `a.4`).
/// - `Breaking` → generation bump (`a.3` → `b.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiDiff {
    /// There is no previous release tag. The caller should use
    /// `a.1` as the next version.
    Initial,
    /// The export set is byte-identical. Internal changes are still
    /// valid and map to a conservative number bump.
    None,
    /// Exports were added, none removed. Additive-only → number bump.
    Additive { added: Vec<String> },
    /// Exports were removed (or renamed, which we treat as removal +
    /// addition). Breaking → generation bump.
    Breaking {
        removed: Vec<String>,
        /// Reserved for Phase 2b (signature changes). Always empty
        /// in Phase 2a.
        changed: Vec<String>,
    },
}

/// Take a snapshot of the public API at `root/taida/*.td` (working tree).
///
/// Files that fail to parse are reported as errors (first error wins)
/// rather than silently skipped — publishing a package with parse
/// errors should not succeed.
pub fn snapshot_head(root: &Path) -> Result<PublicApiSnapshot, String> {
    let taida_dir = root.join("taida");
    if !taida_dir.exists() {
        // Source-only packages without a `taida/` directory have no
        // public API from this detector's point of view. The detector
        // still returns Ok so `taida ingot publish` does not fail hard on
        // packages that are just manifests + main.td.
        return Ok(PublicApiSnapshot::default());
    }

    let mut exports: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<_> = std::fs::read_dir(&taida_dir)
        .map_err(|e| format!("api_diff: cannot read '{}': {}", taida_dir.display(), e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("td"))
        .collect();
    entries.sort();

    for path in entries {
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("api_diff: cannot read '{}': {}", path.display(), e))?;
        collect_exports_into(&source, &mut exports)?;
    }

    Ok(PublicApiSnapshot { exports })
}

/// Take a snapshot of the public API at `tag` by reading from git
/// history. Does not modify the working tree.
///
/// The enumeration uses `git ls-tree <tag> -- taida/` to find the
/// `*.td` blob names, then `git show <tag>:<path>` to read each
/// file's contents at the tag.
pub fn snapshot_at_tag(root: &Path, tag: &str) -> Result<PublicApiSnapshot, String> {
    // Check the tag exists locally; otherwise a helpful error message
    // is much more useful than `git show` failing with "unknown revision".
    let rev = Command::new("git")
        .args(["rev-parse", "--verify", &format!("refs/tags/{}", tag)])
        .current_dir(root)
        .output()
        .map_err(|e| format!("api_diff: cannot invoke git: {}", e))?;
    if !rev.status.success() {
        return Err(format!(
            "api_diff: tag '{}' does not exist in this repository.",
            tag
        ));
    }

    // List `taida/*.td` entries at the tag.
    let ls = Command::new("git")
        .args(["ls-tree", "--name-only", "-r", tag, "--", "taida"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("api_diff: cannot invoke git ls-tree: {}", e))?;
    if !ls.status.success() {
        // No `taida/` directory at that tag → empty snapshot.
        return Ok(PublicApiSnapshot::default());
    }

    let listing = String::from_utf8_lossy(&ls.stdout).to_string();
    let mut td_paths: Vec<String> = listing
        .lines()
        .map(str::trim)
        .filter(|p| p.ends_with(".td"))
        .map(String::from)
        .collect();
    td_paths.sort();

    let mut exports: BTreeSet<String> = BTreeSet::new();
    for path in td_paths {
        let show = Command::new("git")
            .args(["show", &format!("{}:{}", tag, path)])
            .current_dir(root)
            .output()
            .map_err(|e| format!("api_diff: cannot invoke git show: {}", e))?;
        if !show.status.success() {
            return Err(format!(
                "api_diff: git show {}:{} failed: {}",
                tag,
                path,
                String::from_utf8_lossy(&show.stderr).trim()
            ));
        }
        let source = String::from_utf8_lossy(&show.stdout).into_owned();
        collect_exports_into(&source, &mut exports)?;
    }

    Ok(PublicApiSnapshot { exports })
}

/// Classify the difference between two snapshots.
///
/// See `ApiDiff` docstrings for the exact mapping. This function does
/// not call out to git or the filesystem — it is a pure set operation.
pub fn detect(prev: &PublicApiSnapshot, next: &PublicApiSnapshot) -> ApiDiff {
    let removed: Vec<String> = prev.exports.difference(&next.exports).cloned().collect();
    let added: Vec<String> = next.exports.difference(&prev.exports).cloned().collect();

    if removed.is_empty() && added.is_empty() {
        ApiDiff::None
    } else if removed.is_empty() {
        ApiDiff::Additive { added }
    } else {
        ApiDiff::Breaking {
            removed,
            changed: Vec::new(),
        }
    }
}

/// Parse `source` with the real Taida parser and push every export
/// symbol into `sink`. Returns the first parse error if the source
/// does not parse — callers bail out on that.
fn collect_exports_into(source: &str, sink: &mut BTreeSet<String>) -> Result<(), String> {
    let (program, errors) = parse(source);
    if !errors.is_empty() {
        let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        return Err(format!("api_diff: parse errors:\n{}", msgs.join("\n")));
    }
    for stmt in &program.statements {
        if let Statement::Export(exp) = stmt {
            for sym in &exp.symbols {
                sink.insert(sym.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_from_source(source: &str) -> PublicApiSnapshot {
        let mut exports = BTreeSet::new();
        collect_exports_into(source, &mut exports).expect("source should parse");
        PublicApiSnapshot { exports }
    }

    #[test]
    fn detect_none_when_exports_match() {
        let a = snapshot_from_source("<<< @(hello)\n");
        let b = snapshot_from_source("<<< @(hello)\n");
        assert_eq!(detect(&a, &b), ApiDiff::None);
    }

    #[test]
    fn detect_additive_when_symbol_added() {
        let prev = snapshot_from_source("<<< @(hello)\n");
        let next = snapshot_from_source("<<< @(hello, greet)\n");
        assert_eq!(
            detect(&prev, &next),
            ApiDiff::Additive {
                added: vec!["greet".to_string()],
            }
        );
    }

    #[test]
    fn detect_breaking_when_symbol_removed() {
        let prev = snapshot_from_source("<<< @(hello, greet)\n");
        let next = snapshot_from_source("<<< @(hello)\n");
        assert_eq!(
            detect(&prev, &next),
            ApiDiff::Breaking {
                removed: vec!["greet".to_string()],
                changed: Vec::new(),
            }
        );
    }

    #[test]
    fn detect_breaking_when_symbol_renamed() {
        // Rename is not a first-class operation in Phase 2a: it is
        // observed as one removal + one addition, which makes the
        // diff Breaking (removal is present).
        let prev = snapshot_from_source("<<< @(hello)\n");
        let next = snapshot_from_source("<<< @(hi)\n");
        assert_eq!(
            detect(&prev, &next),
            ApiDiff::Breaking {
                removed: vec!["hello".to_string()],
                changed: Vec::new(),
            }
        );
    }

    #[test]
    fn empty_snapshots_are_none() {
        let a = PublicApiSnapshot::default();
        let b = PublicApiSnapshot::default();
        assert_eq!(detect(&a, &b), ApiDiff::None);
    }

    #[test]
    fn parse_errors_are_reported() {
        let mut sink = BTreeSet::new();
        // Unterminated string literal is an unambiguous lex-level
        // error regardless of later production rules.
        let result = collect_exports_into("hello = \"unterminated\n", &mut sink);
        assert!(
            result.is_err(),
            "syntactically invalid source must not silently yield an empty snapshot"
        );
    }

    #[test]
    fn multiple_exports_in_one_file_are_all_collected() {
        let snap = snapshot_from_source("<<< @(hello, greet, add)\n");
        assert_eq!(
            snap.exports.iter().cloned().collect::<Vec<_>>(),
            vec!["add".to_string(), "greet".to_string(), "hello".to_string()]
        );
    }

    #[test]
    fn snapshot_head_missing_taida_dir_is_empty() {
        let tmp = std::env::temp_dir().join(format!(
            "api_diff_head_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let snap = snapshot_head(&tmp).expect("empty project snapshots fine");
        assert!(snap.exports.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn snapshot_head_reads_taida_td_files() {
        let tmp = std::env::temp_dir().join(format!(
            "api_diff_head_read_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let taida_dir = tmp.join("taida");
        std::fs::create_dir_all(&taida_dir).unwrap();
        std::fs::write(taida_dir.join("a.td"), "<<< @(foo)\n").unwrap();
        std::fs::write(taida_dir.join("b.td"), "<<< @(bar)\n").unwrap();
        let snap = snapshot_head(&tmp).expect("snapshot should succeed");
        assert_eq!(
            snap.exports.iter().cloned().collect::<Vec<_>>(),
            vec!["bar".to_string(), "foo".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
