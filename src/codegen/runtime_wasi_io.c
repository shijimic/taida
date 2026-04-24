/**
 * runtime_wasi_io.c — wasm-wasi I/O layer
 *
 * WW-2: WASI import-based I/O for env, file read/write, exists.
 *
 * This file is compiled and linked ONLY for wasm-wasi builds.
 * It depends on functions from runtime_core_wasm.c (e.g., wasm_alloc,
 * taida_lax_new, taida_lax_empty, taida_result_create,
 * taida_make_error, taida_str_hash, taida_hashmap_new, taida_hashmap_set,
 * taida_hashmap_set_value_tag).
 *
 * runtime_core_wasm.c is FROZEN — we do not modify it.
 * All wasm-wasi-specific code lives here.
 *
 * WASI ABI: wasi_snapshot_preview1
 * Runner baseline: wasmtime v25+
 */

#include <stdint.h>

/* ── External declarations from runtime_core_wasm.c ── */

/* Bump allocator (defined in runtime_core_wasm.c, non-static for sharing) */
extern void *wasm_alloc(unsigned int size);

/* String utilities (defined in runtime_core_wasm.c as static, so we
   re-declare minimal helpers here) */
static int32_t wasi_strlen(const char *s) {
    int32_t n = 0;
    while (s[n]) n++;
    return n;
}

static void wasi_memcpy(void *dest, const void *src, int32_t n) {
    char *d = (char *)dest;
    const char *s = (const char *)src;
    while (n-- > 0) *d++ = *s++;
}

/* Lax constructor/empty (defined in runtime_core_wasm.c) */
extern int64_t taida_lax_new(int64_t value, int64_t default_value);
extern int64_t taida_lax_empty(int64_t default_value);

/* Result (defined in runtime_core_wasm.c) */
extern int64_t taida_result_create(int64_t value, int64_t throw_val, int64_t predicate);
extern int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr);

/* HashMap (for allEnv) */
extern int64_t taida_hashmap_new(void);
extern int64_t taida_hashmap_set(int64_t hm, int64_t key_hash, int64_t key_ptr, int64_t value);
extern void taida_hashmap_set_value_tag(int64_t hm, int64_t tag);
extern int64_t taida_str_hash(int64_t str_ptr);
extern int64_t taida_register_field_name(int64_t hash, int64_t name_ptr);

/* Pack (for Result inner) */
extern int64_t taida_pack_new(int64_t field_count);
extern int64_t taida_pack_set(int64_t pack_ptr, int64_t index, int64_t value);
extern int64_t taida_pack_set_hash(int64_t pack_ptr, int64_t index, int64_t hash);
extern int64_t taida_pack_set_tag(int64_t pack_ptr, int64_t index, int64_t tag);

/* Type tags (must match runtime_core_wasm.c) */
#define WASM_TAG_STR 1

/* ── WASI imports (wasi_snapshot_preview1) ── */

typedef int32_t wasi_fd;

typedef struct {
    int32_t buf;
    int32_t len;
} __attribute__((packed)) wasi_ciovec;

typedef struct {
    int32_t buf;
    int32_t len;
} __attribute__((packed)) wasi_iovec;

/* environ_sizes_get: (environ_count_ptr, environ_buf_size_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("environ_sizes_get")))
extern int32_t __wasi_environ_sizes_get(int32_t *environ_count, int32_t *environ_buf_size);

/* environ_get: (environ_ptr, environ_buf_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("environ_get")))
extern int32_t __wasi_environ_get(int32_t *environ, char *environ_buf);

/* path_open: (dirfd, dirflags, path, path_len, oflags, fs_rights_base,
               fs_rights_inheriting, fdflags, fd_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("path_open")))
extern int32_t __wasi_path_open(
    wasi_fd dirfd,
    int32_t dirflags,
    const char *path,
    int32_t path_len,
    int32_t oflags,
    int64_t fs_rights_base,
    int64_t fs_rights_inheriting,
    int32_t fdflags,
    wasi_fd *opened_fd
);

/* fd_read: (fd, iovs, iovs_len, nread_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_read")))
extern int32_t __wasi_fd_read(wasi_fd fd, const wasi_iovec *iovs,
                              int32_t iovs_len, int32_t *nread);

/* fd_write: same import as runtime_core_wasm.c; wasm-ld deduplicates imports */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")))
extern int32_t __wasi_fd_write(wasi_fd fd, const wasi_ciovec *iovs,
                               int32_t iovs_len, int32_t *nwritten);

/* fd_close: (fd) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_close")))
extern int32_t __wasi_fd_close(wasi_fd fd);

/* fd_seek: (fd, offset, whence, newoffset_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_seek")))
extern int32_t __wasi_fd_seek(wasi_fd fd, int64_t offset, int32_t whence,
                              int64_t *newoffset);

/* fd_prestat_get: (fd, prestat_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_prestat_get")))
extern int32_t __wasi_fd_prestat_get(wasi_fd fd, void *prestat);

/* fd_prestat_dir_name: (fd, path, path_len) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_prestat_dir_name")))
extern int32_t __wasi_fd_prestat_dir_name(wasi_fd fd, char *path, int32_t path_len);

/* ── WASI constants ── */

/* oflags */
#define WASI_O_CREAT   1
#define WASI_O_TRUNC   8

/* whence for fd_seek */
#define WASI_SEEK_SET  0
#define WASI_SEEK_CUR  1
#define WASI_SEEK_END  2

/* rights */
#define WASI_RIGHT_FD_READ         (1LL << 1)
#define WASI_RIGHT_FD_WRITE        (1LL << 6)
#define WASI_RIGHT_FD_SEEK         (1LL << 2)
#define WASI_RIGHT_PATH_OPEN       (1LL << 8)
#define WASI_RIGHT_FD_FILESTAT_GET (1LL << 21)

/* prestat tag */
#define WASI_PREOPENTYPE_DIR 0

/* prestat struct layout: tag(i32) + dir_name_len(i32) */
typedef struct {
    int32_t tag;
    int32_t dir_name_len;
} wasi_prestat;

/* ── Preopened FD resolution ── */

/* Resolve a path against preopened directories.
   WASI requires file operations to use preopened directory FDs with relative paths.
   wasmtime assigns FD 3+ for preopened dirs (0=stdin, 1=stdout, 2=stderr).

   Returns the best-matching preopened FD and writes the relative path
   (relative to that preopened dir) into rel_path/rel_path_len.

   For absolute paths like "/tmp/file.txt" with preopened "/tmp",
   the relative path becomes "file.txt".
   For relative paths, returns the first preopened dir and uses the path as-is. */

typedef struct {
    wasi_fd fd;
    const char *rel_path;
    int32_t rel_path_len;
} wasi_resolved_path;

static wasi_resolved_path resolve_preopened_path(const char *path, int32_t path_len) {
    wasi_resolved_path result;
    result.fd = 3;  /* fallback */
    result.rel_path = path;
    result.rel_path_len = path_len;

    wasi_fd best_fd = -1;
    int32_t best_prefix_len = -1;

    /* Scan preopened FDs 3..31 */
    for (wasi_fd fd = 3; fd < 32; fd++) {
        wasi_prestat ps;
        int32_t err = __wasi_fd_prestat_get(fd, &ps);
        if (err != 0) break;  /* No more preopened FDs */
        if (ps.tag != WASI_PREOPENTYPE_DIR) continue;

        int32_t dir_name_len = ps.dir_name_len;
        if (dir_name_len <= 0) {
            /* Empty dir name (e.g., ".") — use as fallback if no better match */
            if (best_fd == -1) {
                best_fd = fd;
                best_prefix_len = 0;
            }
            continue;
        }

        /* Get the directory name */
        char *dir_name = (char *)wasm_alloc(dir_name_len + 1);
        if (!dir_name) continue;
        err = __wasi_fd_prestat_dir_name(fd, dir_name, dir_name_len);
        if (err != 0) continue;
        dir_name[dir_name_len] = '\0';

        /* Remove trailing slash from dir_name for matching */
        int32_t match_len = dir_name_len;
        while (match_len > 0 && dir_name[match_len - 1] == '/') match_len--;

        /* Check if path starts with dir_name */
        if (match_len <= path_len) {
            int match = 1;
            for (int32_t i = 0; i < match_len; i++) {
                if (path[i] != dir_name[i]) { match = 0; break; }
            }
            /* Path must match the prefix and then have '/' or end */
            if (match && (match_len == path_len || path[match_len] == '/')) {
                if (match_len > best_prefix_len) {
                    best_fd = fd;
                    best_prefix_len = match_len;
                }
            }
        }
    }

    if (best_fd != -1) {
        result.fd = best_fd;
        if (best_prefix_len > 0 && best_prefix_len < path_len) {
            /* Strip the prefix + '/' */
            int32_t skip = best_prefix_len;
            if (path[skip] == '/') skip++;
            result.rel_path = path + skip;
            result.rel_path_len = path_len - skip;
        } else if (best_prefix_len == 0) {
            /* Preopened dir is "." or empty — use path as-is */
            result.rel_path = path;
            result.rel_path_len = path_len;
        } else {
            /* Path equals the prefix exactly — use "." */
            result.rel_path = ".";
            result.rel_path_len = 1;
        }
    }

    return result;
}

/* ── Helper: allocate a copy of a string ── */

static char *wasi_str_copy(const char *src, int32_t len) {
    char *buf = (char *)wasm_alloc(len + 1);
    if (!buf) return (char *)0;
    wasi_memcpy(buf, src, len);
    buf[len] = '\0';
    return buf;
}

/* ── EnvVar[name]() → Lax[Str] ── */

int64_t taida_os_env_var(int64_t name_ptr) {
    const char *name = (const char *)(intptr_t)name_ptr;
    if (!name) return taida_lax_empty((int64_t)(intptr_t)"");

    int32_t name_len = wasi_strlen(name);
    if (name_len == 0) return taida_lax_empty((int64_t)(intptr_t)"");

    /* Get environ sizes */
    int32_t env_count = 0;
    int32_t env_buf_size = 0;
    int32_t err = __wasi_environ_sizes_get(&env_count, &env_buf_size);
    if (err != 0 || env_count == 0) return taida_lax_empty((int64_t)(intptr_t)"");

    /* Allocate buffers for environ pointers and data */
    int32_t *env_ptrs = (int32_t *)wasm_alloc(env_count * 4);
    char *env_buf = (char *)wasm_alloc(env_buf_size);
    if (!env_ptrs || !env_buf) return taida_lax_empty((int64_t)(intptr_t)"");

    err = __wasi_environ_get(env_ptrs, env_buf);
    if (err != 0) return taida_lax_empty((int64_t)(intptr_t)"");

    /* Search for matching key */
    for (int32_t i = 0; i < env_count; i++) {
        const char *entry = (const char *)(intptr_t)env_ptrs[i];
        if (!entry) continue;

        /* Check if entry starts with "name=" */
        int match = 1;
        for (int32_t j = 0; j < name_len; j++) {
            if (entry[j] != name[j]) { match = 0; break; }
        }
        if (match && entry[name_len] == '=') {
            /* Found: copy the value part */
            const char *val = entry + name_len + 1;
            int32_t val_len = wasi_strlen(val);
            char *copy = wasi_str_copy(val, val_len);
            if (!copy) return taida_lax_empty((int64_t)(intptr_t)"");
            return taida_lax_new((int64_t)(intptr_t)copy, (int64_t)(intptr_t)"");
        }
    }

    return taida_lax_empty((int64_t)(intptr_t)"");
}

/* ── allEnv() → HashMap[Str, Str] ── */

int64_t taida_os_all_env(void) {
    int64_t hm = taida_hashmap_new();
    taida_hashmap_set_value_tag(hm, WASM_TAG_STR);

    int32_t env_count = 0;
    int32_t env_buf_size = 0;
    int32_t err = __wasi_environ_sizes_get(&env_count, &env_buf_size);
    if (err != 0 || env_count == 0) return hm;

    int32_t *env_ptrs = (int32_t *)wasm_alloc(env_count * 4);
    char *env_buf = (char *)wasm_alloc(env_buf_size);
    if (!env_ptrs || !env_buf) return hm;

    err = __wasi_environ_get(env_ptrs, env_buf);
    if (err != 0) return hm;

    for (int32_t i = 0; i < env_count; i++) {
        const char *entry = (const char *)(intptr_t)env_ptrs[i];
        if (!entry) continue;

        /* Find '=' separator */
        const char *eq = entry;
        while (*eq && *eq != '=') eq++;
        if (*eq != '=') continue;

        int32_t key_len = (int32_t)(eq - entry);
        char *key = wasi_str_copy(entry, key_len);
        if (!key) continue;

        const char *val_start = eq + 1;
        int32_t val_len = wasi_strlen(val_start);
        char *val = wasi_str_copy(val_start, val_len);
        if (!val) continue;

        int64_t key_hash = taida_str_hash((int64_t)(intptr_t)key);
        hm = taida_hashmap_set(hm, key_hash, (int64_t)(intptr_t)key, (int64_t)(intptr_t)val);
    }

    return hm;
}

/* ── Read[path]() → Lax[Str] ── */

int64_t taida_os_read(int64_t path_ptr) {
    const char *path = (const char *)(intptr_t)path_ptr;
    if (!path) return taida_lax_empty((int64_t)(intptr_t)"");

    int32_t path_len = wasi_strlen(path);
    if (path_len == 0) return taida_lax_empty((int64_t)(intptr_t)"");

    wasi_resolved_path rp = resolve_preopened_path(path, path_len);

    /* Open for reading */
    wasi_fd file_fd = -1;
    int32_t err = __wasi_path_open(
        rp.fd,
        1,  /* dirflags: LOOKUPFLAGS_SYMLINK_FOLLOW */
        rp.rel_path,
        rp.rel_path_len,
        0,  /* oflags: none */
        WASI_RIGHT_FD_READ | WASI_RIGHT_FD_SEEK,
        0,  /* fs_rights_inheriting */
        0,  /* fdflags */
        &file_fd
    );
    if (err != 0) return taida_lax_empty((int64_t)(intptr_t)"");

    /* Get file size via fd_seek to end */
    int64_t file_size = 0;
    err = __wasi_fd_seek(file_fd, 0, WASI_SEEK_END, &file_size);
    if (err != 0 || file_size < 0) {
        __wasi_fd_close(file_fd);
        return taida_lax_empty((int64_t)(intptr_t)"");
    }

    /* Seek back to start */
    int64_t dummy;
    __wasi_fd_seek(file_fd, 0, WASI_SEEK_SET, &dummy);

    /* Limit: 64MB */
    if (file_size > 64 * 1024 * 1024) {
        __wasi_fd_close(file_fd);
        return taida_lax_empty((int64_t)(intptr_t)"");
    }

    int32_t size32 = (int32_t)file_size;

    /* Allocate buffer */
    char *buf = (char *)wasm_alloc(size32 + 1);
    if (!buf) {
        __wasi_fd_close(file_fd);
        return taida_lax_empty((int64_t)(intptr_t)"");
    }

    /* Read file contents */
    int32_t total_read = 0;
    while (total_read < size32) {
        wasi_iovec iov;
        iov.buf = (int32_t)(intptr_t)(buf + total_read);
        iov.len = size32 - total_read;
        int32_t nread = 0;
        err = __wasi_fd_read(file_fd, &iov, 1, &nread);
        if (err != 0 || nread == 0) break;
        total_read += nread;
    }
    buf[total_read] = '\0';

    __wasi_fd_close(file_fd);

    return taida_lax_new((int64_t)(intptr_t)buf, (int64_t)(intptr_t)"");
}

/* ── writeFile(path, content) → Result ── */

/* FNV-1a hashes matching native_runtime.c */
#define WASI_HASH_OK      0x08b05d07b5566befULL  /* FNV-1a("ok") */
#define WASI_HASH_CODE    0x0bb51791194b4414ULL  /* FNV-1a("code") */
#define WASI_HASH_MESSAGE 0x546401b5d2a8d2a4ULL  /* FNV-1a("message") */

static void wasi_register_builtin_io_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;

    taida_register_field_name(taida_str_hash((int64_t)(intptr_t)"type"), (int64_t)(intptr_t)"type");
    taida_register_field_name(WASI_HASH_OK, (int64_t)(intptr_t)"ok");
    taida_register_field_name(WASI_HASH_CODE, (int64_t)(intptr_t)"code");
    taida_register_field_name(WASI_HASH_MESSAGE, (int64_t)(intptr_t)"message");
    taida_register_field_name(taida_str_hash((int64_t)(intptr_t)"kind"), (int64_t)(intptr_t)"kind");
}

/* WASI errno → error kind mapping (matches native taida_os_error_kind) */
static const char *wasi_error_kind(int32_t wasi_errno, const char *msg) {
    switch (wasi_errno) {
        case 44: /* ENOENT */   return "not_found";
        case 28: /* EINVAL */   return "invalid";
        case 73: /* ETIMEDOUT */return "timeout";
        case 61: /* ECONNREFUSED */  return "refused";
        case 54: /* ECONNRESET */    return "reset";
        case 53: /* ECONNABORTED */
        case 64: /* EPIPE */
        case 57: /* ENOTCONN */      return "peer_closed";
        default: break;
    }
    /* Fallback: check message content like native */
    if (msg) {
        const char *m = msg;
        /* Simple substring check for "invalid" */
        while (*m) {
            if (*m == 'i' && m[1] == 'n' && m[2] == 'v') return "invalid";
            if (*m == 'n' && m[1] == 'o' && m[2] == 't' && m[3] == ' ' && m[4] == 'f') return "not_found";
            m++;
        }
    }
    return "other";
}

/* Build IoError pack — 4 fields matching native taida_make_io_error */
static int64_t wasi_make_io_error(int32_t wasi_errno, const char *msg) {
    wasi_register_builtin_io_field_names();

    const char *message = msg ? msg : "unknown io error";
    const char *kind = wasi_error_kind(wasi_errno, message);

    int64_t pack = taida_pack_new(4);
    /* type field */
    taida_pack_set_hash(pack, 0, taida_str_hash((int64_t)(intptr_t)"type"));
    char *type_copy = wasi_str_copy("IoError", 7);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)type_copy);
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    /* message field */
    taida_pack_set_hash(pack, 1, WASI_HASH_MESSAGE);
    char *msg_copy = wasi_str_copy(message, wasi_strlen(message));
    taida_pack_set(pack, 1, (int64_t)(intptr_t)(msg_copy ? msg_copy : message));
    taida_pack_set_tag(pack, 1, WASM_TAG_STR);
    /* code field */
    taida_pack_set_hash(pack, 2, WASI_HASH_CODE);
    taida_pack_set(pack, 2, (int64_t)wasi_errno);
    /* kind field */
    taida_pack_set_hash(pack, 3, taida_str_hash((int64_t)(intptr_t)"kind"));
    char *kind_copy = wasi_str_copy(kind, wasi_strlen(kind));
    taida_pack_set(pack, 3, (int64_t)(intptr_t)kind_copy);
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

/* Build os ok inner @(ok=true, code=0, message="") — matches Native */
static int64_t wasi_os_ok_inner(void) {
    wasi_register_builtin_io_field_names();

    int64_t inner = taida_pack_new(3);
    taida_pack_set_hash(inner, 0, WASI_HASH_OK);
    taida_pack_set(inner, 0, 1);  /* true */
    taida_pack_set_hash(inner, 1, WASI_HASH_CODE);
    taida_pack_set(inner, 1, 0);
    taida_pack_set_hash(inner, 2, WASI_HASH_MESSAGE);
    taida_pack_set(inner, 2, (int64_t)(intptr_t)"");
    return inner;
}

/* Build os Result success — matches Native taida_os_result_success.
 * Kept for compatibility; C12B-021 callers should prefer
 * wasi_os_result_success_value to embed a meaningful Int inner. */
static int64_t wasi_os_result_success(void) {
    return taida_result_create(wasi_os_ok_inner(), 0, 0);
}

/* C12B-021: Result[Int] success with an explicit integer inner value
 * (byte count for writeFile, entry count for remove, etc). The cast
 * preserves sign/width across i64 but keeps the shape identical to
 * Native's `taida_os_result_success(value)`. */
static int64_t wasi_os_result_success_value(int64_t value) {
    return taida_result_create(value, 0, 0);
}

/* C12B-021: Result[Bool] success — wraps a Bool inner value and
 * sets the proper runtime tag so `.toString()` / tag-dispatched
 * printing ("true" / "false") matches the Interpreter / Native
 * contract byte-for-byte. Used by Exists on wasi. */
#define WASM_TAG_BOOL 2
static int64_t wasi_os_result_success_bool(int64_t bool_val) {
    int64_t r = taida_result_create(bool_val ? 1 : 0, 0, 0);
    /* Result layout: index 0 = __value. Mark it Bool so downstream
     * polymorphic display prints "true"/"false" rather than "1"/"0". */
    taida_pack_set_tag(r, 0, WASM_TAG_BOOL);
    return r;
}

/* Build os Result failure — matches Native taida_os_result_failure shape */
static int64_t wasi_os_result_failure_code(int32_t wasi_errno, const char *msg) {
    wasi_register_builtin_io_field_names();

    const char *message = msg ? msg : "unknown io error";
    const char *kind = wasi_error_kind(wasi_errno, message);

    /* inner = @(ok=false, code=wasi_errno, message=msg, kind=kind) — 4 fields */
    int64_t inner = taida_pack_new(4);
    /* ok */
    taida_pack_set_hash(inner, 0, WASI_HASH_OK);
    taida_pack_set(inner, 0, 0);  /* false */
    /* code */
    taida_pack_set_hash(inner, 1, WASI_HASH_CODE);
    taida_pack_set(inner, 1, (int64_t)wasi_errno);
    /* message */
    taida_pack_set_hash(inner, 2, WASI_HASH_MESSAGE);
    char *msg_copy = wasi_str_copy(message, wasi_strlen(message));
    taida_pack_set(inner, 2, (int64_t)(intptr_t)(msg_copy ? msg_copy : message));
    taida_pack_set_tag(inner, 2, WASM_TAG_STR);
    /* kind */
    taida_pack_set_hash(inner, 3, taida_str_hash((int64_t)(intptr_t)"kind"));
    char *kind_copy = wasi_str_copy(kind, wasi_strlen(kind));
    taida_pack_set(inner, 3, (int64_t)(intptr_t)kind_copy);
    taida_pack_set_tag(inner, 3, WASM_TAG_STR);

    int64_t error = wasi_make_io_error(wasi_errno, message);
    return taida_result_create(inner, error, 0);
}

/* Convenience: failure with errno=0 (for validation errors without WASI errno) */
static int64_t wasi_os_result_failure(const char *msg) {
    return wasi_os_result_failure_code(0, msg);
}

int64_t taida_os_write_file(int64_t path_ptr, int64_t content_ptr) {
    const char *path = (const char *)(intptr_t)path_ptr;
    const char *content = (const char *)(intptr_t)content_ptr;

    if (!path || !content) return wasi_os_result_failure("writeFile: invalid arguments");

    int32_t path_len = wasi_strlen(path);
    if (path_len == 0) return wasi_os_result_failure("writeFile: empty path");

    wasi_resolved_path rp = resolve_preopened_path(path, path_len);

    /* Open for writing (create + truncate) */
    wasi_fd file_fd = -1;
    int32_t err = __wasi_path_open(
        rp.fd,
        1,  /* dirflags: LOOKUPFLAGS_SYMLINK_FOLLOW */
        rp.rel_path,
        rp.rel_path_len,
        WASI_O_CREAT | WASI_O_TRUNC,
        WASI_RIGHT_FD_WRITE,
        0,  /* fs_rights_inheriting */
        0,  /* fdflags */
        &file_fd
    );
    if (err != 0) return wasi_os_result_failure_code(err, "writeFile: failed to open file");

    /* Write content */
    int32_t content_len = wasi_strlen(content);
    int32_t total_written = 0;
    while (total_written < content_len) {
        wasi_ciovec iov;
        iov.buf = (int32_t)(intptr_t)(content + total_written);
        iov.len = content_len - total_written;
        int32_t nwritten = 0;
        err = __wasi_fd_write(file_fd, &iov, 1, &nwritten);
        if (err != 0 || nwritten == 0) {
            __wasi_fd_close(file_fd);
            return wasi_os_result_failure_code(err, "writeFile: write failed");
        }
        total_written += nwritten;
    }

    __wasi_fd_close(file_fd);
    /* C12B-021: writeFile returns Result[Int] where the inner Int is
     * the byte count that was written. Match Native / Interpreter /
     * JS parity exactly. */
    return wasi_os_result_success_value((int64_t)content_len);
}

/* ── Exists[path]() → Result[Bool] ── */
/* C12B-021: wrap the raw existence bit in a Result envelope so that
 * permission denials and IO errors are distinguishable from a
 * "probe succeeded, path absent" result. The inner value is the
 * raw Bool (0/1). */

int64_t taida_os_exists(int64_t path_ptr) {
    const char *path = (const char *)(intptr_t)path_ptr;
    if (!path) return wasi_os_result_failure("Exists: invalid arguments");

    int32_t path_len = wasi_strlen(path);
    if (path_len == 0) return wasi_os_result_failure("Exists: empty path");

    wasi_resolved_path rp = resolve_preopened_path(path, path_len);

    /* Try to open the file. If successful, it exists. */
    wasi_fd file_fd = -1;
    int32_t err = __wasi_path_open(
        rp.fd,
        1,  /* dirflags: LOOKUPFLAGS_SYMLINK_FOLLOW */
        rp.rel_path,
        rp.rel_path_len,
        0,  /* oflags: none */
        WASI_RIGHT_FD_READ,
        0,
        0,
        &file_fd
    );
    if (err != 0) {
        /* WASI errno 44 = ENOENT, 54 = ENOTDIR — these signal "does not
         * exist" rather than "probe failure", so we still answer
         * Result[success, false]. Any other errno is surfaced as a
         * Result[failure, ...] so callers can tell the difference. */
        if (err == 44 /* ENOENT */ || err == 54 /* ENOTDIR */) {
            return wasi_os_result_success_bool(0);
        }
        return wasi_os_result_failure_code(err, "Exists: probe failed");
    }

    __wasi_fd_close(file_fd);
    return wasi_os_result_success_bool(1);
}

/* ── C26B-020 柱 3: readBytesAt(path, offset, len) → Lax[Bytes] ──
 *
 * wasm-wasi / wasm-full lowering for the chunked-read API landed as
 * `readBytesAt` on Interpreter / JS / Native in Round 1 wD.  Semantics
 * mirror `src/codegen/native_runtime/os.c::taida_os_read_bytes_at`
 * and `src/interpreter/os_eval.rs::readBytesAt` byte-for-byte:
 *   - offset < 0 or len < 0        → Lax failure (default empty Bytes)
 *   - len == 0                     → Lax success (empty Bytes)
 *   - len > 64 MB chunk ceiling    → Lax failure
 *   - offset >= file size          → Lax success (empty Bytes)
 *   - offset + len > file size     → Lax success (truncated tail)
 *   - any IO / path-resolve error  → Lax failure
 *
 * Bytes representation here mirrors `runtime_full_wasm.c::taida_bytes_*`
 * (layout: [MAGIC, len, byte0, byte1, ...] with one i64 per byte).
 * The constructors below are `static` so that wasm-full, which links
 * both rt_wasi and rt_full, does not see duplicate symbols.  Any
 * Bytes value produced here is layout-compatible with rt_full's
 * `_wf_is_bytes()` / `taida_bytes_len()` and round-trips through
 * `Utf8Decode` / `.hasValue` / `@size` identically on wasm-full.
 */

/* Must match runtime_full_wasm.c::WF_BYTES_MAGIC exactly (layout
 * interop — wasm-full links both objects, so values must tag-compare
 * equal).  Static linkage means the symbol itself is file-local; only
 * the produced Bytes pointer is shared. */
#define WASI_BYTES_MAGIC 0x5441494442595400LL  /* "TAIDBYT\0" */

static int64_t wasi_bytes_default_value(void) {
    int64_t *bytes = (int64_t *)wasm_alloc(3 * 8);
    if (!bytes) return 0;
    bytes[0] = WASI_BYTES_MAGIC;
    bytes[1] = 0;  /* len */
    bytes[2] = 0;
    return (int64_t)(intptr_t)bytes;
}

static int64_t wasi_bytes_from_raw(const unsigned char *src, int64_t len) {
    if (len < 0) len = 0;
    /* +2 i64 header, +len i64 for one byte-per-slot (matches rt_full). */
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    if (!bytes) return wasi_bytes_default_value();
    bytes[0] = WASI_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = (int64_t)src[i];
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_os_read_bytes_at(int64_t path_ptr, int64_t offset, int64_t len) {
    const char *path = (const char *)(intptr_t)path_ptr;
    if (!path) return taida_lax_empty(wasi_bytes_default_value());
    if (offset < 0 || len < 0) return taida_lax_empty(wasi_bytes_default_value());
    /* 64 MB chunk ceiling — matches interpreter / native. */
    if (len > 64LL * 1024LL * 1024LL) return taida_lax_empty(wasi_bytes_default_value());
    if (len == 0) {
        /* Lax success with an empty Bytes value. */
        int64_t empty = wasi_bytes_from_raw((const unsigned char *)"", 0);
        return taida_lax_new(empty, wasi_bytes_default_value());
    }

    int32_t path_len = wasi_strlen(path);
    if (path_len == 0) return taida_lax_empty(wasi_bytes_default_value());

    wasi_resolved_path rp = resolve_preopened_path(path, path_len);

    wasi_fd file_fd = -1;
    int32_t err = __wasi_path_open(
        rp.fd,
        1,  /* dirflags: LOOKUPFLAGS_SYMLINK_FOLLOW */
        rp.rel_path,
        rp.rel_path_len,
        0,  /* oflags: none */
        WASI_RIGHT_FD_READ | WASI_RIGHT_FD_SEEK,
        0,
        0,
        &file_fd
    );
    if (err != 0) return taida_lax_empty(wasi_bytes_default_value());

    /* Seek to offset.  If the underlying fd does not support seek or
     * the offset is past EOF, fd_seek will set newoffset to a value
     * past the end; the subsequent fd_read will just return 0 bytes
     * and we surface an empty Lax success, matching the interpreter
     * "beyond-EOF → empty success" branch. */
    int64_t new_off = 0;
    err = __wasi_fd_seek(file_fd, offset, WASI_SEEK_SET, &new_off);
    if (err != 0) {
        __wasi_fd_close(file_fd);
        return taida_lax_empty(wasi_bytes_default_value());
    }

    /* Read up to `len` bytes.  Tolerate short reads at EOF: loop
     * fd_read until we've filled the buffer or EOF is reached. */
    unsigned char *buf = (unsigned char *)wasm_alloc((unsigned int)len);
    if (!buf) {
        __wasi_fd_close(file_fd);
        return taida_lax_empty(wasi_bytes_default_value());
    }

    int64_t filled = 0;
    int32_t io_err = 0;
    while (filled < len) {
        wasi_iovec iov;
        iov.buf = (int32_t)(intptr_t)(buf + filled);
        iov.len = (int32_t)(len - filled);
        int32_t nread = 0;
        int32_t r = __wasi_fd_read(file_fd, &iov, 1, &nread);
        if (r != 0) { io_err = r; break; }
        if (nread == 0) break;  /* EOF */
        filled += nread;
    }
    __wasi_fd_close(file_fd);

    if (io_err != 0) {
        return taida_lax_empty(wasi_bytes_default_value());
    }

    /* Even with filled < len (truncated tail or beyond-EOF), we return
     * Lax success with whatever bytes we managed to read.  This matches
     * the interpreter / native contract. */
    int64_t bytes = wasi_bytes_from_raw(buf, filled);
    return taida_lax_new(bytes, wasi_bytes_default_value());
}
