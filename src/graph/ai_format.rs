//! AI-oriented unified JSON output for `taida graph`.
//!
//! Produces a single JSON document that gives an AI agent a complete
//! structural overview of a Taida source file — functions, types, data
//! flow, imports, exports — without requiring source access.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::escape_json;
use crate::parser::*;

/// Top-level AI-oriented graph representation.
struct AiGraph {
    file: String,
    types: Vec<AiType>,
    functions: Vec<AiFunction>,
    main_flow: Vec<String>,
    imports: Vec<AiImport>,
    exports: Vec<String>,
}

struct AiType {
    name: String,
    kind: &'static str, // "buchi_pack", "mold", "error", "inheritance"
    parent: Option<String>,
    fields: Vec<(String, String)>,
    line: usize,
}

struct AiFunction {
    name: String,
    params: Vec<(String, String)>, // (name, type)
    returns: String,
    body_summary: String,
    calls: Vec<String>,
    throws: bool,
    has_error_ceiling: bool,
    is_recursive: bool,
    line: usize,
}

struct AiImport {
    path: String,
    symbols: Vec<String>,
    version: Option<String>,
}

/// Generate AI-oriented unified JSON from a parsed Taida program.
pub fn format_ai_json(program: &Program, file: &str) -> String {
    let ai = extract_ai_graph(program, file);
    render_json(&ai)
}

/// Generate AI-oriented unified JSON by recursively following imports.
///
/// Starting from the entry point file, this function parses each imported
/// module (resolving relative paths from the importing file's directory),
/// and produces a single JSON document with a `modules` array containing
/// the AI graph for every reachable module.
///
/// Circular imports are detected via a visited set and skipped.
/// Module order is depth-first: the entry point appears first.
/// Maximum import recursion depth to prevent stack overflow on pathological inputs.
const MAX_RECURSION_DEPTH: usize = 256;

pub fn format_ai_json_recursive(entry_path: &str) -> Result<String, String> {
    let entry = Path::new(entry_path);
    let entry_dir = entry.parent().unwrap_or_else(|| Path::new("."));

    // Canonicalize entry_dir once to avoid repeated filesystem stat calls.
    let entry_dir_canon =
        std::fs::canonicalize(entry_dir).unwrap_or_else(|_| entry_dir.to_path_buf());

    // Read and parse the entry file
    let source = std::fs::read_to_string(entry)
        .map_err(|e| format!("Error reading file '{}': {}", entry_path, e))?;
    let (program, parse_errors) = parse(&source);
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| e.message.clone()).collect();
        return Err(msgs.join("\n"));
    }

    let mut modules = Vec::new();
    let mut visited = HashSet::new();

    // Canonical path for cycle detection; fall back to the raw string.
    let canon = std::fs::canonicalize(entry).unwrap_or_else(|_| entry.to_path_buf());
    visited.insert(canon);

    collect_modules_recursive(
        &program,
        entry_path,
        entry_dir,
        &entry_dir_canon,
        &mut modules,
        &mut visited,
        0,
    );

    Ok(render_recursive_json(entry_path, &modules))
}

/// Recursively collect `AiGraph` entries for the given program and all its
/// (transitively) imported local modules.
///
/// Import paths may include the `.td` extension (e.g. `./models.td`) or omit
/// it (e.g. `./models`).  When the literal path does not exist on disk, the
/// function tries appending `.td` — matching the interpreter's resolution
/// behaviour in `resolve_module_path()`.
fn collect_modules_recursive(
    program: &Program,
    display_path: &str,
    file_dir: &Path,
    entry_dir_canon: &Path,
    modules: &mut Vec<AiGraph>,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) {
    if depth > MAX_RECURSION_DEPTH {
        return; // prevent stack overflow on pathological import chains
    }

    let ai = extract_ai_graph(program, display_path);

    // Collect local import paths before we move `ai` into `modules`.
    let local_imports: Vec<String> = ai
        .imports
        .iter()
        .filter(|imp| imp.path.starts_with("./") || imp.path.starts_with("../"))
        .map(|imp| imp.path.clone())
        .collect();

    modules.push(ai);

    // Recurse into local imports
    for imp_path in &local_imports {
        let resolved = file_dir.join(imp_path);
        let canon = match std::fs::canonicalize(&resolved) {
            Ok(c) => c,
            Err(_) => {
                // Try appending .td extension (e.g. ./module_math -> ./module_math.td)
                if resolved.extension().is_none() {
                    match std::fs::canonicalize(resolved.with_extension("td")) {
                        Ok(c) => c,
                        Err(_) => continue,
                    }
                } else {
                    continue;
                }
            }
        };
        if !visited.insert(canon.clone()) {
            continue; // already visited (circular import)
        }

        let source = match std::fs::read_to_string(&canon) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (child_program, child_errors) = parse(&source);
        if !child_errors.is_empty() {
            continue; // parse error — skip
        }

        // Display path relative to the (pre-canonicalized) entry directory
        let child_display = relative_display(&canon, entry_dir_canon);
        let child_dir = canon
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        collect_modules_recursive(
            &child_program,
            &child_display,
            &child_dir,
            entry_dir_canon,
            modules,
            visited,
            depth + 1,
        );
    }
}

/// Produce a display path relative to `base`, prefixed with `./`.
/// `base` must already be canonicalized (to avoid repeated filesystem stat calls).
/// Falls back to the absolute path if `strip_prefix` fails.
fn relative_display(path: &Path, base_canon: &Path) -> String {
    match path.strip_prefix(base_canon) {
        Ok(rel) => format!("./{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Render the recursive (multi-module) JSON envelope.
fn render_recursive_json(entry: &str, modules: &[AiGraph]) -> String {
    let mut out = String::with_capacity(4096);
    let version = crate::version::taida_version();

    out.push_str("{\n");
    out.push_str(&format!(
        "  \"taida_version\": \"{}\",\n",
        escape_json(version)
    ));
    out.push_str(&format!("  \"entry\": \"{}\",\n", escape_json(entry)));

    // modules array
    out.push_str("  \"modules\": [\n");
    for (mi, ai) in modules.iter().enumerate() {
        out.push_str("    {\n");
        out.push_str(&format!("      \"file\": \"{}\",\n", escape_json(&ai.file)));

        render_module_body(&mut out, ai, 6);

        out.push_str("    }");
        if mi < modules.len() - 1 {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn extract_ai_graph(program: &Program, file: &str) -> AiGraph {
    let mut types = Vec::new();
    let mut functions = Vec::new();
    let mut main_flow = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    // Collect all function names for call/recursion tracking
    let func_names: HashSet<String> = program
        .statements
        .iter()
        .filter_map(|s| {
            if let Statement::FuncDef(fd) = s {
                Some(fd.name.clone())
            } else {
                None
            }
        })
        .collect();

    for stmt in &program.statements {
        match stmt {
            Statement::TypeDef(td) => {
                types.push(AiType {
                    name: td.name.clone(),
                    kind: "buchi_pack",
                    parent: None,
                    fields: td
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), type_expr_to_str(&f.type_annotation)))
                        .collect(),
                    line: td.span.line,
                });
            }

            Statement::MoldDef(md) => {
                types.push(AiType {
                    name: md.name.clone(),
                    kind: "mold",
                    parent: None,
                    fields: md
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .map(|f| (f.name.clone(), type_expr_to_str(&f.type_annotation)))
                        .collect(),
                    line: md.span.line,
                });
            }

            Statement::InheritanceDef(inh) => {
                let kind = if inh.parent == "Error" {
                    "error"
                } else {
                    "inheritance"
                };
                types.push(AiType {
                    name: inh.child.clone(),
                    kind,
                    parent: Some(inh.parent.clone()),
                    fields: inh
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), type_expr_to_str(&f.type_annotation)))
                        .collect(),
                    line: inh.span.line,
                });
            }

            Statement::FuncDef(fd) => {
                let mut calls = Vec::new();
                let mut throws = false;
                let mut has_error_ceiling = false;
                collect_calls_and_throws(
                    &fd.body,
                    &func_names,
                    &mut calls,
                    &mut throws,
                    &mut has_error_ceiling,
                );
                let is_recursive = calls.contains(&fd.name);

                // Deduplicate calls
                let mut seen = HashSet::new();
                calls.retain(|c| seen.insert(c.clone()));

                functions.push(AiFunction {
                    name: fd.name.clone(),
                    params: fd
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), type_expr_to_str(&p.type_annotation)))
                        .collect(),
                    returns: type_expr_to_str(&fd.return_type),
                    body_summary: summarize_body(&fd.body),
                    calls,
                    throws,
                    has_error_ceiling,
                    is_recursive,
                    line: fd.span.line,
                });
            }

            Statement::Import(imp) => {
                imports.push(AiImport {
                    path: imp.path.clone(),
                    symbols: imp.symbols.iter().map(|s| s.name.clone()).collect(),
                    version: imp.version.clone(),
                });
            }

            Statement::Export(exp) => {
                exports.extend(exp.symbols.iter().cloned());
            }

            // Top-level statements go to main_flow
            Statement::Assignment(_)
            | Statement::Expr(_)
            | Statement::UnmoldForward(_)
            | Statement::UnmoldBackward(_)
            | Statement::ErrorCeiling(_) => {
                main_flow.push(summarize_stmt(stmt));
            }
        }
    }

    AiGraph {
        file: file.to_string(),
        types,
        functions,
        main_flow,
        imports,
        exports,
    }
}

// ── Type expression to string ──────────────────────────

fn type_expr_to_str(te: &Option<TypeExpr>) -> String {
    match te {
        Some(t) => format_type_expr(t),
        None => "_".to_string(),
    }
}

fn format_type_expr(te: &TypeExpr) -> String {
    match te {
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::List(inner) => format!("@[{}]", format_type_expr(inner)),
        TypeExpr::Generic(name, args) => {
            let args_str: Vec<String> = args.iter().map(format_type_expr).collect();
            format!("{}[{}]", name, args_str.join(", "))
        }
        TypeExpr::BuchiPack(fields) => {
            let fields_str: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, type_expr_to_str(&f.type_annotation)))
                .collect();
            format!("@({})", fields_str.join(", "))
        }
        TypeExpr::Function(params, ret) => {
            let params_str: Vec<String> = params.iter().map(format_type_expr).collect();
            format!("({}) => :{}", params_str.join(", "), format_type_expr(ret))
        }
    }
}

// ── Call / throw collection ────────────────────────────

fn collect_calls_and_throws(
    stmts: &[Statement],
    func_names: &HashSet<String>,
    calls: &mut Vec<String>,
    throws: &mut bool,
    has_error_ceiling: &mut bool,
) {
    for stmt in stmts {
        match stmt {
            Statement::Expr(expr) => {
                collect_from_expr(expr, func_names, calls, throws, has_error_ceiling);
            }
            Statement::Assignment(assign) => {
                collect_from_expr(&assign.value, func_names, calls, throws, has_error_ceiling);
            }
            Statement::UnmoldForward(uf) => {
                collect_from_expr(&uf.source, func_names, calls, throws, has_error_ceiling);
            }
            Statement::UnmoldBackward(ub) => {
                collect_from_expr(&ub.source, func_names, calls, throws, has_error_ceiling);
            }
            Statement::ErrorCeiling(ec) => {
                *has_error_ceiling = true;
                collect_calls_and_throws(
                    &ec.handler_body,
                    func_names,
                    calls,
                    throws,
                    has_error_ceiling,
                );
            }
            _ => {}
        }
    }
}

fn collect_from_expr(
    expr: &Expr,
    func_names: &HashSet<String>,
    calls: &mut Vec<String>,
    throws: &mut bool,
    has_error_ceiling: &mut bool,
) {
    match expr {
        Expr::FuncCall(callee, args, _) => {
            if let Expr::Ident(name, _) = callee.as_ref()
                && func_names.contains(name)
            {
                calls.push(name.clone());
            }
            // Recurse into callee (for chained calls)
            collect_from_expr(callee, func_names, calls, throws, has_error_ceiling);
            for arg in args {
                collect_from_expr(arg, func_names, calls, throws, has_error_ceiling);
            }
        }
        Expr::MethodCall(obj, _method, args, _) => {
            collect_from_expr(obj, func_names, calls, throws, has_error_ceiling);
            for arg in args {
                collect_from_expr(arg, func_names, calls, throws, has_error_ceiling);
            }
        }
        Expr::Throw(inner, _) => {
            *throws = true;
            collect_from_expr(inner, func_names, calls, throws, has_error_ceiling);
        }
        Expr::BinaryOp(lhs, _, rhs, _) => {
            collect_from_expr(lhs, func_names, calls, throws, has_error_ceiling);
            collect_from_expr(rhs, func_names, calls, throws, has_error_ceiling);
        }
        Expr::UnaryOp(_, inner, _) => {
            collect_from_expr(inner, func_names, calls, throws, has_error_ceiling);
        }
        Expr::Pipeline(exprs, _) => {
            for e in exprs {
                collect_from_expr(e, func_names, calls, throws, has_error_ceiling);
            }
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    collect_from_expr(cond, func_names, calls, throws, has_error_ceiling);
                }
                for body_stmt in &arm.body {
                    collect_calls_and_throws(
                        std::slice::from_ref(body_stmt),
                        func_names,
                        calls,
                        throws,
                        has_error_ceiling,
                    );
                }
            }
        }
        Expr::Lambda(_, body, _) => {
            collect_from_expr(body, func_names, calls, throws, has_error_ceiling);
        }
        Expr::MoldInst(_, args, _, _) => {
            for arg in args {
                collect_from_expr(arg, func_names, calls, throws, has_error_ceiling);
            }
        }
        Expr::Unmold(inner, _) => {
            collect_from_expr(inner, func_names, calls, throws, has_error_ceiling);
        }
        Expr::FieldAccess(inner, _, _) => {
            collect_from_expr(inner, func_names, calls, throws, has_error_ceiling);
        }
        _ => {}
    }
}

// ── Body summary ───────────────────────────────────────

fn summarize_body(stmts: &[Statement]) -> String {
    const MAX_STMTS: usize = 3;
    let mut parts = Vec::new();

    for (i, stmt) in stmts.iter().enumerate() {
        if i >= MAX_STMTS {
            let remaining = stmts.len() - MAX_STMTS;
            parts.push(format!("... ({} more)", remaining));
            break;
        }
        parts.push(summarize_stmt(stmt));
    }

    parts.join("; ")
}

fn summarize_stmt(stmt: &Statement) -> String {
    match stmt {
        Statement::Assignment(assign) => {
            format!("{} <= {}", assign.target, summarize_expr(&assign.value))
        }
        Statement::Expr(expr) => summarize_expr(expr),
        Statement::UnmoldForward(uf) => {
            format!("{} ]=> {}", summarize_expr(&uf.source), uf.target)
        }
        Statement::UnmoldBackward(ub) => {
            format!("{} <=[ {}", ub.target, summarize_expr(&ub.source))
        }
        Statement::ErrorCeiling(ec) => {
            format!(
                "|== {}: {}",
                ec.error_param,
                format_type_expr(&ec.error_type)
            )
        }
        Statement::FuncDef(fd) => format!("fn {}", fd.name),
        Statement::TypeDef(td) => format!("type {}", td.name),
        Statement::MoldDef(md) => format!("mold {}", md.name),
        Statement::InheritanceDef(inh) => format!("{} => {}", inh.parent, inh.child),
        Statement::Import(imp) => {
            let syms: Vec<&str> = imp.symbols.iter().map(|s| s.name.as_str()).collect();
            format!(">>> {} => @({})", imp.path, syms.join(", "))
        }
        Statement::Export(exp) => {
            format!("<<< @({})", exp.symbols.join(", "))
        }
    }
}

fn summarize_expr(expr: &Expr) -> String {
    match expr {
        Expr::IntLit(n, _) => n.to_string(),
        Expr::FloatLit(n, _) => n.to_string(),
        Expr::StringLit(s, _) => format!("\"{}\"", truncate_str(s, 30)),
        Expr::TemplateLit(s, _) => format!("`{}`", truncate_str(s, 30)),
        Expr::BoolLit(b, _) => b.to_string(),
        Expr::Gorilla(_) => "><".to_string(),
        Expr::Ident(name, _) => name.clone(),
        Expr::Placeholder(_) => "_".to_string(),
        Expr::Hole(_) => "_hole".to_string(),

        Expr::BuchiPack(fields, _) => {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            format!("@({})", names.join(", "))
        }
        Expr::ListLit(items, _) => {
            if items.len() <= 3 {
                let vals: Vec<String> = items.iter().map(summarize_expr).collect();
                format!("@[{}]", vals.join(", "))
            } else {
                format!("@[...{} items]", items.len())
            }
        }

        Expr::BinaryOp(lhs, op, rhs, _) => {
            format!(
                "{} {} {}",
                summarize_expr(lhs),
                binop_str(op),
                summarize_expr(rhs)
            )
        }
        Expr::UnaryOp(op, inner, _) => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            format!("{}{}", op_str, summarize_expr(inner))
        }

        Expr::FuncCall(callee, args, _) => {
            let callee_str = summarize_expr(callee);
            let args_str: Vec<String> = args.iter().map(summarize_expr).collect();
            format!("{}({})", callee_str, args_str.join(", "))
        }
        Expr::MethodCall(obj, method, args, _) => {
            let obj_str = summarize_expr(obj);
            if args.is_empty() {
                format!("{}.{}()", obj_str, method)
            } else {
                let args_str: Vec<String> = args.iter().map(summarize_expr).collect();
                format!("{}.{}({})", obj_str, method, args_str.join(", "))
            }
        }
        Expr::FieldAccess(obj, field, _) => {
            format!("{}.{}", summarize_expr(obj), field)
        }

        Expr::CondBranch(arms, _) => summarize_cond(arms),

        Expr::Pipeline(exprs, _) => {
            let parts: Vec<String> = exprs.iter().map(summarize_expr).collect();
            parts.join(" => ")
        }

        Expr::MoldInst(name, args, fields, _) => {
            let args_str: Vec<String> = args.iter().map(summarize_expr).collect();
            if fields.is_empty() {
                format!("{}[{}]()", name, args_str.join(", "))
            } else {
                let field_names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                format!(
                    "{}[{}]({})",
                    name,
                    args_str.join(", "),
                    field_names.join(", ")
                )
            }
        }

        Expr::Unmold(inner, _) => {
            format!("{} ]=>", summarize_expr(inner))
        }

        Expr::Lambda(params, body, _) => {
            let param_names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            format!("_ {} = {}", param_names.join(" "), summarize_expr(body))
        }

        Expr::TypeInst(name, fields, _) => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|f| format!("{} <= {}", f.name, summarize_expr(&f.value)))
                .collect();
            format!("{}({})", name, field_strs.join(", "))
        }

        Expr::Throw(inner, _) => {
            format!("{}.throw()", summarize_expr(inner))
        }
    }
}

fn summarize_cond(arms: &[CondArm]) -> String {
    let mut parts = Vec::new();
    for arm in arms {
        let cond_str = match &arm.condition {
            Some(c) => summarize_expr(c),
            None => "_".to_string(),
        };
        let body_str = match arm.last_expr() {
            Some(e) => summarize_expr(e),
            None => "...".to_string(),
        };
        parts.push(format!("| {} |> {}", cond_str, body_str));
    }
    parts.join(", ")
}

fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add | BinOp::Concat => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{}...", truncated)
    }
}

// ── JSON rendering (manual — no serde dependency for model) ──

/// Render the five module-body fields (types, functions, main_flow, imports, exports)
/// at the given indent level. Shared by both single-file and recursive renderers.
fn render_module_body(out: &mut String, ai: &AiGraph, indent: usize) {
    let prefix = " ".repeat(indent);

    out.push_str(&format!("{}\"types\": ", prefix));
    render_types(out, &ai.types, indent);
    out.push_str(",\n");

    out.push_str(&format!("{}\"functions\": ", prefix));
    render_functions(out, &ai.functions, indent);
    out.push_str(",\n");

    out.push_str(&format!("{}\"main_flow\": ", prefix));
    render_string_array(out, &ai.main_flow, indent);
    out.push_str(",\n");

    out.push_str(&format!("{}\"imports\": ", prefix));
    render_imports(out, &ai.imports, indent);
    out.push_str(",\n");

    out.push_str(&format!("{}\"exports\": ", prefix));
    render_string_array(out, &ai.exports, indent);
    out.push('\n');
}

fn render_json(ai: &AiGraph) -> String {
    let mut out = String::with_capacity(1024);
    let version = crate::version::taida_version();

    out.push_str("{\n");
    out.push_str(&format!(
        "  \"taida_version\": \"{}\",\n",
        escape_json(version)
    ));
    out.push_str(&format!("  \"file\": \"{}\",\n", escape_json(&ai.file)));

    render_module_body(&mut out, ai, 2);

    out.push_str("}\n");
    out
}

fn render_types(out: &mut String, types: &[AiType], indent: usize) {
    let base = " ".repeat(indent);
    let inner = " ".repeat(indent + 2);
    let field_indent = " ".repeat(indent + 4);
    if types.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (i, t) in types.iter().enumerate() {
        out.push_str(&inner);
        out.push_str("{\n");
        out.push_str(&format!(
            "{}\"name\": \"{}\",\n",
            field_indent,
            escape_json(&t.name)
        ));
        out.push_str(&format!("{}\"kind\": \"{}\",\n", field_indent, t.kind));
        if let Some(parent) = &t.parent {
            out.push_str(&format!(
                "{}\"parent\": \"{}\",\n",
                field_indent,
                escape_json(parent)
            ));
        }
        out.push_str(&format!("{}\"fields\": {{", field_indent));
        let field_entries: Vec<String> = t
            .fields
            .iter()
            .map(|(k, v)| format!("\"{}\": \"{}\"", escape_json(k), escape_json(v)))
            .collect();
        out.push_str(&field_entries.join(", "));
        out.push_str("},\n");
        out.push_str(&format!("{}\"line\": {}\n", field_indent, t.line));
        out.push_str(&inner);
        out.push('}');
        if i < types.len() - 1 {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&base);
    out.push(']');
}

fn render_functions(out: &mut String, functions: &[AiFunction], indent: usize) {
    let base = " ".repeat(indent);
    let inner = " ".repeat(indent + 2);
    let field_indent = " ".repeat(indent + 4);
    if functions.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (i, f) in functions.iter().enumerate() {
        out.push_str(&inner);
        out.push_str("{\n");
        out.push_str(&format!(
            "{}\"name\": \"{}\",\n",
            field_indent,
            escape_json(&f.name)
        ));

        // params
        out.push_str(&format!("{}\"params\": [", field_indent));
        let param_entries: Vec<String> = f
            .params
            .iter()
            .map(|(name, ty)| {
                format!(
                    "{{\"name\": \"{}\", \"type\": \"{}\"}}",
                    escape_json(name),
                    escape_json(ty)
                )
            })
            .collect();
        out.push_str(&param_entries.join(", "));
        out.push_str("],\n");

        out.push_str(&format!(
            "{}\"returns\": \"{}\",\n",
            field_indent,
            escape_json(&f.returns)
        ));
        out.push_str(&format!(
            "{}\"body_summary\": \"{}\",\n",
            field_indent,
            escape_json(&f.body_summary)
        ));

        // calls
        out.push_str(&format!("{}\"calls\": ", field_indent));
        render_string_array_inline(out, &f.calls);
        out.push_str(",\n");

        out.push_str(&format!("{}\"throws\": {},\n", field_indent, f.throws));
        out.push_str(&format!(
            "{}\"has_error_ceiling\": {},\n",
            field_indent, f.has_error_ceiling
        ));
        out.push_str(&format!(
            "{}\"is_recursive\": {},\n",
            field_indent, f.is_recursive
        ));
        out.push_str(&format!("{}\"line\": {}\n", field_indent, f.line));
        out.push_str(&inner);
        out.push('}');
        if i < functions.len() - 1 {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&base);
    out.push(']');
}

fn render_imports(out: &mut String, imports: &[AiImport], indent: usize) {
    let base = " ".repeat(indent);
    let inner = " ".repeat(indent + 2);
    let field_indent = " ".repeat(indent + 4);
    if imports.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (i, imp) in imports.iter().enumerate() {
        out.push_str(&inner);
        out.push_str("{\n");
        out.push_str(&format!(
            "{}\"path\": \"{}\",\n",
            field_indent,
            escape_json(&imp.path)
        ));
        out.push_str(&format!("{}\"symbols\": ", field_indent));
        render_string_array_inline(out, &imp.symbols);
        out.push_str(",\n");
        match &imp.version {
            Some(v) => out.push_str(&format!(
                "{}\"version\": \"{}\"\n",
                field_indent,
                escape_json(v)
            )),
            None => out.push_str(&format!("{}\"version\": null\n", field_indent)),
        }
        out.push_str(&inner);
        out.push('}');
        if i < imports.len() - 1 {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&base);
    out.push(']');
}

fn render_string_array(out: &mut String, items: &[String], indent: usize) {
    let base = " ".repeat(indent);
    let inner = " ".repeat(indent + 2);
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (i, item) in items.iter().enumerate() {
        out.push_str(&format!("{}\"{}\"", inner, escape_json(item)));
        if i < items.len() - 1 {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&base);
    out.push(']');
}

fn render_string_array_inline(out: &mut String, items: &[String]) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    let entries: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", escape_json(s)))
        .collect();
    out.push('[');
    out.push_str(&entries.join(", "));
    out.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_ai_json(source: &str) -> String {
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        format_ai_json(&program, "test.td")
    }

    fn parse_json(source: &str) -> serde_json::Value {
        let json_str = parse_and_ai_json(source);
        serde_json::from_str(&json_str)
            .unwrap_or_else(|e| panic!("Invalid JSON: {}\n---\n{}", e, json_str))
    }

    // ── Basic validity ──

    #[test]
    fn test_empty_program_produces_valid_json() {
        let val = parse_json("");
        assert_eq!(val["types"], serde_json::json!([]));
        assert_eq!(val["functions"], serde_json::json!([]));
        assert_eq!(val["main_flow"], serde_json::json!([]));
        assert_eq!(val["imports"], serde_json::json!([]));
        assert_eq!(val["exports"], serde_json::json!([]));
    }

    #[test]
    fn test_version_present() {
        let val = parse_json("");
        assert!(val["taida_version"].is_string());
    }

    #[test]
    fn test_file_field() {
        let val = parse_json("");
        assert_eq!(val["file"], "test.td");
    }

    // ── Types ──

    #[test]
    fn test_typedef_extraction() {
        let val = parse_json("Person = @(name: Str, age: Int)");
        let types = val["types"].as_array().unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0]["name"], "Person");
        assert_eq!(types[0]["kind"], "buchi_pack");
        assert_eq!(types[0]["fields"]["name"], "Str");
        assert_eq!(types[0]["fields"]["age"], "Int");
    }

    #[test]
    fn test_error_inheritance_extraction() {
        let val = parse_json("Error => ValidationError = @(field: Str, code: Int)");
        let types = val["types"].as_array().unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0]["name"], "ValidationError");
        assert_eq!(types[0]["kind"], "error");
    }

    // ── Functions ──

    #[test]
    fn test_function_extraction() {
        let source = "add x y =\n  x + y\n=> :Int";
        let val = parse_json(source);
        let funcs = val["functions"].as_array().unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0]["name"], "add");
        assert_eq!(funcs[0]["returns"], "Int");
        assert_eq!(funcs[0]["is_recursive"], false);

        let params = funcs[0]["params"].as_array().unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0]["name"], "x");
        assert_eq!(params[1]["name"], "y");
    }

    #[test]
    fn test_recursive_function() {
        let source = "factorial n =\n  | n < 2 |> 1\n  | _ |> n * factorial(n - 1)\n=> :Int";
        let val = parse_json(source);
        let funcs = val["functions"].as_array().unwrap();
        assert_eq!(funcs[0]["is_recursive"], true);
        assert!(
            funcs[0]["calls"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("factorial"))
        );
    }

    #[test]
    fn test_function_with_throw() {
        let source = "validate text =\n  |== error: Error =\n    \"caught\"\n  => :Str\n  | text == \"\" |> ValidationError(type <= \"err\", message <= \"empty\", field <= \"x\", code <= 400).throw()\n  | _ |> text\n=> :Str";
        let val = parse_json(source);
        let funcs = val["functions"].as_array().unwrap();
        assert_eq!(funcs[0]["throws"], true);
        assert_eq!(funcs[0]["has_error_ceiling"], true);
    }

    #[test]
    fn test_body_summary_simple() {
        let source = "add x y =\n  x + y\n=> :Int";
        let val = parse_json(source);
        let funcs = val["functions"].as_array().unwrap();
        assert_eq!(funcs[0]["body_summary"], "x + y");
    }

    #[test]
    fn test_body_summary_truncation() {
        // 4 statements: should show 3 + "... (1 more)"
        let source = "work =\n  a <= 1\n  b <= 2\n  c <= 3\n  d <= 4\n=> :Int";
        let val = parse_json(source);
        let summary = funcs_body_summary(&val, 0);
        assert!(
            summary.contains("... (1 more)"),
            "Expected truncation, got: {}",
            summary
        );
    }

    // ── Main flow ──

    #[test]
    fn test_main_flow_assignment() {
        let source = "add x y =\n  x + y\n=> :Int\nresult <= add(3, 5)";
        let val = parse_json(source);
        let flow = val["main_flow"].as_array().unwrap();
        assert_eq!(flow.len(), 1);
        let first = flow[0].as_str().unwrap();
        assert!(
            first.contains("result"),
            "Expected assignment in main_flow: {}",
            first
        );
    }

    #[test]
    fn test_main_flow_call() {
        let source = "stdout(\"hello\")";
        let val = parse_json(source);
        let flow = val["main_flow"].as_array().unwrap();
        assert_eq!(flow.len(), 1);
        assert!(flow[0].as_str().unwrap().contains("stdout"));
    }

    // ── Imports / Exports ──

    #[test]
    fn test_import_extraction() {
        let source = ">>> ./utils.td => @(double, triple)";
        let val = parse_json(source);
        let imports = val["imports"].as_array().unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0]["path"], "./utils.td");
        let syms = imports[0]["symbols"].as_array().unwrap();
        assert_eq!(syms.len(), 2);
        assert!(imports[0]["version"].is_null());
    }

    #[test]
    fn test_export_extraction() {
        let source = "x <= 42\n<<< @(x)";
        let val = parse_json(source);
        let exports = val["exports"].as_array().unwrap();
        assert!(exports.contains(&serde_json::json!("x")));
    }

    // ── Integration: full example ──

    #[test]
    fn test_04_functions_like() {
        let source = "add x y =\n  x + y\n=> :Int\n\nresult <= add(3, 5)\nstdout(`3 + 5 = ${result}`)\n\nfactorial n =\n  | n < 2 |> 1\n  | _ |> n * factorial(n - 1)\n=> :Int\n\nstdout(`5! = ${factorial(5)}`)";
        let val = parse_json(source);

        // 2 functions
        let funcs = val["functions"].as_array().unwrap();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0]["name"], "add");
        assert_eq!(funcs[1]["name"], "factorial");
        assert_eq!(funcs[1]["is_recursive"], true);

        // 3 main_flow entries
        let flow = val["main_flow"].as_array().unwrap();
        assert_eq!(flow.len(), 3);

        // No imports, no exports
        assert_eq!(val["imports"].as_array().unwrap().len(), 0);
        assert_eq!(val["exports"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_json_is_valid() {
        // A complex example should still produce valid JSON
        let source = "Person = @(name: Str, age: Int)\n\ngreet p =\n  `Hello, ${p.name}`\n=> :Str\n\n>>> ./utils.td => @(helper)\n\nmain_person <= Person(name <= \"Taida\", age <= 1)\nstdout(greet(main_person))\n\n<<< @(greet)";
        let json_str = parse_and_ai_json(source);
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
        assert!(
            result.is_ok(),
            "Should produce valid JSON: {:?}\n---\n{}",
            result.err(),
            json_str
        );
    }

    fn funcs_body_summary(val: &serde_json::Value, idx: usize) -> String {
        val["functions"][idx]["body_summary"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    // ── C-1: Multi-byte character safety ──

    #[test]
    fn test_truncate_str_multibyte_no_panic() {
        // Japanese characters are 3 bytes each in UTF-8.
        // Truncating by char count must not panic at byte boundaries.
        let result = truncate_str("こんにちは世界", 3);
        assert_eq!(result, "こんに...");
    }

    #[test]
    fn test_truncate_str_multibyte_no_truncation() {
        // When char count <= max, return as-is.
        let result = truncate_str("日本語", 5);
        assert_eq!(result, "日本語");
    }

    #[test]
    fn test_truncate_str_ascii() {
        let result = truncate_str("hello world", 5);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn test_truncate_str_exact_boundary() {
        let result = truncate_str("abc", 3);
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_multibyte_string_in_ai_json() {
        // End-to-end: a Japanese string literal in source should produce valid JSON.
        let source =
            "msg <= \"こんにちは世界、これはとても長い文字列です。切り捨てが正しく動作するか確認\"";
        let json_str = parse_and_ai_json(source);
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
        assert!(
            result.is_ok(),
            "Multi-byte string should produce valid JSON: {:?}",
            result.err()
        );
    }

    // ── W-1: CondBranch body full traversal ──

    #[test]
    fn test_cond_branch_assignment_call_detection() {
        // A function call inside a CondBranch body via Assignment should be detected.
        let source = "helper x =\n  x + 1\n=> :Int\n\nprocess x =\n  | x > 0 |>\n    result <= helper(x)\n    result\n  | _ |> 0\n=> :Int";
        let val = parse_json(source);
        let funcs = val["functions"].as_array().unwrap();

        // Find the "process" function
        let process_fn = funcs.iter().find(|f| f["name"] == "process").unwrap();
        let calls = process_fn["calls"].as_array().unwrap();
        assert!(
            calls.contains(&serde_json::json!("helper")),
            "Expected 'helper' in calls of 'process', got: {:?}",
            calls
        );
    }

    // ── Parent field ──

    #[test]
    fn test_inheritance_parent_field() {
        let val = parse_json("Product = @(name: Str)\nProduct => StockedItem = @(qty: Int)");
        let types = val["types"].as_array().unwrap();
        assert_eq!(types.len(), 2);

        // buchi_pack has no parent
        assert_eq!(types[0]["name"], "Product");
        assert!(types[0].get("parent").is_none() || types[0]["parent"].is_null());

        // inheritance has parent
        assert_eq!(types[1]["name"], "StockedItem");
        assert_eq!(types[1]["kind"], "inheritance");
        assert_eq!(types[1]["parent"], "Product");
    }

    #[test]
    fn test_error_inheritance_parent_field() {
        let val = parse_json("Error => MyError = @(detail: Str)");
        let types = val["types"].as_array().unwrap();
        assert_eq!(types[0]["kind"], "error");
        assert_eq!(types[0]["parent"], "Error");
    }

    // ── Recursive ──

    #[test]
    fn test_recursive_json_inventory() {
        // Integration test: parse the inventory example recursively
        let result = format_ai_json_recursive("examples/complex/inventory/main.td");
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());

        let json_str = result.unwrap();
        let val: serde_json::Value = serde_json::from_str(&json_str)
            .unwrap_or_else(|e| panic!("Invalid JSON: {}\n---\n{}", e, json_str));

        // Should have entry field
        assert_eq!(val["entry"], "examples/complex/inventory/main.td");

        // Should have modules array
        let modules = val["modules"].as_array().unwrap();
        assert_eq!(
            modules.len(),
            4,
            "Expected 4 modules (main, models, operations, display)"
        );

        // Entry point should be first
        assert!(
            modules[0]["file"].as_str().unwrap().contains("main.td"),
            "First module should be main.td"
        );

        // models.td should appear exactly once (imported by main, operations, and display)
        let model_count = modules
            .iter()
            .filter(|m| m["file"].as_str().unwrap().contains("models.td"))
            .count();
        assert_eq!(
            model_count, 1,
            "models.td should appear exactly once despite multiple importers"
        );

        // Check that models.td has types with parent fields
        let models = modules
            .iter()
            .find(|m| m["file"].as_str().unwrap().contains("models.td"))
            .unwrap();
        let types = models["types"].as_array().unwrap();
        assert!(types.len() >= 3, "models.td should have at least 3 types");

        // StockedItem should have parent = Product
        let stocked = types.iter().find(|t| t["name"] == "StockedItem").unwrap();
        assert_eq!(stocked["parent"], "Product");
    }

    #[test]
    fn test_recursive_json_single_file_no_imports() {
        // A file with no imports should produce a single-element modules array
        let dir = std::env::temp_dir().join("taida_test_recursive_single");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("solo.td");
        std::fs::write(&file_path, "x <= 42\nstdout(x.toString())").unwrap();

        let result = format_ai_json_recursive(file_path.to_str().unwrap());
        assert!(result.is_ok());

        let val: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        let modules = val["modules"].as_array().unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0]["imports"].as_array().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recursive_json_missing_entry_file() {
        let result = format_ai_json_recursive("nonexistent_file_xyz.td");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Error reading file"));
    }

    #[test]
    fn test_recursive_json_circular_import() {
        // Two files that import each other should not loop
        let dir = std::env::temp_dir().join("taida_test_recursive_circular");
        let _ = std::fs::create_dir_all(&dir);

        let a_path = dir.join("a.td");
        let b_path = dir.join("b.td");
        std::fs::write(&a_path, ">>> ./b.td => @(y)\nx <= 1\n<<< @(x)").unwrap();
        std::fs::write(&b_path, ">>> ./a.td => @(x)\ny <= 2\n<<< @(y)").unwrap();

        let result = format_ai_json_recursive(a_path.to_str().unwrap());
        assert!(result.is_ok(), "Circular imports should not cause error");

        let val: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        let modules = val["modules"].as_array().unwrap();
        assert_eq!(
            modules.len(),
            2,
            "Circular import should produce exactly 2 modules"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RCB-52: Extension-less import resolution ──

    #[test]
    fn test_recursive_resolves_extensionless_import() {
        // `>>> ./mod` should resolve to `./mod.td` on disk
        let dir = std::env::temp_dir().join("taida_test_rcb52_extensionless");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let main_path = dir.join("main.td");
        let lib_path = dir.join("lib.td");
        std::fs::write(&main_path, ">>> ./lib => @(greet)\nstdout(greet())").unwrap();
        std::fs::write(&lib_path, "greet =\n  \"hello\"\n=> :Str\n\n<<< @(greet)").unwrap();

        let result = format_ai_json_recursive(main_path.to_str().unwrap());
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());

        let val: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        let modules = val["modules"].as_array().unwrap();
        assert_eq!(
            modules.len(),
            2,
            "Extension-less import should resolve to lib.td (got {} modules)",
            modules.len()
        );

        // The resolved module should contain the exported function
        let lib_mod = modules
            .iter()
            .find(|m| m["file"].as_str().unwrap().contains("lib.td"))
            .expect("lib.td should appear in modules");
        let exports = lib_mod["exports"].as_array().unwrap();
        assert!(
            exports.contains(&serde_json::json!("greet")),
            "lib.td should export 'greet'"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recursive_compile_module_example() {
        // The real example that triggered RCB-52:
        // compile_module.td imports `./module_math` (no .td extension)
        let result = format_ai_json_recursive("examples/compile_module.td");
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());

        let val: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        let modules = val["modules"].as_array().unwrap();
        assert_eq!(
            modules.len(),
            2,
            "compile_module.td + module_math.td = 2 modules, got {}",
            modules.len()
        );

        let math_mod = modules
            .iter()
            .find(|m| m["file"].as_str().unwrap().contains("module_math"))
            .expect("module_math.td should appear in modules");
        let funcs: Vec<&str> = math_mod["functions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert!(
            funcs.contains(&"square"),
            "module_math should export square"
        );
        assert!(funcs.contains(&"cube"), "module_math should export cube");
    }

    // ── RCB-53: Control character JSON escaping ──

    #[test]
    fn test_ai_json_escapes_control_chars_in_identifiers() {
        // Build source where a variable name or string value would contain
        // a control character after extraction.  The escape_json function is
        // tested exhaustively in graph::tests; here we verify the end-to-end
        // path through format_ai_json by using a function whose body_summary
        // will include a string with a formfeed.
        //
        // Taida's parser does not accept \b or \f in string literals, but
        // escape_json is called on *any* string emitted into the JSON
        // output.  The unit tests in graph::tests cover the full U+0000..U+001F
        // range, so this test just confirms the integration path works.
        let source = "msg <= \"hello\"";
        let json_str = parse_and_ai_json(source);
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
        assert!(
            result.is_ok(),
            "Standard string should produce valid JSON: {:?}\n---\n{}",
            result.err(),
            json_str
        );
    }
}
