//! commands — split out of src/main.rs (pure move).
//! Behaviour unchanged; imports added per cargo check.

use serde_json::json;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use taida::doc;
use taida::graph::ai_format;
use taida::graph::verify;
use taida::interpreter::Interpreter;
use taida::parser::parse;
use taida::pkg;
use taida::types::{CompileTarget, TypeChecker};
use taida::version::taida_version;

use crate::cli::help::{
    print_doc_help, print_graph_help, print_graph_summary_help, print_init_help, print_lsp_help,
    print_upgrade_help,
};
use crate::cli::ingot::find_packages_tdm;
use crate::cli::way::collect_td_files_reporting;
use crate::{is_help_flag, reject_removed_migration_command};

/// F62B-027: stack reservation for the dedicated evaluation threads
/// (CLI run + REPL). Each Taida call costs multiple Rust frames and
/// `MAX_CALL_DEPTH` is 8192; an unoptimised (debug) build's frames run
/// several times larger than release ones — the CI test archive executes
/// the debug binary, and 512 MiB overflowed there at depth 8000 — so the
/// reservation is sized for the debug worst case. This is virtual
/// address space only: pages are committed as actually used.
const EVAL_STACK_SIZE: usize = 2 * 1024 * 1024 * 1024;

pub(crate) fn run_source(source: &str, filename: &str, no_check: bool) {
    taida::pkg::resolver::set_program_root_for(Path::new(filename));
    let (program, parse_errors) = parse(source);
    if !parse_errors.is_empty() {
        for err in &parse_errors {
            eprintln!("{}", err);
        }
        std::process::exit(1);
    }

    // Type checking
    let mut typed_table = None;
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
        // The static types the interpreter's failed lookups default by.
        typed_table = Some(std::mem::take(&mut checker.typed_expr_table));
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
    //
    // F62B-027: evaluation runs on a dedicated thread with a large stack.
    // Each Taida call costs multiple Rust frames, so the raised
    // MAX_CALL_DEPTH (8192, matching the failure depth class of the
    // compiled backends) does not fit in the default 8 MiB main stack.
    let filename_owned = filename.to_string();
    let eval_thread = std::thread::Builder::new()
        .name("taida-eval".to_string())
        .stack_size(EVAL_STACK_SIZE)
        .spawn(move || run_program_on_eval_thread(program, &filename_owned, typed_table))
        .expect("spawn taida eval thread");
    match eval_thread.join() {
        Ok(()) => {}
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn run_program_on_eval_thread(
    program: taida::parser::Program,
    filename: &str,
    typed_table: Option<taida::types::TypedExprTable>,
) {
    let mut interpreter = Interpreter::new_streaming();
    if let Some(table) = typed_table {
        interpreter.set_typed_table(table);
    }
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
            // F62B-032: the gorilla literal is the documented fixed
            // `exit(1)` immediate termination — the interpreter used to
            // fall through here, display the gorilla value, and exit 0
            // (native already exits 1).
            if matches!(val, taida::interpreter::Value::Gorilla) {
                std::process::exit(1);
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

#[cfg(feature = "community")]
pub(crate) fn run_upgrade(args: &[String]) {
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

pub(crate) fn run_graph(args: &[String]) {
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
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida graph.");
                    eprintln!("Run `taida graph --help` for usage.");
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

pub(crate) fn run_graph_summary(args: &[String]) {
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

pub(crate) fn format_graph_summary_sarif(summary_json: &str) -> String {
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

pub(crate) fn run_init(args: &[String]) {
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

    // Delegate to pkg::init::init_project; it creates the directory once the
    // project name has been accepted.
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

pub(crate) fn run_doc(args: &[String]) {
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

    let mut all_output = String::new();
    // A file that cannot be read or parsed is skipped, so the documentation
    // is incomplete: the other files are still documented, and the command
    // ends with a failure.
    let mut had_failure = false;

    // Collect .td files
    let td_files: Vec<PathBuf> = if target_path.is_dir() {
        let (files, unreadable) = collect_td_files_reporting(target_path);
        for (dir, reason) in &unreadable {
            eprintln!("Error reading directory '{}': {}", dir.display(), reason);
            had_failure = true;
        }
        if files.is_empty() && unreadable.is_empty() {
            eprintln!("No .td files found in '{}'", input);
            std::process::exit(1);
        }
        files
    } else {
        vec![target_path.to_path_buf()]
    };

    for td_file in &td_files {
        let source = match fs::read_to_string(td_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", td_file.display(), e);
                had_failure = true;
                continue;
            }
        };

        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                eprintln!("{}: {}", td_file.display(), err);
            }
            had_failure = true;
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

    if had_failure {
        std::process::exit(1);
    }
}

#[cfg(feature = "lsp")]
pub(crate) fn run_lsp(args: &[String]) {
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
    let code = rt.block_on(taida::lsp::server::run_server());
    // The read of standard input may still be pending (after `exit` the
    // client keeps the pipe open), and waiting for it would keep the process
    // from ending.
    std::process::exit(code);
}

pub(crate) fn repl(no_check: bool) {
    // F62B-027 (review C-1): the REPL evaluates on the same large-stack
    // thread as `run_source` — MAX_CALL_DEPTH 8192 does not fit the
    // default 8 MiB main stack, and a REPL input like `deep(8000)` used
    // to hard-crash with a Rust stack overflow instead of the depth
    // diagnostic.
    let handle = std::thread::Builder::new()
        .name("taida-repl-eval".to_string())
        .stack_size(EVAL_STACK_SIZE)
        .spawn(move || repl_loop(no_check))
        .expect("spawn taida repl thread");
    match handle.join() {
        Ok(()) => {}
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Whether the REPL should keep reading lines instead of running `input`.
///
/// That is the case when it does not parse only because it stops early (an
/// unclosed bracket or template, a definition still waiting for its body or
/// return type), and while the body of a definition opened with a trailing
/// `=` is still being indented: a block-bodied lambda is already complete
/// after its first body line, but the next line may belong to it.
fn repl_input_is_incomplete(input: &str, errors: &[taida::parser::ParseError]) -> bool {
    let shape = InputShape::of(input);
    let opens_body = shape.opens_body();
    let last_is_indented = shape.last_is_indented();
    // The body of a definition is still being written: its header has just
    // been typed, or the last line is indented.
    let in_body = last_is_indented || (opens_body && shape.code_lines.len() == 1);
    let stops_early = errors.iter().any(|err| {
        let message = err.message.as_str();
        // The end of the input came where more was expected. That can be
        // mended by another line only inside a bracket or a body: the
        // grammar has no line break after an operator or a colon.
        ((message.contains("found Eof") || message.contains("token: Eof"))
            && (shape.open_brackets > 0 || in_body))
            // The return type comes after the body, so a definition whose
            // body is still being written may yet get one. A line back at the
            // margin ends the body: what is missing is then an error.
            || (message.contains("[E1526]") && in_body)
            // Only a template string or a block comment can run on to the
            // next line; a plain string cannot, and no further input mends it.
            || message.starts_with("Unterminated template string")
            || message.starts_with("Unterminated block comment")
            || (message.starts_with("Unterminated") && message.contains("in template string"))
    });
    stops_early || (opens_body && last_is_indented)
}

/// Whether `line`, typed at the margin, ends the block-bodied definition
/// `pending` that already parses: the line then starts the next input.
fn repl_line_ends_definition(pending: &str, line: &str) -> bool {
    if line.starts_with([' ', '\t']) || line.starts_with(['=', ')', ']', '}', '|', ',']) {
        return false;
    }
    // A comment is no code: it does not end the body above it.
    if InputShape::of(line).code_lines.is_empty() {
        return false;
    }
    let shape = InputShape::of(pending);
    shape.opens_body() && shape.last_is_indented() && parse(pending).1.is_empty()
}

/// What the REPL needs to know about the text typed so far, read past
/// strings, templates and comments.
struct InputShape {
    /// Brackets opened and not yet closed.
    open_brackets: usize,
    /// The lines that hold code, comments removed: whether the line is
    /// indented, and its code.
    code_lines: Vec<(bool, String)>,
}

impl InputShape {
    fn of(input: &str) -> Self {
        let mut open_brackets: isize = 0;
        let mut code_lines = Vec::new();
        let mut in_template = false;
        let mut block_comment_depth = 0usize;
        for line in input.lines() {
            let indented = line.starts_with([' ', '\t']);
            let mut code = String::new();
            // A plain string ends with its line.
            let mut quote: Option<char> = None;
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                if block_comment_depth > 0 {
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        block_comment_depth -= 1;
                    } else if c == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        block_comment_depth += 1;
                    }
                    continue;
                }
                if in_template || quote.is_some() {
                    code.push(c);
                    if c == '\\' {
                        code.extend(chars.next());
                    } else if quote == Some(c) {
                        quote = None;
                    } else if in_template && c == '`' {
                        in_template = false;
                    }
                    continue;
                }
                match c {
                    '"' | '\'' => {
                        quote = Some(c);
                        code.push(c);
                    }
                    '`' => {
                        in_template = true;
                        code.push(c);
                    }
                    '/' if chars.peek() == Some(&'/') => break,
                    '/' if chars.peek() == Some(&'*') => {
                        chars.next();
                        block_comment_depth += 1;
                    }
                    '(' | '[' | '{' => {
                        open_brackets += 1;
                        code.push(c);
                    }
                    ')' | ']' | '}' => {
                        open_brackets -= 1;
                        code.push(c);
                    }
                    _ => code.push(c),
                }
            }
            if !code.trim().is_empty() {
                code_lines.push((indented, code));
            }
        }
        Self {
            open_brackets: open_brackets.max(0) as usize,
            code_lines,
        }
    }

    /// Whether the first line of code ends with the `=` that opens a body
    /// (not the end of a comparison such as `==`, `!=` or `>=`).
    fn opens_body(&self) -> bool {
        self.code_lines.first().is_some_and(|(_, code)| {
            let code = code.trim_end();
            code.ends_with('=')
                && !code.ends_with("==")
                && !code.ends_with("!=")
                && !code.ends_with(">=")
        })
    }

    /// Whether the last line of code is indented, after a first line.
    fn last_is_indented(&self) -> bool {
        self.code_lines.len() > 1
            && self
                .code_lines
                .last()
                .is_some_and(|(indented, _)| *indented)
    }
}

/// One REPL session: the interpreter and the type checker, which see the
/// same inputs.
struct ReplSession {
    interpreter: Interpreter,
    /// One checker serves the whole session, so a binding or definition made
    /// by one input is known to the next; the interpreter keeps its
    /// environment the same way.
    checker: TypeChecker,
    /// The inputs that ran to the end, in order. An input that fails at run
    /// time binds nothing in the interpreter, but the checker has already
    /// taken its names in; the checker is then rebuilt from these.
    ran: Vec<String>,
    no_check: bool,
}

impl ReplSession {
    fn new(no_check: bool) -> Self {
        Self {
            interpreter: Interpreter::new(),
            checker: Self::fresh_checker(),
            ran: Vec::new(),
            no_check,
        }
    }

    fn fresh_checker() -> TypeChecker {
        let mut checker = TypeChecker::new();
        checker.set_compile_target(CompileTarget::Interpreter);
        checker
    }

    /// Type-check `program`, in the scope the earlier inputs made.
    fn check(&mut self, program: &taida::parser::Program) {
        self.checker.errors.clear();
        // Node ids restart at 1 for every parse, so the previous input's
        // recorded types must not be looked up for this one.
        self.checker.typed_expr_table = taida::types::TypedExprTable::new();
        self.checker.check_program(program);
    }

    /// Start the checker over from the inputs that ran to the end.
    fn rebuild_checker(&mut self) {
        self.checker = Self::fresh_checker();
        for source in std::mem::take(&mut self.ran) {
            let (program, parse_errors) = parse(&source);
            if parse_errors.is_empty() {
                self.check(&program);
            }
            self.ran.push(source);
        }
        self.checker.errors.clear();
    }

    /// Parse, check and evaluate one input.
    fn run(&mut self, input: &str) {
        let (program, parse_errors) = parse(input);
        if !parse_errors.is_empty() {
            for err in &parse_errors {
                eprintln!("  {}", err);
            }
            return;
        }

        // Type checking in REPL (warn but don't abort)
        if !self.no_check {
            self.check(&program);
            if !self.checker.errors.is_empty() {
                for err in &self.checker.errors {
                    eprintln!("  {}", err);
                }
                // Continue execution despite type errors in REPL
            }
        }

        match self.interpreter.eval_program(&program) {
            Ok(val) => {
                for line in &self.interpreter.output {
                    println!("{}", line);
                }
                self.interpreter.output.clear();
                if !matches!(val, taida::interpreter::Value::Unit) {
                    println!("  {}", val);
                }
                self.ran.push(input.to_string());
            }
            Err(e) => {
                for line in &self.interpreter.output {
                    println!("{}", line);
                }
                self.interpreter.output.clear();
                eprintln!("  {}", e);
                if !self.no_check {
                    self.rebuild_checker();
                }
            }
        }
    }
}

fn repl_loop(no_check: bool) {
    let mut session = ReplSession::new(no_check);
    // Lines of an input that continues over several lines.
    let mut pending = String::new();

    loop {
        print!(
            "{}",
            if pending.is_empty() {
                "taida> "
            } else {
                "...    "
            }
        );
        // N-45: REPL stdout flush — failure means the output pipe is broken
        // (e.g. piped into a closed process), in which case continuing the
        // REPL loop is pointless. Use `ok()` to silently exit on next read.
        if io::stdout().flush().is_err() {
            break;
        }

        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => {
                // EOF
                if !pending.trim().is_empty() {
                    session.run(&pending);
                }
                println!();
                break;
            }
            Ok(_) => {
                // Only the line terminator goes: the text of a template that
                // runs over several lines keeps its spaces.
                let line = line.trim_end_matches(['\n', '\r']);
                if pending.is_empty() {
                    let line = line.trim_start();
                    if line.trim().is_empty() {
                        continue;
                    }
                    pending.push_str(line);
                } else if line.trim().is_empty() {
                    // A template may hold blank lines; any other input that
                    // is still open ends at an empty line, so that its parse
                    // error can be reported.
                    let (_, errors) = parse(&pending);
                    if errors
                        .iter()
                        .any(|err| err.message.starts_with("Unterminated template string"))
                    {
                        pending.push('\n');
                        pending.push_str(line);
                        continue;
                    }
                    let input = std::mem::take(&mut pending);
                    session.run(&input);
                    continue;
                } else if repl_line_ends_definition(&pending, line) {
                    // The line does not belong to the definition above it.
                    let input = std::mem::take(&mut pending);
                    session.run(&input);
                    pending.push_str(line.trim_start());
                } else {
                    pending.push('\n');
                    pending.push_str(line);
                }

                let (_, parse_errors) = parse(&pending);
                if repl_input_is_incomplete(&pending, &parse_errors) {
                    continue;
                }
                let input = std::mem::take(&mut pending);
                session.run(&input);
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

    fn incomplete(input: &str) -> bool {
        let (_, errors) = parse(input);
        repl_input_is_incomplete(input, &errors)
    }

    #[test]
    fn repl_waits_for_the_rest_of_an_unfinished_input() {
        for input in [
            "xs <= @[1,",
            "x <= (1 +",
            "Pilot = @(\n  name: Str",
            "add x: Int y: Int =",
            "add x: Int y: Int =\n  x + y",
            "x <= `abc ${",
            // A block-bodied lambda parses after its first body line, but
            // the next indented line may still belong to it.
            "f <= _ x: Int =\n  z <= x * 2",
            // A comment after the `=` does not change that.
            "f <= _ x: Int = // doubles\n  z <= x * 2",
            "x <= `abc\ndef",
        ] {
            assert!(incomplete(input), "{input:?} should continue");
        }
    }

    #[test]
    fn repl_does_not_wait_for_what_no_further_line_can_supply() {
        for input in [
            // A plain string cannot run on to the next line.
            "stdout(\"\\u{41\")",
            "stdout(\"abc\\",
            // A line back at the margin ends the body of a definition that
            // has no return type.
            "f x: Int =\n  x + 1\nf(1)",
        ] {
            assert!(!incomplete(input), "{input:?} should run");
        }
    }

    #[test]
    fn comments_and_strings_are_read_past_when_shaping_the_input() {
        let shape = InputShape::of("f <= \"a // b (\" // c (\n  g /* ( */ + 1");
        assert_eq!(shape.open_brackets, 0);
        assert_eq!(
            shape.code_lines,
            vec![
                (false, "f <= \"a // b (\" ".to_string()),
                (true, "  g  + 1".to_string())
            ]
        );
        let shape = InputShape::of("x <= `a (\n// not a comment inside the template`\ny <= (1,");
        assert_eq!(shape.open_brackets, 1);
        assert!(InputShape::of("f <= _ x: Int = /* doubles */").opens_body());
    }

    #[test]
    fn nothing_waits_for_a_line_after_an_operator_or_a_comment_line() {
        for input in [
            "y <= 1 +",
            "x: Int",
            "stdout(\"a\") >=>",
            "y <= 1 ==",
            "y <= 1 >=",
        ] {
            assert!(!incomplete(input), "{input:?} should run");
        }
        // A comment line does not end the body, and one after the `=` is
        // not part of the code.
        for input in [
            "add x: Int y: Int =\n  x + y\n// returns the sum",
            "g <= _ x: Int = /* triple */\n  y <= x * 3",
        ] {
            assert!(incomplete(input), "{input:?} should continue");
        }
    }

    #[test]
    fn a_line_at_the_margin_ends_a_block_bodied_definition_that_parses() {
        let pending = "f <= _ n: Int =\n  n * 2";
        assert!(repl_line_ends_definition(pending, "f(3)"));
        assert!(!repl_line_ends_definition(pending, "  n * 3"));
        assert!(!repl_line_ends_definition(pending, "=> :Int"));
        // A comment is not code, so it does not end the body.
        assert!(!repl_line_ends_definition(pending, "// note"));
        assert!(!repl_line_ends_definition(pending, "/* note */"));
        // A definition that does not parse yet is still being written.
        assert!(!repl_line_ends_definition("f <= _ n: Int =\n  (", "f(3)"));
    }

    #[test]
    fn repl_runs_a_finished_input_and_reports_a_real_syntax_error() {
        for input in [
            "x <= 42",
            "add x: Int y: Int =\n  x + y\n=> :Int",
            "xs <= @[1,\n2, 3]",
            // A syntax error in the middle is reported, not waited on.
            "x <= 1 2 )",
            "stdout(\"abc",
        ] {
            assert!(!incomplete(input), "{input:?} should run");
        }
    }
}
