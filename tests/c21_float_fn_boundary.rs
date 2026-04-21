//! C21-1 / seed-06: Float 関数跨ぎ parity test 基盤
//!
//! Purpose
//! -------
//! bonsai-wasm Phase 0 で発覚した seed-01 (Wasm Float boxed) / seed-03
//! (関数跨ぎ Float → Str で i64 bit pattern が漏れる) / seed-04
//! (JS Number で .0 が落ちる) の regression guard。
//!
//! 仕様上の期待: Interpreter / JS / Native / WASM-wasi の 4 backend で
//! `triple(4.0)` と `dotProductAt(...)` の出力が完全に一致する。
//! Interpreter をリファレンスとし、他 backend は Interpreter に揃える。
//!
//! Status (Phase 5 land 時点の snapshot)
//! ---------------------------------------
//! * Interpreter: `12.0` / `11.0` を正しく返す (リファレンス)
//! * JS:         `12.0` / `11.0` — Phase 5 で seed-04 (Float→Str parity) を解消済
//! * Native:     Verifier errors で compile 失敗 (C21B-008 新ブロッカー)
//! * WASM-wasi:  `4622945017495814144` / `0` — seed-01/03 (Phase 2/4 で修正)
//!
//! JS parity は Phase 5 で統合済。Native / WASM は Phase 2/4 land 待ちの
//! XFAIL (#[ignore]) として残す。`#[ignore = "..."]` には解除予定 Phase を
//! 明記し、Phase land 時に単純削除で通常 test 化できる状態で維持する。
//!
//! 恒久 `#[ignore]` は禁止。残る XFAIL は Phase 2/4 完了時点で解除予定。

mod common;

use common::{normalize, taida_bin, wasmtime_bin};
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Backend runners (tests-local, parity.rs の helper を流用しない方針)
// ---------------------------------------------------------------------------

/// Run a `.td` file via the interpreter, returning normalized stdout.
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

/// Transpile to JS and execute with node.
fn run_js(td_path: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let js_path = std::env::temp_dir().join(format!("c21_ffb_{}_{}.mjs", std::process::id(), stem));

    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&js_path);
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
            "node failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

/// Compile to native and run.
fn run_native(td_path: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let bin_path =
        std::env::temp_dir().join(format!("c21_ffb_{}_{}.bin", std::process::id(), stem));

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
        return None;
    }

    let run = Command::new(&bin_path).output().ok()?;
    let _ = std::fs::remove_file(&bin_path);
    if !run.status.success() {
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

/// Compile to wasm-wasi and run with wasmtime.
fn run_wasm_wasi(td_path: &Path) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let wasm_path =
        std::env::temp_dir().join(format!("c21_ffb_{}_{}.wasm", std::process::id(), stem));

    let build = Command::new(taida_bin())
        .args(["build", "--target", "wasm-wasi"])
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&wasm_path);
        eprintln!(
            "wasm-wasi build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }

    let run = Command::new(&wasmtime).arg(&wasm_path).output().ok()?;
    let _ = std::fs::remove_file(&wasm_path);
    if !run.status.success() {
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

// ---------------------------------------------------------------------------
// Fixture paths
// ---------------------------------------------------------------------------

fn triple_td() -> &'static Path {
    Path::new("examples/quality/c21b_float_fn_boundary/triple.td")
}

fn dot_product_td() -> &'static Path {
    Path::new("examples/quality/c21b_float_fn_boundary/dot_product.td")
}

// ---------------------------------------------------------------------------
// Interpreter = reference (must pass from Phase 1 onward)
// ---------------------------------------------------------------------------

#[test]
fn triple_interpreter_reference() {
    let out = run_interpreter(triple_td()).expect("interpreter should succeed");
    assert_eq!(
        out, "12.0",
        "interpreter is the reference implementation; triple(4.0) must yield 12.0"
    );
}

#[test]
fn dot_product_interpreter_reference() {
    let out = run_interpreter(dot_product_td()).expect("interpreter should succeed");
    assert_eq!(
        out, "11.0",
        "interpreter is the reference implementation; dotProductAt(@[1.0,2.0],@[3.0,4.0],0,2,0.0) must yield 11.0"
    );
}

// ---------------------------------------------------------------------------
// JS — Phase 5 で seed-04 (Float→Str parity) 解消済。通常 test として常時緑化。
// ---------------------------------------------------------------------------

#[test]
fn triple_js_parity() {
    if which_node().is_none() {
        // node 未インストール環境ではスキップ (WASM と同様の扱い)
        return;
    }
    let out = run_js(triple_td()).expect("js run should succeed");
    assert_eq!(out, "12.0", "JS must match interpreter reference");
}

#[test]
fn dot_product_js_parity() {
    if which_node().is_none() {
        return;
    }
    let out = run_js(dot_product_td()).expect("js run should succeed");
    assert_eq!(out, "11.0", "JS must match interpreter reference");
}

/// Utility: return Some(()) iff `node` is discoverable on PATH. Mirrors
/// the `wasmtime_bin()` gating used below so CI hosts without Node.js
/// skip the JS tests cleanly instead of failing.
fn which_node() -> Option<()> {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(()) } else { None })
}

// ---------------------------------------------------------------------------
// Native — 新ブロッカー: Float 関数戻り値で Cranelift verifier errors。
// Phase 4 の Float → Str ABI 統一作業 (seed-05 audit ペア) で解消予定。
// ---------------------------------------------------------------------------

#[test]
#[ignore = "C21 Phase 4 (seed-05 / native Float unbox audit) 完了時に解除予定。\
            現在: `triple x: Float => :Float` の native build が \
            `Emit error: define_function failed: Compilation error: Verifier errors` で失敗"]
fn triple_native_parity() {
    let out = run_native(triple_td()).expect("native build+run should succeed");
    assert_eq!(out, "12.0", "native must match interpreter reference");
}

#[test]
#[ignore = "C21 Phase 4 (seed-05 / native Float unbox audit) 完了時に解除予定。\
            現在: native build が verifier errors で失敗"]
fn dot_product_native_parity() {
    let out = run_native(dot_product_td()).expect("native build+run should succeed");
    assert_eq!(out, "11.0", "native must match interpreter reference");
}

// ---------------------------------------------------------------------------
// WASM-wasi — seed-01 / seed-03。Phase 2 (unbox) + Phase 4 (Float→Str ABI) 完了時に解除。
// ---------------------------------------------------------------------------

#[test]
#[ignore = "C21 Phase 2 (seed-01 WASM Float unbox) + Phase 4 (seed-03 Float→Str ABI) \
            完了時に解除予定。\
            現在: `stdout(triple(4.0))` は `4622945017495814144` \
            (= 0x4028000000000000 = 12.0 の f64 bit pattern) を出力する"]
fn triple_wasm_wasi_parity() {
    if wasmtime_bin().is_none() {
        // wasmtime が無い環境では skip (XFAIL として扱うので actual assertion は不要)
        return;
    }
    let out = run_wasm_wasi(triple_td()).expect("wasm-wasi build+run should succeed");
    assert_eq!(out, "12.0", "wasm-wasi must match interpreter reference");
}

#[test]
#[ignore = "C21 Phase 2 (seed-01 WASM Float unbox) + Phase 4 (seed-03 Float→Str ABI) \
            完了時に解除予定。\
            現在: 内積計算が seed-01 により `0` を返す (hot loop で Float 演算が壊れる)"]
fn dot_product_wasm_wasi_parity() {
    if wasmtime_bin().is_none() {
        return;
    }
    let out = run_wasm_wasi(dot_product_td()).expect("wasm-wasi build+run should succeed");
    assert_eq!(out, "11.0", "wasm-wasi must match interpreter reference");
}

// ---------------------------------------------------------------------------
// Snapshot tests: Phase 1 時点の「壊れっぷり」を記録として残す。
// Phase land 時に失敗するようになったら、そのまま削除して Parity test に置き換える。
//
// 履歴:
//   - triple_snapshot_js_current_behavior: Phase 5 land (seed-04 解消) で
//     `12` → `12.0` になり削除済。同時に triple_js_parity / dot_product_js_parity
//     の `#[ignore]` を解除し通常 test 化した。
// ---------------------------------------------------------------------------

#[test]
fn triple_snapshot_wasm_wasi_current_behavior() {
    // Phase 2/4 完了時に削除予定。
    // 現状 WASM-wasi は f64 の i64 bit pattern を吐く。これが変わったら修正が効いた証拠。
    if wasmtime_bin().is_none() {
        return;
    }
    let out = match run_wasm_wasi(triple_td()) {
        Some(o) => o,
        None => return, // ビルド失敗時も skip (環境問題と区別しない)
    };
    assert_eq!(
        out, "4622945017495814144",
        "Phase 1 snapshot: WASM-wasi は現在 f64 bit pattern を返す。\
         修正後は `12.0` になる — その時点で本 test を削除し triple_wasm_wasi_parity の \
         #[ignore] を外すこと"
    );
}
