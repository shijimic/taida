//! C26B-017 (@c.26, Cluster 6 Surface): partial application returned as a
//! function's implicit return value loses closure capture in Interpreter.
//!
//! # Bug
//!
//! Interpreter-only. When a user returns a partial application directly as
//! the last expression of a function body (no intermediate bind), the call
//! produced `Runtime error: Cannot add <n> and @()` because the tail-call
//! optimizer treated the partial as a real call to the inner function and
//! evaluated each `Hole` argument as `Value::Unit`.
//!
//! ```taida
//! makeAdder n: Int =
//!   add(n, )             // Hole in last position — implicit return
//! => :Int => :Int
//!
//! add10 <= makeAdder(10)
//! stdout(add10(7))       // Interp: "Cannot add 10 and @()", JS/Native: 17
//! ```
//!
//! Binding the partial to a local first (`myPartial <= add(n, )` then
//! returning `myPartial`) worked around the bug because the bind is not a
//! tail call.
//!
//! # Root cause
//!
//! `src/interpreter/eval.rs::eval_expr_tail` detected any `FuncCall` with a
//! user-defined callee in tail position and short-circuited to a TailCall
//! signal, which:
//!   1. evaluated each arg (Hole → `Value::Unit`), and
//!   2. re-targeted the trampoline to the inner function instead of building
//!      a partial closure.
//!
//! This stripped the closure capture and turned `add(n, )` into
//! `add(n, Unit)`, which then errored inside `add`.
//!
//! # Fix
//!
//! `eval_expr_tail` now checks whether any argument is an `Expr::Hole(_)`.
//! If so, the call is a partial application (not a real tail call) and the
//! expression is evaluated via the normal path, routing through
//! `eval_partial_application` to build the proper closure value.
//!
//! JS and Native were already correct (partial apps lower to explicit
//! closure values at codegen time, bypassing the tail-call machinery).

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_fixture(tag: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "c26b_017_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir tmpdir");
    let src = dir.join("fixture.td");
    fs::write(&src, source).expect("write fixture");
    (dir, src)
}

fn run_interp(src: &PathBuf) -> String {
    let out = Command::new(taida_bin())
        .arg(src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_js(src: &Path, dir: &Path) -> Option<String> {
    if !node_available() {
        eprintln!("node unavailable; skipping JS leg");
        return None;
    }
    let js = dir.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(src)
        .arg("-o")
        .arg(&js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node").arg(&js).output().expect("run js");
    assert!(
        run.status.success(),
        "js run failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
}

fn run_native(src: &Path, dir: &Path) -> Option<String> {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native leg");
        return None;
    }
    let bin = dir.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(
        run.status.success(),
        "native run failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
}

fn parity_assert(tag: &str, source: &str, expected: &str) {
    let (dir, src) = write_fixture(tag, source);
    let interp = run_interp(&src);
    assert_eq!(interp, expected, "interp mismatch ({tag})");
    if let Some(js) = run_js(&src, &dir) {
        assert_eq!(js, expected, "js mismatch ({tag})");
    }
    if let Some(native) = run_native(&src, &dir) {
        assert_eq!(native, expected, "native mismatch ({tag})");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// C26B-017 canonical repro: `makeAdder` returns a partial as its implicit
/// last expression. Interpreter must capture `n` correctly, not lose it to
/// the tail-call trampoline. 3-backend parity.
#[test]
fn c26b_017_make_adder_implicit_return_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

makeAdder n: Int =
  add(n, )
=> :Int => :Int

add10 <= makeAdder(10)
stdout(add10(7))
"#;
    parity_assert("make_adder", source, "17");
}

/// Negative control: partial app at the top level (not tail position). Must
/// still pass after the fix (no regression of the inline-partial path).
#[test]
fn c26b_017_inline_partial_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

add5 <= add(5, )
stdout(add5(3))
"#;
    parity_assert("inline_partial", source, "8");
}

/// Positive control: partial app bound to a local then returned. Worked
/// before the fix; must still work after. 3-backend parity.
#[test]
fn c26b_017_make_adder_bind_then_return_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

makeAdder n: Int =
  myPartial <= add(n, )
  myPartial
=> :Int => :Int

add10 <= makeAdder(10)
stdout(add10(7))
"#;
    parity_assert("bind_then_return", source, "17");
}

/// First-hole-in-middle variant: Hole in middle position (not just last).
/// Confirms the fix generalizes beyond the single-last-hole pattern.
#[test]
fn c26b_017_hole_in_middle_implicit_return_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

// Partial with Hole in FIRST position, captured arg in SECOND position.
// Returned as the function's implicit last expression.
makeAddTo y: Int =
  add(, y)
=> :Int => :Int

plus10 <= makeAddTo(10)
stdout(plus10(7))
"#;
    parity_assert("hole_in_first_pos", source, "17");
}

/// Regression guard: genuine tail call (no Hole) must still trampoline.
/// Ensures we didn't accidentally disable TCO for normal user functions.
#[test]
fn c26b_017_regression_genuine_tail_call_tco_parity() {
    let source = r#"
helper n: Int = n + 1 => :Int

// Tail call (no Hole): must still work and take the TCO path without
// producing a closure value.
wrap n: Int =
  helper(n)
=> :Int

stdout(wrap(41))
"#;
    parity_assert("genuine_tco", source, "42");
}
