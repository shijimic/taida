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

use std::env;
use std::fs;

#[cfg(feature = "community")]
use taida::auth;
#[cfg(feature = "community")]
use taida::community;

mod cli;
use cli::build::*;
use cli::commands::*;
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
        "[E1700] Command '{}' was removed. Use '{}' instead.",
        command, replacement
    );
    eprintln!("        See `taida --help` for the current command set.");
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

    // F62B-005: everything after the first standalone `--` belongs to the
    // user program (argv()), never to taida — so `--no-check` is only
    // recognized as taida's own flag before that separator.
    let dashdash_pos = args.iter().position(|a| a == "--");
    let in_taida_zone = |i: usize| dashdash_pos.is_none_or(|p| i < p);
    // Check for --no-check flag
    let no_check = args
        .iter()
        .enumerate()
        .any(|(i, a)| a == "--no-check" && in_taida_zone(i));
    // Filter out --no-check from args for subcommand processing
    let filtered_args: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.as_str() == "--no-check" && in_taida_zone(*i)))
        .map(|(_, a)| a.clone())
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
                // F62B-005: expose only the user's arguments through argv():
                // everything after the first standalone `--`, or — when no
                // separator is given — everything after the script path.
                // taida's own options were filtered above and never leak.
                let user_args: Vec<String> = match filtered_args.iter().position(|a| a == "--") {
                    Some(pos) => filtered_args[pos + 1..].to_vec(),
                    None => filtered_args.get(2..).unwrap_or(&[]).to_vec(),
                };
                taida::interpreter::set_user_argv(user_args);
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

// ── Lint subcommand ──────────────────────────

// ── Compile / Transpile / Build subcommands ─────────────

// ── Upgrade subcommand ──────────────────────────────────────

// ── Graph subcommand ────────────────────────────────────

// ── Verify subcommand ───────────────────────────────────

// ── Init subcommand ──────────────────────────────────────

// ── Deps subcommand ──────────────────────────────────────

// ── Install subcommand ──────────────────────────────────

// ── Update subcommand ──────────────────────────────────

// ── Publish subcommand ─────────────────────────────────

// ── LSP server ─────────────────────────────────────────

// ── REPL ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::build_descriptor::{
        BuildUnitDescriptor, target_incompatible_import, validate_target_closure_modules,
    };
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use taida::parser::parse;
    use taida::parser::{ImportStmt, Statement};
    use taida::version::taida_version;

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
