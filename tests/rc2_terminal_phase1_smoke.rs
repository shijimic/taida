//! RC2 Phase 1 -- `taida-lang/terminal` package scaffold smoke tests.
//!
//! These tests cover the **taida main repo side** of the RC2 Phase 1
//! contract:
//!
//! - **RC2-1b** (addon binding metadata): the `native/addon.toml`
//!   shipped in the external `taida-lang/terminal` repository must be
//!   parseable by the RC1.5 manifest parser, with the v1-locked shape
//!   pinned in `.dev/RC2_DESIGN.md` Section E ("Package layout").
//!
//! - **RC2-1c** (import smoke): a Taida program that does
//!   `>>> taida-lang/terminal@a.1 => @(...)` must
//!   - return a deterministic "package not found" diagnostic when the
//!     addon is **not installed** under `.taida/deps/`
//!   - be rejected at compile time on every non-Native backend
//!     (`--target js` / `--target wasm-min` / `--target native`)
//!     with the deterministic policy message
//!     (`.dev/RC2_DESIGN.md` Section D, RC2B-204).
//!
//! Constraint (`RC2_IMPL_SPEC.md` G3 + RC2 ロールバック注記):
//!
//! - `taida-lang/terminal` is **NOT** a core-bundled package. It is an
//!   addon-backed external package consumed via `taida install`. These
//!   tests must therefore exercise the `.taida/deps/` resolution path,
//!   not the `CoreBundledProvider` path.
//! - We deliberately do **not** copy a real cdylib into the test
//!   project; the import-time policy guard fires off `addon.toml`
//!   alone, before the cdylib is dlopened.

#![cfg(feature = "native")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use taida::addon::manifest::parse_addon_manifest;
use taida::addon::{TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL};

// ── Helpers ─────────────────────────────────────────────────

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

/// Locate the external `taida-lang/terminal` repository on disk.
///
/// We look for it as a sibling of the main `taida` repo, matching the
/// layout `.dev/RC2_DESIGN.md` E pins (`/home/<user>/Workspace/taida/{taida,terminal}`).
/// Returns `None` if the sibling repo is not present so the tests
/// degrade to a soft skip on machines that haven't checked it out.
fn locate_terminal_repo() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest_dir.parent()?.join("terminal");
    if candidate.join("native").join("addon.toml").exists() {
        Some(candidate)
    } else {
        None
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

/// Lay down a minimal `.taida/deps/taida-lang/terminal/` directory that
/// only contains `native/addon.toml`. This is enough for the import
/// resolver and the backend-policy guard to fire — neither path needs
/// the cdylib to exist for compile-time rejection.
fn write_terminal_dep_skeleton(project_root: &Path, addon_toml: &str) {
    let native_dir = project_root
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal")
        .join("native");
    std::fs::create_dir_all(&native_dir).expect("create native dir");
    std::fs::write(native_dir.join("addon.toml"), addon_toml).expect("write addon.toml");
}

/// The exact `addon.toml` body the external repo ships in
/// `native/addon.toml`. Kept inline so this test stays self-contained
/// even when the sibling repo is not present.
const TERMINAL_ADDON_TOML: &str = r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_lang_terminal"

[functions]
terminalSize = 0
readKey = 0

[library.prebuild]
url = "https://github.com/taida-lang/terminal/releases/download/v{version}/lib{name}-{target}.{ext}"
"#;

// ── RC2-1b: Manifest schema parity ───────────────────────────

/// The inline `TERMINAL_ADDON_TOML` constant must round-trip through
/// `parse_addon_manifest` exactly the way the external repo's manifest
/// does. This is the main parity assertion for RC2-1b.
#[test]
fn terminal_addon_manifest_parses_with_v1_locked_shape() {
    let tmp = unique_temp_dir("rc2_terminal_manifest_inline");
    std::fs::create_dir_all(&tmp).unwrap();
    let manifest_path = tmp.join("addon.toml");
    std::fs::write(&manifest_path, TERMINAL_ADDON_TOML).unwrap();

    let manifest = parse_addon_manifest(&manifest_path).expect("manifest must parse");

    // ABI / entry handshake — pinned by RC1 design lock.
    assert_eq!(manifest.abi, TAIDA_ADDON_ABI_VERSION);
    assert_eq!(manifest.entry, TAIDA_ADDON_ENTRY_SYMBOL);

    // RC2 design lock E: package id and library stem are v1-frozen.
    assert_eq!(manifest.package, "taida-lang/terminal");
    assert_eq!(manifest.library, "taida_lang_terminal");

    // RC2 design lock B: function table is exactly the v1 surface.
    let fn_names: Vec<&str> = manifest.functions.keys().map(String::as_str).collect();
    assert_eq!(fn_names, vec!["readKey", "terminalSize"]);
    assert_eq!(manifest.functions.get("terminalSize").copied(), Some(0));
    assert_eq!(manifest.functions.get("readKey").copied(), Some(0));

    // RC1.5 prebuild section is configured (URL template only — no
    // targets are populated until release time).
    assert!(
        manifest.prebuild.has_prebuild(),
        "terminal addon must declare a prebuild URL template"
    );
    let url = manifest
        .prebuild
        .url_template
        .as_deref()
        .expect("url_template");
    assert!(
        url.contains("{version}")
            && url.contains("{name}")
            && url.contains("{target}")
            && url.contains("{ext}"),
        "url template must use the four canonical placeholders, got: {}",
        url
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Defence against drift: if the external repo is checked out as a
/// sibling, we re-parse its on-disk `native/addon.toml` and assert it
/// matches the inline constant. This catches the case where someone
/// edits the external repo without updating the main repo's frozen
/// expectation, and vice versa.
#[test]
fn terminal_addon_manifest_matches_external_repo_when_present() {
    let Some(repo) = locate_terminal_repo() else {
        eprintln!(
            "note: skipping external-repo parity check -- \
             sibling 'terminal' repo not found"
        );
        return;
    };

    let manifest_path = repo.join("native").join("addon.toml");
    let manifest = parse_addon_manifest(&manifest_path).expect("external manifest must parse");

    assert_eq!(manifest.abi, TAIDA_ADDON_ABI_VERSION);
    assert_eq!(manifest.entry, TAIDA_ADDON_ENTRY_SYMBOL);
    assert_eq!(manifest.package, "taida-lang/terminal");
    assert_eq!(manifest.library, "taida_lang_terminal");

    let fn_names: Vec<&str> = manifest.functions.keys().map(String::as_str).collect();
    assert_eq!(
        fn_names,
        vec!["readKey", "terminalSize"],
        "external repo function table must match the v1 lock"
    );

    assert!(
        manifest.prebuild.has_prebuild(),
        "external repo must declare a prebuild URL template"
    );
}

// ── RC2-1c: Import smoke (not-installed path) ────────────────

/// `>>> taida-lang/terminal` on a project with **no** `.taida/deps/`
/// entry must produce a deterministic failure (not a silent success
/// and not a CoreBundledProvider hit). This is the user-facing failure
/// mode they encounter before they run `taida install taida-lang/terminal`.
///
/// Critically: this test must succeed even though
/// `taida-lang/terminal` is **not** registered in
/// `CoreBundledProvider`. If the rolled-back haiku patch ever returns,
/// this test will start producing the wrong diagnostic (a core-bundled
/// version-mismatch / silent success) and fail.
///
/// Note: versioned imports (`@a.1`) are only legal inside `packages.tdm`.
/// In `.td` source the import must be unversioned, so the diagnostic
/// we expect here is the "module not found" shape that
/// `resolve_module_path` produces when the dependency is missing.
#[test]
fn terminal_import_without_install_returns_module_not_found() {
    let project = unique_temp_dir("rc2_terminal_not_installed");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    // Marker file so find_project_root anchors here.
    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"smoke\"\nversion <= \"0.1.0\"\n",
    )
    .unwrap();

    let main_td = r#">>> taida-lang/terminal => @(terminalSize, readKey)
stdout("unreachable")
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .output()
        .expect("taida binary must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stderr, stdout);

    assert!(
        !output.status.success(),
        "missing terminal addon must fail. stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        combined.contains("taida-lang/terminal") || combined.contains("terminal"),
        "diagnostic must name the package, got: {}",
        combined
    );
    // Negative assertion: a core-bundled version mismatch would have a
    // very specific message. If the rolled-back haiku patch returns,
    // this assertion fires because terminal would resolve through
    // CoreBundledProvider and the source file would not exist.
    assert!(
        !combined.contains("Core-bundled packages have a fixed version"),
        "terminal must NOT be resolved by CoreBundledProvider, got: {}",
        combined
    );
    // Negative assertion: importing must NOT silently succeed.
    assert!(
        !stdout.contains("unreachable"),
        "terminal import must fail before main runs, got stdout: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2-1c: Backend policy compile-time rejection ────────────

/// JS backend rejects `>>> taida-lang/terminal => @(...)` at codegen
/// time with the RC1 deterministic policy message. The terminal addon
/// only needs `addon.toml` to be present in `.taida/deps/` for the
/// detector to fire — no cdylib required.
#[test]
fn terminal_import_rejected_on_js_backend_at_compile_time() {
    let project = unique_temp_dir("rc2_terminal_js_reject");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    // The JS codegen needs a `packages.tdm` to seed `project_root`.
    std::fs::write(project.join("packages.tdm"), ">>> ./main.td => @(main)\n").unwrap();
    write_terminal_dep_skeleton(&project, TERMINAL_ADDON_TOML);

    let main_td = r#">>> taida-lang/terminal => @(terminalSize)
stdout("unreachable")
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.mjs"))
        .output()
        .expect("taida binary must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stderr, stdout);

    assert!(
        !output.status.success(),
        "JS codegen must reject taida-lang/terminal. stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        combined.contains("taida-lang/terminal"),
        "diagnostic must name the package, got: {}",
        combined
    );
    assert!(
        combined.contains("not supported on backend 'js'")
            && combined.contains("supported: interpreter, native, wasm-full"),
        "diagnostic must use the D28B-010 backend-policy template, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5 Phase 1: Cranelift native compile path **accepts** addon
/// package imports. The lowering layer now routes `taida-lang/terminal`
/// through `lower_addon_import`, resolves the cdylib path at build time,
/// and emits a `taida_addon_call` dispatch stub for each imported symbol.
///
/// This test does not require the real sibling `terminal` cdylib — a
/// zero-byte placeholder is enough because the cdylib is only `dlopen`ed
/// at **runtime** (and this test only exercises the build pipeline,
/// not the run step).
#[test]
fn terminal_import_accepted_on_cranelift_native_at_compile_time() {
    let project = unique_temp_dir("rc2_5_terminal_cranelift_accept");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_dep_skeleton(&project, TERMINAL_ADDON_TOML);

    // Placeholder cdylib so `resolve_cdylib_path` succeeds at build time.
    // The real dispatch logic only touches this file when the compiled
    // binary actually runs an addon function; a simple import-only
    // program (no call site) never reaches `dlopen`.
    let cdylib_stem = "libtaida_lang_terminal";
    let suffix = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    let cdylib_path = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal")
        .join("native")
        .join(format!("{}.{}", cdylib_stem, suffix));
    std::fs::write(&cdylib_path, b"").expect("write placeholder cdylib");

    // Import-only program. `terminalSize` is referenced in the import
    // statement but never called, so Phase 1 lowering only needs to
    // register the `addon_func_refs` entry and produce a valid binary.
    let main_td = r#">>> taida-lang/terminal => @(terminalSize)
stdout("rc2_5: terminal import accepted on cranelift native")
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.bin"))
        .output()
        .expect("taida binary must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stderr, stdout);

    assert!(
        output.status.success(),
        "RC2.5 contract: Cranelift native compile must accept taida-lang/terminal import. \
         stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        !combined.contains("Cranelift native backend in RC1"),
        "RC2.5 contract: the old reject message must not fire. got: {}",
        combined
    );
    assert!(
        project.join("main.bin").exists(),
        "RC2.5 contract: native build must produce an executable. combined: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// `--target wasm-min` shares the lowering pass with the Cranelift
/// native compile path, so the same addon-detection branch fires. The
/// resulting compile-time error is the same shape as the Cranelift
/// case (today the lowering layer cannot distinguish between the two
/// targets). The point of this test is to **lock** that addon-backed
/// packages are rejected on the WASM build path so a future refactor
/// of `lower.rs` cannot quietly start emitting code for them.
#[test]
fn terminal_import_rejected_on_wasm_min_at_compile_time() {
    let project = unique_temp_dir("rc2_terminal_wasm_min_reject");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_dep_skeleton(&project, TERMINAL_ADDON_TOML);

    let main_td = r#">>> taida-lang/terminal => @(terminalSize)
stdout("unreachable")
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.wasm"))
        .output()
        .expect("taida binary must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stderr, stdout);

    assert!(
        !output.status.success(),
        "wasm-min compile must reject taida-lang/terminal. stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        combined.contains("taida-lang/terminal"),
        "diagnostic must name the package, got: {}",
        combined
    );
    // The lowering pass shares the addon-detection branch with the
    // Cranelift path, so the message shape is the same in RC2.
    assert!(
        combined.contains("Cranelift native backend in RC1")
            || combined.contains("interpreter dispatch only")
            || combined.contains("not supported on backend"),
        "diagnostic must use a deterministic backend-policy template, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2B-201 guard: terminal must NOT be in CoreBundledProvider ──

/// Hard regression guard: `taida-lang/terminal` must remain absent
/// from `CoreBundledProvider::is_core_bundled`. The rolled-back haiku
/// patch added it next to `os/js/crypto/net/pool`; this assertion is
/// the canary that prevents that mistake from coming back.
#[test]
fn terminal_is_not_a_core_bundled_package() {
    use taida::pkg::provider::CoreBundledProvider;

    assert!(
        !CoreBundledProvider::is_core_bundled("taida-lang", "terminal"),
        "RC2 design lock: taida-lang/terminal is an addon-backed external \
         package and must NOT be registered in CoreBundledProvider. \
         If this fires, see RC2_PROGRESS.md Phase 1 ROLLED BACK note."
    );
    // Sanity check: the actual core-bundled set is unchanged.
    assert!(CoreBundledProvider::is_core_bundled("taida-lang", "os"));
    assert!(CoreBundledProvider::is_core_bundled("taida-lang", "crypto"));
}
