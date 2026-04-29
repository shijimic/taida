//! C26B-016 (@c.26, Option B+): `StrOf[span, raw]()` 3-backend parity.
//!
//! Cold-path counterpart to the Span* comparison molds (SpanEquals /
//! SpanStartsWith / SpanContains / SpanSlice) tracked in
//! `tests/c26b_016_span_aware_mold.rs`. `StrOf` materializes a span pack
//! `@(start: Int, len: Int)` over a raw Bytes/Str into an owned `Str`,
//! intended for log output / debug / JSON parsing (anywhere the user needs
//! an owned string copy rather than a zero-copy view).
//!
//! # Tolerant semantics (consistent across backends)
//!
//! - Invalid UTF-8 span content  → empty `""`.
//! - Out-of-bounds span          → empty `""`.
//! - `len == 0`                  → empty `""`.
//!
//! This matches the Span* family pattern: hot-path boolean molds return
//! `false` on invalid input, cold-path materializers return empty Str.
//!
//! # Backend implementation
//!
//! - Interpreter: `src/interpreter/mold_eval.rs::StrOf`
//! - JS: `src/js/runtime/net.rs::__taida_net_StrOf` + codegen rewrite
//! - Native: `src/codegen/lower_molds.rs::StrOf` (IR composition using
//!   existing `taida_pack_get` + `taida_slice_mold` +
//!   `taida_utf8_decode_mold` + `taida_lax_get_or_default` —
//!   no new C runtime helper, avoiding core.c churn during
//!   Round 3 co-ordination with wG/wI).
//!
//! See `docs/reference/net_api.md §4.1` for the canonical docs reference.

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
        "c26b_016_strof_{}_{}_{}",
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

/// Basic StrOf: extract "GET" from a span over a Bytes buffer.
#[test]
fn c26b_016_strof_basic_parity() {
    let source = r#"
rawLax <= Bytes["GET /api HTTP/1.1"]()
rawLax ]=> raw
method <= @(start <= 0, len <= 3)
m <= StrOf[method, raw]()
stdout(m)
stdout(m.length())
"#;
    parity_assert("basic", source, "GET\n3");
}

/// Path span materialization — longer substring with special chars.
#[test]
fn c26b_016_strof_path_parity() {
    let source = r#"
rawLax <= Bytes["GET /api/users/42 HTTP/1.1"]()
rawLax ]=> raw
path <= @(start <= 4, len <= 13)
p <= StrOf[path, raw]()
stdout(p)
"#;
    parity_assert("path", source, "/api/users/42");
}

/// Raw as Str (not Bytes): StrOf should accept both.
#[test]
fn c26b_016_strof_str_raw_parity() {
    let source = r#"
raw <= "GET /api HTTP/1.1"
method <= @(start <= 0, len <= 3)
m <= StrOf[method, raw]()
stdout(m)
"#;
    parity_assert("str_raw", source, "GET");
}

/// Zero-length span → empty string.
#[test]
fn c26b_016_strof_empty_span_parity() {
    let source = r#"
rawLax <= Bytes["GET /api HTTP/1.1"]()
rawLax ]=> raw
empty <= @(start <= 0, len <= 0)
s <= StrOf[empty, raw]()
stdout(s.length())
"#;
    parity_assert("empty_span", source, "0");
}

/// Out-of-bounds span → empty string (tolerant, no panic).
#[test]
fn c26b_016_strof_oob_parity() {
    let source = r#"
rawLax <= Bytes["GET /api HTTP/1.1"]()
rawLax ]=> raw
oob <= @(start <= 100, len <= 4)
s <= StrOf[oob, raw]()
stdout(s.length())
"#;
    parity_assert("oob", source, "0");
}

/// Mixed hot/cold path — SpanEquals for hot, StrOf for cold. This matches
/// the canonical `docs/reference/net_api.md §4.6` guidance table.
#[test]
fn c26b_016_strof_hot_cold_integration_parity() {
    let source = r#"
rawLax <= Bytes["POST /api/users HTTP/1.1"]()
rawLax ]=> raw
method <= @(start <= 0, len <= 4)
path <= @(start <= 5, len <= 10)
// hot path: byte-exact comparison (no allocation)
isPost <= SpanEquals[method, raw, "POST"]()
// cold path: materialize for logging
pathStr <= StrOf[path, raw]()
stdout(isPost)
stdout(pathStr)
"#;
    parity_assert("hot_cold", source, "true\n/api/users");
}
