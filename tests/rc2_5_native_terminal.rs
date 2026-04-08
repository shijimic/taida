//! RC2.5 Phase 4 -- Integration, parity, and gate tests.
//!
//! Phases 1-3 pinned each step of the RC2.5 pipeline in isolation:
//!
//!   - Phase 1 (`tests/rc2_terminal_phase1_smoke.rs`): lower.rs reject
//!     removal + dlopen init emit. Placeholder cdylib only.
//!   - Phase 2 (`tests/rc2_5_native_terminal_phase2.rs`): MoldInst
//!     uppercase alias dispatch + facade pack bindings. Build-time only.
//!   - Phase 3 (`tests/rc2_5_native_terminal_phase3.rs`): Status::Error
//!     → catchable AddonError variant + dlopen hard-fail semantics.
//!     First runtime test with a real cdylib.
//!
//! Phase 4 (this file) does the final three things RC2.5 promised:
//!
//!   1. `RC2.5-4a` — full v1 surface end-to-end: a single Taida program
//!      that imports **three** symbols (`termIsTty` for the happy path,
//!      `termPrintLn` for a Str argument round-trip, `KeyKind` for the
//!      pure-Taida facade pack binding) must compile with
//!      `taida build --target native` **and** execute, producing all
//!      expected stdout markers.
//!
//!   2. `RC2.5-4b` — interpreter ↔ native parity: the *exact same*
//!      `main.td` + fixture, when run through both the interpreter
//!      (`taida main.td`) and the native binary (`taida build --target
//!      native && ./main.bin`), must emit byte-for-byte identical
//!      stdout. This is the strongest backend-parity guarantee the
//!      RC2.5 gate can make.
//!
//!   3. `RC2.5B-003` — Rust ↔ C ABI struct layout parity. The C
//!      `_Static_assert`s in `native_runtime.c` lock the C-side sizes
//!      at compile time (Phase 1). Here we assert that Rust's
//!      `std::mem::size_of::<TaidaAddon...V1>()` agrees with those same
//!      literal numbers. Together the two checks form a bidirectional
//!      drift detector: if either the Rust `#[repr(C)]` definition in
//!      `crates/addon-rs/src/abi.rs` or the C mirror in
//!      `native_runtime.c` changes without the other, one of the two
//!      sides fails.
//!
//!   4. `RC2.5B-004` — documented known constraint. Phase 3 already
//!      pins the spec hard-fail format; Phase 4 additionally pins the
//!      new `taida: hint: cdylib path was resolved at build time ...`
//!      diagnostic line so developers who move a `.so` after `taida
//!      build` get immediate, actionable feedback.
//!
//! Every test uses a unique project directory so parallel Cargo test
//! workers never collide on the process-wide `taida_addon_registry[]`
//! inside a compiled native binary. The real workspace-built sample
//! cdylib (`libtaida_addon_terminal_sample.so/.dylib/.dll`) is reused
//! from `target/{debug,release}`.

#![cfg(feature = "native")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ── Helpers ─────────────────────────────────────────────────

fn taida_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_taida"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

fn cdylib_ext() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    }
}

fn cdylib_filename(stem: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{}.dll", stem)
    } else if cfg!(target_os = "macos") {
        format!("lib{}.dylib", stem)
    } else {
        format!("lib{}.so", stem)
    }
}

/// Locate the workspace-built sample addon cdylib. Mirrors the
/// helper in `tests/rc2_5_native_terminal_phase3.rs` so the two test
/// files stay in lockstep on fixture discovery.
fn find_sample_terminal_cdylib() -> Option<PathBuf> {
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir().join("target"));
    let lib_name = cdylib_filename("taida_addon_terminal_sample");
    let candidates = [
        target_root.join("debug").join(&lib_name),
        target_root.join("release").join(&lib_name),
        target_root.join("debug").join("deps").join(&lib_name),
        target_root.join("release").join("deps").join(&lib_name),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// addon.toml describing every function the sample addon registers.
/// Phase 4 calls `termIsTty` and `termPrintLn`; the other entries are
/// pinned so the import resolver never rejects a cross-reference.
const SAMPLE_ADDON_TOML: &str = r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_lang_terminal"

[functions]
termPrint = 1
termPrintLn = 1
termReadLine = 0
termSize = 0
termIsTty = 0
"#;

/// The v1 facade exposes `KeyKind` as a pure-Taida pack binding so
/// the user can write `KeyKind.Char` without any addon call. Phase 4
/// uses this to prove the facade pack binding path flows through the
/// Cranelift `_taida_main` 3rd-pass replay (Phase 2 RC2.5-2a) *and*
/// that it matches interpreter output byte-for-byte (RC2.5-4b).
const SAMPLE_FACADE_TD: &str = r#"KeyKind <= @(
  Char      <= 0
  Enter     <= 1
  Escape    <= 2
  Tab       <= 3
  Backspace <= 4
)
"#;

/// Lay down a self-contained project:
///
///   - `packages.tdm` (marker so `find_project_root` anchors)
///   - `.taida/deps/<pkg_dir>/native/addon.toml`
///   - `.taida/deps/<pkg_dir>/native/lib<stem>.<ext>` (real sample cdylib)
///   - `.taida/deps/<pkg_dir>/taida/<stem>.td` (facade with KeyKind)
///   - `.taida/deps/<pkg_dir>/packages.tdm` (addon package marker)
///
/// Returns the absolute path of the copied cdylib so tests that need
/// to delete it (RC2.5B-004 close) can do so without re-walking the
/// layout.
fn write_sample_fixture(project: &Path, pkg_dir_rel: &str) -> PathBuf {
    let cdylib = match find_sample_terminal_cdylib() {
        Some(p) => p,
        None => panic!(
            "RC2.5 Phase 4 tests require the workspace-built \
             `libtaida_addon_terminal_sample.{}` — run \
             `cargo build -p taida-addon-terminal-sample` first",
            cdylib_ext()
        ),
    };

    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"rc2_5-phase4-test\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    let pkg = project.join(".taida").join("deps").join(pkg_dir_rel);
    std::fs::create_dir_all(pkg.join("native")).expect("create native dir");
    std::fs::create_dir_all(pkg.join("taida")).expect("create taida dir");

    std::fs::write(pkg.join("native").join("addon.toml"), SAMPLE_ADDON_TOML)
        .expect("write addon.toml");
    // The facade stem must match the last path segment of the package
    // directory (`<pkg_dir>/taida/<stem>.td`).
    std::fs::write(pkg.join("taida").join("terminal.td"), SAMPLE_FACADE_TD).expect("write facade");
    std::fs::write(
        pkg.join("packages.tdm"),
        "name <= \"taida-lang/terminal\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write pkg packages.tdm");

    let dest_name = cdylib_filename("taida_lang_terminal");
    let dest = pkg.join("native").join(&dest_name);
    std::fs::copy(&cdylib, &dest).expect("copy sample cdylib into fixture");

    dest
}

/// Compile a `.td` via `taida build --target native`. Returns
/// `(ok, stdout, stderr, bin_path)` so callers can assert both the
/// build result *and* the output.
fn build_native(project: &Path, main_td: &str) -> (bool, String, String, PathBuf) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let bin_path = project.join("main.bin");
    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(&bin_path)
        .current_dir(project)
        .output()
        .expect("taida binary must run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        bin_path,
    )
}

/// Run a compiled native binary with stdin attached to /dev/null.
/// Returns `(exit_code, stdout, stderr)`.
fn run_native_with_null_stdin(bin: &Path, project: &Path) -> (Option<i32>, String, String) {
    let output = Command::new(bin)
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("compiled binary must launch");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Run `main.td` through the interpreter (`taida main.td`) with stdin
/// redirected to /dev/null. Returns `(ok, stdout, stderr)`.
fn run_interpreter_with_null_stdin(project: &Path, main_td: &str) -> (bool, String, String) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("taida binary must run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

// ── RC2.5B-003: ABI struct layout parity ───────────────────────────

/// RC2.5B-003 close. The C side of `native_runtime.c` already locks
/// every v1 ABI struct size with `_Static_assert(sizeof(...) == N,
/// "layout drift")` at build time (Phase 1 done). What this test adds
/// is the opposite direction: Rust's `#[repr(C)]` definitions in
/// `crates/addon-rs/src/abi.rs` must produce the *same* `size_of`
/// values. If anyone changes one side without updating the other,
/// exactly one of the two checks fails and the mismatch is diagnosed
/// immediately.
///
/// The expected numbers are the same literals encoded in the C
/// `_Static_assert`s (see `native_runtime.c` search for
/// `layout drift`). LP64 Linux / macOS only — Windows has its own
/// ABI and is RC3+ scope per RC2.5B-005.
#[test]
fn abi_struct_layout_parity_matches_c_static_assert_sizes() {
    use std::mem::size_of;
    use taida_addon::{
        TaidaAddonBytesPayload, TaidaAddonDescriptorV1, TaidaAddonErrorV1, TaidaAddonFloatPayload,
        TaidaAddonFunctionV1, TaidaAddonIntPayload, TaidaAddonPackEntryV1, TaidaAddonPackPayload,
        TaidaAddonValueV1, TaidaHostV1,
    };

    // These numbers must match exactly the `_Static_assert(sizeof(...)
    // == N, ...)` lines in `native_runtime.c`. If you change one,
    // change the other in the same commit.
    assert_eq!(
        size_of::<TaidaAddonValueV1>(),
        16,
        "TaidaAddonValueV1 layout drift (Rust vs expected C sizeof)"
    );
    assert_eq!(
        size_of::<TaidaAddonErrorV1>(),
        16,
        "TaidaAddonErrorV1 layout drift (Rust vs expected C sizeof)"
    );
    assert_eq!(
        size_of::<TaidaAddonIntPayload>(),
        8,
        "TaidaAddonIntPayload layout drift"
    );
    assert_eq!(
        size_of::<TaidaAddonFloatPayload>(),
        8,
        "TaidaAddonFloatPayload layout drift"
    );
    assert_eq!(
        size_of::<TaidaAddonBytesPayload>(),
        16,
        "TaidaAddonBytesPayload layout drift"
    );
    assert_eq!(
        size_of::<TaidaAddonFunctionV1>(),
        24,
        "TaidaAddonFunctionV1 layout drift"
    );
    assert_eq!(
        size_of::<TaidaAddonDescriptorV1>(),
        40,
        "TaidaAddonDescriptorV1 layout drift"
    );
    assert_eq!(size_of::<TaidaHostV1>(), 96, "TaidaHostV1 layout drift");
    assert_eq!(
        size_of::<TaidaAddonPackEntryV1>(),
        16,
        "TaidaAddonPackEntryV1 layout drift"
    );
    assert_eq!(
        size_of::<TaidaAddonPackPayload>(),
        16,
        "TaidaAddonPackPayload layout drift"
    );
}

// ── RC2.5-4a: full v1 surface integration ─────────────────────────

/// The flagship Phase 4 test. A single `main.td` imports three
/// symbols from `taida-lang/terminal`:
///
///   - `termIsTty` (addon function, 0 arity, always Ok → Bool)
///   - `termPrintLn` (addon function, 1 arity, Str → Unit, side effect
///     is a stdout write directly from the addon cdylib via libc)
///   - `KeyKind` (pure-Taida facade pack binding exposed by
///     `terminal.td`)
///
/// This exercises every Cranelift lowering path RC2.5 added:
///
///   - addon lowercase-name dispatch (Phase 1)
///   - facade pack binding replay in `_taida_main` 3rd pass (Phase 2)
///   - `emit_addon_call` IR with Str argument marshaling (Phase 2)
///   - `taida_addon_call` dispatcher end-to-end (Phase 1)
///   - host `value_new_bool` / `value_new_unit` callbacks (Phase 1)
///
/// We assert:
///
///   1. `taida build --target native` succeeds.
///   2. The produced binary exits with code 0.
///   3. Stdout contains the `KeyKind.Char=0` / `KeyKind.Enter=1`
///      markers — proving the facade pack binding reached user code.
///   4. Stdout contains a `termIsTty=` marker — proving the addon
///      call returned normally.
///   5. Stdout contains the exact text emitted via `termPrintLn` —
///      proving the addon can write to stdout from inside the native
///      binary process.
///   6. No hard-fail diagnostic (the `taida: addon load failed:`
///      prefix) appears.
#[test]
fn full_v1_surface_builds_and_runs_on_native() {
    let project = unique_temp_dir("rc2_5_phase4_full_surface");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termIsTty, termPrintLn, KeyKind)
stdout(`phase4: KeyKind.Char=${KeyKind.Char}`)
stdout(`phase4: KeyKind.Enter=${KeyKind.Enter}`)
isTty <= termIsTty()
stdout(`phase4: termIsTty=${isTty}`)
termPrintLn("phase4: hello-from-termPrintLn")
stdout(`phase4: done`)
"#;

    let (ok, build_stdout, build_stderr, bin) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-4a: full-surface native build must succeed. \
         build stdout={} build stderr={}",
        build_stdout, build_stderr
    );
    assert!(bin.exists(), "main.bin must exist after build");

    let (code, run_stdout, run_stderr) = run_native_with_null_stdin(&bin, &project);
    assert_eq!(
        code,
        Some(0),
        "RC2.5-4a: full-surface binary must exit cleanly. \
         stdout={} stderr={}",
        run_stdout,
        run_stderr
    );

    // Facade pack binding (Phase 2 replay) reached user code.
    assert!(
        run_stdout.contains("phase4: KeyKind.Char=0"),
        "RC2.5-4a: KeyKind.Char facade binding must be readable. stdout={}",
        run_stdout
    );
    assert!(
        run_stdout.contains("phase4: KeyKind.Enter=1"),
        "RC2.5-4a: KeyKind.Enter facade binding must be readable. stdout={}",
        run_stdout
    );

    // Addon call round-tripped and produced a Bool value. The
    // actual value depends on whether stdin is a TTY (it isn't under
    // cargo test, so the addon returns Bool(false)), but both true
    // and false are legal — the contract is that the call returned.
    assert!(
        run_stdout.contains("phase4: termIsTty="),
        "RC2.5-4a: termIsTty call must produce a marker line. stdout={}",
        run_stdout
    );

    // The addon wrote directly to stdout via libc. This proves the
    // cdylib is actually loaded and can perform side effects inside
    // the native binary's process.
    assert!(
        run_stdout.contains("phase4: hello-from-termPrintLn"),
        "RC2.5-4a: termPrintLn must write its argument to stdout. stdout={}",
        run_stdout
    );

    assert!(
        run_stdout.contains("phase4: done"),
        "RC2.5-4a: control must reach the post-call stdout marker. stdout={}",
        run_stdout
    );

    // No hard-fail diagnostics from the dispatcher.
    assert!(
        !run_stderr.contains("taida: addon load failed"),
        "RC2.5-4a: happy-path must not emit the dlopen hard-fail prefix. \
         stderr={}",
        run_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2.5-4b: interpreter ↔ native byte-for-byte parity ────────────

/// RC2.5-4b: the same `main.td` against the same fixture must produce
/// the same stdout through both backends. This is the strongest
/// backend-parity guarantee we can make — the interpreter is the
/// reference implementation (per CLAUDE.md), and the native backend
/// now (Phase 1-3) routes every addon call through the same ABI v1
/// descriptors that the interpreter uses, so identical output is the
/// RC2.5 contract.
///
/// We deliberately use only deterministic addon calls here:
///
///   - `KeyKind.Char` / `KeyKind.Enter` (pure facade pack, no I/O)
///   - `termIsTty()` (return value depends on stdin but both runs
///     have stdin pinned to /dev/null so they agree)
///   - `termPrintLn("...")` (the addon's own stdout write happens in
///     both processes, so whatever ordering/semantics one uses, the
///     other uses the same)
///
/// We deliberately do *not* call `termReadLine` here because it's
/// exercised by Phase 3 (`addon_status_error_becomes_catchable_addon_error_variant`)
/// and its error code path already has dedicated cross-backend parity
/// coverage via the interpreter's `addon_eval.rs` error format.
///
/// The assertion is strict byte equality on stdout. A single
/// whitespace or newline difference would fail the test — which is
/// exactly the guarantee a backend-parity gate needs.
#[test]
fn interpreter_and_native_agree_on_terminal_sample_output() {
    let project = unique_temp_dir("rc2_5_phase4_parity");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_sample_fixture(&project, "taida-lang/terminal");

    // The script uses only pack field access + addon calls whose
    // output is stable under /dev/null stdin. Each line is clearly
    // prefixed so a diff-level failure points directly at the
    // divergent marker.
    let main_td = r#">>> taida-lang/terminal => @(termIsTty, termPrintLn, KeyKind)
stdout(`parity: KeyKind.Char=${KeyKind.Char}`)
stdout(`parity: KeyKind.Enter=${KeyKind.Enter}`)
stdout(`parity: KeyKind.Escape=${KeyKind.Escape}`)
stdout(`parity: KeyKind.Tab=${KeyKind.Tab}`)
stdout(`parity: KeyKind.Backspace=${KeyKind.Backspace}`)
isTty <= termIsTty()
stdout(`parity: termIsTty=${isTty}`)
termPrintLn("parity: addon-direct-stdout-line")
stdout(`parity: tail`)
"#;

    // Build the native binary first. The interpreter run reuses
    // the same project directory (and therefore the same addon
    // fixture).
    let (build_ok, build_stdout, build_stderr, bin) = build_native(&project, main_td);
    assert!(
        build_ok,
        "RC2.5-4b: native build must succeed for parity comparison. \
         stdout={} stderr={}",
        build_stdout, build_stderr
    );
    assert!(bin.exists());

    // Now run interpreter (taida main.td) with stdin pinned to
    // /dev/null so it sees the same termIsTty result as the native
    // binary will.
    let (interp_ok, interp_stdout, interp_stderr) =
        run_interpreter_with_null_stdin(&project, main_td);
    assert!(
        interp_ok,
        "RC2.5-4b: interpreter run must succeed. \
         stdout={} stderr={}",
        interp_stdout, interp_stderr
    );

    // Run the native binary with the same stdin source.
    let (native_code, native_stdout, native_stderr) = run_native_with_null_stdin(&bin, &project);
    assert_eq!(
        native_code,
        Some(0),
        "RC2.5-4b: native binary must exit cleanly for parity. \
         stdout={} stderr={}",
        native_stdout,
        native_stderr
    );

    // Strict byte equality. If you see a diff here, the backend
    // parity contract has been broken — look for lowering changes
    // in `src/codegen/lower.rs` addon path or C dispatcher changes
    // in `native_runtime.c` RC2.5 block.
    assert_eq!(
        interp_stdout, native_stdout,
        "RC2.5-4b: interpreter vs native stdout must be byte-for-byte \
         identical for the terminal sample.\n\
         interpreter stdout:\n{}\n\
         ---\n\
         native stdout:\n{}\n",
        interp_stdout, native_stdout
    );

    // Sanity: both sides must actually have produced the expected
    // markers. If stdout is empty on both sides the above assertion
    // trivially passes, which would hide a real regression.
    assert!(
        interp_stdout.contains("parity: KeyKind.Char=0"),
        "RC2.5-4b: parity comparison cannot succeed on empty output. \
         interp stdout={}",
        interp_stdout
    );
    assert!(
        interp_stdout.contains("parity: tail"),
        "RC2.5-4b: interp run must reach the final marker. stdout={}",
        interp_stdout
    );
    assert!(
        interp_stdout.contains("parity: addon-direct-stdout-line"),
        "RC2.5-4b: interp addon-direct stdout line must be present. \
         stdout={}",
        interp_stdout
    );
    assert!(
        !interp_stderr.contains("taida: addon load failed"),
        "RC2.5-4b: interpreter must not emit dlopen hard-fail. \
         stderr={}",
        interp_stderr
    );
    assert!(
        !native_stderr.contains("taida: addon load failed"),
        "RC2.5-4b: native binary must not emit dlopen hard-fail on the \
         happy path. stderr={}",
        native_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2.5B-004: documented known constraint diagnostic ────────────

/// RC2.5B-004 close. Phase 3 already pinned the spec-mandated
/// `taida: addon load failed: <pkg>: <detail>` hard-fail prefix when
/// the cdylib is deleted after build. Phase 4 adds the second
/// diagnostic line that tells developers *why* the path stopped
/// resolving — the Frozen Phase 0 decision was to embed an absolute
/// path resolved at build time, and RC2.5 v1 does not do a runtime
/// rescan. The hint line is additive so Phase 3 tests (which only
/// assert the first line prefix) continue to pass.
///
/// This test is the executable documentation of RC2.5B-004: if a
/// future maintainer removes the hint, this test fails and forces
/// them to update `.dev/RC2_5_BLOCKERS.md::RC2.5B-004` before merging.
#[test]
fn cdylib_moved_after_build_emits_build_time_hint_line() {
    let project = unique_temp_dir("rc2_5_phase4_build_time_hint");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let cdylib_path = write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termIsTty)
result <= termIsTty()
stdout(`should-not-print=${result}`)
"#;

    let (ok, _stdout, _stderr, bin) = build_native(&project, main_td);
    assert!(ok, "RC2.5B-004: native build must succeed");
    assert!(bin.exists());

    // Break the runtime resolution by deleting the build-time
    // resolved cdylib absolute path.
    std::fs::remove_file(&cdylib_path).expect("delete cdylib so dlopen will fail");

    let (code, run_stdout, run_stderr) = run_native_with_null_stdin(&bin, &project);
    assert_eq!(
        code,
        Some(1),
        "RC2.5B-004: missing cdylib must hard-fail with exit 1. \
         stdout={} stderr={}",
        run_stdout,
        run_stderr
    );
    // Phase 3 contract: the first line prefix is unchanged.
    assert!(
        run_stderr.contains("taida: addon load failed:"),
        "RC2.5B-004: Phase 3 prefix must still be present. stderr={}",
        run_stderr
    );
    // Phase 4 addition: the hint line identifies this as the known
    // "cdylib path was resolved at build time" constraint.
    assert!(
        run_stderr.contains("taida: hint:"),
        "RC2.5B-004: Phase 4 hint line must be present. stderr={}",
        run_stderr
    );
    assert!(
        run_stderr.contains("cdylib path was resolved at build time"),
        "RC2.5B-004: hint must mention the build-time resolution. \
         stderr={}",
        run_stderr
    );
    assert!(
        run_stderr.contains("RC2.5B-004"),
        "RC2.5B-004: hint must cross-reference the blocker id so \
         developers can find the documented constraint. stderr={}",
        run_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}
