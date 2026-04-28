//! E30B-006 / Phase 7.5 — graph consumer parity for class-like 統一
//! (Lock-F 軸 2: `NodeKind::ClassLikeType` + `EdgeKind::ClassLikeInheritance`)。
//!
//! 旧 3 系統 (TypeDef / Mold 継承 / Error 継承) を graph layer で
//! `NodeKind::ClassLikeType` 単一 kind に統合した。本 test file は
//! structural_summary / ai_format 等の **公開 API 経由** で graph consumer の
//! parity を pin する。`graph::extract` / `graph::model` は `pub(crate)` のため
//! 直接 NodeKind を参照せず、JSON output contract で挙動を検証する。
//!
//! Acceptance:
//!  - structural_summary が旧 3 系統を NodeKind 統合後も正しく分類する
//!    (mold_types / error_types JSON フィールド維持)
//!  - ai_format JSON が class-like type kind 別 ("buchi_pack" / "mold" /
//!    "error" / "inheritance") を保持する (graph extract 統合と独立で
//!    contract が維持される)
//!  - 旧分類 metadata (`class_like_kind` / `inheritance_parent`) は
//!    structural_summary 側で正しく復元される

use taida::graph::ai_format::format_ai_json;
use taida::graph::verify::structural_summary;
use taida::parser::parse;

/// (E30B-006) BuchiPack kind (`Pilot = @(...)`) は structural_summary の
/// `mold_types` / `error_types` どちらにも入らない。types カウントには 1 加算
/// される。
#[test]
fn test_e30b_006_structural_summary_buchi_pack() {
    let source = "Pilot = @(name: Str, age: Int)\n";
    let (program, _) = parse(source);
    let summary = structural_summary(&program, "test.td");

    assert!(summary.contains("\"types\": 1"), "summary: {}", summary);
    assert!(
        summary.contains("\"mold_types\": []"),
        "summary: {}",
        summary
    );
    assert!(
        summary.contains("\"error_types\": []"),
        "summary: {}",
        summary
    );
}

/// (E30B-006) Mold kind (`Mold[T] => Box[T] = @(...)`) は structural_summary の
/// `mold_types` 配列に入る。`Mold[T]` base node は label exclusion で除外され、
/// `error_types` には入らない。
#[test]
fn test_e30b_006_structural_summary_mold_kind() {
    let source = "Mold[T] => Box[T] = @(value: T)\n";
    let (program, _) = parse(source);
    let summary = structural_summary(&program, "test.td");

    // Mold[T] base node + Box node の 2 nodes (両方 ClassLikeType)
    assert!(summary.contains("\"types\": 2"), "summary: {}", summary);
    // mold_types は子 ("Box") のみ含む — base node "Mold[T]" は除外
    assert!(
        summary.contains("\"mold_types\": [\"Box\"]"),
        "summary: {}",
        summary
    );
    assert!(
        summary.contains("\"error_types\": []"),
        "summary: {}",
        summary
    );
}

/// (E30B-006) Error 継承 (`Error => NotFound = @(...)`) は structural_summary
/// の `error_types` 配列に入る。Error base node は label exclusion で除外。
#[test]
fn test_e30b_006_structural_summary_error_inheritance() {
    let source = "Error => NotFound = @(msg: Str)\n";
    let (program, _) = parse(source);
    let summary = structural_summary(&program, "test.td");

    // Error base + NotFound child の 2 nodes
    assert!(summary.contains("\"types\": 2"), "summary: {}", summary);
    assert!(
        summary.contains("\"error_types\": [\"NotFound\"]"),
        "summary: {}",
        summary
    );
    assert!(
        summary.contains("\"mold_types\": []"),
        "summary: {}",
        summary
    );
}

/// (E30B-006) TypeDef inheritance (`Pilot => NervStaff = @(...)`) は
/// `mold_types` / `error_types` どちらにも入らない (Lock-F 軸 2: 親が Error
/// でない inheritance edge は `StructuralSubtype` で表現される)。
#[test]
fn test_e30b_006_structural_summary_typedef_inheritance_classification() {
    let source = "Pilot = @(name: Str)\nPilot => NervStaff = @(name: Str, role: Str)\n";
    let (program, _) = parse(source);
    let summary = structural_summary(&program, "test.td");

    // Pilot (BuchiPack) + Pilot (Inheritance 親 reference) + NervStaff (Inheritance) — 3 nodes
    // ただし same-id node は重複排除されるため、実際は Pilot 1 + NervStaff 1 = 2 nodes 期待
    // (BuchiPack と Inheritance kind は別 node として登録、type:Pilot 親 ref は inheritance 専用 id)
    assert!(
        summary.contains("\"mold_types\": []"),
        "summary: {}",
        summary
    );
    assert!(
        summary.contains("\"error_types\": []"),
        "summary: {}",
        summary
    );
}

/// (E30B-006) ai_format JSON は graph layer の NodeKind 統合と独立で旧 4
/// 分類 ("buchi_pack" / "mold" / "error" / "inheritance") を維持する
/// (PHILOSOPHY IV 構造的イントロスペクション契約継続)。
#[test]
fn test_e30b_006_ai_format_preserves_class_like_kind_strings() {
    let source = "\
Pilot = @(name: Str)
Mold[T] => Box[T] = @(value: T)
Error => NotFound = @(msg: Str)
Pilot => NervStaff = @(name: Str, role: Str)
";
    let (program, _) = parse(source);
    let json = format_ai_json(&program, "test.td");

    // ai_format は内部の `kind: &'static str` で 4 分類を出力する
    assert!(json.contains("\"kind\": \"buchi_pack\""), "json: {}", json);
    assert!(json.contains("\"kind\": \"mold\""), "json: {}", json);
    assert!(json.contains("\"kind\": \"error\""), "json: {}", json);
    assert!(json.contains("\"kind\": \"inheritance\""), "json: {}", json);
}

/// (E30B-006) structural_summary の JSON 構造 (`version` / `stats` / 各
/// section name) は class-like 統合前後で変わらないこと。
#[test]
fn test_e30b_006_structural_summary_json_contract_preserved() {
    let source = "Pilot = @(name: Str)\n";
    let (program, _) = parse(source);
    let summary = structural_summary(&program, "test.td");

    assert!(summary.contains("\"version\": \"1.0\""));
    assert!(summary.contains("\"stats\":"));
    assert!(summary.contains("\"functions\":"));
    assert!(summary.contains("\"types\":"));
    assert!(summary.contains("\"mold_types\":"));
    assert!(summary.contains("\"error_types\":"));
    assert!(summary.contains("\"type_hierarchy\":"));
    assert!(summary.contains("\"dataflow\":"));
    assert!(summary.contains("\"modules\":"));
    assert!(summary.contains("\"errors\":"));
}

/// (E30B-006) 複合 fixture (4 系統を同 source に置く) で types カウントが
/// 集計され、mold_types / error_types が正しく分類されることを pin する。
#[test]
fn test_e30b_006_structural_summary_aggregate_classification() {
    let source = "\
Pilot = @(name: Str)
Mold[T] => Box[T] = @(value: T)
Error => NotFound = @(msg: Str)
";
    let (program, _) = parse(source);
    let summary = structural_summary(&program, "test.td");

    // mold_types = ["Box"], error_types = ["NotFound"]
    // BuchiPack 単独 (Pilot) は mold_types / error_types に入らない
    assert!(
        summary.contains("\"mold_types\": [\"Box\"]"),
        "summary: {}",
        summary
    );
    assert!(
        summary.contains("\"error_types\": [\"NotFound\"]"),
        "summary: {}",
        summary
    );
}
