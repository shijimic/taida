//! Single source of truth for the `taida-lang/*` bundled package surface.
//!
//! Before this module existed, the bundled package list (and per-package
//! export lists) was hard-coded independently in the package resolver,
//! the interpreter's import materialization, the type checker's import
//! validation, the native lowering's core-bundled path classification,
//! and several tests — so adding or reclassifying a package routinely
//! updated some layers and not others. Every layer now derives its view
//! from `BUNDLED_PACKAGES`; the catalog tests pin the stub sources'
//! actual `<<< @(...)` export lines against the declared export arrays
//! so a drifted stub fails structurally (not via comment-string
//! matching).
//!
//! The catalog deliberately does NOT absorb runtime implementation
//! details, backend-specific lower-function mappings, or per-package
//! typed-signature logic — those stay in their layers; the catalog only
//! guarantees that all layers agree on which packages exist, what class
//! they are, and what they export.

/// How a bundled package participates in the language surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundledPackageClass {
    /// Backed by a runtime implementation (or per-backend mapping /
    /// documented subset): `os`, `net`, `crypto`, `abi`, `pool`.
    Runtime,
    /// Kept for legacy-JS-target compatibility only; not a release
    /// parity surface: `js`.
    Compatibility,
    /// Provides build-driver descriptor values, never ordinary runtime
    /// values: `build`.
    Descriptor,
}

/// One bundled package's catalog entry.
pub struct BundledPackageSpec {
    /// Package name under the `taida-lang/` org (e.g. `"os"`).
    pub name: &'static str,
    /// Bundled stub version recorded by the resolver.
    pub version: &'static str,
    /// Surface classification.
    pub class: BundledPackageClass,
    /// Public export list — the single place the per-package symbol
    /// surface is declared. Checker validation, materialized stub
    /// sources, and sync tests all derive from this array.
    pub exports: &'static [&'static str],
    /// Stub Taida source materialized to `~/.taida/bundled/<name>/main.td`.
    pub stub_source: &'static str,
}

/// The org every bundled package lives under.
pub const BUNDLED_ORG: &str = "taida-lang";

/// The catalog. Order is stable (used by sync tests and the resolver's
/// known-list construction); append new packages at the end.
pub const BUNDLED_PACKAGES: &[BundledPackageSpec] = &[
    BundledPackageSpec {
        name: "os",
        version: "a.1",
        class: BundledPackageClass::Runtime,
        exports: &[
            "Read",
            "ListDir",
            "Stat",
            "Exists",
            "readBytes",
            "readBytesAt",
            "writeFile",
            "writeBytes",
            "appendFile",
            "remove",
            "createDir",
            "rename",
            "run",
            "execShell",
            "runInteractive",
            "execShellInteractive",
            "EnvVar",
            "allEnv",
            "argv",
            "ReadAsync",
            "HttpGet",
            "HttpPost",
            "HttpRequest",
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
        ],
        stub_source: r#"// taida-lang/os — Core bundled package
//
// Input APIs (molds -> Lax/Bool):
//   Read[path]()       -- read file contents (64MB limit)
//   ListDir[path]()    -- list directory entries
//   Stat[path]()       -- file metadata (size, modified, isDir)
//   Exists[path]()     -- existence check (returns Bool)
//   EnvVar[name]()     -- environment variable (read-only)
//
// Binary file APIs:
//   readBytes(path)                      -- read file as Bytes (64MB limit)
//   readBytesAt(path, offset, len)       -- chunked read (offset/length window)
//   writeBytes(path, content)            -- write Bytes payload to file
//
// Side-effect APIs (functions -> Result):
//   writeFile(path, content)    -- write file (create or overwrite)
//   appendFile(path, content)   -- append to file
//   remove(path)                -- remove file/directory
//   createDir(path)             -- mkdir -p
//   rename(from, to)            -- move/rename (atomic)
//
// Process APIs (functions -> Gorillax):
//   run(program, args)          -- direct exec (safe, no shell) — captures stdout/stderr
//   execShell(command)          -- shell exec (pipes, redirects) — captures stdout/stderr
//     WARNING: Shell injection risk. Prefer run() for safety.
//
// Interactive process APIs (functions -> Gorillax[@(code: Int)], C19):
//   runInteractive(program, args)  -- TTY passthrough, child inherits parent's stdin/stdout/stderr
//   execShellInteractive(command)  -- TTY passthrough via `sh -c` (POSIX) / `cmd /C` (Windows)
//     NOTE: stdout / stderr are NOT captured — only the exit code is observable.
//     Intended for TUI apps (nvim, less, fzf, git commit). If you need to
//     inspect output programmatically, use the captured `run` / `execShell`.
//
// Query APIs:
//   allEnv()                    -- all env vars as HashMap[Str, Str]
//   argv()                      -- CLI user args as @[Str]
//
// Async input APIs (molds -> Async[Lax[T]]):
//   ReadAsync[path]()           -- async file read
//   HttpGet[url]()              -- HTTP GET
//   HttpPost[url, body]()       -- HTTP POST
//   HttpRequest[method, url](...) -- generic HTTP request
//
// Async socket APIs (functions -> Async[Result/Lax]):
//   tcpConnect(host, port[, timeoutMs])
//   tcpListen(port[, timeoutMs])
//   tcpAccept(listener[, timeoutMs])
//   socketSend(socket, data[, timeoutMs])
//   socketSendAll(socket, data[, timeoutMs])
//   socketRecv(socket[, timeoutMs])
//   socketSendBytes(socket, data[, timeoutMs])
//   socketRecvBytes(socket[, timeoutMs])
//   socketRecvExact(socket, size[, timeoutMs])
//   udpBind(host, port[, timeoutMs])
//   udpSendTo(socket, host, port, data[, timeoutMs])
//   udpRecvFrom(socket[, timeoutMs])
//   socketClose(socket)
//   listenerClose(listener)
//   udpClose(socket)            -- alias of socketClose

<<< @(Read, ListDir, Stat, Exists, readBytes, readBytesAt, writeFile, writeBytes, appendFile, remove, createDir, rename, run, execShell, runInteractive, execShellInteractive, EnvVar, allEnv, argv, ReadAsync, HttpGet, HttpPost, HttpRequest, tcpConnect, tcpListen, tcpAccept, socketSend, socketSendAll, socketRecv, socketSendBytes, socketRecvBytes, socketRecvExact, udpBind, udpSendTo, udpRecvFrom, socketClose, listenerClose, udpClose)
"#,
    },
    BundledPackageSpec {
        name: "js",
        version: "a.1",
        class: BundledPackageClass::Compatibility,
        exports: &[
            "JSGet",
            "JSCall",
            "JSCallAsync",
            "JSNew",
            "JSSet",
            "JSBind",
            "JSSpread",
        ],
        stub_source: r#"// taida-lang/js — JS interop package (core bundled)
// JSRilla[Out] subfamily of CageRilla[Branch, Out] for Cage[subject, JSRilla[...]()]() boundaries.
// canonical: Cage[subject, JSNew[@["Hono"], @[], Molten]()]() >=> app
// All descriptors are JS-backend only. Interpreter/Native will error.

<<< @(JSGet, JSCall, JSCallAsync, JSNew, JSSet, JSBind, JSSpread)
"#,
    },
    BundledPackageSpec {
        name: "crypto",
        version: "a.1",
        class: BundledPackageClass::Runtime,
        exports: &["sha256"],
        stub_source: r#"// taida-lang/crypto — Core bundled crypto package
// Current surface:
//   sha256(value) -- SHA-256 lower-hex digest
//
// Note:
//   `sha256` is exposed via taida-lang/crypto import path only.
//   Prelude compatibility is intentionally not provided.

<<< @(sha256)
"#,
    },
    BundledPackageSpec {
        name: "net",
        version: "a.1",
        class: BundledPackageClass::Runtime,
        exports: &[
            "httpServe",
            "httpParseRequestHead",
            "httpEncodeResponse",
            "readBody",
            "startResponse",
            "writeChunk",
            "endResponse",
            "sseEvent",
            "readBodyChunk",
            "readBodyAll",
            "wsUpgrade",
            "wsSend",
            "wsReceive",
            "wsClose",
            "wsCloseCode",
            "HttpProtocol",
        ],
        stub_source: r#"// taida-lang/net — Core bundled network package
// HTTP server/runtime surface only.
// Low-level socket / DNS APIs live in taida-lang/os.
//
// HTTP surface:
//   httpServe, httpParseRequestHead, httpEncodeResponse, readBody
//   startResponse, writeChunk, endResponse, sseEvent
//   readBodyChunk, readBodyAll
//   wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode
//   HttpProtocol
//
// Contract notes:
//   TLS verification on Http* uses backend default trust store (no insecure -k path)
//   Protocol/runtime details remain behind httpServe contract
//   Legacy tcp*/udp*/dnsResolve re-exports were removed after HTTP/3 package freeze

Enum => HttpProtocol = :H1 :H2 :H3

<<< @(httpServe, httpParseRequestHead, httpEncodeResponse, readBody, startResponse, writeChunk, endResponse, sseEvent, readBodyChunk, readBodyAll, wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode, HttpProtocol)
"#,
    },
    BundledPackageSpec {
        name: "pool",
        version: "a.1",
        class: BundledPackageClass::Runtime,
        exports: &[
            "poolCreate",
            "poolAcquire",
            "poolRelease",
            "poolClose",
            "poolHealth",
        ],
        stub_source: r#"// taida-lang/pool — Core bundled pool package (contract stub)
// Minimal contract (official upper package):
//   poolCreate(config) -> Result[@(pool)]
//   poolAcquire(pool[, timeoutMs]) -> Async[Result[@(resource, token), _]]
//   poolRelease(pool, token, resource) -> Result[@(ok, reused)]
//   poolClose(pool) -> Async[Result[@(ok), _]]
//   poolHealth(pool) -> @(open, idle, inUse, waiting)
//
// Implementation note:
//   Minimal in-memory pool runtime is provided by core backends.
//   Driver-level connect/validate policy is delegated to upper libraries.

<<< @(poolCreate, poolAcquire, poolRelease, poolClose, poolHealth)
"#,
    },
    BundledPackageSpec {
        name: "abi",
        version: "a.1",
        class: BundledPackageClass::Runtime,
        exports: &[
            "WebRequest",
            "WebResponse",
            "text",
            "json",
            "bytes",
            "status",
            "header",
            "HostCall",
            "HostStep",
            "HostCapability",
        ],
        stub_source: r#"// taida-lang/abi — Core bundled host/guest boundary ABI package
// Handler-mode surface:
//   WebRequest, WebResponse
//   text, json, bytes, status, header
// Host capability descriptor surface:
//   HostCall, HostStep, HostCapability

WebRequest = @(
  method: Str,
  path: Str,
  rawQuery: Str,
  query: @[@(name: Str, value: Str)],
  headers: @[@(name: Str, value: Str)],
  body: Bytes
)

WebResponse = @(
  status: Int,
  headers: @[@(name: Str, value: Str)],
  body: Bytes
)

<<< @(WebRequest, WebResponse, text, json, bytes, status, header, HostCall, HostStep, HostCapability)
"#,
    },
    BundledPackageSpec {
        name: "build",
        version: "a.1",
        class: BundledPackageClass::Descriptor,
        exports: &[
            "BuildUnit",
            "BuildPlan",
            "AssetBundle",
            "RouteAsset",
            "BuildHook",
        ],
        stub_source: r#"// taida-lang/build — Core bundled build descriptor package
// Descriptor-only surface: these symbols name build-driver descriptors
// (consumed by `taida build`), not ordinary runtime values.
//   BuildUnit(name, target, entry[, handler, assets, before])
//   BuildPlan(name, units[, assets, before])
//   AssetBundle(name, root, files[, output, before])
//   RouteAsset(path, unit | asset[, name])
//   BuildHook(name, command, cwd[, env])
//
// The descriptor parser also accepts these constructors without an
// import; importing makes the dependency explicit and keeps the public
// docs' import examples resolvable.

<<< @(BuildUnit, BuildPlan, AssetBundle, RouteAsset, BuildHook)
"#,
    },
];

/// Find a bundled package spec by its short name (e.g. `"os"`).
pub fn find(name: &str) -> Option<&'static BundledPackageSpec> {
    BUNDLED_PACKAGES.iter().find(|spec| spec.name == name)
}

/// True when `org/name` denotes a bundled package of any class.
pub fn is_core_bundled(org: &str, name: &str) -> bool {
    org == BUNDLED_ORG && find(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract the export names from a stub source's `<<< @(...)` line.
    /// This parses the actual export statement — a symbol that only
    /// survives in a comment (the F54 inventory found `dnsResolve`
    /// passing a `source.contains(..)` test that way) does not count.
    fn stub_export_line(stub: &str) -> Vec<&str> {
        let line = stub
            .lines()
            .find(|l| l.trim_start().starts_with("<<<"))
            .expect("stub source must contain a `<<< @(...)` export line");
        let inner = line
            .trim_start()
            .trim_start_matches("<<<")
            .trim()
            .strip_prefix("@(")
            .and_then(|rest| rest.strip_suffix(')'))
            .expect("export line must have the `<<< @(...)` shape");
        inner.split(',').map(|s| s.trim()).collect()
    }

    #[test]
    fn test_catalog_package_names_are_stable() {
        let names: Vec<&str> = BUNDLED_PACKAGES.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            ["os", "js", "crypto", "net", "pool", "abi", "build"],
            "bundled package catalog changed — update every consumer test in the same commit"
        );
    }

    #[test]
    fn test_catalog_exports_match_stub_export_lines() {
        for spec in BUNDLED_PACKAGES {
            let stub_exports = stub_export_line(spec.stub_source);
            assert_eq!(
                stub_exports, spec.exports,
                "package '{}': stub `<<< @(...)` line drifted from the declared exports",
                spec.name
            );
        }
    }

    #[test]
    fn test_catalog_classes() {
        use BundledPackageClass::*;
        for spec in BUNDLED_PACKAGES {
            let expected = match spec.name {
                "js" => Compatibility,
                "build" => Descriptor,
                _ => Runtime,
            };
            assert_eq!(spec.class, expected, "package '{}' class", spec.name);
        }
    }

    #[test]
    fn test_is_core_bundled_covers_every_catalog_entry() {
        // D-5 fix: the old provider test only asserted five of the six
        // packages; derive the assertion from the catalog itself.
        for spec in BUNDLED_PACKAGES {
            assert!(
                is_core_bundled(BUNDLED_ORG, spec.name),
                "is_core_bundled must accept '{}'",
                spec.name
            );
        }
        assert!(!is_core_bundled(BUNDLED_ORG, "terminal"));
        assert!(!is_core_bundled(BUNDLED_ORG, "nonexistent"));
        assert!(!is_core_bundled("someone-else", "os"));
    }

    /// F54B-005 regression: removed symbols must not be resurrected by
    /// comment text. `dnsResolve` survives only inside a net stub comment
    /// describing its removal; the parsed export line must not list it.
    #[test]
    fn test_removed_symbols_absent_from_export_lines() {
        let net = find("net").expect("net spec");
        assert!(net.stub_source.contains("dnsResolve"));
        assert!(!stub_export_line(net.stub_source).contains(&"dnsResolve"));
        assert!(!net.exports.contains(&"dnsResolve"));
    }
}
