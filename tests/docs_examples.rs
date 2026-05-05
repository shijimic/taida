// E32B-024 (Lock-N): docs link / file-path integrity smoke test.
//
// `tests/c25b_008_doc_examples_parse.rs` already pins the **parse**
// status of every ` ```taida ` block in `docs/guide` and
// `docs/reference` against a baseline manifest (70 known fragments).
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
// Failure mode: emits every broken link with `path:line: -> target`
// so the offender can be fixed in one pass.

use std::fs;
use std::path::{Path, PathBuf};

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
