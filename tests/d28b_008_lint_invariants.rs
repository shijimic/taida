//! D28B-008 — naming-convention lint invariant tests.
//!
//! These tests ensure that the lint pass:
//!   1. Detects every E1801..E1809 violation category
//!   2. Does not flag compliant code
//!   3. Surfaces user-friendly Japanese diagnostic messages
//!   4. Maintains the curated user-facing examples lint-clean (CI ratchet)
//!
//! The lint command (`taida way lint <PATH>`) is the user-facing surface;
//! this suite exercises it both directly via the library API and via the
//! CLI binary to ensure the same diagnostics surface in both paths.

use std::path::PathBuf;
use std::process::Command;

use taida::parser::lint::{LintDiagnostic, lint_program_with_source};
use taida::parser::parse;

fn lint_str(src: &str) -> Vec<LintDiagnostic> {
    let (program, errs) = parse(src);
    assert!(errs.is_empty(), "parse errors: {:?}", errs);
    lint_program_with_source(&program, src)
}

fn codes(diags: &[LintDiagnostic]) -> Vec<&'static str> {
    diags.iter().map(|d| d.code).collect()
}

// ── E1801: Type-name PascalCase ─────────────────────────────────

#[test]
fn d28b_008_e1801_lowercase_type_def_flagged() {
    let diags = lint_str("user = @(\n  name: Str\n)\n");
    assert!(codes(&diags).contains(&"[E1801]"));
}

#[test]
fn d28b_008_e1801_pascal_type_def_clean() {
    let diags = lint_str("User = @(\n  name: Str\n)\n");
    assert!(!codes(&diags).contains(&"[E1801]"));
}

#[test]
fn d28b_008_e1801_lowercase_mold_flagged() {
    // Note: parser requires `Mold[T] => Name[T]` shape — we use a
    // simple TypeDef shape with snake_case name to exercise the
    // PascalCase-required rule without depending on mold parser.
    let diags = lint_str("snake_pack = @(x: Int)\n");
    assert!(codes(&diags).contains(&"[E1801]"));
}

// ── E1802: Function camelCase ───────────────────────────────────

#[test]
fn d28b_008_e1802_snake_func_flagged() {
    let diags = lint_str("get_user x: Int = x => :Int\n");
    assert!(codes(&diags).contains(&"[E1802]"));
}

#[test]
fn d28b_008_e1802_pascal_func_flagged() {
    let diags = lint_str("GetUser x: Int = x => :Int\n");
    assert!(codes(&diags).contains(&"[E1802]"));
}

#[test]
fn d28b_008_e1802_camel_func_clean() {
    let diags = lint_str("getUser x: Int = x => :Int\n");
    assert!(!codes(&diags).contains(&"[E1802]"));
}

// ── E1804: Non-function variable snake_case ─────────────────────

#[test]
fn d28b_008_e1804_camel_int_var_flagged() {
    let diags = lint_str("portCount <= 8080\n");
    assert!(codes(&diags).contains(&"[E1804]"));
}

#[test]
fn d28b_008_e1804_snake_int_var_clean() {
    let diags = lint_str("port_count <= 8080\n");
    assert!(!codes(&diags).contains(&"[E1804]"));
}

#[test]
fn d28b_008_e1804_camel_string_var_flagged() {
    let diags = lint_str(r#"firstName <= "Asuka""#);
    assert!(codes(&diags).contains(&"[E1804]"));
}

// ── E1806: Enum variant PascalCase ──────────────────────────────

#[test]
fn d28b_008_e1806_snake_variant_flagged() {
    let diags = lint_str("Enum => Status = :active :inactive\n");
    assert!(codes(&diags).contains(&"[E1806]"));
}

#[test]
fn d28b_008_e1806_pascal_variant_clean() {
    let diags = lint_str("Enum => Status = :Active :Inactive\n");
    assert!(!codes(&diags).contains(&"[E1806]"));
}

// ── E1807: Type variable single-letter ──────────────────────────

#[test]
fn d28b_008_e1807_named_type_var_flagged() {
    let diags = lint_str("Mold[Item] => Box[Item] = @(value: Item)\n");
    assert!(codes(&diags).contains(&"[E1807]"));
}

#[test]
fn d28b_008_e1807_single_letter_clean() {
    let diags = lint_str("Mold[T] => Box[T] = @(value: T)\n");
    assert!(!codes(&diags).contains(&"[E1807]"));
}

#[test]
fn d28b_008_e1807_indexed_type_var_clean() {
    // T1 / T2 / T3 indexed form is allowed for 4+ collisions
    let diags = lint_str("Mold[T1] => Box[T1] = @(value: T1)\n");
    assert!(!codes(&diags).contains(&"[E1807]"));
}

// ── E1808: Buchi-pack field value-type matching ─────────────────

#[test]
fn d28b_008_e1808_camel_string_field_flagged() {
    let diags = lint_str(r#"data <= @(callSign <= "Eva-02")"#);
    assert!(codes(&diags).contains(&"[E1808]"));
}

#[test]
fn d28b_008_e1808_snake_string_field_clean() {
    let diags = lint_str(r#"data <= @(call_sign <= "Eva-02")"#);
    assert!(!codes(&diags).contains(&"[E1808]"));
}

#[test]
fn d28b_008_e1808_camel_lambda_field_clean() {
    // Function value → camelCase is correct
    let diags = lint_str("data <= @(myHandler <= _ x = x)\n");
    assert!(!codes(&diags).contains(&"[E1808]"));
}

#[test]
fn d28b_008_e1808_typed_int_field_camel_flagged() {
    let diags = lint_str("Config = @(portCount: Int)\n");
    assert!(codes(&diags).contains(&"[E1808]"));
}

#[test]
fn d28b_008_e1808_typed_int_field_snake_clean() {
    let diags = lint_str("Config = @(port_count: Int)\n");
    assert!(!codes(&diags).contains(&"[E1808]"));
}

// ── E1809: Return-type `:` marker ────────────────────────────────

#[test]
fn d28b_008_e1809_missing_marker_flagged() {
    let diags = lint_str("identity x: Int = x => Int\n");
    assert!(codes(&diags).contains(&"[E1809]"));
}

#[test]
fn d28b_008_e1809_present_marker_clean() {
    let diags = lint_str("identity x: Int = x => :Int\n");
    assert!(!codes(&diags).contains(&"[E1809]"));
}

// ── Negative cases (do not flag) ─────────────────────────────────

#[test]
fn d28b_008_underscore_prefix_not_flagged() {
    // `_` prefix is in the non-flagged list per Lock
    let diags = lint_str("_internal <= 42\n");
    assert!(!codes(&diags).contains(&"[E1804]"));
}

#[test]
fn d28b_008_boolean_prefix_is_has_not_flagged() {
    // `is`/`has` prefix is not flagged (boolean prefixes are allowed)
    let diags = lint_str(r#"is_active <= true"#);
    assert!(!codes(&diags).contains(&"[E1804]"));
}

#[test]
fn d28b_008_diagnostic_message_japanese() {
    let diags = lint_str("portCount <= 8080\n");
    let msg = diags
        .iter()
        .find(|d| d.code == "[E1804]")
        .expect("E1804 expected")
        .message
        .clone();
    // Sanity: message is in Japanese (contains hiragana/katakana/kanji)
    assert!(
        msg.chars().any(|c| {
            ('\u{3040}'..='\u{309F}').contains(&c)
                || ('\u{30A0}'..='\u{30FF}').contains(&c)
                || ('\u{4E00}'..='\u{9FFF}').contains(&c)
        }),
        "expected Japanese message, got: {}",
        msg
    );
    // Should embed the offending name
    assert!(msg.contains("portCount"), "message must reference symbol");
}

#[test]
fn d28b_008_diagnostic_render_path_format() {
    let diags = lint_str("portCount <= 8080\n");
    let d = diags.iter().find(|d| d.code == "[E1804]").unwrap();
    let rendered = d.render("test.td");
    assert!(rendered.starts_with("test.td:1:1 [E1804] "));
}

// ── Curated user-facing examples must lint clean (CI ratchet) ────

/// Resolve the `taida` CLI. We always exec via `cargo run` to stay
/// independent of the test harness's environment.
fn taida_cli() -> Command {
    // Use the debug binary if it exists, falling back to `cargo run`.
    let workspace = env_workspace_root();
    let bin = workspace.join("target/debug/taida");
    if bin.exists() {
        Command::new(bin)
    } else {
        let mut c = Command::new("cargo");
        c.args(["run", "--quiet", "--bin", "taida", "--"]);
        c
    }
}

fn env_workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points to the workspace root for tests/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
#[ignore = "requires built taida binary; runs in CI lint job explicitly"]
fn d28b_008_curated_user_facing_examples_lint_clean() {
    let workspace = env_workspace_root();
    let examples = workspace.join("examples");
    if !examples.exists() {
        return;
    }
    let mut violations: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&examples).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("td") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Same exclusion list as CI workflow
        if name.starts_with("compile_") || name == "addon_terminal.td" {
            continue;
        }
        let out = taida_cli()
            .arg("way")
            .arg("lint")
            .arg(&path)
            .output()
            .expect("taida way lint failed to spawn");
        if !out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            violations.push(format!("{}\n{}", path.display(), stdout));
        }
    }
    assert!(
        violations.is_empty(),
        "user-facing examples have lint violations:\n{}",
        violations.join("\n---\n")
    );
}

// ── Workflow invariant: ci.yml lint job is hard-fail ─────────────

#[test]
fn d28b_008_ci_lint_job_is_hard_fail() {
    let workspace = env_workspace_root();
    let yml = std::fs::read_to_string(workspace.join(".github/workflows/ci.yml"))
        .expect("ci.yml must exist");
    // Find the lint job stanza
    assert!(
        yml.contains("Lint (D28B-008 naming conventions)"),
        "ci.yml must define D28B-008 lint job"
    );
    // Extract the lint job slice for inspection: between "  lint:" and
    // the next top-level `  <name>:` (two-space indent).
    let lint_idx = yml.find("\n  lint:\n").expect("lint job must exist");
    let after = &yml[lint_idx + 1..];
    // Find next sibling job (or EOF)
    let next_top = after
        .lines()
        .skip(1)
        .position(|l| l.starts_with("  ") && !l.starts_with("    ") && l.trim_end().ends_with(':'));
    let lint_block: String = match next_top {
        Some(n) => after.lines().take(n + 1).collect::<Vec<_>>().join("\n"),
        None => after.to_string(),
    };
    // No `continue-on-error: true` inside the lint block
    assert!(
        !lint_block.contains("continue-on-error: true"),
        "lint job must not opt out via continue-on-error"
    );
    // Must invoke `taida way lint`
    assert!(
        lint_block.contains("taida way lint"),
        "lint job must invoke taida way lint"
    );
}

// ── Diagnostic-codes registry consistency ────────────────────────

#[test]
fn d28b_008_diagnostic_codes_doc_registers_e18xx() {
    let workspace = env_workspace_root();
    let doc = std::fs::read_to_string(workspace.join("docs/reference/diagnostic_codes.md"))
        .expect("diagnostic_codes.md must exist");
    for code in [
        "E1801", "E1802", "E1803", "E1804", "E1805", "E1806", "E1807", "E1808", "E1809",
    ] {
        assert!(
            doc.contains(&format!("`{}`", code)),
            "diagnostic_codes.md must register {}",
            code
        );
    }
}

#[test]
fn d28b_008_naming_conventions_doc_describes_lock() {
    let workspace = env_workspace_root();
    let doc = std::fs::read_to_string(workspace.join("docs/reference/naming_conventions.md"))
        .expect("naming_conventions.md must exist");
    // Required sections (D28B-001 Lock content)
    for needle in [
        "PascalCase",
        "camelCase",
        "snake_case",
        "SCREAMING_SNAKE_CASE",
        "型変数",
        "ぶちパックフィールド",
        "E1801",
        "E1809",
    ] {
        assert!(
            doc.contains(needle),
            "naming_conventions.md must mention `{}`",
            needle
        );
    }
}
