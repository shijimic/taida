/**
 * runtime_edge_host.c -- wasm-edge I/O layer
 *
 * WE-2: Host import-based I/O for env (env_get, env_get_all).
 *
 * This file is compiled and linked ONLY for wasm-edge builds.
 * It depends on functions from runtime_core_wasm.c (e.g., wasm_alloc,
 * taida_lax_new, taida_lax_empty, taida_hashmap_new, taida_hashmap_set,
 * taida_hashmap_set_value_tag, taida_str_hash).
 *
 * runtime_core_wasm.c is FROZEN -- we do not modify it.
 * All wasm-edge-specific code lives here.
 *
 * Host import module: taida_host
 * Target: Cloudflare Workers (JS glue provides host imports)
 *
 * Note: stdout/stderr use the same fd_write import as wasm-min/wasm-wasi.
 * The JS glue provides wasi_snapshot_preview1.fd_write which bridges to
 * console.log/console.error. This means runtime_core_wasm.c's
 * taida_io_stdout/taida_io_stderr work unchanged.
 */

#include <stdint.h>

/* -- External declarations from runtime_core_wasm.c -- */

/* Bump allocator (defined in runtime_core_wasm.c, non-static for sharing) */
extern void *wasm_alloc(unsigned int size);

/* String utilities */
static int32_t edge_strlen(const char *s) {
    int32_t n = 0;
    while (s[n]) n++;
    return n;
}

static void edge_memcpy(void *dest, const void *src, int32_t n) {
    char *d = (char *)dest;
    const char *s = (const char *)src;
    while (n-- > 0) *d++ = *s++;
}

/* Lax constructor/empty (defined in runtime_core_wasm.c) */
extern int64_t taida_lax_new(int64_t value, int64_t default_value);
extern int64_t taida_lax_empty(int64_t default_value);

/* HashMap (for allEnv) */
extern int64_t taida_hashmap_new(void);
extern int64_t taida_hashmap_set(int64_t hm, int64_t key_hash, int64_t key_ptr, int64_t value);
extern void taida_hashmap_set_value_tag(int64_t hm, int64_t tag);
extern int64_t taida_str_hash(int64_t str_ptr);

/* Type tags (must match runtime_core_wasm.c) */
#define TAG_STR 2

/* -- Host imports from taida_host module -- */

/**
 * env_get: Read an environment variable by key.
 *
 * @param key_ptr  Pointer to UTF-8 key in linear memory
 * @param key_len  Length of key in bytes
 * @param buf_ptr  Pointer to output buffer in linear memory
 * @param buf_cap  Capacity of output buffer in bytes
 * @return         Actual length of value, or 0 if key not found.
 *                 If return > buf_cap, caller should retry with larger buffer.
 */
__attribute__((import_module("taida_host"), import_name("env_get")))
extern int32_t taida_host_env_get(int32_t key_ptr, int32_t key_len,
                                   int32_t buf_ptr, int32_t buf_cap);

/**
 * env_get_all: Read all environment variables.
 *
 * Format: "KEY=VALUE\0KEY=VALUE\0\0" (NUL-separated, double-NUL terminated)
 *
 * @param buf_ptr  Pointer to output buffer in linear memory
 * @param buf_cap  Capacity of output buffer in bytes
 * @return         Actual length written, or 0 if no env vars.
 *                 If return > buf_cap, caller should retry with larger buffer.
 */
__attribute__((import_module("taida_host"), import_name("env_get_all")))
extern int32_t taida_host_env_get_all(int32_t buf_ptr, int32_t buf_cap);

/* -- Taida API implementations -- */

/**
 * taida_os_env_var: Get a single environment variable.
 * Returns Lax[Str] -- has_value if found, empty if not.
 */
int64_t taida_os_env_var(int64_t name_ptr) {
    const char *key = (const char *)(intptr_t)name_ptr;
    if (!key) {
        return taida_lax_empty((int64_t)(intptr_t)"");
    }

    int32_t key_len = edge_strlen(key);

    /* First attempt with a reasonable buffer */
    int32_t buf_cap = 256;
    char *buf = (char *)wasm_alloc(buf_cap);
    int32_t actual = taida_host_env_get(
        (int32_t)(intptr_t)key, key_len,
        (int32_t)(intptr_t)buf, buf_cap
    );

    if (actual == 0) {
        /* Key not found */
        return taida_lax_empty((int64_t)(intptr_t)"");
    }

    if (actual > buf_cap) {
        /* Buffer too small, retry with exact size */
        buf_cap = actual;
        buf = (char *)wasm_alloc(buf_cap + 1);
        actual = taida_host_env_get(
            (int32_t)(intptr_t)key, key_len,
            (int32_t)(intptr_t)buf, buf_cap
        );
    }

    /* NUL-terminate the value */
    buf[actual] = '\0';

    /* Create a Taida string (NUL-terminated in linear memory) */
    int64_t str_val = (int64_t)(intptr_t)buf;
    return taida_lax_new(str_val, (int64_t)(intptr_t)"");
}

/**
 * taida_os_all_env: Get all environment variables as HashMap[Str, Str].
 */
int64_t taida_os_all_env(void) {
    int64_t hm = taida_hashmap_new();
    taida_hashmap_set_value_tag(hm, TAG_STR);

    /* First attempt with a reasonable buffer */
    int32_t buf_cap = 4096;
    char *buf = (char *)wasm_alloc(buf_cap);
    int32_t actual = taida_host_env_get_all(
        (int32_t)(intptr_t)buf, buf_cap
    );

    if (actual == 0) {
        return hm;
    }

    if (actual > buf_cap) {
        /* Buffer too small, retry */
        buf_cap = actual;
        buf = (char *)wasm_alloc(buf_cap + 1);
        actual = taida_host_env_get_all(
            (int32_t)(intptr_t)buf, buf_cap
        );
    }

    /* Parse "KEY=VALUE\0KEY=VALUE\0\0" format */
    int32_t pos = 0;
    while (pos < actual) {
        /* Find end of this entry (NUL terminator) */
        int32_t entry_start = pos;
        while (pos < actual && buf[pos] != '\0') pos++;
        int32_t entry_len = pos - entry_start;
        if (entry_len == 0) break;  /* double-NUL = end */

        /* Find '=' separator */
        int32_t eq_pos = entry_start;
        while (eq_pos < pos && buf[eq_pos] != '=') eq_pos++;

        if (eq_pos < pos) {
            /* Allocate and copy key */
            int32_t key_len = eq_pos - entry_start;
            char *key = (char *)wasm_alloc(key_len + 1);
            edge_memcpy(key, buf + entry_start, key_len);
            key[key_len] = '\0';

            /* Allocate and copy value */
            int32_t val_len = pos - (eq_pos + 1);
            char *val = (char *)wasm_alloc(val_len + 1);
            edge_memcpy(val, buf + eq_pos + 1, val_len);
            val[val_len] = '\0';

            int64_t key_ptr = (int64_t)(intptr_t)key;
            int64_t val_ptr = (int64_t)(intptr_t)val;
            int64_t key_hash = taida_str_hash(key_ptr);
            taida_hashmap_set(hm, key_hash, key_ptr, val_ptr);
        }

        pos++;  /* Skip NUL */
    }

    return hm;
}
