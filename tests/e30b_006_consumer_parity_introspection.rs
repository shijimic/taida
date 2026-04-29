//! E30B-006 / Phase 7.5 — introspection (LSP / docs gen / structural verify)
//! consumer parity for class-like 統一 (Lock-F 軸 1, 2)。
//!
//! 旧 3 系統 (TypeDef / Mold 継承 / Error 継承) を `Statement::ClassLikeDef`
//! 単一 variant + `NodeKind::ClassLikeType` 単一 kind に統合した。本 test file
//! は LSP hover / docs gen / structural verify を **公開 API 経由** で
//! consumer parity を pin する。
//!
//! Acceptance:
//!
//! - docs gen (`taida::doc::extract_docs`) が 4 系統 (BuchiPack /
//!   Mold / Inheritance / Error 継承) を旧 docs 構造 (`types` /
//!   `molds` / `inheritances`) に正しく振り分ける
//! - run_all_checks (LSP / `taida way verify` 経路) が class-like 統合後も
//!   type-consistency / cycle 検査を成立させる (NodeKind 統合で false
//!   positive / false negative なし)
//!
//! 注: LSP hover / completion 系の inline test は `src/lsp/hover.rs` /
//! `src/lsp/completion.rs` 内に既存し、`Statement::ClassLikeDef + kind
//! dispatch` 経路はそこで pin 済。本 integration test は LSP feature flag
//! 非依存で動くよう、`extract_docs` (docs gen) と `run_all_checks` (verify
//! CLI 経路) を主軸にする。

use taida::doc::extract_docs;
use taida::graph::verify::run_all_checks;
use taida::parser::parse;

/// (E30B-006) docs gen の `extract_docs` が 4 系統を旧 docs 構造に
/// 振り分ける。BuchiPack → `types`、Mold → `molds`、Inheritance (Error 親
/// 含む) → `inheritances` (`extract_docs` の旧 API contract 維持)。
#[test]
fn test_e30b_006_doc_gen_buchi_pack_into_types() {
    let source = "Pilot = @(name: Str, age: Int)\n";
    let (program, _) = parse(source);
    let doc = extract_docs(&program, "test");

    assert_eq!(doc.types.len(), 1);
    assert_eq!(doc.types[0].name, "Pilot");
    assert!(doc.molds.is_empty());
    assert!(doc.inheritances.is_empty());
}

#[test]
fn test_e30b_006_doc_gen_mold_into_molds() {
    let source = "Mold[T] => Box[T] = @(value: T)\n";
    let (program, _) = parse(source);
    let doc = extract_docs(&program, "test");

    assert!(doc.types.is_empty());
    assert_eq!(doc.molds.len(), 1);
    assert_eq!(doc.molds[0].name, "Box");
    assert!(doc.inheritances.is_empty());
}

#[test]
fn test_e30b_006_doc_gen_error_inheritance_into_inheritances() {
    let source = "Error => NotFound = @(msg: Str)\n";
    let (program, _) = parse(source);
    let doc = extract_docs(&program, "test");

    assert!(doc.types.is_empty());
    assert!(doc.molds.is_empty());
    assert_eq!(doc.inheritances.len(), 1);
    assert_eq!(doc.inheritances[0].child, "NotFound");
    assert_eq!(doc.inheritances[0].parent, "Error");
}

#[test]
fn test_e30b_006_doc_gen_typedef_inheritance_into_inheritances() {
    let source = "Pilot = @(name: Str)\nPilot => NervStaff = @(name: Str, role: Str)\n";
    let (program, _) = parse(source);
    let doc = extract_docs(&program, "test");

    // Pilot is BuchiPack → types
    assert_eq!(doc.types.len(), 1);
    assert_eq!(doc.types[0].name, "Pilot");
    // NervStaff is Inheritance → inheritances
    assert_eq!(doc.inheritances.len(), 1);
    assert_eq!(doc.inheritances[0].child, "NervStaff");
    assert_eq!(doc.inheritances[0].parent, "Pilot");
}

/// (E30B-006) `taida way verify` の structural-summary は class-like 統合後も
/// type-consistency check を成立させる (NodeKind 統合で graph cycle 検出が
/// 壊れていないこと)。
#[test]
fn test_e30b_006_verify_type_consistency_no_false_positive() {
    let source = "\
Pilot = @(name: Str)
Mold[T] => Box[T] = @(value: T)
Error => NotFound = @(msg: Str)
Pilot => NervStaff = @(name: Str, role: Str)
";
    let (program, _) = parse(source);
    let report = run_all_checks(&program, "test.td");

    // 上記 fixture は意味的に正常 — type-consistency error は出ない
    let type_consistency_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.check == "type-consistency")
        .collect();
    assert!(
        type_consistency_findings.is_empty(),
        "type-consistency check should not produce false positives, got: {:?}",
        type_consistency_findings
    );
}

/// (E30B-006) 4 系統を集約した fixture で extract_docs が全部の class-like
/// を取りこぼさず分類することを pin する (Lock-F 軸 1 統合の集約証跡)。
#[test]
fn test_e30b_006_doc_gen_aggregate_class_like_kinds() {
    let source = "\
Pilot = @(name: Str)
Mold[T] => Box[T] = @(value: T)
Error => NotFound = @(msg: Str)
Pilot => NervStaff = @(name: Str, role: Str)
";
    let (program, _) = parse(source);
    let doc = extract_docs(&program, "test");

    // BuchiPack: Pilot → types
    assert_eq!(doc.types.len(), 1, "expected 1 type, got {:?}", doc.types);
    assert_eq!(doc.types[0].name, "Pilot");

    // Mold: Box → molds
    assert_eq!(doc.molds.len(), 1, "expected 1 mold, got {:?}", doc.molds);
    assert_eq!(doc.molds[0].name, "Box");

    // Inheritance: NotFound (Error 親) + NervStaff (Pilot 親) → inheritances
    assert_eq!(
        doc.inheritances.len(),
        2,
        "expected 2 inheritances, got {:?}",
        doc.inheritances
    );
    let names: Vec<&str> = doc.inheritances.iter().map(|i| i.child.as_str()).collect();
    assert!(names.contains(&"NotFound"), "names: {:?}", names);
    assert!(names.contains(&"NervStaff"), "names: {:?}", names);
}
