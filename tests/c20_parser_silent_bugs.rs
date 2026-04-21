//! C20-1: parser silent-bug regression guards.
//!
//! Two checker-green / runtime-broken shapes were discovered via the
//! Hachikuma Phase 8-10 MVP (`C19_BLOCKERS.md#ROOT-4` and `#ROOT-5`):
//!
//!   * `|== error: Error = <expr>` written on a single physical line
//!     silently consumed every subsequent top-level definition as part
//!     of the error-ceiling handler body (module load afterwards failed
//!     to find the vanished symbols).
//!   * `name <= | cond |> A | _ |> B` laid out across multiple lines
//!     on the right-hand side of `<=` greedy-absorbed the next
//!     statement as a continuation arm, so the following
//!     `processResult(name)` (or equivalent) never executed.
//!
//! The parser now:
//!
//!   1. splits `parse_error_ceiling` into a one-line / multi-line
//!      dispatch so the one-line form produces a single-expression
//!      handler body (ROOT-4 fix, see
//!      `src/parser/parser.rs::parse_error_ceiling`);
//!   2. switches into `CondBranchContext::LetRhs` while reading a
//!      `<=` rhs and rejects multi-line continuation arms in that
//!      context with `[E0303]` (ROOT-5 fix, see
//!      `src/parser/parser_expr.rs::parse_cond_branch`).
//!
//! Parentheses restore the top-level context, so
//! `name <= (| ... |> ... | _ |> ...)` remains a legal escape hatch.
//!
//! These are parser unit-scope guards — no backend parity is required
//! (parse / check phase only).

use taida::parser::{Expr, Statement, parse};

// ── ROOT-4: one-line `|==` must not eat the rest of the module ──────

#[test]
fn c20_error_ceiling_one_line_does_not_break_module_load() {
    // Before C20-1: `parse_block` after `=` used `false` as the block
    // indent and kept swallowing `greet x = ...` and `stdout(...)`.
    // AST had exactly 1 top-level statement (the error ceiling) and
    // `main_flow` was empty. After the fix we expect:
    //   * 1 ErrorCeiling
    //   * 1 FuncDef (greet)
    //   * 1 Expr (stdout call)
    let source = "\
Error => ValidationError = @(field: Str, code: Int)

validate text =
  |== error: Error = \"caught: \" + error.message
  => :Str
  | text == \"\" |> ValidationError(type <= \"V\", message <= \"E\", field <= \"t\", code <= 1).throw()
  | _ |> \"Valid: \" + text
=> :Str

stdout(validate(\"hello\"))
stdout(validate(\"\"))
";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Unexpected parse errors: {:?}", errors);

    // InheritanceDef + FuncDef + 2 Expr (stdout calls).
    // The key assertion is that the two `stdout(...)` calls at the
    // bottom survive — i.e. the one-line `|==` did NOT silently eat
    // them as handler-body statements.
    let stdout_count = program
        .statements
        .iter()
        .filter(|s| matches!(s, Statement::Expr(Expr::FuncCall(..))))
        .count();
    assert_eq!(
        stdout_count, 2,
        "both top-level stdout(validate(..)) calls must remain visible, got program: {:#?}",
        program
    );
    // We must see one FuncDef named `validate`.
    let has_validate = program.statements.iter().any(|s| match s {
        Statement::FuncDef(f) => f.name == "validate",
        _ => false,
    });
    assert!(
        has_validate,
        "top-level `validate` function must still be a distinct FuncDef, got: {:#?}",
        program
    );
}

#[test]
fn c20_error_ceiling_one_line_produces_single_expr_body() {
    // Pin the structural shape of the fix: the handler body is
    // exactly `vec![Statement::Expr(...)]`, matching the multi-line
    // form's canonical shape (one expression evaluated as the error
    // result).
    let source = "\
validate x =
  |== error: Error = false
  => :Bool
  | x > 0 |> true
  | _ |> false
=> :Bool
";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Unexpected parse errors: {:?}", errors);

    // The FuncDef's body should contain exactly one ErrorCeiling
    // wrapping a single-expression handler body.
    let func = program
        .statements
        .iter()
        .find_map(|s| match s {
            Statement::FuncDef(f) if f.name == "validate" => Some(f),
            _ => None,
        })
        .expect("validate FuncDef missing");
    let ec = func
        .body
        .iter()
        .find_map(|s| match s {
            Statement::ErrorCeiling(ec) => Some(ec),
            _ => None,
        })
        .expect("validate body should contain an ErrorCeiling");
    assert_eq!(
        ec.handler_body.len(),
        1,
        "one-line `|==` handler body must wrap exactly one Statement::Expr, got: {:#?}",
        ec.handler_body
    );
    assert!(
        matches!(&ec.handler_body[0], Statement::Expr(_)),
        "one-line `|==` handler body must be Statement::Expr, got: {:#?}",
        ec.handler_body[0]
    );
}

#[test]
fn c20_error_ceiling_one_line_parity_multi_line() {
    // A one-line and multi-line form should produce structurally
    // equivalent handler bodies (both `[Statement::Expr(_)]`).
    let one_line = "\
validate x =
  |== error: Error = 0
  => :Int
  | x > 0 |> 1
  | _ |> 0
=> :Int
";
    let multi_line = "\
validate x =
  |== error: Error =
    0
  => :Int
  | x > 0 |> 1
  | _ |> 0
=> :Int
";
    let (prog_one, errs_one) = parse(one_line);
    let (prog_multi, errs_multi) = parse(multi_line);
    assert!(errs_one.is_empty(), "one-line errs: {:?}", errs_one);
    assert!(errs_multi.is_empty(), "multi-line errs: {:?}", errs_multi);

    let one_handler = prog_one
        .statements
        .iter()
        .find_map(|s| match s {
            Statement::FuncDef(f) => f.body.iter().find_map(|inner| match inner {
                Statement::ErrorCeiling(ec) => Some(&ec.handler_body),
                _ => None,
            }),
            _ => None,
        })
        .expect("one-line handler body missing");
    let multi_handler = prog_multi
        .statements
        .iter()
        .find_map(|s| match s {
            Statement::FuncDef(f) => f.body.iter().find_map(|inner| match inner {
                Statement::ErrorCeiling(ec) => Some(&ec.handler_body),
                _ => None,
            }),
            _ => None,
        })
        .expect("multi-line handler body missing");
    assert_eq!(
        one_handler.len(),
        multi_handler.len(),
        "one-line and multi-line `|==` must yield the same handler-body length"
    );
}

// ── ROOT-5: multi-line rhs multi-arm guard must be rejected with [E0303] ─

#[test]
fn c20_rhs_multi_arm_guard_rejected_with_e0303() {
    let source = "\
Ctx = @(mode: Str)

runFlow ctx: Ctx =
  name <=
    | ctx.mode == \"A\" |> \"foo\"
    | _ |> \"bar\"

  processResult(name)
=> :Str
";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E0303]")),
        "multi-line rhs multi-arm guard must raise [E0303], got: {:?}",
        errors
    );
}

#[test]
fn c20_rhs_single_line_guard_accepted() {
    // Single-line form (all `|` tokens on the same physical line)
    // stays legal — `branch_line` equals the continuation `|` line
    // so the LetRhs-context guard does not fire.
    let source = "\
pick x =
  name <= | x > 0 |> \"pos\" | _ |> \"neg\"
  name
=> :Str
";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "single-line rhs multi-arm guard must stay accepted, got: {:?}",
        errors
    );
}

#[test]
fn c20_top_level_multi_arm_guard_still_accepted() {
    // Top-level / function-body `| |>` match is unchanged. We
    // consult `cond_branch_context` only for `<=` rhs, not for
    // function body tails.
    let source = "\
fizzbuzz n =
  | Mod[n, 15]().unmold() == 0 |> \"FizzBuzz\"
  | Mod[n, 3]().unmold() == 0 |> \"Fizz\"
  | _ |> n.toString()
=> :Str

stdout(fizzbuzz(15))
";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "top-level multi-arm match must remain accepted, got: {:?}",
        errors
    );
}

#[test]
fn c20_parenthesised_multi_arm_guard_in_rhs_accepted() {
    // The `LParen` primary resets `cond_branch_context` back to
    // `TopLevel` so that a parenthesised multi-line guard is the
    // canonical escape hatch.
    let source = "\
pick x =
  name <= (
    | x > 0 |> \"pos\"
    | _ |> \"neg\"
  )
  name
=> :Str
";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "parenthesised multi-line rhs guard must stay accepted, got: {:?}",
        errors
    );
}

#[test]
fn c20_rhs_typed_multi_arm_guard_also_rejected() {
    // Same guard applies to the typed binding form `name: T <=`.
    let source = "\
runFlow x: Int =
  name: Str <=
    | x > 0 |> \"pos\"
    | _ |> \"neg\"
  name
=> :Str
";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E0303]")),
        "typed `name: T <=` multi-line rhs must also raise [E0303], got: {:?}",
        errors
    );
}
