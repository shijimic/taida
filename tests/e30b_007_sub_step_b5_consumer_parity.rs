//! E30B-007 sub-step B-5 — consumer parity tests for explicit
//! `Name <= RustAddon["fn"](arity <= N)` bindings.
//!
//! Verifies (Lock-G Sub-G5 verdict, 2026-04-28):
//!   1. doc-gen surfaces RustAddon bindings as **public functions**
//!      (not generic value assignments), regardless of doc-comment
//!      presence.
//!   2. `pkg::facade::classify_symbol_in_module` returns
//!      `SymbolKind::Function` for RustAddon bindings.
//!   3. `graph::ai_format::format_ai_json` includes the RustAddon
//!      bindings in its `functions[]` array (the public JSON contract
//!      consumed by `taida graph` and downstream introspection).
//!   4. `parser::Assignment::as_rust_addon_binding` parses the surface
//!      form (string-literal fn name + `arity <= IntLit` field).

use taida::doc::extract_docs;
use taida::graph::ai_format::format_ai_json;
use taida::parser::{Statement, parse};
use taida::pkg::facade::{SymbolKind, classify_symbol_in_module};

const FACADE_SOURCE: &str = concat!(
    "// Test facade with explicit RustAddon bindings.\n",
    "\n",
    "terminalSize <= RustAddon[\"terminalSize\"](arity <= 0)\n",
    "readKey      <= RustAddon[\"readKey\"](arity <= 0)\n",
    "isTerminal   <= RustAddon[\"isTerminal\"](arity <= 1)\n",
    "\n",
    "<<< @(terminalSize, readKey, isTerminal)\n",
);

#[test]
fn ast_helper_recognises_rust_addon_binding() {
    let (program, errors) = parse(FACADE_SOURCE);
    assert!(errors.is_empty(), "{:?}", errors);
    let mut found = 0;
    for stmt in &program.statements {
        if let Statement::Assignment(a) = stmt
            && let Some((fn_name, arity)) = a.as_rust_addon_binding()
        {
            assert_eq!(a.target, fn_name, "binding target must equal fn name");
            match fn_name.as_str() {
                "terminalSize" => assert_eq!(arity, 0),
                "readKey" => assert_eq!(arity, 0),
                "isTerminal" => assert_eq!(arity, 1),
                other => panic!("unexpected fn_name: {}", other),
            }
            found += 1;
        }
    }
    assert_eq!(found, 3, "expected 3 RustAddon bindings, got {}", found);
}

#[test]
fn doc_gen_surfaces_rust_addon_bindings_as_public_functions() {
    let (program, errors) = parse(FACADE_SOURCE);
    assert!(errors.is_empty(), "{:?}", errors);
    let module_doc = extract_docs(&program, "test_module");

    // The 3 RustAddon bindings should appear in the functions list,
    // NOT in the assignments list (Lock-G Sub-G5 verdict).
    let func_names: Vec<&str> = module_doc
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        func_names.contains(&"terminalSize"),
        "functions list must include terminalSize, got: {:?}",
        func_names
    );
    assert!(
        func_names.contains(&"readKey"),
        "functions list must include readKey, got: {:?}",
        func_names
    );
    assert!(
        func_names.contains(&"isTerminal"),
        "functions list must include isTerminal, got: {:?}",
        func_names
    );

    // None of the RustAddon bindings should be in `assignments` even
    // though AST-wise they are `Statement::Assignment`.
    let assign_names: Vec<&str> = module_doc
        .assignments
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    for name in ["terminalSize", "readKey", "isTerminal"] {
        assert!(
            !assign_names.contains(&name),
            "RustAddon binding '{}' must not appear in assignments list, got: {:?}",
            name,
            assign_names
        );
    }

    // The placeholder param count should equal the manifest arity.
    let is_terminal_doc = module_doc
        .functions
        .iter()
        .find(|f| f.name == "isTerminal")
        .expect("isTerminal must be in functions list");
    assert_eq!(
        is_terminal_doc.params.len(),
        1,
        "isTerminal arity = 1 → 1 placeholder param"
    );
}

#[test]
fn pkg_facade_classifies_rust_addon_binding_as_function() {
    let temp_dir = std::env::temp_dir().join("taida_e30b_007_b5_facade_classify");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    let facade_path = temp_dir.join("test.td");
    std::fs::write(&facade_path, FACADE_SOURCE).unwrap();

    for name in ["terminalSize", "readKey", "isTerminal"] {
        let kind = classify_symbol_in_module(&facade_path, name, None);
        assert_eq!(
            kind,
            Some(SymbolKind::Function),
            "{} must classify as Function, got: {:?}",
            name,
            kind
        );
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn graph_ai_format_lists_rust_addon_bindings_as_public_functions() {
    let (program, errors) = parse(FACADE_SOURCE);
    assert!(errors.is_empty(), "{:?}", errors);
    let json = format_ai_json(&program, "test.td");

    // The 3 RustAddon bindings must appear in the functions[] array of
    // the AI graph JSON. Their body_summary references RustAddon[...] so
    // downstream consumers can identify them as addon-backed.
    for name in ["terminalSize", "readKey", "isTerminal"] {
        assert!(
            json.contains(&format!("\"name\": \"{}\"", name)),
            "function entry for '{}' must appear in AI graph JSON; output was: {}",
            name,
            json
        );
    }

    assert!(
        json.contains("RustAddon[\\\"terminalSize\\\"](arity <= 0)"),
        "body_summary must reference RustAddon[...] for terminalSize; output: {}",
        json
    );
    assert!(
        json.contains("RustAddon[\\\"isTerminal\\\"](arity <= 1)"),
        "body_summary must reference RustAddon[...] for isTerminal; output: {}",
        json
    );
    assert!(
        json.contains("\"returns\": \"Unknown\""),
        "RustAddon function entries must not expose an empty returns field; output: {}",
        json
    );
    assert!(
        json.contains("{\"name\": \"_arg0\", \"type\": \"Unknown\"}"),
        "arity placeholders must carry an Unknown type marker; output: {}",
        json
    );
}

#[test]
fn e1413_diagnostic_documented() {
    // diagnostic_codes.md must list E1413 with a manual-fix hint.
    let ref_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("reference")
        .join("diagnostic_codes.md");
    let content = std::fs::read_to_string(&ref_path).expect("diagnostic_codes.md must be readable");
    assert!(
        content.contains("E1413"),
        "diagnostic_codes.md must document E1413 (legacy bare addon reference)"
    );
    assert!(
        content.contains("Sub-G4")
            || content.contains("legacy 暗黙 pre-inject")
            || content.contains("legacy bare reference")
            || content.contains("bare 参照"),
        "E1413 entry must reference Lock-G Sub-G4 / legacy bare reference"
    );
}
