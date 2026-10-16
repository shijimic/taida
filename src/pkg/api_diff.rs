//! Public API diff detector (symbol-level).
//!
//! This module compares the set of exported symbols between a package's
//! HEAD tree and a previous release tag, and reports whether the next
//! publish should bump the number (compat / additive) or the generation
//! (breaking).
//!
//! Non-negotiable (C14_DESIGN.md §4):
//!
//! - Must reuse the existing Taida parser (`crate::parser::parse`).
//! No independent parser is allowed.
//! - covers the **symbol name set** only. Function signatures,
//! type pack fields, and `native/addon.toml` `[functions]` arity are
//! intentionally deferred to a later compatibility pass.
//! - `rename` is not detected as a distinct operation — `foo -> foo2`
//! is reported as `removed + added` and therefore classified as
//! Breaking.
//!
//! ## What the public API is
//!
//! The names a consumer can import from the package:
//!
//! - the package facade of `packages.tdm` (`<<<@ver owner/name @(...)`),
//!   or, when it lists no symbols, the exports of its entry module
//!   (`>>> ./main.td`);
//! - for an addon package (`native/addon.toml`), the manifest's
//!   `[functions]` and the exports of the addon facade
//!   `taida/<stem>.td`, where `<stem>` is the last segment of the
//!   manifest's `package`;
//! - otherwise, the exports of every top-level `taida/*.td` file.
//!
//! Files below `taida/` subdirectories are internal to the package.
//!
//! ## Snapshot sources
//!
//! - `snapshot_head(root)` — reads those files from the working tree.
//! - `snapshot_at_tag(root, tag)` — reads them as they existed at the
//! tag with `git ls-tree` + `git show`, without checking out the tree.
//!
//! Both paths apply the same rules through the same parser, so they
//! differ only in where the file contents come from.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::addon::manifest::parse_addon_manifest_str;
use crate::parser::{Statement, parse};
use crate::pkg::facade::effective_exports_from_source;
use crate::pkg::manifest::Manifest;

/// A snapshot of the public API of a Taida package at a specific
/// point in time (either HEAD on disk or a git tag).
///
/// only populates the `exports` set. Later phases will add
/// signature / pack / arity fields alongside without removing this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicApiSnapshot {
    /// Export symbol names of the package's public API (see the module
    /// docs for which files contribute).
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
        /// Reserved for (signature changes). Always empty
        /// in.
        changed: Vec<String>,
    },
}

/// Take a snapshot of the public API of the package in the working tree
/// at `root`.
///
/// Files that fail to parse are reported as errors (first error wins)
/// rather than silently skipped — publishing a package with parse
/// errors should not succeed.
pub fn snapshot_head(root: &Path) -> Result<PublicApiSnapshot, String> {
    let read = |rel: &str| -> Result<Option<String>, String> {
        let path = root.join(rel);
        match std::fs::read_to_string(&path) {
            Ok(source) => Ok(Some(source)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("api_diff: cannot read '{}': {}", path.display(), e)),
        }
    };
    let list_taida = || -> Result<Vec<String>, String> {
        let taida_dir = root.join("taida");
        if !taida_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut names: Vec<String> = std::fs::read_dir(&taida_dir)
            .map_err(|e| format!("api_diff: cannot read '{}': {}", taida_dir.display(), e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("td"))
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| format!("taida/{}", n))
            })
            .collect();
        names.sort();
        Ok(names)
    };
    snapshot_from(root, &read, &list_taida)
}

/// Take a snapshot of the public API at `tag` by reading from git
/// history. Does not modify the working tree.
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

    let read = |rel: &str| -> Result<Option<String>, String> {
        let spec = format!("{}:{}", tag, rel);
        let exists = Command::new("git")
            .args(["cat-file", "-e", &spec])
            .current_dir(root)
            .output()
            .map_err(|e| format!("api_diff: cannot invoke git cat-file: {}", e))?;
        if !exists.status.success() {
            return Ok(None);
        }
        let show = Command::new("git")
            .args(["show", &spec])
            .current_dir(root)
            .output()
            .map_err(|e| format!("api_diff: cannot invoke git show: {}", e))?;
        if !show.status.success() {
            return Err(format!(
                "api_diff: git show {} failed: {}",
                spec,
                String::from_utf8_lossy(&show.stderr).trim()
            ));
        }
        Ok(Some(String::from_utf8_lossy(&show.stdout).into_owned()))
    };
    let list_taida = || -> Result<Vec<String>, String> {
        // Immediate children of `taida/` only (no `-r`), like the
        // working-tree listing: subdirectories are package internals.
        let ls = Command::new("git")
            .args(["ls-tree", tag, "--", "taida/"])
            .current_dir(root)
            .output()
            .map_err(|e| format!("api_diff: cannot invoke git ls-tree: {}", e))?;
        if !ls.status.success() {
            return Ok(Vec::new());
        }
        let mut names: Vec<String> = String::from_utf8_lossy(&ls.stdout)
            .lines()
            .filter_map(|line| {
                // `<mode> <type> <object>\t<path>`
                let (meta, path) = line.split_once('\t')?;
                (meta.split_whitespace().nth(1) == Some("blob") && path.ends_with(".td"))
                    .then(|| path.to_string())
            })
            .collect();
        names.sort();
        Ok(names)
    };
    snapshot_from(root, &read, &list_taida)
}

/// Build a snapshot from one file source (working tree or tag): `read`
/// returns a file's contents by path relative to the package root
/// (`None` when it does not exist) and `list_taida` names the top-level
/// `taida/*.td` files. See the module docs for the rules.
fn snapshot_from(
    root: &Path,
    read: &dyn Fn(&str) -> Result<Option<String>, String>,
    list_taida: &dyn Fn() -> Result<Vec<String>, String>,
) -> Result<PublicApiSnapshot, String> {
    let mut exports: BTreeSet<String> = BTreeSet::new();

    if let Some(source) = read("packages.tdm")? {
        let manifest =
            Manifest::parse(&source, root).map_err(|e| format!("api_diff: packages.tdm: {}", e))?;
        if !manifest.exports.is_empty() {
            exports.extend(manifest.exports.iter().cloned());
        } else {
            let entry = manifest
                .entry
                .strip_prefix("./")
                .unwrap_or(&manifest.entry)
                .to_string();
            if let Some(entry_source) = read(&entry)? {
                exports.extend(effective_exports_from_source(
                    &entry_source,
                    Path::new(&entry),
                )?);
            }
        }
    }

    if let Some(source) = read("native/addon.toml")? {
        let manifest = parse_addon_manifest_str(Path::new("native/addon.toml"), &source)
            .map_err(|e| format!("api_diff: {}", e))?;
        exports.extend(manifest.functions.keys().cloned());
        let stem = manifest
            .package
            .rsplit('/')
            .next()
            .unwrap_or(manifest.package.as_str());
        let facade = format!("taida/{}.td", stem);
        if let Some(facade_source) = read(&facade)? {
            exports.extend(effective_exports_from_source(
                &facade_source,
                Path::new(&facade),
            )?);
        }
    } else {
        for rel in list_taida()? {
            if let Some(source) = read(&rel)? {
                collect_exports_into(&source, &mut exports)?;
            }
        }
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
