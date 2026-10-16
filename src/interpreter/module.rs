/// Module system evaluation for the Taida interpreter.
///
/// Contains `resolve_module_path`, `find_project_root`, `eval_import`, and `eval_export`.
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::eval::{Interpreter, LoadedModule, RuntimeError, Signal};
use super::value::{ModuleId, Value};
use crate::parser::*;

/// Outcome of running a source file as a module.
pub(crate) struct ModuleRun {
    pub(crate) module: ModuleId,
    /// The module's top-level bindings when it finished.
    pub(crate) scope: Arc<HashMap<String, Value>>,
    /// Symbols the module listed with `<<<` (empty: it exports everything).
    pub(crate) exported: Vec<String>,
    pub(crate) result: Result<Value, RuntimeError>,
}

impl Interpreter {
    /// Run `program` as the module at `path`, with a module scope and type
    /// registries of its own. `prepare` seeds the fresh scope (bundled
    /// package sentinels) before the program runs. The importer's state is
    /// restored whatever the outcome.
    pub(crate) fn run_module_program(
        &mut self,
        program: &Program,
        path: &Path,
        prepare: impl FnOnce(&mut Self),
    ) -> ModuleRun {
        let module = self.new_module();
        // A user module (not an addon facade, whose `RustAddon[...]` bindings
        // only the facade loader resolves).
        if self.check_imports && self.loading_addon_facade_ctx.is_none() {
            // The module's own types, for the defaults of failed lookups in
            // its code (the main program's table cannot serve it: node ids
            // restart in every file).
            let mut checker = crate::types::TypeChecker::new();
            checker.set_compile_target(crate::types::CompileTarget::Interpreter);
            checker.set_source_file(path);
            checker.check_program(program);
            self.modules[module].typed = Some(Arc::new(checker.typed_expr_table));
        }
        let prev_file = self.current_file.replace(path.to_path_buf());
        self.loading_modules.insert(path.to_path_buf());
        let prev_env = std::mem::take(&mut self.env);
        let prev_top = std::mem::replace(&mut self.top_module, module);
        let prev_active = self.switch_module(module);
        let prev_exported_symbols = std::mem::take(&mut self.module_exported_symbols);
        // A module's top level is not inside any caller's function.
        let prev_function = self.take_active_function();

        prepare(self);
        let result = self.eval_program(program);

        self.restore_active_function(prev_function);
        let exported = std::mem::replace(&mut self.module_exported_symbols, prev_exported_symbols);
        let module_env = std::mem::replace(&mut self.env, prev_env);
        // Stashes the module's registries into its record.
        self.switch_module(prev_active);
        self.top_module = prev_top;
        self.current_file = prev_file;
        self.loading_modules.remove(path);

        let scope = Arc::new(module_env.into_globals());
        self.modules[module].scope = Some(scope.clone());
        ModuleRun {
            module,
            scope,
            exported,
            result,
        }
    }

    /// Seed the module scope of a core-bundled package with the sentinels
    /// its `<<<` list exports. The implementations live in the prelude / os
    /// runtime dispatch.
    fn inject_bundled_sentinels(&mut self, module_path: &Path) {
        let path_str = module_path.to_string_lossy();
        let in_bundled = |pkg: &str| {
            // Global path: ~/.taida/bundled/{pkg}/
            let global_unix = format!(".taida/bundled/{}/", pkg);
            let global_win = format!(".taida\\bundled\\{}\\", pkg);
            // Deps path: .taida/deps/taida-lang/{pkg}/ (symlink or direct)
            let deps_unix = format!(".taida/deps/taida-lang/{}/", pkg);
            let deps_win = format!(".taida\\deps\\taida-lang\\{}\\", pkg);
            path_str.contains(&global_unix)
                || path_str.contains(&global_win)
                || path_str.contains(&deps_unix)
                || path_str.contains(&deps_win)
        };

        if in_bundled("os") {
            for sym in super::os::OS_SYMBOLS {
                self.env
                    .define_force(sym, Value::str(format!("__os_builtin_{}", sym)));
            }
        } else if in_bundled("crypto") {
            // F55 S4: sentinel list derived from the catalog so the
            // injected surface cannot drift from the export list.
            if let Some(spec) = crate::pkg::catalog::find("crypto") {
                for sym in spec.exports {
                    self.env
                        .define_force(sym, Value::str(format!("__crypto_builtin_{}", sym)));
                }
            }
        } else if in_bundled("net") {
            for sym in super::net::eval::NET_SYMBOLS {
                self.env
                    .define_force(sym, Value::str(format!("__net_builtin_{}", sym)));
            }
        } else if in_bundled("pool") {
            // F54: sentinel list derived from the catalog so the
            // injected surface cannot drift from the export list.
            if let Some(spec) = crate::pkg::catalog::find("pool") {
                for sym in spec.exports {
                    self.env
                        .define_force(sym, Value::str(format!("__pool_builtin_{}", sym)));
                }
            }
        } else if in_bundled("abi") {
            // WebRequest/WebResponse are defined by the
            // package body itself, so pre-injecting their names would
            // turn the body's definition into a redefinition error.
            // Only inject what the body does NOT define.
            for sym in ["HostCall", "HostStep", "HostCapability"] {
                self.env.define_force(sym, Value::str(sym.to_string()));
            }
            for sym in super::abi::abi_symbols() {
                self.env
                    .define_force(sym, Value::str(format!("__abi_builtin_{}", sym)));
            }
        } else if in_bundled("build") {
            // F54: descriptor-only package. The descriptor parser
            // recognizes `BuildUnit(...)` & co. by TYPE NAME, with or
            // without an import; these sentinels only make the
            // documented `>>> taida-lang/build => @(...)` import
            // resolve instead of failing at the export-collection
            // step. They are not runtime constructors.
            if let Some(spec) = crate::pkg::catalog::find("build") {
                for sym in spec.exports {
                    self.env
                        .define_force(sym, Value::str(format!("__build_descriptor_{}", sym)));
                }
            }
        } else if in_bundled("js") {
            // F54 (D-2 fix): `taida-lang/js` materialized but never
            // injected sentinels, so the import itself silently
            // half-worked. The descriptors are JS-backend-only; the
            // interpreter rejects their use at evaluation time, but
            // the import statement documents itself honestly now.
            if let Some(spec) = crate::pkg::catalog::find("js") {
                for sym in spec.exports {
                    self.env
                        .define_force(sym, Value::str(format!("__js_descriptor_{}", sym)));
                }
            }
        }
    }

    /// Record a finished module's exports (the `<<<` list, or everything)
    /// and the type metadata that goes with them.
    pub(crate) fn loaded_module_from_run(&self, run: &ModuleRun) -> LoadedModule {
        let exports: HashMap<String, Value> = run
            .scope
            .iter()
            .filter(|(k, _)| run.exported.is_empty() || run.exported.contains(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let registry = &self.modules[run.module].registry;
        let pick = |map: &HashMap<String, Vec<FieldDef>>| -> HashMap<String, Vec<FieldDef>> {
            map.iter()
                .filter(|(k, _)| exports.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        LoadedModule {
            module: run.module,
            type_defs: pick(&registry.type_defs),
            mold_defs: pick(&registry.mold_defs),
            enum_defs: registry
                .enum_defs
                .iter()
                .filter(|(k, _)| exports.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            type_methods: registry
                .type_methods
                .iter()
                .filter(|(k, _)| exports.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            exports,
        }
    }

    /// Make an imported type, mold or enum visible under `local_name` in
    /// the active module. The name keeps pointing at the declaring
    /// module's declaration: instances made here are the same type as
    /// the ones the module makes, and their methods run there.
    fn bind_imported_type(&mut self, loaded: &LoadedModule, name: &str, local_name: &str) {
        if let Some(td_fields) = loaded.type_defs.get(name) {
            self.type_defs
                .insert(local_name.to_string(), td_fields.clone());
        }
        if let Some(mold_fields) = loaded.mold_defs.get(name) {
            self.mold_defs
                .insert(local_name.to_string(), mold_fields.clone());
        }
        if let Some(enum_variants) = loaded.enum_defs.get(name) {
            self.enum_defs
                .insert(local_name.to_string(), enum_variants.clone());
        }
        if let Some(td_methods) = loaded.type_methods.get(name) {
            self.type_methods
                .insert(local_name.to_string(), td_methods.clone());
        }
        let Some(registry) = self.registry_of(loaded.module) else {
            return;
        };
        let key = registry.enum_keys.get(name).cloned();
        let tag = registry.type_tags.get(name).cloned();
        if let Some(key) = key {
            self.enum_keys.insert(local_name.to_string(), key);
        }
        let Some(tag) = tag else {
            return;
        };
        let tag = if local_name == name {
            tag
        } else {
            // An alias gets a tag of its own, so its instances display
            // under the alias, standing for the same declaration.
            match self.origin_of_tag(&tag).cloned() {
                Some(origin) => {
                    self.alias_type_tag(local_name, origin.module, &origin.name, origin.def_site)
                }
                None => tag,
            }
        };
        self.type_tags.insert(local_name.to_string(), tag);
    }
    // ── Module System ────────────────────────────────────────

    /// Resolve a module path relative to the current file.
    pub(crate) fn resolve_module_path(&self, import_path: &str) -> Result<PathBuf, RuntimeError> {
        let path = if import_path.starts_with("./") || import_path.starts_with("../") {
            // Relative path
            if let Some(current) = &self.current_file {
                // Falls back to "." if current_file has no parent (e.g., bare filename)
                let base = current.parent().unwrap_or(Path::new("."));
                base.join(import_path)
            } else {
                PathBuf::from(import_path)
            }
        } else if let Some(stripped) = import_path.strip_prefix("~/") {
            // Project root relative
            let root = self.find_project_root();
            root.join(stripped)
        } else if import_path.starts_with('/') {
            // Absolute path
            PathBuf::from(import_path)
        } else {
            // Package import: "author/pkg", "author/pkg/submodule", or "pkg-name"
            // Uses longest-prefix matching against .taida/deps/
            let root = self.find_project_root();

            if let Some(resolution) =
                crate::pkg::resolver::resolve_package_module(&root, import_path)
            {
                match resolution.submodule {
                    Some(submodule_path) => {
                        resolution.pkg_dir.join(format!("{}.td", submodule_path))
                    }
                    None => {
                        // Package root import: read packages.tdm to determine entry point
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
            } else if let Some(bundled) =
                crate::pkg::provider::CoreBundledProvider::materialize_core_bundled(import_path)
            {
                // C26B-014: core-bundled packages (`taida-lang/os`,
                // `taida-lang/net`, `taida-lang/crypto`, `taida-lang/js`,
                // `taida-lang/pool`) can be imported without a
                // packages.tdm declaration. `docs/reference/os_api.md`
                // and `docs/guide/10_modules.md` promise both the
                // imported and import-less call forms; this branch
                // materializes the bundled stub on-demand so the
                // runtime path agrees with the checker
                // (`install_core_bundled_os_pins` in src/types/checker.rs).
                // Option B (Design Lock 2026-04-24): implementation
                // follows docs. Package imports into packages.tdm still
                // work via the `resolve_package_module` branch above
                // (deps-installed precedence preserved).
                let bundled_dir = bundled.map_err(|e| RuntimeError { message: e })?;
                bundled_dir.join("main.td")
            } else {
                return Err(RuntimeError {
                    message: format!(
                        "Package '{}' not found. Run 'taida ingot deps' to install dependencies.",
                        import_path
                    ),
                });
            }
        };

        // Canonicalize the resolved path
        let canonical = path.canonicalize().map_err(|_| RuntimeError {
            message: format!("Module not found: '{}'", path.display()),
        })?;

        // RCB-303: Reject imports that escape the project root (path traversal).
        // Relative imports (`./` or `../`) and absolute path imports (`/...`)
        // are both sandboxed to the project root. Package imports are resolved
        // through the package system and are trusted by construction.
        // C26B-007 SEC-003: extend RCB-303 to cover absolute path imports so
        // `>>> /etc/passwd.td` / `>>> /tmp/evil.td` cannot be used to probe
        // the filesystem or leak file contents through parser error messages.
        if import_path.starts_with("./")
            || import_path.starts_with("../")
            || import_path.starts_with('/')
        {
            let project_root = self.find_project_root();
            if let Ok(root_canonical) = project_root.canonicalize()
                && !canonical.starts_with(&root_canonical)
            {
                return Err(RuntimeError {
                    message: format!(
                        "Import path '{}' resolves outside the project root. \
                         Path traversal beyond the project boundary is not allowed.",
                        import_path
                    ),
                });
            }
        }

        Ok(canonical)
    }

    /// Find project root by walking up from the current file.
    /// `.taida/` is state/config storage, not a project-root marker; otherwise
    /// `~/.taida` can make `$HOME` look like the active project root.
    pub(crate) fn find_project_root(&self) -> PathBuf {
        let start = if let Some(current) = &self.current_file {
            // Falls back to "." if current_file has no parent (e.g., bare filename)
            current.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            PathBuf::from(".")
        };
        crate::project_root::find_project_root(&start)
    }

    /// Evaluate an import statement: `>>> path => @(symbols)`
    pub(crate) fn eval_import(&mut self, import: &ImportStmt) -> Result<Signal, RuntimeError> {
        // std/ imports are no longer supported (dissolved)
        if import.path.starts_with("std/") {
            return Err(RuntimeError {
                message: format!(
                    "Standard library module '{}' has been dissolved. \
                     Use prelude builtins instead (stdout, jsonParse, etc. are available without import).",
                    import.path
                ),
            });
        }

        // npm imports are only available in the JS transpiler backend
        if import.path.starts_with("npm:") {
            return Err(RuntimeError {
                message: "npm imports are only available in the JS transpiler backend".to_string(),
            });
        }

        // P-6: Versioned imports (>>> pkg@version) are only allowed in packages.tdm
        if import.version.is_some() {
            let is_tdm = self
                .current_file
                .as_ref()
                .is_some_and(|f| f.to_string_lossy().ends_with(".tdm"));
            if !is_tdm {
                return Err(RuntimeError {
                    message: format!(
                        "Versioned import '>>> {}@{}' is only allowed in packages.tdm. \
                         In .td files, use '>>> {} => @(...)' without a version.",
                        import.path,
                        import.version.as_deref().unwrap_or(""),
                        import.path
                    ),
                });
            }
        }

        // RC1 Phase 4 -- addon-backed package early branch.
        //
        // Before falling through to the source-loading path, check
        // whether the import target is an addon-backed package
        // (`<pkg_dir>/native/addon.toml` exists). If so, the import
        // never reads a Taida `.td` source -- the addon manifest
        // declares the function table and the registry hands back
        // dispatch sentinels that `try_builtin_func` routes through
        // `LoadedAddon::call_function`.
        //
        // The check uses the SAME package-directory resolution that
        // the source path uses (`resolve_package_module_versioned` /
        // `resolve_package_module`), so addon-backed and pure-source
        // packages share the same resolution order
        // (addon-backed lookups fall through to the source resolver).
        //
        // C25B-030: the interpreter is a first-class addon backend.
        // The `feature = "native"` gate here selects whether the
        // interpreter binary was built WITH the dlopen dispatcher
        // linked in. The policy guard (`ensure_addon_supported`) passes
        // for `Interpreter` in both cases; what differs is whether we
        // can actually call into a cdylib addon at runtime.
        #[cfg(feature = "native")]
        {
            if let Some(signal) = self.try_eval_addon_import(import)? {
                return Ok(signal);
            }
        }
        #[cfg(not(feature = "native"))]
        {
            // Defensive: on interpreter builds without the native dlopen
            // dispatcher, an addon-backed package is structurally
            // unreachable. We produce a deterministic error at the
            // import boundary rather than silently falling through to
            // "package not found" (or, worse, producing a bogus
            // source-load error later).
            if let Some(pkg_dir) = self.try_locate_addon_pkg_dir(import) {
                if pkg_dir.join("native").join("addon.toml").exists() {
                    // Policy guard first — for Interpreter this returns
                    // Ok(()) so we fall through to the feature-gate
                    // error below. The guard is still called so future
                    // policy restrictions (e.g. allowlist) keep a single
                    // decision point.
                    crate::addon::ensure_addon_supported(
                        crate::addon::AddonBackend::Interpreter,
                        &import.path,
                    )
                    .map_err(|e| RuntimeError {
                        message: e.to_string(),
                    })?;

                    return Err(RuntimeError {
                        message: format!(
                            "addon-backed package '{}' requires a taida binary built with the \
                             'native' feature (addon dispatcher not linked in this build)",
                            import.path
                        ),
                    });
                }
            }
        }

        // For versioned imports (>>> alice/string-utils@b.12 or alice/pkg/submod@b.12),
        // the path is "alice/string-utils" or "alice/pkg/submod" and version is "b.12".
        //
        // RC-1q: Version coexistence support — when a versioned import is encountered,
        // first try the version-qualified directory (.taida/deps/org/name@version/),
        // then fall back to the unversioned directory (.taida/deps/org/name/).
        //
        // RCB-213: Use resolve_package_module_versioned with longest-prefix matching
        // to support submodule imports (e.g., alice/pkg/submod@b.12 resolves to
        // .taida/deps/alice/pkg@b.12/submod.td).
        //
        // B11-9d: For package root imports, also capture manifest.exports so
        // we can use it as the authoritative facade filter.
        let (module_path, manifest_exports_filter) = if let Some(version) = &import.version {
            let root = self.find_project_root();
            let pkg_id = &import.path;
            if let Some(resolution) =
                crate::pkg::resolver::resolve_package_module_versioned(&root, pkg_id, version)
            {
                let (path, exports_filter) = match resolution.submodule {
                    Some(submodule_path) => (
                        resolution.pkg_dir.join(format!("{}.td", submodule_path)),
                        None,
                    ),
                    None => {
                        // Package root import: read packages.tdm to determine entry point
                        let (entry, exports) =
                            match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                                Ok(Some(manifest)) => {
                                    let exports = if manifest.exports.is_empty() {
                                        None
                                    } else {
                                        Some(manifest.exports)
                                    };
                                    (manifest.entry, exports)
                                }
                                _ => ("main.td".to_string(), None),
                            };
                        let p = if entry.starts_with("./") || entry.starts_with("../") {
                            resolution.pkg_dir.join(entry[2..].trim_start_matches('/'))
                        } else {
                            resolution.pkg_dir.join(&entry)
                        };
                        (p, exports)
                    }
                };
                let canonical = path.canonicalize().map_err(|_| RuntimeError {
                    message: format!("Module not found: '{}'", path.display()),
                })?;
                (canonical, exports_filter)
            } else {
                return Err(RuntimeError {
                    message: format!(
                        "Package '{}@{}' not found. Run 'taida ingot deps' to install dependencies.",
                        pkg_id, version
                    ),
                });
            }
        } else {
            // B11-9d: For non-versioned package root imports, also check manifest.exports.
            let is_package_import = !import.path.starts_with("./")
                && !import.path.starts_with("../")
                && !import.path.starts_with('/')
                && !import.path.starts_with("~/")
                && !import.path.starts_with("std/")
                && !import.path.starts_with("npm:");
            let exports_filter = if is_package_import {
                let root = self.find_project_root();
                if let Some(resolution) =
                    crate::pkg::resolver::resolve_package_module(&root, &import.path)
                {
                    if resolution.submodule.is_none() {
                        // Package root import: check manifest.exports
                        match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                            Ok(Some(manifest)) if !manifest.exports.is_empty() => {
                                Some(manifest.exports)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            (self.resolve_module_path(&import.path)?, exports_filter)
        };

        // Check for circular imports
        if self.loading_modules.contains(&module_path) {
            return Err(RuntimeError {
                message: format!("Circular import detected: '{}'", module_path.display()),
            });
        }

        // Check if already loaded (cached)
        let exports = if let Some(cached) = self.loaded_modules.get(&module_path) {
            cached.exports.clone()
        } else {
            // Load and execute the module
            let source = std::fs::read_to_string(&module_path).map_err(|e| RuntimeError {
                message: format!("Cannot read module '{}': {}", module_path.display(), e),
            })?;

            let (program, parse_errors) = crate::parser::parse(&source);
            if !parse_errors.is_empty() {
                return Err(RuntimeError {
                    message: format!(
                        "Parse errors in module '{}': {}",
                        module_path.display(),
                        parse_errors
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                });
            }

            let run = self.run_module_program(&program, &module_path, |this| {
                // Inject core-bundled package symbols.
                // The actual implementations live in prelude/os runtime dispatch.
                // Here we inject sentinel values so that <<< exports resolve correctly.
                this.inject_bundled_sentinels(&module_path);
            });

            // Check for module execution errors
            if let Err(e) = &run.result {
                return Err(RuntimeError {
                    message: format!("Error executing module '{}': {}", module_path.display(), e),
                });
            }
            // A `><` in the module ends the whole program.
            if matches!(run.result, Ok(Value::Gorilla)) {
                return Err(self.gorilla_escape());
            }

            // Exported symbols: the `<<<` list, or every top-level binding
            // when the module has no `<<<`. A function keeps resolving the
            // names of its own module (see `enter_call_scope`), so exporting
            // one never needs the rest of the module to come along.
            let loaded = self.loaded_module_from_run(&run);
            let module_exports = loaded.exports.clone();
            self.loaded_modules.insert(module_path.clone(), loaded);
            module_exports
        };

        let loaded = self
            .loaded_modules
            .get(&module_path)
            .cloned()
            .ok_or_else(|| RuntimeError {
                message: format!("Module '{}' did not finish loading", module_path.display()),
            })?;

        // B11-9d: If manifest.exports is set (package root import with facade),
        // validate each imported symbol against the manifest facade.
        // - Symbol not in manifest.exports → reject (not part of public API)
        // - Symbol in manifest.exports but not in module → error (declared but missing)
        if let Some(ref facade_exports) = manifest_exports_filter {
            for sym in &import.symbols {
                let name = &sym.name;
                if !facade_exports.contains(name) {
                    return Err(RuntimeError {
                        message: crate::pkg::facade::format_facade_violation(
                            &crate::pkg::facade::FacadeViolation::HiddenSymbol {
                                name: name.clone(),
                                available: facade_exports.clone(),
                            },
                        ),
                    });
                }
                if !exports.contains_key(name) {
                    return Err(RuntimeError {
                        message: crate::pkg::facade::format_facade_violation(
                            &crate::pkg::facade::FacadeViolation::GhostSymbol {
                                name: name.clone(),
                            },
                        ),
                    });
                }
            }
        }

        // An import without a symbol list brings in every export
        // (`docs/guide/10_modules.md`); the importer's own definitions take
        // precedence over those names.
        if import.symbols.is_empty() {
            // A package facade (`manifest.exports`) limits what "every
            // export" means.
            let mut names: Vec<&String> = exports
                .keys()
                .filter(|name| {
                    manifest_exports_filter
                        .as_ref()
                        .is_none_or(|facade| facade.contains(*name))
                })
                .collect();
            names.sort();
            for name in names {
                self.env.define_star_import(name, exports[name].clone());
                self.bind_imported_type(&loaded, name, name);
            }
            return Ok(Signal::Value(Value::Unit));
        }

        // Bind imported symbols into current scope
        for sym in &import.symbols {
            let name = &sym.name;
            if let Some(value) = exports.get(name) {
                let local_name = sym.alias.as_deref().unwrap_or(name);
                // Use define() to prevent overwriting existing variables.
                // If there is a name conflict, user can use alias: >>> mod => @(X: alias)
                if self.env.define(local_name, value.clone()).is_err() {
                    return Err(RuntimeError {
                        message: format!(
                            "Cannot import '{}' as '{}': name already defined in this scope. \
                             Use an alias to resolve the conflict: >>> {} => @({}: newName)",
                            name,
                            local_name,
                            module_path.display(),
                            name
                        ),
                    });
                }
                // QF-17: TypeDef がインポートされた場合、type_defs と type_methods も登録
                self.bind_imported_type(&loaded, name, local_name);
            } else {
                return Err(RuntimeError {
                    message: format!(
                        "Symbol '{}' not found in module '{}'",
                        name,
                        module_path.display()
                    ),
                });
            }
        }

        Ok(Signal::Value(Value::Unit))
    }

    /// Evaluate an export statement: `<<< @(symbols)` or `<<< symbol`
    ///
    /// Records which symbols should be visible to importers.
    /// When a module has `<<<`, only the listed symbols are exported.
    /// When no `<<<` exists, all module-level symbols are exported (backward compat).
    pub(crate) fn eval_export(&mut self, export: &ExportStmt) -> Result<Signal, RuntimeError> {
        // RCB-212: Re-export path `<<< ./path` is not supported at runtime.
        // The checker catches this, but --no-check bypasses it.
        if export.path.is_some() {
            return Err(RuntimeError {
                message: "Re-export with path (`<<< ./path`) is not yet supported. \
                         Use explicit import and re-export instead."
                    .to_string(),
            });
        }
        // RCB-102: `<<< @()` (empty export) is an error — a module that exports
        // nothing is useless to importers.  Without this check, the empty symbols
        // list falls through to the "no <<< found → export everything" path.
        if export.symbols.is_empty() && export.path.is_none() {
            return Err(RuntimeError {
                message: "Empty export `<<< @()` exports nothing. \
                         Remove the export statement or list symbols to export: `<<< @(name1, name2)`."
                    .to_string(),
            });
        }
        // P-6: Versioned exports (<<<@version) are only allowed in packages.tdm
        if export.version.is_some() {
            let is_tdm = self
                .current_file
                .as_ref()
                .is_some_and(|f| f.to_string_lossy().ends_with(".tdm"));
            if !is_tdm {
                return Err(RuntimeError {
                    message: format!(
                        "Versioned export '<<<@{}' is only allowed in packages.tdm.",
                        export.version.as_deref().unwrap_or("")
                    ),
                });
            }
        }
        for sym in &export.symbols {
            if !self.module_exported_symbols.contains(sym) {
                self.module_exported_symbols.push(sym.clone());
            }
        }
        Ok(Signal::Value(Value::Unit))
    }

    // ── RC1 Phase 4: addon-backed package import support ────────────────

    /// Resolve only the **package directory** for an import statement
    /// (without canonicalizing to a `.td` source file).
    ///
    /// This is the shared step that addon-backed packages and pure
    /// source packages perform identically. The addon-import branch
    /// uses it to decide whether `<pkg_dir>/native/addon.toml` exists
    /// before committing to a source-loading or addon-loading path.
    ///
    /// Returns `None` for relative imports (`./mod.td`) and absolute
    /// path imports — those can never be addon-backed.
    pub(crate) fn try_locate_addon_pkg_dir(
        &self,
        import: &crate::parser::ImportStmt,
    ) -> Option<std::path::PathBuf> {
        let path = &import.path;
        // Relative / absolute / project-root imports are never addons.
        if path.starts_with("./")
            || path.starts_with("../")
            || path.starts_with('/')
            || path.starts_with("~/")
            || path.starts_with("std/")
            || path.starts_with("npm:")
        {
            return None;
        }

        let project_root = self.find_project_root();

        // Use the same resolver pair as `eval_import` so addon-backed
        // and pure-source resolution stay in lockstep.
        let resolution = if let Some(version) = &import.version {
            crate::pkg::resolver::resolve_package_module_versioned(&project_root, path, version)
        } else {
            crate::pkg::resolver::resolve_package_module(&project_root, path)
        }?;

        // Submodule imports (`org/pkg/sub`) cannot be addon-backed in
        // RC1: addon function dispatch happens at the package level,
        // not the submodule level. Submodules of an addon-backed
        // package fall through to the source path (today there is no
        // such concept, but the structure leaves room).
        if resolution.submodule.is_some() {
            return None;
        }

        Some(resolution.pkg_dir)
    }

    /// Native-only addon import handler.
    ///
    /// Detects whether `import` resolves to an addon-backed package,
    /// and if so:
    /// 1. Calls `ensure_addon_supported` (defensive: should always
    /// pass on the native interpreter binary).
    /// 2. Loads / caches the addon via `AddonRegistry::ensure_loaded`.
    /// 3. If the package ships a Taida-side facade at
    /// `<pkg_dir>/taida/<stem>.td`, executes it as a module. Inside
    /// the facade, each addon-backed function appears as an explicit
    /// `Name <= RustAddon["fn"](arity <= N)` binding,
    /// sub-step B-5 — legacy implicit pre-inject is removed). The
    /// facade can also define pure-Taida companion values (like the
    /// `KeyKind` pack).
    /// 4. Validates that every symbol the import statement asked for
    /// resolves to either
    /// - a facade export (if a facade was loaded), or
    /// - an `addon.toml` `[functions]` entry.
    /// 5. Binds each requested symbol into the current env. Facade
    /// exports are bound by value; addon functions are bound as
    /// sentinels `Value::str("__taida_addon_call::<pkg>::<fn>")` that
    /// `try_addon_func` (in `addon.rs`) routes through
    /// `LoadedAddon::call_function`.
    ///
    /// Returns `Ok(Some(Signal::Value(Unit)))` if the import was
    /// handled by the addon path, `Ok(None)` if it was not addon-backed
    /// (caller should fall through to the source-loading path), or
    /// `Err(RuntimeError)` for any deterministic addon failure mode.
    #[cfg(feature = "native")]
    pub(crate) fn try_eval_addon_import(
        &mut self,
        import: &crate::parser::ImportStmt,
    ) -> Result<Option<Signal>, RuntimeError> {
        let pkg_dir = match self.try_locate_addon_pkg_dir(import) {
            Some(d) => d,
            None => return Ok(None),
        };
        let manifest_path = pkg_dir.join("native").join("addon.toml");
        if !manifest_path.exists() {
            return Ok(None);
        }

        // Backend policy guard. C25B-030 Phase 1B: the interpreter is a
        // first-class addon backend (reference implementation), so we
        // call `ensure_addon_supported` with `AddonBackend::Interpreter`
        // truthfully rather than masquerading as `Native`. The actual
        // dlopen dispatch is still gated on `feature = "native"` below
        // (see `try_addon_func` in `addon.rs`); the policy guard
        // only answers "is this backend allowed to consume addons?".
        crate::addon::ensure_addon_supported(crate::addon::AddonBackend::Interpreter, &import.path)
            .map_err(|e| RuntimeError {
                message: e.to_string(),
            })?;

        let project_root = self.find_project_root();
        let resolved = crate::addon::AddonRegistry::global()
            .ensure_loaded(&project_root, &import.path, &pkg_dir)
            .map_err(|e| RuntimeError {
                message: e.to_string(),
            })?;

        // RC2B-207 / E30B-007 sub-step B-5: Load the optional Taida-side
        // facade. The facade is a single `.td` file at
        // `<pkg_dir>/taida/<stem>.td` where `<stem>` is the final
        // `/`-segment of the canonical package id (e.g. `terminal` for
        // `taida-lang/terminal`). It runs in a dedicated child
        // environment.
        //
        // E30 Lock-G Sub-G4 verdict (2026-04-28): each addon-backed
        // function MUST appear as an explicit `Name <= RustAddon["fn"]
        // (arity <= N)` binding in the facade. `eval_rust_addon_binding`
        // resolves these to addon sentinels (the same string the legacy
        // pre-inject produced). Facade exports drive the user import's
        // symbol lookup and always take precedence over the raw
        // `[functions]` table.
        let facade_exports = self.load_addon_facade(&pkg_dir, &resolved)?;

        // Bind each requested symbol. Lookup order:
        // 1. facade exports (facade's `<<<` or full symbol snapshot), or
        // 2. manifest `[functions]` entries.
        // Anything missing from both is rejected as
        // "Symbol '<name>' not found in addon-backed package '<pkg>'".
        for sym in &import.symbols {
            let orig_name = &sym.name;
            let local_name = sym.alias.as_deref().unwrap_or(orig_name);

            let value = if let Some(val) = facade_exports.get(orig_name) {
                val.clone()
            } else if resolved.manifest.functions.contains_key(orig_name) {
                // The sentinel encodes (package_id, function_name) so
                // the dispatcher can look the addon back up via the
                // registry without needing per-call env state. We use
                // "::" as the separator because existing sentinels
                // (`__os_builtin_*`, `__net_builtin_*`, etc.) use
                // single-segment underscore names, so collision is
                // structurally impossible.
                Value::str(format!(
                    "__taida_addon_call::{}::{}",
                    resolved.package_id, orig_name
                ))
            } else {
                return Err(RuntimeError {
                    message: format!(
                        "Symbol '{}' not found in addon-backed package '{}'",
                        orig_name, import.path
                    ),
                });
            };

            if self.env.define(local_name, value).is_err() {
                return Err(RuntimeError {
                    message: format!(
                        "Cannot import '{}' as '{}': name already defined in this scope. \
                         Use an alias to resolve the conflict: >>> {} => @({}: newName)",
                        orig_name, local_name, import.path, orig_name
                    ),
                });
            }
        }

        Ok(Some(Signal::Value(Value::Unit)))
    }

    /// Load the Taida-side facade for an addon-backed package, if one
    /// exists. Returns the facade's exported environment snapshot (or
    /// an empty map if no facade is present).
    ///
    /// The facade file lives at `<pkg_dir>/taida/<stem>.td` where
    /// `<stem>` is the final `/`-segment of the package id. Inside the
    /// facade, each addon-backed function appears as an explicit
    /// `Name <= RustAddon["fn"](arity <= N)` binding
    /// sub-step B-5 verdict — legacy implicit pre-inject is removed).
    /// `eval_rust_addon_binding` resolves these to addon sentinels
    /// (`Value::str("__taida_addon_call::<pkg>::<fn>")`) — the same
    /// string the legacy pre-inject path produced. The facade can also
    /// combine addon calls with pure-Taida companion values such as the
    /// `KeyKind` enum pack.
    ///
    /// The facade is cached in `loaded_modules` under its canonical
    /// path so subsequent addon imports from the same package return
    /// the same export set without re-executing the facade. This makes
    /// the facade behave exactly like a normal cached source module.
    #[cfg(feature = "native")]
    fn load_addon_facade(
        &mut self,
        pkg_dir: &std::path::Path,
        resolved: &crate::addon::ResolvedAddon,
    ) -> Result<std::collections::HashMap<String, Value>, RuntimeError> {
        // Derive the facade filename from the canonical package id. We
        // use the last segment after `/` so `taida-lang/terminal`
        // picks `taida/terminal.td`, and a plain `mypkg` picks
        // `taida/mypkg.td`.
        let stem = resolved
            .package_id
            .rsplit('/')
            .next()
            .unwrap_or(&resolved.package_id);
        let facade_path = pkg_dir.join("taida").join(format!("{}.td", stem));
        if !facade_path.exists() {
            return Ok(std::collections::HashMap::new());
        }

        // Canonicalize for cache keying. Fall back to the raw path so
        // an unreadable parent directory still produces a deterministic
        // error further down when we try to read the file.
        let canonical = facade_path
            .canonicalize()
            .unwrap_or_else(|_| facade_path.clone());

        if let Some(cached) = self.loaded_modules.get(&canonical) {
            return Ok(cached.exports.clone());
        }

        // Facade execution mirrors the source-module loading path in
        // `eval_import`: parse, swap env/current_file/exports state,
        // evaluate, then restore.
        let source = std::fs::read_to_string(&canonical).map_err(|e| RuntimeError {
            message: format!("Cannot read addon facade '{}': {}", canonical.display(), e),
        })?;
        let (program, parse_errors) = crate::parser::parse(&source);
        if !parse_errors.is_empty() {
            return Err(RuntimeError {
                message: format!(
                    "Parse errors in addon facade '{}': {}",
                    canonical.display(),
                    parse_errors
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            });
        }

        // Circular facade load guard. Reuses the same set as regular
        // module loading so a pathological facade that re-imports
        // itself still trips the circular-import error.
        if self.loading_modules.contains(&canonical) {
            return Err(RuntimeError {
                message: format!(
                    "Circular import detected while loading addon facade '{}'",
                    canonical.display()
                ),
            });
        }

        // Stash the previous facade context (if any) so nested facade loads
        // (currently disallowed but defensively handled) restore correctly.
        // The new context exposes the package id + arity map to
        // `RustAddon["fn"](arity <= N)` evaluation inside the facade: each
        // addon function is bound explicitly there, and a bare reference to
        // a manifest function is an undefined identifier (`[E1413]`).
        let prev_addon_facade_ctx = self.loading_addon_facade_ctx.replace((
            resolved.package_id.clone(),
            resolved.manifest.functions.clone(),
        ));
        let run = self.run_module_program(&program, &canonical, |_| {});
        self.loading_addon_facade_ctx = prev_addon_facade_ctx;

        if let Err(e) = &run.result {
            return Err(RuntimeError {
                message: format!(
                    "Error executing addon facade '{}': {}",
                    canonical.display(),
                    e
                ),
            });
        }
        if matches!(run.result, Ok(Value::Gorilla)) {
            return Err(self.gorilla_escape());
        }

        // Collect facade exports. If the facade used `<<<`, only the
        // listed symbols survive. Otherwise every top-level binding is
        // exported (same rule as regular modules). Type metadata for the
        // exported symbols is kept so user code can use facade-declared
        // types if a facade ever introduces them.
        let loaded = self.loaded_module_from_run(&run);
        let exports = loaded.exports.clone();
        self.loaded_modules.insert(canonical, loaded);

        Ok(exports)
    }
}
