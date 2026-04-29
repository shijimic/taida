//! C21B-seed-04 (2026-04-22 reopen) regression guard: JS Float-origin
//! propagation through local bindings.
//!
//! Background
//! ----------
//! Phase 5 (C21-5) originally fixed `stdout(triple(4.0))` and
//! `Float[3.0]()` by adding compile-time Float-origin tracking on
//! `FloatLit` / arithmetic / `=> :Float` user-fn call *at the terminal
//! site*. It missed the `x <= 3.0` case: local bindings did not feed the
//! Float-origin tag into `is_float_origin_expr(Expr::Ident)`, so code
//! that spelled the rvalue across a let-bind lost the tag and JS fell
//! through to `Number.isInteger(3) === true` — printing `Lax[3]` for
//! `Float[x]()` and `3` for `stdout(x)`, diverging from the interpreter.
//!
//! The re-fix extends the JS codegen's scope-aware Float-origin tracker
//! to register local binding targets (`x <= 3.0`), typed parameters
//! (`x: Float`), typed list parameters (`a: @[Float]`), `@[Float]`-shape
//! homogeneous list literals, and unmold targets rooted in a Float list
//! (`a.get(i) ]=> av`). The runtime also grows a `Float_mold_f` variant
//! that tags the resulting Lax with `__floatHint: true` so
//! `stdout(Float[x]())` renders its `__value` / `__default` with `.0`.
//!
//! This test pins the Interpreter (reference) and JS outputs for the
//! two REOPEN repros and one 1-level-deeper case (Float-returning user
//! function whose result is locally bound before the mold call). Native
//! and WASM backends are not co-pinned on `Float[x]()` here because a
//! separate pre-existing regression (the C21-4 FLOAT tag fast path over-
//! applies to Lax return values) makes those backends print garbage for
//! the exact `Float[x]()` shape — tracked separately outside seed-04.
//! The `float_local_toString.td` case is 4-backend green and is pinned
//! accordingly.

mod common;

use common::{normalize, taida_bin};
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Backend runners (local; parity.rs helpers are unsuitable here because we
// need case-by-case backend selection rather than the uniform 3-backend
// sweep).
// ---------------------------------------------------------------------------

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

fn run_js(td_path: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let js_path =
        std::env::temp_dir().join(format!("c21_jsfb_{}_{}.mjs", std::process::id(), stem));
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

fn run_native(td_path: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let bin_path =
        std::env::temp_dir().join(format!("c21_jsfb_{}_{}.bin", std::process::id(), stem));
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

fn run_wasm_wasi(td_path: &Path) -> Option<String> {
    let wasmtime = common::wasmtime_bin()?;
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let wasm_path =
        std::env::temp_dir().join(format!("c21_jsfb_{}_{}.wasm", std::process::id(), stem));
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

fn which_node() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Fixture paths (shared with c21_float_fn_boundary.rs directory so that
// the existing 4-backend parity runner picks them up too where applicable).
// ---------------------------------------------------------------------------

fn float_local_binding_td() -> &'static Path {
    Path::new("examples/quality/c21b_float_fn_boundary/float_local_binding.td")
}
fn float_local_to_string_td() -> &'static Path {
    Path::new("examples/quality/c21b_float_fn_boundary/float_local_toString.td")
}
fn float_fn_result_local_td() -> &'static Path {
    Path::new("examples/quality/c21b_float_fn_boundary/float_fn_result_local.td")
}

// ---------------------------------------------------------------------------
// Interpreter = reference. These assertions MUST hold from the moment this
// test lands; they describe the semantic contract (`Float[3.0]()` yields a
// Float-tagged Lax, `Int[3.0]()` yields an Int-tagged Lax) that the other
// backends must mirror.
// ---------------------------------------------------------------------------

const EXPECTED_FLOAT_LOCAL_BINDING: &str = concat!(
    "@(hasValue <= true, __value <= 3.0, __default <= 0.0, __type <= \"Lax\")",
    "\n",
    "@(hasValue <= true, __value <= 3, __default <= 0, __type <= \"Lax\")",
);

const EXPECTED_FLOAT_LOCAL_TO_STRING: &str = "3.0\n3.0";

const EXPECTED_FLOAT_FN_RESULT_LOCAL: &str =
    "@(hasValue <= true, __value <= 12.0, __default <= 0.0, __type <= \"Lax\")";

#[test]
fn float_local_binding_interpreter_reference() {
    let out = run_interpreter(float_local_binding_td()).expect("interpreter should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_BINDING,
        "Interpreter reference: Float[x]() and Int[x]() must produce distinct Lax tags"
    );
}

#[test]
fn float_local_to_string_interpreter_reference() {
    let out = run_interpreter(float_local_to_string_td()).expect("interpreter should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_TO_STRING,
        "Interpreter reference: stdout(x) / x.toString() on Float local binding must include `.0`"
    );
}

#[test]
fn float_fn_result_local_interpreter_reference() {
    let out = run_interpreter(float_fn_result_local_td()).expect("interpreter should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_FN_RESULT_LOCAL,
        "Interpreter reference: Float[triple(4.0)-via-local]() must yield Lax[12.0]"
    );
}

// ---------------------------------------------------------------------------
// JS parity (the REOPEN scope). Pre-fix output was:
//   float_local_binding.td    -> @(__value <= 3 ...) / @(__value <= 3 ...)  (BUG)
//   float_local_toString.td   -> 3 / 3                                      (BUG)
//   float_fn_result_local.td  -> @(__value <= 12 ...)                       (BUG)
// Post-fix output matches the interpreter reference above.
// ---------------------------------------------------------------------------

#[test]
fn float_local_binding_js_parity() {
    if !which_node() {
        return; // node not installed; skip (same convention as c21_float_fn_boundary)
    }
    let out = run_js(float_local_binding_td()).expect("js run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_BINDING,
        "JS must match interpreter — Float[x]() on Float local must tag Lax as Float"
    );
}

#[test]
fn float_local_to_string_js_parity() {
    if !which_node() {
        return;
    }
    let out = run_js(float_local_to_string_td()).expect("js run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_TO_STRING,
        "JS must match interpreter — stdout(x) and x.toString() must print `3.0`"
    );
}

#[test]
fn float_fn_result_local_js_parity() {
    if !which_node() {
        return;
    }
    let out = run_js(float_fn_result_local_td()).expect("js run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_FN_RESULT_LOCAL,
        "JS must match interpreter — Float[triple(4.0)-via-local]() must yield Lax[12.0]"
    );
}

// ---------------------------------------------------------------------------
// Native / WASM parity for the `stdout(x)` / `x.toString()` shape. These
// paths do not route through `Float[x]()` so the separate pre-existing
// native/wasm `Float_mold` regression does not affect them, and the
// 4-backend parity contract holds cleanly.
// ---------------------------------------------------------------------------

#[test]
fn float_local_to_string_native_parity() {
    let out = run_native(float_local_to_string_td()).expect("native build+run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_TO_STRING,
        "native must match interpreter — stdout(x) / x.toString() on Float local must print `3.0`"
    );
}

#[test]
fn float_local_to_string_wasm_parity() {
    if common::wasmtime_bin().is_none() {
        return;
    }
    let out = run_wasm_wasi(float_local_to_string_td()).expect("wasm build+run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_TO_STRING,
        "wasm-wasi must match interpreter — stdout(x) / x.toString() on Float local must print `3.0`"
    );
}

// ---------------------------------------------------------------------------
// C21B-seed-07 (2026-04-22): Native / WASM parity for the `Float[x]()`
// mold-call shape. These were split out of the original re-fix as
// "pre-existing regression" but are now fixed:
//
// * `src/codegen/native_runtime/core.c` — each primitive mold
//   (`taida_{int,float,bool,str}_mold_*`) stamps the output tag on the
//   Lax's `__value` / `__default` fields; `taida_pack_to_display_string`
//   / `_full` consult the per-field tag so Float values render through
//   `taida_float_to_str`; `taida_io_stdout_with_tag` / `_stderr_with_tag`
//   route any runtime-detected BuchiPack through
//   `taida_stdout_display_string` which uses the `_full` form.
// * `src/codegen/runtime_core_wasm/{01_core,02_containers}.inc.c` —
//   symmetric changes on wasm: Lax field-name registration, per-field
//   tag dispatch in `_wasm_pack_to_string` / `_full`, new
//   `_wasm_stdout_display_string` + tight `_is_pack_for_stdout` guard
//   (List / HashMap / Set / Async excluded so `stdout(@[1,2,3])` still
//   prints the list form).
// * `src/types/mold_returns.rs` — `Int`/`Float`/`Bool`/`Str` moved to
//   the `Pack` table so `expr_type_tag(Float[x]())` no longer reports
//   the primitive output type and mis-routes the Lax pointer through
//   the FLOAT fast path.
// ---------------------------------------------------------------------------

#[test]
fn float_local_binding_native_parity() {
    let out = run_native(float_local_binding_td()).expect("native build+run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_BINDING,
        "native must match interpreter — Float[x]() / Int[x]() on Float local \
         must render as full-form Lax packs with `__value <= 3.0` / `__value <= 3`"
    );
}

#[test]
fn float_local_binding_wasm_parity() {
    if common::wasmtime_bin().is_none() {
        return;
    }
    let out = run_wasm_wasi(float_local_binding_td()).expect("wasm build+run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_LOCAL_BINDING,
        "wasm-wasi must match interpreter — Float[x]() / Int[x]() on Float local \
         must render as full-form Lax packs with `__value <= 3.0` / `__value <= 3`"
    );
}

#[test]
fn float_fn_result_local_native_parity() {
    let out = run_native(float_fn_result_local_td()).expect("native build+run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_FN_RESULT_LOCAL,
        "native must match interpreter — Float[triple(4.0)-via-local]() \
         must render as a full-form Lax[12.0] pack"
    );
}

#[test]
fn float_fn_result_local_wasm_parity() {
    if common::wasmtime_bin().is_none() {
        return;
    }
    let out = run_wasm_wasi(float_fn_result_local_td()).expect("wasm build+run should succeed");
    assert_eq!(
        out, EXPECTED_FLOAT_FN_RESULT_LOCAL,
        "wasm-wasi must match interpreter — Float[triple(4.0)-via-local]() \
         must render as a full-form Lax[12.0] pack"
    );
}
