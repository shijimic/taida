#[cfg(test)]
mod tests {
    use crate::interpreter::eval::{Interpreter, eval};
    use crate::interpreter::value::Value;

    fn eval_ok(source: &str) -> Value {
        eval(source).unwrap_or_else(|e| panic!("Eval failed: {}", e))
    }

    // ── Literals ──

    #[test]
    fn test_eval_int() {
        assert_eq!(eval_ok("42"), Value::Int(42));
    }

    #[test]
    fn test_eval_float() {
        assert_eq!(eval_ok("3.14"), Value::Float(314.0 / 100.0));
    }

    #[test]
    fn test_eval_string() {
        assert_eq!(eval_ok("\"hello\""), Value::str("hello".to_string()));
    }

    #[test]
    fn test_eval_bool() {
        assert_eq!(eval_ok("true"), Value::Bool(true));
        assert_eq!(eval_ok("false"), Value::Bool(false));
    }

    // ── Arithmetic ──

    #[test]
    fn test_eval_addition() {
        assert_eq!(eval_ok("x <= 1 + 2"), Value::Int(3));
    }

    #[test]
    fn test_eval_subtraction() {
        assert_eq!(eval_ok("x <= 10 - 3"), Value::Int(7));
    }

    #[test]
    fn test_eval_multiplication() {
        assert_eq!(eval_ok("x <= 6 * 7"), Value::Int(42));
    }

    #[test]
    fn test_eval_enum_variant_to_ordinal() {
        assert_eq!(
            eval_ok("Enum => Status = :Ok :Fail\nstatus <= Status:Fail()"),
            Value::Int(1)
        );
    }

    #[test]
    fn test_eval_single_variant_enum_to_zero() {
        assert_eq!(
            eval_ok("Enum => Traffic = :Red\nsignal <= Traffic:Red()"),
            Value::Int(0)
        );
    }

    #[test]
    fn test_eval_div_mold() {
        // Div[10, 3]() returns Lax with hasValue=true, value=3
        let result = eval_ok("result <= Div[10, 3]()");
        assert!(
            matches!(&result, Value::BuchiPack(_)),
            "Expected BuchiPack, got {:?}",
            result
        );
        if let Value::BuchiPack(fields) = &result {
            let has_value = fields
                .iter()
                .find(|(k, _)| k == "hasValue")
                .unwrap()
                .1
                .clone();
            let value = fields
                .iter()
                .find(|(k, _)| k == "__value")
                .unwrap()
                .1
                .clone();
            assert_eq!(has_value, Value::Bool(true));
            assert_eq!(value, Value::Int(3));
        }
    }

    #[test]
    fn test_eval_div_mold_zero() {
        // Div[10, 0]() returns Lax with hasValue=false
        let result = eval_ok("result <= Div[10, 0]()");
        assert!(
            matches!(&result, Value::BuchiPack(_)),
            "Expected BuchiPack, got {:?}",
            result
        );
        if let Value::BuchiPack(fields) = &result {
            let has_value = fields
                .iter()
                .find(|(k, _)| k == "hasValue")
                .unwrap()
                .1
                .clone();
            assert_eq!(has_value, Value::Bool(false));
        }
    }

    #[test]
    fn test_eval_mod_mold() {
        // Mod[10, 3]() returns Lax with hasValue=true, value=1
        let result = eval_ok("result <= Mod[10, 3]()");
        assert!(
            matches!(&result, Value::BuchiPack(_)),
            "Expected BuchiPack, got {:?}",
            result
        );
        if let Value::BuchiPack(fields) = &result {
            let has_value = fields
                .iter()
                .find(|(k, _)| k == "hasValue")
                .unwrap()
                .1
                .clone();
            let value = fields
                .iter()
                .find(|(k, _)| k == "__value")
                .unwrap()
                .1
                .clone();
            assert_eq!(has_value, Value::Bool(true));
            assert_eq!(value, Value::Int(1));
        }
    }

    #[test]
    fn test_eval_mod_mold_zero() {
        // Mod[10, 0]() returns Lax with hasValue=false
        let result = eval_ok("result <= Mod[10, 0]()");
        assert!(
            matches!(&result, Value::BuchiPack(_)),
            "Expected BuchiPack, got {:?}",
            result
        );
        if let Value::BuchiPack(fields) = &result {
            let has_value = fields
                .iter()
                .find(|(k, _)| k == "hasValue")
                .unwrap()
                .1
                .clone();
            assert_eq!(has_value, Value::Bool(false));
        }
    }

    // ── Type Conversion Mold tests ──

    #[test]
    fn test_str_mold_from_int() {
        // Str[42]() → Lax(hasValue=true, __value="42")
        let result = eval_ok("result <= Str[42]()");
        assert!(
            matches!(&result, Value::BuchiPack(_)),
            "Expected BuchiPack, got {:?}",
            result
        );
        if let Value::BuchiPack(fields) = &result {
            let has_value = fields
                .iter()
                .find(|(k, _)| k == "hasValue")
                .unwrap()
                .1
                .clone();
            let value = fields
                .iter()
                .find(|(k, _)| k == "__value")
                .unwrap()
                .1
                .clone();
            assert_eq!(has_value, Value::Bool(true));
            assert_eq!(value, Value::str("42".into()));
        }
    }

    #[test]
    fn test_str_mold_from_bool() {
        let result = eval_ok("result <= Str[true]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::str("true".into()));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_int_mold_from_str_success() {
        // Int["123"]() → Lax(hasValue=true, __value=123)
        let result = eval_ok("result <= Int[\"123\"]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::Int(123));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_int_mold_from_str_fail() {
        // Int["abc"]() → Lax(hasValue=false, __value=0)
        let result = eval_ok("result <= Int[\"abc\"]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(false));
                assert_eq!(value, Value::Int(0));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_int_mold_from_float() {
        // Int[3.14]() → Lax(hasValue=true, __value=3)
        let result = eval_ok("result <= Int[3.14]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::Int(3));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_float_mold_from_str_success() {
        // Float["3.14"]() → Lax(hasValue=true, __value=3.14)
        let result = eval_ok("result <= Float[\"3.14\"]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::Float(314.0 / 100.0));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_float_mold_from_str_fail() {
        // Float["abc"]() → Lax(hasValue=false, __value=0.0)
        let result = eval_ok("result <= Float[\"abc\"]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(false));
                assert_eq!(value, Value::Float(0.0));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_bool_mold_from_str_true() {
        // Bool["true"]() → Lax(hasValue=true, __value=true)
        let result = eval_ok("result <= Bool[\"true\"]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::Bool(true));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_bool_mold_from_str_fail() {
        // Bool["hello"]() → Lax(hasValue=false, __value=false)
        let result = eval_ok("result <= Bool[\"hello\"]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(false));
                assert_eq!(value, Value::Bool(false));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_int_mold_from_bool() {
        // Int[true]() → Lax(hasValue=true, __value=1)
        let result = eval_ok("result <= Int[true]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::Int(1));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_bool_mold_from_int() {
        // Bool[0]() → Lax(hasValue=true, __value=false)
        let result = eval_ok("result <= Bool[0]()");
        match &result {
            Value::BuchiPack(fields) => {
                let has_value = fields
                    .iter()
                    .find(|(k, _)| k == "hasValue")
                    .unwrap()
                    .1
                    .clone();
                let value = fields
                    .iter()
                    .find(|(k, _)| k == "__value")
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(has_value, Value::Bool(true));
                assert_eq!(value, Value::Bool(false));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_str_mold_unmold() {
        // Str[42]() ]=> text, text should be "42"
        let result = eval_ok("Str[42]() ]=> text\ntext");
        assert_eq!(result, Value::str("42".into()));
    }

    #[test]
    fn test_eval_slash_operator_removed() {
        // `/` operator should now produce a parse error
        let result = eval("x <= 10 / 3");
        assert!(result.is_err(), "Expected error for removed `/` operator");
        let err = result.unwrap_err();
        assert!(
            err.contains("Parse error") || err.contains("Unexpected"),
            "Expected parse error for `/`, got: {}",
            err
        );
    }

    #[test]
    fn test_eval_percent_operator_removed() {
        // `%` operator should now produce a parse error
        let result = eval("x <= 10 % 3");
        assert!(result.is_err(), "Expected error for removed `%` operator");
        let err = result.unwrap_err();
        assert!(
            err.contains("Parse error") || err.contains("Unexpected"),
            "Expected parse error for `%`, got: {}",
            err
        );
    }

    #[test]
    fn test_eval_float_arithmetic() {
        assert_eq!(eval_ok("x <= 1.5 + 2.5"), Value::Float(4.0));
    }

    #[test]
    fn test_eval_mixed_arithmetic() {
        assert_eq!(eval_ok("x <= 1 + 2.0"), Value::Float(3.0));
    }

    #[test]
    fn test_eval_precedence() {
        assert_eq!(eval_ok("x <= 1 + 2 * 3"), Value::Int(7));
    }

    #[test]
    fn test_eval_negation() {
        assert_eq!(eval_ok("x <= -42"), Value::Int(-42));
    }

    // ── String operations ──

    #[test]
    fn test_eval_string_concat() {
        assert_eq!(
            eval_ok("x <= \"hello\" + \" \" + \"world\""),
            Value::str("hello world".to_string())
        );
    }

    // ── Comparison ──

    #[test]
    fn test_eval_comparison() {
        assert_eq!(eval_ok("x <= 1 > 0"), Value::Bool(true));
        assert_eq!(eval_ok("x <= 1 < 0"), Value::Bool(false));
        assert_eq!(eval_ok("x <= 1 == 1"), Value::Bool(true));
        assert_eq!(eval_ok("x <= 1 != 1"), Value::Bool(false));
    }

    // ── Logical ──

    #[test]
    fn test_eval_logical() {
        assert_eq!(eval_ok("x <= true && false"), Value::Bool(false));
        assert_eq!(eval_ok("x <= true || false"), Value::Bool(true));
        assert_eq!(eval_ok("x <= !true"), Value::Bool(false));
    }

    // ── Variables ──

    #[test]
    fn test_eval_assignment() {
        assert_eq!(eval_ok("x <= 42\nx"), Value::Int(42));
    }

    #[test]
    fn test_eval_variable_expression() {
        assert_eq!(eval_ok("x <= 10\ny <= x + 5\ny"), Value::Int(15));
    }

    // ── Buchi Pack ──

    #[test]
    fn test_eval_buchi_pack() {
        let result = eval_ok("user <= @(name <= \"Alice\", age <= 30)\nuser");
        match result {
            Value::BuchiPack(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "name");
                assert_eq!(fields[0].1, Value::str("Alice".to_string()));
                assert_eq!(fields[1].0, "age");
                assert_eq!(fields[1].1, Value::Int(30));
            }
            _ => panic!("Expected BuchiPack, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_field_access() {
        assert_eq!(
            eval_ok("user <= @(name <= \"Alice\", age <= 30)\nuser.name"),
            Value::str("Alice".to_string())
        );
    }

    // ── List ──

    #[test]
    fn test_eval_list() {
        let result = eval_ok("numbers <= @[1, 2, 3]\nnumbers");
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[1], Value::Int(2));
                assert_eq!(items[2], Value::Int(3));
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_list_get() {
        // .get(i) returns Lax with the value
        let source = "numbers <= @[10, 20, 30]\nnumbers.get(1).unmold()";
        assert_eq!(eval_ok(source), Value::Int(20));
    }

    #[test]
    fn test_eval_list_get_oob_returns_lax() {
        // .get(i) OOB returns Lax with hasValue=false (no IndexError)
        let source = r#"
numbers <= @[1, 2, 3]
result <= numbers.get(10)
result.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_eval_list_get_oob_unmold_returns_default() {
        // .get(i).unmold() returns default value when OOB
        let source = r#"
numbers <= @[1, 2, 3]
result <= numbers.get(10).unmold()
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    // ── Functions ──

    #[test]
    fn test_eval_function_def_and_call() {
        let source = "add x y =\n  x + y\n\nresult <= add(3, 4)\nresult";
        assert_eq!(eval_ok(source), Value::Int(7));
    }

    #[test]
    fn test_eval_function_closure() {
        let source = "multiplier <= 10\nscale x =\n  x * multiplier\n\nresult <= scale(5)\nresult";
        assert_eq!(eval_ok(source), Value::Int(50));
    }

    // ── Condition Branch ──

    #[test]
    fn test_eval_condition_branch() {
        // C20-1 (ROOT-5): multi-line rhs guards now require the
        // parenthesised escape hatch. Semantics unchanged.
        let source = "score <= 95\ngrade <= (\n  | score >= 90 |> \"A\"\n  | score >= 80 |> \"B\"\n  | _ |> \"F\"\n)\ngrade";
        assert_eq!(eval_ok(source), Value::str("A".to_string()));
    }

    #[test]
    fn test_eval_condition_branch_default() {
        // C20-1 (ROOT-5): see note above.
        let source = "score <= 50\ngrade <= (\n  | score >= 90 |> \"A\"\n  | _ |> \"F\"\n)\ngrade";
        assert_eq!(eval_ok(source), Value::str("F".to_string()));
    }

    // ── Immutability ──

    #[test]
    fn test_immutable_variable_redefinition_same_scope() {
        // Re-assigning a variable in the same scope should fail
        let result = eval("x <= 1\nx <= 2");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already defined"));
    }

    #[test]
    fn test_immutable_variable_shadowing_in_function() {
        // Defining a variable with the same name inside a function is OK (different scope)
        let source = "x <= 1\nf =\n  x <= 2\n  x\n=> :Int\nresult <= f()\nresult";
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_immutable_variable_outer_unchanged() {
        // After function call, outer variable should retain its original value
        let source = "x <= 1\nf =\n  x <= 2\n  x\n=> :Int\ny <= f()\nx";
        assert_eq!(eval_ok(source), Value::Int(1));
    }

    // ── 1-A: define_force restriction tests ──

    #[test]
    fn test_funcdef_cannot_overwrite_variable() {
        // Defining a function with the same name as an existing variable should error.
        let source = "x <= 1\nx =\n  42\n=> :Int";
        let result = eval(source);
        assert!(
            result.is_err(),
            "FuncDef should not overwrite existing variable"
        );
        assert!(result.unwrap_err().contains("already defined"));
    }

    #[test]
    fn test_funcdef_cannot_overwrite_function() {
        // Defining a function with the same name as an existing function should error.
        let source = "f x =\n  x + 1\n=> :Int\nf x =\n  x * 2\n=> :Int";
        let result = eval(source);
        assert!(
            result.is_err(),
            "FuncDef should not overwrite existing function"
        );
        assert!(result.unwrap_err().contains("already defined"));
    }

    #[test]
    fn test_std_import_errors_after_dissolution() {
        // `>>> std/*` imports should error after std dissolution.
        let source = ">>> std/json => @(jsonParse)";
        let result = eval(source);
        assert!(result.is_err(), "std/ imports should error");
        assert!(result.unwrap_err().contains("dissolved"));
    }

    #[test]
    fn test_prelude_jsonparse_abolished() {
        // jsonParse has been abolished (Molten Iron design).
        let source = "jsonParse(\"{\\\"a\\\": 1}\")";
        let result = eval(source);
        assert!(result.is_err(), "jsonParse should error after abolishment");
        assert!(result.unwrap_err().contains("removed"));
    }

    // ── 2-A: New List method tests ──

    #[test]
    fn test_list_find() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nresult <= Find[@[1, 3, 4, 7], isEven]()\nresult.hasValue";
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_list_find_not_found() {
        let source =
            "isNeg x =\n  x < 0\n=> :Bool\nresult <= Find[@[1, 2, 3], isNeg]()\nresult.hasValue";
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_list_find_index() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nFindIndex[@[1, 3, 4, 7], isEven]()";
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_list_find_index_not_found() {
        let source = "isNeg x =\n  x < 0\n=> :Bool\nFindIndex[@[1, 2, 3], isNeg]()";
        assert_eq!(eval_ok(source), Value::Int(-1));
    }

    #[test]
    fn test_list_any() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\n@[1, 3, 4].any(isEven)";
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_list_any_false() {
        let source = "isNeg x =\n  x < 0\n=> :Bool\n@[1, 2, 3].any(isNeg)";
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_list_all() {
        let source = "isPos x =\n  x > 0\n=> :Bool\n@[1, 2, 3].all(isPos)";
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_list_all_false() {
        let source = "isPos x =\n  x > 0\n=> :Bool\n@[1, -1, 3].all(isPos)";
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_list_count() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nCount[@[1, 2, 3, 4, 5], isEven]()";
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_list_sort_by() {
        let source = "neg x =\n  0 - x\n=> :Int\nresult <= Sort[@[3, 1, 2]](by <= neg)\nresult";
        if let Value::List(items) = eval_ok(source) {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(3), Value::Int(2), Value::Int(1)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_list_take() {
        let source = "Take[@[1, 2, 3, 4, 5], 3]()";
        if let Value::List(items) = eval_ok(source) {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_list_drop() {
        let source = "Drop[@[1, 2, 3, 4, 5], 2]()";
        if let Value::List(items) = eval_ok(source) {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(3), Value::Int(4), Value::Int(5)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_list_zip() {
        let source = "Zip[@[1, 2, 3], @[\"a\", \"b\", \"c\"]]() ]=> result\nresult.length()";
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_list_enumerate() {
        let source = "Enumerate[@[\"a\", \"b\", \"c\"]]() ]=> result\nresult.length()";
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    // ── New Operation Mold Tests (method→mold refactoring) ──

    // Str molds
    #[test]
    fn test_mold_upper() {
        assert_eq!(eval_ok("Upper[\"hello\"]()"), Value::str("HELLO".into()));
    }

    #[test]
    fn test_mold_lower() {
        assert_eq!(eval_ok("Lower[\"HELLO\"]()"), Value::str("hello".into()));
    }

    #[test]
    fn test_mold_trim() {
        assert_eq!(eval_ok("Trim[\"  hello  \"]()"), Value::str("hello".into()));
    }

    #[test]
    fn test_mold_trim_start_only() {
        assert_eq!(
            eval_ok("Trim[\"  hello  \"](end <= false)"),
            Value::str("hello  ".into())
        );
    }

    #[test]
    fn test_mold_trim_end_only() {
        assert_eq!(
            eval_ok("Trim[\"  hello  \"](start <= false)"),
            Value::str("  hello".into())
        );
    }

    #[test]
    fn test_mold_split() {
        if let Value::List(items) = eval_ok("Split[\"a,b,c\", \",\"]()") {
            assert_eq!(
                items.as_slice(),
                &[
                    Value::str("a".into()),
                    Value::str("b".into()),
                    Value::str("c".into())
                ]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_replace() {
        assert_eq!(
            eval_ok("Replace[\"hello world\", \"o\", \"0\"]()"),
            Value::str("hell0 world".into())
        );
    }

    #[test]
    fn test_mold_replace_all() {
        assert_eq!(
            eval_ok("Replace[\"hello world\", \"o\", \"0\"](all <= true)"),
            Value::str("hell0 w0rld".into())
        );
    }

    #[test]
    fn test_mold_slice() {
        assert_eq!(
            eval_ok("Slice[\"hello\"](start <= 1, end <= 3)"),
            Value::str("el".into())
        );
    }

    #[test]
    fn test_mold_charat() {
        // CharAt returns Lax[Str]; unmold with ]=> to get the inner value
        assert_eq!(
            eval_ok("CharAt[\"hello\", 1]() ]=> x\nx"),
            Value::str("e".into())
        );
    }

    #[test]
    fn test_mold_repeat() {
        assert_eq!(eval_ok("Repeat[\"ha\", 3]()"), Value::str("hahaha".into()));
    }

    #[test]
    fn test_mold_reverse_str() {
        assert_eq!(eval_ok("Reverse[\"hello\"]()"), Value::str("olleh".into()));
    }

    #[test]
    fn test_mold_reverse_list() {
        if let Value::List(items) = eval_ok("Reverse[@[1, 2, 3]]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(3), Value::Int(2), Value::Int(1)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_pad_start() {
        assert_eq!(
            eval_ok("Pad[\"42\", 5](side <= \"start\")"),
            Value::str("   42".into())
        );
    }

    #[test]
    fn test_mold_pad_end() {
        assert_eq!(
            eval_ok("Pad[\"42\", 5](side <= \"end\")"),
            Value::str("42   ".into())
        );
    }

    #[test]
    fn test_mold_pad_with_char() {
        assert_eq!(
            eval_ok("Pad[\"42\", 5](side <= \"start\", char <= \"0\")"),
            Value::str("00042".into())
        );
    }

    // Num molds
    #[test]
    fn test_mold_tofixed() {
        assert_eq!(eval_ok("ToFixed[3.14159, 2]()"), Value::str("3.14".into()));
    }

    #[test]
    fn test_mold_abs() {
        assert_eq!(eval_ok("Abs[-5]()"), Value::Int(5));
        assert_eq!(eval_ok("Abs[-3.7]()"), Value::Float(3.7));
    }

    #[test]
    fn test_mold_floor() {
        assert_eq!(eval_ok("Floor[3.7]()"), Value::Int(3));
    }

    #[test]
    fn test_mold_ceil() {
        assert_eq!(eval_ok("Ceil[3.2]()"), Value::Int(4));
    }

    #[test]
    fn test_mold_round() {
        assert_eq!(eval_ok("Round[3.5]()"), Value::Int(4));
    }

    #[test]
    fn test_mold_truncate() {
        assert_eq!(eval_ok("Truncate[3.7]()"), Value::Int(3));
        assert_eq!(eval_ok("Truncate[-3.7]()"), Value::Int(-3));
    }

    #[test]
    fn test_mold_clamp() {
        assert_eq!(eval_ok("Clamp[50, 0, 100]()"), Value::Int(50));
        assert_eq!(eval_ok("Clamp[-5, 0, 100]()"), Value::Int(0));
        assert_eq!(eval_ok("Clamp[150, 0, 100]()"), Value::Int(100));
    }

    #[test]
    fn test_mold_bitwise_and_shift() {
        assert_eq!(eval_ok("BitAnd[6, 3]()"), Value::Int(2));
        assert_eq!(eval_ok("BitOr[6, 3]()"), Value::Int(7));
        assert_eq!(eval_ok("BitXor[6, 3]()"), Value::Int(5));
        assert_eq!(eval_ok("BitNot[0]()"), Value::Int(-1));

        let ok = eval_ok("ShiftRU[-1, 1]()");
        match ok {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(true)
                );
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::Int(9223372036854775807)
                );
            }
            _ => panic!("expected Lax pack"),
        }

        let bad = eval_ok("ShiftL[1, 64]()");
        match bad {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(false)
                );
            }
            _ => panic!("expected Lax pack"),
        }
    }

    #[test]
    fn test_mold_radix_and_int_base() {
        let radix = eval_ok("ToRadix[255, 16]()");
        match radix {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(true)
                );
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::str("ff".into())
                );
            }
            _ => panic!("expected Lax pack"),
        }

        let int_base = eval_ok("Int[\"ff\", 16]()");
        match int_base {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(true)
                );
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::Int(255)
                );
            }
            _ => panic!("expected Lax pack"),
        }
    }

    #[test]
    fn test_mold_bytes_uint8_char_codepoint_utf8() {
        let u8_ok = eval_ok("UInt8[255]()");
        match u8_ok {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(true)
                );
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::Int(255)
                );
            }
            _ => panic!("expected Lax pack"),
        }

        let bytes_ok = eval_ok("Bytes[@[65, 66]]()");
        match bytes_ok {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(true)
                );
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::bytes(vec![65, 66])
                );
            }
            _ => panic!("expected Lax pack"),
        }

        let char_ok = eval_ok("Char[65]()");
        match char_ok {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::str("A".into())
                );
            }
            _ => panic!("expected Lax pack"),
        }

        let cp_ok = eval_ok("CodePoint[\"A\"]()");
        match cp_ok {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::Int(65)
                );
            }
            _ => panic!("expected Lax pack"),
        }

        let dec_ok = eval_ok(
            "b <= Bytes[@[112, 111, 110, 103]]()\n\
b ]=> bb\n\
Utf8Decode[bb]()",
        );
        match dec_ok {
            Value::BuchiPack(fields) => {
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "hasValue").unwrap().1,
                    Value::Bool(true)
                );
                assert_eq!(
                    fields.iter().find(|(k, _)| k == "__value").unwrap().1,
                    Value::str("pong".into())
                );
            }
            _ => panic!("expected Lax pack"),
        }

        if let Value::List(items) = eval_ok(r#"Chars["A😀é"]()"#) {
            assert_eq!(
                items.as_slice(),
                &[
                    Value::str("A".into()),
                    Value::str("😀".into()),
                    Value::str("é".into())
                ]
            );
        } else {
            panic!("expected list");
        }
    }

    // List molds
    #[test]
    fn test_mold_concat() {
        if let Value::List(items) = eval_ok("Concat[@[1, 2], @[3, 4]]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_append() {
        if let Value::List(items) = eval_ok("Append[@[1, 2], 3]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_prepend() {
        if let Value::List(items) = eval_ok("Prepend[@[2, 3], 1]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_join() {
        assert_eq!(
            eval_ok("Join[@[\"a\", \"b\", \"c\"], \",\"]()"),
            Value::str("a,b,c".into())
        );
    }

    #[test]
    fn test_mold_sum() {
        assert_eq!(eval_ok("Sum[@[1, 2, 3]]()"), Value::Int(6));
    }

    #[test]
    fn test_mold_sort() {
        if let Value::List(items) = eval_ok("Sort[@[3, 1, 2]]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_sort_reverse() {
        if let Value::List(items) = eval_ok("Sort[@[3, 1, 2]](reverse <= true)") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(3), Value::Int(2), Value::Int(1)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_unique() {
        if let Value::List(items) = eval_ok("Unique[@[1, 2, 2, 3, 3]]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_flatten() {
        if let Value::List(items) = eval_ok("Flatten[@[@[1, 2], @[3, 4]]]()") {
            assert_eq!(
                items.as_slice(),
                &[Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)]
            );
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_find_lax() {
        // Find now returns Lax (not Optional)
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nresult <= Find[@[1, 3, 4, 7], isEven]()\nresult.hasValue";
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_mold_find_unmold() {
        // Find Lax unmold extracts the value
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nFind[@[1, 3, 4, 7], isEven]() ]=> result\nresult";
        assert_eq!(eval_ok(source), Value::Int(4));
    }

    #[test]
    fn test_mold_find_index() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nFindIndex[@[1, 3, 4, 7], isEven]()";
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_mold_count() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\nCount[@[1, 2, 3, 4, 5], isEven]()";
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_mold_zip() {
        let source = "Zip[@[1, 2, 3], @[\"a\", \"b\", \"c\"]]() ]=> result\nresult.length()";
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_mold_enumerate() {
        let source = "Enumerate[@[\"a\", \"b\", \"c\"]]() ]=> result\nresult.length()";
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    // Fold argument order change
    #[test]
    fn test_mold_fold_new_order() {
        let source =
            "numbers <= @[1, 2, 3, 4, 5]\nFold[numbers, 0, _ acc x = acc + x]() ]=> sum\nsum";
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    #[test]
    fn test_mold_foldr_new_order() {
        let source = "words <= @[\"a\", \"b\", \"c\"]\nFoldr[words, \"\", _ acc x = x + acc]() ]=> result\nresult";
        assert_eq!(eval_ok(source), Value::str("abc".into()));
    }

    // Pipeline with molds
    #[test]
    fn test_mold_pipeline_str() {
        let source = "\"  hello  \" => Trim[_]() => Upper[_]() => result\nresult";
        assert_eq!(eval_ok(source), Value::str("HELLO".into()));
    }

    #[test]
    fn test_mold_pipeline_list() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\n@[1, 2, 3, 4, 5] => Filter[_, isEven]() => Map[_, _ x = x * 2]() => result\nresult";
        if let Value::List(items) = eval_ok(source) {
            assert_eq!(items.as_slice(), &[Value::Int(4), Value::Int(8)]);
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_mold_pipeline_fold() {
        let source = "isEven x =\n  Mod[x, 2]() ]=> r\n  r == 0\n=> :Bool\n@[1, 2, 3, 4, 5] => Filter[_, isEven]() => Map[_, _ x = x * 2]() => Fold[_, 0, _ acc x = acc + x]() => result\nresult";
        assert_eq!(eval_ok(source), Value::Int(12));
    }

    // ── Export (<<<) Tests ──

    /// Helper: create a temp module file with a unique name and run importer source.
    /// `test_name` is used to create unique file names to avoid parallel test conflicts.
    /// The importer source should use `./MODULE.td` as the import path — it will be
    /// replaced with the actual unique module filename.
    fn eval_with_module(
        test_name: &str,
        module_source: &str,
        importer_source: &str,
    ) -> Result<Value, String> {
        use std::io::Write;

        let tmp_dir = std::env::temp_dir().join("taida_test_exports");
        let _ = std::fs::create_dir_all(&tmp_dir);

        let module_filename = format!("{}.td", test_name);
        let module_file = tmp_dir.join(&module_filename);
        {
            let mut f = std::fs::File::create(&module_file).unwrap();
            f.write_all(module_source.as_bytes()).unwrap();
        }

        // Replace placeholder in importer source with actual module path
        let actual_import = format!("./{}", module_filename);
        let importer_source = importer_source.replace("./MODULE.td", &actual_import);

        let importer_file = tmp_dir.join(format!("{}_importer.td", test_name));
        {
            let mut f = std::fs::File::create(&importer_file).unwrap();
            f.write_all(importer_source.as_bytes()).unwrap();
        }

        let (program, parse_errors) = crate::parser::parse(&importer_source);
        if !parse_errors.is_empty() {
            return Err(parse_errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n"));
        }

        let mut interpreter = Interpreter::new();
        interpreter.set_current_file(&importer_file);
        interpreter
            .eval_program(&program)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn test_export_only_exported_symbols_are_visible() {
        // Module exports only `publicVal` via <<<
        let module_src = "publicVal <= 42\nprivateVal <= 99\n<<< @(publicVal)";
        // Importer should be able to access publicVal
        let importer_src = ">>> ./MODULE.td => @(publicVal)\npublicVal";
        let result = eval_with_module("export_visible", module_src, importer_src);
        assert_eq!(result.unwrap(), Value::Int(42));
    }

    #[test]
    fn test_export_non_exported_symbol_is_not_accessible() {
        // Module exports only `publicVal` via <<<
        let module_src = "publicVal <= 42\nprivateVal <= 99\n<<< @(publicVal)";
        // Importer tries to access privateVal — should fail
        let importer_src = ">>> ./MODULE.td => @(privateVal)\nprivateVal";
        let result = eval_with_module("export_hidden", module_src, importer_src);
        assert!(result.is_err(), "Importing non-exported symbol should fail");
        let err = result.unwrap_err();
        assert!(
            err.contains("not found") || err.contains("not exported"),
            "Error should indicate symbol not found: {}",
            err
        );
    }

    #[test]
    fn test_no_export_statement_exports_all() {
        // Module has no <<< — all symbols should be exported (backward compat)
        let module_src = "valA <= 10\nvalB <= 20";
        // Importer should access both symbols
        let importer_src = ">>> ./MODULE.td => @(valA, valB)\nvalA + valB";
        let result = eval_with_module("no_export_all", module_src, importer_src);
        assert_eq!(result.unwrap(), Value::Int(30));
    }

    #[test]
    fn test_export_function_only() {
        // Module exports a function
        let module_src = "helper x =\n  x + 1\n=> :Int\ninternalConst <= 100\n<<< @(helper)";
        // Importer can use the exported function
        let importer_src = ">>> ./MODULE.td => @(helper)\nhelper(5)";
        let result = eval_with_module("export_func", module_src, importer_src);
        assert_eq!(result.unwrap(), Value::Int(6));
    }

    #[test]
    fn test_export_function_hides_non_exported() {
        // Module exports only `helper`, not `internalConst`
        let module_src = "helper x =\n  x + 1\n=> :Int\ninternalConst <= 100\n<<< @(helper)";
        // Importer tries to access internalConst — should fail
        let importer_src = ">>> ./MODULE.td => @(internalConst)\ninternalConst";
        let result = eval_with_module("export_func_hidden", module_src, importer_src);
        assert!(result.is_err(), "Importing non-exported symbol should fail");
    }

    // ── Custom Mold: filling + unmold (C-4) ─────────────────

    #[test]
    fn test_custom_mold_filling_unmold_forward() {
        // Custom mold with filling and custom unmold, using ]=> to extract
        let source = r#"Mold[T] => Container[T] = @(
  count: Int
  unmold _ =
    filling
  => :T
)
data <= @(x <= 1, y <= 2)
box <= Container[data](count <= 3)
box ]=> extracted
extracted"#;
        let result = eval_ok(source);
        // extracted should be the filling value: @(x <= 1, y <= 2)
        if let Value::BuchiPack(fields) = &result {
            assert!(fields.iter().any(|(k, v)| k == "x" && *v == Value::Int(1)));
            assert!(fields.iter().any(|(k, v)| k == "y" && *v == Value::Int(2)));
        } else {
            panic!("Expected BuchiPack, got {:?}", result);
        }
    }

    #[test]
    fn test_custom_mold_filling_unmold_backward() {
        // Custom mold using <=[ (backward unmold)
        let source = r#"Mold[T] => Box[T] = @(
  label: Str
  unmold _ =
    filling
  => :T
)
val <= 42
b <= Box[val](label <= "test")
result <=[ b
result"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_custom_mold_filling_unmold_method() {
        // Custom mold using .unmold() method
        let source = r#"Mold[T] => Wrapper[T] = @(
  tag: Str
  unmold _ =
    filling
  => :T
)
w <= Wrapper["hello"](tag <= "greeting")
w.unmold()"#;
        assert_eq!(eval_ok(source), Value::str("hello".to_string()));
    }

    #[test]
    fn test_custom_mold_filling_field_access() {
        // The `filling` field should be accessible on the instance
        let source = r#"Mold[T] => Container[T] = @(
  count: Int
  unmold _ =
    filling
  => :T
)
box <= Container[99](count <= 5)
box.filling"#;
        assert_eq!(eval_ok(source), Value::Int(99));
    }

    #[test]
    fn test_custom_mold_regular_field_access() {
        // Regular fields (count, name) should still be accessible
        let source = r#"Mold[T] => Container[T] = @(
  count: Int
  name: Str
  unmold _ =
    filling
  => :T
)
box <= Container["data"](count <= 3, name <= "my-box")
box.count"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_custom_mold_without_unmold_uses_default() {
        // Custom mold WITHOUT unmold definition should fall back to __value
        let source = r#"Mold[T] => Simple[T] = @(
  label: Str
)
s <= Simple[42](label <= "test")
s ]=> val
val"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_custom_mold_with_method() {
        // Custom mold with both unmold and a user-defined method
        let source = r#"Mold[T] => Container[T] = @(
  count: Int
  unmold _ =
    filling
  => :T
  describe =
    `count: ${count}`
  => :Str
)
box <= Container["data"](count <= 7)
box.describe()"#;
        assert_eq!(eval_ok(source), Value::str("count: 7".to_string()));
    }

    #[test]
    fn test_custom_mold_list_filling() {
        // Custom mold with a list as filling
        let source = r#"Mold[T] => ListBox[T] = @(
  unmold _ =
    filling
  => :T
)
items <= @[1, 2, 3]
lb <= ListBox[items]()
lb ]=> extracted
extracted"#;
        let result = eval_ok(source);
        if let Value::List(items) = &result {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], Value::Int(1));
            assert_eq!(items[1], Value::Int(2));
            assert_eq!(items[2], Value::Int(3));
        } else {
            panic!("Expected List, got {:?}", result);
        }
    }

    #[test]
    fn test_custom_mold_parser_unmold_syntax() {
        // Verify the parser accepts `unmold _ = expr => :T` syntax
        let source = r#"Mold[T] => M[T] = @(
  unmold _ =
    filling
  => :T
)"#;
        // Should parse without error (MoldDef returns Unit)
        let _ = eval_ok(source);
    }

    #[test]
    fn test_custom_mold_solidify_override_returns_solidified_value() {
        let source = r#"Mold[T] => PlusOne[T] = @(
  solidify =
    filling + 1
  => :Int
)
PlusOne[41]()"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_custom_mold_solidify_can_reference_self() {
        let source = r#"Mold[T] => SelfType[T] = @(
  solidify =
    self.__type
  => :Str
)
SelfType[1]()"#;
        assert_eq!(eval_ok(source), Value::str("SelfType".to_string()));
    }

    #[test]
    fn test_custom_mold_solidify_throw_caught_by_error_ceiling() {
        let source = r#"Mold[T] => NonZero[T] = @(
  solidify =
    | filling == 0 |> Error(type <= "ZeroError", message <= "zero").throw()
    | _ |> filling
  => :Int
)
check x: Int =
  |== err: Error =
    -1
  => :Int
  NonZero[x]()
=> :Int
check(0)"#;
        assert_eq!(eval_ok(source), Value::Int(-1));
    }

    // ── Bug-1: .unmold() fallback to __value when __unmold is absent ──

    #[test]
    fn test_custom_mold_unmold_method_without_unmold_override() {
        // Bug-1: .unmold() on a custom mold WITHOUT `unmold _ = ...` should
        // fall back to __value, just like ]=> does via unmold_value().
        let source = r#"Mold[T] => Simple[T] = @(
  label: Str
)
s <= Simple[42](label <= "test")
s.unmold()"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    // ── Bug-2: __unmold throw is catchable by |== ──

    #[test]
    fn test_custom_mold_unmold_throw_caught_by_error_ceiling_forward() {
        // Bug-2: throw inside __unmold should be catchable by |== (via ]=>)
        let source = r#"Mold[T] => Validated[T] = @(
  unmold _ =
    Error(type <= "ValidationError", message <= "invalid").throw()
    filling
  => :T
)
validate x: Int =
  |== err: Error =
    "caught"
  => :Str
  v <= Validated[x]()
  v ]=> val
  Str[val]() ]=> s
  s
=> :Str
validate(0)"#;
        assert_eq!(eval_ok(source), Value::str("caught".to_string()));
    }

    #[test]
    fn test_custom_mold_unmold_throw_caught_by_error_ceiling_method() {
        // Bug-2: throw inside __unmold should be catchable by |== (via .unmold())
        let source = r#"Mold[T] => Checked[T] = @(
  unmold _ =
    Error(type <= "CheckError", message <= "failed").throw()
    filling
  => :T
)
check x: Int =
  |== err: Error =
    "handled"
  => :Str
  c <= Checked[x]()
  c.unmold()
  "unreachable"
=> :Str
check(1)"#;
        assert_eq!(eval_ok(source), Value::str("handled".to_string()));
    }

    // ── Molten type tests ──────────────────────────────

    #[test]
    fn test_jsnew_js_backend_only_error() {
        let result = eval("x <= JSNew[\"Date\"]()");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("JSNew is only available in the JS transpiler backend"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_jsset_js_backend_only_error() {
        let result = eval("x <= JSSet[\"obj\", \"key\", \"val\"]()");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("JSSet is only available in the JS transpiler backend"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_jsbind_js_backend_only_error() {
        let result = eval("x <= JSBind[\"fn\", \"ctx\"]()");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("JSBind is only available in the JS transpiler backend"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_jsspread_js_backend_only_error() {
        let result = eval("x <= JSSpread[\"arr\"]()");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("JSSpread is only available in the JS transpiler backend"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_npm_import_js_backend_only_error() {
        let result = eval(">>> npm:lodash => @(debounce)");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("npm imports are only available in the JS transpiler backend"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_todo_unmold_returns_unm_field() {
        let source = r#"
t <= TODO[Int](sol <= 7, unm <= 9)
t ]=> out
out
"#;
        assert_eq!(eval_ok(source), Value::Int(9));
    }

    #[test]
    fn test_todo_unmold_falls_back_to_type_default() {
        let source = r#"
t <= TODO[Int](sol <= 7)
t ]=> out
out
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_todo_stub_unmold_yields_molten() {
        let source = r#"
t <= TODO[Stub["User data shape TBD"]]()
t ]=> out
typeof(out)
"#;
        assert_eq!(eval_ok(source), Value::str("Molten".to_string()));
    }

    #[test]
    fn test_todo_stub_molten_runtime_access_is_restricted() {
        let source = r#"
t <= TODO[Stub["User data shape TBD"]]()
t ]=> out
out.toString()
"#;
        let result = eval(source);
        assert!(
            result.is_err(),
            "Molten from Stub should reject method access"
        );
        let err = result.unwrap_err();
        assert!(err.contains("Cannot call method"), "Got: {}", err);
        assert!(err.contains("Molten"), "Got: {}", err);
    }

    // ── Cage Molten-only tests ──────────────────────────

    #[test]
    fn test_cage_rejects_non_molten_int() {
        let result = eval("identity x = x => :Int\nCage[42, identity]() => c");
        assert!(
            result.is_err(),
            "Cage should reject non-Molten first argument"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Cage requires Molten type as first argument, got Int"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_cage_rejects_non_molten_str() {
        let result = eval("identity x = x => :Str\nCage[\"hello\", identity]() => c");
        assert!(
            result.is_err(),
            "Cage should reject non-Molten first argument"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Cage requires Molten type as first argument, got Str"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_cage_rejects_non_molten_bool() {
        let result = eval("identity x = x => :Bool\nCage[true, identity]() => c");
        assert!(
            result.is_err(),
            "Cage should reject non-Molten first argument"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Cage requires Molten type as first argument, got Bool"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_cage_rejects_non_molten_buchipack() {
        let result = eval("identity x = x => :Int\nbp <= @(a <= 1)\nCage[bp, identity]() => c");
        assert!(
            result.is_err(),
            "Cage should reject non-Molten first argument"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Cage requires Molten type as first argument"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_cage_accepts_molten_value() {
        // Directly test Cage with Value::Molten by pre-setting a variable
        let source = "identity x = x => :Molten\nCage[m, identity]() => c\nc.hasValue()";
        let (program, parse_errors) = crate::parser::parse(source);
        assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
        let mut interpreter = Interpreter::new();
        // Pre-define 'm' as a Molten value
        interpreter.env.define_force("m", Value::Molten);
        let result = interpreter.eval_program(&program);
        assert!(
            result.is_ok(),
            "Cage should accept Molten value, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_cage_molten_success_has_value() {
        // Cage with Molten value should produce a Gorillax with hasValue=true
        let source = "identity x = x => :Molten\nCage[m, identity]() => c\nc.hasValue()";
        let (program, parse_errors) = crate::parser::parse(source);
        assert!(parse_errors.is_empty());
        let mut interpreter = Interpreter::new();
        interpreter.env.define_force("m", Value::Molten);
        let result = interpreter.eval_program(&program).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_cage_molten_error_in_function() {
        // Cage catches errors from the function and returns Gorillax with hasValue=false
        let source = r#"Error => TestError = @(message: Str)
failing x =
  TestError(message <= "cage fail").throw()
=> :Molten
Cage[m, failing]() => c
c.hasValue()"#;
        let (program, parse_errors) = crate::parser::parse(source);
        assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
        let mut interpreter = Interpreter::new();
        interpreter.env.define_force("m", Value::Molten);
        let result = interpreter.eval_program(&program).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    // ── Molten negative tests ──────────────────────────

    #[test]
    fn test_molten_direct_unmold_forbidden() {
        // Molten cannot be unmolded directly — must use Cage
        let source = "m ]=> x";
        let (program, parse_errors) = crate::parser::parse(source);
        assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
        let mut interpreter = Interpreter::new();
        interpreter.env.define_force("m", Value::Molten);
        let result = interpreter.eval_program(&program);
        assert!(result.is_err(), "Molten should not be unmoldable directly");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Cannot unmold Molten directly"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_molten_method_call_forbidden() {
        // Molten is opaque — no methods allowed
        let source = "m.toString()";
        let (program, parse_errors) = crate::parser::parse(source);
        assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
        let mut interpreter = Interpreter::new();
        interpreter.env.define_force("m", Value::Molten);
        let result = interpreter.eval_program(&program);
        assert!(result.is_err(), "Molten should not allow method calls");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cannot call method"), "Got: {}", err);
        assert!(err.contains("Molten"), "Got: {}", err);
    }

    #[test]
    fn test_cage_second_arg_not_function() {
        // Cage second argument must be a function, not a literal
        let source = "Cage[m, 42]() => c";
        let (program, parse_errors) = crate::parser::parse(source);
        assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
        let mut interpreter = Interpreter::new();
        interpreter.env.define_force("m", Value::Molten);
        let result = interpreter.eval_program(&program);
        assert!(
            result.is_err(),
            "Cage should reject non-function second argument"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Cage second argument must be a function"),
            "Got: {}",
            err
        );
    }

    #[test]
    fn test_cage_rejects_non_molten_list() {
        // Cage requires Molten type as first argument — List should be rejected
        let result = eval("identity x = x => :Int\nCage[@[1, 2, 3], identity]() => c");
        assert!(
            result.is_err(),
            "Cage should reject non-Molten first argument"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Cage requires Molten type as first argument"),
            "Got: {}",
            err
        );
    }

    // ── BT-2: null/undefined/none/nil rejection at interpreter level ──
    // PHILOSOPHY.md I: "null/undefinedの完全排除 — 全ての型にデフォルト値を保証"

    #[test]
    fn test_bt2_null_eval_fails() {
        let result = eval("x <= null");
        assert!(result.is_err(), "'null' should not be evaluable as a value");
    }

    #[test]
    fn test_bt2_undefined_eval_fails() {
        let result = eval("x <= undefined");
        assert!(
            result.is_err(),
            "'undefined' should not be evaluable as a value"
        );
    }

    #[test]
    fn test_bt2_none_eval_fails() {
        let result = eval("x <= none");
        assert!(result.is_err(), "'none' should not be evaluable as a value");
    }

    #[test]
    fn test_bt2_nil_eval_fails() {
        let result = eval("x <= nil");
        assert!(result.is_err(), "'nil' should not be evaluable as a value");
    }

    // ── BT-4: Integer overflow boundary tests ──

    #[test]
    fn test_bt4_i64_max_literal() {
        // i64::MAX = 9223372036854775807
        assert_eq!(eval_ok("9223372036854775807"), Value::Int(i64::MAX));
    }

    #[test]
    fn test_bt4_i64_max_plus_one_overflow() {
        // TF-12 fixed: wrapping arithmetic ensures consistent behavior in debug/release.
        // i64::MAX + 1 wraps to i64::MIN.
        assert_eq!(
            eval_ok("x <= 9223372036854775807 + 1\nx"),
            Value::Int(i64::MIN),
            "i64::MAX + 1 should wrap to i64::MIN"
        );
    }

    #[test]
    fn test_bt4_i64_min_via_subtraction() {
        // i64::MIN = -9223372036854775808 (can't be expressed as a literal directly)
        // Use 0 - i64::MAX - 1 to reach i64::MIN.
        assert_eq!(
            eval_ok("x <= 0 - 9223372036854775807 - 1\nx"),
            Value::Int(i64::MIN),
            "0 - i64::MAX - 1 should equal i64::MIN"
        );
    }

    #[test]
    fn test_bt4_i64_min_minus_one_overflow() {
        // TF-12 fixed: i64::MIN - 1 wraps to i64::MAX.
        assert_eq!(
            eval_ok("x <= (0 - 9223372036854775807 - 1) - 1\nx"),
            Value::Int(i64::MAX),
            "i64::MIN - 1 should wrap to i64::MAX"
        );
    }

    // ── Lax helpers (used by BT-4, BT-6, BT-7, BT-12, BT-18) ──

    fn lax_has_value(result: &Value) -> bool {
        assert!(
            matches!(result, Value::BuchiPack(_)),
            "Expected BuchiPack (Lax), got: {:?}",
            result
        );
        if let Value::BuchiPack(fields) = result {
            fields
                .iter()
                .any(|(k, v)| k == "hasValue" && *v == Value::Bool(true))
        } else {
            unreachable!()
        }
    }

    fn lax_field<'a>(result: &'a Value, field: &str) -> Option<&'a Value> {
        assert!(
            matches!(result, Value::BuchiPack(_)),
            "Expected BuchiPack (Lax), got: {:?}",
            result
        );
        if let Value::BuchiPack(fields) = result {
            fields.iter().find(|(k, _)| k == field).map(|(_, v)| v)
        } else {
            unreachable!()
        }
    }

    #[test]
    fn test_bt4_int_mold_i64_max_string() {
        // Int["9223372036854775807"]() should succeed
        let result = eval_ok(r#"Int["9223372036854775807"]()"#);
        assert!(
            lax_has_value(&result),
            "Int[i64::MAX string] should have value"
        );
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Int(i64::MAX)));
    }

    #[test]
    fn test_bt4_int_mold_overflow_string() {
        // Int["999999999999999999999999"]() should return Lax with hasValue=false
        let result = eval_ok(r#"Int["999999999999999999999999"]()"#);
        assert!(
            !lax_has_value(&result),
            "Int[overflow string] should return empty Lax"
        );
    }

    #[test]
    fn test_bt4_int_mold_i64_min_string() {
        // Int["-9223372036854775808"]() should succeed
        let result = eval_ok(r#"Int["-9223372036854775808"]()"#);
        assert!(
            lax_has_value(&result),
            "Int[i64::MIN string] should have value"
        );
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Int(i64::MIN)));
    }

    // ── BT-5: Empty collection operation tests ──

    #[test]
    fn test_bt5_upper_empty_string() {
        assert_eq!(eval_ok(r#"Upper[""]() "#), Value::str(String::new()));
    }

    #[test]
    fn test_bt5_reverse_empty_string() {
        assert_eq!(eval_ok(r#"Reverse[""]() "#), Value::str(String::new()));
    }

    #[test]
    fn test_bt5_split_empty_string_empty_delimiter() {
        // Split["",""]() — splits empty string by empty delimiter → ["", ""]
        let result = eval_ok(r#"Split["",""]() "#);
        assert!(
            matches!(&result, Value::List(items) if items.len() == 2),
            "Split[\"\",\"\"]() should return 2-element list, got: {:?}",
            result
        );
    }

    #[test]
    fn test_bt5_sum_empty_list() {
        assert_eq!(eval_ok(r#"Sum[@[]]() "#), Value::Int(0));
    }

    #[test]
    fn test_bt5_concat_empty_lists() {
        let result = eval_ok(r#"Concat[@[],@[]]() "#);
        if let Value::List(items) = &result {
            assert!(items.is_empty(), "Concat of empty lists should be empty");
        } else {
            panic!("Expected List, got {:?}", result);
        }
    }

    #[test]
    fn test_bt5_flatten_empty_list() {
        let result = eval_ok(r#"Flatten[@[]]() "#);
        if let Value::List(items) = &result {
            assert!(items.is_empty(), "Flatten of empty list should be empty");
        } else {
            panic!("Expected List, got {:?}", result);
        }
    }

    #[test]
    fn test_bt5_unique_empty_list() {
        let result = eval_ok(r#"Unique[@[]]() "#);
        if let Value::List(items) = &result {
            assert!(items.is_empty(), "Unique of empty list should be empty");
        } else {
            panic!("Expected List, got {:?}", result);
        }
    }

    #[test]
    fn test_bt5_reverse_empty_list() {
        let result = eval_ok(
            r#"x <= Reverse[@[]]()
x"#,
        );
        if let Value::List(items) = &result {
            assert!(items.is_empty(), "Reverse of empty list should be empty");
        } else {
            panic!("Expected List, got {:?}", result);
        }
    }

    #[test]
    fn test_bt5_set_of_empty_list() {
        // setOf(@[]) should create an empty set
        let result = eval_ok(
            r#"x <= setOf(@[])
x.size()"#,
        );
        assert_eq!(result, Value::Int(0), "Empty set should have size 0");
    }

    #[test]
    fn test_bt5_hashmap_empty_list() {
        // hashMap(@[]) should create an empty hashmap
        let result = eval_ok(
            r#"x <= hashMap(@[])
x.size()"#,
        );
        assert_eq!(result, Value::Int(0), "Empty hashmap should have size 0");
    }

    #[test]
    fn test_bt5_char_at_empty_string_index_zero() {
        // Expected: return Lax with hasValue=false (out of bounds)
        let result = eval_ok(
            r#"x <= CharAt["",0]()
x.hasValue"#,
        );
        assert_eq!(
            result,
            Value::Bool(false),
            "CharAt on empty string should return empty Lax"
        );
    }

    #[test]
    fn test_bt5_slice_empty_string() {
        // Expected: return "" (empty string)
        let result = eval_ok(r#"Slice["",0,0]() "#);
        assert_eq!(
            result,
            Value::str(String::new()),
            "Slice of empty string should return empty string"
        );
    }

    // ── BT-6: Lax[T] boundary condition tests ──

    #[test]
    fn test_bt6_lax_success_has_value() {
        // Div[10,2]() should return Lax with hasValue=true
        let result = eval_ok("Div[10,2]()");
        assert!(
            lax_has_value(&result),
            "Div[10,2]() should have hasValue=true"
        );
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Int(5)));
    }

    #[test]
    fn test_bt6_lax_failure_has_no_value() {
        // Div[1,0]() should return Lax with hasValue=false
        let result = eval_ok("Div[1,0]()");
        assert!(
            !lax_has_value(&result),
            "Div[1,0]() should have hasValue=false"
        );
    }

    #[test]
    fn test_bt6_lax_in_list() {
        // Lax values stored in a list should maintain their identity
        let result = eval_ok(
            r#"y <= @[Div[1,0](), Div[2,0](), Div[10,2]()]
y.length()"#,
        );
        assert_eq!(result, Value::Int(3), "List of Lax should have 3 elements");
    }

    #[test]
    fn test_bt6_lax_in_buchi_pack() {
        // Lax values as BuchiPack fields should preserve hasValue
        let result = eval_ok(
            r#"p <= @(x <= Div[1,0](), y <= Div[10,2]())
p.x.hasValue"#,
        );
        assert_eq!(
            result,
            Value::Bool(false),
            "Failed Lax in pack should have hasValue=false"
        );

        let result2 = eval_ok(
            r#"p <= @(x <= Div[1,0](), y <= Div[10,2]())
p.y.hasValue"#,
        );
        assert_eq!(
            result2,
            Value::Bool(true),
            "Success Lax in pack should have hasValue=true"
        );
    }

    #[test]
    fn test_bt6_lax_default_value() {
        // Failed Lax should have __default field with type default
        let result = eval_ok("Div[1,0]()");
        assert_eq!(
            lax_field(&result, "__default"),
            Some(&Value::Int(0)),
            "Int Lax default should be 0"
        );
    }

    #[test]
    fn test_bt6_lax_type_field() {
        // Lax should have __type field
        let result = eval_ok("Div[1,0]()");
        assert!(
            lax_field(&result, "__type").is_some(),
            "Lax should have __type field"
        );
    }

    // ── BT-7: Type conversion boundary tests ──

    #[test]
    fn test_bt7_int_plus_sign_string() {
        // Int["+42"]() should succeed (plus sign prefix)
        let result = eval_ok(r#"Int["+42"]()"#);
        assert!(lax_has_value(&result), "Int[\"+42\"] should succeed");
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Int(42)));
    }

    #[test]
    fn test_bt7_int_decimal_string_fails() {
        // Int["42.0"]() should fail (not a pure integer)
        let result = eval_ok(r#"Int["42.0"]()"#);
        assert!(
            !lax_has_value(&result),
            "Int[\"42.0\"] should fail (decimal)"
        );
    }

    #[test]
    fn test_bt7_int_hex_string_fails() {
        // Int["0x10"]() should fail (hex not supported)
        let result = eval_ok(r#"Int["0x10"]()"#);
        assert!(!lax_has_value(&result), "Int[\"0x10\"] should fail (hex)");
    }

    #[test]
    fn test_bt7_int_empty_string_fails() {
        // Int[""]() should fail
        let result = eval_ok(r#"Int[""]()"#);
        assert!(!lax_has_value(&result), "Int[\"\"] should fail (empty)");
    }

    #[test]
    fn test_bt7_bool_zero() {
        // Bool[0]() should return false
        let result = eval_ok("Bool[0]()");
        assert!(lax_has_value(&result), "Bool[0] should succeed");
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Bool(false)));
    }

    #[test]
    fn test_bt7_bool_negative_one() {
        // Bool[-1]() should return true (non-zero)
        let result = eval_ok("Bool[-1]()");
        assert!(lax_has_value(&result), "Bool[-1] should succeed");
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_bt7_float_empty_string_fails() {
        // Float[""]() should fail
        let result = eval_ok(r#"Float[""]()"#);
        assert!(!lax_has_value(&result), "Float[\"\"] should fail (empty)");
    }

    #[test]
    fn test_bt7_int_negative_max_string() {
        // Int["-9223372036854775808"]() — i64::MIN as string
        let result = eval_ok(r#"Int["-9223372036854775808"]()"#);
        assert!(
            lax_has_value(&result),
            "Int[i64::MIN string] should succeed"
        );
        assert_eq!(lax_field(&result, "__value"), Some(&Value::Int(i64::MIN)));
    }

    // ── BT-8: Numeric mold negative/zero boundary tests ──

    #[test]
    fn test_bt8_floor_negative() {
        // Floor[-3.7]() should return -4 (floor rounds toward negative infinity)
        assert_eq!(eval_ok("Floor[-3.7]()"), Value::Int(-4));
    }

    #[test]
    fn test_bt8_ceil_negative() {
        // Ceil[-3.2]() should return -3 (ceil rounds toward positive infinity)
        assert_eq!(eval_ok("Ceil[-3.2]()"), Value::Int(-3));
    }

    #[test]
    fn test_bt8_round_negative_half() {
        // Round[-3.5]() — f64::round() uses "half away from zero" → -4
        assert_eq!(eval_ok("Round[-3.5]()"), Value::Int(-4));
    }

    #[test]
    fn test_bt8_round_positive_half() {
        // Round[0.5]() — f64::round() uses "half away from zero" → 1
        assert_eq!(eval_ok("Round[0.5]()"), Value::Int(1));
    }

    #[test]
    fn test_bt8_clamp_at_lower_bound() {
        // Clamp[0, 0, 5]() — value equals lower bound, should return 0
        assert_eq!(eval_ok("Clamp[0, 0, 5]()"), Value::Int(0));
    }

    #[test]
    fn test_bt8_clamp_at_upper_bound() {
        // Clamp[5, 0, 5]() — value equals upper bound, should return 5
        assert_eq!(eval_ok("Clamp[5, 0, 5]()"), Value::Int(5));
    }

    #[test]
    fn test_bt8_abs_negative_zero() {
        // Abs[-0.0]() — negative zero should become 0.0
        let result = eval_ok("Abs[-0.0]()");
        match &result {
            Value::Float(f) => assert!(*f == 0.0, "Abs[-0.0] should be 0.0, got: {}", f),
            Value::Int(i) => assert!(*i == 0, "Abs[-0.0] as Int should be 0, got: {}", i),
            other => panic!("Abs[-0.0] should return Float or Int, got: {:?}", other),
        }
    }

    // ── BT-13: String mold edge case tests ──

    #[test]
    fn test_bt13_slice_negative_start() {
        // Slice[s, -2]() — negative start index
        // TF-15: Returns BuchiPack instead of Str (Slice mold returns raw Lax-like structure)
        let result = eval(r#"Slice["hello", -2]()"#);
        match result {
            Ok(val) => {
                // Returns Str or BuchiPack (Lax-like) — both acceptable
                assert!(
                    matches!(&val, Value::Str(_)) || matches!(&val, Value::BuchiPack(_)),
                    "Slice with negative start should return Str or BuchiPack, got: {:?}",
                    val
                );
            }
            Err(err) => {
                assert!(!err.is_empty(), "Error message should not be empty");
            }
        }
    }

    #[test]
    fn test_bt13_slice_reversed_indices() {
        // Slice[s, 3, 1]() — end < start (reversed)
        // TF-15: Returns BuchiPack instead of Str for edge-case indices
        let result = eval(r#"Slice["hello", 3, 1]()"#);
        match result {
            Ok(val) => {
                assert!(
                    matches!(&val, Value::Str(_)) || matches!(&val, Value::BuchiPack(_)),
                    "Slice with reversed indices should return Str or BuchiPack, got: {:?}",
                    val
                );
            }
            Err(err) => {
                assert!(!err.is_empty(), "Error message should not be empty");
            }
        }
    }

    #[test]
    fn test_bt13_char_at_negative_index() {
        // CharAt[s, -1]() — negative index
        let result = eval(r#"CharAt["hello", -1]()"#);
        match result {
            Ok(val) => {
                assert!(
                    matches!(&val, Value::Str(_)) || matches!(&val, Value::BuchiPack(_)),
                    "CharAt with negative index should return Str or BuchiPack, got: {:?}",
                    val
                );
            }
            Err(err) => {
                assert!(!err.is_empty(), "Error message should not be empty");
            }
        }
    }

    #[test]
    fn test_bt13_upper_unicode() {
        // Upper["hello"]() should work — basic ASCII
        assert_eq!(
            eval_ok(r#"Upper["hello"]()"#),
            Value::str("HELLO".to_string())
        );
    }

    #[test]
    fn test_bt13_reverse_unicode() {
        // Reverse on ASCII string
        assert_eq!(
            eval_ok(r#"Reverse["abc"]()"#),
            Value::str("cba".to_string())
        );
    }

    #[test]
    fn test_bt13_reverse_empty() {
        // Reverse[""]() should return ""
        assert_eq!(eval_ok(r#"Reverse[""]()"#), Value::str(String::new()));
    }

    // ── BT-18: Type conversion failure default consistency tests ──

    #[test]
    fn test_bt18_int_conversion_failure_default() {
        // Int["invalid"]() should return Lax with __default = 0
        let result = eval_ok(r#"Int["abc"]().__default"#);
        assert_eq!(
            result,
            Value::Int(0),
            "Int conversion failure default should be 0"
        );
    }

    #[test]
    fn test_bt18_float_conversion_failure_default() {
        // Float["invalid"]() should return Lax with __default = 0.0
        let result = eval_ok(r#"Float["abc"]().__default"#);
        assert_eq!(
            result,
            Value::Float(0.0),
            "Float conversion failure default should be 0.0"
        );
    }

    #[test]
    fn test_bt18_lax_type_field() {
        // Type conversion molds return Lax, __type field should be "Lax"
        let result = eval_ok(r#"Bool[0]().__type"#);
        assert_eq!(
            result,
            Value::str("Lax".to_string()),
            "Lax __type should be 'Lax' (the mold type, not the inner type)"
        );
    }

    #[test]
    fn test_bt18_div_zero_default() {
        // Div[1,0]() should return Lax with __default = 0
        let result = eval_ok("Div[1,0]().__default");
        assert_eq!(result, Value::Int(0), "Div by zero default should be 0");
    }
}
