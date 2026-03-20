/**
 * runtime_core_wasm.c — wasm-min 最小ランタイム (runtime_core 相当)
 *
 * W-2a: Runtime 境界の分類
 * ==========================
 *
 * native_runtime.c (8000+ 行) の全ランタイム関数は以下の 4 カテゴリに分類される:
 *
 *   runtime_core:        I/O (stdout/stderr/debug), 整数演算, ブール演算,
 *                         Div/Mod mold, poly_eq/neq, retain/release (no-op)
 *   runtime_collections: BuchiPack, List, HashMap, Set, String 操作,
 *                         Lax, Result, Gorillax, Molten, Stub, Todo, Cage, JSON
 *   runtime_os:          ファイルI/O, プロセス, 環境変数, CLI 引数
 *   runtime_async:       Async[T], spawn, cancel, all, race, map
 *
 * wasm-min は runtime_core のみをリンクする。
 * native_runtime.c をそのまま wasm に持ち込まない。
 * wasm-min v1 の許可機能: static string print / int print / integer arithmetic のみ。
 *
 * 本ファイル (runtime_core_wasm.c) が wasm-min の全 runtime である。
 * wasm-ld --gc-sections + --strip-all により、未使用関数は .wasm に含まれない。
 * これにより hello_world で RC / collection / OS / async コードが一切混入しないことが
 * 構造的に保証される。
 *
 * WASI fd_write ベースの stdout のみ。ヒープアロケーション禁止。
 * RC / retain / release / collection runtime を持ち込まない。
 *
 * 全値は int64_t (boxed value) として統一。文字列は NUL 終端ポインタを
 * int64_t にキャストして保持する（Native backend と同一表現）。
 * static string literal は heap header を持たない borrowed data として扱う。
 * wasm data section に直接配置され、malloc/free/RC を必要としない。
 *
 * サポートする機能:
 *   - taida_io_stdout(val)    : 文字列ポインタ → stdout + 改行
 *   - taida_io_stderr(val)    : 文字列ポインタ → stderr + 改行
 *   - taida_debug_int(val)    : i64 の 10 進文字列化 + stdout
 *   - taida_debug_str(val)    : debug(Str) — 文字列ポインタ → stdout + 改行
 *   - taida_debug_bool(val)   : debug(Bool) — "true"/"false" + 改行
 *   - taida_int_add/sub/mul   : 整数演算
 *   - taida_int_neg            : 符号反転
 *   - taida_int_eq/neq/lt/gt/gte : 整数比較
 *   - taida_bool_and/or/not   : ブール演算
 *   - taida_div_mold/mod_mold : 除算/剰余（簡易版、ゼロ除算は 0 を返す）
 *   - taida_generic_unmold    : identity（Lax ラッパーなし）
 *   - taida_poly_eq/neq       : 多態比較（整数比較のみ）
 *   - taida_float_add/sub/mul   : Float 演算 (boxed float as int64_t via bitcast)
 *   - taida_float_neg           : Float 符号反転
 *   - taida_debug_float         : debug(Float) — f64 の文字列化 + stdout
 *   - taida_int_to_float        : Int→Float 変換 (bitcast to int64_t)
 *   - taida_float_to_int        : Float→Int 変換 (truncate toward zero)
 *   - taida_retain/taida_release : no-op (wasm-min ではヒープなし)
 *   - _start                  : WASI エントリポイント (_taida_main を呼び出す)
 */

#include <stdint.h>

// WCR-2: Minimum heap address — values below this are small integers, not pointers.
// Used by _wasm_is_valid_ptr and type detection heuristics.
#define WASM_MIN_HEAP_ADDR 4096

/* ── WASI fd_write import ── */

typedef int32_t wasi_fd;

/* iovec: pointer(i32) + length(i32) */
typedef struct {
    int32_t buf;    /* pointer to data (wasm linear memory offset) */
    int32_t len;    /* length in bytes */
} __attribute__((packed)) wasi_ciovec;

/* WASI fd_write: (fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno */
__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")))
extern int32_t __wasi_fd_write(wasi_fd fd, const wasi_ciovec *iovs,
                               int32_t iovs_len, int32_t *nwritten);

/* ── helper: write buffer to stdout ── */

static void write_stdout(const char *buf, int32_t len) {
    wasi_ciovec iov;
    iov.buf = (int32_t)(intptr_t)buf;
    iov.len = len;
    int32_t nwritten;
    __wasi_fd_write(1, &iov, 1, &nwritten);
}

/* ── libc stubs (no libc in freestanding wasm) ── */
/* clang may emit calls to memcpy/memset even for manual loops at -O2.
   Provide minimal implementations for the WASM freestanding environment. */

void *memcpy(void *dest, const void *src, unsigned long n) {
    char *d = (char *)dest;
    const char *s2 = (const char *)src;
    while (n--) *d++ = *s2++;
    return dest;
}

void *memset(void *dest, int c, unsigned long n) {
    char *d = (char *)dest;
    while (n--) *d++ = (char)c;
    return dest;
}

/* ── W-3: Bump allocator (WASM linear memory) ── */
/* Simple bump allocator that never frees. Suitable for wasm-min's
   short-lived programs where memory pressure is minimal.
   Uses __builtin_wasm_memory_size and __builtin_wasm_memory_grow
   to manage WASM linear memory pages (64KB each). */

/* WW-2: bump_ptr and wasm_alloc are non-static so runtime_wasi_io.c can
   share the same allocator. For wasm-min (which doesn't link runtime_wasi_io.c),
   this change has zero behavioral effect. */
unsigned int bump_ptr = 0;  /* 0 = uninitialized */

void *wasm_alloc(unsigned int size) {
    /* Align to 8 bytes */
    size = (size + 7) & ~7u;

    if (bump_ptr == 0) {
        /* Initialize: start after stack/data. Use __heap_base linker symbol. */
        extern unsigned int __heap_base;
        bump_ptr = (unsigned int)(unsigned long)&__heap_base;
        /* Align to 8 bytes */
        bump_ptr = (bump_ptr + 7) & ~7u;
    }

    unsigned int result = bump_ptr;
    bump_ptr += size;

    /* Check if we need to grow memory */
    unsigned int pages_needed = (bump_ptr + 65535) / 65536;
    unsigned int current_pages = __builtin_wasm_memory_size(0);
    if (pages_needed > current_pages) {
        int grew = __builtin_wasm_memory_grow(0, pages_needed - current_pages);
        if (grew == -1) {
            /* Out of memory — return NULL (will crash on use) */
            return (void *)0;
        }
    }

    return (void *)(unsigned long)result;
}

/* ── strlen (no libc) ── */

int32_t wasm_strlen(const char *s) {
    int32_t n = 0;
    while (s[n]) n++;
    return n;
}

/* ── WC-1a: String helper functions (shared by all profiles) ── */
/* These helpers are used by string molds, query functions, etc.
   They duplicate no-libc functionality for the freestanding WASM environment.
   _wf_strlen returns int (not int32_t) matching the full runtime convention. */

static int _wf_strlen(const char *s) {
    if (!s) return 0;
    int len = 0;
    while (s[len]) len++;
    return len;
}

static void _wf_memcpy(void *dst, const void *src, int len) {
    char *d = (char *)dst;
    const char *s = (const char *)src;
    for (int i = 0; i < len; i++) d[i] = s[i];
}

static int _wf_strncmp(const char *a, const char *b, int n) {
    for (int i = 0; i < n; i++) {
        if (a[i] != b[i]) return (unsigned char)a[i] - (unsigned char)b[i];
        if (a[i] == '\0') return 0;
    }
    return 0;
}

static int _wf_strcmp(const char *a, const char *b) {
    while (*a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}

/// Find first occurrence of needle in haystack, or NULL.
static const char *_wf_strstr(const char *haystack, const char *needle) {
    if (!haystack || !needle) return (const char *)0;
    int nlen = _wf_strlen(needle);
    if (nlen == 0) return haystack;
    int hlen = _wf_strlen(haystack);
    if (nlen > hlen) return (const char *)0;
    for (int i = 0; i <= hlen - nlen; i++) {
        if (_wf_strncmp(haystack + i, needle, nlen) == 0)
            return haystack + i;
    }
    return (const char *)0;
}

static int _wf_is_whitespace(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

/* ── taida_io_stdout: stdout 出力（boxed 文字列ポインタ） ── */

int64_t taida_io_stdout(int64_t val_ptr) {
    const char *s = (const char *)(intptr_t)val_ptr;
    if (s) {
        int32_t len = wasm_strlen(s);
        write_stdout(s, len);
        write_stdout("\n", 1);
    }
    return 0;
}

/* ── taida_io_stderr: stderr 出力（wasm-min では stdout にフォールバック） ── */

int64_t taida_io_stderr(int64_t val_ptr) {
    const char *s = (const char *)(intptr_t)val_ptr;
    if (s) {
        int32_t len = wasm_strlen(s);
        /* stderr = fd 2 */
        wasi_ciovec iov;
        iov.buf = (int32_t)(intptr_t)s;
        iov.len = len;
        int32_t nwritten;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        iov.buf = (int32_t)(intptr_t)"\n";
        iov.len = 1;
        __wasi_fd_write(2, &iov, 1, &nwritten);
    }
    return 0;
}

/* ── taida_debug_int: i64 の 10 進出力 ── */

int64_t taida_debug_int(int64_t val) {
    /* stack-local buffer で i64 → 10 進文字列化 */
    char buf[21]; /* -9223372036854775808 = 20 chars + NUL */
    int pos = 20;
    buf[pos] = '\0';

    int negative = 0;
    uint64_t uval;
    if (val < 0) {
        negative = 1;
        uval = (uint64_t)(-(val + 1)) + 1;
    } else {
        uval = (uint64_t)val;
    }

    if (uval == 0) {
        buf[--pos] = '0';
    } else {
        while (uval > 0) {
            buf[--pos] = '0' + (char)(uval % 10);
            uval /= 10;
        }
    }

    if (negative) {
        buf[--pos] = '-';
    }

    int32_t len = 20 - pos;
    write_stdout(buf + pos, len);
    write_stdout("\n", 1);
    return 0;
}

/* ── taida_debug_str: debug(Str) — 文字列の出力 ── */

int64_t taida_debug_str(int64_t val_ptr) {
    const char *s = (const char *)(intptr_t)val_ptr;
    if (s) {
        int32_t len = wasm_strlen(s);
        write_stdout(s, len);
        write_stdout("\n", 1);
    }
    return 0;
}

/* ── taida_debug_polymorphic: forward declaration (impl after _wasm_value_to_display_string) ── */
int64_t taida_debug_polymorphic(int64_t val);

/* ── taida_debug_bool: debug(Bool) ── */

int64_t taida_debug_bool(int64_t val) {
    if (val) {
        write_stdout("true\n", 5);
    } else {
        write_stdout("false\n", 6);
    }
    return 0;
}

/* ── 整数演算 ── */

int64_t taida_int_add(int64_t a, int64_t b) { return a + b; }
int64_t taida_int_sub(int64_t a, int64_t b) { return a - b; }
int64_t taida_int_mul(int64_t a, int64_t b) { return a * b; }
int64_t taida_int_neg(int64_t a) { return -a; }

/* ── 整数比較 ── */

int64_t taida_int_eq(int64_t a, int64_t b) { return a == b ? 1 : 0; }
int64_t taida_int_neq(int64_t a, int64_t b) { return a != b ? 1 : 0; }
int64_t taida_int_lt(int64_t a, int64_t b) { return a < b ? 1 : 0; }
int64_t taida_int_gt(int64_t a, int64_t b) { return a > b ? 1 : 0; }
int64_t taida_int_gte(int64_t a, int64_t b) { return a >= b ? 1 : 0; }

/* ── ブール演算 ── */

int64_t taida_bool_and(int64_t a, int64_t b) { return (a && b) ? 1 : 0; }
int64_t taida_bool_or(int64_t a, int64_t b) { return (a || b) ? 1 : 0; }
int64_t taida_bool_not(int64_t a) { return a ? 0 : 1; }

/* ── Type tags (matching native_runtime.c TAIDA_TAG_* constants) ── */
#define WASM_TAG_INT     0
#define WASM_TAG_FLOAT   1
#define WASM_TAG_BOOL    2
#define WASM_TAG_STR     3
#define WASM_TAG_PACK    4

/* ── Forward declarations for Lax/Result/Gorillax (defined in W-5 section below) ── */
int64_t taida_lax_new(int64_t value, int64_t default_value);
int64_t taida_lax_empty(int64_t default_value);
int64_t taida_lax_unmold(int64_t lax_ptr);
static int _wasm_is_lax(int64_t val);
static int _wasm_is_result(int64_t val);
static int _wasm_is_gorillax(int64_t val);
int64_t taida_pack_get_idx(int64_t pack_ptr, int64_t index);
int64_t taida_pack_set_tag(int64_t pack_ptr, int64_t index, int64_t tag);
int64_t taida_pack_get_tag(int64_t pack_ptr, int64_t index);
int64_t taida_pack_new(int64_t field_count);
int64_t taida_pack_get(int64_t pack_ptr, int64_t field_hash);
int64_t taida_pack_has_hash(int64_t pack_ptr, int64_t field_hash);
int64_t taida_throw(int64_t error_val);
int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr);
int64_t taida_can_throw_payload(int64_t val);
int64_t taida_int_to_str(int64_t a);      /* NTH-5: forward decl for poly_add */
int64_t taida_str_concat(int64_t a_ptr, int64_t b_ptr);  /* NTH-5: forward decl for poly_add */
static int64_t _wasm_invoke_callback1(int64_t fn_ptr, int64_t arg0);
static int64_t _wasm_result_is_error_check(int64_t result);
static int _wasm_is_valid_ptr(int64_t val, unsigned int min_bytes);  /* NTH-4: forward decl */
static int _wasm_is_async_obj(int64_t val);   /* PR-4: forward decl for async */
int64_t taida_async_unmold(int64_t async_ptr);              /* PR-4: forward decl */
int64_t taida_async_map(int64_t async_ptr, int64_t fn_ptr); /* PR-4: forward decl */
int64_t taida_async_get_or_default(int64_t async_ptr, int64_t fallback); /* PR-4 */
static int64_t _wasm_async_to_string(int64_t async_ptr);    /* PR-4: forward decl */

/* ── Hash constants needed by taida_generic_unmold (full definitions in W-5) ── */
#define WASM_HASH___TYPE      0x84d2d84b631f799bLL  /* FNV-1a("__type") */
#define WASM_HASH___VALUE     0x0a7fc9f13472bbe0LL  /* FNV-1a("__value") */
#define WASM_HASH___DEFAULT   0xed4fba440f8602d4LL  /* FNV-1a("__default") */
#define WASM_HASH_TODO_SOL    0x824fa3195cf2e6c1LL  /* FNV-1a("sol") */
#define WASM_HASH_TODO_UNM    0x4cadac193e198b15LL  /* FNV-1a("unm") */

/* ── Div/Mod mold — W-5: now returns Lax (matching native backend) ── */

int64_t taida_div_mold(int64_t a, int64_t b) {
    if (b == 0) return taida_lax_empty(0);
    return taida_lax_new(a / b, 0);
}

int64_t taida_mod_mold(int64_t a, int64_t b) {
    if (b == 0) return taida_lax_empty(0);
    return taida_lax_new(a % b, 0);
}

/* ── generic_unmold — W-5g: Lax/Result/Gorillax-aware, predicate-evaluated ── */

int64_t taida_generic_unmold(int64_t val) {
    /* NTH-4: Guard against non-pointer values (negative integers, small ints, etc.)
       that would cause OOB when dereferenced as pack pointers.
       _wasm_is_valid_ptr already handles this for is_lax/result/gorillax, but
       taida_pack_has_hash (called later) does not. Guard early. */
    if (!_wasm_is_valid_ptr(val, 8)) return val;
    /* PR-4: Async unmold — check before other types */
    if (_wasm_is_async_obj(val)) {
        return taida_async_unmold(val);
    }
    if (_wasm_is_lax(val)) {
        return taida_lax_unmold(val);
    }
    if (_wasm_is_result(val)) {
        /* Result unmold: evaluate predicate + check throw (matching native) */
        int64_t value = taida_pack_get_idx(val, 0);      /* __value */
        int64_t pred = taida_pack_get_idx(val, 1);        /* __predicate */
        int64_t throw_val = taida_pack_get_idx(val, 2);   /* throw */

        if (throw_val != 0) {
            if (pred != 0) {
                int64_t pred_result = _wasm_invoke_callback1(pred, value);
                if (!pred_result) {
                    /* Predicate failed — throw the error */
                    if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
                    int64_t error = taida_make_error(
                        (int64_t)(intptr_t)"ResultError",
                        (int64_t)(intptr_t)"Result predicate failed");
                    return taida_throw(error);
                }
                /* Predicate passed even with throw set — return value */
                return value;
            }
            /* No predicate, throw is set — throw */
            if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
            int64_t error = taida_make_error(
                (int64_t)(intptr_t)"ResultError",
                (int64_t)(intptr_t)"Result error");
            return taida_throw(error);
        }

        /* Evaluate predicate if present (no throw set) */
        if (pred != 0) {
            int64_t pred_result = _wasm_invoke_callback1(pred, value);
            if (pred_result) return value;  /* success */
            /* Predicate failed — throw default error */
            int64_t error = taida_make_error(
                (int64_t)(intptr_t)"ResultError",
                (int64_t)(intptr_t)"Result predicate failed");
            return taida_throw(error);
        }

        return value; /* no throw, no predicate — success */
    }
    if (_wasm_is_gorillax(val)) {
        /* Gorillax unmold: return __value if ok, throw otherwise */
        int64_t is_ok = taida_pack_get_idx(val, 0);
        if (is_ok) return taida_pack_get_idx(val, 1); /* __value */
        int64_t error_val = taida_pack_get_idx(val, 2); /* __error */
        if (taida_can_throw_payload(error_val)) return taida_throw(error_val);
        int64_t err = taida_make_error(
            (int64_t)(intptr_t)"GorillaxError",
            (int64_t)(intptr_t)"Gorillax error");
        return taida_throw(err);
    }
    /* BE-WASM-1: TODO unmold — return unm channel, fallback to sol/__default/__value.
       Matches native_runtime.c taida_generic_unmold TODO branch. */
    if (taida_pack_has_hash(val, WASM_HASH___TYPE)) {
        int64_t type_ptr = taida_pack_get(val, WASM_HASH___TYPE);
        /* Guard: ensure type_ptr looks like a valid pointer (> WASM_MIN_HEAP_ADDR) */
        if ((intptr_t)type_ptr <= WASM_MIN_HEAP_ADDR) return val;
        const char *type_str = (const char *)(intptr_t)type_ptr;
        if (type_str != 0 && type_str[0] == 'T' && type_str[1] == 'O' &&
            type_str[2] == 'D' && type_str[3] == 'O' && type_str[4] == '\0') {
            /* TODO pack: prefer unm > __default > sol > __value */
            if (taida_pack_has_hash(val, WASM_HASH_TODO_UNM))
                return taida_pack_get(val, WASM_HASH_TODO_UNM);
            if (taida_pack_has_hash(val, WASM_HASH___DEFAULT))
                return taida_pack_get(val, WASM_HASH___DEFAULT);
            if (taida_pack_has_hash(val, WASM_HASH_TODO_SOL))
                return taida_pack_get(val, WASM_HASH_TODO_SOL);
            if (taida_pack_has_hash(val, WASM_HASH___VALUE))
                return taida_pack_get(val, WASM_HASH___VALUE);
            return taida_pack_new(0);
        }
        /* Molten detection: cannot unmold Molten directly */
        if (type_str != 0 && type_str[0] == 'M' && type_str[1] == 'o' &&
            type_str[2] == 'l' && type_str[3] == 't' && type_str[4] == 'e' &&
            type_str[5] == 'n' && type_str[6] == '\0') {
            int64_t error = taida_make_error(
                (int64_t)(intptr_t)"TypeError",
                (int64_t)(intptr_t)"Cannot unmold Molten directly. Molten can only be used inside Cage.");
            return taida_throw(error);
        }
        /* Custom mold: pack with __type and __value fields */
        if (taida_pack_has_hash(val, WASM_HASH___VALUE))
            return taida_pack_get(val, WASM_HASH___VALUE);
    }
    return val;
}

/* ── FL-16 / NTH-5: 多態加算 (polymorphic add) — string-aware ── */
/* Heuristic string detection for wasm: a value is considered a string pointer if
   it lies within the wasm data segment (where static string literals reside) or
   within the dynamic heap (>= __heap_base, < bump_ptr).
   Small integer values (< 1024) are never treated as string pointers to avoid
   false positives from small numeric literals. */
static int _wasm_is_string_ptr(int64_t v) {
    if (v <= 1024 || v > 0xFFFFFFFFLL) return 0;
    unsigned int addr = (unsigned int)(uint64_t)v;
    /* Check within wasm linear memory bounds */
    unsigned int mem_bytes = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    if (addr >= mem_bytes) return 0;
    /* Require the address to be in a known region: either the data segment
       (static string literals, typically at low addresses before __heap_base)
       or the dynamic heap (between __heap_base and bump_ptr).
       We check: if it's a dynamically allocated object, it must be < bump_ptr. */
    extern unsigned int __heap_base;
    unsigned int heap_start = (unsigned int)(unsigned long)&__heap_base;
    if (addr >= heap_start && (bump_ptr == 0 || addr >= bump_ptr)) return 0;
    /* Finally, peek at the first byte: must be a printable ASCII char (0x20..0x7E).
       NUL (empty string at address) is excluded since it's indistinguishable from
       zeroed memory.  Integer values that happen to be valid addresses with a
       printable first byte will still false-positive, but with the > 1024 guard
       and heap range check, this is rare. */
    unsigned char first = *(const unsigned char *)(intptr_t)v;
    return first >= 0x20 && first <= 0x7E;
}

int64_t taida_poly_add(int64_t a, int64_t b) {
    int a_str = _wasm_is_string_ptr(a);
    int b_str = _wasm_is_string_ptr(b);
    if (a_str || b_str) {
        /* At least one operand is a string — concatenate.
           Convert non-string operand to its string representation. */
        int64_t sa = a_str ? a : taida_int_to_str(a);
        int64_t sb = b_str ? b : taida_int_to_str(b);
        return taida_str_concat(sa, sb);
    }
    return a + b;
}

/* ── 多態比較 (wasm-min: 整数比較のみ) ── */

int64_t taida_poly_eq(int64_t a, int64_t b) { return a == b ? 1 : 0; }
int64_t taida_poly_neq(int64_t a, int64_t b) { return a != b ? 1 : 0; }

/* ── W-3: String operations (dynamic allocation via bump allocator) ── */

int64_t taida_str_concat(int64_t a_ptr, int64_t b_ptr) {
    const char *a = (const char *)(intptr_t)a_ptr;
    const char *b = (const char *)(intptr_t)b_ptr;
    if (!a) a = "";
    if (!b) b = "";
    int32_t la = wasm_strlen(a);
    int32_t lb = wasm_strlen(b);
    char *buf = (char *)wasm_alloc(la + lb + 1);
    if (!buf) return 0;
    for (int32_t i = 0; i < la; i++) buf[i] = a[i];
    for (int32_t i = 0; i < lb; i++) buf[la + i] = b[i];
    buf[la + lb] = '\0';
    return (int64_t)(intptr_t)buf;
}

int64_t taida_str_length(int64_t s_ptr) {
    const char *s = (const char *)(intptr_t)s_ptr;
    if (!s) return 0;
    return (int64_t)wasm_strlen(s);
}

int64_t taida_str_eq(int64_t a_ptr, int64_t b_ptr) {
    const char *a = (const char *)(intptr_t)a_ptr;
    const char *b = (const char *)(intptr_t)b_ptr;
    if (a == b) return 1;
    if (!a || !b) return 0;
    while (*a && *b) {
        if (*a != *b) return 0;
        a++; b++;
    }
    return *a == *b ? 1 : 0;
}

int64_t taida_str_neq(int64_t a_ptr, int64_t b_ptr) {
    return taida_str_eq(a_ptr, b_ptr) ? 0 : 1;
}

/* ── W-3: Type conversions ── */

int64_t taida_int_to_str(int64_t a) {
    /* Same algorithm as taida_debug_int but returns a heap string */
    char tmp[21];
    int pos = 20;
    tmp[pos] = '\0';

    int negative = 0;
    uint64_t uval;
    if (a < 0) {
        negative = 1;
        uval = (uint64_t)(-(a + 1)) + 1;
    } else {
        uval = (uint64_t)a;
    }

    if (uval == 0) {
        tmp[--pos] = '0';
    } else {
        while (uval > 0) {
            tmp[--pos] = '0' + (char)(uval % 10);
            uval /= 10;
        }
    }
    if (negative) {
        tmp[--pos] = '-';
    }

    int32_t len = 20 - pos;
    char *buf = (char *)wasm_alloc(len + 1);
    if (!buf) return 0;
    for (int i = 0; i < len; i++) buf[i] = tmp[pos + i];
    buf[len] = '\0';
    return (int64_t)(intptr_t)buf;
}

/* NTH-4: Use uint64_t for accumulation to avoid signed overflow UB
   when parsing INT64_MIN ("-9223372036854775808"). */
int64_t taida_str_to_int(int64_t s_ptr) {
    const char *s = (const char *)(intptr_t)s_ptr;
    if (!s) return 0;
    uint64_t result = 0;
    int negative = 0;
    int i = 0;
    if (s[i] == '-') { negative = 1; i++; }
    else if (s[i] == '+') { i++; }
    while (s[i] >= '0' && s[i] <= '9') {
        result = result * 10 + (uint64_t)(s[i] - '0');
        i++;
    }
    /* For negative: -(uint64_t) is well-defined modular arithmetic,
       producing the correct two's complement representation. */
    return negative ? -(int64_t)result : (int64_t)result;
}

int64_t taida_str_from_bool(int64_t v) {
    /* Returns static string "true" or "false" — no alloc needed */
    return v ? (int64_t)(intptr_t)"true" : (int64_t)(intptr_t)"false";
}

/* ── W-3: Int methods ── */

int64_t taida_int_abs(int64_t a) { return a < 0 ? -a : a; }
int64_t taida_int_lte(int64_t a, int64_t b) { return a <= b ? 1 : 0; }

/* ── W-3: Float→Str (uses bump allocator) ── */

static int64_t taida_float_to_str_impl(double d);

/* ── W-3: f64 <-> i64 bitcast helpers ── */
/* Same representation as native backend: f64 bits stored in int64_t */

static double _l2d(int64_t v) {
    union { int64_t l; double d; } u;
    u.l = v;
    return u.d;
}

static int64_t _d2l(double v) {
    union { int64_t l; double d; } u;
    u.d = v;
    return u.l;
}

/* Smart conversion: if the bit pattern looks like a small integer, convert;
   otherwise treat as f64 bit pattern. Matches native runtime's _to_double(). */
static double _to_double(int64_t v) {
    if (v >= -1048576 && v <= 1048576) {
        return (double)v;
    }
    return _l2d(v);
}

/* ── W-3: Float 演算 (boxed float as int64_t) ── */

int64_t taida_float_add(int64_t a, int64_t b) { return _d2l(_to_double(a) + _to_double(b)); }
int64_t taida_float_sub(int64_t a, int64_t b) { return _d2l(_to_double(a) - _to_double(b)); }
int64_t taida_float_mul(int64_t a, int64_t b) { return _d2l(_to_double(a) * _to_double(b)); }
int64_t taida_float_neg(int64_t a) { return _d2l(-_to_double(a)); }

/* ── TF-1/TF-2: Rust f64::Display compatible float formatter ── */
/* Matches Rust's Display for f64: no scientific notation, integers get ".0",
   non-integers use minimum significant digits for exact round-trip.
   Replaces the previous %g-equivalent fmt_g. */

/* Helper: parse a decimal string back to double (freestanding strtod).
   Uses integer accumulation + final division to avoid cumulative
   floating-point errors from repeated factor *= 0.1 multiplication. */
static double _parse_double(const char *s) {
    int i = 0;
    int negative = 0;
    if (s[i] == '-') { negative = 1; i++; }
    /* Accumulate all digits as an integer mantissa, count decimal places */
    uint64_t mantissa = 0;
    int decimal_places = 0;
    int in_frac = 0;
    /* Integer part */
    while (s[i] >= '0' && s[i] <= '9') {
        mantissa = mantissa * 10 + (uint64_t)(s[i] - '0');
        i++;
    }
    /* Fractional part */
    if (s[i] == '.') {
        i++;
        in_frac = 1;
        while (s[i] >= '0' && s[i] <= '9') {
            mantissa = mantissa * 10 + (uint64_t)(s[i] - '0');
            decimal_places++;
            i++;
        }
    }
    /* Convert: result = mantissa / 10^decimal_places */
    double result = (double)mantissa;
    double divisor = 1.0;
    for (int j = 0; j < decimal_places; j++) divisor *= 10.0;
    result /= divisor;
    if (negative) result = -result;
    return result;
}

/* Helper: compute base-10 exponent and normalized mantissa for d > 0 */
static int _compute_exp10(double d) {
    int exp10 = 0;
    double norm = d;
    if (norm >= 10.0) {
        while (norm >= 10.0) { norm /= 10.0; exp10++; }
    } else if (norm < 1.0) {
        while (norm < 1.0) { norm *= 10.0; exp10--; }
    }
    return exp10;
}

/* Helper: power of 10 (freestanding, integer exponent) */
static double _pow10(int e) {
    double r = 1.0;
    double base = 10.0;
    int neg = 0;
    if (e < 0) { neg = 1; e = -e; }
    while (e > 0) {
        if (e & 1) r *= base;
        base *= base;
        e >>= 1;
    }
    return neg ? 1.0 / r : r;
}

/* Format d (positive) with `sig` significant digits in fixed notation.
   Uses digit-by-digit extraction to avoid large-integer precision loss.
   Returns length written to buf (not NUL-terminated). */
static int _fmt_fixed_sig(double d, int sig, char *buf, int bufsize) {
    int len = 0;
    int exp10 = _compute_exp10(d);

    /* Number of integer digits = exp10 + 1 */
    int int_digits = exp10 + 1;
    /* Number of decimal places = sig - int_digits */
    int decimal_places = sig - int_digits;
    if (decimal_places < 0) decimal_places = 0;

    /* Extract integer part */
    uint64_t ipart = (uint64_t)d;
    double frac = d - (double)ipart;

    /* Write integer part */
    char itmp[24];
    int ipos = 23;
    itmp[ipos] = '\0';
    if (ipart == 0) { itmp[--ipos] = '0'; }
    else { while (ipart > 0) { itmp[--ipos] = '0' + (char)(ipart % 10); ipart /= 10; } }
    for (int i = ipos; i < 23; i++) {
        if (len < bufsize) buf[len++] = itmp[i];
    }

    /* Fractional part: extract digit by digit */
    if (decimal_places > 0) {
        if (len < bufsize) buf[len++] = '.';
        int frac_start = len;
        for (int i = 0; i < decimal_places; i++) {
            frac *= 10.0;
            int digit = (int)frac;
            if (digit > 9) digit = 9;
            frac -= (double)digit;
            if (len < bufsize) buf[len++] = '0' + (char)digit;
        }
        /* Round: if remaining frac >= 0.5, round up last digit */
        if (frac >= 0.5 && len > frac_start) {
            int carry = 1;
            for (int i = len - 1; i >= frac_start && carry; i--) {
                int d2 = (buf[i] - '0') + carry;
                if (d2 >= 10) { buf[i] = '0'; carry = 1; }
                else { buf[i] = '0' + (char)d2; carry = 0; }
            }
            /* Carry into integer part */
            if (carry) {
                /* Need to carry into integer portion */
                for (int i = frac_start - 2; i >= 0 && carry; i--) {
                    if (buf[i] >= '0' && buf[i] <= '9') {
                        int d2 = (buf[i] - '0') + carry;
                        if (d2 >= 10) { buf[i] = '0'; carry = 1; }
                        else { buf[i] = '0' + (char)d2; carry = 0; }
                    }
                }
            }
        }
        /* Trim trailing zeros, but keep at least one digit after dot */
        while (len > frac_start + 1 && buf[len-1] == '0') len--;
        /* If only dot remains, remove it too */
        if (len == frac_start) len--;
    }
    return len;
}

static int fmt_g(double d, char *buf, int bufsize) {
    int len = 0;
    union { double d; uint64_t u; } ux;
    ux.d = d;
    int negative = (ux.u >> 63) != 0;

    /* Handle negative — extract sign, then work with positive value */
    if (negative) { buf[len++] = '-'; d = -d; }

    /* NaN check: NaN != NaN */
    if (d != d) { buf[len++]='N'; buf[len++]='a'; buf[len++]='N'; return len; }
    /* Infinity */
    if (d > 1e308) { buf[len++]='i'; buf[len++]='n'; buf[len++]='f'; return len; }

    /* Zero: always "0.0" (or "-0.0") — matching Rust */
    if (d == 0.0) {
        buf[len++] = '0'; buf[len++] = '.'; buf[len++] = '0';
        return len;
    }

    /* Integer check: if d == floor(d) and d < 1e18, format as "X.0" */
    {
        int64_t as_int = (int64_t)d;
        double back = (double)as_int;
        if (back == d && d < 1e18) {
            /* Format integer part */
            uint64_t uval = (uint64_t)d;
            char itmp[24];
            int ipos = 23;
            itmp[ipos] = '\0';
            if (uval == 0) { itmp[--ipos] = '0'; }
            else { while (uval > 0) { itmp[--ipos] = '0' + (char)(uval % 10); uval /= 10; } }
            for (int i = ipos; i < 23; i++) buf[len++] = itmp[i];
            buf[len++] = '.'; buf[len++] = '0';
            return len;
        }
    }

    /* Non-integer: find minimum significant digits that round-trip exactly.
       Try sig = 1..17. For each, format in fixed notation, parse back,
       and check if the result equals the original. */
    for (int sig = 1; sig <= 17; sig++) {
        char trial[80];
        int tlen = _fmt_fixed_sig(d, sig, trial, 79);
        trial[tlen] = '\0';
        /* Parse back */
        double roundtrip = _parse_double(negative ? trial : trial);
        if (roundtrip == d) {
            /* Copy trial to output (after the sign if negative) */
            for (int i = 0; i < tlen; i++) buf[len++] = trial[i];
            return len;
        }
    }

    /* Fallback: 17 significant digits */
    int flen = _fmt_fixed_sig(d, 17, buf + len, bufsize - len);
    len += flen;
    return len;
}

/* ── W-3: taida_debug_float: debug(Float) — f64 の文字列化 + stdout ── */

int64_t taida_debug_float(int64_t val) {
    double d = _l2d(val);
    char buf[64];
    int len = fmt_g(d, buf, 64);
    write_stdout(buf, len);
    write_stdout("\n", 1);
    return 0;
}

/* ── W-3: Int→Float / Float→Int 変換 ── */

int64_t taida_int_to_float(int64_t a) {
    return _d2l((double)a);
}

int64_t taida_float_to_int(int64_t a) {
    return (int64_t)_to_double(a);
}

/* ── W-3: taida_float_to_str: Float→Str 変換 (uses bump allocator) ── */

int64_t taida_float_to_str(int64_t val) {
    double d = _l2d(val);
    char tmp[64];
    int len = fmt_g(d, tmp, 64);

    char *buf = (char *)wasm_alloc(len + 1);
    if (!buf) return 0;
    for (int i = 0; i < len; i++) buf[i] = tmp[i];
    buf[len] = '\0';
    return (int64_t)(intptr_t)buf;
}

/* W-4: Forward declarations for list functions (used by polymorphic helpers) */
int64_t taida_list_length(int64_t list_ptr);

/* W-4f2: Collection type markers for polymorphic dispatch */
#define WASM_SET_MARKER_VAL  0x53455400LL  /* "SET\0" */
/* W-4f2: Collection type markers and layout constants */
#define WASM_HM_HEADER 4
#define WASM_HM_MARKER_VAL 0x484D4150LL  /* "HMAP" — distinguishes HashMap from List/Set */

/* W-4f2: List/Set element offset — header is [cap, len, elem_tag, type_marker] = 4 slots */
#define WASM_LIST_ELEMS 4

/* ── W-3f/W-4: taida_polymorphic_length (wasm-min: string + list) ── */

/* Helper: check if a pointer looks like a list (bump-allocated).
   Lists have [capacity(int64), length(int64), ...] where capacity >= 16.
   Strings have printable ASCII/UTF-8 as first byte.
   We check the first int64_t: if it's a reasonable capacity (8-65536),
   it's likely a list. */
static int _looks_like_list(int64_t ptr) {
    if (ptr == 0) return 0;
    if (ptr < 0 || ptr > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)ptr;
    /* Need at least 4 int64_t slots (header: cap, len, elem_tag, type_marker) */
    if (addr + 32 > mem_size) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    int64_t cap = data[0];
    int64_t len = data[1];
    /* Valid list: capacity is a reasonable power-of-2-ish number,
       length is non-negative and <= capacity */
    if (cap >= 8 && cap <= 65536 && len >= 0 && len <= cap) return 1;
    return 0;
}

/* W-4f2: Check if a pointer is a Set (has WASM_SET_MARKER_VAL at slot[3]) */
static int _is_wasm_set(int64_t ptr) {
    if (!_looks_like_list(ptr)) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WASM_SET_MARKER_VAL;
}

int64_t taida_polymorphic_length(int64_t ptr) {
    if (!ptr) return 0;
    /* W-4: Check if it's a list first */
    if (_looks_like_list(ptr)) {
        return taida_list_length(ptr);
    }
    /* Otherwise treat as string */
    const char *s = (const char *)(intptr_t)ptr;
    return (int64_t)wasm_strlen(s);
}

/* ── W-3f/W-4f2: taida_polymorphic_to_string (full collection support) ── */

/* Helper: check if a value looks like a valid string pointer in WASM linear memory.
   In wasm32, valid heap/data addresses are positive and within linear memory.
   We check that the pointer is in a reasonable range and points to a NUL-terminated
   byte sequence with printable or whitespace characters. */
static int _looks_like_string(int64_t val) {
    /* Zero is not a string (it's the integer 0 or null) */
    if (val == 0) return 0;
    /* Negative values or values > 32-bit range are not pointers on wasm32 */
    if (val < 0 || val > 0xFFFFFFFF) return 0;
    /* Check if it's within current WASM memory */
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr >= mem_size) return 0;
    /* Check if it starts with a printable/whitespace ASCII byte (not \0) */
    const char *s = (const char *)(intptr_t)val;
    if (s[0] == '\0') return 0;
    /* Verify first few bytes are valid UTF-8/ASCII (not random garbage) */
    for (int i = 0; i < 8 && s[i]; i++) {
        unsigned char c = (unsigned char)s[i];
        /* Accept printable ASCII, whitespace, and high bytes (UTF-8 continuation) */
        if (c < 0x20 && c != '\t' && c != '\n' && c != '\r') return 0;
    }
    return 1;
}

/* W-4f2: Check if a value looks like a BuchiPack.
   Pack layout: [field_count, field0_hash, field0_tag, field0_value, ...]
   field_count is typically small (1-50), and field hashes are large int64_t values. */
static int _looks_like_pack(int64_t val) {
    if (val == 0) return 0;
    if (val < 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    /* Need at least 1 int64_t (field_count) */
    if (addr + 8 > mem_size) return 0;
    int64_t *data = (int64_t *)(intptr_t)val;
    int64_t fc = data[0];
    /* Valid pack: field_count is small and positive */
    if (fc < 1 || fc > 100) return 0;
    /* Verify there's enough memory for the full pack */
    int64_t total_bytes = (1 + fc * 3) * 8;
    if (addr + (unsigned int)total_bytes > mem_size) return 0;
    /* Check that at least the first field hash is a non-zero large value
       (field hashes from FNV-1a are typically large numbers) */
    int64_t first_hash = data[1];
    if (first_hash == 0) return 0;
    return 1;
}

/* Forward declarations for toString helpers */
static int64_t _wasm_value_to_display_string(int64_t val);
static int64_t _wasm_value_to_debug_string(int64_t val);
static int _is_wasm_hashmap(int64_t ptr);
static const char *_wasm_lookup_field_name(int64_t hash);
static int64_t _wasm_lookup_field_type(int64_t hash);
/* W-5f: Monadic type hash constants (FNV-1a hashes of field names).
   Centralized here for use by both display_string and the runtime constructors below. */
/* WFX-2: corrected FNV-1a hashes (previous values were wrong, causing
   field access mismatch with compiler-generated hashes from simple_hash()) */
#define WASM_HASH_HAS_VALUE   0x9e9c6dc733414d60LL  /* FNV-1a("hasValue") */
#ifndef WASM_HASH___VALUE
#define WASM_HASH___VALUE     0x0a7fc9f13472bbe0LL  /* FNV-1a("__value") */
#endif
#ifndef WASM_HASH___TYPE
#define WASM_HASH___TYPE      0x84d2d84b631f799bLL  /* FNV-1a("__type") */
#endif
#define WASM_HASH_IS_OK       0x6550c1c5b98b56bfLL  /* FNV-1a("isOk") */
#define WASM_HASH___ERROR     0x15c3e6e41a99a6cbLL  /* FNV-1a("__error") */
#ifndef WASM_HASH___DEFAULT
#define WASM_HASH___DEFAULT   0xed4fba440f8602d4LL  /* FNV-1a("__default") */
#endif
#define WASM_HASH_THROW       0x5a5fe3720c9584cfLL  /* FNV-1a("throw") */
#define WASM_HASH___PREDICATE 0x15592af3c2291540LL  /* FNV-1a("__predicate") */
/* BE-WASM-1: TODO field hashes (matching native_runtime.c) */
#define WASM_HASH_TODO_ID     0x08b72e07b55c3ac0LL  /* FNV-1a("id") */
#define WASM_HASH_TODO_TASK   0xd9603bef07a9524cLL  /* FNV-1a("task") */
#ifndef WASM_HASH_TODO_SOL
#define WASM_HASH_TODO_SOL    0x824fa3195cf2e6c1LL  /* FNV-1a("sol") */
#endif
#ifndef WASM_HASH_TODO_UNM
#define WASM_HASH_TODO_UNM    0x4cadac193e198b15LL  /* FNV-1a("unm") */
#endif
/* Error field hashes (moved early for _wasm_is_error / _wasm_error_to_string) */
#define WASM_HASH_TYPE      0xa79439ef7bfa9c2dLL  /* FNV-1a("type") */
#define WASM_HASH_MESSAGE   0x546401b5d2a8d2a4LL  /* FNV-1a("message") */

/* W-4f2: Dynamic string buffer for building collection toString output */
typedef struct {
    char *buf;
    int len;
    int cap;
} _wasm_strbuf;

static void _sb_init(_wasm_strbuf *sb) {
    sb->cap = 128;
    sb->buf = (char *)wasm_alloc(sb->cap);
    sb->len = 0;
    if (sb->buf) sb->buf[0] = '\0';
}

static void _sb_ensure(_wasm_strbuf *sb, int needed) {
    if (sb->len + needed + 1 > sb->cap) {
        int new_cap = sb->cap;
        while (sb->len + needed + 1 > new_cap) new_cap *= 2;
        char *new_buf = (char *)wasm_alloc(new_cap);
        if (!new_buf) return;
        for (int i = 0; i < sb->len; i++) new_buf[i] = sb->buf[i];
        new_buf[sb->len] = '\0';
        sb->buf = new_buf;
        sb->cap = new_cap;
    }
}

static void _sb_append(_wasm_strbuf *sb, const char *s) {
    int slen = wasm_strlen(s);
    _sb_ensure(sb, slen);
    for (int i = 0; i < slen; i++) sb->buf[sb->len + i] = s[i];
    sb->len += slen;
    sb->buf[sb->len] = '\0';
}

static int64_t _sb_finish(_wasm_strbuf *sb) {
    return (int64_t)(intptr_t)sb->buf;
}

/* W-4f2: HashMap toString: HashMap({"key": value, ...}) */
/* Tombstone: hash == 1, key == 0 (same as native TAIDA_HASHMAP_TOMBSTONE_HASH) */
#define WASM_HM_TOMBSTONE_HASH 1
#define WASM_HM_SLOT_EMPTY(h, k)     ((h) == 0 && (k) == 0)
#define WASM_HM_SLOT_TOMBSTONE(h, k) ((h) == WASM_HM_TOMBSTONE_HASH && (k) == 0)
#define WASM_HM_SLOT_OCCUPIED(h, k)  (!WASM_HM_SLOT_EMPTY(h, k) && !WASM_HM_SLOT_TOMBSTONE(h, k))

static int64_t _wasm_hashmap_to_string(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "HashMap({");
    int64_t count = 0;
    for (int64_t i = 0; i < cap; i++) {
        int64_t sh = hm[WASM_HM_HEADER + i * 3];
        int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
        if (WASM_HM_SLOT_OCCUPIED(sh, sk)) {
            int64_t value = hm[WASM_HM_HEADER + i * 3 + 2];
            if (count > 0) _sb_append(&sb, ", ");
            int64_t key_str = _wasm_value_to_debug_string(sk);
            int64_t val_str = _wasm_value_to_debug_string(value);
            _sb_append(&sb, (const char *)(intptr_t)key_str);
            _sb_append(&sb, ": ");
            _sb_append(&sb, (const char *)(intptr_t)val_str);
            count++;
        }
    }
    _sb_append(&sb, "})");
    return _sb_finish(&sb);
}

/* W-4f2: Set toString: Set({elem1, elem2, ...}) */
static int64_t _wasm_set_to_string(int64_t set_ptr) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "Set({");
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) _sb_append(&sb, ", ");
        /* Native uses snprintf(int64_t) for set elements — integers only */
        int64_t elem = list[WASM_LIST_ELEMS + i];
        int64_t elem_str = _wasm_value_to_display_string(elem);
        _sb_append(&sb, (const char *)(intptr_t)elem_str);
    }
    _sb_append(&sb, "})");
    return _sb_finish(&sb);
}

/* W-4f2: List toString: @[elem1, elem2, ...] */
static int64_t _wasm_list_to_string(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "@[");
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) _sb_append(&sb, ", ");
        int64_t item = list[WASM_LIST_ELEMS + i];
        int64_t item_str = _wasm_value_to_debug_string(item);
        _sb_append(&sb, (const char *)(intptr_t)item_str);
    }
    _sb_append(&sb, "]");
    return _sb_finish(&sb);
}

/* W-4f2: Pack toString: @(field <= value, ...) */
static int64_t _wasm_pack_to_string(int64_t pack_ptr) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    int64_t fc = pack[0];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "@(");
    int count = 0;
    for (int64_t i = 0; i < fc; i++) {
        int64_t field_hash = pack[1 + i * 3];
        int64_t field_val = pack[1 + i * 3 + 2];
        const char *fname = _wasm_lookup_field_name(field_hash);
        if (!fname) continue;
        /* Skip internal __ fields for display (same as native) */
        if (fname[0] == '_' && fname[1] == '_') continue;
        if (count > 0) _sb_append(&sb, ", ");
        _sb_append(&sb, fname);
        _sb_append(&sb, " <= ");
        /* Check if field is Bool via type registry */
        int64_t ftype = _wasm_lookup_field_type(field_hash);
        if (ftype == 4) {
            /* Bool type tag = 4 in native convention */
            _sb_append(&sb, field_val ? "true" : "false");
        } else {
            int64_t val_str = _wasm_value_to_debug_string(field_val);
            _sb_append(&sb, (const char *)(intptr_t)val_str);
        }
        count++;
    }
    _sb_append(&sb, ")");
    return _sb_finish(&sb);
}

/* W-5f: Detect Lax, Result, Gorillax, RelaxedGorillax by pack structure.
   These all have fc=4 with distinctive first-field hashes:
   - Lax:              hash0 = WASM_HASH_HAS_VALUE (0x9e9c6dc733414d60)
   - Gorillax/Relaxed: hash0 = WASM_HASH_IS_OK     (0x6550c1c5b98b56bf)
   - Result:           hash0 = WASM_HASH___VALUE    (0x0a7fc9f13472bbe0) */

/* W-5g: Bounds-check helper for WASM32. On wasm32, intptr_t is 32-bit,
   so int64_t values that are bitcast floats (e.g. _d2l(3.14)) would be
   truncated and dereference invalid memory. This helper prevents that. */
static int _wasm_is_valid_ptr(int64_t val, unsigned int min_bytes) {
    if (val <= 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr < WASM_MIN_HEAP_ADDR) return 0; /* skip null/very-low addresses */
    if (addr + min_bytes > mem_size) return 0;
    return 1;
}

static int _wasm_is_result(int64_t val) {
    /* Need at least 13 int64_t slots (fc + 4*3 fields) = 104 bytes */
    if (!_wasm_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    /* Result: fc=4, hash0 = WASM_HASH___VALUE, hash2 = WASM_HASH_THROW */
    if (p[0] == 4 && p[1] == WASM_HASH___VALUE) {
        int64_t hash2 = p[1 + 2 * 3]; /* field 2 hash */
        if (hash2 == WASM_HASH_THROW) return 1;
    }
    return 0;
}

static int _wasm_is_gorillax(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    /* Gorillax/RelaxedGorillax: fc=4, hash0 = WASM_HASH_IS_OK */
    if (p[0] == 4 && p[1] == WASM_HASH_IS_OK) return 1;
    return 0;
}

/* Detect Gorillax type: 0 = unknown, 1 = Gorillax, 2 = RelaxedGorillax */
static int _wasm_gorillax_type(int64_t val) {
    int64_t *p = (int64_t *)(intptr_t)val;
    /* __type field is at index 3: p[1 + 3*3 + 2] = p[12] */
    int64_t type_str = p[1 + 3 * 3 + 2]; /* field 3 value */
    /* Static strings may be in data section at low addresses, so skip
       WASM_MIN_HEAP_ADDR check; _looks_like_string validates the pointer. */
    if (type_str > 0 && _looks_like_string(type_str)) {
        const char *s = (const char *)(intptr_t)type_str;
        if (s[0] == 'G') return 1; /* "Gorillax" */
        if (s[0] == 'R') return 2; /* "RelaxedGorillax" */
    }
    return 1; /* default to Gorillax */
}

/* TF-4: Convert a Lax inner value to display string using its type tag.
   Without tag, float bit-patterns would fall through to int display. */
static int64_t _wasm_lax_value_display(int64_t val, int64_t tag) {
    if (tag == WASM_TAG_FLOAT) {
        return taida_float_to_str(val);
    }
    if (tag == WASM_TAG_BOOL) {
        return val ? (int64_t)(intptr_t)"true" : (int64_t)(intptr_t)"false";
    }
    /* For INT, STR, PACK etc. — use generic display */
    return _wasm_value_to_display_string(val);
}

/* W-5f: Lax.toString() — "Lax(value)" or "Lax(default: value)" */
static int64_t _wasm_lax_to_string(int64_t lax_ptr) {
    int64_t has_value = taida_pack_get_idx(lax_ptr, 0); /* hasValue */
    int64_t value = taida_pack_get_idx(lax_ptr, 1);     /* __value */
    int64_t def = taida_pack_get_idx(lax_ptr, 2);       /* __default */
    /* TF-4: Use type tag from __value field (index 1) for type-aware display */
    int64_t val_tag = taida_pack_get_tag(lax_ptr, 1);
    int64_t def_tag = taida_pack_get_tag(lax_ptr, 2);
    int64_t rendered = has_value
        ? _wasm_lax_value_display(value, val_tag)
        : _wasm_lax_value_display(def, def_tag);
    const char *rs = (const char *)(intptr_t)rendered;
    _wasm_strbuf sb;
    _sb_init(&sb);
    if (has_value) {
        _sb_append(&sb, "Lax(");
        _sb_append(&sb, rs);
        _sb_append(&sb, ")");
    } else {
        _sb_append(&sb, "Lax(default: ");
        _sb_append(&sb, rs);
        _sb_append(&sb, ")");
    }
    return _sb_finish(&sb);
}

/* Throw value to display string: extract "message" field from error BuchiPack,
   matching native's taida_throw_to_display_string. */
static int64_t _wasm_throw_to_display_string(int64_t throw_val) {
    if (throw_val == 0) return (int64_t)(intptr_t)"error";
    /* If it's a BuchiPack, look for "message" field */
    if (_looks_like_pack(throw_val)) {
        int64_t *p = (int64_t *)(intptr_t)throw_val;
        int64_t fc = p[0];
        for (int64_t i = 0; i < fc; i++) {
            if (p[1 + i * 3] == WASM_HASH_MESSAGE) {
                int64_t msg = p[1 + i * 3 + 2];
                if (msg && _looks_like_string(msg)) return msg;
                break;
            }
        }
        /* Fallback: render full pack structure */
        return _wasm_value_to_display_string(throw_val);
    }
    /* String or other value */
    return _wasm_value_to_display_string(throw_val);
}

/* W-5g: Result.toString() — predicate-aware, matching native */
static int64_t _wasm_result_to_string(int64_t result) {
    if (!_wasm_result_is_error_check(result)) {
        /* Success case */
        int64_t value = taida_pack_get_idx(result, 0); /* __value */
        int64_t value_str = _wasm_value_to_display_string(value);
        _wasm_strbuf sb;
        _sb_init(&sb);
        _sb_append(&sb, "Result(");
        _sb_append(&sb, (const char *)(intptr_t)value_str);
        _sb_append(&sb, ")");
        return _sb_finish(&sb);
    }
    /* Error case — throw_val == 0 means Unit (@()), matching interpreter */
    int64_t throw_val = taida_pack_get_idx(result, 2); /* throw field */
    if (throw_val == 0) {
        return (int64_t)(intptr_t)"Result(throw <= @())";
    }
    int64_t err_str = _wasm_throw_to_display_string(throw_val);
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "Result(throw <= ");
    _sb_append(&sb, (const char *)(intptr_t)err_str);
    _sb_append(&sb, ")");
    return _sb_finish(&sb);
}

/* W-5f: Gorillax.toString() — "Gorillax(value)" or "Gorillax(><)" */
static int64_t _wasm_gorillax_to_string(int64_t gx) {
    int64_t is_ok = taida_pack_get_idx(gx, 0);
    int gtype = _wasm_gorillax_type(gx);
    const char *prefix = (gtype == 2) ? "RelaxedGorillax" : "Gorillax";
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, prefix);
    _sb_append(&sb, "(");
    if (is_ok) {
        int64_t value = taida_pack_get_idx(gx, 1);
        int64_t val_str = _wasm_value_to_display_string(value);
        _sb_append(&sb, (const char *)(intptr_t)val_str);
    } else {
        if (gtype == 2) {
            _sb_append(&sb, "escaped");
        } else {
            _sb_append(&sb, "><");
        }
    }
    _sb_append(&sb, ")");
    return _sb_finish(&sb);
}

/* Detect Error pack: fc in 2..10, first field hash == WASM_HASH_TYPE,
   second field hash == WASM_HASH_MESSAGE. Two-field check prevents false
   positives from user-defined packs like @(type <= "Foo", ...). */
static int _wasm_is_error(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 32)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] >= 2 && p[0] <= 10 && p[1] == WASM_HASH_TYPE && p[4] == WASM_HASH_MESSAGE) return 1;
    return 0;
}

/* Error.toString() — "Error: message" format (matching native/interpreter) */
static int64_t _wasm_error_to_string(int64_t val) {
    int64_t *p = (int64_t *)(intptr_t)val;
    int64_t fc = p[0];
    /* Find message field by hash */
    int64_t msg = 0;
    for (int64_t i = 0; i < fc; i++) {
        if (p[1 + i * 3] == WASM_HASH_MESSAGE) {
            msg = p[1 + i * 3 + 2];
            break;
        }
    }
    if (msg && _looks_like_string(msg)) {
        const char *ms = (const char *)(intptr_t)msg;
        _wasm_strbuf sb;
        _sb_init(&sb);
        _sb_append(&sb, "Error: ");
        _sb_append(&sb, ms);
        return _sb_finish(&sb);
    }
    return (int64_t)(intptr_t)"Error";
}

/* W-4f2: Convert value to display string (like native's taida_value_to_display_string) */
static int64_t _wasm_value_to_display_string(int64_t val) {
    if (val == 0) return (int64_t)(intptr_t)"0";
    /* Check HashMap first (has distinctive marker) */
    if (_is_wasm_hashmap(val)) return _wasm_hashmap_to_string(val);
    /* Check Set (has WASM_SET_MARKER_VAL marker) */
    if (_is_wasm_set(val)) return _wasm_set_to_string(val);
    /* Check List (has list-like header but no set/hashmap marker) */
    if (_looks_like_list(val)) return _wasm_list_to_string(val);
    /* PR-4: Check Async before monadic types */
    if (_wasm_is_async_obj(val)) return _wasm_async_to_string(val);
    /* W-5f: Check monadic types before generic pack (Lax/Result/Gorillax) */
    if (_wasm_is_result(val)) return _wasm_result_to_string(val);
    if (_wasm_is_gorillax(val)) return _wasm_gorillax_to_string(val);
    if (_wasm_is_lax(val)) return _wasm_lax_to_string(val);
    /* Check Error before generic pack (Error is a pack with "type" field) */
    if (_wasm_is_error(val)) return _wasm_error_to_string(val);
    /* Check Pack (field_count + hash pattern) */
    if (_looks_like_pack(val)) return _wasm_pack_to_string(val);
    /* Check if it's a string */
    if (_looks_like_string(val)) return val;
    /* Fallback: integer */
    return taida_int_to_str(val);
}

/* W-4f2: Convert value to debug string (strings are quoted, everything else like display) */
static int64_t _wasm_value_to_debug_string(int64_t val) {
    if (val == 0) return (int64_t)(intptr_t)"0";
    /* Check collection types first */
    if (_is_wasm_hashmap(val)) return _wasm_hashmap_to_string(val);
    if (_is_wasm_set(val)) return _wasm_set_to_string(val);
    if (_looks_like_list(val)) return _wasm_list_to_string(val);
    /* PR-4: Check Async before monadic types */
    if (_wasm_is_async_obj(val)) return _wasm_async_to_string(val);
    /* W-5f: Check monadic types before generic pack */
    if (_wasm_is_result(val)) return _wasm_result_to_string(val);
    if (_wasm_is_gorillax(val)) return _wasm_gorillax_to_string(val);
    if (_wasm_is_lax(val)) return _wasm_lax_to_string(val);
    /* Check Error before generic pack */
    if (_wasm_is_error(val)) return _wasm_error_to_string(val);
    if (_looks_like_pack(val)) return _wasm_pack_to_string(val);
    /* Check if it's a string — quote it for debug */
    if (_looks_like_string(val)) {
        const char *s = (const char *)(intptr_t)val;
        int slen = wasm_strlen(s);
        char *buf = (char *)wasm_alloc(slen + 3);
        if (!buf) return val;
        buf[0] = '"';
        for (int i = 0; i < slen; i++) buf[1 + i] = s[i];
        buf[slen + 1] = '"';
        buf[slen + 2] = '\0';
        return (int64_t)(intptr_t)buf;
    }
    /* Fallback: integer */
    return taida_int_to_str(val);
}

int64_t taida_polymorphic_to_string(int64_t obj) {
    return _wasm_value_to_display_string(obj);
}

/* ── W-6: taida_debug_polymorphic implementation ── */

int64_t taida_debug_polymorphic(int64_t val) {
    int64_t str = _wasm_value_to_display_string(val);
    const char *s = (const char *)(intptr_t)str;
    if (s) {
        int32_t len = wasm_strlen(s);
        write_stdout(s, len);
        write_stdout("\n", 1);
    }
    return 0;
}

/* ── W-3f: taida_int_mold_str (wasm-min: parse string to int, returns Lax) ── */
/* Parses a string to Int and returns Lax[Int]. Invalid strings return empty Lax. */

int64_t taida_int_mold_str(int64_t v) {
    const char *s = (const char *)(intptr_t)v;
    if (!s || s[0] == '\0') return taida_lax_empty(0);
    int neg = 0;
    int i = 0;
    if (s[0] == '-') { neg = 1; i = 1; }
    else if (s[0] == '+') { i = 1; }
    if (s[i] == '\0') return taida_lax_empty(0); /* just sign, no digits */
    int found_digit = 0;
    int digit_count = 0;
    uint64_t acc = 0;
    while (s[i] >= '0' && s[i] <= '9') {
        digit_count++;
        if (digit_count > 19) return taida_lax_empty(0); /* overflow guard: int64_t max is 19 digits */
        acc = acc * 10 + (uint64_t)(s[i] - '0');
        found_digit = 1;
        i++;
    }
    if (!found_digit || s[i] != '\0') return taida_lax_empty(0);
    int64_t result = neg ? -(int64_t)acc : (int64_t)acc;
    return taida_lax_new(result, 0);
}

/* ── W-4f2: Field name/type registry for Pack toString ── */
/* Simple linear array registry. Sufficient for wasm-min programs with few field names. */

#define WASM_FIELD_REGISTRY_MAX 256

static struct {
    int64_t hash;
    const char *name;
    int64_t type_tag;
} _wasm_field_registry[WASM_FIELD_REGISTRY_MAX];
static int _wasm_field_registry_count = 0;

int64_t taida_register_field_name(int64_t hash, int64_t name_ptr) {
    /* Check if already registered */
    for (int i = 0; i < _wasm_field_registry_count; i++) {
        if (_wasm_field_registry[i].hash == hash) return 0;
    }
    if (_wasm_field_registry_count < WASM_FIELD_REGISTRY_MAX) {
        _wasm_field_registry[_wasm_field_registry_count].hash = hash;
        _wasm_field_registry[_wasm_field_registry_count].name = (const char *)(intptr_t)name_ptr;
        _wasm_field_registry[_wasm_field_registry_count].type_tag = -1;
        _wasm_field_registry_count++;
    }
    return 0;
}

int64_t taida_register_field_type(int64_t hash, int64_t name_ptr, int64_t type_tag) {
    /* Update existing entry or add new one */
    for (int i = 0; i < _wasm_field_registry_count; i++) {
        if (_wasm_field_registry[i].hash == hash) {
            _wasm_field_registry[i].type_tag = type_tag;
            return 0;
        }
    }
    if (_wasm_field_registry_count < WASM_FIELD_REGISTRY_MAX) {
        _wasm_field_registry[_wasm_field_registry_count].hash = hash;
        _wasm_field_registry[_wasm_field_registry_count].name = (const char *)(intptr_t)name_ptr;
        _wasm_field_registry[_wasm_field_registry_count].type_tag = type_tag;
        _wasm_field_registry_count++;
    }
    return 0;
}

static const char *_wasm_lookup_field_name(int64_t hash) {
    for (int i = 0; i < _wasm_field_registry_count; i++) {
        if (_wasm_field_registry[i].hash == hash) return _wasm_field_registry[i].name;
    }
    return 0;
}

static int64_t _wasm_lookup_field_type(int64_t hash) {
    for (int i = 0; i < _wasm_field_registry_count; i++) {
        if (_wasm_field_registry[i].hash == hash) return _wasm_field_registry[i].type_tag;
    }
    return -1;
}

/* ── W-4: BuchiPack runtime (bump allocator, no RC/magic) ── */
/* Layout: [field_count, field0_hash, field0_tag, field0_value, field1_hash, ...]
   Same as native_runtime.c but without magic header and refcount.
   Each field occupies 3 int64_t slots: hash, tag, value.
   Total allocation: (1 + field_count * 3) * sizeof(int64_t) */

int64_t taida_pack_new(int64_t field_count) {
    int64_t slots = 1 + field_count * 3;
    int64_t *pack = (int64_t *)wasm_alloc((unsigned int)(slots * 8));
    if (!pack) return 0;
    pack[0] = field_count;
    /* Zero-initialize all field slots (hash=0, tag=0=INT, value=0) */
    for (int64_t i = 1; i < slots; i++) pack[i] = 0;
    return (int64_t)(intptr_t)pack;
}

int64_t taida_pack_set(int64_t pack_ptr, int64_t index, int64_t value) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    pack[1 + index * 3 + 2] = value;
    return pack_ptr;
}

int64_t taida_pack_set_tag(int64_t pack_ptr, int64_t index, int64_t tag) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    pack[1 + index * 3 + 1] = tag;
    return pack_ptr;
}

int64_t taida_pack_get_tag(int64_t pack_ptr, int64_t index) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    return pack[1 + index * 3 + 1];
}

int64_t taida_pack_get_idx(int64_t pack_ptr, int64_t index) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    return pack[1 + index * 3 + 2];
}

int64_t taida_pack_set_hash(int64_t pack_ptr, int64_t index, int64_t hash) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    pack[1 + index * 3] = hash;
    return pack_ptr;
}

int64_t taida_pack_get(int64_t pack_ptr, int64_t field_hash) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    int64_t count = pack[0];
    for (int64_t i = 0; i < count; i++) {
        if (pack[1 + i * 3] == field_hash) {
            return pack[1 + i * 3 + 2];
        }
    }
    return 0; /* default value */
}

int64_t taida_pack_has_hash(int64_t pack_ptr, int64_t field_hash) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    int64_t count = pack[0];
    for (int64_t i = 0; i < count; i++) {
        if (pack[1 + i * 3] == field_hash) {
            return 1;
        }
    }
    return 0;
}

/* ── W-4: List runtime (bump allocator, no RC/magic) ── */
/* Layout: [capacity, length, elem_type_tag, elem0, elem1, ...]
   Same concept as native_runtime.c but without magic header and refcount.
   Note: bump allocator cannot realloc, so we use copy-on-grow. */

int64_t taida_list_new(void) {
    int64_t initial_cap = 16;
    int64_t slots = WASM_LIST_ELEMS + initial_cap;  /* header(4) + elements */
    int64_t *list = (int64_t *)wasm_alloc((unsigned int)(slots * 8));
    if (!list) return 0;
    list[0] = initial_cap;  /* capacity */
    list[1] = 0;            /* length */
    list[2] = -1;           /* elem_type_tag (UNKNOWN) */
    list[3] = 0;            /* type_marker (0 = plain list) */
    return (int64_t)(intptr_t)list;
}

void taida_list_set_elem_tag(int64_t list_ptr, int64_t tag) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    list[2] = tag;
}

int64_t taida_list_push(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t cap = list[0];
    int64_t len = list[1];
    if (len >= cap) {
        /* Grow: allocate new list with double capacity, copy over */
        int64_t new_cap = cap * 2;
        int64_t new_slots = WASM_LIST_ELEMS + new_cap;
        int64_t *new_list = (int64_t *)wasm_alloc((unsigned int)(new_slots * 8));
        if (!new_list) return list_ptr;
        /* Copy header + existing elements */
        for (int64_t i = 0; i < WASM_LIST_ELEMS + len; i++) new_list[i] = list[i];
        new_list[0] = new_cap;
        list = new_list;
        list_ptr = (int64_t)(intptr_t)new_list;
    }
    list[WASM_LIST_ELEMS + len] = item;
    list[1] = len + 1;
    return list_ptr;
}

int64_t taida_list_length(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    return list[1];
}

/* taida_list_get: returns Lax[T]. OOB returns empty Lax. */
int64_t taida_list_get(int64_t list_ptr, int64_t index) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (index < 0 || index >= len) return taida_lax_empty(0);
    return taida_lax_new(list[WASM_LIST_ELEMS + index], 0);
}

int64_t taida_list_is_empty(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    return list[1] == 0 ? 1 : 0;
}

/* ── W-4: HashMap runtime (bump allocator, no RC/magic) ── */
/* Layout: [capacity, length, value_type_tag, type_marker, entries...]
   Each entry: [key_hash, key_ptr, value] (3 slots)
   Header = 4 slots (including type_marker for polymorphic dispatch),
   then capacity * 3 entry slots.
   Open addressing with linear probing. */

/* WASM_HM_HEADER and WASM_HM_MARKER_VAL defined above (near WASM_LIST_ELEMS) */

/* String comparison helper */
static int _wasm_streq(const char *a, const char *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    while (*a && *b) {
        if (*a != *b) return 0;
        a++; b++;
    }
    return *a == *b;
}

static int64_t _wasm_hashmap_new_with_cap(int64_t cap) {
    int64_t slots = WASM_HM_HEADER + cap * 3;
    int64_t *hm = (int64_t *)wasm_alloc((unsigned int)(slots * 8));
    if (!hm) return 0;
    /* Zero-initialize everything (empty slots have hash=0, key=0) */
    for (int64_t i = 0; i < slots; i++) hm[i] = 0;
    hm[0] = cap;
    hm[1] = 0;     /* length */
    hm[2] = -1;    /* value_type_tag = UNKNOWN */
    hm[3] = WASM_HM_MARKER_VAL;  /* type marker for polymorphic dispatch */
    return (int64_t)(intptr_t)hm;
}

int64_t taida_hashmap_new(void) {
    return _wasm_hashmap_new_with_cap(16);
}

void taida_hashmap_set_value_tag(int64_t hm_ptr, int64_t tag) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    hm[2] = tag;
}

/* FNV-1a hash for string keys */
int64_t taida_str_hash(int64_t str_ptr) {
    const unsigned char *s = (const unsigned char *)(intptr_t)str_ptr;
    if (!s) return 0;
    uint64_t hash = 0xcbf29ce484222325ULL;
    while (*s) {
        hash ^= *s++;
        hash *= 0x100000001b3ULL;
    }
    return (int64_t)hash;
}

int64_t taida_hashmap_set(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr, int64_t value) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t len = hm[1];

    /* Resize if load factor > 70% */
    if (len * 10 >= cap * 7) {
        int64_t new_cap = cap * 2;
        int64_t new_hm_ptr = _wasm_hashmap_new_with_cap(new_cap);
        int64_t *new_hm = (int64_t *)(intptr_t)new_hm_ptr;
        new_hm[2] = hm[2]; /* propagate value_type_tag */
        new_hm[3] = WASM_HM_MARKER_VAL;  /* propagate type marker */
        /* Re-hash all occupied entries */
        for (int64_t i = 0; i < cap; i++) {
            int64_t sh = hm[WASM_HM_HEADER + i * 3];
            int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
            if (sh != 0 || sk != 0) {
                /* Insert into new table */
                uint64_t uh = (uint64_t)sh;
                int64_t idx = (int64_t)(uh % (uint64_t)new_cap);
                for (int64_t j = 0; j < new_cap; j++) {
                    int64_t slot = (idx + j) % new_cap;
                    int64_t esh = new_hm[WASM_HM_HEADER + slot * 3];
                    int64_t esk = new_hm[WASM_HM_HEADER + slot * 3 + 1];
                    if (esh == 0 && esk == 0) {
                        new_hm[WASM_HM_HEADER + slot * 3] = sh;
                        new_hm[WASM_HM_HEADER + slot * 3 + 1] = sk;
                        new_hm[WASM_HM_HEADER + slot * 3 + 2] = hm[WASM_HM_HEADER + i * 3 + 2];
                        new_hm[1]++;
                        break;
                    }
                }
            }
        }
        hm = new_hm;
        hm_ptr = new_hm_ptr;
        cap = new_cap;
    }

    /* Insert or update */
    uint64_t uh = (uint64_t)key_hash;
    int64_t idx = (int64_t)(uh % (uint64_t)cap);
    for (int64_t i = 0; i < cap; i++) {
        int64_t slot = (idx + i) % cap;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (sh == 0 && sk == 0) {
            /* Empty slot — insert */
            hm[WASM_HM_HEADER + slot * 3] = key_hash;
            hm[WASM_HM_HEADER + slot * 3 + 1] = key_ptr;
            hm[WASM_HM_HEADER + slot * 3 + 2] = value;
            hm[1]++;
            return hm_ptr;
        }
        if (sh == key_hash && _wasm_streq((const char *)(intptr_t)sk, (const char *)(intptr_t)key_ptr)) {
            /* Existing key — update value */
            hm[WASM_HM_HEADER + slot * 3 + 2] = value;
            return hm_ptr;
        }
    }
    return hm_ptr;
}

int64_t taida_hashmap_get(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    uint64_t uh = (uint64_t)key_hash;
    int64_t idx = (int64_t)(uh % (uint64_t)cap);
    for (int64_t i = 0; i < cap; i++) {
        int64_t slot = (idx + i) % cap;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (sh == 0 && sk == 0) return 0; /* not found */
        if (sh == key_hash && _wasm_streq((const char *)(intptr_t)sk, (const char *)(intptr_t)key_ptr))
            return hm[WASM_HM_HEADER + slot * 3 + 2];
    }
    return 0;
}

int64_t taida_hashmap_has(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    uint64_t uh = (uint64_t)key_hash;
    int64_t idx = (int64_t)(uh % (uint64_t)cap);
    for (int64_t i = 0; i < cap; i++) {
        int64_t slot = (idx + i) % cap;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (sh == 0 && sk == 0) return 0;
        if (sh == key_hash && _wasm_streq((const char *)(intptr_t)sk, (const char *)(intptr_t)key_ptr))
            return 1;
    }
    return 0;
}

int64_t taida_hashmap_is_empty(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    return hm[1] == 0 ? 1 : 0;
}

/* Immutable set: clone the hashmap first to preserve immutable semantics.
   In wasm-min's bump allocator, taida_hashmap_set modifies in place,
   which would mutate the original hashmap. */
static int64_t _wasm_hashmap_clone(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t new_hm_ptr = _wasm_hashmap_new_with_cap(cap);
    int64_t *new_hm = (int64_t *)(intptr_t)new_hm_ptr;
    new_hm[2] = hm[2]; /* propagate value_type_tag */
    /* Copy all entries */
    for (int64_t i = 0; i < cap; i++) {
        int64_t sh = hm[WASM_HM_HEADER + i * 3];
        int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
        new_hm[WASM_HM_HEADER + i * 3] = sh;
        new_hm[WASM_HM_HEADER + i * 3 + 1] = sk;
        new_hm[WASM_HM_HEADER + i * 3 + 2] = hm[WASM_HM_HEADER + i * 3 + 2];
    }
    new_hm[1] = hm[1]; /* copy length */
    return new_hm_ptr;
}

int64_t taida_hashmap_set_immut(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr, int64_t value) {
    int64_t clone = _wasm_hashmap_clone(hm_ptr);
    return taida_hashmap_set(clone, key_hash, key_ptr, value);
}

/* taida_hashmap_get_lax: returns Lax[V]. Key not found returns empty Lax.
   Single-pass lookup (inlined probe) avoids double hash-table traversal. */
int64_t taida_hashmap_get_lax(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    uint64_t uh = (uint64_t)key_hash;
    int64_t idx = (int64_t)(uh % (uint64_t)cap);
    for (int64_t i = 0; i < cap; i++) {
        int64_t slot = (idx + i) % cap;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (sh == 0 && sk == 0) return taida_lax_empty(0); /* empty slot — not found */
        if (sh == key_hash && _wasm_streq((const char *)(intptr_t)sk, (const char *)(intptr_t)key_ptr))
            return taida_lax_new(hm[WASM_HM_HEADER + slot * 3 + 2], 0);
    }
    return taida_lax_empty(0);
}

/* ── W-4f: HashMap type detection helper ── */
/* Check if a pointer is a HashMap by looking for the type marker at index 3. */
static int _is_wasm_hashmap(int64_t ptr) {
    if (ptr == 0) return 0;
    if (ptr < 0 || ptr > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)ptr;
    /* Need at least 4 int64_t header slots */
    if (addr + 32 > mem_size) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WASM_HM_MARKER_VAL;
}

/* ── W-4f: taida_value_hash — polymorphic hash for collection keys ── */
/* For strings, compute FNV-1a hash. For scalars, use identity (adjusted). */
int64_t taida_value_hash(int64_t val) {
    /* Try to detect string pointers */
    if (_looks_like_string(val)) {
        int64_t h = taida_str_hash(val);
        /* Adjust to avoid 0/1 (reserved for empty/tombstone) */
        if (h == 0 || h == 1) h = h + 2;
        return h;
    }
    /* Scalar: use identity, adjusted to avoid 0/1 */
    int64_t h = val;
    if (h == 0 || h == 1) h = h + 2;
    return h;
}

/* ── W-4f: HashMap additional methods ── */

/* HashMap.keys() -> List of key pointers */
int64_t taida_hashmap_keys(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t list = taida_list_new();
    for (int64_t i = 0; i < cap; i++) {
        int64_t sh = hm[WASM_HM_HEADER + i * 3];
        int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
        if (sh != 0 || sk != 0) {
            list = taida_list_push(list, sk);
        }
    }
    return list;
}

/* HashMap.values() -> List of values */
int64_t taida_hashmap_values(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t list = taida_list_new();
    for (int64_t i = 0; i < cap; i++) {
        int64_t sh = hm[WASM_HM_HEADER + i * 3];
        int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
        if (sh != 0 || sk != 0) {
            list = taida_list_push(list, hm[WASM_HM_HEADER + i * 3 + 2]);
        }
    }
    return list;
}

/* HashMap.entries() -> List of BuchiPack @(key, value) */
int64_t taida_hashmap_entries(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t list = taida_list_new();
    /* FNV-1a hashes for "key" and "value" (same as native runtime) */
    #define WASM_HASH_KEY   0x3dc94a19365b10ecLL
    #define WASM_HASH_VAL   0x7ce4fd9430e80ceaLL
    for (int64_t i = 0; i < cap; i++) {
        int64_t sh = hm[WASM_HM_HEADER + i * 3];
        int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
        if (sh != 0 || sk != 0) {
            int64_t pair = taida_pack_new(2);
            taida_pack_set_hash(pair, 0, WASM_HASH_KEY);
            taida_pack_set(pair, 0, sk);
            taida_pack_set_hash(pair, 1, WASM_HASH_VAL);
            taida_pack_set(pair, 1, hm[WASM_HM_HEADER + i * 3 + 2]);
            list = taida_list_push(list, pair);
        }
    }
    return list;
}

/* HashMap.merge(other) -> new HashMap with other's entries overwriting */
int64_t taida_hashmap_merge(int64_t hm_ptr, int64_t other_ptr) {
    int64_t *other = (int64_t *)(intptr_t)other_ptr;
    int64_t cap = other[0];
    /* Start with a copy of hm (in wasm-min, just use the original since bump allocator) */
    /* Actually, we need a new copy. Iterate hm first, then apply other's entries. */
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t hm_cap = hm[0];
    int64_t result = taida_hashmap_new();
    int64_t *r = (int64_t *)(intptr_t)result;
    r[2] = hm[2]; /* propagate value_type_tag */
    /* Copy from hm */
    for (int64_t i = 0; i < hm_cap; i++) {
        int64_t sh = hm[WASM_HM_HEADER + i * 3];
        int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
        if (sh != 0 || sk != 0) {
            result = taida_hashmap_set(result, sh, sk, hm[WASM_HM_HEADER + i * 3 + 2]);
        }
    }
    /* Overwrite/add from other */
    for (int64_t i = 0; i < cap; i++) {
        int64_t sh = other[WASM_HM_HEADER + i * 3];
        int64_t sk = other[WASM_HM_HEADER + i * 3 + 1];
        if (sh != 0 || sk != 0) {
            result = taida_hashmap_set(result, sh, sk, other[WASM_HM_HEADER + i * 3 + 2]);
        }
    }
    return result;
}

/* HashMap.remove(key_hash, key_ptr) -> new HashMap without the key (immutable) */
/* In wasm-min, mutate in place (bump allocator, no sharing). Uses tombstone. */
static int64_t taida_hashmap_remove(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    uint64_t uh = (uint64_t)key_hash;
    int64_t idx = (int64_t)(uh % (uint64_t)cap);
    for (int64_t i = 0; i < cap; i++) {
        int64_t slot = (idx + i) % cap;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (sh == 0 && sk == 0) return hm_ptr; /* not found */
        if (sh == key_hash && _wasm_streq((const char *)(intptr_t)sk, (const char *)(intptr_t)key_ptr)) {
            /* Found — tombstone it */
            hm[WASM_HM_HEADER + slot * 3] = 1; /* tombstone hash */
            hm[WASM_HM_HEADER + slot * 3 + 1] = 0;
            hm[WASM_HM_HEADER + slot * 3 + 2] = 0;
            hm[1]--;
            return hm_ptr;
        }
    }
    return hm_ptr;
}

/* ── W-4: Set runtime (simplified — backed by linear scan array) ── */
/* Layout: [capacity, length, elem_type_tag, WASM_SET_MARKER_VAL, elem0, elem1, ...]
   Same as List layout but with Set marker at slot[3]. Uses linear scan for has/add/remove.
   Sufficient for wasm-min's simple programs. */

int64_t taida_set_new(void) {
    int64_t set = taida_list_new();
    if (set) ((int64_t *)(intptr_t)set)[3] = WASM_SET_MARKER_VAL;
    return set;
}

void taida_set_set_elem_tag(int64_t set_ptr, int64_t tag) {
    taida_list_set_elem_tag(set_ptr, tag);
}

/* W-4f/F-3: Type-tag-aware equality for Set elements.
   For strings, compare by content (strcmp). For others, compare by raw value. */
static int _wasm_value_eq(int64_t a, int64_t b) {
    if (a == b) return 1;
    /* Check if both look like strings — if so, compare by content */
    if (_looks_like_string(a) && _looks_like_string(b)) {
        return _wasm_streq((const char *)(intptr_t)a, (const char *)(intptr_t)b);
    }
    return 0;
}

int64_t taida_set_has(int64_t set_ptr, int64_t item) {
    int64_t *set = (int64_t *)(intptr_t)set_ptr;
    int64_t len = set[1];
    for (int64_t i = 0; i < len; i++) {
        if (_wasm_value_eq(set[WASM_LIST_ELEMS + i], item)) return 1;
    }
    return 0;
}

int64_t taida_set_add(int64_t set_ptr, int64_t item) {
    if (taida_set_has(set_ptr, item)) return set_ptr;
    /* Create a new set (copy elements) to preserve immutable semantics.
       In wasm-min's bump allocator, taida_list_push modifies in place when
       there's room, which would mutate the original set. */
    int64_t *old = (int64_t *)(intptr_t)set_ptr;
    int64_t len = old[1];
    int64_t new_set = taida_set_new();
    for (int64_t i = 0; i < len; i++) {
        new_set = taida_list_push(new_set, old[WASM_LIST_ELEMS + i]);
    }
    new_set = taida_list_push(new_set, item);
    return new_set;
}

int64_t taida_set_from_list(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t set = taida_set_new();
    for (int64_t i = 0; i < len; i++) {
        set = taida_set_add(set, list[WASM_LIST_ELEMS + i]);
    }
    return set;
}

/* W-4f: Set.remove(item) -> new Set without the item */
int64_t taida_set_remove(int64_t set_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    int64_t result = taida_set_new();
    for (int64_t i = 0; i < len; i++) {
        if (!_wasm_value_eq(list[WASM_LIST_ELEMS + i], item)) {
            result = taida_list_push(result, list[WASM_LIST_ELEMS + i]);
        }
    }
    return result;
}

/* W-4f: Set.union(other) -> new Set with all elements from both */
int64_t taida_set_union(int64_t set_a, int64_t set_b) {
    int64_t *a = (int64_t *)(intptr_t)set_a;
    int64_t *b = (int64_t *)(intptr_t)set_b;
    int64_t a_len = a[1];
    int64_t b_len = b[1];
    int64_t result = taida_set_new();
    for (int64_t i = 0; i < a_len; i++) {
        result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
    }
    for (int64_t i = 0; i < b_len; i++) {
        if (!taida_set_has(result, b[WASM_LIST_ELEMS + i])) {
            result = taida_list_push(result, b[WASM_LIST_ELEMS + i]);
        }
    }
    return result;
}

/* W-4f: Set.intersect(other) -> new Set with common elements */
int64_t taida_set_intersect(int64_t set_a, int64_t set_b) {
    int64_t *a = (int64_t *)(intptr_t)set_a;
    int64_t a_len = a[1];
    int64_t result = taida_set_new();
    for (int64_t i = 0; i < a_len; i++) {
        if (taida_set_has(set_b, a[WASM_LIST_ELEMS + i])) {
            result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
        }
    }
    return result;
}

/* W-4f: Set.diff(other) -> new Set with elements in a but not in b */
int64_t taida_set_diff(int64_t set_a, int64_t set_b) {
    int64_t *a = (int64_t *)(intptr_t)set_a;
    int64_t a_len = a[1];
    int64_t result = taida_set_new();
    for (int64_t i = 0; i < a_len; i++) {
        if (!taida_set_has(set_b, a[WASM_LIST_ELEMS + i])) {
            result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
        }
    }
    return result;
}

/* W-4f: Set.toList() -> List copy */
int64_t taida_set_to_list(int64_t set_ptr) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    int64_t result = taida_list_new();
    for (int64_t i = 0; i < len; i++) {
        result = taida_list_push(result, list[WASM_LIST_ELEMS + i]);
    }
    return result;
}

/* ── W-4f: Polymorphic collection methods ── */
/* These work on both HashMap and Set (auto-detect via type marker). */

/* .get(key_or_index) — HashMap: hash-based lookup, List: index-based */
int64_t taida_collection_get(int64_t ptr, int64_t item) {
    if (_is_wasm_hashmap(ptr)) {
        int64_t key_hash = taida_value_hash(item);
        return taida_hashmap_get_lax(ptr, key_hash, item);
    }
    /* List/Set: index-based access */
    return taida_list_get(ptr, item);
}

/* .has(key_or_item) — HashMap: hash-based lookup, Set/List: linear scan */
int64_t taida_collection_has(int64_t ptr, int64_t item) {
    if (_is_wasm_hashmap(ptr)) {
        int64_t key_hash = taida_value_hash(item);
        return taida_hashmap_has(ptr, key_hash, item);
    }
    /* Set/List: linear scan */
    return taida_set_has(ptr, item);
}

/* .remove(key_or_item) — HashMap: clone + hash-based removal, Set: linear scan */
int64_t taida_collection_remove(int64_t ptr, int64_t item) {
    if (_is_wasm_hashmap(ptr)) {
        int64_t clone = _wasm_hashmap_clone(ptr);
        int64_t key_hash = taida_value_hash(item);
        return taida_hashmap_remove(clone, key_hash, item);
    }
    /* Set: taida_set_remove already creates a new set */
    return taida_set_remove(ptr, item);
}

/* .size() — both HashMap and Set store length at ptr[1] */
int64_t taida_collection_size(int64_t ptr) {
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[1];
}

/* Forward declaration — taida_lax_is_empty is defined later */
int64_t taida_lax_is_empty(int64_t lax_ptr);

/* ── W-4f: taida_polymorphic_is_empty (wasm-min: Lax/List/Set/HashMap/String) ── */
int64_t taida_polymorphic_is_empty(int64_t ptr) {
    if (ptr == 0) return 1;
    /* Lax: isEmpty means hasValue == false */
    if (_wasm_is_lax(ptr)) {
        return taida_lax_is_empty(ptr);
    }
    /* All collection types store length at index 1 in wasm-min */
    if (_looks_like_list(ptr) || _is_wasm_hashmap(ptr)) {
        int64_t *data = (int64_t *)(intptr_t)ptr;
        return data[1] == 0 ? 1 : 0;
    }
    /* String: check if empty */
    if (_looks_like_string(ptr)) {
        const char *s = (const char *)(intptr_t)ptr;
        return s[0] == '\0' ? 1 : 0;
    }
    return 0;
}

/* ═══════════════════════════════════════════════════════════════════════════
   WC-6: Collection & Pack Extended + Type Detection + Polymorphic Extended
   ═══════════════════════════════════════════════════════════════════════════ */

/* Forward declarations for functions defined later in this file */
int64_t taida_is_closure_value(int64_t val);
int64_t taida_closure_get_fn(int64_t closure_ptr);
int64_t taida_closure_get_env(int64_t closure_ptr);
int64_t taida_invoke_callback1(int64_t fn_ptr, int64_t arg0);
int64_t taida_invoke_callback2(int64_t fn_ptr, int64_t arg0, int64_t arg1);
int64_t taida_result_create(int64_t value, int64_t throw_val, int64_t predicate);
int64_t taida_result_is_ok(int64_t result);
int64_t taida_str_new_copy(int64_t src_raw);
int64_t taida_list_contains(int64_t list_ptr, int64_t item);
int64_t taida_list_index_of(int64_t list_ptr, int64_t item);
int64_t taida_list_last_index_of(int64_t list_ptr, int64_t item);
int64_t taida_list_map(int64_t list_ptr, int64_t fn_ptr);
int64_t taida_str_contains(int64_t s_raw, int64_t sub_raw);
int64_t taida_str_index_of(int64_t s_raw, int64_t sub_raw);
int64_t taida_str_last_index_of(int64_t s_raw, int64_t sub_raw);

/* ── WC-6a: HashMap extensions ───────────────────────────── */

/* HashMap.length() / HashMap.size() -- return number of entries */
int64_t taida_hashmap_length(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t count = 0;
    for (int64_t i = 0; i < cap; i++) {
        int64_t kh = hm[WASM_HM_HEADER + i * 3];
        int64_t kp = hm[WASM_HM_HEADER + i * 3 + 1];
        if (kh != 0 || kp != 0) {
            /* Not empty and not tombstone (tombstone: hash=1, key=0) */
            if (!(kh == 1 && kp == 0)) count++;
        }
    }
    return count;
}

/* HashMap.clone() -- deep clone a hashmap (public wrapper for _wasm_hashmap_clone) */
int64_t taida_hashmap_clone(int64_t hm_ptr) {
    return _wasm_hashmap_clone(hm_ptr);
}

/* HashMap.toString() -- "HashMap({key: value, ...})" */
int64_t taida_hashmap_to_string(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];

    /* Count entries */
    int entry_count = 0;
    for (int64_t i = 0; i < cap; i++) {
        int64_t kh = hm[WASM_HM_HEADER + i * 3];
        int64_t kp = hm[WASM_HM_HEADER + i * 3 + 1];
        if ((kh != 0 || kp != 0) && !(kh == 1 && kp == 0)) {
            entry_count++;
        }
    }

    if (entry_count == 0) {
        // WCR-6: "HashMap({})" = 11 chars + NUL = 12 bytes
        char *r = (char *)wasm_alloc(12);
        _wf_memcpy(r, "HashMap({})", 11);
        r[11] = '\0';
        return (int64_t)r;
    }

    /* Build "HashMap({k: v, k: v, ...})" */
    int buf_size = 256;
    char *buf = (char *)wasm_alloc((unsigned int)buf_size);
    _wf_memcpy(buf, "HashMap({", 9);
    int pos = 9;
    int first = 1;
    for (int64_t i = 0; i < cap; i++) {
        int64_t kh = hm[WASM_HM_HEADER + i * 3];
        int64_t kp = hm[WASM_HM_HEADER + i * 3 + 1];
        int64_t val = hm[WASM_HM_HEADER + i * 3 + 2];
        if ((kh != 0 || kp != 0) && !(kh == 1 && kp == 0)) {
            if (!first) { buf[pos++] = ','; buf[pos++] = ' '; }
            first = 0;
            const char *ks = (const char *)(intptr_t)taida_polymorphic_to_string(kp);
            int kl = _wf_strlen(ks);
            const char *vs = (const char *)(intptr_t)taida_polymorphic_to_string(val);
            int vl = _wf_strlen(vs);
            while (pos + kl + vl + 10 > buf_size) {
                buf_size *= 2;
                char *new_buf = (char *)wasm_alloc((unsigned int)buf_size);
                _wf_memcpy(new_buf, buf, pos);
                buf = new_buf;
            }
            _wf_memcpy(buf + pos, ks, kl); pos += kl;
            buf[pos++] = ':'; buf[pos++] = ' ';
            _wf_memcpy(buf + pos, vs, vl); pos += vl;
        }
    }
    buf[pos++] = '}'; buf[pos++] = ')';
    buf[pos] = '\0';
    return (int64_t)buf;
}

/* HashMap.remove(key) -> new HashMap without that key (immutable) */
int64_t taida_hashmap_remove_immut(int64_t hm_ptr, int64_t key_hash, int64_t key_ptr) {
    int64_t clone = taida_hashmap_clone(hm_ptr);
    int64_t *hm = (int64_t *)(intptr_t)clone;
    int64_t cap = hm[0];
    for (int64_t i = 0; i < cap; i++) {
        int64_t kh = hm[WASM_HM_HEADER + i * 3];
        int64_t kp = hm[WASM_HM_HEADER + i * 3 + 1];
        if (kh == key_hash && kp != 0) {
            if (taida_str_eq(kp, key_ptr)) {
                hm[WASM_HM_HEADER + i * 3] = 1;     /* tombstone hash */
                hm[WASM_HM_HEADER + i * 3 + 1] = 0;
                hm[WASM_HM_HEADER + i * 3 + 2] = 0;
                break;
            }
        }
    }
    return clone;
}

/* HashMap with initial capacity (public wrapper) */
int64_t taida_hashmap_new_with_cap(int64_t cap) {
    if (cap < 8) cap = 8;
    return _wasm_hashmap_new_with_cap(cap);
}

/* HashMap adjust hash: ensure hash is never 0 or 1 (reserved) */
int64_t taida_hashmap_adjust_hash(int64_t h) {
    if (h == 0 || h == 1) return h + 2;
    return h;
}

/* HashMap set_internal: delegate to taida_hashmap_set */
int64_t taida_hashmap_set_internal(int64_t hm, int64_t kh, int64_t kp, int64_t v, int64_t mode) {
    (void)mode;
    return taida_hashmap_set(hm, kh, kp, v);
}

/* HashMap resize: re-insert all entries into larger map */
int64_t taida_hashmap_resize(int64_t hm_ptr, int64_t new_cap) {
    int64_t *old = (int64_t *)(intptr_t)hm_ptr;
    int64_t old_cap = old[0];
    int64_t new_hm = taida_hashmap_new_with_cap(new_cap);
    for (int64_t i = 0; i < old_cap; i++) {
        int64_t kh = old[WASM_HM_HEADER + i * 3];
        int64_t kp = old[WASM_HM_HEADER + i * 3 + 1];
        int64_t val = old[WASM_HM_HEADER + i * 3 + 2];
        if ((kh != 0 || kp != 0) && !(kh == 1 && kp == 0)) {
            new_hm = taida_hashmap_set(new_hm, kh, kp, val);
        }
    }
    return new_hm;
}

/* HashMap key/value lifecycle helpers (no-ops in WASM bump allocator) */
int64_t taida_hashmap_key_eq(int64_t a, int64_t b) { return taida_str_eq(a, b); }
int64_t taida_hashmap_key_retain(int64_t a, int64_t b) { (void)a; (void)b; return 0; }
int64_t taida_hashmap_key_release(int64_t a, int64_t b) { (void)a; (void)b; return 0; }
int64_t taida_hashmap_val_retain(int64_t a, int64_t b) { (void)a; (void)b; return 0; }
int64_t taida_hashmap_val_release(int64_t a, int64_t b) { (void)a; (void)b; return 0; }
int64_t taida_hashmap_key_valid(int64_t v) { return v != 0 ? 1 : 0; }

/* ── WC-6b: Set extensions ───────────────────────────────── */

int64_t taida_set_contains(int64_t set_ptr, int64_t item) {
    return taida_set_has(set_ptr, item);
}

int64_t taida_set_is_empty(int64_t set_ptr) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    return list[1] == 0 ? 1 : 0;
}

int64_t taida_set_size(int64_t set_ptr) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    return list[1];
}

int64_t taida_set_to_string(int64_t set_ptr) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    if (len == 0) {
        char *r = (char *)wasm_alloc(8);
        _wf_memcpy(r, "Set({})", 8);
        return (int64_t)r;
    }
    int buf_size = 128;
    char *buf = (char *)wasm_alloc((unsigned int)buf_size);
    _wf_memcpy(buf, "Set({", 5);
    int pos = 5;
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) { buf[pos++] = ','; buf[pos++] = ' '; }
        const char *vs = (const char *)(intptr_t)taida_polymorphic_to_string(list[WASM_LIST_ELEMS + i]);
        int vl = _wf_strlen(vs);
        while (pos + vl + 10 > buf_size) {
            buf_size *= 2;
            char *new_buf = (char *)wasm_alloc((unsigned int)buf_size);
            _wf_memcpy(new_buf, buf, pos);
            buf = new_buf;
        }
        _wf_memcpy(buf + pos, vs, vl); pos += vl;
    }
    buf[pos++] = '}'; buf[pos++] = ')';
    buf[pos] = '\0';
    return (int64_t)buf;
}

/* ── WC-6c: Pack extensions ──────────────────────────────── */

/* Pack call field: invoke a BuchiPack field as a function */
int64_t taida_pack_call_field0(int64_t pack_ptr, int64_t field_hash) {
    int64_t fn_ptr = taida_pack_get(pack_ptr, field_hash);
    if (taida_is_closure_value(fn_ptr)) {
        int64_t env = taida_closure_get_env(fn_ptr);
        typedef int64_t (*cb)(int64_t);
        return ((cb)(intptr_t)taida_closure_get_fn(fn_ptr))(env);
    }
    typedef int64_t (*cb0)(void);
    return ((cb0)(intptr_t)fn_ptr)();
}

int64_t taida_pack_call_field1(int64_t pack_ptr, int64_t field_hash, int64_t a) {
    int64_t fn_ptr = taida_pack_get(pack_ptr, field_hash);
    return taida_invoke_callback1(fn_ptr, a);
}

int64_t taida_pack_call_field2(int64_t pack_ptr, int64_t field_hash, int64_t a, int64_t b) {
    int64_t fn_ptr = taida_pack_get(pack_ptr, field_hash);
    return taida_invoke_callback2(fn_ptr, a, b);
}

int64_t taida_pack_call_field3(int64_t pack_ptr, int64_t field_hash, int64_t a, int64_t b, int64_t c) {
    int64_t fn_ptr = taida_pack_get(pack_ptr, field_hash);
    if (taida_is_closure_value(fn_ptr)) {
        int64_t env = taida_closure_get_env(fn_ptr);
        typedef int64_t (*cb)(int64_t, int64_t, int64_t, int64_t);
        return ((cb)(intptr_t)taida_closure_get_fn(fn_ptr))(env, a, b, c);
    }
    typedef int64_t (*cb3)(int64_t, int64_t, int64_t);
    return ((cb3)(intptr_t)fn_ptr)(a, b, c);
}

/* Pack.toString() */
int64_t taida_pack_to_display_string(int64_t pack_ptr) {
    return taida_polymorphic_to_string(pack_ptr);
}

/* make_io_error(msg) -> Error BuchiPack */
int64_t taida_make_io_error(int64_t msg) {
    return taida_make_error(
        (int64_t)(intptr_t)"IOError",
        msg);
}

/* retain_and_tag_field -- no-op in WASM (no RC) */
int64_t taida_retain_and_tag_field(int64_t val, int64_t tag) {
    (void)val; (void)tag;
    return 0;
}

/* ── WC-6d: Type detection & display functions ───────────── */

int64_t taida_is_string_value(int64_t val) { return _looks_like_string(val); }
int64_t taida_is_list(int64_t val) { return _looks_like_list(val); }
int64_t taida_is_hashmap(int64_t val) { return _is_wasm_hashmap(val); }
int64_t taida_is_set(int64_t val) { return _is_wasm_set(val); }
int64_t taida_is_buchi_pack(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 8)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    int64_t fc = p[0];
    if (fc > 0 && fc < 200) {
        int64_t h = p[1];
        if (h > 0x10000 || h < 0) return 1;
    }
    return 0;
}
int64_t taida_is_molten(int64_t val) { (void)val; return 0; /* Molten is JS-only */ }
int64_t taida_is_bytes(int64_t val) { (void)val; return 0; /* Bytes not in core */ }
int64_t taida_is_async(int64_t val) { return _wasm_is_async_obj(val); }
int64_t taida_detect_value_tag(int64_t val) { (void)val; return 0; }
int64_t taida_detect_gorillax_type(int64_t val) { (void)val; return 0; }
int64_t taida_bool_to_int(int64_t v) { return v ? 1 : 0; }
int64_t taida_bool_to_str(int64_t v) {
    return taida_str_new_copy((int64_t)(intptr_t)(v ? "true" : "false"));
}
int64_t taida_value_to_display_string(int64_t val) { return taida_polymorphic_to_string(val); }
int64_t taida_value_to_debug_string(int64_t val) { return taida_polymorphic_to_string(val); }
int64_t taida_has_magic_header(int64_t val) { (void)val; return 0; }
int64_t taida_ptr_is_readable(int64_t val) { return _wasm_is_valid_ptr(val, 1); }
int64_t taida_read_cstr_len_safe(int64_t ptr, int64_t max) {
    if (!_wasm_is_valid_ptr(ptr, 1)) return 0;
    const char *s = (const char *)(intptr_t)ptr;
    int len = 0;
    while (len < (int)max && s[len]) len++;
    if (len >= (int)max) return 0;
    return (int64_t)len;
}

/* ── WC-6e: Polymorphic extensions ───────────────────────── */

/* Lax.getOrDefault(fallback) -- helper for polymorphic_get_or_default */
static int64_t _wc_lax_get_or_default(int64_t lax_ptr, int64_t fallback) {
    if (taida_pack_get_idx(lax_ptr, 0)) {
        return taida_pack_get_idx(lax_ptr, 1); /* __value */
    }
    return fallback;
}

/* Result.getOrDefault(fallback) -- helper for polymorphic_get_or_default */
static int64_t _wc_result_get_or_default(int64_t result, int64_t def) {
    if (taida_result_is_ok(result)) {
        return taida_pack_get_idx(result, 0); /* __value */
    }
    return def;
}

/* Polymorphic .getOrDefault(fallback) -- works on Result, Lax, Async, raw values */
int64_t taida_polymorphic_get_or_default(int64_t obj, int64_t def) {
    if (obj == 0) return def;
    if (obj < 0) return obj; /* negative ints are valid values */
    if (_wasm_is_async_obj(obj)) return taida_async_get_or_default(obj, def);
    if (_wasm_is_valid_ptr(obj, 104)) {
        if (_wasm_is_result(obj)) return _wc_result_get_or_default(obj, def);
        if (_wasm_is_lax(obj)) return _wc_lax_get_or_default(obj, def);
    }
    return obj;
}

/* Polymorphic .hasValue() -- works for Lax, Gorillax, Result (fc=4 monadic types) */
int64_t taida_polymorphic_has_value(int64_t obj) {
    if (obj == 0) return 0;
    if (!_wasm_is_valid_ptr(obj, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)obj;
    if (p[0] == 4) return taida_pack_get_idx(obj, 0); /* fc=4: Lax/Gorillax/Result */
    return 0;
}

/* Polymorphic .contains() */
int64_t taida_polymorphic_contains(int64_t obj, int64_t needle) {
    if (obj == 0) return 0;
    if (_is_wasm_hashmap(obj)) return taida_hashmap_has(obj, taida_value_hash(needle), needle);
    if (_is_wasm_set(obj)) return taida_set_has(obj, needle);
    if (_looks_like_list(obj)) return taida_list_contains(obj, needle);
    if (_looks_like_string(obj)) return taida_str_contains(obj, needle);
    return 0;
}

/* Polymorphic .indexOf() */
int64_t taida_polymorphic_index_of(int64_t obj, int64_t needle) {
    if (obj == 0) return -1;
    if (_looks_like_list(obj)) return taida_list_index_of(obj, needle);
    if (_looks_like_string(obj)) return taida_str_index_of(obj, needle);
    return -1;
}

/* Polymorphic .lastIndexOf() */
int64_t taida_polymorphic_last_index_of(int64_t obj, int64_t needle) {
    if (obj == 0) return -1;
    if (_looks_like_list(obj)) return taida_list_last_index_of(obj, needle);
    if (_looks_like_string(obj)) return taida_str_last_index_of(obj, needle);
    return -1;
}

/* Polymorphic .map(fn) */
int64_t taida_polymorphic_map(int64_t obj, int64_t fn_ptr) {
    if (obj == 0) return obj;
    if (_wasm_is_async_obj(obj)) return taida_async_map(obj, fn_ptr);
    if (_wasm_is_result(obj)) {
        if (taida_result_is_ok(obj)) {
            int64_t value = taida_pack_get_idx(obj, 0);
            int64_t new_val = taida_invoke_callback1(fn_ptr, value);
            return taida_result_create(new_val, 0, 0);
        }
        return obj;
    }
    if (_wasm_is_lax(obj)) {
        if (!taida_pack_get_idx(obj, 0)) return obj; /* empty lax */
        int64_t value = taida_pack_get_idx(obj, 1);
        int64_t def = taida_pack_get_idx(obj, 2);
        int64_t result = taida_invoke_callback1(fn_ptr, value);
        return taida_lax_new(result, def);
    }
    /* Default: list.map */
    return taida_list_map(obj, fn_ptr);
}

/* ── W-5: Closure runtime ────────────────────────────────── */
/* Closure layout: [fn_ptr, env_ptr]
   No magic header or refcount in wasm-min (bump allocator, no free). */

#define WASM_CLOSURE_MARKER 0x434C4F53ULL /* "CLOS" */

int64_t taida_closure_new(int64_t fn_ptr, int64_t env_ptr, int64_t user_arity) {
    int64_t *closure = (int64_t *)wasm_alloc(4 * 8);
    if (!closure) return 0;
    closure[0] = (int64_t)WASM_CLOSURE_MARKER;
    closure[1] = fn_ptr;
    closure[2] = env_ptr;
    closure[3] = user_arity; /* W-5g: number of user args (excluding __env) */
    return (int64_t)(intptr_t)closure;
}

int64_t taida_closure_get_fn(int64_t closure_ptr) {
    int64_t *c = (int64_t *)(intptr_t)closure_ptr;
    return c[1];
}

int64_t taida_closure_get_env(int64_t closure_ptr) {
    int64_t *c = (int64_t *)(intptr_t)closure_ptr;
    return c[2];
}

int64_t taida_is_closure_value(int64_t val) {
    /* W-5g: bounds check before dereference (closure is 4 * int64_t = 32 bytes) */
    if (!_wasm_is_valid_ptr(val, 32)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    return (p[0] == (int64_t)WASM_CLOSURE_MARKER) ? 1 : 0;
}

/* ── W-5: Error ceiling (error-flag based, no setjmp/longjmp) ── */
/* WASM freestanding mode does not have setjmp.h. Instead we use an
   error-flag approach: taida_throw sets a global flag and stores the
   error value. taida_error_try_call wraps a function pointer call and
   checks the flag after return. Functions that may throw must check
   the error flag and propagate it via early return.

   For wasm-min's simpler use case, we implement the same API as native
   but with setjmp emulated via the error flag + wrapper function approach.
   taida_error_try_call calls the function; if taida_throw was invoked,
   the function returns normally (with a dummy value) and try_call detects
   the flag was set. */

static int64_t __wasm_error_val[64];
static int64_t __wasm_try_result[64];
static int __wasm_error_depth = 0;
static int __wasm_error_thrown = 0;

int64_t taida_error_ceiling_push(void) {
    if (__wasm_error_depth >= 64) {
        /* overflow: crash */
        const char *msg = "Error: maximum error handling depth exceeded\n";
        write_stdout(msg, wasm_strlen(msg));
        __builtin_trap();
    }
    int depth = __wasm_error_depth++;
    return (int64_t)depth;
}

void taida_error_ceiling_pop(void) {
    if (__wasm_error_depth > 0) __wasm_error_depth--;
    __wasm_error_thrown = 0;
}

/* RCB-101 fix: expose error-thrown flag so generated C code (separate
   compilation unit) can check after taida_error_type_check_or_rethrow. */
int64_t taida_is_error_thrown(void) {
    return __wasm_error_thrown ? 1 : 0;
}

int64_t taida_throw(int64_t error_val) {
    if (__wasm_error_depth > 0) {
        int depth = __wasm_error_depth - 1;
        __wasm_error_val[depth] = error_val;
        __wasm_error_thrown = 1;
        return 0; /* caller should check __wasm_error_thrown */
    }
    /* No error ceiling: gorilla crash */
    const char *msg = "Unhandled error (no error ceiling)\n";
    write_stdout(msg, wasm_strlen(msg));
    __builtin_trap();
    return 0;
}

/* taida_error_try_call: call fn_ptr(env_ptr) under error ceiling protection.
   Returns 0 if normal, 1 if error was thrown. */
int64_t taida_error_try_call(int64_t fn_ptr, int64_t env_ptr, int64_t depth) {
    typedef int64_t (*fn_t)(int64_t);
    fn_t func = (fn_t)(intptr_t)fn_ptr;
    __wasm_error_thrown = 0;
    int64_t result = func(env_ptr);
    if (__wasm_error_thrown) {
        __wasm_error_thrown = 0;
        return 1; /* error caught */
    }
    __wasm_try_result[(int)depth] = result;
    return 0; /* normal completion */
}

int64_t taida_error_try_get_result(int64_t depth) {
    return __wasm_try_result[(int)depth];
}

int64_t taida_error_setjmp(int64_t depth) {
    /* Legacy compat — not used in wasm-min's error flow */
    (void)depth;
    return 0;
}

int64_t taida_error_get_value(int64_t depth) {
    return __wasm_error_val[(int)depth];
}

/* RCB-101: Inheritance parent registry for error type filtering in |== */
/* Dynamic array using wasm_alloc (bump allocator, copy-on-grow). */
static int64_t *__wasm_type_parent_child = 0;
static int64_t *__wasm_type_parent_parent = 0;
static int __wasm_type_parent_count = 0;
static int __wasm_type_parent_cap = 0;

void taida_register_type_parent(int64_t child_str, int64_t parent_str) {
    if (__wasm_type_parent_count >= __wasm_type_parent_cap) {
        int new_cap = __wasm_type_parent_cap == 0 ? 64 : __wasm_type_parent_cap * 2;
        int64_t *new_child = (int64_t*)wasm_alloc(sizeof(int64_t) * new_cap);
        int64_t *new_parent = (int64_t*)wasm_alloc(sizeof(int64_t) * new_cap);
        if (!new_child || !new_parent) return;
        /* Copy old entries (bump allocator cannot realloc, so we copy) */
        for (int i = 0; i < __wasm_type_parent_count; i++) {
            new_child[i] = __wasm_type_parent_child[i];
            new_parent[i] = __wasm_type_parent_parent[i];
        }
        __wasm_type_parent_child = new_child;
        __wasm_type_parent_parent = new_parent;
        __wasm_type_parent_cap = new_cap;
    }
    __wasm_type_parent_child[__wasm_type_parent_count] = child_str;
    __wasm_type_parent_parent[__wasm_type_parent_count] = parent_str;
    __wasm_type_parent_count++;
}

static int64_t wasm_find_parent_type(int64_t child_str) {
    for (int i = 0; i < __wasm_type_parent_count; i++) {
        if (taida_str_eq(__wasm_type_parent_child[i], child_str)) {
            return __wasm_type_parent_parent[i];
        }
    }
    return 0;
}

int64_t taida_error_type_matches(int64_t error_val, int64_t handler_type_str) {
    /* "Error" catches everything */
    const char *handler_s = (const char*)(intptr_t)handler_type_str;
    if (handler_s && _wf_strcmp(handler_s, "Error") == 0) return 1;

    /* Get the thrown type from __type field of the BuchiPack.
       Fall back to "type" field if __type is absent (legacy errors). */
    int64_t thrown_type_str = 0;
    if (taida_is_buchi_pack(error_val)) {
        if (taida_pack_has_hash(error_val, WASM_HASH___TYPE)) {
            thrown_type_str = taida_pack_get(error_val, WASM_HASH___TYPE);
        } else if (taida_pack_has_hash(error_val, WASM_HASH_TYPE)) {
            thrown_type_str = taida_pack_get(error_val, WASM_HASH_TYPE);
        }
    }
    /* RCB-101 fix: unknown type must NOT be catch-all.  Only the "Error"
       handler (checked above) catches everything. */
    if (thrown_type_str == 0) return 0;

    /* Walk inheritance chain */
    int64_t current = thrown_type_str;
    for (int i = 0; i < 64; i++) {
        if (taida_str_eq(current, handler_type_str)) return 1;
        int64_t parent = wasm_find_parent_type(current);
        if (parent == 0) break;
        current = parent;
    }
    return 0;
}

/* ── W-5: Error object creation ── */
/* FNV-1a hashes for error BuchiPack fields (same as native_runtime.c) */
/* WFX-2: corrected FNV-1a hashes for error fields */
/* WASM_HASH_TYPE and WASM_HASH_MESSAGE are defined in the "Error field hashes" section */
#define WASM_HASH_FIELD     0x2c5d047ff4e6ffc7LL  /* FNV-1a("field") */
#define WASM_HASH_CODE      0x0bb51791194b4414LL  /* FNV-1a("code") */

static void _wasm_register_builtin_error_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;

    taida_register_field_name(WASM_HASH_TYPE, (int64_t)(intptr_t)"type");
    taida_register_field_name(WASM_HASH_MESSAGE, (int64_t)(intptr_t)"message");
    taida_register_field_name(WASM_HASH_FIELD, (int64_t)(intptr_t)"field");
    taida_register_field_name(WASM_HASH_CODE, (int64_t)(intptr_t)"code");
    taida_register_field_name(
        taida_str_hash((int64_t)(intptr_t)"kind"),
        (int64_t)(intptr_t)"kind"
    );
}

int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr) {
    _wasm_register_builtin_error_field_names();

    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, WASM_HASH_TYPE);
    taida_pack_set(pack, 0, type_ptr);
    taida_pack_set_hash(pack, 1, WASM_HASH_MESSAGE);
    taida_pack_set(pack, 1, msg_ptr);
    /* RCB-101 fix: Set __type field so error type matching works.
       Without this, taida_error_type_matches falls through to catch-all
       because it looks for __type, not type. */
    taida_pack_set_hash(pack, 2, WASM_HASH___TYPE);
    taida_pack_set(pack, 2, type_ptr);
    return pack;
}

/* ── W-5: Lax[T] runtime ────────────────────────────────── */
/* Lax is a BuchiPack @(hasValue: Bool, __value: T, __default: T, __type: Str)
   Layout: 4-field pack using same hash constants as native. */

/* WASM_HASH_HAS_VALUE, __VALUE, __DEFAULT, __TYPE defined in W-5f monadic type hash section */

int64_t taida_lax_new(int64_t value, int64_t default_value) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1);  /* hasValue = true */
    taida_pack_set_tag(pack, 0, 2); /* BOOL tag */
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    taida_pack_set_hash(pack, 2, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Lax");
    return pack;
}

int64_t taida_lax_empty(int64_t default_value) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);  /* hasValue = false */
    taida_pack_set_tag(pack, 0, 2); /* BOOL tag */
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, default_value);
    taida_pack_set_hash(pack, 2, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Lax");
    return pack;
}

int64_t taida_lax_has_value(int64_t lax_ptr) {
    return taida_pack_get_idx(lax_ptr, 0);  /* hasValue field */
}

int64_t taida_lax_get_or_default(int64_t lax_ptr, int64_t fallback) {
    if (taida_pack_get_idx(lax_ptr, 0)) {
        return taida_pack_get_idx(lax_ptr, 1);  /* __value */
    }
    return fallback;
}

int64_t taida_lax_unmold(int64_t lax_ptr) {
    if (taida_pack_get_idx(lax_ptr, 0)) {
        return taida_pack_get_idx(lax_ptr, 1);  /* __value */
    }
    return taida_pack_get_idx(lax_ptr, 2);  /* __default */
}

int64_t taida_lax_is_empty(int64_t lax_ptr) {
    return taida_pack_get_idx(lax_ptr, 0) ? 0 : 1;
}

/* ── W-5: generic_unmold — now Lax-aware ── */
/* Override the simplified version from W-1. When the value is a Lax pack
   (detected by field count == 4 and hasValue field), extract the value;
   otherwise return identity. */

/* Forward declare: check if a value is a Lax pack */
static int _wasm_is_lax(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    /* Check if it looks like a pack with 4 fields and first hash = HASH_HAS_VALUE */
    if (p[0] == 4 && p[1] == WASM_HASH_HAS_VALUE) return 1;
    return 0;
}

/* ── W-5: Gorillax (Result container) ── */
/* Gorillax: @(isOk: Bool, __value: T, __error: Error, __type: "Gorillax")
   Using pack fields at fixed indices. */

/* WASM_HASH_IS_OK, __ERROR defined in W-5f monadic type hash section */

int64_t taida_gorillax_new(int64_t value) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_IS_OK);
    taida_pack_set(pack, 0, 1); /* isOk = true */
    taida_pack_set_tag(pack, 0, 2); /* BOOL */
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, 0);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Gorillax");
    return pack;
}

int64_t taida_gorillax_err(int64_t error) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_IS_OK);
    taida_pack_set(pack, 0, 0); /* isOk = false */
    taida_pack_set_tag(pack, 0, 2); /* BOOL */
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, 0);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, error);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Gorillax");
    return pack;
}

int64_t taida_gorillax_is_ok(int64_t gx) {
    return taida_pack_get_idx(gx, 0);
}

int64_t taida_gorillax_get_value(int64_t gx) {
    return taida_pack_get_idx(gx, 1);
}

int64_t taida_gorillax_get_error(int64_t gx) {
    return taida_pack_get_idx(gx, 2);
}

int64_t taida_gorillax_relax(int64_t gx) {
    /* RelaxedGorillax: same layout, just change __type */
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_IS_OK);
    taida_pack_set(pack, 0, taida_pack_get_idx(gx, 0));
    taida_pack_set_tag(pack, 0, 2);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, taida_pack_get_idx(gx, 1));
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, taida_pack_get_idx(gx, 2));
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"RelaxedGorillax");
    return pack;
}

int64_t taida_relaxed_gorillax_new(int64_t value) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_IS_OK);
    taida_pack_set(pack, 0, 1);
    taida_pack_set_tag(pack, 0, 2);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, 0);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"RelaxedGorillax");
    return pack;
}

int64_t taida_relaxed_gorillax_err(int64_t error) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_IS_OK);
    taida_pack_set(pack, 0, 0);
    taida_pack_set_tag(pack, 0, 2);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, 0);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, error);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"RelaxedGorillax");
    return pack;
}

/* ── W-5: Result[T, P] ── */
/* Result: @(__value: T, __predicate: P, throw: Error, __type: "Result")
   field 0: __value, field 1: __predicate, field 2: throw, field 3: __type */

/* WASM_HASH___PREDICATE, WASM_HASH_THROW defined in W-5f monadic type hash section */

int64_t taida_result_create(int64_t value, int64_t throw_val, int64_t predicate) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH___VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, WASM_HASH___PREDICATE);
    taida_pack_set(pack, 1, predicate);
    taida_pack_set_hash(pack, 2, WASM_HASH_THROW);
    taida_pack_set(pack, 2, throw_val);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Result");
    return pack;
}

/* W-5g: Helper — check if Result has error (matching native taida_result_is_error_check).
   1. If throw is set (not 0), it's an error — UNLESS predicate passes
   2. If predicate exists, evaluate P(value) — true = success, false = error
   3. No predicate + no throw = success (backward compatible) */
static int64_t _wasm_result_is_error_check(int64_t result) {
    int64_t throw_val = taida_pack_get_idx(result, 2); /* throw */
    int64_t pred = taida_pack_get_idx(result, 1);      /* __predicate */
    int64_t value = taida_pack_get_idx(result, 0);     /* __value */

    if (throw_val != 0) {
        if (pred != 0) {
            int64_t pred_result = _wasm_invoke_callback1(pred, value);
            if (!pred_result) return 1; /* predicate failed — error */
            return 0; /* predicate passed even though throw was set — success */
        }
        return 1; /* throw set, no predicate — error */
    }
    if (pred != 0) {
        int64_t pred_result = _wasm_invoke_callback1(pred, value);
        return pred_result ? 0 : 1;
    }
    return 0; /* no throw, no predicate — success */
}

int64_t taida_result_is_ok(int64_t result) {
    return _wasm_result_is_error_check(result) ? 0 : 1;
}

int64_t taida_result_is_error(int64_t result) {
    return _wasm_result_is_error_check(result);
}

int64_t taida_result_map_error(int64_t result, int64_t fn_ptr) {
    if (!_wasm_result_is_error_check(result)) {
        return result; /* Success: return as-is */
    }
    int64_t throw_val = taida_pack_get_idx(result, 2); /* throw field */
    /* Extract the error message string to pass to the mapping function
       (matching native: passes display string, not the Error BuchiPack) */
    int64_t err_display = _wasm_throw_to_display_string(throw_val);
    int64_t mapped_str = taida_invoke_callback1(fn_ptr, err_display);
    /* Wrap the mapped result back into an Error BuchiPack */
    int64_t new_error = taida_make_error(
        (int64_t)(intptr_t)"ResultError", mapped_str);
    return taida_result_create(0, new_error, 0);
}

/* =========================================================================
 * WC-5a: Lax extended ops (prelude — all profiles)
 * ========================================================================= */

/* Forward declare taida_invoke_callback1 (defined below in Cage section) */
int64_t taida_invoke_callback1(int64_t fn_ptr, int64_t arg0);

/// Lax.map(fn)
int64_t taida_lax_map(int64_t lax_ptr, int64_t fn_ptr) {
    if (!taida_pack_get_idx(lax_ptr, 0)) {
        int64_t def = taida_pack_get_idx(lax_ptr, 2);
        return taida_lax_empty(def);
    }
    int64_t value = taida_pack_get_idx(lax_ptr, 1);
    int64_t def = taida_pack_get_idx(lax_ptr, 2);
    int64_t result = taida_invoke_callback1(fn_ptr, value);
    return taida_lax_new(result, def);
}

/// Lax.flatMap(fn)
int64_t taida_lax_flat_map(int64_t lax_ptr, int64_t fn_ptr) {
    if (!taida_pack_get_idx(lax_ptr, 0)) {
        int64_t def = taida_pack_get_idx(lax_ptr, 2);
        return taida_lax_empty(def);
    }
    int64_t value = taida_pack_get_idx(lax_ptr, 1);
    return taida_invoke_callback1(fn_ptr, value);
}

/// Lax.toString() — public wrapper for _wasm_lax_to_string
int64_t taida_lax_to_string(int64_t lax_ptr) {
    return _wasm_lax_to_string(lax_ptr);
}

/* =========================================================================
 * WC-5b: Result extended ops (prelude — all profiles)
 * ========================================================================= */

/// Result.isError() check — public wrapper
int64_t taida_result_is_error_check(int64_t result) {
    return _wasm_result_is_error_check(result);
}

/// Result.getOrDefault(fallback)
int64_t taida_result_get_or_default(int64_t result, int64_t def) {
    if (!_wasm_result_is_error_check(result)) return taida_pack_get_idx(result, 0);
    return def;
}

/// Result.map(fn)
int64_t taida_result_map(int64_t result, int64_t fn_ptr) {
    if (_wasm_result_is_error_check(result)) return result;
    int64_t value = taida_pack_get_idx(result, 0);
    int64_t new_val = taida_invoke_callback1(fn_ptr, value);
    return taida_result_create(new_val, 0, 0);
}

/// Result.flatMap(fn)
int64_t taida_result_flat_map(int64_t result, int64_t fn_ptr) {
    if (_wasm_result_is_error_check(result)) return result;
    int64_t value = taida_pack_get_idx(result, 0);
    return taida_invoke_callback1(fn_ptr, value);
}

/// Result.getOrThrow()
int64_t taida_result_get_or_throw(int64_t result) {
    if (!_wasm_result_is_error_check(result)) {
        return taida_pack_get_idx(result, 0);
    }
    int64_t throw_val = taida_pack_get_idx(result, 2);
    if (taida_can_throw_payload(throw_val)) {
        return taida_throw(throw_val);
    }
    int64_t error = taida_make_error(
        (int64_t)(intptr_t)"ResultError",
        (int64_t)(intptr_t)"Result predicate failed");
    return taida_throw(error);
}

/// Result.toString() — public wrapper for _wasm_result_to_string
int64_t taida_result_to_string(int64_t result) {
    return _wasm_result_to_string(result);
}

/* =========================================================================
 * WC-5c: Gorillax extended ops (prelude — all profiles)
 * ========================================================================= */

/// Gorillax.unmold()
int64_t taida_gorillax_unmold(int64_t ptr) {
    if (taida_pack_get_idx(ptr, 0)) {
        return taida_pack_get_idx(ptr, 1);
    }
    /* GORILLA — terminate via WASI fd_write + proc_exit */
    extern int fd_write(int fd, const void *iovs, int iovs_len, int *nwritten)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")));
    const char *msg = "><\n";
    struct { const char *buf; int len; } iov = { msg, 3 };
    int nwritten;
    fd_write(2, &iov, 1, &nwritten);
    extern void proc_exit(int code)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("proc_exit")));
    proc_exit(1);
    return 0;
}

/// Gorillax.toString() — public wrapper for _wasm_gorillax_to_string
int64_t taida_gorillax_to_string(int64_t ptr) {
    return _wasm_gorillax_to_string(ptr);
}

/// RelaxedGorillax.unmold()
int64_t taida_relaxed_gorillax_unmold(int64_t ptr) {
    if (taida_pack_get_idx(ptr, 0)) {
        return taida_pack_get_idx(ptr, 1);
    }
    int64_t error = taida_make_error(
        (int64_t)(intptr_t)"RelaxedGorillaEscaped",
        (int64_t)(intptr_t)"Relaxed gorilla escaped");
    return taida_throw(error);
}

/// RelaxedGorillax.toString() — public wrapper for _wasm_gorillax_to_string
int64_t taida_relaxed_gorillax_to_string(int64_t ptr) {
    return _wasm_gorillax_to_string(ptr);
}

/* =========================================================================
 * WC-5d: Monadic ops (prelude — all profiles)
 * ========================================================================= */

/// Monadic field_count (for dispatch)
int64_t taida_monadic_field_count(int64_t val) {
    if (val == 0 || val < WASM_MIN_HEAP_ADDR) return 0;
    if (_wasm_is_result(val)) return 3;
    if (_wasm_is_lax(val)) return 4;
    return 0;
}

/// Monadic .flatMap(fn)
int64_t taida_monadic_flat_map(int64_t obj, int64_t fn_ptr) {
    if (obj == 0 || obj < WASM_MIN_HEAP_ADDR) return obj;
    if (_wasm_is_result(obj)) {
        if (!taida_result_is_ok(obj)) return obj;
        int64_t value = taida_pack_get_idx(obj, 0);
        return taida_invoke_callback1(fn_ptr, value);
    }
    if (_wasm_is_lax(obj)) {
        if (!taida_pack_get_idx(obj, 0)) return obj;
        int64_t value = taida_pack_get_idx(obj, 1);
        return taida_invoke_callback1(fn_ptr, value);
    }
    return obj;
}

/// Monadic .getOrThrow()
int64_t taida_monadic_get_or_throw(int64_t obj) {
    if (obj == 0 || obj < WASM_MIN_HEAP_ADDR) return obj;
    if (_wasm_is_result(obj)) {
        if (taida_result_is_ok(obj)) return taida_pack_get_idx(obj, 0);
        int64_t throw_val = taida_pack_get_idx(obj, 2);
        if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
        int64_t error = taida_make_error(
            (int64_t)(intptr_t)"ResultError",
            (int64_t)(intptr_t)"Result predicate failed");
        return taida_throw(error);
    }
    if (_wasm_is_lax(obj)) return taida_lax_unmold(obj);
    return obj;
}

/// Monadic .toString()
int64_t taida_monadic_to_string(int64_t obj) {
    return taida_polymorphic_to_string(obj);
}

/* ── W-5: Cage ── */

/* Callback invoker helpers for wasm-min
 * W-5g: In WASM, indirect call type signature must match exactly.
 * Zero-param lambdas (_ = expr) have user_arity=0, so the closure function
 * only takes (__env). We must dispatch based on arity to avoid type mismatch. */
static int64_t _wasm_invoke_callback1(int64_t fn_ptr, int64_t arg0) {
    if (taida_is_closure_value(fn_ptr)) {
        int64_t *closure = (int64_t *)(intptr_t)fn_ptr;
        int64_t user_arity = closure[3];
        if (user_arity == 0) {
            /* Zero-param lambda: call with env only, ignore arg0 */
            typedef int64_t (*closure_fn0_t)(int64_t);
            closure_fn0_t func = (closure_fn0_t)(intptr_t)closure[1];
            return func(closure[2]);
        }
        /* 1+ param lambda: call with env + arg0 */
        typedef int64_t (*closure_fn1_t)(int64_t, int64_t);
        closure_fn1_t func = (closure_fn1_t)(intptr_t)closure[1];
        return func(closure[2], arg0);
    }
    typedef int64_t (*fn_t)(int64_t);
    fn_t func = (fn_t)(intptr_t)fn_ptr;
    return func(arg0);
}

/* RCB-101: Check error type and re-throw if it does not match.
   If the type matches, returns the error_val unchanged.
   If it does not match, calls taida_throw(error_val) which sets the error flag (never returns normally). */
int64_t taida_error_type_check_or_rethrow(int64_t error_val, int64_t handler_type_str) {
    if (taida_error_type_matches(error_val, handler_type_str)) {
        return error_val;
    }
    /* Re-throw: sets __wasm_error_thrown flag */
    taida_throw(error_val);
    return 0; /* caller should check __wasm_error_thrown */
}

int64_t taida_cage_apply(int64_t cage_value, int64_t fn_ptr) {
    if (fn_ptr == 0) {
        int64_t error = taida_make_error(
            (int64_t)(intptr_t)"CageError",
            (int64_t)(intptr_t)"Cage second argument must be a function");
        return taida_gorillax_err(error);
    }

    int64_t depth = taida_error_ceiling_push();
    __wasm_error_thrown = 0;
    int64_t result = _wasm_invoke_callback1(fn_ptr, cage_value);
    if (__wasm_error_thrown) {
        int64_t error = taida_error_get_value(depth);
        taida_error_ceiling_pop();
        if (error == 0) {
            error = taida_make_error(
                (int64_t)(intptr_t)"CageError",
                (int64_t)(intptr_t)"Cage function failed");
        }
        return taida_gorillax_err(error);
    }
    taida_error_ceiling_pop();
    return taida_gorillax_new(result);
}

/* ── W-5: Molten/Stub/Todo stubs ── */

int64_t taida_molten_new(void) {
    int64_t pack = taida_pack_new(1);
    taida_pack_set_hash(pack, 0, WASM_HASH___TYPE);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)"Molten");
    return pack;
}

int64_t taida_stub_new(int64_t message) {
    (void)message;
    return taida_molten_new();
}

int64_t taida_todo_new(int64_t id, int64_t task, int64_t sol, int64_t unm) {
    /* BE-WASM-1: proper TODO pack matching native_runtime.c layout.
       Fields: id(0), task(1), sol(2), unm(3), __value(4), __default(5), __type(6) */
    int64_t pack = taida_pack_new(7);
    taida_pack_set_hash(pack, 0, WASM_HASH_TODO_ID);
    taida_pack_set(pack, 0, id);
    taida_pack_set_hash(pack, 1, WASM_HASH_TODO_TASK);
    taida_pack_set(pack, 1, task);
    taida_pack_set_hash(pack, 2, WASM_HASH_TODO_SOL);
    taida_pack_set(pack, 2, sol);
    taida_pack_set_hash(pack, 3, WASM_HASH_TODO_UNM);
    taida_pack_set(pack, 3, unm);
    taida_pack_set_hash(pack, 4, WASM_HASH___VALUE);
    taida_pack_set(pack, 4, sol);
    taida_pack_set_hash(pack, 5, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 5, unm);
    taida_pack_set_hash(pack, 6, WASM_HASH___TYPE);
    taida_pack_set(pack, 6, (int64_t)(intptr_t)"TODO");
    return pack;
}

/* BE-WASM-2: Gorilla literal — immediate crash (matching native exit(1)).
   In WASM, __builtin_trap() produces an unreachable instruction that
   terminates the module, which is the WASM equivalent of exit(). */
void taida_gorilla(void) {
    /* No output — matches native exit(1) behavior */
    __builtin_trap();
}

/* ── W-5: Type conversion molds (returning Lax) ── */
/* These wrap taida_lax_new with conversion logic, matching native_runtime.c */

int64_t taida_str_mold_int(int64_t v) {
    return taida_lax_new(taida_int_to_str(v), (int64_t)(intptr_t)"");
}

int64_t taida_str_mold_float(int64_t v) {
    return taida_lax_new(taida_float_to_str(v), (int64_t)(intptr_t)"");
}

int64_t taida_str_mold_bool(int64_t v) {
    return taida_lax_new(taida_str_from_bool(v), (int64_t)(intptr_t)"");
}

int64_t taida_str_mold_str(int64_t v) {
    return taida_lax_new(v, (int64_t)(intptr_t)"");
}

int64_t taida_int_mold_int(int64_t v) {
    return taida_lax_new(v, 0);
}

int64_t taida_int_mold_float(int64_t v) {
    return taida_lax_new(taida_float_to_int(v), 0);
}

int64_t taida_int_mold_bool(int64_t v) {
    return taida_lax_new(v != 0 ? 1 : 0, 0);
}

int64_t taida_float_mold_int(int64_t v) {
    int64_t lax = taida_lax_new(taida_int_to_float(v), _d2l(0.0));
    taida_pack_set_tag(lax, 1, WASM_TAG_FLOAT); /* __value tag */
    taida_pack_set_tag(lax, 2, WASM_TAG_FLOAT); /* __default tag */
    return lax;
}

int64_t taida_float_mold_float(int64_t v) {
    int64_t lax = taida_lax_new(v, _d2l(0.0));
    taida_pack_set_tag(lax, 1, WASM_TAG_FLOAT);
    taida_pack_set_tag(lax, 2, WASM_TAG_FLOAT);
    return lax;
}

/* Helper: create a Lax with FLOAT tags on value/default fields */
static int64_t _float_lax_empty(void) {
    int64_t lax = taida_lax_empty(_d2l(0.0));
    taida_pack_set_tag(lax, 1, WASM_TAG_FLOAT);
    taida_pack_set_tag(lax, 2, WASM_TAG_FLOAT);
    return lax;
}

static int64_t _float_lax_new(int64_t val) {
    int64_t lax = taida_lax_new(val, _d2l(0.0));
    taida_pack_set_tag(lax, 1, WASM_TAG_FLOAT);
    taida_pack_set_tag(lax, 2, WASM_TAG_FLOAT);
    return lax;
}

int64_t taida_float_mold_str(int64_t v) {
    /* Parse string to float — manual parser (no strtod in wasm freestanding) */
    const char *s = (const char *)(intptr_t)v;
    if (!s || *s == '\0') return _float_lax_empty();

    int i = 0;
    int negative = 0;
    if (s[i] == '-') { negative = 1; i++; }
    else if (s[i] == '+') { i++; }

    /* Must start with digit or '.' */
    if (!((s[i] >= '0' && s[i] <= '9') || s[i] == '.'))
        return _float_lax_empty();

    double result = 0.0;
    int has_int_digits = 0;
    /* Integer part */
    while (s[i] >= '0' && s[i] <= '9') {
        result = result * 10.0 + (s[i] - '0');
        has_int_digits = 1;
        i++;
    }
    /* Fractional part */
    int has_frac_digits = 0;
    if (s[i] == '.') {
        i++;
        double frac = 0.1;
        while (s[i] >= '0' && s[i] <= '9') {
            result += (s[i] - '0') * frac;
            frac *= 0.1;
            has_frac_digits = 1;
            i++;
        }
    }
    /* "." alone (no digits before or after dot) is invalid */
    if (!has_int_digits && !has_frac_digits)
        return _float_lax_empty();
    /* Exponent part (e/E) */
    if (s[i] == 'e' || s[i] == 'E') {
        i++;
        int exp_neg = 0;
        if (s[i] == '-') { exp_neg = 1; i++; }
        else if (s[i] == '+') { i++; }
        int exp = 0;
        int has_exp_digits = 0;
        while (s[i] >= '0' && s[i] <= '9') {
            exp = exp * 10 + (s[i] - '0');
            has_exp_digits = 1;
            i++;
        }
        /* "1e", "1e+", "1e-" (no exponent digits) is invalid */
        if (!has_exp_digits) return _float_lax_empty();
        double multiplier = 1.0;
        for (int e = 0; e < exp; e++) multiplier *= 10.0;
        if (exp_neg) result /= multiplier;
        else result *= multiplier;
    }
    /* Must have consumed entire string */
    if (s[i] != '\0') return _float_lax_empty();

    if (negative) result = -result;
    return _float_lax_new(_d2l(result));
}

int64_t taida_float_mold_bool(int64_t v) {
    return _float_lax_new(_d2l(v ? 1.0 : 0.0));
}

int64_t taida_bool_mold_int(int64_t v) {
    int64_t lax = taida_lax_new(v != 0 ? 1 : 0, 0);
    taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
    taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
    return lax;
}

int64_t taida_bool_mold_float(int64_t v) {
    double d = _to_double(v);
    int64_t lax = taida_lax_new(d != 0.0 ? 1 : 0, 0);
    taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
    taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
    return lax;
}

int64_t taida_bool_mold_str(int64_t v) {
    const char *s = (const char *)(intptr_t)v;
    int64_t lax;
    if (!s) {
        lax = taida_lax_empty(0);
        taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
        taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
        return lax;
    }
    if (s[0] == 't' && s[1] == 'r' && s[2] == 'u' && s[3] == 'e' && s[4] == 0) {
        lax = taida_lax_new(1, 0);
        taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
        taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
        return lax;
    }
    if (s[0] == 'f' && s[1] == 'a' && s[2] == 'l' && s[3] == 's' && s[4] == 'e' && s[5] == 0) {
        lax = taida_lax_new(0, 0);
        taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
        taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
        return lax;
    }
    /* not "true" or "false" — empty Lax */
    lax = taida_lax_empty(0);
    taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
    taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
    return lax;
}

int64_t taida_bool_mold_bool(int64_t v) {
    int64_t lax = taida_lax_new(v, 0);
    taida_pack_set_tag(lax, 1, WASM_TAG_BOOL);
    taida_pack_set_tag(lax, 2, WASM_TAG_BOOL);
    return lax;
}

/* ── W-5: Float div/mod molds (returning Lax) ── */

int64_t taida_float_div_mold(int64_t a, int64_t b) {
    double da = _to_double(a), db = _to_double(b);
    if (db == 0.0) return _float_lax_new(_d2l(0.0));
    return _float_lax_new(_d2l(da / db));
}

int64_t taida_float_mod_mold(int64_t a, int64_t b) {
    double da = _to_double(a), db = _to_double(b);
    if (db == 0.0) return _float_lax_new(_d2l(0.0));
    /* fmod without libc — use repeated subtraction (good enough for wasm-min) */
    double q = da / db;
    /* truncate toward zero */
    int64_t qi = (int64_t)q;
    double result = da - (double)qi * db;
    return _float_lax_new(_d2l(result));
}

/* ── W-5: Float comparison ── */

int64_t taida_float_eq(int64_t a, int64_t b) { return _to_double(a) == _to_double(b) ? 1 : 0; }
int64_t taida_float_neq(int64_t a, int64_t b) { return _to_double(a) != _to_double(b) ? 1 : 0; }
int64_t taida_float_lt(int64_t a, int64_t b) { return _to_double(a) < _to_double(b) ? 1 : 0; }
int64_t taida_float_gt(int64_t a, int64_t b) { return _to_double(a) > _to_double(b) ? 1 : 0; }
int64_t taida_float_lte(int64_t a, int64_t b) { return _to_double(a) <= _to_double(b) ? 1 : 0; }
int64_t taida_float_gte(int64_t a, int64_t b) { return _to_double(a) >= _to_double(b) ? 1 : 0; }

/* ── W-5: String template helpers (aliases) ── */

int64_t taida_str_from_int(int64_t v) { return taida_int_to_str(v); }
int64_t taida_str_from_float(int64_t v) { return taida_float_to_str(v); }

/* ── W-5: Error helpers ── */

int64_t taida_can_throw_payload(int64_t val) {
    /* Check if val is a pack with a "type" field (looks like an error) */
    if (val < WASM_MIN_HEAP_ADDR) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    /* Simple heuristic: first field hash is type hash */
    if (p[0] >= 1 && p[0] <= 10 && p[1] == WASM_HASH_TYPE) return 1;
    return 0;
}

/* =========================================================================
 * WC-1b: String mold functions (prelude — all profiles)
 * ========================================================================= */

/// Allocate a NUL-terminated string buffer of `len` bytes (+ 1 for NUL).
/// Uses bump allocator. No hidden header needed (no RC in WASM).
int64_t taida_str_alloc(int64_t len_raw) {
    int len = (int)len_raw;
    if (len < 0) len = 0;
    char *buf = (char *)wasm_alloc((unsigned int)(len + 1));
    buf[len] = '\0';
    return (int64_t)buf;
}

/// Copy a NUL-terminated string into a newly allocated buffer.
int64_t taida_str_new_copy(int64_t src_raw) {
    const char *src = (const char *)src_raw;
    if (!src) {
        char *r = (char *)wasm_alloc(1);
        r[0] = '\0';
        return (int64_t)r;
    }
    int len = _wf_strlen(src);
    char *r = (char *)wasm_alloc((unsigned int)(len + 1));
    _wf_memcpy(r, src, len);
    r[len] = '\0';
    return (int64_t)r;
}

/// Release a string. No-op in WASM (bump allocator, no free).
void taida_str_release(int64_t s) {
    (void)s;
}

/// Upper[str]() -- convert ASCII lowercase to uppercase
int64_t taida_str_to_upper(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    char *r = (char *)wasm_alloc((unsigned int)(len + 1));
    for (int i = 0; i < len; i++) {
        r[i] = (s[i] >= 'a' && s[i] <= 'z') ? s[i] - 32 : s[i];
    }
    r[len] = '\0';
    return (int64_t)r;
}

/// Lower[str]() -- convert ASCII uppercase to lowercase
int64_t taida_str_to_lower(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    char *r = (char *)wasm_alloc((unsigned int)(len + 1));
    for (int i = 0; i < len; i++) {
        r[i] = (s[i] >= 'A' && s[i] <= 'Z') ? s[i] + 32 : s[i];
    }
    r[len] = '\0';
    return (int64_t)r;
}

/// Trim[str]() -- strip leading and trailing whitespace
int64_t taida_str_trim(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int start = 0, end = len;
    while (start < len && _wf_is_whitespace(s[start])) start++;
    while (end > start && _wf_is_whitespace(s[end - 1])) end--;
    int slen = end - start;
    char *r = (char *)wasm_alloc((unsigned int)(slen + 1));
    _wf_memcpy(r, s + start, slen);
    r[slen] = '\0';
    return (int64_t)r;
}

/// TrimStart[str]() -- strip leading whitespace
int64_t taida_str_trim_start(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int start = 0;
    while (start < len && _wf_is_whitespace(s[start])) start++;
    int slen = len - start;
    char *r = (char *)wasm_alloc((unsigned int)(slen + 1));
    _wf_memcpy(r, s + start, slen);
    r[slen] = '\0';
    return (int64_t)r;
}

/// TrimEnd[str]() -- strip trailing whitespace
int64_t taida_str_trim_end(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int end = len;
    while (end > 0 && _wf_is_whitespace(s[end - 1])) end--;
    char *r = (char *)wasm_alloc((unsigned int)(end + 1));
    _wf_memcpy(r, s, end);
    r[end] = '\0';
    return (int64_t)r;
}

/// Split[str, sep]() -- split string by separator, return list of strings.
/// If sep is empty, splits into individual characters.
int64_t taida_str_split(int64_t s_raw, int64_t sep_raw) {
    const char *s = (const char *)s_raw;
    const char *sep = (const char *)sep_raw;
    if (!s) return taida_list_new();
    int64_t list = taida_list_new();
    if (!sep || _wf_strlen(sep) == 0) {
        /* Split into individual characters */
        int len = _wf_strlen(s);
        for (int i = 0; i < len; i++) {
            char *c = (char *)wasm_alloc(2);
            c[0] = s[i];
            c[1] = '\0';
            list = taida_list_push(list, (int64_t)c);
        }
        return list;
    }
    int sep_len = _wf_strlen(sep);
    const char *p = s;
    while (1) {
        const char *found = _wf_strstr(p, sep);
        if (!found) {
            int slen = _wf_strlen(p);
            char *part = (char *)wasm_alloc((unsigned int)(slen + 1));
            _wf_memcpy(part, p, slen);
            part[slen] = '\0';
            list = taida_list_push(list, (int64_t)part);
            break;
        }
        int plen = (int)(found - p);
        char *part = (char *)wasm_alloc((unsigned int)(plen + 1));
        _wf_memcpy(part, p, plen);
        part[plen] = '\0';
        list = taida_list_push(list, (int64_t)part);
        p = found + sep_len;
    }
    return list;
}

/// Replace[str, from, to](all <= true) -- replace all occurrences
int64_t taida_str_replace(int64_t s_raw, int64_t from_raw, int64_t to_raw) {
    const char *s = (const char *)s_raw;
    const char *from = (const char *)from_raw;
    const char *to = (const char *)to_raw;
    if (!s || !from || !to) {
        if (!s) { return taida_str_alloc(0); }
        return taida_str_new_copy(s_raw);
    }
    int from_len = _wf_strlen(from);
    int to_len = _wf_strlen(to);
    if (from_len == 0) {
        return taida_str_new_copy(s_raw);
    }
    /* Count occurrences */
    int count = 0;
    const char *p = s;
    while ((p = _wf_strstr(p, from)) != (const char *)0) { count++; p += from_len; }
    int s_len = _wf_strlen(s);
    int new_len = s_len + count * (to_len - from_len);
    char *r = (char *)wasm_alloc((unsigned int)(new_len + 1));
    char *dst = r;
    p = s;
    while (1) {
        const char *found = _wf_strstr(p, from);
        if (!found) {
            int remaining = _wf_strlen(p);
            _wf_memcpy(dst, p, remaining);
            dst += remaining;
            break;
        }
        int chunk = (int)(found - p);
        _wf_memcpy(dst, p, chunk); dst += chunk;
        _wf_memcpy(dst, to, to_len); dst += to_len;
        p = found + from_len;
    }
    *dst = '\0';
    return (int64_t)r;
}

/// ReplaceFirst[str, from, to]() -- replace first occurrence only
int64_t taida_str_replace_first(int64_t s_raw, int64_t from_raw, int64_t to_raw) {
    const char *s = (const char *)s_raw;
    const char *from = (const char *)from_raw;
    const char *to = (const char *)to_raw;
    if (!s || !from || !to) {
        if (!s) { return taida_str_alloc(0); }
        return taida_str_new_copy(s_raw);
    }
    int from_len = _wf_strlen(from);
    int to_len = _wf_strlen(to);
    if (from_len == 0) {
        return taida_str_new_copy(s_raw);
    }
    const char *found = _wf_strstr(s, from);
    if (!found) {
        return taida_str_new_copy(s_raw);
    }
    int s_len = _wf_strlen(s);
    int new_len = s_len - from_len + to_len;
    char *r = (char *)wasm_alloc((unsigned int)(new_len + 1));
    int prefix = (int)(found - s);
    _wf_memcpy(r, s, prefix);
    _wf_memcpy(r + prefix, to, to_len);
    int suffix = s_len - prefix - from_len;
    _wf_memcpy(r + prefix + to_len, found + from_len, suffix);
    r[new_len] = '\0';
    return (int64_t)r;
}

/// Slice[str](start, end) -- extract substring from start to end
int64_t taida_str_slice(int64_t s_raw, int64_t start_raw, int64_t end_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int start = (int)start_raw;
    int end = (int)end_raw;
    /* Native normalizes negative end to len (e.g. end=-1 means "to end of string") */
    if (end < 0) end = len;
    if (start < 0) start = 0;
    if (end > len) end = len;
    if (start >= end) { return taida_str_alloc(0); }
    int slen = end - start;
    char *r = (char *)wasm_alloc((unsigned int)(slen + 1));
    _wf_memcpy(r, s + start, slen);
    r[slen] = '\0';
    return (int64_t)r;
}

/// CharAt[str, index]() -- extract single character at index
int64_t taida_str_char_at(int64_t s_raw, int64_t idx_raw) {
    const char *s = (const char *)s_raw;
    int idx = (int)idx_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    if (idx < 0 || idx >= len) { return taida_str_alloc(0); }
    char *r = (char *)wasm_alloc(2);
    r[0] = s[idx];
    r[1] = '\0';
    return (int64_t)r;
}

/// Repeat[str, n]() -- repeat string n times
int64_t taida_str_repeat(int64_t s_raw, int64_t n_raw) {
    const char *s = (const char *)s_raw;
    int n = (int)n_raw;
    if (!s || n <= 0) { return taida_str_alloc(0); }
    int slen = _wf_strlen(s);
    if (slen == 0) { return taida_str_alloc(0); }
    int total = slen * n;
    char *r = (char *)wasm_alloc((unsigned int)(total + 1));
    for (int i = 0; i < n; i++) {
        _wf_memcpy(r + i * slen, s, slen);
    }
    r[total] = '\0';
    return (int64_t)r;
}

/// Reverse[str]() -- reverse characters
int64_t taida_str_reverse(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    char *r = (char *)wasm_alloc((unsigned int)(len + 1));
    for (int i = 0; i < len; i++) {
        r[i] = s[len - 1 - i];
    }
    r[len] = '\0';
    return (int64_t)r;
}

/// Pad[str, target_len](padChar, padEnd) -- pad string to target length
int64_t taida_str_pad(int64_t s_raw, int64_t target_len_raw, int64_t pad_char_raw, int64_t pad_end_raw) {
    const char *s = (const char *)s_raw;
    int target_len = (int)target_len_raw;
    const char *pad_char = (const char *)pad_char_raw;
    int pad_end = (int)pad_end_raw;
    if (!s) { return taida_str_alloc(0); }
    int slen = _wf_strlen(s);
    if (slen >= target_len) {
        return taida_str_new_copy(s_raw);
    }
    int pad_len = target_len - slen;
    char pc = ' ';
    if (pad_char && _wf_strlen(pad_char) > 0) pc = pad_char[0];
    char *r = (char *)wasm_alloc((unsigned int)(target_len + 1));
    if (pad_end) {
        _wf_memcpy(r, s, slen);
        for (int i = 0; i < pad_len; i++) r[slen + i] = pc;
    } else {
        for (int i = 0; i < pad_len; i++) r[i] = pc;
        _wf_memcpy(r + pad_len, s, slen);
    }
    r[target_len] = '\0';
    return (int64_t)r;
}

/* =========================================================================
 * WC-1c: String query functions (prelude — all profiles)
 * ========================================================================= */

/// str.contains(sub) -- check if string contains substring
int64_t taida_str_contains(int64_t s_raw, int64_t sub_raw) {
    const char *s = (const char *)s_raw;
    const char *sub = (const char *)sub_raw;
    if (!s || !sub) return 0;
    return _wf_strstr(s, sub) != (const char *)0 ? 1 : 0;
}

/// str.startsWith(prefix) -- check if string starts with prefix
int64_t taida_str_starts_with(int64_t s_raw, int64_t prefix_raw) {
    const char *s = (const char *)s_raw;
    const char *prefix = (const char *)prefix_raw;
    if (!s || !prefix) return 0;
    int plen = _wf_strlen(prefix);
    return _wf_strncmp(s, prefix, plen) == 0 ? 1 : 0;
}

/// str.endsWith(suffix) -- check if string ends with suffix
int64_t taida_str_ends_with(int64_t s_raw, int64_t suffix_raw) {
    const char *s = (const char *)s_raw;
    const char *suffix = (const char *)suffix_raw;
    if (!s || !suffix) return 0;
    int slen = _wf_strlen(s);
    int suflen = _wf_strlen(suffix);
    if (suflen > slen) return 0;
    return _wf_strcmp(s + slen - suflen, suffix) == 0 ? 1 : 0;
}

/// str.indexOf(sub) -- find first index of substring, or -1 (char offset, UTF-8 aware)
int64_t taida_str_index_of(int64_t s_raw, int64_t sub_raw) {
    const char *s = (const char *)s_raw;
    const char *sub = (const char *)sub_raw;
    if (!s || !sub) return -1;
    const char *p = _wf_strstr(s, sub);
    if (!p) return -1;
    // Convert byte offset to character offset (UTF-8 aware)
    int64_t char_offset = 0;
    for (const char *c = s; c < p; ) {
        if ((*c & 0x80) == 0) c += 1;
        else if ((*c & 0xE0) == 0xC0) c += 2;
        else if ((*c & 0xF0) == 0xE0) c += 3;
        else c += 4;
        char_offset++;
    }
    return char_offset;
}

/// str.lastIndexOf(sub) -- find last index of substring, or -1 (char offset, UTF-8 aware)
int64_t taida_str_last_index_of(int64_t s_raw, int64_t sub_raw) {
    const char *s = (const char *)s_raw;
    const char *sub = (const char *)sub_raw;
    if (!s || !sub) return -1;
    int slen = _wf_strlen(s);
    int sublen = _wf_strlen(sub);
    if (sublen > slen) return -1;
    for (int i = slen - sublen; i >= 0; i--) {
        if (_wf_strncmp(s + i, sub, sublen) == 0) {
            // Convert byte offset to character offset (UTF-8 aware)
            int64_t char_offset = 0;
            for (const char *c = s; c < s + i; ) {
                if ((*c & 0x80) == 0) c += 1;
                else if ((*c & 0xE0) == 0xC0) c += 2;
                else if ((*c & 0xF0) == 0xE0) c += 3;
                else c += 4;
                char_offset++;
            }
            return char_offset;
        }
    }
    return -1;
}

/// str.get(index) -- get character at index as Lax[Str]
int64_t taida_str_get(int64_t s_raw, int64_t idx_raw) {
    const char *s = (const char *)s_raw;
    int idx = (int)idx_raw;
    if (!s) return taida_lax_empty((int64_t)"");
    int len = _wf_strlen(s);
    if (idx < 0 || idx >= len) return taida_lax_empty((int64_t)"");
    char *r = (char *)wasm_alloc(2);
    r[0] = s[idx];
    r[1] = '\0';
    return taida_lax_new((int64_t)r, (int64_t)"");
}

/// cmp_strings -- comparator for sorting string pointers
int64_t taida_cmp_strings(int64_t a_raw, int64_t b_raw) {
    const char *a = (const char *)a_raw;
    const char *b = (const char *)b_raw;
    if (!a && !b) return 0;
    if (!a) return -1;
    if (!b) return 1;
    return (int64_t)_wf_strcmp(a, b);
}

/// Slice mold -- polymorphic slice for Str, List, Bytes
int64_t taida_slice_mold(int64_t value, int64_t start_raw, int64_t end_raw) {
    return taida_str_slice(value, start_raw, end_raw);
}

/* =========================================================================
 * WC-1d: Char / Codepoint functions (prelude — all profiles)
 * ========================================================================= */

/* UTF-8 helpers for Char/Codepoint molds.
   These are small static helpers, separate from the full UTF-8 encode/decode
   module that remains in runtime_full_wasm.c. */

static int _wc_utf8_encode_scalar(uint32_t cp, unsigned char out[4], int *out_len) {
    if (cp <= 0x7F) {
        out[0] = (unsigned char)cp;
        *out_len = 1;
        return 1;
    }
    if (cp <= 0x7FF) {
        out[0] = (unsigned char)(0xC0 | (cp >> 6));
        out[1] = (unsigned char)(0x80 | (cp & 0x3F));
        *out_len = 2;
        return 1;
    }
    if (cp >= 0xD800 && cp <= 0xDFFF) return 0;
    if (cp <= 0xFFFF) {
        out[0] = (unsigned char)(0xE0 | (cp >> 12));
        out[1] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        out[2] = (unsigned char)(0x80 | (cp & 0x3F));
        *out_len = 3;
        return 1;
    }
    if (cp <= 0x10FFFF) {
        out[0] = (unsigned char)(0xF0 | (cp >> 18));
        out[1] = (unsigned char)(0x80 | ((cp >> 12) & 0x3F));
        out[2] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        out[3] = (unsigned char)(0x80 | (cp & 0x3F));
        *out_len = 4;
        return 1;
    }
    return 0;
}

static int _wc_utf8_decode_one(const unsigned char *buf, int len, int *consumed, uint32_t *out_cp) {
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
        uint32_t cp2 = ((uint32_t)(b0 & 0x0F) << 12) | ((uint32_t)(b1 & 0x3F) << 6) | (uint32_t)(b2 & 0x3F);
        if (cp2 >= 0xD800 && cp2 <= 0xDFFF) return 0;
        *consumed = 3; *out_cp = cp2;
        return 1;
    }
    if (b0 >= 0xF0 && b0 <= 0xF4) {
        if (len < 4) return 0;
        unsigned char b1 = buf[1], b2 = buf[2], b3 = buf[3];
        if ((b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 || (b3 & 0xC0) != 0x80) return 0;
        if (b0 == 0xF0 && b1 < 0x90) return 0;
        if (b0 == 0xF4 && b1 > 0x8F) return 0;
        uint32_t cp2 = ((uint32_t)(b0 & 0x07) << 18) | ((uint32_t)(b1 & 0x3F) << 12) | ((uint32_t)(b2 & 0x3F) << 6) | (uint32_t)(b3 & 0x3F);
        if (cp2 > 0x10FFFF) return 0;
        *consumed = 4; *out_cp = cp2;
        return 1;
    }
    return 0;
}

static int _wc_utf8_single_scalar(const unsigned char *buf, int len, uint32_t *cp_out) {
    int consumed = 0;
    uint32_t cp = 0;
    if (!_wc_utf8_decode_one(buf, len, &consumed, &cp)) return 0;
    if (consumed != len) return 0;
    *cp_out = cp;
    return 1;
}

/// Char[int_codepoint]() -> Lax[Str]
int64_t taida_char_mold_int(int64_t value) {
    if (value < 0 || value > 0x10FFFF) return taida_lax_empty(taida_str_alloc(0));
    if (value >= 0xD800 && value <= 0xDFFF) return taida_lax_empty(taida_str_alloc(0));
    unsigned char utf8[4];
    int out_len = 0;
    if (!_wc_utf8_encode_scalar((uint32_t)value, utf8, &out_len)) {
        return taida_lax_empty(taida_str_alloc(0));
    }
    char *out = (char *)wasm_alloc((unsigned int)(out_len + 1));
    for (int i = 0; i < out_len; i++) out[i] = (char)utf8[i];
    out[out_len] = '\0';
    return taida_lax_new((int64_t)(intptr_t)out, taida_str_alloc(0));
}

/// Char[str]() -> Lax[Str] (extract single codepoint)
int64_t taida_char_mold_str(int64_t value) {
    const char *s = (const char *)(intptr_t)value;
    if (!s) return taida_lax_empty(taida_str_alloc(0));
    int len = _wf_strlen(s);
    if (len == 0) return taida_lax_empty(taida_str_alloc(0));
    uint32_t cp = 0;
    if (!_wc_utf8_single_scalar((const unsigned char *)s, len, &cp)) {
        return taida_lax_empty(taida_str_alloc(0));
    }
    return taida_char_mold_int((int64_t)cp);
}

/// Char.toDigit() -> int (-1 for non-digit)
int64_t taida_char_to_digit(int64_t v) {
    int c = (int)v;
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'z') return c - 'a' + 10;
    if (c >= 'A' && c <= 'Z') return c - 'A' + 10;
    return -1;
}

/// Codepoint[str]() -> Lax[Int]
int64_t taida_codepoint_mold_str(int64_t value) {
    const char *s = (const char *)(intptr_t)value;
    if (!s) return taida_lax_empty(0);
    int len = _wf_strlen(s);
    if (len == 0) return taida_lax_empty(0);
    uint32_t cp = 0;
    if (!_wc_utf8_single_scalar((const unsigned char *)s, len, &cp)) {
        return taida_lax_empty(0);
    }
    return taida_lax_new((int64_t)cp, 0);
}

/// digit_to_char -- 0-9 -> '0'-'9', 10-35 -> 'a'-'z'
int64_t taida_digit_to_char(int64_t digit) {
    return (digit < 10) ? ('0' + digit) : ('a' + (digit - 10));
}

/* ── RC no-ops (wasm-min ではヒープなし) ── */

void taida_retain(int64_t val) { (void)val; }
void taida_release(int64_t val) { (void)val; }
void taida_str_retain(int64_t val) { (void)val; }

/* ── typeof: compile-time tag + runtime heuristic ── */

int64_t taida_typeof(int64_t val, int64_t tag) {
    if (val != 0 && val >= WASM_MIN_HEAP_ADDR) {
        if (_is_wasm_hashmap(val)) return (int64_t)(intptr_t)"HashMap";
        if (_is_wasm_set(val)) return (int64_t)(intptr_t)"Set";
        if (_wasm_is_result(val)) return (int64_t)(intptr_t)"Result";
        if (_wasm_is_lax(val)) return (int64_t)(intptr_t)"Lax";
        if (_looks_like_pack(val)) return (int64_t)(intptr_t)"BuchiPack";
        if (_looks_like_list(val)) return (int64_t)(intptr_t)"List";
        if (_looks_like_string(val)) return (int64_t)(intptr_t)"Str";
    }
    switch (tag) {
        case 1: return (int64_t)(intptr_t)"Float";
        case 2: return (int64_t)(intptr_t)"Bool";
        case 3: return (int64_t)(intptr_t)"Str";
        case 4: return (int64_t)(intptr_t)"BuchiPack";
        case 5: return (int64_t)(intptr_t)"List";
        case 6: return (int64_t)(intptr_t)"Closure";
        default: return (int64_t)(intptr_t)"Int";
    }
}

/* =========================================================================
 * WC-2a: Float mold functions (prelude — all profiles)
 * ========================================================================= */

/// Floor[f]() -- floor(x), returns float (bit-punned int64_t)
int64_t taida_float_floor(int64_t a) {
    double d = _to_double(a);
    double t = (double)(long long)d;
    if (t > d) t -= 1.0;
    return _d2l(t);
}

/// Ceil[f]() -- ceil(x), returns float (bit-punned int64_t)
int64_t taida_float_ceil(int64_t a) {
    double d = _to_double(a);
    double t = (double)(long long)d;
    if (t < d) t += 1.0;
    return _d2l(t);
}

/// Round[f]() -- round(x) to nearest, ties away from zero
int64_t taida_float_round(int64_t a) {
    double d = _to_double(a);
    double t;
    if (d >= 0.0) {
        t = (double)(long long)(d + 0.5);
    } else {
        t = (double)(long long)(d - 0.5);
    }
    return _d2l(t);
}

/// Abs[f]() -- absolute value of float
int64_t taida_float_abs(int64_t a) {
    double d = _to_double(a);
    return _d2l(d < 0.0 ? -d : d);
}

/// Clamp[f, lo, hi]() -- clamp float to range [lo, hi]
int64_t taida_float_clamp(int64_t a, int64_t lo, int64_t hi) {
    double da = _to_double(a);
    double dlo = _to_double(lo);
    double dhi = _to_double(hi);
    if (da < dlo) return lo;
    if (da > dhi) return hi;
    return a;
}

// --- Manual float-to-string helper for ToFixed ---

/// Write the integer part of |val| into buf, return number of chars written.
static int _wc_write_uint64(char *buf, uint64_t val) {
    if (val == 0) { buf[0] = '0'; return 1; }
    char tmp[20];
    int n = 0;
    while (val > 0) {
        tmp[n++] = '0' + (int)(val % 10);
        val /= 10;
    }
    for (int i = 0; i < n; i++) buf[i] = tmp[n - 1 - i];
    return n;
}

/// ToFixed[f, digits]() -- format float to string with N decimal places
int64_t taida_float_to_fixed(int64_t a, int64_t digits_raw) {
    double d = _to_double(a);
    int digits = (int)digits_raw;
    if (digits < 0) digits = 0;
    if (digits > 20) digits = 20;

    // Handle NaN
    if (d != d) {
        char *r = (char *)wasm_alloc(4);
        r[0] = 'N'; r[1] = 'a'; r[2] = 'N'; r[3] = '\0';
        return (int64_t)r;
    }

    int negative = 0;
    if (d < 0.0) { negative = 1; d = -d; }

    // Check infinity
    double zero_test = d * 0.0;
    if (zero_test != 0.0 || (d > 0.0 && d == d + d)) {
        if (negative) {
            char *r = (char *)wasm_alloc(5);
            r[0] = '-'; r[1] = 'i'; r[2] = 'n'; r[3] = 'f'; r[4] = '\0';
            return (int64_t)r;
        } else {
            char *r = (char *)wasm_alloc(4);
            r[0] = 'i'; r[1] = 'n'; r[2] = 'f'; r[3] = '\0';
            return (int64_t)r;
        }
    }

    // Round to `digits` decimal places
    double multiplier = 1.0;
    for (int i = 0; i < digits; i++) multiplier *= 10.0;
    double rounded = d * multiplier;
    rounded = (double)(long long)(rounded + 0.5);
    uint64_t total = (uint64_t)rounded;
    uint64_t int_part = total;
    uint64_t frac_part = 0;
    if (digits > 0) {
        uint64_t divisor = (uint64_t)multiplier;
        int_part = total / divisor;
        frac_part = total % divisor;
    }

    char buf[80];
    int pos = 0;
    if (negative) buf[pos++] = '-';
    pos += _wc_write_uint64(buf + pos, int_part);
    if (digits > 0) {
        buf[pos++] = '.';
        for (int i = digits - 1; i >= 0; i--) {
            uint64_t p = 1;
            for (int j = 0; j < i; j++) p *= 10;
            int digit = (int)((frac_part / p) % 10);
            buf[pos++] = '0' + digit;
        }
    }
    buf[pos] = '\0';

    char *r = (char *)wasm_alloc((unsigned int)(pos + 1));
    _wf_memcpy(r, buf, pos + 1);
    return (int64_t)r;
}

// ── Float state check methods ────────────────────────────

/// isNaN -- NaN != NaN
int64_t taida_float_is_nan(int64_t a) {
    double d = _to_double(a);
    return d != d ? 1 : 0;
}

/// isInfinite -- d * 0 != 0 and not NaN
int64_t taida_float_is_infinite(int64_t a) {
    double d = _to_double(a);
    if (d != d) return 0;  // NaN is not infinite
    double z = d * 0.0;
    return z != 0.0 ? 1 : 0;
}

/// isFinite -- not NaN and not infinite
int64_t taida_float_is_finite_check(int64_t a) {
    double d = _to_double(a);
    if (d != d) return 0;  // NaN
    double z = d * 0.0;
    if (z != 0.0) return 0;  // infinity
    return 1;
}

int64_t taida_float_is_positive(int64_t a) {
    double d = _to_double(a);
    return d > 0.0 ? 1 : 0;
}

int64_t taida_float_is_negative(int64_t a) {
    double d = _to_double(a);
    return d < 0.0 ? 1 : 0;
}

int64_t taida_float_is_zero(int64_t a) {
    double d = _to_double(a);
    return d == 0.0 ? 1 : 0;
}

/* =========================================================================
 * WC-2b: Int extended mold functions (prelude — all profiles)
 * ========================================================================= */

int64_t taida_int_clamp(int64_t a, int64_t lo, int64_t hi) {
    if (a < lo) return lo;
    if (a > hi) return hi;
    return a;
}

int64_t taida_int_is_positive(int64_t a) { return a > 0 ? 1 : 0; }
int64_t taida_int_is_negative(int64_t a) { return a < 0 ? 1 : 0; }
int64_t taida_int_is_zero(int64_t a) { return a == 0 ? 1 : 0; }

// ── Int mold auto / str_base ─────────────────────────────

/// digit_to_char (local) -- 0-9 -> '0'-'9', 10-35 -> 'a'-'z'
static int64_t _wc_digit_to_char(int64_t digit) {
    return (digit < 10) ? ('0' + digit) : ('a' + (digit - 10));
}

/// char_to_digit (local) -- '0'-'9' -> 0-9, 'a'-'z' -> 10-35, 'A'-'Z' -> 10-35, else -1
static int _wc_char_to_digit(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'z') return c - 'a' + 10;
    if (c >= 'A' && c <= 'Z') return c - 'A' + 10;
    return -1;
}

/// Int[v]() auto-detect: tries to distinguish int, string, other
int64_t taida_int_mold_auto(int64_t v) {
    if (v == 0) return taida_lax_new(0, 0);
    if (v < 0 || v < WASM_MIN_HEAP_ADDR) return taida_lax_new(v, 0);

    const char *s = (const char *)(intptr_t)v;
    char c = s[0];
    if (c == '-' || c == '+' || (c >= '0' && c <= '9')) {
        int neg = 0;
        int i = 0;
        if (c == '-') { neg = 1; i = 1; }
        else if (c == '+') { i = 1; }
        int64_t acc = 0;
        int found_digit = 0;
        while (s[i] >= '0' && s[i] <= '9') {
            acc = acc * 10 + (s[i] - '0');
            found_digit = 1;
            i++;
        }
        if (found_digit && s[i] == '\0') {
            return taida_lax_new(neg ? -acc : acc, 0);
        }
    }

    return taida_lax_new(v, 0);
}

/// Int[str, base]() -- parse string in given base
int64_t taida_int_mold_str_base(int64_t v, int64_t base) {
    if (base < 2 || base > 36) return taida_lax_empty(0);
    const char *s = (const char *)(intptr_t)v;
    if (!s || s[0] == '\0') return taida_lax_empty(0);
    int len = _wf_strlen(s);

    int negative = 0;
    int i = 0;
    if (s[0] == '-') {
        negative = 1;
        i = 1;
        if (len == 1) return taida_lax_empty(0);
    }

    uint64_t acc = 0;
    for (; i < len; i++) {
        int d = _wc_char_to_digit((unsigned char)s[i]);
        if (d < 0 || d >= (int)base) return taida_lax_empty(0);
        acc = acc * (uint64_t)base + (uint64_t)d;
    }

    int64_t out;
    if (negative) {
        out = -(int64_t)acc;
    } else {
        out = (int64_t)acc;
    }
    return taida_lax_new(out, 0);
}

/// ToRadix[value, base]() -- convert int to string in given base
int64_t taida_to_radix(int64_t value, int64_t base) {
    if (base < 2 || base > 36) return taida_lax_empty((int64_t)"");
    if (value == 0) {
        char *out = (char *)wasm_alloc(2);
        out[0] = '0';
        out[1] = '\0';
        return taida_lax_new((int64_t)out, (int64_t)"");
    }

    uint64_t mag = value < 0
        ? (uint64_t)(-(value + 1)) + 1
        : (uint64_t)value;
    char tmp[70];
    int pos = 0;
    while (mag > 0) {
        uint64_t rem = mag % (uint64_t)base;
        tmp[pos++] = (char)_wc_digit_to_char((int64_t)rem);
        mag /= (uint64_t)base;
    }
    if (value < 0) tmp[pos++] = '-';

    char *out = (char *)wasm_alloc((unsigned int)(pos + 1));
    for (int i = 0; i < pos; i++) {
        out[i] = tmp[pos - 1 - i];
    }
    out[pos] = '\0';
    return taida_lax_new((int64_t)out, (int64_t)"");
}

/* ── WC-3d: Public callback invoke helpers (list HOF depends on these) ── */
/* These wrap the same logic as _wasm_invoke_callback1 (static, used by Cage)
   but with public linkage so runtime_full_wasm.c can also call them. */

int64_t taida_invoke_callback1(int64_t fn_ptr, int64_t arg0) {
    if (taida_is_closure_value(fn_ptr)) {
        int64_t *closure = (int64_t *)(intptr_t)fn_ptr;
        int64_t user_arity = closure[3];
        if (user_arity == 0) {
            /* Zero-param lambda: call with env only, ignore arg0 */
            typedef int64_t (*closure_fn0_t)(int64_t);
            closure_fn0_t func = (closure_fn0_t)(intptr_t)closure[1];
            return func(closure[2]);
        }
        /* 1+ param lambda: call with env + arg0 */
        typedef int64_t (*closure_fn1_t)(int64_t, int64_t);
        closure_fn1_t func = (closure_fn1_t)(intptr_t)closure[1];
        return func(closure[2], arg0);
    }
    typedef int64_t (*fn_t)(int64_t);
    fn_t func = (fn_t)(intptr_t)fn_ptr;
    return func(arg0);
}

int64_t taida_invoke_callback2(int64_t fn_ptr, int64_t arg0, int64_t arg1) {
    if (taida_is_closure_value(fn_ptr)) {
        int64_t *closure = (int64_t *)(intptr_t)fn_ptr;
        /* Closure with 2 user args: call with env + arg0 + arg1 */
        typedef int64_t (*closure_fn2_t)(int64_t, int64_t, int64_t);
        closure_fn2_t func = (closure_fn2_t)(intptr_t)closure[1];
        return func(closure[2], arg0, arg1);
    }
    typedef int64_t (*fn_t)(int64_t, int64_t);
    fn_t func = (fn_t)(intptr_t)fn_ptr;
    return func(arg0, arg1);
}

/* ── WC-3: Hash constants for enumerate/zip (FNV-1a hashes) ── */
#define WASM_HASH_FIRST  0x89d7ed7f996f1d41ULL  /* FNV-1a("first") */
#define WASM_HASH_SECOND 0xa49985ef4cee20bdULL  /* FNV-1a("second") */
#define WASM_HASH_INDEX  0x83cf8e8f9081468bULL  /* FNV-1a("index") */
#define WASM_HASH_VALUE2 0x7ce4fd9430e80ceaULL  /* FNV-1a("value") -- suffixed to avoid conflict with WASM_HASH___VALUE */

/* ── WC-3a: List HOF functions (all profiles) ── */

int64_t taida_list_map(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t new_list = taida_list_new();
    /* map may change element type, so leave elem_tag as UNKNOWN */
    for (int64_t i = 0; i < len; i++) {
        int64_t result = taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i]);
        new_list = taida_list_push(new_list, result);
    }
    return new_list;
}

int64_t taida_list_filter(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
    }
    return new_list;
}

int64_t taida_list_fold(int64_t list_ptr, int64_t init, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t acc = init;
    for (int64_t i = 0; i < len; i++) {
        acc = taida_invoke_callback2(fn_ptr, acc, list[WASM_LIST_ELEMS + i]);
    }
    return acc;
}

int64_t taida_list_foldr(int64_t list_ptr, int64_t init, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t acc = init;
    for (int64_t i = len - 1; i >= 0; i--) {
        acc = taida_invoke_callback2(fn_ptr, acc, list[WASM_LIST_ELEMS + i]);
    }
    return acc;
}

int64_t taida_list_find(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        if (taida_invoke_callback1(fn_ptr, item)) {
            return taida_lax_new(item, 0);
        }
    }
    return taida_lax_empty(0);
}

int64_t taida_list_find_index(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) return i;
    }
    return -1;
}

int64_t taida_list_take_while(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        } else {
            break;
        }
    }
    return new_list;
}

int64_t taida_list_drop_while(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    int64_t dropping = 1;
    for (int64_t i = 0; i < len; i++) {
        if (dropping && taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) {
            continue;
        }
        dropping = 0;
        new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
    }
    return new_list;
}

int64_t taida_list_any(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) return 1;
    }
    return 0;
}

int64_t taida_list_all(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (!taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) return 0;
    }
    return 1;
}

int64_t taida_list_none(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) return 0;
    }
    return 1;
}

/* ── WC-3b: List operation functions (all profiles) ── */

int64_t taida_list_sort(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    /* Copy items into temp array (on bump allocator) */
    int64_t *items = (int64_t *)wasm_alloc((unsigned int)(len * 8));
    for (int64_t i = 0; i < len; i++) items[i] = list[WASM_LIST_ELEMS + i];
    /* Insertion sort ascending */
    for (int64_t i = 1; i < len; i++) {
        int64_t key = items[i];
        int64_t j = i - 1;
        while (j >= 0 && items[j] > key) { items[j+1] = items[j]; j--; }
        items[j+1] = key;
    }
    for (int64_t i = 0; i < len; i++) {
        new_list = taida_list_push(new_list, items[i]);
    }
    return new_list;
}

int64_t taida_list_sort_desc(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    int64_t *items = (int64_t *)wasm_alloc((unsigned int)(len * 8));
    for (int64_t i = 0; i < len; i++) items[i] = list[WASM_LIST_ELEMS + i];
    /* Insertion sort descending */
    for (int64_t i = 1; i < len; i++) {
        int64_t key = items[i];
        int64_t j = i - 1;
        while (j >= 0 && items[j] < key) { items[j+1] = items[j]; j--; }
        items[j+1] = key;
    }
    for (int64_t i = 0; i < len; i++) {
        new_list = taida_list_push(new_list, items[i]);
    }
    return new_list;
}

/* Sort by key extraction function: fn_ptr maps each element to a sort key,
   then sort ascending by key. Matches interpreter's Sort[list](by <= fn). */
int64_t taida_list_sort_by(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    if (len == 0) return new_list;
    /* Allocate parallel arrays: items and keys */
    int64_t *items = (int64_t *)wasm_alloc((unsigned int)(len * 8));
    int64_t *keys = (int64_t *)wasm_alloc((unsigned int)(len * 8));
    for (int64_t i = 0; i < len; i++) {
        items[i] = list[WASM_LIST_ELEMS + i];
        keys[i] = taida_invoke_callback1(fn_ptr, items[i]);
    }
    /* Insertion sort ascending by key */
    for (int64_t i = 1; i < len; i++) {
        int64_t kkey = keys[i];
        int64_t kitem = items[i];
        int64_t j = i - 1;
        while (j >= 0 && keys[j] > kkey) {
            keys[j+1] = keys[j];
            items[j+1] = items[j];
            j--;
        }
        keys[j+1] = kkey;
        items[j+1] = kitem;
    }
    for (int64_t i = 0; i < len; i++) {
        new_list = taida_list_push(new_list, items[i]);
    }
    return new_list;
}

int64_t taida_list_unique(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl_init = (int64_t *)(intptr_t)new_list;
    nl_init[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        /* Check if already in new_list */
        int64_t *nl = (int64_t *)(intptr_t)new_list;
        int64_t nlen = nl[1];
        int64_t found = 0;
        for (int64_t j = 0; j < nlen; j++) {
            if (nl[WASM_LIST_ELEMS + j] == item) { found = 1; break; }
        }
        if (!found) {
            new_list = taida_list_push(new_list, item);
        }
    }
    return new_list;
}

int64_t taida_list_flatten(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t new_list = taida_list_new();
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        if (_looks_like_list(item)) {
            int64_t *sub = (int64_t *)(intptr_t)item;
            int64_t slen = sub[1];
            /* Propagate inner list's elem_tag to result */
            if (i == 0) {
                int64_t *nl = (int64_t *)(intptr_t)new_list;
                nl[2] = sub[2];
            }
            for (int64_t j = 0; j < slen; j++) {
                new_list = taida_list_push(new_list, sub[WASM_LIST_ELEMS + j]);
            }
        } else {
            new_list = taida_list_push(new_list, item);
        }
    }
    return new_list;
}

int64_t taida_list_reverse(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = len - 1; i >= 0; i--) {
        new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
    }
    return new_list;
}

int64_t taida_list_join(int64_t list_ptr, int64_t sep_raw) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_str_alloc(0);
    const char *sep = (const char *)(intptr_t)sep_raw;
    if (!sep) sep = "";
    int sep_len = _wf_strlen(sep);

    /* Convert each element through polymorphic_to_string */
    const char **strs = (const char **)wasm_alloc((unsigned int)(len * sizeof(const char *)));
    int total = 0;
    for (int64_t i = 0; i < len; i++) {
        strs[i] = (const char *)(intptr_t)taida_polymorphic_to_string(list[WASM_LIST_ELEMS + i]);
        total += _wf_strlen(strs[i]);
        if (i > 0) total += sep_len;
    }

    char *r = (char *)wasm_alloc((unsigned int)(total + 1));
    char *dst = r;
    for (int64_t i = 0; i < len; i++) {
        if (i > 0 && sep_len > 0) { _wf_memcpy(dst, sep, sep_len); dst += sep_len; }
        int sl = _wf_strlen(strs[i]);
        _wf_memcpy(dst, strs[i], sl);
        dst += sl;
    }
    *dst = '\0';
    return (int64_t)r;
}

int64_t taida_list_concat(int64_t list1, int64_t list2) {
    int64_t *l1 = (int64_t *)(intptr_t)list1;
    int64_t *l2 = (int64_t *)(intptr_t)list2;
    int64_t len1 = l1[1], len2 = l2[1];
    int64_t elem_tag = l1[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = 0; i < len1; i++) {
        new_list = taida_list_push(new_list, l1[WASM_LIST_ELEMS + i]);
    }
    for (int64_t i = 0; i < len2; i++) {
        new_list = taida_list_push(new_list, l2[WASM_LIST_ELEMS + i]);
    }
    return new_list;
}

int64_t taida_list_append(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
    }
    new_list = taida_list_push(new_list, item);
    return new_list;
}

int64_t taida_list_prepend(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    new_list = taida_list_push(new_list, item);
    for (int64_t i = 0; i < len; i++) {
        new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
    }
    return new_list;
}

int64_t taida_list_take(int64_t list_ptr, int64_t n) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t take_n = n < len ? n : len;
    if (take_n < 0) take_n = 0;
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = 0; i < take_n; i++) {
        new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
    }
    return new_list;
}

int64_t taida_list_drop(int64_t list_ptr, int64_t n) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t elem_tag = list[2];
    int64_t skip = n < len ? n : len;
    if (skip < 0) skip = 0;
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = elem_tag;
    for (int64_t i = skip; i < len; i++) {
        new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
    }
    return new_list;
}

int64_t taida_list_enumerate(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t new_list = taida_list_new();
    for (int64_t i = 0; i < len; i++) {
        int64_t pair = taida_pack_new(2);
        taida_pack_set_hash(pair, 0, (int64_t)WASM_HASH_INDEX);
        taida_pack_set(pair, 0, i);
        taida_pack_set_hash(pair, 1, (int64_t)WASM_HASH_VALUE2);
        taida_pack_set(pair, 1, list[WASM_LIST_ELEMS + i]);
        new_list = taida_list_push(new_list, pair);
    }
    return new_list;
}

int64_t taida_list_zip(int64_t list1, int64_t list2) {
    int64_t *l1 = (int64_t *)(intptr_t)list1;
    int64_t *l2 = (int64_t *)(intptr_t)list2;
    int64_t len1 = l1[1], len2 = l2[1];
    int64_t min_len = len1 < len2 ? len1 : len2;
    int64_t new_list = taida_list_new();
    for (int64_t i = 0; i < min_len; i++) {
        int64_t pair = taida_pack_new(2);
        taida_pack_set_hash(pair, 0, (int64_t)WASM_HASH_FIRST);
        taida_pack_set(pair, 0, l1[WASM_LIST_ELEMS + i]);
        taida_pack_set_hash(pair, 1, (int64_t)WASM_HASH_SECOND);
        taida_pack_set(pair, 1, l2[WASM_LIST_ELEMS + i]);
        new_list = taida_list_push(new_list, pair);
    }
    return new_list;
}

int64_t taida_list_to_display_string(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) {
        char *result = (char *)wasm_alloc(4);
        _wf_memcpy(result, "@[]", 4);
        return (int64_t)result;
    }
    /* Build "@[elem, elem, ...]" */
    const char **strs = (const char **)wasm_alloc((unsigned int)(len * sizeof(const char *)));
    int total = 3; /* "@[" + "]" */
    for (int64_t i = 0; i < len; i++) {
        strs[i] = (const char *)(intptr_t)taida_polymorphic_to_string(list[WASM_LIST_ELEMS + i]);
        total += _wf_strlen(strs[i]);
        if (i > 0) total += 2; /* ", " */
    }
    char *r = (char *)wasm_alloc((unsigned int)(total + 1));
    r[0] = '@'; r[1] = '[';
    char *dst = r + 2;
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) { dst[0] = ','; dst[1] = ' '; dst += 2; }
        int sl = _wf_strlen(strs[i]);
        _wf_memcpy(dst, strs[i], sl);
        dst += sl;
    }
    *dst++ = ']';
    *dst = '\0';
    return (int64_t)r;
}

/* ── WC-3c: List query functions (all profiles) ── */

int64_t taida_list_first(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_lax_empty(0);
    return taida_lax_new(list[WASM_LIST_ELEMS], 0);
}

int64_t taida_list_last(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_lax_empty(0);
    return taida_lax_new(list[WASM_LIST_ELEMS + len - 1], 0);
}

int64_t taida_list_min(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_lax_empty(0);
    int64_t min_val = list[WASM_LIST_ELEMS];
    for (int64_t i = 1; i < len; i++) {
        if (list[WASM_LIST_ELEMS + i] < min_val) min_val = list[WASM_LIST_ELEMS + i];
    }
    return taida_lax_new(min_val, 0);
}

int64_t taida_list_max(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_lax_empty(0);
    int64_t max_val = list[WASM_LIST_ELEMS];
    for (int64_t i = 1; i < len; i++) {
        if (list[WASM_LIST_ELEMS + i] > max_val) max_val = list[WASM_LIST_ELEMS + i];
    }
    return taida_lax_new(max_val, 0);
}

int64_t taida_list_sum(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t sum = 0;
    for (int64_t i = 0; i < len; i++) {
        sum += list[WASM_LIST_ELEMS + i];
    }
    return sum;
}

int64_t taida_list_contains(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (list[WASM_LIST_ELEMS + i] == item) return 1;
    }
    return 0;
}

int64_t taida_list_index_of(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (list[WASM_LIST_ELEMS + i] == item) return i;
    }
    return -1;
}

int64_t taida_list_last_index_of(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = len - 1; i >= 0; i--) {
        if (list[WASM_LIST_ELEMS + i] == item) return i;
    }
    return -1;
}

int64_t taida_list_count(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t count = 0;
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) count++;
    }
    return count;
}

/* ── List elem retain/release (no-ops in WASM) ── */
void taida_list_elem_retain(int64_t list) { (void)list; }
void taida_list_elem_release(int64_t list) { (void)list; }

/* ══════════════════════════════════════════════════════════════════════════
   WC-4: JSON runtime (moved from runtime_full_wasm.c)
   ══════════════════════════════════════════════════════════════════════════ */

/* ── Helper: manual strtol (base 10) ── */
static int64_t _wc_strtol(const char *s, const char **end) {
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

/* ── Helper: manual strtod ── */
static double _wc_strtod(const char *s, const char **end) {
    if (!s) { if (end) *end = s; return 0.0; }
    const char *p = s;
    double result = 0.0;
    int neg = 0;
    if (*p == '-') { neg = 1; p++; }
    else if (*p == '+') { p++; }
    if (*p < '0' || *p > '9') {
        if (*p != '.') { if (end) *end = s; return 0.0; }
    }
    while (*p >= '0' && *p <= '9') {
        result = result * 10.0 + (*p - '0');
        p++;
    }
    if (*p == '.') {
        p++;
        double frac = 0.1;
        while (*p >= '0' && *p <= '9') {
            result += (*p - '0') * frac;
            frac *= 0.1;
            p++;
        }
    }
    if (*p == 'e' || *p == 'E') {
        p++;
        int exp_neg = 0;
        if (*p == '-') { exp_neg = 1; p++; }
        else if (*p == '+') { p++; }
        int exp = 0;
        while (*p >= '0' && *p <= '9') {
            exp = exp * 10 + (*p - '0');
            p++;
        }
        double factor = 1.0;
        for (int i = 0; i < exp; i++) factor *= 10.0;
        if (exp_neg) result /= factor;
        else result *= factor;
    }
    if (end) *end = p;
    return neg ? -result : result;
}

/* ── Helper: int64_t to string ── */
static char *_wc_i64_to_str(int64_t val) {
    char tmp[24];
    int len = 0;
    int neg = 0;
    uint64_t uval;
    if (val < 0) { neg = 1; uval = (uint64_t)(-(val + 1)) + 1; }
    else { uval = (uint64_t)val; }
    if (uval == 0) { tmp[len++] = '0'; }
    else {
        while (uval > 0) { tmp[len++] = '0' + (int)(uval % 10); uval /= 10; }
    }
    int total = neg + len;
    char *buf = (char *)wasm_alloc((unsigned int)(total + 1));
    int pos = 0;
    if (neg) buf[pos++] = '-';
    for (int i = len - 1; i >= 0; i--) buf[pos++] = tmp[i];
    buf[pos] = '\0';
    return buf;
}

/* ── Helper: double to string ── */
static char *_wc_double_to_str(double val) {
    if (val != val) {
        char *r = (char *)wasm_alloc(4); r[0]='N'; r[1]='a'; r[2]='N'; r[3]='\0'; return r;
    }
    if (val > 1e18 || val < -1e18) {
        int neg = val < 0;
        if (neg) val = -val;
        int exp = 0;
        double v = val;
        while (v >= 10.0) { v /= 10.0; exp++; }
        char *buf = (char *)wasm_alloc(32);
        int pos = 0;
        if (neg) buf[pos++] = '-';
        int d = (int)v;
        buf[pos++] = '0' + d;
        double frac = v - d;
        if (frac > 0.0001) {
            buf[pos++] = '.';
            for (int i = 0; i < 5 && frac > 0.00001; i++) {
                frac *= 10.0;
                int fd = (int)frac;
                buf[pos++] = '0' + fd;
                frac -= fd;
            }
            while (pos > 2 && buf[pos - 1] == '0') pos--;
            if (buf[pos - 1] == '.') pos--;
        }
        buf[pos++] = 'e';
        buf[pos++] = '+';
        if (exp >= 100) { buf[pos++] = '0' + exp / 100; exp %= 100; }
        buf[pos++] = '0' + exp / 10;
        buf[pos++] = '0' + exp % 10;
        buf[pos] = '\0';
        return buf;
    }
    int neg = 0;
    if (val < 0) { neg = 1; val = -val; }
    int64_t int_part = (int64_t)val;
    double frac_part = val - (double)int_part;
    if (frac_part < 0.0000001 && frac_part > -0.0000001) {
        char *istr = _wc_i64_to_str(neg ? -int_part : int_part);
        return istr;
    }
    char *istr = _wc_i64_to_str(int_part);
    int ilen = _wf_strlen(istr);
    char *buf = (char *)wasm_alloc((unsigned int)(ilen + 18));
    int pos = 0;
    if (neg) buf[pos++] = '-';
    for (int i = 0; i < ilen; i++) buf[pos++] = istr[i];
    buf[pos++] = '.';
    for (int i = 0; i < 10; i++) {
        frac_part *= 10.0;
        int d = (int)frac_part;
        if (d > 9) d = 9;
        buf[pos++] = '0' + d;
        frac_part -= d;
        if (frac_part < 0.00000001) break;
    }
    while (pos > 0 && buf[pos - 1] == '0') pos--;
    if (pos > 0 && buf[pos - 1] == '.') pos--;
    buf[pos] = '\0';
    return buf;
}

/* ── Helper: FNV-1a hash (matches Rust side) ── */
static uint64_t _wc_fnv1a(const char *s, int len) {
    uint64_t hash = 0xcbf29ce484222325ULL;
    for (int i = 0; i < len; i++) {
        hash ^= (unsigned char)s[i];
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

/* ── Type detection helpers for JSON serializer ── */

static int _wc_looks_like_list(int64_t ptr) {
    if (ptr == 0) return 0;
    if (ptr < 0 || ptr > 0xFFFFFFFF) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    int64_t cap = data[0];
    int64_t len = data[1];
    if (cap >= 8 && cap <= 65536 && len >= 0 && len <= cap) return 1;
    return 0;
}

static int _wc_looks_like_string(int64_t val) {
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

static int _wc_is_hashmap(int64_t ptr) {
    if (ptr == 0 || ptr < 0 || ptr > 0xFFFFFFFF) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WASM_HM_MARKER_VAL;
}

static int _wc_is_set(int64_t ptr) {
    if (ptr == 0 || ptr < 0 || ptr > 0xFFFFFFFF) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WASM_SET_MARKER_VAL;
}

static int _wc_is_valid_ptr(int64_t val, unsigned int min_bytes) {
    if (val <= 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr + min_bytes > mem_size) return 0;
    return 1;
}

static int _wc_is_lax(int64_t val) {
    if (!_wc_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] == 4 && p[1] == WASM_HASH_HAS_VALUE) return 1;
    return 0;
}

static int _wc_is_result(int64_t val) {
    if (!_wc_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] == 4 && p[1] == WASM_HASH___VALUE) {
        int64_t hash2 = p[1 + 2 * 3]; /* field 2 hash */
        if (hash2 == WASM_HASH_THROW) return 1;
    }
    return 0;
}

/* ── Public field lookup wrappers (WC-4: needed by JSON serializer) ── */
int64_t taida_lookup_field_name(int64_t hash) {
    const char *name = _wasm_lookup_field_name(hash);
    return (int64_t)(intptr_t)name;
}

int64_t taida_lookup_field_type(int64_t hash, int64_t name_ptr) {
    (void)name_ptr;
    return _wasm_lookup_field_type(hash);
}

/* ══════════════════════════════════════════════════════════════════════════
   WC-4: JSON value types and parser
   ══════════════════════════════════════════════════════════════════════════ */

enum {
    WC_JSON_NULL = 0,
    WC_JSON_INT,
    WC_JSON_FLOAT,
    WC_JSON_STRING,
    WC_JSON_BOOL,
    WC_JSON_ARRAY,
    WC_JSON_OBJECT
};

typedef struct wc_json_array wc_json_array;
typedef struct wc_json_obj wc_json_obj;
typedef struct wc_json_obj_entry wc_json_obj_entry;

typedef struct {
    int type;
    int64_t int_val;
    double float_val;
    char *str_val;
    wc_json_array *arr;
    wc_json_obj *obj;
} wc_json_val;

struct wc_json_array {
    wc_json_val *items;
    int count;
    int cap;
};

struct wc_json_obj_entry {
    char *key;
    wc_json_val value;
};

struct wc_json_obj {
    wc_json_obj_entry *entries;
    int count;
    int cap;
};

/* Forward declarations */
static wc_json_val _wc_json_parse_value(const char **p);
static void _wc_json_skip_ws(const char **p);
static int64_t _wc_json_apply_schema(wc_json_val *jval, const char **desc);

static void _wc_json_skip_ws(const char **p) {
    while (**p == ' ' || **p == '\t' || **p == '\n' || **p == '\r') (*p)++;
}

static char *_wc_json_parse_string_raw(const char **p) {
    if (**p != '"') return (char *)0;
    (*p)++;
    const char *scan = *p;
    int len = 0;
    while (*scan && *scan != '"') {
        if (*scan == '\\') { scan++; if (*scan) scan++; }
        else scan++;
        len++;
    }
    char *buf = (char *)wasm_alloc((unsigned int)(len + 1));
    int out = 0;
    while (**p && **p != '"') {
        if (**p == '\\') {
            (*p)++;
            switch (**p) {
                case '"': buf[out++] = '"'; break;
                case '\\': buf[out++] = '\\'; break;
                case '/': buf[out++] = '/'; break;
                case 'n': buf[out++] = '\n'; break;
                case 't': buf[out++] = '\t'; break;
                case 'r': buf[out++] = '\r'; break;
                case 'b': buf[out++] = '\b'; break;
                case 'f': buf[out++] = '\f'; break;
                // TODO(WCR-3): \uXXXX Unicode escape not implemented.
                // Currently outputs raw character after 'u'. Should decode 4-hex-digit
                // codepoint to UTF-8. Surrogate pairs (\uD800-\uDFFF) also needed.
                default: buf[out++] = **p; break;
            }
            (*p)++;
        } else {
            buf[out++] = **p;
            (*p)++;
        }
    }
    buf[out] = '\0';
    if (**p == '"') (*p)++;
    return buf;
}

static wc_json_val _wc_json_parse_string(const char **p) {
    wc_json_val v;
    v.type = WC_JSON_STRING;
    v.str_val = _wc_json_parse_string_raw(p);
    v.arr = (wc_json_array *)0;
    v.obj = (wc_json_obj *)0;
    v.int_val = 0;
    v.float_val = 0.0;
    return v;
}

static wc_json_val _wc_json_parse_number(const char **p) {
    wc_json_val v;
    v.str_val = (char *)0; v.arr = (wc_json_array *)0; v.obj = (wc_json_obj *)0;
    const char *end;
    double d = _wc_strtod(*p, &end);
    int is_int = 1;
    const char *scan = *p;
    if (*scan == '-') scan++;
    while (scan < end) {
        if (*scan == '.' || *scan == 'e' || *scan == 'E') { is_int = 0; break; }
        scan++;
    }
    *p = end;
    if (is_int && d >= -9007199254740992.0 && d <= 9007199254740992.0) {
        v.type = WC_JSON_INT;
        v.int_val = (int64_t)d;
        v.float_val = d;
    } else {
        v.type = WC_JSON_FLOAT;
        v.float_val = d;
        v.int_val = (int64_t)d;
    }
    return v;
}

static wc_json_val _wc_json_parse_array(const char **p) {
    wc_json_val v;
    v.type = WC_JSON_ARRAY;
    v.str_val = (char *)0; v.obj = (wc_json_obj *)0;
    v.int_val = 0; v.float_val = 0.0;
    v.arr = (wc_json_array *)wasm_alloc(sizeof(wc_json_array));
    v.arr->count = 0;
    v.arr->cap = 4;
    v.arr->items = (wc_json_val *)wasm_alloc((unsigned int)(4 * sizeof(wc_json_val)));
    (*p)++;
    _wc_json_skip_ws(p);
    if (**p == ']') { (*p)++; return v; }
    while (**p) {
        wc_json_val item = _wc_json_parse_value(p);
        if (v.arr->count >= v.arr->cap) {
            int new_cap = v.arr->cap * 2;
            wc_json_val *new_items = (wc_json_val *)wasm_alloc(
                (unsigned int)(new_cap * sizeof(wc_json_val)));
            for (int i = 0; i < v.arr->count; i++) new_items[i] = v.arr->items[i];
            v.arr->items = new_items;
            v.arr->cap = new_cap;
        }
        v.arr->items[v.arr->count++] = item;
        _wc_json_skip_ws(p);
        if (**p == ',') { (*p)++; _wc_json_skip_ws(p); }
        else break;
    }
    if (**p == ']') (*p)++;
    return v;
}

static wc_json_val _wc_json_parse_object(const char **p) {
    wc_json_val v;
    v.type = WC_JSON_OBJECT;
    v.str_val = (char *)0; v.arr = (wc_json_array *)0;
    v.int_val = 0; v.float_val = 0.0;
    v.obj = (wc_json_obj *)wasm_alloc(sizeof(wc_json_obj));
    v.obj->count = 0;
    v.obj->cap = 8;
    v.obj->entries = (wc_json_obj_entry *)wasm_alloc(
        (unsigned int)(8 * sizeof(wc_json_obj_entry)));
    (*p)++;
    _wc_json_skip_ws(p);
    if (**p == '}') { (*p)++; return v; }
    while (**p) {
        _wc_json_skip_ws(p);
        char *key = _wc_json_parse_string_raw(p);
        _wc_json_skip_ws(p);
        if (**p == ':') (*p)++;
        _wc_json_skip_ws(p);
        wc_json_val val = _wc_json_parse_value(p);
        if (v.obj->count >= v.obj->cap) {
            int new_cap = v.obj->cap * 2;
            wc_json_obj_entry *new_entries = (wc_json_obj_entry *)wasm_alloc(
                (unsigned int)(new_cap * sizeof(wc_json_obj_entry)));
            for (int i = 0; i < v.obj->count; i++) new_entries[i] = v.obj->entries[i];
            v.obj->entries = new_entries;
            v.obj->cap = new_cap;
        }
        v.obj->entries[v.obj->count].key = key;
        v.obj->entries[v.obj->count].value = val;
        v.obj->count++;
        _wc_json_skip_ws(p);
        if (**p == ',') { (*p)++; _wc_json_skip_ws(p); }
        else break;
    }
    if (**p == '}') (*p)++;
    return v;
}

static wc_json_val _wc_json_parse_value(const char **p) {
    _wc_json_skip_ws(p);
    wc_json_val v;
    v.str_val = (char *)0; v.arr = (wc_json_array *)0; v.obj = (wc_json_obj *)0;
    v.int_val = 0; v.float_val = 0.0;
    if (**p == '"') return _wc_json_parse_string(p);
    if (**p == '{') return _wc_json_parse_object(p);
    if (**p == '[') return _wc_json_parse_array(p);
    if (**p == 't' && _wf_strncmp(*p, "true", 4) == 0) {
        *p += 4; v.type = WC_JSON_BOOL; v.int_val = 1; return v;
    }
    if (**p == 'f' && _wf_strncmp(*p, "false", 5) == 0) {
        *p += 5; v.type = WC_JSON_BOOL; v.int_val = 0; return v;
    }
    if (**p == 'n' && _wf_strncmp(*p, "null", 4) == 0) {
        *p += 4; v.type = WC_JSON_NULL; v.int_val = 0; return v;
    }
    if (**p == '-' || (**p >= '0' && **p <= '9')) return _wc_json_parse_number(p);
    v.type = WC_JSON_NULL; v.int_val = 0;
    return v;
}

/* ── JSON object field lookup ── */
static wc_json_val *_wc_json_obj_get(wc_json_obj *obj, const char *key) {
    if (!obj) return (wc_json_val *)0;
    for (int i = 0; i < obj->count; i++) {
        if (_wf_strcmp(obj->entries[i].key, key) == 0) {
            return &obj->entries[i].value;
        }
    }
    return (wc_json_val *)0;
}

/* ── Schema helpers ── */
static int _wc_schema_find_closing_brace(const char *desc) {
    int depth = 1;
    int i = 0;
    while (desc[i] && depth > 0) {
        if (desc[i] == '{') depth++;
        if (desc[i] == '}') depth--;
        if (depth > 0) i++;
    }
    return i;
}

static int64_t _wc_json_default_value_for_desc(const char *desc) {
    if (!desc || !*desc) return 0;
    switch (desc[0]) {
        case 'i': return 0;
        case 'f': return _d2l(0.0);
        case 's': {
            char *empty = (char *)wasm_alloc(1);
            empty[0] = '\0';
            return (int64_t)(intptr_t)empty;
        }
        case 'b': return 0;
        case 'T': {
            wc_json_val null_val;
            null_val.type = WC_JSON_NULL;
            null_val.str_val = (char *)0; null_val.arr = (wc_json_array *)0;
            null_val.obj = (wc_json_obj *)0;
            null_val.int_val = 0; null_val.float_val = 0.0;
            return _wc_json_apply_schema(&null_val, &desc);
        }
        case 'L': {
            return taida_list_new();
        }
        default: return 0;
    }
}

/* ── Convert JSON value to typed value ── */
static int64_t _wc_json_to_int(wc_json_val *jv) {
    if (!jv) return 0;
    switch (jv->type) {
        case WC_JSON_INT: return jv->int_val;
        case WC_JSON_FLOAT: return (int64_t)jv->float_val;
        case WC_JSON_BOOL: return jv->int_val;
        case WC_JSON_STRING: {
            if (!jv->str_val) return 0;
            const char *end;
            int64_t r = _wc_strtol(jv->str_val, &end);
            if (*end != '\0') return 0;
            return r;
        }
        default: return 0;
    }
}

static int64_t _wc_json_to_float(wc_json_val *jv) {
    if (!jv) return _d2l(0.0);
    switch (jv->type) {
        case WC_JSON_FLOAT: return _d2l(jv->float_val);
        case WC_JSON_INT: return _d2l((double)jv->int_val);
        case WC_JSON_BOOL: return _d2l(jv->int_val ? 1.0 : 0.0);
        case WC_JSON_STRING: {
            if (!jv->str_val) return _d2l(0.0);
            const char *end;
            double r = _wc_strtod(jv->str_val, &end);
            if (*end != '\0') return _d2l(0.0);
            return _d2l(r);
        }
        default: return _d2l(0.0);
    }
}

static int64_t _wc_json_to_str(wc_json_val *jv) {
    if (!jv) return taida_str_alloc(0);
    switch (jv->type) {
        case WC_JSON_STRING: {
            if (!jv->str_val) return taida_str_alloc(0);
            return taida_str_new_copy((int64_t)(intptr_t)jv->str_val);
        }
        case WC_JSON_INT: {
            char *s = _wc_i64_to_str(jv->int_val);
            return (int64_t)(intptr_t)s;
        }
        case WC_JSON_FLOAT: {
            char *s = _wc_double_to_str(jv->float_val);
            return (int64_t)(intptr_t)s;
        }
        case WC_JSON_BOOL: {
            const char *src = jv->int_val ? "true" : "false";
            return taida_str_new_copy((int64_t)(intptr_t)src);
        }
        case WC_JSON_NULL:
            return taida_str_alloc(0);
        default:
            return taida_str_alloc(0);
    }
}

static int64_t _wc_json_to_bool(wc_json_val *jv) {
    if (!jv) return 0;
    switch (jv->type) {
        case WC_JSON_BOOL: return jv->int_val;
        case WC_JSON_INT: return jv->int_val != 0 ? 1 : 0;
        case WC_JSON_FLOAT: return jv->float_val != 0.0 ? 1 : 0;
        case WC_JSON_STRING: return (jv->str_val && jv->str_val[0]) ? 1 : 0;
        case WC_JSON_NULL: return 0;
        default: return 0;
    }
}

/* ── Apply schema descriptor to JSON value ── */
static int64_t _wc_json_apply_schema(wc_json_val *jval, const char **desc) {
    if (!desc || !*desc || !**desc) return 0;
    const char *d = *desc;

    switch (d[0]) {
        case 'i': {
            *desc = d + 1;
            if (!jval || jval->type == WC_JSON_NULL) return 0;
            return _wc_json_to_int(jval);
        }
        case 'f': {
            *desc = d + 1;
            if (!jval || jval->type == WC_JSON_NULL) return _d2l(0.0);
            return _wc_json_to_float(jval);
        }
        case 's': {
            *desc = d + 1;
            if (!jval || jval->type == WC_JSON_NULL) {
                return taida_str_alloc(0);
            }
            return _wc_json_to_str(jval);
        }
        case 'b': {
            *desc = d + 1;
            if (!jval || jval->type == WC_JSON_NULL) return 0;
            return _wc_json_to_bool(jval);
        }
        case 'T': {
            if (d[1] != '{') { *desc = d + 1; return 0; }
            d += 2;
            char type_name[256];
            int tn_len = 0;
            while (*d && *d != '|' && tn_len < 255) { type_name[tn_len++] = *d; d++; }
            type_name[tn_len] = '\0';
            if (*d == '|') d++;

            int field_count = 0;
            {
                const char *scan = d;
                if (*scan && *scan != '}') field_count = 1;
                int depth = 0;
                while (*scan && !(*scan == '}' && depth == 0)) {
                    if (*scan == '{') depth++;
                    if (*scan == '}') depth--;
                    if (*scan == ',' && depth == 0) field_count++;
                    scan++;
                }
            }

            int64_t pack = taida_pack_new(field_count + 1);

            int idx = 0;
            while (*d && *d != '}') {
                char fname[256];
                int fn_len = 0;
                while (*d && *d != ':' && *d != '}' && fn_len < 255) { fname[fn_len++] = *d; d++; }
                fname[fn_len] = '\0';
                if (*d == ':') d++;

                uint64_t hash = _wc_fnv1a(fname, fn_len);
                taida_pack_set_hash(pack, idx, (int64_t)hash);

                wc_json_val *field_jval = (wc_json_val *)0;
                if (jval && jval->type == WC_JSON_OBJECT) {
                    field_jval = _wc_json_obj_get(jval->obj, fname);
                }

                int64_t field_val = _wc_json_apply_schema(field_jval, &d);
                taida_pack_set(pack, idx, field_val);
                idx++;

                if (*d == ',') d++;
            }
            if (*d == '}') d++;

            uint64_t type_hash = _wc_fnv1a("__type", 6);
            taida_pack_set_hash(pack, idx, (int64_t)type_hash);
            char *type_str = (char *)wasm_alloc((unsigned int)(tn_len + 1));
            _wf_memcpy(type_str, type_name, tn_len + 1);
            taida_pack_set(pack, idx, (int64_t)(intptr_t)type_str);

            *desc = d;
            return pack;
        }
        case 'L': {
            if (d[1] != '{') { *desc = d + 1; return taida_list_new(); }
            d += 2;
            int inner_len = _wc_schema_find_closing_brace(d);
            char *inner_desc = (char *)wasm_alloc((unsigned int)(inner_len + 1));
            _wf_memcpy(inner_desc, d, inner_len);
            inner_desc[inner_len] = '\0';

            int64_t list = taida_list_new();

            if (jval && jval->type == WC_JSON_ARRAY && jval->arr) {
                for (int i = 0; i < jval->arr->count; i++) {
                    const char *elem_desc = inner_desc;
                    int64_t elem = _wc_json_apply_schema(&jval->arr->items[i], &elem_desc);
                    list = taida_list_push(list, elem);
                }
            }

            d += inner_len;
            if (*d == '}') d++;
            *desc = d;
            return list;
        }
        default: {
            *desc = d + 1;
            return 0;
        }
    }
}

/* ══════════════════════════════════════════════════════════════════════════
   WC-4: Public JSON API functions
   ══════════════════════════════════════════════════════════════════════════ */

/* JSON[raw, Schema]() -> Lax[T] */
int64_t taida_json_schema_cast(int64_t raw_ptr, int64_t schema_ptr) {
    const char *raw = (const char *)(intptr_t)raw_ptr;
    const char *schema = (const char *)(intptr_t)schema_ptr;

    if (!raw || !schema) {
        int64_t def = _wc_json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    const char *p = raw;
    _wc_json_skip_ws(&p);
    if (!*p) {
        int64_t def = _wc_json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    const char *before_parse = p;
    wc_json_val jval = _wc_json_parse_value(&p);

    if (p == before_parse) {
        int64_t def = _wc_json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    _wc_json_skip_ws(&p);
    if (*p != '\0') {
        int64_t def = _wc_json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    const char *desc = schema;
    int64_t result = _wc_json_apply_schema(&jval, &desc);
    int64_t def = _wc_json_default_value_for_desc(schema);
    return taida_lax_new(result, def);
}

/* taida_json_parse: copy raw JSON string */
int64_t taida_json_parse(int64_t str_ptr) {
    const char *src = (const char *)(intptr_t)str_ptr;
    if (!src) src = "{}";
    int len = _wf_strlen(src);
    char *buf = (char *)wasm_alloc((unsigned int)(len + 1));
    _wf_memcpy(buf, src, len + 1);
    return (int64_t)(intptr_t)buf;
}

/* taida_json_empty: return "{}" */
int64_t taida_json_empty(void) {
    char *buf = (char *)wasm_alloc(3);
    buf[0] = '{'; buf[1] = '}'; buf[2] = '\0';
    return (int64_t)(intptr_t)buf;
}

/* taida_json_from_int: serialize int as JSON string */
int64_t taida_json_from_int(int64_t value) {
    char *s = _wc_i64_to_str(value);
    return (int64_t)(intptr_t)s;
}

/* taida_json_from_str: wrap string in quotes */
int64_t taida_json_from_str(int64_t str_ptr) {
    const char *src = (const char *)(intptr_t)str_ptr;
    if (!src) src = "";
    int src_len = _wf_strlen(src);
    int new_len = src_len + 2;
    char *buf = (char *)wasm_alloc((unsigned int)(new_len + 1));
    buf[0] = '"';
    _wf_memcpy(buf + 1, src, src_len);
    buf[new_len - 1] = '"';
    buf[new_len] = '\0';
    return (int64_t)(intptr_t)buf;
}

/* taida_json_unmold: copy JSON string */
int64_t taida_json_unmold(int64_t json_ptr) {
    const char *src = (const char *)(intptr_t)json_ptr;
    if (!src) return taida_str_alloc(0);
    int len = _wf_strlen(src);
    char *buf = (char *)wasm_alloc((unsigned int)(len + 1));
    _wf_memcpy(buf, src, len + 1);
    return (int64_t)(intptr_t)buf;
}

/* taida_json_stringify: same as unmold */
int64_t taida_json_stringify(int64_t json_ptr) {
    return taida_json_unmold(json_ptr);
}

/* taida_json_to_str: same as unmold */
int64_t taida_json_to_str(int64_t json_ptr) {
    return taida_json_unmold(json_ptr);
}

/* taida_json_to_int: parse JSON as integer */
int64_t taida_json_to_int(int64_t json_ptr) {
    const char *data = (const char *)(intptr_t)json_ptr;
    if (!data) return 0;
    const char *end;
    return _wc_strtol(data, &end);
}

/* taida_json_size: length of JSON string */
int64_t taida_json_size(int64_t json_ptr) {
    const char *data = (const char *)(intptr_t)json_ptr;
    if (!data) return 0;
    return (int64_t)_wf_strlen(data);
}

/* taida_json_has: check if key substring exists */
int64_t taida_json_has(int64_t json_ptr, int64_t key_ptr) {
    const char *json_data = (const char *)(intptr_t)json_ptr;
    const char *key_data = (const char *)(intptr_t)key_ptr;
    if (!json_data || !key_data) return 0;
    return _wf_strstr(json_data, key_data) ? 1 : 0;
}

/* taida_debug_json: print JSON debug info to stdout */
int64_t taida_debug_json(int64_t json_ptr) {
    const char *data = (const char *)(intptr_t)json_ptr;
    const char *prefix = "JSON(";
    const char *suffix = ")\n";
    const char *body = data ? data : "null";
    int plen = _wf_strlen(prefix);
    int blen = _wf_strlen(body);
    int slen = _wf_strlen(suffix);
    int total = plen + blen + slen;
    char *buf = (char *)wasm_alloc((unsigned int)(total + 1));
    _wf_memcpy(buf, prefix, plen);
    _wf_memcpy(buf + plen, body, blen);
    _wf_memcpy(buf + plen + blen, suffix, slen);
    buf[total] = '\0';
    extern int fd_write(int fd, const void *iovs, int iovs_len, int *nwritten)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")));
    struct { const char *buf; int len; } iov = { buf, total };
    int nwritten;
    fd_write(1, &iov, 1, &nwritten);
    return 0;
}

/* taida_debug_list: print list debug info to stdout */
int64_t taida_debug_list(int64_t list_ptr) {
    int64_t str = taida_list_to_display_string(list_ptr);
    const char *s = (const char *)(intptr_t)str;
    int len = _wf_strlen(s);
    extern int fd_write(int fd, const void *iovs, int iovs_len, int *nwritten)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")));
    char *buf = (char *)wasm_alloc((unsigned int)(len + 1));
    _wf_memcpy(buf, s, len);
    buf[len] = '\n';
    struct { const char *buf; int len; } iov = { buf, len + 1 };
    int nwritten;
    fd_write(1, &iov, 1, &nwritten);
    return 0;
}

/* ══════════════════════════════════════════════════════════════════════════
   WC-4: jsonEncode / jsonPretty (type-detection based serializer)
   ══════════════════════════════════════════════════════════════════════════ */

typedef struct {
    char *buf;
    int len;
    int cap;
} _wc_json_buf;

static void _wc_jb_init(_wc_json_buf *jb) {
    jb->cap = 256;
    jb->buf = (char *)wasm_alloc(jb->cap);
    jb->len = 0;
    if (jb->buf) jb->buf[0] = '\0';
}

static void _wc_jb_ensure(_wc_json_buf *jb, int needed) {
    if (jb->len + needed + 1 > jb->cap) {
        int new_cap = jb->cap;
        while (jb->len + needed + 1 > new_cap) new_cap *= 2;
        char *new_buf = (char *)wasm_alloc((unsigned int)new_cap);
        if (!new_buf) return;
        for (int i = 0; i < jb->len; i++) new_buf[i] = jb->buf[i];
        new_buf[jb->len] = '\0';
        jb->buf = new_buf;
        jb->cap = new_cap;
    }
}

static void _wc_jb_append(_wc_json_buf *jb, const char *s) {
    int slen = _wf_strlen(s);
    _wc_jb_ensure(jb, slen);
    for (int i = 0; i < slen; i++) jb->buf[jb->len + i] = s[i];
    jb->len += slen;
    jb->buf[jb->len] = '\0';
}

static void _wc_jb_append_char(_wc_json_buf *jb, char c) {
    _wc_jb_ensure(jb, 1);
    jb->buf[jb->len] = c;
    jb->len++;
    jb->buf[jb->len] = '\0';
}

static void _wc_jb_append_escaped_str(_wc_json_buf *jb, const char *s) {
    _wc_jb_append_char(jb, '"');
    if (s) {
        const char *p = s;
        while (*p) {
            switch (*p) {
                case '"':  _wc_jb_append(jb, "\\\""); break;
                case '\\': _wc_jb_append(jb, "\\\\"); break;
                case '\n': _wc_jb_append(jb, "\\n"); break;
                case '\r': _wc_jb_append(jb, "\\r"); break;
                case '\t': _wc_jb_append(jb, "\\t"); break;
                default:   _wc_jb_append_char(jb, *p); break;
            }
            p++;
        }
    }
    _wc_jb_append_char(jb, '"');
}

static void _wc_jb_append_indent(_wc_json_buf *jb, int indent, int depth) {
    if (indent <= 0) return;
    _wc_jb_append_char(jb, '\n');
    for (int i = 0; i < indent * depth; i++) {
        _wc_jb_append_char(jb, ' ');
    }
}

/* Forward declare */
static void _wc_json_serialize_typed(_wc_json_buf *jb, int64_t val, int indent, int depth, int type_hint);

/* Helper: serialize pack fields as JSON object (alphabetically sorted) */
static void _wc_json_serialize_pack_fields(_wc_json_buf *jb, int64_t *pack, int64_t fc, int indent, int depth) {
    typedef struct { const char *name; int64_t val; int type_hint; } _WcJsonField;
    _WcJsonField fields[100];
    int nfields = 0;
    for (int64_t i = 0; i < fc && nfields < 100; i++) {
        int64_t field_hash = pack[1 + i * 3];
        int64_t field_val = pack[1 + i * 3 + 2];
        int64_t fname_ptr = taida_lookup_field_name(field_hash);
        const char *fname = (const char *)(intptr_t)fname_ptr;
        if (!fname) continue;
        if (fname[0] == '_' && fname[1] == '_') continue;
        int64_t ftype = taida_lookup_field_type(field_hash, 0);
        fields[nfields].name = fname;
        fields[nfields].val = field_val;
        fields[nfields].type_hint = (int)ftype;
        nfields++;
    }
    for (int i = 1; i < nfields; i++) {
        _WcJsonField tmp = fields[i];
        int j = i - 1;
        while (j >= 0 && _wf_strcmp(fields[j].name, tmp.name) > 0) {
            fields[j + 1] = fields[j];
            j--;
        }
        fields[j + 1] = tmp;
    }
    _wc_jb_append_char(jb, '{');
    for (int i = 0; i < nfields; i++) {
        if (i > 0) _wc_jb_append_char(jb, ',');
        if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
        _wc_jb_append_escaped_str(jb, fields[i].name);
        _wc_jb_append_char(jb, ':');
        if (indent > 0) _wc_jb_append_char(jb, ' ');
        _wc_json_serialize_typed(jb, fields[i].val, indent, depth + 1, fields[i].type_hint);
    }
    if (indent > 0 && nfields > 0) _wc_jb_append_indent(jb, indent, depth);
    _wc_jb_append_char(jb, '}');
}

static void _wc_json_serialize_typed(_wc_json_buf *jb, int64_t val, int indent, int depth, int type_hint) {
    if (type_hint == 4) {
        _wc_jb_append(jb, val ? "true" : "false");
        return;
    }
    if (val == 0) {
        if (type_hint == 3) {
            _wc_jb_append(jb, "\"\"");
        } else {
            _wc_jb_append(jb, "{}");
        }
        return;
    }
    if (type_hint == 1 || type_hint == 2) {
        char *num = _wc_i64_to_str(val);
        _wc_jb_append(jb, num);
        return;
    }
    if (type_hint == 3) {
        const char *s = (const char *)(intptr_t)val;
        _wc_jb_append_escaped_str(jb, s);
        return;
    }

    /* No type hint: heuristic detection */
    if (val < 0 || val > 0xFFFFFFFF) {
        char *num = _wc_i64_to_str(val);
        _wc_jb_append(jb, num);
        return;
    }

    if (val > 0 && val < 256) {
        char *num = _wc_i64_to_str(val);
        _wc_jb_append(jb, num);
        return;
    }

    /* Check HashMap */
    if (_wc_is_hashmap(val)) {
        int64_t *hm = (int64_t *)(intptr_t)val;
        int64_t cap = hm[0];
        _wc_jb_append_char(jb, '{');
        int64_t count = 0;
        for (int64_t i = 0; i < cap; i++) {
            int64_t sh = hm[WASM_HM_HEADER + i * 3];
            int64_t sk = hm[WASM_HM_HEADER + i * 3 + 1];
            if (sh != 0 && !(sh == 1 && sk == 0)) {
                if (count > 0) _wc_jb_append_char(jb, ',');
                if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
                const char *key_str = (const char *)(intptr_t)sk;
                if (!key_str) key_str = "";
                _wc_jb_append_escaped_str(jb, key_str);
                _wc_jb_append_char(jb, ':');
                if (indent > 0) _wc_jb_append_char(jb, ' ');
                _wc_json_serialize_typed(jb, hm[WASM_HM_HEADER + i * 3 + 2], indent, depth + 1, 0);
                count++;
            }
        }
        if (indent > 0 && count > 0) _wc_jb_append_indent(jb, indent, depth);
        _wc_jb_append_char(jb, '}');
        return;
    }

    /* Check Set */
    if (_wc_is_set(val)) {
        int64_t *list = (int64_t *)(intptr_t)val;
        int64_t list_len = list[1];
        _wc_jb_append_char(jb, '[');
        for (int64_t i = 0; i < list_len; i++) {
            if (i > 0) _wc_jb_append_char(jb, ',');
            if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
            _wc_json_serialize_typed(jb, list[4 + i], indent, depth + 1, 0);
        }
        if (indent > 0 && list_len > 0) _wc_jb_append_indent(jb, indent, depth);
        _wc_jb_append_char(jb, ']');
        return;
    }

    /* Check monadic types (Result, Lax) */
    if (_wc_is_result(val) || _wc_is_lax(val)) {
        int64_t *pack = (int64_t *)(intptr_t)val;
        int64_t fc = pack[0];
        _wc_json_serialize_pack_fields(jb, pack, fc, indent, depth);
        return;
    }

    /* Check List */
    if (_wc_looks_like_list(val) && !_wc_is_hashmap(val) && !_wc_is_set(val)) {
        int64_t *list = (int64_t *)(intptr_t)val;
        int64_t list_len = list[1];
        _wc_jb_append_char(jb, '[');
        for (int64_t i = 0; i < list_len; i++) {
            if (i > 0) _wc_jb_append_char(jb, ',');
            if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
            _wc_json_serialize_typed(jb, list[4 + i], indent, depth + 1, 0);
        }
        if (indent > 0 && list_len > 0) _wc_jb_append_indent(jb, indent, depth);
        _wc_jb_append_char(jb, ']');
        return;
    }

    /* Check BuchiPack */
    if (_wc_is_valid_ptr(val, 8)) {
        int64_t *obj = (int64_t *)(intptr_t)val;
        int64_t fc = obj[0];
        if (fc > 0 && fc < 200) {
            int64_t hash0 = obj[1];
            if (hash0 > 0x10000 || hash0 < 0) {
                _wc_json_serialize_pack_fields(jb, obj, fc, indent, depth);
                return;
            }
        }
    }

    /* String pointer */
    if (_wc_looks_like_string(val)) {
        _wc_jb_append_escaped_str(jb, (const char *)(intptr_t)val);
        return;
    }

    /* Default: integer */
    char *num = _wc_i64_to_str(val);
    _wc_jb_append(jb, num);
}

/* jsonEncode: serialize value as JSON (compact) */
int64_t taida_json_encode(int64_t val) {
    _wc_json_buf jb;
    _wc_jb_init(&jb);
    _wc_json_serialize_typed(&jb, val, 0, 0, 0);
    return (int64_t)(intptr_t)jb.buf;
}

/* jsonPretty: serialize value as JSON (indented) */
int64_t taida_json_pretty(int64_t val) {
    _wc_json_buf jb;
    _wc_jb_init(&jb);
    _wc_json_serialize_typed(&jb, val, 2, 0, 0);
    return (int64_t)(intptr_t)jb.buf;
}

/* ── PR-4: Async runtime (synchronous blocking for wasm-min) ──────────────
 *
 * Async layout (7 int64_t slots, matching native_runtime.c):
 *   [0] = WASM_ASYNC_MAGIC  (identifier)
 *   [1] = status: 0=pending, 1=fulfilled, 2=rejected
 *   [2] = value
 *   [3] = error
 *   [4] = 0 (no thread in wasm)
 *   [5] = value_tag
 *   [6] = error_tag
 *
 * In wasm-min, all Async operations are synchronous (immediate resolution).
 * No pending state ever occurs — Async[T]() creates a fulfilled Async.
 */

#define WASM_ASYNC_MAGIC 0x5441494441535900LL  /* "TAIDASY\0" — matches native */
#define WASM_ASYNC_TAG_UNKNOWN (-1)

static int _wasm_is_async_obj(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 7 * 8)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    return p[0] == WASM_ASYNC_MAGIC;
}

int64_t taida_async_ok_tagged(int64_t value, int64_t value_tag) {
    int64_t *obj = (int64_t *)wasm_alloc(7 * 8);
    if (!obj) return 0;
    obj[0] = WASM_ASYNC_MAGIC;
    obj[1] = 1;  /* fulfilled */
    obj[2] = value;
    obj[3] = 0;  /* no error */
    obj[4] = 0;  /* no thread */
    obj[5] = value_tag;
    obj[6] = WASM_ASYNC_TAG_UNKNOWN;
    return (int64_t)(intptr_t)obj;
}

int64_t taida_async_ok(int64_t value) {
    return taida_async_ok_tagged(value, WASM_ASYNC_TAG_UNKNOWN);
}

int64_t taida_async_err(int64_t error) {
    int64_t *obj = (int64_t *)wasm_alloc(7 * 8);
    if (!obj) return 0;
    obj[0] = WASM_ASYNC_MAGIC;
    obj[1] = 2;  /* rejected */
    obj[2] = 0;  /* no value */
    obj[3] = error;
    obj[4] = 0;
    obj[5] = WASM_ASYNC_TAG_UNKNOWN;
    obj[6] = WASM_TAG_PACK;  /* error is always a Pack */
    return (int64_t)(intptr_t)obj;
}

void taida_async_set_value_tag(int64_t async_ptr, int64_t tag) {
    if (!_wasm_is_async_obj(async_ptr)) return;
    ((int64_t *)(intptr_t)async_ptr)[5] = tag;
}

int64_t taida_async_unmold(int64_t async_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return async_ptr;
    int64_t *obj = (int64_t *)(intptr_t)async_ptr;
    int64_t status = obj[1];
    if (status == 1) return obj[2];  /* fulfilled -> value */
    if (status == 2) {
        /* rejected -> throw */
        int64_t error = obj[3];
        if (taida_can_throw_payload(error)) return taida_throw(error);
        int64_t err = taida_make_error(
            (int64_t)(intptr_t)"AsyncError",
            (int64_t)(intptr_t)"Async rejected");
        return taida_throw(err);
    }
    return 0;  /* pending (should not happen in wasm-min) */
}

int64_t taida_async_is_pending(int64_t async_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return 0;
    return ((int64_t *)(intptr_t)async_ptr)[1] == 0 ? 1 : 0;
}

int64_t taida_async_is_fulfilled(int64_t async_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return 0;
    return ((int64_t *)(intptr_t)async_ptr)[1] == 1 ? 1 : 0;
}

int64_t taida_async_is_rejected(int64_t async_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return 0;
    return ((int64_t *)(intptr_t)async_ptr)[1] == 2 ? 1 : 0;
}

int64_t taida_async_get_value(int64_t async_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return 0;
    return ((int64_t *)(intptr_t)async_ptr)[2];
}

int64_t taida_async_get_error(int64_t async_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return 0;
    return ((int64_t *)(intptr_t)async_ptr)[3];
}

int64_t taida_async_map(int64_t async_ptr, int64_t fn_ptr) {
    if (!_wasm_is_async_obj(async_ptr)) return async_ptr;
    int64_t *obj = (int64_t *)(intptr_t)async_ptr;
    if (obj[1] != 1) return async_ptr;  /* not fulfilled -> propagate */
    int64_t value = obj[2];
    int64_t new_val = _wasm_invoke_callback1(fn_ptr, value);
    /* WB-3: map may change type (Int->Str etc), use UNKNOWN since WASM has no detect_value_tag */
    return taida_async_ok_tagged(new_val, WASM_ASYNC_TAG_UNKNOWN);
}

int64_t taida_async_get_or_default(int64_t async_ptr, int64_t fallback) {
    if (!_wasm_is_async_obj(async_ptr)) return async_ptr;
    int64_t *obj = (int64_t *)(intptr_t)async_ptr;
    if (obj[1] == 1) return obj[2];  /* fulfilled */
    return fallback;
}

int64_t taida_async_all(int64_t list_ptr) {
    /* Collect values from all fulfilled Async in the list */
    if (!_looks_like_list(list_ptr)) return taida_async_ok_tagged(list_ptr, WASM_ASYNC_TAG_UNKNOWN);
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];  /* list[1] = length */
    int64_t result = taida_list_new();
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        int64_t value;
        if (_wasm_is_async_obj(item)) {
            int64_t *aobj = (int64_t *)(intptr_t)item;
            if (aobj[1] == 2) {
                /* One rejected -> throw (matches native behavior) */
                int64_t error = aobj[3];
                if (taida_can_throw_payload(error)) return taida_throw(error);
                int64_t err = taida_make_error(
                    (int64_t)(intptr_t)"AsyncError",
                    (int64_t)(intptr_t)"All: async rejected");
                return taida_throw(err);
            }
            value = aobj[2];
        } else {
            value = item;
        }
        result = taida_list_push(result, value);
    }
    return taida_async_ok_tagged(result, WASM_ASYNC_TAG_UNKNOWN);
}

int64_t taida_async_race(int64_t list_ptr) {
    /* Return the first value (all are immediately resolved in wasm-min) */
    if (!_looks_like_list(list_ptr)) return taida_async_ok_tagged(list_ptr, WASM_ASYNC_TAG_UNKNOWN);
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];  /* list[1] = length */
    if (len == 0) return taida_async_ok_tagged(taida_pack_new(0), WASM_TAG_PACK);
    int64_t first = list[WASM_LIST_ELEMS];
    if (_wasm_is_async_obj(first)) {
        int64_t *aobj = (int64_t *)(intptr_t)first;
        if (aobj[1] == 2) {
            /* rejected -> throw (matches native behavior) */
            int64_t error = aobj[3];
            if (taida_can_throw_payload(error)) return taida_throw(error);
            int64_t err = taida_make_error(
                (int64_t)(intptr_t)"AsyncError",
                (int64_t)(intptr_t)"Race: async rejected");
            return taida_throw(err);
        }
        return taida_async_ok_tagged(aobj[2], aobj[5]);
    }
    return taida_async_ok_tagged(first, WASM_ASYNC_TAG_UNKNOWN);
}

int64_t taida_async_cancel(int64_t async_ptr) {
    /* Cancel is no-op in synchronous wasm-min — return the async as-is */
    return async_ptr;
}

int64_t taida_async_spawn(int64_t fn_ptr, int64_t arg) {
    /* Synchronous execution in wasm-min — call the function immediately */
    int64_t value = _wasm_invoke_callback1(fn_ptr, arg);
    return taida_async_ok_tagged(value, WASM_ASYNC_TAG_UNKNOWN);
}

/* Async toString helper */
static int64_t _wasm_async_to_string(int64_t async_ptr) {
    int64_t *obj = (int64_t *)(intptr_t)async_ptr;
    int64_t status = obj[1];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "Async[");
    if (status == 1) {
        _sb_append(&sb, "fulfilled: ");
        int64_t val_str = _wasm_value_to_display_string(obj[2]);
        _sb_append(&sb, (const char *)(intptr_t)val_str);
    } else if (status == 2) {
        _sb_append(&sb, "rejected: ");
        int64_t err_str = _wasm_value_to_display_string(obj[3]);
        _sb_append(&sb, (const char *)(intptr_t)err_str);
    } else {
        _sb_append(&sb, "pending");
    }
    _sb_append(&sb, "]");
    return (int64_t)(intptr_t)sb.buf;
}

/* ── _taida_main: C emitter が生成する関数（extern） ── */

extern int64_t _taida_main(void);

/* ── _start: WASI エントリポイント ── */

void _start(void) {
    _taida_main();
}
