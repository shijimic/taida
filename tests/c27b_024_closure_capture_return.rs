//! C27B-024 (@c.27, Phase 9b): Interpreter closure capture bug for partial
//! application returned from functions — extended 3-backend parity guard.
//!
//! # Background
//!
//! C27B-024 is the C27 carry-over of C26B-017 (HI-005 from `hono-inspired`).
//! The original bug: returning a partial application as the implicit last
//! expression of a function body stripped closure capture in the
//! Interpreter, manifesting as "Cannot add <n> and @()". Root cause was
//! `eval_expr_tail` in `src/interpreter/eval.rs` treating a `FuncCall` with
//! `Expr::Hole(_)` arguments as a tail call to the inner function, which
//! evaluated each Hole as `Value::Unit` and re-targeted the trampoline
//! instead of building a partial closure.
//!
//! The fix landed in C26 commit `73fd0a1` (Round 3 wH): a 4-line guard in
//! `eval_expr_tail` that detects `Expr::Hole(_)` in args and routes the
//! expression to the normal evaluator so `eval_partial_application` builds
//! the proper closure value. Existing tests in
//! `tests/c26b_017_partial_app_closure_capture.rs` (5 tests) cover the
//! canonical regression scenarios.
//!
//! # C27B-024 acceptance scope
//!
//! Phase 0 Design Lock (`.dev/C27_BLOCKERS.md::C27B-024`) requires a
//! dedicated `tests/c27b_024_*.rs` regression guard with **5 minimum cases**:
//!
//!   1. 1-引数 partial (single hole, captured value)
//!   2. 2-引数 partial (e.g. hole-in-first variant exercising arg-position
//!      independence of the fix)
//!   3. nested closure return (a function that returns the result of another
//!      higher-order function — closure-of-closure path)
//!   4. pipeline 経由 (partial used as a pipeline step at the call site)
//!   5. pack field 内 closure (partial stored in a Pack field, retrieved,
//!      and invoked through the field accessor)
//!
//! All five cases must produce byte-identical output across Interpreter, JS,
//! and Native backends. This file lives independently of
//! `c26b_017_partial_app_closure_capture.rs` so that the C27 GATE evidence
//! references a C27-suffixed regression file (per Phase 14 GATE evidence
//! template), and so that the broader scenario coverage (cond branch / pack
//! field projection) is durable should the future runtime overhaul touch the
//! partial application machinery again.
//!
//! # Empirical guard-revert verification (2026-04-25, wG Round 1 fix)
//!
//! Round 1 wG review (FIX_REQUIRED) flagged that the original Case 4
//! (`runPipeline base input = input => add(base, _)`) and Case 5
//! (`makeBag base = Bag(label <= ..., op <= add(base, ))`) did NOT exercise
//! the `eval_expr_tail` `is_partial_application` guard at
//! `src/interpreter/eval.rs:820-821`, because the tail expression in those
//! cases was a `Pipeline` (Case 4) or a Pack constructor (Case 5) — neither
//! enters the `Expr::FuncCall` arm of `eval_expr_tail` where the guard sits.
//! The reviewer confirmed empirically that reverting the 4-line guard caused
//! Cases 1/2/3 to FAIL but Cases 4/5 to PASS, defeating the regression-guard
//! intent.
//!
//! Cases 4 and 5 were re-designed so that the partial application **is** the
//! tail expression of a function body, but in distinct syntactic surroundings
//! that broaden the regression coverage:
//!
//!   * Case 4 places the partial inside a `| cond |> body` arm, so
//!     `eval_expr_tail` recurses through `Expr::CondBranch` into each arm
//!     body and re-enters the `Expr::FuncCall` arm with Hole args present.
//!   * Case 5 places the partial in a function whose captured value is
//!     read from a Pack field (`b.base`) rather than from a plain
//!     parameter, ensuring the closure-capture path works when the bound
//!     argument originates from a Pack projection.
//!
//! Both rewritten cases were verified empirically (2026-04-25): with the
//! 4-line guard at `eval.rs:820-821` reverted (line 820 dropped, line 821
//! simplified to the bare `let Expr::Ident` match), the Interpreter emits
//! `Cannot add 50 and @()` for Case 4 and `Cannot add 30 and @()` for
//! Case 5 — the same `Cannot add <n> and @()` symptom that originally
//! reported HI-005. With the guard restored, all five cases pass on
//! Interpreter / JS / Native. Net effect: all 5 cases now RED-on-revert,
//! GREEN-on-restore — the regression guard is honest.

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
        "c27b_024_{}_{}_{}",
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

fn run_interp(src: &Path) -> String {
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

// ─────────────────────────────────────────────────────────────────────────────
// Case 1: 1-引数 partial — single hole, single captured value (canonical).
// Equivalent to `makeAdder n: Int = add(n, ) => :Int => :Int`. The captured
// `n` must survive the partial-closure construction across the function
// boundary.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn c27b_024_one_arg_partial_capture_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

makeAdder n: Int =
  add(n, )
=> :Int => :Int

add10 <= makeAdder(10)
stdout(add10(7))
"#;
    parity_assert("one_arg_partial", source, "17");
}

// ─────────────────────────────────────────────────────────────────────────────
// Case 2: 2-引数 partial — hole-in-first-position. Confirms the eval_expr_tail
// guard does not depend on hole position. The captured value `y` lives in the
// SECOND argument slot, the hole in the FIRST. If closure capture were broken
// only for last-position holes, this case would fail.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn c27b_024_two_arg_partial_hole_in_first_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

makeAddTo y: Int =
  add(, y)
=> :Int => :Int

plus100 <= makeAddTo(100)
stdout(plus100(7))
"#;
    parity_assert("hole_in_first", source, "107");
}

// ─────────────────────────────────────────────────────────────────────────────
// Case 3: nested closure return — `makeNested` returns the result of
// `makeAdder` (which itself returns a closure). The closure produced by
// `makeAdder(base)` must survive being returned from a wrapper function.
// Exercises the closure-of-closure path through the tail-call evaluator.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn c27b_024_nested_closure_return_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

makeAdder n: Int =
  add(n, )
=> :Int => :Int

makeNested base: Int =
  makeAdder(base)
=> :Int => :Int

nestedAdd <= makeNested(20)
stdout(nestedAdd(5))
"#;
    parity_assert("nested_closure", source, "25");
}

// ─────────────────────────────────────────────────────────────────────────────
// Case 4: cond-branch tail — the partial application sits inside a
// `| cond |> body` arm whose body is the tail expression of the function.
// `eval_expr_tail` recurses through `Expr::CondBranch` into each arm body
// and re-enters the `Expr::FuncCall` arm; the guard at
// `src/interpreter/eval.rs:820-821` must fire for the arm body
// `add(n, )` (Hole arg) to avoid the spurious tail call. Without the
// guard, the Interpreter emits `Cannot add 50 and @()` (verified
// empirically on 2026-04-25, see file-level comment).
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn c27b_024_cond_branch_partial_tail_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

makeCondAdder n: Int flag: Bool =
  | flag |> add(n, )
  | _ |> add(0, )
=> :Int => :Int

f <= makeCondAdder(50, true)
stdout(f(8))
"#;
    parity_assert("cond_branch_partial", source, "58");
}

// ─────────────────────────────────────────────────────────────────────────────
// Case 5: Pack-field captured base — the function body's tail expression is
// `add(b.base, )`, where the captured value is the projection of a Pack
// field rather than a plain parameter. This covers the Hono-inspired
// middleware pattern (HI-005 root use-case): a closure that captures
// configuration values from a context Pack and is returned as a handler.
// `eval_expr_tail` enters the `Expr::FuncCall` arm with `b.base` as the
// (non-Hole) first argument and `Expr::Hole(_)` as the second; the guard
// must fire so that the partial closure is built rather than the trampoline
// being re-targeted with `Value::Unit` for the Hole. Without the guard,
// the Interpreter emits `Cannot add 30 and @()` (verified empirically on
// 2026-04-25, see file-level comment).
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn c27b_024_pack_field_base_partial_tail_parity() {
    let source = r#"
add x: Int y: Int = x + y => :Int

Bag = @(base: Int)

opFromBag b: Bag =
  add(b.base, )
=> :Int => :Int

bag <= Bag(base <= 30)
op <= opFromBag(bag)
stdout(op(12))
"#;
    parity_assert("pack_field_base_partial", source, "42");
}
