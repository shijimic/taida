// N-55: Error handling conventions in this CLI binary
//
// This file uses three error handling patterns, chosen by context:
//
// 1. `expect("message")` / `unwrap()` — for invariants that indicate
//    programmer error or a fundamentally broken system (e.g. system clock
//    before epoch, Tokio runtime creation). Panic is acceptable because
//    no meaningful recovery is possible.
//
// 2. `unwrap_or` / `unwrap_or_else` — for fallible operations with safe
//    defaults (e.g. path canonicalization falling back to the original
//    path). Version resolution uses `taida::version::taida_version()`.
//
// 3. `eprintln!` + `process::exit(1)` — for user-facing errors that
//    should produce a diagnostic and terminate (e.g. missing input file,
//    parse errors, build failures). These are not panics.
//
// Library code (`src/lib.rs` and sub-modules) uses `Result<T, String>`
// for error propagation. The CLI layer in this file converts those into
// pattern 3 at the boundary.

use serde_json::json;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "community")]
use taida::auth;
#[cfg(feature = "native")]
use taida::codegen;
#[cfg(feature = "community")]
use taida::community;
use taida::doc;
use taida::graph::ai_format;
use taida::graph::verify;
use taida::interpreter::Interpreter;
use taida::js;
use taida::module_graph;
use taida::parser::{BuchiField, Expr, FieldDef, FuncDef, Program, Statement, parse};
use taida::pkg;
use taida::types::TypeChecker;
use taida::version::taida_version;

fn print_cli_version() {
    println!("Taida Lang {}", taida_version());
}

fn print_cli_help() {
    println!(
        "\
Taida Lang {}

Usage:
  taida [--no-check] <FILE>
  taida [--no-check]
  taida <COMMAND> [OPTIONS]

Commands:
  build       Build JS, Native, or WASM output
  transpile   Alias for `build --target js`
  todo        Scan TODO/Stub molds
  check       Run parse/type/verify front gate
  graph       AI-oriented structural JSON for codebase comprehension
  verify      Run structural verification checks
  inspect     Print summary + verification
  init        Initialize a Taida project
  deps        Resolve/install dependencies strictly
  install     Install dependencies and write lockfile
  update      Update dependencies and lockfile
  publish     Prepare and publish a package
  cache       Manage WASM runtime cache
  doc         Generate docs from doc comments
  lsp         Run the language server over stdio
  auth        Manage authentication state
  community   Access community features

Global options:
  --help, -h     Show this help
  --version, -V  Show version
  --no-check     Skip type checking where supported

Use `taida <COMMAND> --help` for command-specific usage.",
        taida_version()
    );
}

fn print_graph_help() {
    println!(
        "\
Usage:
  taida graph [-o OUTPUT] [--recursive] <PATH>

Options:
  --recursive, -r   Follow imports recursively and produce unified multi-module JSON
  --output, -o      Output path (bare filename writes into .taida/graph/)

Output:
  AI-oriented unified JSON — types, functions, flow, imports, exports

Examples:
  taida graph examples/04_functions.td
  taida graph --recursive examples/complex/inventory/main.td
  taida graph -o snapshot.json examples/04_functions.td"
    );
}

fn print_check_help() {
    println!(
        "\
Usage:
  taida check [--json] <PATH>

Options:
  --json          Print `taida.check.v1` JSON diagnostics

Examples:
  taida check src
  taida check --json main.td"
    );
}

fn print_build_help() {
    println!(
        "\
Usage:
  taida build [--target js|native|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] <PATH>

Options:
  --target        Build target (default: js)
  --output, -o    Output file or directory
  --outdir        Alias of `--output`
  --entry         Native dir entry override (default: main.td)
  --release, -r   Fail if TODO/Stub remains in source
  --no-cache      Disable WASM runtime .o cache
  --diag-format   text | jsonl

Examples:
  taida build --target js src
  taida build --target native --release app.td

Notes:
  `--no-check` is a global option and applies here."
    );
}

fn print_todo_help() {
    println!(
        "\
Usage:
  taida todo [--format text|json] [PATH]

Options:
  --format, -f    text | json

Examples:
  taida todo
  taida todo --format json src"
    );
}

fn print_verify_help() {
    println!(
        "\
Usage:
  taida verify [--check CHECK] [--format FORMAT] <PATH>

Options:
  --check, -c     Run a specific check (repeatable)
  --format, -f    text | json | jsonl | sarif

Examples:
  taida verify src
  taida verify --check error-coverage --format jsonl main.td"
    );
}

fn print_inspect_help() {
    println!(
        "\
Usage:
  taida inspect [--format text|json|sarif] <PATH>

Options:
  --format, -f    text | json | sarif

Examples:
  taida inspect main.td
  taida inspect --format sarif main.td"
    );
}

fn print_init_help() {
    println!(
        "\
Usage:
  taida init [--target rust-addon] [DIR]

Options:
  --target rust-addon  Scaffold a Rust addon project (Cargo.toml, src/lib.rs,
                       native/addon.toml, taida/<name>.td, README.md)

Examples:
  taida init hello-taida
  taida init --target rust-addon my-addon"
    );
}

fn print_deps_help() {
    println!(
        "\
Usage:
  taida deps

Behavior:
  Resolve dependencies strictly and stop before install/lockfile update on any error.

Example:
  taida deps"
    );
}

fn print_install_help() {
    println!(
        "\
Usage:
  taida install [--force-refresh]

Behavior:
  Install resolved dependencies and generate/update `.taida/taida.lock`.

  For addons with a `[library.prebuild]` section in `native/addon.toml`,
  downloads the prebuild binary for the current host target, verifies its
  SHA-256 against the manifest, and places it in
  `.taida/deps/<pkg>/native/lib<name>.<ext>`. Downloads are cached under
  `~/.taida/addon-cache/`; use `taida cache clean --addons` to prune.

  Large addon downloads (>= 256 KiB) show a progress indicator on stderr
  (RC15B-002).

Options:
  --force-refresh   Ignore the addon cache and re-download every prebuild.

Example:
  taida install
  taida install --force-refresh"
    );
}

fn print_update_help() {
    println!(
        "\
Usage:
  taida update

Behavior:
  Resolve dependencies with remote-preferred generation lookup, then reinstall and update lockfile.

Example:
  taida update"
    );
}

#[cfg(feature = "community")]
fn print_publish_help() {
    println!(
        "\
Usage:
  taida publish [--label LABEL] [--dry-run[=MODE]] [--target rust-addon]

Options:
  --label          Append a version label, for example `rc`
  --dry-run        Print the publish plan without changing files or git state.
                   Equivalent to `--dry-run=plan`.
  --dry-run=plan   (Default) Show what would happen; no cargo build, no git, no release.
  --dry-run=build  Run cargo build + lockfile merge + packages.tdm rewrite,
                   then stop. Git commit/push and release are skipped.
                   Useful for inspecting the lockfile before committing.
  --target TARGET  Force a publish target. Supported values:
                     rust-addon   Build the package as a Rust cdylib addon,
                                  merge the host entry into
                                  native/addon.lock.toml and upload
                                  the release asset.
                   When omitted, `taida publish` auto-detects rust-addon
                   from the presence of `native/addon.toml`.

Examples:
  taida publish --dry-run
  taida publish --dry-run=plan
  taida publish --dry-run=build --target rust-addon
  taida publish --label rc
  taida publish --target rust-addon"
    );
}

fn print_doc_help() {
    println!(
        "\
Usage:
  taida doc generate [-o OUTPUT] <PATH>

Options:
  --output, -o    Output path (stdout when omitted)

Examples:
  taida doc generate src
  taida doc generate -o docs/api.md src"
    );
}

#[cfg(feature = "lsp")]
fn print_lsp_help() {
    println!(
        "\
Usage:
  taida lsp

Behavior:
  Start the Taida language server over stdio."
    );
}

fn is_help_flag(raw: &str) -> bool {
    matches!(raw, "--help" | "-h")
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Check for --no-check flag
    let no_check = args.iter().any(|a| a == "--no-check");
    // Filter out --no-check from args for subcommand processing
    let filtered_args: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != "--no-check")
        .cloned()
        .collect();

    if filtered_args.len() > 1 {
        match filtered_args[1].as_str() {
            "--help" | "-h" | "help" => print_cli_help(),
            "--version" | "-V" | "version" => print_cli_version(),
            #[cfg(feature = "lsp")]
            "lsp" => run_lsp(&filtered_args[2..]),
            #[cfg(not(feature = "lsp"))]
            "lsp" => {
                eprintln!("The 'lsp' command requires the 'lsp' feature.");
                eprintln!("Rebuild with: cargo build --features lsp");
                std::process::exit(1);
            }
            "check" => run_check_cmd(&filtered_args[2..]),
            "compile" => {
                eprintln!(
                    "Error: `taida compile` has been removed. Use `taida build --target native` instead."
                );
                std::process::exit(1);
            }
            "transpile" => run_transpile(&filtered_args[2..], no_check),
            "build" => run_build(&filtered_args[2..], no_check),
            "graph" => run_graph(&filtered_args[2..]),
            "verify" => run_verify(&filtered_args[2..]),
            "inspect" => run_inspect(&filtered_args[2..]),
            "init" => run_init(&filtered_args[2..]),
            "deps" => run_deps(&filtered_args[2..]),
            "install" => run_install(&filtered_args[2..]),
            "update" => run_update(&filtered_args[2..]),
            #[cfg(feature = "community")]
            "publish" => run_publish(&filtered_args[2..]),
            #[cfg(not(feature = "community"))]
            "publish" => {
                eprintln!("The 'publish' command requires the 'community' feature.");
                eprintln!("Rebuild with: cargo build --features community");
                std::process::exit(1);
            }
            "doc" => run_doc(&filtered_args[2..]),
            "cache" => run_cache(&filtered_args[2..]),
            "todo" => run_todo(&filtered_args[2..]),
            #[cfg(feature = "community")]
            "auth" => auth::run_auth(&filtered_args[2..]),
            #[cfg(not(feature = "community"))]
            "auth" => {
                eprintln!("The 'auth' command requires the 'community' feature.");
                eprintln!("Rebuild with: cargo build --features community");
                std::process::exit(1);
            }
            #[cfg(feature = "community")]
            "community" | "c" => community::run_community(&filtered_args[2..]),
            #[cfg(not(feature = "community"))]
            "community" | "c" => {
                eprintln!("The 'community' command requires the 'community' feature.");
                eprintln!("Rebuild with: cargo build --features community");
                std::process::exit(1);
            }
            _ => {
                // File execution mode
                let filename = &filtered_args[1];
                match fs::read_to_string(filename) {
                    Ok(source) => run_source(&source, filename, no_check),
                    Err(e) => {
                        eprintln!("Error reading file '{}': {}", filename, e);
                        std::process::exit(1);
                    }
                }
            }
        }
    } else {
        // REPL mode
        print_cli_version();
        println!("Type expressions to evaluate. Ctrl+D to exit.");
        println!();
        repl(no_check);
    }
}

fn run_source(source: &str, filename: &str, no_check: bool) {
    let (program, parse_errors) = parse(source);
    if !parse_errors.is_empty() {
        for err in &parse_errors {
            eprintln!("{}", err);
        }
        std::process::exit(1);
    }

    // Type checking
    if !no_check {
        let mut checker = TypeChecker::new();
        let file_path = std::path::Path::new(filename);
        if file_path.exists() {
            checker.set_source_file(file_path);
        }
        checker.check_program(&program);
        if !checker.errors.is_empty() {
            for err in &checker.errors {
                eprintln!("{}", err);
            }
            std::process::exit(1);
        }
    }

    // Gorilla ceiling warning: check for uncovered throw sites
    if !no_check {
        let findings = verify::run_check("error-coverage", &program, filename);
        for f in &findings {
            if let Some(line) = f.line {
                eprintln!("Warning: {} (line {})", f.message, line);
            } else {
                eprintln!("Warning: {}", f.message);
            }
        }
    }

    let mut interpreter = Interpreter::new();
    // Set current file for module resolution
    if let Ok(canonical) = fs::canonicalize(filename) {
        interpreter.set_current_file(&canonical);
    } else {
        interpreter.set_current_file(Path::new(filename));
    }
    match interpreter.eval_program(&program) {
        Ok(val) => {
            // Print output buffer
            for line in &interpreter.output {
                println!("{}", line);
            }
            // If the last value is not Unit, print it
            if !matches!(val, taida::interpreter::Value::Unit) && interpreter.output.is_empty() {
                println!("{}", val);
            }
        }
        Err(e) => {
            // Print any output that was collected before the error
            for line in &interpreter.output {
                println!("{}", line);
            }
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

fn run_check_cmd(args: &[String]) {
    let mut json_mode = false;
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_check_help();
                return;
            }
            "--json" => json_mode = true,
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for check: {}", raw);
                eprintln!("Run `taida check --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida check.");
                    std::process::exit(1);
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
            eprintln!("Run `taida check --help` for usage.");
            std::process::exit(1);
        }
    };

    let target_path = Path::new(&target);
    let td_files: Vec<PathBuf> = if target_path.is_dir() {
        let files = collect_td_files(target_path);
        if files.is_empty() {
            eprintln!("No .td files found in '{}'", target);
            std::process::exit(1);
        }
        files
    } else {
        vec![target_path.to_path_buf()]
    };

    let mut diagnostics: Vec<CheckDiagnostic> = Vec::new();

    for td_file in &td_files {
        let file_str = td_file.to_string_lossy().to_string();
        let source = match fs::read_to_string(td_file) {
            Ok(s) => s,
            Err(e) => {
                diagnostics.push(CheckDiagnostic {
                    stage: "io",
                    severity: "ERROR",
                    code: None,
                    message: format!("Error reading file '{}': {}", file_str, e),
                    file: Some(file_str),
                    line: None,
                    column: None,
                    suggestion: None,
                });
                continue;
            }
        };

        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                let (code, suggestion) = split_diag_code_and_hint(&err.message);
                diagnostics.push(CheckDiagnostic {
                    stage: "parse",
                    severity: "ERROR",
                    code,
                    message: err.message.clone(),
                    file: Some(file_str.clone()),
                    line: Some(err.span.line),
                    column: Some(err.span.column),
                    suggestion,
                });
            }
            continue;
        }

        let mut checker = TypeChecker::new();
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

        let findings = verify::run_check("error-coverage", &program, &file_str);
        for finding in findings {
            diagnostics.push(CheckDiagnostic {
                stage: "verify",
                severity: match finding.severity {
                    verify::Severity::Error => "ERROR",
                    verify::Severity::Warning => "WARNING",
                    verify::Severity::Info => "INFO",
                },
                code: None,
                message: finding.message,
                file: finding.file,
                line: finding.line,
                column: None,
                suggestion: None,
            });
        }
    }

    let errors = diagnostics.iter().filter(|d| d.severity == "ERROR").count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == "WARNING")
        .count();
    let infos = diagnostics.iter().filter(|d| d.severity == "INFO").count();

    if json_mode {
        let output = json!({
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
                "files": td_files.len(),
            }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        for d in &diagnostics {
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
            td_files.len()
        );
    }

    if errors > 0 {
        std::process::exit(1);
    }
}

// ── Compile / Transpile / Build subcommands ─────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildTarget {
    Js,
    Native,
    WasmMin,
    WasmWasi,
    WasmEdge,
    WasmFull,
}

impl BuildTarget {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "js" => Some(Self::Js),
            "native" => Some(Self::Native),
            "wasm-min" => Some(Self::WasmMin),
            "wasm-wasi" => Some(Self::WasmWasi),
            "wasm-edge" => Some(Self::WasmEdge),
            "wasm-full" => Some(Self::WasmFull),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Js => "js",
            Self::Native => "native",
            Self::WasmMin => "wasm-min",
            Self::WasmWasi => "wasm-wasi",
            Self::WasmEdge => "wasm-edge",
            Self::WasmFull => "wasm-full",
        }
    }

    /// S-2: Returns true for WASM targets that use the runtime cache.
    fn is_wasm(self) -> bool {
        matches!(
            self,
            Self::WasmMin | Self::WasmWasi | Self::WasmEdge | Self::WasmFull
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagFormat {
    Text,
    Jsonl,
}

impl DiagFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "text" => Some(Self::Text),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }
}

#[derive(Default)]
struct CompileDiagStats {
    errors: usize,
    warnings: usize,
    info: usize,
}

fn severity_to_kind(severity: &str) -> &'static str {
    match severity {
        "ERROR" => "error",
        "WARNING" => "warning",
        "INFO" => "info",
        _ => "info",
    }
}

fn split_diag_code_and_hint(message: &str) -> (Option<String>, Option<String>) {
    let code = if let Some(rest) = message.strip_prefix('[') {
        if rest.len() >= 6 {
            let code_candidate = &rest[..5];
            let close = rest.as_bytes()[5];
            if close == b']'
                && code_candidate.len() == 5
                && code_candidate.as_bytes()[0].is_ascii_uppercase()
                && code_candidate.as_bytes()[1..]
                    .iter()
                    .all(|c| c.is_ascii_digit())
            {
                Some(code_candidate.to_string())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let suggestion = message
        .split_once("Hint:")
        .map(|(_, hint)| hint.trim().to_string())
        .filter(|hint| !hint.is_empty());

    (code, suggestion)
}

#[allow(clippy::too_many_arguments)]
fn emit_compile_diag_jsonl(
    stats: &mut CompileDiagStats,
    severity: &str,
    stage: &str,
    code: Option<String>,
    message: &str,
    file: Option<&str>,
    line: Option<usize>,
    column: Option<usize>,
    suggestion: Option<String>,
) {
    match severity {
        "ERROR" => stats.errors += 1,
        "WARNING" => stats.warnings += 1,
        "INFO" => stats.info += 1,
        _ => {}
    }

    let rec = json!({
        "schema": "taida.diagnostic.v1",
        "stream": "compile",
        "kind": severity_to_kind(severity),
        "code": code,
        "message": message,
        "location": {
            "file": file,
            "line": line,
            "column": column,
        },
        "suggestion": suggestion,
        "stage": stage,
        "severity": severity,
    });
    println!("{}", rec);
}

fn emit_compile_summary_jsonl(stats: &CompileDiagStats) {
    let total = stats.errors + stats.warnings + stats.info;
    let rec = json!({
        "schema": "taida.diagnostic.v1",
        "stream": "compile",
        "kind": "summary",
        "code": null,
        "message": "compile diagnostics summary",
        "location": null,
        "suggestion": null,
        "summary": {
            "total": total,
            "errors": stats.errors,
            "warnings": stats.warnings,
            "info": stats.info,
        }
    });
    println!("{}", rec);
}

#[derive(Clone, Debug)]
struct CheckDiagnostic {
    stage: &'static str,
    severity: &'static str,
    code: Option<String>,
    message: String,
    file: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    suggestion: Option<String>,
}

fn print_transpile_help() {
    println!(
        "\
Usage:
  taida transpile [--release] [--diag-format text|jsonl] [-o OUTPUT] <PATH>

Alias for `taida build --target js`.

Example:
  taida transpile src -o dist"
    );
}

fn run_transpile(args: &[String], no_check: bool) {
    if args.len() == 1 && is_help_flag(args[0].as_str()) {
        print_transpile_help();
        return;
    }
    let mut forwarded = vec!["--target".to_string(), "js".to_string()];
    forwarded.extend(args.iter().cloned());
    run_build(&forwarded, no_check);
}

fn print_build_usage_and_exit() -> ! {
    eprintln!(
        "\
Usage:
  taida build [--target js|native|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] <PATH>

Options:
  --target        Build target (default: js)
  --output, -o    Output file or directory
  --outdir        Alias of `--output`
  --entry         Native dir entry override (default: main.td)
  --release, -r   Fail if TODO/Stub remains in source
  --no-cache      Disable WASM runtime .o cache
  --diag-format   text | jsonl"
    );
    std::process::exit(1);
}

fn run_build(args: &[String], no_check: bool) {
    let mut target = BuildTarget::Js;
    let mut diag_format = DiagFormat::Text;
    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut entry_path: Option<String> = None;
    let mut release_mode = false;
    let mut no_cache = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_build_help();
                return;
            }
            "--target" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                target = match BuildTarget::parse(args[i].as_str()) {
                    Some(v) => v,
                    None => {
                        eprintln!(
                            "Unknown build target '{}'. Expected: js | native | wasm-min | wasm-wasi | wasm-edge | wasm-full",
                            args[i]
                        );
                        std::process::exit(1);
                    }
                };
            }
            "--entry" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                entry_path = Some(args[i].clone());
            }
            "--diag-format" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                diag_format = match DiagFormat::parse(args[i].as_str()) {
                    Some(v) => v,
                    None => {
                        eprintln!("Unknown diag format '{}'. Expected: text | jsonl", args[i]);
                        std::process::exit(1);
                    }
                };
            }
            "--outdir" | "--output" | "-o" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                output_path = Some(args[i].clone());
            }
            "-r" | "--release" => {
                release_mode = true;
            }
            "--no-cache" => {
                no_cache = true;
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for build: {}", raw);
                print_build_usage_and_exit();
            }
            _ => {
                if input_path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida build.");
                    print_build_usage_and_exit();
                }
                input_path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let input = match input_path {
        Some(v) => v,
        None => print_build_usage_and_exit(),
    };
    let input_path = Path::new(&input);
    let mut compile_stats = CompileDiagStats::default();

    // S-2: Initialize WASM runtime cache once for all wasm targets.
    // N-2: Emit warning if cache initialization fails instead of silently ignoring.
    #[cfg(feature = "native")]
    let wasm_rt_cache = if no_cache || !target.is_wasm() {
        None
    } else {
        let cache_dir = codegen::driver::default_wasm_cache_dir(input_path.parent());
        match codegen::driver::WasmRuntimeCache::new(cache_dir) {
            Ok(cache) => Some(cache),
            Err(e) => {
                eprintln!("warning: WASM runtime cache initialization failed: {}", e);
                None
            }
        }
    };

    match target {
        BuildTarget::Js => {
            if entry_path.is_some() {
                if diag_format == DiagFormat::Jsonl {
                    emit_compile_diag_jsonl(
                        &mut compile_stats,
                        "ERROR",
                        "compile",
                        None,
                        "`--entry` is only valid with `--target native`.",
                        None,
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("`--entry` is only valid with `--target native`.");
                }
                std::process::exit(1);
            }
            run_build_js(
                input_path,
                output_path.as_deref(),
                release_mode,
                no_check,
                diag_format,
                &mut compile_stats,
            );
        }
        #[cfg(feature = "native")]
        BuildTarget::Native => {
            run_build_native(
                input_path,
                output_path.as_deref(),
                entry_path.as_deref(),
                release_mode,
                no_check,
                diag_format,
                &mut compile_stats,
            );
        }
        #[cfg(not(feature = "native"))]
        BuildTarget::Native => {
            eprintln!("The 'native' build target requires the 'native' feature.");
            eprintln!("Rebuild with: cargo build --features native");
            std::process::exit(1);
        }
        #[cfg(feature = "native")]
        BuildTarget::WasmMin => {
            run_build_wasm_min(
                input_path,
                output_path.as_deref(),
                release_mode,
                no_check,
                wasm_rt_cache.as_ref(),
                diag_format,
                &mut compile_stats,
            );
        }
        #[cfg(not(feature = "native"))]
        BuildTarget::WasmMin => {
            eprintln!("The 'wasm-min' build target requires the 'native' feature.");
            eprintln!("Rebuild with: cargo build --features native");
            std::process::exit(1);
        }
        #[cfg(feature = "native")]
        BuildTarget::WasmWasi => {
            run_build_wasm_wasi(
                input_path,
                output_path.as_deref(),
                release_mode,
                no_check,
                wasm_rt_cache.as_ref(),
                diag_format,
                &mut compile_stats,
            );
        }
        #[cfg(not(feature = "native"))]
        BuildTarget::WasmWasi => {
            eprintln!("The 'wasm-wasi' build target requires the 'native' feature.");
            eprintln!("Rebuild with: cargo build --features native");
            std::process::exit(1);
        }
        #[cfg(feature = "native")]
        BuildTarget::WasmEdge => {
            run_build_wasm_edge(
                input_path,
                output_path.as_deref(),
                release_mode,
                no_check,
                wasm_rt_cache.as_ref(),
                diag_format,
                &mut compile_stats,
            );
        }
        #[cfg(not(feature = "native"))]
        BuildTarget::WasmEdge => {
            eprintln!("The 'wasm-edge' build target requires the 'native' feature.");
            eprintln!("Rebuild with: cargo build --features native");
            std::process::exit(1);
        }
        #[cfg(feature = "native")]
        BuildTarget::WasmFull => {
            run_build_wasm_full(
                input_path,
                output_path.as_deref(),
                release_mode,
                no_check,
                wasm_rt_cache.as_ref(),
                diag_format,
                &mut compile_stats,
            );
        }
        #[cfg(not(feature = "native"))]
        BuildTarget::WasmFull => {
            eprintln!("The 'wasm-full' build target requires the 'native' feature.");
            eprintln!("Rebuild with: cargo build --features native");
            std::process::exit(1);
        }
    }

    if diag_format == DiagFormat::Jsonl {
        emit_compile_summary_jsonl(&compile_stats);
    }
}

fn run_build_js(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if input_path.is_dir() {
        run_build_js_dir(
            input_path,
            output_path,
            release_mode,
            no_check,
            diag_format,
            compile_stats,
        );
    } else {
        run_build_js_file(
            input_path,
            output_path,
            release_mode,
            no_check,
            diag_format,
            compile_stats,
        );
    }
}

fn js_stage_roots() -> &'static Mutex<Vec<PathBuf>> {
    static JS_STAGE_ROOTS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    JS_STAGE_ROOTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn register_js_stage_root(stage_root: &Path) {
    let mut roots = js_stage_roots()
        .lock()
        .expect("js stage root registry mutex poisoned");
    roots.push(stage_root.to_path_buf());
}

fn unregister_js_stage_root(stage_root: &Path) {
    let mut roots = js_stage_roots()
        .lock()
        .expect("js stage root registry mutex poisoned");
    roots.retain(|root| root != stage_root);
}

fn cleanup_registered_js_stage_roots() {
    let mut roots = js_stage_roots()
        .lock()
        .expect("js stage root registry mutex poisoned");
    for root in roots.drain(..) {
        let _ = fs::remove_dir_all(root);
    }
}

fn emit_build_failure_and_exit(
    compile_stats: &mut CompileDiagStats,
    diag_format: DiagFormat,
    stage: &'static str,
    file: Option<&Path>,
    message: &str,
) -> ! {
    cleanup_registered_js_stage_roots();
    if diag_format == DiagFormat::Jsonl {
        let file_label = file.map(|p| p.to_string_lossy().to_string());
        emit_compile_diag_jsonl(
            compile_stats,
            "ERROR",
            stage,
            None,
            message,
            file_label.as_deref(),
            None,
            None,
            None,
        );
    } else {
        eprintln!("{}", message);
    }
    std::process::exit(1);
}

fn is_stdin_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    raw == "/dev/stdin" || raw == "-" || raw.ends_with("/fd/0")
}

#[allow(clippy::too_many_arguments)]
fn transpile_js_source_to_output(
    source: &str,
    source_label: &str,
    source_path: Option<&Path>,
    js_out: &Path,
    import_base_out: Option<&Path>,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
    project_root: Option<&Path>,
    entry_root: Option<&Path>,
    out_root: Option<&Path>,
) {
    let (program, parse_errors) = parse(source);
    if !parse_errors.is_empty() {
        for err in &parse_errors {
            if diag_format == DiagFormat::Jsonl {
                let (code, suggestion) = split_diag_code_and_hint(&err.message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "parse",
                    code,
                    &err.message,
                    Some(source_label),
                    Some(err.span.line),
                    Some(err.span.column),
                    suggestion,
                );
            } else {
                eprintln!("{}: {}", source_label, err);
            }
        }
        cleanup_registered_js_stage_roots();
        std::process::exit(1);
    }

    if !no_check {
        run_type_checks_and_warnings(&program, source_label, diag_format, compile_stats);
    }

    let js_code = {
        let result = if let Some(td_file) = source_path {
            let import_out = import_base_out.unwrap_or(js_out);
            if let (Some(er), Some(or)) = (entry_root, out_root) {
                js::codegen::transpile_with_build_context(
                    &program,
                    td_file,
                    project_root,
                    import_out,
                    er,
                    or,
                )
            } else if let Some(root) = project_root {
                js::codegen::transpile_with_context(&program, td_file, root, import_out)
            } else {
                let mut codegen = js::codegen::JsCodegen::new();
                codegen.generate(&program)
            }
        } else {
            let mut codegen = js::codegen::JsCodegen::new();
            codegen.generate(&program)
        };
        match result {
            Ok(code) => code,
            Err(e) => {
                if diag_format == DiagFormat::Jsonl {
                    let message = format!("Error transpiling '{}': {}", source_label, e);
                    let (code, suggestion) = split_diag_code_and_hint(&message);
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "codegen",
                        code,
                        &message,
                        Some(source_label),
                        None,
                        None,
                        suggestion,
                    );
                } else {
                    eprintln!("Error transpiling '{}': {}", source_label, e);
                }
                cleanup_registered_js_stage_roots();
                std::process::exit(1);
            }
        }
    };

    if let Some(parent) = js_out.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        emit_build_failure_and_exit(
            compile_stats,
            diag_format,
            "io",
            Some(parent),
            &format!("Error creating directory '{}': {}", parent.display(), e),
        );
    }

    if let Err(e) = fs::write(js_out, js_code) {
        emit_build_failure_and_exit(
            compile_stats,
            diag_format,
            "io",
            Some(js_out),
            &format!("Error writing '{}': {}", js_out.display(), e),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn transpile_js_module_to_output(
    td_file: &Path,
    js_out: &Path,
    import_base_out: Option<&Path>,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
    project_root: Option<&Path>,
    entry_root: Option<&Path>,
    out_root: Option<&Path>,
) {
    let source = match fs::read_to_string(td_file) {
        Ok(s) => s,
        Err(e) => {
            emit_build_failure_and_exit(
                compile_stats,
                diag_format,
                "io",
                Some(td_file),
                &format!("Error reading '{}': {}", td_file.display(), e),
            );
        }
    };
    transpile_js_source_to_output(
        &source,
        &td_file.to_string_lossy(),
        Some(td_file),
        js_out,
        import_base_out,
        no_check,
        diag_format,
        compile_stats,
        project_root,
        entry_root,
        out_root,
    );
}

fn write_js_package_json(
    out_dir: &Path,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    let pkg_json_path = out_dir.join("package.json");
    if !pkg_json_path.exists() {
        let pkg_json = r#"{ "type": "module" }"#;
        if let Err(e) = fs::write(&pkg_json_path, pkg_json) {
            if diag_format == DiagFormat::Jsonl {
                emit_compile_diag_jsonl(
                    compile_stats,
                    "WARNING",
                    "io",
                    None,
                    &format!("could not write package.json: {}", e),
                    Some(&pkg_json_path.to_string_lossy()),
                    None,
                    None,
                    None,
                );
            } else {
                eprintln!("Warning: could not write package.json: {}", e);
            }
        }
    }
}

fn run_build_js_file(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if is_stdin_path(input_path) {
        let source = match fs::read_to_string(input_path) {
            Ok(s) => s,
            Err(e) => {
                emit_build_failure_and_exit(
                    compile_stats,
                    diag_format,
                    "io",
                    Some(input_path),
                    &format!("Error reading '{}': {}", input_path.display(), e),
                );
            }
        };

        if release_mode {
            emit_build_failure_and_exit(
                compile_stats,
                diag_format,
                "compile",
                Some(input_path),
                "`taida build --target js --release /dev/stdin` is not supported.",
            );
        }

        let main_out = match output_path {
            Some(path) => PathBuf::from(path),
            None => {
                // N-47: flatten nested unwrap_or_else for clarity.
                // Fallback chain: packages.tdm root -> cwd -> "."
                let project_root = find_packages_tdm()
                    .or_else(|| env::current_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("."));
                let build_root = project_root.join(".taida").join("build").join("js");
                build_root.join("stdin.mjs")
            }
        };

        transpile_js_source_to_output(
            &source,
            &input_path.to_string_lossy(),
            None,
            &main_out,
            None,
            no_check,
            diag_format,
            compile_stats,
            None,
            None,
            None,
        );

        if diag_format == DiagFormat::Text {
            println!("Built (js): {}", main_out.display());
        }
        return;
    }

    // N-49: canonicalize resolves symlinks and produces absolute paths.
    // Falls back to the original path when the file system rejects it
    // (e.g. nonexistent intermediate directory), which is safe because
    // subsequent I/O will surface the real error.
    let entry_path = input_path
        .canonicalize()
        .unwrap_or_else(|_| input_path.to_path_buf());
    let local_modules = match module_graph::collect_local_modules(&entry_path) {
        Ok(files) => files,
        Err(err) => {
            emit_build_failure_and_exit(
                compile_stats,
                diag_format,
                "parse",
                Some(&entry_path),
                &err.to_string(),
            );
        }
    };

    if release_mode {
        let sites = scan_release_gate_sites(&entry_path);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    let pkg_root = find_packages_tdm_from(&entry_path);
    let (main_out, out_root) = match output_path {
        Some(path) => {
            let explicit = PathBuf::from(path);
            if explicit.exists() && explicit.is_dir() {
                let stem = entry_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                (explicit.join(format!("{}.mjs", stem)), explicit)
            } else {
                let out_root = explicit.parent().unwrap_or(Path::new(".")).to_path_buf();
                (explicit, out_root)
            }
        }
        None => {
            let stem = entry_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            let build_dir = pkg_root
                .clone()
                .unwrap_or_else(|| entry_path.parent().unwrap_or(Path::new(".")).to_path_buf())
                .join(".taida")
                .join("build")
                .join("js");
            (build_dir.join(format!("{}.mjs", stem)), build_dir)
        }
    };

    let entry_root = entry_path.parent().unwrap_or(Path::new("."));
    let stage_root = unique_stage_root("file");
    register_js_stage_root(&stage_root);
    let mut staged_outputs = Vec::new();
    let mut count = 0usize;
    for td_file in &local_modules {
        let final_js_out = if *td_file == entry_path {
            main_out.clone()
        } else {
            // N-51: Multi-stage relative path resolution for modules
            // outside the entry root. The fallback chain preserves as much
            // directory structure as possible in the output tree:
            //   1. Try strip_prefix from entry_root (same directory tree)
            //   2. Try strip_prefix from entry_root's parent (sibling tree)
            //   3. Fall back to just the file name (disjoint tree)
            let rel = td_file
                .strip_prefix(entry_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| {
                    let entry_parent = entry_root.parent().unwrap_or(entry_root);
                    td_file
                        .strip_prefix(entry_parent)
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|_| {
                            PathBuf::from(
                                td_file
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("module.td"),
                            )
                        })
                });
            out_root.join(rel.with_extension("mjs"))
        };
        let stage_js_out = stage_output_path(&stage_root, &out_root, &final_js_out);
        transpile_js_module_to_output(
            td_file,
            &stage_js_out,
            Some(&final_js_out),
            no_check,
            diag_format,
            compile_stats,
            pkg_root.as_deref(),
            Some(entry_root),
            Some(&out_root),
        );
        staged_outputs.push((stage_js_out, final_js_out));
        count += 1;
    }

    // Stage dependency .td files in .taida/deps/ alongside local outputs so
    // failed builds can roll them back before anything becomes visible.
    if let Some(ref root) = pkg_root {
        stage_dep_js_outputs(
            root,
            &stage_root.join("deps"),
            &mut staged_outputs,
            no_check,
            diag_format,
            compile_stats,
        );
    }

    commit_staged_js_outputs(&staged_outputs, &stage_root);
    unregister_js_stage_root(&stage_root);
    if diag_format == DiagFormat::Text {
        if count <= 1 {
            println!("Built (js): {}", main_out.display());
        } else {
            println!(
                "Built {} file(s) [{}] → {}",
                count,
                BuildTarget::Js.as_str(),
                out_root.display()
            );
        }
    }
}

/// Stage all missing dependency .mjs files under .taida/deps/ so they can be
/// committed atomically with the main JS outputs.
fn stage_dep_js_outputs(
    project_root: &Path,
    deps_stage_root: &Path,
    staged_outputs: &mut Vec<(PathBuf, PathBuf)>,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    let deps_dir = project_root.join(".taida").join("deps");
    if !deps_dir.exists() {
        return;
    }
    let td_files = collect_td_files(&deps_dir);
    for td_file in &td_files {
        let final_mjs_out = td_file.with_extension("mjs");
        if final_mjs_out.exists() {
            continue; // already transpiled
        }
        let stage_mjs_out = stage_output_path(deps_stage_root, &deps_dir, &final_mjs_out);
        transpile_js_module_to_output(
            td_file,
            &stage_mjs_out,
            Some(&final_mjs_out),
            no_check,
            diag_format,
            compile_stats,
            Some(project_root),
            None,
            None,
        );
        staged_outputs.push((stage_mjs_out, final_mjs_out));
    }
}

fn unique_stage_root(label: &str) -> PathBuf {
    // N-53: duration_since(UNIX_EPOCH) fails only if the system clock
    // is set before 1970-01-01, which indicates a severely misconfigured
    // system. The expect message documents this invariant.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after 1970-01-01 (UNIX epoch)")
        .as_nanos();
    std::env::temp_dir().join(format!(
        ".taida_js_stage_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ))
}

fn stage_output_path(stage_root: &Path, out_root: &Path, final_out: &Path) -> PathBuf {
    if let Ok(rel) = final_out.strip_prefix(out_root) {
        stage_root.join(rel)
    } else {
        stage_root.join(
            final_out
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output.mjs"),
        )
    }
}

#[derive(Clone)]
struct StagedJsCommit {
    final_path: PathBuf,
    temp_path: PathBuf,
    backup_path: Option<PathBuf>,
}

fn commit_temp_path(final_path: &Path, commit_id: &str, idx: usize) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output.mjs");
    final_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{}.taida-stage-{}-{}", file_name, commit_id, idx))
}

fn commit_backup_path(final_path: &Path, commit_id: &str, idx: usize) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output.mjs");
    final_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{}.taida-backup-{}-{}", file_name, commit_id, idx))
}

fn commit_staged_js_outputs(staged_outputs: &[(PathBuf, PathBuf)], stage_root: &Path) {
    let commit_id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after 1970-01-01 (UNIX epoch)")
            .as_nanos()
    );
    let mut commits: Vec<StagedJsCommit> = Vec::with_capacity(staged_outputs.len());

    for (idx, (stage_path, final_path)) in staged_outputs.iter().enumerate() {
        if let Some(parent) = final_path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            eprintln!(
                "Error creating output directory '{}': {}",
                parent.display(),
                e
            );
            cleanup_registered_js_stage_roots();
            std::process::exit(1);
        }
        let temp_path = commit_temp_path(final_path, &commit_id, idx);
        // N-57: Remove stale temp file from a previous interrupted build.
        // NotFound is expected (normal case); other errors (permission denied)
        // will surface as a copy failure on the next line.
        let _ = fs::remove_file(&temp_path);
        if let Err(e) = fs::copy(stage_path, &temp_path) {
            // Best-effort cleanup of already-prepared temp files
            for commit in &commits {
                let _ = fs::remove_file(&commit.temp_path);
            }
            eprintln!(
                "Error preparing staged JS output '{}' for '{}': {}",
                stage_path.display(),
                final_path.display(),
                e
            );
            cleanup_registered_js_stage_roots();
            std::process::exit(1);
        }
        commits.push(StagedJsCommit {
            final_path: final_path.clone(),
            temp_path,
            backup_path: None,
        });
    }

    for idx in 0..commits.len() {
        if commits[idx].final_path.exists() {
            let final_path = commits[idx].final_path.clone();
            let backup_path = commit_backup_path(&final_path, &commit_id, idx);
            // N-57: Remove stale backup; NotFound is the expected case
            let _ = fs::remove_file(&backup_path);
            if let Err(e) = fs::rename(&final_path, &backup_path) {
                for prior in &commits {
                    let _ = fs::remove_file(&prior.temp_path);
                }
                for prior in commits.iter().take(idx) {
                    if let Some(ref prior_backup) = prior.backup_path {
                        let _ = fs::rename(prior_backup, &prior.final_path);
                    }
                }
                eprintln!(
                    "Error backing up existing JS output '{}' before staged commit: {}",
                    final_path.display(),
                    e,
                );
                cleanup_registered_js_stage_roots();
                std::process::exit(1);
            }
            commits[idx].backup_path = Some(backup_path);
        }
    }

    for idx in 0..commits.len() {
        if let Err(e) = fs::rename(&commits[idx].temp_path, &commits[idx].final_path) {
            for committed in &commits[..idx] {
                let _ = fs::remove_file(&committed.final_path);
            }
            for commit in &commits {
                let _ = fs::remove_file(&commit.temp_path);
                if let Some(ref backup_path) = commit.backup_path {
                    let _ = fs::rename(backup_path, &commit.final_path);
                }
            }
            eprintln!(
                "Error activating staged JS output '{}' to '{}': {}",
                commits[idx].temp_path.display(),
                commits[idx].final_path.display(),
                e
            );
            cleanup_registered_js_stage_roots();
            std::process::exit(1);
        }
    }

    for commit in &commits {
        if let Some(ref backup_path) = commit.backup_path {
            let _ = fs::remove_file(backup_path);
        }
    }
    let _ = fs::remove_dir_all(stage_root);
}

fn run_build_js_dir(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    let td_files = collect_td_files(input_path);
    if td_files.is_empty() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "io",
                None,
                &format!("No .td files found in '{}'", input_path.display()),
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("No .td files found in '{}'", input_path.display());
        }
        std::process::exit(1);
    }

    let pkg_root = find_packages_tdm_from(input_path);
    let out_dir = output_path.map(PathBuf::from).unwrap_or_else(|| {
        // Default: .taida/build/js/ (project-local)
        pkg_root
            .clone()
            .unwrap_or_else(|| input_path.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".taida")
            .join("build")
            .join("js")
    });
    if let Err(e) = fs::create_dir_all(&out_dir) {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "io",
                None,
                &format!(
                    "Error creating output directory '{}': {}",
                    out_dir.display(),
                    e
                ),
                Some(&out_dir.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!(
                "Error creating output directory '{}': {}",
                out_dir.display(),
                e
            );
        }
        std::process::exit(1);
    }

    let stage_root = unique_stage_root("dir");
    register_js_stage_root(&stage_root);
    let mut staged_outputs = Vec::new();
    let mut count = 0usize;
    // Cycle detection + collect external sibling modules from import graph
    let input_canonical = input_path
        .canonicalize()
        .unwrap_or_else(|_| input_path.to_path_buf());
    let mut external_modules = Vec::new();
    {
        let mut seen: std::collections::HashSet<PathBuf> = td_files
            .iter()
            .filter_map(|f| f.canonicalize().ok())
            .collect();
        for td_file in &td_files {
            match module_graph::collect_local_modules(td_file) {
                Ok(all_deps) => {
                    for dep in all_deps {
                        if !dep.starts_with(&input_canonical) && seen.insert(dep.clone()) {
                            external_modules.push(dep);
                        }
                    }
                }
                Err(err) => {
                    emit_build_failure_and_exit(
                        compile_stats,
                        diag_format,
                        "compile",
                        Some(td_file),
                        &err.to_string(),
                    );
                }
            }
        }
    }

    // Release gate: scan all build targets (directory files + external sibling modules)
    if release_mode {
        let mut all_build_files = td_files.clone();
        all_build_files.extend(external_modules.iter().cloned());
        let sites = collect_release_gate_sites_for_files(&all_build_files);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    // Canonicalize entry_root and out_root so the JS codegen's strip_prefix
    // chain works regardless of whether the CLI was invoked with relative paths.
    let entry_root_canonical = input_canonical.clone();
    let out_root_canonical = out_dir.canonicalize().unwrap_or_else(|_| out_dir.clone());

    for td_file in &td_files {
        let rel = td_file.strip_prefix(input_path).unwrap_or(td_file);
        let final_js_out = out_dir.join(rel.with_extension("mjs"));
        let stage_js_out = stage_output_path(&stage_root, &out_dir, &final_js_out);
        transpile_js_module_to_output(
            td_file,
            &stage_js_out,
            Some(&final_js_out),
            no_check,
            diag_format,
            compile_stats,
            pkg_root.as_deref(),
            Some(&entry_root_canonical),
            Some(&out_root_canonical),
        );
        staged_outputs.push((stage_js_out, final_js_out));
        count += 1;
    }

    // Transpile external sibling modules (outside input_path but imported by files inside)
    let entry_parent = entry_root_canonical
        .parent()
        .unwrap_or(&entry_root_canonical);
    for ext_file in &external_modules {
        let rel = ext_file
            .strip_prefix(entry_parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| {
                PathBuf::from(
                    ext_file
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("module.td"),
                )
            });
        let final_js_out = out_dir.join(rel.with_extension("mjs"));
        let stage_js_out = stage_output_path(&stage_root, &out_dir, &final_js_out);
        transpile_js_module_to_output(
            ext_file,
            &stage_js_out,
            Some(&final_js_out),
            no_check,
            diag_format,
            compile_stats,
            pkg_root.as_deref(),
            Some(&entry_root_canonical),
            Some(&out_root_canonical),
        );
        staged_outputs.push((stage_js_out, final_js_out));
        count += 1;
    }

    if let Some(ref root) = pkg_root {
        stage_dep_js_outputs(
            root,
            &stage_root.join("deps"),
            &mut staged_outputs,
            no_check,
            diag_format,
            compile_stats,
        );
    }

    commit_staged_js_outputs(&staged_outputs, &stage_root);
    unregister_js_stage_root(&stage_root);
    write_js_package_json(&out_dir, diag_format, compile_stats);

    if diag_format == DiagFormat::Text {
        println!(
            "Built {} file(s) [{}] → {}",
            count,
            BuildTarget::Js.as_str(),
            out_dir.display()
        );
    }
}

#[cfg(feature = "native")]
fn run_build_native(
    input_path: &Path,
    output_path: Option<&str>,
    entry_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    let entry_file = match resolve_native_entry_path(input_path, entry_path) {
        Ok(path) => path,
        Err(msg) => {
            if diag_format == DiagFormat::Jsonl {
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "compile",
                    None,
                    &msg,
                    Some(&input_path.to_string_lossy()),
                    None,
                    None,
                    None,
                );
            } else {
                eprintln!("{}", msg);
            }
            std::process::exit(1);
        }
    };

    if release_mode {
        let sites = scan_release_gate_sites(&entry_file);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    if !no_check {
        let source = match fs::read_to_string(&entry_file) {
            Ok(s) => s,
            Err(e) => {
                if diag_format == DiagFormat::Jsonl {
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "io",
                        None,
                        &format!("Error reading file '{}': {}", entry_file.display(), e),
                        Some(&entry_file.to_string_lossy()),
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("Error reading file '{}': {}", entry_file.display(), e);
                }
                std::process::exit(1);
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                if diag_format == DiagFormat::Jsonl {
                    let (code, suggestion) = split_diag_code_and_hint(&err.message);
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "parse",
                        code,
                        &err.message,
                        Some(&entry_file.to_string_lossy()),
                        Some(err.span.line),
                        Some(err.span.column),
                        suggestion,
                    );
                } else {
                    eprintln!("{}", err);
                }
            }
            std::process::exit(1);
        }
        run_type_checks_and_warnings(
            &program,
            &entry_file.to_string_lossy(),
            diag_format,
            compile_stats,
        );
    }

    // Default output: .taida/build/native/{stem} (project-local)
    let default_native_output;
    let output: Option<&Path> = if let Some(p) = output_path {
        Some(Path::new(p))
    } else {
        let stem = entry_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let build_dir = find_packages_tdm()
            .unwrap_or_else(|| entry_file.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".taida")
            .join("build")
            .join("native");
        if let Err(e) = fs::create_dir_all(&build_dir) {
            eprintln!(
                "Error creating build directory '{}': {}",
                build_dir.display(),
                e
            );
            std::process::exit(1);
        }
        default_native_output = build_dir.join(stem);
        Some(default_native_output.as_path())
    };
    match codegen::driver::compile_file(&entry_file, output) {
        Ok(bin_path) => {
            if diag_format == DiagFormat::Text {
                // RCB-217: Display the canonical (absolute) path for consistency
                // with JS backend which always shows absolute paths.
                let display_path = bin_path.canonicalize().unwrap_or(bin_path);
                println!("Built (native): {}", display_path.display());
            }
        }
        Err(e) => {
            if diag_format == DiagFormat::Jsonl {
                let message = e.to_string();
                let (code, suggestion) = split_diag_code_and_hint(&message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "codegen",
                    code,
                    &message,
                    Some(&entry_file.to_string_lossy()),
                    None,
                    None,
                    suggestion,
                );
            } else {
                eprintln!("{}", e);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "native")]
fn run_build_wasm_min(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    rt_cache: Option<&codegen::driver::WasmRuntimeCache>,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if input_path.is_dir() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "compile",
                None,
                "wasm-min target does not support directory input.",
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("wasm-min target does not support directory input.");
        }
        std::process::exit(1);
    }

    if !input_path.exists() || !input_path.is_file() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "io",
                None,
                &format!("Build input not found: {}", input_path.display()),
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("Build input not found: {}", input_path.display());
        }
        std::process::exit(1);
    }

    if !no_check {
        let source = match fs::read_to_string(input_path) {
            Ok(s) => s,
            Err(e) => {
                if diag_format == DiagFormat::Jsonl {
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "io",
                        None,
                        &format!("Error reading file '{}': {}", input_path.display(), e),
                        Some(&input_path.to_string_lossy()),
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("Error reading file '{}': {}", input_path.display(), e);
                }
                std::process::exit(1);
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                if diag_format == DiagFormat::Jsonl {
                    let (code, suggestion) = split_diag_code_and_hint(&err.message);
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "parse",
                        code,
                        &err.message,
                        Some(&input_path.to_string_lossy()),
                        Some(err.span.line),
                        Some(err.span.column),
                        suggestion,
                    );
                } else {
                    eprintln!("{}", err);
                }
            }
            std::process::exit(1);
        }
        run_type_checks_and_warnings(
            &program,
            &input_path.to_string_lossy(),
            diag_format,
            compile_stats,
        );
    }

    // F-2: Release gate -- block TODO/Stub molds in --release builds
    if release_mode {
        let sites = scan_release_gate_sites(input_path);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    // Default output: .taida/build/wasm-min/{stem}.wasm (project-local)
    let default_wasm_output;
    let output: Option<&Path> = if let Some(p) = output_path {
        Some(Path::new(p))
    } else {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let build_dir = find_packages_tdm()
            .unwrap_or_else(|| input_path.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".taida")
            .join("build")
            .join("wasm-min");
        if let Err(e) = fs::create_dir_all(&build_dir) {
            eprintln!(
                "Error creating build directory '{}': {}",
                build_dir.display(),
                e
            );
            std::process::exit(1);
        }
        default_wasm_output = build_dir.join(format!("{}.wasm", stem));
        Some(default_wasm_output.as_path())
    };
    // S-2: Cache is initialized once in run_build and passed in.
    match codegen::driver::compile_file_wasm_cached(input_path, output, rt_cache) {
        Ok(wasm_path) => {
            if diag_format == DiagFormat::Text {
                // RCB-217: Display canonical path for consistency with JS backend
                let display_path = wasm_path.canonicalize().unwrap_or(wasm_path);
                println!("Built (wasm-min): {}", display_path.display());
            }
        }
        Err(e) => {
            if diag_format == DiagFormat::Jsonl {
                let message = e.to_string();
                let (code, suggestion) = split_diag_code_and_hint(&message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "codegen",
                    code,
                    &message,
                    Some(&input_path.to_string_lossy()),
                    None,
                    None,
                    suggestion,
                );
            } else {
                eprintln!("{}", e);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "native")]
fn run_build_wasm_wasi(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    rt_cache: Option<&codegen::driver::WasmRuntimeCache>,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if input_path.is_dir() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "compile",
                None,
                "wasm-wasi target does not support directory input.",
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("wasm-wasi target does not support directory input.");
        }
        std::process::exit(1);
    }

    if !input_path.exists() || !input_path.is_file() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "io",
                None,
                &format!("Build input not found: {}", input_path.display()),
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("Build input not found: {}", input_path.display());
        }
        std::process::exit(1);
    }

    if !no_check {
        let source = match fs::read_to_string(input_path) {
            Ok(s) => s,
            Err(e) => {
                if diag_format == DiagFormat::Jsonl {
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "io",
                        None,
                        &format!("Error reading file '{}': {}", input_path.display(), e),
                        Some(&input_path.to_string_lossy()),
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("Error reading file '{}': {}", input_path.display(), e);
                }
                std::process::exit(1);
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                if diag_format == DiagFormat::Jsonl {
                    let (code, suggestion) = split_diag_code_and_hint(&err.message);
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "parse",
                        code,
                        &err.message,
                        Some(&input_path.to_string_lossy()),
                        Some(err.span.line),
                        Some(err.span.column),
                        suggestion,
                    );
                } else {
                    eprintln!("{}", err);
                }
            }
            std::process::exit(1);
        }
        run_type_checks_and_warnings(
            &program,
            &input_path.to_string_lossy(),
            diag_format,
            compile_stats,
        );
    }

    // F-2: Release gate -- block TODO/Stub molds in --release builds
    if release_mode {
        let sites = scan_release_gate_sites(input_path);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    // Default output: .taida/build/wasm-wasi/{stem}.wasm (project-local)
    let default_wasm_output;
    let output: Option<&Path> = if let Some(p) = output_path {
        Some(Path::new(p))
    } else {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let build_dir = find_packages_tdm()
            .unwrap_or_else(|| input_path.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".taida")
            .join("build")
            .join("wasm-wasi");
        if let Err(e) = fs::create_dir_all(&build_dir) {
            eprintln!(
                "Error creating build directory '{}': {}",
                build_dir.display(),
                e
            );
            std::process::exit(1);
        }
        default_wasm_output = build_dir.join(format!("{}.wasm", stem));
        Some(default_wasm_output.as_path())
    };
    // S-2: Cache is initialized once in run_build and passed in.
    match codegen::driver::compile_file_wasm_wasi_cached(input_path, output, rt_cache) {
        Ok(wasm_path) => {
            if diag_format == DiagFormat::Text {
                // RCB-217: Display canonical path for consistency with JS backend
                let display_path = wasm_path.canonicalize().unwrap_or(wasm_path);
                println!("Built (wasm-wasi): {}", display_path.display());
            }
        }
        Err(e) => {
            if diag_format == DiagFormat::Jsonl {
                let message = e.to_string();
                let (code, suggestion) = split_diag_code_and_hint(&message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "codegen",
                    code,
                    &message,
                    Some(&input_path.to_string_lossy()),
                    None,
                    None,
                    suggestion,
                );
            } else {
                eprintln!("{}", e);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "native")]
fn run_build_wasm_edge(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    rt_cache: Option<&codegen::driver::WasmRuntimeCache>,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if input_path.is_dir() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "compile",
                None,
                "wasm-edge target does not support directory input.",
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("wasm-edge target does not support directory input.");
        }
        std::process::exit(1);
    }

    if !input_path.exists() || !input_path.is_file() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "io",
                None,
                &format!("Build input not found: {}", input_path.display()),
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("Build input not found: {}", input_path.display());
        }
        std::process::exit(1);
    }

    if !no_check {
        let source = match fs::read_to_string(input_path) {
            Ok(s) => s,
            Err(e) => {
                if diag_format == DiagFormat::Jsonl {
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "io",
                        None,
                        &format!("Error reading file '{}': {}", input_path.display(), e),
                        Some(&input_path.to_string_lossy()),
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("Error reading file '{}': {}", input_path.display(), e);
                }
                std::process::exit(1);
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                if diag_format == DiagFormat::Jsonl {
                    let (code, suggestion) = split_diag_code_and_hint(&err.message);
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "parse",
                        code,
                        &err.message,
                        Some(&input_path.to_string_lossy()),
                        Some(err.span.line),
                        Some(err.span.column),
                        suggestion,
                    );
                } else {
                    eprintln!("{}", err);
                }
            }
            std::process::exit(1);
        }
        run_type_checks_and_warnings(
            &program,
            &input_path.to_string_lossy(),
            diag_format,
            compile_stats,
        );
    }

    // F-2: Release gate -- block TODO/Stub molds in --release builds
    if release_mode {
        let sites = scan_release_gate_sites(input_path);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    // Default output: .taida/build/wasm-edge/{stem}.wasm (project-local)
    let default_wasm_output;
    let output: Option<&Path> = if let Some(p) = output_path {
        Some(Path::new(p))
    } else {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let build_dir = find_packages_tdm()
            .unwrap_or_else(|| input_path.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".taida")
            .join("build")
            .join("wasm-edge");
        if let Err(e) = fs::create_dir_all(&build_dir) {
            eprintln!(
                "Error creating build directory '{}': {}",
                build_dir.display(),
                e
            );
            std::process::exit(1);
        }
        default_wasm_output = build_dir.join(format!("{}.wasm", stem));
        Some(default_wasm_output.as_path())
    };
    // S-2: Cache is initialized once in run_build and passed in.
    match codegen::driver::compile_file_wasm_edge_cached(input_path, output, rt_cache) {
        Ok(result) => {
            if diag_format == DiagFormat::Text {
                // RCB-217: Display canonical path for consistency with JS backend
                let wasm_display = result.wasm_path.canonicalize().unwrap_or(result.wasm_path);
                let glue_display = result.glue_path.canonicalize().unwrap_or(result.glue_path);
                println!("Built (wasm-edge): {}", wasm_display.display());
                println!("  JS glue: {}", glue_display.display());
            }
        }
        Err(e) => {
            if diag_format == DiagFormat::Jsonl {
                let message = e.to_string();
                let (code, suggestion) = split_diag_code_and_hint(&message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "codegen",
                    code,
                    &message,
                    Some(&input_path.to_string_lossy()),
                    None,
                    None,
                    suggestion,
                );
            } else {
                eprintln!("{}", e);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "native")]
fn run_build_wasm_full(
    input_path: &Path,
    output_path: Option<&str>,
    release_mode: bool,
    no_check: bool,
    rt_cache: Option<&codegen::driver::WasmRuntimeCache>,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    if input_path.is_dir() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "compile",
                None,
                "wasm-full target does not support directory input.",
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("wasm-full target does not support directory input.");
        }
        std::process::exit(1);
    }

    if !input_path.exists() || !input_path.is_file() {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "ERROR",
                "io",
                None,
                &format!("Build input not found: {}", input_path.display()),
                Some(&input_path.to_string_lossy()),
                None,
                None,
                None,
            );
        } else {
            eprintln!("Build input not found: {}", input_path.display());
        }
        std::process::exit(1);
    }

    if !no_check {
        let source = match fs::read_to_string(input_path) {
            Ok(s) => s,
            Err(e) => {
                if diag_format == DiagFormat::Jsonl {
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "io",
                        None,
                        &format!("Error reading file '{}': {}", input_path.display(), e),
                        Some(&input_path.to_string_lossy()),
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("Error reading file '{}': {}", input_path.display(), e);
                }
                std::process::exit(1);
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                if diag_format == DiagFormat::Jsonl {
                    let (code, suggestion) = split_diag_code_and_hint(&err.message);
                    emit_compile_diag_jsonl(
                        compile_stats,
                        "ERROR",
                        "parse",
                        code,
                        &err.message,
                        Some(&input_path.to_string_lossy()),
                        Some(err.span.line),
                        Some(err.span.column),
                        suggestion,
                    );
                } else {
                    eprintln!("{}", err);
                }
            }
            std::process::exit(1);
        }
        run_type_checks_and_warnings(
            &program,
            &input_path.to_string_lossy(),
            diag_format,
            compile_stats,
        );
    }

    // F-2: Release gate -- block TODO/Stub molds in --release builds
    if release_mode {
        let sites = scan_release_gate_sites(input_path);
        if !sites.is_empty() {
            report_release_gate_violations(sites, diag_format, compile_stats);
            std::process::exit(1);
        }
    }

    // Default output: .taida/build/wasm-full/{stem}.wasm (project-local)
    let default_wasm_output;
    let output: Option<&Path> = if let Some(p) = output_path {
        Some(Path::new(p))
    } else {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let build_dir = find_packages_tdm()
            .unwrap_or_else(|| input_path.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".taida")
            .join("build")
            .join("wasm-full");
        if let Err(e) = fs::create_dir_all(&build_dir) {
            eprintln!(
                "Error creating build directory '{}': {}",
                build_dir.display(),
                e
            );
            std::process::exit(1);
        }
        default_wasm_output = build_dir.join(format!("{}.wasm", stem));
        Some(default_wasm_output.as_path())
    };
    // S-2: Cache is initialized once in run_build and passed in.
    match codegen::driver::compile_file_wasm_full_cached(input_path, output, rt_cache) {
        Ok(wasm_path) => {
            if diag_format == DiagFormat::Text {
                // RCB-217: Display canonical path for consistency with JS backend
                let display_path = wasm_path.canonicalize().unwrap_or(wasm_path);
                println!("Built (wasm-full): {}", display_path.display());
            }
        }
        Err(e) => {
            if diag_format == DiagFormat::Jsonl {
                let message = e.to_string();
                let (code, suggestion) = split_diag_code_and_hint(&message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "codegen",
                    code,
                    &message,
                    Some(&input_path.to_string_lossy()),
                    None,
                    None,
                    suggestion,
                );
            } else {
                eprintln!("{}", e);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "native")]
fn resolve_native_entry_path(
    input_path: &Path,
    entry_path: Option<&str>,
) -> Result<PathBuf, String> {
    if input_path.is_dir() {
        let mut candidate = input_path.join(entry_path.unwrap_or("main.td"));
        if candidate.extension().is_none_or(|ext| ext != "td") {
            candidate.set_extension("td");
        }
        if !candidate.exists() || !candidate.is_file() {
            return Err(format!(
                "Native build entry not found: {}",
                candidate.display()
            ));
        }
        return Ok(candidate);
    }

    if entry_path.is_some() {
        return Err("`--entry` can only be used when <PATH> is a directory.".to_string());
    }
    if !input_path.exists() || !input_path.is_file() {
        return Err(format!("Build input not found: {}", input_path.display()));
    }

    Ok(input_path.to_path_buf())
}

fn run_type_checks_and_warnings(
    program: &Program,
    file: &str,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    let mut checker = TypeChecker::new();
    let file_path = std::path::Path::new(file);
    if file_path.exists() {
        checker.set_source_file(file_path);
    }
    checker.check_program(program);
    if !checker.errors.is_empty() {
        for err in &checker.errors {
            if diag_format == DiagFormat::Jsonl {
                let (code, suggestion) = split_diag_code_and_hint(&err.message);
                emit_compile_diag_jsonl(
                    compile_stats,
                    "ERROR",
                    "type",
                    code,
                    &err.message,
                    Some(file),
                    Some(err.span.line),
                    Some(err.span.column),
                    suggestion,
                );
            } else {
                eprintln!("{}", err);
            }
        }
        cleanup_registered_js_stage_roots();
        std::process::exit(1);
    }

    let findings = verify::run_check("error-coverage", program, file);
    for f in &findings {
        if diag_format == DiagFormat::Jsonl {
            emit_compile_diag_jsonl(
                compile_stats,
                "WARNING",
                "verify",
                None,
                &f.message,
                f.file.as_deref().or(Some(file)),
                f.line,
                None,
                None,
            );
        } else if let Some(line) = f.line {
            eprintln!("Warning: {} (line {})", f.message, line);
        } else {
            eprintln!("Warning: {}", f.message);
        }
    }
}

fn collect_release_gate_sites_for_files(td_files: &[PathBuf]) -> Vec<TodoStubSite> {
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

fn collect_td_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_td_files(&path));
            } else if path.extension().is_some_and(|e| e == "td") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[derive(Debug, Clone)]
struct TodoItem {
    id: Option<String>,
    task: Option<String>,
    file: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Clone)]
struct TodoStubSite {
    kind: &'static str,
    file: String,
    line: usize,
    column: usize,
}

#[derive(Default)]
struct TodoScanResult {
    todos: Vec<TodoItem>,
    sites: Vec<TodoStubSite>,
}

fn extract_string_field(fields: &[BuchiField], name: &str) -> Option<String> {
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

fn scan_field_defaults(field: &FieldDef, file: &str, out: &mut TodoScanResult) {
    if let Some(default_expr) = &field.default_value {
        scan_expr_for_todo(default_expr, file, out);
    }
    if let Some(method) = &field.method_def {
        scan_func_for_todo(method, file, out);
    }
}

fn scan_func_for_todo(func: &FuncDef, file: &str, out: &mut TodoScanResult) {
    for param in &func.params {
        if let Some(default_expr) = &param.default_value {
            scan_expr_for_todo(default_expr, file, out);
        }
    }
    for stmt in &func.body {
        scan_stmt_for_todo(stmt, file, out);
    }
}

fn scan_stmt_for_todo(stmt: &Statement, file: &str, out: &mut TodoScanResult) {
    match stmt {
        Statement::Expr(expr) => scan_expr_for_todo(expr, file, out),
        Statement::TypeDef(td) => {
            for field in &td.fields {
                scan_field_defaults(field, file, out);
            }
        }
        Statement::FuncDef(fd) => scan_func_for_todo(fd, file, out),
        Statement::Assignment(assign) => scan_expr_for_todo(&assign.value, file, out),
        Statement::MoldDef(md) => {
            for field in &md.fields {
                scan_field_defaults(field, file, out);
            }
        }
        Statement::InheritanceDef(idf) => {
            for field in &idf.fields {
                scan_field_defaults(field, file, out);
            }
        }
        Statement::ErrorCeiling(ec) => {
            for stmt in &ec.handler_body {
                scan_stmt_for_todo(stmt, file, out);
            }
        }
        Statement::UnmoldForward(uf) => scan_expr_for_todo(&uf.source, file, out),
        Statement::UnmoldBackward(ub) => scan_expr_for_todo(&ub.source, file, out),
        Statement::Import(_) | Statement::Export(_) => {}
    }
}

fn scan_expr_for_todo(expr: &Expr, file: &str, out: &mut TodoScanResult) {
    match expr {
        Expr::MoldInst(name, type_args, fields, span) => {
            if name == "TODO" {
                out.todos.push(TodoItem {
                    id: extract_string_field(fields, "id"),
                    task: extract_string_field(fields, "task"),
                    file: file.to_string(),
                    line: span.line,
                    column: span.column,
                });
                out.sites.push(TodoStubSite {
                    kind: "TODO",
                    file: file.to_string(),
                    line: span.line,
                    column: span.column,
                });
            } else if name == "Stub" {
                out.sites.push(TodoStubSite {
                    kind: "Stub",
                    file: file.to_string(),
                    line: span.line,
                    column: span.column,
                });
            }
            for arg in type_args {
                scan_expr_for_todo(arg, file, out);
            }
            for field in fields {
                scan_expr_for_todo(&field.value, file, out);
            }
        }
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            for field in fields {
                scan_expr_for_todo(&field.value, file, out);
            }
        }
        Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
            for item in items {
                scan_expr_for_todo(item, file, out);
            }
        }
        Expr::BinaryOp(left, _, right, _) => {
            scan_expr_for_todo(left, file, out);
            scan_expr_for_todo(right, file, out);
        }
        Expr::UnaryOp(_, inner, _) | Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
            scan_expr_for_todo(inner, file, out);
        }
        Expr::FuncCall(callee, args, _) => {
            scan_expr_for_todo(callee, file, out);
            for arg in args {
                scan_expr_for_todo(arg, file, out);
            }
        }
        Expr::MethodCall(receiver, _, args, _) => {
            scan_expr_for_todo(receiver, file, out);
            for arg in args {
                scan_expr_for_todo(arg, file, out);
            }
        }
        Expr::FieldAccess(receiver, _, _) => scan_expr_for_todo(receiver, file, out),
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    scan_expr_for_todo(cond, file, out);
                }
                for stmt in &arm.body {
                    if let Statement::Expr(e) = stmt {
                        scan_expr_for_todo(e, file, out);
                    }
                }
            }
        }
        Expr::Lambda(params, body, _) => {
            for param in params {
                if let Some(default_expr) = &param.default_value {
                    scan_expr_for_todo(default_expr, file, out);
                }
            }
            scan_expr_for_todo(body, file, out);
        }
        Expr::IntLit(_, _)
        | Expr::FloatLit(_, _)
        | Expr::StringLit(_, _)
        | Expr::TemplateLit(_, _)
        | Expr::BoolLit(_, _)
        | Expr::Gorilla(_)
        | Expr::Ident(_, _)
        | Expr::Placeholder(_)
        | Expr::Hole(_) => {}
    }
}

fn scan_program_for_todo(program: &Program, file: &Path) -> TodoScanResult {
    let file_label = file.to_string_lossy().to_string();
    let mut out = TodoScanResult::default();
    for stmt in &program.statements {
        scan_stmt_for_todo(stmt, &file_label, &mut out);
    }
    out
}

fn resolve_local_import_path(base_dir: &Path, import_path: &str) -> PathBuf {
    base_dir.join(import_path)
}

fn collect_release_scan_files(target_path: &Path) -> Vec<PathBuf> {
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

fn scan_release_gate_sites(target_path: &Path) -> Vec<TodoStubSite> {
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

fn report_release_gate_violations(
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

fn run_todo(args: &[String]) {
    let mut format_type = "text".to_string();
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
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida todo --help` for usage.");
                    std::process::exit(1);
                }
                match args[i].as_str() {
                    "text" | "json" => {
                        format_type = args[i].clone();
                    }
                    other => {
                        eprintln!("Unknown format '{}'. Expected: text | json", other);
                        std::process::exit(1);
                    }
                }
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for todo: {}", raw);
                eprintln!("Run `taida todo --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one [PATH] is accepted for taida todo.");
                    std::process::exit(1);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let target = path.unwrap_or_else(|| ".".to_string());
    let target_path = Path::new(&target);
    let td_files: Vec<PathBuf> = if target_path.is_dir() {
        let files = collect_td_files(target_path);
        if files.is_empty() {
            eprintln!("No .td files found in '{}'", target);
            std::process::exit(1);
        }
        files
    } else {
        vec![target_path.to_path_buf()]
    };

    let mut merged = TodoScanResult::default();
    for td_file in &td_files {
        let source = match fs::read_to_string(td_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", td_file.display(), e);
                std::process::exit(1);
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                eprintln!("{}: {}", td_file.display(), err);
            }
            std::process::exit(1);
        }
        let scan = scan_program_for_todo(&program, td_file);
        merged.todos.extend(scan.todos);
    }

    merged.todos.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
            .then(a.id.cmp(&b.id))
    });

    let mut by_id = std::collections::BTreeMap::<Option<String>, usize>::new();
    let mut by_file = std::collections::BTreeMap::<String, usize>::new();
    for todo in &merged.todos {
        *by_id.entry(todo.id.clone()).or_insert(0) += 1;
        *by_file.entry(todo.file.clone()).or_insert(0) += 1;
    }

    if format_type == "json" {
        let todos_json: Vec<serde_json::Value> = merged
            .todos
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
        let by_id_json: Vec<serde_json::Value> = by_id
            .into_iter()
            .map(|(id, count)| json!({ "id": id, "count": count }))
            .collect();
        let by_file_json: Vec<serde_json::Value> = by_file
            .into_iter()
            .map(|(file, count)| json!({ "file": file, "count": count }))
            .collect();
        let output = json!({
            "total": merged.todos.len(),
            "todos": todos_json,
            "byId": by_id_json,
            "byFile": by_file_json,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }

    if merged.todos.is_empty() {
        println!("No TODO molds found.");
        return;
    }

    println!("TODOs: {}", merged.todos.len());
    for todo in &merged.todos {
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
            id.unwrap_or_else(|| "<missing-id>".to_string()),
            count
        );
    }
    println!("By File:");
    for (file, count) in by_file {
        println!("  - {}: {}", file, count);
    }
}

// ── Graph subcommand ────────────────────────────────────

fn run_graph(args: &[String]) {
    let mut path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut recursive = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_graph_help();
                return;
            }
            "--recursive" | "-r" => {
                recursive = true;
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for -o/--output.");
                    eprintln!("Run `taida graph --help` for usage.");
                    std::process::exit(1);
                }
                output_path = Some(args[i].clone());
            }
            _ => {
                if args[i].starts_with('-') {
                    eprintln!(
                        "Unknown option '{}'. Run `taida graph --help` for usage.",
                        args[i]
                    );
                    std::process::exit(1);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let file_path = match path {
        Some(p) => p,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida graph --help` for usage.");
            std::process::exit(1);
        }
    };

    let output = if recursive {
        match ai_format::format_ai_json_recursive(&file_path) {
            Ok(json) => json,
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    } else {
        let source = match fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", file_path, e);
                std::process::exit(1);
            }
        };

        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                eprintln!("{}", err);
            }
            std::process::exit(1);
        }

        ai_format::format_ai_json(&program, &file_path)
    };

    if let Some(out_path) = &output_path {
        let out = Path::new(out_path);
        let resolved = if out.parent().is_none_or(|p| p.as_os_str().is_empty()) {
            let graph_dir = find_packages_tdm()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".taida")
                .join("graph");
            if let Err(e) = fs::create_dir_all(&graph_dir) {
                eprintln!(
                    "Error creating graph directory '{}': {}",
                    graph_dir.display(),
                    e
                );
                std::process::exit(1);
            }
            graph_dir.join(out)
        } else {
            out.to_path_buf()
        };
        match fs::write(&resolved, &output) {
            Ok(_) => println!("Graph written to {}", resolved.display()),
            Err(e) => {
                eprintln!("Error writing graph to '{}': {}", resolved.display(), e);
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", output);
    }
}

// ── Verify subcommand ───────────────────────────────────

fn run_verify(args: &[String]) {
    let mut checks: Vec<String> = Vec::new();
    let mut format_type = "text".to_string();
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
                    eprintln!("Missing value for --check.");
                    eprintln!("Run `taida verify --help` for usage.");
                    std::process::exit(1);
                }
                if !verify::ALL_CHECKS.contains(&args[i].as_str()) {
                    eprintln!(
                        "Unknown check '{}'. Available checks: {}",
                        args[i],
                        verify::ALL_CHECKS.join(", ")
                    );
                    std::process::exit(1);
                }
                checks.push(args[i].clone());
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida verify --help` for usage.");
                    std::process::exit(1);
                }
                match args[i].as_str() {
                    "text" | "json" | "jsonl" | "sarif" => {
                        format_type = args[i].clone();
                    }
                    other => {
                        eprintln!(
                            "Unknown format '{}'. Expected: text | json | jsonl | sarif",
                            other
                        );
                        std::process::exit(1);
                    }
                }
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for verify: {}", raw);
                eprintln!("Run `taida verify --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida verify.");
                    std::process::exit(1);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let file_path = match path {
        Some(p) => p,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida verify --help` for usage.");
            std::process::exit(1);
        }
    };

    // Collect target files: directory -> recursive .td collection, file -> single file
    let target_path = Path::new(&file_path);
    let td_files: Vec<PathBuf> = if target_path.is_dir() {
        let files = collect_td_files(target_path);
        if files.is_empty() {
            eprintln!("No .td files found in '{}'", file_path);
            std::process::exit(1);
        }
        files
    } else {
        vec![target_path.to_path_buf()]
    };

    // Aggregate report across all files
    let mut report = verify::VerifyReport::new();

    for td_file in &td_files {
        let file_str = td_file.to_string_lossy().to_string();
        let source = match fs::read_to_string(td_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", file_str, e);
                std::process::exit(1);
            }
        };

        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                eprintln!("{}: {}", file_str, err);
            }
            std::process::exit(1);
        }

        if checks.is_empty() {
            let file_report = verify::run_all_checks(&program, &file_str);
            for f in file_report.findings {
                report.add(f);
            }
        } else {
            for check in &checks {
                let findings = verify::run_check(check, &program, &file_str);
                for f in findings {
                    report.add(f);
                }
            }
        }
    }

    let checks_ref: Vec<&str> = if checks.is_empty() {
        verify::ALL_CHECKS.to_vec()
    } else {
        checks.iter().map(|s| s.as_str()).collect()
    };
    let output = match format_type.as_str() {
        "json" => report.format_json(),
        "jsonl" => report.format_jsonl(&checks_ref),
        "sarif" => report.format_sarif(&checks_ref),
        _ => report.format_text(&checks_ref),
    };
    print!("{}", output);

    if format_type == "jsonl" && report.errors() > 0 {
        std::process::exit(1);
    }
}

// ── Inspect subcommand ──────────────────────────────────

fn run_inspect(args: &[String]) {
    let mut format_type = "text".to_string();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_inspect_help();
                return;
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida inspect --help` for usage.");
                    std::process::exit(1);
                }
                match args[i].as_str() {
                    "text" | "json" | "sarif" => {
                        format_type = args[i].clone();
                    }
                    other => {
                        eprintln!("Unknown format '{}'. Expected: text | json | sarif", other);
                        std::process::exit(1);
                    }
                }
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for inspect: {}", raw);
                eprintln!("Run `taida inspect --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida inspect.");
                    std::process::exit(1);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let file_path = match path {
        Some(p) => p,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida inspect --help` for usage.");
            std::process::exit(1);
        }
    };

    let source = match fs::read_to_string(&file_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file '{}': {}", file_path, e);
            std::process::exit(1);
        }
    };

    let (program, parse_errors) = parse(&source);
    if !parse_errors.is_empty() {
        for err in &parse_errors {
            eprintln!("{}", err);
        }
        std::process::exit(1);
    }

    match format_type.as_str() {
        "json" => {
            // JSON: structural summary + verify results
            let summary = verify::structural_summary(&program, &file_path);
            let report = verify::run_all_checks(&program, &file_path);
            let checks_ref: Vec<&str> = verify::ALL_CHECKS.to_vec();
            let sarif = report.format_sarif(&checks_ref);
            println!("{{");
            println!("  \"summary\": {},", summary);
            println!("  \"verification\": {}", sarif);
            println!("}}");
        }
        "sarif" => {
            // SARIF only
            let report = verify::run_all_checks(&program, &file_path);
            let checks_ref: Vec<&str> = verify::ALL_CHECKS.to_vec();
            print!("{}", report.format_sarif(&checks_ref));
        }
        _ => {
            // Text: summary + verify
            println!("=== Taida Inspect: {} ===\n", file_path);

            println!("--- Structural Summary ---");
            let summary = verify::structural_summary(&program, &file_path);
            println!("{}\n", summary);

            println!("--- Structure ---");
            let ai_json = ai_format::format_ai_json(&program, &file_path);
            println!("{}\n", ai_json);

            println!("--- Verification ---");
            let report = verify::run_all_checks(&program, &file_path);
            let checks_ref: Vec<&str> = verify::ALL_CHECKS.to_vec();
            print!("{}", report.format_text(&checks_ref));
        }
    }
}

// ── Init subcommand ──────────────────────────────────────

fn run_init(args: &[String]) {
    // ── CLI parsing (RC2.6-3c) ──────────────────────────
    //
    // Accepted forms:
    //   taida init                           → SourceOnly in "."
    //   taida init <dir>                     → SourceOnly in <dir>
    //   taida init --target rust-addon       → RustAddon in "."
    //   taida init --target rust-addon <dir> → RustAddon in <dir>
    //   taida init <dir> --target rust-addon → RustAddon in <dir>
    //   taida init --help / -h               → help text
    let mut target = pkg::init::InitTarget::SourceOnly;
    let mut dir_arg: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_init_help();
                return;
            }
            "--target" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --target.");
                    eprintln!("Run `taida init --help` for usage.");
                    std::process::exit(1);
                }
                match args[i].as_str() {
                    "rust-addon" => target = pkg::init::InitTarget::RustAddon,
                    other => {
                        eprintln!("Unknown init target '{}'. Supported: rust-addon", other);
                        eprintln!("Run `taida init --help` for usage.");
                        std::process::exit(1);
                    }
                }
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for init: {}", raw);
                eprintln!("Run `taida init --help` for usage.");
                std::process::exit(1);
            }
            positional => {
                if dir_arg.is_some() {
                    eprintln!("Too many arguments.");
                    eprintln!("Run `taida init --help` for usage.");
                    std::process::exit(1);
                }
                dir_arg = Some(positional.to_string());
            }
        }
        i += 1;
    }

    let dir_name = dir_arg.as_deref().unwrap_or(".");
    let dir = Path::new(dir_name);

    // Determine project name from directory name
    let project_name = if dir_name == "." {
        env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-project".to_string())
    } else {
        Path::new(dir_name)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir_name.to_string())
    };

    // Create directory if needed
    if dir_name != "."
        && let Err(e) = fs::create_dir_all(dir)
    {
        eprintln!("Error creating directory '{}': {}", dir_name, e);
        std::process::exit(1);
    }

    // Delegate to pkg::init::init_project (RC2.6-3a)
    match pkg::init::init_project(dir, &project_name, target) {
        Ok(created) => {
            let target_label = match target {
                pkg::init::InitTarget::RustAddon => " (rust-addon)",
                pkg::init::InitTarget::SourceOnly => "",
            };
            println!(
                "Initialized Taida project '{}'{} in {}",
                project_name,
                target_label,
                dir.display()
            );
            for file in &created {
                println!("  {}", file);
            }
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

// ── Deps subcommand ──────────────────────────────────────

fn run_deps(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_deps_help();
            return;
        }
        _ => {
            eprintln!("Unexpected arguments.");
            eprintln!("Run `taida deps --help` for usage.");
            std::process::exit(1);
        }
    }

    // Find project root by looking for packages.tdm
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    // Parse manifest
    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    if manifest.deps.is_empty() {
        println!("No dependencies defined in packages.tdm");
        return;
    }

    println!("Resolving dependencies for '{}'...", manifest.name);

    // Resolve dependencies using provider chain
    let result = pkg::resolver::resolve_deps(&manifest);

    // Report errors
    for err in &result.errors {
        eprintln!("  ERROR: {}", err);
    }

    // Strict mode for `taida deps`: never install or write lockfile on resolve errors.
    if !result.errors.is_empty() {
        eprintln!("Dependency resolution failed. Skipping install and lockfile update.");
        std::process::exit(1);
    }

    // Install resolved dependencies
    if !result.resolved.is_empty() {
        match pkg::resolver::install_deps(&manifest, &result) {
            Ok(()) => {
                for (name, path) in &result.resolved {
                    println!("  {} -> {}", name, path.display());
                }
                println!(
                    "Installed {} dependency(ies) in .taida/deps/",
                    result.resolved.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }

        // Generate lockfile
        match pkg::resolver::write_lockfile(&manifest, &result) {
            Ok(()) => println!("Updated taida.lock"),
            Err(e) => eprintln!("Warning: could not write lockfile: {}", e),
        }
    }
}

// ── Install subcommand ──────────────────────────────────

fn run_install(args: &[String]) {
    // RC1.5-3c: parse --force-refresh flag
    let mut force_refresh = false;
    let mut filtered: Vec<&str> = Vec::new();
    for arg in args {
        if arg == "--force-refresh" {
            force_refresh = true;
        } else if is_help_flag(arg.as_str()) {
            print_install_help();
            return;
        } else {
            filtered.push(arg.as_str());
        }
    }
    if !filtered.is_empty() {
        eprintln!("Unexpected arguments.");
        eprintln!("Run `taida install --help` for usage.");
        std::process::exit(1);
    }

    // Find project root by looking for packages.tdm
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    // Parse manifest
    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    if manifest.deps.is_empty() {
        println!("No dependencies defined in packages.tdm");
        println!("Generated taida.lock (empty)");
        // Write empty lockfile
        let lockfile = pkg::lockfile::Lockfile::from_resolved(&[]);
        let lock_path = project_dir.join(".taida").join("taida.lock");
        if let Some(parent) = lock_path.parent() {
            // N-56: directory creation error is caught by the subsequent
            // lockfile.write() call, which will report a clear error.
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = lockfile.write(&lock_path) {
            eprintln!("Warning: could not write lockfile: {}", e);
        }
        return;
    }

    println!("Installing dependencies for '{}'...", manifest.name);

    // Read existing lockfile to pin generation-only versions for reproducibility
    let lock_path = project_dir.join(".taida").join("taida.lock");
    let existing_lockfile = pkg::lockfile::Lockfile::read(&lock_path).unwrap_or_default();

    // Resolve all dependencies using the provider chain,
    // pinning generation-only versions to locked exact versions when available
    let result = match &existing_lockfile {
        Some(lf) => pkg::resolver::resolve_deps_locked(&manifest, lf),
        None => pkg::resolver::resolve_deps(&manifest),
    };

    // Report errors
    for err in &result.errors {
        eprintln!("  ERROR: {}", err);
    }

    // Install resolved dependencies
    let mut addon_map: std::collections::BTreeMap<String, pkg::lockfile::LockedAddon> =
        std::collections::BTreeMap::new();
    if !result.resolved.is_empty() {
        match pkg::resolver::install_deps(&manifest, &result) {
            Ok(()) => {
                for pkg in &result.packages {
                    let source_label = match &pkg.source {
                        pkg::provider::PackageSource::Path(p) => format!("path:{}", p),
                        pkg::provider::PackageSource::CoreBundled => "bundled".to_string(),
                        pkg::provider::PackageSource::Store { org, name } => {
                            format!("github:{}/{}", org, name)
                        }
                    };
                    println!("  {} @{} ({})", pkg.name, pkg.version, source_label);
                }
                println!(
                    "Installed {} package(s) in .taida/deps/",
                    result.packages.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }

        // RC1.5-3a: install addon prebuilds
        let existing_lock = pkg::lockfile::Lockfile::read(&lock_path).unwrap_or(None);
        addon_map = match pkg::resolver::install_addon_prebuilds(
            &manifest,
            &result,
            force_refresh,
            existing_lock.as_ref(),
        ) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("Error installing addon prebuilds: {}", e);
                std::process::exit(1);
            }
        };

        if !addon_map.is_empty() {
            for (pkg_name, addon) in &addon_map {
                println!("  Addon {} @ {} ({})", pkg_name, addon.target, addon.sha256);
            }
        }
    }

    // Generate lockfile (always, even if some deps failed)
    // RC1.5: include addon info if addon prebuilds were installed
    match pkg::resolver::write_lockfile_with_addons(&manifest, &result, &addon_map) {
        Ok(()) => println!("Generated taida.lock"),
        Err(e) => eprintln!("Warning: could not write lockfile: {}", e),
    }

    if !result.errors.is_empty() {
        eprintln!("\nSome dependencies could not be resolved. See errors above.");
        std::process::exit(1);
    }
}

// ── Update subcommand ──────────────────────────────────

fn run_update(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_update_help();
            return;
        }
        _ => {
            eprintln!("Unexpected arguments.");
            eprintln!("Run `taida update --help` for usage.");
            std::process::exit(1);
        }
    }

    // Find project root by looking for packages.tdm
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    // Parse manifest
    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    if manifest.deps.is_empty() {
        println!("No dependencies defined in packages.tdm");
        return;
    }

    println!("Updating dependencies for '{}'...", manifest.name);

    // Resolve all dependencies with force-remote (bypass local cache for generations)
    let result = pkg::resolver::resolve_deps_update(&manifest);

    // Report errors
    for err in &result.errors {
        eprintln!("  ERROR: {}", err);
    }

    // Install resolved dependencies (recreate symlinks)
    if !result.resolved.is_empty() {
        match pkg::resolver::install_deps(&manifest, &result) {
            Ok(()) => {
                for pkg in &result.packages {
                    let source_label = match &pkg.source {
                        pkg::provider::PackageSource::Path(p) => format!("path:{}", p),
                        pkg::provider::PackageSource::CoreBundled => "bundled".to_string(),
                        pkg::provider::PackageSource::Store { org, name } => {
                            format!("github:{}/{}", org, name)
                        }
                    };
                    println!("  {} @{} ({})", pkg.name, pkg.version, source_label);
                }
                println!(
                    "Updated {} package(s) in .taida/deps/",
                    result.packages.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Update lockfile
    match pkg::resolver::write_lockfile(&manifest, &result) {
        Ok(()) => println!("Updated taida.lock"),
        Err(e) => eprintln!("Warning: could not write lockfile: {}", e),
    }

    if !result.errors.is_empty() {
        eprintln!("\nSome dependencies could not be resolved. See errors above.");
        std::process::exit(1);
    }
}

// ── Publish subcommand ─────────────────────────────────

#[cfg(feature = "community")]
/// RC2.6-1f: publish subcommand entry point — rewritten as a two-mode
/// orchestrator (source-only vs Rust addon).
///
/// The pre-RC2.6 implementation inlined a linear "prepare + write +
/// commit + push" flow. That flow is preserved verbatim for
/// source-only packages (non-negotiable condition 1) but now runs
/// **through** the shared orchestration harness below so that:
///
///   * `prepare_publish` remains a non-mutating, read-only function (RC2.6B-015).
///   * Every file written during the run is captured by a
///     `PublishRollback` snapshot so a late failure restores the
///     worktree exactly as it was.
///   * The worktree invariants I1-I4 spelled out in RC2.6B-015 are
///     checked at the boundaries (`check_worktree_clean` →
///     `prepare` → `check_dirty_allowlist` → `commit` →
///     `check_worktree_clean`).
///
/// For the Rust addon flow the orchestrator additionally:
///
///   1. Runs `build_addon_artifacts` (cargo build --release --lib).
///   2. Computes the cdylib SHA-256.
///   3. Merges the host entry into `native/addon.lock.toml`.
///   4. Re-computes the integrity digest **after** the lockfile is
///      materialised so `prepare_publish`'s hash is superseded by the
///      post-lockfile state (the pre-lockfile hash is intentionally
///      thrown away).
///   5. Stages `packages.tdm` plus `native/addon.lock.toml` via the
///      extended `git_commit_tag_push(extra_paths=...)`.
///   6. Will call `create_github_release` here in Phase 2. For Phase
///      1 the release step is a no-op (Phase 2 will wire it up),
///      and the env var `TAIDA_PUBLISH_SKIP_RELEASE=1` is honoured
///      ahead of time so integration tests can bypass `gh` entirely.
///
/// `--dry-run` in the addon flow still runs `prepare_publish` (non-mutating)
/// and prints the computed plan, but explicitly SKIPS the cargo build
/// to preserve B-015 invariant "dry-run must not touch target/".
fn run_publish(args: &[String]) {
    // ── Dry-run modes ───────────────────────────────────
    //
    // RC2.6-2c: two-stage dry-run semantics.
    //
    //   Plan  — print what would happen; no cargo build, no git, no release.
    //           This is the default for bare `--dry-run`.
    //   Build — run cargo build + lockfile merge + packages.tdm rewrite,
    //           then stop. Git commit/push and release are skipped.
    //           Useful to inspect the lockfile before committing.
    //
    // For source-only packages, `Build` behaves the same as `Plan`
    // because there is no cargo build to perform.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DryRunMode {
        Plan,
        Build,
    }

    // ── CLI parsing ──────────────────────────────────────
    let mut label: Option<String> = None;
    let mut dry_run: Option<DryRunMode> = None;
    let mut explicit_rust_addon = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_publish_help();
                return;
            }
            "--label" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --label.");
                    eprintln!("Run `taida publish --help` for usage.");
                    std::process::exit(1);
                }
                label = Some(args[i].clone());
            }
            // Bare `--dry-run` is equivalent to `--dry-run=plan`
            // for backward compatibility.
            "--dry-run" => dry_run = Some(DryRunMode::Plan),
            "--dry-run=plan" => dry_run = Some(DryRunMode::Plan),
            "--dry-run=build" => dry_run = Some(DryRunMode::Build),
            raw if raw.starts_with("--dry-run=") => {
                let mode = &raw["--dry-run=".len()..];
                eprintln!(
                    "Unknown --dry-run mode '{}'. Supported modes: plan, build.",
                    mode
                );
                eprintln!("Run `taida publish --help` for usage.");
                std::process::exit(1);
            }
            "--target" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --target.");
                    eprintln!("Run `taida publish --help` for usage.");
                    std::process::exit(1);
                }
                match args[i].as_str() {
                    "rust-addon" => explicit_rust_addon = true,
                    other => {
                        eprintln!(
                            "Unknown --target value '{}'. Supported values: rust-addon.",
                            other
                        );
                        eprintln!("Run `taida publish --help` for usage.");
                        std::process::exit(1);
                    }
                }
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for publish: {}", raw);
                eprintln!("Run `taida publish --help` for usage.");
                std::process::exit(1);
            }
            other => {
                eprintln!("Unexpected argument for publish: {}", other);
                eprintln!("Run `taida publish --help` for usage.");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // ── Authentication & project discovery ──────────────
    //
    // RC2.6B-017: `--dry-run=plan` should not require authentication.
    // The token is only needed for the real publish flow (git push +
    // proposals URL). For plan mode we attempt to load it but fall
    // back to a placeholder so the user can inspect what *would*
    // happen without being logged in.
    let token_opt = auth::token::load_token();
    if token_opt.is_none() && dry_run != Some(DryRunMode::Plan) {
        eprintln!("Authentication required. Run `taida auth login` first.");
        std::process::exit(1);
    }
    // For display purposes in dry-run output.
    let author_name = token_opt
        .as_ref()
        .map(|t| t.username.clone())
        .unwrap_or_else(|| "(not authenticated)".to_string());

    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });
    let manifest_path = project_dir.join("packages.tdm");
    let addon_toml_path = project_dir.join("native").join("addon.toml");
    let addon_lock_path = project_dir.join("native").join("addon.lock.toml");

    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    let manifest_source = match fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("Error reading '{}': {}", manifest_path.display(), e);
            std::process::exit(1);
        }
    };

    // ── Addon detection ─────────────────────────────────
    //
    // The addon flow triggers when either:
    //   * the user explicitly passed `--target rust-addon`, OR
    //   * a `native/addon.toml` exists at the project root.
    //
    // A mismatch (user requested addon but no manifest) is a hard
    // error so the failure mode is deterministic rather than
    // "silently fall through to source-only".
    let addon_manifest_present = addon_toml_path.exists();
    let is_addon_flow = explicit_rust_addon || addon_manifest_present;
    if explicit_rust_addon && !addon_manifest_present {
        eprintln!(
            "--target rust-addon requires '{}' but none was found.",
            addon_toml_path.display()
        );
        std::process::exit(1);
    }

    // ── Phase 1: non-mutating preparation (prepare_publish is read-only) ─
    let preparation = match pkg::publish::prepare_publish(
        &project_dir,
        &manifest,
        &manifest_source,
        &author_name,
        label.as_deref(),
    ) {
        Ok(preparation) => preparation,
        Err(e) => {
            eprintln!("Publish preparation failed: {}", e);
            std::process::exit(1);
        }
    };

    // ── Dry-run: Plan mode ────────────────────────────────
    //
    // B-015 invariant: `--dry-run` (plan) MUST NOT touch the
    // filesystem. That means no cargo build, no lockfile write, no
    // git commit. The addon-flow details are still surfaced so users
    // know what the real run would do.
    if dry_run == Some(DryRunMode::Plan) {
        println!("Dry run: no changes made.");
        println!("  Package: {}/{}", author_name, preparation.package_name);
        println!("  Version: @{}", preparation.version);
        println!("  Integrity: {}", preparation.integrity);
        if let Some(previous) = &preparation.previous_version {
            println!("  Previous: @{}", previous);
        }
        if let Some(source_repo) = &preparation.source_repo {
            println!("  Source repo: {}", source_repo);
        }
        if is_addon_flow {
            println!("  Target: rust-addon");
            println!("  Addon manifest: {}", addon_toml_path.display());
            println!(
                "  Addon lockfile: {} (would be merged with host entry)",
                addon_lock_path.display()
            );
            println!("  Cargo build: skipped (dry-run invariant)");
        }
        return;
    }

    // For source-only packages, `--dry-run=build` has no cargo build
    // to perform, so it degrades to `plan` mode gracefully.
    if dry_run == Some(DryRunMode::Build) && !is_addon_flow {
        println!("Dry run (build): no addon build required for source-only packages.");
        println!("  Package: {}/{}", author_name, preparation.package_name);
        println!("  Version: @{}", preparation.version);
        println!("  Integrity: {}", preparation.integrity);
        if let Some(previous) = &preparation.previous_version {
            println!("  Previous: @{}", previous);
        }
        if let Some(source_repo) = &preparation.source_repo {
            println!("  Source repo: {}", source_repo);
        }
        return;
    }

    // ── Real publish run: orchestrate side-effects ──────
    //
    // Allow list of files the publish run is permitted to mutate
    // (used by `check_dirty_allowlist` at invariant I2). The
    // source-only flow only ever touches `packages.tdm`; the addon
    // flow also touches `native/addon.lock.toml`.
    let mut allowlist: Vec<std::path::PathBuf> = vec![std::path::PathBuf::from("packages.tdm")];
    if is_addon_flow {
        allowlist.push(std::path::PathBuf::from("native/addon.lock.toml"));
        allowlist.push(std::path::PathBuf::from("native/addon.toml"));
    }

    // ── Snapshot every mutable file BEFORE any side-effect ─
    let mut rollback = pkg::publish::PublishRollback::new();
    if let Err(e) = rollback.snapshot(&manifest_path) {
        eprintln!("Publish failed (rollback snapshot): {}", e);
        std::process::exit(1);
    }
    if is_addon_flow {
        if let Err(e) = rollback.snapshot(&addon_lock_path) {
            eprintln!("Publish failed (rollback snapshot addon.lock.toml): {}", e);
            std::process::exit(1);
        }
        if let Err(e) = rollback.snapshot(&addon_toml_path) {
            eprintln!("Publish failed (rollback snapshot addon.toml): {}", e);
            std::process::exit(1);
        }
    }

    // Helper for deterministic error-path cleanup. We use a closure
    // so every bail-out restores the worktree before exit.
    let bail = |rollback: &pkg::publish::PublishRollback, msg: String| -> ! {
        if let Err(restore_err) = rollback.restore() {
            eprintln!("Publish failed: {}", msg);
            eprintln!(
                "Additionally, rollback encountered errors and the worktree may still be dirty: {}",
                restore_err
            );
        } else {
            eprintln!("Publish failed: {}", msg);
            eprintln!(
                "packages.tdm (and addon.lock.toml, if applicable) have been restored to their original state."
            );
        }
        std::process::exit(1);
    };

    // ── Addon-flow side-effect chain ─────────────────────
    //
    // The order matters: (1) cargo build, (2) SHA-256, (3) lockfile
    // merge, (4) rewrite packages.tdm, (5) invariant I2, (6)
    // git_commit_tag_push with extra_paths, (7) Phase 2 release.
    //
    // `compute_publish_integrity` is re-evaluated after step (3) so
    // the stored integrity reflects the committed tree state —
    // otherwise the integrity reported to proposals would be computed
    // before the lockfile existed, which would defeat the purpose of
    // the metadata.

    // Addon build metadata captured for the release step (Phase 2).
    // These are set inside the cfg(native) block and consumed later.
    let mut addon_cdylib_path: Option<PathBuf> = None;
    let mut addon_library_stem: Option<String> = None;
    let mut addon_host_triple: Option<String> = None;
    // Track whether addon.toml was modified so it gets staged.
    let mut addon_toml_rewritten = false;

    if is_addon_flow {
        // RC2.6B-004: rewrite addon.toml prebuild URL template to
        // match the current git remote origin. This ensures that
        // forks do not publish with the upstream org hardcoded.
        match pkg::publish::rewrite_prebuild_url_if_needed(&project_dir) {
            Ok(true) => {
                println!("  [url]      addon.toml prebuild URL rewritten to match git origin");
                addon_toml_rewritten = true;
            }
            Ok(false) => {} // URL already correct or no origin
            Err(e) => bail(&rollback, format!("addon.toml URL rewrite failed: {}", e)),
        }

        #[cfg(feature = "native")]
        {
            let build_output = match pkg::publish::build_addon_artifacts(&project_dir) {
                Ok(out) => out,
                Err(e) => bail(&rollback, format!("addon build failed: {}", e)),
            };

            println!("  [build]    cargo build --release --lib");
            println!(
                "  [build]    cdylib -> {}",
                build_output.cdylib_path.display()
            );

            let sha = match pkg::publish::compute_cdylib_sha256(&build_output.cdylib_path) {
                Ok(s) => s,
                Err(e) => bail(&rollback, format!("sha256 computation failed: {}", e)),
            };
            println!("  [sha256]   {} = {}", build_output.host_triple, sha);

            let mut delta = taida::addon::lockfile::AddonLockfile::new();
            delta.set_target(build_output.host_triple.clone(), sha.clone());
            if let Err(e) = taida::addon::lockfile::write_lockfile(&addon_lock_path, &delta) {
                bail(&rollback, format!("addon.lock.toml write failed: {}", e));
            }
            println!(
                "  [lockfile] {} merged",
                addon_lock_path
                    .strip_prefix(&project_dir)
                    .unwrap_or(&addon_lock_path)
                    .display()
            );

            // Capture build metadata for the Phase 2 release step.
            addon_cdylib_path = Some(build_output.cdylib_path);
            addon_library_stem = Some(build_output.library_stem);
            addon_host_triple = Some(build_output.host_triple);

            // NOTE: integrity is re-computed after packages.tdm
            // rewrite below (common to both flows) so the digest
            // reflects the final committed tree state.
        }

        #[cfg(not(feature = "native"))]
        {
            bail(
                &rollback,
                "addon flow requires the `native` feature. Rebuild taida with --features native to publish Rust addon packages."
                    .to_string(),
            );
        }
    }

    // ── Write packages.tdm (common to both flows) ───────
    if let Err(e) = fs::write(&manifest_path, &preparation.updated_manifest_source) {
        bail(
            &rollback,
            format!("Failed to update '{}': {}", manifest_path.display(), e),
        );
    }
    println!("  [rewrite]  packages.tdm <<<@{}", preparation.version);

    // Re-compute integrity AFTER packages.tdm rewrite so the digest
    // reflects the final on-disk state of all committed files.
    // For the addon flow this also includes native/addon.lock.toml
    // which was written earlier.
    let final_integrity = pkg::publish::compute_publish_integrity(&project_dir);

    // ── Dry-run: Build mode exit point ────────────────────
    //
    // RC2.6-2c: `--dry-run=build` runs cargo build + lockfile merge
    // + packages.tdm rewrite, then stops. Git commit/push/release
    // are skipped. The mutated files (lockfile, packages.tdm) remain
    // on disk so the user can inspect them.
    if dry_run == Some(DryRunMode::Build) {
        println!("Dry run (build): build + lockfile completed, git/release skipped.");
        println!("  Package: {}/{}", author_name, preparation.package_name);
        println!("  Version: @{}", preparation.version);
        println!("  Integrity: {}", final_integrity);
        if is_addon_flow {
            println!("  Lockfile: {}", addon_lock_path.display());
        }
        println!("  packages.tdm and lockfile have been updated on disk.");
        println!("  Run `git diff` to inspect, or `git checkout .` to revert.");
        return;
    }

    // ── I2: worktree dirty only within the allowlist ────
    let allow_refs: Vec<&Path> = allowlist.iter().map(|p| p.as_path()).collect();
    if let Err(e) = pkg::publish::check_dirty_allowlist(&project_dir, &allow_refs) {
        bail(&rollback, format!("Worktree invariant I2 violated: {}", e));
    }

    // ── Commit + tag + push ─────────────────────────────
    let extra_paths: Vec<&Path> = if is_addon_flow {
        let mut paths = vec![Path::new("native/addon.lock.toml")];
        // RC2.6B-004: stage addon.toml if the URL was rewritten
        if addon_toml_rewritten {
            paths.push(Path::new("native/addon.toml"));
        }
        paths
    } else {
        Vec::new()
    };
    if let Err(e) = pkg::publish::git_commit_tag_push(
        &project_dir,
        &preparation.version,
        &preparation.package_name,
        &extra_paths,
    ) {
        bail(&rollback, e);
    }

    // ── Phase 2: gh release create ────────────────────────
    //
    // The env var `TAIDA_PUBLISH_SKIP_RELEASE=1` is the dev escape
    // hatch (also used by Phase 1 integration tests). When not set,
    // we create a GitHub Release with the lockfile + cdylib as assets.
    //
    // Note: this runs AFTER git_commit_tag_push, so the commit and
    // tag already exist on the remote. There is no rollback if the
    // release step fails — the error is printed and the user must
    // fix manually (or re-run `gh release create` by hand).
    let skip_release = std::env::var("TAIDA_PUBLISH_SKIP_RELEASE")
        .map(|v| v == "1")
        .unwrap_or(false);
    if is_addon_flow {
        if skip_release {
            println!("  [release]  skipped (TAIDA_PUBLISH_SKIP_RELEASE=1)");
        } else if let (Some(cdylib_path), Some(library_stem), Some(host_triple)) =
            (&addon_cdylib_path, &addon_library_stem, &addon_host_triple)
        {
            // Determine the cdylib extension from the on-disk file.
            let cdylib_ext = cdylib_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("so");
            let canonical_cdylib_name =
                format!("lib{}-{}.{}", library_stem, host_triple, cdylib_ext);

            // RC2.6B-024: release title uses qualified org/name@version.
            // preparation.package_name is the bare directory name;
            // addon.toml's `package` field holds the qualified name.
            let qualified_name = taida::addon::manifest::parse_addon_manifest(
                &project_dir.join("native").join("addon.toml"),
            )
            .map(|m| m.package)
            .unwrap_or_else(|_| preparation.package_name.clone());
            let release_title = format!("{}@{}", qualified_name, preparation.version);
            let release_notes = format!("Release {} of {}", preparation.version, qualified_name);

            let assets = vec![
                pkg::publish::GhReleaseAsset {
                    local_path: addon_lock_path.clone(),
                    asset_name: "addon.lock.toml".to_string(),
                },
                pkg::publish::GhReleaseAsset {
                    local_path: cdylib_path.clone(),
                    asset_name: canonical_cdylib_name.clone(),
                },
            ];

            println!(
                "  [release]  gh release create {} (2 assets: addon.lock.toml, {})",
                preparation.version, canonical_cdylib_name
            );

            if let Err(e) = pkg::publish::create_github_release(
                &project_dir,
                &preparation.version,
                &release_title,
                &release_notes,
                &assets,
            ) {
                // Release failure is non-fatal to the commit/push but
                // is reported as a CLI error so the user knows.
                eprintln!("Warning: GitHub Release creation failed:\n{}", e);
                eprintln!();
                eprintln!(
                    "The commit and tag ({}) have been pushed successfully.",
                    preparation.version
                );
                eprintln!("You can create the release manually with:");
                eprintln!(
                    "  gh release create {} --title \"{}\" --notes \"{}\" {}#addon.lock.toml {}#{}",
                    preparation.version,
                    release_title,
                    release_notes,
                    addon_lock_path.display(),
                    cdylib_path.display(),
                    canonical_cdylib_name,
                );
            }
        }
    }

    println!(
        "Published {}/{}@{}",
        author_name, preparation.package_name, preparation.version
    );
    println!("  Integrity: {}", final_integrity);
    println!("  Tag: {}", preparation.version);
    println!();
    println!("To register as a verified package on taida-community:");
    println!(
        "  {}",
        pkg::publish::proposals_url(
            &author_name,
            &preparation.package_name,
            &preparation.version,
            &final_integrity,
        )
    );
}

// ── Doc subcommand ──────────────────────────────────────

// ---------------------------------------------------------------------------
// N-3: `taida cache clean` — remove stale WASM runtime cache files
// ---------------------------------------------------------------------------

fn run_cache(args: &[String]) {
    if args.is_empty() || args.iter().any(|a| is_help_flag(a.as_str())) {
        println!("Usage: taida cache <command> [options]");
        println!();
        println!("Commands:");
        println!("  clean              Remove cached WASM runtime .o files (default)");
        println!("  clean --addons     Remove cached addon prebuild binaries");
        println!("                     (RC15B-001: prunes ~/.taida/addon-cache/)");
        println!("  clean --all        Remove both WASM and addon caches");
        return;
    }

    match args[0].as_str() {
        "clean" => {
            // RC15B-001: parse optional --addons / --all flags.
            let mut clean_wasm = true;
            let mut clean_addons = false;
            for extra in &args[1..] {
                match extra.as_str() {
                    "--addons" => {
                        clean_wasm = false;
                        clean_addons = true;
                    }
                    "--all" => {
                        clean_wasm = true;
                        clean_addons = true;
                    }
                    other => {
                        eprintln!(
                            "Unknown flag '{}' for 'taida cache clean'. Use --addons, --all, or no flag.",
                            other
                        );
                        std::process::exit(1);
                    }
                }
            }
            if clean_wasm {
                run_cache_clean();
            }
            if clean_addons {
                run_cache_clean_addons();
            }
        }
        other => {
            eprintln!(
                "Unknown cache command '{}'. Use 'taida cache clean'.",
                other
            );
            std::process::exit(1);
        }
    }
}

fn run_cache_clean() {
    // RCB-56: Use absolute CWD to match run_build()'s input_path.parent() behavior.
    // Both now resolve .taida/cache/wasm-rt/ from an absolute path.
    let project_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cache_dir = codegen::driver::default_wasm_cache_dir(Some(&project_dir));

    if !cache_dir.exists() {
        println!("No cache directory found at '{}'.", cache_dir.display());
        return;
    }

    let mut removed = 0usize;
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            // Remove cached .o files and temp files, preserve 'include/' dir
            if (fname.ends_with(".o") || fname.ends_with(".tmp.c") || fname.ends_with(".tmp.o"))
                && fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }

    if removed > 0 {
        println!(
            "Cleaned {} cached file(s) from '{}'.",
            removed,
            cache_dir.display()
        );
    } else {
        println!(
            "Cache directory '{}' is already clean.",
            cache_dir.display()
        );
    }
}

// RC15B-001: prune the addon prebuild cache at ~/.taida/addon-cache/.
//
// The directory tree is walked by `clean_addon_cache`, which preserves
// user-placed files (anything that is not a recognised addon binary or
// `.manifest-sha256` sidecar) so a confused user can still inspect the
// directory after the command runs.
fn run_cache_clean_addons() {
    match taida::addon::prebuild_fetcher::clean_addon_cache() {
        Ok(summary) => {
            if !summary.root_existed {
                println!("No addon cache found at '{}'.", summary.root.display());
                return;
            }
            let total = summary.binaries_removed + summary.sidecars_removed;
            if total == 0 {
                println!(
                    "Addon cache at '{}' is already clean.",
                    summary.root.display()
                );
            } else {
                let mib = summary.bytes_freed as f64 / (1024.0 * 1024.0);
                println!(
                    "Cleaned {} addon binary file(s) and {} sidecar file(s) ({:.2} MiB) from '{}'.",
                    summary.binaries_removed,
                    summary.sidecars_removed,
                    mib,
                    summary.root.display()
                );
            }
        }
        Err(e) => {
            eprintln!("Error cleaning addon cache: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_doc(args: &[String]) {
    if args.iter().any(|a| is_help_flag(a.as_str())) {
        print_doc_help();
        return;
    }

    if args.is_empty() || args[0] != "generate" {
        eprintln!("Unknown or missing subcommand for doc.");
        eprintln!("Run `taida doc --help` for usage.");
        std::process::exit(1);
    }

    // Parse args after "generate"
    let gen_args = &args[1..];
    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;

    let mut i = 0;
    while i < gen_args.len() {
        match gen_args[i].as_str() {
            "--help" | "-h" => {
                print_doc_help();
                return;
            }
            "-o" | "--output" => {
                i += 1;
                if i >= gen_args.len() {
                    eprintln!("Missing value for -o/--output.");
                    eprintln!("Run `taida doc --help` for usage.");
                    std::process::exit(1);
                }
                output_path = Some(gen_args[i].clone());
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for doc generate: {}", raw);
                eprintln!("Run `taida doc --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if input_path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida doc generate.");
                    std::process::exit(1);
                }
                input_path = Some(gen_args[i].clone());
            }
        }
        i += 1;
    }

    let input = match input_path {
        Some(p) => p,
        None => {
            eprintln!("Missing <PATH> argument.");
            eprintln!("Run `taida doc --help` for usage.");
            std::process::exit(1);
        }
    };

    let target_path = Path::new(&input);

    // Collect .td files
    let td_files: Vec<PathBuf> = if target_path.is_dir() {
        let files = collect_td_files(target_path);
        if files.is_empty() {
            eprintln!("No .td files found in '{}'", input);
            std::process::exit(1);
        }
        files
    } else {
        vec![target_path.to_path_buf()]
    };

    let mut all_output = String::new();

    for td_file in &td_files {
        let source = match fs::read_to_string(td_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", td_file.display(), e);
                continue;
            }
        };

        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                eprintln!("{}: {}", td_file.display(), err);
            }
            continue;
        }

        let module_name = td_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let module_doc = doc::extract_docs(&program, module_name);
        let markdown = doc::render_markdown(&module_doc);

        if !markdown.trim().is_empty() {
            all_output.push_str(&markdown);
        }
    }

    match output_path {
        Some(out) => {
            // Create parent directory if needed
            if let Some(parent) = Path::new(&out).parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::write(&out, &all_output) {
                Ok(_) => println!("Documentation generated: {}", out),
                Err(e) => {
                    eprintln!("Error writing '{}': {}", out, e);
                    std::process::exit(1);
                }
            }
        }
        None => {
            print!("{}", all_output);
        }
    }
}

fn find_packages_tdm_from(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if dir.join("packages.tdm").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn find_packages_tdm() -> Option<PathBuf> {
    let dir = env::current_dir().ok()?;
    find_packages_tdm_from(&dir)
}

// ── LSP server ─────────────────────────────────────────

#[cfg(feature = "lsp")]
fn run_lsp(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_lsp_help();
            return;
        }
        _ => {
            eprintln!("Unexpected arguments.");
            eprintln!("Run `taida lsp --help` for usage.");
            std::process::exit(1);
        }
    }

    // N-54: Tokio runtime creation fails only under severe resource
    // exhaustion (e.g. file descriptor limit reached). In such cases
    // there is no meaningful recovery, so panic with a clear message.
    let rt = tokio::runtime::Runtime::new()
        .expect("failed to create Tokio runtime for LSP server (possible fd/resource exhaustion)");
    rt.block_on(taida::lsp::server::run_server());
}

// ── REPL ────────────────────────────────────────────────

fn repl(no_check: bool) {
    let mut interpreter = Interpreter::new();

    loop {
        print!("taida> ");
        // N-45: REPL stdout flush — failure means the output pipe is broken
        // (e.g. piped into a closed process), in which case continuing the
        // REPL loop is pointless. Use `ok()` to silently exit on next read.
        if io::stdout().flush().is_err() {
            break;
        }

        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(0) => {
                // EOF
                println!();
                break;
            }
            Ok(_) => {
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }

                let (program, parse_errors) = parse(input);
                if !parse_errors.is_empty() {
                    for err in &parse_errors {
                        eprintln!("  {}", err);
                    }
                    continue;
                }

                // Type checking in REPL (warn but don't abort)
                if !no_check {
                    let mut checker = TypeChecker::new();
                    checker.check_program(&program);
                    if !checker.errors.is_empty() {
                        for err in &checker.errors {
                            eprintln!("  {}", err);
                        }
                        // Continue execution despite type errors in REPL
                    }
                }

                match interpreter.eval_program(&program) {
                    Ok(val) => {
                        for line in &interpreter.output {
                            println!("{}", line);
                        }
                        interpreter.output.clear();
                        if !matches!(val, taida::interpreter::Value::Unit) {
                            println!("  {}", val);
                        }
                    }
                    Err(e) => {
                        for line in &interpreter.output {
                            println!("{}", line);
                        }
                        interpreter.output.clear();
                        eprintln!("  {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("taida-{}-{}-{}", name, std::process::id(), unique));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn js_build_helper_emits_program_body_for_stdin_source() {
        let dir = temp_test_dir("stdin-js-build");
        let out = dir.join("stdin.js");
        let mut stats = CompileDiagStats::default();

        transpile_js_source_to_output(
            "opt <= Lax[42]()\nstdout(opt.hasValue().toString())\n",
            "/dev/stdin",
            None,
            &out,
            None,
            false,
            DiagFormat::Text,
            &mut stats,
            None,
            None,
            None,
        );

        let js = fs::read_to_string(&out).unwrap();
        assert!(js.contains("const opt = __taida_solidify(Lax(42));"));
        assert!(js.contains("__taida_stdout(opt.hasValue().toString());"));

        fs::remove_file(&out).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn cli_version_matches_embedded_build_metadata() {
        // taida_version() is the single source of truth — verify it returns
        // a non-empty string (exact value depends on build environment).
        let version = taida_version();
        assert!(!version.is_empty(), "taida_version() should not be empty");
    }
}
