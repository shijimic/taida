/// Temporary integration test: verify all examples pass type checking.
/// This test ensures that integrating the type checker into main.rs
/// won't break any existing example files.
use std::fs;

fn check_file(path: &str) -> Vec<String> {
    let source = fs::read_to_string(path).unwrap();
    let (program, parse_errors) = taida::parser::parse(&source);
    if !parse_errors.is_empty() {
        return parse_errors
            .iter()
            .map(|e| format!("PARSE: {}", e))
            .collect();
    }
    let mut checker = taida::types::TypeChecker::new();
    checker.check_program(&program);
    checker.errors.iter().map(|e| format!("{}", e)).collect()
}

#[test]
fn test_all_examples_pass_typecheck() {
    let examples_dir = "examples";
    let mut failures = Vec::new();
    for entry in fs::read_dir(examples_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        // Skip addon example files (they require native addon runtime;
        // same rationale as LSP diagnostics skip).
        // Also skip 3 net_* showcases that still embed legacy
        // `.__value` / `.__error` internal-field access. The skip is
        // removed once the public Lax / Gorillax accessor surface those
        // examples need has landed.
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname.starts_with("addon_")
            || matches!(
                fname,
                "net_http_client.td" | "net_sse_broadcaster.td" | "net_ws_echo.td"
            )
        {
            continue;
        }
        if path.extension().is_some_and(|e| e == "td") {
            let errors = check_file(path.to_str().unwrap());
            if !errors.is_empty() {
                failures.push((path.display().to_string(), errors));
            }
        }
    }
    if !failures.is_empty() {
        let mut msg = String::new();
        for (file, errors) in &failures {
            msg.push_str(&format!("\n=== {} ===\n", file));
            for err in errors {
                msg.push_str(&format!("  {}\n", err));
            }
        }
        // N-72: Error messages include line/column via TypeError's Display impl,
        // so the output below shows "Type error at line X, column Y: ..." for each error.
        panic!("{} example files had type errors:{}", failures.len(), msg);
    }
}

// ── C-11b: docs 由来の否定例（checker がエラーを出すべきケース）──

fn check_source(source: &str) -> Vec<String> {
    let (program, parse_errors) = taida::parser::parse(source);
    if !parse_errors.is_empty() {
        return parse_errors
            .iter()
            .map(|e| format!("PARSE: {}", e))
            .collect();
    }
    let mut checker = taida::types::TypeChecker::new();
    checker.check_program(&program);
    checker.errors.iter().map(|e| format!("{}", e)).collect()
}

#[test]
fn test_negative_same_scope_redefinition() {
    // docs: same-scope redefinition is forbidden
    let errors = check_source("x <= 1\nx <= 2");
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("[E1501]"),
        "Expected E1501, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("line 2"),
        "Error should be on line 2 (redefinition site), got: {}",
        errors[0]
    );
}

#[test]
fn test_negative_function_overload() {
    // docs: function overloading is disallowed
    let errors = check_source("f x: Int =\n  x\n=> :Int\nf x: Str =\n  x\n=> :Str");
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("[E1501]"),
        "Expected E1501, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("line 4"),
        "Error should be on line 4 (second definition), got: {}",
        errors[0]
    );
}

#[test]
fn test_negative_old_placeholder_partial() {
    // docs: old `_` partial application is rejected
    let errors = check_source("add x y = x + y\n=> :Int\nresult <= add(5, _)");
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("[E1502]"),
        "Expected E1502, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("line 3"),
        "Error should be on line 3, got: {}",
        errors[0]
    );
}

#[test]
fn test_negative_typedef_partial_application() {
    // docs: TypeDef partial application is not supported
    let errors = check_source("Point = @(x: Int, y: Int)\np <= Point(1, )");
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("[E1503]"),
        "Expected E1503, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("line 2"),
        "Error should be on line 2, got: {}",
        errors[0]
    );
}

#[test]
fn test_negative_mold_placeholder_outside_pipeline() {
    // docs: Mold[_]() outside pipeline is rejected
    let errors = check_source("x <= Str[_]()");
    assert_eq!(
        errors.len(),
        1,
        "Expected exactly 1 error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("[E1504]"),
        "Expected E1504, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("line 1"),
        "Error should be on line 1, got: {}",
        errors[0]
    );
}

// ── C12B-023 manual-pack bypass closure (2026-04-15) ────────────
//
// The external review found that wasm Regex rejection could be
// bypassed by hand-constructing `@(__type <= "Regex", pattern <= ...,
// flags <= ...)` and passing the pack to `.replaceAll` / `.match`
// /`.search`. The `_poly` dispatchers then routed it as a Regex on
// Interpreter / JS / Native (unvalidated pattern!) and compiled
// successfully on wasm targets (silent UB).
//
// 2026-04-15 v2 follow-up: a second external review showed the
// narrower "literal `__type <= \"Regex\"`" check was itself bypassed
// by `tag <= "Regex"; @(__type <= tag, ...)` (variable binding),
// `if(c, "Regex", "X")` (conditional), `"Re" + "gex"` (concat), and
// function-arg routes. Root fix: reject **any** user-authored
// BuchiPack / TypeInst literal that assigns a `__`-prefixed field
// name, regardless of the value expression. `__`-prefix field names
// are reserved for compiler-internal tags and must only be set by
// official constructors (`Regex`, `Lax`, `Async`, `Result`, ...).
// `[E1617]` fires at type-check time across all backends.

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_topmost() {
    let errors = check_source(
        r#"re <= @(__type <= "Regex", pattern <= "a", flags <= "")
stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617], got: {:?}",
        errors
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("reserved for compiler-internal use") && e.contains("`__type`")),
        "Expected diagnostic to mention reserved `__type` field, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_inside_main() {
    // External reviewer's exact reproduction case.
    let errors = check_source(
        r#"main =
  re <= @(__type <= "Regex", pattern <= "a", flags <= "")
  stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617] for manual pack inside main, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_for_match() {
    // `.match(re)` path — typechecker-level rejection precedes the
    // method-arity Type::Named("Regex") check because pack
    // construction is walked first.
    let errors = check_source(
        r#"re <= @(__type <= "Regex", pattern <= "a", flags <= "i")
x <= "abc".match(re)
stdout(x)
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617] for manual pack fed to .match, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_for_search() {
    let errors = check_source(
        r#"re <= @(__type <= "Regex", pattern <= "x", flags <= "")
i <= "abc".search(re)
stdout(i)
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617] for manual pack fed to .search, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_for_split() {
    let errors = check_source(
        r#"re <= @(__type <= "Regex", pattern <= " ", flags <= "")
parts <= "a b c".split(re)
stdout(parts)
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617] for manual pack fed to .split, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_for_replace() {
    let errors = check_source(
        r#"re <= @(__type <= "Regex", pattern <= "a", flags <= "")
out <= "aba".replace(re, "X")
stdout(out)
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617] for manual pack fed to .replace, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_manual_regex_pack_nested() {
    // Nested: manual Regex pack inside a list / another pack still fires.
    let errors = check_source(
        r#"wrap <= @(res <= @(__type <= "Regex", pattern <= "a", flags <= ""))
stdout("x")
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")),
        "Expected [E1617] for nested manual pack, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_manual_pack_non_regex_type_rejected() {
    // Root fix (2026-04-15 v2): the `__`-prefix is reserved for
    // compiler-internal field names, so even `__type <= "UserTag"` is
    // rejected. User code must pick a non-`__`-prefixed field name
    // (e.g., `tag <= "UserTag"`) for its own discriminators.
    let errors = check_source(
        r#"p <= @(__type <= "UserTag", payload <= "hi")
stdout("ok")
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("`__type`")
            && e.contains("reserved for compiler-internal use")),
        "Expected [E1617] reserved-prefix reject even for non-Regex user tag, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_non_reserved_tag_field_still_allowed() {
    // Guardrail: user tags on non-`__`-prefixed field names (e.g.,
    // `tag`, `kind`, `type`) remain legal. Only `__`-prefix is
    // reserved.
    let errors = check_source(
        r#"p <= @(tag <= "UserTag", payload <= "hi")
q <= @(kind <= "custom", data <= 42)
stdout("ok")
"#,
    );
    assert!(
        errors.is_empty(),
        "Non-`__`-prefixed tag fields must not trip [E1617], got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_variable_bound_tag() {
    // Reviewer's v2 repro: variable-bound `__type` value bypasses the
    // literal check. Root fix rejects based on field name, not value.
    let errors = check_source(
        r#"main =
  tag <= "Regex"
  re <= @(__type <= tag, pattern <= "a", flags <= "")
  stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("[E1617]") && e.contains("`__type`")),
        "Expected [E1617] for variable-bound `__type`, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_function_arg_tag() {
    // Indirect bypass: pass the tag through a function parameter.
    let errors = check_source(
        r#"inner t = @(__type <= t, pattern <= "a", flags <= "")
main =
  re <= inner("Regex")
  stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("[E1617]") && e.contains("`__type`")),
        "Expected [E1617] for function-arg-routed `__type`, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_if_expr_tag() {
    // Indirect bypass: conditional expression as `__type` value.
    let errors = check_source(
        r#"main =
  cond <= true
  re <= @(__type <= if(cond, "Regex", "X"), pattern <= "a", flags <= "")
  stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("[E1617]") && e.contains("`__type`")),
        "Expected [E1617] for if-expr `__type`, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_bypass_rejects_concat_tag() {
    // Indirect bypass: string concatenation as `__type` value.
    let errors = check_source(
        r#"re <= @(__type <= "Re" + "gex", pattern <= "a", flags <= "")
stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("[E1617]") && e.contains("`__type`")),
        "Expected [E1617] for concat `__type`, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_rejects_other_reserved_prefix_fields() {
    // Root fix covers the full `__`-prefix, not just `__type`. Tags
    // like `__value`, `__default`, `__error`, `__tag`, `__items`, etc.
    // are all reserved because compiler-generated nominal packs use
    // them.
    for field in &[
        "__value",
        "__default",
        "__error",
        "__tag",
        "__items",
        "__transforms",
        "__status",
        "__custom_internal",
    ] {
        let src = format!(
            r#"p <= @({} <= "x", data <= 1)
stdout("ok")
"#,
            field
        );
        let errors = check_source(&src);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("[E1617]") && e.contains(&format!("`{}`", field))),
            "Expected [E1617] for reserved prefix field `{}`, got: {:?}",
            field,
            errors
        );
    }
}

#[test]
fn test_c12b_023_typeinst_reserved_field_rejected() {
    // `TypeInst` form (`Name(field <= value, ...)`) shares the check
    // with `BuchiPack` since both are user-authored pack literals.
    let errors = check_source(
        r#"Error => CustomError = @(info: Str)
e <= CustomError(__type <= "Regex", message <= "x", info <= "y")
stdout("ok")
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("[E1617]") && e.contains("`__type`")),
        "Expected [E1617] on TypeInst `__type` assignment, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_official_regex_constructor_still_allowed() {
    // Guardrail: the official `Regex(...)` constructor must keep working
    // on non-wasm backends (this test only exercises the type checker,
    // so it passes on all profiles).
    let errors = check_source(
        r#"re <= Regex("a", "")
stdout("ok")
"#,
    );
    assert!(
        errors.is_empty(),
        "Official Regex(...) constructor must not trip [E1617], got: {:?}",
        errors
    );
}

// ----------------------------------------------------------------------------
// C12B-023 bypass closure — 3rd layer (definition-site)
// ----------------------------------------------------------------------------
// Reviewer (2026-04-15) found a 3rd bypass: user-authored TypeDef / MoldDef /
// InheritanceDef whose field names start with `__` slip past the checker
// because the 2nd-layer fix only inspects `Expr::BuchiPack` / `Expr::TypeInst`
// literals. A definition like `Fake = @(__type <= "Regex", ...)` then
// materialises a pack whose `__type` is literally `"Regex"` when `Fake(...)`
// is instantiated — the same silent-UB class the earlier fixes closed.
// These tests pin the definition-site reject emitted by
// `validate_reserved_internal_field_name` in `src/types/checker.rs`.

#[test]
fn test_c12b_023_typedef_rejects_reserved_prefix_default_type() {
    // Reviewer-provided repro #1: TypeDef default binds `__type <= "Regex"`
    // then `Fake(...)` passes the forged pack to `.replaceAll(re, "x")`.
    // Must be rejected at the TypeDef site, before any user of `Fake`.
    let errors = check_source(
        r#"Fake = @(__type <= "Regex", pattern <= "a", flags <= "", payload: Str)
main =
  re <= Fake(payload <= "x")
  stdout("aba".replaceAll(re, "x"))
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("TypeDef 'Fake'")
            && e.contains("`__type`")),
        "Expected [E1617] on TypeDef `__type` default, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_typedef_rejects_reserved_prefix_body_stream() {
    // Reviewer-provided repro #2: TypeDef carrying `__body_stream` and
    // `__body_token` to fake net-surface internals. Both fields must be
    // rejected (two [E1617] diagnostics).
    let errors = check_source(
        r#"FakeReq = @(__body_stream <= "__v4_body_stream", __body_token <= 99999, x: Int)
main =
  req <= FakeReq(x <= 1)
  stdout("ok")
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("TypeDef 'FakeReq'")
            && e.contains("`__body_stream`")),
        "Expected [E1617] on TypeDef `__body_stream` default, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("TypeDef 'FakeReq'")
            && e.contains("`__body_token`")),
        "Expected [E1617] on TypeDef `__body_token` default, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_typedef_rejects_reserved_prefix_type_annotation_only() {
    // `FieldDef` with only a type annotation (no default) must also be
    // rejected — the name itself is what's reserved, irrespective of
    // whether a default value is bound.
    let errors = check_source(
        r#"Fake = @(__type: Str, payload: Str)
main =
  stdout("ok")
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("TypeDef 'Fake'")
            && e.contains("`__type`")),
        "Expected [E1617] on TypeDef `__type: Str` field, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_inheritancedef_rejects_reserved_prefix_field() {
    // InheritanceDef (`Parent => Child = @(...)`) must also reject
    // `__`-prefix fields on the child side.
    let errors = check_source(
        r#"Error => CustomError = @(__type <= "Regex", info: Str)
main =
  stdout("ok")
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("InheritanceDef 'CustomError'")
            && e.contains("`__type`")),
        "Expected [E1617] on InheritanceDef `__type` field, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_molddef_rejects_reserved_prefix_field() {
    // MoldDef bodies reuse the same `FieldDef` shape as TypeDef, so
    // `__`-prefix fields on the mold body must also be rejected.
    let errors = check_source(
        r#"Mold[T] => FakeMold[T] = @(__value: T, label: Str)
main =
  stdout("ok")
"#,
    );
    assert!(
        errors.iter().any(|e| e.contains("[E1617]")
            && e.contains("MoldDef 'FakeMold'")
            && e.contains("`__value`")),
        "Expected [E1617] on MoldDef `__value` field, got: {:?}",
        errors
    );
}

#[test]
fn test_c12b_023_typedef_rejects_multiple_reserved_prefix_fields() {
    // A TypeDef with multiple `__`-prefix fields emits one [E1617]
    // per offending field (so users see the full set of fixes needed,
    // not a stop-on-first diagnostic experience).
    let errors = check_source(
        r#"Fake = @(__type <= "Regex", __value <= 0, __tag <= "x", payload: Str)
main =
  stdout("ok")
"#,
    );
    let e1617_count = errors.iter().filter(|e| e.contains("[E1617]")).count();
    assert!(
        e1617_count >= 3,
        "Expected 3 [E1617] diagnostics (one per `__` field), got {}: {:?}",
        e1617_count,
        errors
    );
}

#[test]
fn test_c12b_023_typedef_non_reserved_field_still_allowed() {
    // Guardrail: TypeDef with non-`__`-prefix fields still compiles.
    let errors = check_source(
        r#"Person = @(name: Str, age: Int, address <= "unknown")
main =
  p <= Person(name <= "alice", age <= 30)
  stdout(p.name)
"#,
    );
    assert!(
        errors.is_empty(),
        "Non-reserved TypeDef fields must not trip [E1617], got: {:?}",
        errors
    );
}

#[test]
fn test_e32b_018_typedef_internal_field_read_rejected() {
    // E32B-018: `__`-prefixed fields remain compiler-internal metadata.
    // User-facing reads must use official APIs / unmolding paths instead.
    let errors = check_source(
        r#"Error => AppError = @(code: Int)
main =
  e <= AppError(code <= 42)
  stdout("type=" + e.__type)
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("[E1960]") && e.contains("__type")),
        "Reading `.__type` must be rejected by [E1960], got: {:?}",
        errors
    );
}
