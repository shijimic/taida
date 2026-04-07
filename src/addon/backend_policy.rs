//! Backend support policy for addon-backed packages.
//!
//! `.dev/RC1_DESIGN.md` Section E (Backend policy) and the Phase 0 Frozen
//! Contracts in `.dev/RC1_IMPL_SPEC.md` require that addon-backed packages
//! are *only* honoured by the Native backend, and that any other backend
//! produces a **deterministic error** at the import boundary -- not a
//! silent fallback, not a runtime callsite trap.
//!
//! This module is the single decision point. The Native dispatcher (RC1
//! Phase 4) and the import-resolver guard (also Phase 4) both call into
//! `ensure_addon_supported` so the policy lives in one place across all
//! backends.
//!
//! The module is intentionally `cfg`-free so that even non-Native builds
//! can ask "is this backend allowed?" and bail out cleanly.

use std::fmt;

/// All backends that may attempt to consume an addon-backed package.
///
/// `Native` is the only supported variant for RC1. Adding a new backend
/// here is a deliberate, RC-level decision -- the policy table below
/// must be updated in lockstep.
///
/// The enum is `#[non_exhaustive]` so future RCs can extend it without
/// breaking pattern matches in package-resolution code that has already
/// learned to call `ensure_addon_supported`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AddonBackend {
    /// Cranelift + native runtime. Only backend that can dlopen addons.
    Native,
    /// Tree-walking interpreter. RC1 design lock = unsupported.
    Interpreter,
    /// JavaScript codegen. RC1 design lock = unsupported.
    Js,
    /// `wasm-min` target. RC1 design lock = unsupported.
    WasmMin,
    /// `wasm-wasi` target. RC1 design lock = unsupported.
    WasmWasi,
    /// `wasm-edge` target. RC1 design lock = unsupported.
    WasmEdge,
    /// `wasm-full` target. RC1 design lock = unsupported.
    WasmFull,
}

impl AddonBackend {
    /// Stable label used in error messages and diagnostics.
    ///
    /// Matches the CLI `--target` flag spelling so users get a familiar
    /// name back when they hit the unsupported error.
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Interpreter => "interpreter",
            Self::Js => "js",
            Self::WasmMin => "wasm-min",
            Self::WasmWasi => "wasm-wasi",
            Self::WasmEdge => "wasm-edge",
            Self::WasmFull => "wasm-full",
        }
    }

    /// `true` iff this backend may load addon-backed packages.
    ///
    /// RC1 freeze: `Native` only. Do not add `_ => true` arms here -- new
    /// backends must be explicitly enrolled.
    pub fn supports_addons(self) -> bool {
        matches!(self, Self::Native)
    }
}

impl fmt::Display for AddonBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Error returned when a non-Native backend tries to use an addon-backed
/// package.
///
/// Carries the package name (so the user can find the offending import)
/// and the backend label. The `Display` impl produces the deterministic
/// message that `.dev/RC1_DESIGN.md` Section E.4 mandates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonBackendError {
    pub backend: AddonBackend,
    pub package: String,
}

impl AddonBackendError {
    /// Construct a new unsupported-backend diagnostic for `package`.
    pub fn new(backend: AddonBackend, package: impl Into<String>) -> Self {
        Self {
            backend,
            package: package.into(),
        }
    }
}

impl fmt::Display for AddonBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deterministic single-line message. RC1_IMPL_SPEC Phase 0 Frozen
        // Contracts require this string to be classifiable by the import
        // resolver and the LSP, so we keep punctuation simple and stable.
        write!(
            f,
            "addon-backed package '{}' is not supported on backend '{}' (RC1: native only)",
            self.package,
            self.backend.label()
        )
    }
}

impl std::error::Error for AddonBackendError {}

/// The single policy decision point.
///
/// Returns `Ok(())` if `backend` is allowed to consume addon-backed
/// packages, otherwise an [`AddonBackendError`] tagged with `package`.
///
/// Phase 4 (`RC1-4*`) wires this into the import resolver so that
/// `import "taida-lang/terminal"` (an addon-backed package) on, say,
/// the JS backend produces a compile-time error rather than crashing
/// the runtime.
pub fn ensure_addon_supported(
    backend: AddonBackend,
    package: &str,
) -> Result<(), AddonBackendError> {
    if backend.supports_addons() {
        Ok(())
    } else {
        Err(AddonBackendError::new(backend, package.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_is_the_only_supported_backend() {
        // RC1 design lock: native only. Updating this list without bumping
        // the RC is a contract break.
        assert!(AddonBackend::Native.supports_addons());
        assert!(!AddonBackend::Interpreter.supports_addons());
        assert!(!AddonBackend::Js.supports_addons());
        assert!(!AddonBackend::WasmMin.supports_addons());
        assert!(!AddonBackend::WasmWasi.supports_addons());
        assert!(!AddonBackend::WasmEdge.supports_addons());
        assert!(!AddonBackend::WasmFull.supports_addons());
    }

    #[test]
    fn ensure_supported_passes_native() {
        let res = ensure_addon_supported(AddonBackend::Native, "taida-lang/terminal");
        assert!(res.is_ok());
    }

    #[test]
    fn ensure_supported_rejects_interpreter() {
        let err = ensure_addon_supported(AddonBackend::Interpreter, "taida-lang/terminal")
            .expect_err("interpreter must be rejected");
        assert_eq!(err.backend, AddonBackend::Interpreter);
        assert_eq!(err.package, "taida-lang/terminal");
    }

    #[test]
    fn error_message_is_deterministic() {
        // Phase 0 Frozen Contracts: message must be stable so callers can
        // route on it. We pin the exact text here.
        let err = AddonBackendError::new(AddonBackend::Js, "taida-lang/terminal");
        assert_eq!(
            err.to_string(),
            "addon-backed package 'taida-lang/terminal' is not supported on backend 'js' (RC1: native only)"
        );
    }

    #[test]
    fn labels_match_cli_spelling() {
        // The CLI uses these exact strings as `--target` values. Drift
        // here would confuse users hitting the unsupported error.
        assert_eq!(AddonBackend::Native.label(), "native");
        assert_eq!(AddonBackend::Js.label(), "js");
        assert_eq!(AddonBackend::WasmMin.label(), "wasm-min");
        assert_eq!(AddonBackend::WasmWasi.label(), "wasm-wasi");
        assert_eq!(AddonBackend::WasmEdge.label(), "wasm-edge");
        assert_eq!(AddonBackend::WasmFull.label(), "wasm-full");
        assert_eq!(AddonBackend::Interpreter.label(), "interpreter");
    }

    #[test]
    fn rejected_backends_share_one_message_format() {
        // Smoke check that the policy is uniform: every non-Native
        // variant must produce the same shape of message so the LSP /
        // CLI can pattern-match a single substring ("is not supported on
        // backend").
        for b in [
            AddonBackend::Interpreter,
            AddonBackend::Js,
            AddonBackend::WasmMin,
            AddonBackend::WasmWasi,
            AddonBackend::WasmEdge,
            AddonBackend::WasmFull,
        ] {
            let err = ensure_addon_supported(b, "p").unwrap_err();
            assert!(err.to_string().contains("is not supported on backend"));
            assert!(err.to_string().contains("(RC1: native only)"));
        }
    }
}
