// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups core methods of the `Lowering` struct (placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{Lowering, simple_hash};

impl Lowering {
    pub fn new() -> Self {
        let mut stdlib_runtime_funcs = std::collections::HashMap::new();
        // Prelude I/O functions — available without import
        stdlib_runtime_funcs.insert("stdout".to_string(), "taida_io_stdout".to_string());
        stdlib_runtime_funcs.insert("stderr".to_string(), "taida_io_stderr".to_string());
        stdlib_runtime_funcs.insert("stdin".to_string(), "taida_io_stdin".to_string());
        // C20-2: UTF-8-aware Async[Lax[Str]] line editor (linenoise-backed)
        stdlib_runtime_funcs.insert("stdinLine".to_string(), "taida_io_stdin_line".to_string());
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
        // C19: interactive TTY-passthrough variants (import-less parity)
        stdlib_runtime_funcs.insert(
            "runInteractive".to_string(),
            "taida_os_run_interactive".to_string(),
        );
        stdlib_runtime_funcs.insert(
            "execShellInteractive".to_string(),
            "taida_os_exec_shell_interactive".to_string(),
        );
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
        // C12-6a (FB-5): Regex(pattern, flags?) prelude constructor.
        stdlib_runtime_funcs.insert("Regex".to_string(), "taida_regex_new".to_string());
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
            field_enum_descriptors: std::collections::HashMap::new(),
            enum_vars: std::collections::HashMap::new(),
            enum_returning_funcs: std::collections::HashMap::new(),
            mold_defs: std::collections::HashMap::new(),
            enum_defs: std::collections::HashMap::new(),
            type_parents: std::collections::HashMap::new(),
            mold_solidify_funcs: std::collections::HashMap::new(),
            string_returning_funcs: std::collections::HashSet::new(),
            bool_returning_funcs: std::collections::HashSet::new(),
            param_type_check_funcs: std::collections::HashSet::new(),
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
            addon_func_refs: std::collections::HashMap::new(),
            addon_facade_pack_bindings: Vec::new(),
            addon_backend: crate::addon::AddonBackend::Native,
        }
    }

    /// RC2.5: set the addon backend for this lowering run. Called by the
    /// driver immediately after `Lowering::new()` for non-native targets
    /// so that `lower_addon_import` can surface the correct backend-policy
    /// error. Native lowering can skip this call (defaults to Native).
    pub fn set_addon_backend(&mut self, backend: crate::addon::AddonBackend) {
        self.addon_backend = backend;
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

    pub(super) fn current_module_key(&self) -> &str {
        self.module_key
            .as_deref()
            .expect("module_key must be set before lowering")
    }

    pub(super) fn export_func_symbol_for_key(module_key: &str, name: &str) -> String {
        format!("_taida_fn_{}_{}", module_key, name)
    }

    pub(super) fn export_func_symbol(&self, name: &str) -> String {
        Self::export_func_symbol_for_key(self.current_module_key(), name)
    }

    pub(super) fn init_symbol_for_key(module_key: &str) -> String {
        format!("_taida_init_{}", module_key)
    }

    pub(super) fn init_symbol(&self) -> String {
        Self::init_symbol_for_key(self.current_module_key())
    }

    /// グローバル変数のハッシュキーを計算する。
    /// ライブラリモジュールの場合は "module_key:var_name" で名前空間化する。
    pub(super) fn global_var_hash(&self, var_name: &str) -> i64 {
        if let Some(ref module_key) = self.module_key
            && self.is_library_module
        {
            return simple_hash(&format!("{}:{}", module_key, var_name)) as i64;
        }
        simple_hash(var_name) as i64
    }

    pub(super) fn fallback_module_key(path: &str) -> String {
        format!("m{:016x}", simple_hash(path))
    }
}
