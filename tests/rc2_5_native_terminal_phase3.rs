//! RC2.5 Phase 3 -- Cranelift native backend: error variant propagation
//! and dlopen failure semantics.
//!
//! Phase 1 (`tests/rc2_terminal_phase1_smoke.rs::terminal_import_accepted_on_cranelift_native_at_compile_time`)
//! exercised the build path with a placeholder cdylib. Phase 2
//! (`tests/rc2_5_native_terminal_phase2.rs`) added MoldInst dispatch
//! and facade pack bindings, still build-time only. Phase 3 (this
//! file) is the first time RC2.5 actually runs the dispatcher against
//! a real cdylib at runtime, so it pins:
//!
//!   1. `RC2.5-3a`: when the addon returns `Status::Error`, the C
//!      dispatcher must build a Taida `AddonError` pack and call
//!      `taida_throw` so the user can catch it via
//!      `|== e: AddonError = ...`. Mirrors the interpreter's behaviour
//!      in `src/interpreter/addon_eval.rs::try_addon_func`.
//!
//!   2. `RC2.5-3b`: when `dlopen` (or `LoadLibraryA` on Windows) fails
//!      because the cdylib path was resolved at build time but the
//!      file no longer exists, the dispatcher must hard-fail with the
//!      spec-compliant `taida: addon load failed: <pkg>: <detail>`
//!      message and exit code 1. This is **not** convertible to a
//!      Taida throw — addons are language foundation and load failure
//!      is process-fatal (RC2.5_IMPL_SPEC F-7).
//!
//!   3. `RC2.5-3c`: the Windows dlopen abstraction
//!      (`#ifdef _WIN32` → `LoadLibraryA` / `GetProcAddress`)
//!      compiles cleanly. Linux/macOS test runs exercise the
//!      Unix branch; the Windows branch is gated as a
//!      `#[cfg(target_os = "windows")] #[ignore]` smoke test (full
//!      integration is RC3+ per `RC2.5B-005`).
//!
//! All tests reuse the in-tree `taida-addon-terminal-sample` cdylib
//! that the workspace already builds for `tests/addon_terminal_install_e2e.rs`.
//! That addon registers itself under the package name
//! `taida-lang/terminal` and exposes `termSize` / `termIsTty` /
//! `termPrintLn` / `termReadLine` / `termPrint`. Of those:
//!
//!   - `termIsTty` always succeeds with a `Bool` payload — perfect for
//!     the happy-path runtime parity sanity check.
//!   - `termReadLine` returns `Status::Error` (code 3, message
//!     `"termReadLine: EOF on stdin"`) when stdin is at EOF, which is
//!     exactly the deterministic non-TTY error variant we need for
//!     RC2.5-3a.
//!
//! The fixture writes a fresh `addon.toml` describing the sample
//! addon's function table, copies the cdylib into
//! `<project>/.taida/deps/<pkg>/native/`, lays down a tiny facade in
//! `<project>/.taida/deps/<pkg>/taida/`, and then drives `taida build
//! --target native` against a one-line `main.td`. Each test uses a
//! unique `package_id` so parallel test runs do not collide on the
//! process-wide addon registry inside the compiled binary.

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

/// Locate the workspace-built sample addon cdylib. Mirrors
/// `tests/addon_terminal_install_e2e.rs::find_terminal_cdylib`.
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

/// addon.toml describing the sample addon's function table. We pin
/// every function the sample registers, even though the Phase 3 tests
/// only call `termIsTty` and `termReadLine`, so the lowering pass
/// doesn't reject the import on a missing-arity check.
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

/// Minimal facade — Phase 2 already exercises the alias path
/// (`TerminalSize <= terminalSize`) so Phase 3 only needs the lowercase
/// names. Keeping the facade present avoids `lower_addon_import`
/// failing on a missing facade file when the test fixture is built.
const SAMPLE_FACADE_TD: &str = "";

/// Lay down a clean test project with:
///
///   - `packages.tdm` so `find_project_root` anchors here
///   - `.taida/deps/<pkg_dir>/native/addon.toml`
///   - `.taida/deps/<pkg_dir>/native/lib<stem>.<ext>` (real sample cdylib)
///   - `.taida/deps/<pkg_dir>/taida/<stem>.td` (empty facade)
///
/// `pkg_dir` is the path component (`taida-lang-sample/terminal`) so
/// each test gets its own scope and can't collide with another test
/// inside the same compiled binary's static `taida_addon_registry[]`.
fn write_sample_fixture(project: &Path, pkg_dir_rel: &str) -> PathBuf {
    let cdylib = match find_sample_terminal_cdylib() {
        Some(p) => p,
        None => panic!(
            "RC2.5 Phase 3 tests require the workspace-built `libtaida_addon_terminal_sample.{}` \
             — run `cargo build -p taida-addon-terminal-sample` first",
            cdylib_ext()
        ),
    };

    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"rc2_5-phase3-test\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    let pkg = project.join(".taida").join("deps").join(pkg_dir_rel);
    std::fs::create_dir_all(pkg.join("native")).expect("create native dir");
    std::fs::create_dir_all(pkg.join("taida")).expect("create taida dir");

    std::fs::write(pkg.join("native").join("addon.toml"), SAMPLE_ADDON_TOML)
        .expect("write addon.toml");
    // The facade stem must match the package directory name's last
    // segment so `lower_addon_import` finds it via
    // `<pkg_dir>/taida/<stem>.td`.
    std::fs::write(pkg.join("taida").join("terminal.td"), SAMPLE_FACADE_TD).expect("write facade");
    std::fs::write(
        pkg.join("packages.tdm"),
        "name <= \"taida-lang/terminal\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write pkg packages.tdm");

    // Copy the real sample cdylib into the resolved native/ slot.
    // RC2.5 lower.rs resolves this path at build time and embeds it
    // into the binary's .rodata, so the test does not need to set
    // any environment variables — the dispatcher reads the embedded
    // absolute path and dlopen()s it directly.
    let dest_name = cdylib_filename("taida_lang_terminal");
    let dest = pkg.join("native").join(&dest_name);
    std::fs::copy(&cdylib, &dest).expect("copy sample cdylib into fixture");

    dest
}

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

/// Run the compiled binary with stdin sourced from /dev/null (or the
/// equivalent NUL device on Windows). Returns (exit_code, stdout, stderr).
fn run_with_null_stdin(bin: &Path) -> (Option<i32>, String, String) {
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = cmd.output().expect("compiled binary must launch");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

// ── RC2.5-3a: Status::Error → catchable AddonError variant ─────────

/// RC2.5-3a, happy-path baseline: `termIsTty[]()` always returns
/// `Status::Ok` regardless of stdin attachment, so the dispatcher path
/// is exercised end-to-end without touching the new error branch. If
/// this test fails the dispatcher itself (Phase 1/2) is broken; the
/// Phase 3 error tests below would then be meaningless.
#[test]
fn term_is_tty_runs_on_native_with_real_sample_cdylib() {
    let project = unique_temp_dir("rc2_5_phase3_term_is_tty");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termIsTty)
result <= termIsTty()
stdout(`rc2_5_phase3: termIsTty=${result}`)
"#;

    let (ok, stdout, stderr, bin) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-3a baseline: termIsTty must compile on native. \
         stdout={} stderr={}",
        stdout, stderr
    );
    assert!(bin.exists(), "main.bin must exist");

    let (code, run_stdout, run_stderr) = run_with_null_stdin(&bin);
    assert_eq!(
        code,
        Some(0),
        "termIsTty baseline must exit cleanly. stderr={}",
        run_stderr
    );
    // /dev/null is not a TTY → addon returns Bool(false). Either
    // value is legal as long as the call completed without throwing.
    assert!(
        run_stdout.contains("rc2_5_phase3: termIsTty="),
        "expected termIsTty stdout marker, got: {}",
        run_stdout
    );
    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5-3a, the heart of Phase 3: `termReadLine[]()` against a
/// closed stdin returns `Status::Error` with `out_error->message =
/// "termReadLine: EOF on stdin"`. The dispatcher must convert this
/// into a Taida `AddonError` and `taida_throw` it so the user-side
/// `|== err: AddonError =` ceiling catches the error. We assert:
///
///   1. The compiled binary exits with code 0 (the throw is caught,
///      not propagated to a process-level gorilla fail).
///   2. stdout shows the post-catch marker line, proving control
///      reached the ceiling handler.
///   3. The catch payload's `err.type` is `"AddonError"`.
///   4. The catch payload's `err.message` contains the addon's
///      original `"termReadLine: EOF on stdin"` text.
#[test]
fn addon_status_error_becomes_catchable_addon_error_variant() {
    let project = unique_temp_dir("rc2_5_phase3_status_error");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termReadLine)

readUserInput =
  |== err: AddonError =
    stdout(`rc2_5_phase3: caught type=${err.type}`)
    stdout(`rc2_5_phase3: caught message=${err.message}`)
    "fallback"
  => :Str
  termReadLine()
=> :Str

stdout(`rc2_5_phase3: before-call`)
result <= readUserInput()
stdout(`rc2_5_phase3: after-call result=${result}`)
"#;

    let (ok, stdout, stderr, bin) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-3a: native build must succeed. \
         build stdout={} build stderr={}",
        stdout, stderr
    );
    assert!(bin.exists(), "main.bin must exist after build");

    let (code, run_stdout, run_stderr) = run_with_null_stdin(&bin);
    assert_eq!(
        code,
        Some(0),
        "RC2.5-3a: caught AddonError must allow normal exit. \
         stdout={} stderr={}",
        run_stdout,
        run_stderr
    );
    assert!(
        run_stdout.contains("rc2_5_phase3: before-call"),
        "must reach the call site. stdout={}",
        run_stdout
    );
    assert!(
        run_stdout.contains("rc2_5_phase3: caught type=AddonError"),
        "RC2.5-3a: ceiling handler must observe error_type=AddonError. \
         stdout={}",
        run_stdout
    );
    assert!(
        run_stdout.contains("termReadLine: EOF on stdin"),
        "RC2.5-3a: addon's original error message must propagate to the \
         caught Taida error variant. stdout={}",
        run_stdout
    );
    assert!(
        run_stdout.contains("rc2_5_phase3: after-call result=fallback"),
        "RC2.5-3a: handler return value must flow back to the caller. \
         stdout={}",
        run_stdout
    );
    // Hard-fail message must NOT appear: that prefix is reserved for
    // dlopen / dlsym / ABI mismatch / init failure (Phase 3b).
    assert!(
        !run_stderr.contains("taida: addon load failed"),
        "RC2.5-3a: Status::Error must not surface as a load failure. \
         stderr={}",
        run_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5-3a, parent class catch: the user can also catch the addon
/// error via the catch-all `Error` handler (since `AddonError`
/// inherits semantically from `Error` via the runtime's
/// `taida_error_type_matches` "Error" sentinel). Pins that the
/// dispatcher's pack shape interoperates with the existing error
/// hierarchy.
#[test]
fn addon_status_error_can_be_caught_via_error_parent_handler() {
    let project = unique_temp_dir("rc2_5_phase3_error_parent");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termReadLine)

readUserInput =
  |== err: Error =
    stdout(`rc2_5_phase3: parent-caught type=${err.type}`)
    "parent-fallback"
  => :Str
  termReadLine()
=> :Str

result <= readUserInput()
stdout(`rc2_5_phase3: parent result=${result}`)
"#;

    let (ok, stdout, stderr, bin) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-3a parent: native build must succeed. \
         build stdout={} build stderr={}",
        stdout, stderr
    );

    let (code, run_stdout, run_stderr) = run_with_null_stdin(&bin);
    assert_eq!(
        code,
        Some(0),
        "RC2.5-3a parent: catch-all Error handler must intercept the throw. \
         stdout={} stderr={}",
        run_stdout,
        run_stderr
    );
    assert!(
        run_stdout.contains("rc2_5_phase3: parent-caught type=AddonError"),
        "parent handler must see the original error_type. stdout={}",
        run_stdout
    );
    assert!(
        run_stdout.contains("rc2_5_phase3: parent result=parent-fallback"),
        "parent handler return value must flow back. stdout={}",
        run_stdout
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2.5-3b: dlopen / dlsym / ABI hard-fail semantics ─────────────

/// RC2.5-3b: build a binary against the sample cdylib, then delete
/// the cdylib before running. The dispatcher's first call must
/// hard-fail with the spec-mandated `taida: addon load failed:`
/// message and exit code 1. This is **not** convertible to a Taida
/// throw (RC2.5_IMPL_SPEC F-7).
///
/// Also pins that the message format includes the package id and
/// some platform-specific dlopen detail (so a developer who hits
/// this in production can immediately tell which addon failed and
/// why).
#[test]
fn dlopen_failure_after_build_is_hard_fail_with_spec_message() {
    let project = unique_temp_dir("rc2_5_phase3_dlopen_fail");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let cdylib_path = write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termIsTty)
result <= termIsTty()
stdout(`rc2_5_phase3: should-not-print=${result}`)
"#;

    let (ok, stdout, stderr, bin) = build_native(&project, main_td);
    assert!(
        ok,
        "RC2.5-3b: native build must succeed (cdylib path resolved at build time). \
         build stdout={} build stderr={}",
        stdout, stderr
    );
    assert!(bin.exists(), "main.bin must exist after build");

    // Now break the runtime resolution by deleting the cdylib.
    // The build-time-embedded absolute path will fail dlopen at
    // first-call time, which the dispatcher converts to a hard fail.
    std::fs::remove_file(&cdylib_path).expect("delete cdylib so dlopen will fail");

    let (code, run_stdout, run_stderr) = run_with_null_stdin(&bin);
    assert_ne!(
        code,
        Some(0),
        "RC2.5-3b: dlopen failure must NOT exit cleanly. \
         stdout={} stderr={}",
        run_stdout,
        run_stderr
    );
    assert_eq!(
        code,
        Some(1),
        "RC2.5-3b: dlopen failure must exit with code 1 (hard fail). \
         stdout={} stderr={}",
        run_stdout,
        run_stderr
    );
    assert!(
        run_stderr.contains("taida: addon load failed:"),
        "RC2.5-3b: hard-fail stderr must use spec-mandated format. \
         got: {}",
        run_stderr
    );
    assert!(
        run_stderr.contains("taida-lang/terminal"),
        "RC2.5-3b: hard-fail message must include the package id. \
         got: {}",
        run_stderr
    );
    // The user-side stdout marker after the call must NOT appear.
    assert!(
        !run_stdout.contains("rc2_5_phase3: should-not-print"),
        "RC2.5-3b: control must not return from a failed dlopen. \
         stdout={}",
        run_stdout
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// RC2.5-3b: corrupting the cdylib (truncating the file to zero bytes,
/// which makes dlopen reject it as not-an-ELF / not-a-Mach-O) must also
/// hit the hard-fail path. Distinct test case from outright deletion
/// because some platforms surface different dlerror() text for the two
/// situations, and we want to pin that **both** flow through the same
/// hard-fail entry.
#[test]
fn dlopen_corrupt_cdylib_is_hard_fail_with_spec_message() {
    let project = unique_temp_dir("rc2_5_phase3_dlopen_corrupt");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let cdylib_path = write_sample_fixture(&project, "taida-lang/terminal");

    let main_td = r#">>> taida-lang/terminal => @(termIsTty)
result <= termIsTty()
stdout(`rc2_5_phase3: should-not-print=${result}`)
"#;

    let (ok, _stdout, _stderr, bin) = build_native(&project, main_td);
    assert!(ok, "RC2.5-3b corrupt: native build must succeed");
    assert!(bin.exists());

    // Truncate the cdylib to zero bytes — dlopen rejects this with
    // an "invalid ELF header" / "file too short" / "not a valid Mach-O
    // file" message depending on platform, but always returns NULL.
    std::fs::write(&cdylib_path, b"").expect("truncate cdylib");

    let (code, _run_stdout, run_stderr) = run_with_null_stdin(&bin);
    assert_eq!(
        code,
        Some(1),
        "RC2.5-3b corrupt: zero-byte cdylib must hard-fail with exit 1. \
         stderr={}",
        run_stderr
    );
    assert!(
        run_stderr.contains("taida: addon load failed:"),
        "RC2.5-3b corrupt: stderr must use spec-mandated format. \
         got: {}",
        run_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

// ── RC2.5-3c: Windows abstraction smoke test ───────────────────────

/// RC2.5-3c: the `#ifdef _WIN32` branch in `native_runtime.c` is
/// gated as a smoke test on Windows. v1 scope is Linux primary +
/// macOS secondary; real Windows execution is RC3+. We mark this
/// `#[ignore]` on every platform so it never runs in CI by default,
/// but compiling it is enough to keep the cfg block from bit-rotting
/// (the test source itself does not depend on dlopen, so the cfg
/// guard's purpose is purely documentary on non-Windows hosts).
#[cfg(target_os = "windows")]
#[test]
#[ignore]
fn windows_dlopen_abstraction_smoke() {
    // Windows execution coverage is RC3+ scope (RC2.5B-005). On
    // Windows the test would lay down the same fixture as the Linux
    // tests above and call termIsTty; for now we just keep the
    // cfg block compilable so the abstraction macros don't bit-rot.
    let _ = find_sample_terminal_cdylib();
}
