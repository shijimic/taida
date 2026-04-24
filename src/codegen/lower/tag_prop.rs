// C12B-038: Full migration of state-dependent tag-propagation helpers.
//
// This module is the 9th split target of the former monolithic
// `src/codegen/lower.rs` (C12B-024 established submodules 1-8:
// `core` / `imports` / `stdlib` / `molds` / `stmt` / `expr` / `infer`
// plus `mod.rs`). The state-dependent tag-propagation helpers that
// previously lived in `lower/infer.rs` are physically relocated here
// together with the stdout/stderr `_with_tag` dispatch extracted from
// `lower/expr.rs` as the new `lower_stdout_with_tag` helper.
//
// # Relationship with `src/codegen/tag_prop.rs`
//
// `src/codegen/tag_prop.rs` (sibling of `lower/`) is the **pure,
// state-independent** part of tag propagation (C12B-038 PARTIAL fix,
// 2026-04-15): named `TAG_*` constants plus the free function
// `type_expr_to_tag`. This file complements it by holding the
// `impl Lowering` methods that *do* consult `Lowering` state
// (HashSets of Bool-returning functions, the `param_tag_vars` map,
// per-`Expr` inference, etc). The two modules together form the
// complete tag-propagation surface.
//
// # Mechanical move policy
//
// Every method in this file is moved from its previous location with
// its signature, body, privacy, and doc comments preserved. The only
// behavioural change is the new extracted helper `lower_stdout_with_tag`
// which collapses the stdout/stderr `_with_tag` dispatch that used to
// be inlined in `lower/expr.rs::lower_func_call`. Callers go through a
// single entry point so any future tag-propagation refinement (e.g.
// pipeline / unmold / Lax-aware extensions planned for FB-1 coverage
// patterns 2-4) only touches this file.

use super::{LowerError, Lowering, simple_hash};
use crate::codegen::ir::*;
use crate::parser::*;

impl Lowering {
    /// Maximum number of tagged arguments per call.
    /// Arguments beyond this limit are skipped (tag defaults to INT/0
    /// in the callee). 256 exceeds any practical function arity in
    /// Taida.
    pub(super) const TAG_FRAME_SIZE: usize = 256;

    /// 式が bool 値を返すかどうかを判定
    pub(crate) fn expr_is_bool(&self, expr: &Expr) -> bool {
        match expr {
            Expr::BoolLit(_, _) => true,
            Expr::Ident(name, _) => self.bool_vars.contains(name),
            Expr::BinaryOp(_, op, _, _) => {
                matches!(
                    op,
                    BinOp::Eq
                        | BinOp::NotEq
                        | BinOp::Lt
                        | BinOp::Gt
                        | BinOp::GtEq
                        | BinOp::And
                        | BinOp::Or
                )
            }
            Expr::UnaryOp(UnaryOp::Not, _, _) => true,
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "hasValue"
                        | "isEmpty"
                        | "contains"
                        | "has"
                        | "startsWith"
                        | "endsWith"
                        | "any"
                        | "all"
                        | "none"
                        | "isOk"
                        | "isError"
                        | "isSuccess"
                        | "isFulfilled"
                        | "isPending"
                        | "isRejected"
                        | "isNaN"
                        | "isInfinite"
                        | "isFinite"
                        | "isPositive"
                        | "isNegative"
                        | "isZero"
                )
            }
            Expr::FuncCall(callee, _, _) => {
                // Detect bool-returning user-defined functions
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.bool_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            // WFX-3: Exists[path]() returns Bool
            Expr::MoldInst(name, _, _, _) if name == "Exists" => true,
            // B11-6d: TypeIs/TypeExtends return Bool
            Expr::MoldInst(name, _, _, _) if name == "TypeIs" || name == "TypeExtends" => true,
            // C26B-016 (@c.26, Option B+): span-aware Bool molds
            Expr::MoldInst(name, _, _, _)
                if name == "SpanEquals"
                    || name == "SpanStartsWith"
                    || name == "SpanContains" =>
            {
                true
            }
            Expr::FieldAccess(obj, field, _) => {
                // QF-34: hasValue フィールドは Lax/Result の Bool フィールド
                if field == "hasValue" {
                    return true;
                }
                // QF-10: フィールドの型を、アクセス元の TypeDef 定義から判定する。
                // グローバルな field_type_tags は同名フィールドが異なる型で使われると衝突するため、
                // TypeDef の型注釈を直接参照する。
                if let Some(type_name) = self.infer_type_name(obj)
                    && let Some(field_types) = self.type_field_types.get(&type_name)
                {
                    return field_types.iter().any(|(name, ty)| {
                        name == field
                            && matches!(ty, Some(crate::parser::TypeExpr::Named(n)) if n == "Bool")
                    });
                }
                // TypeDef 不明の場合はグローバル field_type_tags にフォールバック
                self.field_type_tags.get(field).copied() == Some(4)
            }
            _ => false,
        }
    }

    /// C18-2: If `expr` is (or produces) a known Enum value, return the
    /// enum type name. Used by anonymous BuchiPack field registration so
    /// `@(state <= HiveState:Running())` marks `state` as an Enum field
    /// for jsonEncode variant-name output.
    ///
    /// Handled cases:
    /// - `Enum:Variant()` literal
    /// - Identifier whose let-binding we previously recorded as Enum-typed
    ///   (either by literal initializer, by type annotation `x: HiveState <= ...`,
    ///   or by being copied from another known Enum variable).
    /// - Function call to a user-defined function whose declared return
    ///   type is a known Enum name.
    ///
    /// Deeper inference (through pipelines, condition branches, HOF
    /// callbacks) is intentionally skipped; the returned ordinal still
    /// encodes correctly, only the wire-format downgrades to Int.
    pub(crate) fn expr_enum_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::EnumVariant(enum_name, _, _) => {
                if self.enum_defs.contains_key(enum_name) {
                    Some(enum_name.clone())
                } else {
                    None
                }
            }
            Expr::Ident(name, _) => self.enum_vars.get(name).cloned(),
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    return self.enum_returning_funcs.get(name).cloned();
                }
                None
            }
            _ => None,
        }
    }

    /// A-4c: TypeDef のフィールド型注釈から型タグを決定する
    pub(super) fn type_field_type_tag(&self, type_name: &str, field_name: &str) -> i64 {
        if let Some(field_types) = self.type_field_types.get(type_name) {
            for (name, ty) in field_types {
                if name == field_name
                    && let Some(ty_expr) = ty
                {
                    return self.type_expr_to_tag(ty_expr);
                }
            }
        }
        // Fallback to global field_type_tags
        self.field_type_tags.get(field_name).copied().unwrap_or(0)
    }

    /// C18B-003 fix: Resolve a TypeDef field's annotated Enum type name,
    /// or `None` if the field isn't declared as an Enum.
    ///
    /// Walks `type_field_types` for `type_name`, and if the annotation
    /// is a `TypeExpr::Named` whose identifier is a registered Enum
    /// (`self.enum_defs` contains it), returns the enum name. Used by
    /// `lower_type_instantiation` to emit per-pack enum descriptors so
    /// TypeDef-based packs sharing a field name across different enums
    /// no longer collide in the global field registry.
    pub(super) fn type_field_enum_name(&self, type_name: &str, field_name: &str) -> Option<String> {
        let field_types = self.type_field_types.get(type_name)?;
        for (name, ty) in field_types {
            if name != field_name {
                continue;
            }
            let ty_expr = ty.as_ref()?;
            if let crate::parser::TypeExpr::Named(n) = ty_expr
                && self.enum_defs.contains_key(n)
            {
                return Some(n.clone());
            }
        }
        None
    }

    /// TypeExpr から型タグへの変換（`Lowering` メソッド版）。
    ///
    /// 純関数版は `crate::codegen::tag_prop::type_expr_to_tag` にあり、
    /// 本メソッドは state を持たないためそれに delegate する。
    pub(super) fn type_expr_to_tag(&self, ty: &crate::parser::TypeExpr) -> i64 {
        crate::codegen::tag_prop::type_expr_to_tag(ty)
    }

    /// A-4c: 式から Pack フィールド値の型タグを推論する
    /// Returns: 0=Int, 1=Float, 2=Bool, 3=Str, 4=Pack, 5=List, 6=Closure, -1=Unknown
    pub(crate) fn expr_type_tag(&self, expr: &Expr) -> i64 {
        match expr {
            Expr::IntLit(_, _) => 0,                              // TAIDA_TAG_INT
            Expr::FloatLit(_, _) => 1,                            // TAIDA_TAG_FLOAT
            Expr::BoolLit(_, _) => 2,                             // TAIDA_TAG_BOOL
            Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => 3, // TAIDA_TAG_STR
            Expr::BuchiPack(_, _) | Expr::TypeInst(_, _, _) => 4, // TAIDA_TAG_PACK
            Expr::ListLit(_, _) => 5,                             // TAIDA_TAG_LIST
            Expr::Lambda(_, _, _) => 6,                           // TAIDA_TAG_CLOSURE
            Expr::Ident(name, _) => {
                if self.bool_vars.contains(name) {
                    2
                } else if self.float_vars.contains(name) {
                    1
                } else if self.string_vars.contains(name) {
                    3
                } else if self.pack_vars.contains(name) {
                    4
                } else if self.list_vars.contains(name) {
                    5
                } else if self.closure_vars.contains(name) {
                    6
                } else {
                    -1 // TAIDA_TAG_UNKNOWN: type cannot be determined at compile time
                }
            }
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    // Known Int-returning: length, indexOf, etc. already match MethodCall above
                    if self.bool_returning_funcs.contains(name.as_str()) {
                        return 2;
                    }
                    if self.float_returning_funcs.contains(name.as_str()) {
                        return 1;
                    }
                    if self.string_returning_funcs.contains(name.as_str()) {
                        return 3;
                    }
                    if self.pack_returning_funcs.contains(name.as_str()) {
                        return 4;
                    }
                    if self.list_returning_funcs.contains(name.as_str()) {
                        return 5;
                    }
                    // Builtin range() returns a List
                    if name == "range" {
                        return 5;
                    }
                }
                -1 // TAIDA_TAG_UNKNOWN: return type cannot be determined at compile time
            }
            Expr::MethodCall(_, method, _, _) => {
                if self.expr_is_bool(expr) {
                    return 2;
                }
                match method.as_str() {
                    "toString" | "toUpperCase" | "toLowerCase" => 3,
                    "length" | "indexOf" | "lastIndexOf" => 0, // known Int-returning methods
                    "map" | "filter" | "flatMap" | "sort" | "unique" | "flatten" | "reverse"
                    | "concat" | "append" | "prepend" | "zip" | "enumerate" => 5,
                    _ => -1, // TAIDA_TAG_UNKNOWN
                }
            }
            // C12-1b (FB-27): MoldInst return-type tag dispatch now consults the
            // single-source-of-truth table in `src/types/mold_returns.rs` instead
            // of hardcoding Pack (4) for every mold. This lets stdout/stderr
            // route Str / Int / Float / Bool / List returning molds through
            // `taida_io_stdout_with_tag` without the B11-2f `convert_to_string`
            // fallback, which the wasm runtime was miscategorizing.
            //
            // Ordering: explicit handling still applies for Dynamic molds whose
            // return tag depends on argument types (Concat / Slice / Abs / ...).
            Expr::MoldInst(name, type_args, _, _) => {
                if let Some(tag) = crate::types::mold_returns::mold_return_tag(name) {
                    return tag;
                }
                // Dynamic / user-defined molds: try argument-based inference
                // for the handful of cases we can resolve statically, otherwise
                // default to Pack (4) which matches the pre-C12 behavior for
                // user-defined molds.
                match name.as_str() {
                    // Reverse[str] → Str, Reverse[list] → List.
                    "Reverse" => {
                        if let Some(arg) = type_args.first() {
                            if self.expr_is_string_full(arg) {
                                return 3; // TAIDA_TAG_STR
                            }
                            if self.expr_is_list(arg) {
                                return 5; // TAIDA_TAG_LIST
                            }
                        }
                        -1
                    }
                    // Slice[str] → Str, Slice[bytes] → Bytes (tag as Str at wasm
                    // level since bytes share the hidden-header str layout).
                    "Slice" => {
                        if let Some(arg) = type_args.first()
                            && self.expr_is_string_full(arg)
                        {
                            return 3;
                        }
                        3 // default: Slice returns Str (checker agrees)
                    }
                    // Concat[list, ...] → List, Concat[bytes, ...] → Bytes (Str tag).
                    "Concat" => {
                        if let Some(arg) = type_args.first()
                            && self.expr_is_list(arg)
                        {
                            return 5;
                        }
                        5 // default: list
                    }
                    // Abs / Clamp / Sum / Min / Max follow the argument's numeric tag.
                    "Abs" | "Clamp" | "Sum" | "Min" | "Max" => {
                        if let Some(arg) = type_args.first() {
                            let t = self.expr_type_tag(arg);
                            if t == 0 || t == 1 {
                                return t;
                            }
                        }
                        -1
                    }
                    // Map / Filter → List of transformed elements.
                    "Map" | "Filter" => 5,
                    // If[cond, then, else] → tag of then branch.
                    "If" => {
                        if type_args.len() >= 2 {
                            self.expr_type_tag(&type_args[1])
                        } else {
                            -1
                        }
                    }
                    "Fold" | "Foldr" | "Reduce" => {
                        if let Some(acc) = type_args.first() {
                            self.expr_type_tag(acc)
                        } else {
                            -1
                        }
                    }
                    // Unknown (user-defined) mold → Pack default.
                    _ => 4,
                }
            }
            Expr::Unmold(_, _) => -1, // TAIDA_TAG_UNKNOWN: could be anything
            _ if self.expr_is_bool(expr) => 2,
            _ => -1, // TAIDA_TAG_UNKNOWN
        }
    }

    /// C12B-022: Scan a function body for `TypeIs[ident, :PrimitiveType]()`
    /// where `ident` is a parameter name. Used to mark callees that need
    /// full arg tag propagation (including INT=0 default) at the call site.
    pub(super) fn body_uses_typeis_on_ident(
        body: &[Statement],
        param_names: &std::collections::HashSet<String>,
    ) -> bool {
        fn expr_hits(e: &Expr, params: &std::collections::HashSet<String>) -> bool {
            match e {
                Expr::MoldInst(name, type_args, args, _) => {
                    if name == "TypeIs"
                        && !type_args.is_empty()
                        && let Expr::Ident(ident_name, _) = &type_args[0]
                        && params.contains(ident_name)
                    {
                        return true;
                    }
                    type_args.iter().any(|a| expr_hits(a, params))
                        || args.iter().any(|f| expr_hits(&f.value, params))
                }
                Expr::FuncCall(callee, args, _) => {
                    expr_hits(callee, params) || args.iter().any(|a| expr_hits(a, params))
                }
                Expr::MethodCall(obj, _, args, _) => {
                    expr_hits(obj, params) || args.iter().any(|a| expr_hits(a, params))
                }
                Expr::BinaryOp(l, _, r, _) => expr_hits(l, params) || expr_hits(r, params),
                Expr::UnaryOp(_, inner, _) => expr_hits(inner, params),
                Expr::Pipeline(steps, _) => steps.iter().any(|s| expr_hits(s, params)),
                Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
                    arm.condition.as_ref().is_some_and(|c| expr_hits(c, params))
                        || arm.body.iter().any(|s| stmt_hits(s, params))
                }),
                Expr::FieldAccess(obj, _, _) => expr_hits(obj, params),
                Expr::BuchiPack(fields, _) => fields.iter().any(|f| expr_hits(&f.value, params)),
                Expr::TypeInst(_, fields, _) => fields.iter().any(|f| expr_hits(&f.value, params)),
                Expr::ListLit(elems, _) => elems.iter().any(|e| expr_hits(e, params)),
                Expr::Lambda(_, body, _) => expr_hits(body, params),
                Expr::Unmold(inner, _) => expr_hits(inner, params),
                Expr::Throw(inner, _) => expr_hits(inner, params),
                _ => false,
            }
        }
        fn stmt_hits(s: &Statement, params: &std::collections::HashSet<String>) -> bool {
            match s {
                Statement::Expr(e) => expr_hits(e, params),
                Statement::Assignment(a) => expr_hits(&a.value, params),
                Statement::FuncDef(f) => f.body.iter().any(|s| stmt_hits(s, params)),
                _ => false,
            }
        }
        body.iter().any(|s| stmt_hits(s, param_names))
    }

    /// NB-14: Get the runtime param tag IrVar for an expression, if it's a function
    /// parameter with a caller-propagated type tag.
    /// Returns Some(tag_var) if the expression is an Ident whose name is in param_tag_vars.
    pub(crate) fn get_param_tag_var(&self, expr: &Expr) -> Option<IrVar> {
        if let Expr::Ident(name, _) = expr {
            self.param_tag_vars.get(name).copied()
        } else {
            None
        }
    }

    /// C12B-038: Predicate helper extracted from `lower/expr.rs` so the
    /// stdout/stderr `_with_tag` dispatch site reads as a single `match`
    /// on compile-time state. Returns true when `arg` is `Expr::Ident(n, _)`
    /// for a parameter whose runtime tag is available in `param_tag_vars`.
    pub(crate) fn is_param_tag_ident(&self, arg: &Expr) -> bool {
        matches!(arg, Expr::Ident(n, _) if self.param_tag_vars.contains_key(n))
    }

    /// NB-14: Check whether any argument requires call-site tag propagation.
    /// Returns true if at least one arg has a non-INT compile-time tag, a transitive
    /// param_tag_var, or is a FuncCall to a user function (which may carry a return tag).
    pub(super) fn needs_call_arg_tags(&self, args: &[Expr]) -> bool {
        for (i, arg) in args.iter().enumerate() {
            if i >= Self::TAG_FRAME_SIZE {
                break;
            }
            let tag = self.expr_type_tag(arg);
            if tag > 0 {
                return true;
            } else if tag == -1 {
                if self.get_param_tag_var(arg).is_some() {
                    return true;
                }
                // FuncCall to user function may carry a return type tag
                if let Expr::FuncCall(callee_box, _, _) = arg
                    && let Expr::Ident(callee_name, _) = callee_box.as_ref()
                    && self.user_funcs.contains(callee_name)
                {
                    return true;
                }
            }
        }
        false
    }

    /// NB-14: Emit taida_set_call_arg_tag() for each argument with a known non-default
    /// type tag before a CallUser. This propagates Bool/Float/Str/etc. type info from
    /// the caller to the callee so that pack field tags can be set correctly.
    /// Note: TAG_FRAME_SIZE (256) is the maximum number of tagged arguments per call.
    /// Arguments beyond this limit are skipped (tag defaults to INT/0 in the callee).
    /// 256 exceeds any practical function arity in Taida.
    pub(super) fn emit_call_arg_tags(&mut self, func: &mut IrFunction, args: &[Expr]) {
        self.emit_call_arg_tags_full(func, args, false);
    }

    /// C12B-022: Variant of `emit_call_arg_tags` that also emits the
    /// default INT=0 tag when `include_int_default` is true. Used for
    /// callees that do `TypeIs[param, :PrimitiveType]()` — the tag
    /// frame initialises to 0xFF (UNKNOWN), so without an explicit
    /// write the callee reads UNKNOWN and the runtime match returns
    /// false even for Int arguments.
    pub(super) fn emit_call_arg_tags_full(
        &mut self,
        func: &mut IrFunction,
        args: &[Expr],
        include_int_default: bool,
    ) {
        for (i, arg) in args.iter().enumerate() {
            if i >= Self::TAG_FRAME_SIZE {
                break;
            }
            let tag = self.expr_type_tag(arg);
            // Only emit for non-INT tags (INT=0 is the default and doesn't need propagation)
            // Also skip UNKNOWN (-1) since that would overwrite any existing tag
            if tag > 0 {
                let idx_var = func.alloc_var();
                func.push(IrInst::ConstInt(idx_var, i as i64));
                let tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(tag_var, tag));
                let dummy = func.alloc_var();
                func.push(IrInst::Call(
                    dummy,
                    "taida_set_call_arg_tag".to_string(),
                    vec![idx_var, tag_var],
                ));
            } else if tag == 0 && include_int_default {
                // C12B-022: Explicitly emit INT=0 for param-type-check callees.
                let idx_var = func.alloc_var();
                func.push(IrInst::ConstInt(idx_var, i as i64));
                let tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(tag_var, 0));
                let dummy = func.alloc_var();
                func.push(IrInst::Call(
                    dummy,
                    "taida_set_call_arg_tag".to_string(),
                    vec![idx_var, tag_var],
                ));
            } else if tag == -1 {
                // UNKNOWN: if we have a param_tag_var for this, propagate it transitively
                if let Some(existing_tag_var) = self.get_param_tag_var(arg) {
                    let idx_var = func.alloc_var();
                    func.push(IrInst::ConstInt(idx_var, i as i64));
                    let dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        dummy,
                        "taida_set_call_arg_tag".to_string(),
                        vec![idx_var, existing_tag_var],
                    ));
                } else if include_int_default {
                    // C12B-022: Even when we can't determine the tag, emit
                    // INT=0 so the callee reads 0 rather than UNKNOWN.
                    let idx_var = func.alloc_var();
                    func.push(IrInst::ConstInt(idx_var, i as i64));
                    let tag_var = func.alloc_var();
                    func.push(IrInst::ConstInt(tag_var, 0));
                    let dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        dummy,
                        "taida_set_call_arg_tag".to_string(),
                        vec![idx_var, tag_var],
                    ));
                }
            }
        }
    }

    /// NB-14: Emit taida_set_call_arg_tag() for arguments whose type was determined
    /// AFTER lowering (via return type tag from a nested CallUser). This complements
    /// emit_call_arg_tags which handles compile-time known types before lowering.
    pub(super) fn emit_post_lower_arg_tags(
        &self,
        func: &mut IrFunction,
        args: &[Expr],
        explicit_arg_vars: &[IrVar],
    ) {
        for (i, arg_var) in explicit_arg_vars.iter().enumerate() {
            if i >= Self::TAG_FRAME_SIZE || i >= args.len() {
                break;
            }
            // Skip args already handled by emit_call_arg_tags (known type or param_tag_var)
            let tag = self.expr_type_tag(&args[i]);
            if tag > 0 {
                continue; // Already set in pre-lower pass
            }
            if tag == -1 && self.get_param_tag_var(&args[i]).is_some() {
                continue; // Already set in pre-lower pass
            }
            // Check if this arg's IrVar has a return_tag_var (from a nested CallUser)
            if let Some(&rtv) = self.return_tag_vars.get(arg_var) {
                let idx_var = func.alloc_var();
                func.push(IrInst::ConstInt(idx_var, i as i64));
                let dummy = func.alloc_var();
                func.push(IrInst::Call(
                    dummy,
                    "taida_set_call_arg_tag".to_string(),
                    vec![idx_var, rtv],
                ));
            }
        }
    }

    /// C12B-038: Lower the stdout/stderr `_with_tag` dispatch. Extracted from
    /// `lower/expr.rs::lower_func_call` so the two-path dispatch (compile-time
    /// Str fast path vs. polymorphic `_with_tag`) lives in a single, grep-able
    /// helper.
    ///
    /// `name` is always either `"stdout"` or `"stderr"` (the caller gates on
    /// `stdlib_runtime_funcs` + `args.len() == 1`). `rt_name` is the resolved
    /// stdlib runtime symbol (e.g. `"taida_io_stdout"`) used for the Str fast
    /// path; the `_with_tag` variant is derived internally.
    ///
    /// # Two-path behaviour (C12B-016 contract)
    ///
    /// 1. **Compile-time Str fast path** — when `expr_type_tag(arg) == TAG_STR`
    ///    the plain `taida_io_stdout(char*)` call is emitted. This is the hot
    ///    path for `stdout("literal")` and keeps wasm-min Hello World small
    ///    (the `_with_tag` symbol stays unreferenced for STR-only programs
    ///    and wasm-ld `--gc-sections` drops it).
    /// 2. **Polymorphic dispatch path** — every other tag (Bool / Int / Float /
    ///    Pack / List / UNKNOWN) reaches `taida_io_{stdout,stderr}_with_tag`
    ///    with the compile-time tag (or `-1` for UNKNOWN). The runtime does
    ///    the final formatting.
    ///
    /// # C12-11 (FB-1) Ident-param coverage
    ///
    /// When `arg` is an `Expr::Ident` whose name lives in `param_tag_vars`,
    /// the caller's runtime tag IrVar is threaded through `_with_tag` so that
    /// Bool values flowing through function parameters are rendered as
    /// `"true"/"false"` instead of `"1"/"0"` on Native.
    ///
    /// # B11B-004 FieldAccess double-eval guard
    ///
    /// For a FieldAccess whose compile-time tag is UNKNOWN, the parent object
    /// is evaluated exactly once and both the field value and its runtime tag
    /// are derived from that single evaluation to avoid double-eval of side
    /// effects (e.g. `makePack().flag`).
    pub(crate) fn lower_stdout_with_tag(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        arg: &Expr,
        rt_name: String,
    ) -> Result<IrVar, LowerError> {
        let compile_tag = self.expr_type_tag(arg);

        // (1) Compile-time Str fast path. The plain I/O entry expects a
        // `char*` and is the smallest reachable symbol on wasm-min —
        // never pulls `_with_tag`.
        if compile_tag == crate::codegen::tag_prop::TAG_STR {
            let arg_var = self.lower_expr(func, arg)?;
            let result = func.alloc_var();
            func.push(IrInst::Call(result, rt_name, vec![arg_var]));
            return Ok(result);
        }

        // (2) Polymorphic dispatch — every other tag.
        let is_param_tag_ident = compile_tag == -1 && self.is_param_tag_ident(arg);
        let is_field_access_unknown =
            compile_tag == -1 && matches!(arg, Expr::FieldAccess(_, _, _));

        let (arg_var, tag_var) = if is_field_access_unknown {
            // FieldAccess with compile-time-unknown tag: evaluate the
            // parent object once, derive both the field value and its
            // runtime tag from that single evaluation (avoids double-eval
            // of side effects).
            let (obj, field) = match arg {
                Expr::FieldAccess(obj, field, _) => (obj, field),
                _ => unreachable!("is_field_access_unknown gated by matches!"),
            };
            let obj_var = self.lower_expr(func, obj)?;
            let field_hash = simple_hash(field);
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, field_hash as i64));

            let field_val = func.alloc_var();
            func.push(IrInst::Call(
                field_val,
                "taida_pack_get".to_string(),
                vec![obj_var, hash_var],
            ));

            let rt_tag = func.alloc_var();
            func.push(IrInst::Call(
                rt_tag,
                "taida_pack_get_field_tag".to_string(),
                vec![obj_var, hash_var],
            ));
            (field_val, rt_tag)
        } else if is_param_tag_ident {
            // C12-11 case (a): arg is `v` where `v` is a parameter with a
            // propagated tag. Use the IrVar that holds the caller's tag
            // (allocated in `lower_func_def` / lambda body entry).
            let val = self.lower_expr(func, arg)?;
            let name_str = match arg {
                Expr::Ident(n, _) => n.clone(),
                _ => unreachable!("is_param_tag_ident guarantees Ident"),
            };
            let tag = *self
                .param_tag_vars
                .get(&name_str)
                .expect("param_tag_vars.contains_key was checked");
            (val, tag)
        } else {
            // Any remaining case: compile-time known tag (Bool / Int /
            // Float / ...) or compile-time -1 (UNKNOWN). Hand the tag as
            // a ConstInt to the runtime polymorphic dispatcher.
            let val = self.lower_expr(func, arg)?;
            let v = func.alloc_var();
            func.push(IrInst::ConstInt(v, compile_tag));
            (val, v)
        };

        let tagged_rt = if name == "stdout" {
            "taida_io_stdout_with_tag".to_string()
        } else {
            "taida_io_stderr_with_tag".to_string()
        };
        let result = func.alloc_var();
        func.push(IrInst::Call(result, tagged_rt, vec![arg_var, tag_var]));
        Ok(result)
    }
}
