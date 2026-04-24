use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, StreamStatus, StreamTransform, StreamValue, Value};
/// Operation mold evaluation for the Taida interpreter.
///
/// Contains `try_operation_mold` (Str/Num/List molds) and `try_list_mold_op`
/// (Map/Filter/Fold/TakeWhile/DropWhile/JSON/Async/etc.).
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use crate::parser::Expr;

/// C25B-025 (Phase 5-A): helper for single-argument numeric molds that
/// return Float (Sqrt / Exp / Ln / Sin / Cos / ...). Accepts Int (widens
/// to f64) or Float. Any other value produces a descriptive runtime
/// error including the mold name. Non-value signals (Throw / TailCall)
/// are propagated.
fn eval_unary_math(
    interp: &mut Interpreter,
    type_args: &[Expr],
    name: &str,
    op: fn(f64) -> f64,
) -> Result<Option<Signal>, RuntimeError> {
    if type_args.is_empty() {
        return Err(RuntimeError {
            message: format!("{} requires 1 argument: {}[num]()", name, name),
        });
    }
    let num = match interp.eval_expr(&type_args[0])? {
        Signal::Value(Value::Int(n)) => n as f64,
        Signal::Value(Value::Float(n)) => n,
        Signal::Value(v) => {
            return Err(RuntimeError {
                message: format!("{}: argument must be numeric, got {}", name, v),
            });
        }
        other => return Ok(Some(other)),
    };
    Ok(Some(Signal::Value(Value::Float(op(num)))))
}

fn make_lax_value(has_value: bool, value: Value, default: Value) -> Value {
    Value::BuchiPack(vec![
        ("hasValue".into(), Value::Bool(has_value)),
        ("__value".into(), value),
        ("__default".into(), default),
        ("__type".into(), Value::Str("Lax".into())),
    ])
}

fn make_bytes_cursor(bytes: Vec<u8>, offset: i64) -> Value {
    let clamped = offset.clamp(0, bytes.len() as i64);
    Value::BuchiPack(vec![
        ("bytes".into(), Value::Bytes(bytes.clone())),
        ("offset".into(), Value::Int(clamped)),
        ("length".into(), Value::Int(bytes.len() as i64)),
        ("__type".into(), Value::Str("BytesCursor".into())),
    ])
}

fn make_bytes_cursor_step(value: Value, cursor: Value) -> Value {
    Value::BuchiPack(vec![("value".into(), value), ("cursor".into(), cursor)])
}

fn parse_bytes_cursor(value: Value, mold_name: &str) -> Result<(Vec<u8>, usize), RuntimeError> {
    let Value::BuchiPack(fields) = value else {
        return Err(RuntimeError {
            message: format!("{}: argument must be BytesCursor, got {}", mold_name, value),
        });
    };

    let bytes = match fields.iter().find(|(name, _)| name == "bytes") {
        Some((_, Value::Bytes(v))) => v.clone(),
        Some((_, v)) => {
            return Err(RuntimeError {
                message: format!("{}: cursor.bytes must be Bytes, got {}", mold_name, v),
            });
        }
        None => {
            return Err(RuntimeError {
                message: format!("{}: cursor.bytes field is required", mold_name),
            });
        }
    };

    let offset_raw = match fields.iter().find(|(name, _)| name == "offset") {
        Some((_, Value::Int(v))) => *v,
        Some((_, v)) => {
            return Err(RuntimeError {
                message: format!("{}: cursor.offset must be Int, got {}", mold_name, v),
            });
        }
        None => 0,
    };

    let offset = offset_raw.clamp(0, bytes.len() as i64) as usize;
    Ok((bytes, offset))
}

/// C26B-016 (Option B+): extract `(start, len)` from a span pack
/// `@(start: Int, len: Int)`. Returns None if the value is not a
/// BuchiPack with both fields present as Int.
fn extract_span_pack(value: &Value) -> Option<(usize, usize)> {
    let Value::BuchiPack(fields) = value else {
        return None;
    };
    let start = fields.iter().find_map(|(k, v)| match v {
        Value::Int(n) if k == "start" => Some(*n),
        _ => None,
    })?;
    let len = fields.iter().find_map(|(k, v)| match v {
        Value::Int(n) if k == "len" => Some(*n),
        _ => None,
    })?;
    if start < 0 || len < 0 {
        return None;
    }
    Some((start as usize, len as usize))
}

/// C26B-016: resolve the raw byte buffer backing a span. Accepts `Bytes`
/// directly or `Str` (UTF-8 re-encoded). Returns None for other types.
fn raw_as_bytes(value: &Value) -> Option<std::borrow::Cow<'_, [u8]>> {
    match value {
        Value::Bytes(b) => Some(std::borrow::Cow::Borrowed(b.as_slice())),
        Value::Str(s) => Some(std::borrow::Cow::Borrowed(s.as_bytes())),
        _ => None,
    }
}

fn clamp_slice_bounds(len: usize, start: i64, end: i64) -> (usize, usize) {
    let len_i = len as i64;
    let s = start.clamp(0, len_i) as usize;
    let e = end.clamp(0, len_i) as usize;
    if s <= e { (s, e) } else { (e, e) }
}

fn to_radix_i64(value: i64, base: u32) -> String {
    debug_assert!((2..=36).contains(&base));
    if value == 0 {
        return "0".to_string();
    }
    let negative = value < 0;
    let mut n = value.unsigned_abs();
    let mut chars = Vec::new();
    while n > 0 {
        let d = (n % base as u64) as u8;
        let ch = if d < 10 {
            (b'0' + d) as char
        } else {
            (b'a' + (d - 10)) as char
        };
        chars.push(ch);
        n /= base as u64;
    }
    if negative {
        chars.push('-');
    }
    chars.iter().rev().collect()
}

impl Interpreter {
    fn todo_default_from_type_arg(&mut self, arg: &Expr) -> Result<Value, RuntimeError> {
        match arg {
            Expr::Ident(name, _) => match name.as_str() {
                "Int" | "Num" => Ok(Value::Int(0)),
                "Float" => Ok(Value::Float(0.0)),
                "Str" => Ok(Value::Str(String::new())),
                "Bool" => Ok(Value::Bool(false)),
                "Molten" => Ok(Value::Molten),
                _ => Ok(Value::Unit),
            },
            Expr::MoldInst(name, type_args, _, _) if name == "Stub" => {
                if type_args.len() != 1 {
                    return Err(RuntimeError {
                        message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                            .to_string(),
                    });
                }
                Ok(Value::Molten)
            }
            _ => Ok(Value::Unit),
        }
    }

    /// Collect a stream's items by eagerly applying all lazy transforms.
    fn collect_stream_items(&mut self, s: &StreamValue) -> Result<Vec<Value>, RuntimeError> {
        let mut items = s.items.clone();
        for transform in &s.transforms {
            match transform {
                StreamTransform::Map(func) => {
                    let mut mapped = Vec::new();
                    for item in &items {
                        let result =
                            self.call_function_with_values(func, std::slice::from_ref(item))?;
                        mapped.push(result);
                    }
                    items = mapped;
                }
                StreamTransform::Filter(func) => {
                    let mut filtered = Vec::new();
                    for item in &items {
                        let keep =
                            self.call_function_with_values(func, std::slice::from_ref(item))?;
                        if keep.is_truthy() {
                            filtered.push(item.clone());
                        }
                    }
                    items = filtered;
                }
                StreamTransform::Take(n) => {
                    items = items.into_iter().take(*n).collect();
                }
                StreamTransform::TakeWhile(func) => {
                    let mut taken = Vec::new();
                    for item in &items {
                        let keep =
                            self.call_function_with_values(func, std::slice::from_ref(item))?;
                        if keep.is_truthy() {
                            taken.push(item.clone());
                        } else {
                            break;
                        }
                    }
                    items = taken;
                }
            }
        }
        Ok(items)
    }

    /// Try to evaluate a new operation mold type (Str/Num/List molds with fields support).
    /// Returns None if the name is not a recognized operation mold.
    ///
    /// This match has many arms because it covers every built-in mold in the
    /// language. Each arm is a self-contained handler; splitting into separate
    /// functions would lose the ability to return `Result<Option<Signal>>` directly
    /// and would increase indirection without reducing complexity.
    pub(crate) fn try_operation_mold(
        &mut self,
        name: &str,
        type_args: &[Expr],
        fields: &[crate::parser::BuchiField],
    ) -> Result<Option<Signal>, RuntimeError> {
        match name {
            // ── Str molds ────────────────────────────────────
            "Upper" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Upper requires 1 argument: Upper[str]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Upper: argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Str(s.to_uppercase()))))
            }
            "Lower" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Lower requires 1 argument: Lower[str]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Lower: argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Str(s.to_lowercase()))))
            }
            "Trim" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Trim requires 1 argument: Trim[str]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Trim: argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let trim_start = self
                    .eval_mold_option(fields, "start")?
                    .map(|v| v.is_truthy())
                    .unwrap_or(true);
                let trim_end = self
                    .eval_mold_option(fields, "end")?
                    .map(|v| v.is_truthy())
                    .unwrap_or(true);
                let result = match (trim_start, trim_end) {
                    (true, true) => s.trim().to_string(),
                    (true, false) => s.trim_start().to_string(),
                    (false, true) => s.trim_end().to_string(),
                    (false, false) => s,
                };
                Ok(Some(Signal::Value(Value::Str(result))))
            }
            "Split" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Split requires 2 arguments: Split[str, delim]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Split: first argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let delim = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Str(d)) => d,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Split: second argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let parts: Vec<Value> =
                    s.split(&delim).map(|p| Value::Str(p.to_string())).collect();
                Ok(Some(Signal::Value(Value::list(parts))))
            }
            "Chars" => {
                if type_args.len() != 1 {
                    return Err(RuntimeError {
                        message: "Chars requires 1 argument: Chars[str]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Chars: argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let chars: Vec<Value> = s.chars().map(|ch| Value::Str(ch.to_string())).collect();
                Ok(Some(Signal::Value(Value::list(chars))))
            }
            "Replace" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "Replace requires 3 arguments: Replace[str, old, new]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Replace: first argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let old = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Str(o)) => o,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "Replace: second argument must be a string, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let new_str = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Str(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Replace: third argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let replace_all = self
                    .eval_mold_option(fields, "all")?
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                let result = if replace_all {
                    s.replace(&old, &new_str)
                } else {
                    s.replacen(&old, &new_str, 1)
                };
                Ok(Some(Signal::Value(Value::Str(result))))
            }
            "Slice" if !type_args.is_empty() && type_args.len() <= 3 => {
                // Slice[str|bytes](start <= n, end <= m)  — 1 type arg + optional fields
                // Slice[str|bytes, start, end]()          — 3 type args shorthand
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                // start: from type_args[1] if present, else from optional field
                let start = if type_args.len() >= 2 {
                    match self.eval_expr(&type_args[1])? {
                        Signal::Value(Value::Int(n)) => n,
                        _ => 0,
                    }
                } else {
                    self.eval_mold_option(fields, "start")?
                        .and_then(|v| if let Value::Int(n) = v { Some(n) } else { None })
                        .unwrap_or(0)
                };
                match val {
                    Value::Str(s) => {
                        let char_count = s.chars().count();
                        let end = if type_args.len() >= 3 {
                            match self.eval_expr(&type_args[2])? {
                                Signal::Value(Value::Int(n)) => n,
                                _ => char_count as i64,
                            }
                        } else {
                            self.eval_mold_option(fields, "end")?
                                .and_then(|v| if let Value::Int(n) = v { Some(n) } else { None })
                                .unwrap_or(char_count as i64)
                        };
                        let (clamped_start, clamped_end) =
                            clamp_slice_bounds(char_count, start, end);
                        let result: String = s
                            .chars()
                            .skip(clamped_start)
                            .take(clamped_end.saturating_sub(clamped_start))
                            .collect();
                        Ok(Some(Signal::Value(Value::Str(result))))
                    }
                    Value::Bytes(bytes) => {
                        let end = if type_args.len() >= 3 {
                            match self.eval_expr(&type_args[2])? {
                                Signal::Value(Value::Int(n)) => n,
                                _ => bytes.len() as i64,
                            }
                        } else {
                            self.eval_mold_option(fields, "end")?
                                .and_then(|v| if let Value::Int(n) = v { Some(n) } else { None })
                                .unwrap_or(bytes.len() as i64)
                        };
                        let (clamped_start, clamped_end) =
                            clamp_slice_bounds(bytes.len(), start, end);
                        let result = bytes[clamped_start..clamped_end].to_vec();
                        Ok(Some(Signal::Value(Value::Bytes(result))))
                    }
                    _ => Ok(None), // Not a supported Slice target, fall through
                }
            }
            "CharAt" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "CharAt requires 2 arguments: CharAt[str, idx]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("CharAt: first argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let idx = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as usize,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "CharAt: second argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                match s.chars().nth(idx) {
                    Some(c) => {
                        let value = Value::Str(c.to_string());
                        Ok(Some(Signal::Value(make_lax_value(
                            true,
                            value,
                            Value::Str(String::new()),
                        ))))
                    }
                    None => {
                        // Out of bounds: return Lax with hasValue=false
                        Ok(Some(Signal::Value(make_lax_value(
                            false,
                            Value::Str(String::new()),
                            Value::Str(String::new()),
                        ))))
                    }
                }
            }
            "Repeat" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Repeat requires 2 arguments: Repeat[str, n]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Repeat: first argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let n = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as usize,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "Repeat: second argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Str(s.repeat(n)))))
            }
            // ── C26B-018 (B) byte-level primitives (UTF-8 byte view) ──
            // These operate on the raw UTF-8 byte stream (not Unicode
            // scalar values). They give O(1) / O(len) predictable
            // complexity for hot-path parsers where the caller already
            // knows they are dealing with ASCII or byte-boundary-safe
            // offsets. Added as a pure additive mold (§ 6.2 widening) —
            // existing `CharAt` / `Slice` / `.length()` semantics are
            // **unchanged** (still Unicode-scalar for surface safety).
            "ByteAt" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "ByteAt requires 2 arguments: ByteAt[str, idx]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ByteAt: first argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let idx = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "ByteAt: second argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let bytes = s.as_bytes();
                if idx < 0 || (idx as usize) >= bytes.len() {
                    Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Int(0),
                        Value::Int(0),
                    ))))
                } else {
                    let b = bytes[idx as usize] as i64;
                    Ok(Some(Signal::Value(make_lax_value(
                        true,
                        Value::Int(b),
                        Value::Int(0),
                    ))))
                }
            }
            "ByteSlice" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message:
                            "ByteSlice requires 3 arguments: ByteSlice[str, start, end]()"
                                .into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "ByteSlice: first argument must be a string, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let start = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "ByteSlice: second argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let end = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "ByteSlice: third argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let bytes = s.as_bytes();
                let len = bytes.len() as i64;
                let s_i = start.clamp(0, len) as usize;
                let e_i = end.clamp(0, len) as usize;
                let (lo, hi) = if s_i <= e_i { (s_i, e_i) } else { (e_i, s_i) };
                // Preserve exact bytes; if the slice lands on a UTF-8
                // boundary mid-sequence, we fall back to a lossless
                // String::from_utf8_lossy → owned. This matches "return
                // what was there" semantics; callers who need scalar
                // safety should use Slice (not ByteSlice).
                let slice = &bytes[lo..hi];
                let out = match std::str::from_utf8(slice) {
                    Ok(v) => v.to_string(),
                    Err(_) => String::from_utf8_lossy(slice).into_owned(),
                };
                Ok(Some(Signal::Value(Value::Str(out))))
            }
            "ByteLength" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "ByteLength requires 1 argument: ByteLength[str]()".into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "ByteLength: argument must be a string, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(s.len() as i64))))
            }
            // ── C26B-018 (C) StringRepeatJoin ─────────────────────────
            // Single-allocation repeat + join primitive. Surface-level
            // equivalent of `Join[Array(str; n), sep]()` but computes
            // total length up front and allocates once, avoiding the
            // O(N²) reallocation cascade in chained `+` concat idioms
            // observed in terminal / hachikuma tui primitives.
            //
            // `StringRepeatJoin[str, n, sep]() -> Str`
            //   n <= 0      → ""
            //   n == 1      → str (sep never emitted)
            //   n >= 2      → str + sep + str + sep + ... + str (n copies, n-1 seps)
            "StringRepeatJoin" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message:
                            "StringRepeatJoin requires 3 arguments: StringRepeatJoin[str, n, sep]()"
                                .into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "StringRepeatJoin: first argument must be a string, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let n = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "StringRepeatJoin: second argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let sep = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "StringRepeatJoin: third argument must be a string, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if n <= 0 {
                    return Ok(Some(Signal::Value(Value::Str(String::new()))));
                }
                let n = n as usize;
                if n == 1 {
                    return Ok(Some(Signal::Value(Value::Str(s))));
                }
                // Pre-compute the total length to allocate once.
                let total = s
                    .len()
                    .saturating_mul(n)
                    .saturating_add(sep.len().saturating_mul(n - 1));
                let mut out = String::with_capacity(total);
                out.push_str(&s);
                for _ in 1..n {
                    out.push_str(&sep);
                    out.push_str(&s);
                }
                Ok(Some(Signal::Value(Value::Str(out))))
            }
            "Reverse" => {
                // Polymorphic: works on both Str and List
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Reverse requires 1 argument: Reverse[value]()".into(),
                    });
                }
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                match val {
                    Value::Str(s) => Ok(Some(Signal::Value(Value::Str(s.chars().rev().collect())))),
                    Value::List(items) => {
                        let mut reversed = Value::list_take(items);
                        reversed.reverse();
                        Ok(Some(Signal::Value(Value::list(reversed))))
                    }
                    _ => Err(RuntimeError {
                        message: format!("Reverse: argument must be a string or list, got {}", val),
                    }),
                }
            }
            "Pad" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message:
                            "Pad requires 2 arguments: Pad[str, len](side <= \"start\"|\"end\")"
                                .into(),
                    });
                }
                let s = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Pad: first argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let target_len = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as usize,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Pad: second argument must be an integer, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let side = self
                    .eval_mold_option(fields, "side")?
                    .and_then(|v| if let Value::Str(s) = v { Some(s) } else { None })
                    .unwrap_or_else(|| "start".to_string());
                let pad_char = self
                    .eval_mold_option(fields, "char")?
                    .and_then(|v| if let Value::Str(s) = v { Some(s) } else { None })
                    .unwrap_or_else(|| " ".to_string());
                if s.len() >= target_len {
                    Ok(Some(Signal::Value(Value::Str(s))))
                } else {
                    let padding = pad_char.repeat(target_len - s.len());
                    let result = if side == "end" {
                        format!("{}{}", s, padding)
                    } else {
                        format!("{}{}", padding, s)
                    };
                    Ok(Some(Signal::Value(Value::Str(result))))
                }
            }

            // ── Num molds ────────────────────────────────────
            "ToFixed" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "ToFixed requires 2 arguments: ToFixed[num, digits]()".into(),
                    });
                }
                let num = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ToFixed: first argument must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let digits = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as usize,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "ToFixed: second argument must be an integer, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Str(format!(
                    "{:.prec$}",
                    num,
                    prec = digits
                )))))
            }
            "Abs" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Abs requires 1 argument: Abs[num]()".into(),
                    });
                }
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                match val {
                    Value::Int(n) => Ok(Some(Signal::Value(Value::Int(n.abs())))),
                    Value::Float(n) => Ok(Some(Signal::Value(Value::Float(n.abs())))),
                    _ => Err(RuntimeError {
                        message: format!("Abs: argument must be numeric, got {}", val),
                    }),
                }
            }
            "Floor" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Floor requires 1 argument: Floor[num]()".into(),
                    });
                }
                let num = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Floor: argument must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(num.floor() as i64))))
            }
            "Ceil" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Ceil requires 1 argument: Ceil[num]()".into(),
                    });
                }
                let num = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Ceil: argument must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(num.ceil() as i64))))
            }
            "Round" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Round requires 1 argument: Round[num]()".into(),
                    });
                }
                let num = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Round: argument must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(num.round() as i64))))
            }
            "Truncate" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Truncate requires 1 argument: Truncate[num]()".into(),
                    });
                }
                let num = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Truncate: argument must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(num.trunc() as i64))))
            }
            // C25B-025 (Phase 5-A): math molds.
            //
            // Previously `Sqrt[4.0]()` / `Pow[2.0, 10]()` flowed through the
            // generic mold-instantiation fallback in `eval_expr::MoldInst`
            // and produced `@(__value <= <first-arg>, __type <= "Sqrt")` —
            // a silent wrong result (type inference registered `Float`,
            // but the value was actually a Lax-shaped BuchiPack).
            // Transcendentals (`Sin` / `Cos` / etc.) had no registration
            // at all and therefore required the `__value` unwrap.
            //
            // These interpreter implementations delegate to `f64::sqrt`,
            // `f64::powf`, etc. NaN / ±Infinity / denormal are preserved
            // as Rust's `f64` semantics. Accepting `Int` widens to `f64`
            // first; `Pow[Int, Int]` returns Float per `mold_returns.rs`.
            "Sqrt" => eval_unary_math(self, type_args, "Sqrt", f64::sqrt),
            "Exp" => eval_unary_math(self, type_args, "Exp", f64::exp),
            "Ln" => eval_unary_math(self, type_args, "Ln", f64::ln),
            "Log2" => eval_unary_math(self, type_args, "Log2", f64::log2),
            "Log10" => eval_unary_math(self, type_args, "Log10", f64::log10),
            "Sin" => eval_unary_math(self, type_args, "Sin", f64::sin),
            "Cos" => eval_unary_math(self, type_args, "Cos", f64::cos),
            "Tan" => eval_unary_math(self, type_args, "Tan", f64::tan),
            "Asin" => eval_unary_math(self, type_args, "Asin", f64::asin),
            "Acos" => eval_unary_math(self, type_args, "Acos", f64::acos),
            "Atan" => eval_unary_math(self, type_args, "Atan", f64::atan),
            "Sinh" => eval_unary_math(self, type_args, "Sinh", f64::sinh),
            "Cosh" => eval_unary_math(self, type_args, "Cosh", f64::cosh),
            "Tanh" => eval_unary_math(self, type_args, "Tanh", f64::tanh),
            "Pow" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Pow requires 2 arguments: Pow[base, exp]()".into(),
                    });
                }
                let base = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Pow: base must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let exp_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Pow: exponent must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Float(base.powf(exp_val)))))
            }
            "Log" => {
                // Log[value, base]() — explicit base. Base defaults to e
                // if omitted (so `Log[x]()` == `Ln[x]()` semantically).
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Log requires 1-2 arguments: Log[value]() or Log[value, base]()"
                            .into(),
                    });
                }
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Log: value must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let result = if type_args.len() >= 2 {
                    let base = match self.eval_expr(&type_args[1])? {
                        Signal::Value(Value::Int(n)) => n as f64,
                        Signal::Value(Value::Float(n)) => n,
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!("Log: base must be numeric, got {}", v),
                            });
                        }
                        other => return Ok(Some(other)),
                    };
                    val.log(base)
                } else {
                    val.ln()
                };
                Ok(Some(Signal::Value(Value::Float(result))))
            }
            "Atan2" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Atan2 requires 2 arguments: Atan2[y, x]()".into(),
                    });
                }
                let y = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Atan2: y must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let x = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Atan2: x must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Float(y.atan2(x)))))
            }
            "Clamp" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "Clamp requires 3 arguments: Clamp[num, min, max]()".into(),
                    });
                }
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let min_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Clamp: min must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let max_val = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Int(n)) => n as f64,
                    Signal::Value(Value::Float(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Clamp: max must be numeric, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let float_val = match &val {
                    Value::Int(n) => *n as f64,
                    Value::Float(n) => *n,
                    _ => {
                        return Err(RuntimeError {
                            message: format!("Clamp: first argument must be numeric, got {}", val),
                        });
                    }
                };
                let clamped = float_val.clamp(min_val, max_val);
                if matches!(val, Value::Int(_)) && clamped == clamped.floor() {
                    Ok(Some(Signal::Value(Value::Int(clamped as i64))))
                } else {
                    Ok(Some(Signal::Value(Value::Float(clamped))))
                }
            }
            "BitAnd" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "BitAnd requires 2 arguments: BitAnd[a, b]()".into(),
                    });
                }
                let a = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitAnd: first argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let b = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitAnd: second argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(a & b))))
            }
            "BitOr" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "BitOr requires 2 arguments: BitOr[a, b]()".into(),
                    });
                }
                let a = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitOr: first argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let b = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitOr: second argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(a | b))))
            }
            "BitXor" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "BitXor requires 2 arguments: BitXor[a, b]()".into(),
                    });
                }
                let a = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitXor: first argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let b = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitXor: second argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(a ^ b))))
            }
            "BitNot" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "BitNot requires 1 argument: BitNot[x]()".into(),
                    });
                }
                let x = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BitNot: argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Int(!x))))
            }
            "ShiftL" | "ShiftR" | "ShiftRU" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: format!("{} requires 2 arguments: {}[x, n]()", name, name),
                    });
                }
                let x = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("{}: first argument must be Int, got {}", name, v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let n = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("{}: second argument must be Int, got {}", name, v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if !(0..=63).contains(&n) {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Int(0),
                        Value::Int(0),
                    ))));
                }
                let result = match name {
                    "ShiftL" => Value::Int(x.wrapping_shl(n as u32)),
                    "ShiftR" => Value::Int(x >> n),
                    "ShiftRU" => Value::Int(((x as u64) >> (n as u32)) as i64),
                    // SAFETY: match arm covers only "ShiftL"/"ShiftR"/"ShiftRU" names
                    _ => unreachable!("only ShiftL/ShiftR/ShiftRU reach this arm"),
                };
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    result,
                    Value::Int(0),
                ))))
            }
            "ToRadix" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "ToRadix requires 2 arguments: ToRadix[int, base]()".into(),
                    });
                }
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ToRadix: first argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let base = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ToRadix: second argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if !(2..=36).contains(&base) {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Str(String::new()),
                        Value::Str(String::new()),
                    ))));
                }
                let out = to_radix_i64(val, base as u32);
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    Value::Str(out),
                    Value::Str(String::new()),
                ))))
            }
            "U16BE" | "U16LE" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: format!("{} requires 1 argument: {}[value]()", name, name),
                    });
                }
                let value = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("{}: argument must be Int, got {}", name, v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if !(0..=u16::MAX as i64).contains(&value) {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Bytes(Vec::new()),
                        Value::Bytes(Vec::new()),
                    ))));
                }
                let n = value as u16;
                let bytes = if name == "U16BE" {
                    vec![(n >> 8) as u8, (n & 0xff) as u8]
                } else {
                    vec![(n & 0xff) as u8, (n >> 8) as u8]
                };
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    Value::Bytes(bytes),
                    Value::Bytes(Vec::new()),
                ))))
            }
            "U32BE" | "U32LE" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: format!("{} requires 1 argument: {}[value]()", name, name),
                    });
                }
                let value = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("{}: argument must be Int, got {}", name, v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if !(0..=u32::MAX as i64).contains(&value) {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Bytes(Vec::new()),
                        Value::Bytes(Vec::new()),
                    ))));
                }
                let n = value as u32;
                let bytes = if name == "U32BE" {
                    vec![
                        ((n >> 24) & 0xff) as u8,
                        ((n >> 16) & 0xff) as u8,
                        ((n >> 8) & 0xff) as u8,
                        (n & 0xff) as u8,
                    ]
                } else {
                    vec![
                        (n & 0xff) as u8,
                        ((n >> 8) & 0xff) as u8,
                        ((n >> 16) & 0xff) as u8,
                        ((n >> 24) & 0xff) as u8,
                    ]
                };
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    Value::Bytes(bytes),
                    Value::Bytes(Vec::new()),
                ))))
            }
            "U16BEDecode" | "U16LEDecode" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: format!("{} requires 1 argument: {}[bytes]()", name, name),
                    });
                }
                let bytes = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Bytes(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("{}: argument must be Bytes, got {}", name, v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if bytes.len() != 2 {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Int(0),
                        Value::Int(0),
                    ))));
                }
                let out = if name == "U16BEDecode" {
                    ((bytes[0] as i64) << 8) | (bytes[1] as i64)
                } else {
                    ((bytes[1] as i64) << 8) | (bytes[0] as i64)
                };
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    Value::Int(out),
                    Value::Int(0),
                ))))
            }
            "U32BEDecode" | "U32LEDecode" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: format!("{} requires 1 argument: {}[bytes]()", name, name),
                    });
                }
                let bytes = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Bytes(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("{}: argument must be Bytes, got {}", name, v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if bytes.len() != 4 {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Int(0),
                        Value::Int(0),
                    ))));
                }
                let out_u32 = if name == "U32BEDecode" {
                    ((bytes[0] as u32) << 24)
                        | ((bytes[1] as u32) << 16)
                        | ((bytes[2] as u32) << 8)
                        | (bytes[3] as u32)
                } else {
                    ((bytes[3] as u32) << 24)
                        | ((bytes[2] as u32) << 16)
                        | ((bytes[1] as u32) << 8)
                        | (bytes[0] as u32)
                };
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    Value::Int(out_u32 as i64),
                    Value::Int(0),
                ))))
            }
            "BytesCursor" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "BytesCursor requires 1 argument: BytesCursor[bytes]()".into(),
                    });
                }
                let bytes = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Bytes(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BytesCursor: argument must be Bytes, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let offset = match self.eval_mold_option(fields, "offset")? {
                    Some(Value::Int(v)) => v,
                    Some(v) => {
                        return Err(RuntimeError {
                            message: format!("BytesCursor: offset must be Int, got {}", v),
                        });
                    }
                    None => 0,
                };
                Ok(Some(Signal::Value(make_bytes_cursor(bytes, offset))))
            }
            "BytesCursorRemaining" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "BytesCursorRemaining requires 1 argument: BytesCursorRemaining[cursor]()".into(),
                    });
                }
                let cursor = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let (bytes, offset) = parse_bytes_cursor(cursor, "BytesCursorRemaining")?;
                Ok(Some(Signal::Value(Value::Int(
                    (bytes.len().saturating_sub(offset)) as i64,
                ))))
            }
            "BytesCursorTake" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message:
                            "BytesCursorTake requires 2 arguments: BytesCursorTake[cursor, size]()"
                                .into(),
                    });
                }
                let cursor_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let (bytes, offset) = parse_bytes_cursor(cursor_val, "BytesCursorTake")?;
                let size = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BytesCursorTake: size must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let current_cursor = make_bytes_cursor(bytes.clone(), offset as i64);
                let default_step =
                    make_bytes_cursor_step(Value::Bytes(Vec::new()), current_cursor.clone());
                if size < 0 {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        default_step.clone(),
                        default_step,
                    ))));
                }
                let size = size as usize;
                if offset + size > bytes.len() {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        default_step.clone(),
                        default_step,
                    ))));
                }
                let chunk = bytes[offset..offset + size].to_vec();
                let next_cursor = make_bytes_cursor(bytes, (offset + size) as i64);
                let step = make_bytes_cursor_step(Value::Bytes(chunk), next_cursor);
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    step,
                    default_step,
                ))))
            }
            "BytesCursorU8" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "BytesCursorU8 requires 1 argument: BytesCursorU8[cursor]()"
                            .into(),
                    });
                }
                let cursor_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let (bytes, offset) = parse_bytes_cursor(cursor_val, "BytesCursorU8")?;
                let current_cursor = make_bytes_cursor(bytes.clone(), offset as i64);
                let default_step = make_bytes_cursor_step(Value::Int(0), current_cursor.clone());
                if offset >= bytes.len() {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        default_step.clone(),
                        default_step,
                    ))));
                }
                let value = bytes[offset] as i64;
                let next_cursor = make_bytes_cursor(bytes, (offset + 1) as i64);
                let step = make_bytes_cursor_step(Value::Int(value), next_cursor);
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    step,
                    default_step,
                ))))
            }

            // ── List molds (new) ────────────────────────────
            "Concat" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Concat requires 2 arguments: Concat[list, other]()".into(),
                    });
                }
                let left = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let right = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                match (left, right) {
                    (Value::List(list), Value::List(other)) => {
                        let mut result = Value::list_take(list);
                        result.extend(Value::list_take(other));
                        Ok(Some(Signal::Value(Value::list(result))))
                    }
                    (Value::Bytes(mut a), Value::Bytes(b)) => {
                        a.extend(b);
                        Ok(Some(Signal::Value(Value::Bytes(a))))
                    }
                    (a, b) => Err(RuntimeError {
                        message: format!(
                            "Concat: arguments must both be list or both be Bytes, got {} and {}",
                            a, b
                        ),
                    }),
                }
            }
            "ByteSet" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "ByteSet requires 3 arguments: ByteSet[bytes, idx, value]()"
                            .into(),
                    });
                }
                let bytes = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Bytes(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ByteSet: first argument must be Bytes, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let idx = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ByteSet: second argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let value = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Int(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("ByteSet: third argument must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if idx < 0 || (idx as usize) >= bytes.len() || !(0..=255).contains(&value) {
                    return Ok(Some(Signal::Value(make_lax_value(
                        false,
                        Value::Bytes(Vec::new()),
                        Value::Bytes(Vec::new()),
                    ))));
                }
                let mut out = bytes.clone();
                out[idx as usize] = value as u8;
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    Value::Bytes(out),
                    Value::Bytes(Vec::new()),
                ))))
            }
            "BytesToList" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "BytesToList requires 1 argument: BytesToList[bytes]()".into(),
                    });
                }
                let bytes = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Bytes(v)) => v,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("BytesToList: argument must be Bytes, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let items = bytes.into_iter().map(|b| Value::Int(b as i64)).collect();
                Ok(Some(Signal::Value(Value::list(items))))
            }
            "Append" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Append requires 2 arguments: Append[list, val]()".into(),
                    });
                }
                // C25B-021 REOPEN fix (2026-04-23, session 20):
                //
                // An earlier Stage-B attempt took the binding out of the
                // innermost scope via `Environment::take_from_current_scope`,
                // pushed into the `Arc<Vec>` under unique ownership via
                // `Arc::make_mut`, and rebound the *new* list back into env.
                // That gave O(N) amortized on the tail-recursive
                // `build(Append[acc, i](), ...)` pattern, but it silently
                // mutated the binding `acc` — so for code like
                //
                //   xs = @[1, 2]
                //   newxs = Append[xs, 9]()
                //   print(xs.length())   // expected 2, saw 3
                //
                // the semantic contract was broken. `Append` must be
                // non-destructive on the argument binding.
                //
                // Fix: revert to the clone-based path. `list_take` tries
                // `Arc::try_unwrap`; when env still holds a clone it falls
                // back to a single `Vec::clone` of the element slice. In
                // the tail-recursive loop, `current_args.clear()` (the
                // trampoline-level release in `call_function`) still
                // releases the caller's Arc after parameter binding, so
                // env is often the sole holder and try_unwrap succeeds —
                // the common case remains cheap. Only the specific
                // "observer in the same scope retained the old binding"
                // case costs a full clone, which is the original O(N²)
                // worst case. Acceptable trade-off until persistent-Vec
                // land; semantic correctness is non-negotiable.
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Append: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let mut result = Value::list_take(list);
                result.push(val);
                Ok(Some(Signal::Value(Value::list(result))))
            }
            "Prepend" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Prepend requires 2 arguments: Prepend[list, val]()".into(),
                    });
                }
                // C25B-021 REOPEN fix (2026-04-23, session 20):
                // See the Append arm above — the env-take path destroyed
                // the `Prepend[xs, v]()` → `xs` unchanged contract, so
                // we revert to the clone-based path.
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Prepend: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let mut result = vec![val];
                result.extend(Value::list_take(list));
                Ok(Some(Signal::Value(Value::list(result))))
            }
            "Join" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Join requires 2 arguments: Join[list, sep]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Join: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let sep = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Join: second argument must be a string, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let result: Vec<String> = list.iter().map(|v| v.to_display_string()).collect();
                Ok(Some(Signal::Value(Value::Str(result.join(&sep)))))
            }
            "Sum" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Sum requires 1 argument: Sum[list]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Sum: argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let sum: f64 = list
                    .iter()
                    .map(|v| match v {
                        Value::Int(n) => *n as f64,
                        Value::Float(n) => *n,
                        _ => 0.0,
                    })
                    .sum();
                if list.iter().all(|v| matches!(v, Value::Int(_))) {
                    Ok(Some(Signal::Value(Value::Int(sum as i64))))
                } else {
                    Ok(Some(Signal::Value(Value::Float(sum))))
                }
            }
            "Sort" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Sort requires 1 argument: Sort[list]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Sort: argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let reverse = self
                    .eval_mold_option(fields, "reverse")?
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                let by_fn = self.eval_mold_option(fields, "by")?;

                let mut sorted: Vec<Value> = if let Some(Value::Function(func)) = by_fn {
                    // Sort by key extraction function
                    let mut keyed: Vec<(Value, Value)> = Vec::new();
                    for item in list.iter() {
                        let key =
                            self.call_function_with_values(&func, std::slice::from_ref(item))?;
                        keyed.push((item.clone(), key));
                    }
                    keyed.sort_by(|(_, ka), (_, kb)| {
                        ka.partial_cmp(kb).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    keyed.into_iter().map(|(item, _)| item).collect()
                } else {
                    let mut items = Value::list_take(list);
                    items.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    items
                };
                if reverse {
                    sorted.reverse();
                }
                Ok(Some(Signal::Value(Value::list(sorted))))
            }
            "Unique" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Unique requires 1 argument: Unique[list]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Unique: argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let by_fn = self.eval_mold_option(fields, "by")?;

                // C25B-021 派生 (Phase 5-C): replace the per-item
                // `Vec::contains` linear scan (O(N²)) with a HashSet
                // fingerprint probe (O(N)) when every produced key is
                // hashable. Float / Function / Async keys fall back to
                // the original linear path so `Value::eq` semantics are
                // preserved for cross-type coercions.
                use crate::interpreter::value_key::ValueKey;
                use std::collections::HashSet;

                let unique = if let Some(Value::Function(func)) = by_fn {
                    // Each `seen_keys[i]` is the canonical key value for
                    // the key-of-item pattern; when a fingerprint collides
                    // we fall back to Value::eq against `seen_keys` so
                    // that distinct-name EnumVals with the same ordinal
                    // (C25B-022 REOPEN scenario) are not incorrectly
                    // folded together.
                    let mut seen_fps: HashSet<u64> = HashSet::new();
                    let mut seen_keys: Vec<Value> = Vec::new();
                    let mut fallback_armed = false;
                    let mut result: Vec<Value> = Vec::new();
                    for item in list.iter() {
                        let key =
                            self.call_function_with_values(&func, std::slice::from_ref(item))?;
                        if fallback_armed {
                            if !seen_keys.contains(&key) {
                                seen_keys.push(key);
                                result.push(item.clone());
                            }
                        } else if let Some(vk) = ValueKey::new(&key) {
                            let fp = vk.fingerprint();
                            if seen_fps.insert(fp) {
                                seen_keys.push(key);
                                result.push(item.clone());
                            } else if !seen_keys.iter().any(|k| k == &key) {
                                // Rare fingerprint collision (e.g.
                                // EnumVal("A",0) vs EnumVal("B",0)).
                                // Value::eq is authoritative.
                                seen_keys.push(key);
                                result.push(item.clone());
                            }
                        } else {
                            // Key turned out not to be hashable. Replay
                            // the fingerprints we already added into the
                            // fallback list and continue with linear
                            // contains for the remainder.
                            fallback_armed = true;
                            if !seen_keys.contains(&key) {
                                seen_keys.push(key);
                                result.push(item.clone());
                            }
                        }
                    }
                    result
                } else {
                    // Fast path: build a fingerprint set up front if the
                    // entire list is hashable. On fingerprint collision
                    // we defer to Value::eq against already-emitted
                    // result items so cross-EnumVal-name inputs with the
                    // same ordinal are preserved as distinct.
                    if list.iter().all(|v| ValueKey::new(v).is_some()) {
                        let mut seen: HashSet<u64> = HashSet::new();
                        let mut result: Vec<Value> = Vec::new();
                        for item in list.iter() {
                            let vk = ValueKey::new(item).expect("hashability pre-checked above");
                            let fp = vk.fingerprint();
                            if seen.insert(fp) {
                                result.push(item.clone());
                            } else if !result.iter().any(|e| e == item) {
                                // Rare collision — preserve Value::eq.
                                result.push(item.clone());
                            }
                        }
                        result
                    } else {
                        let mut result: Vec<Value> = Vec::new();
                        for item in list.iter() {
                            if !result.contains(item) {
                                result.push(item.clone());
                            }
                        }
                        result
                    }
                };
                Ok(Some(Signal::Value(Value::list(unique))))
            }
            "Flatten" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Flatten requires 1 argument: Flatten[list]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Flatten: argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let mut flat = Vec::new();
                for item in list.iter() {
                    if let Value::List(inner) = item {
                        flat.extend(inner.iter().cloned());
                    } else {
                        flat.push(item.clone());
                    }
                }
                Ok(Some(Signal::Value(Value::list(flat))))
            }
            "Find" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Find requires 2 arguments: Find[list, fn]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Find: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let func = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Function(f)) => f,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Find: second argument must be a function, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                for item in list.iter() {
                    let result =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if result.is_truthy() {
                        let default_val = Self::default_for_value(item);
                        return Ok(Some(Signal::Value(Value::BuchiPack(vec![
                            ("hasValue".into(), Value::Bool(true)),
                            ("__value".into(), item.clone()),
                            ("__default".into(), default_val),
                            ("__type".into(), Value::Str("Lax".into())),
                        ]))));
                    }
                }
                // Not found — return Lax with hasValue=false
                let default_val = if let Some(first) = list.first() {
                    Self::default_for_value(first)
                } else {
                    Value::Int(0)
                };
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(false)),
                    ("__value".into(), default_val.clone()),
                    ("__default".into(), default_val),
                    ("__type".into(), Value::Str("Lax".into())),
                ]))))
            }
            "FindIndex" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "FindIndex requires 2 arguments: FindIndex[list, fn]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("FindIndex: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let func = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Function(f)) => f,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "FindIndex: second argument must be a function, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                for (i, item) in list.iter().enumerate() {
                    let result =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if result.is_truthy() {
                        return Ok(Some(Signal::Value(Value::Int(i as i64))));
                    }
                }
                Ok(Some(Signal::Value(Value::Int(-1))))
            }
            "Count" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Count requires 2 arguments: Count[list, fn]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Count: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let func = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Function(f)) => f,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "Count: second argument must be a function, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let mut count = 0i64;
                for item in list.iter() {
                    let result =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if result.is_truthy() {
                        count += 1;
                    }
                }
                Ok(Some(Signal::Value(Value::Int(count))))
            }
            "Zip" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Zip requires 2 arguments: Zip[list, other]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Zip: first argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let other = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Zip: second argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let zipped: Vec<Value> = list
                    .iter()
                    .zip(other.iter())
                    .map(|(a, b)| {
                        Value::BuchiPack(vec![
                            ("first".into(), a.clone()),
                            ("second".into(), b.clone()),
                        ])
                    })
                    .collect();
                Ok(Some(Signal::Value(Value::list(zipped))))
            }
            "Enumerate" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Enumerate requires 1 argument: Enumerate[list]()".into(),
                    });
                }
                let list = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::List(items)) => items,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("Enumerate: argument must be a list, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let enumerated: Vec<Value> = list
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Value::BuchiPack(vec![
                            ("index".into(), Value::Int(i as i64)),
                            ("value".into(), v.clone()),
                        ])
                    })
                    .collect();
                Ok(Some(Signal::Value(Value::list(enumerated))))
            }

            _ => self.try_core_mold(name, type_args, fields),
        }
    }

    /// Try to evaluate core built-in molds that were historically special-cased
    /// in eval.rs (Result/Lax/Gorillax/Cage and conversion molds).
    pub(crate) fn try_core_mold(
        &mut self,
        name: &str,
        type_args: &[Expr],
        fields: &[crate::parser::BuchiField],
    ) -> Result<Option<Signal>, RuntimeError> {
        match name {
            // Optional — ABOLISHED (v0.8.0). Use Lax[T] instead.
            "Optional" => Err(RuntimeError {
                message: "Optional has been removed. Use Lax[value]() instead. Lax[T] provides the same safety with default value guarantees.".to_string(),
            }),

            // Result[value, predicate](): create Result (predicate + throw).
            "Result" => {
                let inner_value = if !type_args.is_empty() {
                    match self.eval_expr(&type_args[0])? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                let predicate = if type_args.len() >= 2 {
                    match self.eval_expr(&type_args[1])? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                let throw_value = fields
                    .iter()
                    .find(|f| f.name == "throw")
                    .map(|f| self.eval_expr(&f.value))
                    .transpose()?
                    .and_then(|s| {
                        if let Signal::Value(v) = s {
                            Some(v)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(Value::Unit);
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("__value".into(), inner_value),
                    ("__predicate".into(), predicate),
                    ("throw".into(), throw_value),
                    ("__type".into(), Value::Str("Result".into())),
                ]))))
            }

            // Lax[T](): create Lax with value and default.
            "Lax" => {
                let inner_value = if !type_args.is_empty() {
                    match self.eval_expr(&type_args[0])? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                let default_value = Self::default_for_value(&inner_value);
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    inner_value,
                    default_value,
                ))))
            }

            // Stub["msg"](): unresolved placeholder value represented as Molten.
            "Stub" => {
                if !fields.is_empty() {
                    return Err(RuntimeError {
                        message: "Stub does not take `()` fields. Use Stub[\"msg\"]".to_string(),
                    });
                }
                if type_args.len() != 1 {
                    return Err(RuntimeError {
                        message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                            .to_string(),
                    });
                }
                let message = match self.eval_expr(&type_args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "Stub message must be a string literal/expression, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let _ = message;
                Ok(Some(Signal::Value(Value::Molten)))
            }

            // TODO[T](): executable todo annotation wrapper.
            //
            // Layout:
            //   id   — task identifier (optional)
            //   task — description of the pending work
            //   sol  — solidify channel (`__value`): the current placeholder value
            //   unm  — unmold channel (`__default`): the value returned by `]=>`
            //
            // When T is provided, both `sol` and `unm` default to `default_for_type(T)`.
            // See unmold.rs for the unmold (`]=>`) behavior.
            "TODO" => {
                let type_default = if let Some(arg) = type_args.first() {
                    self.todo_default_from_type_arg(arg)?
                } else {
                    Value::Unit
                };

                let id = self.eval_mold_option(fields, "id")?.unwrap_or(Value::Unit);
                let task = self.eval_mold_option(fields, "task")?.unwrap_or(Value::Unit);
                let sol = self
                    .eval_mold_option(fields, "sol")?
                    .unwrap_or_else(|| type_default.clone());
                let unm = self
                    .eval_mold_option(fields, "unm")?
                    .unwrap_or_else(|| type_default.clone());

                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("id".into(), id),
                    ("task".into(), task),
                    ("sol".into(), sol.clone()),
                    ("unm".into(), unm.clone()),
                    ("__value".into(), sol),
                    ("__default".into(), unm),
                    ("__type".into(), Value::Str("TODO".into())),
                ]))))
            }

            // Molten[](): create an opaque Molten value.
            "Molten" => {
                if !type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Molten takes no type arguments: Molten[]()".into(),
                    });
                }
                Ok(Some(Signal::Value(Value::Molten)))
            }

            // Gorillax[T](): create Gorillax.
            "Gorillax" => {
                let inner_value = if !type_args.is_empty() {
                    match self.eval_expr(&type_args[0])? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(true)),
                    ("__value".into(), inner_value),
                    ("__error".into(), Value::Unit),
                    ("__type".into(), Value::Str("Gorillax".into())),
                ]))))
            }

            // Cage[molten, function](): protected call -> Gorillax.
            "Cage" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Cage requires 2 type arguments: Cage[value, function]".into(),
                    });
                }
                let cage_value = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                if !matches!(cage_value, Value::Molten) {
                    let type_name = Self::type_name_of(&cage_value);
                    return Err(RuntimeError {
                        message: format!(
                            "Cage requires Molten type as first argument, got {}",
                            type_name
                        ),
                    });
                }
                let cage_fn = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func = match cage_fn {
                    Value::Function(f) => f,
                    _ => {
                        return Err(RuntimeError {
                            message: "Cage second argument must be a function".into(),
                        });
                    }
                };
                match self.call_function_preserving_signals(&func, &[cage_value]) {
                    Ok(Signal::Value(result)) => Ok(Some(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), result),
                        ("__error".into(), Value::Unit),
                        ("__type".into(), Value::Str("Gorillax".into())),
                    ])))),
                    Ok(Signal::Throw(err)) => Ok(Some(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Unit),
                        ("__error".into(), err),
                        ("__type".into(), Value::Str("Gorillax".into())),
                    ])))),
                    Ok(Signal::Gorilla) => Ok(Some(Signal::Gorilla)),
                    Ok(Signal::TailCall(_)) => Err(RuntimeError {
                        message: "Cage function must not use tail recursion".into(),
                    }),
                    Err(e) => Ok(Some(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Unit),
                        (
                            "__error".into(),
                            Value::Error(super::value::ErrorValue {
                                error_type: "CageError".into(),
                                message: e.message.clone(),
                                fields: Vec::new(),
                            }),
                        ),
                        ("__type".into(), Value::Str("Gorillax".into())),
                    ])))),
                }
            }

            // Div[x, y](): safe division returning Lax.
            "Div" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Div requires exactly 2 type arguments: Div[dividend, divisor]"
                            .to_string(),
                    });
                }
                let dividend = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let divisor = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let custom_default = fields
                    .iter()
                    .find(|f| f.name == "default")
                    .map(|f| self.eval_expr(&f.value))
                    .transpose()?
                    .and_then(|s| {
                        if let Signal::Value(v) = s {
                            Some(v)
                        } else {
                            None
                        }
                    });

                let (has_value, result_val, default_val) = match (&dividend, &divisor) {
                    (Value::Int(a), Value::Int(b)) => {
                        let def = custom_default.unwrap_or(Value::Int(0));
                        if *b == 0 {
                            (false, Value::Int(0), def)
                        } else {
                            (true, Value::Int(a / b), def)
                        }
                    }
                    (Value::Float(a), Value::Float(b)) => {
                        let def = custom_default.unwrap_or(Value::Float(0.0));
                        if *b == 0.0 {
                            (false, Value::Float(0.0), def)
                        } else {
                            (true, Value::Float(a / b), def)
                        }
                    }
                    (Value::Int(a), Value::Float(b)) => {
                        let def = custom_default.unwrap_or(Value::Float(0.0));
                        if *b == 0.0 {
                            (false, Value::Float(0.0), def)
                        } else {
                            (true, Value::Float(*a as f64 / b), def)
                        }
                    }
                    (Value::Float(a), Value::Int(b)) => {
                        let def = custom_default.unwrap_or(Value::Float(0.0));
                        if *b == 0 {
                            (false, Value::Float(0.0), def)
                        } else {
                            (true, Value::Float(a / *b as f64), def)
                        }
                    }
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Div: arguments must be numeric, got {} and {}",
                                dividend, divisor
                            ),
                        });
                    }
                };
                Ok(Some(Signal::Value(make_lax_value(
                    has_value, result_val, default_val,
                ))))
            }

            // Mod[x, y](): safe modulo returning Lax.
            "Mod" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "Mod requires exactly 2 type arguments: Mod[dividend, divisor]"
                            .to_string(),
                    });
                }
                let dividend = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let divisor = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let custom_default = fields
                    .iter()
                    .find(|f| f.name == "default")
                    .map(|f| self.eval_expr(&f.value))
                    .transpose()?
                    .and_then(|s| {
                        if let Signal::Value(v) = s {
                            Some(v)
                        } else {
                            None
                        }
                    });

                let (has_value, result_val, default_val) = match (&dividend, &divisor) {
                    (Value::Int(a), Value::Int(b)) => {
                        let def = custom_default.unwrap_or(Value::Int(0));
                        if *b == 0 {
                            (false, Value::Int(0), def)
                        } else {
                            (true, Value::Int(a % b), def)
                        }
                    }
                    (Value::Float(a), Value::Float(b)) => {
                        let def = custom_default.unwrap_or(Value::Float(0.0));
                        if *b == 0.0 {
                            (false, Value::Float(0.0), def)
                        } else {
                            (true, Value::Float(a % b), def)
                        }
                    }
                    (Value::Int(a), Value::Float(b)) => {
                        let def = custom_default.unwrap_or(Value::Float(0.0));
                        if *b == 0.0 {
                            (false, Value::Float(0.0), def)
                        } else {
                            (true, Value::Float(*a as f64 % b), def)
                        }
                    }
                    (Value::Float(a), Value::Int(b)) => {
                        let def = custom_default.unwrap_or(Value::Float(0.0));
                        if *b == 0 {
                            (false, Value::Float(0.0), def)
                        } else {
                            (true, Value::Float(a % *b as f64), def)
                        }
                    }
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Mod: arguments must be numeric, got {} and {}",
                                dividend, divisor
                            ),
                        });
                    }
                };
                Ok(Some(Signal::Value(make_lax_value(
                    has_value, result_val, default_val,
                ))))
            }

            // Str[x](): type conversion to Str, returning Lax.
            "Str" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let result_val = match &input {
                    Value::Int(n) => Value::Str(n.to_string()),
                    Value::Float(f) => Value::Str(f.to_string()),
                    Value::Bool(b) => Value::Str(b.to_string()),
                    Value::Str(s) => Value::Str(s.clone()),
                    other => Value::Str(format!("{}", other)),
                };
                Ok(Some(Signal::Value(make_lax_value(
                    true,
                    result_val,
                    Value::Str(String::new()),
                ))))
            }

            // Int[x](): type conversion to Int, returning Lax.
            // Int[str, base](): parse with explicit base (2..36), returning Lax.
            "Int" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let (has_value, result_val) = if type_args.len() >= 2 {
                    let base = match self.eval_expr(&type_args[1])? {
                        Signal::Value(Value::Int(v)) => v,
                        Signal::Value(_) => -1,
                        other => return Ok(Some(other)),
                    };
                    if !(2..=36).contains(&base) {
                        (false, Value::Int(0))
                    } else if let Value::Str(s) = &input {
                        let negative = s.starts_with('-');
                        let digits = if negative { &s[1..] } else { s.as_str() };
                        match i64::from_str_radix(digits, base as u32) {
                            Ok(n) => (true, Value::Int(if negative { -n } else { n })),
                            Err(_) => (false, Value::Int(0)),
                        }
                    } else {
                        (false, Value::Int(0))
                    }
                } else {
                    match &input {
                        Value::Int(n) => (true, Value::Int(*n)),
                        Value::Float(f) => (true, Value::Int(*f as i64)),
                        Value::Str(s) => match s.parse::<i64>() {
                            Ok(n) => (true, Value::Int(n)),
                            Err(_) => (false, Value::Int(0)),
                        },
                        Value::Bool(b) => (true, Value::Int(if *b { 1 } else { 0 })),
                        _ => (false, Value::Int(0)),
                    }
                };
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    result_val,
                    Value::Int(0),
                ))))
            }

            // Float[x](): type conversion to Float, returning Lax.
            "Float" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let (has_value, result_val) = match &input {
                    Value::Float(f) => (true, Value::Float(*f)),
                    Value::Int(n) => (true, Value::Float(*n as f64)),
                    Value::Str(s) => match s.parse::<f64>() {
                        Ok(f) => (true, Value::Float(f)),
                        Err(_) => (false, Value::Float(0.0)),
                    },
                    Value::Bool(b) => (true, Value::Float(if *b { 1.0 } else { 0.0 })),
                    _ => (false, Value::Float(0.0)),
                };
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    result_val,
                    Value::Float(0.0),
                ))))
            }

            // Bool[x](): type conversion to Bool, returning Lax.
            "Bool" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let (has_value, result_val) = match &input {
                    Value::Bool(b) => (true, Value::Bool(*b)),
                    Value::Int(n) => (true, Value::Bool(*n != 0)),
                    Value::Float(f) => (true, Value::Bool(*f != 0.0)),
                    Value::Str(s) => match s.as_str() {
                        "true" => (true, Value::Bool(true)),
                        "false" => (true, Value::Bool(false)),
                        _ => (false, Value::Bool(false)),
                    },
                    _ => (false, Value::Bool(false)),
                };
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    result_val,
                    Value::Bool(false),
                ))))
            }

            // UInt8[x](): range-checked conversion to Int (0..255), returning Lax[Int].
            "UInt8" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let out = match input {
                    Value::Int(n) if (0..=255).contains(&n) => Some(n),
                    Value::Float(f) if f.fract() == 0.0 && (0.0..=255.0).contains(&f) => {
                        Some(f as i64)
                    }
                    Value::Str(s) => s.parse::<i64>().ok().filter(|n| (0..=255).contains(n)),
                    _ => None,
                };
                let has_value = out.is_some();
                let val = Value::Int(out.unwrap_or(0));
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    val,
                    Value::Int(0),
                ))))
            }

            // Bytes[x](): conversion mold, returning Lax[Bytes].
            "Bytes" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let fill = fields
                    .iter()
                    .find(|f| f.name == "fill")
                    .map(|f| self.eval_expr(&f.value))
                    .transpose()?
                    .and_then(|s| {
                        if let Signal::Value(Value::Int(n)) = s {
                            Some(n)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);

                let bytes_opt: Option<Vec<u8>> = match input {
                    Value::Bytes(v) => Some(v),
                    Value::Str(s) => Some(s.into_bytes()),
                    Value::Int(len) => {
                        if len < 0 || !(0..=255).contains(&fill) {
                            None
                        } else {
                            Some(vec![fill as u8; len as usize])
                        }
                    }
                    Value::List(items) => {
                        let mut out = Vec::with_capacity(items.len());
                        let mut ok = true;
                        for item in items.iter() {
                            if let Value::Int(n) = item {
                                if (0..=255).contains(n) {
                                    out.push(*n as u8);
                                } else {
                                    ok = false;
                                    break;
                                }
                            } else {
                                ok = false;
                                break;
                            }
                        }
                        if ok { Some(out) } else { None }
                    }
                    _ => None,
                };
                let has_value = bytes_opt.is_some();
                let value = Value::Bytes(bytes_opt.unwrap_or_default());
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    value,
                    Value::Bytes(Vec::new()),
                ))))
            }

            // Char[x](): Int codepoint or single-scalar Str -> Lax[Str].
            "Char" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let out = match input {
                    Value::Int(cp) => {
                        if !(0..=0x10FFFF).contains(&cp) || (0xD800..=0xDFFF).contains(&cp) {
                            None
                        } else {
                            char::from_u32(cp as u32).map(|c| c.to_string())
                        }
                    }
                    Value::Str(s) => {
                        let mut it = s.chars();
                        let first = it.next();
                        if first.is_some() && it.next().is_none() {
                            first.map(|c| c.to_string())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let has_value = out.is_some();
                let value = Value::Str(out.unwrap_or_default());
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    value,
                    Value::Str(String::new()),
                ))))
            }

            // CodePoint[str](): single-scalar Str -> Lax[Int].
            "CodePoint" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let out = match input {
                    Value::Str(s) => {
                        let mut it = s.chars();
                        let first = it.next();
                        if first.is_some() && it.next().is_none() {
                            first.map(|c| c as i64)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let has_value = out.is_some();
                let value = Value::Int(out.unwrap_or(0));
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    value,
                    Value::Int(0),
                ))))
            }

            // Utf8Encode[str](): conversion to Lax[Bytes].
            "Utf8Encode" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let bytes = if let Value::Str(s) = input {
                    Some(s.into_bytes())
                } else {
                    None
                };
                let has_value = bytes.is_some();
                let value = Value::Bytes(bytes.unwrap_or_default());
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    value,
                    Value::Bytes(Vec::new()),
                ))))
            }

            // ── C26B-016 (Option B+): span-aware comparison molds ──
            // Accepts a span pack `@(start: Int, len: Int)` + raw Bytes and
            // provides zero-copy (byte-level) comparison / sub-span helpers
            // without materializing a Str. Matches `docs/reference/net_api.md §4`.
            // Parsing: span pack is resolved via fields "start" / "len"; out-of-bounds
            // is treated as Bool(false) / empty span rather than runtime error,
            // matching router-like tolerance semantics (hot path).
            "SpanEquals" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "SpanEquals requires 3 arguments: SpanEquals[span, raw, needle]()"
                            .into(),
                    });
                }
                let span = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let raw = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let needle = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Str(s)) => s.into_bytes(),
                    Signal::Value(Value::Bytes(b)) => b,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "SpanEquals: needle must be Str or Bytes, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let result = match (extract_span_pack(&span), raw_as_bytes(&raw)) {
                    (Some((start, len)), Some(bytes)) => {
                        let end = start.saturating_add(len);
                        if end <= bytes.len() && len == needle.len() {
                            bytes[start..end] == needle[..]
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                Ok(Some(Signal::Value(Value::Bool(result))))
            }
            "SpanStartsWith" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "SpanStartsWith requires 3 arguments: SpanStartsWith[span, raw, prefix]()"
                            .into(),
                    });
                }
                let span = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let raw = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let prefix = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Str(s)) => s.into_bytes(),
                    Signal::Value(Value::Bytes(b)) => b,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "SpanStartsWith: prefix must be Str or Bytes, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let result = match (extract_span_pack(&span), raw_as_bytes(&raw)) {
                    (Some((start, len)), Some(bytes)) => {
                        let end = start.saturating_add(len);
                        if end <= bytes.len() && len >= prefix.len() {
                            bytes[start..start + prefix.len()] == prefix[..]
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                Ok(Some(Signal::Value(Value::Bool(result))))
            }
            "SpanContains" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "SpanContains requires 3 arguments: SpanContains[span, raw, needle]()"
                            .into(),
                    });
                }
                let span = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let raw = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let needle = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Str(s)) => s.into_bytes(),
                    Signal::Value(Value::Bytes(b)) => b,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "SpanContains: needle must be Str or Bytes, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let result = match (extract_span_pack(&span), raw_as_bytes(&raw)) {
                    (Some((start, len)), Some(bytes)) => {
                        let end = start.saturating_add(len);
                        if end <= bytes.len() && !needle.is_empty() && len >= needle.len() {
                            bytes[start..end]
                                .windows(needle.len())
                                .any(|w| w == needle.as_slice())
                        } else if needle.is_empty() {
                            // Empty needle is always contained (stdlib convention).
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                Ok(Some(Signal::Value(Value::Bool(result))))
            }
            // ── C26B-016 (@c.26, Option B+): StrOf mold ──
            // `StrOf[span, raw]() -> Str` — cold-path materialization of a
            // span pack into an owned `Str`. Unlike `SpanEquals` / `SpanStartsWith`
            // (zero-copy hot-path comparisons), `StrOf` allocates a new Str and
            // is intended for logging / JSON parsing / anywhere the user needs
            // an owned string copy.
            //
            // Invalid UTF-8 span / OOB span → empty `Str` (tolerant semantics,
            // consistent with Span* family). This differs from `Utf8Decode`
            // which returns `Lax[Str]` — `StrOf` returns `Str` directly for
            // convenience in cold-path use (see `docs/reference/net_api.md §4.1`).
            //
            // Native backend does not yet wire a dedicated runtime helper
            // (`taida_net_StrOf`); users on native should use the equivalent
            // `Utf8Decode[Slice[raw, span.start, span.start + span.len]]().getOrDefault("")`
            // composition. Tracked in `.dev/C26_BLOCKERS.md::C26B-016 native strOf`.
            "StrOf" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "StrOf requires 2 arguments: StrOf[span, raw]()"
                            .into(),
                    });
                }
                let span = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let raw = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let result = match (extract_span_pack(&span), raw_as_bytes(&raw)) {
                    (Some((start, len)), Some(bytes)) => {
                        let end = start.saturating_add(len);
                        if end <= bytes.len() {
                            // Valid span range: try UTF-8 decode. Invalid UTF-8
                            // falls back to empty string (tolerant).
                            std::str::from_utf8(&bytes[start..end])
                                .map(|s| s.to_string())
                                .unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                };
                Ok(Some(Signal::Value(Value::Str(result))))
            }

            "SpanSlice" => {
                if type_args.len() < 4 {
                    return Err(RuntimeError {
                        message: "SpanSlice requires 4 arguments: SpanSlice[span, raw, start, end]()"
                            .into(),
                    });
                }
                let span = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let _raw = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let sub_start = match self.eval_expr(&type_args[2])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "SpanSlice: start must be Int, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let sub_end = match self.eval_expr(&type_args[3])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "SpanSlice: end must be Int, got {}",
                                v
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let (base_start, base_len) = extract_span_pack(&span).unwrap_or((0, 0));
                let (a, b) = clamp_slice_bounds(base_len, sub_start, sub_end);
                let new_start = base_start.saturating_add(a);
                let new_len = b.saturating_sub(a);
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("start".into(), Value::Int(new_start as i64)),
                    ("len".into(), Value::Int(new_len as i64)),
                ]))))
            }

            // Utf8Decode[bytes](): conversion to Lax[Str].
            "Utf8Decode" if !type_args.is_empty() => {
                let input = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let out = if let Value::Bytes(bytes) = input {
                    String::from_utf8(bytes).ok()
                } else {
                    None
                };
                let has_value = out.is_some();
                let value = Value::Str(out.unwrap_or_default());
                Ok(Some(Signal::Value(make_lax_value(
                    has_value,
                    value,
                    Value::Str(String::new()),
                ))))
            }

            _ => {
                if let Some(result) = self.try_list_mold_op(name, type_args, fields)? {
                    return Ok(Some(result));
                }
                self.try_os_mold(name, type_args, fields)
            }
        }
    }

    // ── List Operation Mold Types (legacy HOF molds) ────────

    /// Try to evaluate a built-in list operation mold type.
    /// Returns None if the name is not a recognized list operation.
    pub(crate) fn try_list_mold_op(
        &mut self,
        name: &str,
        type_args: &[Expr],
        _fields: &[crate::parser::BuchiField],
    ) -> Result<Option<Signal>, RuntimeError> {
        match name {
            "Map" => {
                // Map[list, fn]() -> list with fn applied to each element
                // Map[stream, fn]() -> Stream with Map transform appended (lazy)
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "Map requires exactly 2 type arguments: Map[list, fn]".to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func = match &func_val {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Map: second argument must be a function, got {}",
                                func_val
                            ),
                        });
                    }
                };
                // Stream input: append Map transform (lazy evaluation)
                if let Value::Stream(s) = list_val {
                    let mut transforms = s.transforms.clone();
                    transforms.push(StreamTransform::Map(func));
                    return Ok(Some(Signal::Value(Value::Stream(StreamValue {
                        items: s.items,
                        transforms,
                        status: s.status,
                    }))));
                }
                let items = match &list_val {
                    Value::List(items) => items.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Map: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let mut result = Vec::new();
                for item in items.iter() {
                    let mapped =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    result.push(mapped);
                }
                Ok(Some(Signal::Value(Value::list(result))))
            }

            "Filter" => {
                // Filter[list, fn]() -> list with only elements where fn returns true
                // Filter[stream, fn]() -> Stream with Filter transform appended (lazy)
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "Filter requires exactly 2 type arguments: Filter[list, fn]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func = match &func_val {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Filter: second argument must be a function, got {}",
                                func_val
                            ),
                        });
                    }
                };
                // Stream input: append Filter transform (lazy evaluation)
                if let Value::Stream(s) = list_val {
                    let mut transforms = s.transforms.clone();
                    transforms.push(StreamTransform::Filter(func));
                    return Ok(Some(Signal::Value(Value::Stream(StreamValue {
                        items: s.items,
                        transforms,
                        status: s.status,
                    }))));
                }
                let items = match &list_val {
                    Value::List(items) => items.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Filter: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let mut result = Vec::new();
                for item in items.iter() {
                    let keep = self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if keep.is_truthy() {
                        result.push(item.clone());
                    }
                }
                Ok(Some(Signal::Value(Value::list(result))))
            }

            "Fold" | "Reduce" => {
                // Fold[list, init, fn]() -> accumulated value
                if type_args.len() != 3 {
                    return Err(RuntimeError {
                        message: "Fold requires exactly 3 type arguments: Fold[list, init, fn]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let init_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func_val = match self.eval_expr(&type_args[2])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items: Vec<Value> = match &list_val {
                    Value::List(items) => items.as_ref().clone(),
                    Value::Stream(s) => self.collect_stream_items(s)?,
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Fold: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let func = match &func_val {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Fold: third argument must be a function, got {}",
                                func_val
                            ),
                        });
                    }
                };
                let mut acc = init_val;
                for item in &items {
                    acc = self.call_function_with_values(&func, &[acc, item.clone()])?;
                }
                Ok(Some(Signal::Value(acc)))
            }

            "Foldr" => {
                // Foldr[list, init, fn]() -> accumulated value (right fold)
                if type_args.len() != 3 {
                    return Err(RuntimeError {
                        message: "Foldr requires exactly 3 type arguments: Foldr[list, init, fn]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let init_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func_val = match self.eval_expr(&type_args[2])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items: Vec<Value> = match &list_val {
                    Value::List(items) => items.as_ref().clone(),
                    Value::Stream(s) => self.collect_stream_items(s)?,
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Foldr: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let func = match &func_val {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Foldr: third argument must be a function, got {}",
                                func_val
                            ),
                        });
                    }
                };
                let mut acc = init_val;
                for item in items.iter().rev() {
                    acc = self.call_function_with_values(&func, &[acc, item.clone()])?;
                }
                Ok(Some(Signal::Value(acc)))
            }

            "Take" => {
                // Take[list, n]() -> first n elements
                // Take[stream, n]() -> Stream with Take transform appended (lazy)
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "Take requires exactly 2 type arguments: Take[list, n]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let n_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let n = match &n_val {
                    Value::Int(n) => *n as usize,
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Take: second argument must be an integer, got {}",
                                n_val
                            ),
                        });
                    }
                };
                // Stream input: append Take transform (lazy evaluation)
                if let Value::Stream(s) = list_val {
                    let mut transforms = s.transforms.clone();
                    transforms.push(StreamTransform::Take(n));
                    return Ok(Some(Signal::Value(Value::Stream(StreamValue {
                        items: s.items,
                        transforms,
                        status: s.status,
                    }))));
                }
                let items: Vec<Value> = match &list_val {
                    Value::List(items) => items.as_ref().clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Take: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let result: Vec<Value> = items.into_iter().take(n).collect();
                Ok(Some(Signal::Value(Value::list(result))))
            }

            "TakeWhile" => {
                // TakeWhile[list, fn]() -> elements while fn returns true
                // TakeWhile[stream, fn]() -> Stream with TakeWhile transform appended (lazy)
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "TakeWhile requires exactly 2 type arguments: TakeWhile[list, fn]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func = match &func_val {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "TakeWhile: second argument must be a function, got {}",
                                func_val
                            ),
                        });
                    }
                };
                // Stream input: append TakeWhile transform (lazy evaluation)
                if let Value::Stream(s) = list_val {
                    let mut transforms = s.transforms.clone();
                    transforms.push(StreamTransform::TakeWhile(func));
                    return Ok(Some(Signal::Value(Value::Stream(StreamValue {
                        items: s.items,
                        transforms,
                        status: s.status,
                    }))));
                }
                let items = match &list_val {
                    Value::List(items) => items.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "TakeWhile: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let mut result = Vec::new();
                for item in items.iter() {
                    let keep = self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if keep.is_truthy() {
                        result.push(item.clone());
                    } else {
                        break;
                    }
                }
                Ok(Some(Signal::Value(Value::list(result))))
            }

            "Drop" => {
                // Drop[list, n]() -> skip first n elements
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "Drop requires exactly 2 type arguments: Drop[list, n]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let n_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items: Vec<Value> = match &list_val {
                    Value::List(items) => items.as_ref().clone(),
                    Value::Stream(s) => self.collect_stream_items(s)?,
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Drop: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let n = match &n_val {
                    Value::Int(n) => *n as usize,
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Drop: second argument must be an integer, got {}",
                                n_val
                            ),
                        });
                    }
                };
                let result: Vec<Value> = items.into_iter().skip(n).collect();
                Ok(Some(Signal::Value(Value::list(result))))
            }

            "DropWhile" => {
                // DropWhile[list, fn]() -> skip elements while fn returns true
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "DropWhile requires exactly 2 type arguments: DropWhile[list, fn]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let func_val = match self.eval_expr(&type_args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items: Vec<Value> = match &list_val {
                    Value::List(items) => items.as_ref().clone(),
                    Value::Stream(s) => self.collect_stream_items(s)?,
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "DropWhile: first argument must be a list or stream, got {}",
                                list_val
                            ),
                        });
                    }
                };
                let func = match &func_val {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "DropWhile: second argument must be a function, got {}",
                                func_val
                            ),
                        });
                    }
                };
                let mut dropping = true;
                let mut result = Vec::new();
                for item in &items {
                    if dropping {
                        let skip =
                            self.call_function_with_values(&func, std::slice::from_ref(item))?;
                        if skip.is_truthy() {
                            continue;
                        } else {
                            dropping = false;
                        }
                    }
                    result.push(item.clone());
                }
                Ok(Some(Signal::Value(Value::list(result))))
            }

            // ── JSON Mold Type (Molten Iron) ─────────────────
            // JSON[raw, Schema]() — cast raw JSON through schema
            // JSON[raw]() — ERROR: schema required
            // JSON() — ERROR: schema required
            "JSON" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "JSON requires a schema type argument: JSON[raw, Schema](). Raw JSON cannot be used without a schema.".to_string(),
                    });
                }

                // Evaluate the raw data (first type arg)
                let raw_value = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };

                // Parse raw data into serde_json::Value
                let json_data = match &raw_value {
                    Value::Str(s) => {
                        match serde_json::from_str::<serde_json::Value>(s) {
                            Ok(parsed) => parsed,
                            Err(e) => {
                                // Parse error → return Lax with hasValue=false
                                let schema = self.resolve_json_schema(&type_args[1])?;
                                let default_val =
                                    crate::interpreter::json::default_for_schema(&schema);
                                return Ok(Some(Signal::Value(Value::BuchiPack(vec![
                                    ("hasValue".into(), Value::Bool(false)),
                                    ("__value".into(), default_val.clone()),
                                    ("__default".into(), default_val),
                                    ("__type".into(), Value::Str("Lax".into())),
                                    (
                                        "__error".into(),
                                        Value::Str(format!("JSON parse error: {}", e)),
                                    ),
                                ]))));
                            }
                        }
                    }
                    Value::Json(j) => j.clone(),
                    other => {
                        // Convert Taida value to JSON first
                        crate::interpreter::json::taida_value_to_json(other)
                    }
                };

                // Resolve the schema from the second type arg
                let schema = self.resolve_json_schema(&type_args[1])?;

                // Cast JSON data through schema
                let typed_value =
                    crate::interpreter::json::json_to_typed_value(&json_data, &schema);
                let default_val = crate::interpreter::json::default_for_schema(&schema);

                // Return as Lax (JSON parsing can fail)
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(true)),
                    ("__value".into(), typed_value),
                    ("__default".into(), default_val),
                    ("__type".into(), Value::Str("Lax".into())),
                ]))))
            }

            // ── Async Mold Types ────────────────────────────
            "Async" => {
                // Async[value]() -> Async value (immediately fulfilled)
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Async requires at least 1 type argument: Async[value]"
                            .to_string(),
                    });
                }
                let inner_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Fulfilled,
                    value: Box::new(inner_val),
                    error: Box::new(Value::Unit),
                    task: None,
                }))))
            }

            "AsyncReject" => {
                // AsyncReject[error]() -> Async value (immediately rejected)
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "AsyncReject requires 1 type argument: AsyncReject[error]"
                            .to_string(),
                    });
                }
                let error_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Rejected,
                    value: Box::new(Value::Unit),
                    error: Box::new(error_val),
                    task: None,
                }))))
            }

            "All" => {
                // All[asyncList]() -> await all async values, collecting results.
                // If any pending tasks exist, resolve them via tokio runtime.
                // If any is rejected, the whole thing is rejected (throw).
                if type_args.len() != 1 {
                    return Err(RuntimeError {
                        message: "All requires exactly 1 type argument: All[asyncList]".to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items = match &list_val {
                    Value::List(items) => items.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!("All: argument must be a list, got {}", list_val),
                        });
                    }
                };

                // First, resolve any pending async values
                let mut resolved_items = Vec::new();
                for item in items.iter() {
                    match item {
                        Value::Async(a) => {
                            let resolved = self.resolve_async(a)?;
                            resolved_items.push(Value::Async(resolved));
                        }
                        other => resolved_items.push(other.clone()),
                    }
                }

                // Collect all results; if any is rejected, the whole thing is rejected
                let mut results = Vec::new();
                for item in &resolved_items {
                    match item {
                        Value::Async(a) => {
                            if a.status == AsyncStatus::Rejected {
                                // Propagate rejection as a throw
                                return Ok(Some(Signal::Throw((*a.error).clone())));
                            }
                            results.push((*a.value).clone());
                        }
                        other => {
                            // Non-async values are treated as immediately resolved
                            results.push(other.clone());
                        }
                    }
                }
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Fulfilled,
                    value: Box::new(Value::list(results)),
                    error: Box::new(Value::Unit),
                    task: None,
                }))))
            }

            "Race" => {
                // Race[asyncList]() -> return first resolved value.
                // With tokio, pending tasks are resolved and the first completion wins.
                if type_args.len() != 1 {
                    return Err(RuntimeError {
                        message: "Race requires exactly 1 type argument: Race[asyncList]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items = match &list_val {
                    Value::List(items) => items.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!("Race: argument must be a list, got {}", list_val),
                        });
                    }
                };

                // Check for already-resolved items first (fast path)
                for item in items.iter() {
                    if let Value::Async(a) = item
                        && a.status == AsyncStatus::Fulfilled
                        && a.task.is_none()
                    {
                        return Ok(Some(Signal::Value(Value::Async(AsyncValue {
                            status: AsyncStatus::Fulfilled,
                            value: a.value.clone(),
                            error: Box::new(Value::Unit),
                            task: None,
                        }))));
                    }
                }

                // If we have pending tasks, resolve the first one
                if let Some(first) = items.first() {
                    match first {
                        Value::Async(a) => {
                            let resolved = self.resolve_async(a)?;
                            if resolved.status == AsyncStatus::Rejected {
                                return Ok(Some(Signal::Throw((*resolved.error).clone())));
                            }
                            Ok(Some(Signal::Value(Value::Async(AsyncValue {
                                status: AsyncStatus::Fulfilled,
                                value: resolved.value,
                                error: Box::new(Value::Unit),
                                task: None,
                            }))))
                        }
                        other => Ok(Some(Signal::Value(Value::Async(AsyncValue {
                            status: AsyncStatus::Fulfilled,
                            value: Box::new(other.clone()),
                            error: Box::new(Value::Unit),
                            task: None,
                        })))),
                    }
                } else {
                    Ok(Some(Signal::Value(Value::Async(AsyncValue {
                        status: AsyncStatus::Fulfilled,
                        value: Box::new(Value::Unit),
                        error: Box::new(Value::Unit),
                        task: None,
                    }))))
                }
            }

            "Timeout" => {
                // Timeout[async, ms]() -> await with timeout.
                // For resolved values, timeout has no effect.
                // For pending tasks, tokio::time::timeout is used.
                if type_args.len() != 2 {
                    return Err(RuntimeError {
                        message: "Timeout requires 2 type arguments: Timeout[async, ms]"
                            .to_string(),
                    });
                }
                let async_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let timeout_ms = match self.eval_expr(&type_args[1])? {
                    Signal::Value(Value::Int(ms)) => ms as u64,
                    Signal::Value(Value::Float(ms)) => ms as u64,
                    Signal::Value(other) => {
                        return Err(RuntimeError {
                            message: format!(
                                "Timeout: second argument must be a number (ms), got {}",
                                other
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };

                match async_val {
                    Value::Async(a) => {
                        if a.task.is_some() {
                            // Pending task: use tokio::time::timeout
                            match self.resolve_async_with_timeout(&a, timeout_ms)? {
                                Some(resolved) => Ok(Some(Signal::Value(Value::Async(resolved)))),
                                None => {
                                    // Timeout expired
                                    Ok(Some(Signal::Throw(Value::Error(
                                        super::value::ErrorValue {
                                            error_type: "TimeoutError".into(),
                                            message: format!(
                                                "Async operation timed out after {}ms",
                                                timeout_ms
                                            ),
                                            fields: Vec::new(),
                                        },
                                    ))))
                                }
                            }
                        } else {
                            // Already resolved: timeout has no effect
                            Ok(Some(Signal::Value(Value::Async(a))))
                        }
                    }
                    other => Ok(Some(Signal::Value(Value::Async(AsyncValue {
                        status: AsyncStatus::Fulfilled,
                        value: Box::new(other),
                        error: Box::new(Value::Unit),
                        task: None,
                    })))),
                }
            }

            "Cancel" => {
                // Cancel[async]() -> rejected Async (CancelledError) for pending async.
                // Non-async values are treated as already-fulfilled Async.
                if type_args.len() != 1 {
                    return Err(RuntimeError {
                        message: "Cancel requires 1 type argument: Cancel[async]".to_string(),
                    });
                }
                let async_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };

                match async_val {
                    Value::Async(a) => {
                        if a.status == AsyncStatus::Pending {
                            Ok(Some(Signal::Value(Value::Async(AsyncValue {
                                status: AsyncStatus::Rejected,
                                value: Box::new(Value::Unit),
                                error: Box::new(Value::Error(super::value::ErrorValue {
                                    error_type: "CancelledError".into(),
                                    message: "Async operation cancelled".into(),
                                    fields: Vec::new(),
                                })),
                                task: None,
                            }))))
                        } else {
                            Ok(Some(Signal::Value(Value::Async(a))))
                        }
                    }
                    other => Ok(Some(Signal::Value(Value::Async(AsyncValue {
                        status: AsyncStatus::Fulfilled,
                        value: Box::new(other),
                        error: Box::new(Value::Unit),
                        task: None,
                    })))),
                }
            }

            // ── Stream Mold Types ────────────────────────────
            "Stream" => {
                // Stream[value]() -> Stream with a single value (for testing)
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Stream requires at least 1 type argument: Stream[value]"
                            .to_string(),
                    });
                }
                let inner_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(Value::Stream(StreamValue {
                    items: vec![inner_val],
                    transforms: Vec::new(),
                    status: StreamStatus::Completed,
                }))))
            }

            "StreamFrom" => {
                // StreamFrom[list]() -> Stream from a list (for testing)
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "StreamFrom requires 1 type argument: StreamFrom[list]"
                            .to_string(),
                    });
                }
                let list_val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let items: Vec<Value> = match &list_val {
                    Value::List(items) => items.as_ref().clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "StreamFrom: argument must be a list, got {}",
                                list_val
                            ),
                        });
                    }
                };
                Ok(Some(Signal::Value(Value::Stream(StreamValue {
                    items,
                    transforms: Vec::new(),
                    status: StreamStatus::Completed,
                }))))
            }

            // ── B11-5a: If[cond, then_value, else_value]() ──────
            // Short-circuit: only the selected branch is evaluated.
            "If" => {
                if type_args.len() < 3 {
                    return Err(RuntimeError {
                        message: "If requires 3 arguments: If[condition, then_value, else_value]()"
                            .into(),
                    });
                }
                let cond = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v.is_truthy(),
                    other => return Ok(Some(other)),
                };
                if cond {
                    match self.eval_expr(&type_args[1])? {
                        Signal::Value(v) => Ok(Some(Signal::Value(v))),
                        other => Ok(Some(other)),
                    }
                } else {
                    match self.eval_expr(&type_args[2])? {
                        Signal::Value(v) => Ok(Some(Signal::Value(v))),
                        other => Ok(Some(other)),
                    }
                }
            }

            // ── B11-6b: TypeIs[value, :TypeName]() / TypeIs[value, EnumName:Variant]() ──
            // Returns Bool: true if the runtime value matches the given type.
            "TypeIs" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "TypeIs requires 2 arguments: TypeIs[value, :TypeName]()".into(),
                    });
                }
                let val = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                // The second arg is a TypeLiteral or an evaluated expression
                let result = match &type_args[1] {
                    // TypeLiteral with variant: enum variant check
                    Expr::TypeLiteral(enum_name, Some(variant_name), _) => {
                        // Look up the enum definition and find the variant ordinal
                        if let Some(variants) = self.enum_defs.get(enum_name.as_str()) {
                            if let Some(ordinal) = variants.iter().position(|v| v == variant_name) {
                                // C18-2: Accept both the legacy `Value::Int(n)`
                                // representation (kept for any code path that
                                // still produces raw ints from Enum sources)
                                // AND the C18-2 tagged `Value::EnumVal(name, n)`
                                // where the enum name must match. The latter
                                // prevents a cross-enum false positive.
                                match &val {
                                    Value::Int(n) => *n == ordinal as i64,
                                    Value::EnumVal(n_enum, n) => {
                                        n_enum == enum_name && *n == ordinal as i64
                                    }
                                    _ => false,
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    // TypeLiteral without variant: primitive/named type check
                    Expr::TypeLiteral(type_name, None, _) => match type_name.as_str() {
                        "Int" => matches!(val, Value::Int(_)),
                        "Float" => matches!(val, Value::Float(_)),
                        "Num" => matches!(val, Value::Int(_) | Value::Float(_)),
                        "Str" => matches!(val, Value::Str(_)),
                        "Bool" => matches!(val, Value::Bool(_)),
                        "Bytes" => matches!(val, Value::Bytes(_)),
                        // B11B-015: Error check includes error subtypes via __type + inheritance
                        "Error" => match &val {
                            Value::Error(_) => true,
                            Value::BuchiPack(fields) => {
                                if let Some((_, Value::Str(t))) =
                                    fields.iter().find(|(n, _)| n == "__type")
                                {
                                    self.check_type_extends(t, "Error")
                                } else {
                                    false
                                }
                            }
                            _ => false,
                        },
                        // B11B-015: Named type check via __type field + inheritance chain
                        other => match &val {
                            Value::BuchiPack(fields) => {
                                if let Some((_, Value::Str(t))) =
                                    fields.iter().find(|(n, _)| n == "__type")
                                {
                                    t == other || self.check_type_extends(t, other)
                                } else {
                                    false
                                }
                            }
                            Value::Error(e) => {
                                e.error_type == other
                                    || self.check_type_extends(&e.error_type, other)
                            }
                            _ => false,
                        },
                    },
                    // Fallback: evaluate as string and compare
                    other => {
                        let type_str = match self.eval_expr(other)? {
                            Signal::Value(v) => v.to_display_string(),
                            other_sig => return Ok(Some(other_sig)),
                        };
                        match type_str.as_str() {
                            "Int" => matches!(val, Value::Int(_)),
                            "Float" => matches!(val, Value::Float(_)),
                            "Num" => matches!(val, Value::Int(_) | Value::Float(_)),
                            "Str" => matches!(val, Value::Str(_)),
                            "Bool" => matches!(val, Value::Bool(_)),
                            "Bytes" => matches!(val, Value::Bytes(_)),
                            "Error" => matches!(val, Value::Error(_)),
                            _ => false,
                        }
                    }
                };
                Ok(Some(Signal::Value(Value::Bool(result))))
            }

            // ── B11-6b: TypeExtends[:TypeA, :TypeB]() ──
            // Returns Bool: true if TypeA is the same as or a subtype of TypeB.
            // Pure compile-time operation; interpreter uses simple name matching.
            "TypeExtends" => {
                if type_args.len() < 2 {
                    return Err(RuntimeError {
                        message: "TypeExtends requires 2 arguments: TypeExtends[:TypeA, :TypeB]()"
                            .into(),
                    });
                }
                let type_a = match &type_args[0] {
                    Expr::TypeLiteral(name, _, _) => name.clone(),
                    other => match self.eval_expr(other)? {
                        Signal::Value(v) => v.to_display_string(),
                        other_sig => return Ok(Some(other_sig)),
                    },
                };
                let type_b = match &type_args[1] {
                    Expr::TypeLiteral(name, _, _) => name.clone(),
                    other => match self.eval_expr(other)? {
                        Signal::Value(v) => v.to_display_string(),
                        other_sig => return Ok(Some(other_sig)),
                    },
                };
                // Same type → true
                let result = if type_a == type_b {
                    true
                } else {
                    // Check numeric hierarchy: Int < Num, Float < Num
                    match (type_a.as_str(), type_b.as_str()) {
                        ("Int", "Num") | ("Float", "Num") | ("Int", "Float") => true,
                        _ => {
                            // Check inheritance chain using type_defs_inherited
                            self.check_type_extends(&type_a, &type_b)
                        }
                    }
                };
                Ok(Some(Signal::Value(Value::Bool(result))))
            }

            // ── JS-backend-only mold types ──────────────────────
            // These molds operate on Molten values and are only available
            // in the JS transpiler backend.
            "JSNew" => Err(RuntimeError {
                message: "JSNew is only available in the JS transpiler backend".to_string(),
            }),
            "JSSet" => Err(RuntimeError {
                message: "JSSet is only available in the JS transpiler backend".to_string(),
            }),
            "JSBind" => Err(RuntimeError {
                message: "JSBind is only available in the JS transpiler backend".to_string(),
            }),
            "JSSpread" => Err(RuntimeError {
                message: "JSSpread is only available in the JS transpiler backend".to_string(),
            }),

            // ── C18-3: Ordinal[Enum:Variant()]() — explicit Enum → Int ──
            //
            // Returns the ordinal of an Enum value as an Int. This is the
            // sanctioned path for interop with Int-typed columns / wire
            // formats. Accepts any Enum value (`EnumVal(_, n)` produced by
            // `Expr::EnumVariant`) and returns `Int(n)`.
            //
            // Rejects:
            //   - non-Enum values (Int, Str, Float, Bool, etc.) with a
            //     deterministic RuntimeError so the author cannot silently
            //     use `Ordinal[]` as a generic identity function;
            //   - zero arguments (`Ordinal[]()` is meaningless).
            //
            // Note: the inverse direction (`FromOrdinal[Color, 1]()`) is
            // C18 scope-out. See `.dev/C18_DESIGN.md` §Scope外.
            "Ordinal" => {
                if type_args.is_empty() {
                    return Err(RuntimeError {
                        message: "Ordinal requires 1 argument: Ordinal[<enum_value>]()".into(),
                    });
                }
                let v = match self.eval_expr(&type_args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                match v {
                    Value::EnumVal(_, ordinal) => Ok(Some(Signal::Value(Value::Int(ordinal)))),
                    other => Err(RuntimeError {
                        message: format!(
                            "Ordinal: argument must be an Enum value, got {}. \
                             Hint: pass an Enum variant such as `Ordinal[Color:Red()]()`.",
                            Self::type_name_of(&other)
                        ),
                    }),
                }
            }

            _ => Ok(None),
        }
    }
}
