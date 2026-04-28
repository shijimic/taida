use super::*;

fn parse_ok(source: &str) -> Program {
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    program
}

fn first_stmt(source: &str) -> Statement {
    let program = parse_ok(source);
    assert!(!program.statements.is_empty(), "No statements parsed");
    program.statements.into_iter().next().unwrap()
}

#[test]
fn test_parse_assignment() {
    match first_stmt("x <= 42") {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "x");
            assert!(matches!(a.value, Expr::IntLit(42, _)));
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_string_assignment() {
    match first_stmt("name <= \"Alice\"") {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "name");
            match &a.value {
                Expr::StringLit(s, _) => assert_eq!(s, "Alice"),
                other => panic!("Expected StringLit, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_type_def() {
    // (E30 Sub-step 2.1) ClassLikeDef + ClassLikeKind::BuchiPack
    match first_stmt("Person = @(name: Str, age: Int)") {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.name, "Person");
            assert_eq!(td.fields.len(), 2);
            assert_eq!(td.fields[0].name, "name");
            assert_eq!(td.fields[1].name, "age");
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

// E30 Phase 2 Sub-step 2.2 (Lock-B Sub-B1): zero-arity sugar
#[test]
fn test_parse_class_like_zero_arity_sugar() {
    // `Pilot[] = @(...)` は `Pilot = @(...)` と等価な BuchiPack ClassLikeDef
    match first_stmt("Pilot[] = @(name: Str, age: Int)") {
        Statement::ClassLikeDef(cl) if cl.is_buchi_pack() => {
            assert_eq!(cl.name, "Pilot");
            assert_eq!(cl.fields.len(), 2);
            assert!(
                cl.type_params.is_empty(),
                "zero-arity sugar should have empty type_params, got {:?}",
                cl.type_params
            );
            // header_args は empty なので name_args は None に正規化される
            assert!(
                cl.name_args.is_none(),
                "zero-arity sugar should normalise name_args to None"
            );
        }
        other => panic!(
            "Expected zero-arity BuchiPack ClassLikeDef, got {:?}",
            other
        ),
    }
}

// E30 Phase 2 Sub-step 2.2 (Lock-B Sub-B1): 型引数あり class-like 定義
#[test]
fn test_parse_class_like_with_type_params() {
    // `Box[T] = @(filling: T, label: Str)` は BuchiPack ClassLikeDef + type_params=[T]
    match first_stmt("Box[T] = @(filling: T, label: Str)") {
        Statement::ClassLikeDef(cl) if cl.is_buchi_pack() => {
            assert_eq!(cl.name, "Box");
            assert_eq!(cl.fields.len(), 2);
            assert_eq!(
                cl.type_params.len(),
                1,
                "expected 1 type_param, got {:?}",
                cl.type_params
            );
            assert_eq!(cl.type_params[0].name, "T");
        }
        other => panic!(
            "Expected BuchiPack ClassLikeDef with type_params, got {:?}",
            other
        ),
    }
}

// E30 Phase 2 Sub-step 2.2 (Lock-B Sub-B1): 複数型引数あり class-like 定義
#[test]
fn test_parse_class_like_with_multi_type_params() {
    match first_stmt("Pair[T, U] = @(first: T, second: U)") {
        Statement::ClassLikeDef(cl) if cl.is_buchi_pack() => {
            assert_eq!(cl.name, "Pair");
            assert_eq!(cl.type_params.len(), 2);
            assert_eq!(cl.type_params[0].name, "T");
            assert_eq!(cl.type_params[1].name, "U");
        }
        other => panic!(
            "Expected multi-type-param BuchiPack ClassLikeDef, got {:?}",
            other
        ),
    }
}

// E30 Phase 2 Sub-step 2.2 (Lock-B Sub-B2): `Error =>` prefix の特別扱い撤廃確認
//
// 現状の parser は `Name => Child = @(...)` を一般 inheritance として受理しており、
// Error も特別扱いせずに通常の Inheritance kind ClassLikeDef に着地する。
// 本 test は Sub-B2 verdict (Error は単なる base 型) の parse 段階での pin。
#[test]
fn test_parse_error_prefix_is_general_inheritance() {
    match first_stmt("Error => NotFound = @(msg: Str)") {
        Statement::ClassLikeDef(cl) if cl.is_inheritance() => {
            assert_eq!(cl.name, "NotFound");
            assert_eq!(
                cl.parent(),
                Some("Error"),
                "Error should appear as a regular parent type, not as a special prefix"
            );
            assert_eq!(cl.fields.len(), 1);
            assert_eq!(cl.fields[0].name, "msg");
        }
        other => panic!(
            "Expected Inheritance ClassLikeDef with parent=Error, got {:?}",
            other
        ),
    }
}

// E30 Phase 2 Sub-step 2.2 (Lock-B Sub-B3): 子側型引数追加許容 (parser 受理のみ pin)
//
// 現状の parser は parent_args / child_args の arity 一致を検証しない。
// arity 一致検証は Phase 3 (checker) で `[E1407]` 再定義として実装される
// (E30B-008 と同期 land)。本 test は parser 受理パスの pin のみ。
#[test]
fn test_parse_child_with_extra_type_params() {
    match first_stmt("Result[T, P] => CustomResult[T, P, V] = @(meta: V)") {
        Statement::ClassLikeDef(cl) if cl.is_inheritance() => {
            assert_eq!(cl.name, "CustomResult");
            assert_eq!(cl.parent(), Some("Result"));
            // parent_args は 2、child の name_args は 3
            let parent_args = cl
                .parent_args()
                .expect("inheritance should have parent_args");
            assert_eq!(parent_args.len(), 2);
            let child_args = cl
                .name_args
                .as_ref()
                .expect("child should carry its own name_args");
            assert_eq!(child_args.len(), 3);
        }
        other => panic!(
            "Expected Inheritance ClassLikeDef with parent_args=2 / child_args=3, got {:?}",
            other
        ),
    }
}

#[test]
fn test_parse_enum_def_single_line() {
    match first_stmt("Enum => Status = :Ok :Fail") {
        Statement::EnumDef(ed) => {
            assert_eq!(ed.name, "Status");
            assert_eq!(ed.variants.len(), 2);
            assert_eq!(ed.variants[0].name, "Ok");
            assert_eq!(ed.variants[1].name, "Fail");
        }
        other => panic!("Expected EnumDef, got {:?}", other),
    }
}

#[test]
fn test_parse_enum_def_multiline() {
    match first_stmt("Enum => Status =\n  :Ok\n  :Fail") {
        Statement::EnumDef(ed) => {
            assert_eq!(ed.name, "Status");
            assert_eq!(ed.variants.len(), 2);
            assert_eq!(ed.variants[0].name, "Ok");
            assert_eq!(ed.variants[1].name, "Fail");
        }
        other => panic!("Expected EnumDef, got {:?}", other),
    }
}

#[test]
fn test_parse_enum_def_single_variant() {
    match first_stmt("Enum => Traffic = :Red") {
        Statement::EnumDef(ed) => {
            assert_eq!(ed.name, "Traffic");
            assert_eq!(ed.variants.len(), 1);
            assert_eq!(ed.variants[0].name, "Red");
        }
        other => panic!("Expected EnumDef, got {:?}", other),
    }
}

#[test]
fn test_parse_enum_variant_constructor() {
    match first_stmt("status <= Status:Ok()") {
        Statement::Assignment(assign) => match assign.value {
            Expr::EnumVariant(enum_name, variant_name, _) => {
                assert_eq!(enum_name, "Status");
                assert_eq!(variant_name, "Ok");
            }
            other => panic!("Expected EnumVariant, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_enum_variant_pipeline_assignment() {
    let program = parse_ok("status <= Status:Fail()\nstatus => id(_) => result");
    match &program.statements[1] {
        Statement::Assignment(assign) => {
            assert_eq!(assign.target, "result");
            match &assign.value {
                Expr::Pipeline(steps, _) => {
                    assert!(
                        matches!(steps.first(), Some(Expr::Ident(name, _)) if name == "status")
                    );
                    assert_eq!(steps.len(), 2);
                }
                Expr::FuncCall(_, _, _) => {}
                other => panic!("Expected Pipeline or FuncCall, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_buchi_pack_literal() {
    match first_stmt("user <= @(name <= \"Alice\", age <= 30)") {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "user");
            match &a.value {
                Expr::BuchiPack(fields, _) => {
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name, "name");
                    assert_eq!(fields[1].name, "age");
                }
                other => panic!("Expected BuchiPack, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_list_literal() {
    match first_stmt("numbers <= @[1, 2, 3]") {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "numbers");
            match &a.value {
                Expr::ListLit(items, _) => {
                    assert_eq!(items.len(), 3);
                }
                other => panic!("Expected ListLit, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_arithmetic() {
    match first_stmt("result <= 1 + 2 * 3") {
        Statement::Assignment(a) => {
            // Should parse as 1 + (2 * 3) due to precedence
            match &a.value {
                Expr::BinaryOp(_, BinOp::Add, right, _) => {
                    assert!(matches!(
                        right.as_ref(),
                        Expr::BinaryOp(_, BinOp::Mul, _, _)
                    ));
                }
                other => panic!("Expected BinaryOp(Add), got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_comparison() {
    match first_stmt("result <= x > 0 && y > 0") {
        Statement::Assignment(a) => {
            match &a.value {
                Expr::BinaryOp(_, BinOp::And, _, _) => {
                    // Correct: && binds looser than >
                }
                other => panic!("Expected BinaryOp(And), got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_import() {
    match first_stmt(">>> std/io => @(readFile, writeFile)") {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "std/io");
            assert_eq!(imp.symbols.len(), 2);
            assert_eq!(imp.symbols[0].name, "readFile");
            assert_eq!(imp.symbols[1].name, "writeFile");
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_export() {
    match first_stmt("<<< @(add, subtract)") {
        Statement::Export(exp) => {
            assert_eq!(exp.symbols, vec!["add", "subtract"]);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_inheritance() {
    // (E30 Sub-step 2.1) ClassLikeDef + ClassLikeKind::Inheritance
    match first_stmt("Person => Employee = @(department: Str)") {
        Statement::ClassLikeDef(inh) if inh.is_inheritance() => {
            assert_eq!(inh.parent(), Some("Person"));
            assert!(inh.parent_args().is_none());
            assert_eq!(inh.name, "Employee"); // 旧 inh.child
            assert!(inh.name_args.is_none());
            assert_eq!(inh.fields.len(), 1);
            assert_eq!(inh.fields[0].name, "department");
        }
        other => panic!("Expected InheritanceDef, got {:?}", other),
    }
}

#[test]
fn test_parse_generic_inheritance_headers() {
    // (E30 Sub-step 2.1) ClassLikeDef + ClassLikeKind::Inheritance
    match first_stmt("Parent[T] => Child[T, U <= :T] = @(value: T)") {
        Statement::ClassLikeDef(inh) if inh.is_inheritance() => {
            assert_eq!(inh.parent(), Some("Parent"));
            assert_eq!(inh.name, "Child");
            assert!(matches!(
                inh.parent_args().and_then(|args| args.first()),
                Some(MoldHeaderArg::TypeParam(TypeParam { name, constraint: None })) if name == "T"
            ));
            assert_eq!(inh.name_args.as_ref().map(Vec::len), Some(2));
            assert!(matches!(
                inh.name_args.as_ref().and_then(|args| args.get(1)),
                Some(MoldHeaderArg::TypeParam(TypeParam { name, constraint: Some(TypeExpr::Named(bound)) }))
                    if name == "U" && bound == "T"
            ));
            assert_eq!(inh.fields.len(), 1);
            assert_eq!(inh.fields[0].name, "value");
        }
        other => panic!("Expected InheritanceDef, got {:?}", other),
    }
}

#[test]
fn test_parse_mold_def() {
    // (E30 Sub-step 2.1) ClassLikeDef + ClassLikeKind::Mold
    match first_stmt("Mold[T] => Optional[T] = @(hasValue: Bool)") {
        Statement::ClassLikeDef(md) if md.is_mold() => {
            assert_eq!(md.name, "Optional");
            let mold_args = md.mold_args().unwrap();
            assert_eq!(mold_args.len(), 1);
            assert_eq!(md.name_args.as_ref(), Some(mold_args));
            assert_eq!(md.type_params.len(), 1);
            assert_eq!(md.type_params[0].name, "T");
            assert_eq!(md.fields.len(), 1);
            assert_eq!(md.fields[0].name, "hasValue");
        }
        other => panic!("Expected MoldDef, got {:?}", other),
    }
}

#[test]
fn test_parse_mold_def_with_concrete_and_constrained_header_args() {
    // (E30 Sub-step 2.1) ClassLikeDef + ClassLikeKind::Mold
    match first_stmt("Mold[:Int, T <= :Int] => IntBox[:Int, T <= :Int] = @(count: Int)") {
        Statement::ClassLikeDef(md) if md.is_mold() => {
            assert_eq!(md.name, "IntBox");
            let mold_args = md.mold_args().unwrap();
            assert_eq!(mold_args.len(), 2);
            assert_eq!(md.name_args.as_ref(), Some(mold_args));
            assert_eq!(md.type_params.len(), 1);
            assert_eq!(md.type_params[0].name, "T");
            assert_eq!(
                md.type_params[0].constraint,
                Some(TypeExpr::Named("Int".to_string()))
            );
            assert!(matches!(
                &mold_args[0],
                MoldHeaderArg::Concrete(TypeExpr::Named(name)) if name == "Int"
            ));
            assert!(matches!(
                &mold_args[1],
                MoldHeaderArg::TypeParam(TypeParam { name, constraint: Some(TypeExpr::Named(bound)) })
                    if name == "T" && bound == "Int"
            ));
        }
        other => panic!("Expected MoldDef, got {:?}", other),
    }
}

#[test]
fn test_parse_mold_def_with_implicit_name_header() {
    // (E30 Sub-step 2.1) ClassLikeDef + ClassLikeKind::Mold
    match first_stmt("Mold[:Int] => IntBox = @()") {
        Statement::ClassLikeDef(md) if md.is_mold() => {
            assert_eq!(md.name, "IntBox");
            let mold_args = md.mold_args().unwrap();
            assert_eq!(mold_args.len(), 1);
            assert!(md.name_args.is_none());
            assert!(md.type_params.is_empty());
            assert!(matches!(
                &mold_args[0],
                MoldHeaderArg::Concrete(TypeExpr::Named(name)) if name == "Int"
            ));
        }
        other => panic!("Expected MoldDef, got {:?}", other),
    }
}

#[test]
fn test_parse_unmold_forward() {
    match first_stmt("opt ]=> value") {
        Statement::UnmoldForward(uf) => {
            assert_eq!(uf.target, "value");
            match &uf.source {
                Expr::Ident(name, _) => assert_eq!(name, "opt"),
                other => panic!("Expected Ident, got {:?}", other),
            }
        }
        other => panic!("Expected UnmoldForward, got {:?}", other),
    }
}

#[test]
fn test_parse_function_def() {
    let source = "add x: Int y: Int =\n  x + y\n=> :Int";
    match first_stmt(source) {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "add");
            assert!(fd.type_params.is_empty());
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].name, "x");
            assert_eq!(fd.params[1].name, "y");
            assert!(fd.return_type.is_some());
            match &fd.return_type {
                Some(TypeExpr::Named(n)) => assert_eq!(n, "Int"),
                other => panic!("Expected Named(Int), got {:?}", other),
            }
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_parse_generic_function_def() {
    let source = "id[T <= :Int] x: T =\n  x\n=> :T";
    match first_stmt(source) {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "id");
            assert_eq!(fd.type_params.len(), 1);
            assert_eq!(fd.type_params[0].name, "T");
            assert_eq!(
                fd.type_params[0].constraint,
                Some(TypeExpr::Named("Int".to_string()))
            );
            assert_eq!(fd.params.len(), 1);
            assert_eq!(fd.params[0].name, "x");
            assert_eq!(
                fd.params[0].type_annotation,
                Some(TypeExpr::Named("T".to_string()))
            );
            assert_eq!(fd.return_type, Some(TypeExpr::Named("T".to_string())));
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_parse_function_def_param_default_value() {
    let source = "greet name: Str prefix: Str <= \"Hello\" =\n  prefix + name\n=> :Str";
    match first_stmt(source) {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "greet");
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].name, "name");
            assert_eq!(fd.params[1].name, "prefix");
            assert!(fd.params[0].default_value.is_none());
            match &fd.params[1].default_value {
                Some(Expr::StringLit(value, _)) => assert_eq!(value, "Hello"),
                other => panic!("Expected string default value, got {:?}", other),
            }
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_parse_lambda_param_default_value_is_rejected() {
    let (_program, errors) = parse("f <= _ x <= 1 = x");
    assert!(
        !errors.is_empty(),
        "Expected parse error for lambda default value syntax"
    );
}

#[test]
fn test_parse_stub_mold_with_message_literal() {
    match first_stmt("stub <= Stub[\"User API placeholder\"]") {
        Statement::Assignment(a) => match &a.value {
            Expr::MoldInst(name, type_args, fields, _) => {
                assert_eq!(name, "Stub");
                assert!(fields.is_empty(), "Stub should not require `()` fields");
                assert_eq!(type_args.len(), 1);
                match &type_args[0] {
                    Expr::StringLit(msg, _) => assert_eq!(msg, "User API placeholder"),
                    other => panic!("Expected Stub message StringLit, got {:?}", other),
                }
            }
            other => panic!("Expected Stub MoldInst, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_todo_mold_with_stub_type_arg_and_fields() {
    let source = r#"
todoUser <= TODO[Stub["User API placeholder"]](
  id <= "task-42",
  task <= "Implement user fetch",
  sol <= 1,
  unm <= 0
)
"#;
    match first_stmt(source) {
        Statement::Assignment(a) => match &a.value {
            Expr::MoldInst(name, type_args, fields, _) => {
                assert_eq!(name, "TODO");
                assert_eq!(type_args.len(), 1);
                match &type_args[0] {
                    Expr::MoldInst(inner_name, inner_args, inner_fields, _) => {
                        assert_eq!(inner_name, "Stub");
                        assert!(inner_fields.is_empty());
                        assert_eq!(inner_args.len(), 1);
                        assert!(matches!(inner_args[0], Expr::StringLit(_, _)));
                    }
                    other => panic!("Expected nested Stub MoldInst, got {:?}", other),
                }
                let field_names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                assert_eq!(field_names, vec!["id", "task", "sol", "unm"]);
            }
            other => panic!("Expected TODO MoldInst, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_method_call() {
    match first_stmt("result <= \"hello\".toUpperCase()") {
        Statement::Assignment(a) => match &a.value {
            Expr::MethodCall(obj, method, args, _) => {
                assert!(matches!(obj.as_ref(), Expr::StringLit(s, _) if s == "hello"));
                assert_eq!(method, "toUpperCase");
                assert!(args.is_empty());
            }
            other => panic!("Expected MethodCall, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_field_access() {
    match first_stmt("name <= user.name") {
        Statement::Assignment(a) => match &a.value {
            Expr::FieldAccess(obj, field, _) => {
                assert!(matches!(obj.as_ref(), Expr::Ident(n, _) if n == "user"));
                assert_eq!(field, "name");
            }
            other => panic!("Expected FieldAccess, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_condition_branch() {
    // C20-1 (ROOT-5): a multi-line multi-arm guard on the `<=` rhs is
    // now rejected with [E0303]. The canonical way to express the
    // same semantics is to wrap the branch in parentheses (which
    // resets the parser's branch context to `TopLevel`) or to use a
    // single-line form / helper function. Here we exercise the
    // parenthesised escape hatch so the historical assertion —
    // "a multi-arm conditional binds to the Assignment's value" —
    // still holds.
    let source = "grade <= (\n  | score >= 90 |> \"A\"\n  | _ |> \"F\"\n)";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(!program.statements.is_empty());
}

#[test]
fn test_parse_condition_branch_rhs_multiline_rejected() {
    // C20-1 (ROOT-5 / C19B-009): bare multi-line rhs guard is rejected.
    // This is the inverse of `test_parse_condition_branch` and pins
    // the silent-bug禁圧 guardrail.
    let source = "grade <=\n  | score >= 90 |> \"A\"\n  | _ |> \"F\"";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("[E0303]")),
        "Expected [E0303] for multi-line rhs multi-arm guard, got: {:?}",
        errors
    );
}

#[test]
fn test_parse_condition_branch_rhs_single_line_accepted() {
    // Single-line form stays legal because `|` tokens are unambiguous
    // when they all live on the same physical line.
    let source = "grade <= | 90 >= 80 |> \"A\" | _ |> \"F\"";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert!(!program.statements.is_empty());
}

#[test]
fn test_parse_error_ceiling() {
    let source = "|== error: Error =\n  0\n=> :Int";
    match first_stmt(source) {
        Statement::ErrorCeiling(ec) => {
            assert_eq!(ec.error_param, "error");
            match &ec.error_type {
                TypeExpr::Named(n) => assert_eq!(n, "Error"),
                other => panic!("Expected Named(Error), got {:?}", other),
            }
        }
        other => panic!("Expected ErrorCeiling, got {:?}", other),
    }
}

#[test]
fn test_parse_multiline_program() {
    let source = r#"Person = @(name: Str, age: Int)
alice <= Person(name <= "Alice", age <= 30)
<<< @(Person)
"#;
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(program.statements.len(), 3);
}

#[test]
fn test_parse_unary_negation() {
    match first_stmt("result <= -10") {
        Statement::Assignment(a) => match &a.value {
            Expr::UnaryOp(UnaryOp::Neg, inner, _) => {
                assert!(matches!(inner.as_ref(), Expr::IntLit(10, _)));
            }
            other => panic!("Expected UnaryOp(Neg), got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_boolean_expression() {
    match first_stmt("result <= !flag") {
        Statement::Assignment(a) => match &a.value {
            Expr::UnaryOp(UnaryOp::Not, _, _) => {}
            other => panic!("Expected UnaryOp(Not), got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_unmold_backward() {
    match first_stmt("value <=[ opt") {
        Statement::UnmoldBackward(ub) => {
            assert_eq!(ub.target, "value");
            assert!(matches!(ub.source, Expr::Ident(ref name, _) if name == "opt"));
        }
        other => panic!("Expected UnmoldBackward, got {:?}", other),
    }
}

#[test]
fn test_parse_unmold_backward_complex_expr() {
    match first_stmt("doubled <=[ Map[numbers, _ x = x * 2]()") {
        Statement::UnmoldBackward(ub) => {
            assert_eq!(ub.target, "doubled");
        }
        other => panic!("Expected UnmoldBackward, got {:?}", other),
    }
}

#[test]
fn test_parse_noarg_func_def() {
    let source = "getVersion =\n  \"1.0.0\"\n=> :Str";
    match first_stmt(source) {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "getVersion");
            assert!(fd.params.is_empty());
            assert!(fd.return_type.is_some());
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_parse_noarg_func_def_multiline() {
    let source = "getConfig =\n  host <= \"localhost\"\n  host\n=> :Str";
    match first_stmt(source) {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "getConfig");
            assert!(fd.params.is_empty());
            assert_eq!(fd.body.len(), 2);
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

// ── Pipeline parsing ──

#[test]
fn test_parse_pipeline_simple() {
    // 5 => add(3, _) => result should parse as Assignment with Pipeline value
    let source = "5 => add(3, _) => result";
    match first_stmt(source) {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "result");
            // The value is a Pipeline with two steps: [IntLit(5), FuncCall(add, [3, _])]
            // OR it could be a single-step pipeline that gets unwrapped
            match &a.value {
                Expr::Pipeline(steps, _) => {
                    assert_eq!(steps.len(), 2);
                }
                Expr::FuncCall(_, _, _) => {
                    // Single step pipeline gets unwrapped
                }
                other => panic!("Expected Pipeline or FuncCall, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_pipeline_chain() {
    // Multi-step pipeline ending with assignment
    let source = "10 => add(5, _) => multiply(_, 3) => result";
    match first_stmt(source) {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "result");
            match &a.value {
                Expr::Pipeline(steps, _) => {
                    assert_eq!(steps.len(), 3); // 10, add(5,_), multiply(_,3)
                }
                other => panic!("Expected Pipeline, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_parse_pipeline_ident_start() {
    // Pipeline starting with an identifier
    let source = "data => process => result";
    match first_stmt(source) {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "result");
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

// ── Single-direction constraint ──

#[test]
fn test_single_direction_constraint_violation_arrow() {
    // => and <= mixed in same statement should be a parse error (E0301)
    let source = "data => filter(_) <= result";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected error for direction constraint violation, got none"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("E0301")),
        "Expected E0301 error, got: {:?}",
        errors
    );
}

#[test]
fn test_single_direction_constraint_violation_unmold() {
    // ]=> and <=[ mixed should be a parse error (E0302)
    let source = "mold ]=> x <=[ other";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected error for unmold direction constraint violation"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("E0302")),
        "Expected E0302 error, got: {:?}",
        errors
    );
}

#[test]
fn test_single_direction_ok_different_categories() {
    // => and <=[ in same statement is allowed (different categories)
    // Verify that => alone and <= alone parse fine
    let source = "x <= 42";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Simple assignment should parse: {:?}",
        errors
    );
}

#[test]
fn test_single_direction_assignment_then_pipeline_violation() {
    // result <= add(x, 10) => something should be E0301
    let source = "result <= add(10, 5) => x";
    let (_, errors) = parse(source);
    assert!(!errors.is_empty(), "Expected E0301 error");
    assert!(
        errors.iter().any(|e| e.message.contains("E0301")),
        "Expected E0301 error, got: {:?}",
        errors
    );
}

// ── BT-3: Single direction constraint nested tests ──

#[test]
fn test_bt3_direction_violation_in_mold_args() {
    // Concat[@[1], @[2]] => result then <= — E0301
    let source = r#"x <= Concat[@[1], @[2]] => result"#;
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected E0301 error for direction violation in mold+assignment"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("E0301")),
        "Expected E0301, got: {:?}",
        errors
    );
}

#[test]
fn test_bt3_direction_violation_multiple_in_one_statement() {
    // Multiple direction violations in a single statement
    let source = "a => b <= c => d";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected error(s) for multiple direction violations"
    );
    // Should contain at least one E0301
    assert!(
        errors.iter().any(|e| e.message.contains("E0301")),
        "Expected E0301, got: {:?}",
        errors
    );
}

#[test]
fn test_bt3_direction_ok_separate_statements() {
    // Separate statements with different directions should be fine
    let source = "x => y\na <= b";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Separate statements with different directions should parse: {:?}",
        errors
    );
}

#[test]
fn test_bt3_unmold_direction_violation_nested() {
    // Nested unmold direction violation: ]=> and <=[ in same statement
    let source = "a ]=> x <=[ b";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected E0302 error for unmold direction violation"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("E0302")),
        "Expected E0302, got: {:?}",
        errors
    );
}

#[test]
fn test_bt3_direction_ok_arrow_and_unmold_mix() {
    // => and <=[ in same statement is allowed (different categories)
    // This tests that the two constraint categories are independent
    let source = "x <= 42";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Single direction should parse fine: {:?}",
        errors
    );
}

// ── Method definition parsing ──

#[test]
fn test_parse_method_no_args() {
    let source = "Greeter = @(\n  name: Str\n  greet =\n    name\n  => :Str\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.name, "Greeter");
            assert_eq!(td.fields.len(), 2);
            assert_eq!(td.fields[0].name, "name");
            assert!(!td.fields[0].is_method);
            assert_eq!(td.fields[1].name, "greet");
            assert!(td.fields[1].is_method);
            let md = td.fields[1].method_def.as_ref().unwrap();
            assert_eq!(md.name, "greet");
            assert!(md.params.is_empty());
            assert!(md.return_type.is_some());
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

#[test]
fn test_parse_method_with_args() {
    let source = "Calc = @(\n  base: Int\n  add x: Int =\n    base + x\n  => :Int\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.fields.len(), 2);
            let method = &td.fields[1];
            assert!(method.is_method);
            let md = method.method_def.as_ref().unwrap();
            assert_eq!(md.name, "add");
            assert_eq!(md.params.len(), 1);
            assert_eq!(md.params[0].name, "x");
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

#[test]
fn test_parse_method_mixed_fields() {
    let source = "Thing = @(\n  a: Int\n  b: Str\n  compute =\n    a\n  => :Int\n  label =\n    b\n  => :Str\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.fields.len(), 4);
            assert!(!td.fields[0].is_method); // a
            assert!(!td.fields[1].is_method); // b
            assert!(td.fields[2].is_method); // compute
            assert!(td.fields[3].is_method); // label
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

// ── Versioned import/export tests (gen.num format) ──

#[test]
fn test_parse_version_gen_num() {
    let stmt = first_stmt(">>> alice/http@b.12 => @(Http)");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "alice/http");
            assert_eq!(imp.version, Some("b.12".to_string()));
            assert_eq!(imp.symbols.len(), 1);
            assert_eq!(imp.symbols[0].name, "Http");
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_version_gen_only() {
    let stmt = first_stmt(">>> alice/http@b => @(Http)");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "alice/http");
            assert_eq!(imp.version, Some("b".to_string()));
            assert_eq!(imp.symbols.len(), 1);
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_version_multi_letter_gen() {
    let stmt = first_stmt(">>> org/pkg@aa.1");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "org/pkg");
            assert_eq!(imp.version, Some("aa.1".to_string()));
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_gen_num() {
    let stmt = first_stmt("<<<@a.3 @(MyApp, Config)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("a.3".to_string()));
            assert_eq!(exp.symbols, vec!["MyApp", "Config"]);
            assert_eq!(exp.path, None);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_gen_only() {
    let stmt = first_stmt("<<<@b @(X)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("b".to_string()));
            assert_eq!(exp.symbols, vec!["X"]);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

// ── Version label tests ──

#[test]
fn test_parse_version_with_label() {
    let stmt = first_stmt(">>> org/pkg@a.1.alpha");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "org/pkg");
            assert_eq!(imp.version, Some("a.1.alpha".to_string()));
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_version_with_hyphenated_label() {
    let stmt = first_stmt(">>> org/pkg@x.34.gen-2-stable");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "org/pkg");
            assert_eq!(imp.version, Some("x.34.gen-2-stable".to_string()));
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_version_label_with_symbols() {
    let stmt = first_stmt(">>> alice/http@a.5.beta => @(get, post)");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "alice/http");
            assert_eq!(imp.version, Some("a.5.beta".to_string()));
            assert_eq!(imp.symbols.len(), 2);
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_with_label() {
    let stmt = first_stmt("<<<@a.1.rc @(MyApp)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("a.1.rc".to_string()));
            assert_eq!(exp.symbols, vec!["MyApp"]);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

// ── Legacy SemVer tests (backward compat for core-bundled) ──

#[test]
fn test_parse_versioned_import_legacy_semver() {
    let stmt = first_stmt(">>> taida-lang/string-utils@1.0.0");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "taida-lang/string-utils");
            assert_eq!(imp.version, Some("1.0.0".to_string()));
            assert!(imp.symbols.is_empty());
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_versioned_import_with_symbols_legacy() {
    let stmt = first_stmt(">>> taida-community/http@2.1.0 => @(get, post)");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "taida-community/http");
            assert_eq!(imp.version, Some("2.1.0".to_string()));
            assert_eq!(imp.symbols.len(), 2);
            assert_eq!(imp.symbols[0].name, "get");
            assert_eq!(imp.symbols[1].name, "post");
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_local_import_no_version() {
    let stmt = first_stmt(">>> ./main.td => @(func)");
    match stmt {
        Statement::Import(imp) => {
            assert_eq!(imp.path, "./main.td");
            assert_eq!(imp.version, None);
            assert_eq!(imp.symbols.len(), 1);
            assert_eq!(imp.symbols[0].name, "func");
        }
        other => panic!("Expected Import, got {:?}", other),
    }
}

#[test]
fn test_parse_versioned_export_legacy() {
    let stmt = first_stmt("<<<@1.0.0 @(capitalize, truncate)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("1.0.0".to_string()));
            assert_eq!(exp.symbols, vec!["capitalize", "truncate"]);
            assert_eq!(exp.path, None);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_without_version() {
    let stmt = first_stmt("<<< @(func, helper)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, None);
            assert_eq!(exp.symbols, vec!["func", "helper"]);
            assert_eq!(exp.path, None);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_single_symbol() {
    let stmt = first_stmt("<<< myFunc");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, None);
            assert_eq!(exp.symbols, vec!["myFunc"]);
            assert_eq!(exp.path, None);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

// ── B11-1e: Package identity in export ──

#[test]
fn test_parse_export_version_with_package_identity() {
    // <<<@b.11.rc3 taida-lang/terminal
    let stmt = first_stmt("<<<@b.11.rc3 taida-lang/terminal");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("b.11.rc3".to_string()));
            assert_eq!(exp.path, Some("taida-lang/terminal".to_string()));
            assert!(exp.symbols.is_empty());
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_with_package_identity_and_symbols_accepted() {
    // B11-10b: `<<<@version owner/name @(symbols)` is the Phase 10 canonical surface.
    let stmt = first_stmt("<<<@b.11.rc3 taida-lang/terminal @(capitalize, truncate)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("b.11.rc3".to_string()));
            assert_eq!(exp.path, Some("taida-lang/terminal".to_string()));
            assert_eq!(exp.symbols, vec!["capitalize", "truncate"]);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_with_simple_org_name() {
    // <<<@a.3 shijimic/my-pkg
    let stmt = first_stmt("<<<@a.3 shijimic/my-pkg");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("a.3".to_string()));
            assert_eq!(exp.path, Some("shijimic/my-pkg".to_string()));
            assert!(exp.symbols.is_empty());
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_only_no_package_identity() {
    // <<<@a.3 @(MyApp) — existing format, no package identity
    let stmt = first_stmt("<<<@a.3 @(MyApp)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("a.3".to_string()));
            assert_eq!(exp.path, None);
            assert_eq!(exp.symbols, vec!["MyApp"]);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

#[test]
fn test_parse_export_version_only_bare() {
    // <<<@a.3 — version only, no symbols, no package identity
    let stmt = first_stmt("<<<@a.3");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("a.3".to_string()));
            assert_eq!(exp.path, None);
            assert!(exp.symbols.is_empty());
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

// ── B11-10b: Phase 10 canonical surface (no arrow) ──

#[test]
fn test_parse_export_arrow_surface_rejected() {
    // B11-10b: `<<<@version owner/name => @(symbols)` is no longer supported.
    let (_program, errors) = parse("<<<@b.11.rc3 taida-lang/terminal => @(open, close)");
    assert!(
        !errors.is_empty(),
        "Expected parse error for arrow surface, but got none"
    );
    assert!(
        errors[0]
            .message
            .contains("arrow syntax after package identity is no longer supported"),
        "Error message should mention obsolete arrow syntax, got: {}",
        errors[0].message
    );
}

#[test]
fn test_parse_export_arrow_surface_single_symbol_rejected() {
    // B11-10b: single-symbol arrow surface is also rejected.
    let (_program, errors) = parse("<<<@a.3 shijimic/my-pkg => @(hello)");
    assert!(
        !errors.is_empty(),
        "Expected parse error for arrow surface, but got none"
    );
    assert!(
        errors[0]
            .message
            .contains("arrow syntax after package identity is no longer supported"),
        "Error message should mention obsolete arrow syntax, got: {}",
        errors[0].message
    );
}

#[test]
fn test_parse_export_package_identity_with_symbols_accepted() {
    // B11-10b: `<<<@version owner/name @(symbols)` is the canonical surface.
    let stmt = first_stmt("<<<@b.11.rc3 taida-lang/terminal @(open, close)");
    match stmt {
        Statement::Export(exp) => {
            assert_eq!(exp.version, Some("b.11.rc3".to_string()));
            assert_eq!(exp.path, Some("taida-lang/terminal".to_string()));
            assert_eq!(exp.symbols, vec!["open", "close"]);
        }
        other => panic!("Expected Export, got {:?}", other),
    }
}

/// Test helper: split version suffix from path string.
fn split_version_from_path(path: &str) -> (String, Option<String>) {
    if let Some(at_pos) = path.rfind('@') {
        let after = &path[at_pos + 1..];
        let is_version = after.starts_with(|c: char| c.is_ascii_digit() || c.is_ascii_lowercase());
        if is_version && !after.is_empty() {
            let base = path[..at_pos].to_string();
            let version = after.to_string();
            return (base, Some(version));
        }
    }
    (path.to_string(), None)
}

#[test]
fn test_split_version_from_path() {
    let (p, v) = split_version_from_path("taida-lang/string-utils@1.0.0");
    assert_eq!(p, "taida-lang/string-utils");
    assert_eq!(v, Some("1.0.0".to_string()));

    let (p, v) = split_version_from_path("./main.td");
    assert_eq!(p, "./main.td");
    assert_eq!(v, None);

    let (p, v) = split_version_from_path("pkg@0.2.3");
    assert_eq!(p, "pkg");
    assert_eq!(v, Some("0.2.3".to_string()));

    let (p, v) = split_version_from_path("alice/http@b.12");
    assert_eq!(p, "alice/http");
    assert_eq!(v, Some("b.12".to_string()));

    let (p, v) = split_version_from_path("alice/http@b");
    assert_eq!(p, "alice/http");
    assert_eq!(v, Some("b".to_string()));
}

// ── H-1: Line continuation tests ──────────────────────────

#[test]
fn test_line_continuation_basic() {
    // Backslash at end of line joins with next line
    let source = "x <= 1 + \\\n    2";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "x");
            // Should parse as x <= 1 + 2 (binary add)
            match &a.value {
                Expr::BinaryOp(_, op, _, _) => assert_eq!(*op, BinOp::Add),
                other => panic!("Expected BinaryOp, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_line_continuation_multiple() {
    // Multiple line continuations
    let source = "y <= 1 + \\\n    2 + \\\n    3";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_line_continuation_in_pipeline() {
    // Line continuation in pipeline
    let source = "data => \\\n    Map[_, _ x = x + 1]() => \\\n    result";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_no_continuation_without_backslash() {
    // Without backslash, lines are separate statements
    let source = "x <= 1\ny <= 2";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 2);
}

// ── H-1b: Indentation / nesting abnormal cases ────────────

#[test]
fn test_parse_tab_indentation_in_nested_block_reports_error() {
    let source = "add x y =\n  x + y\n\tz <= 1\n=> :Int";
    let (_, errors) = parse(source);
    assert!(!errors.is_empty(), "Expected tab indentation error");
    assert!(
        errors.iter().any(|e| e.message.contains("Tab")),
        "Expected tab-related parse/lex error, got: {:?}",
        errors
    );
}

#[test]
fn test_parse_nested_list_missing_closing_bracket_error() {
    let source = "x <= @[1, @[2, 3]\nstdout(x)";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected parse error for nested list delimiter mismatch"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Expected RBracket")),
        "Expected RBracket parse error, got: {:?}",
        errors
    );
}

#[test]
fn test_parse_nested_pack_missing_closing_paren_error() {
    let source = "x <= @(a <= @(b <= 1)\nstdout(x)";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected parse error for nested pack delimiter mismatch"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("Expected RParen")),
        "Expected RParen parse error, got: {:?}",
        errors
    );
}

#[test]
fn test_parse_cond_branch_malformed_nested_arm_error() {
    // C20-1 (ROOT-5): the original source placed the multi-line
    // CondBranch on a `<=` rhs, which now trips [E0303] *before* the
    // malformed `|>` is ever examined. Keep the "Expected PipeGt"
    // assertion by moving the branch into a top-level / function-body
    // context (where multi-line arms remain legal).
    let source = "classify score =\n  | score >= 90 |> \"A\"\n  | _ > \"F\"\n=> :Str";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected parse error for malformed condition-branch arm"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("Expected PipeGt")),
        "Expected PipeGt parse error, got: {:?}",
        errors
    );
}

#[test]
fn test_parse_vertical_tab_control_char_reports_lex_error() {
    let source = "x <= 1\u{000b}y <= 2";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected parse/lex error for vertical-tab control char"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Unexpected character")),
        "Expected unexpected-character error, got: {:?}",
        errors
    );
}

#[test]
fn test_parse_form_feed_control_char_reports_lex_error() {
    let source = "x <= 1\u{000c}y <= 2";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected parse/lex error for form-feed control char"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Unexpected character")),
        "Expected unexpected-character error, got: {:?}",
        errors
    );
}

// ── H-2: Function type signature tests ────────────────────

#[test]
fn test_function_type_single_param() {
    // :Int => :Str -- function taking Int returning Str
    // Taida typed assignment: `name: Type <= value`
    let source = "transform: Int => :Str <= _ x = x.toString()";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "transform");
            match &a.type_annotation {
                Some(TypeExpr::Function(params, ret)) => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0], TypeExpr::Named("Int".to_string()));
                    assert_eq!(**ret, TypeExpr::Named("Str".to_string()));
                }
                other => panic!("Expected Function type annotation, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_function_type_no_arg() {
    // _ => :T -- no-argument function type
    let source = "factory: _ => :Int <= _ = 42";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "factory");
            match &a.type_annotation {
                Some(TypeExpr::Function(params, ret)) => {
                    assert_eq!(params.len(), 0); // _ means no params
                    assert_eq!(**ret, TypeExpr::Named("Int".to_string()));
                }
                other => panic!("Expected Function type annotation, got {:?}", other),
            }
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_placeholder_type_in_generic() {
    // Result[T, _] -- placeholder used in generic type args
    let source = "val: Result[Int, _] <= Result[42, _ = true]()";
    let program = parse_ok(source);
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Assignment(a) => match &a.type_annotation {
            Some(TypeExpr::Generic(name, args)) => {
                assert_eq!(name, "Result");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0], TypeExpr::Named("Int".to_string()));
                assert_eq!(args[1], TypeExpr::Named("_".to_string()));
            }
            other => panic!("Expected Generic type annotation, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

// ── Doc Comment AST Tests ──────────────────────────────────

#[test]
fn test_doc_comment_on_func_def() {
    let source = "///@ Purpose: adds two numbers\nadd x: Int y: Int =\n  x + y\n=> :Int";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "add");
            assert_eq!(fd.doc_comments, vec!["Purpose: adds two numbers"]);
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_multiline_on_func_def() {
    let source = "///@ Purpose: adds two numbers\n///@ Returns: the sum\nadd x: Int y: Int =\n  x + y\n=> :Int";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "add");
            assert_eq!(
                fd.doc_comments,
                vec!["Purpose: adds two numbers", "Returns: the sum",]
            );
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_on_type_def() {
    let source = "///@ Purpose: represents a pilot\nPilot = @(\n  name: Str\n  age: Int\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.name, "Pilot");
            assert_eq!(td.doc_comments, vec!["Purpose: represents a pilot"]);
            assert_eq!(td.fields.len(), 2);
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_on_type_def_fields() {
    let source =
        "Pilot = @(\n  ///@ The pilot's name\n  name: Str\n  ///@ The pilot's age\n  age: Int\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.name, "Pilot");
            assert_eq!(td.fields.len(), 2);
            assert_eq!(td.fields[0].name, "name");
            assert_eq!(td.fields[0].doc_comments, vec!["The pilot's name"]);
            assert_eq!(td.fields[1].name, "age");
            assert_eq!(td.fields[1].doc_comments, vec!["The pilot's age"]);
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_on_mold_def() {
    let source =
        "///@ Purpose: wraps async result\nMold[T] => ApiResult[T] = @(\n  success: Bool\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(md) if md.is_mold() => {
            assert_eq!(md.name, "ApiResult");
            assert_eq!(md.doc_comments, vec!["Purpose: wraps async result"]);
        }
        other => panic!("Expected MoldDef, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_on_inheritance_def() {
    let source = "///@ Purpose: employee inherits from person\nPerson => Employee = @(\n  department: Str\n)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(inh) if inh.is_inheritance() => {
            assert_eq!(inh.parent(), Some("Person"));
            assert_eq!(inh.name, "Employee");
            assert_eq!(
                inh.doc_comments,
                vec!["Purpose: employee inherits from person"]
            );
        }
        other => panic!("Expected InheritanceDef, got {:?}", other),
    }
}

#[test]
fn test_no_doc_comment_results_in_empty_vec() {
    let source = "x <= 42";
    let program = parse_ok(source);
    // Assignments don't carry doc comments; just verify parsing works
    match &program.statements[0] {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "x");
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_with_ai_tags() {
    let source = "///@ Purpose: search pilots\n///@ AI-Context: used in admin panel\n///@ AI-Category: pilot-management, search\nsearchPilots query: Str =\n  query\n=> :Str";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::FuncDef(fd) => {
            assert_eq!(fd.name, "searchPilots");
            assert_eq!(fd.doc_comments.len(), 3);
            assert_eq!(fd.doc_comments[0], "Purpose: search pilots");
            assert_eq!(fd.doc_comments[1], "AI-Context: used in admin panel");
            assert_eq!(fd.doc_comments[2], "AI-Category: pilot-management, search");
        }
        other => panic!("Expected FuncDef, got {:?}", other),
    }
}

#[test]
fn test_doc_comment_empty_line() {
    // ///@  with just trailing whitespace should produce empty string
    let source = "///@\n///@ Purpose: test\nPilot = @(name: Str)";
    let program = parse_ok(source);
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.name, "Pilot");
            assert_eq!(td.doc_comments.len(), 2);
            assert_eq!(td.doc_comments[0], ""); // empty doc comment line
            assert_eq!(td.doc_comments[1], "Purpose: test");
        }
        other => panic!("Expected TypeDef, got {:?}", other),
    }
}

// ── C-1d: Method call / pipeline 経由 call でも空スロットが機能する ──

#[test]
fn test_method_call_with_hole() {
    // C-1d: `obj.method(5, )` should produce a Hole in the args list
    let source = "result <= list.slice(1, )";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    match &program.statements[0] {
        Statement::Assignment(a) => match &a.value {
            Expr::MethodCall(_obj, method, args, _) => {
                assert_eq!(method, "slice");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0], Expr::IntLit(1, _)));
                assert!(
                    matches!(args[1], Expr::Hole(_)),
                    "Expected Hole, got {:?}",
                    args[1]
                );
            }
            other => panic!("Expected MethodCall, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_pipeline_with_placeholder() {
    // C-1d: `data => func(5, _)` should parse correctly with Placeholder in pipeline
    let source = r#"5 => add(_, 3) => result"#;
    let (_, errors) = parse(source);
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
}

#[test]
fn test_pipeline_with_hole() {
    // C-1d: `data => func(5, )` should parse correctly with Hole in pipeline
    let source = r#"5 => add(, 3) => result"#;
    let (_, errors) = parse(source);
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
}

// ── C-3a: 空白区切り関数呼び出し `f x` は誤パースされない ──

#[test]
fn test_whitespace_call_not_parsed_as_funccall() {
    // C-3a: `z <= g 5` should NOT parse as `z <= g(5)`.
    // Instead, `z <= g` (assignment) and `5` (standalone expression).
    let source = "z <= g 5";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    // First statement: assignment `z <= g`
    match &program.statements[0] {
        Statement::Assignment(a) => {
            assert_eq!(a.target, "z");
            assert!(matches!(&a.value, Expr::Ident(name, _) if name == "g"));
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
    // Second statement: standalone expression `5`
    assert!(
        program.statements.len() >= 2,
        "Expected 2 statements, got {}",
        program.statements.len()
    );
    match &program.statements[1] {
        Statement::Expr(Expr::IntLit(5, _)) => {}
        other => panic!("Expected Expr(IntLit(5)), got {:?}", other),
    }
}

#[test]
fn test_standalone_f_x_is_func_def_attempt() {
    // C-3a: Bare `f x` at statement level is parsed as a function definition attempt.
    // Without `= body`, the parser should try function def parsing and may succeed or fail
    // but NOT parse as function call.
    let source = "f x = x + 1\n=> :Int";
    let (_, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Valid function def should parse: {:?}",
        errors
    );
}

// ── C-3b: `f(1)` と `f(1, )` が別 AST ──

#[test]
fn test_f1_vs_f1_comma_different_ast() {
    // C-3b: `f(1)` has 1 arg (IntLit), `f(1, )` has 2 args (IntLit, Hole)
    let source_normal = "result <= f(1)";
    let source_partial = "result <= f(1, )";

    let (prog_normal, err1) = parse(source_normal);
    let (prog_partial, err2) = parse(source_partial);
    assert!(err1.is_empty(), "Parse errors: {:?}", err1);
    assert!(err2.is_empty(), "Parse errors: {:?}", err2);

    // Normal call: 1 arg
    let args_normal = match &prog_normal.statements[0] {
        Statement::Assignment(a) => match &a.value {
            Expr::FuncCall(_, args, _) => args,
            other => panic!("Expected FuncCall, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    };
    assert_eq!(args_normal.len(), 1);
    assert!(matches!(args_normal[0], Expr::IntLit(1, _)));

    // Partial application call: 2 args (IntLit, Hole)
    let args_partial = match &prog_partial.statements[0] {
        Statement::Assignment(a) => match &a.value {
            Expr::FuncCall(_, args, _) => args,
            other => panic!("Expected FuncCall, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    };
    assert_eq!(args_partial.len(), 2);
    assert!(matches!(args_partial[0], Expr::IntLit(1, _)));
    assert!(
        matches!(args_partial[1], Expr::Hole(_)),
        "Expected Hole, got {:?}",
        args_partial[1]
    );
}

// ── C-3c: docs サンプルの parser-only テスト ──

#[test]
fn test_docs_sample_pipeline_parses() {
    // Pipeline from docs/guide/09_functions.md
    // Note: `<=` and `=>` cannot be mixed in one statement (E0301),
    // so we use `=>` only for pipeline.
    let source = r#""  Hello World  " => Trim[_]() => Lower[_]() => result"#;
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Pipeline sample should parse: {:?}",
        errors
    );
    assert_eq!(
        program.statements.len(),
        1,
        "Pipeline should produce 1 statement"
    );
    // Pipeline `expr => ... => name` parses as Assignment with pipeline value
    assert!(
        matches!(&program.statements[0], Statement::Assignment(a) if a.target == "result"),
        "Expected Assignment to 'result' via pipeline, got: {:?}",
        program.statements[0]
    );
}

#[test]
fn test_docs_sample_buchi_pack_parses() {
    // BuchiPack from docs/guide/04_buchi_pack.md
    let source = "Pilot = @(name: Str, age: Int, role: Str)";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "BuchiPack sample should parse: {:?}",
        errors
    );
    assert_eq!(
        program.statements.len(),
        1,
        "TypeDef should produce 1 statement"
    );
    match &program.statements[0] {
        Statement::ClassLikeDef(td) if td.is_buchi_pack() => {
            assert_eq!(td.name, "Pilot");
            assert_eq!(td.fields.len(), 3, "Pilot should have 3 fields");
        }
        other => panic!("Expected TypeDef, got: {:?}", other),
    }
}

#[test]
fn test_docs_sample_assignment_parses() {
    let source = "x <= 42\nname <= \"Shinji\"";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Assignment sample should parse: {:?}",
        errors
    );
    assert_eq!(
        program.statements.len(),
        2,
        "Should produce 2 assignment statements"
    );
    assert!(matches!(&program.statements[0], Statement::Assignment(a) if a.target == "x"));
    assert!(matches!(&program.statements[1], Statement::Assignment(a) if a.target == "name"));
}

#[test]
fn test_docs_sample_condition_branch_parses() {
    // C20-1 (ROOT-5): the historical docs sample placed a multi-line
    // multi-arm guard on the `<=` rhs. That shape is now rejected
    // with [E0303]; the updated sample uses the parenthesised escape
    // hatch which preserves the same runtime semantics.
    let source =
        "grade <= (\n  | score >= 90 |> \"A\"\n  | score >= 80 |> \"B\"\n  | _ |> \"F\"\n)";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Condition branch sample should parse: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1, "Should produce 1 statement");
    assert!(
        matches!(&program.statements[0], Statement::Assignment(_)),
        "Expected Assignment with condition branch value"
    );
}

#[test]
fn test_docs_sample_error_ceiling_parses() {
    let source = "|== error: Error =\n  0\n=> :Int";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Error ceiling sample should parse: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1, "Should produce 1 statement");
    assert!(
        matches!(&program.statements[0], Statement::ErrorCeiling(_)),
        "Expected ErrorCeiling statement, got: {:?}",
        program.statements[0]
    );
}

#[test]
fn test_docs_sample_mold_instantiation_parses() {
    let source = r#"result <= Div[10, 3]()"#;
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Mold instantiation sample should parse: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1, "Should produce 1 statement");
    assert!(matches!(&program.statements[0], Statement::Assignment(a) if a.target == "result"));
}

#[test]
fn test_docs_sample_json_parses() {
    let source = r#"User = @(name: Str, age: Int)
parsed <= JSON[raw, User]()"#;
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "JSON sample should parse: {:?}", errors);
    assert_eq!(
        program.statements.len(),
        2,
        "Should produce 2 statements (TypeDef + Assignment)"
    );
    assert!(matches!(
        &program.statements[0],
        Statement::ClassLikeDef(cl) if cl.is_buchi_pack()
    ));
    assert!(matches!(&program.statements[1], Statement::Assignment(a) if a.target == "parsed"));
}

#[test]
fn test_docs_sample_lambda_parses() {
    let source = "doubler <= _ x = x + x";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Lambda sample should parse: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1, "Should produce 1 statement");
    assert!(matches!(&program.statements[0], Statement::Assignment(a) if a.target == "doubler"));
}

#[test]
fn test_docs_sample_import_export_parses() {
    let source = ">>> ./utils.td => @(helper)\n<<< @(main)";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Import/export sample should parse: {:?}",
        errors
    );
    assert_eq!(
        program.statements.len(),
        2,
        "Should produce 2 statements (Import + Export)"
    );
    assert!(matches!(&program.statements[0], Statement::Import(_)));
    assert!(matches!(&program.statements[1], Statement::Export(_)));
}

#[test]
fn test_relative_import_requires_td_extension() {
    // Relative import without .td extension should produce a parse error
    let (_, errors) = parse(">>> ./utils => @(helper)");
    assert!(
        !errors.is_empty(),
        "Relative import without .td should error"
    );
    assert!(
        errors[0].message.contains(".td extension"),
        "Error should mention .td extension: {}",
        errors[0].message
    );

    // Relative import with .td extension should parse fine
    let (_, errors) = parse(">>> ./utils.td => @(helper)");
    assert!(
        errors.is_empty(),
        "Relative import with .td should parse: {:?}",
        errors
    );

    // Parent directory relative import without .td should error
    let (_, errors) = parse(">>> ../lib/utils => @(func)");
    assert!(
        !errors.is_empty(),
        "Parent-relative import without .td should error"
    );

    // Parent directory relative import with .td should parse fine
    let (_, errors) = parse(">>> ../lib/utils.td => @(func)");
    assert!(
        errors.is_empty(),
        "Parent-relative import with .td should parse: {:?}",
        errors
    );

    // Package import without .td should still parse fine (not relative)
    let (_, errors) = parse(">>> author/pkg => @(func)");
    assert!(
        errors.is_empty(),
        "Package import without .td should parse: {:?}",
        errors
    );

    // Absolute path import without .td should error
    let (_, errors) = parse(">>> /tmp/shared => @(msg)");
    assert!(
        !errors.is_empty(),
        "Absolute import without .td should error"
    );
    assert!(
        errors[0].message.contains(".td extension"),
        "Error should mention .td extension: {}",
        errors[0].message
    );

    // Absolute path import with .td should parse fine
    let (_, errors) = parse(">>> /tmp/shared.td => @(msg)");
    assert!(
        errors.is_empty(),
        "Absolute import with .td should parse: {:?}",
        errors
    );
}

#[test]
fn test_docs_sample_empty_slot_partial_application_parses() {
    // Empty slot partial application from docs/reference/operators.md
    let source = "add x y = x + y\n=> :Int\nadd5 <= add(5, )";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "Empty slot partial application should parse: {:?}",
        errors
    );
    // FuncDef followed by Assignment — parser may merge trailing statement
    assert!(
        !program.statements.is_empty(),
        "Should produce at least 1 statement"
    );
    // Verify the function definition exists
    assert!(
        program
            .statements
            .iter()
            .any(|s| matches!(s, Statement::FuncDef(f) if f.name == "add")),
        "Expected FuncDef 'add' in AST, got: {:?}",
        program.statements
    );
}

// ── BT-1c: 10-operator rule — parser-level rejection tests ───────

#[test]
fn test_bt1_division_operator_rejected() {
    // PHILOSOPHY.md: `/` operator removed — use Div[x, y]() mold
    let source = "x <= 10 / 2";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Division operator '/' should be rejected at parser level"
    );
    assert!(
        errors.iter().any(|e| {
            let msg = format!("{}", e);
            msg.contains("Div") || msg.contains("removed")
        }),
        "Error should mention Div mold alternative, got: {:?}",
        errors
    );
}

#[test]
fn test_bt1_modulo_operator_rejected() {
    // PHILOSOPHY.md: `%` operator removed — use Mod[x, y]() mold
    let source = "x <= 10 % 3";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Modulo operator '%' should be rejected at parser level"
    );
    assert!(
        errors.iter().any(|e| {
            let msg = format!("{}", e);
            msg.contains("Mod") || msg.contains("removed")
        }),
        "Error should mention Mod mold alternative, got: {:?}",
        errors
    );
}

// ── BT-9: Deep nesting resilience tests ──

#[test]
fn test_deep_parenthesized_expression_50_levels() {
    // 50 levels of parenthesized expression: (((((...42...)))))
    let depth = 50;
    let open = "(".repeat(depth);
    let close = ")".repeat(depth);
    let source = format!("x <= {open}42{close}");
    let (program, errors) = parse(&source);
    assert!(
        errors.is_empty(),
        "Parser should handle 50-level parenthesized expression without error, got: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_deep_list_nesting_50_levels() {
    // 50 levels of list nesting: @[@[@[...@[1]...]]]
    let depth = 50;
    let mut source = String::from("x <= ");
    for _ in 0..depth {
        source.push_str("@[");
    }
    source.push('1');
    for _ in 0..depth {
        source.push(']');
    }
    let (program, errors) = parse(&source);
    assert!(
        errors.is_empty(),
        "Parser should handle 50-level list nesting without error, got: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_deep_buchipack_nesting_50_levels() {
    // 50 levels of BuchiPack nesting: @(a <= @(a <= @(a <= ... 1 ...)))
    let depth = 50;
    let mut source = String::from("x <= ");
    for _ in 0..depth {
        source.push_str("@(a <= ");
    }
    source.push('1');
    for _ in 0..depth {
        source.push(')');
    }
    let (program, errors) = parse(&source);
    assert!(
        errors.is_empty(),
        "Parser should handle 50-level BuchiPack nesting without error, got: {:?}",
        errors
    );
    assert_eq!(program.statements.len(), 1);
}

// ── C12-4 / FB-17: `| |>` pure-expression discipline ────────
//
// An arm body must be a sequence of let-bindings (`<=`, `]=>`, `<=[`)
// followed by exactly one final result expression. Anything else
// (bare function-call statement, discarded pipeline `=> _name`,
// definitions, etc.) is rejected with `[E1616]`.

#[test]
fn test_c12_4_arm_body_pure_expression_passes() {
    // C20-1 (ROOT-5): rewritten to exercise the parenthesised escape
    // hatch for a multi-line rhs guard — pin the same pure-expression
    // discipline without tripping [E0303].
    let source = "grade <= (\n  | 85 >= 90 |> \"A\"\n  | _ |> \"F\"\n)";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_c12_4_arm_body_let_then_expr_passes() {
    // Let-binding followed by final expression is allowed.
    let source = "\
classify n =\n  \
  | n > 0 |>\n    \
    doubled <= n * 2\n    \
    doubled + 1\n  \
  | _ |>\n    \
    zeroed <= 0\n    \
    zeroed - 1\n\
=> :Int\n";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_c12_4_arm_body_unmold_forward_let_passes() {
    // `]=>` unmold-forward is a legal let-binding inside an arm body.
    let source = "\
firstOrZero items =\n  \
  | items.isEmpty() |> 0\n  \
  | _ |>\n    \
    items.first() ]=> first\n    \
    first\n\
=> :Int\n";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_c12_4_arm_body_bare_call_statement_rejected() {
    // FB-17 re-production: a bare discarded function call in the
    // middle of an arm body is rejected.
    let source = "\
validate =\n  \
  | 1 == 0 |>\n    \
    stdout(\"a\")\n    \
    stdout(\"b\")\n  \
  | _ |>\n    \
    \"ok\"\n\
=> :Str\n";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected [E1616] for bare call-statement in arm body"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "Expected E1616, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message.contains("side-effect")),
        "Expected side-effect mention, got: {:?}",
        errors
    );
}

#[test]
fn test_c12_4_arm_body_discarded_pipeline_rejected() {
    // FB-17 canonical example: `writeFile(...) => _wr` is a discarded
    // side-effect (assignment to `_wr`). The assignment itself is
    // allowed (it is a let-binding syntactically), but the following
    // statement must still be a final expression — here a second
    // discarded call is what gets flagged.
    let source = "\
validateProjectRoot =\n  \
  | 1 == 0 |>\n    \
    writeFile(\".hk_write_check\", \"test\") => _wr\n    \
    RemoveFile[\".hk_write_check\"]() => _rm\n  \
  | _ |>\n    \
    \"ok\"\n\
=> :Str\n";
    let (_, errors) = parse(source);
    assert!(
        !errors.is_empty(),
        "Expected [E1616] for FB-17 canonical discard pattern, got none"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "Expected E1616, got: {:?}",
        errors
    );
}

#[test]
fn test_c13_1_arm_body_trailing_let_binding_accepted() {
    // C13-1 migration of the former test_c12_4_arm_body_trailing_let_binding_rejected.
    // Under C13-1 a trailing `name <= expr` (or `expr => name`) in a
    // `| |>` arm body is accepted as a tail binding that yields the
    // bound value as the arm result.
    let source = "\
broken n =\n  \
  | n > 0 |>\n    \
    x <= n + 1\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    let e1616: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("E1616"))
        .collect();
    assert!(
        e1616.is_empty(),
        "C13-1: trailing `x <= n + 1` should be accepted as a tail binding, got: {:?}",
        e1616
    );
}

#[test]
fn test_c12_4_arm_body_nested_cond_passes() {
    // A nested `| |>` inside an arm body is a valid expression and
    // should not be flagged.
    let source = "\
classify x =\n  \
  | x > 100 |>\n    \
    | x > 500 |> \"big\"\n    \
    | _ |> \"medium\"\n  \
  | _ |> \"small\"\n\
=> :Str\n";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "Errors: {:?}", errors);
    assert_eq!(program.statements.len(), 1);
}

// ── C13-1: expression-block tail-binding semantics ───────────

#[test]
fn test_c13_1_arm_body_tail_assignment_accepted() {
    // `name <= expr` at the tail of a `| |>` arm body is accepted
    // under C13-1 and yields the bound value as the arm result.
    let source = "\
f x =\n  \
  | x > 0 |>\n    \
    doubled <= x * 2\n    \
    result <= doubled + 1\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    let e1616: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("E1616"))
        .collect();
    assert!(
        e1616.is_empty(),
        "C13-1: tail `result <= doubled + 1` should be accepted, got: {:?}",
        e1616
    );
}

#[test]
fn test_c13_1_arm_body_tail_forward_assignment_accepted() {
    // `expr => name` at the tail of a `| |>` arm body is accepted
    // under C13-1 and yields the bound value as the arm result.
    let source = "\
f x =\n  \
  | x > 0 |>\n    \
    x * 2 => doubled\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    let e1616: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("E1616"))
        .collect();
    assert!(
        e1616.is_empty(),
        "C13-1: tail `x * 2 => doubled` should be accepted, got: {:?}",
        e1616
    );
}

#[test]
fn test_c13_1_arm_body_tail_unmold_forward_accepted() {
    // `expr ]=> name` at the tail of a `| |>` arm body is accepted
    // under C13-1 and yields the unmolded value.
    let source = "\
f x =\n  \
  | x > 0 |>\n    \
    lax <= Lax[x]()\n    \
    lax ]=> n\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    let e1616: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("E1616"))
        .collect();
    assert!(
        e1616.is_empty(),
        "C13-1: tail `lax ]=> n` should be accepted, got: {:?}",
        e1616
    );
}

#[test]
fn test_c13_1_arm_body_tail_unmold_backward_accepted() {
    // `name <=[ expr` at the tail of a `| |>` arm body is accepted
    // under C13-1 and yields the unmolded value.
    let source = "\
f x =\n  \
  | x > 0 |>\n    \
    lax <= Lax[x]()\n    \
    n <=[ lax\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    let e1616: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("E1616"))
        .collect();
    assert!(
        e1616.is_empty(),
        "C13-1: tail `n <=[ lax` should be accepted, got: {:?}",
        e1616
    );
}

#[test]
fn test_c13_1_arm_body_discard_binding_still_rejected_at_tail() {
    // FB-17 safety: an underscore-prefixed binding target is a discard
    // pattern and must stay rejected even at the tail position in C13-1.
    let source = "\
bad x =\n  \
  | x > 0 |>\n    \
    writeFile(\"/tmp/x\", \"y\") => _wr\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "C13-1: `=> _wr` must stay rejected as a discard pattern, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message.contains("discard binding")),
        "Expected 'discard binding' mention, got: {:?}",
        errors
    );
}

#[test]
fn test_c13b_010_function_body_discard_binding_rejected() {
    // C13B-010: discard bindings (`=> _x`, `_x <=`, `]=> _x`, `_x <=[`)
    // must be rejected anywhere inside a function body — same rule as
    // `| |>` arm body. Only `validate_cond_arm_body` previously enforced
    // this, leaving function bodies as a safety hole.
    let source = "\
f =\n  \
  1 => _x\n  \
  42\n\
=> :Int\n";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "C13B-010: `=> _x` inside function body must be rejected, got: {:?}",
        errors
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("function body") && e.message.contains("discard binding")),
        "C13B-010: error must mention `function body` and `discard binding`, got: {:?}",
        errors
    );
}

#[test]
fn test_c13b_010_function_body_discard_assignment_rejected() {
    // C13B-010: `_y <= expr` variant in function body.
    let source = "\
f =\n  \
  _y <= 1\n  \
  42\n\
=> :Int\n";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "C13B-010: `_y <=` inside function body must be rejected, got: {:?}",
        errors
    );
}

#[test]
fn test_c13b_010_error_ceiling_body_discard_binding_rejected() {
    // C13B-010: discard bindings inside `|==` handler body must also
    // be rejected — the spec treats arm / function / error-ceiling
    // bodies uniformly as expression blocks.
    let source = "\
boom =\n  \
  0\n\
=> :Int\n\
|== e: Error =\n  \
  1 => _x\n  \
  0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "C13B-010: `=> _x` inside `|==` handler body must be rejected, got: {:?}",
        errors
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("`|==` handler body")
                && e.message.contains("discard binding")),
        "C13B-010: error must mention `|==` handler body, got: {:?}",
        errors
    );
}

#[test]
fn test_c13b_010_function_body_non_discard_binding_still_accepted() {
    // C13B-010 guard: a normal (non-underscore) binding in function
    // body must still be accepted — only discard targets trip the check.
    let source = "\
f =\n  \
  x <= 1\n  \
  x + 41\n\
=> :Int\n";
    let (_, errors) = parse(source);
    assert!(
        !errors.iter().any(|e| e.message.contains("E1616")),
        "C13B-010: `x <= 1` (non-discard) must be accepted, got: {:?}",
        errors
    );
}

#[test]
fn test_c13_1_arm_body_non_final_bare_call_still_rejected() {
    // FB-17 safety: a non-tail bare call statement (side-effect in
    // arm body before the final result) remains rejected in C13-1.
    let source = "\
bad x =\n  \
  | x > 0 |>\n    \
    stdout(\"debug\")\n    \
    x\n  \
  | _ |> 0\n\
=> :Int\n";
    let (_, errors) = parse(source);
    assert!(
        errors.iter().any(|e| e.message.contains("E1616")),
        "C13-1: bare call at non-tail must stay rejected, got: {:?}",
        errors
    );
}

// ---------------------------------------------------------------------------
// E30B-007 / Lock-G: RustAddon[...] explicit binding probes (Phase 7 sub-track B)
// ---------------------------------------------------------------------------

#[test]
fn test_e30b_007_rust_addon_binding_parses_as_mold_inst() {
    // Lock-G verdict: `RustAddon["fn"](arity <= N)` is the explicit
    // binding syntax for addon-backed functions on a `.td` facade.
    // Because `RustAddon` starts with uppercase and `[...](...)`
    // matches the existing mold-instantiation surface, the parser
    // already accepts this form as `Expr::MoldInst("RustAddon",
    // [StringLit], [arity field], span)` without any AST changes.
    //
    // This probe pins that contract so the checker / interpreter /
    // codegen layers can rely on the shape when implementing
    // explicit binding semantics.
    use crate::parser::ast::{Expr, Statement};
    let source = "terminalSize <= RustAddon[\"terminalSize\"](arity <= 0)\n";
    let (program, errors) = parse(source);
    assert!(
        errors.is_empty(),
        "expected no parse errors, got: {:?}",
        errors
    );
    assert_eq!(
        program.statements.len(),
        1,
        "expected single Assignment statement"
    );
    let assign = match &program.statements[0] {
        Statement::Assignment(a) => a,
        other => panic!("expected Assignment, got {:?}", other),
    };
    assert_eq!(assign.target, "terminalSize");
    let (name, type_args, fields) = match &assign.value {
        Expr::MoldInst(n, ta, f, _) => (n.as_str(), ta, f),
        other => panic!("expected MoldInst, got {:?}", other),
    };
    assert_eq!(name, "RustAddon");
    assert_eq!(type_args.len(), 1, "expected single type-arg slot");
    let fn_lit = match &type_args[0] {
        Expr::StringLit(s, _) => s.as_str(),
        other => panic!("expected StringLit, got {:?}", other),
    };
    assert_eq!(fn_lit, "terminalSize");
    assert_eq!(fields.len(), 1, "expected single arity field");
    assert_eq!(fields[0].name, "arity");
    let arity_int = match &fields[0].value {
        Expr::IntLit(n, _) => *n,
        other => panic!("expected IntLit, got {:?}", other),
    };
    assert_eq!(arity_int, 0);
}

#[test]
fn test_e30b_007_rust_addon_binding_with_nonzero_arity() {
    // Lock-G drift check requires arity metadata. Confirm parsing
    // for non-zero arity values.
    use crate::parser::ast::{Expr, Statement};
    let source = "readKey <= RustAddon[\"readKey\"](arity <= 2)\n";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "errors: {:?}", errors);
    let assign = match &program.statements[0] {
        Statement::Assignment(a) => a,
        other => panic!("expected Assignment, got {:?}", other),
    };
    let (_n, _ta, fields) = match &assign.value {
        Expr::MoldInst(n, ta, f, _) => (n, ta, f),
        other => panic!("expected MoldInst, got {:?}", other),
    };
    assert_eq!(fields.len(), 1);
    let arity_int = match &fields[0].value {
        Expr::IntLit(n, _) => *n,
        other => panic!("expected IntLit, got {:?}", other),
    };
    assert_eq!(arity_int, 2);
}

#[test]
fn test_e30b_007_rust_addon_binding_rejects_missing_quotes() {
    // Lock-G fixes the syntax: the function name MUST be a string
    // literal so doc-gen / drift check can extract it without
    // resolving identifiers. A bare ident inside the brackets
    // would parse as a different MoldInst surface (TypeIs/etc.
    // type-arg path) and fail at checker level when we add the
    // E1412 path. For the parser probe here we just confirm the
    // string-literal form is the one we lock on.
    use crate::parser::ast::Expr;
    let source = "f <= RustAddon[\"f\"](arity <= 0)\n";
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "errors: {:?}", errors);
    let assign = match &program.statements[0] {
        crate::parser::ast::Statement::Assignment(a) => a,
        other => panic!("expected Assignment, got {:?}", other),
    };
    let type_args = match &assign.value {
        Expr::MoldInst(_, ta, _, _) => ta,
        other => panic!("expected MoldInst, got {:?}", other),
    };
    assert!(
        matches!(&type_args[0], Expr::StringLit(_, _)),
        "Lock-G requires string literal for fn name, got {:?}",
        type_args[0]
    );
}
