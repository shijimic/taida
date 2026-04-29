//! C20B-014 (ROOT-17): 3-backend parity + checker diagnostic harness for
//! user-defined functions called via mold syntax `Fn[args]()`.
//!
//! Pre-C20B-014 (2.1.3 runtime):
//!
//!   * **Interpreter** silently wrapped the call in a mold envelope:
//!     `cursorMoveTo[1, 1]()` returned `@(__value <= 1, __type <=
//!     "cursorMoveTo")` instead of invoking the function. `taida way check`
//!     passed because the checker fell through to `Type::Unknown`.
//!   * **Native lowering** emitted a `LowerError: unsupported mold type:
//!     <fn-name>` at build time (different failure mode from Interpreter
//!     — also a parity bug).
//!   * **JS** happened to work: the fallback `__taida_solidify(Fn(...))`
//!     actually calls the user function.
//!
//! Hachikuma TUI reproduced this at 81 call sites (`CursorMoveTo[r, c]()`,
//! `PadWidth[...]()`, `TruncateWidth[...]()`, etc.); the smoke bash
//! driver (`docs/smoke/smoke-tui-headless.sh`) crashed on launch in all
//! 3 scenarios.
//!
//! Post-C20B-014:
//!
//!   * Interpreter detects `Value::Function` in scope with no MoldDef
//!     and dispatches to `call_function`, treating `type_args` as
//!     positional arguments.
//!   * Native lowering does the same via `lower_func_call` before the
//!     `unsupported mold type` error.
//!   * Checker returns the function's declared return type instead of
//!     `Type::Unknown`, and rejects named `fields` with `[E1511]`.
//!   * JS path unchanged (already correct).
//!
//! Red test ゼロ容認 — any divergence between `Fn[args]()` and
//! `Fn(args)` on any backend is a C20 regression.

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path() -> PathBuf {
    manifest_dir().join("examples/quality/c20_mold_user_fn/mold_user_fn_parity.td")
}

fn expected_path() -> PathBuf {
    manifest_dir().join("examples/quality/c20_mold_user_fn/mold_user_fn_parity.expected")
}

fn read_expected() -> String {
    fs::read_to_string(expected_path()).expect("expected file must exist")
}

fn unique_temp(prefix: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        ext
    ))
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn outputs_equal(a: &str, b: &str) -> bool {
    a.trim_end_matches('\n') == b.trim_end_matches('\n')
}

/// Parse the expected file and assert that the `moldN=` line equals the
/// `callN=` line for every N. Catches the specific ROOT-17 regression
/// shape without embedding fragile literal output here.
fn assert_mold_call_parity_in_output(stdout: &str, backend: &str) {
    let mut mold_values: Vec<(usize, String)> = Vec::new();
    let mut call_values: Vec<(usize, String)> = Vec::new();

    let lines: Vec<&str> = stdout.lines().collect();
    let mut i = 0;
    while i + 1 < lines.len() {
        let tag = lines[i];
        let val = lines[i + 1];
        if let Some(rest) = tag.strip_prefix("mold")
            && let Some(n_str) = rest.strip_suffix('=')
            && let Ok(n) = n_str.parse::<usize>()
        {
            mold_values.push((n, val.to_string()));
        } else if let Some(rest) = tag.strip_prefix("call")
            && let Some(n_str) = rest.strip_suffix('=')
            && let Ok(n) = n_str.parse::<usize>()
        {
            call_values.push((n, val.to_string()));
        }
        i += 1;
    }

    assert!(
        !mold_values.is_empty(),
        "[{}] no `moldN=` lines parsed from stdout:\n{}",
        backend,
        stdout
    );
    assert_eq!(
        mold_values.len(),
        call_values.len(),
        "[{}] mold/call tag count mismatch: mold={:?} call={:?}",
        backend,
        mold_values,
        call_values
    );
    for ((mi, mv), (ci, cv)) in mold_values.iter().zip(call_values.iter()) {
        assert_eq!(
            mi, ci,
            "[{}] mold/call index drift: mold{} vs call{}",
            backend, mi, ci
        );
        assert_eq!(
            mv, cv,
            "[{}] C20B-014 parity violation: mold{}='{}' but call{}='{}'. \
             User-fn mold syntax must dispatch to the function.",
            backend, mi, mv, ci, cv
        );
        // Extra guard: if the pre-fix silent wrap regressed, the Interpreter
        // would have emitted something like `@(__value <= 1, __type <=
        // "cursorMoveTo")` for `mold1`. Flag that shape explicitly.
        assert!(
            !mv.contains("__value") && !mv.contains("__type"),
            "[{}] C20B-014 regression: mold{} leaked a mold wrapper: '{}'",
            backend,
            mi,
            mv
        );
    }
}

// ── Interpreter: primary regression ──

#[test]
fn c20b_014_mold_user_fn_interpreter_dispatches_to_function() {
    let out = Command::new(taida_bin())
        .arg(fixture_path())
        .output()
        .expect("failed to spawn interpreter");
    assert!(
        out.status.success(),
        "interpreter non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C20B-014 interpreter output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
    assert_mold_call_parity_in_output(&stdout, "interpreter");
}

// ── JS backend: already correct pre-fix (regression guard) ──

#[test]
fn c20b_014_mold_user_fn_js_matches_interpreter() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let mjs = unique_temp("c20b014_js", "mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(fixture_path())
        .arg("-o")
        .arg(&mjs)
        .output()
        .expect("failed to spawn js build");
    assert!(
        build.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&mjs)
        .output()
        .expect("failed to spawn node");
    let _ = fs::remove_file(&mjs);
    assert!(
        run.status.success(),
        "node exit failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C20B-014 JS output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
    assert_mold_call_parity_in_output(&stdout, "js");
}

// ── Native backend: primary lowering regression ──

#[test]
fn c20b_014_mold_user_fn_native_matches_interpreter() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let bin = unique_temp("c20b014_native", "bin");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(fixture_path())
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn native build");
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("failed to spawn native binary");
    let _ = fs::remove_file(&bin);
    assert!(
        run.status.success(),
        "native binary exit failed: status={:?}, stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C20B-014 native output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
    assert_mold_call_parity_in_output(&stdout, "native");
}

// ── Checker: named `()` fields on a user-fn mold-syntax call trigger E1511 ──

#[test]
fn c20b_014_checker_rejects_named_fields_on_user_fn_mold_call() {
    // User function `greet` takes 1 arg. `greet["x"](extra <= "y")` is
    // invalid — user fns have no named-field ABI. This must surface as
    // `[E1511]` at `taida way check` time.
    let src =
        "greet name = \"hi \" + name => :Str\nmsg <= greet[\"x\"](extra <= \"y\")\nstdout(msg)\n";
    let src_path = unique_temp("c20b014_fields_src", "td");
    fs::write(&src_path, src).expect("write src");
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("failed to spawn check");
    let _ = fs::remove_file(&src_path);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let combined = format!("{}\n{}", stdout, stderr);
    assert!(
        !out.status.success(),
        "check expected to fail but exit=0. combined output:\n{}",
        combined
    );
    assert!(
        combined.contains("[E1511]"),
        "expected `[E1511]` diagnostic for user-fn mold call with fields, got:\n{}",
        combined
    );
}

// ── Checker: zero-positional, zero-fields mold-syntax call still typechecks ──

#[test]
fn c20b_014_checker_accepts_user_fn_mold_syntax() {
    // `echo["abc"]()` on a user function is valid mold syntax — the
    // checker must return the fn's real return type (Str) instead of
    // `Type::Unknown` so downstream type-constrained contexts (such as
    // assignment to a typed binding) keep working.
    let src = "echo s = s => :Str\nmsg <= echo[\"abc\"]()\nstdout(msg)\n";
    let src_path = unique_temp("c20b014_accept_src", "td");
    fs::write(&src_path, src).expect("write src");
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("failed to spawn check");
    let _ = fs::remove_file(&src_path);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "check should pass but failed. stdout={} stderr={}",
        stdout,
        stderr
    );
}

// ── C20B-016 (ROOT-19): user-fn mold syntax must reuse normal
//    function-call arity / type / partial validation ──
//
// Pre-C20B-016: the user-fn mold-syntax fallback only rejected named
// fields and returned the raw function return type. Arity mismatches
// (E1301), type mismatches (E1506) and partial-application errors
// (E1505) that `Fn(args)` catches were silently skipped for
// `Fn[args]()`, producing runtime crashes on code that `taida way check`
// accepted.
//
// Fix: delegate to the normal `Expr::FuncCall` path via a synthesised
// call expression. These tests pin the `Fn[args]()` diagnostics to
// match `Fn(args)` exactly.

fn run_check(src: &str, prefix: &str) -> (std::process::Output, String) {
    let src_path = unique_temp(prefix, "td");
    fs::write(&src_path, src).expect("write src");
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("failed to spawn check");
    let _ = fs::remove_file(&src_path);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, combined)
}

#[test]
fn c20b_016_type_mismatch_mold_syntax_emits_e1506() {
    // The pinned repro: `add[1, "x"]()` must surface `[E1506]` at check
    // time, mirroring the `add(1, "x")` diagnostic.
    let src =
        "add a: Int b: Int = a + b => :Int\nresult <= add[1, \"x\"]()\nstdout(result.toString())\n";
    let (out, combined) = run_check(src, "c20b016_type");
    assert!(
        !out.status.success(),
        "[C20B-016] `add[1, \"x\"]()` was accepted by taida way check — regression. combined:\n{}",
        combined
    );
    assert!(
        combined.contains("[E1506]") && combined.contains("add"),
        "[C20B-016] expected `[E1506]` on 'add', got:\n{}",
        combined
    );
}

#[test]
fn c20b_016_type_mismatch_parenthesis_syntax_emits_e1506() {
    // Sanity anchor: `add(1, "x")` emits the same diagnostic. Confirms
    // the two syntaxes land on the same code path.
    let src =
        "add a: Int b: Int = a + b => :Int\nresult <= add(1, \"x\")\nstdout(result.toString())\n";
    let (out, combined) = run_check(src, "c20b016_type_paren");
    assert!(
        !out.status.success(),
        "`add(1, \"x\")` baseline check drift. combined:\n{}",
        combined
    );
    assert!(
        combined.contains("[E1506]") && combined.contains("add"),
        "expected `[E1506]` on 'add' for paren syntax, got:\n{}",
        combined
    );
}

#[test]
fn c20b_016_arity_overflow_mold_syntax_emits_e1301() {
    // `add[1, 2, 3]()` passes too many args — must hit [E1301].
    let src =
        "add a: Int b: Int = a + b => :Int\nresult <= add[1, 2, 3]()\nstdout(result.toString())\n";
    let (out, combined) = run_check(src, "c20b016_arity");
    assert!(
        !out.status.success(),
        "[C20B-016] arity overflow via mold syntax accepted. combined:\n{}",
        combined
    );
    assert!(
        combined.contains("[E1301]") && combined.contains("add"),
        "[C20B-016] expected `[E1301]` on 'add', got:\n{}",
        combined
    );
}

#[test]
fn c20b_016_valid_mold_syntax_call_passes_check_and_runs() {
    // `add[1, 2]()` is the canonical valid form and must keep working:
    // checker passes, interpreter produces `3`.
    let src = "add a: Int b: Int = a + b => :Int\nresult <= add[1, 2]()\nstdout(result.toString())\nstdout(\"\\n\")\n";
    let src_path = unique_temp("c20b016_valid", "td");
    fs::write(&src_path, src).expect("write src");
    // Check phase
    let check = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("spawn check");
    assert!(
        check.status.success(),
        "[C20B-016] valid `add[1, 2]()` rejected by check: stdout={} stderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    // Run phase (interpreter)
    let run = Command::new(taida_bin())
        .arg(&src_path)
        .output()
        .expect("spawn run");
    let _ = fs::remove_file(&src_path);
    assert!(
        run.status.success(),
        "[C20B-016] valid `add[1, 2]()` failed at runtime. stdout={} stderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert!(
        stdout.trim_end() == "3",
        "[C20B-016] expected stdout '3', got '{}'",
        stdout
    );
}
