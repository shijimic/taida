//! C25B-008 — Docs / example doctest harness (parse-only guard).
//!
//! This test extracts every ` ```taida ` fenced block from `docs/guide/*.md`
//! and `docs/reference/*.md`, runs the live parser on each, and compares the
//! set of parse failures against the baseline manifest
//! `tests/c25b_008_doc_parse_baseline.txt`.
//!
//! Goals:
//! 1. Freeze the parse-status of every documented code block at `@c.25.rc7`.
//! 2. Fail CI if a new parse failure appears (silent doc drift).
//! 3. Fail CI if a previously-failing block is now parseable (stale baseline).
//!
//! Non-goals:
//! - Type-check the blocks. Many blocks are intentional fragments demonstrating
//!   compile errors, type signatures (`:Int => :Str`), or syntax shape without
//!   a surrounding program context. Parse-only is the correct granularity for
//!   a documentation harness; type / runtime validation stays with the
//!   existing `examples/quality/` fixture tests.
//!
//! Skip convention (forward-looking):
//! - A block may include the literal comment `// @doctest: skip` on its first
//!   non-blank line. Such blocks are excluded from both the PASS count and the
//!   baseline check. This is reserved for future intentional opt-outs; as of
//!   @c.25.rc7 no block uses it.
//!
//! Maintenance:
//! - When a doc change alters parse status, regenerate the baseline by running
//!   the probe test and pasting failures (paths + line numbers only) into
//!   `tests/c25b_008_doc_parse_baseline.txt`. A mismatch is a prompt to
//!   consider whether the block should be reworded into a parseable form.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const BASELINE_PATH: &str = "tests/c25b_008_doc_parse_baseline.txt";

fn extract_taida_blocks(md: &str) -> Vec<(usize, String, String)> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut info = String::new();
    let mut body = String::new();
    let mut start = 0usize;
    for (i, line) in md.lines().enumerate() {
        let trimmed_start = line.trim_start();
        if !in_block {
            if let Some(rest) = trimmed_start.strip_prefix("```") {
                let word = rest.split_whitespace().next().unwrap_or("");
                if word == "taida" {
                    in_block = true;
                    info = rest.to_string();
                    body.clear();
                    start = i + 1;
                }
            }
        } else if trimmed_start.starts_with("```") {
            blocks.push((start, info.clone(), body.clone()));
            in_block = false;
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    blocks
}

fn has_skip_marker(body: &str) -> bool {
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        return t.contains("@doctest: skip");
    }
    false
}

fn collect_doc_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["docs/guide", "docs/reference"] {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir({}) failed: {}", dir, e))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        paths.sort();
        out.extend(paths);
    }
    out
}

fn load_baseline() -> BTreeSet<String> {
    let raw = fs::read_to_string(BASELINE_PATH)
        .unwrap_or_else(|e| panic!("baseline manifest not found at {}: {}", BASELINE_PATH, e));
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The core guard. Verifies that the current parse-failure set equals the
/// baseline exactly.
#[test]
fn doc_taida_blocks_parse_matches_baseline() {
    let baseline = load_baseline();
    let mut actual_fails: BTreeSet<String> = BTreeSet::new();
    let mut total = 0usize;
    let mut pass = 0usize;
    let mut skipped = 0usize;

    for path in collect_doc_files() {
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => panic!("cannot read {}: {}", path.display(), e),
        };
        for (line, _info, body) in extract_taida_blocks(&content) {
            total += 1;
            if has_skip_marker(&body) {
                skipped += 1;
                continue;
            }
            let (_prog, errs) = taida::parser::parse(&body);
            if errs.is_empty() {
                pass += 1;
            } else {
                actual_fails.insert(format!("{}:{}", path.display(), line));
            }
        }
    }

    let new_fails: Vec<&String> = actual_fails.difference(&baseline).collect();
    let healed_fails: Vec<&String> = baseline.difference(&actual_fails).collect();

    if !new_fails.is_empty() || !healed_fails.is_empty() {
        let mut msg = String::new();
        msg.push_str(&format!(
            "\nC25B-008 doc parse guard mismatch\n  total blocks: {}\n  passing:      {}\n  skipped:      {}\n  baseline:     {} failing blocks\n  actual:       {} failing blocks\n",
            total,
            pass,
            skipped,
            baseline.len(),
            actual_fails.len()
        ));
        if !new_fails.is_empty() {
            msg.push_str(&format!(
                "\n  New parse failures ({}): these blocks used to parse; a doc edit broke them.\n",
                new_fails.len()
            ));
            for f in &new_fails {
                msg.push_str(&format!("    + {}\n", f));
            }
        }
        if !healed_fails.is_empty() {
            msg.push_str(&format!(
                "\n  Healed blocks ({}): these blocks now parse; update the baseline.\n",
                healed_fails.len()
            ));
            for f in &healed_fails {
                msg.push_str(&format!("    - {}\n", f));
            }
        }
        msg.push_str(&format!(
            "\n  Baseline manifest: {}\n  Refresh by re-running the probe test:\n    cargo test --release --test c25b_008_doc_examples_probe -- --ignored --nocapture\n",
            BASELINE_PATH
        ));
        panic!("{}", msg);
    }

    eprintln!(
        "[C25B-008] doc parse guard OK — {}/{} blocks parse cleanly (+ {} failures pinned by baseline, {} skipped).",
        pass,
        total - skipped,
        baseline.len(),
        skipped
    );
}

/// Invariant: baseline entries must refer to blocks that actually exist.
#[test]
fn baseline_entries_reference_existing_blocks() {
    let baseline = load_baseline();
    let mut known_locations: BTreeSet<String> = BTreeSet::new();
    for path in collect_doc_files() {
        let content = fs::read_to_string(&path).unwrap();
        for (line, _info, _body) in extract_taida_blocks(&content) {
            known_locations.insert(format!("{}:{}", path.display(), line));
        }
    }
    let dangling: Vec<&String> = baseline.difference(&known_locations).collect();
    assert!(
        dangling.is_empty(),
        "baseline manifest references non-existent blocks:\n{}",
        dangling
            .iter()
            .map(|s| format!("  {}", s))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Invariant: skip marker wiring is well-formed (no block is both `skip` and
/// listed in the baseline, since skipped blocks are excluded from the check).
#[test]
fn skip_marker_does_not_coexist_with_baseline_entry() {
    let baseline = load_baseline();
    let mut conflicts = Vec::new();
    for path in collect_doc_files() {
        let content = fs::read_to_string(&path).unwrap();
        for (line, _info, body) in extract_taida_blocks(&content) {
            if has_skip_marker(&body) {
                let key = format!("{}:{}", path.display(), line);
                if baseline.contains(&key) {
                    conflicts.push(key);
                }
            }
        }
    }
    assert!(
        conflicts.is_empty(),
        "these blocks carry `@doctest: skip` yet are listed in the baseline:\n{}",
        conflicts
            .iter()
            .map(|s| format!("  {}", s))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
