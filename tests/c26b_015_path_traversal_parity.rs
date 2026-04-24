//! C26B-015: Native backend's path-traversal check must allow `..`
//! imports that stay inside the project root, rejecting only true
//! escapes. Pre-fix the guard at `src/codegen/driver.rs:688-720` fell
//! back to `module_path.contains("..")` whenever the target file could
//! not be canonicalized (typically because native-build runs the check
//! before staging source), which rejected the common
//! `examples/foo.td` → `>>> ../src/bar.td` form.
//!
//! # Fix
//!
//! `lexical_escapes_root()` now walks components of the target path,
//! popping on `..`, and asserts the lexical result remains inside the
//! project root. This accepts legitimate in-project traversals while
//! still rejecting real escapes.
//!
//! # 3-backend parity
//!
//! Interpreter and JS backends already accepted the form (they use a
//! different resolver path that canonicalizes after source is written).
//! This fix brings Native in line with the other two.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

fn project_layout(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_c26b015_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("create src");
    fs::create_dir_all(dir.join("examples")).expect("create examples");
    // Marker so find_project_root() anchors to this directory rather
    // than /tmp (which is above the tempdir).
    fs::write(dir.join("taida.toml"), "").expect("touch marker");
    fs::write(
        dir.join("src/bar.td"),
        "greet n: Str =\n  \"bar-\" + n\n=> :Str\n\n<<< @(greet)\n",
    )
    .expect("write src/bar.td");
    fs::write(
        dir.join("examples/foo.td"),
        ">>> ../src/bar.td => @(greet)\n\nmsg <= greet(\"x\")\nstdout(msg)\n",
    )
    .expect("write examples/foo.td");
    dir
}

fn escape_layout(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("taida_c26b015_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&root);
    // project/ is the "inside", sibling/ sits next to project (outside).
    fs::create_dir_all(root.join("project/examples")).expect("create project/examples");
    fs::create_dir_all(root.join("sibling")).expect("create sibling");
    fs::write(root.join("project/taida.toml"), "").expect("touch project marker");
    fs::write(
        root.join("sibling/bar.td"),
        "greet n: Str =\n  \"outside-\" + n\n=> :Str\n\n<<< @(greet)\n",
    )
    .expect("write sibling/bar.td");
    let foo = root.join("project/examples/foo.td");
    fs::write(
        &foo,
        ">>> ../../sibling/bar.td => @(greet)\n\nmsg <= greet(\"x\")\nstdout(msg)\n",
    )
    .expect("write examples/foo.td (escape)");
    (root, foo)
}

/// Core C26B-015 repro: in-project `../src/bar.td` import from
/// `examples/foo.td` must be accepted by the native backend. Pre-fix
/// this hit `<path traversal rejected: ../src/bar.td>`.
#[test]
fn c26b_015_native_accepts_in_project_parent_import() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let dir = project_layout("native_inproject");
    let src = dir.join("examples/foo.td");
    let bin = dir.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native build must accept ../src/bar.td (inside project root); \
         stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native bin");
    assert!(run.status.success(), "native exit non-zero");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "bar-x",
        "native stdout must be 'bar-x'"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Interpreter parity: the same layout must work on the interpreter
/// (it already did pre-fix; this test pins that the fix did not regress
/// it).
#[test]
fn c26b_015_interpreter_accepts_in_project_parent_import() {
    let dir = project_layout("interp_inproject");
    let src = dir.join("examples/foo.td");
    let run = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run interp");
    assert!(
        run.status.success(),
        "interp must accept ../src/bar.td; stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "bar-x"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// JS parity.
#[test]
fn c26b_015_js_accepts_in_project_parent_import() {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return;
    }
    let dir = project_layout("js_inproject");
    let src = dir.join("examples/foo.td");
    let js = dir.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&src)
        .arg("-o")
        .arg(&js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js build must accept ../src/bar.td; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node").arg(&js).output().expect("run js");
    assert!(run.status.success(), "node exit non-zero");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "bar-x"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// True escape: `../../sibling/bar.td` must still be rejected by the
/// native backend. This is the asymmetry the fix preserves — legitimate
/// in-project `..` is OK, but leaving the project tree remains a
/// blocker.
#[test]
fn c26b_015_native_rejects_true_escape() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping");
        return;
    }
    let (root, src) = escape_layout("native_escape");
    let bin = root.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        !build.status.success(),
        "native build must REJECT a true-escape `../../sibling/bar.td`; \
         stdout: {} / stderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        combined.contains("path traversal") || combined.contains("traversal"),
        "rejection message must mention 'traversal'; got: {}",
        combined
    );
    let _ = fs::remove_dir_all(&root);
}
