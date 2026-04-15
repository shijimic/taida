// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups stmt methods of the `Lowering` struct (placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{ImportedSymbolKind, LowerError, Lowering, simple_hash};
use crate::codegen::ir::*;
use crate::net_surface::NET_HTTP_PROTOCOL_SYMBOL;
use crate::parser::*;

impl Lowering {
    pub fn lower_program(&mut self, program: &Program) -> Result<IrModule, LowerError> {
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
                            crate::parser::TypeExpr::Named(n) if n == "Bool" => {
                                self.bool_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::Named(n) if n == "Float" => {
                                self.float_returning_funcs.insert(func_def.name.clone());
                            }
                            // NB-31: Track Int/Num-returning functions for callable_type_tag
                            crate::parser::TypeExpr::Named(n) if n == "Int" || n == "Num" => {
                                self.int_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::List(_) => {
                                self.list_returning_funcs.insert(func_def.name.clone());
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
                    // C12-11 (FB-1): body-based inference for Bool-returning
                    // functions so that `b <= is_int(42); stdout(b)` preserves
                    // the Bool tag through let-binding and displays
                    // "true"/"false" on Native. Only triggers when:
                    //   - no explicit return type annotation contradicts (if
                    //     `return_type` is declared to a non-Bool type we
                    //     respect the annotation and do NOT override it)
                    //   - the body's last statement is an expression
                    //     recognised by `expr_is_bool` (BoolLit, Bool-returning
                    //     MoldInst like `TypeIs`/`TypeExtends`/`Exists`,
                    //     Bool-returning method call, comparison/logical op,
                    //     `!expr`).
                    let annotated_non_bool = matches!(
                        &func_def.return_type,
                        Some(crate::parser::TypeExpr::Named(n))
                            if n != "Bool"
                    );
                    if !annotated_non_bool
                        && let Some(Statement::Expr(last)) = func_def.body.last()
                        && self.expr_is_bool(last)
                    {
                        self.bool_returning_funcs.insert(func_def.name.clone());
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
                Statement::TypeDef(type_def) => {
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
                                    _ => 0,
                                },
                                _ => 0,
                            };
                            self.register_field_type_tag(&field_def.name, tag);
                        } else if let Some(ref default_expr) = field_def.default_value {
                            // Infer type from default value expression
                            if self.expr_is_bool(default_expr) {
                                self.register_field_type_tag(&field_def.name, 4);
                                // Bool
                            }
                        }
                    }
                    self.type_fields.insert(type_def.name.clone(), fields);
                    // JSON スキーマ解決用: フィールド名+型アノテーション
                    let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                        non_method_field_defs
                            .iter()
                            .map(|f| (f.name.clone(), f.type_annotation.clone()))
                            .collect();
                    self.type_field_types
                        .insert(type_def.name.clone(), field_types);
                    self.type_field_defs
                        .insert(type_def.name.clone(), non_method_field_defs);
                    // Register method definitions for TypeDef method closure generation
                    let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                        .fields
                        .iter()
                        .filter(|f| f.is_method && f.method_def.is_some())
                        .map(|f| (f.name.clone(), f.method_def.clone().unwrap()))
                        .collect();
                    if !methods.is_empty() {
                        self.type_method_defs.insert(type_def.name.clone(), methods);
                    }
                }
                Statement::EnumDef(enum_def) => {
                    self.enum_defs.insert(
                        enum_def.name.clone(),
                        enum_def
                            .variants
                            .iter()
                            .map(|variant| variant.name.clone())
                            .collect(),
                    );
                }
                Statement::MoldDef(mold_def) => {
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
                Statement::InheritanceDef(inh_def) => {
                    let mut all_fields = self
                        .type_fields
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    let mut all_field_types = self
                        .type_field_types
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    let mut all_field_defs = self
                        .type_field_defs
                        .get(&inh_def.parent)
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
                    self.type_fields.insert(inh_def.child.clone(), all_fields);
                    self.type_field_types
                        .insert(inh_def.child.clone(), all_field_types);
                    self.type_field_defs
                        .insert(inh_def.child.clone(), all_field_defs);
                    // Inherit parent methods, then override/add child methods
                    let mut all_methods = self
                        .type_method_defs
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    for field in inh_def
                        .fields
                        .iter()
                        .filter(|f| f.is_method && f.method_def.is_some())
                    {
                        all_methods.retain(|(name, _)| name != &field.name);
                        all_methods.push((field.name.clone(), field.method_def.clone().unwrap()));
                    }
                    if !all_methods.is_empty() {
                        self.type_method_defs
                            .insert(inh_def.child.clone(), all_methods);
                    }
                    if let Some(parent_mold) = self.mold_defs.get(&inh_def.parent).cloned() {
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
                            inh_def.child.clone(),
                            crate::parser::MoldDef {
                                name: inh_def.child.clone(),
                                mold_args: parent_mold.mold_args.clone(),
                                name_args: inh_def
                                    .child_args
                                    .clone()
                                    .or_else(|| inh_def.parent_args.clone())
                                    .or(parent_mold.name_args.clone()),
                                type_params: parent_mold.type_params.clone(),
                                fields: merged_mold_fields,
                                doc_comments: inh_def.doc_comments.clone(),
                                span: inh_def.span.clone(),
                            },
                        );
                    }
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

                    let is_core_bundled_path = matches!(
                        path.as_str(),
                        "taida-lang/os"
                            | "taida-lang/js"
                            | "taida-lang/crypto"
                            | "taida-lang/net"
                            | "taida-lang/pool"
                    );

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
                                message: match v {
                                    crate::pkg::facade::FacadeViolation::HiddenSymbol {
                                        name,
                                        available,
                                    } => {
                                        format!(
                                            "Symbol '{}' is not part of the public API declared in packages.tdm. \
                                             Available exports: {}",
                                            name,
                                            available.join(", ")
                                        )
                                    }
                                    crate::pkg::facade::FacadeViolation::GhostSymbol { name } => {
                                        format!(
                                            "Symbol '{}' is declared in packages.tdm but not found in the entry module. \
                                             The entry module must export all symbols listed in the package facade.",
                                            name
                                        )
                                    }
                                },
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
                }
                if self.expr_is_list(&assign.value) {
                    self.list_vars.insert(assign.target.clone());
                }
                // QF-34: MoldInst の Lax 内部型を追跡（unmold 時の型推定用）
                if let Expr::MoldInst(mold_name, _, _, _) = &assign.value {
                    self.lax_inner_types
                        .insert(assign.target.clone(), mold_name.clone());
                }
                // QF-10: TypeInst の変数に TypeDef 名を記録
                if let Expr::TypeInst(type_name, _, _) = &assign.value {
                    self.var_type_names
                        .insert(assign.target.clone(), type_name.clone());
                }
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

            // RC2.5 Phase 2: replay addon facade pack bindings before
            // any user statement runs. The bindings are synthetic
            // `Name <= @(...)` assignments harvested from the addon's
            // `taida/<stem>.td` facade during `lower_addon_import`; they
            // are the native-backend equivalent of the
            // `module_eval::load_addon_facade` path used by the
            // interpreter (e.g. `KeyKind <= @(Char <= 0, ...)`).
            let facade_bindings = std::mem::take(&mut self.addon_facade_pack_bindings);
            for (name, expr) in &facade_bindings {
                let val = self.lower_expr(&mut main_fn, expr)?;
                main_fn.push(IrInst::DefVar(name.clone(), val));
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
                if type_tag > 0 {
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
                    crate::parser::TypeExpr::List(_) => {
                        self.list_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::BuchiPack(_) => {
                        self.pack_vars.insert(param.name.clone());
                    }
                    _ => {}
                }
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
        // NB3-4 fix: Restore return_type_inferred_params to pre-function state
        self.return_type_inferred_params = prev_return_type_inferred_params;
        // NB3-4: Restore var_aliases, lambda_param_counts, lambda_vars, closure_vars
        // to pre-function state (parameter shadow cleanup)
        self.var_aliases = prev_var_aliases;
        self.lambda_param_counts = prev_lambda_param_counts;
        self.lambda_vars = prev_lambda_vars;
        self.closure_vars = prev_closure_vars;

        Ok(ir_func)
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
                if self.current_func_name.is_none()
                    && self.globals_referenced.contains(&assign.target)
                {
                    let hash = self.global_var_hash(&assign.target);
                    func.push(IrInst::GlobalSet(hash, val));
                }

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
                }
                // retain-on-store: List を返す式の結果を追跡
                if self.expr_is_list(&assign.value) {
                    self.list_vars.insert(assign.target.clone());
                }
                // QF-34: MoldInst の Lax 内部型を追跡（unmold 時の型推定用）
                if let Expr::MoldInst(mold_name, _, _, _) = &assign.value {
                    self.lax_inner_types
                        .insert(assign.target.clone(), mold_name.clone());
                }
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
            Statement::TypeDef(type_def) => {
                // Register type fields (already done in 1st pass, but safe to repeat)
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
                // JSON スキーマ解決用
                let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                    non_method_field_defs
                        .iter()
                        .map(|f| (f.name.clone(), f.type_annotation.clone()))
                        .collect();
                self.type_field_types
                    .insert(type_def.name.clone(), field_types);
                self.type_field_defs
                    .insert(type_def.name.clone(), non_method_field_defs);
                // Register method definitions (safe to repeat from 1st pass)
                let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                    .fields
                    .iter()
                    .filter(|f| f.is_method && f.method_def.is_some())
                    .map(|f| (f.name.clone(), f.method_def.clone().unwrap()))
                    .collect();
                if !methods.is_empty() {
                    self.type_method_defs.insert(type_def.name.clone(), methods);
                }
                Ok(())
            }
            Statement::MoldDef(mold_def) => {
                // MoldDef is internally treated like TypeDef
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
                // JSON スキーマ解決用
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
            Statement::InheritanceDef(inh_def) => {
                // Inheritance: parent fields + child fields
                let mut all_fields = self
                    .type_fields
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                let mut all_field_types = self
                    .type_field_types
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                let mut all_field_defs = self
                    .type_field_defs
                    .get(&inh_def.parent)
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
                self.type_fields.insert(inh_def.child.clone(), all_fields);
                self.type_field_types
                    .insert(inh_def.child.clone(), all_field_types);
                self.type_field_defs
                    .insert(inh_def.child.clone(), all_field_defs);
                // Inherit parent methods, then override/add child methods
                let mut all_methods = self
                    .type_method_defs
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                for field in inh_def
                    .fields
                    .iter()
                    .filter(|f| f.is_method && f.method_def.is_some())
                {
                    all_methods.retain(|(name, _)| name != &field.name);
                    all_methods.push((field.name.clone(), field.method_def.clone().unwrap()));
                }
                if !all_methods.is_empty() {
                    self.type_method_defs
                        .insert(inh_def.child.clone(), all_methods);
                }
                // RCB-101: Register inheritance parent for error type filtering in |==
                // B11-6d: Track inheritance for TypeExtends compile-time resolution
                self.type_parents
                    .insert(inh_def.child.clone(), inh_def.parent.clone());
                let child_str_var = func.alloc_var();
                func.push(IrInst::ConstStr(child_str_var, inh_def.child.clone()));
                let parent_str_var = func.alloc_var();
                func.push(IrInst::ConstStr(parent_str_var, inh_def.parent.clone()));
                let reg_dummy = func.alloc_var();
                func.push(IrInst::Call(
                    reg_dummy,
                    "taida_register_type_parent".to_string(),
                    vec![child_str_var, parent_str_var],
                ));
                if let Some(parent_mold) = self.mold_defs.get(&inh_def.parent).cloned() {
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
                        inh_def.child.clone(),
                        crate::parser::MoldDef {
                            name: inh_def.child.clone(),
                            mold_args: parent_mold.mold_args.clone(),
                            name_args: inh_def
                                .child_args
                                .clone()
                                .or_else(|| inh_def.parent_args.clone())
                                .or(parent_mold.name_args.clone()),
                            type_params: parent_mold.type_params.clone(),
                            fields: merged_mold_fields,
                            doc_comments: inh_def.doc_comments.clone(),
                            span: inh_def.span.clone(),
                        },
                    );
                }
                Ok(())
            }
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
                // expr ]=> name : Async のアンモールド
                let source_var = self.lower_expr(func, &uf.source)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![source_var],
                ));
                func.push(IrInst::DefVar(uf.target.clone(), result));
                // Track type from mold source for debug display
                self.track_unmold_type(&uf.target, &uf.source);
                // Track local unmold-forward shadow for net builtins
                if Self::NET_BUILTIN_NAMES.contains(&uf.target.as_str()) {
                    self.shadowed_net_builtins.insert(uf.target.clone());
                }
                Ok(())
            }
            Statement::UnmoldBackward(ub) => {
                // name <=[ expr : Async のアンモールド（逆方向）
                let source_var = self.lower_expr(func, &ub.source)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![source_var],
                ));
                func.push(IrInst::DefVar(ub.target.clone(), result));
                // Track type from mold source for debug display
                self.track_unmold_type(&ub.target, &ub.source);
                // Track local unmold-backward shadow for net builtins
                if Self::NET_BUILTIN_NAMES.contains(&ub.target.as_str()) {
                    self.shadowed_net_builtins.insert(ub.target.clone());
                }
                Ok(())
            } // All statement types are now handled above.
              // This branch should not be reached.
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
            Expr::Ident(name, _) => {
                if !bound.contains(name.as_str())
                    && !self.user_funcs.contains(name)
                    && !seen.contains(name)
                {
                    seen.insert(name.clone());
                    free.push(name.clone());
                }
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
    /// C13-1: A tail binding statement (`Assignment` / `UnmoldForward` /
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
    /// C13-1: Tail-binding statements cannot be TCO'd (the value is bound
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

    /// C13-1: If `stmt` is a tail-binding statement that was just lowered
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
