// Codegen lower-side `expr_is_bool` parity gaps.
//
// E34 Phase 2 (Lock-B=C, 2026-05-09): both gaps closed by routing
// `expr_is_bool` through the type-checker's Typed HIR side table.
// When the table holds a typed decision for the expression, that
// decision wins outright — both directions:
//
//   - FALSE POSITIVE: a user-defined pack whose field shadows a
//     built-in Bool method name now reports the field's actual
//     return type. The type-checker's non-Bool entry wins before
//     method-name syntax can misclassify the call.
//   - FALSE NEGATIVE: a cross-module `:Bool` function lands in the
//     typed table even though it is defined in another module.
//
// Both fixtures below are active regression coverage across the native and
// wasm lowering paths (no longer #[ignore]'d).

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn wasmtime_bin() -> Option<PathBuf> {
    common::wasmtime_bin()
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "expr_is_bool_completeness_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("mkdir fixture");
    dir
}

fn run_four_backends(main_path: &std::path::Path, dir: &std::path::Path) -> [(String, String); 4] {
    let interp = {
        let out = Command::new(taida_bin())
            .arg(main_path)
            .output()
            .expect("interp run");
        assert!(
            out.status.success(),
            "interp failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let js = if node_available() {
        let mjs = dir.join("main.mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(main_path)
            .arg("-o")
            .arg(&mjs)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("node").arg(&mjs).output().expect("node run");
        assert!(
            run.status.success(),
            "js run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("node unavailable; skipping JS leg");
        String::new()
    };

    let native = if cc_available() {
        let bin = dir.join("main.bin");
        let build = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(main_path)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&bin).output().expect("native run");
        assert!(
            run.status.success(),
            "native run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("cc unavailable; skipping native leg");
        String::new()
    };

    let wasm_full = if let Some(wasmtime) = wasmtime_bin() {
        let wasm = dir.join("main.wasm");
        let build = Command::new(taida_bin())
            .args(["build", "wasm-full"])
            .arg(main_path)
            .arg("-o")
            .arg(&wasm)
            .output()
            .expect("build wasm-full");
        assert!(
            build.status.success(),
            "wasm-full build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(wasmtime)
            .args(["run", "--"])
            .arg(&wasm)
            .output()
            .expect("wasmtime run");
        let _ = fs::remove_file(&wasm);
        assert!(
            run.status.success(),
            "wasm-full run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("wasmtime unavailable; skipping wasm-full leg");
        String::new()
    };

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
        ("wasm-full".to_string(), wasm_full),
    ]
}

#[test]
fn expr_is_bool_cross_module_bool_get_or_default_four_backend_parity() {
    // FALSE NEGATIVE — a Bool fn imported from another module must be
    // typed from the import signature, otherwise `getOrDefault(...)`
    // falls through to the polymorphic stringifier on Native.
    let dir = fixture_dir("cross_module");
    let lib = dir.join("lib.td");
    let main = dir.join("main.td");

    fs::write(
        &lib,
        "giveTrue x: Int = x > 0 => :Bool\n\n<<< @(giveTrue)\n",
    )
    .expect("write lib");
    fs::write(
        &main,
        ">>> ./lib.td => @(giveTrue)\n\nempty: @[Bool] <= @[]\nb <= empty.first().getOrDefault(giveTrue(5))\nstdout(\"bool:\" + b.toString())\n",
    )
    .expect("write main");

    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "bool:true",
        "interp must render the Bool surface form"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interp (false-negative gap: cross-module Bool fn not in local registry)",
            backend
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn expr_is_bool_pack_field_shadows_bool_method_four_backend_parity() {
    // FALSE POSITIVE — a user-defined pack with a field named like a
    // built-in Bool method (`has`, `isEmpty`, `contains`, ...) must not
    // be classified from the method name alone. Every enabled backend should
    // render the Int value.
    let dir = fixture_dir("pack_shadow");
    let main = dir.join("main.td");

    fs::write(
        &main,
        "Box = @(label: Str, has: Int => :Int)\nb <= Box(label <= \"demo\")\nresult <= b.has(7)\nstdout(result.toString())\n",
    )
    .expect("write main");

    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    // The defaultFn for an unbound `Int => :Int` field returns 0; all enabled
    // backends should agree on the Int representation.
    assert_eq!(
        interp, "0",
        "interp must render the underlying Int (defaultFn returns 0)"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interp (false-positive gap: method name classified without checking receiver type)",
            backend
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
