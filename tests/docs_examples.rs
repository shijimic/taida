// E32B-024 (Lock-N): docs link / file-path integrity smoke test.
//
// `tests/c25b_008_doc_examples_parse.rs` already pins the **parse**
// status of every ` ```taida ` block in `docs/guide` and
// `docs/reference` against a baseline manifest (68 known fragments).
// The lock asks for an additional, stricter guard: docs *links* and
// *file-path* references must never dangle, so that AI-generated
// content cannot silently introduce typos like `mold_types.md`
// (deleted at @c.25 — replaced by `class_like_types.md`) or stale
// `../reference/` paths.
//
// Scope:
// - Walks every `.md` under `docs/`, plus the top-level `PHILOSOPHY.md`
//   and `README.md`, plus `.dev/E32_*` is intentionally **out of scope**
//   (gitignored design notes).
// - Extracts every relative markdown link target (`[label](path)`).
//   Anchors-only links (`#section`) are skipped, as are absolute URLs
//   (`http://`, `https://`, `mailto:`). `<...>` autolinks are skipped.
// - Resolves the target path relative to the containing file and asserts
//   the file exists. Anchor fragments (`path#anchor`) are stripped
//   before existence checks.
// - Special-cases code-fenced examples: links *inside* a fenced
//   ```` ``` ```` block are typically illustrative (`crypto/sha256.td`)
//   rather than wiki-links, so they are skipped to avoid false
//   positives.
//
// Failure modes:
// - broken links emit every `path:line: -> target` offender.
// - standalone ` ```taida ` blocks are copied to a temp tree and checked
//   by `taida way check`; intentional fragments use ` ```taida fragment `.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DOC_PARSE_BASELINE_PATH: &str = "tests/c25b_008_doc_parse_baseline.txt";

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        // Skip the gitignored `.dev/` design dir even if it appears
        // under the workspace root.
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s == ".dev" || s == "target" || s == "node_modules" || s == "examples")
            .unwrap_or(false)
        {
            continue;
        }
        if path.is_dir() {
            collect_md_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

fn collect_doc_example_files(manifest: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["docs/guide", "docs/reference"] {
        let mut paths: Vec<PathBuf> = fs::read_dir(manifest.join(dir))
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

fn has_way_check_skip_info(info: &str) -> bool {
    info.split_whitespace()
        .any(|word| matches!(word, "fragment" | "no-check" | "reject"))
}

fn load_doc_parse_baseline(manifest: &Path) -> BTreeSet<String> {
    let path = manifest.join(DOC_PARSE_BASELINE_PATH);
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("baseline manifest not found at {}: {}", path.display(), e));
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn doc_block_file_name(relative_doc: &Path, line: usize) -> String {
    let mut stem = relative_doc
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    stem.push_str(&format!("_line_{}.td", line));
    stem
}

fn extract_links(body: &str) -> Vec<(usize, String)> {
    let mut links = Vec::new();
    let mut in_fence = false;
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Find `[label](target)` patterns. A markdown link requires
        // **both** a balanced `[label]` *and* an immediately following
        // `(target)` — Taida code references like `Str[raw](start, end)`
        // share the `](...)` shape but `[raw]` does not look like a
        // markdown label (no separating space, the `[` is part of a
        // type/mold token).
        //
        // To filter without re-implementing CommonMark, require that
        // the `[` opening of the label be preceded by a markdown-link
        // boundary: start-of-line, whitespace, or one of the punctuation
        // chars that introduce a link in prose (`(`, `>`, `\``, `*`,
        // `_`, ` `, etc.). This rejects code-like brackets that follow
        // an identifier or close-paren without whitespace.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'[' {
                i += 1;
                continue;
            }
            // Boundary check on the byte before `[`.
            if i > 0 {
                let prev = bytes[i - 1];
                let is_boundary = prev == b' '
                    || prev == b'\t'
                    || prev == b'('
                    || prev == b'>'
                    || prev == b'<'
                    || prev == b'*'
                    || prev == b'_'
                    || prev == b'!'
                    || prev == b'|'
                    || prev == b'-'
                    || prev == b'~';
                if !is_boundary {
                    i += 1;
                    continue;
                }
            }
            // Find balanced `]` for the label, allowing nested `[]`
            // pairs once (rare in markdown but seen in some labels).
            let label_start = i + 1;
            let mut depth = 1;
            let mut j = label_start;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                j += 1;
            }
            if depth != 0 || j >= bytes.len() {
                i += 1;
                continue;
            }
            let label_end = j; // points at `]`
            // Need `(` immediately after `]`.
            if label_end + 1 >= bytes.len() || bytes[label_end + 1] != b'(' {
                i += 1;
                continue;
            }
            let target_start = label_end + 2;
            let mut tdepth = 1;
            let mut k = target_start;
            while k < bytes.len() && tdepth > 0 {
                match bytes[k] {
                    b'(' => tdepth += 1,
                    b')' => tdepth -= 1,
                    _ => {}
                }
                if tdepth == 0 {
                    break;
                }
                k += 1;
            }
            if tdepth != 0 {
                i += 1;
                continue;
            }
            let target = &line[target_start..k];
            // Reject non-link-shaped targets: real markdown link
            // targets are paths or URLs and never contain `<=`, `=>`,
            // a bare comma, or a literal space.
            let looks_like_path_or_url = !target.contains(' ')
                && !target.contains('\t')
                && !target.contains("<=")
                && !target.contains("=>")
                && !target.contains(',');
            if looks_like_path_or_url && !target.is_empty() {
                links.push((idx + 1, target.to_string()));
            }
            i = k + 1;
        }
    }
    links
}

fn is_external_or_anchor(target: &str) -> bool {
    let t = target.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with('#') {
        return true;
    }
    if t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("mailto:")
        || t.starts_with("ftp://")
    {
        return true;
    }
    false
}

fn strip_anchor(target: &str) -> &str {
    target.split('#').next().unwrap_or(target)
}

fn resolve(file: &Path, link: &str) -> PathBuf {
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    let path = strip_anchor(link.trim());
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        parent.join(p)
    }
}

#[test]
fn docs_links_resolve_to_real_paths() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut md_files = Vec::new();

    let docs_root = manifest.join("docs");
    if docs_root.exists() {
        collect_md_files(&docs_root, &mut md_files);
    }

    // Top-level PHILOSOPHY.md / README.md / CHANGELOG.md are part of
    // the public surface and host inbound links from docs.
    for top in ["PHILOSOPHY.md", "README.md", "CHANGELOG.md"] {
        let p = manifest.join(top);
        if p.exists() {
            md_files.push(p);
        }
    }

    assert!(
        !md_files.is_empty(),
        "no .md files found; CARGO_MANIFEST_DIR may be misconfigured"
    );

    let mut broken = Vec::new();
    for path in &md_files {
        let body = match fs::read_to_string(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for (line, target) in extract_links(&body) {
            if is_external_or_anchor(&target) {
                continue;
            }
            // Skip image-like content that pandoc may treat as link
            // (we already match `]( ... )` so image syntax `![..]( ... )`
            // would also pass through; the file-existence check applies
            // regardless and is what we want).
            let resolved = resolve(path, &target);
            // Anchors after `#` are not file paths; if the file exists
            // we're done. Some markdown engines accept relative URLs
            // ending in `/` to mean an `index.md`; treat trailing `/`
            // by trying to find any `.md` inside that dir.
            let exists = if resolved.exists() {
                true
            } else if resolved.to_string_lossy().ends_with('/') {
                resolved.exists() && resolved.is_dir()
            } else {
                // Try with `.md` appended if the link omits the suffix
                // (some docs systems use this convention).
                let with_md = {
                    let s = resolved.to_string_lossy();
                    PathBuf::from(format!("{}.md", s))
                };
                with_md.exists()
            };
            if !exists {
                let rel = path.strip_prefix(manifest).unwrap_or(path);
                broken.push(format!("{}:{}: -> `{}`", rel.display(), line, target));
            }
        }
    }

    assert!(
        broken.is_empty(),
        "broken docs link(s):\n{}",
        broken.join("\n")
    );
}

// ── @c.25.rc7 baseline guard echo ────────────────────────────────────
//
// The companion test in `c25b_008_doc_examples_parse.rs` is the source
// of truth for parse-status snapshots. We add a tiny sanity check here
// so a future docs change that introduces NEW broken links also fails
// fast in `cargo test --test docs_examples` (the typical local
// invocation), not only via the broader baseline test name.
#[test]
fn docs_examples_smoke_self_check() {
    // Sanity: the test harness must locate at least 10 docs files.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut md_files = Vec::new();
    let docs_root = manifest.join("docs");
    collect_md_files(&docs_root, &mut md_files);
    assert!(
        md_files.len() >= 10,
        "expected ≥10 docs/*.md files (found {})",
        md_files.len()
    );
}

#[test]
fn doc_parse_pass_blocks_way_check_cleanly() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let parse_baseline = load_doc_parse_baseline(manifest);
    let tmp = common::unique_temp_dir("e32b_024_docs_way_check");

    let mut written = 0usize;
    let mut skipped_parse_fail = 0usize;
    let mut skipped_marked = 0usize;
    for path in collect_doc_example_files(manifest) {
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e));
        let relative = path.strip_prefix(manifest).unwrap_or(&path);
        for (line, info, body) in extract_taida_blocks(&content) {
            if has_way_check_skip_info(&info) || has_skip_marker(&body) {
                skipped_marked += 1;
                continue;
            }
            let key = format!("{}:{}", relative.display(), line);
            if parse_baseline.contains(&key) {
                skipped_parse_fail += 1;
                continue;
            }
            let (_program, parse_errors) = taida::parser::parse(&body);
            assert!(
                parse_errors.is_empty(),
                "{} is not in the parse baseline but no longer parses: {:?}",
                key,
                parse_errors
            );
            let out_path = tmp.join(doc_block_file_name(relative, line));
            common::write_file(&out_path, &body);
            written += 1;
        }
    }

    assert!(
        written > 0,
        "expected at least one parse-pass docs example for way-check smoke"
    );

    let output = Command::new(common::taida_bin())
        .arg("way")
        .arg("check")
        .arg("--format")
        .arg("jsonl")
        .arg(&tmp)
        .output()
        .expect("taida way check docs examples");
    assert!(
        output.status.success(),
        "docs parse-pass blocks failed `taida way check` ({} checked, {} parse-fail baseline, {} marked fragment/no-check/reject)\nstdout:\n{}\nstderr:\n{}",
        written,
        skipped_parse_fail,
        skipped_marked,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    eprintln!(
        "[E32B-024] docs way-check smoke OK — {} parse-pass blocks checked ({} parse-fail baseline, {} marked fragment/no-check/reject).",
        written, skipped_parse_fail, skipped_marked
    );

    let _ = fs::remove_dir_all(&tmp);
}
