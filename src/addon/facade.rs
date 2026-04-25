//! C25B-030 Phase 1G: shared addon-facade static analyser.
//!
//! This module hosts the backend-agnostic static analysis of an
//! addon-backed package's Taida-side facade (`<pkg_dir>/taida/
//! <stem>.td`). Both the native codegen lowering path
//! (`src/codegen/lower/imports.rs`) and the D26-planned WASM
//! backend will consume the same [`AddonFacadeSummary`] value:
//!
//! 1. Parse the facade entry file with the standard Taida parser.
//! 2. Recursively follow facade-internal `>>> ./X.td` relative
//!    imports, building a universe map of every local binding
//!    (public + private) seen across the file tree.
//! 3. Merge the user-visible surface (aliases / pack bindings /
//!    FuncDefs honoured by `<<<` or the implicit export rule)
//!    into an [`AddonFacadeSummary`].
//! 4. Transitively expand the summary with any private
//!    `_`-prefixed helpers that reachable exports reference.
//!
//! The interpreter's addon facade path
//! (`src/interpreter/module_eval.rs::load_addon_facade`) still
//! executes the facade as a dynamic module so it can exchange
//! runtime values with user code; that path is deliberately left
//! alone because the interpreter is Taida's reference
//! implementation. Codegen backends, by contrast, need the AST
//! shape up-front to emit link symbols, which is what this
//! module produces.
//!
//! Error surface: every diagnostic is a [`FacadeLoadError`] whose
//! `message` includes an actionable pointer (addon author vs.
//! language core vs. `C25B-030 Phase 1E-γ pending`) and the
//! originating facade path so IDE / CI tooling can navigate
//! directly to the offending source.
//!
//! Non-goals for Phase 1G:
//!
//! - TypeDef / EnumDef / MoldDef statements inside a facade
//!   (tracked for Phase 1E-γ; the real `taida-lang/terminal`
//!   tree does not use them today).
//! - Non-relative facade `>>>` targets (`>>> taida-lang/foo`,
//!   `>>> npm:*`, versioned imports).
//! - `<<< <path>` re-export clauses.
//!
//! These are rejected deterministically with diagnostic
//! messages pointing at the right follow-up issue.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::parser::{Expr, FuncDef, Statement};

use super::manifest::AddonManifest;

/// Statically extracted view of an addon facade file tree.
///
/// Produced by [`load_facade_summary`] and consumed by any codegen
/// backend that needs to map the facade's user-visible export
/// surface onto its own symbol / function table.
///
/// Semantics:
///
/// - `aliases` — facade-written `FacadeName <= lowercaseFn`
///   bindings. The `lowercaseFn` must appear in the addon
///   manifest's `[functions]` table; the backend resolves it back
///   to the ABI entry when emitting call sites.
/// - `pack_bindings` — facade-written `FacadeName <= <expr>`
///   bindings whose RHS is a pure-Taida value (pack literal,
///   scalar, list, template, arithmetic, method/function call,
///   field access, mold/type instantiation). The backend replays
///   each expression verbatim into the module's init path.
/// - `facade_funcs` — facade-declared `Name args = body => :Type`
///   FuncDefs. Both public (in `exports` or implicitly exported)
///   and private (`_`-prefixed) helpers are collected so the
///   backend can lower internal sibling / recursion calls. Only
///   names that appear in `exports` are visible to user code.
/// - `exports` — symbols listed in the facade's `<<<` export
///   statement, if any. When empty, every alias / pack binding /
///   FuncDef is implicitly exported (same rule the interpreter
///   uses for module-level snapshots).
#[derive(Debug, Default, Clone)]
pub struct AddonFacadeSummary {
    /// Map `FacadeName` -> lowercase addon function name.
    pub aliases: HashMap<String, String>,
    /// Map `FacadeName` -> RHS expression AST.
    pub pack_bindings: HashMap<String, Expr>,
    /// Set of names explicitly listed in the facade's `<<<`
    /// export statement.
    pub exports: HashSet<String>,
    /// Map `FacadeFnName` -> full `FuncDef` AST. Includes both
    /// exported public functions and facade-private helpers
    /// (names starting with `_`).
    pub facade_funcs: HashMap<String, FuncDef>,
}

/// Every diagnostic produced by the facade loader is a
/// [`FacadeLoadError`]. The `message` field is the user-facing
/// string a backend should surface verbatim (error code mapping
/// is a backend concern).
#[derive(Debug, Clone)]
pub struct FacadeLoadError {
    pub message: String,
}

impl std::fmt::Display for FacadeLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FacadeLoadError {}

/// Parse the optional Taida-side facade for an addon-backed
/// package and return an [`AddonFacadeSummary`], if a facade
/// file is present at `<pkg_dir>/taida/<stem>.td` where `<stem>`
/// is the final `/`-segment of the canonical package id
/// (e.g. `terminal` for `taida-lang/terminal`).
///
/// Returns `Ok(None)` when no facade file exists (lowercase-only
/// addons work fine without a facade); `Ok(Some(summary))` when
/// the facade parses and the summary is complete after reachability
/// expansion; `Err(FacadeLoadError)` for parse errors, malformed
/// facade constructs, or missing child symbols.
///
/// Each backend is expected to call this function from its own
/// import-lowering entry point and then translate the returned
/// summary into backend-local bookkeeping (IR function symbols,
/// global binding replay lists, etc.).
pub fn load_facade_summary(
    pkg_dir: &Path,
    manifest: &AddonManifest,
    import_path: &str,
) -> Result<Option<AddonFacadeSummary>, FacadeLoadError> {
    let stem = manifest
        .package
        .rsplit('/')
        .next()
        .unwrap_or(manifest.package.as_str());
    let facade_path = pkg_dir.join("taida").join(format!("{}.td", stem));
    if !facade_path.exists() {
        return Ok(None);
    }

    let mut summary = AddonFacadeSummary::default();
    let mut visiting: HashSet<PathBuf> = HashSet::new();
    // Universe maps track EVERY local binding across the facade
    // file tree (public and private). `expand_reachable_symbols`
    // consults them to promote `_`-prefixed helpers that a
    // harvested FuncDef / pack expression transitively needs.
    let mut universe_funcs: HashMap<String, FuncDef> = HashMap::new();
    let mut universe_packs: HashMap<String, Expr> = HashMap::new();
    let mut universe_aliases: HashMap<String, String> = HashMap::new();

    load_facade_file(
        &facade_path,
        manifest,
        import_path,
        None,
        &mut summary,
        &mut visiting,
        &mut universe_funcs,
        &mut universe_packs,
        &mut universe_aliases,
    )?;

    // If the entry facade defined no explicit `<<<` exports, fall
    // back to exporting every top-level binding we understood.
    if summary.exports.is_empty() {
        for k in summary.aliases.keys() {
            summary.exports.insert(k.clone());
        }
        for k in summary.pack_bindings.keys() {
            summary.exports.insert(k.clone());
        }
        for k in summary.facade_funcs.keys() {
            summary.exports.insert(k.clone());
        }
    }

    expand_reachable_symbols(
        &mut summary,
        &universe_funcs,
        &universe_packs,
        &universe_aliases,
    );

    Ok(Some(summary))
}

/// Recursive facade file loader. Walks a single file's
/// statements, harvests aliases / pack bindings / FuncDefs, and
/// drives recursion into every relative `>>> ./X.td` child.
///
/// Arguments mirror the legacy in-codegen loader so the
/// extraction is semantics-preserving:
///
/// - `facade_path` — absolute path of the file to load.
/// - `manifest` — owning addon's manifest (aliases consult the
///   `[functions]` table).
/// - `import_path` — user-visible package id (used in diagnostics).
/// - `restrict_to` — `Some(set)` means "merge only these symbols"
///   (the parent's `>>> ./X.td => @(a, b)` surface); `None`
///   means "merge everything understood from this facade".
/// - `out_summary` — accumulator for aliases / packs / funcs /
///   exports.
/// - `visiting` — recursion stack for circular-import detection.
/// - `universe_*` — per-facade-tree maps of every binding,
///   consulted later by `expand_reachable_symbols`.
#[allow(clippy::too_many_arguments)]
fn load_facade_file(
    facade_path: &Path,
    manifest: &AddonManifest,
    import_path: &str,
    restrict_to: Option<&HashSet<String>>,
    out_summary: &mut AddonFacadeSummary,
    visiting: &mut HashSet<PathBuf>,
    universe_funcs: &mut HashMap<String, FuncDef>,
    universe_packs: &mut HashMap<String, Expr>,
    universe_aliases: &mut HashMap<String, String>,
) -> Result<(), FacadeLoadError> {
    let canonical = facade_path
        .canonicalize()
        .unwrap_or_else(|_| facade_path.to_path_buf());
    if !visiting.insert(canonical.clone()) {
        return Err(FacadeLoadError {
            message: format!(
                "circular facade import detected while loading addon facade chain for '{}' \
                 at '{}'",
                import_path,
                facade_path.display()
            ),
        });
    }

    let source = std::fs::read_to_string(facade_path).map_err(|e| FacadeLoadError {
        message: format!(
            "cannot read addon facade '{}' for '{}': {}",
            facade_path.display(),
            import_path,
            e
        ),
    })?;
    let (program, parse_errors) = crate::parser::parse(&source);
    if !parse_errors.is_empty() {
        visiting.remove(&canonical);
        return Err(FacadeLoadError {
            message: format!(
                "parse errors in addon facade '{}' for '{}': {}",
                facade_path.display(),
                import_path,
                parse_errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        });
    }

    let mut local_aliases: HashMap<String, String> = HashMap::new();
    let mut local_packs: HashMap<String, Expr> = HashMap::new();
    let mut local_funcs: HashMap<String, FuncDef> = HashMap::new();
    let mut local_exports: HashSet<String> = HashSet::new();
    let mut child_imports: Vec<(PathBuf, HashMap<String, String>)> = Vec::new();

    for stmt in &program.statements {
        match stmt {
            Statement::Assignment(assign) => match &assign.value {
                // `Name <= Ident(B)` → alias if B is a known addon fn.
                Expr::Ident(target_fn, _) => {
                    if manifest.functions.contains_key(target_fn) {
                        local_aliases.insert(assign.target.clone(), target_fn.clone());
                    } else {
                        visiting.remove(&canonical);
                        return Err(FacadeLoadError {
                            message: format!(
                                "addon facade '{}' aliases '{}' to '{}' which is not listed \
                                 in [functions] of '{}'. Chained facade aliasing across \
                                 pure-Taida helpers is not yet supported (C25B-030 Phase 1E-γ).",
                                facade_path.display(),
                                assign.target,
                                target_fn,
                                import_path
                            ),
                        });
                    }
                }
                // Phase 1E-β: accepted pure-Taida RHS shapes. See
                // `src/codegen/lower/imports.rs` comment block for
                // the rationale behind the specific whitelist.
                Expr::BuchiPack(_, _)
                | Expr::IntLit(_, _)
                | Expr::FloatLit(_, _)
                | Expr::StringLit(_, _)
                | Expr::BoolLit(_, _)
                | Expr::TemplateLit(_, _)
                | Expr::ListLit(_, _)
                | Expr::BinaryOp(_, _, _, _)
                | Expr::UnaryOp(_, _, _)
                | Expr::FuncCall(_, _, _)
                | Expr::MethodCall(_, _, _, _)
                | Expr::FieldAccess(_, _, _)
                | Expr::MoldInst(_, _, _, _)
                | Expr::TypeInst(_, _, _) => {
                    local_packs.insert(assign.target.clone(), assign.value.clone());
                }
                _ => {
                    visiting.remove(&canonical);
                    return Err(FacadeLoadError {
                        message: format!(
                            "addon facade '{}' binds '{}' to an unsupported expression shape \
                             (C25B-030 Phase 1E-β supports `Name <= lowercaseFn` aliases, \
                             `Name <= @(...)` pack literals, scalar / list / arithmetic \
                             value bindings, and FuncDef statements; other top-level \
                             shapes are tracked for Phase 1E-γ).",
                            facade_path.display(),
                            assign.target
                        ),
                    });
                }
            },
            Statement::Import(import_stmt) => {
                let p = &import_stmt.path;
                if !(p.starts_with("./") || p.starts_with("../")) {
                    visiting.remove(&canonical);
                    return Err(FacadeLoadError {
                        message: format!(
                            "addon facade '{}' uses `>>> {}` — only relative `>>> ./X.td` \
                             or `>>> ../X.td` imports are supported in addon facades \
                             (C25B-030 Phase 1E-α).",
                            facade_path.display(),
                            p
                        ),
                    });
                }
                if import_stmt.version.is_some() {
                    visiting.remove(&canonical);
                    return Err(FacadeLoadError {
                        message: format!(
                            "addon facade '{}' uses `>>> {}@...` — versioned imports are not \
                             permitted for facade-internal relative imports.",
                            facade_path.display(),
                            p
                        ),
                    });
                }
                let base_dir = facade_path
                    .parent()
                    .ok_or_else(|| FacadeLoadError {
                        message: format!(
                            "addon facade '{}' has no parent directory while resolving \
                             internal import '{}'",
                            facade_path.display(),
                            p
                        ),
                    })?
                    .to_path_buf();
                let child_path = if let Some(rest) = p.strip_prefix("./") {
                    base_dir.join(rest)
                } else {
                    base_dir.join(p)
                };
                if !child_path.exists() {
                    visiting.remove(&canonical);
                    return Err(FacadeLoadError {
                        message: format!(
                            "addon facade '{}' imports '{}' which resolves to '{}' but the \
                             file does not exist.",
                            facade_path.display(),
                            p,
                            child_path.display()
                        ),
                    });
                }
                let requested: HashMap<String, String> = import_stmt
                    .symbols
                    .iter()
                    .map(|s| {
                        (
                            s.name.clone(),
                            s.alias.clone().unwrap_or_else(|| s.name.clone()),
                        )
                    })
                    .collect();
                child_imports.push((child_path, requested));
            }
            Statement::Export(export_stmt) => {
                if export_stmt.path.is_some() {
                    visiting.remove(&canonical);
                    return Err(FacadeLoadError {
                        message: format!(
                            "addon facade '{}' uses `<<< <path>` re-export which is not \
                             supported.",
                            facade_path.display()
                        ),
                    });
                }
                for sym in &export_stmt.symbols {
                    local_exports.insert(sym.clone());
                }
            }
            Statement::FuncDef(fd) => {
                if local_funcs.contains_key(&fd.name) || local_packs.contains_key(&fd.name) {
                    visiting.remove(&canonical);
                    return Err(FacadeLoadError {
                        message: format!(
                            "addon facade '{}' defines '{}' more than once — drop the \
                             duplicate binding or rename one side.",
                            facade_path.display(),
                            fd.name
                        ),
                    });
                }
                local_funcs.insert(fd.name.clone(), fd.clone());
            }
            Statement::TypeDef(td) => {
                visiting.remove(&canonical);
                return Err(FacadeLoadError {
                    message: format!(
                        "addon facade '{}' declares TypeDef '{}' — TypeDef statements \
                         inside addon facades are not yet supported for native codegen \
                         (C25B-030 Phase 1E-γ pending).",
                        facade_path.display(),
                        td.name
                    ),
                });
            }
            Statement::EnumDef(ed) => {
                visiting.remove(&canonical);
                return Err(FacadeLoadError {
                    message: format!(
                        "addon facade '{}' declares EnumDef '{}' — EnumDef statements \
                         inside addon facades are not yet supported for native codegen \
                         (C25B-030 Phase 1E-γ pending).",
                        facade_path.display(),
                        ed.name
                    ),
                });
            }
            Statement::MoldDef(md) => {
                visiting.remove(&canonical);
                return Err(FacadeLoadError {
                    message: format!(
                        "addon facade '{}' declares MoldDef '{}' — MoldDef statements \
                         inside addon facades are not yet supported for native codegen \
                         (C25B-030 Phase 1E-γ pending).",
                        facade_path.display(),
                        md.name
                    ),
                });
            }
            _ => {
                visiting.remove(&canonical);
                return Err(FacadeLoadError {
                    message: format!(
                        "addon facade '{}' contains an unsupported top-level construct \
                         (C25B-030 Phase 1E-β supports assignments, FuncDefs, \
                         `>>> ./X.td` relative imports, and `<<<` exports; TypeDef / \
                         EnumDef / MoldDef are tracked for Phase 1E-γ).",
                        facade_path.display()
                    ),
                });
            }
        }
    }

    for (child_path, requested) in child_imports {
        if requested.is_empty() {
            load_facade_file(
                &child_path,
                manifest,
                import_path,
                None,
                out_summary,
                visiting,
                universe_funcs,
                universe_packs,
                universe_aliases,
            )?;
            continue;
        }

        let requested_names: HashSet<String> = requested.keys().cloned().collect();
        let mut child_summary = AddonFacadeSummary::default();
        load_facade_file(
            &child_path,
            manifest,
            import_path,
            Some(&requested_names),
            &mut child_summary,
            visiting,
            universe_funcs,
            universe_packs,
            universe_aliases,
        )?;

        for (orig_name, local_name) in &requested {
            if let Some(target) = child_summary.aliases.get(orig_name) {
                out_summary
                    .aliases
                    .insert(local_name.clone(), target.clone());
                universe_aliases.insert(local_name.clone(), target.clone());
            }
            if let Some(expr) = child_summary.pack_bindings.get(orig_name) {
                out_summary
                    .pack_bindings
                    .insert(local_name.clone(), expr.clone());
                universe_packs.insert(local_name.clone(), expr.clone());
            }
            if let Some(fd) = child_summary.facade_funcs.get(orig_name) {
                out_summary
                    .facade_funcs
                    .insert(local_name.clone(), fd.clone());
                universe_funcs.insert(local_name.clone(), fd.clone());
            }
        }
    }

    let use_set: HashSet<String> = if let Some(set) = restrict_to {
        set.clone()
    } else if !local_exports.is_empty() {
        local_exports.clone()
    } else {
        let mut s = HashSet::new();
        s.extend(local_aliases.keys().cloned());
        s.extend(local_packs.keys().cloned());
        s.extend(local_funcs.keys().cloned());
        s
    };

    for (name, target) in &local_aliases {
        if use_set.contains(name) {
            out_summary.aliases.insert(name.clone(), target.clone());
        }
        universe_aliases
            .entry(name.clone())
            .or_insert_with(|| target.clone());
    }
    for (name, expr) in &local_packs {
        if use_set.contains(name) {
            out_summary.pack_bindings.insert(name.clone(), expr.clone());
        }
        universe_packs
            .entry(name.clone())
            .or_insert_with(|| expr.clone());
    }
    for (name, fd) in &local_funcs {
        if use_set.contains(name) {
            out_summary.facade_funcs.insert(name.clone(), fd.clone());
        }
        universe_funcs
            .entry(name.clone())
            .or_insert_with(|| fd.clone());
    }

    if restrict_to.is_none() && !local_exports.is_empty() {
        out_summary.exports.extend(local_exports.iter().cloned());
    }

    if let Some(set) = restrict_to {
        for name in set {
            let produced = out_summary.aliases.contains_key(name)
                || out_summary.pack_bindings.contains_key(name)
                || out_summary.facade_funcs.contains_key(name);
            if !produced {
                visiting.remove(&canonical);
                return Err(FacadeLoadError {
                    message: format!(
                        "addon facade '{}' requested symbol '{}' from '{}' but that file \
                         (and its child facades) did not produce a matching binding. \
                         Possible causes: the symbol is declared via a TypeDef / EnumDef / \
                         MoldDef (C25B-030 Phase 1E-γ pending), the symbol is misspelled, \
                         or the symbol lives in a sibling facade not yet imported.",
                        import_path,
                        name,
                        facade_path.display()
                    ),
                });
            }
        }
    }

    visiting.remove(&canonical);
    Ok(())
}

/// Fixpoint loop that grows `summary` with every private binding
/// transitively referenced by an already-harvested FuncDef body
/// or pack expression. Required so a facade FuncDef that calls
/// `_bufferNewInner` or splices `${_sep}` into a template still
/// resolves after lowering even if the user's import never named
/// the private helper.
fn expand_reachable_symbols(
    summary: &mut AddonFacadeSummary,
    all_local_funcs: &HashMap<String, FuncDef>,
    all_local_packs: &HashMap<String, Expr>,
    all_local_aliases: &HashMap<String, String>,
) {
    let mut changed = true;
    while changed {
        changed = false;
        let mut refs: HashSet<String> = HashSet::new();
        for fn_def in summary.facade_funcs.values() {
            let param_names: HashSet<String> =
                fn_def.params.iter().map(|p| p.name.clone()).collect();
            collect_refs_in_body(&fn_def.body, &param_names, &mut refs);
        }
        let empty_params: HashSet<String> = HashSet::new();
        for expr in summary.pack_bindings.values() {
            collect_refs_in_expr(expr, &empty_params, &mut refs);
        }
        for r in &refs {
            if all_local_funcs.contains_key(r) && !summary.facade_funcs.contains_key(r) {
                summary
                    .facade_funcs
                    .insert(r.clone(), all_local_funcs[r].clone());
                changed = true;
            }
            if all_local_packs.contains_key(r) && !summary.pack_bindings.contains_key(r) {
                summary
                    .pack_bindings
                    .insert(r.clone(), all_local_packs[r].clone());
                changed = true;
            }
            if all_local_aliases.contains_key(r) && !summary.aliases.contains_key(r) {
                summary
                    .aliases
                    .insert(r.clone(), all_local_aliases[r].clone());
                changed = true;
            }
        }
    }
}

/// Collect every identifier that appears as a free variable in
/// the given statement body. Parameters and locally-defined names
/// are bound; every other `Expr::Ident` is reported. Kept
/// lighter-weight than the codegen-side `collect_free_vars_inner`
/// because backends apply their own top-level / user-function
/// filters after this walk.
fn collect_refs_in_body(
    body: &[Statement],
    param_names: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    let mut bound: HashSet<String> = param_names.clone();
    for stmt in body {
        if let Statement::Assignment(assign) = stmt {
            bound.insert(assign.target.clone());
        }
    }
    for stmt in body {
        collect_refs_in_stmt(stmt, &bound, out);
    }
}

fn collect_refs_in_stmt(stmt: &Statement, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match stmt {
        Statement::Expr(expr) => collect_refs_in_expr(expr, bound, out),
        Statement::Assignment(assign) => collect_refs_in_expr(&assign.value, bound, out),
        Statement::UnmoldForward(uf) => collect_refs_in_expr(&uf.source, bound, out),
        Statement::UnmoldBackward(ub) => collect_refs_in_expr(&ub.source, bound, out),
        Statement::ErrorCeiling(ec) => {
            for inner in &ec.handler_body {
                collect_refs_in_stmt(inner, bound, out);
            }
        }
        _ => {}
    }
}

fn collect_refs_in_expr(expr: &Expr, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name, _) if !bound.contains(name) => {
            out.insert(name.clone());
        }
        Expr::Ident(_, _) => {}
        Expr::BinaryOp(lhs, _, rhs, _) => {
            collect_refs_in_expr(lhs, bound, out);
            collect_refs_in_expr(rhs, bound, out);
        }
        Expr::UnaryOp(_, operand, _) => {
            collect_refs_in_expr(operand, bound, out);
        }
        Expr::FuncCall(callee, args, _) => {
            collect_refs_in_expr(callee, bound, out);
            for a in args {
                collect_refs_in_expr(a, bound, out);
            }
        }
        Expr::FieldAccess(obj, _, _) => {
            collect_refs_in_expr(obj, bound, out);
        }
        Expr::MethodCall(obj, _, args, _) => {
            collect_refs_in_expr(obj, bound, out);
            for a in args {
                collect_refs_in_expr(a, bound, out);
            }
        }
        Expr::Pipeline(exprs, _) => {
            for e in exprs {
                collect_refs_in_expr(e, bound, out);
            }
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    collect_refs_in_expr(cond, bound, out);
                }
                for s in &arm.body {
                    collect_refs_in_stmt(s, bound, out);
                }
            }
        }
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            for f in fields {
                collect_refs_in_expr(&f.value, bound, out);
            }
        }
        Expr::ListLit(items, _) => {
            for i in items {
                collect_refs_in_expr(i, bound, out);
            }
        }
        Expr::MoldInst(_, args, fields, _) => {
            for a in args {
                collect_refs_in_expr(a, bound, out);
            }
            for f in fields {
                collect_refs_in_expr(&f.value, bound, out);
            }
        }
        Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
            collect_refs_in_expr(inner, bound, out);
        }
        Expr::Lambda(params, body, _) => {
            let mut inner_bound = bound.clone();
            for p in params {
                inner_bound.insert(p.name.clone());
            }
            collect_refs_in_expr(body, &inner_bound, out);
        }
        // C25B-030 Phase 1F: template interpolations are re-parsed
        // by the real lowering path, so the reachability walker
        // must do the same to see references like `${_sep}`.
        Expr::TemplateLit(template, _) => {
            collect_refs_in_template(template, bound, out);
        }
        _ => {}
    }
}

/// Split a `TemplateLit` body on `${...}` boundaries, re-parse
/// each interpolation as a standalone Taida expression, and walk
/// it for identifier references. Mirrors the parser invocation in
/// `src/codegen/lower/expr.rs::lower_template_lit` so reachability
/// sees exactly the same names the backend will later resolve.
fn collect_refs_in_template(template: &str, bound: &HashSet<String>, out: &mut HashSet<String>) {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            i += 2;
            let start = i;
            let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' {
                    depth += 1;
                }
                if chars[i] == '}' {
                    depth -= 1;
                }
                if depth > 0 {
                    i += 1;
                }
            }
            let expr_str: String = chars[start..i].iter().collect();
            let trimmed = expr_str.trim();
            let (program, errors) = crate::parser::parse(trimmed);
            if errors.is_empty()
                && !program.statements.is_empty()
                && let Statement::Expr(ref parsed_expr) = program.statements[0]
            {
                collect_refs_in_expr(parsed_expr, bound, out);
            } else if !trimmed.is_empty() && !bound.contains(trimmed) {
                out.insert(trimmed.to_string());
            }
            if i < chars.len() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::manifest::{AddonManifest, PrebuildConfig};
    use std::collections::BTreeMap;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn test_manifest(package: &str, fns: &[(&str, u32)]) -> AddonManifest {
        let mut functions = BTreeMap::new();
        for (name, arity) in fns {
            functions.insert((*name).to_string(), *arity);
        }
        AddonManifest {
            manifest_path: PathBuf::from("/tmp/nonexistent/addon.toml"),
            abi: 1,
            entry: "taida_addon_get_v1".to_string(),
            package: package.to_string(),
            library: "taida_lang_test".to_string(),
            functions,
            targets: crate::addon::manifest::default_addon_targets(),
            prebuild: PrebuildConfig::default(),
        }
    }

    fn mk_pkg(tmp: &Path, package: &str, facade_td: &str, sibling: &[(&str, &str)]) {
        let pkg = tmp.join(".taida").join("deps").join(package);
        let taida_dir = pkg.join("taida");
        std::fs::create_dir_all(&taida_dir).unwrap();
        let stem = package.rsplit('/').next().unwrap();
        write(&taida_dir.join(format!("{}.td", stem)), facade_td);
        for (name, body) in sibling {
            write(&taida_dir.join(name), body);
        }
    }

    fn facade_pkg_dir(tmp: &Path, package: &str) -> PathBuf {
        tmp.join(".taida").join("deps").join(package)
    }

    #[test]
    fn load_facade_summary_returns_none_when_no_facade_file() {
        let tmp = std::env::temp_dir().join(format!(
            "c25b030_1g_none_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg = tmp.join(".taida").join("deps").join("tst/no-facade");
        std::fs::create_dir_all(&pkg).unwrap();
        let manifest = test_manifest("tst/no-facade", &[("foo", 0)]);
        let got = load_facade_summary(&pkg, &manifest, "tst/no-facade").unwrap();
        assert!(got.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_facade_summary_harvests_mixed_constructs() {
        let tmp = std::env::temp_dir().join(format!(
            "c25b030_1g_mixed_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let facade = r#"
>>> ./helper.td => @(Join2)

KeyKind <= @(
  Char <= 0
  Enter <= 1
)

Greet who = Join2("hi", who) => :Str

<<< @(KeyKind, Greet)
"#;
        let helper = r#"
_sep <= "-"

Join2 a b = `${a}${_sep}${b}` => :Str

<<< @(Join2)
"#;
        mk_pkg(&tmp, "tst/mixed", facade, &[("helper.td", helper)]);
        let manifest = test_manifest("tst/mixed", &[("someFn", 0)]);
        let pkg_dir = facade_pkg_dir(&tmp, "tst/mixed");
        let got = load_facade_summary(&pkg_dir, &manifest, "tst/mixed")
            .expect("loader should not error")
            .expect("facade should exist");

        assert!(
            got.pack_bindings.contains_key("KeyKind"),
            "KeyKind pack missing"
        );
        assert!(
            got.pack_bindings.contains_key("_sep"),
            "private `_sep` must be reachable-pulled from helper.td via Join2 template"
        );
        assert!(got.facade_funcs.contains_key("Greet"));
        assert!(
            got.facade_funcs.contains_key("Join2"),
            "Join2 must be pulled through reachability from Greet's body"
        );
        assert!(got.exports.contains("KeyKind"));
        assert!(got.exports.contains("Greet"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_facade_summary_rejects_typedef_inside_facade() {
        let tmp = std::env::temp_dir().join(format!(
            "c25b030_1g_td_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // `Name = @(...)` at top level is parsed as a TypeDef
        // (the Taida parser distinguishes assignment vs TypeDef by
        // the `<=` vs `=` operator on LHS of a pack literal).
        let facade = r#"
Point = @(
  x <= 0
  y <= 0
)

<<< @(Point)
"#;
        mk_pkg(&tmp, "tst/td", facade, &[]);
        let manifest = test_manifest("tst/td", &[("noop", 0)]);
        let pkg_dir = facade_pkg_dir(&tmp, "tst/td");
        let err = load_facade_summary(&pkg_dir, &manifest, "tst/td").unwrap_err();
        assert!(
            err.message.contains("TypeDef") && err.message.contains("Phase 1E-γ pending"),
            "error must name TypeDef and point at Phase 1E-γ, got: {}",
            err.message
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_facade_summary_rejects_missing_child_symbol() {
        let tmp = std::env::temp_dir().join(format!(
            "c25b030_1g_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let facade = r#"
>>> ./child.td => @(Nope)

<<< @(Nope)
"#;
        let child = r#"
Yep <= @(x <= 1)

<<< @(Yep)
"#;
        mk_pkg(&tmp, "tst/missing", facade, &[("child.td", child)]);
        let manifest = test_manifest("tst/missing", &[("anything", 0)]);
        let pkg_dir = facade_pkg_dir(&tmp, "tst/missing");
        let err = load_facade_summary(&pkg_dir, &manifest, "tst/missing").unwrap_err();
        assert!(
            err.message.contains("Nope") && err.message.contains("matching binding"),
            "missing-symbol error should name `Nope`, got: {}",
            err.message
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn template_reachability_pulls_private_helper_across_files() {
        let tmp = std::env::temp_dir().join(format!(
            "c25b030_1g_tmpl_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let facade = r#"
>>> ./child.td => @(Wrap)

<<< @(Wrap)
"#;
        let child = r#"
_suffix <= "!!"

Wrap word = `${word}${_suffix}` => :Str

<<< @(Wrap)
"#;
        mk_pkg(&tmp, "tst/tmpl", facade, &[("child.td", child)]);
        let manifest = test_manifest("tst/tmpl", &[("noop", 0)]);
        let pkg_dir = facade_pkg_dir(&tmp, "tst/tmpl");
        let got = load_facade_summary(&pkg_dir, &manifest, "tst/tmpl")
            .unwrap()
            .unwrap();
        assert!(
            got.pack_bindings.contains_key("_suffix"),
            "_suffix must be promoted via the template reachability walk"
        );
        assert!(got.facade_funcs.contains_key("Wrap"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
