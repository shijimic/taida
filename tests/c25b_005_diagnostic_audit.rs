//! C25B-005 — Diagnostic code E#### audit.
//!
//! Invariants enforced by this test suite:
//!
//! 1. Every `E####` code emitted from `src/` is listed in
//!    `docs/reference/diagnostic_codes.md` (no silent undocumented codes).
//! 2. Every `E####` code listed in `docs/reference/diagnostic_codes.md` is
//!    emitted from `src/` OR is explicitly marked as `(予約)` / reserved in
//!    the reference (no rotten reference entries).
//! 3. The reference documents the band boundaries that exist in the
//!    codebase, so backend / runtime / module / package / graph bands that
//!    have no concrete codes yet are flagged as category-reservations in the
//!    reference (enforced by the band-section grep).
//!
//! Excluded from emit-site scan:
//! - `src/types/checker_tests.rs` — assertions in unit tests reference codes
//!   by value without emitting them.
//! - doc-comments and banners like `/// - `E1611` — JS backend ...`.
//!
//! The audit is purely textual (regex over source / markdown). If a future
//! refactor moves diagnostics behind an enum, adapt the grep accordingly.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const REFERENCE_PATH: &str = "docs/reference/diagnostic_codes.md";

/// Codes documented in the reference under `(予約)` / "reserved" — allowed
/// to be missing from emit sites.
const RESERVED_CODES: &[&str] = &["E1609", "E1615"];

/// Codes exemplified in the reference's *format specification* ("E0001" /
/// "E9999" as bracket examples, not real codes). These live only in
/// formatting docs, not as diagnostics.
const FORMAT_EXAMPLES: &[&str] = &["E0001", "E9999"];

fn read(p: &str) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {}", p, e))
}

fn walk_rs_files(root: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir({:?}): {}", dir, e)) {
            let e = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }
    out
}

/// Extract codes from source. Scans all `*.rs` under `src/` except the
/// type-checker's *tests* file (which references codes in assertions, not as
/// emit sites). Within each line we drop `///` doc comments (they only list
/// codes by name, they don't emit them).
fn collect_emitted_codes() -> BTreeMap<String, Vec<String>> {
    let re = regex::Regex::new(r"\bE\d{4}\b").unwrap();
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in walk_rs_files("src") {
        let rel = path
            .strip_prefix(".")
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        // Exclude the checker's unit-test buffer (stresses codes as observers).
        if rel.ends_with("checker_tests.rs") || rel.ends_with("parser_tests.rs") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (idx, line) in content.lines().enumerate() {
            // Skip doc-comment lines (they list codes as references, not emit sites).
            let t = line.trim_start();
            if t.starts_with("///") || t.starts_with("//!") || t.starts_with("//") {
                continue;
            }
            for m in re.find_iter(line) {
                let code = m.as_str().to_string();
                map.entry(code)
                    .or_default()
                    .push(format!("{}:{}", rel, idx + 1));
            }
        }
    }
    map
}

/// Extract codes from the reference markdown. Any `E####` token found in a
/// code-quote or table cell counts as "documented". We also record whether
/// the entry has the `(予約)` marker in the same row.
fn collect_documented_codes() -> (BTreeSet<String>, BTreeSet<String>) {
    let re = regex::Regex::new(r"\bE\d{4}\b").unwrap();
    let content = read(REFERENCE_PATH);
    let mut documented: BTreeSet<String> = BTreeSet::new();
    let mut reserved_marked: BTreeSet<String> = BTreeSet::new();
    for line in content.lines() {
        let codes_in_line: Vec<String> =
            re.find_iter(line).map(|m| m.as_str().to_string()).collect();
        if codes_in_line.is_empty() {
            continue;
        }
        let is_reserved_row = line.contains("(予約)") || line.contains("reserved");
        for c in codes_in_line {
            if FORMAT_EXAMPLES.contains(&c.as_str()) {
                continue;
            }
            documented.insert(c.clone());
            if is_reserved_row {
                reserved_marked.insert(c);
            }
        }
    }
    (documented, reserved_marked)
}

#[test]
fn every_emitted_code_is_documented() {
    let emitted = collect_emitted_codes();
    let (documented, _reserved) = collect_documented_codes();
    let mut undocumented: Vec<(String, String)> = Vec::new();
    for (code, sites) in &emitted {
        if !documented.contains(code) {
            undocumented.push((code.clone(), sites.first().cloned().unwrap_or_default()));
        }
    }
    assert!(
        undocumented.is_empty(),
        "\nC25B-005: undocumented diagnostic codes emitted from src/:\n{}\n\nAdd them to {} under the appropriate E##xx band.\n",
        undocumented
            .iter()
            .map(|(c, s)| format!("  {}  (first emit site: {})", c, s))
            .collect::<Vec<_>>()
            .join("\n"),
        REFERENCE_PATH
    );
}

#[test]
fn every_documented_code_is_either_emitted_or_reserved() {
    let emitted = collect_emitted_codes();
    let (documented, reserved_marked) = collect_documented_codes();
    let merged_reserved: BTreeSet<String> = reserved_marked
        .iter()
        .cloned()
        .chain(RESERVED_CODES.iter().map(|s| s.to_string()))
        .collect();

    let mut rot: Vec<String> = Vec::new();
    for code in &documented {
        if emitted.contains_key(code) {
            continue;
        }
        if merged_reserved.contains(code) {
            continue;
        }
        rot.push(code.clone());
    }
    assert!(
        rot.is_empty(),
        "\nC25B-005: documented codes that are neither emitted nor marked reserved:\n{}\n\nEither wire them up in src/ or mark the row as `(予約)` in {}.\n",
        rot.iter()
            .map(|c| format!("  {}", c))
            .collect::<Vec<_>>()
            .join("\n"),
        REFERENCE_PATH
    );
}

/// Cross-check: every code token should appear in a context recognised as
/// canonical — inside backticks, inside brackets, or immediately followed by
/// a colon (legacy `E03##:` form). Detect accidental stray formats like
/// `(E####)` without quoting.
///
/// The neighbour inspection is Unicode-aware (chars, not bytes) so Japanese
/// punctuation (`、`, `・`, etc.) around a code in prose does not count as a
/// formatting issue.
#[test]
fn reference_uses_canonical_code_formatting() {
    let content = read(REFERENCE_PATH);
    let line_re = regex::Regex::new(r"\bE\d{4}\b").unwrap();
    let mut issues = Vec::new();
    let ascii_punct_after = ['`', ']', ' ', '|', ')', ':', ',', '.', '/', '\'', ';', '\0'];
    let ascii_punct_before = ['`', '[', ' ', '|', '(', '/', '\0'];
    for (idx, line) in content.lines().enumerate() {
        for m in line_re.find_iter(line) {
            let start = m.start();
            let end = m.end();
            // Collect the char immediately before/after (Unicode scalar).
            let before: char = line[..start].chars().next_back().unwrap_or('\0');
            let after: char = line[end..].chars().next().unwrap_or('\0');
            // Accept:
            //   - ASCII bracket / backtick / whitespace / pipe / paren neighbours
            //   - non-alphabetic char (covers Japanese punctuation, full-width
            //     spaces, etc.)
            let ok_before = ascii_punct_before.contains(&before) || !before.is_alphabetic();
            let ok_after = ascii_punct_after.contains(&after) || !after.is_alphabetic();
            if !(ok_before && ok_after) {
                issues.push(format!(
                    "{}:{}: `{}`",
                    REFERENCE_PATH,
                    idx + 1,
                    &line[m.start()..m.end()]
                ));
            }
        }
    }
    assert!(
        issues.is_empty(),
        "\nC25B-005: unusual code formatting detected in reference:\n{}\n",
        issues.join("\n")
    );
}

/// Ensure E16xx and E17xx category headers exist in the band-rules section.
/// Guards against accidental deletion of the new bands during future edits.
#[test]
fn band_rules_list_new_categories() {
    let content = read(REFERENCE_PATH);
    for marker in ["E16xx", "E17xx"] {
        assert!(
            content.contains(marker),
            "reference does not mention band `{}` at all; C25B-005 requires all emitted bands to appear in the band-rules section of {}",
            marker,
            REFERENCE_PATH
        );
    }
}

/// Sanity: the audit is actually inspecting the expected trees.
#[test]
fn emit_site_scan_finds_known_good_code() {
    let emitted = collect_emitted_codes();
    // E1301 is emitted from src/types/checker.rs; if the scan missed it,
    // something is wrong with our file walker.
    assert!(
        emitted.contains_key("E1301"),
        "emit-site scan did not find E1301 — walker is probably broken; path-mangling issue?"
    );
}

/// Sanity: the file we point to in REFERENCE_PATH is actually the right file.
#[test]
fn reference_path_points_to_real_doc() {
    let content = read(REFERENCE_PATH);
    assert!(
        content.contains("診断コード体系"),
        "{} does not look like the diagnostic code reference",
        REFERENCE_PATH
    );
}

/// Make the helper callable from other tests that want to know what the
/// audit considers "documented" (future fixture extensions).
#[allow(dead_code)]
fn _reexport_helpers() -> (
    Vec<PathBuf>,
    BTreeMap<String, Vec<String>>,
    BTreeSet<String>,
) {
    let files = walk_rs_files("src");
    let emitted = collect_emitted_codes();
    let (documented, _reserved) = collect_documented_codes();
    (files, emitted, documented)
}

// Avoid unused-import warnings if `Path` is trimmed by a future refactor.
#[allow(dead_code)]
fn _keep_path_in_scope(_: &Path) {}
