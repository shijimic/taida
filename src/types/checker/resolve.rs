//! resolve — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::lexer::Span;
use crate::parser::*;
use crate::types::Type;
use std::collections::{HashMap, HashSet};

use super::{
    BranchInfo, CageBranch, CageRunnerType, CryptoSym, MoldBindingDef, MoldHeaderSpec, TypeChecker,
    TypeError,
};

impl TypeChecker {
    pub(super) fn abi_name_value_pair_list_type() -> Type {
        Type::List(Box::new(Type::BuchiPack(vec![
            ("name".to_string(), Type::Str),
            ("value".to_string(), Type::Str),
        ])))
    }

    pub(super) fn is_wired_constraint_type(ty: &Type) -> bool {
        matches!(ty, Type::Named(name) if name == "Wired")
            || matches!(ty, Type::Generic(name, args) if name == "Wired" && args.len() == 1)
    }

    pub(super) fn is_host_step_type(ty: &Type) -> bool {
        matches!(ty, Type::Generic(name, args) if name == "HostStep" && args.len() == 2)
    }

    pub(super) fn is_crypto_hash_input_type(ty: &Type) -> bool {
        matches!(ty, Type::Str | Type::Bytes)
    }

    pub(super) fn erased_host_step_type() -> Type {
        Type::Generic("HostStep".to_string(), vec![Type::Any, Type::Any])
    }

    pub(super) fn is_wire_encodable_type(&self, ty: &Type) -> bool {
        if Self::contains_unit_like_type(ty) {
            return false;
        }
        match ty {
            Type::Str | Type::Int | Type::Float | Type::Bool | Type::Bytes => true,
            Type::List(inner) => self.is_wire_encodable_type(inner),
            Type::BuchiPack(fields) => {
                !fields.is_empty()
                    && fields
                        .iter()
                        .all(|(_, field_ty)| self.is_wire_encodable_type(field_ty))
            }
            Type::Named(name) => self.registry.get_type_fields(name).is_some_and(|fields| {
                !fields.is_empty()
                    && fields
                        .iter()
                        .all(|(_, field_ty)| self.is_wire_encodable_type(field_ty))
            }),
            Type::Generic(name, args) if name == "HostCapability" => {
                args.len() == 2 && args.iter().all(|arg| matches!(arg, Type::Str))
            }
            _ => false,
        }
    }

    pub(super) fn is_host_capability_type(ty: &Type) -> bool {
        matches!(ty, Type::Generic(name, args) if name == "HostCapability" && args.len() == 2 && args.iter().all(|arg| matches!(arg, Type::Str)))
    }

    pub(super) fn infer_host_capability_type(&mut self, type_args: &[Expr], span: &Span) -> Type {
        if type_args.len() != 2 {
            self.errors.push(TypeError {
                message: format!(
                    "[E1505] `HostCapability[name, kind]()` requires exactly 2 `[]` argument(s), got {}.",
                    type_args.len()
                ),
                span: span.clone(),
            });
        }
        for (idx, arg) in type_args.iter().take(2).enumerate() {
            let ty = self.infer_expr_type(arg);
            if ty != Type::Str && ty != Type::Unknown {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1506] HostCapability argument {} must be Str, got {}.",
                        idx + 1,
                        ty
                    ),
                    span: arg.span().clone(),
                });
            }
        }
        self.validate_host_capability_manifest(type_args, span);
        Type::Generic("HostCapability".to_string(), vec![Type::Str, Type::Str])
    }

    pub(super) fn infer_host_step_type(&mut self, type_args: &[Expr], span: &Span) -> Type {
        if type_args.len() != 2 {
            self.errors.push(TypeError {
                message: format!(
                    "[E1505] `HostStep[method, args]()` requires exactly 2 `[]` argument(s), got {}.",
                    type_args.len()
                ),
                span: span.clone(),
            });
        }
        if let Some(method) = type_args.first() {
            let method_ty = self.infer_expr_type(method);
            if method_ty != Type::Str && method_ty != Type::Unknown {
                self.errors.push(TypeError {
                    message: format!("[E1506] HostStep method must be Str, got {}.", method_ty),
                    span: method.span().clone(),
                });
            }
            if method_ty == Type::Str && self.string_const_expr(method).is_none() {
                self.errors.push(TypeError {
                    message: "[E3603] HostStep method must be a compile-time Str value. \
                             Hint: use a string literal or a Str constant for the method name."
                        .to_string(),
                    span: method.span().clone(),
                });
            }
        }
        let args_ty = if let Some(args) = type_args.get(1) {
            let (args_ty, args_ok) = self.wire_encodable_expr_type(args);
            if !args_ok {
                self.push_wired_constraint_error("HostStep args", &args_ty, args.span());
            }
            if !matches!(args_ty, Type::List(_)) && !matches!(args_ty, Type::Unknown) && args_ok {
                self.errors.push(TypeError {
                    message: format!(
                        "[E3601] HostStep args must be a wire-encodable list, got {}. \
                         Hint: pass positional arguments as `@[arg0, arg1, ...]`; use `@[]` for no arguments.",
                        args_ty
                    ),
                    span: args.span().clone(),
                });
            }
            args_ty
        } else {
            Type::Unknown
        };
        Type::Generic("HostStep".to_string(), vec![Type::Str, args_ty])
    }

    /// Check whether a type contains an unresolved type variable.
    ///
    /// A `Named` type that is not registered in the type registry is
    /// an unresolved generic type parameter (e.g. `T`, `U`). When
    /// either the body type or the declared return type contains such
    /// a variable, the return-type check must be suppressed because
    /// the checker cannot meaningfully compare them.
    /// look up an active enclosing function's `TypeParam`
    /// by name, walking the stack of nested generic functions inside-out.
    /// Returns `None` if the name does not refer to any active type parameter.
    pub(super) fn lookup_active_type_param(&self, name: &str) -> Option<&TypeParam> {
        for frame in self.current_func_type_params.iter().rev() {
            if let Some(tp) = frame.iter().find(|tp| tp.name == name) {
                return Some(tp);
            }
        }
        None
    }

    pub(super) fn type_arg_expr_to_type(&self, expr: &Expr) -> Type {
        match expr {
            Expr::Ident(name, _) => self.type_name_to_type(name),
            Expr::TypeLiteral(name, None, _) => self.type_name_to_type(name),
            Expr::TypeLiteral(enum_name, Some(variant_name), _) => {
                Type::Named(format!("{}:{}", enum_name, variant_name))
            }
            Expr::ListLit(items, _) if items.len() == 1 => {
                Type::List(Box::new(self.type_arg_expr_to_type(&items[0])))
            }
            // F42 sweep (R5) (Codex 第 4 ラウンド指摘): 型引数として書かれた
            // `@()` / `@(name: T, ...)` を `Type::BuchiPack` に正しく変換する。
            // これ以前は `Expr::BuchiPack(...)` がすべて `Type::Unknown` に落ち、
            // `JSGet[@["x"], @()]` のような Cage runner Out が
            // `contains_unit_like_type` の検出網をすり抜けていた (E1520 抜け道)。
            Expr::BuchiPack(fields, _) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|f| (f.name.clone(), self.type_arg_expr_to_type(&f.value)))
                    .collect(),
            ),
            // F42 sweep (R5) follow-up (Codex 第 5 ラウンド指摘): 型引数として
            // 書かれた `Async[Unit]` / `Result[Unit, Str]` / `Optional[Void]` 等の
            // generic な mold instantiation を `Type::Generic` に変換する。これ以前は
            // `Expr::MoldInst(...)` がすべて `Type::Unknown` に落ち、Cage runner Out
            // で `JSGet[..., Async[Unit]]` のような nested unit-like 型が
            // `contains_unit_like_type` の検出網をすり抜けていた (E1520 抜け道)。
            // 関数戻り型注釈位置で書かれた `Async[Unit]` は別経路 `resolve_type` 経由で
            // 既に reject されており、ここは Cage runner Out 等の type-arg 位置専用の
            // 補完。`registry.resolve_type` を呼ぶと scope error を起こすので、
            // shallow に `Type::Generic(name, args)` を構築するに留める。
            Expr::MoldInst(name, type_args, _fields, _) => Type::Generic(
                name.clone(),
                type_args
                    .iter()
                    .map(|arg| self.type_arg_expr_to_type(arg))
                    .collect(),
            ),
            _ => Type::Unknown,
        }
    }

    fn type_name_to_type(&self, name: &str) -> Type {
        match name {
            "Int" | "Integer" => Type::Int,
            "Float" => Type::Float,
            "Num" => Type::Num,
            "Str" | "String" => Type::Str,
            "Bytes" => Type::Bytes,
            "Bool" | "Boolean" => Type::Bool,
            "Unit" => Type::Unit,
            "JSON" => Type::Json,
            "Molten" => Type::Molten,
            other if self.registry.is_error_type(other) => Type::Error(other.to_string()),
            other => Type::Named(other.to_string()),
        }
    }

    pub(super) fn cage_runner_type(&self, expr: &Expr) -> Option<CageRunnerType> {
        let Expr::MoldInst(name, type_args, _, _) = expr else {
            return None;
        };
        match name.as_str() {
            "JSGet" if type_args.len() == 2 => type_args.get(1).map(|out| CageRunnerType {
                branch: CageBranch::Js,
                output: self.type_arg_expr_to_type(out),
                async_boundary: false,
            }),
            "JSCall" | "JSNew" if type_args.len() == 3 => {
                type_args.get(2).map(|out| CageRunnerType {
                    branch: CageBranch::Js,
                    output: self.type_arg_expr_to_type(out),
                    async_boundary: false,
                })
            }
            "JSCallAsync" if type_args.len() == 3 => type_args.get(2).map(|out| CageRunnerType {
                branch: CageBranch::Js,
                output: self.type_arg_expr_to_type(out),
                async_boundary: true,
            }),
            "JSSet" if type_args.len() == 2 => Some(CageRunnerType {
                branch: CageBranch::Js,
                output: Type::Bool,
                async_boundary: false,
            }),
            "JSBind" | "JSSpread" if type_args.len() == 1 => Some(CageRunnerType {
                branch: CageBranch::Js,
                output: Type::Molten,
                async_boundary: false,
            }),
            "JSRilla" => type_args.first().map(|out| CageRunnerType {
                branch: CageBranch::Js,
                output: self.type_arg_expr_to_type(out),
                async_boundary: false,
            }),
            "FileRilla" => type_args.first().map(|out| CageRunnerType {
                branch: CageBranch::File,
                output: self.type_arg_expr_to_type(out),
                async_boundary: false,
            }),
            "BuildRilla" => type_args.first().map(|out| CageRunnerType {
                branch: CageBranch::Build,
                output: self.type_arg_expr_to_type(out),
                async_boundary: false,
            }),
            "HostCall" if type_args.len() == 2 => type_args.get(1).map(|out| CageRunnerType {
                branch: CageBranch::Host,
                output: self.type_arg_expr_to_type(out),
                async_boundary: true,
            }),
            "CageRilla" => {
                let branch = type_args
                    .first()
                    .and_then(|arg| self.branch_from_type_arg(arg))?;
                let output = type_args
                    .get(1)
                    .map(|out| self.type_arg_expr_to_type(out))
                    .unwrap_or(Type::Unknown);
                Some(CageRunnerType {
                    branch,
                    output,
                    async_boundary: false,
                })
            }
            _ => None,
        }
    }

    fn lookup_branch_info(&self, name: &str) -> BranchInfo {
        for scope in self.branch_scope_stack.iter().rev() {
            if let Some(info) = scope.get(name) {
                return *info;
            }
        }
        BranchInfo::None
    }

    pub(super) fn lookup_molten_branch(&self, name: &str) -> Option<CageBranch> {
        match self.lookup_branch_info(name) {
            BranchInfo::Molten(branch) => Some(branch),
            BranchInfo::None | BranchInfo::GorillaxValue(_) => None,
        }
    }

    pub(super) fn lookup_gorillax_value_branch(&self, name: &str) -> Option<CageBranch> {
        match self.lookup_branch_info(name) {
            BranchInfo::GorillaxValue(branch) => Some(branch),
            BranchInfo::None | BranchInfo::Molten(_) => None,
        }
    }

    pub(super) fn lookup_string_const(&self, name: &str) -> Option<String> {
        for scope in self.string_const_scope_stack.iter().rev() {
            if let Some(value) = scope.get(name) {
                return value.clone();
            }
        }
        None
    }

    /// Register exported types and function signatures that cross a module
    /// boundary so the importer can type-check calls without falling back to
    /// `Type::Unknown`.
    ///
    /// Behaviour:
    /// 1. Resolve the import path (relative, package, or submodule) using the same
    /// logic as `validate_import_symbols`.
    /// 2. Parse the target module and collect every `EnumDef` / `FuncDef`
    /// whose name is being imported by the current statement.
    /// 3. If the importer has **not** already defined an enum with the same local
    /// name, register it into `self.registry`. The wire-order is the import
    /// origin (source of truth).
    /// 4. If the importer **has** already defined the enum locally (common pattern
    /// during the enum-schema transition), check that the variant list is identical;
    /// any mismatch emits `[E1618] Enum '<name>' variant order mismatch across
    /// module boundary to catch enum-order drift.
    ///
    /// Notes:
    /// - `[E1618]` is allocated for this check because `[E1610]` is already
    /// occupied by cyclic-inheritance detection.
    /// - Aliased imports (`>>>./m.td => @(Color: Paint)`) register the enum
    /// under the alias, mirroring the interpreter behaviour.
    pub(super) fn register_imported_types(&mut self, imp: &crate::parser::ImportStmt) {
        use crate::parser::Statement as S;

        if imp.path == "taida-lang/abi" {
            self.register_abi_imports(&imp.symbols);
            return;
        }

        // Core bundled packages are handled elsewhere (net / crypto).
        if imp.path.starts_with("npm:") || imp.path.starts_with("taida-lang/") {
            return;
        }

        // Same path-resolution strategy as `validate_import_symbols`.
        let source_file = match &self.source_file {
            Some(f) => f.clone(),
            None => return,
        };

        let td_path: std::path::PathBuf = if imp.path.starts_with("./")
            || imp.path.starts_with("../")
            || imp.path.starts_with('/')
        {
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let path = source_dir.join(&imp.path);
            if path.exists() { path } else { return }
        } else {
            // Package import — resolve via .taida/deps/
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let project_root = Self::find_project_root(source_dir);
            let resolution = if let Some(ref ver) = imp.version {
                crate::pkg::resolver::resolve_package_module_versioned(
                    &project_root,
                    &imp.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(&project_root, &imp.path)
            };
            match resolution {
                Some(res) => match &res.submodule {
                    Some(sub) => {
                        let sub_path = res.pkg_dir.join(format!("{}.td", sub));
                        if sub_path.exists() { sub_path } else { return }
                    }
                    None => {
                        let entry_name =
                            match crate::pkg::manifest::Manifest::from_dir(&res.pkg_dir) {
                                Ok(Some(manifest)) => manifest.entry,
                                _ => "main.td".to_string(),
                            };
                        let entry_path = if let Some(stripped) = entry_name.strip_prefix("./") {
                            res.pkg_dir.join(stripped)
                        } else {
                            res.pkg_dir.join(&entry_name)
                        };
                        if entry_path.exists() {
                            entry_path
                        } else {
                            return;
                        }
                    }
                },
                None => return,
            }
        };

        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _) = crate::parser::parse(&source);

        // Build a map of imported-symbol-name → local-alias (or the same name).
        let requested: std::collections::HashMap<&str, &str> = imp
            .symbols
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.alias.as_deref().unwrap_or(s.name.as_str()),
                )
            })
            .collect();
        if requested.is_empty() {
            return;
        }

        let mut type_aliases: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for stmt in &program.statements {
            match stmt {
                S::EnumDef(ed) if requested.contains_key(ed.name.as_str()) => {
                    type_aliases.insert(ed.name.as_str(), requested[ed.name.as_str()]);
                }
                S::ClassLikeDef(cl) if requested.contains_key(cl.name.as_str()) => {
                    type_aliases.insert(cl.name.as_str(), requested[cl.name.as_str()]);
                }
                _ => {}
            }
        }

        for stmt in &program.statements {
            if let S::EnumDef(ed) = stmt
                && let Some(&local_name) = requested.get(ed.name.as_str())
            {
                let variants: Vec<String> = ed.variants.iter().map(|v| v.name.clone()).collect();

                if let Some(existing) = self.registry.get_enum_variants(local_name) {
                    // Local redefinition already present — must match the
                    // exported module's order exactly.
                    if existing != variants {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1618] Enum '{}' variant order mismatch across module boundary. \
                                 Defined at '{}': [{}]. Imported as: [{}]. \
                                 Hint: Align local redefinition order with the exporting module, \
                                 or remove the local redefinition and rely on the imported type.",
                                local_name,
                                td_path.display(),
                                variants.join(", "),
                                existing.join(", ")
                            ),
                            span: imp.span.clone(),
                        });
                    }
                } else {
                    // No local redefinition — register as if declared here.
                    self.registry.register_enum(local_name, variants);
                    self.declared_concrete_type_names
                        .insert(local_name.to_string());
                    self.declared_header_arities
                        .insert(local_name.to_string(), 0);
                }
            } else if let S::FuncDef(fd) = stmt
                && let Some(&local_name) = requested.get(fd.name.as_str())
            {
                self.register_imported_function_signature(fd, local_name, &type_aliases);
            }
        }
    }

    pub(super) fn resolve_imported_type_expr(
        &self,
        ty: &crate::parser::TypeExpr,
        type_aliases: &std::collections::HashMap<&str, &str>,
    ) -> Type {
        use crate::parser::TypeExpr;

        match ty {
            TypeExpr::Named(name) => {
                let local_name = type_aliases
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(name.as_str());
                self.registry
                    .resolve_type(&TypeExpr::Named(local_name.to_string()))
            }
            TypeExpr::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|field| {
                        let field_ty = field
                            .type_annotation
                            .as_ref()
                            .map(|field_ty| self.resolve_imported_type_expr(field_ty, type_aliases))
                            .unwrap_or(Type::Unknown);
                        (field.name.clone(), field_ty)
                    })
                    .collect(),
            ),
            TypeExpr::List(inner) => Type::List(Box::new(
                self.resolve_imported_type_expr(inner, type_aliases),
            )),
            TypeExpr::Generic(name, args) => Type::Generic(
                type_aliases
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(name.as_str())
                    .to_string(),
                args.iter()
                    .map(|arg| self.resolve_imported_type_expr(arg, type_aliases))
                    .collect(),
            ),
            TypeExpr::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.resolve_imported_type_expr(param, type_aliases))
                    .collect(),
                Box::new(self.resolve_imported_type_expr(ret, type_aliases)),
            ),
        }
    }

    pub(super) fn imported_function_value_type(&self, name: &str) -> Option<Type> {
        let ret = self.func_types.get(name)?;
        let params = self.func_param_types.get(name).cloned().unwrap_or_else(|| {
            vec![
                Type::Unknown;
                self.func_param_counts
                    .get(name)
                    .copied()
                    .unwrap_or_default()
            ]
        });
        Some(Type::Function(params, Box::new(ret.clone())))
    }

    /// Look up a variable type from the scope stack (innermost first).
    pub fn lookup_var(&self, name: &str) -> Option<Type> {
        for scope in self.scope_stack.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    /// Unwrap a mold type to get its inner value type.
    /// Used for `>=>` and `<=<` unmold operations.
    pub(super) fn unmold_type(&self, ty: &Type) -> Type {
        match ty {
            // JSON unmolds to dynamic type (needs schema)
            Type::Json => Type::Unknown,
            // Molten is opaque — cannot unmold directly
            Type::Molten => Type::Unknown,
            // Generic mold types: extract the first type argument
            Type::Generic(name, args) => {
                match name.as_str() {
                    "Lax" | "Result" | "Async" | "Gorillax" | "RelaxedGorillax" => {
                        args.first().cloned().unwrap_or(Type::Unknown)
                    }
                    // Stream[T] unmolds to @[T] (List)
                    "Stream" => {
                        let inner = args.first().cloned().unwrap_or(Type::Unknown);
                        Type::List(Box::new(inner))
                    }
                    _ => {
                        // Custom mold types registered in the registry:
                        // extract the first type argument (filling type T)
                        if self.registry.mold_defs.contains_key(name.as_str()) {
                            args.first().cloned().unwrap_or(Type::Unknown)
                        } else {
                            Type::Unknown
                        }
                    }
                }
            }
            // Named type that is a registered mold: unmold to Unknown
            // (type parameter not instantiated, so we can't determine T)
            Type::Named(name) => {
                if self.registry.mold_defs.contains_key(name.as_str()) {
                    Type::Unknown
                } else {
                    // Non-mold named types pass through
                    ty.clone()
                }
            }
            // Unknown stays unknown
            Type::Unknown => Type::Unknown,
            // Non-mold types pass through (runtime will handle)
            _ => ty.clone(),
        }
    }

    /// Register type definitions from a statement (first pass).
    pub(super) fn register_types(&mut self, stmt: &Statement) {
        match stmt {
            Statement::EnumDef(ed) => {
                let has_collision = self.registry.type_defs.contains_key(&ed.name)
                    || self.registry.enum_defs.contains_key(&ed.name)
                    || self.func_types.contains_key(&ed.name)
                    || self.registry.mold_defs.contains_key(&ed.name);
                if has_collision {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1501] Name '{}' is already defined in this scope. \
                             Redefinition in the same scope is not allowed. \
                             Hint: Use a different name, or define it in an inner scope (shadowing is allowed).",
                            ed.name
                        ),
                        span: ed.span.clone(),
                    });
                }
                let mut seen = HashSet::new();
                for variant in &ed.variants {
                    if !seen.insert(variant.name.clone()) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1501] Enum '{}' redefines variant '{}'. Hint: Enum variants must be unique within the same enum.",
                                ed.name, variant.name
                            ),
                            span: variant.span.clone(),
                        });
                    }
                }
                self.registry.register_enum(
                    &ed.name,
                    ed.variants
                        .iter()
                        .map(|variant| variant.name.clone())
                        .collect(),
                );
                self.declared_header_arities.insert(ed.name.clone(), 0);
            }
            // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef)
            Statement::ClassLikeDef(cl) => match &cl.kind {
                ClassLikeKind::BuchiPack => {
                    let td = cl;
                    // E1501: Check for TypeDef name collision with existing types, functions, or molds
                    let has_collision = self.registry.type_defs.contains_key(&td.name)
                        || self.registry.enum_defs.contains_key(&td.name)
                        || self.func_types.contains_key(&td.name)
                        || self.registry.mold_defs.contains_key(&td.name);
                    if has_collision {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1501] Name '{}' is already defined in this scope. \
                                 Redefinition in the same scope is not allowed. \
                                 Hint: Use a different name, or define it in an inner scope (shadowing is allowed).",
                                td.name
                            ),
                            span: td.span.clone(),
                        });
                    }
                    self.validate_class_like_fields("TypeDef", &td.name, &td.fields);
                    let fields: Vec<(String, Type)> = td
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .map(|f| {
                            let ty = f
                                .type_annotation
                                .as_ref()
                                .map(|t| self.registry.resolve_type(t))
                                .unwrap_or(Type::Unknown);
                            (f.name.clone(), ty)
                        })
                        .collect();
                    self.registry.register_type(&td.name, fields);
                    self.declared_header_arities.insert(td.name.clone(), 0);
                    // E32B-020 (Lock-M): record the FieldDef list so the
                    // closed-constructor validator can distinguish data
                    // fields, method fields, and declare-only function
                    // fields when checking `Name(field <= value, ...)`
                    // call sites. Without this entry, BuchiPack-style
                    // TypeDefs fall through validation and silently
                    // accept undefined fields / type mismatches at
                    // runtime.
                    self.mold_field_defs
                        .insert(td.name.clone(), td.fields.clone());
                }
                ClassLikeKind::Mold { .. } => {
                    let md = cl;
                    // F42 sweep [E1501]: MoldDef collision check (the
                    // BuchiPack / Enum / Inheritance branches above
                    // already had this; the Mold branch was missing,
                    // so `Mold[T] => Box[T] = @(...)` and
                    // `Mold[T] => Box[T, U] = @(...)` would both
                    // register without complaint, silently giving the
                    // impression that arity overload is allowed).
                    // F42B-011 (Phase 2 lock = B / overload 禁止維持)
                    // requires the same enforcement at the MoldDef
                    // surface as at the BuchiPack / Enum surface.
                    let has_collision = self.registry.type_defs.contains_key(&md.name)
                        || self.registry.enum_defs.contains_key(&md.name)
                        || self.func_types.contains_key(&md.name)
                        || self.registry.mold_defs.contains_key(&md.name);
                    if has_collision {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1501] Name '{}' is already defined in this scope. \
                                 Redefinition in the same scope is not allowed (mold overload — \
                                 including arity-different overloads — is forbidden; use a different name). \
                                 Hint: Use a different name, or define it in an inner scope (shadowing is allowed).",
                                md.name
                            ),
                            span: md.span.clone(),
                        });
                    }
                    self.validate_class_like_fields("MoldDef", &md.name, &md.fields);
                    let header_args = Self::effective_mold_header_args(md);
                    self.validate_mold_root_header(md, &header_args);
                    self.validate_mold_extension_bindings(
                        MoldBindingDef {
                            kind: "MoldDef",
                            name: &md.name,
                            span: &md.span,
                        },
                        1,
                        &header_args,
                        &md.fields,
                        &HashSet::new(),
                    );
                    let type_params = Self::collect_mold_type_param_names(&header_args);
                    let fields: Vec<(String, Type)> = md
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .map(|f| {
                            let ty = f
                                .type_annotation
                                .as_ref()
                                .map(|t| self.registry.resolve_type(t))
                                .unwrap_or(Type::Unknown);
                            (f.name.clone(), ty)
                        })
                        .collect();
                    self.registry
                        .register_mold(&md.name, type_params, fields.clone());
                    self.registry.register_type(&md.name, fields);
                    self.mold_header_specs.insert(
                        md.name.clone(),
                        MoldHeaderSpec {
                            header_args: header_args.clone(),
                        },
                    );
                    self.mold_field_defs
                        .insert(md.name.clone(), md.fields.clone());
                    self.declared_header_arities
                        .insert(md.name.clone(), header_args.len());
                }
                ClassLikeKind::Inheritance { .. } => {
                    let inh = cl;
                    let inh_parent = inh.parent().expect("inheritance kind has parent");
                    let inh_child = &inh.name;
                    self.validate_class_like_fields("InheritanceDef", inh_child, &inh.fields);
                    let parent_header = self
                        .mold_header_specs
                        .get(inh_parent)
                        .map(|spec| spec.header_args.clone());
                    self.validate_inheritance_header_arities(inh, parent_header.as_deref());
                    let extra_fields: Vec<(String, Type)> = inh
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .map(|f| {
                            let ty = f
                                .type_annotation
                                .as_ref()
                                .map(|t| self.registry.resolve_type(t))
                                .unwrap_or(Type::Unknown);
                            (f.name.clone(), ty)
                        })
                        .collect();
                    if let Some(parent_fields) = self.registry.get_type_fields(inh_parent) {
                        for (child_name, child_ty) in &extra_fields {
                            if let Some((_, parent_ty)) =
                                parent_fields.iter().find(|(n, _)| n == child_name)
                                && !matches!(parent_ty, Type::Unknown)
                                && !matches!(child_ty, Type::Unknown)
                                && parent_ty != child_ty
                                && !self.registry.is_subtype_of(child_ty, parent_ty)
                            {
                                // (E30 Phase 3 / E30B-008) 旧 `[E1410]` 意味
                                // (InheritanceDef 子フィールド型互換) を `[E1411]` に移動。
                                // `[E1410]` は新意味 (declare-only function field requires
                                // default function or explicit value) 用に予約 (Phase 6 で
                                // E30B-004 defaultFn と同期して full 発火 path 実装予定)。
                                self.errors.push(TypeError {
                                    message: Self::binding_diag(
                                        "E1411",
                                        format!(
                                            "InheritanceDef '{}' redefines field '{}' with incompatible type '{}' (parent '{}' declares it as '{}')",
                                            inh_child, child_name, child_ty, inh_parent, parent_ty
                                        ),
                                        "A child type's field must be compatible with the parent's field type. \
                                         Use the same type or a subtype.",
                                    ),
                                    span: inh.span.clone(),
                                });
                            }
                        }
                    }

                    let registered = if self.registry.is_error_type(inh_parent) {
                        self.registry
                            .register_error_type(inh_parent, inh_child, extra_fields)
                    } else {
                        self.registry
                            .register_inheritance(inh_parent, inh_child, extra_fields)
                    };
                    if !registered {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1610] Cyclic inheritance detected: '{}' => '{}' would create a cycle in the inheritance chain. \
                                 Hint: Remove one of the inheritance relationships to break the cycle.",
                                inh_parent, inh_child
                            ),
                            span: inh.span.clone(),
                        });
                    }

                    if let Some(ref parent_header) = parent_header {
                        let child_header = inh
                            .name_args
                            .clone()
                            .or_else(|| inh.parent_args().cloned())
                            .unwrap_or_else(|| parent_header.clone());
                        self.validate_unique_mold_type_param_names(
                            "InheritanceDef",
                            inh_child,
                            &child_header,
                            &inh.span,
                        );
                        let parent_field_defs = self
                            .mold_field_defs
                            .get(inh_parent)
                            .cloned()
                            .unwrap_or_default();
                        let inherited_field_names: HashSet<String> = parent_field_defs
                            .iter()
                            .map(|field| field.name.clone())
                            .collect();
                        self.validate_mold_extension_bindings(
                            MoldBindingDef {
                                kind: "InheritanceDef",
                                name: inh_child,
                                span: &inh.span,
                            },
                            parent_header.len(),
                            &child_header,
                            &inh.fields,
                            &inherited_field_names,
                        );

                        let merged_field_defs =
                            Self::merge_field_defs(&parent_field_defs, &inh.fields);
                        let merged_fields: Vec<(String, Type)> = merged_field_defs
                            .iter()
                            .filter(|f| !f.is_method)
                            .map(|f| {
                                let ty = f
                                    .type_annotation
                                    .as_ref()
                                    .map(|t| self.registry.resolve_type(t))
                                    .unwrap_or(Type::Unknown);
                                (f.name.clone(), ty)
                            })
                            .collect();
                        self.registry.register_mold(
                            inh_child,
                            Self::collect_mold_type_param_names(&child_header),
                            merged_fields.clone(),
                        );
                        self.registry.register_type(inh_child, merged_fields);
                        self.mold_header_specs.insert(
                            inh_child.clone(),
                            MoldHeaderSpec {
                                header_args: child_header.clone(),
                            },
                        );
                        self.mold_field_defs
                            .insert(inh_child.clone(), merged_field_defs);
                    } else {
                        // E32B-020 (Lock-M): non-mold inheritance (the
                        // common Error path: `Error => MyError = @(...)`)
                        // also needs a `mold_field_defs` entry so the
                        // closed-constructor validator can see the
                        // merged parent + child field list. Without
                        // this, `MyError(feild <= "...")` typos would
                        // fall through unchecked because the parent has
                        // no header args and we'd otherwise skip the
                        // mold-style registration above.
                        let parent_field_defs = self
                            .mold_field_defs
                            .get(inh_parent)
                            .cloned()
                            .unwrap_or_default();
                        let merged_field_defs =
                            Self::merge_field_defs(&parent_field_defs, &inh.fields);
                        self.mold_field_defs
                            .insert(inh_child.clone(), merged_field_defs);
                    }

                    let parent_arity = parent_header
                        .as_ref()
                        .map(Vec::len)
                        .or_else(|| self.declared_header_arities.get(inh_parent).copied())
                        .unwrap_or(0);
                    let child_arity = if parent_header.is_some() {
                        self.inheritance_child_arity(inh, parent_arity)
                    } else {
                        parent_arity
                    };
                    self.declared_header_arities
                        .insert(inh_child.clone(), child_arity);
                }
            },
            Statement::FuncDef(fd) => {
                let duplicate_func_name = !self.seen_func_defs.insert(fd.name.clone());
                let generic_is_inferable = if fd.type_params.is_empty() {
                    true
                } else {
                    self.validate_generic_function_bindability(fd)
                };
                if duplicate_func_name {
                    self.invalid_func_defs.insert(fd.name.clone());
                    self.func_types.remove(&fd.name);
                    self.func_param_counts.remove(&fd.name);
                    self.func_param_types.remove(&fd.name);
                    self.func_defs.remove(&fd.name);
                    self.func_def_scope_depths.remove(&fd.name);
                    self.generic_func_defs.remove(&fd.name);
                } else if fd.type_params.is_empty() || generic_is_inferable {
                    self.invalid_func_defs.remove(&fd.name);
                    if let Some((param_types, ret_ty)) = self.finalize_named_function_signature(fd)
                    {
                        if fd.type_params.is_empty() {
                            self.func_defs.insert(fd.name.clone(), fd.clone());
                        }
                        self.func_types.insert(fd.name.clone(), ret_ty);
                        self.func_param_counts
                            .insert(fd.name.clone(), fd.params.len());
                        self.func_param_types.insert(fd.name.clone(), param_types);
                        if !fd.type_params.is_empty() {
                            self.generic_func_defs.insert(fd.name.clone(), fd.clone());
                        }
                    } else {
                        self.invalid_func_defs.insert(fd.name.clone());
                        self.func_types.remove(&fd.name);
                        self.func_param_counts.remove(&fd.name);
                        self.func_param_types.remove(&fd.name);
                        self.func_defs.remove(&fd.name);
                        self.func_def_scope_depths.remove(&fd.name);
                        self.generic_func_defs.remove(&fd.name);
                    }
                } else {
                    self.invalid_func_defs.insert(fd.name.clone());
                    self.func_types.remove(&fd.name);
                    self.func_param_counts.remove(&fd.name);
                    self.func_param_types.remove(&fd.name);
                    self.func_defs.remove(&fd.name);
                    self.func_def_scope_depths.remove(&fd.name);
                    self.generic_func_defs.remove(&fd.name);
                }
            }
            Statement::Import(imp) => {
                // Core bundled package signatures (imported symbol path).
                if imp.path == "taida-lang/crypto" {
                    for sym in &imp.symbols {
                        if let Some(kind) = CryptoSym::from_export(&sym.name) {
                            let local_name = sym.alias.as_ref().unwrap_or(&sym.name).clone();
                            self.func_types
                                .insert(local_name.clone(), kind.return_type());
                            self.func_param_counts
                                .insert(local_name.clone(), kind.max_arity());
                            self.crypto_funcs.insert(local_name.clone(), kind);
                            // sha256 keeps its dedicated set for the unchanged
                            // legacy [E1506] message wording.
                            if sym.name == "sha256" {
                                self.crypto_sha256_funcs.insert(local_name);
                            }
                        }
                    }
                } else if imp.path == "taida-lang/net" {
                    for sym in &imp.symbols {
                        let local_name = sym.alias.as_ref().unwrap_or(&sym.name);
                        self.register_net_import_symbol(&sym.name, local_name);
                    }
                } else if imp.path == "taida-lang/os" {
                    for sym in &imp.symbols {
                        let local_name = sym.alias.as_ref().unwrap_or(&sym.name).clone();
                        self.register_os_import_symbol(&sym.name, &local_name);
                    }
                } else if imp.path == "taida-lang/abi" {
                    self.register_abi_imports(&imp.symbols);
                }
            }
            _ => {}
        }
    }

    pub(super) fn resolve_mold_header_type(
        &self,
        ty: &TypeExpr,
        bound_types: &HashMap<String, Type>,
    ) -> Type {
        match ty {
            TypeExpr::Named(name) => bound_types
                .get(name)
                .cloned()
                .unwrap_or_else(|| self.registry.resolve_type(ty)),
            TypeExpr::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|field| {
                        let field_ty = field
                            .type_annotation
                            .as_ref()
                            .map(|ty| self.resolve_mold_header_type(ty, bound_types))
                            .unwrap_or(Type::Unknown);
                        (field.name.clone(), field_ty)
                    })
                    .collect(),
            ),
            TypeExpr::List(inner) => {
                Type::List(Box::new(self.resolve_mold_header_type(inner, bound_types)))
            }
            TypeExpr::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| self.resolve_mold_header_type(arg, bound_types))
                    .collect(),
            ),
            TypeExpr::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.resolve_mold_header_type(param, bound_types))
                    .collect(),
                Box::new(self.resolve_mold_header_type(ret, bound_types)),
            ),
        }
    }

    pub(super) fn substitute_generic_type(
        &self,
        pattern: &Type,
        generic_names: &HashSet<String>,
        bindings: &HashMap<String, Type>,
    ) -> Type {
        match pattern {
            Type::Named(name) if generic_names.contains(name) => bindings
                .get(name)
                .cloned()
                .unwrap_or_else(|| pattern.clone()),
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            self.substitute_generic_type(ty, generic_names, bindings),
                        )
                    })
                    .collect(),
            ),
            Type::List(inner) => Type::List(Box::new(self.substitute_generic_type(
                inner,
                generic_names,
                bindings,
            ))),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| self.substitute_generic_type(arg, generic_names, bindings))
                    .collect(),
            ),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.substitute_generic_type(param, generic_names, bindings))
                    .collect(),
                Box::new(self.substitute_generic_type(ret, generic_names, bindings)),
            ),
            _ => pattern.clone(),
        }
    }

    pub(super) fn instantiate_generic_type(
        &self,
        pattern: &Type,
        generic_names: &HashSet<String>,
        bindings: &HashMap<String, Type>,
    ) -> Type {
        match pattern {
            Type::Named(name) if generic_names.contains(name) => {
                bindings.get(name).cloned().unwrap_or(Type::Unknown)
            }
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            self.instantiate_generic_type(ty, generic_names, bindings),
                        )
                    })
                    .collect(),
            ),
            Type::List(inner) => Type::List(Box::new(self.instantiate_generic_type(
                inner,
                generic_names,
                bindings,
            ))),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| self.instantiate_generic_type(arg, generic_names, bindings))
                    .collect(),
            ),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.instantiate_generic_type(param, generic_names, bindings))
                    .collect(),
                Box::new(self.instantiate_generic_type(ret, generic_names, bindings)),
            ),
            _ => pattern.clone(),
        }
    }

    pub(super) fn is_worker_safe_type(&self, ty: &Type) -> bool {
        let mut seen = HashSet::new();
        self.is_worker_safe_type_inner(ty, &mut seen)
    }
}
