//! RC2B-207 / RC2B-208 positive-path tests.
//!
//! The Phase 1 smoke tests in `rc2_terminal_phase1_smoke.rs` only lock
//! the **rejection** side of the terminal addon contract (missing
//! install → module not found, JS/native/wasm-min targets → compile
//! error). This file adds the **positive** side: once the cdylib and
//! the Taida-side facade are actually installed under
//! `.taida/deps/taida-lang/terminal/`, the v1 user surface
//! (`TerminalSize`, `ReadKey`, `KeyKind`) must be reachable from a
//! regular `.td` program running through the interpreter.
//!
//! RC2B-207 (Must Fix): the addon-import path only bound symbols
//! listed in `addon.toml::[functions]`, which are `terminalSize` /
//! `readKey` (lowercase). The uppercase v1 surface symbols were
//! unreachable. Fixed by loading `taida/terminal.td` as a facade after
//! the addon registers, and binding facade exports in preference to
//! the raw function table.
//!
//! RC2B-208 (Must Fix): the review raised that
//! `taida build native` rejects addon-backed packages, yet
//! the README and example told users to run exactly that. The RC2
//! design calls the interpreter binary the "Native backend", so the
//! README / example / design are now aligned to the interpreter run
//! path and the Cranelift reject is reclassified as an explicit,
//! documented out-of-scope limitation. The regression guard for the
//! deterministic Cranelift reject lives in
//! `rc2_terminal_phase1_smoke.rs`.
//!
//! These tests require the sibling `terminal` repository to be checked
//! out at `../terminal` with `cargo build` already run (either debug
//! or release), because they copy the actual cdylib into a temporary
//! project's `.taida/deps/` tree. When the sibling repo is not present
//! the tests soft-skip with an explanatory log line, matching the
//! pattern in `rc2_terminal_phase1_smoke.rs`.

#![cfg(feature = "native")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

/// Locate the sibling `terminal` repository (same layout rule as
/// `rc2_terminal_phase1_smoke.rs`).
fn locate_terminal_repo() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest_dir.parent()?.join("terminal");
    if candidate.join("native").join("addon.toml").exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Find an already-built `libtaida_lang_terminal.so` (Linux) /
/// `.dylib` (macOS) / `.dll` (Windows) in the sibling repo's target
/// directory. Returns `None` if no build has been produced yet.
fn locate_terminal_cdylib(repo: &Path) -> Option<PathBuf> {
    let filename = if cfg!(target_os = "linux") {
        "libtaida_lang_terminal.so"
    } else if cfg!(target_os = "macos") {
        "libtaida_lang_terminal.dylib"
    } else if cfg!(target_os = "windows") {
        "taida_lang_terminal.dll"
    } else {
        "libtaida_lang_terminal.so"
    };
    for profile in ["release", "debug"] {
        let candidate = repo.join("target").join(profile).join(filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

/// Install the terminal package into `<project>/.taida/deps/taida-lang/terminal/`
/// by copying the canonical `native/addon.toml`, the built cdylib, and
/// the `taida/terminal.td` facade from the sibling repository. Returns
/// `false` (with a `note:` log line) when the fixture prerequisites
/// are not present so the test can soft-skip.
fn install_terminal_fixture(project: &Path) -> bool {
    let Some(repo) = locate_terminal_repo() else {
        eprintln!(
            "note: skipping RC2B-207/208 positive-path test -- \
             sibling 'terminal' repo not found"
        );
        return false;
    };
    let Some(cdylib) = locate_terminal_cdylib(&repo) else {
        eprintln!(
            "note: skipping RC2B-207/208 positive-path test -- \
             sibling 'terminal' repo has no cdylib build under target/{{debug,release}}; \
             run `cargo build --release` inside {}",
            repo.display()
        );
        return false;
    };
    let facade_src = repo.join("taida").join("terminal.td");
    if !facade_src.exists() {
        eprintln!(
            "note: skipping RC2B-207/208 positive-path test -- \
             sibling repo is missing taida/terminal.td facade"
        );
        return false;
    }

    let pkg = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal");
    std::fs::create_dir_all(pkg.join("native")).expect("create native dir");
    std::fs::create_dir_all(pkg.join("taida")).expect("create taida dir");

    std::fs::copy(
        repo.join("native").join("addon.toml"),
        pkg.join("native").join("addon.toml"),
    )
    .expect("copy addon.toml");
    std::fs::copy(
        &cdylib,
        pkg.join("native")
            .join(cdylib.file_name().expect("cdylib file name")),
    )
    .expect("copy cdylib");
    std::fs::copy(facade_src, pkg.join("taida").join("terminal.td")).expect("copy facade");
    std::fs::write(
        pkg.join("packages.tdm"),
        "name <= \"taida-lang/terminal\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write packages.tdm");

    // Marker file so find_project_root anchors at `project`.
    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"rc2-surface-test\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    true
}

/// Run a `.td` program through the installed interpreter binary and
/// return `(success, combined_output)`.
fn run_td(project: &Path, main_td: &str) -> (bool, String) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .current_dir(project)
        .output()
        .expect("taida binary must run");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output.status.success(), format!("{}{}", stderr, stdout))
}

// ── RC2B-207 positive: KeyKind is reachable ──────────────────

/// The v1 facade exports `KeyKind` as a pure-Taida pack. User code
/// that imports only `KeyKind` (no addon call at all) must see every
/// variant with the canonical integer tag pinned in `RC2_DESIGN.md`
/// Section B-2. This locks the facade's `KeyKind` shape independent
/// of any cdylib call, so the test survives even in environments
/// where `isatty` is not stable (e.g. CI sandboxes).
#[test]
fn terminal_import_surface_exposes_keykind_pack() {
    let project = unique_temp_dir("rc2b207_keykind");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    if !install_terminal_fixture(&project) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(KeyKind)
stdout(`Char=${KeyKind.Char}`)
stdout(`Enter=${KeyKind.Enter}`)
stdout(`ArrowUp=${KeyKind.ArrowUp}`)
stdout(`F12=${KeyKind.F12}`)
stdout(`Unknown=${KeyKind.Unknown}`)
"#;

    let (ok, combined) = run_td(&project, main_td);
    assert!(ok, "KeyKind import must succeed, got: {}", combined);
    assert!(combined.contains("Char=0"), "Char tag: {}", combined);
    assert!(combined.contains("Enter=1"), "Enter tag: {}", combined);
    assert!(combined.contains("ArrowUp=6"), "ArrowUp tag: {}", combined);
    assert!(combined.contains("F12=26"), "F12 tag: {}", combined);
    assert!(combined.contains("Unknown=27"), "Unknown tag: {}", combined);

    let _ = std::fs::remove_dir_all(&project);
}

/// Importing all three v1 surface symbols in a single statement must
/// succeed. `KeyKind` resolves to the facade pack, `TerminalSize` and
/// `ReadKey` resolve to the aliased addon sentinels. The actual mold
/// call is attempted but falls into the deterministic non-TTY error
/// path under `cargo test` — we only assert that the import resolves
/// (no "Symbol not found" error) and that `KeyKind` is readable.
#[test]
fn terminal_import_exposes_all_three_v1_symbols() {
    let project = unique_temp_dir("rc2b207_all_three");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    if !install_terminal_fixture(&project) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    // Touch KeyKind inside a truthy guard and early-exit before the
    // actual `TerminalSize[]()` / `ReadKey[]()` call. We only need to
    // prove the import resolves all three names into the scope — any
    // call that reaches the addon hits the non-TTY path and becomes
    // noise in CI.
    let main_td = r#">>> taida-lang/terminal => @(TerminalSize, ReadKey, KeyKind)
stdout(`surface-ok kind=${KeyKind.Char}`)
"#;

    let (ok, combined) = run_td(&project, main_td);
    assert!(
        ok,
        "import of @(TerminalSize, ReadKey, KeyKind) must resolve, got: {}",
        combined
    );
    assert!(
        combined.contains("surface-ok kind=0"),
        "facade must bind KeyKind, got: {}",
        combined
    );
    assert!(
        !combined.contains("Symbol 'TerminalSize' not found"),
        "RC2B-207 regression: TerminalSize must be reachable, got: {}",
        combined
    );
    assert!(
        !combined.contains("Symbol 'ReadKey' not found"),
        "RC2B-207 regression: ReadKey must be reachable, got: {}",
        combined
    );
    assert!(
        !combined.contains("Symbol 'KeyKind' not found"),
        "RC2B-207 regression: KeyKind must be reachable, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// `TerminalSize[]()` must route through the addon dispatch path and
/// surface the deterministic non-TTY error under `cargo test` (stdout
/// is a pipe, not a TTY). The error text is wire-pinned by
/// `terminal/src/size.rs::TerminalSizeNotATty` (code 2001).
///
/// Before RC2B-207 this call failed with "Symbol 'TerminalSize' not
/// found in addon-backed package" because the uppercase facade name
/// was never bound. After the fix, the symbol resolves, the mold
/// dispatch bridge forwards to the lowercase Rust function, and the
/// expected wire-frozen error surfaces.
#[test]
fn terminal_size_dispatches_through_facade_to_addon() {
    let project = unique_temp_dir("rc2b207_dispatch_size");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    if !install_terminal_fixture(&project) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(TerminalSize)
size <= TerminalSize[]()
stdout(`cols=${size.cols} rows=${size.rows}`)
"#;

    let (ok, combined) = run_td(&project, main_td);
    assert!(
        !ok,
        "TerminalSize[]() must hit the non-TTY error under cargo test, got: {}",
        combined
    );
    assert!(
        !combined.contains("Symbol 'TerminalSize' not found"),
        "RC2B-207 regression: facade bind missing, got: {}",
        combined
    );
    assert!(
        combined.contains("TerminalSizeNotATty") || combined.contains("code=2001"),
        "dispatch must reach the wire-frozen non-TTY error, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// Companion test for `ReadKey[]()`. Same contract as the
/// `TerminalSize` dispatch test, but pinned to the `ReadKeyNotATty`
/// error (code 1001) in `terminal/src/key.rs`.
#[test]
fn read_key_dispatches_through_facade_to_addon() {
    let project = unique_temp_dir("rc2b207_dispatch_readkey");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    if !install_terminal_fixture(&project) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(ReadKey)
key <= ReadKey[]()
stdout(`kind=${key.kind}`)
"#;

    let (ok, combined) = run_td(&project, main_td);
    assert!(
        !ok,
        "ReadKey[]() must hit the non-TTY error under cargo test, got: {}",
        combined
    );
    assert!(
        !combined.contains("Symbol 'ReadKey' not found"),
        "RC2B-207 regression: facade bind missing, got: {}",
        combined
    );
    assert!(
        combined.contains("ReadKeyNotATty") || combined.contains("code=1001"),
        "dispatch must reach the wire-frozen non-TTY error, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// Lowercase backward compatibility: addon-backed packages that do
/// not ship a facade still expose their `[functions]` table directly
/// under the original snake/camelCase names. The RC1.5 proof-of-
/// concept (`crates/addon-terminal-sample`) relies on this path, so
/// the RC2B-207 fix must not break it.
///
/// We exercise this by importing the lowercase `terminalSize` from
/// the same terminal package. The facade does not export lowercase
/// names in its `<<<`, so the lookup falls through to the manifest
/// `[functions]` fallback and still succeeds.
#[test]
fn terminal_lowercase_function_import_still_works() {
    let project = unique_temp_dir("rc2b207_lowercase_compat");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    if !install_terminal_fixture(&project) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(terminalSize)
size <= terminalSize()
stdout(`cols=${size.cols} rows=${size.rows}`)
"#;

    let (ok, combined) = run_td(&project, main_td);
    assert!(
        !ok,
        "terminalSize() must still hit the non-TTY error under cargo test, got: {}",
        combined
    );
    assert!(
        !combined.contains("Symbol 'terminalSize' not found"),
        "lowercase manifest fallback must resolve, got: {}",
        combined
    );
    assert!(
        combined.contains("TerminalSizeNotATty") || combined.contains("code=2001"),
        "lowercase dispatch must still reach the wire error, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2B-208 lock: Cranelift native target is still rejected ─

/// RC2.5 Phase 1: the Cranelift AOT native backend **accepts**
/// addon-backed package imports. Previously (RC2B-208, RC2 scope) the
/// lowering layer rejected `taida build native` for any
/// package with `native/addon.toml`; that reject has been removed now
/// that `taida_addon_call` exists in the native runtime.
///
/// This test pins the positive path: a minimal import-only program
/// that mentions `taida-lang/terminal` must build cleanly through the
/// Cranelift native pipeline. Actual call-site dispatch (invoking
/// `TerminalSize[]()` and unpacking the result) is exercised by the
/// Phase 2 integration tests; here we only verify that the lowering
/// layer no longer rejects the import.
#[test]
fn cranelift_native_target_accepts_addon_backed_package_import() {
    let project = unique_temp_dir("rc2_5_cranelift_accept");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    if !install_terminal_fixture(&project) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    // Import-only program. The addon function is referenced via
    // the import statement so `lower_addon_import` runs, but no
    // call site is emitted so we do not need the facade sentinel
    // plumbing that Phase 2 introduces.
    let main_td = r#">>> taida-lang/terminal => @(terminalSize)
stdout("rc2_5: terminal import accepted by cranelift native backend")
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.bin"))
        .current_dir(&project)
        .output()
        .expect("taida binary must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stderr, stdout);

    assert!(
        output.status.success(),
        "RC2.5 contract: Cranelift native target must now accept \
         addon-backed package imports. stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        !combined.contains("Cranelift native backend in RC1"),
        "RC2.5 contract: the RC1 reject message must no longer fire. got: {}",
        combined
    );
    assert!(
        !combined.contains("interpreter dispatch only"),
        "RC2.5 contract: the 'interpreter dispatch only' message must no longer fire. got: {}",
        combined
    );
    assert!(
        project.join("main.bin").exists(),
        "RC2.5 contract: native build must produce an executable binary. combined output: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}
