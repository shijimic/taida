//! C24 4-backend parity test — pins the `Zip[]()` / `Enumerate[]()` /
//! Gorillax parity fixes landed under C24-A and C24-B.
//!
//! - `str_from_gorillax` is already pinned by `c23_str_parity.rs` (and
//!   was promoted to 4-backend by C24-A's field-name unification from
//!   `isOk` → `hasValue`). This file covers the new C24-B fixtures:
//!   `str_from_zip` / `str_from_zip_uneven` / `str_from_enumerate` /
//!   `str_from_enumerate_empty`.
//! - All fixtures live under `examples/quality/c24b_collection_parity/`.
//! - Interpreter is the reference (`src/interpreter/prelude.rs` `zip` /
//!   `enumerate`). JS / Native / WASM-wasi must match byte-for-byte.
//!
//! C24-B root cause recap:
//!   * Native `taida_list_zip` / `taida_list_enumerate` + WASM
//!     `taida_list_zip` / `taida_list_enumerate` created pair packs with
//!     `HASH_FIRST` / `HASH_SECOND` / `HASH_INDEX` / `HASH_VALUE` hashes
//!     stamped, but never registered the matching names (`"first"` /
//!     `"second"` / `"index"` / `"value"`) in the runtime's field-name
//!     registry. `taida_pack_to_display_string_full` /
//!     `_wasm_pack_to_string_full` returned NULL from the name lookup
//!     and silently skipped every pair field — the outer list rendered
//!     as `@[]`.
//!   * Additionally, the outer list's `elem_type_tag` was
//!     `TAIDA_TAG_PACK` / `WASM_TAG_PACK`, which forced the pair values
//!     through the recursive full-form renderer. Without
//!     per-field-tag stamps on the pair's slots, primitive INT values
//!     (e.g. `1`) dereferenced as `(char*)1` and segfaulted in
//!     `taida_read_cstr_len_safe` (native) / trapped with "uninitialized
//!     element" in wasm.
//!   * Fix: idempotent `taida_register_zip_enumerate_field_names` /
//!     `_wasm_register_zip_enumerate_field_names` (C23B-009 pattern) +
//!     propagate source-list `elem_type_tag` onto each pair's value
//!     slots + explicit `render_int` / `render_str` branches in
//!     `taida_pack_to_display_string_full` symmetric with the existing
//!     WASM version. See `docs/reference/standard_library.md:238`.

mod common;

use common::{normalize, taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Backend runners (verbatim clones of the c23_str_parity.rs helpers — no
// further refactoring inside C24 to keep the diff minimal).
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

fn tmp_artifact(td_path: &Path, suffix: &str) -> PathBuf {
    let stem = td_path.file_stem().unwrap().to_string_lossy();
    std::env::temp_dir().join(format!(
        "c24_parity_{}_{}.{}",
        std::process::id(),
        stem,
        suffix
    ))
}

fn run_js(td_path: &Path) -> Option<String> {
    let js_path = tmp_artifact(td_path, "mjs");
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

fn run_native(td_path: &Path) -> Option<String> {
    let bin_path = tmp_artifact(td_path, "bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
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

// ---------------------------------------------------------------------------
// Fixtures (C24-B new parity coverage).
// ---------------------------------------------------------------------------

const C24B_FIXTURES: &[&str] = &[
    "str_from_zip",
    "str_from_zip_uneven",
    "str_from_enumerate",
    "str_from_enumerate_empty",
    // C25B-027 (2026-04-23): function-form `zip(a, b)` / `enumerate(xs)`
    // — the mold form was fixed by C24-B, but the function form was not
    // routed to `taida_list_zip` / `taida_list_enumerate` on native /
    // wasm and crashed (segfault 139 / `uninitialized element` trap).
    // Fix lives in `src/codegen/lower/core.rs::stdlib_runtime_funcs` so
    // both spellings share the same runtime helper emission path.
    "str_from_zip_fn",
    "str_from_enumerate_fn",
];

fn fixture_td(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "examples/quality/c24b_collection_parity/{}.td",
        name
    ))
}

fn fixture_expected(name: &str) -> String {
    let path = PathBuf::from(format!(
        "examples/quality/c24b_collection_parity/{}.expected",
        name
    ));
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    normalize(&raw)
}

// ---------------------------------------------------------------------------
// Interpreter reference — pin `.expected` against the source of truth first.
// ---------------------------------------------------------------------------

// C24 Phase 5 (RC-SLOW-2 / C24B-006): per-fixture decomposition.

fn check_interpreter_fixture(name: &str) {
    let td = fixture_td(name);
    let out = run_interpreter(&td).expect("interpreter should succeed");
    let exp = fixture_expected(name);
    assert_eq!(
        out, exp,
        "interpreter output for {} drifted from .expected (source of truth)",
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
        "JS output for {} diverged from interpreter reference (C24-B regression?)",
        name
    );
}

fn check_native_fixture(name: &str) {
    let td = fixture_td(name);
    let exp = fixture_expected(name);
    let out = run_native(&td).unwrap_or_else(|| panic!("native build+run failed for {}", name));
    assert_eq!(
        out, exp,
        "Native output for {} diverged from interpreter reference (C24-B regression?)",
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
        "wasm-wasi output for {} diverged from interpreter reference (C24-B regression?)",
        name
    );
}

macro_rules! c24_per_fixture_tests {
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

c24_per_fixture_tests!(
    str_from_zip,
    str_from_zip_uneven,
    str_from_enumerate,
    str_from_enumerate_empty,
    str_from_zip_fn,
    str_from_enumerate_fn,
);

#[test]
fn c24b_fixture_list_sync_guard() {
    // The per-fixture macro invocation above must mirror C24B_FIXTURES.
    let macro_list: &[&str] = &[
        "str_from_zip",
        "str_from_zip_uneven",
        "str_from_enumerate",
        "str_from_enumerate_empty",
        "str_from_zip_fn",
        "str_from_enumerate_fn",
    ];
    assert_eq!(
        C24B_FIXTURES.len(),
        macro_list.len(),
        "C24B_FIXTURES count != c24_per_fixture_tests!() invocation count",
    );
    for (a, b) in C24B_FIXTURES.iter().zip(macro_list) {
        assert_eq!(a, b);
    }
}
