//! E30B-001 / Lock-E migration tool skeleton smoke test
//! (E30 Phase 2 Sub-step 2.3, 2026-04-28).
//!
//! Verifies the public scan API for `taida upgrade --e30`:
//!  - Detects legacy `Mold[T] => Foo[T] = @(...)` syntax via
//!    `ClassLikeDef::is_legacy_e30_syntax()`.
//!  - Returns proposed migration headers (`Mold[T] => Box[T]` -> `Box[T]`).
//!  - Ignores new-form class-likes (zero-arity sugar / generic type-def /
//!    Error inheritance / plain buchi pack), per Lock-B Sub-B1 / Sub-B2 verdicts.
//!
//! Phase 7 で in-place rewrite + 完全な fields reconstruction を追加予定。
//! 本 skeleton ではファイル書き換えは行わず、scan_source 経由の proposal
//! 出力のみを検証する。

use taida::upgrade_e30::{MigrationProposal, scan_source};

fn assert_legacy_mold(proposals: &[MigrationProposal], expected_legacy: &str, expected_new: &str) {
    assert_eq!(
        proposals.len(),
        1,
        "expected exactly one legacy detection, got {:?}",
        proposals
    );
    let p = &proposals[0];
    assert_eq!(p.legacy_kind, "mold");
    assert_eq!(p.legacy_header, expected_legacy);
    assert_eq!(p.proposed_header, expected_new);
}

#[test]
fn skeleton_detects_legacy_mold_with_single_type_param() {
    // 最も基本的な旧 Mold 構文
    let src = "Mold[T] => Box[T] = @(filling: T)\n";
    let proposals = scan_source(src, std::path::Path::new("test.td"));
    assert_legacy_mold(&proposals, "Mold[T] => Box[T]", "Box[T]");
    assert_eq!(proposals[0].line, 1);
}

#[test]
fn skeleton_detects_legacy_mold_with_multiple_type_params() {
    // 多型 mold: Result[T, P] 系
    let src = "Mold[T, P] => Result[T, P] = @(value: T, error: P)\n";
    let proposals = scan_source(src, std::path::Path::new("test.td"));
    assert_legacy_mold(&proposals, "Mold[T, P] => Result[T, P]", "Result[T, P]");
}

#[test]
fn skeleton_does_not_flag_new_class_like_forms() {
    // Sub-step 2.2 で受理される新構文は migration 対象外。
    // - zero-arity sugar `Pilot[] = @(...)` (Lock-B Sub-B1)
    // - generic type def `Box[T] = @(...)` (Sub-step 2.2)
    // - Error inheritance `Error => NotFound = @(...)` (Lock-B Sub-B2)
    // - plain buchi pack `Pilot = @(...)` (Lock-B Sub-B1)
    let src = "\
Pilot[] = @(name: Str)
Box[T] = @(filling: T)
Error => NotFound = @(msg: Str)
PlainPack = @(value: Int)
";
    let proposals = scan_source(src, std::path::Path::new("test.td"));
    assert!(
        proposals.is_empty(),
        "new-form class-likes must not be flagged, got {:?}",
        proposals
    );
}

#[test]
fn skeleton_check_mode_distinguishes_legacy_vs_clean() {
    // Mixed file: 1 legacy mold + 1 new-form class-like.
    let src = "\
Mold[T] => Old[T] = @(value: T)
NewBox[T] = @(filling: T)
";
    let proposals = scan_source(src, std::path::Path::new("test.td"));
    assert_eq!(proposals.len(), 1, "only the legacy mold should be flagged");
    assert_eq!(proposals[0].legacy_header, "Mold[T] => Old[T]");
    assert_eq!(proposals[0].proposed_header, "Old[T]");
}
