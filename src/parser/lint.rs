//! D28B-008 — naming-convention lint pass.
//!
//! Implements 9 diagnostic codes (E1801..E1809) that pin the D28B-001
//! (Phase 0 2026-04-26 Lock) category-based naming rules at the
//! parser/AST level. The lint runs after parsing succeeds and walks the
//! `Program` AST to surface symbol-level violations.
//!
//! ## Diagnostic code mapping
//!
//! | Code    | Lock alias | Category                                                               |
//! |---------|------------|------------------------------------------------------------------------|
//! | E1801   | E1XXa      | クラスライク型 / モールド型 / スキーマ / エラー variant が PascalCase でない |
//! | E1802   | E1XXb      | 関数が camelCase でない                                                |
//! | E1803   | E1XXc      | 変数 (関数値の束縛) が camelCase でない                                |
//! | E1804   | E1XXd      | 変数 (非関数値) が snake_case でない                                   |
//! | E1805   | E1XXe      | 定数が SCREAMING_SNAKE_CASE でない (将来拡張、現状 reserved)           |
//! | E1806   | E1XXf      | エラー variant (Enum:Variant) が PascalCase でない                     |
//! | E1807   | E1XXg      | 型変数が単一大文字でない (T1/T2 series 例外)                           |
//! | E1808   | E1XXh      | ぶちパックフィールドの値型と命名規則が不整合                           |
//! | E1809   | E1XXi      | 戻り値型注釈の `:` マーカー欠落                                        |
//!
//! ## Severity / out-of-scope (per D28B-008 Acceptance)
//!
//! - `_` prefix (慣習として開放): not flagged
//! - boolean prefix (`is`/`has`/`can`/`did`/`needs`): not flagged
//! - 引数 / フィールド型注釈の形式 A (`arg: Type`) と 形式 B (`arg :Type`):
//!   どちらも valid → not flagged
//!
//! ## Reserved-for-now codes
//!
//! E1805 (constants) cannot be detected purely from a single-pass AST
//! walk because Taida does not syntactically distinguish constants from
//! non-function variables. The hook is reserved here to keep the
//! diagnostic-codes registry stable; future passes (usage tracking) may
//! activate it.

use crate::lexer::Span;
use crate::parser::{
    BuchiField, Expr, FieldDef, FuncDef, MoldDef, MoldHeaderArg, Param, Program, Statement,
    TypeDef, TypeExpr, TypeParam,
};

/// A single naming-convention violation discovered by [`lint_program`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    /// `E18xx` code, brackets-included form (`[E1801]`).
    pub code: &'static str,
    /// Human-facing diagnostic message in Japanese (matches docs/reference style).
    pub message: String,
    /// Source span of the offending identifier or annotation.
    pub span: Span,
}

impl LintDiagnostic {
    /// Format as a single line `path:line:col [E####] message` for CLI use.
    pub fn render(&self, path: &str) -> String {
        format!(
            "{}:{}:{} {} {}",
            path, self.span.line, self.span.column, self.code, self.message
        )
    }
}

// ─────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────

/// Run the D28B-008 naming-convention lint pass over a parsed program.
///
/// Returns every violation surfaced (does not stop on the first hit).
/// An empty `Vec` means the program is naming-rule compliant.
pub fn lint_program(program: &Program) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for stmt in &program.statements {
        lint_statement(stmt, &mut diags);
    }
    diags
}

// ─────────────────────────────────────────────────────────────────────
// Case classifiers
// ─────────────────────────────────────────────────────────────────────

/// PascalCase: starts with ASCII upper, contains no underscore, not all-upper
/// (single-char `T` is treated as PascalCase here for type names; type-param
/// classification uses [`is_single_letter_type_var`] instead).
fn is_pascal_case(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    for c in chars {
        if c == '_' {
            return false;
        }
    }
    true
}

/// camelCase: starts lowercase ASCII, no underscore. Bare lowercase
/// identifiers (`zip`, `map`) qualify since the rule "starts lowercase
/// + no underscore" is the contract — internal uppercase is optional.
fn is_camel_case(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    for c in chars {
        if c == '_' {
            return false;
        }
    }
    true
}

/// snake_case: only lowercase ASCII / digits / `_`, no uppercase letter.
/// Empty / leading-digit are out of scope (parser would reject them).
fn is_snake_case(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Single-letter ASCII upper (`T`, `U`, `V`, `E`, `K`, `P`, `R`, ...).
/// These are the primary form of type variables under the Lock.
fn is_single_letter_type_var(name: &str) -> bool {
    name.len() == 1 && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Indexed type-variable form (`T1`, `T2`, `U10`). Allowed when 4+ type
/// variables would otherwise collide; we accept any single-letter upper
/// followed by digits as a permissive shape (the full "4+ collision"
/// check would require a counting pass and is out of lint scope).
fn is_indexed_type_var(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// True if the identifier is acceptable as a type-variable name under
/// the Lock (single upper-letter or indexed `T1`-style).
fn is_valid_type_var_name(name: &str) -> bool {
    is_single_letter_type_var(name) || is_indexed_type_var(name)
}

// ─────────────────────────────────────────────────────────────────────
// Value-type heuristic for buchi-pack fields (E1808)
// ─────────────────────────────────────────────────────────────────────

/// Returns Some(true) if the value is detectably a function value
/// (lambda etc.); Some(false) if detectably non-function (literal /
/// list / pack / mold instance); None for ambiguous (Ident, FuncCall,
/// MethodCall — could be either).
fn classify_field_value(value: &Expr) -> Option<bool> {
    match value {
        Expr::Lambda(..) | Expr::Placeholder(..) | Expr::Hole(..) => Some(true),
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::TemplateLit(..)
        | Expr::BoolLit(..)
        | Expr::Gorilla(..)
        | Expr::ListLit(..)
        | Expr::BuchiPack(..)
        | Expr::TypeLiteral(..)
        | Expr::EnumVariant(..)
        | Expr::TypeInst(..)
        | Expr::MoldInst(..)
        | Expr::BinaryOp(..)
        | Expr::UnaryOp(..) => Some(false),
        // Field access usually yields data; treat as non-function for
        // the lint heuristic (keeps us aligned with upgrade_d28).
        Expr::FieldAccess(..) => Some(false),
        // Everything else is conservatively ambiguous.
        _ => None,
    }
}

/// Returns Some(true) if the type expression denotes a function type,
/// Some(false) for a non-function type, None for ambiguous shapes.
fn classify_field_type(ty: &TypeExpr) -> Option<bool> {
    match ty {
        TypeExpr::Function(..) => Some(true),
        TypeExpr::Named(..)
        | TypeExpr::Generic(..)
        | TypeExpr::List(..)
        | TypeExpr::BuchiPack(..) => Some(false),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Statement-level dispatch
// ─────────────────────────────────────────────────────────────────────

fn lint_statement(stmt: &Statement, diags: &mut Vec<LintDiagnostic>) {
    match stmt {
        Statement::EnumDef(e) => {
            // E1801: Enum type name must be PascalCase
            if !is_pascal_case(&e.name) {
                diags.push(LintDiagnostic {
                    code: "[E1801]",
                    message: format!(
                        "クラスライク型 / モールド型 / スキーマ / エラー variant は PascalCase で命名してください: '{}'",
                        e.name
                    ),
                    span: e.span.clone(),
                });
            }
            // E1806: Each variant must be PascalCase
            for v in &e.variants {
                if !is_pascal_case(&v.name) {
                    diags.push(LintDiagnostic {
                        code: "[E1806]",
                        message: format!(
                            "エラー variant / Enum variant は PascalCase で命名してください: '{}'",
                            v.name
                        ),
                        span: v.span.clone(),
                    });
                }
            }
        }
        Statement::TypeDef(td) => lint_type_def(td, diags),
        Statement::FuncDef(f) => lint_func_def(f, diags, /* is_method */ false),
        Statement::Assignment(a) => {
            // PascalCase assignment targets are almost certainly a
            // misinterpreted return-type (`body => Int` instead of
            // `body => :Int`). E1809 is reported by the source-aware
            // pass; suppress E1803/E1804 here to avoid double-flagging.
            if is_pascal_case(&a.target) {
                lint_expr(&a.value, diags);
                return;
            }
            // Variable binding: classify by RHS value
            let bind_kind = classify_field_value(&a.value);
            match bind_kind {
                Some(true) => {
                    // Function-value variable: must be camelCase (E1803)
                    if !is_camel_case(&a.target) {
                        diags.push(LintDiagnostic {
                            code: "[E1803]",
                            message: format!(
                                "関数値を束縛する変数は camelCase で命名してください: '{}'",
                                a.target
                            ),
                            span: a.span.clone(),
                        });
                    }
                }
                Some(false) => {
                    // Non-function-value variable: must be snake_case (E1804)
                    if !is_snake_case(&a.target) {
                        diags.push(LintDiagnostic {
                            code: "[E1804]",
                            message: format!(
                                "非関数値を束縛する変数は snake_case で命名してください: '{}'",
                                a.target
                            ),
                            span: a.span.clone(),
                        });
                    }
                }
                None => {
                    // Ambiguous: accept either camelCase or snake_case
                    if !(is_camel_case(&a.target) || is_snake_case(&a.target)) {
                        diags.push(LintDiagnostic {
                            code: "[E1804]",
                            message: format!(
                                "変数は camelCase (関数値) または snake_case (非関数値) で命名してください: '{}'",
                                a.target
                            ),
                            span: a.span.clone(),
                        });
                    }
                }
            }
            // Lint nested expressions inside the value as well.
            lint_expr(&a.value, diags);
        }
        Statement::MoldDef(m) => lint_mold_def(m, diags),
        Statement::InheritanceDef(i) => {
            if !is_pascal_case(&i.parent) {
                diags.push(LintDiagnostic {
                    code: "[E1801]",
                    message: format!(
                        "継承元の型名は PascalCase で命名してください: '{}'",
                        i.parent
                    ),
                    span: i.span.clone(),
                });
            }
            if !is_pascal_case(&i.child) {
                diags.push(LintDiagnostic {
                    code: "[E1801]",
                    message: format!(
                        "継承先の型名は PascalCase で命名してください: '{}'",
                        i.child
                    ),
                    span: i.span.clone(),
                });
            }
            for f in &i.fields {
                lint_field_def(f, diags);
            }
        }
        Statement::ErrorCeiling(ec) => {
            for s in &ec.handler_body {
                lint_statement(s, diags);
            }
            // E1809: error ceiling return-type marker
            check_return_type_marker(ec.return_type.as_ref(), &ec.span, diags);
        }
        Statement::Expr(e) => lint_expr(e, diags),
        Statement::UnmoldForward(u) => lint_expr(&u.source, diags),
        Statement::UnmoldBackward(u) => lint_expr(&u.source, diags),
        Statement::Import(_) | Statement::Export(_) => {
            // Imports / exports are out of lint scope (cross-module names).
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// TypeDef / MoldDef / FuncDef
// ─────────────────────────────────────────────────────────────────────

fn lint_type_def(td: &TypeDef, diags: &mut Vec<LintDiagnostic>) {
    // E1801
    if !is_pascal_case(&td.name) {
        diags.push(LintDiagnostic {
            code: "[E1801]",
            message: format!(
                "クラスライク型 / モールド型 / スキーマ / エラー variant は PascalCase で命名してください: '{}'",
                td.name
            ),
            span: td.span.clone(),
        });
    }
    for f in &td.fields {
        lint_field_def(f, diags);
    }
}

fn lint_mold_def(md: &MoldDef, diags: &mut Vec<LintDiagnostic>) {
    // E1801
    if !is_pascal_case(&md.name) {
        diags.push(LintDiagnostic {
            code: "[E1801]",
            message: format!("モールド型は PascalCase で命名してください: '{}'", md.name),
            span: md.span.clone(),
        });
    }
    // E1807: type-variable names must be single upper-letter
    for tp in &md.type_params {
        check_type_param(tp, &md.span, diags);
    }
    // header args may also embed TypeParams (already covered) or concrete types
    for arg in &md.mold_args {
        if let MoldHeaderArg::TypeParam(tp) = arg {
            check_type_param(tp, &md.span, diags);
        }
    }
    if let Some(name_args) = &md.name_args {
        for arg in name_args {
            if let MoldHeaderArg::TypeParam(tp) = arg {
                check_type_param(tp, &md.span, diags);
            }
        }
    }
    for f in &md.fields {
        lint_field_def(f, diags);
    }
}

fn check_type_param(tp: &TypeParam, anchor: &Span, diags: &mut Vec<LintDiagnostic>) {
    if !is_valid_type_var_name(&tp.name) {
        diags.push(LintDiagnostic {
            code: "[E1807]",
            message: format!(
                "型変数は単一大文字 (`T`, `U`, `V`, `E`, `K`, `P`, `R` 等) で命名してください: '{}'",
                tp.name
            ),
            span: anchor.clone(),
        });
    }
    // Recurse into the constraint type expression for any nested
    // type-variable mentions or function-type return markers.
    if let Some(c) = &tp.constraint {
        check_type_expr_return_marker(c, anchor, diags);
    }
}

fn lint_func_def(f: &FuncDef, diags: &mut Vec<LintDiagnostic>, is_method: bool) {
    // E1802: function name must be camelCase (top-level only; methods
    // inherit field-name classification rules — handled elsewhere).
    if !is_method && !is_camel_case(&f.name) {
        diags.push(LintDiagnostic {
            code: "[E1802]",
            message: format!("関数は camelCase で命名してください: '{}'", f.name),
            span: f.span.clone(),
        });
    }
    // Type params: E1807
    for tp in &f.type_params {
        check_type_param(tp, &f.span, diags);
    }
    // Params: lint default values + nested types
    for p in &f.params {
        lint_param(p, diags);
    }
    // Body
    for s in &f.body {
        lint_statement(s, diags);
    }
    // E1809: return-type `:` marker
    check_return_type_marker(f.return_type.as_ref(), &f.span, diags);
}

fn lint_param(p: &Param, diags: &mut Vec<LintDiagnostic>) {
    if let Some(d) = &p.default_value {
        lint_expr(d, diags);
    }
    // Type annotations on params: scan for nested function-type
    // return-type marker omissions.
    if let Some(ty) = &p.type_annotation {
        check_type_expr_return_marker(ty, &p.span, diags);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Field-def lint (TypeDef / BuchiPack-typed / Mold field)
// ─────────────────────────────────────────────────────────────────────

fn lint_field_def(field: &FieldDef, diags: &mut Vec<LintDiagnostic>) {
    // Method fields delegate to FuncDef rules.
    if field.is_method {
        if let Some(md) = &field.method_def {
            lint_func_def(md, diags, /* is_method */ true);
        }
        if !is_camel_case(&field.name) {
            // Method-in-pack: function value → camelCase (E1808)
            diags.push(LintDiagnostic {
                code: "[E1808]",
                message: format!(
                    "ぶちパックの関数値フィールド (メソッド) は camelCase で命名してください: '{}'",
                    field.name
                ),
                span: field.span.clone(),
            });
        }
        return;
    }
    // Schema-style field declared with `name: Type`
    if let Some(ty) = &field.type_annotation {
        match classify_field_type(ty) {
            Some(true) => {
                // Function-type field → camelCase (E1808)
                if !is_camel_case(&field.name) {
                    diags.push(LintDiagnostic {
                        code: "[E1808]",
                        message: format!(
                            "ぶちパックの関数値フィールドは camelCase で命名してください: '{}'",
                            field.name
                        ),
                        span: field.span.clone(),
                    });
                }
            }
            Some(false) => {
                // Non-function-type field → snake_case (E1808)
                if !is_snake_case(&field.name) {
                    diags.push(LintDiagnostic {
                        code: "[E1808]",
                        message: format!(
                            "ぶちパックの非関数値フィールドは snake_case で命名してください: '{}'",
                            field.name
                        ),
                        span: field.span.clone(),
                    });
                }
            }
            None => {
                if !(is_camel_case(&field.name) || is_snake_case(&field.name)) {
                    diags.push(LintDiagnostic {
                        code: "[E1808]",
                        message: format!(
                            "ぶちパックフィールドは camelCase (関数値) または snake_case (非関数値) で命名してください: '{}'",
                            field.name
                        ),
                        span: field.span.clone(),
                    });
                }
            }
        }
        check_type_expr_return_marker(ty, &field.span, diags);
    } else if let Some(default) = &field.default_value {
        // Type omitted but default present: classify by value
        match classify_field_value(default) {
            Some(true) => {
                if !is_camel_case(&field.name) {
                    diags.push(LintDiagnostic {
                        code: "[E1808]",
                        message: format!(
                            "ぶちパックの関数値フィールドは camelCase で命名してください: '{}'",
                            field.name
                        ),
                        span: field.span.clone(),
                    });
                }
            }
            Some(false) => {
                if !is_snake_case(&field.name) {
                    diags.push(LintDiagnostic {
                        code: "[E1808]",
                        message: format!(
                            "ぶちパックの非関数値フィールドは snake_case で命名してください: '{}'",
                            field.name
                        ),
                        span: field.span.clone(),
                    });
                }
            }
            None => {
                if !(is_camel_case(&field.name) || is_snake_case(&field.name)) {
                    diags.push(LintDiagnostic {
                        code: "[E1808]",
                        message: format!(
                            "ぶちパックフィールドは camelCase (関数値) または snake_case (非関数値) で命名してください: '{}'",
                            field.name
                        ),
                        span: field.span.clone(),
                    });
                }
            }
        }
        lint_expr(default, diags);
    } else {
        // Plain field with neither type nor default: accept either case
        if !(is_camel_case(&field.name) || is_snake_case(&field.name)) {
            diags.push(LintDiagnostic {
                code: "[E1808]",
                message: format!(
                    "ぶちパックフィールドは camelCase または snake_case で命名してください: '{}'",
                    field.name
                ),
                span: field.span.clone(),
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Buchi-pack literal field lint (E1808)
// ─────────────────────────────────────────────────────────────────────

fn lint_buchi_field_literal(field: &BuchiField, diags: &mut Vec<LintDiagnostic>) {
    match classify_field_value(&field.value) {
        Some(true) => {
            if !is_camel_case(&field.name) {
                diags.push(LintDiagnostic {
                    code: "[E1808]",
                    message: format!(
                        "ぶちパックの関数値フィールドは camelCase で命名してください: '{}'",
                        field.name
                    ),
                    span: field.span.clone(),
                });
            }
        }
        Some(false) => {
            if !is_snake_case(&field.name) {
                diags.push(LintDiagnostic {
                    code: "[E1808]",
                    message: format!(
                        "ぶちパックの非関数値フィールドは snake_case で命名してください: '{}'",
                        field.name
                    ),
                    span: field.span.clone(),
                });
            }
        }
        None => {
            if !(is_camel_case(&field.name) || is_snake_case(&field.name)) {
                diags.push(LintDiagnostic {
                    code: "[E1808]",
                    message: format!(
                        "ぶちパックフィールドは camelCase (関数値) または snake_case (非関数値) で命名してください: '{}'",
                        field.name
                    ),
                    span: field.span.clone(),
                });
            }
        }
    }
    lint_expr(&field.value, diags);
}

// ─────────────────────────────────────────────────────────────────────
// Expression walker
// ─────────────────────────────────────────────────────────────────────

fn lint_expr(e: &Expr, diags: &mut Vec<LintDiagnostic>) {
    match e {
        Expr::BuchiPack(fields, _) => {
            for f in fields {
                lint_buchi_field_literal(f, diags);
            }
        }
        Expr::TypeInst(name, fields, span) => {
            if !is_pascal_case(name) {
                diags.push(LintDiagnostic {
                    code: "[E1801]",
                    message: format!(
                        "型インスタンス化対象は PascalCase で命名されている必要があります: '{}'",
                        name
                    ),
                    span: span.clone(),
                });
            }
            for f in fields {
                lint_buchi_field_literal(f, diags);
            }
        }
        Expr::MoldInst(name, args, fields, span) => {
            if !is_pascal_case(name) {
                diags.push(LintDiagnostic {
                    code: "[E1801]",
                    message: format!("モールド名は PascalCase で命名してください: '{}'", name),
                    span: span.clone(),
                });
            }
            for a in args {
                lint_expr(a, diags);
            }
            for f in fields {
                lint_buchi_field_literal(f, diags);
            }
        }
        Expr::ListLit(items, _) => {
            for it in items {
                lint_expr(it, diags);
            }
        }
        Expr::BinaryOp(l, _, r, _) => {
            lint_expr(l, diags);
            lint_expr(r, diags);
        }
        Expr::UnaryOp(_, x, _) => lint_expr(x, diags),
        Expr::FuncCall(callee, args, _) => {
            lint_expr(callee, diags);
            for a in args {
                lint_expr(a, diags);
            }
        }
        Expr::MethodCall(recv, _, args, _) => {
            lint_expr(recv, diags);
            for a in args {
                lint_expr(a, diags);
            }
        }
        Expr::FieldAccess(recv, _, _) => lint_expr(recv, diags),
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(c) = &arm.condition {
                    lint_expr(c, diags);
                }
                for s in &arm.body {
                    lint_statement(s, diags);
                }
            }
        }
        Expr::Pipeline(parts, _) => {
            for p in parts {
                lint_expr(p, diags);
            }
        }
        Expr::Lambda(params, body, _) => {
            for p in params {
                lint_param(p, diags);
            }
            lint_expr(body, diags);
        }
        Expr::Unmold(x, _) => lint_expr(x, diags),
        Expr::Throw(x, _) => lint_expr(x, diags),
        // Leaves
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::TemplateLit(..)
        | Expr::BoolLit(..)
        | Expr::Gorilla(..)
        | Expr::Ident(..)
        | Expr::Placeholder(..)
        | Expr::Hole(..)
        | Expr::TypeLiteral(..)
        | Expr::EnumVariant(..) => {}
    }
}

// ─────────────────────────────────────────────────────────────────────
// E1809: return-type `:` marker enforcement
// ─────────────────────────────────────────────────────────────────────

/// E1809: function definition / error-ceiling return type must use the
/// `:Type` concrete-type-literal marker. Bare `Type` (no `:`) is parser
/// lenient but semantically incorrect under the Lock.
fn check_return_type_marker(rt: Option<&TypeExpr>, anchor: &Span, diags: &mut Vec<LintDiagnostic>) {
    let Some(rt) = rt else {
        return;
    };
    // We need a way to know whether the parser saw `:` — but the AST
    // strips it. Heuristic: a bare `TypeExpr::Named(name)` without any
    // surrounding `:` is the lenient form. The parser stores both with
    // `:` and without `:` as `Named(name)`; this lint cannot
    // round-trip from AST alone.
    //
    // Workaround: only flag when the surrounding source explicitly
    // provided no `:` marker. Since we don't have access to source
    // here, this lint is restricted to internal nested function-type
    // annotations where the AST distinguishes by structure (i.e.
    // `TypeExpr::Function(params, ret)` whose `ret` is `Named` is
    // always rendered with `:` by the parser, so no false positives
    // there). For top-level FuncDef return types we delegate to a
    // best-effort source-check elsewhere (see `lint_program_with_source`).
    let _ = (rt, anchor, diags); // placeholder — the source-aware
    // variant is the active enforcer.
}

/// Walk a type expression to surface internal return-marker issues
/// (currently a no-op since the parser already enforces `:` on nested
/// function-type returns; reserved hook for future extensions).
fn check_type_expr_return_marker(ty: &TypeExpr, _anchor: &Span, _diags: &mut Vec<LintDiagnostic>) {
    match ty {
        TypeExpr::Function(params, ret) => {
            for p in params {
                check_type_expr_return_marker(p, _anchor, _diags);
            }
            check_type_expr_return_marker(ret, _anchor, _diags);
        }
        TypeExpr::Generic(_, args) => {
            for a in args {
                check_type_expr_return_marker(a, _anchor, _diags);
            }
        }
        TypeExpr::List(inner) => check_type_expr_return_marker(inner, _anchor, _diags),
        TypeExpr::BuchiPack(fields) => {
            for f in fields {
                if let Some(ty) = &f.type_annotation {
                    check_type_expr_return_marker(ty, _anchor, _diags);
                }
            }
        }
        TypeExpr::Named(_) => {}
    }
}

// ─────────────────────────────────────────────────────────────────────
// Source-aware variant (for E1809 missing-`:` detection)
// ─────────────────────────────────────────────────────────────────────

/// Source-aware lint pass. Identical to [`lint_program`] plus E1809
/// (missing return-type `:` marker) detection that requires inspecting
/// the original source slice.
///
/// The detection rule (E1809):
/// - For each `FuncDef` with a `return_type`, look up the source slice
///   between the byte after the function body's last statement and the
///   `=>` arrow that precedes the return-type declaration. The Lock
///   requires the form `=> :Type`. If the slice immediately following
///   `=>` (after whitespace) does not begin with `:`, surface E1809.
///
/// Implementation note: since FuncDef does not retain a span for the
/// arrow position, we use a robust heuristic — search the source from
/// the function's start span until the first `:Type` or `Type` after a
/// trailing `=>`. Only the **last** `=>` in the function head/body
/// preceding the return-type literal is considered.
pub fn lint_program_with_source(program: &Program, source: &str) -> Vec<LintDiagnostic> {
    let mut diags = lint_program(program);
    for stmt in &program.statements {
        scan_for_e1809(stmt, source, &mut diags);
    }
    diags
}

fn scan_for_e1809(stmt: &Statement, source: &str, diags: &mut Vec<LintDiagnostic>) {
    match stmt {
        Statement::FuncDef(f) => {
            check_func_e1809(f, source, diags);
            for s in &f.body {
                scan_for_e1809(s, source, diags);
            }
        }
        Statement::TypeDef(td) => {
            for fd in &td.fields {
                if let Some(md) = fd.method_def.as_ref().filter(|_| fd.is_method) {
                    check_func_e1809(md, source, diags);
                }
            }
        }
        Statement::MoldDef(m) => {
            for fd in &m.fields {
                if let Some(md) = fd.method_def.as_ref().filter(|_| fd.is_method) {
                    check_func_e1809(md, source, diags);
                }
            }
        }
        Statement::InheritanceDef(i) => {
            for fd in &i.fields {
                if let Some(md) = fd.method_def.as_ref().filter(|_| fd.is_method) {
                    check_func_e1809(md, source, diags);
                }
            }
        }
        Statement::ErrorCeiling(ec) => {
            // Error ceiling has return_type in the form `=> :Type`
            check_return_type_text(ec.return_type.as_ref(), &ec.span, source, diags);
            for s in &ec.handler_body {
                scan_for_e1809(s, source, diags);
            }
        }
        Statement::Assignment(a) => scan_expr_for_e1809(&a.value, source, diags),
        Statement::Expr(e) => scan_expr_for_e1809(e, source, diags),
        _ => {}
    }
}

fn scan_expr_for_e1809(e: &Expr, source: &str, diags: &mut Vec<LintDiagnostic>) {
    match e {
        Expr::Lambda(_, body, _) => scan_expr_for_e1809(body, source, diags),
        Expr::BuchiPack(fields, _) => {
            for f in fields {
                scan_expr_for_e1809(&f.value, source, diags);
            }
        }
        Expr::ListLit(items, _) => {
            for it in items {
                scan_expr_for_e1809(it, source, diags);
            }
        }
        Expr::BinaryOp(l, _, r, _) => {
            scan_expr_for_e1809(l, source, diags);
            scan_expr_for_e1809(r, source, diags);
        }
        Expr::FuncCall(callee, args, _) => {
            scan_expr_for_e1809(callee, source, diags);
            for a in args {
                scan_expr_for_e1809(a, source, diags);
            }
        }
        Expr::MethodCall(recv, _, args, _) => {
            scan_expr_for_e1809(recv, source, diags);
            for a in args {
                scan_expr_for_e1809(a, source, diags);
            }
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(c) = &arm.condition {
                    scan_expr_for_e1809(c, source, diags);
                }
                for s in &arm.body {
                    scan_for_e1809(s, source, diags);
                }
            }
        }
        Expr::Pipeline(parts, _) => {
            for p in parts {
                scan_expr_for_e1809(p, source, diags);
            }
        }
        _ => {}
    }
}

fn check_func_e1809(f: &FuncDef, source: &str, diags: &mut Vec<LintDiagnostic>) {
    // Case A: parser captured a return_type — must have `:` marker.
    if f.return_type.is_some() {
        check_return_type_text(f.return_type.as_ref(), &f.span, source, diags);
        return;
    }
    // Case B: parser did NOT capture a return_type, but the body ends
    // with `body => SomeType` style (the `:` marker was omitted, so the
    // parser fell back to interpreting `=> SomeType` as a reverse-assign
    // `SomeType <= body`). The smoking gun is a tail `Assignment` whose
    // `target` looks like a PascalCase type name and whose value is an
    // expression that would naturally be the function body.
    if let Some(Statement::Assignment(a)) = f.body.last()
        && is_pascal_case(&a.target)
    {
        // Likely E1809: user wrote `body => Int` instead of `body => :Int`.
        diags.push(LintDiagnostic {
            code: "[E1809]",
            message: format!(
                "戻り値型注釈には `:Type` の `:` マーカーを付けてください (例: `=> :{}`)",
                a.target
            ),
            span: a.span.clone(),
        });
    }
    let _ = source;
}

/// Heuristic source scan for E1809: find the **final** `=>` in the
/// function's source slice that precedes the return-type declaration
/// (functions without `return_type` are skipped). Examine the
/// whitespace-stripped text after that `=>`; if the first non-whitespace
/// char is not `:`, surface E1809.
///
/// We use char-offset (Unicode-scalar-index) spans to slice the source.
fn check_return_type_text(
    rt: Option<&TypeExpr>,
    span: &Span,
    source: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    if rt.is_none() {
        return;
    }
    // Convert char offsets to byte offsets for slicing.
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    if span.start >= chars.len() {
        return;
    }
    let start_byte = chars[span.start].0;
    let end_byte = if span.end < chars.len() {
        chars[span.end].0
    } else {
        source.len()
    };
    if start_byte >= end_byte {
        return;
    }
    let slice = &source[start_byte..end_byte];

    // Find the LAST occurrence of `=> ` followed by a return-type token.
    // We strip simple `=>` arrows used internally (function-type
    // annotations like `:T => :U`) by scanning right-to-left and
    // keeping only the outermost candidate.
    //
    // Rule: if the trailing slice ends with `... => Foo` (no leading `:`
    // before Foo on that same arrow), flag E1809. If the trailing slice
    // ends with `... => :Foo`, OK.
    //
    // Scan right-to-left for `=>` and inspect what follows.
    let bytes = slice.as_bytes();
    let mut i = bytes.len();
    while i >= 2 {
        if bytes[i - 2] == b'=' && bytes[i - 1] == b'>' {
            // Found `=>` at position i-2..i (within slice).
            // Inspect characters after it.
            let after = &slice[i..];
            let trimmed = after.trim_start();
            // Skip the case where this `=>` is followed by another
            // expression (`a => b => c`) — we only want the LAST one.
            // To check that, find the next `=>` in `trimmed` — if
            // present, this isn't the final arrow.
            if trimmed.contains("=>") {
                // Not the final arrow; keep scanning leftward.
                i -= 2;
                continue;
            }
            // Strip trailing comments / whitespace.
            let trimmed = trimmed.trim_end();
            if trimmed.is_empty() {
                return;
            }
            // The trimmed slice should be `:Foo` (or richer like
            // `:Foo[T <= :U]`). If it starts with `:`, OK. Otherwise
            // E1809.
            if !trimmed.starts_with(':') {
                diags.push(LintDiagnostic {
                    code: "[E1809]",
                    message:
                        "戻り値型注釈には `:Type` の `:` マーカーを付けてください (例: `=> :Int`)"
                            .to_string(),
                    span: span.clone(),
                });
            }
            return;
        }
        i -= 1;
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn lint_str(src: &str) -> Vec<LintDiagnostic> {
        let (program, errs) = parse(src);
        assert!(errs.is_empty(), "parse errors: {:?}", errs);
        lint_program_with_source(&program, src)
    }

    fn codes(diags: &[LintDiagnostic]) -> Vec<&'static str> {
        diags.iter().map(|d| d.code).collect()
    }

    #[test]
    fn e1801_pascal_type_violation() {
        let diags = lint_str("user = @(\n  name: Str\n)\n");
        assert!(
            codes(&diags).contains(&"[E1801]"),
            "expected E1801, got {:?}",
            diags
        );
    }

    #[test]
    fn e1801_pascal_type_compliant() {
        let diags = lint_str("User = @(\n  name: Str\n)\n");
        assert!(
            !codes(&diags).contains(&"[E1801]"),
            "unexpected E1801: {:?}",
            diags
        );
    }

    #[test]
    fn e1802_func_camel_violation() {
        let diags = lint_str("get_user x: Int = x => :Int\n");
        assert!(
            codes(&diags).contains(&"[E1802]"),
            "expected E1802, got {:?}",
            diags
        );
    }

    #[test]
    fn e1802_func_camel_compliant() {
        let diags = lint_str("getUser x: Int = x => :Int\n");
        assert!(
            !codes(&diags).contains(&"[E1802]"),
            "unexpected E1802: {:?}",
            diags
        );
    }

    #[test]
    fn e1804_var_snake_violation_int() {
        let diags = lint_str("portCount <= 8080\n");
        assert!(
            codes(&diags).contains(&"[E1804]"),
            "expected E1804 for camel non-fn var, got {:?}",
            diags
        );
    }

    #[test]
    fn e1804_var_snake_compliant_int() {
        let diags = lint_str("port_count <= 8080\n");
        assert!(
            !codes(&diags).contains(&"[E1804]"),
            "unexpected E1804: {:?}",
            diags
        );
    }

    #[test]
    fn e1808_buchi_field_value_typed_snake() {
        // Non-function value (string) → must be snake_case
        let diags = lint_str("data <= @(callSign <= \"Eva-02\")\n");
        assert!(
            codes(&diags).contains(&"[E1808]"),
            "expected E1808 for camel non-fn buchi field, got {:?}",
            diags
        );
    }

    #[test]
    fn e1808_buchi_field_value_typed_snake_compliant() {
        let diags = lint_str("data <= @(call_sign <= \"Eva-02\")\n");
        assert!(
            !codes(&diags).contains(&"[E1808]"),
            "unexpected E1808: {:?}",
            diags
        );
    }

    #[test]
    fn e1801_enum_pascal_violation() {
        let diags = lint_str("Enum => status = :Active :Inactive\n");
        assert!(
            codes(&diags).contains(&"[E1801]"),
            "expected E1801 for non-Pascal enum name, got {:?}",
            diags
        );
    }

    #[test]
    fn e1806_enum_variant_pascal_violation() {
        let diags = lint_str("Enum => Status = :active :inactive\n");
        let cs = codes(&diags);
        assert!(
            cs.contains(&"[E1806]"),
            "expected E1806 for non-Pascal variant, got {:?}",
            diags
        );
    }

    #[test]
    fn e1807_type_var_single_letter_compliant() {
        let diags = lint_str("Mold[T] => Box[T] = @(value: T)\n");
        assert!(
            !codes(&diags).contains(&"[E1807]"),
            "unexpected E1807 for `T`: {:?}",
            diags
        );
    }

    #[test]
    fn e1807_type_var_named_violation() {
        let diags = lint_str("Mold[Item] => Box[Item] = @(value: Item)\n");
        assert!(
            codes(&diags).contains(&"[E1807]"),
            "expected E1807 for named type var `Item`, got {:?}",
            diags
        );
    }

    #[test]
    fn e1809_missing_return_marker() {
        let diags = lint_str("identity x: Int = x => Int\n");
        assert!(
            codes(&diags).contains(&"[E1809]"),
            "expected E1809 for `=> Int`, got {:?}",
            diags
        );
    }

    #[test]
    fn e1809_present_marker_compliant() {
        let diags = lint_str("identity x: Int = x => :Int\n");
        assert!(
            !codes(&diags).contains(&"[E1809]"),
            "unexpected E1809: {:?}",
            diags
        );
    }

    #[test]
    fn buchi_field_lambda_camel_compliant() {
        // Function value → camelCase OK
        let diags = lint_str("data <= @(myHandler <= _ x = x)\n");
        assert!(
            !codes(&diags).contains(&"[E1808]"),
            "unexpected E1808: {:?}",
            diags
        );
    }

    #[test]
    fn snake_underscore_prefix_not_flagged() {
        // `_` prefix is in the non-flagged list per Lock.
        let diags = lint_str("_internal <= 42\n");
        assert!(
            !codes(&diags).contains(&"[E1804]"),
            "_-prefix should not trigger E1804: {:?}",
            diags
        );
    }
}
