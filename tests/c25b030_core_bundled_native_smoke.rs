//! C25B-030 Phase 1D: regression guard for `taida build --target native`
//! acceptance of core-bundled packages.
//!
//! Background: C25B-030 redefined addon backend policy so `Interpreter`
//! and `Native` are first-class addon backends. The core-bundled packages
//! (`taida-lang/os` / `net` / `crypto` / `pool` / `js`) do **not** go
//! through the addon facade loader — they are hand-coded symbol
//! mappings in `src/codegen/lower/stmt.rs` (`is_core_bundled_path` branch)
//! and therefore bypass the addon dispatch path entirely.
//!
//! This smoke test pins the native-build acceptance of a minimal program
//! that imports each core-bundled package. The goal is not to exercise
//! the runtime behaviour of each symbol — existing tests cover that —
//! but to guarantee that the native backend keeps compiling imports of
//! these packages after the C25B-030 Phase E facade-loader extension
//! lands.
//!
//! Design notes:
//!
//! - `taida-lang/js` cannot be imported on the native backend (it is a
//!   JS-only mold surface); the import alone would be fine, but runtime
//!   use of its symbols would panic. We only import a symbol here, which
//!   is the facet this test actually needs to pin.
//! - `taida-lang/net` pins only symbol availability at compile time;
//!   actually running network code requires a socket and is covered by
//!   other tests.
//! - `taida-lang/os` pins `getEnv` which is pure (no filesystem /
//!   process side effects beyond reading the environment map).
//! - `taida-lang/crypto` pins `sha256` which is pure.
//! - `taida-lang/pool` pins `poolCreate` symbol availability.

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "c25b030_core_bundled_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Compile `source` with `taida build --target native` and return
/// whether the compile step itself succeeded. The produced binary is
/// discarded — we only verify the native backend accepts the program.
fn native_build_succeeds(source: &str, label: &str) -> bool {
    let dir = unique_temp_dir(label);
    let td_path = dir.join("main.td");
    fs::write(&td_path, source).expect("write main.td");
    let bin_path = dir.join("main");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("spawn taida build --target native");

    let ok = output.status.success();
    if !ok {
        eprintln!(
            "[c25b030-1d {}] native build failed\nstderr: {}\nstdout: {}",
            label,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
    }

    let _ = fs::remove_dir_all(&dir);
    ok
}

#[test]
fn c25b030_phase1d_os_getenv_compiles_native() {
    // getEnv is a pure read — no filesystem / process side effects.
    let source = "\
>>> taida-lang/os => @(getEnv)
value <= getEnv(\"PATH\", \"fallback\")
stdout(value)
";
    assert!(
        native_build_succeeds(source, "os_getenv"),
        "taida-lang/os getEnv must compile on native (C25B-030 Phase 1D regression guard)"
    );
}

#[test]
fn c25b030_phase1d_crypto_sha256_compiles_native() {
    let source = "\
>>> taida-lang/crypto => @(sha256)
digest <= sha256(\"hello\")
stdout(digest)
";
    assert!(
        native_build_succeeds(source, "crypto_sha256"),
        "taida-lang/crypto sha256 must compile on native (C25B-030 Phase 1D regression guard)"
    );
}

#[test]
fn c25b030_phase1d_net_symbol_compiles_native() {
    // We import a symbol and reference it in a simple binding. The
    // actual HTTP call is not exercised here — tests/parity.rs Phase 3
    // covers runtime semantics.
    let source = "\
>>> taida-lang/net => @(httpParseRequestHead)
parser <= httpParseRequestHead
stdout(\"ok\")
";
    assert!(
        native_build_succeeds(source, "net_symbol"),
        "taida-lang/net import must compile on native (C25B-030 Phase 1D regression guard)"
    );
}

#[test]
fn c25b030_phase1d_pool_symbol_compiles_native() {
    let source = "\
>>> taida-lang/pool => @(poolCreate)
factory <= poolCreate
stdout(\"ok\")
";
    assert!(
        native_build_succeeds(source, "pool_symbol"),
        "taida-lang/pool import must compile on native (C25B-030 Phase 1D regression guard)"
    );
}

#[test]
fn c25b030_phase1d_js_import_compiles_native() {
    // The js package is JS-only at runtime, but importing a symbol
    // must still resolve at native build time (the core-bundled symbol
    // table binds sentinels for every backend). Runtime use would
    // trigger a deterministic error; we do not invoke the sentinel.
    let source = "\
>>> taida-lang/js => @(jsEval)
handle <= jsEval
stdout(\"ok\")
";
    // Note: if jsEval is not a registered core-bundled symbol, the
    // native backend will reject the import at lower time — which is
    // still information we want (the test then tells us the symbol
    // table drifted, not that the backend broke).
    assert!(
        native_build_succeeds(source, "js_symbol"),
        "taida-lang/js import must compile on native (C25B-030 Phase 1D regression guard)"
    );
}
