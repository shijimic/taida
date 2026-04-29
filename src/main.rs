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
use taida::diagnostics::split_diag_code_and_hint;
use taida::doc;
use taida::graph::ai_format;
use taida::graph::verify;
use taida::interpreter::Interpreter;
use taida::js;
use taida::module_graph;
use taida::parser::{BuchiField, Expr, FieldDef, FuncDef, Program, Statement, parse};
use taida::pkg;
use taida::types::{CompileTarget, TypeChecker};
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
  build       Build Native, JS, or WASM output
  way         Quality hub: check, lint, verify, todo
  graph       AI-oriented structural JSON for codebase comprehension
  doc         Generate docs from doc comments
  ingot       Package/dependency hub: deps, install, update, publish, cache
  init        Initialize a Taida project
  lsp         Run the language server over stdio
  auth        Manage authentication state
  community   Access community features
  upgrade     Upgrade taida to a newer version

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
  taida graph summary [--format text|json|sarif] <PATH>

Options:
  --recursive, -r   Follow imports recursively and produce unified multi-module JSON
  --output, -o      Output path (bare filename writes into .taida/graph/)
  --format, -f      Summary output format: text | json | sarif

Output:
  AI-oriented unified JSON — types, functions, flow, imports, exports

Examples:
  taida graph examples/04_functions.td
  taida graph summary --format json examples/04_functions.td
  taida graph --recursive examples/complex/inventory/main.td
  taida graph -o snapshot.json examples/04_functions.td"
    );
}

fn print_graph_summary_help() {
    println!(
        "\
Usage:
  taida graph summary [--format text|json|sarif] <PATH>

Options:
  --format, -f    text | json | sarif

Examples:
  taida graph summary main.td
  taida graph summary --format sarif main.td"
    );
}

fn print_way_help() {
    println!(
        "\
Usage:
  taida way <PATH>
  taida way check <PATH>
  taida way lint <PATH>
  taida way verify <PATH>
  taida way todo [PATH]

Commands:
  check    Parse + type front gate
  lint     Naming-convention lint
  verify   Structural verification checks
  todo     Scan TODO/Stub molds

Notes:
  `taida way <PATH>` is the full quality gate. It runs check, lint, and verify.
  `--no-check` is not accepted under `taida way`."
    );
}

fn print_ingot_help() {
    println!(
        "\
Usage:
  taida ingot [--help]
  taida ingot deps
  taida ingot install [--force-refresh | --no-remote-check] [--allow-local-addon-build]
  taida ingot update [--allow-local-addon-build]
  taida ingot publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]
  taida ingot cache [clean] [--addons|--store|--store-pkg <org>/<name>|--all] [--yes]

Commands:
  deps      Resolve/install dependencies strictly
  install   Install dependencies and write lockfile
  update    Update dependencies and lockfile
  publish   Push a package tag; CI creates release assets
  cache     Manage WASM/runtime/addon caches

Notes:
  `taida ingot` without a subcommand prints this help and exits successfully.
  Dependencies are declared in packages.tdm with `>>> author/pkg@a.1`.
  `taida ingot <author/package>` is not a supported form."
    );
}

fn print_check_help() {
    println!(
        "\
Usage:
  taida way check [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>

Options:
  --format, -f    text | json | jsonl | sarif
  --strict        Treat WARNING diagnostics as failure
  --quiet, -q     Suppress diagnostic output

Examples:
  taida way check src
  taida way check --format json main.td"
    );
}

fn print_build_help() {
    println!(
        "\
Usage:
  taida build [native|js|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] <PATH>

Options:
  --output, -o    Output file or directory
  --outdir        Alias of `--output`
  --entry         Native dir entry override (default: main.td)
  --release, -r   Fail if TODO/Stub remains in source
  --no-cache      Disable WASM runtime .o cache
  --diag-format   text | jsonl

Examples:
  taida build app.td
  taida build js src
  taida build --release app.td

Notes:
  Target defaults to native when omitted.
  `--no-check` is a global option and applies here."
    );
}

fn print_todo_help() {
    println!(
        "\
Usage:
  taida way todo [--format text|json|jsonl|sarif] [--strict] [--quiet] [PATH]

Options:
  --format, -f    text | json | jsonl | sarif
  --strict        Accepted for `way` flag consistency
  --quiet, -q     Suppress diagnostic output

Examples:
  taida way todo
  taida way todo --format json src"
    );
}

fn print_verify_help() {
    println!(
        "\
Usage:
  taida way verify [--check CHECK] [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>

Options:
  --check, -c     Run a specific check (repeatable)
  --format, -f    text | json | jsonl | sarif
  --strict        Treat WARNING findings as failure
  --quiet, -q     Suppress diagnostic output

Examples:
  taida way verify src
  taida way verify --check error-coverage --format jsonl main.td"
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
  taida ingot deps

Behavior:
  Resolve dependencies strictly and stop before install/lockfile update on any error.

Example:
  taida ingot deps"
    );
}

fn print_install_help() {
    println!(
        "\
Usage:
  taida ingot install [--force-refresh | --no-remote-check] [--allow-local-addon-build]

Behavior:
  Install resolved dependencies and generate/update `.taida/taida.lock`.

  For addons with a `[library.prebuild]` section in `native/addon.toml`,
  downloads the prebuild binary for the current host target, verifies its
  SHA-256 against the manifest, and places it in
  `.taida/deps/<pkg>/native/lib<name>.<ext>`. Downloads are cached under
  `~/.taida/addon-cache/`; use `taida ingot cache clean --addons` to prune.

  Large addon downloads (>= 256 KiB) show a progress indicator on stderr
  (RC15B-002).

  C17: before reusing a cached `~/.taida/store/<pkg>/<version>/` entry,
  `taida ingot install` compares the resolved commit SHA of `<version>` on the
  remote with the `commit_sha` recorded in the store `_meta.toml` sidecar.
  When they differ (tag was retagged / recreated), the store entry is
  re-extracted automatically. Offline or unverifiable states emit a
  warning to stderr but never silently skip.

Options:
  --force-refresh              Invalidate the cached store entry for every
                               registry dependency and re-extract it. Also
                               ignores the addon-cache (legacy behaviour).
                               Mutually exclusive with --no-remote-check.
  --no-remote-check            Skip the remote commit-SHA lookup; trust the
                               existing store sidecar. Intended for offline
                               or rate-limited environments. Mutually
                               exclusive with --force-refresh.
  --allow-local-addon-build    When a prebuild is missing or unavailable, fall back
                               to building the addon from source using `cargo build`.
                               Integrity mismatches are never overridden by fallback.

Example:
  taida ingot install
  taida ingot install --force-refresh
  taida ingot install --no-remote-check
  taida ingot install --allow-local-addon-build"
    );
}

fn print_update_help() {
    println!(
        "\
Usage:
  taida ingot update [--allow-local-addon-build]

Behavior:
  Resolve dependencies with remote-preferred generation lookup, then reinstall and update lockfile.

Options:
  --allow-local-addon-build    When a prebuild is missing or unavailable, fall back
                               to building the addon from source using `cargo build`.
                               Integrity mismatches are never overridden by fallback.

Example:
  taida ingot update
  taida ingot update --allow-local-addon-build"
    );
}

#[cfg(feature = "community")]
fn print_publish_help() {
    println!(
        "\
Usage:
  taida ingot publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]

C14 tag-only publish:
  `taida ingot publish` only creates and pushes a git tag. It does not build
  cdylibs, does not compute SHA-256 digests, does not push to `main`,
  and does not call `gh release create`. The addon's CI
  (`.github/workflows/release.yml`) is the exclusive owner of release
  artefact build and upload — the release author will be
  `github-actions[bot]`, not the CLI user.

Options:
  --label LABEL            Attach a pre-release label (rc, rc2, beta, alpha-1, ...)
                           Applied on top of the auto-detected next version.
  --force-version VERSION  Override the auto-detected version. Must be a
                           valid Taida version (`gen.num(.label)?`).
  --retag                  Allow re-tagging an existing tag. The existing
                           remote tag is force-replaced.
  --dry-run                Print the publish plan (next version, tag, push
                           target) without making any git changes.

Auto version bump:
  - First release (no previous tag)      -> a.1
  - Public API removal or rename         -> generation bump (a.3 -> b.1)
  - Public API addition or internal only -> number bump     (a.3 -> a.4)

Examples:
  taida ingot publish --dry-run
  taida ingot publish
  taida ingot publish --label rc
  taida ingot publish --force-version a.5
  taida ingot publish --force-version a.5.rc --retag"
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WayFormat {
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
struct WayOptions {
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

fn reject_no_check_under_way() -> ! {
    eprintln!(
        "--no-check is not allowed under 'taida way'. The way hub exists to run quality checks."
    );
    std::process::exit(2);
}

fn way_should_fail(errors: usize, warnings: usize, strict: bool) -> bool {
    errors > 0 || (strict && warnings > 0)
}

fn parse_way_format_or_exit(raw: &str, command: &str) -> WayFormat {
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

fn push_way_options_args(out: &mut Vec<String>, options: WayOptions) {
    if options.strict {
        out.push("--strict".to_string());
    }
    if options.quiet {
        out.push("--quiet".to_string());
    }
    match options.format {
        WayFormat::Text => {}
        WayFormat::Json => {
            out.push("--format".to_string());
            out.push("json".to_string());
        }
        WayFormat::Jsonl => {
            out.push("--format".to_string());
            out.push("jsonl".to_string());
        }
        WayFormat::Sarif => {
            out.push("--format".to_string());
            out.push("sarif".to_string());
        }
    }
}

fn removed_command_replacement(command: &str) -> Option<&'static str> {
    match command {
        "check" => Some("taida way check"),
        "verify" => Some("taida way verify"),
        "lint" => Some("taida way lint"),
        "todo" => Some("taida way todo"),
        "inspect" => Some("taida graph summary"),
        "transpile" => Some("taida build js"),
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
        "[E1700] Migration command '{}' is not available in @e.X. E31 does not provide AST migration tooling.",
        invocation
    );
    eprintln!(
        "        Update source files manually; run `taida upgrade --help` for self-upgrade usage."
    );
    std::process::exit(2);
}

fn run_way(args: &[String], no_check: bool) {
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

fn run_way_full(args: &[String]) {
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

    let mut sub_args = Vec::new();
    push_way_options_args(&mut sub_args, options);
    sub_args.push(path.clone());
    run_check_cmd(&sub_args);

    let mut sub_args = Vec::new();
    push_way_options_args(&mut sub_args, options);
    sub_args.push(path.clone());
    run_lint_cmd(&sub_args);

    let mut sub_args = Vec::new();
    push_way_options_args(&mut sub_args, options);
    sub_args.push(path);
    run_verify(&sub_args);
}

fn run_ingot(args: &[String]) {
    if args.is_empty() || is_help_flag(args[0].as_str()) {
        print_ingot_help();
        return;
    }

    match args[0].as_str() {
        "deps" => run_deps(&args[1..]),
        "install" => run_install(&args[1..]),
        "update" => run_update(&args[1..]),
        #[cfg(feature = "community")]
        "publish" => run_publish(&args[1..]),
        #[cfg(not(feature = "community"))]
        "publish" => {
            eprintln!("The 'taida ingot publish' command requires the 'community' feature.");
            eprintln!("Rebuild with: cargo build --features community");
            std::process::exit(1);
        }
        "cache" => run_cache(&args[1..]),
        other => {
            eprintln!("Unknown subcommand for `taida ingot`: {}", other);
            eprintln!("Run `taida ingot --help` for usage.");
            std::process::exit(2);
        }
    }
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

fn run_check_cmd(args: &[String]) {
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
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida way check --help` for usage.");
                    std::process::exit(1);
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
        );
    }

    if way_should_fail(errors, warnings, options.strict) {
        std::process::exit(1);
    }
}

fn emit_check_diagnostics(
    diagnostics: &[CheckDiagnostic],
    files: usize,
    options: WayOptions,
    errors: usize,
    warnings: usize,
    infos: usize,
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
            println!(
                "{}",
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
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
                        "locations": [{
                            "physicalLocation": {
                                "artifactLocation": { "uri": d.file },
                                "region": {
                                    "startLine": d.line,
                                    "startColumn": d.column,
                                }
                            }
                        }]
                    })
                })
                .collect();
            println!(
                "{}",
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
                .unwrap_or_else(|_| "{}".to_string())
            );
        }
    }
}

fn check_diagnostics_json(
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

// ── Lint subcommand (D28B-008) ──────────────────────────

fn print_lint_help() {
    println!(
        "\
Usage:
  taida way lint [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>

Description:
  Run the D28B-008 naming-convention lint pass over <PATH>. <PATH> may be
  a single .td file or a directory (.td files are collected recursively).
  The lint pins the D28B-001 (Phase 0 2026-04-26) category-based naming
  rules and emits diagnostics in the E1801..E1809 band.

Exit codes:
  0   No lint diagnostics surfaced.
  1   At least one E18xx diagnostic was reported.
  2   Argument / IO / parse / type error (lint cannot run cleanly).

Options:
  --format, -f    text | json | jsonl | sarif
  --strict        Treat lint diagnostics as failure (same as default)
  --quiet         Suppress diagnostic output, exit code only.
  --help, -h      Show this help.

Examples:
  taida way lint examples
  taida way lint --quiet src/main.td"
    );
}

fn run_lint_cmd(args: &[String]) {
    use taida::parser::lint::lint_program_with_source;

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
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida way lint --help` for usage.");
                    std::process::exit(1);
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

    let target_path = Path::new(&target);
    let td_files: Vec<PathBuf> = if target_path.is_dir() {
        let files = collect_td_files(target_path);
        if files.is_empty() {
            eprintln!("No .td files found in '{}'", target);
            std::process::exit(2);
        }
        files
    } else {
        vec![target_path.to_path_buf()]
    };

    let mut total_diags: usize = 0;
    let mut had_parse_error: bool = false;
    let mut report = verify::VerifyReport::new();

    for td_file in &td_files {
        let file_str = td_file.to_string_lossy().to_string();
        let source = match fs::read_to_string(td_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: read error: {}", file_str, e);
                had_parse_error = true;
                continue;
            }
        };
        let (program, parse_errors) = parse(&source);
        if !parse_errors.is_empty() {
            // Lint cannot run cleanly when parse errors are present.
            // Skip this file and report.
            had_parse_error = true;
            if !options.quiet {
                eprintln!(
                    "{}: parse errors prevent lint ({} error(s))",
                    file_str,
                    parse_errors.len()
                );
            }
            continue;
        }
        let diags = lint_program_with_source(&program, &source);
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

    if had_parse_error {
        // Argument-level failure (lint could not clean-run somewhere)
        std::process::exit(2);
    }
    if !options.quiet {
        match options.format {
            WayFormat::Text => {}
            WayFormat::Json => println!("{}", report.format_json()),
            WayFormat::Jsonl => print!("{}", report.format_jsonl(&["naming-convention"])),
            WayFormat::Sarif => print!("{}", report.format_sarif(&["naming-convention"])),
        }
    }
    if total_diags > 0 {
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

// ── Upgrade subcommand ──────────────────────────────────────

fn print_upgrade_help() {
    println!(
        "\
Usage:
  taida upgrade [--check] [--gen GEN] [--label LABEL] [--version VERSION]

Options:
  --check          Check for updates without installing
  --gen GEN        Filter by generation (e.g. b)
  --label LABEL    Filter by label (e.g. rc2)
  --version VER    Upgrade to an exact version (e.g. @b.10.rc2)

Notes:
  --gen and --label can be combined.
  --version is mutually exclusive with --gen/--label.
  By default, upgrades to the latest stable version.
  AST rewrite flags (`--d28`, `--d29`, `--e30`) were removed in @e.X.
  No migration command is provided.
  Windows: only --check is supported (self-replace is not yet implemented).

Examples:
  taida upgrade
  taida upgrade --check
  taida upgrade --label rc2
  taida upgrade --gen b
  taida upgrade --version @b.10.rc2"
    );
}

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

fn print_build_usage_and_exit() -> ! {
    eprintln!(
        "\
Usage:
  taida build [native|js|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] <PATH>

Options:
  --output, -o    Output file or directory
  --outdir        Alias of `--output`
  --entry         Native dir entry override (default: main.td)
  --release, -r   Fail if TODO/Stub remains in source
  --no-cache      Disable WASM runtime .o cache
  --diag-format   text | jsonl"
    );
    std::process::exit(1);
}

fn reject_removed_build_target_flag() -> ! {
    eprintln!(
        "[E1700] Flag '--target <target>' was removed in @e.X. Use 'taida build <target> <PATH>' instead."
    );
    eprintln!("        For example: `taida build js src`.");
    std::process::exit(2);
}

fn run_build(args: &[String], no_check: bool) {
    let mut target = BuildTarget::Native;
    let mut target_seen = false;
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
                reject_removed_build_target_flag();
            }
            raw if raw.starts_with("--target=") => {
                reject_removed_build_target_flag();
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
            raw => {
                if !target_seen
                    && input_path.is_none()
                    && let Some(parsed) = BuildTarget::parse(raw)
                {
                    target = parsed;
                    target_seen = true;
                    i += 1;
                    continue;
                }
                if input_path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida build.");
                    print_build_usage_and_exit();
                }
                input_path = Some(raw.to_string());
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
                        "`--entry` is only valid with `taida build native`.",
                        None,
                        None,
                        None,
                        None,
                    );
                } else {
                    eprintln!("`--entry` is only valid with `taida build native`.");
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
        run_type_checks_and_warnings(
            &program,
            source_label,
            CompileTarget::Js,
            diag_format,
            compile_stats,
        );
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
                "`taida build js --release /dev/stdin` is not supported.",
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
            CompileTarget::Native,
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
            CompileTarget::WasmMin,
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
            CompileTarget::WasmWasi,
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
            CompileTarget::WasmEdge,
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
            CompileTarget::WasmFull,
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
    compile_target: CompileTarget,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    let mut checker = TypeChecker::new();
    checker.set_compile_target(compile_target);
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
        Statement::EnumDef(_) => {}
        // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
        Statement::ClassLikeDef(cl) => {
            for field in &cl.fields {
                scan_field_defaults(field, file, out);
            }
        }
        Statement::FuncDef(fd) => scan_func_for_todo(fd, file, out),
        Statement::Assignment(assign) => scan_expr_for_todo(&assign.value, file, out),
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
        | Expr::EnumVariant(_, _, _)
        | Expr::TypeLiteral(_, _, _)
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
                    eprintln!("Missing value for --format.");
                    eprintln!("Run `taida way todo --help` for usage.");
                    std::process::exit(1);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "todo");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for todo: {}", raw);
                eprintln!("Run `taida way todo --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one [PATH] is accepted for taida way todo.");
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

    if options.quiet {
        return;
    }

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
        .iter()
        .map(|(id, count)| json!({ "id": id, "count": count }))
        .collect();
    let by_file_json: Vec<serde_json::Value> = by_file
        .iter()
        .map(|(file, count)| json!({ "file": file, "count": count }))
        .collect();
    let output = json!({
        "total": merged.todos.len(),
        "todos": todos_json,
        "byId": by_id_json,
        "byFile": by_file_json,
    });

    match options.format {
        WayFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
            );
            return;
        }
        WayFormat::Jsonl => {
            for todo in &merged.todos {
                println!(
                    "{}",
                    json!({
                        "schema": "taida.diagnostic.v1",
                        "stream": "todo",
                        "kind": "finding",
                        "code": null,
                        "message": todo.task,
                        "location": {
                            "file": todo.file,
                            "line": todo.line,
                            "column": todo.column,
                        },
                        "suggestion": null,
                        "severity": "INFO",
                        "id": todo.id,
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
                        "total": merged.todos.len(),
                        "errors": 0,
                        "warnings": 0,
                        "info": merged.todos.len(),
                    }
                })
            );
            return;
        }
        WayFormat::Sarif => {
            let results: Vec<serde_json::Value> = merged
                .todos
                .iter()
                .map(|todo| {
                    json!({
                        "ruleId": "todo",
                        "level": "note",
                        "message": { "text": todo.task },
                        "locations": [{
                            "physicalLocation": {
                                "artifactLocation": { "uri": todo.file },
                                "region": {
                                    "startLine": todo.line,
                                    "startColumn": todo.column,
                                }
                            }
                        }]
                    })
                })
                .collect();
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

fn run_verify(args: &[String]) {
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
                    eprintln!("Missing value for --check.");
                    eprintln!("Run `taida way verify --help` for usage.");
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
                    eprintln!("Run `taida way verify --help` for usage.");
                    std::process::exit(1);
                }
                options.format = parse_way_format_or_exit(args[i].as_str(), "verify");
            }
            "--strict" => options.strict = true,
            "--quiet" | "-q" => options.quiet = true,
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for verify: {}", raw);
                eprintln!("Run `taida way verify --help` for usage.");
                std::process::exit(1);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Only one <PATH> is accepted for taida way verify.");
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
            eprintln!("Run `taida way verify --help` for usage.");
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
    if !options.quiet {
        let output = match options.format {
            WayFormat::Json => report.format_json(),
            WayFormat::Jsonl => report.format_jsonl(&checks_ref),
            WayFormat::Sarif => report.format_sarif(&checks_ref),
            WayFormat::Text => report.format_text(&checks_ref),
        };
        print!("{}", output);
    }

    if way_should_fail(report.errors(), report.warnings(), options.strict) {
        std::process::exit(1);
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
            eprintln!("Run `taida ingot deps --help` for usage.");
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

    // Strict mode for `taida ingot deps`: never install or write lockfile on resolve errors.
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
    // RC2.7-4a: parse --allow-local-addon-build flag
    // C17-2: parse --no-remote-check (mutually exclusive with --force-refresh)
    let mut force_refresh = false;
    let mut no_remote_check = false;
    let mut allow_local_addon_build = false;
    let mut filtered: Vec<&str> = Vec::new();
    for arg in args {
        if arg == "--force-refresh" {
            force_refresh = true;
        } else if arg == "--no-remote-check" {
            no_remote_check = true;
        } else if arg == "--allow-local-addon-build" {
            allow_local_addon_build = true;
        } else if is_help_flag(arg.as_str()) {
            print_install_help();
            return;
        } else {
            filtered.push(arg.as_str());
        }
    }
    if !filtered.is_empty() {
        eprintln!("Unexpected arguments.");
        eprintln!("Run `taida ingot install --help` for usage.");
        std::process::exit(1);
    }
    // C17-2: mutual exclusion is a hard error so users cannot silently
    // combine the two refresh knobs with surprising semantics.
    let refresh_flags = pkg::resolver::StoreRefreshFlags {
        force_refresh,
        no_remote_check,
    };
    if let Err(msg) = refresh_flags.validate() {
        eprintln!("Error: {}", msg);
        eprintln!("Run `taida ingot install --help` for usage.");
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
    // pinning generation-only versions to locked exact versions when available.
    // C17-2: forward refresh flags so the StoreProvider can consult the
    // stale-detection decision table (or bypass it for --force-refresh).
    let result = match &existing_lockfile {
        Some(lf) => pkg::resolver::resolve_deps_locked_with_flags(&manifest, lf, refresh_flags),
        None => pkg::resolver::resolve_deps_with_flags(&manifest, refresh_flags),
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
        // RC2.7-4b: pass allow_local_addon_build for fallback policy
        let existing_lock = pkg::lockfile::Lockfile::read(&lock_path).unwrap_or(None);
        addon_map = match pkg::resolver::install_addon_prebuilds(
            &manifest,
            &result,
            force_refresh,
            existing_lock.as_ref(),
            allow_local_addon_build,
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
    // RC2.7-4a / RC2.7B-005: parse --allow-local-addon-build flag
    let mut allow_local_addon_build = false;
    for arg in args {
        if arg == "--allow-local-addon-build" {
            allow_local_addon_build = true;
        } else if is_help_flag(arg.as_str()) {
            print_update_help();
            return;
        } else {
            eprintln!("Unexpected arguments.");
            eprintln!("Run `taida ingot update --help` for usage.");
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
                    "Updated {} package(s) in .taida/deps/",
                    result.packages.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }

        // RC2.7B-011: install addon prebuilds after deps are recreated.
        // Without this, `taida ingot update` destroys addon binaries because
        // `install_deps` recreates `.taida/deps` from scratch.
        let lock_path = project_dir.join(".taida").join("taida.lock");
        let existing_lock = pkg::lockfile::Lockfile::read(&lock_path).unwrap_or(None);
        addon_map = match pkg::resolver::install_addon_prebuilds(
            &manifest,
            &result,
            false, // force_refresh: update fetches latest versions but does not bypass cache
            existing_lock.as_ref(),
            allow_local_addon_build,
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

    // RC2.7B-012: use write_lockfile_with_addons to preserve addon stanzas.
    // The old write_lockfile call would discard all [[package.addon]] entries.
    match pkg::resolver::write_lockfile_with_addons(&manifest, &result, &addon_map) {
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
/// C14-1: `taida ingot publish` is now a tag-push-only command.
///
/// Flow:
///
///   1. Find the `packages.tdm` in the current tree and parse it.
///   2. Validate the manifest identity (`<<<@version owner/name`
///      required; bare names are rejected).
///   3. Cross-check identity against `origin` (GitHub URL, exact
///      `owner/repo` match).
///   4. Check the working tree is clean.
///   5. Compute the next version from the public API diff (or honour
///      `--force-version`).
///   6. Detect tag collision (reject unless `--retag`).
///   7. `--dry-run` prints the plan and exits.
///   8. Otherwise, `git tag <next> && git push origin <tag>`. Done.
///
/// `taida ingot publish` does not build cdylibs, compute SHA-256, mutate
/// `packages.tdm`, push to `main`, or call `gh release create`. All
/// release artefact work is delegated to the addon
/// `release.yml` running as `github-actions[bot]`.
fn run_publish(args: &[String]) {
    // ── CLI parsing ──────────────────────────────────────
    let mut label: Option<String> = None;
    let mut force_version: Option<String> = None;
    let mut retag = false;
    let mut dry_run = false;

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
                    eprintln!("Run `taida ingot publish --help` for usage.");
                    std::process::exit(1);
                }
                label = Some(args[i].clone());
            }
            "--force-version" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --force-version.");
                    eprintln!("Run `taida ingot publish --help` for usage.");
                    std::process::exit(1);
                }
                force_version = Some(args[i].clone());
            }
            "--retag" => retag = true,
            "--dry-run" => dry_run = true,
            raw if raw.starts_with("--dry-run=") => {
                eprintln!(
                    "`--dry-run=<mode>` was removed in @c.14.rc1. Use plain `--dry-run` instead."
                );
                eprintln!("Run `taida ingot publish --help` for the new flow.");
                std::process::exit(1);
            }
            "--target" => {
                eprintln!(
                    "`--target` was removed in @c.14.rc1. `taida ingot publish` now only pushes a git tag; \
                     addon builds happen in CI via `.github/workflows/release.yml`."
                );
                eprintln!("Run `taida ingot publish --help` for the new flow.");
                std::process::exit(1);
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for publish: {}", raw);
                eprintln!("Run `taida ingot publish --help` for usage.");
                std::process::exit(1);
            }
            other => {
                eprintln!("Unexpected argument for publish: {}", other);
                eprintln!("Run `taida ingot publish --help` for usage.");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // ── Project discovery ──────────────────────────────────
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

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

    // ── Invariant: working tree must be clean ──────────────
    if let Err(e) = pkg::publish::check_worktree_clean(&project_dir) {
        eprintln!("Publish refused: {}", e);
        std::process::exit(1);
    }

    // ── Plan ───────────────────────────────────────────────
    let plan = match pkg::publish::plan_publish(
        &project_dir,
        &manifest,
        label.as_deref(),
        force_version.as_deref(),
        retag,
    ) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("Publish refused: {}", e);
            std::process::exit(1);
        }
    };

    // ── Dry-run exits after printing the plan ──────────────
    if dry_run {
        print!("{}", pkg::publish::render_plan(&plan));
        return;
    }

    // ── Authentication check (real run only) ───────────────
    if let Err(e) = pkg::publish::check_gh_auth() {
        eprintln!("Publish refused: {}", e);
        std::process::exit(1);
    }

    // ── Tag + push (the only git-mutating step) ────────────
    if let Err(e) = pkg::publish::tag_and_push(&project_dir, &plan.next_version, plan.retag) {
        eprintln!("Publish failed: {}", e);
        std::process::exit(1);
    }

    // ── Report and exit ────────────────────────────────────
    println!(
        "Pushed tag {} for {} to {}.",
        plan.next_version, plan.package_id, plan.remote
    );
    println!("CI (`.github/workflows/release.yml`) will build artefacts and create the release.");
}

fn run_cache(args: &[String]) {
    if args.is_empty() || args.iter().any(|a| is_help_flag(a.as_str())) {
        println!("Usage: taida ingot cache <command> [options]");
        println!();
        println!("Commands:");
        println!("  clean                       Remove cached WASM runtime .o files (default)");
        println!("  clean --addons              Remove cached addon prebuild binaries");
        println!("                              (RC15B-001: prunes ~/.taida/addon-cache/)");
        println!("  clean --store [--yes]       C17: prune ~/.taida/store/ (shows a summary");
        println!("                              first; then asks to confirm interactively on a");
        println!("                              TTY, or requires --yes in non-TTY contexts)");
        println!("  clean --store-pkg <org>/<name>   C17: prune a single store package");
        println!("                              (no confirmation prompt; scope is narrow)");
        println!("  clean --all [--yes]         Remove WASM + addon cache + store (C17)");
        return;
    }

    match args[0].as_str() {
        "clean" => {
            // RC15B-001: parse optional --addons / --all flags.
            // C17-3: add --store / --store-pkg / --yes flags.
            let mut clean_wasm = true;
            let mut clean_addons = false;
            let mut clean_store = false;
            let mut store_pkg: Option<String> = None;
            let mut assume_yes = false;

            let mut i = 1;
            while i < args.len() {
                let extra = args[i].as_str();
                match extra {
                    "--addons" => {
                        clean_wasm = false;
                        clean_addons = true;
                    }
                    "--store" => {
                        clean_wasm = false;
                        clean_store = true;
                    }
                    "--store-pkg" => {
                        clean_wasm = false;
                        i += 1;
                        if i >= args.len() {
                            eprintln!("Missing value for --store-pkg. Expected <org>/<name>.");
                            std::process::exit(1);
                        }
                        store_pkg = Some(args[i].clone());
                    }
                    "--all" => {
                        clean_wasm = true;
                        clean_addons = true;
                        clean_store = true;
                    }
                    "--yes" | "-y" => {
                        assume_yes = true;
                    }
                    other => {
                        eprintln!(
                            "Unknown flag '{}' for 'taida ingot cache clean'. \
                             Use --addons, --store, --store-pkg <org>/<name>, --all, or no flag.",
                            other
                        );
                        std::process::exit(1);
                    }
                }
                i += 1;
            }

            // --store-pkg is mutually exclusive with --store / --all:
            // targeted prune should not also wipe the whole store.
            if store_pkg.is_some() && (clean_store || (clean_wasm && clean_addons)) {
                eprintln!(
                    "--store-pkg cannot be combined with --store or --all. \
                     Use one or the other."
                );
                std::process::exit(1);
            }

            if clean_wasm {
                run_cache_clean();
            }
            if clean_addons {
                run_cache_clean_addons();
            }
            if clean_store {
                run_cache_clean_store(assume_yes);
            }
            if let Some(pkg) = store_pkg {
                run_cache_clean_store_pkg(&pkg);
            }
        }
        other => {
            eprintln!(
                "Unknown cache command '{}'. Use 'taida ingot cache clean'.",
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

// C17-3: prune `~/.taida/store/` (all packages, all versions).
//
// Shows a summary first. Requires confirmation (`y` / `yes` / `Y` / `YES`
// on stdin) unless `--yes` is passed. Non-TTY stdin must pass `--yes`
// explicitly so scripts do not wipe the store accidentally.
fn run_cache_clean_store(assume_yes: bool) {
    let store_root = match taida::util::taida_home_dir() {
        Ok(home) => home.join(".taida").join("store"),
        Err(e) => {
            eprintln!("Cannot locate taida home directory: {}", e);
            std::process::exit(1);
        }
    };
    let summary = match taida::pkg::store::summarize_store_root(&store_root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading store: {}", e);
            std::process::exit(1);
        }
    };
    if !summary.root_existed {
        println!("No store cache found at '{}'.", summary.root.display());
        return;
    }
    if summary.packages_removed == 0 && summary.scratch_removed == 0 {
        println!(
            "Store cache at '{}' is already clean.",
            summary.root.display()
        );
        return;
    }

    // Show summary.
    let mib = summary.bytes_removed as f64 / (1024.0 * 1024.0);
    println!(
        "Store cache at '{}' contains {} package(s), {:.2} MiB.",
        summary.root.display(),
        summary.packages_removed,
        mib
    );
    // C17B-011: report scratch (leftover .tmp-*, .refresh-staging-*)
    // separately so the user sees what is being cleaned up without the
    // count inflating the package number.
    if summary.scratch_removed > 0 {
        println!(
            "  ... and {} leftover scratch directory(ies) from past installs",
            summary.scratch_removed
        );
    }
    // Preview the first few so a user can sanity-check.
    let preview_n = 10usize;
    for name in summary.packages.iter().take(preview_n) {
        println!("  {}", name);
    }
    if summary.packages.len() > preview_n {
        println!("  ... and {} more", summary.packages.len() - preview_n);
    }

    if !assume_yes {
        use std::io::Write;
        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if !is_tty {
            eprintln!(
                "Refusing to prune store in a non-TTY context without --yes. \
                 Re-run with `taida ingot cache clean --store --yes`."
            );
            std::process::exit(1);
        }
        print!("Remove all {} package(s)? [y/N] ", summary.packages_removed);
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            eprintln!("No input received; aborting.");
            std::process::exit(1);
        }
        let answer = answer.trim();
        if !matches!(answer, "y" | "Y" | "yes" | "YES") {
            println!("Aborted.");
            return;
        }
    }

    match taida::pkg::store::prune_store_root(&store_root) {
        Ok(report) => {
            let mib = report.bytes_removed as f64 / (1024.0 * 1024.0);
            println!(
                "Removed {} package(s) ({:.2} MiB) from '{}'.",
                report.packages_removed,
                mib,
                report.root.display()
            );
        }
        Err(e) => {
            eprintln!("Error pruning store: {}", e);
            std::process::exit(1);
        }
    }
}

// C17-3: prune a single package from the store (all versions of
// `<org>/<name>/*`). No confirmation is required since the scope is
// narrow.
fn run_cache_clean_store_pkg(pkg_spec: &str) {
    let (org, name) = match pkg_spec.split_once('/') {
        Some((o, n)) if !o.is_empty() && !n.is_empty() && !n.contains('/') => (o, n),
        _ => {
            eprintln!(
                "Invalid --store-pkg value '{}'. Expected <org>/<name>.",
                pkg_spec
            );
            std::process::exit(1);
        }
    };
    let store_root = match taida::util::taida_home_dir() {
        Ok(home) => home.join(".taida").join("store"),
        Err(e) => {
            eprintln!("Cannot locate taida home directory: {}", e);
            std::process::exit(1);
        }
    };
    match taida::pkg::store::prune_store_package(&store_root, org, name) {
        Ok(report) => {
            if !report.root_existed {
                println!("No store cache found at '{}'.", report.root.display());
                return;
            }
            if report.packages_removed == 0 {
                println!(
                    "Package '{}/{}' not found in store at '{}'.",
                    org,
                    name,
                    report.root.display()
                );
                return;
            }
            let mib = report.bytes_removed as f64 / (1024.0 * 1024.0);
            println!(
                "Removed {} version(s) of {}/{} ({:.2} MiB) from '{}'.",
                report.packages_removed,
                org,
                name,
                mib,
                report.root.display()
            );
        }
        Err(e) => {
            eprintln!("Error pruning store package: {}", e);
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
}
