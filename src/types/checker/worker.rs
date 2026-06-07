//! worker — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::lexer::Span;
use crate::parser::*;
use crate::types::Type;
use std::collections::HashSet;

use super::{TypeChecker, TypeError, WorkerAddonBinding, WorkerAddonDecision};

impl TypeChecker {
    pub(super) fn register_worker_addon_imports(&mut self, imp: &crate::parser::ImportStmt) {
        if imp.path.starts_with("npm:")
            || imp.path.starts_with("taida-lang/")
            || imp.path.starts_with("./")
            || imp.path.starts_with("../")
            || imp.path.starts_with('/')
        {
            return;
        }

        let Some(source_file) = self.source_file.clone() else {
            return;
        };
        let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
        let project_root = Self::find_project_root(source_dir);
        let resolution = if let Some(ref version) = imp.version {
            crate::pkg::resolver::resolve_package_module_versioned(
                &project_root,
                &imp.path,
                version,
            )
        } else {
            crate::pkg::resolver::resolve_package_module(&project_root, &imp.path)
        };
        let Some(resolution) = resolution else {
            return;
        };
        if resolution.submodule.is_some() {
            return;
        }
        let manifest_path = resolution.pkg_dir.join("native").join("addon.toml");
        if !manifest_path.exists() {
            return;
        }

        let manifest = match crate::addon::manifest::parse_addon_manifest(&manifest_path) {
            Ok(manifest) => manifest,
            Err(err) => {
                for sym in &imp.symbols {
                    let local = sym.alias.as_ref().unwrap_or(&sym.name);
                    self.worker_addon_bindings.insert(
                        local.to_string(),
                        WorkerAddonBinding {
                            package_id: imp.path.clone(),
                            function_name: sym.name.clone(),
                            decision: WorkerAddonDecision::Deny {
                                code: "[E1631]",
                                reason: err.to_string(),
                                active_policy: "unresolved".to_string(),
                                effective_claim: "invalid".to_string(),
                            },
                        },
                    );
                }
                return;
            }
        };

        let policy = crate::pkg::addon_purity_policy::load_addon_purity_policy(&project_root);

        for sym in &imp.symbols {
            let local = sym.alias.as_ref().unwrap_or(&sym.name);
            let decision = match &policy {
                Ok(policy) => self.decide_worker_addon_import(policy, &manifest, &sym.name),
                Err(err) => WorkerAddonDecision::Deny {
                    code: "[E1630]",
                    reason: err.clone(),
                    active_policy: "invalid".to_string(),
                    effective_claim: "unresolved".to_string(),
                },
            };
            self.worker_addon_bindings.insert(
                local.to_string(),
                WorkerAddonBinding {
                    package_id: manifest.package.clone(),
                    function_name: sym.name.clone(),
                    decision,
                },
            );
        }
    }

    pub(super) fn decide_worker_addon_import(
        &self,
        policy: &crate::pkg::addon_purity_policy::AddonPurityPolicy,
        manifest: &crate::addon::manifest::AddonManifest,
        function_name: &str,
    ) -> WorkerAddonDecision {
        let active_policy = policy.mode.as_str().to_string();
        if !manifest.functions.contains_key(function_name) {
            return WorkerAddonDecision::Deny {
                code: "[E1631]",
                reason: format!(
                    "addon manifest for '{}' does not declare function '{}'",
                    manifest.package, function_name
                ),
                active_policy,
                effective_claim: "invalid".to_string(),
            };
        }
        if policy.is_override_trusted(&manifest.package, function_name) {
            return WorkerAddonDecision::Allow;
        }

        let purity = manifest.function_purity_for(function_name);
        match purity.claim {
            crate::addon::manifest::AddonPurityClaim::Unspecified => WorkerAddonDecision::Deny {
                code: "[E1627]",
                reason: "function has no `declared` purity claim".to_string(),
                active_policy,
                effective_claim: "unspecified".to_string(),
            },
            crate::addon::manifest::AddonPurityClaim::Declared => {
                if purity.audit.is_some() {
                    return WorkerAddonDecision::Deny {
                        code: "[E1629]",
                        reason: "audit metadata is present but no F48 audit verifier is available"
                            .to_string(),
                        active_policy,
                        effective_claim: "invalid".to_string(),
                    };
                }
                if policy.allows_declared() {
                    WorkerAddonDecision::Allow
                } else {
                    WorkerAddonDecision::Deny {
                        code: "[E1628]",
                        reason: "`declared` purity is below the active policy".to_string(),
                        active_policy,
                        effective_claim: "declared".to_string(),
                    }
                }
            }
        }
    }

    pub(super) fn push_worker_error(&mut self, code: &str, span: &Span, message: String) {
        if self
            .errors
            .iter()
            .any(|err| err.span == *span && err.message.contains(code))
        {
            return;
        }
        self.errors.push(TypeError {
            message,
            span: span.clone(),
        });
    }

    pub(super) fn validate_async_task_worker_body(&mut self, task_arg: &Expr) {
        match task_arg {
            Expr::Lambda(params, body, _) => {
                let mut local_names = HashSet::new();
                let mut function_stack = HashSet::new();
                self.push_scope();
                for param in params {
                    if let Some(default_value) = &param.default_value {
                        self.validate_worker_expr(
                            default_value,
                            &mut local_names,
                            &mut function_stack,
                        );
                    }
                    let ty = param
                        .type_annotation
                        .as_ref()
                        .map(|ann| self.registry.resolve_type(ann))
                        .unwrap_or(Type::Unknown);
                    self.define_var_silent(&param.name, ty);
                    local_names.insert(param.name.clone());
                }
                self.validate_worker_expr(body, &mut local_names, &mut function_stack);
                self.pop_scope();
            }
            Expr::Ident(name, span) => {
                let mut function_stack = HashSet::new();
                let local_names = HashSet::new();
                self.validate_worker_call_name(name, span, &local_names, &mut function_stack);
            }
            other => {
                let mut local_names = HashSet::new();
                let mut function_stack = HashSet::new();
                self.validate_worker_expr(other, &mut local_names, &mut function_stack);
                self.push_worker_error(
                    "[E1624]",
                    other.span(),
                    "[E1624] CPU worker body must be a lambda literal or a visible Taida function. \
                     Hint: write `AsyncTask[_ = expr]()` or pass a direct mapper lambda to `ParMap` so the worker body is explicit."
                        .to_string(),
                );
            }
        }
    }

    pub(super) fn validate_worker_user_function(
        &mut self,
        name: &str,
        span: &Span,
        function_stack: &mut HashSet<String>,
    ) {
        if !function_stack.insert(name.to_string()) {
            return;
        }

        let Some(fd) = self
            .func_defs
            .get(name)
            .or_else(|| self.generic_func_defs.get(name))
            .cloned()
        else {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot call opaque function value '{}'. \
                     Hint: call a Taida function whose body is visible to the checker, or inline a local lambda inside the task.",
                    name
                ),
            );
            function_stack.remove(name);
            return;
        };

        let param_types = self.func_param_types.get(name).cloned().unwrap_or_else(|| {
            fd.params
                .iter()
                .map(|param| {
                    param
                        .type_annotation
                        .as_ref()
                        .map(|ann| self.registry.resolve_type(ann))
                        .unwrap_or(Type::Unknown)
                })
                .collect()
        });

        let mut local_names = HashSet::new();
        self.push_scope();
        for (idx, param) in fd.params.iter().enumerate() {
            if let Some(default_value) = &param.default_value {
                self.validate_worker_expr(default_value, &mut local_names, function_stack);
            }
            self.define_var_silent(
                &param.name,
                param_types.get(idx).cloned().unwrap_or(Type::Unknown),
            );
            local_names.insert(param.name.clone());
        }

        for stmt in &fd.body {
            self.validate_worker_stmt(stmt, &mut local_names, function_stack);
        }
        self.pop_scope();
        function_stack.remove(name);
    }

    pub(super) fn validate_worker_stmt(
        &mut self,
        stmt: &Statement,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Assignment(assign) => {
                self.validate_worker_expr(&assign.value, local_names, function_stack);
                let ty = self
                    .typed_expr_table
                    .lookup(&assign.value)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                self.define_var_silent(&assign.target, ty);
                local_names.insert(assign.target.clone());
            }
            Statement::Expr(expr) => self.validate_worker_expr(expr, local_names, function_stack),
            Statement::ErrorCeiling(ec) => {
                let mut handler_locals = local_names.clone();
                self.push_scope();
                let err_ty = self.registry.resolve_type(&ec.error_type);
                self.define_var_silent(&ec.error_param, err_ty);
                handler_locals.insert(ec.error_param.clone());
                for stmt in &ec.handler_body {
                    self.validate_worker_stmt(stmt, &mut handler_locals, function_stack);
                }
                self.pop_scope();
            }
            Statement::UnmoldForward(stmt) => {
                self.validate_worker_expr(&stmt.source, local_names, function_stack);
                let source_ty = self
                    .typed_expr_table
                    .lookup(&stmt.source)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                self.define_var_silent(&stmt.target, self.unmold_type(&source_ty));
                local_names.insert(stmt.target.clone());
            }
            Statement::UnmoldBackward(stmt) => {
                self.validate_worker_expr(&stmt.source, local_names, function_stack);
                let source_ty = self
                    .typed_expr_table
                    .lookup(&stmt.source)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                self.define_var_silent(&stmt.target, self.unmold_type(&source_ty));
                local_names.insert(stmt.target.clone());
            }
            Statement::FuncDef(fd) => {
                self.validate_worker_inline_function_def(fd, local_names, function_stack);
            }
            Statement::ClassLikeDef(_)
            | Statement::EnumDef(_)
            | Statement::Import(_)
            | Statement::Export(_) => {}
        }
    }

    pub(super) fn validate_worker_expr(
        &mut self,
        expr: &Expr,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, span) => self.validate_worker_ident(name, span, local_names),
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    self.validate_worker_expr(&field.value, local_names, function_stack);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.validate_worker_expr(item, local_names, function_stack);
                }
            }
            Expr::Pipeline(items, _) => {
                let last_idx = items.len().saturating_sub(1);
                let mut pipeline_locals = local_names.clone();
                self.push_scope();
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0
                        && idx < last_idx
                        && let Expr::Ident(name, _) = item
                        && !self.is_pipeline_callable_ident(name)
                    {
                        pipeline_locals.insert(name.clone());
                        continue;
                    }
                    self.validate_worker_expr(item, &mut pipeline_locals, function_stack);
                }
                self.pop_scope();
            }
            Expr::BinaryOp(left, _, right, _) => {
                self.validate_worker_expr(left, local_names, function_stack);
                self.validate_worker_expr(right, local_names, function_stack);
            }
            Expr::UnaryOp(_, inner, _)
            | Expr::FieldAccess(inner, _, _)
            | Expr::Unmold(inner, _)
            | Expr::Throw(inner, _) => {
                self.validate_worker_expr(inner, local_names, function_stack);
            }
            Expr::FuncCall(callee, args, span) => {
                for arg in args {
                    self.validate_worker_expr(arg, local_names, function_stack);
                }
                match callee.as_ref() {
                    Expr::Ident(name, callee_span) => self.validate_worker_call_name(
                        name,
                        callee_span,
                        local_names,
                        function_stack,
                    ),
                    Expr::Lambda(params, body, _) => {
                        self.validate_worker_lambda(params, body, local_names, function_stack);
                    }
                    other => {
                        self.validate_worker_expr(other, local_names, function_stack);
                        self.push_worker_error(
                            "[E1624]",
                            span,
                            "[E1624] CPU worker body cannot call a computed function value. \
                             Hint: use a direct Taida function call or a lambda literal inside the task."
                                .to_string(),
                        );
                    }
                }
            }
            Expr::MethodCall(receiver, _, args, _) => {
                self.validate_worker_expr(receiver, local_names, function_stack);
                for arg in args {
                    self.validate_worker_expr(arg, local_names, function_stack);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(condition) = &arm.condition {
                        self.validate_worker_expr(condition, local_names, function_stack);
                    }
                    let mut arm_locals = local_names.clone();
                    self.push_scope();
                    for stmt in &arm.body {
                        self.validate_worker_stmt(stmt, &mut arm_locals, function_stack);
                    }
                    self.pop_scope();
                }
            }
            Expr::MoldInst(name, type_args, fields, span) => {
                let value_arg_count = Self::worker_mold_value_arg_count(name, type_args.len());
                for arg in type_args.iter().take(value_arg_count) {
                    self.validate_worker_expr(arg, local_names, function_stack);
                }
                for field in fields {
                    self.validate_worker_expr(&field.value, local_names, function_stack);
                }
                self.validate_worker_mold_name(name, span);
            }
            Expr::Lambda(params, body, _) => {
                self.validate_worker_lambda(params, body, local_names, function_stack);
            }
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::TemplateLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Gorilla(_)
            | Expr::Placeholder(_)
            | Expr::Hole(_)
            | Expr::EnumVariant(_, _, _)
            | Expr::TypeLiteral(_, _, _) => {}
        }
    }

    pub(super) fn validate_worker_lambda(
        &mut self,
        params: &[Param],
        body: &Expr,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        let mut nested_locals = local_names.clone();
        self.push_scope();
        for param in params {
            if let Some(default_value) = &param.default_value {
                self.validate_worker_expr(default_value, &mut nested_locals, function_stack);
            }
            let ty = param
                .type_annotation
                .as_ref()
                .map(|ann| self.registry.resolve_type(ann))
                .unwrap_or(Type::Unknown);
            self.define_var_silent(&param.name, ty);
            nested_locals.insert(param.name.clone());
        }
        self.validate_worker_expr(body, &mut nested_locals, function_stack);
        self.pop_scope();
    }

    pub(super) fn validate_worker_inline_function_def(
        &mut self,
        fd: &FuncDef,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        let param_types: Vec<Type> = fd
            .params
            .iter()
            .map(|param| {
                param
                    .type_annotation
                    .as_ref()
                    .map(|ann| self.registry.resolve_type(ann))
                    .unwrap_or(Type::Unknown)
            })
            .collect();
        let ret_ty = fd
            .return_type
            .as_ref()
            .map(|ann| self.registry.resolve_type(ann))
            .unwrap_or(Type::Unknown);

        let mut nested_locals = local_names.clone();
        self.push_scope();
        for (idx, param) in fd.params.iter().enumerate() {
            if let Some(default_value) = &param.default_value {
                self.validate_worker_expr(default_value, &mut nested_locals, function_stack);
            }
            self.define_var_silent(
                &param.name,
                param_types.get(idx).cloned().unwrap_or(Type::Unknown),
            );
            nested_locals.insert(param.name.clone());
        }
        for stmt in &fd.body {
            self.validate_worker_stmt(stmt, &mut nested_locals, function_stack);
        }
        self.pop_scope();

        self.define_var_silent(&fd.name, Type::Function(param_types, Box::new(ret_ty)));
        local_names.insert(fd.name.clone());
    }

    pub(super) fn validate_worker_call_name(
        &mut self,
        name: &str,
        span: &Span,
        local_names: &HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        if local_names.contains(name) {
            return;
        }
        if self.is_worker_effect_symbol(name) {
            self.push_worker_error(
                "[E1620]",
                span,
                format!(
                    "[E1620] CPU worker body cannot call effectful API '{}'. \
                     Hint: perform I/O before creating the task or after `Par[jobs]()` completes.",
                    name
                ),
            );
            return;
        }
        if let Some(binding) = self.worker_addon_bindings.get(name).cloned() {
            match binding.decision {
                WorkerAddonDecision::Allow => {}
                WorkerAddonDecision::Deny {
                    code,
                    reason,
                    active_policy,
                    effective_claim,
                } => {
                    self.push_worker_error(
                        code,
                        span,
                        format!(
                            "{} CPU worker body cannot call addon function '{}::{}'. \
                             Effective claim: {}; active policy: {}. {}. \
                             Hint: add function purity metadata and project policy, or move the addon call outside the worker task.",
                            code,
                            binding.package_id,
                            binding.function_name,
                            effective_claim,
                            active_policy,
                            reason
                        ),
                    );
                }
            }
            return;
        }
        if self.worker_addon_symbols.contains(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot cross addon or host boundary '{}'. \
                     Hint: move addon and host interop calls outside the worker task.",
                    name
                ),
            );
            return;
        }
        if self.func_defs.contains_key(name) || self.generic_func_defs.contains_key(name) {
            self.validate_worker_user_function(name, span, function_stack);
            return;
        }
        if Self::is_core_builtin_name(name) {
            return;
        }
        if matches!(self.lookup_var(name), Some(Type::Function(_, _)))
            || self.func_types.contains_key(name)
        {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot call captured function value '{}'. \
                     Hint: call a visible Taida function directly or inline a local lambda inside the task.",
                    name
                ),
            );
            return;
        }
        if matches!(self.lookup_var(name), Some(Type::Unknown | Type::Any)) {
            self.push_worker_error(
                "[E1626]",
                span,
                format!(
                    "[E1626] CPU worker body calls '{}' before its type is fully known. \
                     Hint: add a concrete annotation or use a visible Taida function.",
                    name
                ),
            );
            return;
        }
        if let Some(ty) = self.lookup_var(name)
            && !self.is_worker_safe_type(&ty)
        {
            self.push_worker_error(
                "[E1623]",
                span,
                format!(
                    "[E1623] CPU worker body cannot call '{}' with non-transferable type {}. \
                     Hint: call visible Taida functions directly and keep host values outside the worker task.",
                    name, ty
                ),
            );
        }
    }

    pub(super) fn validate_worker_ident(
        &mut self,
        name: &str,
        span: &Span,
        local_names: &HashSet<String>,
    ) {
        if local_names.contains(name) {
            return;
        }
        if self.is_worker_effect_symbol(name) {
            self.push_worker_error(
                "[E1620]",
                span,
                format!(
                    "[E1620] CPU worker body cannot capture effectful API '{}'. \
                     Hint: perform I/O before creating the task or after `Par[jobs]()` completes.",
                    name
                ),
            );
            return;
        }
        if self.worker_addon_bindings.contains_key(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot capture addon or host boundary '{}'. \
                     Hint: call allowed pure addon functions directly inside the worker task; do not capture them as values.",
                    name
                ),
            );
            return;
        }
        if self.worker_addon_symbols.contains(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot capture addon or host boundary '{}'. \
                     Hint: move addon and host interop values outside the worker task.",
                    name
                ),
            );
            return;
        }
        if self.func_defs.contains_key(name)
            || self.generic_func_defs.contains_key(name)
            || self.func_types.contains_key(name)
        {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot capture function value '{}'. \
                     Hint: call a visible Taida function directly or inline a local lambda inside the task.",
                    name
                ),
            );
            return;
        }
        let Some(ty) = self.lookup_var(name) else {
            self.push_worker_error(
                "[E1626]",
                span,
                format!(
                    "[E1626] CPU worker body captures '{}' before its type is known. \
                     Hint: define the value before creating the task and give it a concrete type.",
                    name
                ),
            );
            return;
        };
        if matches!(ty, Type::Unknown | Type::Any) {
            self.push_worker_error(
                "[E1626]",
                span,
                format!(
                    "[E1626] CPU worker body captures '{}' with unresolved type {}. \
                     Hint: add a concrete annotation before creating the task.",
                    name, ty
                ),
            );
            return;
        }
        if matches!(ty, Type::Function(_, _)) {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot capture function value '{}'. \
                     Hint: call a visible Taida function directly or inline a local lambda inside the task.",
                    name
                ),
            );
            return;
        }
        if !self.is_worker_safe_type(&ty) {
            self.push_worker_error(
                "[E1623]",
                span,
                format!(
                    "[E1623] CPU worker body captures '{}' with non-transferable type {}. \
                     Hint: capture primitives, lists, and structurally safe buchi packs only.",
                    name, ty
                ),
            );
        }
    }

    pub(super) fn validate_worker_mold_name(&mut self, name: &str, span: &Span) {
        if Self::is_worker_effect_mold(name) {
            self.push_worker_error(
                "[E1620]",
                span,
                format!(
                    "[E1620] CPU worker body cannot call effectful mold '{}'. \
                     Hint: perform file, environment, or network access outside the worker task.",
                    name
                ),
            );
        } else if Self::is_worker_host_boundary_mold(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot cross addon or host boundary '{}'. \
                     Hint: move addon and host interop calls outside the worker task.",
                    name
                ),
            );
        } else if Self::is_worker_nested_async_mold(name) {
            self.push_worker_error(
                "[E1622]",
                span,
                format!(
                    "[E1622] CPU worker body cannot create nested async or parallel value '{}'. \
                     Hint: build parallel tasks at the outer level and keep each task body synchronous.",
                    name
                ),
            );
        }
    }

    pub(super) fn is_worker_effect_builtin(name: &str) -> bool {
        matches!(
            name,
            "debug"
                | "nowMs"
                | "stdout"
                | "stderr"
                | "exit"
                | "stdin"
                | "stdinLine"
                | "argv"
                | "sleep"
                | "readBytes"
                | "readBytesAt"
                | "writeFile"
                | "writeBytes"
                | "appendFile"
                | "remove"
                | "createDir"
                | "rename"
                | "allEnv"
                | "dnsResolve"
                | "tcpConnect"
                | "tcpListen"
                | "tcpAccept"
                | "socketSend"
                | "socketSendAll"
                | "socketSendBytes"
                | "socketRecv"
                | "socketRecvBytes"
                | "socketRecvExact"
                | "udpBind"
                | "udpSendTo"
                | "udpRecvFrom"
                | "socketClose"
                | "listenerClose"
                | "udpClose"
                | "poolCreate"
                | "poolAcquire"
                | "poolRelease"
                | "poolClose"
                | "poolHealth"
                | "run"
                | "execShell"
                | "runInteractive"
                | "execShellInteractive"
        )
    }

    pub(super) fn is_worker_safe_type_inner(
        &self,
        ty: &Type,
        seen_named: &mut HashSet<String>,
    ) -> bool {
        match ty {
            Type::Int | Type::Float | Type::Num | Type::Str | Type::Bytes | Type::Bool => true,
            Type::BuchiPack(fields) => fields
                .iter()
                .all(|(_, field_ty)| self.is_worker_safe_type_inner(field_ty, seen_named)),
            Type::List(inner) => self.is_worker_safe_type_inner(inner, seen_named),
            Type::Named(name) => {
                if self.registry.is_enum_type(name) {
                    return true;
                }
                if !seen_named.insert(name.clone()) {
                    return true;
                }
                let safe = self.registry.get_type_fields(name).is_some_and(|fields| {
                    fields
                        .iter()
                        .all(|(_, field_ty)| self.is_worker_safe_type_inner(field_ty, seen_named))
                });
                seen_named.remove(name);
                safe
            }
            Type::Generic(name, args) => {
                use crate::types::mold_specs::{WorkerSafety, lookup_worker_safety};
                match lookup_worker_safety(name) {
                    WorkerSafety::Pure => true,
                    WorkerSafety::Transparent => args
                        .iter()
                        .all(|arg| self.is_worker_safe_type_inner(arg, seen_named)),
                    WorkerSafety::Unsafe => self.is_worker_safe_user_mold(name, args, seen_named),
                }
            }
            Type::Error(name) => {
                if !seen_named.insert(name.clone()) {
                    return true;
                }
                let safe = self.registry.get_type_fields(name).is_some_and(|fields| {
                    fields
                        .iter()
                        .all(|(_, field_ty)| self.is_worker_safe_type_inner(field_ty, seen_named))
                });
                seen_named.remove(name);
                safe
            }
            Type::Function(_, _)
            | Type::Unit
            | Type::Unknown
            | Type::Any
            | Type::Json
            | Type::Molten => false,
        }
    }
}
