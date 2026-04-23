//! C25B-033 4-backend parity — user FuncDef whose Taida-level name collides
//! with a prelude reserved identifier on the JS backend.
//!
//! Pre-fix (reproducible on upstream/main 2026-04-23, `0fd0029`):
//! * interpreter: `hi-world` (or `55`, etc. — correct)
//! * native: same correct output
//! * wasm-wasi: same correct output
//! * js: `SyntaxError: Identifier 'Join' has already been declared`
//!   (node ESM rejects duplicate top-level declaration)
//!
//! Fix:
//! * `src/js/codegen.rs::PRELUDE_RESERVED_IDENTS` +
//!   `JsCodegen::js_user_func_ident` — user FuncDefs whose names collide
//!   with a top-level `function X(...)` in the JS runtime prelude are
//!   emitted under the mangled form `_td_user_<name>` at every reference
//!   site (declaration, trampoline wrapper, TCO inner call, direct call
//!   callee, pipeline fallback). Non-colliding PascalCase user FuncDefs
//!   are emitted verbatim so the Taida surface invariant (users may name
//!   freely) is preserved for the common case.
//!
//! Fixtures live under `examples/quality/c25b_033_pascal_funcdef/`.
//!
//! Regression matrix:
//! * `join_collision` — prelude `Join(list, sep)` vs user `Join a b`
//! * `concat_collision` — prelude `Concat(list, other)` vs user `Concat a b`
//! * `count_recursive` — tail-recursive user `Count n acc` exercising
//!   the JS trampoline (`__inner_<name>` +
//!   `const <name> = __taida_trampoline(__inner_<name>)`)
//! * `sum_pipeline` — direct-call form of prelude `Sum(list)` vs
//!   user `Sum a b` (covers `Expr::Ident` emission)
//! * `noncollision_pascal` — guardrail: PascalCase user name with no
//!   prelude collision must NOT be mangled.

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
        "c25b033_{}_{}.{}",
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

fn fixture_td(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "examples/quality/c25b_033_pascal_funcdef/{}.td",
        name
    ))
}

fn fixture_expected(name: &str) -> String {
    let path = PathBuf::from(format!(
        "examples/quality/c25b_033_pascal_funcdef/{}.expected",
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
        "JS output for {} diverged from interpreter reference (C25B-033 regression?)",
        name
    );
}

fn check_native_fixture(name: &str) {
    let td = fixture_td(name);
    let exp = fixture_expected(name);
    let out = run_native(&td).unwrap_or_else(|| panic!("native build+run failed for {}", name));
    assert_eq!(
        out, exp,
        "Native output for {} diverged from interpreter reference (C25B-033 regression?)",
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
        "wasm-wasi output for {} diverged from interpreter reference (C25B-033 regression?)",
        name
    );
}

macro_rules! c25b033_per_fixture_tests {
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

c25b033_per_fixture_tests!(
    join_collision,
    concat_collision,
    count_recursive,
    sum_pipeline,
    noncollision_pascal,
);

/// Structural guardrail: the JS build artefact for the `noncollision_pascal`
/// fixture must emit `function MyCustomFunc(...)` verbatim (no `_td_user_`
/// prefix). Pins the "mangling is collision-only" promise so that a future
/// over-eager regression (e.g. mangling every PascalCase FuncDef) is caught
/// at the bit level, not just at runtime output parity.
#[test]
fn noncollision_pascal_is_not_mangled() {
    if which_node().is_none() {
        return;
    }
    let td = fixture_td("noncollision_pascal");
    // Use a dedicated suffix so we don't race with the parallel
    // `noncollision_pascal::js` functional test, which also writes an
    // `.mjs` artefact under the same stem + PID and deletes it after use.
    let js_path = tmp_artifact(&td, "struct.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&td)
        .arg("-o")
        .arg(&js_path)
        .output()
        .expect("js build spawn");
    assert!(
        build.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let src = std::fs::read_to_string(&js_path).expect("read emitted .mjs");
    let _ = std::fs::remove_file(&js_path);
    assert!(
        src.contains("function MyCustomFunc("),
        "expected `function MyCustomFunc(` in emitted JS — non-colliding \
         PascalCase user FuncDefs must be emitted verbatim (no mangling)"
    );
    assert!(
        !src.contains("_td_user_MyCustomFunc"),
        "non-colliding PascalCase user FuncDef was mangled — C25B-033 \
         mangling must be collision-only"
    );
}

/// Structural guardrail (positive): the JS build artefact for the
/// `join_collision` fixture must emit the mangled declaration
/// `function _td_user_Join(` AND preserve the prelude `function Join(` so
/// both coexist without `SyntaxError: Identifier 'Join' has already been
/// declared`.
#[test]
fn join_collision_is_mangled() {
    if which_node().is_none() {
        return;
    }
    let td = fixture_td("join_collision");
    // Same parallel-race protection as `noncollision_pascal_is_not_mangled`.
    let js_path = tmp_artifact(&td, "struct.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&td)
        .arg("-o")
        .arg(&js_path)
        .output()
        .expect("js build spawn");
    assert!(
        build.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let src = std::fs::read_to_string(&js_path).expect("read emitted .mjs");
    let _ = std::fs::remove_file(&js_path);
    assert!(
        src.contains("function _td_user_Join("),
        "expected mangled declaration `function _td_user_Join(` in emitted \
         JS — user FuncDef colliding with prelude `Join` must be mangled"
    );
    assert!(
        src.contains("function Join("),
        "expected prelude `function Join(` to remain intact — mangling must \
         not touch the runtime helper"
    );
    assert!(
        src.contains("_td_user_Join(\"hi\", \"world\")")
            || src.contains("_td_user_Join('hi', 'world')"),
        "expected mangled call site `_td_user_Join(...)` — call-side \
         rewrite must follow the declaration"
    );
}
