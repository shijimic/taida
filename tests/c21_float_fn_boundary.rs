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
//! Status (Phase 2 + Phase 4 land 時点の final snapshot)
//!
//! * Interpreter: `12.0` / `11.0` を正しく返す (リファレンス)
//! * JS: `12.0` / `11.0` — Phase 5 で seed-04 解消済
//! * Native: `12.0` / `11.0` — Phase 4 で C21B-008 (Cranelift verifier
//!   errors) 解消済: `ConstFloat` を emit 即 bitcast して boxed i64 に
//!   正規化、`taida_io_stdout_with_tag` で FLOAT tag を `taida_float_to_str`
//!   経由にルートする
//! * WASM-wasi: `12.0` / `11.0` — Phase 2/4 で seed-01/03/C21B-009
//!   解消済: (a) `@[Float]` 要素の unmold 経路 (`a.get(i) ]=> av`) で
//!   element type を `list_element_types` に伝播して `av*bv` を
//!   `taida_float_mul` に降ろす、(b) `taida_io_stdout_with_tag` が
//!   FLOAT tag で `taida_float_to_str` (bit-pattern decode) を呼ぶ。
//!
//! 全 XFAIL は解除済。新しい regression は snapshot ではなく
//! 4-backend parity test が検出する。

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
        .args(["build", "js"])
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
        .args(["build", "wasm-wasi"])
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
// Native — Phase 4 で C21B-008 (`ConstFloat` → CallUser/Return で raw f64 が
// i64 ABI slot に漏れて Cranelift verifier errors) + FLOAT tag stdout dispatch
// を修正。現在は parity 成立。
// ---------------------------------------------------------------------------

#[test]
fn triple_native_parity() {
    let out = run_native(triple_td()).expect("native build+run should succeed");
    assert_eq!(out, "12.0", "native must match interpreter reference");
}

#[test]
fn dot_product_native_parity() {
    let out = run_native(dot_product_td()).expect("native build+run should succeed");
    assert_eq!(out, "11.0", "native must match interpreter reference");
}

// ---------------------------------------------------------------------------
// WASM-wasi — Phase 2 (`@[Float]` 要素 unmold の型タグ伝播) + Phase 4
// (FLOAT tag を `taida_float_to_str` にルート) で seed-01 / seed-03 / C21B-009 解消。
// ---------------------------------------------------------------------------

#[test]
fn triple_wasm_wasi_parity() {
    if wasmtime_bin().is_none() {
        // wasmtime が無い環境では skip (CI 未設定環境での early-exit)
        return;
    }
    let out = run_wasm_wasi(triple_td()).expect("wasm-wasi build+run should succeed");
    assert_eq!(out, "12.0", "wasm-wasi must match interpreter reference");
}

#[test]
fn dot_product_wasm_wasi_parity() {
    if wasmtime_bin().is_none() {
        return;
    }
    let out = run_wasm_wasi(dot_product_td()).expect("wasm-wasi build+run should succeed");
    assert_eq!(out, "11.0", "wasm-wasi must match interpreter reference");
}

// ---------------------------------------------------------------------------
// Snapshot 系 test (Phase 1 時点の壊れっぷりを固定) は Phase 2/4 修正により
// 全て解除済み。履歴のみドキュメント留め:
//   - triple_snapshot_js_current_behavior   — Phase 5 で削除 (seed-04 解消)
//   - triple_snapshot_wasm_wasi_current_behavior — Phase 4 で削除
//     (上記 `triple_wasm_wasi_parity` に通常 test 化)
// ---------------------------------------------------------------------------
