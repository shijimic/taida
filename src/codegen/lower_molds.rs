use super::ir::*;
use super::lower::{LowerError, Lowering};
/// Mold instantiation lowering for the Taida native backend.
///
/// Contains `lower_mold_inst` (68 mold types) and mold field processing helpers.
///
/// These are `impl Lowering` methods split from lower.rs for maintainability.
use crate::parser::*;

impl Lowering {
    /// モールドフィールド（オプション引数）から名前指定のフィールドを検索してloweringする
    pub(crate) fn lower_mold_field_bool(
        &mut self,
        _func: &mut IrFunction,
        fields: &[crate::parser::BuchiField],
        name: &str,
        default: bool,
    ) -> Result<bool, LowerError> {
        for field in fields {
            if field.name == name {
                // 静的にリテラル値を評価
                match &field.value {
                    Expr::BoolLit(b, _) => return Ok(*b),
                    Expr::Ident(s, _) if s == "true" => return Ok(true),
                    Expr::Ident(s, _) if s == "false" => return Ok(false),
                    _ => return Ok(default), // 動的値は default にフォールバック
                }
            }
        }
        Ok(default)
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

    /// モールディング型インスタンス化: `Async[val]()`, `AsyncReject[err]()` etc.
    pub(crate) fn lower_mold_inst(
        &mut self,
        func: &mut IrFunction,
        type_name: &str,
        type_args: &[Expr],
        fields: &[crate::parser::BuchiField],
    ) -> Result<IrVar, LowerError> {
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
                let trim_start = self.lower_mold_field_bool(func, fields, "start", true)?;
                let trim_end = self.lower_mold_field_bool(func, fields, "end", true)?;
                let runtime_fn = match (trim_start, trim_end) {
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
                Ok(result)
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
                let replace_all = self.lower_mold_field_bool(func, fields, "all", false)?;
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
                            let v = func.alloc_var();
                            func.push(IrInst::ConstInt(v, -1));
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
                // TF-15: CharAt returns Lax[Str] (matching interpreter)
                let default_val = func.alloc_var();
                func.push(IrInst::ConstStr(default_val, String::new()));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_lax_new".to_string(),
                    vec![raw, default_val],
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
                let pad_char_var = match self.lower_mold_field_str(func, fields, "char")? {
                    Some(c) => {
                        let v = func.alloc_var();
                        func.push(IrInst::ConstStr(v, c));
                        v
                    }
                    None => {
                        let v = func.alloc_var();
                        func.push(IrInst::ConstStr(v, " ".to_string()));
                        v
                    }
                };
                let side_str = self
                    .lower_mold_field_str(func, fields, "side")?
                    .unwrap_or_else(|| "start".to_string());
                let pad_end_flag = func.alloc_var();
                let is_end = if side_str == "end" { 1i64 } else { 0i64 };
                func.push(IrInst::ConstInt(pad_end_flag, is_end));
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
                let min_val = self.lower_expr(func, &type_args[1])?;
                let max_val = self.lower_expr(func, &type_args[2])?;
                let is_float = self.expr_returns_float(&type_args[0])
                    || self.expr_returns_float(&type_args[1])
                    || self.expr_returns_float(&type_args[2]);
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
            // return Float per `src/types/mold_returns.rs` and widen
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
                // `val.log(base)` = `ln(val) / ln(base)`. When base is
                // omitted, interpreter falls through to `val.ln()`; we
                // don't lower that single-arg form here because it
                // would shadow the 2-arg form, and the fixture always
                // supplies both. Single-arg callers should use Ln.
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Log requires 2 arguments at the codegen boundary: \
                                  Log[value, base](). For natural-log use Ln[value]()."
                            .into(),
                    });
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
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_append".to_string(),
                    vec![list, val],
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
                let reverse = self.lower_mold_field_bool(func, fields, "reverse", false)?;
                let desc = self.lower_mold_field_bool(func, fields, "desc", false)?;
                let result = func.alloc_var();
                if let Some(by_var) = by_fn {
                    // Sort by key extraction function: taida_list_sort_by(list, fn)
                    // The function extracts a sort key from each element, then sorts ascending by key.
                    // If reverse or desc is set, we sort descending instead (sort_by returns ascending).
                    func.push(IrInst::Call(result, "taida_list_sort_by".to_string(), vec![list, by_var]));
                    if reverse || desc {
                        // Reverse the result for descending order
                        let reversed = func.alloc_var();
                        func.push(IrInst::Call(reversed, "taida_list_reverse".to_string(), vec![result]));
                        return Ok(reversed);
                    }
                } else {
                    let runtime_fn = if reverse || desc {
                        "taida_list_sort_desc"
                    } else {
                        "taida_list_sort"
                    };
                    func.push(IrInst::Call(result, runtime_fn.to_string(), vec![list]));
                }
                Ok(result)
            }
            "Unique" => {
                if type_args.is_empty() {
                    return Err(LowerError {
                        message: "Unique requires 1 argument: Unique[list]()".into(),
                    });
                }
                let list = self.lower_expr(func, &type_args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_unique".to_string(),
                    vec![list],
                ));
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
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_list_map".to_string(),
                    vec![list, fn_var],
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
                    let val = self.lower_expr(func, &type_args[0])?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_async_err".to_string(),
                        vec![val],
                    ));
                    Ok(result)
                }
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
                // Pass schema descriptor as string constant
                let schema_var = func.alloc_var();
                func.push(IrInst::ConstStr(schema_var, schema_desc));
                // Call: taida_json_schema_cast(raw_str, schema_descriptor) -> Lax[BuchiPack]
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_json_schema_cast".to_string(),
                    vec![raw_val, schema_var],
                ));
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
                    let val = self.lower_expr(func, &type_args[0])?;
                    // default value is 0 for native (Int default)
                    let default_val = func.alloc_var();
                    func.push(IrInst::ConstInt(default_val, 0));
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_lax_new".to_string(),
                        vec![val, default_val],
                    ));
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
                    return Ok(result);
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_div_mold".to_string(),
                    vec![a_raw, b_raw],
                ));
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
                    return Ok(result);
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_mod_mold".to_string(),
                    vec![a_raw, b_raw],
                ));
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
                let val = self.lower_expr(func, &type_args[0])?;
                let runtime_fn = if self.expr_returns_float(&type_args[0]) {
                    "taida_str_mold_float"
                } else if self.expr_is_string_full(&type_args[0]) {
                    "taida_str_mold_str"
                } else if self.expr_is_bool(&type_args[0]) {
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
                } else if self.expr_is_bool(&type_args[0]) {
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
                } else if self.expr_is_bool(&type_args[0]) {
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
                } else if self.expr_is_bool(&type_args[0]) {
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
                // Exists[path]() -> taida_os_exists(path) -> Bool
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
            "Cage" => {
                // Cage[molten, fn]() -> taida_cage_apply(molten, fn)
                if type_args.len() < 2 {
                    return Err(LowerError {
                        message: "Cage requires 2 type arguments: Cage[value, function]".into(),
                    });
                }
                let cage_value = self.lower_expr(func, &type_args[0])?;
                let cage_fn = self.lower_expr(func, &type_args[1])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_cage_apply".to_string(),
                    vec![cage_value, cage_fn],
                ));
                Ok(result)
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
                        6 => "Closure",
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
                        super::ir::CondArm {
                            condition: Some(cond_var),
                            body: then_body,
                            result: then_result,
                        },
                        super::ir::CondArm {
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
                                        _ if self.expr_is_bool(&type_args[0]) => Some(false),
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
                                    _ if self.expr_is_bool(&type_args[0]) => Some(false),
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
                                    _ if self.expr_is_bool(&type_args[0]) => Some(true),
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
                                let type_str = func.alloc_var();
                                func.push(IrInst::ConstStr(type_str, type_name.to_string()));
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

            // JS-only molds -- error in native backend
            "JSNew" | "JSSet" | "JSBind" | "JSSpread" => Err(LowerError {
                message: format!(
                    "{} is only available in the JS transpiler backend.",
                    type_name
                ),
            }),
            _ => {
                let Some(mold_def) = self.mold_defs.get(type_name).cloned() else {
                    // C20B-014 (ROOT-17): user-defined function called via mold syntax.
                    //
                    // Pre-C20B-014, `Fn[args]()` for a user function hit this
                    // `unsupported mold type` error at lowering time, even though
                    // the program is semantically valid (Interpreter wrapped,
                    // JS `__taida_solidify(Fn(...))` accidentally worked).
                    // Fix: when `type_name` is not a mold but *is* a registered
                    // user function, lower as a plain function call with
                    // `type_args` as positional arguments. Named `fields` are
                    // rejected — user functions have no named-field ABI.
                    if self.user_funcs.contains(type_name) {
                        if !fields.is_empty() {
                            return Err(LowerError {
                                message: format!(
                                    "User-defined function '{}' called via mold syntax \
                                     cannot accept named fields '()'. \
                                     Pass arguments positionally: {}[arg1, arg2]() or {}(arg1, arg2).",
                                    type_name, type_name, type_name
                                ),
                            });
                        }
                        let callee = Expr::Ident(
                            type_name.to_string(),
                            crate::lexer::Span::new(0, 0, 0, 0),
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
                        "__taida_mold_bind_{}_{}_{}",
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
                        let rewritten = rewrite_expr_ident_aliases(default_expr, &alias_map);
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

                let mut materialized_fields = Vec::<(String, IrVar)>::new();
                let type_var = func.alloc_var();
                func.push(IrInst::ConstStr(type_var, type_name.to_string()));
                materialized_fields.push(("__type".to_string(), type_var));
                materialized_fields.push(("__value".to_string(), filling_var));
                materialized_fields.push(("filling".to_string(), filling_var));

                for field_def in non_method_fields.iter().filter(|f| f.name != "filling") {
                    if let Some(v) = bound_vars.get(&field_def.name).copied() {
                        materialized_fields.push((field_def.name.clone(), v));
                    }
                }
                for name in extra_named {
                    if let Some(v) = bound_vars.get(&name).copied() {
                        materialized_fields.push((name, v));
                    }
                }

                let pack_var = func.alloc_var();
                func.push(IrInst::PackNew(pack_var, materialized_fields.len()));
                for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
                    self.emit_pack_field_hash(func, pack_var, i, field_name);
                    func.push(IrInst::PackSet(pack_var, i, *field_val));
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
        // B11-6a: TypeLiteral passes through unchanged (compile-time construct)
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
