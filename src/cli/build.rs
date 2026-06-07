//! build — split out of src/main.rs (pure move).
//! Behaviour unchanged; imports added per cargo check.

use serde_json::json;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "native")]
use taida::codegen;
use taida::diagnostics::split_diag_code_and_hint;
use taida::graph::verify;
use taida::js;
use taida::module_graph;
use taida::parser::{Program, Statement, TypeExpr, parse};
use taida::types::{CompileTarget, TypeChecker};

use crate::cli::build_descriptor::{DescriptorBuildFlags, run_build_descriptor_mode};
use crate::cli::help::{print_build_help, print_build_usage_and_exit};
use crate::cli::ingot::{find_packages_tdm, find_packages_tdm_from};
use crate::cli::way::{
    collect_release_gate_sites_for_files, collect_td_files, report_release_gate_violations,
    scan_release_gate_sites,
};

// JSONL diagnostics mirror the public record fields directly; bundling them
// into a transient struct would obscure the emitter contract at call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_compile_diag_jsonl(
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

pub(crate) fn emit_compile_summary_jsonl(stats: &CompileDiagStats) {
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

pub(crate) fn reject_removed_build_target_flag() -> ! {
    eprintln!(
        "[E1700] Flag '--target <target>' was removed in @e.X. Use 'taida build <target> <PATH>' instead."
    );
    eprintln!("        For example: `taida build native src`.");
    std::process::exit(2);
}

pub(crate) fn emit_build_cli_diagnostic_and_exit(
    compile_stats: &mut CompileDiagStats,
    diag_format: DiagFormat,
    code: &'static str,
    message: &str,
    suggestion: Option<&str>,
    exit_code: i32,
) -> ! {
    if diag_format == DiagFormat::Jsonl {
        emit_compile_diag_jsonl(
            compile_stats,
            "ERROR",
            "cli",
            Some(code.to_string()),
            message,
            None,
            None,
            None,
            suggestion.map(str::to_string),
        );
    } else {
        eprintln!("[{}] {}", code, message);
        if let Some(suggestion) = suggestion {
            eprintln!("        {}", suggestion);
        }
    }
    std::process::exit(exit_code);
}

pub(crate) fn run_build(args: &[String], no_check: bool) {
    let mut target = BuildTarget::Native;
    let mut target_seen = false;
    let mut diag_format = DiagFormat::Text;
    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut entry_path: Option<String> = None;
    let mut handler_symbol: Option<String> = None;
    let mut release_mode = false;
    let mut no_cache = false;
    let mut run_hooks = false;
    let mut descriptor_flags = DescriptorBuildFlags::default();

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
            "--handler" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                handler_symbol = Some(args[i].clone());
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
            "--unit" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                descriptor_flags.unit = Some(args[i].clone());
            }
            "--plan" => {
                i += 1;
                if i >= args.len() {
                    print_build_usage_and_exit();
                }
                descriptor_flags.plan = Some(args[i].clone());
            }
            "--all-units" => {
                descriptor_flags.all_units = true;
            }
            "--run-hooks" => {
                run_hooks = true;
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

    let selector_count = descriptor_flags.selector_count();
    if selector_count > 1 {
        emit_build_cli_diagnostic_and_exit(
            &mut compile_stats,
            diag_format,
            "E1901",
            "`--unit`, `--plan`, and `--all-units` are mutually exclusive.",
            Some("Use exactly one descriptor build selector."),
            2,
        );
    }
    if selector_count == 1 && target_seen {
        emit_build_cli_diagnostic_and_exit(
            &mut compile_stats,
            diag_format,
            "E1900",
            "Descriptor build mode does not accept a positional build target.",
            Some(
                "Use `taida build <PATH> --unit NAME` or single-target `taida build <target> <PATH>`.",
            ),
            2,
        );
    }
    if selector_count == 1 && entry_path.is_some() {
        emit_build_cli_diagnostic_and_exit(
            &mut compile_stats,
            diag_format,
            "E1900",
            "`--entry` is only valid in single-target native build mode.",
            Some("Descriptor BuildUnit.entry is a symbol, not a CLI file override."),
            2,
        );
    }
    if selector_count == 1 && handler_symbol.is_some() {
        emit_build_cli_diagnostic_and_exit(
            &mut compile_stats,
            diag_format,
            "E1900",
            "`--handler` is only valid in single-target Native/WASM build mode.",
            Some("Use BuildUnit.handler for descriptor builds."),
            2,
        );
    }
    if selector_count == 0 && run_hooks {
        emit_build_cli_diagnostic_and_exit(
            &mut compile_stats,
            diag_format,
            "E1900",
            "`--run-hooks` is only valid in descriptor build mode.",
            Some("Use `taida build <PATH> --unit NAME --run-hooks`."),
            2,
        );
    }
    if let Some(selector) = descriptor_flags.selector() {
        run_build_descriptor_mode(
            input_path,
            selector,
            run_hooks,
            release_mode,
            no_check,
            diag_format,
            &mut compile_stats,
        );
    }

    if handler_symbol.is_some() && !target.supports_handler() {
        emit_build_cli_diagnostic_and_exit(
            &mut compile_stats,
            diag_format,
            "E1900",
            "`--handler` is only valid with Native/WASM build targets.",
            Some(
                "Use `taida build native --handler handle app.td` or `taida build wasm-edge --handler handle app.td`.",
            ),
            2,
        );
    }

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
                handler_symbol.as_deref(),
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
                handler_symbol.as_deref(),
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
                handler_symbol.as_deref(),
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
                handler_symbol.as_deref(),
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
                handler_symbol.as_deref(),
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

pub(crate) fn run_build_js(
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

pub(crate) fn js_stage_roots() -> &'static Mutex<Vec<PathBuf>> {
    static JS_STAGE_ROOTS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    JS_STAGE_ROOTS.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn register_js_stage_root(stage_root: &Path) {
    let mut roots = js_stage_roots()
        .lock()
        .expect("js stage root registry mutex poisoned");
    roots.push(stage_root.to_path_buf());
}

pub(crate) fn unregister_js_stage_root(stage_root: &Path) {
    let mut roots = js_stage_roots()
        .lock()
        .expect("js stage root registry mutex poisoned");
    roots.retain(|root| root != stage_root);
}

pub(crate) fn cleanup_registered_js_stage_roots() {
    let mut roots = js_stage_roots()
        .lock()
        .expect("js stage root registry mutex poisoned");
    for root in roots.drain(..) {
        let _ = fs::remove_dir_all(root);
    }
}

pub(crate) fn emit_build_failure_and_exit(
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

pub(crate) fn is_stdin_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    raw == "/dev/stdin" || raw == "-" || raw.ends_with("/fd/0")
}

// The JS source transpile entry point keeps CLI flags and diagnostic sinks
// explicit so the file and stdin callers do not need adapter structs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn transpile_js_source_to_output(
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

// Module transpilation shares the same explicit CLI/output contract as source
// transpilation; a wrapper struct would not be reused outside this boundary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn transpile_js_module_to_output(
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

pub(crate) fn write_js_package_json(
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

pub(crate) fn run_build_js_file(
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

/// Stage all missing dependency.mjs files under.taida/deps/ so they can be
/// committed atomically with the main JS outputs.
pub(crate) fn stage_dep_js_outputs(
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

pub(crate) fn unique_stage_root(label: &str) -> PathBuf {
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

pub(crate) fn stage_output_path(stage_root: &Path, out_root: &Path, final_out: &Path) -> PathBuf {
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

pub(crate) fn commit_temp_path(final_path: &Path, commit_id: &str, idx: usize) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output.mjs");
    final_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{}.taida-stage-{}-{}", file_name, commit_id, idx))
}

pub(crate) fn commit_backup_path(final_path: &Path, commit_id: &str, idx: usize) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output.mjs");
    final_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{}.taida-backup-{}-{}", file_name, commit_id, idx))
}

pub(crate) fn commit_staged_js_outputs(staged_outputs: &[(PathBuf, PathBuf)], stage_root: &Path) {
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

pub(crate) fn run_build_js_dir(
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
// Build target entry points preserve the CLI surface shape explicitly; packing
// these one-off arguments would hide the target-specific diagnostic flow.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_build_native(
    input_path: &Path,
    output_path: Option<&str>,
    entry_path: Option<&str>,
    handler_symbol: Option<&str>,
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

    if !no_check || handler_symbol.is_some() {
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
        if let Some(handler) = handler_symbol
            && let Err(message) = validate_web_handler_entry(&program, handler)
        {
            emit_build_cli_diagnostic_and_exit(
                compile_stats,
                diag_format,
                "E1961",
                &message,
                Some(
                    "Use a one-argument handler: `handle req: WebRequest = text(\"ok\") => :WebResponse`.",
                ),
                1,
            );
        }
        if !no_check || handler_symbol.is_some() {
            run_type_checks_and_warnings(
                &program,
                &entry_file.to_string_lossy(),
                CompileTarget::Native,
                diag_format,
                compile_stats,
            );
        }
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
    let build_result = if let Some(handler) = handler_symbol {
        codegen::driver::compile_file_handler(&entry_file, output, handler)
    } else {
        codegen::driver::compile_file(&entry_file, output)
    };
    match build_result {
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
// Build target entry points preserve the CLI surface shape explicitly; packing
// these one-off arguments would hide the target-specific diagnostic flow.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_build_wasm_min(
    input_path: &Path,
    output_path: Option<&str>,
    handler_symbol: Option<&str>,
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

    if !no_check || handler_symbol.is_some() {
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
        if let Some(handler) = handler_symbol
            && let Err(message) = validate_web_handler_entry(&program, handler)
        {
            emit_build_cli_diagnostic_and_exit(
                compile_stats,
                diag_format,
                "E1961",
                &message,
                Some(
                    "Use a one-argument handler: `handle req: WebRequest = text(\"ok\") => :WebResponse`.",
                ),
                1,
            );
        }
        if !no_check || handler_symbol.is_some() {
            run_type_checks_and_warnings(
                &program,
                &input_path.to_string_lossy(),
                CompileTarget::WasmMin,
                diag_format,
                compile_stats,
            );
        }
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
    let build_result = if let Some(handler) = handler_symbol {
        codegen::driver::compile_file_wasm_handler_cached(input_path, output, rt_cache, handler)
    } else {
        codegen::driver::compile_file_wasm_cached(input_path, output, rt_cache)
    };
    match build_result {
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
// Build target entry points preserve the CLI surface shape explicitly; packing
// these one-off arguments would hide the target-specific diagnostic flow.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_build_wasm_wasi(
    input_path: &Path,
    output_path: Option<&str>,
    handler_symbol: Option<&str>,
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

    if !no_check || handler_symbol.is_some() {
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
        if let Some(handler) = handler_symbol
            && let Err(message) = validate_web_handler_entry(&program, handler)
        {
            emit_build_cli_diagnostic_and_exit(
                compile_stats,
                diag_format,
                "E1961",
                &message,
                Some(
                    "Use a one-argument handler: `handle req: WebRequest = text(\"ok\") => :WebResponse`.",
                ),
                1,
            );
        }
        if !no_check || handler_symbol.is_some() {
            run_type_checks_and_warnings(
                &program,
                &input_path.to_string_lossy(),
                CompileTarget::WasmWasi,
                diag_format,
                compile_stats,
            );
        }
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
    let build_result = if let Some(handler) = handler_symbol {
        codegen::driver::compile_file_wasm_wasi_handler_cached(
            input_path, output, rt_cache, handler,
        )
    } else {
        codegen::driver::compile_file_wasm_wasi_cached(input_path, output, rt_cache)
    };
    match build_result {
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

pub(crate) fn validate_web_handler_entry(program: &Program, handler: &str) -> Result<(), String> {
    let mut request_names: HashSet<String> = HashSet::from(["WebRequest".to_string()]);
    let mut response_names: HashSet<String> = HashSet::from(["WebResponse".to_string()]);
    let mut has_abi_import = false;

    for stmt in &program.statements {
        if let Statement::Import(import) = stmt
            && import.path == "taida-lang/abi"
        {
            has_abi_import = true;
            for sym in &import.symbols {
                let local = sym.alias.as_ref().unwrap_or(&sym.name).clone();
                match sym.name.as_str() {
                    "WebRequest" => {
                        request_names.insert(local);
                    }
                    "WebResponse" => {
                        response_names.insert(local);
                    }
                    _ => {}
                }
            }
        }
    }

    if !has_abi_import {
        return Err(
            "handler mode requires `>>> taida-lang/abi => @(WebRequest, WebResponse, ...)`."
                .to_string(),
        );
    }

    let Some(func) = program.statements.iter().find_map(|stmt| match stmt {
        Statement::FuncDef(fd) if fd.name == handler => Some(fd),
        _ => None,
    }) else {
        return Err(format!("Handler function '{}' was not found.", handler));
    };

    if func.params.len() != 1 {
        return Err(format!(
            "Handler '{}' must take exactly one WebRequest parameter.",
            handler
        ));
    }

    let param_ty = func.params[0]
        .type_annotation
        .as_ref()
        .and_then(named_type_expr);
    if !param_ty.is_some_and(|name| request_names.contains(name)) {
        return Err(format!(
            "Handler '{}' parameter must be annotated as WebRequest.",
            handler
        ));
    }

    let ret_ty = func.return_type.as_ref().and_then(named_type_expr);
    if !ret_ty.is_some_and(|name| response_names.contains(name)) {
        return Err(format!(
            "Handler '{}' return type must be annotated as WebResponse.",
            handler
        ));
    }

    Ok(())
}

pub(crate) fn named_type_expr(ty: &TypeExpr) -> Option<&str> {
    match ty {
        TypeExpr::Named(name) => Some(name.as_str()),
        _ => None,
    }
}

#[cfg(feature = "native")]
// Build target entry points preserve the CLI surface shape explicitly; packing
// these one-off arguments would hide the target-specific diagnostic flow.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_build_wasm_edge(
    input_path: &Path,
    output_path: Option<&str>,
    handler_symbol: Option<&str>,
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

    if !no_check || handler_symbol.is_some() {
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
        if let Some(handler) = handler_symbol
            && let Err(message) = validate_web_handler_entry(&program, handler)
        {
            emit_build_cli_diagnostic_and_exit(
                compile_stats,
                diag_format,
                "E1961",
                &message,
                Some(
                    "Use a one-argument handler: `handle req: WebRequest = text(\"ok\") => :WebResponse`.",
                ),
                1,
            );
        }
        if !no_check || handler_symbol.is_some() {
            let host_capability_manifest =
                match wasm_edge_host_capability_manifest_for_source(input_path) {
                    Ok(manifest) => manifest,
                    Err(message) => emit_build_cli_diagnostic_and_exit(
                        compile_stats,
                        diag_format,
                        "E3603",
                        &message,
                        Some("Fix the Cloudflare manifest before building wasm-edge output."),
                        1,
                    ),
                };
            run_type_checks_and_warnings_with_host_capability_manifest(
                &program,
                &input_path.to_string_lossy(),
                CompileTarget::WasmEdge,
                diag_format,
                compile_stats,
                Some(&host_capability_manifest),
            );
        }
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
    match codegen::driver::compile_file_wasm_edge_cached(
        input_path,
        output,
        rt_cache,
        handler_symbol,
    ) {
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
// Build target entry points preserve the CLI surface shape explicitly; packing
// these one-off arguments would hide the target-specific diagnostic flow.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_build_wasm_full(
    input_path: &Path,
    output_path: Option<&str>,
    handler_symbol: Option<&str>,
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

    if !no_check || handler_symbol.is_some() {
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
        if let Some(handler) = handler_symbol
            && let Err(message) = validate_web_handler_entry(&program, handler)
        {
            emit_build_cli_diagnostic_and_exit(
                compile_stats,
                diag_format,
                "E1961",
                &message,
                Some(
                    "Use a one-argument handler: `handle req: WebRequest = text(\"ok\") => :WebResponse`.",
                ),
                1,
            );
        }
        if !no_check || handler_symbol.is_some() {
            run_type_checks_and_warnings(
                &program,
                &input_path.to_string_lossy(),
                CompileTarget::WasmFull,
                diag_format,
                compile_stats,
            );
        }
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
    let build_result = if let Some(handler) = handler_symbol {
        codegen::driver::compile_file_wasm_full_handler_cached(
            input_path, output, rt_cache, handler,
        )
    } else {
        codegen::driver::compile_file_wasm_full_cached(input_path, output, rt_cache)
    };
    match build_result {
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
pub(crate) fn resolve_native_entry_path(
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

pub(crate) fn run_type_checks_and_warnings(
    program: &Program,
    file: &str,
    compile_target: CompileTarget,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
) {
    run_type_checks_and_warnings_with_host_capability_manifest(
        program,
        file,
        compile_target,
        diag_format,
        compile_stats,
        None,
    );
}

pub(crate) fn run_type_checks_and_warnings_with_host_capability_manifest(
    program: &Program,
    file: &str,
    compile_target: CompileTarget,
    diag_format: DiagFormat,
    compile_stats: &mut CompileDiagStats,
    host_capability_manifest: Option<&[(String, String)]>,
) {
    let mut checker = TypeChecker::new();
    checker.set_compile_target(compile_target);
    if let Some(manifest) = host_capability_manifest {
        checker.set_host_capability_manifest(
            manifest
                .iter()
                .map(|(name, kind)| (name.as_str(), kind.as_str())),
        );
    }
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

pub(crate) fn find_wrangler_manifest_for_source(source_path: &Path) -> Option<PathBuf> {
    let mut dir = if source_path.is_dir() {
        source_path.to_path_buf()
    } else {
        source_path.parent()?.to_path_buf()
    };

    loop {
        for name in ["wrangler.jsonc", "wrangler.json"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        if dir.join("packages.tdm").exists()
            || dir.join("taida.toml").exists()
            || dir.join(".git").exists()
        {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub(crate) fn strip_jsonc_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut escape = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                        }
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                    continue;
                }
                _ => {}
            }
        }

        out.push(ch);
    }

    out
}

pub(crate) fn remove_json_trailing_commas(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escape = false;

    for (idx, ch) in chars.iter().copied().enumerate() {
        if in_string {
            out.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }

        if ch == ',' {
            let next = chars
                .iter()
                .skip(idx + 1)
                .find(|candidate| !candidate.is_whitespace())
                .copied();
            if matches!(next, Some(']') | Some('}')) {
                continue;
            }
        }

        out.push(ch);
    }

    out
}

pub(crate) fn strip_jsonc_to_json(source: &str) -> String {
    remove_json_trailing_commas(&strip_jsonc_comments(source))
}

pub(crate) fn wrangler_array_at<'a>(
    root: &'a serde_json::Value,
    path: &[&str],
) -> Result<Option<&'a Vec<serde_json::Value>>, String> {
    let mut current = root;
    for key in path {
        let Some(next) = current.get(*key) else {
            return Ok(None);
        };
        current = next;
    }
    current.as_array().map(Some).ok_or_else(|| {
        format!(
            "wrangler manifest field `{}` must be an array.",
            path.join(".")
        )
    })
}

pub(crate) fn push_wrangler_binding_capabilities(
    capabilities: &mut Vec<(String, String)>,
    seen: &mut HashSet<(String, String)>,
    entries: Option<&Vec<serde_json::Value>>,
    binding_keys: &[&str],
    kind: &str,
) {
    let Some(entries) = entries else {
        return;
    };
    for entry in entries {
        let Some(name) = binding_keys
            .iter()
            .filter_map(|key| entry.get(*key).and_then(serde_json::Value::as_str))
            .find(|value| !value.is_empty())
        else {
            continue;
        };
        let pair = (name.to_string(), kind.to_string());
        if seen.insert(pair.clone()) {
            capabilities.push(pair);
        }
    }
}

pub(crate) fn parse_wrangler_host_capability_manifest_str(
    source: &str,
) -> Result<Vec<(String, String)>, String> {
    let json_source = strip_jsonc_to_json(source);
    let root: serde_json::Value = serde_json::from_str(&json_source)
        .map_err(|err| format!("wrangler manifest is not valid JSONC: {}", err))?;
    if !root.is_object() {
        return Err("wrangler manifest root must be a JSON object.".to_string());
    }

    let mut capabilities = Vec::new();
    let mut seen = HashSet::new();
    push_wrangler_binding_capabilities(
        &mut capabilities,
        &mut seen,
        wrangler_array_at(&root, &["d1_databases"])?,
        &["binding"],
        "cloudflare/d1",
    );
    push_wrangler_binding_capabilities(
        &mut capabilities,
        &mut seen,
        wrangler_array_at(&root, &["kv_namespaces"])?,
        &["binding"],
        "cloudflare/kv",
    );
    push_wrangler_binding_capabilities(
        &mut capabilities,
        &mut seen,
        wrangler_array_at(&root, &["durable_objects", "bindings"])?,
        &["name", "binding"],
        "cloudflare/do_namespace",
    );
    push_wrangler_binding_capabilities(
        &mut capabilities,
        &mut seen,
        wrangler_array_at(&root, &["r2_buckets"])?,
        &["binding"],
        "cloudflare/r2",
    );
    push_wrangler_binding_capabilities(
        &mut capabilities,
        &mut seen,
        wrangler_array_at(&root, &["queues", "producers"])?,
        &["binding"],
        "cloudflare/queue_producer",
    );
    push_wrangler_binding_capabilities(
        &mut capabilities,
        &mut seen,
        wrangler_array_at(&root, &["services"])?,
        &["binding"],
        "cloudflare/fetcher",
    );

    Ok(capabilities)
}

pub(crate) fn load_wrangler_host_capability_manifest(
    path: &Path,
) -> Result<Vec<(String, String)>, String> {
    let source = fs::read_to_string(path).map_err(|err| {
        format!(
            "Error reading wrangler manifest '{}': {}",
            path.display(),
            err
        )
    })?;
    parse_wrangler_host_capability_manifest_str(&source).map_err(|err| {
        format!(
            "Error parsing wrangler manifest '{}': {}",
            path.display(),
            err
        )
    })
}

pub(crate) fn wasm_edge_host_capability_manifest_for_source(
    source_path: &Path,
) -> Result<Vec<(String, String)>, String> {
    match find_wrangler_manifest_for_source(source_path) {
        Some(path) => load_wrangler_host_capability_manifest(&path),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn severity_to_kind(severity: &str) -> &'static str {
    match severity {
        "ERROR" => "error",
        "WARNING" => "warning",
        "INFO" => "info",
        _ => "info",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildTarget {
    Js,
    Native,
    WasmMin,
    WasmWasi,
    WasmEdge,
    WasmFull,
}

impl BuildTarget {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
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

    pub(crate) fn as_str(self) -> &'static str {
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
    pub(crate) fn is_wasm(self) -> bool {
        matches!(
            self,
            Self::WasmMin | Self::WasmWasi | Self::WasmEdge | Self::WasmFull
        )
    }

    pub(crate) fn supports_handler(self) -> bool {
        matches!(
            self,
            Self::Native | Self::WasmMin | Self::WasmWasi | Self::WasmEdge | Self::WasmFull
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagFormat {
    Text,
    Jsonl,
}

impl DiagFormat {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "text" => Some(Self::Text),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct CompileDiagStats {
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
    pub(crate) info: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BuildDiagContext {
    pub(crate) unit: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) edge_kind: Option<&'static str>,
    pub(crate) dependency_path: Vec<String>,
    pub(crate) transaction_id: Option<String>,
    pub(crate) hook_name: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) exit_code: Option<i32>,
}

#[derive(Clone)]
pub(crate) struct StagedJsCommit {
    pub(crate) final_path: PathBuf,
    pub(crate) temp_path: PathBuf,
    pub(crate) backup_path: Option<PathBuf>,
}
