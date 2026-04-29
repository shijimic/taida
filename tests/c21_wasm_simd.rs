//! C21-3 / C21-6: `-msimd128` profile-split smoke test.
//!
//! Purpose
//! -------
//! Phase 3 split `WASM_CLANG_FLAGS` into a profile-specific vector so that
//! `wasm-wasi` / `wasm-edge` / `wasm-full` enable `-msimd128` while
//! `wasm-min` does not. This test disassembles the resulting `.wasm` with
//! `wasm-tools` and asserts:
//!
//!   * `wasm-wasi` output contains at least one SIMD-ish opcode
//!     (`v128.*`, `f32x4.*`, `f64x2.*`, `i*x*.*`) — Phase 2's Float unbox
//!     + Phase 3's `-msimd128` together re-opened the auto-vectorizer path.
//!   * `wasm-min` output contains zero such opcodes — the no-SIMD
//!     compatibility door we leave open for minimal runtimes stays closed
//!     to simd128 feature requirements.
//!
//! The two together are the load-bearing guard for seed-02. If either side
//! drifts (simd128 silently leaks into wasm-min, or wasi stops emitting
//! vectorized code), this test trips.
//!
//! Gating: both `wasm-tools` and `wasmtime` must be on the runner; if
//! either is missing we skip cleanly. CI images that ship the Taida wasm
//! track have both.

mod common;

use common::{taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

fn matmul_td() -> &'static Path {
    Path::new("examples/quality/c21b_wasm_simd/matmul_small.td")
}

/// Locate `wasm-tools` on the system. Checks `$HOME/.cargo/bin/wasm-tools`
/// first (the canonical cargo-install location used by this repo's CI),
/// then falls back to `which wasm-tools`.
fn wasm_tools_bin() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(&home).join(".cargo/bin/wasm-tools");
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("which").arg("wasm-tools").output()
        && output.status.success()
    {
        let p = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    None
}

/// Compile `matmul_small.td` against the given `--target` and return the
/// disassembled WAT string + path to the wasm (for cleanup).
fn build_and_print(target: &str) -> Option<(String, PathBuf)> {
    let stem = format!("c21_simd_{}_{}", std::process::id(), target);
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));

    let build = Command::new(taida_bin())
        .args(["build", target])
        .arg(matmul_td())
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;
    if !build.status.success() {
        eprintln!(
            "wasm build failed (target={}): {}",
            target,
            String::from_utf8_lossy(&build.stderr)
        );
        let _ = std::fs::remove_file(&wasm_path);
        return None;
    }

    let wt = wasm_tools_bin()?;
    let print = Command::new(&wt)
        .arg("print")
        .arg(&wasm_path)
        .output()
        .ok()?;
    if !print.status.success() {
        eprintln!(
            "wasm-tools print failed for {}: {}",
            wasm_path.display(),
            String::from_utf8_lossy(&print.stderr)
        );
        let _ = std::fs::remove_file(&wasm_path);
        return None;
    }
    let wat = String::from_utf8_lossy(&print.stdout).to_string();
    Some((wat, wasm_path))
}

/// Count the number of SIMD-flavored opcodes in a WAT dump. We treat any
/// line that contains a word-boundary match for one of the known SIMD
/// opcode families as "vectorized". The point is binary presence, not a
/// precise instruction mix (that's a downstream performance concern).
fn count_simd_ops(wat: &str) -> usize {
    let needles: &[&str] = &[
        "v128.", "f32x4.", "f64x2.", "i8x16.", "i16x8.", "i32x4.", "i64x2.",
    ];
    wat.lines()
        .filter(|line| needles.iter().any(|n| line.contains(n)))
        .count()
}

/// Count plain `f32.*` / `f64.*` instructions. Phase 2 asserts these show
/// up at all; Phase 3 asserts the SIMD variants *also* show up.
fn count_float_ops(wat: &str) -> usize {
    wat.lines()
        .filter(|line| line.contains("f32.") || line.contains("f64."))
        .count()
}

#[test]
fn wasm_wasi_has_simd_and_float_ops() {
    if wasmtime_bin().is_none() || wasm_tools_bin().is_none() {
        // CI runners without wasm-tools or wasmtime skip; the other Phase
        // 1 parity tests still catch functional regressions.
        return;
    }

    let (wat, wasm_path) =
        build_and_print("wasm-wasi").expect("wasm-wasi build+disassemble should succeed");

    let simd = count_simd_ops(&wat);
    let floats = count_float_ops(&wat);

    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        floats > 0,
        "Phase 2 invariant: wasm-wasi must emit scalar f64/f32 ops \
         in Float hot loops (got {} f*. instructions)",
        floats
    );
    assert!(
        simd > 0,
        "Phase 3 invariant: wasm-wasi with -msimd128 must let LLVM's \
         auto-vectorizer emit at least one v128/fNxM/iNxM instruction \
         for this matmul-shape fixture (got {} SIMD instructions, \
         {} scalar Float instructions)",
        simd,
        floats
    );
}

#[test]
fn wasm_min_stays_simd_free() {
    if wasmtime_bin().is_none() || wasm_tools_bin().is_none() {
        return;
    }

    let (wat, wasm_path) =
        build_and_print("wasm-min").expect("wasm-min build+disassemble should succeed");

    let simd = count_simd_ops(&wat);
    let _ = std::fs::remove_file(&wasm_path);

    assert_eq!(
        simd, 0,
        "wasm-min must stay SIMD-free for minimal-runtime compatibility: \
         found {} SIMD-flavored instructions, which would silently force \
         simd128 feature requirement on consumers that chose wasm-min",
        simd
    );
}

#[test]
fn wasm_wasi_matmul_small_runs_correctly() {
    // Parity guard: the same binary we disassemble must still compute
    // 1+4+9+16+25+36+49+64 = 204.0 under wasmtime. Catches a scenario
    // where SIMD emission succeeds but breaks numerical correctness
    // (e.g. misaligned f64x2 loads).
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => return,
    };

    let stem = format!("c21_simd_run_{}", std::process::id());
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));

    let build = Command::new(taida_bin())
        .args(["build", "wasm-wasi"])
        .arg(matmul_td())
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("wasm-wasi build should spawn");
    assert!(
        build.status.success(),
        "wasm-wasi build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&wasmtime)
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should spawn");
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        run.status.success(),
        "wasmtime run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        stdout.trim(),
        "204.0",
        "matmul_small.td sumSquares must equal 1+4+9+16+25+36+49+64 = 204.0 under wasm-wasi"
    );
}
