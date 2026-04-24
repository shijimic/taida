//! C26B-018 (B) (@c.26, wK Round 4): byte-level primitives 3-backend parity.
//!
//! `ByteAt` / `ByteSlice` / `ByteLength` operate on the raw UTF-8 byte
//! stream of a `Str` value. They are **additive** molds (§ 6.2 widening) —
//! existing `CharAt` / `Slice` / `.length()` semantics are unchanged
//! (still Unicode-scalar on the surface).
//!
//! # Semantics (consistent across backends)
//!
//!   - `ByteAt[str, idx]()`       → `Lax[Int]` — byte value at idx (0..=255),
//!                                  empty Lax if OOB. O(1).
//!   - `ByteSlice[str, s, e]()`   → `Str` — byte-range slice. If the slice
//!                                  cuts a UTF-8 sequence mid-codepoint,
//!                                  the interp/native backends return the
//!                                  raw bytes as-is (lossy where needed);
//!                                  JS uses TextDecoder (non-fatal) which
//!                                  matches when the slice lands on a
//!                                  codepoint boundary. All test fixtures
//!                                  use boundary-safe offsets.
//!   - `ByteLength[str]()`        → `Int` — UTF-8 byte length. O(1).
//!
//! # Backend implementation
//!
//!   - Interpreter: `src/interpreter/mold_eval.rs` (ByteAt / ByteSlice /
//!                  ByteLength handlers)
//!   - JS:          `src/js/runtime/core.rs` (`function ByteAt` / `ByteSlice`
//!                  / `ByteLength`, TextEncoder-backed)
//!   - Native:      `src/codegen/lower_molds.rs` + `src/codegen/native_runtime/core.c`
//!                  (`taida_str_byte_at_lax` / `taida_str_byte_slice` /
//!                  `taida_str_byte_length`)

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

fn write_fixture(tag: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "c26b_018_byteprim_{}_{}_{}",
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

/// `ByteAt`: ASCII access (hot path).
#[test]
fn c26b_018_byte_at_ascii_parity() {
    // "GET" → 0x47, 0x45, 0x54
    let source = r#"
s <= "GET"
b0Lax <= ByteAt[s, 0]()
b0Lax ]=> b0
stdout(b0)
b1Lax <= ByteAt[s, 1]()
b1Lax ]=> b1
stdout(b1)
b2Lax <= ByteAt[s, 2]()
b2Lax ]=> b2
stdout(b2)
"#;
    parity_assert("ascii", source, "71\n69\n84");
}

/// `ByteAt`: out-of-bounds → Lax(hasValue=false, default 0).
#[test]
fn c26b_018_byte_at_oob_parity() {
    let source = r#"
s <= "ab"
oobLax <= ByteAt[s, 5]()
oobLax ]=> oob
stdout(oob)
negLax <= ByteAt[s, -1]()
negLax ]=> neg
stdout(neg)
"#;
    parity_assert("oob", source, "0\n0");
}

/// `ByteSlice`: ASCII-boundary-safe slicing.
#[test]
fn c26b_018_byte_slice_ascii_parity() {
    let source = r#"
s <= "GET /api HTTP/1.1"
method <= ByteSlice[s, 0, 3]()
stdout(method)
path <= ByteSlice[s, 4, 8]()
stdout(path)
version <= ByteSlice[s, 9, 17]()
stdout(version)
"#;
    parity_assert("slice_ascii", source, "GET\n/api\nHTTP/1.1");
}

/// `ByteSlice`: out-of-bounds clamps to valid range.
#[test]
fn c26b_018_byte_slice_clamp_parity() {
    let source = r#"
s <= "hello"
// end > len → clamped to len
t1 <= ByteSlice[s, 0, 100]()
stdout(t1)
// start > end → swap (returns reversed range as empty-or-forward;
// we define the semantic as swap → forward slice)
t2 <= ByteSlice[s, 3, 1]()
stdout(t2.length())
// negative start → clamped to 0
t3 <= ByteSlice[s, -5, 3]()
stdout(t3)
"#;
    parity_assert("slice_clamp", source, "hello\n2\nhel");
}

/// `ByteLength`: ASCII-only strings match `.length()`.
#[test]
fn c26b_018_byte_length_ascii_parity() {
    let source = r#"
s <= "GET /api HTTP/1.1"
stdout(ByteLength[s]())
empty <= ""
stdout(ByteLength[empty]())
"#;
    parity_assert("bytelen_ascii", source, "17\n0");
}

/// `ByteLength`: non-ASCII strings report UTF-8 byte count (distinct from
/// `.length()` which counts Unicode scalars in the interp/native backends).
/// This is the raison d'être of `ByteLength` — hot-path byte parsers need
/// byte count, not codepoint count.
#[test]
fn c26b_018_byte_length_utf8_parity() {
    let source = r#"
// "café" — 4 codepoints, 5 UTF-8 bytes (é = 0xC3 0xA9)
s <= "café"
stdout(ByteLength[s]())
"#;
    parity_assert("bytelen_utf8", source, "5");
}

/// Combined hot-path: parse HTTP method from a string using ByteAt for
/// first-char dispatch + ByteSlice for span extraction. Mimics the
/// tightest loop in a request-line parser.
#[test]
fn c26b_018_hot_path_http_parse_parity() {
    let source = r#"
line <= "POST /api/users HTTP/1.1"
// 0x47='G' 0x50='P' 0x48='H'
firstLax <= ByteAt[line, 0]()
firstLax ]=> first
stdout(first)
// POST takes 4 chars
method <= ByteSlice[line, 0, 4]()
stdout(method)
stdout(ByteLength[line]())
"#;
    parity_assert("hot_http", source, "80\nPOST\n24");
}
