/// Extended tests for the Taida interpreter.
///
/// Tests for: string methods, number methods, list methods, bool methods,
/// map/filter/fold, TCO, async, unmold, stdlib, partial application,
/// pipeline, HTTP, prelude functions (Optional, Result, HashMap, Set),
/// JSON, OOB errors, Div/Mod molds, and user-defined methods.
#[cfg(test)]
mod tests {
    use crate::interpreter::eval::eval;
    use crate::interpreter::value::{AsyncStatus, Value};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn eval_ok(source: &str) -> Value {
        eval(source).unwrap_or_else(|e| panic!("Eval failed: {}", e))
    }

    fn eval_with_output(source: &str) -> (Value, Vec<String>) {
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interpreter = crate::interpreter::eval::Interpreter::new();
        let val = interpreter
            .eval_program(&program)
            .unwrap_or_else(|e| panic!("Eval failed: {}", e));
        let output = interpreter.output.clone();
        (val, output)
    }

    #[test]
    fn test_function_closure_reuses_captured_env_arc() {
        let source = r#"
f0 x =
  x
=> :Int

f1 x =
  f0(x)
=> :Int

f2 x =
  f1(x)
=> :Int

0
"#;

        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interpreter = crate::interpreter::eval::Interpreter::new();
        let _ = interpreter
            .eval_program(&program)
            .unwrap_or_else(|e| panic!("Eval failed: {}", e));

        let f1 = match interpreter.env.get("f1") {
            Some(Value::Function(f)) => f.clone(),
            other => panic!("Expected f1 function, got {:?}", other),
        };
        let f2 = match interpreter.env.get("f2") {
            Some(Value::Function(f)) => f.clone(),
            other => panic!("Expected f2 function, got {:?}", other),
        };
        let captured_f1 = match f2.closure.get("f1") {
            Some(Value::Function(f)) => f,
            other => panic!(
                "Expected captured f1 function in f2 closure, got {:?}",
                other
            ),
        };

        assert!(
            Arc::ptr_eq(&f1.closure, &captured_f1.closure),
            "captured function closure should share Arc to avoid recursive deep clone"
        );
    }

    // ── String Methods (auto-mold) ──

    #[test]
    fn test_eval_string_length() {
        assert_eq!(eval_ok("x <= \"hello\".length()\nx"), Value::Int(5));
    }

    #[test]
    fn test_eval_string_upper() {
        assert_eq!(
            eval_ok("Upper[\"hello\"]() ]=> x\nx"),
            Value::str("HELLO".to_string())
        );
    }

    #[test]
    fn test_eval_string_lower() {
        assert_eq!(
            eval_ok("Lower[\"HELLO\"]() ]=> x\nx"),
            Value::str("hello".to_string())
        );
    }

    #[test]
    fn test_eval_string_trim() {
        assert_eq!(
            eval_ok("Trim[\"  hello  \"]() ]=> x\nx"),
            Value::str("hello".to_string())
        );
    }

    #[test]
    fn test_eval_string_contains() {
        assert_eq!(
            eval_ok("x <= \"hello world\".contains(\"world\")\nx"),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_eval_string_split() {
        let result = eval_ok("Split[\"a,b,c\", \",\"]() ]=> x\nx");
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::str("a".to_string()));
                assert_eq!(items[1], Value::str("b".to_string()));
                assert_eq!(items[2], Value::str("c".to_string()));
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_string_replace() {
        assert_eq!(
            eval_ok("Replace[\"hello world\", \"world\", \"taida\"]() ]=> x\nx"),
            Value::str("hello taida".to_string())
        );
    }

    #[test]
    fn test_eval_string_to_int() {
        // Use Int[] type conversion mold instead of .toInt()
        assert_eq!(eval_ok("Int[\"42\"]() ]=> x\nx"), Value::Int(42));
    }

    // ── Number Methods (auto-mold) ──

    #[test]
    fn test_eval_num_to_string() {
        assert_eq!(
            eval_ok("x <= 42.toString()\nx"),
            Value::str("42".to_string())
        );
    }

    #[test]
    fn test_eval_num_abs() {
        assert_eq!(eval_ok("Abs[-5]() ]=> x\nx"), Value::Int(5));
    }

    #[test]
    fn test_eval_num_floor() {
        assert_eq!(eval_ok("Floor[3.7]() ]=> x\nx"), Value::Int(3));
    }

    #[test]
    fn test_eval_num_ceil() {
        assert_eq!(eval_ok("Ceil[3.2]() ]=> x\nx"), Value::Int(4));
    }

    #[test]
    fn test_eval_num_round() {
        assert_eq!(eval_ok("Round[3.5]() ]=> x\nx"), Value::Int(4));
    }

    // ── List Methods (auto-mold) ──

    #[test]
    fn test_eval_list_length() {
        assert_eq!(eval_ok("x <= @[1, 2, 3].length()\nx"), Value::Int(3));
    }

    #[test]
    fn test_eval_list_first() {
        // first() returns Lax — .unmold() extracts value
        assert_eq!(
            eval_ok("x <= @[10, 20, 30].first().unmold()\nx"),
            Value::Int(10)
        );
    }

    #[test]
    fn test_eval_list_last() {
        // last() returns Lax — .unmold() extracts value
        assert_eq!(
            eval_ok("x <= @[10, 20, 30].last().unmold()\nx"),
            Value::Int(30)
        );
    }

    #[test]
    fn test_eval_list_contains() {
        assert_eq!(eval_ok("x <= @[1, 2, 3].contains(2)\nx"), Value::Bool(true));
        assert_eq!(
            eval_ok("x <= @[1, 2, 3].contains(5)\nx"),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_eval_list_reverse() {
        let result = eval_ok("Reverse[@[1, 2, 3]]() ]=> x\nx");
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(3), Value::Int(2), Value::Int(1)]
                );
            }
            _ => panic!("Expected List"),
        }
    }

    #[test]
    fn test_eval_list_join() {
        assert_eq!(
            eval_ok("Join[@[\"a\", \"b\", \"c\"], \",\"]() ]=> x\nx"),
            Value::str("a,b,c".to_string())
        );
    }

    #[test]
    fn test_eval_list_sum() {
        assert_eq!(eval_ok("Sum[@[1, 2, 3]]() ]=> x\nx"), Value::Int(6));
    }

    #[test]
    fn test_eval_list_concat() {
        let result = eval_ok("Concat[@[1, 2], @[3, 4]]() ]=> x\nx");
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 4);
            }
            _ => panic!("Expected List"),
        }
    }

    #[test]
    fn test_eval_list_append() {
        let result = eval_ok("Append[@[1, 2], 3]() ]=> x\nx");
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(1), Value::Int(2), Value::Int(3)]
                );
            }
            _ => panic!("Expected List"),
        }
    }

    #[test]
    fn test_eval_list_sort() {
        let result = eval_ok("Sort[@[3, 1, 2]]() ]=> x\nx");
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(1), Value::Int(2), Value::Int(3)]
                );
            }
            _ => panic!("Expected List"),
        }
    }

    // ── Bool Methods ──

    #[test]
    fn test_eval_bool_to_string() {
        assert_eq!(
            eval_ok("x <= true.toString()\nx"),
            Value::str("true".to_string())
        );
    }

    #[test]
    fn test_eval_bool_to_int() {
        // Use Int[] type conversion mold instead of .toInt()
        assert_eq!(eval_ok("Int[true]() ]=> x\nx"), Value::Int(1));
        assert_eq!(eval_ok("Int[false]() ]=> x\nx"), Value::Int(0));
    }

    // ── Debug (builtin prelude) ──

    #[test]
    fn test_eval_debug() {
        let (val, output) = eval_with_output("debug(\"Hello, Taida!\")");
        assert_eq!(output, vec!["Hello, Taida!"]);
        // debug returns the value
        assert_eq!(val, Value::str("Hello, Taida!".to_string()));
    }

    #[test]
    fn test_eval_debug_multiple() {
        let (_, output) = eval_with_output("debug(\"line 1\")\ndebug(\"line 2\")");
        assert_eq!(output, vec!["line 1", "line 2"]);
    }

    #[test]
    fn test_eval_debug_with_label() {
        let (val, output) = eval_with_output("debug(42, \"check\")");
        assert_eq!(output, vec!["[check] Int: 42"]);
        assert_eq!(val, Value::Int(42));
    }

    // ── Stdout (prelude builtin) ──

    #[test]
    fn test_eval_stdout_via_import() {
        // stdout is now a prelude builtin — no import needed
        let source = r#"stdout("Hello, Taida!")"#;
        let (_, output) = eval_with_output(source);
        assert_eq!(output, vec!["Hello, Taida!"]);
    }

    #[test]
    fn test_std_import_errors() {
        // `>>> std/io` should error after dissolution
        let source = r#">>> std/io => @(stdout)"#;
        let result = eval(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("dissolved"));
    }

    // ── Gorilla ──

    #[test]
    fn test_eval_gorilla() {
        let result = eval_ok("><");
        assert_eq!(result, Value::Gorilla);
    }

    // ── Type Def + Instantiation ──

    #[test]
    fn test_eval_type_def_and_instantiation() {
        let source = "Person = @(name: Str, age: Int)\nalice <= Person(name <= \"Alice\", age <= 30)\nalice.name";
        assert_eq!(eval_ok(source), Value::str("Alice".to_string()));
    }

    #[test]
    fn test_eval_type_inst_injects_typed_field_defaults() {
        let source = r#"
Person = @(name: Str, age: Int, active: Bool)
alice <= Person(name <= "Alice")
alice.age
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_eval_type_inst_injects_default_value_field() {
        let source = r#"
Person = @(name: Str, country <= "JP")
alice <= Person(name <= "Alice")
alice.country
"#;
        assert_eq!(eval_ok(source), Value::str("JP".to_string()));
    }

    #[test]
    fn test_eval_inheritance_type_inst_injects_parent_and_child_defaults() {
        let source = r#"
Person = @(name: Str, age: Int)
Person => Employee = @(department: Str)
e <= Employee(name <= "Shinji")
e.department
"#;
        assert_eq!(eval_ok(source), Value::str(String::new()));
    }

    // ── Complex Programs ──

    #[test]
    fn test_eval_multiline_program() {
        let source = r#"
x <= 10
y <= 20
z <= x + y
z
"#;
        assert_eq!(eval_ok(source), Value::Int(30));
    }

    #[test]
    fn test_eval_nested_field_access() {
        let source = r#"
config <= @(server <= @(host <= "localhost", port <= 8080))
config.server.host
"#;
        assert_eq!(eval_ok(source), Value::str("localhost".to_string()));
    }

    // ── Error Ceiling Runtime ──

    #[test]
    fn test_eval_error_ceiling_catches_throw() {
        let source = r#"
Error => DivError = @(divisor: Int)
divide x y =
  |== error: Error =
    0
  => :Int
  | y == 0 |> DivError(type <= "DivError", message <= "Division by zero", divisor <= y).throw()
  | _ |> Div[x, y]().unmold()
=> :Int
result <= divide(10, 0)
result
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_eval_error_ceiling_normal_path() {
        let source = r#"
Error => DivError = @(divisor: Int)
divide x y =
  |== error: Error =
    0
  => :Int
  | y == 0 |> DivError(type <= "DivError", message <= "Division by zero", divisor <= y).throw()
  | _ |> Div[x, y]().unmold()
=> :Int
result <= divide(10, 2)
result
"#;
        assert_eq!(eval_ok(source), Value::Int(5));
    }

    #[test]
    fn test_eval_error_ceiling_with_error_field_access() {
        let source = r#"
Error => MyError = @(code: Int)
process x =
  |== error: Error =
    error.message
  => :Str
  | x < 0 |> MyError(type <= "MyError", message <= "Negative value", code <= 400).throw()
  | _ |> "OK"
=> :Str
result <= process(-5)
result
"#;
        assert_eq!(eval_ok(source), Value::str("Negative value".to_string()));
    }

    #[test]
    fn test_eval_error_ceiling_propagation() {
        // Error thrown in inner function propagates to outer error ceiling
        let source = r#"
Error => InnerError = @()
inner x =
  | x < 0 |> InnerError(type <= "InnerError", message <= "Negative").throw()
  | _ |> x * 2
=> :Int
outer x =
  |== error: Error =
    -1
  => :Int
  result <= inner(x)
  result + 10
=> :Int
r <= outer(-5)
r
"#;
        assert_eq!(eval_ok(source), Value::Int(-1));
    }

    #[test]
    fn test_eval_error_ceiling_propagation_normal() {
        let source = r#"
Error => InnerError = @()
inner x =
  | x < 0 |> InnerError(type <= "InnerError", message <= "Negative").throw()
  | _ |> x * 2
=> :Int
outer x =
  |== error: Error =
    -1
  => :Int
  result <= inner(x)
  result + 10
=> :Int
r <= outer(5)
r
"#;
        assert_eq!(eval_ok(source), Value::Int(20));
    }

    // ── BT-15: Error ceiling nesting edge cases ──

    #[test]
    fn test_bt15_double_error_ceiling() {
        // Inner error ceiling catches inner throw, outer ceiling is untouched
        let source = r#"
Error => MyError = @()
inner x =
  |== error: Error =
    -1
  => :Int
  | x < 0 |> MyError(type <= "MyError", message <= "inner error").throw()
  | _ |> x
=> :Int
outer x =
  |== error: Error =
    -999
  => :Int
  result <= inner(x)
  result + 100
=> :Int
r <= outer(-5)
r
"#;
        // Inner ceiling catches the throw, returns -1. Outer ceiling gets -1 + 100 = 99
        assert_eq!(eval_ok(source), Value::Int(99));
    }

    #[test]
    fn test_bt15_closure_error_ceiling() {
        // Error ceiling inside a function called indirectly
        let source = r#"
Error => SafeError = @()
safe x =
  |== error: Error =
    0
  => :Int
  | x < 0 |> SafeError(type <= "SafeError", message <= "negative").throw()
  | _ |> x * 2
=> :Int
result <= safe(-3)
result
"#;
        // Error ceiling should catch the throw inside safe(-3) and return 0
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_eval_gorilla_ceiling_unhandled_throw() {
        // Unhandled throw causes program termination (Gorilla ceiling)
        let source = r#"
Error => TestError = @()
TestError(type <= "TestError", message <= "Boom").throw()
"#;
        let result = eval(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unhandled error"));
    }

    #[test]
    fn test_eval_error_ceiling_conditional_handler() {
        let source = r#"
Error => ValidationError = @(field: Str)
Error => ParseError = @(input: Str)
process text =
  |== error: Error =
    | error.type == "ValidationError" |> "Invalid"
    | error.type == "ParseError" |> "Parse failed"
    | _ |> "Unknown"
  => :Str
  | text == "" |> ValidationError(type <= "ValidationError", message <= "Empty input", field <= "text").throw()
  | _ |> "Processed: " + text
=> :Str
r <= process("")
r
"#;
        assert_eq!(eval_ok(source), Value::str("Invalid".to_string()));
    }

    // ── Module System ──

    #[test]
    fn test_eval_module_import() {
        // Create a temp module file
        let dir = std::env::temp_dir().join("taida_test_modules");
        std::fs::create_dir_all(&dir).unwrap();

        let module_file = dir.join("utils.td");
        std::fs::write(
            &module_file,
            r#"
double x =
  x * 2
=> :Int
<<< @(double)
"#,
        )
        .unwrap();

        let main_source = format!(
            ">>> {} => @(double)\nresult <= double(21)\nresult",
            module_file.display()
        );

        let (program, errors) = crate::parser::parse(&main_source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interp = crate::interpreter::eval::Interpreter::new();
        interp.set_current_file(std::path::Path::new("/tmp/main.td"));
        let result = interp.eval_program(&program).unwrap();
        assert_eq!(result, Value::Int(42));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_eval_module_import_relative() {
        let dir = std::env::temp_dir().join("taida_test_rel");
        std::fs::create_dir_all(&dir).unwrap();

        let module_file = dir.join("helper.td");
        std::fs::write(
            &module_file,
            "greet name =\n  \"Hello, \" + name\n=> :Str\n<<< @(greet)\n",
        )
        .unwrap();

        let main_file = dir.join("main.td");
        let main_source = ">>> ./helper.td => @(greet)\nresult <= greet(\"World\")\nresult";

        let (program, errors) = crate::parser::parse(main_source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interp = crate::interpreter::eval::Interpreter::new();
        interp.set_current_file(&main_file);
        let result = interp.eval_program(&program).unwrap();
        assert_eq!(result, Value::str("Hello, World".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_eval_module_circular_import_detection() {
        let dir = std::env::temp_dir().join("taida_test_circular");
        std::fs::create_dir_all(&dir).unwrap();

        let a_file = dir.join("a.td");
        let b_file = dir.join("b.td");

        // a.td imports b.td
        std::fs::write(
            &a_file,
            format!(
                ">>> {} => @(funcB)\nfuncA x =\n  x + 1\n=> :Int\n<<< @(funcA)\n",
                b_file.display()
            ),
        )
        .unwrap();

        // b.td imports a.td (circular)
        std::fs::write(
            &b_file,
            format!(
                ">>> {} => @(funcA)\nfuncB x =\n  x + 2\n=> :Int\n<<< @(funcB)\n",
                a_file.display()
            ),
        )
        .unwrap();

        let main_source = format!(
            ">>> {} => @(funcA)\nresult <= funcA(10)\nresult",
            a_file.display()
        );
        let (program, errors) = crate::parser::parse(&main_source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);

        let mut interp = crate::interpreter::eval::Interpreter::new();
        interp.set_current_file(&dir.join("main.td"));
        let result = interp.eval_program(&program);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Circular import"),
            "Expected circular import error, got: {}",
            err_msg
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_eval_module_caching() {
        // A module imported twice should only be executed once
        let dir = std::env::temp_dir().join("taida_test_cache");
        std::fs::create_dir_all(&dir).unwrap();

        let module_file = dir.join("counter.td");
        std::fs::write(&module_file, "value <= 42\n<<< @(value)\n").unwrap();

        let main_source = format!(
            ">>> {} => @(value)\nfirst <= value\nfirst",
            module_file.display()
        );

        let (program, errors) = crate::parser::parse(&main_source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interp = crate::interpreter::eval::Interpreter::new();
        interp.set_current_file(&dir.join("main.td"));
        let result = interp.eval_program(&program).unwrap();
        assert_eq!(result, Value::Int(42));

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── List Operation Mold Types (Map/Filter/Fold) ──

    #[test]
    fn test_eval_map() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
Map[numbers, _ x = x * 2]() ]=> doubled
doubled
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[
                        Value::Int(2),
                        Value::Int(4),
                        Value::Int(6),
                        Value::Int(8),
                        Value::Int(10)
                    ]
                );
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_filter() {
        let source = r#"
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
Filter[numbers, isEven]() ]=> evens
evens
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[
                        Value::Int(2),
                        Value::Int(4),
                        Value::Int(6),
                        Value::Int(8),
                        Value::Int(10)
                    ]
                );
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_fold_sum() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
Fold[numbers, 0, _ acc x = acc + x]() ]=> sum
sum
"#;
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    #[test]
    fn test_eval_fold_product() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
Fold[numbers, 1, _ acc x = acc * x]() ]=> product
product
"#;
        assert_eq!(eval_ok(source), Value::Int(120));
    }

    #[test]
    fn test_eval_take() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
Take[numbers, 3]() ]=> first3
first3
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(1), Value::Int(2), Value::Int(3)]
                );
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_drop() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
Drop[numbers, 2]() ]=> rest
rest
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(3), Value::Int(4), Value::Int(5)]
                );
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_take_while() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
TakeWhile[numbers, _ x = x < 4]() ]=> under4
under4
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(1), Value::Int(2), Value::Int(3)]
                );
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_drop_while() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
DropWhile[numbers, _ x = x < 4]() ]=> from4
from4
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(items.as_slice(), &[Value::Int(4), Value::Int(5)]);
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_foldr() {
        let source = r#"
words <= @["a", "b", "c"]
Foldr[words, "", _ acc x = x + acc]() ]=> concat
concat
"#;
        assert_eq!(eval_ok(source), Value::str("abc".to_string()));
    }

    #[test]
    fn test_eval_filter_map_fold_chain() {
        // Filter even numbers, double them, sum them
        let source = r#"
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
Filter[numbers, isEven]() ]=> evens
Map[evens, _ x = x * 2]() ]=> doubled
Fold[doubled, 0, _ acc x = acc + x]() ]=> sum
sum
"#;
        assert_eq!(eval_ok(source), Value::Int(60));
    }

    #[test]
    fn test_eval_map_with_condition_func() {
        // Use a named function for complex logic instead of inline lambda
        let source = r#"
transform x =
  | x > 3 |> x * 10
  | _ |> x
=> :Int
numbers <= @[1, 2, 3, 4, 5]
Map[numbers, transform]() ]=> processed
processed
"#;
        let result = eval_ok(source);
        match result {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[
                        Value::Int(1),
                        Value::Int(2),
                        Value::Int(3),
                        Value::Int(40),
                        Value::Int(50)
                    ]
                );
            }
            _ => panic!("Expected List, got {:?}", result),
        }
    }

    #[test]
    fn test_eval_map_lambda_call_does_not_trigger_tailcall_error() {
        let source = r#"
idFn x =
  x
=> :Int

invoke dummy =
  nums <= @[1, 2, 3]
  mapped <= Map[nums, _ x = idFn(x)]()
  mapped.length()
=> :Int

invoke(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_eval_fold_with_named_func() {
        // Use a named function for accumulation
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
addToSum acc x =
  acc + x
=> :Int
Fold[numbers, 0, addToSum]() ]=> total
total
"#;
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    #[test]
    fn test_eval_module_alias_import() {
        let dir = std::env::temp_dir().join("taida_test_alias");
        std::fs::create_dir_all(&dir).unwrap();

        let module_file = dir.join("math.td");
        std::fs::write(&module_file, "add x y =\n  x + y\n=> :Int\n<<< @(add)\n").unwrap();

        let main_source = format!(
            ">>> {} => @(add => myAdd)\nresult <= myAdd(3, 4)\nresult",
            module_file.display()
        );

        let (program, errors) = crate::parser::parse(&main_source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut interp = crate::interpreter::eval::Interpreter::new();
        interp.set_current_file(&dir.join("main.td"));
        let result = interp.eval_program(&program).unwrap();
        assert_eq!(result, Value::Int(7));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_eval_core_bundled_crypto_import() {
        let dir = std::env::temp_dir().join("taida_test_core_crypto_import");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("packages.tdm"), ">>> taida-lang/crypto@a.1\n").unwrap();

        let manifest = crate::pkg::manifest::Manifest::from_dir(&dir)
            .unwrap()
            .expect("manifest should exist");
        let resolved = crate::pkg::resolver::resolve_deps(&manifest);
        assert!(
            resolved.errors.is_empty(),
            "resolve errors: {:?}",
            resolved.errors
        );
        crate::pkg::resolver::install_deps(&manifest, &resolved).unwrap();

        let source = ">>> taida-lang/crypto => @(sha256)\nsha256(\"abc\")";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);

        let mut interp = crate::interpreter::eval::Interpreter::new();
        interp.set_current_file(&dir.join("main.td"));
        let result = interp.eval_program(&program).unwrap();
        assert_eq!(
            result,
            Value::str(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()
            )
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Tail Recursion Optimization ──

    #[test]
    fn test_tco_factorial() {
        let source = r#"
factorialTail n acc =
  | n < 1 |> acc
  | _ |> factorialTail(n - 1, acc * n)

result <= factorialTail(10, 1)
result"#;
        assert_eq!(eval_ok(source), Value::Int(3628800));
    }

    #[test]
    fn test_tco_sum_recursive() {
        let source = r#"
sumTail n acc =
  | n == 0 |> acc
  | _ |> sumTail(n - 1, acc + n)

result <= sumTail(100, 0)
result"#;
        assert_eq!(eval_ok(source), Value::Int(5050));
    }

    #[test]
    fn test_tco_fibonacci() {
        let source = r#"
fibTail n a b =
  | n == 0 |> a
  | n == 1 |> b
  | _ |> fibTail(n - 1, b, a + b)

result <= fibTail(20, 0, 1)
result"#;
        assert_eq!(eval_ok(source), Value::Int(6765));
    }

    #[test]
    fn test_tco_deep_recursion() {
        // This would stack overflow without TCO
        let source = r#"
countdown n =
  | n == 0 |> 0
  | _ |> countdown(n - 1)

result <= countdown(100000)
result"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_tco_non_tail_still_works() {
        // Non-tail recursive call should still work (just not optimized)
        let source = r#"
factorial n =
  | n < 1 |> 1
  | _ |> n * factorial(n - 1)

result <= factorial(10)
result"#;
        assert_eq!(eval_ok(source), Value::Int(3628800));
    }

    #[test]
    fn test_tco_accumulator_pattern() {
        let source = r#"
repeatStr str n acc =
  | n < 1 |> acc
  | _ |> repeatStr(str, n - 1, acc + str)

result <= repeatStr("ab", 3, "")
result"#;
        assert_eq!(eval_ok(source), Value::str("ababab".to_string()));
    }

    #[test]
    fn test_tco_with_error_ceiling() {
        let source = r#"
safeDivLoop n total =
  |== error: Error =
    total
  => :Int

  | n == 0 |> total
  | _ |> safeDivLoop(n - 1, total + Div[100, n]().unmold())

result <= safeDivLoop(5, 0)
result"#;
        // 100/5=20, 100/4=25, 100/3=33, 100/2=50, 100/1=100 => 228
        assert_eq!(eval_ok(source), Value::Int(228));
    }

    // ── Mutual Recursion TCO ──

    #[test]
    fn test_mutual_recursion_is_even_is_odd() {
        let source = r#"
isEven n =
  | n == 0 |> true
  | _ |> isOdd(n - 1)

isOdd n =
  | n == 0 |> false
  | _ |> isEven(n - 1)

stdout(isEven(0))
stdout(isEven(1))
stdout(isEven(4))
stdout(isOdd(3))
stdout(isOdd(4))
"#;
        let (_, output) = eval_with_output(source);
        assert_eq!(output, vec!["true", "false", "true", "true", "false"]);
    }

    #[test]
    fn test_mutual_recursion_deep() {
        // This would stack overflow without TCO — 100000 mutual calls
        let source = r#"
isEven n =
  | n == 0 |> true
  | _ |> isOdd(n - 1)

isOdd n =
  | n == 0 |> false
  | _ |> isEven(n - 1)

isEven(100000)
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_mutual_recursion_three_functions() {
        let source = r#"
fizzBuzz n =
  | n == 0 |> 0
  | _ |> fizz(n - 1)

fizz n =
  | n == 0 |> 0
  | _ |> buzz(n - 1)

buzz n =
  | n == 0 |> 0
  | _ |> fizzBuzz(n - 1)

fizzBuzz(99999)
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_mutual_recursion_with_accumulator() {
        let source = r#"
countDown n acc =
  | n == 0 |> acc
  | _ |> countUp(n - 1, acc + 1)

countUp n acc =
  | n == 0 |> acc
  | _ |> countDown(n - 1, acc + 1)

countDown(10, 0)
"#;
        assert_eq!(eval_ok(source), Value::Int(10));
    }

    // ── Async Basics ──

    #[test]
    fn test_async_create_fulfilled() {
        // Async[value]() creates a fulfilled async
        let source = r#"
a <= Async[42]()
a
"#;
        match eval_ok(source) {
            Value::Async(a) => {
                assert_eq!(a.status, AsyncStatus::Fulfilled);
                assert_eq!(*a.value, Value::Int(42));
            }
            other => panic!("Expected Async, got {:?}", other),
        }
    }

    #[test]
    fn test_async_create_rejected() {
        // AsyncReject[error]() creates a rejected async
        let source = r#"
a <= AsyncReject["something went wrong"]()
a
"#;
        match eval_ok(source) {
            Value::Async(a) => {
                assert_eq!(a.status, AsyncStatus::Rejected);
                assert_eq!(*a.error, Value::str("something went wrong".to_string()));
            }
            other => panic!("Expected Async, got {:?}", other),
        }
    }

    #[test]
    fn test_async_unmold_forward() {
        // ]=> unwraps fulfilled async
        let source = r#"
a <= Async[42]()
a ]=> val
val
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_async_unmold_string() {
        // ]=> unwraps fulfilled async containing a string
        let source = r#"
a <= Async["hello"]()
a ]=> val
val
"#;
        assert_eq!(eval_ok(source), Value::str("hello".to_string()));
    }

    #[test]
    fn test_async_rejected_unmold_throws() {
        // Unmolding a rejected async should throw, caught by error ceiling
        let source = r#"
handleError unused =
  |== error: Error =
    "caught"
  => :Str

  a <= AsyncReject["fail"]()
  a ]=> val
  val
=> :Str

result <= handleError(0)
result
"#;
        assert_eq!(eval_ok(source), Value::str("caught".to_string()));
    }

    #[test]
    fn test_async_is_pending() {
        // Status check methods
        let source = r#"
a <= Async[42]()
a.isFulfilled()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_async_is_rejected() {
        let source = r#"
a <= AsyncReject["err"]()
a.isRejected()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_async_map() {
        // map applies function to fulfilled async
        let source = r#"
double x =
  x * 2
=> :Int

a <= Async[21]()
b <= a.map(double)
b ]=> val
val
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_async_map_rejected_propagates() {
        // map on rejected async propagates rejection
        let source = r#"
double x =
  x * 2
=> :Int

a <= AsyncReject["err"]()
b <= a.map(double)
b.isRejected()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_async_get_or_default_fulfilled() {
        let source = r#"
a <= Async[42]()
a.getOrDefault(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_async_get_or_default_rejected() {
        let source = r#"
a <= AsyncReject["err"]()
a.getOrDefault(99)
"#;
        assert_eq!(eval_ok(source), Value::Int(99));
    }

    #[test]
    fn test_async_all() {
        // All collects all fulfilled results
        let source = r#"
asyncs <= @[Async[1](), Async[2](), Async[3]()]
All[asyncs]() ]=> results
results
"#;
        match eval_ok(source) {
            Value::List(items) => {
                assert_eq!(
                    items.as_slice(),
                    &[Value::Int(1), Value::Int(2), Value::Int(3)]
                );
            }
            other => panic!("Expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_async_all_rejected_propagates() {
        // All with a rejected async should throw
        let source = r#"
handleError unused =
  |== error: Error =
    "all_failed"
  => :Str

  asyncs <= @[Async[1](), AsyncReject["fail"](), Async[3]()]
  All[asyncs]() ]=> results
  "success"
=> :Str

result <= handleError(0)
result
"#;
        assert_eq!(eval_ok(source), Value::str("all_failed".to_string()));
    }

    #[test]
    fn test_async_race() {
        // Race returns the first resolved
        let source = r#"
asyncs <= @[Async[42](), Async[99]()]
Race[asyncs]() ]=> winner
winner
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_async_timeout_passthrough() {
        // Timeout in synchronous mode just passes through
        let source = r#"
a <= Async[42]()
Timeout[a, 5000]() ]=> val
val
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_async_cancel_pending_sleep() {
        // Cancel converts pending async into rejected Async[CancelledError].
        let source = r#"
handleError unused =
  |== e: Error =
    e.type
  => :Str

  a <= sleep(1000)
  c <= Cancel[a]()
  c ]=> ignored
  "unexpected"
=> :Str

handleError(0)
"#;
        assert_eq!(eval_ok(source), Value::str("CancelledError".to_string()));
    }

    #[test]
    fn test_async_to_string() {
        let source = r#"
a <= Async[42]()
a.toString()
"#;
        assert_eq!(
            eval_ok(source),
            Value::str("Async[fulfilled: 42]".to_string())
        );
    }

    #[test]
    fn test_async_chain() {
        // Chain async operations via ]=>
        let source = r#"
addOne x =
  Async[x + 1]()

a <= Async[10]()
a ]=> v1
b <= addOne(v1)
b ]=> v2
c <= addOne(v2)
c ]=> v3
v3
"#;
        assert_eq!(eval_ok(source), Value::Int(12));
    }

    #[test]
    fn test_async_with_error_ceiling() {
        // Error ceiling catches rejected async errors
        let source = r#"
fetchData key: Str =
  |== error: Error =
    "default"
  => :Str

  a <= AsyncReject["network error"]()
  a ]=> data
  data
=> :Str

result <= fetchData("test")
result
"#;
        assert_eq!(eval_ok(source), Value::str("default".to_string()));
    }

    // ── UnmoldBackward <=[ Tests ──────────────────────────────

    #[test]
    fn test_unmold_backward_lax() {
        let source = r#"
opt <= Lax[42]()
value <=[ opt
value
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_unmold_backward_string() {
        let source = r#"
opt <= Lax["hello"]()
s <=[ opt
s
"#;
        assert_eq!(eval_ok(source), Value::str("hello".to_string()));
    }

    #[test]
    fn test_unmold_backward_map() {
        let source = r#"
numbers <= @[1, 2, 3]
doubled <=[ Map[numbers, _ x = x * 2]()
doubled
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
    }

    #[test]
    fn test_unmold_backward_filter() {
        let source = r#"
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
numbers <= @[1, 2, 3, 4, 5]
evens <=[ Filter[numbers, isEven]()
evens
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(2), Value::Int(4)])
        );
    }

    #[test]
    fn test_unmold_backward_reduce() {
        let source = r#"
numbers <= @[1, 2, 3, 4, 5]
total <=[ Reduce[numbers, 0, _ acc x = acc + x]()
total
"#;
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    #[test]
    fn test_unmold_backward_async() {
        let source = r#"
a <= Async[100]()
val <=[ a
val
"#;
        assert_eq!(eval_ok(source), Value::Int(100));
    }

    // ── No-argument Function Tests ───────────────────────────

    #[test]
    fn test_noarg_function_basic() {
        let source = r#"
getVersion =
  "1.0.0"
=> :Str

getVersion()
"#;
        assert_eq!(eval_ok(source), Value::str("1.0.0".to_string()));
    }

    #[test]
    fn test_noarg_function_multiline() {
        let source = r#"
getSum =
  a <= 10
  b <= 20
  a + b
=> :Int

getSum()
"#;
        assert_eq!(eval_ok(source), Value::Int(30));
    }

    #[test]
    fn test_noarg_function_returns_buchi_pack() {
        let source = r#"
getDefaults =
  @(name <= "default", value <= 0)
=> :@(name: Str, value: Int)

d <= getDefaults()
d.name
"#;
        assert_eq!(eval_ok(source), Value::str("default".to_string()));
    }

    #[test]
    fn test_function_call_missing_args_use_effective_defaults() {
        let source = r#"
sum3 a: Int b: Int <= 10 c: Int <= a + b =
  a + b + c
=> :Int

sum3()
"#;
        assert_eq!(eval_ok(source), Value::Int(20));
    }

    #[test]
    fn test_function_call_defaults_can_reference_previous_params() {
        let source = r#"
sum3 a: Int b: Int <= 10 c: Int <= a + b =
  a + b + c
=> :Int

sum3(1, 2)
"#;
        assert_eq!(eval_ok(source), Value::Int(6));
    }

    #[test]
    fn test_function_call_too_many_args_is_runtime_error() {
        let source = r#"
id x: Int =
  x
=> :Int

id(1, 2)
"#;
        let err = eval(source).expect_err("expected runtime arity error");
        assert!(
            err.contains("expected at most 1 argument(s), got 2"),
            "Unexpected error: {}",
            err
        );
    }

    // ── Mold Unmold consistency ──────────────────────────────

    #[test]
    fn test_unmold_forward_lax() {
        let source = r#"
opt <= Lax[42]()
opt ]=> value
value
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    // ── Standard Library Integration Tests ────────────────────
    // NOTE: JSON Molten Iron design — jsonDecode/jsonParse abolished.
    // JSON must be cast through schema: JSON[raw, Schema]()

    #[test]
    fn test_stdlib_json_encode_roundtrip() {
        // jsonEncode still works (output direction)
        let source = r#"
data <= @(name <= "Alice", age <= 30)
encoded <= jsonEncode(data)
encoded
"#;
        if let Value::Str(s) = eval_ok(source) {
            assert!(s.contains("Alice"));
        } else {
            panic!("Expected Str");
        }
    }

    #[test]
    fn test_stdlib_json_schema_cast() {
        // JSON[raw, Schema]() with TypeDef
        let source = r#"
Data = @(x: Int)
raw <= "{\"x\": 42}"
JSON[raw, Data]() ]=> result
result.x
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_stdlib_json_pretty() {
        let source = r#"
data <= @(x <= 1)
result <= jsonPretty(data)
result
"#;
        if let Value::Str(s) = eval_ok(source) {
            assert!(s.contains('\n'), "Pretty JSON should have newlines");
            assert!(s.contains("\"x\": 1"), "Should contain x: 1");
        } else {
            panic!("Expected Str");
        }
    }

    #[test]
    fn test_stdlib_json_null_becomes_default() {
        // Taida's philosophy: no null. JSON null -> default value
        let source = r#"
Data = @(value: Str)
raw <= "{\"value\": null}"
JSON[raw, Data]() ]=> result
result.value
"#;
        assert_eq!(eval_ok(source), Value::str(String::new()));
    }

    #[test]
    fn test_jsonparse_abolished() {
        let source = r#"jsonParse("{}")"#;
        let result = eval(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("removed"));
    }

    #[test]
    fn test_jsondecode_abolished() {
        let source = r#"jsonDecode("{}")"#;
        let result = eval(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("removed"));
    }

    #[test]
    fn test_jsonfrom_abolished() {
        let source = r#"jsonFrom(@(x <= 1))"#;
        let result = eval(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("removed"));
    }

    // ── Partial Application (empty slot syntax) ──

    #[test]
    fn test_partial_application_basic() {
        // add(5, ) should return a function that adds 5
        let source = r#"
add x y = x + y => :Int
add5 <= add(5, )
add5(3)
"#;
        assert_eq!(eval_ok(source), Value::Int(8));
    }

    #[test]
    fn test_partial_application_second_arg() {
        // multiply(, 2) should return a function that doubles
        let source = r#"
multiply x y = x * y => :Int
double <= multiply(, 2)
double(7)
"#;
        assert_eq!(eval_ok(source), Value::Int(14));
    }

    #[test]
    fn test_partial_application_first_arg() {
        // multiply(3, ) should return a function that multiplies by 3
        let source = r#"
multiply x y = x * y => :Int
triple <= multiply(3, )
triple(5)
"#;
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    #[test]
    fn test_partial_application_with_debug() {
        // Partial application should work with debug output
        let source = r#"
add x y = x + y => :Int
add10 <= add(10, )
debug(add10(5))
"#;
        let (_, output) = eval_with_output(source);
        assert_eq!(output, vec!["15"]);
    }

    #[test]
    fn test_partial_application_chained() {
        // Use partial application result as argument
        let source = r#"
add x y = x + y => :Int
add5 <= add(5, )
result <= add5(add5(3))
result
"#;
        assert_eq!(eval_ok(source), Value::Int(13)); // add5(8) = 13
    }

    #[test]
    fn test_partial_application_subtract() {
        let source = r#"
sub x y = x - y => :Int
sub_from_100 <= sub(100, )
sub_from_100(30)
"#;
        assert_eq!(eval_ok(source), Value::Int(70));
    }

    // ── BT-14: Partial application edge cases ──

    #[test]
    fn test_bt14_partial_all_holes() {
        // add(, ) — all arguments as holes, should return a function equivalent to add itself
        let source = r#"
add x y = x + y => :Int
addAll <= add(, )
addAll(3, 4)
"#;
        assert_eq!(eval_ok(source), Value::Int(7));
    }

    #[test]
    fn test_bt14_partial_first_arg_hole() {
        // add(, 5) — first argument is hole
        let source = r#"
add x y = x + y => :Int
addTo5 <= add(, 5)
addTo5(3)
"#;
        assert_eq!(eval_ok(source), Value::Int(8));
    }

    // ── Pipeline ──

    #[test]
    fn test_pipeline_basic() {
        // 5 => add(3, _) => result
        let source = r#"
add x y = x + y => :Int
5 => add(3, _) => result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(8));
    }

    #[test]
    fn test_pipeline_chain() {
        // 10 => add(5, _) => multiply(_, 3) => result
        let source = r#"
add x y = x + y => :Int
multiply x y = x * y => :Int
10 => add(5, _) => multiply(_, 3) => result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(45));
    }

    #[test]
    fn test_pipeline_no_placeholder() {
        // Without placeholder, current is passed as first arg
        let source = r#"
double x = x * 2 => :Int
5 => double => result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(10));
    }

    #[test]
    fn test_pipeline_assign_to_variable() {
        // Pipeline ending with identifier assigns to that variable
        let source = r#"
add x y = x + y => :Int
3 => add(2, _) => result
debug(result)
"#;
        let (_, output) = eval_with_output(source);
        assert_eq!(output, vec!["5"]);
    }

    #[test]
    fn test_pipeline_single_step() {
        // Single step pipeline: expr => func => target
        let source = r#"
inc x = x + 1 => :Int
10 => inc => result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(11));
    }

    #[test]
    fn test_pipeline_string_method() {
        // Pipeline with method-like functions
        let source = r#"
data <= "hello"
data => result
result
"#;
        assert_eq!(eval_ok(source), Value::str("hello".to_string()));
    }

    #[test]
    fn test_pipeline_with_literal_start() {
        // Pipeline starting with a literal value
        let source = r#"
add x y = x + y => :Int
multiply x y = x * y => :Int
2 => add(3, _) => multiply(_, 4) => result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(20)); // (2+3)*4 = 20
    }

    // ── Prelude: Optional (list-derived) ──

    // ── Optional ABOLISHED (v0.8.0) — verify errors ──

    #[test]
    fn test_optional_abolished_with_value() {
        // Optional[42]() should produce an error
        let (program, errors) = crate::parser::parse("Optional[42]()");
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        let result = interp.eval_program(&program);
        assert!(result.is_err(), "Optional should produce an error");
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.message.contains("Optional has been removed"),
            "Error should mention removal: {}",
            err_msg.message
        );
    }

    #[test]
    fn test_optional_abolished_empty() {
        // Optional[]() should produce an error
        let (program, errors) = crate::parser::parse("Optional[]()");
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        let result = interp.eval_program(&program);
        assert!(result.is_err(), "Optional should produce an error");
    }

    #[test]
    fn test_optional_abolished_some() {
        // Some() should produce an error
        let (program, errors) = crate::parser::parse("Some(42)");
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        let result = interp.eval_program(&program);
        assert!(result.is_err(), "Some() should produce an error");
    }

    #[test]
    fn test_optional_abolished_none() {
        // None() should produce an error
        let (program, errors) = crate::parser::parse("None()");
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        let result = interp.eval_program(&program);
        assert!(result.is_err(), "None() should produce an error");
    }

    #[test]
    fn test_optional_replaced_by_lax() {
        // Lax[value]() is the replacement for Optional
        assert_eq!(eval_ok("Lax[42]().hasValue()"), Value::Bool(true));
        assert_eq!(eval_ok("Lax[42]().getOrDefault(0)"), Value::Int(42));
    }

    // ── Prelude: Lax ──

    #[test]
    fn test_lax_create_with_value() {
        let val = eval_ok("Lax(42)");
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields.iter().find(|(n, _)| n == "hasValue").map(|(_, v)| v),
                Some(&Value::Bool(true))
            );
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__value").map(|(_, v)| v),
                Some(&Value::Int(42))
            );
            assert_eq!(
                fields
                    .iter()
                    .find(|(n, _)| n == "__default")
                    .map(|(_, v)| v),
                Some(&Value::Int(0))
            );
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__type").map(|(_, v)| v),
                Some(&Value::str("Lax".into()))
            );
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_lax_mold_inst() {
        // Lax[42]() via MoldInst syntax
        let val = eval_ok("Lax[42]()");
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields.iter().find(|(n, _)| n == "hasValue").map(|(_, v)| v),
                Some(&Value::Bool(true))
            );
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__value").map(|(_, v)| v),
                Some(&Value::Int(42))
            );
            assert_eq!(
                fields
                    .iter()
                    .find(|(n, _)| n == "__default")
                    .map(|(_, v)| v),
                Some(&Value::Int(0))
            );
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_lax_has_value() {
        assert_eq!(eval_ok("Lax(42).hasValue()"), Value::Bool(true));
        assert_eq!(eval_ok("Lax[42]().hasValue()"), Value::Bool(true));
    }

    #[test]
    fn test_lax_is_empty() {
        assert_eq!(eval_ok("Lax(42).isEmpty()"), Value::Bool(false));
    }

    #[test]
    fn test_lax_get_or_default() {
        assert_eq!(eval_ok("Lax(42).getOrDefault(0)"), Value::Int(42));
    }

    #[test]
    fn test_lax_unmold_forward() {
        let source = r#"
lax <= Lax[42]()
lax ]=> value
value
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_lax_unmold_backward() {
        let source = r#"
lax <= Lax[42]()
value <=[ lax
value
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_lax_map() {
        let source = r#"
double x = x * 2 => :Int
Lax(21).map(double).getOrDefault(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_lax_flat_map() {
        let source = r#"
safeLax x = Lax(x * 2) => :Lax
Lax(21).flatMap(safeLax).getOrDefault(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_lax_to_string() {
        assert_eq!(eval_ok("Lax(42).toString()"), Value::str("Lax(42)".into()));
    }

    #[test]
    fn test_lax_typeof() {
        assert_eq!(eval_ok("typeof(Lax(42))"), Value::str("Lax".into()));
    }

    #[test]
    fn test_lax_string_default() {
        let val = eval_ok(r#"Lax("hello")"#);
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields
                    .iter()
                    .find(|(n, _)| n == "__default")
                    .map(|(_, v)| v),
                Some(&Value::str(String::new()))
            );
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_lax_unmold_method() {
        assert_eq!(eval_ok("Lax(42).unmold()"), Value::Int(42));
    }

    // ── Prelude: Result (operation mold with throw) ──

    #[test]
    fn test_prelude_ok() {
        let val = eval_ok("Result[42]()");
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__value").map(|(_, v)| v),
                Some(&Value::Int(42))
            );
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__type").map(|(_, v)| v),
                Some(&Value::str("Result".into()))
            );
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_prelude_err() {
        let source = r#"
Error => NotFound = @(message: Str)
Result[0](throw <= NotFound(message <= "not found"))
"#;
        let val = eval_ok(source);
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__type").map(|(_, v)| v),
                Some(&Value::str("Result".into()))
            );
            // throw field should contain an error
            let throw_val = fields.iter().find(|(n, _)| n == "throw").map(|(_, v)| v);
            assert!(throw_val.is_some());
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_result_is_success() {
        assert_eq!(eval_ok("Result[42]().isSuccess()"), Value::Bool(true));
        let source = r#"
Error => Fail = @(message: Str)
Result[0](throw <= Fail(message <= "fail")).isSuccess()
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_result_is_error() {
        assert_eq!(eval_ok("Result[42]().isError()"), Value::Bool(false));
        let source = r#"
Error => Fail = @(message: Str)
Result[0](throw <= Fail(message <= "fail")).isError()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_result_get_or_default() {
        assert_eq!(eval_ok("Result[42]().getOrDefault(0)"), Value::Int(42));
        let source = r#"
Error => Fail = @(message: Str)
Result[0](throw <= Fail(message <= "fail")).getOrDefault(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_result_get_or_throw() {
        assert_eq!(eval_ok("Result[42]().getOrThrow()"), Value::Int(42));
        // Result with throw should produce an unhandled error
        let source = r#"
Error => Fail = @(message: Str)
Result[0](throw <= Fail(message <= "fail")).getOrThrow()
"#;
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        assert!(interp.eval_program(&program).is_err());
    }

    #[test]
    fn test_result_map() {
        let source = r#"
double x = x * 2 => :Int
Result[21]().map(double).getOrDefault(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_result_map_error_passthrough() {
        let source = r#"
Error => Fail = @(message: Str)
double x = x * 2 => :Int
Result[0](throw <= Fail(message <= "fail")).map(double).isError()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_result_map_error() {
        let source = r#"
Error => Fail = @(message: Str)
addPrefix msg = "Error: " + msg => :Str
Result[0](throw <= Fail(message <= "fail")).mapError(addPrefix).toString()
"#;
        assert_eq!(
            eval_ok(source),
            Value::str("Result(throw <= Error: fail)".into())
        );
    }

    #[test]
    fn test_result_to_string() {
        assert_eq!(
            eval_ok("Result[42]().toString()"),
            Value::str("Result(42)".into())
        );
        let source = r#"
Error => NotFound = @(message: Str)
Result[0](throw <= NotFound(message <= "not found")).toString()
"#;
        assert_eq!(
            eval_ok(source),
            Value::str("Result(throw <= not found)".into())
        );
    }

    // ── Result[T, P] predicate evaluation (v0.8.0) ──

    #[test]
    fn test_result_predicate_always_true() {
        // _ = true is a lambda that always returns true
        // Result[42, _ = true]() ]=> value → 42
        let source = r#"
Result[42, _ = true]() ]=> value
value
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_result_predicate_always_false_unmold() {
        // _ = false is a lambda that always returns false
        // Result[0, _ = false]() ]=> value → throw (predicate fails)
        let source = r#"
|== error: Error =
  | _ |> -1
=> :Int
Result[0, _ = false]() ]=> value
value
"#;
        assert_eq!(eval_ok(source), Value::Int(-1));
    }

    #[test]
    fn test_result_predicate_validation() {
        // _ x = x >= 18 — age validation predicate
        let source = r#"
Result[25, _ x = x >= 18]() ]=> validAge
validAge
"#;
        assert_eq!(eval_ok(source), Value::Int(25));
    }

    #[test]
    fn test_result_predicate_validation_fail() {
        // Age 15 fails >= 18 check
        let source = r#"
|== error: Error =
  | _ |> -1
=> :Int
Result[15, _ x = x >= 18]() ]=> validAge
validAge
"#;
        assert_eq!(eval_ok(source), Value::Int(-1));
    }

    #[test]
    fn test_result_predicate_with_throw() {
        // Predicate fails + explicit throw → throw the explicit error
        let source = r#"
Error => ValidationError = @(field: Str)
|== error: Error =
  | _ |> "caught"
=> :Str
Result[15, _ x = x >= 18](throw <= ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age")) ]=> validAge
validAge.toString()
"#;
        assert_eq!(eval_ok(source), Value::str("caught".into()));
    }

    #[test]
    fn test_result_predicate_assign_no_eval() {
        // => assigns Result as-is, predicate not evaluated
        let source = r#"
Result[15, _ x = x >= 18]() => result
result.isSuccess()
"#;
        // When isSuccess is called, it evaluates the predicate
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_result_predicate_is_success() {
        assert_eq!(
            eval_ok("Result[25, _ x = x >= 18]().isSuccess()"),
            Value::Bool(true)
        );
        assert_eq!(
            eval_ok("Result[15, _ x = x >= 18]().isSuccess()"),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_result_predicate_is_error() {
        assert_eq!(
            eval_ok("Result[25, _ x = x >= 18]().isError()"),
            Value::Bool(false)
        );
        assert_eq!(
            eval_ok("Result[15, _ x = x >= 18]().isError()"),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_result_predicate_get_or_default() {
        assert_eq!(
            eval_ok("Result[42, _ = true]().getOrDefault(0)"),
            Value::Int(42)
        );
        assert_eq!(
            eval_ok("Result[42, _ = false]().getOrDefault(0)"),
            Value::Int(0)
        );
    }

    #[test]
    fn test_result_predicate_get_or_throw() {
        assert_eq!(
            eval_ok("Result[42, _ = true]().getOrThrow()"),
            Value::Int(42)
        );
        // Predicate fails → getOrThrow should throw
        let source = r#"
Result[42, _ = false]().getOrThrow()
"#;
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        assert!(interp.eval_program(&program).is_err());
    }

    #[test]
    fn test_result_predicate_backward_compat() {
        // Result[42]() with no predicate — still works as before
        assert_eq!(eval_ok("Result[42]().isSuccess()"), Value::Bool(true));
        assert_eq!(eval_ok("Result[42]().getOrDefault(0)"), Value::Int(42));
        let source = "Result[42]() ]=> value\nvalue";
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_result_predicate_map() {
        // map on success Result with predicate
        let source = r#"
double x = x * 2 => :Int
Result[21, _ = true]().map(double).getOrDefault(0)
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_result_predicate_map_on_failed() {
        // map on failed Result (predicate = false) should pass through
        let source = r#"
double x = x * 2 => :Int
Result[21, _ = false]().map(double).isError()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_result_predicate_to_string() {
        assert_eq!(
            eval_ok("Result[42, _ = true]().toString()"),
            Value::str("Result(42)".into())
        );
        // _ = false means predicate fails → toString should show throw
        let source = "Result[42, _ = false]().toString()";
        let result = eval_ok(source);
        // Predicate fails, no explicit throw set → should show error
        if let Value::Str(s) = result {
            assert!(
                s.contains("throw"),
                "Expected throw in toString, got: {}",
                s
            );
        } else {
            panic!("Expected Str, got {:?}", result);
        }
    }

    // ── Prelude: HashMap ──

    #[test]
    fn test_prelude_hashmap_empty() {
        let val = eval_ok("hashMap()");
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__type").map(|(_, v)| v),
                Some(&Value::str("HashMap".into()))
            );
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_hashmap_set_and_get() {
        let source = r#"
m <= hashMap()
m2 <= m.set("name", "Alice")
m2.get("name").getOrDefault("")
"#;
        assert_eq!(eval_ok(source), Value::str("Alice".into()));
    }

    #[test]
    fn test_hashmap_get_missing() {
        let source = r#"
m <= hashMap()
m.get("missing").getOrDefault("default")
"#;
        assert_eq!(eval_ok(source), Value::str("default".into()));
    }

    #[test]
    fn test_hashmap_has() {
        let source = r#"
m <= hashMap().set("a", 1)
@[m.has("a"), m.has("b")]
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Bool(true), Value::Bool(false)])
        );
    }

    #[test]
    fn test_hashmap_remove() {
        let source = r#"
m <= hashMap().set("a", 1).set("b", 2).remove("a")
m.has("a")
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_hashmap_keys_values() {
        let source = r#"
m <= hashMap().set("a", 1).set("b", 2)
m.keys().length()
"#;
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_hashmap_size() {
        let source = r#"
m <= hashMap().set("a", 1).set("b", 2)
m.size()
"#;
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_hashmap_is_empty() {
        assert_eq!(eval_ok("hashMap().isEmpty()"), Value::Bool(true));
        assert_eq!(
            eval_ok(r#"hashMap().set("a", 1).isEmpty()"#),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_hashmap_merge() {
        let source = r#"
m1 <= hashMap().set("a", 1).set("b", 2)
m2 <= hashMap().set("b", 3).set("c", 4)
merged <= m1.merge(m2)
@[merged.get("a").getOrDefault(0), merged.get("b").getOrDefault(0), merged.get("c").getOrDefault(0)]
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(1), Value::Int(3), Value::Int(4)])
        );
    }

    #[test]
    fn test_hashmap_entries() {
        let source = r#"
m <= hashMap().set("a", 1)
entries <= m.entries()
entries.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(1));
    }

    // ── Prelude: Set ──

    #[test]
    fn test_prelude_set_of() {
        let val = eval_ok("setOf(@[1, 2, 3])");
        if let Value::BuchiPack(fields) = &val {
            assert_eq!(
                fields.iter().find(|(n, _)| n == "__type").map(|(_, v)| v),
                Some(&Value::str("Set".into()))
            );
        } else {
            panic!("Expected BuchiPack, got {:?}", val);
        }
    }

    #[test]
    fn test_set_dedup() {
        assert_eq!(eval_ok("setOf(@[1, 2, 2, 3, 3]).size()"), Value::Int(3));
    }

    #[test]
    fn test_set_has() {
        let source = r#"
s <= setOf(@[1, 2, 3])
@[s.has(2), s.has(5)]
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Bool(true), Value::Bool(false)])
        );
    }

    #[test]
    fn test_set_add() {
        assert_eq!(eval_ok("setOf(@[1, 2]).add(3).size()"), Value::Int(3));
        // Adding existing item should not change size
        assert_eq!(eval_ok("setOf(@[1, 2]).add(2).size()"), Value::Int(2));
    }

    #[test]
    fn test_set_remove() {
        assert_eq!(eval_ok("setOf(@[1, 2, 3]).remove(2).size()"), Value::Int(2));
    }

    #[test]
    fn test_set_union() {
        assert_eq!(
            eval_ok("setOf(@[1, 2]).union(setOf(@[2, 3])).size()"),
            Value::Int(3)
        );
    }

    #[test]
    fn test_set_intersect() {
        assert_eq!(
            eval_ok("setOf(@[1, 2, 3]).intersect(setOf(@[2, 3, 4])).size()"),
            Value::Int(2)
        );
    }

    #[test]
    fn test_set_diff() {
        assert_eq!(
            eval_ok("setOf(@[1, 2, 3]).diff(setOf(@[2, 3, 4])).size()"),
            Value::Int(1)
        );
    }

    #[test]
    fn test_set_to_list() {
        let val = eval_ok("setOf(@[1, 2, 3]).toList()");
        if let Value::List(items) = val {
            assert_eq!(items.len(), 3);
        } else {
            panic!("Expected List, got {:?}", val);
        }
    }

    #[test]
    fn test_set_is_empty() {
        assert_eq!(eval_ok("setOf(@[]).isEmpty()"), Value::Bool(true));
        assert_eq!(eval_ok("setOf(@[1]).isEmpty()"), Value::Bool(false));
    }

    // ── Prelude: Utility functions ──

    #[test]
    fn test_prelude_typeof() {
        assert_eq!(eval_ok("typeof(42)"), Value::str("Int".into()));
        assert_eq!(eval_ok("typeof(3.14)"), Value::str("Float".into()));
        assert_eq!(eval_ok(r#"typeof("hello")"#), Value::str("Str".into()));
        assert_eq!(eval_ok("typeof(true)"), Value::str("Bool".into()));
        assert_eq!(eval_ok("typeof(@[1,2])"), Value::str("List".into()));
        assert_eq!(eval_ok("typeof(@(x <= 1))"), Value::str("BuchiPack".into()));
        // Optional is abolished — typeof(Optional[1]()) would error
        assert_eq!(eval_ok("typeof(Lax[1]())"), Value::str("Lax".into()));
        assert_eq!(eval_ok("typeof(Result[1]())"), Value::str("Result".into()));
        assert_eq!(eval_ok("typeof(hashMap())"), Value::str("HashMap".into()));
        assert_eq!(eval_ok("typeof(setOf(@[]))"), Value::str("Set".into()));
    }

    #[test]
    fn test_prelude_range() {
        assert_eq!(
            eval_ok("range(0, 5)"),
            Value::list(vec![
                Value::Int(0),
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
            ])
        );
        assert_eq!(eval_ok("range(3, 3)"), Value::list(vec![]));
        assert_eq!(eval_ok("range(0, 1)"), Value::list(vec![Value::Int(0)]));
    }

    #[test]
    fn test_prelude_enumerate() {
        let source = r#"
items <= enumerate(@["a", "b", "c"])
items.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_prelude_enumerate_get_access() {
        let source = r#"
items <= enumerate(@["a", "b"])
first <= items.get(0).unmold()
first.index
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_prelude_zip() {
        let source = r#"
pairs <= zip(@[1, 2], @["a", "b"])
pairs.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_prelude_zip_field_access() {
        let source = r#"
pairs <= zip(@[1, 2], @["a", "b"])
first <= pairs.get(0).unmold()
first.first
"#;
        assert_eq!(eval_ok(source), Value::Int(1));
    }

    #[test]
    fn test_prelude_assert_pass() {
        assert_eq!(eval_ok("assert(true, \"ok\")"), Value::Bool(true));
    }

    #[test]
    fn test_prelude_assert_fail() {
        // assert(false, msg) should produce an unhandled error
        let (program, errors) = crate::parser::parse(r#"assert(false, "must be true")"#);
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        let result = interp.eval_program(&program);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("must be true"));
    }

    #[test]
    fn test_prelude_debug_returns_value() {
        let source = r#"
x <= debug(42)
x + 1
"#;
        let (val, output) = eval_with_output(source);
        assert_eq!(val, Value::Int(43));
        assert_eq!(output, vec!["42"]);
    }

    // ── std dissolution: no backward compat ──

    #[test]
    fn test_no_backward_compat_std_io() {
        let source = r#">>> std/io => @(stdout)"#;
        let result = eval(source);
        assert!(result.is_err(), "std/io should error after dissolution");
    }

    // ── Prelude builtins (no import needed) ──

    #[test]
    fn test_prelude_stdout_no_import() {
        // stdout is now a prelude builtin, no import needed
        let source = r#"
stdout("hello prelude")
"#;
        let (_, output) = eval_with_output(source);
        assert_eq!(output, vec!["hello prelude"]);
    }

    #[test]
    fn test_prelude_stderr_no_import() {
        // stderr is now a prelude builtin.
        // C12-5 (FB-18): stderr now returns the payload byte count (Int)
        // instead of `Value::Unit`. Migration test: previously this asserted
        // `Value::Unit`, now we pin the Int(len("error msg")) contract.
        let source = r#"
stderr("error msg")
"#;
        let (val, _) = eval_with_output(source);
        assert_eq!(val, Value::Int("error msg".len() as i64));
    }

    #[test]
    fn test_prelude_nowms_returns_epoch_int() {
        let v = eval_ok("nowMs()");
        match v {
            Value::Int(ms) => assert!(
                ms > 946684800000,
                "nowMs should be unix epoch millis: {}",
                ms
            ),
            other => panic!("Expected Int from nowMs(), got {:?}", other),
        }
    }

    #[test]
    fn test_prelude_nowms_rejects_arguments() {
        let result = eval("nowMs(1)");
        assert!(result.is_err(), "nowMs should reject extra arguments");
    }

    #[test]
    fn test_sha256_without_crypto_import_fails() {
        let result = eval(r#"sha256("abc")"#);
        assert!(
            result.is_err(),
            "sha256 should require taida-lang/crypto import"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Undefined variable: 'sha256'"),
            "expected undefined variable error, got: {}",
            msg
        );
    }

    #[test]
    fn test_prelude_sleep_zero_unmolds_to_unit() {
        let v = eval_ok(
            r#"
s <= sleep(0)
s ]=> done
done
"#,
        );
        assert_eq!(v, Value::Unit);
    }

    #[test]
    fn test_prelude_sleep_waits_short_duration() {
        let start = Instant::now();
        let v = eval_ok(
            r#"
s <= sleep(20)
s ]=> done
done
"#,
        );
        assert_eq!(v, Value::Unit);
        assert!(
            start.elapsed() >= Duration::from_millis(10),
            "sleep(20) should wait at least ~10ms"
        );
    }

    #[test]
    fn test_prelude_sleep_negative_rejected_with_range_error() {
        let v = eval_ok(
            r#"
handle =
  |== error: Error =
    error.type
  => :Str

  s <= sleep(-1)
  s ]=> waited
  "ok"
=> :Str

handle()
"#,
        );
        assert_eq!(v, Value::str("RangeError".to_string()));
    }

    #[test]
    fn test_prelude_sleep_too_large_rejected_with_range_error() {
        let v = eval_ok(
            r#"
handle =
  |== error: Error =
    error.type
  => :Str

  s <= sleep(2147483648)
  s ]=> waited
  "ok"
=> :Str

handle()
"#,
        );
        assert_eq!(v, Value::str("RangeError".to_string()));
    }

    #[test]
    fn test_prelude_json_parse_abolished() {
        // jsonParse is abolished (Molten Iron)
        let source = r#"result <= jsonParse("{\"x\": 42}")"#;
        let result = eval(source);
        assert!(result.is_err(), "jsonParse should error");
    }

    #[test]
    fn test_prelude_json_encode_no_import() {
        // jsonEncode is now a prelude builtin
        let source = r#"
data <= @(name <= "Bob")
result <= jsonEncode(data)
result
"#;
        if let Value::Str(s) = eval_ok(source) {
            assert!(s.contains("Bob"), "Should contain Bob");
        } else {
            panic!("Expected Str");
        }
    }

    #[test]
    fn test_prelude_json_decode_abolished() {
        // jsonDecode is abolished (Molten Iron)
        let source = r#"result <= jsonDecode("{\"y\": 99}")"#;
        let result = eval(source);
        assert!(result.is_err(), "jsonDecode should error");
    }

    // ── User-defined type methods ──

    #[test]
    fn test_method_no_args_field_access() {
        let source = r#"
Greeter = @(
  name: Str
  greet =
    name
  => :Str
)
g <= Greeter(name <= "Alice")
result <= g.greet()
"#;
        assert_eq!(eval_ok(source), Value::str("Alice".to_string()));
    }

    #[test]
    fn test_method_with_args() {
        let source = r#"
Calc = @(
  base: Int
  add x: Int =
    base + x
  => :Int
)
c <= Calc(base <= 10)
result <= c.add(5)
"#;
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    #[test]
    fn test_inherited_method() {
        let source = r#"
Person = @(
  name: Str
  age: Int
  greet =
    name
  => :Str
)
Person => Employee = @(
  department: Str
)
e <= Employee(name <= "Bob", age <= 30, department <= "Eng")
result <= e.greet()
"#;
        assert_eq!(eval_ok(source), Value::str("Bob".to_string()));
    }

    #[test]
    fn test_inherited_method_parent_field_access() {
        let source = r#"
Person = @(
  name: Str
  age: Int
)
Person => Pilot = @(
  seq: Int
  info =
    name + " #" + seq.toString()
  => :Str
)
p <= Pilot(name <= "Asuka", age <= 14, seq <= 2)
result <= p.info()
"#;
        assert_eq!(eval_ok(source), Value::str("Asuka #2".to_string()));
    }

    #[test]
    fn test_method_override() {
        let source = r#"
Base = @(
  val: Int
  compute =
    val
  => :Int
)
Base => Child = @(
  extra: Int
  compute =
    val + extra
  => :Int
)
c <= Child(val <= 10, extra <= 5)
result <= c.compute()
"#;
        assert_eq!(eval_ok(source), Value::Int(15));
    }

    // ── JSON Molten Iron tests ──────────────────────────────
    // JSON is opaque. Must be cast through schema: JSON[raw, Schema]()

    #[test]
    fn test_json_schema_cast_basic() {
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":"Alice","age":30}'
JSON[raw, User]() ]=> user
user.name
"#;
        assert_eq!(eval_ok(source), Value::str("Alice".to_string()));
    }

    #[test]
    fn test_json_schema_cast_age() {
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":"Alice","age":30}'
JSON[raw, User]() ]=> user
user.age
"#;
        assert_eq!(eval_ok(source), Value::Int(30));
    }

    #[test]
    fn test_json_schema_extra_fields_ignored() {
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":"Alice","age":30,"extra":"ignored"}'
JSON[raw, User]() ]=> user
user.name
"#;
        assert_eq!(eval_ok(source), Value::str("Alice".to_string()));
    }

    #[test]
    fn test_json_schema_missing_fields_default() {
        let source = r#"
User = @(name: Str, age: Int, email: Str)
raw <= '{"name":"Asuka"}'
JSON[raw, User]() ]=> user
user.age
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_json_schema_type_mismatch_default() {
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":"Asuka","age":"not a number"}'
JSON[raw, User]() ]=> user
user.age
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_json_schema_null_to_default() {
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":null,"age":null}'
JSON[raw, User]() ]=> user
user.name
"#;
        assert_eq!(eval_ok(source), Value::str(String::new()));
    }

    #[test]
    fn test_json_schema_nested() {
        let source = r#"
Address = @(city: Str, zip: Str)
User = @(name: Str, address: Address)
raw <= '{"name":"Asuka","address":{"city":"Tokyo-3","zip":"999"}}'
JSON[raw, User]() ]=> user
user.address.city
"#;
        assert_eq!(eval_ok(source), Value::str("Tokyo-3".to_string()));
    }

    #[test]
    fn test_json_schema_list() {
        let source = r#"
Pilot = @(name: Str, syncRate: Int)
raw <= '[{"name":"Asuka","syncRate":78},{"name":"Rei","syncRate":65}]'
JSON[raw, @[Pilot]]() ]=> pilots
pilots.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_json_schema_list_element_access() {
        let source = r#"
Pilot = @(name: Str, syncRate: Int)
raw <= '[{"name":"Asuka","syncRate":78},{"name":"Rei","syncRate":65}]'
JSON[raw, @[Pilot]]() ]=> pilots
pilots.get(0) ]=> first
first.name
"#;
        assert_eq!(eval_ok(source), Value::str("Asuka".to_string()));
    }

    #[test]
    fn test_json_schema_primitive_int() {
        let source = r#"
raw <= '42'
JSON[raw, Int]() ]=> num
num
"#;
        assert_eq!(eval_ok(source), Value::Int(42));
    }

    #[test]
    fn test_json_schema_primitive_str() {
        let source = r#"
raw <= '"hello"'
JSON[raw, Str]() ]=> s
s
"#;
        assert_eq!(eval_ok(source), Value::str("hello".to_string()));
    }

    #[test]
    fn test_json_schema_returns_lax() {
        let source = r#"
User = @(name: Str)
raw <= '{"name":"Alice"}'
result <= JSON[raw, User]()
result.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_json_schema_parse_error_returns_lax_false() {
        let source = r#"
User = @(name: Str)
raw <= 'not valid json'
result <= JSON[raw, User]()
result.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_json_no_args_errors() {
        let source = r#"JSON()"#;
        let result = eval(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_one_arg_errors() {
        let source = r#"JSON['{"x":1}']() "#;
        let result = eval(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_same_raw_multiple_schemas() {
        let source = r#"
UserInfo = @(name: Str, age: Int)
StatsInfo = @(score: Float, rank: Int)
raw <= '{"name":"Asuka","age":14,"score":95.5,"rank":3}'
JSON[raw, UserInfo]() ]=> user
JSON[raw, StatsInfo]() ]=> stats
user.name
"#;
        assert_eq!(eval_ok(source), Value::str("Asuka".to_string()));
    }

    #[test]
    fn test_json_schema_int_list() {
        let source = r#"
raw <= '[1, 2, 3]'
JSON[raw, @[Int]]() ]=> nums
nums.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_json_encode_still_works() {
        let source = r#"
data <= @(name <= "Alice")
result <= jsonEncode(data)
result
"#;
        if let Value::Str(s) = eval_ok(source) {
            assert!(s.contains("Alice"));
        } else {
            panic!("Expected Str");
        }
    }

    // ── BT-12: JSON schema edge case tests ──────────────────

    #[test]
    fn test_bt12_json_empty_object_schema() {
        // JSON['{}', Schema]() — empty object should produce a BuchiPack with default values
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{}'
result <= JSON[raw, User]()
result.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_bt12_json_empty_object_defaults() {
        // Empty JSON object fields should fall back to type defaults
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{}'
JSON[raw, User]() ]=> user
user.name
"#;
        // name should be "" (default for Str)
        assert_eq!(eval_ok(source), Value::str(String::new()));
    }

    #[test]
    fn test_bt12_json_empty_array_schema() {
        // JSON['[]', @[Int]]() — empty array
        let source = r#"
raw <= '[]'
JSON[raw, @[Int]]() ]=> nums
nums.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_bt12_json_malformed_input() {
        // JSON[malformed, Schema]() — invalid JSON should return Lax with hasValue=false
        let source = r#"
User = @(name: Str)
raw <= '{invalid json}'
result <= JSON[raw, User]()
result.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_bt12_json_null_field_to_default() {
        // JSON['{"name":null}', @(name: Str)]() — null field should become default
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":null,"age":null}'
JSON[raw, User]() ]=> user
user.name
"#;
        // null should become "" (Str default)
        assert_eq!(eval_ok(source), Value::str(String::new()));
    }

    #[test]
    fn test_bt12_json_null_field_int_default() {
        // null int field should become 0 (Int default)
        let source = r#"
User = @(name: Str, age: Int)
raw <= '{"name":"Alice","age":null}'
JSON[raw, User]() ]=> user
user.age
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    // ── IndexAccess removed (v0.5.0) — .get() returns Lax ──────────────────

    #[test]
    fn test_oob_str_list_get_returns_lax() {
        // .get(99) returns Lax with hasValue=false (no IndexError)
        let source = r#"result <= @["a", "b"].get(99).hasValue"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_oob_float_list_get_returns_lax() {
        let source = r#"result <= @[1.5, 2.5].get(99).hasValue"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_oob_bool_list_get_returns_lax() {
        let source = r#"result <= @[true, false].get(99).hasValue"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_oob_int_list_get_returns_lax() {
        let source = r#"result <= @[1, 2, 3].get(99).hasValue"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_list_first_str_list() {
        // first() returns Lax — .unmold() extracts value
        let source = r#"
items <= @["hello", "world"]
result <= items.first().unmold()
"#;
        assert_eq!(eval_ok(source), Value::str("hello".to_string()));
    }

    #[test]
    fn test_list_first_empty_returns_lax() {
        // first() on empty list returns Lax with hasValue=false (no IndexError)
        let source = r#"
items <= @[]
result <= items.first()
result.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_list_last_returns_lax() {
        let source = r#"
items <= @[1, 2, 3]
result <= items.last().unmold()
"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_list_get_oob_returns_lax_false() {
        // get() on OOB returns Lax with hasValue=false
        let source = r#"
items <= @["a", "b", "c"]
result <= items.get(99)
"#;
        let val = eval_ok(source);
        match val {
            Value::BuchiPack(fields) => {
                let has_value = fields.iter().find(|(k, _)| k == "hasValue");
                assert_eq!(has_value.map(|(_, v)| v), Some(&Value::Bool(false)));
                let typ = fields.iter().find(|(k, _)| k == "__type");
                assert_eq!(typ.map(|(_, v)| v), Some(&Value::str("Lax".to_string())));
            }
            other => panic!("Expected BuchiPack (Lax), got {:?}", other),
        }
    }

    #[test]
    fn test_list_get_valid_returns_lax_true() {
        // get() on valid index returns Lax with hasValue=true
        let source = r#"
items <= @["a", "b", "c"]
result <= items.get(1)
"#;
        let val = eval_ok(source);
        match val {
            Value::BuchiPack(fields) => {
                let has_value = fields.iter().find(|(k, _)| k == "hasValue");
                assert_eq!(has_value.map(|(_, v)| v), Some(&Value::Bool(true)));
                let inner = fields.iter().find(|(k, _)| k == "__value");
                assert_eq!(inner.map(|(_, v)| v), Some(&Value::str("b".to_string())));
                let typ = fields.iter().find(|(k, _)| k == "__type");
                assert_eq!(typ.map(|(_, v)| v), Some(&Value::str("Lax".to_string())));
            }
            other => panic!("Expected BuchiPack (Lax), got {:?}", other),
        }
    }

    #[test]
    fn test_list_max_min_str() {
        // max() returns Lax — .unmold() extracts value
        let source = r#"
items <= @["cherry", "apple", "banana"]
result <= items.max().unmold()
"#;
        assert_eq!(eval_ok(source), Value::str("cherry".to_string()));
    }

    // ── Div/Mod mold tests ──

    #[test]
    fn test_div_mold_int() {
        let source = r#"
r <= Div[10, 3]()
result <= r.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_div_mold_zero_division() {
        let source = r#"
r <= Div[10, 0]()
result <= r.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_div_mold_unmold() {
        let source = r#"
r <= Div[10, 3]()
r ]=> result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_div_mold_zero_unmold() {
        let source = r#"
r <= Div[10, 0]()
r ]=> result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(0));
    }

    #[test]
    fn test_mod_mold_int() {
        let source = r#"
r <= Mod[17, 5]()
r ]=> result
result
"#;
        assert_eq!(eval_ok(source), Value::Int(2));
    }

    #[test]
    fn test_mod_mold_zero() {
        let source = r#"
r <= Mod[17, 0]()
result <= r.hasValue
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_div_mold_float() {
        let source = r#"
r <= Div[10.0, 3.0]()
r ]=> result
result
"#;
        let val = eval_ok(source);
        match val {
            Value::Float(f) => assert!((f - 3.333333333).abs() < 0.01),
            other => panic!("Expected Float, got {:?}", other),
        }
    }

    #[test]
    fn test_div_mold_mixed_types() {
        let source = r#"
r <= Div[10, 3.0]()
r ]=> result
result
"#;
        let val = eval_ok(source);
        match val {
            Value::Float(f) => assert!((f - 3.333333333).abs() < 0.01),
            other => panic!("Expected Float, got {:?}", other),
        }
    }

    // ── define_force() scope integrity (C-3) ──

    #[test]
    fn test_define_force_error_ceiling_scope() {
        // Error ceiling parameter should not leak out of handler scope
        let source = r#"
error_var <= "outer"
Error => MyErr = @(info: Str)
testFn dummy: Int =
  |== error: Error =
    | _ |> "caught"
  => :Str
  MyErr(type <= "MyErr", message <= "fail", info <= "x").throw()
  "unreachable"
=> :Str
result <= testFn(0)
error_var
"#;
        assert_eq!(eval_ok(source), Value::str("outer".into()));
    }

    #[test]
    fn test_immutability_enforced() {
        // Re-assignment in the same scope should error
        let source = "x <= 42\nx <= 43";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty());
        let mut interp = crate::interpreter::eval::Interpreter::new();
        assert!(interp.eval_program(&program).is_err());
    }

    #[test]
    fn test_define_force_pipeline_scope() {
        // Pipeline temporary variables should not leak
        let source = r#"
items <= @[1, 2, 3]
result <= Map[items, _ x = x * 2]()
result
"#;
        let val = eval_ok(source);
        assert_eq!(
            val,
            Value::list(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
    }

    // ── True Async (tokio integration) ──────────────────────

    #[test]
    fn test_async_pending_resolves_via_tokio() {
        // Create a pending Async with a oneshot channel, send a value,
        // then unmold it — the interpreter should block_on and get the value.
        use crate::interpreter::value::{AsyncStatus, AsyncValue, PendingState};
        use std::sync::{Arc, Mutex};

        let (tx, rx) = tokio::sync::oneshot::channel();
        let pending = Value::Async(AsyncValue {
            status: AsyncStatus::Pending,
            value: Box::new(Value::Unit),
            error: Box::new(Value::Unit),
            task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
        });

        // Send the result from another thread
        std::thread::spawn(move || {
            tx.send(Ok(Value::Int(42))).unwrap();
        });

        // The interpreter should resolve the pending async via block_on
        let mut interpreter = crate::interpreter::eval::Interpreter::new();
        interpreter.env.define("pending_val", pending).unwrap();

        let (program, errors) = crate::parser::parse("pending_val ]=> result\nresult");
        assert!(errors.is_empty());
        let val = interpreter.eval_program(&program).unwrap();
        assert_eq!(val, Value::Int(42));
    }

    #[test]
    fn test_async_pending_rejected_via_tokio() {
        // A pending async that resolves to an error should throw.
        use crate::interpreter::value::{AsyncStatus, AsyncValue, PendingState};
        use std::sync::{Arc, Mutex};

        let (tx, rx) = tokio::sync::oneshot::channel();
        let async_val = AsyncValue {
            status: AsyncStatus::Pending,
            value: Box::new(Value::Unit),
            error: Box::new(Value::Unit),
            task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
        };

        // Send an error result from another thread
        std::thread::spawn(move || {
            tx.send(Err("network error".to_string())).unwrap();
        });

        // Directly test resolve_async
        let interpreter = crate::interpreter::eval::Interpreter::new();
        let resolved = interpreter.resolve_async(&async_val).unwrap();
        assert_eq!(resolved.status, AsyncStatus::Rejected);
        if let Value::Error(e) = &*resolved.error {
            assert_eq!(e.message, "network error");
        } else {
            panic!("Expected Error value, got {:?}", resolved.error);
        }
    }

    #[test]
    fn test_async_pending_timeout_success() {
        // Timeout should succeed when task completes within time limit.
        use crate::interpreter::value::{AsyncStatus, AsyncValue, PendingState};
        use std::sync::{Arc, Mutex};

        let (tx, rx) = tokio::sync::oneshot::channel();
        let async_val = AsyncValue {
            status: AsyncStatus::Pending,
            value: Box::new(Value::Unit),
            error: Box::new(Value::Unit),
            task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
        };

        // Send immediately — well within any timeout
        std::thread::spawn(move || {
            tx.send(Ok(Value::str("done".into()))).unwrap();
        });

        let interpreter = crate::interpreter::eval::Interpreter::new();
        let result = interpreter
            .resolve_async_with_timeout(&async_val, 5000)
            .unwrap();
        assert!(result.is_some());
        let resolved = result.unwrap();
        assert_eq!(resolved.status, AsyncStatus::Fulfilled);
        assert_eq!(*resolved.value, Value::str("done".into()));
    }

    #[test]
    fn test_async_pending_timeout_expired() {
        // Timeout should return None when task doesn't complete in time.
        use crate::interpreter::value::{AsyncStatus, AsyncValue, PendingState};
        use std::sync::{Arc, Mutex};

        let (_tx, rx) = tokio::sync::oneshot::channel::<Result<Value, String>>();
        let async_val = AsyncValue {
            status: AsyncStatus::Pending,
            value: Box::new(Value::Unit),
            error: Box::new(Value::Unit),
            task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
        };

        // Don't send anything — the timeout should expire
        // Use a very short timeout (10ms) to keep test fast
        let interpreter = crate::interpreter::eval::Interpreter::new();
        let result = interpreter
            .resolve_async_with_timeout(&async_val, 10)
            .unwrap();
        assert!(result.is_none(), "Expected timeout but got: {:?}", result);
    }

    #[test]
    fn test_async_all_with_pending() {
        // All should resolve pending items via tokio before collecting results.
        use crate::interpreter::value::{AsyncStatus, AsyncValue, PendingState};
        use std::sync::{Arc, Mutex};

        let (tx1, rx1) = tokio::sync::oneshot::channel();
        let (tx2, rx2) = tokio::sync::oneshot::channel();

        let list = Value::list(vec![
            Value::Async(AsyncValue {
                status: AsyncStatus::Pending,
                value: Box::new(Value::Unit),
                error: Box::new(Value::Unit),
                task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx1)))),
            }),
            Value::Async(AsyncValue {
                status: AsyncStatus::Pending,
                value: Box::new(Value::Unit),
                error: Box::new(Value::Unit),
                task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx2)))),
            }),
        ]);

        // Send values from another thread
        std::thread::spawn(move || {
            tx1.send(Ok(Value::Int(10))).unwrap();
            tx2.send(Ok(Value::Int(20))).unwrap();
        });

        let mut interpreter = crate::interpreter::eval::Interpreter::new();
        interpreter.env.define("async_list", list).unwrap();

        let (program, errors) = crate::parser::parse("All[async_list]() ]=> results\nresults");
        assert!(errors.is_empty());
        let val = interpreter.eval_program(&program).unwrap();
        assert_eq!(val, Value::list(vec![Value::Int(10), Value::Int(20)]));
    }

    #[test]
    fn test_async_existing_sync_tests_still_pass() {
        // Verify all original synchronous Async patterns still work.
        // Fulfilled
        assert_eq!(eval_ok("Async[42]() ]=> x\nx"), Value::Int(42));
        // All with sync
        let source =
            "asyncs <= @[Async[1](), Async[2](), Async[3]()]\nAll[asyncs]() ]=> results\nresults";
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        // Race
        assert_eq!(eval_ok("Race[@[Async[99]()]]() ]=> w\nw"), Value::Int(99));
        // Timeout (no-op for resolved)
        assert_eq!(
            eval_ok("Timeout[Async[42](), 1000]() ]=> x\nx"),
            Value::Int(42)
        );
        // AsyncReject caught by error ceiling (via function-level |==)
        let source = r#"
handleReject unused =
  |== error: Error =
    "caught"
  => :Str

  AsyncReject["fail"]() ]=> x
  x
=> :Str

handleReject(0)
"#;
        assert_eq!(eval_ok(source), Value::str("caught".into()));
    }

    // ── Stream[T] Mold Type ──

    #[test]
    fn test_stream_single_value() {
        let source = r#"
s <= Stream[42]()
s ]=> result
result
"#;
        assert_eq!(eval_ok(source), Value::list(vec![Value::Int(42)]));
    }

    #[test]
    fn test_stream_from_list() {
        let source = r#"
s <= StreamFrom[@[1, 2, 3]]()
s ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn test_stream_map() {
        let source = r#"
s <= StreamFrom[@[1, 2, 3]]()
mapped <= Map[s, _ x = x * 10]()
mapped ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(10), Value::Int(20), Value::Int(30)])
        );
    }

    #[test]
    fn test_stream_filter() {
        let source = r#"
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
s <= StreamFrom[@[1, 2, 3, 4, 5, 6]]()
filtered <= Filter[s, isEven]()
filtered ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
    }

    #[test]
    fn test_stream_take() {
        let source = r#"
s <= StreamFrom[@[10, 20, 30, 40, 50]]()
taken <= Take[s, 3]()
taken ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(10), Value::Int(20), Value::Int(30)])
        );
    }

    #[test]
    fn test_stream_take_while() {
        let source = r#"
s <= StreamFrom[@[1, 2, 3, 10, 20]]()
tw <= TakeWhile[s, _ x = x < 5]()
tw ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn test_stream_chained_transforms() {
        let source = r#"
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
s <= StreamFrom[@[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]]()
step1 <= Filter[s, isEven]()
step2 <= Map[step1, _ x = x * 10]()
step3 <= Take[step2, 3]()
step3 ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(20), Value::Int(40), Value::Int(60)])
        );
    }

    #[test]
    fn test_stream_methods_tostring() {
        let source = r#"
s <= StreamFrom[@[1, 2, 3]]()
s.toString()
"#;
        assert_eq!(
            eval_ok(source),
            Value::str("Stream[completed: 3 items]".into())
        );
    }

    #[test]
    fn test_stream_methods_length() {
        let source = r#"
s <= StreamFrom[@[1, 2, 3]]()
s.length()
"#;
        assert_eq!(eval_ok(source), Value::Int(3));
    }

    #[test]
    fn test_stream_methods_is_empty() {
        let source = r#"
s <= StreamFrom[@[]]()
s.isEmpty()
"#;
        assert_eq!(eval_ok(source), Value::Bool(true));
    }

    #[test]
    fn test_stream_methods_is_empty_false() {
        let source = r#"
s <= StreamFrom[@[1]]()
s.isEmpty()
"#;
        assert_eq!(eval_ok(source), Value::Bool(false));
    }

    #[test]
    fn test_stream_empty_collect() {
        let source = r#"
s <= StreamFrom[@[]]()
s ]=> result
result
"#;
        assert_eq!(eval_ok(source), Value::list(vec![]));
    }

    #[test]
    fn test_stream_pipeline() {
        // Test chained pipeline with Stream
        let source = r#"
double x = x * 2 => :Int
s <= StreamFrom[@[1, 2, 3, 4, 5]]()
step1 <= Map[s, double]()
step2 <= Take[step1, 3]()
step2 ]=> result
result
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
    }

    #[test]
    fn test_stream_with_output() {
        let source = r#"
s <= StreamFrom[@[10, 20, 30]]()
s ]=> result
stdout(Join[result, " "]())
"#;
        let (_, output) = eval_with_output(source);
        assert_eq!(output, vec!["10 20 30"]);
    }

    #[test]
    fn test_stream_list_molds_unchanged() {
        // Verify List molds still work unchanged when given a list (not a stream)
        let source = r#"
nums <= @[1, 2, 3, 4, 5]
Map[nums, _ x = x * 2]() ]=> mapped
mapped
"#;
        assert_eq!(
            eval_ok(source),
            Value::list(vec![
                Value::Int(2),
                Value::Int(4),
                Value::Int(6),
                Value::Int(8),
                Value::Int(10)
            ])
        );
    }

    // ── F-46 regression: lambda with CondBranch must not eat enclosing scope ──

    #[test]
    fn test_f46_lambda_cond_branch_does_not_break_outer_scope() {
        // A lambda whose body is a CondBranch (| ... |> ... | _ |> ...)
        // should NOT consume the indent tokens of subsequent statements
        // in the enclosing function body.
        let src = r#"
TestType = @(id: Int, name: Str)
Store = @(items: @[TestType])

makeStore dummy =
  @(items <= @[@(id <= 1, name <= "a"), @(id <= 2, name <= "b")])
=> :Store

updateItem targetId state =
  mapper <= _ item = | item.id == targetId |> @(id <= targetId, name <= "updated") | _ |> item
  newItems <= Map[state.items, mapper]()
  stdout(jsonPretty(newItems))
  0
=> :Int

s <= makeStore(0)
updateItem(1, s)
"#;
        let (_val, output) = eval_with_output(src);
        // Should output the JSON-pretty list with item 1 updated
        let joined = output.join("\n");
        assert!(
            joined.contains("\"name\": \"updated\""),
            "Expected updated name in output, got: {}",
            joined
        );
        assert!(
            joined.contains("\"id\": 1"),
            "Expected id 1 in output, got: {}",
            joined
        );
    }

    #[test]
    fn test_f46_all_params_accessible_after_lambda_with_cond_branch() {
        // Ensure ALL function parameters remain accessible after defining
        // a lambda that uses CondBranch in its body.
        let src = r#"
check a b c =
  f <= _ x = | x > 0 |> a | _ |> b
  stdout(`c=${c}`)
  stdout(`f(1)=${f(1)}`)
  stdout(`f(0)=${f(0)}`)
  0
=> :Int

check(10, 20, 30)
"#;
        let (_val, output) = eval_with_output(src);
        assert_eq!(output[0], "c=30");
        assert_eq!(output[1], "f(1)=10");
        assert_eq!(output[2], "f(0)=20");
    }

    #[test]
    fn test_f46_multiline_cond_branch_still_works() {
        // C20-1 (ROOT-5): multi-line rhs guards now require parens. The
        // parenthesised form is the canonical escape hatch and the
        // `medium` semantics the test pins is unchanged.
        let src = r#"
x <= 5
result <= (
  | x > 10 |> "big"
  | x > 3 |> "medium"
  | _ |> "small"
)
stdout(result)
"#;
        let (_val, output) = eval_with_output(src);
        assert_eq!(output[0], "medium");
    }

    // ── F-51 regression: nested CondBranch must not consume outer arms ──

    #[test]
    fn test_f51_nested_cond_branch_outer_wildcard() {
        // Nested pattern match: outer wildcard must not be consumed by the inner CondBranch.
        let src = r#"
test x y =
  | x == "a" |>
    | y == "1" |> "a-1"
    | _ |> "a-other"
  | _ |> "wildcard"
=> :Str
stdout(test("a", "1"))
stdout(test("a", "z"))
stdout(test("b", "1"))
"#;
        let (_val, output) = eval_with_output(src);
        assert_eq!(output[0], "a-1");
        assert_eq!(output[1], "a-other");
        assert_eq!(
            output[2], "wildcard",
            "F-51: outer wildcard must be reached when outer condition is false"
        );
    }

    #[test]
    fn test_f51_nested_cond_branch_three_levels() {
        // Three levels of nesting should all resolve correctly.
        let src = r#"
classify a b c =
  | a == 1 |>
    | b == 1 |>
      | c == 1 |> "all-one"
      | _ |> "a1-b1-cx"
    | _ |> "a1-bx"
  | _ |> "ax"
=> :Str
stdout(classify(1, 1, 1))
stdout(classify(1, 1, 2))
stdout(classify(1, 2, 1))
stdout(classify(2, 1, 1))
"#;
        let (_val, output) = eval_with_output(src);
        assert_eq!(output[0], "all-one");
        assert_eq!(output[1], "a1-b1-cx");
        assert_eq!(output[2], "a1-bx");
        assert_eq!(output[3], "ax");
    }

    // ── F-54 regression: function call in tail position must not be misidentified as mutual tail call ──

    #[test]
    fn test_f54_function_call_in_cond_branch_tail_position() {
        // A non-recursive function call in pattern match tail position
        // should work correctly, not be misidentified as mutual tail call.
        let src = r#"
handleRoot =
  jsonEncode(@(ok <= true))
=> :Str

route method =
  | method == "GET" |> handleRoot()
  | _ |> "error"
=> :Str

stdout(route("GET"))
stdout(route("POST"))
"#;
        let (_val, output) = eval_with_output(src);
        assert_eq!(output[0], "{\"ok\":true}");
        assert_eq!(output[1], "error");
    }

    #[test]
    fn test_f54_different_arity_function_in_tail_position() {
        // Function with different arity called from pattern match in tail position.
        let src = r#"
greet name =
  "hello " + name
=> :Str

dispatch code =
  | code == 1 |> greet("world")
  | code == 2 |> greet("taida")
  | _ |> "unknown"
=> :Str

stdout(dispatch(1))
stdout(dispatch(2))
stdout(dispatch(3))
"#;
        let (_val, output) = eval_with_output(src);
        assert_eq!(output[0], "hello world");
        assert_eq!(output[1], "hello taida");
        assert_eq!(output[2], "unknown");
    }
}
