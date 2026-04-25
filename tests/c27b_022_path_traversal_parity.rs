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
//! 3. **root_inside_boundary** — `examples/foo.td` importing
//!    `../taida.toml-adjacent/bar.td` where the resolved path is
//!    exactly the project-root direct child. All 3 backends must
//!    accept.
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

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CANONICAL_REJECT: &str = "resolves outside the project root. \
     Path traversal beyond the project boundary is not allowed.";

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

// ── Case 3: root_inside_boundary ───────────────────────────────────────

/// `examples/foo.td` → `>>> ../bar.td`. The resolved target sits as
/// a direct child of the project root (exactly at the boundary). All
/// 3 backends must accept.
fn case3_layout() -> PathBuf {
    let root = temp_root("c3_boundary");
    fs::create_dir_all(root.join("examples")).unwrap();
    fs::write(
        root.join("bar.td"),
        "greet n: Str =\n  \"boundary-\" + n\n=> :Str\n\n<<< @(greet)\n",
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
fn c27b_022_case3_root_inside_boundary_interpreter() {
    let root = case3_layout();
    let src = root.join("examples/foo.td");
    let out = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp must accept boundary `..` import; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "boundary-x");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case3_root_inside_boundary_js() {
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
        "JS must accept boundary `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&out_js)
        .output()
        .expect("run node");
    assert!(run.status.success(), "node exit non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "boundary-x");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn c27b_022_case3_root_inside_boundary_native() {
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
        "native must accept boundary `..` import; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(run.status.success(), "native exit non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "boundary-x");
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

fn assert_canonical_reject(combined: &str, label: &str, escape_path: &Path) {
    assert!(
        combined.contains(CANONICAL_REJECT),
        "{} rejection message must contain canonical phrase '{}'; got: {}",
        label,
        CANONICAL_REJECT,
        combined
    );
    let escape_disp = escape_path.display().to_string();
    assert!(
        combined.contains(&escape_disp) || combined.contains("Import path '"),
        "{} rejection message must mention the offending import path '{}' or 'Import path '...''; got: {}",
        label,
        escape_disp,
        combined
    );
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
    assert_canonical_reject(&combined, "interpreter", &outside_td);
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
    assert_canonical_reject(&combined, "JS", &outside_td);
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
    assert_canonical_reject(&combined, "native", &outside_td);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside_dir);
}

// ── Case 5: root_outside_relative ──────────────────────────────────────

/// Outer dir contains `project/` and `sibling/`. `project/main.td`
/// imports `../../sibling/leak.td` — escapes the project root via
/// relative path. All 3 backends must reject with the canonical
/// message.
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
        ">>> ../../sibling/leak.td => @(secret)\n\nmsg <= secret(\"x\")\nstdout(msg)\n",
    )
    .unwrap();
    (outer, main, leak)
}

#[test]
fn c27b_022_case5_root_outside_relative_interpreter() {
    let (outer, main, leak) = case5_layout();
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
    assert_canonical_reject(&combined, "interpreter", &leak);
    let _ = fs::remove_dir_all(&outer);
}

#[test]
fn c27b_022_case5_root_outside_relative_js() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let (outer, main, leak) = case5_layout();
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
    assert_canonical_reject(&combined, "JS", &leak);
    let _ = fs::remove_dir_all(&outer);
}

#[test]
fn c27b_022_case5_root_outside_relative_native() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let (outer, main, leak) = case5_layout();
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
    assert_canonical_reject(&combined, "native", &leak);
    let _ = fs::remove_dir_all(&outer);
}
