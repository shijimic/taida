// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups expr methods of the `Lowering` struct (placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{LowerError, Lowering, OS_NET_DEFAULT_TIMEOUT_MS, simple_hash};
use crate::codegen::ir::*;
use crate::parser::*;

impl Lowering {
    pub(crate) fn lower_expr(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
    ) -> Result<IrVar, LowerError> {
        match expr {
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
                // ユーザー定義関数を値として参照する場合は FuncAddr を使う
                if self.user_funcs.contains(name) {
                    let mangled = self.resolve_user_func_symbol(name);
                    let var = func.alloc_var();
                    func.push(IrInst::FuncAddr(var, mangled));
                    return Ok(var);
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
            Expr::TemplateLit(template, _) => self.lower_template_lit(func, template),
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
                {
                    // 末尾位置の自己再帰 → TailCall
                    let arg_vars =
                        self.lower_user_call_effective_args_from_exprs(func, name, args)?;
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

        for arm in arms {
            let condition = match &arm.condition {
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

            ir_arms.push(IrCondArm {
                condition,
                body: body_insts,
                result: body_var,
            });
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

    pub(super) fn lower_user_call_effective_args_from_vars(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        explicit_arg_vars: Vec<IrVar>,
    ) -> Result<Vec<IrVar>, LowerError> {
        let Some(params) = self.func_param_defs.get(name).cloned() else {
            return Ok(explicit_arg_vars);
        };

        if explicit_arg_vars.len() > params.len() {
            return Err(LowerError {
                message: format!(
                    "Function '{}' expected at most {} argument(s), got {}",
                    name,
                    params.len(),
                    explicit_arg_vars.len()
                ),
            });
        }

        // Materialize defaults in parameter order while exposing earlier params
        // by their declared names for default-expression references.
        let mut snapshots = Vec::<(String, IrVar)>::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for param in &params {
            if seen.insert(param.name.clone()) {
                let prev = func.alloc_var();
                func.push(IrInst::UseVar(prev, param.name.clone()));
                snapshots.push((param.name.clone(), prev));
            }
        }

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

        for (name, prev) in snapshots {
            func.push(IrInst::DefVar(name, prev));
        }

        Ok(effective_args)
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

        if let Expr::Ident(name, _) = callee {
            // OS network APIs with unified timeout argument (optional last arg).
            // Native backend uses fixed runtime signatures, so we inject defaults here.
            if matches!(
                name.as_str(),
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

                match name.as_str() {
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
                        return Ok(result);
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
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_pool_acquire".to_string(),
                            vec![pool, timeout],
                        ));
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                        return Ok(result);
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
                    let tls_expr = self.rewrite_http_serve_tls_expr_for_runtime(arg);
                    self.lower_expr(func, &tls_expr)?
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
                return Ok(result);
            }

            if name == "debug" {
                return self.lower_debug_call(func, args);
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
                return Ok(result);
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
                return Ok(result);
            }

            if name == "sleep" {
                if args.len() != 1 {
                    return Err(LowerError {
                        message: format!(
                            "sleep requires exactly 1 argument (ms), got {}",
                            args.len()
                        ),
                    });
                }
                let ms = self.lower_expr(func, &args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_time_sleep".to_string(),
                    vec![ms],
                ));
                return Ok(result);
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
                return Ok(result);
            }

            // RC2.5: addon dispatch takes precedence over stdlib lookup.
            // The call is emitted as a single `taida_addon_call` runtime
            // invocation; argv is packed into a tiny per-call Taida Pack
            // so the C dispatcher can read positional arguments with tags.
            //
            // The same helper is reused by the `Expr::MoldInst` lowering
            // path (Phase 2 / RC2.5-2a), so any user-side spelling of
            // `foo()` or `Foo[]()` routes through identical IR.
            if self.addon_func_refs.contains_key(name) {
                return self.emit_addon_call(func, name, args);
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
                    return self.lower_stdout_with_tag(func, name, &args[0], rt_name);
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
                    return Ok(result);
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
                    return Ok(result);
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
                    return Ok(result);
                }
                // jsonEncode/jsonPretty: pass value directly (no auto-conversion)
                // The C runtime handles polymorphic serialization
                if name == "jsonEncode" || name == "jsonPretty" {
                    let val_var = if let Some(arg) = args.first() {
                        self.lower_expr(func, arg)?
                    } else {
                        let zero = func.alloc_var();
                        func.push(IrInst::ConstInt(zero, 0));
                        zero
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(result, rt_name, vec![val_var]));
                    return Ok(result);
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
                    return Ok(result);
                }
                let mut arg_vars = Vec::new();
                for arg in args {
                    let var = self.lower_expr(func, arg)?;
                    arg_vars.push(var);
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, arg_vars));
                return Ok(result);
            }

            // ユーザー定義関数呼び出し
            if self.user_funcs.contains(name) {
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
                let mut explicit_arg_vars = Vec::with_capacity(args.len());
                for arg in args {
                    explicit_arg_vars.push(self.lower_expr(func, arg)?);
                }
                self.in_tail_call_return = saved_tail;
                if needs_tags {
                    // Post-lower tags: set tags for args whose type came from a call's return tag
                    self.emit_post_lower_arg_tags(func, args, &explicit_arg_vars);
                }
                let arg_vars =
                    self.lower_user_call_effective_args_from_vars(func, name, explicit_arg_vars)?;
                let result = func.alloc_var();
                let mangled = self.resolve_user_func_symbol(name);
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

        let mut last_result = None;
        for arg in args {
            let arg_var = self.lower_expr(func, arg)?;
            let runtime_fn = self.debug_fn_for_expr(arg);
            let result = func.alloc_var();
            func.push(IrInst::Call(result, runtime_fn, vec![arg_var]));
            last_result = Some(result);
        }
        Ok(last_result.unwrap())
    }

    pub(super) fn debug_fn_for_expr(&self, expr: &Expr) -> String {
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
                if let Expr::Ident(name, _) = callee.as_ref() {
                    if self.string_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_str".to_string();
                    }
                    if self.float_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_float".to_string();
                    }
                    if self.bool_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_bool".to_string();
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

    pub(super) fn lower_binary_op(
        &mut self,
        func: &mut IrFunction,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<IrVar, LowerError> {
        let lhs_var = self.lower_expr(func, lhs)?;
        let rhs_var = self.lower_expr(func, rhs)?;

        // Add (+) with string operands → string concatenation
        let lhs_is_str = self.expr_is_string_full(lhs);
        let rhs_is_str = self.expr_is_string_full(rhs);

        let runtime_fn = match op {
            BinOp::Add => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_concat"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
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
            BinOp::Eq => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_eq"
                } else if self.expr_returns_float(lhs)
                    || self.expr_returns_float(rhs)
                    || self.expr_is_bool(lhs)
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
                if lhs_is_str || rhs_is_str {
                    "taida_str_neq"
                } else if self.expr_returns_float(lhs)
                    || self.expr_returns_float(rhs)
                    || self.expr_is_bool(lhs)
                    || self.expr_is_bool(rhs)
                    || matches!(lhs, Expr::IntLit(_, _))
                    || matches!(rhs, Expr::IntLit(_, _))
                {
                    "taida_int_neq"
                } else {
                    "taida_poly_neq"
                }
            }
            BinOp::Lt => "taida_int_lt",
            BinOp::Gt => "taida_int_gt",
            BinOp::GtEq => "taida_int_gte",
            BinOp::And => "taida_bool_and",
            BinOp::Or => "taida_bool_or",
            BinOp::Concat => "taida_str_concat",
        };
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![lhs_var, rhs_var],
        ));
        Ok(result)
    }

    pub(super) fn lower_unary_op(
        &mut self,
        func: &mut IrFunction,
        op: &UnaryOp,
        operand: &Expr,
    ) -> Result<IrVar, LowerError> {
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
    /// C13-1 / C13B-007: An intermediate `=> name` step (where `name` is
    /// not a user-defined function or known builtin) acts as a
    /// **bind-and-forward**: the current value is bound to `name` via
    /// `DefVar` and passed through unchanged. Steps that explicitly
    /// reference the bound `name` skip the classic auto-injection of
    /// `prev_result` as an extra argument.
    pub(super) fn lower_pipeline(
        &mut self,
        func: &mut IrFunction,
        exprs: &[Expr],
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

        for (step_idx, expr) in exprs[1..].iter().enumerate() {
            let i = step_idx + 1;
            // Intermediate `=> name` bind-and-forward
            if i < last_idx
                && let Expr::Ident(name, _) = expr
                && !self.is_native_pipeline_callable_ident(name)
            {
                func.push(IrInst::DefVar(name.clone(), current));
                bound_names.push(name.clone());
                continue;
            }
            // Step consumes a pipeline-scope binding explicitly → evaluate
            // as-written, no auto-inject.
            if !bound_names.is_empty() && native_expr_references_any_name(expr, &bound_names) {
                current = self.lower_expr(func, expr)?;
                continue;
            }
            current = self.lower_pipeline_step(func, expr, current)?;
        }

        Ok(current)
    }

    /// C13-1 / C13B-007: True if `name` is a callable reference for the
    /// Native pipeline lowerer (user function, prelude builtin with a
    /// pipeline form, etc.), rather than a bind-and-forward target.
    fn is_native_pipeline_callable_ident(&self, name: &str) -> bool {
        if self.user_funcs.contains(name) {
            return true;
        }
        // `lower_pipeline_step` currently only special-cases "debug" for
        // the Ident branch; every other prelude builtin enters the
        // pipeline via FuncCall / MoldInst syntax. Treat "debug" as
        // callable here so `data => debug` still invokes the builtin.
        name == "debug"
    }

    pub(super) fn lower_pipeline_step(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
        prev_result: IrVar,
    ) -> Result<IrVar, LowerError> {
        match expr {
            // f(_) → f(prev_result)
            Expr::FuncCall(callee, args, span) => {
                let new_args: Vec<Expr> = args
                    .iter()
                    .map(|arg| {
                        if matches!(arg, Expr::Placeholder(_)) {
                            // _ を prev_result を指す特殊マーカーに置換
                            // ここでは直接 IrVar を渡せないので、
                            // Ident 参照に変換して DefVar で仮名をつける
                            Expr::Ident("__pipe_prev".to_string(), span.clone())
                        } else {
                            arg.clone()
                        }
                    })
                    .collect();

                // prev_result を __pipe_prev として定義
                func.push(IrInst::DefVar("__pipe_prev".to_string(), prev_result));

                self.lower_func_call(func, callee, &new_args)
            }
            // 変数名のみ（関数として呼び出し）: `expr => func_name`
            Expr::Ident(name, _) => {
                if name == "debug" {
                    // debug は特殊: debug(prev)
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_debug_int".to_string(),
                        vec![prev_result],
                    ));
                    return Ok(result);
                }
                if self.user_funcs.contains(name) {
                    let arg_vars = self.lower_user_call_effective_args_from_vars(
                        func,
                        name,
                        vec![prev_result],
                    )?;
                    let result = func.alloc_var();
                    let mangled = self.resolve_user_func_symbol(name);
                    func.push(IrInst::CallUser(result, mangled, arg_vars));
                    return Ok(result);
                }
                Err(LowerError {
                    message: format!("unknown pipeline target: {}", name),
                })
            }
            // B11-5c: MoldInst in pipeline: replace _ with prev_result
            Expr::MoldInst(name, type_args, fields, span) => {
                // Bind prev_result via DefVar so Ident("__pipe_prev") resolves.
                func.push(IrInst::DefVar("__pipe_prev".to_string(), prev_result));

                // Deep-check for any Placeholder (top-level or nested, e.g. `_ > 3`)
                let has_any_placeholder = type_args.iter().any(|a| self.expr_has_placeholder(a));

                let new_type_args: Vec<Expr> = if has_any_placeholder {
                    // Rewrite ALL Placeholder nodes to Ident("__pipe_prev"),
                    // handling both top-level `_` and nested `_ > 3` uniformly.
                    type_args
                        .iter()
                        .map(|a| self.rewrite_placeholder(a, "__pipe_prev", span))
                        .collect()
                } else {
                    // No placeholder — prepend prev_result as first type arg
                    let mut args = vec![Expr::Ident("__pipe_prev".to_string(), span.clone())];
                    args.extend(type_args.iter().cloned());
                    args
                };

                self.lower_mold_inst(func, name, &new_type_args, fields)
            }
            // C12B-020: `expr => _` is a no-op pipeline step. The
            // Interpreter already discards the result; the Native
            // lowerer historically rejected it. Accept it explicitly
            // by returning the previous accumulator unchanged so the
            // surrounding code sees the last meaningful value.
            Expr::Placeholder(_) => Ok(prev_result),
            _ => Err(LowerError {
                message: "unsupported pipeline step".to_string(),
            }),
        }
    }

    /// B11-5c: Check if an expression tree contains any Placeholder `_`.
    pub(super) fn expr_has_placeholder(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Placeholder(_) => true,
            Expr::BinaryOp(lhs, _, rhs, _) => {
                self.expr_has_placeholder(lhs) || self.expr_has_placeholder(rhs)
            }
            Expr::UnaryOp(_, inner, _) => self.expr_has_placeholder(inner),
            Expr::FuncCall(callee, args, _) => {
                self.expr_has_placeholder(callee)
                    || args.iter().any(|a| self.expr_has_placeholder(a))
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.expr_has_placeholder(obj) || args.iter().any(|a| self.expr_has_placeholder(a))
            }
            Expr::MoldInst(_, type_args, _, _) => {
                type_args.iter().any(|a| self.expr_has_placeholder(a))
            }
            _ => false,
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
            Expr::Placeholder(_) => Expr::Ident(replacement.to_string(), span.clone()),
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
                fields.clone(),
                s.clone(),
            ),
            other => other.clone(),
        }
    }

    /// ぶちパック: `@(field <= value, ...)`
    pub(super) fn lower_buchi_pack(
        &mut self,
        func: &mut IrFunction,
        fields: &[BuchiField],
    ) -> Result<IrVar, LowerError> {
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
        let obj_var = self.lower_expr(func, obj)?;

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

    /// 空スロット部分適用: `func(5, )` → ラムダ（クロージャ）を生成
    /// Hole 位置のパラメータを持つクロージャを作り、non-hole 引数はキャプチャする。
    /// 旧 `_` (Placeholder) 部分適用は checker (E1502) で拒否済み。
    pub(super) fn lower_partial_application(
        &mut self,
        func: &mut IrFunction,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_partial_{}", lambda_id);

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
            lambda_fn.push(IrInst::DefVar(format!("__pa_cap_{}", arg_idx), dst));
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
                lambda_fn.push(IrInst::UseVar(v, format!("__pa_cap_{}", i)));
                call_args.push(v);
            }
        }

        // Generate the call inside the lambda
        let result = lambda_fn.alloc_var();
        if let Expr::Ident(name, _) = callee {
            if self.user_funcs.contains(name) {
                let mangled = self.resolve_user_func_symbol(name);
                lambda_fn.push(IrInst::CallUser(result, mangled, call_args));
            } else if let Some(rt_name) = self.stdlib_runtime_funcs.get(name).cloned() {
                lambda_fn.push(IrInst::Call(result, rt_name, call_args));
            } else {
                // Lambda/closure variable call
                let closure_var = lambda_fn.alloc_var();
                // Need to restore callee from globals or environment
                self.globals_referenced.insert(name.clone());
                let hash = self.global_var_hash(name);
                lambda_fn.push(IrInst::GlobalGet(closure_var, hash));
                lambda_fn.push(IrInst::CallIndirect(result, closure_var, call_args));
            }
        } else {
            // Non-ident callee: evaluate in parent, capture, and call indirectly
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
                    "__pa_callee".to_string()
                } else {
                    format!("__pa_cap_{}", idx)
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
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_lambda_{}", lambda_id);

        // キャプチャ変数の検出: ラムダ本体で使われる変数のうち、
        // パラメータでもなく、ユーザー定義関数でもないもの
        let param_names: std::collections::HashSet<&str> =
            params.iter().map(|p| p.name.as_str()).collect();
        let free_vars = self.collect_free_vars(body, &param_names);

        // Scope-aware net builtin shadowing: snapshot/restore for lambda scope
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        // NB3-4: Snapshot var_aliases, lambda_param_counts, lambda_vars, closure_vars
        // for lambda scope
        let prev_var_aliases = self.var_aliases.clone();
        let prev_lambda_param_counts = self.lambda_param_counts.clone();
        let prev_lambda_vars = self.lambda_vars.clone();
        let prev_closure_vars = self.closure_vars.clone();
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

            let body_var = self.lower_expr(&mut lambda_fn, body)?;

            // NB-14: Set return type tag before Return (symmetric with lower_func_def)
            if let Some(&rtv) = self.return_tag_vars.get(&body_var) {
                let dummy = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::Call(
                    dummy,
                    "taida_set_return_tag".to_string(),
                    vec![rtv],
                ));
            } else {
                let tag = self.expr_type_tag(body);
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

            Ok(dst)
        }
    }

    /// リストリテラル: `@[1, 2, 3]`
    pub(super) fn lower_list_lit(
        &mut self,
        func: &mut IrFunction,
        items: &[Expr],
    ) -> Result<IrVar, LowerError> {
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
            // per-element downgrade logic in `taida_list_set_elem_tag`
            // can latch onto HETEROGENEOUS as soon as two primitive
            // types disagree. For homogeneous lists the tag converges
            // to that primitive tag after the first call and stays put
            // (subsequent calls are no-ops).
            let tag = self.expr_type_tag(item);
            let tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(tag_var, tag));
            let tag_dummy = func.alloc_var();
            func.push(IrInst::Call(
                tag_dummy,
                "taida_list_set_elem_tag".to_string(),
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
    ) -> Result<IrVar, LowerError> {
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

                // Parse the interpolation expression using the full Taida parser.
                let (program, errors) = crate::parser::parse(expr_str_trimmed);
                let str_var = if errors.is_empty()
                    && !program.statements.is_empty()
                    && let crate::parser::Statement::Expr(ref parsed_expr) = program.statements[0]
                {
                    // Lower the parsed expression and convert to string
                    let expr_var = self.lower_expr(func, parsed_expr)?;
                    self.convert_to_string(func, parsed_expr, expr_var)?
                } else {
                    // Fallback: treat as simple variable name (backward compat)
                    let var_name = expr_str_trimmed.to_string();
                    let name_var = func.alloc_var();
                    func.push(IrInst::UseVar(name_var, var_name.clone()));
                    let v = func.alloc_var();
                    func.push(IrInst::Call(
                        v,
                        "taida_polymorphic_to_string".to_string(),
                        vec![name_var],
                    ));
                    v
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

/// C13-1 / C13B-007: True if `expr` references any name in `bound_names`
/// anywhere in its subtree. Used by `lower_pipeline` to decide whether a
/// pipeline step should skip the classic `prev_result` auto-injection
/// because the user explicitly consumed a pipeline-scope binding.
fn native_expr_references_any_name(expr: &Expr, bound_names: &[String]) -> bool {
    fn walk(e: &Expr, names: &[String]) -> bool {
        match e {
            Expr::Ident(n, _) => names.iter().any(|bn| bn == n),
            Expr::BinaryOp(l, _, r, _) => walk(l, names) || walk(r, names),
            Expr::UnaryOp(_, inner, _) => walk(inner, names),
            Expr::FuncCall(callee, args, _) => {
                walk(callee, names) || args.iter().any(|a| walk(a, names))
            }
            Expr::MethodCall(obj, _, args, _) => {
                walk(obj, names) || args.iter().any(|a| walk(a, names))
            }
            Expr::FieldAccess(obj, _, _) => walk(obj, names),
            Expr::BuchiPack(fields, _) => fields.iter().any(|f| walk(&f.value, names)),
            Expr::ListLit(items, _) => items.iter().any(|x| walk(x, names)),
            Expr::Pipeline(steps, _) => steps.iter().any(|s| walk(s, names)),
            Expr::MoldInst(_, type_args, fields, _) => {
                type_args.iter().any(|a| walk(a, names))
                    || fields.iter().any(|f| walk(&f.value, names))
            }
            Expr::Unmold(inner, _) => walk(inner, names),
            Expr::Lambda(_, body, _) => walk(body, names),
            Expr::TypeInst(_, fields, _) => fields.iter().any(|f| walk(&f.value, names)),
            Expr::Throw(inner, _) => walk(inner, names),
            Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
                arm.condition.as_ref().is_some_and(|c| walk(c, names))
                    || arm.body.iter().any(|s| {
                        if let Statement::Expr(e) = s {
                            walk(e, names)
                        } else {
                            false
                        }
                    })
            }),
            _ => false,
        }
    }
    walk(expr, bound_names)
}
