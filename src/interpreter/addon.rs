//! Interpreter dispatch for addon-backed function calls.
//!
//! This module enforces two contracts:
//!
//! 1. **No raw pointer in the user surface**: the dispatcher receives
//! Taida `Value` arguments, calls
//! `LoadedAddon::call_function(&[Value]) -> Result<Value, AddonCallError>`,
//! and returns either a Taida `Value` or an error.
//! `*mut TaidaAddon*` types live entirely inside `src/addon`.
//! 2. **Single dispatch entry**: every addon call goes through
//! `try_addon_func`, which `try_builtin_func` invokes after every
//! other built-in family. The sentinel format
//! (`__taida_addon_call::<package>::<function>`) is structurally
//! distinct from the underscore-flat sentinels used by
//! `__os_builtin_*`, `__net_builtin_*`, `__crypto_builtin_*`, etc.,
//! so collisions are impossible.

use crate::interpreter::eval::{Interpreter, RuntimeError, Signal};
use crate::interpreter::value::{ErrorValue, Value};
use crate::parser::Expr;

/// Sentinel prefix the import resolver writes into env entries for
/// addon-backed function symbols. The format is
/// `__taida_addon_call::<package_id>::<function_name>`.
const ADDON_SENTINEL_PREFIX: &str = "__taida_addon_call::";

fn public_error_ident_from_message(message: &str) -> String {
    let candidate = message
        .split_once(':')
        .map(|(head, _)| head.trim())
        .unwrap_or(message.trim());
    let mut chars = candidate.chars();
    let Some(first) = chars.next() else {
        return "AddonError".to_string();
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return "AddonError".to_string();
    }
    if chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        candidate.to_string()
    } else {
        "AddonError".to_string()
    }
}

fn addon_call_error_to_error_value(call_err: &crate::addon::AddonCallError) -> ErrorValue {
    let (error_type, message, code) = match call_err {
        crate::addon::AddonCallError::AddonError { code, message, .. } => (
            public_error_ident_from_message(message),
            message.clone(),
            i64::from(*code),
        ),
        crate::addon::AddonCallError::FunctionNotFound { .. } => {
            ("AddonFunctionNotFound".to_string(), call_err.to_string(), 0)
        }
        crate::addon::AddonCallError::ArityMismatch { .. } => {
            ("AddonArityMismatch".to_string(), call_err.to_string(), 0)
        }
        crate::addon::AddonCallError::UnsupportedInput { .. } => {
            ("AddonUnsupportedInput".to_string(), call_err.to_string(), 0)
        }
        crate::addon::AddonCallError::AddonStatus { .. }
        | crate::addon::AddonCallError::InvalidStatus { .. } => {
            ("AddonStatus".to_string(), call_err.to_string(), 0)
        }
        crate::addon::AddonCallError::MalformedOutput { .. } => {
            ("AddonMalformedOutput".to_string(), call_err.to_string(), 0)
        }
    };

    ErrorValue {
        error_type: error_type.clone(),
        message,
        fields: vec![
            ("kind".to_string(), Value::str(error_type)),
            ("code".to_string(), Value::Int(code)),
        ],
    }
}

impl Interpreter {
    /// Try to dispatch an addon-backed function call.
    ///
    /// Returns:
    /// - `Ok(Some(Signal::Value(_)))` on a successful addon call.
    /// - `Ok(Some(Signal::Throw(_)))` if the addon returned an error
    /// that should be propagated as a Taida throwable.
    /// - `Ok(None)` if `name` is not bound to an addon sentinel
    /// (caller should fall through to the next builtin family).
    /// - `Err(RuntimeError)` for unrecoverable host-side failures
    /// (e.g. registry lookup miss after a successful import).
    pub(crate) fn try_addon_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // Sentinel guard: extract `(package_id, function_name)` from
        // the env entry. Anything that does not start with the addon
        // prefix returns `None` immediately so the call site can fall
        // through to the next builtin family.
        let (package_id, function_name) = match self.env.get(name) {
            Some(Value::Str(tag)) if tag.starts_with(ADDON_SENTINEL_PREFIX) => {
                let payload = &tag[ADDON_SENTINEL_PREFIX.len()..];
                match payload.split_once("::") {
                    Some((pkg, func)) => (pkg.to_string(), func.to_string()),
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };

        // Defensive backend re-check. The import resolver already
        // gated this on `ensure_addon_supported`, so reaching this
        // point on a non-Native backend would mean the sentinel was
        // somehow leaked across backends. Re-check anyway -- the
        // design lock requires the dispatch path to also be
        // policy-aware.
        if let Err(policy_err) =
            crate::addon::ensure_addon_supported(crate::addon::AddonBackend::Native, &package_id)
        {
            return Err(RuntimeError {
                message: policy_err.to_string(),
            });
        }

        // Look the addon up in the process-wide registry. The import
        // path always populates the registry before binding the
        // sentinel, so a miss here is a hard host-side bug.
        let project_root = self.find_project_root();
        let resolved = match crate::addon::AddonRegistry::global()
            .lookup(&project_root, &package_id)
        {
            Some(r) => r,
            None => {
                return Err(RuntimeError {
                    message: format!(
                        "addon dispatch failed: package '{}' not found in registry. \
                         This is a host-side bug -- the import resolver should have populated the registry before binding the call sentinel.",
                        package_id
                    ),
                });
            }
        };

        // Evaluate every argument. Propagate Throw / Return signals.
        let mut evaluated_args: Vec<Value> = Vec::with_capacity(args.len());
        for arg in args {
            match self.eval_expr(arg)? {
                Signal::Value(v) => evaluated_args.push(v),
                other => return Ok(Some(other)),
            }
        }

        // Cross the safe addon boundary. `call_function` accepts
        // `&[Value]` and returns `Result<Value, AddonCallError>`. Raw
        // pointers stay inside `src/addon`.
        match resolved.call_function(&function_name, &evaluated_args) {
            Ok(value) => Ok(Some(Signal::Value(value))),
            Err(call_err) => Ok(Some(Signal::Throw(Value::Error(
                addon_call_error_to_error_value(&call_err),
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_str(fields: &[(String, Value)], name: &str) -> String {
        match fields.iter().find(|(field, _)| field == name) {
            Some((_, Value::Str(value))) => value.as_string().clone(),
            other => panic!("expected string field {name}, got {other:?}"),
        }
    }

    fn pack_int(fields: &[(String, Value)], name: &str) -> i64 {
        match fields.iter().find(|(field, _)| field == name) {
            Some((_, Value::Int(value))) => *value,
            other => panic!("expected int field {name}, got {other:?}"),
        }
    }

    #[test]
    fn addon_error_maps_to_canonical_error_info_fields() {
        let call_err = crate::addon::AddonCallError::AddonError {
            addon: "taida-lang/terminal".to_string(),
            function: "readEvent".to_string(),
            code: 4002,
            message: "ReadEventNotATty: stdin is not a TTY".to_string(),
        };

        let err = addon_call_error_to_error_value(&call_err);
        assert_eq!(err.error_type, "ReadEventNotATty");
        assert_eq!(err.message, "ReadEventNotATty: stdin is not a TTY");

        let info = Interpreter::error_info_value(&Value::Error(err));
        let Value::BuchiPack(fields) = info else {
            panic!("ErrorInfo must be represented as a buchi pack");
        };
        assert_eq!(pack_str(&fields, "type"), "ReadEventNotATty");
        assert_eq!(
            pack_str(&fields, "message"),
            "ReadEventNotATty: stdin is not a TTY"
        );
        assert_eq!(pack_str(&fields, "kind"), "ReadEventNotATty");
        assert_eq!(pack_int(&fields, "code"), 4002);
        assert_eq!(pack_str(&fields, "__type"), "ErrorInfo");
    }

    #[test]
    fn addon_error_falls_back_when_message_has_no_public_kind() {
        let call_err = crate::addon::AddonCallError::AddonError {
            addon: "taida-lang/terminal".to_string(),
            function: "readEvent".to_string(),
            code: 4002,
            message: "stdin is not a TTY".to_string(),
        };

        let err = addon_call_error_to_error_value(&call_err);
        assert_eq!(err.error_type, "AddonError");
        assert_eq!(err.message, "stdin is not a TTY");
        assert_eq!(pack_str(&err.fields, "kind"), "AddonError");
        assert_eq!(pack_int(&err.fields, "code"), 4002);
    }
}
