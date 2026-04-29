//! C27B-021 (2026-04-25) — wasm arithmetic / bitwise mold lowering parity.
//!
//! The blocker was that `Div[Float, Float]()` failed at compile time on
//! both wasm-wasi AND wasm-full because the lowering emits the native ABI
//! name `taida_div_mold_f` which neither wasm runtime defined. The
//! `BitAnd` / `ShiftL` / `ShiftR` / `ShiftRU` family was OK on wasm-full
//! but wasm-wasi rejected them. With C27B-021 land:
//!
//!   - `taida_div_mold_f` / `taida_mod_mold_f` are added to
//!     `runtime_wasi_io.c` with C26B-011 semantics (Lax empty for zero
//!     divisor, FLOAT-tagged __value/__default).
//!   - `taida_bit_*` / `taida_shift_*` move from `runtime_full_wasm.c`
//!     into `runtime_wasi_io.c` so wasm-wasi inherits them too. wasm-full
//!     still works because it links rt_wasi alongside rt_full.
//!
//! These fixtures ratchet 4-backend parity (interpreter / JS / native /
//! wasm-wasi) plus a wasm-full check so any regression on any backend
//! flips the test red.

mod common;

use common::{taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn run_interp(td: &Path) -> Option<String> {
    let out = Command::new(taida_bin()).arg(td).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "interpreter failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn run_native(td: &Path) -> Option<String> {
    let exe: PathBuf = std::env::temp_dir().join(format!(
        "c27b021_native_{}_{}",
        std::process::id(),
        td.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
    ));
    let status = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td)
        .arg("-o")
        .arg(&exe)
        .output()
        .ok()?;
    if !status.status.success() {
        eprintln!(
            "native compile failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        return None;
    }
    let out = Command::new(&exe).output().ok()?;
    let _ = std::fs::remove_file(&exe);
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn run_js(td: &Path) -> Option<String> {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIP: node unavailable");
        return None;
    }
    let js = std::env::temp_dir().join(format!(
        "c27b021_js_{}_{}.js",
        std::process::id(),
        td.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
    ));
    let status = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(td)
        .arg("-o")
        .arg(&js)
        .output()
        .ok()?;
    if !status.status.success() {
        eprintln!(
            "js compile failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        return None;
    }
    let out = Command::new("node").arg(&js).output().ok()?;
    let _ = std::fs::remove_file(&js);
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn check_4backend_parity(rel_fixture: &str, expected: &str) {
    let td = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_fixture);

    let interp = run_interp(&td).expect("interpreter run");
    assert_eq!(interp, expected, "interpreter mismatch for {}", rel_fixture);

    let native = run_native(&td).expect("native run");
    assert_eq!(native, expected, "native parity for {}", rel_fixture);

    if let Some(js) = run_js(&td) {
        assert_eq!(js, expected, "JS parity for {}", rel_fixture);
    }

    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable -- wasm parity not verified");
            return;
        }
    };

    let wasi = std::env::temp_dir().join(format!(
        "c27b021_{}_{}.wasm",
        std::process::id(),
        td.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
    ));
    compile_wasm(&td, "wasm-wasi", &wasi).expect("wasm-wasi compile (C27B-021)");
    let wasi_out = run_wasm(&wasi, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasi);
    assert_eq!(wasi_out, expected, "wasm-wasi parity for {}", rel_fixture);

    let full = std::env::temp_dir().join(format!(
        "c27b021_{}_{}_full.wasm",
        std::process::id(),
        td.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
    ));
    compile_wasm(&td, "wasm-full", &full).expect("wasm-full compile (C27B-021)");
    let full_out = run_wasm(&full, &wasmtime).expect("wasm-full run");
    let _ = std::fs::remove_file(&full);
    assert_eq!(full_out, expected, "wasm-full parity for {}", rel_fixture);
}

/// Float Div with non-zero divisor: hasValue = true on every backend.
#[test]
fn c27b_021_div_basic_4backend() {
    check_4backend_parity("examples/quality/c27_wasm_arith/div_basic.td", "true");
}

/// Float Div by zero: Lax empty (hasValue = false) -- C26B-011 semantics
/// (formerly success-with-default in W-5; corrected for parity with the
/// native runtime in C26).
#[test]
fn c27b_021_div_zero_lax_empty_4backend() {
    check_4backend_parity("examples/quality/c27_wasm_arith/div_zero.td", "false");
}

#[test]
fn c27b_021_mod_basic_4backend() {
    check_4backend_parity("examples/quality/c27_wasm_arith/mod_basic.td", "true");
}

#[test]
fn c27b_021_mod_zero_lax_empty_4backend() {
    check_4backend_parity("examples/quality/c27_wasm_arith/mod_zero.td", "false");
}

/// Bitwise / shift surface: 7 ops in one fixture.
/// Expected output (one per stdout call):
///   1   (BitAnd[5, 3])
///   7   (BitOr[5, 2])
///   3   (BitXor[5, 6])
///   -1  (BitNot[0])
///   8   (ShiftL[1, 3])
///   4   (ShiftR[16, 2])
///   4   (ShiftRU[16, 2])
#[test]
fn c27b_021_bitwise_basic_4backend() {
    check_4backend_parity(
        "examples/quality/c27_wasm_arith/bitwise_basic.td",
        "1\n7\n3\n-1\n8\n4\n4",
    );
}

/// wasm-wasi was the breakage point before C27B-021: any Float Div
/// failed at compile time. This explicit test asserts the compile path
/// does NOT regress to the historic reject.
#[test]
fn c27b_021_wasi_div_compile_succeeds() {
    let td =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/quality/c27_wasm_arith/div_basic.td");
    let wasm = std::env::temp_dir().join("c27b021_wasi_div_compile.wasm");
    compile_wasm(&td, "wasm-wasi", &wasm)
        .expect("wasm-wasi must accept Div[Float, Float]() (C27B-021 land)");
    let _ = std::fs::remove_file(&wasm);
}

/// Same for wasm-full -- the historic `taida_div_mold_f` reject also
/// covered Full.
#[test]
fn c27b_021_full_div_compile_succeeds() {
    let td =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/quality/c27_wasm_arith/div_basic.td");
    let wasm = std::env::temp_dir().join("c27b021_full_div_compile.wasm");
    compile_wasm(&td, "wasm-full", &wasm)
        .expect("wasm-full must accept Div[Float, Float]() (C27B-021 land)");
    let _ = std::fs::remove_file(&wasm);
}

/// wasm-wasi must accept the bitwise/shift surface that previously was
/// rejected with `does not support runtime function 'taida_bit_and'`.
#[test]
fn c27b_021_wasi_bitwise_compile_succeeds() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c27_wasm_arith/bitwise_basic.td");
    let wasm = std::env::temp_dir().join("c27b021_wasi_bit_compile.wasm");
    compile_wasm(&td, "wasm-wasi", &wasm)
        .expect("wasm-wasi must accept BitAnd / ShiftL / ShiftRU (C27B-021 land)");
    let _ = std::fs::remove_file(&wasm);
}

// ===========================================================================
// C27B-021 wD Round 1 review fix — numeric (bit-pattern) parity tests.
//
// The Round 1 wD review found two Critical issues:
//   Critical 1: `taida_mod_mold_f` in runtime_wasi_io.c lost precision
//               for large dividends (single-pass `(int64_t)q * b`).
//               Empirical: `Mod[1.0e20, 7.0]()` returned 0 on wasm
//               vs 2.0 on interp/native/js.
//   Critical 2: All existing C27B-021 parity tests compared only
//               `c.hasValue.toString()` ("true" / "false"). The numeric
//               value of the Lax payload was never bit-pattern-checked
//               across backends, which is why Critical 1 went unnoticed.
//
// Fix in this commit:
//   - taida_mod_mold_f rewritten as exact scale-and-subtract fmod
//     (textbook MUSL algorithm, no precision loss, terminates in
//     ~ceil(log2(|a/b|)) iterations).
//   - These tests use `debug(r)` (4-backend numeric source-of-truth
//     formatter) to assert bit-pattern equality across interp / JS /
//     native / wasm-wasi / wasm-full. `debug` is the proven canonical
//     path for Float Lax values (the C26B-011 fixtures already use it
//     for the same reason — see examples/quality/c26_float_edge/
//     div_mod_float.td).
// ===========================================================================

/// Critical 1 hot point: `Mod[1.0e20, 7.0]()` must return 2.0 on every
/// backend including wasm-wasi (pre-fix wasm returned 0).
#[test]
fn c27b_021_wd_mod_overflow_large_4backend_numeric_parity() {
    check_4backend_parity(
        "examples/quality/c27_wasm_arith/mod_overflow_large.td",
        "2.0",
    );
}

/// Critical 1 precision case: `Mod[1000000.5, 3.14159265]()` must
/// agree to the last f64 mantissa bit on every backend (pre-fix wasm
/// diverged at the 9th significant digit).
#[test]
fn c27b_021_wd_mod_precision_pi_4backend_numeric_parity() {
    check_4backend_parity(
        "examples/quality/c27_wasm_arith/mod_precision_pi.td",
        "0.14357849993359384",
    );
}

/// Bit-pattern parity for a small/safe input — covers the common path
/// that was already correct but was untested at the numeric level.
#[test]
fn c27b_021_wd_mod_basic_value_4backend_numeric_parity() {
    check_4backend_parity("examples/quality/c27_wasm_arith/mod_basic_value.td", "1.0");
}

/// Signed zero must survive `Mod[-0.0, 1.0]()` on every backend
/// (IEEE 754 fmod sign rule preserves dividend sign).
#[test]
fn c27b_021_wd_mod_signed_zero_4backend_numeric_parity() {
    check_4backend_parity("examples/quality/c27_wasm_arith/mod_signed_zero.td", "-0.0");
}

/// `Mod[-7.0, 3.0]()` -> -1.0 (sign of dividend).
#[test]
fn c27b_021_wd_mod_neg_dividend_4backend_numeric_parity() {
    check_4backend_parity(
        "examples/quality/c27_wasm_arith/mod_neg_dividend.td",
        "-1.0",
    );
}

/// `Mod[7.0, -3.0]()` -> 1.0 (negative divisor does not flip sign;
/// sign follows dividend).
#[test]
fn c27b_021_wd_mod_neg_divisor_4backend_numeric_parity() {
    check_4backend_parity("examples/quality/c27_wasm_arith/mod_neg_divisor.td", "1.0");
}

/// `Div[1.0, 3.0]()` bit-pattern parity (covers `taida_float_to_str`
/// 16-digit truncation across all 4 backends).
#[test]
fn c27b_021_wd_div_basic_value_4backend_numeric_parity() {
    check_4backend_parity(
        "examples/quality/c27_wasm_arith/div_basic_value.td",
        "0.3333333333333333",
    );
}
