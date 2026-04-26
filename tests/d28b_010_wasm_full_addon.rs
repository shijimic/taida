//! D28B-010 (@d.X stable widening): WasmFull addon dispatcher 正式化.
//!
//! Phase 0 で POST-STABLE 送り判定だったが 2026-04-26 user verdict で
//! D28 scope に復帰。`@d.X` breaking-change phase で wasm addon
//! backend を一級 supported に widening するのが coherent (主軸 4
//! multi-target の coherence)、widening は §6.2 addition なので
//! gen 不要だが gen-D で land しないと「wasm widened by next gen」の
//! ための breaking-change schedule が必要になる。今 land = 後続
//! generation の breaking change を 1 件減らす。
//!
//! Acceptance:
//!   1. `AddonBackend::WasmFull.supports_addons() == true`
//!   2. `AddonBackend::WasmMin / WasmWasi / WasmEdge.supports_addons() == false`
//!   3. `ensure_addon_supported(WasmFull, ...)` returns `Ok(())`
//!   4. Manifest with `targets = ["wasm-full"]` parses without `[E2001]`
//!   5. Manifest with `targets = ["native", "wasm-full"]` parses
//!   6. Manifest with `targets = ["wasm"]` (legacy short form) is still
//!      rejected — the allowlist contains the canonical `"wasm-full"`
//!      label, not a short alias
//!   7. `SUPPORTED_ADDON_TARGETS` contains both `"native"` and `"wasm-full"`
//!   8. Default value when `targets` is omitted remains `["native"]` (the
//!      D28B-021 bit-identical contract is preserved across the widening)
//!
//! These eight assertions pin both the policy decision (the
//! `supports_addons` change) and the manifest schema decision (the
//! allowlist widening) so a future generation cannot silently un-widen
//! the wasm-full opt-in without the test failing.

use std::path::Path;
use taida::addon::backend_policy::{AddonBackend, ensure_addon_supported};
use taida::addon::manifest::{
    AddonManifest, AddonManifestError, SUPPORTED_ADDON_TARGETS, default_addon_targets,
    parse_addon_manifest_str,
};

fn parse(source: &str) -> Result<AddonManifest, AddonManifestError> {
    parse_addon_manifest_str(Path::new("test://d28b_010_wasm_full.toml"), source)
}

#[test]
fn d28b_010_wasm_full_supports_addons() {
    assert!(
        AddonBackend::WasmFull.supports_addons(),
        "D28B-010: WasmFull must be enrolled in supports_addons() at @d.X"
    );
}

#[test]
fn d28b_010_other_wasm_profiles_remain_unsupported() {
    // The widening is intentionally narrow: only WasmFull joins the
    // supported set. WasmMin / WasmWasi / WasmEdge stay rejected so
    // their addon dispatch path can be designed independently in
    // post-stable scope.
    assert!(!AddonBackend::WasmMin.supports_addons());
    assert!(!AddonBackend::WasmWasi.supports_addons());
    assert!(!AddonBackend::WasmEdge.supports_addons());
}

#[test]
fn d28b_010_ensure_addon_supported_accepts_wasm_full() {
    let res = ensure_addon_supported(AddonBackend::WasmFull, "taida-lang/terminal");
    assert!(
        res.is_ok(),
        "D28B-010: ensure_addon_supported must accept WasmFull at @d.X"
    );
}

#[test]
fn d28b_010_supported_addon_targets_includes_native_and_wasm_full() {
    // The allowlist is the manifest-schema half of the widening. Pin
    // the exact contents so accidental additions / deletions surface
    // as a test failure rather than a silent contract drift.
    assert_eq!(
        SUPPORTED_ADDON_TARGETS,
        &["native", "wasm-full"],
        "D28B-010: SUPPORTED_ADDON_TARGETS must contain exactly \
         [native, wasm-full] at @d.X"
    );
}

#[test]
fn d28b_010_manifest_with_wasm_full_target_parses() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b010-sample"
library = "d28b010_sample"
targets = ["wasm-full"]

[functions]
echo = 1
"#;
    let manifest = parse(src).expect("targets = [\"wasm-full\"] must parse at @d.X");
    assert_eq!(manifest.targets, vec!["wasm-full".to_string()]);
}

#[test]
fn d28b_010_manifest_with_native_and_wasm_full_targets_parses() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b010-sample"
library = "d28b010_sample"
targets = ["native", "wasm-full"]

[functions]
echo = 1
"#;
    let manifest = parse(src).expect("targets = [\"native\", \"wasm-full\"] must parse at @d.X");
    assert_eq!(
        manifest.targets,
        vec!["native".to_string(), "wasm-full".to_string()]
    );
}

#[test]
fn d28b_010_short_wasm_alias_remains_rejected() {
    // The allowlist contains the canonical `"wasm-full"` label, not a
    // short `"wasm"` alias. Manifests that use the legacy short form
    // continue to be rejected with [E2001]. This guards against
    // future drift where someone might "helpfully" accept `"wasm"` as
    // an alias and silently route it to wasm-full.
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b010-sample"
library = "d28b010_sample"
targets = ["wasm"]

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("short \"wasm\" alias must remain rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("[E2001]"),
        "diagnostic must include [E2001], got: {msg}"
    );
    assert!(
        msg.contains("'wasm'"),
        "diagnostic must echo the offending target, got: {msg}"
    );
}

#[test]
fn d28b_010_default_when_targets_omitted_is_still_native_only() {
    // The D28B-021 default-injection contract is preserved across the
    // D28B-010 widening: addons that omit `targets` continue to
    // resolve to `["native"]`, not `["native", "wasm-full"]`. The
    // widening grew the allowlist but did NOT change what existing
    // addons are interpreted as.
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b010-sample"
library = "d28b010_sample"

[functions]
echo = 1
"#;
    let manifest = parse(src).expect("manifest without targets must parse");
    assert_eq!(
        manifest.targets,
        vec!["native".to_string()],
        "D28B-021 default must remain ['native'] across the D28B-010 widening"
    );
    assert_eq!(default_addon_targets(), vec!["native".to_string()]);
}
