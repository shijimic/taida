//! C26B-023 (@c.26, Cluster 6 Surface): 2-arg `httpServe` handler `req.body`
//! silent breakage — 3-backend parity pin for the **correct** `readBody(req)`
//! path.
//!
//! # Context
//!
//! A 2-arg `httpServe` handler receives a req pack where `req.body` is
//! **intentionally** empty (`@(start: bodyOffset, len: 0)`) because the runtime
//! defers body read until the handler explicitly asks for it via
//! `readBody(req)` / `readBodyChunk(req)` / `readBodyAll(req)`. This enables
//! streaming but creates a silent breakage when users migrate a 1-arg handler
//! (where `req.body` was the buffered body span) to 2-arg without rewriting the
//! body extraction to `readBody`.
//!
//! See `docs/reference/net_api.md §8` for the full pattern guide.
//!
//! # What this file pins
//!
//! Rather than spinning up a live HTTP server (flaky in CI — see C26B-003),
//! this file builds **synthetic request packs** that exercise the two body
//! shapes:
//!
//!   1. 1-arg-shape: `req.body = @(start, len)` with `len > 0` — `readBody`
//!      returns the slice of `req.raw`.
//!   2. Empty body (GET): `req.body = @(start, len=0)` — `readBody` returns
//!      empty Bytes.
//!
//! The "2-arg-shape with live stream" case (requires `__body_stream` sentinel
//! and an active socket) is covered by the existing
//! `tests/parity.rs::test_net_readbody_*` tests and is intentionally NOT
//! duplicated here — this file is a docs-path parity pin, not a replacement.
//!
//! # Docs amendment pair
//!
//! `docs/reference/net_api.md §8` was updated in the same commit to add the
//! 2-arg body handling pattern guide (correct vs anti-pattern + implementation
//! references). When this file changes, also keep §8 in sync.

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
        "c26b_023_{}_{}_{}",
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

/// C26B-023 P1: `readBody(req)` on a synthetic 1-arg-shape req pack with a
/// buffered body span. All three backends should return the 5-byte "hello"
/// slice. This pins the docs example in `net_api.md §8.1`.
#[test]
fn c26b_023_readbody_buffered_body_parity() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["POST /echo HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 42, len <= 5))
body <= readBody(req)
stdout(body.length())
bodyStr <= Utf8Decode[body]().getOrDefault("")
stdout(bodyStr)
"#;
    parity_assert("readbody_buffered", source, "5\nhello");
}

/// C26B-023 P2: `readBody(req)` on a GET-shape pack (body span `len=0`).
/// All three backends should return empty Bytes.
#[test]
fn c26b_023_readbody_empty_body_parity() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 35, len <= 0))
body <= readBody(req)
stdout(body.length())
"#;
    parity_assert("readbody_empty", source, "0");
}

/// C26B-023 P3: Anti-pattern regression guard — the *wrong* direct-slice path
/// on a 2-arg-shape req pack (body.len=0) returns empty Bytes across all 3
/// backends. This is **silent breakage** (intentionally reproduced here so we
/// have a docs-aligned fixture pinning the behavior). The correct fix is to
/// use `readBody(req)` (P1/P2 above) — this test just verifies that the
/// anti-pattern behaves identically across backends (i.e. silent breakage is
/// uniform, not a 3-backend divergence).
#[test]
fn c26b_023_anti_pattern_direct_slice_empty_parity() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["POST /echo HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"]()
rawLax ]=> raw
// 2-arg-shape simulation: body span has len=0 even though Content-Length > 0.
// The direct-slice path below incorrectly returns empty bytes (silent
// breakage). This fixture pins that behavior so a future C26B-023 runtime
// warning land can reference this as the "before" state.
req <= @(raw <= raw, body <= @(start <= 42, len <= 0))
bodyBytes <= Slice[req.raw](start <= req.body.start, end <= req.body.start + req.body.len)
stdout(bodyBytes.length())
"#;
    parity_assert("anti_pattern_slice", source, "0");
}
