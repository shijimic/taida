/**
 * runtime_edge_host.c -- wasm-edge I/O layer
 *
 * WE-2: Host import-based I/O for env (env_get, env_get_all).
 *
 * This file is compiled and linked only for wasm-edge builds. It depends on
 * the shared runtime_core_wasm.c functions such as wasm_alloc, Lax helpers,
 * HashMap helpers, and string hashing.
 *
 * Host import module: taida_host
 * Target: Cloudflare Workers (JS glue provides host imports)
 *
 * The request-handler ABI lives in runtime_abi_web_wasm.c and is linked only
 * when handler mode or `taida-lang/abi` helpers need it.
 */

#include <stdint.h>

extern void *wasm_alloc(unsigned int size);
extern char *_wasm_str_alloc(unsigned int total); /* header-carrying */

/* Header-carrying string literal — the profile-runtime twin of the
   core runtime's WSTR. Every string entering the Taida value space
   must carry the magic word: identification is positive-only (no byte
   heuristics), so a bare C literal cast to int64_t is invisible to
   _wasm_is_string_ptr and renders as a pointer-valued integer. */
#define WSTR_MAGIC 0x5441494453545300LL /* "TAIDSTS\0" */
#define WSTR(str) \
    ({ \
        static const struct { \
            int64_t m; \
            char s[sizeof(str)]; \
        } _wstr_lit = {WSTR_MAGIC, str}; \
        (int64_t)(intptr_t)_wstr_lit.s; \
    })


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

extern int64_t taida_lax_new(int64_t value, int64_t default_value);
extern int64_t taida_lax_empty(int64_t default_value);
extern int64_t taida_hashmap_new(void);
extern int64_t taida_hashmap_set(int64_t hm, int64_t key_hash, int64_t key_ptr, int64_t value);
extern void taida_hashmap_set_value_tag(int64_t hm, int64_t tag);
extern int64_t taida_str_hash(int64_t str_ptr);

#define TAG_STR 2

__attribute__((import_module("taida_host"), import_name("env_get")))
extern int32_t taida_host_env_get(int32_t key_ptr, int32_t key_len,
                                   int32_t buf_ptr, int32_t buf_cap);

__attribute__((import_module("taida_host"), import_name("env_get_all")))
extern int32_t taida_host_env_get_all(int32_t buf_ptr, int32_t buf_cap);

int64_t taida_os_env_var(int64_t name_ptr) {
    const char *key = (const char *)(intptr_t)name_ptr;
    if (!key) {
        return taida_lax_empty(WSTR(""));
    }

    int32_t key_len = edge_strlen(key);
    int32_t buf_cap = 256;
    char *buf = (char *)_wasm_str_alloc(buf_cap + 1);
    int32_t actual = taida_host_env_get(
        (int32_t)(intptr_t)key, key_len,
        (int32_t)(intptr_t)buf, buf_cap
    );

    if (actual < 0) {
        return taida_lax_empty(WSTR(""));
    }

    if (actual > buf_cap) {
        buf_cap = actual;
        buf = (char *)_wasm_str_alloc(buf_cap + 1);
        actual = taida_host_env_get(
            (int32_t)(intptr_t)key, key_len,
            (int32_t)(intptr_t)buf, buf_cap
        );
    }

    // The host may report a different size during the retry. Never index
    // outside the supplied buffer, including on a missing-value sentinel.
    if (actual < 0 || actual > buf_cap) return taida_lax_empty(WSTR(""));
    buf[actual] = '\0';
    return taida_lax_new((int64_t)(intptr_t)buf, WSTR(""));
}

int64_t taida_os_all_env(void) {
    int64_t hm = taida_hashmap_new();
    taida_hashmap_set_value_tag(hm, TAG_STR);

    int32_t buf_cap = 4096;
    char *buf = (char *)_wasm_str_alloc(buf_cap + 1);
    int32_t actual = taida_host_env_get_all(
        (int32_t)(intptr_t)buf, buf_cap
    );

    if (actual <= 0) {
        return hm;
    }

    if (actual > buf_cap) {
        buf_cap = actual;
        buf = (char *)_wasm_str_alloc(buf_cap + 1);
        actual = taida_host_env_get_all(
            (int32_t)(intptr_t)buf, buf_cap
        );
    }

    if (actual < 0 || actual > buf_cap) return hm;
    int32_t pos = 0;
    while (pos < actual) {
        int32_t entry_start = pos;
        while (pos < actual && buf[pos] != '\0') pos++;
        int32_t entry_len = pos - entry_start;
        if (entry_len == 0) break;

        int32_t eq_pos = entry_start;
        while (eq_pos < pos && buf[eq_pos] != '=') eq_pos++;

        if (eq_pos < pos) {
            int32_t key_len = eq_pos - entry_start;
            char *key = (char *)_wasm_str_alloc(key_len + 1);
            edge_memcpy(key, buf + entry_start, key_len);
            key[key_len] = '\0';

            int32_t val_len = pos - (eq_pos + 1);
            char *val = (char *)_wasm_str_alloc(val_len + 1);
            edge_memcpy(val, buf + eq_pos + 1, val_len);
            val[val_len] = '\0';

            int64_t key_ptr = (int64_t)(intptr_t)key;
            int64_t val_ptr = (int64_t)(intptr_t)val;
            taida_hashmap_set(hm, taida_str_hash(key_ptr), key_ptr, val_ptr);
        }

        pos++;
    }

    return hm;
}

/* ── prelude: nowMs (wasm-edge) ──────────────────────────────────────────
 * F62B-014: wall clock through the WASI clock_time_get import. The
 * generated Workers glue implements it with Date.now(); any custom edge
 * host must provide the same import (realtime clock id 0, nanoseconds
 * written to out_ptr). */

__attribute__((import_module("wasi_snapshot_preview1"), import_name("clock_time_get")))
extern int32_t __wasi_clock_time_get(int32_t clock_id, int64_t precision, int64_t *out_ns);

int64_t taida_time_now_ms(void) {
    int64_t ns = 0;
    if (__wasi_clock_time_get(0, 1000000LL, &ns) != 0) {
        return 0;
    }
    return ns / 1000000LL;
}

/* ── crypto: randomBytes (wasm-edge) ─────────────────────────────────────
 * Bytes construction uses the shared [TAIDBYT, len, byte...] layout (one
 * int64_t per byte) that the core runtime understands everywhere since the
 * F62B-012 unification. The constructors below mirror runtime_wasi_io.c
 * (rt_wasi is not linked on wasm-edge, so the symbols do not collide); the
 * layout contract is pinned by 02_containers.inc.c `_looks_like_bytes`.
 *
 * Entropy comes from the WASI random_get import. The generated Workers
 * glue implements it with crypto.getRandomValues (a CSPRNG); a host that
 * cannot provide cryptographically strong entropy must reject the import
 * rather than substitute a weak source. */

#define EDGE_BYTES_MAGIC 0x5441494442595400LL /* "TAIDBYT\0" */

extern int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr);
extern int64_t taida_throw(int64_t error_val);

int64_t taida_bytes_default_value(void) {
    int64_t *bytes = (int64_t *)wasm_alloc(2 * 8);
    if (!bytes) return 0;
    bytes[0] = EDGE_BYTES_MAGIC;
    bytes[1] = 0;
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_from_raw(int64_t ptr, int64_t len) {
    if (len < 0) len = 0;
    const unsigned char *data = (const unsigned char *)(intptr_t)ptr;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    if (!bytes) return taida_bytes_default_value();
    bytes[0] = EDGE_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = data ? (int64_t)data[i] : 0;
    return (int64_t)(intptr_t)bytes;
}

/* random_get: (buf, buf_len) -> errno (0 == success) */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("random_get")))
extern int32_t __wasi_random_get(unsigned char *buf, int32_t buf_len);

#define TAIDA_EDGE_CRYPTO_MAX (256LL * 1024LL * 1024LL)

int64_t taida_crypto_random_bytes(int64_t n_val) {
    if (n_val < 0) {
        return taida_throw(taida_make_error(WSTR("CryptoError"), WSTR("randomBytes: count must be non-negative")));
    }
    if (n_val == 0) {
        return taida_bytes_from_raw(0, 0);
    }
    if (n_val > TAIDA_EDGE_CRYPTO_MAX) {
        return taida_throw(taida_make_error(WSTR("CryptoError"), WSTR("randomBytes: count exceeds 256 MiB limit")));
    }
    unsigned char *buf = (unsigned char *)_wasm_str_alloc((unsigned int)n_val);
    int64_t got = 0;
    while (got < n_val) {
        int32_t want = (int32_t)((n_val - got > 0x7fffffffLL) ? 0x7fffffff : (n_val - got));
        int32_t rc = __wasi_random_get(buf + got, want);
        if (rc != 0) {
            return taida_throw(taida_make_error(WSTR("CryptoError"), WSTR("randomBytes: WASI random_get failed")));
        }
        got += want;
    }
    return taida_bytes_from_raw((int64_t)(intptr_t)buf, n_val);
}
