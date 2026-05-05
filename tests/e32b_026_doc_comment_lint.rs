// E32B-026 (Lock-N): doc-comment template / docs lint regression
//
// `docs/reference/documentation_comments.md` defines the lint contract:
//   - `@Since:` values must match `[a-z]\.\d+(\.[a-z0-9]+)?` (Taida
//     versioning `<gen>.<num>.<label?>`). Semver-shaped strings like
//     `1.2.0` are rejected because they would trigger the
//     `feedback_taida_versioning` immediate-reject rule (`docs/STABILITY.md`
//     §3 also forbids semver-shaped numbers in release artefacts).
//   - PHILOSOPHY I forbids `null` / `undefined` in the surface; the
//     doc-comment AI-Constraints template must use `Lax.hasValue` /
//     `Result` predicate phrasing instead of "null チェック".
//
// This test pins the regression by walking `docs/` recursively and
// asserting:
//   1. every `@Since:` value matches the locked regex;
//   2. the doc-comment template (`documentation_comments.md` and the
//      AI-Constraints body) does not reintroduce the legacy "null チェック"
//      / "nullチェック" / "null check" phrases that were removed in the
//      Phase 1 docs cleanup.
//
// New documentation must keep both rules satisfied; the test fails loud
// rather than relying on reviewer vigilance.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

fn taida_version_value_matches(value: &str) -> bool {
    // Locked regex: `[a-z]\.\d+(\.[a-z0-9]+)?`
    // Hand-rolled to avoid a regex dependency for a single test.
    let mut chars = value.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    if chars.next() != Some('.') {
        return false;
    }
    let mut saw_digit = false;
    let mut peek_after_digits: Option<char> = None;
    for c in chars.by_ref() {
        if c.is_ascii_digit() {
            saw_digit = true;
            continue;
        }
        peek_after_digits = Some(c);
        break;
    }
    if !saw_digit {
        return false;
    }
    match peek_after_digits {
        None => true,
        Some('.') => {
            let mut saw_label_char = false;
            for c in chars {
                if c.is_ascii_lowercase() || c.is_ascii_digit() {
                    saw_label_char = true;
                    continue;
                }
                return false;
            }
            saw_label_char
        }
        Some(_) => false,
    }
}

#[test]
fn e32b_026_at_since_values_use_taida_versioning() {
    let docs_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    assert!(
        docs_root.exists(),
        "docs/ directory not found at {}",
        docs_root.display()
    );

    let mut md_files = Vec::new();
    collect_md_files(&docs_root, &mut md_files);
    assert!(
        !md_files.is_empty(),
        "no .md files found under docs/ (cwd?)"
    );

    let mut violations = Vec::new();
    for path in &md_files {
        let body = match fs::read_to_string(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for (idx, line) in body.lines().enumerate() {
            // Match both `///@ Since: x.y` (doc-comment example syntax)
            // and the bare `@Since:` referenced in prose. Some docs
            // illustrate the rule with a *bad* value inside a
            // back-tick code-span (e.g. ``@Since: 1.2.0``); skip lines
            // that explicitly cite the value as a counter-example.
            if !line.contains("@Since:") && !line.contains("@ Since:") {
                continue;
            }
            // Skip prose that explicitly cites bad values as
            // counter-examples (the docs themselves explain the rule by
            // citing semver-shaped values).
            if line.contains("Semantic Versioning") || line.contains("セマンティック") {
                continue;
            }
            // Skip lines describing the regex itself.
            if line.contains("[a-z]\\.\\d+") {
                continue;
            }
            // Counter-example phrasing: "1.2.0 のような" or "を使用しません".
            if line.contains("のような Semantic")
                || line.contains("のような値")
                || line.contains("を使用しません")
            {
                continue;
            }

            // Extract the value after `@Since:`.
            let after = line
                .splitn(2, "@Since:")
                .nth(1)
                .or_else(|| line.splitn(2, "@ Since:").nth(1))
                .unwrap_or("")
                .trim();
            // Strip optional surrounding back-ticks / commas / spaces.
            let trimmed: String = after
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '`' && *c != ',' && *c != ')')
                .collect();
            if trimmed.is_empty() {
                continue;
            }
            // Skip self-referential placeholders.
            if trimmed == "<gen>.<num>.<label?>"
                || trimmed.starts_with("<")
                || trimmed.contains("バージョン表記")
            {
                continue;
            }
            // Skip prose mentioning the marker without giving a value
            // (e.g. ``@Since:`` inside back-ticks without a real value).
            if trimmed
                .chars()
                .next()
                .map(|c| !c.is_ascii_lowercase())
                .unwrap_or(true)
            {
                continue;
            }

            if !taida_version_value_matches(&trimmed) {
                violations.push(format!(
                    "{}:{}: @Since value `{}` does not match `[a-z]\\.\\d+(\\.[a-z0-9]+)?`",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(path)
                        .display(),
                    idx + 1,
                    trimmed
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "@Since: lint violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn e32b_026_doc_comment_template_no_null_check_phrasing() {
    // Walks `docs/` looking for the legacy "null チェック" / "nullチェック"
    // / "null check" phrasing that was removed in the Phase 1 cleanup.
    // PHILOSOPHY I forbids `null` / `undefined` in the surface, so the
    // doc-comment template must steer AI users toward `Lax.hasValue` and
    // `Result` predicates instead of "null check"-style guidance.
    let docs_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    let mut md_files = Vec::new();
    collect_md_files(&docs_root, &mut md_files);

    // Allow-list: lines that *explicitly state the rule against* the
    // legacy phrasing, or that explain *why* null-style thinking is
    // unnecessary in Taida. The test should not fail on those.
    let allow_substrings: &[&str] = &[
        // documentation_comments.md PHILOSOPHY callout
        "を使わず",
        "を許可しません",
        "を使わない",
        // 00_overview.md style explanation of why null isn't needed
        "は不要",
        "不要です",
    ];

    let needles: &[&str] = &["null チェック", "nullチェック", "null check", "Null Check"];

    let mut hits = Vec::new();
    for path in &md_files {
        let body = match fs::read_to_string(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for (idx, line) in body.lines().enumerate() {
            for needle in needles {
                if line.contains(needle) {
                    if allow_substrings.iter().any(|allow| line.contains(allow)) {
                        continue;
                    }
                    hits.push(format!(
                        "{}:{}: legacy phrase `{}` — replace with `Lax.hasValue` / `Result` predicate guidance per PHILOSOPHY I",
                        path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                            .unwrap_or(path)
                            .display(),
                        idx + 1,
                        needle
                    ));
                }
            }
        }
    }

    assert!(
        hits.is_empty(),
        "doc-comment legacy null-phrasing violations:\n{}",
        hits.join("\n")
    );
}

#[test]
fn e32b_026_doc_generate_output_has_no_semver_or_null_surface() {
    let dir = unique_temp_dir("e32b_026_doc_generate");
    let src = dir.join("api.td");
    let out = dir.join("api.md");
    write_file(
        &src,
        r#"
///@ Purpose: Generated documentation surface smoke.
///@ Since: e.32
///@ AI-Constraints:
///@   - Lax.hasValue を確認する
///@   - Result predicate を使う
answer <= 42
"#,
    );

    let output = Command::new(taida_bin())
        .args(["doc", "generate", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("run taida doc generate");
    assert!(
        output.status.success(),
        "taida doc generate should succeed; stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let markdown = fs::read_to_string(&out).expect("read generated markdown");
    assert!(
        markdown.contains("### answer"),
        "generated docs should include documented binding, got:\n{}",
        markdown
    );
    assert!(
        markdown.contains("**Since**: e.32"),
        "generated docs should preserve Taida versioning, got:\n{}",
        markdown
    );

    let mut since_lines = Vec::new();
    for line in markdown.lines() {
        if let Some(value) = line.strip_prefix("**Since**:") {
            let value = value.trim();
            since_lines.push(value.to_string());
            assert!(
                taida_version_value_matches(value),
                "generated @Since value `{}` must use Taida versioning in:\n{}",
                value,
                markdown
            );
        }
    }
    assert!(
        !since_lines.is_empty(),
        "generated docs should include at least one Since line"
    );

    let lower = markdown.to_ascii_lowercase();
    assert!(
        !lower.contains("null") && !lower.contains("undefined"),
        "generated docs must not reintroduce null/undefined surface wording:\n{}",
        markdown
    );

    let _ = fs::remove_dir_all(&dir);
}
