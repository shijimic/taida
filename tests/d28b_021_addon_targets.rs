//! D28B-021: addon manifest `targets` 互換契約 — bit-identical /
//! unknown reject / `[E2xxx]` 文言 / contract-level acceptance tests.
//!
//! Phase 0 Design Lock (`@d.X`) pinned the following four guarantees;
//! this file is the executable evidence:
//!
//! 1. `targets` 欠落時の default 値 = `["native"]`、loader が **明示
//!    inject** する (silent fallback ではない、parsed `AddonManifest`
//!    の `targets` フィールドに必ず値が入る)。
//! 2. `targets = ["native"]` 明示と `targets` 欠落の **bit-identical**
//!    挙動 (`AddonManifest` 同値 + 例外メッセージ同値)。
//! 3. unknown target は parse 時に early reject、`[E2001]` / `[E2002]`
//!    の文言を含む `AddonManifestError` で返す。
//! 4. stable 後の default 変更は次 gen でしか許容しない (本契約は
//!    `docs/STABILITY.md § 6` + `docs/reference/addon_manifest.md` で
//!    明文化、本テストは default 値の現状を pin することで
//!    accidental drift を CI で捕捉する)。

use taida::addon::manifest::{
    AddonManifest, AddonManifestError, SUPPORTED_ADDON_TARGETS, default_addon_targets,
    parse_addon_manifest_str,
};

use std::path::Path;

fn parse(source: &str) -> Result<AddonManifest, AddonManifestError> {
    parse_addon_manifest_str(Path::new("test://d28b_021_addon.toml"), source)
}

const HEADER_OMITTED: &str = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"

[functions]
echo = 1
"#;

const HEADER_EXPLICIT_NATIVE: &str = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["native"]

[functions]
echo = 1
"#;

// ── Acceptance 1: default は `["native"]`、loader が明示 inject ────────

#[test]
fn d28b_021_default_is_native_when_targets_absent() {
    let manifest = parse(HEADER_OMITTED).expect("manifest without targets must parse");
    assert_eq!(
        manifest.targets,
        vec!["native".to_string()],
        "targets must be explicitly populated with [\"native\"] when omitted"
    );
}

#[test]
fn d28b_021_default_addon_targets_helper_returns_native() {
    // Pin the default value at the public API level so changing it is
    // a visible diff in this test as well as in the manifest module.
    assert_eq!(default_addon_targets(), vec!["native".to_string()]);
}

#[test]
fn d28b_021_supported_targets_is_native_only() {
    // D28B-021 contract: stable 後の default / allowlist 変更は次 gen
    // でしか許容しない。本 assertion は accidental allowlist 拡張を
    // CI で捕捉する。
    assert_eq!(SUPPORTED_ADDON_TARGETS, &["native"]);
}

// ── Acceptance 2: 欠落 と `targets = ["native"]` 明示の bit-identical ──

#[test]
fn d28b_021_omitted_and_explicit_native_are_bit_identical_struct() {
    let omitted = parse(HEADER_OMITTED).expect("omitted form must parse");
    let explicit = parse(HEADER_EXPLICIT_NATIVE).expect("explicit form must parse");

    // Same `AddonManifest` value (PartialEq derives over every field).
    assert_eq!(
        omitted, explicit,
        "omitted `targets` and explicit `targets = [\"native\"]` must produce \
         a structurally identical AddonManifest"
    );

    // Debug repr is identical too — protects against future fields
    // added to the struct that derive PartialEq trivially but get
    // serialised differently.
    assert_eq!(format!("{:?}", omitted), format!("{:?}", explicit));
}

#[test]
fn d28b_021_omitted_and_explicit_native_have_identical_targets_vec() {
    let omitted = parse(HEADER_OMITTED).expect("omitted form must parse");
    let explicit = parse(HEADER_EXPLICIT_NATIVE).expect("explicit form must parse");
    assert_eq!(omitted.targets, explicit.targets);
    assert_eq!(omitted.targets, vec!["native".to_string()]);
}

#[test]
fn d28b_021_duplicate_native_collapses_to_single_entry_bit_identical() {
    // `targets = ["native", "native"]` collapses to `["native"]` so
    // the bit-identical contract still holds even for sloppy authors.
    let dup_src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["native", "native"]

[functions]
echo = 1
"#;
    let dup = parse(dup_src).expect("duplicate native must parse to single entry");
    let omitted = parse(HEADER_OMITTED).expect("omitted form must parse");
    assert_eq!(dup, omitted);
}

// ── Acceptance 3: unknown target は `[E2001]` で early reject ──

#[test]
fn d28b_021_unknown_target_is_rejected_with_e2001() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["wasm"]

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("unknown target must be rejected");
    match &err {
        AddonManifestError::UnknownAddonTarget { target, .. } => {
            assert_eq!(target, "wasm");
        }
        other => panic!("expected UnknownAddonTarget, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("[E2001]"),
        "diagnostic must include [E2001], got: {msg}"
    );
    assert!(
        msg.contains("unknown addon target"),
        "diagnostic message must mention 'unknown addon target', got: {msg}"
    );
    assert!(
        msg.contains("'wasm'"),
        "diagnostic must echo the offending target, got: {msg}"
    );
    assert!(
        msg.contains("supported: native"),
        "diagnostic must list the supported allowlist, got: {msg}"
    );
}

#[test]
fn d28b_021_mixed_valid_and_unknown_rejects_at_first_unknown() {
    // Even when "native" appears before the unknown entry, the parser
    // must reject the manifest as a whole — silently dropping the
    // unknown entry would defeat the compatibility contract.
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["native", "future-vm"]

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("mixed unknown target must be rejected");
    match err {
        AddonManifestError::UnknownAddonTarget { target, .. } => {
            assert_eq!(target, "future-vm");
        }
        other => panic!("expected UnknownAddonTarget, got {other:?}"),
    }
}

#[test]
fn d28b_021_case_sensitive_target_match() {
    // "Native" (capital N) must be rejected — the allowlist is
    // canonical lowercase to avoid `Native` / `native` ambiguity in
    // dispatcher routing.
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["Native"]

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("Capitalised target must be rejected");
    assert!(matches!(
        err,
        AddonManifestError::UnknownAddonTarget { ref target, .. } if target == "Native"
    ));
}

// ── Acceptance 4: empty array は `[E2002]` で reject ──

#[test]
fn d28b_021_empty_targets_array_is_rejected_with_e2002() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = []

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("empty targets array must be rejected");
    assert!(matches!(err, AddonManifestError::EmptyAddonTargets { .. }));
    let msg = err.to_string();
    assert!(
        msg.contains("[E2002]"),
        "diagnostic must include [E2002], got: {msg}"
    );
    assert!(
        msg.contains("non-empty array"),
        "diagnostic must mention non-empty array, got: {msg}"
    );
}

// ── Acceptance 5: targets が array 以外の型は AddonTargetsTypeMismatch ──

#[test]
fn d28b_021_targets_string_value_is_rejected_as_type_mismatch() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = "native"

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("string targets must be rejected");
    assert!(matches!(
        err,
        AddonManifestError::AddonTargetsTypeMismatch { .. }
    ));
    let msg = err.to_string();
    assert!(
        msg.contains("must be an array of strings"),
        "diagnostic must explain the required shape, got: {msg}"
    );
    assert!(
        msg.contains("got string"),
        "diagnostic must echo the actual kind, got: {msg}"
    );
}

#[test]
fn d28b_021_targets_integer_value_is_rejected_as_type_mismatch() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = 42

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("integer targets must be rejected");
    assert!(matches!(
        err,
        AddonManifestError::AddonTargetsTypeMismatch { .. }
    ));
}

// ── Acceptance 6: array 内の non-string も reject ──

#[test]
fn d28b_021_array_with_non_string_element_is_syntax_error() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = [1, 2, 3]

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("non-string array element must be rejected");
    // Falls through to the array-element parser which reports a Syntax
    // error rather than UnknownAddonTarget.
    assert!(matches!(err, AddonManifestError::Syntax { .. }));
}

#[test]
fn d28b_021_array_unterminated_is_syntax_error() {
    let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["native"

[functions]
echo = 1
"#;
    let err = parse(src).expect_err("unterminated array must be rejected");
    assert!(matches!(err, AddonManifestError::Syntax { .. }));
}

// ── Acceptance 7: 例外メッセージ も bit-identical ──
//
// 欠落の場合は parse 成功 → 例外メッセージは存在しない。明示の場合
// も同様。bit-identical な失敗側として「両者が unknown target を
// 含むケース」を比較する: 欠落側で targets= が無いので「不正な
// targets」を再現できないため、本テストは「明示形式同士で同じ
// unknown を出した場合に文字列が安定」をピンする。

#[test]
fn d28b_021_unknown_target_message_is_deterministic() {
    let src1 = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/d28b021-sample"
library = "d28b021_sample"
targets = ["foo"]

[functions]
echo = 1
"#;
    let err1 = parse(src1).unwrap_err().to_string();
    let err2 = parse(src1).unwrap_err().to_string();
    assert_eq!(err1, err2, "diagnostic must be deterministic across runs");
    assert!(err1.starts_with("addon manifest error: [E2001]"));
}

// ── Acceptance 8: existing manifests without `targets` keep working ──

#[test]
fn d28b_021_existing_manifest_without_targets_continues_to_load() {
    // Regression guard: every manifest shipped before D28B-021
    // landed had no `targets` key. The compatibility contract pinned
    // in docs/reference/addon_manifest.md says these manifests must
    // continue to load with the same surface as before. The struct
    // gains a `targets` field but its value (`["native"]`) matches
    // the historical implicit behaviour.
    let manifest = parse(HEADER_OMITTED).expect("legacy-shape manifest must still parse");
    assert_eq!(manifest.abi, 1);
    assert_eq!(manifest.package, "taida-lang/d28b021-sample");
    assert_eq!(manifest.library, "d28b021_sample");
    assert_eq!(manifest.functions.len(), 1);
    assert_eq!(manifest.functions.get("echo"), Some(&1));
    // Field present — that is the explicit-inject part of the contract.
    assert_eq!(manifest.targets, default_addon_targets());
}
