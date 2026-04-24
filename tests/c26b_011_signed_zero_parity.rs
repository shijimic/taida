//! C26B-011 (wS Round 6, 2026-04-24) — IEEE-754 signed-zero formatting
//! parity stretch.
//!
//! # Scope
//!
//! Pins interpreter (reference) ↔ Native parity for `-0.0` rendering.
//! Before the wS fix `taida_float_to_str` tested `a == 0.0` which is
//! true for both `+0.0` and `-0.0`, so Native silently dropped the
//! minus sign. The wS patch dispatches on `signbit(a)` so the output
//! matches Rust f64::Display → interpreter `format!("{:.1}", n)` for
//! both zeros.
//!
//! # JS follow-up (explicitly out of wS scope)
//!
//! The JS runtime's `__taida_float_render` was patched in wS to use
//! `Object.is(v, -0)` as the negative-zero probe (runtime change only,
//! safe / idempotent). However the JS codegen's Float-literal emission
//! currently produces integer-valued literals (e.g. `-1.0` → `-1`), so
//! `-1.0 * 0.0` lowers to `-1 * 0` and `__taida_mul` uses the Int fast
//! path, returning `+0`. That codegen-level gap is a distinct,
//! pre-existing issue and is out of scope for wS (a 2-hour auto-mode
//! stretch session). JS parity for signed-zero will be completed in a
//! follow-up round once the Float-literal emission path is revisited.
//!
//! # D27 escalation checklist (3 points, all NO)
//!
//! 1. No public mold signature changed.
//! 2. No STABILITY-pinned error string altered.
//! 3. Append-only: new test file, no existing assertion modified.

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

fn run_native(td_path: &Path) -> Option<String> {
    if !cc_available() {
        return None;
    }
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let bin_path = std::env::temp_dir().join(format!(
        "c26b011_sz_{}_{}.bin",
        std::process::id(),
        stem
    ));
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
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
