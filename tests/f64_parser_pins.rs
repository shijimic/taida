//! Regression coverage for source spelling, delimiters and parser recovery.
mod common;
use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::process::Command;
use taida::lexer::{TokenKind, tokenize};
use taida::parser::{Expr, Statement, parse};

#[test]
fn numeric_import_spelling_survives_bom_and_unicode() {
    for prefix in ["", "\u{feff}", "// 日本語\n", "\u{feff}// 日本語\n"] {
        for path in [
            "./2.0/mod.td",
            "./1.10/mod.td",
            "./001/mod.td",
            "./1_000/mod.td",
        ] {
            let (program, errors) = parse(&format!("{prefix}>>> {path}\n"));
            assert!(errors.is_empty(), "{errors:?}");
            let Statement::Import(import) = &program.statements[0] else {
                panic!("import")
            };
            assert_eq!(import.path, path);
        }
    }
    for version in ["1.10.0", "1.00.2"] {
        let (program, errors) = parse(&format!("\u{feff}>>> owner/pkg@{version}\n"));
        assert!(errors.is_empty(), "{errors:?}");
        let Statement::Import(import) = &program.statements[0] else {
            panic!("import")
        };
        assert_eq!(import.version.as_deref(), Some(version));
    }
}

#[test]
fn trailing_empty_slot_preserves_partial_application() {
    for src in ["f(1,)", "f(1,\n)"] {
        let (program, errors) = parse(src);
        assert!(errors.is_empty(), "{src}: {errors:?}");
        let Statement::Expr(Expr::FuncCall(_, args, _)) = &program.statements[0] else {
            panic!("call")
        };
        assert_eq!(args.len(), 2, "{src}");
        assert!(matches!(args[1], Expr::Hole(_)));
        assert!(matches!(args[0], Expr::IntLit(1, _)));
    }
    let (program, errors) = parse("f(, 2)");
    assert!(errors.is_empty());
    let Statement::Expr(Expr::FuncCall(_, args, _)) = &program.statements[0] else {
        panic!("call")
    };
    assert!(matches!(args[0], Expr::Hole(_)));
}

#[test]
fn mold_separator_error_restores_parser_context() {
    let (_, errors) = parse("x <= Unknown[1 2]()\ny <= Lax[3]\n");
    assert!(
        errors.iter().any(|e| e.message.contains("Expected ','")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("[E1546]")),
        "{errors:?}"
    );
    let (_, errors) = parse("x <= Unknown[1,\n  2,\n]()\n");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn comments_do_not_change_block_boundaries() {
    for source in [
        "f x: Int =\n    // introductory comment\n  x\n  // trailing comment\n=> :Int\nstdout(f(3))\n",
        "f <= _ x: Int =\n  x\n  // trailing comment\n=> :Int\nstdout(f(3))\n",
    ] {
        let (_, errors) = parse(source);
        assert!(errors.is_empty(), "{source}: {errors:?}");
    }
}

#[test]
fn crlf_comments_and_float_overflow_are_checked() {
    let (tokens, errors) = tokenize("\u{feff}///@ doc\r\n// comment\r\nx <= 1e999\n");
    assert!(
        errors.iter().any(|e| e.message.contains("overflows")),
        "{errors:?}"
    );
    for token in tokens {
        if let TokenKind::DocComment(s) | TokenKind::LineComment(s) = token.kind {
            assert!(!s.contains('\r'));
        }
    }
    assert!(tokenize("x <= 1e308").1.is_empty());
}

#[test]
fn deeply_nested_statements_report_a_diagnostic() {
    let source = (0..1000)
        .map(|i| format!("{}f{i} x: Int =\n", "  ".repeat(i)))
        .collect::<String>();
    let (_, errors) = parse(&source);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Maximum statement nesting depth")),
        "{errors:?}"
    );
}

#[test]
fn escaped_interpolation_stays_literal_across_backends() {
    let dir = unique_temp_dir("template_escape");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"x <= 7
stdout(`\${missing}`)
stdout(`a\${missing}b ${x}`)
stdout(`\\${x}`)
stdout(`\x24{missing}`)
"#,
    )
    .unwrap();
    let expected = "${missing}\na${missing}b 7\n\\7\n${missing}";
    assert_eq!(run_interpreter(&td).unwrap().trim_end(), expected);
    for profile in ["native", "wasm-min"] {
        if profile == "wasm-min" && wasmtime_bin().is_none() {
            continue;
        }
        let out = dir.join(format!("out-{profile}"));
        let build = Command::new(taida_bin())
            .args(["build", profile])
            .arg(&td)
            .arg("-o")
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = if profile == "native" {
            Command::new(&out).output()
        } else {
            Command::new(wasmtime_bin().unwrap()).arg(&out).output()
        }
        .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&run.stdout).trim_end(),
            expected,
            "{profile}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
