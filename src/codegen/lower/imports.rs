// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups imports methods of the `Lowering` struct (placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{
    AddonFacadeSummary, AddonFuncRef, ImportedSymbolKind, InheritanceChainFields, LowerError,
    Lowering, simple_hash,
};
use crate::codegen::ir::*;
use crate::parser::*;

impl Lowering {
    /// C18-1: Read an exporter module's `.td` source and register any
    /// `EnumDef` whose name is being imported into `self.enum_defs`.
    /// Mirror of the interpreter / type-checker / JS codegen behaviour so
    /// `Color:Red()` in the importer can resolve at codegen time.
    ///
    /// Silent no-op for:
    /// - core-bundled (`taida-lang/*`) and npm paths (pre-filtered by caller)
    /// - unresolved paths / unreadable files / parse errors — downstream
    ///   lowering emits the real diagnostic
    ///
    /// For relative / absolute paths the resolver uses `self.source_dir`;
    /// for package paths we follow `resolve_package_module` chains so
    /// `>>> org/pkg => @(Color)` and submodule imports both succeed.
    pub(super) fn absorb_cross_module_enum_defs(&mut self, import: &crate::parser::ImportStmt) {
        let td_path = if import.path.starts_with("./")
            || import.path.starts_with("../")
            || import.path.starts_with('/')
        {
            let source_dir = match &self.source_dir {
                Some(d) => d.clone(),
                None => return,
            };
            let p = source_dir.join(&import.path);
            if !p.exists() {
                return;
            }
            p
        } else if import.path.starts_with("npm:")
            || import.path == "taida-lang/net"
            || import.path == "taida-lang/js"
            || import.path == "taida-lang/os"
            || import.path == "taida-lang/crypto"
            || import.path == "taida-lang/pool"
        {
            // Core-bundled / npm packages — they don't define user
            // Enum types in .td sources that we can read, so there is
            // nothing to absorb.
            return;
        } else {
            // C18B-004 fix: package import (`org/pkg` or
            // `org/pkg/submodule`). Mirror the checker resolver path
            // (`src/types/checker.rs::absorb_cross_module_enum_defs`)
            // so `>>> acme/lib => @(Color)` produces the same
            // `self.enum_defs` entries that the local-path branch
            // would produce. Without this, downstream codegen for
            // `Color:Red()` raised `Unknown enum variant` at lowering
            // time even though the checker happily resolved the same
            // import.
            let source_dir = match &self.source_dir {
                Some(d) => d.clone(),
                None => return,
            };
            let project_root = Self::find_project_root(&source_dir);
            let resolution = if let Some(ver) = import.version.as_ref() {
                crate::pkg::resolver::resolve_package_module_versioned(
                    &project_root,
                    &import.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(&project_root, &import.path)
            };
            let resolution = match resolution {
                Some(r) => r,
                None => return,
            };
            match &resolution.submodule {
                Some(sub) => {
                    let sub_path = resolution.pkg_dir.join(format!("{}.td", sub));
                    if !sub_path.exists() {
                        return;
                    }
                    sub_path
                }
                None => {
                    let entry_name =
                        match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                            Ok(Some(manifest)) => manifest.entry,
                            _ => "main.td".to_string(),
                        };
                    let entry_path = if let Some(stripped) = entry_name.strip_prefix("./") {
                        resolution.pkg_dir.join(stripped)
                    } else {
                        resolution.pkg_dir.join(&entry_name)
                    };
                    if !entry_path.exists() {
                        return;
                    }
                    entry_path
                }
            }
        };

        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _parse_errors) = crate::parser::parse(&source);

        let requested: std::collections::HashMap<&str, &str> = import
            .symbols
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.alias.as_deref().unwrap_or(s.name.as_str()),
                )
            })
            .collect();
        if requested.is_empty() {
            return;
        }

        for stmt in &program.statements {
            if let crate::parser::Statement::EnumDef(ed) = stmt
                && let Some(&local_name) = requested.get(ed.name.as_str())
            {
                let variants: Vec<String> = ed.variants.iter().map(|v| v.name.clone()).collect();
                // A local redefinition (same-name EnumDef later in the
                // consumer) will overwrite this entry through the
                // `Statement::EnumDef` branch in `lower_program`'s 1st
                // pass — the checker guards order consistency via
                // [E1618], so either the lists agree or we never reach
                // codegen. Using `entry().or_insert` keeps the idiom
                // symmetric with the JS backend.
                self.enum_defs
                    .entry(local_name.to_string())
                    .or_insert(variants);
            }
        }
    }

    /// RC1 Phase 4 helper: resolve only the **package directory** for
    /// an import path, without producing a `.td` source path. Used by
    /// the addon-policy guard in `Statement::Import` so the Cranelift
    /// native lower can detect addon-backed packages and emit a
    /// deterministic compile-time error rather than silently
    /// generating native call symbols that would never resolve.
    ///
    /// Returns `None` for relative / absolute / project-root /
    /// `std/` / `npm:` imports — those can never be addon-backed.
    /// Also returns `None` for submodule imports (`org/pkg/sub`)
    /// because RC1 addons are package-level only.
    pub(super) fn try_locate_addon_pkg_dir(
        &self,
        path: &str,
        version: Option<&str>,
    ) -> Option<std::path::PathBuf> {
        if path.starts_with("./")
            || path.starts_with("../")
            || path.starts_with('/')
            || path.starts_with("~/")
            || path.starts_with("std/")
            || path.starts_with("npm:")
        {
            return None;
        }
        let source_dir = self.source_dir.as_ref()?;
        let project_root = Self::find_project_root(source_dir);
        let resolution = if let Some(ver) = version {
            crate::pkg::resolver::resolve_package_module_versioned(&project_root, path, ver)
        } else {
            crate::pkg::resolver::resolve_package_module(&project_root, path)
        }?;
        if resolution.submodule.is_some() {
            return None;
        }
        Some(resolution.pkg_dir)
    }

    /// RC2.5 Phase 1: lower an addon-backed package import.
    ///
    /// Reads `native/addon.toml`, resolves the cdylib absolute path at
    /// build time (per `.dev/RC2_5_IMPL_SPEC.md` F-4), and registers each
    /// imported symbol in `addon_func_refs` for later dispatch at the
    /// call site via `taida_addon_call`.
    ///
    /// Failures surface as compile errors (manifest missing / symbol
    /// not declared in `[functions]` / cdylib not yet built). Runtime
    /// dispatch failures are out of scope here.
    pub(super) fn lower_addon_import(
        &mut self,
        pkg_dir: &std::path::Path,
        import_path: &str,
        import_stmt: &crate::parser::ImportStmt,
    ) -> Result<(), LowerError> {
        let manifest_path = pkg_dir.join("native").join("addon.toml");
        let manifest =
            crate::addon::manifest::parse_addon_manifest(&manifest_path).map_err(|e| {
                LowerError {
                    message: format!("addon manifest load failed for '{}': {}", import_path, e),
                }
            })?;

        // Resolve cdylib absolute path at build time. The path is
        // embedded in .rodata and consumed by `taida_addon_call` at
        // runtime; post-build relocation is a known limitation
        // (RC2.5B-004).
        let cdylib_path = crate::addon::registry::resolve_cdylib_path(pkg_dir, &manifest.library)
            .ok_or_else(|| LowerError {
                message: format!(
                    "addon-backed package '{}' cdylib not found: looked for lib{}.{{so,dylib,dll}} under '{}' (did you run 'taida install'?)",
                    import_path,
                    manifest.library,
                    pkg_dir.display()
                ),
            })?;

        let cdylib_abs = cdylib_path
            .canonicalize()
            .unwrap_or_else(|_| cdylib_path.clone());
        let cdylib_str = cdylib_abs
            .to_str()
            .ok_or_else(|| LowerError {
                message: format!(
                    "addon-backed package '{}' cdylib path is not valid UTF-8: {}",
                    import_path,
                    cdylib_abs.display()
                ),
            })?
            .to_string();

        // RC2.5 Phase 2: optionally load the Taida-side facade at
        // `<pkg_dir>/taida/<stem>.td` where `<stem>` is the final
        // `/`-segment of the canonical package id (e.g. `terminal`
        // for `taida-lang/terminal`). The facade provides the
        // uppercase / pure-Taida user-facing surface; without it we
        // fall back to the raw manifest `[functions]` table.
        //
        // Facade semantics mirror `module_eval::load_addon_facade`:
        //   - `Name <= lowercase_addon_fn` → alias the addon sentinel
        //     under the new name (facade alias).
        //   - `Name <= <pack expr>` → pure-Taida facade value; we
        //     replay the assignment at the top of `_taida_main`.
        //   - `<<< @(...)` → collected as the facade export set; any
        //     imported symbol that is not exported falls through to
        //     the `[functions]` lookup.
        //
        // C25B-030 Phase 1E-α + 1E-β: the facade loader now accepts
        // `>>>` relative imports (1E-α) and FuncDef statements (1E-β)
        // in addition to the RC2.5 v1 alias + pack-literal surface.
        // The only remaining restrictions are TypeDef / EnumDef /
        // MoldDef (tracked for 1E-γ) and non-relative `>>>` paths.
        let facade = Self::load_addon_facade_for_lower(pkg_dir, &manifest, import_path)?;

        // C25B-030 Phase 1E-β: register every facade FuncDef (public
        // + private helpers) with a mangled link symbol so sibling
        // facade functions can call each other and so user imports
        // of a public facade FuncDef resolve to the mangled symbol.
        // The mangle includes a package-id hash to avoid collisions
        // when two addons ship a FuncDef of the same name.
        //
        // Dedup: if the same addon is imported twice (two `>>>`
        // statements referencing the same package id) we still only
        // collect each FuncDef once — `addon_facade_mangled` is a
        // set of already-registered mangled symbols.
        if let Some(facade_summary) = &facade {
            let pkg_hash = simple_hash(&manifest.package);
            for (fn_local_name, fn_def) in &facade_summary.facade_funcs {
                let mangled = format!("_taida_fn_facade_{:016x}_{}", pkg_hash, fn_local_name);
                if !self.addon_facade_mangled.insert(mangled.clone()) {
                    // Already registered via an earlier import
                    // statement pointing at the same facade. Skip —
                    // we keep the first registration's FuncDef AST.
                    continue;
                }
                // Track the FuncDef so it is lowered in the 2nd
                // pass of lower_program.
                self.addon_facade_funcs.push((
                    fn_local_name.clone(),
                    fn_def.clone(),
                    mangled.clone(),
                ));
                // Make sibling / cross-facade calls resolve:
                // - Public names are overwritten by the user import
                //   binding below (alias support).
                // - Private helpers (`_`-prefixed) become reachable
                //   under their raw name throughout the current
                //   lowering run.
                self.user_funcs.insert(fn_local_name.clone());
                self.imported_func_links
                    .insert(fn_local_name.clone(), mangled.clone());
                // Track arity / return-type tags for downstream
                // inference (same as the main module's FuncDef 1st
                // pass).
                self.register_facade_func_signature(fn_local_name, fn_def);
            }
            // C25B-030 Phase 1E-β-2 / 1E-β-3-2: pre-register every
            // facade pack / value binding (both private `_`-prefixed
            // helpers and public packs like `LineEditorState` /
            // `CompletionState` / `LineEditorAction`) under its raw
            // name. User imports of a public pack still go through
            // the per-symbol loop below (which honours aliasing);
            // pre-registering here in addition ensures that a
            // harvested facade FuncDef body — pulled in transitively
            // via `facade_expand_reachable_symbols` — can still
            // resolve a reference to the public pack even when the
            // user only imported the function (e.g. the user wrote
            // `>>> taida-lang/terminal => @(LineEditorNew)` and the
            // body of `LineEditorNew` references the public
            // `LineEditorState` pack without the user ever naming
            // it).
            //
            // Dedup semantics:
            //   - If the user imports `LineEditorState` without
            //     alias, the per-symbol loop will use the same
            //     `alias == orig_name == local_name`, so the two
            //     markers collide and the per-symbol loop's
            //     `addon_facade_mangled.insert(marker)` returns
            //     false — the pre-reg wins with identical content.
            //   - If the user aliases `LineEditorState as MyState`,
            //     the per-symbol loop builds a different marker
            //     (`...value_{hash}_MyState`) and its registration
            //     runs side-by-side with the raw-name pre-reg.
            //     The facade FuncDef body's raw-name reference
            //     still resolves; the user's main body sees the
            //     alias.
            //
            // `addon_facade_mangled` doubles as a deterministic
            // dedup set — we prefix the value-binding marker with
            // a distinct `facade_value::` namespace to avoid
            // colliding with FuncDef mangles.
            for (local_name, value_expr) in &facade_summary.pack_bindings {
                let marker = format!("_taida_facade_value_{:016x}_{}", pkg_hash, local_name);
                if !self.addon_facade_mangled.insert(marker) {
                    continue;
                }
                self.addon_facade_pack_bindings
                    .push((local_name.clone(), value_expr.clone()));
                self.top_level_vars.insert(local_name.clone());
                // Narrowed flagging mirrors the per-symbol loop
                // below: only real `@(...)` packs become
                // `pack_vars`; scalar / list / arithmetic
                // bindings take the appropriate primitive tag.
                if matches!(value_expr, Expr::BuchiPack(_, _)) {
                    self.pack_vars.insert(local_name.clone());
                } else if matches!(value_expr, Expr::ListLit(_, _)) {
                    self.list_vars.insert(local_name.clone());
                } else if matches!(value_expr, Expr::IntLit(_, _)) {
                    self.int_vars.insert(local_name.clone());
                } else if matches!(value_expr, Expr::FloatLit(_, _)) {
                    self.float_vars.insert(local_name.clone());
                } else if matches!(value_expr, Expr::StringLit(_, _) | Expr::TemplateLit(_, _)) {
                    self.string_vars.insert(local_name.clone());
                } else if matches!(value_expr, Expr::BoolLit(_, _)) {
                    self.bool_vars.insert(local_name.clone());
                }
            }
            // Private aliases (facade author wrote
            // `_MyAlias <= terminalSize` as an internal rename).
            // Rare in practice but cheap to support. The alias
            // resolves to the manifest function's arity just like
            // public aliases.
            for (local_name, target_fn) in &facade_summary.aliases {
                if !local_name.starts_with('_') {
                    continue;
                }
                let marker = format!("_taida_facade_alias_{:016x}_{}", pkg_hash, local_name);
                if !self.addon_facade_mangled.insert(marker) {
                    continue;
                }
                if let Some(arity) = manifest.functions.get(target_fn) {
                    self.addon_func_refs.insert(
                        local_name.clone(),
                        AddonFuncRef {
                            package_id: manifest.package.clone(),
                            cdylib_path: cdylib_str.clone(),
                            function_name: target_fn.clone(),
                            arity: *arity,
                        },
                    );
                    self.user_funcs.insert(local_name.clone());
                }
            }
        }

        for sym in &import_stmt.symbols {
            let orig_name = sym.name.clone();
            let alias = sym.alias.clone().unwrap_or_else(|| sym.name.clone());

            // Lookup order (must match interpreter
            // `module_eval::try_eval_addon_import`):
            //   1. facade exports (uppercase / pure-Taida surface)
            //   2. manifest `[functions]` entries (raw addon API)
            if let Some(facade) = &facade
                && facade.exports.contains(&orig_name)
            {
                if let Some(target_fn) = facade.aliases.get(&orig_name) {
                    // Facade alias: look the function up in the
                    // manifest to recover its arity, then register
                    // the new alias under `addon_func_refs`.
                    let arity = manifest
                        .functions
                        .get(target_fn)
                        .ok_or_else(|| LowerError {
                            message: format!(
                                "addon facade for '{}' aliases '{}' to unknown function '{}'",
                                import_path, orig_name, target_fn
                            ),
                        })?;
                    self.addon_func_refs.insert(
                        alias.clone(),
                        AddonFuncRef {
                            package_id: manifest.package.clone(),
                            cdylib_path: cdylib_str.clone(),
                            function_name: target_fn.clone(),
                            arity: *arity,
                        },
                    );
                    self.user_funcs.insert(alias.clone());
                    // RC2.5 Phase 4 (RC2.5B-008): mirror the raw-import
                    // return-type tracking. Facade aliases point at a
                    // manifest function whose return type we consult via
                    // `addon_known_return_tag`. See the non-facade path
                    // below for the rationale.
                    if let Some(return_tag) =
                        Self::addon_known_return_tag(&manifest.package, target_fn)
                    {
                        match return_tag {
                            "Bool" => {
                                self.bool_returning_funcs.insert(alias.clone());
                            }
                            "Str" => {
                                self.string_returning_funcs.insert(alias.clone());
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                if let Some(value_expr) = facade.pack_bindings.get(&orig_name) {
                    // Pure-Taida facade value. Record the binding so
                    // the 3rd pass can replay it in `_taida_main`
                    // before user statements execute.
                    //
                    // Phase 1E-α treated every `pack_bindings` entry
                    // as a pack literal. Phase 1E-β widened the
                    // accepted RHS so we now distinguish based on
                    // the actual Expr shape — only real
                    // `Expr::BuchiPack` bindings get the `pack_vars`
                    // / `top_level_vars` flags so downstream
                    // field-access semantics stay honest for
                    // scalar / list / arithmetic bindings.
                    self.addon_facade_pack_bindings
                        .push((alias.clone(), value_expr.clone()));
                    self.top_level_vars.insert(alias.clone());
                    if matches!(value_expr, Expr::BuchiPack(_, _)) {
                        self.pack_vars.insert(alias.clone());
                    } else if matches!(value_expr, Expr::ListLit(_, _)) {
                        self.list_vars.insert(alias.clone());
                    }
                    // Track primitive scalar tags so downstream
                    // type inference (arithmetic, string
                    // interpolation) picks the right class.
                    if matches!(value_expr, Expr::IntLit(_, _)) {
                        self.int_vars.insert(alias.clone());
                    } else if matches!(value_expr, Expr::FloatLit(_, _)) {
                        self.float_vars.insert(alias.clone());
                    } else if matches!(value_expr, Expr::StringLit(_, _) | Expr::TemplateLit(_, _))
                    {
                        self.string_vars.insert(alias.clone());
                    } else if matches!(value_expr, Expr::BoolLit(_, _)) {
                        self.bool_vars.insert(alias.clone());
                    }
                    continue;
                }
                if let Some(fn_def) = facade.facade_funcs.get(&orig_name) {
                    // C25B-030 Phase 1E-β: facade FuncDef.
                    //
                    // The FuncDef was already harvested into
                    // `self.addon_facade_funcs` (with a mangled
                    // link symbol) in the block above this per-
                    // symbol loop. Here we only bind the user-
                    // facing alias to that mangled symbol so call
                    // sites `alias(...)` resolve through the
                    // normal user-function path.
                    let pkg_hash = simple_hash(&manifest.package);
                    let mangled = format!("_taida_fn_facade_{:016x}_{}", pkg_hash, orig_name);
                    self.user_funcs.insert(alias.clone());
                    self.imported_func_links.insert(alias.clone(), mangled);
                    // Re-register signature metadata under the
                    // alias name so the type-inference paths
                    // (string_returning_funcs, pack_returning_funcs,
                    // bool_returning_funcs, func_param_defs) all
                    // agree with the aliased call site.
                    self.register_facade_func_signature(&alias, fn_def);
                    continue;
                }
                // Exported by the facade but none of the known
                // forms matched. This should be unreachable given
                // the facade loader's invariants (`exports` is
                // always a subset of aliases | pack_bindings |
                // facade_funcs) but we still emit a defensive
                // compile error so future loader bugs do not leak
                // into silent divergence.
                return Err(LowerError {
                    message: format!(
                        "addon facade for '{}' exports '{}' via an unknown binding form. \
                         This is a facade-loader invariant violation; please file a bug \
                         with the facade contents reproducing the issue.",
                        import_path, orig_name
                    ),
                });
            }

            let arity = manifest
                .functions
                .get(&orig_name)
                .ok_or_else(|| LowerError {
                    message: format!(
                        "Symbol '{}' not found in addon-backed package '{}'",
                        orig_name, import_path
                    ),
                })?;

            self.addon_func_refs.insert(
                alias.clone(),
                AddonFuncRef {
                    package_id: manifest.package.clone(),
                    cdylib_path: cdylib_str.clone(),
                    function_name: orig_name.clone(),
                    arity: *arity,
                },
            );
            // Also register the alias as a user function so that any
            // downstream `name` lookup outside the dedicated addon
            // dispatch branch finds it. The addon dispatch branch
            // still runs first and short-circuits normal lowering.
            self.user_funcs.insert(alias.clone());
            // RC2.5 Phase 4 (RC2.5B-008): track the addon function's
            // return type for the native backend's `convert_to_string`
            // / `expr_is_bool` / `expr_is_string_full` hints. The ABI
            // v1 manifest schema is frozen (RC1 F-1) and cannot carry
            // return-type annotations, so we consult a v1-scoped lookup
            // table keyed on `(package_id, function_name)`.
            //
            // Without this, `isTty <= termIsTty()` followed by
            // `stdout(\`${isTty}\`)` would render as the raw i64 "0"
            // on native (because the template lit's `convert_to_string`
            // defaults to `taida_polymorphic_to_string`) while the
            // interpreter renders it as "false" — a real
            // backend-parity gap surfaced by RC2.5-4b.
            if let Some(return_tag) = Self::addon_known_return_tag(&manifest.package, &orig_name) {
                match return_tag {
                    "Bool" => {
                        self.bool_returning_funcs.insert(alias.clone());
                    }
                    "Str" => {
                        self.string_returning_funcs.insert(alias.clone());
                    }
                    "Pack" => {
                        // Pack return values flow through `PackGet`
                        // lookup for field access; no stringification
                        // hint needed here because users unpack the
                        // fields before interpolating.
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// RC2.5 Phase 4 (RC2.5B-008): hardcoded return-type table for the
    /// v1-scoped addon functions whose stringification must match the
    /// interpreter byte-for-byte. The ABI v1 manifest (`addon.toml`)
    /// only carries `name = arity`, so return types live here as a
    /// per-package lookup table. RC3+ will consider a manifest schema
    /// extension or dynamic facade-based lookup; for now the table
    /// enumerates both the production `taida-lang/terminal` surface
    /// (external package at `../terminal`) **and** the workspace sample
    /// crate (`crates/addon-terminal-sample`) which unfortunately
    /// declares the same `taida-lang/terminal` package id. The two
    /// surfaces do not overlap, so a superset entry is safe.
    ///
    /// Package id collision (`taida-lang/terminal` declared by both
    /// the production external repo and the in-tree sample crate) is
    /// tracked as tech debt in `.dev/RC2_6_BLOCKERS.md::RC2.6B-015`
    /// and should be resolved in RC3+ by renaming the sample crate's
    /// package id to something like `taida-lang/addon-rs-sample`.
    ///
    /// Returns the Taida type name (`"Bool"`, `"Str"`, `"Pack"`, ...)
    /// or `None` if the function's return type is unknown.
    pub(super) fn addon_known_return_tag(
        package_id: &str,
        function_name: &str,
    ) -> Option<&'static str> {
        match (package_id, function_name) {
            // Production `taida-lang/terminal` external package v1
            // surface (`../terminal/src/{size,key}.rs`). Both functions
            // return a Pack:
            //   terminalSize → @(cols: Int, rows: Int)
            //   readKey      → @(kind: Int, text: Str, ctrl: Bool, alt: Bool, shift: Bool)
            ("taida-lang/terminal", "terminalSize") => Some("Pack"),
            ("taida-lang/terminal", "readKey") => Some("Pack"),

            // Workspace sample crate `crates/addon-terminal-sample`
            // which also declares `package = "taida-lang/terminal"`
            // (package id collision, see RC2.6B-015). Kept so the
            // sample's install E2E test
            // (`tests/addon_terminal_install_e2e.rs`) continues to
            // resolve return types correctly until the collision is
            // resolved.
            ("taida-lang/terminal", "termIsTty") => Some("Bool"),
            ("taida-lang/terminal", "termReadLine") => Some("Str"),
            ("taida-lang/terminal", "termSize") => Some("Pack"),
            // `termPrint` / `termPrintLn` return Unit; no hint needed
            // because their results are discarded at the statement
            // level in Taida source.
            _ => None,
        }
    }

    /// C25B-030 Phase 1E-β-2: walk a facade FuncDef body and collect
    /// every identifier name that appears as a free variable (i.e.
    /// not a function parameter, not shadowed by a local
    /// assignment, and not bound inside a nested lambda).
    ///
    /// Used by [`Self::facade_expand_reachable_symbols`] to grow
    /// the summary's export set transitively — a facade FuncDef
    /// that references `_bufferNewInner` pulls the private helper
    /// into the summary even if the user import did not name it.
    ///
    /// Lighter-weight than `collect_free_vars_inner`: it does NOT
    /// filter against `user_funcs` / `top_level_vars` (those are
    /// global to a lowering run, not facade-local) and it emits
    /// every free identifier regardless of whether it resolves to
    /// a function, a value, or a runtime builtin. The caller
    /// intersects the result with the facade's own local symbol
    /// set to pick out only the names that actually need
    /// harvesting.
    fn facade_collect_refs_in_body(
        body: &[crate::parser::Statement],
        param_names: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        let mut bound: std::collections::HashSet<String> = param_names.clone();
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                bound.insert(assign.target.clone());
            }
        }
        for stmt in body {
            Self::facade_collect_refs_in_stmt(stmt, &bound, out);
        }
    }

    fn facade_collect_refs_in_stmt(
        stmt: &crate::parser::Statement,
        bound: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        match stmt {
            Statement::Expr(expr) => Self::facade_collect_refs_in_expr(expr, bound, out),
            Statement::Assignment(assign) => {
                Self::facade_collect_refs_in_expr(&assign.value, bound, out);
            }
            Statement::UnmoldForward(uf) => {
                Self::facade_collect_refs_in_expr(&uf.source, bound, out);
            }
            Statement::UnmoldBackward(ub) => {
                Self::facade_collect_refs_in_expr(&ub.source, bound, out);
            }
            Statement::ErrorCeiling(ec) => {
                for inner in &ec.handler_body {
                    Self::facade_collect_refs_in_stmt(inner, bound, out);
                }
            }
            _ => {}
        }
    }

    fn facade_collect_refs_in_expr(
        expr: &crate::parser::Expr,
        bound: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, _) => {
                if !bound.contains(name) {
                    out.insert(name.clone());
                }
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                Self::facade_collect_refs_in_expr(lhs, bound, out);
                Self::facade_collect_refs_in_expr(rhs, bound, out);
            }
            Expr::UnaryOp(_, operand, _) => {
                Self::facade_collect_refs_in_expr(operand, bound, out);
            }
            Expr::FuncCall(callee, args, _) => {
                Self::facade_collect_refs_in_expr(callee, bound, out);
                for a in args {
                    Self::facade_collect_refs_in_expr(a, bound, out);
                }
            }
            Expr::FieldAccess(obj, _, _) => {
                Self::facade_collect_refs_in_expr(obj, bound, out);
            }
            Expr::MethodCall(obj, _, args, _) => {
                Self::facade_collect_refs_in_expr(obj, bound, out);
                for a in args {
                    Self::facade_collect_refs_in_expr(a, bound, out);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    Self::facade_collect_refs_in_expr(e, bound, out);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        Self::facade_collect_refs_in_expr(cond, bound, out);
                    }
                    for s in &arm.body {
                        Self::facade_collect_refs_in_stmt(s, bound, out);
                    }
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for f in fields {
                    Self::facade_collect_refs_in_expr(&f.value, bound, out);
                }
            }
            Expr::ListLit(items, _) => {
                for i in items {
                    Self::facade_collect_refs_in_expr(i, bound, out);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for a in args {
                    Self::facade_collect_refs_in_expr(a, bound, out);
                }
                for f in fields {
                    Self::facade_collect_refs_in_expr(&f.value, bound, out);
                }
            }
            Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                Self::facade_collect_refs_in_expr(inner, bound, out);
            }
            Expr::Lambda(params, body, _) => {
                let mut inner_bound = bound.clone();
                for p in params {
                    inner_bound.insert(p.name.clone());
                }
                Self::facade_collect_refs_in_expr(body, &inner_bound, out);
            }
            _ => {}
        }
    }

    /// C25B-030 Phase 1E-β-2: grow the facade summary so every
    /// private helper transitively referenced by an already-
    /// exported binding is also carried through.
    ///
    /// We repeatedly scan the bodies of already-harvested
    /// FuncDefs and the expressions of already-harvested
    /// pack/value bindings for identifiers that match names in
    /// the `all_local_*` maps (the per-facade universe of
    /// definitions, which is larger than `summary.*` because it
    /// also contains the private `_`-prefixed helpers the first
    /// merge pass skipped). When a match is found we promote
    /// that binding into the summary and requeue its dependencies.
    /// The loop terminates because each iteration either adds at
    /// least one new symbol or does nothing.
    fn facade_expand_reachable_symbols(
        summary: &mut AddonFacadeSummary,
        all_local_funcs: &std::collections::HashMap<String, crate::parser::FuncDef>,
        all_local_packs: &std::collections::HashMap<String, crate::parser::Expr>,
        all_local_aliases: &std::collections::HashMap<String, String>,
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            let mut refs: std::collections::HashSet<String> = std::collections::HashSet::new();
            for fn_def in summary.facade_funcs.values() {
                let param_names: std::collections::HashSet<String> =
                    fn_def.params.iter().map(|p| p.name.clone()).collect();
                Self::facade_collect_refs_in_body(&fn_def.body, &param_names, &mut refs);
            }
            let empty_params: std::collections::HashSet<String> = std::collections::HashSet::new();
            for expr in summary.pack_bindings.values() {
                Self::facade_collect_refs_in_expr(expr, &empty_params, &mut refs);
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

    /// C25B-030 Phase 1E-β: register the arity, parameter defs, and
    /// return-type inference hints for a facade-declared FuncDef
    /// under `local_name`.
    ///
    /// `local_name` is the binding the caller is registering the
    /// signature under: during the facade-wide registration pass
    /// this is the facade's raw FuncDef name (e.g. `ClearScreen`);
    /// during the per-symbol user-import loop this is the user's
    /// alias (e.g. `MyClear` from `>>> ... => @(ClearScreen: MyClear)`).
    ///
    /// Mirrors the logic in `lower_program`'s 1st pass for ordinary
    /// FuncDefs (see `stmt.rs`), but applied to facade FuncDefs
    /// which do not live in the main program's AST. Only the type-
    /// inference hints that actually affect downstream lowering
    /// (string / bool / int / float return classes, pack/list
    /// returns) are replicated here — the body-based fallbacks like
    /// the TCO detection are recomputed in `lower_func_def` when
    /// the FuncDef is actually lowered.
    pub(super) fn register_facade_func_signature(
        &mut self,
        local_name: &str,
        fn_def: &crate::parser::FuncDef,
    ) {
        self.func_param_defs
            .insert(local_name.to_string(), fn_def.params.clone());
        if let Some(ref rt) = fn_def.return_type {
            match rt {
                crate::parser::TypeExpr::Named(n) if n == "Str" => {
                    self.string_returning_funcs.insert(local_name.to_string());
                }
                crate::parser::TypeExpr::Named(n) if n == "Bool" => {
                    self.bool_returning_funcs.insert(local_name.to_string());
                }
                crate::parser::TypeExpr::Named(n) if n == "Float" => {
                    self.float_returning_funcs.insert(local_name.to_string());
                }
                crate::parser::TypeExpr::Named(n) if n == "Int" || n == "Num" => {
                    self.int_returning_funcs.insert(local_name.to_string());
                }
                crate::parser::TypeExpr::List(_) => {
                    self.list_returning_funcs.insert(local_name.to_string());
                }
                _ => {}
            }
        }
        // Body-based inference (F-58/F-60 / C12-11 equivalents) —
        // the minimum we need so `BufferNew` / `_bufferNewInner`
        // style helpers that build a pack literal get tagged as
        // pack-returning. Kept narrow on purpose; the richer
        // heuristics fire inside `lower_func_def`.
        if Self::func_body_returns_pack(&fn_def.body) {
            self.pack_returning_funcs.insert(local_name.to_string());
        }
        if Self::func_body_returns_list(&fn_def.body) {
            self.list_returning_funcs.insert(local_name.to_string());
        }
    }

    /// RC2.5 Phase 2 / C25B-030 Phase 1E-α + 1E-β: parse the optional
    /// Taida-side facade for an addon-backed package, if one exists
    /// at `<pkg_dir>/taida/<stem>.td`.
    ///
    /// Returns `Ok(None)` if no facade file is present (lowercase-only
    /// addons work without a facade). Returns a populated
    /// [`AddonFacadeSummary`] when a facade exists.
    ///
    /// Supported top-level constructs:
    ///
    /// - `Name <= lowercaseFn` — alias of a manifest [functions]
    ///   entry, possibly reached through a child facade's addon alias
    ///   (chained aliasing)
    /// - `Name <= @(...)` — pure-Taida pack literal binding
    /// - `>>> ./X.td => @(syms...)` — facade-internal relative import
    ///   (C25B-030 Phase 1E-α). The referenced file is recursively
    ///   parsed using the same rules, and only the requested symbols
    ///   (or all its exports when the import symbol list is empty)
    ///   are merged into the parent summary.
    /// - `Name args = body => :Type` — function definition
    ///   (C25B-030 Phase 1E-β). Harvested into
    ///   [`AddonFacadeSummary::facade_funcs`] and later lowered as
    ///   IR functions under a mangled link symbol derived from the
    ///   addon package id, so user imports of facade functions
    ///   dispatch via the normal user-function call path.
    /// - `<<< @(...)` — single explicit export clause
    ///
    /// Still unsupported (Phase 1E-γ follow-up):
    ///
    /// - TypeDef / EnumDef / MoldDef statements inside a facade.
    /// - Non-relative `>>>` paths (`>>> taida-lang/foo`, `>>> npm:*`).
    /// - `<<< <path>` re-export.
    pub(super) fn load_addon_facade_for_lower(
        pkg_dir: &std::path::Path,
        manifest: &crate::addon::manifest::AddonManifest,
        import_path: &str,
    ) -> Result<Option<AddonFacadeSummary>, LowerError> {
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
        let mut visiting: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        // C25B-030 Phase 1E-β-2: track EVERY local binding across
        // the facade file tree (both public and private), so the
        // reachability expansion below can promote private
        // `_`-prefixed helpers into the summary when they are
        // transitively referenced by an exported FuncDef / pack.
        let mut universe_funcs: std::collections::HashMap<String, crate::parser::FuncDef> =
            std::collections::HashMap::new();
        let mut universe_packs: std::collections::HashMap<String, crate::parser::Expr> =
            std::collections::HashMap::new();
        let mut universe_aliases: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        Self::load_addon_facade_file(
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
        // This matches the facade behaviour of "export everything that
        // reached the top level". Child facades reached through
        // `>>> ./X.td => @(syms)` always restrict via the requested
        // symbol list regardless of the child's own `<<<` clause.
        //
        // Private FuncDef helpers (names starting with `_`) are
        // never surfaced here because the file-merge logic in
        // `load_addon_facade_file` only forwards `use_set` members
        // up the chain. Authors who want to hide an underscore-
        // prefixed symbol should drop it from the `<<<` clause;
        // it will still get promoted by `facade_expand_reachable_
        // symbols` below if an exported binding needs it.
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

        // C25B-030 Phase 1E-β-2: transitively pull in private
        // helpers referenced by already-harvested bindings.
        // Without this, `BufferNew` would build fine at the user
        // call site but its body's `_bufferNewInner(...)` call
        // would resolve to nothing because the private helper
        // was filtered out by the initial `use_set` merge.
        Self::facade_expand_reachable_symbols(
            &mut summary,
            &universe_funcs,
            &universe_packs,
            &universe_aliases,
        );

        Ok(Some(summary))
    }

    /// C25B-030 Phase 1E-α: recursive facade file loader shared
    /// between the top-level addon facade and its `>>> ./X.td`
    /// relative children.
    ///
    /// Arguments:
    ///
    /// - `facade_path`: absolute path of the facade file to load.
    /// - `manifest`: addon manifest used to resolve aliases into the
    ///   `[functions]` table. Shared across the recursion so child
    ///   facade files can still alias lowercase sentinels.
    /// - `import_path`: the user-visible package id used in error
    ///   messages (e.g. `taida-lang/terminal`).
    /// - `restrict_to`: when `Some(set)`, only symbols listed in the
    ///   set are merged into `out_summary`. Used by `>>> ./X.td
    ///   => @(a, b)` to import a subset of the child's exports.
    ///   `None` means "merge everything we understand from this
    ///   facade" (the top-level call path).
    /// - `out_summary`: accumulator that collects aliases / pack
    ///   bindings / exports.
    /// - `visiting`: set of facade paths currently on the recursion
    ///   stack, used to detect circular `>>>` chains.
    /// - `universe_funcs` / `universe_packs` / `universe_aliases`:
    ///   C25B-030 Phase 1E-β-2 universe maps that record EVERY
    ///   local binding seen across the entire facade file tree
    ///   (both public and private). The caller of
    ///   `load_addon_facade_for_lower` runs
    ///   [`Self::facade_expand_reachable_symbols`] after this
    ///   function returns to promote private `_`-prefixed helpers
    ///   into `out_summary` when an already-harvested FuncDef /
    ///   pack expression references them. On a duplicate local
    ///   name across sibling files the first definition wins;
    ///   duplicates within a single file are still rejected by
    ///   the per-file first-pass check.
    #[allow(clippy::too_many_arguments)]
    fn load_addon_facade_file(
        facade_path: &std::path::Path,
        manifest: &crate::addon::manifest::AddonManifest,
        import_path: &str,
        restrict_to: Option<&std::collections::HashSet<String>>,
        out_summary: &mut AddonFacadeSummary,
        visiting: &mut std::collections::HashSet<std::path::PathBuf>,
        universe_funcs: &mut std::collections::HashMap<String, crate::parser::FuncDef>,
        universe_packs: &mut std::collections::HashMap<String, crate::parser::Expr>,
        universe_aliases: &mut std::collections::HashMap<String, String>,
    ) -> Result<(), LowerError> {
        let canonical = facade_path
            .canonicalize()
            .unwrap_or_else(|_| facade_path.to_path_buf());
        if !visiting.insert(canonical.clone()) {
            return Err(LowerError {
                message: format!(
                    "circular facade import detected while loading addon facade chain for '{}' \
                     at '{}'",
                    import_path,
                    facade_path.display()
                ),
            });
        }

        let source = std::fs::read_to_string(facade_path).map_err(|e| LowerError {
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
            return Err(LowerError {
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

        // First pass: harvest this file's own bindings (aliases /
        // pack literals / FuncDefs) into a per-file staging summary.
        // We then merge only the requested subset into `out_summary`
        // once the entire file is scanned (matches the `<<<`
        // semantics: exports are authoritative for the module, not
        // the order of assignments).
        let mut local_aliases: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut local_packs: std::collections::HashMap<String, Expr> =
            std::collections::HashMap::new();
        let mut local_funcs: std::collections::HashMap<String, FuncDef> =
            std::collections::HashMap::new();
        let mut local_exports: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Pending child imports discovered in this file. We resolve
        // them after the first pass so chained aliasing (this file
        // binds `Name <= otherChildName`) can look up the child's
        // exports even if the `>>>` appears later in source order.
        // For 1E-α the loop is single-pass because none of the
        // observed facades rely on forward references, but the
        // design keeps the door open for 1E-β.
        let mut child_imports: Vec<(std::path::PathBuf, std::collections::HashSet<String>)> =
            Vec::new();

        for stmt in &program.statements {
            match stmt {
                Statement::Assignment(assign) => match &assign.value {
                    // `Name <= Ident(B)` → alias if B is a known addon fn
                    Expr::Ident(target_fn, _) => {
                        if manifest.functions.contains_key(target_fn) {
                            local_aliases.insert(assign.target.clone(), target_fn.clone());
                        } else {
                            visiting.remove(&canonical);
                            return Err(LowerError {
                                message: format!(
                                    "addon facade '{}' aliases '{}' to '{}' which is not listed \
                                     in [functions] of '{}'. Chained facade aliasing across pure-Taida \
                                     helpers is not yet supported (C25B-030 Phase 1E-γ).",
                                    facade_path.display(),
                                    assign.target,
                                    target_fn,
                                    import_path
                                ),
                            });
                        }
                    }
                    // C25B-030 Phase 1E-β: widened to accept any pure-Taida
                    // value literal on the RHS. Pack literals (`@(...)`)
                    // are tracked separately because user code reaches
                    // their fields via the pack-get path; scalar / string
                    // / arithmetic bindings become generic top-level
                    // values replayed into `_taida_main` just like pack
                    // bindings, but without the `pack_vars` flag.
                    //
                    // Supported RHS shapes on the AST level:
                    //
                    //   - `@(...)` pack literal
                    //   - `IntLit` / `FloatLit` / `StringLit` / `BoolLit`
                    //   - `TemplateLit` (simple interpolation — usage
                    //     inside real addon facades is limited to cached
                    //     ANSI escapes)
                    //   - `ListLit` (pure list literal)
                    //   - `BinaryOp` / `UnaryOp` / `FuncCall` — evaluated
                    //     in main_fn context; any reference to
                    //     facade-local symbols still resolves through
                    //     the main lowering's name tables because facade
                    //     bindings are replayed before user code.
                    //
                    // Non-pack value bindings are collected in
                    // `local_packs` so the merging / exporting /
                    // `pack_bindings` plumbing stays uniform. The
                    // `pack_vars` / `top_level_vars` flagging is
                    // narrowed in `lower_addon_import` based on the
                    // RHS shape to keep field-access semantics honest.
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
                    // Anything else is out of scope for 1E-β.
                    _ => {
                        visiting.remove(&canonical);
                        return Err(LowerError {
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
                    // C25B-030 Phase 1E-α: support facade-internal
                    // relative imports only. Non-relative paths
                    // (package imports, `taida-lang/*`, `npm:*`,
                    // ...) are out of scope for the current phase.
                    let p = &import_stmt.path;
                    if !(p.starts_with("./") || p.starts_with("../")) {
                        visiting.remove(&canonical);
                        return Err(LowerError {
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
                        return Err(LowerError {
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
                        .ok_or_else(|| LowerError {
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
                        return Err(LowerError {
                            message: format!(
                                "addon facade '{}' imports '{}' which resolves to '{}' but the \
                                 file does not exist.",
                                facade_path.display(),
                                p,
                                child_path.display()
                            ),
                        });
                    }
                    let requested: std::collections::HashSet<String> =
                        if import_stmt.symbols.is_empty() {
                            // No symbol list: will import "all the child
                            // facade's exports". We resolve "all" lazily
                            // below by passing `restrict_to = None` so
                            // the recursive loader merges every binding.
                            std::collections::HashSet::new()
                        } else {
                            import_stmt
                                .symbols
                                .iter()
                                .map(|s| {
                                    // C25B-030 Phase 1E-α: facade aliasing
                                    // on the import side (`>>> ./x.td =>
                                    // @(Foo as Bar)`) is not supported.
                                    // The parent facade should instead
                                    // re-export using its own `<<<`.
                                    s.name.clone()
                                })
                                .collect()
                        };
                    child_imports.push((child_path, requested));
                }
                Statement::Export(export_stmt) => {
                    if export_stmt.path.is_some() {
                        visiting.remove(&canonical);
                        return Err(LowerError {
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
                // C25B-030 Phase 1E-β: function definitions are
                // now harvested into `local_funcs`. Each FuncDef is
                // lowered later as an IR function under a mangled
                // link symbol derived from the addon package id
                // (see `lower_addon_import` → `addon_facade_funcs`
                // drain in stmt.rs 2nd pass). Both exported names
                // and facade-private helpers (names starting with
                // `_`) are collected so internal calls between
                // facade functions resolve correctly.
                Statement::FuncDef(fd) => {
                    // Defensive: duplicate FuncDef within one file
                    // is a facade authoring bug. The interpreter
                    // would silently last-write-wins; for codegen
                    // we surface a deterministic error so authors
                    // notice the shadowing early.
                    if local_funcs.contains_key(&fd.name) || local_packs.contains_key(&fd.name) {
                        visiting.remove(&canonical);
                        return Err(LowerError {
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
                // Phase 1E-γ follow-up: TypeDef / EnumDef / MoldDef
                // statements inside a facade file. The real
                // `taida-lang/terminal` facade does not need these
                // (every nested schema is expressed as a pure-Taida
                // pack literal); if a future addon needs them we
                // track the work in C25B-030 Phase 1E-γ.
                Statement::TypeDef(td) => {
                    visiting.remove(&canonical);
                    return Err(LowerError {
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
                    return Err(LowerError {
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
                    return Err(LowerError {
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
                    return Err(LowerError {
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

        // Recursively load child facades, merging only the requested
        // symbols into `out_summary`.
        for (child_path, requested) in child_imports {
            let child_restrict = if requested.is_empty() {
                None
            } else {
                Some(&requested)
            };
            Self::load_addon_facade_file(
                &child_path,
                manifest,
                import_path,
                child_restrict,
                out_summary,
                visiting,
                universe_funcs,
                universe_packs,
                universe_aliases,
            )?;
            // When a child exposed symbols with no explicit listing,
            // treat the merged set as "any name that the child is
            // willing to publish". The child itself has already
            // restricted itself (if requested_to was Some).
        }

        // Decide which local bindings from this file we will export.
        // Precedence:
        //   - `restrict_to = Some(set)`: only symbols in `set` — this
        //     corresponds to the parent's `>>> ./X.td => @(a, b)`.
        //   - `restrict_to = None` + local `<<<` present: the <<<
        //     clause is authoritative for this file.
        //   - `restrict_to = None` + no local `<<<`: all local
        //     bindings are eligible.
        //
        // Facade-private helper FuncDefs (names starting with `_`)
        // are ALWAYS collected regardless of the export set, so that
        // an exported FuncDef can still call its private helpers
        // after lowering. Only their visibility to user code is
        // gated by `use_set`.
        let use_set: std::collections::HashSet<String> = if let Some(set) = restrict_to {
            set.clone()
        } else if !local_exports.is_empty() {
            local_exports.clone()
        } else {
            let mut s = std::collections::HashSet::new();
            s.extend(local_aliases.keys().cloned());
            s.extend(local_packs.keys().cloned());
            s.extend(local_funcs.keys().cloned());
            s
        };

        for (name, target) in &local_aliases {
            if use_set.contains(name) {
                out_summary.aliases.insert(name.clone(), target.clone());
            }
            // C25B-030 Phase 1E-β-2: universe record for
            // reachability expansion below (first-wins across
            // sibling files).
            universe_aliases
                .entry(name.clone())
                .or_insert_with(|| target.clone());
        }
        // Pack / value bindings — direct merge gated by
        // `use_set`; private `_`-prefixed helpers are pulled in
        // transitively by `facade_expand_reachable_symbols`
        // after the whole tree is loaded.
        for (name, expr) in &local_packs {
            if use_set.contains(name) {
                out_summary.pack_bindings.insert(name.clone(), expr.clone());
            }
            universe_packs
                .entry(name.clone())
                .or_insert_with(|| expr.clone());
        }
        // FuncDefs — same rule as pack bindings.
        for (name, fd) in &local_funcs {
            if use_set.contains(name) {
                out_summary.facade_funcs.insert(name.clone(), fd.clone());
            }
            universe_funcs
                .entry(name.clone())
                .or_insert_with(|| fd.clone());
        }
        // Child-facade bindings already added through recursive
        // calls above stay in `out_summary` regardless.

        // If this call-site is the top-level entry (restrict_to is
        // None), adopt the local `<<<` clause as the parent summary's
        // export surface. Internal callers pass restrict_to = Some and
        // rely on caller-driven export shaping.
        if restrict_to.is_none() && !local_exports.is_empty() {
            out_summary.exports.extend(local_exports.iter().cloned());
        }

        // If restrict_to came from a parent's `>>> ./X.td => @(a, b)`
        // request, make sure every requested symbol was actually
        // produced by this file or one of its child facades. Missing
        // symbols are a compile error (matches interpreter's
        // behaviour where an unresolved facade import yields a clean
        // name error).
        if let Some(set) = restrict_to {
            for name in set {
                let produced = out_summary.aliases.contains_key(name)
                    || out_summary.pack_bindings.contains_key(name)
                    || out_summary.facade_funcs.contains_key(name);
                if !produced {
                    visiting.remove(&canonical);
                    return Err(LowerError {
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

    /// RC2.5 Phase 2: emit the IR for a single addon function call.
    ///
    /// Used by both the regular `FuncCall` lowering path and the
    /// `MoldInst` lowering path (`Foo[]()` desugars to a call on an
    /// addon sentinel). The emitted IR is exactly the shape the
    /// C-side dispatcher (`taida_addon_call` in `native_runtime.c`)
    /// expects:
    ///
    /// ```text
    ///   taida_addon_call(
    ///     <const char*> package_id,    // .rodata
    ///     <const char*> cdylib_path,   // .rodata (absolute path)
    ///     <const char*> function_name, // .rodata
    ///     <i64>         argc,
    ///     <i64>         argv_pack)     // Taida Pack or 0 when argc == 0
    /// ```
    ///
    /// The argv pack is allocated fresh per call so the dispatcher can
    /// read positional arguments with their type tags (TAIDA_TAG_*).
    /// For `argc == 0` we pass the integer constant 0 instead of an
    /// empty pack, matching the C-side contract documented at the
    /// `taida_addon_call` implementation.
    pub(super) fn emit_addon_call(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        let addon_ref = self
            .addon_func_refs
            .get(name)
            .cloned()
            .expect("emit_addon_call invoked for non-addon name");
        if args.len() != addon_ref.arity as usize {
            return Err(LowerError {
                message: format!(
                    "addon function '{}' expects {} argument(s), got {}",
                    addon_ref.function_name,
                    addon_ref.arity,
                    args.len()
                ),
            });
        }

        // Lower argument expressions first so any inner error surfaces
        // before we allocate stack slots / const strings.
        //
        // Tag inference is best-effort: Str / Bool / Int are what the
        // RC2 v1 terminal surface actually exercises. Everything else
        // falls through to TAIDA_TAG_INT (0) and is treated as a raw
        // i64 payload by the C dispatcher.
        let mut arg_vars: Vec<IrVar> = Vec::with_capacity(args.len());
        let mut arg_tags: Vec<i64> = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.lower_expr(func, arg)?;
            let tag: i64 = if self.expr_is_string_full(arg) {
                3 // TAIDA_TAG_STR
            } else if self.expr_is_bool(arg) {
                2 // TAIDA_TAG_BOOL
            } else {
                0 // TAIDA_TAG_INT
            };
            arg_vars.push(v);
            arg_tags.push(tag);
        }

        // Emit the 3 static `.rodata` strings (package id, cdylib
        // absolute path, function name). ConstStr auto-deduplicates at
        // the emit layer through its global-data table.
        let pkg_var = func.alloc_var();
        func.push(IrInst::ConstStr(pkg_var, addon_ref.package_id.clone()));
        let cdylib_var = func.alloc_var();
        func.push(IrInst::ConstStr(cdylib_var, addon_ref.cdylib_path.clone()));
        let fn_name_var = func.alloc_var();
        func.push(IrInst::ConstStr(
            fn_name_var,
            addon_ref.function_name.clone(),
        ));

        // Build argv pack. For argc == 0 we pass 0 and short-circuit
        // the per-call allocation entirely.
        let argv_var = if arg_vars.is_empty() {
            let z = func.alloc_var();
            func.push(IrInst::ConstInt(z, 0));
            z
        } else {
            let p = func.alloc_var();
            func.push(IrInst::PackNew(p, arg_vars.len()));
            for (i, (av, tag)) in arg_vars.iter().zip(arg_tags.iter()).enumerate() {
                func.push(IrInst::PackSet(p, i, *av));
                func.push(IrInst::PackSetTag(p, i, *tag));
            }
            p
        };

        let argc_var = func.alloc_var();
        func.push(IrInst::ConstInt(argc_var, arg_vars.len() as i64));

        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_addon_call".to_string(),
            vec![pkg_var, cdylib_var, fn_name_var, argc_var, argv_var],
        ));
        // Track that this variable holds a pack-like value so
        // downstream `.field` access uses the pack lookup path.
        // (addon return values for terminal v1 are all Pack shapes.)
        Ok(result)
    }

    pub(super) fn resolve_import_path(
        &self,
        module_path: &str,
        version: Option<&str>,
    ) -> Option<std::path::PathBuf> {
        let source_dir = self.source_dir.as_ref()?;

        let path = if module_path.starts_with("./") || module_path.starts_with("../") {
            // Relative path
            source_dir.join(module_path)
        } else if std::path::Path::new(module_path).is_absolute() {
            // Absolute path
            std::path::PathBuf::from(module_path)
        } else if let Some(stripped) = module_path.strip_prefix("~/") {
            // RCB-103: Project root relative
            let root = Self::find_project_root(source_dir);
            root.join(stripped)
        } else {
            // RCB-103/RCB-213: Package import (e.g., "author/pkg" or "author/pkg/submodule")
            // When version is provided, try version-qualified directory first
            // (e.g., .taida/deps/author/pkg@version/), then fall back to unversioned.
            let root = Self::find_project_root(source_dir);

            // RCB-213: Versioned resolution with longest-prefix matching.
            // Supports submodule imports (e.g., alice/pkg/submod@b.12 resolves to
            // .taida/deps/alice/pkg@b.12/submod.td).
            if let Some(ver) = version {
                if let Some(resolution) =
                    crate::pkg::resolver::resolve_package_module_versioned(&root, module_path, ver)
                {
                    match resolution.submodule {
                        Some(submodule_path) => {
                            resolution.pkg_dir.join(format!("{}.td", submodule_path))
                        }
                        None => {
                            let entry =
                                match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir)
                                {
                                    Ok(Some(manifest)) => manifest.entry,
                                    _ => "main.td".to_string(),
                                };
                            if entry.starts_with("./") || entry.starts_with("../") {
                                resolution.pkg_dir.join(entry[2..].trim_start_matches('/'))
                            } else {
                                resolution.pkg_dir.join(&entry)
                            }
                        }
                    }
                } else {
                    // RCB-213: Versioned package not found — do not fall back silently.
                    return None;
                }
            } else if let Some(resolution) =
                crate::pkg::resolver::resolve_package_module(&root, module_path)
            {
                match resolution.submodule {
                    Some(submodule_path) => {
                        resolution.pkg_dir.join(format!("{}.td", submodule_path))
                    }
                    None => {
                        let entry =
                            match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                                Ok(Some(manifest)) => manifest.entry,
                                _ => "main.td".to_string(),
                            };
                        if entry.starts_with("./") || entry.starts_with("../") {
                            resolution.pkg_dir.join(entry[2..].trim_start_matches('/'))
                        } else {
                            resolution.pkg_dir.join(&entry)
                        }
                    }
                }
            } else {
                // RCB-103 fix: package resolution failed — do not fall back
                // to local path, which would silently misresolve a package
                // import to a nonexistent relative file.
                return None;
            }
        };

        let resolved = path.canonicalize().unwrap_or(path);

        // RCB-303: Reject relative imports that escape the project root (path traversal).
        if (module_path.starts_with("./") || module_path.starts_with("../"))
            && let Ok(sd) = source_dir.canonicalize()
        {
            let project_root = Self::find_project_root(&sd);
            if let Ok(root_canonical) = project_root.canonicalize()
                && !resolved.starts_with(&root_canonical)
            {
                return None;
            }
        }

        Some(resolved)
    }

    /// RCB-103: Find project root by walking up from the given directory.
    /// Mirrors Interpreter::find_project_root().
    pub(super) fn find_project_root(start_dir: &std::path::Path) -> std::path::PathBuf {
        let mut dir = start_dir.to_path_buf();
        loop {
            if dir.join("packages.tdm").exists()
                || dir.join("taida.toml").exists()
                || dir.join(".taida").exists()
                || dir.join(".git").exists()
            {
                return dir;
            }
            if !dir.pop() {
                break;
            }
        }
        start_dir.to_path_buf()
    }

    pub(super) fn import_module_key(&self, module_path: &str, version: Option<&str>) -> String {
        self.resolve_import_path(module_path, version)
            .map(|path| Self::module_key_for_path(&path))
            .unwrap_or_else(|| Self::fallback_module_key(module_path))
    }

    pub(super) fn resolve_user_func_symbol(&self, name: &str) -> String {
        if let Some(link_name) = self.imported_func_links.get(name) {
            link_name.clone()
        } else if self.exported_symbols.contains(name) {
            self.export_func_symbol(name)
        } else if self.is_library_module {
            // RC-1o: Library module non-exported functions must be namespaced
            // with module_key to prevent symbol collision when multiple modules
            // are inlined into the main WASM/Native module.
            // Reuse export_func_symbol() for its module_key namespacing, not
            // because this function is exported.
            self.export_func_symbol(name)
        } else {
            format!("_taida_fn_{}", name)
        }
    }

    /// QF-16/17: インポートされたシンボルの種類を判定する。
    /// モジュールのソースを解析し、シンボルが関数定義/値代入/TypeDef のいずれかを返す。
    /// Collect the explicit export list from a parsed module's AST.
    /// Returns `None` if the module has no `<<<` statements (backward compat: export everything).
    /// Returns `Some(set)` if the module has `<<<` statements listing specific symbols.
    pub(super) fn collect_module_export_list(
        statements: &[Statement],
    ) -> Option<std::collections::HashSet<String>> {
        let mut export_symbols: Vec<String> = Vec::new();
        let mut has_export = false;
        for stmt in statements {
            if let Statement::Export(export_stmt) = stmt {
                has_export = true;
                for sym in &export_stmt.symbols {
                    if !export_symbols.contains(sym) {
                        export_symbols.push(sym.clone());
                    }
                }
            }
        }
        if has_export {
            Some(export_symbols.into_iter().collect())
        } else {
            None
        }
    }

    pub(super) fn classify_imported_symbol(
        &self,
        module_path: &str,
        symbol_name: &str,
        version: Option<&str>,
        pre_resolved_facade: Option<&crate::pkg::facade::FacadeContext>,
    ) -> Result<ImportedSymbolKind, LowerError> {
        // モジュールパスを解決
        let path = match self.resolve_import_path(module_path, version) {
            Some(path) => path,
            None => return Ok(ImportedSymbolKind::Function),
        };

        // B11B-023: If facade was pre-resolved, use classify_symbol_in_module
        // for re-export-aware classification (B11B-022 fix).
        // Facade validation was already done at the import level.
        if let Some(ctx) = pre_resolved_facade {
            // Symbol kind classification uses the entry module path from facade context
            if let Some(kind) =
                crate::pkg::facade::classify_symbol_in_module(&ctx.entry_path, symbol_name, None)
            {
                return Ok(match kind {
                    crate::pkg::facade::SymbolKind::Function => ImportedSymbolKind::Function,
                    crate::pkg::facade::SymbolKind::TypeDef => ImportedSymbolKind::TypeDef,
                    crate::pkg::facade::SymbolKind::Value => ImportedSymbolKind::Value,
                });
            }
            // If classify_symbol_in_module returns None but we have a facade,
            // the symbol should have been caught by validate_facade already.
            // Fall through to Function as safe default.
            return Ok(ImportedSymbolKind::Function);
        }

        // No facade — original classification path for non-facade imports
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Ok(ImportedSymbolKind::Function),
        };
        let (program, _) = crate::parser::parse(&source);

        // Non-facade: fall back to entry module's <<< check (RCB-201)
        let export_list = Self::collect_module_export_list(&program.statements);
        if let Some(ref exports) = export_list
            && !exports.contains(symbol_name)
        {
            return Err(LowerError {
                message: format!(
                    "Symbol '{}' not found in module '{}'. \
                     The module exports: {}",
                    symbol_name,
                    module_path,
                    if exports.is_empty() {
                        "(nothing)".to_string()
                    } else {
                        let mut sorted: Vec<&String> = exports.iter().collect();
                        sorted.sort();
                        sorted
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
            });
        }

        // シンボルの種類を判定 — B11B-022: use classify_symbol_in_module
        // for re-export awareness even in non-facade path
        if let Some(kind) =
            crate::pkg::facade::classify_symbol_in_module(&path, symbol_name, Some(&source))
        {
            return Ok(match kind {
                crate::pkg::facade::SymbolKind::Function => ImportedSymbolKind::Function,
                crate::pkg::facade::SymbolKind::TypeDef => ImportedSymbolKind::TypeDef,
                crate::pkg::facade::SymbolKind::Value => ImportedSymbolKind::Value,
            });
        }

        // 見つからなかった場合はデフォルトで関数扱い
        Ok(ImportedSymbolKind::Function)
    }

    // collect_module_top_level_values は廃止。
    // init 関数方式では、モジュール側が自身の全トップレベル値を
    // _taida_init_<module_key> で GlobalSet するため、import 側での収集が不要。

    /// QF-16/17: インポートされた TypeDef のメタデータを登録する。
    /// classify_imported_symbol で TypeDef と判定されたシンボルのフィールド/メソッド情報を登録。
    /// `register_name` は alias 名（alias がない場合は orig_name と同じ）。
    pub(super) fn register_imported_typedef(
        &mut self,
        module_path: &str,
        symbol_name: &str,
        register_name: &str,
        version: Option<&str>,
    ) {
        let path = match self.resolve_import_path(module_path, version) {
            Some(path) => path,
            None => return,
        };

        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _) = crate::parser::parse(&source);

        for stmt in &program.statements {
            match stmt {
                Statement::TypeDef(type_def) if type_def.name == symbol_name => {
                    let non_method_fields: Vec<crate::parser::FieldDef> = type_def
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .cloned()
                        .collect();
                    let fields: Vec<String> =
                        non_method_fields.iter().map(|f| f.name.clone()).collect();
                    let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                        non_method_fields
                            .iter()
                            .map(|f| (f.name.clone(), f.type_annotation.clone()))
                            .collect();
                    let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                        .fields
                        .iter()
                        .filter(|f| f.is_method && f.method_def.is_some())
                        .map(|f| (f.name.clone(), f.method_def.clone().unwrap()))
                        .collect();

                    // alias 名で登録（alias なしの場合は orig_name と同一）
                    self.type_fields.insert(register_name.to_string(), fields);
                    self.type_field_types
                        .insert(register_name.to_string(), field_types);
                    self.type_field_defs
                        .insert(register_name.to_string(), non_method_fields);
                    if !methods.is_empty() {
                        self.type_method_defs
                            .insert(register_name.to_string(), methods);
                    }

                    // フィールドの型タグも登録
                    for field_def in type_def.fields.iter().filter(|f| !f.is_method) {
                        self.field_names.insert(field_def.name.clone());
                        if let Some(ref ty) = field_def.type_annotation {
                            let tag = match ty {
                                crate::parser::TypeExpr::Named(n) => match n.as_str() {
                                    "Int" => 1,
                                    "Float" => 2,
                                    "Str" => 3,
                                    "Bool" => 4,
                                    _ => 0,
                                },
                                _ => 0,
                            };
                            self.register_field_type_tag(&field_def.name, tag);
                        }
                    }
                    return;
                }
                Statement::InheritanceDef(inh_def) if inh_def.child == symbol_name => {
                    // InheritanceDef の場合、親チェーンを再帰的に辿って全フィールド/メソッドを収集
                    let (mut all_fields, mut all_field_types, mut all_field_defs, mut all_methods) =
                        Self::collect_inheritance_chain_fields(&program.statements, &inh_def.parent);

                    // 子のフィールド/メソッドを親にマージ（同名はオーバーライド）
                    for field in inh_def.fields.iter() {
                        if field.is_method {
                            if let Some(ref md) = field.method_def {
                                all_methods.retain(|(name, _)| name != &field.name);
                                all_methods.push((field.name.clone(), md.clone()));
                            }
                        } else {
                            all_fields.retain(|name| name != &field.name);
                            all_fields.push(field.name.clone());
                            all_field_types.retain(|(name, _)| name != &field.name);
                            all_field_types
                                .push((field.name.clone(), field.type_annotation.clone()));
                            all_field_defs.retain(|f| f.name != field.name);
                            all_field_defs.push(field.clone());
                        }
                    }

                    self.type_fields
                        .insert(register_name.to_string(), all_fields);
                    self.type_field_types
                        .insert(register_name.to_string(), all_field_types);
                    self.type_field_defs
                        .insert(register_name.to_string(), all_field_defs);
                    if !all_methods.is_empty() {
                        self.type_method_defs
                            .insert(register_name.to_string(), all_methods);
                    }

                    // 全フィールドの型タグを登録（親チェーン含む）
                    for field_def in inh_def.fields.iter().filter(|f| !f.is_method) {
                        self.field_names.insert(field_def.name.clone());
                        if let Some(ref ty) = field_def.type_annotation {
                            let tag = match ty {
                                crate::parser::TypeExpr::Named(n) => match n.as_str() {
                                    "Int" => 1,
                                    "Float" => 2,
                                    "Str" => 3,
                                    "Bool" => 4,
                                    _ => 0,
                                },
                                _ => 0,
                            };
                            self.register_field_type_tag(&field_def.name, tag);
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
    }

    /// 継承チェーンを再帰的に辿り、全フィールド/メソッドを収集する。
    /// TypeDef（チェーンの最上位）または InheritanceDef（中間ノード）を辿り、
    /// 全祖先のフィールド/メソッドをマージして返す。
    pub(super) fn collect_inheritance_chain_fields(
        statements: &[Statement],
        parent_name: &str,
    ) -> InheritanceChainFields {
        for stmt in statements {
            match stmt {
                Statement::TypeDef(type_def) if type_def.name == parent_name => {
                    // チェーンの最上位: TypeDef から直接フィールド/メソッドを収集
                    let mut fields = Vec::new();
                    let mut field_types = Vec::new();
                    let mut field_defs = Vec::new();
                    let mut methods = Vec::new();
                    for f in type_def.fields.iter() {
                        if f.is_method {
                            if let Some(ref md) = f.method_def {
                                methods.push((f.name.clone(), md.clone()));
                            }
                        } else {
                            fields.push(f.name.clone());
                            field_types.push((f.name.clone(), f.type_annotation.clone()));
                            field_defs.push(f.clone());
                        }
                    }
                    return (fields, field_types, field_defs, methods);
                }
                Statement::InheritanceDef(inh_def) if inh_def.child == parent_name => {
                    // 中間ノード: さらに親を再帰的に辿る
                    let (mut fields, mut field_types, mut field_defs, mut methods) =
                        Self::collect_inheritance_chain_fields(statements, &inh_def.parent);
                    // この InheritanceDef のフィールド/メソッドをマージ（同名はオーバーライド）
                    for f in inh_def.fields.iter() {
                        if f.is_method {
                            if let Some(ref md) = f.method_def {
                                methods.retain(|(name, _)| name != &f.name);
                                methods.push((f.name.clone(), md.clone()));
                            }
                        } else {
                            fields.retain(|name| name != &f.name);
                            fields.push(f.name.clone());
                            field_types.retain(|(name, _)| name != &f.name);
                            field_types.push((f.name.clone(), f.type_annotation.clone()));
                            field_defs.retain(|fd| fd.name != f.name);
                            field_defs.push(f.clone());
                        }
                    }
                    return (fields, field_types, field_defs, methods);
                }
                _ => {}
            }
        }
        // 親が見つからない場合は空を返す
        (Vec::new(), Vec::new(), Vec::new(), Vec::new())
    }

    pub(super) fn emit_imported_module_inits(&mut self, func: &mut IrFunction) {
        for init_symbol in std::mem::take(&mut self.module_inits_needed) {
            let dummy = func.alloc_var();
            func.push(IrInst::CallUser(dummy, init_symbol, vec![]));
        }
    }

    pub(super) fn bind_imported_values(&mut self, func: &mut IrFunction) {
        for (alias_name, orig_name, module_key) in std::mem::take(&mut self.imported_value_symbols)
        {
            let imported_hash = simple_hash(&format!("{}:{}", module_key, orig_name)) as i64;
            let result = func.alloc_var();
            func.push(IrInst::GlobalGet(result, imported_hash));
            func.push(IrInst::DefVar(alias_name.clone(), result));

            let local_hash = self.global_var_hash(&alias_name);
            func.push(IrInst::GlobalSet(local_hash, result));
            if alias_name != orig_name {
                let orig_hash = self.global_var_hash(&orig_name);
                func.push(IrInst::GlobalSet(orig_hash, result));
            }

            self.current_heap_vars.push(alias_name);
        }
    }

    /// ライブラリモジュールのトップレベル値を初期化するモジュール init 関数を生成する。
    /// `_taida_init_<module_key>()` — 依存モジュールを初期化した後、
    /// import 値をローカル名へ束縛し、全トップレベル代入を評価して名前空間化されたハッシュキーで
    /// グローバルテーブルに格納する。
    pub(super) fn generate_module_init_func(
        &mut self,
        module: &mut IrModule,
        program: &Program,
    ) -> Result<(), LowerError> {
        let module_key = self
            .module_key
            .as_ref()
            .expect("module_key must be set for library modules")
            .clone();
        let func_name = self.init_symbol();
        let mut init_fn = IrFunction::new(func_name);
        self.current_heap_vars.clear();

        self.emit_imported_module_inits(&mut init_fn);
        self.bind_imported_values(&mut init_fn);

        for stmt in &program.statements {
            match stmt {
                Statement::Assignment(assign) => {
                    let val = self.lower_expr(&mut init_fn, &assign.value)?;
                    let hash = simple_hash(&format!("{}:{}", module_key, assign.target)) as i64;
                    init_fn.push(IrInst::GlobalSet(hash, val));
                }
                Statement::InheritanceDef(inh_def) => {
                    // RCB-101 fix: Register inheritance parent for cross-module
                    // error type filtering.  Without this, error types defined in
                    // a library module are not registered in the parent map when
                    // the module is initialised, so |== catch handlers in the
                    // importing module cannot walk the inheritance chain.
                    let child_str_var = init_fn.alloc_var();
                    init_fn.push(IrInst::ConstStr(child_str_var, inh_def.child.clone()));
                    let parent_str_var = init_fn.alloc_var();
                    init_fn.push(IrInst::ConstStr(parent_str_var, inh_def.parent.clone()));
                    let reg_dummy = init_fn.alloc_var();
                    init_fn.push(IrInst::Call(
                        reg_dummy,
                        "taida_register_type_parent".to_string(),
                        vec![child_str_var, parent_str_var],
                    ));
                }
                _ => {}
            }
        }

        let zero = init_fn.alloc_var();
        init_fn.push(IrInst::ConstInt(zero, 0));
        init_fn.push(IrInst::Return(zero));
        module.functions.push(init_fn);
        Ok(())
    }
}
