//! E32B-019 / E1605 regression coverage.
//!
//! Comparison BinaryOps must be diagnosed even when they sit under expression
//! containers whose outer type can be inferred without visiting every child.

use taida::types::{TypeChecker, TypeError};

fn check(source: &str) -> Vec<TypeError> {
    let (program, parse_errors) = taida::parser::parse(source);
    assert!(
        parse_errors.is_empty(),
        "parse failed for source:\n{}\nerrors: {:?}",
        source,
        parse_errors
    );

    let mut checker = TypeChecker::new();
    checker.check_program(&program);
    checker.errors
}

fn assert_has_e1605(case: &str, source: &str) {
    let errors = check(source);
    assert!(
        errors.iter().any(|err| err.message.contains("[E1605]")),
        "{}: expected [E1605], got {:?}",
        case,
        errors
    );
}

#[test]
fn e32b_019_reports_nested_comparison_mismatches() {
    let cases = [
        ("function arg", r#"stdout(1 == "a")"#),
        (
            "method arg",
            r#"
text <= "abc"
out <= text.replace(1 == "a", "x")
"#,
        ),
        ("BuchiPack field", r#"stdout(@(bad <= 1 == "a"))"#),
        (
            "lambda body",
            r#"
n <= 1
stdout(_ x = n == "a")
"#,
        ),
        (
            "template interpolation",
            r#"
Enum => Status = :Ok :Retry
msg <= `bad ${Status:Retry() > 0}`
"#,
        ),
        (
            "conditional arm",
            r#"
stdout((
  | true |> 1 == "a"
  | _ |> false
))
"#,
        ),
    ];

    for (case, source) in cases {
        assert_has_e1605(case, source);
    }
}

#[test]
fn e32b_019_accepts_nested_compatible_comparisons() {
    let errors = check(
        r#"
n <= 1
stdout(@(ok <= n == 2, label <= `ok ${n < 3}`))
"#,
    );
    let e1605: Vec<_> = errors
        .iter()
        .filter(|err| err.message.contains("[E1605]"))
        .collect();
    assert!(e1605.is_empty(), "unexpected E1605 errors: {:?}", e1605);
}
