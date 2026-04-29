//! C25B-025 math mold family — 4-backend parity.
//!
//! Phase 5-A (commit 86d5743) added the math mold family
//! (Sqrt / Pow / Exp / Ln / Log / Log2 / Log10 / Sin / Cos / Tan /
//! Asin / Acos / Atan / Atan2 / Sinh / Cosh / Tanh) to the
//! interpreter and JS runtime. Phase 5-I (this test) extends
//! the family to the native and wasm-wasi codegen backends.
//!
//! Parity semantics (by backend):
//!
//! Interpreter is the source of truth. It delegates to Rust's
//! `f64::sqrt` / `f64::exp` / ..., which on Linux / macOS call into
//! glibc libm via the LLVM `@llvm.*.f64` intrinsics.
//!
//! JS delegates to V8's `Math.*`. Historically bit-exact with libm on
//! the fixture inputs we test, so we keep the existing exact-string
//! comparison.
//!
//! Native links `-lm`, so `taida_float_sqrt` / `_exp` / ... invoke the
//! same glibc libm. Bit-for-bit parity with the interpreter on
//! x86_64-linux / aarch64-linux is expected and enforced (`assert_eq!`
//! on the exact output bytes).
//!
//! WASM-wasi: `-nostdlib` precludes linking libm. The runtime
//! implements each transcendental with a range-reduction plus
//! truncated-series kernel (see
//! `src/codegen/runtime_core_wasm/03_typeof_list.inc.c`). These
//! kernels are within ~1 ULP of libm for the fixture inputs but are
//! NOT correctly-rounded in general. We compare wasm vs interpreter
//! with a relative tolerance of `1e-10`; values that are bit-exact in
//! interpreter (e.g. `Sqrt[4.0]() == 2.0`, which the wasm backend
//! delivers via the `f64.sqrt` opcode) are still expected to match
//! exactly.
//!
//! NB: JS `Number.prototype.toString()` renders whole-valued floats
//! without the `.0` suffix (`2` vs `2.0`). This is a 4-backend
//! display gap that predates Phase 5-A (affects Abs / Clamp / …).
//! `normalise_whole_floats` below strips the trailing `.0` so the
//! parity assertion focuses on numerical correctness.
mod common;

use common::{taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp_artifact(td_path: &Path, suffix: &str) -> PathBuf {
    let stem = td_path.file_stem().unwrap().to_string_lossy();
    std::env::temp_dir().join(format!(
        "c25b_025_{}_{}.{}",
        std::process::id(),
        stem,
        suffix
    ))
}

/// Normalise whole-float display (`2.0` ↔ `2`) so that the JS
/// backend's default Number→string rendering compares equal to the
/// interpreter's `{:.1}` formatting. Non-whole floats, ints, strings
/// pass through unchanged.
fn normalise_whole_floats(s: &str) -> Vec<String> {
    s.lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(stripped) = trimmed.strip_suffix(".0")
                && stripped.parse::<i64>().is_ok()
            {
                stripped.to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect()
}

fn run_interpreter(td_path: &Path) -> Vec<String> {
    let out = Command::new(taida_bin())
        .arg(td_path)
        .output()
        .expect("spawn interpreter");
    assert!(
        out.status.success(),
        "interpreter failed for {}: {}",
        td_path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    normalise_whole_floats(&String::from_utf8_lossy(&out.stdout))
}

fn run_js(td_path: &Path) -> Option<Vec<String>> {
    let js_path = tmp_artifact(td_path, "mjs");
    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .expect("spawn taida build");
    if !build.status.success() {
        eprintln!(
            "js build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
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
    Some(normalise_whole_floats(&String::from_utf8_lossy(
        &run.stdout,
    )))
}

fn run_native(td_path: &Path) -> Option<Vec<String>> {
    let bin_path = tmp_artifact(td_path, "bin");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("spawn taida build");
    if !build.status.success() {
        eprintln!(
            "native build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
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
    Some(normalise_whole_floats(&String::from_utf8_lossy(
        &run.stdout,
    )))
}

fn run_wasm_wasi(td_path: &Path) -> Option<Vec<String>> {
    let wasmtime = wasmtime_bin()?;
    let wasm_path = tmp_artifact(td_path, "wasm");
    let build = Command::new(taida_bin())
        .args(["build", "wasm-wasi"])
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("spawn taida build");
    if !build.status.success() {
        eprintln!(
            "wasm build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }
    let run = Command::new(&wasmtime).arg(&wasm_path).output().ok()?;
    let _ = std::fs::remove_file(&wasm_path);
    if !run.status.success() {
        eprintln!(
            "wasm run failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalise_whole_floats(&String::from_utf8_lossy(
        &run.stdout,
    )))
}

/// Parse a line as f64 if possible.  Lines that don't parse as a
/// number compare exactly (strings / booleans / `-inf` / etc.).
fn parse_float(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

/// Compare two lines with relative-ULP tolerance for numeric values.
/// Non-numeric lines compare exactly.
fn numerically_close(a: &str, b: &str, rel_tol: f64) -> bool {
    if a == b {
        return true;
    }
    match (parse_float(a), parse_float(b)) {
        (Some(x), Some(y)) => {
            if x.is_nan() && y.is_nan() {
                return true;
            }
            if !x.is_finite() || !y.is_finite() {
                return x == y;
            }
            let diff = (x - y).abs();
            let scale = x.abs().max(y.abs()).max(1.0);
            diff <= rel_tol * scale
        }
        _ => false,
    }
}

fn assert_close_vecs(actual: &[String], expected: &[String], rel_tol: f64, label: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{label} line count mismatch: actual {} vs expected {}",
        actual.len(),
        expected.len()
    );
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            numerically_close(a, e, rel_tol),
            "{label} line {i}: '{a}' vs '{e}' (rel_tol={rel_tol})"
        );
    }
}

#[test]
fn c25b_025_sqrt_pow_interpreter_and_js() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c25b_025_math_molds/sqrt_pow.td");

    let interp = run_interpreter(&fixture);
    assert_eq!(
        interp,
        vec![
            "2".to_string(),
            "1.4142135623730951".to_string(),
            "1024".to_string(),
            "9".to_string(),
        ],
        "interpreter sqrt/pow output shape regressed — Phase 5-A expected Sqrt[4.0]()=2, Sqrt[2.0]()=sqrt2, Pow[2.0,10]()=1024, Pow[3.0,2]()=9"
    );

    if let Some(js) = run_js(&fixture) {
        assert_eq!(js, interp, "JS math molds diverge from interpreter");
    } else {
        eprintln!("skipping JS parity check — node not available");
    }
}

#[test]
fn c25b_025_transcendentals_interpreter_and_js() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c25b_025_math_molds/transcendentals.td");

    let interp = run_interpreter(&fixture);
    assert_eq!(interp.len(), 8, "transcendental fixture emits 8 lines");
    // Line-by-line spot check for the well-known identities so the
    // test fails loudly if a future tweak breaks an identity.
    assert_eq!(interp[0], "0", "Sin[0.0]() == 0");
    assert_eq!(interp[1], "1", "Cos[0.0]() == 1");
    // Exp[1] = e = 2.718281828459045 (f64 rounding)
    assert_eq!(interp[2], "2.718281828459045", "Exp[1.0]() == e");
    // Ln[e] = 1 but f64 rounding yields 1.0 (we strip trailing .0)
    assert_eq!(interp[3], "1", "Ln[e]() == 1");
    assert_eq!(interp[4], "2", "Log10[100.0]() == 2");
    assert_eq!(interp[5], "3", "Log2[8.0]() == 3");
    // Atan2[1, 1] = pi/4 = 0.7853981633974483
    assert_eq!(interp[6], "0.7853981633974483", "Atan2[1.0, 1.0]() == pi/4");
    assert_eq!(interp[7], "3", "Log[8.0, 2.0]() == 3");

    if let Some(js) = run_js(&fixture) {
        assert_eq!(js, interp, "JS transcendentals diverge from interpreter");
    } else {
        eprintln!("skipping JS parity check — node not available");
    }
}

/// C25B-025 Phase 5-I: native backend parity. Native links `-lm`
/// and calls the same glibc libm as Rust's `f64::*` — we expect
/// bit-for-bit match with the interpreter.
#[test]
fn c25b_025_sqrt_pow_native_parity() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c25b_025_math_molds/sqrt_pow.td");
    let interp = run_interpreter(&fixture);
    let Some(native) = run_native(&fixture) else {
        eprintln!("skipping native parity — native toolchain unavailable");
        return;
    };
    assert_eq!(
        native, interp,
        "native sqrt/pow diverge from interpreter (expected bit-exact on glibc-linked native)"
    );
}

#[test]
fn c25b_025_transcendentals_native_parity() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c25b_025_math_molds/transcendentals.td");
    let interp = run_interpreter(&fixture);
    let Some(native) = run_native(&fixture) else {
        eprintln!("skipping native parity — native toolchain unavailable");
        return;
    };
    assert_eq!(
        native, interp,
        "native transcendentals diverge from interpreter (expected bit-exact on glibc-linked native)"
    );
}

/// C25B-025 Phase 5-I: wasm-wasi backend parity with tolerance.
/// The wasm runtime implements transcendentals manually (`-nostdlib`
/// precludes libm) so bit-exact match is not achievable for e.g.
/// `Exp[1.0]()`. A 1e-12 relative tolerance still catches all
/// structural / sign / identity bugs.
#[test]
fn c25b_025_sqrt_pow_wasm_parity() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c25b_025_math_molds/sqrt_pow.td");
    let interp = run_interpreter(&fixture);
    let Some(wasm) = run_wasm_wasi(&fixture) else {
        eprintln!("skipping wasm-wasi parity — wasmtime unavailable");
        return;
    };
    // Sqrt uses the wasm `f64.sqrt` opcode (hardware), and Pow with
    // an integer exponent uses exact repeated squaring — both are
    // expected bit-exact with libm here.
    assert_eq!(
        wasm, interp,
        "wasm Sqrt / Pow diverge from interpreter \
         (f64.sqrt opcode + integer-exponent Pow should be bit-exact)"
    );
}

#[test]
fn c25b_025_transcendentals_wasm_parity() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c25b_025_math_molds/transcendentals.td");
    let interp = run_interpreter(&fixture);
    let Some(wasm) = run_wasm_wasi(&fixture) else {
        eprintln!("skipping wasm-wasi parity — wasmtime unavailable");
        return;
    };
    // Transcendentals on wasm use range-reduction + series. Accept
    // 1e-10 relative tolerance — tight enough to catch sign / quadrant
    // / off-by-factor bugs, loose enough to tolerate ~1 ULP series
    // truncation error.
    assert_close_vecs(&wasm, &interp, 1e-10, "wasm transcendentals");
}
