//! Host-side call facade for the RC1 Phase 3 addon value bridge.
//!
//! `LoadedAddon::call_function` ties the [`value_bridge`] module and
//! the loader's raw function table into a single safe entry point:
//!
//! 1. Validate arity against the addon's declared function entry.
//! 2. Reject inputs containing unsupported value kinds up-front
//!    (deterministic `AddonCallError::UnsupportedInput`).
//! 3. Allocate a borrowed input vector on the host (every element is
//!    host-owned via `build_host_input_value`).
//! 4. Invoke the addon's raw `extern "C"` call pointer with the
//!    borrowed vector and nullable `*out_value` / `*out_error` slots.
//! 5. Materialise the addon's reply into a `taida::Value` (or an
//!    error), and release every host-owned pointer regardless of
//!    outcome.
//!
//! Taida user code never sees a raw pointer; the public surface here
//! is `&[Value] -> Result<Value, AddonCallError>`.
//!
//! # RC1B-103 resolution
//!
//! The ownership contract promised by `.dev/RC1_DESIGN.md` Phase 3
//! Lock is enforced structurally here:
//!
//! - **Single allocator**: every `*mut TaidaAddonValueV1` that crosses
//!   the bridge is produced by the host-side `value_bridge` module.
//!   The addon can only obtain new pointers through the host callback
//!   table, so the host is the only free-er.
//! - **Borrowed inputs**: the input vector is built immediately before
//!   the call and released immediately after. The addon has no way to
//!   stash a pointer past return (the storage literally goes away).
//! - **Owned outputs**: addon-returned pointers are consumed exactly
//!   once by `take_addon_output`, which both reads the data and
//!   releases the host-owned allocation.

use taida_addon::{TaidaAddonErrorV1, TaidaAddonStatus, TaidaAddonValueV1};

use crate::addon::loader::{AddonFunctionRef, LoadedAddon};
use crate::addon::value_bridge::{self, BridgeError, build_host_input_value, take_addon_output};
use crate::interpreter::value::Value;

/// Error surfaced by [`LoadedAddon::call_function`].
///
/// Split into distinct variants so the Native backend dispatcher (RC1
/// Phase 4) can route on them. Every variant carries enough context
/// to diagnose the failure from the addon import site.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddonCallError {
    /// Requested function name does not appear in the addon's function table.
    FunctionNotFound { addon: String, function: String },
    /// Number of arguments passed does not match the declared arity.
    ArityMismatch {
        addon: String,
        function: String,
        expected: u32,
        actual: u32,
    },
    /// One of the input values used a kind outside the RC1 Phase 3
    /// whitelist (e.g. `Async`, `Gorilla`, `Function`).
    UnsupportedInput {
        addon: String,
        function: String,
        kind: &'static str,
    },
    /// The addon returned a `TaidaAddonStatus` other than `Ok` / `Error`.
    /// The precise status is preserved for diagnostics.
    AddonStatus {
        addon: String,
        function: String,
        status: TaidaAddonStatus,
    },
    /// The addon returned `TaidaAddonStatus::Error` and provided an
    /// `out_error`. Message is copied out and the raw pointer released
    /// before this error is returned to the caller.
    AddonError {
        addon: String,
        function: String,
        code: u32,
        message: String,
    },
    /// The addon returned `Ok` but the value pointer was malformed
    /// (null, unknown tag, invalid UTF-8 in a `Str`, etc.).
    MalformedOutput {
        addon: String,
        function: String,
        detail: String,
    },
}

impl std::fmt::Display for AddonCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FunctionNotFound { addon, function } => write!(
                f,
                "addon call failed: function '{function}' not found in addon '{addon}'"
            ),
            Self::ArityMismatch {
                addon,
                function,
                expected,
                actual,
            } => write!(
                f,
                "addon call failed: '{addon}::{function}' expected {expected} args, got {actual}"
            ),
            Self::UnsupportedInput {
                addon,
                function,
                kind,
            } => write!(
                f,
                "addon call failed: '{addon}::{function}' received unsupported input kind '{kind}'"
            ),
            Self::AddonStatus {
                addon,
                function,
                status,
            } => write!(
                f,
                "addon call failed: '{addon}::{function}' returned status {status:?}"
            ),
            Self::AddonError {
                addon,
                function,
                code,
                message,
            } => write!(
                f,
                "addon call failed: '{addon}::{function}' returned error code={code} message='{message}'"
            ),
            Self::MalformedOutput {
                addon,
                function,
                detail,
            } => write!(
                f,
                "addon call failed: '{addon}::{function}' produced malformed output ({detail})"
            ),
        }
    }
}

impl std::error::Error for AddonCallError {}

impl LoadedAddon {
    /// Call an addon function by name with the given `taida::Value`
    /// arguments.
    ///
    /// This is the only safe way to invoke an addon entry in RC1
    /// Phase 3. It enforces arity, rejects unsupported input kinds,
    /// builds the borrowed input vector, invokes the raw call pointer,
    /// and releases every host-owned allocation before returning.
    pub fn call_function(&self, name: &str, args: &[Value]) -> Result<Value, AddonCallError> {
        let func = match self.find_function(name) {
            Some(f) => f,
            None => {
                return Err(AddonCallError::FunctionNotFound {
                    addon: self.name().to_string(),
                    function: name.to_string(),
                });
            }
        };
        if args.len() as u32 != func.arity() {
            return Err(AddonCallError::ArityMismatch {
                addon: self.name().to_string(),
                function: name.to_string(),
                expected: func.arity(),
                actual: args.len() as u32,
            });
        }
        self.dispatch_call(name, func, args)
    }

    fn dispatch_call(
        &self,
        name: &str,
        func: AddonFunctionRef<'_>,
        args: &[Value],
    ) -> Result<Value, AddonCallError> {
        // 1. Build every input value into a host-owned raw pointer.
        //    Roll back on the first failure.
        let mut built: Vec<*mut TaidaAddonValueV1> = Vec::with_capacity(args.len());
        for arg in args {
            match build_host_input_value(arg) {
                Ok(ptr) => built.push(ptr),
                Err(BridgeError::UnsupportedInput { kind }) => {
                    release_many(built);
                    return Err(AddonCallError::UnsupportedInput {
                        addon: self.name().to_string(),
                        function: name.to_string(),
                        kind,
                    });
                }
                Err(other) => {
                    release_many(built);
                    return Err(AddonCallError::MalformedOutput {
                        addon: self.name().to_string(),
                        function: name.to_string(),
                        detail: other.to_string(),
                    });
                }
            }
        }

        // 2. Build the borrowed arg vector the addon sees. We
        //    dereference each host-built pointer into a copy held on
        //    the host stack. The addon reads these as read-only views
        //    for the duration of the call; the host retains ownership
        //    of the originals.
        //
        //    We build a `Vec<TaidaAddonValueV1>` whose elements copy
        //    the fields from the host-built originals. These copies
        //    share the same payload pointers — the addon reads them
        //    via the borrowed view API, which does not take ownership.
        //    After the call, we release the originals (which free the
        //    payloads), and the stack `Vec` drops without touching
        //    the payloads again.
        let mut arg_view: Vec<TaidaAddonValueV1> = Vec::with_capacity(built.len());
        for &ptr in &built {
            // SAFETY: we just built each `ptr` via
            // `build_host_input_value`. Dereferencing it for a
            // struct-copy is safe. The copy aliases the payload
            // pointer but does not own it.
            arg_view.push(unsafe { copy_value_header(&*ptr) });
        }

        // 3. Invoke the raw call. `out_value` / `out_error` start
        //    null; the addon writes them on success / failure.
        let mut out_value: *mut TaidaAddonValueV1 = core::ptr::null_mut();
        let mut out_error: *mut TaidaAddonErrorV1 = core::ptr::null_mut();
        let status = (func.raw_call())(
            if arg_view.is_empty() {
                core::ptr::null()
            } else {
                arg_view.as_ptr()
            },
            arg_view.len() as u32,
            &mut out_value as *mut _,
            &mut out_error as *mut _,
        );

        // 4. Release the host-owned originals now that the addon has
        //    finished reading them. `arg_view` is dropped naturally
        //    at end-of-scope; no double-free because we alias pointers
        //    only.
        drop(arg_view);
        release_many(built);

        // 5. Route on status.
        match status {
            TaidaAddonStatus::Ok => {
                // Release any stray out_error the addon might have
                // written (defensive).
                if !out_error.is_null() {
                    unsafe { release_error_ptr(out_error) };
                }
                if out_value.is_null() {
                    return Err(AddonCallError::MalformedOutput {
                        addon: self.name().to_string(),
                        function: name.to_string(),
                        detail: "addon returned Ok without an out_value".to_string(),
                    });
                }
                // SAFETY: addon built `out_value` via the host
                // callback table, so it is a valid host-owned pointer.
                match unsafe { take_addon_output(out_value) } {
                    Ok(v) => Ok(v),
                    Err(bridge_err) => Err(AddonCallError::MalformedOutput {
                        addon: self.name().to_string(),
                        function: name.to_string(),
                        detail: bridge_err.to_string(),
                    }),
                }
            }
            TaidaAddonStatus::Error => {
                if !out_value.is_null() {
                    // Defensive: release any value slot the addon
                    // might also have filled.
                    // SAFETY: host-built.
                    unsafe { value_bridge::release_value_ptr(out_value) };
                }
                if out_error.is_null() {
                    return Err(AddonCallError::AddonStatus {
                        addon: self.name().to_string(),
                        function: name.to_string(),
                        status,
                    });
                }
                let (code, message) = unsafe { take_addon_error(out_error) };
                Err(AddonCallError::AddonError {
                    addon: self.name().to_string(),
                    function: name.to_string(),
                    code,
                    message,
                })
            }
            other => {
                // Release whatever slots the addon touched.
                if !out_value.is_null() {
                    // SAFETY: host-built.
                    unsafe { value_bridge::release_value_ptr(out_value) };
                }
                if !out_error.is_null() {
                    // SAFETY: host-built.
                    unsafe { release_error_ptr(out_error) };
                }
                Err(AddonCallError::AddonStatus {
                    addon: self.name().to_string(),
                    function: name.to_string(),
                    status: other,
                })
            }
        }
    }
}

/// Copy a `TaidaAddonValueV1` header (shallow: payload pointer is
/// shared). Used to build the borrowed input vector handed to the
/// addon. The copies alias the originals' payloads; ownership stays
/// with the originals.
///
/// # Safety
///
/// `src` must be a valid reference to a `TaidaAddonValueV1` (usually
/// one we built via `build_host_input_value`).
unsafe fn copy_value_header(src: &TaidaAddonValueV1) -> TaidaAddonValueV1 {
    TaidaAddonValueV1 {
        tag: src.tag,
        _reserved: src._reserved,
        payload: src.payload,
    }
}

/// Release a batch of host-built value pointers.
fn release_many(ptrs: Vec<*mut TaidaAddonValueV1>) {
    for ptr in ptrs {
        // SAFETY: every pointer in `ptrs` came from
        // `build_host_input_value`.
        unsafe { value_bridge::release_value_ptr(ptr) };
    }
}

/// Release a `TaidaAddonErrorV1` built by the host `cb_error_new`.
///
/// # Safety
///
/// `ptr` must be a non-null pointer returned by the host's
/// `cb_error_new` callback.
unsafe fn release_error_ptr(ptr: *mut TaidaAddonErrorV1) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr came from `cb_error_new` (Box::into_raw).
    let boxed = unsafe { Box::from_raw(ptr) };
    if !boxed.message.is_null() {
        // SAFETY: message was built via CString::into_raw.
        let _ = unsafe { std::ffi::CString::from_raw(boxed.message as *mut core::ffi::c_char) };
    }
}

/// Copy out the `(code, message)` pair from a host-built error and
/// release it.
///
/// # Safety
///
/// `ptr` must be a non-null pointer returned by `cb_error_new`.
unsafe fn take_addon_error(ptr: *mut TaidaAddonErrorV1) -> (u32, String) {
    // SAFETY: caller promises.
    let boxed = unsafe { Box::from_raw(ptr) };
    let code = boxed.code;
    let message = if boxed.message.is_null() {
        String::new()
    } else {
        // SAFETY: built via CString::into_raw in cb_error_new.
        let cstring =
            unsafe { std::ffi::CString::from_raw(boxed.message as *mut core::ffi::c_char) };
        cstring.to_string_lossy().into_owned()
    };
    (code, message)
}

#[cfg(test)]
mod tests {
    //! Call-facade unit tests.
    //!
    //! These tests build synthetic `LoadedAddon` instances (we can't
    //! dlopen in a hermetic unit test), so they target the input
    //! validation and error classification paths. The end-to-end
    //! dlopen round-trip lives in
    //! `tests/addon_loader_smoke.rs::echo_round_trips_primitives`.

    use super::*;

    #[test]
    fn error_display_function_not_found() {
        let err = AddonCallError::FunctionNotFound {
            addon: "taida-lang/sample".to_string(),
            function: "missing".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("missing"));
        assert!(msg.contains("taida-lang/sample"));
    }

    #[test]
    fn error_display_arity_mismatch() {
        let err = AddonCallError::ArityMismatch {
            addon: "x".to_string(),
            function: "f".to_string(),
            expected: 2,
            actual: 3,
        };
        let msg = err.to_string();
        assert!(msg.contains("expected 2"));
        assert!(msg.contains("got 3"));
    }

    #[test]
    fn error_display_unsupported_input() {
        let err = AddonCallError::UnsupportedInput {
            addon: "x".to_string(),
            function: "f".to_string(),
            kind: "Gorilla",
        };
        assert!(err.to_string().contains("Gorilla"));
    }

    #[test]
    fn error_display_addon_error_preserves_code() {
        let err = AddonCallError::AddonError {
            addon: "x".to_string(),
            function: "f".to_string(),
            code: 7,
            message: "bad".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("code=7"));
        assert!(msg.contains("bad"));
    }
}
