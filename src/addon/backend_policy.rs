//! Backend support policy for addon-backed packages.
//!
//! **C25 redefinition (C25B-030 Phase 1A)**: the RC1 "addon is Native only"
//! freeze is lifted. Addon-backed packages are now formally supported on
//! both the `Native` and `Interpreter` backends (the interpreter is the
//! reference implementation and always ships the dispatch path).
//!
//! **D28B-010 widening (@d.X stable initial release)**: `WasmFull` joins
//! `Native` and `Interpreter` as a first-class addon backend. wasm-full
//! is the only wasm profile in the stable contract that exposes the
//! addon dispatcher; wasm-min / wasm-wasi / wasm-edge remain unsupported
//! at @d.X. The widening is a `docs/STABILITY.md § 6.2` addition (the
//! set of accepted backends grows; no existing addon is reinterpreted),
//! so it does not require a generation bump. Manifest authors who want
//! to declare wasm-full compatibility add `"wasm-full"` to the top-level
//! `targets` array; addons that omit `targets` continue to default to
//! `["native"]` (D28B-021 contract preserved). The `Js` backend remains
//! unsupported until a JS-side dispatcher is designed.
//!
//! Historical note: the original RC1 freeze (see `.dev/RC1_DESIGN.md`
//! Section E and `.dev/RC1_IMPL_SPEC.md` Phase 0 Frozen Contracts) forced
//! the interpreter to masquerade as `AddonBackend::Native` to pass the
//! policy guard. That dishonest state is removed in C25; the interpreter
//! binary now calls `ensure_addon_supported(AddonBackend::Interpreter, ...)`
//! truthfully.
//!
//! This module is the single decision point. The Native dispatcher (RC1
//! Phase 4), the interpreter's addon import handler, and the import-
//! resolver guard for every other backend all call into
//! `ensure_addon_supported` so the policy lives in one place across all
//! backends.
//!
//! The module is intentionally `cfg`-free so that even builds without
//! the `native` feature can ask "is this backend allowed?" and bail out
//! cleanly.

use std::fmt;

/// All backends that may attempt to consume an addon-backed package.
///
/// **C25 policy (C25B-030)**: `Native` and `Interpreter` are first-class
/// addon backends. **D28B-010 (@d.X)**: `WasmFull` joins them as a
/// first-class addon backend (§6.2 widening). `Js`, `WasmMin`,
/// `WasmWasi`, and `WasmEdge` remain unsupported. Adding a new backend
/// to the supported set is a deliberate, RC-level decision -- the
/// policy table below must be updated in lockstep with the manifest
/// `SUPPORTED_ADDON_TARGETS` allowlist and `docs/STABILITY.md § 5.2`.
///
/// The enum is `#[non_exhaustive]` so future RCs can extend it without
/// breaking pattern matches in package-resolution code that has already
/// learned to call `ensure_addon_supported`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AddonBackend {
    /// Cranelift + native runtime. Dlopens cdylib addons directly.
    Native,
    /// Tree-walking interpreter. C25 first-class addon backend
    /// (reference implementation).
    Interpreter,
    /// JavaScript codegen. No addon dispatcher yet (post-stable).
    Js,
    /// `wasm-min` target. No addon dispatcher (out of @d.X stable).
    WasmMin,
    /// `wasm-wasi` target. No addon dispatcher (out of @d.X stable).
    WasmWasi,
    /// `wasm-edge` target. No addon dispatcher (out of @d.X stable).
    WasmEdge,
    /// `wasm-full` target. **D28B-010 (@d.X)**: addon dispatcher
    /// supported via the same registry / facade path used by Native and
    /// Interpreter. Manifest authors opt in by adding `"wasm-full"` to
    /// `targets`. cdylib loading on wasm-full reuses the host's native
    /// loader at this generation; a wasm-side dispatcher (cdylib loaded
    /// inside the wasm module) is post-stable scope.
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
    /// **C25 policy (C25B-030)**: `Native` and `Interpreter` are supported.
    /// **D28B-010 (@d.X)**: `WasmFull` is enrolled as a first-class
    /// addon backend (§6.2 widening). `Js`, `WasmMin`, `WasmWasi`, and
    /// `WasmEdge` remain unsupported. Do not add `_ => true` arms here
    /// -- new backends must be explicitly enrolled.
    pub fn supports_addons(self) -> bool {
        matches!(self, Self::Native | Self::Interpreter | Self::WasmFull)
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
        // Deterministic single-line message. The substring `"is not
        // supported on backend"` is stable and classifiable by the import
        // resolver and the LSP. D28B-010 replaces the C25-era
        // `(supported: interpreter, native; wasm planned for D26)` tail
        // with the @d.X-stable list `(supported: interpreter, native,
        // wasm-full)` so the message is actionable: users can see all
        // working targets without an obsolete reference to D26.
        write!(
            f,
            "addon-backed package '{}' is not supported on backend '{}' (supported: interpreter, native, wasm-full). Run 'taida build native' or use the interpreter; for wasm targets, only 'wasm-full' supports addons.",
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
    fn interpreter_native_and_wasm_full_are_supported() {
        // C25B-030 + D28B-010 policy lock: Interpreter, Native, and
        // WasmFull are first-class addon backends; Js / WasmMin /
        // WasmWasi / WasmEdge remain unsupported. Updating this list
        // without bumping the generation counter is a contract break
        // (D28B-010 widening was a §6.2 addition; future widenings
        // beyond @d.X must use the same gen-aware process).
        assert!(AddonBackend::Native.supports_addons());
        assert!(AddonBackend::Interpreter.supports_addons());
        assert!(AddonBackend::WasmFull.supports_addons());
        assert!(!AddonBackend::Js.supports_addons());
        assert!(!AddonBackend::WasmMin.supports_addons());
        assert!(!AddonBackend::WasmWasi.supports_addons());
        assert!(!AddonBackend::WasmEdge.supports_addons());
    }

    #[test]
    fn ensure_supported_passes_native() {
        let res = ensure_addon_supported(AddonBackend::Native, "taida-lang/terminal");
        assert!(res.is_ok());
    }

    #[test]
    fn ensure_supported_accepts_interpreter() {
        // C25B-030: the interpreter is a first-class addon backend.
        let res = ensure_addon_supported(AddonBackend::Interpreter, "taida-lang/terminal");
        assert!(res.is_ok());
    }

    #[test]
    fn ensure_supported_rejects_js() {
        // Js has no addon dispatcher yet (D26+).
        let err = ensure_addon_supported(AddonBackend::Js, "taida-lang/terminal")
            .expect_err("js must be rejected");
        assert_eq!(err.backend, AddonBackend::Js);
        assert_eq!(err.package, "taida-lang/terminal");
    }

    #[test]
    fn error_message_is_deterministic() {
        // The message must be stable so callers (CLI / LSP) can route
        // on the substring. We pin the exact text here. D28B-010 updated
        // the supported list to include wasm-full.
        let err = AddonBackendError::new(AddonBackend::Js, "taida-lang/terminal");
        assert_eq!(
            err.to_string(),
            "addon-backed package 'taida-lang/terminal' is not supported on backend 'js' (supported: interpreter, native, wasm-full). Run 'taida build native' or use the interpreter; for wasm targets, only 'wasm-full' supports addons."
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
        // Smoke check that the policy is uniform: every rejected backend
        // must produce the same shape of message so the LSP / CLI can
        // pattern-match a single substring ("is not supported on
        // backend"). C25B-030: Interpreter and Native no longer appear
        // here because they are supported. D28B-010: WasmFull also
        // moves to the supported set; only Js / WasmMin / WasmWasi /
        // WasmEdge remain rejected.
        for b in [
            AddonBackend::Js,
            AddonBackend::WasmMin,
            AddonBackend::WasmWasi,
            AddonBackend::WasmEdge,
        ] {
            let err = ensure_addon_supported(b, "p").unwrap_err();
            assert!(err.to_string().contains("is not supported on backend"));
            assert!(
                err.to_string()
                    .contains("(supported: interpreter, native, wasm-full)")
            );
        }
    }

    #[test]
    fn ensure_supported_accepts_wasm_full() {
        // D28B-010: WasmFull is a first-class addon backend at @d.X.
        // The widening is a §6.2 addition (no existing addon is
        // reinterpreted) and must remain pinned across the @d.* series.
        let res = ensure_addon_supported(AddonBackend::WasmFull, "taida-lang/terminal");
        assert!(res.is_ok());
    }
}
