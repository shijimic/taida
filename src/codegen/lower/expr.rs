// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups expr methods of the `Lowering` struct (per the lower/ split's
// placement table). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{HIDDEN, LowerError, Lowering, OS_NET_DEFAULT_TIMEOUT_MS, PIPE_PREV, simple_hash};
use crate::codegen::ir::*;
use crate::parser::*;

impl Lowering {
    pub(crate) fn lower_expr(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
    ) -> Result<IrVar, LowerError> {
        let value = self.lower_expr_value(func, expr)?;
        if matches!(
            expr,
            Expr::MoldInst(..) | Expr::MethodCall(..) | Expr::FuncCall(..)
        ) {
            self.stamp_payload_kind(func, expr, value);
        }
        if let Expr::MethodCall(obj, method, _, _) = expr
            && matches!(method.as_str(), "get" | "first" | "last")
        {
            self.fill_static_miss_default(func, obj, method, value)?;
        }
        if let Expr::MoldInst(name, type_args, _, _) = expr
            && name == "Find"
            && let Some(list) = type_args.first()
        {
            self.fill_static_miss_default(func, list, "first", value)?;
        }
        Ok(value)
    }

    /// A lookup that finds nothing (`get` / `first` / `last` / `Find` on a
    /// list, `get` on a HashMap) yields the default of the element's static
    /// type, as the interpreter's does. The runtime knows only the element's
    /// kind (none for an empty map), which fixes no pack, list or map, so the
    /// lowering builds the default — only when the lookup found nothing.
    fn fill_static_miss_default(
        &mut self,
        func: &mut IrFunction,
        obj: &Expr,
        method: &str,
        lax: IrVar,
    ) -> Result<(), LowerError> {
        use crate::types::Type;
        let elem = match self.typed_expr_table.lookup(obj) {
            Some(Type::List(elem)) => (**elem).clone(),
            Some(Type::Generic(name, args))
                if name == "HashMap" && args.len() == 2 && method == "get" =>
            {
                args[1].clone()
            }
            _ => return Ok(()),
        };
        let Some(type_expr) = Self::type_expr_of(&elem) else {
            return Ok(());
        };
        let slot = func.alloc_var();
        func.push(IrInst::ConstInt(slot, 0));
        let found = func.alloc_var();
        func.push(IrInst::Call(
            found,
            "taida_pack_get_idx".to_string(),
            vec![lax, slot],
        ));
        let saved = std::mem::take(&mut func.body);
        let default = self.lower_default_for_type_expr(
            func,
            &type_expr,
            &mut std::collections::HashSet::new(),
        )?;
        let tag = func.alloc_var();
        func.push(IrInst::ConstInt(tag, self.type_expr_to_tag(&type_expr)));
        let filled = func.alloc_var();
        func.push(IrInst::Call(
            filled,
            "taida_lax_fill_default".to_string(),
            vec![lax, default, tag],
        ));
        let fill = std::mem::replace(&mut func.body, saved);
        let out = func.alloc_var();
        func.push(IrInst::CondBranch(
            out,
            vec![
                crate::codegen::ir::CondArm {
                    condition: Some(found),
                    body: Vec::new(),
                    result: lax,
                },
                crate::codegen::ir::CondArm {
                    condition: None,
                    body: fill,
                    result: filled,
                },
            ],
        ));
        Ok(())
    }

    /// The type annotation naming the checker type `ty`, for the default
    /// builder; `None` for a type it has no annotation for.
    fn type_expr_of(ty: &crate::types::Type) -> Option<TypeExpr> {
        use crate::types::Type;
        Some(match ty {
            Type::Int => TypeExpr::Named("Int".to_string()),
            Type::Float => TypeExpr::Named("Float".to_string()),
            Type::Str => TypeExpr::Named("Str".to_string()),
            Type::Bool => TypeExpr::Named("Bool".to_string()),
            Type::List(inner) => TypeExpr::List(Box::new(Self::type_expr_of(inner)?)),
            Type::BuchiPack(fields) => TypeExpr::BuchiPack(
                fields
                    .iter()
                    .map(|(name, field_ty)| {
                        Some(FieldDef {
                            name: name.clone(),
                            type_annotation: Some(Self::type_expr_of(field_ty)?),
                            default_value: None,
                            is_method: false,
                            method_def: None,
                            doc_comments: Vec::new(),
                            span: crate::lexer::Span::new(0, 0, 0, 0),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            Type::Named(name) => TypeExpr::Named(name.clone()),
            // An empty map or set, whatever it would hold.
            Type::Generic(name, _) if name == "HashMap" || name == "Set" => {
                TypeExpr::Named(name.clone())
            }
            Type::Generic(name, args) => TypeExpr::Generic(
                name.clone(),
                args.iter()
                    .map(Self::type_expr_of)
                    .collect::<Option<Vec<_>>>()?,
            ),
            // A function returning its return type's default; the parameters
            // are only counted.
            Type::Function(params, ret) => TypeExpr::Function(
                params
                    .iter()
                    .map(|p| {
                        Self::type_expr_of(p).unwrap_or_else(|| TypeExpr::Named("Int".to_string()))
                    })
                    .collect(),
                Box::new(Self::type_expr_of(ret)?),
            ),
            _ => return None,
        })
    }

    /// A Lax, Gorillax or Result the checker knows to carry a Float, Bool or
    /// Str: tag the slots holding it (`__value`, and a Lax's `__default`)
    /// with that kind. The runtime cannot always tell the kind from the
    /// payload (a Float's bits, a Str literal on native, any Str on WASM)
    /// and would show it as a number.
    fn stamp_payload_kind(&self, func: &mut IrFunction, expr: &Expr, mold: IrVar) {
        use crate::types::Type;
        let Some(Type::Generic(name, args)) = self.typed_expr_table.lookup(expr) else {
            return;
        };
        let slots: &[usize] = match name.as_str() {
            "Lax" => &[1, 2],
            "Gorillax" | "RelaxedGorillax" => &[1],
            "Result" => &[0],
            _ => return,
        };
        let tag = match args.first() {
            Some(Type::Float) => crate::codegen::tag_prop::TAG_FLOAT,
            Some(Type::Bool) => crate::codegen::tag_prop::TAG_BOOL,
            Some(Type::Str) => crate::codegen::tag_prop::TAG_STR,
            _ => return,
        };
        for &slot in slots {
            func.push(IrInst::PackSetTag(mold, slot, tag));
        }
    }

    fn lower_expr_value(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
    ) -> Result<IrVar, LowerError> {
        match expr {
            // F62B-022: blocks are unwrapped into the lambda's statement
            // body by lower_lambda; a standalone block reaching the
            // expression lowerer is a compiler bug.
            Expr::Block(_, _) => Err(LowerError {
                message: "internal: expression block outside a lambda body".to_string(),
            }),
            Expr::IntLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstInt(var, *val));
                Ok(var)
            }
            Expr::FloatLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstFloat(var, *val));
                Ok(var)
            }
            Expr::StringLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstStr(var, val.clone()));
                Ok(var)
            }
            Expr::BoolLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstBool(var, *val));
                Ok(var)
            }
            Expr::Ident(name, _) => {
                // stdlib 定数（PI, E 等）はインライン展開
                if let Some(&val) = self.stdlib_constants.get(name) {
                    let var = func.alloc_var();
                    func.push(IrInst::ConstFloat(var, val));
                    return Ok(var);
                }
                // 引数・局所変数は、同名のユーザー関数や組み込みより優先する
                let locally_bound = Self::is_local_binding(func, name);
                // ユーザー定義関数を値として参照する場合は FuncAddr を使う
                if !locally_bound
                    && (self.inner_func_symbol(name).is_some() || self.user_funcs.contains(name))
                {
                    let mangled = self.resolve_user_func_symbol(name);
                    let var = func.alloc_var();
                    func.push(IrInst::FuncAddr(var, mangled));
                    // The value shows as `<function name>`.
                    let name_var = func.alloc_var();
                    func.push(IrInst::ConstStr(name_var, name.clone()));
                    let named = func.alloc_var();
                    func.push(IrInst::Call(
                        named,
                        "taida_register_fn_name".to_string(),
                        vec![var, name_var],
                    ));
                    return Ok(named);
                }
                if !locally_bound
                    && !self.top_level_vars.contains(name)
                    && self.inner_func_symbol(name).is_none()
                    && let Some(runtime) = self.stdlib_runtime_funcs.get(name).cloned()
                    && let Some(arity) = crate::codegen::emit::runtime_callable_arity(&runtime)
                {
                    // A known builtin used as a value needs a callable address.
                    // The wrapper applies runtime ABI conversions just like a
                    // direct call, including boxed Float arguments and results.
                    let symbol = self.next_lambda_symbol("builtin");
                    let params = (0..arity).map(|i| format!("__arg_{i}")).collect();
                    let mut wrapper = IrFunction::new_with_params(symbol.clone(), params);
                    let result = wrapper.alloc_var();
                    wrapper.push(IrInst::Call(result, runtime, (0..arity as u32).collect()));
                    wrapper.push(IrInst::Return(result));
                    self.lambda_funcs.push(wrapper);
                    let value = func.alloc_var();
                    func.push(IrInst::FuncAddr(value, symbol));
                    // The value shows as `<function name>`, like a user function.
                    let name_var = func.alloc_var();
                    func.push(IrInst::ConstStr(name_var, name.clone()));
                    let named = func.alloc_var();
                    func.push(IrInst::Call(
                        named,
                        "taida_register_fn_name".to_string(),
                        vec![value, name_var],
                    ));
                    return Ok(named);
                }
                let var = func.alloc_var();
                func.push(IrInst::UseVar(var, name.clone()));
                Ok(var)
            }
            Expr::FuncCall(callee, args, _) => self.lower_func_call(func, callee, args),
            Expr::BinaryOp(lhs, op, rhs, _) => self.lower_binary_op(func, lhs, op, rhs),
            Expr::UnaryOp(op, operand, _) => self.lower_unary_op(func, op, operand),
            Expr::Pipeline(exprs, _) => self.lower_pipeline(func, exprs),
            Expr::BuchiPack(fields, _) => self.lower_buchi_pack(func, fields),
            Expr::TypeInst(name, fields, _) => self.lower_type_inst(func, name, fields),
            Expr::EnumVariant(enum_name, variant_name, _) => {
                let ordinal = self
                    .enum_defs
                    .get(enum_name)
                    .and_then(|variants| {
                        variants.iter().position(|variant| variant == variant_name)
                    })
                    .ok_or_else(|| LowerError {
                        message: format!("unknown enum variant '{}:{}()'", enum_name, variant_name),
                    })?;
                let result = func.alloc_var();
                func.push(IrInst::ConstInt(result, ordinal as i64));
                Ok(result)
            }
            Expr::FieldAccess(obj, field, _) => self.lower_field_access(func, obj, field),
            Expr::CondBranch(arms, _) => self.lower_cond_branch(func, arms),
            Expr::Lambda(params, body, _) => self.lower_lambda(func, params, body),
            Expr::MethodCall(obj, method, args, _) => {
                self.lower_method_call(func, obj, method, args)
            }
            Expr::ListLit(items, _) => self.lower_list_lit(func, items),
            Expr::Gorilla(_) => {
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_gorilla".to_string(), vec![]));
                Ok(result)
            }
            Expr::MoldInst(type_name, type_args, fields, _) => {
                // RC2.5 Phase 2 (RC2.5-2a): addon sentinel dispatch.
                // `Foo[]()` where `Foo` was imported from an addon-backed
                // package resolves to the same `taida_addon_call` path
                // as the plain `foo()` form. The user may spell the call
                // either way (mold-instantiation form is the RC2 facade
                // surface, `terminalSize()` is the lowercase fallback).
                //
                // `type_args` in mold syntax are positional call
                // arguments (Upper[str]() -> `str` is type_args[0]), so
                // we forward them to the shared `emit_addon_call`
                // helper. Field-form arguments (`TerminalSize[](x <= 1)`)
                // are not part of the v1 terminal contract — reject
                // them explicitly so misuse is diagnosed at compile
                // time rather than silently dropped.
                if self.addon_func_refs.contains_key(type_name) {
                    if !fields.is_empty() {
                        return Err(LowerError {
                            message: format!(
                                "addon function '{}' does not accept buchi-field arguments",
                                type_name
                            ),
                        });
                    }
                    return self.emit_addon_call(func, type_name, type_args);
                }
                self.lower_mold_inst(func, type_name, type_args, fields)
            }
            Expr::Unmold(expr, _) => {
                // expr.unmold() → taida_generic_unmold(expr)
                let val = self.lower_expr(func, expr)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            Expr::TemplateLit(template, span) => self.lower_template_lit(func, template, span),
            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::Throw(inner, _) => {
                let val = self.lower_expr(func, inner)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_throw".to_string(), vec![val]));
                Ok(result)
            }
            // B11-6a: TypeLiteral emits type name as string constant (used by TypeIs/TypeExtends lowering)
            Expr::TypeLiteral(name, variant, _) => {
                let result = func.alloc_var();
                let s = if let Some(var) = variant {
                    format!("{}:{}", name, var)
                } else {
                    name.clone()
                };
                func.push(IrInst::ConstStr(result, s));
                Ok(result)
            }
            Expr::Placeholder(_) => {
                // Placeholder outside pipeline context: return 0 (Unit)
                let var = func.alloc_var();
                func.push(IrInst::ConstInt(var, 0));
                Ok(var)
            }
            Expr::Hole(_) => {
                // Hole outside partial application context: return 0 (Unit)
                let var = func.alloc_var();
                func.push(IrInst::ConstInt(var, 0));
                Ok(var)
            }
        }
    }

    /// 末尾位置の式を lowering する（TCO対応）
    /// 自己再帰呼び出しを IrInst::TailCall に変換する
    pub(super) fn lower_expr_tail(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
    ) -> Result<IrVar, LowerError> {
        match expr {
            // 自己再帰呼び出しの検出
            Expr::FuncCall(callee, args, _) => {
                if let Expr::Ident(name, _) = callee.as_ref()
                    && self.current_func_name.as_deref() == Some(name)
                    && !Self::is_local_binding(func, name)
                {
                    // 末尾位置の自己再帰 → TailCall
                    let arg_vars =
                        self.lower_user_call_effective_args_from_exprs(func, name, args)?;
                    // Inside the body an error ceiling guards: the
                    // parameters live in the body's environment. Update
                    // them in place and re-enter the body, still under the
                    // same ceiling.
                    if let Some(param_count) = self.ceiling_tail_params
                        && arg_vars.len() == param_count
                    {
                        let env = func.alloc_var();
                        func.push(IrInst::UseVar(env, "__env".to_string()));
                        for (i, arg) in arg_vars.iter().enumerate() {
                            func.push(IrInst::PackSet(env, i, *arg));
                        }
                        func.push(IrInst::TailCall(vec![env]));
                        let dummy = func.alloc_var();
                        func.push(IrInst::ConstInt(dummy, 0));
                        return Ok(dummy);
                    }
                    // TailCall は戻り値を持たないが、IRVar は必要
                    // (Return と組み合わされるため、ダミーの var を使う)
                    func.push(IrInst::TailCall(arg_vars));
                    // TailCall の後にコードが生成されないように、
                    // ダミーの戻り値を返す（実際には到達しない）
                    let dummy = func.alloc_var();
                    func.push(IrInst::ConstInt(dummy, 0));
                    return Ok(dummy);
                }
                // 自己再帰でない場合は通常の呼び出し
                // NB-14: Mark as tail call to skip get_return_tag (preserves C TCO for WASM)
                self.in_tail_call_return = true;
                let result = self.lower_func_call(func, callee, args);
                self.in_tail_call_return = false;
                result
            }
            // CondBranch: 各アームの末尾を再帰的にチェック
            Expr::CondBranch(arms, _) => self.lower_cond_branch_tail(func, arms),
            // A pipeline in tail position: a last stage that calls a function
            // with the piped value at its `_` is that call, in tail position.
            Expr::Pipeline(exprs, _) => self.lower_pipeline_in(func, exprs, true),
            // その他の式は通常の lowering
            _ => self.lower_expr(func, expr),
        }
    }

    /// TCO対応の条件分岐 lowering
    /// 各アームの本体は末尾位置なので、自己再帰呼び出しを TailCall に変換する
    pub(super) fn lower_cond_branch_tail(
        &mut self,
        func: &mut IrFunction,
        arms: &[crate::parser::CondArm],
    ) -> Result<IrVar, LowerError> {
        use crate::codegen::ir::CondArm as IrCondArm;

        let result_var = func.alloc_var();
        let mut ir_arms = Vec::new();

        for (i, arm) in arms.iter().enumerate() {
            // A later guard that could have an effect runs only after the
            // arms before it did not match (see `lower_cond_branch`).
            if i > 0
                && arm
                    .condition
                    .as_ref()
                    .is_some_and(|cond| !Self::expr_is_effect_free(cond))
            {
                let saved = std::mem::take(&mut func.body);
                let nested = self.lower_cond_branch_tail(func, &arms[i..])?;
                let insts = std::mem::replace(&mut func.body, saved);
                ir_arms.push(IrCondArm {
                    condition: None,
                    body: insts,
                    result: nested,
                });
                break;
            }
            let condition = match &arm.condition {
                // `| true |>` is a default arm.
                Some(Expr::BoolLit(true, _)) => None,
                Some(cond_expr) => {
                    let cond_var = self.lower_expr(func, cond_expr)?;
                    Some(cond_var)
                }
                None => None,
            };

            // 本体を末尾位置として lowering（複数ステートメント対応）
            let (body_insts, body_var) = {
                let saved = std::mem::take(&mut func.body);
                let body_result = self.lower_cond_arm_body_tail(func, &arm.body)?;
                let insts = std::mem::replace(&mut func.body, saved);
                (insts, body_result)
            };

            let is_default = condition.is_none();
            ir_arms.push(IrCondArm {
                condition,
                body: body_insts,
                result: body_var,
            });
            // Arms after a default arm are never reached.
            if is_default {
                break;
            }
        }

        func.push(IrInst::CondBranch(result_var, ir_arms));
        Ok(result_var)
    }

    pub(super) fn lower_user_call_effective_args_from_exprs(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        args: &[Expr],
    ) -> Result<Vec<IrVar>, LowerError> {
        let mut explicit_arg_vars = Vec::with_capacity(args.len());
        for arg in args {
            explicit_arg_vars.push(self.lower_expr(func, arg)?);
        }
        self.lower_user_call_effective_args_from_vars(func, name, explicit_arg_vars)
    }

    /// The default entry a call of the user function `name` with `argc`
    /// explicit arguments goes through, when it omits parameters that declare
    /// defaults.
    /// The user function `name` is bound to through plain aliases
    /// (`f <= add3`, `g <= f`).
    fn alias_target_function(&self, name: &str) -> Option<String> {
        let mut current = name;
        for _ in 0..8 {
            let source = self.var_aliases.get(current)?;
            if self.user_funcs.contains(source)
                || self.inner_func_symbol(source).is_some()
                || self.local_closure_symbol(source).is_some()
            {
                return Some(source.clone());
            }
            current = source;
        }
        None
    }

    pub(super) fn default_arity_entry_for(&self, name: &str, argc: usize) -> Option<String> {
        let key = self
            .inner_func_symbol(name)
            .unwrap_or_else(|| name.to_string());
        let params = self.func_param_defs.get(&key)?;
        if self.generic_schema_params.contains_key(name)
            || !Self::default_arity_range(params).contains(&argc)
        {
            return None;
        }
        Some(Self::default_arity_symbol(
            &self.resolve_user_func_symbol(name),
            argc,
        ))
    }

    pub(super) fn lower_user_call_effective_args_from_vars(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        explicit_arg_vars: Vec<IrVar>,
    ) -> Result<Vec<IrVar>, LowerError> {
        let key = self
            .inner_func_symbol(name)
            .unwrap_or_else(|| name.to_string());
        let Some(params) = self.func_param_defs.get(&key).cloned() else {
            return Ok(explicit_arg_vars);
        };
        self.complete_args_with_param_defaults(func, name, &params, explicit_arg_vars)
    }

    /// Fill the arguments a call omits from `params`' defaults (or their
    /// types' default values), evaluated in the calling function. Only
    /// correct where the call's scope sees what the defaults refer to: the
    /// defining function's own tail calls, and local functions called from
    /// the body that defines them.
    fn complete_args_with_param_defaults(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        params: &[crate::parser::Param],
        explicit_arg_vars: Vec<IrVar>,
    ) -> Result<Vec<IrVar>, LowerError> {
        // F62B-021: schema-passing generics carry hidden schema parameters
        // appended after the declared ones; explicit call sites supply them.
        let hidden_schema_count = self
            .generic_schema_params
            .get(name)
            .map(|needed| needed.len())
            .unwrap_or(0);
        if explicit_arg_vars.len() > params.len() + hidden_schema_count {
            return Err(LowerError {
                message: format!(
                    "Function '{}' expected at most {} argument(s), got {}",
                    name,
                    params.len() + hidden_schema_count,
                    explicit_arg_vars.len()
                ),
            });
        }
        if hidden_schema_count > 0 || explicit_arg_vars.len() == params.len() {
            // Hidden args are pre-resolved; defaults never apply to them.
            return Ok(explicit_arg_vars);
        }

        // Defaults can refer to earlier parameters. Give these temporary
        // bindings their own names so absent caller variables are never read
        // and existing caller bindings remain intact.
        let body_start = func.body.len();
        let scope_id = func.alloc_var();
        let mut effective_args = Vec::with_capacity(params.len());
        for (i, param) in params.iter().enumerate() {
            let val = if let Some(v) = explicit_arg_vars.get(i) {
                *v
            } else if let Some(default_expr) = &param.default_value {
                self.lower_expr(func, default_expr)?
            } else if let Some(type_expr) = &param.type_annotation {
                let mut visiting = std::collections::HashSet::new();
                self.lower_default_for_type_expr(func, type_expr, &mut visiting)?
            } else {
                let zero = func.alloc_var();
                func.push(IrInst::ConstInt(zero, 0));
                zero
            };
            func.push(IrInst::DefVar(param.name.clone(), val));
            effective_args.push(val);
        }

        let names: std::collections::HashMap<_, _> = params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    format!("{HIDDEN}default_{scope_id}_{}", param.name),
                )
            })
            .collect();
        fn scope_bindings(insts: &mut [IrInst], names: &std::collections::HashMap<String, String>) {
            for inst in insts {
                match inst {
                    IrInst::UseVar(_, name) | IrInst::DefVar(name, _) => {
                        if let Some(scoped) = names.get(name) {
                            *name = scoped.clone();
                        }
                    }
                    IrInst::MakeClosure(_, _, captures) => {
                        for name in captures {
                            if let Some(scoped) = names.get(name) {
                                *name = scoped.clone();
                            }
                        }
                    }
                    IrInst::CondBranch(_, arms) => {
                        for arm in arms {
                            scope_bindings(&mut arm.body, names);
                        }
                    }
                    _ => {}
                }
            }
        }
        scope_bindings(&mut func.body[body_start..], &names);

        Ok(effective_args)
    }

    /// Whether `name` is bound as a parameter or a local of the function
    /// being lowered (captured values are re-bound as locals on entry).
    pub(super) fn is_local_binding(func: &IrFunction, name: &str) -> bool {
        fn defines(insts: &[IrInst], name: &str) -> bool {
            insts.iter().any(|inst| match inst {
                IrInst::DefVar(defined, _) => defined == name,
                IrInst::CondBranch(_, arms) => arms.iter().any(|arm| defines(&arm.body, name)),
                _ => false,
            })
        }
        func.params.iter().any(|p| p == name) || defines(&func.body, name)
    }

    /// Lower a call to the prelude / core-package builtin `name`, or return
    /// `None` when `name` is not one. Callers only ask for names the
    /// program does not bind itself.
    fn lower_builtin_func_call(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<IrVar>, LowerError> {
        // OS network APIs with unified timeout argument (optional last arg).
        // Native backend uses fixed runtime signatures, so we inject defaults here.
        if matches!(
            name,
            "dnsResolve"
                | "poolAcquire"
                | "tcpConnect"
                | "tcpListen"
                | "tcpAccept"
                | "socketSend"
                | "socketSendAll"
                | "socketRecv"
                | "socketSendBytes"
                | "socketRecvBytes"
                | "socketRecvExact"
                | "udpBind"
                | "udpSendTo"
                | "udpRecvFrom"
        ) {
            let timeout_var = |this: &mut Self, f: &mut IrFunction, idx: usize| {
                if let Some(arg) = args.get(idx) {
                    this.lower_expr(f, arg)
                } else {
                    let t = f.alloc_var();
                    f.push(IrInst::ConstInt(t, OS_NET_DEFAULT_TIMEOUT_MS));
                    Ok(t)
                }
            };

            match name {
                "dnsResolve" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message:
                                "dnsResolve requires 1 or 2 arguments: dnsResolve(host[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let host = self.lower_expr(func, &args[0])?;
                    let timeout = timeout_var(self, func, 1)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_dns_resolve".to_string(),
                        vec![host, timeout],
                    ));
                    return Ok(Some(result));
                }
                "poolAcquire" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message:
                                "poolAcquire requires 1 or 2 arguments: poolAcquire(pool[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let pool = self.lower_expr(func, &args[0])?;
                    // Unlike the socket APIs above, the pool carries its
                    // own configured acquireTimeoutMs. Injecting
                    // OS_NET_DEFAULT_TIMEOUT_MS here made the omitted-arg
                    // call always pass an explicit 30s, silently dead-
                    // lettering poolCreate's acquireTimeoutMs (interpreter
                    // falls back to the pool config). i64::MIN is the
                    // omitted-argument sentinel the runtime maps to the
                    // pool's configured timeout.
                    let timeout = if let Some(arg) = args.get(1) {
                        self.lower_expr(func, arg)?
                    } else {
                        let t = func.alloc_var();
                        func.push(IrInst::ConstInt(t, i64::MIN));
                        t
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_pool_acquire".to_string(),
                        vec![pool, timeout],
                    ));
                    return Ok(Some(result));
                }
                "tcpConnect" => {
                    if args.len() < 2 || args.len() > 3 {
                        return Err(LowerError {
                            message:
                                "tcpConnect requires 2 or 3 arguments: tcpConnect(host, port[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let host = self.lower_expr(func, &args[0])?;
                    let port = self.lower_expr(func, &args[1])?;
                    let timeout = timeout_var(self, func, 2)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_tcp_connect".to_string(),
                        vec![host, port, timeout],
                    ));
                    return Ok(Some(result));
                }
                "tcpListen" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message:
                                "tcpListen requires 1 or 2 arguments: tcpListen(port[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let port = self.lower_expr(func, &args[0])?;
                    let timeout = timeout_var(self, func, 1)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_tcp_listen".to_string(),
                        vec![port, timeout],
                    ));
                    return Ok(Some(result));
                }
                "tcpAccept" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message:
                                "tcpAccept requires 1 or 2 arguments: tcpAccept(listener[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let listener = self.lower_expr(func, &args[0])?;
                    let timeout = timeout_var(self, func, 1)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_tcp_accept".to_string(),
                        vec![listener, timeout],
                    ));
                    return Ok(Some(result));
                }
                "socketSend" => {
                    if args.len() < 2 || args.len() > 3 {
                        return Err(LowerError {
                            message:
                                "socketSend requires 2 or 3 arguments: socketSend(socket, data[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let data = self.lower_expr(func, &args[1])?;
                    let timeout = timeout_var(self, func, 2)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_socket_send".to_string(),
                        vec![socket, data, timeout],
                    ));
                    return Ok(Some(result));
                }
                "socketSendAll" => {
                    if args.len() < 2 || args.len() > 3 {
                        return Err(LowerError {
                            message:
                                "socketSendAll requires 2 or 3 arguments: socketSendAll(socket, data[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let data = self.lower_expr(func, &args[1])?;
                    let timeout = timeout_var(self, func, 2)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_socket_send_all".to_string(),
                        vec![socket, data, timeout],
                    ));
                    return Ok(Some(result));
                }
                "socketRecv" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message:
                                "socketRecv requires 1 or 2 arguments: socketRecv(socket[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let timeout = timeout_var(self, func, 1)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_socket_recv".to_string(),
                        vec![socket, timeout],
                    ));
                    return Ok(Some(result));
                }
                "socketRecvExact" => {
                    if args.len() < 2 || args.len() > 3 {
                        return Err(LowerError {
                            message:
                                "socketRecvExact requires 2 or 3 arguments: socketRecvExact(socket, size[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let size = self.lower_expr(func, &args[1])?;
                    let timeout = timeout_var(self, func, 2)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_socket_recv_exact".to_string(),
                        vec![socket, size, timeout],
                    ));
                    return Ok(Some(result));
                }
                "socketSendBytes" => {
                    if args.len() < 2 || args.len() > 3 {
                        return Err(LowerError {
                            message: "socketSendBytes requires 2 or 3 arguments: socketSendBytes(socket, data[, timeoutMs])".to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let data = self.lower_expr(func, &args[1])?;
                    let timeout = timeout_var(self, func, 2)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_socket_send_bytes".to_string(),
                        vec![socket, data, timeout],
                    ));
                    return Ok(Some(result));
                }
                "socketRecvBytes" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message: "socketRecvBytes requires 1 or 2 arguments: socketRecvBytes(socket[, timeoutMs])".to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let timeout = timeout_var(self, func, 1)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_socket_recv_bytes".to_string(),
                        vec![socket, timeout],
                    ));
                    return Ok(Some(result));
                }
                "udpBind" => {
                    if args.len() < 2 || args.len() > 3 {
                        return Err(LowerError {
                            message:
                                "udpBind requires 2 or 3 arguments: udpBind(host, port[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let host = self.lower_expr(func, &args[0])?;
                    let port = self.lower_expr(func, &args[1])?;
                    let timeout = timeout_var(self, func, 2)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_udp_bind".to_string(),
                        vec![host, port, timeout],
                    ));
                    return Ok(Some(result));
                }
                "udpSendTo" => {
                    if args.len() < 4 || args.len() > 5 {
                        return Err(LowerError {
                            message:
                                "udpSendTo requires 4 or 5 arguments: udpSendTo(socket, host, port, data[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let host = self.lower_expr(func, &args[1])?;
                    let port = self.lower_expr(func, &args[2])?;
                    let data = self.lower_expr(func, &args[3])?;
                    let timeout = timeout_var(self, func, 4)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_udp_send_to".to_string(),
                        vec![socket, host, port, data, timeout],
                    ));
                    return Ok(Some(result));
                }
                "udpRecvFrom" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(LowerError {
                            message:
                                "udpRecvFrom requires 1 or 2 arguments: udpRecvFrom(socket[, timeoutMs])"
                                    .to_string(),
                        });
                    }
                    let socket = self.lower_expr(func, &args[0])?;
                    let timeout = timeout_var(self, func, 1)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_os_udp_recv_from".to_string(),
                        vec![socket, timeout],
                    ));
                    return Ok(Some(result));
                }
                _ => {}
            }
        }

        // httpServe(port, handler, maxRequests <= 0, timeoutMs <= 5000, maxConnections <= 128)
        // handler is a function/closure, passed as a function pointer.
        // maxRequests, timeoutMs, and maxConnections are optional with defaults.
        // Skip if the name is shadowed by a parameter in the current function scope.
        if name == "httpServe"
            && !self.is_net_builtin_shadowed("httpServe")
            && self
                .stdlib_runtime_funcs
                .get("httpServe")
                .is_some_and(|v| v == "taida_net_http_serve")
        {
            if args.is_empty() || args.len() > 6 {
                return Err(LowerError {
                    message:
                        "httpServe requires 2 to 6 arguments: httpServe(port, handler[, maxRequests, timeoutMs, maxConnections, tls])"
                            .to_string(),
                });
            }
            let port = self.lower_expr(func, &args[0])?;
            let handler = self.lower_expr(func, &args[1])?;
            let max_requests = if let Some(arg) = args.get(2) {
                self.lower_expr(func, arg)?
            } else {
                let v = func.alloc_var();
                func.push(IrInst::ConstInt(v, 0)); // default: 0 = unlimited
                v
            };
            let timeout_ms = if let Some(arg) = args.get(3) {
                self.lower_expr(func, arg)?
            } else {
                let v = func.alloc_var();
                func.push(IrInst::ConstInt(v, 5000)); // default: 5000ms
                v
            };
            // NET2-5d: maxConnections (optional, default 128)
            let max_connections = if let Some(arg) = args.get(4) {
                self.lower_expr(func, arg)?
            } else {
                let v = func.alloc_var();
                func.push(IrInst::ConstInt(v, 128)); // default: 128
                v
            };
            // v5: tls parameter (optional, default 0 = plaintext)
            // When omitted, pass 0 (tagged int 0 = plaintext).
            // When provided, it's a BuchiPack expression (@() or @(cert, key)).
            let tls_var = if let Some(arg) = args.get(5) {
                self.lower_expr(func, arg)?
            } else {
                let v = func.alloc_var();
                func.push(IrInst::ConstInt(v, 0)); // default: 0 = plaintext
                v
            };
            // NB-31: Pass compile-time handler type tag so the C runtime
            // can reject non-callable values (large ints, strings, packs)
            // without relying on the heuristic _taida_is_callable_impl.
            // Tag 6 = CLOSURE, 10 = named function ref, -1 = unknown.
            let handler_tag = self.callable_type_tag(&args[1]);
            let handler_tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(handler_tag_var, handler_tag));
            // NET3-5a: Pass compile-time handler arity so the C runtime
            // can distinguish 1-arg (one-shot) vs 2-arg (streaming) handlers.
            // -1 = unknown (dynamic).
            let handler_arity = self.handler_arity(&args[1]);
            let handler_arity_var = func.alloc_var();
            func.push(IrInst::ConstInt(handler_arity_var, handler_arity));
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_net_http_serve".to_string(),
                vec![
                    port,
                    handler,
                    max_requests,
                    timeout_ms,
                    max_connections,
                    tls_var,
                    handler_tag_var,
                    handler_arity_var,
                ],
            ));
            return Ok(Some(result));
        }

        if name == "debug" {
            for arg in args {
                self.reject_untyped_addon_value(func, arg)?;
            }
            return self.lower_debug_call(func, args).map(Some);
        }

        // ── D28B-015: `strOf(span, raw)` lowercase function-form ──
        // Lowers identically to the existing `StrOf[span, raw]()` mold
        // (`lower/molds_inst.rs::StrOf` arm) — pure IR composition with no new
        // C runtime symbol. Naming Lock D28B-001: function form is
        // camelCase, mold form is PascalCase, both valid prelude entries
        // with bit-identical 4-backend semantics.
        //
        // User-defined `strOf` shadows the prelude builtin: only intercept
        // when the name is NOT a user FuncDef (matches the interpreter's
        // try_builtin_func dispatch order: user functions resolve first).
        if name == "strOf" && !self.user_funcs.contains(name) {
            if args.len() != 2 {
                return Err(LowerError {
                    message: format!(
                        "strOf requires exactly 2 arguments: strOf(span, raw), got {}",
                        args.len()
                    ),
                });
            }
            let span = self.lower_expr(func, &args[0])?;
            let raw_is_str = self.expr_is_string_full(&args[1]);
            let raw = self.lower_expr(func, &args[1])?;

            // start = taida_pack_get(span, hash("start"))
            let start_hash_var = func.alloc_var();
            let start_hash = crate::codegen::lower::simple_hash("start") as i64;
            func.push(IrInst::ConstInt(start_hash_var, start_hash));
            let start_val = func.alloc_var();
            func.push(IrInst::Call(
                start_val,
                "taida_pack_get".to_string(),
                vec![span, start_hash_var],
            ));

            // len = taida_pack_get(span, hash("len"))
            let len_hash_var = func.alloc_var();
            let len_hash = crate::codegen::lower::simple_hash("len") as i64;
            func.push(IrInst::ConstInt(len_hash_var, len_hash));
            let len_val = func.alloc_var();
            func.push(IrInst::Call(
                len_val,
                "taida_pack_get".to_string(),
                vec![span, len_hash_var],
            ));

            // end = taida_int_add(start, len)
            let end_val = func.alloc_var();
            func.push(IrInst::Call(
                end_val,
                "taida_int_add".to_string(),
                vec![start_val, len_val],
            ));

            // raw_bytes = (raw_is_str ? utf8_encode_mold(raw) lax-default to "" : raw)
            let raw_bytes = if raw_is_str {
                let enc_lax = func.alloc_var();
                func.push(IrInst::Call(
                    enc_lax,
                    "taida_utf8_encode_mold".to_string(),
                    vec![raw],
                ));
                let empty_bytes_default = func.alloc_var();
                func.push(IrInst::ConstStr(empty_bytes_default, String::new()));
                let bytes_from_str = func.alloc_var();
                func.push(IrInst::Call(
                    bytes_from_str,
                    "taida_lax_get_or_default".to_string(),
                    vec![enc_lax, empty_bytes_default],
                ));
                bytes_from_str
            } else {
                raw
            };

            // bytes = taida_slice_mold(raw_bytes, start, end)
            let bytes_val = func.alloc_var();
            func.push(IrInst::Call(
                bytes_val,
                "taida_slice_mold".to_string(),
                vec![raw_bytes, start_val, end_val],
            ));

            // lax = taida_utf8_decode_mold(bytes)
            let lax_val = func.alloc_var();
            func.push(IrInst::Call(
                lax_val,
                "taida_utf8_decode_mold".to_string(),
                vec![bytes_val],
            ));

            // str = taida_lax_get_or_default(lax, "")
            let default_val = func.alloc_var();
            func.push(IrInst::ConstStr(default_val, String::new()));
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_lax_get_or_default".to_string(),
                vec![lax_val, default_val],
            ));
            return Ok(Some(result));
        }

        if name == "typeof" || name == "typeOf" {
            if args.len() != 1 {
                return Err(LowerError {
                    message: format!("typeof requires exactly 1 argument, got {}", args.len()),
                });
            }
            let arg = &args[0];
            let arg_var = self.lower_expr(func, arg)?;
            // Pass compile-time type tag as second argument to disambiguate
            // Int/Float/Bool which are all i64 at runtime
            let tag = self.expr_type_tag(arg);
            let tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(tag_var, tag));
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_typeof".to_string(),
                vec![arg_var, tag_var],
            ));
            return Ok(Some(result));
        }

        if name == "nowMs" {
            if !args.is_empty() {
                return Err(LowerError {
                    message: format!("nowMs requires 0 arguments, got {}", args.len()),
                });
            }
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_time_now_ms".to_string(),
                vec![],
            ));
            return Ok(Some(result));
        }

        if name == "sleep" {
            if args.len() != 1 {
                return Err(LowerError {
                    message: format!("sleep requires exactly 1 argument (ms), got {}", args.len()),
                });
            }
            let ms = self.lower_expr(func, &args[0])?;
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_time_sleep".to_string(),
                vec![ms],
            ));
            return Ok(Some(result));
        }

        if name == "range" {
            if args.len() < 2 || args.len() > 3 {
                return Err(LowerError {
                    message: format!("range requires 2 or 3 arguments, got {}", args.len()),
                });
            }
            let start = self.lower_expr(func, &args[0])?;
            let end = self.lower_expr(func, &args[1])?;
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_range".to_string(),
                vec![start, end],
            ));
            return Ok(Some(result));
        }

        if name == "allEnv" || name == "argv" {
            if !args.is_empty() {
                return Err(LowerError {
                    message: format!("{} requires 0 arguments, got {}", name, args.len()),
                });
            }
            let rt_name = if name == "allEnv" {
                "taida_os_all_env"
            } else {
                "taida_os_argv"
            };
            let result = func.alloc_var();
            func.push(IrInst::Call(result, rt_name.to_string(), vec![]));
            return Ok(Some(result));
        }

        // stdlib ランタイム関数呼び出し（std/math, std/io etc.）
        // Skip if the name is a net builtin that is shadowed by a parameter.
        if let Some(rt_name) = self.stdlib_runtime_funcs.get(name).cloned()
            && !self.is_net_builtin_shadowed(name)
        {
            // C12B-038: stdout/stderr `_with_tag` dispatch is
            // now centralised in `lower_stdout_with_tag` (see
            // `src/codegen/lower/tag_prop.rs`). The helper preserves
            // the B11-2b/B11B-004/C12-1/C12-11/C12B-016 semantics:
            // Str fast path + polymorphic dispatch + FieldAccess
            // single-eval + param_tag_vars Ident propagation.
            if (name == "stdout" || name == "stderr") && args.len() == 1 {
                return self
                    .lower_stdout_with_tag(func, name, &args[0], rt_name)
                    .map(Some);
            }
            // `hashMap(entries)`: built from a list of `@(key, value)`
            // packs or a pack's fields at run time.
            if name == "hashMap" && args.len() == 1 {
                return self.lower_hashmap_from(func, &args[0]).map(Some);
            }
            // stdin: optional prompt arg (pass empty string if none)
            if name == "stdin" {
                let prompt_var = if let Some(arg) = args.first() {
                    let arg_var = self.lower_expr(func, arg)?;
                    self.convert_to_string(func, arg, arg_var)?
                } else {
                    let empty = func.alloc_var();
                    func.push(IrInst::ConstStr(empty, String::new()));
                    empty
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, vec![prompt_var]));
                return Ok(Some(result));
            }
            // C20-2: stdinLine — mirrors `stdin` arity / prompt
            // stringification but the runtime returns Async[Lax[Str]]
            // rather than a bare Str. The prompt is optional and is
            // display-stringified when present so non-Str prompts
            // parity-match the interpreter / JS behaviour.
            if name == "stdinLine" {
                let prompt_var = if let Some(arg) = args.first() {
                    let arg_var = self.lower_expr(func, arg)?;
                    self.convert_to_string(func, arg, arg_var)?
                } else {
                    let empty = func.alloc_var();
                    func.push(IrInst::ConstStr(empty, String::new()));
                    empty
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, vec![prompt_var]));
                return Ok(Some(result));
            }
            // v5: wsClose(ws) or wsClose(ws, code) — always pass 2 args.
            // When code is omitted, pass 0 (C runtime treats 0 as default 1000).
            if name == "wsClose" {
                let ws_var = if let Some(arg) = args.first() {
                    self.lower_expr(func, arg)?
                } else {
                    return Err(LowerError {
                        message: "wsClose requires at least 1 argument (ws)".to_string(),
                    });
                };
                let code_var = if let Some(arg) = args.get(1) {
                    self.lower_expr(func, arg)?
                } else {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 0)); // 0 = use default 1000
                    v
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, vec![ws_var, code_var]));
                return Ok(Some(result));
            }
            // jsonEncode/jsonPretty: pass value directly (no auto-conversion)
            // The C runtime handles polymorphic serialization
            if name == "jsonEncode" || name == "jsonPretty" {
                let val_var = if let Some(arg) = args.first() {
                    self.reject_untyped_addon_value(func, arg)?;
                    self.lower_expr(func, arg)?
                } else {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    zero
                };
                // The argument's static type decides how each part is
                // written wherever the checker knows it.
                if let Some(desc) = args
                    .first()
                    .and_then(|arg| self.json_encode_descriptor(arg))
                {
                    self.note_json_schema_field_names(&desc);
                    let desc_var = func.alloc_var();
                    func.push(IrInst::ConstStr(desc_var, desc));
                    let typed_fn = if name == "jsonEncode" {
                        "taida_json_encode_desc"
                    } else {
                        "taida_json_pretty_desc"
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        typed_fn.to_string(),
                        vec![val_var, desc_var],
                    ));
                    return Ok(Some(result));
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, vec![val_var]));
                return Ok(Some(result));
            }
            // C12-6a: Regex(pattern, flags?) — the C entry
            // `taida_regex_new(pattern, flags)` takes 2 Str args;
            // when `flags` is omitted we pass an empty string so
            // the ABI signature stays fixed.
            if name == "Regex" {
                if args.is_empty() || args.len() > 2 {
                    return Err(LowerError {
                        message: format!(
                            "Regex requires 1 or 2 arguments (pattern, flags?), got {}",
                            args.len()
                        ),
                    });
                }
                let pattern_var = self.lower_expr(func, &args[0])?;
                let flags_var = if let Some(arg) = args.get(1) {
                    self.lower_expr(func, arg)?
                } else {
                    let empty = func.alloc_var();
                    func.push(IrInst::ConstStr(empty, String::new()));
                    empty
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, vec![pattern_var, flags_var]));
                return Ok(Some(result));
            }
            let mut arg_vars = Vec::new();
            for arg in args {
                let var = self.lower_expr(func, arg)?;
                arg_vars.push(var);
            }
            let result = func.alloc_var();
            func.push(IrInst::Call(result, rt_name, arg_vars));
            return Ok(Some(result));
        }
        Ok(None)
    }

    pub(crate) fn lower_func_call(
        &mut self,
        func: &mut IrFunction,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        // Empty-slot partial application: if any arg is Hole, emit a lambda.
        // Note: Old `_` (Placeholder) partial application is rejected by checker
        // (E1502) before reaching codegen. Only Hole-based syntax `f(5, )` is handled.
        let has_hole = args.iter().any(|a| matches!(a, Expr::Hole(_)));
        if has_hole {
            return self.lower_partial_application(func, callee, args);
        }

        if let Expr::Ident(name, span) = callee {
            // A name the program binds itself — a parameter, a local, a
            // user function, a top-level variable — shadows the prelude /
            // core-package builtin of the same name: the prelude behaves as
            // the outermost scope, which is how the checker types the call.
            let locally_bound = Self::is_local_binding(func, name);
            // `Pilot()`: a declared type called with no fields is the instance
            // of its defaults, as `Pilot(...)` with every field omitted is.
            if args.is_empty()
                && !locally_bound
                && !self.user_funcs.contains(name)
                && self.inner_func_symbol(name).is_none()
                && !self.top_level_vars.contains(name)
                && self.type_field_defs.contains_key(name)
            {
                let inst = Expr::TypeInst(
                    name.clone(),
                    Vec::new(),
                    span.clone()
                        .with_node_id(crate::parser::SYNTHETIC_NODE_ID_BASE),
                );
                return self.lower_expr(func, &inst);
            }
            // A local function compiled as a closure calling itself: re-enter
            // its own symbol with the environment it was called with.
            if !locally_bound
                && let Some((self_name, symbol)) = self.closure_self.clone()
                && self_name == *name
            {
                let env = func.alloc_var();
                func.push(IrInst::UseVar(env, "__env".to_string()));
                let mut arg_vars = vec![env];
                for arg in args {
                    arg_vars.push(self.lower_expr(func, arg)?);
                }
                let result = func.alloc_var();
                func.push(IrInst::CallUser(result, symbol, arg_vars));
                return Ok(result);
            }
            let local_function = !locally_bound && self.inner_func_symbol(name).is_some();
            if !locally_bound
                && !local_function
                && !self.user_funcs.contains(name)
                && !self.top_level_vars.contains(name)
                && let Some(result) = self.lower_builtin_func_call(func, name, args)?
            {
                return Ok(result);
            }

            // RC2.5: an addon import is emitted as a single
            // `taida_addon_call` runtime invocation; argv is packed into a
            // tiny per-call Taida Pack so the C dispatcher can read
            // positional arguments with tags. The same helper is reused by
            // the `Expr::MoldInst` lowering path (Phase 2 / RC2.5-2a), so
            // any user-side spelling of `foo()` or `Foo[]()` routes through
            // identical IR.
            if !locally_bound && !local_function && self.addon_func_refs.contains_key(name) {
                return self.emit_addon_call(func, name, args);
            }

            // ユーザー定義関数呼び出し（引数・局所変数が同名なら、そちらを呼ぶ）
            if local_function || (!locally_bound && self.user_funcs.contains(name)) {
                // NB-14: Stack-based arg tag propagation. Only push/pop when at least
                // one argument needs tag propagation (avoids overhead for simple calls
                // like mutual recursion where all args are Int).
                // C12B-022: Callees that do `TypeIs[param, :PrimitiveType]()` need
                // their callers to emit tags for ALL args (including the INT=0
                // default) so the runtime primitive-tag-match helper gets a
                // concrete tag instead of UNKNOWN(-1).
                let needs_param_check_full = self.param_type_check_funcs.contains(name);
                let needs_tags = needs_param_check_full || self.needs_call_arg_tags(args);
                if needs_tags {
                    let push_dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        push_dummy,
                        "taida_push_call_tags".to_string(),
                        vec![],
                    ));
                    // Pre-lower tags: set tags for args with compile-time known types
                    self.emit_call_arg_tags_full(func, args, needs_param_check_full);
                }
                // Lower args (may include nested CallUser that populate return_tag_vars).
                // Clear tail flag during arg lowering so nested CallUser emit get_return_tag.
                let saved_tail = self.in_tail_call_return;
                self.in_tail_call_return = false;
                let param_annotations: Vec<Option<TypeExpr>> = self
                    .func_param_defs
                    .get(name)
                    .map(|params| params.iter().map(|p| p.type_annotation.clone()).collect())
                    .unwrap_or_default();
                let mut explicit_arg_vars = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    let var = self.lower_expr(func, arg)?;
                    let declared = param_annotations.get(i).cloned().flatten();
                    explicit_arg_vars.push(self.coerce_to_declared(
                        func,
                        declared.as_ref(),
                        arg,
                        var,
                    ));
                }
                self.in_tail_call_return = saved_tail;
                if needs_tags {
                    // Post-lower tags: set tags for args whose type came from a call's return tag
                    self.emit_post_lower_arg_tags(func, args, &explicit_arg_vars);
                }
                let (mangled, arg_vars) =
                    match self.default_arity_entry_for(name, explicit_arg_vars.len()) {
                        Some(entry) => (entry, explicit_arg_vars),
                        None => (
                            self.resolve_user_func_symbol(name),
                            self.lower_user_call_effective_args_from_vars(
                                func,
                                name,
                                explicit_arg_vars,
                            )?,
                        ),
                    };
                let result = func.alloc_var();
                func.push(IrInst::CallUser(result, mangled, arg_vars));
                // NB-14: Capture return type tag from callee (skip in tail position for TCO)
                if !self.in_tail_call_return {
                    let return_tag = func.alloc_var();
                    func.push(IrInst::Call(
                        return_tag,
                        "taida_get_return_tag".to_string(),
                        vec![],
                    ));
                    self.return_tag_vars.insert(result, return_tag);
                }
                if needs_tags {
                    let pop_dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        pop_dummy,
                        "taida_pop_call_tags".to_string(),
                        vec![],
                    ));
                }
                return Ok(result);
            }

            // ラムダ変数経由の呼び出し
            // ラムダ変数 or 未知の変数呼び出し:
            // 全ラムダはクロージャ構造体として生成されるため、
            // lambda_vars に登録されているかどうかに関わらず
            // 統一的に CallIndirect で間接呼び出しする。
            {
                let mut arg_vars = Vec::new();
                for arg in args {
                    let var = self.lower_expr(func, arg)?;
                    arg_vars.push(var);
                }
                // A local function compiled as a closure, called with
                // trailing arguments omitted from the body that defines it.
                if let Some(symbol) = self.local_closure_symbol(name)
                    && let Some(params) = self.func_param_defs.get(&symbol).cloned()
                    && arg_vars.len() < params.len()
                {
                    arg_vars =
                        self.complete_args_with_param_defaults(func, name, &params, arg_vars)?;
                }
                // A name bound to a user function (`f <= add3`), called with
                // trailing arguments omitted: that function's default entry
                // completes the call.
                if let Some(target) = self.alias_target_function(name) {
                    if let Some(symbol) = self.local_closure_symbol(&target) {
                        if let Some(params) = self.func_param_defs.get(&symbol).cloned()
                            && arg_vars.len() < params.len()
                        {
                            arg_vars = self.complete_args_with_param_defaults(
                                func, &target, &params, arg_vars,
                            )?;
                        }
                    } else if let Some(entry) =
                        self.default_arity_entry_for(&target, arg_vars.len())
                    {
                        let result = func.alloc_var();
                        func.push(IrInst::CallUser(result, entry, arg_vars));
                        return Ok(result);
                    }
                }
                let closure_var = func.alloc_var();
                func.push(IrInst::UseVar(closure_var, name.clone()));
                let result = func.alloc_var();
                func.push(IrInst::CallIndirect(result, closure_var, arg_vars));
                return Ok(result);
            }
        }

        // 非 Ident の callee: ラムダ式や関数呼び出し結果（IIFE, カリー化）等
        // callee を評価し、結果をクロージャ/関数ポインタとして間接呼び出しする
        {
            let callee_var = self.lower_expr(func, callee)?;

            // callee がキャプチャなしラムダ（FuncAddr）の場合も
            // CallIndirect でクロージャとして呼ぶと壊れるため、
            // ここでは統一的に CallIndirect を使う。
            // ただし FuncAddr の場合はクロージャ構造体ではないので、
            // ラムダ式の場合はキャプチャの有無で分岐する。
            if let Expr::Lambda(_, _, _) = callee {
                // IIFE: lower_lambda で既に FuncAddr または MakeClosure が生成済み
                // キャプチャなしの場合は直接呼び出しが必要
                // → lower_lambda の戻り値が FuncAddr ならユーザー関数呼び出し
                //   MakeClosure なら間接呼び出し
                // 判定: lambda_funcs の最後に追加された関数の名前を使う
                if let Some(last_fn) = self.lambda_funcs.last() {
                    let lambda_name = last_fn.name.clone();
                    // キャプチャありかどうかは FuncAddr vs MakeClosure で判定
                    // → func.body の最後の命令を見る
                    let is_closure = func.body.iter().rev().any(
                        |inst| matches!(inst, IrInst::MakeClosure(v, _, _) if *v == callee_var),
                    );
                    if is_closure {
                        // NB-14: Stack-based arg tag propagation for IIFE closure calls
                        // (symmetric with named-function path at lower.rs:3082)
                        let needs_tags = self.needs_call_arg_tags(args);
                        if needs_tags {
                            let push_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                push_dummy,
                                "taida_push_call_tags".to_string(),
                                vec![],
                            ));
                            self.emit_call_arg_tags(func, args);
                        }
                        let saved_tail = self.in_tail_call_return;
                        self.in_tail_call_return = false;
                        let mut arg_vars = Vec::new();
                        for arg in args {
                            let var = self.lower_expr(func, arg)?;
                            arg_vars.push(var);
                        }
                        self.in_tail_call_return = saved_tail;
                        if needs_tags {
                            self.emit_post_lower_arg_tags(func, args, &arg_vars);
                        }
                        let result = func.alloc_var();
                        func.push(IrInst::CallIndirect(result, callee_var, arg_vars));
                        // NB-14: Capture return type tag from lambda (skip in tail position)
                        if !self.in_tail_call_return {
                            let return_tag = func.alloc_var();
                            func.push(IrInst::Call(
                                return_tag,
                                "taida_get_return_tag".to_string(),
                                vec![],
                            ));
                            self.return_tag_vars.insert(result, return_tag);
                        }
                        if needs_tags {
                            let pop_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                pop_dummy,
                                "taida_pop_call_tags".to_string(),
                                vec![],
                            ));
                        }
                        return Ok(result);
                    } else {
                        // NB-14: Stack-based arg tag propagation for direct lambda calls
                        // (symmetric with named-function path at lower.rs:3082)
                        let needs_tags = self.needs_call_arg_tags(args);
                        if needs_tags {
                            let push_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                push_dummy,
                                "taida_push_call_tags".to_string(),
                                vec![],
                            ));
                            self.emit_call_arg_tags(func, args);
                        }
                        // Lower args with tail flag cleared for nested CallUser
                        let saved_tail = self.in_tail_call_return;
                        self.in_tail_call_return = false;
                        let mut explicit_arg_vars = Vec::with_capacity(args.len());
                        for arg in args {
                            explicit_arg_vars.push(self.lower_expr(func, arg)?);
                        }
                        self.in_tail_call_return = saved_tail;
                        if needs_tags {
                            self.emit_post_lower_arg_tags(func, args, &explicit_arg_vars);
                        }
                        let result = func.alloc_var();
                        func.push(IrInst::CallUser(result, lambda_name, explicit_arg_vars));
                        // NB-14: Capture return type tag from callee (skip in tail position)
                        if !self.in_tail_call_return {
                            let return_tag = func.alloc_var();
                            func.push(IrInst::Call(
                                return_tag,
                                "taida_get_return_tag".to_string(),
                                vec![],
                            ));
                            self.return_tag_vars.insert(result, return_tag);
                        }
                        if needs_tags {
                            let pop_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                pop_dummy,
                                "taida_pop_call_tags".to_string(),
                                vec![],
                            ));
                        }
                        return Ok(result);
                    }
                }
            }

            // その他の非Lambda callee: 先に引数をlower
            let mut arg_vars = Vec::new();
            for arg in args {
                let var = self.lower_expr(func, arg)?;
                arg_vars.push(var);
            }

            // その他: 関数呼び出し結果やフィールドアクセス結果を間接呼び出し
            let result = func.alloc_var();
            func.push(IrInst::CallIndirect(result, callee_var, arg_vars));
            Ok(result)
        }
    }

    pub(super) fn lower_debug_call(
        &mut self,
        func: &mut IrFunction,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.is_empty() {
            return Err(LowerError {
                message: "debug() requires at least one argument".to_string(),
            });
        }

        // `debug(x)` prints `x` and yields `x` itself.
        let mut last_value = None;
        for arg in args {
            let arg_var = self.lower_expr(func, arg)?;
            let printed = func.alloc_var();
            if self.expr_type_tag(arg) == crate::codegen::tag_prop::TAG_CLOSURE {
                // `<function name>`: only the static type tells a named
                // function's WASM table index apart from an Int.
                let shown = func.alloc_var();
                func.push(IrInst::Call(
                    shown,
                    "taida_function_to_string".to_string(),
                    vec![arg_var],
                ));
                func.push(IrInst::Call(
                    printed,
                    "taida_debug_str".to_string(),
                    vec![shown],
                ));
            } else {
                let runtime_fn = self.debug_fn_for_expr(arg);
                func.push(IrInst::Call(printed, runtime_fn, vec![arg_var]));
            }
            last_value = Some(arg_var);
        }
        Ok(last_value.unwrap())
    }

    pub(super) fn debug_fn_for_expr(&self, expr: &Expr) -> String {
        if let Some(tag) = self.checked_tag(expr) {
            use crate::codegen::tag_prop::*;
            return match tag {
                TAG_INT => "taida_debug_int",
                TAG_FLOAT => "taida_debug_float",
                TAG_BOOL => "taida_debug_bool",
                TAG_STR => "taida_debug_str",
                _ => "taida_debug_polymorphic",
            }
            .to_string();
        }
        match expr {
            Expr::IntLit(..) => "taida_debug_int".to_string(),
            Expr::FloatLit(..) => "taida_debug_float".to_string(),
            Expr::StringLit(..) => "taida_debug_str".to_string(),
            Expr::BoolLit(..) => "taida_debug_bool".to_string(),
            Expr::BinaryOp(
                _,
                BinOp::Eq
                | BinOp::NotEq
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::And
                | BinOp::Or,
                _,
                _,
            ) => "taida_debug_bool".to_string(),
            Expr::BinaryOp(..) => "taida_debug_int".to_string(),
            Expr::UnaryOp(UnaryOp::Not, _, _) => "taida_debug_bool".to_string(),
            Expr::UnaryOp(UnaryOp::Neg, _, _) => "taida_debug_int".to_string(),
            Expr::Ident(name, _) => {
                if self.float_vars.contains(name) {
                    "taida_debug_float".to_string()
                } else if self.string_vars.contains(name) {
                    "taida_debug_str".to_string()
                } else if self.bool_vars.contains(name) {
                    "taida_debug_bool".to_string()
                } else if self.pack_vars.contains(name)
                    || self.list_vars.contains(name)
                    || self.closure_vars.contains(name)
                {
                    "taida_debug_polymorphic".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
            Expr::MethodCall(_, method, _, _) => {
                if self.expr_is_bool(expr) {
                    "taida_debug_bool".to_string()
                } else if matches!(method.as_str(), "toString" | "toStr") {
                    "taida_debug_str".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
            Expr::FuncCall(callee, _, _) => {
                if self.expr_is_bool(expr) {
                    return "taida_debug_bool".to_string();
                }
                if let Expr::Ident(name, _) = callee.as_ref() {
                    if self.string_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_str".to_string();
                    }
                    if self.float_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_float".to_string();
                    }
                }
                "taida_debug_int".to_string()
            }
            Expr::FieldAccess(receiver, _, _) => {
                // Field access on a pack: use polymorphic to_string + debug_str
                // because field types are not always tracked
                if self.expr_is_string_full(expr) {
                    "taida_debug_str".to_string()
                } else if self.expr_returns_float(expr) {
                    "taida_debug_float".to_string()
                } else if self.expr_is_bool(expr) {
                    "taida_debug_bool".to_string()
                } else if self.expr_is_pack(receiver) || self.expr_is_list(receiver) {
                    // Pack field or list: could be any type, use polymorphic
                    "taida_debug_polymorphic".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
            // Catch-all: use type detection helpers before defaulting to int
            _ => {
                if self.expr_is_string_full(expr) {
                    "taida_debug_str".to_string()
                } else if self.expr_returns_float(expr) {
                    "taida_debug_float".to_string()
                } else if self.expr_is_bool(expr) {
                    "taida_debug_bool".to_string()
                } else if self.expr_is_pack(expr) || self.expr_is_list(expr) {
                    "taida_debug_polymorphic".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
        }
    }

    /// Whether evaluating `expr` has no effect beyond producing its value
    /// (no call, nothing printed or thrown), so evaluating it early, or
    /// not at all, cannot be observed.
    pub(super) fn expr_is_effect_free(expr: &Expr) -> bool {
        match expr {
            Expr::IntLit(..)
            | Expr::FloatLit(..)
            | Expr::StringLit(..)
            | Expr::BoolLit(..)
            | Expr::Ident(..) => true,
            Expr::BinaryOp(lhs, _, rhs, _) => {
                Self::expr_is_effect_free(lhs) && Self::expr_is_effect_free(rhs)
            }
            Expr::UnaryOp(_, inner, _) | Expr::FieldAccess(inner, _, _) => {
                Self::expr_is_effect_free(inner)
            }
            _ => false,
        }
    }

    /// `a && b` / `a || b` whose right operand could have an effect: the
    /// right operand runs only when the left one does not decide the result.
    fn lower_short_circuit(
        &mut self,
        func: &mut IrFunction,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<IrVar, LowerError> {
        use crate::codegen::ir::CondArm as IrCondArm;

        let is_and = matches!(op, BinOp::And);
        let runtime_fn = if is_and {
            "taida_bool_and"
        } else {
            "taida_bool_or"
        };
        let lhs_var = self.lower_expr(func, lhs)?;

        // The arm that evaluates the right operand combines it with the
        // left one through the usual runtime helper, so the result is the
        // same Bool the eager form gives.
        let saved = std::mem::take(&mut func.body);
        let rhs_var = self.lower_expr(func, rhs)?;
        let combined = func.alloc_var();
        func.push(IrInst::Call(
            combined,
            runtime_fn.to_string(),
            vec![lhs_var, rhs_var],
        ));
        let rhs_insts = std::mem::replace(&mut func.body, saved);

        let saved = std::mem::take(&mut func.body);
        let decided = func.alloc_var();
        func.push(IrInst::ConstBool(decided, !is_and));
        let decided_insts = std::mem::replace(&mut func.body, saved);

        let (taken, taken_var, other, other_var) = if is_and {
            (rhs_insts, combined, decided_insts, decided)
        } else {
            (decided_insts, decided, rhs_insts, combined)
        };
        let result = func.alloc_var();
        func.push(IrInst::CondBranch(
            result,
            vec![
                IrCondArm {
                    condition: Some(lhs_var),
                    body: taken,
                    result: taken_var,
                },
                IrCondArm {
                    condition: None,
                    body: other,
                    result: other_var,
                },
            ],
        ));
        Ok(result)
    }

    pub(super) fn lower_binary_op(
        &mut self,
        func: &mut IrFunction,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<IrVar, LowerError> {
        if matches!(op, BinOp::And | BinOp::Or) && !Self::expr_is_effect_free(rhs) {
            return self.lower_short_circuit(func, lhs, op, rhs);
        }
        if !matches!(op, BinOp::And | BinOp::Or) {
            self.reject_untyped_addon_value(func, lhs)?;
            self.reject_untyped_addon_value(func, rhs)?;
        }
        let lhs_var = self.lower_expr(func, lhs)?;
        let rhs_var = self.lower_expr(func, rhs)?;

        let lhs_is_str = self.expr_is_string_full(lhs);
        let rhs_is_str = self.expr_is_string_full(rhs);

        if matches!(op, BinOp::Add) {
            if lhs_is_str && rhs_is_str {
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_concat".to_string(),
                    vec![lhs_var, rhs_var],
                ));
                return Ok(result);
            }
            if lhs_is_str || rhs_is_str {
                let lhs_str = self.convert_to_string(func, lhs, lhs_var)?;
                let rhs_str = self.convert_to_string(func, rhs, rhs_var)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_concat".to_string(),
                    vec![lhs_str, rhs_str],
                ));
                return Ok(result);
            }
        }

        if matches!(op, BinOp::Concat) && (lhs_is_str || rhs_is_str) {
            let lhs_str = self.convert_to_string(func, lhs, lhs_var)?;
            let rhs_str = self.convert_to_string(func, rhs, rhs_var)?;
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_str_concat".to_string(),
                vec![lhs_str, rhs_str],
            ));
            return Ok(result);
        }

        let runtime_fn = match op {
            BinOp::Add => {
                if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    // Float arithmetic: use float add
                    "taida_float_add"
                } else if self.expr_type_is_unknown(lhs) || self.expr_type_is_unknown(rhs) {
                    // FL-16: untyped operand (e.g. function param without annotation)
                    // → use polymorphic add that dispatches at runtime
                    "taida_poly_add"
                } else {
                    "taida_int_add"
                }
            }
            BinOp::Sub => {
                if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_sub"
                } else {
                    "taida_int_sub"
                }
            }
            BinOp::Mul => {
                if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_mul"
                } else {
                    "taida_int_mul"
                }
            }
            // BinOp::Div and BinOp::Mod removed — use Div[x, y]() and Mod[x, y]() molds
            // F54 numeric parity: a Float on either side routes to the
            // f64 comparison helpers (covering Int↔Float cross-type via
            // _to_double). Raw i64 comparison of f64 bit patterns made
            // `3 == 3.0` false and inverted negative-float ordering.
            BinOp::Eq => {
                // Value-tag track: an operand carrying a runtime shadow
                // kind (an unmolded Lax payload from a possibly mixed
                // list) must not take a static fast path — its actual
                // kind is only known at runtime, so the comparison goes
                // through the tagged poly engine which reads the shadow.
                if self.operand_shadow_kind(lhs).is_some()
                    || self.operand_shadow_kind(rhs).is_some()
                    // Value-tag track: two statically-known lists compare
                    // structurally (kind-aware element walk) instead of
                    // by raw pointer identity, matching the interpreter's
                    // Value::eq. The untyped poly fallback below keeps the
                    // historical raw comparison for dynamic operands.
                    || (self.expr_is_list(lhs) && self.expr_is_list(rhs))
                    || self.eq_needs_kind_aware_compare(lhs, rhs)
                    || (self.collection_kind(lhs).is_some() && self.collection_kind(rhs).is_some())
                {
                    "taida_poly_eq_tagged"
                } else if self.eq_operands_are_ints(lhs, rhs) {
                    "taida_int_eq"
                } else if lhs_is_str || rhs_is_str {
                    "taida_str_eq"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_eq"
                } else if self.expr_is_bool(lhs)
                    || self.expr_is_bool(rhs)
                    || matches!(lhs, Expr::IntLit(_, _))
                    || matches!(rhs, Expr::IntLit(_, _))
                {
                    "taida_int_eq"
                } else {
                    "taida_poly_eq"
                }
            }
            BinOp::NotEq => {
                if self.operand_shadow_kind(lhs).is_some()
                    || self.operand_shadow_kind(rhs).is_some()
                    || (self.expr_is_list(lhs) && self.expr_is_list(rhs))
                    || self.eq_needs_kind_aware_compare(lhs, rhs)
                    || (self.collection_kind(lhs).is_some() && self.collection_kind(rhs).is_some())
                {
                    "taida_poly_neq_tagged"
                } else if self.eq_operands_are_ints(lhs, rhs) {
                    "taida_int_neq"
                } else if lhs_is_str || rhs_is_str {
                    "taida_str_neq"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_neq"
                } else if self.expr_is_bool(lhs)
                    || self.expr_is_bool(rhs)
                    || matches!(lhs, Expr::IntLit(_, _))
                    || matches!(rhs, Expr::IntLit(_, _))
                {
                    "taida_int_neq"
                } else {
                    "taida_poly_neq"
                }
            }
            // F62B-017: Str operands order lexicographically (interpreter
            // semantics). Without the string arm these fell through to the
            // i64 comparison and ordered by pointer value.
            BinOp::Lt => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_lt"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_lt"
                } else {
                    "taida_int_lt"
                }
            }
            BinOp::Gt => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_gt"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_gt"
                } else {
                    "taida_int_gt"
                }
            }
            BinOp::GtEq => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_gte"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_gte"
                } else {
                    "taida_int_gte"
                }
            }
            BinOp::And => "taida_bool_and",
            BinOp::Or => "taida_bool_or",
            BinOp::Concat => "taida_str_concat",
        };
        // Value-tag track: the tagged poly comparisons take four arguments
        // (each operand rides with its kind — a shadow IR variable when
        // the operand is an unmolded Lax payload, a compile-time EKIND
        // constant otherwise).
        if runtime_fn == "taida_poly_eq_tagged" || runtime_fn == "taida_poly_neq_tagged" {
            let lhs_kind = self.emit_eq_operand_ekind(func, lhs);
            let rhs_kind = self.emit_eq_operand_ekind(func, rhs);
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                runtime_fn.to_string(),
                vec![lhs_var, lhs_kind, rhs_var, rhs_kind],
            ));
            return Ok(result);
        }
        // When a float-family helper was selected, lift any operand that is
        // statically known to be Int to an f64 bit pattern at the call site
        // (`taida_int_to_float` — the conversion the runtime's own comment
        // always documented as "the lowering inserts a taida_int_to_float
        // call" but binary ops never emitted). The helpers previously
        // guessed the operand kind from the value via a ±2^20 magnitude
        // heuristic, which broke both comparison (`2000000 == 2000000.0` →
        // false) and arithmetic (`2000000 + 0.5` → 0.5) outside that
        // window. With the lift, every statically-typed operand arrives as
        // a genuine f64 bit pattern and the heuristic is only left covering
        // dynamically-typed operands.
        let (lhs_var, rhs_var) = if runtime_fn.starts_with("taida_float_") {
            (
                self.lift_static_int_to_f64(func, lhs, lhs_var),
                self.lift_static_int_to_f64(func, rhs, rhs_var),
            )
        } else {
            (lhs_var, rhs_var)
        };
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![lhs_var, rhs_var],
        ));
        Ok(result)
    }

    /// `==` / `!=` whose operands the checker typed as heap values (packs,
    /// lists, maps, sets, bytes, functions) or as different kinds: the
    /// interpreter compares them by value, kind first, so they go through
    /// the kind-aware structural comparison rather than a raw word compare.
    fn eq_needs_kind_aware_compare(&self, lhs: &Expr, rhs: &Expr) -> bool {
        use crate::codegen::tag_prop::*;
        let (Some(l), Some(r)) = (self.checked_tag(lhs), self.checked_tag(rhs)) else {
            return false;
        };
        let numeric = |t: i64| t == TAG_INT || t == TAG_FLOAT;
        let heap = |t: i64| matches!(t, TAG_PACK | TAG_LIST | TAG_CLOSURE);
        heap(l) || heap(r) || (l != r && !(numeric(l) && numeric(r)))
    }

    /// The runtime kind of a HashMap (7) or Set (8) operand, from the
    /// checker's type. Such values carry no static tag of their own (the
    /// shared pack tag would send their method calls down the pack path),
    /// so `==` names their kind for the runtime to compare them entry by
    /// entry.
    fn collection_kind(&self, expr: &Expr) -> Option<i64> {
        use crate::types::Type;
        match self.typed_expr_table.lookup(expr)? {
            Type::Named(name) | Type::Generic(name, _) if name == "HashMap" => Some(7),
            Type::Named(name) | Type::Generic(name, _) if name == "Set" => Some(8),
            _ => None,
        }
    }

    /// The kind entry an `==` / `!=` operand rides with.
    fn emit_eq_operand_ekind(&mut self, func: &mut IrFunction, expr: &Expr) -> IrVar {
        match self.collection_kind(expr) {
            Some(kind) => {
                let v = func.alloc_var();
                func.push(IrInst::ConstInt(v, kind));
                v
            }
            None => self.emit_operand_ekind(func, expr),
        }
    }

    /// `==` / `!=` between two values the checker typed as Int (or both as
    /// Bool): a plain word comparison.
    fn eq_operands_are_ints(&self, lhs: &Expr, rhs: &Expr) -> bool {
        use crate::codegen::tag_prop::*;
        matches!(
            (self.checked_tag(lhs), self.checked_tag(rhs)),
            (Some(TAG_INT), Some(TAG_INT)) | (Some(TAG_BOOL), Some(TAG_BOOL))
        )
    }

    /// Value-tag track: the shadow-kind variable name for an operand, when
    /// the operand is an identifier bound by a runtime-kind unmold.
    pub(crate) fn operand_shadow_kind(&self, expr: &Expr) -> Option<String> {
        if let Expr::Ident(name, _) = expr
            && self.shadow_kind_vars.contains(name)
        {
            return Some(format!("{HIDDEN}ekind_{name}"));
        }
        None
    }

    /// Emit the kind argument for one operand of a tagged runtime call
    /// (poly comparisons and the tagged Set/collection entry points): the
    /// shadow IR variable when one exists, otherwise the compile-time
    /// EKIND constant.
    pub(crate) fn emit_operand_ekind(&mut self, func: &mut IrFunction, expr: &Expr) -> IrVar {
        if let Some(shadow_name) = self.operand_shadow_kind(expr) {
            let v = func.alloc_var();
            func.push(IrInst::UseVar(v, shadow_name));
            v
        } else {
            let v = func.alloc_var();
            func.push(IrInst::ConstInt(v, self.expr_ekind(expr)));
            v
        }
    }

    /// Lift an operand of a float-family binary op to an f64 bit pattern
    /// when (and only when) it is statically known to be an Int. Float
    /// operands already carry f64 bits, dynamically-typed operands keep the
    /// runtime-side dispatch, and Bool/Str can only reach a float helper
    /// through paths the checker rejects, so they are left untouched.
    /// Convert `var` (the value of `expr`) to the representation a slot
    /// declared as `declared` expects. The checker accepts an Int where a
    /// Float is declared; the value is converted at the boundary so that
    /// the arithmetic, comparisons and formatting inside see a Float.
    pub(super) fn coerce_to_declared(
        &mut self,
        func: &mut IrFunction,
        declared: Option<&TypeExpr>,
        expr: &Expr,
        var: IrVar,
    ) -> IrVar {
        let declared_float = matches!(declared, Some(TypeExpr::Named(n)) if n == "Float");
        if !declared_float || self.checked_tag(expr) != Some(crate::codegen::tag_prop::TAG_INT) {
            return var;
        }
        let lifted = func.alloc_var();
        func.push(IrInst::Call(
            lifted,
            "taida_int_to_float".to_string(),
            vec![var],
        ));
        lifted
    }

    fn lift_static_int_to_f64(&mut self, func: &mut IrFunction, expr: &Expr, var: IrVar) -> IrVar {
        // Only an operand known to be an Int is lifted. "Not known to be
        // anything else" is no evidence: a value taken out of an untyped
        // HashMap may be a Float, and lifting its bit pattern as an Int
        // would read it as a huge number.
        if self.expr_returns_float(expr) || !self.expr_is_int(expr) {
            return var;
        }
        let lifted = func.alloc_var();
        func.push(IrInst::Call(
            lifted,
            "taida_int_to_float".to_string(),
            vec![var],
        ));
        lifted
    }

    pub(super) fn lower_unary_op(
        &mut self,
        func: &mut IrFunction,
        op: &UnaryOp,
        operand: &Expr,
    ) -> Result<IrVar, LowerError> {
        if matches!(op, UnaryOp::Neg) {
            self.reject_untyped_addon_value(func, operand)?;
        }
        let operand_var = self.lower_expr(func, operand)?;
        let runtime_fn = match op {
            UnaryOp::Neg => {
                if self.expr_returns_float(operand) {
                    "taida_float_neg"
                } else {
                    "taida_int_neg"
                }
            }
            UnaryOp::Not => "taida_bool_not",
        };
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![operand_var],
        ));
        Ok(result)
    }

    /// パイプライン: `a => f(_) => g(_)` → 各段の結果を次の引数に
    ///
    /// An intermediate `=> name` step (where `name` is not a
    /// user-defined function or known builtin) acts as a
    /// **bind-and-forward**: the current value is bound and passed through
    /// unchanged, and steps that explicitly reference the bound `name` skip the
    /// classic auto-injection of `prev_result` as an extra argument. The bind
    /// targets a *fresh synthetic name* (not `name` itself) so an outer
    /// same-named variable keeps its value across the pipeline, matching the
    /// interpreter's child-scope binding.
    pub(super) fn lower_pipeline(
        &mut self,
        func: &mut IrFunction,
        exprs: &[Expr],
    ) -> Result<IrVar, LowerError> {
        self.lower_pipeline_in(func, exprs, false)
    }

    /// `tail`: the pipeline is in tail position, so a last stage that calls a
    /// function with the piped value (`x => f(a, _)`) is lowered as that call
    /// in tail position (a self call becomes a `TailCall`), as the interpreter
    /// evaluates it.
    fn lower_pipeline_in(
        &mut self,
        func: &mut IrFunction,
        exprs: &[Expr],
        tail: bool,
    ) -> Result<IrVar, LowerError> {
        if exprs.is_empty() {
            return Err(LowerError {
                message: "empty pipeline".to_string(),
            });
        }

        // 最初の式を評価
        let mut current = self.lower_expr(func, &exprs[0])?;
        let last_idx = exprs.len().saturating_sub(1);
        let mut bound_names: Vec<String> = Vec::new();
        // F57B-007: each `=> name` bind is lowered to a *fresh synthetic* name
        // rather than `name` itself, so the binding never overwrites an outer
        // same-named variable's slot (the interpreter binds `=> name` in a child
        // scope; a native/WASM `DefVar(name, ...)` clobbered the outer value — a
        // parity violation). `bind_renames` maps each bound `name` to its
        // synthetic; steps consuming the binding are rewritten to read the
        // synthetic, while the outer `name` keeps its value/kind afterward.
        let mut bind_renames: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for (step_idx, expr) in exprs[1..].iter().enumerate() {
            let i = step_idx + 1;
            // Intermediate `=> name` bind-and-forward (a name holding a
            // function value is applied instead, as the interpreter does)
            if i < last_idx
                && let Expr::Ident(name, _) = expr
                && !self.is_native_pipeline_callable_ident(name)
                && self.checked_tag(expr) != Some(crate::codegen::tag_prop::TAG_CLOSURE)
            {
                // Bind to a fresh synthetic so the outer `name` is untouched.
                let synthetic = format!("{HIDDEN}pipe_bind_{}_{}", self.lambda_counter, name);
                self.lambda_counter += 1;
                func.push(IrInst::DefVar(synthetic.clone(), current));
                // Carry the forwarded value's static kind onto the synthetic so
                // a later `=> name => ... _.toString()` / `... name.toString()`
                // still dispatches on the right kind. The previous stage's expr
                // is renamed through earlier binds first, so a chained forward
                // (`true => a => b => ...`) propagates the kind stage by stage.
                let prev_renamed = rewrite_idents(&exprs[i - 1], &bind_renames);
                self.track_forwarded_pipe_type(&synthetic, &prev_renamed);
                bound_names.push(name.clone());
                bind_renames.insert(name.clone(), synthetic);
                continue;
            }
            // Step consumes a pipeline-scope binding explicitly → evaluate as
            // written (no auto-inject), with each bound name rewritten to its
            // synthetic so the binding — not an outer same-named variable — is read.
            if !bound_names.is_empty() && native_expr_references_any_name(expr, &bound_names) {
                // Review C-4: `_` has no injection value in an as-written
                // stage — reject for cross-backend agreement.
                if expr_count_placeholders(expr) > 0 {
                    return Err(LowerError {
                        message: "[E1543] A pipeline stage that references a `=> name` binding \
                                  is evaluated as written — `_` has no injection value there. \
                                  Use the bound name instead of `_`."
                            .to_string(),
                    });
                }
                let rewritten = rewrite_idents(expr, &bind_renames);
                current = self.lower_expr(func, &rewritten)?;
                continue;
            }
            // The last stage of a pipeline in tail position, calling a
            // function with the piped value: Rule 1 of `lower_pipeline_step`,
            // lowered in tail position.
            if tail && i == last_idx && is_piped_call(expr) {
                let prev = rewrite_idents(&exprs[i - 1], &bind_renames);
                self.track_pipe_prev_type(&prev);
                func.push(IrInst::DefVar(PIPE_PREV.to_string(), current));
                let span = expr.span().clone();
                let rewritten = self.rewrite_placeholder(expr, PIPE_PREV, &span);
                return self.lower_expr_tail(func, &rewritten);
            }
            // Placeholder/transforming step: the incoming value's kind comes
            // from the previous stage's expr, renamed through active binds so a
            // bound name resolves to its synthetic's recorded kind.
            if bind_renames.is_empty() {
                current = self.lower_pipeline_step(func, expr, current, &exprs[i - 1])?;
            } else {
                let prev_renamed = rewrite_idents(&exprs[i - 1], &bind_renames);
                current = self.lower_pipeline_step(func, expr, current, &prev_renamed)?;
            }
        }

        Ok(current)
    }

    /// True if `name` is a callable reference for the Native pipeline
    /// lowerer (user function, prelude builtin with a pipeline form,
    /// etc.), rather than a bind-and-forward target.
    fn is_native_pipeline_callable_ident(&self, name: &str) -> bool {
        if self.user_funcs.contains(name) || self.inner_func_symbol(name).is_some() {
            return true;
        }
        // `lower_pipeline_step` currently only special-cases "debug" for
        // the Ident branch; every other prelude builtin enters the
        // pipeline via FuncCall / MoldInst syntax. Treat "debug" as
        // callable here so `data => debug` still invokes the builtin.
        name == "debug"
    }

    /// Record the previous pipeline stage's static type on the synthetic
    /// `#pipe_prev` binding so a placeholder consumer (`_.toString()`,
    /// comparisons, ...) dispatches on the right kind. The native lowering
    /// reads `bool_vars` / `float_vars` / `string_vars` for kind dispatch;
    /// `#pipe_prev` is otherwise a kind-less `DefVar` that collapses
    /// Bool/Float to the Int-display polymorphic path — e.g.
    /// `flag => stdout(_.toString())` would print `1` instead of `true` on
    /// native without this.
    fn track_pipe_prev_type(&mut self, prev_expr: &Expr) {
        self.track_forwarded_pipe_type(PIPE_PREV, prev_expr);
    }

    /// Record `prev_expr`'s static type on `target` so a later placeholder /
    /// bound consumer dispatches on the right kind. `target` is either the
    /// synthetic `#pipe_prev` placeholder binding (`flag => stdout(_.toString())`)
    /// or a `=> name` bind-and-forward synthetic (`true => x => stdout(_.toString())`,
    /// where the kind must survive the intermediate binding). All six kind sets
    /// are cleared first so a stage that forwards a different kind (or a kind
    /// with no match) cannot leave a stale membership behind; the checks are
    /// ordered most-specific-first (Bool/Float/String/Pack/List then Int as the
    /// scalar fallback).
    fn track_forwarded_pipe_type(&mut self, target: &str, prev_expr: &Expr) {
        self.bool_vars.remove(target);
        self.float_vars.remove(target);
        self.string_vars.remove(target);
        self.pack_vars.remove(target);
        self.list_vars.remove(target);
        self.int_vars.remove(target);
        // The checker's type of the value the stage passes on decides.
        if let Some(ty) = self.typed_expr_table.stage_result(prev_expr).cloned() {
            use crate::types::Type;
            match &ty {
                Type::Bool => self.bool_vars.insert(target.to_string()),
                Type::Float => self.float_vars.insert(target.to_string()),
                Type::Str => self.string_vars.insert(target.to_string()),
                Type::Int => self.int_vars.insert(target.to_string()),
                Type::List(_) => self.list_vars.insert(target.to_string()),
                Type::BuchiPack(_) | Type::Error(_) | Type::Named(_)
                    if self.type_runtime_tag(&ty) == Some(crate::codegen::tag_prop::TAG_PACK) =>
                {
                    self.pack_vars.insert(target.to_string())
                }
                _ => false,
            };
            if self.type_runtime_tag(&ty).is_some() {
                return;
            }
        }
        if self.expr_is_likely_bool(prev_expr) {
            self.bool_vars.insert(target.to_string());
        } else if self.expr_returns_float(prev_expr) {
            self.float_vars.insert(target.to_string());
        } else if self.expr_is_string_full(prev_expr) {
            self.string_vars.insert(target.to_string());
        } else if self.expr_is_pack(prev_expr) {
            self.pack_vars.insert(target.to_string());
        } else if self.expr_is_list(prev_expr) {
            self.list_vars.insert(target.to_string());
        } else if self.expr_is_int(prev_expr) {
            self.int_vars.insert(target.to_string());
        }
    }

    pub(super) fn lower_pipeline_step(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
        prev_result: IrVar,
        prev_expr: &Expr,
    ) -> Result<IrVar, LowerError> {
        // POST-STABLE-007: track the previous stage's static type on
        // `#pipe_prev` before lowering this step (see `track_pipe_prev_type`).
        self.track_pipe_prev_type(prev_expr);

        // F62B-025: pipeline application closes over exactly two rules
        // (mirror of the interpreter's `eval_pipeline_step`).
        //
        // Rule 1 — the stage contains `_` (one at most, E1543): bind the
        // piped value to `#pipe_prev`, rewrite the placeholder, and lower
        // the stage as written.
        //
        // Rule 2 — no `_`: lower the stage as written and call the result
        // with the piped value (`CallIndirect`). This is what makes
        // `5 => add(, 3)` ≡ `f <= add(, 3)` + `5 => f`. The legacy
        // implicit first-argument injection is gone; the checker rejects
        // statically non-function stages as E1544 before lowering.
        let placeholder_count = expr_count_placeholders(expr);
        if placeholder_count > 1 {
            // The checker rejects this as E1543; keep a defensive guard so
            // the rewrite below can assume a single placeholder.
            return Err(LowerError {
                message: format!(
                    "[E1543] A pipeline stage can contain at most one `_` (found {}).",
                    placeholder_count
                ),
            });
        }

        if placeholder_count == 1 {
            // Rule 1: syntactic injection at the placeholder.
            // C12B-020: a lone `expr => _` rewrites to `#pipe_prev` and
            // lowers to a plain variable read — the previous accumulator
            // passes through unchanged.
            func.push(IrInst::DefVar(PIPE_PREV.to_string(), prev_result));
            let span = expr.span().clone();
            let rewritten = self.rewrite_placeholder(expr, PIPE_PREV, &span);
            return self.lower_expr(func, &rewritten);
        }

        // Rule 2: evaluate the stage as written, apply the piped value.
        match expr {
            // `expr => name`: route through the normal call lowering with
            // the piped value as the single argument — user functions take
            // the `CallUser` path (with default-argument completion),
            // closure-typed variables and lambdas take `CallIndirect`,
            // builtins take their dedicated runtime calls.
            Expr::Ident(name, span) => {
                let locally_bound = Self::is_local_binding(func, name);
                if !locally_bound
                    && (self.inner_func_symbol(name).is_some() || self.user_funcs.contains(name))
                {
                    let (mangled, arg_vars) = match self.default_arity_entry_for(name, 1) {
                        Some(entry) => (entry, vec![prev_result]),
                        None => (
                            self.resolve_user_func_symbol(name),
                            self.lower_user_call_effective_args_from_vars(
                                func,
                                name,
                                vec![prev_result],
                            )?,
                        ),
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::CallUser(result, mangled, arg_vars));
                    return Ok(result);
                }
                // Closure-typed variable (`f <= add(, 3)` then `5 => f`) or
                // builtin name: lower as `name(#pipe_prev)`.
                func.push(IrInst::DefVar(PIPE_PREV.to_string(), prev_result));
                // A synthesised node: it must not alias the stage's own id
                // in the checker's type table.
                let arg = Expr::Ident(
                    PIPE_PREV.to_string(),
                    span.clone()
                        .with_node_id(crate::parser::SYNTHETIC_NODE_ID_BASE),
                );
                self.lower_func_call(func, expr, &[arg])
            }
            _ => {
                // Lower the stage as written; the result must be a closure
                // (partial application `add(, 3)`, a lambda, or a call that
                // returns a function). Statically non-function stages were
                // rejected by the checker (E1544).
                let stage_var = self.lower_expr(func, expr)?;
                let result = func.alloc_var();
                func.push(IrInst::CallIndirect(result, stage_var, vec![prev_result]));
                Ok(result)
            }
        }
    }

    /// B11-5c: Rewrite all `Placeholder` nodes in an expression to `Ident(replacement)`.
    pub(super) fn rewrite_placeholder(
        &self,
        expr: &Expr,
        replacement: &str,
        span: &crate::lexer::Span,
    ) -> Expr {
        match expr {
            // The placeholder's own node id carries the piped value's
            // checker type; the other rewritten nodes keep theirs too.
            Expr::Placeholder(placeholder_span) => {
                Expr::Ident(replacement.to_string(), placeholder_span.clone())
            }
            Expr::BinaryOp(lhs, op, rhs, s) => Expr::BinaryOp(
                Box::new(self.rewrite_placeholder(lhs, replacement, span)),
                op.clone(),
                Box::new(self.rewrite_placeholder(rhs, replacement, span)),
                s.clone(),
            ),
            Expr::UnaryOp(op, inner, s) => Expr::UnaryOp(
                op.clone(),
                Box::new(self.rewrite_placeholder(inner, replacement, span)),
                s.clone(),
            ),
            Expr::FuncCall(callee, args, s) => Expr::FuncCall(
                Box::new(self.rewrite_placeholder(callee, replacement, span)),
                args.iter()
                    .map(|a| self.rewrite_placeholder(a, replacement, span))
                    .collect(),
                s.clone(),
            ),
            Expr::MethodCall(obj, method, args, s) => Expr::MethodCall(
                Box::new(self.rewrite_placeholder(obj, replacement, span)),
                method.clone(),
                args.iter()
                    .map(|a| self.rewrite_placeholder(a, replacement, span))
                    .collect(),
                s.clone(),
            ),
            Expr::MoldInst(name, type_args, fields, s) => Expr::MoldInst(
                name.clone(),
                type_args
                    .iter()
                    .map(|a| self.rewrite_placeholder(a, replacement, span))
                    .collect(),
                fields
                    .iter()
                    .map(|field| BuchiField {
                        name: field.name.clone(),
                        value: self.rewrite_placeholder(&field.value, replacement, span),
                        span: field.span.clone(),
                    })
                    .collect(),
                s.clone(),
            ),
            Expr::FieldAccess(obj, field, s) => Expr::FieldAccess(
                Box::new(self.rewrite_placeholder(obj, replacement, span)),
                field.clone(),
                s.clone(),
            ),
            Expr::ListLit(items, s) => Expr::ListLit(
                items
                    .iter()
                    .map(|item| self.rewrite_placeholder(item, replacement, span))
                    .collect(),
                s.clone(),
            ),
            Expr::Pipeline(items, s) => Expr::Pipeline(
                items
                    .iter()
                    .map(|item| self.rewrite_placeholder(item, replacement, span))
                    .collect(),
                s.clone(),
            ),
            Expr::BuchiPack(fields, s) => Expr::BuchiPack(
                fields
                    .iter()
                    .map(|field| BuchiField {
                        name: field.name.clone(),
                        value: self.rewrite_placeholder(&field.value, replacement, span),
                        span: field.span.clone(),
                    })
                    .collect(),
                s.clone(),
            ),
            Expr::TypeInst(name, fields, s) => Expr::TypeInst(
                name.clone(),
                fields
                    .iter()
                    .map(|field| BuchiField {
                        name: field.name.clone(),
                        value: self.rewrite_placeholder(&field.value, replacement, span),
                        span: field.span.clone(),
                    })
                    .collect(),
                s.clone(),
            ),
            Expr::Unmold(inner, s) => Expr::Unmold(
                Box::new(self.rewrite_placeholder(inner, replacement, span)),
                s.clone(),
            ),
            Expr::Lambda(params, body, s) => Expr::Lambda(
                params.clone(),
                Box::new(self.rewrite_placeholder(body, replacement, span)),
                s.clone(),
            ),
            Expr::Throw(inner, s) => Expr::Throw(
                Box::new(self.rewrite_placeholder(inner, replacement, span)),
                s.clone(),
            ),
            Expr::CondBranch(arms, s) => Expr::CondBranch(
                arms.iter()
                    .map(|arm| crate::parser::CondArm {
                        condition: arm
                            .condition
                            .as_ref()
                            .map(|c| self.rewrite_placeholder(c, replacement, span)),
                        body: arm
                            .body
                            .iter()
                            .map(|stmt| self.rewrite_statement_placeholder(stmt, replacement, span))
                            .collect(),
                        span: arm.span.clone(),
                    })
                    .collect(),
                s.clone(),
            ),
            other => other.clone(),
        }
    }

    /// Statement-level companion of [`Self::rewrite_placeholder`] for
    /// cond-branch arm bodies inside a pipeline stage.
    fn rewrite_statement_placeholder(
        &self,
        stmt: &Statement,
        replacement: &str,
        span: &crate::lexer::Span,
    ) -> Statement {
        match stmt {
            Statement::Expr(expr) => {
                Statement::Expr(self.rewrite_placeholder(expr, replacement, span))
            }
            Statement::Assignment(assign) => Statement::Assignment(Assignment {
                target: assign.target.clone(),
                type_annotation: assign.type_annotation.clone(),
                value: self.rewrite_placeholder(&assign.value, replacement, span),
                doc_comments: assign.doc_comments.clone(),
                span: assign.span.clone(),
                forward: assign.forward,
            }),
            // Review C-8: unmold bindings rewrite their source like the
            // interpreter's statement rewriter — the shared placeholder
            // count includes them, so the rewrite must reach them too.
            Statement::UnmoldForward(u) => {
                Statement::UnmoldForward(crate::parser::UnmoldForwardStmt {
                    source: self.rewrite_placeholder(&u.source, replacement, span),
                    target: u.target.clone(),
                    type_annotation: u.type_annotation.clone(),
                    span: u.span.clone(),
                })
            }
            Statement::UnmoldBackward(u) => {
                Statement::UnmoldBackward(crate::parser::UnmoldBackwardStmt {
                    target: u.target.clone(),
                    type_annotation: u.type_annotation.clone(),
                    source: self.rewrite_placeholder(&u.source, replacement, span),
                    span: u.span.clone(),
                })
            }
            other => other.clone(),
        }
    }

    /// ぶちパック: `@(field <= value, ...)`
    pub(super) fn lower_buchi_pack(
        &mut self,
        func: &mut IrFunction,
        fields: &[BuchiField],
    ) -> Result<IrVar, LowerError> {
        for field in fields {
            self.reject_untyped_addon_value(func, &field.value)?;
        }
        // QF-16: Placeholder 値のフィールドをスキップ（=> :Type が Placeholder として
        // パースされるため、BuchiPack 内ラムダの戻り値型注釈が不正なフィールドになる）
        let real_fields: Vec<_> = fields
            .iter()
            .filter(|f| !matches!(f.value, Expr::Placeholder(_)))
            .collect();
        let pack_var = func.alloc_var();
        func.push(IrInst::PackNew(pack_var, real_fields.len()));

        for (i, field) in real_fields.iter().enumerate() {
            // Register field name for jsonEncode
            self.field_names.insert(field.name.clone());

            // Detect Bool fields at compile time for field type registry
            let is_bool = self.expr_is_bool(&field.value);
            if is_bool {
                self.register_field_type_tag(&field.name, 4); // 4 = Bool
            }

            // C18-2: Detect Enum-variant field values so anonymous
            // BuchiPack literals (`@(state <= HiveState:Running())`) emit
            // variant-name Str via jsonEncode. `expr_enum_type_name`
            // walks the expression to find the source Enum name.
            //
            // C18B-003 fix: also remember the per-instance descriptor so
            // we can emit `taida_register_pack_field_enum(pack, hash, csv)`
            // after this field's `PackSet`. Without the per-pack
            // registration two packs that happen to share a field name
            // (e.g. `state`) but hold different enums
            // (`BuildState`, `RunState`) would overwrite each other's
            // entry in the global `__field_registry` and produce the
            // wrong variant name during `jsonEncode`.
            let enum_descriptor = self
                .expr_enum_type_name(&field.value)
                .and_then(|name| self.enum_defs.get(&name).map(|v| v.join(",")));
            let per_pack_enum_csv: Option<String> = enum_descriptor.clone();
            if let Some(csv) = enum_descriptor {
                self.register_field_type_tag(&field.name, 5);
                self.field_enum_descriptors.insert(field.name.clone(), csv);
            }

            // Emit inline field registration for jsonEncode (ensures library modules
            // register their field names at runtime, not just in _taida_main)
            let hash = simple_hash(&field.name);
            let type_tag = if is_bool {
                4
            } else {
                self.field_type_tags.get(&field.name).copied().unwrap_or(0)
            };
            self.emit_field_registration_inline(func, &field.name, hash, type_tag);

            // フィールド名ハッシュを設定
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let idx_var = func.alloc_var();
            func.push(IrInst::ConstInt(idx_var, i as i64));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_pack_set_hash".to_string(),
                vec![pack_var, idx_var, hash_var],
            ));

            let val = self.lower_expr(func, &field.value)?;
            func.push(IrInst::PackSet(pack_var, i, val));

            // C18B-003 fix: register the enum descriptor against this
            // pack instance so `json_serialize_pack_fields` can resolve
            // the correct variant list even when another pack uses the
            // same field name with a different Enum type.
            if let Some(csv) = per_pack_enum_csv.as_ref() {
                let hash_var2 = func.alloc_var();
                func.push(IrInst::ConstInt(hash_var2, hash as i64));
                let csv_var = func.alloc_var();
                func.push(IrInst::ConstStr(csv_var, csv.clone()));
                let reg_dummy = func.alloc_var();
                func.push(IrInst::Call(
                    reg_dummy,
                    "taida_register_pack_field_enum".to_string(),
                    vec![pack_var, hash_var2, csv_var],
                ));
            }

            // A-4c: Set type tag for this field value
            let val_tag = self.expr_type_tag(&field.value);
            if val_tag == -1 {
                // NB-14: UNKNOWN tag -- check if the value comes from a function
                // parameter with a runtime tag var (caller-propagated Bool/Int info).
                if let Some(tag_var) = self.get_param_tag_var(&field.value) {
                    // Use the runtime tag from the caller via taida_pack_set_tag()
                    let idx_var2 = func.alloc_var();
                    func.push(IrInst::ConstInt(idx_var2, i as i64));
                    let dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        dummy,
                        "taida_pack_set_tag".to_string(),
                        vec![pack_var, idx_var2, tag_var],
                    ));
                } else {
                    func.push(IrInst::PackSetTag(pack_var, i, val_tag));
                }
            } else if val_tag != 0 {
                func.push(IrInst::PackSetTag(pack_var, i, val_tag));
            }
            // retain-on-store: 再帰 release に対応するため子を retain
            self.emit_retain_if_heap_tag(func, val, val_tag);
        }

        Ok(pack_var)
    }

    /// フィールドアクセス: `expr.field`
    pub(super) fn lower_field_access(
        &mut self,
        func: &mut IrFunction,
        obj: &Expr,
        field: &str,
    ) -> Result<IrVar, LowerError> {
        if field.starts_with("__") {
            return Err(LowerError {
                message: format!(
                    "[E1960] Field '{}' is compiler-internal and cannot be accessed from Taida code. Hint: use unmolding, getOrDefault(default), or errorInfo() instead.",
                    field
                ),
            });
        }

        let obj_var = self.lower_expr(func, obj)?;

        // `type` / `message` of an Error value may be omitted from the value
        // itself; they read as the type's name and "".
        if matches!(field, "type" | "message") && self.expr_is_error_value(obj) {
            let which = func.alloc_var();
            func.push(IrInst::ConstInt(which, i64::from(field == "message")));
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_error_field".to_string(),
                vec![obj_var, which],
            ));
            return Ok(result);
        }

        // A Result given no error reads its `throw` as the empty Error.
        if field == "throw"
            && matches!(
                self.typed_expr_table.lookup(obj),
                Some(crate::types::Type::Generic(name, _)) if name == "Result"
            )
        {
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_result_throw_field".to_string(),
                vec![obj_var],
            ));
            return Ok(result);
        }

        // フィールドのインデックスをランタイムで解決
        // ランタイム関数 taida_pack_get_by_name(pack, field_name_hash) を使う
        let field_hash = simple_hash(field);
        let hash_var = func.alloc_var();
        func.push(IrInst::ConstInt(hash_var, field_hash as i64));

        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_pack_get".to_string(),
            vec![obj_var, hash_var],
        ));
        Ok(result)
    }

    /// 空スロット部分適用: `func(5)` → ラムダ（クロージャ）を生成
    /// Hole 位置のパラメータを持つクロージャを作り、non-hole 引数はキャプチャする。
    /// 旧 `_` (Placeholder) 部分適用は checker (E1502) で拒否済み。
    pub(super) fn lower_partial_application(
        &mut self,
        func: &mut IrFunction,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        let lambda_name = self.next_lambda_symbol("partial");

        // Evaluate non-hole arguments and track hole positions
        let mut captured_vars: Vec<(usize, IrVar)> = Vec::new(); // (arg_index, ir_var)
        let mut hole_count = 0usize;
        for (i, arg) in args.iter().enumerate() {
            if matches!(arg, Expr::Hole(_)) {
                hole_count += 1;
            } else {
                let var = self.lower_expr(func, arg)?;
                captured_vars.push((i, var));
            }
        }

        // Build a lambda function: __env holds captured non-hole args,
        // parameters are the hole slots
        let mut ir_params: Vec<String> = vec!["__env".to_string()];
        for i in 0..hole_count {
            ir_params.push(format!("__pa_{}", i));
        }

        let mut lambda_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

        // Restore captured args from environment pack
        for (pack_idx, (arg_idx, _)) in captured_vars.iter().enumerate() {
            let dst = lambda_fn.alloc_var();
            lambda_fn.push(IrInst::PackGet(dst, 0u32, pack_idx));
            lambda_fn.push(IrInst::DefVar(format!("{HIDDEN}pa_cap_{arg_idx}"), dst));
        }

        // Build the actual call arguments in order
        let mut call_args = Vec::new();
        let mut hole_idx = 0usize;
        for (i, arg) in args.iter().enumerate() {
            if matches!(arg, Expr::Hole(_)) {
                let v = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::UseVar(v, format!("__pa_{}", hole_idx)));
                call_args.push(v);
                hole_idx += 1;
            } else {
                let v = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::UseVar(v, format!("{HIDDEN}pa_cap_{i}")));
                call_args.push(v);
            }
        }

        // Generate the call inside the lambda: a named function is called
        // directly; any other callee (a function value held by a parameter,
        // a local or a top-level variable, or a computed callee) is
        // evaluated here, captured, and called indirectly.
        let result = lambda_fn.alloc_var();
        let direct = match callee {
            Expr::Ident(name, _) if !Self::is_local_binding(func, name) => {
                if self.inner_func_symbol(name).is_some() || self.user_funcs.contains(name) {
                    Some(IrInst::CallUser(
                        result,
                        self.resolve_user_func_symbol(name),
                        call_args.clone(),
                    ))
                } else if !self.top_level_vars.contains(name)
                    && let Some(rt_name) = self.stdlib_runtime_funcs.get(name).cloned()
                {
                    Some(IrInst::Call(result, rt_name, call_args.clone()))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(call) = direct {
            lambda_fn.push(call);
        } else {
            let callee_var = self.lower_expr(func, callee)?;
            captured_vars.push((usize::MAX, callee_var)); // special capture for callee
            let callee_restore = lambda_fn.alloc_var();
            lambda_fn.push(IrInst::PackGet(
                callee_restore,
                0u32,
                captured_vars.len() - 1,
            ));
            lambda_fn.push(IrInst::CallIndirect(result, callee_restore, call_args));
        }

        lambda_fn.push(IrInst::Return(result));

        self.user_funcs.insert(lambda_name.clone());
        self.lambda_funcs.push(lambda_fn);

        // Create closure with captured values
        let capture_names: Vec<String> = captured_vars
            .iter()
            .map(|(idx, _)| {
                if *idx == usize::MAX {
                    format!("{HIDDEN}pa_callee")
                } else {
                    format!("{HIDDEN}pa_cap_{idx}")
                }
            })
            .collect();

        // Store captured values in current scope so MakeClosure can find them
        for (cap_name, (_, ir_var)) in capture_names.iter().zip(captured_vars.iter()) {
            func.push(IrInst::DefVar(cap_name.clone(), *ir_var));
        }

        let dst = func.alloc_var();
        func.push(IrInst::MakeClosure(dst, lambda_name, capture_names));
        Ok(dst)
    }

    /// ラムダ式: `_ x = x * 2`
    /// キャプチャなしの場合は通常の関数として生成
    /// キャプチャありの場合はクロージャ（ファットポインタ）を生成
    pub(super) fn lower_lambda(
        &mut self,
        func: &mut IrFunction,
        params: &[Param],
        body: &Expr,
    ) -> Result<IrVar, LowerError> {
        let lambda_name = self.next_lambda_symbol("lambda");
        // `name <= _ x = ... name(...) ...`: the lambda's reference to its
        // own binding is a call back into itself, not a captured value (the
        // binding does not exist yet when the closure is created).
        let self_binding = self
            .lambda_binding_name
            .take()
            .filter(|name| !params.iter().any(|p| p.name == *name));

        // キャプチャ変数の検出: ラムダ本体で使われる変数のうち、
        // パラメータでもなく、ユーザー定義関数でもないもの
        let param_names: std::collections::HashSet<&str> =
            params.iter().map(|p| p.name.as_str()).collect();
        let mut free_vars = self.collect_free_vars(body, &param_names);
        if let Some(name) = &self_binding {
            free_vars.retain(|v| v != name);
        }
        // Only this lambda's own name re-enters it; a lambda nested in a
        // local closure does not inherit the outer self-reference.
        let prev_closure_self = std::mem::replace(
            &mut self.closure_self,
            self_binding.map(|name| (name, lambda_name.clone())),
        );

        // Scope-aware net builtin shadowing: snapshot/restore for lambda scope
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        // NB3-4: Snapshot var_aliases, lambda_param_counts, lambda_vars, closure_vars
        // for lambda scope
        let prev_var_aliases = self.var_aliases.clone();
        let prev_lambda_param_counts = self.lambda_param_counts.clone();
        let prev_lambda_vars = self.lambda_vars.clone();
        let prev_closure_vars = self.closure_vars.clone();
        let prev_function_ref_vars = self.function_ref_vars.clone();
        let prev_heap_vars = std::mem::take(&mut self.current_heap_vars);
        let prev_int_vars = self.int_vars.clone();
        // Value-tag track: shadow kinds are IR variables of the ENCLOSING
        // body — a lambda body must not UseVar a parent's shadow (and its
        // params must not inherit one by name collision).
        let prev_shadow_kinds = std::mem::take(&mut self.shadow_kind_vars);
        let prev_float_vars = self.float_vars.clone();
        let prev_string_vars = self.string_vars.clone();
        let prev_bool_vars = self.bool_vars.clone();
        let prev_pack_vars = self.pack_vars.clone();
        let prev_list_vars = self.list_vars.clone();
        for p in params {
            if Self::NET_BUILTIN_NAMES.contains(&p.name.as_str()) {
                self.shadowed_net_builtins.insert(p.name.clone());
            }
            // NB3-4 parameter shadow: remove outer-scope aliases so that
            // resolve_ident_arity / resolve_ident_callable_tag return unknown (-1)
            // for parameters that shadow outer aliases.
            self.var_aliases.remove(&p.name);
            self.lambda_param_counts.remove(&p.name);
            self.lambda_vars.remove(&p.name);
            self.closure_vars.remove(&p.name);
            self.function_ref_vars.remove(&p.name);
            if let Some(type_ann) = &p.type_annotation {
                match type_ann {
                    crate::parser::TypeExpr::Named(name) if name == "Int" || name == "Num" => {
                        self.int_vars.insert(p.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Float" => {
                        self.float_vars.insert(p.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Str" => {
                        self.string_vars.insert(p.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Bool" => {
                        self.bool_vars.insert(p.name.clone());
                    }
                    crate::parser::TypeExpr::BuchiPack(_) => {
                        self.pack_vars.insert(p.name.clone());
                    }
                    crate::parser::TypeExpr::List(_) => {
                        self.list_vars.insert(p.name.clone());
                    }
                    _ => {}
                }
            }
        }

        // 全ラムダを統一的にクロージャとして生成する。
        // キャプチャなしでも __env を第1引数として受け取り（未使用）、
        // MakeClosure で空の環境と共にクロージャ構造体を生成する。
        // これにより、ラムダが関数から返されたり変数に格納されたりしても、
        // 常に CallIndirect で安全に呼び出せる。
        {
            let mut ir_params: Vec<String> = vec!["__env".to_string()];
            ir_params.extend(params.iter().map(|p| p.name.clone()));

            let mut lambda_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

            // 環境からキャプチャ変数を復元（キャプチャなしの場合はスキップ）
            if !free_vars.is_empty() {
                let env_var = 0u32; // __env は第0パラメータ
                for (i, free_var) in free_vars.iter().enumerate() {
                    let get_dst = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::PackGet(get_dst, env_var, i));
                    lambda_fn.push(IrInst::DefVar(free_var.clone(), get_dst));
                }
            }

            // NB-14: Emit taida_get_call_arg_tag() for lambda params whose type
            // cannot be determined at compile time. This is the callee-side mirror
            // of the caller-side push/set/pop in the IIFE CallIndirect path.
            let prev_param_tag_vars = std::mem::take(&mut self.param_tag_vars);
            let prev_return_tag_vars = std::mem::take(&mut self.return_tag_vars);
            for (i, param) in params.iter().enumerate() {
                let has_known_type = self.bool_vars.contains(&param.name)
                    || self.int_vars.contains(&param.name)
                    || self.float_vars.contains(&param.name)
                    || self.string_vars.contains(&param.name)
                    || self.pack_vars.contains(&param.name)
                    || self.list_vars.contains(&param.name)
                    || self.closure_vars.contains(&param.name);
                if !has_known_type {
                    let idx_var = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::ConstInt(idx_var, i as i64));
                    let tag_var = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::Call(
                        tag_var,
                        "taida_get_call_arg_tag".to_string(),
                        vec![idx_var],
                    ));
                    self.param_tag_vars.insert(param.name.clone(), tag_var);
                }
            }

            // F62B-022: a block-bodied lambda lowers its statements through
            // the same machinery named functions use; the block's value is
            // the last statement's yield (final expression or tail binding).
            // Phase-2 review M-1: tail unmold bindings yield the BOUND
            // value, whose type differs from the source (`Lax[n]() >=> x`
            // yields the Int) — their return tag must not be derived from
            // the source expression, so they carry no tag expression.
            self.local_body_depth += 1;
            let (body_var, body_value_expr): (IrVar, Option<&Expr>) = match body {
                Expr::Block(stmts, _) => {
                    let Some((last, init)) = stmts.split_last() else {
                        return Err(LowerError {
                            message: "internal: empty lambda block body".to_string(),
                        });
                    };
                    for stmt in init {
                        self.lower_statement(&mut lambda_fn, stmt)?;
                    }
                    match last {
                        Statement::Expr(e) => (self.lower_expr(&mut lambda_fn, e)?, Some(e)),
                        Statement::Assignment(a) => {
                            self.lower_statement(&mut lambda_fn, last)?;
                            let v = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::UseVar(v, a.target.clone()));
                            (v, Some(&a.value))
                        }
                        Statement::UnmoldForward(u) => {
                            self.lower_statement(&mut lambda_fn, last)?;
                            let v = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::UseVar(v, u.target.clone()));
                            (v, None)
                        }
                        Statement::UnmoldBackward(u) => {
                            self.lower_statement(&mut lambda_fn, last)?;
                            let v = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::UseVar(v, u.target.clone()));
                            (v, None)
                        }
                        _ => {
                            return Err(LowerError {
                                message:
                                    "internal: lambda block must end with an expression or binding"
                                        .to_string(),
                            });
                        }
                    }
                }
                other => (self.lower_expr(&mut lambda_fn, other)?, Some(other)),
            };
            self.local_body_depth -= 1;

            // NB-14: Set return type tag before Return (symmetric with lower_func_def)
            if let Some(&rtv) = self.return_tag_vars.get(&body_var) {
                let dummy = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::Call(
                    dummy,
                    "taida_set_return_tag".to_string(),
                    vec![rtv],
                ));
            } else if let Some(body_value_expr) = body_value_expr {
                let tag = self.expr_type_tag(body_value_expr);
                if tag > 0 {
                    let tag_var = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::ConstInt(tag_var, tag));
                    let dummy = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![tag_var],
                    ));
                } else if tag == -1
                    && let Some(ptv) = self.get_param_tag_var(body)
                {
                    let dummy = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![ptv],
                    ));
                }
            }

            lambda_fn.push(IrInst::Return(body_var));

            // NB-14: Restore param/return tag vars to pre-lambda state
            // (lambda-scope IrVars must not leak into outer function scope)
            self.param_tag_vars = prev_param_tag_vars;
            self.return_tag_vars = prev_return_tag_vars;

            self.user_funcs.insert(lambda_name.clone());
            self.lambda_funcs.push(lambda_fn);

            // クロージャ生成: 環境パックを作り、MakeClosure を発行
            // （キャプチャなしの場合は空の環境パック）
            let dst = func.alloc_var();
            func.push(IrInst::MakeClosure(dst, lambda_name, free_vars));

            // Restore net builtin shadow set to pre-lambda state
            self.shadowed_net_builtins = prev_shadowed_net;
            // NB3-4: Restore var_aliases, lambda_param_counts, lambda_vars, closure_vars
            // to pre-lambda state (parameter shadow cleanup)
            self.var_aliases = prev_var_aliases;
            self.lambda_param_counts = prev_lambda_param_counts;
            self.lambda_vars = prev_lambda_vars;
            self.closure_vars = prev_closure_vars;
            self.function_ref_vars = prev_function_ref_vars;
            self.int_vars = prev_int_vars;
            self.shadow_kind_vars = prev_shadow_kinds;
            self.float_vars = prev_float_vars;
            self.string_vars = prev_string_vars;
            self.bool_vars = prev_bool_vars;
            self.pack_vars = prev_pack_vars;
            self.list_vars = prev_list_vars;
            self.current_heap_vars = prev_heap_vars;
            self.closure_self = prev_closure_self;

            Ok(dst)
        }
    }

    /// リストリテラル: `@[1, 2, 3]`
    pub(super) fn lower_list_lit(
        &mut self,
        func: &mut IrFunction,
        items: &[Expr],
    ) -> Result<IrVar, LowerError> {
        for item in items {
            self.reject_untyped_addon_value(func, item)?;
        }
        let list_var = func.alloc_var();
        func.push(IrInst::Call(list_var, "taida_list_new".to_string(), vec![]));

        // C23B-007 (2026-04-22): call `taida_list_set_elem_tag` for EVERY
        // element so heterogeneous literals like `@[1, "a", 2]` downgrade
        // to the HETEROGENEOUS(-2) sentinel. Previously we stamped only
        // the first element's tag (trusting a "checker guarantees
        // homogeneity" invariant that no longer holds — the interpreter
        // accepts `@[1, "a", 2]` verbatim, so the checker must too). The
        // first call transitions UNKNOWN → the first tag; each subsequent
        // mismatched call transitions that to HETEROGENEOUS which latches
        // (runtime-side `taida_list_set_elem_tag` rejects re-promotion).

        // taida_list_push は realloc で新ポインタを返す可能性がある
        // 最新のポインタを追跡する
        let mut current_list = list_var;
        for item in items {
            // C23B-007: stamp this element's tag BEFORE the push so the
            // per-element downgrade logic can latch as soon as two
            // primitive types disagree. For homogeneous lists the tag
            // converges to that primitive tag after the first call and
            // stays put (subsequent calls are no-ops).
            //
            // Value-tag track: the stamp goes through the EKIND entry
            // point (kind | enum-type-id<<8) so a mixed literal
            // materialises the per-element kind array instead of
            // collapsing to the bare HETEROGENEOUS sentinel, and enum
            // variants keep their type id for container equality.
            let tag = self.expr_type_tag(item);
            let ekind = self.expr_ekind(item);
            // A value of a type parameter (`wrap[T] x: T = @[x, x]`): its
            // kind is the one its caller tagged it with, known at run time.
            if ekind == 0xFF
                && let Some(param_tag) = self.get_param_tag_var(item)
            {
                let item_var = self.lower_expr(func, item)?;
                let note_dummy = func.alloc_var();
                func.push(IrInst::Call(
                    note_dummy,
                    "taida_list_note_push_dyn".to_string(),
                    vec![current_list, item_var, param_tag],
                ));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_push".to_string(),
                    vec![current_list, item_var],
                ));
                current_list = result;
                continue;
            }
            let tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(tag_var, ekind));
            let tag_dummy = func.alloc_var();
            func.push(IrInst::Call(
                tag_dummy,
                "taida_list_note_push_ekind".to_string(),
                vec![current_list, tag_var],
            ));
            let item_var = self.lower_expr(func, item)?;
            // retain-on-store: Pack/List/Closure 要素を格納する際に retain。
            // taida_release の List 再帰 release と対になり、double-free を防ぐ。
            // Pack フィールド格納時の retain-on-store (A-4c) と同じパターン。
            self.emit_retain_if_heap_tag(func, item_var, tag);
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_list_push".to_string(),
                vec![current_list, item_var],
            ));
            current_list = result;
        }

        Ok(current_list)
    }

    /// テンプレート文字列: `"Hello, ${name}!"` → 部分文字列を連結
    pub(super) fn lower_template_lit(
        &mut self,
        func: &mut IrFunction,
        template: &str,
        template_span: &crate::lexer::Span,
    ) -> Result<IrVar, LowerError> {
        let template_node_id = template_span.node_id;
        let mut interpolation_index = 0;
        // Parse template: split on ${ and } to get literal parts and expression parts.
        // Interpolation expressions are parsed using the full Taida parser and lowered
        // as real AST expressions, so field access, function calls, method calls etc.
        // are all supported (matching the interpreter behaviour).
        let mut result_var = {
            let var = func.alloc_var();
            func.push(IrInst::ConstStr(var, String::new()));
            var
        };

        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                // Skip '$' and '{'
                i += 2;
                // Find matching }
                let start = i;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '{' {
                        depth += 1;
                    }
                    if chars[i] == '}' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        i += 1;
                    }
                }
                let expr_str: String = chars[start..i].iter().collect();
                let expr_str_trimmed = expr_str.trim();

                // Parse the interpolation expression using the full Taida
                // parser, numbered as the checker numbered it so its
                // recorded types apply.
                let (program, errors) = crate::parser::parse_template_interpolation(
                    expr_str_trimmed,
                    template_node_id,
                    interpolation_index,
                );
                interpolation_index += 1;
                // Mirror the interpreter (`eval_template_string`) exactly, which
                // has three cases (F56-FB-006):
                //   1. parsed as an expression  → lower + stringify
                //   2. parsed but NOT an expression (e.g. an assignment
                //      `${x <= 1}`) → the interpreter emits nothing, so emit ""
                //   3. failed to parse / empty (e.g. a parser-rejected literal
                //      `${@(a: @(b <= 42))}`) → emit the raw body text verbatim
                // The previous code collapsed cases 2 and 3 into a bare-variable
                // read (rendering `0` on native) and, after the first fix, into
                // a single raw-text fallback (which left case 2 emitting the raw
                // body while the interpreter emits nothing). Splitting the three
                // cases keeps `${...}` parity with the interpreter. (A parse-
                // *successful* undefined identifier `${undefinedVar}` takes
                // case 1 above, not the fallback; its interp(error)/native(0)
                // divergence is a separate, unrelated gap.)
                let str_var = if errors.is_empty() && !program.statements.is_empty() {
                    if let crate::parser::Statement::Expr(ref parsed_expr) = program.statements[0] {
                        // Case 1: lower the parsed expression and convert to string.
                        self.reject_untyped_addon_value_at(func, parsed_expr, template_span)?;
                        let expr_var = self.lower_expr(func, parsed_expr)?;
                        self.convert_to_string(func, parsed_expr, expr_var)?
                    } else {
                        // Case 2 is a non-expression statement — the
                        // interpreter rejects it with [E1702] instead of
                        // silently dropping the body, so the lowering must
                        // reject it too rather than emit the empty string.
                        return Err(LowerError {
                            message: format!(
                                "[E1702] Template interpolation `${{{expr_str}}}` must be an expression, not a statement."
                            ),
                        });
                    }
                } else {
                    // Case 3: did not parse as a program (parser-rejected body)
                    // or empty — emit the raw body text verbatim, as the
                    // interpreter does.
                    let lit = func.alloc_var();
                    func.push(IrInst::ConstStr(lit, expr_str.clone()));
                    lit
                };
                let concat_var = func.alloc_var();
                func.push(IrInst::Call(
                    concat_var,
                    "taida_str_concat".to_string(),
                    vec![result_var, str_var],
                ));
                result_var = concat_var;
                // skip closing '}'
                if i < chars.len() {
                    i += 1;
                }
            } else {
                // Collect literal characters until next ${ or end
                let start = i;
                while i < chars.len() {
                    if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                        break;
                    }
                    i += 1;
                }
                let literal: String = chars[start..i].iter().collect();
                let lit_var = func.alloc_var();
                func.push(IrInst::ConstStr(lit_var, literal));
                let concat_var = func.alloc_var();
                func.push(IrInst::Call(
                    concat_var,
                    "taida_str_concat".to_string(),
                    vec![result_var, lit_var],
                ));
                result_var = concat_var;
            }
        }

        Ok(result_var)
    }
}

/// Substitute identifiers per `renames` (real name → synthetic) throughout
/// `expr`, returning a rewritten copy. Used by `lower_pipeline` to redirect a
/// step's references to a `=> name` bind from the (untouched) outer variable to
/// the pipeline-local synthetic. Capture-avoiding: a `Lambda` whose parameters
/// shadow a renamed name keeps that name un-substituted inside its body.
///
/// Variant coverage is kept consistent with `native_expr_references_any_name`
/// (the gate that decides whether a step references a bound name): both descend
/// the same expression variants, so any reference the gate detects is rewritten
/// here. Neither descends into a `TemplateLit`'s interpolation text (re-parsed
/// on its own elsewhere) nor into non-`Expr` statements inside a `CondArm` body;
/// the parser cannot place a pipeline-bound-name reference in those positions
/// mid-pipeline, so they are left as-is. If that ever changes, the gate and this
/// rewriter must grow together. Literals and other leaves are returned unchanged.
pub(crate) fn rewrite_idents(
    expr: &Expr,
    renames: &std::collections::HashMap<String, String>,
) -> Expr {
    if renames.is_empty() {
        return expr.clone();
    }
    let rewrite_fields = |fields: &[BuchiField]| -> Vec<BuchiField> {
        fields
            .iter()
            .map(|f| BuchiField {
                name: f.name.clone(),
                value: rewrite_idents(&f.value, renames),
                span: f.span.clone(),
            })
            .collect()
    };
    match expr {
        Expr::Ident(name, span) => match renames.get(name) {
            Some(syn) => Expr::Ident(syn.clone(), span.clone()),
            None => expr.clone(),
        },
        Expr::BinaryOp(l, op, r, s) => Expr::BinaryOp(
            Box::new(rewrite_idents(l, renames)),
            op.clone(),
            Box::new(rewrite_idents(r, renames)),
            s.clone(),
        ),
        Expr::UnaryOp(op, inner, s) => Expr::UnaryOp(
            op.clone(),
            Box::new(rewrite_idents(inner, renames)),
            s.clone(),
        ),
        Expr::FuncCall(callee, args, s) => Expr::FuncCall(
            Box::new(rewrite_idents(callee, renames)),
            args.iter().map(|a| rewrite_idents(a, renames)).collect(),
            s.clone(),
        ),
        Expr::MethodCall(obj, m, args, s) => Expr::MethodCall(
            Box::new(rewrite_idents(obj, renames)),
            m.clone(),
            args.iter().map(|a| rewrite_idents(a, renames)).collect(),
            s.clone(),
        ),
        Expr::FieldAccess(obj, f, s) => {
            Expr::FieldAccess(Box::new(rewrite_idents(obj, renames)), f.clone(), s.clone())
        }
        Expr::BuchiPack(fields, s) => Expr::BuchiPack(rewrite_fields(fields), s.clone()),
        Expr::ListLit(items, s) => Expr::ListLit(
            items.iter().map(|x| rewrite_idents(x, renames)).collect(),
            s.clone(),
        ),
        Expr::Pipeline(steps, s) => Expr::Pipeline(
            steps.iter().map(|x| rewrite_idents(x, renames)).collect(),
            s.clone(),
        ),
        Expr::MoldInst(name, type_args, fields, s) => Expr::MoldInst(
            name.clone(),
            type_args
                .iter()
                .map(|a| rewrite_idents(a, renames))
                .collect(),
            rewrite_fields(fields),
            s.clone(),
        ),
        Expr::TypeInst(name, fields, s) => {
            Expr::TypeInst(name.clone(), rewrite_fields(fields), s.clone())
        }
        Expr::Unmold(inner, s) => Expr::Unmold(Box::new(rewrite_idents(inner, renames)), s.clone()),
        Expr::Throw(inner, s) => Expr::Throw(Box::new(rewrite_idents(inner, renames)), s.clone()),
        Expr::CondBranch(arms, s) => Expr::CondBranch(
            arms.iter()
                .map(|arm| crate::parser::CondArm {
                    condition: arm.condition.as_ref().map(|c| rewrite_idents(c, renames)),
                    body: arm
                        .body
                        .iter()
                        .map(|st| match st {
                            Statement::Expr(e) => Statement::Expr(rewrite_idents(e, renames)),
                            other => other.clone(),
                        })
                        .collect(),
                    span: arm.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Lambda(params, body, s) => {
            // Capture-avoidance: a parameter that shadows a renamed name keeps
            // that name bound to the parameter inside the body, so drop it from
            // the rename set before recursing.
            if params.iter().any(|p| renames.contains_key(&p.name)) {
                let filtered: std::collections::HashMap<String, String> = renames
                    .iter()
                    .filter(|(k, _)| !params.iter().any(|p| &p.name == *k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                Expr::Lambda(
                    params.clone(),
                    Box::new(rewrite_idents(body, &filtered)),
                    s.clone(),
                )
            } else {
                Expr::Lambda(
                    params.clone(),
                    Box::new(rewrite_idents(body, renames)),
                    s.clone(),
                )
            }
        }
        _ => expr.clone(),
    }
}

/// True if `expr` references any name in `bound_names`
/// anywhere in its subtree. Used by `lower_pipeline` to decide whether a
/// pipeline step should skip the classic `prev_result` auto-injection
/// Shared `=> name` binding-reference rule — single definition in
/// `crate::parser::ast` (review C-6/C-9).
fn native_expr_references_any_name(expr: &Expr, bound_names: &[String]) -> bool {
    crate::parser::expr_references_any_name(expr, bound_names)
}

/// A pipeline stage that calls a named function with the piped value at its
/// one `_` (`x => f(a, _)`), as the interpreter recognises it.
fn is_piped_call(expr: &Expr) -> bool {
    match expr {
        Expr::FuncCall(callee, args, _) => {
            matches!(callee.as_ref(), Expr::Ident(..))
                && !args.iter().any(|arg| matches!(arg, Expr::Hole(_)))
                && expr_count_placeholders(expr) == 1
        }
        _ => false,
    }
}
