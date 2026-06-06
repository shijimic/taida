//! F54: bundled package catalog behavior pins.
//!
//! The `taida-lang/*` surface is now declared once in
//! `src/pkg/catalog.rs` and every layer (resolver, interpreter
//! materialization, checker import validation, native lowering
//! classification, JS codegen) derives its view from it. These tests pin
//! the user-visible consequences:
//!
//! - `taida-lang/build` is an actual bundled descriptor package: the
//!   documented `>>> taida-lang/build => @(BuildUnit)` import resolves on
//!   the interpreter and native (it used to fail with "Package not
//!   found"), while the import-less descriptor spelling keeps working.
//! - Unknown symbols on ANY bundled package are a checker diagnostic
//!   (they used to be validated for net/abi only; an os/pool typo built
//!   fine and silently lowered to nothing).
//! - `taida-lang/js` imports resolve on the interpreter (sentinel
//!   injection existed for every other package but js).

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn unique_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("f54cat_{}_{}_{}", label, std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run_interp(source: &str, label: &str) -> (bool, String, String) {
    let dir = unique_dir(label);
    let td = dir.join("main.td");
    fs::write(&td, source).expect("write main.td");
    let out = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("spawn taida (interpreter)");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let ok = out.status.success();
    let _ = fs::remove_dir_all(&dir);
    (ok, stdout, stderr)
}

fn native_build_and_run(source: &str, label: &str) -> (bool, String, String) {
    let dir = unique_dir(label);
    let td = dir.join("main.td");
    let bin = dir.join("app");
    fs::write(&td, source).expect("write main.td");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(&td)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("spawn taida build native");
    if !build.status.success() {
        let stderr = String::from_utf8_lossy(&build.stderr).to_string();
        let _ = fs::remove_dir_all(&dir);
        return (false, String::new(), stderr);
    }
    let run = Command::new(&bin).output().expect("run native binary");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let ok = run.status.success();
    let _ = fs::remove_dir_all(&dir);
    (ok, stdout, stderr)
}

const BUILD_IMPORT_SOURCE: &str = "\
>>> taida-lang/build => @(BuildUnit)

serverMain =
  stdout(\"hello\")
  0
=> :Int

u <= BuildUnit(name <= \"server-x\", target <= \"native\", entry <= serverMain)
stdout(\"descriptor-ok\")
";

const BUILD_IMPORTLESS_SOURCE: &str = "\
serverMain =
  stdout(\"hello\")
  0
=> :Int

u <= BuildUnit(name <= \"server-x\", target <= \"native\", entry <= serverMain)
stdout(\"descriptor-ok\")
";

#[test]
fn f54_build_import_resolves_on_interpreter() {
    let (ok, stdout, stderr) = run_interp(BUILD_IMPORT_SOURCE, "build_import_interp");
    assert!(
        ok,
        "documented `>>> taida-lang/build` import must resolve on the interpreter.\nstderr: {}",
        stderr
    );
    assert!(stdout.contains("descriptor-ok"), "got: {}", stdout);
}

#[test]
fn f54_build_import_resolves_on_native() {
    let (ok, stdout, stderr) = native_build_and_run(BUILD_IMPORT_SOURCE, "build_import_native");
    assert!(
        ok,
        "documented `>>> taida-lang/build` import must build and run on native.\nstderr: {}",
        stderr
    );
    assert!(stdout.contains("descriptor-ok"), "got: {}", stdout);
}

#[test]
fn f54_build_importless_descriptor_still_works() {
    // The descriptor parser recognizes `BuildUnit(...)` by type name with
    // or without an import; adding the bundled stub must not regress the
    // import-less spelling.
    let (ok, stdout, stderr) = run_interp(BUILD_IMPORTLESS_SOURCE, "build_importless_interp");
    assert!(
        ok,
        "import-less descriptor spelling broke.\nstderr: {}",
        stderr
    );
    assert!(stdout.contains("descriptor-ok"), "got: {}", stdout);
}

#[test]
fn f54_js_import_resolves_on_interpreter() {
    // D-2 fix: `taida-lang/js` materialized but had no sentinel injection,
    // so the import statement itself failed at export collection. The
    // descriptors are still JS-backend-only at evaluation time.
    let source = "\
>>> taida-lang/js => @(JSGet)
stdout(\"js-import-ok\")
";
    let (ok, stdout, stderr) = run_interp(source, "js_import_interp");
    assert!(
        ok,
        "`>>> taida-lang/js` import must resolve on the interpreter.\nstderr: {}",
        stderr
    );
    assert!(stdout.contains("js-import-ok"), "got: {}", stdout);
}

#[test]
fn f54_unknown_symbol_diagnostic_is_uniform() {
    // F54B-003: a typo'd import symbol must be a checker diagnostic on
    // every bundled package, with the same message shape net/abi already
    // had. Before the catalog, os/crypto/pool/js/build skipped validation.
    for (pkg, bogus) in [
        ("os", "getEnv"),
        ("crypto", "sha512"),
        ("pool", "poolCreat"),
        ("js", "JSEval"),
        ("build", "BuildUnitt"),
        ("net", "dnsResolve"),
        ("abi", "CliRequest"),
    ] {
        let source = format!(
            ">>> taida-lang/{} => @({})\nstdout(\"never\")\n",
            pkg, bogus
        );
        let (ok, stdout, stderr) = run_interp(&source, &format!("typo_{}", pkg));
        assert!(
            !ok,
            "taida-lang/{}: unknown symbol '{}' must fail type check (stdout: {})",
            pkg, bogus, stdout
        );
        assert!(
            stderr.contains(&format!(
                "Symbol '{}' not found in module 'taida-lang/{}'",
                bogus, pkg
            )),
            "taida-lang/{}: diagnostic must use the uniform shape.\nstderr: {}",
            pkg,
            stderr
        );
        assert!(
            !stdout.contains("never"),
            "taida-lang/{}: program must not run past a bad import",
            pkg
        );
    }
}

#[test]
fn f54_known_symbols_still_import_cleanly() {
    // Smoke: one real symbol per package must keep passing validation.
    // (Execution is covered elsewhere; `js` use is JS-target-only and
    // build descriptors are driver-only, so this only runs imports.)
    let source = "\
>>> taida-lang/os => @(EnvVar)
>>> taida-lang/crypto => @(sha256)
>>> taida-lang/pool => @(poolCreate)
>>> taida-lang/net => @(httpServe)
>>> taida-lang/abi => @(WebRequest)
>>> taida-lang/js => @(JSGet)
>>> taida-lang/build => @(BuildUnit)
stdout(\"all-imports-ok\")
";
    let (ok, stdout, stderr) = run_interp(source, "known_symbols");
    assert!(ok, "real exports must validate.\nstderr: {}", stderr);
    assert!(stdout.contains("all-imports-ok"), "got: {}", stdout);
}
