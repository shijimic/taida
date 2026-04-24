//! JS runtime: `taida-lang/os` package (13 OS APIs) + crypto.
//!
//! Split out from monolithic `src/js/runtime.rs` as part of C12-9
//! (FB-21 mechanical file split). Original source line range: 2005..3137.
//!
//! See `src/js/runtime/mod.rs::RUNTIME_JS`.

pub(super) const OS_JS: &str = r#"
// ── taida-lang/os — Core-bundled OS package (13 APIs) ──
// Uses Node.js fs, child_process, process modules.

// ESM: reuse __taida_fs for fs operations, load child_process via dynamic import
const __os_fs = __taida_fs || null;
const __os_cp = await import('node:child_process').catch(() => null);

const __OS_MAX_READ_SIZE = 64 * 1024 * 1024; // 64 MB

// Helper: create os Result success value
function __taida_os_result_ok(inner) {
  return __taida_result_create(inner, null, null);
}

// Helper: build IoError value from runtime error object.
//
// C19 compatibility: `code` and `kind` are mirrored at the top level so
// that `err.code` / `err.kind` work on the JS backend — matching the
// interpreter's `Value::Error` dot-access behaviour. The `fields` object
// is kept so existing `err.fields.code` callers keep working.
//
// C19B-001: Node exposes `err.errno` as a negative POSIX errno on Linux
// (e.g. `-2` for ENOENT). The interpreter and native backends surface the
// positive errno (`2`), so normalize here so 3-backend parity callers can
// compare `r.__error.code` without per-backend abs().
function __taida_os_io_error(err) {
  let code = err && err.errno !== undefined ? err.errno : -1;
  if (typeof code === 'number' && code < 0 && code !== -1) {
    code = -code;
  }
  const message = err && err.message ? err.message : String(err);
  const kind = __taida_os_classify_error_kind(err);
  return {
    __type: 'IoError',
    type: 'IoError',
    message: message,
    code: code,
    kind: kind,
    fields: { code: code, kind: kind },
  };
}

// Helper: classify error kind from errno/message (mirrors Rust classify_io_error_kind)
function __taida_os_classify_error_kind(err) {
  const errno = err && err.errno !== undefined ? err.errno : -1;
  const code = err && err.code ? err.code : '';
  const msg = err && err.message ? err.message : '';
  if (code === 'ETIMEDOUT' || code === 'EAGAIN' || errno === 110 || errno === 60 || errno === 11
      || msg.includes('timed out')) return 'timeout';
  if (code === 'ECONNREFUSED' || errno === 111 || errno === 61) return 'refused';
  if (code === 'ECONNRESET' || errno === 104 || errno === 54) return 'reset';
  if (code === 'ECONNABORTED' || code === 'EPIPE' || code === 'ENOTCONN'
      || errno === 32 || errno === 57 || errno === 107) return 'peer_closed';
  if (code === 'ENOENT' || errno === 2) return 'not_found';
  if (code === 'EINVAL') return 'invalid';
  return 'unknown';
}

// Helper: create os Result failure from error
function __taida_os_result_fail(err) {
  const code = err && err.errno !== undefined ? err.errno : -1;
  const message = err && err.message ? err.message : String(err);
  const kind = __taida_os_classify_error_kind(err);
  const inner = Object.freeze({ ok: false, code: code, message: message, kind: kind });
  return __taida_result_create(inner, __taida_os_io_error(err), null);
}

// Helper: create os Result failure with explicit kind/message (non-OS errors)
function __taida_os_result_fail_with_kind(kind, message) {
  const inner = Object.freeze({ ok: false, code: -1, message: message, kind: kind });
  const errVal = { __type: 'IoError', type: 'IoError', message: message, fields: { code: -1, kind: kind } };
  return __taida_result_create(inner, errVal, null);
}

function __taida_os_gorillax_ok(inner) {
  return Gorillax(inner, null);
}

function __taida_os_gorillax_fail(errVal) {
  return Gorillax(null, errVal);
}

// Helper: standard success inner @(ok=true, code=0, message="")
function __taida_os_ok_inner() {
  return Object.freeze({ ok: true, code: 0, message: '' });
}

// Helper: process result inner @(stdout, stderr, code)
function __taida_os_process_inner(stdout, stderr, code) {
  return Object.freeze({ stdout: stdout, stderr: stderr, code: code });
}

// C19: code-only inner @(code: Int) for runInteractive / execShellInteractive.
// Intentionally omits stdout / stderr because interactive variants do not
// capture them — the child writes directly to the inherited TTY.
function __taida_os_process_inner_code_only(code) {
  return Object.freeze({ code: code });
}

// C19: POSIX signal name -> number. Only the main ones are mapped; unknown
// signals fall back to 0 so that `128 + signal` becomes 128 rather than NaN.
function __taida_os_signal_to_number(sig) {
  const map = {
    SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5,
    SIGABRT: 6, SIGBUS: 7, SIGFPE: 8, SIGKILL: 9, SIGUSR1: 10,
    SIGSEGV: 11, SIGUSR2: 12, SIGPIPE: 13, SIGALRM: 14, SIGTERM: 15
  };
  return map[sig] || 0;
}

// C19: extract an exit code from a Node `spawnSync` result, following the
// `128 + signal` convention used by the interpreter and Native backends.
function __taida_os_extract_spawn_sync_code(result) {
  if (result.status !== null && result.status !== undefined) return result.status;
  if (result.signal !== null && result.signal !== undefined) {
    return 128 + __taida_os_signal_to_number(result.signal);
  }
  return -1;
}

// ── Input molds (Read, ListDir, Stat, Exists, EnvVar) ──

function __taida_os_read(path) {
  if (!__os_fs) return Lax(null, '');
  try {
    const stat = __os_fs.statSync(path);
    if (stat.size > __OS_MAX_READ_SIZE) return Lax(null, '');
    const content = __os_fs.readFileSync(path, 'utf-8');
    return Lax(content);
  } catch (e) {
    return Lax(null, '');
  }
}

function __taida_os_readBytes(path) {
  if (!__os_fs) return __taida_lax_from_bytes(new Uint8Array(0), false);
  try {
    const stat = __os_fs.statSync(path);
    if (stat.size > __OS_MAX_READ_SIZE) return __taida_lax_from_bytes(new Uint8Array(0), false);
    const content = __os_fs.readFileSync(path);
    return __taida_lax_from_bytes(new Uint8Array(content), true);
  } catch (e) {
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  }
}

// C26B-020 柱 1: chunked / large-file bytes read.
// Mirrors the Interpreter semantics from os_eval.rs:
//   - negative offset/len   → Lax failure (default Bytes[])
//   - len > 64 MB ceiling   → Lax failure (default Bytes[])
//   - len == 0              → Lax success with empty Bytes
//   - offset >= file size   → Lax success with empty Bytes
//   - offset + len > size   → Lax success with truncated tail
function __taida_os_readBytesAt(path, offset, len) {
  if (!__os_fs) return __taida_lax_from_bytes(new Uint8Array(0), false);
  const off = typeof offset === 'bigint' ? Number(offset) : (offset | 0);
  const n = typeof len === 'bigint' ? Number(len) : (len | 0);
  if (off < 0 || n < 0) return __taida_lax_from_bytes(new Uint8Array(0), false);
  if (n > __OS_MAX_READ_SIZE) return __taida_lax_from_bytes(new Uint8Array(0), false);
  if (n === 0) return __taida_lax_from_bytes(new Uint8Array(0), true);
  let fd = -1;
  try {
    const buf = Buffer.alloc(n);
    fd = __os_fs.openSync(path, 'r');
    const filled = __os_fs.readSync(fd, buf, 0, n, off);
    __os_fs.closeSync(fd);
    fd = -1;
    const view = filled === n ? new Uint8Array(buf) : new Uint8Array(buf.buffer, buf.byteOffset, filled);
    return __taida_lax_from_bytes(view, true);
  } catch (e) {
    if (fd !== -1) { try { __os_fs.closeSync(fd); } catch (_) {} }
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  }
}

function __taida_os_listdir(path) {
  if (!__os_fs) return Lax(null, Object.freeze([]));
  try {
    const entries = __os_fs.readdirSync(path).sort();
    return Lax(Object.freeze(entries));
  } catch (e) {
    return Lax(null, Object.freeze([]));
  }
}

function __taida_os_stat(path) {
  const defaultStat = Object.freeze({ size: 0, modified: '', isDir: false });
  if (!__os_fs) return Lax(null, defaultStat);
  try {
    const stat = __os_fs.statSync(path);
    const modified = new Date(stat.mtimeMs).toISOString().replace(/\.\d{3}Z$/, 'Z');
    const result = Object.freeze({
      size: stat.size,
      modified: modified,
      isDir: stat.isDirectory()
    });
    return Lax(result);
  } catch (e) {
    return Lax(null, defaultStat);
  }
}

// C12B-021: Exists now returns Result[Bool] instead of bare Bool.
// The Result envelope distinguishes "probe succeeded, path not
// present" from "probe failed (e.g. fs module unavailable)". This
// matches the Interpreter / Native contract.
function __taida_os_exists(path) {
  if (!__os_fs) {
    return __taida_os_result_fail(new __NativeError('fs module not available'));
  }
  try {
    const b = __os_fs.existsSync(path);
    return __taida_os_result_ok(b === true);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_envvar(name) {
  const val = typeof process !== 'undefined' && process.env ? process.env[name] : undefined;
  if (val !== undefined) return Lax(val);
  return Lax(null, '');
}

// ── Side-effect functions (writeFile, appendFile, remove, createDir, rename) ──

// C12B-021: the five write/remove/create APIs now return
// Result[Int]. Inner value is the byte count (writeFile /
// writeBytes / appendFile), the number of entries removed
// (remove), or 1/0 for "newly created"/"already existed"
// (createDir). Rationale and parity matrix: see
// .dev/C12_DESIGN.md §C12B-021.
function __taida_os_writeFile(path, content) {
  try {
    // Compute the byte count from the same encoding Node will use
    // when writing a string (utf-8). This lets the returned Int
    // match the Interpreter's `content.len() as i64` on ASCII/UTF-8
    // inputs byte-for-byte.
    const buf = typeof Buffer !== 'undefined'
      ? Buffer.byteLength(content, 'utf8')
      : (typeof content === 'string' ? content.length : 0);
    __os_fs.writeFileSync(path, content);
    return __taida_os_result_ok(buf);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_writeBytes(path, content) {
  try {
    const payload = __taida_to_bytes_payload(content);
    __os_fs.writeFileSync(path, payload);
    const n = (payload && typeof payload.length === 'number') ? payload.length : 0;
    return __taida_os_result_ok(n);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_appendFile(path, content) {
  try {
    const buf = typeof Buffer !== 'undefined'
      ? Buffer.byteLength(content, 'utf8')
      : (typeof content === 'string' ? content.length : 0);
    __os_fs.appendFileSync(path, content);
    return __taida_os_result_ok(buf);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_remove(path) {
  try {
    // Count the entries BEFORE removal so the returned number is
    // deterministic even when the tree traversal itself partially
    // succeeds (rare with rmSync + recursive, but well-defined).
    let count = 0;
    const walk = (p) => {
      count += 1;
      try {
        const st = __os_fs.lstatSync(p);
        if (st.isDirectory()) {
          for (const name of __os_fs.readdirSync(p)) {
            walk(p + '/' + name);
          }
        }
      } catch (_) { /* swallow — stat can race with rm */ }
    };
    try { walk(path); } catch (_) { count = count || 1; }
    __os_fs.rmSync(path, { recursive: true, force: false });
    return __taida_os_result_ok(count);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_createDir(path) {
  try {
    let already = false;
    try {
      const st = __os_fs.lstatSync(path);
      already = st.isDirectory();
    } catch (_) {
      already = false;
    }
    __os_fs.mkdirSync(path, { recursive: true });
    return __taida_os_result_ok(already ? 0 : 1);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_rename(from, to) {
  try {
    __os_fs.renameSync(from, to);
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

// ── Process functions (run, execShell) ──

// C19 note: ProcessError objects also mirror `code` at the top level (in
// addition to `fields.code`) so that `.code` on the JS backend matches the
// interpreter's `Value::Error` dot-access behaviour.
function __taida_os_process_error(program_or_cmd, code, is_shell) {
  const message = is_shell
    ? 'Shell command exited with code ' + code + ': ' + program_or_cmd
    : "Process '" + program_or_cmd + "' exited with code " + code;
  return {
    __type: 'ProcessError',
    type: 'ProcessError',
    message: message,
    code: code,
    fields: { code: code },
  };
}

function __taida_os_run(program, args) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const result = __os_cp.execFileSync(program, args || [], { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    const inner = __taida_os_process_inner(result, '', 0);
    return __taida_os_gorillax_ok(inner);
  } catch (e) {
    if (e.status !== undefined) {
      // Process exited with non-zero code
      const code = e.status !== null ? e.status : -1;
      return __taida_os_gorillax_fail(__taida_os_process_error(program, code, false));
    }
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

function __taida_os_execShell(command) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const result = __os_cp.execSync(command, { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    const inner = __taida_os_process_inner(result, '', 0);
    return __taida_os_gorillax_ok(inner);
  } catch (e) {
    if (e.status !== undefined) {
      const code = e.status !== null ? e.status : -1;
      return __taida_os_gorillax_fail(__taida_os_process_error(command, code, true));
    }
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

// ── C19: Interactive process functions (TTY passthrough) ──
//
// These variants call spawnSync with stdio: 'inherit', which hands the
// parent process's stdin / stdout / stderr file descriptors directly to the
// child. No pipes are created, and nothing is captured; the child can draw
// a TUI (nvim, vim, less, fzf, git commit) and read keystrokes live.
//
// Contract (must match the interpreter reference exactly):
// - Success: Gorillax.ok(Object.freeze({ code: 0 }))
// - Non-zero exit: Gorillax.err(ProcessError{code})
// - Pre-exec failure (ENOENT etc.): Gorillax.err(IoError{code, kind})
// - Signal death: code = 128 + signum (best-effort)
//
// Note: inner shape is { code } only — stdout / stderr are deliberately
// absent to signal that the caller cannot observe child I/O.

function __taida_os_runInteractive(program, args) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const result = __os_cp.spawnSync(program, args || [], { stdio: 'inherit' });

    if (result.error) {
      return __taida_os_gorillax_fail(__taida_os_io_error(result.error));
    }

    const code = __taida_os_extract_spawn_sync_code(result);
    const inner = __taida_os_process_inner_code_only(code);
    if (code === 0) {
      return __taida_os_gorillax_ok(inner);
    }
    return __taida_os_gorillax_fail(__taida_os_process_error(program, code, false));
  } catch (e) {
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

function __taida_os_execShellInteractive(command) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const isWin = typeof process !== 'undefined' && process.platform === 'win32';
    const shellProgram = isWin ? 'cmd' : 'sh';
    const shellArgs = isWin ? ['/C', command] : ['-c', command];
    const result = __os_cp.spawnSync(shellProgram, shellArgs, { stdio: 'inherit' });

    if (result.error) {
      return __taida_os_gorillax_fail(__taida_os_io_error(result.error));
    }

    const code = __taida_os_extract_spawn_sync_code(result);
    const inner = __taida_os_process_inner_code_only(code);
    if (code === 0) {
      return __taida_os_gorillax_ok(inner);
    }
    return __taida_os_gorillax_fail(__taida_os_process_error(command, code, true));
  } catch (e) {
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

// ── Query function (allEnv) ──

function __taida_os_allEnv() {
  const entries = [];
  if (typeof process !== 'undefined' && process.env) {
    for (const [key, value] of Object.entries(process.env)) {
      entries.push(Object.freeze({ key: key, value: value || '' }));
    }
  }
  return __taida_createHashMap(entries);
}

function __taida_os_argv() {
  if (typeof process === 'undefined' || !Array.isArray(process.argv)) {
    return Object.freeze([]);
  }
  return Object.freeze(process.argv.slice(2));
}

// ── Phase 2: Async APIs ───────────────────────────────────

// Helper: build HTTP response Lax
function __taida_os_http_response(status, body, headers) {
  const headerObj = {};
  if (headers) {
    for (const [k, v] of headers) {
      headerObj[k.toLowerCase()] = v;
    }
  }
  return Lax(Object.freeze({ status: status, body: body, headers: Object.freeze(headerObj) }));
}

function __taida_os_http_failure() {
  return Lax(null, Object.freeze({ status: 0, body: '', headers: Object.freeze({}) }));
}

// ReadAsync[path]() -> Promise<Lax[Str]>
async function __taida_os_readAsync(path) {
  if (!__os_fs) return Lax(null, '');
  try {
    const fsp = __os_fs.promises || await import('node:fs/promises').then(m => m.default || m).catch(() => null);
    if (!fsp) return Lax(null, '');
    const stat = await fsp.stat(path);
    if (stat.size > 64 * 1024 * 1024) return Lax(null, '');
    const content = await fsp.readFile(path, 'utf-8');
    return Lax(content);
  } catch (e) {
    return Lax(null, '');
  }
}

// HttpGet[url]() -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpGet(url) {
  try {
    const resp = await fetch(url);
    const body = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, body, headers);
  } catch (e) {
    return __taida_os_http_failure();
  }
}

// HttpPost[url, body]() -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpPost(url, body) {
  try {
    const resp = await fetch(url, { method: 'POST', body: body || '' });
    const respBody = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, respBody, headers);
  } catch (e) {
    return __taida_os_http_failure();
  }
}

// HttpRequest[method, url](headers, body) -> Promise<Lax[@(status, body, headers)]>
//
// C20-4 (C19B-007): `reqHeaders` now accepts two shapes to mirror the
// interpreter and native backends:
//
//   * BuchiPack object — legacy `headers <= @(content_type <= "...")`
//     (each own-enumerable key is treated as a wire header name; the
//     identifier ban on `-` means dash-bearing headers are unreachable
//     this way).
//   * Array of records — new `headers <= @[@(name <= "x-api-key",
//     value <= "...")]`. Each entry with Str `name` + Str `value`
//     contributes one wire header; arbitrary UTF-8 is allowed in the
//     name, so `x-api-key`, `anthropic-version`, etc. are expressible.
async function __taida_os_httpRequest(method, url, reqHeaders, body) {
  try {
    const opts = { method: method || 'GET' };
    if (body) opts.body = body;
    if (reqHeaders) {
      const h = {};
      if (Array.isArray(reqHeaders)) {
        for (const rec of reqHeaders) {
          if (rec && typeof rec === 'object') {
            const n = rec.name;
            const v = rec.value;
            if (typeof n === 'string' && typeof v === 'string' && n.length > 0) {
              h[n] = v;
            }
          }
        }
      } else if (typeof reqHeaders === 'object') {
        for (const [k, v] of Object.entries(reqHeaders)) {
          if (typeof v === 'string') h[k] = v;
        }
      }
      if (Object.keys(h).length > 0) opts.headers = h;
    }
    const resp = await fetch(url, opts);
    const respBody = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, respBody, headers);
  } catch (e) {
    return __taida_os_http_failure();
  }
}

// TCP: load net module
const __os_net = await import('node:net').catch(() => null);
const __os_tls = await import('node:tls').catch(() => null);
const __os_dgram = await import('node:dgram').catch(() => null);
const __TAIDA_OS_NETWORK_TIMEOUT_MS = 30000;

function __taida_os_network_timeout_ms(timeoutMs) {
  if (typeof timeoutMs === 'number' && Number.isFinite(timeoutMs)) {
    const ms = Math.floor(timeoutMs);
    if (ms > 0 && ms <= 600000) return ms;
  }
  return __TAIDA_OS_NETWORK_TIMEOUT_MS;
}

// tcpConnect(host, port, timeoutMs?) -> Promise<Result[@(socket, host, port), _]>
async function __taida_os_tcpConnect(host, port, timeoutMs) {
  if (!__os_net) return __taida_os_result_fail(new __NativeError('net module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    const socket = new (__os_net.Socket || __os_net.default.Socket)();
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('connect', onConnect);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onConnect = () => {
      const inner = Object.freeze({ socket: socket, host: host, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`tcpConnect: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      if (typeof socket.destroy === 'function') socket.destroy();
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('connect', onConnect);
    socket.once('error', onError);
    try {
      socket.connect(port, host);
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// tcpListen(port, timeoutMs?) -> Promise<Result[@(listener, port), _]>
async function __taida_os_tcpListen(port, timeoutMs) {
  if (!__os_net) return __taida_os_result_fail(new __NativeError('net module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    const server = (__os_net.createServer || __os_net.default.createServer)();
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      server.removeListener('listening', onListening);
      server.removeListener('error', onError);
      resolve(result);
    };
    const onListening = () => {
      const inner = Object.freeze({ listener: server, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`tcpListen: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      try { server.close(); } catch (_) {}
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    server.once('listening', onListening);
    server.once('error', onError);
    try {
      server.listen(port, '127.0.0.1');
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// tcpAccept(listener, timeoutMs?) -> Promise<Result[@(socket, host, port), _]>
async function __taida_os_tcpAccept(listenerOrPack, timeoutMs) {
  const server = (listenerOrPack && listenerOrPack.listener) ? listenerOrPack.listener : listenerOrPack;
  if (!server || !server.once) return __taida_os_result_fail(new __NativeError('tcpAccept: invalid listener'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      server.removeListener('connection', onConnection);
      server.removeListener('error', onError);
      resolve(result);
    };
    const onConnection = (socket) => {
      const addr = socket.remoteAddress || '';
      const port = socket.remotePort || 0;
      const inner = Object.freeze({ socket: socket, host: addr, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`tcpAccept: timed out after ${effectiveTimeout}ms`);
      try { err.errno = 110; } catch (_) {}
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    server.once('connection', onConnection);
    server.once('error', onError);
  });
}

// socketSend(socket, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
async function __taida_os_socketSend(socketOrPack, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.write) return __taida_os_result_fail(new __NativeError('Invalid socket'));
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`socketSend: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.write(payload, (err) => {
      if (err) {
          finish(__taida_os_result_fail(err));
      } else {
        const inner = Object.freeze({ ok: true, bytesSent: payload.length });
          finish(__taida_os_result_ok(inner));
      }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// socketSendBytes(socket, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
async function __taida_os_socketSendBytes(socketOrPack, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.write) return __taida_os_result_fail(new __NativeError('Invalid socket'));
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`socketSendBytes: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.write(payload, (err) => {
      if (err) {
          finish(__taida_os_result_fail(err));
      } else {
        const inner = Object.freeze({ ok: true, bytesSent: payload.length });
          finish(__taida_os_result_ok(inner));
      }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// socketSendAll(socket, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
// In Node.js, socket.write() already buffers, so this is equivalent to socketSend.
async function __taida_os_socketSendAll(socketOrPack, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.write) return __taida_os_result_fail(new __NativeError('socketSendAll: invalid socket'));
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`socketSendAll: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.write(payload, (err) => {
      if (err) {
          finish(__taida_os_result_fail(err));
      } else {
        const inner = Object.freeze({ ok: true, bytesSent: payload.length });
          finish(__taida_os_result_ok(inner));
      }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// socketRecv(socket, timeoutMs?) -> Promise<Lax[Str]>
async function __taida_os_socketRecv(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return Lax(null, '');
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('end', onEnd);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onData = (chunk) => {
      finish(Lax(chunk.toString('utf-8')));
    };
    const onEnd = () => {
      finish(Lax(null, ''));
    };
    const onError = () => {
      finish(Lax(null, ''));
    };
    const timer = setTimeout(() => {
      finish(Lax(null, ''));
    }, effectiveTimeout);

    socket.once('data', onData);
    socket.once('end', onEnd);
    socket.once('error', onError);
  });
}

// socketRecvBytes(socket, timeoutMs?) -> Promise<Lax[Bytes]>
async function __taida_os_socketRecvBytes(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return __taida_lax_from_bytes(new Uint8Array(0), false);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('end', onEnd);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onData = (chunk) => {
      const bytes = chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
      finish(__taida_lax_from_bytes(bytes, true));
    };
    const onEnd = () => {
      finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    };
    const onError = () => {
      finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    };
    const timer = setTimeout(() => {
      finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    }, effectiveTimeout);

    socket.once('data', onData);
    socket.once('end', onEnd);
    socket.once('error', onError);
  });
}

// udpBind(host, port, timeoutMs?) -> Promise<Result[@(socket, host, port), _]>
async function __taida_os_udpBind(host, port, timeoutMs) {
  if (!__os_dgram) return __taida_os_result_fail(new __NativeError('dgram module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    const socket = (__os_dgram.createSocket || __os_dgram.default.createSocket)('udp4');
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('listening', onListening);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onListening = () => {
      const inner = Object.freeze({ socket: socket, host: host, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`udpBind: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      try { socket.close(); } catch (_) {}
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('listening', onListening);
    socket.once('error', onError);
    try {
      socket.bind(port, host);
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// udpSendTo(socket, host, port, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
async function __taida_os_udpSendTo(socketOrPack, host, port, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || typeof socket.send !== 'function') {
    return __taida_os_result_fail(new __NativeError('udpSendTo: invalid socket'));
  }
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`udpSendTo: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.send(payload, port, host, (err, bytes) => {
        if (err) {
          finish(__taida_os_result_fail(err));
        } else {
          const inner = Object.freeze({ ok: true, bytesSent: bytes });
          finish(__taida_os_result_ok(inner));
        }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// udpRecvFrom(socket, timeoutMs?) -> Promise<Lax[@(host, port, data, truncated)]>
async function __taida_os_udpRecvFrom(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  const defaultPayload = Object.freeze({ host: '', port: 0, data: new Uint8Array(0), truncated: false });
  if (!socket || typeof socket.once !== 'function') return Lax(null, defaultPayload);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('message', onMessage);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onMessage = (msg, rinfo) => {
      const cap = 65507;
      const truncated = msg.length > cap;
      const data = truncated ? msg.subarray(0, cap) : msg;
      const payload = Object.freeze({
        host: (rinfo && typeof rinfo.address === 'string') ? rinfo.address : '',
        port: (rinfo && Number.isFinite(rinfo.port)) ? rinfo.port : 0,
        data: new Uint8Array(data),
        truncated: truncated,
      });
      finish(Lax(payload, defaultPayload));
    };
    const onError = () => {
      finish(Lax(null, defaultPayload));
    };
    const timer = setTimeout(() => {
      finish(Lax(null, defaultPayload));
    }, effectiveTimeout);

    socket.once('message', onMessage);
    socket.once('error', onError);
  });
}

// socketClose(socket) -> Promise<Result[@(ok, code, message), _]>
async function __taida_os_socketClose(socketOrPack) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || typeof socket !== 'object') {
    return __taida_os_result_fail(new __NativeError('socketClose: invalid socket'));
  }
  if (socket.__taidaClosed === true || socket.destroyed === true) {
    return __taida_os_result_fail(new __NativeError('socketClose: socket already closed'));
  }
  try {
    if (typeof socket.end === 'function') socket.end();
    if (typeof socket.close === 'function') socket.close();
    if (typeof socket.destroy === 'function') socket.destroy();
    socket.__taidaClosed = true;
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

// listenerClose(listener) -> Promise<Result[@(ok, code, message), _]>
async function __taida_os_listenerClose(listenerOrPack) {
  const listener = (listenerOrPack && listenerOrPack.listener) ? listenerOrPack.listener : listenerOrPack;
  if (!listener || typeof listener.close !== 'function') {
    return __taida_os_result_fail(new __NativeError('listenerClose: invalid listener'));
  }
  if (listener.__taidaClosed === true || listener.listening === false) {
    return __taida_os_result_fail(new __NativeError('listenerClose: listener already closed'));
  }

  return new Promise((resolve) => {
    listener.close((err) => {
      if (err) {
        resolve(__taida_os_result_fail(err));
      } else {
        listener.__taidaClosed = true;
        resolve(__taida_os_result_ok(__taida_os_ok_inner()));
      }
    });
  });
}

// udpClose(socket) is an alias of socketClose(socket)
async function __taida_os_udpClose(socketOrPack) {
  return __taida_os_socketClose(socketOrPack);
}

// ── socketRecvExact(socket, size, timeoutMs?) → Promise<Lax[Bytes]> ──
async function __taida_os_socketRecvExact(socketOrPack, size, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return __taida_lax_from_bytes(new Uint8Array(0), false);
  if (!__taida_isIntNumber(size) || size < 0) return __taida_lax_from_bytes(new Uint8Array(0), false);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const chunks = [];
    let received = 0;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('error', onError);
      socket.removeListener('end', onEnd);
      resolve(result);
    };
    const onData = (chunk) => {
      const buf = chunk instanceof Uint8Array ? chunk : Buffer.from(chunk);
      chunks.push(buf);
      received += buf.length;
      if (received >= size) {
        const all = Buffer.concat(chunks);
        const exact = new Uint8Array(all.slice(0, size));
        // Push remaining bytes back (if any) by unshifting
        if (all.length > size) {
          socket.unshift(all.slice(size));
        }
        finish(__taida_lax_from_bytes(exact, true));
      }
    };
    const onError = () => finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    const onEnd = () => finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    const timer = setTimeout(() => finish(__taida_lax_from_bytes(new Uint8Array(0), false)), effectiveTimeout);
    socket.on('data', onData);
    socket.once('error', onError);
    socket.once('end', onEnd);
  });
}

// ── dnsResolve(host, timeoutMs?) → Promise<Result[@(addresses), _]> ──
async function __taida_os_dnsResolve(host, timeoutMs) {
  const dns = await import('node:dns').catch(() => null);
  if (!dns) return __taida_os_result_fail(new __NativeError('dns module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => { if (!settled) { settled = true; clearTimeout(timer); resolve(result); } };
    const timer = setTimeout(() => {
      const err = new __NativeError(`dnsResolve: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);
    dns.promises.lookup(host, { all: true }).then((results) => {
      const seen = new __NativeSet();
      const addrs = [];
      for (const r of results) {
        if (!seen.has(r.address)) { seen.add(r.address); addrs.push(r.address); }
      }
      if (addrs.length === 0) {
        const err = new __NativeError(`dnsResolve: no records for '${host}'`);
        err.code = 'ENOENT';
        finish(__taida_os_result_fail(err));
      } else {
        finish(__taida_os_result_ok(Object.freeze({ addresses: addrs })));
      }
    }).catch((err) => {
      finish(__taida_os_result_fail(err));
    });
  });
}

// ── Pool management (in-process, no real connections) ──
const __taida_pool_states = new __NativeMap();
let __taida_next_pool_id = 1;

function __taida_os_poolCreate(config) {
  if (!config || typeof config !== 'object') {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: config must be a pack, got ' + String(config));
  }
  const maxSize = config.maxSize !== undefined ? config.maxSize : 10;
  if (!__taida_isIntNumber(maxSize) || maxSize <= 0) {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: maxSize must be > 0, got ' + maxSize);
  }
  let maxIdle = config.maxIdle !== undefined ? config.maxIdle : maxSize;
  if (!__taida_isIntNumber(maxIdle) || maxIdle < 0) {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: maxIdle must be >= 0, got ' + maxIdle);
  }
  if (maxIdle > maxSize) maxIdle = maxSize;
  const acquireTimeoutMs = config.acquireTimeoutMs !== undefined ? config.acquireTimeoutMs : 30000;
  if (!__taida_isIntNumber(acquireTimeoutMs) || acquireTimeoutMs <= 0) {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: acquireTimeoutMs must be > 0, got ' + acquireTimeoutMs);
  }
  const poolId = __taida_next_pool_id++;
  __taida_pool_states.set(poolId, {
    open: true, maxSize, maxIdle, acquireTimeoutMs,
    idle: [], inUse: new __NativeSet(), nextToken: 1
  });
  return __taida_os_result_ok(Object.freeze({ pool: poolId }));
}

async function __taida_os_poolAcquire(poolOrPack, timeoutMs) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return __taida_os_result_fail_with_kind('invalid', 'poolAcquire: unknown pool handle');
  if (!state.open) return __taida_os_result_fail_with_kind('closed', 'poolAcquire: pool is closed');
  const effectiveTimeout = (__taida_isIntNumber(timeoutMs) && timeoutMs > 0)
    ? timeoutMs : state.acquireTimeoutMs;
  let resource = Object.freeze({});
  let token;
  if (state.idle.length > 0) {
    const entry = state.idle.pop();
    resource = entry.resource;
    token = entry.token;
  } else if (state.inUse.size < state.maxSize) {
    token = state.nextToken++;
  } else {
    return __taida_os_result_fail_with_kind('timeout', `poolAcquire: timed out after ${effectiveTimeout}ms`);
  }
  state.inUse.add(token);
  return __taida_os_result_ok(Object.freeze({ resource, token }));
}

function __taida_os_poolRelease(poolOrPack, token, resource) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return __taida_os_result_fail_with_kind('invalid', 'poolRelease: unknown pool handle');
  if (!state.open) return __taida_os_result_fail_with_kind('closed', 'poolRelease: pool is closed');
  if (!state.inUse.has(token)) return __taida_os_result_fail_with_kind('invalid', 'poolRelease: token is not in-use');
  state.inUse.delete(token);
  let reused = false;
  if (state.idle.length < state.maxIdle) {
    state.idle.push({ token, resource });
    reused = true;
  }
  return __taida_os_result_ok(Object.freeze({ ok: true, reused }));
}

async function __taida_os_poolClose(poolOrPack) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return __taida_os_result_fail_with_kind('invalid', 'poolClose: unknown pool handle');
  if (!state.open) return __taida_os_result_fail_with_kind('closed', 'poolClose: pool already closed');
  state.open = false;
  state.idle.length = 0;
  state.inUse.clear();
  return __taida_os_result_ok(Object.freeze({ ok: true }));
}

function __taida_os_poolHealth(poolOrPack) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return Object.freeze({ open: false, idle: 0, inUse: 0, waiting: 0 });
  return Object.freeze({
    open: state.open,
    idle: state.idle.length,
    inUse: state.inUse.size,
    waiting: 0
  });
}

// ── Cancel mold: Cancel[async]() → cancelled Async ──
function Cancel_mold(asyncVal) {
  if (asyncVal instanceof __TaidaAsync) {
    if (asyncVal.status === 'pending') {
      return new __TaidaAsync(
        null,
        new __TaidaError('CancelledError', 'Async operation cancelled', {}),
        'rejected'
      );
    }
    return asyncVal;
  }
  // Non-async: wrap as fulfilled
  return new __TaidaAsync(asyncVal, null, 'fulfilled');
}

// ── Endian pack/unpack molds ──
function U16BE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 65535)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([(value >> 8) & 0xff, value & 0xff]), true);
}
function U16LE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 65535)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([value & 0xff, (value >> 8) & 0xff]), true);
}
function U32BE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 4294967295)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([
    (value >>> 24) & 0xff, (value >>> 16) & 0xff, (value >>> 8) & 0xff, value & 0xff
  ]), true);
}
function U32LE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 4294967295)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([
    value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff
  ]), true);
}
function U16BEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 2) return Lax(null, 0);
  return Lax((bytes[0] << 8) | bytes[1]);
}
function U16LEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 2) return Lax(null, 0);
  return Lax((bytes[1] << 8) | bytes[0]);
}
function U32BEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 4) return Lax(null, 0);
  return Lax(((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3]) >>> 0);
}
function U32LEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 4) return Lax(null, 0);
  return Lax(((bytes[3] << 24) | (bytes[2] << 16) | (bytes[1] << 8) | bytes[0]) >>> 0);
}

// ── BytesCursor molds ──
function BytesCursor_mold(bytesVal) {
  if (!(bytesVal instanceof Uint8Array)) bytesVal = new Uint8Array(0);
  const offset = 0;
  return Object.freeze({
    __type: 'BytesCursor',
    bytes: bytesVal,
    offset: offset,
    length: bytesVal.length
  });
}
function BytesCursorRemaining_mold(cursor) {
  if (!cursor || cursor.__type !== 'BytesCursor') return 0;
  return Math.max(0, cursor.bytes.length - cursor.offset);
}
function BytesCursorTake_mold(cursor, size) {
  const makeCursor = (b, o) => Object.freeze({ __type: 'BytesCursor', bytes: b, offset: o, length: b.length });
  const makeStep = (v, c) => Object.freeze({ value: v, cursor: c });
  if (!cursor || cursor.__type !== 'BytesCursor') {
    const emptyCursor = makeCursor(new Uint8Array(0), 0);
    const defStep = makeStep(new Uint8Array(0), emptyCursor);
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const bytes = cursor.bytes;
  const off = cursor.offset;
  const currentCursor = makeCursor(bytes, off);
  const defStep = makeStep(new Uint8Array(0), currentCursor);
  if (!__taida_isIntNumber(size) || size < 0) {
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  if (off + size > bytes.length) {
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const chunk = new Uint8Array(bytes.slice(off, off + size));
  const nextCursor = makeCursor(bytes, off + size);
  const step = makeStep(chunk, nextCursor);
  return __taida_lax_from_bytes_cursor_step(step, true);
}
function BytesCursorU8_mold(cursor) {
  const makeCursor = (b, o) => Object.freeze({ __type: 'BytesCursor', bytes: b, offset: o, length: b.length });
  const makeStep = (v, c) => Object.freeze({ value: v, cursor: c });
  if (!cursor || cursor.__type !== 'BytesCursor') {
    const emptyCursor = makeCursor(new Uint8Array(0), 0);
    const defStep = makeStep(0, emptyCursor);
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const bytes = cursor.bytes;
  const off = cursor.offset;
  const currentCursor = makeCursor(bytes, off);
  const defStep = makeStep(0, currentCursor);
  if (off >= bytes.length) {
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const value = bytes[off];
  const nextCursor = makeCursor(bytes, off + 1);
  const step = makeStep(value, nextCursor);
  return __taida_lax_from_bytes_cursor_step(step, true);
}
// Helper: create Lax wrapping a BytesCursor step (value+cursor pair)
function __taida_lax_from_bytes_cursor_step(step, hasValue) {
  const val = step;
  const def = Object.freeze({ value: step.value, cursor: step.cursor });
  return Object.freeze({
    __type: 'Lax',
    __value: hasValue ? val : def,
    __default: def,
    hasValue: __taida_hasValue(!!hasValue),
    isEmpty() { return !hasValue; },
    getOrDefault(d) { return hasValue ? val : d; },
    map(fn) { return hasValue ? Lax(fn(val)) : this; },
    flatMap(fn) {
      if (!hasValue) return this;
      const result = fn(val);
      if (result && result.__type === 'Lax') return result;
      return Lax(result);
    },
    unmold() { return hasValue ? val : def; },
    toString() { return hasValue ? 'Lax(BytesCursorStep)' : 'Lax(default: BytesCursorStep)'; },
  });
}

// ── taida-lang/crypto: sha256 ──────────────────────────────────
const __taida_crypto = await import('node:crypto').catch(() => null);
function sha256(value) {
  const data = typeof value === 'string' ? value : String(value);
  if (__taida_crypto) {
    return __taida_crypto.createHash('sha256').update(data, 'utf8').digest('hex');
  }
  // Fallback: pure-JS SHA-256 (should not reach here in Node.js)
  return '';
}
"#;
