/// Convert Taida parse errors and type errors to LSP Diagnostics.
///
/// Provides:
/// - Parse errors (severity: Error)
/// - Type errors from TypeChecker (severity: Warning)
///   - Type mismatches in assignments
///   - Undefined variable warnings (when TypeChecker reports unknown types)
///   - Empty list literal without type annotation
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::parser::{ParseError, parse};
use crate::types::TypeChecker;

/// Result of analyzing a Taida source file.
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
}

/// Analyze source code and produce LSP diagnostics.
pub fn analyze(source: &str) -> AnalysisResult {
    let mut diagnostics = Vec::new();

    // Phase 1: Parse errors
    let (program, parse_errors) = parse(source);
    for err in &parse_errors {
        diagnostics.push(parse_error_to_diagnostic(err, source));
    }

    // Phase 2: Type errors (only if parse succeeded)
    if parse_errors.is_empty() {
        let mut checker = TypeChecker::new();
        checker.check_program(&program);
        for err in &checker.errors {
            diagnostics.push(type_error_to_diagnostic(err, source));
        }
    }

    AnalysisResult { diagnostics }
}

/// Convert a ParseError to an LSP Diagnostic with improved range calculation.
fn parse_error_to_diagnostic(err: &ParseError, source: &str) -> Diagnostic {
    let line = if err.span.line > 0 {
        err.span.line - 1
    } else {
        0
    };
    let col = if err.span.column > 0 {
        err.span.column - 1
    } else {
        0
    };

    let line_text = source.lines().nth(line).unwrap_or("");

    // Calculate end column (0-based char index) from the span's char range.
    let end_col_char = if err.span.end > err.span.start {
        // Span.start/end are char offsets (from Vec<char> indexing in the lexer).
        // Compute how many chars precede this line to get a line-relative offset.
        let line_start_char: usize = source
            .lines()
            .take(line)
            .map(|l| l.chars().count() + 1) // +1 for newline
            .sum();

        err.span.end.saturating_sub(line_start_char)
    } else {
        // Default: highlight the next few characters
        let remaining = line_text.chars().count().saturating_sub(col);
        col + remaining.clamp(1, 10)
    };

    // Convert 0-based char indices to 0-based UTF-16 offsets for LSP.
    let start_utf16 = super::utf16::char_index_to_utf16_offset(line_text, col);
    let end_utf16 = super::utf16::char_index_to_utf16_offset(line_text, end_col_char);

    Diagnostic {
        range: Range {
            start: Position {
                line: line as u32,
                character: start_utf16 as u32,
            },
            end: Position {
                line: line as u32,
                character: end_utf16 as u32,
            },
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("taida".to_string()),
        message: err.message.clone(),
        ..Default::default()
    }
}

/// Convert a TypeError to an LSP Diagnostic with improved range calculation.
fn type_error_to_diagnostic(err: &crate::types::TypeError, source: &str) -> Diagnostic {
    let line = if err.span.line > 0 {
        err.span.line - 1
    } else {
        0
    };
    let col = if err.span.column > 0 {
        err.span.column - 1
    } else {
        0
    };

    let line_text = source.lines().nth(line).unwrap_or("");

    // Calculate end column (0-based char index) from the span's char range.
    let end_col_char = if err.span.end > err.span.start {
        let line_start_char: usize = source
            .lines()
            .take(line)
            .map(|l| l.chars().count() + 1) // +1 for newline
            .sum();

        err.span.end.saturating_sub(line_start_char)
    } else {
        // Use the full line as the range for type errors
        line_text.chars().count()
    };

    // Determine severity based on the error message content
    let severity = if err.message.contains("Type mismatch")
        || err.message.contains("requires a type annotation")
    {
        DiagnosticSeverity::ERROR
    } else {
        DiagnosticSeverity::WARNING
    };

    // Convert 0-based char indices to 0-based UTF-16 offsets for LSP.
    let start_utf16 = super::utf16::char_index_to_utf16_offset(line_text, col);
    let end_utf16 = super::utf16::char_index_to_utf16_offset(line_text, end_col_char);

    Diagnostic {
        range: Range {
            start: Position {
                line: line as u32,
                character: start_utf16 as u32,
            },
            end: Position {
                line: line as u32,
                character: end_utf16 as u32,
            },
        },
        severity: Some(severity),
        source: Some("taida-typecheck".to_string()),
        message: err.message.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_valid_source() {
        let source = "x <= 42\nname <= \"hello\"";
        let result = analyze(source);
        assert!(
            result.diagnostics.is_empty(),
            "Valid source should produce no diagnostics"
        );
    }

    #[test]
    fn test_analyze_parse_error() {
        // Incomplete assignment should produce a parse error
        let source = "x <=";
        let result = analyze(source);
        // Parse errors should be present
        assert!(
            !result.diagnostics.is_empty(),
            "Incomplete assignment should produce diagnostics. Got: {:?}",
            result.diagnostics
        );
        assert_eq!(
            result.diagnostics[0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
    }

    #[test]
    fn test_analyze_type_error_empty_list() {
        let source = "items <= @[]";
        let result = analyze(source);
        // Empty list without type annotation should produce a type error
        let has_type_error = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("type annotation") || d.message.contains("@[]"));
        assert!(
            has_type_error,
            "Empty list literal should produce type annotation warning. Got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_diagnostic_source_tag() {
        let source = "x <= 42";
        let result = analyze(source);
        // No errors expected, but verify the function doesn't panic
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_parse_error_has_taida_source() {
        let source = ">>> ";
        let result = analyze(source);
        if !result.diagnostics.is_empty() {
            assert_eq!(
                result.diagnostics[0].source,
                Some("taida".to_string()),
                "Parse errors should have 'taida' source"
            );
        }
    }

    #[test]
    fn test_type_mismatch_detection() {
        let source = "x: Int <= \"hello\"";
        let result = analyze(source);
        let has_mismatch = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("Type mismatch"));
        assert!(
            has_mismatch,
            "Type mismatch should be detected. Got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    // ── RC-4b: diagnostics quality tests ──

    #[test]
    fn test_rc4b_valid_programs_no_false_positives() {
        // Valid programs should produce zero diagnostics
        let valid_programs = [
            "x <= 42",
            "name <= \"hello\"",
            "flag <= true",
            "pi <= 3.14",
            "items: @[Int] <= @[]",
            "Person = @(name: Str, age: Int)\np <= Person(name <= \"Alice\", age <= 30)",
            "add a b = a + b => :Int",
            "x <= 42\nstdout(x)",
        ];
        for (i, src) in valid_programs.iter().enumerate() {
            let result = analyze(src);
            assert!(
                result.diagnostics.is_empty(),
                "Program #{} should produce no diagnostics but got: {:?}",
                i,
                result
                    .diagnostics
                    .iter()
                    .map(|d| &d.message)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_rc4b_parse_error_invalid_syntax() {
        // Double operator should produce a parse error
        let source = "x <= <= 42";
        let result = analyze(source);
        assert!(
            !result.diagnostics.is_empty(),
            "Invalid syntax should produce diagnostics"
        );
        assert_eq!(
            result.diagnostics[0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
        assert_eq!(result.diagnostics[0].source, Some("taida".to_string()));
    }

    #[test]
    fn test_rc4b_parse_error_unclosed_buchi_pack() {
        let source = "p <= @(name <= \"Alice\"";
        let result = analyze(source);
        assert!(
            !result.diagnostics.is_empty(),
            "Unclosed BuchiPack should produce diagnostics"
        );
        assert_eq!(
            result.diagnostics[0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
    }

    #[test]
    fn test_rc4b_parse_error_unclosed_list() {
        let source = "items <= @[1, 2, 3";
        let result = analyze(source);
        assert!(
            !result.diagnostics.is_empty(),
            "Unclosed list should produce diagnostics"
        );
    }

    #[test]
    fn test_rc4b_type_error_severity_mismatch_is_error() {
        let source = "x: Int <= \"hello\"";
        let result = analyze(source);
        let mismatch_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("Type mismatch"));
        assert!(mismatch_diag.is_some(), "Should detect type mismatch");
        assert_eq!(
            mismatch_diag.unwrap().severity,
            Some(DiagnosticSeverity::ERROR),
            "Type mismatch should be ERROR severity"
        );
    }

    #[test]
    fn test_rc4b_type_error_source_tag() {
        let source = "x: Int <= \"hello\"";
        let result = analyze(source);
        for d in &result.diagnostics {
            if d.message.contains("Type mismatch") {
                assert_eq!(
                    d.source,
                    Some("taida-typecheck".to_string()),
                    "Type errors should have 'taida-typecheck' source"
                );
            }
        }
    }

    #[test]
    fn test_rc4b_type_errors_skipped_on_parse_error() {
        // When there are parse errors, type checking should be skipped
        let source = "x: Int <= \n y <= 42";
        let result = analyze(source);
        // All diagnostics should be parse errors, not type errors
        for d in &result.diagnostics {
            assert_eq!(
                d.source,
                Some("taida".to_string()),
                "On parse error, only parse error diagnostics should be present"
            );
        }
    }

    #[test]
    fn test_rc4b_diagnostic_line_numbers_zero_based() {
        // LSP positions are 0-based
        let source = "x <= 42\ny: Int <= \"hello\"";
        let result = analyze(source);
        let mismatch_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("Type mismatch"));
        if let Some(d) = mismatch_diag {
            assert_eq!(
                d.range.start.line, 1,
                "Second line should be line 1 (0-based)"
            );
        }
    }

    #[test]
    fn test_rc4b_multiple_errors_reported() {
        let source = "x: Int <= \"hello\"\ny: Str <= 42";
        let result = analyze(source);
        let mismatch_count = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("Type mismatch"))
            .count();
        assert!(
            mismatch_count >= 2,
            "Both type mismatches should be reported, got {}",
            mismatch_count
        );
    }

    #[test]
    fn test_rc4b_empty_source_no_panic() {
        let source = "";
        let result = analyze(source);
        // Should not panic, may or may not produce diagnostics
        let _ = result.diagnostics.len();
    }

    #[test]
    fn test_rc4b_multiline_function_no_false_positive() {
        let source = "add a b =\n  a + b\n=> :Int";
        let result = analyze(source);
        assert!(
            result.diagnostics.is_empty(),
            "Multi-line function should produce no diagnostics. Got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    // ── RCB-318: Additional LSP integration tests ──

    #[test]
    fn test_rcb318_deep_nesting_no_crash() {
        // SEC-002/RCB-301: deep nesting should produce a parse error, not crash.
        // Parser has MAX_PARSE_DEPTH=256; we test just above the limit.
        // Run in a thread with larger stack to avoid stack overflow in the
        // recursive descent parser before the depth counter catches it.
        let handle = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024) // 8 MB stack
            .spawn(|| {
                let source =
                    "x <= ".to_string() + &"(@(y <= ".repeat(300) + "1" + &"))".repeat(300);
                let result = analyze(&source);
                // Should produce a parse error about nesting depth, not crash
                assert!(
                    !result.diagnostics.is_empty(),
                    "Deep nesting should produce diagnostics"
                );
                let has_depth_error = result
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("nesting depth"));
                assert!(
                    has_depth_error,
                    "Should report nesting depth error. Got: {:?}",
                    result
                        .diagnostics
                        .iter()
                        .map(|d| &d.message)
                        .collect::<Vec<_>>()
                );
            })
            .expect("Failed to spawn test thread");
        handle.join().expect("Test thread panicked");
    }

    #[test]
    fn test_rcb318_error_ceiling_diagnostics() {
        let source = "handler x =\n  |== e: Error = e.message\n  => :Str\n  x.toString()\n=> :Str\nstdout(handler(1))";
        let result = analyze(source);
        // Should not produce false parse errors for error ceiling syntax
        let parse_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.source == Some("taida".to_string()))
            .collect();
        assert!(
            parse_errors.is_empty(),
            "Error ceiling syntax should not produce parse errors. Got: {:?}",
            parse_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_rcb318_inheritance_def_diagnostics() {
        let source = "Animal = @(species: Str)\nAnimal => Dog = @(breed: Str)\nd <= Dog(species <= \"Canine\", breed <= \"Shiba\")";
        let result = analyze(source);
        let parse_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.source == Some("taida".to_string()))
            .collect();
        assert!(
            parse_errors.is_empty(),
            "Inheritance def should not produce parse errors. Got: {:?}",
            parse_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_rcb318_import_export_diagnostics() {
        let source = ">>> ./lib.td => @(helper)\nresult <= helper(42)\nstdout(result)";
        let result = analyze(source);
        // Import statements should parse without error (file resolution is runtime)
        let parse_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.source == Some("taida".to_string()))
            .collect();
        assert!(
            parse_errors.is_empty(),
            "Import syntax should not produce parse errors. Got: {:?}",
            parse_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_rcb318_empty_export_error() {
        let source = "x <= 42\n<<< @()";
        let result = analyze(source);
        // RCB-102: Empty export should produce a type error
        let has_empty_export_error = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("Empty export"));
        assert!(
            has_empty_export_error,
            "Empty export <<< @() should produce diagnostic. Got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_rcb318_large_source_no_timeout() {
        // Ensure LSP can handle moderately large files without hanging
        let mut source = String::new();
        for i in 0..200 {
            source.push_str(&format!("x_{} <= {}\n", i, i));
        }
        let result = analyze(&source);
        // Should complete without timing out
        assert!(
            result.diagnostics.is_empty(),
            "Large valid source should produce no errors"
        );
    }

    // ── RC-4f: integration test — all examples/*.td produce no parse errors ──

    #[test]
    fn test_rc4f_all_examples_no_parse_errors() {
        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let mut failures = Vec::new();

        for entry in std::fs::read_dir(&examples_dir).expect("examples dir should exist") {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "td") {
                continue;
            }
            // Skip module import files (they require their dependency files)
            // Skip addon example files (they require native addon runtime)
            let fname = path.file_name().unwrap().to_str().unwrap();
            if fname.starts_with("module_") || fname.starts_with("addon_") {
                continue;
            }

            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("Failed to read {}", path.display()));
            let result = analyze(&source);

            // Collect parse errors only (type warnings are acceptable)
            let parse_errors: Vec<_> = result
                .diagnostics
                .iter()
                .filter(|d| d.source == Some("taida".to_string()))
                .collect();

            if !parse_errors.is_empty() {
                failures.push(format!(
                    "{}: {} parse error(s): {}",
                    fname,
                    parse_errors.len(),
                    parse_errors
                        .iter()
                        .map(|d| d.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "The following example files produced parse errors in LSP diagnostics:\n{}",
            failures.join("\n")
        );
    }

    // ── RCB-54: UTF-16 diagnostic position regression tests ──

    #[test]
    fn test_rcb54_diagnostic_positions_with_japanese() {
        // Type mismatch on a line that starts with Japanese characters.
        let source = "\u{540D}\u{524D}: Int <= \"hello\"";
        let result = analyze(source);
        let mismatch = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("Type mismatch"));
        assert!(
            mismatch.is_some(),
            "Should detect type mismatch with Japanese variable name"
        );
        let d = mismatch.unwrap();
        // Span.column for the start is 1 (1-based), so 0-based char index = 0,
        // which maps to UTF-16 offset 0.
        assert_eq!(
            d.range.start.character, 0,
            "Start character should be UTF-16 offset 0, got {}",
            d.range.start.character
        );
    }

    #[test]
    fn test_rcb54_diagnostic_positions_with_emoji() {
        // Emoji in a string literal, then type mismatch on line 2.
        let source = "x <= \"a\u{1F600}b\"\ny: Int <= \"oops\"";
        let result = analyze(source);
        let mismatch = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("Type mismatch"));
        assert!(
            mismatch.is_some(),
            "Should detect type mismatch after emoji line"
        );
        let d = mismatch.unwrap();
        assert_eq!(
            d.range.start.line, 1,
            "Mismatch should be on line 1 (0-based)"
        );
    }

    #[test]
    fn test_rcb54_valid_japanese_program_no_diagnostics() {
        let source = "msg <= \"\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}\"\nstdout(msg)";
        let result = analyze(source);
        assert!(
            result.diagnostics.is_empty(),
            "Valid Japanese program should produce no diagnostics. Got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }
}
