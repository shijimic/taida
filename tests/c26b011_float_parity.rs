//! C26B-011 (Phase 11) — Float parity 3-backend regression guard
//!
//! Scope: NaN / +Infinity / -Infinity parity between Interpreter
//! (reference), JS, and Native for `Div` / `Mod` molds and math mold
//! family (`Sqrt` / `Pow` / …). wasm-wasi is out of C26 scope (D27
//! send-off per `project_net_stable_viewpoint_gap.md`); wasm profiles
//! are explicitly skipped here to avoid regressing D27 blockers while
//! still pinning the 3-backend parity that C26B-011 must deliver.
//!
//! Root causes fixed (see blocker):
//!
//! 1. **Native `taida_debug_float` used `%g`** — dropped `.0` on
//!    integer-valued floats (`1.0` → `1`) and diverged from the
//!    `taida_float_to_str` formatter used everywhere else. Fixed by
//!    routing through `taida_float_to_str`.
//! 2. **Native `Div` / `Mod` always returned Int-tagged Lax** —
//!    Float-origin args produced bit-patterns rendered as Int. Added
//!    `taida_div_mold_f` / `taida_mod_mold_f` (Float-hint variants)
//!    dispatched from `lower_molds.rs` when `expr_returns_float`
//!    matches. Lax slots are tagged `TAIDA_TAG_FLOAT` so
//!    `taida_lax_to_string` routes through `taida_float_to_str`.
//! 3. **Native unmold target missed Float origin** — `Div[1.0, 2.0]()
//!    ]=> r` left `r` untagged; `debug(r)` fell through to
//!    `taida_debug_int`. Extended `track_unmold_type` / `_by_mold_name`
//!    to cover `Div` / `Mod` (Float-arg case) and math molds.
//! 4. **JS `__taida_float_render` missed NaN / Inf** — `String(Infinity)`
//!    produces `"Infinity"` (drift). Added explicit NaN / Inf / -Inf
//!    branches matching Rust's `f64::Display`.
//! 5. **JS Lax default path dropped `__floatHint`** — `Div[1.0, 0.0]()`
//!    returned `Lax(default: 0)` not `Lax(default: 0.0)`. Fixed by
//!    threading `floatHint` into `Lax()` constructor and rendering via
//!    `__taida_float_render` when set. `is_float_origin_expr` in JS
//!    codegen recognises `Div` / `Mod` MoldInst so unmold target gets
//!    Float-origin and `debug` dispatches to `__taida_debug_f`.

mod common;

use common::{normalize, taida_bin};
use std::path::Path;
use std::process::Command;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new("examples/quality/c26_float_edge").join(name)
}

fn read_expected(name: &str) -> String {
    let path = fixture(name);
    let content = std::fs::read_to_string(&path)
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

fn run_js(td_path: &Path) -> Option<String> {
    which_node()?;
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let js_path = std::env::temp_dir().join(format!("c26b011_{}_{}.mjs", std::process::id(), stem));
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
        std::env::temp_dir().join(format!("c26b011_{}_{}.bin", std::process::id(), stem));
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

fn which_node() -> Option<std::path::PathBuf> {
    let out = Command::new("which").arg("node").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(s))
    }
}

// ---------------------------------------------------------------------------
// div_mod_float.td — integer + float-origin Div / Mod through `]=>` unmold
// ---------------------------------------------------------------------------

#[test]
fn div_mod_float_interpreter_reference() {
    let td = fixture("div_mod_float.td");
    let expected = read_expected("div_mod_float.expected");
    let got = run_interpreter(&td).expect("interpreter should succeed");
    assert_eq!(got, expected, "interpreter reference mismatch");
}

#[test]
fn div_mod_float_js_parity() {
    if which_node().is_none() {
        return;
    }
    let td = fixture("div_mod_float.td");
    let expected = read_expected("div_mod_float.expected");
    let got = run_js(&td).expect("js run should succeed");
    assert_eq!(got, expected, "JS must match interpreter reference");
}

#[test]
fn div_mod_float_native_parity() {
    let td = fixture("div_mod_float.td");
    let expected = read_expected("div_mod_float.expected");
    let got = run_native(&td).expect("native run should succeed");
    assert_eq!(got, expected, "Native must match interpreter reference");
}

// ---------------------------------------------------------------------------
// nan_inf_parity.td — IEEE 754 NaN / +Inf / -Inf
// ---------------------------------------------------------------------------

#[test]
fn nan_inf_interpreter_reference() {
    let td = fixture("nan_inf_parity.td");
    let expected = read_expected("nan_inf_parity.expected");
    let got = run_interpreter(&td).expect("interpreter should succeed");
    assert_eq!(got, expected, "interpreter reference mismatch (NaN/Inf)");
}

#[test]
fn nan_inf_js_parity() {
    if which_node().is_none() {
        return;
    }
    let td = fixture("nan_inf_parity.td");
    let expected = read_expected("nan_inf_parity.expected");
    let got = run_js(&td).expect("js run should succeed");
    assert_eq!(
        got, expected,
        "JS must match interpreter reference (NaN/Inf)"
    );
}

#[test]
fn nan_inf_native_parity() {
    let td = fixture("nan_inf_parity.td");
    let expected = read_expected("nan_inf_parity.expected");
    let got = run_native(&td).expect("native run should succeed");
    assert_eq!(
        got, expected,
        "Native must match interpreter reference (NaN/Inf)"
    );
}
