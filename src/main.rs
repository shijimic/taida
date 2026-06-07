#![allow(clippy::doc_lazy_continuation)]

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

#[cfg(feature = "community")]
use taida::auth;
#[cfg(feature = "community")]
use taida::community;
use taida::doc;
use taida::graph::ai_format;
use taida::graph::verify;
use taida::interpreter::Interpreter;
use taida::parser::parse;
use taida::pkg;
use taida::types::{CompileTarget, TypeChecker};
use taida::version::taida_version;

mod cli;
use cli::build::*;
use cli::help::*;
use cli::ingot::*;
use cli::way::*;

fn is_help_flag(raw: &str) -> bool {
    matches!(raw, "--help" | "-h")
}

fn removed_command_replacement(command: &str) -> Option<&'static str> {
    match command {
        "check" => Some("taida way check"),
        "verify" => Some("taida way verify"),
        "lint" => Some("taida way lint"),
        "todo" => Some("taida way todo"),
        "inspect" => Some("taida graph summary"),
        "transpile" => Some("taida build native"),
        "compile" => Some("taida build native"),
        "deps" => Some("taida ingot deps"),
        "install" => Some("taida ingot install"),
        "update" => Some("taida ingot update"),
        "publish" => Some("taida ingot publish"),
        "cache" => Some("taida ingot cache"),
        "c" => Some("taida community"),
        _ => None,
    }
}

fn reject_removed_command(command: &str) -> ! {
    let replacement = removed_command_replacement(command).unwrap_or("taida --help");
    eprintln!(
        "[E1700] Command '{}' was removed in @e.X. Use '{}' instead.",
        command, replacement
    );
    eprintln!("        See `taida --help` for the new command set.");
    std::process::exit(2);
}

fn reject_removed_migration_command(invocation: &str) -> ! {
    eprintln!(
        "[E1700] Migration command '{}' is not available. Current CLI does not provide AST migration tooling.",
        invocation
    );
    eprintln!(
        "        Update source files manually; run `taida upgrade --help` for self-upgrade usage."
    );
    std::process::exit(2);
}

fn main() {
    // C25B-018: install the panic hook + fatal-signal cleanup handlers
    // **before** we otherwise perturb signal dispositions below. This
    // way a panic during very early startup (before `filtered_args`
    // parsing etc.) still runs the terminal-state-restoration path,
    // and the SIGPIPE-ignore below is unaffected (SIGPIPE is not in
    // our cleanup signal set).
    taida::panic_cleanup::install_panic_cleanup_hook();
    taida::panic_cleanup::install_signal_cleanup_handlers();

    // C22-4 / C22B-004: restore `taida <file> ... | head` as a first-class UNIX
    // pipeline. Rust binaries default to SIGPIPE-driven exit(141) the moment
    // a downstream consumer closes early; we disable that disposition here so
    // that subsequent `write(2)` calls fail with EPIPE instead — which the
    // `stdout` builtin (C22-2) silently absorbs via `writeln!+flush().ok()`.
    //
    // Scope note: this sets *process-wide* signal disposition. Matches the
    // convention of every major CLI (ripgrep, bat, fd, coreutils …). Child
    // processes started via `std::process::Command` / tokio are unaffected
    // because `execve` resets signal dispositions on the child side.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let args: Vec<String> = env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

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
            old if removed_command_replacement(old).is_some() => reject_removed_command(old),
            "way" => run_way(&filtered_args[2..], no_check),
            "build" => run_build(&filtered_args[2..], no_check),
            "graph" => run_graph(&filtered_args[2..]),
            "init" => run_init(&filtered_args[2..]),
            "ingot" => run_ingot(&filtered_args[2..]),
            "doc" => run_doc(&filtered_args[2..]),
            #[cfg(feature = "community")]
            "auth" => auth::run_auth(&filtered_args[2..]),
            #[cfg(not(feature = "community"))]
            "auth" => {
                eprintln!("The 'auth' command requires the 'community' feature.");
                eprintln!("Rebuild with: cargo build --features community");
                std::process::exit(1);
            }
            #[cfg(feature = "community")]
            "community" => community::run_community(&filtered_args[2..]),
            #[cfg(not(feature = "community"))]
            "community" => {
                eprintln!("The 'community' command requires the 'community' feature.");
                eprintln!("Rebuild with: cargo build --features community");
                std::process::exit(1);
            }
            #[cfg(feature = "community")]
            "upgrade" => run_upgrade(&filtered_args[2..]),
            #[cfg(not(feature = "community"))]
            "upgrade" => {
                eprintln!("The 'upgrade' command requires the 'community' feature.");
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
        checker.set_compile_target(CompileTarget::Interpreter);
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

    // C22-2 / C22B-002: CLI execution uses stream mode so that `stdout(...)`
    // / `debug(...)` flush to the terminal immediately. REPL (`run_repl`)
    // and in-process tests continue to use `Interpreter::new()` (buffered).
    let mut interpreter = Interpreter::new_streaming();
    // Set current file for module resolution
    if let Ok(canonical) = fs::canonicalize(filename) {
        interpreter.set_current_file(&canonical);
    } else {
        interpreter.set_current_file(Path::new(filename));
    }
    match interpreter.eval_program(&program) {
        Ok(val) => {
            // In buffered mode the Vec accumulated output during eval; drain it
            // now. In stream mode the Vec is empty (output was flushed inline),
            // so this loop is a no-op.
            if !interpreter.stream_stdout {
                for line in &interpreter.output {
                    println!("{}", line);
                }
            }
            // If the last value is not Unit and nothing was ever printed
            // via `stdout(...)`, print the value so that `taida expr.td`
            // continues to show the result of a pure-expression script.
            let no_emissions = if interpreter.stream_stdout {
                interpreter.stdout_emissions == 0
            } else {
                interpreter.output.is_empty()
            };
            if !matches!(val, taida::interpreter::Value::Unit) && no_emissions {
                println!("{}", val);
            }
        }
        Err(e) => {
            // Print any output that was collected before the error (buffered
            // mode only; in stream mode it has already been flushed inline).
            if !interpreter.stream_stdout {
                for line in &interpreter.output {
                    println!("{}", line);
                }
            }
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

// ── Lint subcommand ──────────────────────────

// ── Compile / Transpile / Build subcommands ─────────────

// ── Upgrade subcommand ──────────────────────────────────────

#[cfg(feature = "community")]
fn run_upgrade(args: &[String]) {
    use taida::upgrade::{UpgradeConfig, VersionFilter};

    if args.len() == 1 && is_help_flag(args[0].as_str()) {
        print_upgrade_help();
        return;
    }

    if args.iter().any(|a| a == "--d28") {
        reject_removed_migration_command("taida upgrade --d28");
    }
    if args.iter().any(|a| a == "--d29") {
        reject_removed_migration_command("taida upgrade --d29");
    }
    if args.iter().any(|a| a == "--e30") {
        reject_removed_migration_command("taida upgrade --e30");
    }

    let mut check_only = false;
    let mut generation: Option<String> = None;
    let mut label: Option<String> = None;
    let mut exact: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_upgrade_help();
                return;
            }
            "--check" => {
                check_only = true;
            }
            "--gen" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --gen requires a value");
                    std::process::exit(1);
                }
                generation = Some(args[i].clone());
            }
            "--label" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --label requires a value");
                    std::process::exit(1);
                }
                label = Some(args[i].clone());
            }
            "--version" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --version requires a value");
                    std::process::exit(1);
                }
                exact = Some(args[i].clone());
            }
            other => {
                eprintln!("Error: unknown option '{}'", other);
                eprintln!("Run `taida upgrade --help` for usage.");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Validate mutual exclusivity
    if exact.is_some() && (generation.is_some() || label.is_some()) {
        eprintln!("Error: --version cannot be combined with --gen or --label");
        std::process::exit(1);
    }

    let config = UpgradeConfig {
        check_only,
        filter: VersionFilter {
            generation,
            label,
            exact,
        },
    };

    if let Err(e) = taida::upgrade::run(config) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// ── Graph subcommand ────────────────────────────────────

fn run_graph(args: &[String]) {
    if args.first().is_some_and(|arg| arg == "summary") {
        run_graph_summary(&args[1..]);
        return;
    }

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

fn run_graph_summary(args: &[String]) {
    let mut format_type = "text".to_string();
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_graph_summary_help();
                return;
            }
            "--format" | "-f" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida graph summary --help` for usage.");
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
                eprintln!("Unknown option for graph summary: {}", raw);
                eprintln!("Run `taida graph summary --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida graph summary.");
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
            eprintln!("Run `taida graph summary --help` for usage.");
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

    let summary = verify::structural_summary(&program, &file_path);
    match format_type.as_str() {
        "sarif" => print!("{}", format_graph_summary_sarif(&summary)),
        _ => println!("{}", summary),
    }
}

fn format_graph_summary_sarif(summary_json: &str) -> String {
    let summary =
        serde_json::from_str::<serde_json::Value>(summary_json).unwrap_or_else(|_| json!({}));
    serde_json::to_string_pretty(&json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [
            {
                "tool": {
                    "driver": {
                        "name": "taida-graph-summary",
                        "version": taida_version(),
                        "rules": []
                    }
                },
                "results": [],
                "properties": {
                    "summary": summary
                }
            }
        ]
    }))
    .expect("graph summary SARIF serialization should not fail")
}

// ── Verify subcommand ───────────────────────────────────

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

// ── Install subcommand ──────────────────────────────────

// ── Update subcommand ──────────────────────────────────

// ── Publish subcommand ─────────────────────────────────

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
                    checker.set_compile_target(CompileTarget::Interpreter);
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
    use crate::cli::build_descriptor::{
        BuildUnitDescriptor, target_incompatible_import, validate_target_closure_modules,
    };
    use std::time::{SystemTime, UNIX_EPOCH};
    use taida::parser::{ImportStmt, Statement};

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
        // C12-2b: `.toString()` is routed through `__taida_to_string` so
        // plain BuchiPacks render as `@(...)` instead of the JS default
        // `[object Object]`. The receiver is still wrapped — here the
        // hasValue() call returns a primitive Boolean, which the helper
        // formats via `String(v)` (matches interpreter / native).
        assert!(js.contains("__taida_stdout(__taida_to_string(opt.hasValue()));"));

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

    /// `validate_target_closure_modules` rejects any closure module that
    /// has parse errors with `[E1941]` so a TOCTOU race window between
    /// `module_graph::collect_local_modules` and the inner re-read cannot
    /// silently downgrade a target-incompatibility diagnostic. Exercised
    /// directly here because the upstream `collect_local_modules` step in
    /// `validate_target_closure` would otherwise reject the same fixture
    /// before the inner loop runs, leaving the inner hard-fail untested
    /// in end-to-end flows.
    #[test]
    fn validate_target_closure_modules_rejects_parse_error_inner() {
        let dir = temp_test_dir("validate-closure-inner-parse");
        let entry = dir.join("entry.td");
        fs::write(&entry, "stdout(\"entry\")\n").expect("write entry");
        let bad = dir.join("bad.td");
        fs::write(&bad, "let bad = (\n").expect("write bad module");

        let unit = BuildUnitDescriptor {
            symbol: "frontendA".to_string(),
            name: "frontend-a".to_string(),
            target: BuildTarget::WasmMin,
            entry_symbol: "entryMain".to_string(),
            entry_path: Some(entry.clone()),
            handler: None,
            route_assets: Vec::new(),
            before_hooks: Vec::new(),
        };

        let err = validate_target_closure_modules(&unit, &entry, std::slice::from_ref(&bad))
            .expect_err(
                "TOCTOU defence must reject any closure module that fails to parse on re-read",
            );
        assert_eq!(err.code, "E1941");
        assert!(
            err.message.contains("frontend-a") && err.message.contains("bad.td"),
            "diagnostic must mention the unit and offending module: {}",
            err.message
        );
        assert!(
            err.message.to_ascii_lowercase().contains("parse error"),
            "diagnostic must surface the parse error context: {}",
            err.message
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// Sibling guarantee: when the closure target is not restricted (e.g.
    /// `js`), the inner re-parse path must short-circuit so that benign
    /// build pipelines that lower through unrestricted targets do not pay
    /// the wasm-only TOCTOU cost.
    #[test]
    fn validate_target_closure_modules_skips_inner_parse_for_unrestricted_target() {
        let dir = temp_test_dir("validate-closure-inner-skip");
        let entry = dir.join("entry.td");
        fs::write(&entry, "stdout(\"entry\")\n").expect("write entry");
        let bad = dir.join("bad.td");
        fs::write(&bad, "let bad = (\n").expect("write bad module");

        let unit = BuildUnitDescriptor {
            symbol: "serverA".to_string(),
            name: "server-a".to_string(),
            target: BuildTarget::Js,
            entry_symbol: "entryMain".to_string(),
            entry_path: Some(entry.clone()),
            handler: None,
            route_assets: Vec::new(),
            before_hooks: Vec::new(),
        };

        validate_target_closure_modules(&unit, &entry, &[bad])
            .expect("non-wasm targets must skip the closure re-parse pass");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wrangler_manifest_reader_maps_cloudflare_bindings() {
        let source = r#"
{
  // JSONC comments and trailing commas are accepted.
  "name": "edge-app",
  "route": "https://example.com/*",
  "d1_databases": [{ "binding": "DB" }],
  "kv_namespaces": [{ "binding": "CACHE" }],
  "durable_objects": {
    "bindings": [{ "name": "COUNTER", "class_name": "Counter" }],
  },
  "r2_buckets": [{ "binding": "ASSETS" }],
  "queues": {
    "producers": [{ "binding": "OUTBOX", "queue": "outbox" }],
  },
  "services": [{ "binding": "API", "service": "api" }],
}
"#;

        let capabilities =
            parse_wrangler_host_capability_manifest_str(source).expect("manifest should parse");
        assert_eq!(
            capabilities,
            vec![
                ("DB".to_string(), "cloudflare/d1".to_string()),
                ("CACHE".to_string(), "cloudflare/kv".to_string()),
                ("COUNTER".to_string(), "cloudflare/do_namespace".to_string()),
                ("ASSETS".to_string(), "cloudflare/r2".to_string()),
                (
                    "OUTBOX".to_string(),
                    "cloudflare/queue_producer".to_string()
                ),
                ("API".to_string(), "cloudflare/fetcher".to_string()),
            ]
        );
    }

    #[test]
    fn wrangler_manifest_reader_stops_at_project_marker() {
        let outer = temp_test_dir("wrangler-outer");
        let project = outer.join("project");
        let src = project.join("src");
        fs::create_dir_all(&src).expect("create project tree");
        fs::write(outer.join("wrangler.jsonc"), r#"{ "d1_databases": [] }"#)
            .expect("write outer wrangler");
        fs::write(project.join("taida.toml"), "").expect("write project marker");
        let td = src.join("main.td");
        fs::write(&td, "stdout(\"ok\")\n").expect("write source");

        assert!(
            find_wrangler_manifest_for_source(&td).is_none(),
            "manifest search must not cross the project marker"
        );

        fs::remove_dir_all(&outer).ok();
    }

    fn parse_single_import(source: &str) -> ImportStmt {
        let (program, errors) = parse(source);
        assert!(errors.is_empty(), "fixture parse errors: {errors:?}");
        program
            .statements
            .into_iter()
            .find_map(|stmt| match stmt {
                Statement::Import(import) => Some(import),
                _ => None,
            })
            .expect("fixture must contain an import")
    }

    #[test]
    fn wasm_descriptor_closure_matrix_rejects_incompatible_core_imports() {
        let net = parse_single_import(">>> taida-lang/net@a.1 => @(httpServe)\n");
        let terminal = parse_single_import(">>> taida-lang/terminal@a.1 => @(readKey)\n");
        let os_env = parse_single_import(">>> taida-lang/os@a.1 => @(EnvVar, allEnv)\n");
        let os_file = parse_single_import(">>> taida-lang/os@a.1 => @(Read)\n");
        let os_process = parse_single_import(">>> taida-lang/os@a.1 => @(run)\n");

        assert_eq!(
            target_incompatible_import(BuildTarget::WasmMin, &os_env).as_deref(),
            Some("taida-lang/os")
        );
        assert_eq!(
            target_incompatible_import(BuildTarget::WasmWasi, &net).as_deref(),
            Some("taida-lang/net")
        );
        assert_eq!(
            target_incompatible_import(BuildTarget::WasmFull, &net).as_deref(),
            Some("taida-lang/net")
        );
        assert_eq!(
            target_incompatible_import(BuildTarget::WasmEdge, &terminal).as_deref(),
            Some("taida-lang/terminal")
        );
        assert!(
            target_incompatible_import(BuildTarget::WasmEdge, &os_env).is_none(),
            "wasm-edge supports environment-only OS imports"
        );
        assert_eq!(
            target_incompatible_import(BuildTarget::WasmEdge, &os_file).as_deref(),
            Some("taida-lang/os::Read")
        );
        assert!(
            target_incompatible_import(BuildTarget::WasmWasi, &os_file).is_none(),
            "wasm-wasi supports the WASI file subset"
        );
        assert_eq!(
            target_incompatible_import(BuildTarget::WasmFull, &os_process).as_deref(),
            Some("taida-lang/os::run")
        );
    }
}
