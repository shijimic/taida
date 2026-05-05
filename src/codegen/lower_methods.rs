use super::ir::*;
use super::lower::{LowerError, Lowering, simple_hash};
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
                return self.lower_polymorphic_map(func, obj_var, args);
            }
            "flatMap" => {
                return self.lower_monadic_hof(func, obj_var, args, "taida_monadic_flat_map");
            }
            "mapError" => {
                return self.lower_monadic_hof(func, obj_var, args, "taida_result_map_error");
            }
            "isEmpty" => "taida_polymorphic_is_empty",
            "toString" | "toStr" => {
                if self.expr_is_bool(obj) {
                    "taida_str_from_bool"
                } else if self.expr_is_string_full(obj) {
                    // Str.toString() is identity — just return obj directly
                    return Ok(obj_var);
                } else if self.expr_returns_float(obj) {
                    "taida_float_to_str"
                } else {
                    "taida_polymorphic_to_string"
                }
            }
            // Lax/Gorillax methods (polymorphic dispatch)
            "hasValue" => "taida_polymorphic_has_value",
            "getOrDefault" => "taida_polymorphic_get_or_default",
            "getOrThrow" => "taida_monadic_get_or_throw",
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
            "slice" => "taida_str_slice",
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
                    let arg_var = self.lower_expr(func, &args[0])?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_str_get".to_string(),
                        vec![obj_var, arg_var],
                    ));
                    return Ok(result);
                }
                return self.lower_polymorphic_get(func, obj_var, args);
            }
            "push" | "sum" | "reverse" | "concat" | "join" | "sort" | "unique" | "flatten" => {
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
                return self.lower_list_predicate(func, obj_var, args, "taida_list_any");
            }
            "all" => {
                return self.lower_list_predicate(func, obj_var, args, "taida_list_all");
            }
            "none" => {
                return self.lower_list_predicate(func, obj_var, args, "taida_list_none");
            }
            // Async methods
            "isPending" => "taida_async_is_pending",
            "isFulfilled" => "taida_async_is_fulfilled",
            "isRejected" => "taida_async_is_rejected",
            "unmold" => "taida_generic_unmold",
            // HashMap methods (immutable semantics)
            "set" => {
                return self.lower_hashmap_set(func, obj_var, args);
            }
            "remove" => {
                return self.lower_hashmap_remove(func, obj_var, args);
            }
            "has" => {
                return self.lower_hashmap_has(func, obj_var, args);
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
                return self.lower_set_add(func, obj_var, args);
            }
            "union" => "taida_set_union",
            "intersect" => "taida_set_intersect",
            "diff" => "taida_set_diff",
            "toList" => "taida_set_to_list",
            _ => {
                // Fallback: treat as BuchiPack field function call.
                // obj.method(args) → taida_pack_call_fieldN(obj, hash(method), args...)
                return self.lower_pack_field_call(func, obj, method, args);
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
        runtime_fn: &str,
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: format!(".{}() requires exactly 1 argument", runtime_fn),
            });
        }
        let fn_var = self.lower_expr(func, &args[0])?;
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![list_var, fn_var],
        ));
        Ok(result)
    }

    /// Polymorphic .map() -- dispatches to list_map or monadic_map at runtime
    fn lower_polymorphic_map(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".map() requires exactly 1 argument".to_string(),
            });
        }
        let fn_var = self.lower_expr(func, &args[0])?;
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
        runtime_fn: &str,
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: format!(".{}() requires exactly 1 argument", runtime_fn),
            });
        }
        let fn_var = self.lower_expr(func, &args[0])?;
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![obj_var, fn_var],
        ));
        Ok(result)
    }

    /// Polymorphic .get() -- Str.get(idx), list.get(idx), or HashMap.get(key)
    fn lower_polymorphic_get(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".get() requires exactly 1 argument".to_string(),
            });
        }
        let arg_var = self.lower_expr(func, &args[0])?;
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
    ) -> Result<IrVar, LowerError> {
        if args.len() != 2 {
            return Err(LowerError {
                message: ".set() requires exactly 2 arguments (key, value)".to_string(),
            });
        }
        let key_var = self.lower_expr(func, &args[0])?;
        let value_var = self.lower_expr(func, &args[1])?;
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
        Ok(result)
    }

    /// HashMap.remove(key) -- compute key hash, then call taida_hashmap_remove_immut
    fn lower_hashmap_remove(
        &mut self,
        func: &mut IrFunction,
        hm_var: IrVar,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".remove() requires exactly 1 argument".to_string(),
            });
        }
        let key_var = self.lower_expr(func, &args[0])?;
        // For HashMap: compute key hash, then call taida_collection_remove
        // taida_collection_remove auto-detects HashMap vs Set
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_collection_remove".to_string(),
            vec![hm_var, key_var],
        ));
        Ok(result)
    }

    /// HashMap.has(key) / Set.has(item) -- polymorphic collection has
    fn lower_hashmap_has(
        &mut self,
        func: &mut IrFunction,
        obj_var: IrVar,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".has() requires exactly 1 argument".to_string(),
            });
        }
        let item_var = self.lower_expr(func, &args[0])?;
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_collection_has".to_string(),
            vec![obj_var, item_var],
        ));
        Ok(result)
    }

    /// Set.add(item) -- add item to set
    fn lower_set_add(
        &mut self,
        func: &mut IrFunction,
        set_var: IrVar,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.len() != 1 {
            return Err(LowerError {
                message: ".add() requires exactly 1 argument".to_string(),
            });
        }
        let item_var = self.lower_expr(func, &args[0])?;
        // NO-2: Set elem_type_tag — set before add so the new Set inherits the tag.
        // This is idempotent — subsequent .add() calls will overwrite with the same tag.
        let elem_tag = self.expr_type_tag(&args[0]);
        let tag_const = func.alloc_var();
        func.push(IrInst::ConstInt(tag_const, elem_tag));
        let tag_dummy = func.alloc_var();
        func.push(IrInst::Call(
            tag_dummy,
            "taida_set_set_elem_tag".to_string(),
            vec![set_var, tag_const],
        ));
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_set_add".to_string(),
            vec![set_var, item_var],
        ));
        Ok(result)
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

        let obj_var = self.lower_expr(func, obj)?;

        // Compute field name hash (FNV-1a, same as taida_pack_get uses)
        let field_hash = simple_hash(method);
        let hash_var = func.alloc_var();
        func.push(IrInst::ConstInt(hash_var, field_hash as i64));

        // Build call args: (pack_ptr, field_hash, arg0, arg1, ...)
        let mut call_args = vec![obj_var, hash_var];
        for arg in args {
            let var = self.lower_expr(func, arg)?;
            call_args.push(var);
        }

        let runtime_fn = format!("taida_pack_call_field{}", args.len());
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
