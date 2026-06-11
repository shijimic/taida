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
#define TAIDA_WASM_I32_MAX 2147483647
#define TAIDA_WASM_I64_MAX 9223372036854775807LL
#define TAIDA_WASM_I64_MIN (-9223372036854775807LL - 1LL)

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

/* Allocation counters for the perf measurement build (mirror of the native
   TAIDA_PERF_COUNTERS hooks). WASM execution is single-threaded, so plain
   increments suffice. wasm_perf_dump() runs at the tail of `_start` and
   writes one machine-parsable line to stderr (fd=2) using only stack
   buffers — the dump must not itself allocate, since it reports on the
   allocator. The normal build compiles every hook away to a no-op. */
#ifdef TAIDA_PERF_COUNTERS
static unsigned long long wasm_perf_alloc_calls;
static unsigned long long wasm_perf_alloc_bytes;
static unsigned long long wasm_perf_arena_enters;
#define WASM_PERF_INC(c) ((c) += 1ULL)
#define WASM_PERF_ADD(c, n) ((c) += (unsigned long long)(n))
static int32_t wasm_perf_format_u64(char *dst, unsigned long long v) {
    char tmp[20];
    int32_t n = 0;
    if (v == 0) { dst[0] = '0'; return 1; }
    while (v > 0) { tmp[n++] = (char)('0' + (char)(v % 10ULL)); v /= 10ULL; }
    for (int32_t i = 0; i < n; i++) dst[i] = tmp[n - 1 - i];
    return n;
}
static void wasm_perf_append(char *buf, int32_t *off, const char *s) {
    while (*s) buf[(*off)++] = *s++;
}
static void wasm_perf_dump(void) {
    char buf[128];
    int32_t off = 0;
    wasm_perf_append(buf, &off, "TAIDA_PERF wasm alloc_calls=");
    off += wasm_perf_format_u64(buf + off, wasm_perf_alloc_calls);
    wasm_perf_append(buf, &off, " alloc_bytes=");
    off += wasm_perf_format_u64(buf + off, wasm_perf_alloc_bytes);
    wasm_perf_append(buf, &off, " arena_enters=");
    off += wasm_perf_format_u64(buf + off, wasm_perf_arena_enters);
    buf[off++] = '\n';
    wasi_ciovec iov;
    iov.buf = (int32_t)(intptr_t)buf;
    iov.len = off;
    int32_t nwritten;
    __wasi_fd_write(2, &iov, 1, &nwritten);
}
#else
#define WASM_PERF_INC(c) ((void)0)
#define WASM_PERF_ADD(c, n) ((void)0)
#endif

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

/* Loud failure exit shared by the slow path below. */
__attribute__((cold, noinline, noreturn))
static void wasm_alloc_fail(const char *msg, int32_t len) {
    wasi_ciovec iov;
    iov.buf = (int32_t)(intptr_t)msg;
    iov.len = len;
    int32_t nwritten;
    __wasi_fd_write(2, &iov, 1, &nwritten);
    __builtin_trap();
}

/* Slow path of wasm_alloc, kept out of line. Two jobs:
 *
 * 1. Hard heap ceiling at 2GB: pointers travel the value space as
 *    int64_t via sign-EXTENDING (intptr_t) casts, so an address past
 *    2^31 turns negative and every pointer-validity check rejects it
 *    — results silently degrade to empty objects long before the
 *    4GB linear-memory limit. Trap loudly at the real boundary
 *    instead. (Lifting the ceiling means zero-extending every
 *    pointer-to-value conversion — a representation-level change
 *    tracked separately.)
 * 2. Page growth, trapping loudly on exhaustion: returning NULL here
 *    used to flow into list/pack constructors as an empty object and
 *    silently corrupt results (a 10k-append loop run twice produced
 *    an empty second list instead of failing).
 *
 * Both used to live inside wasm_alloc itself, which pushed the
 * allocator past clang's inlining threshold; taida_list_push (which
 * inlines the allocator) in turn stopped being inlined into the list
 * runtime, and alloc-heavy benchmarks regressed ~20%. The hot path
 * keeps a single combined unlikely-branch. */
__attribute__((cold, noinline))
static void wasm_alloc_slow(void) {
    if (bump_ptr > 0x7FFF0000u) {
        static const char ceil_msg[] = "taida: wasm heap ceiling reached (2GB address space for boxed values)\n";
        wasm_alloc_fail(ceil_msg, (int32_t)(sizeof(ceil_msg) - 1));
    }
    unsigned int pages_needed = (bump_ptr + 65535) / 65536;
    unsigned int current_pages = __builtin_wasm_memory_size(0);
    if (pages_needed > current_pages) {
        int grew = __builtin_wasm_memory_grow(0, pages_needed - current_pages);
        if (grew == -1) {
            static const char oom_msg[] = "taida: wasm linear memory exhausted (allocation failed)\n";
            wasm_alloc_fail(oom_msg, (int32_t)(sizeof(oom_msg) - 1));
        }
    }
}

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
    unsigned int next = result + size; /* may wrap; tested below */
    WASM_PERF_INC(wasm_perf_alloc_calls);
    WASM_PERF_ADD(wasm_perf_alloc_bytes, size);

    /* Ceiling / wrap / page-growth check, one unlikely branch (see
       wasm_alloc_slow). `next < result` detects the unsigned wrap a
       huge size could take — a wrapped bump pointer would otherwise
       slip back into the valid range. The ceiling is tested on every
       allocation, not only when a grow is due:
       TAIDA_WASM_INITIAL_PAGES can pre-size memory past 2GB, in which
       case the bump pointer crosses the boundary without ever needing
       a grow. */
    unsigned int pages_needed = (next + 65535) / 65536;
    if (__builtin_expect(next > 0x7FFF0000u || next < result
                             || pages_needed > __builtin_wasm_memory_size(0),
                         0)) {
        /* Saturate on ceiling/wrap so the slow path's test fires (it
           traps; only the page-growth case returns). */
        bump_ptr = (next > 0x7FFF0000u || next < result) ? 0x7FFF0008u : next;
        wasm_alloc_slow();
        return (void *)(unsigned long)result;
    }
    bump_ptr = next;

    return (void *)(unsigned long)result;
}

/* ── F58: positive string identification (hidden one-word header) ──
 *
 * Every Taida Str value on WASM points just past a single hidden magic
 * word: [WASM_STR_MAGIC] | bytes...\0 for heap strings built through
 * _wasm_str_alloc, and [WASM_STR_STATIC_MAGIC] | bytes...\0 for the
 * static literals the emitter lays out as file-scope structs. The
 * previous identification ("address in a plausible range and the first
 * byte is printable ASCII") misread a monotonically growing Int
 * accumulator as a live string the moment its value coincided with an
 * address the bump allocator had handed to a string. No length field is
 * kept — string length stays NUL-scan (wasm_strlen) exactly as before,
 * which is what makes the allocation-site conversion purely mechanical.
 */
#define WASM_STR_MAGIC        0x5441494453545200LL /* "TAIDSTR\0" */
#define WASM_STR_STATIC_MAGIC 0x5441494453545300LL /* "TAIDSTS\0" */

/* Header-carrying string literal for runtime code. Every string that
   enters the Taida value space must carry the magic word — the
   identification is positive-only (no byte heuristics), so a bare C
   literal cast to int64_t is invisible to _wasm_is_string_ptr and
   renders wrong. The statement expression creates one function-local
   static per use site; sizeof(str) keeps the NUL. */
#define WSTR(str) \
    ({ \
        static const struct { \
            int64_t m; \
            char s[sizeof(str)]; \
        } _wstr_lit = {WASM_STR_STATIC_MAGIC, str}; \
        (int64_t)(intptr_t)_wstr_lit.s; \
    })

/* Allocate a string buffer of `total` bytes (the caller's historical
 * request, NUL included by the caller's own accounting) behind a hidden
 * magic word. Returns the data pointer; wasm_alloc's 8-byte alignment
 * keeps both the header and the data aligned. */
/* Non-static: the profile runtimes (edge host / wasi io / full / abi
   web) build strings in their own translation units and must produce
   header-carrying ones — identification is positive-only. */
char *_wasm_str_alloc(unsigned int total) {
    int64_t *hdr = (int64_t *)wasm_alloc(total + 8);
    if (!hdr) return (char *)0;
    hdr[0] = WASM_STR_MAGIC;
    return (char *)(hdr + 1);
}

/* C25B-026 / Phase 5-G (2026-04-23): Arena-scoped bump release.
 *
 * The WASM bump allocator never frees, so a long-running loop that
 * produces short-lived allocations (`@[Float]` per-iteration scratch,
 * LLM forward-pass per-token kernels) grows `bump_ptr` linearly until
 * `memory.grow` runs out of pages and traps.
 *
 * These three helpers expose a watermark-style "arena frame" over the
 * existing bump allocator:
 *   * `wasm_arena_enter()` snapshots the current `bump_ptr` and
 *     returns it as an opaque i32 handle.
 *   * `wasm_arena_leave(saved)` restores `bump_ptr` to the snapshot,
 *     effectively releasing every allocation made since the matching
 *     `enter`. No memory is handed back to the host (WebAssembly has
 *     no `memory.shrink`), but the pages become reusable — the next
 *     `wasm_alloc` call reuses the same addresses without triggering
 *     a new `memory.grow`.
 *   * `wasm_arena_used()` returns the number of bytes currently held
 *     in the heap since initialisation, useful for regression tests.
 *
 * These helpers are infrastructure; Taida user code does not call them
 * directly. The lowerer can inject enter/leave pairs around
 * escape-analysis-safe scopes (Phase 5-I or later). Today the helpers
 * let regression tests assert that a bounded-scope loop does not
 * monotonically leak heap across iterations.
 *
 * The allocator lazy-initialises `bump_ptr` on first `wasm_alloc`; the
 * helpers call `wasm_alloc(0)` to force initialisation so a caller
 * that enters an arena before any allocation still observes a stable
 * `__heap_base` snapshot.
 *
 * The symbols are NOT marked `__attribute__((used))` — we intentionally
 * rely on `wasm-ld --gc-sections` to drop them for size-sensitive
 * profiles (wasm-min / wasm-edge). The wasm-wasi and wasm-full
 * compilation paths add `--export=wasm_arena_{enter,leave,used,
 * roundtrip_test}` via `wasm_arena_export_flags` in
 * `src/codegen/driver.rs`, and those `--export=` flags simultaneously
 * pin the symbols against GC and make them callable from wasmtime /
 * host harnesses. For wasm-min / wasm-edge the symbols fall out of
 * the module entirely, preserving the `tests/wasm_min.rs::
 * wasm_min_size_gate` 512-byte hello.wasm ceiling.
 */

int32_t wasm_arena_enter(void) {
    if (bump_ptr == 0) {
        /* Force lazy init so the first enter captures a real __heap_base. */
        (void)wasm_alloc(0);
    }
    WASM_PERF_INC(wasm_perf_arena_enters);
    return (int32_t)bump_ptr;
}

void wasm_arena_leave(int32_t saved) {
    unsigned int target = (unsigned int)saved;
    /* Guard against restoring forward (nested / corrupted callers).
     * A forward restore would alias live data owned by the outer
     * arena, which is strictly worse than leaking a frame. Silently
     * ignore such calls — the caller made a pairing mistake. */
    if (target == 0 || target > bump_ptr) {
        return;
    }
    bump_ptr = target;
}

/* F58 P2-2: shared-IR iteration-scope hooks. The lowerer emits the same
   taida_arena_iter_* calls for every backend; on WASM the bump
   allocator's single watermark maps directly onto wasm_arena_enter /
   wasm_arena_leave. There are no freelists here, so exit is a no-op. */
int64_t taida_arena_iter_enter(void) {
    return (int64_t)wasm_arena_enter();
}

int64_t taida_arena_iter_reset(int64_t mark) {
    wasm_arena_leave((int32_t)mark);
    return 0;
}

int64_t taida_arena_iter_exit(int64_t mark) {
    (void)mark;
    return 0;
}

int32_t wasm_arena_used(void) {
    if (bump_ptr == 0) {
        return 0;
    }
    extern unsigned int __heap_base;
    unsigned int base = (unsigned int)(unsigned long)&__heap_base;
    base = (base + 7) & ~7u;
    if (bump_ptr < base) {
        return 0;
    }
    return (int32_t)(bump_ptr - base);
}

/* C25B-026 / Phase 5-G regression self-test.
 *
 * Exported solely so `tests/wasm_wasi.rs::wasm_wasi_arena_release_is_bounded`
 * can invoke it via `wasmtime --invoke` to confirm that an enter / leave
 * pair around a bounded loop releases every allocation made inside the
 * scope. The function performs `iters` rounds of
 *   saved = wasm_arena_enter();
 *   for (j = 0; j < inner; j++) (void)wasm_alloc(64);
 *   wasm_arena_leave(saved);
 * and returns the final `wasm_arena_used()` minus the baseline before
 * the first enter. If the arena release works, the delta is 0. If it
 * leaks (old behaviour), the delta is `iters * inner * 64` bytes.
 *
 * `iters` and `inner` are int32_t; the function takes no real arguments
 * (wasmtime's `--invoke` passes decimal args via command line, but we
 * hardcode the loop shape to keep the test harness trivial). Parameters
 * are two int32_t so that if we ever want to parametrise from the
 * harness, the hook is already there. */
int32_t wasm_arena_roundtrip_test(int32_t iters, int32_t inner) {
    if (iters <= 0) iters = 128;
    if (inner <= 0) inner = 16;
    /* Force init + capture baseline. */
    (void)wasm_alloc(0);
    int32_t baseline = wasm_arena_used();
    for (int32_t i = 0; i < iters; i++) {
        int32_t saved = wasm_arena_enter();
        for (int32_t j = 0; j < inner; j++) {
            (void)wasm_alloc(64);
        }
        wasm_arena_leave(saved);
    }
    int32_t end = wasm_arena_used();
    return end - baseline;
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
/// Terminator-driven: no up-front strlen of the haystack. The previous
/// version measured the whole remaining haystack on every call, which
/// turned every scan-forward caller (Split / Replace walk a cursor
/// through the string calling this per occurrence) into O(n^2) — a
/// 10KB string split into a thousand parts re-measured ~5MB of tail
/// per pipeline pass.
static const char *_wf_strstr(const char *haystack, const char *needle) {
    if (!haystack || !needle) return (const char *)0;
    if (!needle[0]) return haystack;
    for (const char *h = haystack; *h; h++) {
        if (*h != needle[0]) continue;
        const char *a = h + 1;
        const char *b = needle + 1;
        while (*a && *b && *a == *b) { a++; b++; }
        if (!*b) return h;
    }
    return (const char *)0;
}

static int _wf_is_whitespace(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

/* B11-2: Type tag constants (early definition for taida_io_stdout_with_tag) */
#ifndef WASM_TAG_INT
#define WASM_TAG_INT     0
#define WASM_TAG_FLOAT   1
#define WASM_TAG_BOOL    2
#define WASM_TAG_STR     3
#define WASM_TAG_PACK    4
#endif
/* Value-tag track (mirror of native core.c): the remaining heap kinds plus
   the two new kinds. Enum gets its own kind (it was lowered as bare INT
   ordinals, making same-ordinal variants of different enums and Int(n)
   indistinguishable inside containers) and Bytes is split out of the PACK
   umbrella so kind-aware equality can short-circuit on it. These names are
   deliberately in the WASM_TAG_* namespace; the pack field enum descriptor
   registry uses its own unrelated numeric scheme. */
#ifndef WASM_TAG_LIST
#define WASM_TAG_LIST    5
#define WASM_TAG_CLOSURE 6
#define WASM_TAG_HMAP    7
#define WASM_TAG_SET     8
#define WASM_TAG_ENUM    9
#define WASM_TAG_BYTES   10
#endif
/* C23B-007 (2026-04-22): HETEROGENEOUS sentinel, distinct from UNKNOWN(-1).
   Used by `taida_list_set_elem_tag` / `taida_hashmap_set_value_tag` after a
   type-conflict downgrade so subsequent `.set()` / `.push()` calls cannot
   re-promote the container to a single primitive tag. Renderers that read
   `list[2]` / `hm[2]` must treat -2 the same as -1 (fall back to per-element
   structural dispatch), but the setters must NOT treat -2 as "unset" — once
   heterogeneous, the container stays heterogeneous for its lifetime. */
#ifndef WASM_TAG_UNKNOWN
#define WASM_TAG_UNKNOWN       -1
#define WASM_TAG_HETEROGENEOUS -2
#endif

/* ── Per-element kind array (value-tag track, mirror of native core.c) ──────
 * The elem-tag slot of a WASM container (list[2] / set[2] / hmap[2] — note
 * the WASM layout puts elem_tag at index 2, length at index 1, unlike the
 * native [rc|magic, cap, len, elem_tag]) is three-state:
 *   v >= 0                    homogeneous WASM_TAG_* (legacy behaviour)
 *   v == -1 / -2              UNKNOWN / HETEROGENEOUS with NO per-element
 *                             info (legacy degraded state; also the
 *                             conservative result of derived containers
 *                             whose source had a kind array)
 *   v >= 4096 (pointer)       per-element kind array, implicitly
 *                             HETEROGENEOUS. Layout: u32[0] = capacity,
 *                             u32[1 + i] = kind|aux for element i (kept in
 *                             lockstep with the container length at [1]).
 * Each u32 entry packs the kind in the low 8 bits and an auxiliary payload
 * (the enum type id for WASM_TAG_ENUM) in the upper 24 bits. 0xFF in the
 * kind byte means "unknown" (e.g. a dynamically-typed push). The array is
 * plain POD owned by the container. WASM uses a bump allocator with no
 * free, so there is no per-element release path and no free helper: the
 * array is reclaimed wholesale on arena reset. Growth re-bumps a larger
 * block and memcpys (copy-on-grow, like taida_list_push).
 *
 * Raw reads/writes of slot [2] outside the helpers below are forbidden —
 * every legacy direct access has been rewritten onto this API so a slot
 * holding an array pointer can never be misread as a plain tag. */
#define WASM_EKIND_UNKNOWN 0xFFu
#define WASM_EKIND(kind, aux) ((uint32_t)((uint32_t)((kind) & 0xFF) | ((uint32_t)(aux) << 8)))
#define WASM_EKIND_KIND(ek)  ((uint32_t)((ek) & 0xFFu))
#define WASM_EKIND_AUX(ek)   ((uint32_t)((ek) >> 8))

static int _wasm_elem_slot_is_array(int64_t v) { return v >= 4096; }

/* Classify the slot for kind-level decisions: returns the homogeneous tag,
   WASM_TAG_UNKNOWN, or WASM_TAG_HETEROGENEOUS (for both the array-less
   degraded state and the array-carrying state). */
static int64_t _wasm_elem_tag_kind(const int64_t *c) {
    int64_t v = c[2];
    if (_wasm_elem_slot_is_array(v)) return WASM_TAG_HETEROGENEOUS;
    return v;
}

/* Read the slot for propagation into a DERIVED container. Never returns the
   array pointer: a derived container must build its own array (via the
   append helper inside its own loop) or conservatively degrade to
   HETEROGENEOUS-without-array, which downstream consumers treat exactly
   like the legacy mixed state. */
static int64_t _wasm_elem_tag_for_propagation(const int64_t *c) {
    int64_t v = c[2];
    if (_wasm_elem_slot_is_array(v)) return WASM_TAG_HETEROGENEOUS;
    return v;
}

/* Per-element kind|aux for element i. Homogeneous containers report the
   single tag (aux 0), array carriers report the recorded entry, and the
   array-less UNKNOWN/HETEROGENEOUS states report EKIND unknown. */
static uint32_t _wasm_elem_kind_at(const int64_t *c, int64_t i) {
    int64_t v = c[2];
    if (_wasm_elem_slot_is_array(v)) {
        const uint32_t *arr = (const uint32_t *)(intptr_t)v;
        if (i >= 0 && i < c[1]) return arr[1 + i];
        return WASM_EKIND_UNKNOWN;
    }
    if (v >= 0) return WASM_EKIND((uint32_t)v, 0);
    return WASM_EKIND_UNKNOWN;
}

/* Append one kind entry to a container that already carries an array,
   growing it (copy-on-grow via the bump allocator) when the recorded
   capacity is exhausted. `idx` is the element index the entry belongs to
   (== container length at the matching pre-push call site). */
static void _wasm_elem_tags_append(int64_t *c, int64_t idx, uint32_t ekind) {
    uint32_t *arr = (uint32_t *)(intptr_t)c[2];
    uint32_t cap = arr[0];
    if ((uint64_t)idx + 1 > cap) {
        uint32_t new_cap = cap ? cap * 2 : 16;
        while (new_cap < (uint64_t)idx + 1) new_cap *= 2;
        uint32_t *bigger = (uint32_t *)wasm_alloc(
            (unsigned int)(((uint64_t)new_cap + 1) * sizeof(uint32_t)));
        for (uint32_t k = 0; k <= cap; k++) bigger[k] = arr[k];
        bigger[0] = new_cap;
        arr = bigger;
        c[2] = (int64_t)(intptr_t)arr;
    }
    arr[1 + idx] = ekind;
}

/* Materialise the per-element array for a container whose first `len`
   elements all carry `old_tag` (guaranteed by the homogeneous latch at the
   downgrade point), then record `new_ekind` for the element about to be
   appended at index `len`. */
static void _wasm_elem_tags_materialise(int64_t *c, int64_t old_tag,
                                        int64_t len, uint32_t new_ekind) {
    uint32_t cap = 16;
    while (cap < (uint64_t)len + 1) cap *= 2;
    uint32_t *arr = (uint32_t *)wasm_alloc(
        (unsigned int)(((uint64_t)cap + 1) * sizeof(uint32_t)));
    arr[0] = cap;
    uint32_t old_ekind = old_tag >= 0 ? WASM_EKIND((uint32_t)old_tag, 0)
                                      : WASM_EKIND_UNKNOWN;
    for (int64_t i = 0; i < len; i++) arr[1 + i] = old_ekind;
    arr[1 + len] = new_ekind;
    c[2] = (int64_t)(intptr_t)arr;
}

/* Map a per-element kind entry back onto the WASM_TAG_* domain
   (EKIND unknown → WASM_TAG_UNKNOWN). */
static int64_t _wasm_ekind_to_tag(uint32_t ek) {
    uint32_t k = WASM_EKIND_KIND(ek);
    return k == WASM_EKIND_UNKNOWN ? WASM_TAG_UNKNOWN : (int64_t)k;
}

/* Record the kind entry for the element about to be pushed (pre-push
   contract: the append index is the container length at [1]). Mirrors
   taida_list_set_elem_tag but takes a full kind|aux entry so enum type ids
   survive container projection. A fresh container stays homogeneous as long
   as entries are plain same-kind tags; an aux-carrying entry (enum) or a
   kind conflict materialises the array. Used by the runtime-internal
   builders (unique / set_from_list / Set ops) when projecting from an
   array-carrying source. */
int64_t taida_list_push(int64_t list_ptr, int64_t item); /* fwd */
static void _wasm_elem_tags_note_push_ek(int64_t *c, uint32_t ek); /* fwd */

/* Project one element (with its source-recorded kind) into a derived
   container, then push (native taida_list_project_push mirror — no
   retain on WASM's bump runtime). */
static int64_t _wasm_list_project_push(int64_t dst, int64_t item,
                                       const int64_t *src, int64_t src_idx) {
    _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)dst, _wasm_elem_kind_at(src, src_idx));
    return taida_list_push(dst, item);
}

/* True when two single-tag containers pin DIFFERENT concrete tags
   (native taida_elem_tags_cross mirror). */
static int _wasm_elem_tags_cross(const int64_t *a, const int64_t *b) {
    int64_t ta = a[2], tb = b[2];
    return ta >= 0 && tb >= 0 && ta != tb;
}

/* Kind entry → pack field tag domain (native mirror): known kinds map
   directly except ENUM, which falls back to the caller's legacy tag. */
static int64_t _wasm_ekind_to_pack_tag(uint32_t ek, int64_t fallback) {
    uint32_t k = ek & 0xFFu;
    if (k == WASM_EKIND_UNKNOWN || k == (uint32_t)WASM_TAG_ENUM) return fallback;
    return (int64_t)k;
}

static void _wasm_elem_tags_note_push_ek(int64_t *c, uint32_t ek) {
    int64_t existing = c[2];
    if (_wasm_elem_slot_is_array(existing)) {
        _wasm_elem_tags_append(c, c[1], ek);
        return;
    }
    if (existing == WASM_TAG_HETEROGENEOUS) return; /* degraded: stay degraded */
    uint32_t k = WASM_EKIND_KIND(ek);
    int64_t tag = (k == WASM_EKIND_UNKNOWN) ? WASM_TAG_UNKNOWN : (int64_t)k;
    if (WASM_EKIND_AUX(ek) == 0) {
        if (existing == WASM_TAG_UNKNOWN || existing == tag) {
            c[2] = tag;
            return;
        }
        _wasm_elem_tags_materialise(c, existing, c[1], ek);
        return;
    }
    /* Aux-carrying (enum) entry: the single slot cannot hold the type id,
       so go straight to the array form (existing elements keep their
       homogeneous tag, or UNKNOWN on a fresh container). */
    _wasm_elem_tags_materialise(c, existing, c[1], ek);
}

/* B11-2: Forward declaration for polymorphic display */
int64_t taida_polymorphic_to_string(int64_t obj);
/* C21-4: Forward declaration for FLOAT tag fast path in stdout_with_tag */
int64_t taida_float_to_str(int64_t val);
/* C21B-seed-07: forward declarations for the BuchiPack-aware stdout
   dispatcher used by `stdout_with_tag` / `stderr_with_tag` below. All
   three definitions sit further down in the file because they depend
   on the Lax / Pack helpers declared later. `_looks_like_list` and
   `_is_wasm_hashmap` are used by `_is_pack_for_stdout` to distinguish
   a pack pointer from a List / HashMap / Set at runtime. */
static int _looks_like_pack(int64_t val);
/* C23B-003 reopen 3: empty pack (`@()` / `Value::Unit`) is allocated by
   `taida_pack_new(0)` and stores `field_count = 0` in its first slot.
   `_looks_like_pack` rejects this because it requires `fc >= 1` (the
   heuristic was tuned to avoid Lists / HashMaps whose header layout
   overlaps with non-empty pack signatures). So without a dedicated
   detector, any nested `@()` inside a Lax / Pack / HashMap / Set / List
   falls through to `taida_int_to_str(val)` and renders as a raw pointer
   integer — breaking 4-backend parity with interpreter / JS / native,
   all of which render it as the string `"@()"`.
   C23B-003 reopen 4 (2026-04-22): switched from a heap-range + zero-slot
   heuristic to a positive-identification magic sentinel. `taida_pack_new(0)`
   now stamps `WASM_EMPTY_PACK_MAGIC` in slot[1], so we can identify an
   empty pack by exact tag match instead of address guesses. This closes
   the HIGH-severity false-positive where dynamic Int expressions
   (e.g. `Str[a + b]()`) whose bit pattern happens to fall inside the
   bump arena and point at a zero int64_t chunk were mis-rendered as
   `@()`. */
#define WASM_EMPTY_PACK_MAGIC 0x5441494450414B55LL  /* "TAIDPAKU" (U=Unit) */
/* Tail sentinel that taida_pack_new stamps after the last field slot of
   every NON-empty pack (definition site of the layout lives with
   taida_pack_new below; the constant is hoisted here so the unmold
   guard can verify pack shape positively). */
#define WASM_PACK_MAGIC 0x54414944504B4B00LL  /* "TAIDPKK\0" (non-empty) */
static int _looks_like_empty_pack(int64_t val);
static int _looks_like_list(int64_t ptr);
static int _is_wasm_hashmap(int64_t ptr);
static int _is_wasm_set(int64_t ptr);
static int _wasm_is_async_obj(int64_t ptr);
static int64_t _wasm_stdout_display_string(int64_t obj);

/* Tight BuchiPack check — `_looks_like_pack` alone false-positives on
   Lists and HashMaps because its structural signature (fc in 1..100 +
   first hash non-zero) overlaps with the list header layout (cap=small,
   len<=cap produces a `fc` in that range). `stdout_with_tag` must not
   reroute a List through the pack-display path, or `@[1, 2, 3]` gets
   rendered as `@()`.  */
static int _is_pack_for_stdout(int64_t val) {
    if (val < 4096) return 0;
    if (_looks_like_list(val)) return 0;
    if (_is_wasm_hashmap(val)) return 0;
    if (_is_wasm_set(val)) return 0;
    if (_wasm_is_async_obj(val)) return 0;
    return _looks_like_pack(val);
}

/* ── taida_io_stdout: stdout 出力（boxed 文字列ポインタ） ──
   C12-5 (FB-18): returns the UTF-8 byte length of the payload as Int.
   The trailing newline is NOT counted — matches interpreter / native. */

int64_t taida_io_stdout(int64_t val_ptr) {
    const char *s = (const char *)(intptr_t)val_ptr;
    if (s) {
        int32_t len = wasm_strlen(s);
        write_stdout(s, len);
        write_stdout("\n", 1);
        return (int64_t)len;
    }
    return 0;
}

/* B11-2a: Type-tagged stdout for Bool display parity (FB-3).
   B11-2f / C12-1: Only Bool needs type-based dispatch at this layer.
   All other tags (including UNKNOWN and the statically-known STR case)
   fall through to the raw `char*` path — matching the pre-B11
   `taida_io_stdout(val_ptr)` behaviour.

   C12B-016 (2026-04-15): The codegen-side `convert_to_string` fallback
   was removed. `stdout(...)` now dispatches in two paths at the call
   site (see `src/codegen/lower/expr.rs`):

     * compile-time-known Str (`TAG_STR == 3`) → plain
       `taida_io_stdout(char*)` (fast path, keeps wasm-min Hello World
       at 340 bytes).
     * everything else → `taida_io_stdout_with_tag(val, tag)` → this
       function. Bool is handled inline; every other tag routes through
       `taida_polymorphic_to_string`, matching the Native backend's
       contract and C12B-034 hardening.

   C12-5: returns bytes written (Int). */
int64_t taida_int_to_str(int64_t a); /* fwd: early Int display path */

int64_t taida_io_stdout_with_tag(int64_t val, int64_t tag) {
    if ((int)tag == WASM_TAG_BOOL) {
        int32_t len = val ? 4 : 5;
        if (val) { write_stdout("true", 4); } else { write_stdout("false", 5); }
        write_stdout("\n", 1);
        return (int64_t)len;
    }
    /* Statically-known Int: the value IS the integer — format it
       directly, before any runtime re-detection. An Int whose value
       coincides with a live string's data address carries a REAL magic
       word at v-8, so the detection heuristics below cannot tell it
       from a string and would print that string instead. */
    if ((int)tag == WASM_TAG_INT) {
        int64_t str_v = taida_int_to_str(val);
        const char *si = (const char *)(intptr_t)str_v;
        int32_t len = wasm_strlen(si);
        write_stdout(si, len);
        write_stdout("\n", 1);
        return (int64_t)len;
    }
    /* C21B-seed-07: BuchiPack values (including Lax / Result / Gorillax
       and user-defined packs) must render in the interpreter-parity
       full form `@(f <= …, …)`. Detect at runtime so a value whose
       compile-time tag was mis-stated (e.g. `Float[x]()` previously
       tagged as FLOAT by `mold_returns.rs`) still routes correctly.
       The runtime detector catches:
         * `mold_returns.rs` fallout: `Int[]/Float[]/Bool[]/Str[]`
           compile-time tag is now Pack (4) — the branch below handles it.
         * UNKNOWN (-1) / Int / Float values that happen to be a valid
           pack pointer at runtime (e.g. user-fn return type not
           statically inferred).
       The 4096 guard keeps low integers from mis-identifying as packs. */
    if (_is_pack_for_stdout(val)) {
        int64_t str_v = _wasm_stdout_display_string(val);
        const char *sp = (const char *)(intptr_t)str_v;
        if (sp) {
            int32_t len = wasm_strlen(sp);
            write_stdout(sp, len);
            write_stdout("\n", 1);
            return (int64_t)len;
        }
        return 0;
    }
    /* C12B-034 fix: for WASM_TAG_STR the incoming `val` is already a
       char*, so the fast path below is safe. For any other tag
       (Int / Float / runtime-unknown UNKNOWN=-1 / pack / list / ...),
       casting a non-pointer integer to `char*` and calling `wasm_strlen`
       is undefined behaviour and can cause SIGSEGV / memory corruption
       when a non-Bool value flows through a `param_tag_vars` ident into
       `stdout(v)`. Route those through the polymorphic formatter so the
       failure mode is well-defined and matches the Native backend's
       `taida_polymorphic_to_string` contract. */
    if ((int)tag == WASM_TAG_STR) {
        const char *s = (const char *)(intptr_t)val;
        if (s) {
            int32_t len = wasm_strlen(s);
            write_stdout(s, len);
            write_stdout("\n", 1);
            return (int64_t)len;
        }
        return 0;
    }
    /* C21-4 / seed-03: Float tag path — decode the i64 bit-pattern via
       `taida_float_to_str` so `stdout(triple(4.0))` prints `12.0` instead
       of the raw bit pattern. Without this, the value flows through
       `taida_polymorphic_to_string` → `_wasm_value_to_display_string`
       which has no tag context and renders the f64 bits as an int64_t. */
    if ((int)tag == WASM_TAG_FLOAT) {
        int64_t str_v = taida_float_to_str(val);
        const char *sf = (const char *)(intptr_t)str_v;
        if (sf) {
            int32_t len = wasm_strlen(sf);
            write_stdout(sf, len);
            write_stdout("\n", 1);
            return (int64_t)len;
        }
        return 0;
    }
    /* Non-Bool, non-Str, non-Float tag → let the polymorphic converter
       produce a safe char* representation. Handles Int / Pack / List /
       UNKNOWN(-1) runtime values emitted through `param_tag_vars`. */
    int64_t str_v = taida_polymorphic_to_string(val);
    const char *s2 = (const char *)(intptr_t)str_v;
    if (s2) {
        int32_t len = wasm_strlen(s2);
        write_stdout(s2, len);
        write_stdout("\n", 1);
        return (int64_t)len;
    }
    return 0;
}

/* ── taida_io_stderr: stderr 出力（wasm-min では stdout にフォールバック） ──
   C12-5 (FB-18): returns bytes written (Int), newline excluded. */

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
        return (int64_t)len;
    }
    return 0;
}

/* B11-2a: Type-tagged stderr for Bool display parity (FB-3).
   B11-2f / C12-1: Only Bool needs type-based dispatch; see stdout_with_tag.
   C12B-016 (2026-04-15): Same two-path dispatch as stdout_with_tag —
   codegen routes compile-time-known Str literals through the plain
   `taida_io_stderr(char*)` entry, and everything else reaches this
   polymorphic dispatcher.
   C12-5: returns bytes written (Int). */
int64_t taida_io_stderr_with_tag(int64_t val, int64_t tag) {
    if ((int)tag == WASM_TAG_BOOL) {
        const char *s = val ? "true" : "false";
        int32_t len = val ? 4 : 5;
        wasi_ciovec iov;
        iov.buf = (int32_t)(intptr_t)s;
        iov.len = len;
        int32_t nwritten;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        iov.buf = (int32_t)(intptr_t)"\n";
        iov.len = 1;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        return (int64_t)len;
    }
    /* Statically-known Int: format directly, before runtime
       re-detection — symmetric with taida_io_stdout_with_tag. */
    if ((int)tag == WASM_TAG_INT) {
        int64_t str_v = taida_int_to_str(val);
        const char *si = (const char *)(intptr_t)str_v;
        int32_t len = wasm_strlen(si);
        wasi_ciovec iov;
        iov.buf = (int32_t)(intptr_t)si;
        iov.len = len;
        int32_t nwritten;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        iov.buf = (int32_t)(intptr_t)"\n";
        iov.len = 1;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        return (int64_t)len;
    }
    /* C21B-seed-07: symmetric with stdout — render buchi packs in
       interpreter-parity full form. Runtime detection catches the
       `Float[x]()`/`Int[x]()`/`Bool[x]()`/`Str[x]()` Lax-return shape
       that used to crash through a FLOAT / STR fast path below. */
    if (_is_pack_for_stdout(val)) {
        int64_t str_v = _wasm_stdout_display_string(val);
        const char *sp = (const char *)(intptr_t)str_v;
        if (sp) {
            int32_t plen = wasm_strlen(sp);
            wasi_ciovec iov;
            iov.buf = (int32_t)(intptr_t)sp;
            iov.len = plen;
            int32_t nwritten;
            __wasi_fd_write(2, &iov, 1, &nwritten);
            iov.buf = (int32_t)(intptr_t)"\n";
            iov.len = 1;
            __wasi_fd_write(2, &iov, 1, &nwritten);
            return (int64_t)plen;
        }
        return 0;
    }
    /* C12B-034 fix: same rationale as taida_io_stdout_with_tag — treat
       non-Bool / non-Str tags as polymorphic values rather than blindly
       casting to char*.
       C21-4: FLOAT tag routes through taida_float_to_str (bit-pattern
       decode) so the i64 bits are rendered as a proper "12.0" decimal. */
    const char *s;
    int32_t len;
    int64_t scratch = 0;
    if ((int)tag == WASM_TAG_STR) {
        s = (const char *)(intptr_t)val;
    } else if ((int)tag == WASM_TAG_FLOAT) {
        scratch = taida_float_to_str(val);
        s = (const char *)(intptr_t)scratch;
    } else {
        scratch = taida_polymorphic_to_string(val);
        s = (const char *)(intptr_t)scratch;
    }
    if (s) {
        len = wasm_strlen(s);
        wasi_ciovec iov;
        iov.buf = (int32_t)(intptr_t)s;
        iov.len = len;
        int32_t nwritten;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        iov.buf = (int32_t)(intptr_t)"\n";
        iov.len = 1;
        __wasi_fd_write(2, &iov, 1, &nwritten);
        return (int64_t)len;
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

int64_t taida_int_add(int64_t a, int64_t b) { return (int64_t)(((uint64_t)a) + ((uint64_t)b)); }
int64_t taida_int_sub(int64_t a, int64_t b) { return (int64_t)(((uint64_t)a) - ((uint64_t)b)); }
int64_t taida_int_mul(int64_t a, int64_t b) { return (int64_t)(((uint64_t)a) * ((uint64_t)b)); }
int64_t taida_int_neg(int64_t a) { return (int64_t)(0ULL - ((uint64_t)a)); }

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
/* Note: WASM_TAG_* already defined above (B11-2 early definition) */

/* ── Forward declarations for Lax/Result/Gorillax (defined in W-5 section below) ── */
int64_t taida_lax_new(int64_t value, int64_t default_value);
int64_t taida_lax_empty(int64_t default_value);
int64_t taida_lax_empty_error(int64_t default_value, int64_t error);
int64_t taida_make_error_with_kind(int64_t type_ptr, int64_t msg_ptr, int64_t kind_ptr);
int64_t taida_make_error_with_kind_code(int64_t type_ptr, int64_t msg_ptr, int64_t kind_ptr, int64_t code);
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
static int _wasm_is_string_ptr(int64_t v);   /* F58: forward decl for the unmold string guard */
static int _wasm_is_async_obj(int64_t val);   /* PR-4: forward decl for async */
static int _looks_like_bytes(int64_t val);    /* F54B-016: forward decl for Bytes structural eq (defined in 02_containers) */
static int64_t _wasm_bytes_len(int64_t val);  /* F54B-016 */
static unsigned char _wasm_bytes_at(int64_t val, int64_t i); /* F54B-016 */
int64_t taida_async_unmold(int64_t async_ptr);              /* PR-4: forward decl */
int64_t taida_async_map(int64_t async_ptr, int64_t fn_ptr); /* PR-4: forward decl */
int64_t taida_async_get_or_default(int64_t async_ptr, int64_t fallback); /* PR-4 */
static int64_t _wasm_async_to_string(int64_t async_ptr);    /* PR-4: forward decl */

/* ── Hash constants needed by taida_generic_unmold (full definitions in W-5) ── */
#define WASM_HASH___TYPE      0x84d2d84b631f799bLL  /* FNV-1a("__type") */
#define WASM_HASH___VALUE     0x0a7fc9f13472bbe0LL  /* FNV-1a("__value") */
#define WASM_HASH___DEFAULT   0xed4fba440f8602d4LL  /* FNV-1a("__default") */
#define WASM_HASH_THROW       0x5a5fe3720c9584cfLL  /* FNV-1a("throw") */
#define WASM_HASH___PREDICATE 0x15592af3c2291540LL  /* FNV-1a("__predicate") */
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

// F58 P2-4: division/modulo with a compiler-proven non-zero divisor —
// the fused form of `Div[a, b]() >=> v` where `b` is a non-zero Int
// literal, so the empty-Lax path is unreachable and no Lax is built.
int64_t taida_div_exact(int64_t a, int64_t b) { return a / b; }
int64_t taida_mod_exact(int64_t a, int64_t b) { return a % b; }

/* F56: shared __type tags for sealed carriers. The producers
   (taida_moltenize_new / taida_secret_new in 02_containers.inc.c) and the
   detector below reference these SAME statics, so classification is a pure
   pointer-identity check. Crucially it never dereferences an arbitrary __type
   slot: a content compare on an unrelated pack's __type — e.g. the magic-tagged
   AsyncTask / Par packs surfaced by `out.toString()` — walked past the end of
   linear memory and trapped (OOB). Pointer identity is safe and exact: only a
   carrier built by the producers carries these addresses. */
const char __wasm_moltenized_str[] = "Moltenized";
const char __wasm_secret_str[] = "Secret";
static int _wasm_carrier_kind(int64_t val) {
    /* Read ONLY the header + field-0 slots directly. Pack layout is
       `[fc, hash0, tag0, val0, ...]`, so a carrier is `fc==2`, field-0 hash ==
       __TYPE, field-0 value == one of the shared __type statics (pointer
       identity — exact, and the producers store those exact addresses). We must
       NOT call taida_pack_has_hash / taida_pack_get here: those iterate the hash
       slots and walk past the end of linear memory (OOB trap) on the
       magic-tagged Async / Lax / Result packs that reach the equality / hashing
       helpers (e.g. `i == n` inside pi_approx). A bounded 4-slot read is safe on
       any value — a small int / magic pack / Lax (`fc != 2`) all fail fast. */
    if (!_wasm_is_valid_ptr(val, 4 * 8)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] != 2) return 0;                /* field_count */
    if (p[1] != WASM_HASH___TYPE) return 0; /* field-0 hash */
    int64_t type_ptr = p[3];                /* field-0 value */
    if (type_ptr == (int64_t)(intptr_t)__wasm_moltenized_str) return 1;
    if (type_ptr == (int64_t)(intptr_t)__wasm_secret_str) return 2;
    return 0;
}

/* ── generic_unmold — W-5g: Lax/Result/Gorillax-aware, predicate-evaluated ── */

int64_t taida_generic_unmold(int64_t val) {
    /* NTH-4: Guard against non-pointer values (negative integers, small ints, etc.)
       that would cause OOB when dereferenced as pack pointers.
       _wasm_is_valid_ptr already handles this for is_lax/result/gorillax, but
       taida_pack_has_hash (called later) does not. Guard early. */
    if (!_wasm_is_valid_ptr(val, 8)) return val;
    /* F58: a string is never a monadic carrier — return it as-is before
       the pack walks below. taida_pack_has_hash trusts slot 0 as a field
       count, so a string pointer reaching it reads its own bytes as a
       huge count and walks far out of bounds (latent for years because
       the misread count was layout-dependent; the hidden string headers
       shifted layouts and made it reproducible). Strings now carry a
       magic word, so the check is exact. */
    if (_wasm_is_string_ptr(val)) return val;
    /* PR-4: Async unmold — check before other types */
    if (_wasm_is_async_obj(val)) {
        return taida_async_unmold(val);
    }
    if (_wasm_is_lax(val)) {
        return taida_lax_unmold(val);
    }
    if (_wasm_is_result(val)) {
        /* Result unmold: evaluate predicate + check throw (matching native) */
        int64_t value = taida_pack_get(val, WASM_HASH___VALUE);
        int64_t pred = taida_pack_get(val, WASM_HASH___PREDICATE);
        int64_t throw_val = taida_pack_get(val, WASM_HASH_THROW);

        if (throw_val != 0) {
            if (pred != 0) {
                int64_t pred_result = _wasm_invoke_callback1(pred, value);
                if (!pred_result) {
                    /* Predicate failed — throw the error */
                    if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
                    int64_t error = taida_make_error(
                        WSTR("ResultError"),
                        WSTR("Result predicate failed"));
                    return taida_throw(error);
                }
                /* Predicate passed even with throw set — return value */
                return value;
            }
            /* No predicate, throw is set — throw */
            if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
            int64_t error = taida_make_error(
                WSTR("ResultError"),
                WSTR("Result error"));
            return taida_throw(error);
        }

        /* Evaluate predicate if present (no throw set) */
        if (pred != 0) {
            int64_t pred_result = _wasm_invoke_callback1(pred, value);
            if (pred_result) return value;  /* success */
            /* Predicate failed — throw default error */
            int64_t error = taida_make_error(
                WSTR("ResultError"),
                WSTR("Result predicate failed"));
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
            WSTR("GorillaxError"),
            WSTR("Gorillax error"));
        return taida_throw(err);
    }
    /* F58: the field-pack walks below trust slot 0 as a field count.
       A non-pack heap object reaching this point (a List from
       `Split[...]() >=> parts`, Bytes, a hash map...) would read its own
       first word as a huge count and walk far out of bounds — latent for
       years because the misread count was layout-dependent. Validate the
       pack shape positively instead: an in-bounds slot range and the
       WASM_PACK_MAGIC tail sentinel that taida_pack_new stamps on every
       non-empty pack. No arbitrary field-count cap — a user-defined mold
       may legitimately carry hundreds of fields, and capping silently
       skipped their unmold. Empty packs (fc == 0) have nothing to walk
       and return as-is, exactly like every other non-monadic value. */
    {
        int64_t *p_chk = (int64_t *)(intptr_t)val;
        int64_t fc_chk = p_chk[0];
        if (fc_chk <= 0) return val;
        /* Reject counts whose slot range cannot possibly fit in linear
           memory before computing byte sizes (overflow guard). */
        if (fc_chk > 0x0FFFFFFFLL) return val;
        unsigned int mem_chk = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
        uint64_t need_chk = (uint64_t)(2 + fc_chk * 3) * 8ULL;
        if ((uint64_t)(unsigned int)val + need_chk > (uint64_t)mem_chk) return val;
        if (p_chk[1 + fc_chk * 3] != WASM_PACK_MAGIC) return val;
    }
    /* BE-WASM-1: TODO mold unmold — return unm channel, fallback to sol/__default/__value.
       Matches native_runtime.c taida_generic_unmold TODO branch. */
    if (taida_pack_has_hash(val, WASM_HASH___TYPE)) {
        int64_t type_ptr = taida_pack_get(val, WASM_HASH___TYPE);
        /* F56: a sealed carrier (Moltenized/Secret) cannot be unmolded directly —
           the __value fallback below would otherwise leak the sealed value. Use
           pointer identity against the shared __type statics (the producers store
           those exact addresses), placed before the low-address guard which would
           otherwise return the carrier unchanged. Fail closed (parity with the
           checker [E1535] and the interpreter/JS/native runtimes). val is a
           confirmed field-pack here — the async/lax/result/gorillax branches
           returned earlier — so this never reaches a magic-tagged pack. */
        if (type_ptr == (int64_t)(intptr_t)__wasm_moltenized_str
            || type_ptr == (int64_t)(intptr_t)__wasm_secret_str) {
            int64_t error = taida_make_error(
                WSTR("TypeError"),
                WSTR("Cannot unmold a sealed carrier (Moltenized/Secret) directly. Use a secret-aware consumer."));
            return taida_throw(error);
        }
        /* The __type slot must reference a string; the magic word
           identifies one exactly. The previous guard rejected every
           address <= 4096, but emitted literals live in the data
           segment from global-base 1024 — so EVERY user-defined mold's
           __type was rejected here and its unmold silently returned the
           carrier pack instead of the value (wasm-only; interp / JS /
           native all unmolded correctly). */
        if (!_wasm_is_string_ptr(type_ptr)) return val;
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
                WSTR("TypeError"),
                WSTR("Cannot unmold Molten directly. Molten can only be used inside Cage."));
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
    /* F58: positive identification via the hidden one-word header. Every
       Taida Str value points just past its magic word (heap strings via
       _wasm_str_alloc, static literals via the emitter's header-carrying
       structs), so anything without the magic — in particular an Int
       whose value happens to coincide with a live address — is not a
       string. Replaces the printable-first-byte heuristic that misread
       monotonically growing loop accumulators as strings. */
    if (v < 1024 + 8 || v > 0xFFFFFFFFLL) return 0;
    if ((v & 0x7) != 0) return 0;
    unsigned int addr = (unsigned int)(uint64_t)v;
    /* The header word (and at least the first data byte) must lie inside
       linear memory. */
    unsigned int mem_bytes = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    if (addr >= mem_bytes) return 0;
    int64_t magic = *(const int64_t *)(intptr_t)(v - 8);
    return magic == WASM_STR_MAGIC || magic == WASM_STR_STATIC_MAGIC;
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

/* F56: a sealed carrier (Moltenized/Secret) is never equal — not even to itself
   — so `==`/`!=` agree with the interpreter and never form an equality oracle.
   (`==` is `[E1536]` in checked code; this covers `--no-check` + the tagged
   wrappers `taida_poly_eq_tagged`/`taida_poly_neq_tagged` which route here.) */
int64_t taida_poly_eq(int64_t a, int64_t b) {
    if (_wasm_carrier_kind(a) || _wasm_carrier_kind(b)) return 0;
    return a == b ? 1 : 0;
}
int64_t taida_poly_neq(int64_t a, int64_t b) {
    if (_wasm_carrier_kind(a) || _wasm_carrier_kind(b)) return 1;
    return a != b ? 1 : 0;
}

/* ── W-3: String operations (dynamic allocation via bump allocator) ── */

int64_t taida_str_concat(int64_t a_ptr, int64_t b_ptr) {
    const char *a = (const char *)(intptr_t)a_ptr;
    const char *b = (const char *)(intptr_t)b_ptr;
    if (!a) a = "";
    if (!b) b = "";
    int32_t la = wasm_strlen(a);
    int32_t lb = wasm_strlen(b);
    char *buf = _wasm_str_alloc(la + lb + 1);
    if (!buf) return 0;
    for (int32_t i = 0; i < la; i++) buf[i] = a[i];
    for (int32_t i = 0; i < lb; i++) buf[la + i] = b[i];
    buf[la + lb] = '\0';
    return (int64_t)(intptr_t)buf;
}

/* ── Unicode code-point string indexing (mirror of native) ──
   Every index-taking Str API counts Unicode code points (the
   reference iterates chars); these helpers walk UTF-8 lead bytes. */
static int64_t _wasm_utf8_count(const char *s, int byte_len) {
    int64_t n = 0;
    for (int i = 0; i < byte_len; i++)
        if (((unsigned char)s[i] & 0xC0) != 0x80) n++;
    return n;
}

/* Byte offset where code point `cp` starts; byte_len when past the end. */
static int _wasm_utf8_cp_to_byte(const char *s, int byte_len, int64_t cp) {
    if (cp <= 0) return 0;
    int64_t seen = 0;
    for (int i = 0; i < byte_len; i++) {
        if (((unsigned char)s[i] & 0xC0) != 0x80) {
            if (seen == cp) return i;
            seen++;
        }
    }
    return byte_len;
}

/* Byte length of the code point starting at s[i]. */
static int _wasm_utf8_cp_len_at(const char *s, int byte_len, int i) {
    int j = i + 1;
    while (j < byte_len && (((unsigned char)s[j]) & 0xC0) == 0x80) j++;
    return j - i;
}

int64_t taida_str_length(int64_t s_ptr) {
    const char *s = (const char *)(intptr_t)s_ptr;
    if (!s) return 0;
    /* Code points, not bytes — mirrors native taida_str_length. */
    return _wasm_utf8_count(s, wasm_strlen(s));
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
    char *buf = _wasm_str_alloc(len + 1);
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
    return v ? WSTR("true") : WSTR("false");
}

/* ── W-3: Int methods ── */

int64_t taida_int_abs(int64_t a) {
    if (a == TAIDA_WASM_I64_MIN) return TAIDA_WASM_I64_MAX;
    return a < 0 ? -a : a;
}
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

    /* Large integer-valued f64: every finite f64 at or above 2^53 is an
       integer, and past ~1e18/1e19 the i64/u64 casts below are UB — the
       old path printed 1e20 as "0". Expand exactly: decompose into
       mantissa * 2^exp and double out in decimal (cold path; <= ~310
       digits for the largest finite f64). */
    if (d >= 1e18) {
        union { double d; uint64_t u; } um;
        um.d = d;
        uint64_t mant = (um.u & 0xFFFFFFFFFFFFFULL) | (1ULL << 52);
        int exp2 = (int)((um.u >> 52) & 0x7FF) - 1023 - 52; /* >= 0 here */
        unsigned char dig[340];
        int n = 0;
        while (mant > 0) { dig[n++] = (unsigned char)(mant % 10); mant /= 10; }
        for (int k = 0; k < exp2; k++) {
            int carry = 0;
            for (int i = 0; i < n; i++) {
                int v = dig[i] * 2 + carry;
                dig[i] = (unsigned char)(v % 10);
                carry = v / 10;
            }
            while (carry) { dig[n++] = (unsigned char)(carry % 10); carry /= 10; }
        }
        for (int i = n - 1; i >= 0 && len < bufsize; i--) buf[len++] = (char)('0' + dig[i]);
        if (len + 2 <= bufsize) { buf[len++] = '.'; buf[len++] = '0'; }
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

    char *buf = _wasm_str_alloc(len + 1);
    if (!buf) return 0;
    for (int i = 0; i < len; i++) buf[i] = tmp[i];
    buf[len] = '\0';
    return (int64_t)(intptr_t)buf;
}

/* W-4: Forward declarations for list functions (used by polymorphic helpers) */
int64_t taida_list_length(int64_t list_ptr);

/* C23B-005 reopen / C23B-006 (2026-04-22): Wide (8-byte) printable-ASCII
   magic sentinels for positive identification of every WASM collection.
   The pre-C23B-005 detectors relied on structural heuristics (cap/len
   ranges for List, 4-byte `"HMAP"` / `"SET\0"` markers at `data[3]` for
   HashMap / Set) that false-positived on untagged Int64 values living
   inside a HashMap value slot or a Pack field — `memory[addr + 24]`
   could coincidentally equal the 32-bit marker, causing integers like
   `73088` to be re-rendered as empty HashMaps or to send the renderer
   into a stack-overflow list recursion.
   The wide sentinels below are 64 bits of `TAIDA`-prefixed printable
   ASCII — they cannot arise from arithmetic on user-supplied integers
   and they cannot collide with FNV-1a hashes stored in Pack field
   slots (which always have `data[0] >= 1` in non-empty packs, gating
   pack dispatch before these sentinels are ever checked). All four
   detectors below now require (a) a valid linear-memory address, (b)
   8-byte alignment (every heap allocation goes through `wasm_alloc`
   which rounds up to 8-byte boundaries), and (c) the sentinel at the
   designated slot. This supersedes every cap/len / fc-range / first-
   hash-non-zero heuristic, closing C23B-005 (list / set / pack
   collection false-positives on large Int members) and C23B-006
   (HashMap false-positive on untagged Int members). Symmetric with
   the native runtime's `TAIDA_PACK_MAGIC` / `TAIDA_LIST_MAGIC`
   positive identification.
   Allocation shapes are:
     - List / Set: `[cap, len, elem_tag, MAGIC, elems...]`
     - HashMap:    `[cap, len, value_tag, MAGIC, entries...]`
     - Non-empty Pack: `[fc, hash0, tag0, val0, hash1, ..., MAGIC]`
       (one extra slot appended after the last field; existing readers
       that only touch indices `0..1+fc*3)` are unaffected)
     - Empty Pack:  `[0, EMPTY_PACK_MAGIC]` (already tag-based via
       reopen 4) */
#define WASM_LIST_MAGIC 0x544149444C535400LL  /* "TAIDLST\0" */
#define WASM_SET_MAGIC  0x5441494453455400LL  /* "TAIDSET\0" */
#define WASM_HM_MAGIC   0x54414944484D5000LL  /* "TAIDHMP\0" */
/* D29B-016 / Phase 10-E (Track-θ, 2026-04-27): Rope-backed string sentinel.
   Reserved for a future rope-aware `_wf_str_concat` polymorphic dispatch.
   The interpreter side (Lock-K verdict V-1) implements transparent rope
   promotion via an internal `StrRepr::Rope` variant on `StrValue`
   (DEVIATION matching Track-ε commit `e179238` to avoid 146-site Value::Str
   match arm churn). The wasm-wasi backend currently relies on the existing
   bump-allocator + `_wf_str_*` heap-copy path which already meets the
   Lock-K acceptance envelope at the surface level (4-backend parity is
   verified by `tests/d29b_016_str_rope_parity.rs`). This sentinel is
   pinned now so a future widening addition (§ 6.2) can detect rope-backed
   inputs without an ABI break. */
#define WASM_STR_ROPE_MAGIC 0x5441494452505400LL  /* "TAIDRPT\0" */
/* Back-compat aliases so the remaining call sites in the file keep
   compiling until every reference is renamed. Each alias points to the
   widened sentinel so the allocation stamp and the detector check
   remain consistent. */
#define WASM_SET_MARKER_VAL  WASM_SET_MAGIC
#define WASM_HM_MARKER_VAL   WASM_HM_MAGIC
/* W-4f2: Collection type markers and layout constants */
#define WASM_HM_HEADER 4

/* W-4f2: List/Set element offset — header is [cap, len, elem_tag, type_marker] = 4 slots */
#define WASM_LIST_ELEMS 4

/* C23B-008 (2026-04-22): HashMap insertion-order side-index offsets.
   After the `cap * 3` entry slots and the trailing magic we append:
     [next_ord, order_array[cap]]
   so display / iteration helpers can walk entries in their insertion
   order (matches the interpreter's `Vec<(k,v)>` semantics; JS already
   preserves insertion order via its `__entries` Array). See the
   `_wasm_hashmap_new_with_cap` comment for the allocation layout. */
#define WASM_HM_ORD_HEADER_SLOT(cap) ((WASM_HM_HEADER) + (cap) * 3 + 1)
#define WASM_HM_ORD_SLOT(cap, i)     (WASM_HM_ORD_HEADER_SLOT(cap) + 1 + (i))

/* ── W-3f/W-4: taida_polymorphic_length (wasm-min: string + list) ── */

/* C23B-005 (2026-04-22) forward decl. `_wasm_is_valid_ptr` sits later in
   this fragment but is needed here for the tag-based collection
   detectors. `bump_ptr` / `__heap_base` were already declared near the
   allocator. */
static int _wasm_is_valid_ptr(int64_t val, unsigned int min_bytes);

/* Helper: verify that a list/set allocation has the shape-dependent
   trailing magic at `data[WASM_LIST_ELEMS + cap]`, in addition to the
   head magic at `data[3]`. This is the C23B-005 reopen fix — single-
   magic identification was still foolable because a user-supplied
   large Int stored as a collection member could coincidentally point
   at another list's base and match the head magic legitimately. The
   trailing magic lives at an offset dependent on `data[0]` (cap),
   so the attacker would need to also satisfy `memory[addr + (4+cap)*8] == MAGIC`
   — vanishingly unlikely to collide. */
static int _check_list_trailing_magic(int64_t *data, int64_t expected_magic, unsigned int base_addr) {
    int64_t cap = data[0];
    if (cap < 8 || cap > 0x1000000) return 0;  /* request Bytes may carry up to the 16MiB ABI cap */
    unsigned int tail_bytes = (unsigned int)(WASM_LIST_ELEMS + cap + 1) * 8u;
    unsigned int mem_size = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    if ((uint64_t)base_addr + (uint64_t)tail_bytes > (uint64_t)mem_size) return 0;
    return data[WASM_LIST_ELEMS + cap] == expected_magic;
}

/* C23B-005 (2026-04-22): positive identification for bump-allocated
   lists. Checks BOTH head and trailing magics (see
   `_check_list_trailing_magic` above). */
static int _looks_like_list(int64_t ptr) {
    if (!_wasm_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    if (data[3] == WASM_LIST_MAGIC) return _check_list_trailing_magic(data, WASM_LIST_MAGIC, addr);
    if (data[3] == WASM_SET_MAGIC) return _check_list_trailing_magic(data, WASM_SET_MAGIC, addr);
    return 0;
}

/* C23B-005 (2026-04-22): positive identification for bump-allocated
   sets. Requires the distinct set sentinel at both head and tail. */
static int _is_wasm_set(int64_t ptr) {
    if (!_wasm_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    if (data[3] != WASM_SET_MAGIC) return 0;
    return _check_list_trailing_magic(data, WASM_SET_MAGIC, addr);
}

int64_t taida_polymorphic_length(int64_t ptr) {
    if (!ptr) return 0;
    /* W-4: Check if it's a list first */
    if (_looks_like_list(ptr)) {
        return taida_list_length(ptr);
    }
    /* Otherwise treat as string: code points, not bytes (mirrors
       native taida_polymorphic_length). */
    const char *s = (const char *)(intptr_t)ptr;
    return _wasm_utf8_count(s, wasm_strlen(s));
}

/* ── W-3f/W-4f2: taida_polymorphic_to_string (full collection support) ── */

/* Helper: check if a value looks like a valid string pointer in WASM linear memory.
   In wasm32, valid heap/data addresses are positive and within linear memory.
   We check that the pointer is in a reasonable range and points to a NUL-terminated
   byte sequence with printable or whitespace characters. */
static int _looks_like_string(int64_t val) {
    /* F58: header-carrying strings (every string built by the runtime
       and every emitted literal) are identified exactly by their magic
       word — including the empty string, which the byte heuristic below
       can never accept (its first byte IS the terminator; a Split
       producing a leading empty fragment used to render that fragment
       as its pointer value). The heuristic survives only as a fallback
       for the runtime's own bare C literals. */
    return _wasm_is_string_ptr(val);
}

/* C23B-005 (2026-04-22): positive identification for non-empty
   BuchiPacks. Layout is `[fc, hash0, tag0, val0, ..., WASM_PACK_MAGIC]`
   — the sentinel is appended after the last field by `taida_pack_new`
   (when fc >= 1). Existing pack readers only touch indices
   `0..1+fc*3)`, so the extra slot is backward-compatible. The empty
   pack case (`fc == 0`) is matched separately by
   `_looks_like_empty_pack` via `WASM_EMPTY_PACK_MAGIC` at pack[1].
   The old detector used `fc in 1..100 && data[1] != 0` which
   false-positived on any untagged Int that pointed at memory whose
   first int64_t happened to land in that range — the exact bug class
   C23B-005 tracks. */
static int _looks_like_pack(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 16)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)val;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)val;
    int64_t fc = data[0];
    /* fc must be strictly positive — empty pack is matched separately.
       No upper cap: the tail sentinel below is the positive proof, and
       a cap would silently reject legitimate wide packs (the in-bounds
       check right after keeps the sentinel read safe for any fc). */
    if (fc < 1 || fc > 0x0FFFFFFF) return 0;
    unsigned int mem_size = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    /* Need slots [0..1+fc*3] inclusive, i.e. (2 + fc*3) int64_t = 16 + fc*24 bytes. */
    uint64_t need_bytes = (uint64_t)(2 + fc * 3) * 8ULL;
    if ((uint64_t)addr + need_bytes > (uint64_t)mem_size) return 0;
    return data[1 + fc * 3] == WASM_PACK_MAGIC;
}

/* C23B-003 reopen 3: detect an empty BuchiPack (`@()` literal / Unit).
   `taida_pack_new(0)` allocates a single int64_t slot initialised to `0`
   (the `field_count` header). Unlike the native runtime — which stamps a
   distinctive `TAIDA_PACK_MAGIC` header — the wasm runtime carries no
   magic byte on pack allocations, so we match via address-range
   heuristics rather than a memory marker:
     1. `val` must be a bump-allocator address: `addr >= __heap_base` AND
        `addr < bump_ptr`. This is the SAME invariant `_wasm_is_string_ptr`
        uses to separate heap-allocated pointers from arbitrary integers,
        and it is essential here: without it, any small integer printed
        through the polymorphic display dispatch (e.g. `5050` from
        `examples/12_tail_recursion.td`, or `8080` in `:8080` string
        interpolation) would happen to index into the static data section,
        read 8 zero bytes at that offset, and be falsely re-rendered as
        `@()`. The pure memory-peek detector we started with failed this
        way on the `wasm_full` regression suite.
     2. `addr + 8 <= mem_size` so the int64_t read is in bounds.
     3. `*(int64_t*)val == 0` — the `field_count` slot of an empty pack
        is zero (`taida_pack_new(0)` zero-initialises the allocation).
     4. Not a List / HashMap / Set header. In practice `_looks_like_list`
        already rejects zero caps (`cap >= 8`), and HashMap / Set need
        richer header shapes, but running the guards here keeps the
        invariant explicit and makes the detector commute freely with the
        other dispatchers.
   Callers must place this check AFTER the non-empty pack / list / hashmap
   / set / async / error detectors so richer signatures still win on
   overlapping bit patterns, and BEFORE `_looks_like_string` / the integer
   fallback so nested `@()` rendering beats raw-pointer stringification. */
static int _looks_like_empty_pack(int64_t val) {
    /* C23B-003 reopen 4: positive identification via a magic sentinel
       stored in `pack[1]` by `taida_pack_new(0)`. Requires:
         1. A valid linear-memory address (wasm32 ⇒ 32-bit),
         2. Room for at least two int64_t slots (16 bytes),
         3. `data[0] == 0` — the `field_count` slot,
         4. `data[1] == WASM_EMPTY_PACK_MAGIC` — the sentinel.
       This supersedes the previous heap-range + zero-slot heuristic
       that false-positive'd on dynamic Int expressions whose bit
       patterns landed inside the bump arena. The magic (`0x5441…4B55`,
       = "TAIDPAKU") has 7 of 8 bytes in the printable ASCII range —
       user code cannot produce it via arithmetic on small integers,
       and FNV-1a hashes stored in `pack[1]` of non-empty packs are
       gated by `data[0] >= 1`, which we reject at step 3. */
    if (val <= 0 || val > 0xFFFFFFFFLL) return 0;
    unsigned int addr = (unsigned int)(uint64_t)val;
    unsigned int mem_size = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    if (addr + 16 > mem_size) return 0;
    if ((addr & 7u) != 0) return 0;  /* pack is 8-byte aligned */
    int64_t *data = (int64_t *)(intptr_t)val;
    if (data[0] != 0) return 0;
    if (data[1] != WASM_EMPTY_PACK_MAGIC) return 0;
    return 1;
}

/* Forward declarations for toString helpers */
static int64_t _wasm_value_to_display_string(int64_t val);
static int64_t _wasm_value_to_debug_string(int64_t val);
/* C23B-003 reopen 2: recursive debug-string variant that keeps nested
   typed runtime objects (HashMap / Set / BuchiPack) in their synthetic
   full-form shape. Symmetric with native's
   `taida_value_to_debug_string_full`. Used only inside the full-form
   helpers below so nested HashMap/Set/Pack render as
   `@(__entries <= …, __type <= "HashMap")` /
   `@(__items <= …, __type <= "Set")` / `@(…, __type <= …)` instead of
   collapsing to the short-form `.toString()` output. The helper also
   calls `_wasm_is_error` / `_wasm_error_to_string`, which are defined
   later in this fragment — forward-declare them here. */
static int64_t _wasm_value_to_debug_string_full(int64_t val);
/* C23B-005 (2026-04-22): tag-aware debug-string dispatcher. When the
   caller knows the element's primitive tag (e.g. a list's stored
   `elem_type_tag`, or a pack field's `field_tag`), this helper bypasses
   the structural detectors and renders via the primitive-specific
   formatter. That closes the last residual false-positive path in the
   tag-based collection identification: even if a hashmap / list has
   been positively identified at the top level, its element slots may
   carry untagged Int64 values whose bit pattern happens to point at
   another heap allocation whose `data[3]` coincidentally matches a
   collection magic. Dispatching by the KNOWN element tag eliminates
   that probabilistic path entirely. `tag == -1` (UNKNOWN) falls back
   to the existing structural detector. */
static int64_t _wasm_render_elem_tagged_debug(int64_t val, int64_t tag);
static int64_t _wasm_render_elem_tagged_debug_full(int64_t val, int64_t tag);
static int64_t _wasm_pack_to_string_full(int64_t pack_ptr);
static int _wasm_is_error(int64_t val);
static int64_t _wasm_error_to_string(int64_t val);
static int _is_wasm_hashmap(int64_t ptr);
static const char *_wasm_lookup_field_name(int64_t hash);
static int64_t _wasm_lookup_field_type(int64_t hash);
/* W-5f: Monadic type hash constants (FNV-1a hashes of field names).
   Centralized here for use by both display_string and the runtime constructors below. */
/* WFX-2: corrected FNV-1a hashes (previous values were wrong, causing
   field access mismatch with compiler-generated hashes from simple_hash()) */
#define WASM_HASH_HAS_VALUE   0x175d21da0757452dLL  /* FNV-1a("has_value") */
#define WASM_HASH_IS_OK       0x6550c1c5b98b56bfLL  /* FNV-1a("isOk") */
#define WASM_HASH___ERROR     0x15c3e6e41a99a6cbLL  /* FNV-1a("__error") */
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
    sb->buf = _wasm_str_alloc(sb->cap);
    sb->len = 0;
    if (sb->buf) sb->buf[0] = '\0';
}

static void _sb_ensure(_wasm_strbuf *sb, int needed) {
    if (sb->len + needed + 1 > sb->cap) {
        int new_cap = sb->cap;
        while (sb->len + needed + 1 > new_cap) new_cap *= 2;
        char *new_buf = _wasm_str_alloc(new_cap);
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
    /* C23B-005 / C23B-006 / C23B-007: `hm[2]` is reliable now — see the
       `_wasm_hashmap_to_string` comment and the hardened
       `taida_hashmap_set_value_tag` (HETEROGENEOUS latch). */
    int64_t value_tag = hm[2];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "HashMap({");
    /* C23B-008 (2026-04-22): iterate the insertion-order side-index so
       `.toString()` emits the same sequence as interpreter / JS. */
    int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
    int64_t count = 0;
    for (int64_t oi = 0; oi < next_ord; oi++) {
        int64_t slot = hm[WASM_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (WASM_HM_SLOT_OCCUPIED(sh, sk)) {
            int64_t value = hm[WASM_HM_HEADER + slot * 3 + 2];
            if (count > 0) _sb_append(&sb, ", ");
            int64_t key_str = _wasm_value_to_debug_string(sk);
            int64_t val_str = _wasm_render_elem_tagged_debug(value, value_tag);
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
    /* C23B-005: honour the set's elem_type_tag. */
    int64_t elem_tag = list[2];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "Set({");
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) _sb_append(&sb, ", ");
        int64_t elem = list[WASM_LIST_ELEMS + i];
        int64_t elem_str = _wasm_render_elem_tagged_debug(elem, elem_tag);
        _sb_append(&sb, (const char *)(intptr_t)elem_str);
    }
    _sb_append(&sb, "})");
    return _sb_finish(&sb);
}

/* W-4f2: List toString: @[elem1, elem2, ...] */
static int64_t _wasm_list_to_string(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    /* C23B-005: consult the list's stored elem_type_tag so primitive
       elements dispatch through the tag-aware renderer instead of the
       structural detectors. Closes stack-overflow / short-form drift
       on `@[<large Int>]` etc. where a bare Int element aliased a
       heap allocation whose layout happened to match a collection
       sentinel. */
    int64_t elem_tag = list[2];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "@[");
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) _sb_append(&sb, ", ");
        int64_t item = list[WASM_LIST_ELEMS + i];
        int64_t item_str = _wasm_render_elem_tagged_debug(item, elem_tag);
        _sb_append(&sb, (const char *)(intptr_t)item_str);
    }
    _sb_append(&sb, "]");
    return _sb_finish(&sb);
}

/* W-4f2: Pack toString: @(field <= value, ...) */
static int64_t _wasm_pack_to_string(int64_t pack_ptr) {
    /* F56: a sealed carrier (Moltenized/Secret) renders only its policy label;
       every pack display path converges here so the sealed __value can never
       reach stdout / Str[] / debug / nesting. */
    {
        int ck = _wasm_carrier_kind(pack_ptr);
        if (ck == 1) return WSTR("<Moltenized>");
        if (ck == 2) return WSTR("<Secret>");
    }
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    int64_t fc = pack[0];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "@(");
    int count = 0;
    for (int64_t i = 0; i < fc; i++) {
        int64_t field_hash = pack[1 + i * 3];
        int64_t field_tag  = pack[1 + i * 3 + 1];
        int64_t field_val  = pack[1 + i * 3 + 2];
        const char *fname = _wasm_lookup_field_name(field_hash);
        if (!fname) continue;
        /* Skip internal __ fields for display (same as native) */
        if (fname[0] == '_' && fname[1] == '_') continue;
        if (count > 0) _sb_append(&sb, ", ");
        _sb_append(&sb, fname);
        _sb_append(&sb, " <= ");
        /* C21B-seed-07: the per-field tag is authoritative for Float /
           Bool rendering (matches the native `taida_pack_to_display_string`
           rewrite). The legacy registry lookup (ftype == 4 => Bool) is kept
           as a fallback for pre-seed-07 packs that only populated the
           global field-name/type table. */
        int64_t ftype = _wasm_lookup_field_type(field_hash);
        int render_bool  = (field_tag == WASM_TAG_BOOL) || (field_tag == 0 && ftype == 4);
        int render_float = (field_tag == WASM_TAG_FLOAT);
        int render_str   = (field_tag == WASM_TAG_STR);
        int render_unit  = field_val == 0
            && (field_hash == WASM_HASH___PREDICATE
                || field_hash == WASM_HASH_THROW
                || _wf_strcmp(fname, "__predicate") == 0
                || _wf_strcmp(fname, "throw") == 0);
        /* C23B-005 (2026-04-22): explicit Int branch. Packs default
           `field_tag = 0` (INT), so without a dedicated branch an
           untagged large Int value would fall through to the generic
           structural dispatcher and false-match a collection detector
           whose allocation layout happened to alias the Int's bit
           pattern. We take `field_tag == WASM_TAG_INT` AND `ftype`
           says it's an Int (ftype 1/3 for Int / literal-Int in the
           legacy registry) to avoid stealing from the legacy Bool
           fallback (`ftype == 4`). */
        int render_int = (field_tag == WASM_TAG_INT) && !render_bool;
        if (render_bool) {
            _sb_append(&sb, field_val ? "true" : "false");
        } else if (render_float) {
            int64_t fs = taida_float_to_str(field_val);
            _sb_append(&sb, (const char *)(intptr_t)fs);
        } else if (render_str) {
            /* C23-4: WASM_TAG_STR is authoritative for Str primitive fields
               (set by `taida_{int,str,float,bool}_mold_*` via `_lax_tag_vd`).
               The generic `_wasm_value_to_debug_string` false-positives on
               low-addressed int values (e.g. empty string at a low static
               address is misread as 0-length pack), rendering `__default`
               as a raw pointer integer. Rendering the char* directly with
               explicit double-quotes matches the interpreter's BuchiPack
               full-form contract (`val.to_debug_string()` for Str wraps in
               `"..."`). */
            const char *sv = (const char *)(intptr_t)field_val;
            _sb_append(&sb, "\"");
            if (sv) _sb_append(&sb, sv);
            _sb_append(&sb, "\"");
        } else if (render_unit) {
            _sb_append(&sb, "@()");
        } else if (render_int) {
            int64_t is = taida_int_to_str(field_val);
            _sb_append(&sb, (const char *)(intptr_t)is);
        } else {
            int64_t val_str = _wasm_value_to_debug_string(field_val);
            _sb_append(&sb, (const char *)(intptr_t)val_str);
        }
        count++;
    }
    _sb_append(&sb, ")");
    return _sb_finish(&sb);
}

/* C21B-seed-07: Pack-to-display-string in "full" form — matches the
   interpreter's `to_display_string()` for BuchiPack which shows ALL
   fields, including the `__`-prefixed internals used by Lax / Result /
   Gorillax / primitive-mold return values. Symmetric with native's
   `taida_pack_to_display_string_full`. The per-field tag dispatches
   Float / Bool rendering the same way the non-full variant does. */
static int64_t _wasm_pack_to_string_full(int64_t pack_ptr) {
    /* F56: sealed carrier — policy label only (the full-form path keeps __
       fields, so without this guard stdout(secret) would expose __value). */
    {
        int ck = _wasm_carrier_kind(pack_ptr);
        if (ck == 1) return WSTR("<Moltenized>");
        if (ck == 2) return WSTR("<Secret>");
    }
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    int64_t fc = pack[0];
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "@(");
    int count = 0;
    for (int64_t i = 0; i < fc; i++) {
        int64_t field_hash = pack[1 + i * 3];
        int64_t field_tag  = pack[1 + i * 3 + 1];
        int64_t field_val  = pack[1 + i * 3 + 2];
        const char *fname = _wasm_lookup_field_name(field_hash);
        if (!fname) continue;
        /* Unlike `_wasm_pack_to_string`, we do NOT skip __ fields here. */
        if (count > 0) _sb_append(&sb, ", ");
        _sb_append(&sb, fname);
        _sb_append(&sb, " <= ");
        int64_t ftype = _wasm_lookup_field_type(field_hash);
        int render_bool  = (field_tag == WASM_TAG_BOOL) || (field_tag == 0 && ftype == 4);
        int render_float = (field_tag == WASM_TAG_FLOAT);
        int render_str   = (field_tag == WASM_TAG_STR);
        int render_unit  = field_val == 0
            && (field_hash == WASM_HASH___PREDICATE
                || field_hash == WASM_HASH_THROW
                || _wf_strcmp(fname, "__predicate") == 0
                || _wf_strcmp(fname, "throw") == 0);
        /* C23B-005 (2026-04-22): explicit Int branch — see the
           `_wasm_pack_to_string` counterpart comment. */
        int render_int = (field_tag == WASM_TAG_INT) && !render_bool;
        if (render_bool) {
            _sb_append(&sb, field_val ? "true" : "false");
        } else if (render_float) {
            int64_t fs = taida_float_to_str(field_val);
            _sb_append(&sb, (const char *)(intptr_t)fs);
        } else if (render_str) {
            /* C23-4: symmetric with `_wasm_pack_to_string` — render
               WASM_TAG_STR fields as explicitly-quoted char* so
               `__value <= ""` / `__default <= ""` come through as the
               interpreter's `Value::to_debug_string()` for `Value::Str`
               (which wraps strings in double quotes), rather than
               degrading to pointer-integer fallback inside
               `_wasm_value_to_debug_string`. */
            const char *sv = (const char *)(intptr_t)field_val;
            _sb_append(&sb, "\"");
            if (sv) _sb_append(&sb, sv);
            _sb_append(&sb, "\"");
        } else if (render_unit) {
            _sb_append(&sb, "@()");
        } else if (render_int) {
            int64_t is = taida_int_to_str(field_val);
            _sb_append(&sb, (const char *)(intptr_t)is);
        } else {
            /* C23B-003 reopen 2: nested typed runtime objects must
               keep their full-form rendering; `_wasm_value_to_debug_string`
               would otherwise dispatch HashMap / Set to their short-form
               `.toString()` helpers (`HashMap({...})` / `Set({...})`),
               breaking 4-backend parity with the interpreter. Use the
               full-form variant so nested Pack / HashMap / Set continue
               to expose `__`-prefixed internals recursively. */
            int64_t val_str = _wasm_value_to_debug_string_full(field_val);
            _sb_append(&sb, (const char *)(intptr_t)val_str);
        }
        count++;
    }
    _sb_append(&sb, ")");
    return _sb_finish(&sb);
}

/* C23B-005 (2026-04-22): tag-aware element renderer. When the caller
   knows the primitive tag of `val` (e.g. from `list[2]` elem_type_tag,
   or a pack field's stored `field_tag`), dispatch directly to the
   primitive-specific formatter and skip every structural detector.
   That makes Int / Float / Bool / Str elements unconditionally
   render as their primitive string form, even if the underlying
   int64_t bit pattern happens to alias a heap address whose memory
   at `data[3]` coincidentally matches a collection sentinel. The
   helpers fall back to the structural detectors (`_wasm_value_to_debug_string`
   / `_wasm_value_to_debug_string_full`) when `tag == -1` (UNKNOWN),
   so lists / packs that did not propagate an element tag still get
   rendered via the usual path. This is the defence-in-depth partner
   for the `WASM_LIST_MAGIC` / `WASM_HM_MAGIC` / `WASM_SET_MAGIC` /
   `WASM_PACK_MAGIC` positive identification at the detector level. */
static int64_t _wasm_render_elem_tagged_debug(int64_t val, int64_t tag) {
    if (tag == WASM_TAG_INT) return taida_int_to_str(val);
    if (tag == WASM_TAG_FLOAT) return taida_float_to_str(val);
    if (tag == WASM_TAG_BOOL) return val ? WSTR("true") : WSTR("false");
    if (tag == WASM_TAG_STR) {
        const char *s = (const char *)(intptr_t)val;
        int slen = s ? wasm_strlen(s) : 0;
        char *buf = _wasm_str_alloc(slen + 3);
        if (!buf) return val;
        buf[0] = '"';
        for (int i = 0; i < slen; i++) buf[1 + i] = s[i];
        buf[slen + 1] = '"';
        buf[slen + 2] = '\0';
        return (int64_t)(intptr_t)buf;
    }
    /* Unknown / Pack tag — fall back to the structural dispatcher. */
    return _wasm_value_to_debug_string(val);
}

static int64_t _wasm_render_elem_tagged_debug_full(int64_t val, int64_t tag) {
    if (tag == WASM_TAG_INT) return taida_int_to_str(val);
    if (tag == WASM_TAG_FLOAT) return taida_float_to_str(val);
    if (tag == WASM_TAG_BOOL) return val ? WSTR("true") : WSTR("false");
    if (tag == WASM_TAG_STR) {
        const char *s = (const char *)(intptr_t)val;
        int slen = s ? wasm_strlen(s) : 0;
        char *buf = _wasm_str_alloc(slen + 3);
        if (!buf) return val;
        buf[0] = '"';
        for (int i = 0; i < slen; i++) buf[1 + i] = s[i];
        buf[slen + 1] = '"';
        buf[slen + 2] = '\0';
        return (int64_t)(intptr_t)buf;
    }
    /* Unknown / Pack tag — fall back to the full-form recursive dispatcher. */
    return _wasm_value_to_debug_string_full(val);
}

/* C23B-003 reopen 2: recursive debug-string variant for nested typed
   runtime objects. Mirrors native's `taida_value_to_debug_string_full`.
   Must be defined BEFORE `_wasm_pack_to_string_full` and the
   container renderers call
   it — but each of those uses it from the function body, so forward
   declarations at the top of this fragment keep the ordering valid. */
static int64_t _wasm_value_to_debug_string_full(int64_t val) {
    if (val == 0) return WSTR("0");
    /* HashMap / Set always display the public `HashMap({...})` /
       `Set({...})` shape — the carrier fields are compiler-internal. */
    if (_is_wasm_hashmap(val)) return _wasm_hashmap_to_string(val);
    if (_is_wasm_set(val)) return _wasm_set_to_string(val);
    /* C23B-003 reopen 3: render empty packs (`@()`) with their interpreter-
       parity string. Must be checked BEFORE `_looks_like_pack` (which
       requires `fc >= 1`) and BEFORE the int fallback which would otherwise
       emit the raw heap pointer of the `taida_pack_new(0)` allocation. */
    if (_looks_like_empty_pack(val)) return WSTR("@()");
    /* List items recurse through the full-form helper so nested
       HashMap/Set/Pack inside a list keep their full shape. */
    if (_looks_like_list(val)) {
        int64_t *list = (int64_t *)(intptr_t)val;
        int64_t len = list[1];
        /* C23B-005: honour the stored elem_type_tag so primitive items
           bypass the structural detectors. */
        int64_t elem_tag = list[2];
        _wasm_strbuf sb;
        _sb_init(&sb);
        _sb_append(&sb, "@[");
        for (int64_t i = 0; i < len; i++) {
            if (i > 0) _sb_append(&sb, ", ");
            int64_t item = list[WASM_LIST_ELEMS + i];
            int64_t item_str = _wasm_render_elem_tagged_debug_full(item, elem_tag);
            _sb_append(&sb, (const char *)(intptr_t)item_str);
        }
        _sb_append(&sb, "]");
        return _sb_finish(&sb);
    }
    /* PR-4: Check Async before monadic types (unchanged from
       `_wasm_value_to_debug_string`). Async uses its short-form string
       because the interpreter's `Async.to_display_string()` is itself a
       short-form (`Async[fulfilled: …]` etc.) — not a full-form
       BuchiPack. */
    if (_wasm_is_async_obj(val)) return _wasm_async_to_string(val);
    /* Check Error before generic pack (unchanged). */
    if (_wasm_is_error(val)) return _wasm_error_to_string(val);
    /* All pack shapes (Lax / Result / Gorillax / user packs) must
       recurse through the full-form helper so nested typed values
       continue to render with their `__`-prefixed internals. */
    if (_looks_like_pack(val)) return _wasm_pack_to_string_full(val);
    if (_looks_like_string(val)) {
        const char *s = (const char *)(intptr_t)val;
        int slen = wasm_strlen(s);
        char *buf = _wasm_str_alloc(slen + 3);
        if (!buf) return val;
        buf[0] = '"';
        for (int i = 0; i < slen; i++) buf[1 + i] = s[i];
        buf[slen + 1] = '"';
        buf[slen + 2] = '\0';
        return (int64_t)(intptr_t)buf;
    }
    return taida_int_to_str(val);
}

/* C21B-seed-07: stdout-display entry point for BuchiPack — matches the
   native `taida_stdout_display_string` contract. When the value is a pack
   (fc=4 Lax / fc=3 Result / user packs / Gorillax), the interpreter-parity
   stdout rendering shows ALL fields including `__value` / `__default` /
   `__type`. `.toString()` still uses the short forms (`Lax(v)` /
   `Result(v)` / `Gorillax(v)`) — that split mirrors the interpreter's
   `to_display_string()` vs. `to_string()` separation. */
/* C25B-001 forward declarations (definitions live in 02_containers.inc.c). */
static int _wasm_is_stream_pack(int64_t obj);
static int64_t _wasm_stream_to_display_string(int64_t stream_ptr);

static int64_t _wasm_stdout_display_string(int64_t obj) {
    if (obj == 0) return WSTR("0");
    /* C25B-001: Stream packs render as `Stream[completed: N items]`
       (matching the interpreter's `Value::Stream` display shape).
       Must precede the generic pack path so `Str[Stream[...]]()`
       doesn't stringify the internal `__stream_*` fields. */
    if (_wasm_is_stream_pack(obj)) return _wasm_stream_to_display_string(obj);
    /* HashMap / Set display the public `HashMap({...})` / `Set({...})`
       shape everywhere — stdout, interpolation and `Str[...]()` alike.
       The `__entries` / `__items` carrier fields are compiler-internal
       and the reference no longer prints them from any path. */
    if (_is_wasm_hashmap(obj)) return _wasm_hashmap_to_string(obj);
    if (_is_wasm_set(obj)) return _wasm_set_to_string(obj);
    /* C23B-003 reopen 3: top-level `stdout(@())` / `Str[@()]()` must render
       as the interpreter's `Value::Unit.to_debug_string()` output `"@()"`
       rather than the raw heap pointer (the `taida_pack_new(0)` allocation
       stores `field_count = 0`, which `_looks_like_pack` rejects and which
       `_is_pack_for_stdout` therefore also rejects). Checked before
       `_looks_like_list` to avoid any path where the list branch below
       could be reached on an empty pack (it wouldn't — `_looks_like_list`
       needs `cap >= 8` — but keeping the ordering tight mirrors the other
       early-return checks and makes the intent explicit). */
    if (_looks_like_empty_pack(obj)) return WSTR("@()");
    /* C23B-003 reopen 2: Lists must render items through the full-form
       debug helper so nested HashMap/Set/Pack keep their interpreter
       shape. `_wasm_list_to_string` alone would dispatch those items
       to the short-form `_wasm_value_to_debug_string` and collapse the
       nested HashMap/Set to `HashMap({…})` / `Set({…})`. */
    if (_looks_like_list(obj)) {
        int64_t *list = (int64_t *)(intptr_t)obj;
        int64_t len = list[1];
        /* C23B-005: dispatch list items by the stored elem_type_tag so
           primitive elements (Int / Float / Bool / Str) bypass the
           structural collection detectors entirely. */
        int64_t elem_tag = list[2];
        _wasm_strbuf sb;
        _sb_init(&sb);
        _sb_append(&sb, "@[");
        for (int64_t i = 0; i < len; i++) {
            if (i > 0) _sb_append(&sb, ", ");
            int64_t item = list[WASM_LIST_ELEMS + i];
            int64_t item_str = _wasm_render_elem_tagged_debug_full(item, elem_tag);
            _sb_append(&sb, (const char *)(intptr_t)item_str);
        }
        _sb_append(&sb, "]");
        return _sb_finish(&sb);
    }
    /* Use the same runtime guard as stdout_with_tag so Lists / HashMaps
       / Sets / Async do not accidentally take the pack path — their
       structural signatures overlap with `_looks_like_pack`. */
    if (_is_pack_for_stdout(obj)) return _wasm_pack_to_string_full(obj);
    return _wasm_value_to_display_string(obj);
}

/* W-5f: Detect Lax, Result, Gorillax, RelaxedGorillax by pack structure.
   These all have fc=4 with distinctive first-field hashes:
   - Lax:              hash0 = WASM_HASH_HAS_VALUE (0x175d21da0757452d)
   - Gorillax/Relaxed: hash0 = WASM_HASH_HAS_VALUE (0x175d21da0757452d)  [C24-A]
   - Result:           hash0 = WASM_HASH___VALUE    (0x0a7fc9f13472bbe0)

   C24-A (2026-04-23): Lax and Gorillax now share hash0 (both use
   `has_value` as the first-field name to match interpreter / JS / native).
   `_wasm_is_gorillax` / `_wasm_is_lax` disambiguate via the field-2
   hash — `__error` (WASM_HASH___ERROR) for Gorillax / RelaxedGorillax
   vs `__default` (WASM_HASH___DEFAULT) for Lax. The hash check is a
   plain int64_t compare (no pointer dereference), which keeps the
   wasm-min size gate green. */

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
    /* Result: core wasm stores `throw` at field 2; wasm-wasi keeps its
       historical display order with `throw` at field 1. Require the
       `__type` slot too so user packs that merely contain `__value` and
       `throw` do not route as Result. */
    if (p[0] == 4 && p[1] == WASM_HASH___VALUE) {
        int64_t hash1 = p[1 + 1 * 3];
        int64_t hash2 = p[1 + 2 * 3];
        int64_t hash3 = p[1 + 3 * 3];
        if ((hash1 == WASM_HASH_THROW || hash2 == WASM_HASH_THROW)
            && hash3 == WASM_HASH___TYPE) return 1;
    }
    return 0;
}

static int _wasm_is_gorillax(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    /* C24-A: Gorillax / RelaxedGorillax: fc=4, hash0 = WASM_HASH_HAS_VALUE
       (unified with Lax), disambiguate via field-2 hash
       (`__error` for Gorillax / RelaxedGorillax, `__default` for Lax).
       Pack layout places field-2 hash at offset `1 + 2*3 = 7`. */
    if (p[0] != 4 || p[1] != WASM_HASH_HAS_VALUE) return 0;
    return p[1 + 2 * 3] == WASM_HASH___ERROR ? 1 : 0;
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
        return val ? WSTR("true") : WSTR("false");
    }
    /* For INT, STR, PACK etc. — use generic display */
    return _wasm_value_to_display_string(val);
}

/* W-5f: Lax.toString() — "Lax(value)" or "Lax(default: value)" */
static int64_t _wasm_lax_to_string(int64_t lax_ptr) {
    int64_t has_value = taida_pack_get_idx(lax_ptr, 0); /* has_value */
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
    if (throw_val == 0) return WSTR("error");
    /* If it's a BuchiPack, look for "message" field. Explicit empty
       messages render as "" instead of falling back to the type name. */
    if (_looks_like_pack(throw_val)) {
        int64_t *p = (int64_t *)(intptr_t)throw_val;
        int64_t fc = p[0];
        for (int64_t i = 0; i < fc; i++) {
            if (p[1 + i * 3] == WASM_HASH_MESSAGE) {
                int64_t tag = p[1 + i * 3 + 1];
                int64_t msg = p[1 + i * 3 + 2];
                if (msg == 0) return WSTR("\"\"");
                if (tag == WASM_TAG_STR) {
                    const char *ms = (const char *)(intptr_t)msg;
                    if (ms[0] == '\0') return WSTR("\"\"");
                    return msg;
                }
                if (msg && _looks_like_string(msg)) {
                    const char *ms = (const char *)(intptr_t)msg;
                    if (ms[0] == '\0') return WSTR("\"\"");
                    return msg;
                }
                break;
            }
        }
        /* Error-derived pack with no message field — surface the
           declared `__type` name so the four-backend toString output
           stays aligned with the Interpreter / Native fallback. */
        for (int64_t i = 0; i < fc; i++) {
            if (p[1 + i * 3] == WASM_HASH___TYPE) {
                int64_t ty = p[1 + i * 3 + 2];
                if (ty && _looks_like_string(ty)) return ty;
                break;
            }
        }
        /* Anonymous pack — render structure verbatim. */
        return _wasm_value_to_display_string(throw_val);
    }
    /* String or other value */
    return _wasm_value_to_display_string(throw_val);
}

/* W-5g: Result.toString() — predicate-aware, matching native */
static int64_t _wasm_result_to_string(int64_t result) {
    if (!_wasm_result_is_error_check(result)) {
        /* Success case */
        int64_t value = taida_pack_get(result, WASM_HASH___VALUE);
        int64_t value_str = _wasm_value_to_display_string(value);
        _wasm_strbuf sb;
        _sb_init(&sb);
        _sb_append(&sb, "Result(");
        _sb_append(&sb, (const char *)(intptr_t)value_str);
        _sb_append(&sb, ")");
        return _sb_finish(&sb);
    }
    /* Error case — throw_val == 0 means Unit (@()), matching interpreter */
    int64_t throw_val = taida_pack_get(result, WASM_HASH_THROW);
    if (throw_val == 0) {
        return WSTR("Result(throw <= @())");
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

/* Detect Error pack: runtime-created errors carry type/message plus __type. */
static int _wasm_is_error(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 32)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] < 2 || p[0] > 10 || p[1] != WASM_HASH_TYPE || p[4] != WASM_HASH_MESSAGE) return 0;
    for (int64_t i = 0; i < p[0]; i++) {
        if (p[1 + i * 3] == WASM_HASH___TYPE) return 1;
    }
    return 0;
}

/* Error.toString() — "Error(type: message)" format (matching interpreter) */
static int64_t _wasm_error_to_string(int64_t val) {
    int64_t *p = (int64_t *)(intptr_t)val;
    int64_t fc = p[0];
    int64_t type = 0;
    int64_t msg = 0;
    for (int64_t i = 0; i < fc; i++) {
        if (p[1 + i * 3] == WASM_HASH_TYPE) {
            type = p[1 + i * 3 + 2];
        } else if (p[1 + i * 3] == WASM_HASH_MESSAGE) {
            msg = p[1 + i * 3 + 2];
        }
    }
    const char *type_s = (type && _looks_like_string(type)) ? (const char *)(intptr_t)type : "Error";
    const char *msg_s = (msg && _looks_like_string(msg)) ? (const char *)(intptr_t)msg : "";
    _wasm_strbuf sb;
    _sb_init(&sb);
    _sb_append(&sb, "Error(");
    _sb_append(&sb, type_s);
    _sb_append(&sb, ": ");
    _sb_append(&sb, msg_s);
    _sb_append(&sb, ")");
    return _sb_finish(&sb);
}

static int _wasm_is_async_task_pack(int64_t val) {
    if (!_looks_like_pack(val)) return 0;
    if (!taida_pack_has_hash(val, WASM_HASH_TODO_TASK)) return 0;
    if (!taida_pack_has_hash(val, WASM_HASH___TYPE)) return 0;
    int64_t type_name = taida_pack_get(val, WASM_HASH___TYPE);
    return taida_str_eq(type_name, WSTR("AsyncTask")) ? 1 : 0;
}

/* W-4f2: Convert value to display string (like native's taida_value_to_display_string) */
static int64_t _wasm_value_to_display_string(int64_t val) {
    if (val == 0) return WSTR("0");
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
    if (_wasm_is_async_task_pack(val)) return WSTR("AsyncTask[<task>]");
    /* Check Pack (field_count + hash pattern) */
    if (_looks_like_pack(val)) return _wasm_pack_to_string(val);
    /* C23B-003 reopen 3: empty packs (`@()`) have `fc == 0` and slip past
       `_looks_like_pack`. Without this branch, `stdout(@())` on wasm-wasi
       prints the raw heap pointer (e.g. `73040`) instead of `@()`. */
    if (_looks_like_empty_pack(val)) return WSTR("@()");
    /* Check if it's a string */
    if (_looks_like_string(val)) return val;
    /* Fallback: integer */
    return taida_int_to_str(val);
}

/* W-4f2: Convert value to debug string (strings are quoted, everything else like display) */
static int64_t _wasm_value_to_debug_string(int64_t val) {
    if (val == 0) return WSTR("0");
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
    /* C23B-003 reopen 3: same empty-pack fallback as the display helper so
       `_wasm_list_to_string` / `_wasm_pack_to_string` rendering nested
       `@()` don't collapse to pointer integers in the short-form path. */
    if (_looks_like_empty_pack(val)) return WSTR("@()");
    /* Check if it's a string — quote it for debug */
    if (_looks_like_string(val)) {
        const char *s = (const char *)(intptr_t)val;
        int slen = wasm_strlen(s);
        char *buf = _wasm_str_alloc(slen + 3);
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

/* ── W-4: BuchiPack runtime (bump allocator, no RC) ── */
/* Layout: [field_count, field0_hash, field0_tag, field0_value, ...,
            WASM_PACK_MAGIC]
   No refcount and no leading header (unlike native_runtime.c), but a
   trailing WASM_PACK_MAGIC sentinel at slot [1 + field_count * 3]
   positively identifies non-empty packs (empty packs store
   WASM_EMPTY_PACK_MAGIC at slot [1] instead).
   Each field occupies 3 int64_t slots: hash, tag, value.
   Total allocation: (2 + field_count * 3) * sizeof(int64_t) */

int64_t taida_pack_new(int64_t field_count) {
    /* C23B-003 reopen 4 (2026-04-22): empty packs (`@()` / `Value::Unit`)
       used to allocate a single int64_t slot `[0]`. `_looks_like_empty_pack`
       then relied on a bump-range + `*(int64_t*)addr == 0` heuristic to
       distinguish the empty pack pointer from an arbitrary int value.
       That heuristic false-positives on dynamic Int expressions (e.g.
       `Str[a + b]()` when `a + b` lands inside the bump arena and the
       memory at that offset happens to be zero), causing wasm to render
       the integer as `@()`. Fix: stamp a magic sentinel in the slot
       immediately after `field_count` for empty packs. Non-empty packs
       are unaffected — their `field_count >= 1` already disambiguates
       them from empty packs, and they keep `[field_count, field0_hash,
       field0_tag, field0_value, …]` layout. Detectors now identify an
       empty pack by `data[0] == 0 && data[1] == WASM_EMPTY_PACK_MAGIC`,
       which is collision-free with any int64_t value produced by user
       code (the magic has high bits set and a unique trailing-byte
       pattern — see the `WASM_EMPTY_PACK_MAGIC` definition below).

       C25B-026 (2026-04-23, Codex reopen of C24-A HOLD): cache a single
       empty-pack allocation and reuse the pointer on every subsequent
       call. The wasm bump allocator never frees, so the old
       "one 16-byte allocation per empty pack" path leaked linearly for
       every `Gorillax[v]()` (which stamps `__error <= @()` by calling
       `taida_pack_new(0)`) or any other repeated `@()` producer. A
       long-running program that constructs successful Gorillax values
       in a loop would grow `bump_ptr` by 16 B × N until
       `memory.grow` ran out of pages and trapped. Because an empty
       pack is immutable (field_count = 0 means no `taida_pack_set`
       target exists) and all readers only consult the
       `WASM_EMPTY_PACK_MAGIC` slot, the singleton is race-free and
       indistinguishable from per-call allocation at the observable
       level. Native's `taida_gorillax_new` already avoids this leak
       via a `PACK + 0 → @()` rendering special-case (no per-call
       allocation); the singleton approach here is the wasm-side
       equivalent without touching the detector or renderer path. */
    if (field_count == 0) {
        static int64_t empty_pack_singleton = 0;
        if (empty_pack_singleton == 0) {
            int64_t *pack = (int64_t *)wasm_alloc(16);
            if (!pack) return 0;
            pack[0] = 0;
            pack[1] = WASM_EMPTY_PACK_MAGIC;
            empty_pack_singleton = (int64_t)(intptr_t)pack;
        }
        return empty_pack_singleton;
    }
    /* C23B-005 (2026-04-22): append a magic sentinel slot after the last
       field so `_looks_like_pack` can positive-identify non-empty packs
       without the fc-range + first-hash heuristic. Layout becomes
       `[fc, hash0, tag0, val0, hash1, ..., WASM_PACK_MAGIC]`. All
       existing readers only touch slots `[0..1+fc*3)`, so appending
       one extra slot is fully backward compatible. */
    int64_t slots = 1 + field_count * 3 + 1;  /* + magic tail */
    int64_t *pack = (int64_t *)wasm_alloc((unsigned int)(slots * 8));
    if (!pack) return 0;
    pack[0] = field_count;
    /* Zero-initialize all field slots (hash=0, tag=0=INT, value=0) */
    for (int64_t i = 1; i < 1 + field_count * 3; i++) pack[i] = 0;
    pack[1 + field_count * 3] = WASM_PACK_MAGIC;
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

// NB-14: Stack-based call-site argument type tag propagation (Bool/Int disambiguation).
// WASM has the same Bool/Int representation issue as Native.
// The stack ensures nested calls do not overwrite the outer call's tags.
#define WASM_TAG_STACK_DEPTH 64
#define WASM_TAG_FRAME_SIZE 256

static int64_t __wasm_tag_stack[WASM_TAG_STACK_DEPTH][WASM_TAG_FRAME_SIZE];
static int __wasm_tag_stack_top = 0;

void taida_push_call_tags(void) {
    if (__wasm_tag_stack_top < WASM_TAG_STACK_DEPTH) {
        memset(__wasm_tag_stack[__wasm_tag_stack_top], 0xFF, sizeof(__wasm_tag_stack[0]));
        __wasm_tag_stack_top++;
    }
}

void taida_pop_call_tags(void) {
    if (__wasm_tag_stack_top > 0) {
        __wasm_tag_stack_top--;
    }
}

int64_t taida_set_call_arg_tag(int64_t index, int64_t tag) {
    if (__wasm_tag_stack_top > 0 && index >= 0 && index < WASM_TAG_FRAME_SIZE) {
        __wasm_tag_stack[__wasm_tag_stack_top - 1][index] = tag;
    }
    return 0;
}

int64_t taida_get_call_arg_tag(int64_t index) {
    if (__wasm_tag_stack_top > 0 && index >= 0 && index < WASM_TAG_FRAME_SIZE) {
        return __wasm_tag_stack[__wasm_tag_stack_top - 1][index];
    }
    return -1; // UNKNOWN
}

/* C12B-022: WASM mirror of taida_primitive_tag_match (native_runtime.c).
 * Returns 0 for UNKNOWN(-1) tags so the lowerer does not inadvertently
 * match on wasm profiles when param-tag propagation was not set up.
 * -10 sentinel = Num (INT||FLOAT) match, otherwise tag == expected. */
int64_t taida_primitive_tag_match(int64_t tag, int64_t expected) {
    if (tag == -1) return 0;
    if (expected == -10) {
        return (tag == WASM_TAG_INT || tag == WASM_TAG_FLOAT) ? 1 : 0;
    }
    return (tag == expected) ? 1 : 0;
}

// NB-14: Return type tag propagation (WASM mirror of native_runtime.c).
/* Non-static: the generated C reads/writes this slot directly (the
   call-site expansion of taida_get/set_return_tag — one runtime call
   per user-function call showed up as half the remaining call
   overhead in recursive hot loops). The two functions below stay as
   the ABI for any caller outside the generated module. */
int64_t __wasm_return_tag = -1; // UNKNOWN

int64_t taida_set_return_tag(int64_t tag) {
    __wasm_return_tag = tag;
    return 0;
}

int64_t taida_get_return_tag(void) {
    int64_t tag = __wasm_return_tag;
    __wasm_return_tag = -1;
    return tag;
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

/* B11-2b: Get the type tag for a field by its hash. Returns -1 (UNKNOWN) if not found. */
int64_t taida_pack_get_field_tag(int64_t pack_ptr, int64_t field_hash) {
    int64_t *pack = (int64_t *)(intptr_t)pack_ptr;
    int64_t count = pack[0];
    for (int64_t i = 0; i < count; i++) {
        if (pack[1 + i * 3] == field_hash) {
            return pack[1 + i * 3 + 1];
        }
    }
    return -1; /* TAIDA_TAG_UNKNOWN */
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

/* ── W-4: List runtime (bump allocator, no RC) ── */
/* Layout: [capacity, length, elem_type_tag, elem0, ..., trailing magic]
   No refcount and no leading header (unlike native_runtime.c), but a
   trailing magic at slot [WASM_LIST_ELEMS + capacity] positively
   identifies lists (see taida_list_new / _is_wasm_hashmap).
   Note: bump allocator cannot realloc, so we use copy-on-grow. */

int64_t taida_list_new(void) {
    int64_t initial_cap = 16;
    /* C23B-005 reopen (2026-04-22): allocate one extra slot at
       `list[WASM_LIST_ELEMS + initial_cap]` to stamp a trailing
       magic, giving lists 128 bits of identification entropy. See the
       `_is_wasm_hashmap` comment for the rationale. */
    int64_t slots = WASM_LIST_ELEMS + initial_cap + 1;  /* header(4) + elements + trailing magic */
    int64_t *list = (int64_t *)wasm_alloc((unsigned int)(slots * 8));
    if (!list) return 0;
    list[0] = initial_cap;       /* capacity */
    list[1] = 0;                 /* length */
    list[2] = -1;                /* elem_type_tag (UNKNOWN) */
    list[3] = WASM_LIST_MAGIC;   /* head magic */
    list[WASM_LIST_ELEMS + initial_cap] = WASM_LIST_MAGIC;  /* trailing magic */
    return (int64_t)(intptr_t)list;
}

void taida_list_set_elem_tag(int64_t list_ptr, int64_t tag) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    /* C23B-005 (2026-04-22): downgrade on type conflict so mixed lists
       cannot pin a single primitive tag for all elements.
       C23B-007 (2026-04-22): use a dedicated HETEROGENEOUS sentinel
       (-2) instead of reusing UNKNOWN (-1). Previously a downgraded
       list was indistinguishable from an unset one, so a subsequent
       `.push()` with a fresh primitive tag would RE-PROMOTE the
       container back to a primitive tag and the renderer would then
       force every element through that tag — e.g. `@[1, "a", 2]`
       got rendered as `@[1, 1127, 2]` because after the string was
       downgraded the trailing `2` re-promoted the list to INT. Now:
         existing == UNKNOWN(-1)    → stamp the new tag (first write)
         existing == HETEROGENEOUS  → keep (never re-promote)
         existing == new_tag        → no-op
         otherwise                  → downgrade to HETEROGENEOUS(-2) */
    /* Value-tag track: instead of throwing the per-element information away
       at the downgrade point, the first conflict materialises the kind
       array (every element already in the list is guaranteed to carry the
       old homogeneous tag by the latch invariant) and subsequent calls
       append to it. Contract: at codegen pre-push call sites list[1] is the
       index of the element about to be pushed. Runtime-internal bulk
       callers only ever (re)assert a single tag on a fresh or same-tag
       container, so they can never reach the materialise/append branches. */
    int64_t existing = list[2];
    if (_wasm_elem_slot_is_array(existing)) {
        uint32_t ek = tag >= 0 ? WASM_EKIND((uint32_t)tag, 0) : WASM_EKIND_UNKNOWN;
        _wasm_elem_tags_append(list, list[1], ek);
        return;
    }
    if (existing == WASM_TAG_HETEROGENEOUS) return;
    if (existing == WASM_TAG_UNKNOWN || existing == tag) {
        list[2] = tag;
    } else {
        uint32_t ek = tag >= 0 ? WASM_EKIND((uint32_t)tag, 0) : WASM_EKIND_UNKNOWN;
        _wasm_elem_tags_materialise(list, existing, list[1], ek);
    }
}

int64_t taida_list_push(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t cap = list[0];
    int64_t len = list[1];
    if (len >= cap) {
        /* Grow: allocate new list with double capacity, copy over.
           C23B-005 reopen: include one extra slot for the trailing
           magic, stamped after the copy propagates the head magic. */
        int64_t new_cap = cap * 2;
        int64_t new_slots = WASM_LIST_ELEMS + new_cap + 1;
        int64_t *new_list = (int64_t *)wasm_alloc((unsigned int)(new_slots * 8));
        if (!new_list) return list_ptr;
        /* Copy header + existing elements */
        for (int64_t i = 0; i < WASM_LIST_ELEMS + len; i++) new_list[i] = list[i];
        new_list[0] = new_cap;
        /* Re-stamp trailing magic at new end — the head magic at
           list[3] was already copied and reflects either the
           list-new stamp or the set-new overwrite. */
        new_list[WASM_LIST_ELEMS + new_cap] = list[3];  /* same magic as head */
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

/* Kind-stamped Lax constructor (mirror of native taida_lax_new_k):
   identical to taida_lax_new but records the payload's known scalar kind on
   the __value field tag, so an unmolding call site can read it back
   (taida_lax_value_ekind) and run exact pair comparisons on the payload.
   Heap kinds keep whatever the renderer's tag heuristic classified; ENUM
   stays on the heuristic path too for now (the pack field tag's existing
   consumers only understand the legacy scalar tags). */
static int64_t _wasm_lax_new_k(int64_t value, int64_t default_value, uint32_t ekind) {
    int64_t pack = taida_lax_new(value, default_value);
    uint32_t k = ekind & 0xFFu;
    /* Stamp every known kind except ENUM (native mirror): in particular
       STR, so the constructor heuristic's bare INT on a string payload
       can never be mistaken for a trusted kind by the shadow reader. */
    if (k != WASM_EKIND_UNKNOWN && k != (uint32_t)WASM_TAG_ENUM) {
        taida_pack_set_tag(pack, 1, (int64_t)k);
    }
    return pack;
}

/* taida_list_get: returns Lax[T]. OOB returns empty Lax. */
int64_t taida_list_get(int64_t list_ptr, int64_t index) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (index < 0 || index >= len) return taida_lax_empty(0);
    return _wasm_lax_new_k(list[WASM_LIST_ELEMS + index], 0, _wasm_elem_kind_at(list, index));
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

/* C23B-008 (2026-04-22): see the `WASM_HM_ORD_*` macros near the top of
   this file (colocated with `WASM_HM_HEADER`) for the insertion-order
   side-index layout. `_wasm_hashmap_new_with_cap` stamps the magic +
   ord header; all mutators and iterators consult those macros. */
static int64_t _wasm_hashmap_new_with_cap(int64_t cap) {
    /* C23B-005 / C23B-006 (2026-04-22): allocate one extra slot at the
       end and stamp `WASM_HM_MAGIC` there as a *second* magic sentinel.
       This defeats the memory-overlap attack where an untagged large
       Int value (stored in a HashMap value slot) happens to equal the
       base pointer of another HashMap allocation in the same bump
       arena: single-magic detection would pass because that real
       HashMap has the magic at `data[3]`, but the trailing magic at
       `data[WASM_HM_HEADER + cap*3]` is at a different offset that
       cannot simultaneously align for both the fake and real
       interpretations. 2 × 64 = 128 bits of entropy makes false
       identification effectively impossible.
       C23B-008 (2026-04-22): additionally allocate `1 + cap` slots at
       the end for the insertion-order side-index (`next_ord` + order
       array). The trailing-magic position is UNCHANGED so `_is_wasm_hashmap`
       still probes `data[WASM_HM_HEADER + cap*3]` — the extra slots sit
       at higher offsets that only HashMap-order-aware code accesses. */
    int64_t slots = WASM_HM_HEADER + cap * 3 + 1 + 1 + cap;  /* + trailing magic + next_ord + order_array[cap] */
    int64_t *hm = (int64_t *)wasm_alloc((unsigned int)(slots * 8));
    if (!hm) return 0;
    /* Zero-initialize everything (empty slots have hash=0, key=0).
       This also initialises `next_ord = 0` and every `order_array[i] = 0`;
       `order_array[i]` is only consulted for indices `i < next_ord`, and
       while the container is empty `next_ord == 0` so the zeros are never
       read. Once populated, set() will overwrite the relevant slots. */
    for (int64_t i = 0; i < slots; i++) hm[i] = 0;
    hm[0] = cap;
    hm[1] = 0;     /* length */
    hm[2] = -1;    /* value_type_tag = UNKNOWN */
    hm[3] = WASM_HM_MARKER_VAL;              /* head magic */
    hm[WASM_HM_HEADER + cap * 3] = WASM_HM_MAGIC;  /* trailing magic */
    /* next_ord (hm[WASM_HM_HEADER + cap*3 + 1]) and order_array (the
       `cap` slots after that) are left zero-initialised. */
    return (int64_t)(intptr_t)hm;
}

int64_t taida_hashmap_new(void) {
    return _wasm_hashmap_new_with_cap(16);
}

void taida_hashmap_set_value_tag(int64_t hm_ptr, int64_t tag) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    /* C23B-005 / C23B-006 (2026-04-22): the codegen calls this on every
       `.set()`, not just the first one. For homogeneous HashMaps the
       tag is stable; for heterogeneous HashMaps (e.g.
       `hashMap().set("name", "Asuka").set("age", 14)`) the subsequent
       calls would silently overwrite an already-established tag with
       a different primitive type, leaving `hm[2]` misleadingly
       pointing at the last-inserted value's type. When that conflict
       happens we downgrade to HETEROGENEOUS(-2) so display helpers fall
       back to structural dispatch rather than trust a wrong tag.
       First-write (tag transitioning from UNKNOWN) is kept intact.
       C23B-007 (2026-04-22): introduced the HETEROGENEOUS(-2) sentinel
       distinct from UNKNOWN(-1). Reusing -1 meant a three-way mix like
       `.set("a", 1).set("b", "x").set("c", 2)` would downgrade after
       the second call but then silently RE-PROMOTE to INT on the third
       call, letting the renderer emit the pointer of "x" as an Int
       (`value <= 1058`). The HETEROGENEOUS latch makes the downgrade
       terminal. */
    int64_t existing = hm[2];
    if (existing == WASM_TAG_HETEROGENEOUS) return;
    if (existing == WASM_TAG_UNKNOWN || existing == tag) {
        hm[2] = tag;
    } else {
        hm[2] = WASM_TAG_HETEROGENEOUS;
    }
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
        /* C23B-008 (2026-04-22): preserve insertion order across resize
           by walking the OLD order array (`order_array[0..old_next_ord]`)
           and re-inserting each entry into the new table in the same
           sequence. Removed entries (`old_order[i] == -1` or whose bucket
           has since been emptied) are skipped. The new table's order
           side-index is rebuilt as we go — `new_next_ord` grows
           monotonically from 0. */
        int64_t old_next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
        int64_t new_next_ord = 0;
        for (int64_t oi = 0; oi < old_next_ord; oi++) {
            int64_t slot = hm[WASM_HM_ORD_SLOT(cap, oi)];
            if (slot < 0 || slot >= cap) continue;  /* removed or invalid */
            int64_t sh = hm[WASM_HM_HEADER + slot * 3];
            int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
            if (sh == 0 && sk == 0) continue;  /* bucket emptied */
            int64_t sval = hm[WASM_HM_HEADER + slot * 3 + 2];
            /* Insert into new table */
            uint64_t uh = (uint64_t)sh;
            int64_t idx = (int64_t)(uh % (uint64_t)new_cap);
            for (int64_t j = 0; j < new_cap; j++) {
                int64_t nslot = (idx + j) % new_cap;
                int64_t esh = new_hm[WASM_HM_HEADER + nslot * 3];
                int64_t esk = new_hm[WASM_HM_HEADER + nslot * 3 + 1];
                if (esh == 0 && esk == 0) {
                    new_hm[WASM_HM_HEADER + nslot * 3] = sh;
                    new_hm[WASM_HM_HEADER + nslot * 3 + 1] = sk;
                    new_hm[WASM_HM_HEADER + nslot * 3 + 2] = sval;
                    new_hm[1]++;
                    new_hm[WASM_HM_ORD_SLOT(new_cap, new_next_ord)] = nslot;
                    new_next_ord++;
                    break;
                }
            }
        }
        new_hm[WASM_HM_ORD_HEADER_SLOT(new_cap)] = new_next_ord;
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
            /* Empty slot — insert. C23B-008: record the insertion ordinal
               in the order side-index so display / iteration honour the
               first-insertion-wins ordering that interpreter / JS use. */
            hm[WASM_HM_HEADER + slot * 3] = key_hash;
            hm[WASM_HM_HEADER + slot * 3 + 1] = key_ptr;
            hm[WASM_HM_HEADER + slot * 3 + 2] = value;
            hm[1]++;
            int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
            hm[WASM_HM_ORD_SLOT(cap, next_ord)] = slot;
            hm[WASM_HM_ORD_HEADER_SLOT(cap)] = next_ord + 1;
            return hm_ptr;
        }
        if (sh == key_hash && _wasm_streq((const char *)(intptr_t)sk, (const char *)(intptr_t)key_ptr)) {
            /* Existing key — update value in place. C23B-008: do NOT touch
               the order side-index; the first insertion's position is
               preserved, matching interpreter semantics (updating an
               existing key keeps its original Vec index). */
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
    /* C23B-008 (2026-04-22): propagate the insertion-order side-index
       so the clone iterates in the same order as the original. We copy
       both `next_ord` and every `order_array` slot verbatim since the
       bucket layout is identical (same cap → same hash → same slots). */
    int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
    new_hm[WASM_HM_ORD_HEADER_SLOT(cap)] = next_ord;
    for (int64_t i = 0; i < next_ord; i++) {
        new_hm[WASM_HM_ORD_SLOT(cap, i)] = hm[WASM_HM_ORD_SLOT(cap, i)];
    }
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

/* C23B-006 / C23B-005 reopen (2026-04-22): positive identification for
   bump-allocated HashMaps via TWO 8-byte printable-ASCII sentinels —
   one at `data[3]` (head), one at `data[WASM_HM_HEADER + cap * 3]`
   (trailing). Single-magic identification was still foolable when a
   user-supplied large Int living inside one HashMap's value slot
   coincidentally pointed at the base address of another HashMap
   allocation (both structures legitimately carry the head magic at
   `data[3]`). Requiring the trailing magic at the shape-dependent
   offset `data[WASM_HM_HEADER + cap * 3]` closes that attack because
   an attacker would need TWO independent 64-bit matches (128 bits
   total entropy) at offsets determined by the pointed-to memory's
   own `data[0]` value — astronomically unlikely to collide.
   `_wasm_hashmap_new_with_cap` allocates one extra slot for the
   trailing magic; all existing HashMap readers only touch the
   original layout `[cap, len, value_tag, head_magic, entries...]`. */
static int _is_wasm_hashmap(int64_t ptr) {
    if (!_wasm_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    if (data[3] != WASM_HM_MAGIC) return 0;
    /* Verify trailing magic. `data[0]` is the capacity — reject
       non-positive or wildly out-of-range caps before reading the
       trailing slot to keep the bounds check safe. */
    int64_t cap = data[0];
    if (cap <= 0 || cap > 0x100000) return 0;  /* 2^20 entries max = paranoid */
    unsigned int tail_bytes = (unsigned int)(WASM_HM_HEADER + cap * 3 + 1) * 8u;
    unsigned int mem_size = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    if ((uint64_t)addr + (uint64_t)tail_bytes > (uint64_t)mem_size) return 0;
    return data[WASM_HM_HEADER + cap * 3] == WASM_HM_MAGIC;
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

/* HashMap.keys() -> List of key pointers.
   C23B-008 (2026-04-22): walk via the insertion-order side-index so
   the returned list matches interpreter / JS ordering. */
int64_t taida_hashmap_keys(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t list = taida_list_new();
    int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
    for (int64_t oi = 0; oi < next_ord; oi++) {
        int64_t slot = hm[WASM_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (WASM_HM_SLOT_OCCUPIED(sh, sk)) {
            list = taida_list_push(list, sk);
        }
    }
    return list;
}

/* HashMap.values() -> List of values.
   C23B-008 (2026-04-22): insertion-order walk, same rationale. */
int64_t taida_hashmap_values(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t list = taida_list_new();
    /* Propagate the value tag onto the result list (native NO-1 twin) —
       without it a uniform-Float map's values displayed as raw bits. */
    ((int64_t *)(intptr_t)list)[2] = hm[2];
    int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
    for (int64_t oi = 0; oi < next_ord; oi++) {
        int64_t slot = hm[WASM_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (WASM_HM_SLOT_OCCUPIED(sh, sk)) {
            list = taida_list_push(list, hm[WASM_HM_HEADER + slot * 3 + 2]);
        }
    }
    return list;
}

/* HashMap.entries() -> List of BuchiPack @(key, value).
   C23B-008 (2026-04-22): insertion-order walk.
   C23B-009 (2026-04-22): (1) idempotently register "key" / "value" in
   `_wasm_field_registry` so `_wasm_pack_to_string_full` resolves the field
   names (unregistered hashes returned NULL → pairs rendered as `@()`,
   diverging from interpreter and the documented
   `docs/reference/standard_library.md:238` shape of `@[@(key, value)]`).
   (2) Stamp per-field tags (`WASM_TAG_STR` on `key`, the hashmap's stored
   `value_type_tag` on `value`) so primitives render through the tagged
   fast-path instead of `_wasm_value_to_debug_string` heuristics. Also
   flag the outer list's `elem_type_tag` = PACK so that list rendering
   descends into each pair via the full-form helper. */
int64_t taida_hashmap_entries(int64_t hm_ptr) {
    int64_t *hm = (int64_t *)(intptr_t)hm_ptr;
    int64_t cap = hm[0];
    int64_t val_tag = hm[2];  /* value_type_tag from hashmap header */
    int64_t list = taida_list_new();
    /* entries returns List[Pack] — stamp elem_type_tag = PACK for tagged
       rendering in `_wasm_render_elem_tagged_debug*`. */
    ((int64_t *)(intptr_t)list)[2] = WASM_TAG_PACK;
    /* FNV-1a hashes for "key" and "value" (same as native runtime) */
    #define WASM_HASH_KEY   0x3dc94a19365b10ecLL
    #define WASM_HASH_VAL   0x7ce4fd9430e80ceaLL
    /* C23B-009: register pair field names once. `taida_register_field_name`
       is idempotent (skips duplicates). */
    static int __wasm_entries_names_registered = 0;
    if (!__wasm_entries_names_registered) {
        __wasm_entries_names_registered = 1;
        taida_register_field_name(WASM_HASH_KEY, WSTR("key"));
        taida_register_field_name(WASM_HASH_VAL, WSTR("value"));
    }
    int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
    for (int64_t oi = 0; oi < next_ord; oi++) {
        int64_t slot = hm[WASM_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        int64_t sh = hm[WASM_HM_HEADER + slot * 3];
        int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
        if (WASM_HM_SLOT_OCCUPIED(sh, sk)) {
            int64_t pair = taida_pack_new(2);
            taida_pack_set_hash(pair, 0, WASM_HASH_KEY);
            /* C23B-009: key is always Str (HashMap key contract). */
            taida_pack_set_tag(pair, 0, WASM_TAG_STR);
            taida_pack_set(pair, 0, sk);
            taida_pack_set_hash(pair, 1, WASM_HASH_VAL);
            /* C23B-009: propagate hashmap value_type_tag onto the value slot
               so primitive values render correctly via the pack full-form
               helper (matches native's `val_tag` propagation). */
            taida_pack_set_tag(pair, 1, val_tag);
            taida_pack_set(pair, 1, hm[WASM_HM_HEADER + slot * 3 + 2]);
            list = taida_list_push(list, pair);
        }
    }
    return list;
}

/* HashMap.merge(other) -> new HashMap with other's entries overwriting.
   C23B-008 reopen (2026-04-22): interpreter semantics for merge are
   retain-then-push (`src/interpreter/methods.rs:787-822`) — any overlap
   key is REMOVED from self and re-appended at other's position with
   other's value. The previous wasm implementation copied self then
   called `taida_hashmap_set` per other entry; `taida_hashmap_set`
   updates in place for existing keys (preserving self's ordinal), so
   overlap keys ended up at self's position instead of other's. Repro:
   `a=[a,b]`, `b=[c,b,d]` expected `[a,c,b,d]`, got `[a,b,c,d]`.

   Fix (matches native + JS): allocate a fresh HashMap and fill it in
   the order interpreter would emit: first the self entries whose key
   is absent from other (self-order), then every other entry in
   other-order (guaranteed fresh to the result map).

   Note: `taida_hashmap_set` must keep its update-in-place ordinal
   semantics for plain `.set("k", v)` chains, so we cannot change set
   itself — we just avoid going through it for overlap keys. */
int64_t taida_hashmap_merge(int64_t hm_ptr, int64_t other_ptr) {
    int64_t *self = (int64_t *)(intptr_t)hm_ptr;
    int64_t *other = (int64_t *)(intptr_t)other_ptr;
    int64_t self_cap = self[0];
    int64_t other_cap = other[0];

    int64_t result = taida_hashmap_new();

    /* Step 1: self entries whose key is absent from other (self-order). */
    int64_t self_next_ord = self[WASM_HM_ORD_HEADER_SLOT(self_cap)];
    for (int64_t oi = 0; oi < self_next_ord; oi++) {
        int64_t slot = self[WASM_HM_ORD_SLOT(self_cap, oi)];
        if (slot < 0 || slot >= self_cap) continue;
        int64_t sh = self[WASM_HM_HEADER + slot * 3];
        int64_t sk = self[WASM_HM_HEADER + slot * 3 + 1];
        if (!WASM_HM_SLOT_OCCUPIED(sh, sk)) continue;
        if (taida_hashmap_has(other_ptr, sh, sk)) continue;
        result = taida_hashmap_set(result, sh, sk, self[WASM_HM_HEADER + slot * 3 + 2]);
    }

    /* Step 2: all other entries in other-order (all new to result). */
    int64_t other_next_ord = other[WASM_HM_ORD_HEADER_SLOT(other_cap)];
    for (int64_t oi = 0; oi < other_next_ord; oi++) {
        int64_t slot = other[WASM_HM_ORD_SLOT(other_cap, oi)];
        if (slot < 0 || slot >= other_cap) continue;
        int64_t sh = other[WASM_HM_HEADER + slot * 3];
        int64_t sk = other[WASM_HM_HEADER + slot * 3 + 1];
        if (!WASM_HM_SLOT_OCCUPIED(sh, sk)) continue;
        result = taida_hashmap_set(result, sh, sk, other[WASM_HM_HEADER + slot * 3 + 2]);
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
            /* C23B-008 (2026-04-22): null out the corresponding order
               slot so display / iteration helpers skip this ordinal. We
               walk `order_array[0..next_ord]` to find the slot index,
               then overwrite it with -1 (HOLE). `next_ord` itself is not
               decremented — it's a monotonic high-water mark used only
               as an upper bound for iteration. */
            int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
            for (int64_t oi = 0; oi < next_ord; oi++) {
                if (hm[WASM_HM_ORD_SLOT(cap, oi)] == slot) {
                    hm[WASM_HM_ORD_SLOT(cap, oi)] = -1;
                    break;
                }
            }
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
    if (set) {
        /* C23B-005: overwrite both the head and trailing magic with
           the Set sentinel so `_is_wasm_set`'s dual-magic check
           succeeds. `taida_list_new` stamped the list magic at both
           positions; we retag both here. */
        int64_t *s = (int64_t *)(intptr_t)set;
        int64_t cap = s[0];
        s[3] = WASM_SET_MARKER_VAL;
        s[WASM_LIST_ELEMS + cap] = WASM_SET_MARKER_VAL;
    }
    return set;
}

void taida_set_set_elem_tag(int64_t set_ptr, int64_t tag) {
    taida_list_set_elem_tag(set_ptr, tag);
}

/* W-4f/F-3: Type-tag-aware equality for Set elements.
   For strings, compare by content (strcmp). For others, compare by raw value. */
/* F54B-016 (G4): structural value equality for Set / list.unique, mirroring
   the interpreter `ValueKey` (src/interpreter/value_key.rs). Strings, Lists and
   BuchiPacks recurse; BuchiPack field order is ignored. Scalars (Int / Bool /
   Enum-ordinal) compare by raw value -- Enum is lowered to its Int ordinal, so
   Int<->Enum equivalence is automatic (enum-name distinction is out of scope,
   F54B-018). HashMap / Set elements fall back to identity. */
static int _wasm_list_struct_eq_k(int64_t *la, int64_t *lb); /* kind-aware list walk (below) */
static int _wasm_value_eq(int64_t a, int64_t b) {
    /* F56: a sealed carrier (Moltenized/Secret) is NEVER equal — not even to
       itself (same pointer). A content compare of __value would be an equality
       oracle (Unique dedup / Set / contains / indexOf / `@[a]==@[b]` leak the
       match bit), and the interpreter has value semantics (no pointer identity)
       so it always returns non-equal. Checked BEFORE the identity fast-path so
       every backend agrees (`a == a` is false for a secret). */
    if (_wasm_carrier_kind(a) || _wasm_carrier_kind(b)) return 0;
    if (a == b) return 1;
    int a_str = _looks_like_string(a), b_str = _looks_like_string(b);
    if (a_str || b_str)
        return (a_str && b_str)
            ? _wasm_streq((const char *)(intptr_t)a, (const char *)(intptr_t)b)
            : 0;
    int a_by = _looks_like_bytes(a), b_by = _looks_like_bytes(b);
    if (a_by || b_by) {
        if (!a_by || !b_by) return 0;
        int64_t lab = _wasm_bytes_len(a), lbb = _wasm_bytes_len(b);
        if (lab != lbb) return 0;
        for (int64_t i = 0; i < lab; i++)
            if (_wasm_bytes_at(a, i) != _wasm_bytes_at(b, i)) return 0;
        return 1;
    }
    int a_list = _looks_like_list(a), b_list = _looks_like_list(b);
    if (a_list || b_list) {
        if (!a_list || !b_list) return 0;
        /* Value-tag track: element pairs compare under their recorded kinds
           (single-tag containers report it via _wasm_elem_kind_at), so
           nested Bool↔Int stops colliding and nested Int↔Float crosses over
           like the interpreter's Value::eq. */
        int64_t *la = (int64_t *)(intptr_t)a, *lb = (int64_t *)(intptr_t)b;
        return _wasm_list_struct_eq_k(la, lb);
    }
    if (_is_wasm_hashmap(a) || _is_wasm_hashmap(b) || _is_wasm_set(a) || _is_wasm_set(b))
        return 0;
    int a_ep = _looks_like_empty_pack(a), b_ep = _looks_like_empty_pack(b);
    if (a_ep || b_ep) return (a_ep && b_ep) ? 1 : 0;
    int a_pack = _looks_like_pack(a), b_pack = _looks_like_pack(b);
    if (a_pack || b_pack) {
        if (!a_pack || !b_pack) return 0;
        int64_t *pa = (int64_t *)(intptr_t)a, *pb = (int64_t *)(intptr_t)b;
        int64_t na = pa[0], nb = pb[0];
        if (na != nb) return 0;
        for (int64_t i = 0; i < na; i++) {
            int64_t nh = pa[1 + i * 3];
            int64_t va = pa[1 + i * 3 + 2];
            int found = 0;
            for (int64_t j = 0; j < nb; j++) {
                if (pb[1 + j * 3] == nh && _wasm_value_eq(va, pb[1 + j * 3 + 2])) { found = 1; break; }
            }
            if (!found) return 0;
        }
        return 1;
    }
    return 0;
}

/* F54B-016 (G4) commit 2: O(n) fingerprint seen-set for Set / list.unique.
   FNV-1a over the same structure _wasm_value_eq compares; collisions confirmed
   with _wasm_value_eq. Engaged only when every element is _wasm_value_hashable;
   linear path kept otherwise. Bump-allocated (wasm_alloc), no free needed. */
static void _wasm_fnv_bytes(uint64_t *h, const void *p, int n) {
    const unsigned char *b = (const unsigned char*)p;
    for (int i = 0; i < n; i++) { *h ^= b[i]; *h *= 0x100000001b3ULL; }
}
/* Forward declaration: the structural accumulator and the kind-aware one
   are mutually recursive (lists hash their elements under recorded kinds;
   non-scalar kinds delegate back to the structural walk). */
static void _wasm_fp_accum_k(int64_t v, uint32_t ekind, uint64_t *h);
static void _wasm_fp_accum(int64_t v, uint64_t *h) {
    /* F56: a sealed carrier contributes a fixed tag, never its __value — mixing
       the plaintext into a hash is a side channel (and the hashable gate keeps
       carriers out of collection hashing entirely). */
    if (_wasm_carrier_kind(v)) { unsigned char tag = 0x5F; _wasm_fnv_bytes(h, &tag, 1); return; }
    if (_looks_like_string(v)) {
        unsigned char tag = 2; _wasm_fnv_bytes(h, &tag, 1);
        const char *s = (const char*)(intptr_t)v; int l = _wf_strlen(s);
        uint64_t ll = (uint64_t)l; _wasm_fnv_bytes(h, &ll, sizeof(ll));
        _wasm_fnv_bytes(h, s, l);
        return;
    }
    if (_looks_like_bytes(v)) {
        unsigned char tag = 3; _wasm_fnv_bytes(h, &tag, 1);
        int64_t l = _wasm_bytes_len(v);
        uint64_t ll = (uint64_t)l; _wasm_fnv_bytes(h, &ll, sizeof(ll));
        for (int64_t i = 0; i < l; i++) { unsigned char c = _wasm_bytes_at(v, i); _wasm_fnv_bytes(h, &c, 1); }
        return;
    }
    if (_looks_like_list(v)) {
        unsigned char tag = 7; _wasm_fnv_bytes(h, &tag, 1);
        int64_t *l = (int64_t*)(intptr_t)v; int64_t n = l[1];
        uint64_t nn = (uint64_t)n; _wasm_fnv_bytes(h, &nn, sizeof(nn));
        /* Value-tag track: elements hash under their recorded kind so a
           nested Bool no longer collides with Int (homogeneous lists report
           their single tag through _wasm_elem_kind_at, which keeps the
           legacy fingerprints for Int/Str/... lists stable). */
        for (int64_t i = 0; i < n; i++)
            _wasm_fp_accum_k(l[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(l, i), h);
        return;
    }
    if (_looks_like_empty_pack(v)) {
        unsigned char tag = 8; _wasm_fnv_bytes(h, &tag, 1);
        uint64_t z = 0; _wasm_fnv_bytes(h, &z, sizeof(z)); _wasm_fnv_bytes(h, &z, sizeof(z));
        return;
    }
    if (_looks_like_pack(v)) {
        unsigned char tag = 8; _wasm_fnv_bytes(h, &tag, 1);
        int64_t *p = (int64_t*)(intptr_t)v; int64_t n = p[0];
        uint64_t nn = (uint64_t)n; _wasm_fnv_bytes(h, &nn, sizeof(nn));
        uint64_t mix = 0;
        for (int64_t i = 0; i < n; i++) {
            uint64_t fh = 0xcbf29ce484222325ULL;
            int64_t nh = p[1 + i*3]; _wasm_fnv_bytes(&fh, &nh, sizeof(nh));
            _wasm_fp_accum(p[1 + i*3 + 2], &fh);
            mix ^= fh;
        }
        _wasm_fnv_bytes(h, &mix, sizeof(mix));
        return;
    }
    unsigned char tag = 0; _wasm_fnv_bytes(h, &tag, 1);
    _wasm_fnv_bytes(h, &v, sizeof(v));
}
static uint64_t _wasm_value_fingerprint(int64_t v) {
    uint64_t h = 0xcbf29ce484222325ULL; _wasm_fp_accum(v, &h); return h;
}
static int _wasm_elem_hashable_for_gate(int64_t v, uint32_t ekind); /* below */

static int _wasm_value_hashable(int64_t v) {
    /* F56: a sealed carrier is non-hashable so it never enters a hash-based
       dedup / Set bucket / fingerprint path (any of which would expose an
       equality oracle). Mirrors the interpreter's `is_hashable == false`. */
    if (_wasm_carrier_kind(v)) return 0;
    if (_is_wasm_hashmap(v) || _is_wasm_set(v) || _wasm_is_async_obj(v)) return 0;
    if (_looks_like_bytes(v)) return 1;
    if (_looks_like_string(v)) return 1;
    if (_looks_like_list(v)) {
        /* Value-tag track (native mirror): gate on each element's
           recorded kind — a nested list containing a known Float must
           take the linear path (its bit pattern classifies as a plain
           scalar otherwise). Kind-less elements keep the legacy
           structural classification. */
        int64_t *l = (int64_t*)(intptr_t)v; int64_t n = l[1];
        for (int64_t i = 0; i < n; i++)
            if (!_wasm_elem_hashable_for_gate(l[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(l, i)))
                return 0;
        return 1;
    }
    if (_looks_like_empty_pack(v)) return 1;
    if (_looks_like_pack(v)) {
        int64_t *p = (int64_t*)(intptr_t)v; int64_t n = p[0];
        for (int64_t i = 0; i < n; i++) if (!_wasm_value_hashable(p[1 + i*3 + 2])) return 0;
        return 1;
    }
    return 1; /* scalar */
}

/* Hashability of one element under its recorded kind, for the legacy
   whole-container gate (mirror of native taida_elem_hashable_for_gate): a
   known Float forces the linear path, a known scalar kind is hashable, a
   kind-less element falls back to the structural classifier (so untagged
   containers keep their historical behaviour bit-for-bit), and heap kinds
   recurse structurally. */
static int _wasm_elem_hashable_for_gate(int64_t v, uint32_t ekind) {
    uint32_t k = WASM_EKIND_KIND(ekind);
    switch (k) {
        case WASM_TAG_INT:
        case WASM_TAG_BOOL:
        case WASM_TAG_ENUM:
            return 1;
        case WASM_TAG_FLOAT:
            return 0;
        case WASM_EKIND_UNKNOWN:
            return _wasm_value_hashable(v);
        default:
            return _wasm_value_hashable(v);
    }
}

/* -- Value-tag track: kind-aware equality / fingerprint / membership -------
 * Mirror of native core.c. When a container carries a per-element kind
 * array, dedup and membership run on (value, ekind) pairs: Bool stops
 * colliding with Int, the Int↔Float crossing follows f64 semantics
 * regardless of magnitude, and same-ordinal variants of different enums
 * stay distinct (type id in the aux bits) while still crossing over to
 * Int(n)/Float(n) the way the interpreter's equality does. A side without
 * kind information (EKIND unknown) degrades to the legacy structural
 * comparison so old containers stay bit-compatible. (`_l2d` is the f64
 * bitcast helper defined with the float ops above.) */
static int _wasm_ekind_value_eq(int64_t a, uint32_t eka, int64_t b, uint32_t ekb);

/* Kind-aware list equality: element pairs compare under their recorded
   kinds. _wasm_elem_kind_at reports the single tag for homogeneous
   containers, so this is also the engine for plain nested lists. */
static int _wasm_list_struct_eq_k(int64_t *la, int64_t *lb) {
    int64_t n = la[1];
    if (n != lb[1]) return 0;
    for (int64_t i = 0; i < n; i++) {
        if (!_wasm_ekind_value_eq(la[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(la, i),
                                  lb[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(lb, i)))
            return 0;
    }
    return 1;
}

static int _wasm_ekind_value_eq(int64_t a, uint32_t eka, int64_t b, uint32_t ekb) {
    uint32_t ka = WASM_EKIND_KIND(eka), kb = WASM_EKIND_KIND(ekb);
    if (ka == WASM_EKIND_UNKNOWN || kb == WASM_EKIND_UNKNOWN)
        return _wasm_value_eq(a, b);
    switch (ka) {
        case WASM_TAG_INT:
            switch (kb) {
                case WASM_TAG_INT:
                case WASM_TAG_ENUM:  return a == b; /* Int(n) == ordinal n (interp crossing) */
                case WASM_TAG_FLOAT: return (double)a == _l2d(b);
                default: return 0; /* BOOL and every heap kind: unequal */
            }
        case WASM_TAG_BOOL:
            return kb == WASM_TAG_BOOL ? (a == b) : 0;
        case WASM_TAG_FLOAT:
            switch (kb) {
                case WASM_TAG_FLOAT: return _l2d(a) == _l2d(b); /* IEEE: NaN!=NaN, +0==-0 */
                case WASM_TAG_INT:
                case WASM_TAG_ENUM:  return _l2d(a) == (double)b;
                default: return 0;
            }
        case WASM_TAG_ENUM:
            switch (kb) {
                case WASM_TAG_ENUM:
                    /* Type id (aux) AND ordinal must match. Note the
                       deliberate non-transitivity inherited from the
                       interpreter: two same-ordinal enums of different
                       types are unequal to each other yet each equal to
                       the bare Int(ordinal). */
                    return eka == ekb && a == b;
                case WASM_TAG_INT:   return a == b;
                case WASM_TAG_FLOAT: return (double)a == _l2d(b);
                default: return 0;
            }
        case WASM_TAG_LIST:
            if (kb != WASM_TAG_LIST) return 0;
            if (a == b) return 1;
            if (!_looks_like_list(a) || !_looks_like_list(b)) return _wasm_value_eq(a, b);
            return _wasm_list_struct_eq_k((int64_t *)(intptr_t)a, (int64_t *)(intptr_t)b);
        default:
            /* Remaining heap kinds (STR / BYTES / PACK / SET / HMAP /
               CLOSURE): same-kind structural equality, cross-kind unequal. */
            if (ka != kb) return 0;
            return _wasm_value_eq(a, b);
    }
}

/* Kind-aware fingerprint. Int and Enum share tag 0 + ordinal — mirroring
   the interpreter's ValueKey, which deliberately leaves the enum type id
   out of the hash and lets equality confirmation distinguish same-ordinal
   enums — while Bool gets its own tag byte so it stops colliding with
   Int(0/1). Everything else reuses the structural accumulator. */
static void _wasm_fp_accum_k(int64_t v, uint32_t ekind, uint64_t *h) {
    switch (WASM_EKIND_KIND(ekind)) {
        case WASM_TAG_INT:
        case WASM_TAG_ENUM:
        case WASM_TAG_FLOAT: {
            /* Numeric kinds fingerprint their f64 image (mirror of
               native): equality crosses Int/Enum/Float by widening to
               f64, so the fingerprint must live in that domain too —
               past 2^53, Int(2^60) == Float(2^60) == Int(2^60 + 1).
               Same-kind Int collisions inside one f64 bucket are split
               by the equality confirmation; -0.0 canonicalises to +0.0;
               a NaN keeps its never-equal bits and inserts fresh. */
            double d = (WASM_EKIND_KIND(ekind) == WASM_TAG_FLOAT)
                ? _l2d(v)
                : (double)(int64_t)v;
            if (d == 0.0) d = 0.0; /* -0.0 -> +0.0 */
            unsigned char tag = 0; _wasm_fnv_bytes(h, &tag, 1);
            int64_t bits; __builtin_memcpy(&bits, &d, sizeof(bits));
            _wasm_fnv_bytes(h, &bits, sizeof(bits));
            break;
        }
        case WASM_TAG_BOOL: {
            unsigned char tag = 1; _wasm_fnv_bytes(h, &tag, 1);
            int64_t s = (int64_t)v; _wasm_fnv_bytes(h, &s, sizeof(s));
            break;
        }
        default:
            _wasm_fp_accum(v, h);
            break;
    }
}

/* Kind-aware hashability gate. Numeric kinds (Float included) are
   hashable: the fingerprint is the f64 image of the value — exactly the
   domain the kind-aware equality compares in. An unknown kind forces
   the linear path so legacy-fingerprint values and kind-fingerprint
   values never share one seen-set inconsistently. */
static int _wasm_ekind_hashable(int64_t v, uint32_t ekind) {
    switch (WASM_EKIND_KIND(ekind)) {
        case WASM_TAG_INT:
        case WASM_TAG_BOOL:
        case WASM_TAG_ENUM:
        case WASM_TAG_FLOAT:
            return 1;
        case WASM_EKIND_UNKNOWN:
            return 0;
        default:
            return _wasm_value_hashable(v);
    }
}

typedef struct { uint64_t *fps; int64_t *vals; unsigned char *used; int mask; } _wasm_seen;
static int _wasm_seen_init(_wasm_seen *s, int64_t hint) {
    int cap = 16; int64_t want = (hint > 0 ? hint : 1) * 2 + 1;
    while ((int64_t)cap < want) cap <<= 1;
    s->mask = cap - 1;
    s->fps  = (uint64_t*)wasm_alloc((unsigned int)((int)cap * (int)sizeof(uint64_t)));
    s->vals = (int64_t*)wasm_alloc((unsigned int)((int)cap * (int)sizeof(int64_t)));
    s->used = (unsigned char*)wasm_alloc((unsigned int)cap);
    if (!s->fps || !s->vals || !s->used) return 0;
    for (int i = 0; i < cap; i++) s->used[i] = 0;
    return 1;
}
static int _wasm_seen_add(_wasm_seen *s, int64_t v) {
    uint64_t fp = _wasm_value_fingerprint(v);
    int i = (int)(fp & (uint64_t)s->mask);
    for (;;) {
        if (!s->used[i]) { s->used[i] = 1; s->fps[i] = fp; s->vals[i] = v; return 1; }
        if (s->fps[i] == fp && _wasm_value_eq(s->vals[i], v)) return 0;
        i = (i + 1) & s->mask;
    }
}
static int _wasm_seen_contains(_wasm_seen *s, int64_t v) {
    uint64_t fp = _wasm_value_fingerprint(v);
    int i = (int)(fp & (uint64_t)s->mask);
    for (;;) {
        if (!s->used[i]) return 0;
        if (s->fps[i] == fp && _wasm_value_eq(s->vals[i], v)) return 1;
        i = (i + 1) & s->mask;
    }
}

/* Kind-aware seen-set (mirror of native taida_seen_k): entries carry
   (value, ekind) pairs so collision confirmation runs under
   _wasm_ekind_value_eq. Engaged only when every element kind passes
   _wasm_ekind_hashable — unknown kinds fall back to the linear
   pair-equality scan, matching the interpreter's all-or-nothing ValueKey
   gating (and its complexity profile). Bump-allocated, no free needed. */
typedef struct {
    uint64_t *fps; int64_t *vals; uint32_t *eks; unsigned char *used; int mask;
} _wasm_seen_k;
static int _wasm_seen_k_init(_wasm_seen_k *s, int64_t hint) {
    int cap = 16; int64_t want = (hint > 0 ? hint : 1) * 2 + 1;
    while ((int64_t)cap < want) cap <<= 1;
    s->mask = cap - 1;
    s->fps  = (uint64_t*)wasm_alloc((unsigned int)((int)cap * (int)sizeof(uint64_t)));
    s->vals = (int64_t*)wasm_alloc((unsigned int)((int)cap * (int)sizeof(int64_t)));
    s->eks  = (uint32_t*)wasm_alloc((unsigned int)((int)cap * (int)sizeof(uint32_t)));
    s->used = (unsigned char*)wasm_alloc((unsigned int)cap);
    if (!s->fps || !s->vals || !s->eks || !s->used) return 0;
    for (int i = 0; i < cap; i++) s->used[i] = 0;
    return 1;
}
static int _wasm_seen_k_add(_wasm_seen_k *s, int64_t v, uint32_t ek) {
    uint64_t fp = 0xcbf29ce484222325ULL;
    _wasm_fp_accum_k(v, ek, &fp);
    int i = (int)(fp & (uint64_t)s->mask);
    for (;;) {
        if (!s->used[i]) {
            s->used[i] = 1; s->fps[i] = fp; s->vals[i] = v; s->eks[i] = ek;
            return 1;
        }
        if (s->fps[i] == fp && _wasm_ekind_value_eq(s->vals[i], s->eks[i], v, ek)) return 0;
        i = (i + 1) & s->mask;
    }
}

static int _wasm_list_all_hashable(int64_t *list) {
    int64_t n = list[1];
    for (int64_t i = 0; i < n; i++) if (!_wasm_value_hashable(list[WASM_LIST_ELEMS + i])) return 0;
    return 1;
}

int64_t taida_set_has(int64_t set_ptr, int64_t item) {
    int64_t *set = (int64_t *)(intptr_t)set_ptr;
    int64_t len = set[1];
    for (int64_t i = 0; i < len; i++) {
        if (_wasm_value_eq(set[WASM_LIST_ELEMS + i], item)) return 1;
    }
    return 0;
}

/* Value-tag entry points (mirror of native core.c): the shared lowering
 * emits the EKIND-form entry points (kind in the low byte, enum type id
 * above, 0xFF = unknown). These now drive the per-element kind machinery
 * instead of collapsing onto the legacy tag latch. */
void taida_list_note_push_ekind(int64_t list_ptr, int64_t ekind) {
    _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)list_ptr, (uint32_t)ekind);
}

/* Kind-aware membership core: the set side reports each element's recorded
   kind, the probe item's kind comes from the caller (EKIND unknown degrades
   pairs to the legacy structural comparison, so a 0xFF probe is identical
   to the historical taida_set_has scan). */
static int64_t _wasm_set_contains_k(int64_t set_ptr, int64_t item, uint32_t ek) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        if (_wasm_ekind_value_eq(list[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(list, i), item, ek))
            return 1;
    }
    return 0;
}

/* Tagged entry point: codegen passes the probe argument's static kind so
   single-value membership follows the same pair semantics as
   construction-time dedup. An UNKNOWN entry degrades to the legacy
   structural behaviour. */
int64_t taida_set_has_tagged(int64_t set_ptr, int64_t item, int64_t ekind) {
    return _wasm_set_contains_k(set_ptr, item, (uint32_t)ekind);
}

/* Read back the kind recorded on a Lax pack's __value field (EKIND form;
   EKIND unknown when the input is not pack-shaped or the recorded tag is
   not a scalar kind). Unmolding call sites use this to carry a runtime
   "shadow kind" alongside the payload value. */
int64_t taida_lax_value_ekind(int64_t maybe_lax) {
    if (!_looks_like_pack(maybe_lax)) return (int64_t)WASM_EKIND_UNKNOWN;
    int64_t tag = taida_pack_get_field_tag(maybe_lax, WASM_HASH___VALUE);
    /* Only kind-stamped Lax packs reach this reader (shadow whitelist),
       so the recorded tag is trustworthy across the known range. */
    if (tag >= WASM_TAG_INT && tag <= WASM_TAG_BYTES && tag != WASM_TAG_ENUM)
        return tag;
    return (int64_t)WASM_EKIND_UNKNOWN;
}

/* Tagged polymorphic equality: when both operands carry a known kind the
   pair semantics engine decides (Bool≠Int, Int↔Float f64 crossing at any
   magnitude, ...); any unknown side falls back to the legacy poly
   comparison so untagged call sites keep their historical behaviour. */
int64_t taida_poly_eq_tagged(int64_t a, int64_t eka, int64_t b, int64_t ekb) {
    uint32_t ka = (uint32_t)eka & 0xFFu, kb = (uint32_t)ekb & 0xFFu;
    if (ka == WASM_EKIND_UNKNOWN || kb == WASM_EKIND_UNKNOWN) return taida_poly_eq(a, b);
    return _wasm_ekind_value_eq(a, (uint32_t)eka, b, (uint32_t)ekb) ? 1 : 0;
}

int64_t taida_poly_neq_tagged(int64_t a, int64_t eka, int64_t b, int64_t ekb) {
    return taida_poly_eq_tagged(a, eka, b, ekb) ? 0 : 1;
}

int64_t taida_set_add(int64_t set_ptr, int64_t item) {
    if (taida_set_has(set_ptr, item)) return set_ptr;
    /* Create a new set (copy elements) to preserve immutable semantics.
       In wasm-min's bump allocator, taida_list_push modifies in place when
       there's room, which would mutate the original set. */
    int64_t *old = (int64_t *)(intptr_t)set_ptr;
    int64_t len = old[1];
    int64_t new_set = taida_set_new();
    /* C23B-005: propagate elem_type_tag so the tag-aware renderer can
       still dispatch primitive elements correctly after an immutable
       `.add` clone. */
    taida_list_set_elem_tag(new_set, old[2]);
    for (int64_t i = 0; i < len; i++) {
        new_set = taida_list_push(new_set, old[WASM_LIST_ELEMS + i]);
    }
    new_set = taida_list_push(new_set, item);
    return new_set;
}

/* Tagged entry point (mirror of native taida_set_add_tagged): codegen
   passes the insert argument's static kind so insertion follows the same
   pair semantics as construction-time dedup. An UNKNOWN entry degrades to
   the legacy structural behaviour. */
int64_t taida_set_add_tagged(int64_t set_ptr, int64_t item, int64_t ekind) {
    uint32_t ek = (uint32_t)ekind;
    if (_wasm_set_contains_k(set_ptr, item, ek)) {
        return set_ptr;  /* already present under pair semantics */
    }
    /* Clone the set, projecting each element's recorded kind into the
       result, then append the new item under the caller's kind. */
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    int64_t new_set = taida_set_new();
    for (int64_t i = 0; i < len; i++) {
        uint32_t eki = _wasm_elem_kind_at(list, i);
        _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_set, eki);
        new_set = taida_list_push(new_set, list[WASM_LIST_ELEMS + i]);
    }
    _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_set, ek);
    new_set = taida_list_push(new_set, item);
    return new_set;
}

int64_t taida_set_from_list(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (_wasm_elem_slot_is_array(list[2])
        || _wasm_elem_tag_for_propagation(list) == WASM_TAG_FLOAT) {
        /* Kind-aware dedup over (value, recorded kind) pairs (mirror of
           native taida_set_from_list). The resulting Set keeps per-element
           kinds so later membership tests stay exact. The hash path engages
           only when every element kind is hashable — Float or unknown kinds
           fall back to the linear pair scan. */
        int64_t new_set = taida_set_new();
        _wasm_seen_k seen;
        int all_h = 1;
        for (int64_t i = 0; i < len && all_h; i++)
            all_h = _wasm_ekind_hashable(list[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(list, i));
        int use_hash = all_h && _wasm_seen_k_init(&seen, len);
        for (int64_t i = 0; i < len; i++) {
            int64_t item = list[WASM_LIST_ELEMS + i];
            uint32_t ek = _wasm_elem_kind_at(list, i);
            int dup;
            if (use_hash) {
                dup = !_wasm_seen_k_add(&seen, item, ek);
            } else {
                int64_t *ns = (int64_t *)(intptr_t)new_set;
                int64_t nlen = ns[1];
                dup = 0;
                for (int64_t j = 0; j < nlen; j++) {
                    if (_wasm_ekind_value_eq(ns[WASM_LIST_ELEMS + j], _wasm_elem_kind_at(ns, j), item, ek)) {
                        dup = 1;
                        break;
                    }
                }
            }
            if (!dup) {
                _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_set, ek);
                new_set = taida_list_push(new_set, item);
            }
        }
        return new_set;
    }
    int64_t set = taida_set_new();
    /* C23B-005: propagate the source list's elem_type_tag so the
       resulting Set's `list[2]` slot carries the primitive tag
       needed by the tag-aware renderer. Without this, `setOf(@[73088])`
       would leave the Set with `elem_tag = -1` (UNKNOWN) and the
       structural dispatcher would false-match the large Int against a
       collection sentinel. */
    taida_list_set_elem_tag(set, _wasm_elem_tag_for_propagation(list));
    _wasm_seen seen;
    int use_hash = _wasm_list_all_hashable(list) && _wasm_seen_init(&seen, len);
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        int dup = use_hash ? !_wasm_seen_add(&seen, item) : taida_set_has(set, item);
        if (!dup) set = taida_list_push(set, item);
    }
    return set;
}

/* W-4f: Set.remove(item) -> new Set without the item */
static int64_t taida_set_remove_k(int64_t set_ptr, int64_t item, uint32_t probe_ek) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    if (_wasm_elem_slot_is_array(list[2])) {
        /* Kind-aware removal (mirror of native): survivors keep their
           recorded kinds; the probe's kind comes from the caller. */
        int64_t new_set = taida_set_new();
        for (int64_t i = 0; i < len; i++) {
            uint32_t ek = _wasm_elem_kind_at(list, i);
            if (!_wasm_ekind_value_eq(list[WASM_LIST_ELEMS + i], ek, item, probe_ek)) {
                _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_set, ek);
                new_set = taida_list_push(new_set, list[WASM_LIST_ELEMS + i]);
            }
        }
        return new_set;
    }
    int64_t result = taida_set_new();
    /* F54 tier 2: the result holds source elements only, so the source
       elem tag stays honest (mirrors native taida_set_remove). */
    taida_list_set_elem_tag(result, _wasm_elem_tag_for_propagation(list));
    for (int64_t i = 0; i < len; i++) {
        if (!_wasm_ekind_value_eq(list[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(list, i), item, probe_ek)) {
            result = taida_list_push(result, list[WASM_LIST_ELEMS + i]);
        }
    }
    return result;
}

int64_t taida_set_remove(int64_t set_ptr, int64_t item) {
    return taida_set_remove_k(set_ptr, item, WASM_EKIND_UNKNOWN);
}

/* ── F54 tier 2: numeric-domain aware Set×Set comparison ──────────
 * Mirrors native core.c: containers carry one elem_type_tag (slot [2]
 * in the wasm layout) and raw Int / Bool / Float values are mutually
 * indistinguishable. When BOTH operands pin a scalar domain, compare in
 * the interpreter's numeric domain (Int↔Float lifts the int side; Bool
 * never equals a number; Float uses f64). Tags that do not pin a scalar
 * domain keep the structural engine. */
static int _wasm_tag_is_scalar_domain(int64_t tag) {
    return tag == WASM_TAG_INT || tag == WASM_TAG_BOOL || tag == WASM_TAG_FLOAT;
}

static int _wasm_set_numeric_cross(int64_t tag_a, int64_t tag_b) {
    if (!_wasm_tag_is_scalar_domain(tag_a) || !_wasm_tag_is_scalar_domain(tag_b)) return 0;
    if (tag_a == WASM_TAG_FLOAT || tag_b == WASM_TAG_FLOAT) return 1;
    return tag_a != tag_b;
}


/* W-4f: Set.union(other) -> new Set with all elements from both */
int64_t taida_set_union(int64_t set_a, int64_t set_b) {
    int64_t *a = (int64_t *)(intptr_t)set_a;
    int64_t *b = (int64_t *)(intptr_t)set_b;
    int64_t a_len = a[1];
    int64_t b_len = b[1];
    if (_wasm_elem_slot_is_array(a[2]) || _wasm_elem_slot_is_array(b[2])
        || _wasm_elem_tags_cross(a, b)
        || _wasm_set_numeric_cross(_wasm_elem_tag_for_propagation(a),
                                   _wasm_elem_tag_for_propagation(b))) {
        /* Kind-aware union over (value, kind) pairs (mirror of native).
           Numeric-crossing homogeneous sets (Float in play, or Int vs
           Bool) route here too: the kind-aware seen-set keeps them
           O(|a|+|b|); non-hashable kinds keep the exact linear scan. */
        int64_t result = taida_set_new();
        _wasm_seen_k seen;
        int all_h = 1;
        for (int64_t i = 0; i < a_len && all_h; i++)
            all_h = _wasm_ekind_hashable(a[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(a, i));
        for (int64_t i = 0; i < b_len && all_h; i++)
            all_h = _wasm_ekind_hashable(b[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(b, i));
        int use_hash = all_h && _wasm_seen_k_init(&seen, a_len + b_len);
        for (int64_t i = 0; i < a_len; i++) {
            uint32_t ek = _wasm_elem_kind_at(a, i);
            if (use_hash) _wasm_seen_k_add(&seen, a[WASM_LIST_ELEMS + i], ek);
            _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)result, ek);
            result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
        }
        for (int64_t i = 0; i < b_len; i++) {
            uint32_t ek = _wasm_elem_kind_at(b, i);
            int dup = use_hash ? !_wasm_seen_k_add(&seen, b[WASM_LIST_ELEMS + i], ek)
                               : _wasm_set_contains_k(result, b[WASM_LIST_ELEMS + i], ek);
            if (!dup) {
                _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)result, ek);
                result = taida_list_push(result, b[WASM_LIST_ELEMS + i]);
            }
        }
        return result;
    }
    int64_t tag_a = _wasm_elem_tag_for_propagation(a);
    int64_t tag_b = _wasm_elem_tag_for_propagation(b);
    /* Same-domain homogeneous fast path: the numeric-crossing cases
       (and every Float-bearing set) are owned by the kind-aware branch
       above (mirrors native taida_set_union). */
    int64_t result = taida_set_new();
    taida_list_set_elem_tag(result, tag_a);
    _wasm_seen seen;
    int use_hash = _wasm_list_all_hashable(a) && _wasm_list_all_hashable(b)
                   && _wasm_seen_init(&seen, a_len + b_len);
    for (int64_t i = 0; i < a_len; i++) {
        if (use_hash) _wasm_seen_add(&seen, a[WASM_LIST_ELEMS + i]);
        result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
    }
    for (int64_t i = 0; i < b_len; i++) {
        int dup = use_hash ? !_wasm_seen_add(&seen, b[WASM_LIST_ELEMS + i])
                           : (taida_set_has(result, b[WASM_LIST_ELEMS + i]) != 0);
        if (!dup) {
            /* F54 tier 2: every actually-added b element stamps its tag
               through the shared latch — BEFORE the push (the latch's
               materialise path indexes the element about to be pushed,
               so a post-push call would mis-index the kind array). An
               UNKNOWN-tagged result (empty-a union) promotes to tag_b;
               genuine cross-tag unions are owned by the kind-aware
               branch above (mirrors native taida_set_union). */
            taida_list_set_elem_tag(result, tag_b);
            result = taida_list_push(result, b[WASM_LIST_ELEMS + i]);
        }
    }
    return result;
}

/* W-4f: Set.intersect(other) -> new Set with common elements */
int64_t taida_set_intersect(int64_t set_a, int64_t set_b) {
    int64_t *a = (int64_t *)(intptr_t)set_a;
    int64_t a_len = a[1];
    int64_t *b = (int64_t *)(intptr_t)set_b;
    if (_wasm_elem_slot_is_array(a[2]) || _wasm_elem_slot_is_array(b[2])
        || _wasm_elem_tags_cross(a, b)
        || _wasm_set_numeric_cross(_wasm_elem_tag_for_propagation(a),
                                   _wasm_elem_tag_for_propagation(b))) {
        /* Kind-aware intersection (mirror of native; see union for the
           hash-path rationale). The b side seeds the kind-aware
           seen-set so each a-side membership test is O(1). The result
           holds a-side elements only, with their recorded kinds. */
        int64_t b_len = b[1];
        int64_t result = taida_set_new();
        _wasm_seen_k seen;
        int all_h = 1;
        for (int64_t i = 0; i < a_len && all_h; i++)
            all_h = _wasm_ekind_hashable(a[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(a, i));
        for (int64_t i = 0; i < b_len && all_h; i++)
            all_h = _wasm_ekind_hashable(b[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(b, i));
        int use_hash = all_h && _wasm_seen_k_init(&seen, b_len);
        if (use_hash)
            for (int64_t i = 0; i < b_len; i++)
                _wasm_seen_k_add(&seen, b[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(b, i));
        for (int64_t i = 0; i < a_len; i++) {
            uint32_t ek = _wasm_elem_kind_at(a, i);
            int in_b = use_hash ? !_wasm_seen_k_add(&seen, a[WASM_LIST_ELEMS + i], ek)
                                : _wasm_set_contains_k(set_b, a[WASM_LIST_ELEMS + i], ek);
            if (in_b) {
                _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)result, ek);
                result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
            }
        }
        return result;
    }
    int64_t tag_a = _wasm_elem_tag_for_propagation(a);
    int64_t result = taida_set_new();
    /* Same-domain homogeneous fast path; the result holds a-side
       elements only, so the a tag stays honest (mirrors native). */
    taida_list_set_elem_tag(result, tag_a);
    _wasm_seen seen;
    int use_hash = _wasm_list_all_hashable(a) && _wasm_list_all_hashable(b)
                   && _wasm_seen_init(&seen, b[1]);
    if (use_hash) for (int64_t i = 0; i < b[1]; i++) _wasm_seen_add(&seen, b[WASM_LIST_ELEMS + i]);
    for (int64_t i = 0; i < a_len; i++) {
        int in_b = use_hash ? _wasm_seen_contains(&seen, a[WASM_LIST_ELEMS + i])
                            : (taida_set_has(set_b, a[WASM_LIST_ELEMS + i]) != 0);
        if (in_b) result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
    }
    return result;
}

/* W-4f: Set.diff(other) -> new Set with elements in a but not in b */
int64_t taida_set_diff(int64_t set_a, int64_t set_b) {
    int64_t *a = (int64_t *)(intptr_t)set_a;
    int64_t a_len = a[1];
    int64_t *b = (int64_t *)(intptr_t)set_b;
    if (_wasm_elem_slot_is_array(a[2]) || _wasm_elem_slot_is_array(b[2])
        || _wasm_elem_tags_cross(a, b)
        || _wasm_set_numeric_cross(_wasm_elem_tag_for_propagation(a),
                                   _wasm_elem_tag_for_propagation(b))) {
        /* Kind-aware difference (mirror of native; see union for the
           hash-path rationale). */
        int64_t b_len = b[1];
        int64_t result = taida_set_new();
        _wasm_seen_k seen;
        int all_h = 1;
        for (int64_t i = 0; i < a_len && all_h; i++)
            all_h = _wasm_ekind_hashable(a[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(a, i));
        for (int64_t i = 0; i < b_len && all_h; i++)
            all_h = _wasm_ekind_hashable(b[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(b, i));
        int use_hash = all_h && _wasm_seen_k_init(&seen, b_len);
        if (use_hash)
            for (int64_t i = 0; i < b_len; i++)
                _wasm_seen_k_add(&seen, b[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(b, i));
        for (int64_t i = 0; i < a_len; i++) {
            uint32_t ek = _wasm_elem_kind_at(a, i);
            int in_b = use_hash ? !_wasm_seen_k_add(&seen, a[WASM_LIST_ELEMS + i], ek)
                                : _wasm_set_contains_k(set_b, a[WASM_LIST_ELEMS + i], ek);
            if (!in_b) {
                _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)result, ek);
                result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
            }
        }
        return result;
    }
    int64_t tag_a = _wasm_elem_tag_for_propagation(a);
    int64_t result = taida_set_new();
    /* Same-domain homogeneous fast path (mirrors native). */
    taida_list_set_elem_tag(result, tag_a);
    _wasm_seen seen;
    int use_hash = _wasm_list_all_hashable(a) && _wasm_list_all_hashable(b)
                   && _wasm_seen_init(&seen, b[1]);
    if (use_hash) for (int64_t i = 0; i < b[1]; i++) _wasm_seen_add(&seen, b[WASM_LIST_ELEMS + i]);
    for (int64_t i = 0; i < a_len; i++) {
        int in_b = use_hash ? _wasm_seen_contains(&seen, a[WASM_LIST_ELEMS + i])
                            : (taida_set_has(set_b, a[WASM_LIST_ELEMS + i]) != 0);
        if (!in_b) result = taida_list_push(result, a[WASM_LIST_ELEMS + i]);
    }
    return result;
}

/* W-4f: Set.toList() -> List copy */
int64_t taida_set_to_list(int64_t set_ptr) {
    int64_t *list = (int64_t *)(intptr_t)set_ptr;
    int64_t len = list[1];
    if (_wasm_elem_slot_is_array(list[2])) {
        /* Kind-aware clone: the list keeps the set's per-element kinds. */
        int64_t result = taida_list_new();
        for (int64_t i = 0; i < len; i++) {
            uint32_t ek = _wasm_elem_kind_at(list, i);
            _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)result, ek);
            result = taida_list_push(result, list[WASM_LIST_ELEMS + i]);
        }
        return result;
    }
    int64_t result = taida_list_new();
    /* Propagate the source's single elem tag so primitive rendering of the
       resulting list stays correct (mirrors the array-less native path). */
    taida_list_set_elem_tag(result, _wasm_elem_tag_for_propagation(list));
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

/* Tagged sibling (mirror of native): codegen supplies the probe argument's
 * static kind so Set membership follows pair semantics; HashMaps keep their
 * key-hash path (key kinds are a separate system). */
int64_t taida_collection_has_tagged(int64_t ptr, int64_t item, int64_t ekind) {
    if (_is_wasm_hashmap(ptr)) {
        int64_t key_hash = taida_value_hash(item);
        return taida_hashmap_has(ptr, key_hash, item);
    }
    return _wasm_set_contains_k(ptr, item, (uint32_t)ekind);
}

/* Tagged removal: the probe's static kind drives pair-semantics removal
   on Sets (native mirror); HashMaps keep their key-hash path. */
static int64_t taida_set_remove_k(int64_t set_ptr, int64_t item, uint32_t probe_ek); /* fwd */
int64_t taida_collection_remove_tagged(int64_t ptr, int64_t item, int64_t ekind) {
    if (_is_wasm_hashmap(ptr)) {
        int64_t clone = _wasm_hashmap_clone(ptr);
        int64_t key_hash = taida_value_hash(item);
        return taida_hashmap_remove(clone, key_hash, item);
    }
    return taida_set_remove_k(ptr, item, (uint32_t)ekind);
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
    /* Lax: isEmpty means has_value == false */
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
        char *r = _wasm_str_alloc(12);
        _wf_memcpy(r, "HashMap({})", 11);
        r[11] = '\0';
        return (int64_t)r;
    }

    /* Build "HashMap({k: v, k: v, ...})" */
    int buf_size = 256;
    char *buf = _wasm_str_alloc((unsigned int)buf_size);
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
                char *new_buf = _wasm_str_alloc((unsigned int)buf_size);
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
        char *r = _wasm_str_alloc(8);
        _wf_memcpy(r, "Set({})", 8);
        return (int64_t)r;
    }
    int buf_size = 128;
    char *buf = _wasm_str_alloc((unsigned int)buf_size);
    _wf_memcpy(buf, "Set({", 5);
    int pos = 5;
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) { buf[pos++] = ','; buf[pos++] = ' '; }
        const char *vs = (const char *)(intptr_t)taida_polymorphic_to_string(list[WASM_LIST_ELEMS + i]);
        int vl = _wf_strlen(vs);
        while (pos + vl + 10 > buf_size) {
            buf_size *= 2;
            char *new_buf = _wasm_str_alloc((unsigned int)buf_size);
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
        WSTR("IOError"),
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
    if (p[0] == 4 || p[0] == 5) return taida_pack_get_idx(obj, 0); /* monadic has_value */
    return 0;
}

/* Polymorphic .contains() */
int64_t taida_polymorphic_contains(int64_t obj, int64_t needle) {
    if (obj == 0) return 0;
    /* F56: a sealed carrier has no methods (checker [E1533]/[E1536], interpreter
       rejects); guard the receiver so a `--no-check` `secret.contains(x)` cannot
       misread the carrier pack as a list. */
    if (_wasm_carrier_kind(obj)) return 0;
    if (_is_wasm_hashmap(obj)) return taida_hashmap_has(obj, taida_value_hash(needle), needle);
    if (_is_wasm_set(obj)) return taida_set_has(obj, needle);
    if (_looks_like_list(obj)) return taida_list_contains(obj, needle);
    if (_looks_like_string(obj)) return taida_str_contains(obj, needle);
    return 0;
}

/* Polymorphic .indexOf() */
int64_t taida_polymorphic_index_of(int64_t obj, int64_t needle) {
    if (obj == 0) return -1;
    if (_wasm_carrier_kind(obj)) return -1; /* F56: sealed receiver — see contains. */
    if (_looks_like_list(obj)) return taida_list_index_of(obj, needle);
    if (_looks_like_string(obj)) return taida_str_index_of(obj, needle);
    return -1;
}

/* Polymorphic .lastIndexOf() */
int64_t taida_polymorphic_last_index_of(int64_t obj, int64_t needle) {
    if (obj == 0) return -1;
    if (_wasm_carrier_kind(obj)) return -1; /* F56: sealed receiver — see contains. */
    if (_looks_like_list(obj)) return taida_list_last_index_of(obj, needle);
    if (_looks_like_string(obj)) return taida_str_last_index_of(obj, needle);
    return -1;
}

/* E32B-022 (Lock-N): Lax[Int]-returning siblings of the legacy `-1`
 * sentinel `*indexOf*` helpers.  Same probe semantics as the raw
 * helpers above; the `-1` sentinel is rewritten to a has_value=false
 * Lax with default 0 (PHILOSOPHY I — no magic-value sentinels).  */
extern int64_t taida_list_find_index(int64_t list_ptr, int64_t fn_ptr);
static int64_t _taida_int_lax_found_wasm(int64_t idx) { return taida_lax_new(idx, 0); }
static int64_t _taida_int_lax_missing_wasm(void) { return taida_lax_empty(0); }

int64_t taida_polymorphic_index_of_lax(int64_t obj, int64_t needle) {
    int64_t raw = taida_polymorphic_index_of(obj, needle);
    if (raw < 0) return _taida_int_lax_missing_wasm();
    return _taida_int_lax_found_wasm(raw);
}

int64_t taida_polymorphic_last_index_of_lax(int64_t obj, int64_t needle) {
    int64_t raw = taida_polymorphic_last_index_of(obj, needle);
    if (raw < 0) return _taida_int_lax_missing_wasm();
    return _taida_int_lax_found_wasm(raw);
}

int64_t taida_list_find_index_lax(int64_t list_ptr, int64_t fn_ptr) {
    int64_t raw = taida_list_find_index(list_ptr, fn_ptr);
    if (raw < 0) return _taida_int_lax_missing_wasm();
    return _taida_int_lax_found_wasm(raw);
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
