//! infer — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::lexer::Span;
use crate::parser::*;
use crate::types::Type;
use std::collections::{HashMap, HashSet};

use super::{
    CageBranch, FunctionHintDiagnostic, RESERVED_INTERNAL_FIELD_PREFIX, TypeChecker, TypeError,
};

impl TypeChecker {
    fn result_type(success_ty: Type) -> Type {
        Type::Generic(
            "Result".to_string(),
            vec![success_ty, Type::Named("ErrorInfo".to_string())],
        )
    }

    fn async_type(inner_ty: Type) -> Type {
        Type::Generic("Async".to_string(), vec![inner_ty])
    }

    fn core_builtin_return_type(&mut self, name: &str, args: &[Expr]) -> Option<Type> {
        match name {
            "debug" => Some(
                args.first()
                    .map(|arg| self.infer_expr_type(arg))
                    .unwrap_or(Type::Unit),
            ),
            "toString" | "toStr" => Some(Type::Str),
            "strOf" => Some(Type::Str),
            "typeOf" | "typeof" => Some(Type::Str),
            "jsonEncode" | "jsonPretty" => Some(Type::Str),
            "nowMs" => Some(Type::Int),
            // F42 sweep: assert returns Bool(true) on success, throws on failure.
            // Aligned with `src/interpreter/prelude.rs:801` (Value::Bool(true)).
            "assert" => Some(Type::Bool),
            "throw" => Some(Type::Unknown),
            "range" => Some(Type::List(Box::new(Type::Int))),
            "enumerate" => Some(Type::List(Box::new(Type::Unknown))),
            "zip" => Some(Type::List(Box::new(Type::Unknown))),
            "hashMap" => Some(Type::Named("HashMap".to_string())),
            "setOf" => Some(Type::Named("Set".to_string())),
            "stdout" | "stderr" => Some(Type::Int),
            "exit" => Some(Type::Int),
            "stdin" => Some(Type::Str),
            "stdinLine" => Some(Self::async_type(Type::Generic(
                "Lax".to_string(),
                vec![Type::Str],
            ))),
            "argv" => Some(Type::List(Box::new(Type::Str))),
            "sleep" => Some(Self::async_type(Type::Int)),
            "Regex" => Some(Type::Named("Regex".to_string())),
            "readBytes" | "readBytesAt" => {
                Some(Type::Generic("Lax".to_string(), vec![Type::Bytes]))
            }
            "writeFile" | "writeBytes" | "appendFile" | "remove" | "createDir" | "rename" => {
                Some(Self::result_type(Type::Int))
            }
            "allEnv" => Some(Type::Generic(
                "HashMap".to_string(),
                vec![Type::Str, Type::Str],
            )),
            "poolHealth" => Some(Type::BuchiPack(vec![
                ("open".to_string(), Type::Bool),
                ("idle".to_string(), Type::Int),
                ("inUse".to_string(), Type::Int),
                ("waiting".to_string(), Type::Int),
            ])),
            known if Self::core_builtin_arity(known).is_some() => {
                debug_assert!(
                    Self::core_builtin_allows_unknown_return(known),
                    "core builtin arity/return registries drifted for {known}"
                );
                Some(Type::Unknown)
            }
            _ => None,
        }
    }

    pub(super) fn wire_encodable_expr_type(&mut self, expr: &Expr) -> (Type, bool) {
        if matches!(expr, Expr::ListLit(items, _) if items.is_empty()) {
            let ty = Type::List(Box::new(Type::Any));
            return (ty, true);
        }
        let ty = self.infer_expr_type(expr);
        let ok = self.is_wire_encodable_type(&ty);
        (ty, ok)
    }

    pub(super) fn is_async_type(ty: &Type) -> bool {
        matches!(ty, Type::Generic(name, _) if name == "Async")
    }

    pub(super) fn static_add_operand_type(expr: &Expr) -> Option<Type> {
        match expr {
            Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => Some(Type::Str),
            Expr::IntLit(_, _) => Some(Type::Int),
            Expr::FloatLit(_, _) => Some(Type::Float),
            Expr::BoolLit(_, _) => Some(Type::Bool),
            Expr::ListLit(_, _) => Some(Type::List(Box::new(Type::Unknown))),
            Expr::BuchiPack(_, _) | Expr::TypeInst(_, _, _) => Some(Type::Unknown),
            Expr::MethodCall(_, method, args, _)
                if args.is_empty() && matches!(method.as_str(), "toString" | "toStr") =>
            {
                Some(Type::Str)
            }
            Expr::BinaryOp(lhs, BinOp::Add, rhs, _)
                if matches!(Self::static_add_operand_type(lhs), Some(Type::Str))
                    && matches!(Self::static_add_operand_type(rhs), Some(Type::Str)) =>
            {
                Some(Type::Str)
            }
            Expr::BinaryOp(_, BinOp::Concat, _, _) => Some(Type::Str),
            Expr::MoldInst(name, _, _, _)
                if crate::types::mold_specs::mold_return_tag(name)
                    == Some(crate::codegen::tag_prop::TAG_STR) =>
            {
                Some(Type::Str)
            }
            _ => None,
        }
    }

    pub(super) fn infer_expr_type_recording_only_e1605(&mut self, expr: &Expr) -> Type {
        let error_count = self.errors.len();
        let ty = self.infer_expr_type(expr);
        let mut retained = Vec::new();
        for err in self.errors.drain(error_count..) {
            if err.message.contains("[E1605]") {
                retained.push(err);
            }
        }
        self.errors.extend(retained);
        ty
    }

    /// Infer the type of an expression.
    ///
    /// Wraps `infer_expr_type_inner` and records the inferred type into
    /// `typed_expr_table` so downstream consumers (codegen lowering)
    /// can query the result without re-running inference.
    pub fn infer_expr_type(&mut self, expr: &Expr) -> Type {
        let ty = self.infer_expr_type_inner(expr);
        self.typed_expr_table.record(expr, ty.clone());
        ty
    }

    fn infer_expr_type_with_expected_for_function_arg(
        &mut self,
        expr: &Expr,
        expected: &Type,
    ) -> Type {
        self.infer_expr_type_with_expected_inner(
            expr,
            expected,
            FunctionHintDiagnostic::FunctionArg,
        )
    }

    fn infer_expr_type_with_expected_inner(
        &mut self,
        expr: &Expr,
        expected: &Type,
        diagnostic: FunctionHintDiagnostic,
    ) -> Type {
        if let Type::Function(_, _) = expected {
            if let Expr::Lambda(_, _, _) = expr {
                return self.infer_lambda_with_hint(expr, expected);
            }
            if let Some(fn_ty) = self.infer_named_function_with_hint(expr, expected, diagnostic) {
                return fn_ty;
            }
        }

        let inferred = self.infer_expr_type(expr);
        let hinted = Self::fill_unknowns_from_expected(&inferred, expected);
        if hinted != inferred {
            self.typed_expr_table.record(expr, hinted.clone());
        }
        hinted
    }

    fn infer_named_function_with_hint(
        &mut self,
        expr: &Expr,
        expected: &Type,
        diagnostic: FunctionHintDiagnostic,
    ) -> Option<Type> {
        let (Expr::Ident(name, span), Type::Function(expected_params, expected_ret)) =
            (expr, expected)
        else {
            return None;
        };
        if self.hinted_func_stack.iter().any(|active| active == name) {
            return None;
        }
        if self.visible_binding_shadows_function(name) {
            return None;
        }
        let fd = self.func_defs.get(name)?.clone();
        // Generic named functions use the generic-call substitution path;
        // this expected-hint path is intentionally limited to plain names.
        if !fd.type_params.is_empty() || fd.params.len() != expected_params.len() {
            return None;
        }

        let param_types: Vec<Type> = fd
            .params
            .iter()
            .enumerate()
            .map(|(i, param)| {
                param
                    .type_annotation
                    .as_ref()
                    .map(|ty| self.registry.resolve_type(ty))
                    .unwrap_or_else(|| expected_params.get(i).cloned().unwrap_or(Type::Unknown))
            })
            .collect();

        let ret_annotation = fd
            .return_type
            .as_ref()
            .map(|ty| self.registry.resolve_type(ty));
        let (ret_type, body_failed) = if let Some(ret) = ret_annotation.clone() {
            (ret, false)
        } else {
            self.hinted_func_stack.push(name.clone());
            let inferred = self.infer_function_body_with_param_types(&fd, &param_types);
            self.hinted_func_stack.pop();
            inferred
        };
        if body_failed {
            let code = diagnostic.code();
            self.errors.push(TypeError {
                message: format!(
                    "[{}] Function argument '{}' could not be inferred as {}. \
                     Hint: Add parameter and return annotations, or simplify the function body so it matches the expected function type.",
                    code,
                    name, expected
                ),
                span: span.clone(),
            });
            self.typed_expr_table.record(expr, Type::Unknown);
            return Some(Type::Unknown);
        }
        let hinted_ret = if ret_type == Type::Unknown && ret_annotation.is_some() {
            expected_ret.as_ref().clone()
        } else {
            Self::fill_unknowns_from_expected(&ret_type, expected_ret)
        };

        let fn_ty = Type::Function(param_types, Box::new(hinted_ret));
        self.typed_expr_table.record(expr, fn_ty.clone());

        Some(fn_ty)
    }

    fn infer_function_body_with_param_types(
        &mut self,
        fd: &FuncDef,
        param_types: &[Type],
    ) -> (Type, bool) {
        let Some(Statement::Expr(expr)) = fd.body.last() else {
            return (Type::Unknown, true);
        };
        if fd.body.len() != 1 || !Self::is_narrow_body_inference_expr(expr, &fd.params) {
            return (Type::Unknown, true);
        }

        self.push_scope();
        for (param, ty) in fd.params.iter().zip(param_types.iter()) {
            self.define_var(&param.name, ty.clone());
        }

        let error_len = self.errors.len();
        let table_snapshot = std::mem::take(&mut self.typed_expr_table);
        let ret = self.infer_expr_type(expr);
        self.typed_expr_table = table_snapshot;
        let ret = if self.errors.len() > error_len {
            // The normal FuncDef pass owns body-local diagnostics. This
            // contextual re-inference only decides whether a call-site hint
            // can resolve the function boundary, so collapse internal errors
            // into a single boundary diagnostic at the use site.
            self.errors.truncate(error_len);
            (Type::Unknown, true)
        } else {
            (ret, false)
        };

        self.pop_scope();
        ret
    }

    /// Inner implementation of `infer_expr_type`. Does NOT record into
    /// the typed expression table — recording happens in the public wrapper.
    /// Recursive calls go through the public wrapper so every subexpression
    /// is recorded as well.
    pub(super) fn infer_expr_type_inner(&mut self, expr: &Expr) -> Type {
        match expr {
            Expr::IntLit(_, _) => Type::Int,
            Expr::FloatLit(_, _) => Type::Float,
            Expr::StringLit(_, _) => Type::Str,
            Expr::TemplateLit(template, span) => {
                self.check_comparison_errors_in_template(template, span);
                Type::Str
            }
            Expr::BoolLit(_, _) => Type::Bool,
            Expr::Gorilla(_) => Type::Unit,
            Expr::Placeholder(span) => {
                if !self.in_pipeline {
                    self.errors.push(TypeError {
                        message: "[E1502] `_` is only valid inside a pipeline placeholder position. \
                                  Hint: Use `_` in an expression after `=>`, such as `value => f(_)`."
                            .to_string(),
                        span: span.clone(),
                    });
                }
                Type::Unknown
            }
            Expr::Hole(span) => {
                self.errors.push(TypeError {
                    message: "[E1502] Empty argument slots are only valid inside function calls. \
                              Hint: Use `f(5, )` for partial application."
                        .to_string(),
                    span: span.clone(),
                });
                Type::Unknown
            }
            // B11-6a: TypeLiteral is a compile-time type reference, not a value
            Expr::TypeLiteral(_, _, _) => Type::Str,

            Expr::Ident(name, span) => {
                // Look up variable in scope
                if let Some(ty) = self.lookup_var(name) {
                    ty
                } else if self.func_types.contains_key(name)
                    || self.generic_func_defs.contains_key(name)
                    || self.declared_concrete_type_names.contains(name)
                    || self.registry.mold_defs.contains_key(name)
                    || Self::is_core_builtin_name(name)
                {
                    // Known function/type/mold name used as value reference
                    Type::Unknown
                } else {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1502] Undefined variable '{}'. \
                             Hint: Check the variable name for typos, or define it before use.",
                            name
                        ),
                        span: span.clone(),
                    });
                    Type::Unknown
                }
            }

            Expr::BuchiPack(fields, _) => {
                let field_types: Vec<(String, Type)> = fields
                    .iter()
                    .map(|f| {
                        let ty = self.infer_expr_type(&f.value);
                        (f.name.clone(), ty)
                    })
                    .collect();
                Type::BuchiPack(field_types)
            }

            Expr::ListLit(items, span) => {
                if items.is_empty() {
                    Type::List(Box::new(Type::Unknown))
                } else {
                    let first_type = self.infer_expr_type(&items[0]);
                    // リスト要素の同質性チェック (E0401)
                    // Int/Float 混在は Num に統一
                    let mut unified_type = if Self::is_host_step_type(&first_type) {
                        Self::erased_host_step_type()
                    } else {
                        first_type.clone()
                    };
                    for (i, item) in items.iter().enumerate().skip(1) {
                        let item_type = self.infer_expr_type(item);
                        let unified_is_host_step = Self::is_host_step_type(&unified_type);
                        let item_is_host_step = Self::is_host_step_type(&item_type);
                        if unified_is_host_step || item_is_host_step {
                            if unified_is_host_step && item_is_host_step {
                                unified_type = Self::erased_host_step_type();
                                continue;
                            }
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E3602] HostStep list literals cannot mix HostStep elements with {} at position {}. \
                                     Hint: keep HostCall steps as a list containing only HostStep[...] values.",
                                    item_type, i
                                ),
                                span: span.clone(),
                            });
                            break;
                        }
                        if Self::contains_unknown(&item_type)
                            || Self::contains_unknown(&unified_type)
                        {
                            // Unknown を含む型は型推論未完了 — スキップ
                            // unified_type が Unknown で item_type が具体型なら更新
                            if unified_type == Type::Unknown && item_type != Type::Unknown {
                                unified_type = item_type;
                            }
                            continue;
                        }
                        // Int/Float の混在は Num に統一
                        if (unified_type == Type::Int
                            || unified_type == Type::Float
                            || unified_type == Type::Num)
                            && item_type.is_numeric()
                        {
                            if unified_type != item_type {
                                unified_type = Type::Num;
                            }
                            continue;
                        }
                        // BuchiPack 同士は構造的部分型なので許容
                        if matches!(unified_type, Type::BuchiPack(_))
                            && matches!(item_type, Type::BuchiPack(_))
                        {
                            continue;
                        }
                        // 型不一致
                        if item_type != unified_type {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E0401] リスト要素の型が不一致: 先頭要素は {} ですが、位置 {} の要素は {} です",
                                    first_type, i, item_type
                                ),
                                span: span.clone(),
                            });
                            break;
                        }
                    }
                    Type::List(Box::new(unified_type))
                }
            }

            Expr::BinaryOp(left, op, right, span) => {
                let left_type = self.infer_expr_type(left);
                let right_type = self.infer_expr_type(right);
                // D28B-024: a Type::Named(T) where T is an active generic
                // type parameter constrained by a numeric primitive
                // (`T <= :Num` / `:Int` / `:Float`) should be treated as
                // numeric for arithmetic and ordering. Helper closures
                // capture this judgement uniformly across operator arms.
                let left_is_numeric_var =
                    matches!(&left_type, Type::Named(n) if self.type_param_is_numeric(n));
                let right_is_numeric_var =
                    matches!(&right_type, Type::Named(n) if self.type_param_is_numeric(n));
                let left_is_numeric_ext = left_type.is_numeric() || left_is_numeric_var;
                let right_is_numeric_ext = right_type.is_numeric() || right_is_numeric_var;
                // F56: a sealed carrier (Moltenized/Secret) cannot participate in
                // any binary operation. `+`/concat would leak the value; `==`/`!=`
                // would act as an equality oracle. Comparison must go through
                // ConstantTimeEq[]; consumption through Reveal[].
                let left_sealed =
                    matches!(&left_type, Type::Generic(n, _) if n == "Moltenized" || n == "Secret");
                let right_sealed = matches!(&right_type, Type::Generic(n, _) if n == "Moltenized" || n == "Secret");
                if left_sealed || right_sealed {
                    let carrier = if left_sealed { &left_type } else { &right_type };
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1536] a sealed carrier ({}) cannot be used in a `{:?}` \
                             operation; the value would leak or act as an equality \
                             oracle. Hint: use `ConstantTimeEq[secret, candidate]()` to \
                             compare, or `Reveal[secret, consumer]()` to consume it.",
                            carrier, op
                        ),
                        span: span.clone(),
                    });
                    return match op {
                        BinOp::Eq
                        | BinOp::NotEq
                        | BinOp::Lt
                        | BinOp::Gt
                        | BinOp::GtEq
                        | BinOp::And
                        | BinOp::Or => Type::Bool,
                        _ => Type::Unknown,
                    };
                }
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        if left_is_numeric_ext && right_is_numeric_ext {
                            // D28B-024: when both operands are the SAME
                            // generic numeric type variable, preserve it
                            // (so a body declared `=> :T` type-checks
                            // against the body's tail value of type T).
                            // Mixed `T` + concrete numeric, or two
                            // different numeric type variables, widen to
                            // a concrete numeric primitive (Int / Float)
                            // following the existing precedence rule:
                            // any Float operand widens to Float, else Int.
                            if let (Type::Named(l), Type::Named(r)) = (&left_type, &right_type)
                                && l == r
                                && self.type_param_is_numeric(l)
                            {
                                left_type.clone()
                            } else if matches!(left_type, Type::Float)
                                || matches!(right_type, Type::Float)
                            {
                                Type::Float
                            } else if left_is_numeric_var || right_is_numeric_var {
                                // At least one side is a generic numeric
                                // var; cannot statically pick Int vs Float
                                // without inference at the call site, so
                                // surface as Num and let return-type
                                // compatibility (subtype: Int<:Num,
                                // Float<:Num) absorb the imprecision.
                                Type::Num
                            } else {
                                Type::Int
                            }
                        } else if matches!(op, BinOp::Add)
                            && matches!(left_type, Type::Str)
                            && matches!(right_type, Type::Str)
                        {
                            Type::Str
                        } else if left_type == Type::Unknown || right_type == Type::Unknown {
                            if self.errors.is_empty() {
                                self.errors.push(TypeError {
                                    message: "[E1525] Cannot infer operand type for `+`. Add parameter or expression type annotations.".to_string(),
                                    span: span.clone(),
                                });
                            }
                            Type::Unknown
                        } else {
                            self.errors.push(TypeError {
                                message: format!(
                                    "Cannot apply {:?} to {} and {}",
                                    op, left_type, right_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                    BinOp::Eq | BinOp::NotEq => {
                        // FL-4: Equality operators allow any types but warn on incompatible comparisons
                        self.emit_comparison_mismatch_if_needed(&left_type, op, &right_type, span);
                        Type::Bool
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::GtEq => {
                        // FL-4: Ordering operators require numeric or string operands.
                        //
                        // C18-4: Same-Enum ordering is also allowed. `>=` / `<=` /
                        // `<` / `>` compare the declared ordinal position of the
                        // two variants. Cross-Enum and Enum↔Int ordering stays
                        // rejected with `[E1605]` — use `Ordinal[]` to obtain
                        // the Int explicitly (added in C18-3). The declared
                        // order of an Enum is therefore semantic; see
                        // `docs/guide/01_types.md` for the author contract.
                        self.emit_comparison_mismatch_if_needed(&left_type, op, &right_type, span);
                        Type::Bool
                    }
                    BinOp::And | BinOp::Or => {
                        // FL-4: Logical operators require Bool operands
                        if left_type != Type::Unknown
                            && !Self::contains_unknown(&left_type)
                            && !matches!(left_type, Type::Bool)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1606] Logical operator {:?} requires Bool operands, got {} on left side. \
                                     Hint: Use a boolean expression or comparison.",
                                    op, left_type
                                ),
                                span: span.clone(),
                            });
                        }
                        if right_type != Type::Unknown
                            && !Self::contains_unknown(&right_type)
                            && !matches!(right_type, Type::Bool)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1606] Logical operator {:?} requires Bool operands, got {} on right side. \
                                     Hint: Use a boolean expression or comparison.",
                                    op, right_type
                                ),
                                span: span.clone(),
                            });
                        }
                        Type::Bool
                    }
                    BinOp::Concat => Type::Str,
                }
            }

            Expr::UnaryOp(op, inner, span) => {
                let inner_type = self.infer_expr_type(inner);
                match op {
                    UnaryOp::Neg => {
                        if inner_type.is_numeric() || inner_type == Type::Unknown {
                            inner_type
                        } else {
                            // FL-4: Report non-numeric operand for unary negation
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1607] Unary negation `-` requires a numeric operand, got {}. \
                                     Hint: Use `-` only with Int or Float values.",
                                    inner_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                    UnaryOp::Not => {
                        // FL-4: Not operator requires Bool operand
                        if inner_type != Type::Unknown
                            && !Self::contains_unknown(&inner_type)
                            && !matches!(inner_type, Type::Bool)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1607] Logical not `!` requires a Bool operand, got {}. \
                                     Hint: Use `!` only with boolean expressions.",
                                    inner_type
                                ),
                                span: span.clone(),
                            });
                        }
                        Type::Bool
                    }
                }
            }

            Expr::FuncCall(func, args, span) => {
                // A placeholder-free call that is itself a pipeline stage
                // receives the piped value as an implicit first argument
                // at runtime (`data => f()` runs as `f(data)`). Take
                // (consume) the stage state here so calls nested inside
                // the arguments don't see it; when the injection applies,
                // keep the previous stage's result type so arity and
                // argument-type validation below can cover the injected
                // value.
                let pipeline_stage_type = std::mem::take(&mut self.pipeline_stage_injected_type);
                let injected_first_arg: Option<Type> = if args.iter().any(expr_contains_placeholder)
                {
                    None
                } else {
                    pipeline_stage_type
                };
                // C-5c: Reject old `_` partial application syntax in function call args.
                // Pipeline context (`data => f(_)`) is allowed — `_` refers to pipe value.
                if !self.in_pipeline {
                    for arg in args.iter() {
                        if let Expr::Placeholder(ph_span) = arg {
                            self.errors.push(TypeError {
                                message: "[E1502] Use empty slot syntax `f(5, )` instead of `f(5, _)` for partial application. \
                                     Hint: Remove the `_` and leave the argument position empty.".to_string(),
                                span: ph_span.clone(),
                            });
                        }
                    }
                }
                if !self.in_comparison_error_walk
                    && self.func_call_args_need_comparison_walk(func, args)
                {
                    self.run_comparison_error_walk(func);
                    for arg in args {
                        if !matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                            self.run_comparison_error_walk(arg);
                        }
                    }
                }

                // C-5d: Reject partial application (Placeholder or Hole) in TypeDef/BuchiPack instantiation.
                // TypeDef calls look like FuncCall where callee is an uppercase Ident.
                if let Expr::Ident(callee_name, _) = func.as_ref()
                    && callee_name.chars().next().is_some_and(|c| c.is_uppercase())
                    && !self.func_types.contains_key(callee_name.as_str())
                {
                    // This is likely a TypeDef instantiation
                    for arg in args.iter() {
                        match arg {
                            Expr::Placeholder(ph_span) => {
                                self.errors.push(TypeError {
                                        message: "[E1503] Partial application is not supported for TypeDef/BuchiPack instantiation. \
                                             Hint: Provide all fields explicitly when creating a TypeDef instance.".to_string(),
                                        span: ph_span.clone(),
                                    });
                            }
                            Expr::Hole(h_span) => {
                                self.errors.push(TypeError {
                                        message: "[E1503] Partial application is not supported for TypeDef/BuchiPack instantiation. \
                                             Hint: Provide all fields explicitly when creating a TypeDef instance.".to_string(),
                                        span: h_span.clone(),
                                    });
                            }
                            _ => {}
                        }
                    }
                }

                // Count holes in the argument list
                let hole_count = args.iter().filter(|a| matches!(a, Expr::Hole(_))).count();

                // Try to resolve return type from function name
                if let Expr::Ident(name, _) = func.as_ref() {
                    self.validate_http_serve_protocol_capability(name, args);

                    if let Some(fd) = self.generic_func_defs.get(name).cloned() {
                        let param_patterns: Vec<Type> = fd
                            .params
                            .iter()
                            .map(|param| {
                                param
                                    .type_annotation
                                    .as_ref()
                                    .map(|ty| self.registry.resolve_type(ty))
                                    .unwrap_or(Type::Unknown)
                            })
                            .collect();
                        let ret_pattern = fd
                            .return_type
                            .as_ref()
                            .map(|ty| self.registry.resolve_type(ty))
                            .unwrap_or(Type::Unknown);
                        let generic_names: HashSet<String> =
                            fd.type_params.iter().map(|tp| tp.name.clone()).collect();
                        let mut bindings = HashMap::<String, Type>::new();

                        if args.len() > fd.params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1301] Function '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or update the function signature.",
                                    name,
                                    fd.params.len(),
                                    args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        if hole_count > 0 && args.len() != fd.params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                     Hint: Provide a value or empty slot for each parameter.",
                                    name,
                                    fd.params.len(),
                                    args.len()
                                ),
                                span: span.clone(),
                            });
                        }

                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                continue;
                            }
                            let Some(pattern) = param_patterns.get(i) else {
                                continue;
                            };
                            let expected_hint =
                                self.generic_expected_hint(pattern, &generic_names, &bindings);
                            let actual_ty = self.infer_expr_type_with_expected_for_function_arg(
                                arg,
                                &expected_hint,
                            );
                            if actual_ty == Type::Unknown {
                                continue;
                            }
                            if !self.bind_generic_type_pattern(
                                pattern,
                                &actual_ty,
                                &generic_names,
                                &mut bindings,
                            ) {
                                let expected_ty = self.substitute_generic_type(
                                    pattern,
                                    &generic_names,
                                    &bindings,
                                );
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                         Hint: Pass a value of the correct type, or use an explicit conversion.",
                                        i + 1,
                                        name,
                                        actual_ty,
                                        expected_ty
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }

                        if !self.validate_generic_function_inference(&fd, &bindings, span) {
                            return Type::Unknown;
                        }
                        self.validate_generic_function_bindings(&fd, &bindings, span);
                        let resolved_ret =
                            self.instantiate_generic_type(&ret_pattern, &generic_names, &bindings);

                        if hole_count > 0 {
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, arg)| matches!(arg, Expr::Hole(_)))
                                .map(|(i, _)| {
                                    param_patterns
                                        .get(i)
                                        .map(|pattern| {
                                            self.instantiate_generic_type(
                                                pattern,
                                                &generic_names,
                                                &bindings,
                                            )
                                        })
                                        .unwrap_or(Type::Unknown)
                                })
                                .collect();
                            return Type::Function(hole_param_types, Box::new(resolved_ret));
                        }

                        return resolved_ret;
                    }

                    // First check func_types (registered function return types)
                    if let Some(ret_ty) = self.func_types.get(name).cloned() {
                        if let Some(expected) = self.func_param_counts.get(name).copied() {
                            if args.len() > expected {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1301] Function '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or update the function signature.",
                                        name, expected, args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                            // Slot count (args.len()) must equal arity when holes are present
                            if hole_count > 0 && args.len() != expected {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                         Hint: Provide a value or empty slot for each parameter.",
                                        name, expected, args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                        if self.crypto_sha256_funcs.contains(name) {
                            let injected = injected_first_arg.clone();
                            self.validate_crypto_sha256_call(name, args, span, injected.as_ref());
                        } else if let Some(kind) = self.crypto_funcs.get(name).copied() {
                            let injected = injected_first_arg.clone();
                            self.validate_crypto_call(name, kind, args, span, injected.as_ref());
                        }
                        // E1506: Check argument types against registered parameter types
                        if let Some(param_types) = self.func_param_types.get(name).cloned() {
                            for (i, arg) in args.iter().enumerate() {
                                // Skip holes (partial application) and placeholders
                                if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                    continue;
                                }
                                if let Some(expected_ty) = param_types.get(i) {
                                    if *expected_ty == Type::Unknown {
                                        continue;
                                    }
                                    let actual_ty = self
                                        .infer_expr_type_with_expected_for_function_arg(
                                            arg,
                                            expected_ty,
                                        );
                                    if actual_ty == Type::Unknown {
                                        continue;
                                    }
                                    if Self::contains_unknown(&actual_ty)
                                        && !Self::contains_unknown(expected_ty)
                                    {
                                        self.errors.push(TypeError {
                                            message: format!(
                                                "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                                 Hint: Add annotations or simplify the function body so inference can resolve the argument type.",
                                                i + 1,
                                                name,
                                                actual_ty,
                                                expected_ty
                                            ),
                                            span: span.clone(),
                                        });
                                        continue;
                                    }
                                    if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                        self.errors.push(TypeError {
                                            message: format!(
                                                "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                                 Hint: Pass a value of the correct type, or use an explicit conversion.",
                                                i + 1, name, actual_ty, expected_ty
                                            ),
                                            span: span.clone(),
                                        });
                                    }
                                }
                            }
                        }
                        // If holes present, return a function type (partial application)
                        if hole_count > 0 {
                            // Use registered param types to infer concrete hole types
                            let registered_param_types = self.func_param_types.get(name);
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                .map(|(i, _)| {
                                    registered_param_types
                                        .and_then(|pts| pts.get(i).cloned())
                                        .unwrap_or(Type::Unknown)
                                })
                                .collect();
                            return Type::Function(hole_param_types, Box::new(ret_ty));
                        }
                        return ret_ty;
                    }
                    // Check if variable holds a function type
                    if let Some(Type::Function(params, ret)) = self.lookup_var(name) {
                        if args.len() > params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1301] Function value '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or adjust the function type.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        if hole_count > 0 && args.len() != params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                     Hint: Provide a value or empty slot for each parameter.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        // E1506: Check argument types against function parameter types
                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                continue;
                            }
                            if let Some(expected_ty) = params.get(i) {
                                if *expected_ty == Type::Unknown {
                                    continue;
                                }
                                let actual_ty = self
                                    .infer_expr_type_with_expected_for_function_arg(
                                        arg,
                                        expected_ty,
                                    );
                                if actual_ty == Type::Unknown {
                                    continue;
                                }
                                if Self::contains_unknown(&actual_ty)
                                    && !Self::contains_unknown(expected_ty)
                                {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                             Hint: Add annotations or simplify the function body so inference can resolve the argument type.",
                                            i + 1,
                                            name,
                                            actual_ty,
                                            expected_ty
                                        ),
                                        span: span.clone(),
                                    });
                                    continue;
                                }
                                if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                             Hint: Pass a value of the correct type, or use an explicit conversion.",
                                            i + 1, name, actual_ty, expected_ty
                                        ),
                                        span: span.clone(),
                                    });
                                }
                            }
                        }
                        if hole_count > 0 {
                            // Collect the types of the hole positions from the original param types
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                .map(|(i, _)| params.get(i).cloned().unwrap_or(Type::Unknown))
                                .collect();
                            return Type::Function(hole_param_types, ret);
                        }
                        return *ret;
                    }
                    // D28B-023: variable's declared type is a generic type
                    // parameter whose subtype constraint is a function type
                    // (e.g. `applyFn[T, F <= :T => :T] x: T fn: F = fn(x)`),
                    // so resolving `fn(x)` should dispatch on the constraint's
                    // function shape rather than fall through to [E1510].
                    if let Some(Type::Named(var_name)) = self.lookup_var(name)
                        && let Some(Type::Function(params, ret)) =
                            self.type_param_function_constraint(&var_name)
                    {
                        if args.len() > params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1301] Function value '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or adjust the function type.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        if hole_count > 0 && args.len() != params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                     Hint: Provide a value or empty slot for each parameter.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        // E1506: argument type compatibility against the
                        // declared function-constraint params. Skip when
                        // either side mentions an unresolved type variable
                        // (the body of the enclosing generic function does
                        // not bind T to a concrete type yet).
                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                continue;
                            }
                            let Some(expected_ty) = params.get(i) else {
                                continue;
                            };
                            if *expected_ty == Type::Unknown
                                || self.contains_unresolved_type_var(expected_ty)
                            {
                                continue;
                            }
                            let actual_ty = self.infer_expr_type_with_expected(arg, expected_ty);
                            if actual_ty == Type::Unknown
                                || self.contains_unresolved_type_var(&actual_ty)
                            {
                                continue;
                            }
                            if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                         Hint: Pass a value of the correct type, or use an explicit conversion.",
                                        i + 1, name, actual_ty, expected_ty
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                        if hole_count > 0 {
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                .map(|(i, _)| params.get(i).cloned().unwrap_or(Type::Unknown))
                                .collect();
                            return Type::Function(hole_param_types, ret);
                        }
                        return *ret;
                    }
                    // FL-23: Check if variable is a non-function type being called
                    if let Some(var_ty) = self.lookup_var(name)
                        && !matches!(var_ty, Type::Unknown)
                    {
                        // D28B-023: when the rejected variable's type is an
                        // active generic type parameter (a Named type that
                        // is not registered as a concrete type) but it has
                        // no function-type constraint, the user likely
                        // declared `[T] x: T fn: T` (no constraint) or
                        // `[T, F <= :SomeNonFunction] fn: F`. Append a
                        // targeted hint so the diagnostic guides toward
                        // adding `<= :A => :B` or providing the function
                        // type at the call site.
                        let hint_extra = match &var_ty {
                            Type::Named(n) if self.lookup_active_type_param(n).is_some() => {
                                " For higher-order generic functions, declare the \
                                 callable with a function-type constraint, e.g. \
                                 `[T, F <= :T => :T] x: T fn: F = fn(x) => :T`."
                            }
                            _ => "",
                        };
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1510] Cannot call '{}' of type {} as a function. \
                                 Hint: Only functions and molds can be called.{}",
                                name, var_ty, hint_extra
                            ),
                            span: span.clone(),
                        });
                        return Type::Unknown;
                    }
                    // Check if it's a known builtin
                    // E1507: Builtin arity check
                    // (name, min_args, max_args)
                    let builtin_arity = Self::core_builtin_arity(name.as_str());
                    if let Some((min_args, max_args)) = builtin_arity
                        && (args.len() < min_args || args.len() > max_args)
                    {
                        let arity_desc = if min_args == max_args {
                            format!("{}", min_args)
                        } else {
                            format!("{}-{}", min_args, max_args)
                        };
                        self.errors.push(TypeError {
                                message: format!(
                                    "[E1507] Builtin '{}' takes {} argument(s), got {}. \
                                     Hint: Check the function signature and provide the correct number of arguments.",
                                    name, arity_desc, args.len()
                                ),
                                span: span.clone(),
                            });
                    }
                    // C12-2c: walk builtin args specifically for
                    // `.toString(args)` arity violations so that nested
                    // method calls inside (e.g.) `stdout(n.toString(16))`
                    // are still rejected. Scoped narrowly to `toString`
                    // to avoid changing type-inference semantics for
                    // other builtin arg contexts.
                    //
                    // C19B-002: also walk for FieldAccess nodes whose
                    // receiver type is a pinned Gorillax (or reducible
                    // to one) so that `.__value.<bogus>` chains inside
                    // builtin args surface the same E1602-style rejection
                    // as when assigned to a variable. We deliberately do
                    // NOT recurse into BinaryOp / arithmetic subtrees: that
                    // would retroactively surface pre-existing Str+Int
                    // tolerance in examples like `"foo" + lax.getOrDefault(0)`.
                    if builtin_arity.is_some() && name != "debug" {
                        for arg in args.iter() {
                            if !matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                self.check_tostring_arity_in_expr(arg);
                                self.check_pinned_field_access_in_expr(arg);
                                self.check_str_plus_known_non_str_in_expr(arg);
                            }
                        }
                    }
                    // F56: display / serialization sinks must not receive a sealed
                    // carrier (Moltenized/Secret). This compile-time guard is the
                    // primary defence; the runtime display/JSON paths are the
                    // fail-closed second layer.
                    //
                    // Only the arg forms that can *directly* resolve to a sealed
                    // carrier are inspected. `BinaryOp` operands are deliberately
                    // excluded: a sealed value inside a binary op is already
                    // rejected by [E1536], and re-inferring a `BinaryOp` subtree
                    // here would double-infer method chains (e.g. HashMap
                    // `.get().getOrDefault()`) and disturb the pre-existing
                    // `"x" + lax.getOrDefault(_)` tolerance (see the
                    // `check_str_plus_known_non_str_in_expr` note above). This
                    // mirrors the stdout/stderr arg-walk filter just below.
                    if matches!(
                        name.as_str(),
                        "stdout" | "stderr" | "debug" | "jsonEncode" | "jsonPretty"
                    ) {
                        // Side-effect-free detection only (`first_direct_sealed_operand`
                        // uses `lookup_var` + syntactic mold checks, never
                        // `infer_expr_type`). Re-inferring a sink arg here would
                        // (a) double-infer method chains and disturb the
                        // `"x" + lax.getOrDefault(_)` tolerance, and (b) surface
                        // spurious errors for doc-fragment idents defined in an
                        // earlier block (`jsonEncode(rec)`). A sealed value reached
                        // through a `FuncCall` / `MethodCall` / `FieldAccess` return
                        // is left to the fail-closed runtime (it renders the policy
                        // label / `null`, never plaintext).
                        for arg in args.iter() {
                            let Some(carrier) = self.first_direct_sealed_operand(arg) else {
                                // F56 (lock L0-4c): a sealed carrier nested in a
                                // `@(...)` / `@[...]` literal passed to the sink —
                                // the structure as a whole reaches serialization /
                                // display. Reject it too (runtime fails closed with
                                // `null` / `<policy>` for the nested element).
                                if let Some(nested) = self.first_nested_sealed_in_literal(arg) {
                                    let code =
                                        if matches!(name.as_str(), "jsonEncode" | "jsonPretty") {
                                            "E1534"
                                        } else {
                                            "E1533"
                                        };
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[{}] `{}` cannot receive a structure containing a \
                                             sealed carrier ({}); the secret value would be \
                                             exposed. Hint: project the secret out (e.g. with \
                                             `Redact[secret]()`) before serializing.",
                                            code, name, nested
                                        ),
                                        span: span.clone(),
                                    });
                                }
                                continue;
                            };
                            if matches!(arg, Expr::Ident(_, _) | Expr::MoldInst(_, _, _, _)) {
                                // The arg is *itself* a sealed carrier.
                                let code = if matches!(name.as_str(), "jsonEncode" | "jsonPretty") {
                                    "E1534"
                                } else {
                                    "E1533"
                                };
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[{}] `{}` cannot receive a sealed carrier ({}); the \
                                         secret value would be exposed. Hint: use \
                                         `Redact[secret]()` for a masked string, or a \
                                         secret-aware consumer such as `HmacSha256[]`.",
                                        code, name, carrier
                                    ),
                                    span: span.clone(),
                                });
                            } else {
                                // BinaryOp / UnaryOp arg containing a sealed operand:
                                // the op itself is the leak (`+` concat / `==` oracle).
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1536] a sealed carrier ({}) cannot be used in a binary \
                                         operation; the value would leak or act as an equality \
                                         oracle. Hint: use `ConstantTimeEq[secret, candidate]()` \
                                         to compare, or `Redact[secret]()` to render a mask.",
                                        carrier
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                    }
                    // F56: `assert` observes truthiness / a comparison. A sealed
                    // carrier reaching it — directly or inside the asserted
                    // expression — is an oracle (the pass/fail bit leaks even
                    // though the plaintext never prints), so reject it as the same
                    // [E1536] equality/observation family. Design lock L0-4 lists
                    // `assert` among the error-channel sinks. Side-effect-free
                    // (`first_direct_sealed_operand`) so it preserves the
                    // `"x" + lax.getOrDefault(_)` tolerance.
                    if name == "assert" {
                        for arg in args.iter() {
                            if let Some(carrier) = self.first_direct_sealed_operand(arg) {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1536] a sealed carrier ({}) cannot be observed by \
                                         `assert`; the pass/fail result is an oracle. Hint: assert \
                                         on `Redact[secret]()` or a `ConstantTimeEq[]` result.",
                                        carrier
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                    }
                    if matches!(name.as_str(), "stdout" | "stderr") {
                        for arg in args.iter() {
                            if matches!(
                                arg,
                                Expr::FuncCall(_, _, _)
                                    | Expr::MethodCall(_, _, _, _)
                                    | Expr::MoldInst(_, _, _, _)
                                    | Expr::FieldAccess(_, _, _)
                            ) {
                                let _ = self.infer_expr_type(arg);
                            }
                        }
                    }
                    let base_ty = self
                        .core_builtin_return_type(name.as_str(), args)
                        .unwrap_or(Type::Unknown);
                    if hole_count > 0 {
                        let hole_param_types: Vec<Type> =
                            (0..hole_count).map(|_| Type::Unknown).collect();
                        return Type::Function(hole_param_types, Box::new(base_ty));
                    }
                    base_ty
                } else {
                    // Calling a non-ident expression (e.g. lambda call)
                    let func_type = self.infer_expr_type(func);
                    match func_type {
                        Type::Function(params, ret) => {
                            if args.len() > params.len() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1301] Function call takes at most {} argument(s), got {}. Hint: Remove extra arguments or adjust the callee signature.",
                                        params.len(), args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                            if hole_count > 0 && args.len() != params.len() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1505] Partial application requires exactly {} slot(s) (got {}). \
                                         Hint: Provide a value or empty slot for each parameter.",
                                        params.len(), args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                            for (i, arg) in args.iter().enumerate() {
                                if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                    continue;
                                }
                                let Some(expected_ty) = params.get(i) else {
                                    continue;
                                };
                                if *expected_ty == Type::Unknown {
                                    continue;
                                }
                                let actual_ty = self
                                    .infer_expr_type_with_expected_for_function_arg(
                                        arg,
                                        expected_ty,
                                    );
                                if actual_ty == Type::Unknown {
                                    continue;
                                }
                                if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] Argument {} has type {}, expected {}. \
                                             Hint: Pass a value of the correct type, or use an explicit conversion.",
                                            i + 1,
                                            actual_ty,
                                            expected_ty
                                        ),
                                        span: span.clone(),
                                    });
                                }
                            }
                            if hole_count > 0 {
                                let hole_param_types: Vec<Type> = args
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                    .map(|(i, _)| params.get(i).cloned().unwrap_or(Type::Unknown))
                                    .collect();
                                return Type::Function(hole_param_types, ret);
                            }
                            *ret
                        }
                        Type::Unknown => Type::Unknown,
                        _ => {
                            // FL-23: non-function call
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1510] Cannot call non-function value of type {}. \
                                     Hint: Only functions and molds can be called.",
                                    func_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                }
            }

            Expr::MethodCall(obj, method, args, span) => {
                let obj_type = self.infer_expr_type(obj);
                // F56: a sealed carrier exposes *no* observable methods — the
                // interpreter rejects every `.method()` on a `Moltenized`/`Secret`
                // (see `interpreter/methods.rs`), so reject them at compile time
                // too. Without this, a sealed *receiver* with a plain argument
                // (`secret.contains("x")` / `secret.toString()`) slips past the
                // arg-only guards and the Native polymorphic dispatcher misreads
                // the carrier pack as a list. `.toString()` / `.toStr()` and the
                // membership methods get their specific code; anything else is a
                // display/observation attempt.
                let receiver_sealed = matches!(
                    &obj_type,
                    Type::Generic(n, _) if n == "Secret" || n == "Moltenized"
                );
                if receiver_sealed {
                    let (code, hint) = if matches!(
                        method.as_str(),
                        "contains" | "indexOf" | "lastIndexOf"
                    ) {
                        (
                            "E1536",
                            "compare with `ConstantTimeEq[secret, candidate]()`",
                        )
                    } else {
                        (
                            "E1533",
                            "use `Redact[secret]()` or a secret-aware consumer (`HmacSha256[]` / `ConstantTimeEq[]`)",
                        )
                    };
                    self.errors.push(TypeError {
                        message: format!(
                            "[{}] `.{}()` cannot observe a sealed carrier ({}); the secret value \
                             would be exposed. Hint: {}.",
                            code, method, obj_type, hint
                        ),
                        span: span.clone(),
                    });
                }
                // F56: membership of a sealed carrier (`list.contains(secret)` /
                // `.indexOf(secret)`) is an equality oracle — the match bit would
                // leak whether the secret is present. The runtime is fail-closed
                // (never finds it), but reject at compile time too (lock L0-4
                // collection sink). Compare with `ConstantTimeEq[]` instead.
                // Only when the receiver is *not* itself sealed: a sealed receiver
                // is already rejected above, and firing both guards would emit a
                // duplicate `[E1536]` on the same span (`secret.contains(other)`).
                else if matches!(method.as_str(), "contains" | "indexOf" | "lastIndexOf") {
                    for arg in args {
                        if let Some(carrier) = self.first_direct_sealed_operand(arg) {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1536] `.{}()` cannot test membership of a sealed carrier \
                                     ({}); the match would be an equality oracle. Hint: compare \
                                     with `ConstantTimeEq[secret, candidate]()`.",
                                    method, carrier
                                ),
                                span: span.clone(),
                            });
                        }
                    }
                }
                if !self.in_comparison_error_walk {
                    for arg in args {
                        if !matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                            self.run_comparison_error_walk(arg);
                        }
                    }
                }
                // E1508: Method call argument count and type checking
                self.check_method_args(&obj_type, method, args, span);
                // E34 Phase 1.4 (Lock-C=B full pin): use arg-aware return type
                // inference so chains like `obj.map(fn1).map(fn2)` propagate
                // type info through the Typed HIR.
                self.infer_method_return_type_with_args(&obj_type, method, args)
            }

            Expr::FieldAccess(obj, field, span) => {
                let obj_type = self.infer_expr_type(obj);
                if field.starts_with(RESERVED_INTERNAL_FIELD_PREFIX) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1960] Field '{}' is compiler-internal and cannot be accessed from Taida code. \
                             Hint: use `>=>` / `<=<` to unmold values, `getOrDefault(default)` for Lax values, \
                             or `errorInfo()` for failure details.",
                            field
                        ),
                        span: span.clone(),
                    });
                    return Type::Unknown;
                }
                match &obj_type {
                    Type::BuchiPack(fields) => {
                        if let Some((_, ty)) = fields.iter().find(|(name, _)| name == field) {
                            ty.clone()
                        } else {
                            self.errors.push(TypeError {
                                message: format!(
                                    "Field '{}' does not exist on type {}",
                                    field, obj_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                    Type::Named(type_name) => {
                        if self.registry.enum_defs.contains_key(type_name) && field == "has_value" {
                            return Type::Bool;
                        }
                        // Look up field in registered type definition
                        if let Some(fields) = self.registry.get_type_fields(type_name) {
                            if let Some((_, ty)) = fields.iter().find(|(name, _)| name == field) {
                                ty.clone()
                            } else {
                                // FL-2: Report undefined field access on Named types
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1602] Field '{}' does not exist on type '{}'. \
                                         Hint: Check the type definition for available fields.",
                                        field, type_name
                                    ),
                                    span: span.clone(),
                                });
                                Type::Unknown
                            }
                        } else {
                            Type::Unknown
                        }
                    }
                    Type::Error(error_name) => {
                        if field == "kind" {
                            return Type::Str;
                        }
                        if let Some(fields) = self.registry.get_type_fields(error_name) {
                            if let Some((_, ty)) = fields.iter().find(|(name, _)| name == field) {
                                ty.clone()
                            } else if error_name != "Error"
                                && let Some(base_fields) = self.registry.get_type_fields("Error")
                                && let Some((_, ty)) =
                                    base_fields.iter().find(|(name, _)| name == field)
                            {
                                ty.clone()
                            } else {
                                Type::Unknown
                            }
                        } else {
                            Type::Unknown
                        }
                    }
                    Type::Generic(name, _) if name == "Lax" => match field.as_str() {
                        "has_value" | "isEmpty" => Type::Bool,
                        "hasValue" => {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1602] Field '{}' does not exist on type '{}'. \
                                     Hint: use `has_value` for field access or `hasValue()` for the state-check method.",
                                    field, name
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                        _ => Type::Unknown,
                    },
                    // E32B-018: internal `__*` envelope slots are rejected
                    // above before type-specific dispatch. Public `has_value`
                    // remains available.
                    Type::Generic(name, args)
                        if name == "Gorillax" || name == "RelaxedGorillax" =>
                    {
                        match field.as_str() {
                            "has_value" => Type::Bool,
                            "hasValue" => {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1602] Field '{}' does not exist on type '{}'. \
                                         Hint: use `has_value` for field access or `hasValue()` for the state-check method.",
                                        field, name
                                    ),
                                    span: span.clone(),
                                });
                                Type::Unknown
                            }
                            "throw" => Type::Unknown,
                            _ => {
                                // Only surface an error for fields that are
                                // clearly not Gorillax envelope slots. Unknown
                                // user-level names fall through to Unknown so
                                // we don't regress any callers that treat a
                                // Gorillax as a dyn pack on purpose.
                                Type::Unknown
                            }
                        }
                    }
                    Type::Unknown => Type::Unknown,
                    _ => Type::Unknown,
                }
            }

            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::CondBranch(arms, span) => self.check_cond_branch(arms, span),

            Expr::Pipeline(exprs, _) => {
                // Pipeline: walk all expressions, set in_pipeline for non-first elements.
                //
                // C13-1 / C13B-007: In a pure `=>` pipeline, an intermediate
                // `=> name` step acts as a bind-and-forward: the value of
                // the preceding step is bound to `name` in a scope that
                // covers the remaining pipeline steps. When the intermediate
                // step is an `Expr::Ident(name)` and `name` is NOT already a
                // known function / type / mold / builtin, we register it as
                // a local binding carrying the current step's type rather
                // than reporting `[E1502] Undefined variable`.
                let old_in_pipeline = self.in_pipeline;
                let last_idx = exprs.len().saturating_sub(1);
                // A fresh scope holds any intermediate bind-and-forward bindings.
                self.push_scope();
                let mut result_type = Type::Unknown;
                for (i, pipe_expr) in exprs.iter().enumerate() {
                    if i > 0 {
                        self.in_pipeline = true;
                    }
                    if i > 0
                        && i < last_idx
                        && let Expr::Ident(name, _) = pipe_expr
                        && !self.is_pipeline_callable_ident(name)
                    {
                        // Intermediate bind-and-forward: carry the current
                        // step's type and make `name` visible to later steps.
                        // result_type is unchanged (value passes through).
                        self.define_var(name, result_type.clone());
                        continue;
                    }
                    if i > 0 && matches!(pipe_expr, Expr::FuncCall(..)) {
                        // The runtime hands the piped value to a
                        // placeholder-free stage call as its implicit
                        // first argument — pass the previous stage's
                        // result type to the FuncCall arm (which
                        // takes/consumes it) so arity and argument-type
                        // validation can cover the injected value.
                        self.pipeline_stage_injected_type = Some(result_type.clone());
                    }
                    result_type = self.infer_expr_type(pipe_expr);
                    self.pipeline_stage_injected_type = None;
                }
                self.pop_scope();
                self.in_pipeline = old_in_pipeline;
                result_type
            }

            Expr::MoldInst(name, type_args, fields, mold_span) => {
                if !self.in_comparison_error_walk {
                    for arg in type_args {
                        self.run_comparison_error_walk(arg);
                    }
                    for field in fields {
                        self.run_comparison_error_walk(&field.value);
                    }
                }
                // C-5e: Reject Mold[_]() direct binding outside pipeline.
                // In pipeline (`data => Trim[_]()`), `_` refers to the pipe value — allowed.
                if !self.in_pipeline {
                    for arg in type_args.iter() {
                        if let Expr::Placeholder(ph_span) = arg {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1504] `{}[_]()` cannot be used outside a pipeline. \
                                     The `_` placeholder in mold type arguments is only valid inside a pipeline expression (`data => {}[_]()`). \
                                     Hint: Pass a concrete value to the mold, e.g., `{}[value]()`.",
                                    name, name, name
                                ),
                                span: ph_span.clone(),
                            });
                        }
                    }
                }

                self.validate_custom_mold_inst_bindings(name, type_args, fields, mold_span);
                self.validate_mold_header_constraints(name, type_args, mold_span);
                self.validate_builtin_mold_spec(name, type_args, fields, mold_span);
                // F56: `Str[secret]()` stringifies a sealed carrier — a display
                // sink. Reject at compile time (lock L0-4), matching the runtime
                // policy-label fail-close. Side-effect-free detection only.
                if name == "Str"
                    && let Some(carrier) = type_args
                        .first()
                        .and_then(|a| self.first_direct_sealed_operand(a))
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1533] `Str[]` cannot stringify a sealed carrier ({}); the secret \
                             value would be exposed. Hint: use `Redact[secret]()` for a masked \
                             string.",
                            carrier
                        ),
                        span: mold_span.clone(),
                    });
                }
                match name.as_str() {
                    "HostCapability" => self.infer_host_capability_type(type_args, mold_span),
                    "HostStep" => self.infer_host_step_type(type_args, mold_span),
                    "HostCall" => {
                        self.validate_host_call_descriptor(type_args, mold_span);
                        self.cage_runner_type(expr)
                            .map(|runner| {
                                Type::Generic(
                                    "CageRilla".to_string(),
                                    vec![
                                        Type::Named(runner.branch.label().to_string()),
                                        runner.output,
                                    ],
                                )
                            })
                            .unwrap_or(Type::Unknown)
                    }
                    // JSON[raw, Schema]() returns Lax[Schema].
                    "JSON" => {
                        let schema_ty = type_args
                            .get(1)
                            .map(|arg| self.type_arg_expr_to_type(arg))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Lax".to_string(), vec![schema_ty])
                    }
                    // Async[T] wraps a value. AsyncReject[err]() has no
                    // fulfilled payload, so use the supplied rejection value
                    // type as the best available concrete Async parameter.
                    "Async" => Type::Generic(
                        "Async".to_string(),
                        vec![
                            type_args
                                .first()
                                .map(|a| self.infer_expr_type(a))
                                .unwrap_or(Type::Unknown),
                        ],
                    ),
                    "AsyncReject" => Type::Generic(
                        "Async".to_string(),
                        vec![
                            type_args
                                .first()
                                .map(|a| self.infer_expr_type(a))
                                .unwrap_or(Type::Unknown),
                        ],
                    ),
                    "AsyncTask" => {
                        let task_ty = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        if let Some(task_arg) = type_args.first() {
                            self.validate_async_task_worker_body(task_arg);
                        }
                        let inner = match task_ty {
                            Type::Function(params, ret) if params.is_empty() => *ret,
                            Type::Function(params, ret) => {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] `AsyncTask[_ = expr]()` requires a zero-argument thunk, got a function with {} parameter(s).",
                                        params.len()
                                    ),
                                    span: mold_span.clone(),
                                });
                                *ret
                            }
                            Type::Unknown => Type::Unknown,
                            other => {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] `AsyncTask[_ = expr]()` requires a zero-argument thunk, got {}.",
                                        other
                                    ),
                                    span: mold_span.clone(),
                                });
                                Type::Unknown
                            }
                        };
                        Type::Generic("AsyncTask".to_string(), vec![inner])
                    }
                    // Cancel[async]() returns Async[T] (or Async[Unknown] fallback)
                    "Cancel" => {
                        if type_args.len() != 1 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] `Cancel[async]()` requires exactly 1 type argument, got {}. \
                                     Hint: pass a single Async value, e.g. `Cancel[asyncTask]()`.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .map(|t| match t {
                                Type::Generic(name, args) if name == "Async" => {
                                    args.first().cloned().unwrap_or(Type::Unknown)
                                }
                                other => other,
                            })
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Async".to_string(), vec![inner])
                    }
                    // E34B-018 (Codex review #15 follow-up): All / Race /
                    // Timeout had no checker-side arity validation and no
                    // dedicated return-type pin, so `All[xs, extra]()` and
                    // `Timeout[async]()` (missing ms) silently passed type
                    // checking. Pin the signatures here to align with
                    // `src/interpreter/mold.rs` (4-backend parity).
                    //
                    // Runtime contracts:
                    //   All[asyncList]() -> Async[List[T]]    (1 arg)
                    //   Race[asyncList]() -> Async[T]         (1 arg)
                    //   Timeout[async, ms]() -> Async[T]      (2 args)
                    "All" => {
                        if type_args.len() != 1 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] `All[asyncList]()` requires exactly 1 type \
                                     argument, got {}. Hint: pass a single list of Async \
                                     values, e.g. `All[@[a1, a2]]()`.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .map(|t| match t {
                                Type::List(elem) => match *elem {
                                    Type::Generic(ref name, ref args) if name == "Async" => {
                                        Type::List(Box::new(
                                            args.first().cloned().unwrap_or(Type::Unknown),
                                        ))
                                    }
                                    other => Type::List(Box::new(other)),
                                },
                                _ => Type::List(Box::new(Type::Unknown)),
                            })
                            .unwrap_or(Type::List(Box::new(Type::Unknown)));
                        Type::Generic("Async".to_string(), vec![inner])
                    }
                    "Par" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .map(|t| match t {
                                Type::List(elem) => match *elem {
                                    Type::Generic(ref name, ref args) if name == "AsyncTask" => {
                                        Type::List(Box::new(
                                            args.first().cloned().unwrap_or(Type::Unknown),
                                        ))
                                    }
                                    Type::Unknown => Type::List(Box::new(Type::Unknown)),
                                    other => {
                                        self.errors.push(TypeError {
                                            message: format!(
                                                "[E1506] `Par[jobs]()` expects a list of AsyncTask values, got list element type {}.",
                                                other
                                            ),
                                            span: mold_span.clone(),
                                        });
                                        Type::List(Box::new(Type::Unknown))
                                    }
                                },
                                Type::Unknown => Type::List(Box::new(Type::Unknown)),
                                other => {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] `Par[jobs]()` expects a list of AsyncTask values, got {}.",
                                            other
                                        ),
                                        span: mold_span.clone(),
                                    });
                                    Type::List(Box::new(Type::Unknown))
                                }
                            })
                            .unwrap_or(Type::List(Box::new(Type::Unknown)));
                        Type::Generic("Async".to_string(), vec![inner])
                    }
                    "ParMap" => {
                        if let Some(mapper_arg) = type_args.get(1) {
                            self.validate_async_task_worker_body(mapper_arg);
                        }
                        let list_ty = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        let elem_ty = match list_ty {
                            Type::List(elem) => *elem,
                            Type::Unknown => Type::Unknown,
                            other => {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] `ParMap[list, fn]()` expects a list as its first argument, got {}.",
                                        other
                                    ),
                                    span: mold_span.clone(),
                                });
                                Type::Unknown
                            }
                        };
                        let ret_ty = type_args
                            .get(1)
                            .map(|a| self.infer_expr_type(a))
                            .map(|fn_ty| match fn_ty {
                                Type::Function(params, ret) if params.len() == 1 => {
                                    if let Some(param_ty) = params.first()
                                        && !matches!(&elem_ty, Type::Unknown | Type::Any)
                                        && !matches!(param_ty, Type::Unknown | Type::Any)
                                        && !self.registry.is_subtype_of(&elem_ty, param_ty)
                                    {
                                        self.errors.push(TypeError {
                                            message: format!(
                                                "[E1506] `ParMap[list, fn]()` function parameter has type {}, but list elements are {}.",
                                                param_ty, elem_ty
                                            ),
                                            span: mold_span.clone(),
                                        });
                                    }
                                    *ret
                                }
                                Type::Function(params, ret) => {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] `ParMap[list, fn]()` requires a one-argument function, got {} parameter(s).",
                                            params.len()
                                        ),
                                        span: mold_span.clone(),
                                    });
                                    *ret
                                }
                                Type::Unknown => Type::Unknown,
                                other => {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] `ParMap[list, fn]()` expects a function as its second argument, got {}.",
                                            other
                                        ),
                                        span: mold_span.clone(),
                                    });
                                    Type::Unknown
                                }
                            })
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Async".to_string(), vec![Type::List(Box::new(ret_ty))])
                    }
                    "Race" => {
                        if type_args.len() != 1 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] `Race[asyncList]()` requires exactly 1 type \
                                     argument, got {}. Hint: pass a single list of Async \
                                     values, e.g. `Race[@[a1, a2]]()`.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .map(|t| match t {
                                Type::List(elem) => match *elem {
                                    Type::Generic(ref name, ref args) if name == "Async" => {
                                        args.first().cloned().unwrap_or(Type::Unknown)
                                    }
                                    other => other,
                                },
                                _ => Type::Unknown,
                            })
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Async".to_string(), vec![inner])
                    }
                    "Timeout" => {
                        if type_args.len() != 2 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] `Timeout[async, ms]()` requires exactly 2 type \
                                     arguments, got {}. Hint: pass an Async value and a \
                                     numeric timeout (ms), e.g. `Timeout[asyncTask, 5000]()`.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        if let Some(ms_arg) = type_args.get(1) {
                            let ms_ty = self.infer_expr_type(ms_arg);
                            if !matches!(ms_ty, Type::Unknown) && !ms_ty.is_numeric() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] `Timeout[async, ms]()`: second argument has \
                                         type {}, expected a numeric (Int / Float / Num) \
                                         timeout in milliseconds.",
                                        ms_ty
                                    ),
                                    span: mold_span.clone(),
                                });
                            }
                        }
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .map(|t| match t {
                                Type::Generic(name, args) if name == "Async" => {
                                    args.first().cloned().unwrap_or(Type::Unknown)
                                }
                                other => other,
                            })
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Async".to_string(), vec![inner])
                    }
                    // Result[value]() / Result[value](throw <= ErrorVal) returns Result[T, P].
                    // E34 Phase 1.4 (Lock-C=B): pin error type P from the
                    // `throw <= ...` field when present so chains like
                    // `r.flatMap(...)` can enforce Result[U, P] preservation
                    // (方針 A: error type 保存 strict).
                    "Result" => {
                        // Pin upper arity. `Result[value, predicate?]()` is the
                        // public shape; the runtime reads `type_args[0]` /
                        // `type_args[1]` only. Anything past index 1 was
                        // silently dropped at the front gate.
                        if type_args.len() > 2 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] `Result[value, predicate?]()` accepts at most \
                                     2 type arguments, got {}. Hint: extra information \
                                     belongs in the `(throw <= ErrorVal)` field block.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        let success_ty = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        let error_ty = fields
                            .iter()
                            .find(|f| f.name == "throw")
                            .map(|f| self.infer_expr_type(&f.value))
                            .unwrap_or(Type::Named("ErrorInfo".to_string()));
                        Type::Generic("Result".to_string(), vec![success_ty, error_ty])
                    }
                    // Lax[value]() returns Lax[T]
                    "Lax" => {
                        // Same silent-drop gap as `Result` — any
                        // `type_args[1..]` were ignored, masking simple
                        // typos like `Lax[1, 2, 3]()`.
                        if type_args.len() > 1 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] `Lax[value]()` accepts at most 1 type \
                                     argument, got {}. Hint: wrap a single value, e.g. \
                                     `Lax[42]()`.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Lax".to_string(), vec![inner])
                    }
                    // F56: Moltenize[v]() -> Moltenized[T], MoltenizeSecret[v]() -> Secret[T].
                    // The inner type T is preserved as the reveal type; the carrier
                    // itself is opaque (sink matrix rejects display / JSON / unmold).
                    "Moltenize" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Moltenized".to_string(), vec![inner])
                    }
                    "MoltenizeSecret" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Secret".to_string(), vec![inner])
                    }
                    // Redact[secret]() -> Str (fixed "***").
                    "Redact" => Type::Str,
                    // F56 Phase 2: source-side secret producers. The read value is
                    // sealed at the boundary; the failure channel is `Lax` (sync
                    // env) or `Async[Lax]` (input / file I/O).
                    "MoltenizeSecretFromEnv" => Type::Generic(
                        "Lax".to_string(),
                        vec![Type::Generic("Secret".to_string(), vec![Type::Str])],
                    ),
                    "MoltenizeSecretFromInput" => Type::Generic(
                        "Async".to_string(),
                        vec![Type::Generic(
                            "Lax".to_string(),
                            vec![Type::Generic("Secret".to_string(), vec![Type::Str])],
                        )],
                    ),
                    "MoltenizeSecretFromFile" => Type::Generic(
                        "Async".to_string(),
                        vec![Type::Generic(
                            "Lax".to_string(),
                            vec![Type::Generic("Secret".to_string(), vec![Type::Bytes])],
                        )],
                    ),
                    // F56 Phase 4: secret-aware consumers. The result is public —
                    // the MAC (`Str`) / comparison verdict (`Bool`) is not a
                    // secret, so it leaves the sealed-carrier type behind.
                    "HmacSha256" => Type::Str,
                    "ConstantTimeEq" => Type::Bool,
                    // F56 Phase 4: Reveal[secret, consumer]() returns the
                    // consumer's return type `R` (the plaintext `T` is consumed
                    // inside the function and does not appear in the result type).
                    "Reveal" => type_args
                        .get(1)
                        .map(|a| self.infer_expr_type(a))
                        .and_then(|t| match t {
                            Type::Function(_, ret) => Some(*ret),
                            _ => None,
                        })
                        .unwrap_or(Type::Unknown),
                    // Div[x, y]() and Mod[x, y]() return Lax[Num]
                    "Div" | "Mod" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Num);
                        let inner = if inner.is_numeric() { inner } else { Type::Num };
                        Type::Generic("Lax".to_string(), vec![inner])
                    }
                    // Type conversion molds: Str[x]() -> Lax[Str], Int[x]() -> Lax[Int], etc.
                    "Str" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "Int" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    "Float" => Type::Generic("Lax".to_string(), vec![Type::Float]),
                    "Bool" => Type::Generic("Lax".to_string(), vec![Type::Bool]),
                    "Bytes" => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
                    "UInt8" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    "Char" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "CodePoint" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    "Utf8Encode" => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
                    "Utf8Decode" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "U16BE" | "U16LE" | "U32BE" | "U32LE" => {
                        Type::Generic("Lax".to_string(), vec![Type::Bytes])
                    }
                    "U16BEDecode" | "U16LEDecode" | "U32BEDecode" | "U32LEDecode" => {
                        Type::Generic("Lax".to_string(), vec![Type::Int])
                    }
                    "BytesCursor" => Type::BuchiPack(vec![
                        ("bytes".to_string(), Type::Bytes),
                        ("offset".to_string(), Type::Int),
                        ("length".to_string(), Type::Int),
                    ]),
                    "BytesCursorRemaining" => Type::Int,
                    "BytesCursorTake" => Type::Generic(
                        "Lax".to_string(),
                        vec![Type::BuchiPack(vec![
                            ("value".to_string(), Type::Bytes),
                            (
                                "cursor".to_string(),
                                Type::BuchiPack(vec![
                                    ("bytes".to_string(), Type::Bytes),
                                    ("offset".to_string(), Type::Int),
                                    ("length".to_string(), Type::Int),
                                ]),
                            ),
                        ])],
                    ),
                    "BytesCursorU8" => Type::Generic(
                        "Lax".to_string(),
                        vec![Type::BuchiPack(vec![
                            ("value".to_string(), Type::Int),
                            (
                                "cursor".to_string(),
                                Type::BuchiPack(vec![
                                    ("bytes".to_string(), Type::Bytes),
                                    ("offset".to_string(), Type::Int),
                                    ("length".to_string(), Type::Int),
                                ]),
                            ),
                        ])],
                    ),
                    "BitAnd" | "BitOr" | "BitXor" | "BitNot" => Type::Int,
                    "ShiftL" | "ShiftR" | "ShiftRU" => {
                        Type::Generic("Lax".to_string(), vec![Type::Int])
                    }
                    "ToRadix" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "ByteSet" => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
                    "BytesToList" => Type::List(Box::new(Type::Int)),
                    // HOF molds return the appropriate type
                    // If input is Stream[T], output is also Stream[U]
                    "Map" | "Filter" | "Sort" | "Unique" | "Flatten" | "Reverse" | "Take"
                    | "TakeWhile" | "Drop" | "DropWhile" | "Append" | "Prepend" | "Zip"
                    | "Enumerate" => {
                        // These return a list or stream (same or transformed)
                        if let Some(first_arg) = type_args.first() {
                            let arg_type = self.infer_expr_type(first_arg);
                            if matches!(arg_type, Type::Generic(ref n, _) if n == "Stream") {
                                // Stream input: return Stream (lazy transform)
                                arg_type
                            } else if matches!(arg_type, Type::List(_)) {
                                arg_type
                            } else {
                                Type::List(Box::new(Type::Unknown))
                            }
                        } else {
                            Type::List(Box::new(Type::Unknown))
                        }
                    }
                    // Stream[value]() → Stream[T]
                    "Stream" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Stream".to_string(), vec![inner])
                    }
                    // StreamFrom[list]() → Stream[T]
                    "StreamFrom" => {
                        if let Some(first_arg) = type_args.first() {
                            let arg_type = self.infer_expr_type(first_arg);
                            if let Type::List(inner) = arg_type {
                                Type::Generic("Stream".to_string(), vec![*inner])
                            } else {
                                Type::Generic("Stream".to_string(), vec![Type::Unknown])
                            }
                        } else {
                            Type::Generic("Stream".to_string(), vec![Type::Unknown])
                        }
                    }
                    "Fold" | "Foldr" | "Reduce" => {
                        // Returns the accumulator type (first arg)
                        if let Some(first_arg) = type_args.first() {
                            self.infer_expr_type(first_arg)
                        } else {
                            Type::Unknown
                        }
                    }
                    // String / Bytes operation molds
                    // B11-5d: If[cond, then, else]() returns the type of the then branch
                    // B11B-014: check branch type compatibility (same as | |> E1603)
                    "If" => {
                        if type_args.len() >= 3 {
                            let then_ty = self.infer_expr_type(&type_args[1]);
                            let else_ty = self.infer_expr_type(&type_args[2]);
                            if !(then_ty == Type::Unknown
                                || else_ty == Type::Unknown
                                || Self::contains_unknown(&then_ty)
                                || Self::contains_unknown(&else_ty)
                                || self.registry.is_subtype_of(&else_ty, &then_ty)
                                || then_ty.is_numeric() && else_ty.is_numeric())
                            {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1603] Condition branch type mismatch: then branch returns {}, but else branch returns {}. \
                                         Hint: Both branches of If[] should return the same type.",
                                        then_ty, else_ty
                                    ),
                                    span: mold_span.clone(),
                                });
                            }
                            then_ty
                        } else if type_args.len() >= 2 {
                            self.infer_expr_type(&type_args[1])
                        } else {
                            Type::Unknown
                        }
                    }
                    // B11-6e: TypeIs[value, :TypeName]() → Bool
                    "TypeIs" => Type::Bool,
                    // B11-6e: TypeExtends[:TypeA, :TypeB]() → Bool
                    // Note: E1613 (variant rejection) is checked by
                    // check_mold_errors_in_expr(), not here, to ensure it
                    // fires regardless of expression context.
                    "TypeExtends" => Type::Bool,
                    "Exists" => Self::result_type(Type::Bool),
                    "TypeName" => {
                        if type_args.len() != 1 {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1507] TypeName requires exactly 1 argument: TypeName[value](), got {}.",
                                    type_args.len()
                                ),
                                span: mold_span.clone(),
                            });
                        }
                        Type::Str
                    }
                    "JSGet" | "JSCall" | "JSCallAsync" | "JSNew" | "JSSet" | "JSBind"
                    | "JSSpread" | "JSRilla" | "FileRilla" | "BuildRilla" | "CageRilla" => self
                        .cage_runner_type(expr)
                        .map(|runner| {
                            Type::Generic(
                                "CageRilla".to_string(),
                                vec![
                                    Type::Named(runner.branch.label().to_string()),
                                    runner.output,
                                ],
                            )
                        })
                        .unwrap_or(Type::Unknown),
                    "Upper" | "Lower" | "Trim" | "Replace" | "Repeat" | "Pad" => Type::Str,
                    // C26B-018 (B)(C): byte-level primitive + single-alloc repeat/join
                    "ByteSlice" | "StringRepeatJoin" => Type::Str,
                    "ByteLength" => Type::Int,
                    "ByteAt" => Type::Generic("Lax".into(), vec![Type::Int]),
                    "CharAt" => Type::Generic("Lax".into(), vec![Type::Str]),
                    "Slice" => {
                        if let Some(first_arg) = type_args.first() {
                            let t = self.infer_expr_type(first_arg);
                            if t == Type::Bytes {
                                Type::Bytes
                            } else {
                                Type::Str
                            }
                        } else {
                            Type::Str
                        }
                    }
                    "Split" | "Chars" => Type::List(Box::new(Type::Str)),
                    // Number operation molds
                    "Abs" | "Clamp" => {
                        if let Some(first_arg) = type_args.first() {
                            let t = self.infer_expr_type(first_arg);
                            if t.is_numeric() { t } else { Type::Num }
                        } else {
                            Type::Num
                        }
                    }
                    "Floor" | "Ceil" | "Round" | "Truncate" => Type::Int,
                    "ToFixed" => Type::Str,
                    // List/Bytes operation molds
                    "Concat" => {
                        if let Some(first_arg) = type_args.first() {
                            let t = self.infer_expr_type(first_arg);
                            if t == Type::Bytes {
                                Type::Bytes
                            } else if matches!(t, Type::List(_)) || t == Type::Unknown {
                                t
                            } else {
                                Type::List(Box::new(Type::Unknown))
                            }
                        } else {
                            Type::List(Box::new(Type::Unknown))
                        }
                    }
                    "Join" => Type::Str,
                    "Sum" => Type::Num,
                    "Find" => Type::Generic("Lax".to_string(), vec![Type::Unknown]),
                    "FindIndex" | "Count" => Type::Int,
                    // E32B-022 (Lock-N): Lax[Int]-returning replacement for
                    // the legacy `-1`-sentinel `FindIndex`.
                    "FindIndexLax" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    // Gorillax[value]() returns Gorillax[T]
                    "Gorillax" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Gorillax".to_string(), vec![inner])
                    }
                    // Molten[]() returns Molten (no type arguments allowed)
                    "Molten" => Type::Molten,
                    // Cage[subject, runner] where runner <: CageRilla[Branch, Out].
                    "Cage" => {
                        let Some(subject) = type_args.first() else {
                            self.push_cage_error(
                                "[E1517]",
                                mold_span,
                                "[E1517] Cage requires a subject and runner: `Cage[subject, runner]()`."
                                    .to_string(),
                            );
                            return Type::Generic("Gorillax".to_string(), vec![Type::Unknown]);
                        };
                        let subject_type = self.infer_expr_type(subject);

                        let Some(runner_expr) = type_args.get(1) else {
                            self.push_cage_error(
                                "[E1517]",
                                mold_span,
                                "[E1517] Cage requires a runner descriptor: `Cage[subject, runner]()`."
                                    .to_string(),
                            );
                            return Type::Generic("Gorillax".to_string(), vec![Type::Unknown]);
                        };
                        let runner = self.validate_cage_runner_expr(runner_expr, mold_span);
                        if let Some(runner) = runner {
                            if runner.branch == CageBranch::Host {
                                if !Self::is_host_capability_type(&subject_type)
                                    && subject_type != Type::Unknown
                                {
                                    self.push_cage_error(
                                        "[E1517]",
                                        subject.span(),
                                        format!(
                                            "[E1517] Host Cage subject must be HostCapability, got {}. \
                                             Hint: construct the subject with `HostCapability[name, kind]()`.",
                                            subject_type
                                        ),
                                    );
                                }
                                return Type::Generic("Async".to_string(), vec![runner.output]);
                            }

                            if Self::is_hammer_cage_boundary_expr(subject) {
                                self.push_cage_error(
                                    "[E1518]",
                                    subject.span(),
                                    "[E1518] JSON/Hammer schema casts must not be used as Cage subjects. \
                                     Hint: keep `JSON[raw, Schema]()` on its `Lax[T]` path."
                                        .to_string(),
                                );
                            } else if subject_type != Type::Molten && subject_type != Type::Unknown
                            {
                                self.push_cage_error(
                                    "[E1517]",
                                    subject.span(),
                                    format!(
                                        "[E1517] Cage subject must carry a resolved Molten branch, got {}. \
                                         Hint: pass an external Molten value such as an `npm:` import.",
                                        subject_type
                                    ),
                                );
                            }

                            let subject_branch = self.molten_branch_for_expr(subject);
                            if subject_type == Type::Molten && subject_branch.is_none() {
                                self.push_cage_error(
                                    "[E1517]",
                                    subject.span(),
                                    "[E1517] Cage subject branch is unresolved. \
                                     Hint: use a Molten value whose source fixes the branch, such as an `npm:` import for JS."
                                        .to_string(),
                                );
                            }

                            if let Some(subject_branch) = subject_branch
                                && subject_branch != runner.branch
                            {
                                self.push_cage_error(
                                    "[E1512]",
                                    runner_expr.span(),
                                    format!(
                                        "[E1512] Cage branch mismatch: subject is {}, runner is {}. \
                                         Hint: choose a runner descriptor from the matching CageRilla family.",
                                        subject_branch.label(),
                                        runner.branch.label()
                                    ),
                                );
                            }
                            return if runner.async_boundary {
                                Type::Generic("Async".to_string(), vec![runner.output])
                            } else {
                                Type::Generic("Gorillax".to_string(), vec![runner.output])
                            };
                        }
                        Type::Generic("Gorillax".to_string(), vec![Type::Unknown])
                    }
                    _ => {
                        if matches!(
                            name.as_str(),
                            "SpanEquals" | "SpanStartsWith" | "SpanContains"
                        ) {
                            return Type::Bool;
                        }
                        // Look up in mold definitions
                        if self.registry.mold_defs.contains_key(name) {
                            Type::Named(name.clone())
                        } else if self.generic_func_defs.contains_key(name)
                            || self.func_types.contains_key(name)
                            || matches!(self.lookup_var(name), Some(Type::Function(_, _)))
                        {
                            // C20B-014 (ROOT-17) + C20B-016 (ROOT-19):
                            // user-defined function called via mold syntax
                            // `Fn[args]()`. Pre-C20B-016 this branch only
                            // rejected named fields and returned the raw
                            // function return type — arity, type-mismatch,
                            // partial-application and generic-inference
                            // validation were silently skipped, so
                            // `add[1, "x"]()` passed `taida check` while
                            // `add(1, "x")` correctly surfaced `[E1506]`.
                            //
                            // Post-fix: reject named fields first, then
                            // synthesize the equivalent `FuncCall` and
                            // delegate to the normal function-call path.
                            // This is the exact same AST shape the parser
                            // would have produced for `Fn(args)`, so every
                            // downstream rule (generic-func E1301 / E1506 /
                            // E1505, non-generic E1301 / E1506, function
                            // value E1301 / E1506 / E1505) fires uniformly.
                            if !fields.is_empty() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1511] User-defined function '{}' called via mold syntax \
                                         cannot accept named fields '()'. \
                                         Pass arguments positionally: {}[arg1, arg2]() or {}(arg1, arg2).",
                                        name, name, name
                                    ),
                                    span: mold_span.clone(),
                                });
                            }
                            // Synthesize `name(type_args)` with the mold
                            // span and recurse. The callee span is the
                            // `mold_span` itself; positional args are the
                            // `type_args` list (which for `Fn[a, b]()` are
                            // the runtime values, cf. lower/molds_inst.rs).
                            let synth_callee = Expr::Ident(name.clone(), mold_span.clone());
                            let synth_call = Expr::FuncCall(
                                Box::new(synth_callee),
                                type_args.clone(),
                                mold_span.clone(),
                            );
                            self.infer_expr_type(&synth_call)
                        } else if let Some(spec) = crate::types::mold_specs::lookup_mold_spec(name)
                        {
                            match spec.return_kind {
                                crate::types::mold_specs::MoldReturnKind::Int => Type::Int,
                                crate::types::mold_specs::MoldReturnKind::Float => Type::Float,
                                crate::types::mold_specs::MoldReturnKind::Bool => Type::Bool,
                                crate::types::mold_specs::MoldReturnKind::Str => Type::Str,
                                crate::types::mold_specs::MoldReturnKind::List => {
                                    Type::List(Box::new(Type::Unknown))
                                }
                                crate::types::mold_specs::MoldReturnKind::Pack
                                | crate::types::mold_specs::MoldReturnKind::Dynamic => {
                                    Type::Unknown
                                }
                            }
                        } else if matches!(self.lookup_var(name), Some(Type::Unknown)) {
                            Type::Unknown
                        } else if self.mold_field_defs.contains_key(name)
                            || self.registry.type_defs.contains_key(name)
                            || self.registry.enum_defs.contains_key(name)
                        {
                            Type::Named(name.clone())
                        } else {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1530] Unknown mold '{}'. Hint: Define the mold/type before use or call a function with `{}(...)` syntax.",
                                    name, name
                                ),
                                span: mold_span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                }
            }

            Expr::Unmold(inner, span) => {
                // Unmolding a Mold[T] returns T
                let inner_type = self.infer_expr_type(inner);
                // F56: a sealed carrier (Moltenized/Secret) must never be unmolded
                // directly — that is exactly the leak the carrier exists to prevent.
                // The compile-time guard is the primary defence; every backend
                // runtime also fails closed (`>=>` / `<=<` throws on a sealed value).
                self.reject_sealed_carrier_unmold(&inner_type, span);
                match &inner_type {
                    Type::Generic(name, args) => {
                        match name.as_str() {
                            "Lax" | "Result" | "Async" => {
                                // Return the first type argument (the wrapped value type)
                                args.first().cloned().unwrap_or(Type::Unknown)
                            }
                            "Stream" => {
                                // Stream[T] unmolds to @[T] (List)
                                let inner = args.first().cloned().unwrap_or(Type::Unknown);
                                Type::List(Box::new(inner))
                            }
                            _ => Type::Unknown,
                        }
                    }
                    _ => Type::Unknown,
                }
            }

            Expr::Lambda(params, body, _span) => {
                let param_types: Vec<Type> = params
                    .iter()
                    .map(|p| {
                        p.type_annotation
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                            .unwrap_or(Type::Unknown)
                    })
                    .collect();
                // Push scope with lambda params so body references don't trigger E1502
                self.push_scope();
                for (i, p) in params.iter().enumerate() {
                    self.define_var(
                        &p.name,
                        param_types.get(i).cloned().unwrap_or(Type::Unknown),
                    );
                }
                // Try to infer return type from the body expression
                let ret_type = self.infer_expr_type(body);
                self.pop_scope();
                for (idx, param_ty) in param_types.iter().enumerate() {
                    if Self::contains_unknown(param_ty) {
                        let param_name = params
                            .get(idx)
                            .map(|param| param.name.as_str())
                            .unwrap_or("<unknown>");
                        let span = params
                            .get(idx)
                            .map(|param| param.span.clone())
                            .unwrap_or_else(|| body.span().clone());
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1527] Lambda parameter '{}' has no inferred type. Add `{}: Type` or use the lambda where a function type is expected.",
                                param_name, param_name
                            ),
                            span,
                        });
                    }
                }
                if Self::contains_unknown(&ret_type) {
                    self.errors.push(TypeError {
                        message:
                            "[E1525] Lambda return type could not be inferred from its body. Add parameter annotations or use the lambda where a function type is expected."
                                .to_string(),
                        span: body.span().clone(),
                    });
                }
                Type::Function(param_types, Box::new(ret_type))
            }

            Expr::EnumVariant(enum_name, variant_name, span) => {
                if !self.registry.is_enum_type(enum_name) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1608] Unknown enum type '{}'. Hint: Define `Enum => {} = ...` before using {}:{}().",
                            enum_name, enum_name, enum_name, variant_name
                        ),
                        span: span.clone(),
                    });
                    Type::Unknown
                } else if self
                    .registry
                    .get_enum_variant_ordinal(enum_name, variant_name)
                    .is_none()
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1608] Unknown enum variant '{}:{}()'. Hint: Use one of the variants declared on '{}'.",
                            enum_name, variant_name, enum_name
                        ),
                        span: span.clone(),
                    });
                    Type::Unknown
                } else {
                    Type::Named(enum_name.clone())
                }
            }

            Expr::TypeInst(name, fields, span) => {
                self.validate_type_inst_constructor(name, fields, span);
                Type::Named(name.clone())
            }
            Expr::Throw(inner, span) => {
                let inner_ty = self.infer_expr_type(inner);
                let is_error = match &inner_ty {
                    Type::Error(_) => true,
                    Type::Named(name) => self.registry.is_error_type(name),
                    Type::Unknown | Type::Any => true,
                    _ => false,
                };
                if !is_error {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1531] `.throw()` requires an Error value, got {}. \
                             Hint: construct an Error-derived value before throwing.",
                            inner_ty
                        ),
                        span: span.clone(),
                    });
                }
                Type::Unknown
            }
        }
    }

    /// Infer the type of an arm's result. The result is:
    /// - `Statement::Expr(e)` → the inferred type of `e`
    /// - `Statement::Assignment(_)` / `UnmoldForward(_)` / `UnmoldBackward(_)`
    /// → the registered type of the bound target (already recorded by
    /// the preceding `check_statement` loop).
    /// - Anything else (definitions, imports, …) → `Type::Unknown`.
    ///
    /// Must be called *after* `check_statement` has processed the arm
    /// body so that tail-binding targets are present in scope.
    pub(super) fn arm_result_type(&mut self, arm: &CondArm) -> Type {
        let Some(last_stmt) = arm.body.last() else {
            return Type::Unknown;
        };
        match last_stmt {
            Statement::Expr(e) => self.infer_expr_type(e),
            Statement::Assignment(a) => self.lookup_var(&a.target).unwrap_or(Type::Unknown),
            Statement::UnmoldForward(u) => self.lookup_var(&u.target).unwrap_or(Type::Unknown),
            Statement::UnmoldBackward(u) => self.lookup_var(&u.target).unwrap_or(Type::Unknown),
            _ => Type::Unknown,
        }
    }

    /// F56: reject a direct unmold (`>=>` / `<=<`, statement or expression) of a
    /// sealed carrier (`Moltenized[T]` / `Secret[T]`). Pulling the inner value
    /// back out is exactly the leak the carrier exists to prevent — the value
    /// can only be consumed by a secret-aware operation. Emits `[E1535]`.
    pub(super) fn reject_sealed_carrier_unmold(&mut self, source_ty: &Type, span: &Span) {
        if matches!(source_ty, Type::Generic(n, _) if n == "Moltenized" || n == "Secret") {
            self.errors.push(TypeError {
                message: format!(
                    "[E1535] a sealed carrier ({}) cannot be unmolded directly with \
                     `>=>` / `<=<`; the secret value would be exposed. Hint: consume it \
                     with a secret-aware operation (e.g. `HmacSha256[]` / \
                     `ConstantTimeEq[]`) or render a mask with `Redact[secret]()`.",
                    source_ty
                ),
                span: span.clone(),
            });
        }
    }

    /// F56: find a sealed carrier (`Moltenized[T]` / `Secret[T]`) used as a
    /// *direct* operand of a binary/unary expression, without inferring the full
    /// subtree. This is the cheap, idempotent counterpart to the binary-op
    /// `[E1536]` guard for sink-builtin arguments: a sink's `BinaryOp` arg is not
    /// type-inferred (to preserve the `"x" + lax.getOrDefault(_)` tolerance), so
    /// `stdout("x" + secret)` would otherwise reach the divergent runtime path.
    /// Only `Ident` (via `lookup_var`) and `Moltenize[]` / `MoltenizeSecret[]`
    /// producers are recognised — never a method chain, so HashMap `.get()`
    /// inference is left untouched.
    pub(super) fn first_direct_sealed_operand(&self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Ident(name, _) => {
                let t = self.lookup_var(name)?;
                matches!(&t, Type::Generic(n, _) if n == "Moltenized" || n == "Secret").then_some(t)
            }
            Expr::MoldInst(name, _, _, _) if name == "Moltenize" || name == "MoltenizeSecret" => {
                let policy = if name == "MoltenizeSecret" {
                    "Secret"
                } else {
                    "Moltenized"
                };
                Some(Type::Generic(policy.to_string(), vec![Type::Unknown]))
            }
            Expr::BinaryOp(l, _, r, _) => self
                .first_direct_sealed_operand(l)
                .or_else(|| self.first_direct_sealed_operand(r)),
            Expr::UnaryOp(_, inner, _) => self.first_direct_sealed_operand(inner),
            _ => None,
        }
    }

    /// F56: find a sealed carrier nested one or more levels inside a `@(...)` /
    /// `@[...]` literal (lock L0-4c). Used by the serialization / display sink
    /// guard so `jsonEncode(@(token <= secret))` is rejected at compile time,
    /// matching the runtime's nested fail-close (`null` / `<policy>`). Only walks
    /// literal structures and `first_direct_sealed_operand` leaves — side-effect-
    /// free, so it never re-infers a method chain.
    pub(super) fn first_nested_sealed_in_literal(&self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                fields.iter().find_map(|f| {
                    self.first_direct_sealed_operand(&f.value)
                        .or_else(|| self.first_nested_sealed_in_literal(&f.value))
                })
            }
            Expr::ListLit(items, _) => items.iter().find_map(|item| {
                self.first_direct_sealed_operand(item)
                    .or_else(|| self.first_nested_sealed_in_literal(item))
            }),
            _ => None,
        }
    }
}

impl TypeChecker {
    pub(super) fn finalize_named_function_signature(
        &mut self,
        fd: &FuncDef,
    ) -> Option<(Vec<Type>, Type)> {
        let Some(return_type) = &fd.return_type else {
            self.errors.push(TypeError {
                message: format!(
                    "[E1526] Function '{}' must declare a return type with `=> :Type`.",
                    fd.name
                ),
                span: fd.span.clone(),
            });
            return None;
        };

        let ret_ty = self.registry.resolve_type(return_type);
        let mut param_types: Vec<Type> = fd
            .params
            .iter()
            .map(|p| {
                p.type_annotation
                    .as_ref()
                    .map(|t| self.registry.resolve_type(t))
                    .unwrap_or(Type::Unknown)
            })
            .collect();

        if let Some(tail_expr) = fd.body.last().and_then(Statement::yielded_expr) {
            self.current_func_type_params.push(fd.type_params.clone());
            self.collect_named_function_param_constraints(fd, tail_expr, &ret_ty, &mut param_types);
            self.current_func_type_params.pop();
        }

        let mut ok = true;
        for (idx, param) in fd.params.iter().enumerate() {
            let ty = param_types.get(idx).cloned().unwrap_or(Type::Unknown);
            if Self::contains_unknown(&ty) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1525] Cannot infer type of parameter '{}' in function '{}'. Add a type annotation.",
                        param.name, fd.name
                    ),
                    span: param.span.clone(),
                });
                ok = false;
            }
        }

        ok.then_some((param_types, ret_ty))
    }

    fn collect_named_function_param_constraints(
        &mut self,
        fd: &FuncDef,
        expr: &Expr,
        expected: &Type,
        param_types: &mut [Type],
    ) {
        match expr {
            Expr::Ident(name, span) => {
                self.constrain_named_function_param(fd, name, expected, param_types, span);
            }
            Expr::BinaryOp(left, op, right, span) => {
                if let Some(operand_ty) = self.binary_operand_constraint_from_expected(op, expected)
                {
                    self.collect_named_function_param_constraints(
                        fd,
                        left,
                        &operand_ty,
                        param_types,
                    );
                    self.collect_named_function_param_constraints(
                        fd,
                        right,
                        &operand_ty,
                        param_types,
                    );
                } else if matches!(op, BinOp::Add) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1525] Cannot resolve overloaded '+' in function '{}'. Add parameter annotations or use a concrete return type.",
                            fd.name
                        ),
                        span: span.clone(),
                    });
                }
            }
            Expr::UnaryOp(_, inner, _) => {
                self.collect_named_function_param_constraints(fd, inner, expected, param_types);
            }
            Expr::Unmold(base, _) | Expr::Throw(base, _) => {
                self.collect_named_function_param_constraints(fd, base, expected, param_types);
            }
            Expr::FieldAccess(_, _, _) => {}
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(arm_expr) = arm.last_expr() {
                        self.collect_named_function_param_constraints(
                            fd,
                            arm_expr,
                            expected,
                            param_types,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn binary_operand_constraint_from_expected(&self, op: &BinOp, expected: &Type) -> Option<Type> {
        match op {
            BinOp::Add => match expected {
                Type::Int | Type::Float | Type::Num | Type::Str => Some(expected.clone()),
                Type::Named(name) if self.type_param_is_numeric(name) => Some(expected.clone()),
                _ => None,
            },
            BinOp::Sub | BinOp::Mul => match expected {
                Type::Int | Type::Float | Type::Num => Some(expected.clone()),
                Type::Named(name) if self.type_param_is_numeric(name) => Some(expected.clone()),
                _ => None,
            },
            BinOp::Lt | BinOp::Gt | BinOp::GtEq => None,
            BinOp::Eq | BinOp::NotEq | BinOp::And | BinOp::Or | BinOp::Concat => None,
        }
    }

    fn constrain_named_function_param(
        &mut self,
        fd: &FuncDef,
        name: &str,
        expected: &Type,
        param_types: &mut [Type],
        span: &Span,
    ) {
        if matches!(expected, Type::Unknown) || Self::contains_unknown(expected) {
            return;
        }
        let Some(idx) = fd.params.iter().position(|param| param.name == name) else {
            return;
        };
        let current = param_types.get(idx).cloned().unwrap_or(Type::Unknown);
        if current == Type::Unknown {
            param_types[idx] = expected.clone();
            return;
        }
        if current != *expected
            && !self.registry.is_subtype_of(&current, expected)
            && !self.registry.is_subtype_of(expected, &current)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1525] Conflicting inferred type for parameter '{}' in function '{}': {} vs {}.",
                    name, fd.name, current, expected
                ),
                span: span.clone(),
            });
        }
    }

    pub(super) fn infer_expr_type_with_expected(&mut self, expr: &Expr, expected: &Type) -> Type {
        self.infer_expr_type_with_expected_inner(expr, expected, FunctionHintDiagnostic::MethodArg)
    }

    fn fill_unknowns_from_expected(inferred: &Type, expected: &Type) -> Type {
        Self::fill_unknowns_from_expected_at_depth(inferred, expected, 0)
    }

    fn fill_unknowns_from_expected_at_depth(
        inferred: &Type,
        expected: &Type,
        depth: usize,
    ) -> Type {
        if depth >= Self::MAX_BIDI_TYPE_HINT_DEPTH {
            return inferred.clone();
        }
        match (inferred, expected) {
            (
                Type::Generic(inferred_name, inferred_args),
                Type::Generic(expected_name, expected_args),
            ) if inferred_name == expected_name && inferred_args.len() == expected_args.len() => {
                Type::Generic(
                    inferred_name.clone(),
                    inferred_args
                        .iter()
                        .zip(expected_args.iter())
                        .map(|(actual, expected)| {
                            if matches!(actual, Type::Unknown) {
                                expected.clone()
                            } else {
                                Self::fill_unknowns_from_expected_at_depth(
                                    actual,
                                    expected,
                                    depth + 1,
                                )
                            }
                        })
                        .collect(),
                )
            }
            (Type::List(inferred_inner), Type::List(expected_inner)) => Type::List(Box::new(
                if matches!(inferred_inner.as_ref(), Type::Unknown) {
                    expected_inner.as_ref().clone()
                } else {
                    Self::fill_unknowns_from_expected_at_depth(
                        inferred_inner,
                        expected_inner,
                        depth + 1,
                    )
                },
            )),
            (Type::BuchiPack(inferred_fields), Type::BuchiPack(expected_fields)) => {
                Type::BuchiPack(
                    inferred_fields
                        .iter()
                        .map(|(field_name, inferred_ty)| {
                            let hinted_ty = expected_fields
                                .iter()
                                .find(|(expected_name, _)| expected_name == field_name)
                                .map(|(_, expected_ty)| {
                                    if matches!(inferred_ty, Type::Unknown) {
                                        expected_ty.clone()
                                    } else {
                                        Self::fill_unknowns_from_expected_at_depth(
                                            inferred_ty,
                                            expected_ty,
                                            depth + 1,
                                        )
                                    }
                                })
                                .unwrap_or_else(|| inferred_ty.clone());
                            (field_name.clone(), hinted_ty)
                        })
                        .collect(),
                )
            }
            (
                Type::Function(inferred_params, inferred_ret),
                Type::Function(expected_params, expected_ret),
            ) if inferred_params.len() == expected_params.len() => Type::Function(
                // This is hint filling, not subtype validation. Function
                // boundary variance is checked later by is_function_arg_subtype_of.
                inferred_params
                    .iter()
                    .zip(expected_params.iter())
                    .map(|(actual, expected)| {
                        if matches!(actual, Type::Unknown) {
                            expected.clone()
                        } else {
                            Self::fill_unknowns_from_expected_at_depth(actual, expected, depth + 1)
                        }
                    })
                    .collect(),
                Box::new(if matches!(inferred_ret.as_ref(), Type::Unknown) {
                    expected_ret.as_ref().clone()
                } else {
                    Self::fill_unknowns_from_expected_at_depth(
                        inferred_ret,
                        expected_ret,
                        depth + 1,
                    )
                }),
            ),
            _ => inferred.clone(),
        }
    }

    fn generic_expected_hint(
        &self,
        pattern: &Type,
        generic_names: &HashSet<String>,
        bindings: &HashMap<String, Type>,
    ) -> Type {
        let substituted = self.substitute_generic_type(pattern, generic_names, bindings);
        Self::erase_unbound_generic_names(&substituted, generic_names)
    }

    fn erase_unbound_generic_names(ty: &Type, generic_names: &HashSet<String>) -> Type {
        Self::erase_unbound_generic_names_at_depth(ty, generic_names, 0)
    }

    fn erase_unbound_generic_names_at_depth(
        ty: &Type,
        generic_names: &HashSet<String>,
        depth: usize,
    ) -> Type {
        if depth >= Self::MAX_BIDI_TYPE_HINT_DEPTH {
            return ty.clone();
        }
        match ty {
            Type::Named(name) if generic_names.contains(name) => Type::Unknown,
            Type::List(inner) => Type::List(Box::new(Self::erase_unbound_generic_names_at_depth(
                inner,
                generic_names,
                depth + 1,
            ))),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| {
                        Self::erase_unbound_generic_names_at_depth(arg, generic_names, depth + 1)
                    })
                    .collect(),
            ),
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            Self::erase_unbound_generic_names_at_depth(
                                ty,
                                generic_names,
                                depth + 1,
                            ),
                        )
                    })
                    .collect(),
            ),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| {
                        Self::erase_unbound_generic_names_at_depth(param, generic_names, depth + 1)
                    })
                    .collect(),
                Box::new(Self::erase_unbound_generic_names_at_depth(
                    ret,
                    generic_names,
                    depth + 1,
                )),
            ),
            _ => ty.clone(),
        }
    }
}
