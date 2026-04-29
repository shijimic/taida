//! RC2.5 Phase 2 -- Cranelift native backend: MoldInst addon dispatch
//! and facade pack bindings.
//!
//! Phase 1 (`tests/rc2_terminal_phase1_smoke.rs::terminal_import_accepted_on_cranelift_native_at_compile_time`)
//! only exercised the lowercase `@(terminalSize)` path, where the
//! import resolver finds the function directly in `addon.toml::[functions]`
//! and the user program never actually calls anything. Phase 2
//! (RC2.5-2a / RC2.5-2b) exercises the full RC2 facade surface on
//! the Cranelift native path:
//!
//!   1. Uppercase facade aliases (`TerminalSize <= terminalSize`)
//!      route through `taida_addon_call` when the user writes
//!      `TerminalSize[]()` or `terminalSize()`. Both forms must
//!      resolve to the same addon sentinel.
//!   2. Pure-Taida facade pack bindings (`KeyKind <= @(Char <= 0, ...)`)
//!      land as synthetic top-level assignments at the top of
//!      `_taida_main`, so user code can read `KeyKind.Char` without
//!      the facade file ever being parsed by the main program.
//!   3. Addon return-value unpacking (`size.cols`, `size.rows`,
//!      `key.kind`) flows through the regular pack field access path
//!      — the C dispatcher (`taida_addon_val_to_raw` PACK case in
//!      `native_runtime.c`) converts the addon-side Pack into a
//!      native runtime pack that behaves identically to a user-
//!      written `@(...)` literal.
//!
//! These tests only verify the **build** side. Phase 4 (`RC2.5-4a`)
//! lands the `run` side that compares interpreter vs. native output
//! byte-for-byte; until then we rely on the existing interpreter
//! tests (`rc2_terminal_surface.rs`) for the runtime contract.

#![cfg(feature = "native")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

// ── Helpers ─────────────────────────────────────────────────

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

/// The canonical `native/addon.toml` for the `taida-lang/terminal`
/// package. Kept inline so this test never depends on the sibling
/// `terminal` repo being checked out (Phase 2 only needs the metadata).
const TERMINAL_ADDON_TOML: &str = r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_lang_terminal"

[functions]
terminalSize = 0
readKey = 0
"#;

/// The v1 `taida/terminal.td` facade shape, inlined so the Phase 2
/// tests do not depend on the sibling `terminal` repo. Mirrors
/// `terminal/taida/terminal.td` exactly for the alias + pack binding
/// surface that RC2 design lock B-2 pins.
const TERMINAL_FACADE_TD: &str = r#"KeyKind <= @(
  Char       <= 0
  Enter      <= 1
  Escape     <= 2
  Tab        <= 3
  Backspace  <= 4
  Delete     <= 5
  ArrowUp    <= 6
  ArrowDown  <= 7
  ArrowLeft  <= 8
  ArrowRight <= 9
  Home       <= 10
  End        <= 11
  PageUp     <= 12
  PageDown   <= 13
  Insert     <= 14
  F1         <= 15
  F2         <= 16
  F3         <= 17
  F4         <= 18
  F5         <= 19
  F6         <= 20
  F7         <= 21
  F8         <= 22
  F9         <= 23
  F10        <= 24
  F11        <= 25
  F12        <= 26
  Unknown    <= 27
)

TerminalSize <= terminalSize
ReadKey      <= readKey

<<< @(TerminalSize, ReadKey, KeyKind)
"#;

/// Lay down a minimal `.taida/deps/taida-lang/terminal/` directory
/// with the canonical `addon.toml`, the `taida/terminal.td` facade,
/// and a zero-byte placeholder cdylib. The placeholder is enough for
/// `resolve_cdylib_path` to succeed at build time; the Phase 2 tests
/// only exercise the compile path, not the `dlopen` runtime.
fn write_terminal_fixture_with_facade(project: &Path) {
    // project anchor for find_project_root.
    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"rc2_5-phase2-test\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    let pkg = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal");
    std::fs::create_dir_all(pkg.join("native")).expect("create native dir");
    std::fs::create_dir_all(pkg.join("taida")).expect("create taida dir");

    std::fs::write(pkg.join("native").join("addon.toml"), TERMINAL_ADDON_TOML)
        .expect("write addon.toml");
    std::fs::write(pkg.join("taida").join("terminal.td"), TERMINAL_FACADE_TD)
        .expect("write facade");

    // Zero-byte placeholder cdylib. `resolve_cdylib_path` only checks
    // that the file exists at build time; it never tries to `dlopen`
    // it during compilation.
    let cdylib_stem = "libtaida_lang_terminal";
    let suffix = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    std::fs::write(
        pkg.join("native")
            .join(format!("{}.{}", cdylib_stem, suffix)),
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

// ── RC2.5-2a: MoldInst addon sentinel dispatch ─────────────

/// RC2.5-2a, lowercase path: `terminalSize[]()` written in mold-
/// instantiation form must route through the addon dispatcher even
/// though the same function also works as a plain `terminalSize()`
/// call. This exercises the MoldInst → `emit_addon_call` branch
/// added in Phase 2.
#[test]
fn mold_inst_lowercase_addon_fn_compiles_on_cranelift_native() {
    let project = unique_temp_dir("rc2_5_phase2_moldinst_lower");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_fixture_with_facade(&project);

    let main_td = r#">>> taida-lang/terminal => @(terminalSize)
size <= terminalSize[]()
stdout(`rc2_5_phase2: mold lowercase cols=${size.cols} rows=${size.rows}`)
"#;

    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-2a: `terminalSize[]()` must compile on Cranelift native. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(
        project.join("main.bin").exists(),
        "RC2.5-2a: native build must produce an executable. stdout={}, stderr={}",
        stdout,
        stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5-2a, facade uppercase path: `TerminalSize[]()` must resolve
/// through the facade alias `TerminalSize <= terminalSize`. This is
/// the full RC2 user-facing surface.
#[test]
fn mold_inst_facade_uppercase_alias_compiles_on_cranelift_native() {
    let project = unique_temp_dir("rc2_5_phase2_moldinst_upper");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_fixture_with_facade(&project);

    let main_td = r#">>> taida-lang/terminal => @(TerminalSize)
size <= TerminalSize[]()
stdout(`rc2_5_phase2: mold uppercase cols=${size.cols} rows=${size.rows}`)
"#;

    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-2a: `TerminalSize[]()` must compile on Cranelift native via facade alias. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(
        !stderr.contains("Symbol 'TerminalSize' not found"),
        "facade alias lookup must succeed, got: {}",
        stderr
    );
    assert!(
        project.join("main.bin").exists(),
        "native build must produce an executable"
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5-2a, facade alias ReadKey: pins the other half of the v1
/// surface. Separate from the TerminalSize test so a regression in
/// either alias is caught independently.
#[test]
fn mold_inst_facade_read_key_alias_compiles_on_cranelift_native() {
    let project = unique_temp_dir("rc2_5_phase2_moldinst_readkey");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_fixture_with_facade(&project);

    let main_td = r#">>> taida-lang/terminal => @(ReadKey)
key <= ReadKey[]()
stdout(`rc2_5_phase2: readKey kind=${key.kind}`)
"#;

    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-2a: `ReadKey[]()` must compile on Cranelift native via facade alias. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(
        project.join("main.bin").exists(),
        "native build must produce an executable"
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2.5-2a: facade pack bindings (pure-Taida values) ───────

/// RC2.5-2a, facade pack binding: `KeyKind <= @(Char <= 0, ...)` is
/// not an addon function — it is a pure-Taida pack synthesised by
/// the facade. The Cranelift native path must lift the assignment
/// into `_taida_main` so user code can read `KeyKind.Char` without
/// ever calling `dlopen`.
#[test]
fn facade_keykind_pack_binds_in_main_on_cranelift_native() {
    let project = unique_temp_dir("rc2_5_phase2_keykind");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_fixture_with_facade(&project);

    let main_td = r#">>> taida-lang/terminal => @(KeyKind)
stdout(`rc2_5_phase2: kk.Char=${KeyKind.Char} kk.F12=${KeyKind.F12}`)
"#;

    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-2a: facade pack binding `KeyKind` must compile on Cranelift native. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(
        !stderr.contains("Symbol 'KeyKind' not found"),
        "facade pack binding lookup must succeed, got: {}",
        stderr
    );
    assert!(
        project.join("main.bin").exists(),
        "native build must produce an executable"
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5-2a, full facade surface: one import statement that pulls in
/// all three symbols (`TerminalSize`, `ReadKey`, `KeyKind`), mirroring
/// the canonical RC2 user-facing import line.
#[test]
fn facade_full_surface_compiles_on_cranelift_native() {
    let project = unique_temp_dir("rc2_5_phase2_all_three");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_fixture_with_facade(&project);

    // Guarded `KeyKind` access plus a reference to the addon aliases
    // (without calling them — calling would reach dlopen and fail on
    // the placeholder cdylib). Build-side only.
    let main_td = r#">>> taida-lang/terminal => @(TerminalSize, ReadKey, KeyKind)
stdout(`rc2_5_phase2: surface ok kind=${KeyKind.Char}`)
"#;

    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-2a: full facade surface must compile on Cranelift native. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(
        !stderr.contains("Symbol 'TerminalSize' not found"),
        "TerminalSize must be reachable through the facade, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("Symbol 'ReadKey' not found"),
        "ReadKey must be reachable through the facade, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("Symbol 'KeyKind' not found"),
        "KeyKind must be reachable through the facade, got: {}",
        stderr
    );
    assert!(project.join("main.bin").exists());

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2.5-2b: return value pack field access ───────────────

/// RC2.5-2b: `.cols` / `.rows` access on the addon return value goes
/// through the normal pack field path. The Cranelift native path
/// must register `cols`, `rows` as field names so `size.cols` resolves
/// correctly even though the addon return value is constructed by
/// the C dispatcher, not by a user-written `@(...)` literal.
///
/// This test only asserts build-time success (type checker + jsonEncode
/// registration must see the field names). The runtime byte-identical
/// check against the interpreter lives in Phase 4 (`RC2.5-4b`).
#[test]
fn addon_return_pack_field_access_compiles_on_cranelift_native() {
    let project = unique_temp_dir("rc2_5_phase2_pack_access");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_terminal_fixture_with_facade(&project);

    // Touches every field of both return shapes in a single program.
    let main_td = r#">>> taida-lang/terminal => @(TerminalSize, ReadKey)
size <= TerminalSize[]()
stdout(`size.cols=${size.cols} size.rows=${size.rows}`)
key <= ReadKey[]()
stdout(`key.kind=${key.kind} key.text=${key.text} key.ctrl=${key.ctrl} key.alt=${key.alt} key.shift=${key.shift}`)
"#;

    let (ok, stdout, stderr) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-2b: addon return pack field access must compile on Cranelift native. \
         stdout={}, stderr={}",
        stdout, stderr
    );
    assert!(project.join("main.bin").exists());

    let _ = std::fs::remove_dir_all(&project);
}
