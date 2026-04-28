// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups molds methods of the `Lowering` struct (placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{LowerError, Lowering, simple_hash};
use crate::codegen::ir::*;
use crate::parser::*;

impl Lowering {
    pub(super) fn mold_solidify_helper_name(mold_name: &str) -> String {
        format!("__taida_mold_solidify_{}", mold_name)
    }

    pub(super) fn register_mold_solidify_helpers(&mut self) -> Result<(), LowerError> {
        // (E30 Phase 2 Sub-step 2.1) `MoldDef` 廃止に伴い `ClassLikeDef` を保持。
        let mut mold_defs: Vec<crate::parser::ClassLikeDef> =
            self.mold_defs.values().cloned().collect();
        mold_defs.sort_by(|a, b| a.name.cmp(&b.name));

        // Register helper symbols first, so recursive mold references can resolve.
        for mold_def in &mold_defs {
            let has_solidify = mold_def
                .fields
                .iter()
                .any(|f| f.is_method && f.name == "solidify" && f.method_def.is_some());
            if has_solidify {
                let helper_raw = Self::mold_solidify_helper_name(&mold_def.name);
                let helper_symbol = format!("_taida_fn_{}", helper_raw);
                self.mold_solidify_funcs
                    .insert(mold_def.name.clone(), helper_symbol);
            }
        }

        for mold_def in mold_defs {
            let Some(solidify_method) = mold_def
                .fields
                .iter()
                .find(|f| f.is_method && f.name == "solidify")
                .and_then(|f| f.method_def.clone())
            else {
                continue;
            };

            if !solidify_method.params.is_empty() {
                return Err(LowerError {
                    message: format!(
                        "Native backend does not support solidify method parameters on mold '{}'",
                        mold_def.name
                    ),
                });
            }

            let non_method_fields: Vec<crate::parser::FieldDef> = mold_def
                .fields
                .iter()
                .filter(|f| !f.is_method)
                .cloned()
                .collect();
            let required_fields: Vec<crate::parser::FieldDef> = non_method_fields
                .iter()
                .filter(|f| f.name != "filling" && f.default_value.is_none())
                .cloned()
                .collect();
            let optional_fields: Vec<crate::parser::FieldDef> = non_method_fields
                .iter()
                .filter(|f| f.name != "filling" && f.default_value.is_some())
                .cloned()
                .collect();

            let mut params = Vec::<crate::parser::Param>::new();
            let mut seen = std::collections::HashSet::<String>::new();
            let mut push_param = |name: &str| {
                if seen.insert(name.to_string()) {
                    params.push(crate::parser::Param {
                        name: name.to_string(),
                        type_annotation: None,
                        default_value: None,
                        span: mold_def.span.clone(),
                    });
                }
            };
            push_param("filling");
            for field in &required_fields {
                push_param(&field.name);
            }
            for field in &optional_fields {
                push_param(&field.name);
            }
            push_param("self");

            let helper_raw = Self::mold_solidify_helper_name(&mold_def.name);
            let synthetic = crate::parser::FuncDef {
                name: helper_raw,
                type_params: Vec::new(),
                params,
                body: solidify_method.body.clone(),
                return_type: solidify_method.return_type.clone(),
                doc_comments: Vec::new(),
                span: mold_def.span.clone(),
            };
            let helper_ir = self.lower_func_def(&synthetic)?;
            self.lambda_funcs.push(helper_ir);
        }

        Ok(())
    }

    /// 型インスタンス化: `TypeName(field <= value, ...)`
    /// Adds __type field (like interpreter) so jsonEncode can include it.
    pub(super) fn lower_type_inst(
        &mut self,
        func: &mut IrFunction,
        type_name: &str,
        fields: &[BuchiField],
    ) -> Result<IrVar, LowerError> {
        let mut materialized_fields: Vec<(String, IrVar)> = Vec::new();

        if let Some(type_fields) = self.type_field_defs.get(type_name).cloned() {
            let mut consumed = std::collections::HashSet::new();
            let mut visiting = std::collections::HashSet::new();
            for field_def in type_fields.iter().filter(|f| !f.is_method) {
                let value_var = if let Some(provided) =
                    fields.iter().rev().find(|f| f.name == field_def.name)
                {
                    self.lower_expr(func, &provided.value)?
                } else {
                    self.lower_default_for_field_def(func, field_def, &mut visiting)?
                };
                materialized_fields.push((field_def.name.clone(), value_var));
                consumed.insert(field_def.name.clone());
            }
            // Keep undeclared fields for structural flexibility (interpreter parity).
            for field in fields {
                if !consumed.contains(&field.name) {
                    let val = self.lower_expr(func, &field.value)?;
                    materialized_fields.push((field.name.clone(), val));
                }
            }
        } else {
            for field in fields {
                let val = self.lower_expr(func, &field.value)?;
                materialized_fields.push((field.name.clone(), val));
            }
        }

        // Generate method closures that capture the data fields.
        // Each method becomes a closure with the data fields as its environment.
        let method_defs = self
            .type_method_defs
            .get(type_name)
            .cloned()
            .unwrap_or_default();
        let data_field_names: Vec<String> =
            materialized_fields.iter().map(|(n, _)| n.clone()).collect();

        // Register data field values as named variables so MakeClosure can capture them.
        // Use unique temporary names to avoid conflicts with existing variables.
        let capture_prefix = format!("__typeinst_{}_{}_", type_name, self.lambda_counter);
        let capture_names: Vec<String> = data_field_names
            .iter()
            .map(|n| format!("{}{}", capture_prefix, n))
            .collect();
        for ((_field_name, field_val), cap_name) in
            materialized_fields.iter().zip(capture_names.iter())
        {
            func.push(IrInst::DefVar(cap_name.clone(), *field_val));
        }

        let mut method_closures: Vec<(String, IrVar)> = Vec::new();
        for (method_name, method_func_def) in &method_defs {
            let closure_var = self.lower_type_method_closure(
                func,
                type_name,
                method_name,
                method_func_def,
                &capture_names,
                &data_field_names,
            )?;
            method_closures.push((method_name.clone(), closure_var));
        }

        // Create pack with slots for data fields + method closures + __type.
        let total_fields = materialized_fields.len() + method_closures.len() + 1;
        let pack_var = func.alloc_var();
        func.push(IrInst::PackNew(pack_var, total_fields));

        // Set user/defaulted fields.
        for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
            self.emit_pack_field_hash(func, pack_var, i, field_name);
            func.push(IrInst::PackSet(pack_var, i, *field_val));

            // C18B-003 fix: per-pack enum descriptor registration for
            // TypeDef-based packs. Look up the TypeDef's field type
            // annotation and, if it's a known Enum, register the CSV
            // under the current pack instance. This parallels the
            // anonymous-BuchiPack path in `lower_buchi_pack`.
            let per_pack_enum_csv = self
                .type_field_enum_name(type_name, field_name)
                .and_then(|enum_name| self.enum_defs.get(&enum_name).map(|v| v.join(",")));
            if let Some(csv) = per_pack_enum_csv {
                let hash_var = func.alloc_var();
                let field_hash = crate::codegen::lower::simple_hash(field_name);
                func.push(IrInst::ConstInt(hash_var, field_hash as i64));
                let csv_var = func.alloc_var();
                func.push(IrInst::ConstStr(csv_var, csv));
                let reg_dummy = func.alloc_var();
                func.push(IrInst::Call(
                    reg_dummy,
                    "taida_register_pack_field_enum".to_string(),
                    vec![pack_var, hash_var, csv_var],
                ));
            }

            // A-4c: determine type tag from field_type_tags registry or TypeDef field types
            let tag = self.type_field_type_tag(type_name, field_name);
            if tag != 0 {
                func.push(IrInst::PackSetTag(pack_var, i, tag));
            }
            // retain-on-store
            self.emit_retain_if_heap_tag(func, *field_val, tag);
        }

        // Set method closure fields.
        let method_offset = materialized_fields.len();
        for (i, (method_name, closure_var)) in method_closures.iter().enumerate() {
            let slot = method_offset + i;
            self.emit_pack_field_hash(func, pack_var, slot, method_name);
            func.push(IrInst::PackSet(pack_var, slot, *closure_var));
            func.push(IrInst::PackSetTag(pack_var, slot, 6)); // TAIDA_TAG_CLOSURE
            // retain-on-store: method closure
            func.push(IrInst::Retain(*closure_var));
        }

        // Set __type field at the last slot.
        let type_slot = materialized_fields.len() + method_closures.len();
        self.emit_pack_field_hash(func, pack_var, type_slot, "__type");
        let type_str_var = func.alloc_var();
        func.push(IrInst::ConstStr(type_str_var, type_name.to_string()));
        func.push(IrInst::PackSet(pack_var, type_slot, type_str_var));
        func.push(IrInst::PackSetTag(pack_var, type_slot, 3)); // TAIDA_TAG_STR

        Ok(pack_var)
    }

    /// Generate a closure for a TypeDef method.
    /// The closure captures all data fields of the instance as its environment.
    /// `capture_names` are the unique temporary variable names used for MakeClosure.
    /// `data_field_names` are the original field names restored inside the method body.
    pub(super) fn lower_type_method_closure(
        &mut self,
        func: &mut IrFunction,
        type_name: &str,
        _method_name: &str,
        method_func_def: &FuncDef,
        capture_names: &[String],
        data_field_names: &[String],
    ) -> Result<IrVar, LowerError> {
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_method_{}_{}", type_name, lambda_id);

        // The method function takes __env as the first parameter,
        // followed by the method's own parameters.
        let mut ir_params: Vec<String> = vec!["__env".to_string()];
        ir_params.extend(method_func_def.params.iter().map(|p| p.name.clone()));

        let mut method_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

        // Restore data fields from the environment pack.
        let env_var = 0u32; // __env is parameter 0
        for (i, field_name) in data_field_names.iter().enumerate() {
            let get_dst = method_fn.alloc_var();
            method_fn.push(IrInst::PackGet(get_dst, env_var, i));
            method_fn.push(IrInst::DefVar(field_name.clone(), get_dst));
        }

        // Pre-process local function definitions in the method body.
        // These need to be lowered as separate IR functions and registered
        // in user_funcs before the method body is lowered.
        for stmt in &method_func_def.body {
            if let Statement::FuncDef(inner_func_def) = stmt {
                self.user_funcs.insert(inner_func_def.name.clone());
                // Store parameter definitions for arity/default resolution
                self.func_param_defs
                    .insert(inner_func_def.name.clone(), inner_func_def.params.clone());
                let ir_func = self.lower_func_def(inner_func_def)?;
                self.lambda_funcs.push(ir_func);
            }
        }

        // Lower method body (same pattern as lower_func_def).
        let prev_heap = std::mem::take(&mut self.current_heap_vars);
        let prev_func_name = self.current_func_name.take();

        let mut last_var = None;
        let body_refs: Vec<&Statement> = method_func_def.body.iter().collect();
        let has_error_ceiling = body_refs
            .iter()
            .any(|s| matches!(s, Statement::ErrorCeiling(_)));

        if has_error_ceiling {
            self.lower_statement_sequence(&mut method_fn, &body_refs)?;
        } else {
            for (i, stmt) in method_func_def.body.iter().enumerate() {
                let is_last = i == method_func_def.body.len() - 1;
                match stmt {
                    Statement::Expr(expr) => {
                        let var = self.lower_expr(&mut method_fn, expr)?;
                        if is_last {
                            last_var = Some(var);
                        }
                    }
                    _ => {
                        self.lower_statement(&mut method_fn, stmt)?;
                    }
                }
            }
        }

        self.current_func_name = prev_func_name;
        let _heap_vars = std::mem::replace(&mut self.current_heap_vars, prev_heap);

        // Implicit return value
        if let Some(ret) = last_var {
            method_fn.push(IrInst::Return(ret));
        } else {
            let zero = method_fn.alloc_var();
            method_fn.push(IrInst::ConstInt(zero, 0));
            method_fn.push(IrInst::Return(zero));
        }

        self.user_funcs.insert(lambda_name.clone());
        self.lambda_funcs.push(method_fn);

        // Create closure: capture all data field values as environment
        let dst = func.alloc_var();
        func.push(IrInst::MakeClosure(
            dst,
            lambda_name,
            capture_names.to_vec(),
        ));
        Ok(dst)
    }

    pub(crate) fn emit_pack_field_hash(
        &mut self,
        func: &mut IrFunction,
        pack_var: IrVar,
        index: usize,
        field_name: &str,
    ) {
        self.field_names.insert(field_name.to_string());
        if field_name == "__type" {
            self.register_field_type_tag("__type", 3);
        }
        let hash = simple_hash(field_name);

        // Emit inline field registration for jsonEncode (library module support)
        let type_tag = self.field_type_tags.get(field_name).copied().unwrap_or(0);
        self.emit_field_registration_inline(func, field_name, hash, type_tag);

        let hash_var = func.alloc_var();
        func.push(IrInst::ConstInt(hash_var, hash as i64));
        let idx_var = func.alloc_var();
        func.push(IrInst::ConstInt(idx_var, index as i64));
        let result_var = func.alloc_var();
        func.push(IrInst::Call(
            result_var,
            "taida_pack_set_hash".to_string(),
            vec![pack_var, idx_var, hash_var],
        ));
    }

    /// Emit inline taida_register_field_name/taida_register_field_type calls.
    /// This ensures field names are registered at runtime even in library modules
    /// that don't have a _taida_main to batch-register field names.
    /// The C runtime's registry handles duplicates safely (skips if already registered).
    ///
    /// C18-2: `type_tag == 5` is an Enum field — we emit
    /// `taida_register_field_enum(hash, name, variants_csv)` so the
    /// runtime can look up the variant name by ordinal when serializing
    /// to JSON.
    pub(super) fn emit_field_registration_inline(
        &mut self,
        func: &mut IrFunction,
        field_name: &str,
        hash: u64,
        type_tag: i64,
    ) {
        if type_tag == 5 {
            // Enum field: need both the name and the variant CSV.
            let variants_csv = self
                .field_enum_descriptors
                .get(field_name)
                .cloned()
                .unwrap_or_default();
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let name_var = func.alloc_var();
            func.push(IrInst::ConstStr(name_var, field_name.to_string()));
            let variants_var = func.alloc_var();
            func.push(IrInst::ConstStr(variants_var, variants_csv));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_register_field_enum".to_string(),
                vec![hash_var, name_var, variants_var],
            ));
        } else if type_tag > 0 {
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let name_var = func.alloc_var();
            func.push(IrInst::ConstStr(name_var, field_name.to_string()));
            let tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(tag_var, type_tag));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_register_field_type".to_string(),
                vec![hash_var, name_var, tag_var],
            ));
        } else {
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let name_var = func.alloc_var();
            func.push(IrInst::ConstStr(name_var, field_name.to_string()));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_register_field_name".to_string(),
                vec![hash_var, name_var],
            ));
        }
    }

    pub(super) fn lower_default_for_field_def(
        &mut self,
        func: &mut IrFunction,
        field_def: &FieldDef,
        visiting: &mut std::collections::HashSet<String>,
    ) -> Result<IrVar, LowerError> {
        if let Some(default_expr) = &field_def.default_value {
            return self.lower_expr(func, default_expr);
        }
        if let Some(type_expr) = &field_def.type_annotation {
            return self.lower_default_for_type_expr(func, type_expr, visiting);
        }
        let zero = func.alloc_var();
        func.push(IrInst::ConstInt(zero, 0));
        Ok(zero)
    }

    pub(super) fn lower_default_for_type_expr(
        &mut self,
        func: &mut IrFunction,
        type_expr: &TypeExpr,
        visiting: &mut std::collections::HashSet<String>,
    ) -> Result<IrVar, LowerError> {
        match type_expr {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" | "Num" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 0));
                    Ok(v)
                }
                "Float" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstFloat(v, 0.0));
                    Ok(v)
                }
                "Str" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstStr(v, String::new()));
                    Ok(v)
                }
                "Bool" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstBool(v, false));
                    Ok(v)
                }
                _ => {
                    if visiting.contains(name) {
                        let pack_var = func.alloc_var();
                        func.push(IrInst::PackNew(pack_var, 1));
                        self.emit_pack_field_hash(func, pack_var, 0, "__type");
                        let type_var = func.alloc_var();
                        func.push(IrInst::ConstStr(type_var, name.clone()));
                        func.push(IrInst::PackSet(pack_var, 0, type_var));
                        func.push(IrInst::PackSetTag(pack_var, 0, 3)); // TAIDA_TAG_STR
                        return Ok(pack_var);
                    }
                    if let Some(type_fields) = self.type_field_defs.get(name).cloned() {
                        visiting.insert(name.clone());
                        let mut materialized_fields: Vec<(String, IrVar)> = Vec::new();
                        for field in type_fields.iter().filter(|f| !f.is_method) {
                            let val = self.lower_default_for_field_def(func, field, visiting)?;
                            materialized_fields.push((field.name.clone(), val));
                        }
                        visiting.remove(name);

                        let pack_var = func.alloc_var();
                        func.push(IrInst::PackNew(pack_var, materialized_fields.len() + 1));
                        for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
                            self.emit_pack_field_hash(func, pack_var, i, field_name);
                            func.push(IrInst::PackSet(pack_var, i, *field_val));
                            // A-4c: Type tag for default fields (based on TypeDef field types)
                            let tag = self.type_field_type_tag(name, field_name);
                            if tag != 0 {
                                func.push(IrInst::PackSetTag(pack_var, i, tag));
                            }
                            // retain-on-store
                            self.emit_retain_if_heap_tag(func, *field_val, tag);
                        }
                        self.emit_pack_field_hash(
                            func,
                            pack_var,
                            materialized_fields.len(),
                            "__type",
                        );
                        let type_var = func.alloc_var();
                        func.push(IrInst::ConstStr(type_var, name.clone()));
                        func.push(IrInst::PackSet(
                            pack_var,
                            materialized_fields.len(),
                            type_var,
                        ));
                        func.push(IrInst::PackSetTag(pack_var, materialized_fields.len(), 3)); // TAIDA_TAG_STR
                        return Ok(pack_var);
                    }

                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    Ok(zero)
                }
            },
            TypeExpr::List(_) => {
                let list = func.alloc_var();
                func.push(IrInst::Call(list, "taida_list_new".to_string(), vec![]));
                Ok(list)
            }
            TypeExpr::BuchiPack(fields) => {
                let mut materialized_fields: Vec<(String, IrVar)> = Vec::new();
                for field in fields.iter().filter(|f| !f.is_method) {
                    let val = self.lower_default_for_field_def(func, field, visiting)?;
                    materialized_fields.push((field.name.clone(), val));
                }
                let pack_var = func.alloc_var();
                func.push(IrInst::PackNew(pack_var, materialized_fields.len()));
                for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
                    self.emit_pack_field_hash(func, pack_var, i, field_name);
                    func.push(IrInst::PackSet(pack_var, i, *field_val));
                }
                Ok(pack_var)
            }
            TypeExpr::Generic(name, args) if name == "Lax" => {
                let inner = if let Some(inner_ty) = args.first() {
                    self.lower_default_for_type_expr(func, inner_ty, visiting)?
                } else {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    zero
                };
                let pack_var = func.alloc_var();
                func.push(IrInst::PackNew(pack_var, 4));

                self.emit_pack_field_hash(func, pack_var, 0, "hasValue");
                let has_value = func.alloc_var();
                func.push(IrInst::ConstBool(has_value, false));
                func.push(IrInst::PackSet(pack_var, 0, has_value));
                func.push(IrInst::PackSetTag(pack_var, 0, 2)); // TAIDA_TAG_BOOL

                self.emit_pack_field_hash(func, pack_var, 1, "__value");
                func.push(IrInst::PackSet(pack_var, 1, inner));

                self.emit_pack_field_hash(func, pack_var, 2, "__default");
                func.push(IrInst::PackSet(pack_var, 2, inner));

                self.emit_pack_field_hash(func, pack_var, 3, "__type");
                let lax_type = func.alloc_var();
                func.push(IrInst::ConstStr(lax_type, "Lax".to_string()));
                func.push(IrInst::PackSet(pack_var, 3, lax_type));
                func.push(IrInst::PackSetTag(pack_var, 3, 3)); // TAIDA_TAG_STR
                Ok(pack_var)
            }
            TypeExpr::Generic(_, _) => {
                let zero = func.alloc_var();
                func.push(IrInst::ConstInt(zero, 0));
                Ok(zero)
            }
            // E30 Phase 6 / E30B-004 (Lock-D verdict): synthetic defaultFn
            // for declare-only function fields. Emit a lambda function that,
            // when called, returns the **return type's default value**.
            //
            // The lambda takes `params.len()` ignored arguments (matching the
            // declared type arity) and immediately returns the lowered default
            // value of `ret`. `MakeClosure` produces a closure value carrying
            // no captures, so calling the field at runtime materialises the
            // expected type-default — matching the interpreter's
            // `DEFAULT_FN_SENTINEL_NAME` synthetic FuncValue and the JS
            // backend's `__taida_defaultForSchema({ __fn: ... })` arrow
            // function.
            //
            // PHILOSOPHY: completes "全型にデフォルト値保証 / null/undefined
            // 排除" — function-typed fields no longer fall back to the
            // crashing `ConstInt(0)` placeholder; they yield a callable
            // closure whose result is the proper type-default.
            TypeExpr::Function(params, ret) => {
                self.lower_default_fn_synthetic(func, params, ret, visiting)
            }
        }
    }

    /// E30 Phase 6 / E30B-004: Emit a synthetic defaultFn lambda whose body
    /// returns the lowered default value of `ret`. Returns the IR var of the
    /// resulting closure (boxed value).
    fn lower_default_fn_synthetic(
        &mut self,
        _outer: &mut IrFunction,
        params: &[TypeExpr],
        ret: &TypeExpr,
        visiting: &mut std::collections::HashSet<String>,
    ) -> Result<IrVar, LowerError> {
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_default_fn_{}", lambda_id);

        // Synthetic params: __env (closure env, unused) + one slot per
        // declared type-arg (also unused inside the body).
        let mut ir_params: Vec<String> = vec!["__env".to_string()];
        for i in 0..params.len() {
            ir_params.push(format!("__df_arg_{}", i));
        }

        let mut lambda_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

        // Body: materialise the return-type default and return it.
        let result = self.lower_default_for_type_expr(&mut lambda_fn, ret, visiting)?;
        lambda_fn.push(IrInst::Return(result));

        self.user_funcs.insert(lambda_name.clone());
        self.lambda_funcs.push(lambda_fn);

        // Emit MakeClosure in the outer function. Captures: empty (synthetic
        // defaultFn does not close over any caller-scope variable).
        let closure_var = _outer.alloc_var();
        _outer.push(IrInst::MakeClosure(closure_var, lambda_name, Vec::new()));
        Ok(closure_var)
    }
}
