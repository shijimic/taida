//! C26B-018 (C) (@c.26, wK Round 4): `StringRepeatJoin` 3-backend parity.
//!
//! Single-allocation repeat+join primitive. Addresses the O(N²) reallocation
//! cascade observed in the pattern
//!
//! ```text
//! acc <= ""
//! 1..n => forEach(_) => acc <= acc + sep + item
//! ```
//!
//! which appears in terminal / hachikuma tui primitives (`repeatCh` +
//! column-join idioms) and in several documented Taida examples.
//! `StringRepeatJoin[str, n, sep]()` pre-computes the total length once
//! and allocates a single buffer.
//!
//! # Semantics (consistent across backends)
//!
//! - `n <= 0` → `""`
//! - `n == 1` → `str` (separator never emitted)
//! - `n >= 2` → `str + sep + str + sep + ... + str` (n copies, n-1 seps)
//!
//! # Backend implementation
//!
//! - Interpreter: `src/interpreter/mold_eval.rs::StringRepeatJoin`
//! - JS: `src/js/runtime/core.rs::StringRepeatJoin`
//!   (uses `String#repeat` when sep is empty,
//!   `Array(n).fill(str).join(sep)` otherwise)
//! - Native: `src/codegen/lower_molds.rs::StringRepeatJoin` →
//!   `taida_str_repeat_join` in `core.c` (precomputes
//!   total length, single `taida_str_alloc`)

mod common;

use std::fs;
use std::path::{Path, PathBuf};
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

fn write_fixture(tag: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "c26b_018_repjoin_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir tmpdir");
    let src = dir.join("fixture.td");
    fs::write(&src, source).expect("write fixture");
    (dir, src)
}

fn run_interp(src: &PathBuf) -> String {
    let out = Command::new(taida_bin())
        .arg(src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_js(src: &Path, dir: &Path) -> Option<String> {
    if !node_available() {
        eprintln!("node unavailable; skipping JS leg");
        return None;
    }
    let js = dir.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(src)
        .arg("-o")
        .arg(&js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node").arg(&js).output().expect("run js");
    assert!(
        run.status.success(),
        "js run failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
}

fn run_native(src: &Path, dir: &Path) -> Option<String> {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native leg");
        return None;
    }
    let bin = dir.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(
        run.status.success(),
        "native run failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
}

fn parity_assert(tag: &str, source: &str, expected: &str) {
    let (dir, src) = write_fixture(tag, source);
    let interp = run_interp(&src);
    assert_eq!(interp, expected, "interp mismatch ({tag})");
    if let Some(js) = run_js(&src, &dir) {
        assert_eq!(js, expected, "js mismatch ({tag})");
    }
    if let Some(native) = run_native(&src, &dir) {
        assert_eq!(native, expected, "native mismatch ({tag})");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Basic: "ab" × 3 joined by "," → "ab,ab,ab".
#[test]
fn c26b_018_srj_basic_parity() {
    let source = r#"
out <= StringRepeatJoin["ab", 3, ","]()
stdout(out)
"#;
    parity_assert("basic", source, "ab,ab,ab");
}

/// Empty separator: behaves like `Repeat`.
#[test]
fn c26b_018_srj_empty_sep_parity() {
    let source = r#"
out <= StringRepeatJoin["x", 5, ""]()
stdout(out)
stdout(out.length())
"#;
    parity_assert("empty_sep", source, "xxxxx\n5");
}

/// n == 1: separator never appears.
#[test]
fn c26b_018_srj_n_one_parity() {
    let source = r#"
out <= StringRepeatJoin["solo", 1, "---"]()
stdout(out)
"#;
    parity_assert("n_one", source, "solo");
}

/// n <= 0: empty result (both 0 and negative treated as 0).
#[test]
fn c26b_018_srj_n_zero_parity() {
    let source = r#"
z <= StringRepeatJoin["a", 0, ","]()
stdout(z.length())
neg <= StringRepeatJoin["a", -5, ","]()
stdout(neg.length())
"#;
    parity_assert("n_zero", source, "0\n0");
}

/// Multi-char separator with multi-char base.
#[test]
fn c26b_018_srj_multichar_parity() {
    let source = r#"
out <= StringRepeatJoin["hello", 4, " -> "]()
stdout(out)
"#;
    parity_assert("multichar", source, "hello -> hello -> hello -> hello");
}

/// Tui-style column separator (the concrete use-case that motivated this
/// mold — hachikuma `tui_screen.td::repeatCh` and column-join idioms).
#[test]
fn c26b_018_srj_tui_column_parity() {
    let source = r#"
// 80-col rule drawn as 20 dashes × 4 joined by "+"
segment <= "----"
rule <= StringRepeatJoin[segment, 4, "+"]()
stdout(rule)
stdout(rule.length())
"#;
    // 4 segments of 4 dashes + 3 joiners = 16 + 3 = 19 bytes
    parity_assert("tui_column", source, "----+----+----+----\n19");
}
