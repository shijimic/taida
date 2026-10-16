use super::{HIDDEN, LowerError, Lowering};
use crate::codegen::ir::*;
/// Mold instantiation lowering for the Taida native backend.
///
/// Contains `lower_mold_inst` (68 mold types) and mold field processing helpers.
///
/// These are `impl Lowering` methods split from lower.rs for maintainability.
use crate::parser::*;

/// A Bool mold option (`all`, `start`, `reverse`, ...): a literal decided
/// while lowering, or a value computed at run time.
pub(crate) enum MoldFlag {
    Static(bool),
    Dynamic(IrVar),
}

impl Lowering {
    /// The Bool mold option `name` of `fields` (`default` when absent).
    pub(crate) fn lower_mold_flag(
        &mut self,
        func: &mut IrFunction,
        fields: &[crate::parser::BuchiField],
        name: &str,
        default: bool,
    ) -> Result<MoldFlag, LowerError> {
        let Some(field) = fields.iter().find(|f| f.name == name) else {
            return Ok(MoldFlag::Static(default));
        };
        Ok(match &field.value {
            Expr::BoolLit(b, _) => MoldFlag::Static(*b),
            other => MoldFlag::Dynamic(self.lower_expr(func, other)?),
        })
    }

    /// `if flag { name(args) } else { otherwise }`.
    pub(crate) fn call_if(
        func: &mut IrFunction,
        flag: IrVar,
        name: &str,
        args: Vec<IrVar>,
        otherwise: IrVar,
    ) -> IrVar {
        let called = func.alloc_var();
        let result = func.alloc_var();
        func.push(IrInst::CondBranch(
            result,
            vec![
                crate::codegen::ir::CondArm {
                    condition: Some(flag),
                    body: vec![IrInst::Call(called, name.to_string(), args)],
                    result: called,
                },
                crate::codegen::ir::CondArm {
                    condition: None,
                    body: vec![],
                    result: otherwise,
                },
            ],
        ));
        result
    }

    /// `if flag { then_name(args) } else { else_name(args) }`.
    pub(crate) fn call_either(
        func: &mut IrFunction,
        flag: IrVar,
        then_name: &str,
        else_name: &str,
        args: Vec<IrVar>,
    ) -> IrVar {
        let then_var = func.alloc_var();
        let else_var = func.alloc_var();
        let result = func.alloc_var();
        func.push(IrInst::CondBranch(
            result,
            vec![
                crate::codegen::ir::CondArm {
                    condition: Some(flag),
                    body: vec![IrInst::Call(then_var, then_name.to_string(), args.clone())],
                    result: then_var,
                },
                crate::codegen::ir::CondArm {
                    condition: None,
                    body: vec![IrInst::Call(else_var, else_name.to_string(), args)],
                    result: else_var,
                },
            ],
        ));
        result
    }

    /// Store a mold's `default` option (already evaluated) as the default of
    /// the fresh Lax `lax` the mold returned.
    fn set_lax_default(
        &self,
        func: &mut IrFunction,
        lax: IrVar,
        fields: &[crate::parser::BuchiField],
        default: Option<IrVar>,
    ) {
        let Some(default) = default else {
            return;
        };
        func.push(IrInst::PackSet(lax, 2, default));
        if let Some(field) = fields.iter().find(|f| f.name == "default") {
            let tag = self.expr_type_tag(&field.value);
            if tag > 0 {
                func.push(IrInst::PackSetTag(lax, 2, tag));
            }
        }
    }

    /// Apply `name` to `value` according to `flag`.
    fn apply_if(func: &mut IrFunction, flag: MoldFlag, name: &str, value: IrVar) -> IrVar {
        match flag {
            MoldFlag::Static(false) => value,
            MoldFlag::Static(true) => {
                let result = func.alloc_var();
                func.push(IrInst::Call(result, name.to_string(), vec![value]));
                result
            }
            MoldFlag::Dynamic(v) => Self::call_if(func, v, name, vec![value], value),
        }
    }

    /// モールドフィールドから文字列リテラル値を取得
    pub(crate) fn lower_mold_field_str(
        &mut self,
        _func: &mut IrFunction,
        fields: &[crate::parser::BuchiField],
        name: &str,
    ) -> Result<Option<String>, LowerError> {
        for field in fields {
            if field.name == name {
                match &field.value {
                    Expr::StringLit(s, _) => return Ok(Some(s.clone())),
                    _ => return Ok(None),
                }
            }
        }
        Ok(None)
    }

    /// モールドフィールドから式を lower して IrVar として返す
    pub(crate) fn lower_mold_field_expr(
        &mut self,
        func: &mut IrFunction,
        fields: &[crate::parser::BuchiField],
        name: &str,
    ) -> Result<Option<IrVar>, LowerError> {
        for field in fields {
            if field.name == name {
                let var = self.lower_expr(func, &field.value)?;
                return Ok(Some(var));
            }
        }
        Ok(None)
    }

    fn lower_todo_default_unit(&mut self, func: &mut IrFunction) -> IrVar {
        let zero = func.alloc_var();
        func.push(IrInst::ConstInt(zero, 0));
        let unit = func.alloc_var();
        func.push(IrInst::Call(unit, "taida_pack_new".to_string(), vec![zero]));
        unit
    }

    fn lower_todo_default_from_type_arg(
        &mut self,
        func: &mut IrFunction,
        arg: &Expr,
    ) -> Result<IrVar, LowerError> {
        match arg {
            Expr::Ident(name, _) => {
                let v = func.alloc_var();
                match name.as_str() {
                    "Int" | "Num" => func.push(IrInst::ConstInt(v, 0)),
                    "Float" => func.push(IrInst::ConstFloat(v, 0.0)),
                    "Str" => func.push(IrInst::ConstStr(v, String::new())),
                    "Bool" => func.push(IrInst::ConstBool(v, false)),
                    "Molten" => func.push(IrInst::Call(v, "taida_molten_new".to_string(), vec![])),
                    _ => return Ok(self.lower_todo_default_unit(func)),
                }
                Ok(v)
            }
            Expr::MoldInst(name, type_args, fields, _) if name == "Stub" => {
                if !fields.is_empty() {
                    return Err(LowerError {
                        message: "Stub does not take `()` fields. Use Stub[\"msg\"]".into(),
                    });
                }
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message: "Stub requires exactly 1 message argument: Stub[\"msg\"]".into(),
                    });
                }
                let v = func.alloc_var();
                func.push(IrInst::Call(v, "taida_molten_new".to_string(), vec![]));
                Ok(v)
            }
            _ => Ok(self.lower_todo_default_unit(func)),
        }
    }

    /// モールド型インスタンス化: `Async[val]()`, `AsyncReject[err]()` etc.
    /// F62B-020: pick the `>=` runtime entry for an operand pair the same
    /// way the `<` / `>` operator lowering dispatches kinds.
    fn ordering_gte_runtime_fn(&self, lhs: &Expr, rhs: &Expr) -> &'static str {
        if self.expr_is_string_full(lhs) || self.expr_is_string_full(rhs) {
            "taida_str_gte"
        } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
            "taida_float_gte"
        } else {
            "taida_int_gte"
        }
    }

    /// F62B-021: lower a host-call Out slot into its schema operand. A
    /// concrete type resolves to a constant descriptor string; a type
    /// parameter of the enclosing schema-passing generic reads the hidden
    /// `#schema_{T}` parameter instead (dictionary passing).
    fn lower_out_schema_operand(
        &mut self,
        func: &mut IrFunction,
        out: &Expr,
    ) -> Result<IrVar, LowerError> {
        if let Expr::Ident(name, span) = out
            && let Some(hidden) = self.current_schema_params.get(name).cloned()
        {
            // F62B-038 #10: synthetic node — do not reuse the Out slot's
            // node id in the node-id-keyed TypedExprTable.
            return self.lower_expr(
                func,
                &Expr::Ident(
                    hidden,
                    span.clone()
                        .with_node_id(crate::parser::SYNTHETIC_NODE_ID_BASE),
                ),
            );
        }
        let desc = self.resolve_json_schema_descriptor(out)?;
        let var = func.alloc_var();
        func.push(IrInst::ConstStr(var, desc));
        Ok(var)
    }

    pub(crate) fn lower_mold_inst(
        &mut self,
        func: &mut IrFunction,
        type_name: &str,
        type_args: &[Expr],
        fields: &[crate::parser::BuchiField],
    ) -> Result<IrVar, LowerError> {
        if let Some(exported) = self.core_mold_aliases.get(type_name).cloned() {
            return self.lower_mold_inst(func, &exported, type_args, fields);
        }
        match type_name {
            // -- Str molds --
            "Upper" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Upper requires 1 argument: Upper[str]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_to_upper".to_string(),
                    vec![s],
                ));
                Ok(result)
            }
            "Lower" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Lower requires 1 argument: Lower[str]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_to_lower".to_string(),
                    vec![s],
                ));
                Ok(result)
            }
            "Trim" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Trim requires 1 argument: Trim[str]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                // オプション: start (default true), end (default true)
                let trim_start = self.lower_mold_flag(func, fields, "start", true)?;
                let trim_end = self.lower_mold_flag(func, fields, "end", true)?;
                if let (MoldFlag::Static(start), MoldFlag::Static(end)) = (&trim_start, &trim_end) {
                    let runtime_fn = match (start, end) {
                        (true, true) => "taida_str_trim",
                        (true, false) => "taida_str_trim_start",
                        (false, true) => "taida_str_trim_end",
                        (false, false) => {
                            // No trimming -- return the string as-is
                            return Ok(s);
                        }
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(result, runtime_fn.to_string(), vec![s]));
                    return Ok(result);
                }
                // A run-time option: trimming the start and then the end
                // trims both sides.
                let trimmed = Self::apply_if(func, trim_start, "taida_str_trim_start", s);
                Ok(Self::apply_if(func, trim_end, "taida_str_trim_end", trimmed))
            }
            "Split" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Split requires 2 arguments: Split[str, delim]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let delim = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_split".to_string(),
                    vec![s, delim],
                ));
                Ok(result)
            }
            "Chars" => {
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message: "Chars requires 1 argument: Chars[str]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_chars".to_string(),
                    vec![s],
                ));
                Ok(result)
            }
            "Replace" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "Replace requires 3 arguments: Replace[str, old, new]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let old = self.lower_expr(func, &type_args[1])?;
                let new_str = self.lower_expr(func, &type_args[2])?;
                // オプション: all (default false)
                match self.lower_mold_flag(func, fields, "all", false)? {
                    MoldFlag::Static(replace_all) => {
                        let runtime_fn = if replace_all {
                            "taida_str_replace"
                        } else {
                            "taida_str_replace_first"
                        };
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            runtime_fn.to_string(),
                            vec![s, old, new_str],
                        ));
                        Ok(result)
                    }
                    MoldFlag::Dynamic(all) => Ok(Self::call_either(
                        func,
                        all,
                        "taida_str_replace",
                        "taida_str_replace_first",
                        vec![s, old, new_str],
                    )),
                }
            }
            // F62B-020: ordering comparison molds. `a <= b` ≡ `b >= a`, so
            // both lower onto the existing gte runtime family with swapped
            // operands (Float NaN semantics included), kind-dispatched the
            // same way the `<` / `>` operators are.
            "Lte" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Lte requires 2 arguments: Lte[a, b]()".into(),
                    });
                }
                let a = self.lower_expr(func, &type_args[0])?;
                let b = self.lower_expr(func, &type_args[1])?;
                let runtime_fn = self.ordering_gte_runtime_fn(&type_args[0], &type_args[1]);
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![b, a]));
                Ok(result)
            }
            "Between" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "Between requires 3 arguments: Between[x, lo, hi]()".into(),
                    });
                }
                let x = self.lower_expr(func, &type_args[0])?;
                let lo = self.lower_expr(func, &type_args[1])?;
                let hi = self.lower_expr(func, &type_args[2])?;
                let lo_fn = self.ordering_gte_runtime_fn(&type_args[1], &type_args[0]);
                let hi_fn = self.ordering_gte_runtime_fn(&type_args[0], &type_args[2]);
                // lo <= x  ≡  x >= lo
                let lo_ok = func.alloc_var();
                func.push(IrInst::Call(lo_ok, lo_fn.to_string(), vec![x, lo]));
                // x <= hi  ≡  hi >= x
                let hi_ok = func.alloc_var();
                func.push(IrInst::Call(hi_ok, hi_fn.to_string(), vec![hi, x]));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bool_and".to_string(),
                    vec![lo_ok, hi_ok],
                ));
                Ok(result)
            }
            // F62B-003: search / replace / pad molds, dispatching to the same
            // runtime symbols as the string-method forms so the two surfaces
            // can never disagree. Previously these were "unsupported mold
            // type" on the C-lowering backends (and an unresolved pack on the
            // interpreter).
            "IndexOf" | "LastIndexOf" | "Contains" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: format!(
                            "{} requires 2 arguments: {}[str, needle]()",
                            type_name, type_name
                        ),
                    });
                }
                let subject = self.lower_expr(func, &type_args[0])?;
                let needle = self.lower_expr(func, &type_args[1])?;
                let runtime_fn = match type_name {
                    "IndexOf" => "taida_polymorphic_index_of",
                    "LastIndexOf" => "taida_polymorphic_last_index_of",
                    _ => "taida_polymorphic_contains",
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    runtime_fn.to_string(),
                    vec![subject, needle],
                ));
                Ok(result)
            }
            "ReplaceAll" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "ReplaceAll requires 3 arguments: ReplaceAll[str, old, new]()"
                            .into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let old = self.lower_expr(func, &type_args[1])?;
                let new_str = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_replace".to_string(),
                    vec![s, old, new_str],
                ));
                Ok(result)
            }
            "PadLeft" | "PadRight" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: format!(
                            "{} requires 3 arguments: {}[str, len, char]()",
                            type_name, type_name
                        ),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let target_len = self.lower_expr(func, &type_args[1])?;
                let pad_char = self.lower_expr(func, &type_args[2])?;
                let pad_end_flag = func.alloc_var();
                let is_end = if type_name == "PadRight" { 1i64 } else { 0i64 };
                func.push(IrInst::ConstInt(pad_end_flag, is_end));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_pad".to_string(),
                    vec![s, target_len, pad_char, pad_end_flag],
                ));
                Ok(result)
            }
            // ── C26B-016 (@c.26, Option B+): span-aware comparison molds ──
            // Dispatches to the C runtime helpers in native_runtime/core.c.
            // SpanEquals / SpanStartsWith / SpanContains → Bool (taida_val 0/1).
            // SpanSlice → Pack (sub-span `@(start, len)`).
            "SpanEquals" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "SpanEquals requires 3 arguments: SpanEquals[span, raw, needle]()".into(),
                    });
                }
                let span = self.lower_expr(func, &type_args[0])?;
                let raw = self.lower_expr(func, &type_args[1])?;
                let needle = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_net_SpanEquals".to_string(),
                    vec![span, raw, needle],
                ));
                Ok(result)
            }
            "SpanStartsWith" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "SpanStartsWith requires 3 arguments: SpanStartsWith[span, raw, prefix]()".into(),
                    });
                }
                let span = self.lower_expr(func, &type_args[0])?;
                let raw = self.lower_expr(func, &type_args[1])?;
                let prefix = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_net_SpanStartsWith".to_string(),
                    vec![span, raw, prefix],
                ));
                Ok(result)
            }
            "SpanContains" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "SpanContains requires 3 arguments: SpanContains[span, raw, needle]()".into(),
                    });
                }
                let span = self.lower_expr(func, &type_args[0])?;
                let raw = self.lower_expr(func, &type_args[1])?;
                let needle = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_net_SpanContains".to_string(),
                    vec![span, raw, needle],
                ));
                Ok(result)
            }
            "SpanSlice" => {
                if type_args.len() < 4 {
                    return Err(LowerError {
                        message: "SpanSlice requires 4 arguments: SpanSlice[span, raw, start, end]()".into(),
                    });
                }
                let span = self.lower_expr(func, &type_args[0])?;
                let raw = self.lower_expr(func, &type_args[1])?;
                let sub_start = self.lower_expr(func, &type_args[2])?;
                let sub_end = self.lower_expr(func, &type_args[3])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_net_SpanSlice".to_string(),
                    vec![span, raw, sub_start, sub_end],
                ));
                Ok(result)
            }
            // ── C26B-016 (@c.26, Option B+): `StrOf[span, raw]() -> Str` ──
            // Cold-path materialization of a span into an owned Str.
            // This lowering is a **pure IR composition** using existing C
            // runtime helpers — it deliberately does NOT add a new
            // `taida_net_StrOf` runtime function (avoids core.c churn during
            // Round 3 and coexists cleanly with the clone-heavy abstraction
            // work in wG).
            //
            // IR sequence:
            //   start = taida_pack_get(span, hash("start"))
            //   len   = taida_pack_get(span, hash("len"))
            //   end   = taida_int_add(start, len)
            //   bytes = taida_slice_mold(raw, start, end)
            //   lax   = taida_utf8_decode_mold(bytes)
            //   str   = taida_lax_get_or_default(lax, "")
            //
            // This matches `StrOf` interpreter / JS semantics: invalid UTF-8
            // or OOB span → empty string (tolerant; `taida_slice_mold` clamps
            // end to buf_len, and `taida_utf8_decode_mold` returns an empty
            // Lax on UTF-8 failure which `get_or_default("")` then materializes).
            "StrOf" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "StrOf requires 2 arguments: StrOf[span, raw]()".into(),
                    });
                }
                let span = self.lower_expr(func, &type_args[0])?;
                let raw_is_str = self.expr_is_string_full(&type_args[1]);
                let raw = self.lower_expr(func, &type_args[1])?;

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

                // If raw is known at lower time to be a Str, convert to Bytes first
                // so `taida_slice_mold` → Bytes (not character-based Str slice) and
                // `taida_utf8_decode_mold` succeeds. When raw is Bytes (the common
                // `req.raw` case), skip the conversion.
                let raw_bytes = if raw_is_str {
                    // enc_lax = taida_utf8_encode_mold(raw)
                    let enc_lax = func.alloc_var();
                    func.push(IrInst::Call(
                        enc_lax,
                        "taida_utf8_encode_mold".to_string(),
                        vec![raw],
                    ));
                    // default for encode = empty bytes via ConstStr("") (runtime
                    // permits using empty Str as Bytes fallback here because
                    // taida_lax_get_or_default is type-polymorphic at C level)
                    let empty_bytes_default = func.alloc_var();
                    func.push(IrInst::ConstStr(empty_bytes_default, String::new()));
                    // bytes = taida_lax_get_or_default(enc_lax, "")
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

                // bytes = taida_slice_mold(rawBytes, start, end)
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

                // default = "" (empty Str)
                let default_val = func.alloc_var();
                func.push(IrInst::ConstStr(default_val, String::new()));

                // str = taida_lax_get_or_default(lax, "")
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_lax_get_or_default".to_string(),
                    vec![lax_val, default_val],
                ));
                Ok(result)
            }
            "Slice" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Slice requires 1 argument: Slice[value](start, end)".into(),
                    });
                }
                let value = self.lower_expr(func, &type_args[0])?;
                // C25B-031: positional (`Slice[s, start, end]()`) and named
                // (`Slice[s](start <= n, end <= m)`) forms must both resolve
                // the start/end variables. Interpreter prefers positional
                // `type_args[1..]` over named `fields`; match that here.
                let start_var = if type_args.len() >= 2 {
                    self.lower_expr(func, &type_args[1])?
                } else {
                    match self.lower_mold_field_expr(func, fields, "start")? {
                        Some(v) => v,
                        None => {
                            let v = func.alloc_var();
                            func.push(IrInst::ConstInt(v, 0));
                            v
                        }
                    }
                };
                let end_var = if type_args.len() >= 3 {
                    self.lower_expr(func, &type_args[2])?
                } else {
                    match self.lower_mold_field_expr(func, fields, "end")? {
                        Some(v) => v,
                        None => {
                            // Omitted-end sentinel: i64::MIN is unreachable as a
                            // user index, so the runtime can distinguish "to the
                            // end" from an explicit negative end (which the
                            // reference clamps to 0 — an empty slice).
                            let v = func.alloc_var();
                            func.push(IrInst::ConstInt(v, i64::MIN));
                            v
                        }
                    }
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_slice_mold".to_string(),
                    vec![value, start_var, end_var],
                ));
                Ok(result)
            }
            "CharAt" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "CharAt requires 2 arguments: CharAt[str, idx]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let idx = self.lower_expr(func, &type_args[1])?;
                let raw = func.alloc_var();
                func.push(IrInst::Call(
                    raw,
                    "taida_str_char_at".to_string(),
                    vec![s, idx],
                ));
                // TF-15: CharAt returns Lax[Str] (matching interpreter).
                // An index outside the string yields "" from the runtime,
                // while a character is never empty: "" is the failed Lax.
                let default_val = func.alloc_var();
                func.push(IrInst::ConstStr(default_val, String::new()));
                let found = func.alloc_var();
                func.push(IrInst::Call(
                    found,
                    "taida_str_neq".to_string(),
                    vec![raw, default_val],
                ));
                let present = func.alloc_var();
                let absent = func.alloc_var();
                let result = func.alloc_var();
                func.push(IrInst::CondBranch(
                    result,
                    vec![
                        crate::codegen::ir::CondArm {
                            condition: Some(found),
                            body: vec![IrInst::Call(
                                present,
                                "taida_lax_new".to_string(),
                                vec![raw, default_val],
                            )],
                            result: present,
                        },
                        crate::codegen::ir::CondArm {
                            condition: None,
                            body: vec![IrInst::Call(
                                absent,
                                "taida_lax_empty".to_string(),
                                vec![default_val],
                            )],
                            result: absent,
                        },
                    ],
                ));
                Ok(result)
            }
            "Repeat" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Repeat requires 2 arguments: Repeat[str, n]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_repeat".to_string(),
                    vec![s, n],
                ));
                Ok(result)
            }
            // ── C26B-018 (B) byte-level primitives ──────────────────
            "ByteAt" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ByteAt requires 2 arguments: ByteAt[str, idx]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let idx = self.lower_expr(func, &type_args[1])?;
                let raw = func.alloc_var();
                func.push(IrInst::Call(
                    raw,
                    "taida_str_byte_at".to_string(),
                    vec![s, idx],
                ));
                // ByteAt returns Lax[Int] (has_value=false → default 0).
                let default_val = func.alloc_var();
                func.push(IrInst::ConstInt(default_val, 0));
                // Sentinel: taida_str_byte_at returns -1 for OOB; we
                // encode "has_value" at the runtime layer (see
                // taida_str_byte_at_lax helper below in core.c).
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_byte_at_lax".to_string(),
                    vec![s, idx, default_val],
                ));
                // raw is unused on this path (dead-store elimination
                // keeps it out of the final codegen); we compute it only
                // to keep the variable-slot alloc pattern consistent
                // with other byte-level molds in case future Int-tag
                // projection wants a non-Lax fast path.
                let _ = raw;
                Ok(result)
            }
            "ByteSlice" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message:
                            "ByteSlice requires 3 arguments: ByteSlice[str, start, end]()"
                                .into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let start = self.lower_expr(func, &type_args[1])?;
                let end = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_byte_slice".to_string(),
                    vec![s, start, end],
                ));
                Ok(result)
            }
            "ByteLength" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "ByteLength requires 1 argument: ByteLength[str]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_byte_length".to_string(),
                    vec![s],
                ));
                Ok(result)
            }
            // ── C26B-018 (C) StringRepeatJoin ───────────────────────
            "StringRepeatJoin" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message:
                            "StringRepeatJoin requires 3 arguments: StringRepeatJoin[str, n, sep]()"
                                .into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let sep = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_repeat_join".to_string(),
                    vec![s, n, sep],
                ));
                Ok(result)
            }
            "Reverse" => {
                // Polymorphic: works on both Str and List
                // In native backend, we use taida_str_reverse for strings
                // and taida_list_reverse for lists. At compile time we don't
                // have type info, so we use a unified runtime function.
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Reverse requires 1 argument: Reverse[value]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                // Check if the expression is statically known to be a string
                let is_string = self.expr_is_string_full(&type_args[0]);
                let runtime_fn = if is_string {
                    "taida_str_reverse"
                } else {
                    // Default to list reverse -- most common case in native
                    // For polymorphic dispatch at runtime, we'd need a tag check
                    "taida_list_reverse"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![val]));
                Ok(result)
            }
            "Pad" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Pad requires 2 arguments: Pad[str, len]()".into(),
                    });
                }
                let s = self.lower_expr(func, &type_args[0])?;
                let target_len = self.lower_expr(func, &type_args[1])?;
                // オプション: char (default " "), side (default "start")
                let pad_char_var = match self.lower_mold_field_expr(func, fields, "char")? {
                    Some(v) => v,
                    None => {
                        let v = func.alloc_var();
                        func.push(IrInst::ConstStr(v, " ".to_string()));
                        v
                    }
                };
                let pad_end_flag = match fields.iter().find(|f| f.name == "side") {
                    Some(side) if !matches!(side.value, Expr::StringLit(_, _)) => {
                        // A run-time side: pad the end exactly when it is "end".
                        let side_var = self.lower_expr(func, &side.value)?;
                        let end = func.alloc_var();
                        func.push(IrInst::ConstStr(end, "end".to_string()));
                        let flag = func.alloc_var();
                        func.push(IrInst::Call(
                            flag,
                            "taida_str_eq".to_string(),
                            vec![side_var, end],
                        ));
                        flag
                    }
                    _ => {
                        let side_str = self
                            .lower_mold_field_str(func, fields, "side")?
                            .unwrap_or_else(|| "start".to_string());
                        let flag = func.alloc_var();
                        let is_end = if side_str == "end" { 1i64 } else { 0i64 };
                        func.push(IrInst::ConstInt(flag, is_end));
                        flag
                    }
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_str_pad".to_string(),
                    vec![s, target_len, pad_char_var, pad_end_flag],
                ));
                Ok(result)
            }
            // -- Num molds --
            "ToFixed" => {
                // ToFixed[num, digits]() -> Str
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ToFixed requires 2 arguments: ToFixed[num, digits]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let digits = self.lower_expr(func, &type_args[1])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                // If Int, convert to Float first
                let float_val = if is_float {
                    num
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![num],
                    ));
                    tmp
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_float_to_fixed".to_string(),
                    vec![float_val, digits],
                ));
                Ok(result)
            }
            "Abs" => {
                // Abs[num]() -> same type as input
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Abs requires 1 argument: Abs[num]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                let result = func.alloc_var();
                if is_float {
                    func.push(IrInst::Call(
                        result,
                        "taida_float_abs".to_string(),
                        vec![num],
                    ));
                } else {
                    func.push(IrInst::Call(result, "taida_int_abs".to_string(), vec![num]));
                }
                Ok(result)
            }
            "Floor" => {
                // Floor[num]() -> Int
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Floor requires 1 argument: Floor[num]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                if is_float {
                    // Float: floor then convert to int
                    let floored = func.alloc_var();
                    func.push(IrInst::Call(
                        floored,
                        "taida_float_floor".to_string(),
                        vec![num],
                    ));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_float_to_int".to_string(),
                        vec![floored],
                    ));
                    Ok(result)
                } else {
                    // Int: identity
                    Ok(num)
                }
            }
            "Ceil" => {
                // Ceil[num]() -> Int
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Ceil requires 1 argument: Ceil[num]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                if is_float {
                    let ceiled = func.alloc_var();
                    func.push(IrInst::Call(
                        ceiled,
                        "taida_float_ceil".to_string(),
                        vec![num],
                    ));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_float_to_int".to_string(),
                        vec![ceiled],
                    ));
                    Ok(result)
                } else {
                    Ok(num)
                }
            }
            "Round" => {
                // Round[num]() -> Int
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Round requires 1 argument: Round[num]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                if is_float {
                    let rounded = func.alloc_var();
                    func.push(IrInst::Call(
                        rounded,
                        "taida_float_round".to_string(),
                        vec![num],
                    ));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_float_to_int".to_string(),
                        vec![rounded],
                    ));
                    Ok(result)
                } else {
                    Ok(num)
                }
            }
            "Truncate" => {
                // Truncate[num]() -> Int (trunc toward zero)
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Truncate requires 1 argument: Truncate[num]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                if is_float {
                    // taida_float_to_int does (long)a which is trunc toward zero
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_float_to_int".to_string(),
                        vec![num],
                    ));
                    Ok(result)
                } else {
                    Ok(num)
                }
            }
            "Clamp" => {
                // Clamp[num, min, max]() -> same type as input
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "Clamp requires 3 arguments: Clamp[num, min, max]()".into(),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let mut min_val = self.lower_expr(func, &type_args[1])?;
                let mut max_val = self.lower_expr(func, &type_args[2])?;
                let is_float = self.expr_returns_float(&type_args[0])
                    || self.expr_returns_float(&type_args[1])
                    || self.expr_returns_float(&type_args[2]);
                if self.expr_returns_float(&type_args[0]) {
                    // Int bounds on a Float value compare in f64, as the
                    // reference widens them.
                    for (bound, expr) in [(&mut min_val, &type_args[1]), (&mut max_val, &type_args[2])] {
                        if self.expr_is_int(expr) {
                            let widened = func.alloc_var();
                            func.push(IrInst::Call(
                                widened,
                                "taida_int_to_float".to_string(),
                                vec![*bound],
                            ));
                            *bound = widened;
                        }
                    }
                }
                let result = func.alloc_var();
                if is_float {
                    func.push(IrInst::Call(
                        result,
                        "taida_float_clamp".to_string(),
                        vec![num, min_val, max_val],
                    ));
                } else {
                    func.push(IrInst::Call(
                        result,
                        "taida_int_clamp".to_string(),
                        vec![num, min_val, max_val],
                    ));
                }
                Ok(result)
            }
            // C25B-025 Phase 5-I: math mold family. Interpreter + JS
            // land in Phase 5-A (commit 86d5743). Native delegates to
            // libm (`sqrt`, `pow`, `sin`, ...); wasm uses manual
            // freestanding implementations in
            // `runtime_core_wasm/03_typeof_list.inc.c`. All molds
            // return Float per `src/types/mold_specs.rs` and widen
            // Int arguments to Float via `taida_int_to_float` first
            // (same as interpreter's `eval_unary_math` which accepts
            // either Int or Float and widens Int to f64).
            "Sqrt" | "Exp" | "Ln" | "Log2" | "Log10" | "Sin" | "Cos" | "Tan" | "Asin"
            | "Acos" | "Atan" | "Sinh" | "Cosh" | "Tanh" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: format!("{} requires 1 argument: {}[num]()", type_name, type_name),
                    });
                }
                let num = self.lower_expr(func, &type_args[0])?;
                let is_float = self.expr_returns_float(&type_args[0]);
                let float_val = if is_float {
                    num
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![num],
                    ));
                    tmp
                };
                let runtime_fn = match type_name {
                    "Sqrt" => "taida_float_sqrt",
                    "Exp" => "taida_float_exp",
                    "Ln" => "taida_float_ln",
                    "Log2" => "taida_float_log2",
                    "Log10" => "taida_float_log10",
                    "Sin" => "taida_float_sin",
                    "Cos" => "taida_float_cos",
                    "Tan" => "taida_float_tan",
                    "Asin" => "taida_float_asin",
                    "Acos" => "taida_float_acos",
                    "Atan" => "taida_float_atan",
                    "Sinh" => "taida_float_sinh",
                    "Cosh" => "taida_float_cosh",
                    "Tanh" => "taida_float_tanh",
                    _ => unreachable!(),
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![float_val]));
                Ok(result)
            }
            "Pow" => {
                // Pow[base, exp]() -> Float
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Pow requires 2 arguments: Pow[base, exp]()".into(),
                    });
                }
                let base_raw = self.lower_expr(func, &type_args[0])?;
                let exp_raw = self.lower_expr(func, &type_args[1])?;
                let base = if self.expr_returns_float(&type_args[0]) {
                    base_raw
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![base_raw],
                    ));
                    tmp
                };
                let exp_val = if self.expr_returns_float(&type_args[1]) {
                    exp_raw
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![exp_raw],
                    ));
                    tmp
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_float_pow".to_string(),
                    vec![base, exp_val],
                ));
                Ok(result)
            }
            "Log" => {
                // Log[value, base]() -> Float. Matches interpreter:
                // `val.log(base)` = `ln(val) / ln(base)`; without a base,
                // `Log[value]()` is the natural logarithm (`val.ln()`).
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Log requires 1-2 arguments: Log[value]() or Log[value, base]()"
                            .into(),
                    });
                }
                if type_args.len() == 1 {
                    let val_raw = self.lower_expr(func, &type_args[0])?;
                    let val = if self.expr_returns_float(&type_args[0]) {
                        val_raw
                    } else {
                        let tmp = func.alloc_var();
                        func.push(IrInst::Call(
                            tmp,
                            "taida_int_to_float".to_string(),
                            vec![val_raw],
                        ));
                        tmp
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_float_ln".to_string(),
                        vec![val],
                    ));
                    return Ok(result);
                }
                let val_raw = self.lower_expr(func, &type_args[0])?;
                let base_raw = self.lower_expr(func, &type_args[1])?;
                let val = if self.expr_returns_float(&type_args[0]) {
                    val_raw
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![val_raw],
                    ));
                    tmp
                };
                let base = if self.expr_returns_float(&type_args[1]) {
                    base_raw
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![base_raw],
                    ));
                    tmp
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_float_log".to_string(),
                    vec![val, base],
                ));
                Ok(result)
            }
            "Atan2" => {
                // Atan2[y, x]() -> Float
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Atan2 requires 2 arguments: Atan2[y, x]()".into(),
                    });
                }
                let y_raw = self.lower_expr(func, &type_args[0])?;
                let x_raw = self.lower_expr(func, &type_args[1])?;
                let y = if self.expr_returns_float(&type_args[0]) {
                    y_raw
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![y_raw],
                    ));
                    tmp
                };
                let x = if self.expr_returns_float(&type_args[1]) {
                    x_raw
                } else {
                    let tmp = func.alloc_var();
                    func.push(IrInst::Call(
                        tmp,
                        "taida_int_to_float".to_string(),
                        vec![x_raw],
                    ));
                    tmp
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_float_atan2".to_string(),
                    vec![y, x],
                ));
                Ok(result)
            }
            "BitAnd" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "BitAnd requires 2 arguments: BitAnd[a, b]()".into(),
                    });
                }
                let a = self.lower_expr(func, &type_args[0])?;
                let b = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bit_and".to_string(),
                    vec![a, b],
                ));
                Ok(result)
            }
            "BitOr" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "BitOr requires 2 arguments: BitOr[a, b]()".into(),
                    });
                }
                let a = self.lower_expr(func, &type_args[0])?;
                let b = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_bit_or".to_string(), vec![a, b]));
                Ok(result)
            }
            "BitXor" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "BitXor requires 2 arguments: BitXor[a, b]()".into(),
                    });
                }
                let a = self.lower_expr(func, &type_args[0])?;
                let b = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bit_xor".to_string(),
                    vec![a, b],
                ));
                Ok(result)
            }
            "BitNot" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "BitNot requires 1 argument: BitNot[x]()".into(),
                    });
                }
                let x = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_bit_not".to_string(), vec![x]));
                Ok(result)
            }
            "ShiftL" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ShiftL requires 2 arguments: ShiftL[x, n]()".into(),
                    });
                }
                let x = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_shift_l".to_string(),
                    vec![x, n],
                ));
                Ok(result)
            }
            "ShiftR" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ShiftR requires 2 arguments: ShiftR[x, n]()".into(),
                    });
                }
                let x = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_shift_r".to_string(),
                    vec![x, n],
                ));
                Ok(result)
            }
            "ShiftRU" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ShiftRU requires 2 arguments: ShiftRU[x, n]()".into(),
                    });
                }
                let x = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_shift_ru".to_string(),
                    vec![x, n],
                ));
                Ok(result)
            }
            "ToRadix" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ToRadix requires 2 arguments: ToRadix[int, base]()".into(),
                    });
                }
                let value = self.lower_expr(func, &type_args[0])?;
                let base = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_to_radix".to_string(),
                    vec![value, base],
                ));
                Ok(result)
            }
            "U16BE" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U16BE requires 1 argument: U16BE[value]()".into(),
                    });
                }
                let value = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u16be_mold".to_string(),
                    vec![value],
                ));
                Ok(result)
            }
            "U16LE" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U16LE requires 1 argument: U16LE[value]()".into(),
                    });
                }
                let value = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u16le_mold".to_string(),
                    vec![value],
                ));
                Ok(result)
            }
            "U32BE" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U32BE requires 1 argument: U32BE[value]()".into(),
                    });
                }
                let value = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u32be_mold".to_string(),
                    vec![value],
                ));
                Ok(result)
            }
            "U32LE" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U32LE requires 1 argument: U32LE[value]()".into(),
                    });
                }
                let value = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u32le_mold".to_string(),
                    vec![value],
                ));
                Ok(result)
            }
            "U16BEDecode" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U16BEDecode requires 1 argument: U16BEDecode[bytes]()".into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u16be_decode_mold".to_string(),
                    vec![bytes],
                ));
                Ok(result)
            }
            "U16LEDecode" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U16LEDecode requires 1 argument: U16LEDecode[bytes]()".into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u16le_decode_mold".to_string(),
                    vec![bytes],
                ));
                Ok(result)
            }
            "U32BEDecode" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U32BEDecode requires 1 argument: U32BEDecode[bytes]()".into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u32be_decode_mold".to_string(),
                    vec![bytes],
                ));
                Ok(result)
            }
            "U32LEDecode" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "U32LEDecode requires 1 argument: U32LEDecode[bytes]()".into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_u32le_decode_mold".to_string(),
                    vec![bytes],
                ));
                Ok(result)
            }
            "BytesCursor" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "BytesCursor requires 1 argument: BytesCursor[bytes]()".into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let offset = match self.lower_mold_field_expr(func, fields, "offset")? {
                    Some(v) => v,
                    None => {
                        let v = func.alloc_var();
                        func.push(IrInst::ConstInt(v, 0));
                        v
                    }
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_cursor_new".to_string(),
                    vec![bytes, offset],
                ));
                Ok(result)
            }
            "BytesCursorRemaining" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "BytesCursorRemaining requires 1 argument: BytesCursorRemaining[cursor]()".into(),
                    });
                }
                let cursor = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_cursor_remaining".to_string(),
                    vec![cursor],
                ));
                Ok(result)
            }
            "BytesCursorTake" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message:
                            "BytesCursorTake requires 2 arguments: BytesCursorTake[cursor, size]()"
                                .into(),
                    });
                }
                let cursor = self.lower_expr(func, &type_args[0])?;
                let size = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_cursor_take".to_string(),
                    vec![cursor, size],
                ));
                Ok(result)
            }
            "BytesCursorU8" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "BytesCursorU8 requires 1 argument: BytesCursorU8[cursor]()"
                            .into(),
                    });
                }
                let cursor = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_cursor_u8".to_string(),
                    vec![cursor],
                ));
                Ok(result)
            }

            // -- List molds --
            "Concat" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Concat requires 2 arguments: Concat[list, other]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let other = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_concat".to_string(),
                    vec![list, other],
                ));
                Ok(result)
            }
            "ByteSet" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "ByteSet requires 3 arguments: ByteSet[bytes, idx, value]()"
                            .into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let idx = self.lower_expr(func, &type_args[1])?;
                let value = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_set".to_string(),
                    vec![bytes, idx, value],
                ));
                Ok(result)
            }
            "BytesToList" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "BytesToList requires 1 argument: BytesToList[bytes]()".into(),
                    });
                }
                let bytes = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_to_list".to_string(),
                    vec![bytes],
                ));
                Ok(result)
            }
            "Append" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Append requires 2 arguments: Append[list, val]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let val = self.lower_expr(func, &type_args[1])?;
                // Value-tag track: supply the appended item's statically
                // known kind so an empty / array-carrying list records the
                // element identity (a Float pushed into `@[]` used to leave
                // the list kindless and display as raw f64 bits). UNKNOWN
                // keeps the legacy tag behaviour.
                let ek_var = func.alloc_var();
                func.push(IrInst::ConstInt(ek_var, self.expr_ekind(&type_args[1])));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_append_k".to_string(),
                    vec![list, val, ek_var],
                ));
                Ok(result)
            }
            "Prepend" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Prepend requires 2 arguments: Prepend[list, val]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let val = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_prepend".to_string(),
                    vec![list, val],
                ));
                Ok(result)
            }
            "Join" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Join requires 2 arguments: Join[list, sep]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let sep = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_join".to_string(),
                    vec![list, sep],
                ));
                Ok(result)
            }
            "Min" | "Max" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: format!("{} requires 1 argument: {}[list]()", type_name, type_name),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                let rt = if type_name == "Min" {
                    "taida_list_min"
                } else {
                    "taida_list_max"
                };
                func.push(IrInst::Call(result, rt.to_string(), vec![list]));
                Ok(result)
            }
            "Sum" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Sum requires 1 argument: Sum[list]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_sum".to_string(),
                    vec![list],
                ));
                Ok(result)
            }
            "Sort" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Sort requires 1 argument: Sort[list]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                // オプション: by (key extraction function), reverse, desc
                let by_fn = self.lower_mold_field_expr(func, fields, "by")?;
                let reverse = self.lower_mold_flag(func, fields, "reverse", false)?;
                let desc = self.lower_mold_flag(func, fields, "desc", false)?;
                // Descending when either option is set.
                let descending = match (reverse, desc) {
                    (MoldFlag::Static(a), MoldFlag::Static(b)) => MoldFlag::Static(a || b),
                    (MoldFlag::Static(true), _) | (_, MoldFlag::Static(true)) => {
                        MoldFlag::Static(true)
                    }
                    (MoldFlag::Dynamic(v), MoldFlag::Static(false))
                    | (MoldFlag::Static(false), MoldFlag::Dynamic(v)) => MoldFlag::Dynamic(v),
                    (MoldFlag::Dynamic(a), MoldFlag::Dynamic(b)) => {
                        let either = func.alloc_var();
                        func.push(IrInst::Call(either, "taida_bool_or".to_string(), vec![a, b]));
                        MoldFlag::Dynamic(either)
                    }
                };
                // The reference's order (see `taida_list_sort_kind`): keys of
                // the kind the `by` function returns, NaN keys last.
                let key_tag = match fields.iter().find(|f| f.name == "by") {
                    Some(field) => match self.typed_expr_table.lookup(&field.value) {
                        Some(crate::types::Type::Function(_, ret)) => {
                            self.type_runtime_tag(ret).unwrap_or(-1)
                        }
                        _ => -1,
                    },
                    None => -1,
                };
                let key_fn = match by_fn {
                    Some(by_var) => by_var,
                    None => {
                        let zero = func.alloc_var();
                        func.push(IrInst::ConstInt(zero, 0));
                        zero
                    }
                };
                let key_tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(key_tag_var, key_tag));
                let descending_var = match descending {
                    MoldFlag::Static(flag) => {
                        let v = func.alloc_var();
                        func.push(IrInst::ConstInt(v, i64::from(flag)));
                        v
                    }
                    MoldFlag::Dynamic(v) => v,
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_sort_kind".to_string(),
                    vec![list, key_fn, key_tag_var, descending_var],
                ));
                Ok(result)
            }
            "Unique" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Unique requires 1 argument: Unique[list]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                // E34B-020 (Codex review #16 follow-up): consume the
                // optional `by` callback to match the interpreter and
                // restore 4-backend parity. Without this fork, the
                // Native / WASM lowering silently fell back to value-
                // identity dedup whenever the user supplied a key
                // extractor.
                let by_fn = self.lower_mold_field_expr(func, fields, "by")?;
                let result = func.alloc_var();
                if let Some(by_var) = by_fn {
                    func.push(IrInst::Call(
                        result,
                        "taida_list_unique_by".to_string(),
                        vec![list, by_var],
                    ));
                } else {
                    func.push(IrInst::Call(
                        result,
                        "taida_list_unique".to_string(),
                        vec![list],
                    ));
                }
                Ok(result)
            }
            "Flatten" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Flatten requires 1 argument: Flatten[list]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_flatten".to_string(),
                    vec![list],
                ));
                Ok(result)
            }
            "Find" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Find requires 2 arguments: Find[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_find".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            "FindIndex" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "FindIndex requires 2 arguments: FindIndex[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_find_index".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            // E32B-022 (Lock-N): Lax[Int]-returning replacement for the
            // legacy `-1`-sentinel `FindIndex`.
            "FindIndexLax" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "FindIndexLax requires 2 arguments: FindIndexLax[list, fn]()"
                            .into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_find_index_lax".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            // Length[x](): the `.length()` of the value (list elements, Str
            // characters, Bytes bytes).
            "Length" if type_args.len() == 1 => {
                let value = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_polymorphic_length".to_string(),
                    vec![value],
                ));
                Ok(result)
            }
            "Count" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Count requires 2 arguments: Count[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_count".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            "Zip" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Zip requires 2 arguments: Zip[list, other]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let other = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_zip".to_string(),
                    vec![list, other],
                ));
                Ok(result)
            }
            "Enumerate" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Enumerate requires 1 argument: Enumerate[list]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_enumerate".to_string(),
                    vec![list],
                ));
                Ok(result)
            }

            // -- HOF molds --
            "Map" => {
                // Map[list, fn]() -> new list with fn applied
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Map requires 2 arguments: Map[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                // Value-tag track: supply the callback's statically known
                // return kind so a type-changing map keeps exact container
                // kinds. An identity-ish lambda (body kind unknown at this
                // point — params are not yet in scope) passes UNKNOWN and
                // the result stays kindless, which is the documented
                // residual (dynamic return kinds need a function-return
                // shadow mechanism out of S1 scope).
                let ret_ekind = if let Expr::Lambda(_, body, _) = &type_args[1] {
                    self.expr_ekind(body)
                } else {
                    0xFF
                };
                let ek_var = func.alloc_var();
                func.push(IrInst::ConstInt(ek_var, ret_ekind));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_map_k".to_string(),
                    vec![list, fn_var, ek_var],
                ));
                Ok(result)
            }
            "Filter" => {
                // Filter[list, fn]() -> new list with only truthy elements
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Filter requires 2 arguments: Filter[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_filter".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            "Fold" | "Reduce" => {
                // Fold[list, init, fn]() -> accumulated value (left fold)
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "Fold requires 3 arguments: Fold[list, init, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let init = self.lower_expr(func, &type_args[1])?;
                let fn_var = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_fold".to_string(),
                    vec![list, init, fn_var],
                ));
                Ok(result)
            }
            "Foldr" => {
                // Foldr[list, init, fn]() -> accumulated value (right fold)
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message: "Foldr requires 3 arguments: Foldr[list, init, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let init = self.lower_expr(func, &type_args[1])?;
                let fn_var = self.lower_expr(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_foldr".to_string(),
                    vec![list, init, fn_var],
                ));
                Ok(result)
            }
            "Take" => {
                // Take[list, n]() -> first n elements
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Take requires 2 arguments: Take[list, n]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_take".to_string(),
                    vec![list, n],
                ));
                Ok(result)
            }
            "TakeWhile" => {
                // TakeWhile[list, fn]() -> take while fn returns truthy
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "TakeWhile requires 2 arguments: TakeWhile[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_take_while".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            "Drop" => {
                // Drop[list, n]() -> skip first n elements
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Drop requires 2 arguments: Drop[list, n]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let n = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_drop".to_string(),
                    vec![list, n],
                ));
                Ok(result)
            }
            "DropWhile" => {
                // DropWhile[list, fn]() -> skip while fn returns truthy
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "DropWhile requires 2 arguments: DropWhile[list, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_drop_while".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }

            "Async" => {
                // NO-3: Async[val]() -> taida_async_ok_tagged(val, tag)
                if type_args.is_empty() {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    let tag_var = func.alloc_var();
                    func.push(IrInst::ConstInt(tag_var, 0)); // TAIDA_TAG_INT
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_async_ok_tagged".to_string(),
                        vec![zero, tag_var],
                    ));
                    Ok(result)
                } else {
                    let val = self.lower_expr(func, &type_args[0])?;
                    let tag = self.expr_type_tag(&type_args[0]);
                    let tag_var = func.alloc_var();
                    func.push(IrInst::ConstInt(tag_var, tag));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_async_ok_tagged".to_string(),
                        vec![val, tag_var],
                    ));
                    Ok(result)
                }
            }
            "AsyncReject" => {
                // AsyncReject[err]() -> taida_async_err(err)
                if type_args.is_empty() {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_async_err".to_string(),
                        vec![zero],
                    ));
                    Ok(result)
                } else {
                    // The payload's static kind lets the runtime release,
                    // display and throw it as what it is.
                    let val = self.lower_expr(func, &type_args[0])?;
                    let ekind = self.emit_operand_ekind(func, &type_args[0]);
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_async_err_kinded".to_string(),
                        vec![val, ekind],
                    ));
                    Ok(result)
                }
            }
            "AsyncTask" => {
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message:
                            "AsyncTask requires 1 argument: AsyncTask[_ = expr]()".into(),
                    });
                }
                let thunk = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_async_task_new".to_string(),
                    vec![thunk],
                ));
                Ok(result)
            }
            "Par" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Par requires 1 argument: Par[jobs]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_async_task_par".to_string(),
                    vec![list],
                ));
                Ok(result)
            }
            "ParMap" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "ParMap requires 2 arguments: ParMap[items, fn]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let fn_var = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_async_task_par_map".to_string(),
                    vec![list, fn_var],
                ));
                Ok(result)
            }
            "All" => {
                // All[asyncList]() -> taida_async_all(list)
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "All requires 1 argument: All[asyncList]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_async_all".to_string(),
                    vec![list],
                ));
                Ok(result)
            }
            "Race" => {
                // Race[asyncList]() -> taida_async_race(list)
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Race requires 1 argument: Race[asyncList]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_async_race".to_string(),
                    vec![list],
                ));
                Ok(result)
            }
            "Timeout" => {
                // Timeout[async, ms]() -> taida_async_timeout(async, ms)
                // In synchronous mode, just returns the async as-is
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Timeout requires 2 arguments: Timeout[async, ms]()".into(),
                    });
                }
                let async_val = self.lower_expr(func, &type_args[0])?;
                let _ms = self.lower_expr(func, &type_args[1])?;
                // NOTE: Native backend runs in synchronous simulation mode.
                // Timeout has no effect — the async value is returned directly without
                // enforcing the timeout duration. This is a known limitation.
                // A pthread-based timeout could be implemented in the future.
                Ok(async_val)
            }
            "Cancel" => {
                // Cancel[async]() -> taida_async_cancel(async)
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Cancel requires 1 argument: Cancel[async]()".into(),
                    });
                }
                let async_val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_async_cancel".to_string(),
                    vec![async_val],
                ));
                Ok(result)
            }
            "JSON" => {
                // JSON[raw, Schema]() -- Molten Iron: schema-based casting
                // JSON[raw]() or JSON() -> error (schema required)
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "JSON requires a schema type argument: JSON[raw, Schema](). Raw JSON cannot be used without a schema.".to_string(),
                    });
                }
                // Evaluate raw data (first type arg)
                let raw_val = self.lower_expr(func, &type_args[0])?;
                // Resolve schema from second type arg at compile time
                let schema_desc = self.resolve_json_schema_descriptor(&type_args[1])?;
                self.note_json_schema_field_names(&schema_desc);
                // Pass schema descriptor as string constant
                let schema_var = func.alloc_var();
                func.push(IrInst::ConstStr(schema_var, schema_desc));
                // Call: taida_json_schema_cast(raw_str, schema_descriptor) -> Lax[BuchiPack]
                // (the _hooked form also gets the rebuild functions of the
                // TypeDefs with methods the schema can build).
                let hooks = self.lower_json_rebuild_hooks(func, &type_args[1])?;
                let result = func.alloc_var();
                match hooks {
                    Some(hooks) => func.push(IrInst::Call(
                        result,
                        "taida_json_schema_cast_hooked".to_string(),
                        vec![raw_val, schema_var, hooks],
                    )),
                    None => func.push(IrInst::Call(
                        result,
                        "taida_json_schema_cast".to_string(),
                        vec![raw_val, schema_var],
                    )),
                }
                Ok(result)
            }
            "Lax" => {
                // Lax[value]() -> taida_lax_new(value, default)
                // Lax() -> taida_lax_empty(0)
                if type_args.is_empty() {
                    let default_val = func.alloc_var();
                    func.push(IrInst::ConstInt(default_val, 0));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_lax_empty".to_string(),
                        vec![default_val],
                    ));
                    Ok(result)
                } else {
                    use crate::codegen::tag_prop::{
                        TAG_BOOL, TAG_FLOAT, TAG_LIST, TAG_PACK, TAG_STR,
                    };
                    let val = self.lower_expr(func, &type_args[0])?;
                    // The default is the empty value of the payload's kind, as
                    // the reference picks it; both slots record that kind.
                    let checked = self.typed_expr_table.lookup(&type_args[0]).cloned();
                    let tag = checked
                        .as_ref()
                        .and_then(|ty| self.type_runtime_tag(ty))
                        .unwrap_or(-1);
                    let default_val = func.alloc_var();
                    match tag {
                        TAG_FLOAT => func.push(IrInst::ConstFloat(default_val, 0.0)),
                        TAG_BOOL => func.push(IrInst::ConstBool(default_val, false)),
                        TAG_STR => func.push(IrInst::ConstStr(default_val, String::new())),
                        TAG_LIST => func.push(IrInst::Call(
                            default_val,
                            "taida_list_new".to_string(),
                            vec![],
                        )),
                        TAG_PACK if !matches!(checked, Some(crate::types::Type::Bytes)) => {
                            func.push(IrInst::PackNew(default_val, 0))
                        }
                        _ => func.push(IrInst::ConstInt(default_val, 0)),
                    }
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_lax_new".to_string(),
                        vec![val, default_val],
                    ));
                    if tag > 0 && !matches!(checked, Some(crate::types::Type::Bytes)) {
                        func.push(IrInst::PackSetTag(result, 1, tag));
                        func.push(IrInst::PackSetTag(result, 2, tag));
                    }
                    Ok(result)
                }
            }
            "Gorillax" => {
                // Gorillax[value]() -> taida_gorillax_new(value)
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Gorillax requires 1 type argument: Gorillax[value]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_gorillax_new".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            "Molten" => {
                // Molten[]() -> taida_molten_new()
                if !type_args.is_empty() {
                    return Err(LowerError {
                        message: "Molten takes no type arguments: Molten[]()".into(),
                    });
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_molten_new".to_string(), vec![]));
                Ok(result)
            }
            // F56: Moltenize[v]() -> taida_moltenize_new(v),
            // MoltenizeSecret[v]() -> taida_secret_new(v), Redact[v]() -> taida_redact(v).
            "Moltenize" | "MoltenizeSecret" | "Redact" => {
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message: format!(
                            "{} requires exactly 1 type argument: {}[value]",
                            type_name, type_name
                        ),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                let fn_name = match type_name {
                    "MoltenizeSecret" => "taida_secret_new",
                    "Redact" => "taida_redact",
                    _ => "taida_moltenize_new",
                };
                func.push(IrInst::Call(result, fn_name.to_string(), vec![val]));
                Ok(result)
            }
            // F56 Phase 4: secret-aware consumers. HmacSha256[secret, msg]() ->
            // taida_hmac_sha256_secret; ConstantTimeEq[secret, cand]() ->
            // taida_constant_time_eq_secret. The secret's inner bytes are read
            // from the sealed pack and fed to the crypto primitive.
            "HmacSha256" | "ConstantTimeEq" => {
                if type_args.len() != 2 {
                    return Err(LowerError {
                        message: format!("{} requires 2 type arguments: {}[secret, x]", type_name, type_name),
                    });
                }
                let secret = self.lower_expr(func, &type_args[0])?;
                let other = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                let fn_name = if type_name == "HmacSha256" {
                    "taida_hmac_sha256_secret"
                } else {
                    "taida_constant_time_eq_secret"
                };
                func.push(IrInst::Call(result, fn_name.to_string(), vec![secret, other]));
                Ok(result)
            }
            // F56 Phase 4: Reveal[secret, consumer]() is the explicit escape
            // hatch (apply a user function to the revealed plaintext). It is
            // available on the Interpreter and JS backends; the Native / WASM
            // backends gate it with a capability error (calling an arbitrary
            // function value on a revealed secret in a non-list mold is deferred
            // alongside the graph-flow lint that governs Reveal). The secret-
            // aware consumers (HmacSha256 / ConstantTimeEq) cover the common
            // cases on all four backends.
            "Reveal" => Err(LowerError {
                message: "Reveal is supported on the Interpreter and JS backends only. \
                          On Native / WASM, use a secret-aware consumer \
                          (HmacSha256 / ConstantTimeEq) instead of revealing the plaintext."
                    .into(),
            }),
            // F56 Phase 6+: source-side secret producers from file / stdin ->
            // Async[Lax[Secret[_]]]. Implemented on Native (the env producer is
            // already four-backend). WASM has no file/stdin capability, so the
            // emitted runtime call has no WASM prototype and the WASM C emitter
            // rejects it with a capability error (design L0-5); JS rejects in its
            // codegen.
            "MoltenizeSecretFromFile" | "MoltenizeSecretFromInput" => {
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message: format!("{} requires 1 type argument: {}[arg]", type_name, type_name),
                    });
                }
                let arg = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                let fn_name = if type_name == "MoltenizeSecretFromFile" {
                    "taida_os_secret_from_file"
                } else {
                    "taida_os_secret_from_input"
                };
                func.push(IrInst::Call(result, fn_name.to_string(), vec![arg]));
                Ok(result)
            }
            // F56 Phase 2: MoltenizeSecretFromEnv[name]() -> Lax[Secret[Str]].
            "MoltenizeSecretFromEnv" => {
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message:
                            "MoltenizeSecretFromEnv requires 1 type argument: \
                             MoltenizeSecretFromEnv[name]"
                                .into(),
                    });
                }
                let name = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_env_var_secret".to_string(),
                    vec![name],
                ));
                Ok(result)
            }
            // C25B-001: Stream[val]() — minimal native/wasm lowering for
            // string-form parity. Wraps a single value into a completed
            // single-item stream; `Str[stream]()` renders it as
            // `Lax[Str]("Stream[completed: N items]")`. Full lazy-
            // transform chain (Map / Filter / Take / TakeWhile) remains
            // interpreter-only until a future C26 phase.
            "Stream" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Stream requires 1 type argument: Stream[value]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_stream_new".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            "Stub" => {
                if !fields.is_empty() {
                    return Err(LowerError {
                        message: "Stub does not take `()` fields. Use Stub[\"msg\"]".into(),
                    });
                }
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                            .to_string(),
                    });
                }
                let message = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_stub_new".to_string(),
                    vec![message],
                ));
                Ok(result)
            }
            "TODO" => {
                let type_default = if let Some(arg) = type_args.first() {
                    self.lower_todo_default_from_type_arg(func, arg)?
                } else {
                    self.lower_todo_default_unit(func)
                };
                let id = match self.lower_mold_field_expr(func, fields, "id")? {
                    Some(v) => v,
                    None => self.lower_todo_default_unit(func),
                };
                let task = match self.lower_mold_field_expr(func, fields, "task")? {
                    Some(v) => v,
                    None => self.lower_todo_default_unit(func),
                };
                let sol = match self.lower_mold_field_expr(func, fields, "sol")? {
                    Some(v) => v,
                    None => type_default,
                };
                let unm = match self.lower_mold_field_expr(func, fields, "unm")? {
                    Some(v) => v,
                    None => type_default,
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_todo_new".to_string(),
                    vec![id, task, sol, unm],
                ));
                Ok(result)
            }
            "Div" => {
                // Div[a, b]() -> taida_div_mold(a, b) -> returns Lax
                //
                // C26B-011 (Phase 11): if at least one operand is Float,
                // dispatch to `taida_div_mold_f` so the returned Lax is
                // tagged FLOAT (matches interpreter — which returns
                // `Value::Float` in its Lax __value/__default, and JS
                // which sets `__floatHint: true` on the Lax). Int args
                // are widened to f64 via `taida_int_to_float` before the
                // call; the Float-hint runtime uses hardware division
                // so NaN / ±Infinity / denormal propagate per IEEE 754.
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Div requires 2 type arguments".to_string(),
                    });
                }
                let a_raw = self.lower_expr(func, &type_args[0])?;
                let b_raw = self.lower_expr(func, &type_args[1])?;
                // The `default` option is evaluated after the operands (a throw
                // in it reaches the caller) and becomes the Lax's default.
                let custom_default = self.lower_mold_field_expr(func, fields, "default")?;
                let a_is_float = self.expr_returns_float(&type_args[0]);
                let b_is_float = self.expr_returns_float(&type_args[1]);
                if a_is_float || b_is_float {
                    let a = if a_is_float {
                        a_raw
                    } else {
                        let tmp = func.alloc_var();
                        func.push(IrInst::Call(
                            tmp,
                            "taida_int_to_float".to_string(),
                            vec![a_raw],
                        ));
                        tmp
                    };
                    let b = if b_is_float {
                        b_raw
                    } else {
                        let tmp = func.alloc_var();
                        func.push(IrInst::Call(
                            tmp,
                            "taida_int_to_float".to_string(),
                            vec![b_raw],
                        ));
                        tmp
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_div_mold_f".to_string(),
                        vec![a, b],
                    ));
                    self.set_lax_default(func, result, fields, custom_default);
                    return Ok(result);
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_div_mold".to_string(),
                    vec![a_raw, b_raw],
                ));
                self.set_lax_default(func, result, fields, custom_default);
                Ok(result)
            }
            "Mod" => {
                // Mod[a, b]() -> taida_mod_mold(a, b) -> returns Lax
                //
                // C26B-011 (Phase 11): Float-hint dispatch — see the Div
                // comment above. `fmod(a, b)` is used for the Float path
                // to match Rust's `%` operator on f64 / interpreter
                // `Value::Float % Value::Float`.
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Mod requires 2 type arguments".to_string(),
                    });
                }
                let a_raw = self.lower_expr(func, &type_args[0])?;
                let b_raw = self.lower_expr(func, &type_args[1])?;
                // The `default` option is evaluated after the operands (a throw
                // in it reaches the caller) and becomes the Lax's default.
                let custom_default = self.lower_mold_field_expr(func, fields, "default")?;
                let a_is_float = self.expr_returns_float(&type_args[0]);
                let b_is_float = self.expr_returns_float(&type_args[1]);
                if a_is_float || b_is_float {
                    let a = if a_is_float {
                        a_raw
                    } else {
                        let tmp = func.alloc_var();
                        func.push(IrInst::Call(
                            tmp,
                            "taida_int_to_float".to_string(),
                            vec![a_raw],
                        ));
                        tmp
                    };
                    let b = if b_is_float {
                        b_raw
                    } else {
                        let tmp = func.alloc_var();
                        func.push(IrInst::Call(
                            tmp,
                            "taida_int_to_float".to_string(),
                            vec![b_raw],
                        ));
                        tmp
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_mod_mold_f".to_string(),
                        vec![a, b],
                    ));
                    self.set_lax_default(func, result, fields, custom_default);
                    return Ok(result);
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_mod_mold".to_string(),
                    vec![a_raw, b_raw],
                ));
                self.set_lax_default(func, result, fields, custom_default);
                Ok(result)
            }
            // -- Type conversion molds (v0.5.0) --
            "Str" => {
                // Str[x]() -> type conversion to Str, returning Lax.
                //
                // C23-2: The interpreter implements `Str[x]()` as
                // `format!("{}", other)` for any non-primitive value —
                // i.e. List / Pack / Lax / Result are rendered via
                // `to_display_string()`. The previous dispatch fell
                // through to `taida_str_mold_int` for anything that was
                // not compile-time-known Float / Str / Bool, which
                // stringified non-primitive heap pointers as raw
                // integers. For non-primitive cases we now route through
                // `taida_str_mold_any`, which calls the backend's
                // stdout-display helper to obtain the full-form display
                // string (matching the interpreter).
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Str requires 1 argument: Str[x]()".into(),
                    });
                }
                let mut val = self.lower_expr(func, &type_args[0])?;
                let is_function =
                    self.expr_type_tag(&type_args[0]) == crate::codegen::tag_prop::TAG_CLOSURE;
                if is_function {
                    // `<function name>`: only the static type tells a named
                    // function's WASM table index apart from an Int.
                    let shown = func.alloc_var();
                    func.push(IrInst::Call(
                        shown,
                        "taida_function_to_string".to_string(),
                        vec![val],
                    ));
                    val = shown;
                }
                let runtime_fn = if is_function {
                    "taida_str_mold_str"
                } else if self.expr_returns_float(&type_args[0]) {
                    "taida_str_mold_float"
                } else if self.expr_is_string_full(&type_args[0]) {
                    "taida_str_mold_str"
                } else if self.expr_is_likely_bool(&type_args[0]) {
                    "taida_str_mold_bool"
                } else if self.expr_is_int(&type_args[0]) {
                    // Compile-time-known Int: literal, negated literal,
                    // int-typed binding (via `int_vars`), arithmetic on
                    // Int operands, int-returning method/function call.
                    // Using the integer fast path avoids having
                    // `taida_str_mold_any` mis-interpret an Int value as
                    // a pointer:
                    //   - `_looks_like_string` false-positives on small
                    //     positive offsets that fall in the static data
                    //     section (the reason the `expr_is_int_literal`
                    //     shape existed in the initial C23-2 land).
                    //   - `_looks_like_empty_pack` false-positives on
                    //     Int values whose bit pattern lands on an
                    //     8-byte-aligned zero chunk inside the bump
                    //     arena. This was the C23B-003 reopen 4
                    //     regression (`Str[a + b]()` for bump-sized
                    //     sums) — now fixed at the detector level via
                    //     a magic sentinel, but routing through
                    //     `taida_str_mold_int` here still short-circuits
                    //     the whole heuristic stack for any shape we
                    //     can decide at compile time.
                    // Richer check (`expr_is_int`) lives in
                    // `src/codegen/lower/infer.rs` and recognises
                    // bindings / arithmetic / method calls / function
                    // calls with `:Int` return types.
                    "taida_str_mold_int"
                } else {
                    // Unknown at compile time (function parameter, local binding,
                    // BuchiPack literal, list, typed pack, …). Route through the
                    // generic helper which dispatches at runtime via
                    // `taida_stdout_display_string` / `_wasm_stdout_display_string`.
                    "taida_str_mold_any"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![val]));
                Ok(result)
            }
            "Int" => {
                // Int[x]() -> type conversion to Int, returning Lax
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Int requires 1 argument: Int[x]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let (runtime_fn, mut args) = if type_args.len() >= 2 {
                    let base = self.lower_expr(func, &type_args[1])?;
                    ("taida_int_mold_str_base", vec![val, base])
                } else if self.expr_returns_float(&type_args[0]) {
                    ("taida_int_mold_float", vec![val])
                } else if self.expr_is_string_full(&type_args[0]) {
                    ("taida_int_mold_str", vec![val])
                } else if self.expr_is_likely_bool(&type_args[0]) {
                    ("taida_int_mold_bool", vec![val])
                } else {
                    // Dynamic fallback (e.g. function parameters/local vars):
                    // detect string/int at runtime.
                    ("taida_int_mold_auto", vec![val])
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    runtime_fn.to_string(),
                    std::mem::take(&mut args),
                ));
                Ok(result)
            }
            "Float" => {
                // Float[x]() -> type conversion to Float, returning Lax
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Float requires 1 argument: Float[x]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let runtime_fn = if self.expr_returns_float(&type_args[0]) {
                    "taida_float_mold_float"
                } else if self.expr_is_string_full(&type_args[0]) {
                    "taida_float_mold_str"
                } else if self.expr_is_likely_bool(&type_args[0]) {
                    "taida_float_mold_bool"
                } else {
                    // Default: Int->Float promotion
                    "taida_float_mold_int"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![val]));
                Ok(result)
            }
            "Bool" => {
                // Bool[x]() -> type conversion to Bool, returning Lax
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Bool requires 1 argument: Bool[x]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let runtime_fn = if self.expr_returns_float(&type_args[0]) {
                    "taida_bool_mold_float"
                } else if self.expr_is_string_full(&type_args[0]) {
                    "taida_bool_mold_str"
                } else if self.expr_is_likely_bool(&type_args[0]) {
                    "taida_bool_mold_bool"
                } else {
                    // Default: Int->Bool (!=0)
                    "taida_bool_mold_int"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![val]));
                Ok(result)
            }
            "UInt8" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "UInt8 requires 1 argument: UInt8[x]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let runtime_fn = if self.expr_returns_float(&type_args[0]) {
                    "taida_uint8_mold_float"
                } else {
                    "taida_uint8_mold"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![val]));
                Ok(result)
            }
            "Bytes" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Bytes requires 1 argument: Bytes[x]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let fill = match self.lower_mold_field_expr(func, fields, "fill")? {
                    Some(v) => v,
                    None => {
                        let v = func.alloc_var();
                        func.push(IrInst::ConstInt(v, 0));
                        v
                    }
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_bytes_mold".to_string(),
                    vec![val, fill],
                ));
                Ok(result)
            }
            "Char" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Char requires 1 argument: Char[x]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let runtime_fn = if self.expr_is_string_full(&type_args[0]) {
                    "taida_char_mold_str"
                } else {
                    "taida_char_mold_int"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, runtime_fn.to_string(), vec![val]));
                Ok(result)
            }
            "CodePoint" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "CodePoint requires 1 argument: CodePoint[str]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_codepoint_mold_str".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            "Utf8Encode" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Utf8Encode requires 1 argument: Utf8Encode[str]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_utf8_encode_mold".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            "Utf8Decode" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Utf8Decode requires 1 argument: Utf8Decode[bytes]()".into(),
                    });
                }
                let val = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_utf8_decode_mold".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            // -- Optional — ABOLISHED (v0.8.0) --
            "Optional" => {
                Err(LowerError {
                    message: "Optional has been removed. Use Lax[value]() instead. Lax[T] provides the same safety with default value guarantees.".to_string(),
                })
            }
            // -- Result[value, pred]() / Result[value](throw <= error) --
            "Result" => {
                // Result is an operation mold with predicate + throw field.
                // Native layout: BuchiPack with 4 fields:
                //   field 0: __value
                //   field 1: __predicate (0 = no predicate, non-zero = function pointer)
                //   field 2: throw (0 = success, non-zero = error)
                //   field 3: __type ("Result" string)
                let inner_value = if type_args.is_empty() {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 0));
                    v
                } else {
                    self.lower_expr(func, &type_args[0])?
                };
                // Evaluate predicate (2nd type arg) if present
                let predicate = if type_args.len() >= 2 {
                    self.lower_expr(func, &type_args[1])?
                } else {
                    // No predicate — backward compatible (0 = no predicate)
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 0));
                    v
                };
                // Check for throw field in settings
                let throw_val = match self.lower_mold_field_expr(func, fields, "throw")? {
                    Some(v) => v,
                    None => {
                        // No throw field: success (throw = 0 = Unit)
                        let v = func.alloc_var();
                        func.push(IrInst::ConstInt(v, 0));
                        v
                    }
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_result_create".to_string(),
                    vec![inner_value, throw_val, predicate],
                ));
                if let Some(value_expr) = type_args.first() {
                    let value_tag = self.expr_type_tag(value_expr);
                    if value_tag != 0 && value_tag != -1 {
                        func.push(IrInst::PackSetTag(result, 0, value_tag));
                    }
                }
                Ok(result)
            }
            // -- OS input molds (taida-lang/os) --
            "Read" => {
                // Read[path]() -> taida_os_read(path) -> Lax[Str]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Read requires 1 argument: Read[path]()".into(),
                    });
                }
                let path = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_read".to_string(),
                    vec![path],
                ));
                Ok(result)
            }
            "ListDir" => {
                // ListDir[path]() -> taida_os_list_dir(path) -> Lax[@[Str]]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "ListDir requires 1 argument: ListDir[path]()".into(),
                    });
                }
                let path = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_list_dir".to_string(),
                    vec![path],
                ));
                Ok(result)
            }
            "Stat" => {
                // Stat[path]() -> taida_os_stat(path) -> Lax[@(size, modified, isDir)]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Stat requires 1 argument: Stat[path]()".into(),
                    });
                }
                let path = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_stat".to_string(),
                    vec![path],
                ));
                Ok(result)
            }
            "Exists" => {
                // Exists[path]() -> taida_os_exists(path) -> Result[Bool]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Exists requires 1 argument: Exists[path]()".into(),
                    });
                }
                let path = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_exists".to_string(),
                    vec![path],
                ));
                Ok(result)
            }
            "EnvVar" => {
                // EnvVar[name]() -> taida_os_env_var(name) -> Lax[Str]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "EnvVar requires 1 argument: EnvVar[name]()".into(),
                    });
                }
                let name = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_env_var".to_string(),
                    vec![name],
                ));
                Ok(result)
            }
            // -- Phase 2: Async OS molds --
            "ReadAsync" => {
                // ReadAsync[path]() -> taida_os_read_async(path) -> Async[Lax[Str]]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "ReadAsync requires 1 argument: ReadAsync[path]()".into(),
                    });
                }
                let path = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_read_async".to_string(),
                    vec![path],
                ));
                Ok(result)
            }
            "HttpGet" => {
                // HttpGet[url]() -> taida_os_http_get(url) -> Async[Lax[@(status, body, headers)]]
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "HttpGet requires 1 argument: HttpGet[url]()".into(),
                    });
                }
                let url = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_http_get".to_string(),
                    vec![url],
                ));
                Ok(result)
            }
            "HttpPost" => {
                // HttpPost[url, body]() -> taida_os_http_post(url, body) -> Async[Lax[@(status, body, headers)]]
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "HttpPost requires 2 arguments: HttpPost[url, body]()".into(),
                    });
                }
                let url = self.lower_expr(func, &type_args[0])?;
                let body = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_http_post".to_string(),
                    vec![url, body],
                ));
                Ok(result)
            }
            "HttpRequest" => {
                // HttpRequest[method, url](headers <= @(...), body <= "...") -> Async[Lax[@(...)]]
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message:
                            "HttpRequest requires at least 2 arguments: HttpRequest[method, url]()"
                                .into(),
                    });
                }
                let method = self.lower_expr(func, &type_args[0])?;
                let url = self.lower_expr(func, &type_args[1])?;

                let headers_var = match self.lower_mold_field_expr(func, fields, "headers")? {
                    Some(v) => v,
                    None => {
                        let zero = func.alloc_var();
                        func.push(IrInst::ConstInt(zero, 0));
                        let empty_headers = func.alloc_var();
                        func.push(IrInst::Call(
                            empty_headers,
                            "taida_pack_new".to_string(),
                            vec![zero],
                        ));
                        empty_headers
                    }
                };

                // C20-5 (ROOT-15 / C20B-012): the undocumented legacy 3rd type arg
                // body fallback (`HttpRequest["POST", url, body]()`) was removed
                // here to align Native lowering with Interpreter and JS codegen,
                // which only ever consult the `body <= ...` field. Keeping the
                // legacy branch made Native silently send a request body that
                // the other two backends left empty — a cross-backend parity
                // trap. No Taida code in this tree uses the 3-type-arg shape;
                // callers must migrate to `HttpRequest["POST", url](body <= ...)`.
                let body_var = if let Some(v) = self.lower_mold_field_expr(func, fields, "body")? {
                    v
                } else {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstStr(v, String::new()));
                    v
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_os_http_request".to_string(),
                    vec![method, url, headers_var, body_var],
                ));
                Ok(result)
            }
            "HostCapability" => {
                if type_args.len() != 2 {
                    return Err(LowerError {
                        message:
                            "HostCapability requires 2 arguments: HostCapability[name, kind]()"
                                .into(),
                    });
                }
                let name = self.lower_expr(func, &type_args[0])?;
                let kind = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_abi_host_capability".to_string(),
                    vec![name, kind],
                ));
                Ok(result)
            }
            "InCage" => {
                // F62B-024: push one step onto the builder. The step is the
                // exact `taida_abi_host_step` descriptor `HostStep[...]()`
                // produces (the per-step wire schema resolves statically
                // from the args expression, like the direct form).
                if type_args.len() != 3 {
                    return Err(LowerError {
                        message: "InCage requires 3 arguments: InCage[builder, method, args]()"
                            .into(),
                    });
                }
                let builder = self.lower_expr(func, &type_args[0])?;
                let method = self.lower_expr(func, &type_args[1])?;
                let args = self.lower_expr(func, &type_args[2])?;
                let args_schema_desc = self.resolve_wire_schema_descriptor(&type_args[2])?;
                let args_schema = func.alloc_var();
                func.push(IrInst::ConstStr(args_schema, args_schema_desc));
                let step = func.alloc_var();
                func.push(IrInst::Call(
                    step,
                    "taida_abi_host_step".to_string(),
                    vec![method, args, args_schema],
                ));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_cage_builder_push".to_string(),
                    vec![builder, step],
                ));
                Ok(result)
            }
            "Uncage" => {
                // F62B-024: final chain step (arg-less) + fire the host cage.
                if type_args.len() != 3 {
                    return Err(LowerError {
                        message: "Uncage requires 3 arguments: Uncage[builder, method, Out]()"
                            .into(),
                    });
                }
                let builder = self.lower_expr(func, &type_args[0])?;
                let method = self.lower_expr(func, &type_args[1])?;
                // F62B-038 #10: synthetic node — do not reuse the method
                // argument's node id in the node-id-keyed TypedExprTable.
                let empty_args_expr = crate::parser::Expr::ListLit(
                    Vec::new(),
                    type_args[1]
                        .span()
                        .clone()
                        .with_node_id(crate::parser::SYNTHETIC_NODE_ID_BASE),
                );
                let args = self.lower_expr(func, &empty_args_expr)?;
                let args_schema_desc = self.resolve_wire_schema_descriptor(&empty_args_expr)?;
                let args_schema = func.alloc_var();
                func.push(IrInst::ConstStr(args_schema, args_schema_desc));
                let final_step = func.alloc_var();
                func.push(IrInst::Call(
                    final_step,
                    "taida_abi_host_step".to_string(),
                    vec![method, args, args_schema],
                ));
                let out_schema = self.lower_out_schema_operand(func, &type_args[2])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_cage_builder_fire".to_string(),
                    vec![builder, final_step, out_schema],
                ));
                Ok(result)
            }
            "HostStep" => {
                if type_args.len() != 2 {
                    return Err(LowerError {
                        message: "HostStep requires 2 arguments: HostStep[method, args]()".into(),
                    });
                }
                let method = self.lower_expr(func, &type_args[0])?;
                let args = self.lower_expr(func, &type_args[1])?;
                let args_schema_desc = self.resolve_wire_schema_descriptor(&type_args[1])?;
                let args_schema = func.alloc_var();
                func.push(IrInst::ConstStr(args_schema, args_schema_desc));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_abi_host_step".to_string(),
                    vec![method, args, args_schema],
                ));
                Ok(result)
            }
            "HostCall" => {
                if type_args.len() != 2 {
                    return Err(LowerError {
                        message: "HostCall requires 2 arguments: HostCall[steps, Out]()".into(),
                    });
                }
                let steps = self.lower_expr(func, &type_args[0])?;
                let schema = self.lower_out_schema_operand(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_abi_host_call".to_string(),
                    vec![steps, schema],
                ));
                Ok(result)
            }
            "Cage" => {
                // F62B-024: 1-arg form opens a CageBuilder chain.
                if type_args.len() == 1 {
                    let subject = self.lower_expr(func, &type_args[0])?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_cage_builder_new".to_string(),
                        vec![subject],
                    ));
                    return Ok(result);
                }
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Cage requires a subject: Cage[subject]() for a builder chain, or Cage[subject, runner]() for a direct call".into(),
                    });
                }
                if matches!(&type_args[1], Expr::MoldInst(name, _, _, _) if name == "HostCall") {
                    let subject = self.lower_expr(func, &type_args[0])?;
                    let runner = self.lower_expr(func, &type_args[1])?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_abi_host_cage".to_string(),
                        vec![subject, runner],
                    ));
                    return Ok(result);
                }
                // Cage[subject, runner](): JS-only CageRilla descriptors are
                // rejected while lowering the runner expression. Direct native
                // function/lambda runners are not part of the Cage contract.
                let _subject = self.lower_expr(func, &type_args[0])?;
                let _runner = self.lower_expr(func, &type_args[1])?;
                Err(LowerError {
                    message: "Cage runner must be a CageRilla descriptor; direct native functions and lambdas are not supported".into(),
                })
            }
            // C18-3: Ordinal[<enum_value>]() — explicit Enum → Int.
            // On Native, Enum values are already stored as int32 ordinals
            // (`Value::Int(ordinal)` in the interpreter is `int` in C).
            // The lowering is therefore an identity on the sole argument
            // — `Ordinal[Status:Running()]()` evaluates to the ordinal
            // value which is the Int(n) representation. Arity-1 only.
            //
            // C18B-005 fix: when the static type of the argument is known
            // and is NOT a registered Enum, emit a `taida_runtime_panic`
            // call so non-Enum inputs are rejected at run time with the
            // same message shape as the interpreter. Native doesn't carry
            // a runtime tag distinguishing "Enum ordinal int" from "plain
            // int", so we rely on compile-time type propagation via
            // `expr_enum_type_name` / `expr_type_tag`. When the type is
            // genuinely unknown at lowering time we keep identity
            // semantics (graceful degradation — same behaviour as the
            // post-fix JS path when runtime information is missing).
            "Ordinal" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message:
                            "Ordinal requires 1 argument: Ordinal[<enum_value>]()"
                                .into(),
                    });
                }
                let is_known_enum = self
                    .expr_enum_type_name(&type_args[0])
                    .is_some();
                // `expr_type_tag`: 0=Int, 1=Float, 2=Bool, 3=Str,
                // 4=Pack, 5=List, 6=Closure, -1=Unknown.
                //
                // Enum ordinals are stored as 0 (Int) at the IR level,
                // so static_tag==0 is ambiguous with "plain Int misuse"
                // and must also be rejected when the expression is
                // NOT an Enum expression we recognise. `-1` (Unknown)
                // keeps the historical identity behaviour — we cannot
                // prove misuse at compile time without sacrificing a
                // large amount of legitimate cross-function Enum flow.
                let static_tag = self.expr_type_tag(&type_args[0]);
                let static_nonenum_misuse = !is_known_enum && static_tag >= 0;
                if static_nonenum_misuse {
                    let type_label = match static_tag {
                        0 => "Int",
                        1 => "Float",
                        2 => "Bool",
                        3 => "Str",
                        4 => "Pack",
                        5 => "List",
                        6 => "Function",
                        _ => "unknown",
                    };
                    let msg = format!(
                        "Ordinal: argument must be an Enum value, got {}. \
                         Hint: pass an Enum variant such as `Ordinal[Color:Red()]()`.",
                        type_label
                    );
                    let msg_var = func.alloc_var();
                    func.push(IrInst::ConstStr(msg_var, msg));
                    let panic_dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        panic_dummy,
                        "taida_runtime_panic".to_string(),
                        vec![msg_var],
                    ));
                    // `taida_runtime_panic` never returns, but the IR
                    // flow graph expects a value from this expression.
                    // Emit a zero to keep downstream lowering happy.
                    let z = func.alloc_var();
                    func.push(IrInst::ConstInt(z, 0));
                    return Ok(z);
                }
                let arg_var = self.lower_expr(func, &type_args[0])?;
                Ok(arg_var)
            }

            // B11-5c: If[cond, then, else]() → CondBranch (short-circuit)
            "If" => {
                if type_args.len() < 3 {
                    return Err(LowerError {
                        message:
                            "If requires 3 arguments: If[condition, then_value, else_value]()"
                                .into(),
                    });
                }
                let cond_var = self.lower_expr(func, &type_args[0])?;
                let result_var = func.alloc_var();

                // Then branch: only evaluate type_args[1]
                let (then_body, then_result) = {
                    let saved = std::mem::take(&mut func.body);
                    let r = self.lower_expr(func, &type_args[1])?;
                    let body = std::mem::replace(&mut func.body, saved);
                    (body, r)
                };

                // Else branch: only evaluate type_args[2]
                let (else_body, else_result) = {
                    let saved = std::mem::take(&mut func.body);
                    let r = self.lower_expr(func, &type_args[2])?;
                    let body = std::mem::replace(&mut func.body, saved);
                    (body, r)
                };

                func.push(IrInst::CondBranch(
                    result_var,
                    vec![
                        crate::codegen::ir::CondArm {
                            condition: Some(cond_var),
                            body: then_body,
                            result: then_result,
                        },
                        crate::codegen::ir::CondArm {
                            condition: None,
                            body: else_body,
                            result: else_result,
                        },
                    ],
                ));

                Ok(result_var)
            }

            // ── B11-6d: TypeIs[value, :TypeName]() → compile-time type check ──
            "TypeIs" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "TypeIs requires 2 arguments: TypeIs[value, :TypeName]()".into(),
                    });
                }
                let result = func.alloc_var();
                match &type_args[1] {
                    // Enum variant check: compare ordinals at runtime
                    Expr::TypeLiteral(enum_name, Some(variant_name), _) => {
                        let val_var = self.lower_expr(func, &type_args[0])?;
                        let ordinal: usize = self
                            .enum_defs
                            .get(enum_name.as_str())
                            .and_then(|variants: &Vec<String>| {
                                variants.iter().position(|v| v == variant_name)
                            })
                            .unwrap_or(usize::MAX);
                        let ord_var = func.alloc_var();
                        func.push(IrInst::ConstInt(ord_var, ordinal as i64));
                        // Emit comparison: val == ordinal → bool
                        func.push(IrInst::Call(
                            result,
                            "taida_int_eq".to_string(),
                            vec![val_var, ord_var],
                        ));
                    }
                    // Primitive type check: use compile-time type analysis
                    Expr::TypeLiteral(type_name, None, _) => {
                        // The checker's type of the operand answers a check
                        // against a primitive type outright, whatever else
                        // the operand is (a list, an enum, a pack).
                        if let Some(is_match) =
                            self.static_primitive_type_is(&type_args[0], type_name)
                        {
                            func.push(IrInst::ConstBool(result, is_match));
                            return Ok(result);
                        }
                        if let Expr::Ident(name, _) = &type_args[0] {
                            let tracked_match = if self.int_vars.contains(name) {
                                Some(matches!(type_name.as_str(), "Int" | "Num"))
                            } else if self.float_vars.contains(name) {
                                Some(matches!(type_name.as_str(), "Float" | "Num"))
                            } else if self.bool_vars.contains(name) {
                                Some(type_name == "Bool")
                            } else if self.string_vars.contains(name) {
                                Some(type_name == "Str")
                            } else {
                                None
                            };
                            if let Some(is_match) = tracked_match {
                                func.push(IrInst::ConstBool(result, is_match));
                                return Ok(result);
                            }
                        }

                        if let Some(known_ty) = self.typed_expr_table.lookup(&type_args[0]) {
                            let known_match = match (known_ty, type_name.as_str()) {
                                (crate::types::Type::Int, "Int" | "Num") => Some(true),
                                (crate::types::Type::Float, "Float" | "Num") => Some(true),
                                (crate::types::Type::Bool, "Bool") => Some(true),
                                (crate::types::Type::Str, "Str") => Some(true),
                                (crate::types::Type::Int, "Float" | "Bool" | "Str") => Some(false),
                                (crate::types::Type::Float, "Int" | "Bool" | "Str") => Some(false),
                                (crate::types::Type::Bool, "Int" | "Float" | "Num" | "Str") => {
                                    Some(false)
                                }
                                (crate::types::Type::Str, "Int" | "Float" | "Num" | "Bool") => {
                                    Some(false)
                                }
                                _ => None,
                            };
                            if let Some(is_match) = known_match {
                                func.push(IrInst::ConstBool(result, is_match));
                                return Ok(result);
                            }
                        }

                        // C12B-022: If the operand is a function parameter whose
                        // runtime tag is threaded through `param_tag_vars`
                        // (caller-propagated type tag), prefer a runtime tag
                        // comparison over compile-time inference. The existing
                        // compile-time branches assume Ident resolves to a known
                        // static type, which is false for generic `is_foo v =
                        // TypeIs[v, :T]()` patterns where `v` can be any type.
                        //
                        // `taida_get_param_tag_primitive_match(tag, expected)`
                        // handles the compound `Num` case (tag == INT || tag == FLOAT)
                        // and returns 0 (false) for UNKNOWN(-1) tags so the
                        // behaviour on older call sites that do not propagate
                        // tags is preserved (and matches pre-C12B-022 output on
                        // literal paths, because the compile-time branches
                        // above still fire when the arg is a literal).
                        let param_tag = self.get_param_tag_var(&type_args[0]);
                        let primitive_tag: Option<i64> = match type_name.as_str() {
                            "Int" => Some(0),
                            "Float" => Some(1),
                            "Bool" => Some(2),
                            "Str" => Some(3),
                            "Num" => Some(-10), // sentinel: handled by runtime as INT|FLOAT
                            _ => None,
                        };

                        if let (Some(tag_var), Some(expected_tag)) = (param_tag, primitive_tag) {
                            let expected_var = func.alloc_var();
                            func.push(IrInst::ConstInt(expected_var, expected_tag));
                            func.push(IrInst::Call(
                                result,
                                "taida_primitive_tag_match".to_string(),
                                vec![tag_var, expected_var],
                            ));
                        } else {
                            let is_match = match type_name.as_str() {
                                "Int" => {
                                    // Int: expression produces an int AND is not a bool
                                    match &type_args[0] {
                                        Expr::IntLit(_, _) => Some(true),
                                        Expr::FloatLit(_, _) => Some(false),
                                        Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => {
                                            Some(false)
                                        }
                                        Expr::BoolLit(_, _) => Some(false),
                                        Expr::EnumVariant(_, _, _) => Some(false),
                                        _ if self.expr_is_likely_bool(&type_args[0]) => Some(false),
                                        _ => {
                                            // Check if the expression is a known string type
                                            if self.expr_is_string_full(&type_args[0]) {
                                                Some(false)
                                            } else {
                                                // Assume Int for non-bool, non-string unboxed values
                                                Some(true)
                                            }
                                        }
                                    }
                                }
                                "Float" => match &type_args[0] {
                                    Expr::FloatLit(_, _) => Some(true),
                                    _ => Some(false),
                                },
                                "Num" => match &type_args[0] {
                                    Expr::IntLit(_, _) | Expr::FloatLit(_, _) => Some(true),
                                    Expr::BoolLit(_, _) => Some(false),
                                    Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => Some(false),
                                    _ if self.expr_is_likely_bool(&type_args[0]) => Some(false),
                                    _ if self.expr_is_string_full(&type_args[0]) => Some(false),
                                    _ => Some(true),
                                },
                                "Str" => match &type_args[0] {
                                    Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => Some(true),
                                    _ if self.expr_is_string_full(&type_args[0]) => Some(true),
                                    Expr::IntLit(_, _)
                                    | Expr::FloatLit(_, _)
                                    | Expr::BoolLit(_, _) => Some(false),
                                    _ => Some(false),
                                },
                                "Bool" => match &type_args[0] {
                                    Expr::BoolLit(_, _) => Some(true),
                                    _ if self.expr_is_likely_bool(&type_args[0]) => Some(true),
                                    Expr::IntLit(_, _)
                                    | Expr::FloatLit(_, _)
                                    | Expr::StringLit(_, _)
                                    | Expr::TemplateLit(_, _) => Some(false),
                                    _ => Some(false),
                                },
                                "Bytes" => Some(false), // Bytes is rare, default false
                                // B11B-015: Error and named types need runtime check
                                "Error" => None,
                                _ => None,
                            };
                            if let Some(b) = is_match {
                                func.push(IrInst::ConstBool(result, b));
                            } else {
                                // Runtime check via taida_typeis_named
                                let val_var = self.lower_expr(func, &type_args[0])?;
                                let type_str = self.emit_type_name_tag(func, type_name);
                                func.push(IrInst::Call(
                                    result,
                                    "taida_typeis_named".to_string(),
                                    vec![val_var, type_str],
                                ));
                            }
                        }
                    }
                    _ => {
                        func.push(IrInst::ConstBool(result, false));
                    }
                }
                Ok(result)
            }

            // ── B11-6d: TypeExtends[:TypeA, :TypeB]() → compile-time type relationship ──
            "TypeExtends" => {
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message:
                            "TypeExtends requires 2 arguments: TypeExtends[:TypeA, :TypeB]()".into(),
                    });
                }
                let type_a = match &type_args[0] {
                    Expr::TypeLiteral(name, _, _) => name.clone(),
                    _ => String::new(),
                };
                let type_b = match &type_args[1] {
                    Expr::TypeLiteral(name, _, _) => name.clone(),
                    _ => String::new(),
                };
                let extends = if type_a == type_b {
                    true
                } else {
                    match (type_a.as_str(), type_b.as_str()) {
                        ("Int", "Num") | ("Float", "Num") | ("Int", "Float") => true,
                        (a, b) if !a.is_empty() && !b.is_empty() => {
                            // Check inheritance chain
                            self.check_type_inheritance(a, b)
                        }
                        _ => false,
                    }
                };
                let result = func.alloc_var();
                func.push(IrInst::ConstBool(result, extends));
                Ok(result)
            }

            "TypeName" => {
                if type_args.len() != 1 {
                    return Err(LowerError {
                        message: format!(
                            "TypeName requires 1 argument: TypeName[value](), got {}",
                            type_args.len()
                        ),
                    });
                }
                let arg = &type_args[0];
                if let Expr::EnumVariant(_, variant_name, _) = arg {
                    let result = func.alloc_var();
                    func.push(IrInst::ConstStr(result, variant_name.clone()));
                    return Ok(result);
                }
                let arg_var = self.lower_expr(func, arg)?;
                let tag = self.expr_type_tag(arg);
                let tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(tag_var, tag));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_type_name".to_string(),
                    vec![arg_var, tag_var],
                ));
                Ok(result)
            }

            // JS-only molds -- error in native backend
            "JSGet" | "JSCall" | "JSCallAsync" | "JSNew" | "JSSet" | "JSBind"
            | "JSSpread" => Err(LowerError {
                message: format!(
                    "{} is only available in the JS transpiler backend.",
                    type_name
                ),
            }),
            "JSRilla" | "FileRilla" | "BuildRilla" | "CageRilla" => Err(LowerError {
                message: format!(
                    "{} is an abstract CageRilla descriptor. Use JSGet/JSCall/JSCallAsync/JSNew/JSSet/JSBind/JSSpread.",
                    type_name
                ),
            }),
            _ => {
                let Some(mold_def) = self.mold_defs.get(type_name).cloned() else {
                    // User-defined functions may be called with mold syntax.
                    // Lower `Fn[args]()` as a plain positional function call;
                    // named fields are rejected because functions have no
                    // named-field ABI.
                    if self.user_funcs.contains(type_name) {
                        let is_generic = self.generic_fn_type_params.contains_key(type_name);
                        if is_generic || !fields.is_empty() {
                            // F62B-021: explicit type arguments —
                            // `genfn[T1, T2](args)`. The `[]` carries type
                            // arguments (erased except for schema-passing
                            // type params, whose resolved schema descriptors
                            // are appended as hidden string arguments) and
                            // `()` carries the positional value arguments.
                            let mut arg_exprs = Vec::with_capacity(fields.len());
                            let mut all_positional = true;
                            for field in fields {
                                let is_positional = field.name.starts_with('_')
                                    && field.name[1..].chars().all(|c| c.is_ascii_digit());
                                if !is_positional {
                                    all_positional = false;
                                    break;
                                }
                                arg_exprs.push(field.value.clone());
                            }
                            if all_positional
                                && let Some(needed) =
                                    self.generic_schema_params.get(type_name).cloned()
                            {
                                let declared = self
                                    .generic_fn_type_params
                                    .get(type_name)
                                    .cloned()
                                    .unwrap_or_default();
                                for type_param in needed {
                                    let Some(idx) =
                                        declared.iter().position(|n| n == &type_param)
                                    else {
                                        continue;
                                    };
                                    let Some(type_arg) = type_args.get(idx) else {
                                        return Err(LowerError {
                                            message: format!(
                                                "explicit-type-argument call '{}' is missing the type argument for '{}'",
                                                type_name, type_param
                                            ),
                                        });
                                    };
                                    // Forward an enclosing hidden param when
                                    // the type argument is itself an abstract
                                    // type parameter (recursive generic call);
                                    // otherwise resolve the schema now.
                                    // F62B-038 #10: both branches synthesise a
                                    // new node — give it a synthetic node id
                                    // instead of reusing the source type_arg's
                                    // (TypedExprTable is keyed by node id
                                    // alone, so a cloned id would resolve to
                                    // the type argument's recorded type).
                                    let schema_expr = if let Expr::Ident(arg_name, sp) = type_arg
                                        && let Some(hidden) =
                                            self.current_schema_params.get(arg_name)
                                    {
                                        Expr::Ident(
                                            hidden.clone(),
                                            sp.clone().with_node_id(
                                                crate::parser::SYNTHETIC_NODE_ID_BASE,
                                            ),
                                        )
                                    } else {
                                        let desc =
                                            self.resolve_json_schema_descriptor(type_arg)?;
                                        Expr::StringLit(
                                            desc,
                                            type_arg.span().clone().with_node_id(
                                                crate::parser::SYNTHETIC_NODE_ID_BASE,
                                            ),
                                        )
                                    };
                                    arg_exprs.push(schema_expr);
                                }
                            }
                            if all_positional {
                                // F62B-041 (the lowering twin of the
                                // interpreter's combined-form guard): for a
                                // NON-generic function the bracket is the
                                // legacy positional call (`fn[arg1, arg2]()`),
                                // so combining it with `()` arguments
                                // (`fn[1](2)`) is ambiguous. The checker
                                // rejects this statically; without this guard
                                // an unchecked build silently dropped the
                                // bracket values and lowered `fn(2)`.
                                if !is_generic && !type_args.is_empty() {
                                    return Err(LowerError {
                                        message: format!(
                                            "Non-generic function '{}' cannot take both \
                                             bracket values and call arguments. \
                                             Call it as {}(arg1, arg2) or with the \
                                             positional bracket form {}[arg1, arg2]().",
                                            type_name, type_name, type_name
                                        ),
                                    });
                                }
                                let mut callee = Expr::Ident(
                                    type_name.to_string(),
                                    crate::lexer::Span::new(0, 0, 0, 0),
                                );
                                crate::parser::reassign_expr_node_ids(
                                    &mut callee,
                                    // F62B-038 #10: synthetic ids — a fresh
                                    // allocator restarts at 1 and collides
                                    // with real parser nodes in the
                                    // node-id-keyed TypedExprTable.
                                    &mut crate::parser::NodeIdAllocator::synthetic(),
                                );
                                return self.lower_func_call(func, &callee, &arg_exprs);
                            }
                            return Err(LowerError {
                                message: format!(
                                    "User-defined function '{}' called via mold syntax \
                                     cannot accept named fields '()'. \
                                     Pass arguments positionally: {}[arg1, arg2]() or {}(arg1, arg2).",
                                    type_name, type_name, type_name
                                ),
                            });
                        }
                        let mut callee = Expr::Ident(
                            type_name.to_string(),
                            crate::lexer::Span::new(0, 0, 0, 0),
                        );
                        crate::parser::reassign_expr_node_ids(
                            &mut callee,
                            // F62B-038 #10: synthetic ids — a fresh
                            // allocator restarts at 1 and collides with real
                            // parser nodes in the node-id-keyed TypedExprTable.
                            &mut crate::parser::NodeIdAllocator::synthetic(),
                        );
                        return self.lower_func_call(func, &callee, type_args);
                    }
                    return Err(LowerError {
                        message: format!("unsupported mold type: {}", type_name),
                    });
                };

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

                let mut positional_vars = Vec::<IrVar>::new();
                for arg in type_args {
                    positional_vars.push(self.lower_expr(func, arg)?);
                }

                let mut named_vars = std::collections::HashMap::<String, IrVar>::new();
                let mut named_order = Vec::<String>::new();
                for field in fields {
                    let v = self.lower_expr(func, &field.value)?;
                    if !named_vars.contains_key(&field.name) {
                        named_order.push(field.name.clone());
                    }
                    named_vars.insert(field.name.clone(), v);
                }

                let bind_nonce = func.next_var;
                let mut bound_vars = std::collections::HashMap::<String, IrVar>::new();
                let mut alias_map = std::collections::HashMap::<String, String>::new();
                let mut bind_order = Vec::<String>::new();
                let bind_field = |field_name: &str,
                                  value_var: IrVar,
                                  func: &mut IrFunction,
                                  bound_vars: &mut std::collections::HashMap<String, IrVar>,
                                  alias_map: &mut std::collections::HashMap<String, String>,
                                  bind_order: &mut Vec<String>| {
                    bound_vars.insert(field_name.to_string(), value_var);
                    if !bind_order.iter().any(|n| n == field_name) {
                        bind_order.push(field_name.to_string());
                    }
                    let alias = format!(
                        "{HIDDEN}mold_bind_{}_{}_{}",
                        bind_nonce, type_name, field_name
                    );
                    func.push(IrInst::DefVar(alias.clone(), value_var));
                    alias_map.insert(field_name.to_string(), alias);
                };
                let zero_var = |func: &mut IrFunction| {
                    let z = func.alloc_var();
                    func.push(IrInst::ConstInt(z, 0));
                    z
                };

                let filling_var = positional_vars
                    .first()
                    .copied()
                    .unwrap_or_else(|| zero_var(func));
                bind_field(
                    "filling",
                    filling_var,
                    func,
                    &mut bound_vars,
                    &mut alias_map,
                    &mut bind_order,
                );

                let mut consumed = std::collections::HashSet::<String>::new();
                consumed.insert("filling".to_string());

                for (idx, field_def) in required_fields.iter().enumerate() {
                    let value_var = positional_vars
                        .get(idx + 1)
                        .copied()
                        .unwrap_or_else(|| zero_var(func));
                    bind_field(
                        &field_def.name,
                        value_var,
                        func,
                        &mut bound_vars,
                        &mut alias_map,
                        &mut bind_order,
                    );
                    consumed.insert(field_def.name.clone());
                }

                for field_def in &optional_fields {
                    let value_var = if let Some(v) = named_vars.get(&field_def.name).copied() {
                        v
                    } else if let Some(default_expr) = field_def.default_value.as_ref() {
                        let mut rewritten = rewrite_expr_ident_aliases(default_expr, &alias_map);
                        crate::parser::reassign_expr_node_ids(
                            &mut rewritten,
                            // F62B-038 #10: synthetic ids — a fresh
                            // allocator restarts at 1 and collides with real
                            // parser nodes in the node-id-keyed TypedExprTable.
                            &mut crate::parser::NodeIdAllocator::synthetic(),
                        );
                        self.lower_expr(func, &rewritten)?
                    } else {
                        zero_var(func)
                    };
                    bind_field(
                        &field_def.name,
                        value_var,
                        func,
                        &mut bound_vars,
                        &mut alias_map,
                        &mut bind_order,
                    );
                    consumed.insert(field_def.name.clone());
                }

                let mut extra_named = Vec::<String>::new();
                for name in named_order {
                    if name == "filling" || consumed.contains(&name) {
                        continue;
                    }
                    if let Some(v) = named_vars.get(&name).copied() {
                        bind_field(
                            &name,
                            v,
                            func,
                            &mut bound_vars,
                            &mut alias_map,
                            &mut bind_order,
                        );
                        extra_named.push(name);
                    }
                }

                // The reference's layout: `filling`, the declared fields, the
                // extra named fields, `__value`, `__unmold`, `__type`, each
                // slot recording its value's kind.
                let annotation_tag = |this: &Self, field: &crate::parser::FieldDef| {
                    field
                        .type_annotation
                        .as_ref()
                        .map(|ann| this.type_expr_to_tag(ann))
                        .unwrap_or(-1)
                };
                let mut field_tags = std::collections::HashMap::<String, i64>::new();
                if let Some(arg) = type_args.first() {
                    field_tags.insert("filling".to_string(), self.expr_type_tag(arg));
                }
                for (idx, field_def) in required_fields.iter().enumerate() {
                    let tag = match type_args.get(idx + 1) {
                        Some(arg) => self.expr_type_tag(arg),
                        None => annotation_tag(self, field_def),
                    };
                    field_tags.insert(field_def.name.clone(), tag);
                }
                for field_def in &optional_fields {
                    let tag = match fields.iter().rev().find(|f| f.name == field_def.name) {
                        Some(given) => self.expr_type_tag(&given.value),
                        None => annotation_tag(self, field_def),
                    };
                    field_tags.insert(field_def.name.clone(), tag);
                }
                for field in fields {
                    field_tags
                        .entry(field.name.clone())
                        .or_insert_with(|| self.expr_type_tag(&field.value));
                }

                let mut materialized_fields = Vec::<(String, IrVar)>::new();
                materialized_fields.push(("filling".to_string(), filling_var));
                for field_def in non_method_fields.iter().filter(|f| f.name != "filling") {
                    if let Some(v) = bound_vars.get(&field_def.name).copied() {
                        materialized_fields.push((field_def.name.clone(), v));
                    }
                }
                for name in &extra_named {
                    if let Some(v) = bound_vars.get(name).copied() {
                        materialized_fields.push((name.clone(), v));
                    }
                }
                materialized_fields.push(("__value".to_string(), filling_var));
                if let Some(tag) = field_tags.get("filling").copied() {
                    field_tags.insert("__value".to_string(), tag);
                }

                // F62B-034: a custom `unmold` hook compiles into a closure
                // captured over the instance's field values and rides the
                // pack as `__unmold` (the interpreter's convention), so the
                // generic unmold can run it dynamically — before this, the
                // hook never ran natively and `>=>` fell back to the
                // `__value` (filling) channel.
                if let Some(unmold_def) = mold_def
                    .fields
                    .iter()
                    .find(|f| f.is_method && f.name == "unmold")
                    .and_then(|f| f.method_def.clone())
                {
                    let data_names: Vec<String> = bind_order.clone();
                    let capture_prefix =
                        format!("{HIDDEN}moldinst_{}_{}_", type_name, self.lambda_counter);
                    let capture_names: Vec<String> = data_names
                        .iter()
                        .map(|n| format!("{capture_prefix}{n}"))
                        .collect();
                    for (name, cap_name) in data_names.iter().zip(capture_names.iter()) {
                        let v = bound_vars
                            .get(name)
                            .copied()
                            .unwrap_or_else(|| zero_var(func));
                        func.push(IrInst::DefVar(cap_name.clone(), v));
                    }
                    let closure_var = self.lower_type_method_closure(
                        func,
                        type_name,
                        "unmold",
                        &unmold_def,
                        &capture_names,
                        &data_names,
                    )?;
                    materialized_fields.push(("__unmold".to_string(), closure_var));
                    field_tags.insert("__unmold".to_string(), crate::codegen::tag_prop::TAG_CLOSURE);
                    let name_var = func.alloc_var();
                    func.push(IrInst::ConstStr(name_var, "__unmold".to_string()));
                    let closure_fn = func.alloc_var();
                    func.push(IrInst::Call(
                        closure_fn,
                        "taida_closure_get_fn".to_string(),
                        vec![closure_var],
                    ));
                    let dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        dummy,
                        "taida_register_fn_name".to_string(),
                        vec![closure_fn, name_var],
                    ));
                }
                // The mold's other methods ride the instance as closures over
                // its fields, as a class-like type's do.
                let method_defs: Vec<(String, crate::parser::FuncDef)> = mold_def
                    .fields
                    .iter()
                    .filter(|f| f.is_method && f.name != "unmold")
                    .filter_map(|f| f.method_def.clone().map(|m| (f.name.clone(), m)))
                    .collect();
                let data: Vec<(String, IrVar)> = bind_order
                    .iter()
                    .filter_map(|n| bound_vars.get(n).map(|v| (n.clone(), *v)))
                    .collect();
                let method_closures =
                    self.lower_method_closures(func, type_name, "moldinst", &method_defs, &data)?;
                for (method_name, closure_var) in method_closures {
                    field_tags.insert(method_name.clone(), crate::codegen::tag_prop::TAG_CLOSURE);
                    materialized_fields.push((method_name, closure_var));
                }
                let type_var = func.alloc_var();
                func.push(IrInst::ConstStr(type_var, type_name.to_string()));
                materialized_fields.push(("__type".to_string(), type_var));
                field_tags.insert("__type".to_string(), crate::codegen::tag_prop::TAG_STR);

                let pack_var = func.alloc_var();
                func.push(IrInst::PackNew(pack_var, materialized_fields.len()));
                for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
                    self.emit_pack_field_hash(func, pack_var, i, field_name);
                    func.push(IrInst::PackSet(pack_var, i, *field_val));
                    let tag = field_tags.get(field_name).copied().unwrap_or(-1);
                    if tag > 0 {
                        func.push(IrInst::PackSetTag(pack_var, i, tag));
                        // retain-on-store
                        self.emit_retain_if_heap_tag(func, *field_val, tag);
                    }
                }

                if let Some(helper_symbol) = self.mold_solidify_funcs.get(type_name).cloned() {
                    let mut helper_args = Vec::<IrVar>::new();
                    helper_args.push(filling_var);
                    for field_def in &required_fields {
                        helper_args.push(
                            bound_vars
                                .get(&field_def.name)
                                .copied()
                                .unwrap_or_else(|| zero_var(func)),
                        );
                    }
                    for field_def in &optional_fields {
                        helper_args.push(
                            bound_vars
                                .get(&field_def.name)
                                .copied()
                                .unwrap_or_else(|| zero_var(func)),
                        );
                    }
                    helper_args.push(pack_var); // self
                    let result = func.alloc_var();
                    func.push(IrInst::CallUser(result, helper_symbol, helper_args));
                    Ok(result)
                } else {
                    Ok(pack_var)
                }
            }
        }
    }
}

fn rewrite_expr_ident_aliases(
    expr: &crate::parser::Expr,
    aliases: &std::collections::HashMap<String, String>,
) -> crate::parser::Expr {
    use crate::parser::Expr;

    match expr {
        Expr::IntLit(v, s) => Expr::IntLit(*v, s.clone()),
        Expr::FloatLit(v, s) => Expr::FloatLit(*v, s.clone()),
        Expr::StringLit(v, s) => Expr::StringLit(v.clone(), s.clone()),
        Expr::Block(stmts, s) => Expr::Block(stmts.clone(), s.clone()),
        Expr::TemplateLit(v, s) => Expr::TemplateLit(v.clone(), s.clone()),
        Expr::BoolLit(v, s) => Expr::BoolLit(*v, s.clone()),
        Expr::Gorilla(s) => Expr::Gorilla(s.clone()),
        Expr::Ident(name, s) => {
            if let Some(alias) = aliases.get(name) {
                Expr::Ident(alias.clone(), s.clone())
            } else {
                Expr::Ident(name.clone(), s.clone())
            }
        }
        Expr::EnumVariant(enum_name, variant_name, s) => {
            Expr::EnumVariant(enum_name.clone(), variant_name.clone(), s.clone())
        }
        // TypeLiteral passes through unchanged as a compile-time construct.
        Expr::TypeLiteral(name, variant, s) => {
            Expr::TypeLiteral(name.clone(), variant.clone(), s.clone())
        }
        Expr::Placeholder(s) => Expr::Placeholder(s.clone()),
        Expr::Hole(s) => Expr::Hole(s.clone()),
        Expr::BuchiPack(fields, s) => Expr::BuchiPack(
            fields
                .iter()
                .map(|f| crate::parser::BuchiField {
                    name: f.name.clone(),
                    value: rewrite_expr_ident_aliases(&f.value, aliases),
                    span: f.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::ListLit(items, s) => Expr::ListLit(
            items
                .iter()
                .map(|it| rewrite_expr_ident_aliases(it, aliases))
                .collect(),
            s.clone(),
        ),
        Expr::BinaryOp(lhs, op, rhs, s) => Expr::BinaryOp(
            Box::new(rewrite_expr_ident_aliases(lhs, aliases)),
            op.clone(),
            Box::new(rewrite_expr_ident_aliases(rhs, aliases)),
            s.clone(),
        ),
        Expr::UnaryOp(op, inner, s) => Expr::UnaryOp(
            op.clone(),
            Box::new(rewrite_expr_ident_aliases(inner, aliases)),
            s.clone(),
        ),
        Expr::FuncCall(callee, args, s) => Expr::FuncCall(
            Box::new(rewrite_expr_ident_aliases(callee, aliases)),
            args.iter()
                .map(|a| rewrite_expr_ident_aliases(a, aliases))
                .collect(),
            s.clone(),
        ),
        Expr::MethodCall(obj, method, args, s) => Expr::MethodCall(
            Box::new(rewrite_expr_ident_aliases(obj, aliases)),
            method.clone(),
            args.iter()
                .map(|a| rewrite_expr_ident_aliases(a, aliases))
                .collect(),
            s.clone(),
        ),
        Expr::FieldAccess(obj, field, s) => Expr::FieldAccess(
            Box::new(rewrite_expr_ident_aliases(obj, aliases)),
            field.clone(),
            s.clone(),
        ),
        Expr::CondBranch(arms, s) => Expr::CondBranch(
            arms.iter()
                .map(|arm| crate::parser::CondArm {
                    condition: arm
                        .condition
                        .as_ref()
                        .map(|c| rewrite_expr_ident_aliases(c, aliases)),
                    body: arm
                        .body
                        .iter()
                        .map(|stmt| match stmt {
                            crate::parser::Statement::Expr(e) => crate::parser::Statement::Expr(
                                rewrite_expr_ident_aliases(e, aliases),
                            ),
                            other => other.clone(),
                        })
                        .collect(),
                    span: arm.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Pipeline(exprs, s) => Expr::Pipeline(
            exprs
                .iter()
                .map(|e| rewrite_expr_ident_aliases(e, aliases))
                .collect(),
            s.clone(),
        ),
        Expr::MoldInst(name, type_args, fields, s) => Expr::MoldInst(
            name.clone(),
            type_args
                .iter()
                .map(|a| rewrite_expr_ident_aliases(a, aliases))
                .collect(),
            fields
                .iter()
                .map(|f| crate::parser::BuchiField {
                    name: f.name.clone(),
                    value: rewrite_expr_ident_aliases(&f.value, aliases),
                    span: f.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Unmold(inner, s) => Expr::Unmold(
            Box::new(rewrite_expr_ident_aliases(inner, aliases)),
            s.clone(),
        ),
        Expr::Lambda(params, body, s) => Expr::Lambda(
            params.clone(),
            Box::new(rewrite_expr_ident_aliases(body, aliases)),
            s.clone(),
        ),
        Expr::TypeInst(name, fields, s) => Expr::TypeInst(
            name.clone(),
            fields
                .iter()
                .map(|f| crate::parser::BuchiField {
                    name: f.name.clone(),
                    value: rewrite_expr_ident_aliases(&f.value, aliases),
                    span: f.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Throw(inner, s) => Expr::Throw(
            Box::new(rewrite_expr_ident_aliases(inner, aliases)),
            s.clone(),
        ),
    }
}
