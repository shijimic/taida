//! C26B-011 (wS Round 6 + wV-a Round 7, 2026-04-24) — IEEE-754
//! signed-zero formatting parity.
//!
//! # Scope
//!
//! Pins 3-backend parity (interpreter reference ↔ JS ↔ Native) for
//! `-0.0` rendering via `debug`. Before wS, `taida_float_to_str`
//! tested `a == 0.0` which is true for both `+0.0` and `-0.0`, so
//! Native silently dropped the minus sign; wS dispatched on
//! `signbit(a)` so Native matches Rust f64::Display.
//!
//! # wV-a Round 7 JS completion
//!
//! The JS runtime's `__taida_float_render` was patched in wS to use
//! `Object.is(v, -0)` as the negative-zero probe (runtime change
//! only, safe / idempotent). This wV-a follow-up lands two JS-side
//! completions so the JS backend joins the parity:
//!
//!  1. `__taida_float_render` now renders `-0` integer-valued floats
//!     as `"-0.0"` (the previous `(-0).toFixed(1) === "0.0"` drift
//!     is fixed).
//!  2. `__taida_mul` preserves the negative-zero sign bit when either
//!     operand is `-0` or the Number-path product is `-0`, rather
//!     than routing through the BigInt fast-path (BigInt has no
//!     `-0` concept, so `BigInt(-1) * BigInt(0) = 0n` collapses the
//!     sign).
//!
//! Together these ensure `-1.0 * 0.0` renders as `"-0.0"` across all
//! three backends.
//!
//! # D27 escalation checklist (3 points, all NO)
//!
//! 1. No public mold signature changed. Runtime helpers are internal
//!    (`__taida_*` namespace, non-contractual).
//! 2. No STABILITY-pinned error string altered.
//! 3. Append-only: the existing `signed_zero_interpreter_reference`
//!    and `signed_zero_native_parity` assertions are preserved;
//!    `signed_zero_js_parity` is added alongside.

mod common;

use common::{normalize, taida_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new("examples/quality/c26_float_edge/signed_zero_parity.td").to_path_buf()
}

fn read_expected() -> String {
    let path = Path::new("examples/quality/c26_float_edge/signed_zero_parity.expected");
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("missing fixture {}", path.display()));
    normalize(&content)
}

fn run_interpreter(td_path: &Path) -> Option<String> {
    let out = Command::new(taida_bin()).arg(td_path).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "interpreter failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&out.stdout)))
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

fn run_js(td_path: &Path) -> Option<String> {
    if !node_available() {
        return None;
    }
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let js_path =
        std::env::temp_dir().join(format!("c26b011_sz_{}_{}.mjs", std::process::id(), stem));
    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;
    if !build.status.success() {
        eprintln!(
            "js build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        let _ = std::fs::remove_file(&js_path);
        return None;
    }
    let run = Command::new("node").arg(&js_path).output().ok()?;
    let _ = std::fs::remove_file(&js_path);
    if !run.status.success() {
        eprintln!(
            "js run failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn run_native(td_path: &Path) -> Option<String> {
    if !cc_available() {
        return None;
    }
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let bin_path =
        std::env::temp_dir().join(format!("c26b011_sz_{}_{}.bin", std::process::id(), stem));
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;
    if !build.status.success() {
        eprintln!(
            "native build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        let _ = std::fs::remove_file(&bin_path);
        return None;
    }
    let run = Command::new(&bin_path).output().ok()?;
    let _ = std::fs::remove_file(&bin_path);
    if !run.status.success() {
        eprintln!(
            "native run failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

#[test]
fn signed_zero_interpreter_reference() {
    let path = fixture();
    let got = run_interpreter(&path).expect("interpreter run");
    assert_eq!(
        got,
        read_expected(),
        "interpreter must render -0.0 as '-0.0' (Rust f64::Display parity)"
    );
}

#[test]
fn signed_zero_native_parity() {
    if !cc_available() {
        eprintln!("cc unavailable; skipping signed-zero native parity test");
        return;
    }
    let path = fixture();
    let native = run_native(&path).expect("native run");
    let expected = read_expected();
    assert_eq!(
        native, expected,
        "Native must match interpreter for -0.0 rendering (signbit path in taida_float_to_str)"
    );
}

#[test]
fn signed_zero_js_parity() {
    if !node_available() {
        eprintln!("node unavailable; skipping signed-zero JS parity test");
        return;
    }
    let path = fixture();
    let js = run_js(&path).expect("js run");
    let expected = read_expected();
    assert_eq!(
        js, expected,
        "JS must match interpreter for -0.0 rendering (Object.is(v, -0) path in __taida_float_render + signed-zero preservation in __taida_mul)"
    );
}
