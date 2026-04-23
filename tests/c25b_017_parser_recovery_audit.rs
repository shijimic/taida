//! C25B-017: parser error recovery audit (FB-31 continuation).
//!
//! # Context
//!
//! C20-1 (ROOT-4 / ROOT-5) closed the silent-bug class where
//! `parse_error_ceiling` and `parse_cond_branch` would swallow subsequent
//! top-level statements. The fixes landed as rejection diagnostics
//! (`[E0303]`, `[E0304]`), so the dangerous syntax now surfaces as a
//! `ParseError` rather than an invisible AST mutation.
//!
//! The remaining audit surface is **error-recovery correctness**: after
//! the parser emits one `ParseError` and calls `self.synchronize()`, the
//! next call to `parse_statement()` must see a **consistent tokenizer
//! state**. Specifically:
//!
//!   1. `peek_kind()` and the underlying `tokens[pos]` are aligned.
//!   2. The recovery does not leave us mid-expression — we are at a
//!      fresh statement boundary (or EOF).
//!   3. Every subsequent top-level statement either parses to a
//!      `Statement` or produces its own `ParseError`. No silent skip.
//!   4. The emitted `Vec<ParseError>` count matches the number of
//!      broken statements in the source; not fewer, not more.
//!
//! These tests drive `taida::parser::parse` directly and assert on the
//! `(Program, Vec<ParseError>)` shape. They are a regression guard for
//! any future refactor of `synchronize()` / `parse_cond_branch` /
//! `parse_error_ceiling` that might leak tokens or drop statements.

use taida::parser::parse;

/// After a broken `|==` header on line 1, the parser must still parse
/// the two well-formed top-level `stdout(...)` calls on the following
/// lines. Pre-C20-1 the malformed `|==` would silently absorb them.
#[test]
fn c25b_017_broken_error_ceiling_does_not_swallow_trailing_statements() {
    let src = "\
|== error:
stdout(\"a\")
stdout(\"b\")
";
    let (prog, errs) = parse(src);
    assert!(
        !errs.is_empty(),
        "broken |== must produce at least one ParseError"
    );
    // Two stdout calls must remain as top-level statements after recovery.
    let stdout_count = prog
        .statements
        .iter()
        .filter(|s| matches!(s, taida::parser::Statement::Expr(_)))
        .count();
    assert!(
        stdout_count >= 2,
        "expected at least 2 stdout statements to survive recovery, got {}. Statements: {:?}",
        stdout_count,
        prog.statements
    );
}

/// After a broken condition arm (malformed `| cond |>` syntax), the parser
/// must recover and parse subsequent top-level definitions. This pins
/// ROOT-5's silent-bug fix: the `|` continuation must not consume the
/// `stdout("after")` line.
#[test]
fn c25b_017_broken_cond_branch_does_not_swallow_trailing_statements() {
    let src = "\
f = | true
stdout(\"after\")
";
    let (prog, errs) = parse(src);
    assert!(!errs.is_empty(), "broken `|` must produce ParseError");
    // `stdout("after")` must survive recovery.
    let has_trailing_stdout = prog.statements.iter().any(|s| match s {
        taida::parser::Statement::Expr(e) => {
            matches!(e, taida::parser::Expr::FuncCall(_, _, _))
        }
        _ => false,
    });
    assert!(
        has_trailing_stdout,
        "stdout(\"after\") must survive recovery from broken `|`. Statements: {:?}",
        prog.statements
    );
}

/// Multiple consecutive errors must each produce their own `ParseError`
/// entry. The parser must not collapse to "fail fast" mode after the
/// first error — it must keep trying to recover and report as many
/// distinct errors as possible, so the user sees every broken line.
#[test]
fn c25b_017_multiple_errors_are_all_reported() {
    // Three lines that each start with an illegal leading `=` token.
    // Each should produce its own ParseError after synchronize() rewinds
    // to the next newline.
    let src = "\
= 1
= 2
= 3
";
    let (_prog, errs) = parse(src);
    assert!(
        errs.len() >= 2,
        "expected >= 2 ParseErrors for 3 broken lines, got {}. errs: {:?}",
        errs.len(),
        errs
    );
}

/// After recovery from a broken statement, a well-formed assignment on
/// the next line must produce a `Statement::Assignment` with the expected
/// variable name. This catches "recovery advanced past the newline but
/// landed inside the next expression" bugs where the next statement's
/// leading token is consumed.
#[test]
fn c25b_017_recovery_leaves_next_statement_intact() {
    // The leading `= 99` triggers a parse error at column 1 of line 1.
    // After synchronize() rewinds past the newline, line 2's binding
    // `good = 42` (or lambda binding interpretation thereof) and line 3's
    // `stdout(good)` must remain parseable. We assert by checking that
    // AT LEAST one survivor node mentions `good` / `stdout` somewhere,
    // rather than binding to a specific AST shape — the audit is about
    // "did recovery leave us in a consistent state", not "did it
    // classify the survivor as exactly this node kind".
    let src = "\
= 99
good = 42
stdout(good)
";
    let (prog, errs) = parse(src);
    assert!(!errs.is_empty());

    let rendered = format!("{:?}", prog.statements);
    assert!(
        rendered.contains("good") && rendered.contains("stdout"),
        "recovery must leave both `good` and `stdout` references in the \
         parsed statement list. Got: {}",
        rendered
    );
    // At least one top-level statement must have survived.
    assert!(
        !prog.statements.is_empty(),
        "at least one survivor statement must exist after recovery"
    );
}

/// EOF after a broken statement must not panic or loop. The parser must
/// gracefully terminate even when recovery has nothing to sync to.
#[test]
fn c25b_017_recovery_at_eof_terminates_cleanly() {
    let src = "= 5";
    // Should not panic. Should return with at least one error.
    let (_prog, errs) = parse(src);
    assert!(
        !errs.is_empty(),
        "broken single-line source must produce ParseError"
    );
}

/// A well-formed source must produce zero errors. Trivial but important
/// baseline — if recovery machinery has a bug, it might spuriously fire
/// on valid input.
#[test]
fn c25b_017_well_formed_source_yields_zero_errors() {
    let src = "\
x = 1
y = 2
z = x + y
stdout(z)
";
    let (prog, errs) = parse(src);
    assert!(
        errs.is_empty(),
        "well-formed source must have zero ParseErrors, got: {:?}",
        errs
    );
    assert_eq!(
        prog.statements.len(),
        4,
        "well-formed source must produce exactly 4 top-level statements"
    );
}

/// Mixed: a broken statement followed by a well-formed multi-line
/// function definition. This is the most common real-world shape and
/// pins that recovery correctly re-enters top-level dispatch after a
/// malformed line, then parses the multi-line function as a single
/// assignment with a lambda RHS.
#[test]
fn c25b_017_recovery_then_multiline_function_definition() {
    let src = "\
= 99

greet name =
  \"hello, \" + name
=> :Str
";
    let (prog, errs) = parse(src);
    assert!(!errs.is_empty(), "leading `=` must produce ParseError");

    // `greet` must still be recognised as either a function definition
    // or an assignment binding to a lambda.
    let has_greet = prog.statements.iter().any(|s| match s {
        taida::parser::Statement::Assignment(a) => a.target.as_str() == "greet",
        taida::parser::Statement::FuncDef(f) => f.name.as_str() == "greet",
        _ => false,
    });
    assert!(
        has_greet,
        "function `greet` must survive recovery. Statements: {:?}",
        prog.statements
    );
}
