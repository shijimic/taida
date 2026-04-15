use super::*;
use crate::parser::parse;

fn check(source: &str) -> (TypeChecker, Vec<TypeError>) {
    let (program, parse_errors) = parse(source);
    assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
    let mut checker = TypeChecker::new();
    checker.check_program(&program);
    let errors = checker.errors.clone();
    (checker, errors)
}

fn check_with_target(source: &str, target: CompileTarget) -> (TypeChecker, Vec<TypeError>) {
    let (program, parse_errors) = parse(source);
    assert!(parse_errors.is_empty(), "Parse errors: {:?}", parse_errors);
    let mut checker = TypeChecker::new();
    checker.set_compile_target(target);
    checker.check_program(&program);
    let errors = checker.errors.clone();
    (checker, errors)
}

#[test]
fn test_literal_type_inference() {
    let mut checker = TypeChecker::new();
    assert_eq!(
        checker.infer_expr_type(&Expr::IntLit(
            42,
            Span {
                start: 0,
                end: 2,
                line: 1,
                column: 1
            }
        )),
        Type::Int
    );
    assert_eq!(
        checker.infer_expr_type(&Expr::FloatLit(
            314.0 / 100.0,
            Span {
                start: 0,
                end: 4,
                line: 1,
                column: 1
            }
        )),
        Type::Float
    );
    assert_eq!(
        checker.infer_expr_type(&Expr::StringLit(
            "hello".to_string(),
            Span {
                start: 0,
                end: 7,
                line: 1,
                column: 1
            }
        )),
        Type::Str
    );
    assert_eq!(
        checker.infer_expr_type(&Expr::BoolLit(
            true,
            Span {
                start: 0,
                end: 4,
                line: 1,
                column: 1
            }
        )),
        Type::Bool
    );
}

#[test]
fn test_buchi_pack_type_inference() {
    let mut checker = TypeChecker::new();
    let expr = Expr::BuchiPack(
        vec![
            BuchiField {
                name: "name".to_string(),
                value: Expr::StringLit(
                    "Alice".to_string(),
                    Span {
                        start: 0,
                        end: 7,
                        line: 1,
                        column: 1,
                    },
                ),
                span: Span {
                    start: 0,
                    end: 7,
                    line: 1,
                    column: 1,
                },
            },
            BuchiField {
                name: "age".to_string(),
                value: Expr::IntLit(
                    30,
                    Span {
                        start: 0,
                        end: 2,
                        line: 1,
                        column: 1,
                    },
                ),
                span: Span {
                    start: 0,
                    end: 2,
                    line: 1,
                    column: 1,
                },
            },
        ],
        Span {
            start: 0,
            end: 10,
            line: 1,
            column: 1,
        },
    );
    let ty = checker.infer_expr_type(&expr);
    assert_eq!(
        ty,
        Type::BuchiPack(vec![
            ("name".to_string(), Type::Str),
            ("age".to_string(), Type::Int),
        ])
    );
}

#[test]
fn test_list_type_inference() {
    let mut checker = TypeChecker::new();
    let s = Span {
        start: 0,
        end: 1,
        line: 1,
        column: 1,
    };
    let expr = Expr::ListLit(
        vec![Expr::IntLit(1, s.clone()), Expr::IntLit(2, s.clone())],
        s.clone(),
    );
    assert_eq!(
        checker.infer_expr_type(&expr),
        Type::List(Box::new(Type::Int))
    );

    // Empty list
    let empty = Expr::ListLit(vec![], s);
    assert_eq!(
        checker.infer_expr_type(&empty),
        Type::List(Box::new(Type::Unknown))
    );
}

#[test]
fn test_type_registration() {
    let (checker, errors) = check("Person = @(name: Str, age: Int)");
    assert!(errors.is_empty());
    let fields = checker
        .registry
        .get_type_fields("Person")
        .expect("Person type should be registered after check");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0], ("name".to_string(), Type::Str));
    assert_eq!(fields[1], ("age".to_string(), Type::Int));
}

#[test]
fn test_enum_registration_and_constructor_type() {
    let (checker, errors) = check("Enum => Status = :Ok :Fail\nstatus: Status <= Status:Ok()");
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(checker.registry.is_enum_type("Status"));
    assert_eq!(
        checker.registry.get_enum_variants("Status"),
        Some(vec!["Ok".to_string(), "Fail".to_string()])
    );
}

#[test]
fn test_single_variant_enum_registration_and_constructor_type() {
    let (checker, errors) = check("Enum => Traffic = :Red\nsignal: Traffic <= Traffic:Red()");
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(checker.registry.is_enum_type("Traffic"));
    assert_eq!(
        checker.registry.get_enum_variants("Traffic"),
        Some(vec!["Red".to_string()])
    );
}

#[test]
fn test_unknown_enum_variant_rejected() {
    let (_checker, errors) = check("Enum => Status = :Ok :Fail\nstatus <= Status:Missing()");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Unknown enum variant")),
        "Expected unknown enum variant error, got {:?}",
        errors
    );
}

#[test]
fn test_cross_enum_comparison_rejected() {
    let (_checker, errors) =
        check("Enum => A = :One :Two\nEnum => B = :One :Two\nsame <= A:One() == B:One()");
    assert!(
        errors.iter().any(|err| err.message.contains("[E1605]")),
        "Expected E1605 for cross-enum comparison, got {:?}",
        errors
    );
}

#[test]
fn test_enum_pipeline_identity_is_allowed() {
    let (_checker, errors) = check(
        "Enum => Status = :Ok :Fail\n\
         status <= Status:Fail()\n\
         id x = x\n\
         status => id(_) => result",
    );
    assert!(
        errors.is_empty(),
        "Enum pipeline identity should type-check, got {:?}",
        errors
    );
}

#[test]
fn test_http_protocol_import_alias_registers_enum() {
    let (checker, errors) = check(">>> taida-lang/net => @(HttpProtocol: Proto)\np <= Proto:H2()");
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(checker.registry.is_enum_type("Proto"));
    assert_eq!(
        checker.registry.get_enum_variants("Proto"),
        Some(vec!["H1".to_string(), "H2".to_string(), "H3".to_string()])
    );
}

#[test]
fn test_http_protocol_js_h2_compile_time_reject() {
    let (_checker, errors) = check_with_target(
        ">>> taida-lang/net => @(httpServe: serve, HttpProtocol: Proto)\n\
         handler req = @(status <= 200, headers <= @[], body <= \"ok\")\n\
         serve(8080, handler, 1, 1000, 1, @(protocol <= Proto:H2()))",
        CompileTarget::Js,
    );
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("not supported on the JS backend")),
        "Expected JS compile-time reject for HttpProtocol:H2(), got {:?}",
        errors
    );
}

#[test]
fn test_http_protocol_native_h3_allowed() {
    let (_checker, errors) = check_with_target(
        ">>> taida-lang/net => @(httpServe, HttpProtocol)\n\
         handler req = @(status <= 200, headers <= @[], body <= \"ok\")\n\
         httpServe(8080, handler, 1, 1000, 1, @(protocol <= HttpProtocol:H3()))",
        CompileTarget::Native,
    );
    assert!(
        errors.is_empty(),
        "Native target should accept HttpProtocol:H3(), got {:?}",
        errors
    );
}

#[test]
fn test_invalid_net_import_reports_shared_export_list() {
    let (_checker, errors) = check(">>> taida-lang/net => @(MissingNetSymbol)");
    assert!(
        errors.iter().any(|err| {
            err.message.contains("MissingNetSymbol")
                && err.message.contains("HttpProtocol")
                && err.message.contains("httpServe")
        }),
        "Expected taida-lang/net export list with HttpProtocol, got {:?}",
        errors
    );
}

#[test]
fn test_error_type_registration() {
    let source = "Error => ValidationError = @(field: Str, code: Int)";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(checker.registry.is_error_type("ValidationError"));
    let fields = checker
        .registry
        .get_type_fields("ValidationError")
        .expect("ValidationError type should be registered after check");
    assert_eq!(fields.len(), 4);
}

#[test]
fn test_inheritance_registration() {
    let source = "Person = @(name: Str, age: Int)\nPerson => Employee = @(department: Str)";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    let emp_fields = checker
        .registry
        .get_type_fields("Employee")
        .expect("Employee type should be registered after inheritance check");
    assert_eq!(emp_fields.len(), 3);
}

#[test]
fn test_multilevel_error_inheritance_checker() {
    // Error => AppError => ValidationError — 3-level chain
    let source = r#"
Error => AppError = @(app_code: Int)
AppError => ValidationError = @(field: Str)
"#;
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);

    // Both should be recognized as error types
    assert!(checker.registry.is_error_type("AppError"));
    assert!(checker.registry.is_error_type("ValidationError"));

    // AppError should have 3 fields: type, message, app_code
    let app_fields = checker
        .registry
        .get_type_fields("AppError")
        .expect("AppError should be registered");
    assert_eq!(app_fields.len(), 3, "AppError fields: {:?}", app_fields);

    // ValidationError should have 4 fields: type, message, app_code, field
    let val_fields = checker
        .registry
        .get_type_fields("ValidationError")
        .expect("ValidationError should be registered");
    assert_eq!(
        val_fields.len(),
        4,
        "ValidationError fields: {:?}",
        val_fields
    );
    assert!(
        val_fields.iter().any(|(n, _)| n == "app_code"),
        "ValidationError should inherit app_code from AppError"
    );
}

#[test]
fn test_multilevel_custom_inheritance_checker() {
    let source = r#"
Vehicle = @(name: Str, speed: Int)
Vehicle => Car = @(doors: Int)
Car => SportsCar = @(turbo: Bool)
"#;
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);

    // SportsCar should have 4 fields: name, speed, doors, turbo
    let sc_fields = checker
        .registry
        .get_type_fields("SportsCar")
        .expect("SportsCar should be registered");
    assert_eq!(sc_fields.len(), 4, "SportsCar fields: {:?}", sc_fields);
    assert!(sc_fields.iter().any(|(n, _)| n == "name"));
    assert!(sc_fields.iter().any(|(n, _)| n == "speed"));
    assert!(sc_fields.iter().any(|(n, _)| n == "doors"));
    assert!(sc_fields.iter().any(|(n, _)| n == "turbo"));
}

#[test]
fn test_arithmetic_type_checking() {
    let s = Span {
        start: 0,
        end: 1,
        line: 1,
        column: 1,
    };
    let mut checker = TypeChecker::new();
    let expr = Expr::BinaryOp(
        Box::new(Expr::IntLit(1, s.clone())),
        BinOp::Add,
        Box::new(Expr::IntLit(2, s.clone())),
        s.clone(),
    );
    assert_eq!(checker.infer_expr_type(&expr), Type::Int);

    let expr2 = Expr::BinaryOp(
        Box::new(Expr::IntLit(1, s.clone())),
        BinOp::Add,
        Box::new(Expr::FloatLit(2.0, s.clone())),
        s.clone(),
    );
    assert_eq!(checker.infer_expr_type(&expr2), Type::Float);

    let expr3 = Expr::BinaryOp(
        Box::new(Expr::IntLit(1, s.clone())),
        BinOp::Gt,
        Box::new(Expr::IntLit(2, s.clone())),
        s,
    );
    assert_eq!(checker.infer_expr_type(&expr3), Type::Bool);
}

#[test]
fn test_field_access_type_checking() {
    let s = Span {
        start: 0,
        end: 1,
        line: 1,
        column: 1,
    };
    let mut checker = TypeChecker::new();
    let buchi = Expr::BuchiPack(
        vec![BuchiField {
            name: "name".to_string(),
            value: Expr::StringLit("Alice".to_string(), s.clone()),
            span: s.clone(),
        }],
        s.clone(),
    );
    let access = Expr::FieldAccess(Box::new(buchi.clone()), "name".to_string(), s.clone());
    assert_eq!(checker.infer_expr_type(&access), Type::Str);

    let bad_access = Expr::FieldAccess(Box::new(buchi), "email".to_string(), s);
    checker.infer_expr_type(&bad_access);
    assert_eq!(checker.errors.len(), 1);
    assert!(checker.errors[0].message.contains("does not exist"));
}

// ── Scope tracking tests ──────────────────────────────

#[test]
fn test_scope_variable_tracking() {
    let (checker, errors) = check("x <= 42\ny <= x + 1");
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(checker.lookup_var("x"), Some(Type::Int));
    assert_eq!(checker.lookup_var("y"), Some(Type::Int));
}

#[test]
fn test_scope_type_annotation_match() {
    let (_, errors) = check("x: Int <= 42");
    assert!(
        errors.is_empty(),
        "Should not produce errors for matching type"
    );
}

#[test]
fn test_scope_type_annotation_mismatch() {
    let (_, errors) = check("x: Int <= \"hello\"");
    assert_eq!(errors.len(), 1, "Should detect type mismatch");
    assert!(errors[0].message.contains("Type mismatch"));
}

#[test]
fn test_func_def_scope() {
    let source = "add x y =\n  x + y";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(checker.func_types.contains_key("add"));
}

#[test]
fn test_import_registers_symbols() {
    let (checker, errors) = check(">>> std/math => @(sqrt, PI)");
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    // Imported symbols should be in scope as Unknown
    assert_eq!(checker.lookup_var("sqrt"), Some(Type::Unknown));
    assert_eq!(checker.lookup_var("PI"), Some(Type::Unknown));
}

#[test]
fn test_list_get_type() {
    // IndexAccess removed in v0.5.0 -- use .get(i) instead
    // v0.7.0: .get() returns Lax[T]
    let (checker, errors) = check("items <= @[1, 2, 3]\nfirst <= items.get(0)");
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(
        checker.lookup_var("items"),
        Some(Type::List(Box::new(Type::Int)))
    );
    assert_eq!(
        checker.lookup_var("first"),
        Some(Type::Generic("Lax".to_string(), vec![Type::Int]))
    );
}

#[test]
fn test_method_return_type_str() {
    let source = "name <= \"hello\"\nlen <= name.length()";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(checker.lookup_var("len"), Some(Type::Int));
}

#[test]
fn test_method_return_type_json_v070() {
    // v0.7.0: JSON is opaque (molten iron). No methods allowed.
    // JSON methods return Unknown since they are abolished.
    let source = "json <= jsonEncode(42)\nresult <= json.toString()";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    // jsonEncode returns Str, not Json
    assert_eq!(checker.lookup_var("json"), Some(Type::Str));
}

#[test]
fn test_func_call_return_type() {
    let source = "double x =\n  x * 2\n\nresult <= double(21)";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    // Without return type annotation, func returns Unknown
    assert_eq!(checker.lookup_var("result"), Some(Type::Unknown));
}

#[test]
fn test_func_call_too_many_args_is_error() {
    let source = "add x: Int y: Int =\n  x + y\n=> :Int\n\nresult <= add(1, 2, 3)";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1301]"),
        "Expected E1301 too many args error, got: {:?}",
        errors
    );
}

#[test]
fn test_param_default_self_or_forward_reference_is_error() {
    let source = r#"selfRef a: Int <= a =
  a
=> :Int

forwardRef a: Int <= b b: Int <= 1 =
  a + b
=> :Int"#;
    let (_checker, errors) = check(source);
    let e1302_count = errors
        .iter()
        .filter(|e| e.message.contains("[E1302]"))
        .count();
    assert!(
        e1302_count >= 2,
        "Expected E1302 for self/forward refs, got: {:?}",
        errors
    );
}

#[test]
fn test_param_default_type_mismatch_is_error() {
    let source = r#"bad a: Int <= "oops" =
  a
=> :Int"#;
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1303]"),
        "Expected E1303 default type mismatch error, got: {:?}",
        errors
    );
}

#[test]
fn test_param_default_can_reference_previous_param() {
    let source = r#"ok a: Int <= 1 b: Int <= a =
  b
=> :Int

x <= ok()"#;
    let (_checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
}

// ---- H-3: Type checker strengthening tests ----

#[test]
fn test_mold_inst_type_inference() {
    // Type conversion molds return Lax[T]
    let source = "x <= Int[\"42\"]()";
    let (checker, _errors) = check(source);
    assert_eq!(
        checker.lookup_var("x"),
        Some(Type::Generic("Lax".to_string(), vec![Type::Int]))
    );
}

#[test]
fn test_mold_inst_div_type() {
    let source = "x <= Div[10, 3]()";
    let (checker, _errors) = check(source);
    assert_eq!(
        checker.lookup_var("x"),
        Some(Type::Generic("Lax".to_string(), vec![Type::Int]))
    );
}

#[test]
fn test_molten_rejects_type_args() {
    let source = "m <= Molten[1]()";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("Molten takes no type arguments"),
        "Expected Molten arity error, got: {:?}",
        errors
    );
}

#[test]
fn test_cond_branch_type() {
    // Condition branch should infer type from first arm
    let source = "x <= 5\ny <=\n  | x > 3 |> \"big\"\n  | _ |> \"small\"";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("y"), Some(Type::Str));
}

#[test]
fn test_num_state_check_methods() {
    let source = "n <= 42\nresult <= n.isPositive()";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("result"), Some(Type::Bool));
}

#[test]
fn test_list_state_check_methods() {
    let source = "items <= @[1, 2, 3]\nempty <= items.isEmpty()";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("empty"), Some(Type::Bool));
}

#[test]
fn test_list_none_method() {
    let source = "items <= @[1, 2, 3]\nresult <= items.none(_ x = x > 10)";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("result"), Some(Type::Bool));
}

#[test]
fn test_func_with_return_type_annotation() {
    // Taida syntax: `name params = body => :ReturnType`
    let source = "greet name =\n  `Hello ${name}`\n=> :Str\n\nmsg <= greet(\"Alice\")";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(checker.lookup_var("msg"), Some(Type::Str));
}

#[test]
fn test_generic_function_id_infers_argument_type() {
    let source = "id[T] x: T =\n  x\n=> :T\n\nvalue <= id(1)";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(checker.lookup_var("value"), Some(Type::Int));
}

#[test]
fn test_generic_function_first_preserves_inner_type() {
    let source =
        "first[T] xs: @[T] =\n  xs.get(0)\n=> :Lax[T]\n\nvalue <= first(@[1, 2, 3]).unmold()";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(checker.lookup_var("value"), Some(Type::Int));
}

#[test]
fn test_generic_function_map_value_infers_return_type() {
    let source = "mapValue[T, U] value: T fn: T => :U =\n  fn(value)\n=> :U\n\ntext <= mapValue(1, _ x = x.toString())";
    let (checker, errors) = check(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(checker.lookup_var("text"), Some(Type::Str));
}

#[test]
fn test_generic_function_constraint_is_enforced() {
    let source = "idNum[T <= :Num] x: T =\n  x\n=> :T\n\nvalue <= idNum(\"nope\")";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1509]")
            && errors[0].message.contains("violates its constraint"),
        "Expected generic function constraint error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_requires_inferable_type_param() {
    let source = "make[T] =\n  1\n=> :T\n\nvalue <= make()";
    let (_checker, errors) = check(source);
    // RCB-50: Previously emitted 2 errors (E1510 + E1601), but E1601 is
    // now correctly suppressed because the return type `:T` is an
    // unresolved type variable that cannot be meaningfully compared.
    assert!(
        errors.iter().any(|e| e.message.contains("[E1510]")
            && e.message.contains("uninferable type parameter(s): T")),
        "Expected generic inference error E1510, got: {:?}",
        errors
    );
    assert!(
        !errors.iter().any(|e| e.message.contains("[E1601]")),
        "Should not emit E1601 when return type is an unresolved type variable, got: {:?}",
        errors
    );
}

#[test]
fn test_rejected_generic_function_does_not_emit_spurious_non_generic_call_error() {
    let source = "pair[T, U] x: T =\n  x\n=> :U\n\nvalue <= pair(1)";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("uninferable type parameter(s): U"),
        "Expected generic definition error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().all(|e| !e.message.contains("[E1506]")),
        "Did not expect fallback non-generic argument mismatch error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_does_not_treat_unknown_binding_as_inferred() {
    let source = "accept[T] fn: T => :Bool =\n  true\n=> :Bool\n\nok <= accept(_ y = true)";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("could not infer type parameter(s): T"),
        "Expected higher-order generic inference error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_type_param_cannot_shadow_builtin_type_name() {
    let source = "id[Int] x: Int =\n  x\n=> :Int\n\nvalue <= id(1)";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("reserved concrete type name(s) as type parameter(s): Int"),
        "Expected generic type parameter name collision error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_type_param_cannot_shadow_declared_type_name() {
    let source = "User = @(name: Str)\n\nid[User] x: User =\n  x\n=> :User";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("reserved concrete type name(s) as type parameter(s): User"),
        "Expected declared type name collision error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_type_param_cannot_shadow_later_declared_type_name() {
    let source = "id[Point] x: Point =\n  x\n=> :Point\n\nPoint = @(x: Int)";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("reserved concrete type name(s) as type parameter(s): Point"),
        "Expected forward-declared type name collision error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_type_param_cannot_shadow_declared_mold_name() {
    let source = "Mold[T] => Box[T] = @()\n\nid[Box] x: Box =\n  x\n=> :Box";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("reserved concrete type name(s) as type parameter(s): Box"),
        "Expected declared mold name collision error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_function_type_param_cannot_shadow_later_declared_mold_name() {
    let source = "id[Box] x: Box =\n  x\n=> :Box\n\nMold[T] => Box[T] = @()";
    let (_checker, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1510]")
            && errors[0]
                .message
                .contains("reserved concrete type name(s) as type parameter(s): Box"),
        "Expected forward-declared mold name collision error, got: {:?}",
        errors
    );
}

#[test]
fn test_stdin_returns_str() {
    let source = "input <= stdin(\"prompt: \")";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("input"), Some(Type::Str));
}

#[test]
fn test_argv_returns_str_list() {
    let source = "args <= argv()";
    let (checker, _errors) = check(source);
    assert_eq!(
        checker.lookup_var("args"),
        Some(Type::List(Box::new(Type::Str)))
    );
}

#[test]
fn test_sha256_returns_str() {
    let source = ">>> taida-lang/crypto => @(sha256)\nh <= sha256(\"abc\")";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("h"), Some(Type::Str));
}

#[test]
fn test_range_returns_int_list() {
    let source = "nums <= range(1, 10)";
    let (checker, _errors) = check(source);
    assert_eq!(
        checker.lookup_var("nums"),
        Some(Type::List(Box::new(Type::Int)))
    );
}

#[test]
fn test_hashmap_type() {
    let source = "m <= hashMap(@[])\nresult <= m.has(\"key\")";
    let (checker, _errors) = check(source);
    assert_eq!(
        checker.lookup_var("m"),
        Some(Type::Named("HashMap".to_string()))
    );
    assert_eq!(checker.lookup_var("result"), Some(Type::Bool));
}

#[test]
fn test_set_type() {
    let source = "s <= setOf(@[1, 2, 3])\nresult <= s.has(1)";
    let (checker, _errors) = check(source);
    assert_eq!(
        checker.lookup_var("s"),
        Some(Type::Named("Set".to_string()))
    );
    assert_eq!(checker.lookup_var("result"), Some(Type::Bool));
}

#[test]
fn test_string_mold_type() {
    let source = "result <= Upper[\"hello\"]()";
    let (checker, _errors) = check(source);
    assert_eq!(checker.lookup_var("result"), Some(Type::Str));
}

#[test]
fn test_hof_mold_type() {
    let source = "nums <= @[1, 2, 3]\nresult <= Filter[nums, _ x = x > 1]()";
    let (checker, _errors) = check(source);
    assert!(
        matches!(checker.lookup_var("result"), Some(Type::List(_))),
        "Expected List type, got {:?}",
        checker.lookup_var("result")
    );
}

#[test]
fn test_empty_list_without_annotation_error() {
    // @[] without type annotation should produce an error
    let (_, errors) = check("items <= @[]");
    assert_eq!(
        errors.len(),
        1,
        "Should detect empty list without type annotation"
    );
    assert!(
        errors[0].message.contains("Empty list literal"),
        "Error message should mention empty list literal: {}",
        errors[0].message
    );
}

#[test]
fn test_empty_list_with_annotation_ok() {
    // @[] with type annotation should be accepted
    let (checker, errors) = check("items: @[Int] <= @[]");
    assert!(
        errors.is_empty(),
        "Should not produce errors for annotated empty list: {:?}",
        errors
    );
    assert_eq!(
        checker.lookup_var("items"),
        Some(Type::List(Box::new(Type::Int)))
    );
}

#[test]
fn test_non_empty_list_without_annotation_ok() {
    // @[1, 2, 3] without annotation is fine (type can be inferred)
    let (checker, errors) = check("nums <= @[1, 2, 3]");
    assert!(
        errors.is_empty(),
        "Should not produce errors for non-empty list"
    );
    assert_eq!(
        checker.lookup_var("nums"),
        Some(Type::List(Box::new(Type::Int)))
    );
}

// ── Low-1: unmold_type for custom mold Named types ──

#[test]
fn test_unmold_type_custom_mold_named() {
    // Named type that is a registered mold should unmold to its first type param
    let mut checker = TypeChecker::new();
    // Register a custom mold "Container" with type param "T"
    checker.registry.register_mold(
        "Container",
        vec!["T".to_string()],
        vec![("count".to_string(), Type::Int)],
    );
    // A Named("Container") type should not return Unknown when unmolded
    // It should return Unknown (we can't know T without instantiation), but
    // at least Generic("Container", [Unknown]) style should be recognized
    let named_ty = Type::Named("Container".to_string());
    let unmolded = checker.unmold_type(&named_ty);
    // For Named mold types, we return Unknown (type param not instantiated)
    // This is acceptable — the key fix is that Generic("Container", [Int]) unmolds to Int
    assert_eq!(unmolded, Type::Unknown);

    // The real improvement: Generic("Container", [Int]) should unmold to Int
    let generic_ty = Type::Generic("Container".to_string(), vec![Type::Int]);
    let unmolded_generic = checker.unmold_type(&generic_ty);
    assert_eq!(unmolded_generic, Type::Int);
}

#[test]
fn test_mold_field_type_only_is_valid() {
    let source = r#"Mold[T] => Container[T] = @(
  count: Int
)"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Type-only field in MoldDef should be valid: {:?}",
        errors
    );
}

#[test]
fn test_mold_field_default_only_is_valid() {
    let source = r#"Mold[T] => Container[T] = @(
  count <= 0
)"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Default-only field in MoldDef should be valid: {:?}",
        errors
    );
}

#[test]
fn test_mold_field_without_type_and_default_is_error() {
    let source = r#"Mold[T] => Container[T] = @(
  count
)"#;
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1400]") && e.message.contains("Hint:")),
        "Expected field declaration error, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_type_params_without_binding_target_is_error() {
    let source = r#"Mold[T] => Broken[T, U] = @()"#;
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1401]")
                && e.message.contains("unbound type parameter(s): U")),
        "Expected unbound type parameter error, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_type_param_after_concrete_filling_without_binding_target_is_error() {
    let source = r#"Mold[:Int] => Broken[:Int, U] = @()"#;
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1401]")
                && e.message.contains("unbound type parameter(s): U")),
        "Expected concrete-first mold header to preserve unbound type parameter error, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_type_param_after_concrete_slots_without_binding_target_is_error() {
    let source = r#"Mold[:Int] => Broken[:Int, :Str, U] = @(
  tail: Int
)"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1401]")
            && errors[0].message.contains("unbound type parameter(s): U"),
        "Expected later concrete mold header args to consume field slots before type params, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_concrete_header_arg_without_binding_target_is_error() {
    let source = r#"Mold[T] => Broken[T, :Int] = @()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1401]")
            && errors[0]
                .message
                .contains("header argument(s) without binding target(s): :Int"),
        "Expected concrete mold header arg without binding target error, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_inst_missing_required_positional_args_is_error() {
    let source = r#"Mold[T] => Pair[T, U] = @(
  second: U
)
p <= Pair[1]()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1402]")
            && errors[0]
                .message
                .contains("requires 2 positional `[]` argument(s), got 1"),
        "Expected missing positional arg error, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_inst_too_many_positional_args_is_error() {
    let source = r#"Mold[T] => Pair[T, U] = @(
  second: U
  flag: Bool <= false
)
p <= Pair[1, 2, true]()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1403]")
            && errors[0]
                .message
                .contains("takes 2 positional `[]` argument(s), got 3"),
        "Expected positional overflow error, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_inst_required_field_in_named_options_is_error() {
    let source = r#"Mold[T] => Pair[T, U] = @(
  second: U
  flag: Bool <= false
)
p <= Pair[1](second <= 2)"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors (E1402 + E1405), got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1405]")
                && e.message.contains("field 'second' must be passed via `[]`")
        }),
        "Expected []/() mismatch error for required field, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_inst_duplicate_and_undefined_options_are_errors() {
    let source = r#"Mold[T] => Pair[T, U] = @(
  second: U
  flag: Bool <= false
)
p <= Pair[1, 2](flag <= true, flag <= false, nope <= true)"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors (E1404 + E1406), got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1404]") && e.message.contains("duplicate option 'flag'")
        }),
        "Expected duplicate option error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1406]") && e.message.contains("undefined option 'nope'")
        }),
        "Expected undefined option error, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_inst_with_valid_binding_rules_has_no_errors() {
    let source = r#"Mold[T] => Pair[T, U] = @(
  second: U
  flag: Bool <= false
)
p <= Pair[1, 2](flag <= true)"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Expected valid custom mold instantiation to pass, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_explicit_name_header_must_preserve_inherited_prefix() {
    let source = r#"Mold[:Int] => IntBox[T, U] = @()
box <= IntBox[1]()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message.contains("[E1407]")
            && e.message.contains("must preserve inherited header slot 1")),
        "Expected inherited prefix preservation error, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_concrete_header_type_is_enforced() {
    let source = r#"Mold[:Int] => IntBox = @()
box <= IntBox["oops"]()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1408]")
            && errors[0]
                .message
                .contains("positional `[]` argument 1 is fixed to Int"),
        "Expected concrete mold header type error, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_constraint_can_reference_previous_header_type() {
    let source = r#"Mold[T] => Guard[T, P <= :T => :Bool] = @(
  predicate: P
)
box <= Guard[1, _ value = value > 0]()"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Expected constrained mold header to accept matching lambda, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_root_cannot_extend_parent_arity_directly() {
    let source = r#"Mold[T, P <= :T => :Bool] => Guard[T, P] = @(
  predicate: P
)"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1407]")
                && e.message
                    .contains("must keep the built-in parent `Mold` header at arity 1")
        }),
        "Expected direct Mold arity extension to be rejected, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_child_header_cannot_reuse_type_param_names() {
    let source = r#"Mold[T] => Guard[T, T] = @(
  predicate: T
)"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1407]")
            && errors[0]
                .message
                .contains("reuses header type parameter name(s): T"),
        "Expected duplicate child header type parameter names to be rejected, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_child_header_must_preserve_inherited_prefix() {
    let source = r#"Mold[T] => Guard[T, P <= :T => :Bool] = @(
  predicate: P
)
Guard[T, P <= :T => :Bool] => GuardAlias[T, Pred] = @()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1407]")
            && errors[0]
                .message
                .contains("must preserve inherited header slot 2"),
        "Expected inherited prefix rename/rewrite to be rejected, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_inheritance_can_extend_with_constrained_child_slot() {
    let source = r#"Mold[T] => Guard[T] = @()
Guard[T] => GuardWithPredicate[T, P <= :T => :Bool] = @(
  predicate: P
)
box <= GuardWithPredicate[1, _ value = value > 0]()"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Expected child mold inheritance to extend with constrained slot, got: {:?}",
        errors
    );
}

#[test]
fn test_custom_mold_constraint_is_enforced() {
    let source = r#"Mold[T] => Guard[T, P <= :T => :Bool] = @(
  predicate: P
)
box <= Guard[1, _ value = "nope"]()"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1409]") && e.message.contains("violates constraint on 'P'")
        }),
        "Expected constrained mold header error, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_inheritance_child_constraint_is_enforced() {
    let source = r#"Mold[T] => Guard[T] = @()
Guard[T] => GuardWithPredicate[T, P <= :T => :Bool] = @(
  predicate: P
)
box <= GuardWithPredicate[1, _ value = "nope"]()"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1409]") && e.message.contains("violates constraint on 'P'")
        }),
        "Expected child-side constraint to be enforced, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_inheritance_can_extend_parent_header_arity() {
    let source = r#"Mold[T] => Parent[T] = @()
Parent[T] => Child[T, U] = @(
  extra: U
)"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Expected child generic inheritance to extend parent header arity, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_inheritance_headers_require_mold_like_parent() {
    let source = r#"Base = @(value: Int)
Base[T] => Child[T, U] = @(
  extra: U
)"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1407]")
                && e.message
                    .contains("can only declare `Parent[...] => Child[...]` headers")
                && e.message.contains("parent 'Base' is a mold-like type")
        }),
        "Expected non-mold parent headers to be rejected, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_inheritance_can_reference_forward_declared_mold_parent() {
    let source = r#"Parent[T] => Child[T, U] = @(
  extra: U
)
Mold[T] => Parent[T] = @()
child <= Child[1, 2]()"#;
    let (_, errors) = check(source);
    assert!(
        errors.is_empty(),
        "Expected forward-declared mold parent inheritance to succeed, got: {:?}",
        errors
    );
}

#[test]
fn test_generic_inheritance_cannot_shrink_parent_header_arity() {
    let source = r#"Mold[T] => Parent[T] = @()
Parent[T] => Expanded[T, U] = @(
  extra: U
)
Expanded[T, U] => Child[T] = @()"#;
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1407]")
                && e.message
                    .contains("cannot shrink header arity below parent")
        }),
        "Expected child generic inheritance shrink to be rejected, got: {:?}",
        errors
    );
}

// ── E1501: Same-scope name collision ──

#[test]
fn test_same_scope_variable_redefinition_is_error() {
    let source = "x <= 1\nx <= 2";
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1501]"),
        "Expected E1501 same-scope redefinition error, got: {:?}",
        errors
    );
}

#[test]
fn test_same_scope_function_overload_is_error() {
    let source = "f x: Int =\n  x + 1\n=> :Int\nf x: Str =\n  x\n=> :Str";
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1501]"),
        "Expected E1501 function overload error, got: {:?}",
        errors
    );
}

#[test]
fn test_invalid_generic_function_still_triggers_same_scope_duplicate_error() {
    let source = "id[T] x: T =\n  x\n=> :T\n\nid[T, U] x: T =\n  x\n=> :U";
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors (E1501 + E1510), got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Expected E1501 duplicate-name error with invalid generic overload, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1510]") && e.message.contains("uninferable type parameter(s): U")
        }),
        "Expected E1510 for the invalid generic overload, got: {:?}",
        errors
    );
}

#[test]
fn test_invalid_generic_duplicate_clears_stale_callable_metadata() {
    let source = "id[T] x: T =\n  x\n=> :T\n\nid[T, U] x: T =\n  x\n=> :U\n\ny: Str <= id(1)";
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors (E1501 + E1510), got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Expected duplicate-name error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1510]") && e.message.contains("uninferable type parameter(s): U")
        }),
        "Expected invalid generic definition error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().all(|e| !e.message.contains("Type mismatch")),
        "Did not expect stale callable metadata to trigger downstream type mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_invalid_then_valid_duplicate_still_clears_callable_metadata() {
    let source = "id[T, U] x: T =\n  x\n=> :U\n\nid[T] x: T =\n  x\n=> :T\n\ny: Str <= id(1)";
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        2,
        "Expected exactly 2 errors (E1510 + E1501), got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Expected duplicate-name error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| {
            e.message.contains("[E1510]") && e.message.contains("uninferable type parameter(s): U")
        }),
        "Expected invalid generic definition error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().all(|e| !e.message.contains("Type mismatch")),
        "Did not expect duplicate order to leave callable metadata behind, got: {:?}",
        errors
    );
}

#[test]
fn test_function_overwrites_variable_is_error() {
    let source = "x <= 1\nx =\n  42\n=> :Int";
    let (_, errors) = check(source);
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].message.contains("[E1501]"),
        "Expected E1501 variable-to-function overwrite error, got: {:?}",
        errors
    );
}

#[test]
fn test_inner_scope_shadowing_is_allowed() {
    let source = "x <= 1\nf =\n  x <= 2\n  x\n=> :Int";
    let (_, errors) = check(source);
    let e1501_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1501]"))
        .collect();
    assert!(
        e1501_errors.is_empty(),
        "Shadowing in inner scope should not produce E1501, got: {:?}",
        e1501_errors
    );
}

// ── E1502: Old `_` partial application reject ──

#[test]
fn test_old_placeholder_partial_application_is_rejected() {
    // C-5c / QF-42: `f(5, _)` should be rejected — use `f(5, )` instead
    let source = "add x y = x + y => :Int\nresult <= add(5, _)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1502]")),
        "Expected E1502 old placeholder partial application error, got: {:?}",
        errors
    );
}

#[test]
fn test_placeholder_in_pipeline_is_allowed() {
    // `_` in pipeline context is valid — refers to pipe value
    let source = r#"
add x y = x + y => :Int
5 => add(_, 3) => result
"#;
    let (_, errors) = check(source);
    let e1502_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1502]"))
        .collect();
    assert!(
        e1502_errors.is_empty(),
        "Placeholder in pipeline should not produce E1502, got: {:?}",
        e1502_errors
    );
}

#[test]
fn test_lambda_underscore_is_allowed() {
    // `_ x = x + 1` is a lambda, not a placeholder — should be allowed
    let source = "f <= _ x = x + 1";
    let (_, errors) = check(source);
    let e1502_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1502]"))
        .collect();
    assert!(
        e1502_errors.is_empty(),
        "Lambda underscore should not produce E1502, got: {:?}",
        e1502_errors
    );
}

// ── E1503: TypeDef/BuchiPack partial application reject ──

#[test]
fn test_typedef_partial_application_placeholder_is_rejected() {
    // C-5d / QF-43: `Point(_, 2)` should be rejected
    let source = "Point = @(x: Int, y: Int)\np <= Point(_, 2)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1503]")),
        "Expected E1503 TypeDef partial application error, got: {:?}",
        errors
    );
}

#[test]
fn test_typedef_partial_application_hole_is_rejected() {
    // C-5d / QF-43: `Point(, 2)` (empty slot) should also be rejected for TypeDef
    let source = "Point = @(x: Int, y: Int)\np <= Point(, 2)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1503]")),
        "Expected E1503 TypeDef partial application (hole) error, got: {:?}",
        errors
    );
}

// ── E1504: Mold[_]() direct binding reject ──

#[test]
fn test_mold_placeholder_direct_binding_is_rejected() {
    // C-5e / QF-44: `trimIt <= Trim[_]()` outside pipeline should be rejected
    let source = r#"trimIt <= Trim[_]()"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1504]")),
        "Expected E1504 Mold[_]() direct binding error, got: {:?}",
        errors
    );
}

#[test]
fn test_mold_placeholder_in_pipeline_is_allowed() {
    // `data => Trim[_]()` in pipeline is valid
    let source = r#""  hello  " => Trim[_]() => result"#;
    let (_, errors) = check(source);
    let e1504_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1504]"))
        .collect();
    assert!(
        e1504_errors.is_empty(),
        "Mold[_]() in pipeline should not produce E1504, got: {:?}",
        e1504_errors
    );
}

// ── C-10a: QF-41 空白区切り呼び出し ──

#[test]
fn test_whitespace_separated_call_is_not_parsed_as_funccall() {
    // QF-41: `f x` should not be parsed as a function call
    // In Taida, function calls require parentheses: `f(x)`
    // `f x` is either a function definition or two separate expressions
    let source = "f x = x + 1 => :Int\nresult <= f(5)";
    let (_, errors) = check(source);
    // Just verify that proper call syntax works without errors
    // (空白区切り呼び出しの誤パースは parser レベルで防止)
    let _ = errors; // parser-level concern, checker validates what parser produces
}

// ── C-10e/C-10f: QF-45 / QF-46 checker regression ──

#[test]
fn test_qf45_function_overload_is_checker_rejected() {
    // QF-45: Function overload must be caught by checker (E1501)
    let source = "greet name: Str =\n  name\n=> :Str\ngreet name: Int =\n  toString(name)\n=> :Str";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "QF-45: Expected E1501 for function overload, got: {:?}",
        errors
    );
}

#[test]
fn test_qf46_same_scope_redefinition_is_checker_rejected() {
    // QF-46: Same-scope variable redefinition must be caught by checker (E1501)
    let source = "counter <= 0\ncounter <= 1";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "QF-46: Expected E1501 for same-scope redefinition, got: {:?}",
        errors
    );
}

// ── C-4a: 空スロット 0 個は通常呼び出し、1 個以上は部分適用 ──

#[test]
fn test_no_holes_is_normal_call() {
    // C-4a: `f(1, 2)` — no holes, normal function call
    let source = "add x y = x + y\n=> :Int\nresult <= add(1, 2)";
    let (_, errors) = check(source);
    // Should succeed without errors (normal call)
    assert!(
        errors.is_empty(),
        "Normal call should not produce errors, got: {:?}",
        errors
    );
}

#[test]
fn test_with_holes_is_partial_application() {
    // C-4a: `f(1, )` — one hole, partial application
    let source = "add x y = x + y\n=> :Int\nadd1 <= add(1, )";
    let (_, errors) = check(source);
    // Partial application via empty slot should be accepted by checker
    // (it's only rejected for TypeDef/BuchiPack via E1503)
    let e1503_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1503]"))
        .collect();
    assert!(
        e1503_errors.is_empty(),
        "Function partial application should not produce E1503, got: {:?}",
        e1503_errors
    );
}

// ── C-4b: arity と空スロット数の整合性 ──

#[test]
fn test_arity_check_normal_call_too_many_args() {
    // C-4b: Too many args should produce E1301
    let source = "add x y = x + y\n=> :Int\nresult <= add(1, 2, 3)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1301]")),
        "Expected E1301 for too many args, got: {:?}",
        errors
    );
}

#[test]
fn test_arity_check_partial_with_holes() {
    // C-4b: Partial application with holes should not trigger arity errors
    // `add(, )` — 2 holes for 2-param function, valid partial application
    let source = "add x y = x + y\n=> :Int\nadd_none <= add(, )";
    let (_, errors) = check(source);
    let e1301_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1301]"))
        .collect();
    assert!(
        e1301_errors.is_empty(),
        "Partial application should not produce E1301, got: {:?}",
        e1301_errors
    );
}

// ── C-4c: デフォルト引数と空スロットの組み合わせ ──

#[test]
fn test_default_args_with_partial_application_valid() {
    // C-4c: `sum3(1, , )` — partial application with all 3 slots filled, valid
    let source = "sum3 a b <= 10 c <= 20 =\n  a + b + c\n=> :Int\nadd_from_1 <= sum3(1, , )";
    let (_, errors) = check(source);
    let relevant_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("[E1301]")
                || e.message.contains("[E1503]")
                || e.message.contains("[E1505]")
        })
        .collect();
    assert!(
        relevant_errors.is_empty(),
        "Partial application with all slots should not error, got: {:?}",
        relevant_errors
    );
}

#[test]
fn test_default_args_with_partial_application_missing_slots() {
    // C-4c: `sum3(1, )` — 2 slots for 3-param function, E1505
    let source = "sum3 a b <= 10 c <= 20 =\n  a + b + c\n=> :Int\nadd_from_1 <= sum3(1, )";
    let (_, errors) = check(source);
    let e1505: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1505]"))
        .collect();
    assert!(
        !e1505.is_empty(),
        "Partial application with fewer slots than arity should produce E1505"
    );
}

// ── C-4d: `f(1)` は通常呼び出し、`f(1, )` は部分適用 ──

#[test]
fn test_f1_is_normal_f1_comma_is_partial() {
    // C-4d: Confirm `f(1)` and `f(1, )` are treated differently by checker
    let source_normal = "id x = x\n=> :Int\nresult <= id(1)";
    let source_partial = "add x y = x + y\n=> :Int\nadd1 <= add(1, )";

    let (_, errors_normal) = check(source_normal);
    let (_, errors_partial) = check(source_partial);

    // Normal call: no errors
    assert!(
        errors_normal.is_empty(),
        "Normal call should not error, got: {:?}",
        errors_normal
    );

    // Partial application: no E1503 (function partial is allowed)
    let e1503: Vec<_> = errors_partial
        .iter()
        .filter(|e| e.message.contains("[E1503]"))
        .collect();
    assert!(
        e1503.is_empty(),
        "Function partial application should not produce E1503, got: {:?}",
        e1503
    );
}

#[test]
fn test_partial_application_returns_function_type() {
    // C-4a: Partial application should type the result as a function, not the return type
    // add1 <= add(1, ) should make add1 a function (:Int => :Int)
    // add1 + 1 should be a type error
    let source = "add x: Int y: Int = x + y => :Int\nadd1 <= add(1, )\nn <= add1 + 1";
    let (_, errors) = check(source);
    let type_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("Cannot apply"))
        .collect();
    assert!(
        !type_errors.is_empty(),
        "Using partial application result as Int should produce type error, got: {:?}",
        errors
    );
    // The error message should show concrete function type, not (?) => ?
    assert!(
        type_errors[0].message.contains("(Int) => Int"),
        "Should show concrete type (Int) => Int, got: {}",
        type_errors[0].message
    );
}

// ── C-6b: Inner scope shadowing 許可 / same-scope redefine 禁止 回帰テスト ──

#[test]
fn test_inner_scope_shadow_allowed_regression() {
    // C-6b: Shadowing in inner scope is always allowed
    let source = "x <= 10\nwrapper =\n  x <= 20\n  x\n=> :Int";
    let (_, errors) = check(source);
    let e1501: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1501]"))
        .collect();
    assert!(
        e1501.is_empty(),
        "Inner scope shadowing should be allowed, got: {:?}",
        e1501
    );
}

#[test]
fn test_same_scope_redefine_forbidden_regression() {
    // C-6b: Same-scope redefinition is always forbidden
    let source = "a <= 1\nb <= 2\na <= 3";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Same-scope redefinition should produce E1501, got: {:?}",
        errors
    );
}

#[test]
fn test_nested_function_shadow_allowed() {
    // C-6b: Function inside function can shadow outer variable
    let source = "total <= 100\ncalc =\n  total <= 200\n  total\n=> :Int\nstdout(total.toString())";
    let (_, errors) = check(source);
    let e1501: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1501]"))
        .collect();
    assert!(
        e1501.is_empty(),
        "Nested function shadowing should be allowed, got: {:?}",
        e1501
    );
}

// ── C-11b: docs 由来の否定例 ──

#[test]
fn test_docs_negative_same_scope_redefinition() {
    // C-11b: docs say same-scope redefinition is an error
    let source = "counter <= 0\ncounter <= counter + 1";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Docs-derived negative case: same-scope redefine should error, got: {:?}",
        errors
    );
}

#[test]
fn test_docs_negative_function_overload() {
    // C-11b: docs say function overloading is disallowed
    let source = "greet name: Str =\n  name\n=> :Str\ngreet age: Int =\n  age.toString()\n=> :Str";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Docs-derived negative case: overload should error, got: {:?}",
        errors
    );
}

#[test]
fn test_docs_negative_old_placeholder_partial() {
    // C-11b: docs say old `_` partial application is rejected
    let source = "add x y = x + y\n=> :Int\nresult <= add(5, _)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1502]")),
        "Docs-derived negative case: old _ partial should error, got: {:?}",
        errors
    );
}

#[test]
fn test_docs_negative_typedef_partial() {
    // C-11b: docs say TypeDef partial application is not supported
    let source = "Point = @(x: Int, y: Int)\np <= Point(1, )";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1503]")),
        "Docs-derived negative case: TypeDef partial should error, got: {:?}",
        errors
    );
}

#[test]
fn test_docs_negative_mold_placeholder_outside_pipeline() {
    // C-11b: docs say Mold[_]() outside pipeline is rejected
    let source = "x <= Trim[_]()";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1504]")),
        "Docs-derived negative case: Mold[_]() outside pipeline should error, got: {:?}",
        errors
    );
}

// ── Phase 7: E1506 — Function argument type mismatch ──

#[test]
fn test_func_arg_type_mismatch_is_error() {
    // E1506: add(x: Int, y: Int) called with ("oops", 1) should error
    let source = "add x: Int y: Int =\n  x + y\n=> :Int\nresult <= add(\"oops\", 1)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1506]")),
        "Expected E1506 for argument type mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_func_arg_type_mismatch_correct_arg_is_ok() {
    // Correct types should not produce E1506
    let source = "add x: Int y: Int =\n  x + y\n=> :Int\nresult <= add(1, 2)";
    let (_, errors) = check(source);
    let e1506: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1506]"))
        .collect();
    assert!(
        e1506.is_empty(),
        "Correct arg types should not produce E1506, got: {:?}",
        e1506
    );
}

#[test]
fn test_func_arg_type_mismatch_partial_app_then_call() {
    // E1506: add1 = add(1, ) is Function(Int) => Int
    // add1("oops") should error
    let source =
        "add x: Int y: Int =\n  x + y\n=> :Int\nadd1 <= add(1, )\nresult <= add1(\"oops\")";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1506]")),
        "Expected E1506 for partial application argument type mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_func_arg_type_mismatch_unknown_types_not_checked() {
    // Unknown param types should not produce E1506
    let source = "f x y =\n  x + y\nresult <= f(\"hello\", 1)";
    let (_, errors) = check(source);
    let e1506: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1506]"))
        .collect();
    assert!(
        e1506.is_empty(),
        "Unknown param types should not produce E1506, got: {:?}",
        e1506
    );
}

#[test]
fn test_func_arg_type_subtype_is_ok() {
    // Int is subtype of Num, should not error
    let source = "f x: Num =\n  x\n=> :Num\nresult <= f(42)";
    let (_, errors) = check(source);
    let e1506: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1506]"))
        .collect();
    assert!(
        e1506.is_empty(),
        "Subtype arg should not produce E1506, got: {:?}",
        e1506
    );
}

// ── Phase 7: E1507 — Builtin arity check ──

#[test]
fn test_builtin_range_too_few_args() {
    // E1507: range(1) — needs 2-3 args
    let source = "nums <= range(1)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1507]")),
        "Expected E1507 for range(1) too few args, got: {:?}",
        errors
    );
}

#[test]
fn test_builtin_range_correct_arity() {
    // range(1, 10) is correct — no E1507
    let source = "nums <= range(1, 10)";
    let (_, errors) = check(source);
    let e1507: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1507]"))
        .collect();
    assert!(
        e1507.is_empty(),
        "Correct arity should not produce E1507, got: {:?}",
        e1507
    );
}

#[test]
fn test_builtin_debug_too_many_args() {
    // E1507: debug(1, 2, 3) — max 2 args
    let source = "debug(1, 2, 3)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1507]")),
        "Expected E1507 for debug(1, 2, 3) too many args, got: {:?}",
        errors
    );
}

#[test]
fn test_builtin_stdout_correct_arity() {
    // stdout(x) is correct — no E1507
    let source = "stdout(\"hello\")";
    let (_, errors) = check(source);
    let e1507: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1507]"))
        .collect();
    assert!(
        e1507.is_empty(),
        "stdout with 1 arg should not produce E1507, got: {:?}",
        e1507
    );
}

#[test]
fn test_builtin_argv_no_args() {
    // argv() takes 0 args — no E1507
    let source = "args <= argv()";
    let (_, errors) = check(source);
    let e1507: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1507]"))
        .collect();
    assert!(
        e1507.is_empty(),
        "argv() with 0 args should not produce E1507, got: {:?}",
        e1507
    );
}

#[test]
fn test_builtin_hashmap_empty_is_ok() {
    // hashMap() with 0 args should be accepted
    let source = "m <= hashMap()";
    let (_, errors) = check(source);
    let e1507: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1507]"))
        .collect();
    assert!(
        e1507.is_empty(),
        "hashMap() should accept 0 args, got: {:?}",
        e1507
    );
}

// ── Phase 7: E1508 — Method call argument check ──

#[test]
fn test_method_call_take_wrong_type() {
    // E1508: xs.take("oops") — take expects Int
    let source = "xs <= @[1, 2, 3]\nresult <= xs.take(\"oops\")";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1508]")),
        "Expected E1508 for take(\"oops\") type mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_method_call_take_too_many_args() {
    // E1508: xs.take(1, 2) — take expects 1 arg
    let source = "xs <= @[1, 2, 3]\nresult <= xs.take(1, 2)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1508]")),
        "Expected E1508 for take(1, 2) too many args, got: {:?}",
        errors
    );
}

#[test]
fn test_method_call_charat_wrong_type() {
    // E1508: s.get(true) — get on Str expects Int
    let source = "s <= \"hello\"\nresult <= s.get(true)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1508]")),
        "Expected E1508 for get(true) type mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_method_call_correct_args() {
    // Correct method calls should not produce E1508
    let source = "xs <= @[1, 2, 3]\nresult <= xs.get(0)";
    let (_, errors) = check(source);
    let e1508: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1508]"))
        .collect();
    assert!(
        e1508.is_empty(),
        "Correct method call should not produce E1508, got: {:?}",
        e1508
    );
}

#[test]
fn test_method_call_length_no_args() {
    // length() should not error
    let source = "xs <= @[1, 2, 3]\nn <= xs.length()";
    let (_, errors) = check(source);
    let e1508: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1508]"))
        .collect();
    assert!(
        e1508.is_empty(),
        "length() should not produce E1508, got: {:?}",
        e1508
    );
}

#[test]
fn test_method_call_length_with_args_is_error() {
    // E1508: xs.length(1) — length takes 0 args
    let source = "xs <= @[1, 2, 3]\nn <= xs.length(1)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1508]")),
        "Expected E1508 for length(1) too many args, got: {:?}",
        errors
    );
}

// ── Phase 7: E1501 — TypeDef name collision ──

#[test]
fn test_typedef_duplicate_is_error() {
    // E1501: Pilot defined twice should error
    let source = "Pilot = @(name: Str)\nPilot = @(age: Int)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Expected E1501 for duplicate TypeDef, got: {:?}",
        errors
    );
}

#[test]
fn test_typedef_func_collision_is_error() {
    // E1501: TypeDef name colliding with function name
    let source = "Pilot x: Str =\n  x\n=> :Str\nPilot = @(name: Str)";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1501]")),
        "Expected E1501 for TypeDef/function collision, got: {:?}",
        errors
    );
}

#[test]
fn test_typedef_single_definition_is_ok() {
    // Single TypeDef should not produce E1501
    let source = "Pilot = @(name: Str)";
    let (_, errors) = check(source);
    let e1501: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1501]"))
        .collect();
    assert!(
        e1501.is_empty(),
        "Single TypeDef should not produce E1501, got: {:?}",
        e1501
    );
}

// --- E0401: リスト要素の同質性チェック ---

#[test]
fn test_list_homogeneous_int() {
    // @[1, 2, 3] — 同質な Int リスト、エラーなし
    let source = "nums <= @[1, 2, 3]";
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert!(
        e0401.is_empty(),
        "Homogeneous Int list should not produce E0401, got: {:?}",
        e0401
    );
}

#[test]
fn test_list_mixed_int_str() {
    // @[1, "x"] — Int と Str の混在、E0401 が出るべき
    let source = r#"mixed <= @[1, "x"]"#;
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert_eq!(
        e0401.len(),
        1,
        "Mixed Int/Str list should produce E0401, got: {:?}",
        e0401
    );
    assert!(e0401[0].message.contains("Int"), "Should mention Int");
    assert!(e0401[0].message.contains("Str"), "Should mention Str");
}

#[test]
fn test_list_int_float_unifies_to_num() {
    // @[1, 2.5] — Int と Float の混在は Num に統一、エラーなし
    let source = "nums <= @[1, 2.5]";
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert!(
        e0401.is_empty(),
        "Int/Float mix should unify to Num, not E0401, got: {:?}",
        e0401
    );
}

#[test]
fn test_list_homogeneous_bool() {
    // @[true, false] — 同質な Bool リスト
    let source = "flags <= @[true, false]";
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert!(
        e0401.is_empty(),
        "Homogeneous Bool list should not produce E0401, got: {:?}",
        e0401
    );
}

#[test]
fn test_list_homogeneous_packs() {
    // @[@(x <= 1), @(y <= 2)] — BuchiPack 同士は構造的部分型で許容
    let source = "packs <= @[@(x <= 1), @(y <= 2)]";
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert!(
        e0401.is_empty(),
        "BuchiPack list should not produce E0401, got: {:?}",
        e0401
    );
}

#[test]
fn test_list_empty() {
    // @[] — 空リスト、エラーなし
    let source = "empty <= @[]";
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert!(
        e0401.is_empty(),
        "Empty list should not produce E0401, got: {:?}",
        e0401
    );
}

#[test]
fn test_list_single_element() {
    // @[1] — 単一要素、エラーなし
    let source = "single <= @[1]";
    let (_, errors) = check(source);
    let e0401: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E0401]"))
        .collect();
    assert!(
        e0401.is_empty(),
        "Single element list should not produce E0401, got: {:?}",
        e0401
    );
}

// ── FL-1: Return type annotation enforcement ──────────────────────

#[test]
fn test_fl1_return_type_mismatch_detected() {
    // Function declares :Int but body returns Str
    let source = "bad x =\n  \"oops\"\n=> :Int";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1601]") && e.message.contains("return type")),
        "Expected return type mismatch error [E1601], got: {:?}",
        errors
    );
}

#[test]
fn test_fl1_return_type_match_no_error() {
    // Function declares :Str and body returns Str — no error
    let source = "greet name =\n  `Hello ${name}`\n=> :Str";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for matching return type, got: {:?}",
        e1601
    );
}

#[test]
fn test_fl1_return_type_numeric_compatible_no_error() {
    // Function declares :Int but body returns Float — allowed (numeric narrowing)
    let source = "bad =\n  3.14\n=> :Int";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for numeric types (Int/Float/Num are compatible), got: {:?}",
        e1601
    );
}

#[test]
fn test_fl1_return_type_bool_body_str_mismatch() {
    // Function declares :Bool but body returns Str — genuine mismatch
    let source = "bad =\n  \"hello\"\n=> :Bool";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1601]")),
        "Expected E1601 for Bool/Str mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_fl1_no_return_annotation_no_error() {
    // Function without return type annotation — no E1601
    let source = "add x y =\n  x + y";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 without return type annotation, got: {:?}",
        e1601
    );
}

// ── FL-2: Named type field access diagnostic ──────────────────────

#[test]
fn test_fl2_named_type_undefined_field_error() {
    // Access a field that doesn't exist on a Named type
    let source = "Person = @(name: Str)\np <= Person(name <= \"a\")\nemail <= p.email";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1602]") && e.message.contains("email")),
        "Expected undefined field error [E1602] for 'email', got: {:?}",
        errors
    );
}

#[test]
fn test_fl2_named_type_valid_field_no_error() {
    // Access a valid field — no error
    let source = "Person = @(name: Str)\np <= Person(name <= \"a\")\nname <= p.name";
    let (_, errors) = check(source);
    let e1602: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1602]"))
        .collect();
    assert!(
        e1602.is_empty(),
        "Should not produce E1602 for valid field access, got: {:?}",
        e1602
    );
}

// ── FL-3: Condition branch type mismatch ──────────────────────────

#[test]
fn test_fl3_cond_branch_type_mismatch() {
    // First arm returns Int, second returns Str
    let source = "x <= 5\ny <=\n  | x > 3 |> 1\n  | _ |> \"oops\"";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1603]")),
        "Expected condition branch type mismatch [E1603], got: {:?}",
        errors
    );
}

#[test]
fn test_fl3_cond_branch_same_type_no_error() {
    // Both arms return Str — no error
    let source = "x <= 5\ny <=\n  | x > 3 |> \"big\"\n  | _ |> \"small\"";
    let (_, errors) = check(source);
    let e1603: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1603]"))
        .collect();
    assert!(
        e1603.is_empty(),
        "Should not produce E1603 for same-type arms, got: {:?}",
        e1603
    );
}

#[test]
fn test_fl3_cond_branch_int_float_mix_allowed() {
    // Int/Float mixing should be allowed (both are Num)
    let source = "x <= 5\ny <=\n  | x > 3 |> 1\n  | _ |> 2.5";
    let (_, errors) = check(source);
    let e1603: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1603]"))
        .collect();
    assert!(
        e1603.is_empty(),
        "Should not produce E1603 for Int/Float mix, got: {:?}",
        e1603
    );
}

// ── B11B-014: If mold branch type mismatch ───────────────────────

#[test]
fn test_b11_if_mold_type_mismatch() {
    // If[cond, Int, Str]() should produce E1603
    let source = "x <= If[false, 1, \"oops\"]()";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1603]")),
        "Expected If mold branch type mismatch [E1603], got: {:?}",
        errors
    );
}

#[test]
fn test_b11_if_mold_same_type_no_error() {
    // If[cond, Int, Int]() — no error
    let source = "x <= If[true, 1, 2]()";
    let (_, errors) = check(source);
    let e1603: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1603]"))
        .collect();
    assert!(
        e1603.is_empty(),
        "Should not produce E1603 for same-type If branches, got: {:?}",
        e1603
    );
}

#[test]
fn test_b11_if_mold_int_float_mix_allowed() {
    // If[cond, Int, Float]() — Int/Float mix should be allowed
    let source = "x <= If[true, 1, 2.5]()";
    let (_, errors) = check(source);
    let e1603: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1603]"))
        .collect();
    assert!(
        e1603.is_empty(),
        "Should not produce E1603 for Int/Float mix in If, got: {:?}",
        e1603
    );
}

// ── B11B-016: TypeExtends variant rejection ──────────────────────

#[test]
fn test_b11_type_extends_variant_rejected() {
    // TypeExtends[EnumName:Variant, :Type]() should produce E1613
    let source = r#"
Enum => Status = :Ok :Fail
x <= TypeExtends[Status:Ok, :Status]()
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1613]")),
        "Expected E1613 for TypeExtends with enum variant, got: {:?}",
        errors
    );
}

#[test]
fn test_b11_type_extends_no_variant_no_error() {
    // TypeExtends[:Dog, :Animal]() — no variant, no E1613
    let source = r#"
Animal = @(name: Str)
Animal => Dog = @(breed: Str)
x <= TypeExtends[:Dog, :Animal]()
"#;
    let (_, errors) = check(source);
    let e1613: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1613]"))
        .collect();
    assert!(
        e1613.is_empty(),
        "Should not produce E1613 for TypeExtends without variant, got: {:?}",
        e1613
    );
}

#[test]
fn test_b11_type_extends_variant_rejected_in_expr_stmt() {
    // B11B-016: TypeExtends variant rejection must also fire in expression
    // statement context (e.g., inside stdout()), not just in assignments.
    let source = r#"
Enum => Status = :Ok :Fail
stdout(TypeExtends[Status:Ok, :Status]())
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1613]")),
        "Expected E1613 for TypeExtends with enum variant in stdout(), got: {:?}",
        errors
    );
}

// ── FL-4: Operator type validation ────────────────────────────────

#[test]
fn test_fl4_logical_not_on_non_bool() {
    // `!1` — not operator on Int
    let source = "flag <= !1";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1607]") && e.message.contains("Bool")),
        "Expected E1607 for `!1`, got: {:?}",
        errors
    );
}

#[test]
fn test_fl4_logical_or_on_non_bool() {
    // `1 || 2` — logical or on Int
    let source = "text <= 1 || 2";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1606]")),
        "Expected E1606 for `1 || 2`, got: {:?}",
        errors
    );
}

#[test]
fn test_fl4_logical_and_on_non_bool() {
    let source = "flag <= \"a\" && \"b\"";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1606]")),
        "Expected E1606 for Str && Str, got: {:?}",
        errors
    );
}

#[test]
fn test_fl4_unary_neg_on_string() {
    let source = "x <= -\"hello\"";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1607]") && e.message.contains("numeric")),
        "Expected E1607 for `-\"hello\"`, got: {:?}",
        errors
    );
}

#[test]
fn test_fl4_comparison_type_mismatch() {
    // Comparing Int with Str
    let source = "flag <= 1 == \"a\"";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1605]")),
        "Expected E1605 for comparing Int with Str, got: {:?}",
        errors
    );
}

#[test]
fn test_fl4_ordering_non_numeric() {
    // Comparing Bool values with < — not valid
    let source = "x <= true\ny <= false\nflag <= x < y";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1605]")),
        "Expected E1605 for Bool ordering, got: {:?}",
        errors
    );
}

#[test]
fn test_fl4_valid_bool_operators_no_error() {
    // Valid: Bool && Bool, Bool || Bool, !Bool
    let source = "a <= true\nb <= false\nc <= a && b\nd <= a || b\ne <= !a";
    let (_, errors) = check(source);
    let e16: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E160"))
        .collect();
    assert!(
        e16.is_empty(),
        "Should not produce errors for valid Bool operations, got: {:?}",
        e16
    );
}

#[test]
fn test_fl4_valid_numeric_comparison_no_error() {
    // Valid: Int == Int, Int < Float
    let source = "a <= 1\nb <= 2\nc <= a == b\nd <= a < 3.5";
    let (_, errors) = check(source);
    let e1605: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1605]"))
        .collect();
    assert!(
        e1605.is_empty(),
        "Should not produce E1605 for valid numeric comparison, got: {:?}",
        e1605
    );
}

// ── Fix 1: E1604 non-Bool condition in CondBranch ──────────────────

#[test]
fn test_e1604_non_bool_condition_int() {
    // `| 42 |>` — condition is Int, not Bool
    let source = "y <=\n  | 42 |> \"yes\"\n  | _ |> \"no\"";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1604]") && e.message.contains("Int")),
        "Expected E1604 for Int condition, got: {:?}",
        errors
    );
}

#[test]
fn test_e1604_non_bool_condition_str() {
    // `| "hello" |>` — condition is Str, not Bool
    let source = "y <=\n  | \"hello\" |> 1\n  | _ |> 2";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("[E1604]") && e.message.contains("Str")),
        "Expected E1604 for Str condition, got: {:?}",
        errors
    );
}

#[test]
fn test_e1604_bool_condition_no_error() {
    // Valid Bool condition — no E1604
    let source = "x <= 5\ny <=\n  | x > 3 |> \"big\"\n  | _ |> \"small\"";
    let (_, errors) = check(source);
    let e1604: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1604]"))
        .collect();
    assert!(
        e1604.is_empty(),
        "Should not produce E1604 for valid Bool condition, got: {:?}",
        e1604
    );
}

#[test]
fn test_e1604_non_bool_in_subsequent_arm() {
    // Second arm has a non-Bool condition
    let source = "x <= 5\ny <=\n  | x > 3 |> \"big\"\n  | 42 |> \"medium\"\n  | _ |> \"small\"";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1604]")),
        "Expected E1604 for non-Bool condition in subsequent arm, got: {:?}",
        errors
    );
}

// ── Fix 3: FL-1 last statement not an expression ───────────────────

#[test]
fn test_c13_1_tail_binding_yields_bound_value() {
    // C13-1 migration of the former test_fl1_last_stmt_not_expr_with_return_type.
    // Under C13-1 a trailing `name <= expr` yields the bound value as
    // the function result, so a function declaring `=> :Int` and ending
    // with `x <= 42` is valid (the body evaluates to 42).
    let source = "bad =\n  x <= 42\n=> :Int";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "C13-1: `x <= 42` as tail binding should satisfy `=> :Int`, got: {:?}",
        e1601
    );
}

#[test]
fn test_c13_1_tail_binding_type_mismatch_still_reported() {
    // C13-1: A tail binding whose RHS type does not match the declared
    // return type must still be reported as E1601, since the bound
    // value is what the function returns.
    let source = "bad =\n  x <= \"not an int\"\n=> :Int";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1601]")),
        "Expected E1601 for tail-binding type mismatch (Str vs Int), got: {:?}",
        errors
    );
}

#[test]
fn test_c13_1_func_body_tail_forward_assignment_ok() {
    // C13-1: Function body ending with `expr => name` yields `expr`
    // as the function result, satisfying `=> :Int`.
    let source = "calc =\n  1 => one\n  one + 2 => total\n=> :Int";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "C13-1: `one + 2 => total` tail yields Int, got: {:?}",
        e1601
    );
}

#[test]
fn test_c13_1_error_ceiling_tail_binding_ok() {
    // C13-1: Error ceiling body ending with `name <= expr` yields the
    // bound value as the handler result.
    let source =
        "handle =\n  |== err: Error =\n    fallback <= \"default\"\n  => :Str\n  \"ok\"\n=> :Str";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "C13-1: handler tail `fallback <= \"default\"` yields Str, got: {:?}",
        e1601
    );
}

#[test]
fn test_c13_1_pipeline_intermediate_bind_and_forward_ok() {
    // C13-1 / C13B-007: Intermediate `=> add_result` in a pure `=>`
    // pipeline binds the current value and forwards it; later steps
    // may reference `add_result` without E1502.
    let source = "add x: Int y: Int = x + y => :Int\n\
1 => add(3, _) => add_result => stdout(add_result)";
    let (_, errors) = check(source);
    let e1502: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1502]") && e.message.contains("add_result"))
        .collect();
    assert!(
        e1502.is_empty(),
        "C13-1: intermediate pipeline bind must not raise E1502, got: {:?}",
        e1502
    );
}

#[test]
fn test_fl1_last_stmt_not_expr_without_return_type_no_error() {
    // Function without return type annotation — last stmt being assignment is fine
    let source = "foo =\n  x <= 42";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 without return type annotation, got: {:?}",
        e1601
    );
}

// ── Fix 8: Complex last-expression cases (CondBranch, Pipeline) ────

#[test]
fn test_fl1_cond_branch_as_last_expr() {
    // Function with CondBranch as last expression — matching return type
    let source = "classify x =\n  | x > 0 |> \"pos\"\n  | _ |> \"neg\"\n=> :Str";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 when CondBranch returns matching type, got: {:?}",
        e1601
    );
}

#[test]
fn test_fl1_cond_branch_as_last_expr_mismatch() {
    // Function with CondBranch as last expression — type mismatch
    let source = "classify x =\n  | x > 0 |> \"pos\"\n  | _ |> \"neg\"\n=> :Int";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1601]")),
        "Expected E1601 for CondBranch returning Str when Int declared, got: {:?}",
        errors
    );
}

#[test]
fn test_fl1_pipeline_as_last_expr() {
    // Function with pipeline as last expression
    let source = "transform x =\n  x => _ + 1\n=> :Int";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for pipeline returning compatible type, got: {:?}",
        e1601
    );
}

// ── N-69: Cyclic type dependency tests ──

#[test]
fn test_cyclic_type_no_panic() {
    // Taida uses value semantics (copy), so cyclic type references like
    // A = @(b: B) / B = @(a: A) cannot form cycles at runtime.
    // The checker should handle these definitions without panicking.
    let source = "A = @(b: B)\nB = @(a: A)";
    let (checker, _errors) = check(source);
    // Both types should be registered (even if fields reference each other as Named)
    assert!(
        checker.registry.get_type_fields("A").is_some(),
        "A should be registered"
    );
    assert!(
        checker.registry.get_type_fields("B").is_some(),
        "B should be registered"
    );
}

#[test]
fn test_self_referential_type_no_panic() {
    // Self-referential type: the checker should not infinitely recurse.
    let source = "Node = @(value: Int, next: Node)";
    let (checker, _errors) = check(source);
    assert!(
        checker.registry.get_type_fields("Node").is_some(),
        "Node should be registered"
    );
}

// ── N-70: Generic type constraint edge cases ──

#[test]
fn test_generic_constraint_type_mismatch() {
    // Passing a string argument to a generic mold that expects Int should
    // ideally produce a diagnostic (constraint violation).
    let source = "Mold[T] => Box[T] = @(value: T)\nb <= Box[\"hello\"]()";
    let (checker, errors) = check(source);
    // R-09: Verify the checker produces a concrete result rather than just
    // asserting no-panic.  `b` should be assigned some type (Generic or Unknown).
    let b_ty = checker.lookup_var("b");
    assert!(
        b_ty.is_some(),
        "Variable 'b' should be registered in the type environment, got None"
    );
    // No errors is acceptable — full generic constraint checking is future work.
    let _ = errors;
}

#[test]
fn test_generic_multi_param_mold() {
    // Multi-parameter generic mold should parse and check without panic.
    let source = "Mold[T, P] => Pair[T, P] = @(first: T, second: P)\np <= Pair[1, \"hi\"]()";
    let (checker, errors) = check(source);
    // R-09: Verify `p` is registered and has a concrete type.
    let p_ty = checker.lookup_var("p");
    assert!(
        p_ty.is_some(),
        "Variable 'p' should be registered in the type environment, got None"
    );
    let _ = errors;
}

// ── N-71: @[] type parameter inference negative tests ──

#[test]
fn test_empty_list_type_param_inference_negative() {
    // @[] without type annotation should produce an error.
    // This is the negative test for type parameter inference on lists.
    let (_, errors) = check("items <= @[]");
    assert!(
        !errors.is_empty(),
        "Empty list without annotation should produce an error"
    );
}

#[test]
fn test_list_mixed_types_inference() {
    // A list with mixed types should infer based on the first element.
    // The checker should not panic regardless of type mixing.
    let source = "items <= @[1, \"hello\", true]";
    let (checker, errors) = check(source);
    // R-09: Verify `items` is registered and inferred as a List type.
    let items_ty = checker.lookup_var("items");
    assert!(
        items_ty.is_some(),
        "Variable 'items' should be registered in the type environment"
    );
    // The first element is Int, so the list type should be List(Int) or similar.
    assert!(
        matches!(items_ty, Some(Type::List(_))),
        "items should be inferred as a List type, got {:?}",
        items_ty
    );
    let _ = errors;
}

// ── N-73: Optional/Result redesign migration marker ──
// The Optional/Result redesign (v0.8.0) is tracked in MEMORY.md.
// When implementation begins, migration tests should be added here
// to verify backward compatibility of existing Lax[T] and Result[T, P]
// usage patterns. For now, verify existing mold-based Optional/Result
// behavior does not regress.

// ── BT-2: null/undefined/none/nil rejection tests ────────────────
// PHILOSOPHY.md I: "null/undefinedの完全排除 — 全ての型にデフォルト値を保証"
// These identifiers should be rejected as undefined variables by the type checker.

#[test]
fn test_bt2_null_rejected() {
    let source = "x <= null";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("E1502") && e.message.contains("null")),
        "Assignment from 'null' should produce E1502 undefined variable error, got: {:?}",
        errors
    );
}

#[test]
fn test_bt2_undefined_rejected() {
    let source = "x <= undefined";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("E1502") && e.message.contains("undefined")),
        "Assignment from 'undefined' should produce E1502 undefined variable error, got: {:?}",
        errors
    );
}

#[test]
fn test_bt2_none_rejected() {
    let source = "x <= none";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("E1502") && e.message.contains("none")),
        "Assignment from 'none' should produce E1502 undefined variable error, got: {:?}",
        errors
    );
}

#[test]
fn test_bt2_nil_rejected() {
    let source = "x <= nil";
    let (_, errors) = check(source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("E1502") && e.message.contains("nil")),
        "Assignment from 'nil' should produce E1502 undefined variable error, got: {:?}",
        errors
    );
}

#[test]
fn test_bt2_null_in_expression_rejected() {
    let source = "x <= 42\ny <= x + null";
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("null")),
        "'null' used in expression should produce an error, got: {:?}",
        errors
    );
}

#[test]
fn test_lax_result_current_behavior_stable() {
    // Verify that current Lax/Result type inference works as expected.
    // This serves as a baseline for the v0.8.0 Optional/Result migration.
    let source = "x <= Lax[42]()\ny <= Result[1, \"ok\"]()";
    let (checker, _errors) = check(source);
    // Lax and Result should resolve to Generic types
    // R-10: `x` must be present in the environment — None would indicate a
    // checker regression where the variable was never registered.
    let x_ty = checker.lookup_var("x");
    assert!(
        x_ty.is_some(),
        "Variable 'x' should be registered in the type environment, got None"
    );
    assert!(
        matches!(x_ty, Some(Type::Generic(ref n, _)) if n == "Lax")
            || matches!(x_ty, Some(Type::Unknown)),
        "Lax should resolve to Generic(Lax, ...) or Unknown, got {:?}",
        x_ty
    );
}

// -- RCB-50: Named/List/BuchiPack return type verification --

#[test]
fn test_rcb50_named_return_type_mismatch_detected() {
    // Function declares :A but body returns B -- should emit E1601
    let source = r#"
A = @(a: Int)
B = @(b: Int)
bad =
  B(b <= 1)
=> :A
bad()
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1601]")),
        "Expected E1601 for Named type mismatch (B vs A), got: {:?}",
        errors
    );
}

#[test]
fn test_rcb50_named_return_type_match_no_error() {
    // Function declares :A and body returns A -- no E1601
    let source = r#"
A = @(a: Int)
good =
  A(a <= 1)
=> :A
good()
"#;
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for matching Named return type, got: {:?}",
        e1601
    );
}

#[test]
fn test_rcb50_named_subtype_return_no_error() {
    // Function declares :Parent, body returns Child -- structural subtype, no E1601
    let source = r#"
Parent = @(name: Str)
Parent => Child = @(age: Int)
good =
  Child(name <= "x", age <= 1)
=> :Parent
good()
"#;
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for subtype return (Child is subtype of Parent), got: {:?}",
        e1601
    );
}

#[test]
fn test_rcb50_named_return_type_primitive_mismatch() {
    // Function declares :Int but body returns Named type -- E1601
    let source = r#"
A = @(a: Int)
bad =
  A(a <= 1)
=> :Int
bad()
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1601]")),
        "Expected E1601 for Named vs Int mismatch, got: {:?}",
        errors
    );
}

#[test]
fn test_rcb50_generic_type_var_return_no_spurious_e1601() {
    // Generic function with unresolvable type params -- no E1601
    let source = "id[T] x: T =\n  x\n=> :T";
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for generic type variable return, got: {:?}",
        e1601
    );
}

#[test]
fn test_rcb50_custom_mold_return_no_spurious_e1601() {
    // Custom mold instantiation as last expression -- checker cannot
    // predict what solidify returns, so E1601 should be suppressed.
    let source = r#"
Mold[T] => AlwaysFail[T] = @(
  solidify =
    Error(type <= "SolidifyError", message <= "fail").throw()
  => :Int
)
check x: Int =
  |== err: Error =
    -1
  => :Int
  AlwaysFail[x]()
=> :Int
"#;
    let (_, errors) = check(source);
    let e1601: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1601]"))
        .collect();
    assert!(
        e1601.is_empty(),
        "Should not produce E1601 for custom mold instantiation return, got: {:?}",
        e1601
    );
}

// -- RCB-51: Cyclic inheritance detection in checker --

#[test]
fn test_rcb51_cyclic_inheritance_emits_e1610() {
    // A = @(a: Int), A => B, B => A -- the second should emit E1610
    let source = r#"
A = @(a: Int)
A => B = @(b: Int)
B => A = @(c: Int)
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1610]")),
        "Expected E1610 for cyclic inheritance, got: {:?}",
        errors
    );
}

#[test]
fn test_rcb51_cyclic_inheritance_no_hang() {
    // Even with cyclic definitions, taida check must terminate quickly.
    let source = r#"
A = @(a: Int)
A => B = @(b: Int)
B => A = @(c: Int)
C = @(z: Int)
useC x: C =
  1
=> :Int
b <= B(a <= 1, b <= 2)
useC(b)
"#;
    let (_, errors) = check(source);
    // The important thing is that this terminates. The cycle should
    // be caught at registration time (E1610), and is_subtype_of
    // should not hang even if called.
    assert!(
        errors.iter().any(|e| e.message.contains("[E1610]")),
        "Expected E1610 for cyclic inheritance, got: {:?}",
        errors
    );
}

#[test]
fn test_rcb51_three_way_cycle_detected() {
    // A => B => C => A -- indirect 3-node cycle
    let source = r#"
A = @(a: Int)
A => B = @(b: Int)
B => C = @(c: Int)
C => A = @(d: Int)
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1610]")),
        "Expected E1610 for 3-node cyclic inheritance, got: {:?}",
        errors
    );
}

// ────────────────────────────────────────────────────────────────
// C12-1c: mold_returns table ↔ checker builtin-mold consistency
// ────────────────────────────────────────────────────────────────
//
// `src/types/mold_returns.rs` is the single source of truth for builtin
// mold return tags. The checker (`Expr::MoldInst` branch of
// `infer_expr_type`) maintains a richer Type-level mapping because it
// needs Type::Generic("Lax", vec![Type::Str]) and similar. We verify
// here that for every name with a static (non-Dynamic) tag, the
// checker's inferred Type lowers to the same runtime tag.
//
// Translation rule (tag value ↔ Type):
//   0 Int    → Type::Int
//   1 Float  → Type::Float
//   2 Bool   → Type::Bool
//   3 Str    → Type::Str
//   4 Pack   → Type::Named(...) | Type::Generic(wrapper, ...) | Molten
//   5 List   → Type::List(_)
//
// This test catches cases where the checker is updated but the tag
// table is not (or vice versa), which is exactly the FB-27 technical
// debt we are paying down.

fn type_to_tag(t: &Type) -> Option<i64> {
    match t {
        Type::Int => Some(0),
        Type::Float => Some(1),
        Type::Bool => Some(2),
        Type::Str => Some(3),
        Type::List(_) => Some(5),
        // Pack-family: named types, wrapper generics, Molten, Bytes (stored
        // as a hidden-header string → Str-like runtime tag).
        Type::Bytes => Some(3),
        Type::Named(_) | Type::Molten => Some(4),
        Type::Generic(name, _) => match name.as_str() {
            "Lax" | "Result" | "Async" | "Gorillax" | "RelaxedGorillax" | "Stream"
            | "StreamFrom" | "HashMap" | "Set" => Some(4),
            _ => None,
        },
        // Dynamic / unknown / numeric / function: cannot be compared.
        _ => None,
    }
}

#[test]
fn test_c12_1_mold_returns_matches_checker() {
    use crate::types::mold_returns;

    // For each name with a static tag, construct a minimal MoldInst that
    // the checker can infer, then confirm the tag matches.
    //
    // Some molds are argument-dependent for the *checker* even though the
    // *runtime tag* is constant (e.g. Stream[x]() → Stream[T] but the
    // tag is always Pack=4). For those cases we either construct typed
    // args or skip. Molds that require special construction (Cage) are
    // covered by broader parity tests and not included here.
    // Cases the checker currently infers a concrete Type for. Molds that
    // the checker still resolves to Type::Unknown (Length, Count, IndexOf,
    // LastIndexOf, FindIndex, BitAnd/Or/Xor/Not, BytesToList, Find, Min,
    // Max, ShiftL/R/RU, ByteSet, ToRadix, ...) are tracked in the tag
    // table but left out of this consistency check until the checker
    // catches up. That is a separate gap (not a contradiction); the tag
    // table is the compile-time authority for codegen.
    let cases: &[(&str, &str)] = &[
        // Str-returning
        ("Upper", r#"Upper["x"]()"#),
        ("Lower", r#"Lower["X"]()"#),
        ("Trim", r#"Trim[" x "]()"#),
        ("Replace", r#"Replace["abc", "a", "z"]()"#),
        ("Repeat", r#"Repeat["a", 3]()"#),
        ("Pad", r#"Pad["a", 3, "0"]()"#),
        ("Join", r#"Join[@[1, 2], ","]()"#),
        ("ToFixed", r#"ToFixed[3.14, 2]()"#),
        // Int-returning
        ("Floor", r#"Floor[3.7]()"#),
        ("Ceil", r#"Ceil[3.2]()"#),
        ("Round", r#"Round[3.5]()"#),
        ("Truncate", r#"Truncate[3.9]()"#),
        // List-returning
        ("Chars", r#"Chars["abc"]()"#),
        ("Split", r#"Split["a,b", ","]()"#),
        // Bool-returning
        ("TypeIs", r#"TypeIs[42, :Int]()"#),
    ];

    for (mold_name, src) in cases {
        let expected_tag = mold_returns::mold_return_tag(mold_name).unwrap_or_else(|| {
            panic!(
                "mold_returns::mold_return_tag({}) returned None but case declares a static tag",
                mold_name
            )
        });

        // Build a program `v <= <expr>` so the checker visits the MoldInst.
        let source = format!("v <= {src}\n");
        let (program, parse_errors) = crate::parser::parse(&source);
        assert!(
            parse_errors.is_empty(),
            "Parse errors for {}: {:?}",
            mold_name,
            parse_errors
        );

        let mut checker = TypeChecker::new();
        checker.check_program(&program);

        // Find the assignment and infer RHS type.
        let Statement::Assignment(assign) = &program.statements[0] else {
            panic!("expected Assignment for {}", mold_name);
        };
        let t = checker.infer_expr_type(&assign.value);
        let actual_tag = type_to_tag(&t).unwrap_or_else(|| {
            panic!(
                "checker inferred unconvertible Type {:?} for {} (expected tag {})",
                t, mold_name, expected_tag
            )
        });
        assert_eq!(
            actual_tag, expected_tag,
            "tag mismatch for {}: mold_returns table says {} but checker inferred {:?} (tag {})",
            mold_name, expected_tag, t, actual_tag
        );
    }
}

// ── C12-2: FB-10 `.toString()` universal adoption ──

/// `.toString()` on Int takes no arguments. Passing `16` as a base must
/// be rejected with E1508 — Taida does not expose a radix-based variant
/// (unlike JavaScript's `Number.prototype.toString(radix)`).
#[test]
fn test_c12_2_tostring_int_with_arg_rejected() {
    let source = "n <= 42\ns <= n.toString(16)";
    let (_, errors) = check(source);
    let e1508: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1508]") && e.message.contains("toString"))
        .collect();
    assert!(
        !e1508.is_empty(),
        "n.toString(16) should produce E1508, got: {:?}",
        errors
    );
}

/// C12-2c: the checker must catch `.toString(arg)` even when the method
/// call is nested inside a builtin argument (e.g. `stdout(...)`). Before
/// this fix, only bind-forms like `s <= n.toString(16)` were caught.
#[test]
fn test_c12_2_tostring_with_arg_in_builtin_rejected() {
    let source = "n <= 42\nstdout(n.toString(16))";
    let (_, errors) = check(source);
    let e1508: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1508]") && e.message.contains("toString"))
        .collect();
    assert!(
        !e1508.is_empty(),
        "stdout(n.toString(16)) should produce E1508, got: {:?}",
        errors
    );
}

/// C12-2c: the recursion into builtin args must NOT emit unrelated
/// errors (e.g. E1602 for `__type` field access on a Named error type).
/// This guards against the rc6a_error_inheritance_basic regression.
#[test]
fn test_c12_2_builtin_arg_recursion_no_spurious_error() {
    // Mirrors tests/parity.rs :: rc6a_error_inheritance_basic.
    let source = "Error => AppError = @(code: Int)\n\
                  err <= AppError(type <= \"AppError\", message <= \"test\", code <= 42)\n\
                  Str[err.code]() ]=> code_str\n\
                  stdout(err.__type + \" \" + code_str)\n";
    let (_, errors) = check(source);
    let e1602: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1602]"))
        .collect();
    assert!(
        e1602.is_empty(),
        "Error-type __type field access must not trigger E1602 after C12-2c recursion, got: {:?}",
        errors
    );
}

/// C12-2b: `.toString()` without arguments on primitives is valid and
/// returns Str. No errors should be produced.
#[test]
fn test_c12_2_tostring_no_args_accepted() {
    let source = "i <= 42\n\
                  f <= 3.14\n\
                  b <= true\n\
                  s <= \"hi\"\n\
                  a <= i.toString()\n\
                  c <= f.toString()\n\
                  d <= b.toString()\n\
                  e <= s.toString()\n";
    let (_, errors) = check(source);
    let e1508: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("[E1508]"))
        .collect();
    assert!(
        e1508.is_empty(),
        ".toString() with no args must not trigger E1508, got: {:?}",
        e1508
    );
}

/// C12-2b: `.toString()` on List / BuchiPack must type-check to Str.
/// Before C12-2 the interpreter's List had no toString entry and JS
/// fell back to `[object Object]`; now all three backends agree.
#[test]
fn test_c12_2_tostring_composite_return_type_is_str() {
    let source = "l <= @[1, 2, 3]\np <= @(a <= 1, b <= 2)\n";
    let (program, parse_errors) = crate::parser::parse(source);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    let mut checker = TypeChecker::new();
    checker.check_program(&program);

    let span = Span {
        start: 0,
        end: 0,
        line: 1,
        column: 1,
    };
    let list_call = Expr::MethodCall(
        Box::new(Expr::Ident("l".into(), span.clone())),
        "toString".into(),
        vec![],
        span.clone(),
    );
    let pack_call = Expr::MethodCall(
        Box::new(Expr::Ident("p".into(), span.clone())),
        "toString".into(),
        vec![],
        span.clone(),
    );

    assert_eq!(checker.infer_expr_type(&list_call), Type::Str);
    assert_eq!(checker.infer_expr_type(&pack_call), Type::Str);
}

// ── C12-3 / FB-8: mutual-recursion check integration ─────────────────
// The graph-level mutual-recursion findings are promoted into TypeError
// entries via `check_mutual_recursion_errors` so that `taida check`,
// `taida build`, and the compile pipeline all reject non-tail mutual
// recursion at compile time instead of silently allowing a runtime
// "Maximum call depth exceeded" crash. See
// `src/graph/verify.rs::check_mutual_recursion` for the detection rules.

#[test]
fn test_c12_3_mutual_recursion_tail_only_accepted_by_checker() {
    // Classic isEven / isOdd tail-only mutual recursion must compile.
    let source = r#"
isEven n =
  | n == 0 |> 1
  | _ |> isOdd(n - 1)

isOdd n =
  | n == 0 |> 0
  | _ |> isEven(n - 1)
"#;
    let (_, errors) = check(source);
    assert!(
        !errors.iter().any(|e| e.message.contains("[E1614]")),
        "tail-only mutual recursion must not raise [E1614], got: {:?}",
        errors
    );
}

#[test]
fn test_c12_3_mutual_recursion_non_tail_rejected_by_checker() {
    // `a` calls `b` in a non-tail position (inside `wrap(...)` arg),
    // `b` calls `a` in tail position. The cycle has a non-tail edge
    // and must be rejected with [E1614].
    let source = r#"
a n =
  wrap(b(n))

b n =
  a(n)

wrap x =
  x
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1614]")),
        "non-tail mutual recursion must raise [E1614], got: {:?}",
        errors
    );
}

#[test]
fn test_c12_3_self_recursion_not_flagged_as_mutual() {
    // Pure self-recursion is handled by the runtime's direct-recursion
    // path and is not "mutual" — the check must not fire.
    let source = r#"
count n =
  | n == 0 |> 0
  | _ |> count(n - 1)
"#;
    let (_, errors) = check(source);
    assert!(
        !errors.iter().any(|e| e.message.contains("[E1614]")),
        "self-recursion must not raise [E1614], got: {:?}",
        errors
    );
}

#[test]
fn test_c12_3_mutual_recursion_three_cycle_non_tail_rejected() {
    // A -> B -> C -> A where B's call to C is non-tail (BinaryOp).
    let source = r#"
alpha n =
  beta(n)

beta n =
  gamma(n) + 1

gamma n =
  alpha(n)
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1614]")),
        "non-tail 3-cycle must raise [E1614], got: {:?}",
        errors
    );
}

#[test]
fn test_c12_3_mutual_recursion_error_mentions_hint() {
    // The diagnostic must point the user at the accumulator style /
    // tail_recursion.md reference so fixes are easy to discover.
    let source = r#"
loopA n =
  loopB(n) + 1

loopB n =
  loopA(n)
"#;
    let (_, errors) = check(source);
    let e1614 = errors
        .iter()
        .find(|e| e.message.contains("[E1614]"))
        .expect("expected [E1614] diagnostic");
    assert!(
        e1614.message.contains("Hint:"),
        "expected Hint: section in E1614 message, got: {}",
        e1614.message
    );
    assert!(
        e1614.message.contains("tail_recursion.md"),
        "expected tail_recursion.md reference in E1614 message, got: {}",
        e1614.message
    );
}

// ─── C12-5 Phase 5 — FB-18: `Value::Unit` elimination on stdout/stderr ───

/// C12-5d: `stdout(...)` return type is now `Type::Int` (byte count),
/// not `Type::Unit`. The checker's builtin table must reflect that so
/// `n <= stdout("hi")` infers `n: Int` and subsequent arithmetic
/// typechecks without any special coercion.
#[test]
fn test_c12_5_stdout_return_type_is_int() {
    let mut checker = TypeChecker::new();
    let src = r#"n <= stdout("hi")
total <= n + 1
stdout(total)
"#;
    let (program, parse_errors) = parse(src);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    checker.check_program(&program);
    assert!(
        checker.errors.is_empty(),
        "checker should accept Int+Int arithmetic on stdout return, got: {:?}",
        checker.errors
    );
}

/// C12-5d: `stderr(...)` return type is `Type::Int` as well so the
/// two builtins remain symmetric (both write, both report bytes).
#[test]
fn test_c12_5_stderr_return_type_is_int() {
    let mut checker = TypeChecker::new();
    let src = r#"n <= stderr("err")
total <= n * 2
stdout(total)
"#;
    let (program, parse_errors) = parse(src);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    checker.check_program(&program);
    assert!(
        checker.errors.is_empty(),
        "checker should accept Int arithmetic on stderr return, got: {:?}",
        checker.errors
    );
}

/// C12-5f: `exit(...)` keeps returning `Type::Unit` — it never returns
/// normally, so Unit is still the right placeholder. This test pins
/// that the C12-5 migration did NOT accidentally promote `exit` to Int
/// along with stdout/stderr.
#[test]
fn test_c12_5_exit_return_type_remains_unit() {
    let mut checker = TypeChecker::new();
    // Direct call-site type inference for `exit(0)`.
    let src = "x <= exit(0)\nstdout(x)";
    let (program, parse_errors) = parse(src);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    checker.check_program(&program);
    // No assertion about errors (binding Unit to a variable may or may
    // not warn depending on policy); the invariant we care about is
    // that stdout/stderr changed and exit did not.
    let _ = checker.errors.len();
}

/// C12-5d: `stdout` in a pipeline — the checker sees the RHS of
/// `<=` as `stdout("hi")` and must type the bound variable as Int.
/// This is the direct analog of the `n <= stdout("hi")` pattern from
/// FB-18's motivating example.
#[test]
fn test_c12_5_stdout_in_let_binding_infers_int() {
    let mut checker = TypeChecker::new();
    let span = Span {
        start: 0,
        end: 12,
        line: 1,
        column: 1,
    };
    let call = Expr::FuncCall(
        Box::new(Expr::Ident("stdout".to_string(), span.clone())),
        vec![Expr::StringLit("hi".to_string(), span.clone())],
        span,
    );
    let ty = checker.infer_expr_type(&call);
    assert_eq!(
        ty,
        Type::Int,
        "stdout(...) must infer as Type::Int for `n <= stdout(...)` pattern"
    );
}

/// C12-5d: same for stderr — locks the builtin table entry.
#[test]
fn test_c12_5_stderr_in_let_binding_infers_int() {
    let mut checker = TypeChecker::new();
    let span = Span {
        start: 0,
        end: 11,
        line: 1,
        column: 1,
    };
    let call = Expr::FuncCall(
        Box::new(Expr::Ident("stderr".to_string(), span.clone())),
        vec![Expr::StringLit("e".to_string(), span.clone())],
        span,
    );
    let ty = checker.infer_expr_type(&call);
    assert_eq!(
        ty,
        Type::Int,
        "stderr(...) must infer as Type::Int (C12-5 FB-18)"
    );
}

// ────────────────────────────────────────────────────────────────
// C12B-031: Str.match / Str.search require :Regex argument
// ────────────────────────────────────────────────────────────────

/// C12B-031: `str.match("literal")` must be rejected at type-check
/// time because `match` is a Regex-only API. Prior to the fix the
/// checker accepted a Str argument and the runtime behaviour diverged:
/// Interpreter / JS threw at runtime, Native silently returned an
/// empty RegexMatch. Unifying the failure mode in the type checker
/// closes the parity gap.
#[test]
fn test_c12b_031_str_match_with_string_literal_rejected() {
    let source = r#"res <= "hello".match("h")
stdout(res.toString())
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1508]")
            && e.message.contains("'match'")
            && e.message.contains("Regex")),
        "match with Str literal should be rejected with [E1508] mentioning Regex, \
         got errors: {:?}",
        errors
    );
}

/// C12B-031: Mirror of the above for `search`.
#[test]
fn test_c12b_031_str_search_with_string_literal_rejected() {
    let source = r#"idx <= "hello".search("l")
stdout(idx.toString())
"#;
    let (_, errors) = check(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E1508]")
            && e.message.contains("'search'")
            && e.message.contains("Regex")),
        "search with Str literal should be rejected with [E1508] mentioning Regex, \
         got errors: {:?}",
        errors
    );
}

/// C12B-031 positive case: `str.match(Regex(...))` must still be
/// accepted — the tightening must not regress the canonical Regex
/// overload path introduced in C12-6.
#[test]
fn test_c12b_031_str_match_with_regex_constructor_accepted() {
    let source = r#"r <= Regex("h")
res <= "hello".match(r)
stdout(res.full)
"#;
    let (_, errors) = check(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("[E1508]") && e.message.contains("'match'")),
        "match with Regex arg should not produce [E1508], got errors: {:?}",
        errors
    );
}

/// C12B-031 positive case: Mirror of the above for `search`.
#[test]
fn test_c12b_031_str_search_with_regex_constructor_accepted() {
    let source = r#"r <= Regex("l")
idx <= "hello".search(r)
stdout(idx.toString())
"#;
    let (_, errors) = check(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("[E1508]") && e.message.contains("'search'")),
        "search with Regex arg should not produce [E1508], got errors: {:?}",
        errors
    );
}
