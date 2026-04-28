//! Centralized package facade validation.
//!
//! B11B-023: Extracted from checker / JS / Native to eliminate 3-way duplication.
//! B11B-022: Ghost symbol check includes Import-sourced (re-exported) symbols.
//! B11B-024: All violations are collected (not just the first).
//!
//! Each compile-time path (checker / JS codegen / Native lowering) calls
//! `validate_facade()` and converts the returned `FacadeViolation`s into its
//! own error type.

use std::collections::HashSet;
use std::path::Path;

use crate::parser::{self, Statement};
use crate::pkg::manifest::Manifest;

/// A single facade validation failure.
#[derive(Debug, Clone, PartialEq)]
pub enum FacadeViolation {
    /// Symbol is not listed in `packages.tdm` facade exports.
    HiddenSymbol {
        name: String,
        available: Vec<String>,
    },
    /// Symbol is listed in `packages.tdm` but not defined/imported in the entry module.
    GhostSymbol { name: String },
}

/// Outcome of facade resolution for a package root import.
pub struct FacadeContext {
    /// The facade export list from `packages.tdm` (non-empty).
    pub facade_exports: Vec<String>,
    /// Path to the entry module `.td` file.
    pub entry_path: std::path::PathBuf,
}

/// Attempt to resolve facade context for a package root import.
///
/// Returns `Some(FacadeContext)` if the package has a `packages.tdm` with
/// non-empty `exports`. Returns `None` for submodule imports, packages
/// without facade, or resolution failures.
pub fn resolve_facade_context(pkg_dir: &Path) -> Option<FacadeContext> {
    let manifest = match Manifest::from_dir(pkg_dir) {
        Ok(Some(m)) => m,
        _ => return None,
    };

    if manifest.exports.is_empty() {
        return None;
    }

    let entry_name = &manifest.entry;
    let entry_path = if let Some(stripped) = entry_name.strip_prefix("./") {
        pkg_dir.join(stripped)
    } else {
        pkg_dir.join(entry_name)
    };

    if !entry_path.exists() {
        return None;
    }

    Some(FacadeContext {
        facade_exports: manifest.exports,
        entry_path,
    })
}

/// Validate imported symbols against a package facade.
///
/// Performs two checks:
/// 1. **Membership**: each symbol must be listed in `facade_exports`.
/// 2. **Ghost**: each facade-matching symbol must actually be defined or
///    imported in the entry module (not a phantom declaration).
///
/// Returns all violations found (not just the first).
pub fn validate_facade(
    facade_exports: &[String],
    entry_path: &Path,
    imported_symbols: &[String],
) -> Vec<FacadeViolation> {
    let mut violations = Vec::new();

    // Step 1: Membership check
    for sym in imported_symbols {
        if !facade_exports.contains(sym) {
            violations.push(FacadeViolation::HiddenSymbol {
                name: sym.clone(),
                available: facade_exports.to_vec(),
            });
        }
    }

    // Step 2: Ghost check — verify facade-declared symbols exist in the entry module.
    // Only check symbols that passed the membership test.
    let source = match std::fs::read_to_string(entry_path) {
        Ok(s) => s,
        Err(_) => return violations,
    };
    let (program, _) = parser::parse(&source);

    let defined_symbols = collect_defined_symbols(&program.statements);

    for sym in imported_symbols {
        if facade_exports.contains(sym) && !defined_symbols.contains(sym.as_str()) {
            violations.push(FacadeViolation::GhostSymbol { name: sym.clone() });
        }
    }

    violations
}

/// Collect all symbols that are "available" in a module's top-level scope.
///
/// This includes:
/// - Local definitions: FuncDef, Assignment, TypeDef, InheritanceDef, EnumDef
/// - Imported symbols: `>>> ./other.td => @(sym)` makes `sym` available
///
/// B11B-022: Import-sourced symbols are included so that re-exports are not
/// falsely flagged as ghost symbols.
fn collect_defined_symbols(statements: &[Statement]) -> HashSet<String> {
    let mut defined = HashSet::new();
    for stmt in statements {
        match stmt {
            Statement::FuncDef(f) => {
                defined.insert(f.name.clone());
            }
            Statement::Assignment(a) => {
                defined.insert(a.target.clone());
            }
            // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
            // 注: 旧コードでは MoldDef は defined に入れず Native lowering の symbol kind 解決を
            // Function fallback に落としていた (silent bug、E30B-006 で解消予定)。
            // 本 Sub-step 2.1 では旧挙動維持: BuchiPack / Inheritance のみ defined に登録。
            Statement::ClassLikeDef(cl) => match &cl.kind {
                crate::parser::ClassLikeKind::BuchiPack
                | crate::parser::ClassLikeKind::Inheritance { .. } => {
                    defined.insert(cl.name.clone());
                }
                crate::parser::ClassLikeKind::Mold { .. } => {
                    // 旧挙動: MoldDef は defined に入れない (E30B-006 で再検討)
                }
            },
            Statement::EnumDef(e) => {
                defined.insert(e.name.clone());
            }
            // B11B-022: Import statements bring symbols into module scope.
            // If entry module does `>>> ./helper.td => @(sym)`, `sym` is
            // available for re-export.
            Statement::Import(imp) => {
                for sym in &imp.symbols {
                    let local_name = sym.alias.as_deref().unwrap_or(&sym.name);
                    defined.insert(local_name.to_string());
                }
            }
            _ => {}
        }
    }
    defined
}

/// Classify an imported symbol's kind by scanning the entry module's AST.
///
/// Used by the Native lowering path which needs to know whether a symbol
/// is a Function, TypeDef, or Value.
///
/// For re-exported symbols (imported into the entry module), we trace
/// into the source module to determine the kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SymbolKind {
    Function,
    TypeDef,
    Value,
}

/// Determine the kind of a symbol defined in a module's top-level scope.
///
/// For locally defined symbols, the kind is determined from the AST node.
/// For imported (re-exported) symbols, we recursively resolve the source
/// module to determine the kind.
///
/// Returns `None` if the symbol is not found.
pub fn classify_symbol_in_module(
    entry_path: &Path,
    symbol_name: &str,
    source_override: Option<&str>,
) -> Option<SymbolKind> {
    let owned_source;
    let source = match source_override {
        Some(s) => s,
        None => {
            owned_source = std::fs::read_to_string(entry_path).ok()?;
            &owned_source
        }
    };
    let (program, _) = parser::parse(source);

    // Check local definitions first
    for stmt in &program.statements {
        match stmt {
            Statement::FuncDef(f) if f.name == symbol_name => {
                return Some(SymbolKind::Function);
            }
            // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧挙動維持: BuchiPack / Inheritance のみ TypeDef SymbolKind)
            Statement::ClassLikeDef(cl)
                if cl.name == symbol_name && (cl.is_buchi_pack() || cl.is_inheritance()) =>
            {
                return Some(SymbolKind::TypeDef);
            }
            Statement::Assignment(a) if a.target == symbol_name => {
                return Some(SymbolKind::Value);
            }
            // Note: EnumDef is intentionally not handled here.
            // In the native backend, imported enums are treated as Functions
            // (the default fallback), which matches existing behavior.
            _ => {}
        }
    }

    // B11B-022: Check imported symbols (for re-export tracing)
    for stmt in &program.statements {
        if let Statement::Import(imp) = stmt {
            for sym in &imp.symbols {
                let local_name = sym.alias.as_deref().unwrap_or(&sym.name);
                if local_name == symbol_name {
                    // This symbol is imported from another module.
                    // Try to resolve the source module to determine kind.
                    let source_module_path = resolve_import_relative(entry_path, &imp.path);
                    if let Some(ref src_path) = source_module_path {
                        // Use the original name (not alias) in the source module
                        if let Some(kind) = classify_symbol_in_module(src_path, &sym.name, None) {
                            return Some(kind);
                        }
                    }
                    // Cannot resolve source — default to Function (safe fallback)
                    return Some(SymbolKind::Function);
                }
            }
        }
    }

    None
}

/// Resolve a relative import path from a source file.
fn resolve_import_relative(source_file: &Path, import_path: &str) -> Option<std::path::PathBuf> {
    if import_path.starts_with("./") || import_path.starts_with("../") {
        let source_dir = source_file.parent()?;
        let candidate = source_dir.join(import_path);
        if candidate.exists() {
            Some(candidate)
        } else {
            // Try with .td extension
            let with_ext = source_dir.join(format!("{}.td", import_path.trim_end_matches(".td")));
            if with_ext.exists() {
                Some(with_ext)
            } else {
                None
            }
        }
    } else {
        // Package imports — we don't trace into other packages for re-export classification
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("taida_facade_test_{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_facade_hidden_symbol() {
        let dir = test_dir("hidden");
        let entry = dir.join("main.td");
        fs::write(&entry, "public <= 1\n<<< @(public)\n").unwrap();

        let violations = validate_facade(&["public".to_string()], &entry, &["secret".to_string()]);

        assert_eq!(violations.len(), 1);
        match &violations[0] {
            FacadeViolation::HiddenSymbol { name, available } => {
                assert_eq!(name, "secret");
                assert_eq!(available, &["public".to_string()]);
            }
            other => panic!("Expected HiddenSymbol, got {:?}", other),
        }
    }

    #[test]
    fn test_facade_ghost_symbol() {
        let dir = test_dir("ghost");
        let entry = dir.join("main.td");
        fs::write(&entry, "public <= 1\n<<< @(public)\n").unwrap();

        let violations = validate_facade(
            &["public".to_string(), "ghost".to_string()],
            &entry,
            &["ghost".to_string()],
        );

        assert_eq!(violations.len(), 1);
        match &violations[0] {
            FacadeViolation::GhostSymbol { name } => {
                assert_eq!(name, "ghost");
            }
            other => panic!("Expected GhostSymbol, got {:?}", other),
        }
    }

    #[test]
    fn test_facade_reexport_accepted() {
        let dir = test_dir("reexport");

        // helper.td defines the symbol
        let helper = dir.join("helper.td");
        fs::write(&helper, "reExported <= 42\n<<< @(reExported)\n").unwrap();

        // main.td imports and re-exports
        let entry = dir.join("main.td");
        fs::write(
            &entry,
            ">>> ./helper.td => @(reExported)\n<<< @(reExported)\n",
        )
        .unwrap();

        let violations = validate_facade(
            &["reExported".to_string()],
            &entry,
            &["reExported".to_string()],
        );

        assert!(
            violations.is_empty(),
            "Re-exported symbol should not produce violations, got: {:?}",
            violations
        );
    }

    #[test]
    fn test_facade_all_errors_collected() {
        let dir = test_dir("all_errors");
        let entry = dir.join("main.td");
        fs::write(&entry, "public <= 1\n<<< @(public)\n").unwrap();

        let violations = validate_facade(
            &["public".to_string()],
            &entry,
            &["hidden1".to_string(), "hidden2".to_string()],
        );

        // Both hidden symbols should be reported (B11B-024)
        assert_eq!(violations.len(), 2);
        assert!(
            violations
                .iter()
                .all(|v| matches!(v, FacadeViolation::HiddenSymbol { .. }))
        );
    }

    #[test]
    fn test_classify_local_function() {
        let dir = test_dir("classify_func");
        let entry = dir.join("main.td");
        fs::write(&entry, "myFunc x: Int =\n  x + 1\n=> :Int\n").unwrap();

        let kind = classify_symbol_in_module(&entry, "myFunc", None);
        assert_eq!(kind, Some(SymbolKind::Function));
    }

    #[test]
    fn test_classify_local_value() {
        let dir = test_dir("classify_val");
        let entry = dir.join("main.td");
        fs::write(&entry, "myVal <= 42\n").unwrap();

        let kind = classify_symbol_in_module(&entry, "myVal", None);
        assert_eq!(kind, Some(SymbolKind::Value));
    }

    #[test]
    fn test_classify_reexported_value() {
        let dir = test_dir("classify_reval");

        let helper = dir.join("helper.td");
        fs::write(&helper, "reVal <= 42\n<<< @(reVal)\n").unwrap();

        let entry = dir.join("main.td");
        fs::write(&entry, ">>> ./helper.td => @(reVal)\n<<< @(reVal)\n").unwrap();

        let kind = classify_symbol_in_module(&entry, "reVal", None);
        assert_eq!(kind, Some(SymbolKind::Value));
    }

    #[test]
    fn test_classify_reexported_function() {
        let dir = test_dir("classify_refunc");

        let helper = dir.join("helper.td");
        fs::write(
            &helper,
            "myFunc x: Int =\n  x + 1\n=> :Int\n<<< @(myFunc)\n",
        )
        .unwrap();

        let entry = dir.join("main.td");
        fs::write(&entry, ">>> ./helper.td => @(myFunc)\n<<< @(myFunc)\n").unwrap();

        let kind = classify_symbol_in_module(&entry, "myFunc", None);
        assert_eq!(kind, Some(SymbolKind::Function));
    }
}
