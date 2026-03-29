use super::ir::*;
/// AST → Taida IR 変換（Lowering）
use crate::parser::*;

/// 簡易フィールド名ハッシュ（FNV-1a）
pub(crate) fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

const OS_NET_DEFAULT_TIMEOUT_MS: i64 = 30_000;

pub struct Lowering {
    /// ユーザー定義関数名のセット
    user_funcs: std::collections::HashSet<String>,
    /// 関数名 → パラメータ定義（実効デフォルト補完/arity診断用）
    func_param_defs: std::collections::HashMap<String, Vec<crate::parser::Param>>,
    /// TypeDef 名 → フィールド名リスト
    type_fields: std::collections::HashMap<String, Vec<String>>,
    /// TypeDef 名 → フィールド名+型アノテーションリスト（JSON スキーマ解決用）
    pub(crate) type_field_types:
        std::collections::HashMap<String, Vec<(String, Option<crate::parser::TypeExpr>)>>,
    /// TypeDef 名 → フィールド定義（型注釈/デフォルト注入用）
    type_field_defs: std::collections::HashMap<String, Vec<crate::parser::FieldDef>>,
    /// ラムダカウンター（一意名生成用）
    lambda_counter: usize,
    /// ラムダから生成された関数（後で module に追加）
    lambda_funcs: Vec<IrFunction>,
    /// 変数名 → ラムダ関数名のマッピング（キャプチャなしラムダの直接呼び出し用）
    lambda_vars: std::collections::HashMap<String, String>,
    /// 変数名 → ラムダ関数名（キャプチャありクロージャ）
    closure_vars: std::collections::HashSet<String>,
    /// 現在の関数内でヒープに確保された変数名のリスト（RC用）
    current_heap_vars: Vec<String>,
    /// stdlib ランタイム関数: インポート名 → C ランタイム関数名
    /// std/math 等の stdlib 関数はユーザー関数ではなくランタイム関数として呼び出す
    stdlib_runtime_funcs: std::collections::HashMap<String, String>,
    /// stdlib 定数: インポート名 → f64 値
    stdlib_constants: std::collections::HashMap<String, f64>,
    /// int 値を保持する変数名のセット（FL-16: poly_add 誤発火防止）
    int_vars: std::collections::HashSet<String>,
    /// float を返す stdlib 関数の結果変数（型追跡用）
    /// float 値を保持する変数名のセット
    float_vars: std::collections::HashSet<String>,
    /// 文字列値を保持する変数名のセット
    string_vars: std::collections::HashSet<String>,
    /// bool 値を保持する変数名のセット
    bool_vars: std::collections::HashSet<String>,
    /// 現在処理中の関数名（末尾再帰検出用）
    current_func_name: Option<String>,
    /// BuchiPack フィールド名のセット（jsonEncode 用フィールド名レジストリ）
    field_names: std::collections::HashSet<String>,
    /// フィールド名 → 型タグ (0=unknown, 1=Int, 2=Float, 3=Str, 4=Bool)
    field_type_tags: std::collections::HashMap<String, i64>,
    /// Mold 定義レジストリ（custom mold lowering 用）
    pub(crate) mold_defs: std::collections::HashMap<String, crate::parser::MoldDef>,
    /// Mold 名 → solidify ヘルパー関数シンボル（mangled）
    pub(crate) mold_solidify_funcs: std::collections::HashMap<String, String>,
    /// 戻り値が Str のユーザー定義関数名セット
    string_returning_funcs: std::collections::HashSet<String>,
    /// 戻り値が Bool のユーザー定義関数名セット
    bool_returning_funcs: std::collections::HashSet<String>,
    /// 戻り値が Float のユーザー定義関数名セット
    float_returning_funcs: std::collections::HashSet<String>,
    /// NB-31: 戻り値が Int/Num のユーザー定義関数名セット
    int_returning_funcs: std::collections::HashSet<String>,
    /// BuchiPack/TypeInst を保持する変数名のセット（F-58 メソッド名衝突回避用）
    pack_vars: std::collections::HashSet<String>,
    /// BuchiPack/TypeInst を返すユーザー定義関数名セット
    pack_returning_funcs: std::collections::HashSet<String>,
    /// List を保持する変数名のセット（retain-on-store 型タグ推論用）
    list_vars: std::collections::HashSet<String>,
    /// List を返すユーザー定義関数名セット（retain-on-store 型タグ推論用）
    list_returning_funcs: std::collections::HashSet<String>,
    /// TypeDef 名 → メソッド定義リスト（メソッド名, FuncDef）
    type_method_defs: std::collections::HashMap<String, Vec<(String, crate::parser::FuncDef)>>,
    /// トップレベルで定義される変数名のセット（Native グローバル変数テーブル用）
    top_level_vars: std::collections::HashSet<String>,
    /// 関数から参照されるトップレベル変数名のセット（GlobalSet の emit 対象）
    globals_referenced: std::collections::HashSet<String>,
    /// 変数名 → TypeDef 名のマッピング（QF-10: フィールドアクセス時の型解決用）
    var_type_names: std::collections::HashMap<String, String>,
    /// ローカル関数定義でキャプチャが必要なもの: 関数名 → (ラムダ名, キャプチャ変数リスト)
    /// lower_statement の FuncDef 分岐で MakeClosure を発行するために使用
    pending_local_closures: std::collections::HashMap<String, (String, Vec<String>)>,
    /// インポートされた値シンボル（BuchiPack 等）。
    /// モジュール init 後に GlobalGet してローカル名へ束縛する。
    /// (alias名, original名, import元 module_key) のタプル
    imported_value_symbols: Vec<(String, String, String)>,
    /// init が必要な依存モジュール関数シンボルのリスト（重複排除済み）
    module_inits_needed: Vec<String>,
    /// 現在のモジュールを一意に識別するキー
    module_key: Option<String>,
    /// ライブラリモジュールかどうか（is_library の早期判定用）
    is_library_module: bool,
    /// QF-17: インポートされた TypeDef シンボル
    /// lower_type_inst で型メタデータが無い場合に constructor 呼び出しにフォールバック
    imported_type_symbols: std::collections::HashSet<String>,
    /// ソースファイルのディレクトリ（インポートモジュール解決用）
    source_dir: Option<std::path::PathBuf>,
    /// import 関数名（alias 含む）→ 実リンクシンボル名のマッピング
    imported_func_links: std::collections::HashMap<String, String>,
    /// 関数本体から参照可能な import 値名
    imported_value_names: std::collections::HashSet<String>,
    /// 現在のモジュールで export されるシンボル名
    exported_symbols: std::collections::HashSet<String>,
    /// Lax 変数の内部型追跡（変数名 → 内部型名）
    /// 例: `x <= Bool["maybe"]()` → lax_inner_types["x"] = "Bool"
    /// `x ]=> val` で val の型を正しく推定するために使用
    lax_inner_types: std::collections::HashMap<String, String>,
    /// Net builtin names shadowed by a parameter in the current function scope.
    /// When a name is here, stdlib_runtime_funcs dispatch for that name is skipped
    /// and the call is treated as a parameter/variable call instead.
    shadowed_net_builtins: std::collections::HashSet<String>,
    /// NB-14: Parameter name -> IrVar holding the runtime type tag from the caller.
    /// Used to propagate Bool/Int distinction through function boundaries.
    /// Populated at function entry via taida_get_call_arg_tag().
    param_tag_vars: std::collections::HashMap<String, IrVar>,
    /// NB-14: IrVar (CallUser result) -> IrVar (return type tag from that call).
    /// Populated after CallUser by calling taida_get_return_tag().
    /// Used to propagate type tags through function return values.
    return_tag_vars: std::collections::HashMap<IrVar, IrVar>,
    /// NB-14: When true, the current CallUser is in tail position (return value).
    /// Skip get_return_tag to preserve C compiler tail call optimization (WASM/mutual recursion).
    in_tail_call_return: bool,
    /// NB3-4: Variable alias tracking for identity assignments (e.g., `h <= handler`).
    /// Maps target variable name to source variable name.
    var_aliases: std::collections::HashMap<String, String>,
    /// NB3-4: Lambda parameter count tracking for lambda assignments (e.g., `h <= req, writer => @(...)`).
    /// Maps variable name to the number of lambda parameters.
    lambda_param_counts: std::collections::HashMap<String, usize>,
    /// NB3-4 fix: Parameter names whose type was inferred from return-type annotation
    /// (not from explicit type annotations or literal assignments).
    /// These are unreliable for callable_type_tag because the parameter might actually
    /// be a function/closure passed at runtime.
    return_type_inferred_params: std::collections::HashSet<String>,
}

#[derive(Debug)]
pub struct LowerError {
    pub message: String,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lowering error: {}", self.message)
    }
}

/// QF-16/17: インポートされたシンボルの種類
enum ImportedSymbolKind {
    /// 通常の関数定義
    Function,
    /// トップレベル代入値（BuchiPack, ラムダ等）
    Value,
    /// TypeDef / InheritanceDef
    TypeDef,
}

type InheritanceChainFields = (
    Vec<String>,
    Vec<(String, Option<crate::parser::TypeExpr>)>,
    Vec<crate::parser::FieldDef>,
    Vec<(String, crate::parser::FuncDef)>,
);

impl Default for Lowering {
    fn default() -> Self {
        Self::new()
    }
}

impl Lowering {
    pub fn new() -> Self {
        let mut stdlib_runtime_funcs = std::collections::HashMap::new();
        // Prelude I/O functions — available without import
        stdlib_runtime_funcs.insert("stdout".to_string(), "taida_io_stdout".to_string());
        stdlib_runtime_funcs.insert("stderr".to_string(), "taida_io_stderr".to_string());
        stdlib_runtime_funcs.insert("stdin".to_string(), "taida_io_stdin".to_string());
        // Prelude JSON functions (output-direction only)
        stdlib_runtime_funcs.insert("jsonEncode".to_string(), "taida_json_encode".to_string());
        stdlib_runtime_funcs.insert("jsonPretty".to_string(), "taida_json_pretty".to_string());
        // Prelude time functions (minimal kernel)
        stdlib_runtime_funcs.insert("nowMs".to_string(), "taida_time_now_ms".to_string());
        stdlib_runtime_funcs.insert("sleep".to_string(), "taida_time_sleep".to_string());
        // Prelude constructors — ABOLISHED: Some/None/Ok/Err/Optional (v0.8.0)
        // Use Lax[value]() and Result[value]() mold syntax.
        // Prelude collection constructors
        stdlib_runtime_funcs.insert("hashMap".to_string(), "taida_hashmap_new".to_string());
        stdlib_runtime_funcs.insert("setOf".to_string(), "taida_set_from_list".to_string());
        // Core-bundled os side-effect/query/async functions (import-less parity with interpreter/JS)
        stdlib_runtime_funcs.insert("readBytes".to_string(), "taida_os_read_bytes".to_string());
        stdlib_runtime_funcs.insert("writeFile".to_string(), "taida_os_write_file".to_string());
        stdlib_runtime_funcs.insert("writeBytes".to_string(), "taida_os_write_bytes".to_string());
        stdlib_runtime_funcs.insert("appendFile".to_string(), "taida_os_append_file".to_string());
        stdlib_runtime_funcs.insert("remove".to_string(), "taida_os_remove".to_string());
        stdlib_runtime_funcs.insert("createDir".to_string(), "taida_os_create_dir".to_string());
        stdlib_runtime_funcs.insert("rename".to_string(), "taida_os_rename".to_string());
        stdlib_runtime_funcs.insert("run".to_string(), "taida_os_run".to_string());
        stdlib_runtime_funcs.insert("execShell".to_string(), "taida_os_exec_shell".to_string());
        stdlib_runtime_funcs.insert("allEnv".to_string(), "taida_os_all_env".to_string());
        stdlib_runtime_funcs.insert("argv".to_string(), "taida_os_argv".to_string());
        stdlib_runtime_funcs.insert("dnsResolve".to_string(), "taida_os_dns_resolve".to_string());
        stdlib_runtime_funcs.insert("tcpConnect".to_string(), "taida_os_tcp_connect".to_string());
        stdlib_runtime_funcs.insert("tcpListen".to_string(), "taida_os_tcp_listen".to_string());
        stdlib_runtime_funcs.insert("tcpAccept".to_string(), "taida_os_tcp_accept".to_string());
        stdlib_runtime_funcs.insert("socketSend".to_string(), "taida_os_socket_send".to_string());
        stdlib_runtime_funcs.insert(
            "socketSendAll".to_string(),
            "taida_os_socket_send_all".to_string(),
        );
        stdlib_runtime_funcs.insert("socketRecv".to_string(), "taida_os_socket_recv".to_string());
        stdlib_runtime_funcs.insert(
            "socketSendBytes".to_string(),
            "taida_os_socket_send_bytes".to_string(),
        );
        stdlib_runtime_funcs.insert(
            "socketRecvBytes".to_string(),
            "taida_os_socket_recv_bytes".to_string(),
        );
        stdlib_runtime_funcs.insert(
            "socketRecvExact".to_string(),
            "taida_os_socket_recv_exact".to_string(),
        );
        stdlib_runtime_funcs.insert("udpBind".to_string(), "taida_os_udp_bind".to_string());
        stdlib_runtime_funcs.insert("udpSendTo".to_string(), "taida_os_udp_send_to".to_string());
        stdlib_runtime_funcs.insert(
            "udpRecvFrom".to_string(),
            "taida_os_udp_recv_from".to_string(),
        );
        stdlib_runtime_funcs.insert(
            "socketClose".to_string(),
            "taida_os_socket_close".to_string(),
        );
        stdlib_runtime_funcs.insert(
            "listenerClose".to_string(),
            "taida_os_listener_close".to_string(),
        );
        stdlib_runtime_funcs.insert("udpClose".to_string(), "taida_os_socket_close".to_string());
        // Core-bundled pool functions
        stdlib_runtime_funcs.insert("poolCreate".to_string(), "taida_pool_create".to_string());
        stdlib_runtime_funcs.insert("poolAcquire".to_string(), "taida_pool_acquire".to_string());
        stdlib_runtime_funcs.insert("poolRelease".to_string(), "taida_pool_release".to_string());
        stdlib_runtime_funcs.insert("poolClose".to_string(), "taida_pool_close".to_string());
        stdlib_runtime_funcs.insert("poolHealth".to_string(), "taida_pool_health".to_string());
        Self {
            user_funcs: std::collections::HashSet::new(),
            func_param_defs: std::collections::HashMap::new(),
            type_fields: std::collections::HashMap::new(),
            type_field_types: std::collections::HashMap::new(),
            type_field_defs: std::collections::HashMap::new(),
            lambda_counter: 0,
            lambda_funcs: Vec::new(),
            lambda_vars: std::collections::HashMap::new(),
            closure_vars: std::collections::HashSet::new(),
            current_heap_vars: Vec::new(),
            stdlib_runtime_funcs,
            stdlib_constants: std::collections::HashMap::new(),
            int_vars: std::collections::HashSet::new(),
            float_vars: std::collections::HashSet::new(),
            string_vars: std::collections::HashSet::new(),
            bool_vars: std::collections::HashSet::new(),
            current_func_name: None,
            field_names: std::collections::HashSet::new(),
            field_type_tags: std::collections::HashMap::new(),
            mold_defs: std::collections::HashMap::new(),
            mold_solidify_funcs: std::collections::HashMap::new(),
            string_returning_funcs: std::collections::HashSet::new(),
            bool_returning_funcs: std::collections::HashSet::new(),
            float_returning_funcs: std::collections::HashSet::new(),
            int_returning_funcs: std::collections::HashSet::new(),
            pack_vars: std::collections::HashSet::new(),
            pack_returning_funcs: std::collections::HashSet::new(),
            list_vars: std::collections::HashSet::new(),
            list_returning_funcs: std::collections::HashSet::new(),
            type_method_defs: std::collections::HashMap::new(),
            top_level_vars: std::collections::HashSet::new(),
            globals_referenced: std::collections::HashSet::new(),
            var_type_names: std::collections::HashMap::new(),
            pending_local_closures: std::collections::HashMap::new(),
            imported_value_symbols: Vec::new(),
            module_inits_needed: Vec::new(),
            module_key: None,
            is_library_module: false,
            imported_type_symbols: std::collections::HashSet::new(),
            source_dir: None,
            imported_func_links: std::collections::HashMap::new(),
            imported_value_names: std::collections::HashSet::new(),
            exported_symbols: std::collections::HashSet::new(),
            lax_inner_types: std::collections::HashMap::new(),
            shadowed_net_builtins: std::collections::HashSet::new(),
            param_tag_vars: std::collections::HashMap::new(),
            return_tag_vars: std::collections::HashMap::new(),
            in_tail_call_return: false,
            var_aliases: std::collections::HashMap::new(),
            lambda_param_counts: std::collections::HashMap::new(),
            return_type_inferred_params: std::collections::HashSet::new(),
        }
    }

    /// QF-16/17: ソースファイルのディレクトリを設定（インポートモジュール解決用）
    pub fn set_source_dir(&mut self, dir: std::path::PathBuf) {
        self.source_dir = Some(dir);
    }

    pub fn set_module_key(&mut self, key: String) {
        self.module_key = Some(key);
    }

    pub fn module_key_for_path(path: &std::path::Path) -> String {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        format!("m{:016x}", simple_hash(&canonical.to_string_lossy()))
    }

    fn current_module_key(&self) -> &str {
        self.module_key
            .as_deref()
            .expect("module_key must be set before lowering")
    }

    fn export_func_symbol_for_key(module_key: &str, name: &str) -> String {
        format!("_taida_fn_{}_{}", module_key, name)
    }

    fn export_func_symbol(&self, name: &str) -> String {
        Self::export_func_symbol_for_key(self.current_module_key(), name)
    }

    fn init_symbol_for_key(module_key: &str) -> String {
        format!("_taida_init_{}", module_key)
    }

    fn init_symbol(&self) -> String {
        Self::init_symbol_for_key(self.current_module_key())
    }

    /// グローバル変数のハッシュキーを計算する。
    /// ライブラリモジュールの場合は "module_key:var_name" で名前空間化する。
    fn global_var_hash(&self, var_name: &str) -> i64 {
        if let Some(ref module_key) = self.module_key
            && self.is_library_module
        {
            return simple_hash(&format!("{}:{}", module_key, var_name)) as i64;
        }
        simple_hash(var_name) as i64
    }

    fn fallback_module_key(path: &str) -> String {
        format!("m{:016x}", simple_hash(path))
    }

    fn resolve_import_path(
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
    fn find_project_root(start_dir: &std::path::Path) -> std::path::PathBuf {
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

    fn import_module_key(&self, module_path: &str, version: Option<&str>) -> String {
        self.resolve_import_path(module_path, version)
            .map(|path| Self::module_key_for_path(&path))
            .unwrap_or_else(|| Self::fallback_module_key(module_path))
    }

    fn resolve_user_func_symbol(&self, name: &str) -> String {
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
    fn collect_module_export_list(
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

    fn classify_imported_symbol(
        &self,
        module_path: &str,
        symbol_name: &str,
        version: Option<&str>,
    ) -> Result<ImportedSymbolKind, LowerError> {
        // モジュールパスを解決
        let path = match self.resolve_import_path(module_path, version) {
            Some(path) => path,
            None => return Ok(ImportedSymbolKind::Function),
        };

        // ソースを読み込んでパース
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Ok(ImportedSymbolKind::Function),
        };
        let (program, _) = crate::parser::parse(&source);

        // RCB-201: Check if the module has explicit exports (<<<).
        // If it does, verify the symbol is in the export list before classifying.
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

        // シンボルの種類を判定
        for stmt in &program.statements {
            match stmt {
                Statement::FuncDef(func_def) if func_def.name == symbol_name => {
                    return Ok(ImportedSymbolKind::Function);
                }
                Statement::TypeDef(type_def) if type_def.name == symbol_name => {
                    // 種類判定のみ。メタデータ登録は register_imported_typedef で行う。
                    return Ok(ImportedSymbolKind::TypeDef);
                }
                Statement::InheritanceDef(inh_def) if inh_def.child == symbol_name => {
                    return Ok(ImportedSymbolKind::TypeDef);
                }
                Statement::Assignment(assign) if assign.target == symbol_name => {
                    return Ok(ImportedSymbolKind::Value);
                }
                _ => {}
            }
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
    fn register_imported_typedef(
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
    fn collect_inheritance_chain_fields(
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

    /// stdout/stderr/stdin → C ランタイム関数名にマッピング (prelude builtins)
    fn stdlib_io_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "stdout" => Some("taida_io_stdout"),
            "stderr" => Some("taida_io_stderr"),
            "stdin" => Some("taida_io_stdin"),
            _ => None,
        }
    }

    /// Register a field type tag, detecting conflicts.
    /// If the same field name is used with different types across different
    /// BuchiPack/Mold definitions (e.g., `Todo.status: Str` vs `HttpResp.status: Int`),
    /// the tag is set to 0 (unknown) so the JSON serializer falls back to
    /// runtime heuristic type detection instead of using the wrong type.
    fn register_field_type_tag(&mut self, name: &str, tag: i64) {
        if tag == 0 {
            return;
        }
        if let Some(&existing) = self.field_type_tags.get(name) {
            if existing != tag && existing != 0 {
                // Conflict: same field name used with different types.
                // Set to 0 (unknown) to force runtime heuristic detection.
                self.field_type_tags.insert(name.to_string(), 0);
            }
            // If existing == tag, no change needed (same type, idempotent).
            // If existing == 0, already conflicted, leave it.
        } else {
            self.field_type_tags.insert(name.to_string(), tag);
        }
    }

    /// taida-lang/os package function → C ランタイム関数名にマッピング
    fn os_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "readBytes" => Some("taida_os_read_bytes"),
            "writeFile" => Some("taida_os_write_file"),
            "writeBytes" => Some("taida_os_write_bytes"),
            "appendFile" => Some("taida_os_append_file"),
            "remove" => Some("taida_os_remove"),
            "createDir" => Some("taida_os_create_dir"),
            "rename" => Some("taida_os_rename"),
            "run" => Some("taida_os_run"),
            "execShell" => Some("taida_os_exec_shell"),
            "allEnv" => Some("taida_os_all_env"),
            "argv" => Some("taida_os_argv"),
            "dnsResolve" => Some("taida_os_dns_resolve"),
            // Phase 2: async TCP functions
            "tcpConnect" => Some("taida_os_tcp_connect"),
            "tcpListen" => Some("taida_os_tcp_listen"),
            "tcpAccept" => Some("taida_os_tcp_accept"),
            "socketSend" => Some("taida_os_socket_send"),
            "socketSendAll" => Some("taida_os_socket_send_all"),
            "socketRecv" => Some("taida_os_socket_recv"),
            "socketSendBytes" => Some("taida_os_socket_send_bytes"),
            "socketRecvBytes" => Some("taida_os_socket_recv_bytes"),
            "socketRecvExact" => Some("taida_os_socket_recv_exact"),
            "udpBind" => Some("taida_os_udp_bind"),
            "udpSendTo" => Some("taida_os_udp_send_to"),
            "udpRecvFrom" => Some("taida_os_udp_recv_from"),
            "socketClose" => Some("taida_os_socket_close"),
            "listenerClose" => Some("taida_os_listener_close"),
            "udpClose" => Some("taida_os_socket_close"),
            // Mold names (Read, ListDir, Stat, Exists, EnvVar, ReadAsync, Http*) are handled in lower_molds.rs
            "Read" | "ListDir" | "Stat" | "Exists" | "EnvVar" | "ReadAsync" | "HttpGet"
            | "HttpPost" | "HttpRequest" => Some("__os_mold__"),
            _ => None,
        }
    }

    /// taida-lang/crypto package function → C runtime function mapping.
    fn crypto_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "sha256" => Some("taida_sha256"),
            _ => None,
        }
    }

    /// taida-lang/net package function → C runtime function mapping.
    /// Names of taida-lang/net builtins that require scope-aware dispatch.
    /// HTTP v1 (3) + HTTP v2 (1) + HTTP v3 (4) = 8.
    const NET_BUILTIN_NAMES: &'static [&'static str] = &[
        "httpServe",
        "httpParseRequestHead",
        "httpEncodeResponse",
        "readBody",
        "startResponse",
        "writeChunk",
        "endResponse",
        "sseEvent",
    ];

    /// Check if a name is a net HTTP v1 builtin that is currently shadowed by a parameter.
    fn is_net_builtin_shadowed(&self, name: &str) -> bool {
        self.shadowed_net_builtins.contains(name)
    }

    /// Legacy surface reuses the existing socket runtime path.
    /// HTTP v1 surface maps to dedicated taida_net_* runtime functions.
    fn net_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            // Legacy surface (shared with os)
            "dnsResolve" => Some("taida_os_dns_resolve"),
            "tcpConnect" => Some("taida_os_tcp_connect"),
            "tcpListen" => Some("taida_os_tcp_listen"),
            "tcpAccept" => Some("taida_os_tcp_accept"),
            "socketSend" => Some("taida_os_socket_send"),
            "socketSendAll" => Some("taida_os_socket_send_all"),
            "socketRecv" => Some("taida_os_socket_recv"),
            "socketSendBytes" => Some("taida_os_socket_send_bytes"),
            "socketRecvBytes" => Some("taida_os_socket_recv_bytes"),
            "socketRecvExact" => Some("taida_os_socket_recv_exact"),
            "udpBind" => Some("taida_os_udp_bind"),
            "udpSendTo" => Some("taida_os_udp_send_to"),
            "udpRecvFrom" => Some("taida_os_udp_recv_from"),
            "socketClose" => Some("taida_os_socket_close"),
            "listenerClose" => Some("taida_os_listener_close"),
            "udpClose" => Some("taida_os_socket_close"),
            // HTTP v1 surface
            "httpServe" => Some("taida_net_http_serve"),
            "httpParseRequestHead" => Some("taida_net_http_parse_request_head"),
            "httpEncodeResponse" => Some("taida_net_http_encode_response"),
            // HTTP v2 surface
            "readBody" => Some("taida_net_read_body"),
            // HTTP v3 streaming surface
            "startResponse" => Some("taida_net_start_response"),
            "writeChunk" => Some("taida_net_write_chunk"),
            "endResponse" => Some("taida_net_end_response"),
            "sseEvent" => Some("taida_net_sse_event"),
            // HTTP v4 request body streaming surface
            "readBodyChunk" => Some("taida_net_read_body_chunk"),
            "readBodyAll" => Some("taida_net_read_body_all"),
            // HTTP v4 WebSocket surface
            "wsUpgrade" => Some("taida_net_ws_upgrade"),
            "wsSend" => Some("taida_net_ws_send"),
            "wsReceive" => Some("taida_net_ws_receive"),
            "wsClose" => Some("taida_net_ws_close"),
            _ => None,
        }
    }

    /// taida-lang/pool package function → C runtime function mapping.
    fn pool_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "poolCreate" => Some("taida_pool_create"),
            "poolAcquire" => Some("taida_pool_acquire"),
            "poolRelease" => Some("taida_pool_release"),
            "poolClose" => Some("taida_pool_close"),
            "poolHealth" => Some("taida_pool_health"),
            _ => None,
        }
    }

    fn mold_solidify_helper_name(mold_name: &str) -> String {
        format!("__taida_mold_solidify_{}", mold_name)
    }

    fn register_mold_solidify_helpers(&mut self) -> Result<(), LowerError> {
        let mut mold_defs: Vec<crate::parser::MoldDef> = self.mold_defs.values().cloned().collect();
        mold_defs.sort_by(|a, b| a.name.cmp(&b.name));

        // Register helper symbols first, so recursive mold references can resolve.
        for mold_def in &mold_defs {
            let has_solidify = mold_def
                .fields
                .iter()
                .any(|f| f.is_method && f.name == "solidify" && f.method_def.is_some());
            if has_solidify {
                let helper_raw = Self::mold_solidify_helper_name(&mold_def.name);
                let helper_symbol = format!("_taida_fn_{}", helper_raw);
                self.mold_solidify_funcs
                    .insert(mold_def.name.clone(), helper_symbol);
            }
        }

        for mold_def in mold_defs {
            let Some(solidify_method) = mold_def
                .fields
                .iter()
                .find(|f| f.is_method && f.name == "solidify")
                .and_then(|f| f.method_def.clone())
            else {
                continue;
            };

            if !solidify_method.params.is_empty() {
                return Err(LowerError {
                    message: format!(
                        "Native backend does not support solidify method parameters on mold '{}'",
                        mold_def.name
                    ),
                });
            }

            let non_method_fields: Vec<crate::parser::FieldDef> = mold_def
                .fields
                .iter()
                .filter(|f| !f.is_method)
                .cloned()
                .collect();
            let required_fields: Vec<crate::parser::FieldDef> = non_method_fields
                .iter()
                .filter(|f| f.name != "filling" && f.default_value.is_none())
                .cloned()
                .collect();
            let optional_fields: Vec<crate::parser::FieldDef> = non_method_fields
                .iter()
                .filter(|f| f.name != "filling" && f.default_value.is_some())
                .cloned()
                .collect();

            let mut params = Vec::<crate::parser::Param>::new();
            let mut seen = std::collections::HashSet::<String>::new();
            let mut push_param = |name: &str| {
                if seen.insert(name.to_string()) {
                    params.push(crate::parser::Param {
                        name: name.to_string(),
                        type_annotation: None,
                        default_value: None,
                        span: mold_def.span.clone(),
                    });
                }
            };
            push_param("filling");
            for field in &required_fields {
                push_param(&field.name);
            }
            for field in &optional_fields {
                push_param(&field.name);
            }
            push_param("self");

            let helper_raw = Self::mold_solidify_helper_name(&mold_def.name);
            let synthetic = crate::parser::FuncDef {
                name: helper_raw,
                type_params: Vec::new(),
                params,
                body: solidify_method.body.clone(),
                return_type: solidify_method.return_type.clone(),
                doc_comments: Vec::new(),
                span: mold_def.span.clone(),
            };
            let helper_ir = self.lower_func_def(&synthetic)?;
            self.lambda_funcs.push(helper_ir);
        }

        Ok(())
    }

    pub fn lower_program(&mut self, program: &Program) -> Result<IrModule, LowerError> {
        let mut module = IrModule::new();
        module.module_key = Some(self.current_module_key().to_string());

        // 1st pass: 関数定義、型定義、エクスポート/インポートを収集
        for stmt in &program.statements {
            match stmt {
                Statement::FuncDef(func_def) => {
                    self.user_funcs.insert(func_def.name.clone());
                    self.func_param_defs
                        .insert(func_def.name.clone(), func_def.params.clone());
                    // Track return types for type inference in binary ops
                    if let Some(ref rt) = func_def.return_type {
                        match rt {
                            crate::parser::TypeExpr::Named(n) if n == "Str" => {
                                self.string_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::Named(n) if n == "Bool" => {
                                self.bool_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::Named(n) if n == "Float" => {
                                self.float_returning_funcs.insert(func_def.name.clone());
                            }
                            // NB-31: Track Int/Num-returning functions for callable_type_tag
                            crate::parser::TypeExpr::Named(n) if n == "Int" || n == "Num" => {
                                self.int_returning_funcs.insert(func_def.name.clone());
                            }
                            crate::parser::TypeExpr::List(_) => {
                                self.list_returning_funcs.insert(func_def.name.clone());
                            }
                            _ => {}
                        }
                    }
                    // F-58/F-60: Detect functions that return BuchiPack/TypeInst
                    if Self::func_body_returns_pack(&func_def.body) {
                        self.pack_returning_funcs.insert(func_def.name.clone());
                    }
                    // retain-on-store: Detect functions that return List
                    if Self::func_body_returns_list(&func_def.body) {
                        self.list_returning_funcs.insert(func_def.name.clone());
                    }
                }
                Statement::TypeDef(type_def) => {
                    let non_method_field_defs: Vec<crate::parser::FieldDef> = type_def
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .cloned()
                        .collect();
                    let fields: Vec<String> = type_def
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .map(|f| f.name.clone())
                        .collect();
                    // Register field names and types for jsonEncode
                    for field_def in &non_method_field_defs {
                        self.field_names.insert(field_def.name.clone());
                        // Map type annotation to type tag
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
                        } else if let Some(ref default_expr) = field_def.default_value {
                            // Infer type from default value expression
                            if self.expr_is_bool(default_expr) {
                                self.register_field_type_tag(&field_def.name, 4);
                                // Bool
                            }
                        }
                    }
                    self.type_fields.insert(type_def.name.clone(), fields);
                    // JSON スキーマ解決用: フィールド名+型アノテーション
                    let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                        non_method_field_defs
                            .iter()
                            .map(|f| (f.name.clone(), f.type_annotation.clone()))
                            .collect();
                    self.type_field_types
                        .insert(type_def.name.clone(), field_types);
                    self.type_field_defs
                        .insert(type_def.name.clone(), non_method_field_defs);
                    // Register method definitions for TypeDef method closure generation
                    let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                        .fields
                        .iter()
                        .filter(|f| f.is_method && f.method_def.is_some())
                        .map(|f| (f.name.clone(), f.method_def.clone().unwrap()))
                        .collect();
                    if !methods.is_empty() {
                        self.type_method_defs.insert(type_def.name.clone(), methods);
                    }
                }
                Statement::MoldDef(mold_def) => {
                    let non_method_field_defs: Vec<crate::parser::FieldDef> = mold_def
                        .fields
                        .iter()
                        .filter(|f| !f.is_method)
                        .cloned()
                        .collect();
                    let fields: Vec<String> = non_method_field_defs
                        .iter()
                        .map(|f| f.name.clone())
                        .collect();
                    for field_def in &non_method_field_defs {
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
                        } else if let Some(ref default_expr) = field_def.default_value
                            && self.expr_is_bool(default_expr)
                        {
                            self.register_field_type_tag(&field_def.name, 4);
                        }
                    }
                    self.type_fields.insert(mold_def.name.clone(), fields);
                    let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                        non_method_field_defs
                            .iter()
                            .map(|f| (f.name.clone(), f.type_annotation.clone()))
                            .collect();
                    self.type_field_types
                        .insert(mold_def.name.clone(), field_types);
                    self.type_field_defs
                        .insert(mold_def.name.clone(), non_method_field_defs);
                    self.mold_defs
                        .insert(mold_def.name.clone(), mold_def.clone());
                }
                Statement::InheritanceDef(inh_def) => {
                    let mut all_fields = self
                        .type_fields
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    let mut all_field_types = self
                        .type_field_types
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    let mut all_field_defs = self
                        .type_field_defs
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    for field in inh_def.fields.iter().filter(|f| !f.is_method) {
                        all_fields.retain(|name| name != &field.name);
                        all_fields.push(field.name.clone());
                        all_field_types.retain(|(name, _)| name != &field.name);
                        all_field_types.push((field.name.clone(), field.type_annotation.clone()));
                        all_field_defs.retain(|f| f.name != field.name);
                        all_field_defs.push(field.clone());
                    }
                    self.type_fields.insert(inh_def.child.clone(), all_fields);
                    self.type_field_types
                        .insert(inh_def.child.clone(), all_field_types);
                    self.type_field_defs
                        .insert(inh_def.child.clone(), all_field_defs);
                    // Inherit parent methods, then override/add child methods
                    let mut all_methods = self
                        .type_method_defs
                        .get(&inh_def.parent)
                        .cloned()
                        .unwrap_or_default();
                    for field in inh_def
                        .fields
                        .iter()
                        .filter(|f| f.is_method && f.method_def.is_some())
                    {
                        all_methods.retain(|(name, _)| name != &field.name);
                        all_methods.push((field.name.clone(), field.method_def.clone().unwrap()));
                    }
                    if !all_methods.is_empty() {
                        self.type_method_defs
                            .insert(inh_def.child.clone(), all_methods);
                    }
                    if let Some(parent_mold) = self.mold_defs.get(&inh_def.parent).cloned() {
                        let mut merged_mold_fields = parent_mold.fields.clone();
                        for child_field in &inh_def.fields {
                            if let Some(existing) = merged_mold_fields
                                .iter_mut()
                                .find(|field| field.name == child_field.name)
                            {
                                *existing = child_field.clone();
                            } else {
                                merged_mold_fields.push(child_field.clone());
                            }
                        }
                        self.mold_defs.insert(
                            inh_def.child.clone(),
                            crate::parser::MoldDef {
                                name: inh_def.child.clone(),
                                mold_args: parent_mold.mold_args.clone(),
                                name_args: inh_def
                                    .child_args
                                    .clone()
                                    .or_else(|| inh_def.parent_args.clone())
                                    .or(parent_mold.name_args.clone()),
                                type_params: parent_mold.type_params.clone(),
                                fields: merged_mold_fields,
                                doc_comments: inh_def.doc_comments.clone(),
                                span: inh_def.span.clone(),
                            },
                        );
                    }
                }
                Statement::Export(export_stmt) => {
                    // RCB-212: Re-export path `<<< ./path` is not supported.
                    if export_stmt.path.is_some() {
                        return Err(LowerError {
                            message: "Re-export with path (`<<< ./path`) is not yet supported. \
                                     Use explicit import and re-export instead."
                                .to_string(),
                        });
                    }
                    for sym in &export_stmt.symbols {
                        self.exported_symbols.insert(sym.clone());
                        module.exports.push(self.export_func_symbol(sym));
                    }
                }
                Statement::Import(import_stmt) => {
                    // stdlib モジュールの関数はランタイム関数にマッピング
                    // 定数は stdlib_constants にマッピング
                    // RCB-213: version is now passed through to resolve_import_path
                    // for version-aware package resolution (.taida/deps/org/name@ver/).
                    let path = &import_stmt.path;
                    let version = import_stmt.version.as_deref();
                    let is_core_bundled_path = matches!(
                        path.as_str(),
                        "taida-lang/os"
                            | "taida-lang/js"
                            | "taida-lang/crypto"
                            | "taida-lang/net"
                            | "taida-lang/pool"
                    );
                    let mut import_link_symbols = Vec::new();
                    let mut needs_module_object = false;
                    for sym in &import_stmt.symbols {
                        let orig_name = &sym.name;
                        let alias = sym.alias.clone().unwrap_or_else(|| sym.name.clone());

                        // std/ imports: only std/io is still supported (backward compat)
                        // std/math, std/time, etc. are removed after std dissolution
                        if path.starts_with("std/") && path != "std/io" {
                            // Skip removed std modules silently
                            continue;
                        }

                        // 関数チェック
                        let runtime_name = match path.as_str() {
                            "std/io" => Self::stdlib_io_mapping(orig_name),
                            "taida-lang/os" => Self::os_func_mapping(orig_name),
                            "taida-lang/crypto" => Self::crypto_func_mapping(orig_name),
                            "taida-lang/net" => Self::net_func_mapping(orig_name),
                            "taida-lang/pool" => Self::pool_func_mapping(orig_name),
                            _ => None,
                        };

                        if let Some(rt_name) = runtime_name {
                            self.stdlib_runtime_funcs.insert(alias, rt_name.to_string());
                        } else if is_core_bundled_path {
                            // Core-bundled symbols that do not have native runtime mapping yet
                            // are intentionally skipped here (e.g. pool contract placeholders).
                            // This prevents unresolved pseudo-user-function stubs.
                            continue;
                        } else {
                            // stdlib でないか、マッピングのない関数はユーザー関数として登録
                            // QF-16/17: シンボルの種類に応じて処理を分岐
                            // RCB-201: classify now validates against module's export list
                            let sym_kind =
                                self.classify_imported_symbol(path, orig_name, version)?;
                            let module_key = self.import_module_key(path, version);
                            let init_symbol = Self::init_symbol_for_key(&module_key);
                            needs_module_object = true;
                            match sym_kind {
                                ImportedSymbolKind::Value => {
                                    // 値 export: module init 後に GlobalGet で取得
                                    self.imported_value_symbols.push((
                                        alias.clone(),
                                        orig_name.clone(),
                                        module_key,
                                    ));
                                    self.imported_value_names.insert(alias.clone());
                                    self.pack_vars.insert(alias.clone());
                                    // user_funcs には入れない（UseVar で解決する）
                                    // init 関数を呼ぶ必要がある
                                    if !self.module_inits_needed.contains(&init_symbol) {
                                        self.module_inits_needed.push(init_symbol);
                                    }
                                }
                                ImportedSymbolKind::TypeDef => {
                                    // TypeDef export: メタデータを登録（インラインで TypeInst 構築）
                                    self.imported_type_symbols.insert(alias.clone());
                                    self.register_imported_typedef(
                                        path, orig_name, &alias, version,
                                    );
                                }
                                ImportedSymbolKind::Function => {
                                    // 通常の関数 export
                                    let link_name =
                                        Self::export_func_symbol_for_key(&module_key, orig_name);
                                    self.user_funcs.insert(alias.clone());
                                    self.imported_func_links
                                        .insert(alias.clone(), link_name.clone());
                                    import_link_symbols.push(link_name);
                                    // 関数 import がある場合も init 関数を呼ぶ必要がある
                                    // （関数が参照する private value の初期化のため）
                                    if !self.module_inits_needed.contains(&init_symbol) {
                                        self.module_inits_needed.push(init_symbol);
                                    }
                                }
                            }
                        }
                    }

                    // ローカルモジュール依存は、値/TypeDef import だけでも object を生成する必要がある。
                    if needs_module_object {
                        module.imports.push((
                            import_stmt.path.clone(),
                            import_link_symbols,
                            import_stmt.version.clone(),
                        ));
                    }
                }
                _ => {}
            }
        }

        self.register_mold_solidify_helpers()?;

        // Pre-2nd pass: トップレベル変数名と型情報を収集（Native グローバル変数テーブル用）
        for stmt in &program.statements {
            if let Statement::Assignment(assign) = stmt {
                self.top_level_vars.insert(assign.target.clone());
                // 型情報を事前登録（2nd pass の lower_func_def 内で正しく型判定するため）
                if self.expr_is_int(&assign.value) {
                    self.int_vars.insert(assign.target.clone());
                }
                if self.expr_is_string_full(&assign.value) {
                    self.string_vars.insert(assign.target.clone());
                }
                if self.expr_returns_float(&assign.value) {
                    self.float_vars.insert(assign.target.clone());
                }
                if self.expr_is_bool(&assign.value) {
                    self.bool_vars.insert(assign.target.clone());
                }
                if self.expr_is_pack(&assign.value) {
                    self.pack_vars.insert(assign.target.clone());
                }
                if self.expr_is_list(&assign.value) {
                    self.list_vars.insert(assign.target.clone());
                }
                // QF-34: MoldInst の Lax 内部型を追跡（unmold 時の型推定用）
                if let Expr::MoldInst(mold_name, _, _, _) = &assign.value {
                    self.lax_inner_types
                        .insert(assign.target.clone(), mold_name.clone());
                }
                // QF-10: TypeInst の変数に TypeDef 名を記録
                if let Expr::TypeInst(type_name, _, _) = &assign.value {
                    self.var_type_names
                        .insert(assign.target.clone(), type_name.clone());
                }
            }
        }

        // ライブラリモジュール判定（2nd pass の前に実施 — is_library_module フラグが必要）
        module.is_library = !module.exports.is_empty();
        self.is_library_module = module.is_library;

        // 2nd pass: ユーザー定義関数を IR に変換
        for stmt in &program.statements {
            if let Statement::FuncDef(func_def) = stmt {
                let ir_func = self.lower_func_def(func_def)?;
                module.functions.push(ir_func);
            }
        }

        // ライブラリモジュールの場合、モジュール単位の init 関数を生成
        if module.is_library {
            self.generate_module_init_func(&mut module, program)?;
        }

        // 3rd pass: トップレベル文を _taida_main に変換（ライブラリでない場合のみ）
        if !module.is_library {
            self.current_heap_vars.clear();
            let mut main_fn = IrFunction::new("_taida_main".to_string());

            self.emit_imported_module_inits(&mut main_fn);
            self.bind_imported_values(&mut main_fn);

            let top_level_stmts: Vec<&Statement> = program
                .statements
                .iter()
                .filter(|s| !matches!(s, Statement::FuncDef(_)))
                .collect();
            self.lower_statement_sequence(&mut main_fn, &top_level_stmts)?;

            // Emit field name registrations for jsonEncode (after all field names collected)
            // Prepend to the beginning of _taida_main body
            let mut reg_insts = Vec::new();
            let mut sorted_names: Vec<String> = self.field_names.iter().cloned().collect();
            sorted_names.sort(); // deterministic order
            for name in &sorted_names {
                let hash = simple_hash(name);
                let type_tag = self.field_type_tags.get(name).copied().unwrap_or(0);
                if type_tag > 0 {
                    // Use register_field_type (with type tag)
                    let hash_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(hash_var, hash as i64));
                    let name_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstStr(name_var, name.clone()));
                    let tag_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(tag_var, type_tag));
                    let result_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::Call(
                        result_var,
                        "taida_register_field_type".to_string(),
                        vec![hash_var, name_var, tag_var],
                    ));
                } else {
                    // Use register_field_name (no type info)
                    let hash_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstInt(hash_var, hash as i64));
                    let name_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::ConstStr(name_var, name.clone()));
                    let result_var = main_fn.alloc_var();
                    reg_insts.push(IrInst::Call(
                        result_var,
                        "taida_register_field_name".to_string(),
                        vec![hash_var, name_var],
                    ));
                }
            }
            if !reg_insts.is_empty() {
                let body = std::mem::take(&mut main_fn.body);
                main_fn.body = reg_insts;
                main_fn.body.extend(body);
            }

            // _taida_main 終了時: 全ヒープ変数を Release
            let heap_vars = std::mem::take(&mut self.current_heap_vars);
            for name in &heap_vars {
                let use_var = main_fn.alloc_var();
                main_fn.push(IrInst::UseVar(use_var, name.clone()));
                main_fn.push(IrInst::Release(use_var));
            }

            let ret_var = main_fn.alloc_var();
            main_fn.push(IrInst::ConstInt(ret_var, 0));
            main_fn.push(IrInst::Return(ret_var));

            module.functions.push(main_fn);
        }

        // ラムダから生成された関数を追加
        for lambda_fn in std::mem::take(&mut self.lambda_funcs) {
            module.functions.push(lambda_fn);
        }

        Ok(module)
    }

    fn lower_func_def(&mut self, func_def: &FuncDef) -> Result<IrFunction, LowerError> {
        let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
        let parent_scope_vars = self.collect_nested_scope_vars(params.clone(), &func_def.body);

        let mangled = self.resolve_user_func_symbol(&func_def.name);
        let mut ir_func = IrFunction::new_with_params(mangled, params.clone());

        // Scope-aware net builtin shadowing: snapshot before function body,
        // restore after. This covers both parameter shadows and local assignment
        // shadows (e.g. `httpServe <= add`) within the function scope.
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        // NB3-4: Snapshot var_aliases and lambda_param_counts so that aliases
        // defined inside this function do not leak to sibling/parent scopes.
        let prev_var_aliases = self.var_aliases.clone();
        let prev_lambda_param_counts = self.lambda_param_counts.clone();
        let prev_lambda_vars = self.lambda_vars.clone();
        let prev_closure_vars = self.closure_vars.clone();
        for p in &func_def.params {
            if Self::NET_BUILTIN_NAMES.contains(&p.name.as_str()) {
                self.shadowed_net_builtins.insert(p.name.clone());
            }
            // NB3-4 parameter shadow: remove outer-scope aliases so that
            // resolve_ident_arity / resolve_ident_callable_tag return unknown (-1)
            // for parameters that shadow outer aliases.
            self.var_aliases.remove(&p.name);
            self.lambda_param_counts.remove(&p.name);
            self.lambda_vars.remove(&p.name);
            self.closure_vars.remove(&p.name);
        }

        // ヒープ変数トラッカーをリセット
        self.current_heap_vars.clear();

        // FL-16: パラメータの型注釈から型トラッキング変数を登録
        for param in &func_def.params {
            if let Some(type_ann) = &param.type_annotation {
                match type_ann {
                    crate::parser::TypeExpr::Named(name) if name == "Int" || name == "Num" => {
                        self.int_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Str" => {
                        self.string_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Float" => {
                        self.float_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::Named(name) if name == "Bool" => {
                        self.bool_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::List(_) => {
                        self.list_vars.insert(param.name.clone());
                    }
                    crate::parser::TypeExpr::BuchiPack(_) => {
                        self.pack_vars.insert(param.name.clone());
                    }
                    _ => {}
                }
            }
        }

        // NB3-4 fix: Save/restore return_type_inferred_params across function boundaries
        // so that inner function parameters don't inherit outer function's inference.
        // Must be saved BEFORE return-type inference so that current function's inferred
        // params are tracked in the fresh (empty) set.
        let prev_return_type_inferred_params =
            std::mem::take(&mut self.return_type_inferred_params);

        // 戻り値型注釈から型注釈なしパラメータの型を推論登録
        // 例: `sumTo n acc = ... => :Int` の場合、n, acc を int_vars に登録
        // これにより poly_add 等のヒューリスティック関数の誤発火を防ぐ
        if let Some(ref rt) = func_def.return_type {
            let inferred_numeric = matches!(
                rt,
                crate::parser::TypeExpr::Named(name) if name == "Int" || name == "Num"
            );
            if inferred_numeric {
                for param in &func_def.params {
                    if param.type_annotation.is_none()
                        && !self.string_vars.contains(&param.name)
                        && !self.float_vars.contains(&param.name)
                        && !self.bool_vars.contains(&param.name)
                        && !self.pack_vars.contains(&param.name)
                        && !self.list_vars.contains(&param.name)
                        && !self.closure_vars.contains(&param.name)
                    {
                        self.int_vars.insert(param.name.clone());
                        // NB3-4 fix: Track that this parameter's type was inferred
                        // from the return type, not from an explicit annotation.
                        // callable_type_tag must not trust this inference for
                        // handler arguments, since the parameter might be a function.
                        self.return_type_inferred_params.insert(param.name.clone());
                    }
                }
            }
        }

        // NB-14: Emit taida_get_call_arg_tag() for parameters whose type cannot be
        // determined at compile time. This reads the type tag that the caller set via
        // taida_set_call_arg_tag(), enabling Bool/Int disambiguation in pack field tags.
        let prev_param_tag_vars = std::mem::take(&mut self.param_tag_vars);
        for (i, param) in func_def.params.iter().enumerate() {
            // Only emit for parameters that don't have a compile-time type
            let has_known_type = self.bool_vars.contains(&param.name)
                || self.int_vars.contains(&param.name)
                || self.float_vars.contains(&param.name)
                || self.string_vars.contains(&param.name)
                || self.pack_vars.contains(&param.name)
                || self.list_vars.contains(&param.name)
                || self.closure_vars.contains(&param.name);
            if !has_known_type {
                let idx_var = ir_func.alloc_var();
                ir_func.push(IrInst::ConstInt(idx_var, i as i64));
                let tag_var = ir_func.alloc_var();
                ir_func.push(IrInst::Call(
                    tag_var,
                    "taida_get_call_arg_tag".to_string(),
                    vec![idx_var],
                ));
                self.param_tag_vars.insert(param.name.clone(), tag_var);
            }
        }

        // ローカル関数定義の前処理: 関数本体内の FuncDef を先に IR 化して登録する。
        // 内部関数が親スコープの変数を参照する場合はクロージャとして生成する。
        for stmt in &func_def.body {
            if let Statement::FuncDef(inner_func_def) = stmt {
                // 内部関数の自由変数を検出
                let inner_params: std::collections::HashSet<&str> = inner_func_def
                    .params
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                let inner_free_vars = self
                    .collect_free_vars_in_func_body_unfiltered(&inner_func_def.body, &inner_params);
                // 親スコープの変数のみをキャプチャ対象とする
                // （トップレベル変数は GlobalGet で解決されるので除外）
                let parent_scope_set: std::collections::HashSet<&str> =
                    parent_scope_vars.iter().map(|s| s.as_str()).collect();
                let captures: Vec<String> = inner_free_vars
                    .into_iter()
                    .filter(|v| parent_scope_set.contains(v.as_str()))
                    .collect();

                if captures.is_empty() {
                    // キャプチャなし: 通常のユーザー関数として登録
                    self.user_funcs.insert(inner_func_def.name.clone());
                    self.func_param_defs
                        .insert(inner_func_def.name.clone(), inner_func_def.params.clone());
                    let inner_ir = self.lower_func_def(inner_func_def)?;
                    self.lambda_funcs.push(inner_ir);
                } else {
                    // キャプチャあり: クロージャとして生成
                    let lambda_name = format!("_taida_lambda_{}", self.lambda_counter);
                    self.lambda_counter += 1;

                    // lambda_vars と closure_vars に登録
                    self.lambda_vars
                        .insert(inner_func_def.name.clone(), lambda_name.clone());
                    self.closure_vars.insert(inner_func_def.name.clone());

                    // __env + 元のパラメータ
                    let mut closure_params: Vec<String> = vec!["__env".to_string()];
                    closure_params.extend(inner_func_def.params.iter().map(|p| p.name.clone()));
                    let mut lambda_fn =
                        IrFunction::new_with_params(lambda_name.clone(), closure_params);

                    // 環境からキャプチャ変数を復元
                    let env_var = 0u32;
                    for (i, cap_name) in captures.iter().enumerate() {
                        let get_dst = lambda_fn.alloc_var();
                        lambda_fn.push(IrInst::PackGet(get_dst, env_var, i));
                        lambda_fn.push(IrInst::DefVar(cap_name.clone(), get_dst));
                    }

                    // 内部関数の前処理: クロージャ本体内のネストされた FuncDef を検出し処理する
                    // (deep nesting: f1 → f2(closure) → f3 → f4 → f5 のパターンに対応)
                    {
                        let scope_vars = self.collect_nested_scope_vars(
                            captures
                                .iter()
                                .cloned()
                                .chain(inner_func_def.params.iter().map(|p| p.name.clone())),
                            &inner_func_def.body,
                        );
                        self.preprocess_inner_funcdefs(&inner_func_def.body, &scope_vars)?;
                    }

                    // 関数本体を処理
                    let mut last_var = None;
                    for (i, inner_stmt) in inner_func_def.body.iter().enumerate() {
                        let is_last = i == inner_func_def.body.len() - 1;
                        match inner_stmt {
                            Statement::Expr(expr) => {
                                let var = self.lower_expr(&mut lambda_fn, expr)?;
                                if is_last {
                                    last_var = Some(var);
                                }
                            }
                            _ => {
                                self.lower_statement(&mut lambda_fn, inner_stmt)?;
                            }
                        }
                    }

                    if let Some(ret) = last_var {
                        lambda_fn.push(IrInst::Return(ret));
                    } else {
                        let zero = lambda_fn.alloc_var();
                        lambda_fn.push(IrInst::ConstInt(zero, 0));
                        lambda_fn.push(IrInst::Return(zero));
                    }

                    self.user_funcs.insert(lambda_name.clone());
                    self.lambda_funcs.push(lambda_fn);

                    // MakeClosure は本体処理時に発行する（下記 lower_statement で処理）
                    self.pending_local_closures
                        .insert(inner_func_def.name.clone(), (lambda_name, captures));
                }
            }
        }

        // グローバル変数復元: 関数本体で参照されるトップレベル変数/インポート値を GlobalGet で復元
        let global_refs = self.collect_free_vars_in_body(&func_def.body, &params);
        for var_name in &global_refs {
            self.globals_referenced.insert(var_name.clone());
            let hash = self.global_var_hash(var_name);
            let dst = ir_func.alloc_var();
            ir_func.push(IrInst::GlobalGet(dst, hash));
            ir_func.push(IrInst::DefVar(var_name.clone(), dst));
        }

        // TCO: 現在の関数名を設定
        let prev_func_name = self.current_func_name.take();
        self.current_func_name = Some(func_def.name.clone());

        // 関数本体（ErrorCeiling を含む場合は lower_statement_sequence で処理）
        let mut last_var = None;
        let mut last_expr: Option<&Expr> = None;
        let body_refs: Vec<&Statement> = func_def.body.iter().collect();
        let has_error_ceiling = body_refs
            .iter()
            .any(|s| matches!(s, Statement::ErrorCeiling(_)));

        if has_error_ceiling {
            // ErrorCeiling があるので lower_statement_sequence を使う
            self.lower_statement_sequence(&mut ir_func, &body_refs)?;
            // ErrorCeiling 使用時は暗黙の戻り値なし（handler が return 相当）
        } else {
            for (i, stmt) in func_def.body.iter().enumerate() {
                let is_last = i == func_def.body.len() - 1;
                match stmt {
                    Statement::Expr(expr) => {
                        // 最後の式は末尾位置 — TCO対象
                        let var = if is_last {
                            self.lower_expr_tail(&mut ir_func, expr)?
                        } else {
                            self.lower_expr(&mut ir_func, expr)?
                        };
                        if is_last {
                            last_var = Some(var);
                            last_expr = Some(expr);
                        }
                    }
                    _ => {
                        self.lower_statement(&mut ir_func, stmt)?;
                    }
                }
            }
        }

        // TCO: 関数名を復元
        self.current_func_name = prev_func_name;

        // F-48: 戻り値式から推移的に到達可能な変数を計算し、
        // それらのヒープ変数は Release しない（dangling pointer 防止）
        let reachable_from_return = if let Some(ret_expr) = last_expr {
            Self::compute_reachable_vars(ret_expr, &func_def.body)
        } else {
            std::collections::HashSet::new()
        };

        // 関数終了時: ヒープ変数を Release（戻り値から到達可能な変数は除外）
        let heap_vars = std::mem::take(&mut self.current_heap_vars);
        for name in &heap_vars {
            if reachable_from_return.contains(name) {
                continue; // 戻り値から参照される可能性あり — 所有権はcallerに移転
            }
            let use_var = ir_func.alloc_var();
            ir_func.push(IrInst::UseVar(use_var, name.clone()));
            ir_func.push(IrInst::Release(use_var));
        }

        // 暗黙の戻り値
        if let Some(ret) = last_var {
            // NB-14: Set return type tag so callers can propagate it.
            // This enables type info to survive through generic functions like `id x = x`
            // and transitive chains like `g x = f(x)`.
            if let Some(&rtv) = self.return_tag_vars.get(&ret) {
                // Return value came from a CallUser — propagate that call's return tag
                let dummy = ir_func.alloc_var();
                ir_func.push(IrInst::Call(
                    dummy,
                    "taida_set_return_tag".to_string(),
                    vec![rtv],
                ));
            } else if let Some(ret_expr) = last_expr {
                let tag = self.expr_type_tag(ret_expr);
                if tag > 0 {
                    let tag_var = ir_func.alloc_var();
                    ir_func.push(IrInst::ConstInt(tag_var, tag));
                    let dummy = ir_func.alloc_var();
                    ir_func.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![tag_var],
                    ));
                } else if tag == -1
                    && let Some(ptv) = self.get_param_tag_var(ret_expr)
                {
                    let dummy = ir_func.alloc_var();
                    ir_func.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![ptv],
                    ));
                }
            }
            ir_func.push(IrInst::Return(ret));
        } else {
            let zero = ir_func.alloc_var();
            ir_func.push(IrInst::ConstInt(zero, 0));
            ir_func.push(IrInst::Return(zero));
        }

        // Restore net builtin shadow set to pre-function state
        self.shadowed_net_builtins = prev_shadowed_net;
        // NB-14: Restore param_tag_vars to pre-function state
        self.param_tag_vars = prev_param_tag_vars;
        // NB3-4 fix: Restore return_type_inferred_params to pre-function state
        self.return_type_inferred_params = prev_return_type_inferred_params;
        // NB3-4: Restore var_aliases, lambda_param_counts, lambda_vars, closure_vars
        // to pre-function state (parameter shadow cleanup)
        self.var_aliases = prev_var_aliases;
        self.lambda_param_counts = prev_lambda_param_counts;
        self.lambda_vars = prev_lambda_vars;
        self.closure_vars = prev_closure_vars;

        Ok(ir_func)
    }

    /// 文列を処理。ErrorCeiling が出現したら後続文をすべて通常パスに包む。
    fn lower_statement_sequence(
        &mut self,
        func: &mut IrFunction,
        stmts: &[&Statement],
    ) -> Result<(), LowerError> {
        let mut i = 0;
        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = stmts[i] {
                // ErrorCeiling: 後続の全文を「通常パス」に入れる
                let remaining: Vec<&Statement> = stmts[i + 1..].to_vec();
                self.lower_error_ceiling_with_body(func, ec, &remaining)?;
                return Ok(()); // 残りの文は lower_error_ceiling_with_body 内で処理済み
            } else {
                self.lower_statement(func, stmts[i])?;
            }
            i += 1;
        }
        Ok(())
    }

    /// Collect variable names defined in IR instructions (DefVar names).
    fn collect_defvar_names(insts: &[IrInst]) -> Vec<String> {
        let mut names = Vec::new();
        for inst in insts {
            if let IrInst::DefVar(name, _) = inst
                && !names.contains(name)
            {
                names.push(name.clone());
            }
            // Also recurse into CondBranch arms
            if let IrInst::CondBranch(_, arms) = inst {
                for arm in arms {
                    for inner_name in Self::collect_defvar_names(&arm.body) {
                        if !names.contains(&inner_name) {
                            names.push(inner_name);
                        }
                    }
                }
            }
        }
        names
    }

    /// ErrorCeiling を後続文を含めて処理
    /// 後続文を別関数に抽出し、taida_error_try_call で setjmp 保護下で実行する
    fn lower_error_ceiling_with_body(
        &mut self,
        func: &mut IrFunction,
        ec: &crate::parser::ErrorCeiling,
        subsequent_stmts: &[&Statement],
    ) -> Result<(), LowerError> {
        // 後続文を別関数に抽出（setjmp は呼び出し元の C 関数内で行う）
        let try_func_name = format!("_taida_try_{}", self.lambda_counter);
        self.lambda_counter += 1;

        // Collect variables from parent scope that _taida_try_N needs access to.
        // This includes function parameters and any DefVar'd variables before the ErrorCeiling.
        let mut captured_vars: Vec<String> = func.params.clone();
        for name in Self::collect_defvar_names(&func.body) {
            if !captured_vars.contains(&name) {
                captured_vars.push(name);
            }
        }

        // 後続文の関数を生成（1引数: env パック）
        let mut try_fn =
            IrFunction::new_with_params(try_func_name.clone(), vec!["__env".to_string()]);

        // Restore captured variables from env pack at the beginning of _taida_try_N
        for (i, var_name) in captured_vars.iter().enumerate() {
            let get_var = try_fn.alloc_var();
            try_fn.push(IrInst::PackGet(get_var, 0, i)); // param 0 = __env
            try_fn.push(IrInst::DefVar(var_name.clone(), get_var));
        }

        // Lower subsequent statements, tracking the last expression for return value
        let mut last_try_var: Option<IrVar> = None;
        if !subsequent_stmts.is_empty() {
            // Lower all statements except possibly the last one
            let last_idx = subsequent_stmts.len() - 1;
            for (idx, stmt) in subsequent_stmts.iter().enumerate() {
                if idx == last_idx {
                    // Last statement: if it's an expression, capture its value
                    if let Statement::Expr(expr) = stmt {
                        last_try_var = Some(self.lower_expr(&mut try_fn, expr)?);
                    } else {
                        self.lower_statement(&mut try_fn, stmt)?;
                    }
                } else if let Statement::ErrorCeiling(ec2) = stmt {
                    // Nested ErrorCeiling: delegate to lower_error_ceiling_with_body
                    let remaining: Vec<&Statement> = subsequent_stmts[idx + 1..].to_vec();
                    self.lower_error_ceiling_with_body(&mut try_fn, ec2, &remaining)?;
                    break;
                } else {
                    self.lower_statement(&mut try_fn, stmt)?;
                }
            }
        }
        // Return the last expression value, or 0 if none
        match last_try_var {
            Some(v) => {
                try_fn.push(IrInst::Return(v));
            }
            None => {
                let ret_var = try_fn.alloc_var();
                try_fn.push(IrInst::ConstInt(ret_var, 0));
                try_fn.push(IrInst::Return(ret_var));
            }
        }
        self.lambda_funcs.push(try_fn);
        // ユーザー関数として登録（emit で関数として扱われるように）
        self.user_funcs.insert(try_func_name.clone());

        // Build env pack with captured variables
        let env_pack = func.alloc_var();
        func.push(IrInst::PackNew(env_pack, captured_vars.len()));
        for (i, var_name) in captured_vars.iter().enumerate() {
            let use_var = func.alloc_var();
            func.push(IrInst::UseVar(use_var, var_name.clone()));
            let hash = simple_hash(var_name);
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            // Use Call to set hash + value (reuse existing PackSet infrastructure)
            func.push(IrInst::PackSet(env_pack, i, use_var));
        }

        // Push error ceiling
        let depth = func.alloc_var();
        func.push(IrInst::Call(
            depth,
            "taida_error_ceiling_push".to_string(),
            vec![],
        ));

        // 関数アドレスを取得
        let fn_addr = func.alloc_var();
        func.push(IrInst::FuncAddr(fn_addr, try_func_name));

        // taida_error_try_call(fn_ptr, env_ptr, depth) → 0 正常 / 1 エラー
        let try_result = func.alloc_var();
        func.push(IrInst::Call(
            try_result,
            "taida_error_try_call".to_string(),
            vec![fn_addr, env_pack, depth],
        ));

        // Handler arm (try_call returned 1 → error caught)
        let handler_insts = {
            let saved = std::mem::take(&mut func.body);
            // Pop error ceiling BEFORE handler body execution.
            // This is critical for re-throw: if the handler body throws again,
            // the depth must already be decremented so the throw goes to the
            // correct outer ceiling (not the now-invalid current one).
            let pop_var = func.alloc_var();
            func.push(IrInst::Call(
                pop_var,
                "taida_error_ceiling_pop".to_string(),
                vec![],
            ));
            let err_var = func.alloc_var();
            func.push(IrInst::Call(
                err_var,
                "taida_error_get_value".to_string(),
                vec![depth],
            ));
            // RCB-101: Type filter — re-throw if error type does not match handler type.
            // taida_error_type_check_or_rethrow(err_var, handler_type_str)
            // If type does not match, this calls taida_throw internally (longjmp/never returns).
            let handler_type_name = match &ec.error_type {
                crate::parser::TypeExpr::Named(name) => name.clone(),
                _ => "Error".to_string(),
            };
            let handler_type_str = func.alloc_var();
            func.push(IrInst::ConstStr(handler_type_str, handler_type_name));
            let checked_err = func.alloc_var();
            func.push(IrInst::Call(
                checked_err,
                "taida_error_type_check_or_rethrow".to_string(),
                vec![err_var, handler_type_str],
            ));
            func.push(IrInst::DefVar(ec.error_param.clone(), checked_err));
            // Lower handler body, capturing the last expression's result
            let mut last_handler_var = None;
            for (idx, stmt) in ec.handler_body.iter().enumerate() {
                let is_last = idx == ec.handler_body.len() - 1;
                if is_last {
                    if let Statement::Expr(expr) = stmt {
                        last_handler_var = Some(self.lower_expr(func, expr)?);
                    } else {
                        self.lower_statement(func, stmt)?;
                    }
                } else {
                    self.lower_statement(func, stmt)?;
                }
            }
            let handler_result = match last_handler_var {
                Some(v) => {
                    // Handler produced a value — return it and also push a Return
                    // so this value becomes the function return
                    func.push(IrInst::Return(v));
                    v
                }
                None => {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    zero
                }
            };
            let insts = std::mem::replace(&mut func.body, saved);
            (insts, handler_result)
        };

        // Normal arm (try_call returned 0 → completed without error)
        let normal_insts = {
            let saved = std::mem::take(&mut func.body);
            let pop_var = func.alloc_var();
            func.push(IrInst::Call(
                pop_var,
                "taida_error_ceiling_pop".to_string(),
                vec![],
            ));
            // Retrieve the return value from _taida_try_N via the global result slot
            let normal_result = func.alloc_var();
            func.push(IrInst::Call(
                normal_result,
                "taida_error_try_get_result".to_string(),
                vec![depth],
            ));
            func.push(IrInst::Return(normal_result));
            let insts = std::mem::replace(&mut func.body, saved);
            (insts, normal_result)
        };

        let cond_result = func.alloc_var();
        let arms = vec![
            super::ir::CondArm {
                condition: Some(try_result),
                body: handler_insts.0,
                result: handler_insts.1,
            },
            super::ir::CondArm {
                condition: None,
                body: normal_insts.0,
                result: normal_insts.1,
            },
        ];
        func.push(IrInst::CondBranch(cond_result, arms));

        Ok(())
    }

    fn lower_statement(
        &mut self,
        func: &mut IrFunction,
        stmt: &Statement,
    ) -> Result<(), LowerError> {
        match stmt {
            Statement::Expr(expr) => {
                self.lower_expr(func, expr)?;
                Ok(())
            }
            Statement::Assignment(assign) => {
                // ラムダが変数に代入される場合、マッピングを記録
                if let Expr::Lambda(params, body, _) = &assign.value {
                    let next_lambda_name = format!("_taida_lambda_{}", self.lambda_counter);
                    let param_names: std::collections::HashSet<&str> =
                        params.iter().map(|p| p.name.as_str()).collect();
                    let free_vars = self.collect_free_vars(body, &param_names);
                    if free_vars.is_empty() {
                        self.lambda_vars
                            .insert(assign.target.clone(), next_lambda_name);
                    } else {
                        self.lambda_vars
                            .insert(assign.target.clone(), next_lambda_name);
                        self.closure_vars.insert(assign.target.clone());
                    }
                    // NB3-4: Record lambda parameter count for handler_arity resolution
                    self.lambda_param_counts
                        .insert(assign.target.clone(), params.len());
                }
                // NB3-4: Track variable aliases for identity assignments (e.g., `h <= handler`)
                if let Expr::Ident(source_name, _) = &assign.value {
                    self.var_aliases
                        .insert(assign.target.clone(), source_name.clone());
                }
                let val = self.lower_expr(func, &assign.value)?;
                func.push(IrInst::DefVar(assign.target.clone(), val));

                // トップレベル変数をグローバルテーブルにも格納
                // （_taida_main 内で、かつ関数から参照されるトップレベル変数のみ）
                if self.current_func_name.is_none()
                    && self.globals_referenced.contains(&assign.target)
                {
                    let hash = self.global_var_hash(&assign.target);
                    func.push(IrInst::GlobalSet(hash, val));
                }

                // NB-31: int を返す式の結果を追跡（callable_type_tag 精度向上）
                if self.expr_is_int(&assign.value) {
                    self.int_vars.insert(assign.target.clone());
                }
                // float を返す式の結果を追跡
                if self.expr_returns_float(&assign.value) {
                    self.float_vars.insert(assign.target.clone());
                }
                // string を返す式の結果を追跡
                if self.expr_is_string_full(&assign.value) {
                    self.string_vars.insert(assign.target.clone());
                }
                // bool を返す式の結果を追跡
                if self.expr_is_bool(&assign.value) {
                    self.bool_vars.insert(assign.target.clone());
                }
                // F-58: BuchiPack/TypeInst を返す式の結果を追跡
                if self.expr_is_pack(&assign.value) {
                    self.pack_vars.insert(assign.target.clone());
                }
                // retain-on-store: List を返す式の結果を追跡
                if self.expr_is_list(&assign.value) {
                    self.list_vars.insert(assign.target.clone());
                }
                // QF-34: MoldInst の Lax 内部型を追跡（unmold 時の型推定用）
                if let Expr::MoldInst(mold_name, _, _, _) = &assign.value {
                    self.lax_inner_types
                        .insert(assign.target.clone(), mold_name.clone());
                }
                // QF-10: TypeInst の変数に TypeDef 名を記録（フィールド型解決用）
                if let Expr::TypeInst(type_name, _, _) = &assign.value {
                    self.var_type_names
                        .insert(assign.target.clone(), type_name.clone());
                }

                // ヒープ確保される式の変数をトラッキング
                if Self::is_heap_expr(&assign.value) {
                    self.current_heap_vars.push(assign.target.clone());
                } else if self.closure_vars.contains(&assign.target) {
                    // キャプチャありラムダ = クロージャ = ヒープオブジェクト
                    self.current_heap_vars.push(assign.target.clone());
                }

                // Track local assignment shadow: if the target name matches a net
                // builtin, subsequent calls in the same scope must use the local
                // variable, not the builtin dispatch.
                if Self::NET_BUILTIN_NAMES.contains(&assign.target.as_str()) {
                    self.shadowed_net_builtins.insert(assign.target.clone());
                }

                Ok(())
            }
            Statement::FuncDef(func_def_stmt) => {
                // トップレベルの定義は1st passで処理済み。
                // ローカル関数でキャプチャが必要なものは前処理で pending_local_closures に
                // 登録されているので、ここで MakeClosure + DefVar を発行する。
                if let Some((lambda_name, captures)) =
                    self.pending_local_closures.remove(&func_def_stmt.name)
                {
                    let dst = func.alloc_var();
                    func.push(IrInst::MakeClosure(dst, lambda_name, captures));
                    func.push(IrInst::DefVar(func_def_stmt.name.clone(), dst));
                    self.current_heap_vars.push(func_def_stmt.name.clone());
                }
                Ok(())
            }
            Statement::TypeDef(type_def) => {
                // Register type fields (already done in 1st pass, but safe to repeat)
                let non_method_field_defs: Vec<crate::parser::FieldDef> = type_def
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .cloned()
                    .collect();
                let fields: Vec<String> = non_method_field_defs
                    .iter()
                    .map(|f| f.name.clone())
                    .collect();
                self.type_fields.insert(type_def.name.clone(), fields);
                // JSON スキーマ解決用
                let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                    non_method_field_defs
                        .iter()
                        .map(|f| (f.name.clone(), f.type_annotation.clone()))
                        .collect();
                self.type_field_types
                    .insert(type_def.name.clone(), field_types);
                self.type_field_defs
                    .insert(type_def.name.clone(), non_method_field_defs);
                // Register method definitions (safe to repeat from 1st pass)
                let methods: Vec<(String, crate::parser::FuncDef)> = type_def
                    .fields
                    .iter()
                    .filter(|f| f.is_method && f.method_def.is_some())
                    .map(|f| (f.name.clone(), f.method_def.clone().unwrap()))
                    .collect();
                if !methods.is_empty() {
                    self.type_method_defs.insert(type_def.name.clone(), methods);
                }
                Ok(())
            }
            Statement::MoldDef(mold_def) => {
                // MoldDef is internally treated like TypeDef
                let non_method_field_defs: Vec<crate::parser::FieldDef> = mold_def
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .cloned()
                    .collect();
                let fields: Vec<String> = non_method_field_defs
                    .iter()
                    .map(|f| f.name.clone())
                    .collect();
                self.type_fields.insert(mold_def.name.clone(), fields);
                // JSON スキーマ解決用
                let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> =
                    non_method_field_defs
                        .iter()
                        .map(|f| (f.name.clone(), f.type_annotation.clone()))
                        .collect();
                self.type_field_types
                    .insert(mold_def.name.clone(), field_types);
                self.type_field_defs
                    .insert(mold_def.name.clone(), non_method_field_defs);
                self.mold_defs
                    .insert(mold_def.name.clone(), mold_def.clone());
                Ok(())
            }
            Statement::InheritanceDef(inh_def) => {
                // Inheritance: parent fields + child fields
                let mut all_fields = self
                    .type_fields
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                let mut all_field_types = self
                    .type_field_types
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                let mut all_field_defs = self
                    .type_field_defs
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                for field in inh_def.fields.iter().filter(|f| !f.is_method) {
                    all_fields.retain(|name| name != &field.name);
                    all_fields.push(field.name.clone());
                    all_field_types.retain(|(name, _)| name != &field.name);
                    all_field_types.push((field.name.clone(), field.type_annotation.clone()));
                    all_field_defs.retain(|f| f.name != field.name);
                    all_field_defs.push(field.clone());
                }
                self.type_fields.insert(inh_def.child.clone(), all_fields);
                self.type_field_types
                    .insert(inh_def.child.clone(), all_field_types);
                self.type_field_defs
                    .insert(inh_def.child.clone(), all_field_defs);
                // Inherit parent methods, then override/add child methods
                let mut all_methods = self
                    .type_method_defs
                    .get(&inh_def.parent)
                    .cloned()
                    .unwrap_or_default();
                for field in inh_def
                    .fields
                    .iter()
                    .filter(|f| f.is_method && f.method_def.is_some())
                {
                    all_methods.retain(|(name, _)| name != &field.name);
                    all_methods.push((field.name.clone(), field.method_def.clone().unwrap()));
                }
                if !all_methods.is_empty() {
                    self.type_method_defs
                        .insert(inh_def.child.clone(), all_methods);
                }
                // RCB-101: Register inheritance parent for error type filtering in |==
                let child_str_var = func.alloc_var();
                func.push(IrInst::ConstStr(child_str_var, inh_def.child.clone()));
                let parent_str_var = func.alloc_var();
                func.push(IrInst::ConstStr(parent_str_var, inh_def.parent.clone()));
                let reg_dummy = func.alloc_var();
                func.push(IrInst::Call(
                    reg_dummy,
                    "taida_register_type_parent".to_string(),
                    vec![child_str_var, parent_str_var],
                ));
                if let Some(parent_mold) = self.mold_defs.get(&inh_def.parent).cloned() {
                    let mut merged_mold_fields = parent_mold.fields.clone();
                    for child_field in &inh_def.fields {
                        if let Some(existing) = merged_mold_fields
                            .iter_mut()
                            .find(|field| field.name == child_field.name)
                        {
                            *existing = child_field.clone();
                        } else {
                            merged_mold_fields.push(child_field.clone());
                        }
                    }
                    self.mold_defs.insert(
                        inh_def.child.clone(),
                        crate::parser::MoldDef {
                            name: inh_def.child.clone(),
                            mold_args: parent_mold.mold_args.clone(),
                            name_args: inh_def
                                .child_args
                                .clone()
                                .or_else(|| inh_def.parent_args.clone())
                                .or(parent_mold.name_args.clone()),
                            type_params: parent_mold.type_params.clone(),
                            fields: merged_mold_fields,
                            doc_comments: inh_def.doc_comments.clone(),
                            span: inh_def.span.clone(),
                        },
                    );
                }
                Ok(())
            }
            Statement::ErrorCeiling(ec) => {
                // lower_statement_sequence 経由で呼ばれるべきだが、
                // 直接呼ばれた場合は後続文なしで処理（フォールバック）
                self.lower_error_ceiling_with_body(func, ec, &[])
            }
            Statement::Export(_) | Statement::Import(_) => {
                // モジュールレベルで処理済み
                Ok(())
            }
            Statement::UnmoldForward(uf) => {
                // expr ]=> name : Async のアンモールド
                let source_var = self.lower_expr(func, &uf.source)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![source_var],
                ));
                func.push(IrInst::DefVar(uf.target.clone(), result));
                // Track type from mold source for debug display
                self.track_unmold_type(&uf.target, &uf.source);
                // Track local unmold-forward shadow for net builtins
                if Self::NET_BUILTIN_NAMES.contains(&uf.target.as_str()) {
                    self.shadowed_net_builtins.insert(uf.target.clone());
                }
                Ok(())
            }
            Statement::UnmoldBackward(ub) => {
                // name <=[ expr : Async のアンモールド（逆方向）
                let source_var = self.lower_expr(func, &ub.source)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![source_var],
                ));
                func.push(IrInst::DefVar(ub.target.clone(), result));
                // Track type from mold source for debug display
                self.track_unmold_type(&ub.target, &ub.source);
                // Track local unmold-backward shadow for net builtins
                if Self::NET_BUILTIN_NAMES.contains(&ub.target.as_str()) {
                    self.shadowed_net_builtins.insert(ub.target.clone());
                }
                Ok(())
            } // All statement types are now handled above.
              // This branch should not be reached.
        }
    }

    pub(crate) fn lower_expr(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
    ) -> Result<IrVar, LowerError> {
        match expr {
            Expr::IntLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstInt(var, *val));
                Ok(var)
            }
            Expr::FloatLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstFloat(var, *val));
                Ok(var)
            }
            Expr::StringLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstStr(var, val.clone()));
                Ok(var)
            }
            Expr::BoolLit(val, _) => {
                let var = func.alloc_var();
                func.push(IrInst::ConstBool(var, *val));
                Ok(var)
            }
            Expr::Ident(name, _) => {
                // stdlib 定数（PI, E 等）はインライン展開
                if let Some(&val) = self.stdlib_constants.get(name) {
                    let var = func.alloc_var();
                    func.push(IrInst::ConstFloat(var, val));
                    return Ok(var);
                }
                // ユーザー定義関数を値として参照する場合は FuncAddr を使う
                if self.user_funcs.contains(name) {
                    let mangled = self.resolve_user_func_symbol(name);
                    let var = func.alloc_var();
                    func.push(IrInst::FuncAddr(var, mangled));
                    return Ok(var);
                }
                let var = func.alloc_var();
                func.push(IrInst::UseVar(var, name.clone()));
                Ok(var)
            }
            Expr::FuncCall(callee, args, _) => self.lower_func_call(func, callee, args),
            Expr::BinaryOp(lhs, op, rhs, _) => self.lower_binary_op(func, lhs, op, rhs),
            Expr::UnaryOp(op, operand, _) => self.lower_unary_op(func, op, operand),
            Expr::Pipeline(exprs, _) => self.lower_pipeline(func, exprs),
            Expr::BuchiPack(fields, _) => self.lower_buchi_pack(func, fields),
            Expr::TypeInst(name, fields, _) => self.lower_type_inst(func, name, fields),
            Expr::FieldAccess(obj, field, _) => self.lower_field_access(func, obj, field),
            Expr::CondBranch(arms, _) => self.lower_cond_branch(func, arms),
            Expr::Lambda(params, body, _) => self.lower_lambda(func, params, body),
            Expr::MethodCall(obj, method, args, _) => {
                self.lower_method_call(func, obj, method, args)
            }
            Expr::ListLit(items, _) => self.lower_list_lit(func, items),
            Expr::Gorilla(_) => {
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_gorilla".to_string(), vec![]));
                Ok(result)
            }
            Expr::MoldInst(type_name, type_args, fields, _) => {
                self.lower_mold_inst(func, type_name, type_args, fields)
            }
            Expr::Unmold(expr, _) => {
                // expr.unmold() → taida_generic_unmold(expr)
                let val = self.lower_expr(func, expr)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_generic_unmold".to_string(),
                    vec![val],
                ));
                Ok(result)
            }
            Expr::TemplateLit(template, _) => self.lower_template_lit(func, template),
            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::Throw(inner, _) => {
                let val = self.lower_expr(func, inner)?;
                let result = func.alloc_var();
                func.push(IrInst::Call(result, "taida_throw".to_string(), vec![val]));
                Ok(result)
            }
            Expr::Placeholder(_) => {
                // Placeholder outside pipeline context: return 0 (Unit)
                let var = func.alloc_var();
                func.push(IrInst::ConstInt(var, 0));
                Ok(var)
            }
            Expr::Hole(_) => {
                // Hole outside partial application context: return 0 (Unit)
                let var = func.alloc_var();
                func.push(IrInst::ConstInt(var, 0));
                Ok(var)
            }
        }
    }

    /// 末尾位置の式を lowering する（TCO対応）
    /// 自己再帰呼び出しを IrInst::TailCall に変換する
    fn lower_expr_tail(&mut self, func: &mut IrFunction, expr: &Expr) -> Result<IrVar, LowerError> {
        match expr {
            // 自己再帰呼び出しの検出
            Expr::FuncCall(callee, args, _) => {
                if let Expr::Ident(name, _) = callee.as_ref()
                    && self.current_func_name.as_deref() == Some(name)
                {
                    // 末尾位置の自己再帰 → TailCall
                    let arg_vars =
                        self.lower_user_call_effective_args_from_exprs(func, name, args)?;
                    // TailCall は戻り値を持たないが、IRVar は必要
                    // (Return と組み合わされるため、ダミーの var を使う)
                    func.push(IrInst::TailCall(arg_vars));
                    // TailCall の後にコードが生成されないように、
                    // ダミーの戻り値を返す（実際には到達しない）
                    let dummy = func.alloc_var();
                    func.push(IrInst::ConstInt(dummy, 0));
                    return Ok(dummy);
                }
                // 自己再帰でない場合は通常の呼び出し
                // NB-14: Mark as tail call to skip get_return_tag (preserves C TCO for WASM)
                self.in_tail_call_return = true;
                let result = self.lower_func_call(func, callee, args);
                self.in_tail_call_return = false;
                result
            }
            // CondBranch: 各アームの末尾を再帰的にチェック
            Expr::CondBranch(arms, _) => self.lower_cond_branch_tail(func, arms),
            // その他の式は通常の lowering
            _ => self.lower_expr(func, expr),
        }
    }

    /// TCO対応の条件分岐 lowering
    /// 各アームの本体は末尾位置なので、自己再帰呼び出しを TailCall に変換する
    fn lower_cond_branch_tail(
        &mut self,
        func: &mut IrFunction,
        arms: &[crate::parser::CondArm],
    ) -> Result<IrVar, LowerError> {
        use super::ir::CondArm as IrCondArm;

        let result_var = func.alloc_var();
        let mut ir_arms = Vec::new();

        for arm in arms {
            let condition = match &arm.condition {
                Some(cond_expr) => {
                    let cond_var = self.lower_expr(func, cond_expr)?;
                    Some(cond_var)
                }
                None => None,
            };

            // 本体を末尾位置として lowering（複数ステートメント対応）
            let (body_insts, body_var) = {
                let saved = std::mem::take(&mut func.body);
                let body_result = self.lower_cond_arm_body_tail(func, &arm.body)?;
                let insts = std::mem::replace(&mut func.body, saved);
                (insts, body_result)
            };

            ir_arms.push(IrCondArm {
                condition,
                body: body_insts,
                result: body_var,
            });
        }

        func.push(IrInst::CondBranch(result_var, ir_arms));
        Ok(result_var)
    }

    fn lower_user_call_effective_args_from_exprs(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        args: &[Expr],
    ) -> Result<Vec<IrVar>, LowerError> {
        let mut explicit_arg_vars = Vec::with_capacity(args.len());
        for arg in args {
            explicit_arg_vars.push(self.lower_expr(func, arg)?);
        }
        self.lower_user_call_effective_args_from_vars(func, name, explicit_arg_vars)
    }

    fn lower_user_call_effective_args_from_vars(
        &mut self,
        func: &mut IrFunction,
        name: &str,
        explicit_arg_vars: Vec<IrVar>,
    ) -> Result<Vec<IrVar>, LowerError> {
        let Some(params) = self.func_param_defs.get(name).cloned() else {
            return Ok(explicit_arg_vars);
        };

        if explicit_arg_vars.len() > params.len() {
            return Err(LowerError {
                message: format!(
                    "Function '{}' expected at most {} argument(s), got {}",
                    name,
                    params.len(),
                    explicit_arg_vars.len()
                ),
            });
        }

        // Materialize defaults in parameter order while exposing earlier params
        // by their declared names for default-expression references.
        let mut snapshots = Vec::<(String, IrVar)>::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for param in &params {
            if seen.insert(param.name.clone()) {
                let prev = func.alloc_var();
                func.push(IrInst::UseVar(prev, param.name.clone()));
                snapshots.push((param.name.clone(), prev));
            }
        }

        let mut effective_args = Vec::with_capacity(params.len());
        for (i, param) in params.iter().enumerate() {
            let val = if let Some(v) = explicit_arg_vars.get(i) {
                *v
            } else if let Some(default_expr) = &param.default_value {
                self.lower_expr(func, default_expr)?
            } else if let Some(type_expr) = &param.type_annotation {
                let mut visiting = std::collections::HashSet::new();
                self.lower_default_for_type_expr(func, type_expr, &mut visiting)?
            } else {
                let zero = func.alloc_var();
                func.push(IrInst::ConstInt(zero, 0));
                zero
            };
            func.push(IrInst::DefVar(param.name.clone(), val));
            effective_args.push(val);
        }

        for (name, prev) in snapshots {
            func.push(IrInst::DefVar(name, prev));
        }

        Ok(effective_args)
    }

    fn lower_func_call(
        &mut self,
        func: &mut IrFunction,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        // Empty-slot partial application: if any arg is Hole, emit a lambda.
        // Note: Old `_` (Placeholder) partial application is rejected by checker
        // (E1502) before reaching codegen. Only Hole-based syntax `f(5, )` is handled.
        let has_hole = args.iter().any(|a| matches!(a, Expr::Hole(_)));
        if has_hole {
            return self.lower_partial_application(func, callee, args);
        }

        if let Expr::Ident(name, _) = callee {
            // OS network APIs with unified timeout argument (optional last arg).
            // Native backend uses fixed runtime signatures, so we inject defaults here.
            if matches!(
                name.as_str(),
                "dnsResolve"
                    | "poolAcquire"
                    | "tcpConnect"
                    | "tcpListen"
                    | "tcpAccept"
                    | "socketSend"
                    | "socketSendAll"
                    | "socketRecv"
                    | "socketSendBytes"
                    | "socketRecvBytes"
                    | "socketRecvExact"
                    | "udpBind"
                    | "udpSendTo"
                    | "udpRecvFrom"
            ) {
                let timeout_var = |this: &mut Self, f: &mut IrFunction, idx: usize| {
                    if let Some(arg) = args.get(idx) {
                        this.lower_expr(f, arg)
                    } else {
                        let t = f.alloc_var();
                        f.push(IrInst::ConstInt(t, OS_NET_DEFAULT_TIMEOUT_MS));
                        Ok(t)
                    }
                };

                match name.as_str() {
                    "dnsResolve" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message:
                                    "dnsResolve requires 1 or 2 arguments: dnsResolve(host[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let host = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_dns_resolve".to_string(),
                            vec![host, timeout],
                        ));
                        return Ok(result);
                    }
                    "poolAcquire" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message:
                                    "poolAcquire requires 1 or 2 arguments: poolAcquire(pool[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let pool = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_pool_acquire".to_string(),
                            vec![pool, timeout],
                        ));
                        return Ok(result);
                    }
                    "tcpConnect" => {
                        if args.len() < 2 || args.len() > 3 {
                            return Err(LowerError {
                                message:
                                    "tcpConnect requires 2 or 3 arguments: tcpConnect(host, port[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let host = self.lower_expr(func, &args[0])?;
                        let port = self.lower_expr(func, &args[1])?;
                        let timeout = timeout_var(self, func, 2)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_tcp_connect".to_string(),
                            vec![host, port, timeout],
                        ));
                        return Ok(result);
                    }
                    "tcpListen" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message:
                                    "tcpListen requires 1 or 2 arguments: tcpListen(port[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let port = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_tcp_listen".to_string(),
                            vec![port, timeout],
                        ));
                        return Ok(result);
                    }
                    "tcpAccept" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message:
                                    "tcpAccept requires 1 or 2 arguments: tcpAccept(listener[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let listener = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_tcp_accept".to_string(),
                            vec![listener, timeout],
                        ));
                        return Ok(result);
                    }
                    "socketSend" => {
                        if args.len() < 2 || args.len() > 3 {
                            return Err(LowerError {
                                message:
                                    "socketSend requires 2 or 3 arguments: socketSend(socket, data[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let data = self.lower_expr(func, &args[1])?;
                        let timeout = timeout_var(self, func, 2)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_socket_send".to_string(),
                            vec![socket, data, timeout],
                        ));
                        return Ok(result);
                    }
                    "socketSendAll" => {
                        if args.len() < 2 || args.len() > 3 {
                            return Err(LowerError {
                                message:
                                    "socketSendAll requires 2 or 3 arguments: socketSendAll(socket, data[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let data = self.lower_expr(func, &args[1])?;
                        let timeout = timeout_var(self, func, 2)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_socket_send_all".to_string(),
                            vec![socket, data, timeout],
                        ));
                        return Ok(result);
                    }
                    "socketRecv" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message:
                                    "socketRecv requires 1 or 2 arguments: socketRecv(socket[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_socket_recv".to_string(),
                            vec![socket, timeout],
                        ));
                        return Ok(result);
                    }
                    "socketRecvExact" => {
                        if args.len() < 2 || args.len() > 3 {
                            return Err(LowerError {
                                message:
                                    "socketRecvExact requires 2 or 3 arguments: socketRecvExact(socket, size[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let size = self.lower_expr(func, &args[1])?;
                        let timeout = timeout_var(self, func, 2)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_socket_recv_exact".to_string(),
                            vec![socket, size, timeout],
                        ));
                        return Ok(result);
                    }
                    "socketSendBytes" => {
                        if args.len() < 2 || args.len() > 3 {
                            return Err(LowerError {
                                message: "socketSendBytes requires 2 or 3 arguments: socketSendBytes(socket, data[, timeoutMs])".to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let data = self.lower_expr(func, &args[1])?;
                        let timeout = timeout_var(self, func, 2)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_socket_send_bytes".to_string(),
                            vec![socket, data, timeout],
                        ));
                        return Ok(result);
                    }
                    "socketRecvBytes" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message: "socketRecvBytes requires 1 or 2 arguments: socketRecvBytes(socket[, timeoutMs])".to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_socket_recv_bytes".to_string(),
                            vec![socket, timeout],
                        ));
                        return Ok(result);
                    }
                    "udpBind" => {
                        if args.len() < 2 || args.len() > 3 {
                            return Err(LowerError {
                                message:
                                    "udpBind requires 2 or 3 arguments: udpBind(host, port[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let host = self.lower_expr(func, &args[0])?;
                        let port = self.lower_expr(func, &args[1])?;
                        let timeout = timeout_var(self, func, 2)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_udp_bind".to_string(),
                            vec![host, port, timeout],
                        ));
                        return Ok(result);
                    }
                    "udpSendTo" => {
                        if args.len() < 4 || args.len() > 5 {
                            return Err(LowerError {
                                message:
                                    "udpSendTo requires 4 or 5 arguments: udpSendTo(socket, host, port, data[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let host = self.lower_expr(func, &args[1])?;
                        let port = self.lower_expr(func, &args[2])?;
                        let data = self.lower_expr(func, &args[3])?;
                        let timeout = timeout_var(self, func, 4)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_udp_send_to".to_string(),
                            vec![socket, host, port, data, timeout],
                        ));
                        return Ok(result);
                    }
                    "udpRecvFrom" => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(LowerError {
                                message:
                                    "udpRecvFrom requires 1 or 2 arguments: udpRecvFrom(socket[, timeoutMs])"
                                        .to_string(),
                            });
                        }
                        let socket = self.lower_expr(func, &args[0])?;
                        let timeout = timeout_var(self, func, 1)?;
                        let result = func.alloc_var();
                        func.push(IrInst::Call(
                            result,
                            "taida_os_udp_recv_from".to_string(),
                            vec![socket, timeout],
                        ));
                        return Ok(result);
                    }
                    _ => {}
                }
            }

            // httpServe(port, handler, maxRequests <= 0, timeoutMs <= 5000, maxConnections <= 128)
            // handler is a function/closure, passed as a function pointer.
            // maxRequests, timeoutMs, and maxConnections are optional with defaults.
            // Skip if the name is shadowed by a parameter in the current function scope.
            if name == "httpServe"
                && !self.is_net_builtin_shadowed("httpServe")
                && self
                    .stdlib_runtime_funcs
                    .get("httpServe")
                    .is_some_and(|v| v == "taida_net_http_serve")
            {
                if args.is_empty() || args.len() > 5 {
                    return Err(LowerError {
                        message:
                            "httpServe requires 2 to 5 arguments: httpServe(port, handler[, maxRequests, timeoutMs, maxConnections])"
                                .to_string(),
                    });
                }
                let port = self.lower_expr(func, &args[0])?;
                let handler = self.lower_expr(func, &args[1])?;
                let max_requests = if let Some(arg) = args.get(2) {
                    self.lower_expr(func, arg)?
                } else {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 0)); // default: 0 = unlimited
                    v
                };
                let timeout_ms = if let Some(arg) = args.get(3) {
                    self.lower_expr(func, arg)?
                } else {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 5000)); // default: 5000ms
                    v
                };
                // NET2-5d: maxConnections (optional, default 128)
                let max_connections = if let Some(arg) = args.get(4) {
                    self.lower_expr(func, arg)?
                } else {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 128)); // default: 128
                    v
                };
                // NB-31: Pass compile-time handler type tag so the C runtime
                // can reject non-callable values (large ints, strings, packs)
                // without relying on the heuristic _taida_is_callable_impl.
                // Tag 6 = CLOSURE, 10 = named function ref, -1 = unknown.
                let handler_tag = self.callable_type_tag(&args[1]);
                let handler_tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(handler_tag_var, handler_tag));
                // NET3-5a: Pass compile-time handler arity so the C runtime
                // can distinguish 1-arg (one-shot) vs 2-arg (streaming) handlers.
                // -1 = unknown (dynamic).
                let handler_arity = self.handler_arity(&args[1]);
                let handler_arity_var = func.alloc_var();
                func.push(IrInst::ConstInt(handler_arity_var, handler_arity));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_net_http_serve".to_string(),
                    vec![
                        port,
                        handler,
                        max_requests,
                        timeout_ms,
                        max_connections,
                        handler_tag_var,
                        handler_arity_var,
                    ],
                ));
                return Ok(result);
            }

            if name == "debug" {
                return self.lower_debug_call(func, args);
            }

            if name == "typeof" || name == "typeOf" {
                if args.len() != 1 {
                    return Err(LowerError {
                        message: format!("typeof requires exactly 1 argument, got {}", args.len()),
                    });
                }
                let arg = &args[0];
                let arg_var = self.lower_expr(func, arg)?;
                // Pass compile-time type tag as second argument to disambiguate
                // Int/Float/Bool which are all i64 at runtime
                let tag = self.expr_type_tag(arg);
                let tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(tag_var, tag));
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_typeof".to_string(),
                    vec![arg_var, tag_var],
                ));
                return Ok(result);
            }

            if name == "nowMs" {
                if !args.is_empty() {
                    return Err(LowerError {
                        message: format!("nowMs requires 0 arguments, got {}", args.len()),
                    });
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_time_now_ms".to_string(),
                    vec![],
                ));
                return Ok(result);
            }

            if name == "sleep" {
                if args.len() != 1 {
                    return Err(LowerError {
                        message: format!(
                            "sleep requires exactly 1 argument (ms), got {}",
                            args.len()
                        ),
                    });
                }
                let ms = self.lower_expr(func, &args[0])?;
                let result = func.alloc_var();
                func.push(IrInst::Call(
                    result,
                    "taida_time_sleep".to_string(),
                    vec![ms],
                ));
                return Ok(result);
            }

            if name == "allEnv" || name == "argv" {
                if !args.is_empty() {
                    return Err(LowerError {
                        message: format!("{} requires 0 arguments, got {}", name, args.len()),
                    });
                }
                let rt_name = if name == "allEnv" {
                    "taida_os_all_env"
                } else {
                    "taida_os_argv"
                };
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name.to_string(), vec![]));
                return Ok(result);
            }

            // stdlib ランタイム関数呼び出し（std/math, std/io etc.）
            // Skip if the name is a net builtin that is shadowed by a parameter.
            if let Some(rt_name) = self.stdlib_runtime_funcs.get(name).cloned()
                && !self.is_net_builtin_shadowed(name)
            {
                // stdout/stderr: auto-convert non-string args to string
                if (name == "stdout" || name == "stderr") && args.len() == 1 {
                    let arg = &args[0];
                    let arg_var = self.lower_expr(func, arg)?;
                    let str_var = self.convert_to_string(func, arg, arg_var)?;
                    let result = func.alloc_var();
                    func.push(IrInst::Call(result, rt_name, vec![str_var]));
                    return Ok(result);
                }
                // stdin: optional prompt arg (pass empty string if none)
                if name == "stdin" {
                    let prompt_var = if let Some(arg) = args.first() {
                        let arg_var = self.lower_expr(func, arg)?;
                        self.convert_to_string(func, arg, arg_var)?
                    } else {
                        let empty = func.alloc_var();
                        func.push(IrInst::ConstStr(empty, String::new()));
                        empty
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(result, rt_name, vec![prompt_var]));
                    return Ok(result);
                }
                // jsonEncode/jsonPretty: pass value directly (no auto-conversion)
                // The C runtime handles polymorphic serialization
                if name == "jsonEncode" || name == "jsonPretty" {
                    let val_var = if let Some(arg) = args.first() {
                        self.lower_expr(func, arg)?
                    } else {
                        let zero = func.alloc_var();
                        func.push(IrInst::ConstInt(zero, 0));
                        zero
                    };
                    let result = func.alloc_var();
                    func.push(IrInst::Call(result, rt_name, vec![val_var]));
                    return Ok(result);
                }
                let mut arg_vars = Vec::new();
                for arg in args {
                    let var = self.lower_expr(func, arg)?;
                    arg_vars.push(var);
                }
                let result = func.alloc_var();
                func.push(IrInst::Call(result, rt_name, arg_vars));
                return Ok(result);
            }

            // ユーザー定義関数呼び出し
            if self.user_funcs.contains(name) {
                // NB-14: Stack-based arg tag propagation. Only push/pop when at least
                // one argument needs tag propagation (avoids overhead for simple calls
                // like mutual recursion where all args are Int).
                let needs_tags = self.needs_call_arg_tags(args);
                if needs_tags {
                    let push_dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        push_dummy,
                        "taida_push_call_tags".to_string(),
                        vec![],
                    ));
                    // Pre-lower tags: set tags for args with compile-time known types
                    self.emit_call_arg_tags(func, args);
                }
                // Lower args (may include nested CallUser that populate return_tag_vars).
                // Clear tail flag during arg lowering so nested CallUser emit get_return_tag.
                let saved_tail = self.in_tail_call_return;
                self.in_tail_call_return = false;
                let mut explicit_arg_vars = Vec::with_capacity(args.len());
                for arg in args {
                    explicit_arg_vars.push(self.lower_expr(func, arg)?);
                }
                self.in_tail_call_return = saved_tail;
                if needs_tags {
                    // Post-lower tags: set tags for args whose type came from a call's return tag
                    self.emit_post_lower_arg_tags(func, args, &explicit_arg_vars);
                }
                let arg_vars =
                    self.lower_user_call_effective_args_from_vars(func, name, explicit_arg_vars)?;
                let result = func.alloc_var();
                let mangled = self.resolve_user_func_symbol(name);
                func.push(IrInst::CallUser(result, mangled, arg_vars));
                // NB-14: Capture return type tag from callee (skip in tail position for TCO)
                if !self.in_tail_call_return {
                    let return_tag = func.alloc_var();
                    func.push(IrInst::Call(
                        return_tag,
                        "taida_get_return_tag".to_string(),
                        vec![],
                    ));
                    self.return_tag_vars.insert(result, return_tag);
                }
                if needs_tags {
                    let pop_dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        pop_dummy,
                        "taida_pop_call_tags".to_string(),
                        vec![],
                    ));
                }
                return Ok(result);
            }

            // ラムダ変数経由の呼び出し
            // ラムダ変数 or 未知の変数呼び出し:
            // 全ラムダはクロージャ構造体として生成されるため、
            // lambda_vars に登録されているかどうかに関わらず
            // 統一的に CallIndirect で間接呼び出しする。
            {
                let mut arg_vars = Vec::new();
                for arg in args {
                    let var = self.lower_expr(func, arg)?;
                    arg_vars.push(var);
                }
                let closure_var = func.alloc_var();
                func.push(IrInst::UseVar(closure_var, name.clone()));
                let result = func.alloc_var();
                func.push(IrInst::CallIndirect(result, closure_var, arg_vars));
                return Ok(result);
            }
        }

        // 非 Ident の callee: ラムダ式や関数呼び出し結果（IIFE, カリー化）等
        // callee を評価し、結果をクロージャ/関数ポインタとして間接呼び出しする
        {
            let callee_var = self.lower_expr(func, callee)?;

            // callee がキャプチャなしラムダ（FuncAddr）の場合も
            // CallIndirect でクロージャとして呼ぶと壊れるため、
            // ここでは統一的に CallIndirect を使う。
            // ただし FuncAddr の場合はクロージャ構造体ではないので、
            // ラムダ式の場合はキャプチャの有無で分岐する。
            if let Expr::Lambda(_, _, _) = callee {
                // IIFE: lower_lambda で既に FuncAddr または MakeClosure が生成済み
                // キャプチャなしの場合は直接呼び出しが必要
                // → lower_lambda の戻り値が FuncAddr ならユーザー関数呼び出し
                //   MakeClosure なら間接呼び出し
                // 判定: lambda_funcs の最後に追加された関数の名前を使う
                if let Some(last_fn) = self.lambda_funcs.last() {
                    let lambda_name = last_fn.name.clone();
                    // キャプチャありかどうかは FuncAddr vs MakeClosure で判定
                    // → func.body の最後の命令を見る
                    let is_closure = func.body.iter().rev().any(
                        |inst| matches!(inst, IrInst::MakeClosure(v, _, _) if *v == callee_var),
                    );
                    if is_closure {
                        // NB-14: Stack-based arg tag propagation for IIFE closure calls
                        // (symmetric with named-function path at lower.rs:3082)
                        let needs_tags = self.needs_call_arg_tags(args);
                        if needs_tags {
                            let push_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                push_dummy,
                                "taida_push_call_tags".to_string(),
                                vec![],
                            ));
                            self.emit_call_arg_tags(func, args);
                        }
                        let saved_tail = self.in_tail_call_return;
                        self.in_tail_call_return = false;
                        let mut arg_vars = Vec::new();
                        for arg in args {
                            let var = self.lower_expr(func, arg)?;
                            arg_vars.push(var);
                        }
                        self.in_tail_call_return = saved_tail;
                        if needs_tags {
                            self.emit_post_lower_arg_tags(func, args, &arg_vars);
                        }
                        let result = func.alloc_var();
                        func.push(IrInst::CallIndirect(result, callee_var, arg_vars));
                        // NB-14: Capture return type tag from lambda (skip in tail position)
                        if !self.in_tail_call_return {
                            let return_tag = func.alloc_var();
                            func.push(IrInst::Call(
                                return_tag,
                                "taida_get_return_tag".to_string(),
                                vec![],
                            ));
                            self.return_tag_vars.insert(result, return_tag);
                        }
                        if needs_tags {
                            let pop_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                pop_dummy,
                                "taida_pop_call_tags".to_string(),
                                vec![],
                            ));
                        }
                        return Ok(result);
                    } else {
                        // NB-14: Stack-based arg tag propagation for direct lambda calls
                        // (symmetric with named-function path at lower.rs:3082)
                        let needs_tags = self.needs_call_arg_tags(args);
                        if needs_tags {
                            let push_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                push_dummy,
                                "taida_push_call_tags".to_string(),
                                vec![],
                            ));
                            self.emit_call_arg_tags(func, args);
                        }
                        // Lower args with tail flag cleared for nested CallUser
                        let saved_tail = self.in_tail_call_return;
                        self.in_tail_call_return = false;
                        let mut explicit_arg_vars = Vec::with_capacity(args.len());
                        for arg in args {
                            explicit_arg_vars.push(self.lower_expr(func, arg)?);
                        }
                        self.in_tail_call_return = saved_tail;
                        if needs_tags {
                            self.emit_post_lower_arg_tags(func, args, &explicit_arg_vars);
                        }
                        let result = func.alloc_var();
                        func.push(IrInst::CallUser(result, lambda_name, explicit_arg_vars));
                        // NB-14: Capture return type tag from callee (skip in tail position)
                        if !self.in_tail_call_return {
                            let return_tag = func.alloc_var();
                            func.push(IrInst::Call(
                                return_tag,
                                "taida_get_return_tag".to_string(),
                                vec![],
                            ));
                            self.return_tag_vars.insert(result, return_tag);
                        }
                        if needs_tags {
                            let pop_dummy = func.alloc_var();
                            func.push(IrInst::Call(
                                pop_dummy,
                                "taida_pop_call_tags".to_string(),
                                vec![],
                            ));
                        }
                        return Ok(result);
                    }
                }
            }

            // その他の非Lambda callee: 先に引数をlower
            let mut arg_vars = Vec::new();
            for arg in args {
                let var = self.lower_expr(func, arg)?;
                arg_vars.push(var);
            }

            // その他: 関数呼び出し結果やフィールドアクセス結果を間接呼び出し
            let result = func.alloc_var();
            func.push(IrInst::CallIndirect(result, callee_var, arg_vars));
            Ok(result)
        }
    }

    fn lower_debug_call(
        &mut self,
        func: &mut IrFunction,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if args.is_empty() {
            return Err(LowerError {
                message: "debug() requires at least one argument".to_string(),
            });
        }

        let mut last_result = None;
        for arg in args {
            let arg_var = self.lower_expr(func, arg)?;
            let runtime_fn = self.debug_fn_for_expr(arg);
            let result = func.alloc_var();
            func.push(IrInst::Call(result, runtime_fn, vec![arg_var]));
            last_result = Some(result);
        }
        Ok(last_result.unwrap())
    }

    fn debug_fn_for_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::IntLit(..) => "taida_debug_int".to_string(),
            Expr::FloatLit(..) => "taida_debug_float".to_string(),
            Expr::StringLit(..) => "taida_debug_str".to_string(),
            Expr::BoolLit(..) => "taida_debug_bool".to_string(),
            Expr::BinaryOp(
                _,
                BinOp::Eq
                | BinOp::NotEq
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::And
                | BinOp::Or,
                _,
                _,
            ) => "taida_debug_bool".to_string(),
            Expr::BinaryOp(..) => "taida_debug_int".to_string(),
            Expr::UnaryOp(UnaryOp::Not, _, _) => "taida_debug_bool".to_string(),
            Expr::UnaryOp(UnaryOp::Neg, _, _) => "taida_debug_int".to_string(),
            Expr::Ident(name, _) => {
                if self.float_vars.contains(name) {
                    "taida_debug_float".to_string()
                } else if self.string_vars.contains(name) {
                    "taida_debug_str".to_string()
                } else if self.bool_vars.contains(name) {
                    "taida_debug_bool".to_string()
                } else if self.pack_vars.contains(name)
                    || self.list_vars.contains(name)
                    || self.closure_vars.contains(name)
                {
                    "taida_debug_polymorphic".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
            Expr::MethodCall(_, method, _, _) => {
                if self.expr_is_bool(expr) {
                    "taida_debug_bool".to_string()
                } else if matches!(method.as_str(), "toString" | "toStr") {
                    "taida_debug_str".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    if self.string_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_str".to_string();
                    }
                    if self.float_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_float".to_string();
                    }
                    if self.bool_returning_funcs.contains(name.as_str()) {
                        return "taida_debug_bool".to_string();
                    }
                }
                "taida_debug_int".to_string()
            }
            Expr::FieldAccess(receiver, _, _) => {
                // Field access on a pack: use polymorphic to_string + debug_str
                // because field types are not always tracked
                if self.expr_is_string_full(expr) {
                    "taida_debug_str".to_string()
                } else if self.expr_returns_float(expr) {
                    "taida_debug_float".to_string()
                } else if self.expr_is_bool(expr) {
                    "taida_debug_bool".to_string()
                } else if self.expr_is_pack(receiver) || self.expr_is_list(receiver) {
                    // Pack field or list: could be any type, use polymorphic
                    "taida_debug_polymorphic".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
            // Catch-all: use type detection helpers before defaulting to int
            _ => {
                if self.expr_is_string_full(expr) {
                    "taida_debug_str".to_string()
                } else if self.expr_returns_float(expr) {
                    "taida_debug_float".to_string()
                } else if self.expr_is_bool(expr) {
                    "taida_debug_bool".to_string()
                } else if self.expr_is_pack(expr) || self.expr_is_list(expr) {
                    "taida_debug_polymorphic".to_string()
                } else {
                    "taida_debug_int".to_string()
                }
            }
        }
    }

    fn lower_binary_op(
        &mut self,
        func: &mut IrFunction,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<IrVar, LowerError> {
        let lhs_var = self.lower_expr(func, lhs)?;
        let rhs_var = self.lower_expr(func, rhs)?;

        // Add (+) with string operands → string concatenation
        let lhs_is_str = self.expr_is_string_full(lhs);
        let rhs_is_str = self.expr_is_string_full(rhs);

        let runtime_fn = match op {
            BinOp::Add => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_concat"
                } else if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    // Float arithmetic: use float add
                    "taida_float_add"
                } else if self.expr_type_is_unknown(lhs) || self.expr_type_is_unknown(rhs) {
                    // FL-16: untyped operand (e.g. function param without annotation)
                    // → use polymorphic add that dispatches at runtime
                    "taida_poly_add"
                } else {
                    "taida_int_add"
                }
            }
            BinOp::Sub => {
                if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_sub"
                } else {
                    "taida_int_sub"
                }
            }
            BinOp::Mul => {
                if self.expr_returns_float(lhs) || self.expr_returns_float(rhs) {
                    "taida_float_mul"
                } else {
                    "taida_int_mul"
                }
            }
            // BinOp::Div and BinOp::Mod removed — use Div[x, y]() and Mod[x, y]() molds
            BinOp::Eq => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_eq"
                } else if self.expr_returns_float(lhs)
                    || self.expr_returns_float(rhs)
                    || self.expr_is_bool(lhs)
                    || self.expr_is_bool(rhs)
                    || matches!(lhs, Expr::IntLit(_, _))
                    || matches!(rhs, Expr::IntLit(_, _))
                {
                    "taida_int_eq"
                } else {
                    "taida_poly_eq"
                }
            }
            BinOp::NotEq => {
                if lhs_is_str || rhs_is_str {
                    "taida_str_neq"
                } else if self.expr_returns_float(lhs)
                    || self.expr_returns_float(rhs)
                    || self.expr_is_bool(lhs)
                    || self.expr_is_bool(rhs)
                    || matches!(lhs, Expr::IntLit(_, _))
                    || matches!(rhs, Expr::IntLit(_, _))
                {
                    "taida_int_neq"
                } else {
                    "taida_poly_neq"
                }
            }
            BinOp::Lt => "taida_int_lt",
            BinOp::Gt => "taida_int_gt",
            BinOp::GtEq => "taida_int_gte",
            BinOp::And => "taida_bool_and",
            BinOp::Or => "taida_bool_or",
            BinOp::Concat => "taida_str_concat",
        };
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![lhs_var, rhs_var],
        ));
        Ok(result)
    }

    fn lower_unary_op(
        &mut self,
        func: &mut IrFunction,
        op: &UnaryOp,
        operand: &Expr,
    ) -> Result<IrVar, LowerError> {
        let operand_var = self.lower_expr(func, operand)?;
        let runtime_fn = match op {
            UnaryOp::Neg => {
                if self.expr_returns_float(operand) {
                    "taida_float_neg"
                } else {
                    "taida_int_neg"
                }
            }
            UnaryOp::Not => "taida_bool_not",
        };
        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            runtime_fn.to_string(),
            vec![operand_var],
        ));
        Ok(result)
    }

    /// パイプライン: `a => f(_) => g(_)` → 各段の結果を次の引数に
    fn lower_pipeline(
        &mut self,
        func: &mut IrFunction,
        exprs: &[Expr],
    ) -> Result<IrVar, LowerError> {
        if exprs.is_empty() {
            return Err(LowerError {
                message: "empty pipeline".to_string(),
            });
        }

        // 最初の式を評価
        let mut current = self.lower_expr(func, &exprs[0])?;

        // 残りの式について、_ を前の結果で置換して評価
        for expr in &exprs[1..] {
            current = self.lower_pipeline_step(func, expr, current)?;
        }

        Ok(current)
    }

    fn lower_pipeline_step(
        &mut self,
        func: &mut IrFunction,
        expr: &Expr,
        prev_result: IrVar,
    ) -> Result<IrVar, LowerError> {
        match expr {
            // f(_) → f(prev_result)
            Expr::FuncCall(callee, args, span) => {
                let new_args: Vec<Expr> = args
                    .iter()
                    .map(|arg| {
                        if matches!(arg, Expr::Placeholder(_)) {
                            // _ を prev_result を指す特殊マーカーに置換
                            // ここでは直接 IrVar を渡せないので、
                            // Ident 参照に変換して DefVar で仮名をつける
                            Expr::Ident("__pipe_prev".to_string(), span.clone())
                        } else {
                            arg.clone()
                        }
                    })
                    .collect();

                // prev_result を __pipe_prev として定義
                func.push(IrInst::DefVar("__pipe_prev".to_string(), prev_result));

                self.lower_func_call(func, callee, &new_args)
            }
            // 変数名のみ（関数として呼び出し）: `expr => func_name`
            Expr::Ident(name, _) => {
                if name == "debug" {
                    // debug は特殊: debug(prev)
                    let result = func.alloc_var();
                    func.push(IrInst::Call(
                        result,
                        "taida_debug_int".to_string(),
                        vec![prev_result],
                    ));
                    return Ok(result);
                }
                if self.user_funcs.contains(name) {
                    let arg_vars = self.lower_user_call_effective_args_from_vars(
                        func,
                        name,
                        vec![prev_result],
                    )?;
                    let result = func.alloc_var();
                    let mangled = self.resolve_user_func_symbol(name);
                    func.push(IrInst::CallUser(result, mangled, arg_vars));
                    return Ok(result);
                }
                Err(LowerError {
                    message: format!("unknown pipeline target: {}", name),
                })
            }
            _ => Err(LowerError {
                message: "unsupported pipeline step".to_string(),
            }),
        }
    }

    /// ぶちパック: `@(field <= value, ...)`
    fn lower_buchi_pack(
        &mut self,
        func: &mut IrFunction,
        fields: &[BuchiField],
    ) -> Result<IrVar, LowerError> {
        // QF-16: Placeholder 値のフィールドをスキップ（=> :Type が Placeholder として
        // パースされるため、BuchiPack 内ラムダの戻り値型注釈が不正なフィールドになる）
        let real_fields: Vec<_> = fields
            .iter()
            .filter(|f| !matches!(f.value, Expr::Placeholder(_)))
            .collect();
        let pack_var = func.alloc_var();
        func.push(IrInst::PackNew(pack_var, real_fields.len()));

        for (i, field) in real_fields.iter().enumerate() {
            // Register field name for jsonEncode
            self.field_names.insert(field.name.clone());

            // Detect Bool fields at compile time for field type registry
            let is_bool = self.expr_is_bool(&field.value);
            if is_bool {
                self.register_field_type_tag(&field.name, 4); // 4 = Bool
            }

            // Emit inline field registration for jsonEncode (ensures library modules
            // register their field names at runtime, not just in _taida_main)
            let hash = simple_hash(&field.name);
            let type_tag = if is_bool {
                4
            } else {
                self.field_type_tags.get(&field.name).copied().unwrap_or(0)
            };
            self.emit_field_registration_inline(func, &field.name, hash, type_tag);

            // フィールド名ハッシュを設定
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let idx_var = func.alloc_var();
            func.push(IrInst::ConstInt(idx_var, i as i64));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_pack_set_hash".to_string(),
                vec![pack_var, idx_var, hash_var],
            ));

            let val = self.lower_expr(func, &field.value)?;
            func.push(IrInst::PackSet(pack_var, i, val));
            // A-4c: Set type tag for this field value
            let val_tag = self.expr_type_tag(&field.value);
            if val_tag == -1 {
                // NB-14: UNKNOWN tag -- check if the value comes from a function
                // parameter with a runtime tag var (caller-propagated Bool/Int info).
                if let Some(tag_var) = self.get_param_tag_var(&field.value) {
                    // Use the runtime tag from the caller via taida_pack_set_tag()
                    let idx_var2 = func.alloc_var();
                    func.push(IrInst::ConstInt(idx_var2, i as i64));
                    let dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        dummy,
                        "taida_pack_set_tag".to_string(),
                        vec![pack_var, idx_var2, tag_var],
                    ));
                } else {
                    func.push(IrInst::PackSetTag(pack_var, i, val_tag));
                }
            } else if val_tag != 0 {
                func.push(IrInst::PackSetTag(pack_var, i, val_tag));
            }
            // retain-on-store: 再帰 release に対応するため子を retain
            self.emit_retain_if_heap_tag(func, val, val_tag);
        }

        Ok(pack_var)
    }

    /// 型インスタンス化: `TypeName(field <= value, ...)`
    /// Adds __type field (like interpreter) so jsonEncode can include it.
    fn lower_type_inst(
        &mut self,
        func: &mut IrFunction,
        type_name: &str,
        fields: &[BuchiField],
    ) -> Result<IrVar, LowerError> {
        let mut materialized_fields: Vec<(String, IrVar)> = Vec::new();

        if let Some(type_fields) = self.type_field_defs.get(type_name).cloned() {
            let mut consumed = std::collections::HashSet::new();
            let mut visiting = std::collections::HashSet::new();
            for field_def in type_fields.iter().filter(|f| !f.is_method) {
                let value_var = if let Some(provided) =
                    fields.iter().rev().find(|f| f.name == field_def.name)
                {
                    self.lower_expr(func, &provided.value)?
                } else {
                    self.lower_default_for_field_def(func, field_def, &mut visiting)?
                };
                materialized_fields.push((field_def.name.clone(), value_var));
                consumed.insert(field_def.name.clone());
            }
            // Keep undeclared fields for structural flexibility (interpreter parity).
            for field in fields {
                if !consumed.contains(&field.name) {
                    let val = self.lower_expr(func, &field.value)?;
                    materialized_fields.push((field.name.clone(), val));
                }
            }
        } else {
            for field in fields {
                let val = self.lower_expr(func, &field.value)?;
                materialized_fields.push((field.name.clone(), val));
            }
        }

        // Generate method closures that capture the data fields.
        // Each method becomes a closure with the data fields as its environment.
        let method_defs = self
            .type_method_defs
            .get(type_name)
            .cloned()
            .unwrap_or_default();
        let data_field_names: Vec<String> =
            materialized_fields.iter().map(|(n, _)| n.clone()).collect();

        // Register data field values as named variables so MakeClosure can capture them.
        // Use unique temporary names to avoid conflicts with existing variables.
        let capture_prefix = format!("__typeinst_{}_{}_", type_name, self.lambda_counter);
        let capture_names: Vec<String> = data_field_names
            .iter()
            .map(|n| format!("{}{}", capture_prefix, n))
            .collect();
        for ((_field_name, field_val), cap_name) in
            materialized_fields.iter().zip(capture_names.iter())
        {
            func.push(IrInst::DefVar(cap_name.clone(), *field_val));
        }

        let mut method_closures: Vec<(String, IrVar)> = Vec::new();
        for (method_name, method_func_def) in &method_defs {
            let closure_var = self.lower_type_method_closure(
                func,
                type_name,
                method_name,
                method_func_def,
                &capture_names,
                &data_field_names,
            )?;
            method_closures.push((method_name.clone(), closure_var));
        }

        // Create pack with slots for data fields + method closures + __type.
        let total_fields = materialized_fields.len() + method_closures.len() + 1;
        let pack_var = func.alloc_var();
        func.push(IrInst::PackNew(pack_var, total_fields));

        // Set user/defaulted fields.
        for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
            self.emit_pack_field_hash(func, pack_var, i, field_name);
            func.push(IrInst::PackSet(pack_var, i, *field_val));
            // A-4c: determine type tag from field_type_tags registry or TypeDef field types
            let tag = self.type_field_type_tag(type_name, field_name);
            if tag != 0 {
                func.push(IrInst::PackSetTag(pack_var, i, tag));
            }
            // retain-on-store
            self.emit_retain_if_heap_tag(func, *field_val, tag);
        }

        // Set method closure fields.
        let method_offset = materialized_fields.len();
        for (i, (method_name, closure_var)) in method_closures.iter().enumerate() {
            let slot = method_offset + i;
            self.emit_pack_field_hash(func, pack_var, slot, method_name);
            func.push(IrInst::PackSet(pack_var, slot, *closure_var));
            func.push(IrInst::PackSetTag(pack_var, slot, 6)); // TAIDA_TAG_CLOSURE
            // retain-on-store: method closure
            func.push(IrInst::Retain(*closure_var));
        }

        // Set __type field at the last slot.
        let type_slot = materialized_fields.len() + method_closures.len();
        self.emit_pack_field_hash(func, pack_var, type_slot, "__type");
        let type_str_var = func.alloc_var();
        func.push(IrInst::ConstStr(type_str_var, type_name.to_string()));
        func.push(IrInst::PackSet(pack_var, type_slot, type_str_var));
        func.push(IrInst::PackSetTag(pack_var, type_slot, 3)); // TAIDA_TAG_STR

        Ok(pack_var)
    }

    /// Generate a closure for a TypeDef method.
    /// The closure captures all data fields of the instance as its environment.
    /// `capture_names` are the unique temporary variable names used for MakeClosure.
    /// `data_field_names` are the original field names restored inside the method body.
    fn lower_type_method_closure(
        &mut self,
        func: &mut IrFunction,
        type_name: &str,
        _method_name: &str,
        method_func_def: &FuncDef,
        capture_names: &[String],
        data_field_names: &[String],
    ) -> Result<IrVar, LowerError> {
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_method_{}_{}", type_name, lambda_id);

        // The method function takes __env as the first parameter,
        // followed by the method's own parameters.
        let mut ir_params: Vec<String> = vec!["__env".to_string()];
        ir_params.extend(method_func_def.params.iter().map(|p| p.name.clone()));

        let mut method_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

        // Restore data fields from the environment pack.
        let env_var = 0u32; // __env is parameter 0
        for (i, field_name) in data_field_names.iter().enumerate() {
            let get_dst = method_fn.alloc_var();
            method_fn.push(IrInst::PackGet(get_dst, env_var, i));
            method_fn.push(IrInst::DefVar(field_name.clone(), get_dst));
        }

        // Pre-process local function definitions in the method body.
        // These need to be lowered as separate IR functions and registered
        // in user_funcs before the method body is lowered.
        for stmt in &method_func_def.body {
            if let Statement::FuncDef(inner_func_def) = stmt {
                self.user_funcs.insert(inner_func_def.name.clone());
                // Store parameter definitions for arity/default resolution
                self.func_param_defs
                    .insert(inner_func_def.name.clone(), inner_func_def.params.clone());
                let ir_func = self.lower_func_def(inner_func_def)?;
                self.lambda_funcs.push(ir_func);
            }
        }

        // Lower method body (same pattern as lower_func_def).
        let prev_heap = std::mem::take(&mut self.current_heap_vars);
        let prev_func_name = self.current_func_name.take();

        let mut last_var = None;
        let body_refs: Vec<&Statement> = method_func_def.body.iter().collect();
        let has_error_ceiling = body_refs
            .iter()
            .any(|s| matches!(s, Statement::ErrorCeiling(_)));

        if has_error_ceiling {
            self.lower_statement_sequence(&mut method_fn, &body_refs)?;
        } else {
            for (i, stmt) in method_func_def.body.iter().enumerate() {
                let is_last = i == method_func_def.body.len() - 1;
                match stmt {
                    Statement::Expr(expr) => {
                        let var = self.lower_expr(&mut method_fn, expr)?;
                        if is_last {
                            last_var = Some(var);
                        }
                    }
                    _ => {
                        self.lower_statement(&mut method_fn, stmt)?;
                    }
                }
            }
        }

        self.current_func_name = prev_func_name;
        let _heap_vars = std::mem::replace(&mut self.current_heap_vars, prev_heap);

        // Implicit return value
        if let Some(ret) = last_var {
            method_fn.push(IrInst::Return(ret));
        } else {
            let zero = method_fn.alloc_var();
            method_fn.push(IrInst::ConstInt(zero, 0));
            method_fn.push(IrInst::Return(zero));
        }

        self.user_funcs.insert(lambda_name.clone());
        self.lambda_funcs.push(method_fn);

        // Create closure: capture all data field values as environment
        let dst = func.alloc_var();
        func.push(IrInst::MakeClosure(
            dst,
            lambda_name,
            capture_names.to_vec(),
        ));
        Ok(dst)
    }

    pub(crate) fn emit_pack_field_hash(
        &mut self,
        func: &mut IrFunction,
        pack_var: IrVar,
        index: usize,
        field_name: &str,
    ) {
        self.field_names.insert(field_name.to_string());
        if field_name == "__type" {
            self.register_field_type_tag("__type", 3);
        }
        let hash = simple_hash(field_name);

        // Emit inline field registration for jsonEncode (library module support)
        let type_tag = self.field_type_tags.get(field_name).copied().unwrap_or(0);
        self.emit_field_registration_inline(func, field_name, hash, type_tag);

        let hash_var = func.alloc_var();
        func.push(IrInst::ConstInt(hash_var, hash as i64));
        let idx_var = func.alloc_var();
        func.push(IrInst::ConstInt(idx_var, index as i64));
        let result_var = func.alloc_var();
        func.push(IrInst::Call(
            result_var,
            "taida_pack_set_hash".to_string(),
            vec![pack_var, idx_var, hash_var],
        ));
    }

    /// Emit inline taida_register_field_name/taida_register_field_type calls.
    /// This ensures field names are registered at runtime even in library modules
    /// that don't have a _taida_main to batch-register field names.
    /// The C runtime's registry handles duplicates safely (skips if already registered).
    fn emit_field_registration_inline(
        &mut self,
        func: &mut IrFunction,
        field_name: &str,
        hash: u64,
        type_tag: i64,
    ) {
        if type_tag > 0 {
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let name_var = func.alloc_var();
            func.push(IrInst::ConstStr(name_var, field_name.to_string()));
            let tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(tag_var, type_tag));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_register_field_type".to_string(),
                vec![hash_var, name_var, tag_var],
            ));
        } else {
            let hash_var = func.alloc_var();
            func.push(IrInst::ConstInt(hash_var, hash as i64));
            let name_var = func.alloc_var();
            func.push(IrInst::ConstStr(name_var, field_name.to_string()));
            let result_var = func.alloc_var();
            func.push(IrInst::Call(
                result_var,
                "taida_register_field_name".to_string(),
                vec![hash_var, name_var],
            ));
        }
    }

    fn lower_default_for_field_def(
        &mut self,
        func: &mut IrFunction,
        field_def: &FieldDef,
        visiting: &mut std::collections::HashSet<String>,
    ) -> Result<IrVar, LowerError> {
        if let Some(default_expr) = &field_def.default_value {
            return self.lower_expr(func, default_expr);
        }
        if let Some(type_expr) = &field_def.type_annotation {
            return self.lower_default_for_type_expr(func, type_expr, visiting);
        }
        let zero = func.alloc_var();
        func.push(IrInst::ConstInt(zero, 0));
        Ok(zero)
    }

    fn lower_default_for_type_expr(
        &mut self,
        func: &mut IrFunction,
        type_expr: &TypeExpr,
        visiting: &mut std::collections::HashSet<String>,
    ) -> Result<IrVar, LowerError> {
        match type_expr {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" | "Num" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstInt(v, 0));
                    Ok(v)
                }
                "Float" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstFloat(v, 0.0));
                    Ok(v)
                }
                "Str" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstStr(v, String::new()));
                    Ok(v)
                }
                "Bool" => {
                    let v = func.alloc_var();
                    func.push(IrInst::ConstBool(v, false));
                    Ok(v)
                }
                _ => {
                    if visiting.contains(name) {
                        let pack_var = func.alloc_var();
                        func.push(IrInst::PackNew(pack_var, 1));
                        self.emit_pack_field_hash(func, pack_var, 0, "__type");
                        let type_var = func.alloc_var();
                        func.push(IrInst::ConstStr(type_var, name.clone()));
                        func.push(IrInst::PackSet(pack_var, 0, type_var));
                        func.push(IrInst::PackSetTag(pack_var, 0, 3)); // TAIDA_TAG_STR
                        return Ok(pack_var);
                    }
                    if let Some(type_fields) = self.type_field_defs.get(name).cloned() {
                        visiting.insert(name.clone());
                        let mut materialized_fields: Vec<(String, IrVar)> = Vec::new();
                        for field in type_fields.iter().filter(|f| !f.is_method) {
                            let val = self.lower_default_for_field_def(func, field, visiting)?;
                            materialized_fields.push((field.name.clone(), val));
                        }
                        visiting.remove(name);

                        let pack_var = func.alloc_var();
                        func.push(IrInst::PackNew(pack_var, materialized_fields.len() + 1));
                        for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
                            self.emit_pack_field_hash(func, pack_var, i, field_name);
                            func.push(IrInst::PackSet(pack_var, i, *field_val));
                            // A-4c: Type tag for default fields (based on TypeDef field types)
                            let tag = self.type_field_type_tag(name, field_name);
                            if tag != 0 {
                                func.push(IrInst::PackSetTag(pack_var, i, tag));
                            }
                            // retain-on-store
                            self.emit_retain_if_heap_tag(func, *field_val, tag);
                        }
                        self.emit_pack_field_hash(
                            func,
                            pack_var,
                            materialized_fields.len(),
                            "__type",
                        );
                        let type_var = func.alloc_var();
                        func.push(IrInst::ConstStr(type_var, name.clone()));
                        func.push(IrInst::PackSet(
                            pack_var,
                            materialized_fields.len(),
                            type_var,
                        ));
                        func.push(IrInst::PackSetTag(pack_var, materialized_fields.len(), 3)); // TAIDA_TAG_STR
                        return Ok(pack_var);
                    }

                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    Ok(zero)
                }
            },
            TypeExpr::List(_) => {
                let list = func.alloc_var();
                func.push(IrInst::Call(list, "taida_list_new".to_string(), vec![]));
                Ok(list)
            }
            TypeExpr::BuchiPack(fields) => {
                let mut materialized_fields: Vec<(String, IrVar)> = Vec::new();
                for field in fields.iter().filter(|f| !f.is_method) {
                    let val = self.lower_default_for_field_def(func, field, visiting)?;
                    materialized_fields.push((field.name.clone(), val));
                }
                let pack_var = func.alloc_var();
                func.push(IrInst::PackNew(pack_var, materialized_fields.len()));
                for (i, (field_name, field_val)) in materialized_fields.iter().enumerate() {
                    self.emit_pack_field_hash(func, pack_var, i, field_name);
                    func.push(IrInst::PackSet(pack_var, i, *field_val));
                }
                Ok(pack_var)
            }
            TypeExpr::Generic(name, args) if name == "Lax" => {
                let inner = if let Some(inner_ty) = args.first() {
                    self.lower_default_for_type_expr(func, inner_ty, visiting)?
                } else {
                    let zero = func.alloc_var();
                    func.push(IrInst::ConstInt(zero, 0));
                    zero
                };
                let pack_var = func.alloc_var();
                func.push(IrInst::PackNew(pack_var, 4));

                self.emit_pack_field_hash(func, pack_var, 0, "hasValue");
                let has_value = func.alloc_var();
                func.push(IrInst::ConstBool(has_value, false));
                func.push(IrInst::PackSet(pack_var, 0, has_value));
                func.push(IrInst::PackSetTag(pack_var, 0, 2)); // TAIDA_TAG_BOOL

                self.emit_pack_field_hash(func, pack_var, 1, "__value");
                func.push(IrInst::PackSet(pack_var, 1, inner));

                self.emit_pack_field_hash(func, pack_var, 2, "__default");
                func.push(IrInst::PackSet(pack_var, 2, inner));

                self.emit_pack_field_hash(func, pack_var, 3, "__type");
                let lax_type = func.alloc_var();
                func.push(IrInst::ConstStr(lax_type, "Lax".to_string()));
                func.push(IrInst::PackSet(pack_var, 3, lax_type));
                func.push(IrInst::PackSetTag(pack_var, 3, 3)); // TAIDA_TAG_STR
                Ok(pack_var)
            }
            TypeExpr::Generic(_, _) | TypeExpr::Function(_, _) => {
                let zero = func.alloc_var();
                func.push(IrInst::ConstInt(zero, 0));
                Ok(zero)
            }
        }
    }

    /// フィールドアクセス: `expr.field`
    fn lower_field_access(
        &mut self,
        func: &mut IrFunction,
        obj: &Expr,
        field: &str,
    ) -> Result<IrVar, LowerError> {
        let obj_var = self.lower_expr(func, obj)?;

        // フィールドのインデックスをランタイムで解決
        // ランタイム関数 taida_pack_get_by_name(pack, field_name_hash) を使う
        let field_hash = simple_hash(field);
        let hash_var = func.alloc_var();
        func.push(IrInst::ConstInt(hash_var, field_hash as i64));

        let result = func.alloc_var();
        func.push(IrInst::Call(
            result,
            "taida_pack_get".to_string(),
            vec![obj_var, hash_var],
        ));
        Ok(result)
    }

    /// 空スロット部分適用: `func(5, )` → ラムダ（クロージャ）を生成
    /// Hole 位置のパラメータを持つクロージャを作り、non-hole 引数はキャプチャする。
    /// 旧 `_` (Placeholder) 部分適用は checker (E1502) で拒否済み。
    fn lower_partial_application(
        &mut self,
        func: &mut IrFunction,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<IrVar, LowerError> {
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_partial_{}", lambda_id);

        // Evaluate non-hole arguments and track hole positions
        let mut captured_vars: Vec<(usize, IrVar)> = Vec::new(); // (arg_index, ir_var)
        let mut hole_count = 0usize;
        for (i, arg) in args.iter().enumerate() {
            if matches!(arg, Expr::Hole(_)) {
                hole_count += 1;
            } else {
                let var = self.lower_expr(func, arg)?;
                captured_vars.push((i, var));
            }
        }

        // Build a lambda function: __env holds captured non-hole args,
        // parameters are the hole slots
        let mut ir_params: Vec<String> = vec!["__env".to_string()];
        for i in 0..hole_count {
            ir_params.push(format!("__pa_{}", i));
        }

        let mut lambda_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

        // Restore captured args from environment pack
        for (pack_idx, (arg_idx, _)) in captured_vars.iter().enumerate() {
            let dst = lambda_fn.alloc_var();
            lambda_fn.push(IrInst::PackGet(dst, 0u32, pack_idx));
            lambda_fn.push(IrInst::DefVar(format!("__pa_cap_{}", arg_idx), dst));
        }

        // Build the actual call arguments in order
        let mut call_args = Vec::new();
        let mut hole_idx = 0usize;
        for (i, arg) in args.iter().enumerate() {
            if matches!(arg, Expr::Hole(_)) {
                let v = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::UseVar(v, format!("__pa_{}", hole_idx)));
                call_args.push(v);
                hole_idx += 1;
            } else {
                let v = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::UseVar(v, format!("__pa_cap_{}", i)));
                call_args.push(v);
            }
        }

        // Generate the call inside the lambda
        let result = lambda_fn.alloc_var();
        if let Expr::Ident(name, _) = callee {
            if self.user_funcs.contains(name) {
                let mangled = self.resolve_user_func_symbol(name);
                lambda_fn.push(IrInst::CallUser(result, mangled, call_args));
            } else if let Some(rt_name) = self.stdlib_runtime_funcs.get(name).cloned() {
                lambda_fn.push(IrInst::Call(result, rt_name, call_args));
            } else {
                // Lambda/closure variable call
                let closure_var = lambda_fn.alloc_var();
                // Need to restore callee from globals or environment
                self.globals_referenced.insert(name.clone());
                let hash = self.global_var_hash(name);
                lambda_fn.push(IrInst::GlobalGet(closure_var, hash));
                lambda_fn.push(IrInst::CallIndirect(result, closure_var, call_args));
            }
        } else {
            // Non-ident callee: evaluate in parent, capture, and call indirectly
            let callee_var = self.lower_expr(func, callee)?;
            captured_vars.push((usize::MAX, callee_var)); // special capture for callee
            let callee_restore = lambda_fn.alloc_var();
            lambda_fn.push(IrInst::PackGet(
                callee_restore,
                0u32,
                captured_vars.len() - 1,
            ));
            lambda_fn.push(IrInst::CallIndirect(result, callee_restore, call_args));
        }

        lambda_fn.push(IrInst::Return(result));

        self.user_funcs.insert(lambda_name.clone());
        self.lambda_funcs.push(lambda_fn);

        // Create closure with captured values
        let capture_names: Vec<String> = captured_vars
            .iter()
            .map(|(idx, _)| {
                if *idx == usize::MAX {
                    "__pa_callee".to_string()
                } else {
                    format!("__pa_cap_{}", idx)
                }
            })
            .collect();

        // Store captured values in current scope so MakeClosure can find them
        for (cap_name, (_, ir_var)) in capture_names.iter().zip(captured_vars.iter()) {
            func.push(IrInst::DefVar(cap_name.clone(), *ir_var));
        }

        let dst = func.alloc_var();
        func.push(IrInst::MakeClosure(dst, lambda_name, capture_names));
        Ok(dst)
    }

    /// ラムダ式: `_ x = x * 2`
    /// キャプチャなしの場合は通常の関数として生成
    /// キャプチャありの場合はクロージャ（ファットポインタ）を生成
    fn lower_lambda(
        &mut self,
        func: &mut IrFunction,
        params: &[Param],
        body: &Expr,
    ) -> Result<IrVar, LowerError> {
        let lambda_id = self.lambda_counter;
        self.lambda_counter += 1;
        let lambda_name = format!("_taida_lambda_{}", lambda_id);

        // キャプチャ変数の検出: ラムダ本体で使われる変数のうち、
        // パラメータでもなく、ユーザー定義関数でもないもの
        let param_names: std::collections::HashSet<&str> =
            params.iter().map(|p| p.name.as_str()).collect();
        let free_vars = self.collect_free_vars(body, &param_names);

        // Scope-aware net builtin shadowing: snapshot/restore for lambda scope
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        // NB3-4: Snapshot var_aliases, lambda_param_counts, lambda_vars, closure_vars
        // for lambda scope
        let prev_var_aliases = self.var_aliases.clone();
        let prev_lambda_param_counts = self.lambda_param_counts.clone();
        let prev_lambda_vars = self.lambda_vars.clone();
        let prev_closure_vars = self.closure_vars.clone();
        for p in params {
            if Self::NET_BUILTIN_NAMES.contains(&p.name.as_str()) {
                self.shadowed_net_builtins.insert(p.name.clone());
            }
            // NB3-4 parameter shadow: remove outer-scope aliases so that
            // resolve_ident_arity / resolve_ident_callable_tag return unknown (-1)
            // for parameters that shadow outer aliases.
            self.var_aliases.remove(&p.name);
            self.lambda_param_counts.remove(&p.name);
            self.lambda_vars.remove(&p.name);
            self.closure_vars.remove(&p.name);
        }

        // 全ラムダを統一的にクロージャとして生成する。
        // キャプチャなしでも __env を第1引数として受け取り（未使用）、
        // MakeClosure で空の環境と共にクロージャ構造体を生成する。
        // これにより、ラムダが関数から返されたり変数に格納されたりしても、
        // 常に CallIndirect で安全に呼び出せる。
        {
            let mut ir_params: Vec<String> = vec!["__env".to_string()];
            ir_params.extend(params.iter().map(|p| p.name.clone()));

            let mut lambda_fn = IrFunction::new_with_params(lambda_name.clone(), ir_params);

            // 環境からキャプチャ変数を復元（キャプチャなしの場合はスキップ）
            if !free_vars.is_empty() {
                let env_var = 0u32; // __env は第0パラメータ
                for (i, free_var) in free_vars.iter().enumerate() {
                    let get_dst = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::PackGet(get_dst, env_var, i));
                    lambda_fn.push(IrInst::DefVar(free_var.clone(), get_dst));
                }
            }

            // NB-14: Emit taida_get_call_arg_tag() for lambda params whose type
            // cannot be determined at compile time. This is the callee-side mirror
            // of the caller-side push/set/pop in the IIFE CallIndirect path.
            let prev_param_tag_vars = std::mem::take(&mut self.param_tag_vars);
            let prev_return_tag_vars = std::mem::take(&mut self.return_tag_vars);
            for (i, param) in params.iter().enumerate() {
                let has_known_type = self.bool_vars.contains(&param.name)
                    || self.int_vars.contains(&param.name)
                    || self.float_vars.contains(&param.name)
                    || self.string_vars.contains(&param.name)
                    || self.pack_vars.contains(&param.name)
                    || self.list_vars.contains(&param.name)
                    || self.closure_vars.contains(&param.name);
                if !has_known_type {
                    let idx_var = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::ConstInt(idx_var, i as i64));
                    let tag_var = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::Call(
                        tag_var,
                        "taida_get_call_arg_tag".to_string(),
                        vec![idx_var],
                    ));
                    self.param_tag_vars.insert(param.name.clone(), tag_var);
                }
            }

            let body_var = self.lower_expr(&mut lambda_fn, body)?;

            // NB-14: Set return type tag before Return (symmetric with lower_func_def)
            if let Some(&rtv) = self.return_tag_vars.get(&body_var) {
                let dummy = lambda_fn.alloc_var();
                lambda_fn.push(IrInst::Call(
                    dummy,
                    "taida_set_return_tag".to_string(),
                    vec![rtv],
                ));
            } else {
                let tag = self.expr_type_tag(body);
                if tag > 0 {
                    let tag_var = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::ConstInt(tag_var, tag));
                    let dummy = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![tag_var],
                    ));
                } else if tag == -1
                    && let Some(ptv) = self.get_param_tag_var(body)
                {
                    let dummy = lambda_fn.alloc_var();
                    lambda_fn.push(IrInst::Call(
                        dummy,
                        "taida_set_return_tag".to_string(),
                        vec![ptv],
                    ));
                }
            }

            lambda_fn.push(IrInst::Return(body_var));

            // NB-14: Restore param/return tag vars to pre-lambda state
            // (lambda-scope IrVars must not leak into outer function scope)
            self.param_tag_vars = prev_param_tag_vars;
            self.return_tag_vars = prev_return_tag_vars;

            self.user_funcs.insert(lambda_name.clone());
            self.lambda_funcs.push(lambda_fn);

            // クロージャ生成: 環境パックを作り、MakeClosure を発行
            // （キャプチャなしの場合は空の環境パック）
            let dst = func.alloc_var();
            func.push(IrInst::MakeClosure(dst, lambda_name, free_vars));

            // Restore net builtin shadow set to pre-lambda state
            self.shadowed_net_builtins = prev_shadowed_net;
            // NB3-4: Restore var_aliases, lambda_param_counts, lambda_vars, closure_vars
            // to pre-lambda state (parameter shadow cleanup)
            self.var_aliases = prev_var_aliases;
            self.lambda_param_counts = prev_lambda_param_counts;
            self.lambda_vars = prev_lambda_vars;
            self.closure_vars = prev_closure_vars;

            Ok(dst)
        }
    }

    /// クロージャ本体内のネストされた FuncDef を再帰的に前処理する。
    /// scope_vars: 親スコープで利用可能な変数名
    /// （params + captures + ローカル代入変数 + ローカル関数名）。
    /// ネストされた FuncDef がスコープ変数を参照する場合はクロージャとして生成し、
    /// pending_local_closures に登録する。さらに深いネストも再帰的に処理する。
    fn preprocess_inner_funcdefs(
        &mut self,
        body: &[Statement],
        scope_vars: &[String],
    ) -> Result<(), LowerError> {
        let scope_set: std::collections::HashSet<&str> =
            scope_vars.iter().map(|s| s.as_str()).collect();

        for stmt in body {
            if let Statement::FuncDef(fd) = stmt {
                let fd_params: std::collections::HashSet<&str> =
                    fd.params.iter().map(|p| p.name.as_str()).collect();
                let free = self.collect_free_vars_in_func_body_unfiltered(&fd.body, &fd_params);
                let captures: Vec<String> = free
                    .into_iter()
                    .filter(|v| scope_set.contains(v.as_str()))
                    .collect();

                if captures.is_empty() {
                    // キャプチャなし: 通常のユーザー関数として登録
                    self.user_funcs.insert(fd.name.clone());
                    self.func_param_defs
                        .insert(fd.name.clone(), fd.params.clone());
                    let ir = self.lower_func_def(fd)?;
                    self.lambda_funcs.push(ir);
                } else {
                    // キャプチャあり: クロージャとして生成
                    let lambda_name = format!("_taida_lambda_{}", self.lambda_counter);
                    self.lambda_counter += 1;

                    self.lambda_vars
                        .insert(fd.name.clone(), lambda_name.clone());
                    self.closure_vars.insert(fd.name.clone());

                    let mut closure_params: Vec<String> = vec!["__env".to_string()];
                    closure_params.extend(fd.params.iter().map(|p| p.name.clone()));
                    let mut lambda_fn =
                        IrFunction::new_with_params(lambda_name.clone(), closure_params);

                    // 環境からキャプチャ変数を復元
                    let env_var = 0u32;
                    for (i, cap_name) in captures.iter().enumerate() {
                        let get_dst = lambda_fn.alloc_var();
                        lambda_fn.push(IrInst::PackGet(get_dst, env_var, i));
                        lambda_fn.push(IrInst::DefVar(cap_name.clone(), get_dst));
                    }

                    // グローバル変数復元
                    let param_names: Vec<String> =
                        fd.params.iter().map(|p| p.name.clone()).collect();
                    let global_refs = self.collect_free_vars_in_body(&fd.body, &param_names);
                    for var_name in &global_refs {
                        if !captures.contains(var_name) {
                            self.globals_referenced.insert(var_name.clone());
                            let hash = self.global_var_hash(var_name);
                            let dst = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::GlobalGet(dst, hash));
                            lambda_fn.push(IrInst::DefVar(var_name.clone(), dst));
                        }
                    }

                    // 再帰的に内部 FuncDef を前処理（深いネスト対応）
                    let inner_scope = self.collect_nested_scope_vars(
                        captures
                            .iter()
                            .cloned()
                            .chain(fd.params.iter().map(|p| p.name.clone())),
                        &fd.body,
                    );
                    self.preprocess_inner_funcdefs(&fd.body, &inner_scope)?;

                    // 関数本体を処理
                    let body_refs: Vec<&Statement> = fd.body.iter().collect();
                    let has_ec = body_refs
                        .iter()
                        .any(|s| matches!(s, Statement::ErrorCeiling(_)));
                    if has_ec {
                        self.lower_statement_sequence(&mut lambda_fn, &body_refs)?;
                    } else {
                        let mut last_var = None;
                        for (j, s) in fd.body.iter().enumerate() {
                            let is_last = j == fd.body.len() - 1;
                            match s {
                                Statement::Expr(expr) => {
                                    let var = self.lower_expr(&mut lambda_fn, expr)?;
                                    if is_last {
                                        last_var = Some(var);
                                    }
                                }
                                _ => {
                                    self.lower_statement(&mut lambda_fn, s)?;
                                }
                            }
                        }

                        if let Some(ret) = last_var {
                            lambda_fn.push(IrInst::Return(ret));
                        } else {
                            let zero = lambda_fn.alloc_var();
                            lambda_fn.push(IrInst::ConstInt(zero, 0));
                            lambda_fn.push(IrInst::Return(zero));
                        }
                    }

                    self.user_funcs.insert(lambda_name.clone());
                    self.lambda_funcs.push(lambda_fn);

                    self.pending_local_closures
                        .insert(fd.name.clone(), (lambda_name, captures));
                }
            }
        }
        Ok(())
    }

    /// ネスト関数が参照可能な親スコープ変数を収集する。
    /// base_vars に加え、同一ボディで束縛されるローカル代入とローカル関数名を含める。
    fn collect_nested_scope_vars<I>(&self, base_vars: I, body: &[Statement]) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut vars = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push_unique = |name: String| {
            if seen.insert(name.clone()) {
                vars.push(name);
            }
        };

        for name in base_vars {
            push_unique(name);
        }

        for stmt in body {
            match stmt {
                Statement::Assignment(assign) => push_unique(assign.target.clone()),
                Statement::FuncDef(fd) => push_unique(fd.name.clone()),
                _ => {}
            }
        }

        vars
    }

    /// 式中の自由変数を収集する
    fn collect_free_vars(
        &self,
        expr: &Expr,
        bound: &std::collections::HashSet<&str>,
    ) -> Vec<String> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_free_vars_inner(expr, bound, &mut free, &mut seen);
        free
    }

    fn collect_free_vars_inner(
        &self,
        expr: &Expr,
        bound: &std::collections::HashSet<&str>,
        free: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, _) => {
                if !bound.contains(name.as_str())
                    && !self.user_funcs.contains(name)
                    && !seen.contains(name)
                {
                    seen.insert(name.clone());
                    free.push(name.clone());
                }
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                self.collect_free_vars_inner(lhs, bound, free, seen);
                self.collect_free_vars_inner(rhs, bound, free, seen);
            }
            Expr::UnaryOp(_, operand, _) => {
                self.collect_free_vars_inner(operand, bound, free, seen);
            }
            Expr::FuncCall(callee, args, _) => {
                self.collect_free_vars_inner(callee, bound, free, seen);
                for arg in args {
                    self.collect_free_vars_inner(arg, bound, free, seen);
                }
            }
            Expr::FieldAccess(obj, _, _) => {
                self.collect_free_vars_inner(obj, bound, free, seen);
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.collect_free_vars_inner(obj, bound, free, seen);
                for arg in args {
                    self.collect_free_vars_inner(arg, bound, free, seen);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.collect_free_vars_inner(e, bound, free, seen);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.collect_free_vars_inner(cond, bound, free, seen);
                    }
                    for stmt in &arm.body {
                        self.collect_free_vars_in_stmt(stmt, bound, free, seen);
                    }
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    self.collect_free_vars_inner(&field.value, bound, free, seen);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.collect_free_vars_inner(item, bound, free, seen);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for arg in args {
                    self.collect_free_vars_inner(arg, bound, free, seen);
                }
                for field in fields {
                    self.collect_free_vars_inner(&field.value, bound, free, seen);
                }
            }
            Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                self.collect_free_vars_inner(inner, bound, free, seen);
            }
            Expr::Lambda(params, body, _) => {
                let mut inner_bound = bound.clone();
                for p in params {
                    inner_bound.insert(p.name.as_str());
                }
                self.collect_free_vars_inner(body, &inner_bound, free, seen);
            }
            _ => {}
        }
    }

    /// Collect free variables from a single statement.
    fn collect_free_vars_in_stmt(
        &self,
        stmt: &Statement,
        bound: &std::collections::HashSet<&str>,
        free: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match stmt {
            Statement::Expr(expr) => {
                self.collect_free_vars_inner(expr, bound, free, seen);
            }
            Statement::Assignment(assign) => {
                self.collect_free_vars_inner(&assign.value, bound, free, seen);
            }
            Statement::UnmoldForward(u) => {
                self.collect_free_vars_inner(&u.source, bound, free, seen);
            }
            Statement::UnmoldBackward(u) => {
                self.collect_free_vars_inner(&u.source, bound, free, seen);
            }
            _ => {}
        }
    }

    /// 関数本体（Statement列）から参照される自由変数を収集する。
    /// パラメータと関数内で定義される変数は除外し、
    /// トップレベル変数または import 値のみ残す。
    fn collect_free_vars_in_body(&self, body: &[Statement], param_names: &[String]) -> Vec<String> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // 関数内で定義される変数名も bound に含める
        let mut bound: std::collections::HashSet<&str> =
            param_names.iter().map(|s| s.as_str()).collect();
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                bound.insert(assign.target.as_str());
            }
        }
        for stmt in body {
            match stmt {
                Statement::Expr(expr) => {
                    self.collect_free_vars_inner(expr, &bound, &mut free, &mut seen);
                }
                Statement::Assignment(assign) => {
                    self.collect_free_vars_inner(&assign.value, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldForward(uf) => {
                    self.collect_free_vars_inner(&uf.source, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldBackward(ub) => {
                    self.collect_free_vars_inner(&ub.source, &bound, &mut free, &mut seen);
                }
                Statement::ErrorCeiling(ec) => {
                    // ErrorCeiling のハンドラ本体からも自由変数を収集
                    for handler_stmt in &ec.handler_body {
                        match handler_stmt {
                            Statement::Expr(e) => {
                                self.collect_free_vars_inner(e, &bound, &mut free, &mut seen);
                            }
                            Statement::Assignment(a) => {
                                self.collect_free_vars_inner(
                                    &a.value, &bound, &mut free, &mut seen,
                                );
                            }
                            Statement::UnmoldForward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source, &bound, &mut free, &mut seen,
                                );
                            }
                            Statement::UnmoldBackward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source, &bound, &mut free, &mut seen,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        // トップレベル変数または import 値のみフィルタ
        free.into_iter()
            .filter(|name| {
                self.top_level_vars.contains(name) || self.imported_value_names.contains(name)
            })
            .collect()
    }

    /// 関数本体の自由変数を収集する（フィルタなし版）。
    /// ローカル関数が親スコープの変数をキャプチャするかどうかの判定に使用。
    fn collect_free_vars_in_func_body_unfiltered(
        &self,
        body: &[Statement],
        param_names: &std::collections::HashSet<&str>,
    ) -> Vec<String> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut bound: std::collections::HashSet<&str> = param_names.clone();
        // 関数内で定義される変数名も bound に含める
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                bound.insert(assign.target.as_str());
            }
            if let Statement::FuncDef(fd) = stmt {
                bound.insert(fd.name.as_str());
            }
        }
        for stmt in body {
            match stmt {
                Statement::Expr(expr) => {
                    self.collect_free_vars_inner(expr, &bound, &mut free, &mut seen);
                }
                Statement::Assignment(assign) => {
                    self.collect_free_vars_inner(&assign.value, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldForward(uf) => {
                    self.collect_free_vars_inner(&uf.source, &bound, &mut free, &mut seen);
                }
                Statement::UnmoldBackward(ub) => {
                    self.collect_free_vars_inner(&ub.source, &bound, &mut free, &mut seen);
                }
                Statement::FuncDef(fd) => {
                    // Recurse into nested function definitions to find transitively
                    // referenced free variables (e.g. f1 → f2 → f3 where f3 uses f1's var).
                    let inner_params: std::collections::HashSet<&str> =
                        fd.params.iter().map(|p| p.name.as_str()).collect();
                    let inner_free =
                        self.collect_free_vars_in_func_body_unfiltered(&fd.body, &inner_params);
                    for var in inner_free {
                        if !bound.contains(var.as_str()) && !seen.contains(&var) {
                            seen.insert(var.clone());
                            free.push(var);
                        }
                    }
                }
                Statement::ErrorCeiling(ec) => {
                    // ErrorCeiling の handler_body を走査
                    // (collect_free_vars_in_body のパターンを踏襲)
                    // error_param はハンドラのバインド変数
                    let mut handler_bound = bound.clone();
                    handler_bound.insert(ec.error_param.as_str());
                    for handler_stmt in &ec.handler_body {
                        match handler_stmt {
                            Statement::Expr(e) => {
                                self.collect_free_vars_inner(
                                    e,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            Statement::Assignment(a) => {
                                self.collect_free_vars_inner(
                                    &a.value,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            Statement::UnmoldForward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            Statement::UnmoldBackward(u) => {
                                self.collect_free_vars_inner(
                                    &u.source,
                                    &handler_bound,
                                    &mut free,
                                    &mut seen,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        free
    }

    /// リストリテラル: `@[1, 2, 3]`
    fn lower_list_lit(
        &mut self,
        func: &mut IrFunction,
        items: &[Expr],
    ) -> Result<IrVar, LowerError> {
        let list_var = func.alloc_var();
        func.push(IrInst::Call(list_var, "taida_list_new".to_string(), vec![]));

        // Set elem_type_tag based on first element's type (checker guarantees homogeneity)
        if let Some(first) = items.first() {
            let tag = self.expr_type_tag(first);
            let tag_var = func.alloc_var();
            func.push(IrInst::ConstInt(tag_var, tag));
            let dummy = func.alloc_var();
            func.push(IrInst::Call(
                dummy,
                "taida_list_set_elem_tag".to_string(),
                vec![list_var, tag_var],
            ));
        }

        // taida_list_push は realloc で新ポインタを返す可能性がある
        // 最新のポインタを追跡する
        let mut current_list = list_var;
        for item in items {
            let item_var = self.lower_expr(func, item)?;
            // retain-on-store: Pack/List/Closure 要素を格納する際に retain。
            // taida_release の List 再帰 release と対になり、double-free を防ぐ。
            // Pack フィールド格納時の retain-on-store (A-4c) と同じパターン。
            let tag = self.expr_type_tag(item);
            self.emit_retain_if_heap_tag(func, item_var, tag);
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_list_push".to_string(),
                vec![current_list, item_var],
            ));
            current_list = result;
        }

        Ok(current_list)
    }

    /// テンプレート文字列: `"Hello, ${name}!"` → 部分文字列を連結
    fn lower_template_lit(
        &mut self,
        func: &mut IrFunction,
        template: &str,
    ) -> Result<IrVar, LowerError> {
        // Parse template: split on ${ and } to get literal parts and expression parts.
        // Interpolation expressions are parsed using the full Taida parser and lowered
        // as real AST expressions, so field access, function calls, method calls etc.
        // are all supported (matching the interpreter behaviour).
        let mut result_var = {
            let var = func.alloc_var();
            func.push(IrInst::ConstStr(var, String::new()));
            var
        };

        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                // Skip '$' and '{'
                i += 2;
                // Find matching }
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
                let expr_str_trimmed = expr_str.trim();

                // Parse the interpolation expression using the full Taida parser.
                let (program, errors) = crate::parser::parse(expr_str_trimmed);
                let str_var = if errors.is_empty()
                    && !program.statements.is_empty()
                    && let crate::parser::Statement::Expr(ref parsed_expr) = program.statements[0]
                {
                    // Lower the parsed expression and convert to string
                    let expr_var = self.lower_expr(func, parsed_expr)?;
                    self.convert_to_string(func, parsed_expr, expr_var)?
                } else {
                    // Fallback: treat as simple variable name (backward compat)
                    let var_name = expr_str_trimmed.to_string();
                    let name_var = func.alloc_var();
                    func.push(IrInst::UseVar(name_var, var_name.clone()));
                    let v = func.alloc_var();
                    func.push(IrInst::Call(
                        v,
                        "taida_polymorphic_to_string".to_string(),
                        vec![name_var],
                    ));
                    v
                };
                let concat_var = func.alloc_var();
                func.push(IrInst::Call(
                    concat_var,
                    "taida_str_concat".to_string(),
                    vec![result_var, str_var],
                ));
                result_var = concat_var;
                // skip closing '}'
                if i < chars.len() {
                    i += 1;
                }
            } else {
                // Collect literal characters until next ${ or end
                let start = i;
                while i < chars.len() {
                    if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                        break;
                    }
                    i += 1;
                }
                let literal: String = chars[start..i].iter().collect();
                let lit_var = func.alloc_var();
                func.push(IrInst::ConstStr(lit_var, literal));
                let concat_var = func.alloc_var();
                func.push(IrInst::Call(
                    concat_var,
                    "taida_str_concat".to_string(),
                    vec![result_var, lit_var],
                ));
                result_var = concat_var;
            }
        }

        Ok(result_var)
    }

    // lower_index_access removed in v0.5.0 — IndexAccess no longer exists in AST

    /// ヒープオブジェクトを生成する式かどうかを判定
    /// Lambda は除外: キャプチャありのクロージャのみヒープ（closure_vars で判定）
    /// 式が float 値を返すかどうかを判定
    pub(crate) fn expr_returns_float(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FloatLit(_, _) => true,
            Expr::FuncCall(callee, _, _) => {
                // Detect float-returning user-defined functions
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.float_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                // 一方が float なら結果も float
                self.expr_returns_float(lhs) || self.expr_returns_float(rhs)
            }
            Expr::UnaryOp(_, inner, _) => {
                // -2.3 etc: negate preserves float type
                self.expr_returns_float(inner)
            }
            Expr::Ident(name, _) => {
                // float 変数への参照
                self.float_vars.contains(name) || self.stdlib_constants.contains_key(name)
            }
            _ => false,
        }
    }

    /// 式が文字列を返すかどうかを判定（静的に推測可能な場合のみ、変数名の追跡あり）
    pub(crate) fn expr_is_string_full(&self, expr: &Expr) -> bool {
        match expr {
            Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => true,
            Expr::Ident(name, _) => self.string_vars.contains(name),
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "toString"
                        | "toStr"
                        | "toUpperCase"
                        | "toLowerCase"
                        | "trim"
                        | "replace"
                        | "slice"
                        | "charAt"
                        | "repeat"
                        | "join"
                )
            }
            Expr::FuncCall(callee, _, _) => {
                // Detect string-returning prelude functions and user-defined functions
                if let Expr::Ident(name, _) = callee.as_ref() {
                    matches!(name.as_str(), "stdin" | "jsonEncode" | "jsonPretty")
                        || self.string_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            Expr::BinaryOp(lhs, BinOp::Add, rhs, _) => {
                self.expr_is_string_full(lhs) || self.expr_is_string_full(rhs)
            }
            // WF-2b: MoldInst string molds (Upper, Lower, etc.) return strings
            // Note: CharAt returns Lax[Str], not raw Str (TF-15)
            // Note: Reverse is polymorphic (Str or List), so NOT included here
            Expr::MoldInst(name, _, _, _) => matches!(
                name.as_str(),
                "Str"
                    | "Upper"
                    | "Lower"
                    | "Trim"
                    | "Replace"
                    | "Slice"
                    | "Repeat"
                    | "Pad"
                    | "Join"
                    | "ToFixed"
            ),
            Expr::BinaryOp(_, BinOp::Concat, _, _) => true,
            Expr::CondBranch(arms, _) => {
                // If ANY arm body's last expression is a string, the whole branch is string
                arms.iter().any(|arm| {
                    arm.body
                        .last()
                        .map(|stmt| match stmt {
                            Statement::Expr(e) => self.expr_is_string_full(e),
                            _ => false,
                        })
                        .unwrap_or(false)
                })
            }
            _ => false,
        }
    }

    /// FL-16: 式の型がコンパイル時に不明かどうかを判定（untyped パラメータ等）
    fn expr_type_is_unknown(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => {
                !self.int_vars.contains(name)
                    && !self.string_vars.contains(name)
                    && !self.float_vars.contains(name)
                    && !self.bool_vars.contains(name)
                    && !self.pack_vars.contains(name)
                    && !self.list_vars.contains(name)
                    && !self.closure_vars.contains(name)
                    && !self.top_level_vars.contains(name)
                    && !self.user_funcs.contains(name)
                    && !self.stdlib_constants.contains_key(name)
            }
            _ => false,
        }
    }

    /// 式が bool 値を返すかどうかを判定
    pub(crate) fn expr_is_bool(&self, expr: &Expr) -> bool {
        match expr {
            Expr::BoolLit(_, _) => true,
            Expr::Ident(name, _) => self.bool_vars.contains(name),
            Expr::BinaryOp(_, op, _, _) => {
                matches!(
                    op,
                    BinOp::Eq
                        | BinOp::NotEq
                        | BinOp::Lt
                        | BinOp::Gt
                        | BinOp::GtEq
                        | BinOp::And
                        | BinOp::Or
                )
            }
            Expr::UnaryOp(UnaryOp::Not, _, _) => true,
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "hasValue"
                        | "isEmpty"
                        | "contains"
                        | "has"
                        | "startsWith"
                        | "endsWith"
                        | "any"
                        | "all"
                        | "none"
                        | "isOk"
                        | "isError"
                        | "isSuccess"
                        | "isFulfilled"
                        | "isPending"
                        | "isRejected"
                        | "isNaN"
                        | "isInfinite"
                        | "isFinite"
                        | "isPositive"
                        | "isNegative"
                        | "isZero"
                )
            }
            Expr::FuncCall(callee, _, _) => {
                // Detect bool-returning user-defined functions
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.bool_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            // WFX-3: Exists[path]() returns Bool
            Expr::MoldInst(name, _, _, _) if name == "Exists" => true,
            Expr::FieldAccess(obj, field, _) => {
                // QF-34: hasValue フィールドは Lax/Result の Bool フィールド
                if field == "hasValue" {
                    return true;
                }
                // QF-10: フィールドの型を、アクセス元の TypeDef 定義から判定する。
                // グローバルな field_type_tags は同名フィールドが異なる型で使われると衝突するため、
                // TypeDef の型注釈を直接参照する。
                if let Some(type_name) = self.infer_type_name(obj)
                    && let Some(field_types) = self.type_field_types.get(&type_name)
                {
                    return field_types.iter().any(|(name, ty)| {
                        name == field
                            && matches!(ty, Some(crate::parser::TypeExpr::Named(n)) if n == "Bool")
                    });
                }
                // TypeDef 不明の場合はグローバル field_type_tags にフォールバック
                self.field_type_tags.get(field).copied() == Some(4)
            }
            _ => false,
        }
    }

    /// A-4c: TypeDef のフィールド型注釈から型タグを決定する
    fn type_field_type_tag(&self, type_name: &str, field_name: &str) -> i64 {
        if let Some(field_types) = self.type_field_types.get(type_name) {
            for (name, ty) in field_types {
                if name == field_name
                    && let Some(ty_expr) = ty
                {
                    return self.type_expr_to_tag(ty_expr);
                }
            }
        }
        // Fallback to global field_type_tags
        self.field_type_tags.get(field_name).copied().unwrap_or(0)
    }

    /// TypeExpr から型タグへの変換
    fn type_expr_to_tag(&self, ty: &crate::parser::TypeExpr) -> i64 {
        match ty {
            crate::parser::TypeExpr::Named(n) => match n.as_str() {
                "Int" => 0,
                "Float" => 1,
                "Bool" => 2,
                "Str" => 3,
                _ => 4, // user-defined types are Packs
            },
            crate::parser::TypeExpr::List(_) => 5,
            crate::parser::TypeExpr::BuchiPack(_) => 4,
            crate::parser::TypeExpr::Function(_, _) => 6,
            crate::parser::TypeExpr::Generic(name, _) => match name.as_str() {
                "Lax" | "Gorillax" | "RelaxedGorillax" | "Result" | "Async" => 4,
                "HashMap" => 4,
                "Set" => 4,
                _ => 0,
            },
        }
    }

    /// A-4c: 式から Pack フィールド値の型タグを推論する
    /// Returns: 0=Int, 1=Float, 2=Bool, 3=Str, 4=Pack, 5=List, 6=Closure, -1=Unknown
    pub(crate) fn expr_type_tag(&self, expr: &Expr) -> i64 {
        match expr {
            Expr::IntLit(_, _) => 0,                              // TAIDA_TAG_INT
            Expr::FloatLit(_, _) => 1,                            // TAIDA_TAG_FLOAT
            Expr::BoolLit(_, _) => 2,                             // TAIDA_TAG_BOOL
            Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => 3, // TAIDA_TAG_STR
            Expr::BuchiPack(_, _) | Expr::TypeInst(_, _, _) => 4, // TAIDA_TAG_PACK
            Expr::ListLit(_, _) => 5,                             // TAIDA_TAG_LIST
            Expr::Lambda(_, _, _) => 6,                           // TAIDA_TAG_CLOSURE
            Expr::Ident(name, _) => {
                if self.bool_vars.contains(name) {
                    2
                } else if self.float_vars.contains(name) {
                    1
                } else if self.string_vars.contains(name) {
                    3
                } else if self.pack_vars.contains(name) {
                    4
                } else if self.list_vars.contains(name) {
                    5
                } else if self.closure_vars.contains(name) {
                    6
                } else {
                    -1 // TAIDA_TAG_UNKNOWN: type cannot be determined at compile time
                }
            }
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    // Known Int-returning: length, indexOf, etc. already match MethodCall above
                    if self.bool_returning_funcs.contains(name.as_str()) {
                        return 2;
                    }
                    if self.float_returning_funcs.contains(name.as_str()) {
                        return 1;
                    }
                    if self.string_returning_funcs.contains(name.as_str()) {
                        return 3;
                    }
                    if self.pack_returning_funcs.contains(name.as_str()) {
                        return 4;
                    }
                    if self.list_returning_funcs.contains(name.as_str()) {
                        return 5;
                    }
                    // Builtin range() returns a List
                    if name == "range" {
                        return 5;
                    }
                }
                -1 // TAIDA_TAG_UNKNOWN: return type cannot be determined at compile time
            }
            Expr::MethodCall(_, method, _, _) => {
                if self.expr_is_bool(expr) {
                    return 2;
                }
                match method.as_str() {
                    "toString" | "toUpperCase" | "toLowerCase" => 3,
                    "length" | "indexOf" | "lastIndexOf" => 0, // known Int-returning methods
                    "map" | "filter" | "flatMap" | "sort" | "unique" | "flatten" | "reverse"
                    | "concat" | "append" | "prepend" | "zip" | "enumerate" => 5,
                    _ => -1, // TAIDA_TAG_UNKNOWN
                }
            }
            Expr::MoldInst(_, _, _, _) => 4, // Mold instantiation returns a Pack
            Expr::Unmold(_, _) => -1,        // TAIDA_TAG_UNKNOWN: could be anything
            _ if self.expr_is_bool(expr) => 2,
            _ => -1, // TAIDA_TAG_UNKNOWN
        }
    }

    /// NB-14: Get the runtime param tag IrVar for an expression, if it's a function
    /// parameter with a caller-propagated type tag.
    /// Returns Some(tag_var) if the expression is an Ident whose name is in param_tag_vars.
    fn get_param_tag_var(&self, expr: &Expr) -> Option<IrVar> {
        if let Expr::Ident(name, _) = expr {
            self.param_tag_vars.get(name).copied()
        } else {
            None
        }
    }

    /// NB-14: Check whether any argument requires call-site tag propagation.
    /// Returns true if at least one arg has a non-INT compile-time tag, a transitive
    /// param_tag_var, or is a FuncCall to a user function (which may carry a return tag).
    fn needs_call_arg_tags(&self, args: &[Expr]) -> bool {
        for (i, arg) in args.iter().enumerate() {
            if i >= Self::TAG_FRAME_SIZE {
                break;
            }
            let tag = self.expr_type_tag(arg);
            if tag > 0 {
                return true;
            } else if tag == -1 {
                if self.get_param_tag_var(arg).is_some() {
                    return true;
                }
                // FuncCall to user function may carry a return type tag
                if let Expr::FuncCall(callee_box, _, _) = arg
                    && let Expr::Ident(callee_name, _) = callee_box.as_ref()
                    && self.user_funcs.contains(callee_name)
                {
                    return true;
                }
            }
        }
        false
    }

    /// NB-14: Emit taida_set_call_arg_tag() for each argument with a known non-default
    /// type tag before a CallUser. This propagates Bool/Float/Str/etc. type info from
    /// the caller to the callee so that pack field tags can be set correctly.
    /// Note: TAG_FRAME_SIZE (256) is the maximum number of tagged arguments per call.
    /// Arguments beyond this limit are skipped (tag defaults to INT/0 in the callee).
    /// 256 exceeds any practical function arity in Taida.
    const TAG_FRAME_SIZE: usize = 256;

    fn emit_call_arg_tags(&mut self, func: &mut IrFunction, args: &[Expr]) {
        for (i, arg) in args.iter().enumerate() {
            if i >= Self::TAG_FRAME_SIZE {
                break;
            }
            let tag = self.expr_type_tag(arg);
            // Only emit for non-INT tags (INT=0 is the default and doesn't need propagation)
            // Also skip UNKNOWN (-1) since that would overwrite any existing tag
            if tag > 0 {
                let idx_var = func.alloc_var();
                func.push(IrInst::ConstInt(idx_var, i as i64));
                let tag_var = func.alloc_var();
                func.push(IrInst::ConstInt(tag_var, tag));
                let dummy = func.alloc_var();
                func.push(IrInst::Call(
                    dummy,
                    "taida_set_call_arg_tag".to_string(),
                    vec![idx_var, tag_var],
                ));
            } else if tag == -1 {
                // UNKNOWN: if we have a param_tag_var for this, propagate it transitively
                if let Some(existing_tag_var) = self.get_param_tag_var(arg) {
                    let idx_var = func.alloc_var();
                    func.push(IrInst::ConstInt(idx_var, i as i64));
                    let dummy = func.alloc_var();
                    func.push(IrInst::Call(
                        dummy,
                        "taida_set_call_arg_tag".to_string(),
                        vec![idx_var, existing_tag_var],
                    ));
                }
            }
        }
    }

    /// NB-14: Emit taida_set_call_arg_tag() for arguments whose type was determined
    /// AFTER lowering (via return type tag from a nested CallUser). This complements
    /// emit_call_arg_tags which handles compile-time known types before lowering.
    fn emit_post_lower_arg_tags(
        &self,
        func: &mut IrFunction,
        args: &[Expr],
        explicit_arg_vars: &[IrVar],
    ) {
        for (i, arg_var) in explicit_arg_vars.iter().enumerate() {
            if i >= Self::TAG_FRAME_SIZE || i >= args.len() {
                break;
            }
            // Skip args already handled by emit_call_arg_tags (known type or param_tag_var)
            let tag = self.expr_type_tag(&args[i]);
            if tag > 0 {
                continue; // Already set in pre-lower pass
            }
            if tag == -1 && self.get_param_tag_var(&args[i]).is_some() {
                continue; // Already set in pre-lower pass
            }
            // Check if this arg's IrVar has a return_tag_var (from a nested CallUser)
            if let Some(&rtv) = self.return_tag_vars.get(arg_var) {
                let idx_var = func.alloc_var();
                func.push(IrInst::ConstInt(idx_var, i as i64));
                let dummy = func.alloc_var();
                func.push(IrInst::Call(
                    dummy,
                    "taida_set_call_arg_tag".to_string(),
                    vec![idx_var, rtv],
                ));
            }
        }
    }

    /// NB-31: Determine compile-time callable type tag for httpServe handler.
    /// Returns:
    ///   6  (TAIDA_TAG_CLOSURE) — lambda or closure variable
    ///  10  (TAIDA_TAG_FUNC)    — named function reference (user_funcs / lambda_vars)
    ///  -1  (TAIDA_TAG_UNKNOWN) — dynamic / cannot determine at compile time
    ///   other (0..5, etc.)     — statically known non-callable type
    ///
    /// Strategy: check callable first, then delegate to noncallable_type_tag()
    /// which uses the existing expr_returns_float / expr_is_string_full / expr_is_bool /
    /// expr_is_pack / expr_is_list helpers + arithmetic Int detection.
    /// NET3-5a: Determine compile-time handler parameter count for httpServe.
    /// Returns the number of parameters if statically known, -1 if dynamic.
    fn handler_arity(&self, expr: &Expr) -> i64 {
        match expr {
            Expr::Lambda(params, _, _) => params.len() as i64,
            Expr::Ident(name, _) => self.resolve_ident_arity(name),
            _ => -1,
        }
    }

    /// NB3-4: Resolve handler arity for a named identifier by following
    /// func_param_defs, lambda_vars, lambda_param_counts, and var_aliases chains.
    /// Max chain depth of 16 to prevent infinite loops from cyclic aliases.
    fn resolve_ident_arity(&self, name: &str) -> i64 {
        let mut current = name;
        let mut depth = 0;
        loop {
            if depth > 16 {
                return -1; // too deep, give up
            }
            // 1. Named function definition (top-level or inner FuncDef)
            if let Some(params) = self.func_param_defs.get(current) {
                return params.len() as i64;
            }
            // 2. Lambda variable mapped to a lambda function name
            if let Some(lambda_name) = self.lambda_vars.get(current)
                && let Some(params) = self.func_param_defs.get(lambda_name.as_str())
            {
                return params.len() as i64;
            }
            // 3. Direct lambda param count (from lambda assignment: `h <= req, writer => @(...)`)
            if let Some(&count) = self.lambda_param_counts.get(current) {
                return count as i64;
            }
            // 4. Variable alias — follow the chain (e.g., `h <= handler`)
            if let Some(source) = self.var_aliases.get(current) {
                current = source.as_str();
                depth += 1;
                continue;
            }
            return -1; // dynamic / unknown
        }
    }

    fn callable_type_tag(&self, expr: &Expr) -> i64 {
        // 1. Callable detection
        match expr {
            Expr::Lambda(_, _, _) => return 6, // TAIDA_TAG_CLOSURE
            Expr::Ident(name, _) => {
                if let Some(tag) = self.resolve_ident_callable_tag(name) {
                    return tag;
                }
                // NB3-4 fix: If this identifier (or any alias-chain ancestor)
                // was inferred solely from the function's return-type annotation
                // (e.g. `run_server h ... => :Int` puts `h` into int_vars),
                // do NOT trust noncallable_type_tag. The parameter might actually
                // be a function/closure passed at runtime.
                // Follow var_aliases to cover `x <= h` 1-hop (and N-hop) aliases.
                if self.ident_or_alias_is_return_type_inferred(name) {
                    return -1;
                }
            }
            _ => {}
        }
        // 2. Non-callable detection — use rich expression-type helpers
        if let Some(tag) = self.noncallable_type_tag(expr) {
            return tag;
        }
        -1 // TAIDA_TAG_UNKNOWN
    }

    /// NB3-4: Resolve callable type tag for a named identifier, following var_aliases.
    fn resolve_ident_callable_tag(&self, name: &str) -> Option<i64> {
        let mut current = name;
        let mut depth = 0;
        loop {
            if depth > 16 {
                return None;
            }
            if self.closure_vars.contains(current) {
                return Some(6); // TAIDA_TAG_CLOSURE
            }
            if self.user_funcs.contains(current) || self.lambda_vars.contains_key(current) {
                return Some(10); // TAIDA_TAG_FUNC
            }
            // NB3-4: Follow variable alias chain
            if let Some(source) = self.var_aliases.get(current) {
                current = source.as_str();
                depth += 1;
                continue;
            }
            return None;
        }
    }

    /// NB3-4: Check if an identifier, or any ancestor in its var_aliases chain,
    /// belongs to return_type_inferred_params. This ensures that `x <= h` where
    /// `h` is a return-type-inferred parameter is also treated as unknown callable.
    fn ident_or_alias_is_return_type_inferred(&self, name: &str) -> bool {
        let mut current = name;
        let mut depth = 0;
        loop {
            if depth > 16 {
                return false;
            }
            if self.return_type_inferred_params.contains(current) {
                return true;
            }
            if let Some(source) = self.var_aliases.get(current) {
                current = source.as_str();
                depth += 1;
                continue;
            }
            return false;
        }
    }

    /// NB-31: Determine if an expression is a known non-callable type.
    /// Returns Some(tag) for statically known non-callable, None for unknown.
    /// Leverages existing expr_returns_float / expr_is_string_full / expr_is_bool /
    /// expr_is_pack / expr_is_list which already handle literals, variables, BinaryOp,
    /// MethodCall, FuncCall, etc.
    fn noncallable_type_tag(&self, expr: &Expr) -> Option<i64> {
        // Bool (2) — handles BoolLit, bool_vars, comparison ops, boolean methods
        if self.expr_is_bool(expr) {
            return Some(2);
        }
        // Float (1) — handles FloatLit, float_vars, float BinaryOp, float-returning funcs
        if self.expr_returns_float(expr) {
            return Some(1);
        }
        // String (3) — handles StringLit, TemplateLit, string_vars, string methods/funcs
        if self.expr_is_string_full(expr) {
            return Some(3);
        }
        // Pack (4) — handles BuchiPack, TypeInst, pack_vars
        if self.expr_is_pack(expr) {
            return Some(4);
        }
        // List (5) — handles ListLit, list_vars, list methods/funcs
        if self.expr_is_list(expr) {
            return Some(5);
        }
        // Int (0) — literals, int_vars, arithmetic ops, int-returning methods/funcs
        if self.expr_is_int(expr) {
            return Some(0);
        }
        // MoldInst always returns a Pack-like value
        if matches!(expr, Expr::MoldInst(_, _, _, _)) {
            return Some(4);
        }
        None
    }

    /// retain-on-store: Pack/List/Closure/Str をフィールドに格納する際に retain する。
    /// taida_release の再帰 release と対になり、double-free を防ぐ。
    /// tag が TAIDA_TAG_STR(3), TAIDA_TAG_PACK(4), TAIDA_TAG_LIST(5), TAIDA_TAG_CLOSURE(6) の場合に retain。
    fn emit_retain_if_heap_tag(&self, func: &mut IrFunction, val: IrVar, tag: i64) {
        if tag == 4 || tag == 5 || tag == 6 {
            func.push(IrInst::Retain(val));
        } else if tag == 3 {
            // TAIDA_TAG_STR: hidden-header string は taida_str_retain で retain する。
            // taida_retain は Pack/List/Closure 用なので Str には使えない。
            let dummy = func.alloc_var();
            func.push(IrInst::Call(
                dummy,
                "taida_str_retain".to_string(),
                vec![val],
            ));
        }
    }

    /// QF-10: 式の TypeDef 名を推論する（FieldAccess の型解決用）
    fn infer_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(name, _) => self.var_type_names.get(name).cloned(),
            Expr::TypeInst(type_name, _, _) => Some(type_name.clone()),
            _ => None,
        }
    }

    /// F-58: 式が BuchiPack/TypeInst を返すかどうかを判定
    /// BuchiPack フィールドの関数呼び出しが組み込みメソッド名と衝突するのを防ぐため
    pub(crate) fn expr_is_pack(&self, expr: &Expr) -> bool {
        match expr {
            Expr::BuchiPack(_, _) => true,
            Expr::TypeInst(_, _, _) => true,
            Expr::Ident(name, _) => self.pack_vars.contains(name),
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.pack_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            Expr::MethodCall(obj, method, _, _) => {
                // HashMap.set() returns HashMap, not pack — but if the receiver
                // is a pack, method calls that return the same type are still packs
                self.expr_is_pack(obj) && method != "toString" && method != "toStr"
            }
            _ => false,
        }
    }

    /// retain-on-store: 式が List を返すかどうかを判定
    fn expr_is_list(&self, expr: &Expr) -> bool {
        match expr {
            Expr::ListLit(_, _) => true,
            Expr::Ident(name, _) => self.list_vars.contains(name),
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.list_returning_funcs.contains(name.as_str()) || name == "range"
                } else {
                    false
                }
            }
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "map"
                        | "filter"
                        | "flatMap"
                        | "sort"
                        | "unique"
                        | "flatten"
                        | "reverse"
                        | "concat"
                        | "append"
                        | "prepend"
                        | "zip"
                        | "enumerate"
                )
            }
            _ => false,
        }
    }

    /// NB-31: 式が Int を返すかどうかを判定（noncallable_type_tag 用）
    /// arithmetic 演算、Int-returning メソッド/関数、int_vars を網羅する。
    fn expr_is_int(&self, expr: &Expr) -> bool {
        match expr {
            Expr::IntLit(_, _) => true,
            Expr::UnaryOp(UnaryOp::Neg, inner, _) => self.expr_is_int(inner),
            Expr::Ident(name, _) => self.int_vars.contains(name),
            // Arithmetic ops: Int if neither side is Float
            Expr::BinaryOp(lhs, op, rhs, _) => {
                matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul)
                    && !self.expr_returns_float(lhs)
                    && !self.expr_returns_float(rhs)
            }
            // Methods that always return Int
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "length" | "indexOf" | "lastIndexOf" | "count"
                )
            }
            // Functions with :Int/:Num return type
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.int_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Unmold 先の変数に型情報を伝播する
    /// MoldInst("Str", ...) ]=> x の場合、x を string_vars に追加
    fn track_unmold_type(&mut self, target: &str, source: &Expr) {
        match source {
            Expr::MoldInst(name, _, _, _) => self.track_unmold_type_by_mold_name(target, name),
            // QF-34: Ident source — look up lax_inner_types to propagate type through unmold
            // e.g., `x <= Bool["maybe"]()` then `x ]=> val` → val is Bool
            Expr::Ident(name, _) => {
                if let Some(inner_type) = self.lax_inner_types.get(name).cloned() {
                    self.track_unmold_type_by_mold_name(target, &inner_type);
                }
            }
            // MethodCall results: hasValue() -> bool
            Expr::MethodCall(_, method, _, _) => {
                if matches!(
                    method.as_str(),
                    "hasValue"
                        | "isEmpty"
                        | "contains"
                        | "startsWith"
                        | "endsWith"
                        | "any"
                        | "all"
                        | "none"
                        | "isOk"
                        | "isError"
                        | "isSuccess"
                        | "isFulfilled"
                        | "isPending"
                        | "isRejected"
                        | "isNaN"
                        | "isInfinite"
                        | "isFinite"
                        | "isPositive"
                        | "isNegative"
                        | "isZero"
                ) {
                    self.bool_vars.insert(target.to_string());
                }
            }
            _ => {}
        }
    }

    /// Helper: track unmold result type based on mold name
    fn track_unmold_type_by_mold_name(&mut self, target: &str, mold_name: &str) {
        match mold_name {
            // Note: Reverse is polymorphic (Str or List), so NOT included here
            "Str" | "Upper" | "Lower" | "Trim" | "Replace" | "Slice" | "CharAt" | "Repeat"
            | "Pad" | "Join" | "ToFixed" => {
                self.string_vars.insert(target.to_string());
            }
            "Bool" => {
                self.bool_vars.insert(target.to_string());
            }
            "Float" => {
                self.float_vars.insert(target.to_string());
            }
            _ => {}
        }
    }

    /// 式の結果を文字列に変換する。既に文字列なら何もしない。
    /// stdout/stderr の引数の自動変換に使用。
    fn convert_to_string(
        &self,
        func: &mut IrFunction,
        expr: &Expr,
        var: IrVar,
    ) -> Result<IrVar, LowerError> {
        if self.expr_is_string_full(expr) {
            // Already a string — no conversion needed
            Ok(var)
        } else if self.expr_is_bool(expr) {
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_str_from_bool".to_string(),
                vec![var],
            ));
            Ok(result)
        } else if self.expr_returns_float(expr) {
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_float_to_str".to_string(),
                vec![var],
            ));
            Ok(result)
        } else {
            // Default: polymorphic to_string (handles int, monadic types, etc.)
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_polymorphic_to_string".to_string(),
                vec![var],
            ));
            Ok(result)
        }
    }

    /// F-58/F-60: Check if a function body's last expression returns a BuchiPack/TypeInst.
    fn func_body_returns_pack(body: &[Statement]) -> bool {
        matches!(
            body.last(),
            Some(Statement::Expr(
                Expr::BuchiPack(_, _) | Expr::TypeInst(_, _, _)
            ))
        )
    }

    /// retain-on-store: Check if a function body's last expression returns a List.
    fn func_body_returns_list(body: &[Statement]) -> bool {
        matches!(body.last(), Some(Statement::Expr(Expr::ListLit(_, _))))
    }

    fn is_heap_expr(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::BuchiPack(..) | Expr::TypeInst(..) | Expr::ListLit(..)
        ) || matches!(expr, Expr::MethodCall(_, method, _, _)
            if method == "map" || method == "filter" || method == "reverse"
        )
    }

    /// F-48: 式中に出現する全ての識別子名を収集する（ラムダ本体には入らない）
    fn collect_idents_in_expr(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        match expr {
            Expr::Ident(name, _) => {
                out.insert(name.clone());
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                Self::collect_idents_in_expr(lhs, out);
                Self::collect_idents_in_expr(rhs, out);
            }
            Expr::UnaryOp(_, operand, _) => {
                Self::collect_idents_in_expr(operand, out);
            }
            Expr::FuncCall(callee, args, _) => {
                Self::collect_idents_in_expr(callee, out);
                for arg in args {
                    Self::collect_idents_in_expr(arg, out);
                }
            }
            Expr::FieldAccess(obj, _, _) => {
                Self::collect_idents_in_expr(obj, out);
            }
            Expr::MethodCall(obj, _, args, _) => {
                Self::collect_idents_in_expr(obj, out);
                for arg in args {
                    Self::collect_idents_in_expr(arg, out);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    Self::collect_idents_in_expr(e, out);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        Self::collect_idents_in_expr(cond, out);
                    }
                    for stmt in &arm.body {
                        if let Statement::Expr(e) = stmt {
                            Self::collect_idents_in_expr(e, out);
                        } else if let Statement::Assignment(a) = stmt {
                            Self::collect_idents_in_expr(&a.value, out);
                        }
                    }
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    Self::collect_idents_in_expr(&field.value, out);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    Self::collect_idents_in_expr(item, out);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for arg in args {
                    Self::collect_idents_in_expr(arg, out);
                }
                for field in fields {
                    Self::collect_idents_in_expr(&field.value, out);
                }
            }
            Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                Self::collect_idents_in_expr(inner, out);
            }
            // ラムダ本体には入らない（キャプチャは別途管理）
            _ => {}
        }
    }

    /// F-48: 関数本体の代入文から、戻り値式が間接的に参照する全変数の集合を計算する。
    /// 代入グラフの推移的閉包を求め、戻り値から到達可能な全変数名を返す。
    fn compute_reachable_vars(
        return_expr: &Expr,
        body: &[Statement],
    ) -> std::collections::HashSet<String> {
        // 1. 代入グラフを構築: target -> {式中の識別子}
        let mut assign_deps: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                let mut deps = std::collections::HashSet::new();
                Self::collect_idents_in_expr(&assign.value, &mut deps);
                assign_deps.insert(assign.target.clone(), deps);
            }
        }

        // 2. 戻り値式の直接参照を収集
        let mut reachable = std::collections::HashSet::new();
        Self::collect_idents_in_expr(return_expr, &mut reachable);

        // 3. 推移的閉包（BFS）
        let mut queue: Vec<String> = reachable.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            if let Some(deps) = assign_deps.get(&name) {
                for dep in deps {
                    if reachable.insert(dep.clone()) {
                        queue.push(dep.clone());
                    }
                }
            }
        }

        reachable
    }

    /// 条件分岐: `| cond |> value` パターン
    fn lower_cond_branch(
        &mut self,
        func: &mut IrFunction,
        arms: &[crate::parser::CondArm],
    ) -> Result<IrVar, LowerError> {
        use super::ir::CondArm as IrCondArm;

        let result_var = func.alloc_var();
        let mut ir_arms = Vec::new();

        for arm in arms {
            let condition = match &arm.condition {
                Some(cond_expr) => {
                    let cond_var = self.lower_expr(func, cond_expr)?;
                    Some(cond_var)
                }
                None => None, // デフォルトケース
            };

            // 本体を一時的な命令列に lowering（複数ステートメント対応）
            let (body_insts, body_var) = {
                let saved = std::mem::take(&mut func.body);
                let body_result = self.lower_cond_arm_body(func, &arm.body)?;
                let insts = std::mem::replace(&mut func.body, saved);
                (insts, body_result)
            };

            ir_arms.push(IrCondArm {
                condition,
                body: body_insts,
                result: body_var,
            });
        }

        func.push(IrInst::CondBranch(result_var, ir_arms));
        Ok(result_var)
    }

    /// Lower a condition arm body (Vec<Statement>) to IR.
    /// Returns the IR variable holding the result of the last expression.
    fn lower_cond_arm_body(
        &mut self,
        func: &mut IrFunction,
        body: &[Statement],
    ) -> Result<IrVar, LowerError> {
        // Fallback: allocate a default result (int 0) in case body has no expression
        let mut last_var = func.alloc_var();
        func.push(IrInst::ConstInt(last_var, 0));
        for (i, stmt) in body.iter().enumerate() {
            let is_last = i == body.len() - 1;
            match stmt {
                Statement::Expr(expr) => {
                    let var = self.lower_expr(func, expr)?;
                    if is_last {
                        last_var = var;
                    }
                }
                _ => {
                    self.lower_statement(func, stmt)?;
                }
            }
        }
        Ok(last_var)
    }

    /// Lower a condition arm body in tail position.
    /// The last expression is lowered with tail-call optimization.
    fn lower_cond_arm_body_tail(
        &mut self,
        func: &mut IrFunction,
        body: &[Statement],
    ) -> Result<IrVar, LowerError> {
        // Fallback: allocate a default result (int 0) in case body has no expression
        let mut last_var = func.alloc_var();
        func.push(IrInst::ConstInt(last_var, 0));
        for (i, stmt) in body.iter().enumerate() {
            let is_last = i == body.len() - 1;
            match stmt {
                Statement::Expr(expr) => {
                    let var = if is_last {
                        self.lower_expr_tail(func, expr)?
                    } else {
                        self.lower_expr(func, expr)?
                    };
                    if is_last {
                        last_var = var;
                    }
                }
                _ => {
                    self.lower_statement(func, stmt)?;
                }
            }
        }
        Ok(last_var)
    }

    fn emit_imported_module_inits(&mut self, func: &mut IrFunction) {
        for init_symbol in std::mem::take(&mut self.module_inits_needed) {
            let dummy = func.alloc_var();
            func.push(IrInst::CallUser(dummy, init_symbol, vec![]));
        }
    }

    fn bind_imported_values(&mut self, func: &mut IrFunction) {
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
    fn generate_module_init_func(
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
