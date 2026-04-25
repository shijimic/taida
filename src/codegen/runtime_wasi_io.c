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

/* =========================================================================
 * C27B-020 / C27B-021 (2026-04-25): wasm widening (addition only)
 *
 * The bytes mold group, bitwise/shift, and Float Div/Mod molds were
 * historically defined in `runtime_full_wasm.c` for wasm-full only.
 * They are migrated here so wasm-wasi (which links rt_core + rt_wasi
 * but not rt_full) can also use them. wasm-full keeps working because
 * it still links rt_wasi and pulls these symbols from here -- the
 * rt_full file no longer defines them (avoiding duplicate symbols).
 *
 * `runtime_core_wasm/` is FROZEN -- new wasm-side runtime functions
 * always live in rt_wasi or rt_full. This block follows that contract.
 *
 * Static helpers (`_wi_*`) are file-local and do not collide with the
 * `_wf_*` helpers in rt_full (which retain the same shape for the
 * subset of bytes-detection paths still used inside rt_full's
 * polymorphic helpers, but with separate names).
 * ========================================================================= */

/* Tag constants must match runtime_core_wasm/01_core.inc.c. The pre-
 * existing `WASM_TAG_STR 1` defined above is a historic mismatch with
 * core's value (3) and is only used by this file's allEnv path -- not
 * worth touching. New constants below use the correct core values. */
#define WASI_RT_TAG_INT     0
#define WASI_RT_TAG_FLOAT   1
#define WASI_RT_TAG_BOOL    2
#define WASI_RT_TAG_STR_REAL 3

/* Float bit-punning helpers (mirror runtime_full_wasm.c::_to_double /
 * core_wasm::_d2l).  Unboxed double <-> int64 bit-pattern. */
static double _wi_to_double(int64_t v) {
    /* Match core_wasm::_to_double: small ints widen, larger values are
     * already bitcast-encoded f64. Threshold mirrors core (-1048576..1048576). */
    if (v >= -1048576 && v <= 1048576) return (double)v;
    union { int64_t l; double d; } u; u.l = v; return u.d;
}
static int64_t _wi_d2l(double v) {
    union { int64_t l; double d; } u; u.d = v; return u.l;
}

/* Pointer / shape detectors (file-local clones of rt_full's `_wf_*`
 * helpers; identical semantics, separate names to avoid any chance of
 * future collision if rt_full ever exports them). */
static int _wi_is_valid_ptr(int64_t val, unsigned int min_bytes) {
    if (val <= 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr + min_bytes > mem_size) return 0;
    return 1;
}

static int _wi_is_bytes(int64_t val) {
    if (!_wi_is_valid_ptr(val, 16)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    return (p[0] & 0xFFFFFFFFFFFFFF00LL) == WASI_BYTES_MAGIC;
}

#define WASI_RT_LIST_ELEMS 4
#define WASI_RT_LIST_MAGIC 0x544149444C535400LL  /* "TAIDLST\0" */
#define WASI_RT_SET_MAGIC  0x5441494453455400LL  /* "TAIDSET\0" */

static int _wi_looks_like_list(int64_t ptr) {
    if (!_wi_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WASI_RT_LIST_MAGIC || data[3] == WASI_RT_SET_MAGIC;
}

static int _wi_looks_like_string(int64_t val) {
    if (val == 0) return 0;
    if (val < 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr >= mem_size) return 0;
    const char *s = (const char *)(intptr_t)val;
    if (s[0] == '\0') return 1;
    for (int i = 0; i < 8 && s[i]; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c < 0x20 && c != '\t' && c != '\n' && c != '\r') return 0;
    }
    return 1;
}

/* Externs needed for the new functions. taida_pack_new / set / set_hash /
 * set_tag / get_idx are already declared above. taida_list_new / push /
 * taida_str_new_copy / taida_str_alloc / taida_register_field_name need
 * to be added. */
extern int64_t taida_list_new(void);
extern int64_t taida_list_push(int64_t list_ptr, int64_t item);
extern int64_t taida_pack_get_idx(int64_t pack_ptr, int64_t index);
extern int64_t taida_str_alloc(int64_t len);
extern int64_t taida_str_new_copy(int64_t src);

/* ─── Bytes constructors (public, replaces rt_full versions) ─── */

int64_t taida_bytes_default_value(void) {
    return wasi_bytes_default_value();
}

int64_t taida_bytes_len(int64_t bytes_ptr) {
    if (!_wi_is_bytes(bytes_ptr)) return 0;
    return ((int64_t *)(intptr_t)bytes_ptr)[1];
}

int64_t taida_bytes_new_filled(int64_t len, int64_t fill) {
    if (len < 0) len = 0;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    bytes[0] = WASI_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = fill;
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_from_raw(int64_t ptr, int64_t len) {
    if (len < 0) len = 0;
    const unsigned char *data = (const unsigned char *)(intptr_t)ptr;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    bytes[0] = WASI_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = (int64_t)data[i];
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_clone(int64_t bytes_ptr) {
    if (!_wi_is_bytes(bytes_ptr)) return taida_bytes_default_value();
    int64_t *src = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = src[1];
    int64_t *dst = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    for (int64_t i = 0; i < 2 + len; i++) dst[i] = src[i];
    return (int64_t)(intptr_t)dst;
}

int64_t taida_bytes_get_lax(int64_t bytes_ptr, int64_t idx) {
    if (!_wi_is_bytes(bytes_ptr)) return taida_lax_empty(0);
    int64_t len = ((int64_t *)(intptr_t)bytes_ptr)[1];
    if (idx < 0 || idx >= len) return taida_lax_empty(0);
    int64_t val = ((int64_t *)(intptr_t)bytes_ptr)[2 + idx];
    return taida_lax_new(val, 0);
}

int64_t taida_bytes_set(int64_t bytes_ptr, int64_t idx, int64_t val) {
    if (!_wi_is_bytes(bytes_ptr)) return taida_lax_empty(taida_bytes_default_value());
    int64_t len = ((int64_t *)(intptr_t)bytes_ptr)[1];
    if (idx < 0 || idx >= len) return taida_lax_empty(taida_bytes_default_value());
    if (val < 0 || val > 255) return taida_lax_empty(taida_bytes_default_value());
    int64_t out = taida_bytes_clone(bytes_ptr);
    ((int64_t *)(intptr_t)out)[2 + idx] = val;
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_bytes_to_list(int64_t bytes_ptr) {
    int64_t list = taida_list_new();
    if (!_wi_is_bytes(bytes_ptr)) return list;
    int64_t *bytes = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = bytes[1];
    for (int64_t i = 0; i < len; i++) {
        list = taida_list_push(list, bytes[2 + i]);
    }
    return list;
}

/* Local string buffer for Bytes display */
typedef struct { char *buf; int len; int cap; } _wi_strbuf;
static int _wi_strlen(const char *s) {
    int n = 0; if (!s) return 0; while (s[n]) n++; return n;
}
static char *_wi_i64_to_str(int64_t val) {
    char tmp[24]; int len = 0; int neg = 0; uint64_t uval;
    if (val < 0) { neg = 1; uval = (uint64_t)(-(val + 1)) + 1; }
    else { uval = (uint64_t)val; }
    if (uval == 0) { tmp[len++] = '0'; }
    else { while (uval > 0) { tmp[len++] = '0' + (int)(uval % 10); uval /= 10; } }
    int total = neg + len;
    char *buf = (char *)wasm_alloc((unsigned int)(total + 1));
    int pos = 0;
    if (neg) buf[pos++] = '-';
    for (int i = len - 1; i >= 0; i--) buf[pos++] = tmp[i];
    buf[pos] = '\0';
    return buf;
}
static void _wi_sb_init(_wi_strbuf *sb) {
    sb->cap = 256; sb->buf = (char *)wasm_alloc((unsigned int)sb->cap); sb->len = 0;
    if (sb->buf) sb->buf[0] = '\0';
}
static void _wi_sb_append(_wi_strbuf *sb, const char *s) {
    int slen = _wi_strlen(s);
    if (sb->len + slen + 1 > sb->cap) {
        int new_cap = sb->cap;
        while (sb->len + slen + 1 > new_cap) new_cap *= 2;
        char *nb = (char *)wasm_alloc((unsigned int)new_cap);
        if (!nb) return;
        for (int i = 0; i < sb->len; i++) nb[i] = sb->buf[i];
        sb->buf = nb; sb->cap = new_cap;
    }
    for (int i = 0; i < slen; i++) sb->buf[sb->len + i] = s[i];
    sb->len += slen; sb->buf[sb->len] = '\0';
}

int64_t taida_bytes_to_display_string(int64_t bytes_ptr) {
    if (!_wi_is_bytes(bytes_ptr)) {
        return taida_str_new_copy((int64_t)(intptr_t)"Bytes()");
    }
    int64_t *bytes = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = bytes[1];
    _wi_strbuf jb;
    _wi_sb_init(&jb);
    _wi_sb_append(&jb, "Bytes([");
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) _wi_sb_append(&jb, ", ");
        char *s = _wi_i64_to_str(bytes[2 + i]);
        _wi_sb_append(&jb, s);
    }
    _wi_sb_append(&jb, "])");
    return (int64_t)(intptr_t)jb.buf;
}

int64_t taida_bytes_mold(int64_t value, int64_t fill) {
    if (_wi_is_bytes(value)) {
        int64_t cloned = taida_bytes_clone(value);
        return taida_lax_new(cloned, taida_bytes_default_value());
    }
    if (_wi_looks_like_list(value)) {
        int64_t *list = (int64_t *)(intptr_t)value;
        int64_t len = list[1];
        int64_t out = taida_bytes_new_filled(len, 0);
        int64_t *ob = (int64_t *)(intptr_t)out;
        for (int64_t i = 0; i < len; i++) {
            int64_t item = list[WASI_RT_LIST_ELEMS + i];
            if (item < 0 || item > 255) {
                return taida_lax_empty(taida_bytes_default_value());
            }
            ob[2 + i] = item;
        }
        return taida_lax_new(out, taida_bytes_default_value());
    }
    if (_wi_looks_like_string(value)) {
        const char *s = (const char *)(intptr_t)value;
        int slen = _wi_strlen(s);
        int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)s, (int64_t)slen);
        return taida_lax_new(out, taida_bytes_default_value());
    }
    int64_t len = value;
    if (len < 0 || len > 10000000) return taida_lax_empty(taida_bytes_default_value());
    if (fill < 0 || fill > 255) return taida_lax_empty(taida_bytes_default_value());
    int64_t out = taida_bytes_new_filled(len, (int64_t)(unsigned char)fill);
    return taida_lax_new(out, taida_bytes_default_value());
}

/* ─── Bytes cursor ─── */
/* Hash constants are FNV-1a of the actual user-facing field names. The
 * code generator's `simple_hash` (`src/codegen/lower/mod.rs`) hashes the
 * field name verbatim, so e.g. `cursor.bytes` hashes "bytes", not
 * "__bytes". rt_full's previous constants used "__value"-style hashes
 * which never matched the lowering -- the field accesses came back zero.
 * Using the matching names below keeps `step.value`, `step.cursor`,
 * `cursor.bytes`, etc. resolvable from generated wasm code. */
#define WASI_HASH_CURSOR_BYTES   0x2f2ec0474f1c4fe4ULL  /* FNV-1a("bytes") */
#define WASI_HASH_CURSOR_OFFSET  0x0268b0f8129435caULL  /* FNV-1a("offset") */
#define WASI_HASH_CURSOR_LENGTH  0xea11573f1af59eb5ULL  /* FNV-1a("length") */
#define WASI_HASH_STEP_VALUE     0x7ce4fd9430e80ceaULL  /* FNV-1a("value") */
#define WASI_HASH_STEP_CURSOR    0xf927453fbe6252efULL  /* FNV-1a("cursor") */

static uint64_t _wi_fnv1a(const char *s, int len) {
    uint64_t hash = 0xcbf29ce484222325ULL;
    for (int i = 0; i < len; i++) {
        hash ^= (unsigned char)s[i];
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

static int _wi_bytes_cursor_unpack(int64_t cursor_ptr, int64_t *bytes_out, int64_t *offset_out) {
    if (!_wi_is_valid_ptr(cursor_ptr, 8)) return 0;
    int64_t *pack = (int64_t *)(intptr_t)cursor_ptr;
    int64_t fc = pack[0];
    if (fc < 2) return 0;
    int64_t bytes_ptr = pack[1 + 0 * 3 + 2];
    int64_t offset = pack[1 + 1 * 3 + 2];
    if (!_wi_is_bytes(bytes_ptr)) return 0;
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    *bytes_out = bytes_ptr;
    *offset_out = offset;
    return 1;
}

static int64_t _wi_bytes_cursor_step(int64_t value, int64_t cursor) {
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, WASI_HASH_STEP_VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, WASI_HASH_STEP_CURSOR);
    taida_pack_set(pack, 1, cursor);
    return pack;
}

int64_t taida_bytes_cursor_new(int64_t bytes_ptr, int64_t offset) {
    if (!_wi_is_bytes(bytes_ptr)) bytes_ptr = taida_bytes_default_value();
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASI_HASH_CURSOR_BYTES);
    taida_pack_set(pack, 0, bytes_ptr);
    taida_pack_set_hash(pack, 1, WASI_HASH_CURSOR_OFFSET);
    taida_pack_set(pack, 1, offset);
    taida_pack_set_hash(pack, 2, WASI_HASH_CURSOR_LENGTH);
    taida_pack_set(pack, 2, len);
    uint64_t type_hash = _wi_fnv1a("__type", 6);
    taida_pack_set_hash(pack, 3, (int64_t)type_hash);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"BytesCursor");
    return pack;
}

int64_t taida_bytes_cursor_u8(int64_t cursor_ptr) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wi_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        int64_t empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        return taida_lax_empty(_wi_bytes_cursor_step(0, empty_cursor));
    }
    int64_t cur = taida_bytes_cursor_new(bytes_ptr, offset);
    int64_t def_step = _wi_bytes_cursor_step(0, cur);
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset >= len) return taida_lax_empty(def_step);
    int64_t val = ((int64_t *)(intptr_t)bytes_ptr)[2 + offset];
    int64_t next = taida_bytes_cursor_new(bytes_ptr, offset + 1);
    return taida_lax_new(_wi_bytes_cursor_step(val, next), def_step);
}

int64_t taida_bytes_cursor_take(int64_t cursor_ptr, int64_t size) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wi_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        int64_t empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        return taida_lax_empty(_wi_bytes_cursor_step(taida_bytes_default_value(), empty_cursor));
    }
    int64_t cur = taida_bytes_cursor_new(bytes_ptr, offset);
    int64_t def_step = _wi_bytes_cursor_step(taida_bytes_default_value(), cur);
    if (size < 0) return taida_lax_empty(def_step);
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset + size > len) return taida_lax_empty(def_step);
    int64_t *src = (int64_t *)(intptr_t)bytes_ptr;
    int64_t out = taida_bytes_new_filled(size, 0);
    int64_t *dst = (int64_t *)(intptr_t)out;
    for (int64_t i = 0; i < size; i++) dst[2 + i] = src[2 + offset + i];
    int64_t next = taida_bytes_cursor_new(bytes_ptr, offset + size);
    return taida_lax_new(_wi_bytes_cursor_step(out, next), def_step);
}

int64_t taida_bytes_cursor_step(int64_t cursor_ptr, int64_t n) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wi_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        return taida_bytes_cursor_new(taida_bytes_default_value(), 0);
    }
    return taida_bytes_cursor_new(bytes_ptr, offset + n);
}

int64_t taida_bytes_cursor_remaining(int64_t cursor_ptr) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wi_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
    return taida_bytes_len(bytes_ptr) - offset;
}

int64_t taida_bytes_cursor_unpack(int64_t cursor_ptr, int64_t schema) {
    (void)schema;
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wi_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
    return bytes_ptr;
}

/* ─── u16/u32 LE/BE encode/decode molds ─── */

int64_t taida_u16be_mold(int64_t value) {
    if (value < 0 || value > 65535) return taida_lax_empty(taida_bytes_default_value());
    unsigned char raw[2];
    uint16_t n = (uint16_t)value;
    raw[0] = (unsigned char)((n >> 8) & 0xFF);
    raw[1] = (unsigned char)(n & 0xFF);
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)raw, 2);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_u16le_mold(int64_t value) {
    if (value < 0 || value > 65535) return taida_lax_empty(taida_bytes_default_value());
    unsigned char raw[2];
    uint16_t n = (uint16_t)value;
    raw[0] = (unsigned char)(n & 0xFF);
    raw[1] = (unsigned char)((n >> 8) & 0xFF);
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)raw, 2);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_u32be_mold(int64_t value) {
    if (value < 0 || value > 4294967295LL) return taida_lax_empty(taida_bytes_default_value());
    unsigned char raw[4];
    uint32_t n = (uint32_t)value;
    raw[0] = (unsigned char)((n >> 24) & 0xFF);
    raw[1] = (unsigned char)((n >> 16) & 0xFF);
    raw[2] = (unsigned char)((n >> 8) & 0xFF);
    raw[3] = (unsigned char)(n & 0xFF);
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)raw, 4);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_u32le_mold(int64_t value) {
    if (value < 0 || value > 4294967295LL) return taida_lax_empty(taida_bytes_default_value());
    unsigned char raw[4];
    uint32_t n = (uint32_t)value;
    raw[0] = (unsigned char)(n & 0xFF);
    raw[1] = (unsigned char)((n >> 8) & 0xFF);
    raw[2] = (unsigned char)((n >> 16) & 0xFF);
    raw[3] = (unsigned char)((n >> 24) & 0xFF);
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)raw, 4);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_u16be_decode_mold(int64_t value) {
    if (!_wi_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 2) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint16_t n = (uint16_t)(((unsigned)b[2] << 8) | (unsigned)b[3]);
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u16le_decode_mold(int64_t value) {
    if (!_wi_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 2) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint16_t n = (uint16_t)((unsigned)b[2] | ((unsigned)b[3] << 8));
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u32be_decode_mold(int64_t value) {
    if (!_wi_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 4) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint32_t n = ((uint32_t)b[2] << 24) | ((uint32_t)b[3] << 16) | ((uint32_t)b[4] << 8) | (uint32_t)b[5];
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u32le_decode_mold(int64_t value) {
    if (!_wi_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 4) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint32_t n = (uint32_t)b[2] | ((uint32_t)b[3] << 8) | ((uint32_t)b[4] << 16) | ((uint32_t)b[5] << 24);
    return taida_lax_new((int64_t)n, 0);
}

static int64_t _wi_strtol(const char *s, const char **end) {
    if (!s) { if (end) *end = s; return 0; }
    int64_t result = 0;
    int neg = 0;
    const char *p = s;
    if (*p == '-') { neg = 1; p++; }
    else if (*p == '+') { p++; }
    if (*p < '0' || *p > '9') { if (end) *end = s; return 0; }
    while (*p >= '0' && *p <= '9') {
        result = result * 10 + (*p - '0');
        p++;
    }
    if (end) *end = p;
    return neg ? -result : result;
}

int64_t taida_uint8_mold(int64_t v) {
    int64_t parsed = v;
    if (_wi_looks_like_string(v)) {
        const char *s = (const char *)(intptr_t)v;
        const char *cend;
        parsed = _wi_strtol(s, &cend);
        if (*cend != '\0') parsed = v;
    }
    if (parsed < 0 || parsed > 255) return taida_lax_empty(0);
    return taida_lax_new(parsed, 0);
}

int64_t taida_uint8_mold_float(int64_t v) {
    double d = _wi_to_double(v);
    if (d != d) return taida_lax_empty(0); /* NaN */
    if (d < 0.0 || d > 255.0) return taida_lax_empty(0);
    double fl = (double)(int64_t)d;
    if (fl != d) return taida_lax_empty(0);
    return taida_lax_new((int64_t)d, 0);
}

/* ─── UTF-8 encode/decode molds ─── */

static int _wi_utf8_decode_one(const unsigned char *buf, int len, int *consumed, uint32_t *out_cp) {
    if (len == 0) return 0;
    unsigned char b0 = buf[0];
    if (b0 < 0x80) { *consumed = 1; *out_cp = (uint32_t)b0; return 1; }
    if (b0 >= 0xC2 && b0 <= 0xDF) {
        if (len < 2) return 0;
        unsigned char b1 = buf[1];
        if ((b1 & 0xC0) != 0x80) return 0;
        *consumed = 2;
        *out_cp = ((uint32_t)(b0 & 0x1F) << 6) | (uint32_t)(b1 & 0x3F);
        return 1;
    }
    if (b0 >= 0xE0 && b0 <= 0xEF) {
        if (len < 3) return 0;
        unsigned char b1 = buf[1], b2 = buf[2];
        if ((b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80) return 0;
        if (b0 == 0xE0 && b1 < 0xA0) return 0;
        if (b0 == 0xED && b1 >= 0xA0) return 0;
        uint32_t cp = ((uint32_t)(b0 & 0x0F) << 12) | ((uint32_t)(b1 & 0x3F) << 6) | (uint32_t)(b2 & 0x3F);
        if (cp >= 0xD800 && cp <= 0xDFFF) return 0;
        *consumed = 3; *out_cp = cp;
        return 1;
    }
    if (b0 >= 0xF0 && b0 <= 0xF4) {
        if (len < 4) return 0;
        unsigned char b1 = buf[1], b2 = buf[2], b3 = buf[3];
        if ((b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 || (b3 & 0xC0) != 0x80) return 0;
        if (b0 == 0xF0 && b1 < 0x90) return 0;
        if (b0 == 0xF4 && b1 > 0x8F) return 0;
        uint32_t cp = ((uint32_t)(b0 & 0x07) << 18) | ((uint32_t)(b1 & 0x3F) << 12) | ((uint32_t)(b2 & 0x3F) << 6) | (uint32_t)(b3 & 0x3F);
        if (cp > 0x10FFFF) return 0;
        *consumed = 4; *out_cp = cp;
        return 1;
    }
    return 0;
}

int64_t taida_utf8_encode_mold(int64_t value) {
    const char *s = (const char *)(intptr_t)value;
    if (!s || !_wi_looks_like_string(value)) {
        return taida_lax_empty(taida_bytes_default_value());
    }
    int slen = _wi_strlen(s);
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)s, (int64_t)slen);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_utf8_decode_mold(int64_t value) {
    if (!_wi_is_bytes(value)) return taida_lax_empty(taida_str_alloc(0));
    int64_t *bytes = (int64_t *)(intptr_t)value;
    int64_t len = bytes[1];
    unsigned char *raw = (unsigned char *)wasm_alloc((unsigned int)len);
    for (int64_t i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];
    int pos = 0;
    while (pos < (int)len) {
        int consumed = 0;
        uint32_t cp = 0;
        if (!_wi_utf8_decode_one(raw + pos, (int)len - pos, &consumed, &cp)) {
            return taida_lax_empty(taida_str_alloc(0));
        }
        pos += consumed;
    }
    char *out = (char *)wasm_alloc((unsigned int)(len + 1));
    for (int64_t i = 0; i < len; i++) out[i] = (char)raw[i];
    out[len] = '\0';
    return taida_lax_new((int64_t)(intptr_t)out, taida_str_alloc(0));
}

int64_t taida_utf8_encode_scalar(int64_t v) {
    /* Encode a single Unicode scalar (codepoint) into a Bytes value. */
    uint32_t cp = (uint32_t)v;
    unsigned char raw[4]; int n = 0;
    if (cp < 0x80) { raw[0] = (unsigned char)cp; n = 1; }
    else if (cp < 0x800) {
        raw[0] = (unsigned char)(0xC0 | (cp >> 6));
        raw[1] = (unsigned char)(0x80 | (cp & 0x3F));
        n = 2;
    } else if (cp < 0x10000) {
        if (cp >= 0xD800 && cp <= 0xDFFF) return taida_lax_empty(taida_bytes_default_value());
        raw[0] = (unsigned char)(0xE0 | (cp >> 12));
        raw[1] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        raw[2] = (unsigned char)(0x80 | (cp & 0x3F));
        n = 3;
    } else if (cp <= 0x10FFFF) {
        raw[0] = (unsigned char)(0xF0 | (cp >> 18));
        raw[1] = (unsigned char)(0x80 | ((cp >> 12) & 0x3F));
        raw[2] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        raw[3] = (unsigned char)(0x80 | (cp & 0x3F));
        n = 4;
    } else {
        return taida_lax_empty(taida_bytes_default_value());
    }
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)raw, n);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_utf8_decode_one(int64_t v) {
    /* Decode the first scalar from a Bytes value, returning Lax[Int (codepoint)]. */
    if (!_wi_is_bytes(v)) return taida_lax_empty(0);
    int64_t *bytes = (int64_t *)(intptr_t)v;
    int64_t len = bytes[1];
    unsigned char raw[4];
    int avail = (int)(len < 4 ? len : 4);
    for (int i = 0; i < avail; i++) raw[i] = (unsigned char)bytes[2 + i];
    int consumed = 0;
    uint32_t cp = 0;
    if (!_wi_utf8_decode_one(raw, avail, &consumed, &cp)) return taida_lax_empty(0);
    return taida_lax_new((int64_t)cp, 0);
}

int64_t taida_utf8_single_scalar(int64_t v) {
    /* Decode a Bytes value that must contain exactly one UTF-8 scalar. */
    if (!_wi_is_bytes(v)) return taida_lax_empty(0);
    int64_t *bytes = (int64_t *)(intptr_t)v;
    int64_t len = bytes[1];
    if (len < 1 || len > 4) return taida_lax_empty(0);
    unsigned char raw[4];
    for (int64_t i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];
    int consumed = 0;
    uint32_t cp = 0;
    if (!_wi_utf8_decode_one(raw, (int)len, &consumed, &cp)) return taida_lax_empty(0);
    if (consumed != (int)len) return taida_lax_empty(0);
    return taida_lax_new((int64_t)cp, 0);
}

/* =========================================================================
 * C27B-021 (2026-04-25): Bitwise / Shift / Float Div+Mod molds
 *
 * `taida_bit_*` / `taida_shift_*`: pure 64-bit ops, identical to rt_full's
 *   previous versions.  Migrated here so wasm-wasi can use them.
 *
 * `taida_div_mold_f` / `taida_mod_mold_f` (NEW): C26B-011 semantics --
 *   Float div/mod via boxed-double bitcast.  zero divisor -> Lax empty
 *   tagged FLOAT (matches native runtime exactly: see
 *   src/codegen/native_runtime/core.c::taida_div_mold_f).
 *
 *   The wasm runtime represents Lax as a 4-field pack (hasValue, __value,
 *   __default, __type).  Tagging __value/__default with WASM_TAG_FLOAT (=1)
 *   makes `_wasm_pack_to_string_full` interpret them as f64 bit-patterns
 *   when stringifying -- mirrors `_float_lax_new` in
 *   `runtime_core_wasm/02_containers.inc.c`.
 * ========================================================================= */

int64_t taida_bit_and(int64_t a, int64_t b) { return (int64_t)(((uint64_t)a) & ((uint64_t)b)); }
int64_t taida_bit_or(int64_t a, int64_t b) { return (int64_t)(((uint64_t)a) | ((uint64_t)b)); }
int64_t taida_bit_xor(int64_t a, int64_t b) { return (int64_t)(((uint64_t)a) ^ ((uint64_t)b)); }
int64_t taida_bit_not(int64_t x) { return (int64_t)(~((uint64_t)x)); }

int64_t taida_shift_l(int64_t x, int64_t n) {
    if (n < 0 || n > 63) return taida_lax_empty(0);
    uint64_t shifted = ((uint64_t)x) << (unsigned int)n;
    return taida_lax_new((int64_t)shifted, 0);
}

int64_t taida_shift_r(int64_t x, int64_t n) {
    if (n < 0 || n > 63) return taida_lax_empty(0);
    int64_t shifted = ((int64_t)x) >> (unsigned int)n;
    return taida_lax_new(shifted, 0);
}

int64_t taida_shift_ru(int64_t x, int64_t n) {
    if (n < 0 || n > 63) return taida_lax_empty(0);
    uint64_t shifted = ((uint64_t)x) >> (unsigned int)n;
    return taida_lax_new((int64_t)shifted, 0);
}

/* C26B-011 semantics: Lax with __value/__default tagged FLOAT. */
static int64_t _wi_float_lax_new(int64_t bits) {
    int64_t lax = taida_lax_new(bits, _wi_d2l(0.0));
    taida_pack_set_tag(lax, 1, WASI_RT_TAG_FLOAT);
    taida_pack_set_tag(lax, 2, WASI_RT_TAG_FLOAT);
    return lax;
}

static int64_t _wi_float_lax_empty(void) {
    int64_t lax = taida_lax_empty(_wi_d2l(0.0));
    taida_pack_set_tag(lax, 1, WASI_RT_TAG_FLOAT);
    taida_pack_set_tag(lax, 2, WASI_RT_TAG_FLOAT);
    return lax;
}

/* int64 ABI: a_bits, b_bits are bit-patterns of f64 (or small ints
 * widened by the lowering via `taida_int_to_float`).  Returns Lax pack
 * with FLOAT-tagged __value/__default. */
int64_t taida_div_mold_f(int64_t a_bits, int64_t b_bits) {
    double a = _wi_to_double(a_bits);
    double b = _wi_to_double(b_bits);
    if (b == 0.0) return _wi_float_lax_empty();
    return _wi_float_lax_new(_wi_d2l(a / b));
}

/* C27B-021 wD review fix (Critical 1): exact IEEE-754 fmod.
 *
 * The previous single-pass `(int64_t)q; a - (double)qi * b` lost
 * precision in two ways:
 *   1. `(int64_t)q` saturates when |q| > INT64_MAX (i64.trunc_sat_f64_s
 *      on wasm), so for |a/b| > 9.22e18 the quotient is clamped and the
 *      subtraction yields a silent wrong answer. Empirically
 *      `Mod[1.0e20, 7.0]()` returned `0` instead of `2.0`.
 *   2. Even when |q| < 2^53, `(double)qi * b` rounds to f64 once the
 *      product no longer fits in 53 bits of mantissa, so a single-pass
 *      subtraction loses the last digits. Empirically
 *      `Mod[1000000.5, 3.14159265]()` returned `0.14357849990483373`
 *      instead of `0.14357849993359384`.
 *
 * Replacement is the textbook "scale-and-subtract" fmod (see e.g.
 * MUSL libc): preserve the sign of `a`, compute `|a| - n*|b|` by
 * repeatedly scaling |b| up to the largest power-of-two multiple
 * that still does not exceed |a|, subtracting, and halving back.
 * Each iteration is exact in f64 (subtraction of like-exponent
 * numbers loses no bits below the leading bit), and the loop
 * terminates after at most ~ceil(log2(|a/b|)) iterations.
 *
 * Special cases (mirrors interpreter `f64::%` on Float / native libm
 * fmod):
 *   - b == 0.0          -> Lax empty (existing C26B-011 semantics)
 *   - a is NaN / Inf    -> Lax empty (cannot produce a meaningful
 *                         finite remainder; matches C26B-011's
 *                         "Lax-safe over silent NaN propagation")
 *   - b is NaN / Inf    -> Lax empty
 *   - |a| < |b|         -> result == a   (preserves sign, including
 *                         signed zero)
 */
static int _wi_is_finite(double x) {
    /* True iff x is neither NaN nor +/-Inf. NaN: x != x. Inf: |x| ==
     * +Inf, detected by checking the f64 exponent field is not all-1s. */
    union { double d; int64_t l; } u; u.d = x;
    int64_t exp = (u.l >> 52) & 0x7FF;
    return exp != 0x7FF;
}

int64_t taida_mod_mold_f(int64_t a_bits, int64_t b_bits) {
    double a = _wi_to_double(a_bits);
    double b = _wi_to_double(b_bits);
    if (b == 0.0) return _wi_float_lax_empty();
    if (!_wi_is_finite(a) || !_wi_is_finite(b)) return _wi_float_lax_empty();
    /* Preserve sign of `a` (IEEE 754 fmod sign rule). */
    int sign_a = (a < 0.0) || (a == 0.0 && _wi_d2l(a) < 0);
    double aa = sign_a ? -a : a;
    double bb = (b < 0.0) ? -b : b;
    if (aa < bb) {
        /* |a| < |b|: remainder is a itself (preserves signed zero). */
        return _wi_float_lax_new(_wi_d2l(a));
    }
    /* Scale bb upward by powers of 2 until 2*bb > aa. Bound the loop
     * by the f64 exponent range (1075 = 1023 bias + 52 mantissa) so a
     * pathological input cannot spin forever. */
    double scaled = bb;
    int up = 0;
    while (scaled <= aa * 0.5 && up < 1075) {
        scaled *= 2.0;
        up++;
    }
    /* Subtract `scaled` (or `scaled/2`, etc.) whenever it fits.  Each
     * subtraction is exact because aa and scaled are aligned at this
     * point (scaled <= aa < 2*scaled). */
    while (up >= 0) {
        if (aa >= scaled) {
            aa -= scaled;
        }
        scaled *= 0.5;
        up--;
    }
    /* aa now holds |a| mod |b| in [0, |b|).  Reapply sign of a. */
    double result = sign_a ? -aa : aa;
    return _wi_float_lax_new(_wi_d2l(result));
}

/* =========================================================================
 * C27B-020 (2026-04-25): Bytes-aware polymorphic length override
 *
 * `runtime_core_wasm/01_core.inc.c::taida_polymorphic_length` only knows
 * list and string. Bytes pointers fall into the string branch and call
 * `wasm_strlen` on `WASI_BYTES_MAGIC = 0x5441494442595400LL`, whose
 * little-endian low byte is `0x00` -- strlen returns 0 immediately,
 * producing the silent-0 wrong answer the blocker describes.
 *
 * This override is exposed for both wasm-wasi and wasm-full; the
 * generated C `#define`s `taida_polymorphic_length` to this name when
 * the profile is Wasi or Full (see `emit_wasm_c.rs::emit_c`).
 * Core (FROZEN) is not modified.
 * ========================================================================= */

extern int64_t taida_polymorphic_length(int64_t ptr);

int64_t taida_polymorphic_length_bytes_aware(int64_t ptr) {
    if (!ptr) return 0;
    if (_wi_is_bytes(ptr)) {
        return taida_bytes_len(ptr);
    }
    return taida_polymorphic_length(ptr);
}
