/// Module system evaluation for the Taida interpreter.
///
/// Contains `resolve_module_path`, `find_project_root`, `eval_import`, and `eval_export`.
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use std::path::{Path, PathBuf};

use super::env::Environment;
use super::eval::{Interpreter, LoadedModule, RuntimeError, Signal};
use super::value::Value;
use crate::parser::*;

impl Interpreter {
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
            } else {
                return Err(RuntimeError {
                    message: format!(
                        "Package '{}' not found. Run 'taida deps' to install dependencies.",
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
        // Only check relative imports (`./` or `../`); absolute and package imports
        // are either trusted paths or resolved through the package system.
        if import_path.starts_with("./") || import_path.starts_with("../") {
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
    /// Looks for `packages.tdm`, `taida.toml`, `.taida` (project-local), or `.git`.
    pub(crate) fn find_project_root(&self) -> PathBuf {
        let start = if let Some(current) = &self.current_file {
            // Falls back to "." if current_file has no parent (e.g., bare filename)
            current.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            PathBuf::from(".")
        };
        let mut dir = start.clone();
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
        // Fallback to the current file's directory
        start
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
        // packages share the resolution order documented in
        // `.dev/RC1_DESIGN.md` Phase 4 Lock.
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
                        "Package '{}@{}' not found. Run 'taida deps' to install dependencies.",
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

            // Save current state
            let prev_file = self.current_file.clone();
            let prev_env = self.env.clone();
            let prev_exported_symbols = std::mem::take(&mut self.module_exported_symbols);
            // QF-17: TypeDef 登録情報を退避（モジュール実行中の TypeDef はモジュールのスコープ）
            let prev_type_defs = self.type_defs.clone();
            let prev_type_methods = self.type_methods.clone();

            // Set up for module execution
            self.current_file = Some(module_path.clone());
            self.loading_modules.insert(module_path.clone());
            self.env = Environment::new();

            // Inject core-bundled package symbols.
            // The actual implementations live in prelude/os runtime dispatch.
            // Here we inject sentinel values so that <<< exports resolve correctly.
            {
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
                    for sym in super::os_eval::OS_SYMBOLS {
                        self.env
                            .define_force(sym, Value::Str(format!("__os_builtin_{}", sym)));
                    }
                } else if in_bundled("crypto") {
                    {
                        let sym = "sha256";
                        self.env
                            .define_force(sym, Value::Str(format!("__crypto_builtin_{}", sym)));
                    }
                } else if in_bundled("net") {
                    for sym in super::net_eval::NET_SYMBOLS {
                        self.env
                            .define_force(sym, Value::Str(format!("__net_builtin_{}", sym)));
                    }
                } else if in_bundled("pool") {
                    for sym in [
                        "poolCreate",
                        "poolAcquire",
                        "poolRelease",
                        "poolClose",
                        "poolHealth",
                    ] {
                        self.env
                            .define_force(sym, Value::Str(format!("__pool_builtin_{}", sym)));
                    }
                }
            }

            // Execute the module
            let result = self.eval_program(&program);

            // Collect exports: snapshot all symbols, then filter by <<< declarations
            let module_env = self.env.clone();
            let exported_symbols = std::mem::take(&mut self.module_exported_symbols);
            // QF-17: モジュール内で定義された TypeDef 情報を取得
            let module_type_defs = self.type_defs.clone();
            let module_type_methods = self.type_methods.clone();
            // C20B-015 / ROOT-18: Snapshot enum_defs from the defining module so
            // exported functions can resolve their own JSON schemas even when the
            // caller module does not import the typedef.
            let module_enum_defs = self.enum_defs.clone();

            // Restore state
            self.env = prev_env;
            self.current_file = prev_file;
            self.loading_modules.remove(&module_path);
            self.module_exported_symbols = prev_exported_symbols;
            self.type_defs = prev_type_defs;
            self.type_methods = prev_type_methods;

            // Check for module execution errors
            if let Err(e) = result {
                return Err(RuntimeError {
                    message: format!("Error executing module '{}': {}", module_path.display(), e),
                });
            }

            // Extract exported symbols from module environment.
            // If the module has <<< declarations, only export those symbols.
            // If no <<< exists, export everything (backward compatibility).
            let all_symbols = module_env.snapshot();
            let module_exports = if exported_symbols.is_empty() {
                // No <<< found — export all symbols (backward compat)
                all_symbols
            } else {
                // <<< found — filter to only declared exports
                all_symbols
                    .into_iter()
                    .filter(|(k, _)| exported_symbols.contains(k))
                    .collect()
            };

            // F-56 fix: Enrich exported function closures with all module-level symbols.
            // When a function is defined, its closure captures env.snapshot() at that point,
            // which may not include functions defined later or even itself (for recursion).
            // By enriching closures with the final module env, exported functions can
            // reference any module-level symbol (including other exports) without the
            // importer having to explicitly import those symbols.
            //
            // Two-pass approach:
            // 1. First, enrich all Function values in the full env so that every function's
            //    closure includes all module-level symbols (including itself for recursion).
            // 2. Then, rebuild module_exports using the enriched values.
            let module_exports = {
                let full_env = module_env.snapshot();
                // C20B-015 / ROOT-18: Share the defining module's typedef / enum
                // registries across every enriched function. Arc-wrapped so we pay
                // only the clone-of-Arc cost per function, not the full map clone.
                let module_td_arc = if module_type_defs.is_empty() {
                    None
                } else {
                    Some(std::sync::Arc::new(module_type_defs.clone()))
                };
                let module_ed_arc = if module_enum_defs.is_empty() {
                    None
                } else {
                    Some(std::sync::Arc::new(module_enum_defs.clone()))
                };
                // Pass 1: Create enriched versions of all Function values
                let mut enriched_env: std::collections::HashMap<String, Value> =
                    std::collections::HashMap::new();
                for (name, value) in &full_env {
                    if let Value::Function(fv) = value {
                        let mut new_closure = (*fv.closure).clone();
                        for (k, v) in &full_env {
                            if !new_closure.contains_key(k) {
                                new_closure.insert(k.clone(), v.clone());
                            }
                        }
                        let mut enriched_fv = fv.clone();
                        enriched_fv.closure = std::sync::Arc::new(new_closure);
                        // C20B-015 / ROOT-18: attach defining-module scope so
                        // `JSON[raw, Schema]()` inside this function can resolve
                        // Schema even after the function crosses a module boundary.
                        if enriched_fv.module_type_defs.is_none() {
                            enriched_fv.module_type_defs = module_td_arc.clone();
                        }
                        if enriched_fv.module_enum_defs.is_none() {
                            enriched_fv.module_enum_defs = module_ed_arc.clone();
                        }
                        enriched_env.insert(name.clone(), Value::Function(enriched_fv));
                    } else {
                        enriched_env.insert(name.clone(), value.clone());
                    }
                }
                // Pass 2: For enriched functions, update their closures to reference
                // enriched versions of other functions (so recursive calls also get enriched closures)
                let enriched_snapshot = enriched_env.clone();
                for (_name, value) in enriched_env.iter_mut() {
                    if let Value::Function(fv) = value {
                        let mut updated_closure = (*fv.closure).clone();
                        for (k, v) in &enriched_snapshot {
                            // Overwrite with enriched version
                            updated_closure.insert(k.clone(), v.clone());
                        }
                        fv.closure = std::sync::Arc::new(updated_closure);
                    }
                }
                // Filter to only exported symbols
                module_exports
                    .into_keys()
                    .map(|name| {
                        // Falls back to Unit if export symbol is declared but not
                        // defined in the environment (e.g., forward-declared but
                        // never assigned). This matches Taida's null-exclusion
                        // philosophy where Unit serves as the universal default.
                        let enriched_val = enriched_env.get(&name).cloned().unwrap_or(Value::Unit);
                        (name, enriched_val)
                    })
                    .collect::<std::collections::HashMap<String, Value>>()
            };

            // QF-17: export された TypeDef の情報をフィルタリング
            let exported_type_defs: std::collections::HashMap<
                String,
                Vec<crate::parser::FieldDef>,
            > = module_type_defs
                .into_iter()
                .filter(|(k, _)| module_exports.contains_key(k))
                .collect();
            let exported_enum_defs: std::collections::HashMap<String, Vec<String>> = self
                .enum_defs
                .iter()
                .filter(|(k, _)| module_exports.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let exported_type_methods: std::collections::HashMap<
                String,
                std::collections::HashMap<String, crate::parser::FuncDef>,
            > = module_type_methods
                .into_iter()
                .filter(|(k, _)| module_exports.contains_key(k))
                .collect();

            // Cache the module
            self.loaded_modules.insert(
                module_path.clone(),
                LoadedModule {
                    exports: module_exports.clone(),
                    type_defs: exported_type_defs,
                    enum_defs: exported_enum_defs,
                    type_methods: exported_type_methods,
                },
            );

            module_exports
        };

        // QF-17: キャッシュから TypeDef 情報を取得（import 後の TypeDef 登録用）
        let cached_type_defs = self
            .loaded_modules
            .get(&module_path)
            .map(|m| m.type_defs.clone())
            .unwrap_or_default();
        let cached_enum_defs = self
            .loaded_modules
            .get(&module_path)
            .map(|m| m.enum_defs.clone())
            .unwrap_or_default();
        let cached_type_methods = self
            .loaded_modules
            .get(&module_path)
            .map(|m| m.type_methods.clone())
            .unwrap_or_default();

        // B11-9d: If manifest.exports is set (package root import with facade),
        // validate each imported symbol against the manifest facade.
        // - Symbol not in manifest.exports → reject (not part of public API)
        // - Symbol in manifest.exports but not in module → error (declared but missing)
        if let Some(ref facade_exports) = manifest_exports_filter {
            for sym in &import.symbols {
                let name = &sym.name;
                if !facade_exports.contains(name) {
                    return Err(RuntimeError {
                        message: format!(
                            "Symbol '{}' is not part of the public API declared in packages.tdm. \
                             Available exports: {}",
                            name,
                            facade_exports.join(", ")
                        ),
                    });
                }
                if !exports.contains_key(name) {
                    return Err(RuntimeError {
                        message: format!(
                            "Symbol '{}' is declared in packages.tdm but not found in the entry module. \
                             The entry module must export all symbols listed in the package facade.",
                            name
                        ),
                    });
                }
            }
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
                if let Some(td_fields) = cached_type_defs.get(name) {
                    self.type_defs
                        .insert(local_name.to_string(), td_fields.clone());
                }
                if let Some(enum_variants) = cached_enum_defs.get(name) {
                    self.enum_defs
                        .insert(local_name.to_string(), enum_variants.clone());
                }
                if let Some(td_methods) = cached_type_methods.get(name) {
                    self.type_methods
                        .insert(local_name.to_string(), td_methods.clone());
                }
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
    ///    pass on the native interpreter binary).
    /// 2. Loads / caches the addon via `AddonRegistry::ensure_loaded`.
    /// 3. If the package ships a Taida-side facade at
    ///    `<pkg_dir>/taida/<stem>.td`, executes it as a module with the
    ///    addon's `[functions]` pre-injected as sentinels. This lets the
    ///    facade wrap the lowercase Rust functions under uppercase
    ///    Taida-side names (e.g. `TerminalSize <= terminalSize`) and
    ///    define pure-Taida companion values (like the `KeyKind` pack).
    /// 4. Validates that every symbol the import statement asked for
    ///    resolves to either
    ///      - a facade export (if a facade was loaded), or
    ///      - an `addon.toml` `[functions]` entry.
    /// 5. Binds each requested symbol into the current env. Facade
    ///    exports are bound by value; addon functions are bound as
    ///    sentinels `Value::Str("__taida_addon_call::<pkg>::<fn>")` that
    ///    `try_addon_func` (in `addon_eval.rs`) routes through
    ///    `LoadedAddon::call_function`.
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
        // (see `try_addon_func` in `addon_eval.rs`); the policy guard
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

        // RC2B-207: Load the optional Taida-side facade. The facade is a
        // single `.td` file at `<pkg_dir>/taida/<stem>.td` where `<stem>`
        // is the final `/`-segment of the canonical package id (e.g.
        // `terminal` for `taida-lang/terminal`). It runs in a dedicated
        // child environment with all `[functions]` entries pre-bound as
        // addon sentinels, so the facade can write
        // `TerminalSize <= terminalSize` to re-export the Rust function
        // under a Taida-side name, or define auxiliary pure-Taida values
        // like the `KeyKind` enum pack. Facade exports drive the user
        // import's symbol lookup and always take precedence over the raw
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
                Value::Str(format!(
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
    /// facade, every manifest `[functions]` entry is pre-bound as an
    /// addon sentinel (`Value::Str(__taida_addon_call::<pkg>::<fn>)`)
    /// so the facade can assign `TerminalSize <= terminalSize` to
    /// rename the Rust function under a Taida-side name, or combine
    /// addon calls with pure-Taida companion values such as the
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

        let prev_file = self.current_file.clone();
        let prev_env = self.env.clone();
        let prev_exported_symbols = std::mem::take(&mut self.module_exported_symbols);
        let prev_type_defs = self.type_defs.clone();
        let prev_type_methods = self.type_methods.clone();

        self.current_file = Some(canonical.clone());
        self.loading_modules.insert(canonical.clone());
        self.env = Environment::new();

        // Pre-inject every manifest `[functions]` entry as an addon
        // sentinel. The facade can bind these to uppercase Taida names
        // (`TerminalSize <= terminalSize`) or reference them to combine
        // addon calls with pure-Taida values (`KeyKind`, wrapper funcs).
        for fn_name in resolved.manifest.functions.keys() {
            let sentinel = format!("__taida_addon_call::{}::{}", resolved.package_id, fn_name);
            self.env.define_force(fn_name, Value::Str(sentinel));
        }

        let exec_result = self.eval_program(&program);

        let module_env = self.env.clone();
        let exported_symbols = std::mem::take(&mut self.module_exported_symbols);
        let module_type_defs = self.type_defs.clone();
        let module_type_methods = self.type_methods.clone();

        self.env = prev_env;
        self.current_file = prev_file;
        self.loading_modules.remove(&canonical);
        self.module_exported_symbols = prev_exported_symbols;
        self.type_defs = prev_type_defs;
        self.type_methods = prev_type_methods;

        if let Err(e) = exec_result {
            return Err(RuntimeError {
                message: format!(
                    "Error executing addon facade '{}': {}",
                    canonical.display(),
                    e
                ),
            });
        }

        // Collect facade exports. If the facade used `<<<`, only the
        // listed symbols survive. Otherwise every top-level binding is
        // exported (same rule as regular modules).
        let all_symbols = module_env.snapshot();
        let mut exports: std::collections::HashMap<String, Value> = if exported_symbols.is_empty() {
            all_symbols.into_iter().collect()
        } else {
            all_symbols
                .into_iter()
                .filter(|(k, _)| exported_symbols.contains(k))
                .collect()
        };

        // C20B-015 / ROOT-18: Attach the facade's TypeDef / enum scope onto
        // any exported function so that `JSON[raw, Schema]()` inside a facade
        // helper resolves against the facade's own typedefs, not the
        // importing module's.
        {
            let facade_td_arc = if module_type_defs.is_empty() {
                None
            } else {
                Some(std::sync::Arc::new(module_type_defs.clone()))
            };
            let facade_ed_arc = if self.enum_defs.is_empty() {
                None
            } else {
                Some(std::sync::Arc::new(self.enum_defs.clone()))
            };
            for value in exports.values_mut() {
                if let Value::Function(fv) = value {
                    if fv.module_type_defs.is_none() {
                        fv.module_type_defs = facade_td_arc.clone();
                    }
                    if fv.module_enum_defs.is_none() {
                        fv.module_enum_defs = facade_ed_arc.clone();
                    }
                }
            }
        }

        // Persist TypeDef / method metadata for exported symbols so
        // user code can pattern-match on facade-declared types if the
        // facade ever introduces them. Today the terminal facade only
        // exports packs + aliases, but keeping the hook in place
        // avoids a future gotcha.
        let exported_type_defs: std::collections::HashMap<String, Vec<crate::parser::FieldDef>> =
            module_type_defs
                .into_iter()
                .filter(|(k, _)| exports.contains_key(k))
                .collect();
        let exported_enum_defs: std::collections::HashMap<String, Vec<String>> = self
            .enum_defs
            .iter()
            .filter(|(k, _)| exports.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let exported_type_methods: std::collections::HashMap<
            String,
            std::collections::HashMap<String, crate::parser::FuncDef>,
        > = module_type_methods
            .into_iter()
            .filter(|(k, _)| exports.contains_key(k))
            .collect();

        self.loaded_modules.insert(
            canonical,
            LoadedModule {
                exports: exports.clone(),
                type_defs: exported_type_defs,
                enum_defs: exported_enum_defs,
                type_methods: exported_type_methods,
            },
        );

        Ok(exports)
    }
}
