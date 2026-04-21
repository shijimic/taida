//! C20B-017 / ROOT-20: `|==` (ErrorCeiling) handler-body cleanup.
//!
//! Pre-fix: when the handler body itself raises a `RuntimeError` (e.g. a call
//! to an undefined function, a method-lookup failure, etc.), the handler
//! scope pushed at `src/interpreter/eval.rs:341` is never popped because
//! `eval_statements(&ec.handler_body)?` propagates the error with the `?`
//! operator, bypassing the `pop_scope()` on the following line.
//!
//! The caller (`call_function*`) already applies Pattern B cleanup on body
//! errors — it pops the local scope and closure scope. But those two pops
//! peel off the leaked handler scope and the local scope, leaving the
//! closure scope of the enclosing function alive across REPL inputs.
//!
//! Post-fix: the handler path is rewritten to Pattern B (no `?`):
//!
//!   let handler_result = self.eval_statements(&ec.handler_body);
//!   self.env.pop_scope();
//!   let handler_signal = handler_result?;
//!
//! so the handler scope is popped on both Ok and Err paths.
//!
//! These tests drive the interpreter directly so they can observe `env.depth()`
//! after a failed evaluation, which the CLI cannot expose (the CLI exits on
//! the first `RuntimeError`).

use taida::interpreter::Interpreter;
use taida::parser::parse;

/// Minimal repro from the blocker description.
///
/// `makeOuter("alice")` returns an inner closure that captures `secret`.
/// Inside, `boom(x)` has an error ceiling whose handler calls
/// `notAFunction(error.message)` — an undefined variable. When `boom(-1)`
/// throws `MyError`, the handler matches and runs; looking up `notAFunction`
/// raises a `RuntimeError`, skipping the handler-scope pop.
///
/// Pre-fix: `call_function`'s Pattern B cleanup then pops the handler scope
/// and the local scope of the inner closure, but the closure scope (holding
/// `secret = "alice"`) remains. A subsequent `stdout(secret)` at the top
/// level silently prints "alice".
///
/// Post-fix: the handler scope is popped before control returns to
/// `call_function`; `call_function`'s cleanup pops the local and closure
/// scopes; `secret` is undefined at the top level.
#[test]
fn c20b_017_handler_body_error_pops_handler_scope() {
    let prog1_src = "\
Error => MyError = @()

boom y =
  |== error: Error =
    notAFunction(error.message)
  => :Str
  | y < 0 |> MyError(type <= \"MyError\", message <= \"boom\").throw()
  | _ |> \"ok\"
=> :Str

makeOuter secret = _ x = boom(x) => :Str
f <= makeOuter(\"alice\")
stdout(f(0 - 1))
";
    let (prog1, errs1) = parse(prog1_src);
    assert!(errs1.is_empty(), "prog1 must parse: {:?}", errs1);

    let mut interp = Interpreter::new();
    let depth_before = interp.env.depth();

    let r1 = interp.eval_program(&prog1);
    assert!(
        r1.is_err(),
        "prog1 must error (handler body calls undefined `notAFunction`). Got: {:?}",
        r1
    );

    // The handler scope MUST be popped even when handler body errors.
    // Pre-fix: depth leaked upward (1 -> 2 or more).
    // Post-fix: depth returns to the baseline.
    let depth_after = interp.env.depth();
    assert_eq!(
        depth_after, depth_before,
        "env.depth() must return to baseline after handler body error. \
         before={}, after={}. Pre-fix leaked the handler scope (and closure \
         scope via call_function cleanup mis-alignment).",
        depth_before, depth_after
    );

    // prog2: read `secret` at the top level on the SAME Interpreter without
    // rebinding. Pre-fix the leaked closure scope still holds
    // `secret = "alice"`, so `stdout(secret)` silently prints "alice".
    // Post-fix: `secret` is undefined and we get a RuntimeError.
    let prog2_src = "\
stdout(secret)
";
    let (prog2, errs2) = parse(prog2_src);
    assert!(errs2.is_empty(), "prog2 must parse: {:?}", errs2);

    let r2 = interp.eval_program(&prog2);
    match r2 {
        Ok(_) => panic!(
            "prog2 unexpectedly succeeded — closure scope from failed \
             handler leaked: `secret` was still bound at the top level. \
             stdout buffer: {:?}",
            interp.output
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("secret") || msg.to_lowercase().contains("undefined"),
                "prog2 must fail with an undefined-variable error for 'secret'. \
                 Got: {}",
                msg
            );
        }
    }
}

/// Focused pin for the handler scope pop in isolation.
///
/// No closure / outer function — just a top-level `|==` whose handler body
/// errors. This is the narrowest observable symptom: `env.depth()` leaks
/// even without any caller-side cleanup scaffolding.
#[test]
fn c20b_017_top_level_handler_body_error_pops_scope() {
    let prog_src = "\
Error => MyError = @()

trigger y =
  |== error: Error =
    notAFunction(error.message)
  => :Str
  | y < 0 |> MyError(type <= \"MyError\", message <= \"boom\").throw()
  | _ |> \"ok\"
=> :Str

stdout(trigger(0 - 1))
";
    let (prog, errs) = parse(prog_src);
    assert!(errs.is_empty(), "prog must parse: {:?}", errs);

    let mut interp = Interpreter::new();
    let depth_before = interp.env.depth();

    let r = interp.eval_program(&prog);
    assert!(
        r.is_err(),
        "prog must error (handler body calls undefined `notAFunction`). Got: {:?}",
        r
    );

    let depth_after = interp.env.depth();
    assert_eq!(
        depth_after, depth_before,
        "env.depth() must return to baseline after handler body error. \
         before={}, after={}",
        depth_before, depth_after
    );
}

/// Regression guard: the success path of a `|==` handler must continue to
/// work. If the handler catches the throw and evaluates a clean body, the
/// value should be returned and `env.depth()` should return to baseline.
#[test]
fn c20b_017_handler_success_path_unchanged() {
    let prog_src = "\
Error => MyError = @()

recover y =
  |== error: Error =
    \"caught:\" + error.message
  => :Str
  | y < 0 |> MyError(type <= \"MyError\", message <= \"boom\").throw()
  | _ |> \"ok\"
=> :Str

stdout(recover(0 - 1))
stdout(recover(5))
";
    let (prog, errs) = parse(prog_src);
    assert!(errs.is_empty(), "prog must parse: {:?}", errs);

    let mut interp = Interpreter::new();
    let depth_before = interp.env.depth();

    let r = interp.eval_program(&prog);
    assert!(
        r.is_ok(),
        "handler success path must continue to work. Got: {:?}",
        r
    );

    let depth_after = interp.env.depth();
    assert_eq!(
        depth_after, depth_before,
        "env.depth() must return to baseline after handler success. \
         before={}, after={}",
        depth_before, depth_after
    );

    let stdout_joined: String = interp.output.concat();
    assert!(
        stdout_joined.contains("caught:boom"),
        "handler must produce 'caught:boom' for y < 0. stdout: {:?}",
        interp.output
    );
    assert!(
        stdout_joined.contains("ok"),
        "no-throw path must produce 'ok' for y >= 0. stdout: {:?}",
        interp.output
    );
}
