use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, StreamStatus, StreamValue, Value};
use super::value_key::ValueKey;
/// Method dispatch for Taida values (auto-mold methods).
///
/// This module contains eval_method_call and all type-specific method
/// implementations: Str, Num, List, Bool, Async, Result,
/// HashMap, Set, and user-defined type methods.
///
/// NOTE: JSON methods are ABOLISHED (Molten Iron design).
/// JSON is opaque — no methods allowed. Cast through a schema: JSON[raw, Schema]()
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use crate::parser::FuncDef;
use std::collections::HashSet;

/// C25B-022 (Phase 5-C) / C25B-023 (Phase 5-E) — fast-path helper.
///
/// Try to build a set of `u64` fingerprints (derived from `ValueKey`)
/// over `items`. Returns `None` if any element is not key-eligible
/// (see `value_key.rs` for the classification); the caller must then
/// fall back to the pre-existing `Vec::contains` linear scan, which
/// preserves full `Value::eq` semantics including Int↔Float↔EnumVal
/// coercion. When `Some(fingerprints)` is returned, callers can probe
/// membership in O(1) and skip the O(N) linear walk.
///
/// Using a fingerprint (not `ValueKey<'a>` directly) sidesteps the
/// lifetime plumbing that would be required to carry borrowed
/// `ValueKey<'a>` values across the `eval_*_method` entry points —
/// all of which operate on owned `Vec<Value>` captured from the
/// receiver. Fingerprint collision risk is u64-wide and negligible
/// for the scales we're targeting (tens of thousands of entries).
/// On a fingerprint hit the caller still runs `Value::eq` to confirm
/// so false positives are impossible.
fn try_build_fingerprint_set(items: &[Value]) -> Option<HashSet<u64>> {
    let mut set = HashSet::with_capacity(items.len());
    for it in items {
        let key = ValueKey::new(it)?;
        set.insert(key.fingerprint());
    }
    Some(set)
}

impl Interpreter {
    /// Evaluate auto-mold method calls on values.
    pub(crate) fn eval_method_call(
        &mut self,
        obj: &Value,
        method: &str,
        args: &[crate::parser::Expr],
    ) -> Result<Signal, RuntimeError> {
        // Evaluate args
        let mut arg_values = Vec::new();
        for arg in args {
            let val = match self.eval_expr(arg)? {
                Signal::Value(v) => v,
                other => return Ok(other),
            };
            arg_values.push(val);
        }

        match obj {
            Value::Str(s) => self.eval_str_method(s, method, &arg_values),
            Value::Bytes(bytes) => self.eval_bytes_method(bytes, method, &arg_values),
            Value::Int(n) => self.eval_num_method(&Value::Int(*n), method, &arg_values),
            Value::Float(n) => self.eval_num_method(&Value::Float(*n), method, &arg_values),
            Value::List(items) => self.eval_list_method(items, method, &arg_values),
            Value::Bool(b) => self.eval_bool_method(*b, method, &arg_values),
            Value::Json(_) => {
                // JSON is opaque (Molten Iron) — no methods allowed.
                // JSON data must be cast through a schema: JSON[raw, Schema]()
                Err(RuntimeError {
                    message: format!(
                        "Cannot call method '{}' on JSON. JSON is opaque — cast it through a schema first: JSON[raw, Schema]()",
                        method
                    ),
                })
            }
            Value::Molten => {
                // Molten is opaque — no methods allowed.
                // Molten data can only be manipulated inside Cage.
                Err(RuntimeError {
                    message: format!(
                        "Cannot call method '{}' on Molten. Molten is opaque — it can only be used inside Cage.",
                        method
                    ),
                })
            }
            Value::Stream(s) => self.eval_stream_method(s, method, &arg_values),
            Value::Async(a) => self.eval_async_method(a, method, &arg_values),
            Value::BuchiPack(fields) => {
                // Check if method is "throw"
                if method == "throw" {
                    return Ok(Signal::Throw(obj.clone()));
                }
                // Check __type for typed BuchiPack dispatch
                let type_name = fields
                    .iter()
                    .find(|(n, _)| n == "__type")
                    .and_then(|(_, v)| {
                        if let Value::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    });
                match type_name {
                    Some("Optional") => Err(RuntimeError {
                        message: "Optional has been removed. Use Lax[value]() instead. Lax[T] provides the same safety with default value guarantees.".to_string(),
                    }),
                    Some("Result") => self.eval_result_method(fields, method, &arg_values),
                    Some("Lax") => self.eval_lax_method(fields, method, &arg_values),
                    Some("Gorillax") => self.eval_gorillax_method(fields, method, &arg_values),
                    Some("RelaxedGorillax") => self.eval_relaxed_gorillax_method(fields, method, &arg_values),
                    Some("HashMap") => self.eval_hashmap_method(fields, method, &arg_values),
                    Some("Set") => self.eval_set_method(fields, method, &arg_values),
                    _ => {
                        // .unmold() on custom mold: delegate to unmold_value() which
                        // handles __unmold (with Signal::Throw propagation) and
                        // __value fallback — exactly the same path as ]=> / <=[
                        if method == "unmold" {
                            return self.unmold_value(obj.clone());
                        }
                        // Try user-defined methods from type_methods
                        if let Some(type_name_str) = type_name {
                            let func_def_opt = self.type_methods
                                .get(type_name_str)
                                .and_then(|methods| methods.get(method))
                                .cloned();
                            if let Some(func_def) = func_def_opt {
                                return self.eval_user_method(&func_def, fields, &arg_values);
                            }
                        }
                        // Fallback: check for Function field in BuchiPack
                        if let Some((_, Value::Function(func))) = fields.iter().find(|(n, _)| n == method) {
                            let func = func.clone();
                            return self.call_function_with_values(&func, &arg_values)
                                .map(Signal::Value);
                        }
                        // C12-2b: .toString() universal fallback for BuchiPacks.
                        // All types get `.toString()` as a display helper, equivalent
                        // to the Rust side's `to_display_string()`.
                        if method == "toString" {
                            return Ok(Signal::Value(Value::Str(obj.to_display_string())));
                        }
                        Err(RuntimeError {
                            message: format!("Unknown method '{}' on {}", method, obj.to_error_display(200)),
                        })
                    }
                }
            }
            Value::Error(_) => {
                if method == "throw" {
                    Ok(Signal::Throw(obj.clone()))
                } else {
                    Err(RuntimeError {
                        message: format!(
                            "Unknown method '{}' on {}",
                            method,
                            obj.to_error_display(200)
                        ),
                    })
                }
            }
            _ => {
                // C12-2b: .toString() universal fallback for values that do not
                // belong to any specialised dispatch table (Function, Gorilla, etc.).
                if method == "toString" {
                    return Ok(Signal::Value(Value::Str(obj.to_display_string())));
                }
                Err(RuntimeError {
                    message: format!(
                        "Cannot call method '{}' on {}",
                        method,
                        obj.to_error_display(200)
                    ),
                })
            }
        }
    }

    // ── Helper: extract display string from a throw value ─────
    fn throw_val_to_display_str(throw_val: &Value) -> String {
        match throw_val {
            Value::Error(err) => {
                if !err.message.is_empty() {
                    err.message.clone()
                } else if !err.error_type.is_empty() {
                    err.error_type.clone()
                } else {
                    "error".to_string()
                }
            }
            Value::BuchiPack(fields) => {
                // TypeInst error: look for message field, then type field
                fields
                    .iter()
                    .find(|(n, _)| n == "message")
                    .and_then(|(_, v)| {
                        if let Value::Str(s) = v {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        fields
                            .iter()
                            .find(|(n, _)| n == "__type")
                            .and_then(|(_, v)| {
                                if let Value::Str(s) = v {
                                    Some(s.clone())
                                } else {
                                    None
                                }
                            })
                    })
                    .unwrap_or_else(|| throw_val.to_display_string())
            }
            Value::Str(s) => s.clone(),
            other => other.to_display_string(),
        }
    }

    // ── JSON methods — ALL ABOLISHED (Molten Iron) ─────────────
    // JSON is opaque. No methods. Cast through a schema: JSON[raw, Schema]()

    // ── User-defined type methods ────────────────────────────

    fn eval_user_method(
        &mut self,
        func_def: &FuncDef,
        instance_fields: &[(String, Value)],
        arg_values: &[Value],
    ) -> Result<Signal, RuntimeError> {
        // Instance fields scope (like closure scope)
        self.env.push_scope();
        // Inject instance fields as closure variables
        for (name, val) in instance_fields {
            if name != "__type" {
                self.env.define_force(name, val.clone());
            }
        }
        // Local scope for parameters and body
        self.env.push_scope();
        // Bind parameters
        for (i, param) in func_def.params.iter().enumerate() {
            let val = if i < arg_values.len() {
                arg_values[i].clone()
            } else {
                Value::Unit
            };
            self.env.define_force(&param.name, val);
        }
        // C20B-015 (symmetric fix): body evaluation must NOT use `?`.
        // A `RuntimeError` from the body would skip the two pops below,
        // leaving the pushed instance-fields scope and local scope in
        // place. In REPL mode the interpreter is reused across inputs,
        // so the leak would let a subsequent input collide with a
        // method parameter / instance field at the top level. This
        // mirrors the Pattern B cleanup applied to the three
        // `call_function*` body-evaluation paths in eval.rs.
        //
        // Note: `eval_user_method` does not touch `type_defs` /
        // `enum_defs` overlays, `active_function`, or `call_depth`, so
        // the cleanup here is limited to the two scope pops. Success
        // path semantics are unchanged.
        let body_result = self.eval_statements(&func_def.body);
        self.env.pop_scope(); // pop local scope
        self.env.pop_scope(); // pop instance fields scope
        body_result
    }

    // ── Optional ABOLISHED (v0.8.0) — use Lax[T] instead ────

    // ── Result methods (operation mold with throw field) ────

    fn eval_result_method(
        &mut self,
        fields: &[(String, Value)],
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let inner_value = fields
            .iter()
            .find(|(n, _)| n == "__value")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);
        let throw_val = fields
            .iter()
            .find(|(n, _)| n == "throw")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);
        let predicate = fields
            .iter()
            .find(|(n, _)| n == "__predicate")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);

        // Determine success/failure:
        // 1. If throw is explicitly set (not Unit), it's an error
        // 2. If predicate exists, evaluate P(value) — true = success, false = error
        // 3. No predicate + no throw = success (backward compatible)
        let is_error = if throw_val != Value::Unit {
            true
        } else if let Value::Function(ref func) = predicate {
            let pred_result =
                self.call_function_with_values(func, std::slice::from_ref(&inner_value))?;
            !pred_result.is_truthy()
        } else {
            false
        };
        let is_success = !is_error;

        match method {
            "isSuccess" => Ok(Signal::Value(Value::Bool(is_success))),
            "isError" => Ok(Signal::Value(Value::Bool(is_error))),
            "getOrDefault" => {
                if is_success {
                    Ok(Signal::Value(inner_value))
                } else {
                    let default = args.first().cloned().unwrap_or(Value::Unit);
                    Ok(Signal::Value(default))
                }
            }
            "getOrThrow" => {
                if is_success {
                    Ok(Signal::Value(inner_value))
                } else if throw_val != Value::Unit {
                    Ok(Signal::Throw(throw_val))
                } else {
                    // Predicate failed but no explicit throw — generate default error
                    Ok(Signal::Throw(Value::Error(super::value::ErrorValue {
                        error_type: "ResultError".into(),
                        message: format!(
                            "Result predicate failed for value: {}",
                            inner_value.to_display_string()
                        ),
                        fields: Vec::new(),
                    })))
                }
            }
            "map" => {
                if is_error {
                    return Ok(Signal::Value(Value::BuchiPack(fields.to_vec())));
                }
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "Result.map requires a function argument".into(),
                        });
                    }
                };
                let result = self.call_function_with_values(&func, &[inner_value])?;
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__value".into(), result),
                    ("__predicate".into(), Value::Unit),
                    ("throw".into(), Value::Unit),
                    ("__type".into(), Value::Str("Result".into())),
                ])))
            }
            "flatMap" => {
                if is_error {
                    return Ok(Signal::Value(Value::BuchiPack(fields.to_vec())));
                }
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "Result.flatMap requires a function argument".into(),
                        });
                    }
                };
                let result = self.call_function_with_values(&func, &[inner_value])?;
                if let Value::BuchiPack(ref result_fields) = result {
                    let is_result = result_fields
                        .iter()
                        .any(|(n, v)| n == "__type" && v == &Value::Str("Result".into()));
                    if is_result {
                        return Ok(Signal::Value(result));
                    }
                }
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__value".into(), result),
                    ("__predicate".into(), Value::Unit),
                    ("throw".into(), Value::Unit),
                    ("__type".into(), Value::Str("Result".into())),
                ])))
            }
            "mapError" => {
                if is_success {
                    return Ok(Signal::Value(Value::BuchiPack(fields.to_vec())));
                }
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "Result.mapError requires a function argument".into(),
                        });
                    }
                };
                // Pass the error's display string to the mapping function
                let error_display = Self::throw_val_to_display_str(&throw_val);
                let result = self.call_function_with_values(&func, &[Value::Str(error_display)])?;
                let new_throw = Value::Error(super::value::ErrorValue {
                    error_type: "ResultError".into(),
                    message: result.to_display_string(),
                    fields: Vec::new(),
                });
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__value".into(), Value::Unit),
                    ("__predicate".into(), Value::Unit),
                    ("throw".into(), new_throw),
                    ("__type".into(), Value::Str("Result".into())),
                ])))
            }
            "toString" => {
                if is_success {
                    Ok(Signal::Value(Value::Str(format!(
                        "Result({})",
                        inner_value.to_display_string()
                    ))))
                } else {
                    let err_display = Self::throw_val_to_display_str(&throw_val);
                    Ok(Signal::Value(Value::Str(format!(
                        "Result(throw <= {})",
                        err_display
                    ))))
                }
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}' on Result", method),
            }),
        }
    }

    // ── Gorillax methods ─────────────────────────────────────

    fn eval_gorillax_method(
        &mut self,
        fields: &[(String, Value)],
        method: &str,
        _args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let has_value = fields
            .iter()
            .find(|(n, _)| n == "hasValue")
            .map(|(_, v)| v.is_truthy())
            .unwrap_or(false);
        let inner_value = fields
            .iter()
            .find(|(n, _)| n == "__value")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);
        let error_value = fields
            .iter()
            .find(|(n, _)| n == "__error")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);

        match method {
            "hasValue" => Ok(Signal::Value(Value::Bool(has_value))),
            "isEmpty" => Ok(Signal::Value(Value::Bool(!has_value))),
            "relax" => {
                // Convert to RelaxedGorillax — throwable instead of gorilla
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(has_value)),
                    ("__value".into(), inner_value),
                    ("__error".into(), error_value),
                    ("__type".into(), Value::Str("RelaxedGorillax".into())),
                ])))
            }
            "toString" => {
                if has_value {
                    Ok(Signal::Value(Value::Str(format!(
                        "Gorillax({})",
                        inner_value.to_display_string()
                    ))))
                } else {
                    Ok(Signal::Value(Value::Str("Gorillax(><)".to_string())))
                }
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}' on Gorillax", method),
            }),
        }
    }

    // ── RelaxedGorillax methods ──────────────────────────────

    fn eval_relaxed_gorillax_method(
        &mut self,
        fields: &[(String, Value)],
        method: &str,
        _args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let has_value = fields
            .iter()
            .find(|(n, _)| n == "hasValue")
            .map(|(_, v)| v.is_truthy())
            .unwrap_or(false);
        let inner_value = fields
            .iter()
            .find(|(n, _)| n == "__value")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);

        match method {
            "hasValue" => Ok(Signal::Value(Value::Bool(has_value))),
            "isEmpty" => Ok(Signal::Value(Value::Bool(!has_value))),
            "toString" => {
                if has_value {
                    Ok(Signal::Value(Value::Str(format!(
                        "RelaxedGorillax({})",
                        inner_value.to_display_string()
                    ))))
                } else {
                    Ok(Signal::Value(Value::Str(
                        "RelaxedGorillax(escaped)".to_string(),
                    )))
                }
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}' on RelaxedGorillax", method),
            }),
        }
    }

    // ── Lax methods ────────────────────────────────────────

    fn eval_lax_method(
        &mut self,
        fields: &[(String, Value)],
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let has_value = fields
            .iter()
            .find(|(n, _)| n == "hasValue")
            .map(|(_, v)| v.is_truthy())
            .unwrap_or(false);
        let inner_value = fields
            .iter()
            .find(|(n, _)| n == "__value")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);
        let default_value = fields
            .iter()
            .find(|(n, _)| n == "__default")
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Unit);

        match method {
            "hasValue" => Ok(Signal::Value(Value::Bool(has_value))),
            "isEmpty" => Ok(Signal::Value(Value::Bool(!has_value))),
            "getOrDefault" => {
                if has_value {
                    Ok(Signal::Value(inner_value))
                } else {
                    let custom_default = args.first().cloned().unwrap_or(default_value);
                    Ok(Signal::Value(custom_default))
                }
            }
            "map" => {
                if !has_value {
                    // Empty Lax stays empty with same default
                    return Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), default_value.clone()),
                        ("__default".into(), default_value),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])));
                }
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "Lax.map requires a function argument".into(),
                        });
                    }
                };
                let result = self.call_function_with_values(&func, &[inner_value])?;
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(true)),
                    ("__value".into(), result),
                    ("__default".into(), default_value),
                    ("__type".into(), Value::Str("Lax".into())),
                ])))
            }
            "flatMap" => {
                if !has_value {
                    return Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), default_value.clone()),
                        ("__default".into(), default_value),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])));
                }
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "Lax.flatMap requires a function argument".into(),
                        });
                    }
                };
                let result = self.call_function_with_values(&func, &[inner_value])?;
                // flatMap expects fn to return Lax
                if let Value::BuchiPack(ref result_fields) = result {
                    let is_lax = result_fields
                        .iter()
                        .any(|(n, v)| n == "__type" && v == &Value::Str("Lax".into()));
                    if is_lax {
                        return Ok(Signal::Value(result));
                    }
                }
                // Wrap non-Lax result in Lax with value
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(true)),
                    ("__value".into(), result),
                    ("__default".into(), default_value),
                    ("__type".into(), Value::Str("Lax".into())),
                ])))
            }
            "unmold" => {
                // Same as ]=> : hasValue → __value, otherwise → __default
                if has_value {
                    Ok(Signal::Value(inner_value))
                } else {
                    Ok(Signal::Value(default_value))
                }
            }
            "toString" => {
                if has_value {
                    Ok(Signal::Value(Value::Str(format!(
                        "Lax({})",
                        inner_value.to_display_string()
                    ))))
                } else {
                    Ok(Signal::Value(Value::Str(format!(
                        "Lax(default: {})",
                        default_value.to_display_string()
                    ))))
                }
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}' on Lax", method),
            }),
        }
    }

    // ── HashMap methods ────────────────────────────────────

    fn eval_hashmap_method(
        &mut self,
        fields: &[(String, Value)],
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let entries: Vec<Value> = fields
            .iter()
            .find(|(n, _)| n == "__entries")
            .and_then(|(_, v)| {
                if let Value::List(items) = v {
                    Some(items.as_ref().clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        match method {
            "get" => {
                let key = args.first().cloned().unwrap_or(Value::Str(String::new()));
                for entry in entries.iter() {
                    if let Value::BuchiPack(ef) = entry {
                        let entry_key = ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v);
                        if entry_key == Some(&key) {
                            let value = ef
                                .iter()
                                .find(|(n, _)| n == "value")
                                .map(|(_, v)| v.clone())
                                .unwrap_or(Value::Unit);
                            let default_value = Interpreter::default_for_value(&value);
                            return Ok(Signal::Value(Value::BuchiPack(vec![
                                ("hasValue".into(), Value::Bool(true)),
                                ("__value".into(), value),
                                ("__default".into(), default_value),
                                ("__type".into(), Value::Str("Lax".into())),
                            ])));
                        }
                    }
                }
                // Key not found — return empty Lax
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(false)),
                    ("__value".into(), Value::Unit),
                    ("__default".into(), Value::Unit),
                    ("__type".into(), Value::Str("Lax".into())),
                ])))
            }
            "set" => {
                let key = args.first().cloned().unwrap_or(Value::Str(String::new()));
                let value = args.get(1).cloned().unwrap_or(Value::Unit);
                let mut new_entries = Vec::new();
                let mut found = false;
                for entry in entries.iter() {
                    if let Value::BuchiPack(ef) = entry {
                        let entry_key = ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v);
                        if entry_key == Some(&key) {
                            new_entries.push(Value::BuchiPack(vec![
                                ("key".into(), key.clone()),
                                ("value".into(), value.clone()),
                            ]));
                            found = true;
                        } else {
                            new_entries.push(entry.clone());
                        }
                    }
                }
                if !found {
                    new_entries.push(Value::BuchiPack(vec![
                        ("key".into(), key),
                        ("value".into(), value),
                    ]));
                }
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__entries".into(), Value::list(new_entries)),
                    ("__type".into(), Value::Str("HashMap".into())),
                ])))
            }
            "remove" => {
                let key = args.first().cloned().unwrap_or(Value::Str(String::new()));
                let new_entries: Vec<Value> = entries
                    .into_iter()
                    .filter(|entry| {
                        if let Value::BuchiPack(ef) = entry {
                            let entry_key = ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v);
                            entry_key != Some(&key)
                        } else {
                            true
                        }
                    })
                    .collect();
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__entries".into(), Value::list(new_entries)),
                    ("__type".into(), Value::Str("HashMap".into())),
                ])))
            }
            "has" => {
                let key = args.first().cloned().unwrap_or(Value::Str(String::new()));
                let found = entries.iter().any(|entry| {
                    if let Value::BuchiPack(ef) = entry {
                        ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v) == Some(&key)
                    } else {
                        false
                    }
                });
                Ok(Signal::Value(Value::Bool(found)))
            }
            "keys" => {
                let keys: Vec<Value> = entries
                    .iter()
                    .filter_map(|entry| {
                        if let Value::BuchiPack(ef) = entry {
                            ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Signal::Value(Value::list(keys)))
            }
            "values" => {
                let values: Vec<Value> = entries
                    .iter()
                    .filter_map(|entry| {
                        if let Value::BuchiPack(ef) = entry {
                            ef.iter()
                                .find(|(n, _)| n == "value")
                                .map(|(_, v)| v.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Signal::Value(Value::list(values)))
            }
            "entries" => {
                let pairs: Vec<Value> = entries
                    .iter()
                    .filter_map(|entry| {
                        if let Value::BuchiPack(ef) = entry {
                            let key = ef
                                .iter()
                                .find(|(n, _)| n == "key")
                                .map(|(_, v)| v.clone())?;
                            let value = ef
                                .iter()
                                .find(|(n, _)| n == "value")
                                .map(|(_, v)| v.clone())?;
                            Some(Value::BuchiPack(vec![
                                ("key".into(), key),
                                ("value".into(), value),
                            ]))
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Signal::Value(Value::list(pairs)))
            }
            "size" => Ok(Signal::Value(Value::Int(entries.len() as i64))),
            "isEmpty" => Ok(Signal::Value(Value::Bool(entries.is_empty()))),
            "merge" => {
                let other = args.first().cloned().unwrap_or(Value::Unit);
                let other_entries: Vec<Value> = if let Value::BuchiPack(of) = &other {
                    of.iter()
                        .find(|(n, _)| n == "__entries")
                        .and_then(|(_, v)| {
                            if let Value::List(items) = v {
                                Some(items.as_ref().clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                // C25B-023 (Phase 5-E) fast path: pre-hash the set of
                // `other` keys. The original implementation walked the
                // full `merged` Vec on every other_entry and did a
                // BuchiPack field scan for "key" — O(N*M*K). With the
                // HashSet we can do a single pass over `merged`
                // retaining only entries whose key is *not* in
                // `other_keys`, then append all of `other_entries`.
                //
                // If any key is not hashable (Float, Function, etc.)
                // we fall back to the O(N*M) path to preserve the
                // original Value::eq semantics (which coerce Int↔Float↔
                // EnumVal).
                let extract_key = |entry: &Value| -> Option<Value> {
                    if let Value::BuchiPack(ef) = entry {
                        ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v.clone())
                    } else {
                        None
                    }
                };
                let other_keys: Vec<Value> = other_entries.iter().filter_map(extract_key).collect();
                let mut merged: Vec<Value> = entries;
                if let Some(other_key_fps) = try_build_fingerprint_set(&other_keys) {
                    merged.retain(|e| match extract_key(e) {
                        Some(k) => match ValueKey::new(&k) {
                            Some(vk) if other_key_fps.contains(&vk.fingerprint()) => {
                                // Confirm with Value::eq (guards against
                                // hypothetical fingerprint collision).
                                !other_keys.iter().any(|ok| ok == &k)
                            }
                            Some(_) => true,
                            None => !other_keys.iter().any(|ok| ok == &k),
                        },
                        None => true,
                    });
                    merged.extend(other_entries);
                } else {
                    for other_entry in other_entries {
                        if let Value::BuchiPack(ref oef) = other_entry {
                            let other_key = oef.iter().find(|(n, _)| n == "key").map(|(_, v)| v);
                            merged.retain(|e| {
                                if let Value::BuchiPack(ef) = e {
                                    ef.iter().find(|(n, _)| n == "key").map(|(_, v)| v) != other_key
                                } else {
                                    true
                                }
                            });
                        }
                        merged.push(other_entry);
                    }
                }
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__entries".into(), Value::list(merged)),
                    ("__type".into(), Value::Str("HashMap".into())),
                ])))
            }
            "toString" => {
                let pairs: Vec<String> = entries
                    .iter()
                    .filter_map(|entry| {
                        if let Value::BuchiPack(ef) = entry {
                            let key = ef
                                .iter()
                                .find(|(n, _)| n == "key")
                                .map(|(_, v)| v.to_debug_string())?;
                            let value = ef
                                .iter()
                                .find(|(n, _)| n == "value")
                                .map(|(_, v)| v.to_debug_string())?;
                            Some(format!("{}: {}", key, value))
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Signal::Value(Value::Str(format!(
                    "HashMap({{{}}})",
                    pairs.join(", ")
                ))))
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}' on HashMap", method),
            }),
        }
    }

    // ── Set methods ────────────────────────────────────

    fn eval_set_method(
        &self,
        fields: &[(String, Value)],
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let items: Vec<Value> = fields
            .iter()
            .find(|(n, _)| n == "__items")
            .and_then(|(_, v)| {
                if let Value::List(items) = v {
                    Some(items.as_ref().clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        match method {
            "add" => {
                let item = args.first().cloned().unwrap_or(Value::Unit);
                let mut new_items = items;
                if !new_items.contains(&item) {
                    new_items.push(item);
                }
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__items".into(), Value::list(new_items)),
                    ("__type".into(), Value::Str("Set".into())),
                ])))
            }
            "remove" => {
                let item = args.first().cloned().unwrap_or(Value::Unit);
                let new_items: Vec<Value> = items.into_iter().filter(|i| i != &item).collect();
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__items".into(), Value::list(new_items)),
                    ("__type".into(), Value::Str("Set".into())),
                ])))
            }
            "has" => {
                let item = args.first().cloned().unwrap_or(Value::Unit);
                Ok(Signal::Value(Value::Bool(items.contains(&item))))
            }
            "union" => {
                let other = args.first().cloned().unwrap_or(Value::Unit);
                let other_items: Vec<Value> = if let Value::BuchiPack(of) = &other {
                    of.iter()
                        .find(|(n, _)| n == "__items")
                        .and_then(|(_, v)| {
                            if let Value::List(items) = v {
                                Some(items.as_ref().clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                // C25B-022 (Phase 5-C) fast path: HashSet of existing
                // fingerprints. If both sides are fully hashable, each
                // `contains` probe becomes O(1), dropping the union
                // cost from O(N*M) to O(N+M). Mixed-Float / Function
                // operands fall back to the original `Vec::contains`.
                let mut result = items;
                if let Some(mut seen) = try_build_fingerprint_set(&result) {
                    for item in other_items {
                        if let Some(key) = ValueKey::new(&item) {
                            let fp = key.fingerprint();
                            if seen.insert(fp) {
                                // Confirm with Value::eq against any
                                // previously-inserted entry that hashed
                                // to the same bucket. For the key-
                                // eligible subset this coincides with
                                // ValueKey::eq, so a hit-check is only
                                // needed when an existing fingerprint
                                // is present (seen.insert already told
                                // us `false` in that case, so we skip).
                                result.push(item);
                            } else if !result.iter().any(|e| e == &item) {
                                // Extremely rare fingerprint collision:
                                // keep Value::eq authoritative. 64-bit
                                // hash; collision probability ≈ 0.
                                result.push(item);
                            }
                        } else if !result.contains(&item) {
                            // Item is not key-eligible (e.g. Float).
                            // Linear scan preserves Value::eq semantics.
                            result.push(item);
                        }
                    }
                } else {
                    for item in other_items {
                        if !result.contains(&item) {
                            result.push(item);
                        }
                    }
                }
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__items".into(), Value::list(result)),
                    ("__type".into(), Value::Str("Set".into())),
                ])))
            }
            "intersect" => {
                let other = args.first().cloned().unwrap_or(Value::Unit);
                let other_items: Vec<Value> = if let Value::BuchiPack(of) = &other {
                    of.iter()
                        .find(|(n, _)| n == "__items")
                        .and_then(|(_, v)| {
                            if let Value::List(items) = v {
                                Some(items.as_ref().clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                // C25B-022 (Phase 5-C) fast path: pre-hash `other_items`
                // and probe each `items` entry in O(1).
                let result: Vec<Value> =
                    if let Some(other_fps) = try_build_fingerprint_set(&other_items) {
                        items
                            .into_iter()
                            .filter(|item| match ValueKey::new(item) {
                                Some(k) if other_fps.contains(&k.fingerprint()) => {
                                    // Confirm via Value::eq to guard against
                                    // the (astronomically unlikely) 64-bit
                                    // fingerprint collision.
                                    other_items.iter().any(|o| o == item)
                                }
                                Some(_) => false,
                                None => other_items.contains(item),
                            })
                            .collect()
                    } else {
                        items
                            .into_iter()
                            .filter(|item| other_items.contains(item))
                            .collect()
                    };
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__items".into(), Value::list(result)),
                    ("__type".into(), Value::Str("Set".into())),
                ])))
            }
            "diff" => {
                let other = args.first().cloned().unwrap_or(Value::Unit);
                let other_items: Vec<Value> = if let Value::BuchiPack(of) = &other {
                    of.iter()
                        .find(|(n, _)| n == "__items")
                        .and_then(|(_, v)| {
                            if let Value::List(items) = v {
                                Some(items.as_ref().clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                // C25B-022 (Phase 5-C) fast path — symmetric to intersect.
                let result: Vec<Value> =
                    if let Some(other_fps) = try_build_fingerprint_set(&other_items) {
                        items
                            .into_iter()
                            .filter(|item| match ValueKey::new(item) {
                                Some(k) if other_fps.contains(&k.fingerprint()) => {
                                    !other_items.iter().any(|o| o == item)
                                }
                                Some(_) => true,
                                None => !other_items.contains(item),
                            })
                            .collect()
                    } else {
                        items
                            .into_iter()
                            .filter(|item| !other_items.contains(item))
                            .collect()
                    };
                Ok(Signal::Value(Value::BuchiPack(vec![
                    ("__items".into(), Value::list(result)),
                    ("__type".into(), Value::Str("Set".into())),
                ])))
            }
            "toList" => Ok(Signal::Value(Value::list(items))),
            "size" => Ok(Signal::Value(Value::Int(items.len() as i64))),
            "isEmpty" => Ok(Signal::Value(Value::Bool(items.is_empty()))),
            "toString" => {
                let strs: Vec<String> = items.iter().map(|i| i.to_debug_string()).collect();
                Ok(Signal::Value(Value::Str(format!(
                    "Set({{{}}})",
                    strs.join(", ")
                ))))
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}' on Set", method),
            }),
        }
    }

    /// Bytes methods.
    pub(crate) fn eval_bytes_method(
        &self,
        bytes: &[u8],
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        match method {
            "length" => Ok(Signal::Value(Value::Int(bytes.len() as i64))),
            "get" => {
                let idx = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => -1,
                };
                if idx >= 0 && (idx as usize) < bytes.len() {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), Value::Int(bytes[idx as usize] as i64)),
                        ("__default".into(), Value::Int(0)),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Int(0)),
                        ("__default".into(), Value::Int(0)),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            "toString" => Ok(Signal::Value(Value::Str(
                Value::Bytes(bytes.to_vec()).to_string(),
            ))),
            _ => Err(RuntimeError {
                message: format!(
                    "Unknown bytes method: '{}'. Supported: length(), get()",
                    method
                ),
            }),
        }
    }

    /// String methods (auto-mold Str).
    /// Only state-check methods remain. Operations moved to molds:
    /// Upper[], Lower[], Trim[], Split[], Replace[], Slice[], CharAt[], Repeat[], Reverse[], Pad[]
    pub(crate) fn eval_str_method(
        &self,
        s: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        match method {
            // State checks (remain as methods)
            "length" => Ok(Signal::Value(Value::Int(s.chars().count() as i64))),
            "contains" => {
                let substr = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                Ok(Signal::Value(Value::Bool(s.contains(&substr))))
            }
            "startsWith" => {
                let prefix = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                Ok(Signal::Value(Value::Bool(s.starts_with(&prefix))))
            }
            "endsWith" => {
                let suffix = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                Ok(Signal::Value(Value::Bool(s.ends_with(&suffix))))
            }
            "indexOf" => {
                let substr = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                // Convert byte offset to character offset for consistency with length()/get()
                let idx = s
                    .find(&substr)
                    .map(|byte_pos| s[..byte_pos].chars().count() as i64)
                    .unwrap_or(-1);
                Ok(Signal::Value(Value::Int(idx)))
            }
            "lastIndexOf" => {
                let substr = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                // Convert byte offset to character offset for consistency with length()/get()
                let idx = s
                    .rfind(&substr)
                    .map(|byte_pos| s[..byte_pos].chars().count() as i64)
                    .unwrap_or(-1);
                Ok(Signal::Value(Value::Int(idx)))
            }
            // Safe access (returns Lax)
            "get" => {
                let idx = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
                let char_len = s.chars().count();
                if idx >= 0 && (idx as usize) < char_len {
                    // SAFETY: bounds checked above — idx is in [0, char_len)
                    let ch = s
                        .chars()
                        .nth(idx as usize)
                        .expect("bounds checked above")
                        .to_string();
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), Value::Str(ch)),
                        ("__default".into(), Value::Str(String::new())),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Str(String::new())),
                        ("__default".into(), Value::Str(String::new())),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            // Display
            "toString" => Ok(Signal::Value(Value::Str(s.to_string()))),
            // B11-4b: replace / replaceAll / split methods
            // C12-6c: Regex overload — if the first arg is a Regex
            // BuchiPack, dispatch to the regex engine; otherwise fall
            // back to the B11-4a fixed-string semantics.
            "replace" => {
                if let Some((pat, flags)) = args.first().and_then(super::regex_eval::as_regex) {
                    let replacement = args
                        .get(1)
                        .map(|v| v.to_display_string())
                        .unwrap_or_default();
                    match super::regex_eval::replace_first(s, &pat, &flags, &replacement) {
                        Ok(out) => return Ok(Signal::Value(Value::Str(out))),
                        Err(msg) => return Err(RuntimeError { message: msg }),
                    }
                }
                let target = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                let replacement = args
                    .get(1)
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                // Empty target → no-op (B11-4a edge semantics lock)
                if target.is_empty() {
                    return Ok(Signal::Value(Value::Str(s.to_string())));
                }
                Ok(Signal::Value(Value::Str(s.replacen(
                    &target,
                    &replacement,
                    1,
                ))))
            }
            "replaceAll" => {
                if let Some((pat, flags)) = args.first().and_then(super::regex_eval::as_regex) {
                    let replacement = args
                        .get(1)
                        .map(|v| v.to_display_string())
                        .unwrap_or_default();
                    match super::regex_eval::replace_all(s, &pat, &flags, &replacement) {
                        Ok(out) => return Ok(Signal::Value(Value::Str(out))),
                        Err(msg) => return Err(RuntimeError { message: msg }),
                    }
                }
                let target = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                let replacement = args
                    .get(1)
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                // Empty target → no-op (B11-4a edge semantics lock)
                if target.is_empty() {
                    return Ok(Signal::Value(Value::Str(s.to_string())));
                }
                Ok(Signal::Value(Value::Str(s.replace(&target, &replacement))))
            }
            "split" => {
                if let Some((pat, flags)) = args.first().and_then(super::regex_eval::as_regex) {
                    match super::regex_eval::split(s, &pat, &flags) {
                        Ok(parts) => {
                            return Ok(Signal::Value(Value::list(
                                parts.into_iter().map(Value::Str).collect(),
                            )));
                        }
                        Err(msg) => return Err(RuntimeError { message: msg }),
                    }
                }
                let separator = args
                    .first()
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                let parts: Vec<Value> = if separator.is_empty() {
                    // B11-4a: split("") → chars split (like Chars[] mold)
                    // "".split("") → empty list (matches JS/Native)
                    if s.is_empty() {
                        vec![]
                    } else {
                        s.chars().map(|ch| Value::Str(ch.to_string())).collect()
                    }
                } else {
                    s.split(&separator)
                        .map(|p| Value::Str(p.to_string()))
                        .collect()
                };
                Ok(Signal::Value(Value::list(parts)))
            }
            // C12-6c: match / search methods — Regex arg required.
            // Surface a RuntimeError if the first arg isn't a Regex
            // BuchiPack (philosophy: explicit, no silent coercion).
            "match" => {
                let (pat, flags) = args
                    .first()
                    .and_then(super::regex_eval::as_regex)
                    .ok_or_else(|| RuntimeError {
                        message: "str.match(...) requires a Regex argument. Use Regex(\"pattern\") to construct one.".to_string(),
                    })?;
                match super::regex_eval::match_first(s, &pat, &flags) {
                    Ok(m) => Ok(Signal::Value(super::regex_eval::build_match_value(m))),
                    Err(msg) => Err(RuntimeError { message: msg }),
                }
            }
            "search" => {
                let (pat, flags) = args
                    .first()
                    .and_then(super::regex_eval::as_regex)
                    .ok_or_else(|| RuntimeError {
                        message: "str.search(...) requires a Regex argument. Use Regex(\"pattern\") to construct one.".to_string(),
                    })?;
                match super::regex_eval::search_first(s, &pat, &flags) {
                    Ok(idx) => Ok(Signal::Value(Value::Int(idx))),
                    Err(msg) => Err(RuntimeError { message: msg }),
                }
            }
            _ => Err(RuntimeError {
                message: format!(
                    "Unknown string method: '{}'. Available: length, contains, startsWith, endsWith, indexOf, lastIndexOf, get, replace, replaceAll, split, match, search, toString",
                    method
                ),
            }),
        }
    }

    /// Number methods (auto-mold Num).
    /// Only state-check methods remain. Operations moved to molds:
    /// ToFixed[], Abs[], Floor[], Ceil[], Round[], Truncate[], Clamp[]
    pub(crate) fn eval_num_method(
        &self,
        val: &Value,
        method: &str,
        _args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        let float_val = match val {
            Value::Int(n) => *n as f64,
            Value::Float(n) => *n,
            _ => {
                return Err(RuntimeError {
                    message: format!("Expected number for method '{}', got {}", method, val),
                });
            }
        };

        match method {
            // Display
            "toString" => Ok(Signal::Value(Value::Str(val.to_display_string()))),
            // State checks (remain as methods)
            "isNaN" => Ok(Signal::Value(Value::Bool(float_val.is_nan()))),
            "isInfinite" => Ok(Signal::Value(Value::Bool(float_val.is_infinite()))),
            "isFinite" => Ok(Signal::Value(Value::Bool(float_val.is_finite()))),
            "isPositive" => Ok(Signal::Value(Value::Bool(float_val > 0.0))),
            "isNegative" => Ok(Signal::Value(Value::Bool(float_val < 0.0))),
            "isZero" => Ok(Signal::Value(Value::Bool(float_val == 0.0))),
            _ => Err(RuntimeError {
                message: format!(
                    "Unknown number method: '{}'. Operations moved to molds: ToFixed[], Abs[], Floor[], Ceil[], Round[], Truncate[], Clamp[]",
                    method
                ),
            }),
        }
    }

    /// List methods (auto-mold List).
    /// Only state-check methods remain. Operations moved to molds:
    /// Reverse[], Concat[], Append[], Prepend[], Join[], Sum[], Sort[], Unique[],
    /// Flatten[], Find[], FindIndex[], Count[], Take[], Drop[], TakeWhile[], DropWhile[],
    /// Zip[], Enumerate[], Filter[], Map[], Fold[], Foldr[]
    pub(crate) fn eval_list_method(
        &mut self,
        items: &[Value],
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        match method {
            // State checks (remain as methods)
            "length" => Ok(Signal::Value(Value::Int(items.len() as i64))),
            "isEmpty" => Ok(Signal::Value(Value::Bool(items.is_empty()))),
            "first" => {
                if let Some(val) = items.first() {
                    let default_val = super::eval::Interpreter::default_for_value(val);
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), val.clone()),
                        ("__default".into(), default_val),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Int(0)),
                        ("__default".into(), Value::Int(0)),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            "last" => {
                if let Some(val) = items.last() {
                    let default_val = super::eval::Interpreter::default_for_value(val);
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), val.clone()),
                        ("__default".into(), default_val),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Int(0)),
                        ("__default".into(), Value::Int(0)),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            "get" => {
                let idx = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
                let custom_default = args.get(1).cloned();
                if idx >= 0 && (idx as usize) < items.len() {
                    let val = items[idx as usize].clone();
                    let default_val = custom_default
                        .unwrap_or_else(|| super::eval::Interpreter::default_for_value(&val));
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), val),
                        ("__default".into(), default_val),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    let default_val = custom_default.unwrap_or_else(|| {
                        if let Some(first) = items.first() {
                            super::eval::Interpreter::default_for_value(first)
                        } else {
                            Value::Int(0)
                        }
                    });
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), default_val.clone()),
                        ("__default".into(), default_val),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            "contains" => {
                let target = args.first().cloned().unwrap_or(Value::Unit);
                Ok(Signal::Value(Value::Bool(items.contains(&target))))
            }
            "indexOf" => {
                let target = args.first().cloned().unwrap_or(Value::Unit);
                let idx = items.iter().position(|v| v == &target);
                Ok(Signal::Value(Value::Int(
                    idx.map(|i| i as i64).unwrap_or(-1),
                )))
            }
            "lastIndexOf" => {
                let target = args.first().cloned().unwrap_or(Value::Unit);
                let idx = items.iter().rposition(|v| v == &target);
                Ok(Signal::Value(Value::Int(
                    idx.map(|i| i as i64).unwrap_or(-1),
                )))
            }
            "max" => {
                if items.is_empty() {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Int(0)),
                        ("__default".into(), Value::Int(0)),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    let max_val = items
                        .iter()
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .cloned()
                        .unwrap_or_else(|| Value::default_for_list(items));
                    let default_val = super::eval::Interpreter::default_for_value(&max_val);
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), max_val),
                        ("__default".into(), default_val),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            "min" => {
                if items.is_empty() {
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(false)),
                        ("__value".into(), Value::Int(0)),
                        ("__default".into(), Value::Int(0)),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                } else {
                    let min_val = items
                        .iter()
                        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .cloned()
                        .unwrap_or_else(|| Value::default_for_list(items));
                    let default_val = super::eval::Interpreter::default_for_value(&min_val);
                    Ok(Signal::Value(Value::BuchiPack(vec![
                        ("hasValue".into(), Value::Bool(true)),
                        ("__value".into(), min_val),
                        ("__default".into(), default_val),
                        ("__type".into(), Value::Str("Lax".into())),
                    ])))
                }
            }
            // Predicate checks (remain as methods)
            "any" => {
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "any() requires a function argument".to_string(),
                        });
                    }
                };
                for item in items {
                    let result =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if result == Value::Bool(true) {
                        return Ok(Signal::Value(Value::Bool(true)));
                    }
                }
                Ok(Signal::Value(Value::Bool(false)))
            }
            "all" => {
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "all() requires a function argument".to_string(),
                        });
                    }
                };
                for item in items {
                    let result =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if result != Value::Bool(true) {
                        return Ok(Signal::Value(Value::Bool(false)));
                    }
                }
                Ok(Signal::Value(Value::Bool(true)))
            }
            "none" => {
                let func = match args.first() {
                    Some(Value::Function(f)) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: "none() requires a function argument".to_string(),
                        });
                    }
                };
                for item in items {
                    let result =
                        self.call_function_with_values(&func, std::slice::from_ref(item))?;
                    if result == Value::Bool(true) {
                        return Ok(Signal::Value(Value::Bool(false)));
                    }
                }
                Ok(Signal::Value(Value::Bool(true)))
            }
            // Display (C12-2b: universal .toString() adoption)
            "toString" => Ok(Signal::Value(Value::Str(
                Value::list(items.to_vec()).to_display_string(),
            ))),
            _ => Err(RuntimeError {
                message: format!(
                    "Unknown list method: '{}'. Operations moved to molds: Reverse[], Concat[], Append[], Prepend[], Join[], Sum[], Sort[], Unique[], Flatten[], Find[], FindIndex[], Count[]",
                    method
                ),
            }),
        }
    }

    /// Bool methods (auto-mold Bool).
    fn eval_bool_method(
        &self,
        val: bool,
        method: &str,
        _args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        match method {
            "toString" => Ok(Signal::Value(Value::Str(val.to_string()))),
            _ => Err(RuntimeError {
                message: format!(
                    "Unknown bool method: '{}'. Use Int[bool]() for conversion",
                    method
                ),
            }),
        }
    }

    /// Stream methods (Stream[T] mold type).
    fn eval_stream_method(
        &mut self,
        stream_val: &StreamValue,
        method: &str,
        _args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        match method {
            "length" => {
                // For completed streams, return item count.
                // For active streams, return -1 (unknown).
                let len = match stream_val.status {
                    StreamStatus::Completed => stream_val.items.len() as i64,
                    StreamStatus::Active => -1,
                };
                Ok(Signal::Value(Value::Int(len)))
            }
            "isEmpty" => Ok(Signal::Value(Value::Bool(
                stream_val.items.is_empty() && stream_val.status == StreamStatus::Completed,
            ))),
            "toString" => Ok(Signal::Value(Value::Str(
                Value::Stream(stream_val.clone()).to_display_string(),
            ))),
            _ => Err(RuntimeError {
                message: format!("Unknown Stream method: '{}'", method),
            }),
        }
    }

    /// Async methods (Async[T] mold type).
    fn eval_async_method(
        &mut self,
        async_val: &AsyncValue,
        method: &str,
        args: &[Value],
    ) -> Result<Signal, RuntimeError> {
        match method {
            "isPending" => Ok(Signal::Value(Value::Bool(
                async_val.status == AsyncStatus::Pending,
            ))),
            "isFulfilled" => Ok(Signal::Value(Value::Bool(
                async_val.status == AsyncStatus::Fulfilled,
            ))),
            "isRejected" => Ok(Signal::Value(Value::Bool(
                async_val.status == AsyncStatus::Rejected,
            ))),
            "unmold" => {
                // Blocking await — extract the inner value
                match async_val.status {
                    AsyncStatus::Fulfilled => Ok(Signal::Value((*async_val.value).clone())),
                    AsyncStatus::Rejected => {
                        // Rejected async throws the error
                        Ok(Signal::Throw((*async_val.error).clone()))
                    }
                    AsyncStatus::Pending => {
                        // In synchronous mode, pending returns Unit
                        Ok(Signal::Value(Value::Unit))
                    }
                }
            }
            "map" => {
                // map(fn) -> apply fn to the inner value if fulfilled
                if args.is_empty() {
                    return Err(RuntimeError {
                        message: "Async.map requires 1 argument (function)".to_string(),
                    });
                }
                let func = match &args[0] {
                    Value::Function(f) => f.clone(),
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "Async.map: argument must be a function, got {}",
                                args[0]
                            ),
                        });
                    }
                };
                match async_val.status {
                    AsyncStatus::Rejected => {
                        // Rejected: propagate rejection unchanged
                        Ok(Signal::Value(Value::Async(async_val.clone())))
                    }
                    AsyncStatus::Pending => {
                        // Pending: return pending unchanged
                        Ok(Signal::Value(Value::Async(async_val.clone())))
                    }
                    AsyncStatus::Fulfilled => {
                        // Apply the function to the inner value
                        let mapped = self.call_function_with_values(
                            &func,
                            std::slice::from_ref(async_val.value.as_ref()),
                        )?;
                        Ok(Signal::Value(Value::Async(AsyncValue {
                            status: AsyncStatus::Fulfilled,
                            value: Box::new(mapped),
                            error: Box::new(Value::Unit),
                            task: None,
                        })))
                    }
                }
            }
            "getOrDefault" => {
                // getOrDefault(default) -> return inner value if fulfilled, otherwise default
                if args.is_empty() {
                    return Err(RuntimeError {
                        message: "Async.getOrDefault requires 1 argument (default value)"
                            .to_string(),
                    });
                }
                match async_val.status {
                    AsyncStatus::Fulfilled => Ok(Signal::Value((*async_val.value).clone())),
                    _ => Ok(Signal::Value(args[0].clone())),
                }
            }
            "toString" => Ok(Signal::Value(Value::Str(
                Value::Async(async_val.clone()).to_display_string(),
            ))),
            _ => Err(RuntimeError {
                message: format!("Unknown Async method: '{}'", method),
            }),
        }
    }
}
