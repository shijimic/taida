//! Interpreter dispatch for addon-backed function calls
//! (RC1 Phase 4 -- `RC1-4c`).
//!
//! `.dev/RC1_DESIGN.md` Phase 4 Lock §Dispatch and §Taida user surface
//! pin two contracts that this module enforces:
//!
//! 1. **No raw pointer in the user surface**: the dispatcher receives
//!    Taida `Value` arguments, calls
//!    `LoadedAddon::call_function(&[Value]) -> Result<Value, AddonCallError>`
//!    (Phase 3), and returns either a Taida `Value` or an error.
//!    `*mut TaidaAddon*` types live entirely inside `src/addon`.
//! 2. **Single dispatch entry**: every addon call goes through
//!    `try_addon_func`, which `try_builtin_func` invokes after every
//!    other built-in family. The sentinel format
//!    (`__taida_addon_call::<package>::<function>`) is structurally
//!    distinct from the underscore-flat sentinels used by
//!    `__os_builtin_*`, `__net_builtin_*`, `__crypto_builtin_*`, etc.,
//!    so collisions are impossible.

use crate::interpreter::eval::{Interpreter, RuntimeError, Signal};
use crate::interpreter::value::{ErrorValue, Value};
use crate::parser::Expr;

/// Sentinel prefix the import resolver writes into env entries for
/// addon-backed function symbols. The format is
/// `__taida_addon_call::<package_id>::<function_name>`.
const ADDON_SENTINEL_PREFIX: &str = "__taida_addon_call::";

impl Interpreter {
    /// Try to dispatch an addon-backed function call.
    ///
    /// Returns:
    /// - `Ok(Some(Signal::Value(_)))` on a successful addon call.
    /// - `Ok(Some(Signal::Throw(_)))` if the addon returned an error
    ///   that should be propagated as a Taida throwable.
    /// - `Ok(None)` if `name` is not bound to an addon sentinel
    ///   (caller should fall through to the next builtin family).
    /// - `Err(RuntimeError)` for unrecoverable host-side failures
    ///   (e.g. registry lookup miss after a successful import).
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

        // Cross the safe Phase 3 boundary. `call_function` accepts
        // `&[Value]` and returns `Result<Value, AddonCallError>`. Raw
        // pointers stay inside `src/addon`.
        match resolved.call_function(&function_name, &evaluated_args) {
            Ok(value) => Ok(Some(Signal::Value(value))),
            Err(call_err) => Ok(Some(Signal::Throw(Value::Error(ErrorValue {
                error_type: "AddonError".to_string(),
                message: call_err.to_string(),
                fields: Vec::new(),
            })))),
        }
    }
}
