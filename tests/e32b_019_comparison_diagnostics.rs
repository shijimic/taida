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

fn assert_e1605_contains(case: &str, source: &str, parts: &[&str]) {
    let errors = check(source);
    let e1605 = errors
        .iter()
        .find(|err| err.message.contains("[E1605]"))
        .unwrap_or_else(|| panic!("{}: expected [E1605], got {:?}", case, errors));
    for part in parts {
        assert!(
            e1605.message.contains(part),
            "{}: expected [E1605] message to contain {:?}, got {}",
            case,
            part,
            e1605.message
        );
    }
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

// E32B-064: extend the E1605 net so containers that previously slipped past the
// fourth-pass walk (list literals, named args of constructors, parenthesised
// let-rhs) are also diagnosed. Implementation already covers them via the
// recursive `infer_expr_type_without_recording_errors` walk; without these
// fixtures a future refactor could regress the coverage silently.
#[test]
fn e32b_064_reports_nested_comparison_mismatches_extra_contexts() {
    let cases = [
        (
            "list literal",
            r#"
Enum => Status = :Ok :Retry
xs <= @[Status:Retry() > 0]
"#,
        ),
        (
            "named arg of constructor",
            r#"
Enum => Status = :Ok :Retry
Box = @(value: Bool)
b <= Box(value <= Status:Retry() > 0)
"#,
        ),
        (
            "let-rhs with extra paren",
            r#"
Enum => Status = :Ok :Retry
res <= ((Status:Retry() > 0)).toString()
"#,
        ),
    ];

    for (case, source) in cases {
        assert_has_e1605(case, source);
    }
}

// E32B-045: Template interpolations that parse-error on a trailing fragment
// (e.g. `|> bar` is not legal as a binary expression) still produce a partial
// AST whose comparison prefix must be diagnosed. Previously the checker
// dropped the partial AST whenever `parse_errors` was non-empty, swallowing
// the embedded `[E1605]`. Multiple interpolations and trailing operator drops
// all need to keep firing so that the user sees the real type mismatch.
#[test]
fn e32b_045_template_interpolation_partial_parse_still_emits_e1605() {
    let cases = [
        (
            "trailing pipe drop",
            r#"
foo <= 1
msg <= `bad ${foo == "x" |> bar}`
"#,
        ),
        (
            "trailing pipe with stdout sink",
            r#"
n <= 1
msg <= `head ${n == "a" |> stdout}`
"#,
        ),
        (
            "multiple interpolations second has trailing pipe drop",
            r#"
foo <= 1
msg <= `head ${foo == 2} tail ${foo == "x" |> bar}`
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

#[test]
fn e32b_060_comparison_uses_main_pass_binding_types() {
    assert_e1605_contains(
        "generic binding types",
        r#"
left <= Lax[1]()
right <= Lax["x"]()
stdout(left == right)
"#,
        &["Lax[Int]", "Lax[Str]"],
    );
}

#[test]
fn e32b_059_deep_nested_builtin_arg_does_not_need_fourth_pass_rewalk() {
    let mut source = String::new();
    source.push_str("seed <= 0\n");
    for i in 0..1200 {
        source.push_str(&format!("v{} <= seed + {}\n", i, i));
    }
    source.push_str("stdout(v1199 == \"bad\")\n");
    assert_e1605_contains("deep builtin arg", &source, &["Int", "Str"]);
}
