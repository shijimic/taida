//! C25B-031 4-backend parity — `Slice[s, pos_var, end]()` positional-args
//! must resolve IntVars on native / wasm / js, matching the interpreter.
//!
//! Pre-fix (reproducible on upstream/main 2026-04-23):
//! * interpreter:  `World`
//! * native:       `Hello, World!`  ← pos_var ignored
//! * wasm-wasi:    `Hello, World!`  ← same
//! * js:           `Hello, World!`  ← runtime accepted `(val, opts)`
//!   and treated the IntVar as opts
//!
//! Fix:
//! * `src/codegen/lower_molds.rs::Slice` — prefer `type_args[1]` /
//!   `type_args[2]` over named `fields` (matches interpreter dispatch
//!   in `src/interpreter/mold_eval.rs:343`).
//! * `src/js/runtime/core.rs::Slice` — accept `(val, start, end)`
//!   positional form in addition to `(val, {start, end})` named form.
//!
//! Fixtures live under `examples/quality/c25b_031_slice_positional/`.

mod common;

use common::{normalize, taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn tmp_artifact(td_path: &Path, suffix: &str) -> PathBuf {
    let stem = td_path.file_stem().unwrap().to_string_lossy();
    std::env::temp_dir().join(format!(
        "c25b031_{}_{}.{}",
        std::process::id(),
        stem,
        suffix
    ))
}

fn run_js(td_path: &Path) -> Option<String> {
    let js_path = tmp_artifact(td_path, "mjs");
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
    let bin_path = tmp_artifact(td_path, "bin");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&bin_path);
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
            "native binary failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn run_wasm_wasi(td_path: &Path) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let wasm_path = tmp_artifact(td_path, "wasm");
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
        eprintln!(
            "wasmtime failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn which_node() -> Option<()> {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(()) } else { None })
}

fn fixture_td(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "examples/quality/c25b_031_slice_positional/{}.td",
        name
    ))
}

fn fixture_expected(name: &str) -> String {
    let path = PathBuf::from(format!(
        "examples/quality/c25b_031_slice_positional/{}.expected",
        name
    ));
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    normalize(&raw)
}

fn check_interpreter_fixture(name: &str) {
    let td = fixture_td(name);
    let out = run_interpreter(&td).expect("interpreter should succeed");
    let exp = fixture_expected(name);
    assert_eq!(
        out, exp,
        "interpreter output for {} drifted from .expected",
        name
    );
}

fn check_js_fixture(name: &str) {
    if which_node().is_none() {
        return;
    }
    let td = fixture_td(name);
    let exp = fixture_expected(name);
    let out = run_js(&td).unwrap_or_else(|| panic!("js build+run failed for {}", name));
    assert_eq!(
        out, exp,
        "JS output for {} diverged from interpreter reference (C25B-031 regression?)",
        name
    );
}

fn check_native_fixture(name: &str) {
    let td = fixture_td(name);
    let exp = fixture_expected(name);
    let out = run_native(&td).unwrap_or_else(|| panic!("native build+run failed for {}", name));
    assert_eq!(
        out, exp,
        "Native output for {} diverged from interpreter reference (C25B-031 regression?)",
        name
    );
}

fn check_wasm_wasi_fixture(name: &str) {
    if wasmtime_bin().is_none() {
        return;
    }
    let td = fixture_td(name);
    let exp = fixture_expected(name);
    let out =
        run_wasm_wasi(&td).unwrap_or_else(|| panic!("wasm-wasi build+run failed for {}", name));
    assert_eq!(
        out, exp,
        "wasm-wasi output for {} diverged from interpreter reference (C25B-031 regression?)",
        name
    );
}

macro_rules! c25b031_per_fixture_tests {
    ($($name:ident),* $(,)?) => {
        $(
            mod $name {
                use super::*;
                #[test] fn interp() { check_interpreter_fixture(stringify!($name)); }
                #[test] fn js() { check_js_fixture(stringify!($name)); }
                #[test] fn native() { check_native_fixture(stringify!($name)); }
                #[test] fn wasm_wasi() { check_wasm_wasi_fixture(stringify!($name)); }
            }
        )*
    };
}

c25b031_per_fixture_tests!(positional_int_var, positional_int_var_both, named_int_var,);
