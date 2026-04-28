//! E30B-006 / Phase 7.5 — pkg facade consumer parity for class-like 統一
//! (Lock-F 軸 1: `Statement::ClassLikeDef` 単一 variant)。
//!
//! 旧挙動 (Phase 2 Sub-step 2.1) では `Mold` kind class-like を
//! `collect_defined_symbols` が defined symbol set に登録せず、
//! `classify_symbol_in_module` が `SymbolKind::TypeDef` に分類しなかった
//! (Native lowering の symbol kind 解決を Function fallback に落とす silent
//! bug)。E30B-006 で BuchiPack / Mold / Inheritance を統一して登録 + 分類
//! するよう修正した。本 test file は publicly accessible な
//! `pkg::facade::validate_facade` / `pkg::facade::classify_symbol_in_module` /
//! `pkg::facade::SymbolKind` で silent bug の解消を pin する。

use std::fs;

use taida::pkg::facade::{SymbolKind, classify_symbol_in_module, validate_facade};

fn make_test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_e30b_006_facade_{}", name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// (E30B-006) BuchiPack kind class-like (`Pilot = @(...)`) は
/// classify_symbol_in_module で `SymbolKind::TypeDef` を返す
/// (旧挙動と整合、regression guard)。
#[test]
fn test_e30b_006_facade_buchi_pack_classified_as_type_def() {
    let dir = make_test_dir("buchi_pack");
    let entry = dir.join("main.td");
    fs::write(&entry, "Pilot = @(name: Str)\n<<< @(Pilot)\n").unwrap();

    let kind = classify_symbol_in_module(&entry, "Pilot", None);
    assert_eq!(kind, Some(SymbolKind::TypeDef));
}

/// (E30B-006 silent bug 解消) Mold kind class-like (`Mold[T] => Box[T] = @(...)`)
/// は classify_symbol_in_module で `SymbolKind::TypeDef` を返す。
/// 旧挙動では Function fallback (Sub-step 2.1 で意図的に維持された silent
/// bug) に落ちていた。
#[test]
fn test_e30b_006_facade_mold_classified_as_type_def() {
    let dir = make_test_dir("mold");
    let entry = dir.join("main.td");
    fs::write(&entry, "Mold[T] => Box[T] = @(value: T)\n<<< @(Box)\n").unwrap();

    let kind = classify_symbol_in_module(&entry, "Box", None);
    assert_eq!(
        kind,
        Some(SymbolKind::TypeDef),
        "Mold kind class-like must classify as TypeDef (E30B-006 silent bug fix)"
    );
}

/// (E30B-006) Inheritance kind class-like (`Pilot => NervStaff = @(...)`) は
/// classify_symbol_in_module で `SymbolKind::TypeDef` を返す (旧挙動と整合)。
#[test]
fn test_e30b_006_facade_inheritance_classified_as_type_def() {
    let dir = make_test_dir("inheritance");
    let entry = dir.join("main.td");
    fs::write(
        &entry,
        "Pilot = @(name: Str)\nPilot => NervStaff = @(name: Str, role: Str)\n<<< @(NervStaff)\n",
    )
    .unwrap();

    let kind = classify_symbol_in_module(&entry, "NervStaff", None);
    assert_eq!(kind, Some(SymbolKind::TypeDef));
}

/// (E30B-006) Error 継承 (`Error => NotFound = @(...)`) は Inheritance kind の
/// 一種なので classify_symbol_in_module で `SymbolKind::TypeDef` を返す。
#[test]
fn test_e30b_006_facade_error_inheritance_classified_as_type_def() {
    let dir = make_test_dir("error_inheritance");
    let entry = dir.join("main.td");
    fs::write(&entry, "Error => NotFound = @(msg: Str)\n<<< @(NotFound)\n").unwrap();

    let kind = classify_symbol_in_module(&entry, "NotFound", None);
    assert_eq!(kind, Some(SymbolKind::TypeDef));
}

/// (E30B-006 silent bug 解消) Mold kind class-like を facade-export して
/// validate_facade 経由で参照したとき、ghost symbol violation が発生しない
/// (旧挙動では Mold kind を defined symbol に登録していなかったため
/// `FacadeViolation::GhostSymbol` が誤発火していた可能性がある)。
#[test]
fn test_e30b_006_facade_mold_kind_not_ghost_symbol() {
    let dir = make_test_dir("mold_facade_export");
    let entry = dir.join("main.td");
    // Box は Mold kind class-like として定義し、facade で公開する想定
    fs::write(&entry, "Mold[T] => Box[T] = @(value: T)\n<<< @(Box)\n").unwrap();

    let violations = validate_facade(&["Box".to_string()], &entry, &["Box".to_string()]);
    assert!(
        violations.is_empty(),
        "Mold kind class-like must be recognised as defined symbol (E30B-006 silent bug fix), got: {:?}",
        violations
    );
}

/// (E30B-006) FuncDef / Assignment の SymbolKind 分類は変更されないこと
/// (regression guard、Phase 2 Sub-step 2.1 で land した分類挙動を継承)。
#[test]
fn test_e30b_006_facade_function_value_classification_unchanged() {
    let dir = make_test_dir("function_value");
    let entry = dir.join("main.td");
    fs::write(
        &entry,
        "myFunc x =\n  x + 1\n=> :Int\nmyVal <= 42\n<<< @(myFunc, myVal)\n",
    )
    .unwrap();

    assert_eq!(
        classify_symbol_in_module(&entry, "myFunc", None),
        Some(SymbolKind::Function)
    );
    assert_eq!(
        classify_symbol_in_module(&entry, "myVal", None),
        Some(SymbolKind::Value)
    );
}

/// (E30B-006) 4 系統 (BuchiPack / Mold / Inheritance + Error 継承) を
/// 同じ source に置いた集約 fixture で、全てが SymbolKind::TypeDef として
/// 一貫分類されることを pin する (Lock-F 軸 1 統合の証跡)。
#[test]
fn test_e30b_006_facade_all_class_like_kinds_uniform_classification() {
    let dir = make_test_dir("aggregate");
    let entry = dir.join("main.td");
    fs::write(
        &entry,
        "\
Pilot = @(name: Str)
Mold[T] => Box[T] = @(value: T)
Error => NotFound = @(msg: Str)
Pilot => NervStaff = @(name: Str, role: Str)
<<< @(Pilot, Box, NotFound, NervStaff)
",
    )
    .unwrap();

    for sym in &["Pilot", "Box", "NotFound", "NervStaff"] {
        assert_eq!(
            classify_symbol_in_module(&entry, sym, None),
            Some(SymbolKind::TypeDef),
            "All class-like kinds must classify uniformly as TypeDef (E30B-006), failed at: {}",
            sym
        );
    }
}
