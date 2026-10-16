use super::{LowerError, Lowering, simple_hash};
use crate::codegen::ir::*;
/// Method call lowering for the Taida native backend.
///
/// Contains `lower_method_call` and polymorphic dispatch helpers.
///
/// These are `impl Lowering` methods split from lower.rs for maintainability.
use crate::parser::*;

impl Lowering {
    /// メソッド呼び出し: `expr.method(args)`
    /// 標準メソッドをランタイム関数に変換
    pub(crate) fn lower_method_call(
        &mut self,
        func: &mut IrFunction,
        obj: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if method == "toString" {
            self.reject_untyped_addon_value(func, obj)?;
        }
        // A method the receiver's own type declares wins over a built-in
        // method of the same name (`length`, `first`, `add`, ...): the type's
        // methods and function fields live in the value itself.
        if self.receiver_declares_member(obj, method) {
            return self.lower_pack_field_call(func, obj, method, args);
        }

        // F-58: If the receiver is known to be a BuchiPack/TypeInst and the method
        // name conflicts with a built-in method (get, set, has, keys, values, etc.),
        // dispatch to BuchiPack field call instead of the built-in method.
        if self.expr_is_pack(obj) {
            let is_conflicting = matches!(
                method,
                "get"
                    | "set"
                    | "has"
                    | "keys"
                    | "values"
                    | "entries"
                    | "size"
                    | "remove"
                    | "merge"
                    | "add"
                    | "union"
                    | "intersect"
                    | "diff"
                    | "toList"
            );
            if is_conflicting {
                return self.lower_pack_field_call(func, obj, method, args);
            }
        }

        // Direct-form fusion: `Mold[...]().unmold()` constructs its
        // receiver solely for this call — the same single-reference
        // guarantee as `Mold[...]() >=> v` — so when the result is
        // statically known the Lax is never materialised. This is the
        // form lambda bodies use, where the materialised Lax otherwise
        // dominates HOF loops.
        if method == "unmold"
            && args.is_empty()
            && let Some(result) = self.try_lower_fused_unmold(func, obj)?
        {
            return Ok(result);
        }

        let obj_var = self.lower_expr(func, obj)?;

        let mut arg_vars = vec![obj_var];
        for arg in args {
            let var = self.lower_expr(func, arg)?;
            arg_vars.push(var);
        }

        // 標準メソッドをランタイム関数にマッピング
        let runtime_fn = match method {
            // Polymorphic methods (work on List + Result/Lax)
            "map" => {
                return self.lower_polymorphic_map(func, obj_var, args, &arg_vars[1..]);
            }
            "flatMap" => {
                return self.lower_monadic_hof(
                    func,
                    obj_var,
                    args,
                    &arg_vars[1..],
                    "taida_monadic_flat_map",
                );
            }
            "mapError" => {
                return self.lower_monadic_hof(
                    func,
                    obj_var,
                    args,
                    &arg_vars[1..],
                    "taida_result_map_error",
                );
            }
            "isEmpty" => "taida_polymorphic_is_empty",
            "toString" | "toStr" => {
                if self.expr_is_likely_bool(obj) {
                    "taida_str_from_bool"
                } else if self.expr_is_string_full(obj) {
                    // Str.toString() is identity — just return obj directly
                    return Ok(obj_var);
                } else if self.expr_returns_float(obj) {
                    "taida_float_to_str"
                } else if self.checked_tag(obj) == Some(crate::codegen::tag_prop::TAG_INT) {
                    // An Int is written as a number, whatever its value.
                    "taida_str_from_int"
                } else {
                    "taida_polymorphic_to_string"
                }
            }
            // Lax/Gorillax methods (polymorphic dispatch)
            "hasValue" => "taida_polymorphic_has_value",
            "getOrDefault" => "taida_polymorphic_get_or_default",
            "getOrThrow" => "taida_monadic_get_or_throw",
            "errorInfo" => "taida_error_info",
            // Gorillax.relax() → RelaxedGorillax
            "relax" => "taida_gorillax_relax",
            // Result methods
            "isOk" | "isSuccess" => "taida_result_is_ok",
            "isError" => "taida_result_is_error",
            // Int/Number methods
            "abs" => "taida_int_abs",
            "toFloat" => "taida_int_to_float",
            "clamp" => "taida_int_clamp",
            // Float methods
            "floor" => "taida_float_floor",
            "ceil" => "taida_float_ceil",
            "round" => "taida_float_round",
            "toFixed" => "taida_float_to_fixed",
            "toInt" => "taida_float_to_int",
            // Num state check methods (Int vs Float dispatch)
            "isNaN" => {
                if self.expr_returns_float(obj) {
                    "taida_float_is_nan"
                } else {
                    // Int is never NaN — return constant 0
                    let result = func.alloc_var();
                    func.push(IrInst::ConstInt(result, 0));
                    return Ok(result);
                }
            }
            "isInfinite" => {
                if self.expr_returns_float(obj) {
                    "taida_float_is_infinite"
                } else {
                    let result = func.alloc_var();
                    func.push(IrInst::ConstInt(result, 0));
                    return Ok(result);
                }
            }
            "isFinite" => {
                if self.expr_returns_float(obj) {
                    "taida_float_is_finite_check"
                } else {
                    // Int is always finite
                    let result = func.alloc_var();
                    func.push(IrInst::ConstInt(result, 1));
                    return Ok(result);
                }
            }
            "isPositive" => {
                if self.expr_returns_float(obj) {
                    "taida_float_is_positive"
                } else {
                    "taida_int_is_positive"
                }
            }
            "isNegative" => {
                if self.expr_returns_float(obj) {
                    "taida_float_is_negative"
                } else {
                    "taida_int_is_negative"
                }
            }
            "isZero" => {
                if self.expr_returns_float(obj) {
                    "taida_float_is_zero"
                } else {
                    "taida_int_is_zero"
                }
            }
            // Polymorphic length (works on Str, List, Set, HashMap)
            "length" | "len" => "taida_polymorphic_length",
            // Str methods
            "toUpperCase" => "taida_str_to_upper",
            "toLowerCase" => "taida_str_to_lower",
            "trim" => "taida_str_trim",
            // C12-6c: runtime-polymorphic dispatch — inspect whether
            // the target arg is a Str or a :Regex BuchiPack and
            // dispatch accordingly inside the C helper. Fixed-string
            // callers pay one tag inspection; Regex callers use
            // POSIX regex.h-backed implementations.
            "split" => "taida_str_split_poly",
            "replace" => "taida_str_replace_first_poly",
            "replaceAll" => "taida_str_replace_poly",
            // C12-6c: match / search — Regex-only. The C helper
            // assumes the 2nd arg is a Regex pack; if not, it
            // returns an empty match / -1 (mirroring a TypeError at
            // runtime, but since the checker wasn't strict enough,
            // we err on the side of a well-defined empty result).
            "match" => "taida_str_match_regex",
            "search" => "taida_str_search_regex",
            // E32B-022 (Lock-N): Lax[Int]-returning sibling of `search`.
            "searchLax" => "taida_str_search_regex_lax",
            "slice" => {
                if self.expr_is_string_full(obj) {
                    "taida_str_slice"
                } else {
                    return Err(LowerError {
                        message: "list method .slice() has moved to Slice[] mold.".to_string(),
                    });
                }
            }
            "charAt" => "taida_str_char_at",
            "repeat" => "taida_str_repeat",
            "startsWith" => "taida_str_starts_with",
            "endsWith" => "taida_str_ends_with",
            // Polymorphic at runtime: Str.contains(substr) / List.contains(val).
            // Field access in lambdas can lose static string provenance in lowering,
            // so dispatch dynamically to preserve backend parity.
            "contains" => "taida_polymorphic_contains",
            // Polymorphic: Str.indexOf(substr) vs List.indexOf(val)
            "indexOf" => "taida_polymorphic_index_of",
            // Polymorphic: Str.lastIndexOf(substr) vs List.lastIndexOf(val)
            "lastIndexOf" => "taida_polymorphic_last_index_of",
            // E32B-022 (Lock-N): Lax[Int]-returning siblings (no `-1` sentinel).
            "indexOfLax" => "taida_polymorphic_index_of_lax",
            "lastIndexOfLax" => "taida_polymorphic_last_index_of_lax",
            // List methods
            "first" => "taida_list_first",
            "last" => "taida_list_last",
            "get" => {
                // Polymorphic: Str.get(idx) -> Lax, list.get(idx) -> Lax, HashMap.get(key) -> Lax
                if self.expr_is_string_full(obj) {
                    // Str.get(idx) — compile-time dispatch
                    if args.len() != 1 {
                        return Err(LowerError {
                            message: ".get() requires exactly 1 argument".to_string(),
                        });
                    }
                    let arg_var = arg_vars[1];
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_str_get".to_string(),
                        vec![obj_var, arg_var],
                    ));
                    return Ok(result);
                }
                return self.lower_polymorphic_get(func, obj_var, args, &arg_vars[1..]);
            }
            "take" | "drop" | "push" | "append" | "sum" | "reverse" | "concat" | "join"
            | "sort" | "unique" | "flatten" | "zip" => {
                return Err(LowerError {
                    message: format!(
                        "list method .{}() has moved to molds. Use the corresponding mold (e.g. Join[], Sum[], Reverse[], Sort[]).",
                        method
                    ),
                });
            }
            "max" => "taida_list_max",
            "min" => "taida_list_min",
            "filter" => {
                return Err(LowerError {
                    message: "list method .filter() has moved to Filter[] mold.".to_string(),
                });
            }
            // List predicate methods (any/all/none take lambda)
            "any" => {
                return self.lower_list_predicate(
                    func,
                    obj_var,
                    args,
                    &arg_vars[1..],
                    "taida_list_any",
                );
            }
            "all" => {
                return self.lower_list_predicate(
                    func,
                    obj_var,
                    args,
                    &arg_vars[1..],
                    "taida_list_all",
                );
            }
            "none" => {
                return self.lower_list_predicate(
                    func,
                    obj_var,
                    args,
                    &arg_vars[1..],
                    "taida_list_none",
                );
            }
            "fold" | "reduce" => {
                return self.lower_list_fold(
                    func,
                    obj_var,
                    args,
                    &arg_vars[1..],
                    "taida_list_fold",
                );
            }
            // Async methods
            "isPending" => "taida_async_is_pending",
            "isFulfilled" => "taida_async_is_fulfilled",
            "isRejected" => "taida_async_is_rejected",
            "unmold" => "taida_generic_unmold",
            "remove" if self.expr_is_list(obj) => {
                return Err(LowerError {
                    message: "list method .remove() is not supported.".to_string(),
                });
            }
            // HashMap methods (immutable semantics)
            "set" => {
                return self.lower_hashmap_set(func, obj_var, args, &arg_vars[1..]);
            }
            "remove" => {
                return self.lower_hashmap_remove(func, obj_var, args, &arg_vars[1..]);
            }
            "has" => {
                return self.lower_hashmap_has(func, obj_var, args, &arg_vars[1..]);
            }
            "keys" => "taida_hashmap_keys",
            "values" => "taida_hashmap_values",
            "entries" => "taida_hashmap_entries",
            "size" => "taida_collection_size",
            "merge" => "taida_hashmap_merge",
            // Set methods
            // QF-16: Set.add() は引数1個。引数数が一致しない場合は BuchiPack フィールド呼び出しに
            // フォールバック（モジュールからインポートした BuchiPack の .add() フィールドとの衝突回避）
            "add" if args.len() == 1 => {
                return self.lower_set_add(func, obj_var, args, &arg_vars[1..]);
            }
            "union" => "taida_set_union",
            "intersect" => "taida_set_intersect",
            "diff" => "taida_set_diff",
            "toList" => "taida_set_to_list",
            _ => {
                // Fallback: treat as BuchiPack field function call.
                // obj.method(args) → taida_pack_call_fieldN(obj, hash(method), args...)
                // The receiver and arguments are already evaluated.
                return self.lower_pack_field_call_vars(func, method, &arg_vars);
            }
        };

        let result = func.alloc_var();
        func.push(IrInst::Call(result, runtime_fn.to_string(), arg_vars));
        Ok(result)
    }

    /// list.any(fn) / list.all(fn) / list.none(fn) -- predicate checks
    fn lower_list_predicate(
        &mut self,
        func: &mut IrFunction,
        list_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
        runtime_fn: &str,
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: format!(".{}() requires exactly 1 argument", runtime_fn),
            });
        }
        let fn_var = arg_vars[0];
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![list_var, fn_var],
        ));
        Ok(result)
    }

    /// list.fold(init, fn) / list.reduce(init, fn)
    fn lower_list_fold(
        &mut self,
        func: &mut IrFunction,
        list_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
        runtime_fn: &str,
    ) -> Result<IrVar, LowerError> {
        if args.len() != 2 {
            return Err(LowerError {
                message: format!(".{}() requires exactly 2 arguments", runtime_fn),
            });
        }
        let init_var = arg_vars[0];
        let fn_var = arg_vars[1];
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![list_var, init_var, fn_var],
        ));
        Ok(result)
    }

    /// Polymorphic.map() -- dispatches to list_map or monadic_map at runtime
    fn lower_polymorphic_map(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".map() requires exactly 1 argument".to_string(),
            });
        }
        let fn_var = arg_vars[0];
        let result = func.alloc_var();
        // Use polymorphic runtime dispatch (detects list vs Result/Lax)
        func.push(IrInst::Call(
            result,
            "taida_polymorphic_map".to_string(),
            vec![obj_var, fn_var],
        ));
        Ok(result)
    }

    /// Monadic HOF (flatMap, mapError) -- pass obj + fn_ptr to runtime
    fn lower_monadic_hof(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
        runtime_fn: &str,
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: format!(".{}() requires exactly 1 argument", runtime_fn),
            });
        }
        let fn_var = arg_vars[0];
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![obj_var, fn_var],
        ));
        Ok(result)
    }

    /// Polymorphic.get() -- Str.get(idx), list.get(idx), or HashMap.get(key)
    fn lower_polymorphic_get(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".get() requires exactly 1 argument".to_string(),
            });
        }
        let arg_var = arg_vars[0];
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_collection_get".to_string(),
            vec![obj_var, arg_var],
        ));
        Ok(result)
    }

    /// HashMap.set(key, value) -- compute key hash, then call taida_hashmap_set_immut
    /// NO-1: Also sets value_type_tag on the HashMap based on the value expression type.
    fn lower_hashmap_set(
        &mut self,
        func: &mut IrFunction,
        hm_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 2 {
            return Err(LowerError {
                message: ".set() requires exactly 2 arguments (key, value)".to_string(),
            });
        }
        let key_var = arg_vars[0];
        let value_var = arg_vars[1];
        // NO-1: Set value_type_tag on the HashMap before storing.
        // This is idempotent — subsequent .set() calls will overwrite with the same tag.
        let val_tag = self.expr_type_tag(&args[1]);
        let tag_const = func.alloc_var();
        func.push(IrInst::ConstInt(tag_const, val_tag));
        let tag_dummy = func.alloc_var();
        func.push(IrInst::Call(
            tag_dummy,
            "taida_hashmap_set_value_tag".to_string(),
            vec![hm_var, tag_const],
        ));
        // Compute key hash at runtime using polymorphic value hash
        let key_hash = func.alloc_var();
        func.push(IrInst::Call(
            key_hash,
            "taida_value_hash".to_string(),
            vec![key_var],
        ));
        // Call taida_hashmap_set_immut(hm, key_hash, key_ptr, value)
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_hashmap_set_immut".to_string(),
            vec![hm_var, key_hash, key_var, value_var],
        ));
        // The entry keeps the static kinds of its key and value, so a Bool
        // or Float key and each value of a mixed map show as their kinds,
        // and an Enum value is named in jsonEncode.
        if let Some(enum_name) = self.expr_enum_type_name(&args[1])
            && let Some(id) = self.enum_type_ids.get(&enum_name).copied()
            && let Some(variants) = self.enum_defs.get(&enum_name).cloned()
        {
            let id_var = func.alloc_var();
            func.push(IrInst::ConstInt(id_var, id));
            let names_var = func.alloc_var();
            func.push(IrInst::ConstStr(names_var, variants.join(",")));
            let dummy = func.alloc_var();
            func.push(IrInst::Call(
                dummy,
                "taida_register_enum_variants".to_string(),
                vec![id_var, names_var],
            ));
        }
        let key_ek = self.emit_operand_ekind(func, &args[0]);
        let value_ek = self.emit_operand_ekind(func, &args[1]);
        let dummy = func.alloc_var();
        func.push(IrInst::Call(
            dummy,
            "taida_hashmap_note_kinds".to_string(),
            vec![result, key_hash, key_var, key_ek, value_ek],
        ));
        Ok(result)
    }

    /// `hashMap(src)`: the runtime builds the map from a list of
    /// `@(key, value)` packs or from a pack's fields, keeping the kinds the
    /// keys and values carry. The entries of a list share its element type,
    /// so the key and value kinds it fixes are passed along (an Enum's with
    /// its type id, which a value's own tag does not carry); the entries of
    /// a pack's Enum fields have their kinds noted after it is built.
    pub(super) fn lower_hashmap_from(
        &mut self,
        func: &mut IrFunction,
        src: &Expr,
    ) -> Result<IrVar, LowerError> {
        use crate::types::Type;
        let src_var = self.lower_expr(func, src)?;
        let src_ty = self.typed_expr_table.lookup(src).cloned();
        let (key_ty, value_ty) = match &src_ty {
            Some(Type::List(elem)) => match elem.as_ref() {
                Type::BuchiPack(fields) => {
                    let named = |name: &str| {
                        fields
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, ty)| ty.clone())
                    };
                    match (named("key"), named("value")) {
                        (Some(key), Some(value)) => (Some(key), Some(value)),
                        _ if fields.len() >= 2 => {
                            (Some(fields[0].1.clone()), Some(fields[1].1.clone()))
                        }
                        _ => (None, None),
                    }
                }
                Type::List(inner) => (Some((**inner).clone()), Some((**inner).clone())),
                _ => (None, None),
            },
            _ => (None, None),
        };
        let key_ek = self.static_entry_ekind(func, key_ty.as_ref());
        let value_ek = self.static_entry_ekind(func, value_ty.as_ref());
        let key_ek_var = func.alloc_var();
        func.push(IrInst::ConstInt(key_ek_var, key_ek));
        let value_ek_var = func.alloc_var();
        func.push(IrInst::ConstInt(value_ek_var, value_ek));
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_hashmap_from".to_string(),
            vec![src_var, key_ek_var, value_ek_var],
        ));
        let enum_fields: Vec<(String, String)> = match &src_ty {
            Some(Type::BuchiPack(fields)) => fields
                .iter()
                .filter_map(|(name, ty)| match ty {
                    Type::Named(enum_name) if self.enum_defs.contains_key(enum_name) => {
                        Some((name.clone(), enum_name.clone()))
                    }
                    _ => None,
                })
                .collect(),
            Some(Type::Named(type_name)) => self
                .type_field_types
                .get(type_name)
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|(name, _)| {
                            self.type_field_enum_name(type_name, name)
                                .map(|enum_name| (name.clone(), enum_name))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for (field, enum_name) in enum_fields {
            if field.starts_with("__") {
                continue;
            }
            let value_ek = self.static_entry_ekind(func, Some(&Type::Named(enum_name)));
            let key_var = func.alloc_var();
            func.push(IrInst::ConstStr(key_var, field));
            let key_hash = func.alloc_var();
            func.push(IrInst::Call(
                key_hash,
                "taida_value_hash".to_string(),
                vec![key_var],
            ));
            let key_ek_var = func.alloc_var();
            func.push(IrInst::ConstInt(
                key_ek_var,
                crate::codegen::tag_prop::TAG_STR,
            ));
            let value_ek_var = func.alloc_var();
            func.push(IrInst::ConstInt(value_ek_var, value_ek));
            let dummy = func.alloc_var();
            func.push(IrInst::Call(
                dummy,
                "taida_hashmap_note_kinds".to_string(),
                vec![result, key_hash, key_var, key_ek_var, value_ek_var],
            ));
        }
        Ok(result)
    }

    /// The entry kind (EKIND) a key or value of static type `ty` has: an
    /// Enum's kind with its type id (registering the Enum's value names
    /// for jsonEncode), a primitive's kind, or unknown (0xFF) when the
    /// value's own tag decides.
    fn static_entry_ekind(
        &mut self,
        func: &mut IrFunction,
        ty: Option<&crate::types::Type>,
    ) -> i64 {
        use crate::types::Type;
        match ty {
            Some(Type::Named(name)) => {
                let (Some(id), Some(variants)) = (
                    self.enum_type_ids.get(name).copied(),
                    self.enum_defs.get(name).cloned(),
                ) else {
                    return 0xFF;
                };
                let id_var = func.alloc_var();
                func.push(IrInst::ConstInt(id_var, id));
                let names_var = func.alloc_var();
                func.push(IrInst::ConstStr(names_var, variants.join(",")));
                let dummy = func.alloc_var();
                func.push(IrInst::Call(
                    dummy,
                    "taida_register_enum_variants".to_string(),
                    vec![id_var, names_var],
                ));
                9 | (id << 8)
            }
            Some(ty @ (Type::Int | Type::Float | Type::Bool | Type::Str)) => {
                self.type_runtime_tag(ty).unwrap_or(0xFF)
            }
            _ => 0xFF,
        }
    }

    /// HashMap.remove(key) -- compute key hash, then call taida_hashmap_remove_immut
    fn lower_hashmap_remove(
        &mut self,
        func: &mut IrFunction,
        hm_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".remove() requires exactly 1 argument".to_string(),
            });
        }
        let key_var = arg_vars[0];
        // For HashMap: compute key hash, then call taida_collection_remove
        // taida_collection_remove auto-detects HashMap vs Set.
        // Value-tag track: the probe's kind rides along so Set removal
        // follows the same pair semantics as has/add (HashMaps ignore it).
        // Without this, removing Int 1 from setOf(@[1.0, 2]) missed the
        // Float under the interpreter's crossing. The kind comes from the
        // unmold shadow variable when the probe carries a runtime kind
        // (`xs.get(i) >=> a`), falling back to the static EKIND constant.
        let ek_var = self.emit_operand_ekind(func, &args[0]);
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_collection_remove_tagged".to_string(),
            vec![hm_var, key_var, ek_var],
        ));
        Ok(result)
    }

    /// HashMap.has(key) / Set.has(item) -- polymorphic collection has.
    /// Value-tag track: the probe argument's static kind (EKIND entry —
    /// kind | enum-type-id<<8) rides along so Set membership follows the
    /// same pair semantics as construction-time dedup. HashMaps ignore
    /// the kind (key hashing is a separate system).
    fn lower_hashmap_has(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".has() requires exactly 1 argument".to_string(),
            });
        }
        let item_var = arg_vars[0];
        // The probe kind honours the unmold shadow variable (runtime kind)
        // before the static EKIND constant — see lower_hashmap_remove.
        let ek_var = self.emit_operand_ekind(func, &args[0]);
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_collection_has_tagged".to_string(),
            vec![obj_var, item_var, ek_var],
        ));
        Ok(result)
    }

    /// Set.add(item) -- add item to set.
    /// Value-tag track: the inserted item's static kind rides along and
    /// the runtime projects the result Set's per-element kinds itself, so
    /// the legacy pre-add `taida_set_set_elem_tag` stamp on the INPUT set
    /// is gone (it mutated the input's tag latch; the tagged add records
    /// the kind on the result where it belongs).
    fn lower_set_add(
        &mut self,
        func: &mut IrFunction,
        set_var: IrVar,
        args: &[Expr],
        arg_vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".add() requires exactly 1 argument".to_string(),
            });
        }
        let item_var = arg_vars[0];
        // The inserted item's kind honours the unmold shadow variable
        // (runtime kind) before the static EKIND constant — see
        // lower_hashmap_remove.
        let ek_var = self.emit_operand_ekind(func, &args[0]);
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_set_add_tagged".to_string(),
            vec![set_var, item_var, ek_var],
        ));
        Ok(result)
    }

    /// Whether the checker's type of `obj` is a pack type declaring a method
    /// or field named `member`.
    fn receiver_declares_member(&self, obj: &Expr, member: &str) -> bool {
        use crate::types::Type;
        match self.typed_expr_table.lookup(obj) {
            Some(Type::Named(name)) => {
                self.type_method_defs
                    .get(name)
                    .is_some_and(|methods| methods.iter().any(|(m, _)| m == member))
                    || self
                        .type_fields
                        .get(name)
                        .is_some_and(|fields| fields.iter().any(|f| f == member))
            }
            Some(Type::BuchiPack(fields)) => fields.iter().any(|(f, _)| f == member),
            _ => false,
        }
    }

    /// BuchiPack field function call: `obj.field(args)`
    /// Gets the field value from the BuchiPack by hash, then invokes it as a function.
    /// Supports 0-3 arguments. The runtime handles both plain function pointers and closures.
    fn lower_pack_field_call(
        &mut self,
        func: &mut IrFunction,
        obj: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.len() > 3 {
            return Err(LowerError {
                message: format!(
                    "BuchiPack field call .{}() supports up to 3 arguments, got {}",
                    method,
                    args.len()
                ),
            });
        }

        let mut vars = vec![self.lower_expr(func, obj)?];
        for arg in args {
            vars.push(self.lower_expr(func, arg)?);
        }
        self.lower_pack_field_call_vars(func, method, &vars)
    }

    /// [`Self::lower_pack_field_call`] with the receiver (`vars[0]`) and the
    /// arguments already evaluated.
    fn lower_pack_field_call_vars(
        &mut self,
        func: &mut IrFunction,
        method: &str,
        vars: &[IrVar],
    ) -> Result<IrVar, LowerError> {
        let argc = vars.len() - 1;
        if argc > 3 {
            return Err(LowerError {
                message: format!(
                    "BuchiPack field call .{}() supports up to 3 arguments, got {}",
                    method, argc
                ),
            });
        }

        // Compute field name hash (FNV-1a, same as taida_pack_get uses)
        let field_hash = simple_hash(method);
        let hash_var = func.alloc_var();
        func.push(IrInst::ConstInt(hash_var, field_hash as i64));

        // Build call args: (pack_ptr, field_hash, arg0, arg1, ...)
        let mut call_args = vec![vars[0], hash_var];
        call_args.extend_from_slice(&vars[1..]);

        let runtime_fn = format!("taida_pack_call_field{}", argc);
        let result = func.alloc_var();
        func.push(IrInst::Call(result, runtime_fn, call_args));

        // E30 Phase 8 / E30B-011: capture the return tag set by the
        // invoked callback (closure or function pointer) so that
        // downstream tag-aware operations (`stdout(_with_tag)`,
        // `.toString()` polymorphic dispatch) can render the value
        // correctly. Without this, a `Bool`-returning declare-only
        // function field's defaultFn closure would set the thread-local
        // tag inside the synthetic lambda but the caller would never
        // read it, so `b.toString()` would observe a raw i64 (0) and
        // print "0" instead of "false".
        let return_tag = func.alloc_var();
        func.push(IrInst::Call(
            return_tag,
            "taida_get_return_tag".to_string(),
            vec![],
        ));
        self.record_call_return_tag(result, return_tag);

        Ok(result)
    }
}
