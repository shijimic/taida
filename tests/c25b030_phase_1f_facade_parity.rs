//! C25B-030 Phase 1F: interpreter ↔ codegen facade **parity** +
//! pure-Taida package (no `addon.toml`) native verify.
//!
//! Background: Phase 1A-1E lifted the facade loader in
//! `src/codegen/lower/imports.rs::load_addon_facade_for_lower` to
//! the same surface area as the interpreter-side
//! `src/interpreter/module_eval.rs::load_addon_facade` (aliases /
//! pack literals / FuncDefs / private `_`-prefixed helpers / relative
//! `>>>` child imports). Phase 1F pins the **parity contract** between
//! the two loaders: the same facade ingest must produce the same
//! user-visible export shape (symbol set / typed values / function
//! arity behaviour) whether executed by the interpreter or lowered to
//! native IR.
//!
//! Scope (Phase 1F):
//!
//! 1. **Interpreter ↔ native parity** on a custom facade that
//!    exercises every construct accepted by the C25B-030 loader
//!    (pack literal / alias / public FuncDef / private `_`-prefixed
//!    helper / relative `>>>` / `<<<` authoritative export).
//! 2. **Pure-Taida package** (`.taida/deps/<org>/<pkg>/` without
//!    `addon.toml`) native-build smoke. Verifies that C25B-030's
//!    facade-loader changes did not regress the non-addon package
//!    import path (which feeds through `resolve_package_module` and
//!    does **not** invoke `load_addon_facade_for_lower`).
//!
//! Out of scope (tracked elsewhere):
//!
//! - Phase 1E-γ constructs (TypeDef / EnumDef / MoldDef / `<<<
//!   <path>`). Real `taida-lang/terminal` does not use any of these,
//!   and the loader's error messages already point authors at
//!   `C25B-030 Phase 1E-γ pending`.
//! - C25B-031 (`Slice[s, pos_var, end]()` positional-args parity) —
//!   independent parity bug isolated during Phase 1E-β-3, tracked
//!   for Phase 3.
//!
//! Soft-skip discipline: the interpreter-backed tests require a
//! functional `taida-lang/terminal` cdylib so that
//! `AddonRegistry::ensure_loaded` can successfully `dlopen` the
//! library at import time (the actual addon functions are NEVER
//! called by these tests — we only exercise the facade loader).
//! When no pre-built cdylib is available, those tests soft-skip
//! the same way `tests/rc2_terminal_surface.rs` does.

#![cfg(feature = "native")]

mod common;

use common::taida_bin as resolve_taida_bin;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Shared fixture helpers ──────────────────────────────────

fn taida_bin() -> PathBuf {
    resolve_taida_bin()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "c25b030_1f_{}_{}_{}",
        prefix,
        std::process::id(),
        nanos
    ))
}

/// Locate the official `taida-lang/terminal` package's native
/// artefacts. We search in order:
///
/// 1. The sibling repo layout used by `rc2_terminal_surface.rs` —
///    `../terminal/native/addon.toml` relative to this crate.
/// 2. The git submodule that ships with this repo under
///    `.dev/official-package-repos/terminal/`.
///
/// Returns `Some((addon_toml_path, cdylib_path))` when both a
/// manifest and a pre-built cdylib are present, `None` otherwise.
/// `None` drives soft-skip with a `note:` log line.
fn locate_terminal_artefacts() -> Option<(PathBuf, PathBuf)> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let cdylib_name = if cfg!(target_os = "linux") {
        "libtaida_lang_terminal.so"
    } else if cfg!(target_os = "macos") {
        "libtaida_lang_terminal.dylib"
    } else if cfg!(target_os = "windows") {
        "taida_lang_terminal.dll"
    } else {
        "libtaida_lang_terminal.so"
    };

    let candidates: Vec<PathBuf> = vec![
        // Sibling-repo layout (matches rc2_terminal_surface.rs).
        manifest_dir.parent().map(|p| p.join("terminal")),
        // In-repo submodule (default).
        Some(
            manifest_dir
                .join(".dev")
                .join("official-package-repos")
                .join("terminal"),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();

    for candidate in candidates {
        let addon_toml = candidate.join("native").join("addon.toml");
        if !addon_toml.exists() {
            continue;
        }
        // Try the repo's own native/ directory first (what was copied
        // into the dist), then fall back to target/{release,debug}.
        let mut search: Vec<PathBuf> = vec![candidate.join("native").join(cdylib_name)];
        for profile in ["release", "debug"] {
            search.push(candidate.join("target").join(profile).join(cdylib_name));
        }
        for cdylib in search {
            if cdylib.exists() {
                return Some((addon_toml, cdylib));
            }
        }
    }

    None
}

/// Install a custom addon facade against the real
/// `taida-lang/terminal` manifest + cdylib. The facade text is
/// caller-supplied so each test can exercise a different construct
/// mix; the cdylib is copied in so the interpreter's
/// `AddonRegistry::ensure_loaded` can `dlopen` successfully at
/// import time.
///
/// Returns `false` when prerequisites are missing (no built
/// cdylib) so the test can soft-skip. Prints a `note:` line in
/// that case.
fn install_terminal_with_custom_facade(
    project: &Path,
    facade_td: &str,
    extra_files: &[(&str, &str)],
) -> bool {
    let Some((addon_toml_src, cdylib_src)) = locate_terminal_artefacts() else {
        eprintln!(
            "note: skipping Phase 1F parity test — \
             no taida-lang/terminal cdylib found. Run `cargo build --release` \
             inside `.dev/official-package-repos/terminal/` (or a sibling \
             `../terminal/` checkout) to enable."
        );
        return false;
    };

    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"c25b030-1f-test\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    let pkg = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal");
    std::fs::create_dir_all(pkg.join("native")).expect("create native dir");
    std::fs::create_dir_all(pkg.join("taida")).expect("create taida dir");

    std::fs::copy(&addon_toml_src, pkg.join("native").join("addon.toml")).expect("copy addon.toml");
    let cdylib_name = cdylib_src.file_name().expect("cdylib file name").to_owned();
    std::fs::copy(&cdylib_src, pkg.join("native").join(&cdylib_name)).expect("copy cdylib");
    std::fs::write(pkg.join("taida").join("terminal.td"), facade_td)
        .expect("write terminal facade");

    for (name, body) in extra_files {
        std::fs::write(pkg.join("taida").join(name), body).expect("write sibling facade file");
    }

    std::fs::write(
        pkg.join("packages.tdm"),
        "name <= \"taida-lang/terminal\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write pkg packages.tdm");

    true
}

fn run_interpreter(project: &Path, main_td: &str) -> (bool, String, String) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .current_dir(project)
        .output()
        .expect("taida binary must run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn build_and_run_native(project: &Path, main_td: &str) -> (bool, String, String) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.bin"))
        .current_dir(project)
        .output()
        .expect("taida build must run");
    if !build.status.success() {
        return (
            false,
            String::from_utf8_lossy(&build.stdout).into_owned(),
            String::from_utf8_lossy(&build.stderr).into_owned(),
        );
    }
    let run = Command::new(project.join("main.bin"))
        .current_dir(project)
        .output()
        .expect("built binary must run");
    (
        run.status.success(),
        String::from_utf8_lossy(&run.stdout).into_owned(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    )
}

fn build_native_only(project: &Path, main_td: &str) -> (bool, String, String) {
    std::fs::write(project.join("main.td"), main_td).expect("write main.td");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.bin"))
        .current_dir(project)
        .output()
        .expect("taida build must run");
    (
        build.status.success(),
        String::from_utf8_lossy(&build.stdout).into_owned(),
        String::from_utf8_lossy(&build.stderr).into_owned(),
    )
}

// ── Parity core: interpreter ↔ native on a mixed facade ────

/// Phase 1F core parity: a facade mixing pack literal / public
/// FuncDef / private `_`-prefixed helper / relative `>>>` / `<<<`
/// must produce identical stdout under interpreter and native.
///
/// We deliberately avoid calling any `[functions]` entry from the
/// manifest — the cdylib is only needed so the interpreter's
/// `AddonRegistry::ensure_loaded` can open it at import time. The
/// observable surface is the facade's own FuncDefs and packs.
///
/// Note: facade FuncDefs must carry an explicit `=> :Type` return
/// tag when the body is a single-line expression. The parser
/// otherwise treats `Hello name = expr` as an assignment with
/// `Hello(name)` as the LHS (Taida ambiguity rule).
#[test]
fn phase_1f_mixed_facade_produces_identical_output_interpreter_vs_native() {
    let project = unique_temp_dir("mixed_facade_parity");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let terminal_td = r#"
>>> ./style.td => @(Stylize, Swatch)

KeyKind <= @(
  Char  <= 0
  Enter <= 1
  Esc   <= 2
)

_combine prefix name = `${prefix}-${name}` => :Str

Greet who = _combine("hi", who) => :Str

Shout phrase = _combine("!!", phrase) => :Str

<<< @(KeyKind, Greet, Shout, Stylize, Swatch)
"#;

    let style_td = r#"
Swatch <= @(
  red   <= "31"
  green <= "32"
  blue  <= "34"
)

_wrapCode code body = `\x1b[${code}m${body}\x1b[0m` => :Str

Stylize text code = _wrapCode(code, text) => :Str

<<< @(Stylize, Swatch)
"#;

    if !install_terminal_with_custom_facade(&project, terminal_td, &[("style.td", style_td)]) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    // Main exercises: pack field access, single-arg facade FuncDef,
    // nested private-helper resolution, cross-file public helper,
    // cross-file private helper chain.
    let main_td = r#">>> taida-lang/terminal => @(KeyKind, Greet, Shout, Stylize, Swatch)
stdout(`kc=${KeyKind.Char} ke=${KeyKind.Enter} kesc=${KeyKind.Esc}`)
stdout(Greet("world"))
stdout(Shout("ok"))
stdout(`swR=${Swatch.red} swG=${Swatch.green} swB=${Swatch.blue}`)
stdout(Stylize("alpha", Swatch.red))
stdout(Stylize("beta", Swatch.green))
"#;

    let (interp_ok, interp_stdout, interp_stderr) = run_interpreter(&project, main_td);
    assert!(
        interp_ok,
        "interpreter must accept the mixed facade. stdout={}, stderr={}",
        interp_stdout, interp_stderr
    );

    let (native_ok, native_stdout, native_stderr) = build_and_run_native(&project, main_td);
    assert!(
        native_ok,
        "native build+run must accept the mixed facade. stdout={}, stderr={}",
        native_stdout, native_stderr
    );

    assert_eq!(
        interp_stdout.trim_end_matches('\n'),
        native_stdout.trim_end_matches('\n'),
        "interpreter vs native stdout must match byte-for-byte.\n\
         interpreter:\n{}\n----\nnative:\n{}",
        interp_stdout,
        native_stdout
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// Phase 1F symbol-arity parity: a facade `FuncDef` with guards
/// and multiple argument shapes must have identical runtime arity
/// dispatch under both backends.
#[test]
fn phase_1f_facade_funcdef_arity_and_guards_match_interpreter() {
    let project = unique_temp_dir("arity_parity");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let terminal_td = r#"
Clamp n lo hi =
  | n < lo |> lo
  | n > hi |> hi
  | _      |> n

Sum3 a b c = a + b + c => :Int

Tag name = `<${name}>` => :Str

<<< @(Clamp, Sum3, Tag)
"#;

    if !install_terminal_with_custom_facade(&project, terminal_td, &[]) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(Clamp, Sum3, Tag)
stdout(`c1=${Clamp(5, 0, 10)}`)
stdout(`c2=${Clamp(-3, 0, 10)}`)
stdout(`c3=${Clamp(99, 0, 10)}`)
stdout(`s=${Sum3(1, 2, 3)}`)
stdout(Tag("facade"))
"#;

    let (interp_ok, interp_stdout, interp_stderr) = run_interpreter(&project, main_td);
    assert!(
        interp_ok,
        "interpreter arity path failed. stdout={}, stderr={}",
        interp_stdout, interp_stderr
    );

    let (native_ok, native_stdout, native_stderr) = build_and_run_native(&project, main_td);
    assert!(
        native_ok,
        "native arity path failed. stdout={}, stderr={}",
        native_stdout, native_stderr
    );

    assert_eq!(
        interp_stdout.trim_end_matches('\n'),
        native_stdout.trim_end_matches('\n'),
        "FuncDef arity / guard parity diverged.\n\
         interpreter:\n{}\n----\nnative:\n{}",
        interp_stdout,
        native_stdout
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// Phase 1F: `<<<` export list is authoritative on BOTH backends.
/// A name present in the facade but absent from `<<<` must fail
/// the user import deterministically on both paths. The exact
/// error text differs between interpreter and codegen, but both
/// must surface the missing-symbol condition — no silent fallback
/// to the raw manifest `[functions]` table.
#[test]
fn phase_1f_explicit_exports_are_authoritative_on_both_backends() {
    let project = unique_temp_dir("export_auth");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let terminal_td = r#"
Public <= @(kind <= 1, val <= "public")
Secret <= @(kind <= 2, val <= "secret")

<<< @(Public)
"#;

    if !install_terminal_with_custom_facade(&project, terminal_td, &[]) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(Secret)
stdout(`leaked=${Secret.val}`)
"#;

    let (interp_ok, _interp_stdout, interp_stderr) = run_interpreter(&project, main_td);
    assert!(
        !interp_ok,
        "interpreter must reject importing a name not in `<<<`"
    );
    assert!(
        interp_stderr.contains("not found") || interp_stderr.contains("Secret"),
        "interpreter stderr should name the missing symbol, got: {}",
        interp_stderr
    );

    let (native_ok, _native_stdout, native_stderr) = build_native_only(&project, main_td);
    assert!(
        !native_ok,
        "native build must reject importing a name not in `<<<`"
    );
    assert!(
        native_stderr.contains("not found")
            || native_stderr.contains("is not exported")
            || native_stderr.contains("does not export")
            || native_stderr.contains("Secret"),
        "native stderr should name the missing symbol, got: {}",
        native_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// Phase 1F cross-file private helper parity: the "`Greet` calls
/// `Join2` calls `_sep` lookup" chain must resolve identically on
/// both backends when private helpers span multiple facade files
/// reached through `>>> ./X.td`.
#[test]
fn phase_1f_cross_file_private_helper_chain_matches_interpreter() {
    let project = unique_temp_dir("cross_file_private");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    let terminal_td = r#"
>>> ./join.td => @(Join2)

Greet who = Join2("hi", who) => :Str

<<< @(Greet)
"#;
    let join_td = r#"
_sep <= "-"

Join2 a b = `${a}${_sep}${b}` => :Str

<<< @(Join2)
"#;

    if !install_terminal_with_custom_facade(&project, terminal_td, &[("join.td", join_td)]) {
        let _ = std::fs::remove_dir_all(&project);
        return;
    }

    let main_td = r#">>> taida-lang/terminal => @(Greet)
stdout(Greet("world"))
stdout(Greet("parity"))
"#;

    let (interp_ok, interp_stdout, interp_stderr) = run_interpreter(&project, main_td);
    assert!(
        interp_ok,
        "interpreter cross-file private helper chain failed. stdout={}, stderr={}",
        interp_stdout, interp_stderr
    );

    let (native_ok, native_stdout, native_stderr) = build_and_run_native(&project, main_td);
    assert!(
        native_ok,
        "native cross-file private helper chain failed. stdout={}, stderr={}",
        native_stdout, native_stderr
    );

    assert_eq!(
        interp_stdout.trim_end_matches('\n'),
        native_stdout.trim_end_matches('\n'),
        "cross-file private helper parity failed.\n\
         interpreter:\n{}\n----\nnative:\n{}",
        interp_stdout,
        native_stdout
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// Phase 1F pure-Taida package **native-build** smoke: a package
/// installed under `.taida/deps/<org>/<pkg>/` **without**
/// `addon.toml` (i.e. a plain source package, not an addon) must
/// still compile natively via the normal
/// `resolve_package_module` path. This exercises the fact that
/// C25B-030's facade-loader changes did not accidentally leak
/// into the non-addon import path.
///
/// We assert build-success only (not output parity), because
/// resolver-path cross-module FuncDef imports on the interpreter
/// have a pre-existing quirk: a single-line body
/// `F x = expr` without an explicit `=> :Type` return tag parses
/// as an assignment rather than a FuncDef, so the imported symbol
/// reaches the consumer as a Unit pack. That behaviour is
/// unrelated to C25B-030 (it affects the resolver path, not the
/// facade loader) and is tracked separately in FUTURE_BLOCKERS /
/// docs clarifications. For Phase 1F scope we only want the
/// **build-side** assurance that the facade-loader changes did
/// not regress package build coverage.
#[test]
fn phase_1f_pure_taida_package_builds_natively() {
    let project = unique_temp_dir("pure_taida");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();

    std::fs::write(
        project.join("packages.tdm"),
        "name <= \"c25b030-1f-pure\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write project packages.tdm");

    // Pure-Taida package: no `native/addon.toml`, just a source
    // entry. Use the explicit-return-tag FuncDef form so the
    // function symbol survives on both interpreter and native
    // resolver paths.
    let pkg = project
        .join(".taida")
        .join("deps")
        .join("demo")
        .join("greet");
    std::fs::create_dir_all(&pkg).expect("create pure package dir");
    std::fs::write(
        pkg.join("packages.tdm"),
        "name <= \"demo/greet\"\nversion <= \"0.1.0\"\n",
    )
    .expect("write pkg packages.tdm");
    std::fs::write(
        pkg.join("main.td"),
        r#"Hello name = `hi, ${name}!` => :Str

<<< @(Hello)
"#,
    )
    .expect("write pure package main.td");

    // Consumer: just import the symbol. We only verify
    // build-success here — see the doc comment above.
    let main_td = r#">>> demo/greet => @(Hello)
stdout("pure-taida-build-ok")
"#;

    let (native_ok, native_stdout, native_stderr) = build_native_only(&project, main_td);
    assert!(
        native_ok,
        "native pure-Taida package build failed. stdout={}, stderr={}",
        native_stdout, native_stderr
    );

    let _ = std::fs::remove_dir_all(&project);
}
