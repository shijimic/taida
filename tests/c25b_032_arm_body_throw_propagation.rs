//! C25B-032: `| _ |> <function-call-that-throws>` propagation to the
//! enclosing `|==` handler in the **same function body**.
//!
//! # Root cause
//!
//! `src/interpreter/control_flow.rs::Interpreter::eval_cond_arm_body` (the
//! **non-tail** variant, reached from `eval_expr → Expr::CondBranch`) used
//! to delegate to `eval_statements`, which unconditionally applies
//! tail-call optimisation on the arm body's **last** statement. When the
//! arm body is a single call to a user-defined function (e.g.
//! `| _ |> throwBoom("boom")`), `eval_expr_tail` classifies the call as a
//! mutual tail call and returns `Signal::TailCall(args)`.
//!
//! The `Signal::TailCall` then bubbles up past the protected-block guard
//! inside `eval_statements` → past `call_function`'s post-body dispatch,
//! where the trampoline switches the active function to `throwBoom` and
//! **re-executes it outside the enclosing `|==` error ceiling**. The
//! subsequent `.throw()` therefore surfaces as an unhandled error instead
//! of being caught by the in-scope `|== error: Error = ... => :Str`
//! handler.
//!
//! # Fix
//!
//! `eval_cond_arm_body` now delegates to `eval_statements_no_tco` —
//! mirroring the guard `eval_statements` already installs for an
//! `ErrorCeiling`'s protected block. The tail-position variant
//! `eval_cond_arm_body_tail` (reached from `eval_expr_tail`) is unchanged
//! because the surrounding context IS a legitimate tail position there.
//!
//! # Why this isn't a TCO correctness regression
//!
//! The non-tail `eval_cond_arm_body` is only invoked when the
//! `Expr::CondBranch` itself is not in tail position (e.g. the CondBranch
//! is the right-hand side of a `<=` binding, or a subexpression of a
//! larger expression). In those contexts the caller already cannot
//! collapse the arm body's final call into a TCO frame — the enclosing
//! expression always produces a value that outlives the call. So we lose
//! no legitimate TCO opportunity; we only stop the bogus tail-call that
//! was escaping the `|==` scope.
//!
//! Pre-existing `test_tco_*` suites (including `test_tco_with_error_ceiling`)
//! confirm the tail-position CondBranch path still performs TCO correctly.
//!
//! # Scope
//!
//! 3-backend parity: interpreter, JS, native. WASM lowering inherits JS
//! `try/catch` semantics and is not affected.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn write_fixture(body: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_c25b032_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join("main.td");
    fs::write(&path, body).expect("write fixture");
    path
}

const FIXTURE_SINGLE_ARM: &str = r#"Error => MyError = @()
throwBoom msg =
  MyError(type <= "MyError", message <= msg).throw()
  ""
=> :Str

trial flag =
  |== error: Error =
    "caught:" + error.message
  => :Str
  | _ |> throwBoom("boom")
=> :Str

stdout(trial(true))
"#;

const FIXTURE_GUARDED_ARM: &str = r#"Error => MyError = @()
throwBoom msg =
  MyError(type <= "MyError", message <= msg).throw()
  ""
=> :Str

trial flag =
  |== error: Error =
    "caught:" + error.message
  => :Str
  | flag |> throwBoom("guarded")
  | _ |> "fallthrough"
=> :Str

stdout(trial(true))
"#;

const FIXTURE_CHAINED_ARM: &str = r#"Error => MyError = @()
throwBoom msg =
  MyError(type <= "MyError", message <= msg).throw()
  ""
=> :Str

relay msg = throwBoom(msg) => :Str

trial flag =
  |== error: Error =
    "caught:" + error.message
  => :Str
  | _ |> relay("chained")
=> :Str

stdout(trial(true))
"#;

fn run_interp(path: &std::path::Path) -> (String, i32) {
    let output = Command::new(taida_bin())
        .arg(path)
        .output()
        .expect("run interp");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// The minimal bug repro from C25B-032: `| _ |> throwBoom("boom")` must
/// propagate `.throw()` inside `throwBoom` to the `|==` handler in the same
/// function, just as a direct `throwBoom("boom")` (no arm) already does.
#[test]
fn c25b_032_single_default_arm_calling_throwing_fn_is_caught_by_same_fn_handler() {
    let path = write_fixture(FIXTURE_SINGLE_ARM, "single_arm");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(
        stdout.trim(),
        "caught:boom",
        "arm body throw must be caught by same-function |== handler"
    );
}

/// Guard-variant: `| flag |> throwBoom("guarded")` must also propagate. The
/// earlier fix only covered the default `| _ |>` arm; verify both arm
/// shapes (conditional + default) share the fix.
#[test]
fn c25b_032_guarded_arm_calling_throwing_fn_is_caught() {
    let path = write_fixture(FIXTURE_GUARDED_ARM, "guarded_arm");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(
        stdout.trim(),
        "caught:guarded",
        "guarded arm body throw must be caught by same-function |== handler"
    );
}

/// Chain-variant: arm body calls a helper that itself calls the throwing
/// function. The throw originates two call levels below the arm body, which
/// stresses the TCO / Signal::TailCall escape path more aggressively. Pre-fix
/// the intermediate `relay` call would TCO out of the error-ceiling scope.
#[test]
fn c25b_032_chained_arm_body_throw_is_caught() {
    let path = write_fixture(FIXTURE_CHAINED_ARM, "chained_arm");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(
        stdout.trim(),
        "caught:chained",
        "chained arm body throw must be caught by same-function |== handler"
    );
}

/// 3-backend parity: interpreter, JS, native must all emit the same
/// `caught:boom` line for the default-arm fixture. Pre-fix the interpreter
/// diverged (unhandled error); JS (try/catch) and native (setjmp/longjmp)
/// already matched the fixed behaviour.
#[test]
fn c25b_032_default_arm_throw_3backend_parity() {
    // Node availability check. If `node` is not on PATH, skip the JS half
    // and only assert interp/native parity.
    let node_ok = Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let cc_ok = Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let path = write_fixture(FIXTURE_SINGLE_ARM, "parity");
    let dir = path.parent().unwrap().to_path_buf();

    let (interp_out, interp_code) = run_interp(&path);
    assert_eq!(
        interp_code, 0,
        "interpreter must exit 0, got {}",
        interp_code
    );

    if node_ok {
        let js_path = dir.join("out.mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&path)
            .arg("-o")
            .arg(&js_path)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("node").arg(&js_path).output().expect("run js");
        assert!(run.status.success(), "node exit non-zero");
        let js_out = String::from_utf8_lossy(&run.stdout).to_string();
        assert_eq!(
            js_out.trim(),
            interp_out.trim(),
            "interp vs JS parity on C25B-032 fixture must match"
        );
    }

    if cc_ok {
        let bin_path = dir.join("out.bin");
        let build = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(&path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&bin_path).output().expect("run native bin");
        assert!(run.status.success(), "native exit non-zero");
        let nat_out = String::from_utf8_lossy(&run.stdout).to_string();
        assert_eq!(
            nat_out.trim(),
            interp_out.trim(),
            "interp vs native parity on C25B-032 fixture must match"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Regression guard: confirm the direct (no-arm) form ALREADY worked and
/// still does. This pins the baseline that pre-fix was the only working
/// pattern and protects against any over-correction that would regress it.
#[test]
fn c25b_032_direct_throw_without_arm_still_caught() {
    const FIXTURE: &str = r#"Error => MyError = @()
throwBoom msg =
  MyError(type <= "MyError", message <= msg).throw()
  ""
=> :Str

trial flag =
  |== error: Error =
    "caught:" + error.message
  => :Str
  throwBoom("direct")
=> :Str

stdout(trial(true))
"#;
    let path = write_fixture(FIXTURE, "direct");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(stdout.trim(), "caught:direct");
}
