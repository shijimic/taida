//! C26B-016 (@c.26, Option B+): span-aware comparison mold 3-backend parity.
//!
//! The NET request pack (`httpServe` handler / `httpParseRequestHead`) exposes
//! `method` / `path` / `query` / header names/values / `body` as
//! **zero-copy span packs** `@(start: Int, len: Int)` over `req.raw: Bytes`.
//! This keeps the hot path allocation-free but makes user-level `req.method ==
//! "GET"` impossible (Option A would require breaking the existing `body <=
//! req.method` tests — D27 送り).
//!
//! Option B+ adds a family of span-aware comparison molds so routers can match
//! method / path without materializing a new `Str`:
//!
//!   - `SpanEquals[span, raw, needle]() -> Bool`     (byte-exact match)
//!   - `SpanStartsWith[span, raw, prefix]() -> Bool` (prefix match)
//!   - `SpanContains[span, raw, needle]() -> Bool`   (substring existence)
//!   - `SpanSlice[span, raw, start, end]() -> Pack`  (sub-span @(start, len))
//!
//! `strOf(span, raw)` (the cold-path materializer) is tracked separately in
//! `docs/reference/net_api.md §4.1`; this file covers the 4 Span* molds that
//! are implemented uniformly across interpreter / JS / native.
//!
//! # 3-backend parity protocol
//!
//! Each test writes a single `.td` fixture, runs it through all three
//! backends, and asserts byte-identical stdout. This matches the pattern used
//! by `c26b_015_path_traversal_parity.rs` and `c26b_021_stdout_flush_parity.rs`.

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

/// Write `source` to `<tmpdir>/span_<tag>.td` and return the path. Each test
/// gets its own tmpdir so parallel execution doesn't collide.
fn write_fixture(tag: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "c26b_016_{}_{}_{}",
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

fn run_js(src: &PathBuf, dir: &PathBuf) -> Option<String> {
    if !node_available() {
        eprintln!("node unavailable; skipping JS leg");
        return None;
    }
    let js = dir.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
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

fn run_native(src: &PathBuf, dir: &PathBuf) -> Option<String> {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native leg");
        return None;
    }
    let bin = dir.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
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

/// SpanEquals: positive / negative / length mismatch / OOB.
#[test]
fn c26b_016_span_equals_parity() {
    let source = r#"
span <= @(start <= 4, len <= 4)
raw <= "GET /api HTTP/1.1"
stdout(SpanEquals[span, raw, "/api"]())
stdout(SpanEquals[span, raw, "/xyz"]())
stdout(SpanEquals[span, raw, "/ap"]())
stdout(SpanEquals[span, raw, "/apix"]())
oob <= @(start <= 100, len <= 4)
stdout(SpanEquals[oob, raw, "/api"]())
"#;
    parity_assert("span_equals", source, "true\nfalse\nfalse\nfalse\nfalse");
}

/// SpanStartsWith: positive / negative / equal-length positive / OOB.
#[test]
fn c26b_016_span_starts_with_parity() {
    let source = r#"
span <= @(start <= 4, len <= 8)
raw <= "GET /api/foo HTTP/1.1"
stdout(SpanStartsWith[span, raw, "/api"]())
stdout(SpanStartsWith[span, raw, "/xyz"]())
stdout(SpanStartsWith[span, raw, "/api/foo"]())
stdout(SpanStartsWith[span, raw, "/api/foo/bar"]())
"#;
    parity_assert(
        "span_starts",
        source,
        "true\nfalse\ntrue\nfalse",
    );
}

/// SpanContains: positive / empty-needle / negative.
#[test]
fn c26b_016_span_contains_parity() {
    let source = r#"
span <= @(start <= 4, len <= 8)
raw <= "GET /api/foo HTTP/1.1"
stdout(SpanContains[span, raw, "api"]())
stdout(SpanContains[span, raw, "foo"]())
stdout(SpanContains[span, raw, "xyz"]())
stdout(SpanContains[span, raw, ""]())
"#;
    parity_assert("span_contains", source, "true\ntrue\nfalse\ntrue");
}

/// SpanSlice: sub-span @(start, len) arithmetic.
#[test]
fn c26b_016_span_slice_parity() {
    let source = r#"
span <= @(start <= 4, len <= 8)
raw <= "GET /api/foo HTTP/1.1"
sub <= SpanSlice[span, raw, 1, 4]()
stdout(sub.start)
stdout(sub.len)
sub2 <= SpanSlice[span, raw, 0, 0]()
stdout(sub2.start)
stdout(sub2.len)
// Over-end clamps to base.len:
sub3 <= SpanSlice[span, raw, 2, 100]()
stdout(sub3.start)
stdout(sub3.len)
"#;
    parity_assert("span_slice", source, "5\n3\n4\n0\n6\n6");
}

/// Integrated: router-style method + path dispatch using SpanEquals and
/// SpanStartsWith together. Matches the expected hot-path ergonomics the
/// `docs/reference/net_api.md §4.6` indicator table recommends.
#[test]
fn c26b_016_router_style_integration_parity() {
    let source = r#"
method <= @(start <= 0, len <= 3)
path <= @(start <= 4, len <= 4)
raw <= "GET /api HTTP/1.1"
isGet <= SpanEquals[method, raw, "GET"]()
isApi <= SpanStartsWith[path, raw, "/api"]()
stdout(isGet)
stdout(isApi)
matched <= isGet && isApi
stdout(matched)
"#;
    parity_assert("router_style", source, "true\ntrue\ntrue");
}
