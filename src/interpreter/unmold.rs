use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, ErrorValue, PendingState, StreamTransform, Value};
/// Unmolding and default-value helpers for the Taida interpreter.
///
/// Contains `default_for_value`, `unmold_value`, `resolve_async`, and `eval_mold_option`.
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use crate::parser::BuchiField;

impl Interpreter {
    // ── Default Value Helper ─────────────────────────────────

    /// Get the default value for a given value's type.
    /// Used by Lax[T] to determine the fallback when hasValue is false.
    pub(crate) fn default_for_value(val: &Value) -> Value {
        match val {
            Value::Int(_) => Value::Int(0),
            Value::Float(_) => Value::Float(0.0),
            Value::Str(_) => Value::Str(String::new()),
            Value::Bytes(_) => Value::Bytes(Vec::new()),
            Value::Bool(_) => Value::Bool(false),
            Value::List(_) => Value::list(Vec::new()),
            Value::BuchiPack(_) => Value::BuchiPack(Vec::new()),
            Value::Json(_) => Value::Json(serde_json::Value::Object(serde_json::Map::new())),
            Value::Stream(_) => Value::default_stream(),
            Value::Unit => Value::Unit,
            _ => Value::Unit,
        }
    }

    // ── Async Resolution Helpers ─────────────────────────────

    /// Resolve a pending async value by blocking on its tokio task.
    /// If the async is already resolved (Fulfilled/Rejected), returns as-is.
    /// If pending with a task, uses tokio runtime to block_on the receiver.
    pub(crate) fn resolve_async(&self, a: &AsyncValue) -> Result<AsyncValue, RuntimeError> {
        // Already resolved or no task — return immediately
        let task_arc = match (a.status == AsyncStatus::Pending, a.task.as_ref()) {
            (true, Some(arc)) => arc.clone(),
            _ => return Ok(a.clone()),
        };

        // Pending with a task — block on the receiver
        let mut guard = task_arc.lock().map_err(|e| RuntimeError {
            message: format!("Async task lock poisoned: {}", e),
        })?;

        match std::mem::replace(&mut *guard, PendingState::Done) {
            PendingState::Waiting(receiver) => {
                drop(guard); // Release lock before blocking
                let result = self.tokio_runtime.block_on(async {
                    receiver
                        .await
                        .map_err(|_| "Async task channel closed".to_string())
                });
                match result {
                    Ok(Ok(value)) => Ok(AsyncValue {
                        status: AsyncStatus::Fulfilled,
                        value: Box::new(value),
                        error: Box::new(Value::Unit),
                        task: None,
                    }),
                    Ok(Err(err_msg)) => Ok(AsyncValue {
                        status: AsyncStatus::Rejected,
                        value: Box::new(Value::Unit),
                        error: Box::new(Value::Error(ErrorValue {
                            error_type: "AsyncError".into(),
                            message: err_msg,
                            fields: Vec::new(),
                        })),
                        task: None,
                    }),
                    Err(err_msg) => Ok(AsyncValue {
                        status: AsyncStatus::Rejected,
                        value: Box::new(Value::Unit),
                        error: Box::new(Value::Error(ErrorValue {
                            error_type: "AsyncError".into(),
                            message: err_msg,
                            fields: Vec::new(),
                        })),
                        task: None,
                    }),
                }
            }
            PendingState::Done => {
                // Already consumed — return Unit (should not happen in normal use)
                Ok(AsyncValue {
                    status: AsyncStatus::Fulfilled,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
                    task: None,
                })
            }
        }
    }

    /// Resolve a pending async value with a timeout (in milliseconds).
    /// Returns Some(resolved) if completed within timeout, None if timed out.
    pub(crate) fn resolve_async_with_timeout(
        &self,
        a: &AsyncValue,
        timeout_ms: u64,
    ) -> Result<Option<AsyncValue>, RuntimeError> {
        // Already resolved or no task — return immediately
        let task_arc = match (a.status == AsyncStatus::Pending, a.task.as_ref()) {
            (true, Some(arc)) => arc.clone(),
            _ => return Ok(Some(a.clone())),
        };
        let mut guard = task_arc.lock().map_err(|e| RuntimeError {
            message: format!("Async task lock poisoned: {}", e),
        })?;

        match std::mem::replace(&mut *guard, PendingState::Done) {
            PendingState::Waiting(receiver) => {
                drop(guard);
                let duration = std::time::Duration::from_millis(timeout_ms);

                self.tokio_runtime.block_on(async {
                    match tokio::time::timeout(duration, receiver).await {
                        Ok(Ok(Ok(value))) => Ok(Some(AsyncValue {
                            status: AsyncStatus::Fulfilled,
                            value: Box::new(value),
                            error: Box::new(Value::Unit),
                            task: None,
                        })),
                        Ok(Ok(Err(err_msg))) => Ok(Some(AsyncValue {
                            status: AsyncStatus::Rejected,
                            value: Box::new(Value::Unit),
                            error: Box::new(Value::Error(ErrorValue {
                                error_type: "AsyncError".into(),
                                message: err_msg,
                                fields: Vec::new(),
                            })),
                            task: None,
                        })),
                        Ok(Err(_)) => Ok(Some(AsyncValue {
                            status: AsyncStatus::Rejected,
                            value: Box::new(Value::Unit),
                            error: Box::new(Value::Error(ErrorValue {
                                error_type: "AsyncError".into(),
                                message: "Async task channel closed".into(),
                                fields: Vec::new(),
                            })),
                            task: None,
                        })),
                        Err(_) => Ok(None), // Timeout elapsed
                    }
                })
            }
            PendingState::Done => Ok(Some(AsyncValue {
                status: AsyncStatus::Fulfilled,
                value: Box::new(Value::Unit),
                error: Box::new(Value::Unit),
                task: None,
            })),
        }
    }

    // ── Unmold Helper ────────────────────────────────────────

    /// Unmold a value: extract the inner value from a Mold wrapper.
    /// For Async values, this is a blocking await (via tokio runtime).
    /// For rejected Async, this returns a Throw signal.
    /// Returns Signal so that rejected Async can be caught by error ceiling.
    pub(crate) fn unmold_value(&mut self, val: Value) -> Result<Signal, RuntimeError> {
        match val {
            Value::Async(a) => {
                // If pending with a task, resolve it first via tokio
                if a.status == AsyncStatus::Pending && a.task.is_some() {
                    let resolved = self.resolve_async(&a)?;
                    return match resolved.status {
                        AsyncStatus::Fulfilled => Ok(Signal::Value(*resolved.value)),
                        AsyncStatus::Rejected => Ok(Signal::Throw(*resolved.error)),
                        AsyncStatus::Pending => Ok(Signal::Value(Value::Unit)),
                    };
                }
                // Already resolved or no task
                match a.status {
                    AsyncStatus::Fulfilled => Ok(Signal::Value(*a.value)),
                    AsyncStatus::Rejected => {
                        // Rejected async: throw the error so it can be caught by |==
                        Ok(Signal::Throw(*a.error))
                    }
                    AsyncStatus::Pending => {
                        // Pending without a task (legacy mode): treated as Unit
                        Ok(Signal::Value(Value::Unit))
                    }
                }
            }
            Value::Stream(s) => {
                // Stream unmold: collect all items, applying lazy transforms
                // Returns Value::List (the collected items)
                let mut items = s.items.clone();
                for transform in &s.transforms {
                    match transform {
                        StreamTransform::Map(func) => {
                            let mut mapped = Vec::new();
                            for item in &items {
                                let result = self
                                    .call_function_with_values(func, std::slice::from_ref(item))?;
                                mapped.push(result);
                            }
                            items = mapped;
                        }
                        StreamTransform::Filter(func) => {
                            let mut filtered = Vec::new();
                            for item in &items {
                                let keep = self
                                    .call_function_with_values(func, std::slice::from_ref(item))?;
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
                                let keep = self
                                    .call_function_with_values(func, std::slice::from_ref(item))?;
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
                Ok(Signal::Value(Value::list(items)))
            }
            Value::Json(_) => {
                // JSON is opaque (Molten Iron) — cannot unmold without schema
                Err(RuntimeError {
                    message: "Cannot unmold JSON directly. Use JSON[raw, Schema]() to cast through a schema first.".to_string(),
                })
            }
            Value::Molten => {
                // Molten is opaque — cannot unmold directly
                Err(RuntimeError {
                    message: "Cannot unmold Molten directly. Molten can only be used inside Cage."
                        .to_string(),
                })
            }
            // BuchiPack with __type field (Mold type): extract the inner value
            Value::BuchiPack(ref fields) => {
                let type_name = fields
                    .iter()
                    .find(|(k, _)| k == "__type")
                    .and_then(|(_, v)| {
                        if let Value::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    });

                // TODO[T] unmold: `]=>` returns the `unm` channel when present,
                // otherwise falls back to the default for the `sol` type.
                if type_name == Some("TODO") {
                    if let Some((_, unm_val)) =
                        fields.iter().find(|(k, _)| k == "unm" || k == "__default")
                    {
                        return Ok(Signal::Value(unm_val.clone()));
                    }
                    let sol = fields
                        .iter()
                        .find(|(k, _)| k == "sol" || k == "__value")
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit);
                    return Ok(Signal::Value(Self::default_for_value(&sol)));
                }

                // Gorillax: hasValue==true → __value, hasValue==false → GORILLA (program terminates)
                if type_name == Some("Gorillax") {
                    let has_value = fields
                        .iter()
                        .find(|(k, _)| k == "hasValue")
                        .map(|(_, v)| v.is_truthy())
                        .unwrap_or(false);
                    if has_value {
                        let inner = fields
                            .iter()
                            .find(|(k, _)| k == "__value")
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        return Ok(Signal::Value(inner));
                    } else {
                        // Gorilla! Program terminates
                        return Ok(Signal::Gorilla);
                    }
                }

                // RelaxedGorillax: hasValue==true → __value, hasValue==false → throw RelaxedGorillaEscaped
                if type_name == Some("RelaxedGorillax") {
                    let has_value = fields
                        .iter()
                        .find(|(k, _)| k == "hasValue")
                        .map(|(_, v)| v.is_truthy())
                        .unwrap_or(false);
                    if has_value {
                        let inner = fields
                            .iter()
                            .find(|(k, _)| k == "__value")
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        return Ok(Signal::Value(inner));
                    } else {
                        let error = fields
                            .iter()
                            .find(|(k, _)| k == "__error")
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        let throw_error = if let Value::Error(_) = &error {
                            error
                        } else {
                            Value::Error(super::value::ErrorValue {
                                error_type: "RelaxedGorillaEscaped".into(),
                                message: format!(
                                    "Relaxed gorilla escaped: {}",
                                    error.to_display_string()
                                ),
                                fields: Vec::new(),
                            })
                        };
                        return Ok(Signal::Throw(throw_error));
                    }
                }

                // Lax: hasValue==true → __value, hasValue==false → __default
                if type_name == Some("Lax") {
                    let has_value = fields
                        .iter()
                        .find(|(k, _)| k == "hasValue")
                        .map(|(_, v)| v.is_truthy())
                        .unwrap_or(false);
                    if has_value {
                        let inner = fields
                            .iter()
                            .find(|(k, _)| k == "__value")
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        return Ok(Signal::Value(inner));
                    } else {
                        let default = fields
                            .iter()
                            .find(|(k, _)| k == "__default")
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        return Ok(Signal::Value(default));
                    }
                }

                // Result: predicate evaluation on unmold (]=>)
                // 1. If throw field is already set (not Unit), throw immediately
                // 2. If __predicate is present, evaluate P(value):
                //    - true  → return value T
                //    - false → throw the error from throw field (or default ResultError)
                // 3. No predicate (Unit) → backward compatible: return value
                if type_name == Some("Result") {
                    let throw_val = fields
                        .iter()
                        .find(|(k, _)| k == "throw")
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit);
                    let inner = fields
                        .iter()
                        .find(|(k, _)| k == "__value")
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit);
                    let predicate = fields
                        .iter()
                        .find(|(k, _)| k == "__predicate")
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit);

                    // If throw is already set explicitly, throw immediately
                    if throw_val != Value::Unit {
                        // If predicate exists, evaluate it first
                        if let Value::Function(ref func) = predicate {
                            let pred_result =
                                self.call_function_with_values(func, std::slice::from_ref(&inner))?;
                            if !pred_result.is_truthy() {
                                return Ok(Signal::Throw(throw_val));
                            }
                            // Predicate passed even though throw was set — return value
                            return Ok(Signal::Value(inner));
                        }
                        // No predicate, throw is set — throw the error
                        return Ok(Signal::Throw(throw_val));
                    }

                    // Evaluate predicate if present
                    if let Value::Function(ref func) = predicate {
                        let pred_result =
                            self.call_function_with_values(func, std::slice::from_ref(&inner))?;
                        if pred_result.is_truthy() {
                            return Ok(Signal::Value(inner));
                        } else {
                            // Predicate failed — throw the error (or default ResultError)
                            let error_to_throw = if throw_val != Value::Unit {
                                throw_val
                            } else {
                                Value::Error(super::value::ErrorValue {
                                    error_type: "ResultError".into(),
                                    message: format!(
                                        "Result predicate failed for value: {}",
                                        inner.to_display_string()
                                    ),
                                    fields: Vec::new(),
                                })
                            };
                            return Ok(Signal::Throw(error_to_throw));
                        }
                    }

                    // No predicate (backward compatible) — success: return value
                    return Ok(Signal::Value(inner));
                }

                // Custom mold: check for __unmold function (defined via `unmold _ = ...`)
                // Use call_function_preserving_signals so that throw inside
                // __unmold is propagated as Signal::Throw (catchable by |==),
                // not converted to a fatal RuntimeError.
                if let Some((_, Value::Function(unmold_func))) =
                    fields.iter().find(|(k, _)| k == "__unmold")
                {
                    let unmold_func = unmold_func.clone();
                    return self.call_function_preserving_signals(&unmold_func, &[]);
                }

                if let Some((_, inner)) = fields.iter().find(|(k, _)| k == "__value") {
                    Ok(Signal::Value(inner.clone()))
                } else {
                    // No __value field: pass through unchanged
                    Ok(Signal::Value(val))
                }
            }
            // Non-mold values pass through unchanged
            other => Ok(Signal::Value(other)),
        }
    }

    // ── Operation Mold Types (method→mold refactoring) ─────────

    /// Helper: evaluate a BuchiField option, returning the value or a default.
    pub(crate) fn eval_mold_option(
        &mut self,
        fields: &[BuchiField],
        name: &str,
    ) -> Result<Option<Value>, RuntimeError> {
        if let Some(f) = fields.iter().find(|f| f.name == name) {
            match self.eval_expr(&f.value)? {
                Signal::Value(v) => Ok(Some(v)),
                Signal::Throw(err) => Err(RuntimeError {
                    message: format!("Error in mold option '{}': {}", name, err),
                }),
                _other => Ok(None), // Gorilla/TailCall — safe to ignore in mold option context
            }
        } else {
            Ok(None)
        }
    }
}
