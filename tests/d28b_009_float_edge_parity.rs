//! D28B-009 (2026-04-26, wD Round 1) — Float edge case 4-backend parity.
//!
//! Pins NaN / ±Infinity / signed-zero / denormal behaviour across the
//! interpreter (reference), JS, native, and wasm-wasi backends.
//!
//! Background: C26B-011 pinned the same edge cases for 3 backends
//! (interp / JS / native) but explicitly excluded wasm because the
//! wasi/full mod_mold_f and the wasm core float renderer (fmt_g)
//! diverged from the libc fmod / Rust f64::Display contracts.
//!
//! D28B-009 closes the wasi/full gap with two co-located fixes in
//! `runtime_wasi_io.c`:
//!   1. `taida_mod_mold_f` is aligned to libc fmod for NaN / ±Inf
//!      inputs (NaN propagates, Mod[finite, ±Inf] = finite).
//!   2. New `taida_debug_float_d28b009` / `taida_float_to_str_d28b009`
//!      check NaN BEFORE extracting the sign bit, so a signed-NaN
//!      bit-pattern (e.g. from `Sqrt[-1.0]`) renders as canonical "NaN"
//!      not "-NaN". Activated via `#define` in `emit_wasm_c.rs` for
//!      the Wasi and Full profiles.
//!
//! wasm-min is intentionally not in scope (it has no IEEE renderer
//! override and the C26B-011 "3-backend" contract spelled out the
//! exclusion). wasm-full is included as a regression guard because
//! it links rt_wasi.

mod common;

use common::{taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/quality/d28_float_edge")
}

fn read_expected(stem: &str) -> String {
    let path = fixture_dir().join(format!("{}.expected", stem));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing fixture {}", path.display()));
    content.trim_end().to_string()
}

fn normalize_stdout(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

fn run_interpreter(td: &Path) -> Option<String> {
    let out = Command::new(taida_bin()).arg(td).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "interpreter failed for {}: {}",
            td.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(normalize_stdout(&String::from_utf8_lossy(&out.stdout)))
}

fn run_native(td: &Path, label: &str) -> Option<String> {
    let exe: PathBuf =
        std::env::temp_dir().join(format!("d28b009_native_{}_{}", std::process::id(), label));
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td)
        .arg("-o")
        .arg(&exe)
        .output()
        .ok()?;
    if !build.status.success() {
        eprintln!(
            "native compile failed for {}: {}",
            td.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }
    let out = Command::new(&exe).output().ok()?;
    let _ = std::fs::remove_file(&exe);
    if !out.status.success() {
        return None;
    }
    Some(normalize_stdout(&String::from_utf8_lossy(&out.stdout)))
}

fn run_js(td: &Path, label: &str) -> Option<String> {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIP: node unavailable");
        return None;
    }
    let js = std::env::temp_dir().join(format!("d28b009_js_{}_{}.mjs", std::process::id(), label));
    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(td)
        .arg("-o")
        .arg(&js)
        .output()
        .ok()?;
    if !build.status.success() {
        eprintln!(
            "js compile failed for {}: {}",
            td.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }
    let out = Command::new("node").arg(&js).output().ok()?;
    let _ = std::fs::remove_file(&js);
    if !out.status.success() {
        return None;
    }
    Some(normalize_stdout(&String::from_utf8_lossy(&out.stdout)))
}

fn compile_wasm(td: &Path, target: &str, out: &Path) -> Result<(), String> {
    let output = Command::new(taida_bin())
        .args(["build", target])
        .arg(td)
        .arg("-o")
        .arg(out)
        .output()
        .map_err(|e| format!("spawn taida failed: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

fn run_wasm(wasm: &Path, wasmtime: &Path) -> Option<String> {
    let out = Command::new(wasmtime)
        .args(["run", "--"])
        .arg(wasm)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "wasmtime exec failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(normalize_stdout(&String::from_utf8_lossy(&out.stdout)))
}

fn check_4backend(stem: &str) {
    let td = fixture_dir().join(format!("{}.td", stem));
    let expected = read_expected(stem);

    let interp = run_interpreter(&td).expect("interpreter run");
    assert_eq!(interp, expected, "interpreter mismatch for {}", stem);

    let native = run_native(&td, stem).expect("native run");
    assert_eq!(native, expected, "native parity for {}", stem);

    if let Some(js) = run_js(&td, stem) {
        assert_eq!(js, expected, "JS parity for {}", stem);
    }

    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable -- wasm-wasi parity not verified");
            return;
        }
    };

    let wasi =
        std::env::temp_dir().join(format!("d28b009_{}_{}_wasi.wasm", std::process::id(), stem));
    compile_wasm(&td, "wasm-wasi", &wasi).expect("wasm-wasi compile (D28B-009)");
    let wasi_out = run_wasm(&wasi, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasi);
    assert_eq!(wasi_out, expected, "wasm-wasi parity for {}", stem);

    // wasm-full also links rt_wasi, so the override applies. Regression
    // guard so the same renderer / mod_mold_f override holds for full.
    let full =
        std::env::temp_dir().join(format!("d28b009_{}_{}_full.wasm", std::process::id(), stem));
    compile_wasm(&td, "wasm-full", &full).expect("wasm-full compile (D28B-009)");
    let full_out = run_wasm(&full, &wasmtime).expect("wasm-full run");
    let _ = std::fs::remove_file(&full);
    assert_eq!(full_out, expected, "wasm-full parity for {}", stem);
}

/// NaN propagation through Sqrt / Div / Mod / + / * / -. wasi pre-fix
/// rendered the result of `Sqrt[-1.0]` as "-NaN" because `fmt_g`
/// extracted the sign bit before the NaN check, and silently returned
/// 0.0 from `Mod[NaN, 3.0]` because `taida_mod_mold_f` rejected non-
/// finite inputs.
#[test]
fn d28b_009_nan_propagation_4backend_parity() {
    check_4backend("nan_propagation");
}

/// ±Infinity arithmetic, including `Mod[finite, ±Inf] = finite` which
/// matches libc fmod / native runtime. wasi pre-fix returned 0.0.
#[test]
fn d28b_009_inf_arithmetic_4backend_parity() {
    check_4backend("inf_arithmetic");
}

/// Subnormal-bracket boolean behaviour. Avoids the textual rendering
/// divergence (interpreter uses Rust `f64::Display` fixed-form, native
/// / JS use shortest-form scientific) which is a separate, longer-term
/// renderer-rewrite question and not in D28B-009 scope.
#[test]
fn d28b_009_denormal_compare_4backend_parity() {
    check_4backend("denormal_compare");
}

/// Regression: existing C26B-011 NaN/Inf 3-backend fixture is now
/// 4-backend pin (wasi was previously skipped per `c26b011_float_parity.rs`).
#[test]
fn d28b_009_c26b011_nan_inf_pin_4backend() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_float_edge/nan_inf_parity.td");
    let expected_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_float_edge/nan_inf_parity.expected");
    let expected = std::fs::read_to_string(&expected_path)
        .expect("missing nan_inf_parity.expected")
        .trim_end()
        .to_string();

    let interp = run_interpreter(&td).expect("interpreter run");
    assert_eq!(interp, expected, "interpreter mismatch (nan_inf_parity)");
    let native = run_native(&td, "c26b011_nan_inf").expect("native run");
    assert_eq!(native, expected, "native parity (nan_inf_parity)");
    if let Some(js) = run_js(&td, "c26b011_nan_inf") {
        assert_eq!(js, expected, "JS parity (nan_inf_parity)");
    }
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable -- wasm-wasi parity not verified");
            return;
        }
    };
    let wasi =
        std::env::temp_dir().join(format!("d28b009_c26nan_{}_wasi.wasm", std::process::id()));
    compile_wasm(&td, "wasm-wasi", &wasi).expect("wasm-wasi compile");
    let wasi_out = run_wasm(&wasi, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasi);
    assert_eq!(wasi_out, expected, "wasm-wasi parity (nan_inf_parity)");
}

/// Regression: C26B-011 signed-zero parity is now 4-backend.
#[test]
fn d28b_009_c26b011_signed_zero_pin_4backend() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_float_edge/signed_zero_parity.td");
    let expected_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_float_edge/signed_zero_parity.expected");
    let expected = std::fs::read_to_string(&expected_path)
        .expect("missing signed_zero_parity.expected")
        .trim_end()
        .to_string();

    let interp = run_interpreter(&td).expect("interpreter run");
    assert_eq!(interp, expected, "interpreter mismatch (signed_zero)");
    let native = run_native(&td, "c26b011_signed_zero").expect("native run");
    assert_eq!(native, expected, "native parity (signed_zero)");
    if let Some(js) = run_js(&td, "c26b011_signed_zero") {
        assert_eq!(js, expected, "JS parity (signed_zero)");
    }
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable -- wasm-wasi parity not verified");
            return;
        }
    };
    let wasi = std::env::temp_dir().join(format!("d28b009_c26sz_{}_wasi.wasm", std::process::id()));
    compile_wasm(&td, "wasm-wasi", &wasi).expect("wasm-wasi compile");
    let wasi_out = run_wasm(&wasi, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasi);
    assert_eq!(wasi_out, expected, "wasm-wasi parity (signed_zero)");
}
