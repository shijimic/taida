//! Structural verification checks for Taida code.

use super::escape_json;
use super::extract::GraphExtractor;
use super::model::*;
use super::query;
use super::tail_pos;
use crate::diagnostics::split_diag_code_and_hint;
use crate::module_graph;
use crate::parser::*;
use serde_json::json;
use std::path::Path;

/// Verification result severity.
#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Warning => write!(f, "WARN"),
            Severity::Error => write!(f, "ERROR"),
        }
    }
}

/// A single verification finding.
#[derive(Debug, Clone)]
pub struct VerifyFinding {
    pub check: String,
    pub severity: Severity,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
}

impl std::fmt::Display for VerifyFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location = match (&self.file, &self.line) {
            (Some(file), Some(line)) => format!(" ({}:{})", file, line),
            (Some(file), None) => format!(" ({})", file),
            _ => String::new(),
        };
        write!(
            f,
            "[{}] {}: {}{}",
            self.severity, self.check, self.message, location
        )
    }
}

/// Overall verification result.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub findings: Vec<VerifyFinding>,
}

impl Default for VerifyReport {
    fn default() -> Self {
        Self::new()
    }
}

impl VerifyReport {
    pub fn new() -> Self {
        Self {
            findings: Vec::new(),
        }
    }

    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    pub fn add(&mut self, finding: VerifyFinding) {
        self.findings.push(finding);
    }

    /// Format as human-readable text.
    pub fn format_text(&self, checks_run: &[&str]) -> String {
        let mut out = String::new();

        // Group findings by check
        let mut findings_by_check: std::collections::HashMap<String, Vec<&VerifyFinding>> =
            std::collections::HashMap::new();
        for f in &self.findings {
            findings_by_check
                .entry(f.check.clone())
                .or_default()
                .push(f);
        }

        for check in checks_run {
            if let Some(findings) = findings_by_check.get(*check) {
                for finding in findings {
                    out.push_str(&format!("{}\n", finding));
                }
            } else {
                out.push_str(&format!("[PASS] {}\n", check));
            }
        }

        out.push_str(&format!(
            "\nResults: {} passed, {} warnings, {} errors\n",
            passed_count(checks_run, &findings_by_check),
            self.warnings(),
            self.errors()
        ));

        out
    }

    /// Format as JSON.
    pub fn format_json(&self) -> String {
        let mut findings_json = Vec::new();

        for finding in &self.findings {
            let severity = match finding.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARNING",
                Severity::Info => "INFO",
            };

            let file_str = match &finding.file {
                Some(f) => format!("\"{}\"", escape_json(f)),
                None => "null".to_string(),
            };

            let line_str = match finding.line {
                Some(l) => format!("{}", l),
                None => "null".to_string(),
            };

            findings_json.push(format!(
                r#"    {{
      "check": "{}",
      "severity": "{}",
      "message": "{}",
      "file": {},
      "line": {}
    }}"#,
                escape_json(&finding.check),
                severity,
                escape_json(&finding.message),
                file_str,
                line_str,
            ));
        }

        let errors = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count();
        let warnings = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count();
        let infos = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .count();
        let total = self.findings.len();

        format!(
            r#"{{
  "findings": [
{}
  ],
  "summary": {{
    "total": {},
    "errors": {},
    "warnings": {},
    "info": {}
  }}
}}"#,
            findings_json.join(",\n"),
            total,
            errors,
            warnings,
            infos,
        )
    }

    /// Format as SARIF (Static Analysis Results Interchange Format) v2.1.0.
    pub fn format_sarif(&self, checks_run: &[&str]) -> String {
        let mut rules_json = Vec::new();
        let mut results_json = Vec::new();
        let mut seen_rules: std::collections::HashSet<String> = std::collections::HashSet::new();

        for finding in &self.findings {
            // Add rule if not seen
            if !seen_rules.contains(&finding.check) {
                seen_rules.insert(finding.check.clone());
                rules_json.push(format!(
                    r#"        {{
          "id": "{}",
          "shortDescription": {{
            "text": "{}"
          }}
        }}"#,
                    escape_json(&finding.check),
                    escape_json(&finding.check),
                ));
            }

            // Map severity
            let level = match finding.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "note",
            };

            // Build location
            let location = if let (Some(file), Some(line)) = (&finding.file, &finding.line) {
                format!(
                    r#",
          "locations": [
            {{
              "physicalLocation": {{
                "artifactLocation": {{
                  "uri": "{}"
                }},
                "region": {{
                  "startLine": {}
                }}
              }}
            }}
          ]"#,
                    escape_json(file),
                    line,
                )
            } else if let Some(file) = &finding.file {
                format!(
                    r#",
          "locations": [
            {{
              "physicalLocation": {{
                "artifactLocation": {{
                  "uri": "{}"
                }}
              }}
            }}
          ]"#,
                    escape_json(file),
                )
            } else {
                String::new()
            };

            results_json.push(format!(
                r#"        {{
          "ruleId": "{}",
          "level": "{}",
          "message": {{
            "text": "{}"
          }}{}
        }}"#,
                escape_json(&finding.check),
                level,
                escape_json(&finding.message),
                location,
            ));
        }

        // Add passed checks as rules with no results
        for check in checks_run {
            if !seen_rules.contains(*check) {
                rules_json.push(format!(
                    r#"        {{
          "id": "{}",
          "shortDescription": {{
            "text": "{}"
          }}
        }}"#,
                    escape_json(check),
                    escape_json(check),
                ));
            }
        }

        format!(
            r#"{{
  "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
  "version": "2.1.0",
  "runs": [
    {{
      "tool": {{
        "driver": {{
          "name": "taida-verify",
          "version": "0.2.0",
          "rules": [
{}
          ]
        }}
      }},
      "results": [
{}
      ]
    }}
  ]
}}"#,
            rules_json.join(",\n"),
            results_json.join(",\n"),
        )
    }

    /// Format as JSONL diagnostics stream (`taida.diagnostic.v1`).
    pub fn format_jsonl(&self, checks_run: &[&str]) -> String {
        let mut lines: Vec<String> = Vec::new();

        for finding in &self.findings {
            let severity = match finding.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARNING",
                Severity::Info => "INFO",
            };
            let (code, suggestion) = split_diag_code_and_hint(&finding.message);
            let rec = json!({
                "schema": "taida.diagnostic.v1",
                "stream": "verify",
                "kind": "finding",
                "code": code,
                "message": finding.message,
                "location": {
                    "file": finding.file,
                    "line": finding.line,
                    "column": null,
                },
                "suggestion": suggestion,
                "check": finding.check,
                "severity": severity,
            });
            lines.push(rec.to_string());
        }

        let summary = json!({
            "schema": "taida.diagnostic.v1",
            "stream": "verify",
            "kind": "summary",
            "code": null,
            "message": "verify summary",
            "location": null,
            "suggestion": null,
            "summary": {
                "total": self.findings.len(),
                "errors": self.errors(),
                "warnings": self.warnings(),
                "info": self.findings.iter().filter(|f| f.severity == Severity::Info).count(),
                "checks_run": checks_run.len(),
            }
        });
        lines.push(summary.to_string());

        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

fn passed_count(
    checks_run: &[&str],
    findings_by_check: &std::collections::HashMap<String, Vec<&VerifyFinding>>,
) -> usize {
    checks_run
        .iter()
        .filter(|check| !findings_by_check.keys().any(|key| key == *check))
        .count()
}

/// Available verification checks.
pub const ALL_CHECKS: &[&str] = &[
    "error-coverage",
    "no-circular-deps",
    "dead-code",
    "type-consistency",
    "mutual-recursion",
    "unchecked-division",
    "direction-constraint",
    "unchecked-lax",
];

/// Run a specific verification check.
pub fn run_check(check: &str, program: &Program, file: &str) -> Vec<VerifyFinding> {
    match check {
        "error-coverage" => check_error_coverage(program, file),
        "no-circular-deps" => check_no_circular_deps(program, file),
        "dead-code" => check_dead_code(program, file),
        "type-consistency" => check_type_consistency(program, file),
        "mutual-recursion" => check_mutual_recursion(program, file),
        "unchecked-division" => check_unchecked_division(program, file),
        "direction-constraint" => check_direction_constraint(program, file),
        "unchecked-lax" => check_unchecked_lax(program, file),
        "naming-convention" => check_naming_convention(program, file),
        // unchecked-index removed in v0.5.0 — IndexAccess no longer exists
        _ => vec![VerifyFinding {
            check: check.to_string(),
            severity: Severity::Warning,
            message: format!("Unknown check: {}", check),
            file: None,
            line: None,
        }],
    }
}

/// Run all verification checks.
pub fn run_all_checks(program: &Program, file: &str) -> VerifyReport {
    let mut report = VerifyReport::new();
    for check in ALL_CHECKS {
        let findings = run_check(check, program, file);
        for f in findings {
            report.add(f);
        }
    }
    report
}

// ── Check: error-coverage ─────────────────────────────

/// Verify that all throw sites are covered by error ceilings.
fn check_error_coverage(program: &Program, file: &str) -> Vec<VerifyFinding> {
    let mut extractor = GraphExtractor::new(file);
    let graph = extractor.extract(program, GraphView::Error);
    let result = query::uncovered_throws(&graph);

    match result {
        query::QueryResult::Nodes(uncovered) => {
            if uncovered.is_empty() {
                vec![]
            } else {
                uncovered
                    .iter()
                    .map(|node| VerifyFinding {
                        check: "error-coverage".to_string(),
                        severity: Severity::Error,
                        message: format!("Uncovered throw site: {}", node.label),
                        file: Some(node.location.file.clone()),
                        line: Some(node.location.line),
                    })
                    .collect()
            }
        }
        _ => vec![],
    }
}

// ── Check: no-circular-deps ───────────────────────────

/// Verify no circular module dependencies.
fn check_no_circular_deps(_program: &Program, file: &str) -> Vec<VerifyFinding> {
    match module_graph::detect_local_import_cycle(Path::new(file)) {
        Ok(()) => vec![],
        Err(module_graph::ModuleGraphError::Circular { path }) => {
            vec![VerifyFinding {
                check: "no-circular-deps".to_string(),
                severity: Severity::Error,
                message: format!("Circular dependency: {}", path.display()),
                file: Some(file.to_string()),
                line: None,
            }]
        }
        Err(_) => vec![],
    }
}

// ── Check: dead-code ──────────────────────────────────

/// Detect unreachable functions.
fn check_dead_code(program: &Program, file: &str) -> Vec<VerifyFinding> {
    let mut extractor = GraphExtractor::new(file);
    let graph = extractor.extract(program, GraphView::Call);
    let result = query::unreachable_functions(&graph);

    match result {
        query::QueryResult::Nodes(unreachable) => unreachable
            .iter()
            .map(|node| VerifyFinding {
                check: "dead-code".to_string(),
                severity: Severity::Warning,
                message: format!("Unreachable function: {}", node.label),
                file: Some(node.location.file.clone()),
                line: Some(node.location.line),
            })
            .collect(),
        _ => vec![],
    }
}

// ── Check: type-consistency ───────────────────────────

/// Verify type hierarchy consistency.
fn check_type_consistency(program: &Program, file: &str) -> Vec<VerifyFinding> {
    let mut extractor = GraphExtractor::new(file);
    let graph = extractor.extract(program, GraphView::TypeHierarchy);

    // Check for cycles in the type hierarchy
    let result = query::find_cycles(&graph);
    match result {
        query::QueryResult::Cycles(cycles) => cycles
            .iter()
            .map(|cycle| VerifyFinding {
                check: "type-consistency".to_string(),
                severity: Severity::Error,
                message: format!("Circular type hierarchy: {}", cycle.join(" -> ")),
                file: Some(file.to_string()),
                line: None,
            })
            .collect(),
        _ => vec![],
    }
}

// ── Check: mutual-recursion ───────────────────────────

/// Detect mutual recursion (function call cycles) where at least one edge
/// of the cycle is in **non-tail** position. Such a cycle is guaranteed to
/// blow the stack at runtime and is therefore promoted to a compile-time
/// error (C12-3 / FB-8).
///
/// Tail-only mutual recursion is supported by the runtime (Interpreter /
/// JS via the `mutual_tail_call_target` trampoline) and is left to pass
/// with no finding from this check. The Native backend emits its own
/// warning elsewhere — here we only enforce the "unbounded stack" hard
/// rule. A separate `mutual-recursion-native-warning` pipeline can be
/// added in a future Phase if needed.
///
/// Error code: `[E1614]`.
fn check_mutual_recursion(program: &Program, file: &str) -> Vec<VerifyFinding> {
    // Map function name → FuncDef (source order preserved via Vec)
    let mut func_defs: std::collections::HashMap<String, &FuncDef> =
        std::collections::HashMap::new();
    for stmt in &program.statements {
        if let Statement::FuncDef(fd) = stmt {
            func_defs.insert(fd.name.clone(), fd);
        }
    }
    if func_defs.is_empty() {
        return Vec::new();
    }

    // Build the Call graph and find cycles via the shared query engine.
    let mut extractor = GraphExtractor::new(file);
    let graph = extractor.extract(program, GraphView::Call);
    let cycles = match query::find_cycles(&graph) {
        query::QueryResult::Cycles(c) => c,
        _ => return Vec::new(),
    };
    if cycles.is_empty() {
        return Vec::new();
    }

    // Precompute per-function tail/non-tail call sites.
    // tail_calls[fn] = set of callee names called in tail position
    // non_tail_calls[fn] = set of callee names called in non-tail position
    let mut tail_calls: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    let mut non_tail_calls: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    // first_non_tail_line[(caller, callee)] = earliest source line of a
    // non-tail call — used to anchor the diagnostic.
    let mut first_non_tail_line: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for (name, fd) in &func_defs {
        let sites = tail_pos::collect_call_sites(fd);
        for s in sites {
            if s.is_tail {
                tail_calls
                    .entry(name.clone())
                    .or_default()
                    .insert(s.callee.clone());
            } else {
                non_tail_calls
                    .entry(name.clone())
                    .or_default()
                    .insert(s.callee.clone());
                let key = (name.clone(), s.callee.clone());
                first_non_tail_line
                    .entry(key)
                    .and_modify(|l| *l = (*l).min(s.span.line))
                    .or_insert(s.span.line);
            }
        }
    }

    let mut findings = Vec::new();
    // Deduplicate: the same cycle may be reported multiple times by
    // find_cycles for undirected-looking paths. Normalise by rotation.
    let mut seen_cycles: std::collections::HashSet<String> = std::collections::HashSet::new();

    for cycle in &cycles {
        // `query::find_cycles` returns node *labels* along the cycle path.
        // Filter to cycles that include at least two distinct user-defined
        // functions — a self-recursion (single node cycle) is handled by
        // direct-recursion analyses elsewhere and is not "mutual".
        let user_cycle: Vec<&String> = cycle
            .iter()
            .filter(|lbl| func_defs.contains_key(lbl.as_str()))
            .collect();
        if user_cycle.len() < 2 {
            continue;
        }
        let distinct: std::collections::HashSet<&str> =
            user_cycle.iter().map(|s| s.as_str()).collect();
        if distinct.len() < 2 {
            continue;
        }

        // Canonicalise cycle key so the same cycle isn't reported twice
        // regardless of the DFS entry point.
        let mut sorted: Vec<String> = distinct.iter().map(|s| s.to_string()).collect();
        sorted.sort();
        let key = sorted.join("|");
        if !seen_cycles.insert(key) {
            continue;
        }

        // Walk the cycle edges: cycle[i] calls cycle[i+1]. If ANY such
        // edge has a non-tail call from caller→callee, the cycle is
        // unsafe (may overflow stack) and we reject.
        // `user_cycle` is a path; close it by reconnecting the last to
        // the first.
        let mut unsafe_edge: Option<(String, String, usize)> = None;
        let n = user_cycle.len();
        for i in 0..n {
            let caller = user_cycle[i].clone();
            let callee = user_cycle[(i + 1) % n].clone();
            if caller == callee {
                // self edge — not a mutual edge
                continue;
            }
            let has_non_tail = non_tail_calls
                .get(&caller)
                .map(|set| set.contains(&callee))
                .unwrap_or(false);
            // Conservative rule: if there is ANY non-tail call from
            // caller to callee in the cycle, the recursion may overflow
            // the stack at runtime. Reject. (Having additional tail calls
            // side-by-side does not make the non-tail path safe, because
            // at runtime the branch chosen depends on dynamic input.)
            if has_non_tail {
                let line = first_non_tail_line
                    .get(&(caller.clone(), callee.clone()))
                    .copied()
                    .unwrap_or_else(|| func_defs.get(&caller).map(|fd| fd.span.line).unwrap_or(0));
                unsafe_edge = Some((caller, callee, line));
                break;
            }
        }

        if let Some((caller, callee, line)) = unsafe_edge {
            // Render the cycle path as "A -> B -> ... -> A" for the
            // diagnostic message.
            let mut path_display: Vec<String> = user_cycle.iter().map(|s| (*s).clone()).collect();
            if let Some(first) = path_display.first().cloned() {
                path_display.push(first);
            }
            let msg = format!(
                "[E1614] Mutual recursion in non-tail position: {}. \
                 The non-tail call '{}' inside '{}' will overflow the stack at runtime. \
                 Hint: rewrite the recursive call so it is the last operation in the function body, \
                 or convert to an accumulator-passing style (see docs/reference/tail_recursion.md).",
                path_display.join(" -> "),
                callee,
                caller,
            );
            findings.push(VerifyFinding {
                check: "mutual-recursion".to_string(),
                severity: Severity::Error,
                message: msg,
                file: Some(file.to_string()),
                line: Some(line),
            });
        }
    }

    findings
}

// ── Check: unchecked-division ─────────────────────────

/// Detect division operations that may encounter zero divisors.
fn check_unchecked_division(_program: &Program, _file: &str) -> Vec<VerifyFinding> {
    Vec::new()
}

// unchecked-index check removed in v0.5.0 — IndexAccess no longer exists in the language.
// All index access is now via .get(i) which returns Lax (never throws IndexError).

// ── Check: direction-constraint ──────────────────────

/// Flags used to track which direction operators appear within a single statement.
#[derive(Debug, Default)]
struct DirectionFlags {
    has_forward_arrow: bool,   // =>
    has_backward_arrow: bool,  // <=
    has_unmold_forward: bool,  // ]=>
    has_unmold_backward: bool, // <=[
}

/// Verify single-direction constraint: no mixing => with <= or ]=> with <=[ within one statement.
fn check_direction_constraint(program: &Program, file: &str) -> Vec<VerifyFinding> {
    let mut findings = Vec::new();
    for stmt in &program.statements {
        scan_stmt_for_direction(stmt, file, &mut findings);
    }
    findings
}

fn scan_stmt_for_direction(stmt: &Statement, file: &str, findings: &mut Vec<VerifyFinding>) {
    match stmt {
        Statement::Assignment(assign) => {
            // <= is used at statement level
            let mut flags = DirectionFlags {
                has_backward_arrow: true,
                ..Default::default()
            };
            scan_expr_for_direction(&assign.value, &mut flags);

            if flags.has_forward_arrow && flags.has_backward_arrow {
                findings.push(VerifyFinding {
                    check: "direction-constraint".to_string(),
                    severity: Severity::Error,
                    message: "[E0301] 単一方向制約違反 — 一つの文内で => と <= を混在させることはできません".to_string(),
                    file: Some(file.to_string()),
                    line: Some(assign.span.line),
                });
            }
        }
        Statement::Expr(expr) => {
            // Check if expression uses => (pipeline) and also contains <=
            let mut flags = DirectionFlags::default();
            scan_expr_for_direction(expr, &mut flags);

            if flags.has_forward_arrow && flags.has_backward_arrow {
                findings.push(VerifyFinding {
                    check: "direction-constraint".to_string(),
                    severity: Severity::Error,
                    message: "[E0301] 単一方向制約違反 — 一つの文内で => と <= を混在させることはできません".to_string(),
                    file: Some(file.to_string()),
                    line: Some(expr.span().line),
                });
            }
        }
        Statement::UnmoldForward(uf) => {
            // ]=> is used at statement level
            let mut flags = DirectionFlags {
                has_unmold_forward: true,
                ..Default::default()
            };
            scan_expr_for_direction(&uf.source, &mut flags);

            // Check for <=[ inside the source expression (shouldn't happen but safety net)
            if flags.has_unmold_forward && flags.has_unmold_backward {
                findings.push(VerifyFinding {
                    check: "direction-constraint".to_string(),
                    severity: Severity::Error,
                    message: "[E0302] 単一方向制約違反 — 一つの文内で ]=> と <=[ を混在させることはできません".to_string(),
                    file: Some(file.to_string()),
                    line: Some(uf.span.line),
                });
            }
        }
        Statement::UnmoldBackward(ub) => {
            // <=[ is used at statement level
            let mut flags = DirectionFlags {
                has_unmold_backward: true,
                ..Default::default()
            };
            scan_expr_for_direction(&ub.source, &mut flags);

            if flags.has_unmold_forward && flags.has_unmold_backward {
                findings.push(VerifyFinding {
                    check: "direction-constraint".to_string(),
                    severity: Severity::Error,
                    message: "[E0302] 単一方向制約違反 — 一つの文内で ]=> と <=[ を混在させることはできません".to_string(),
                    file: Some(file.to_string()),
                    line: Some(ub.span.line),
                });
            }
        }
        Statement::FuncDef(fd) => {
            // Recurse into function body — each body statement is its own direction scope
            for body_stmt in &fd.body {
                scan_stmt_for_direction(body_stmt, file, findings);
            }
        }
        Statement::ErrorCeiling(ec) => {
            for handler_stmt in &ec.handler_body {
                scan_stmt_for_direction(handler_stmt, file, findings);
            }
        }
        // TypeDef, MoldDef, InheritanceDef, Import, Export — no direction operators
        _ => {}
    }
}

/// Scan an expression tree for direction operator usage, updating flags.
fn scan_expr_for_direction(expr: &Expr, flags: &mut DirectionFlags) {
    match expr {
        Expr::Pipeline(stages, _) => {
            flags.has_forward_arrow = true;
            // Pipeline stages are sub-expressions — scan them too
            for stage in stages {
                scan_expr_for_direction(stage, flags);
            }
        }
        Expr::BinaryOp(left, _, right, _) => {
            scan_expr_for_direction(left, flags);
            scan_expr_for_direction(right, flags);
        }
        Expr::UnaryOp(_, inner, _) => {
            scan_expr_for_direction(inner, flags);
        }
        Expr::FuncCall(func, args, _) => {
            scan_expr_for_direction(func, flags);
            for arg in args {
                scan_expr_for_direction(arg, flags);
            }
        }
        Expr::MethodCall(obj, _, args, _) => {
            scan_expr_for_direction(obj, flags);
            for arg in args {
                scan_expr_for_direction(arg, flags);
            }
        }
        Expr::FieldAccess(obj, _, _) => {
            scan_expr_for_direction(obj, flags);
        }
        Expr::ListLit(items, _) => {
            for item in items {
                scan_expr_for_direction(item, flags);
            }
        }
        Expr::Lambda(_, body, _) => {
            scan_expr_for_direction(body, flags);
        }
        Expr::CondBranch(branches, _) => {
            for branch in branches {
                if let Some(cond) = &branch.condition {
                    scan_expr_for_direction(cond, flags);
                }
                for stmt in &branch.body {
                    if let Statement::Expr(e) = stmt {
                        scan_expr_for_direction(e, flags);
                    }
                }
            }
        }
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            for field in fields {
                scan_expr_for_direction(&field.value, flags);
            }
        }
        Expr::MoldInst(_, args, fields, _) => {
            for arg in args {
                scan_expr_for_direction(arg, flags);
            }
            for field in fields {
                scan_expr_for_direction(&field.value, flags);
            }
        }
        Expr::Unmold(inner, _) => {
            scan_expr_for_direction(inner, flags);
        }
        Expr::Throw(inner, _) => {
            scan_expr_for_direction(inner, flags);
        }
        // Literals and identifiers — no direction operators
        _ => {}
    }
}

// ── Check: unchecked-lax ─────────────────────────────

/// Detect Lax[T] values used without ]=> unmold or .hasValue check.
///
/// Lax-returning expressions:
/// - `Lax[...]()` mold instantiation
/// - `Div[...]()`, `Mod[...]()` mold instantiation (return Lax)
/// - `.get(...)` method call (list index access returns Lax)
/// - `.first()`, `.last()`, `.max()`, `.min()` method calls (return Lax)
///
/// Safe usage patterns (excluded from warnings):
/// - `]=>` / `<=[` unmold (UnmoldForward/UnmoldBackward statements, Unmold expr)
/// - `.hasValue` field access
/// - `.map()`, `.flatMap()` method calls (monadic operations)
/// - Passing to another function (callee is responsible)
/// - Returning from a function (caller is responsible)
fn check_unchecked_lax(program: &Program, file: &str) -> Vec<VerifyFinding> {
    let mut findings = Vec::new();
    let mut lax_vars: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut safe_vars: std::collections::HashSet<String> = std::collections::HashSet::new();

    scan_stmts_for_unchecked_lax(
        &program.statements,
        file,
        &mut lax_vars,
        &mut safe_vars,
        &mut findings,
    );
    findings
}

/// Check if an expression is a Lax-returning expression.
fn is_lax_producing_expr(expr: &Expr) -> bool {
    match expr {
        // Mold instantiation: Lax[...](), Div[...](), Mod[...]()
        Expr::MoldInst(name, _, _, _) => name == "Lax" || name == "Div" || name == "Mod",
        // Method calls that return Lax: .get(), .first(), .last(), .max(), .min()
        Expr::MethodCall(_, method, _, _) => {
            matches!(method.as_str(), "get" | "first" | "last" | "max" | "min")
        }
        _ => false,
    }
}

/// Check if an expression safely handles a Lax variable (via .hasValue, .map, .flatMap, ]=>).
fn is_safe_lax_usage(expr: &Expr, var_name: &str) -> bool {
    match expr {
        // .hasValue field access on the variable
        Expr::FieldAccess(obj, field, _) if field == "hasValue" => {
            expr_references_var(obj, var_name)
        }
        // .map(), .flatMap() method calls on the variable
        Expr::MethodCall(obj, method, _, _) if method == "map" || method == "flatMap" => {
            expr_references_var(obj, var_name)
        }
        // Unmold expression: expr ]=> (as expression)
        Expr::Unmold(inner, _) => expr_references_var(inner, var_name),
        _ => false,
    }
}

/// Check if an expression directly references a variable by name.
fn expr_references_var(expr: &Expr, var_name: &str) -> bool {
    match expr {
        Expr::Ident(name, _) => name == var_name,
        _ => false,
    }
}

/// Check if an expression uses a Lax variable unsafely (direct use without unmold/check).
/// Returns true if the variable is used unsafely in this expression.
fn expr_uses_lax_unsafely(expr: &Expr, var_name: &str) -> bool {
    match expr {
        // Direct identifier reference — unsafe use
        Expr::Ident(name, _) if name == var_name => true,

        // Safe patterns — not unsafe
        Expr::FieldAccess(obj, field, _)
            if field == "hasValue" && expr_references_var(obj, var_name) =>
        {
            false
        }
        Expr::MethodCall(obj, method, _, _)
            if (method == "map" || method == "flatMap" || method == "hasValue")
                && expr_references_var(obj, var_name) =>
        {
            false
        }
        Expr::Unmold(inner, _) if expr_references_var(inner, var_name) => false,

        // Recurse into sub-expressions
        Expr::BinaryOp(left, _, right, _) => {
            expr_uses_lax_unsafely(left, var_name) || expr_uses_lax_unsafely(right, var_name)
        }
        Expr::UnaryOp(_, inner, _) => expr_uses_lax_unsafely(inner, var_name),
        Expr::FuncCall(func, _args, _) => {
            // Passing a Lax variable to a function is unsafe — the Lax should be
            // unwrapped before use. Check both the function and arguments.
            expr_uses_lax_unsafely(func, var_name)
                || _args.iter().any(|a| expr_uses_lax_unsafely(a, var_name))
        }
        Expr::MethodCall(obj, _, _args, _) => {
            // Check both object and args
            expr_uses_lax_unsafely(obj, var_name)
                || _args.iter().any(|a| expr_uses_lax_unsafely(a, var_name))
        }
        Expr::FieldAccess(obj, _, _) => {
            // Accessing a field on a Lax variable (other than hasValue) is unsafe
            expr_references_var(obj, var_name)
        }
        Expr::Pipeline(stages, _) => stages.iter().any(|s| expr_uses_lax_unsafely(s, var_name)),
        Expr::ListLit(items, _) => items.iter().any(|i| expr_uses_lax_unsafely(i, var_name)),
        Expr::Lambda(_, body, _) => expr_uses_lax_unsafely(body, var_name),
        Expr::CondBranch(branches, _) => branches.iter().any(|b| {
            b.condition
                .as_ref()
                .is_some_and(|c| expr_uses_lax_unsafely(c, var_name))
                || b.body.iter().any(|stmt| {
                    if let Statement::Expr(e) = stmt {
                        expr_uses_lax_unsafely(e, var_name)
                    } else {
                        false
                    }
                })
        }),
        Expr::BuchiPack(fields, _) => fields
            .iter()
            .any(|f| expr_uses_lax_unsafely(&f.value, var_name)),
        Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|f| expr_uses_lax_unsafely(&f.value, var_name)),
        Expr::MoldInst(_, args, fields, _) => {
            args.iter().any(|a| expr_uses_lax_unsafely(a, var_name))
                || fields
                    .iter()
                    .any(|f| expr_uses_lax_unsafely(&f.value, var_name))
        }
        Expr::Throw(inner, _) => expr_uses_lax_unsafely(inner, var_name),
        _ => false,
    }
}

/// Scan a list of statements for unchecked Lax usage within a scope.
fn scan_stmts_for_unchecked_lax(
    stmts: &[Statement],
    file: &str,
    lax_vars: &mut std::collections::HashSet<String>,
    safe_vars: &mut std::collections::HashSet<String>,
    findings: &mut Vec<VerifyFinding>,
) {
    for stmt in stmts {
        scan_stmt_for_unchecked_lax(stmt, file, lax_vars, safe_vars, findings);
    }
}

fn scan_stmt_for_unchecked_lax(
    stmt: &Statement,
    file: &str,
    lax_vars: &mut std::collections::HashSet<String>,
    safe_vars: &mut std::collections::HashSet<String>,
    findings: &mut Vec<VerifyFinding>,
) {
    match stmt {
        // Assignment: check if RHS produces Lax
        Statement::Assignment(assign) => {
            if is_lax_producing_expr(&assign.value) {
                lax_vars.insert(assign.target.clone());
                safe_vars.remove(&assign.target);
            } else {
                // Re-assignment to non-Lax — variable is no longer Lax
                lax_vars.remove(&assign.target);
                safe_vars.remove(&assign.target);
                // Check if the RHS uses any Lax var unsafely
                check_expr_for_unsafe_lax_use(&assign.value, file, lax_vars, safe_vars, findings);
            }
        }

        // Unmold forward: `expr ]=> name` — marks the source as safely consumed
        Statement::UnmoldForward(uf) => {
            // If the source is a Lax variable, mark it safe
            if let Expr::Ident(name, _) = &uf.source {
                safe_vars.insert(name.clone());
            }
            // Also check if source is a Lax-producing expression used directly (OK — ]=> consumes it)
        }

        // Unmold backward: `name <=[ expr` — marks the source as safely consumed
        Statement::UnmoldBackward(ub) => {
            if let Expr::Ident(name, _) = &ub.source {
                safe_vars.insert(name.clone());
            }
        }

        // Expression statement: check for unsafe Lax usage
        Statement::Expr(expr) => {
            // First check if this expression is a safe usage pattern for any Lax var
            for var in lax_vars.iter() {
                if is_safe_lax_usage(expr, var) {
                    safe_vars.insert(var.clone());
                }
            }
            check_expr_for_unsafe_lax_use(expr, file, lax_vars, safe_vars, findings);
        }

        // Function definition: scan body with fresh scope
        Statement::FuncDef(fd) => {
            let mut inner_lax = std::collections::HashSet::new();
            let mut inner_safe = std::collections::HashSet::new();
            scan_stmts_for_unchecked_lax(&fd.body, file, &mut inner_lax, &mut inner_safe, findings);
        }

        // Error ceiling: scan handler body
        Statement::ErrorCeiling(ec) => {
            scan_stmts_for_unchecked_lax(&ec.handler_body, file, lax_vars, safe_vars, findings);
        }

        _ => {}
    }
}

/// Check an expression for unsafe usage of known Lax variables.
fn check_expr_for_unsafe_lax_use(
    expr: &Expr,
    file: &str,
    lax_vars: &std::collections::HashSet<String>,
    safe_vars: &std::collections::HashSet<String>,
    findings: &mut Vec<VerifyFinding>,
) {
    for var in lax_vars.iter() {
        if safe_vars.contains(var) {
            continue;
        }
        if expr_uses_lax_unsafely(expr, var) {
            let line = Some(expr.span().line);
            findings.push(VerifyFinding {
                check: "unchecked-lax".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Lax value '{}' used without ]=> unmold or .hasValue check",
                    var
                ),
                file: Some(file.to_string()),
                line,
            });
        }
    }
}

/// Generate a structural summary in JSON format.
pub fn structural_summary(program: &Program, file: &str) -> String {
    let mut extractor = GraphExtractor::new(file);

    let dataflow_graph = extractor.extract(program, GraphView::Dataflow);
    let module_graph = extractor.extract(program, GraphView::Module);
    let type_graph = extractor.extract(program, GraphView::TypeHierarchy);
    let error_graph = extractor.extract(program, GraphView::Error);
    let call_graph = extractor.extract(program, GraphView::Call);

    // Count functions
    let functions = call_graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function)
        .count();

    // (E30 Phase 7.5 / E30B-006, Lock-F 軸 2) type-hierarchy node を
    // `NodeKind::ClassLikeType` 単一 kind に統合。旧分類 (BuchiPack / Mold /
    // Inheritance) は `metadata["class_like_kind"]` で復元する。
    // Inheritance kind 内の Error 親判定は `metadata["inheritance_parent"]`
    // を使う。structural_summary JSON contract (mold_types / error_types
    // フィールド) は維持する。
    let types = type_graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ClassLikeType)
        .count();

    let mold_types: Vec<String> = type_graph
        .nodes
        .iter()
        .filter(|n| {
            n.kind == NodeKind::ClassLikeType
                && n.metadata.get("class_like_kind").map(|s| s.as_str()) == Some("Mold")
                && n.label != "Mold[T]"
        })
        .map(|n| n.label.clone())
        .collect();

    let error_types: Vec<String> = type_graph
        .nodes
        .iter()
        .filter(|n| {
            n.kind == NodeKind::ClassLikeType
                && n.metadata.get("class_like_kind").map(|s| s.as_str()) == Some("Inheritance")
                && n.metadata.get("inheritance_parent").map(|s| s.as_str()) == Some("Error")
                && n.label != "Error"
        })
        .map(|n| n.label.clone())
        .collect();

    // Dataflow stats
    let forward_pipes = dataflow_graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::PipeForward)
        .count();
    let backward_pipes = dataflow_graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::PipeBackward)
        .count();
    let unmold_ops = dataflow_graph
        .edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::UnmoldForward | EdgeKind::UnmoldBackward))
        .count();

    // Module stats
    let imports = module_graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .count();
    let exports = module_graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Exports)
        .count();

    // Error stats
    let ceilings = error_graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ErrorCeiling)
        .count();
    let throw_sites = error_graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ThrowSite)
        .count();

    let uncovered = match query::uncovered_throws(&error_graph) {
        query::QueryResult::Nodes(nodes) => nodes.len(),
        _ => 0,
    };

    let cycles = match query::find_cycles(&module_graph) {
        query::QueryResult::Cycles(c) => !c.is_empty(),
        _ => false,
    };

    // Build JSON
    let mold_json: Vec<String> = mold_types.iter().map(|m| format!("\"{}\"", m)).collect();
    let error_json: Vec<String> = error_types.iter().map(|e| format!("\"{}\"", e)).collect();

    format!(
        r#"{{
  "version": "1.0",
  "stats": {{
    "files": 1,
    "functions": {},
    "types": {},
    "mold_types": {},
    "error_types": {}
  }},
  "dataflow": {{
    "total_pipes": {},
    "forward_pipes": {},
    "backward_pipes": {},
    "unmold_operations": {}
  }},
  "modules": {{
    "total_imports": {},
    "total_exports": {},
    "has_cycles": {}
  }},
  "errors": {{
    "total_ceilings": {},
    "total_throw_sites": {},
    "uncovered_throws": {}
  }},
  "type_hierarchy": {{
    "mold_types": [{}],
    "error_types": [{}]
  }}
}}"#,
        functions,
        types,
        mold_types.len(),
        error_types.len(),
        forward_pipes + backward_pipes,
        forward_pipes,
        backward_pipes,
        unmold_ops,
        imports,
        exports,
        cycles,
        ceilings,
        throw_sites,
        uncovered,
        mold_json.join(", "),
        error_json.join(", "),
    )
}

// ── Check: naming-convention ─────────────────────────

/// Prelude built-in names that should be excluded from naming convention checks.
const PRELUDE_BUILTINS: &[&str] = &[
    "stdout",
    "stderr",
    "stdin",
    "nowMs",
    "sleep",
    "jsonEncode",
    "jsonPretty",
    "true",
    "false",
];

/// Mold internal field names excluded from naming convention checks.
const MOLD_INTERNAL_FIELDS: &[&str] = &["filling", "unmold", "throw"];

/// Check if a name is a `_` placeholder or starts with `_` (private convention).
fn is_excluded_name(name: &str) -> bool {
    name == "_" || name.starts_with('_') || PRELUDE_BUILTINS.contains(&name)
}

/// Check if a name is PascalCase: starts with uppercase, no underscores, at least one lowercase.
fn is_pascal_case(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_uppercase() {
        return false;
    }
    // Must not contain underscores
    if name.contains('_') {
        return false;
    }
    // Must have at least one lowercase letter (to distinguish from UPPER_SNAKE_CASE single word)
    // Single uppercase letter like "T" is fine for type params — treated as PascalCase
    if name.len() == 1 {
        return true;
    }
    name.chars().any(|c| c.is_ascii_lowercase())
}

/// Check if a name is snake_case: all lowercase/digits/underscores, starts with letter.
fn is_snake_case(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Check if a name is camelCase: starts with lowercase, no underscores, has uppercase.
/// Single-word lowercase names are also valid camelCase (e.g., `add`, `get`).
fn is_camel_case(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }
    // Must not contain underscores
    if name.contains('_') {
        return false;
    }
    // All chars must be alphanumeric
    name.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Convert a name to PascalCase suggestion.
fn to_pascal_case(name: &str) -> String {
    // Handle snake_case and camelCase conversions
    let mut result = String::new();
    let mut capitalize_next = true;
    for ch in name.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert a name to snake_case suggestion.
fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert a name to camelCase suggestion.
fn to_camel_case(name: &str) -> String {
    // If already PascalCase, just lowercase first char
    if is_pascal_case(name) {
        let mut chars = name.chars();
        let first = chars.next().unwrap().to_ascii_lowercase();
        return std::iter::once(first).chain(chars).collect();
    }
    // If snake_case, convert
    let mut result = String::new();
    let mut capitalize_next = false;
    for ch in name.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

/// Verify naming conventions across the program.
fn check_naming_convention(program: &Program, file: &str) -> Vec<VerifyFinding> {
    let mut findings = Vec::new();
    for stmt in &program.statements {
        check_stmt_naming(stmt, file, &mut findings);
    }
    findings
}

fn check_stmt_naming(stmt: &Statement, file: &str, findings: &mut Vec<VerifyFinding>) {
    match stmt {
        // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
        Statement::ClassLikeDef(cl) => {
            let label = match &cl.kind {
                crate::parser::ClassLikeKind::BuchiPack => "Type",
                crate::parser::ClassLikeKind::Mold { .. } => "Mold",
                crate::parser::ClassLikeKind::Inheritance { .. } => "Type",
            };
            if !is_excluded_name(&cl.name) && !is_pascal_case(&cl.name) {
                findings.push(VerifyFinding {
                    check: "naming-convention".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "{} name '{}' should be PascalCase (suggestion: '{}')",
                        label,
                        cl.name,
                        to_pascal_case(&cl.name)
                    ),
                    file: Some(file.to_string()),
                    line: Some(cl.span.line),
                });
            }
            for field in &cl.fields {
                check_field_naming(field, file, findings);
            }
        }

        // Function definition: name must be camelCase
        Statement::FuncDef(fd) => {
            if !is_excluded_name(&fd.name) && !is_camel_case(&fd.name) {
                findings.push(VerifyFinding {
                    check: "naming-convention".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "Function name '{}' should be camelCase (suggestion: '{}')",
                        fd.name,
                        to_camel_case(&fd.name)
                    ),
                    file: Some(file.to_string()),
                    line: Some(fd.span.line),
                });
            }
            // Check parameter names
            for param in &fd.params {
                if !is_excluded_name(&param.name) && !is_snake_case(&param.name) {
                    findings.push(VerifyFinding {
                        check: "naming-convention".to_string(),
                        severity: Severity::Warning,
                        message: format!(
                            "Parameter name '{}' should be snake_case (suggestion: '{}')",
                            param.name,
                            to_snake_case(&param.name)
                        ),
                        file: Some(file.to_string()),
                        line: Some(param.span.line),
                    });
                }
            }
            // Recurse into function body
            for body_stmt in &fd.body {
                check_stmt_naming(body_stmt, file, findings);
            }
        }

        // Assignment: target must be snake_case
        Statement::Assignment(assign)
            if !is_excluded_name(&assign.target) && !is_snake_case(&assign.target) =>
        {
            findings.push(VerifyFinding {
                check: "naming-convention".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Variable name '{}' should be snake_case (suggestion: '{}')",
                    assign.target,
                    to_snake_case(&assign.target)
                ),
                file: Some(file.to_string()),
                line: Some(assign.span.line),
            });
        }

        // Unmold forward: target must be snake_case
        Statement::UnmoldForward(uf)
            if !is_excluded_name(&uf.target) && !is_snake_case(&uf.target) =>
        {
            findings.push(VerifyFinding {
                check: "naming-convention".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Variable name '{}' should be snake_case (suggestion: '{}')",
                    uf.target,
                    to_snake_case(&uf.target)
                ),
                file: Some(file.to_string()),
                line: Some(uf.span.line),
            });
        }

        // Unmold backward: target must be snake_case
        Statement::UnmoldBackward(ub)
            if !is_excluded_name(&ub.target) && !is_snake_case(&ub.target) =>
        {
            findings.push(VerifyFinding {
                check: "naming-convention".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Variable name '{}' should be snake_case (suggestion: '{}')",
                    ub.target,
                    to_snake_case(&ub.target)
                ),
                file: Some(file.to_string()),
                line: Some(ub.span.line),
            });
        }

        // Error ceiling: error_param must be snake_case
        Statement::ErrorCeiling(ec) => {
            if !is_excluded_name(&ec.error_param) && !is_snake_case(&ec.error_param) {
                findings.push(VerifyFinding {
                    check: "naming-convention".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "Error parameter '{}' should be snake_case (suggestion: '{}')",
                        ec.error_param,
                        to_snake_case(&ec.error_param)
                    ),
                    file: Some(file.to_string()),
                    line: Some(ec.span.line),
                });
            }
            // Recurse into handler body
            for handler_stmt in &ec.handler_body {
                check_stmt_naming(handler_stmt, file, findings);
            }
        }

        // Import/Export/Expr — no naming checks needed
        _ => {}
    }
}

/// Check field naming: non-method fields should be snake_case, method fields should be camelCase.
fn check_field_naming(field: &FieldDef, file: &str, findings: &mut Vec<VerifyFinding>) {
    // Skip mold internal fields
    if MOLD_INTERNAL_FIELDS.contains(&field.name.as_str()) {
        return;
    }
    // Skip excluded names
    if is_excluded_name(&field.name) {
        return;
    }

    if field.is_method {
        // Method fields should be camelCase
        if !is_camel_case(&field.name) {
            findings.push(VerifyFinding {
                check: "naming-convention".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Method name '{}' should be camelCase (suggestion: '{}')",
                    field.name,
                    to_camel_case(&field.name)
                ),
                file: Some(file.to_string()),
                line: Some(field.span.line),
            });
        }
        // Also check method params if there's a method_def
        if let Some(md) = &field.method_def {
            for param in &md.params {
                if !is_excluded_name(&param.name) && !is_snake_case(&param.name) {
                    findings.push(VerifyFinding {
                        check: "naming-convention".to_string(),
                        severity: Severity::Warning,
                        message: format!(
                            "Parameter name '{}' should be snake_case (suggestion: '{}')",
                            param.name,
                            to_snake_case(&param.name)
                        ),
                        file: Some(file.to_string()),
                        line: Some(param.span.line),
                    });
                }
            }
        }
    } else {
        // Non-method fields should be snake_case
        if !is_snake_case(&field.name) {
            findings.push(VerifyFinding {
                check: "naming-convention".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Field name '{}' should be snake_case (suggestion: '{}')",
                    field.name,
                    to_snake_case(&field.name)
                ),
                file: Some(file.to_string()),
                line: Some(field.span.line),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_source(source: &str) -> Program {
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        program
    }

    #[test]
    fn test_verify_error_coverage_pass() {
        let program = parse_source("x <= 42");
        let findings = check_error_coverage(&program, "test.td");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_verify_no_circular_deps_pass() {
        let program = parse_source("x <= 42");
        let findings = check_no_circular_deps(&program, "test.td");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_verify_type_consistency_pass() {
        let program = parse_source("Person = @(name: Str, age: Int)");
        let findings = check_type_consistency(&program, "test.td");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_structural_summary() {
        let program = parse_source("add x y =\n  x + y\nx <= 42");
        let summary = structural_summary(&program, "test.td");
        assert!(summary.contains("\"version\": \"1.0\""));
        assert!(summary.contains("\"functions\":"));
    }

    #[test]
    fn test_run_all_checks() {
        let program = parse_source("x <= 42\ny <= x + 1");
        let report = run_all_checks(&program, "test.td");
        // Simple code should pass all checks
        assert_eq!(report.errors(), 0);
    }

    // ── unchecked-division tests ──────────────────────────
    // BinOp::Div and BinOp::Mod have been removed from the language.
    // Division is now via Div[x, y]() and Mod[x, y]() molds which always return Lax.
    // No unchecked-division warnings are needed.

    #[test]
    fn test_no_division_no_findings() {
        let program = parse_source("x <= 10 + 5");
        let findings = check_unchecked_division(&program, "test.td");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_div_mold_no_division_findings() {
        // Div mold should not trigger division warnings
        let program = parse_source("x <= Div[10, 0]()");
        let findings = check_unchecked_division(&program, "test.td");
        assert!(findings.is_empty());
    }

    // unchecked-index tests removed in v0.5.0 — IndexAccess no longer exists

    // ── dead-code tests ──────────────────────────────────

    #[test]
    fn test_dead_code_function_called_directly() {
        // A function called directly should not be dead code
        let program = parse_source("helper x =\n  x + 1\nresult <= helper(5)");
        let findings = check_dead_code(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Directly called function should not be dead code"
        );
    }

    #[test]
    fn test_dead_code_function_in_pipeline() {
        // A function used in a pipeline (with _ placeholder) should not be dead code
        let program = parse_source("double x =\n  x * 2\n5 => double(_)");
        let findings = check_dead_code(&program, "test.td");
        assert!(
            !findings.iter().any(|f| f.message.contains("double")),
            "Function used in pipeline should not be dead code"
        );
    }

    #[test]
    fn test_dead_code_function_assigned_to_variable() {
        // A function assigned to a variable (referenced) should not be dead code
        let program = parse_source("myFunc x =\n  x + 1\nref <= myFunc");
        let findings = check_dead_code(&program, "test.td");
        assert!(
            !findings.iter().any(|f| f.message.contains("myFunc")),
            "Function assigned to variable should not be dead code"
        );
    }

    #[test]
    fn test_dead_code_exported_function() {
        // An exported function should not be dead code
        let program = parse_source("helper x =\n  x + 1\n<<< @(helper)");
        let findings = check_dead_code(&program, "test.td");
        assert!(
            !findings.iter().any(|f| f.message.contains("helper")),
            "Exported function should not be dead code"
        );
    }

    #[test]
    fn test_format_json_empty_findings() {
        let report = VerifyReport::new();
        let json = report.format_json();
        assert!(json.contains("\"findings\": ["));
        assert!(json.contains("\"total\": 0"));
        assert!(json.contains("\"errors\": 0"));
        assert!(json.contains("\"warnings\": 0"));
        assert!(json.contains("\"info\": 0"));
        // Verify it's valid JSON-like structure
        assert!(json.starts_with('{'));
        assert!(json.trim().ends_with('}'));
    }

    #[test]
    fn test_format_json_with_findings() {
        let mut report = VerifyReport::new();
        report.add(VerifyFinding {
            check: "error-coverage".to_string(),
            severity: Severity::Error,
            message: "Uncovered throw site: foo".to_string(),
            file: Some("example.td".to_string()),
            line: Some(42),
        });
        report.add(VerifyFinding {
            check: "dead-code".to_string(),
            severity: Severity::Warning,
            message: "Unreachable function: bar".to_string(),
            file: Some("example.td".to_string()),
            line: Some(10),
        });
        let json = report.format_json();
        assert!(json.contains("\"check\": \"error-coverage\""));
        assert!(json.contains("\"severity\": \"ERROR\""));
        assert!(json.contains("\"line\": 42"));
        assert!(json.contains("\"check\": \"dead-code\""));
        assert!(json.contains("\"severity\": \"WARNING\""));
        assert!(json.contains("\"total\": 2"));
        assert!(json.contains("\"errors\": 1"));
        assert!(json.contains("\"warnings\": 1"));
    }

    #[test]
    fn test_format_json_null_file_and_line() {
        let mut report = VerifyReport::new();
        report.add(VerifyFinding {
            check: "test-check".to_string(),
            severity: Severity::Info,
            message: "Test message".to_string(),
            file: None,
            line: None,
        });
        let json = report.format_json();
        assert!(json.contains("\"file\": null"));
        assert!(json.contains("\"line\": null"));
        assert!(json.contains("\"severity\": \"INFO\""));
        assert!(json.contains("\"info\": 1"));
    }

    #[test]
    fn test_format_json_escapes_special_chars() {
        let mut report = VerifyReport::new();
        report.add(VerifyFinding {
            check: "test".to_string(),
            severity: Severity::Error,
            message: "Message with \"quotes\" and \\backslash".to_string(),
            file: Some("path/to/file.td".to_string()),
            line: Some(1),
        });
        let json = report.format_json();
        assert!(json.contains(r#"\"quotes\""#));
        assert!(json.contains(r#"\\backslash"#));
    }

    #[test]
    fn test_format_jsonl_with_findings_and_summary() {
        let mut report = VerifyReport::new();
        report.add(VerifyFinding {
            check: "error-coverage".to_string(),
            severity: Severity::Error,
            message: "Uncovered throw site: foo".to_string(),
            file: Some("example.td".to_string()),
            line: Some(11),
        });
        report.add(VerifyFinding {
            check: "dead-code".to_string(),
            severity: Severity::Warning,
            message: "Unreachable function: bar".to_string(),
            file: Some("example.td".to_string()),
            line: Some(21),
        });

        let jsonl = report.format_jsonl(&["error-coverage", "dead-code"]);
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 3, "2 findings + 1 summary expected");

        let first: serde_json::Value =
            serde_json::from_str(lines[0]).expect("first jsonl line should be valid json");
        assert_eq!(first["schema"], "taida.diagnostic.v1");
        assert_eq!(first["stream"], "verify");
        assert_eq!(first["kind"], "finding");
        assert_eq!(first["severity"], "ERROR");
        assert_eq!(first["check"], "error-coverage");
        assert_eq!(first["location"]["file"], "example.td");
        assert_eq!(first["location"]["line"], 11);
        assert!(first["location"]["column"].is_null());

        let summary: serde_json::Value =
            serde_json::from_str(lines[2]).expect("summary jsonl line should be valid json");
        assert_eq!(summary["kind"], "summary");
        assert_eq!(summary["summary"]["total"], 2);
        assert_eq!(summary["summary"]["errors"], 1);
        assert_eq!(summary["summary"]["warnings"], 1);
        assert_eq!(summary["summary"]["checks_run"], 2);
    }

    #[test]
    fn test_format_jsonl_extracts_code_and_hint() {
        let mut report = VerifyReport::new();
        report.add(VerifyFinding {
            check: "type-consistency".to_string(),
            severity: Severity::Error,
            message: "[E1400] missing field Hint: add field `x`".to_string(),
            file: None,
            line: None,
        });

        let jsonl = report.format_jsonl(&["type-consistency"]);
        let first: serde_json::Value = serde_json::from_str(jsonl.lines().next().unwrap_or("{}"))
            .expect("jsonl first line should be valid json");
        assert_eq!(first["code"], "E1400");
        assert_eq!(first["suggestion"], "add field `x`");
    }

    #[test]
    fn test_format_jsonl_extracts_legacy_colon_code() {
        let mut report = VerifyReport::new();
        report.add(VerifyFinding {
            check: "direction-constraint".to_string(),
            severity: Severity::Error,
            message: "E0301: 単一方向制約違反".to_string(),
            file: None,
            line: None,
        });

        let jsonl = report.format_jsonl(&["direction-constraint"]);
        let first: serde_json::Value = serde_json::from_str(jsonl.lines().next().unwrap_or("{}"))
            .expect("jsonl first line should be valid json");
        assert_eq!(first["code"], "E0301");
    }

    #[test]
    fn test_dead_code_truly_unused() {
        // A truly unused function should be dead code
        let program = parse_source("unused x =\n  x + 1\nresult <= 42");
        let findings = check_dead_code(&program, "test.td");
        assert!(
            findings.iter().any(|f| f.message.contains("unused")),
            "Truly unused function should be reported as dead code"
        );
    }

    // ── direction-constraint tests ───────────────────────

    #[test]
    fn test_direction_constraint_backward_only() {
        // Pure backward assignment — no violation
        let program = parse_source("x <= 42");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(findings.is_empty(), "Pure <= assignment should pass");
    }

    #[test]
    fn test_direction_constraint_forward_only() {
        // Pure forward pipeline — no violation
        let program = parse_source("42 => stdout(_)");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(findings.is_empty(), "Pure => pipeline should pass");
    }

    #[test]
    fn test_direction_constraint_unmold_forward_only() {
        // Pure ]=> — no violation
        let program = parse_source("Lax[42]() ]=> x");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(findings.is_empty(), "Pure ]=> should pass");
    }

    #[test]
    fn test_direction_constraint_unmold_backward_only() {
        // Pure <=[ — no violation
        let program = parse_source("x <=[ Lax[42]()");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(findings.is_empty(), "Pure <=[ should pass");
    }

    #[test]
    fn test_direction_constraint_all_checks_includes() {
        // direction-constraint should be in ALL_CHECKS
        assert!(
            ALL_CHECKS.contains(&"direction-constraint"),
            "direction-constraint should be registered in ALL_CHECKS"
        );
    }

    #[test]
    fn test_direction_constraint_run_check_not_unknown() {
        // run_check should not return "Unknown check" for direction-constraint
        let program = parse_source("x <= 42");
        let findings = run_check("direction-constraint", &program, "test.td");
        assert!(
            !findings.iter().any(|f| f.message.contains("Unknown check")),
            "direction-constraint should be a known check"
        );
    }

    #[test]
    fn test_direction_constraint_in_function_body() {
        // Direction constraint applies within function bodies too
        let program = parse_source("myFunc x =\n  result <= x + 1");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Function body with pure <= should pass"
        );
    }

    #[test]
    fn test_direction_constraint_separate_statements_ok() {
        // Different directions in SEPARATE statements is fine
        let program = parse_source("x <= 42\n42 => stdout(_)");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Different directions in separate statements should pass"
        );
    }

    #[test]
    fn test_direction_constraint_different_categories_ok() {
        // => and <=[ are different categories, should be allowed
        let program = parse_source("x <=[ Lax[42]()");
        let findings = check_direction_constraint(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Different operator categories should not conflict"
        );
    }

    // ── error-coverage cross-function tests (V-4) ──────

    #[test]
    fn test_error_coverage_cross_function_covered() {
        // risky has uncovered throw, safe calls risky under ceiling.
        // error-coverage should report no findings (cross-function coverage).
        let source = "risky x =
  Error(message <= \"boom\").throw()
=> :Str

safe input =
  |== e: Error =
    \"default\"
  => :Str
  risky(input)
=> :Str";
        let program = parse_source(source);
        let findings = check_error_coverage(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Cross-function coverage: risky's throw should be covered by safe's ceiling. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_error_coverage_uncovered_throw() {
        // risky has throw with no ceiling anywhere -> should report uncovered
        let source = "risky x =
  Error(message <= \"boom\").throw()
=> :Str";
        let program = parse_source(source);
        let findings = check_error_coverage(&program, "test.td");
        assert_eq!(findings.len(), 1, "Should report exactly 1 uncovered throw");
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("Uncovered throw site"));
    }

    #[test]
    fn test_error_coverage_transitive() {
        // inner -> middle -> outer chain:
        // inner has throw, middle calls inner (no ceiling), outer calls middle (with ceiling).
        // Should report no findings (transitive coverage).
        let source = "inner x =
  Error(message <= \"boom\").throw()
=> :Str

middle x =
  inner(x)
=> :Str

outer input =
  |== e: Error =
    \"default\"
  => :Str
  middle(input)
=> :Str";
        let program = parse_source(source);
        let findings = check_error_coverage(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Transitive coverage: inner's throw should be covered via middle -> outer's ceiling. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    // ── unchecked-lax tests ───────────────────────────────

    #[test]
    fn test_unchecked_lax_direct_use_warns() {
        // Lax value used directly without ]=> or .hasValue — should warn
        let program = parse_source("result <= Lax[42]()\nstdout(result)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert_eq!(findings.len(), 1, "Should warn about unchecked Lax usage");
        assert!(findings[0].message.contains("result"));
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn test_unchecked_lax_unmold_forward_safe() {
        // Lax value consumed via ]=> — should NOT warn
        let program = parse_source("Lax[42]() ]=> value\nstdout(value)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Unmold forward should be safe. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_unchecked_lax_unmold_after_assign_safe() {
        // Lax assigned to variable, then ]=> — should NOT warn
        let program = parse_source("result <= Lax[42]()\nresult ]=> value\nstdout(value)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Unmold after assign should be safe. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_unchecked_lax_has_value_safe() {
        // Lax value checked with .hasValue — should NOT warn
        let program = parse_source("result <= Lax[42]()\nresult.hasValue");
        let findings = check_unchecked_lax(&program, "test.td");
        assert!(
            findings.is_empty(),
            ".hasValue check should be safe. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_unchecked_lax_div_warns() {
        // Div mold returns Lax — should warn if used without check
        let program = parse_source("result <= Div[10, 3]()\nstdout(result)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert_eq!(findings.len(), 1, "Div result used unchecked should warn");
        assert!(findings[0].message.contains("result"));
    }

    #[test]
    fn test_unchecked_lax_mod_warns() {
        // Mod mold returns Lax — should warn if used without check
        let program = parse_source("result <= Mod[10, 3]()\nstdout(result)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert_eq!(findings.len(), 1, "Mod result used unchecked should warn");
        assert!(findings[0].message.contains("result"));
    }

    #[test]
    fn test_unchecked_lax_get_method_warns() {
        // .get() returns Lax — should warn if used without check
        let program = parse_source("items <= @[1, 2, 3]\nresult <= items.get(0)\nstdout(result)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert_eq!(
            findings.len(),
            1,
            ".get() result used unchecked should warn"
        );
        assert!(findings[0].message.contains("result"));
    }

    #[test]
    fn test_unchecked_lax_map_safe() {
        // .map() on Lax is monadic — should NOT warn
        let program = parse_source("result <= Lax[42]()\nresult.map(_ x = x + 1)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert!(
            findings.is_empty(),
            ".map() on Lax should be safe. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_unchecked_lax_no_lax_no_findings() {
        // No Lax values — should have no findings
        let program = parse_source("x <= 42\ny <= x + 1\nstdout(y)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_unchecked_lax_reassigned_non_lax() {
        // Lax variable reassigned to non-Lax — should NOT warn after reassignment
        let program = parse_source("result <= Lax[42]()\nresult <= 99\nstdout(result)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Reassigned non-Lax should be safe. Findings: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_unchecked_lax_in_function_scope() {
        // Lax inside function body — should warn within that scope
        let source = "process x =\n  result <= Lax[42]()\n  stdout(result)\n=> :Int";
        let program = parse_source(source);
        let findings = check_unchecked_lax(&program, "test.td");
        assert_eq!(
            findings.len(),
            1,
            "Unchecked Lax in function body should warn"
        );
    }

    #[test]
    fn test_unchecked_lax_run_check_integration() {
        // Integration: run_check with "unchecked-lax" should work
        let program = parse_source("result <= Lax[42]()\nstdout(result)");
        let findings = run_check("unchecked-lax", &program, "test.td");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check, "unchecked-lax");
    }

    #[test]
    fn test_unchecked_lax_first_last_warns() {
        // .first() and .last() return Lax
        let program = parse_source("items <= @[1, 2]\nf <= items.first()\nstdout(f)");
        let findings = check_unchecked_lax(&program, "test.td");
        assert_eq!(
            findings.len(),
            1,
            ".first() result used unchecked should warn"
        );
    }

    // ── naming-convention tests ──────────────────────────

    #[test]
    fn test_naming_type_pascal_case_pass() {
        let program = parse_source("Person = @(name: Str, age: Int)");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "PascalCase type name should pass");
    }

    #[test]
    fn test_naming_type_snake_case_fail() {
        let program = parse_source("my_type = @(name: Str)");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "snake_case type name should fail");
        assert!(findings[0].message.contains("my_type"));
        assert!(findings[0].message.contains("PascalCase"));
        assert!(findings[0].message.contains("MyType"));
    }

    #[test]
    fn test_naming_func_camel_case_pass() {
        let program = parse_source("getPilot id =\n  id\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "camelCase function name should pass");
    }

    #[test]
    fn test_naming_func_single_word_pass() {
        // Single-word lowercase function names are valid camelCase
        let program = parse_source("add x y =\n  x + y\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "Single-word function name should pass");
    }

    #[test]
    fn test_naming_func_pascal_case_fail() {
        let program = parse_source("GetPilot id =\n  id\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "PascalCase function name should fail");
        assert!(findings[0].message.contains("GetPilot"));
        assert!(findings[0].message.contains("camelCase"));
        assert!(findings[0].message.contains("getPilot"));
    }

    #[test]
    fn test_naming_func_snake_case_fail() {
        let program = parse_source("get_pilot id =\n  id\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "snake_case function name should fail");
        assert!(findings[0].message.contains("get_pilot"));
        assert!(findings[0].message.contains("camelCase"));
    }

    #[test]
    fn test_naming_var_snake_case_pass() {
        let program = parse_source("pilot_name <= \"Misato\"");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "snake_case variable should pass");
    }

    #[test]
    fn test_naming_var_camel_case_fail() {
        let program = parse_source("pilotName <= \"Misato\"");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "camelCase variable should fail");
        assert!(findings[0].message.contains("pilotName"));
        assert!(findings[0].message.contains("snake_case"));
        assert!(findings[0].message.contains("pilot_name"));
    }

    #[test]
    fn test_naming_var_pascal_case_fail() {
        let program = parse_source("PilotName <= \"Misato\"");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "PascalCase variable should fail");
        assert!(findings[0].message.contains("PilotName"));
        assert!(findings[0].message.contains("snake_case"));
    }

    #[test]
    fn test_naming_param_snake_case_pass() {
        let program = parse_source("process pilot_id =\n  pilot_id\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "snake_case param should pass");
    }

    #[test]
    fn test_naming_param_camel_case_fail() {
        let program = parse_source("process pilotId =\n  pilotId\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        // Function name "process" is valid camelCase (single word)
        // Parameter "pilotId" should fail
        let param_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("pilotId"))
            .collect();
        assert_eq!(param_findings.len(), 1, "camelCase param should fail");
        assert!(param_findings[0].message.contains("snake_case"));
    }

    #[test]
    fn test_naming_field_snake_case_pass() {
        let program = parse_source("Pilot = @(first_name: Str, last_name: Str)");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "snake_case field names should pass");
    }

    #[test]
    fn test_naming_field_camel_case_fail() {
        let program = parse_source("Pilot = @(firstName: Str)");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "camelCase field should fail");
        assert!(findings[0].message.contains("firstName"));
        assert!(findings[0].message.contains("snake_case"));
    }

    #[test]
    fn test_naming_placeholder_excluded() {
        // _ and _prefix are excluded via is_excluded_name helper
        assert!(is_excluded_name("_"), "_ placeholder should be excluded");
        assert!(is_excluded_name("_private"), "_prefix should be excluded");
        assert!(
            is_excluded_name("stdout"),
            "prelude builtin should be excluded"
        );
        assert!(
            is_excluded_name("nowMs"),
            "camelCase prelude builtin should be excluded"
        );
        assert!(
            !is_excluded_name("myVar"),
            "regular name should not be excluded"
        );
    }

    #[test]
    fn test_naming_underscore_prefix_excluded() {
        // _private convention should be excluded
        let program = parse_source("_private <= 42");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "_prefix should be excluded");
    }

    #[test]
    fn test_naming_prelude_builtins_excluded() {
        // Prelude builtins should be excluded from checks
        let program = parse_source("x <= 42\nstdout(x)");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "Prelude builtins should be excluded");
    }

    #[test]
    fn test_naming_mold_internal_fields_excluded() {
        // Mold internal fields (filling, unmold, throw) should be excluded
        let program = parse_source("Mold[T] => MyMold[T] = @(filling: T, unmold: T, throw: Error)");
        let findings = check_naming_convention(&program, "test.td");
        assert!(
            findings.is_empty(),
            "Mold internal fields should be excluded"
        );
    }

    #[test]
    fn test_naming_unmold_target_snake_case() {
        // ]=> target should be snake_case
        let program = parse_source("Lax[42]() ]=> my_value");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "snake_case unmold target should pass");
    }

    #[test]
    fn test_naming_unmold_target_camel_case_fail() {
        let program = parse_source("Lax[42]() ]=> myValue");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(findings.len(), 1, "camelCase unmold target should fail");
        assert!(findings[0].message.contains("myValue"));
        assert!(findings[0].message.contains("snake_case"));
    }

    #[test]
    fn test_naming_error_ceiling_param() {
        // Error ceiling parameter should be snake_case
        let program = parse_source("|== e: Error =\n  \"default\"\n=> :Str");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "snake_case error param should pass");
    }

    #[test]
    fn test_naming_not_in_all_checks() {
        assert!(
            !ALL_CHECKS.contains(&"naming-convention"),
            "naming-convention belongs to `taida way lint`, not `taida way verify`"
        );
    }

    #[test]
    fn test_naming_run_check_integration() {
        let program = parse_source("my_type = @(name: Str)");
        let findings = run_check("naming-convention", &program, "test.td");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check, "naming-convention");
    }

    #[test]
    fn test_naming_inheritance_pascal_case() {
        let program = parse_source("Error => ValidationError = @(field: Str)");
        let findings = check_naming_convention(&program, "test.td");
        assert!(
            findings.is_empty(),
            "PascalCase inheritance child should pass"
        );
    }

    #[test]
    fn test_naming_inheritance_snake_case_fail() {
        let program = parse_source("Error => validation_error = @(field: Str)");
        let findings = check_naming_convention(&program, "test.td");
        assert_eq!(
            findings.len(),
            1,
            "snake_case inheritance child should fail"
        );
        assert!(findings[0].message.contains("validation_error"));
        assert!(findings[0].message.contains("PascalCase"));
    }

    #[test]
    fn test_naming_mold_def_pascal_case() {
        let program = parse_source("Mold[T] => Container[T] = @(count: Int)");
        let findings = check_naming_convention(&program, "test.td");
        assert!(findings.is_empty(), "PascalCase mold name should pass");
    }

    #[test]
    fn test_naming_multiple_violations() {
        // Multiple violations in one program
        let program = parse_source("my_type = @(firstName: Str)\nGet_Data x =\n  x\n=> :Int");
        let findings = check_naming_convention(&program, "test.td");
        assert!(
            findings.len() >= 3,
            "Should report multiple violations: type name, field name, function name. Got: {}",
            findings
                .iter()
                .map(|f| f.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    #[test]
    fn test_naming_nested_function_body() {
        // Naming violations inside function body should be caught
        let source = "process x =\n  BadName <= 42\n=> :Int";
        let program = parse_source(source);
        let findings = check_naming_convention(&program, "test.td");
        let var_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("BadName"))
            .collect();
        assert_eq!(
            var_findings.len(),
            1,
            "Variable inside function body should be checked"
        );
    }

    // ── naming helper function tests ─────────────────────

    #[test]
    fn test_is_pascal_case() {
        assert!(is_pascal_case("Person"));
        assert!(is_pascal_case("HttpRequest"));
        assert!(is_pascal_case("ValidationError"));
        assert!(is_pascal_case("T")); // Single uppercase letter (type param)
        assert!(!is_pascal_case("person"));
        assert!(!is_pascal_case("my_type"));
        assert!(!is_pascal_case("myType"));
        assert!(!is_pascal_case("CONST_NAME"));
        assert!(!is_pascal_case(""));
    }

    #[test]
    fn test_is_snake_case() {
        assert!(is_snake_case("pilot_name"));
        assert!(is_snake_case("x"));
        assert!(is_snake_case("total_count"));
        assert!(is_snake_case("item2"));
        assert!(!is_snake_case("PilotName"));
        assert!(!is_snake_case("pilotName"));
        assert!(!is_snake_case("CONST"));
        assert!(!is_snake_case(""));
    }

    #[test]
    fn test_is_camel_case() {
        assert!(is_camel_case("getPilot"));
        assert!(is_camel_case("add"));
        assert!(is_camel_case("calculateTotal"));
        assert!(is_camel_case("x"));
        assert!(!is_camel_case("GetPilot"));
        assert!(!is_camel_case("get_pilot"));
        assert!(!is_camel_case("CONST"));
        assert!(!is_camel_case(""));
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("my_type"), "MyType");
        assert_eq!(to_pascal_case("validation_error"), "ValidationError");
        assert_eq!(to_pascal_case("person"), "Person");
        assert_eq!(to_pascal_case("getPilot"), "GetPilot");
    }

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("pilotName"), "pilot_name");
        assert_eq!(to_snake_case("PilotName"), "pilot_name");
        assert_eq!(to_snake_case("HTTPRequest"), "h_t_t_p_request");
        assert_eq!(to_snake_case("name"), "name");
    }

    #[test]
    fn test_to_camel_case() {
        assert_eq!(to_camel_case("GetPilot"), "getPilot");
        assert_eq!(to_camel_case("get_pilot"), "getPilot");
        assert_eq!(to_camel_case("ValidationError"), "validationError");
    }
}
