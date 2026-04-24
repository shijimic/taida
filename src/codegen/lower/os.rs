// C13-2: `lower/stdlib.rs` から taida-lang/os + pool のマッピングを機械的に分離 (FB-21 carryover).
//
// This module groups the `Lowering` helpers that describe the
// `taida-lang/os` (filesystem / process / DNS / socket) and
// `taida-lang/pool` (connection pool) package surfaces. All items are
// moved verbatim from `src/codegen/lower/stdlib.rs` with their
// original signatures, bodies, and privacy preserved per the C13-2
// move-only policy.
//
// Relationship with other submodules:
// - `stdlib.rs` retains stdlib IO / crypto / field-tag registry helpers.
// - `net.rs` holds the `taida-lang/net` package surface.
// - `os.rs` (this file) holds the `taida-lang/os` + `taida-lang/pool`
//   surfaces.

use super::Lowering;

impl Lowering {
    /// taida-lang/os package function → C ランタイム関数名にマッピング
    pub(super) fn os_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "readBytes" => Some("taida_os_read_bytes"),
            // C26B-020 柱 1: chunked read API
            "readBytesAt" => Some("taida_os_read_bytes_at"),
            "writeFile" => Some("taida_os_write_file"),
            "writeBytes" => Some("taida_os_write_bytes"),
            "appendFile" => Some("taida_os_append_file"),
            "remove" => Some("taida_os_remove"),
            "createDir" => Some("taida_os_create_dir"),
            "rename" => Some("taida_os_rename"),
            "run" => Some("taida_os_run"),
            "execShell" => Some("taida_os_exec_shell"),
            // C19: interactive TTY-passthrough variants
            "runInteractive" => Some("taida_os_run_interactive"),
            "execShellInteractive" => Some("taida_os_exec_shell_interactive"),
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

    /// taida-lang/pool package function → C runtime function mapping.
    pub(super) fn pool_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "poolCreate" => Some("taida_pool_create"),
            "poolAcquire" => Some("taida_pool_acquire"),
            "poolRelease" => Some("taida_pool_release"),
            "poolClose" => Some("taida_pool_close"),
            "poolHealth" => Some("taida_pool_health"),
            _ => None,
        }
    }
}
