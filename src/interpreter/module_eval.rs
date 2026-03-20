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
        let module_path = if let Some(version) = &import.version {
            let root = self.find_project_root();
            let pkg_id = &import.path;
            if let Some(resolution) =
                crate::pkg::resolver::resolve_package_module_versioned(&root, pkg_id, version)
            {
                let path = match resolution.submodule {
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
                };
                path.canonicalize().map_err(|_| RuntimeError {
                    message: format!("Module not found: '{}'", path.display()),
                })?
            } else {
                return Err(RuntimeError {
                    message: format!(
                        "Package '{}@{}' not found. Run 'taida deps' to install dependencies.",
                        pkg_id, version
                    ),
                });
            }
        } else {
            self.resolve_module_path(&import.path)?
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
                    path_str.contains(&global_unix) || path_str.contains(&global_win)
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
                    for sym in [
                        "dnsResolve",
                        "tcpConnect",
                        "tcpListen",
                        "tcpAccept",
                        "socketSend",
                        "socketSendAll",
                        "socketRecv",
                        "socketSendBytes",
                        "socketRecvBytes",
                        "socketRecvExact",
                        "udpBind",
                        "udpSendTo",
                        "udpRecvFrom",
                        "socketClose",
                        "listenerClose",
                        "udpClose",
                    ] {
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
        let cached_type_methods = self
            .loaded_modules
            .get(&module_path)
            .map(|m| m.type_methods.clone())
            .unwrap_or_default();

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
}
