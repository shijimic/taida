//! E30B-007 / Lock-G — addon facade explicit binding integration tests.
//!
//! Phase 7 sub-track B (sub-step B-2) lands the new explicit
//! addon-binding form `Name <= RustAddon["fn"](arity <= N)` across
//! the Interpreter (runtime eval) and the codegen-side facade
//! summary (`src/addon/facade.rs`) consumed by native / JS / wasm
//! lowering. This test file exercises the surface end-to-end on
//! both the Interpreter and the native Cranelift backend so a
//! user-side `>>> taida-lang/test => @(TerminalSize)` import
//! resolves identically whether the facade uses the legacy
//! `TerminalSize <= terminalSize` alias path or the new explicit
//! `TerminalSize <= RustAddon["terminalSize"](arity <= 0)` path.
//!
//! Out of scope (deferred to sub-step B-5 / TM-coordinated session):
//!
//! - Removal of the legacy implicit pre-injection in
//!   `load_addon_facade` (would break TM track's b-gen rollout).
//! - Migration of the live `taida-lang/terminal` 23 sentinels to
//!   the explicit form (TM track responsibility).
//! - Hard-fail `[E1412]` checker diagnostic for facades that omit
//!   any explicit binding (coordinated rollout).

#![cfg(feature = "native")]

mod common;

use common::taida_bin as resolve_taida_bin;
use std::path::{Path, PathBuf};
use std::process::Command;

fn taida_bin() -> PathBuf {
    resolve_taida_bin()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "e30b_007_{}_{}_{}",
        prefix,
        std::process::id(),
        nanos
    ))
}

/// Minimal manifest used by every test case. Two functions with
/// distinct arities so drift / unknown checks hit deterministic
/// targets.
const ADDON_TOML: &str = r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_lang_terminal"

[functions]
terminalSize = 0
readKey = 0
"#;

/// Lay down a `taida-lang/terminal` fixture skeleton with a
/// caller-supplied facade body so each test exercises a different
/// surface shape. Mirrors the helper in
/// `tests/c25b030_phase_1e_facade_chain.rs` to keep diff noise low.
fn write_terminal_fixture(project: &Path, facade_td: &str) {
    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"e30b-007-test\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    let pkg = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal");
    std::fs::create_dir_all(pkg.join("native")).expect("create native dir");
    std::fs::create_dir_all(pkg.join("taida")).expect("create taida dir");

    std::fs::write(pkg.join("native").join("addon.toml"), ADDON_TOML).expect("write addon.toml");
    std::fs::write(pkg.join("taida").join("terminal.td"), facade_td)
        .expect("write terminal facade");

    let suffix = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    std::fs::write(
        pkg.join("native")
            .join(format!("libtaida_lang_terminal.{}", suffix)),
        b"",
    )
    .expect("write placeholder cdylib");

    std::fs::write(
        pkg.join("packages.tdm"),
        "name <= \"taida-lang/terminal\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write pkg packages.tdm");
}

fn build_native(project: &Path, main_td: &str) -> (bool, String, String) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let output = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.bin"))
        .current_dir(project)
        .output()
        .expect("taida binary must run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

// ── Positive: explicit RustAddon[...] binding compiles natively ──

/// Lock-G smoke test on the Cranelift native path: a facade that
/// re-exports `terminalSize` as `TerminalSize` via the new explicit
/// binding form must build. The codegen-side facade summary
/// recognises the binding as an alias (identical shape to the
/// legacy `TerminalSize <= terminalSize` path) so no per-backend
/// changes are required.
#[test]
fn e30b_007_native_build_accepts_explicit_rust_addon_binding() {
    let project = unique_temp_dir("native_explicit");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let facade = r#"
TerminalSize <= RustAddon["terminalSize"](arity <= 0)

<<< @(TerminalSize)
"#;
    write_terminal_fixture(&project, facade);

    let main_td = r#">>> taida-lang/terminal => @(TerminalSize)

// Confirm import resolves at build time without invoking the addon
// (the placeholder cdylib has no real symbols). Body just exercises
// the side-effect path so the binary is non-empty.
stdout("phase 7B-2 explicit binding linked")
"#;
    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "Lock-G: explicit RustAddon[...] binding must build on native. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(
        project.join("main.bin").exists(),
        "native build must produce an executable. stderr={}",
        stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── Negative: arity drift is rejected at build time ──

/// Lock-G drift check: a facade declaring `arity <= 5` for a
/// manifest entry of `arity = 0` must fail at native build time
/// with `[E1412]`. The error must propagate through `taida build`
/// stderr so CI / IDE consumers can route the diagnostic.
#[test]
fn e30b_007_native_build_rejects_rust_addon_arity_drift() {
    let project = unique_temp_dir("native_drift");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let facade = r#"
TerminalSize <= RustAddon["terminalSize"](arity <= 5)

<<< @(TerminalSize)
"#;
    write_terminal_fixture(&project, facade);

    let main_td = r#">>> taida-lang/terminal => @(TerminalSize)
stdout("should-not-build")
"#;
    let (ok, _stdout, stderr) = build_native(&project, main_td);
    assert!(!ok, "Lock-G drift must fail the native build");
    assert!(
        stderr.contains("[E1412]"),
        "expected [E1412] in stderr, got: {}",
        stderr
    );
    assert!(
        stderr.contains("drift") || stderr.contains("arity"),
        "stderr must mention drift, got: {}",
        stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── Negative: unknown function name is rejected ──

/// Lock-G: a facade naming a function that is not present in the
/// manifest `[functions]` table must fail at native build time
/// with `[E1412]`. Symmetric to the legacy alias path's "not listed
/// in [functions]" diagnostic but routed through the new explicit
/// binding handler.
#[test]
fn e30b_007_native_build_rejects_rust_addon_unknown_fn() {
    let project = unique_temp_dir("native_unknown");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let facade = r#"
Foo <= RustAddon["doesNotExist"](arity <= 0)

<<< @(Foo)
"#;
    write_terminal_fixture(&project, facade);

    let main_td = r#">>> taida-lang/terminal => @(Foo)
stdout("should-not-build")
"#;
    let (ok, _stdout, stderr) = build_native(&project, main_td);
    assert!(!ok, "Lock-G unknown fn must fail the native build");
    assert!(
        stderr.contains("[E1412]"),
        "expected [E1412] in stderr, got: {}",
        stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}
