//! Taida Lang -- addon foundation host bindings.
//!
//! This module is the **host-side** counterpart to the `taida-addon` crate
//! (`crates/addon-rs`). It exposes:
//!
//! 1. [`backend_policy`] -- centralized "is this backend allowed to consume
//!    addon-backed packages?" decision. Used by both the Native dispatcher
//!    (which says yes) and every other backend (which says no, with a
//!    deterministic error message). RC1 design lock requires that
//!    `unsupported` is *never* a silent fallback.
//!
//! 2. [`loader`] (Native-only) -- a thin RAII wrapper around `libloading`
//!    that resolves the frozen `taida_addon_get_v1` symbol, validates the
//!    ABI version handshake **before** touching any other descriptor
//!    field, and reports load failures with structured, distinct error
//!    variants (RC1B-101 / RC1B-102).
//!
//! ## Non-negotiable invariants (`.dev/RC1_DESIGN.md`)
//!
//! - RC1 is **Native backend only**. Any other backend that hits an
//!   addon-backed package must produce a deterministic error at the
//!   import boundary. Silent fallback is forbidden.
//! - The loader validates `descriptor.abi_version == TAIDA_ADDON_ABI_VERSION`
//!   *before* reading any other field. Mismatch -> hard
//!   `AddonLoadError::AbiMismatch`. (RC1B-101)
//! - Load failures are split into distinct, classifiable variants so
//!   diagnostics can route on them. (RC1B-102)
//! - Taida user code never sees raw pointers; the safe Rust API in this
//!   module is the only allowed boundary.
//!
//! Phase 2 deliberately stops at the loader + the policy guard. Phase 3
//! (`RC1-3*`) wires the value bridge; Phase 4 (`RC1-4*`) connects the
//! loader to the package import path.

pub mod backend_policy;
// RC2.7 Phase 1: shared addon build helper (publish + install fallback).
#[cfg(feature = "native")]
pub mod build;
pub mod manifest;

// C25B-030 Phase 1G: shared addon-facade static analyser. Extracted
// from `src/codegen/lower/imports.rs` so the D26-planned WASM
// backend can consume the same `AddonFacadeSummary` shape without
// re-deriving the recursive `>>>` walker or the reachability
// expansion. The interpreter's addon-facade path
// (`src/interpreter/module_eval.rs::load_addon_facade`) continues
// to execute the facade dynamically — it runs as Taida's
// reference implementation — but every codegen backend routes
// through this module.
pub mod facade;

// RC1.5: install-time pipeline
pub mod host_target;
pub mod prebuild_fetcher;
// C26B-030 / SEC-011: install-side Sigstore cosign verification hook.
// Pairs with `.github/workflows/release.yml`'s `sign` + `provenance`
// jobs and `scripts/release/verify-signatures.sh` so the signature
// the release workflow generates is actually consumed at install
// time. Kept feature-flag-free so local/offline tests can exercise
// `file://` bundle handling without the `community` HTTPS stack.
pub mod signature_verify;
pub mod url_template;

// RC2.6 Phase 1: addon publish lockfile (native/addon.lock.toml).
// Pure data / hand-rolled TOML subset, no feature gate needed — the
// module is consumed by both the `community` publish flow and (later)
// the install resolver on every backend that needs to inspect per-host
// SHA-256 digests.
pub mod lockfile;

#[cfg(feature = "native")]
pub mod loader;

#[cfg(feature = "native")]
pub mod value_bridge;

#[cfg(feature = "native")]
pub mod call;

#[cfg(feature = "native")]
pub mod registry;

#[cfg(feature = "native")]
pub use loader::{AddonFunctionRef, AddonLoadError, LoadedAddon};

#[cfg(feature = "native")]
pub use call::AddonCallError;

#[cfg(feature = "native")]
pub use value_bridge::{BridgeError, make_host_table};

#[cfg(feature = "native")]
pub use registry::{AddonImportError, AddonRegistry, ResolvedAddon};

pub use backend_policy::{AddonBackend, AddonBackendError, ensure_addon_supported};
pub use manifest::{AddonManifest, AddonManifestError, parse_addon_manifest};

// Re-export the ABI constants the host loader pins on. Keeping them here
// (rather than asking callers to depend on `taida_addon` directly) means
// host code has a single import path for the addon foundation.
pub use taida_addon::{TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL};

/// Re-export of the addon authoring crate so downstream consumers
/// (integration tests, future Phase 4 import resolver) have a single
/// path to the frozen ABI v1 types without an additional Cargo.toml
/// dependency. The crate is intentionally tiny and `no_std` friendly.
pub use taida_addon as abi_crate;
