//! Typed HIR smoke test: verifies that `TypeChecker::typed_expr_table`
//! is populated correctly after `check_program` for representative
//! fixtures. Codegen lowering consumes this table to drive Bool
//! detection from the type-checker's verdict instead of syntax.

use taida::parser::parse;
use taida::types::{Type, TypeChecker, TypedExprTable};

fn typed_table_for(src: &str) -> TypedExprTable {
    let (program, parse_errors) = parse(src);
    assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
    let mut checker = TypeChecker::new();
    checker.check_program(&program);
    assert!(
        checker.errors.is_empty(),
        "Type errors: {:?}",
        checker.errors
    );
    checker.typed_expr_table.clone()
}

#[test]
fn smoke_records_lax_inner_int() {
    let table = typed_table_for("obj <= Lax[42]()\n");
    let has_lax_int = table.iter().any(|(_, ty)| {
        matches!(ty, Type::Generic(n, args)
            if n == "Lax" && args.len() == 1 && args[0] == Type::Int)
    });
    assert!(has_lax_int, "Expected Lax[Int] in TypedExprTable");
}

#[test]
fn smoke_records_method_call_chain() {
    // Lax[42]().map(double).hasValue() — table should contain Bool, Int, Lax[Int] entries.
    let src = "double x: Int = x * 2 => :Int\n\
               obj <= Lax[42]()\n\
               result <= obj.map(double).hasValue()\n";
    let table = typed_table_for(src);
    assert!(
        table.iter().any(|(_, ty)| ty == &Type::Bool),
        "Expected Bool in table"
    );
    assert!(
        table.iter().any(|(_, ty)| ty == &Type::Int),
        "Expected Int in table"
    );
}

#[test]
fn smoke_records_chained_lambda_inference() {
    // Lambda chain via bidirectional inference: `obj.map(_ x = x + 1)`.
    // Expects Lambda's Function type recorded with [Int] -> Int.
    let src = "obj <= Lax[42]()\n\
               result <= obj.map(_ x = x + 1)\n";
    let table = typed_table_for(src);
    let has_int_to_int_fn = table.iter().any(|(_, ty)| {
        matches!(ty, Type::Function(params, ret)
            if params == &vec![Type::Int] && **ret == Type::Int)
    });
    assert!(
        has_int_to_int_fn,
        "Expected Function([Int], Int) for bidirectional-hinted lambda, got: {:?}",
        table.iter().collect::<Vec<_>>()
    );
}

#[test]
fn smoke_records_result_with_error_type() {
    // Result[42](throw <= Fail(...)) — table should record Result[Int, Fail].
    let src = "Error => Fail = @(message: Str)\n\
               r <= Result[42](throw <= Fail(message <= \"e\"))\n";
    let table = typed_table_for(src);
    let has_result = table.iter().any(|(_, ty)| {
        matches!(ty, Type::Generic(n, args)
            if n == "Result" && args.len() == 2 && args[0] == Type::Int)
    });
    assert!(has_result, "Expected Result[Int, _] in table");
}

#[test]
fn smoke_simple_program_no_residual_unknown() {
    // Lock-C 文: type-checker 完了後の Typed HIR には Type::Unknown 残らない (this fixture).
    let src = "x <= 42\ny <= x + 1\nstdout(y.toString())\n";
    let table = typed_table_for(src);
    assert!(
        !table.has_residual_unknown(),
        "Phase 1 acceptance: simple program should have no residual Unknown, got: {:?}",
        table
            .iter()
            .filter(|(_, t)| t.contains_concrete_unknown())
            .collect::<Vec<_>>()
    );
}

#[test]
fn smoke_records_bool_method_call() {
    // hasValue() / isEmpty() are recorded as Bool so codegen uses the
    // type-checker's verdict instead of method-name syntax.
    let src = "b1 <= Lax[42]().hasValue()\n\
               b2 <= Lax[42]().isEmpty()\n";
    let table = typed_table_for(src);
    let bool_count = table.iter().filter(|(_, t)| **t == Type::Bool).count();
    assert!(
        bool_count >= 2,
        "Expected at least 2 Bool entries (hasValue, isEmpty); got {}",
        bool_count
    );
}

#[test]
fn smoke_records_str_method_call() {
    // toString() recorded as Str.
    let src = "s <= 42.toString()\n";
    let table = typed_table_for(src);
    assert!(
        table.iter().any(|(_, t)| t == &Type::Str),
        "Expected Str entry from toString()"
    );
}

#[test]
fn smoke_table_grows_with_program() {
    let small = typed_table_for("x <= 1\n");
    let large = typed_table_for("x <= 1\ny <= 2\nz <= x + y\n");
    assert!(
        large.len() > small.len(),
        "Larger program should populate more entries: small={}, large={}",
        small.len(),
        large.len()
    );
}
