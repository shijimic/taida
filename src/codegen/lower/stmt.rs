// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups stmt methods of the `Lowering` struct (per the lower/ split's
// placement table). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{ImportedSymbolKind, LowerError, Lowering, simple_hash};
use crate::codegen::ir::*;
use crate::net_surface::NET_HTTP_PROTOCOL_SYMBOL;
use crate::parser::*;

impl Lowering {
    pub fn lower_program(&mut self, program: &Program) -> Result<IrModule, LowerError> {
        if !self.typed_expr_table.is_empty()
            && !self.typed_expr_table.residual_unknown_types().is_empty()
        {
            let residuals = self
                .typed_expr_table
                .residual_unknown_types()
                .into_iter()
                .take(5)
                .map(|ty| ty.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(LowerError {
                message: format!(
                    "Typed expression table contains unresolved types before lowering: {}",
                    residuals
                ),
            });
        }

        let mut module = IrModule::new();
        module.module_key = Some(self.current_module_key().to_string());

        // 1st pass: 関数定義、型定義、エクスポート/インポートを収集
        for stmt in &program.statements {
            match stmt {
                Statement::FuncDef(func_def) => {
                    self.user_funcs.insert(func_def.name.clone());
                    self.func_param_defs
                        .insert(func_def.name.clone(), func_def.params.clone());
                    // Track return types for type inference in binary ops
                    if let Some(ref rt) = func_def.return_type {
                        match rt {
                            crate::parser::TypeExpr::Named(n) if n == "Str" => {
                                self.string_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::Named(n) if n == "Float" => {
                                self.float_returning_funcs.insert(func_def.name.clone());
                            }
                            // NB-31: Track Int/Num-returning functions for callable_type_tag
                            // Num is rejected as a value annotation by the
                            // checker; registering it as Int here made a
                            // --no-check Float body render as raw bits.
                            crate::parser::TypeExpr::Named(n) if n == "Int" => {
                                self.int_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::List(_) => {
                                self.list_returning_funcs.insert(func_def.name.clone());
                            }
                            // C18-2: Functions declared `=> :SomeEnum` should
                            // propagate the enum type to call sites so that
                            // `@(state <= pickColor(n))` tags `state` as an
                            // Enum field for jsonEncode variant-name output.
                            crate::parser::TypeExpr::Named(n) if self.enum_defs.contains_key(n) => {
                                self.enum_returning_funcs
                                    .insert(func_def.name.clone(), n.clone());
                            }
                            _ => {}
                        }
                    }
                    // F-58/F-60: Detect functions that return BuchiPack/TypeInst
                    if Self::func_body_returns_pack(&func_def.body) {
                        self.pack_returning_funcs.insert(func_def.name.clone());
                    }
                    // retain-on-store: Detect functions that return List
                    if Self::func_body_returns_list(&func_def.body) {
                        self.list_returning_funcs.insert(func_def.name.clone());
                    }
                    // C12B-022: Detect TypeIs[param, :PrimitiveType]() usage on
                    // function parameters. Such callers need full arg tag
                    // propagation (including INT=0 default) so the runtime
                    // primitive-tag-match helper can distinguish Int/Bool/Str/Float.
                    let param_names: std::collections::HashSet<String> =
                        func_def.params.iter().map(|p| p.name.clone()).collect();
                    if Self::body_uses_typeis_on_ident(&func_def.body, &param_names) {
                        self.param_type_check_funcs.insert(func_def.name.clone());
                    }
                }
                // (E30 Phase 2 Sub-step 2.1) ClassLikeDef 単一 variant に統合。
                // 内部で kind discriminator dispatch して旧 3 系統の lowering 経路を維持。
                Statement::ClassLikeDef(cl) => match &cl.kind {
                    crate::parser::ClassLikeKind::BuchiPack => {
                        let type_def = cl;
                        let non_method_field_defs: Vec<crate::parser::FieldDef> = type_def
                            .fields
                            .iter()
                            .filter(|f| !f.is_method)
                            .cloned()
                            .collect();
                        let fields: Vec<String> = type_def
                            .fields
                            .iter()
                            .filter(|f| !f.is_method)
                            .map(|f| f.name.clone())
                            .collect();
                        // Register field names and types for jsonEncode
                        for field_def in &non_method_field_defs {
                            self.field_names.insert(field_def.name.clone());
                            // Map type annotation to type tag
                            if let Some(ref ty) = field_def.type_annotation {
                                let tag = match ty {
                                    crate::parser::TypeExpr::Named(n) => match n.as_str() {
                                        "Int" => 1,
                                        "Float" => 2,
                                        "Str" => 3,
                                        "Bool" => 4,
                                        // C18-2: Enum-typed field → tag 5 (Enum).
                                        other => {
                                            if let Some(variants) = self.enum_defs.get(other) {
                                                self.field_enum_descriptors.insert(
                                                    field_def.name.clone(),
                                                    variants.join(","),
                                                );
                                                5
                                            } else {
                                                0
                                            }
                                        }
                                    },
                                    _ => 0,
                                };
                                self.register_field_type_tag(&field_def.name, tag);
                            } else if let Some(ref default_expr) = field_def.default_value
                                && self.expr_is_bool(default_expr)
                            {
                                self.register_field_type_tag(&field_def.name, 4);
                            }
                        }
                        self.type_fields.insert(type_def.name.clone(), fields);
                        let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                            non_method_field_defs
                                .iter()
                                .map(|f| (f.name.clone(), f.type_annotation.clone()))
                                .collect();
                        self.type_field_types
                            .insert(type_def.name.clone(), field_types);
                        self.type_field_defs
                            .insert(type_def.name.clone(), non_method_field_defs);
                        let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                            .fields
                            .iter()
                            .filter_map(|f| {
                                if f.is_method {
                                    f.method_def.clone().map(|method| (f.name.clone(), method))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !methods.is_empty() {
                            self.type_method_defs.insert(type_def.name.clone(), methods);
                        }
                    }
                    crate::parser::ClassLikeKind::Mold { .. } => {
                        let mold_def = cl;
                        let non_method_field_defs: Vec<crate::parser::FieldDef> = mold_def
                            .fields
                            .iter()
                            .filter(|f| !f.is_method)
                            .cloned()
                            .collect();
                        let fields: Vec<String> = non_method_field_defs
                            .iter()
                            .map(|f| f.name.clone())
                            .collect();
                        for field_def in &non_method_field_defs {
                            self.field_names.insert(field_def.name.clone());
                            if let Some(ref ty) = field_def.type_annotation {
                                let tag = match ty {
                                    crate::parser::TypeExpr::Named(n) => match n.as_str() {
                                        "Int" => 1,
                                        "Float" => 2,
                                        "Str" => 3,
                                        "Bool" => 4,
                                        _ => 0,
                                    },
                                    _ => 0,
                                };
                                self.register_field_type_tag(&field_def.name, tag);
                            } else if let Some(ref default_expr) = field_def.default_value
                                && self.expr_is_bool(default_expr)
                            {
                                self.register_field_type_tag(&field_def.name, 4);
                            }
                        }
                        self.type_fields.insert(mold_def.name.clone(), fields);
                        let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                            non_method_field_defs
                                .iter()
                                .map(|f| (f.name.clone(), f.type_annotation.clone()))
                                .collect();
                        self.type_field_types
                            .insert(mold_def.name.clone(), field_types);
                        self.type_field_defs
                            .insert(mold_def.name.clone(), non_method_field_defs);
                        self.mold_defs
                            .insert(mold_def.name.clone(), mold_def.clone());
                    }
                    crate::parser::ClassLikeKind::Inheritance {
                        parent,
                        parent_args,
                    } => {
                        let inh_def = cl;
                        let inh_child = &inh_def.name;
                        let inh_parent = parent;
                        let mut all_fields = self
                            .type_fields
                            .get(inh_parent)
                            .cloned()
                            .unwrap_or_default();
                        let mut all_field_types = self
                            .type_field_types
                            .get(inh_parent)
                            .cloned()
                            .unwrap_or_default();
                        let mut all_field_defs = self
                            .type_field_defs
                            .get(inh_parent)
                            .cloned()
                            .unwrap_or_default();
                        for field in inh_def.fields.iter().filter(|f| !f.is_method) {
                            all_fields.retain(|name| name != &field.name);
                            all_fields.push(field.name.clone());
                            all_field_types.retain(|(name, _)| name != &field.name);
                            all_field_types
                                .push((field.name.clone(), field.type_annotation.clone()));
                            all_field_defs.retain(|f| f.name != field.name);
                            all_field_defs.push(field.clone());
                        }
                        self.type_fields.insert(inh_child.clone(), all_fields);
                        self.type_field_types
                            .insert(inh_child.clone(), all_field_types);
                        self.type_field_defs
                            .insert(inh_child.clone(), all_field_defs);
                        // Inherit parent methods, then override/add child methods
                        let mut all_methods = self
                            .type_method_defs
                            .get(inh_parent)
                            .cloned()
                            .unwrap_or_default();
                        for field in inh_def.fields.iter().filter(|f| f.is_method) {
                            if let Some(method) = field.method_def.clone() {
                                all_methods.retain(|(name, _)| name != &field.name);
                                all_methods.push((field.name.clone(), method));
                            }
                        }
                        if !all_methods.is_empty() {
                            self.type_method_defs.insert(inh_child.clone(), all_methods);
                        }
                        if let Some(parent_mold) = self.mold_defs.get(inh_parent).cloned() {
                            let parent_mold_args: Vec<crate::parser::MoldHeaderArg> =
                                parent_mold.mold_args().cloned().unwrap_or_default();
                            let parent_name_args = parent_mold.name_args.clone();
                            let parent_type_params = parent_mold.type_params.clone();
                            let mut merged_mold_fields = parent_mold.fields.clone();
                            for child_field in &inh_def.fields {
                                if let Some(existing) = merged_mold_fields
                                    .iter_mut()
                                    .find(|field| field.name == child_field.name)
                                {
                                    *existing = child_field.clone();
                                } else {
                                    merged_mold_fields.push(child_field.clone());
                                }
                            }
                            self.mold_defs.insert(
                                inh_child.clone(),
                                crate::parser::ClassLikeDef {
                                    name: inh_child.clone(),
                                    fields: merged_mold_fields,
                                    doc_comments: inh_def.doc_comments.clone(),
                                    span: inh_def.span.clone(),
                                    kind: crate::parser::ClassLikeKind::Mold {
                                        mold_args: parent_mold_args,
                                    },
                                    name_args: inh_def
                                        .name_args
                                        .clone()
                                        .or_else(|| parent_args.clone())
                                        .or(parent_name_args),
                                    type_params: parent_type_params,
                                },
                            );
                        }
                    }
                },
                Statement::EnumDef(enum_def) => {
                    self.enum_defs.insert(
                        enum_def.name.clone(),
                        enum_def
                            .variants
                            .iter()
                            .map(|variant| variant.name.clone())
                            .collect(),
                    );
                    self.register_enum_type_id(&enum_def.name);
                }
                Statement::Export(export_stmt) => {
                    // RCB-212: Re-export path `<<< ./path` is not supported.
                    if export_stmt.path.is_some() {
                        return Err(LowerError {
                            message: "Re-export with path (`<<< ./path`) is not yet supported. \
                                     Use explicit import and re-export instead."
                                .to_string(),
                        });
                    }
                    for sym in &export_stmt.symbols {
                        self.exported_symbols.insert(sym.clone());
                        module.exports.push(self.export_func_symbol(sym));
                    }
                }
                Statement::Import(import_stmt) => {
                    // C18-1: Before any addon / stdlib classification,
                    // pull in Enum type definitions exported by the target
                    // module so `Color:Red()` in the importer can resolve
                    // at codegen time. The call is a no-op for
                    // `taida-lang/*` and `npm:*` paths.
                    if !import_stmt.path.starts_with("taida-lang/")
                        && !import_stmt.path.starts_with("npm:")
                    {
                        self.absorb_cross_module_enum_defs(import_stmt);
                    }

                    // stdlib モジュールの関数はランタイム関数にマッピング
                    // 定数は stdlib_constants にマッピング
                    // RCB-213: version is now passed through to resolve_import_path
                    // for version-aware package resolution (.taida/deps/org/name@ver/).
                    let path = &import_stmt.path;
                    let version = import_stmt.version.as_deref();

                    // RC2.5 Phase 1: addon-backed package dispatch.
                    //
                    // Cranelift native compile path now routes addon
                    // imports through a C-side dispatcher
                    // (`taida_addon_call` in native_runtime.c) that
                    // lazily `dlopen`s the cdylib at first call.
                    //
                    // Resolution order matches the interpreter:
                    //   1. `ensure_addon_supported(backend, path)` —
                    //      honours the backend policy table; wasm / js
                    //      targets still reject with the deterministic
                    //      "not supported on backend" message.
                    //   2. `resolve_cdylib_path` — absolute path is
                    //      embedded in `.rodata` at build time
                    //      (RC2.5B-004 known limitation)
                    //   3. manifest `[functions]` supplies arity
                    //
                    // If `try_locate_addon_pkg_dir` finds an addon
                    // package, we skip the normal stdlib / user-
                    // function classification below and jump straight
                    // into `lower_addon_import`.
                    if let Some(addon_pkg_dir) = self.try_locate_addon_pkg_dir(path, version)
                        && addon_pkg_dir.join("native").join("addon.toml").exists()
                    {
                        crate::addon::ensure_addon_supported(self.addon_backend, path).map_err(
                            |e| LowerError {
                                message: e.to_string(),
                            },
                        )?;
                        self.lower_addon_import(&addon_pkg_dir, path, import_stmt)?;
                        continue;
                    }

                    // F54: derive the core-bundled classification from the
                    // package catalog instead of a per-layer hard-coded list.
                    let is_core_bundled_path = path
                        .as_str()
                        .split_once('/')
                        .is_some_and(|(org, name)| crate::pkg::catalog::is_core_bundled(org, name));

                    // B11B-023 + B11B-026: Pre-resolve facade once per import statement
                    // instead of per-symbol. Validates all symbols at once.
                    let pre_resolved_facade: Option<crate::pkg::facade::FacadeContext> = {
                        if !is_core_bundled_path
                            && !path.starts_with("./")
                            && !path.starts_with("../")
                            && !path.starts_with('/')
                            && !path.starts_with("~/")
                            && !path.starts_with("std/")
                            && !path.starts_with("npm:")
                            && path.contains('/')
                        {
                            let source_dir_opt = self.source_dir.as_ref();
                            if let Some(source_dir) = source_dir_opt {
                                let root = Self::find_project_root(source_dir);
                                let resolution = if let Some(ver) = version {
                                    crate::pkg::resolver::resolve_package_module_versioned(
                                        &root, path, ver,
                                    )
                                } else {
                                    crate::pkg::resolver::resolve_package_module(&root, path)
                                };
                                if let Some(res) = resolution {
                                    if res.submodule.is_none() {
                                        crate::pkg::facade::resolve_facade_context(&res.pkg_dir)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };

                    // B11B-023: Validate all symbols against facade at once
                    if let Some(ref ctx) = pre_resolved_facade {
                        let sym_names: Vec<String> =
                            import_stmt.symbols.iter().map(|s| s.name.clone()).collect();
                        let violations = crate::pkg::facade::validate_facade(
                            &ctx.facade_exports,
                            &ctx.entry_path,
                            &sym_names,
                        );
                        if let Some(v) = violations.first() {
                            return Err(LowerError {
                                message: crate::pkg::facade::format_facade_violation(v),
                            });
                        }
                    }

                    let mut import_link_symbols = Vec::new();
                    let mut needs_module_object = false;
                    for sym in &import_stmt.symbols {
                        let orig_name = &sym.name;
                        let alias = sym.alias.clone().unwrap_or_else(|| sym.name.clone());

                        // std/ imports: only std/io is still supported (backward compat)
                        // std/math, std/time, etc. are removed after std dissolution
                        if path.starts_with("std/") && path != "std/io" {
                            // Skip removed std modules silently
                            continue;
                        }

                        // 関数チェック
                        let runtime_name = match path.as_str() {
                            "std/io" => Self::stdlib_io_mapping(orig_name),
                            "taida-lang/os" => Self::os_func_mapping(orig_name),
                            "taida-lang/crypto" => Self::crypto_func_mapping(orig_name),
                            "taida-lang/net" => Self::net_func_mapping(orig_name),
                            "taida-lang/pool" => Self::pool_func_mapping(orig_name),
                            "taida-lang/abi" => Self::abi_func_mapping(orig_name),
                            _ => None,
                        };

                        if let Some(rt_name) = runtime_name {
                            self.stdlib_runtime_funcs.insert(alias, rt_name.to_string());
                        } else if path == "taida-lang/net" && orig_name == NET_HTTP_PROTOCOL_SYMBOL
                        {
                            self.register_net_enum_import(&alias);
                        } else if is_core_bundled_path {
                            if path == "taida-lang/net" {
                                return Err(LowerError {
                                    message: format!(
                                        "Symbol '{}' not found in module '{}'",
                                        orig_name, path
                                    ),
                                });
                            }
                            // Core-bundled symbols that do not have native runtime mapping yet
                            // are intentionally skipped here (e.g. pool contract placeholders).
                            // This prevents unresolved pseudo-user-function stubs.
                            continue;
                        } else {
                            // stdlib でないか、マッピングのない関数はユーザー関数として登録
                            // QF-16/17: シンボルの種類に応じて処理を分岐
                            // B11B-023/026: pass pre-resolved facade to avoid per-symbol manifest reads
                            let sym_kind = self.classify_imported_symbol(
                                path,
                                orig_name,
                                version,
                                pre_resolved_facade.as_ref(),
                            )?;
                            let module_key = self.import_module_key(path, version);
                            let init_symbol = Self::init_symbol_for_key(&module_key);
                            needs_module_object = true;
                            match sym_kind {
                                ImportedSymbolKind::Value => {
                                    // 値 export: module init 後に GlobalGet で取得
                                    self.imported_value_symbols.push((
                                        alias.clone(),
                                        orig_name.clone(),
                                        module_key,
                                    ));
                                    self.imported_value_names.insert(alias.clone());
                                    self.pack_vars.insert(alias.clone());
                                    // user_funcs には入れない（UseVar で解決する）
                                    // init 関数を呼ぶ必要がある
                                    if !self.module_inits_needed.contains(&init_symbol) {
                                        self.module_inits_needed.push(init_symbol);
                                    }
                                }
                                ImportedSymbolKind::TypeDef => {
                                    // TypeDef export: メタデータを登録（インラインで TypeInst 構築）
                                    self.imported_type_symbols.insert(alias.clone());
                                    self.register_imported_typedef(
                                        path, orig_name, &alias, version,
                                    );
                                }
                                ImportedSymbolKind::Function => {
                                    // 通常の関数 export
                                    let link_name =
                                        Self::export_func_symbol_for_key(&module_key, orig_name);
                                    self.user_funcs.insert(alias.clone());
                                    self.imported_func_links
                                        .insert(alias.clone(), link_name.clone());
                                    import_link_symbols.push(link_name);
                                    // 関数 import がある場合も init 関数を呼ぶ必要がある
                                    // （関数が参照する private value の初期化のため）
                                    if !self.module_inits_needed.contains(&init_symbol) {
                                        self.module_inits_needed.push(init_symbol);
                                    }
                                }
                            }
                        }
                    }

                    // ローカルモジュール依存は、値/TypeDef import だけでも object を生成する必要がある。
                    if needs_module_object {
                        module.imports.push((
                            import_stmt.path.clone(),
                            import_link_symbols,
                            import_stmt.version.clone(),
                        ));
                    }
                }
                _ => {}
            }
        }

        self.register_mold_solidify_helpers()?;

        // Pre-2nd pass: トップレベル変数名と型情報を収集（Native グローバル変数テーブル用）
        for stmt in &program.statements {
            if let Statement::Assignment(assign) = stmt {
                self.top_level_vars.insert(assign.target.clone());
                // 型情報を事前登録（2nd pass の lower_func_def 内で正しく型判定するため）
                if self.expr_is_int(&assign.value) {
                    self.int_vars.insert(assign.target.clone());
                }
                if self.expr_is_string_full(&assign.value) {
                    self.string_vars.insert(assign.target.clone());
                }
                if self.expr_returns_float(&assign.value) {
                    self.float_vars.insert(assign.target.clone());
                }
                if self.expr_is_bool(&assign.value) {
                    self.bool_vars.insert(assign.target.clone());
                }
                if self.expr_is_pack(&assign.value) {
                    self.pack_vars.insert(assign.target.clone());
                    // Record per-field static kinds for pack literals so a
                    // later `p.x` field read carries its element kind into
                    // display dispatch (Float/Bool payloads are invisible
                    // to the value heuristics).
                    if let Expr::BuchiPack(fields, _) = &assign.value {
                        let kinds: std::collections::HashMap<String, i64> = fields
                            .iter()
                            .filter(|f| !matches!(f.value, Expr::Placeholder(_)))
                            .map(|f| (f.name.clone(), self.expr_type_tag(&f.value)))
                            .collect();
                        self.pack_field_kinds.insert(assign.target.clone(), kinds);
                    } else {
                        self.pack_field_kinds.remove(&assign.target);
                    }
                }
                if self.expr_is_list(&assign.value) {
                    self.list_vars.insert(assign.target.clone());
                    // C21-4: Infer homogeneous element type from ListLit.
                    // If every element of `@[...]` has the same primitive
                    // type (FloatLit / IntLit / StringLit / BoolLit), record
                    // it so later unmold via `a.get(i) >=> av` can tag `av`.
                    if let Expr::ListLit(elems, _) = &assign.value
                        && !elems.is_empty()
                    {
                        let elem_ty = match &elems[0] {
                            Expr::FloatLit(_, _) => Some("Float"),
                            Expr::IntLit(_, _) => Some("Int"),
                            Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => Some("Str"),
                            Expr::BoolLit(_, _) => Some("Bool"),
                            _ => None,
                        };
                        if let Some(ty) = elem_ty {
                            let homogeneous = elems.iter().all(|e| {
                                matches!(
                                    (ty, e),
                                    ("Float", Expr::FloatLit(_, _))
                                        | ("Int", Expr::IntLit(_, _))
                                        | ("Str", Expr::StringLit(_, _) | Expr::TemplateLit(_, _))
                                        | ("Bool", Expr::BoolLit(_, _))
                                )
                            });
                            if homogeneous {
                                self.list_element_types
                                    .insert(assign.target.clone(), ty.to_string());
                            }
                        }
                    }
                }
                // QF-34 / F58B-003: MoldInst の Lax 内部型を追跡（unmold 時の型推定用）
                self.record_lax_inner_type(&assign.target, &assign.value);
                // QF-10: TypeInst の変数に TypeDef 名を記録
                if let Expr::TypeInst(type_name, _, _) = &assign.value {
                    self.var_type_names
                        .insert(assign.target.clone(), type_name.clone());
                }
            }
            // `>=>` / `<=<` bindings are top-level variables too: the
            // free-var collector filters on this set, so a name missing
            // here never reaches globals_referenced — a function body
            // referencing an unmold-bound top-level then reads 0 from
            // the uninitialised global slot (silently, on native and
            // wasm; the interpreter scopes it correctly).
            if let Statement::UnmoldForward(uf) = stmt {
                self.top_level_vars.insert(uf.target.clone());
            }
            if let Statement::UnmoldBackward(ub) = stmt {
                self.top_level_vars.insert(ub.target.clone());
            }
        }

        // ライブラリモジュール判定（2nd pass の前に実施 — is_library_module フラグが必要）
        module.is_library = !module.exports.is_empty();
        self.is_library_module = module.is_library;

        // 2nd pass: ユーザー定義関数を IR に変換
        for stmt in &program.statements {
            if let Statement::FuncDef(func_def) = stmt {
                let ir_func = self.lower_func_def(func_def)?;
                module.functions.push(ir_func);
            }
        }

        // C25B-030 Phase 1E-β: lower facade-declared FuncDefs
        // harvested during `lower_addon_import`. Each entry is
        // `(local_name, FuncDef, mangled_link_symbol)`; we
        // temporarily rewrite the FuncDef so `lower_func_def`'s
        // `resolve_user_func_symbol` returns the mangled symbol
        // directly without tripping the usual user-function
        // export-symbol mangling. The mangle is stable across
        // multiple imports of the same addon (see
        // `addon_facade_mangled` dedup in `lower_addon_import`).
        //
        // We drain the vec so repeated calls to `lower_program`
        // on the same `Lowering` do not re-emit duplicate IR
        // functions. Should that ever happen in practice
        // (e.g. testing harnesses) the caller has to reset the
        // `Lowering` state first.
        let facade_funcs = std::mem::take(&mut self.addon_facade_funcs);
        for (local_name, func_def, mangled) in &facade_funcs {
            // Route the body's `resolve_user_func_symbol(local_name)`
            // through the mangled link so self- and sibling-calls
            // (including recursion and cross-FuncDef dispatch inside
            // the same facade) land on the correct symbol.
            self.imported_func_links
                .insert(local_name.clone(), mangled.clone());
            if func_def.name != *local_name {
                self.imported_func_links
                    .insert(func_def.name.clone(), mangled.clone());
            }
            let ir_func = self.lower_func_def(func_def)?;
            module.functions.push(ir_func);
        }
        // Put the harvested list back so post-lowering consumers
        // (e.g. driver.rs diagnostics, potential test harnesses)
        // can still observe what was emitted.
        self.addon_facade_funcs = facade_funcs;

        // ライブラリモジュールの場合、モジュール単位の init 関数を生成
        if module.is_library {
            self.generate_module_init_func(&mut module, program)?;
        }

        // 3rd pass: トップレベル文を _taida_main に変換（ライブラリでない場合のみ）
        if !module.is_library {
            self.current_heap_vars.clear();
            let mut main_fn = IrFunction::new("_taida_main".to_string());

            self.emit_imported_module_inits(&mut main_fn);
            self.bind_imported_values(&mut main_fn);

            // RC2.5 Phase 2 / C25B-030 Phase 1E-β-2: replay addon
            // facade pack/value bindings before any user statement
            // runs. The bindings are synthetic `Name <= @(...)`
            // (or scalar) assignments harvested from the addon's
            // `taida/<stem>.td` facade tree during
            // `lower_addon_import`; they are the native-backend
            // equivalent of the `module::load_addon_facade`
            // path used by the interpreter.
            //
            // Each binding is emitted with both `DefVar` (so user
            // code in main runs with a local scope binding) and
            // `GlobalSet` (so facade FuncDef bodies compiled
            // elsewhere in the IR can `GlobalGet(hash)` the
            // binding at runtime). Without the `GlobalSet`, a
            // facade FuncDef body that references a private
            // `_Cell` / `_KK_Char` helper would read 0 from the
            // uninitialised global slot and behave as if the
            // binding never existed.
            let facade_bindings = std::mem::take(&mut self.addon_facade_pack_bindings);
            for (name, expr) in &facade_bindings {
                let val = self.lower_expr(&mut main_fn, expr)?;
                main_fn.push(IrInst::DefVar(name.clone(), val));
                let hash = self.global_var_hash(name);
                main_fn.push(IrInst::GlobalSet(hash, val));
            }
            // Put the bindings back so repeat calls to lower_program
            // (if any) would still see them. In practice lower_program
            // is called once per build, but take()/put-back is safer
            // than leaving the vec drained and silently losing data.
            self.addon_facade_pack_bindings = facade_bindings;

            let top_level_stmts: Vec<&Statement> = program
                .statements
                .iter()
                .filter(|s| !matches!(s, Statement::FuncDef(_)))
                .collect();
            self.lower_statement_sequence(&mut main_fn, &top_level_stmts)?;

            // Emit field name registrations for jsonEncode (after all field names collected)
            // Prepend to the beginning of _taida_main body
            let mut reg_insts = Vec::new();
            let mut sorted_names: Vec<String> = self.field_names.iter().cloned().collect();
            sorted_names.sort(); // deterministic order
            for name in &sorted_names {
                let hash = simple_hash(name);
                let type_tag = self.field_type_tags.get(name).copied().unwrap_or(0);
                if type_tag == 5 {
                    // C18-2: Enum field — register with variants CSV.
                    let variants_csv = self
                        .field_enum_descriptors
                        .get(name)
                        .cloned()
                        .unwrap_or_default();
                    let hash_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(hash_var, hash as i64));
                    let name_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstStr(name_var, name.clone()));
                    let variants_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstStr(variants_var, variants_csv));
                    let result_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::Call(
                        result_var,
                        "taida_register_field_enum".to_string(),
                        vec![hash_var, name_var, variants_var],
                    ));
                } else if type_tag > 0 {
                    // Use register_field_type (with type tag)
                    let hash_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(hash_var, hash as i64));
                    let name_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstStr(name_var, name.clone()));
                    let tag_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(tag_var, type_tag));
                    let result_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::Call(
                        result_var,
                        "taida_register_field_type".to_string(),
                        vec![hash_var, name_var, tag_var],
                    ));
                } else {
                    // Use register_field_name (no type info)
                    let hash_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(hash_var, hash as i64));
                    let name_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstStr(name_var, name.clone()));
                    let result_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::Call(
                        result_var,
                        "taida_register_field_name".to_string(),
                        vec![hash_var, name_var],
                    ));
                }
            }
            if !reg_insts.is_empty() {
                let body = std::mem::take(&mut main_fn.body);
                main_fn.body = reg_insts;
                main_fn.body.extend(body);
            }

            // _taida_main 終了時: 全ヒープ変数を Release
            let heap_vars = std::mem::take(&mut self.current_heap_vars);
            for name in &heap_vars {
                let use_var = main_fn.alloc_var();
                main_fn.push(IrInst::UseVar(use_var, name.clone()));
                main_fn.push(IrInst::Release(use_var));
            }

            let ret_var = main_fn.alloc_var();
            main_fn.push(IrInst::ConstInt(ret_var, 0));
            main_fn.push(IrInst::Return(ret_var));

            module.functions.push(main_fn);
        }

        // ラムダから生成された関数を追加
        for lambda_fn in std::mem::take(&mut self.lambda_funcs) {
            module.functions.push(lambda_fn);
        }

        Ok(module)
    }

    pub(super) fn lower_func_def(&mut self, func_def: &FuncDef) -> Result<IrFunction, LowerError> {
        let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
        let parent_scope_vars = self.collect_nested_scope_vars(params.clone(), &func_def.body);

        let mangled = self.resolve_user_func_symbol(&func_def.name);
        let mut ir_func = IrFunction::new_with_params(mangled, params.clone());

        // Scope-aware net builtin shadowing: snapshot before function body,
        // restore after. This covers both parameter shadows and local assignment
        // shadows (e.g. `httpServe <= add`) within the function scope.
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        // NB3-4: Snapshot var_aliases and lambda_param_counts so that aliases
        // defined inside this function do not leak to sibling/parent scopes.
        let prev_var_aliases = self.var_aliases.clone();
        let prev_lambda_param_counts = self.lambda_param_counts.clone();
        // Value-tag track: shadow kinds are IR variables of the CURRENT
        // function body — a nested body must not UseVar a parent's shadow.
        let prev_shadow_kinds = std::mem::take(&mut self.shadow_kind_vars);
        let prev_lambda_vars = self.lambda_vars.clone();
        let prev_closure_vars = self.closure_vars.clone();
        for p in &func_def.params {
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

        // ヒープ変数トラッカーをリセット
        self.current_heap_vars.clear();

        // FL-16: パラメータの型注釈から型トラッキング変数を登録
        for param in &func_def.params {
            if let Some(type_ann) = &param.type_annotation {
                match type_ann {
                    crate::parser::TypeExpr::Named(name) if name == "Int" || name == "Num" => {
                        self.int_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Str" => {
                        self.string_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Float" => {
                        self.float_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Bool" => {
                        self.bool_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::List(inner) => {
                        self.list_vars.insert(param.name.clone());
                        // C21-4: Remember element type for `@[Float]` unmold tracking
                        if let crate::parser::TypeExpr::Named(elem) = inner.as_ref() {
                            self.list_element_types
                                .insert(param.name.clone(), elem.clone());
                        }
                    }
                    crate::parser::TypeExpr::BuchiPack(_) => {
                        self.pack_vars.insert(param.name.clone());
                    }
                    _ => {}
                }
            }
        }

        for stmt in &func_def.body {
            if let Statement::Assignment(assign) = stmt
                && self.expr_type_tag(&assign.value) == crate::codegen::tag_prop::TAG_BOOL
            {
                self.bool_vars.insert(assign.target.clone());
            }
        }

        // NB3-4 fix: Save/restore return_type_inferred_params across function boundaries
        // so that inner function parameters don't inherit outer function's inference.
        // Must be saved BEFORE return-type inference so that current function's inferred
        // params are tracked in the fresh (empty) set.
        let prev_return_type_inferred_params =
            std::mem::take(&mut self.return_type_inferred_params);

        // 戻り値型注釈から型注釈なしパラメータの型を推論登録
        // 例: `sumTo n acc = ... => :Int` の場合、n, acc を int_vars に登録
        // これにより poly_add 等のヒューリスティック関数の誤発火を防ぐ
        if let Some(ref rt) = func_def.return_type {
            let inferred_numeric = matches!(
                rt,
                crate::parser::TypeExpr::Named(name) if name == "Int" || name == "Num"
            );
            if inferred_numeric {
                for param in &func_def.params {
                    if param.type_annotation.is_none()
                        && !self.string_vars.contains(&param.name)
                        && !self.float_vars.contains(&param.name)
                        && !self.bool_vars.contains(&param.name)
                        && !self.pack_vars.contains(&param.name)
                        && !self.list_vars.contains(&param.name)
                        && !self.closure_vars.contains(&param.name)
                    {
                        self.int_vars.insert(param.name.clone());
                        // NB3-4 fix: Track that this parameter's type was inferred
                        // from the return type, not from an explicit annotation.
                        // callable_type_tag must not trust this inference for
                        // handler arguments, since the parameter might be a function.
                        self.return_type_inferred_params.insert(param.name.clone());
                    }
                }
            }
        }

        // NB-14: Emit taida_get_call_arg_tag() for parameters whose type cannot be
        // determined at compile time. This reads the type tag that the caller set via
        // taida_set_call_arg_tag(), enabling Bool/Int disambiguation in pack field tags.
        let prev_param_tag_vars = std::mem::take(&mut self.param_tag_vars);
        // C25B-030 Phase 1E-β-3: snapshot and reset `return_tag_vars`
        // across function boundaries. `IrVar`s are allocated per
        // IR function, so an entry like `return_tag_vars[14] = 15`
        // recorded while lowering function A aliases with var 14
        // in function B's fresh `alloc_var()` counter, producing
        // a Cranelift verifier error (`set_return_tag` argument
        // references a value defined only inside a CondBranch arm,
        // not in the outer block). The lambda path in
        // `lower_lambda` already snapshots/restores this field;
        // `lower_func_def` was missing the same discipline which
        // became load-bearing once facade FuncDefs (Phase 1E-β)
        // started lowering dozens of sibling functions in one
        // `lower_program` invocation.
        let prev_return_tag_vars = std::mem::take(&mut self.return_tag_vars);
        for (i, param) in func_def.params.iter().enumerate() {
            // Only emit for parameters that don't have a compile-time type
            let has_known_type = self.bool_vars.contains(&param.name)
                || self.int_vars.contains(&param.name)
                || self.float_vars.contains(&param.name)
                || self.string_vars.contains(&param.name)
                || self.pack_vars.contains(&param.name)
                || self.list_vars.contains(&param.name)
                || self.closure_vars.contains(&param.name);
            if !has_known_type {
                let idx_var = ir_func.alloc_var();
                ir_func.push(IrInst::ConstInt(idx_var, i as i64));
                let tag_var = ir_func.alloc_var();
                ir_func.push(IrInst::Call(
                    tag_var,
                    "taida_get_call_arg_tag".to_string(),
                    vec![idx_var],
                ));
                self.param_tag_vars.insert(param.name.clone(), tag_var);
            }
        }

        // ローカル関数定義の前処理: 関数本体内の FuncDef を先に IR 化して登録する。
        // 内部関数が親スコープの変数を参照する場合はクロージャとして生成する。
        for stmt in &func_def.body {
            if let Statement::FuncDef(inner_func_def) = stmt {
                // 内部関数の自由変数を検出
                let inner_params: std::collections::HashSet<&str> = inner_func_def
                    .params
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                let inner_free_vars = self
                    .collect_free_vars_in_func_body_unfiltered(&inner_func_def.body, &inner_params);
                // 親スコープの変数のみをキャプチャ対象とする
                // （トップレベル変数は GlobalGet で解決されるので除外）
                let parent_scope_set: std::collections::HashSet<&str> =
                    parent_scope_vars.iter().map(|s| s.as_str()).collect();
                let captures: Vec<String> = inner_free_vars
                    .into_iter()
                    .filter(|v| parent_scope_set.contains(v.as_str()))
                    .collect();

                if captures.is_empty() {
                    // キャプチャなし: 通常のユーザー関数として登録
                    self.user_funcs.insert(inner_func_def.name.clone());
                    self.func_param_defs
                        .insert(inner_func_def.name.clone(), inner_func_def.params.clone());
                    let inner_ir = self.lower_func_def(inner_func_def)?;
                    self.lambda_funcs.push(inner_ir);
                } else {
                    // キャプチャあり: クロージャとして生成
                    let lambda_name = format!("_taida_lambda_{}", self.lambda_counter);
                    self.lambda_counter += 1;

                    // lambda_vars と closure_vars に登録
                    self.lambda_vars
                        .insert(inner_func_def.name.clone(), lambda_name.clone());
                    self.closure_vars.insert(inner_func_def.name.clone());

                    // __env + 元のパラメータ
                    let mut closure_params: Vec<String> = vec!["__env".to_string()];
                    closure_params.extend(inner_func_def.params.iter().map(|p| p.name.clone()));
                    let mut lambda_fn =
                        IrFunction::new_with_params(lambda_name.clone(), closure_params);

                    // 環境からキャプチャ変数を復元
                    let env_var = 0u32;
                    for (i, cap_name) in captures.iter().enumerate() {
                        let get_dst = lambda_fn.alloc_var();
                        lambda_fn.push(IrInst::PackGet(get_dst, env_var, i));
                        lambda_fn.push(IrInst::DefVar(cap_name.clone(), get_dst));
                    }

                    // 内部関数の前処理: クロージャ本体内のネストされた FuncDef を検出し処理する
                    // (deep nesting: f1 → f2(closure) → f3 → f4 → f5 のパターンに対応)
                    {
                        let scope_vars = self.collect_nested_scope_vars(
                            captures
                                .iter()
                                .cloned()
                                .chain(inner_func_def.params.iter().map(|p| p.name.clone())),
                            &inner_func_def.body,
                        );
                        self.preprocess_inner_funcdefs(&inner_func_def.body, &scope_vars)?;
                    }

                    // 関数本体を処理
                    let mut last_var = None;
                    for (i, inner_stmt) in inner_func_def.body.iter().enumerate() {
                        let is_last = i == inner_func_def.body.len() - 1;
                        match inner_stmt {
                            Statement::Expr(expr) => {
                                let var = self.lower_expr(&mut lambda_fn, expr)?;
                                if is_last {
                                    last_var = Some(var);
                                }
                            }
                            _ => {
                                self.lower_statement(&mut lambda_fn, inner_stmt)?;
                            }
                        }
                    }

                    if let Some(ret) = last_var {
                        lambda_fn.push(IrInst::Return(ret));
                    } else {
                        let zero = lambda_fn.alloc_var();
                        lambda_fn.push(IrInst::ConstInt(zero, 0));
                        lambda_fn.push(IrInst::Return(zero));
                    }

                    self.user_funcs.insert(lambda_name.clone());
                    self.lambda_funcs.push(lambda_fn);

                    // MakeClosure は本体処理時に発行する（下記 lower_statement で処理）
                    self.pending_local_closures
                        .insert(inner_func_def.name.clone(), (lambda_name, captures));
                }
            }
        }

        // グローバル変数復元: 関数本体で参照されるトップレベル変数/インポート値を GlobalGet で復元
        let global_refs = self.collect_free_vars_in_body(&func_def.body, &params);
        for var_name in &global_refs {
            self.globals_referenced.insert(var_name.clone());
            let hash = self.global_var_hash(var_name);
            let dst = ir_func.alloc_var();
            ir_func.push(IrInst::GlobalGet(dst, hash));
            ir_func.push(IrInst::DefVar(var_name.clone(), dst));
        }

        // TCO: 現在の関数名を設定
        let prev_func_name = self.current_func_name.take();
        self.current_func_name = Some(func_def.name.clone());

        // 関数本体（ErrorCeiling を含む場合は lower_statement_sequence で処理）
        let mut last_var = None;
        let mut last_expr: Option<&Expr> = None;
        let body_refs: Vec<&Statement> = func_def.body.iter().collect();
        let has_error_ceiling = body_refs
            .iter()
            .any(|s| matches!(s, Statement::ErrorCeiling(_)));

        if has_error_ceiling {
            // ErrorCeiling があるので lower_statement_sequence を使う
            self.lower_statement_sequence(&mut ir_func, &body_refs)?;
            // ErrorCeiling 使用時は暗黙の戻り値なし（handler が return 相当）
        } else {
            for (i, stmt) in func_def.body.iter().enumerate() {
                let is_last = i == func_def.body.len() - 1;
                match stmt {
                    Statement::Expr(expr) => {
                        // 最後の式は末尾位置 — TCO対象
                        let var = if is_last {
                            self.lower_expr_tail(&mut ir_func, expr)?
                        } else {
                            self.lower_expr(&mut ir_func, expr)?
                        };
                        if is_last {
                            last_var = Some(var);
                            last_expr = Some(expr);
                        }
                    }
                    _ => {
                        self.lower_statement(&mut ir_func, stmt)?;
                        // C13-1: A tail binding statement yields the bound
                        // value as the function's return value.
                        if is_last && let Some(bound_var) = Self::tail_binding_var(&ir_func, stmt) {
                            last_var = Some(bound_var);
                            // Use the RHS expression of the binding for the
                            // heap-escape reachability analysis below. For an
                            // unmold binding, the source is the `Mold[_]`
                            // holder — reachability from it still covers the
                            // bound value.
                            last_expr = stmt.yielded_expr();
                        }
                    }
                }
            }
        }

        // TCO: 関数名を復元
        self.current_func_name = prev_func_name;

        // F-48: 戻り値式から推移的に到達可能な変数を計算し、
        // それらのヒープ変数は Release しない（dangling pointer 防止）
        let reachable_from_return = if let Some(ret_expr) = last_expr {
            Self::compute_reachable_vars(ret_expr, &func_def.body)
        } else {
            std::collections::HashSet::new()
        };

        // 関数終了時: ヒープ変数を Release（戻り値から到達可能な変数は除外）
        let heap_vars = std::mem::take(&mut self.current_heap_vars);
        for name in &heap_vars {
            if reachable_from_return.contains(name) {
                continue; // 戻り値から参照される可能性あり — 所有権はcallerに移転
            }
            let use_var = ir_func.alloc_var();
            ir_func.push(IrInst::UseVar(use_var, name.clone()));
            ir_func.push(IrInst::Release(use_var));
        }

        // 暗黙の戻り値
        if let Some(ret) = last_var {
            // NB-14: Set return type tag so callers can propagate it.
            // This enables type info to survive through generic functions like `id x = x`
            // and transitive chains like `g x = f(x)`.
            if let Some(&rtv) = self.return_tag_vars.get(&ret) {
                // Return value came from a CallUser — propagate that call's return tag
                let dummy = ir_func.alloc_var();
                ir_func.push(IrInst::Call(
                    dummy,
                    "taida_set_return_tag".to_string(),
                    vec![rtv],
                ));
            } else if let Some(ret_expr) = last_expr {
                let tag = self.expr_type_tag(ret_expr);
                if tag > 0 {
                    let tag_var = ir_func.alloc_var();
                    ir_func.push(IrInst::ConstInt(tag_var, tag));
                    let dummy = ir_func.alloc_var();
                    ir_func.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![tag_var],
                    ));
                } else if tag == -1
                    && let Some(ptv) = self.get_param_tag_var(ret_expr)
                {
                    let dummy = ir_func.alloc_var();
                    ir_func.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![ptv],
                    ));
                }
            }
            ir_func.push(IrInst::Return(ret));
        } else {
            let zero = ir_func.alloc_var();
            ir_func.push(IrInst::ConstInt(zero, 0));
            ir_func.push(IrInst::Return(zero));
        }

        // Restore net builtin shadow set to pre-function state
        self.shadowed_net_builtins = prev_shadowed_net;
        // NB-14: Restore param_tag_vars to pre-function state
        self.param_tag_vars = prev_param_tag_vars;
        // C25B-030 Phase 1E-β-3: restore return_tag_vars (see snapshot
        // comment above — cross-function IrVar aliasing breaks
        // Cranelift verification once facade FuncDefs stream through
        // the same `Lowering` instance).
        self.return_tag_vars = prev_return_tag_vars;
        // NB3-4 fix: Restore return_type_inferred_params to pre-function state
        self.return_type_inferred_params = prev_return_type_inferred_params;
        // NB3-4: Restore var_aliases, lambda_param_counts, lambda_vars, closure_vars
        // to pre-function state (parameter shadow cleanup)
        self.var_aliases = prev_var_aliases;
        self.lambda_param_counts = prev_lambda_param_counts;
        self.lambda_vars = prev_lambda_vars;
        self.closure_vars = prev_closure_vars;
        // Value-tag track: restore the caller scope's shadow kinds.
        self.shadow_kind_vars = prev_shadow_kinds;

        // F58 P2-2: wrap provably escape-free tail-recursive loops in an
        // iteration-scope arena watermark (enter at entry, reset right
        // before each loop back-edge, exit before return).
        Self::maybe_apply_append_consume(&mut ir_func, func_def);
        Self::maybe_insert_iter_scope(&mut ir_func, func_def);

        Ok(ir_func)
    }

    /// Runtime calls that neither retain their arguments nor
    /// register pointers anywhere — safe to appear inside an
    /// iteration-scope watermark. Anything outside this list (or any
    /// CallUser / CallIndirect / MakeClosure / FuncAddr / GlobalSet)
    /// disqualifies the loop, keeping the analysis fail-closed: an
    /// unknown call might stash an arena pointer (async spawn keeps its
    /// argument for a worker thread; the enum-descriptor registry keeps
    /// the pack pointer itself), which a scope reset would dangle.
    const ITER_SCOPE_CALL_WHITELIST: &'static [&'static str] = &[
        // scalar arithmetic / comparison / logic
        "taida_int_add",
        "taida_int_sub",
        "taida_int_mul",
        "taida_int_neg",
        "taida_int_eq",
        "taida_int_neq",
        "taida_int_lt",
        "taida_int_gt",
        "taida_int_gte",
        "taida_int_lte",
        "taida_int_to_float",
        "taida_float_add",
        "taida_float_sub",
        "taida_float_mul",
        "taida_float_neg",
        "taida_float_eq",
        "taida_float_neq",
        "taida_float_lt",
        "taida_float_gt",
        "taida_float_gte",
        "taida_float_lte",
        "taida_bool_and",
        "taida_bool_or",
        "taida_bool_not",
        "taida_poly_add",
        "taida_poly_eq",
        "taida_poly_neq",
        "taida_poly_eq_tagged",
        "taida_poly_neq_tagged",
        // mold construction / unmold (build into the arena, retain nothing)
        "taida_lax_new",
        "taida_lax_empty",
        "taida_lax_tag_value_default",
        "taida_lax_value_ekind",
        "taida_generic_unmold",
        "taida_lax_unmold",
        "taida_gorillax_unmold",
        "taida_relaxed_gorillax_unmold",
        "taida_div_mold",
        "taida_mod_mold",
        "taida_div_mold_f",
        "taida_mod_mold_f",
        // pack plumbing (writes into the pack being built / reads fields);
        // taida_register_field_name only records the static field-name
        // string for display, never the pack pointer.
        "taida_pack_set_hash",
        "taida_pack_set_tag",
        "taida_pack_get",
        "taida_pack_get_idx",
        "taida_pack_get_field_tag",
        "taida_pack_has_hash",
        // field-name / field-type display registries record the hash, a
        // static literal name and an integer tag — never the pack pointer
        // (unlike taida_register_pack_field_enum, which stays excluded).
        "taida_register_field_name",
        "taida_register_field_type",
        // collection construction local to the iteration.
        // taida_list_push relies on a NON-LOCAL invariant: pushing an
        // arena pointer into a list that outlives the scope would dangle
        // on reset, but every list visible inside an iter-scope body is
        // necessarily built inside that same iteration — the all-scalar
        // parameter gate means no list can enter through the back-edge,
        // and GlobalSet / MakeClosure / CallUser exclusion means none
        // can enter from outside the loop body.
        "taida_list_new",
        "taida_list_push",
        "taida_list_note_push_ekind",
        "taida_list_set_elem_tag",
        "taida_list_get",
        "taida_set_from_list",
        "taida_collection_size",
        // immediate output (fprintf-and-return, no retention)
        "taida_io_stdout_with_tag",
        "taida_io_stderr_with_tag",
        "taida_stdout_display_string",
        // call-site tag stack (push/pop pairs of plain integers)
        "taida_push_call_tags",
        "taida_pop_call_tags",
        "taida_set_call_arg_tag",
        "taida_get_call_arg_tag",
        "taida_set_return_tag",
        "taida_get_return_tag",
        // reference counting: adjusts the header counter / recycles dead
        // objects; never registers the pointer anywhere. The freelist
        // push inside release is iteration-scope aware (depth gate).
        "taida_retain",
        "taida_release",
        "taida_str_retain",
        "taida_str_release",
        "taida_list_elem_retain",
        "taida_list_elem_release",
    ];

    /// The subset of loop-visible operations that can actually
    /// allocate from the arena. A loop whose body never allocates gains
    /// nothing from the watermark and would pay one reset call per
    /// iteration (a 300M-iteration scalar loop measured ~14x slower), so
    /// the scope is only inserted when at least one of these appears.
    const ITER_SCOPE_ALLOCATING_CALLS: &'static [&'static str] = &[
        "taida_lax_new",
        "taida_lax_empty",
        "taida_div_mold",
        "taida_mod_mold",
        "taida_div_mold_f",
        "taida_mod_mold_f",
        "taida_list_new",
        "taida_list_push",
        "taida_set_from_list",
        "taida_stdout_display_string",
        "taida_poly_add",
    ];

    fn iter_scope_insts_allocate(insts: &[IrInst]) -> bool {
        insts.iter().any(|inst| match inst {
            IrInst::PackNew(_, _) => true,
            IrInst::Call(_, name, _) => Self::ITER_SCOPE_ALLOCATING_CALLS.contains(&name.as_str()),
            IrInst::CondBranch(_, arms) => arms
                .iter()
                .any(|arm| Self::iter_scope_insts_allocate(&arm.body)),
            _ => false,
        })
    }

    fn ir_insts_contain_tail_call(insts: &[IrInst]) -> bool {
        insts.iter().any(|inst| match inst {
            IrInst::TailCall(_) => true,
            IrInst::CondBranch(_, arms) => arms
                .iter()
                .any(|arm| Self::ir_insts_contain_tail_call(&arm.body)),
            _ => false,
        })
    }

    /// Debug helper for TAIDA_DEBUG_ITER_SCOPE: name the first
    /// instruction that disqualified the loop.
    fn iter_scope_first_unsafe(insts: &[IrInst]) -> Option<String> {
        for inst in insts {
            match inst {
                IrInst::GlobalSet(_, _) => return Some("GlobalSet".to_string()),
                IrInst::MakeClosure(_, _, _) => return Some("MakeClosure".to_string()),
                IrInst::CallIndirect(_, _, _) => return Some("CallIndirect".to_string()),
                IrInst::CallUser(_, name, _) => return Some(format!("CallUser {name}")),
                IrInst::FuncAddr(_, _) => return Some("FuncAddr".to_string()),
                IrInst::Call(_, name, _)
                    if !Self::ITER_SCOPE_CALL_WHITELIST.contains(&name.as_str()) =>
                {
                    return Some(format!("Call {name}"));
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        if let Some(r) = Self::iter_scope_first_unsafe(&arm.body) {
                            return Some(r);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn iter_scope_insts_safe(insts: &[IrInst]) -> bool {
        insts.iter().all(|inst| match inst {
            IrInst::GlobalSet(_, _)
            | IrInst::MakeClosure(_, _, _)
            | IrInst::CallIndirect(_, _, _)
            | IrInst::CallUser(_, _, _)
            | IrInst::FuncAddr(_, _) => false,
            IrInst::Call(_, name, _) => Self::ITER_SCOPE_CALL_WHITELIST.contains(&name.as_str()),
            IrInst::CondBranch(_, arms) => arms
                .iter()
                .all(|arm| Self::iter_scope_insts_safe(&arm.body)),
            // CAUTION: this catch-all treats every other instruction as
            // safe, which is correct for the current IrInst set (loads,
            // stores, consts, locals, branches, Return/TailCall). If a
            // new IrInst variant can publish a pointer beyond the
            // iteration (registry write, global cache, thread handoff),
            // it MUST be added to the deny arms above — the watermark
            // reset would otherwise dangle it.
            _ => true,
        })
    }

    /// A top-level binding that some function body references must
    /// also land in the global table: function bodies restore their
    /// free variables through GlobalGet, so a binding that only does
    /// DefVar in `_taida_main` leaves the global slot at 0 and the
    /// function silently reads the wrong value. The Assignment arm has
    /// always done this; every other binding form (the `>=>` / `<=<`
    /// unmolds) must apply the same rule.
    fn maybe_globalize_toplevel_binding(&mut self, func: &mut IrFunction, name: &str, val: IrVar) {
        if self.current_func_name.is_none() && self.globals_referenced.contains(name) {
            let hash = self.global_var_hash(name);
            func.push(IrInst::GlobalSet(hash, val));
        }
    }

    /// Sequential-Append tail recursion (`f(n-1, Append[acc, x]())`)
    /// copies the whole accumulator list per element — O(n^2), ~1.2GB
    /// of traffic for a 10k build. When this pass can prove the
    /// accumulator's ONLY use in the recursive arm is the Append's
    /// first argument, it rewrites the call to the consume variant and
    /// threads an ownership bit: 0 on entry (the list belongs to the
    /// caller and may be aliased — the runtime detaches via the copy
    /// path), set to 1 only by the tail-calls that actually consumed
    /// (a pass-through tail-call keeps it unchanged, so a list that
    /// arrived from outside can never be flagged owned). Everything
    /// here is fail-closed: any condition miss keeps the copy variant.
    fn maybe_apply_append_consume(ir_func: &mut IrFunction, func_def: &FuncDef) {
        macro_rules! actrace {
            ($($t:tt)*) => {
                if std::env::var("TAIDA_DEBUG_APPEND_CONSUME").is_ok() {
                    eprintln!("append-consume [{}]: {}", ir_func.name, format!($($t)*));
                }
            };
        }
        if func_def.params.is_empty() {
            return;
        }
        // AST-side guards: default completion / ErrorCeiling / Lambda /
        // unmodeled statement forms — all fail closed.
        if Self::funcdef_blocks_append_consume(func_def) {
            actrace!("AST guard blocked");
            return;
        }
        // Body shape: optional prelude (condition evaluation), a single
        // CondBranch, then the trailing Return of its result — the
        // buildN shape.
        let body_snapshot = ir_func.body.clone();
        let n_body = body_snapshot.len();
        if n_body < 2 {
            return;
        }
        let IrInst::Return(_) = &body_snapshot[n_body - 1] else {
            actrace!("body does not end in Return");
            return;
        };
        let IrInst::CondBranch(_, arms) = &body_snapshot[n_body - 2] else {
            actrace!("no CondBranch before the Return");
            return;
        };
        let prelude = &body_snapshot[..n_body - 2];

        let param_names: Vec<&str> = func_def.params.iter().map(|p| p.name.as_str()).collect();

        // Pick the candidate param: appears in NO prelude instruction
        // (conditions are evaluated eagerly every iteration), and in
        // every tail-calling arm appears exactly once — as the first
        // argument of a trailing `taida_list_append` whose result
        // feeds that arm's TailCall at a consistent position.
        let mut candidate: Option<(usize, &str)> = None;
        'params: for (idx, pname) in param_names.iter().enumerate() {
            if Self::insts_use_var(prelude, pname) {
                actrace!("param {} used in prelude", pname);
                continue;
            }
            let mut saw_consume_arm = false;
            for arm in arms {
                let has_tail = Self::ir_insts_contain_tail_call(&arm.body);
                if !has_tail {
                    continue; // non-recursive arms may use p freely
                }
                match Self::arm_append_consume_shape(&arm.body, pname, idx, &param_names) {
                    Some(true) => saw_consume_arm = true,
                    // A pass-through arm hands the CALLER's list to the
                    // next iteration unchanged, but the emitters set the
                    // ownership bit after EVERY self tail-call (the bit
                    // is loop machinery, not per-arm) — a subsequent
                    // consume would then mutate a list the caller still
                    // owns. Fail closed on the whole param. (Today's
                    // lowering also rejects this shape incidentally via
                    // the param write-back plumbing; this arm makes the
                    // invariant explicit rather than emergent.)
                    Some(false) | None => {
                        actrace!("param {} rejected by arm shape", pname);
                        continue 'params;
                    }
                }
            }
            if saw_consume_arm {
                candidate = Some((idx, pname));
                break;
            }
        }
        let Some((p_idx, p_name)) = candidate else {
            actrace!("no candidate param");
            return;
        };
        actrace!("APPLYING consume for param {}", p_name);
        let param_names_again: Vec<&str> =
            func_def.params.iter().map(|p| p.name.as_str()).collect();

        // Rewrite: swap the append call for the consume variant in
        // every consume-shaped arm. The ownership bit itself is loop
        // machinery and is wired by the emitters (0 on entry, 1 after
        // every self tail-call): a named IR variable cannot carry a
        // loop-mutable value on native, where DefVar/UseVar resolve
        // statically at emission time.
        let ret_inst = ir_func.body[n_body - 1].clone();
        let IrInst::CondBranch(result, arms) = ir_func.body[n_body - 2].clone() else {
            unreachable!("checked above");
        };
        let mut new_body: Vec<IrInst> = prelude.to_vec();
        let mut new_arms = Vec::with_capacity(arms.len());
        for arm in arms {
            let has_tail = Self::ir_insts_contain_tail_call(&arm.body);
            if !has_tail {
                new_arms.push(arm);
                continue;
            }
            let consumes = matches!(
                Self::arm_append_consume_shape(&arm.body, p_name, p_idx, &param_names_again),
                Some(true)
            );
            if !consumes {
                new_arms.push(arm);
                continue;
            }
            let mut body = arm.body.clone();
            let tail_pos = body
                .iter()
                .rposition(|i| matches!(i, IrInst::TailCall(_)))
                .expect("shape-checked: TailCall present");
            let append_pos = body[..tail_pos]
                .iter()
                .rposition(
                    |i| matches!(i, IrInst::Call(_, name, _) if name == "taida_list_append_k"),
                )
                .expect("shape-checked: append present");
            let IrInst::Call(dst, _, args) = body[append_pos].clone() else {
                unreachable!("shape-checked");
            };
            body[append_pos] = IrInst::Call(dst, "taida_list_append_consume_k".to_string(), args);
            new_arms.push(crate::codegen::ir::CondArm {
                condition: arm.condition,
                body,
                result: arm.result,
            });
        }
        new_body.push(IrInst::CondBranch(result, new_arms));
        new_body.push(ret_inst);
        ir_func.body = new_body;
        ir_func.append_consume_owned = true;
    }

    /// AST guard for the consume rewrite: fail closed when any self
    /// call omits arguments (default completion evaluates AFTER the
    /// explicit args and may read the consumed list), when an
    /// ErrorCeiling / Lambda opens a capture escape route, or when a
    /// statement form we do not model appears.
    fn funcdef_blocks_append_consume(func_def: &FuncDef) -> bool {
        // Conservative expression scan: walks the variants that can
        // syntactically appear inside a buildN-shaped body. Any
        // variant outside this list fails closed.
        fn scan(e: &Expr, fname: &str, arity: usize, blocked: &mut bool) {
            if *blocked {
                return;
            }
            match e {
                Expr::Lambda(_, _, _) => *blocked = true,
                Expr::FuncCall(callee, args, _) => {
                    if let Expr::Ident(n, _) = callee.as_ref()
                        && n == fname
                        && args.len() < arity
                    {
                        *blocked = true;
                    }
                    scan(callee, fname, arity, blocked);
                    for a in args {
                        scan(a, fname, arity, blocked);
                    }
                }
                Expr::BinaryOp(l, _, r, _) => {
                    scan(l, fname, arity, blocked);
                    scan(r, fname, arity, blocked);
                }
                Expr::UnaryOp(_, x, _) => scan(x, fname, arity, blocked),
                Expr::MoldInst(_, targs, named, _) => {
                    for t in targs {
                        scan(t, fname, arity, blocked);
                    }
                    for field in named {
                        scan(&field.value, fname, arity, blocked);
                    }
                }
                Expr::Ident(_, _)
                | Expr::IntLit(_, _)
                | Expr::FloatLit(_, _)
                | Expr::BoolLit(_, _)
                | Expr::StringLit(_, _) => {}
                Expr::CondBranch(arms, _) => {
                    for arm in arms {
                        if let Some(c) = &arm.condition {
                            scan(c, fname, arity, blocked);
                        }
                        for st in &arm.body {
                            scan_stmt(st, fname, arity, blocked);
                        }
                    }
                }
                _ => *blocked = true, // unmodeled expression: fail closed
            }
        }
        fn scan_stmt(st: &Statement, fname: &str, arity: usize, blocked: &mut bool) {
            if *blocked {
                return;
            }
            match st {
                Statement::ErrorCeiling(_) => *blocked = true,
                Statement::Expr(e) => scan(e, fname, arity, blocked),
                Statement::Assignment(a) => scan(&a.value, fname, arity, blocked),
                Statement::UnmoldForward(u) => scan(&u.source, fname, arity, blocked),
                Statement::UnmoldBackward(u) => scan(&u.source, fname, arity, blocked),
                _ => *blocked = true,
            }
        }
        let mut blocked = false;
        for st in &func_def.body {
            scan_stmt(st, &func_def.name, func_def.params.len(), &mut blocked);
        }
        blocked
    }

    /// Whether `insts` (recursively) contain `UseVar(_, name)`.
    fn insts_use_var(insts: &[IrInst], name: &str) -> bool {
        insts.iter().any(|inst| match inst {
            IrInst::UseVar(_, n) => n == name,
            IrInst::CondBranch(_, arms) => arms.iter().any(|a| Self::insts_use_var(&a.body, name)),
            _ => false,
        })
    }

    /// Classify a tail-calling arm for the consume rewrite.
    /// Returns Some(true) = consume shape (trailing `append(p, item)`
    /// feeding the TailCall at param position `p_idx`, single use of
    /// `p`, safe instruction set), Some(false) = pass-through shape
    /// (`p` travels unchanged at its own position, no other use),
    /// None = anything else (fail closed).
    fn arm_append_consume_shape(
        body: &[IrInst],
        p_name: &str,
        p_idx: usize,
        func_params: &[&str],
    ) -> Option<bool> {
        macro_rules! shtrace {
            ($($t:tt)*) => {
                if std::env::var("TAIDA_DEBUG_APPEND_CONSUME").is_ok() {
                    eprintln!("  shape({}): {}", p_name, format!($($t)*));
                }
            };
        }
        // No nested branches inside recursive arms in v1.
        if body.iter().any(|i| matches!(i, IrInst::CondBranch(_, _))) {
            shtrace!("nested branch");
            return None;
        }
        // The TailCall is followed by dead result plumbing (a ConstInt
        // feeding the arm's never-reached result) — locate it instead
        // of requiring it to be last, and only allow trivially dead
        // instructions after it.
        let tail_pos = body
            .iter()
            .rposition(|i| matches!(i, IrInst::TailCall(_)))
            .or_else(|| {
                shtrace!("no TailCall in arm");
                None
            })?;
        for inst in &body[tail_pos + 1..] {
            match inst {
                IrInst::ConstInt(_, _) | IrInst::UseVar(_, _) | IrInst::DefVar(_, _) => {}
                _ => {
                    shtrace!("live instruction after TailCall");
                    return None;
                }
            }
        }
        let n = tail_pos + 1; // analyse only up to and including the TailCall
        let IrInst::TailCall(targs) = &body[n - 1] else {
            unreachable!("located above");
        };
        // Instruction safety: only scalar-pure calls besides the append.
        for inst in &body[..n - 1] {
            match inst {
                IrInst::Call(_, name, _) => {
                    let ok = name == "taida_list_append_k"
                        || name.starts_with("taida_int_")
                        || name.starts_with("taida_float_")
                        || name.starts_with("taida_bool_");
                    if !ok {
                        shtrace!("unsafe call {}", name);
                        return None;
                    }
                }
                IrInst::ConstInt(_, _)
                | IrInst::ConstFloat(_, _)
                | IrInst::ConstBool(_, _)
                | IrInst::UseVar(_, _)
                | IrInst::DefVar(_, _) => {}
                other => {
                    let what = match other {
                        IrInst::CallUser(_, n, _) => format!("CallUser {}", n),
                        IrInst::Retain(_) => "Retain".to_string(),
                        IrInst::Release(_) => "Release".to_string(),
                        IrInst::ReleaseAuto(_) => "ReleaseAuto".to_string(),
                        IrInst::TailCall(_) => "TailCall".to_string(),
                        _ => format!("{:?}", std::mem::discriminant(other)),
                    };
                    shtrace!("unsafe inst {}", what);
                    return None;
                }
            }
        }
        // Collect every read of p, then discount the TCO machinery's
        // save/restore noise: the lowering snapshots params around a
        // tail call as `UseVar(v, p)` whose value is consumed ONLY by
        // a `DefVar(p, v)` write-back — observationally a no-op, so
        // such reads do not count as uses.
        let mut substantive_reads: Vec<(usize, IrVar)> = Vec::new();
        for (i, inst) in body.iter().enumerate() {
            let IrInst::UseVar(v, n) = inst else {
                continue;
            };
            if n != p_name {
                continue;
            }
            let mut consumers = 0usize;
            let mut writeback_only = true;
            for other in body.iter() {
                match other {
                    IrInst::DefVar(dn, src) if *src == *v => {
                        consumers += 1;
                        if dn != p_name {
                            writeback_only = false;
                        }
                    }
                    _ if Self::inst_reads_var(other, *v)
                        && !matches!(other, IrInst::DefVar(_, _)) =>
                    {
                        consumers += 1;
                        writeback_only = false;
                    }
                    _ => {}
                }
            }
            if consumers > 0 && writeback_only {
                continue; // pure save/restore noise
            }
            substantive_reads.push((i, *v));
        }
        if substantive_reads.len() != 1 {
            shtrace!("substantive reads = {}", substantive_reads.len());
            return None;
        }
        let (p_read_pos, p_var) = substantive_reads[0];
        // Pass-through shape: p's value travels directly at its slot.
        if targs.get(p_idx) == Some(&p_var) {
            // ... and is used nowhere else.
            let other_use = body.iter().enumerate().any(|(i, inst)| {
                i != n - 1 && i != p_read_pos && Self::inst_reads_var(inst, p_var)
            }) || targs
                .iter()
                .enumerate()
                .any(|(ai, v)| ai != p_idx && *v == p_var);
            if other_use {
                return None;
            }
            return Some(false);
        }
        // Consume shape: an append whose only successors before the
        // tail call are the TCO save/restore machinery (UseVar /
        // DefVar-to-params / consts) and the scalar-pure evaluation of
        // LATER tail-call arguments (`f(Append[p, x](), n + 1)` puts
        // the increment after the append). Those later calls are safe
        // exactly when they touch neither p (the substantive-read
        // check rejects that) nor the consumed result.
        let append_pos = body[..n - 1].iter().rposition(
            |i| matches!(i, IrInst::Call(_, name, _) if name == "taida_list_append_k"),
        )?;
        let IrInst::Call(append_dst, append_name, append_args) = &body[append_pos] else {
            return None;
        };
        for inst in &body[append_pos + 1..n - 1] {
            match inst {
                IrInst::ConstInt(_, _)
                | IrInst::ConstFloat(_, _)
                | IrInst::ConstBool(_, _)
                | IrInst::UseVar(_, _) => {}
                // Between the append and the tail call only the TCO
                // save/restore machinery runs: writes into the
                // function's own param slots (including the new list
                // into p — that IS the back-edge hand-off, and the
                // momentary old-value restore is overwritten by the
                // emitter's _tco_arg assignment). A write to any NEW
                // name would be a real alias escaping the iteration —
                // fail closed on those.
                IrInst::DefVar(dn, _) => {
                    if !func_params.iter().any(|pp| pp == dn) {
                        shtrace!("non-param DefVar {} between append and tail", dn);
                        return None;
                    }
                }
                IrInst::Call(_, name, args) => {
                    let pure = name.starts_with("taida_int_")
                        || name.starts_with("taida_float_")
                        || name.starts_with("taida_bool_");
                    if !pure || args.contains(append_dst) {
                        shtrace!("unsafe call {} between append and tail", name);
                        return None;
                    }
                }
                _ => {
                    shtrace!("unsafe inst between append and tail");
                    return None;
                }
            }
        }
        if append_name != "taida_list_append_k" || append_args.len() != 3 {
            shtrace!("trailing call is not a 3-arg kind-supplying append");
            return None;
        }
        if append_args[0] != p_var {
            shtrace!("append arg0 is not p");
            return None;
        }
        // p feeds ONLY the append's first slot.
        if append_args[1] == p_var {
            return None;
        }
        let other_use = body.iter().enumerate().any(|(i, inst)| {
            i != append_pos && i != p_read_pos && Self::inst_reads_var(inst, p_var)
        }) || targs.contains(&p_var);
        if other_use {
            shtrace!("p escapes beyond the append");
            return None;
        }
        // The append result feeds the tail call at p's position only.
        if targs.get(p_idx) != Some(append_dst) {
            shtrace!("append result does not feed the tail call at p's slot");
            return None;
        }
        if targs
            .iter()
            .enumerate()
            .any(|(ai, v)| ai != p_idx && v == append_dst)
        {
            return None;
        }
        Some(true)
    }

    /// Whether an instruction reads the given IR var.
    fn inst_reads_var(inst: &IrInst, var: IrVar) -> bool {
        match inst {
            IrInst::Call(_, _, args) | IrInst::CallUser(_, _, args) => args.contains(&var),
            IrInst::TailCall(args) => args.contains(&var),
            IrInst::DefVar(_, src) => *src == var,
            IrInst::Return(v) => *v == var,
            _ => false,
        }
    }

    fn maybe_insert_iter_scope(ir_func: &mut IrFunction, func_def: &FuncDef) {
        if !Self::ir_insts_contain_tail_call(&ir_func.body) {
            return;
        }
        // Every parameter must be a scalar-annotated value: the TailCall
        // arguments become the next iteration's parameters, so scalar
        // params guarantee nothing allocated inside the scope survives
        // the back-edge reset.
        let all_scalar = !func_def.params.is_empty()
            && func_def.params.iter().all(|p| {
                matches!(
                    &p.type_annotation,
                    Some(crate::parser::TypeExpr::Named(n))
                        if n == "Int" || n == "Num" || n == "Float" || n == "Bool"
                )
            });
        if !all_scalar {
            return;
        }
        if !Self::iter_scope_insts_safe(&ir_func.body) {
            if std::env::var("TAIDA_DEBUG_ITER_SCOPE").is_ok() {
                eprintln!(
                    "iter-scope reject ({}): {}",
                    ir_func.name,
                    Self::iter_scope_first_unsafe(&ir_func.body)
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
            return;
        }
        if !Self::iter_scope_insts_allocate(&ir_func.body) {
            if std::env::var("TAIDA_DEBUG_ITER_SCOPE").is_ok() {
                eprintln!("iter-scope skip ({}): allocation-free body", ir_func.name);
            }
            return;
        }

        let mark = ir_func.alloc_var();
        let mut counter_seed = mark;
        Self::insert_iter_scope_hooks(&mut ir_func.body, mark, &mut counter_seed);
        ir_func.body.insert(
            0,
            IrInst::Call(mark, "taida_arena_iter_enter".to_string(), vec![]),
        );
        // insert_iter_scope_hooks allocated dummy vars past `mark`;
        // reflect them in the function's counter so any later pass never
        // collides with them.
        if counter_seed >= ir_func.next_var {
            ir_func.next_var = counter_seed + 1;
        }
    }

    fn insert_iter_scope_hooks(insts: &mut Vec<IrInst>, mark: IrVar, next_var: &mut IrVar) {
        let mut i = 0;
        while i < insts.len() {
            match &mut insts[i] {
                IrInst::TailCall(_) => {
                    *next_var += 1;
                    let dummy = *next_var;
                    insts.insert(
                        i,
                        IrInst::Call(dummy, "taida_arena_iter_reset".to_string(), vec![mark]),
                    );
                    i += 2;
                    continue;
                }
                IrInst::Return(_) => {
                    *next_var += 1;
                    let dummy = *next_var;
                    insts.insert(
                        i,
                        IrInst::Call(dummy, "taida_arena_iter_exit".to_string(), vec![mark]),
                    );
                    i += 2;
                    continue;
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms.iter_mut() {
                        Self::insert_iter_scope_hooks(&mut arm.body, mark, next_var);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// 文列を処理。ErrorCeiling が出現したら後続文をすべて通常パスに包む。
    pub(super) fn lower_statement_sequence(
        &mut self,
        func: &mut IrFunction,
        stmts: &[&Statement],
    ) -> Result<(), LowerError> {
        let mut i = 0;
        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = stmts[i] {
                // ErrorCeiling: 後続の全文を「通常パス」に入れる
                let remaining: Vec<&Statement> = stmts[i + 1..].to_vec();
                self.lower_error_ceiling_with_body(func, ec, &remaining)?;
                return Ok(()); // 残りの文は lower_error_ceiling_with_body 内で処理済み
            } else {
                self.lower_statement(func, stmts[i])?;
            }
            i += 1;
        }
        Ok(())
    }

    /// Collect variable names defined in IR instructions (DefVar names).
    pub(super) fn collect_defvar_names(insts: &[IrInst]) -> Vec<String> {
        let mut names = Vec::new();
        for inst in insts {
            if let IrInst::DefVar(name, _) = inst
                && !names.contains(name)
            {
                names.push(name.clone());
            }
            // Also recurse into CondBranch arms
            if let IrInst::CondBranch(_, arms) = inst {
                for arm in arms {
                    for inner_name in Self::collect_defvar_names(&arm.body) {
                        if !names.contains(&inner_name) {
                            names.push(inner_name);
                        }
                    }
                }
            }
        }
        names
    }

    /// ErrorCeiling を後続文を含めて処理
    /// 後続文を別関数に抽出し、taida_error_try_call で setjmp 保護下で実行する
    pub(super) fn lower_error_ceiling_with_body(
        &mut self,
        func: &mut IrFunction,
        ec: &crate::parser::ErrorCeiling,
        subsequent_stmts: &[&Statement],
    ) -> Result<(), LowerError> {
        // 後続文を別関数に抽出（setjmp は呼び出し元の C 関数内で行う）
        let try_func_name = format!("_taida_try_{}", self.lambda_counter);
        self.lambda_counter += 1;

        // Collect variables from parent scope that _taida_try_N needs access to.
        // This includes function parameters and any DefVar'd variables before the ErrorCeiling.
        let mut captured_vars: Vec<String> = func.params.clone();
        for name in Self::collect_defvar_names(&func.body) {
            if !captured_vars.contains(&name) {
                captured_vars.push(name);
            }
        }

        // 後続文の関数を生成（1引数: env パック）
        let mut try_fn =
            IrFunction::new_with_params(try_func_name.clone(), vec!["__env".to_string()]);

        // Restore captured variables from env pack at the beginning of _taida_try_N
        for (i, var_name) in captured_vars.iter().enumerate() {
            let get_var = try_fn.alloc_var();
            try_fn.push(IrInst::PackGet(get_var, 0, i)); // param 0 = __env
            try_fn.push(IrInst::DefVar(var_name.clone(), get_var));
        }

        // Lower subsequent statements, tracking the last expression for return value.
        // C13-1: A tail binding statement yields the bound value as the
        // try-block's effective result.
        let mut last_try_var: Option<IrVar> = None;
        if !subsequent_stmts.is_empty() {
            // Lower all statements except possibly the last one
            let last_idx = subsequent_stmts.len() - 1;
            for (idx, stmt) in subsequent_stmts.iter().enumerate() {
                if idx == last_idx {
                    // Last statement: if it's an expression, capture its value
                    if let Statement::Expr(expr) = stmt {
                        last_try_var = Some(self.lower_expr(&mut try_fn, expr)?);
                    } else {
                        self.lower_statement(&mut try_fn, stmt)?;
                        if let Some(bound_var) = Self::tail_binding_var(&try_fn, stmt) {
                            last_try_var = Some(bound_var);
                        }
                    }
                } else if let Statement::ErrorCeiling(ec2) = stmt {
                    // Nested ErrorCeiling: delegate to lower_error_ceiling_with_body
                    let remaining: Vec<&Statement> = subsequent_stmts[idx + 1..].to_vec();
                    self.lower_error_ceiling_with_body(&mut try_fn, ec2, &remaining)?;
                    break;
                } else {
                    self.lower_statement(&mut try_fn, stmt)?;
                }
            }
        }
        // Return the last expression value, or 0 if none
        match last_try_var {
            Some(v) => {
                try_fn.push(IrInst::Return(v));
            }
            None => {
                let ret_var = try_fn.alloc_var();
                try_fn.push(IrInst::ConstInt(ret_var, 0));
                try_fn.push(IrInst::Return(ret_var));
            }
        }
        self.lambda_funcs.push(try_fn);
        // ユーザー関数として登録（emit で関数として扱われるように）
        self.user_funcs.insert(try_func_name.clone());

        // Build env pack with captured variables
        let env_pack = func.alloc_var();
        func.push(IrInst::PackNew(env_pack, captured_vars.len()));
        for (i, var_name) in captured_vars.iter().enumerate() {
            let use_var = func.alloc_var();
            func.push(IrInst::UseVar(use_var, var_name.clone()));
            let hash = simple_hash(var_name);
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            // Use Call to set hash + value (reuse existing PackSet infrastructure)
            func.push(IrInst::PackSet(env_pack, i, use_var));
        }

        // Push error ceiling
        let depth = func.alloc_var();
        func.push(IrInst::Call(
            depth,
            "taida_error_ceiling_push".to_string(),
            vec![],
        ));

        // 関数アドレスを取得
        let fn_addr = func.alloc_var();
        func.push(IrInst::FuncAddr(fn_addr, try_func_name));

        // taida_error_try_call(fn_ptr, env_ptr, depth) → 0 正常 / 1 エラー
        let try_result = func.alloc_var();
        func.push(IrInst::Call(
            try_result,
            "taida_error_try_call".to_string(),
            vec![fn_addr, env_pack, depth],
        ));

        // Handler arm (try_call returned 1 → error caught)
        let handler_insts = {
            let saved = std::mem::take(&mut func.body);
            // Pop error ceiling BEFORE handler body execution.
            // This is critical for re-throw: if the handler body throws again,
            // the depth must already be decremented so the throw goes to the
            // correct outer ceiling (not the now-invalid current one).
            let pop_var = func.alloc_var();
            func.push(IrInst::Call(
                pop_var,
                "taida_error_ceiling_pop".to_string(),
                vec![],
            ));
            let err_var = func.alloc_var();
            func.push(IrInst::Call(
                err_var,
                "taida_error_get_value".to_string(),
                vec![depth],
            ));
            // RCB-101: Type filter — re-throw if error type does not match handler type.
            // taida_error_type_check_or_rethrow(err_var, handler_type_str)
            // If type does not match, this calls taida_throw internally (longjmp/never returns).
            let handler_type_name = match &ec.error_type {
                crate::parser::TypeExpr::Named(name) => name.clone(),
                _ => "Error".to_string(),
            };
            let handler_type_str = func.alloc_var();
            func.push(IrInst::ConstStr(handler_type_str, handler_type_name));
            let checked_err = func.alloc_var();
            func.push(IrInst::Call(
                checked_err,
                "taida_error_type_check_or_rethrow".to_string(),
                vec![err_var, handler_type_str],
            ));
            func.push(IrInst::DefVar(ec.error_param.clone(), checked_err));
            // Lower handler body, capturing the last expression's result.
            // C13-1: If the last statement is a tail binding, yield the
            // bound value as the handler result.
            let mut last_handler_var = None;
            for (idx, stmt) in ec.handler_body.iter().enumerate() {
                let is_last = idx == ec.handler_body.len() - 1;
                if is_last {
                    if let Statement::Expr(expr) = stmt {
                        last_handler_var = Some(self.lower_expr(func, expr)?);
                    } else {
                        self.lower_statement(func, stmt)?;
                        if let Some(bound_var) = Self::tail_binding_var(func, stmt) {
                            last_handler_var = Some(bound_var);
                        }
                    }
                } else {
                    self.lower_statement(func, stmt)?;
                }
            }
            let handler_result = match last_handler_var {
                Some(v) => {
                    // Handler produced a value — return it and also push a Return
                    // so this value becomes the function return
                    func.push(IrInst::Return(v));
                    v
                }
                None => {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    zero
                }
            };
            let insts = std::mem::replace(&mut func.body, saved);
            (insts, handler_result)
        };

        // Normal arm (try_call returned 0 → completed without error)
        let normal_insts = {
            let saved = std::mem::take(&mut func.body);
            let pop_var = func.alloc_var();
            func.push(IrInst::Call(
                pop_var,
                "taida_error_ceiling_pop".to_string(),
                vec![],
            ));
            // Retrieve the return value from _taida_try_N via the global result slot
            let normal_result = func.alloc_var();
            func.push(IrInst::Call(
                normal_result,
                "taida_error_try_get_result".to_string(),
                vec![depth],
            ));
            func.push(IrInst::Return(normal_result));
            let insts = std::mem::replace(&mut func.body, saved);
            (insts, normal_result)
        };

        let cond_result = func.alloc_var();
        let arms = vec![
            crate::codegen::ir::CondArm {
                condition: Some(try_result),
                body: handler_insts.0,
                result: handler_insts.1,
            },
            crate::codegen::ir::CondArm {
                condition: None,
                body: normal_insts.0,
                result: normal_insts.1,
            },
        ];
        func.push(IrInst::CondBranch(cond_result, arms));

        Ok(())
    }

    pub(super) fn lower_statement(
        &mut self,
        func: &mut IrFunction,
        stmt: &Statement,
    ) -> Result<(), LowerError> {
        match stmt {
            Statement::EnumDef(_) => Ok(()),
            Statement::Expr(expr) => {
                self.lower_expr(func, expr)?;
                Ok(())
            }
            Statement::Assignment(assign) => {
                // ラムダが変数に代入される場合、マッピングを記録
                if let Expr::Lambda(params, body, _) = &assign.value {
                    let next_lambda_name = format!("_taida_lambda_{}", self.lambda_counter);
                    let param_names: std::collections::HashSet<&str> =
                        params.iter().map(|p| p.name.as_str()).collect();
                    let free_vars = self.collect_free_vars(body, &param_names);
                    if free_vars.is_empty() {
                        self.lambda_vars
                            .insert(assign.target.clone(), next_lambda_name);
                    } else {
                        self.lambda_vars
                            .insert(assign.target.clone(), next_lambda_name);
                        self.closure_vars.insert(assign.target.clone());
                    }
                    // NB3-4: Record lambda parameter count for handler_arity resolution
                    self.lambda_param_counts
                        .insert(assign.target.clone(), params.len());
                }
                // NB3-4: Track variable aliases for identity assignments (e.g., `h <= handler`)
                if let Expr::Ident(source_name, _) = &assign.value {
                    self.var_aliases
                        .insert(assign.target.clone(), source_name.clone());
                }
                let val = self.lower_expr(func, &assign.value)?;
                func.push(IrInst::DefVar(assign.target.clone(), val));

                // トップレベル変数をグローバルテーブルにも格納
                // （_taida_main 内で、かつ関数から参照されるトップレベル変数のみ）
                self.maybe_globalize_toplevel_binding(func, &assign.target, val);

                // NB-31: int を返す式の結果を追跡（callable_type_tag 精度向上）
                if self.expr_is_int(&assign.value) {
                    self.int_vars.insert(assign.target.clone());
                }
                // float を返す式の結果を追跡
                if self.expr_returns_float(&assign.value) {
                    self.float_vars.insert(assign.target.clone());
                }
                // string を返す式の結果を追跡
                if self.expr_is_string_full(&assign.value) {
                    self.string_vars.insert(assign.target.clone());
                }
                // bool を返す式の結果を追跡
                if self.expr_is_bool(&assign.value) {
                    self.bool_vars.insert(assign.target.clone());
                }
                // F-58: BuchiPack/TypeInst を返す式の結果を追跡
                if self.expr_is_pack(&assign.value) {
                    self.pack_vars.insert(assign.target.clone());
                    // Record per-field static kinds for pack literals so a
                    // later `p.x` field read carries its element kind into
                    // display dispatch (Float/Bool payloads are invisible
                    // to the value heuristics).
                    if let Expr::BuchiPack(fields, _) = &assign.value {
                        let kinds: std::collections::HashMap<String, i64> = fields
                            .iter()
                            .filter(|f| !matches!(f.value, Expr::Placeholder(_)))
                            .map(|f| (f.name.clone(), self.expr_type_tag(&f.value)))
                            .collect();
                        self.pack_field_kinds.insert(assign.target.clone(), kinds);
                    } else {
                        self.pack_field_kinds.remove(&assign.target);
                    }
                }
                // retain-on-store: List を返す式の結果を追跡
                if self.expr_is_list(&assign.value) {
                    self.list_vars.insert(assign.target.clone());
                }
                // Value-tag track: a plain rebind invalidates any runtime
                // shadow kind the name previously carried.
                self.shadow_kind_vars.remove(&assign.target);
                // C18-2: Enum-variant literal / known Enum var / annotation で
                // 束縛された変数を enum_vars に記録する。
                // `state <= HiveState:Policy()` や `state: HiveState <= ...`
                // を後段で `@(state <= state)` 構築時に field 型として認識する。
                if let Some(enum_name) = self.expr_enum_type_name(&assign.value) {
                    self.enum_vars.insert(assign.target.clone(), enum_name);
                } else if let Some(crate::parser::TypeExpr::Named(tn)) =
                    assign.type_annotation.as_ref()
                    && self.enum_defs.contains_key(tn)
                {
                    self.enum_vars.insert(assign.target.clone(), tn.clone());
                } else if let Expr::Ident(src_name, _) = &assign.value
                    && let Some(src_enum) = self.enum_vars.get(src_name).cloned()
                {
                    self.enum_vars.insert(assign.target.clone(), src_enum);
                }
                // QF-34 / F58B-003: MoldInst の Lax 内部型を追跡（unmold 時の型推定用）
                self.record_lax_inner_type(&assign.target, &assign.value);
                // QF-10: TypeInst の変数に TypeDef 名を記録（フィールド型解決用）
                if let Expr::TypeInst(type_name, _, _) = &assign.value {
                    self.var_type_names
                        .insert(assign.target.clone(), type_name.clone());
                }

                // ヒープ確保される式の変数をトラッキング
                if Self::is_heap_expr(&assign.value) {
                    self.current_heap_vars.push(assign.target.clone());
                } else if self.closure_vars.contains(&assign.target) {
                    // キャプチャありラムダ = クロージャ = ヒープオブジェクト
                    self.current_heap_vars.push(assign.target.clone());
                }

                // Track local assignment shadow: if the target name matches a net
                // builtin, subsequent calls in the same scope must use the local
                // variable, not the builtin dispatch.
                if Self::NET_BUILTIN_NAMES.contains(&assign.target.as_str()) {
                    self.shadowed_net_builtins.insert(assign.target.clone());
                }

                Ok(())
            }
            Statement::FuncDef(func_def_stmt) => {
                // トップレベルの定義は1st passで処理済み。
                // ローカル関数でキャプチャが必要なものは前処理で pending_local_closures に
                // 登録されているので、ここで MakeClosure + DefVar を発行する。
                if let Some((lambda_name, captures)) =
                    self.pending_local_closures.remove(&func_def_stmt.name)
                {
                    let dst = func.alloc_var();
                    func.push(IrInst::MakeClosure(dst, lambda_name, captures));
                    func.push(IrInst::DefVar(func_def_stmt.name.clone(), dst));
                    self.current_heap_vars.push(func_def_stmt.name.clone());
                }
                Ok(())
            }
            // (E30 Phase 2 Sub-step 2.1) ClassLikeDef 単一 variant + kind dispatch
            Statement::ClassLikeDef(cl) => match &cl.kind {
                crate::parser::ClassLikeKind::BuchiPack => {
                    let type_def = cl;
                    let non_method_field_defs: Vec<crate::parser::FieldDef> = type_def
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .cloned()
                        .collect();
                    let fields: Vec<String> = non_method_field_defs
                        .iter()
                        .map(|f| f.name.clone())
                        .collect();
                    self.type_fields.insert(type_def.name.clone(), fields);
                    let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                        non_method_field_defs
                            .iter()
                            .map(|f| (f.name.clone(), f.type_annotation.clone()))
                            .collect();
                    self.type_field_types
                        .insert(type_def.name.clone(), field_types);
                    self.type_field_defs
                        .insert(type_def.name.clone(), non_method_field_defs);
                    let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                        .fields
                        .iter()
                        .filter(|f| f.is_method)
                        .filter_map(|f| f.method_def.clone().map(|method| (f.name.clone(), method)))
                        .collect();
                    if !methods.is_empty() {
                        self.type_method_defs.insert(type_def.name.clone(), methods);
                    }
                    Ok(())
                }
                crate::parser::ClassLikeKind::Mold { .. } => {
                    let mold_def = cl;
                    let non_method_field_defs: Vec<crate::parser::FieldDef> = mold_def
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .cloned()
                        .collect();
                    let fields: Vec<String> = non_method_field_defs
                        .iter()
                        .map(|f| f.name.clone())
                        .collect();
                    self.type_fields.insert(mold_def.name.clone(), fields);
                    let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                        non_method_field_defs
                            .iter()
                            .map(|f| (f.name.clone(), f.type_annotation.clone()))
                            .collect();
                    self.type_field_types
                        .insert(mold_def.name.clone(), field_types);
                    self.type_field_defs
                        .insert(mold_def.name.clone(), non_method_field_defs);
                    self.mold_defs
                        .insert(mold_def.name.clone(), mold_def.clone());
                    Ok(())
                }
                crate::parser::ClassLikeKind::Inheritance {
                    parent,
                    parent_args,
                } => {
                    let inh_def = cl;
                    let inh_child = &inh_def.name;
                    let inh_parent = parent;
                    let mut all_fields = self
                        .type_fields
                        .get(inh_parent)
                        .cloned()
                        .unwrap_or_default();
                    let mut all_field_types = self
                        .type_field_types
                        .get(inh_parent)
                        .cloned()
                        .unwrap_or_default();
                    let mut all_field_defs = self
                        .type_field_defs
                        .get(inh_parent)
                        .cloned()
                        .unwrap_or_default();
                    for field in inh_def.fields.iter().filter(|f| !f.is_method) {
                        all_fields.retain(|name| name != &field.name);
                        all_fields.push(field.name.clone());
                        all_field_types.retain(|(name, _)| name != &field.name);
                        all_field_types.push((field.name.clone(), field.type_annotation.clone()));
                        all_field_defs.retain(|f| f.name != field.name);
                        all_field_defs.push(field.clone());
                    }
                    self.type_fields.insert(inh_child.clone(), all_fields);
                    self.type_field_types
                        .insert(inh_child.clone(), all_field_types);
                    self.type_field_defs
                        .insert(inh_child.clone(), all_field_defs);
                    let mut all_methods = self
                        .type_method_defs
                        .get(inh_parent)
                        .cloned()
                        .unwrap_or_default();
                    for field in inh_def.fields.iter().filter(|f| f.is_method) {
                        if let Some(method) = field.method_def.clone() {
                            all_methods.retain(|(name, _)| name != &field.name);
                            all_methods.push((field.name.clone(), method));
                        }
                    }
                    if !all_methods.is_empty() {
                        self.type_method_defs.insert(inh_child.clone(), all_methods);
                    }
                    self.type_parents
                        .insert(inh_child.clone(), inh_parent.clone());
                    let child_str_var = func.alloc_var();
                    func.push(IrInst::ConstStr(child_str_var, inh_child.clone()));
                    let parent_str_var = func.alloc_var();
                    func.push(IrInst::ConstStr(parent_str_var, inh_parent.clone()));
                    let reg_dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        reg_dummy,
                        "taida_register_type_parent".to_string(),
                        vec![child_str_var, parent_str_var],
                    ));
                    if let Some(parent_mold) = self.mold_defs.get(inh_parent).cloned() {
                        let parent_mold_args: Vec<crate::parser::MoldHeaderArg> =
                            parent_mold.mold_args().cloned().unwrap_or_default();
                        let parent_name_args = parent_mold.name_args.clone();
                        let parent_type_params = parent_mold.type_params.clone();
                        let mut merged_mold_fields = parent_mold.fields.clone();
                        for child_field in &inh_def.fields {
                            if let Some(existing) = merged_mold_fields
                                .iter_mut()
                                .find(|field| field.name == child_field.name)
                            {
                                *existing = child_field.clone();
                            } else {
                                merged_mold_fields.push(child_field.clone());
                            }
                        }
                        self.mold_defs.insert(
                            inh_child.clone(),
                            crate::parser::ClassLikeDef {
                                name: inh_child.clone(),
                                fields: merged_mold_fields,
                                doc_comments: inh_def.doc_comments.clone(),
                                span: inh_def.span.clone(),
                                kind: crate::parser::ClassLikeKind::Mold {
                                    mold_args: parent_mold_args,
                                },
                                name_args: inh_def
                                    .name_args
                                    .clone()
                                    .or_else(|| parent_args.clone())
                                    .or(parent_name_args),
                                type_params: parent_type_params,
                            },
                        );
                    }
                    Ok(())
                }
            },
            Statement::ErrorCeiling(ec) => {
                // lower_statement_sequence 経由で呼ばれるべきだが、
                // 直接呼ばれた場合は後続文なしで処理（フォールバック）
                self.lower_error_ceiling_with_body(func, ec, &[])
            }
            Statement::Export(_) | Statement::Import(_) => {
                // モジュールレベルで処理済み
                Ok(())
            }
            Statement::UnmoldForward(uf) => {
                // expr >=> name : Async のアンモールド
                // F58 P2-4 (first stage of the escape-analysis design):
                // direct-form unmold fusion. In `Mold[...]() >=> v` the
                // syntax guarantees the mold value has no other reference,
                // so when the unmold result is statically known the Lax is
                // never materialised — no allocation, no runtime call, no
                // has_value branch.
                if let Some(result) = self.try_lower_fused_unmold(func, &uf.source)? {
                    func.push(IrInst::DefVar(uf.target.clone(), result));
                    self.maybe_globalize_toplevel_binding(func, &uf.target, result);
                    self.track_unmold_type(&uf.target, &uf.source);
                    // A rebind invalidates any previous shadow kind for
                    // this name (mirrors maybe_capture_shadow_kind).
                    self.shadow_kind_vars.remove(&uf.target);
                    if Self::NET_BUILTIN_NAMES.contains(&uf.target.as_str()) {
                        self.shadowed_net_builtins.insert(uf.target.clone());
                    }
                    return Ok(());
                }
                let source_var = self.lower_expr(func, &uf.source)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![source_var],
                ));
                func.push(IrInst::DefVar(uf.target.clone(), result));
                self.maybe_globalize_toplevel_binding(func, &uf.target, result);
                // Track type from mold source for debug display
                self.track_unmold_type(&uf.target, &uf.source);
                self.maybe_capture_shadow_kind(func, &uf.target, &uf.source, source_var);
                // Track local unmold-forward shadow for net builtins
                if Self::NET_BUILTIN_NAMES.contains(&uf.target.as_str()) {
                    self.shadowed_net_builtins.insert(uf.target.clone());
                }
                Ok(())
            }
            Statement::UnmoldBackward(ub) => {
                // name <=< expr : Async のアンモールド（逆方向）
                // F58 P2-4: same direct-form fusion as UnmoldForward.
                if let Some(result) = self.try_lower_fused_unmold(func, &ub.source)? {
                    func.push(IrInst::DefVar(ub.target.clone(), result));
                    self.maybe_globalize_toplevel_binding(func, &ub.target, result);
                    self.track_unmold_type(&ub.target, &ub.source);
                    self.shadow_kind_vars.remove(&ub.target);
                    if Self::NET_BUILTIN_NAMES.contains(&ub.target.as_str()) {
                        self.shadowed_net_builtins.insert(ub.target.clone());
                    }
                    return Ok(());
                }
                let source_var = self.lower_expr(func, &ub.source)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![source_var],
                ));
                func.push(IrInst::DefVar(ub.target.clone(), result));
                self.maybe_globalize_toplevel_binding(func, &ub.target, result);
                // Track type from mold source for debug display
                self.track_unmold_type(&ub.target, &ub.source);
                self.maybe_capture_shadow_kind(func, &ub.target, &ub.source, source_var);
                // Track local unmold-backward shadow for net builtins
                if Self::NET_BUILTIN_NAMES.contains(&ub.target.as_str()) {
                    self.shadowed_net_builtins.insert(ub.target.clone());
                }
                Ok(())
            } // All statement types are now handled above.
              // This branch should not be reached.
        }
    }

    /// First stage of the escape-analysis design: fuse the
    /// direct form `Mold[...]() >=> v` when the unmold result is
    /// statically known and carries no allocation:
    ///
    /// - `Lax[x]() >=> v` always has `has_value = true`, so `v` is `x`
    ///   itself. Restricted to statically-scalar `x` for now: a heap
    ///   payload would change the retain/release pairing that the
    ///   materialised path performs (second stage, with the IR-level
    ///   escape pass).
    /// - `Div[a, b]() >=> v` / `Mod[a, b]()` with all-Int operands and a
    ///   non-zero Int-literal divisor can never produce the empty Lax,
    ///   so `v` is the exact quotient/remainder via a divisor-proven
    ///   runtime helper (no Lax, no branch).
    ///
    /// Named arguments (an explicit default, etc.) keep the general
    /// materialised path.
    pub(super) fn try_lower_fused_unmold(
        &mut self,
        func: &mut IrFunction,
        source: &Expr,
    ) -> Result<Option<IrVar>, LowerError> {
        let Expr::MoldInst(name, type_args, fields, _) = source else {
            return Ok(None);
        };
        if !fields.is_empty() {
            return Ok(None);
        }
        match (name.as_str(), type_args.as_slice()) {
            ("Lax", [inner])
                if self.expr_is_int(inner)
                    || self.expr_returns_float(inner)
                    || self.expr_is_bool(inner) =>
            {
                Ok(Some(self.lower_expr(func, inner)?))
            }
            ("Div" | "Mod", [a, b]) if Self::nonzero_int_literal(b) && self.expr_is_int(a) => {
                let av = self.lower_expr(func, a)?;
                let bv = self.lower_expr(func, b)?;
                let result = func.alloc_var();
                let helper = if name == "Div" {
                    "taida_div_exact"
                } else {
                    "taida_mod_exact"
                };
                func.push(IrInst::Call(result, helper.to_string(), vec![av, bv]));
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    /// Divisor proof for the Div/Mod fusion: a non-zero Int literal
    /// (optionally negated).
    fn nonzero_int_literal(e: &Expr) -> bool {
        match e {
            Expr::IntLit(v, _) => *v != 0,
            Expr::UnaryOp(crate::parser::UnaryOp::Neg, inner, _) => {
                matches!(inner.as_ref(), Expr::IntLit(v, _) if *v != 0)
            }
            _ => false,
        }
    }

    /// クロージャ本体内のネストされた FuncDef を再帰的に前処理する。
    /// scope_vars: 親スコープで利用可能な変数名
    /// （params + captures + ローカル代入変数 + ローカル関数名）。
    /// ネストされた FuncDef がスコープ変数を参照する場合はクロージャとして生成し、
    /// pending_local_closures に登録する。さらに深いネストも再帰的に処理する。
    pub(super) fn preprocess_inner_funcdefs(
        &mut self,
        body: &[Statement],
        scope_vars: &[String],
    ) -> Result<(), LowerError> {
        let scope_set: std::collections::HashSet<&str> =
            scope_vars.iter().map(|s| s.as_str()).collect();

        for stmt in body {
            if let Statement::FuncDef(fd) = stmt {
                let fd_params: std::collections::HashSet<&str> =
                    fd.params.iter().map(|p| p.name.as_str()).collect();
                let free = self.collect_free_vars_in_func_body_unfiltered(&fd.body, &fd_params);
                let captures: Vec<String> = free
                    .into_iter()
                    .filter(|v| scope_set.contains(v.as_str()))
                    .collect();

                if captures.is_empty() {
                    // キャプチャなし: 通常のユーザー関数として登録
                    self.user_funcs.insert(fd.name.clone());
                    self.func_param_defs
                        .insert(fd.name.clone(), fd.params.clone());
                    let ir = self.lower_func_def(fd)?;
                    self.lambda_funcs.push(ir);
                } else {
                    // キャプチャあり: クロージャとして生成
                    let lambda_name = format!("_taida_lambda_{}", self.lambda_counter);
                    self.lambda_counter += 1;

                    self.lambda_vars
                        .insert(fd.name.clone(), lambda_name.clone());
                    self.closure_vars.insert(fd.name.clone());

                    let mut closure_params: Vec<String> = vec!["__env".to_string()];
                    closure_params.extend(fd.params.iter().map(|p| p.name.clone()));
                    let mut lambda_fn =
                        IrFunction::new_with_params(lambda_name.clone(), closure_params);

                    // 環境からキャプチャ変数を復元
                    let env_var = 0u32;
                    for (i, cap_name) in captures.iter().enumerate() {
                        let get_dst = lambda_fn.alloc_var();
                        lambda_fn.push(IrInst::PackGet(get_dst, env_var, i));
                        lambda_fn.push(IrInst::DefVar(cap_name.clone(), get_dst));
                    }

                    // グローバル変数復元
                    let param_names: Vec<String> =
                        fd.params.iter().map(|p| p.name.clone()).collect();
                    let global_refs = self.collect_free_vars_in_body(&fd.body, &param_names);
                    for var_name in &global_refs {
                        if !captures.contains(var_name) {
                            self.globals_referenced.insert(var_name.clone());
                            let hash = self.global_var_hash(var_name);
                            let dst = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::GlobalGet(dst, hash));
                            lambda_fn.push(IrInst::DefVar(var_name.clone(), dst));
                        }
                    }

                    // 再帰的に内部 FuncDef を前処理（深いネスト対応）
                    let inner_scope = self.collect_nested_scope_vars(
                        captures
                            .iter()
                            .cloned()
                            .chain(fd.params.iter().map(|p| p.name.clone())),
                        &fd.body,
                    );
                    self.preprocess_inner_funcdefs(&fd.body, &inner_scope)?;

                    // 関数本体を処理
                    let body_refs: Vec<&Statement> = fd.body.iter().collect();
                    let has_ec = body_refs
                        .iter()
                        .any(|s| matches!(s, Statement::ErrorCeiling(_)));
                    if has_ec {
                        self.lower_statement_sequence(&mut lambda_fn, &body_refs)?;
                    } else {
                        let mut last_var = None;
                        for (j, s) in fd.body.iter().enumerate() {
                            let is_last = j == fd.body.len() - 1;
                            match s {
                                Statement::Expr(expr) => {
                                    let var = self.lower_expr(&mut lambda_fn, expr)?;
                                    if is_last {
                                        last_var = Some(var);
                                    }
                                }
                                _ => {
                                    self.lower_statement(&mut lambda_fn, s)?;
                                }
                            }
                        }

                        if let Some(ret) = last_var {
                            lambda_fn.push(IrInst::Return(ret));
                        } else {
                            let zero = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::ConstInt(zero, 0));
                            lambda_fn.push(IrInst::Return(zero));
                        }
                    }

                    self.user_funcs.insert(lambda_name.clone());
                    self.lambda_funcs.push(lambda_fn);

                    self.pending_local_closures
                        .insert(fd.name.clone(), (lambda_name, captures));
                }
            }
        }
        Ok(())
    }

    /// ネスト関数が参照可能な親スコープ変数を収集する。
    /// base_vars に加え、同一ボディで束縛されるローカル代入とローカル関数名を含める。
    pub(super) fn collect_nested_scope_vars<I>(
        &self,
        base_vars: I,
        body: &[Statement],
    ) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut vars = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push_unique = |name: String| {
            if seen.insert(name.clone()) {
                vars.push(name);
            }
        };

        for name in base_vars {
            push_unique(name);
        }

        for stmt in body {
            match stmt {
                Statement::Assignment(assign) => push_unique(assign.target.clone()),
                Statement::FuncDef(fd) => push_unique(fd.name.clone()),
                _ => {}
            }
        }

        vars
    }

    /// 式中の自由変数を収集する
    pub(super) fn collect_free_vars(
        &self,
        expr: &Expr,
        bound: &std::collections::HashSet<&str>,
    ) -> Vec<String> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_free_vars_inner(expr, bound, &mut free, &mut seen);
        free
    }

    pub(super) fn collect_free_vars_inner(
        &self,
        expr: &Expr,
        bound: &std::collections::HashSet<&str>,
        free: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, _)
                if !bound.contains(name.as_str())
                    && !self.user_funcs.contains(name)
                    && !seen.contains(name) =>
            {
                seen.insert(name.clone());
                free.push(name.clone());
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                self.collect_free_vars_inner(lhs, bound, free, seen);
                self.collect_free_vars_inner(rhs, bound, free, seen);
            }
            Expr::UnaryOp(_, operand, _) => {
                self.collect_free_vars_inner(operand, bound, free, seen);
            }
            Expr::FuncCall(callee, args, _) => {
                self.collect_free_vars_inner(callee, bound, free, seen);
                for arg in args {
                    self.collect_free_vars_inner(arg, bound, free, seen);
                }
            }
            Expr::FieldAccess(obj, _, _) => {
                self.collect_free_vars_inner(obj, bound, free, seen);
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.collect_free_vars_inner(obj, bound, free, seen);
                for arg in args {
                    self.collect_free_vars_inner(arg, bound, free, seen);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.collect_free_vars_inner(e, bound, free, seen);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.collect_free_vars_inner(cond, bound, free, seen);
                    }
                    for stmt in &arm.body {
                        self.collect_free_vars_in_stmt(stmt, bound, free, seen);
                    }
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    self.collect_free_vars_inner(&field.value, bound, free, seen);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.collect_free_vars_inner(item, bound, free, seen);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for arg in args {
                    self.collect_free_vars_inner(arg, bound, free, seen);
                }
                for field in fields {
                    self.collect_free_vars_inner(&field.value, bound, free, seen);
                }
            }
            Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                self.collect_free_vars_inner(inner, bound, free, seen);
            }
            Expr::Lambda(params, body, _) => {
                let mut inner_bound = bound.clone();
                for p in params {
                    inner_bound.insert(p.name.as_str());
                }
                self.collect_free_vars_inner(body, &inner_bound, free, seen);
            }
            // C25B-030 Phase 1F: `TemplateLit` stores the raw
            // interpolated source (`"${a}${sep}${b}"` etc.); the
            // real interpolation expressions are re-parsed during
            // `lower_template_lit`. Before Phase 1F this arm fell
            // into the `_ => {}` catch-all, so a top-level binding
            // referenced only from a template (e.g. `sep <= "-"` at
            // module top level, then `join a b = \`${a}${sep}${b}\``
            // as a FuncDef) was never added to `globals_referenced`
            // and no `GlobalGet(hash)` was emitted at the head of
            // the FuncDef body. The native binary read 0 from the
            // uninitialised global slot and printed `ab` instead of
            // `a-b`, diverging from the interpreter which eagerly
            // resolves `sep` through lexical scope.
            //
            // The fix splits the template on `${...}` boundaries
            // (same logic as `src/codegen/lower/expr.rs::
            // lower_template_lit`) and re-parses each interpolation
            // as a standalone expression so the normal free-var
            // walker runs over it. A non-expression or parser-rejected
            // body lowers to a `ConstStr` (empty or raw text) and
            // captures nothing, mirroring the real lowering.
            Expr::TemplateLit(template, _) => {
                Self::collect_free_vars_in_template(template, bound, free, seen);
            }
            _ => {}
        }
    }

    /// helper: walk a `TemplateLit` body for
    /// free-variable references. Mirrors
    /// `lower_template_lit`'s `${...}` parser so the free-var
    /// collection sees exactly the same identifiers the native
    /// lowering will later resolve. Structurally identical to
    /// `facade_collect_refs_in_template` in
    /// `src/codegen/lower/imports.rs`; kept as a sibling to avoid
    /// coupling the `Lowering` struct's method signature to the
    /// facade-loader universe maps.
    fn collect_free_vars_in_template(
        template: &str,
        bound: &std::collections::HashSet<&str>,
        free: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                i += 2;
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
                let trimmed = expr_str.trim();
                let (program, errors) = crate::parser::parse(trimmed);
                if errors.is_empty()
                    && !program.statements.is_empty()
                    && let Statement::Expr(ref parsed_expr) = program.statements[0]
                {
                    Self::collect_free_vars_in_parsed_expr(parsed_expr, bound, free, seen);
                }
                // Otherwise the body lowers to a `ConstStr` in
                // `lower_template_lit` (a non-expression statement → ""; a
                // parser-rejected / empty body → raw text), reading no variable,
                // so collect no free var here. This keeps the capture walker in
                // step with the ConstStr fallback (F56-FB-006 follow-up;
                // previously this path captured the trimmed body as a bare
                // identifier, matching the old bare-variable lowering).
                if i < chars.len() {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }

    /// Free-function variant of the free-vars walker used by the
    /// template interpolation path. Does not consult `self` — the
    /// enclosing `collect_free_vars_in_body` filter applies its
    /// `top_level_vars` / `imported_value_names` pass after the
    /// walk, so we can stay associative here.
    fn collect_free_vars_in_parsed_expr(
        expr: &Expr,
        bound: &std::collections::HashSet<&str>,
        free: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, _) if !bound.contains(name.as_str()) && !seen.contains(name) => {
                seen.insert(name.clone());
                free.push(name.clone());
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                Self::collect_free_vars_in_parsed_expr(lhs, bound, free, seen);
                Self::collect_free_vars_in_parsed_expr(rhs, bound, free, seen);
            }
            Expr::UnaryOp(_, operand, _) => {
                Self::collect_free_vars_in_parsed_expr(operand, bound, free, seen);
            }
            Expr::FuncCall(callee, args, _) => {
                Self::collect_free_vars_in_parsed_expr(callee, bound, free, seen);
                for arg in args {
                    Self::collect_free_vars_in_parsed_expr(arg, bound, free, seen);
                }
            }
            Expr::FieldAccess(obj, _, _) => {
                Self::collect_free_vars_in_parsed_expr(obj, bound, free, seen);
            }
            Expr::MethodCall(obj, _, args, _) => {
                Self::collect_free_vars_in_parsed_expr(obj, bound, free, seen);
                for arg in args {
                    Self::collect_free_vars_in_parsed_expr(arg, bound, free, seen);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    Self::collect_free_vars_in_parsed_expr(e, bound, free, seen);
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    Self::collect_free_vars_in_parsed_expr(&field.value, bound, free, seen);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    Self::collect_free_vars_in_parsed_expr(item, bound, free, seen);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for arg in args {
                    Self::collect_free_vars_in_parsed_expr(arg, bound, free, seen);
                }
                for field in fields {
                    Self::collect_free_vars_in_parsed_expr(&field.value, bound, free, seen);
                }
            }
            Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                Self::collect_free_vars_in_parsed_expr(inner, bound, free, seen);
            }
            Expr::TemplateLit(nested, _) => {
                Self::collect_free_vars_in_template(nested, bound, free, seen);
            }
            _ => {}
        }
    }

    /// Collect free variables from a single statement.
    pub(super) fn collect_free_vars_in_stmt(
        &self,
        stmt: &Statement,
        bound: &std::collections::HashSet<&str>,
        free: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match stmt {
            Statement::Expr(expr) => {
                self.collect_free_vars_inner(expr, bound, free, seen);
            }
            Statement::Assignment(assign) => {
                self.collect_free_vars_inner(&assign.value, bound, free, seen);
            }
            Statement::UnmoldForward(u) => {
                self.collect_free_vars_inner(&u.source, bound, free, seen);
            }
            Statement::UnmoldBackward(u) => {
                self.collect_free_vars_inner(&u.source, bound, free, seen);
            }
            _ => {}
        }
    }

    /// 関数本体（Statement列）から参照される自由変数を収集する。
    /// パラメータと関数内で定義される変数は除外し、
    /// トップレベル変数または import 値のみ残す。
    pub(super) fn collect_free_vars_in_body(
        &self,
        body: &[Statement],
        param_names: &[String],
    ) -> Vec<String> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // 関数内で定義される変数名も bound に含める
        let mut bound: std::collections::HashSet<&str> =
            param_names.iter().map(|s| s.as_str()).collect();
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                bound.insert(assign.target.as_str());
            }
            // Unmold bindings are local definitions as well — without
            // this, a body-local `... >=> v` whose name collides with a
            // top-level variable would be misread as a global reference
            // and the GlobalGet restore would clobber the local.
            if let Statement::UnmoldForward(uf) = stmt {
                bound.insert(uf.target.as_str());
            }
            if let Statement::UnmoldBackward(ub) = stmt {
                bound.insert(ub.target.as_str());
            }
        }
        for stmt in body {
            match stmt {
                Statement::Expr(expr) => {
                    self.collect_free_vars_inner(expr, &bound, &mut free, &mut seen);
                }
                Statement::Assignment(assign) => {
                    self.collect_free_vars_inner(&assign.value, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldForward(uf) => {
                    self.collect_free_vars_inner(&uf.source, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldBackward(ub) => {
                    self.collect_free_vars_inner(&ub.source, &bound, &mut free, &mut seen);
                }
                Statement::ErrorCeiling(ec) => {
                    // ErrorCeiling のハンドラ本体からも自由変数を収集
                    for handler_stmt in &ec.handler_body {
                        match handler_stmt {
                            Statement::Expr(e) => {
                                self.collect_free_vars_inner(e, &bound, &mut free, &mut seen);
                            }
                            Statement::Assignment(a) => {
                                self.collect_free_vars_inner(
                                    &a.value, &bound, &mut free, &mut seen,
                                );
                            }
                            Statement::UnmoldForward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source, &bound, &mut free, &mut seen,
                                );
                            }
                            Statement::UnmoldBackward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source, &bound, &mut free, &mut seen,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        // トップレベル変数または import 値のみフィルタ
        free.into_iter()
            .filter(|name| {
                self.top_level_vars.contains(name) || self.imported_value_names.contains(name)
            })
            .collect()
    }

    /// 関数本体の自由変数を収集する（フィルタなし版）。
    /// ローカル関数が親スコープの変数をキャプチャするかどうかの判定に使用。
    pub(super) fn collect_free_vars_in_func_body_unfiltered(
        &self,
        body: &[Statement],
        param_names: &std::collections::HashSet<&str>,
    ) -> Vec<String> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut bound: std::collections::HashSet<&str> = param_names.clone();
        // 関数内で定義される変数名も bound に含める
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                bound.insert(assign.target.as_str());
            }
            if let Statement::FuncDef(fd) = stmt {
                bound.insert(fd.name.as_str());
            }
        }
        for stmt in body {
            match stmt {
                Statement::Expr(expr) => {
                    self.collect_free_vars_inner(expr, &bound, &mut free, &mut seen);
                }
                Statement::Assignment(assign) => {
                    self.collect_free_vars_inner(&assign.value, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldForward(uf) => {
                    self.collect_free_vars_inner(&uf.source, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldBackward(ub) => {
                    self.collect_free_vars_inner(&ub.source, &bound, &mut free, &mut seen);
                }
                Statement::FuncDef(fd) => {
                    // Recurse into nested function definitions to find transitively
                    // referenced free variables (e.g. f1 → f2 → f3 where f3 uses f1's var).
                    let inner_params: std::collections::HashSet<&str> =
                        fd.params.iter().map(|p| p.name.as_str()).collect();
                    let inner_free =
                        self.collect_free_vars_in_func_body_unfiltered(&fd.body, &inner_params);
                    for var in inner_free {
                        if !bound.contains(var.as_str()) && !seen.contains(&var) {
                            seen.insert(var.clone());
                            free.push(var);
                        }
                    }
                }
                Statement::ErrorCeiling(ec) => {
                    // ErrorCeiling の handler_body を走査
                    // (collect_free_vars_in_body のパターンを踏襲)
                    // error_param はハンドラのバインド変数
                    let mut handler_bound = bound.clone();
                    handler_bound.insert(ec.error_param.as_str());
                    for handler_stmt in &ec.handler_body {
                        match handler_stmt {
                            Statement::Expr(e) => {
                                self.collect_free_vars_inner(
                                    e,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            Statement::Assignment(a) => {
                                self.collect_free_vars_inner(
                                    &a.value,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            Statement::UnmoldForward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            Statement::UnmoldBackward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        free
    }

    /// 条件分岐: `| cond |> value` パターン
    pub(super) fn lower_cond_branch(
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
                None => None, // デフォルトケース
            };

            // 本体を一時的な命令列に lowering（複数ステートメント対応）
            let (body_insts, body_var) = {
                let saved = std::mem::take(&mut func.body);
                let body_result = self.lower_cond_arm_body(func, &arm.body)?;
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

    /// Lower a condition arm body (Vec<Statement>) to IR.
    /// Returns the IR variable holding the result of the last expression.
    ///
    /// A tail binding statement (`Assignment` / `UnmoldForward` /
    /// `UnmoldBackward`) yields the bound value as the arm result, so the
    /// IR variable produced by that statement becomes `last_var`.
    pub(super) fn lower_cond_arm_body(
        &mut self,
        func: &mut IrFunction,
        body: &[Statement],
    ) -> Result<IrVar, LowerError> {
        // Fallback: allocate a default result (int 0) in case body has no expression
        let mut last_var = func.alloc_var();
        func.push(IrInst::ConstInt(last_var, 0));
        for (i, stmt) in body.iter().enumerate() {
            let is_last = i == body.len() - 1;
            match stmt {
                Statement::Expr(expr) => {
                    let var = self.lower_expr(func, expr)?;
                    if is_last {
                        last_var = var;
                    }
                }
                _ => {
                    self.lower_statement(func, stmt)?;
                    if is_last && let Some(bound_var) = Self::tail_binding_var(func, stmt) {
                        last_var = bound_var;
                    }
                }
            }
        }
        Ok(last_var)
    }

    /// Lower a condition arm body in tail position.
    /// The last expression is lowered with tail-call optimization.
    ///
    /// Tail-binding statements cannot be TCO'd (the value is bound
    /// first and then yielded), so they are lowered via the normal path
    /// and the IR variable for the binding becomes the tail value.
    pub(super) fn lower_cond_arm_body_tail(
        &mut self,
        func: &mut IrFunction,
        body: &[Statement],
    ) -> Result<IrVar, LowerError> {
        // Fallback: allocate a default result (int 0) in case body has no expression
        let mut last_var = func.alloc_var();
        func.push(IrInst::ConstInt(last_var, 0));
        for (i, stmt) in body.iter().enumerate() {
            let is_last = i == body.len() - 1;
            match stmt {
                Statement::Expr(expr) => {
                    let var = if is_last {
                        self.lower_expr_tail(func, expr)?
                    } else {
                        self.lower_expr(func, expr)?
                    };
                    if is_last {
                        last_var = var;
                    }
                }
                _ => {
                    self.lower_statement(func, stmt)?;
                    if is_last && let Some(bound_var) = Self::tail_binding_var(func, stmt) {
                        last_var = bound_var;
                    }
                }
            }
        }
        Ok(last_var)
    }

    /// If `stmt` is a tail-binding statement that was just lowered
    /// via `lower_statement`, return the IR variable bound by the trailing
    /// `DefVar(target, value)` instruction so the caller can treat it as
    /// the block's yield value.
    ///
    /// `lower_statement` always ends with `DefVar(assign.target, val)` for
    /// `Assignment` / `UnmoldForward` / `UnmoldBackward`, so peeking at the
    /// last `DefVar` whose name matches the binding target reliably
    /// recovers the IR value.
    fn tail_binding_var(func: &IrFunction, stmt: &Statement) -> Option<IrVar> {
        let target = match stmt {
            Statement::Assignment(a) => &a.target,
            Statement::UnmoldForward(u) => &u.target,
            Statement::UnmoldBackward(u) => &u.target,
            _ => return None,
        };
        for inst in func.body.iter().rev() {
            if let IrInst::DefVar(name, var) = inst
                && name == target
            {
                return Some(*var);
            }
        }
        None
    }
}
