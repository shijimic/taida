//! C27B-022 (引き取り C26B-015): Native backend path traversal `..`
//! reject 3-backend parity. Pins the 5 acceptance cases listed in
//! `.dev/C27_BLOCKERS.md::C27B-022 Acceptance` × 3 backends
//! (Interpreter / JS / Native) for a total of 15 regression checks.
//!
//! # Cases
//!
//! 1. **root_inside_sibling** — `examples/foo.td` importing
//!    `../src/bar.td` (sibling directory inside the project root).
//!    All 3 backends must accept.
//! 2. **root_inside_nested** — `examples/sub/foo.td` importing
//!    `../../src/bar.td` (two-level pop, lands back inside the
//!    project root). All 3 backends must accept.
//! 3. **root_inside_direct_child** — `examples/foo.td` importing
//!    `../bar.td` where the resolved target is a direct child of
//!    the project root (one level above `examples/`, still inside
//!    the boundary — closest in-bounds resolution shy of leaving
//!    the root). All 3 backends must accept. The corresponding
//!    out-of-bounds counterpart is exercised by Case 5
//!    (root_outside_relative).
//! 4. **root_outside_absolute** — `>>> /tmp/<existing-outside>/x.td`
//!    where the absolute target exists but lives outside the
//!    project root. All 3 backends must reject with the canonical
//!    message: "Import path '<p>' resolves outside the project
//!    root. Path traversal beyond the project boundary is not
//!    allowed.". This case extends the SEC-003 (Interpreter) reach
//!    to JS / Native for true 3-backend parity (closes a confirmed
//!    parity gap discovered during C27B-022 work).
//! 5. **root_outside_relative** — `>>> ../../sibling/bar.td` where
//!    the lexical resolution escapes the project root. All 3
//!    backends must reject with the same canonical message.
//!
//! # 3-backend parity contract
//!
//! - Interpreter rejection comes from
//!   `src/interpreter/module_eval.rs::resolve_import_path` (RCB-303
//!   + SEC-003).
//! - Native rejection comes from
//!   `src/codegen/driver.rs::resolve_module_path` (RCB-303 +
//!   C26B-015 lexical fallback) surfaced via the
//!   `traversal_rejection_path` helper introduced in C27B-022.
//! - JS rejection comes from
//!   `src/js/codegen.rs::resolve_local_import_js_path` (RCB-303 +
//!   C27B-022 absolute-path extension and `js_find_project_root`
//!   marker fallback).
//!
//! All three sites format the rejection message identically so the
//! regression assertions below can be character-exact.
//!
//! # SEC-009 alignment
//!
//! SEC-009 (`src/pkg/store.rs::validate_path_component`) handles a
//! disjoint domain: package store URL component validation
//! (`@(org, name, version)` triples loaded from manifests). The two
//! guards do not overlap — SEC-009 stays in `pkg/store.rs`, this
//! 3-backend RCB-303 / C26B-015 / C27B-022 stack stays in
//! interpreter / JS / native module resolution. No duplicate logic
//! introduced.

// `foo` / `bar` mirror canonical fixture filenames in this domain.
#![allow(clippy::disallowed_names)]

mod common;

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Canonical rejection sentence template. All 3 backends format
/// `format!("Import path '{}' resolves outside the project root. \
///          Path traversal beyond the project boundary is not allowed.", import_path)`
/// — Rust line-continuation keeps the indentation literal, so the
/// runtime byte sequence between the period and `Path` is the
/// fixed whitespace block embedded in the source format strings:
///   `". " + "                         "` (one space + 25 indent
/// spaces). The strict matcher below uses `\s+` to absorb that
/// block defensively in case a future codegen change normalises
/// whitespace; pairwise byte-identity across backends is enforced
/// by deriving the *expected* full sentence from one canonical
/// builder and asserting every backend produces it verbatim.
fn canonical_reject_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"Import path '([^']+)' resolves outside the project root\.\s+Path traversal beyond the project boundary is not allowed\.",
        )
        .expect("canonical reject regex")
    })
}

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Allocate a clean temp directory. We use `taida.toml` as the
/// project-root marker so that all 3 backends agree the directory
/// is the project boundary (Native / Interpreter / JS all walk up
/// looking for `packages.tdm`, `taida.toml`, `.taida`, or `.git`).
fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "taida_c27b022_{}_{}_{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create root");
    fs::write(dir.join("taida.toml"), "").expect("touch taida.toml");
    dir
}

// ── Case 1: root_inside_sibling ────────────────────────────────────────

/// `examples/foo.td` → `>>> ../src/bar.td`. The lexical resolution
/// stays inside the project root — all 3 backends must accept.
fn case1_layout() -> PathBuf {
    let root = temp_root("c1_sibling");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();
    fs::write(
        root.join("src/bar.td"),
        "greet n: Str =\n  \"sibling-\" + n\n=> :Str\n\n<<< @(greet)\n",
    )
    .unwrap();
    fs::write(
        root.join("examples/foo.td"),
        ">>> ../src/bar.td => @(greet)\n\nmsg <= greet(\"x\")\nstdout(msg)\n",
    )
    .unwrap();
    root
}

#[test]
fn c27b_022_case1_root_inside_sibling_interpreter() {
    let root = case1_layout();
    let src = root.join("examples/foo.td");
    let out = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp must accept in-project sibling `..` import; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "sibling-x");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case1_root_inside_sibling_js() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let root = case1_layout();
    let src = root.join("examples/foo.td");
    let out_js = root.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&src)
        .arg("-o")
        .arg(&out_js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "JS must accept in-project sibling `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&out_js)
        .output()
        .expect("run node");
    assert!(run.status.success(), "node exit non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "sibling-x");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case1_root_inside_sibling_native() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let root = case1_layout();
    let src = root.join("examples/foo.td");
    let bin = root.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native must accept in-project sibling `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(run.status.success(), "native exit non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "sibling-x");
    let _ = fs::remove_dir_all(&root);
}

// ── Case 2: root_inside_nested ─────────────────────────────────────────

/// `examples/sub/foo.td` → `>>> ../../src/bar.td`. Two-level `..`
/// pop lands back inside the project root — all 3 backends accept.
fn case2_layout() -> PathBuf {
    let root = temp_root("c2_nested");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("examples/sub")).unwrap();
    fs::write(
        root.join("src/bar.td"),
        "greet n: Str =\n  \"nested-\" + n\n=> :Str\n\n<<< @(greet)\n",
    )
    .unwrap();
    fs::write(
        root.join("examples/sub/foo.td"),
        ">>> ../../src/bar.td => @(greet)\n\nmsg <= greet(\"x\")\nstdout(msg)\n",
    )
    .unwrap();
    root
}

#[test]
fn c27b_022_case2_root_inside_nested_interpreter() {
    let root = case2_layout();
    let src = root.join("examples/sub/foo.td");
    let out = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp must accept nested in-project `..` import; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "nested-x");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case2_root_inside_nested_js() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let root = case2_layout();
    let src = root.join("examples/sub/foo.td");
    let out_js = root.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&src)
        .arg("-o")
        .arg(&out_js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "JS must accept nested in-project `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&out_js)
        .output()
        .expect("run node");
    assert!(run.status.success(), "node exit non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "nested-x");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case2_root_inside_nested_native() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let root = case2_layout();
    let src = root.join("examples/sub/foo.td");
    let bin = root.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native must accept nested in-project `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(run.status.success(), "native exit non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "nested-x");
    let _ = fs::remove_dir_all(&root);
}

// ── Case 3: root_inside_direct_child ───────────────────────────────────

/// `examples/foo.td` → `>>> ../bar.td`. The resolved target sits as
/// a direct child of the project root — one level above `examples/`
/// but still inside the project boundary (the closest in-bounds
/// resolution short of leaving the root). All 3 backends must accept.
///
/// The dual case — relative `..` that *escapes* the root — is
/// covered by Case 5 (root_outside_relative).
fn case3_layout() -> PathBuf {
    let root = temp_root("c3_direct_child");
    fs::create_dir_all(root.join("examples")).unwrap();
    fs::write(
        root.join("bar.td"),
        "greet n: Str =\n  \"direct-child-\" + n\n=> :Str\n\n<<< @(greet)\n",
    )
    .unwrap();
    fs::write(
        root.join("examples/foo.td"),
        ">>> ../bar.td => @(greet)\n\nmsg <= greet(\"x\")\nstdout(msg)\n",
    )
    .unwrap();
    root
}

#[test]
fn c27b_022_case3_root_inside_direct_child_interpreter() {
    let root = case3_layout();
    let src = root.join("examples/foo.td");
    let out = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp must accept direct-child `..` import; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "direct-child-x"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case3_root_inside_direct_child_js() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let root = case3_layout();
    let src = root.join("examples/foo.td");
    let out_js = root.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&src)
        .arg("-o")
        .arg(&out_js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "JS must accept direct-child `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&out_js)
        .output()
        .expect("run node");
    assert!(run.status.success(), "node exit non-zero");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "direct-child-x"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case3_root_inside_direct_child_native() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let root = case3_layout();
    let src = root.join("examples/foo.td");
    let bin = root.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native must accept direct-child `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(run.status.success(), "native exit non-zero");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "direct-child-x"
    );
    let _ = fs::remove_dir_all(&root);
}

// ── Case 4: root_outside_absolute ──────────────────────────────────────

/// Set up a project root + an existing outside file at an absolute
/// path. Returns (project_root, project_main_td, outside_file_path).
fn case4_layout() -> (PathBuf, PathBuf, PathBuf) {
    let root = temp_root("c4_abs");
    let outside_dir = std::env::temp_dir().join(format!(
        "taida_c27b022_c4_outside_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&outside_dir);
    fs::create_dir_all(&outside_dir).unwrap();
    let outside_td = outside_dir.join("leak.td");
    fs::write(
        &outside_td,
        "secret n: Str =\n  \"leaked-\" + n\n=> :Str\n\n<<< @(secret)\n",
    )
    .unwrap();
    let main_td = root.join("main.td");
    fs::write(
        &main_td,
        format!(
            ">>> {} => @(secret)\n\nmsg <= secret(\"x\")\nstdout(msg)\n",
            outside_td.display()
        ),
    )
    .unwrap();
    (root, main_td, outside_dir)
}

/// Strict canonical-message extractor. Locates a *full* match of
/// the canonical rejection sentence in `combined` and returns
/// `(full_sentence, captured_path)`. Panics with diagnostic context
/// if no match is found — the OR-fallback of the previous lax
/// helper (`combined.contains("Import path '")` standalone) is
/// intentionally removed: every backend MUST surface the entire
/// canonical sentence with the exact import path embedded.
fn extract_canonical(combined: &str) -> Option<(String, String)> {
    canonical_reject_re()
        .captures(combined)
        .map(|c| (c[0].to_string(), c[1].to_string()))
}

/// Build the byte-exact canonical sentence that every backend
/// MUST emit for a given offending import path. Mirrors the
/// `format!(...)` calls in:
/// - `src/interpreter/module_eval.rs::resolve_import_path`
/// - `src/js/codegen.rs::resolve_local_import_js_path`
/// - `src/codegen/driver.rs::resolve_module_path` (and the
///   secondary site at `:1252`).
///
/// The literal whitespace block between `root.` and `Path` is
/// `" " + "                         "` (1 separator space + 25
/// indent spaces preserved by `\<newline>` line continuation).
fn expected_canonical(escape_path: &Path) -> String {
    format!(
        "Import path '{}' resolves outside the project root. \
                         Path traversal beyond the project boundary is not allowed.",
        escape_path.display()
    )
}

/// Strict 3-backend assertion: extract the canonical sentence,
/// require the captured path to equal `escape_path.display()`,
/// and require the full sentence to equal `expected_canonical`.
/// Returns `(full, path)` so callers can assert pairwise eq across
/// backends within their fixture lifetime.
///
/// **`escape_path` semantics**: this is the *lexical* import token
/// embedded in the rejection message, NOT necessarily the resolved
/// filesystem path. Case 4 (absolute-path import) has the absolute
/// outside path as the lexical token and they coincide; Case 5
/// (relative `..` escape) has `../../sibling/leak.td` as the
/// lexical token while the resolved path is the absolute leak
/// location — Case 5 callers MUST pass the lexical token.
fn assert_canonical_reject_strict(
    label: &str,
    combined: &str,
    escape_path: &Path,
) -> (String, String) {
    let (full, path) = extract_canonical(combined).unwrap_or_else(|| {
        panic!(
            "{} rejection must contain the full canonical sentence \
             matching the regex `Import path '...' resolves outside \
             the project root\\.\\s+Path traversal beyond the project \
             boundary is not allowed\\.`; got: {}",
            label, combined
        )
    });
    let escape_disp = escape_path.display().to_string();
    assert_eq!(
        path, escape_disp,
        "{} captured import path must equal escape_path verbatim; \
         got captured='{}', expected='{}'; combined: {}",
        label, path, escape_disp, combined
    );
    let expected = expected_canonical(escape_path);
    assert_eq!(
        full, expected,
        "{} canonical sentence must be byte-identical to the \
         expected template; got: {:?}, expected: {:?}",
        label, full, expected
    );
    (full, path)
}

#[test]
fn c27b_022_case4_root_outside_absolute_interpreter() {
    let (root, main, outside_dir) = case4_layout();
    let outside_td = outside_dir.join("leak.td");
    let out = Command::new(taida_bin())
        .arg(&main)
        .output()
        .expect("run interp");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "interp must REJECT absolute path traversal; combined: {}",
        combined
    );
    // Strict canonical check: byte-identical to the template
    // derived from `escape_path`. Because every backend
    // (interpreter / JS / native) compares against the *same*
    // `expected_canonical(escape_path)` builder, transitive
    // equality across backends is enforced even though each
    // backend runs in its own `#[test]` process.
    assert_canonical_reject_strict("interpreter", &combined, &outside_td);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside_dir);
}

#[test]
fn c27b_022_case4_root_outside_absolute_js() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let (root, main, outside_dir) = case4_layout();
    let outside_td = outside_dir.join("leak.td");
    let out_js = root.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&main)
        .arg("-o")
        .arg(&out_js)
        .output()
        .expect("build js");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        !build.status.success(),
        "JS must REJECT absolute path traversal at build time; combined: {}",
        combined
    );
    assert_canonical_reject_strict("JS", &combined, &outside_td);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside_dir);
}

#[test]
fn c27b_022_case4_root_outside_absolute_native() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let (root, main, outside_dir) = case4_layout();
    let outside_td = outside_dir.join("leak.td");
    let bin = root.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&main)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        !build.status.success(),
        "native must REJECT absolute path traversal at build time; combined: {}",
        combined
    );
    assert_canonical_reject_strict("native", &combined, &outside_td);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside_dir);
}

// ── Case 5: root_outside_relative ──────────────────────────────────────

/// The `>>>` import token literal that Case 5 plants in
/// `project/examples/foo.td`. All 3 backends must surface this
/// **exact** lexical token (NOT the resolved absolute path) inside
/// the rejection message, because each rejection site formats with
/// the source `import_path` argument verbatim before resolution.
/// This contract is what makes Case 5 a true 3-backend byte-parity
/// pin: any backend that pre-canonicalises the path before
/// embedding it into the message would diverge here.
const CASE5_IMPORT_TOKEN: &str = "../../sibling/leak.td";

/// Outer dir contains `project/` and `sibling/`. `project/main.td`
/// imports `../../sibling/leak.td` — escapes the project root via
/// relative path. All 3 backends must reject with the canonical
/// message embedding the lexical token verbatim.
fn case5_layout() -> (PathBuf, PathBuf, PathBuf) {
    let outer = std::env::temp_dir().join(format!(
        "taida_c27b022_c5_outer_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&outer);
    fs::create_dir_all(outer.join("project/examples")).unwrap();
    fs::create_dir_all(outer.join("sibling")).unwrap();
    // Anchor the project root to project/, NOT outer/, so the
    // `..` walk truly escapes.
    fs::write(outer.join("project/taida.toml"), "").unwrap();
    let leak = outer.join("sibling/leak.td");
    fs::write(
        &leak,
        "secret n: Str =\n  \"leaked-\" + n\n=> :Str\n\n<<< @(secret)\n",
    )
    .unwrap();
    let main = outer.join("project/examples/foo.td");
    fs::write(
        &main,
        format!(
            ">>> {} => @(secret)\n\nmsg <= secret(\"x\")\nstdout(msg)\n",
            CASE5_IMPORT_TOKEN
        ),
    )
    .unwrap();
    (outer, main, leak)
}

#[test]
fn c27b_022_case5_root_outside_relative_interpreter() {
    let (outer, main, _leak) = case5_layout();
    let out = Command::new(taida_bin())
        .arg(&main)
        .output()
        .expect("run interp");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "interp must REJECT relative path traversal that escapes; combined: {}",
        combined
    );
    // Case 5 pins the *lexical* import token (not the resolved
    // absolute leak path) inside the rejection — that is the
    // 3-backend byte-parity contract for relative-token sites.
    assert_canonical_reject_strict("interpreter", &combined, Path::new(CASE5_IMPORT_TOKEN));
    let _ = fs::remove_dir_all(&outer);
}

#[test]
fn c27b_022_case5_root_outside_relative_js() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let (outer, main, _leak) = case5_layout();
    let out_js = outer.join("project/out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&main)
        .arg("-o")
        .arg(&out_js)
        .output()
        .expect("build js");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        !build.status.success(),
        "JS must REJECT relative path traversal that escapes; combined: {}",
        combined
    );
    assert_canonical_reject_strict("JS", &combined, Path::new(CASE5_IMPORT_TOKEN));
    let _ = fs::remove_dir_all(&outer);
}

#[test]
fn c27b_022_case5_root_outside_relative_native() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let (outer, main, _leak) = case5_layout();
    let bin = outer.join("project/out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&main)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        !build.status.success(),
        "native must REJECT relative path traversal that escapes; combined: {}",
        combined
    );
    assert_canonical_reject_strict("native", &combined, Path::new(CASE5_IMPORT_TOKEN));
    let _ = fs::remove_dir_all(&outer);
}
