//! way — split out of src/main.rs (pure move).
//! Behaviour unchanged; imports added per cargo check.

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use taida::diagnostics::split_diag_code_and_hint;
use taida::graph::{verify, walk};
use taida::lexer::Span;
use taida::parser::{BuchiField, Expr, Program, Statement, parse};
use taida::types::{CompileTarget, TypeChecker};

use crate::cli::build::{CompileDiagStats, DiagFormat, emit_compile_diag_jsonl};
use crate::cli::help::{
    print_check_help, print_lint_help, print_todo_help, print_verify_help, print_way_help,
};
use crate::{is_help_flag, reject_removed_migration_command};

pub(crate) fn reject_no_check_under_way() -> ! {
    eprintln!(
        "--no-check is not allowed under 'taida way'. The way hub exists to run quality checks."
    );
    std::process::exit(2);
}

pub(crate) fn way_should_fail(errors: usize, warnings: usize, strict: bool) -> bool {
    errors > 0 || (strict && warnings > 0)
}

pub(crate) fn parse_way_format_or_exit(raw: &str, command: &str) -> WayFormat {
    match WayFormat::parse(raw) {
        Some(format) => format,
        None => {
            eprintln!(
                "Unknown format '{}'. Expected: text | json | jsonl | sarif",
                raw
            );
            if command.is_empty() {
                eprintln!("Run `taida way --help` for usage.");
            } else {
                eprintln!("Run `taida way {} --help` for usage.", command);
            }
            std::process::exit(2);
        }
    }
}

pub(crate) fn run_way(args: &[String], no_check: bool) {
    if no_check {
        reject_no_check_under_way();
    }

    if args.is_empty() || is_help_flag(args[0].as_str()) {
        print_way_help();
        return;
    }

    match args[0].as_str() {
        "check" => run_check_cmd(&args[1..]),
        "lint" => run_lint_cmd(&args[1..]),
        "verify" => run_verify(&args[1..]),
        "todo" => run_todo(&args[1..]),
        "migrate" => reject_removed_migration_command("taida way migrate"),
        _ => run_way_full(args),
    }
}

/// Where a stage of `taida way` writes the one document of a machine-readable
/// format. A stage run on its own prints it; the full gate collects the
/// documents of its stages and prints them as a single one.
pub(crate) enum DocSink<'a> {
    Stdout,
    Collect(&'a mut Vec<(&'static str, String)>),
}

impl DocSink<'_> {
    fn document(&mut self, stage: &'static str, text: String, trailing_newline: bool) {
        match self {
            DocSink::Stdout => {
                if trailing_newline {
                    println!("{}", text);
                } else {
                    print!("{}", text);
                }
            }
            DocSink::Collect(docs) => docs.push((stage, text)),
        }
    }
}

/// Usage errors (a missing value, an unknown option, a second path) end the
/// command with exit code 2, whichever subcommand found them.
fn usage_error(lines: &[String]) -> ! {
    for line in lines {
        eprintln!("{}", line);
    }
    std::process::exit(2);
}

pub(crate) fn run_way_full(args: &[String]) {
    let mut options = WayOptions::default();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida way --help` for usage.");
                    std::process::exit(2);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for `taida way`: {}", raw);
                eprintln!("Run `taida way --help` for usage.");
                std::process::exit(2);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida way.");
                    std::process::exit(2);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let path = match path {
        Some(path) => path,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida way --help` for usage.");
            std::process::exit(2);
        }
    };

    // The stages run in order and the gate ends at the first one that fails.
    // A document format (json, sarif) is one document for the whole gate, so
    // the stages hand theirs over instead of printing it.
    let mut documents: Vec<(&'static str, String)> = Vec::new();
    let collect = matches!(options.format, WayFormat::Json | WayFormat::Sarif);
    let mut code = 0;
    for stage in [WayStage::Check, WayStage::Lint, WayStage::Verify] {
        let mut sink = if collect {
            DocSink::Collect(&mut documents)
        } else {
            DocSink::Stdout
        };
        code = match stage {
            WayStage::Check => check_stage(&path, options, &mut sink),
            WayStage::Lint => lint_stage(&path, options, &mut sink),
            WayStage::Verify => verify_stage(&[], &path, options, &mut sink),
        };
        if code != 0 {
            break;
        }
    }
    if collect && !options.quiet {
        println!("{}", combine_stage_documents(options.format, &documents));
    }
    if code != 0 {
        std::process::exit(code);
    }
}

#[derive(Clone, Copy)]
enum WayStage {
    Check,
    Lint,
    Verify,
}

/// The documents of the stages the gate ran, as the single document of the
/// requested format: a JSON object keyed by stage, or one SARIF log holding
/// the run of each stage.
fn combine_stage_documents(format: WayFormat, documents: &[(&'static str, String)]) -> String {
    let parsed: Vec<(&'static str, serde_json::Value)> = documents
        .iter()
        .map(|(stage, text)| {
            (
                *stage,
                serde_json::from_str(text).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    let combined = match format {
        WayFormat::Sarif => {
            // Every stage is a tool of its own in the log: lint and verify
            // are written by the same report writer, which names both alike.
            let runs: Vec<serde_json::Value> = parsed
                .iter()
                .flat_map(|(stage, doc)| {
                    let runs = doc["runs"].as_array().cloned().unwrap_or_default();
                    runs.into_iter().map(move |mut run| {
                        run["tool"]["driver"]["name"] = json!(format!("taida way {}", stage));
                        run
                    })
                })
                .collect();
            json!({
                "version": "2.1.0",
                "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
                "runs": runs,
            })
        }
        _ => {
            let mut stages = serde_json::Map::new();
            for (stage, doc) in parsed {
                stages.insert(stage.to_string(), doc);
            }
            json!({
                "schema": "taida.way.v1",
                "stages": stages,
            })
        }
    };
    serde_json::to_string_pretty(&combined).unwrap_or_else(|_| "{}".to_string())
}

/// The `.td` files a `<PATH>` names: the file itself, or every `.td` file
/// under a directory, and the input failures met on the way: a directory that
/// cannot be read, or a directory without any `.td` file.
fn resolve_way_targets(target: &str) -> (Vec<PathBuf>, Vec<CheckDiagnostic>) {
    let target_path = Path::new(target);
    if !target_path.is_dir() {
        return (vec![target_path.to_path_buf()], Vec::new());
    }
    let (files, unreadable) = collect_td_files_reporting(target_path);
    let mut failures: Vec<CheckDiagnostic> = unreadable
        .into_iter()
        .map(|(dir, reason)| {
            let dir = dir.to_string_lossy().to_string();
            input_failure(
                "io",
                Some(&dir),
                format!("Error reading directory '{}': {}", dir, reason),
            )
        })
        .collect();
    if files.is_empty() && failures.is_empty() {
        failures.push(input_failure(
            "io",
            None,
            format!("No .td files found in '{}'", target),
        ));
    }
    (files, failures)
}

/// A file that could not be read or parsed, as the diagnostic every stage
/// reports it with.
fn input_failure(stage: &'static str, file: Option<&str>, message: String) -> CheckDiagnostic {
    let (code, suggestion) = split_diag_code_and_hint(&message);
    CheckDiagnostic {
        stage,
        severity: "ERROR",
        code,
        message,
        file: file.map(str::to_string),
        line: None,
        column: None,
        suggestion,
    }
}

/// Read and parse one file. On failure returns a diagnostic for the read
/// error, or one per parse error.
fn load_way_source(td_file: &Path) -> Result<(Program, String), Vec<CheckDiagnostic>> {
    let file_str = td_file.to_string_lossy().to_string();
    let source = fs::read_to_string(td_file).map_err(|e| {
        vec![input_failure(
            "io",
            Some(&file_str),
            format!("Error reading file '{}': {}", file_str, e),
        )]
    })?;
    let (program, parse_errors) = parse(&source);
    if !parse_errors.is_empty() {
        return Err(parse_errors
            .iter()
            .map(|err| {
                let (code, suggestion) = split_diag_code_and_hint(&err.message);
                CheckDiagnostic {
                    stage: "parse",
                    severity: "ERROR",
                    code,
                    message: err.message.clone(),
                    file: Some(file_str.clone()),
                    line: Some(err.span.line),
                    column: Some(err.span.column),
                    suggestion,
                }
            })
            .collect());
    }
    Ok((program, source))
}

pub(crate) fn run_check_cmd(args: &[String]) {
    let mut options = WayOptions::default();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_check_help();
                return;
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    usage_error(&[
                        "Missing value for --format.".to_string(),
                        "Run `taida way check --help` for usage.".to_string(),
                    ]);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "check");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            "--json" => {
                eprintln!("`--json` was removed. Use `taida way check --format json`.");
                std::process::exit(2);
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for check: {}", raw);
                eprintln!("Run `taida way check --help` for usage.");
                std::process::exit(2);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida way check.");
                    std::process::exit(2);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let target = match path {
        Some(p) => p,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida way check --help` for usage.");
            std::process::exit(2);
        }
    };

    let code = check_stage(&target, options, &mut DocSink::Stdout);
    if code != 0 {
        std::process::exit(code);
    }
}

/// `taida way check`: parse and type-check every file. Returns the exit code.
fn check_stage(target: &str, options: WayOptions, sink: &mut DocSink<'_>) -> i32 {
    let (td_files, mut diagnostics) = resolve_way_targets(target);

    for td_file in &td_files {
        let file_str = td_file.to_string_lossy().to_string();
        let program = match load_way_source(td_file) {
            Ok((program, _)) => program,
            Err(failures) => {
                diagnostics.extend(failures);
                continue;
            }
        };

        taida::pkg::resolver::set_program_root_for(std::path::Path::new(&file_str));
        let mut checker = TypeChecker::new();
        checker.set_compile_target(CompileTarget::Interpreter);
        checker.set_source_file(std::path::Path::new(&file_str));
        checker.check_program(&program);
        for err in &checker.errors {
            let (code, suggestion) = split_diag_code_and_hint(&err.message);
            diagnostics.push(CheckDiagnostic {
                stage: "type",
                severity: "ERROR",
                code,
                message: err.message.clone(),
                file: Some(file_str.clone()),
                line: Some(err.span.line),
                column: Some(err.span.column),
                suggestion,
            });
        }

        // F42 sweep [E0701]: surface direct non-tail recursion at
        // `way check` time. The verify check is registered in
        // `verify::ALL_CHECKS` for `way verify`, but `way check` does
        // not run the full verify pipeline; pulling this single check
        // in keeps PHILOSOPHY I — strict, no unbounded stacks — visible
        // during the default lint flow as well.
        let f42_findings = verify::run_check("direct-non-tail-recursion", &program, &file_str);
        for f in f42_findings {
            let (code, suggestion) = split_diag_code_and_hint(&f.message);
            diagnostics.push(CheckDiagnostic {
                stage: "verify",
                severity: "ERROR",
                code,
                message: f.message.clone(),
                file: f.file.clone(),
                line: f.line,
                column: None,
                suggestion,
            });
        }
    }

    let errors = diagnostics.iter().filter(|d| d.severity == "ERROR").count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == "WARNING")
        .count();
    let infos = diagnostics.iter().filter(|d| d.severity == "INFO").count();

    if !options.quiet {
        emit_check_diagnostics(
            &diagnostics,
            td_files.len(),
            options,
            errors,
            warnings,
            infos,
            sink,
        );
    }

    i32::from(way_should_fail(errors, warnings, options.strict))
}

pub(crate) fn emit_check_diagnostics(
    diagnostics: &[CheckDiagnostic],
    files: usize,
    options: WayOptions,
    errors: usize,
    warnings: usize,
    infos: usize,
    sink: &mut DocSink<'_>,
) {
    match options.format {
        WayFormat::Text => {
            for d in diagnostics {
                match (&d.file, d.line, d.column) {
                    (Some(file), Some(line), Some(column)) => {
                        eprintln!(
                            "[{}][{}] {} ({}:{}:{})",
                            d.severity, d.stage, d.message, file, line, column
                        );
                    }
                    (Some(file), Some(line), None) => {
                        eprintln!(
                            "[{}][{}] {} ({}:{})",
                            d.severity, d.stage, d.message, file, line
                        );
                    }
                    (Some(file), None, _) => {
                        eprintln!("[{}][{}] {} ({})", d.severity, d.stage, d.message, file);
                    }
                    _ => eprintln!("[{}][{}] {}", d.severity, d.stage, d.message),
                }
            }
            eprintln!(
                "check summary: total={}, errors={}, warnings={}, info={}, files={}",
                diagnostics.len(),
                errors,
                warnings,
                infos,
                files
            );
        }
        WayFormat::Json => {
            let output = check_diagnostics_json(diagnostics, files, errors, warnings, infos);
            sink.document(
                "check",
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
                true,
            );
        }
        WayFormat::Jsonl => {
            for d in diagnostics {
                let rec = json!({
                    "schema": "taida.diagnostic.v1",
                    "stream": "check",
                    "kind": "finding",
                    "code": d.code,
                    "message": d.message,
                    "location": {
                        "file": d.file,
                        "line": d.line,
                        "column": d.column,
                    },
                    "suggestion": d.suggestion,
                    "stage": d.stage,
                    "severity": d.severity,
                });
                println!("{}", rec);
            }
            println!(
                "{}",
                json!({
                    "schema": "taida.diagnostic.v1",
                    "stream": "check",
                    "kind": "summary",
                    "code": null,
                    "message": "check summary",
                    "location": null,
                    "suggestion": null,
                    "summary": {
                        "total": diagnostics.len(),
                        "errors": errors,
                        "warnings": warnings,
                        "info": infos,
                        "files": files,
                    }
                })
            );
        }
        WayFormat::Sarif => {
            let results: Vec<serde_json::Value> = diagnostics
                .iter()
                .map(|d| {
                    let level = match d.severity {
                        "ERROR" => "error",
                        "WARNING" => "warning",
                        _ => "note",
                    };
                    json!({
                        "ruleId": d.code.as_deref().unwrap_or(d.stage),
                        "level": level,
                        "message": { "text": d.message },
                        "locations": sarif_locations(d.file.as_deref(), d.line, d.column),
                    })
                })
                .collect();
            sink.document(
                "check",
                serde_json::to_string_pretty(&json!({
                    "version": "2.1.0",
                    "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
                    "runs": [{
                        "tool": {
                            "driver": {
                                "name": "taida way check",
                                "rules": []
                            }
                        },
                        "results": results
                    }]
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                true,
            );
        }
    }
}

/// The SARIF `locations` of a finding: the file, and the region in it when
/// the finding has one. SARIF has no null for either, so a finding without a
/// place has no `locations`.
fn sarif_locations(
    file: Option<&str>,
    line: Option<usize>,
    column: Option<usize>,
) -> Vec<serde_json::Value> {
    let Some(file) = file else {
        return Vec::new();
    };
    let mut physical = json!({ "artifactLocation": { "uri": file } });
    if let Some(line) = line {
        let mut region = json!({ "startLine": line });
        if let Some(column) = column {
            region["startColumn"] = json!(column);
        }
        physical["region"] = region;
    }
    vec![json!({ "physicalLocation": physical })]
}

pub(crate) fn check_diagnostics_json(
    diagnostics: &[CheckDiagnostic],
    files: usize,
    errors: usize,
    warnings: usize,
    infos: usize,
) -> serde_json::Value {
    json!({
        "schema": "taida.check.v1",
        "diagnostics": diagnostics
            .iter()
            .map(|d| {
                json!({
                    "stage": d.stage,
                    "severity": d.severity,
                    "code": d.code,
                    "message": d.message,
                    "location": {
                        "file": d.file,
                        "line": d.line,
                        "column": d.column,
                    },
                    "suggestion": d.suggestion,
                })
            })
            .collect::<Vec<serde_json::Value>>(),
        "summary": {
            "total": diagnostics.len(),
            "errors": errors,
            "warnings": warnings,
            "info": infos,
            "files": files,
        }
    })
}

pub(crate) fn run_lint_cmd(args: &[String]) {
    let mut options = WayOptions::default();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_lint_help();
                return;
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    usage_error(&[
                        "Missing value for --format.".to_string(),
                        "Run `taida way lint --help` for usage.".to_string(),
                    ]);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "lint");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for lint: {}", raw);
                eprintln!("Run `taida way lint --help` for usage.");
                std::process::exit(2);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida way lint.");
                    std::process::exit(2);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let target = match path {
        Some(p) => p,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida way lint --help` for usage.");
            std::process::exit(2);
        }
    };

    let code = lint_stage(&target, options, &mut DocSink::Stdout);
    if code != 0 {
        std::process::exit(code);
    }
}

/// A file that could not be read or parsed, as a finding of the `parse` /
/// `io` pseudo-check in a lint or verify report.
fn input_failure_finding(failure: &CheckDiagnostic) -> verify::VerifyFinding {
    verify::VerifyFinding {
        check: failure.stage.to_string(),
        severity: verify::Severity::Error,
        message: failure.message.clone(),
        file: failure.file.clone(),
        line: failure.line,
    }
}

/// `taida way lint`: the naming-convention pass over every file. Returns the
/// exit code: 1 when it reported a diagnostic, 2 when a file could not be
/// read or parsed, so the lint could not run cleanly.
fn lint_stage(target: &str, options: WayOptions, sink: &mut DocSink<'_>) -> i32 {
    use taida::parser::lint::lint_program_with_source;

    let mut total_diags: usize = 0;
    let mut linted_files: usize = 0;
    let mut report = verify::VerifyReport::new();
    let (td_files, mut failures) = resolve_way_targets(target);

    for td_file in &td_files {
        let file_str = td_file.to_string_lossy().to_string();
        let (program, source) = match load_way_source(td_file) {
            Ok(loaded) => loaded,
            Err(file_failures) => {
                // Lint cannot run cleanly when a file cannot be parsed:
                // skip this file and report.
                failures.extend(file_failures);
                continue;
            }
        };
        let diags = walk::with_stack_for(&program, || lint_program_with_source(&program, &source));
        linted_files += 1;
        total_diags += diags.len();
        for d in &diags {
            report.add(verify::VerifyFinding {
                check: "naming-convention".to_string(),
                severity: verify::Severity::Error,
                message: format!("{} {}", d.code, d.message),
                file: Some(file_str.clone()),
                line: Some(d.span.line),
            });
        }
        if !options.quiet && options.format == WayFormat::Text {
            for d in &diags {
                println!("{}", d.render(&file_str));
            }
        }
    }

    // The lint ran only if some file could be read and parsed.
    let mut checks_run = if linted_files > 0 {
        vec!["naming-convention"]
    } else {
        Vec::new()
    };
    for failure in &failures {
        report.add(input_failure_finding(failure));
        if !checks_run.contains(&failure.stage) {
            checks_run.push(failure.stage);
        }
    }
    if !options.quiet {
        if options.format == WayFormat::Text {
            for failure in &failures {
                eprintln!("{}", failure_text(failure));
            }
        } else {
            match options.format {
                WayFormat::Json => sink.document("lint", report.format_json(), true),
                WayFormat::Jsonl => print!("{}", report.format_jsonl(&checks_run)),
                WayFormat::Sarif => sink.document("lint", report.format_sarif(&checks_run), false),
                WayFormat::Text => {}
            }
        }
    }
    if !failures.is_empty() {
        // Argument-level failure (lint could not clean-run somewhere)
        return 2;
    }
    i32::from(total_diags > 0)
}

/// One line for an input failure in the text formats.
fn failure_text(failure: &CheckDiagnostic) -> String {
    match (&failure.file, failure.line, failure.column) {
        (Some(file), Some(line), Some(column)) => {
            format!("{}:{}:{}: {}", file, line, column, failure.message)
        }
        (Some(file), _, _) => format!("{}: {}", file, failure.message),
        _ => failure.message.clone(),
    }
}

pub(crate) fn collect_release_gate_sites_for_files(td_files: &[PathBuf]) -> Vec<TodoStubSite> {
    let mut sites = Vec::<TodoStubSite>::new();
    for td_file in td_files {
        sites.extend(scan_release_gate_sites(td_file));
    }
    sites.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
            .then(a.kind.cmp(b.kind))
    });
    sites.dedup_by(|a, b| {
        a.file == b.file && a.line == b.line && a.column == b.column && a.kind == b.kind
    });
    sites
}

pub(crate) fn collect_td_files(dir: &Path) -> Vec<std::path::PathBuf> {
    collect_td_files_reporting(dir).0
}

/// The `.td` files under `dir` in path order, and the directories that could
/// not be read with the reason.
pub(crate) fn collect_td_files_reporting(dir: &Path) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut walk = TdWalk {
        root: fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
        files: Vec::new(),
        unreadable: Vec::new(),
        visited: std::collections::HashSet::new(),
    };
    walk.directory(dir);
    walk.files.sort();
    (walk.files, walk.unreadable)
}

/// A walk for `.td` files. A directory is walked once, and entries are
/// visited in name order, so the path a file is listed under does not depend
/// on the order the file system returns them in.
struct TdWalk {
    /// The directory the walk started in, resolved.
    root: PathBuf,
    files: Vec<PathBuf>,
    unreadable: Vec<(PathBuf, String)>,
    visited: std::collections::HashSet<PathBuf>,
}

impl TdWalk {
    fn directory(&mut self, dir: &Path) {
        let identity = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !self.visited.insert(identity) {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                self.unreadable.push((dir.to_path_buf(), err.to_string()));
                return;
            }
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if self.is_link_into_the_tree(&path) {
                continue;
            }
            if path.is_dir() {
                self.directory(&path);
            } else if path.extension().is_some_and(|e| e == "td") {
                self.files.push(path);
            }
        }
    }

    /// Whether `path` is a symlink to something inside the tree being
    /// walked. What it points to is walked under its own name, so following
    /// the link would list the same file twice, under an alias (such as the
    /// link a local dependency is installed as) rather than where it lives,
    /// and a link back to an ancestor would never end. A link to something
    /// outside the tree is followed, once.
    fn is_link_into_the_tree(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
            && fs::canonicalize(path).is_ok_and(|target| target.starts_with(&self.root))
    }
}

pub(crate) fn extract_string_field(fields: &[BuchiField], name: &str) -> Option<String> {
    fields.iter().find_map(|f| {
        if f.name == name {
            if let Expr::StringLit(s, _) = &f.value {
                Some(s.clone())
            } else {
                None
            }
        } else {
            None
        }
    })
}

pub(crate) fn scan_stmt_for_todo(stmt: &Statement, file: &str, out: &mut TodoScanResult) {
    scan_stmt_at(stmt, file, out, None);
}

fn scan_stmt_at(stmt: &Statement, file: &str, out: &mut TodoScanResult, anchor: Option<&Span>) {
    walk::for_each_child_of_stmt(stmt, &mut |child| {
        scan_child_for_todo(child, file, out, anchor)
    });
}

fn scan_child_for_todo(
    child: walk::Node<'_>,
    file: &str,
    out: &mut TodoScanResult,
    anchor: Option<&Span>,
) {
    match child {
        walk::Node::Stmt(stmt) => scan_stmt_at(stmt, file, out, anchor),
        walk::Node::Expr(expr) => scan_expr_at(expr, file, out, anchor),
    }
}

/// Scan an expression for `TODO` / `Stub` molds. Bindings inside branch arms
/// and blocks are statements like any other, and the `${...}` bodies of a
/// template literal are expressions of the enclosing code; a mold found in
/// one is reported at the template's own position, since the interpolation
/// text has no position of its own.
fn scan_expr_at(expr: &Expr, file: &str, out: &mut TodoScanResult, anchor: Option<&Span>) {
    match expr {
        Expr::MoldInst(name, _, fields, span) if name == "TODO" || name == "Stub" => {
            let at = anchor.unwrap_or(span);
            let kind = if name == "TODO" { "TODO" } else { "Stub" };
            if kind == "TODO" {
                out.todos.push(TodoItem {
                    kind,
                    id: extract_string_field(fields, "id"),
                    task: extract_string_field(fields, "task"),
                    file: file.to_string(),
                    line: at.line,
                    column: at.column,
                });
            } else {
                out.todos.push(TodoItem {
                    kind,
                    id: None,
                    task: None,
                    file: file.to_string(),
                    line: at.line,
                    column: at.column,
                });
            }
            out.sites.push(TodoStubSite {
                kind,
                file: file.to_string(),
                line: at.line,
                column: at.column,
            });
        }
        Expr::TemplateLit(template, span) => {
            let at = anchor.unwrap_or(span);
            for inner in walk::template_interpolations(template, span) {
                scan_expr_at(&inner, file, out, Some(at));
            }
            return;
        }
        _ => {}
    }
    walk::for_each_child_of_expr(expr, &mut |child| {
        scan_child_for_todo(child, file, out, anchor)
    });
}

pub(crate) fn scan_program_for_todo(program: &Program, file: &Path) -> TodoScanResult {
    let file_label = file.to_string_lossy().to_string();
    walk::with_stack_for(program, || {
        let mut out = TodoScanResult::default();
        for stmt in &program.statements {
            scan_stmt_for_todo(stmt, &file_label, &mut out);
        }
        out
    })
}

pub(crate) fn resolve_local_import_path(base_dir: &Path, import_path: &str) -> PathBuf {
    base_dir.join(import_path)
}

pub(crate) fn collect_release_scan_files(target_path: &Path) -> Vec<PathBuf> {
    if target_path.is_dir() {
        return collect_td_files(target_path);
    }

    let mut out = Vec::<PathBuf>::new();
    let mut pending = vec![
        target_path
            .canonicalize()
            .unwrap_or_else(|_| target_path.to_path_buf()),
    ];
    let mut seen = std::collections::HashSet::<PathBuf>::new();

    while let Some(file) = pending.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        out.push(file.clone());

        let source = match fs::read_to_string(&file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            continue;
        }

        // N-48: parent() returns None only for root or prefix-less paths,
        // which cannot occur for resolved .td file paths. "." is a safe default.
        let base_dir = file.parent().unwrap_or(Path::new("."));
        for stmt in &program.statements {
            if let Statement::Import(import) = stmt {
                let dep = resolve_local_import_path(base_dir, &import.path);
                if dep.exists() && dep.is_file() {
                    // Canonicalize for dedup; fall back to the joined path if
                    // the file system rejects it (e.g. dangling symlink).
                    pending.push(dep.canonicalize().unwrap_or(dep));
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

pub(crate) fn scan_release_gate_sites(target_path: &Path) -> Vec<TodoStubSite> {
    let mut sites = Vec::<TodoStubSite>::new();
    for td_file in collect_release_scan_files(target_path) {
        let source = match fs::read_to_string(&td_file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            continue;
        }
        let scan = scan_program_for_todo(&program, &td_file);
        sites.extend(scan.sites);
    }
    sites
}

pub(crate) fn report_release_gate_violations(
    mut sites: Vec<TodoStubSite>,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if sites.is_empty() {
        return;
    }
    sites.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
            .then(a.kind.cmp(b.kind))
    });
    if diag_format == DiagFormat::Jsonl {
        emit_compile_diag_jsonl(
            compile_stats,
            "ERROR",
            "verify",
            None,
            "Release gate failed: TODO/Stub remains in source.",
            None,
            None,
            None,
            Some("Remove all TODO/Stub molds before --release build.".to_string()),
        );
        for site in &sites {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "verify",
                None,
                &format!("{} remains in source", site.kind),
                Some(&site.file),
                Some(site.line),
                Some(site.column),
                Some("Replace TODO/Stub with concrete implementation.".to_string()),
            );
        }
    } else {
        eprintln!("Release gate failed: TODO/Stub remains in source.");
        for site in sites.iter().take(20) {
            eprintln!(
                "  - {} at {}:{}:{}",
                site.kind, site.file, site.line, site.column
            );
        }
        if sites.len() > 20 {
            eprintln!("  ... and {} more site(s)", sites.len() - 20);
        }
    }
}

pub(crate) fn run_todo(args: &[String]) {
    let mut options = WayOptions::default();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_todo_help();
                return;
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    usage_error(&[
                        "Missing value for --format.".to_string(),
                        "Run `taida way todo --help` for usage.".to_string(),
                    ]);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "todo");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            raw if raw.starts_with('-') => {
                usage_error(&[
                    format!("Unknown option for todo: {}", raw),
                    "Run `taida way todo --help` for usage.".to_string(),
                ]);
            }
            _ => {
                if path.is_some() {
                    usage_error(&["Only one [PATH] is accepted for taida way todo.".to_string()]);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let target = path.unwrap_or_else(|| ".".to_string());
    let (td_files, mut failures) = resolve_way_targets(&target);

    let mut merged = TodoScanResult::default();
    for td_file in &td_files {
        match load_way_source(td_file) {
            Ok((program, _)) => {
                let scan = scan_program_for_todo(&program, td_file);
                merged.todos.extend(scan.todos);
            }
            Err(file_failures) => failures.extend(file_failures),
        }
    }

    merged.todos.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
            .then(a.id.cmp(&b.id))
    });

    // `Stub` molds are listed apart from the `TODO` molds: they carry no id
    // or task, and the totals count TODOs.
    let (todos, stubs): (Vec<TodoItem>, Vec<TodoItem>) = merged
        .todos
        .into_iter()
        .partition(|item| item.kind == "TODO");
    let mut by_id = std::collections::BTreeMap::<Option<String>, usize>::new();
    let mut by_file = std::collections::BTreeMap::<String, usize>::new();
    for todo in &todos {
        *by_id.entry(todo.id.clone()).or_insert(0) += 1;
        *by_file.entry(todo.file.clone()).or_insert(0) += 1;
    }

    if !options.quiet {
        emit_todo_report(options, &todos, &stubs, &failures, &by_id, &by_file);
    }
    // A file that could not be read or parsed cannot be scanned, so its
    // TODOs are unknown: that fails the command.
    if !failures.is_empty() {
        std::process::exit(1);
    }
}

fn emit_todo_report(
    options: WayOptions,
    todos: &[TodoItem],
    stubs: &[TodoItem],
    failures: &[CheckDiagnostic],
    by_id: &std::collections::BTreeMap<Option<String>, usize>,
    by_file: &std::collections::BTreeMap<String, usize>,
) {
    let todos_json: Vec<serde_json::Value> = todos
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "task": t.task,
                "file": t.file,
                "line": t.line,
                "column": t.column,
            })
        })
        .collect();
    let stubs_json: Vec<serde_json::Value> = stubs
        .iter()
        .map(|t| {
            json!({
                "file": t.file,
                "line": t.line,
                "column": t.column,
            })
        })
        .collect();
    let by_id_json: Vec<serde_json::Value> = by_id
        .iter()
        .map(|(id, count)| json!({ "id": id, "count": count }))
        .collect();
    let by_file_json: Vec<serde_json::Value> = by_file
        .iter()
        .map(|(file, count)| json!({ "file": file, "count": count }))
        .collect();
    let errors_json: Vec<serde_json::Value> = failures
        .iter()
        .map(|f| {
            json!({
                "stage": f.stage,
                "code": f.code,
                "message": f.message,
                "file": f.file,
                "line": f.line,
                "column": f.column,
            })
        })
        .collect();
    // Every mold the scan found, for the streaming formats.
    let all_molds: Vec<&TodoItem> = todos.iter().chain(stubs.iter()).collect();

    match options.format {
        WayFormat::Json => {
            let output = json!({
                "total": todos.len(),
                "todos": todos_json,
                "stubs": stubs_json,
                "byId": by_id_json,
                "byFile": by_file_json,
                "errors": errors_json,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
            );
            return;
        }
        WayFormat::Jsonl => {
            for mold in &all_molds {
                println!(
                    "{}",
                    json!({
                        "schema": "taida.diagnostic.v1",
                        "stream": "todo",
                        "kind": "finding",
                        "code": null,
                        "message": mold.task,
                        "location": {
                            "file": mold.file,
                            "line": mold.line,
                            "column": mold.column,
                        },
                        "suggestion": null,
                        "severity": "INFO",
                        "id": mold.id,
                        "mold": mold.kind,
                    })
                );
            }
            for failure in failures {
                println!(
                    "{}",
                    json!({
                        "schema": "taida.diagnostic.v1",
                        "stream": "todo",
                        "kind": "finding",
                        "code": failure.code,
                        "message": failure.message,
                        "location": {
                            "file": failure.file,
                            "line": failure.line,
                            "column": failure.column,
                        },
                        "suggestion": failure.suggestion,
                        "stage": failure.stage,
                        "severity": "ERROR",
                    })
                );
            }
            println!(
                "{}",
                json!({
                    "schema": "taida.diagnostic.v1",
                    "stream": "todo",
                    "kind": "summary",
                    "code": null,
                    "message": "todo summary",
                    "location": null,
                    "suggestion": null,
                    "summary": {
                        "total": all_molds.len() + failures.len(),
                        "errors": failures.len(),
                        "warnings": 0,
                        "info": all_molds.len(),
                    }
                })
            );
            return;
        }
        WayFormat::Sarif => {
            let mut results: Vec<serde_json::Value> = all_molds
                .iter()
                .map(|mold| {
                    json!({
                        "ruleId": if mold.kind == "TODO" { "todo" } else { "stub" },
                        "level": "note",
                        "message": { "text": mold.task.clone().unwrap_or_else(|| format!("{} remains in source", mold.kind)) },
                        "locations": sarif_locations(Some(&mold.file), Some(mold.line), Some(mold.column)),
                    })
                })
                .collect();
            results.extend(failures.iter().map(|failure| {
                json!({
                    "ruleId": failure.code.as_deref().unwrap_or(failure.stage),
                    "level": "error",
                    "message": { "text": failure.message },
                    "locations": sarif_locations(failure.file.as_deref(), failure.line, failure.column),
                })
            }));
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": "2.1.0",
                    "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
                    "runs": [{
                        "tool": {
                            "driver": {
                                "name": "taida way todo",
                                "rules": []
                            }
                        },
                        "results": results
                    }]
                }))
                .unwrap_or_else(|_| "{}".to_string())
            );
            return;
        }
        WayFormat::Text => {}
    }

    for failure in failures {
        eprintln!("{}", failure_text(failure));
    }
    if todos.is_empty() && stubs.is_empty() {
        if failures.is_empty() {
            println!("No TODO molds found.");
        }
        return;
    }

    if !todos.is_empty() {
        println!("TODOs: {}", todos.len());
        for todo in todos {
            let id = todo.id.as_deref().unwrap_or("<missing-id>");
            let task = todo.task.as_deref().unwrap_or("<missing-task>");
            println!(
                "- [{}] {}:{}:{} {}",
                id, todo.file, todo.line, todo.column, task
            );
        }
        println!();
        println!("By ID:");
        for (id, count) in by_id {
            println!(
                "  - {}: {}",
                id.clone().unwrap_or_else(|| "<missing-id>".to_string()),
                count
            );
        }
        println!("By File:");
        for (file, count) in by_file {
            println!("  - {}: {}", file, count);
        }
    }
    if !stubs.is_empty() {
        if !todos.is_empty() {
            println!();
        }
        println!("Stubs: {}", stubs.len());
        for stub in stubs {
            println!("- {}:{}:{}", stub.file, stub.line, stub.column);
        }
    }
}

pub(crate) fn run_verify(args: &[String]) {
    let mut checks: Vec<String> = Vec::new();
    let mut options = WayOptions::default();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_verify_help();
                return;
            }
            "--check" | "-c" => {
                i += 1;
                if i >= args.len() {
                    usage_error(&[
                        "Missing value for --check.".to_string(),
                        "Run `taida way verify --help` for usage.".to_string(),
                    ]);
                }
                if !verify::ALL_CHECKS.contains(&args[i].as_str()) {
                    usage_error(&[format!(
                        "Unknown check '{}'. Available checks: {}",
                        args[i],
                        verify::ALL_CHECKS.join(", ")
                    )]);
                }
                checks.push(args[i].clone());
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    usage_error(&[
                        "Missing value for --format.".to_string(),
                        "Run `taida way verify --help` for usage.".to_string(),
                    ]);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "verify");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            raw if raw.starts_with('-') => {
                usage_error(&[
                    format!("Unknown option for verify: {}", raw),
                    "Run `taida way verify --help` for usage.".to_string(),
                ]);
            }
            _ => {
                if path.is_some() {
                    usage_error(&["Only one <PATH> is accepted for taida way verify.".to_string()]);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let file_path = match path {
        Some(p) => p,
        None => usage_error(&[
            "Missing <PATH> argument.".to_string(),
            "Run `taida way verify --help` for usage.".to_string(),
        ]),
    };

    let code = verify_stage(&checks, &file_path, options, &mut DocSink::Stdout);
    if code != 0 {
        std::process::exit(code);
    }
}

/// `taida way verify`: the structural checks (`checks`, or all of them when
/// empty) over every file. A file that cannot be read or parsed is reported
/// as a finding of its own and the other files are still verified. Returns
/// the exit code.
fn verify_stage(
    checks: &[String],
    target: &str,
    options: WayOptions,
    sink: &mut DocSink<'_>,
) -> i32 {
    // Aggregate report across all files
    let mut report = verify::VerifyReport::new();
    let mut verified_files = 0usize;
    let (td_files, mut failures) = resolve_way_targets(target);

    for td_file in &td_files {
        let file_str = td_file.to_string_lossy().to_string();
        let program = match load_way_source(td_file) {
            Ok((program, _)) => program,
            Err(file_failures) => {
                failures.extend(file_failures);
                continue;
            }
        };

        verified_files += 1;
        if checks.is_empty() {
            let file_report = verify::run_all_checks(&program, &file_str);
            for f in file_report.findings {
                report.add(f);
            }
        } else {
            for check in checks {
                let findings = verify::run_check(check, &program, &file_str);
                for f in findings {
                    report.add(f);
                }
            }
        }
    }

    // Checks that never ran (no file could be read and parsed) are not
    // reported as passed.
    let mut checks_ref: Vec<&str> = if verified_files == 0 {
        Vec::new()
    } else if checks.is_empty() {
        verify::ALL_CHECKS.to_vec()
    } else {
        checks.iter().map(|s| s.as_str()).collect()
    };
    for failure in &failures {
        report.add(input_failure_finding(failure));
        if !checks_ref.contains(&failure.stage) {
            checks_ref.push(failure.stage);
        }
    }
    if !options.quiet {
        match options.format {
            WayFormat::Json => sink.document("verify", report.format_json(), false),
            WayFormat::Jsonl => print!("{}", report.format_jsonl(&checks_ref)),
            WayFormat::Sarif => sink.document("verify", report.format_sarif(&checks_ref), false),
            WayFormat::Text => print!("{}", report.format_text(&checks_ref)),
        }
    }

    i32::from(way_should_fail(
        report.errors(),
        report.warnings(),
        options.strict,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WayFormat {
    Text,
    Json,
    Jsonl,
    Sarif,
}

impl WayFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "jsonl" => Some(Self::Jsonl),
            "sarif" => Some(Self::Sarif),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WayOptions {
    format: WayFormat,
    strict: bool,
    quiet: bool,
}

impl Default for WayOptions {
    fn default() -> Self {
        Self {
            format: WayFormat::Text,
            strict: false,
            quiet: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CheckDiagnostic {
    stage: &'static str,
    severity: &'static str,
    code: Option<String>,
    message: String,
    file: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    suggestion: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TodoItem {
    /// `"TODO"` or `"Stub"`: both mark code that must not reach a release.
    kind: &'static str,
    id: Option<String>,
    task: Option<String>,
    file: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TodoStubSite {
    kind: &'static str,
    file: String,
    line: usize,
    column: usize,
}

#[derive(Default)]
pub(crate) struct TodoScanResult {
    todos: Vec<TodoItem>,
    sites: Vec<TodoStubSite>,
}
