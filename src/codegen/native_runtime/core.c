#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include <math.h>
#include <dirent.h>
#include <sys/stat.h>
#include <unistd.h>
#include <errno.h>
#include <fcntl.h>
#include <time.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <stdint.h>
#include <inttypes.h>
#include <limits.h>
#include <pthread.h>

// FL-9: Safe realloc wrapper — aborts on NULL with a diagnostic message.
// Usage: TAIDA_REALLOC(ptr, new_size, context_label)
// ptr is reassigned in-place; on failure, prints OOM message and exits.
#define TAIDA_REALLOC(ptr, size, label) do { \
    void *_tmp = realloc((ptr), (size)); \
    if (!_tmp) { fprintf(stderr, "taida: out of memory (%s)\n", (label)); exit(1); } \
    (ptr) = _tmp; \
} while(0)

// Safe malloc wrapper — aborts on NULL with a diagnostic message.
// Usage: void *p = TAIDA_MALLOC(size, context_label);
// Returns the allocated pointer; on failure, prints OOM message and exits.
static inline void *taida_safe_malloc(size_t size, const char *label) {
    // R-02: malloc(0) is implementation-defined (may return NULL or a unique
    // pointer). Normalize to 1 so that the NULL check below reliably detects
    // real OOM rather than a valid zero-size allocation.
    if (size == 0) size = 1;
    void *p = malloc(size);
    if (!p) { fprintf(stderr, "taida: out of memory (%s)\n", label); exit(1); }
    return p;
}
#define TAIDA_MALLOC(size, label) taida_safe_malloc((size), (label))

// M-00: Safe arithmetic helpers for allocation size calculations.
// These detect overflow BEFORE passing the result to malloc, preventing
// heap corruption from silently-wrapped size_t values.

// Multiply two size_t values; abort on overflow.
static inline size_t taida_safe_mul(size_t a, size_t b, const char *label) {
    if (a != 0 && b > SIZE_MAX / a) {
        fprintf(stderr, "taida: integer overflow in size calculation (%s): %zu * %zu\n", label, a, b);
        exit(1);
    }
    return a * b;
}

// Add two size_t values; abort on overflow.
static inline size_t taida_safe_add(size_t a, size_t b, const char *label) {
    if (a > SIZE_MAX - b) {
        fprintf(stderr, "taida: integer overflow in size calculation (%s): %zu + %zu\n", label, a, b);
        exit(1);
    }
    return a + b;
}

// ---------------------------------------------------------------------------
// W-0d/W-0f: ABI 正規化 — 値型・ポインタ型・関数ポインタ型の typedef
//
// Native (LP64): taida_val = int64_t (8B), taida_ptr = intptr_t (8B)
// WASM32 (ILP32): taida_val = int64_t (8B), taida_ptr = intptr_t (4B)
//
// W-0 では Native のみ。LP64 環境で taida_val = int64_t = intptr_t = 64-bit なので
// 動作は一切変わらない。WASM32 対応時に taida_ptr が 32-bit になる。
//
// W-0f: Forward declarations と主要関数定義（pack, closure, retain/release, list）に
// taida_ptr/taida_fn_ptr を適用済み。残りの関数定義は LP64 では taida_val と同一型
// のため動作に影響なし。WASM32 runtime は別ファイル（runtime_core_wasm.c）になる
// ため、native_runtime.c の完全移行は必須ではない。
// ---------------------------------------------------------------------------
typedef int64_t   taida_val;     // 整数値 (Int, Bool, Hash, tag, count, index, dummy)
typedef intptr_t  taida_ptr;     // ヒープポインタ (Str, Pack, List, HashMap, Set, ...)
typedef intptr_t  taida_fn_ptr;  // 関数ポインタ

// Taida Magic Numbers for pointer tagging and type safety
#define TAIDA_MAGIC_MASK  0xFFFFFFFFFFFFFF00LL
#define TAIDA_RC_MASK     0x00000000000000FFLL
#define TAIDA_LIST_MAGIC  0x544149444C535400LL // "TAIDLST\0"
#define TAIDA_PACK_MAGIC  0x5441494450414B00LL // "TAIDPAK\0"
#define TAIDA_STR_MAGIC   0x5441494453545200LL // "TAIDSTR\0"
#define TAIDA_HMAP_MAGIC  0x54414944484D4100LL // "TAIDHMA\0"
#define TAIDA_SET_MAGIC   0x5441494453455400LL // "TAIDSET\0"
#define TAIDA_ASYNC_MAGIC 0x5441494441535900LL // "TAIDASY\0"
#define TAIDA_BYTES_MAGIC 0x5441494442595400LL // "TAIDBYT\0"
#define TAIDA_CLOSURE_MAGIC 0x54414944434C4F00LL // "TAIDCLO\0"

// Type tags for BuchiPack field values (A-4a)
#define TAIDA_TAG_INT     0
#define TAIDA_TAG_FLOAT   1
#define TAIDA_TAG_BOOL    2
#define TAIDA_TAG_STR     3
#define TAIDA_TAG_PACK    4
#define TAIDA_TAG_LIST    5
#define TAIDA_TAG_CLOSURE 6
#define TAIDA_TAG_HMAP    7
#define TAIDA_TAG_SET     8
#define TAIDA_TAG_UNKNOWN -1
// C23B-007 (2026-04-22): HETEROGENEOUS sentinel distinct from UNKNOWN.
// Used by taida_list_set_elem_tag / taida_hashmap_set_value_tag after a
// type-conflict downgrade so a subsequent .set()/.push() with a fresh
// primitive tag cannot re-promote a mixed container back to a single
// primitive tag. Mirror of WASM_TAG_HETEROGENEOUS. Retain/release treats
// HETEROGENEOUS the same as UNKNOWN (the "leak rather than crash"
// conservative path) — the container's lifetime memory accounting was
// already leaky for unknown-tag containers before this change.
#define TAIDA_TAG_HETEROGENEOUS -2

// ============================================================================
// NO-4: Native Ownership Rules (runtime helper 監査ルール)
// See docs/NATIVE_OWNERSHIP_AUDIT.md for the full specification.
//
// RULE 1 — No raw store: aggregate objects (Pack, List, HashMap, Set, Async)
//   MUST use retain + tag helper paths when storing heap children.
//   Direct pointer writes without retain/tag are PROHIBITED.
//   Helpers: taida_retain_and_tag_field (Pack), taida_list_elem_retain (List),
//            taida_str_retain (String), taida_hashmap_set (HashMap),
//            taida_set_add (Set).
//
// RULE 2 — Display/toString temporaries: every heap string created by
//   taida_value_to_display_string / taida_value_to_debug_string / *_to_string
//   MUST be released by the caller via taida_str_release after use.
//   Pattern: taida_val tmp = taida_value_to_debug_string(val); use(tmp); taida_str_release(tmp);
//
// RULE 3 — New runtime helper checklist (see NATIVE_OWNERSHIP_AUDIT.md):
//   (a) Does it allocate heap objects? -> Set magic header + initial rc=1.
//   (b) Does it store heap children? -> Use retain + tag helper, never raw store.
//   (c) Does it return heap objects? -> Document ownership transfer (move vs borrow).
//   (d) Does it create temporary heap values? -> Release after use.
//   (e) Does it copy from existing containers? -> Retain each copied child.
//   (f) Add regression test in tests/native_compile.rs.
// ============================================================================

// HashMap layout macros (needed by taida_release before HashMap section)
#define HM_HEADER 4  // number of header slots before entries
#define TAIDA_HASHMAP_TOMBSTONE_HASH ((taida_val)1)
// C23B-008 (2026-04-22): HashMap insertion-order side-index.
// After the `cap * 3` entry slots we append:
//   [next_ord, order_array[cap]]
// where `next_ord` is a monotonic ordinal counter and `order_array[i]`
// stores the bucket slot of the i-th insertion (or -1 for a hole left
// by a subsequent .remove()). Display / iteration helpers walk
// `order_array[0..next_ord]` so Native ordering matches interpreter /
// JS insertion-order semantics (interpreter stores HashMap as
// BuchiPack(__entries <= @[(k,v), ...]) — a Vec<(name, value)> that
// preserves insertion order, `src/interpreter/methods.rs:674-702`).
//
// Layout (native, no trailing magic — cap=16 example):
//   [0]   refcount+magic
//   [1]   capacity (= 16)
//   [2]   length
//   [3]   value_type_tag
//   [4..52)  entries (16 entries * 3 slots = 48)
//   [52]  next_ord
//   [53..69)  order_array (16 slots)
// Total: HM_HEADER + cap*3 + 1 + cap
#define TAIDA_HM_ORD_HEADER_SLOT(cap) ((HM_HEADER) + (cap) * 3)
#define TAIDA_HM_ORD_SLOT(cap, i)     (TAIDA_HM_ORD_HEADER_SLOT(cap) + 1 + (i))
#define TAIDA_HM_TOTAL_SLOTS(cap)     ((HM_HEADER) + (cap) * 3 + 1 + (cap))
#define HM_SLOT_EMPTY(h, k)     ((h) == 0 && (k) == 0)
#define HM_SLOT_TOMBSTONE(h, k) ((h) == TAIDA_HASHMAP_TOMBSTONE_HASH && (k) == 0)
#define HM_SLOT_OCCUPIED(h, k)  (!HM_SLOT_EMPTY(h, k) && !HM_SLOT_TOMBSTONE(h, k))

#define TAIDA_IS_LIST(ptr)  (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_LIST_MAGIC)
#define TAIDA_IS_PACK(ptr)  (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_PACK_MAGIC)
#define TAIDA_IS_STR(ptr)   (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_STR_MAGIC)
#define TAIDA_IS_HMAP(ptr)  (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_HMAP_MAGIC)
#define TAIDA_IS_SET(ptr)   (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_SET_MAGIC)
#define TAIDA_IS_ASYNC(ptr) (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_ASYNC_MAGIC)
#define TAIDA_IS_BYTES(ptr) (taida_ptr_is_readable(ptr, 8) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_BYTES_MAGIC)
#define TAIDA_IS_CLOSURE(ptr) (taida_ptr_is_readable(ptr, sizeof(taida_val) * 3) && (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) == TAIDA_CLOSURE_MAGIC)

// NB-31: Callable check — closure OR readable non-heap-tagged pointer (function pointer).
// Catches: integers (not readable), data objects (readable + heap magic), null/zero.
// Cannot catch all adversarial cases (e.g., aligned readable int in code segment range)
// but prevents the common httpServe(port, 42) / httpServe(port, 50000) crash paths.
#define TAIDA_IS_CALLABLE(val) _taida_is_callable_impl(val)

#define TAIDA_GET_RC(ptr)      (((taida_val*)ptr)[0] & TAIDA_RC_MASK)
#define TAIDA_SET_RC(ptr, rc)  (((taida_val*)ptr)[0] = (((taida_val*)ptr)[0] & TAIDA_MAGIC_MASK) | ((rc) & TAIDA_RC_MASK))
// NB2-7: Thread-safe RC increment/decrement using CAS loop.
// Prevents data races when multiple worker threads retain/release objects concurrently.
#define TAIDA_INC_RC(ptr) do { \
    volatile taida_val *_hdr = (volatile taida_val*)(ptr); \
    taida_val _old, _new; \
    do { \
        _old = __atomic_load_n(_hdr, __ATOMIC_RELAXED); \
        taida_val _rc = _old & TAIDA_RC_MASK; \
        if (_rc >= 255) break; \
        _new = (_old & TAIDA_MAGIC_MASK) | ((_rc + 1) & TAIDA_RC_MASK); \
    } while (!__atomic_compare_exchange_n(_hdr, &_old, _new, 0, __ATOMIC_ACQ_REL, __ATOMIC_RELAXED)); \
} while (0)
#define TAIDA_DEC_RC(ptr) do { \
    volatile taida_val *_hdr = (volatile taida_val*)(ptr); \
    taida_val _old, _new; \
    do { \
        _old = __atomic_load_n(_hdr, __ATOMIC_RELAXED); \
        taida_val _rc = _old & TAIDA_RC_MASK; \
        if (_rc == 0) break; \
        _new = (_old & TAIDA_MAGIC_MASK) | ((_rc - 1) & TAIDA_RC_MASK); \
    } while (!__atomic_compare_exchange_n(_hdr, &_old, _new, 0, __ATOMIC_ACQ_REL, __ATOMIC_RELAXED)); \
} while (0)

extern taida_val _taida_main(void);
static int taida_cli_argc = 0;
static char **taida_cli_argv = NULL;

// Forward declarations
// W-0f F-3: taida_ptr = heap pointer, taida_fn_ptr = function pointer, taida_val = value
taida_val taida_retain(taida_ptr ptr);
taida_ptr taida_async_ok(taida_val value);
taida_ptr taida_async_ok_tagged(taida_val value, taida_val value_tag);
taida_ptr taida_async_err(taida_ptr error);
void taida_async_set_value_tag(taida_ptr async_ptr, taida_val tag);
taida_ptr taida_pack_new(taida_val field_count);
taida_ptr taida_pack_set(taida_ptr pack_ptr, taida_val field_idx, taida_val value);
taida_ptr taida_pack_set_hash(taida_ptr pack_ptr, taida_val index, taida_val hash);
taida_ptr taida_pack_set_tag(taida_ptr pack_ptr, taida_val index, taida_val tag);
taida_val taida_pack_get_idx(taida_ptr pack_ptr, taida_val index);
taida_val taida_throw(taida_ptr error_val);
taida_ptr taida_lax_new(taida_val value, taida_val default_value);
taida_ptr taida_lax_empty(taida_val default_value);
taida_val taida_lax_unmold(taida_ptr lax_ptr);
taida_val taida_async_unmold(taida_ptr async_ptr);
taida_ptr taida_async_all(taida_ptr list_ptr);
taida_ptr taida_async_race(taida_ptr list_ptr);
taida_ptr taida_async_map(taida_ptr async_ptr, taida_fn_ptr fn_ptr);
taida_val taida_async_get_or_default(taida_ptr async_ptr, taida_val def);
taida_ptr taida_async_spawn(taida_fn_ptr fn_ptr, taida_val arg);
taida_ptr taida_async_cancel(taida_ptr async_ptr);
static void taida_async_join(taida_ptr async_ptr);
static taida_ptr taida_async_to_string(taida_ptr async_ptr);
taida_ptr taida_str_from_int(taida_val v);
taida_ptr taida_str_from_float(double v);
taida_ptr taida_str_from_bool(taida_val v);
taida_val taida_is_closure_value(taida_val ptr);
taida_ptr taida_json_schema_cast(taida_ptr raw_ptr, taida_ptr schema_ptr);
taida_ptr taida_list_new(void);
taida_ptr taida_list_push(taida_ptr list_ptr, taida_val item);
taida_ptr taida_result_create(taida_val value, taida_ptr throw_val, taida_fn_ptr predicate);
taida_ptr taida_result_map(taida_ptr result, taida_fn_ptr fn_ptr);
taida_ptr taida_result_flat_map(taida_ptr result, taida_fn_ptr fn_ptr);
taida_ptr taida_result_map_error(taida_ptr result, taida_fn_ptr fn_ptr);
taida_val taida_result_get_or_throw(taida_ptr result);
taida_ptr taida_result_to_string(taida_ptr result);
taida_val taida_list_length(taida_ptr list_ptr);
taida_ptr taida_lax_map(taida_ptr lax_ptr, taida_fn_ptr fn_ptr);
taida_ptr taida_lax_flat_map(taida_ptr lax_ptr, taida_fn_ptr fn_ptr);
taida_ptr taida_gorillax_new(taida_val value);
taida_ptr taida_molten_new(void);
taida_ptr taida_stub_new(taida_ptr message);
taida_ptr taida_todo_new(taida_ptr id, taida_ptr task, taida_ptr sol, taida_ptr unm);
taida_ptr taida_cage_apply(taida_val cage_value, taida_fn_ptr fn_ptr);
taida_ptr taida_gorillax_err(taida_ptr error);
taida_val taida_gorillax_unmold(taida_ptr ptr);
taida_ptr taida_gorillax_relax(taida_ptr ptr);
taida_ptr taida_gorillax_to_string(taida_ptr ptr);
taida_val taida_relaxed_gorillax_unmold(taida_ptr ptr);
taida_ptr taida_relaxed_gorillax_to_string(taida_ptr ptr);
taida_ptr taida_list_map(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
taida_val taida_list_is_empty(taida_ptr list_ptr);
taida_ptr taida_hashmap_new(void);
void taida_hashmap_set_value_tag(taida_ptr hm_ptr, taida_val tag);
taida_ptr taida_hashmap_set(taida_ptr hm_ptr, taida_val key_hash, taida_ptr key_ptr, taida_val value);
taida_val taida_hashmap_get(taida_ptr hm_ptr, taida_val key_hash, taida_ptr key_ptr);
taida_val taida_hashmap_has(taida_ptr hm_ptr, taida_val key_hash, taida_ptr key_ptr);
taida_ptr taida_hashmap_remove(taida_ptr hm_ptr, taida_val key_hash, taida_ptr key_ptr);
taida_ptr taida_hashmap_remove_immut(taida_ptr hm_ptr, taida_val key_hash, taida_ptr key_ptr);
taida_val taida_hashmap_is_empty(taida_ptr hm_ptr);
taida_ptr taida_hashmap_get_lax(taida_ptr hm_ptr, taida_val key_hash, taida_ptr key_ptr);
taida_ptr taida_hashmap_to_string(taida_ptr hm_ptr);
taida_val taida_str_hash(taida_ptr str_ptr);
taida_ptr taida_str_concat(const char* a, const char* b);
// Heap string helpers (hidden header: [magic+rc, len, bytes...\0])
static char* taida_str_alloc(size_t len);
static char* taida_str_new_copy(const char* src);
void  taida_str_retain(taida_ptr ptr);
static void  taida_str_release(taida_ptr ptr);
// List element retain/release helpers (based on elem_type_tag)
static void taida_list_elem_retain(taida_val elem, taida_val elem_tag);
static void taida_list_elem_release(taida_val elem, taida_val elem_tag);
taida_ptr taida_list_get(taida_ptr list_ptr, taida_val index);
taida_val taida_set_has(taida_ptr set_ptr, taida_val item);
taida_ptr taida_set_remove(taida_ptr set_ptr, taida_val item);
taida_ptr taida_set_add(taida_ptr set_ptr, taida_val item);
void taida_set_set_elem_tag(taida_ptr set_ptr, taida_val tag);
// Str state check methods
taida_val taida_str_contains(const char* s, const char* sub);
taida_val taida_str_starts_with(const char* s, const char* prefix);
taida_val taida_str_ends_with(const char* s, const char* suffix);
taida_val taida_str_last_index_of(const char* s, const char* sub);
taida_ptr taida_str_get(const char* s, taida_val idx);
taida_ptr taida_str_chars(const char* s);
// List state check methods
taida_val taida_list_last_index_of(taida_ptr list_ptr, taida_val item);
taida_val taida_list_any(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
taida_val taida_list_all(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
taida_val taida_list_none(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
// Prelude functions
taida_ptr taida_io_stdin(taida_ptr prompt_ptr);
// C20-2: UTF-8-aware Async[Lax[Str]] line editor. See implementation
// near the bottom of this file for the full prologue.
taida_ptr taida_io_stdin_line(taida_ptr prompt_ptr);
taida_ptr taida_sha256(taida_val value);
taida_val taida_time_now_ms(void);
taida_val taida_time_sleep(taida_val ms);
taida_ptr taida_json_encode(taida_val val);
taida_ptr taida_json_pretty(taida_val val);
taida_val taida_register_field_name(taida_val hash, taida_ptr name_ptr);
taida_val taida_register_field_type(taida_val hash, taida_ptr name_ptr, taida_val type_tag);
/// C18-2: Register `field_hash` as an Enum-typed field. `variants_ptr`
/// points to a comma-separated list of variant names, used by
/// `json_serialize_pack_fields` to emit variant-name Str in jsonEncode.
taida_val taida_register_field_enum(taida_val hash, taida_ptr name_ptr, taida_ptr variants_ptr);
/// C18B-003 fix: Register a per-pack enum descriptor so two packs that
/// share the same field name (e.g. `state`) but hold different enums
/// (`BuildState`, `RunState`) no longer collide in the global field
/// registry. Keyed by `(pack_ptr, field_hash)`; looked up at
/// `json_serialize_pack_fields` time before falling back to the global
/// descriptor.
taida_val taida_register_pack_field_enum(taida_ptr pack_ptr, taida_val field_hash, taida_ptr variants_ptr);
static const char* taida_lookup_field_name(taida_val hash);
static int taida_lookup_field_type(taida_val hash);
static taida_val taida_is_hashmap(taida_val ptr);
// NO-1: HashMap ownership helpers (forward declarations)
static void taida_hashmap_val_retain(taida_val val, taida_val val_tag);
static void taida_hashmap_val_release(taida_val val, taida_val val_tag);
static void taida_hashmap_key_retain(taida_val key);
static void taida_hashmap_key_release(taida_val key);
static taida_val taida_is_set(taida_val ptr);
static int taida_is_list(taida_val ptr);
static int taida_is_bytes(taida_val ptr);
static int taida_is_buchi_pack(taida_val ptr);
static int taida_is_stream_pack(taida_val ptr);
static taida_val taida_stream_to_display_string(taida_val stream_ptr);
static taida_ptr taida_lax_to_string(taida_ptr lax_ptr);
static taida_ptr taida_set_to_string(taida_ptr set_ptr);
static int taida_has_magic_header(taida_val tag);
static taida_val taida_make_error(const char *error_type, const char *error_msg);
static const char *taida_os_error_kind(int err_code, const char *err_msg);
static taida_val taida_make_io_error(int err_code, const char *err_msg);
static int taida_ptr_is_readable(taida_val ptr, size_t bytes);
static int taida_read_cstr_len_safe(const char *s, size_t max_len, size_t *out_len);
static taida_val taida_value_to_display_string(taida_val val);
static taida_val taida_value_to_debug_string(taida_val val);
static taida_val taida_throw_to_display_string(taida_val throw_val);
// C26B-011 (Phase 11): forward-declare `taida_float_to_str` so
// `taida_debug_float` (defined early in the file) can route through
// the same Rust-f64::Display formatter used everywhere else.
taida_val taida_float_to_str(double a);
// C23-2: generic Str[x]() entry — forward declare the stdout-display helper so
// that `taida_str_mold_any` (defined near the other str-mold helpers up in the
// file) can route through the full-form BuchiPack rendering used by stdout.
taida_val taida_stdout_display_string(taida_val obj);
taida_val taida_typeof(taida_val val, taida_val tag);
taida_val taida_polymorphic_contains(taida_val obj, taida_val needle);
taida_val taida_polymorphic_index_of(taida_val obj, taida_val needle);
taida_val taida_polymorphic_last_index_of(taida_val obj, taida_val needle);
// C12-6c: Regex polymorphic dispatchers + constructor.
taida_val taida_regex_new(const char *pattern_s, const char *flags_s);
taida_val taida_str_split_poly(const char *s, taida_val sep);
taida_val taida_str_replace_first_poly(const char *s, taida_val target, const char *rep);
taida_val taida_str_replace_poly(const char *s, taida_val target, const char *rep);
taida_val taida_str_match_regex(const char *s, taida_val regex_pack);
taida_val taida_str_search_regex(const char *s, taida_val regex_pack);
static taida_val taida_bytes_new_filled(taida_val len, unsigned char fill);
static taida_val taida_bytes_from_raw(const unsigned char *data, taida_val len);
static taida_val taida_bytes_clone(taida_val bytes_ptr);
static taida_val taida_bytes_len(taida_val bytes_ptr);
static taida_val taida_bytes_default_value(void);
static taida_val taida_bytes_get_lax(taida_val bytes_ptr, taida_val index);
static int taida_bytes_cursor_unpack(taida_val cursor_ptr, taida_val *bytes_out, taida_val *offset_out);
static taida_val taida_bytes_cursor_step(taida_val value, taida_val cursor);
static double _l2d(taida_val v);
static taida_val _d2l(double v);
// OS package forward declarations
taida_val taida_os_read(taida_val path_ptr);
taida_val taida_os_read_bytes(taida_val path_ptr);
taida_val taida_os_list_dir(taida_val path_ptr);
taida_val taida_os_stat(taida_val path_ptr);
taida_val taida_os_exists(taida_val path_ptr);
taida_val taida_os_env_var(taida_val name_ptr);
taida_val taida_os_write_file(taida_val path_ptr, taida_val content_ptr);
taida_val taida_os_write_bytes(taida_val path_ptr, taida_val content_ptr);
taida_val taida_os_append_file(taida_val path_ptr, taida_val content_ptr);
taida_val taida_os_remove(taida_val path_ptr);
taida_val taida_os_create_dir(taida_val path_ptr);
taida_val taida_os_rename(taida_val from_ptr, taida_val to_ptr);
taida_val taida_os_run(taida_val program_ptr, taida_val args_list_ptr);
taida_val taida_os_exec_shell(taida_val command_ptr);
// C19: interactive TTY-passthrough variants
taida_val taida_os_run_interactive(taida_val program_ptr, taida_val args_list_ptr);
taida_val taida_os_exec_shell_interactive(taida_val command_ptr);
taida_val taida_os_all_env(void);
taida_val taida_os_argv(void);
// OS package Phase 2: async APIs
taida_val taida_os_read_async(taida_val path_ptr);
taida_val taida_os_http_get(taida_val url_ptr);
taida_val taida_os_http_post(taida_val url_ptr, taida_val body_ptr);
taida_val taida_os_http_request(taida_val method_ptr, taida_val url_ptr, taida_val headers_ptr, taida_val body_ptr);
taida_val taida_os_dns_resolve(taida_val host_ptr, taida_val timeout_ms);
taida_val taida_os_tcp_connect(taida_val host_ptr, taida_val port, taida_val timeout_ms);
taida_val taida_os_tcp_listen(taida_val port, taida_val timeout_ms);
taida_val taida_os_tcp_accept(taida_val listener_fd, taida_val timeout_ms);
taida_val taida_os_socket_send(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms);
taida_val taida_os_socket_send_all(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms);
taida_val taida_os_socket_recv(taida_val socket_fd, taida_val timeout_ms);
taida_val taida_os_socket_send_bytes(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms);
taida_val taida_os_socket_recv_bytes(taida_val socket_fd, taida_val timeout_ms);
taida_val taida_os_socket_recv_exact(taida_val socket_fd, taida_val size, taida_val timeout_ms);
taida_val taida_os_udp_bind(taida_val host_ptr, taida_val port, taida_val timeout_ms);
taida_val taida_os_udp_send_to(taida_val socket_fd, taida_val host_ptr, taida_val port, taida_val data_ptr, taida_val timeout_ms);
taida_val taida_os_udp_recv_from(taida_val socket_fd, taida_val timeout_ms);
taida_val taida_os_socket_close(taida_val socket_fd);
taida_val taida_os_listener_close(taida_val listener_fd);
// Pool package runtime
taida_val taida_pool_create(taida_val config_ptr);
taida_val taida_pool_acquire(taida_val pool_or_pack, taida_val timeout_ms);
taida_val taida_pool_release(taida_val pool_or_pack, taida_val token, taida_val resource);
taida_val taida_pool_close(taida_val pool_or_pack);
taida_val taida_pool_health(taida_val pool_or_pack);
taida_val taida_bit_and(taida_val a, taida_val b);
taida_val taida_bit_or(taida_val a, taida_val b);
taida_val taida_bit_xor(taida_val a, taida_val b);
taida_val taida_bit_not(taida_val x);
taida_val taida_shift_l(taida_val x, taida_val n);
taida_val taida_shift_r(taida_val x, taida_val n);
taida_val taida_shift_ru(taida_val x, taida_val n);
taida_val taida_to_radix(taida_val value, taida_val base);
taida_val taida_int_mold_auto(taida_val v);
taida_val taida_int_mold_str_base(taida_val v, taida_val base);
taida_val taida_uint8_mold(taida_val v);
taida_val taida_uint8_mold_float(double v);
taida_val taida_u16be_mold(taida_val value);
taida_val taida_u16le_mold(taida_val value);
taida_val taida_u32be_mold(taida_val value);
taida_val taida_u32le_mold(taida_val value);
taida_val taida_u16be_decode_mold(taida_val value);
taida_val taida_u16le_decode_mold(taida_val value);
taida_val taida_u32be_decode_mold(taida_val value);
taida_val taida_u32le_decode_mold(taida_val value);
taida_val taida_bytes_mold(taida_val value, taida_val fill);
taida_val taida_bytes_set(taida_val bytes_ptr, taida_val idx, taida_val value);
taida_val taida_bytes_to_list(taida_val bytes_ptr);
taida_val taida_bytes_cursor_new(taida_val bytes_ptr, taida_val offset);
taida_val taida_bytes_cursor_remaining(taida_val cursor_ptr);
taida_val taida_bytes_cursor_take(taida_val cursor_ptr, taida_val size);
taida_val taida_bytes_cursor_u8(taida_val cursor_ptr);
taida_val taida_char_mold_int(taida_val value);
taida_val taida_char_mold_str(taida_val value);
taida_val taida_codepoint_mold_str(taida_val value);
taida_val taida_utf8_encode_mold(taida_val value);
taida_val taida_utf8_decode_mold(taida_val value);
taida_val taida_slice_mold(taida_val value, taida_val start, taida_val end);
// NB-14: Call-site arg tag propagation (Bool/Int disambiguation)
void taida_push_call_tags(void);
void taida_pop_call_tags(void);
taida_val taida_set_call_arg_tag(taida_val index, taida_val tag);
taida_val taida_get_call_arg_tag(taida_val index);
// C12B-022: Runtime primitive-type check for TypeIs on param-tag idents
taida_val taida_primitive_tag_match(taida_val tag, taida_val expected);

// Taida runtime functions

// NB-14/NB-21: Stack-based call-site argument type tag propagation.
// Bool and Int are indistinguishable at the value level (both are unboxed i64).
// When a Bool value passes through a function parameter into a BuchiPack field,
// the field tag becomes UNKNOWN because the compiler cannot infer the parameter type.
// This mechanism propagates the caller's compile-time type tag to the callee:
//   Caller: taida_push_call_tags() + taida_set_call_arg_tag(i, tag) before CallUser
//   Callee: taida_get_call_arg_tag(i) at function entry
//   Caller: taida_pop_call_tags() after CallUser returns
// The stack ensures nested calls do not overwrite the outer call's tags.
#define TAG_STACK_DEPTH 64
#define TAG_FRAME_SIZE 256

// NB2-7: Thread-local tag stack prevents concurrent worker threads from corrupting
// each other's call-site type tag frames during handler invocation.
static __thread int64_t __taida_tag_stack[TAG_STACK_DEPTH][TAG_FRAME_SIZE];
static __thread int __taida_tag_stack_top = 0;

void taida_push_call_tags(void) {
    if (__taida_tag_stack_top < TAG_STACK_DEPTH) {
        memset(__taida_tag_stack[__taida_tag_stack_top], 0xFF, sizeof(__taida_tag_stack[0]));
        __taida_tag_stack_top++;
    }
}

void taida_pop_call_tags(void) {
    if (__taida_tag_stack_top > 0) {
        __taida_tag_stack_top--;
    }
}

taida_val taida_set_call_arg_tag(taida_val index, taida_val tag) {
    if (__taida_tag_stack_top > 0 && index >= 0 && index < TAG_FRAME_SIZE) {
        __taida_tag_stack[__taida_tag_stack_top - 1][index] = tag;
    }
    return 0;
}

taida_val taida_get_call_arg_tag(taida_val index) {
    if (__taida_tag_stack_top > 0 && index >= 0 && index < TAG_FRAME_SIZE) {
        return __taida_tag_stack[__taida_tag_stack_top - 1][index];
    }
    return TAIDA_TAG_UNKNOWN;
}

/* C12B-022: Runtime primitive-type check for `TypeIs[v, :T]()` when `v`
 * is a parameter whose type tag is threaded through `param_tag_vars`.
 *
 * `tag` is the runtime tag value (TAIDA_TAG_INT / _FLOAT / _BOOL / _STR / ...).
 * `expected` is the lowerer-emitted sentinel describing which primitive to
 * match:
 *    0  → Int  (tag == TAIDA_TAG_INT)
 *    1  → Float (tag == TAIDA_TAG_FLOAT)
 *    2  → Bool  (tag == TAIDA_TAG_BOOL)
 *    3  → Str   (tag == TAIDA_TAG_STR)
 *   -10 → Num   (tag == TAIDA_TAG_INT || tag == TAIDA_TAG_FLOAT)
 *
 * Returns 0 (false) for TAIDA_TAG_UNKNOWN (-1) so unknown-tag callers do
 * not accidentally match. This preserves the pre-C12B-022 output on
 * compile-time-literal paths (which never reach this helper because the
 * lowerer only emits it when `get_param_tag_var` returns Some).
 */
taida_val taida_primitive_tag_match(taida_val tag, taida_val expected) {
    if (tag == TAIDA_TAG_UNKNOWN) return 0;
    if (expected == -10) {
        return (tag == TAIDA_TAG_INT || tag == TAIDA_TAG_FLOAT) ? 1 : 0;
    }
    return (tag == expected) ? 1 : 0;
}

// NB-14: Return type tag propagation.
// Allows type info to survive through generic function returns (e.g. `id x = x`).
// Callee sets before return, caller reads after CallUser.
// NB2-7: Thread-local return tag prevents worker thread A's return value
// from being overwritten by worker thread B's concurrent handler call.
static __thread int64_t __taida_return_tag = TAIDA_TAG_UNKNOWN;

taida_val taida_set_return_tag(taida_val tag) {
    __taida_return_tag = tag;
    return 0;
}

taida_val taida_get_return_tag(void) {
    taida_val tag = __taida_return_tag;
    __taida_return_tag = TAIDA_TAG_UNKNOWN;
    return tag;
}

void taida_gorilla(void) { exit(1); }

// C18B-005 fix: print a `RuntimeError: <msg>` line to stderr and exit
// with status 1. Used by the native Ordinal[] lowering to reject
// non-Enum arguments — mirrors the interpreter's
// `mold_eval.rs::Ordinal` RuntimeError shape, and the JS runtime's
// `__taida_enumOrdinalStrict` throw path. Keeping stderr (not stdout)
// matches how the interpreter prints RuntimeError messages so parity
// tests can diff `.expected` against stdout only.
taida_val taida_runtime_panic(const char *msg) {
    if (msg) {
        fprintf(stderr, "Runtime error: %s\n", msg);
    } else {
        fprintf(stderr, "Runtime error\n");
    }
    exit(1);
}

taida_val taida_debug_int(taida_val value) {
    printf("%" PRId64 "\n", value);
    return 0;
}

taida_val taida_debug_float(double value) {
    // C26B-011 (Phase 11): route through `taida_float_to_str` so the
    // output matches the interpreter's Rust-f64::Display contract —
    // "X.0" for integer-valued floats, "NaN"/"inf"/"-inf" for IEEE 754
    // specials, shortest round-trip form for everything else. The old
    // `%g` dropped trailing ".0" and used "inf"/"-inf" without sign
    // parity with `taida_float_to_str`.
    const char *s = (const char *)taida_float_to_str(value);
    if (s) printf("%s\n", s); else printf("0.0\n");
    return 0;
}

taida_val taida_debug_bool(taida_val value) {
    if (value) printf("true\n"); else printf("false\n");
    return 0;
}

taida_val taida_debug_str(const char* ptr) {
    if (ptr) printf("%s\n", ptr); else printf("\n");
    return 0;
}

// Polymorphic debug: convert any value to string and print
// Uses taida_value_to_display_string (forward-declared above)
taida_val taida_debug_polymorphic(taida_val val) {
    taida_val str = taida_value_to_display_string(val);
    const char *s = (const char *)(intptr_t)str;
    if (s) printf("%s\n", s); else printf("\n");
    return 0;
}

// Arithmetic runtime
taida_val taida_int_add(taida_val a, taida_val b) { return a + b; }
taida_val taida_int_sub(taida_val a, taida_val b) { return a - b; }
taida_val taida_int_mul(taida_val a, taida_val b) { return a * b; }

// Bitwise molds (Int64 semantics)
taida_val taida_bit_and(taida_val a, taida_val b) { return (taida_val)(((uint64_t)a) & ((uint64_t)b)); }
taida_val taida_bit_or(taida_val a, taida_val b) { return (taida_val)(((uint64_t)a) | ((uint64_t)b)); }
taida_val taida_bit_xor(taida_val a, taida_val b) { return (taida_val)(((uint64_t)a) ^ ((uint64_t)b)); }
taida_val taida_bit_not(taida_val x) { return (taida_val)(~((uint64_t)x)); }

taida_val taida_shift_l(taida_val x, taida_val n) {
    if (n < 0 || n > 63) return taida_lax_empty(0);
    uint64_t shifted = ((uint64_t)x) << (unsigned int)n;
    return taida_lax_new((taida_val)shifted, 0);
}

taida_val taida_shift_r(taida_val x, taida_val n) {
    if (n < 0 || n > 63) return taida_lax_empty(0);
    int64_t shifted = ((int64_t)x) >> (unsigned int)n;
    return taida_lax_new((taida_val)shifted, 0);
}

taida_val taida_shift_ru(taida_val x, taida_val n) {
    if (n < 0 || n > 63) return taida_lax_empty(0);
    uint64_t shifted = ((uint64_t)x) >> (unsigned int)n;
    return taida_lax_new((taida_val)shifted, 0);
}

static taida_val taida_digit_to_char(taida_val digit) {
    return (digit < 10) ? ('0' + digit) : ('a' + (digit - 10));
}

taida_val taida_to_radix(taida_val value, taida_val base) {
    if (base < 2 || base > 36) return taida_lax_empty((taida_val)"");
    if (value == 0) {
        char *out = taida_str_alloc(1);
        out[0] = '0';
        return taida_lax_new((taida_val)out, (taida_val)"");
    }

    uint64_t mag = value < 0
        ? (uint64_t)(-(value + 1)) + 1
        : (uint64_t)value;
    char tmp[70];
    size_t pos = 0;
    while (mag > 0) {
        uint64_t rem = mag % (uint64_t)base;
        tmp[pos++] = (char)taida_digit_to_char((taida_val)rem);
        mag /= (uint64_t)base;
    }
    if (value < 0) tmp[pos++] = '-';

    char *out = taida_str_alloc(pos);
    for (size_t i = 0; i < pos; i++) {
        out[i] = tmp[pos - 1 - i];
    }
    return taida_lax_new((taida_val)out, (taida_val)"");
}
// FNV-1a hashes for error field names
#define HASH_TYPE    0xa79439ef7bfa9c2dULL
#define HASH_MESSAGE 0x546401b5d2a8d2a4ULL

static void taida_register_builtin_error_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;

    taida_register_field_name((taida_val)HASH_TYPE, (taida_val)"type");
    taida_register_field_name((taida_val)HASH_MESSAGE, (taida_val)"message");
    taida_register_field_name(taida_str_hash((taida_val)"field"), (taida_val)"field");
    taida_register_field_name(taida_str_hash((taida_val)"code"), (taida_val)"code");
    taida_register_field_name(taida_str_hash((taida_val)"kind"), (taida_val)"kind");
}

static taida_val taida_make_error(const char *error_type, const char *error_msg) {
    taida_register_builtin_error_field_names();

    taida_val pack = taida_pack_new(3);
    // Set hash for "type" field (index 0)
    taida_pack_set_hash(pack, 0, (taida_val)HASH_TYPE);
    char *type_str = taida_str_new_copy(error_type);
    taida_pack_set(pack, 0, (taida_val)type_str);
    taida_pack_set_tag(pack, 0, TAIDA_TAG_STR);
    // Set hash for "message" field (index 1)
    taida_pack_set_hash(pack, 1, (taida_val)HASH_MESSAGE);
    char *msg_str = taida_str_new_copy(error_msg);
    taida_pack_set(pack, 1, (taida_val)msg_str);
    taida_pack_set_tag(pack, 1, TAIDA_TAG_STR);
    // RCB-101 fix: Set __type field (index 2) so error type matching works.
    // Without this, taida_error_type_matches falls through to catch-all
    // because it looks for __type, not type.
    // Use literal FNV-1a("__type") hash since HASH___TYPE is defined later.
    taida_pack_set_hash(pack, 2, (taida_val)0x84d2d84b631f799bULL);
    char *type_str2 = taida_str_new_copy(error_type);
    taida_pack_set(pack, 2, (taida_val)type_str2);
    taida_pack_set_tag(pack, 2, TAIDA_TAG_STR);
    return pack;
}

static int taida_os_msg_contains(const char *msg, const char *needle) {
    return msg && needle && strstr(msg, needle) != NULL;
}

static const char *taida_os_error_kind(int err_code, const char *err_msg) {
    int code = err_code;
    if (code < 0) code = -code;

    switch (code) {
#ifdef EAGAIN
        case EAGAIN:
#endif
#if defined(EWOULDBLOCK) && (!defined(EAGAIN) || EWOULDBLOCK != EAGAIN)
        case EWOULDBLOCK:
#endif
#ifdef ETIMEDOUT
        case ETIMEDOUT:
#endif
            return "timeout";
#ifdef ECONNREFUSED
        case ECONNREFUSED:
#endif
            return "refused";
#ifdef ECONNRESET
        case ECONNRESET:
#endif
            return "reset";
#ifdef ECONNABORTED
        case ECONNABORTED:
#endif
#ifdef EPIPE
        case EPIPE:
#endif
#ifdef ENOTCONN
        case ENOTCONN:
#endif
            return "peer_closed";
#ifdef ENOENT
        case ENOENT:
#endif
            return "not_found";
#ifdef EINVAL
        case EINVAL:
#endif
            return "invalid";
        default:
            break;
    }

    if (
        taida_os_msg_contains(err_msg, "timed out") ||
        taida_os_msg_contains(err_msg, "Timed out") ||
        taida_os_msg_contains(err_msg, "timeout") ||
        taida_os_msg_contains(err_msg, "Timeout")
    ) {
        return "timeout";
    }
    if (
        taida_os_msg_contains(err_msg, "connection refused") ||
        taida_os_msg_contains(err_msg, "Connection refused")
    ) {
        return "refused";
    }
    if (
        taida_os_msg_contains(err_msg, "connection reset") ||
        taida_os_msg_contains(err_msg, "Connection reset")
    ) {
        return "reset";
    }
    if (
        taida_os_msg_contains(err_msg, "peer closed") ||
        taida_os_msg_contains(err_msg, "broken pipe") ||
        taida_os_msg_contains(err_msg, "socket hang up") ||
        taida_os_msg_contains(err_msg, "unexpected eof")
    ) {
        return "peer_closed";
    }
    if (
        taida_os_msg_contains(err_msg, "getaddrinfo") ||
        taida_os_msg_contains(err_msg, "Name or service not known") ||
        taida_os_msg_contains(err_msg, "dns") ||
        taida_os_msg_contains(err_msg, "DNS") ||
        taida_os_msg_contains(err_msg, "resolution failed")
    ) {
        return "dns";
    }
    if (taida_os_msg_contains(err_msg, "invalid") || taida_os_msg_contains(err_msg, "Invalid")) {
        return "invalid";
    }
    return "other";
}

static taida_val taida_make_io_error(int err_code, const char *err_msg) {
    taida_register_builtin_error_field_names();

    const char *message = err_msg ? err_msg : "unknown io error";
    const char *kind = taida_os_error_kind(err_code, message);

    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_TYPE);
    char *type_str = taida_str_new_copy("IoError");
    taida_pack_set(pack, 0, (taida_val)type_str);
    taida_pack_set_tag(pack, 0, TAIDA_TAG_STR);

    taida_pack_set_hash(pack, 1, (taida_val)HASH_MESSAGE);
    char *msg_str = taida_str_new_copy(message);
    taida_pack_set(pack, 1, (taida_val)msg_str);
    taida_pack_set_tag(pack, 1, TAIDA_TAG_STR);

    taida_val code_hash = taida_str_hash((taida_val)"code");
    taida_pack_set_hash(pack, 2, code_hash);
    taida_pack_set(pack, 2, (taida_val)err_code);
    // code is Int, tag defaults to 0 (TAIDA_TAG_INT)

    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(pack, 3, kind_hash);
    char *kind_str = taida_str_new_copy(kind);
    taida_pack_set(pack, 3, (taida_val)kind_str);
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);
    return pack;
}

// ── Lax[T] runtime ────────────────────────────────────────
// Lax is a BuchiPack with 4 fields: @(hasValue, __value, __default, __type)
// Layout: [refcount, field_count=4, hash0, val0, hash1, val1, hash2, val2, hash3, val3]
// Field 0: hasValue (0 or 1)
// Field 1: __value
// Field 2: __default
// Field 3: __type (pointer to "Lax" string)

static const char __lax_type_str[] = "Lax";
static const char __gorillax_type_str[] = "Gorillax";
static const char __relaxed_gorillax_type_str[] = "RelaxedGorillax";
static const char __molten_type_str[] = "Molten";
static const char __todo_type_str[] = "TODO";
static const char __bytes_cursor_type_str[] = "BytesCursor";

// FNV-1a hashes for Lax field names (computed with FNV-1a algorithm)
#define HASH_HAS_VALUE 0x9e9c6dc733414d60ULL
#define HASH___VALUE   0x0a7fc9f13472bbe0ULL
#define HASH___DEFAULT 0xed4fba440f8602d4ULL
#define HASH___ERROR   0x15c3e6e41a99a6cbULL
#define HASH___TYPE    0x84d2d84b631f799bULL
#define HASH_TODO_ID   0x08b72e07b55c3ac0ULL
#define HASH_TODO_TASK 0xd9603bef07a9524cULL
#define HASH_TODO_SOL  0x824fa3195cf2e6c1ULL
#define HASH_TODO_UNM  0x4cadac193e198b15ULL
#define HASH_CURSOR_BYTES  0x2f2ec0474f1c4fe4ULL
#define HASH_CURSOR_OFFSET 0x0268b0f8129435caULL
#define HASH_CURSOR_LENGTH 0xea11573f1af59eb5ULL
#define HASH_STEP_VALUE    0x7ce4fd9430e80ceaULL
#define HASH_STEP_CURSOR   0xf927453fbe6252efULL

// retain-on-store helper: detect heap type via magic header and retain + set tag
// NO-4 RULE 1: All runtime-built packs MUST use this helper (or taida_pack_set_tag
// + explicit retain) when storing heap children. Raw taida_pack_set alone is
// INSUFFICIENT — it skips type_tag and retain, causing leak on release.
static void taida_retain_and_tag_field(taida_val pack, taida_val field_idx, taida_val value) {
    if (value > 4096 && taida_ptr_is_readable(value, sizeof(taida_val))) {
        taida_val vtag = ((taida_val*)value)[0] & TAIDA_MAGIC_MASK;
        if (vtag == TAIDA_PACK_MAGIC) {
            taida_retain(value);
            taida_pack_set_tag(pack, field_idx, TAIDA_TAG_PACK);
        } else if (vtag == TAIDA_LIST_MAGIC) {
            taida_retain(value);
            taida_pack_set_tag(pack, field_idx, TAIDA_TAG_LIST);
        } else if (vtag == TAIDA_CLOSURE_MAGIC) {
            taida_retain(value);
            taida_pack_set_tag(pack, field_idx, TAIDA_TAG_CLOSURE);
        } else if (vtag == TAIDA_HMAP_MAGIC) {
            taida_retain(value);
            taida_pack_set_tag(pack, field_idx, TAIDA_TAG_HMAP);
        } else if (vtag == TAIDA_SET_MAGIC) {
            taida_retain(value);
            taida_pack_set_tag(pack, field_idx, TAIDA_TAG_SET);
        } else if (vtag == TAIDA_ASYNC_MAGIC) {
            taida_retain(value);
            taida_pack_set_tag(pack, field_idx, TAIDA_TAG_PACK);  // Async uses PACK tag for retain/release
        }
    }
    // Str の hidden-header は value の 16 バイト前にあるため、上記の value[0] チェックでは検出できない。
    // 別途 ptr-16 を調べて TAIDA_STR_MAGIC を確認する。
    if (value > 4096) {
        taida_val *hdr = ((taida_val*)value) - 2;
        if (taida_ptr_is_readable((taida_val)hdr, sizeof(taida_val))) {
            taida_val htag = hdr[0] & TAIDA_MAGIC_MASK;
            if (htag == TAIDA_STR_MAGIC) {
                taida_str_retain(value);
                taida_pack_set_tag(pack, field_idx, TAIDA_TAG_STR);
            }
        }
    }
}

// TF-15: Register Lax internal field names so display_string can render them
// C23B-003 reopen: also register `__error` (used by Gorillax/RelaxedGorillax)
// so `taida_pack_to_display_string_full` emits those fields instead of
// silently skipping them (was rendering `@()` for Gorillax `Str[...]()`).
static void taida_register_lax_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;
    taida_register_field_name((taida_val)HASH_HAS_VALUE, (taida_val)"hasValue");
    taida_register_field_name((taida_val)HASH___VALUE, (taida_val)"__value");
    taida_register_field_name((taida_val)HASH___DEFAULT, (taida_val)"__default");
    taida_register_field_name((taida_val)HASH___ERROR, (taida_val)"__error");
    taida_register_field_name((taida_val)HASH___TYPE, (taida_val)"__type");
    // Register hasValue as Bool type for correct display (true/false instead of 0/1)
    taida_register_field_type((taida_val)HASH_HAS_VALUE, (taida_val)"hasValue", 4);
}

taida_val taida_lax_new(taida_val value, taida_val default_value) {
    taida_register_lax_field_names();
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1);  // hasValue = true
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, value);
    // retain-on-store: value が Pack/List/Closure の場合 retain + tag 設定
    taida_retain_and_tag_field(pack, 1, value);
    taida_pack_set_hash(pack, 2, (taida_val)HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    // retain-on-store: default_value が Pack/List/Closure の場合 retain + tag 設定
    taida_retain_and_tag_field(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__lax_type_str);
    // C24-B (2026-04-23): stamp STR tag on __type so the new explicit
    // `render_int` branch in `taida_pack_to_display_string_full` doesn't
    // intercept this slot as a raw integer. `__lax_type_str` is a static
    // C string (not heap-allocated), so TAIDA_TAG_STR is rendering-only —
    // release / free paths still see it as non-heap and skip.
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);
    return pack;
}

taida_val taida_lax_empty(taida_val default_value) {
    taida_register_lax_field_names();
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);  // hasValue = false
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, default_value);
    // retain-on-store: default_value stored in __value slot
    taida_retain_and_tag_field(pack, 1, default_value);
    taida_pack_set_hash(pack, 2, (taida_val)HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    // retain-on-store: same default_value stored in __default slot (needs separate retain)
    taida_retain_and_tag_field(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__lax_type_str);
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);  // C24-B: see `taida_lax_new`
    return pack;
}

taida_val taida_lax_has_value(taida_val lax_ptr) {
    return taida_pack_get_idx(lax_ptr, 0);  // hasValue field
}

taida_val taida_lax_get_or_default(taida_val lax_ptr, taida_val fallback) {
    if (taida_pack_get_idx(lax_ptr, 0)) {
        return taida_pack_get_idx(lax_ptr, 1);  // __value
    }
    return fallback;
}

taida_val taida_lax_unmold(taida_val lax_ptr) {
    if (taida_pack_get_idx(lax_ptr, 0)) {
        return taida_pack_get_idx(lax_ptr, 1);  // __value
    }
    return taida_pack_get_idx(lax_ptr, 2);  // __default
}

taida_val taida_lax_is_empty(taida_val lax_ptr) {
    return taida_pack_get_idx(lax_ptr, 0) ? 0 : 1;
}

taida_val taida_molten_new(void) {
    taida_val pack = taida_pack_new(1);
    taida_pack_set_hash(pack, 0, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 0, (taida_val)__molten_type_str);
    // Static string - leave tag as INT(0) to skip free
    return pack;
}

// C25B-001: Stream[val]() — minimal wrapper matching the interpreter's
// `Stream[val]()` semantics (wrap a single value as a single-item
// completed stream; see `src/interpreter/mold_eval.rs:3127`). Phase 3
// scope: string-form parity with `Str[Stream[...]]()` →
// `Lax[Str]("Stream[completed: N items]")`. Full lazy-transform chain
// (Map / Filter / Take / TakeWhile) remains interpreter-only until a
// later Phase / C26+ roadmap.
//
// Layout: fc=2 pack
//   field 0: hash = HASH_STREAM_STATUS, val = status tag (1 = Completed)
//   field 1: hash = HASH_STREAM_COUNT,  val = item count (Int)
//   plus a third slot carrying __type = "Stream" for display paths.
#define HASH_STREAM_STATUS 0x6d32b928f2c5d8aeULL /* FNV-1a("__stream_status") */
#define HASH_STREAM_COUNT  0x1c0dd3a9e6fd1178ULL /* FNV-1a("__stream_count")  */
static const char __stream_type_str[] = "Stream";
static const char __stream_status_completed[] = "completed";

static int __stream_names_registered = 0;
static void taida_register_stream_field_names(void) {
    if (__stream_names_registered) return;
    __stream_names_registered = 1;
    taida_register_field_name((taida_val)HASH_STREAM_STATUS, (taida_val)"__stream_status");
    taida_register_field_name((taida_val)HASH_STREAM_COUNT,  (taida_val)"__stream_count");
}

taida_val taida_stream_new(taida_val inner_value) {
    (void)inner_value; // Minimal impl: only retain the item count (=1).
    taida_register_stream_field_names();
    taida_val pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_STREAM_STATUS);
    taida_pack_set(pack, 0, (taida_val)__stream_status_completed);
    taida_pack_set_tag(pack, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(pack, 1, (taida_val)HASH_STREAM_COUNT);
    taida_pack_set(pack, 1, 1); // single-item stream
    taida_pack_set_tag(pack, 1, TAIDA_TAG_INT);
    taida_pack_set_hash(pack, 2, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 2, (taida_val)__stream_type_str);
    taida_pack_set_tag(pack, 2, TAIDA_TAG_STR);
    return pack;
}

// Detect a Stream pack by structural shape (fc=3, hash0=HASH_STREAM_STATUS).
static int taida_is_stream_pack(taida_val ptr) {
    if (!taida_ptr_is_readable(ptr, sizeof(taida_val) * 3)) return 0;
    taida_val *obj = (taida_val*)ptr;
    if (obj[1] != 3) return 0; // fc must be 3
    return obj[2] == (taida_val)HASH_STREAM_STATUS ? 1 : 0;
}

static taida_val taida_stream_to_display_string(taida_val stream_ptr) {
    taida_val *obj = (taida_val*)stream_ptr;
    taida_val count = obj[2 + 1 * 3 + 2]; // slot 1 value
    char buf[64];
    snprintf(buf, sizeof(buf), "Stream[completed: %" PRId64 " items]", count);
    return (taida_val)taida_str_new_copy(buf);
}

taida_val taida_stub_new(taida_val message) {
    if (message == 0 || message < 4096) {
        taida_val err = taida_make_error("TypeError", "Stub message must be a string literal/expression");
        return taida_throw(err);
    }
    size_t len = 0;
    if (!taida_read_cstr_len_safe((const char*)message, 1024, &len)) {
        taida_val err = taida_make_error("TypeError", "Stub message must be a string literal/expression");
        return taida_throw(err);
    }
    return taida_molten_new();
}

taida_val taida_todo_new(taida_val id, taida_val task, taida_val sol, taida_val unm) {
    taida_val pack = taida_pack_new(7);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_TODO_ID);
    taida_pack_set(pack, 0, id);
    taida_pack_set_hash(pack, 1, (taida_val)HASH_TODO_TASK);
    taida_pack_set(pack, 1, task);
    taida_pack_set_hash(pack, 2, (taida_val)HASH_TODO_SOL);
    taida_pack_set(pack, 2, sol);
    taida_pack_set_hash(pack, 3, (taida_val)HASH_TODO_UNM);
    taida_pack_set(pack, 3, unm);
    taida_pack_set_hash(pack, 4, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 4, sol);
    taida_pack_set_hash(pack, 5, (taida_val)HASH___DEFAULT);
    taida_pack_set(pack, 5, unm);
    taida_pack_set_hash(pack, 6, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 6, (taida_val)__todo_type_str);
    return pack;
}

static int taida_is_molten(taida_val ptr) {
    if (!TAIDA_IS_PACK(ptr)) return 0;
    if (!taida_ptr_is_readable(ptr, sizeof(taida_val) * 5)) return 0;
    taida_val *obj = (taida_val*)ptr;
    if (obj[1] != 1) return 0;
    if (obj[2] != (taida_val)HASH___TYPE) return 0;  // hash at stride-3 offset 0
    taida_val type_ptr = obj[4];  // value at stride-3 offset 2
    if (type_ptr == (taida_val)__molten_type_str) return 1;
    if (!taida_ptr_is_readable(type_ptr, 1)) return 0;
    const char *type_str = (const char*)type_ptr;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(type_str, 32, &len)) return 0;
    return len == 6 && memcmp(type_str, "Molten", 6) == 0;
}

// ── Gorillax / RelaxedGorillax ──────────────────────────────
// Gorillax is like Lax but unmold failure = program termination (gorilla).
// RelaxedGorillax is like Gorillax but unmold failure = throw.
// Same BuchiPack layout as Lax: @(hasValue, __value, __error, __type)
// Field 2 is __error (not __default like Lax).

taida_val taida_gorillax_new(taida_val value) {
    // C23B-003 reopen: ensure `__error` / `__type` / `hasValue` / `__value`
    // field names are in the registry so `taida_pack_to_display_string_full`
    // (used by `Str[Gorillax[...]]()`) emits all four fields instead of
    // silently skipping them. The Lax registration also covers these.
    taida_register_lax_field_names();
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1);  // hasValue = true
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, value);
    // retain-on-store: value が Pack/List/Closure の場合 retain + tag 設定
    taida_retain_and_tag_field(pack, 1, value);
    // C19B-001: field 2 is `__error`, not `__default`. Using the correct
    // FNV-1a hash lets user code actually look up `.__error` at runtime.
    taida_pack_set_hash(pack, 2, (taida_val)HASH___ERROR);
    taida_pack_set(pack, 2, 0);  // __error = Unit (no error)
    // C23B-003 reopen: tag `__error` as PACK so that
    // `taida_pack_to_display_string_full` renders Unit as `@()` instead of
    // `0` (matches interpreter `Value::Unit.to_debug_string()`).
    taida_pack_set_tag(pack, 2, TAIDA_TAG_PACK);
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__gorillax_type_str);
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);  // C24-B: __type is a string
    return pack;
}

taida_val taida_gorillax_err(taida_val error) {
    taida_register_lax_field_names();  // C23B-003 reopen — register __error
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);  // hasValue = false
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, 0);  // __value = Unit
    // C19B-001: use the correct `__error` hash so `.__error.<field>` resolves.
    taida_pack_set_hash(pack, 2, (taida_val)HASH___ERROR);
    taida_pack_set(pack, 2, error);  // __error may be a Pack
    taida_pack_set_tag(pack, 2, TAIDA_TAG_PACK);
    if (error != 0) taida_retain(error);  // retain-on-store: error pack child
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__gorillax_type_str);
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);  // C24-B: __type is a string
    return pack;
}

taida_val taida_gorillax_unmold(taida_val ptr) {
    if (taida_pack_get_idx(ptr, 0)) {
        return taida_pack_get_idx(ptr, 1);  // hasValue=true → __value
    }
    // hasValue=false → GORILLA (program terminates)
    fprintf(stderr, "><\n");
    exit(1);
    return 0;  // unreachable
}

// Gorillax.relax() → RelaxedGorillax (same data, different __type)
taida_val taida_gorillax_relax(taida_val ptr) {
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, taida_pack_get_idx(ptr, 0));  // hasValue
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, taida_pack_get_idx(ptr, 1));  // __value
    // QF-50: retain-on-store for __value (may be Pack/List/Closure/HMap/Set/Str)
    taida_retain_and_tag_field(pack, 1, taida_pack_get_idx(ptr, 1));
    // C19B-001: field 2 is `__error`, keep its hash consistent with the
    // producer side (taida_gorillax_err) so `.__error.<field>` stays
    // resolvable across relax() transitions.
    taida_pack_set_hash(pack, 2, (taida_val)HASH___ERROR);
    taida_pack_set(pack, 2, taida_pack_get_idx(ptr, 2));  // __error
    // QF-50: retain-on-store for __error (typically a Pack)
    taida_retain_and_tag_field(pack, 2, taida_pack_get_idx(ptr, 2));
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__relaxed_gorillax_type_str);
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);  // C24-B: __type is a string
    return pack;
}

taida_val taida_relaxed_gorillax_unmold(taida_val ptr) {
    if (taida_pack_get_idx(ptr, 0)) {
        return taida_pack_get_idx(ptr, 1);  // hasValue=true → __value
    }
    // hasValue=false → throw RelaxedGorillaEscaped
    taida_val error = taida_make_error("RelaxedGorillaEscaped", "Relaxed gorilla escaped");
    return taida_throw(error);
}

taida_val taida_gorillax_to_string(taida_val ptr) {
    char tmp[128];
    if (taida_pack_get_idx(ptr, 0)) {
        taida_val value = taida_pack_get_idx(ptr, 1);
        snprintf(tmp, sizeof(tmp), "Gorillax(%" PRId64 ")", value);
    } else {
        memcpy(tmp, "Gorillax(><)", 13); /* 12 chars + '\0' */
    }
    return (taida_val)taida_str_new_copy(tmp);
}

taida_val taida_relaxed_gorillax_to_string(taida_val ptr) {
    char tmp[128];
    if (taida_pack_get_idx(ptr, 0)) {
        taida_val value = taida_pack_get_idx(ptr, 1);
        snprintf(tmp, sizeof(tmp), "RelaxedGorillax(%" PRId64 ")", value);
    } else {
        memcpy(tmp, "RelaxedGorillax(escaped)", 24); /* 23 chars + '\0' */
    }
    return (taida_val)taida_str_new_copy(tmp);
}

// Helper: check __type field of fc=4 BuchiPack for Gorillax/RelaxedGorillax
// Returns: 0 = Lax, 1 = Gorillax, 2 = RelaxedGorillax
static int taida_detect_gorillax_type(taida_val ptr) {
    if (!taida_ptr_is_readable(ptr, sizeof(taida_val) * 10)) return 0;
    taida_val type_ptr = taida_pack_get_idx(ptr, 3);  // __type field
    if (type_ptr == 0 || type_ptr < 4096) return 0;
    if (type_ptr == (taida_val)__gorillax_type_str) return 1;
    if (type_ptr == (taida_val)__relaxed_gorillax_type_str) return 2;
    const char *type_str = (const char*)type_ptr;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(type_str, 64, &len)) return 0;
    if (len == 8 && memcmp(type_str, "Gorillax", 8) == 0) return 1;
    if (len == 15 && memcmp(type_str, "RelaxedGorillax", 15) == 0) return 2;
    return 0;  // Lax
}

// taida_int_div and taida_int_mod removed — use Div/Mod molds
// Div[x, y]() and Mod[x, y]() return Lax BuchiPack
taida_val taida_div_mold(taida_val a, taida_val b) {
    if (b == 0) return taida_lax_empty(0);
    return taida_lax_new(a / b, 0);
}
taida_val taida_mod_mold(taida_val a, taida_val b) {
    if (b == 0) return taida_lax_empty(0);
    return taida_lax_new(a % b, 0);
}

// ── Type conversion molds (Str/Int/Float/Bool) ──────────────
// Each returns a Lax BuchiPack. Str default="", Int default=0, Float default=0.0, Bool default=false(0).
//
// C21B-seed-07: After constructing the Lax we stamp the `__value` (index 1)
// and `__default` (index 2) per-field tags with the mold's output primitive
// type. Without these tags, `taida_pack_to_display_string_full` cannot tell
// a Float bit-pattern apart from an Int and ends up rendering `Lax[3.0]` as
// `Lax[4613937818241073152]` or — worse — decoding a pack *pointer* as
// double bits via the FLOAT fast path in stdout_with_tag. See also the
// mold_returns.rs change that routes `Int[]/Float[]/Bool[]/Str[]` through
// the PACK tag path rather than their primitive output type.

// Helper: tag `__value` and `__default` of a Lax pack with the given primitive
// tag (`TAIDA_TAG_INT`/FLOAT/BOOL/STR). Must be called AFTER `taida_lax_new`
// or `taida_lax_empty` — both paths zero the tag slots for scalars by default.
static inline taida_val taida_lax_tag_value_default(taida_val lax, taida_val tag) {
    taida_pack_set_tag(lax, 1, tag);
    taida_pack_set_tag(lax, 2, tag);
    return lax;
}

// C26B-011 (Phase 11): Float-hint variants. Called when the lowering
// sees at least one Float operand on `Div[a, b]()` / `Mod[a, b]()`.
// Arguments are IEEE 754 doubles; the result is a FLOAT-tagged Lax so
// `taida_lax_to_string` / `__value` slot rendering goes through
// `taida_float_to_str` (Rust-f64 parity: `.0`, `inf`/`-inf`/`NaN`).
//
// Division semantics mirror the interpreter (`src/interpreter/mold_eval.rs`
// `Div` branch Float/Float case): b == 0.0 → Lax empty with default 0.0
// (no NaN / Inf coercion — Taida's Div is Lax-safe). For non-zero b we
// let the hardware produce `a / b` directly so special cases like
// `Div[1.0, Infinity]()` → `Lax(0.0)` match Rust / C libm exactly.
//
// Placement note: must live AFTER `taida_lax_tag_value_default` is defined
// (the helper is `static inline` so there is no forward declaration).
taida_val taida_div_mold_f(double a, double b) {
    if (b == 0.0) {
        return taida_lax_tag_value_default(
            taida_lax_empty(_d2l(0.0)), TAIDA_TAG_FLOAT);
    }
    return taida_lax_tag_value_default(
        taida_lax_new(_d2l(a / b), _d2l(0.0)), TAIDA_TAG_FLOAT);
}
taida_val taida_mod_mold_f(double a, double b) {
    if (b == 0.0) {
        return taida_lax_tag_value_default(
            taida_lax_empty(_d2l(0.0)), TAIDA_TAG_FLOAT);
    }
    // fmod(a, b): IEEE 754 remainder matching Rust's `%` operator on f64
    // and interpreter's `a % b` on Value::Float. Preserves sign of `a`.
    return taida_lax_tag_value_default(
        taida_lax_new(_d2l(fmod(a, b)), _d2l(0.0)), TAIDA_TAG_FLOAT);
}

// Str[x]() — always succeeds
taida_val taida_str_mold_int(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(taida_str_from_int(v), (taida_val)""), TAIDA_TAG_STR);
}
taida_val taida_str_mold_float(double v) {
    return taida_lax_tag_value_default(taida_lax_new(taida_str_from_float(v), (taida_val)""), TAIDA_TAG_STR);
}
taida_val taida_str_mold_bool(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(taida_str_from_bool(v), (taida_val)""), TAIDA_TAG_STR);
}
taida_val taida_str_mold_str(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(v, (taida_val)""), TAIDA_TAG_STR);
}

// C23-2: generic Str[x]() entry for non-primitive values (List/Pack/Lax/Result/…).
// The interpreter implements `Str[x]()` as `format!("{}", other)` which is the
// same as `to_display_string()` — for BuchiPacks that means the full-form
// `@(field <= value, ...)` including `__`-prefixed internals, which is what
// `taida_stdout_display_string` produces (and is distinct from the short-form
// `taida_value_to_display_string` Lax rendering `Lax(3)`). Wrap the resulting
// string in a Lax with STR tags on value/default, matching the primitive Str
// molds above.
taida_val taida_str_mold_any(taida_val v) {
    taida_val str = taida_stdout_display_string(v);
    return taida_lax_tag_value_default(taida_lax_new(str, (taida_val)""), TAIDA_TAG_STR);
}

// Int[x]() — Str parse can fail
taida_val taida_int_mold_int(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(v, 0), TAIDA_TAG_INT);
}
taida_val taida_int_mold_float(double v) {
    return taida_lax_tag_value_default(taida_lax_new((taida_val)v, 0), TAIDA_TAG_INT);
}
taida_val taida_int_mold_str(taida_val v) {
    const char *s = (const char *)v;
    if (!s || *s == '\0') return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
    // Reject leading whitespace to match Interpreter parity (Rust parse::<i64>)
    if (s[0] == ' ' || s[0] == '\t' || s[0] == '\n' || s[0] == '\r') return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
    char *end;
    taida_val result = strtol(s, &end, 10);
    if (*end != '\0') return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);  // parse failed
    return taida_lax_tag_value_default(taida_lax_new(result, 0), TAIDA_TAG_INT);
}
taida_val taida_int_mold_bool(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(v ? 1 : 0, 0), TAIDA_TAG_INT);
}

taida_val taida_int_mold_auto(taida_val v) {
    if (v == 0) return taida_int_mold_int(0);
    if (v < 0 || v < 4096) return taida_int_mold_int(v);

    if (taida_ptr_is_readable(v, sizeof(taida_val))) {
        taida_val tag = ((taida_val*)v)[0];
        if (taida_has_magic_header(tag)) {
            return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
        }
    }

    size_t len = 0;
    if (taida_read_cstr_len_safe((const char*)v, 4096, &len)) {
        return taida_int_mold_str(v);
    }

    return taida_int_mold_int(v);
}

// Float[x]() — Str parse can fail, result stored as bitcast taida_val
taida_val taida_float_mold_int(taida_val v) {
    double d = (double)v;
    return taida_lax_tag_value_default(taida_lax_new(_d2l(d), _d2l(0.0)), TAIDA_TAG_FLOAT);
}
taida_val taida_float_mold_float(double v) {
    return taida_lax_tag_value_default(taida_lax_new(_d2l(v), _d2l(0.0)), TAIDA_TAG_FLOAT);
}
taida_val taida_float_mold_str(taida_val v) {
    const char *s = (const char *)v;
    if (!s || *s == '\0') return taida_lax_tag_value_default(taida_lax_empty(_d2l(0.0)), TAIDA_TAG_FLOAT);
    char *end;
    double result = strtod(s, &end);
    if (*end != '\0') return taida_lax_tag_value_default(taida_lax_empty(_d2l(0.0)), TAIDA_TAG_FLOAT);  // parse failed
    return taida_lax_tag_value_default(taida_lax_new(_d2l(result), _d2l(0.0)), TAIDA_TAG_FLOAT);
}
taida_val taida_float_mold_bool(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(_d2l(v ? 1.0 : 0.0), _d2l(0.0)), TAIDA_TAG_FLOAT);
}

// Bool[x]() — Str accepts only "true"/"false"
taida_val taida_bool_mold_int(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(v != 0 ? 1 : 0, 0), TAIDA_TAG_BOOL);
}
taida_val taida_bool_mold_float(double v) {
    return taida_lax_tag_value_default(taida_lax_new(v != 0.0 ? 1 : 0, 0), TAIDA_TAG_BOOL);
}
taida_val taida_bool_mold_str(taida_val v) {
    const char *s = (const char *)v;
    if (!s) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_BOOL);
    if (strcmp(s, "true") == 0) return taida_lax_tag_value_default(taida_lax_new(1, 0), TAIDA_TAG_BOOL);
    if (strcmp(s, "false") == 0) return taida_lax_tag_value_default(taida_lax_new(0, 0), TAIDA_TAG_BOOL);
    return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_BOOL);  // not "true" or "false"
}
taida_val taida_bool_mold_bool(taida_val v) {
    return taida_lax_tag_value_default(taida_lax_new(v, 0), TAIDA_TAG_BOOL);
}

static int taida_char_to_digit(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'z') return c - 'a' + 10;
    if (c >= 'A' && c <= 'Z') return c - 'A' + 10;
    return -1;
}

taida_val taida_int_mold_str_base(taida_val v, taida_val base) {
    // C21B-seed-07: propagate INT tag to Lax fields for display parity.
    if (base < 2 || base > 36) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
    const char *s = (const char*)v;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(s, 4096, &len) || len == 0) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);

    int negative = 0;
    size_t i = 0;
    if (s[0] == '-') {
        negative = 1;
        i = 1;
        if (len == 1) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
    } else if (s[0] == '+') {
        i = 1;
        if (len == 1) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
    }

    uint64_t acc = 0;
    uint64_t limit = negative ? ((uint64_t)INT64_MAX + 1ULL) : (uint64_t)INT64_MAX;
    for (; i < len; i++) {
        int d = taida_char_to_digit((unsigned char)s[i]);
        if (d < 0 || d >= base) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
        if (acc > (limit - (uint64_t)d) / (uint64_t)base) return taida_lax_tag_value_default(taida_lax_empty(0), TAIDA_TAG_INT);
        acc = acc * (uint64_t)base + (uint64_t)d;
    }

    int64_t out = 0;
    if (negative) {
        if (acc == ((uint64_t)INT64_MAX + 1ULL)) out = INT64_MIN;
        else out = -(int64_t)acc;
    } else {
        out = (int64_t)acc;
    }
    return taida_lax_tag_value_default(taida_lax_new((taida_val)out, 0), TAIDA_TAG_INT);
}

taida_val taida_uint8_mold(taida_val v) {
    taida_val parsed = v;
    const char *s = (const char*)v;
    size_t len = 0;
    if (taida_read_cstr_len_safe(s, 256, &len) && len > 0) {
        char *end = NULL;
        taida_val tmp = strtol(s, &end, 10);
        if (end && *end == '\0') parsed = tmp;
    }
    if (parsed < 0 || parsed > 255) return taida_lax_empty(0);
    return taida_lax_new(parsed, 0);
}

taida_val taida_uint8_mold_float(double v) {
    if (!isfinite(v)) return taida_lax_empty(0);
    if (v < 0.0 || v > 255.0) return taida_lax_empty(0);
    if (floor(v) != v) return taida_lax_empty(0);
    return taida_lax_new((taida_val)v, 0);
}

taida_val taida_u16be_mold(taida_val value) {
    if (value < 0 || value > 65535) return taida_lax_empty(taida_bytes_default_value());
    unsigned char raw[2];
    uint16_t n = (uint16_t)value;
    raw[0] = (unsigned char)((n >> 8) & 0xFF);
    raw[1] = (unsigned char)(n & 0xFF);
    taida_val out = taida_bytes_from_raw(raw, 2);
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_u16le_mold(taida_val value) {
    if (value < 0 || value > 65535) return taida_lax_empty(taida_bytes_default_value());
    unsigned char raw[2];
    uint16_t n = (uint16_t)value;
    raw[0] = (unsigned char)(n & 0xFF);
    raw[1] = (unsigned char)((n >> 8) & 0xFF);
    taida_val out = taida_bytes_from_raw(raw, 2);
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_u32be_mold(taida_val value) {
    if (value < 0 || (uint64_t)value > 0xFFFFFFFFULL) {
        return taida_lax_empty(taida_bytes_default_value());
    }
    unsigned char raw[4];
    uint32_t n = (uint32_t)(uint64_t)value;
    raw[0] = (unsigned char)((n >> 24) & 0xFF);
    raw[1] = (unsigned char)((n >> 16) & 0xFF);
    raw[2] = (unsigned char)((n >> 8) & 0xFF);
    raw[3] = (unsigned char)(n & 0xFF);
    taida_val out = taida_bytes_from_raw(raw, 4);
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_u32le_mold(taida_val value) {
    if (value < 0 || (uint64_t)value > 0xFFFFFFFFULL) {
        return taida_lax_empty(taida_bytes_default_value());
    }
    unsigned char raw[4];
    uint32_t n = (uint32_t)(uint64_t)value;
    raw[0] = (unsigned char)(n & 0xFF);
    raw[1] = (unsigned char)((n >> 8) & 0xFF);
    raw[2] = (unsigned char)((n >> 16) & 0xFF);
    raw[3] = (unsigned char)((n >> 24) & 0xFF);
    taida_val out = taida_bytes_from_raw(raw, 4);
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_u16be_decode_mold(taida_val value) {
    if (!TAIDA_IS_BYTES(value)) return taida_lax_empty(0);
    taida_val *bytes = (taida_val*)value;
    if (bytes[1] != 2) return taida_lax_empty(0);
    uint16_t out = (uint16_t)(((uint16_t)bytes[2] << 8) | (uint16_t)bytes[3]);
    return taida_lax_new((taida_val)out, 0);
}

taida_val taida_u16le_decode_mold(taida_val value) {
    if (!TAIDA_IS_BYTES(value)) return taida_lax_empty(0);
    taida_val *bytes = (taida_val*)value;
    if (bytes[1] != 2) return taida_lax_empty(0);
    uint16_t out = (uint16_t)(((uint16_t)bytes[3] << 8) | (uint16_t)bytes[2]);
    return taida_lax_new((taida_val)out, 0);
}

taida_val taida_u32be_decode_mold(taida_val value) {
    if (!TAIDA_IS_BYTES(value)) return taida_lax_empty(0);
    taida_val *bytes = (taida_val*)value;
    if (bytes[1] != 4) return taida_lax_empty(0);
    uint32_t out =
        ((uint32_t)bytes[2] << 24) |
        ((uint32_t)bytes[3] << 16) |
        ((uint32_t)bytes[4] << 8) |
        (uint32_t)bytes[5];
    return taida_lax_new((taida_val)(uint64_t)out, 0);
}

taida_val taida_u32le_decode_mold(taida_val value) {
    if (!TAIDA_IS_BYTES(value)) return taida_lax_empty(0);
    taida_val *bytes = (taida_val*)value;
    if (bytes[1] != 4) return taida_lax_empty(0);
    uint32_t out =
        ((uint32_t)bytes[5] << 24) |
        ((uint32_t)bytes[4] << 16) |
        ((uint32_t)bytes[3] << 8) |
        (uint32_t)bytes[2];
    return taida_lax_new((taida_val)(uint64_t)out, 0);
}

static int taida_utf8_decode_one(
    const unsigned char *buf,
    size_t len,
    size_t *consumed,
    uint32_t *out_cp
) {
    if (len == 0) return 0;
    unsigned char b0 = buf[0];
    if (b0 < 0x80) {
        *consumed = 1;
        *out_cp = (uint32_t)b0;
        return 1;
    }

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
        unsigned char b1 = buf[1];
        unsigned char b2 = buf[2];
        if ((b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80) return 0;
        if (b0 == 0xE0 && b1 < 0xA0) return 0; // overlong
        if (b0 == 0xED && b1 >= 0xA0) return 0; // surrogate
        uint32_t cp = ((uint32_t)(b0 & 0x0F) << 12)
            | ((uint32_t)(b1 & 0x3F) << 6)
            | (uint32_t)(b2 & 0x3F);
        if (cp >= 0xD800 && cp <= 0xDFFF) return 0;
        *consumed = 3;
        *out_cp = cp;
        return 1;
    }

    if (b0 >= 0xF0 && b0 <= 0xF4) {
        if (len < 4) return 0;
        unsigned char b1 = buf[1];
        unsigned char b2 = buf[2];
        unsigned char b3 = buf[3];
        if ((b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 || (b3 & 0xC0) != 0x80) return 0;
        if (b0 == 0xF0 && b1 < 0x90) return 0; // overlong
        if (b0 == 0xF4 && b1 > 0x8F) return 0; // > U+10FFFF
        uint32_t cp = ((uint32_t)(b0 & 0x07) << 18)
            | ((uint32_t)(b1 & 0x3F) << 12)
            | ((uint32_t)(b2 & 0x3F) << 6)
            | (uint32_t)(b3 & 0x3F);
        if (cp > 0x10FFFF) return 0;
        *consumed = 4;
        *out_cp = cp;
        return 1;
    }

    return 0;
}

static int taida_utf8_encode_scalar(uint32_t cp, unsigned char out[4], size_t *out_len) {
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

static int taida_utf8_single_scalar(const unsigned char *buf, size_t len, uint32_t *cp_out) {
    size_t consumed = 0;
    uint32_t cp = 0;
    if (!taida_utf8_decode_one(buf, len, &consumed, &cp)) return 0;
    if (consumed != len) return 0;
    *cp_out = cp;
    return 1;
}

taida_val taida_char_mold_int(taida_val value) {
    if (value < 0 || value > 0x10FFFF) return taida_lax_empty((taida_val)"");
    if (value >= 0xD800 && value <= 0xDFFF) return taida_lax_empty((taida_val)"");
    unsigned char utf8[4];
    size_t out_len = 0;
    if (!taida_utf8_encode_scalar((uint32_t)value, utf8, &out_len)) {
        return taida_lax_empty((taida_val)"");
    }
    char *out = taida_str_alloc(out_len);
    memcpy(out, utf8, out_len);
    return taida_lax_new((taida_val)out, (taida_val)"");
}

taida_val taida_char_mold_str(taida_val value) {
    const char *s = (const char*)value;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(s, 4096, &len) || len == 0) {
        return taida_lax_empty((taida_val)"");
    }
    uint32_t cp = 0;
    if (!taida_utf8_single_scalar((const unsigned char*)s, len, &cp)) {
        return taida_lax_empty((taida_val)"");
    }
    return taida_char_mold_int((taida_val)cp);
}

taida_val taida_codepoint_mold_str(taida_val value) {
    const char *s = (const char*)value;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(s, 4096, &len) || len == 0) {
        return taida_lax_empty(0);
    }
    uint32_t cp = 0;
    if (!taida_utf8_single_scalar((const unsigned char*)s, len, &cp)) {
        return taida_lax_empty(0);
    }
    return taida_lax_new((taida_val)cp, 0);
}

taida_val taida_bytes_mold(taida_val value, taida_val fill) {
    if (TAIDA_IS_BYTES(value)) {
        taida_val cloned = taida_bytes_clone(value);
        return taida_lax_new(cloned, taida_bytes_default_value());
    }

    if (TAIDA_IS_LIST(value)) {
        taida_val *list = (taida_val*)value;
        taida_val len = list[2];
        taida_val out = taida_bytes_new_filled(len, 0);
        taida_val *bytes = (taida_val*)out;
        for (taida_val i = 0; i < len; i++) {
            taida_val item = list[4 + i];
            if (item < 0 || item > 255) {
                return taida_lax_empty(taida_bytes_default_value());
            }
            bytes[2 + i] = item;
        }
        return taida_lax_new(out, taida_bytes_default_value());
    }

    const char *s = (const char*)value;
    size_t slen = 0;
    if (taida_read_cstr_len_safe(s, 65536, &slen)) {
        taida_val out = taida_bytes_from_raw((const unsigned char*)s, (taida_val)slen);
        return taida_lax_new(out, taida_bytes_default_value());
    }

    taida_val len = value;
    if (len < 0 || len > 10000000) return taida_lax_empty(taida_bytes_default_value());
    if (fill < 0 || fill > 255) return taida_lax_empty(taida_bytes_default_value());
    taida_val out = taida_bytes_new_filled(len, (unsigned char)fill);
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_bytes_set(taida_val bytes_ptr, taida_val idx, taida_val value) {
    if (!TAIDA_IS_BYTES(bytes_ptr)) return taida_lax_empty(taida_bytes_default_value());
    taida_val len = taida_bytes_len(bytes_ptr);
    if (idx < 0 || idx >= len) return taida_lax_empty(taida_bytes_default_value());
    if (value < 0 || value > 255) return taida_lax_empty(taida_bytes_default_value());
    taida_val out = taida_bytes_clone(bytes_ptr);
    taida_val *bytes = (taida_val*)out;
    bytes[2 + idx] = value;
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_bytes_to_list(taida_val bytes_ptr) {
    taida_val list = taida_list_new();
    if (!TAIDA_IS_BYTES(bytes_ptr)) return list;
    taida_val *bytes = (taida_val*)bytes_ptr;
    taida_val len = bytes[1];
    for (taida_val i = 0; i < len; i++) {
        list = taida_list_push(list, bytes[2 + i]);
    }
    return list;
}

static int taida_bytes_cursor_unpack(taida_val cursor_ptr, taida_val *bytes_out, taida_val *offset_out) {
    if (!TAIDA_IS_PACK(cursor_ptr)) return 0;
    taida_val *pack = (taida_val*)cursor_ptr;
    taida_val field_count = pack[1];
    if (field_count < 2) return 0;
    // A-4b: stride-3 layout: [magic+rc, fc, hash0, tag0, val0, hash1, tag1, val1, ...]
    taida_val bytes_ptr = pack[2 + 0 * 3 + 2];  // field 0 value
    taida_val offset = pack[2 + 1 * 3 + 2];      // field 1 value
    if (!TAIDA_IS_BYTES(bytes_ptr)) return 0;
    taida_val len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    *bytes_out = bytes_ptr;
    *offset_out = offset;
    return 1;
}

static taida_val taida_bytes_cursor_step(taida_val value, taida_val cursor) {
    taida_val pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_STEP_VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, (taida_val)HASH_STEP_CURSOR);
    taida_pack_set(pack, 1, cursor);
    return pack;
}

taida_val taida_bytes_cursor_new(taida_val bytes_ptr, taida_val offset) {
    if (!TAIDA_IS_BYTES(bytes_ptr)) {
        bytes_ptr = taida_bytes_default_value();
    }
    taida_val len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;

    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_CURSOR_BYTES);
    taida_pack_set(pack, 0, bytes_ptr);
    taida_pack_set_hash(pack, 1, (taida_val)HASH_CURSOR_OFFSET);
    taida_pack_set(pack, 1, offset);
    taida_pack_set_hash(pack, 2, (taida_val)HASH_CURSOR_LENGTH);
    taida_pack_set(pack, 2, len);
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__bytes_cursor_type_str);
    return pack;
}

taida_val taida_bytes_cursor_remaining(taida_val cursor_ptr) {
    taida_val bytes_ptr = 0;
    taida_val offset = 0;
    if (!taida_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
    return taida_bytes_len(bytes_ptr) - offset;
}

taida_val taida_bytes_cursor_take(taida_val cursor_ptr, taida_val size) {
    taida_val bytes_ptr = 0;
    taida_val offset = 0;
    if (!taida_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        taida_val empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        taida_val def = taida_bytes_cursor_step(taida_bytes_default_value(), empty_cursor);
        return taida_lax_empty(def);
    }

    taida_val current_cursor = taida_bytes_cursor_new(bytes_ptr, offset);
    taida_val default_step = taida_bytes_cursor_step(taida_bytes_default_value(), current_cursor);
    if (size < 0) return taida_lax_empty(default_step);

    taida_val len = taida_bytes_len(bytes_ptr);
    if (offset + size > len) return taida_lax_empty(default_step);

    taida_val *src = (taida_val*)bytes_ptr;
    taida_val out = taida_bytes_new_filled(size, 0);
    taida_val *dst = (taida_val*)out;
    for (taida_val i = 0; i < size; i++) {
        dst[2 + i] = src[2 + offset + i];
    }
    taida_val next_cursor = taida_bytes_cursor_new(bytes_ptr, offset + size);
    taida_val step = taida_bytes_cursor_step(out, next_cursor);
    return taida_lax_new(step, default_step);
}

taida_val taida_bytes_cursor_u8(taida_val cursor_ptr) {
    taida_val bytes_ptr = 0;
    taida_val offset = 0;
    if (!taida_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        taida_val empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        taida_val def = taida_bytes_cursor_step(0, empty_cursor);
        return taida_lax_empty(def);
    }

    taida_val current_cursor = taida_bytes_cursor_new(bytes_ptr, offset);
    taida_val default_step = taida_bytes_cursor_step(0, current_cursor);
    taida_val len = taida_bytes_len(bytes_ptr);
    if (offset >= len) return taida_lax_empty(default_step);

    taida_val *bytes = (taida_val*)bytes_ptr;
    taida_val value = bytes[2 + offset];
    taida_val next_cursor = taida_bytes_cursor_new(bytes_ptr, offset + 1);
    taida_val step = taida_bytes_cursor_step(value, next_cursor);
    return taida_lax_new(step, default_step);
}

taida_val taida_utf8_encode_mold(taida_val value) {
    const char *s = (const char*)value;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(s, 65536, &len)) {
        return taida_lax_empty(taida_bytes_default_value());
    }
    taida_val out = taida_bytes_from_raw((const unsigned char*)s, (taida_val)len);
    return taida_lax_new(out, taida_bytes_default_value());
}

taida_val taida_utf8_decode_mold(taida_val value) {
    if (!TAIDA_IS_BYTES(value)) return taida_lax_empty((taida_val)"");
    taida_val *bytes = (taida_val*)value;
    taida_val len = bytes[1];
    // R-01: Guard against negative length — a corrupted Bytes header could
    // pass a negative len, which would be cast to a huge size_t and trigger
    // a massive malloc followed by OOM abort.
    if (len <= 0) return taida_lax_new((taida_val)taida_str_new_copy(""), (taida_val)"");
    unsigned char *raw = (unsigned char*)TAIDA_MALLOC((size_t)len, "bytes_decode");
    for (taida_val i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];

    size_t pos = 0;
    while (pos < (size_t)len) {
        size_t consumed = 0;
        uint32_t cp = 0;
        if (!taida_utf8_decode_one(raw + pos, (size_t)len - pos, &consumed, &cp)) {
            free(raw);
            return taida_lax_empty((taida_val)"");
        }
        pos += consumed;
    }

    char *out = taida_str_alloc((size_t)len);
    memcpy(out, raw, (size_t)len);
    free(raw);
    return taida_lax_new((taida_val)out, (taida_val)"");
}

taida_val taida_int_neg(taida_val a) { return -a; }
double taida_float_neg(double a) { return -a; }

// Comparison runtime
taida_val taida_int_eq(taida_val a, taida_val b)  { return a == b ? 1 : 0; }
taida_val taida_int_neq(taida_val a, taida_val b) { return a != b ? 1 : 0; }
taida_val taida_str_eq(taida_val a, taida_val b)  { return (a && b) ? (strcmp((char*)a, (char*)b) == 0 ? 1 : 0) : (a == b ? 1 : 0); }
taida_val taida_str_neq(taida_val a, taida_val b) { return (a && b) ? (strcmp((char*)a, (char*)b) != 0 ? 1 : 0) : (a != b ? 1 : 0); }
static int taida_is_string_value(taida_val v) {
    if (v == 0 || v == 1 || v < 4096) return 0;
    // Check if 8-byte aligned (heap object) and has magic header
    if ((v & 0x7) == 0) {
        unsigned char vec = 0;
        uintptr_t page = (uintptr_t)v & ~((uintptr_t)4095);
        if (mincore((void*)page, 4096, &vec) == 0) {
            taida_val first = ((taida_val*)v)[0];
            if (taida_has_magic_header(first)) return 0;
        }
    }
    // If not a heap object with magic header, try reading as string
    // Use mincore to check page is mapped for the pointer itself
    unsigned char vec = 0;
    uintptr_t page = (uintptr_t)v & ~((uintptr_t)4095);
    if (mincore((void*)page, 4096, &vec) != 0) return 0;
    return 1;
}
taida_val taida_poly_eq(taida_val a, taida_val b) {
    if (taida_is_string_value(a) && taida_is_string_value(b))
        return strcmp((char*)a, (char*)b) == 0 ? 1 : 0;
    return a == b ? 1 : 0;
}
taida_val taida_poly_neq(taida_val a, taida_val b) {
    if (taida_is_string_value(a) && taida_is_string_value(b))
        return strcmp((char*)a, (char*)b) != 0 ? 1 : 0;
    return a != b ? 1 : 0;
}
// NTH-1: Helper — format a taida_val as a string for poly_add concatenation.
// If the value is a heap string, return it directly.
// If it looks like a bitcast float (outside small-integer range), format as %g.
// Otherwise format as integer.
static const char *_poly_val_to_str(taida_val v, char *buf, int bufsz) {
    if (taida_is_string_value(v)) return (const char *)v;
    // Heuristic: values outside [-1048576, 1048576] that are not valid pointers
    // are likely bitcast floats. This matches _to_double() in the float section.
    if (v < -1048576 || v > 1048576) {
        // Additional guard: skip if the value looks like a heap pointer
        if (!taida_is_string_value(v) && !taida_ptr_is_readable(v, 8)) {
            union { taida_val l; double d; } u;
            u.l = v;
            // Sanity: only treat as float if the double is finite
            if (u.d == u.d && u.d != (1.0/0.0) && u.d != -(1.0/0.0)) {
                snprintf(buf, bufsz, "%g", u.d);
                return buf;
            }
        }
    }
    snprintf(buf, bufsz, "%lld", (long long)v);
    return buf;
}

// FL-16: Polymorphic add — dispatches to str_concat or int_add at runtime
taida_val taida_poly_add(taida_val a, taida_val b) {
    if (taida_is_string_value(a) || taida_is_string_value(b)) {
        // Convert both to strings and concatenate
        char buf_a[64], buf_b[64];
        const char *sa = _poly_val_to_str(a, buf_a, sizeof(buf_a));
        const char *sb = _poly_val_to_str(b, buf_b, sizeof(buf_b));
        return taida_str_concat(sa, sb);
    }
    return a + b;
}
taida_val taida_int_lt(taida_val a, taida_val b)  { return a < b ? 1 : 0; }
taida_val taida_int_gt(taida_val a, taida_val b)  { return a > b ? 1 : 0; }
taida_val taida_int_gte(taida_val a, taida_val b) { return a >= b ? 1 : 0; }

// Boolean runtime
taida_val taida_bool_and(taida_val a, taida_val b) { return (a && b) ? 1 : 0; }
taida_val taida_bool_or(taida_val a, taida_val b)  { return (a || b) ? 1 : 0; }
taida_val taida_bool_not(taida_val a)         { return a ? 0 : 1; }

// ── Reference Counting ────────────────────────────────────
// All heap objects (Pack, List, Closure) start with refcount at [0].
// taida_retain increments refcount.
// taida_release decrements refcount and frees when it reaches 0.

static int taida_has_magic_header(taida_val tag) {
    taida_val magic = tag & TAIDA_MAGIC_MASK;
    return magic == TAIDA_LIST_MAGIC ||
           magic == TAIDA_PACK_MAGIC ||
           magic == TAIDA_HMAP_MAGIC ||
           magic == TAIDA_SET_MAGIC ||
           magic == TAIDA_ASYNC_MAGIC ||
           magic == TAIDA_CLOSURE_MAGIC ||
           magic == TAIDA_BYTES_MAGIC;
}

taida_val taida_retain(taida_ptr ptr) {
    if (ptr == 0) return 0;
    // Non-heap values are never retained.
    if (ptr > 0 && ptr < 4096) return ptr;
    if (ptr < 0) return ptr;
    if (!taida_ptr_is_readable(ptr, sizeof(taida_val))) return ptr;

    taida_val *obj = (taida_val*)ptr;
    taida_val tag = obj[0];
    if (taida_has_magic_header(tag)) {
        TAIDA_INC_RC(ptr);
        return ptr;
    }

    return ptr;
}

taida_val taida_release(taida_ptr ptr) {
    if (ptr == 0) return 0;
    // Skip non-heap values (small integers, negative values)
    if (ptr > 0 && ptr < 4096) return 0;
    if (ptr < 0) return 0;
    if (!taida_ptr_is_readable(ptr, sizeof(taida_val))) return 0;

    taida_val *obj = (taida_val*)ptr;
    taida_val tag = obj[0];
    if (taida_has_magic_header(tag)) {
        taida_val rc = TAIDA_GET_RC(ptr);
        if (rc <= 1) {
            // A-4d: Recursive release for Pack children before freeing.
            // Uses type tags (A-4a/b) to safely identify child heap objects.
            //
            // SAFETY: We only recursively release children that are KNOWN to be
            // exclusively owned by this pack. In the current Taida runtime,
            // values are immutable and sharing is pervasive, so we can only
            // safely release children in specific ownership patterns:
            //
            // 1. Closure env packs — always exclusively owned by the closure
            // 2. Pack fields tagged as PACK/LIST/CLOSURE — released recursively
            //    only if the child's own refcount allows it (the child's
            //    taida_release will check its own refcount and only free if <= 1)
            //
            // String fields (TAIDA_TAG_STR) are released via taida_str_release
            // which safely distinguishes heap strings (hidden header) from
            // static literals (no-op).
            // Recursive child release using type tags.
            // retain-on-store ensures each child stored in a Pack/List field
            // has been retained, so releasing here is balanced.
            //
            // Pack: iterate fields with stride=3 [hash, tag, value].
            // Release children tagged as PACK(4)/LIST(5)/CLOSURE(6)/STR(3).
            // STR fields use taida_str_release which is safe for both
            // heap strings (hidden header) and static literals (no-op).
            if (TAIDA_IS_PACK(ptr)) {
                taida_val count = obj[1];
                for (taida_val i = 0; i < count; i++) {
                    taida_val field_tag = obj[2 + i * 3 + 1];
                    if (field_tag == TAIDA_TAG_PACK || field_tag == TAIDA_TAG_LIST || field_tag == TAIDA_TAG_CLOSURE
                        || field_tag == TAIDA_TAG_HMAP || field_tag == TAIDA_TAG_SET) {
                        taida_val child = obj[2 + i * 3 + 2];
                        if (child > 4096) {
                            taida_release(child);
                        }
                    } else if (field_tag == TAIDA_TAG_STR) {
                        taida_val child = obj[2 + i * 3 + 2];
                        if (child > 4096) {
                            taida_str_release(child);
                        }
                    }
                }
            }
            // List elements: recursively released using elem_type_tag.
            // elem_type_tag (list[3]) is set by the checker-guaranteed
            // homogeneous type system (Step 1+2). Elements that were
            // copied from other lists have been retained (Step 3), so
            // release here is balanced.
            if (TAIDA_IS_LIST(ptr)) {
                taida_val *lobj = (taida_val*)ptr;
                taida_val llen = lobj[2];
                taida_val etag = lobj[3];
                for (taida_val i = 0; i < llen; i++) {
                    taida_list_elem_release(lobj[4 + i], etag);
                }
            }
            // Closure: release env pack (exclusively owned)
            if (TAIDA_IS_CLOSURE(ptr)) {
                taida_val env_ptr = obj[2];
                if (env_ptr > 4096) {
                    taida_release(env_ptr);
                }
            }
            // NO-2: Set — recursively release all elements using elem_type_tag
            // Same pattern as List recursive release (offset 3 = elem_type_tag).
            if (TAIDA_IS_SET(ptr)) {
                taida_val *sobj = (taida_val*)ptr;
                taida_val slen = sobj[2];
                taida_val etag = sobj[3];  // elem_type_tag
                for (taida_val i = 0; i < slen; i++) {
                    taida_list_elem_release(sobj[4 + i], etag);
                }
            }
            // NO-1: HashMap — recursively release all keys and values
            if (TAIDA_IS_HMAP(ptr)) {
                taida_val *hobj = (taida_val*)ptr;
                taida_val hcap = hobj[1];
                taida_val vtag = hobj[3];  // value_type_tag
                for (taida_val i = 0; i < hcap; i++) {
                    taida_val sh = hobj[HM_HEADER + i * 3];
                    taida_val sk = hobj[HM_HEADER + i * 3 + 1];
                    if (HM_SLOT_OCCUPIED(sh, sk)) {
                        taida_hashmap_key_release(sk);
                        taida_hashmap_val_release(hobj[HM_HEADER + i * 3 + 2], vtag);
                    }
                }
            }
            // NO-3: Async — recursively release value and error based on type tags
            if (TAIDA_IS_ASYNC(ptr)) {
                taida_val *aobj = (taida_val*)ptr;
                taida_val status = aobj[1];
                taida_val vtag = aobj[5];  // value_tag
                taida_val etag = aobj[6];  // error_tag
                if (status == 1) {
                    // fulfilled: release value
                    taida_list_elem_release(aobj[2], vtag);
                } else if (status == 2) {
                    // rejected: release error
                    taida_list_elem_release(aobj[3], etag);
                }
                // pending (status == 0): no value/error to release
                // thread_handle: if still pending with a live thread, we
                // must join before freeing (the thread writes into aobj).
                if (status == 0 && aobj[4] != 0) {
                    pthread_join((pthread_t)aobj[4], NULL);
                    aobj[4] = 0;
                    // After join, thread may have written value/error.
                    // Re-check and release.
                    taida_val new_status = aobj[1];
                    if (new_status == 1) {
                        taida_list_elem_release(aobj[2], aobj[5]);
                    } else if (new_status == 2) {
                        taida_list_elem_release(aobj[3], aobj[6]);
                    }
                }
            }
            free(obj);
            return 0;
        }
        TAIDA_DEC_RC(ptr);
        return 0;
    }

    return 0;
}

// ── Heap String helpers (A-4k) ────────────────────────────
// Hidden header layout: [magic+rc (8 bytes), len (8 bytes)] + [bytes...\0]
// The returned char* points to the bytes area (header + 16).
// Static strings (string literals, ConstStr) have no header and are
// identified by the absence of TAIDA_STR_MAGIC at ptr - 16.

// Allocate a heap string with room for `len` characters (+ \0).
// Returns pointer to the bytes area.  Caller must fill the bytes.
static char* taida_str_alloc(size_t len) {
    // M-09: Guard against size_t overflow in header+len+NUL calculation.
    // sizeof(taida_val)*2 = 16 on LP64, so len > SIZE_MAX - 17 would wrap.
    if (len > SIZE_MAX - (sizeof(taida_val) * 2 + 1)) {
        fprintf(stderr, "taida: string length overflow in taida_str_alloc: %zu\n", len);
        exit(1);
    }
    taida_val *hdr = (taida_val*)TAIDA_MALLOC(sizeof(taida_val) * 2 + len + 1, "str_alloc");
    hdr[0] = TAIDA_STR_MAGIC | 1;  // magic + refcount = 1
    hdr[1] = (taida_val)len;
    char *bytes = (char*)(hdr + 2);
    bytes[len] = '\0';
    return bytes;
}

// Copy a C string into a new heap string with hidden header.
static char* taida_str_new_copy(const char* src) {
    if (!src) {
        char *r = taida_str_alloc(0);
        r[0] = '\0';
        return r;
    }
    size_t len = strlen(src);
    char *r = taida_str_alloc(len);
    memcpy(r, src, len);
    return r;
}

// Retain a heap string (RC++).  No-op for static strings.
void taida_str_retain(taida_val ptr) {
    if (ptr == 0) return;
    if (ptr > 0 && ptr < 4096) return;
    if (ptr < 0) return;
    // Check hidden header at ptr - 16
    taida_val *hdr = ((taida_val*)ptr) - 2;
    if (!taida_ptr_is_readable((taida_val)hdr, sizeof(taida_val))) return;
    taida_val tag = hdr[0];
    if ((tag & TAIDA_MAGIC_MASK) == TAIDA_STR_MAGIC) {
        taida_val rc = tag & TAIDA_RC_MASK;
        if (rc < 255) {
            hdr[0] = (tag & TAIDA_MAGIC_MASK) | (rc + 1);
        }
    }
    // Static strings: no header → no-op
}

// Release a heap string (RC--).  Frees when RC reaches 0.
// No-op for static strings (no hidden header).
static void taida_str_release(taida_val ptr) {
    if (ptr == 0) return;
    if (ptr > 0 && ptr < 4096) return;
    if (ptr < 0) return;
    // Check hidden header at ptr - 16
    taida_val *hdr = ((taida_val*)ptr) - 2;
    if (!taida_ptr_is_readable((taida_val)hdr, sizeof(taida_val))) return;
    taida_val tag = hdr[0];
    if ((tag & TAIDA_MAGIC_MASK) == TAIDA_STR_MAGIC) {
        taida_val rc = tag & TAIDA_RC_MASK;
        if (rc <= 1) {
            free(hdr);  // Free the entire allocation (header + bytes)
        } else {
            hdr[0] = (tag & TAIDA_MAGIC_MASK) | (rc - 1);
        }
    }
    // Static strings: no header → no-op
}

// ── BuchiPack runtime ─────────────────────────────────────
// Pack layout (A-4b): [magic+rc, field_count, hash0, tag0, val0, hash1, tag1, val1, ...]
// Stride = 3 per field: [hash, type_tag, value]

taida_ptr taida_pack_new(taida_val field_count) {
    // M-01: Guard against negative field_count (taida_val is int64_t) and
    // overflow in the size calculation (2 + field_count * 3) * sizeof(taida_val).
    if (field_count < 0) {
        fprintf(stderr, "taida: invalid field_count %" PRId64 " in taida_pack_new\n", (int64_t)field_count);
        exit(1);
    }
    size_t fc = (size_t)field_count;
    size_t slots = taida_safe_add(2, taida_safe_mul(fc, 3, "pack_new fields"), "pack_new header");
    size_t alloc_size = taida_safe_mul(slots, sizeof(taida_val), "pack_new bytes");
    taida_val *pack = (taida_val*)TAIDA_MALLOC(alloc_size, "pack_new");
    pack[0] = TAIDA_PACK_MAGIC | 1;  // Magic + refcount
    pack[1] = field_count;
    // Initialize tag slots to TAIDA_TAG_INT (0) by default
    for (taida_val i = 0; i < field_count; i++) {
        pack[2 + i * 3] = 0;     // hash
        pack[2 + i * 3 + 1] = 0; // tag = INT (default)
        pack[2 + i * 3 + 2] = 0; // value
    }
    return (taida_ptr)pack;
}

taida_ptr taida_pack_set(taida_ptr pack_ptr, taida_val index, taida_val value) {
    taida_val *pack = (taida_val*)pack_ptr;
    pack[2 + index * 3 + 2] = value;
    return pack_ptr;
}

taida_ptr taida_pack_set_tag(taida_ptr pack_ptr, taida_val index, taida_val tag) {
    taida_val *pack = (taida_val*)pack_ptr;
    pack[2 + index * 3 + 1] = tag;
    return pack_ptr;
}

// C26B-011: positional tag getter (by slot index, not hash) — used by
// Lax.toString() / Result.toString() short-form renderers so `__value` /
// `__default` slots tagged FLOAT render via `taida_float_to_str`
// (Rust-f64 parity: `0.0` / `inf` / `-inf` / `NaN`). Without this path
// the short-form fell through to `taida_value_to_display_string(val)` →
// untagged fallback → `%lld` rendering, dropping Float-origin `.0`.
static taida_val taida_pack_get_tag_idx(taida_ptr pack_ptr, taida_val index) {
    taida_val *pack = (taida_val*)pack_ptr;
    return pack[2 + index * 3 + 1];
}

taida_val taida_pack_get(taida_ptr pack_ptr, taida_val field_hash) {
    taida_val *pack = (taida_val*)pack_ptr;
    taida_val count = pack[1];
    for (taida_val i = 0; i < count; i++) {
        if (pack[2 + i * 3] == field_hash) {
            return pack[2 + i * 3 + 2];
        }
    }
    return 0; // default value
}

// Get the type tag for a field by its hash. Returns TAIDA_TAG_UNKNOWN if not found.
// B11-2b: Made public (non-static) so codegen can emit calls for runtime tag lookup
// when stdout/stderr needs to format FieldAccess values with proper type display.
taida_val taida_pack_get_field_tag(taida_ptr pack_ptr, taida_val field_hash) {
    taida_val *pack = (taida_val*)pack_ptr;
    taida_val count = pack[1];
    for (taida_val i = 0; i < count; i++) {
        if (pack[2 + i * 3] == field_hash) {
            return pack[2 + i * 3 + 1];
        }
    }
    return TAIDA_TAG_UNKNOWN;
}

// Return a human-readable type name for a given type tag.
static const char *taida_tag_name(taida_val tag) {
    switch (tag) {
        case TAIDA_TAG_INT:     return "Int";
        case TAIDA_TAG_FLOAT:   return "Float";
        case TAIDA_TAG_BOOL:    return "Bool";
        case TAIDA_TAG_STR:     return "Str";
        case TAIDA_TAG_PACK:    return "BuchiPack";
        case TAIDA_TAG_LIST:    return "List";
        case TAIDA_TAG_CLOSURE: return "Closure";
        case TAIDA_TAG_HMAP:    return "HashMap";
        case TAIDA_TAG_SET:     return "Set";
        default:                return "unknown";
    }
}

// NB-14/NB-21: Runtime type detection for UNKNOWN-tagged values.
// When the compiler cannot determine the field tag (e.g., dynamic argument passing),
// we inspect the runtime value to determine its actual type.
// Note: Bool and Int are indistinguishable at the value level (both are unboxed scalars).
// Call-site arg tag propagation (taida_set/get_call_arg_tag) ensures that the pack
// field tag carries the correct Bool tag through function parameters.
static taida_val taida_runtime_detect_tag(taida_val value) {
    // Heap types can be distinguished by magic headers
    if (value == 0 || (value > 0 && value < 4096)) return TAIDA_TAG_INT;  // small scalar
    if (taida_is_list(value)) return TAIDA_TAG_LIST;
    if (taida_is_buchi_pack(value)) return TAIDA_TAG_PACK;
    if (TAIDA_IS_BYTES(value)) return TAIDA_TAG_STR;  // Bytes is string-like
    if (TAIDA_IS_CLOSURE(value)) return TAIDA_TAG_CLOSURE;
    if (taida_is_hashmap(value)) return TAIDA_TAG_HMAP;
    if (taida_is_set(value)) return TAIDA_TAG_SET;
    if (taida_is_string_value(value)) return TAIDA_TAG_STR;
    return TAIDA_TAG_INT;  // unboxed scalar fallback
}

// Format a value for error messages (parity with Interpreter/JS).
// Returns 1 if the tag was known, 0 if UNKNOWN (best-effort formatting).
static int taida_format_value(taida_val tag, taida_val value, char *buf, size_t size) {
    switch (tag) {
        case TAIDA_TAG_BOOL:
            snprintf(buf, size, "%s", value ? "true" : "false");
            return 1;
        case TAIDA_TAG_INT:
            snprintf(buf, size, "%lld", (long long)value);
            return 1;
        case TAIDA_TAG_FLOAT: {
            double d;
            memcpy(&d, &value, sizeof(double));
            snprintf(buf, size, "%g", d);
            return 1;
        }
        case TAIDA_TAG_STR: {
            size_t slen = 0;
            if (taida_read_cstr_len_safe((const char*)value, 128, &slen)) {
                snprintf(buf, size, "%s", (const char*)value);
            } else {
                snprintf(buf, size, "Str");
            }
            return 1;
        }
        case TAIDA_TAG_PACK:
            snprintf(buf, size, "BuchiPack");
            return 1;
        case TAIDA_TAG_LIST:
            snprintf(buf, size, "List");
            return 1;
        case TAIDA_TAG_CLOSURE:
            snprintf(buf, size, "Closure");
            return 1;
        default: {
            // UNKNOWN tag: use runtime type detection to format meaningfully
            taida_val detected = taida_runtime_detect_tag(value);
            if (detected != TAIDA_TAG_INT) {
                // Detected a non-scalar type; recurse with the resolved tag
                return taida_format_value(detected, value, buf, size);
            }
            // Unboxed scalar: Bool/Int indistinguishable at runtime, display as Int
            snprintf(buf, size, "%lld", (long long)value);
            return 0;
        }
    }
}

taida_val taida_pack_has_hash(taida_ptr pack_ptr, taida_val field_hash) {
    taida_val *pack = (taida_val*)pack_ptr;
    taida_val count = pack[1];
    for (taida_val i = 0; i < count; i++) {
        if (pack[2 + i * 3] == field_hash) {
            return 1;
        }
    }
    return 0;
}

taida_val taida_pack_get_idx(taida_ptr pack_ptr, taida_val index) {
    taida_val *pack = (taida_val*)pack_ptr;
    return pack[2 + index * 3 + 2];
}

taida_ptr taida_pack_set_hash(taida_ptr pack_ptr, taida_val index, taida_val hash) {
    taida_val *pack = (taida_val*)pack_ptr;
    pack[2 + index * 3] = hash;
    return pack_ptr;
}

// ── Global variable table ─────────────────────────────────
// Simple hash table for top-level variables accessed from functions.
// Key = name_hash (FNV-1a), Value = taida_val (tagged pointer or int).

#define TAIDA_GLOBAL_TABLE_SIZE 256
static taida_val taida_global_keys[TAIDA_GLOBAL_TABLE_SIZE];
static taida_val taida_global_vals[TAIDA_GLOBAL_TABLE_SIZE];
static int taida_global_used[TAIDA_GLOBAL_TABLE_SIZE];

void taida_global_set(taida_val name_hash, taida_val value) {
    unsigned int idx = (unsigned int)((uint64_t)name_hash % TAIDA_GLOBAL_TABLE_SIZE);
    for (int i = 0; i < TAIDA_GLOBAL_TABLE_SIZE; i++) {
        unsigned int slot = (idx + i) % TAIDA_GLOBAL_TABLE_SIZE;
        if (!taida_global_used[slot] || taida_global_keys[slot] == name_hash) {
            taida_global_keys[slot] = name_hash;
            taida_global_vals[slot] = value;
            taida_global_used[slot] = 1;
            return;
        }
    }
}

taida_val taida_global_get(taida_val name_hash) {
    unsigned int idx = (unsigned int)((uint64_t)name_hash % TAIDA_GLOBAL_TABLE_SIZE);
    for (int i = 0; i < TAIDA_GLOBAL_TABLE_SIZE; i++) {
        unsigned int slot = (idx + i) % TAIDA_GLOBAL_TABLE_SIZE;
        if (!taida_global_used[slot]) return 0;
        if (taida_global_keys[slot] == name_hash) return taida_global_vals[slot];
    }
    return 0;
}

// ── Closure runtime ───────────────────────────────────────
// Closure layout: [magic+refcount, fn_ptr, env_ptr]

taida_ptr taida_closure_new(taida_fn_ptr fn_ptr, taida_ptr env_ptr) {
    taida_val *closure = (taida_val*)TAIDA_MALLOC(3 * sizeof(taida_val), "closure_new");
    closure[0] = TAIDA_CLOSURE_MAGIC | 1;
    closure[1] = (taida_val)fn_ptr;
    closure[2] = (taida_val)env_ptr;
    return (taida_ptr)closure;
}

taida_fn_ptr taida_closure_get_fn(taida_ptr closure_ptr) {
    return (taida_fn_ptr)((taida_val*)closure_ptr)[1];
}

taida_ptr taida_closure_get_env(taida_ptr closure_ptr) {
    return (taida_ptr)((taida_val*)closure_ptr)[2];
}

taida_val taida_is_closure_value(taida_val ptr) {
    return TAIDA_IS_CLOSURE(ptr) ? 1 : 0;
}

typedef taida_val (*taida_cb1_fn)(taida_val);
typedef taida_val (*taida_closure_cb1_fn)(taida_val, taida_val);
typedef taida_val (*taida_cb2_fn)(taida_val, taida_val);
typedef taida_val (*taida_closure_cb2_fn)(taida_val, taida_val, taida_val);

static taida_val taida_invoke_callback1(taida_val fn_ptr, taida_val arg0) {
    if (TAIDA_IS_CLOSURE(fn_ptr)) {
        taida_val *closure = (taida_val*)fn_ptr;
        taida_closure_cb1_fn closure_fn = (taida_closure_cb1_fn)closure[1];
        taida_val env_ptr = closure[2];
        return closure_fn(env_ptr, arg0);
    }
    taida_cb1_fn fn = (taida_cb1_fn)fn_ptr;
    return fn(arg0);
}

static taida_val taida_invoke_callback2(taida_val fn_ptr, taida_val arg0, taida_val arg1) {
    if (TAIDA_IS_CLOSURE(fn_ptr)) {
        taida_val *closure = (taida_val*)fn_ptr;
        taida_closure_cb2_fn closure_fn = (taida_closure_cb2_fn)closure[1];
        taida_val env_ptr = closure[2];
        return closure_fn(env_ptr, arg0, arg1);
    }
    taida_cb2_fn fn = (taida_cb2_fn)fn_ptr;
    return fn(arg0, arg1);
}

// ── BuchiPack field call runtime ──────────────────────────
// obj.field(args) — get field from pack, then invoke as function.
// Handles both plain function pointers and closures.

typedef taida_val (*taida_cb0_fn)(void);
typedef taida_val (*taida_closure_cb0_fn)(taida_val);
typedef taida_val (*taida_cb3_fn)(taida_val, taida_val, taida_val);
typedef taida_val (*taida_closure_cb3_fn)(taida_val, taida_val, taida_val, taida_val);

taida_val taida_pack_call_field0(taida_val pack_ptr, taida_val field_hash) {
    taida_val fn_ptr = taida_pack_get(pack_ptr, field_hash);
    if (TAIDA_IS_CLOSURE(fn_ptr)) {
        taida_val *closure = (taida_val*)fn_ptr;
        taida_closure_cb0_fn closure_fn = (taida_closure_cb0_fn)closure[1];
        taida_val env_ptr = closure[2];
        return closure_fn(env_ptr);
    }
    taida_cb0_fn fn = (taida_cb0_fn)fn_ptr;
    return fn();
}

taida_val taida_pack_call_field1(taida_val pack_ptr, taida_val field_hash, taida_val arg0) {
    taida_val fn_ptr = taida_pack_get(pack_ptr, field_hash);
    if (TAIDA_IS_CLOSURE(fn_ptr)) {
        taida_val *closure = (taida_val*)fn_ptr;
        taida_closure_cb1_fn closure_fn = (taida_closure_cb1_fn)closure[1];
        taida_val env_ptr = closure[2];
        return closure_fn(env_ptr, arg0);
    }
    taida_cb1_fn fn = (taida_cb1_fn)fn_ptr;
    return fn(arg0);
}

taida_val taida_pack_call_field2(taida_val pack_ptr, taida_val field_hash, taida_val arg0, taida_val arg1) {
    taida_val fn_ptr = taida_pack_get(pack_ptr, field_hash);
    if (TAIDA_IS_CLOSURE(fn_ptr)) {
        taida_val *closure = (taida_val*)fn_ptr;
        taida_closure_cb2_fn closure_fn = (taida_closure_cb2_fn)closure[1];
        taida_val env_ptr = closure[2];
        return closure_fn(env_ptr, arg0, arg1);
    }
    taida_cb2_fn fn = (taida_cb2_fn)fn_ptr;
    return fn(arg0, arg1);
}

taida_val taida_pack_call_field3(taida_val pack_ptr, taida_val field_hash, taida_val arg0, taida_val arg1, taida_val arg2) {
    taida_val fn_ptr = taida_pack_get(pack_ptr, field_hash);
    if (TAIDA_IS_CLOSURE(fn_ptr)) {
        taida_val *closure = (taida_val*)fn_ptr;
        taida_closure_cb3_fn closure_fn = (taida_closure_cb3_fn)closure[1];
        taida_val env_ptr = closure[2];
        return closure_fn(env_ptr, arg0, arg1, arg2);
    }
    taida_cb3_fn fn = (taida_cb3_fn)fn_ptr;
    return fn(arg0, arg1, arg2);
}

// ── List runtime ──────────────────────────────────────────
// List layout: [refcount, capacity, length, elem_type_tag, item0, item1, ...]
// NO-4 RULE 1: New list builders MUST set elem_type_tag (via taida_list_set_elem_tag)
// AND retain each heap element (via taida_list_elem_retain). Direct taida_list_push
// without these two steps will cause tag loss and leak on release.

// Retain a list element based on the list's elem_type_tag.
// Used when copying elements from one list to another (shared ownership).
static void taida_list_elem_retain(taida_val elem, taida_val elem_tag) {
    if (elem_tag == TAIDA_TAG_PACK || elem_tag == TAIDA_TAG_LIST || elem_tag == TAIDA_TAG_CLOSURE
        || elem_tag == TAIDA_TAG_HMAP || elem_tag == TAIDA_TAG_SET) {
        if (elem > 4096) taida_retain(elem);
    } else if (elem_tag == TAIDA_TAG_STR) {
        if (elem > 4096) taida_str_retain(elem);
    }
    // INT, FLOAT, BOOL, UNKNOWN → no-op
}

// Release a list element based on the list's elem_type_tag.
// Used when freeing a list to release owned elements.
static void taida_list_elem_release(taida_val elem, taida_val elem_tag) {
    if (elem_tag == TAIDA_TAG_PACK || elem_tag == TAIDA_TAG_LIST || elem_tag == TAIDA_TAG_CLOSURE
        || elem_tag == TAIDA_TAG_HMAP || elem_tag == TAIDA_TAG_SET) {
        if (elem > 4096) taida_release(elem);
    } else if (elem_tag == TAIDA_TAG_STR) {
        if (elem > 4096) taida_str_release(elem);
    }
    // INT, FLOAT, BOOL, UNKNOWN → no-op (conservative: leak rather than crash)
}

taida_ptr taida_list_new(void) {
    taida_val *list = (taida_val*)TAIDA_MALLOC((4 + 16) * sizeof(taida_val), "list_new");
    list[0] = TAIDA_LIST_MAGIC | 1;   // Magic + refcount
    list[1] = 16;  // capacity
    list[2] = 0;   // length
    list[3] = TAIDA_TAG_UNKNOWN;  // elem_type_tag (unknown until set)
    return (taida_ptr)list;
}

void taida_list_set_elem_tag(taida_ptr list_ptr, taida_val tag) {
    // C23B-007 (2026-04-22): sentinel-separated downgrade logic (mirror of
    // wasm `taida_list_set_elem_tag`). Codegen calls this on every list-push
    // site, so for mixed lists like `@[1, "a", 2]` the naive overwrite would
    // end up with the last element's tag and confuse downstream tag-aware
    // consumers. Latch on HETEROGENEOUS(-2) so once downgraded the container
    // stays mixed for its lifetime; re-promotion is forbidden.
    taida_val *list = (taida_val*)list_ptr;
    taida_val existing = list[3];
    if (existing == TAIDA_TAG_HETEROGENEOUS) return;
    if (existing == TAIDA_TAG_UNKNOWN || existing == tag) {
        list[3] = tag;
    } else {
        list[3] = TAIDA_TAG_HETEROGENEOUS;
    }
}

taida_ptr taida_list_push(taida_ptr list_ptr, taida_val item) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val rc  = list[0];
    taida_val cap = list[1];
    taida_val len = list[2];
    if (len >= cap) {
        // M-05: Guard against cap * 2 overflow and (4 + new_cap) * sizeof overflow.
        taida_val new_cap = cap * 2;
        if (new_cap < cap || new_cap <= 0) {
            // Signed overflow detected — cap was already huge.
            fprintf(stderr, "taida: list capacity overflow (taida_list_push): %" PRId64 "\n", cap);
            exit(1);
        }
        size_t realloc_size = taida_safe_mul(taida_safe_add(4, (size_t)new_cap, "list_push slots"), sizeof(taida_val), "list_push bytes");
        taida_val *tmp = (taida_val*)realloc(list, realloc_size);
        if (!tmp) { fprintf(stderr, "taida: out of memory (taida_list_push)\n"); exit(1); }
        list = tmp;
        list[0] = rc;       // preserve refcount
        list[1] = new_cap;
    }
    list[4 + len] = item;
    list[2] = len + 1;
    return (taida_val)list;
}

taida_val taida_list_length(taida_val list_ptr) {
    return ((taida_val*)list_ptr)[2];
}

taida_val taida_list_get(taida_val list_ptr, taida_val index) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (index < 0 || index >= len) {
        return taida_lax_empty(0);  // OOB returns empty Lax (v0.5.0)
    }
    return taida_lax_new(list[4 + index], 0);
}

taida_val taida_list_first(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (len == 0) return taida_lax_empty(0);
    return taida_lax_new(list[4], 0);
}

taida_val taida_list_last(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (len == 0) return taida_lax_empty(0);
    return taida_lax_new(list[4 + len - 1], 0);
}

taida_val taida_list_sum(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val sum = 0;
    for (taida_val i = 0; i < len; i++) {
        sum += list[4 + i];
    }
    return sum;
}

taida_val taida_list_reverse(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = len - 1; i >= 0; i--) {
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    return new_list;
}

taida_val taida_list_contains(taida_val list_ptr, taida_val item) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        if (list[4 + i] == item) return 1;
    }
    return 0;
}

taida_val taida_list_is_empty(taida_val list_ptr) {
    return ((taida_val*)list_ptr)[2] == 0 ? 1 : 0;
}

// list.map(fn_ptr) - fn_ptr takes (taida_val) -> taida_val
taida_val taida_list_map(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val new_list = taida_list_new();
    // map may change element type, so leave elem_tag as UNKNOWN
    for (taida_val i = 0; i < len; i++) {
        taida_val result = taida_invoke_callback1(fn_ptr, list[4 + i]);
        new_list = taida_list_push(new_list, result);
    }
    return new_list;
}

// list.filter(fn_ptr) - fn_ptr takes (taida_val) -> taida_val (truthy/falsy)
taida_val taida_list_filter(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[4 + i])) {
            taida_list_elem_retain(list[4 + i], elem_tag);
            new_list = taida_list_push(new_list, list[4 + i]);
        }
    }
    return new_list;
}

// ── Bytes runtime ─────────────────────────────────────────
// Bytes layout: [magic+rc, length, b0, b1, ...] (each byte stored in taida_val slot)

static taida_val taida_bytes_new_filled(taida_val len, unsigned char fill) {
    if (len < 0) len = 0;
    // M-04: Guard against huge len values that could cause OOM or overflow
    // in (2 + len) * sizeof(taida_val). Limit to ~128M entries.
    if (len > (taida_val)(SIZE_MAX / sizeof(taida_val) - 2)) {
        fprintf(stderr, "taida: bytes length too large (taida_bytes_new_filled): %" PRId64 "\n", len);
        exit(1);
    }
    size_t alloc_size = taida_safe_mul(taida_safe_add(2, (size_t)len, "bytes_new_filled slots"), sizeof(taida_val), "bytes_new_filled bytes");
    taida_val *bytes = (taida_val*)TAIDA_MALLOC(alloc_size, "bytes_new_filled");
    bytes[0] = TAIDA_BYTES_MAGIC | 1;
    bytes[1] = len;
    for (taida_val i = 0; i < len; i++) {
        bytes[2 + i] = (taida_val)fill;
    }
    return (taida_val)bytes;
}

static taida_val taida_bytes_from_raw(const unsigned char *data, taida_val len) {
    taida_val out = taida_bytes_new_filled(len, 0);
    taida_val *bytes = (taida_val*)out;
    for (taida_val i = 0; i < len; i++) {
        bytes[2 + i] = (taida_val)data[i];
    }
    return out;
}

static taida_val taida_bytes_clone(taida_val bytes_ptr) {
    if (!TAIDA_IS_BYTES(bytes_ptr)) return taida_bytes_new_filled(0, 0);
    taida_val *src = (taida_val*)bytes_ptr;
    taida_val len = src[1];
    taida_val out = taida_bytes_new_filled(len, 0);
    taida_val *dst = (taida_val*)out;
    for (taida_val i = 0; i < len; i++) {
        dst[2 + i] = src[2 + i];
    }
    return out;
}

static taida_val taida_bytes_len(taida_val bytes_ptr) {
    if (!TAIDA_IS_BYTES(bytes_ptr)) return 0;
    return ((taida_val*)bytes_ptr)[1];
}

static taida_val taida_bytes_default_value(void) {
    return taida_bytes_new_filled(0, 0);
}

static taida_val taida_bytes_get_lax(taida_val bytes_ptr, taida_val index) {
    if (!TAIDA_IS_BYTES(bytes_ptr)) return taida_lax_empty(0);
    taida_val *bytes = (taida_val*)bytes_ptr;
    taida_val len = bytes[1];
    if (index < 0 || index >= len) return taida_lax_empty(0);
    return taida_lax_new(bytes[2 + index], 0);
}

// ── String methods ────────────────────────────────────────

taida_val taida_str_concat(const char* a, const char* b) {
    if (!a) a = "";
    if (!b) b = "";
    size_t la = strlen(a), lb = strlen(b);
    // M-10: Overflow guard on la + lb before passing to taida_str_alloc.
    size_t total_len = taida_safe_add(la, lb, "str_concat length");
    char *buf = taida_str_alloc(total_len);
    memcpy(buf, a, la);
    memcpy(buf + la, b, lb);
    return (taida_val)buf;
}

taida_val taida_str_length(const char* s) {
    if (!s) return 0;
    return (taida_val)strlen(s);
}

taida_val taida_str_char_at(const char* s, taida_val idx) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    if (idx < 0 || idx >= len) { char *r = taida_str_alloc(0); return (taida_val)r; }
    char *r = taida_str_alloc(1);
    r[0] = s[idx];
    return (taida_val)r;
}

taida_val taida_str_slice(const char* s, taida_val start, taida_val end) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    if (start < 0) start = 0;
    if (end > len) end = len;
    if (start >= end) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val slen = end - start;
    char *r = taida_str_alloc(slen);
    memcpy(r, s + start, slen);
    return (taida_val)r;
}

taida_val taida_slice_mold(taida_val value, taida_val start, taida_val end) {
    if (TAIDA_IS_BYTES(value)) {
        taida_val *bytes = (taida_val*)value;
        taida_val len = bytes[1];
        taida_val s = start;
        taida_val e = end;
        if (s < 0) s = 0;
        if (s > len) s = len;
        if (e < 0 || e > len) e = len;
        if (e < s) e = s;
        taida_val out_len = e - s;
        taida_val out = taida_bytes_new_filled(out_len, 0);
        taida_val *dst = (taida_val*)out;
        for (taida_val i = 0; i < out_len; i++) {
            dst[2 + i] = bytes[2 + s + i];
        }
        return out;
    }

    if (TAIDA_IS_LIST(value)) {
        taida_val *list = (taida_val*)value;
        taida_val len = list[2];
        taida_val elem_tag = list[3];
        taida_val s = start;
        taida_val e = end;
        if (s < 0) s = 0;
        if (s > len) s = len;
        if (e < 0 || e > len) e = len;
        if (e < s) e = s;
        taida_val out = taida_list_new();
        taida_val *ol = (taida_val*)out;
        ol[3] = elem_tag;  // propagate elem_type_tag
        for (taida_val i = s; i < e; i++) {
            taida_list_elem_retain(list[4 + i], elem_tag);
            out = taida_list_push(out, list[4 + i]);
        }
        return out;
    }

    const char *s = (const char*)value;
    if (!s) {
        char *r = taida_str_alloc(0);
        return (taida_val)r;
    }
    taida_val len = (taida_val)strlen(s);
    taida_val cs = start;
    taida_val ce = end;
    if (cs < 0) cs = 0;
    if (cs > len) cs = len;
    if (ce < 0 || ce > len) ce = len;
    if (ce < cs) ce = cs;
    return taida_str_slice(s, cs, ce);
}

taida_val taida_str_index_of(const char* s, const char* sub) {
    if (!s || !sub) return -1;
    const char *p = strstr(s, sub);
    if (!p) return -1;
    // Convert byte offset to character offset (UTF-8 aware)
    taida_val char_offset = 0;
    for (const char *c = s; c < p; ) {
        if ((*c & 0x80) == 0) c += 1;
        else if ((*c & 0xE0) == 0xC0) c += 2;
        else if ((*c & 0xF0) == 0xE0) c += 3;
        else c += 4;
        char_offset++;
    }
    return char_offset;
}

taida_val taida_str_last_index_of(const char* s, const char* sub) {
    if (!s || !sub) return -1;
    taida_val slen = (taida_val)strlen(s);
    taida_val sublen = (taida_val)strlen(sub);
    if (sublen > slen) return -1;
    for (taida_val i = slen - sublen; i >= 0; i--) {
        if (strncmp(s + i, sub, sublen) == 0) {
            // Convert byte offset to character offset (UTF-8 aware)
            taida_val char_offset = 0;
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

taida_val taida_str_contains(const char* s, const char* sub) {
    if (!s || !sub) return 0;
    return strstr(s, sub) != NULL ? 1 : 0;
}

taida_val taida_str_starts_with(const char* s, const char* prefix) {
    if (!s || !prefix) return 0;
    taida_val plen = (taida_val)strlen(prefix);
    return strncmp(s, prefix, plen) == 0 ? 1 : 0;
}

taida_val taida_str_ends_with(const char* s, const char* suffix) {
    if (!s || !suffix) return 0;
    taida_val slen = (taida_val)strlen(s);
    taida_val suflen = (taida_val)strlen(suffix);
    if (suflen > slen) return 0;
    return strcmp(s + slen - suflen, suffix) == 0 ? 1 : 0;
}

taida_val taida_str_get(const char* s, taida_val idx) {
    if (!s) return taida_lax_empty((taida_val)"");
    taida_val len = (taida_val)strlen(s);
    if (idx < 0 || idx >= len) return taida_lax_empty((taida_val)"");
    char *r = taida_str_alloc(1);
    r[0] = s[idx];
    return taida_lax_new((taida_val)r, (taida_val)"");
}

taida_val taida_str_to_upper(const char* s) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    char *r = taida_str_alloc(len);
    for (taida_val i = 0; i < len; i++) {
        r[i] = (s[i] >= 'a' && s[i] <= 'z') ? s[i] - 32 : s[i];
    }
    return (taida_val)r;
}

taida_val taida_str_to_lower(const char* s) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    char *r = taida_str_alloc(len);
    for (taida_val i = 0; i < len; i++) {
        r[i] = (s[i] >= 'A' && s[i] <= 'Z') ? s[i] + 32 : s[i];
    }
    return (taida_val)r;
}

taida_val taida_str_trim(const char* s) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    taida_val start = 0, end = len;
    while (start < len && (s[start]==' '||s[start]=='\t'||s[start]=='\n'||s[start]=='\r')) start++;
    while (end > start && (s[end-1]==' '||s[end-1]=='\t'||s[end-1]=='\n'||s[end-1]=='\r')) end--;
    taida_val slen = end - start;
    char *r = taida_str_alloc(slen);
    memcpy(r, s + start, slen);
    return (taida_val)r;
}

taida_val taida_str_split(const char* s, const char* sep) {
    if (!s) return taida_list_new();
    taida_val list = taida_list_new();
    if (!sep || strlen(sep) == 0) {
        // Split into Unicode codepoints (same logic as taida_str_chars)
        size_t slen = 0;
        if (!taida_read_cstr_len_safe(s, 65536, &slen) || slen == 0) return list;
        const unsigned char *buf = (const unsigned char*)s;
        size_t offset = 0;
        while (offset < slen) {
            size_t consumed = 0;
            uint32_t cp = 0;
            if (!taida_utf8_decode_one(buf + offset, slen - offset, &consumed, &cp) || consumed == 0) {
                char *fallback = taida_str_alloc(1);
                fallback[0] = (char)buf[offset];
                list = taida_list_push(list, (taida_val)fallback);
                offset += 1;
                continue;
            }
            unsigned char utf8[4];
            size_t out_len = 0;
            if (!taida_utf8_encode_scalar(cp, utf8, &out_len) || out_len == 0) {
                offset += consumed;
                continue;
            }
            char *ch = taida_str_alloc(out_len);
            memcpy(ch, utf8, out_len);
            list = taida_list_push(list, (taida_val)ch);
            offset += consumed;
        }
        return list;
    }
    taida_val sep_len = (taida_val)strlen(sep);
    const char *p = s;
    while (1) {
        const char *found = strstr(p, sep);
        if (!found) {
            taida_val slen = (taida_val)strlen(p);
            char *part = taida_str_alloc(slen);
            memcpy(part, p, slen);
            list = taida_list_push(list, (taida_val)part);
            break;
        }
        taida_val plen = (taida_val)(found - p);
        char *part = taida_str_alloc(plen);
        memcpy(part, p, plen);
        list = taida_list_push(list, (taida_val)part);
        p = found + sep_len;
    }
    return list;
}

taida_val taida_str_chars(const char* s) {
    taida_val list = taida_list_new();
    taida_list_set_elem_tag(list, TAIDA_TAG_STR);
    if (!s) return list;

    size_t len = 0;
    if (!taida_read_cstr_len_safe(s, 65536, &len) || len == 0) {
        return list;
    }

    const unsigned char *buf = (const unsigned char*)s;
    size_t offset = 0;
    while (offset < len) {
        size_t consumed = 0;
        uint32_t cp = 0;
        if (!taida_utf8_decode_one(buf + offset, len - offset, &consumed, &cp) || consumed == 0) {
            char *fallback = taida_str_alloc(1);
            fallback[0] = (char)buf[offset];
            list = taida_list_push(list, (taida_val)fallback);
            offset += 1;
            continue;
        }

        unsigned char utf8[4];
        size_t out_len = 0;
        if (!taida_utf8_encode_scalar(cp, utf8, &out_len) || out_len == 0) {
            offset += consumed;
            continue;
        }

        char *ch = taida_str_alloc(out_len);
        memcpy(ch, utf8, out_len);
        list = taida_list_push(list, (taida_val)ch);
        offset += consumed;
    }

    return list;
}

taida_val taida_str_replace(const char* s, const char* from, const char* to) {
    if (!s || !from || !to) {
        if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
        return (taida_val)taida_str_new_copy(s);
    }
    taida_val from_len = (taida_val)strlen(from);
    taida_val to_len = (taida_val)strlen(to);
    if (from_len == 0) {
        return (taida_val)taida_str_new_copy(s);
    }
    // Count occurrences
    taida_val count = 0;
    const char *p = s;
    while ((p = strstr(p, from)) != NULL) { count++; p += from_len; }
    taida_val s_len = (taida_val)strlen(s);
    taida_val new_len = s_len + count * (to_len - from_len);
    char *r = taida_str_alloc(new_len);
    char *dst = r;
    p = s;
    while (1) {
        const char *found = strstr(p, from);
        if (!found) {
            taida_val remaining = (taida_val)strlen(p);
            memcpy(dst, p, remaining);
            dst += remaining;
            break;
        }
        taida_val chunk = (taida_val)(found - p);
        memcpy(dst, p, chunk); dst += chunk;
        memcpy(dst, to, to_len); dst += to_len;
        p = found + from_len;
    }
    *dst = '\0';
    return (taida_val)r;
}

taida_val taida_str_to_int(const char* s) {
    if (!s) return 0;
    return atol(s);
}

taida_val taida_str_repeat(const char* s, taida_val n) {
    if (!s || n <= 0) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val slen = (taida_val)strlen(s);
    // Overflow check: slen * n must not overflow
    if (slen > 0 && n > (taida_val)(((size_t)-1) / 2) / slen) {
        // Overflow: return empty string
        char *r = taida_str_alloc(0); return (taida_val)r;
    }
    taida_val total = slen * n;
    char *r = taida_str_alloc(total);
    for (taida_val i = 0; i < n; i++) {
        memcpy(r + i * slen, s, slen);
    }
    return (taida_val)r;
}

taida_val taida_str_reverse(const char* s) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    char *r = taida_str_alloc(len);
    for (taida_val i = 0; i < len; i++) {
        r[i] = s[len - 1 - i];
    }
    return (taida_val)r;
}

taida_val taida_str_trim_start(const char* s) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    taida_val start = 0;
    while (start < len && (s[start]==' '||s[start]=='\t'||s[start]=='\n'||s[start]=='\r')) start++;
    taida_val slen = len - start;
    char *r = taida_str_alloc(slen);
    memcpy(r, s + start, slen);
    return (taida_val)r;
}

taida_val taida_str_trim_end(const char* s) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val len = (taida_val)strlen(s);
    taida_val end = len;
    while (end > 0 && (s[end-1]==' '||s[end-1]=='\t'||s[end-1]=='\n'||s[end-1]=='\r')) end--;
    char *r = taida_str_alloc(end);
    memcpy(r, s, end);
    return (taida_val)r;
}

taida_val taida_str_replace_first(const char* s, const char* from, const char* to) {
    if (!s || !from || !to) {
        if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
        return (taida_val)taida_str_new_copy(s);
    }
    taida_val from_len = (taida_val)strlen(from);
    taida_val to_len = (taida_val)strlen(to);
    if (from_len == 0) {
        return (taida_val)taida_str_new_copy(s);
    }
    const char *found = strstr(s, from);
    if (!found) {
        return (taida_val)taida_str_new_copy(s);
    }
    taida_val s_len = (taida_val)strlen(s);
    taida_val new_len = s_len - from_len + to_len;
    char *r = taida_str_alloc(new_len);
    taida_val prefix = (taida_val)(found - s);
    memcpy(r, s, prefix);
    memcpy(r + prefix, to, to_len);
    taida_val suffix = s_len - prefix - from_len;
    memcpy(r + prefix + to_len, found + from_len, suffix);
    return (taida_val)r;
}

taida_val taida_str_pad(const char* s, taida_val target_len, const char* pad_char, taida_val pad_end) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    taida_val slen = (taida_val)strlen(s);
    if (slen >= target_len) {
        return (taida_val)taida_str_new_copy(s);
    }
    taida_val pad_len = target_len - slen;
    char pc = ' ';
    if (pad_char && strlen(pad_char) > 0) pc = pad_char[0];
    char *r = taida_str_alloc(target_len);
    if (pad_end) {
        memcpy(r, s, slen);
        for (taida_val i = 0; i < pad_len; i++) r[slen + i] = pc;
    } else {
        for (taida_val i = 0; i < pad_len; i++) r[i] = pc;
        memcpy(r + pad_len, s, slen);
    }
    return (taida_val)r;
}

// ── C12-6 (FB-5): Regex support via POSIX regex.h ─────────
//
// Value representation: `Regex("pattern", "flags")` returns a
// BuchiPack with 3 fields (pattern, flags, __type). The field hashes
// used here are FNV-1a 64-bit of the field-name literals, matching
// the Rust side of the interpreter (see `regex_eval::REGEX_TYPE_TAG`)
// and the JS runtime (`__taida_is_regex`).
//
// Philosophy: construction validates the pattern eagerly; on failure
// we fall back to an empty-match / no-op result (no null / undefined
// escape path). wasm profiles link a fallback stub that treats
// Regex args as empty strings — Regex overloads are documented as
// Native/Interpreter/JS only (see design lock §C12-6).

#include <regex.h>

#define HASH_PATTERN      0x0b7a1e4bea695f89ULL
#define HASH_FLAGS        0x17a3a1a985f75aecULL
#define HASH_FULL         0x865b7478db00e67cULL
#define HASH_GROUPS       0x6039b1239580514dULL
#define HASH_START        0xee5d97ad45ad251fULL
#define HASH_REGEX_MATCH_STR 0x3b377cd3b47dd849ULL /* FNV-1a("RegexMatch"), unused — for reference */

// Detect whether `v` points to a :Regex BuchiPack. We check the pack
// magic + scan for `__type <= "Regex"`. Returns 1 on match, 0
// otherwise (or when the pointer is not a pack). Safe for NULL /
// small integer values.
static int taida_val_is_regex_pack(taida_val v) {
    if (v <= 4096) return 0;
    if (!taida_ptr_is_readable(v, sizeof(taida_val))) return 0;
    taida_val hdr = ((taida_val*)v)[0];
    if ((hdr & TAIDA_MAGIC_MASK) != TAIDA_PACK_MAGIC) return 0;
    taida_val type_val = taida_pack_get((taida_ptr)v, (taida_val)HASH___TYPE);
    if (type_val == 0) return 0;
    size_t len = 0;
    if (!taida_read_cstr_len_safe((const char*)type_val, 64, &len)) return 0;
    if (len != 5) return 0;
    return memcmp((const char*)type_val, "Regex", 5) == 0;
}

// Extract (pattern, flags) from a validated Regex pack. Returns
// pointers into pack-owned Str payloads — caller must NOT free them.
// Each output pointer falls back to "" when the field is missing.
static void taida_regex_get_fields(taida_val regex_pack,
                                    const char **pattern_out,
                                    const char **flags_out) {
    taida_val pat = taida_pack_get((taida_ptr)regex_pack, (taida_val)HASH_PATTERN);
    taida_val flg = taida_pack_get((taida_ptr)regex_pack, (taida_val)HASH_FLAGS);
    *pattern_out = pat > 4096 ? (const char*)pat : "";
    *flags_out = flg > 4096 ? (const char*)flg : "";
}

// POSIX ERE doesn't understand Perl-style meta escapes (`\d` / `\w`
// / `\s` + complements). Translate them into POSIX character classes
// before passing the pattern to `regcomp`. This keeps the surface
// semantics identical across Interpreter (Rust `regex` crate), JS
// (RegExp), and Native (POSIX regex.h).
//
// Translation table (C12B-030):
//   \d → [0-9]             \D → [^0-9]
//   \w → [0-9A-Za-z_]      \W → [^0-9A-Za-z_]
//   \s → [ \t\n\r\f\v]     \S → [^ \t\n\r\f\v]
//   \x{HH}  / \xHH         → literal byte with hex code point
//   \u{HH..} / \uHHHH      → same, encoded as UTF-8 when > 0x7F
//
// `\\` escapes are preserved so users can match a literal backslash.
// Other `\X` sequences are passed through untouched (POSIX will
// interpret `\.` / `\(` / `\)` etc. as literals).
//
// Documented gaps (C12 supported subset — see
// `docs/reference/standard_methods.md` and `.dev/C12_DESIGN.md §C12-6`):
//   - `\b` / `\B` word boundaries: POSIX ERE has no equivalent. Users
//     who need word boundaries must target Interpreter / JS only, or
//     emulate with `(^|[^A-Za-z0-9_])` / `([^A-Za-z0-9_]|$)`.
//   - `s` flag (dotall): POSIX has no direct equivalent. `.` still
//     skips `\n` on Native; Interpreter / JS honour the flag.
//
// Caller owns the returned buffer — free with `free()`.
static char *taida_regex_rewrite_pattern(const char *pat) {
    if (!pat) {
        char *empty = (char*)TAIDA_MALLOC(1, "regex_pattern empty");
        empty[0] = '\0';
        return empty;
    }
    size_t cap = strlen(pat) * 4 + 16;
    char *out = (char*)TAIDA_MALLOC(cap, "regex_pattern rewrite");
    size_t len = 0;
    #define APPEND(s, n) do { \
        size_t _n = (n); \
        while (len + _n + 1 > cap) { cap *= 2; TAIDA_REALLOC(out, cap, "regex_pattern grow"); } \
        memcpy(out + len, (s), _n); len += _n; \
    } while(0)
    /* C12B-030 helper: parse a hex escape starting at `pat[i+1]` where
       `pat[i]` is the leading `x` or `u`. Supports both bracketed
       (`\x{HH}` / `\u{HHHH}`) and fixed-width (`\xHH` 2 digits,
       `\uHHHH` 4 digits) forms. On success, encodes the code point as
       UTF-8 into out buffer and advances `*i_inout` past the escape.
       Returns 1 on success, 0 if the escape is malformed (caller
       falls back to literal pass-through). */
    #define HEX_DIGIT(ch) (((ch) >= '0' && (ch) <= '9') ? ((ch) - '0') : \
                           ((ch) >= 'a' && (ch) <= 'f') ? ((ch) - 'a' + 10) : \
                           ((ch) >= 'A' && (ch) <= 'F') ? ((ch) - 'A' + 10) : -1)
    for (size_t i = 0; pat[i]; ) {
        char c = pat[i];
        if (c == '\\' && pat[i+1]) {
            char n = pat[i+1];
            if (n == 'd') { APPEND("[0-9]", 5); i += 2; continue; }
            if (n == 'D') { APPEND("[^0-9]", 6); i += 2; continue; }
            if (n == 'w') { APPEND("[0-9A-Za-z_]", 12); i += 2; continue; }
            if (n == 'W') { APPEND("[^0-9A-Za-z_]", 13); i += 2; continue; }
            if (n == 's') { APPEND("[ \t\n\r\f\v]", 8); i += 2; continue; }
            if (n == 'S') { APPEND("[^ \t\n\r\f\v]", 9); i += 2; continue; }
            /* C12B-030: \x{...} / \u{...} / \xHH / \uHHHH hex escapes.
               Encode the code point as UTF-8 and append the raw bytes
               so POSIX ERE sees them as literal characters. Locale is
               LC_COLLATE-agnostic here because we always emit bytes
               verbatim. */
            if (n == 'x' || n == 'u') {
                size_t j = i + 2;
                uint32_t cp = 0;
                int parsed_digits = 0;
                int ok = 0;
                if (pat[j] == '{') {
                    /* Bracketed form: up to 8 hex digits. */
                    j++;
                    while (pat[j] && pat[j] != '}' && parsed_digits < 8) {
                        int d = HEX_DIGIT(pat[j]);
                        if (d < 0) break;
                        cp = (cp << 4) | (uint32_t)d;
                        parsed_digits++;
                        j++;
                    }
                    if (pat[j] == '}' && parsed_digits > 0) {
                        j++;
                        ok = 1;
                    }
                } else {
                    /* Fixed-width form: \xHH (2) or \uHHHH (4). */
                    int needed = (n == 'x') ? 2 : 4;
                    for (int k = 0; k < needed; k++) {
                        int d = HEX_DIGIT(pat[j + k]);
                        if (d < 0) { parsed_digits = 0; break; }
                        cp = (cp << 4) | (uint32_t)d;
                        parsed_digits++;
                    }
                    if (parsed_digits == needed) {
                        j += needed;
                        ok = 1;
                    }
                }
                if (ok && cp <= 0x10FFFF) {
                    /* Encode code point as UTF-8 and emit the raw bytes. */
                    char utf[4];
                    int utf_len;
                    if (cp < 0x80) {
                        utf[0] = (char)cp;
                        utf_len = 1;
                    } else if (cp < 0x800) {
                        utf[0] = (char)(0xC0 | (cp >> 6));
                        utf[1] = (char)(0x80 | (cp & 0x3F));
                        utf_len = 2;
                    } else if (cp < 0x10000) {
                        utf[0] = (char)(0xE0 | (cp >> 12));
                        utf[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
                        utf[2] = (char)(0x80 | (cp & 0x3F));
                        utf_len = 3;
                    } else {
                        utf[0] = (char)(0xF0 | (cp >> 18));
                        utf[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
                        utf[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
                        utf[3] = (char)(0x80 | (cp & 0x3F));
                        utf_len = 4;
                    }
                    /* For bytes that have POSIX ERE meaning we need to
                       escape them. Safely wrap every byte that could be
                       a metacharacter in `[]` as a single-member class. */
                    for (int k = 0; k < utf_len; k++) {
                        unsigned char ub = (unsigned char)utf[k];
                        if (ub >= 0x80) {
                            APPEND(&utf[k], 1);
                        } else {
                            char ch = utf[k];
                            if (ch == '.' || ch == '*' || ch == '+' || ch == '?' ||
                                ch == '(' || ch == ')' || ch == '{' || ch == '}' ||
                                ch == '|' || ch == '^' || ch == '$' || ch == '[' ||
                                ch == ']' || ch == '\\' || ch == '/') {
                                char esc[2] = { '\\', ch };
                                APPEND(esc, 2);
                            } else {
                                APPEND(&utf[k], 1);
                            }
                        }
                    }
                    i = j;
                    continue;
                }
                /* Malformed hex escape — fall through and pass literally. */
            }
            // Any other backslash escape: keep as-is (including `\\`).
            APPEND(pat + i, 2);
            i += 2;
            continue;
        }
        APPEND(pat + i, 1);
        i += 1;
    }
    out[len] = '\0';
    #undef APPEND
    #undef HEX_DIGIT
    return out;
}

// Compile a POSIX regex from (pattern, flags). Flags: 'i' = icase,
// 'm' = REG_NEWLINE (line anchors), 's' = POSIX doesn't support
// dotall directly. We intentionally leave 's' unhandled at the C
// level: `.` will not cross newline on Native. This is a documented
// parity gap in the design lock; parity tests for `s` cover
// Interpreter / JS only.
//
// Returns 0 on success, non-zero on failure.
//
// NOTE: This is the "always fresh" compile path used by
// `taida_regex_new` (construction-time validation) where caching the
// probe would be semantically wrong — we must let the caller
// `regfree()` after use. The hot method paths (replace / split /
// match / search) go through `taida_regex_acquire()` below which
// caches the compiled `regex_t` across calls (C12B-036).
static int taida_regex_compile(const char *pattern, const char *flags, regex_t *out) {
    int cflags = REG_EXTENDED;
    for (const char *p = flags; p && *p; p++) {
        if (*p == 'i') cflags |= REG_ICASE;
        else if (*p == 'm') cflags |= REG_NEWLINE;
        // 's' (dotall) — not supported by POSIX ERE; silently ignored.
    }
    char *rewritten = taida_regex_rewrite_pattern(pattern);
    int rc = regcomp(out, rewritten, cflags);
    free(rewritten);
    return rc;
}

// C12B-036: POSIX `regex_t` FIFO cache shared by the hot Str-method
// paths (`replace`, `replaceAll`, `split`, `match`, `search`). Size 16
// was chosen as a pragmatic small-working-set default — large enough
// to cover typical programs that use a handful of distinct patterns
// (log parsing, tokenization, URL matching) while keeping the linear
// lookup cost negligible. Each entry owns its `regex_t` and the
// strings used to form the key; eviction calls `regfree` and `free`.
// The cache is process-wide rather than per-thread because POSIX
// `regexec` is documented thread-safe on a read-only `regex_t`, and
// the Native backend's typical workload is single-threaded (threading
// model is left to the user's code, not the runtime primitives).
//
// IMPORTANT: `taida_regex_new` (construction-time validation) still
// uses the uncached `taida_regex_compile` path to avoid polluting
// the cache with patterns that the program may never actually
// execute, and to keep probe lifetime LOCAL (we need to `regfree`
// on the error branch before throwing). Repeated construction with
// identical patterns therefore does not benefit from the cache, but
// that is intentionally off the hot path.
#define TAIDA_REGEX_CACHE_CAPACITY 16
typedef struct {
    char *pattern;    // owned copy of the input pattern (NULL => slot empty)
    char *flags;      // owned copy of the input flags   (NULL => slot empty)
    regex_t re;       // compiled regex object — valid iff active == 1
    int active;       // 0 = empty / evicted, 1 = compiled and valid
} taida_regex_cache_entry;
static taida_regex_cache_entry g_regex_cache[TAIDA_REGEX_CACHE_CAPACITY];
static int g_regex_cache_next = 0; // FIFO eviction pointer

static int _taida_str_eq_nullable(const char *a, const char *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    return strcmp(a, b) == 0;
}

// Return a borrowed pointer to a cached `regex_t` for (pattern, flags),
// compiling and inserting on miss. On compilation failure returns NULL
// — callers fall back to their "no-match" branch (e.g. return the
// input string unchanged) just as if `taida_regex_compile` had
// returned non-zero.
//
// Lifetime: the returned pointer remains valid until the slot is
// evicted by a subsequent insertion that rolls the FIFO pointer over
// to the same index. Callers must NOT `regfree` it.
static const regex_t *taida_regex_acquire(const char *pattern, const char *flags) {
    if (!pattern) pattern = "";
    if (!flags) flags = "";
    // Linear probe for a hit. Cache is small (16) so O(n) is fine.
    for (int i = 0; i < TAIDA_REGEX_CACHE_CAPACITY; i++) {
        taida_regex_cache_entry *e = &g_regex_cache[i];
        if (!e->active) continue;
        if (_taida_str_eq_nullable(e->pattern, pattern) &&
            _taida_str_eq_nullable(e->flags, flags)) {
            return &e->re;
        }
    }
    // Miss: compile into a scratch slot, then move into the FIFO next
    // position. If that position was occupied, evict it first.
    regex_t fresh;
    if (taida_regex_compile(pattern, flags, &fresh) != 0) {
        return NULL;
    }
    int idx = g_regex_cache_next;
    g_regex_cache_next = (g_regex_cache_next + 1) % TAIDA_REGEX_CACHE_CAPACITY;
    taida_regex_cache_entry *slot = &g_regex_cache[idx];
    if (slot->active) {
        regfree(&slot->re);
        if (slot->pattern) { free(slot->pattern); slot->pattern = NULL; }
        if (slot->flags)   { free(slot->flags);   slot->flags = NULL; }
        slot->active = 0;
    }
    size_t plen = strlen(pattern);
    size_t flen = strlen(flags);
    slot->pattern = (char*)malloc(plen + 1);
    slot->flags   = (char*)malloc(flen + 1);
    if (!slot->pattern || !slot->flags) {
        // OOM on the metadata — free the freshly compiled regex and
        // bail. Callers see this as a miss that produces NULL.
        regfree(&fresh);
        if (slot->pattern) { free(slot->pattern); slot->pattern = NULL; }
        if (slot->flags)   { free(slot->flags);   slot->flags = NULL; }
        return NULL;
    }
    memcpy(slot->pattern, pattern, plen + 1);
    memcpy(slot->flags, flags, flen + 1);
    slot->re = fresh;
    slot->active = 1;
    return &slot->re;
}

// Construct a Regex BuchiPack from (pattern_str, flags_str). Field
// layout matches the interpreter / JS representation.
//
// C12B-029: Validate fail-fast at construction time — mirror the
// Interpreter (`src/interpreter/regex_eval.rs::build_regex_value`)
// and JS (`new RegExp(...)` throw) behaviour so the 3 backends share
// the same failure mode:
//   1. Reject unsupported flags (anything other than `i` / `m` / `s`).
//   2. Compile the (rewritten) pattern once; on POSIX regcomp failure,
//      throw a :RegexError via the error ceiling rather than returning
//      a usable pack that silently no-ops at the first method call.
taida_val taida_regex_new(const char *pattern_s, const char *flags_s) {
    if (!pattern_s) pattern_s = "";
    if (!flags_s) flags_s = "";
    // C12B-029: Flag validation. POSIX can only honour i/m directly,
    // but the Interpreter / JS layer accepts `s` as well. Other flags
    // must be rejected at construction time on every backend.
    for (const char *fp = flags_s; *fp; fp++) {
        char ch = *fp;
        if (ch != 'i' && ch != 'm' && ch != 's') {
            char buf[160];
            snprintf(buf, sizeof(buf),
                "Regex: unsupported flag '%c'. Supported flags: i (case-insensitive), m (multiline), s (dotall)",
                ch);
            taida_val err = taida_make_error("ValueError", buf);
            return taida_throw(err);
        }
    }
    // C12B-029: Pattern validation. Compile the rewritten pattern
    // once so that malformed input (unbalanced parens, stray
    // quantifiers, etc.) fails at `Regex(...)` instead of silently
    // returning the input string at each Str-method call site.
    {
        regex_t probe;
        int cflags = REG_EXTENDED;
        for (const char *fp = flags_s; *fp; fp++) {
            if (*fp == 'i') cflags |= REG_ICASE;
            else if (*fp == 'm') cflags |= REG_NEWLINE;
        }
        char *rewritten = taida_regex_rewrite_pattern(pattern_s);
        int rc = regcomp(&probe, rewritten ? rewritten : "", cflags);
        if (rewritten) free(rewritten);
        if (rc != 0) {
            // Mirror POSIX regerror: callable with the failing regex_t
            // to produce a descriptive message. We skip regfree on the
            // error path because regfree on a not-successfully-compiled
            // regex_t is unspecified behaviour.
            char errbuf[96];
            (void)regerror(rc, &probe, errbuf, sizeof(errbuf));
            char msg[256];
            snprintf(msg, sizeof(msg),
                "Regex: invalid pattern '%s' — %s", pattern_s, errbuf);
            taida_val err = taida_make_error("ValueError", msg);
            return taida_throw(err);
        }
        regfree(&probe);
    }
    taida_val pack = taida_pack_new(3);
    taida_pack_set_hash((taida_ptr)pack, 0, (taida_val)HASH_PATTERN);
    taida_pack_set_hash((taida_ptr)pack, 1, (taida_val)HASH_FLAGS);
    taida_pack_set_hash((taida_ptr)pack, 2, (taida_val)HASH___TYPE);
    taida_val pat_str = (taida_val)taida_str_new_copy(pattern_s);
    taida_val flg_str = (taida_val)taida_str_new_copy(flags_s);
    taida_val type_str = (taida_val)taida_str_new_copy("Regex");
    taida_pack_set((taida_ptr)pack, 0, pat_str);
    taida_pack_set_tag((taida_ptr)pack, 0, TAIDA_TAG_STR);
    taida_pack_set((taida_ptr)pack, 1, flg_str);
    taida_pack_set_tag((taida_ptr)pack, 1, TAIDA_TAG_STR);
    taida_pack_set((taida_ptr)pack, 2, type_str);
    taida_pack_set_tag((taida_ptr)pack, 2, TAIDA_TAG_STR);
    return pack;
}

// Apply Regex replace_first: build the output by copying the prefix,
// the literal replacement (no `$N` expansion), and the suffix from
// the first regex match. Returns a freshly allocated taida_str.
static taida_val taida_regex_replace_first_impl(const char *s,
                                                 const char *pattern,
                                                 const char *flags,
                                                 const char *replacement) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    if (!replacement) replacement = "";
    // C12B-036: shared compiled regex from cache; do not regfree.
    const regex_t *re = taida_regex_acquire(pattern, flags);
    if (!re) {
        return (taida_val)taida_str_new_copy(s);
    }
    regmatch_t m;
    if (regexec(re, s, 1, &m, 0) != 0 || m.rm_so < 0) {
        return (taida_val)taida_str_new_copy(s);
    }
    size_t prefix_len = (size_t)m.rm_so;
    size_t match_len = (size_t)(m.rm_eo - m.rm_so);
    size_t rep_len = strlen(replacement);
    size_t suffix_len = strlen(s) - prefix_len - match_len;
    size_t out_len = prefix_len + rep_len + suffix_len;
    char *out = taida_str_alloc(out_len);
    memcpy(out, s, prefix_len);
    memcpy(out + prefix_len, replacement, rep_len);
    memcpy(out + prefix_len + rep_len, s + prefix_len + match_len, suffix_len);
    return (taida_val)out;
}

// Apply Regex replace_all: iterate regexec with REG_NOTBOL on
// subsequent calls so `^` only anchors at the very start.
static taida_val taida_regex_replace_all_impl(const char *s,
                                               const char *pattern,
                                               const char *flags,
                                               const char *replacement) {
    if (!s) { char *r = taida_str_alloc(0); return (taida_val)r; }
    if (!replacement) replacement = "";
    // C12B-036: shared compiled regex from cache; do not regfree.
    const regex_t *re = taida_regex_acquire(pattern, flags);
    if (!re) {
        return (taida_val)taida_str_new_copy(s);
    }
    // Build output into a growing buffer.
    size_t cap = 64;
    size_t len = 0;
    char *out = (char*)TAIDA_MALLOC(cap, "regex_replace_all");
    size_t rep_len = strlen(replacement);
    const char *cursor = s;
    int eflags = 0;
    // Safety bound: number of iterations cannot exceed input byte
    // length + 1 (each zero-width match advances by one byte).
    size_t max_iters = strlen(s) + 1;
    for (size_t iter = 0; iter < max_iters; iter++) {
        regmatch_t m;
        if (regexec(re, cursor, 1, &m, eflags) != 0 || m.rm_so < 0) {
            // Copy remaining tail.
            size_t tail = strlen(cursor);
            while (len + tail + 1 > cap) { cap *= 2; TAIDA_REALLOC(out, cap, "regex_replace_all grow"); }
            memcpy(out + len, cursor, tail);
            len += tail;
            break;
        }
        size_t prefix = (size_t)m.rm_so;
        size_t match_len = (size_t)(m.rm_eo - m.rm_so);
        // Append prefix.
        while (len + prefix + 1 > cap) { cap *= 2; TAIDA_REALLOC(out, cap, "regex_replace_all grow"); }
        memcpy(out + len, cursor, prefix);
        len += prefix;
        // Append replacement.
        while (len + rep_len + 1 > cap) { cap *= 2; TAIDA_REALLOC(out, cap, "regex_replace_all grow"); }
        memcpy(out + len, replacement, rep_len);
        len += rep_len;
        // Advance cursor. On zero-width matches, bump one byte to
        // avoid an infinite loop.
        if (match_len == 0) {
            if (cursor[prefix] == '\0') break;
            while (len + 1 + 1 > cap) { cap *= 2; TAIDA_REALLOC(out, cap, "regex_replace_all grow"); }
            out[len++] = cursor[prefix];
            cursor += prefix + 1;
        } else {
            cursor += prefix + match_len;
        }
        eflags = REG_NOTBOL;
    }
    // C12B-036: compiled regex is cache-owned; do not regfree.
    // Copy into a taida-owned Str buffer.
    char *taida_out = taida_str_alloc(len);
    memcpy(taida_out, out, len);
    free(out);
    return (taida_val)taida_out;
}

// Apply Regex split: emit the text between successive matches.
static taida_val taida_regex_split_impl(const char *s,
                                         const char *pattern,
                                         const char *flags) {
    taida_val list = taida_list_new();
    taida_list_set_elem_tag(list, TAIDA_TAG_STR);
    if (!s) return list;
    // C12B-036: shared compiled regex from cache; do not regfree.
    const regex_t *re = taida_regex_acquire(pattern, flags);
    if (!re) {
        // Compile failed — return a single-element list with the
        // original string (matches interpreter fallback via `split`
        // on an unmatched pattern: whole-string list).
        char *copy = (char*)taida_str_new_copy(s);
        list = taida_list_push(list, (taida_val)copy);
        return list;
    }
    const char *cursor = s;
    int eflags = 0;
    size_t max_iters = strlen(s) + 1;
    for (size_t iter = 0; iter < max_iters; iter++) {
        regmatch_t m;
        if (regexec(re, cursor, 1, &m, eflags) != 0 || m.rm_so < 0) {
            // Tail.
            size_t tail = strlen(cursor);
            char *piece = taida_str_alloc(tail);
            memcpy(piece, cursor, tail);
            list = taida_list_push(list, (taida_val)piece);
            break;
        }
        size_t prefix = (size_t)m.rm_so;
        size_t match_len = (size_t)(m.rm_eo - m.rm_so);
        char *piece = taida_str_alloc(prefix);
        memcpy(piece, cursor, prefix);
        list = taida_list_push(list, (taida_val)piece);
        if (match_len == 0) {
            if (cursor[prefix] == '\0') break;
            cursor += prefix + 1;
        } else {
            cursor += prefix + match_len;
        }
        eflags = REG_NOTBOL;
    }
    // C12B-036: compiled regex is cache-owned; do not regfree.
    return list;
}

// Convert a byte offset within `s` to a character (codepoint) offset.
// UTF-8 aware; returns -1 if the input is malformed up to `byte_off`.
static taida_val taida_bytes_to_chars_offset(const char *s, size_t byte_off) {
    if (!s) return -1;
    size_t offset = 0;
    taida_val chars = 0;
    const unsigned char *buf = (const unsigned char*)s;
    while (offset < byte_off) {
        size_t consumed = 0;
        uint32_t cp = 0;
        if (!taida_utf8_decode_one(buf + offset, byte_off - offset, &consumed, &cp) || consumed == 0) {
            // Fallback: 1 byte = 1 "char" on malformed input.
            offset += 1;
        } else {
            offset += consumed;
        }
        chars += 1;
    }
    return chars;
}

// Build a :RegexMatch BuchiPack (`hasValue`, `full`, `groups`,
// `start`, `__type <= "RegexMatch"`). `start` is the char index of
// the first match (not the byte index), matching the JS helper.
static taida_val taida_regex_build_match_value(int matched,
                                                const char *full,
                                                taida_val start_chars,
                                                taida_val groups_list) {
    static uint64_t HASH_has_value_local = 0x9e9c6dc733414d60ULL; // HASH_HAS_VALUE
    taida_val pack = taida_pack_new(5);
    taida_pack_set_hash((taida_ptr)pack, 0, (taida_val)HASH_has_value_local);
    taida_pack_set_hash((taida_ptr)pack, 1, (taida_val)HASH_FULL);
    taida_pack_set_hash((taida_ptr)pack, 2, (taida_val)HASH_GROUPS);
    taida_pack_set_hash((taida_ptr)pack, 3, (taida_val)HASH_START);
    taida_pack_set_hash((taida_ptr)pack, 4, (taida_val)HASH___TYPE);
    taida_pack_set((taida_ptr)pack, 0, (taida_val)(matched ? 1 : 0));
    taida_pack_set_tag((taida_ptr)pack, 0, TAIDA_TAG_BOOL);
    taida_val full_str = (taida_val)taida_str_new_copy(full ? full : "");
    taida_pack_set((taida_ptr)pack, 1, full_str);
    taida_pack_set_tag((taida_ptr)pack, 1, TAIDA_TAG_STR);
    // groups list was already constructed by the caller via
    // taida_list_new / taida_list_push; retain happens implicitly
    // via our pack_set below.
    taida_pack_set((taida_ptr)pack, 2, groups_list);
    taida_pack_set_tag((taida_ptr)pack, 2, TAIDA_TAG_LIST);
    taida_pack_set((taida_ptr)pack, 3, start_chars);
    taida_pack_set_tag((taida_ptr)pack, 3, TAIDA_TAG_INT);
    taida_val type_str = (taida_val)taida_str_new_copy("RegexMatch");
    taida_pack_set((taida_ptr)pack, 4, type_str);
    taida_pack_set_tag((taida_ptr)pack, 4, TAIDA_TAG_STR);
    return pack;
}

// ── Polymorphic dispatchers — called by lowered Str methods ────
// (C12-6c): the 2nd arg is either a Str pointer or a :Regex pack.

taida_val taida_str_split_poly(const char *s, taida_val sep) {
    if (taida_val_is_regex_pack(sep)) {
        const char *pat, *flg;
        taida_regex_get_fields(sep, &pat, &flg);
        return taida_regex_split_impl(s ? s : "", pat, flg);
    }
    return taida_str_split(s, (const char*)sep);
}

taida_val taida_str_replace_first_poly(const char *s, taida_val target, const char *rep) {
    if (taida_val_is_regex_pack(target)) {
        const char *pat, *flg;
        taida_regex_get_fields(target, &pat, &flg);
        return taida_regex_replace_first_impl(s ? s : "", pat, flg, rep ? rep : "");
    }
    return taida_str_replace_first(s, (const char*)target, rep);
}

taida_val taida_str_replace_poly(const char *s, taida_val target, const char *rep) {
    if (taida_val_is_regex_pack(target)) {
        const char *pat, *flg;
        taida_regex_get_fields(target, &pat, &flg);
        return taida_regex_replace_all_impl(s ? s : "", pat, flg, rep ? rep : "");
    }
    return taida_str_replace(s, (const char*)target, rep);
}

taida_val taida_str_match_regex(const char *s, taida_val regex_pack) {
    // Build default empty-match pack up front so both failure paths
    // emit the same shape.
    if (!s || !taida_val_is_regex_pack(regex_pack)) {
        taida_val empty_list = taida_list_new();
        taida_list_set_elem_tag(empty_list, TAIDA_TAG_STR);
        return taida_regex_build_match_value(0, "", -1, empty_list);
    }
    const char *pat, *flg;
    taida_regex_get_fields(regex_pack, &pat, &flg);
    // C12B-036: shared compiled regex from cache; do not regfree.
    const regex_t *re = taida_regex_acquire(pat, flg);
    if (!re) {
        taida_val empty_list = taida_list_new();
        taida_list_set_elem_tag(empty_list, TAIDA_TAG_STR);
        return taida_regex_build_match_value(0, "", -1, empty_list);
    }
    // Allow up to 16 capture groups (design lock says no PCRE
    // look-around; 16 groups is ample for Phase 2-3 scope).
    regmatch_t matches[16];
    if (regexec(re, s, 16, matches, 0) != 0 || matches[0].rm_so < 0) {
        taida_val empty_list = taida_list_new();
        taida_list_set_elem_tag(empty_list, TAIDA_TAG_STR);
        return taida_regex_build_match_value(0, "", -1, empty_list);
    }
    size_t full_len = (size_t)(matches[0].rm_eo - matches[0].rm_so);
    char *full_buf = (char*)TAIDA_MALLOC(full_len + 1, "regex_match full");
    memcpy(full_buf, s + matches[0].rm_so, full_len);
    full_buf[full_len] = '\0';
    taida_val start_chars = taida_bytes_to_chars_offset(s, (size_t)matches[0].rm_so);
    // Groups list.
    taida_val groups_list = taida_list_new();
    taida_list_set_elem_tag(groups_list, TAIDA_TAG_STR);
    for (int i = 1; i < 16; i++) {
        if (matches[i].rm_so < 0) {
            // No more groups available — but we must keep pushing
            // empty strings for missing groups only if there are
            // *registered* groups at position i. POSIX re_nsub
            // indicates number of sub-expressions. Stop at the first
            // rm_so < 0 after re_nsub boundary.
            if ((size_t)i > re->re_nsub) break;
            char *empty = taida_str_alloc(0);
            groups_list = taida_list_push(groups_list, (taida_val)empty);
            continue;
        }
        size_t gl = (size_t)(matches[i].rm_eo - matches[i].rm_so);
        char *g = taida_str_alloc(gl);
        memcpy(g, s + matches[i].rm_so, gl);
        groups_list = taida_list_push(groups_list, (taida_val)g);
    }
    // C12B-036: compiled regex is cache-owned; do not regfree.
    taida_val out = taida_regex_build_match_value(1, full_buf, start_chars, groups_list);
    free(full_buf);
    return out;
}

taida_val taida_str_search_regex(const char *s, taida_val regex_pack) {
    if (!s || !taida_val_is_regex_pack(regex_pack)) return -1;
    const char *pat, *flg;
    taida_regex_get_fields(regex_pack, &pat, &flg);
    // C12B-036: shared compiled regex from cache; do not regfree.
    const regex_t *re = taida_regex_acquire(pat, flg);
    if (!re) return -1;
    regmatch_t m;
    if (regexec(re, s, 1, &m, 0) != 0 || m.rm_so < 0) {
        return -1;
    }
    taida_val chars = taida_bytes_to_chars_offset(s, (size_t)m.rm_so);
    return chars;
}

// ── Template string (sprintf-based) ──────────────────────
// Format: "Hello, {0}! You are {1}." with args as variadic
// We use a simpler approach: taida_template_concat builds result from parts and values
taida_val taida_str_from_int(taida_val v) {
    char tmp[32];
    snprintf(tmp, sizeof(tmp), "%" PRId64 "", v);
    return (taida_val)taida_str_new_copy(tmp);
}

taida_val taida_str_from_float(double v) {
    char tmp[64];
    snprintf(tmp, sizeof(tmp), "%g", v);
    return (taida_val)taida_str_new_copy(tmp);
}

taida_val taida_str_from_bool(taida_val v) {
    return (taida_val)taida_str_new_copy(v ? "true" : "false");
}

// ── Int methods ───────────────────────────────────────────
taida_val taida_int_abs(taida_val a) { return a < 0 ? -a : a; }

taida_val taida_int_to_str(taida_val a) {
    char tmp[32];
    snprintf(tmp, sizeof(tmp), "%" PRId64 "", a);
    return (taida_val)taida_str_new_copy(tmp);
}

taida_val taida_int_to_float(taida_val a) {
    double d = (double)a;
    taida_val result;
    memcpy(&result, &d, sizeof(taida_val));
    return result;
}

taida_val taida_int_clamp(taida_val a, taida_val lo, taida_val hi) {
    if (a < lo) return lo;
    if (a > hi) return hi;
    return a;
}

// ── Float methods ────────────────────────────────────────
double taida_float_floor(double a) { return floor(a); }
double taida_float_ceil(double a) { return ceil(a); }
double taida_float_round(double a) { return round(a); }
double taida_float_abs(double a) { return a < 0 ? -a : a; }

taida_val taida_float_to_int(double a) { return (taida_val)a; }

taida_val taida_float_to_str(double a) {
    // C21-4: Match the interpreter's Rust-f64::Display contract — integer
    // values render as "X.0" (not "X"), non-integers use the **shortest**
    // decimal representation that round-trips back to the same f64.
    // This mirrors Rust's Grisu/Ryu-based `f64::to_string` which Taida's
    // interpreter delegates to via `n.to_string()`. The previous `%g`
    // dropped the trailing ".0" and sometimes lost precision; switching
    // naively to `%.17g` added spurious trailing digits (symptom:
    // `3.14` → `3.1400000000000001`). The loop below picks the shortest
    // precision that survives a round-trip, matching Rust's behaviour
    // for every double in practice while remaining self-contained.
    char tmp[64];
    if (isnan(a)) { snprintf(tmp, sizeof(tmp), "NaN"); }
    else if (isinf(a)) { snprintf(tmp, sizeof(tmp), a < 0 ? "-inf" : "inf"); }
    else if (a == 0.0) { snprintf(tmp, sizeof(tmp), "0.0"); }
    else if (a == floor(a) && fabs(a) < 1e16) {
        // Integer-valued float in the exact range — always "X.0".
        snprintf(tmp, sizeof(tmp), "%.1f", a);
    } else {
        // Find the smallest precision p in [1..17] such that
        // `strtod(sprintf("%.*g", p, a))` round-trips. That is the
        // shortest decimal form, matching Rust's f64::Display.
        int chosen = 17;
        for (int p = 1; p <= 17; p++) {
            snprintf(tmp, sizeof(tmp), "%.*g", p, a);
            double back = strtod(tmp, NULL);
            if (back == a) { chosen = p; break; }
        }
        if (chosen != 17) {
            // tmp already holds the chosen rendering.
        } else {
            snprintf(tmp, sizeof(tmp), "%.17g", a);
        }
        // If the output lacks a '.' (e.g. `%g` elided the fraction on
        // an integer-looking float outside the above range), append ".0"
        // so the Rust-style invariant holds.
        if (!strchr(tmp, '.') && !strchr(tmp, 'e') && !strchr(tmp, 'E')
            && !strchr(tmp, 'n') && !strchr(tmp, 'i')) {
            size_t l = strlen(tmp);
            if (l + 2 < sizeof(tmp)) {
                tmp[l] = '.'; tmp[l+1] = '0'; tmp[l+2] = '\0';
            }
        }
    }
    return (taida_val)taida_str_new_copy(tmp);
}

taida_val taida_float_to_fixed(double a, taida_val digits) {
    char tmp[64];
    snprintf(tmp, sizeof(tmp), "%.*f", (int)digits, a);
    return (taida_val)taida_str_new_copy(tmp);
}

double taida_float_clamp(double a, double lo, double hi) {
    if (a < lo) return lo;
    if (a > hi) return hi;
    return a;
}

// ── Math mold family (C25B-025 Phase 5-I, 2026-04-23) ────
//
// Native implementations for the math mold family added to the
// interpreter + JS in Phase 5-A (commit 86d5743). Native delegates
// to glibc libm, which Rust's `f64::sqrt` / `f64::exp` / ... also
// call through the LLVM `@llvm.*.f64` intrinsics on x86_64-linux
// and aarch64-linux, giving us bit-for-bit parity with the
// interpreter on all practical Linux / macOS targets. NaN /
// ±Infinity / denormal are preserved by libm.
//
// Accept `double` only — the lowering (`src/codegen/lower_molds.rs`)
// widens Int arguments to Float via `taida_int_to_float` before the
// call (same way the interpreter's `eval_unary_math` widens Int to
// f64). `Pow[Int, Int]` returns Float per `src/types/mold_returns.rs`.

double taida_float_sqrt(double a) { return sqrt(a); }
double taida_float_pow(double base, double e) { return pow(base, e); }
double taida_float_exp(double a) { return exp(a); }
double taida_float_ln(double a) { return log(a); }
double taida_float_log2(double a) { return log2(a); }
double taida_float_log10(double a) { return log10(a); }
// `Log[value, base]()` — `value` log of `base`. Matches interpreter's
// `val.log(base)` which is `ln(val) / ln(base)`.
double taida_float_log(double value, double base) {
    return log(value) / log(base);
}
double taida_float_sin(double a) { return sin(a); }
double taida_float_cos(double a) { return cos(a); }
double taida_float_tan(double a) { return tan(a); }
double taida_float_asin(double a) { return asin(a); }
double taida_float_acos(double a) { return acos(a); }
double taida_float_atan(double a) { return atan(a); }
double taida_float_atan2(double y, double x) { return atan2(y, x); }
double taida_float_sinh(double a) { return sinh(a); }
double taida_float_cosh(double a) { return cosh(a); }
double taida_float_tanh(double a) { return tanh(a); }

// ── Num state check methods ──────────────────────────────
// For Int: isNaN=false, isInfinite=false, isFinite=true always
// For Float: need actual NaN/Inf checks

taida_val taida_int_is_positive(taida_val a) { return a > 0 ? 1 : 0; }
taida_val taida_int_is_negative(taida_val a) { return a < 0 ? 1 : 0; }
taida_val taida_int_is_zero(taida_val a) { return a == 0 ? 1 : 0; }

taida_val taida_float_is_nan(double a) { return isnan(a) ? 1 : 0; }
taida_val taida_float_is_infinite(double a) { return isinf(a) ? 1 : 0; }
taida_val taida_float_is_finite_check(double a) { return isfinite(a) ? 1 : 0; }
taida_val taida_float_is_positive(double a) { return a > 0.0 ? 1 : 0; }
taida_val taida_float_is_negative(double a) { return a < 0.0 ? 1 : 0; }
taida_val taida_float_is_zero(double a) { return a == 0.0 ? 1 : 0; }

// ── Bool methods ──────────────────────────────────────────
taida_val taida_bool_to_str(taida_val a) {
    return (taida_val)taida_str_new_copy(a ? "true" : "false");
}

taida_val taida_bool_to_int(taida_val a) { return a ? 1 : 0; }

// ── Additional List methods ──────────────────────────────
taida_val taida_list_index_of(taida_val list_ptr, taida_val item) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        if (list[4 + i] == item) return i;
    }
    return -1;
}

taida_val taida_list_last_index_of(taida_val list_ptr, taida_val item) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = len - 1; i >= 0; i--) {
        if (list[4 + i] == item) return i;
    }
    return -1;
}

taida_val taida_list_any(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[4 + i])) return 1;
    }
    return 0;
}

taida_val taida_list_all(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        if (!taida_invoke_callback1(fn_ptr, list[4 + i])) return 0;
    }
    return 1;
}

taida_val taida_list_none(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[4 + i])) return 0;
    }
    return 1;
}

taida_val taida_list_concat(taida_val list1, taida_val list2) {
    if (TAIDA_IS_BYTES(list1) && TAIDA_IS_BYTES(list2)) {
        taida_val len1 = taida_bytes_len(list1);
        taida_val len2 = taida_bytes_len(list2);
        taida_val out = taida_bytes_new_filled(len1 + len2, 0);
        taida_val *dst = (taida_val*)out;
        taida_val *a = (taida_val*)list1;
        taida_val *b = (taida_val*)list2;
        for (taida_val i = 0; i < len1; i++) dst[2 + i] = a[2 + i];
        for (taida_val i = 0; i < len2; i++) dst[2 + len1 + i] = b[2 + i];
        return out;
    }

    taida_val *l1 = (taida_val*)list1;
    taida_val *l2 = (taida_val*)list2;
    taida_val len1 = l1[2], len2 = l2[2];
    taida_val elem_tag = l1[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag from first list
    for (taida_val i = 0; i < len1; i++) {
        taida_list_elem_retain(l1[4 + i], elem_tag);
        new_list = taida_list_push(new_list, l1[4 + i]);
    }
    for (taida_val i = 0; i < len2; i++) {
        taida_list_elem_retain(l2[4 + i], elem_tag);
        new_list = taida_list_push(new_list, l2[4 + i]);
    }
    return new_list;
}

taida_val taida_list_join(taida_val list_ptr, const char* sep) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (len == 0) { char *r = taida_str_alloc(0); return (taida_val)r; }
    if (!sep) sep = "";
    size_t sep_len = strlen(sep);

    // Convert each element through the shared toString path.
    // This avoids pointer heuristics and keeps behavior consistent.
    // M-06: Overflow guard on len * sizeof + NULL check.
    size_t strs_size = taida_safe_mul((size_t)len, sizeof(const char*), "list_join strs");
    const char **strs = (const char**)TAIDA_MALLOC(strs_size, "list_join_strs");
    // M-16: Use size_t for total with overflow guards to prevent wrap-around.
    size_t total = 0;
    for (taida_val i = 0; i < len; i++) {
        strs[i] = (const char*)taida_value_to_display_string(list[4 + i]);
        total = taida_safe_add(total, strlen(strs[i]), "list_join total");
        if (i > 0) total = taida_safe_add(total, sep_len, "list_join sep");
    }

    char *r = taida_str_alloc(total);
    char *dst = r;
    for (taida_val i = 0; i < len; i++) {
        if (i > 0 && sep_len > 0) { memcpy(dst, sep, sep_len); dst += sep_len; }
        taida_val sl = (taida_val)strlen(strs[i]);
        memcpy(dst, strs[i], sl);
        dst += sl;
    }
    *dst = '\0';

    // Free temporary strings
    for (taida_val i = 0; i < len; i++) {
        taida_str_release((taida_val)strs[i]);
    }
    free(strs);

    return (taida_val)r;
}

taida_val taida_list_sort(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    if (len == 0) return new_list;
    // M-07: Overflow guard + NULL check on items allocation.
    size_t items_size = taida_safe_mul((size_t)len, sizeof(taida_val), "list_sort items");
    taida_val *items = (taida_val*)TAIDA_MALLOC(items_size, "list_sort");
    for (taida_val i = 0; i < len; i++) items[i] = list[4 + i];
    // Simple insertion sort
    for (taida_val i = 1; i < len; i++) {
        taida_val key = items[i];
        taida_val j = i - 1;
        while (j >= 0 && items[j] > key) { items[j+1] = items[j]; j--; }
        items[j+1] = key;
    }
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(items[i], elem_tag);
        new_list = taida_list_push(new_list, items[i]);
    }
    free(items);
    return new_list;
}

taida_val taida_list_unique(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl_init = (taida_val*)new_list;
    nl_init[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        // Check if already in new_list
        taida_val *nl = (taida_val*)new_list;
        taida_val nlen = nl[2];
        taida_val found = 0;
        for (taida_val j = 0; j < nlen; j++) {
            if (nl[4 + j] == item) { found = 1; break; }
        }
        if (!found) {
            taida_list_elem_retain(item, elem_tag);
            new_list = taida_list_push(new_list, item);
        }
    }
    return new_list;
}

taida_val taida_list_flatten(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val new_list = taida_list_new();
    // flatten changes nesting level, propagate inner list's elem_tag if possible
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (TAIDA_IS_LIST(item)) {
            taida_val *sub = (taida_val*)item;
            taida_val slen = sub[2];
            taida_val sub_tag = sub[3];
            // Propagate inner list's elem_tag to result
            if (i == 0) {
                taida_val *nl = (taida_val*)new_list;
                nl[3] = sub_tag;
            }
            for (taida_val j = 0; j < slen; j++) {
                taida_list_elem_retain(sub[4 + j], sub_tag);
                new_list = taida_list_push(new_list, sub[4 + j]);
            }
        } else {
            // Non-list element: retain using outer list's elem_tag
            taida_list_elem_retain(item, list[3]);
            new_list = taida_list_push(new_list, item);
        }
    }
    return new_list;
}

taida_val taida_list_max(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (len == 0) return taida_lax_empty(0);
    taida_val max = list[4];
    for (taida_val i = 1; i < len; i++) {
        if (list[4 + i] > max) max = list[4 + i];
    }
    return taida_lax_new(max, 0);
}

taida_val taida_list_min(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (len == 0) return taida_lax_empty(0);
    taida_val min = list[4];
    for (taida_val i = 1; i < len; i++) {
        if (list[4 + i] < min) min = list[4 + i];
    }
    return taida_lax_new(min, 0);
}

// ── Additional List mold operations ──────────────────────

taida_val taida_list_append(taida_val list_ptr, taida_val item) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    // New item: no retain (ownership transferred from caller)
    new_list = taida_list_push(new_list, item);
    return new_list;
}

taida_val taida_list_prepend(taida_val list_ptr, taida_val item) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    // New item: no retain (ownership transferred from caller)
    new_list = taida_list_push(new_list, item);
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    return new_list;
}

taida_val taida_list_sort_desc(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    if (len == 0) return new_list;
    // M-07: Overflow guard + NULL check on items allocation.
    size_t items_size = taida_safe_mul((size_t)len, sizeof(taida_val), "list_sort_desc items");
    taida_val *items = (taida_val*)TAIDA_MALLOC(items_size, "list_sort_desc");
    for (taida_val i = 0; i < len; i++) items[i] = list[4 + i];
    // Insertion sort descending
    for (taida_val i = 1; i < len; i++) {
        taida_val key = items[i];
        taida_val j = i - 1;
        while (j >= 0 && items[j] < key) { items[j+1] = items[j]; j--; }
        items[j+1] = key;
    }
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(items[i], elem_tag);
        new_list = taida_list_push(new_list, items[i]);
    }
    free(items);
    return new_list;
}

/* Sort by key extraction function: fn_ptr maps each element to a sort key,
   then sort ascending by key. Matches interpreter's Sort[list](by <= fn). */
taida_val taida_list_sort_by(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;
    if (len == 0) return new_list;
    size_t items_size = taida_safe_mul((size_t)len, sizeof(taida_val), "list_sort_by items");
    taida_val *items = (taida_val*)TAIDA_MALLOC(items_size, "list_sort_by items");
    taida_val *keys = (taida_val*)TAIDA_MALLOC(items_size, "list_sort_by keys");
    for (taida_val i = 0; i < len; i++) {
        items[i] = list[4 + i];
        keys[i] = taida_invoke_callback1(fn_ptr, items[i]);
    }
    /* Insertion sort ascending by key */
    for (taida_val i = 1; i < len; i++) {
        taida_val kkey = keys[i];
        taida_val kitem = items[i];
        taida_val j = i - 1;
        while (j >= 0 && keys[j] > kkey) {
            keys[j+1] = keys[j];
            items[j+1] = items[j];
            j--;
        }
        keys[j+1] = kkey;
        items[j+1] = kitem;
    }
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(items[i], elem_tag);
        new_list = taida_list_push(new_list, items[i]);
    }
    free(items);
    free(keys);
    return new_list;
}

taida_val taida_list_find(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (taida_invoke_callback1(fn_ptr, item)) {
            return taida_lax_new(item, 0);
        }
    }
    return taida_lax_empty(0);
}

taida_val taida_list_find_index(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[4 + i])) return i;
    }
    return -1;
}

taida_val taida_list_count(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val count = 0;
    for (taida_val i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[4 + i])) count++;
    }
    return count;
}

// list.fold(init, fn) — left fold: fn takes (acc, item) -> acc
taida_val taida_list_fold(taida_val list_ptr, taida_val init, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val acc = init;
    for (taida_val i = 0; i < len; i++) {
        acc = taida_invoke_callback2(fn_ptr, acc, list[4 + i]);
    }
    return acc;
}

// list.foldr(init, fn) — right fold: fn takes (acc, item) -> acc
taida_val taida_list_foldr(taida_val list_ptr, taida_val init, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val acc = init;
    for (taida_val i = len - 1; i >= 0; i--) {
        acc = taida_invoke_callback2(fn_ptr, acc, list[4 + i]);
    }
    return acc;
}

// Take[list, n]() — first n elements
taida_val taida_list_take(taida_val list_ptr, taida_val n) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val take_n = n < len ? n : len;
    if (take_n < 0) take_n = 0;
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = 0; i < take_n; i++) {
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    return new_list;
}

// TakeWhile[list, fn]() — take while fn returns truthy
taida_val taida_list_take_while(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[4 + i])) {
            taida_list_elem_retain(list[4 + i], elem_tag);
            new_list = taida_list_push(new_list, list[4 + i]);
        } else {
            break;
        }
    }
    return new_list;
}

// Drop[list, n]() — skip first n elements
taida_val taida_list_drop(taida_val list_ptr, taida_val n) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val skip = n < len ? n : len;
    if (skip < 0) skip = 0;
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    for (taida_val i = skip; i < len; i++) {
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    return new_list;
}

// DropWhile[list, fn]() — skip while fn returns truthy
taida_val taida_list_drop_while(taida_val list_ptr, taida_val fn_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = elem_tag;  // propagate elem_type_tag
    taida_val dropping = 1;
    for (taida_val i = 0; i < len; i++) {
        if (dropping && taida_invoke_callback1(fn_ptr, list[4 + i])) {
            continue;
        }
        dropping = 0;
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    return new_list;
}

// FNV-1a hashes for Zip/Enumerate BuchiPack fields
#define HASH_FIRST  0x89d7ed7f996f1d41ULL
#define HASH_SECOND 0xa49985ef4cee20bdULL
#define HASH_INDEX  0x83cf8e8f9081468bULL
#define HASH_VALUE  0x7ce4fd9430e80ceaULL

// C24-B (2026-04-23): Register zip/enumerate pair-pack field names into
// the global field-name registry so `taida_pack_to_display_string_full`
// emits `first <= …, second <= …` / `index <= …, value <= …`. Without
// these, the registry lookup returns NULL and every pair pack renders
// as empty `@()`, which crashed on dereference when the outer list's
// elem_type_tag = TAIDA_TAG_PACK triggered the full-form recursion.
// Idempotent — follows the same pattern as
// `taida_register_lax_field_names` + C23B-009's entries() registration.
static void taida_register_zip_enumerate_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;
    taida_register_field_name((taida_val)HASH_FIRST,  (taida_val)"first");
    taida_register_field_name((taida_val)HASH_SECOND, (taida_val)"second");
    taida_register_field_name((taida_val)HASH_INDEX,  (taida_val)"index");
    taida_register_field_name((taida_val)HASH_VALUE,  (taida_val)"value");
}

taida_val taida_list_zip(taida_val list1, taida_val list2) {
    taida_register_zip_enumerate_field_names();  // C24-B
    taida_val *l1 = (taida_val*)list1;
    taida_val *l2 = (taida_val*)list2;
    taida_val len1 = l1[2], len2 = l2[2];
    taida_val min_len = len1 < len2 ? len1 : len2;
    taida_val elem_tag1 = l1[3];  // ソースリスト1の要素型タグ
    taida_val elem_tag2 = l2[3];  // ソースリスト2の要素型タグ
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = TAIDA_TAG_PACK;  // zip produces Pack elements
    for (taida_val i = 0; i < min_len; i++) {
        // Create a BuchiPack with fields: first, second
        taida_val pair = taida_pack_new(2);
        pair = taida_pack_set_hash(pair, 0, (taida_val)HASH_FIRST);
        pair = taida_pack_set(pair, 0, l1[4 + i]);
        // tag + retain for first field based on source list's elem_type_tag
        taida_pack_set_tag(pair, 0, elem_tag1);
        taida_list_elem_retain(l1[4 + i], elem_tag1);
        pair = taida_pack_set_hash(pair, 1, (taida_val)HASH_SECOND);
        pair = taida_pack_set(pair, 1, l2[4 + i]);
        // tag + retain for second field based on source list's elem_type_tag
        taida_pack_set_tag(pair, 1, elem_tag2);
        taida_list_elem_retain(l2[4 + i], elem_tag2);
        new_list = taida_list_push(new_list, pair);
    }
    return new_list;
}

taida_val taida_list_enumerate(taida_val list_ptr) {
    taida_register_zip_enumerate_field_names();  // C24-B
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];  // ソースリストの要素型タグ
    taida_val new_list = taida_list_new();
    taida_val *nl = (taida_val*)new_list;
    nl[3] = TAIDA_TAG_PACK;  // enumerate produces Pack elements
    for (taida_val i = 0; i < len; i++) {
        // Create a BuchiPack with fields: index, value
        taida_val pair = taida_pack_new(2);
        pair = taida_pack_set_hash(pair, 0, (taida_val)HASH_INDEX);
        pair = taida_pack_set(pair, 0, i);
        // index は INT なのでタグはデフォルト(0)のまま、retain 不要
        pair = taida_pack_set_hash(pair, 1, (taida_val)HASH_VALUE);
        pair = taida_pack_set(pair, 1, list[4 + i]);
        // value フィールドにソースリストの elem_type_tag に基づいて tag + retain
        taida_pack_set_tag(pair, 1, elem_tag);
        taida_list_elem_retain(list[4 + i], elem_tag);
        new_list = taida_list_push(new_list, pair);
    }
    return new_list;
}

// ── HashMap runtime ──────────────────────────────────────
// HashMap layout: [tag, capacity, length, value_type_tag, (key_hash, key_ptr, value)...]
// Header slots: [0]=magic+rc, [1]=capacity, [2]=length, [3]=value_type_tag
// Entry offset = 4. Entry stride = 3: [key_hash, key_ptr, value]
// Open-addressing hash map with linear probing.
// Tombstone: hash = 1, key = 0 (marker for deleted slots).
// Ownership contract (NO-1):
//   - key is always Str: taida_str_retain on store, taida_str_release on remove/drop
//   - value uses value_type_tag: retain-on-store, release-on-remove/drop
//   - clone retains all keys+values; resize is a move (no retain/release)
//   - taida_release recursively releases all keys+values when rc<=1

// (HM_HEADER, TAIDA_HASHMAP_TOMBSTONE_HASH, HM_SLOT_* macros defined earlier)

// Retain a HashMap value based on the map's value_type_tag.
static void taida_hashmap_val_retain(taida_val val, taida_val val_tag) {
    if (val_tag == TAIDA_TAG_PACK || val_tag == TAIDA_TAG_LIST || val_tag == TAIDA_TAG_CLOSURE || val_tag == TAIDA_TAG_HMAP || val_tag == TAIDA_TAG_SET) {
        if (val > 4096) taida_retain(val);
    } else if (val_tag == TAIDA_TAG_STR) {
        if (val > 4096) taida_str_retain(val);
    }
    // INT, FLOAT, BOOL, UNKNOWN → no-op
}

// Release a HashMap value based on the map's value_type_tag.
static void taida_hashmap_val_release(taida_val val, taida_val val_tag) {
    if (val_tag == TAIDA_TAG_PACK || val_tag == TAIDA_TAG_LIST || val_tag == TAIDA_TAG_CLOSURE || val_tag == TAIDA_TAG_HMAP || val_tag == TAIDA_TAG_SET) {
        if (val > 4096) taida_release(val);
    } else if (val_tag == TAIDA_TAG_STR) {
        if (val > 4096) taida_str_release(val);
    }
    // INT, FLOAT, BOOL, UNKNOWN → no-op
}

// Retain a HashMap key (always Str).
static void taida_hashmap_key_retain(taida_val key) {
    if (key > 4096) taida_str_retain(key);
}

// Release a HashMap key (always Str).
static void taida_hashmap_key_release(taida_val key) {
    if (key > 4096) taida_str_release(key);
}

static taida_val taida_hashmap_adjust_hash(taida_val h) {
    // Avoid 0 (empty) and 1 (tombstone)
    if (h == 0) return 42424242L;
    if (h == 1) return 14141414L;
    return h;
}

// Slot helpers: HM_SLOT_* macros defined in header section above

static taida_val taida_is_hashmap(taida_val ptr) {
    return TAIDA_IS_HMAP(ptr);
}

static int taida_ptr_is_readable(taida_val ptr, size_t bytes) {
    if (ptr == 0 || ptr < 4096) return 0;
    // Taida heap objects are always 8-byte aligned.
    if (ptr & 0x7) return 0;
    if (bytes == 0) return 1;

    uintptr_t start = (uintptr_t)ptr;
    if (start > UINTPTR_MAX - (bytes - 1)) return 0;
    uintptr_t end = start + (bytes - 1);

    taida_val page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) page_size = 4096;
    uintptr_t step = (uintptr_t)page_size;
    uintptr_t page_mask = step - 1;
    uintptr_t page = start & ~page_mask;
    uintptr_t last_page = end & ~page_mask;

    for (;;) {
        unsigned char vec = 0;
        if (mincore((void*)page, (size_t)page_size, &vec) != 0) {
            return 0;
        }
        if (page == last_page) break;
        if (page > UINTPTR_MAX - step) return 0;
        page += step;
    }
    return 1;
}

// NB-31: Check if a taida_val is callable (function pointer or closure).
// Uses negative logic: reject values that are DEFINITELY not callable.
// Function pointers may not be 8-byte aligned, so we cannot use taida_ptr_is_readable
// as a positive gate (it requires 8-byte alignment for heap objects).
static int _taida_is_callable_impl(taida_val val) {
    // Closures are always callable
    if (TAIDA_IS_CLOSURE(val)) return 1;
    // Small non-negative integers (covers most user-facing Int values including 42, 50000)
    if (val >= 0 && val <= 65535) return 0;
    // Negative integers
    if (val < 0) return 0;
    // 8-byte aligned + readable → check for known heap data types
    if ((val & 0x7) == 0 && taida_ptr_is_readable(val, 8)) {
        taida_val magic = ((taida_val*)val)[0] & TAIDA_MAGIC_MASK;
        if (magic == TAIDA_PACK_MAGIC || magic == TAIDA_LIST_MAGIC ||
            magic == TAIDA_STR_MAGIC || magic == TAIDA_HMAP_MAGIC ||
            magic == TAIDA_SET_MAGIC || magic == TAIDA_ASYNC_MAGIC ||
            magic == TAIDA_BYTES_MAGIC) return 0;
    }
    // Assume callable: function pointer or large integer (rare edge case)
    return 1;
}

static int taida_read_cstr_len_safe(const char *s, size_t max_len, size_t *out_len) {
    if (!s) return 0;
    uintptr_t ptr = (uintptr_t)s;
    if (ptr < 4096) return 0;

    taida_val page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) page_size = 4096;
    uintptr_t page_mask = (uintptr_t)(page_size - 1);
    uintptr_t current_page = 0;

    for (size_t i = 0; i < max_len; i++) {
        uintptr_t addr = ptr + i;
        uintptr_t page = addr & ~page_mask;
        if (page != current_page) {
            unsigned char vec = 0;
            if (mincore((void*)page, (size_t)page_size, &vec) != 0) {
                return 0;
            }
            current_page = page;
        }
        unsigned char ch = ((const unsigned char*)s)[i];
        if (ch == 0) {
            if (out_len) *out_len = i;
            return 1;
        }
    }
    return 0;
}

// NB3-8: Get string byte length from heap header metadata when available,
// falling back to taida_read_cstr_len_safe for static strings.
// Returns 1 on success (length stored in *out_len), 0 on failure.
static int taida_str_byte_len(const char *s, size_t *out_len) {
    if (!s) return 0;
    uintptr_t ptr = (uintptr_t)s;
    if (ptr < 4096) return 0;
    // Check if this is a heap string with hidden header
    taida_val *hdr = ((taida_val*)s) - 2;
    if (taida_ptr_is_readable((taida_val)hdr, sizeof(taida_val) * 2)) {
        taida_val tag = hdr[0];
        if ((tag & TAIDA_MAGIC_MASK) == TAIDA_STR_MAGIC) {
            if (out_len) *out_len = (size_t)hdr[1];
            return 1;
        }
    }
    // Static string — fall back to NUL scan
    return taida_read_cstr_len_safe(s, 16 * 1024 * 1024, out_len);
}

static int taida_hashmap_key_valid(taida_val key_ptr) {
    // All values are valid keys in Taida.
    // Null (0) is traditionally not a key, but we can allow it as Int(0).
    return 1;
}

// FNV-1a hash for string keys (runtime computation)
taida_val taida_str_hash(taida_val str_ptr) {
    const unsigned char *s = (const unsigned char*)str_ptr;
    size_t len = 0;
    if (!taida_read_cstr_len_safe((const char*)s, 8192, &len)) return 0;

    uint64_t hash = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) {
        hash ^= s[i];
        hash *= 0x100000001b3ULL;
    }
    return (taida_val)hash;
}

taida_val taida_value_hash(taida_val val) {
    size_t len = 0;
    taida_val h = val;
    // Check if it's a valid string pointer
    if (taida_read_cstr_len_safe((const char*)val, 8192, &len)) {
        h = taida_str_hash(val);
    }
    // Identity hash for scalars (ints/floats), or FNV-1a for strings.
    // ALWAYS adjust to avoid 0/1.
    return taida_hashmap_adjust_hash(h);
}

static int taida_hashmap_key_eq(taida_val key_a, taida_val key_b) {
    if (key_a == key_b) return 1;
    // For string comparison, ensure both are valid pointers
    if (!taida_hashmap_key_valid(key_a) || !taida_hashmap_key_valid(key_b)) return 0;

    const char *sa = (const char*)key_a;
    const char *sb = (const char*)key_b;
    size_t la = 0, lb = 0;
    if (!taida_read_cstr_len_safe(sa, 8192, &la)) return 0;
    if (!taida_read_cstr_len_safe(sb, 8192, &lb)) return 0;
    if (la != lb) return 0;
    return memcmp(sa, sb, la) == 0;
}

static taida_val taida_hashmap_new_with_cap(taida_val cap) {
    // M-02: Guard against non-positive cap and cap * 3 overflow.
    if (cap <= 0) cap = 16;
    // C23B-008 (2026-04-22): allocate extra `1 + cap` slots for the
    // insertion-order side-index. `calloc` zero-initialises everything,
    // so `next_ord` starts at 0 and `order_array` is all zeros (never
    // read while `next_ord == 0`).
    size_t slots = taida_safe_add((size_t)HM_HEADER, taida_safe_mul((size_t)cap, 3, "hm_new_with_cap slots"), "hm_new_with_cap entries");
    slots = taida_safe_add(slots, (size_t)(1 + cap), "hm_new_with_cap ord");
    size_t alloc_size = taida_safe_mul(slots, sizeof(taida_val), "hm_new_with_cap bytes");
    taida_val *hm = (taida_val*)calloc(1, alloc_size);
    if (!hm) { fprintf(stderr, "taida: out of memory (taida_hashmap_new_with_cap)\n"); exit(1); }
    hm[0] = TAIDA_HMAP_MAGIC | 1;  // Magic + refcount
    hm[1] = cap;  // capacity
    hm[2] = 0;    // length
    hm[3] = TAIDA_TAG_UNKNOWN;  // value_type_tag (unknown until set)
    // next_ord (hm[TAIDA_HM_ORD_HEADER_SLOT(cap)]) and order_array are
    // already zero thanks to calloc.
    return (taida_val)hm;
}

void taida_hashmap_set_value_tag(taida_val hm_ptr, taida_val tag) {
    // C23B-007 (2026-04-22): sentinel-separated downgrade logic (mirror of
    // wasm `taida_hashmap_set_value_tag`). See the list variant above for
    // the full rationale; in short, mixed-value HashMaps like
    // `.set("a", 1).set("b", "x").set("c", 2)` must stay HETEROGENEOUS(-2)
    // once they've seen two incompatible primitive tags, and must never
    // re-promote. Native HashMap iteration (retain/release, display) treats
    // HETEROGENEOUS identically to UNKNOWN — same leak-rather-than-crash
    // behaviour as before for tag-ambiguous containers.
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val existing = hm[3];
    if (existing == TAIDA_TAG_HETEROGENEOUS) return;
    if (existing == TAIDA_TAG_UNKNOWN || existing == tag) {
        hm[3] = tag;
    } else {
        hm[3] = TAIDA_TAG_HETEROGENEOUS;
    }
}

taida_val taida_hashmap_new(void) {
    return taida_hashmap_new_with_cap(16);
}

// Internal set used by resize (does not trigger resize).
// C23B-008: returns the bucket slot where the insert landed (so the
// caller can record it in the insertion-order side-index), or -1 when
// an existing key was updated in place (no new ordinal allocated).
static taida_val taida_hashmap_set_internal(taida_val *hm, taida_val cap, taida_val key_hash, taida_val key_ptr, taida_val value) {
    uint64_t uh = (uint64_t)key_hash;
    taida_val idx = (taida_val)(uh % (uint64_t)cap);
    for (taida_val i = 0; i < cap; i++) {
        taida_val slot = (idx + i) % cap;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_EMPTY(sh, sk)) {
            hm[HM_HEADER + slot * 3] = key_hash;
            hm[HM_HEADER + slot * 3 + 1] = key_ptr;
            hm[HM_HEADER + slot * 3 + 2] = value;
            hm[2]++;
            return slot;
        }
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) {
            hm[HM_HEADER + slot * 3 + 2] = value;
            return -1;
        }
    }
    return -1;
}

// Resize the hashmap to new_cap (re-hash all occupied entries)
// This is a MOVE operation — entries transfer ownership from old to new.
// No retain/release needed; the old table's raw memory is freed.
// C23B-008 (2026-04-22): walk the OLD insertion-order side-index so the
// new table's entries keep the same insertion sequence (required for
// parity with interpreter / JS). Tombstoned / removed entries are
// skipped. Rebuild the new side-index as we go.
static taida_val taida_hashmap_resize(taida_val hm_ptr, taida_val new_cap) {
    taida_val *old_hm = (taida_val*)hm_ptr;
    taida_val old_cap = old_hm[1];
    taida_val new_hm_ptr = taida_hashmap_new_with_cap(new_cap);
    taida_val *new_hm = (taida_val*)new_hm_ptr;
    // NO-1: propagate value_type_tag from old to new
    new_hm[3] = old_hm[3];
    taida_val old_next_ord = old_hm[TAIDA_HM_ORD_HEADER_SLOT(old_cap)];
    taida_val new_next_ord = 0;
    for (taida_val oi = 0; oi < old_next_ord; oi++) {
        taida_val slot = old_hm[TAIDA_HM_ORD_SLOT(old_cap, oi)];
        if (slot < 0 || slot >= old_cap) continue;  // removed or invalid
        taida_val sh = old_hm[HM_HEADER + slot * 3];
        taida_val sk = old_hm[HM_HEADER + slot * 3 + 1];
        if (!HM_SLOT_OCCUPIED(sh, sk)) continue;
        taida_val new_slot = taida_hashmap_set_internal(new_hm, new_cap, sh, sk, old_hm[HM_HEADER + slot * 3 + 2]);
        if (new_slot >= 0) {
            new_hm[TAIDA_HM_ORD_SLOT(new_cap, new_next_ord)] = new_slot;
            new_next_ord++;
        }
    }
    new_hm[TAIDA_HM_ORD_HEADER_SLOT(new_cap)] = new_next_ord;
    free(old_hm);
    return new_hm_ptr;
}

taida_val taida_hashmap_set(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr, taida_val value) {
    if (!taida_hashmap_key_valid(key_ptr)) {
        return hm_ptr;
    }

    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val len = hm[2];

    // Load factor check: resize at 0.75 (F-21 fix)
    if (len * 4 >= cap * 3) {
        hm_ptr = taida_hashmap_resize(hm_ptr, cap * 2);
        hm = (taida_val*)hm_ptr;
        cap = hm[1];
    }

    taida_val val_tag = hm[3];  // value_type_tag
    uint64_t uh = (uint64_t)key_hash;
    taida_val idx = (taida_val)(uh % (uint64_t)cap);
    taida_val first_tombstone = -1;
    for (taida_val i = 0; i < cap; i++) {
        taida_val slot = (idx + i) % cap;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_EMPTY(sh, sk)) {
            // Insert at tombstone if we passed one, else at this empty slot
            taida_val target = (first_tombstone >= 0) ? first_tombstone : slot;
            hm[HM_HEADER + target * 3] = key_hash;
            hm[HM_HEADER + target * 3 + 1] = key_ptr;
            hm[HM_HEADER + target * 3 + 2] = value;
            // NO-1: retain-on-store for new entry
            taida_hashmap_key_retain(key_ptr);
            taida_hashmap_val_retain(value, val_tag);
            hm[2]++;
            // C23B-008 (2026-04-22): record the insertion ordinal.
            taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
            hm[TAIDA_HM_ORD_SLOT(cap, next_ord)] = target;
            hm[TAIDA_HM_ORD_HEADER_SLOT(cap)] = next_ord + 1;
            return hm_ptr;
        }
        if (HM_SLOT_TOMBSTONE(sh, sk)) {
            if (first_tombstone < 0) first_tombstone = slot;
            continue;  // skip tombstone, keep probing
        }
        // Occupied slot: compare hash AND key (F-19 fix)
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) {
            // NO-1: release old value, retain new value on overwrite
            taida_val old_val = hm[HM_HEADER + slot * 3 + 2];
            taida_hashmap_val_release(old_val, val_tag);
            taida_hashmap_val_retain(value, val_tag);
            hm[HM_HEADER + slot * 3 + 2] = value;
            // C23B-008: update-in-place keeps the existing insertion
            // ordinal (no side-index change), so display order remains
            // first-insertion-wins — matches interpreter semantics.
            return hm_ptr;
        }
    }
    // Table full of tombstones — insert at first tombstone
    if (first_tombstone >= 0) {
        hm[HM_HEADER + first_tombstone * 3] = key_hash;
        hm[HM_HEADER + first_tombstone * 3 + 1] = key_ptr;
        hm[HM_HEADER + first_tombstone * 3 + 2] = value;
        // NO-1: retain-on-store for new entry
        taida_hashmap_key_retain(key_ptr);
        taida_hashmap_val_retain(value, val_tag);
        hm[2]++;
        // C23B-008: record ordinal for the tombstone-reuse insertion.
        taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
        hm[TAIDA_HM_ORD_SLOT(cap, next_ord)] = first_tombstone;
        hm[TAIDA_HM_ORD_HEADER_SLOT(cap)] = next_ord + 1;
    }
    return hm_ptr;
}

taida_val taida_hashmap_get(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr) {
    if (!taida_hashmap_key_valid(key_ptr)) return 0;

    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    uint64_t uh = (uint64_t)key_hash;
    taida_val idx = (taida_val)(uh % (uint64_t)cap);
    for (taida_val i = 0; i < cap; i++) {
        taida_val slot = (idx + i) % cap;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_EMPTY(sh, sk)) return 0; // not found
        if (HM_SLOT_TOMBSTONE(sh, sk)) continue; // skip tombstone
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) return hm[HM_HEADER + slot * 3 + 2];
    }
    return 0;
}

taida_val taida_hashmap_has(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr) {
    if (!taida_hashmap_key_valid(key_ptr)) return 0;

    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    uint64_t uh = (uint64_t)key_hash;
    taida_val idx = (taida_val)(uh % (uint64_t)cap);
    for (taida_val i = 0; i < cap; i++) {
        taida_val slot = (idx + i) % cap;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_EMPTY(sh, sk)) return 0;
        if (HM_SLOT_TOMBSTONE(sh, sk)) continue;
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) return 1;
    }
    return 0;
}

taida_val taida_hashmap_remove(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr) {
    if (!taida_hashmap_key_valid(key_ptr)) return hm_ptr;

    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val val_tag = hm[3];  // value_type_tag
    uint64_t uh = (uint64_t)key_hash;
    taida_val idx = (taida_val)(uh % (uint64_t)cap);
    for (taida_val i = 0; i < cap; i++) {
        taida_val slot = (idx + i) % cap;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_EMPTY(sh, sk)) return hm_ptr;
        if (HM_SLOT_TOMBSTONE(sh, sk)) continue;
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) {
            // NO-1: release key and value before tombstoning
            taida_hashmap_key_release(sk);
            taida_hashmap_val_release(hm[HM_HEADER + slot * 3 + 2], val_tag);
            // Set tombstone marker (F-20 fix)
            hm[HM_HEADER + slot * 3] = TAIDA_HASHMAP_TOMBSTONE_HASH;
            hm[HM_HEADER + slot * 3 + 1] = 0;
            hm[HM_HEADER + slot * 3 + 2] = 0;
            hm[2]--;
            // C23B-008 (2026-04-22): hole out the ordinal slot so
            // display / iteration skip it. `next_ord` stays as a
            // monotonic upper bound (never decremented).
            taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
            for (taida_val oi = 0; oi < next_ord; oi++) {
                if (hm[TAIDA_HM_ORD_SLOT(cap, oi)] == slot) {
                    hm[TAIDA_HM_ORD_SLOT(cap, oi)] = -1;
                    break;
                }
            }
            return hm_ptr;
        }
    }
    return hm_ptr;
}

taida_val taida_hashmap_keys(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val list = taida_list_new();
    // NO-1: keys are always Str — set elem_type_tag and retain each key
    ((taida_val*)list)[3] = TAIDA_TAG_STR;
    // C23B-008 (2026-04-22): insertion-order walk so .keys() matches
    // interpreter / JS ordering.
    taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
    for (taida_val oi = 0; oi < next_ord; oi++) {
        taida_val slot = hm[TAIDA_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_hashmap_key_retain(sk);
            list = taida_list_push(list, sk);
        }
    }
    return list;
}

taida_val taida_hashmap_values(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val val_tag = hm[3];  // value_type_tag
    taida_val list = taida_list_new();
    // NO-1: propagate value_type_tag to the returned list and retain each value
    ((taida_val*)list)[3] = val_tag;
    // C23B-008: insertion-order walk.
    taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
    for (taida_val oi = 0; oi < next_ord; oi++) {
        taida_val slot = hm[TAIDA_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val v = hm[HM_HEADER + slot * 3 + 2];
            taida_hashmap_val_retain(v, val_tag);
            list = taida_list_push(list, v);
        }
    }
    return list;
}

taida_val taida_hashmap_length(taida_val hm_ptr) {
    return ((taida_val*)hm_ptr)[2];
}

taida_val taida_hashmap_is_empty(taida_val hm_ptr) {
    return ((taida_val*)hm_ptr)[2] == 0 ? 1 : 0;
}

// Clone a hashmap (for immutable set/remove/merge semantics)
// Cloned entries share ownership with the original, so retain all keys+values.
// C23B-008 (2026-04-22): bump allocation size to include the insertion-
// order side-index and copy it verbatim (same bucket layout → same slot
// indices remain valid).
static taida_val taida_hashmap_clone(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val val_tag = hm[3];  // value_type_tag
    // M-03: Guard against negative/overflow cap and NULL malloc result.
    if (cap < 0) {
        fprintf(stderr, "taida: invalid hashmap cap %" PRId64 " in taida_hashmap_clone\n", (int64_t)cap);
        exit(1);
    }
    size_t total = taida_safe_add((size_t)HM_HEADER, taida_safe_mul((size_t)cap, 3, "hm_clone slots"), "hm_clone entries");
    // C23B-008: include the insertion-order side-index (1 + cap slots)
    // at the end of the allocation.
    total = taida_safe_add(total, (size_t)(1 + cap), "hm_clone ord");
    size_t alloc_size = taida_safe_mul(total, sizeof(taida_val), "hm_clone bytes");
    taida_val *new_hm = (taida_val*)TAIDA_MALLOC(alloc_size, "hashmap_clone");
    memcpy(new_hm, hm, alloc_size);
    new_hm[0] = TAIDA_HMAP_MAGIC | 1;  // preserve magic + reset rc
    // Retain all keys and values in the clone (shared ownership)
    for (taida_val i = 0; i < cap; i++) {
        taida_val sh = new_hm[HM_HEADER + i * 3];
        taida_val sk = new_hm[HM_HEADER + i * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_hashmap_key_retain(sk);
            taida_hashmap_val_retain(new_hm[HM_HEADER + i * 3 + 2], val_tag);
        }
    }
    return (taida_val)new_hm;
}

// Immutable set: clone then set
taida_val taida_hashmap_set_immut(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr, taida_val value) {
    taida_val new_hm = taida_hashmap_clone(hm_ptr);
    return taida_hashmap_set(new_hm, key_hash, key_ptr, value);
}

// Immutable remove: clone then remove
taida_val taida_hashmap_remove_immut(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr) {
    taida_val new_hm = taida_hashmap_clone(hm_ptr);
    return taida_hashmap_remove(new_hm, key_hash, key_ptr);
}

// Get returning Lax (Lax[value]() or empty Lax)
taida_val taida_hashmap_get_lax(taida_val hm_ptr, taida_val key_hash, taida_val key_ptr) {
    if (!taida_hashmap_key_valid(key_ptr)) return taida_lax_empty(0);

    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    uint64_t uh = (uint64_t)key_hash;
    taida_val idx = (taida_val)(uh % (uint64_t)cap);
    for (taida_val i = 0; i < cap; i++) {
        taida_val slot = (idx + i) % cap;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_EMPTY(sh, sk)) return taida_lax_empty(0);
        if (HM_SLOT_TOMBSTONE(sh, sk)) continue;
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) return taida_lax_new(hm[HM_HEADER + slot * 3 + 2], 0);
    }
    return taida_lax_empty(0);
}

// Entries: returns list of BuchiPack @(key, value).
// C23B-008 (2026-04-22): insertion-order walk so .entries() matches
// interpreter / JS ordering.
// C23B-009 (2026-04-22): idempotently register `"key"` / `"value"` in the
// field-name registry so `taida_pack_to_display_string_full` can resolve
// them. Without this registration, `taida_lookup_field_name` returned NULL
// and every pair pack printed as `@()` — diverging from interpreter's
// `@(key <= …, value <= …)` shape documented in
// `docs/reference/standard_library.md:238`.
taida_val taida_hashmap_entries(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val val_tag = hm[3];  // value_type_tag
    taida_val list = taida_list_new();
    // NO-1: entries returns List[Pack] — set elem_type_tag = PACK
    ((taida_val*)list)[3] = TAIDA_TAG_PACK;
    // FNV-1a hashes for "key" and "value"
    #define HASH_KEY   0x3dc94a19365b10ecULL
    #define HASH_VAL   0x7ce4fd9430e80ceaULL
    // C23B-009: register pair field names once. `taida_register_field_name`
    // is idempotent (skips duplicates). Using static-string literals so the
    // registry can hold the pointer indefinitely without ownership issues.
    static int __entries_names_registered = 0;
    if (!__entries_names_registered) {
        __entries_names_registered = 1;
        taida_register_field_name((taida_val)HASH_KEY, (taida_val)"key");
        taida_register_field_name((taida_val)HASH_VAL, (taida_val)"value");
    }
    taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
    for (taida_val oi = 0; oi < next_ord; oi++) {
        taida_val slot = hm[TAIDA_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val pair = taida_pack_new(2);
            taida_pack_set_hash(pair, 0, (taida_val)HASH_KEY);
            // NO-1: tag + retain key (Str) and value fields in pair pack
            taida_pack_set_tag(pair, 0, TAIDA_TAG_STR);
            taida_hashmap_key_retain(sk);
            taida_pack_set(pair, 0, sk);
            taida_pack_set_hash(pair, 1, (taida_val)HASH_VAL);
            taida_pack_set_tag(pair, 1, val_tag);
            taida_val v = hm[HM_HEADER + slot * 3 + 2];
            taida_hashmap_val_retain(v, val_tag);
            taida_pack_set(pair, 1, v);
            list = taida_list_push(list, pair);
        }
    }
    return list;
}

// Merge two hashmaps (other overwrites this).
// C23B-008 reopen (2026-04-22): interpreter semantics for merge are NOT
// "update-in-place for overlap keys". `src/interpreter/methods.rs:787-822`
// does `merged.retain(|e| e.key != other_key); merged.push(other_entry)`
// for every other entry, which MOVES any overlap key to other's position
// (with other's value). The previous implementation called
// `taida_hashmap_set(clone_of_self, other_entry)` for each other entry,
// which updated in place and preserved self's ordinal — divergent from
// interpreter (repro: `a=[a,b]`, `b=[c,b,d]`, interpreter gives
// `[a,c,b,d]`, buggy backends gave `[a,b,c,d]`).
//
// New algorithm (mirrors interpreter retain+push):
//   1. Build a fresh empty HashMap.
//   2. Walk self in insertion order; insert entries whose key is NOT in
//      other. These land in self-order at the front of the side-index.
//   3. Walk other in insertion order; insert each entry (all of these are
//      new to the fresh map since step 2 filtered the overlap keys out).
//      Values and ordinals come from other. Value retention flows through
//      `taida_hashmap_set` exactly as a fresh `.set()` chain.
//
// This exactly reproduces `retain-then-push` ordering without needing to
// touch the `taida_hashmap_set` contract (which must keep preserving
// ordinal on .set-on-existing for the plain `.set()` path).
taida_val taida_hashmap_merge(taida_val hm_ptr, taida_val other_ptr) {
    taida_val *self = (taida_val*)hm_ptr;
    taida_val *other = (taida_val*)other_ptr;
    taida_val self_cap = self[1];
    taida_val other_cap = other[1];

    taida_val result = taida_hashmap_new();

    // Step 1: self entries whose key is absent from `other` (self-order).
    taida_val self_next_ord = self[TAIDA_HM_ORD_HEADER_SLOT(self_cap)];
    for (taida_val oi = 0; oi < self_next_ord; oi++) {
        taida_val slot = self[TAIDA_HM_ORD_SLOT(self_cap, oi)];
        if (slot < 0 || slot >= self_cap) continue;
        taida_val sh = self[HM_HEADER + slot * 3];
        taida_val sk = self[HM_HEADER + slot * 3 + 1];
        if (!HM_SLOT_OCCUPIED(sh, sk)) continue;
        if (taida_hashmap_has(other_ptr, sh, sk)) continue;
        result = taida_hashmap_set(result, sh, sk, self[HM_HEADER + slot * 3 + 2]);
    }

    // Step 2: all other entries in other-order (guaranteed new to result).
    taida_val other_next_ord = other[TAIDA_HM_ORD_HEADER_SLOT(other_cap)];
    for (taida_val oi = 0; oi < other_next_ord; oi++) {
        taida_val slot = other[TAIDA_HM_ORD_SLOT(other_cap, oi)];
        if (slot < 0 || slot >= other_cap) continue;
        taida_val sh = other[HM_HEADER + slot * 3];
        taida_val sk = other[HM_HEADER + slot * 3 + 1];
        if (!HM_SLOT_OCCUPIED(sh, sk)) continue;
        result = taida_hashmap_set(result, sh, sk, other[HM_HEADER + slot * 3 + 2]);
    }
    return result;
}

// HashMap.toString() -> "HashMap({key1: val1, key2: val2})"
// C23B-008 (2026-04-22): insertion-order walk so .toString() matches
// interpreter / JS ordering byte-for-byte.
taida_val taida_hashmap_to_string(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];

    size_t buf_size = 256;
    char *buf = (char*)TAIDA_MALLOC(buf_size, "hm_to_string");
    // R-03: Use offset tracking instead of strcat (O(n) per call → O(1)).
    memcpy(buf, "HashMap({", 10); /* 9 chars + '\0' */
    size_t off = 9;
    taida_val count = 0;

    taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
    for (taida_val oi = 0; oi < next_ord; oi++) {
        taida_val slot = hm[TAIDA_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val value = hm[HM_HEADER + slot * 3 + 2];

            taida_val key_str_ptr = taida_value_to_debug_string(sk);
            taida_val val_str_ptr = taida_value_to_debug_string(value);
            const char *key_str = (const char*)key_str_ptr;
            const char *val_str = (const char*)val_str_ptr;
            if (!key_str) key_str = "\"\"";
            if (!val_str) val_str = "0";

            size_t klen = strlen(key_str);
            size_t vlen = strlen(val_str);
            size_t needed = klen + vlen + 4;
            if (count > 0) needed += 2;
            while (off + needed + 3 > buf_size) {
                buf_size *= 2;
                TAIDA_REALLOC(buf, buf_size, "hashmap_to_string");
            }

            if (count > 0) { memcpy(buf + off, ", ", 2); off += 2; }
            memcpy(buf + off, key_str, klen); off += klen;
            memcpy(buf + off, ": ", 2); off += 2;
            memcpy(buf + off, val_str, vlen); off += vlen;
            buf[off] = '\0';

            taida_str_release(key_str_ptr);
            taida_str_release(val_str_ptr);
            count++;
        }
    }
    memcpy(buf + off, "})", 3); /* 2 chars + '\0' */
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

// ── Set runtime ──────────────────────────────────────────
// NO-4 RULE 1: Set follows List pattern — elem_type_tag + retain-on-copy.
// All set mutation ops (add/remove/union/intersect/diff) MUST propagate
// elem_type_tag and retain copied elements via taida_list_elem_retain.
// Set layout: same as list but with uniqueness constraint
// We use a regular list as the backing store (linear scan for uniqueness)
// Set is tagged with a special marker in slot[0] (negative refcount area won't work)
// Instead: Set = BuchiPack @(__items: List, __type: "Set")
// But for performance, let's use a simpler approach:
// Set is just a list with uniqueness, tagged by runtime functions.
// We'll use a structure: [refcount, capacity, length, items...]
// with a tag value stored differently.
//
// Simplest approach: Set = list ptr, and all set ops maintain uniqueness.
// The set functions accept and return list ptrs.

static taida_val taida_is_set(taida_val ptr) {
    return TAIDA_IS_SET(ptr);
}

taida_val taida_set_new(void) {
    taida_val list = taida_list_new();
    // Tag the list as a Set with magic
    ((taida_val*)list)[0] = TAIDA_SET_MAGIC | 1;
    // elem_type_tag at offset 3 is already TAIDA_TAG_UNKNOWN from taida_list_new
    return list;
}

// NO-2: Set elem_type_tag setter (analogous to taida_list_set_elem_tag / taida_hashmap_set_value_tag)
void taida_set_set_elem_tag(taida_val set_ptr, taida_val tag) {
    ((taida_val*)set_ptr)[3] = tag;
}

static taida_val taida_set_contains(taida_val set_ptr, taida_val item) {
    taida_val *list = (taida_val*)set_ptr;
    taida_val len = list[2];  // length at index 2
    for (taida_val i = 0; i < len; i++) {
        if (list[4 + i] == item) return 1;
    }
    return 0;
}

taida_val taida_set_from_list(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];  // length at index 2
    taida_val elem_tag = list[3];  // NO-2: propagate elem_type_tag from source list
    taida_val new_set = taida_set_new();
    ((taida_val*)new_set)[3] = elem_tag;  // NO-2: set elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (!taida_set_contains(new_set, item)) {
            taida_list_elem_retain(item, elem_tag);  // NO-2: retain-on-copy
            new_set = taida_list_push(new_set, item);
        }
    }
    return new_set;
}

taida_val taida_set_add(taida_val set_ptr, taida_val item) {
    if (taida_set_contains(set_ptr, item)) {
        return set_ptr;  // Already exists, return unchanged
    }
    // Use taida_list_push for correct list manipulation
    taida_val *list = (taida_val*)set_ptr;
    taida_val len = list[2];
    taida_val elem_tag = list[3];  // NO-2: propagate elem_type_tag
    // Clone the set, then push the new item
    taida_val new_set = taida_set_new();
    ((taida_val*)new_set)[3] = elem_tag;  // NO-2: set elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(list[4 + i], elem_tag);  // NO-2: retain-on-copy
        new_set = taida_list_push(new_set, list[4 + i]);
    }
    taida_list_elem_retain(item, elem_tag);  // NO-2: retain new element
    new_set = taida_list_push(new_set, item);
    return new_set;
}

taida_val taida_set_remove(taida_val set_ptr, taida_val item) {
    taida_val *list = (taida_val*)set_ptr;
    taida_val len = list[2];  // length at index 2
    taida_val elem_tag = list[3];  // NO-2: propagate elem_type_tag
    taida_val new_set = taida_set_new();
    ((taida_val*)new_set)[3] = elem_tag;  // NO-2: set elem_type_tag
    for (taida_val i = 0; i < len; i++) {
        if (list[4 + i] != item) {
            taida_list_elem_retain(list[4 + i], elem_tag);  // NO-2: retain-on-copy
            new_set = taida_list_push(new_set, list[4 + i]);
        }
    }
    return new_set;
}

taida_val taida_set_has(taida_val set_ptr, taida_val item) {
    return taida_set_contains(set_ptr, item);
}

taida_val taida_set_size(taida_val set_ptr) {
    return ((taida_val*)set_ptr)[2];  // length at index 2
}

taida_val taida_set_is_empty(taida_val set_ptr) {
    return ((taida_val*)set_ptr)[2] == 0 ? 1 : 0;  // length at index 2
}

taida_val taida_set_to_list(taida_val set_ptr) {
    // Clone the set as a regular list (not tagged as Set)
    taida_val *list = (taida_val*)set_ptr;
    taida_val len = list[2];  // length at index 2
    taida_val elem_tag = list[3];  // NO-2: propagate elem_type_tag
    taida_val new_list = taida_list_new();  // regular list, refcount=1 (not SET tag)
    ((taida_val*)new_list)[3] = elem_tag;  // NO-2: set elem_type_tag on result list
    for (taida_val i = 0; i < len; i++) {
        taida_list_elem_retain(list[4 + i], elem_tag);  // NO-2: retain-on-copy
        new_list = taida_list_push(new_list, list[4 + i]);
    }
    return new_list;
}

taida_val taida_set_union(taida_val set_a, taida_val set_b) {
    taida_val *a = (taida_val*)set_a;
    taida_val *b = (taida_val*)set_b;
    taida_val a_len = a[2];  // length at index 2
    taida_val b_len = b[2];
    taida_val elem_tag = a[3];  // NO-2: propagate elem_type_tag from set_a
    // Start with a copy of a
    taida_val result = taida_set_new();
    ((taida_val*)result)[3] = elem_tag;  // NO-2: set elem_type_tag
    for (taida_val i = 0; i < a_len; i++) {
        taida_list_elem_retain(a[4 + i], elem_tag);  // NO-2: retain-on-copy
        result = taida_list_push(result, a[4 + i]);
    }
    // Add items from b that aren't in a
    for (taida_val i = 0; i < b_len; i++) {
        if (!taida_set_contains(result, b[4 + i])) {
            taida_list_elem_retain(b[4 + i], elem_tag);  // NO-2: retain-on-copy
            result = taida_list_push(result, b[4 + i]);
        }
    }
    return result;
}

taida_val taida_set_intersect(taida_val set_a, taida_val set_b) {
    taida_val *a = (taida_val*)set_a;
    taida_val a_len = a[2];  // length at index 2
    taida_val elem_tag = a[3];  // NO-2: propagate elem_type_tag
    taida_val result = taida_set_new();
    ((taida_val*)result)[3] = elem_tag;  // NO-2: set elem_type_tag
    for (taida_val i = 0; i < a_len; i++) {
        if (taida_set_contains(set_b, a[4 + i])) {
            taida_list_elem_retain(a[4 + i], elem_tag);  // NO-2: retain-on-copy
            result = taida_list_push(result, a[4 + i]);
        }
    }
    return result;
}

taida_val taida_set_diff(taida_val set_a, taida_val set_b) {
    taida_val *a = (taida_val*)set_a;
    taida_val a_len = a[2];  // length at index 2
    taida_val elem_tag = a[3];  // NO-2: propagate elem_type_tag
    taida_val result = taida_set_new();
    ((taida_val*)result)[3] = elem_tag;  // NO-2: set elem_type_tag
    for (taida_val i = 0; i < a_len; i++) {
        if (!taida_set_contains(set_b, a[4 + i])) {
            taida_list_elem_retain(a[4 + i], elem_tag);  // NO-2: retain-on-copy
            result = taida_list_push(result, a[4 + i]);
        }
    }
    return result;
}

// Set.toString() -> "Set({1, 2, 3})"
taida_val taida_set_to_string(taida_val set_ptr) {
    taida_val *list = (taida_val*)set_ptr;
    taida_val len = list[2];  // length at index 2
    size_t buf_size = 128;
    char *buf = (char*)TAIDA_MALLOC(buf_size, "set_to_string");
    // R-03: Use offset tracking instead of strcat (O(n) per call → O(1)).
    memcpy(buf, "Set({", 6); /* 5 chars + '\0' */
    size_t off = 5;
    for (taida_val i = 0; i < len; i++) {
        char item_str[64];
        int item_len = snprintf(item_str, sizeof(item_str), "%" PRId64 "", list[4 + i]);
        size_t needed = (size_t)item_len + (i > 0 ? 2 : 0) + 10;
        if (off + needed > buf_size) {
            buf_size *= 2;
            TAIDA_REALLOC(buf, buf_size, "set_to_string");
        }
        if (i > 0) { memcpy(buf + off, ", ", 2); off += 2; }
        memcpy(buf + off, item_str, (size_t)item_len); off += (size_t)item_len;
        buf[off] = '\0';
    }
    memcpy(buf + off, "})", 3); /* 2 chars + '\0' */
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

// ── Polymorphic length ───────────────────────────────────
// .length() — works on Str (strlen) and List/Set (list[2], unchanged by elem_type_tag)
// Detection: if ptr looks like a heap object with list layout (ptr[0] small, ptr[1] >= 8),
// treat as list. Otherwise treat as string.
taida_val taida_polymorphic_length(taida_val ptr) {
    if (ptr == 0) return 0;
    if (ptr < 4096) return 0;
    // Check for HashMap
    if (taida_is_hashmap(ptr)) {
        if (!taida_ptr_is_readable(ptr, sizeof(taida_val) * 3)) return 0;
        return ((taida_val*)ptr)[2];
    }
    // Check for Set
    if (taida_is_set(ptr)) {
        if (!taida_ptr_is_readable(ptr, sizeof(taida_val) * 3)) return 0;
        return ((taida_val*)ptr)[2];
    }
    // Check for List
    if (TAIDA_IS_LIST(ptr)) {
        return ((taida_val*)ptr)[2];
    }
    // Check for Bytes
    if (TAIDA_IS_BYTES(ptr)) {
        return ((taida_val*)ptr)[1];
    }
    // Treat as string
    size_t sl = 0;
    if (taida_read_cstr_len_safe((const char*)ptr, 65536, &sl)) return (taida_val)sl;
    return 0;
}

// Polymorphic .contains(needle) — works on Str and List.
// Runtime dispatch is required for cases where static lowering cannot prove
// string type (e.g. field access inside lambda callbacks).
taida_val taida_polymorphic_contains(taida_val obj, taida_val needle) {
    if (obj == 0 || obj < 4096) return 0;
    // Check for non-string types first (list, pack, bytes, hashmap, etc.)
    if (taida_ptr_is_readable(obj, 8) && taida_has_magic_header(((taida_val*)obj)[0])) {
        return taida_list_contains(obj, needle);
    }
    // No magic header — treat as C string
    taida_val needle_str = taida_value_to_display_string(needle);
    taida_val out = taida_str_contains((const char*)obj, (const char*)needle_str);
    taida_release(needle_str);
    return out;
}

taida_val taida_polymorphic_index_of(taida_val obj, taida_val needle) {
    if (obj == 0 || obj < 4096) return -1;
    // Check for non-string types first (list, pack, bytes, hashmap, etc.)
    if (taida_ptr_is_readable(obj, 8) && taida_has_magic_header(((taida_val*)obj)[0])) {
        return taida_list_index_of(obj, needle);
    }
    // No magic header — treat as C string
    return taida_str_index_of((const char*)obj, (const char*)needle);
}

taida_val taida_polymorphic_last_index_of(taida_val obj, taida_val needle) {
    if (obj == 0 || obj < 4096) return -1;
    if (taida_ptr_is_readable(obj, 8) && taida_has_magic_header(((taida_val*)obj)[0])) {
        return taida_list_last_index_of(obj, needle);
    }
    return taida_str_last_index_of((const char*)obj, (const char*)needle);
}

// ── Polymorphic collection methods ───────────────────────
// These work on both HashMap and Set (auto-detect via tag)

// .get(key_or_index) — HashMap: hash-based lookup returning Lax, List: index-based returning Lax
taida_val taida_collection_get(taida_val ptr, taida_val item) {
    if (taida_is_hashmap(ptr)) {
        taida_val key_hash = taida_value_hash(item);
        return taida_hashmap_get_lax(ptr, key_hash, item);
    }
    if (taida_is_bytes(ptr)) {
        return taida_bytes_get_lax(ptr, item);
    }
    // List: index-based access returning Lax
    return taida_list_get(ptr, item);
}

// .has(key_or_item) — HashMap: hash-based lookup, Set: linear scan
taida_val taida_collection_has(taida_val ptr, taida_val item) {
    if (taida_is_hashmap(ptr)) {
        taida_val key_hash = taida_value_hash(item);
        return taida_hashmap_has(ptr, key_hash, item);
    }
    // Set/List: linear scan
    return taida_set_has(ptr, item);
}

// .remove(key_or_item) — HashMap: hash-based removal, Set: linear scan
taida_val taida_collection_remove(taida_val ptr, taida_val item) {
    if (taida_is_hashmap(ptr)) {
        taida_val key_hash = taida_value_hash(item);
        return taida_hashmap_remove_immut(ptr, key_hash, item);
    }
    // Set: linear scan removal
    return taida_set_remove(ptr, item);
}

// .size() — works on both HashMap and Set (both store length at ptr[2])
taida_val taida_collection_size(taida_val ptr) {
    return ((taida_val*)ptr)[2];
}

// ── Error ceiling (setjmp/longjmp) ───────────────────────
// Uses setjmp/longjmp for error catching. The key function is
// taida_error_try_call which wraps setjmp and calls a function pointer.
#include <setjmp.h>

static __thread jmp_buf __taida_error_jmp[64];
static __thread taida_val __taida_error_val[64];
static __thread taida_val __taida_try_result[64];
static __thread int __taida_error_depth = 0;

taida_val taida_error_ceiling_push(void) {
    if (__taida_error_depth >= 64) {
        fprintf(stderr, "Error: maximum error handling depth exceeded (64)\n");
        exit(1);
    }
    int depth = __taida_error_depth++;
    return (taida_val)depth;
}

void taida_error_ceiling_pop(void) {
    if (__taida_error_depth > 0) __taida_error_depth--;
}

taida_val taida_throw(taida_val error_val) {
    if (__taida_error_depth > 0) {
        int depth = __taida_error_depth - 1;
        __taida_error_val[depth] = error_val;
        longjmp(__taida_error_jmp[depth], 1);
    }
    // No error ceiling: gorilla — print the actual error message
    taida_val msg = taida_throw_to_display_string(error_val);
    if (msg != 0) {
        fprintf(stderr, "Runtime error: %s\n", (const char*)msg);
    } else {
        fprintf(stderr, "Unhandled error (no error ceiling)\n");
    }
    exit(1);
    return 0;
}

// Try to execute a function pointer; if it throws, return 1 and store error.
// This wraps setjmp so the jmp_buf lives in THIS function's stack frame.
// fn_ptr: pointer to a 1-arg function (env_ptr) returning taida_val
// env_ptr: environment pack containing captured variables from parent scope
// Returns: 0 if fn completed normally, 1 if an error was thrown
taida_val taida_error_try_call(taida_val fn_ptr, taida_val env_ptr, taida_val depth) {
    typedef taida_val (*fn_t)(taida_val);
    fn_t func = (fn_t)fn_ptr;
    if (setjmp(__taida_error_jmp[(int)depth]) == 0) {
        __taida_try_result[(int)depth] = func(env_ptr);
        return 0; // normal completion
    } else {
        return 1; // error caught
    }
}

// Get the return value of the last successful try_call at the given depth
taida_val taida_error_try_get_result(taida_val depth) {
    return __taida_try_result[(int)depth];
}

// Legacy: for backward compat with existing IR that calls setjmp directly.
// This won't work properly from Cranelift code but is kept for reference.
taida_val taida_error_setjmp(taida_val depth) {
    return (taida_val)setjmp(__taida_error_jmp[(int)depth]);
}

taida_val taida_error_get_value(taida_val depth) {
    return __taida_error_val[(int)depth];
}

// RCB-101: Inheritance parent registry for error type filtering in |==
// Dynamic array — grows as needed to handle projects with many type hierarchies.
// NB2-7: Protected by mutex — realloc during registration could cause dangling
// pointers if a worker thread reads while the main thread grows the arrays.
static taida_val *__taida_type_parent_child = NULL;
static taida_val *__taida_type_parent_parent = NULL;
static int __taida_type_parent_count = 0;
static int __taida_type_parent_cap = 0;
static pthread_mutex_t __taida_type_parent_mutex = PTHREAD_MUTEX_INITIALIZER;

// Register an inheritance parent: child IS-A parent
void taida_register_type_parent(taida_val child_str, taida_val parent_str) {
    pthread_mutex_lock(&__taida_type_parent_mutex);
    if (__taida_type_parent_count >= __taida_type_parent_cap) {
        int new_cap = __taida_type_parent_cap == 0 ? 64 : __taida_type_parent_cap * 2;
        // Allocate both new arrays first, then copy + swap atomically.
        // This avoids stale pointers if one allocation fails.
        taida_val *new_child = (taida_val*)malloc(sizeof(taida_val) * new_cap);
        taida_val *new_parent = (taida_val*)malloc(sizeof(taida_val) * new_cap);
        if (!new_child || !new_parent) {
            free(new_child);
            free(new_parent);
            fprintf(stderr, "Warning: type parent registry allocation failed\n");
            pthread_mutex_unlock(&__taida_type_parent_mutex);
            return;
        }
        if (__taida_type_parent_count > 0) {
            memcpy(new_child, __taida_type_parent_child, sizeof(taida_val) * __taida_type_parent_count);
            memcpy(new_parent, __taida_type_parent_parent, sizeof(taida_val) * __taida_type_parent_count);
        }
        free(__taida_type_parent_child);
        free(__taida_type_parent_parent);
        __taida_type_parent_child = new_child;
        __taida_type_parent_parent = new_parent;
        __taida_type_parent_cap = new_cap;
    }
    __taida_type_parent_child[__taida_type_parent_count] = child_str;
    __taida_type_parent_parent[__taida_type_parent_count] = parent_str;
    __taida_type_parent_count++;
    pthread_mutex_unlock(&__taida_type_parent_mutex);
}

// Find the parent type string for a given child type string.
// Returns 0 if not found.
// NB2-7: Protected by mutex for safe concurrent reads during handler execution.
static taida_val taida_find_parent_type(taida_val child_str) {
    pthread_mutex_lock(&__taida_type_parent_mutex);
    taida_val result = 0;
    for (int i = 0; i < __taida_type_parent_count; i++) {
        if (taida_str_eq(__taida_type_parent_child[i], child_str)) {
            result = __taida_type_parent_parent[i];
            break;
        }
    }
    pthread_mutex_unlock(&__taida_type_parent_mutex);
    return result;
}

// Check if thrown_type IS-A handler_type by walking the inheritance chain.
// handler_type_str and thrown_type_str are C string pointers.
// Returns 1 if match, 0 if not.
taida_val taida_error_type_matches(taida_val error_val, taida_val handler_type_str) {
    // "Error" catches everything
    const char *handler_s = (const char*)handler_type_str;
    if (handler_s && strcmp(handler_s, "Error") == 0) return 1;

    // Get the thrown type from __type field of the BuchiPack.
    // Fall back to "type" field if __type is absent (legacy errors).
    taida_val thrown_type_str = 0;
    if (taida_is_buchi_pack(error_val)) {
        if (taida_pack_has_hash(error_val, (taida_val)HASH___TYPE)) {
            thrown_type_str = taida_pack_get(error_val, (taida_val)HASH___TYPE);
        } else if (taida_pack_has_hash(error_val, (taida_val)HASH_TYPE)) {
            thrown_type_str = taida_pack_get(error_val, (taida_val)HASH_TYPE);
        }
    }
    // RCB-101 fix: unknown type must NOT be catch-all.  Only the "Error"
    // handler (checked above) catches everything.  A typed handler like
    // |== e: MyError should not match an error with no type information.
    if (thrown_type_str == 0) return 0;

    // Walk inheritance chain
    taida_val current = thrown_type_str;
    for (int i = 0; i < 64; i++) {
        if (taida_str_eq(current, handler_type_str)) return 1;
        taida_val parent = taida_find_parent_type(current);
        if (parent == 0) break;
        current = parent;
    }
    return 0;
}

// B11B-015: Runtime type check for TypeIs with named types.
// Gets __type from the BuchiPack and walks the inheritance chain.
// Returns 1 (true) or 0 (false).
taida_val taida_typeis_named(taida_val val, taida_val expected_type_str) {
    if (!taida_is_buchi_pack(val)) return 0;
    taida_val type_str = 0;
    if (taida_pack_has_hash(val, (taida_val)HASH___TYPE)) {
        type_str = taida_pack_get(val, (taida_val)HASH___TYPE);
    }
    if (type_str == 0) return 0;
    // Direct match
    if (taida_str_eq(type_str, expected_type_str)) return 1;
    // Walk inheritance chain
    taida_val current = type_str;
    for (int i = 0; i < 64; i++) {
        taida_val parent = taida_find_parent_type(current);
        if (parent == 0) break;
        if (taida_str_eq(parent, expected_type_str)) return 1;
        current = parent;
    }
    return 0;
}

// RCB-101: Check error type and re-throw if it does not match.
// Called at the start of error ceiling handler arm.
// If the type matches, returns the error_val unchanged.
// If it does not match, calls taida_throw(error_val) which longjmps (never returns).
taida_val taida_error_type_check_or_rethrow(taida_val error_val, taida_val handler_type_str) {
    if (taida_error_type_matches(error_val, handler_type_str)) {
        return error_val;
    }
    // Re-throw: this longjmps to the next outer error ceiling
    taida_throw(error_val);
    return 0; // unreachable
}

taida_val taida_cage_apply(taida_val cage_value, taida_val fn_ptr) {
    if (fn_ptr == 0) {
        taida_val error = taida_make_error("CageError", "Cage second argument must be a function");
        return taida_gorillax_err(error);
    }

    taida_val depth = taida_error_ceiling_push();
    if (setjmp(__taida_error_jmp[(int)depth]) == 0) {
        taida_val result = taida_invoke_callback1(fn_ptr, cage_value);
        taida_error_ceiling_pop();
        return taida_gorillax_new(result);
    }

    taida_val error = taida_error_get_value(depth);
    taida_error_ceiling_pop();
    if (error == 0) {
        error = taida_make_error("CageError", "Cage function failed");
    }
    return taida_gorillax_err(error);
}

// ── Result[T, P] (v0.8.0 redesign — predicate support) ───
// Optional abolished in v0.8.0 — use Lax[T] instead.
// Result: operation mold — BuchiPack @(__value: T, __predicate: P, throw: Error, __type: "Result")
//   Layout: [refcount, field_count=4, hash0(__value), val0, hash1(__predicate), val1, hash2(throw), val2, hash3(__type), val3("Result")]
//   field 0: __value
//   field 1: __predicate (0 = no predicate, non-zero = function pointer)
//   field 2: throw (0 = Unit = success, non-zero = error)
//   field 3: __type ("Result" string)

// FNV-1a hashes for Result fields
#define HASH___TYPE            0x84d2d84b631f799bULL  // "__type"
#define HASH_RES___VALUE       0x0a7fc9f13472bbe0ULL  // "__value"
#define HASH_RES___PREDICATE   0x15592af3c2291540ULL  // "__predicate"
#define HASH_RES_THROW         0x5a5fe3720c9584cfULL  // "throw"

static const char __result_type_str[] = "Result";

// Throw payload must be type-confirmed to avoid pointer-guess heuristics.
static int taida_can_throw_payload(taida_val val) {
    if (val == 0) return 0;
    if (TAIDA_IS_PACK(val) || TAIDA_IS_LIST(val) || TAIDA_IS_HMAP(val) || TAIDA_IS_SET(val) || TAIDA_IS_ASYNC(val)) {
        return 1;
    }
    size_t sl = 0;
    return taida_read_cstr_len_safe((const char*)val, 65536, &sl);
}

// ── Result constructors ──

// C25B-028: register Result's `__predicate` and `throw` field names so
// jsonEncode (and any other name-lookup based renderer) can see them.
// Idempotent — C23B-009 pattern.
static void taida_register_result_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;
    taida_register_field_name((taida_val)HASH_RES___VALUE, (taida_val)"__value");
    taida_register_field_name((taida_val)HASH_RES___PREDICATE, (taida_val)"__predicate");
    taida_register_field_name((taida_val)HASH_RES_THROW, (taida_val)"throw");
    taida_register_field_name((taida_val)HASH___TYPE, (taida_val)"__type");
}

// Result[value, predicate](throw <= error) — create Result with optional predicate
taida_val taida_result_create(taida_val value, taida_val throw_val, taida_val predicate) {
    taida_register_result_field_names();
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_RES___VALUE);
    taida_pack_set(pack, 0, value);
    // retain-on-store: value が Pack/List/Closure の場合 retain
    // value の型は不明なので magic header で判定
    if (value > 4096 && taida_ptr_is_readable(value, sizeof(taida_val))) {
        taida_val vtag = ((taida_val*)value)[0] & TAIDA_MAGIC_MASK;
        if (vtag == TAIDA_PACK_MAGIC || vtag == TAIDA_LIST_MAGIC || vtag == TAIDA_CLOSURE_MAGIC) {
            taida_retain(value);
            // value の型タグも設定
            if (vtag == TAIDA_PACK_MAGIC) taida_pack_set_tag(pack, 0, TAIDA_TAG_PACK);
            else if (vtag == TAIDA_LIST_MAGIC) taida_pack_set_tag(pack, 0, TAIDA_TAG_LIST);
            else taida_pack_set_tag(pack, 0, TAIDA_TAG_CLOSURE);
        }
    }
    taida_pack_set_hash(pack, 1, (taida_val)HASH_RES___PREDICATE);
    taida_pack_set(pack, 1, predicate);  // 0 = no predicate, non-zero = function pointer
    if (predicate != 0) {
        taida_pack_set_tag(pack, 1, TAIDA_TAG_CLOSURE);
        taida_retain(predicate);  // retain-on-store: closure child
    }
    taida_pack_set_hash(pack, 2, (taida_val)HASH_RES_THROW);
    taida_pack_set(pack, 2, throw_val);  // 0 = success (Unit), non-zero = error
    if (throw_val != 0) {
        taida_pack_set_tag(pack, 2, TAIDA_TAG_PACK);
        taida_retain(throw_val);  // retain-on-store: pack child
    }
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__result_type_str);
    // __result_type_str is static - leave tag as INT(0)
    return pack;
}

// Helper: check if Result has error
// 1. If throw is set (not 0), it's an error — UNLESS predicate passes
// 2. If predicate exists, evaluate P(value) — true = success, false = error
// 3. No predicate + no throw = success (backward compatible)
static taida_val taida_result_is_error_check(taida_val result) {
    taida_val throw_val = taida_pack_get_idx(result, 2);  // throw
    taida_val pred = taida_pack_get_idx(result, 1);  // __predicate
    taida_val value = taida_pack_get_idx(result, 0);  // __value

    if (throw_val != 0) {
        // If predicate exists, evaluate it even when throw is set
        if (pred != 0) {
            taida_val pred_result = taida_invoke_callback1(pred, value);
            if (!pred_result) return 1;  // predicate failed — error
            return 0;  // predicate passed even though throw was set — success
        }
        return 1;  // throw set, no predicate — error
    }
    if (pred != 0) {
        taida_val pred_result = taida_invoke_callback1(pred, value);
        return pred_result ? 0 : 1;
    }
    return 0;  // no throw, no predicate — success
}

taida_val taida_result_is_ok(taida_val result) {
    return taida_result_is_error_check(result) ? 0 : 1;
}

taida_val taida_result_get_or_default(taida_val result, taida_val def) {
    if (!taida_result_is_error_check(result)) return taida_pack_get_idx(result, 0);
    return def;
}

taida_val taida_result_is_error(taida_val result) {
    return taida_result_is_error_check(result);
}

// ── Result methods (map, flatMap, mapError, getOrThrow, isError, toString) ──

// Result.map(fn) — if success, apply fn to __value
taida_val taida_result_map(taida_val result, taida_val fn_ptr) {
    if (taida_result_is_error_check(result)) {
        return result;  // Error: return as-is
    }
    taida_val value = taida_pack_get_idx(result, 0);  // __value
    taida_val new_val = taida_invoke_callback1(fn_ptr, value);
    return taida_result_create(new_val, 0, 0);  // success, no predicate
}

// Result.flatMap(fn) — if success, apply fn (which should return Result)
taida_val taida_result_flat_map(taida_val result, taida_val fn_ptr) {
    if (taida_result_is_error_check(result)) {
        return result;
    }
    taida_val value = taida_pack_get_idx(result, 0);  // __value
    taida_val new_result = taida_invoke_callback1(fn_ptr, value);
    return new_result;
}

// Result.mapError(fn) — if error, apply fn to throw value
taida_val taida_result_map_error(taida_val result, taida_val fn_ptr) {
    if (!taida_result_is_error_check(result)) {
        return result;  // Success: return as-is
    }
    taida_val throw_val = taida_pack_get_idx(result, 2);  // throw (shifted from idx 1 to idx 2)
    // Extract the error message string to pass to the mapping function
    // (matching interpreter: passes display string, not the Error BuchiPack)
    taida_val err_display = taida_throw_to_display_string(throw_val);
    taida_val mapped_str = taida_invoke_callback1(fn_ptr, err_display);
    // Wrap the mapped result back into an Error BuchiPack
    const char *new_msg = (const char*)mapped_str;
    size_t sl = 0;
    if (taida_read_cstr_len_safe(new_msg, 65536, &sl)) {
        taida_val new_error = taida_make_error("ResultError", new_msg);
        taida_str_release(mapped_str);
        taida_str_release(err_display);
        return taida_result_create(0, new_error, 0);
    }
    // Fallback: use mapped value as-is
    taida_str_release(err_display);
    return taida_result_create(0, mapped_str, 0);
}

// Result.getOrThrow() — if success return __value, otherwise throw
taida_val taida_result_get_or_throw(taida_val result) {
    if (!taida_result_is_error_check(result)) {
        return taida_pack_get_idx(result, 0);  // __value
    }
    taida_val throw_val = taida_pack_get_idx(result, 2);  // throw (shifted to idx 2)
    if (taida_can_throw_payload(throw_val)) {
        return taida_throw(throw_val);
    }
    // Fallback: create a generic error
    taida_val error = taida_make_error("ResultError", "Result predicate failed");
    return taida_throw(error);
}

// Result.toString() — "Result(value)" or "Result(throw <= ...)"
// Helper: render a throw value for display.
// TF-16: BuchiPack errors — extract the "message" field value
// (matching interpreter: shows just the message string, not the full pack structure).
static taida_val taida_throw_to_display_string(taida_val throw_val) {
    if (throw_val == 0) return (taida_val)taida_str_new_copy("error");
    // If it's a BuchiPack (Error TypeDef), extract the "message" field
    if (taida_is_buchi_pack(throw_val)) {
        if (taida_pack_has_hash(throw_val, (taida_val)HASH_MESSAGE)) {
            taida_val msg = taida_pack_get(throw_val, (taida_val)HASH_MESSAGE);
            if (msg != 0) {
                size_t sl = 0;
                if (taida_read_cstr_len_safe((const char*)msg, 65536, &sl)) {
                    return (taida_val)taida_str_new_copy((const char*)msg);
                }
            }
        }
        // Fallback: render full pack structure for non-message packs
        return taida_value_to_display_string(throw_val);
    }
    // String error message
    const char *s = (const char*)throw_val;
    size_t sl = 0;
    if (taida_read_cstr_len_safe(s, 65536, &sl)) {
        return (taida_val)taida_str_new_copy(s);
    }
    return taida_value_to_display_string(throw_val);
}

taida_val taida_result_to_string(taida_val result) {
    if (!taida_result_is_error_check(result)) {
        taida_val value = taida_pack_get_idx(result, 0);  // __value
        taida_val value_str = taida_value_to_display_string(value);
        const char *value_cstr = (const char*)value_str;
        size_t value_len = strlen(value_cstr);
        size_t need = value_len + 10;
        char *buf = taida_str_alloc(need);
        snprintf(buf, need + 1, "Result(%s)", value_cstr);
        taida_str_release(value_str);
        return (taida_val)buf;
    }
    taida_val throw_val = taida_pack_get_idx(result, 2);  // throw (shifted to idx 2)
    if (throw_val == 0) {
        return (taida_val)taida_str_new_copy("Result(throw <= error)");
    }
    taida_val err_disp = taida_throw_to_display_string(throw_val);
    const char *err_str = (const char*)err_disp;
    size_t elen = strlen(err_str);
    size_t need = elen + 24;
    char *buf = taida_str_alloc(need);
    snprintf(buf, need + 1, "Result(throw <= %s)", err_str);
    taida_str_release(err_disp);
    return (taida_val)buf;
}

// ── Lax methods (map, flatMap) ──────────────────────────────

// Lax.map(fn) — if hasValue, apply fn to __value and return new Lax
taida_val taida_lax_map(taida_val lax_ptr, taida_val fn_ptr) {
    if (!taida_pack_get_idx(lax_ptr, 0)) {
        // Empty Lax: return empty with same default
        taida_val def = taida_pack_get_idx(lax_ptr, 2);
        return taida_lax_empty(def);
    }
    taida_val value = taida_pack_get_idx(lax_ptr, 1);
    taida_val def = taida_pack_get_idx(lax_ptr, 2);
    taida_val result = taida_invoke_callback1(fn_ptr, value);
    return taida_lax_new(result, def);
}

// Lax.flatMap(fn) — if hasValue, apply fn (which should return Lax)
taida_val taida_lax_flat_map(taida_val lax_ptr, taida_val fn_ptr) {
    if (!taida_pack_get_idx(lax_ptr, 0)) {
        taida_val def = taida_pack_get_idx(lax_ptr, 2);
        return taida_lax_empty(def);
    }
    taida_val value = taida_pack_get_idx(lax_ptr, 1);
    taida_val result = taida_invoke_callback1(fn_ptr, value);
    // flatMap expects fn to return Lax — return directly
    return result;
}

// Lax.toString() — "Lax(value)" or "Lax(default: value)"
//
// C26B-011 (Phase 11): honour the FLOAT tag on `__value` / `__default`
// slots so Float-origin Lax renders `0.0` / `inf` / `-inf` / `NaN` to
// match interpreter (Rust f64 Display) and JS (`__taida_float_render`).
// Before this fix, `Div[1.0, 0.0]()` / `Div[1.0, 2.0]()` in native fell
// through to `taida_value_to_display_string` which treated the f64 bit
// pattern as an Int pointer and either rendered `0` (matching `b == 0`
// short-circuit where the default was plain `0`) or lost the `.0`
// entirely. `taida_float_to_str` already has NaN/Inf/denormal parity
// with Rust — we just need to route FLOAT-tagged slots through it.
taida_val taida_lax_to_string(taida_val lax_ptr) {
    taida_val val = taida_pack_get_idx(lax_ptr, 1);
    taida_val def = taida_pack_get_idx(lax_ptr, 2);
    int hasv = (int)taida_pack_get_idx(lax_ptr, 0);
    taida_val slot_tag = taida_pack_get_tag_idx(lax_ptr, hasv ? 1 : 2);
    taida_val rendered;
    if (slot_tag == TAIDA_TAG_FLOAT) {
        double d;
        taida_val bits = hasv ? val : def;
        memcpy(&d, &bits, sizeof(double));
        rendered = taida_float_to_str(d);
    } else {
        rendered = hasv
            ? taida_value_to_display_string(val)
            : taida_value_to_display_string(def);
    }
    const char *rs = (const char*)rendered;
    size_t need = strlen(rs) + 24;
    char *buf = taida_str_alloc(need);
    if (hasv) {
        snprintf(buf, need + 1, "Lax(%s)", rs);
    } else {
        snprintf(buf, need + 1, "Lax(default: %s)", rs);
    }
    taida_str_release(rendered);
    return (taida_val)buf;
}

// ── Polymorphic monadic dispatch ──────────────────────────
// These functions detect the type at runtime and dispatch to the correct impl.
// Type detection uses BuchiPack field_count + first field hash:
//   - field_count == 4, hash0 == HASH_RES___VALUE → Result (__value, __predicate, throw, __type)
//   - field_count == 4, hash0 == HASH_HAS_VALUE   → Lax (hasValue, __value, __default, __type)
//   - otherwise → List (check via capacity/length heuristic)
// Note: Optional (fc==2) was abolished in v0.8.0.
// taida_monadic_field_count returns stable type IDs:
//   3 = Result (for backward compat with all dispatch code)
//   4 = Lax/Gorillax/RelaxedGorillax

static int taida_is_list(taida_val ptr) {
    return TAIDA_IS_LIST(ptr);
}

static int taida_is_bytes(taida_val ptr) {
    return TAIDA_IS_BYTES(ptr);
}

static int taida_monadic_field_count(taida_val ptr) {
    if (!taida_ptr_is_readable(ptr, sizeof(taida_val) * 3)) return 0;
    taida_val *obj = (taida_val*)ptr;
    taida_val fc = obj[1];
    // Both Result and Lax are now fc=4; distinguish by hash0
    if (fc == 4) {
        taida_val hash0 = obj[2];
        if (hash0 > 0x10000 || hash0 < 0) {
            // Result (fc=4, hash0=HASH_RES___VALUE) → return 3 for compat
            if (hash0 == (taida_val)HASH_RES___VALUE) return 3;
            // Lax/Gorillax/RelaxedGorillax (fc=4, hash0=HASH_HAS_VALUE) → return 4
            if (hash0 == (taida_val)HASH_HAS_VALUE) return 4;
        }
    }
    return 0;
}

// ── Async pthread support ────────────────────────────────────
// Thread argument: passed to pthread entry, stores callback + result pointer.
typedef struct {
    taida_val fn_ptr;
    taida_val arg;               // callback argument
    taida_val *async_obj;        // back-pointer to Async object (writes value/status)
} taida_thread_arg;

// NO-3: Detect the type tag of a runtime value by inspecting its magic header.
// Returns TAIDA_TAG_* constant. Used by thread entry to set value_tag dynamically.
static taida_val taida_detect_value_tag(taida_val val) {
    if (val == 0) return TAIDA_TAG_INT;
    if (val > 0 && val < 4096) return TAIDA_TAG_INT;  // small integer
    if (val < 0) return TAIDA_TAG_INT;  // negative integer (or float-as-bits, but conservative)
    if (!taida_ptr_is_readable(val, sizeof(taida_val))) return TAIDA_TAG_INT;
    taida_val *obj = (taida_val*)val;
    taida_val magic = obj[0] & TAIDA_MAGIC_MASK;
    if (magic == TAIDA_PACK_MAGIC) return TAIDA_TAG_PACK;
    if (magic == TAIDA_LIST_MAGIC) return TAIDA_TAG_LIST;
    if (magic == TAIDA_CLOSURE_MAGIC) return TAIDA_TAG_CLOSURE;
    if (magic == TAIDA_HMAP_MAGIC) return TAIDA_TAG_HMAP;
    if (magic == TAIDA_SET_MAGIC) return TAIDA_TAG_SET;
    if (magic == TAIDA_ASYNC_MAGIC) return TAIDA_TAG_PACK;  // Async uses PACK tag for retain/release
    if (magic == TAIDA_STR_MAGIC) return TAIDA_TAG_STR;
    // Check hidden-header String: ptr-16 may contain STR_MAGIC.
    // Same pattern as taida_str_release.
    {
        taida_val *hdr = ((taida_val*)val) - 2;
        if (taida_ptr_is_readable((taida_val)hdr, sizeof(taida_val))) {
            taida_val htag = hdr[0] & TAIDA_MAGIC_MASK;
            if (htag == TAIDA_STR_MAGIC) return TAIDA_TAG_STR;
        }
    }
    // Could be a raw char* or an integer pointer.
    // Conservative: return UNKNOWN to avoid misidentifying ints as pointers.
    return TAIDA_TAG_UNKNOWN;
}

// pthread entry point: call the function, write result into the Async object.
static void* taida_thread_entry(void* raw) {
    taida_thread_arg *ta = (taida_thread_arg*)raw;
    taida_val result = taida_invoke_callback1(ta->fn_ptr, ta->arg);
    // NO-3: detect value type and store tag for recursive release on drop.
    // Move semantics: the callback result is transferred to the Async object.
    taida_val vtag = taida_detect_value_tag(result);
    ta->async_obj[2] = result;   // write value
    ta->async_obj[5] = vtag;     // set value_tag
    __atomic_thread_fence(__ATOMIC_RELEASE);  // barrier: ensure value+tag visible before status
    ta->async_obj[1] = 1;        // mark fulfilled (must be last — signals to readers)
    free(ta);
    return NULL;
}

// Detect Async value: [ASYNC_MAGIC, status, value, error, thread_handle, value_tag, error_tag]
// Uses a magic number in slot[0] for unambiguous identification.
static int taida_is_async(taida_val ptr) {
    return TAIDA_IS_ASYNC(ptr);
}

// Detect BuchiPack of any size (fc >= 1, with FNV-1a hash check)
static int taida_is_buchi_pack(taida_val ptr) {
    return TAIDA_IS_PACK(ptr);
}

// Forward declare recursive value-to-display-string
// NO-4 RULE 2: These functions return heap-allocated strings via taida_str_new_copy
// or taida_str_alloc. The CALLER is responsible for calling taida_str_release on
// the returned value after use. Intermediate strings generated during recursive
// formatting (e.g., item_str in list display) are released within the function.
static taida_val taida_value_to_display_string(taida_val val);
static taida_val taida_value_to_debug_string(taida_val val);
// C23B-003 reopen 2: debug-string variant that recurses into the
// synthetic full-form helpers for HashMap / Set / BuchiPack. Used only
// inside `taida_hashmap_to_display_string_full` / `_set_…_full` /
// `taida_pack_to_display_string_full` so nested typed runtime objects
// keep their full-form rendering (matching the interpreter's
// `Value::to_debug_string()` on BuchiPack, which itself recurses).
static taida_val taida_value_to_debug_string_full(taida_val val);
static taida_val taida_hashmap_to_display_string_full(taida_val hm_ptr);
static taida_val taida_set_to_display_string_full(taida_val set_ptr);
static taida_val taida_pack_to_display_string_full(taida_val pack_ptr);

// Convert a list to display string: @[item1, item2, ...]
static taida_val taida_list_to_display_string(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val list_len = list[2];
    size_t cap = 64;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "list_to_string");
    buf[0] = '\0';
    // Append "@["
    { const char *s = "@["; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0'; }
    for (taida_val i = 0; i < list_len; i++) {
        if (i > 0) {
            const char *s = ", "; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0';
        }
        taida_val item = list[4 + i];
        taida_val item_str = taida_value_to_debug_string(item);
        const char *is = (const char*)item_str;
        if (is) {
            size_t sl = strlen(is); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, is, sl); len += sl; buf[len] = '\0';
        }
        taida_str_release(item_str);
    }
    // Append "]"
    while (len + 2 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
    buf[len++] = ']'; buf[len] = '\0';
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

static taida_val taida_bytes_to_display_string(taida_val bytes_ptr) {
    if (!TAIDA_IS_BYTES(bytes_ptr)) {
        return (taida_val)taida_str_new_copy("Bytes[@[]]");
    }
    taida_val *bytes = (taida_val*)bytes_ptr;
    taida_val len_bytes = bytes[1];
    size_t cap = 64;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "bytes_to_string");
    buf[0] = '\0';
    const char *prefix = "Bytes[@[";
    size_t pl = strlen(prefix);
    while (len + pl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
    memcpy(buf + len, prefix, pl);
    len += pl;
    buf[len] = '\0';

    for (taida_val i = 0; i < len_bytes; i++) {
        if (i > 0) {
            const char *sep = ", ";
            size_t sl = 2;
            while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
            memcpy(buf + len, sep, sl);
            len += sl;
            buf[len] = '\0';
        }
        char nbuf[8];
        int wrote = snprintf(nbuf, sizeof(nbuf), "%" PRId64 "", bytes[2 + i]);
        if (wrote < 0) wrote = 0;
        size_t sl = (size_t)wrote;
        while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
        memcpy(buf + len, nbuf, sl);
        len += sl;
        buf[len] = '\0';
    }

    const char *suffix = "]]";
    size_t sl = 2;
    while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
    memcpy(buf + len, suffix, sl);
    len += sl;
    buf[len] = '\0';
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

// Convert a BuchiPack to display string: @(field <= value, ...)
static taida_val taida_pack_to_display_string(taida_val pack_ptr) {
    taida_val *pack = (taida_val*)pack_ptr;
    taida_val fc = pack[1];
    size_t cap = 128;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "pack_to_display_string");
    buf[0] = '\0';
    // Append "@("
    { const char *s = "@("; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0'; }
    int count = 0;
    for (taida_val i = 0; i < fc; i++) {
        taida_val field_hash = pack[2 + i * 3];
        taida_val field_tag  = pack[2 + i * 3 + 1];
        taida_val field_val  = pack[2 + i * 3 + 2];
        const char *fname = taida_lookup_field_name(field_hash);
        if (!fname) continue;
        // Skip internal __ fields for display
        if (fname[0] == '_' && fname[1] == '_') continue;
        if (count > 0) {
            const char *s = ", "; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0';
        }
        // Append "fieldname <= "
        size_t nlen = strlen(fname);
        while (len + nlen + 5 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
        memcpy(buf + len, fname, nlen); len += nlen;
        memcpy(buf + len, " <= ", 4); len += 4;
        buf[len] = '\0';
        // C21B-seed-07: per-field tag takes precedence over the global
        // field-name/type registry (see _full counterpart for the rationale).
        int ftype = taida_lookup_field_type(field_hash);
        int render_bool  = (field_tag == TAIDA_TAG_BOOL) || (field_tag == 0 && ftype == 4);
        int render_float = (field_tag == TAIDA_TAG_FLOAT);
        if (render_bool) {
            // Bool: display as true/false
            const char *bv = field_val ? "true" : "false";
            size_t sl = strlen(bv); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, bv, sl); len += sl; buf[len] = '\0';
        } else if (render_float) {
            double d;
            memcpy(&d, &field_val, sizeof(double));
            taida_val fstr = taida_float_to_str(d);
            const char *fs = (const char*)fstr;
            if (fs) {
                size_t sl = strlen(fs); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, fs, sl); len += sl; buf[len] = '\0';
            }
            taida_str_release(fstr);
        } else {
            // Append value (debug string: strings are quoted)
            taida_val val_str = taida_value_to_debug_string(field_val);
            const char *vs = (const char*)val_str;
            if (vs) {
                size_t sl = strlen(vs); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, vs, sl); len += sl; buf[len] = '\0';
            }
            taida_str_release(val_str);
        }
        count++;
    }
    // Append ")"
    while (len + 2 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); }
    buf[len++] = ')'; buf[len] = '\0';
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

// TF-15: Pack to display string with ALL fields (including __ internal fields).
// Matches interpreter's to_display_string() for BuchiPack which shows all fields.
//
// C21B-seed-07: The per-field tag (`pack[2 + i*3 + 1]`) is now authoritative
// for primitive rendering — the global field-name/type registry is only
// consulted as a safety net for compile-time-tagged Bool fields (which
// predate the per-field tag propagation on Lax constructors). Without this,
// a Lax built by `taida_float_mold_float` would render its `__value` as the
// raw int64 bit-pattern of the f64 (= `4613937818241073152` for `3.0`).
static taida_val taida_pack_to_display_string_full(taida_val pack_ptr) {
    taida_val *pack = (taida_val*)pack_ptr;
    taida_val fc = pack[1];
    size_t cap = 128;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "pack_to_display_string_full");
    buf[0] = '\0';
    // Append "@("
    { const char *s = "@("; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0'; }
    int count = 0;
    for (taida_val i = 0; i < fc; i++) {
        taida_val field_hash = pack[2 + i * 3];
        taida_val field_tag  = pack[2 + i * 3 + 1];
        taida_val field_val  = pack[2 + i * 3 + 2];
        const char *fname = taida_lookup_field_name(field_hash);
        if (!fname) continue;
        // NOTE: Unlike taida_pack_to_display_string, we do NOT skip __ fields
        if (count > 0) {
            const char *s = ", "; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0';
        }
        // Append "fieldname <= "
        size_t nlen = strlen(fname);
        while (len + nlen + 5 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); }
        memcpy(buf + len, fname, nlen); len += nlen;
        memcpy(buf + len, " <= ", 4); len += 4;
        buf[len] = '\0';
        // Per-field tag takes precedence over the global registry. The
        // registry is only consulted when the per-field tag is the default
        // zero (= TAIDA_TAG_INT) AND the registry entry explicitly marks
        // the field as Bool (legacy pattern used by Lax's `hasValue`).
        int ftype = taida_lookup_field_type(field_hash);
        // Note: taida_lookup_field_type uses tag *4* to mean Bool (legacy
        // convention pre-dating C21B-seed-07). Keep that mapping intact so
        // Lax's `hasValue` field continues to print `true`/`false`.
        int render_bool   = (field_tag == TAIDA_TAG_BOOL) || (field_tag == 0 && ftype == 4);
        int render_float  = (field_tag == TAIDA_TAG_FLOAT);
        // C23B-003 reopen: `__error == 0` with a PACK-tagged slot represents
        // `Value::Unit` (absence of error) on Gorillax. The interpreter's
        // `to_debug_string()` on `Value::Unit` returns `"@()"`, not `"0"`.
        int render_unit_pack = (field_tag == TAIDA_TAG_PACK && field_val == 0);
        // C24-B (2026-04-23): explicit INT branch — symmetric with WASM's
        // `_wasm_pack_to_string_full` `render_int` guard (C23B-005). Before
        // this guard, `zip(@[1,2,3], @["x","y","z"])` crashed: the pair
        // pack's `first` slot carried an INT value (e.g. `1`) with the
        // source list's elem_type_tag (TAIDA_TAG_INT = 0) stamped on the
        // pack field. Without a dedicated INT render branch, the `else`
        // fell into `taida_value_to_debug_string_full(1)`, which
        // dereferenced `(char*)1` and segfaulted on
        // `taida_read_cstr_len_safe`. The guard uses
        // `!render_bool` to keep Lax's `hasValue` (INT tag +
        // legacy ftype-4 registry hint) rendering as `true`/`false`.
        int render_int = (field_tag == TAIDA_TAG_INT) && !render_bool && !render_unit_pack;
        // C24-B: explicit STR branch — symmetric with WASM. Without it,
        // `zip`'s `second` slot for a string list would fall into
        // `taida_value_to_debug_string_full` which has a char* path, but
        // going through the explicit branch here both matches WASM's
        // layout and adds defence in depth against heap-address aliasing
        // in the structural detectors.
        int render_str = (field_tag == TAIDA_TAG_STR);
        if (render_bool) {
            const char *bv = field_val ? "true" : "false";
            size_t sl = strlen(bv); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, bv, sl); len += sl; buf[len] = '\0';
        } else if (render_float) {
            double d;
            memcpy(&d, &field_val, sizeof(double));
            taida_val fstr = taida_float_to_str(d);
            const char *fs = (const char*)fstr;
            if (fs) {
                size_t sl = strlen(fs); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, fs, sl); len += sl; buf[len] = '\0';
            }
            taida_str_release(fstr);
        } else if (render_unit_pack) {
            const char *uv = "@()";
            size_t sl = 3; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, uv, sl); len += sl; buf[len] = '\0';
        } else if (render_int) {
            char tmp[32];
            snprintf(tmp, sizeof(tmp), "%" PRId64 "", (int64_t)field_val);
            size_t sl = strlen(tmp); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, tmp, sl); len += sl; buf[len] = '\0';
        } else if (render_str) {
            const char *sv = (const char*)field_val;
            size_t sl = 0;
            if (sv && taida_read_cstr_len_safe(sv, 65536, &sl)) {
                while (len + sl + 3 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); }
                buf[len++] = '"';
                memcpy(buf + len, sv, sl); len += sl;
                buf[len++] = '"';
                buf[len] = '\0';
            } else {
                // Fallback: unresolvable string slot, render as "" quotes
                while (len + 2 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); }
                buf[len++] = '"'; buf[len++] = '"'; buf[len] = '\0';
            }
        } else {
            // C23B-003 reopen 2: nested typed runtime objects must keep
            // their full-form rendering. The short-form
            // `taida_value_to_debug_string` would dispatch HashMap / Set
            // to their `HashMap({...})` / `Set({...})` `.toString()`
            // helpers. Using `_full` ensures `@(__entries <= …, __type <= …)`
            // shows up recursively (matches interpreter's
            // `BuchiPack.to_display_string()` which walks fields via
            // `to_debug_string()` → recursive `to_display_string()`).
            taida_val val_str = taida_value_to_debug_string_full(field_val);
            const char *vs = (const char*)val_str;
            if (vs) {
                size_t sl = strlen(vs); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, vs, sl); len += sl; buf[len] = '\0';
            }
            taida_str_release(val_str);
        }
        count++;
    }
    while (len + 2 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); }
    buf[len++] = ')'; buf[len] = '\0';
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

// Convert any Taida value to its display string (like interpreter's to_display_string)
static taida_val taida_value_to_display_string(taida_val val) {
    if (val == 0) {
        return (taida_val)taida_str_new_copy("0");
    }
    // Precise object checks using magics first.
    if (taida_is_hashmap(val)) return taida_hashmap_to_string(val);
    if (taida_is_set(val)) return taida_set_to_string(val);
    if (taida_is_async(val)) return taida_async_to_string(val);
    if (taida_is_list(val)) return taida_list_to_display_string(val);
    if (taida_is_bytes(val)) return taida_bytes_to_display_string(val);

    // Check for BuchiPack (including monadic types)
    if (taida_is_buchi_pack(val)) {
        int fc = taida_monadic_field_count(val);
        if (fc == 3) return taida_result_to_string(val);
        if (fc == 4) {
            int gtype = taida_detect_gorillax_type(val);
            if (gtype == 1) return taida_gorillax_to_string(val);
            if (gtype == 2) return taida_relaxed_gorillax_to_string(val);
            return taida_lax_to_string(val);
        }
        return taida_pack_to_display_string(val);
    }

    // Check if it's a safely readable string (char*).
    const char *s = (const char*)val;
    size_t sl = 0;
    if (taida_read_cstr_len_safe(s, 65536, &sl)) {
        char *r = taida_str_alloc(sl);
        memcpy(r, s, sl);
        return (taida_val)r;
    }
    // Fallback: it's an integer.
    char tmp[32]; snprintf(tmp, sizeof(tmp), "%" PRId64 "", val); return (taida_val)taida_str_new_copy(tmp);
}

// Convert value to debug string (strings are quoted, everything else like display)
static taida_val taida_value_to_debug_string(taida_val val) {
    if (val == 0) {
        return (taida_val)taida_str_new_copy("0");
    }
    // Check for objects first using magics
    if (taida_is_hashmap(val)) return taida_hashmap_to_string(val);
    if (taida_is_set(val)) return taida_set_to_string(val);
    if (taida_is_async(val)) return taida_async_to_string(val);
    if (taida_is_list(val)) return taida_list_to_display_string(val);
    if (taida_is_bytes(val)) return taida_bytes_to_display_string(val);
    if (taida_is_buchi_pack(val)) {
        int fc = taida_monadic_field_count(val);
        if (fc == 3) return taida_result_to_string(val);
        if (fc == 4) return taida_lax_to_string(val);
        return taida_pack_to_display_string(val);
    }

    // Check for string (quoted in debug output)
    const char *s = (const char*)val;
    size_t sl = 0;
    if (taida_read_cstr_len_safe(s, 65536, &sl)) {
        char *r = taida_str_alloc(sl + 2);
        r[0] = '"';
        memcpy(r + 1, s, sl);
        r[sl + 1] = '"';
        return (taida_val)r;
    }
    // Fallback: integer
    char tmp[32]; snprintf(tmp, sizeof(tmp), "%" PRId64 "", val); return (taida_val)taida_str_new_copy(tmp);
}

// C23B-003 reopen 2: recursive debug-string variant for nested typed
// runtime objects. Used *inside* the full-form helpers so nested
// HashMap / Set / BuchiPack keep their interpreter-parity full-form
// rendering instead of collapsing to the short-form `.toString()`
// dispatch that `taida_value_to_debug_string` uses for flat
// `.toString()` callers.
//
// Interpreter reference: `Value::BuchiPack.to_display_string()` walks
// fields calling `to_debug_string()` which, for non-Str values, is the
// same recursive `to_display_string()` — so nested HashMap (which the
// interpreter models as `BuchiPack(__entries, __type)`) expands to the
// full `@(__entries <= …, __type <= "HashMap")` form, not the short
// `HashMap({…})` form.
static taida_val taida_value_to_debug_string_full(taida_val val) {
    if (val == 0) return (taida_val)taida_str_new_copy("0");
    if (taida_is_hashmap(val)) return taida_hashmap_to_display_string_full(val);
    if (taida_is_set(val)) return taida_set_to_display_string_full(val);
    if (taida_is_async(val)) return taida_async_to_string(val);
    if (taida_is_list(val)) {
        // List itself uses `@[...]` format (already matches interpreter);
        // its items must recurse through the full-form helper so nested
        // HashMap/Set/Pack inside the list stay in full form.
        taida_val *list = (taida_val*)val;
        taida_val list_len = list[2];
        size_t cap = 64;
        size_t len = 0;
        char *buf = (char*)TAIDA_MALLOC(cap, "list_to_string_full");
        buf[0] = '\0';
        { const char *s = "@["; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0'; }
        for (taida_val i = 0; i < list_len; i++) {
            if (i > 0) {
                const char *s = ", "; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0';
            }
            taida_val item = list[4 + i];
            taida_val item_str = taida_value_to_debug_string_full(item);
            const char *is = (const char*)item_str;
            if (is) {
                size_t sl = strlen(is); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, is, sl); len += sl; buf[len] = '\0';
            }
            taida_str_release(item_str);
        }
        while (len + 2 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); }
        buf[len++] = ']'; buf[len] = '\0';
        taida_val result = (taida_val)taida_str_new_copy(buf);
        free(buf);
        return result;
    }
    if (taida_is_bytes(val)) return taida_bytes_to_display_string(val);
    if (taida_is_buchi_pack(val)) {
        // Route every pack shape through the full-form renderer so
        // nested Lax / Result / user-packs all render with their
        // `__`-prefixed internals exposed (matches interpreter's
        // `BuchiPack.to_display_string()`).
        return taida_pack_to_display_string_full(val);
    }
    // Quoted string for non-pack Str (same as the short helper).
    const char *s = (const char*)val;
    size_t sl = 0;
    if (taida_read_cstr_len_safe(s, 65536, &sl)) {
        char *r = taida_str_alloc(sl + 2);
        r[0] = '"';
        memcpy(r + 1, s, sl);
        r[sl + 1] = '"';
        return (taida_val)r;
    }
    char tmp[32]; snprintf(tmp, sizeof(tmp), "%" PRId64 "", val); return (taida_val)taida_str_new_copy(tmp);
}

// Polymorphic .getOrDefault(fallback) — works on Result, Lax
taida_val taida_polymorphic_get_or_default(taida_val obj, taida_val def) {
    if (obj == 0 || obj < 4096) return def;
    // Check Async first (before monadic, since Async has different layout)
    if (taida_is_async(obj)) return taida_async_get_or_default(obj, def);
    int fc = taida_monadic_field_count(obj);
    if (fc == 3) return taida_result_get_or_default(obj, def);    // Result
    if (fc == 4) return taida_lax_get_or_default(obj, def);       // Lax
    return def;
}

// Polymorphic .hasValue() — works on Lax
taida_val taida_polymorphic_has_value(taida_val obj) {
    if (obj == 0 || obj < 4096) return 0;
    int fc = taida_monadic_field_count(obj);
    if (fc == 4) return taida_pack_get_idx(obj, 0);     // Lax: hasValue field
    return 0;
}

// Polymorphic .isEmpty() — works on List, Lax
taida_val taida_polymorphic_is_empty(taida_val obj) {
    if (obj == 0 || obj < 4096) return 1;
    // Check for HashMap
    if (taida_is_hashmap(obj)) return taida_hashmap_is_empty(obj);
    // Check for Set (uses same layout as list, so list_is_empty works)
    if (taida_is_set(obj)) return taida_set_is_empty(obj);
    if (taida_is_bytes(obj)) return taida_bytes_len(obj) == 0 ? 1 : 0;
    int fc = taida_monadic_field_count(obj);
    if (fc == 4) return taida_pack_get_idx(obj, 0) ? 0 : 1;  // Lax
    // Default: treat as list
    return taida_list_is_empty(obj);
}

// Polymorphic .toString() — works on Int, Float, Bool, Result, Lax, HashMap, Set, List, BuchiPack
taida_val taida_polymorphic_to_string(taida_val obj) {
    // RCB-222: Check for user-defined toString method on BuchiPack types.
    // If the pack has a function field named "toString", call it instead of
    // formatting as @(field <= value, ...). This matches the Interpreter's
    // type_methods dispatch behavior.
    if (taida_is_buchi_pack(obj)) {
        // FNV-1a hash of "toString"
        const taida_val toString_hash = 0xc5c8cdb28370e485ULL;
        taida_val fn_ptr = taida_pack_get(obj, toString_hash);
        if (fn_ptr != 0 && (TAIDA_IS_CLOSURE(fn_ptr) || taida_ptr_is_readable(fn_ptr, 1))) {
            // Check if it looks like a function (closure or function pointer)
            if (TAIDA_IS_CLOSURE(fn_ptr)) {
                taida_val *closure = (taida_val*)fn_ptr;
                taida_closure_cb0_fn closure_fn = (taida_closure_cb0_fn)closure[1];
                taida_val env_ptr = closure[2];
                return closure_fn(env_ptr);
            }
            // Plain function pointer — but we need to distinguish from non-function values.
            // Function pointers are in code segment, not heap. We cannot reliably distinguish
            // them from string pointers at runtime, so only dispatch closures here.
            // Non-closure toString fields (e.g., string values) fall through to default display.
        }
    }
    return taida_value_to_display_string(obj);
}

// TF-15: stdout display — renders BuchiPacks with ALL fields (including __ internal fields)
// matching the interpreter's to_display_string() behavior.
// .toString() methods use taida_polymorphic_to_string which produces Lax(...)/Result(...) forms.
//
// C23B-003 reopen: HashMap / Set are not `BuchiPack` in the native runtime
// (they carry dedicated magic-tagged layouts) but the interpreter represents
// them as `BuchiPack(__entries <= ..., __type <= "HashMap")` /
// `BuchiPack(__items <= ..., __type <= "Set")` (see
// `src/interpreter/prelude.rs:618-621` and `:644-647`). For `Str[...]()` to
// match the interpreter byte-for-byte we must therefore emit the synthetic
// full-form pack shape, not the short-form `HashMap({...})` /
// `Set({...})` that `taida_hashmap_to_string` / `taida_set_to_string`
// produce for `.toString()`.
static taida_val taida_hashmap_to_display_string_full(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    size_t buf_size = 256;
    size_t off = 0;
    char *buf = (char*)TAIDA_MALLOC(buf_size, "hm_display_full");
    memcpy(buf, "@(__entries <= @[", 17); off = 17;
    taida_val count = 0;
    // C23B-008 (2026-04-22): walk the insertion-order side-index so the
    // emitted pair sequence matches the interpreter's Vec<(k,v)> order.
    // Holes (order slot == -1) and tombstoned entries (bucket no longer
    // occupied) are both skipped.
    taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(cap)];
    for (taida_val oi = 0; oi < next_ord; oi++) {
        taida_val slot = hm[TAIDA_HM_ORD_SLOT(cap, oi)];
        if (slot < 0 || slot >= cap) continue;
        taida_val sh = hm[HM_HEADER + slot * 3];
        taida_val sk = hm[HM_HEADER + slot * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val value = hm[HM_HEADER + slot * 3 + 2];
            // C23B-003 reopen 2: nested typed runtime objects must recurse
            // through the full-form helper so nested HashMap/Set/BuchiPack
            // keep their `@(__entries …)` / `@(__items …)` / full pack
            // shape instead of collapsing to the `.toString()` short-form.
            taida_val key_str_ptr = taida_value_to_debug_string_full(sk);
            taida_val val_str_ptr = taida_value_to_debug_string_full(value);
            const char *key_str = (const char*)key_str_ptr;
            const char *val_str = (const char*)val_str_ptr;
            if (!key_str) key_str = "\"\"";
            if (!val_str) val_str = "0";
            size_t klen = strlen(key_str);
            size_t vlen = strlen(val_str);
            // "@(key <= <k>, value <= <v>)" + optional ", " prefix
            size_t needed = klen + vlen + 22;
            if (count > 0) needed += 2;
            while (off + needed + 32 > buf_size) {
                buf_size *= 2;
                TAIDA_REALLOC(buf, buf_size, "hm_display_full");
            }
            if (count > 0) { memcpy(buf + off, ", ", 2); off += 2; }
            memcpy(buf + off, "@(key <= ", 9); off += 9;
            memcpy(buf + off, key_str, klen); off += klen;
            memcpy(buf + off, ", value <= ", 11); off += 11;
            memcpy(buf + off, val_str, vlen); off += vlen;
            buf[off++] = ')';
            buf[off] = '\0';
            taida_str_release(key_str_ptr);
            taida_str_release(val_str_ptr);
            count++;
        }
    }
    const char *suffix = "], __type <= \"HashMap\")";
    size_t slen = strlen(suffix);
    while (off + slen + 1 > buf_size) {
        buf_size *= 2;
        TAIDA_REALLOC(buf, buf_size, "hm_display_full");
    }
    memcpy(buf + off, suffix, slen); off += slen;
    buf[off] = '\0';
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

static taida_val taida_set_to_display_string_full(taida_val set_ptr) {
    // Set uses List layout internally (same as taida_list_to_display_string):
    //   set[0] = magic, set[1] = capacity, set[2] = length, set[3] = type_tag,
    //   set[4..4+len] = items.
    taida_val *set = (taida_val*)set_ptr;
    taida_val set_len = set[2];
    size_t buf_size = 256;
    size_t off = 0;
    char *buf = (char*)TAIDA_MALLOC(buf_size, "set_display_full");
    memcpy(buf, "@(__items <= @[", 15); off = 15;
    for (taida_val i = 0; i < set_len; i++) {
        taida_val item = set[4 + i];
        // C23B-003 reopen 2: recurse into full-form for nested
        // HashMap/Set/Pack items.
        taida_val item_str = taida_value_to_debug_string_full(item);
        const char *is = (const char*)item_str;
        if (!is) is = "0";
        size_t ilen = strlen(is);
        size_t needed = ilen + 2;
        if (i > 0) needed += 2;
        while (off + needed + 32 > buf_size) {
            buf_size *= 2;
            TAIDA_REALLOC(buf, buf_size, "set_display_full");
        }
        if (i > 0) { memcpy(buf + off, ", ", 2); off += 2; }
        memcpy(buf + off, is, ilen); off += ilen;
        buf[off] = '\0';
        taida_str_release(item_str);
    }
    const char *suffix = "], __type <= \"Set\")";
    size_t slen = strlen(suffix);
    while (off + slen + 1 > buf_size) {
        buf_size *= 2;
        TAIDA_REALLOC(buf, buf_size, "set_display_full");
    }
    memcpy(buf + off, suffix, slen); off += slen;
    buf[off] = '\0';
    taida_val result = (taida_val)taida_str_new_copy(buf);
    free(buf);
    return result;
}

taida_val taida_stdout_display_string(taida_val obj) {
    if (obj == 0) return (taida_val)taida_str_new_copy("0");
    // C25B-001: Stream packs render as `Stream[completed: N items]`
    // (matching the interpreter's `Value::Stream` display shape). Must
    // precede the generic BuchiPack path so `Str[Stream[...]]()` doesn't
    // stringify the internal `__stream_count` / `__stream_status` fields.
    if (taida_is_stream_pack(obj)) return taida_stream_to_display_string(obj);
    // C23B-003 reopen: route HashMap / Set through their synthetic full-form
    // helpers so `Str[...]()` matches the interpreter's
    // `BuchiPack(__entries/__items, __type)` rendering instead of the
    // short-form `HashMap({...})` / `Set({...})` that
    // `taida_value_to_display_string` would produce for `.toString()`.
    if (taida_is_hashmap(obj)) return taida_hashmap_to_display_string_full(obj);
    if (taida_is_set(obj)) return taida_set_to_display_string_full(obj);
    if (taida_is_buchi_pack(obj)) {
        return taida_pack_to_display_string_full(obj);
    }
    // C23B-003 reopen 2: Lists must recurse through the full-form
    // debug-string so nested HashMap / Set / Pack items keep their
    // interpreter-parity shape (e.g. `Str[@[hashMap()...]]()` matches
    // the interpreter's `@[@(__entries <= …, __type <= "HashMap"), …]`).
    // `taida_list_to_display_string` alone would use the short-form
    // debug helper and collapse nested HashMaps to `HashMap({…})`.
    if (taida_is_list(obj)) {
        taida_val *list = (taida_val*)obj;
        taida_val list_len = list[2];
        size_t cap = 64;
        size_t len = 0;
        char *buf = (char*)TAIDA_MALLOC(cap, "list_display_full");
        buf[0] = '\0';
        { const char *s = "@["; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0'; }
        for (taida_val i = 0; i < list_len; i++) {
            if (i > 0) {
                const char *s = ", "; size_t sl = 2; while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, s, sl); len += sl; buf[len] = '\0';
            }
            taida_val item = list[4 + i];
            taida_val item_str = taida_value_to_debug_string_full(item);
            const char *is = (const char*)item_str;
            if (is) {
                size_t sl = strlen(is); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, is, sl); len += sl; buf[len] = '\0';
            }
            taida_str_release(item_str);
        }
        while (len + 2 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); }
        buf[len++] = ']'; buf[len] = '\0';
        taida_val result = (taida_val)taida_str_new_copy(buf);
        free(buf);
        return result;
    }
    return taida_value_to_display_string(obj);
}

// typeof(value, tag) — returns type name as a string.
// tag is a compile-time hint: 0=Int, 1=Float, 2=Bool, 3=Str, 4=Pack, 5=List, 6=Closure.
// For heap objects the tag is ignored and runtime detection is used.
taida_val taida_typeof(taida_val val, taida_val tag) {
    // For non-zero heap pointers, detect at runtime via magic headers
    if (val != 0 && val >= 4096) {
        if (taida_is_hashmap(val)) return (taida_val)taida_str_new_copy("HashMap");
        if (taida_is_set(val)) return (taida_val)taida_str_new_copy("Set");
        if (taida_is_async(val)) return (taida_val)taida_str_new_copy("Async");
        if (taida_is_list(val)) return (taida_val)taida_str_new_copy("List");
        if (taida_is_bytes(val)) return (taida_val)taida_str_new_copy("Bytes");
        if (taida_is_buchi_pack(val)) {
            int fc = taida_monadic_field_count(val);
            if (fc == 3) return (taida_val)taida_str_new_copy("Result");
            if (fc == 4) {
                int gtype = taida_detect_gorillax_type(val);
                if (gtype == 1) return (taida_val)taida_str_new_copy("Gorillax");
                if (gtype == 2) return (taida_val)taida_str_new_copy("RelaxedGorillax");
                return (taida_val)taida_str_new_copy("Lax");
            }
            return (taida_val)taida_str_new_copy("BuchiPack");
        }
        // Check if it's a string pointer
        const char *s = (const char*)val;
        size_t sl = 0;
        if (taida_read_cstr_len_safe(s, 65536, &sl)) {
            return (taida_val)taida_str_new_copy("Str");
        }
    }
    // For scalars, use the compile-time tag
    switch (tag) {
        case 1: return (taida_val)taida_str_new_copy("Float");
        case 2: return (taida_val)taida_str_new_copy("Bool");
        case 3: return (taida_val)taida_str_new_copy("Str");
        case 4: return (taida_val)taida_str_new_copy("BuchiPack");
        case 5: return (taida_val)taida_str_new_copy("List");
        case 6: return (taida_val)taida_str_new_copy("Closure");
        default: return (taida_val)taida_str_new_copy("Int");
    }
}

// Polymorphic .map(fn) — works on List, Result, Lax, Async
taida_val taida_polymorphic_map(taida_val obj, taida_val fn_ptr) {
    if (obj == 0 || obj < 4096) return obj;
    // Check Async first (before monadic, since Async has different layout)
    if (taida_is_async(obj)) return taida_async_map(obj, fn_ptr);
    int fc = taida_monadic_field_count(obj);
    if (fc == 3) return taida_result_map(obj, fn_ptr);
    if (fc == 4) return taida_lax_map(obj, fn_ptr);
    // Default: treat as list
    return taida_list_map(obj, fn_ptr);
}

// Polymorphic .flatMap(fn) — works on Result, Lax
taida_val taida_monadic_flat_map(taida_val obj, taida_val fn_ptr) {
    if (obj == 0 || obj < 4096) return obj;
    int fc = taida_monadic_field_count(obj);
    if (fc == 3) return taida_result_flat_map(obj, fn_ptr);
    if (fc == 4) return taida_lax_flat_map(obj, fn_ptr);
    return obj;  // fallback
}

// Polymorphic .getOrThrow() — works on Result
taida_val taida_monadic_get_or_throw(taida_val obj) {
    if (obj == 0 || obj < 4096) return obj;
    int fc = taida_monadic_field_count(obj);
    if (fc == 3) return taida_result_get_or_throw(obj);
    // Lax doesn't have getOrThrow — fall back to unmold
    if (fc == 4) return taida_lax_unmold(obj);
    return obj;
}

// Polymorphic .toString() — works on Result, Lax
taida_val taida_monadic_to_string(taida_val obj) {
    if (obj == 0 || obj < 4096) {
        char tmp[32];
        snprintf(tmp, 32, "%" PRId64 "", obj);
        return (taida_val)taida_str_new_copy(tmp);
    }
    int fc = taida_monadic_field_count(obj);
    if (fc == 3) return taida_result_to_string(obj);
    if (fc == 4) {
        int gtype = taida_detect_gorillax_type(obj);
        if (gtype == 1) return taida_gorillax_to_string(obj);
        if (gtype == 2) return taida_relaxed_gorillax_to_string(obj);
        return taida_lax_to_string(obj);
    }
    // Fallback: treat as int
    char tmp[32];
    snprintf(tmp, 32, "%" PRId64 "", obj);
    return (taida_val)taida_str_new_copy(tmp);
}

// ── Async methods ────────────────────────────────────────
taida_val taida_async_map(taida_val async_ptr, taida_val fn_ptr) {
    taida_val *obj = (taida_val*)async_ptr;
    // Join thread if pending
    if (obj[1] == 0) taida_async_join(async_ptr);
    if (obj[1] != 1) return async_ptr; // not fulfilled, return as-is
    taida_val new_val = taida_invoke_callback1(fn_ptr, obj[2]);
    // NO-3: detect type of mapped value and create tagged async
    taida_val vtag = taida_detect_value_tag(new_val);
    return taida_async_ok_tagged(new_val, vtag);
}

taida_val taida_async_get_or_default(taida_val async_ptr, taida_val def) {
    taida_val *obj = (taida_val*)async_ptr;
    // Join thread if pending
    if (obj[1] == 0) taida_async_join(async_ptr);
    if (obj[1] == 1) return obj[2]; // fulfilled
    return def;
}

// ── Async runtime ─────────────────────────────────────────
// NO-4 RULE 1: Async producers MUST use taida_async_ok_tagged (not taida_async_ok)
// to set value_tag. Legacy taida_async_ok uses UNKNOWN tag (conservative leak).
// NO-3: Async layout: [ASYNC_MAGIC, status, value, error, thread_handle, value_tag, error_tag]
//   status: 0=pending, 1=fulfilled, 2=rejected
//   thread_handle: 0 = no thread, otherwise pthread_t cast to taida_val
//   value_tag: type tag for value (TAIDA_TAG_* constant, -1 = UNKNOWN)
//   error_tag: type tag for error (usually TAIDA_TAG_PACK from taida_make_error)

taida_val taida_async_ok_tagged(taida_val value, taida_val value_tag) {
    // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
    taida_val *obj = (taida_val*)TAIDA_MALLOC(7 * sizeof(taida_val), "async_ok_tagged");
    obj[0] = TAIDA_ASYNC_MAGIC | 1;  // magic + refcount
    obj[1] = 1;  // fulfilled
    obj[2] = value;
    obj[3] = 0;  // no error
    obj[4] = 0;  // no thread
    obj[5] = value_tag;
    obj[6] = TAIDA_TAG_UNKNOWN;  // no error
    // NO-3: move semantics — caller transfers ownership of value to Async.
    // Async release will call taida_list_elem_release on value.
    // If the value is shared, the caller must retain before calling this.
    return (taida_val)obj;
}

taida_val taida_async_ok(taida_val value) {
    // Legacy wrapper: uses UNKNOWN tag (conservative — no retain/release for children)
    // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
    taida_val *obj = (taida_val*)TAIDA_MALLOC(7 * sizeof(taida_val), "async_ok");
    obj[0] = TAIDA_ASYNC_MAGIC | 1;  // magic + refcount
    obj[1] = 1;  // fulfilled
    obj[2] = value;
    obj[3] = 0;  // no error
    obj[4] = 0;  // no thread
    obj[5] = TAIDA_TAG_UNKNOWN;  // value_tag unknown
    obj[6] = TAIDA_TAG_UNKNOWN;  // no error
    return (taida_val)obj;
}

taida_val taida_async_err(taida_val error) {
    // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
    taida_val *obj = (taida_val*)TAIDA_MALLOC(7 * sizeof(taida_val), "async_err");
    obj[0] = TAIDA_ASYNC_MAGIC | 1;  // magic + refcount
    obj[1] = 2;  // rejected
    obj[2] = 0;  // no value
    obj[3] = error;
    obj[4] = 0;  // no thread
    obj[5] = TAIDA_TAG_UNKNOWN;  // no value
    obj[6] = TAIDA_TAG_PACK;    // error is always a Pack (from taida_make_error)
    // NO-3: move semantics — caller transfers ownership of error to Async.
    return (taida_val)obj;
}

// NO-3: Set value_tag on an existing Async object (for lowering to call after taida_async_ok)
void taida_async_set_value_tag(taida_val async_ptr, taida_val tag) {
    ((taida_val*)async_ptr)[5] = tag;
}

// Join a pending Async's thread (if any). After this call, status is no longer Pending.
static void taida_async_join(taida_val async_ptr) {
    taida_val *obj = (taida_val*)async_ptr;
    if (obj[1] != 0) return;              // not pending — nothing to join
    taida_val th = obj[4];
    if (th != 0) {
        pthread_join((pthread_t)th, NULL);
        obj[4] = 0;                       // clear thread handle
        // Thread entry already set status + value
    }
}

taida_val taida_async_unmold(taida_val async_ptr) {
    if (async_ptr == 0) return 0;
    taida_val *obj = (taida_val*)async_ptr;
    // If pending with a thread, join it first
    if (obj[1] == 0) {
        taida_async_join(async_ptr);
    }
    taida_val status = obj[1];
    if (status == 1) return obj[2];       // fulfilled → value
    if (status == 2) {                    // rejected → throw (catchable by |==)
        taida_val error = obj[3];
        if (taida_can_throw_payload(error)) {
            return taida_throw(error);
        }
        taida_val err = taida_make_error("AsyncError", "Async rejected");
        return taida_throw(err);
    }
    return 0;                              // pending (no thread) → Unit
}

// ── Async spawn (pthread-based) ──────────────────────────────

// Spawn a function in a background pthread. Returns Async[pending] with thread_handle.
taida_val taida_async_spawn(taida_val fn_ptr, taida_val arg) {
    taida_thread_arg *ta = (taida_thread_arg*)TAIDA_MALLOC(sizeof(taida_thread_arg), "async_spawn_arg");
    taida_val *obj = (taida_val*)TAIDA_MALLOC(7 * sizeof(taida_val), "async_spawn");
    obj[0] = TAIDA_ASYNC_MAGIC | 1; // Magic + initial refcount
    obj[1] = 0;   // status: pending
    obj[2] = 0;   // no value yet
    obj[3] = 0;   // no error
    obj[4] = 0;   // thread handle (set below)
    obj[5] = TAIDA_TAG_UNKNOWN;  // value_tag (set when resolved)
    obj[6] = TAIDA_TAG_UNKNOWN;  // error_tag (set when rejected)

    ta->fn_ptr = fn_ptr;
    ta->arg = arg;
    ta->async_obj = obj;

    pthread_t thread;
    pthread_create(&thread, NULL, taida_thread_entry, ta);
    obj[4] = (taida_val)thread;

    return (taida_val)obj;
}

taida_val taida_async_cancel(taida_val async_ptr) {
    if (async_ptr == 0) {
        taida_val err = taida_make_error("CancelledError", "Async operation cancelled");
        return taida_async_err(err);
    }
    if (!TAIDA_IS_ASYNC(async_ptr)) {
        // NO-3: detect value type for ownership tracking
        taida_val vtag = taida_detect_value_tag(async_ptr);
        return taida_async_ok_tagged(async_ptr, vtag);
    }

    taida_val *obj = (taida_val*)async_ptr;
    if (obj[1] != 0) {
        // Fulfilled/rejected async values are already resolved.
        return async_ptr;
    }

    taida_val th = obj[4];
    if (th != 0) {
        // Best-effort cancellation for pending pthread tasks.
        pthread_cancel((pthread_t)th);
        pthread_detach((pthread_t)th);
    }
    taida_val err = taida_make_error("CancelledError", "Async operation cancelled");
    return taida_async_err(err);
}

// ── Async aggregation ────────────────────────────────────────

// All[asyncList]() — join all pending threads, collect all fulfilled values.
// If any element is rejected, throw the error.
taida_val taida_async_all(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    // First pass: join all pending threads
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (TAIDA_IS_ASYNC(item)) {
            taida_async_join(item);
        }
    }
    // Second pass: collect values, retaining each element and tracking elem_type_tag.
    taida_val result_list = taida_list_new();
    taida_val unified_tag = TAIDA_TAG_UNKNOWN;
    int tag_set = 0;
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        taida_val val;
        taida_val vtag;
        if (TAIDA_IS_ASYNC(item)) {
            taida_val *obj = (taida_val*)item;
            taida_val status = obj[1];
            if (status == 2) {
                taida_val error = obj[3];
                // Release partially built result_list before throwing
                taida_release(result_list);
                if (taida_can_throw_payload(error)) {
                    return taida_throw(error);
                }
                taida_val err = taida_make_error("AsyncError", "All: async rejected");
                return taida_throw(err);
            }
            val = obj[2];
            vtag = obj[5];  // value_tag from source Async
        } else {
            val = item;
            vtag = taida_detect_value_tag(item);
        }
        // QF-58: retain element before pushing (source Async still owns it)
        taida_list_elem_retain(val, vtag);
        result_list = taida_list_push(result_list, val);
        // Track unified elem_type_tag
        if (!tag_set) {
            unified_tag = vtag;
            tag_set = 1;
        } else if (unified_tag != vtag) {
            unified_tag = TAIDA_TAG_UNKNOWN;  // heterogeneous → UNKNOWN
        }
    }
    // QF-58: set elem_type_tag on result list
    taida_list_set_elem_tag(result_list, unified_tag);
    // NO-3: result is always a List
    return taida_async_ok_tagged(result_list, TAIDA_TAG_LIST);
}

// Race[asyncList]() — join all pending threads, return the first fulfilled value.
taida_val taida_async_race(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    if (len == 0) {
        // Matches Interpreter behavior: Race[@[]] -> Async(@())
        return taida_async_ok_tagged(taida_pack_new(0), TAIDA_TAG_PACK);
    }
    // Join all pending threads (simple approach: join all, pick first)
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (TAIDA_IS_ASYNC(item)) {
            taida_async_join(item);
        }
    }
    taida_val first = list[4];
    if (TAIDA_IS_ASYNC(first)) {
        taida_val *obj = (taida_val*)first;
        taida_val status = obj[1];
        if (status == 2) {
            taida_val error = obj[3];
            if (taida_can_throw_payload(error)) {
                return taida_throw(error);
            }
            taida_val err = taida_make_error("AsyncError", "Race: async rejected");
            return taida_throw(err);
        }
        // NO-3: propagate value_tag from the source Async.
        // Retain because source Async still owns obj[2] and will release on drop.
        taida_list_elem_retain(obj[2], obj[5]);
        return taida_async_ok_tagged(obj[2], obj[5]);
    }
    // NO-3: non-async element — detect its type.
    // The element is borrowed from the input list; retain for new Async ownership.
    taida_val ftag = taida_detect_value_tag(first);
    taida_list_elem_retain(first, ftag);
    return taida_async_ok_tagged(first, ftag);
}

// Generic unmold: detect whether this is a Result, Lax, or Async at runtime
// Optional abolished in v0.8.0 — use Lax[T] instead.
// Result:   BuchiPack fc=4, hash0=HASH_RES___VALUE → evaluate predicate, check throw, return __value or throw
// Lax:      BuchiPack fc=4, hash0=HASH_HAS_VALUE → lax_unmold
// Async:    [ASYNC_MAGIC, status, value, error, thread_handle, value_tag, error_tag]
taida_val taida_generic_unmold(taida_val ptr) {
    if (ptr == 0) return 0;

    if (taida_is_molten(ptr)) {
        taida_val error = taida_make_error(
            "TypeError",
            "Cannot unmold Molten directly. Molten can only be used inside Cage."
        );
        return taida_throw(error);
    }
    
    // Check for BuchiPack (monadic types) using magic
    if (TAIDA_IS_PACK(ptr)) {
        taida_val *obj = (taida_val*)ptr;
        taida_val field_count = obj[1];
        taida_val hash0 = obj[2];

        // Result (fc=4, hash0=HASH_RES___VALUE): evaluate predicate + check throw
        if (field_count == 4 && hash0 == (taida_val)HASH_RES___VALUE) {
        taida_val value = taida_pack_get_idx(ptr, 0);       // __value
        taida_val pred = taida_pack_get_idx(ptr, 1);         // __predicate
        taida_val throw_val = taida_pack_get_idx(ptr, 2);    // throw

        // If throw is set explicitly, check predicate first
        if (throw_val != 0) {
            if (pred != 0) {
                taida_val pred_result = taida_invoke_callback1(pred, value);
                if (!pred_result) {
                    // Predicate failed — throw the error
                    if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
                    taida_val error = taida_make_error("ResultError", "Result predicate failed");
                    return taida_throw(error);
                }
                // Predicate passed even with throw set — return value
                return value;
            }
            // No predicate, throw is set — throw
            if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
            taida_val error = taida_make_error("ResultError", "Result error");
            return taida_throw(error);
        }

        // Evaluate predicate if present (no throw set)
        if (pred != 0) {
            taida_val pred_result = taida_invoke_callback1(pred, value);
            if (pred_result) return value;  // success
            // Predicate failed — throw default error
            taida_val error = taida_make_error("ResultError", "Result predicate failed");
            return taida_throw(error);
        }

        // No predicate, no throw — success
        return value;
    }

    // Lax/Gorillax/RelaxedGorillax (fc=4, hash0=HASH_HAS_VALUE)
    if (field_count == 4 && hash0 == (taida_val)HASH_HAS_VALUE) {
        int gtype = taida_detect_gorillax_type(ptr);
        if (gtype == 1) return taida_gorillax_unmold(ptr);
        if (gtype == 2) return taida_relaxed_gorillax_unmold(ptr);
        return taida_lax_unmold(ptr);
    }

    // TODO mold unmold — check __type tag and extract via unm/default/sol/value channels.
    // The `unm` channel is returned when present (priority: unm > __default > sol > __value).
    if (taida_pack_has_hash(ptr, (taida_val)HASH___TYPE)) {
        taida_val type_ptr = taida_pack_get(ptr, (taida_val)HASH___TYPE);
        int is_todo = 0;
        if (type_ptr == (taida_val)__todo_type_str) {
            is_todo = 1;
        } else if (type_ptr > 4096) {
            const char *type_str = (const char*)type_ptr;
            size_t len = 0;
            if (taida_read_cstr_len_safe(type_str, 32, &len) &&
                len == 4 && memcmp(type_str, "TODO", 4) == 0) {
                is_todo = 1;
            }
        }
        if (is_todo) {
            if (taida_pack_has_hash(ptr, (taida_val)HASH_TODO_UNM)) {
                return taida_pack_get(ptr, (taida_val)HASH_TODO_UNM);
            }
            if (taida_pack_has_hash(ptr, (taida_val)HASH___DEFAULT)) {
                return taida_pack_get(ptr, (taida_val)HASH___DEFAULT);
            }
            if (taida_pack_has_hash(ptr, (taida_val)HASH_TODO_SOL)) {
                return taida_pack_get(ptr, (taida_val)HASH_TODO_SOL);
            }
            if (taida_pack_has_hash(ptr, (taida_val)HASH___VALUE)) {
                return taida_pack_get(ptr, (taida_val)HASH___VALUE);
            }
            return taida_pack_new(0);
        }
    }

    // Custom mold default unmold:
    // pack with first field __type and a __value field.
    if (hash0 == (taida_val)HASH___TYPE &&
        taida_pack_has_hash(ptr, (taida_val)HASH___VALUE)) {
        return taida_pack_get(ptr, (taida_val)HASH___VALUE);
    }
    }

    // Check if this is an Async: [ASYNC_MAGIC, status, value, error, thread_handle, value_tag, error_tag]
    if (TAIDA_IS_ASYNC(ptr)) {
        return taida_async_unmold(ptr);
    }
    // Not a monadic type or Async — return as-is (e.g., list, string, plain value)
    return ptr;
}

taida_val taida_async_is_pending(taida_val async_ptr) {
    return ((taida_val*)async_ptr)[1] == 0 ? 1 : 0;
}

taida_val taida_async_is_fulfilled(taida_val async_ptr) {
    return ((taida_val*)async_ptr)[1] == 1 ? 1 : 0;
}

taida_val taida_async_is_rejected(taida_val async_ptr) {
    return ((taida_val*)async_ptr)[1] == 2 ? 1 : 0;
}

taida_val taida_async_get_value(taida_val async_ptr) {
    return ((taida_val*)async_ptr)[2];
}

taida_val taida_async_get_error(taida_val async_ptr) {
    return ((taida_val*)async_ptr)[3];
}

// Async toString — format like interpreter: Async[fulfilled: value] / Async[rejected: error] / Async[pending]
static taida_val taida_async_to_string(taida_val async_ptr) {
    taida_val *obj = (taida_val*)async_ptr;
    taida_val status = obj[1];
    char tmp[256];
    if (status == 1) {
        taida_val value = obj[2];
        taida_val val_str = taida_value_to_display_string(value);
        snprintf(tmp, sizeof(tmp), "Async[fulfilled: %s]", (const char*)val_str);
        taida_str_release(val_str);
    } else if (status == 2) {
        taida_val error = obj[3];
        taida_val err_str = taida_value_to_display_string(error);
        snprintf(tmp, sizeof(tmp), "Async[rejected: %s]", (const char*)err_str);
        taida_str_release(err_str);
    } else {
        memcpy(tmp, "Async[pending]", 15); /* 14 chars + '\0' */
    }
    return (taida_val)taida_str_new_copy(tmp);
}

// ── Debug for list ────────────────────────────────────────
taida_val taida_debug_list(taida_val list_ptr) {
    taida_val *list = (taida_val*)list_ptr;
    taida_val len = list[2];
    printf("@[");
    for (taida_val i = 0; i < len; i++) {
        if (i > 0) printf(", ");
        printf("%" PRId64 "", list[4 + i]);
    }
    printf("]\n");
    return 0;
}

// ── JSON Molten Iron runtime ──────────────────────────────
// JSON is an opaque primitive. To use JSON data, it must be cast through
// a schema using JSON[raw, Schema](). The schema is resolved at compile
// time and passed as a descriptor string.
//
// Schema descriptor format:
//   "i" = Int (default 0)
//   "f" = Float (default 0.0)
//   "s" = Str (default "")
//   "b" = Bool (default false)
//   "T{TypeName|field1:desc,field2:desc,...}" = TypeDef (BuchiPack)
//   "L{desc}" = List of elements
//
// The runtime parses JSON, interprets the schema descriptor, and constructs
// a Lax[BuchiPack] with proper FNV-1a hashes.

// --- Minimal JSON parser (recursive descent) ---

// JSON value types
#define JSON_NULL    0
#define JSON_BOOL    1
#define JSON_INT     2
#define JSON_FLOAT   3
#define JSON_STRING  4
#define JSON_ARRAY   5
#define JSON_OBJECT  6

typedef struct {
    int type;
    taida_val int_val;
    double float_val;
    char *str_val;        // for strings (heap-allocated, NUL-terminated
                          // for convenience but may contain embedded NULs)
    int str_len;          // C18B-006 fix: length of `str_val` in bytes,
                          // not counting the terminating NUL. Needed so
                          // enum validation can compare strings with
                          // embedded NULs against their variant names
                          // (see the JSON_STRING branch in
                          // `json_apply_schema` near the E{...} handler).
    struct json_array *arr;  // for arrays
    struct json_obj *obj;    // for objects
} json_val;

typedef struct json_array {
    json_val *items;
    int count;
    int cap;
} json_array;

typedef struct json_obj_entry {
    char *key;
    json_val value;
} json_obj_entry;

typedef struct json_obj {
    json_obj_entry *entries;
    int count;
    int cap;
} json_obj;

// Forward declarations
static json_val json_parse_value(const char **p);
static void json_skip_ws(const char **p);
static json_val json_default_for_desc(const char *desc);
static taida_val json_apply_schema(json_val *jval, const char **desc);

// FNV-1a hash (matches Rust side)
static uint64_t fnv1a(const char *s, int len) {
    uint64_t hash = 0xcbf29ce484222325ULL;
    for (int i = 0; i < len; i++) {
        hash ^= (unsigned char)s[i];
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

static void json_skip_ws(const char **p) {
    while (**p == ' ' || **p == '\t' || **p == '\n' || **p == '\r') (*p)++;
}

// C18B-006 fix: return both the decoded bytes AND the decoded length
// through `*out_len`, so embedded-NUL JSON strings compare correctly
// against Enum variant names and other length-aware consumers. When
// `out_len` is NULL the caller doesn't need the length (e.g. object
// keys which must remain C-strings).
static char *json_parse_string_raw_len(const char **p, int *out_len) {
    if (**p != '"') {
        if (out_len) *out_len = 0;
        return NULL;
    }
    (*p)++;  // skip opening quote
    // Find end of string (handle escape sequences)
    const char *start = *p;
    int len = 0;
    const char *scan = *p;
    while (*scan && *scan != '"') {
        if (*scan == '\\') { scan++; if (*scan) scan++; }
        else scan++;
        len++;
    }
    (void)start;
    // Allocate and copy with escape handling
    char *buf = (char*)TAIDA_MALLOC(len + 1, "json_parse_str");
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
                default: buf[out++] = **p; break;
            }
            (*p)++;
        } else {
            buf[out++] = **p;
            (*p)++;
        }
    }
    buf[out] = '\0';
    if (**p == '"') (*p)++;  // skip closing quote
    if (out_len) *out_len = out;
    return buf;
}

// Back-compat wrapper for callers that only need the C string (e.g.
// object keys). Preserves the pre-C18B-006 signature.
static char *json_parse_string_raw(const char **p) {
    return json_parse_string_raw_len(p, NULL);
}

static json_val json_parse_string(const char **p) {
    json_val v;
    v.type = JSON_STRING;
    int slen = 0;
    v.str_val = json_parse_string_raw_len(p, &slen);
    // C18B-006: `str_len` is the decoded byte count, which may be
    // shorter than `strlen(str_val)` if the JSON input contained an
    // escaped NUL (`\u0000`). Length-aware consumers (enum variant
    // match in `json_apply_schema`) must use this field to avoid
    // silently truncating at the first embedded NUL.
    v.str_len = slen;
    v.int_val = 0;
    v.float_val = 0.0;
    v.arr = NULL; v.obj = NULL;
    return v;
}

static json_val json_parse_number(const char **p) {
    json_val v;
    v.str_val = NULL; v.str_len = 0; v.arr = NULL; v.obj = NULL;
    char *end;
    double d = strtod(*p, &end);
    // Check if it's an integer (no decimal point or exponent)
    int is_int = 1;
    const char *scan = *p;
    if (*scan == '-') scan++;
    while (scan < end) {
        if (*scan == '.' || *scan == 'e' || *scan == 'E') { is_int = 0; break; }
        scan++;
    }
    *p = end;
    if (is_int && d >= -9007199254740992.0 && d <= 9007199254740992.0) {
        v.type = JSON_INT;
        v.int_val = (taida_val)d;
        v.float_val = d;
    } else {
        v.type = JSON_FLOAT;
        v.float_val = d;
        v.int_val = (taida_val)d;
    }
    return v;
}

static json_val json_parse_array(const char **p) {
    json_val v;
    v.type = JSON_ARRAY;
    v.str_val = NULL; v.str_len = 0; v.obj = NULL;
    // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
    v.arr = (json_array*)TAIDA_MALLOC(sizeof(json_array), "json_array");
    v.arr->count = 0;
    v.arr->cap = 4;
    v.arr->items = (json_val*)TAIDA_MALLOC(4 * sizeof(json_val), "json_array_items");
    (*p)++;  // skip '['
    json_skip_ws(p);
    if (**p == ']') { (*p)++; return v; }
    while (**p) {
        json_val item = json_parse_value(p);
        if (v.arr->count >= v.arr->cap) {
            v.arr->cap *= 2;
            json_val *_tmp = (json_val*)realloc(v.arr->items, v.arr->cap * sizeof(json_val));
            if (!_tmp) { fprintf(stderr, "taida: out of memory (json_array)\n"); exit(1); }
            v.arr->items = _tmp;
        }
        v.arr->items[v.arr->count++] = item;
        json_skip_ws(p);
        if (**p == ',') { (*p)++; json_skip_ws(p); }
        else break;
    }
    if (**p == ']') (*p)++;
    return v;
}

static json_val json_parse_object(const char **p) {
    json_val v;
    v.type = JSON_OBJECT;
    v.str_val = NULL; v.str_len = 0; v.arr = NULL;
    // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
    v.obj = (json_obj*)TAIDA_MALLOC(sizeof(json_obj), "json_obj");
    v.obj->count = 0;
    v.obj->cap = 8;
    v.obj->entries = (json_obj_entry*)TAIDA_MALLOC(8 * sizeof(json_obj_entry), "json_obj_entries");
    (*p)++;  // skip '{'
    json_skip_ws(p);
    if (**p == '}') { (*p)++; return v; }
    while (**p) {
        json_skip_ws(p);
        char *key = json_parse_string_raw(p);
        json_skip_ws(p);
        if (**p == ':') (*p)++;
        json_skip_ws(p);
        json_val val = json_parse_value(p);
        if (v.obj->count >= v.obj->cap) {
            v.obj->cap *= 2;
            json_obj_entry *_tmp = (json_obj_entry*)realloc(v.obj->entries, v.obj->cap * sizeof(json_obj_entry));
            if (!_tmp) { fprintf(stderr, "taida: out of memory (json_object)\n"); exit(1); }
            v.obj->entries = _tmp;
        }
        v.obj->entries[v.obj->count].key = key;
        v.obj->entries[v.obj->count].value = val;
        v.obj->count++;
        json_skip_ws(p);
        if (**p == ',') { (*p)++; json_skip_ws(p); }
        else break;
    }
    if (**p == '}') (*p)++;
    return v;
}

static json_val json_parse_value(const char **p) {
    json_skip_ws(p);
    json_val v;
    v.str_val = NULL; v.str_len = 0; v.arr = NULL; v.obj = NULL;
    if (**p == '"') return json_parse_string(p);
    if (**p == '{') return json_parse_object(p);
    if (**p == '[') return json_parse_array(p);
    if (**p == 't' && strncmp(*p, "true", 4) == 0) {
        *p += 4; v.type = JSON_BOOL; v.int_val = 1; return v;
    }
    if (**p == 'f' && strncmp(*p, "false", 5) == 0) {
        *p += 5; v.type = JSON_BOOL; v.int_val = 0; return v;
    }
    if (**p == 'n' && strncmp(*p, "null", 4) == 0) {
        *p += 4; v.type = JSON_NULL; v.int_val = 0; return v;
    }
    if (**p == '-' || (**p >= '0' && **p <= '9')) return json_parse_number(p);
    // Parse error: return null
    v.type = JSON_NULL; v.int_val = 0;
    return v;
}

// --- JSON object field lookup ---
static json_val *json_obj_get(json_obj *obj, const char *key) {
    if (!obj) return NULL;
    for (int i = 0; i < obj->count; i++) {
        if (strcmp(obj->entries[i].key, key) == 0) {
            return &obj->entries[i].value;
        }
    }
    return NULL;
}

// --- Schema descriptor parsing ---

// Parse a field name from schema descriptor. Returns length consumed.
// Reads until ':' or ',' or '}' or end of string.
static int schema_read_name(const char *desc, char *buf, int buf_size) {
    int i = 0;
    while (desc[i] && desc[i] != ':' && desc[i] != ',' && desc[i] != '}' && desc[i] != '|' && i < buf_size - 1) {
        buf[i] = desc[i];
        i++;
    }
    buf[i] = '\0';
    return i;
}

// Find matching closing brace, accounting for nesting
static int schema_find_closing_brace(const char *desc) {
    int depth = 1;
    int i = 0;
    while (desc[i] && depth > 0) {
        if (desc[i] == '{') depth++;
        if (desc[i] == '}') depth--;
        if (depth > 0) i++;
    }
    return i;
}

// --- Default values from schema ---
//
// C16B-001: The descriptor walker for "pure" default values.
//
// This mirrors `src/interpreter/json.rs::default_for_schema` and must NOT be
// confused with the Lax-producing `json_apply_schema(NULL, ...)` path. Interior
// Enum fields of a TypeDef default MUST become `Int(0)` (first variant ordinal),
// NOT `Lax[Enum]` — the Lax wrapping is only for actual schema validation
// failures (mismatch / missing key / null field), not for "this is the baseline
// default of an uninhabited schema".
//
// Advances `*desc` past the consumed schema token so nested descriptors are
// parsed once with correct length accounting.
static taida_val json_pure_default_apply(const char **desc) {
    if (!desc || !*desc || !**desc) return 0;
    const char *d = *desc;

    switch (d[0]) {
        case 'i': {
            *desc = d + 1;
            return 0;
        }
        case 'f': {
            *desc = d + 1;
            return _d2l(0.0);
        }
        case 's': {
            *desc = d + 1;
            char *empty = (char*)TAIDA_MALLOC(1, "json_default_str");
            empty[0] = '\0';
            return (taida_val)empty;
        }
        case 'b': {
            *desc = d + 1;
            return 0;
        }
        case 'T': {
            // T{TypeName|field1:desc,field2:desc,...} — default BuchiPack with
            // each field set to the pure default of its sub-schema. Crucially
            // Enum fields resolve to Int(0), not Lax[Enum].
            if (d[1] != '{') {
                *desc = d + 1;
                return 0;
            }
            d += 2;  // skip "T{"
            // Read type name (until '|' or '}').
            char type_name[256];
            int tn_len = 0;
            while (*d && *d != '|' && *d != '}' && tn_len < 255) {
                type_name[tn_len++] = *d;
                d++;
            }
            type_name[tn_len] = '\0';
            if (*d == '|') d++;

            // Count fields (mirrors the counting logic in json_apply_schema
            // case 'T': 0 if body starts with '}', else 1 + (top-level ',' count)).
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

            // +1 for __type field.
            taida_val pack = taida_pack_new(field_count + 1);
            int idx = 0;
            while (*d && *d != '}') {
                // Read field name (until ':' or ',' or '}').
                char fname[256];
                int fn_len = 0;
                while (*d && *d != ':' && *d != '}' && fn_len < 255) {
                    fname[fn_len++] = *d;
                    d++;
                }
                fname[fn_len] = '\0';
                if (*d == ':') d++;

                uint64_t hash = fnv1a(fname, fn_len);
                taida_pack_set_hash(pack, idx, (taida_val)hash);

                // Recurse into sub-schema using the pure-default walker.
                taida_val field_val = json_pure_default_apply(&d);
                taida_pack_set(pack, idx, field_val);
                idx++;

                if (*d == ',') d++;
            }
            if (*d == '}') d++;

            // Add __type field.
            uint64_t type_hash = fnv1a("__type", 6);
            taida_pack_set_hash(pack, idx, (taida_val)type_hash);
            char *type_str = (char*)TAIDA_MALLOC((size_t)tn_len + 1, "json_type_str");
            memcpy(type_str, type_name, (size_t)tn_len + 1);
            taida_pack_set(pack, idx, (taida_val)type_str);

            *desc = d;
            return pack;
        }
        case 'L': {
            // L{desc} — default for a list schema is the empty list regardless
            // of inner schema. Skip past the nested descriptor to keep the
            // outer pointer well-formed for any caller that continues parsing.
            if (d[1] != '{') {
                *desc = d + 1;
                return taida_list_new();
            }
            d += 2;  // skip "L{"
            int inner_len = schema_find_closing_brace(d);
            d += inner_len;
            if (*d == '}') d++;
            *desc = d;
            return taida_list_new();
        }
        case 'E': {
            // C16: Enum default is the first variant's ordinal (= Int(0)).
            // Matches `docs/guide/01_types.md:609` — 最初のバリアントがデフォルト —
            // and the Interpreter's `default_for_schema(JsonSchema::Enum)`.
            if (d[1] != '{') {
                *desc = d + 1;
                return 0;
            }
            d += 2;  // skip "E{"
            int inner_len = schema_find_closing_brace(d);
            d += inner_len;
            if (*d == '}') d++;
            *desc = d;
            return 0;
        }
        default: {
            *desc = d + 1;
            return 0;
        }
    }
}

static taida_val json_default_value_for_desc(const char *desc) {
    if (!desc || !*desc) return 0;
    const char *cur = desc;
    return json_pure_default_apply(&cur);
}

// --- Convert JSON value to typed value using schema ---
// Returns a taida_val (int, float-as-bitcast, string pointer, or BuchiPack pointer)

static taida_val json_to_int(json_val *jv) {
    if (!jv) return 0;
    switch (jv->type) {
        case JSON_INT: return jv->int_val;
        case JSON_FLOAT: return (taida_val)jv->float_val;
        case JSON_BOOL: return jv->int_val;
        case JSON_STRING: {
            if (!jv->str_val) return 0;
            char *end;
            taida_val r = strtol(jv->str_val, &end, 10);
            if (*end != '\0') return 0;
            return r;
        }
        default: return 0;
    }
}

static taida_val json_to_float(json_val *jv) {
    if (!jv) return _d2l(0.0);
    switch (jv->type) {
        case JSON_FLOAT: return _d2l(jv->float_val);
        case JSON_INT: return _d2l((double)jv->int_val);
        case JSON_BOOL: return _d2l(jv->int_val ? 1.0 : 0.0);
        case JSON_STRING: {
            if (!jv->str_val) return _d2l(0.0);
            char *end;
            double r = strtod(jv->str_val, &end);
            if (*end != '\0') return _d2l(0.0);
            return _d2l(r);
        }
        default: return _d2l(0.0);
    }
}

static taida_val json_to_str(json_val *jv) {
    if (!jv) { return (taida_val)taida_str_alloc(0); }
    switch (jv->type) {
        case JSON_STRING: {
            if (!jv->str_val) { return (taida_val)taida_str_alloc(0); }
            return (taida_val)taida_str_new_copy(jv->str_val);
        }
        case JSON_INT: {
            char buf[32]; snprintf(buf, sizeof(buf), "%" PRId64 "", jv->int_val);
            return (taida_val)taida_str_new_copy(buf);
        }
        case JSON_FLOAT: {
            char buf[64]; snprintf(buf, sizeof(buf), "%g", jv->float_val);
            return (taida_val)taida_str_new_copy(buf);
        }
        case JSON_BOOL: {
            return (taida_val)taida_str_new_copy(jv->int_val ? "true" : "false");
        }
        case JSON_NULL: {
            return (taida_val)taida_str_alloc(0);
        }
        default: {
            char *e = (char*)TAIDA_MALLOC(1, "json_default_empty"); e[0]='\0'; return (taida_val)e;
        }
    }
}

static taida_val json_to_bool(json_val *jv) {
    if (!jv) return 0;
    switch (jv->type) {
        case JSON_BOOL: return jv->int_val;
        case JSON_INT: return jv->int_val != 0 ? 1 : 0;
        case JSON_FLOAT: return jv->float_val != 0.0 ? 1 : 0;
        case JSON_STRING: return (jv->str_val && jv->str_val[0]) ? 1 : 0;
        case JSON_NULL: return 0;
        default: return 0;
    }
}

// Apply a schema descriptor to a JSON value, constructing the appropriate native value.
// Returns: taida_val (the native value — int, float-bitcast, string ptr, BuchiPack ptr, or list ptr)
// The desc pointer is advanced past the consumed descriptor.
static taida_val json_apply_schema(json_val *jval, const char **desc) {
    if (!desc || !*desc || !**desc) return 0;
    const char *d = *desc;

    switch (d[0]) {
        case 'i': {
            *desc = d + 1;
            if (!jval || jval->type == JSON_NULL) return 0;
            return json_to_int(jval);
        }
        case 'f': {
            *desc = d + 1;
            if (!jval || jval->type == JSON_NULL) return _d2l(0.0);
            return json_to_float(jval);
        }
        case 's': {
            *desc = d + 1;
            if (!jval || jval->type == JSON_NULL) {
                char *e = (char*)TAIDA_MALLOC(1, "json_null_str"); e[0]='\0'; return (taida_val)e;
            }
            return json_to_str(jval);
        }
        case 'b': {
            *desc = d + 1;
            if (!jval || jval->type == JSON_NULL) return 0;
            return json_to_bool(jval);
        }
        case 'T': {
            // T{TypeName|field1:desc,field2:desc,...}
            // Parse type name
            if (d[1] != '{') { *desc = d + 1; return 0; }
            d += 2;  // skip "T{"
            // Read type name (until '|')
            char type_name[256];
            int tn_len = 0;
            while (*d && *d != '|' && tn_len < 255) { type_name[tn_len++] = *d; d++; }
            type_name[tn_len] = '\0';
            if (*d == '|') d++;

            // Count fields first
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

            // +1 for __type field
            taida_val pack = taida_pack_new(field_count + 1);

            // Parse each field and apply schema
            int idx = 0;
            while (*d && *d != '}') {
                // Read field name
                char fname[256];
                int fn_len = 0;
                while (*d && *d != ':' && *d != '}' && fn_len < 255) { fname[fn_len++] = *d; d++; }
                fname[fn_len] = '\0';
                if (*d == ':') d++;

                // Compute FNV-1a hash for field name
                uint64_t hash = fnv1a(fname, fn_len);
                taida_pack_set_hash(pack, idx, (taida_val)hash);

                // Look up this field in JSON object
                json_val *field_jval = NULL;
                if (jval && jval->type == JSON_OBJECT) {
                    field_jval = json_obj_get(jval->obj, fname);
                }

                // Apply sub-schema to field value
                taida_val field_val = json_apply_schema(field_jval, &d);
                taida_pack_set(pack, idx, field_val);
                idx++;

                if (*d == ',') d++;
            }
            if (*d == '}') d++;

            // Add __type field
            uint64_t type_hash = fnv1a("__type", 6);
            taida_pack_set_hash(pack, idx, (taida_val)type_hash);
            char *type_str = (char*)TAIDA_MALLOC(tn_len + 1, "json_type_str");
            memcpy(type_str, type_name, tn_len + 1);
            taida_pack_set(pack, idx, (taida_val)type_str);

            *desc = d;
            return pack;
        }
        case 'L': {
            // L{desc}
            if (d[1] != '{') { *desc = d + 1; return taida_list_new(); }
            d += 2;  // skip "L{"
            // Find closing brace
            int inner_len = schema_find_closing_brace(d);
            // Make a copy of the inner descriptor for repeated use
            char *inner_desc = (char*)TAIDA_MALLOC(inner_len + 1, "json_inner_desc");
            memcpy(inner_desc, d, inner_len);
            inner_desc[inner_len] = '\0';

            taida_val list = taida_list_new();

            if (jval && jval->type == JSON_ARRAY && jval->arr) {
                for (int i = 0; i < jval->arr->count; i++) {
                    const char *elem_desc = inner_desc;
                    taida_val elem = json_apply_schema(&jval->arr->items[i], &elem_desc);
                    list = taida_list_push(list, elem);
                }
            }
            // else: non-array or null -> empty list

            free(inner_desc);
            d += inner_len;
            if (*d == '}') d++;
            *desc = d;
            return list;
        }
        case 'E': {
            // C16: E{EnumName|Variant1,Variant2,...}
            // JSON String matching a variant -> Int(ordinal).
            // Anything else -> Lax[Enum] empty (hasValue=false, __value=0, __default=0).
            if (d[1] != '{') { *desc = d + 1; return taida_lax_empty(0); }
            d += 2;  // skip "E{"

            // Skip enum name (until '|').
            while (*d && *d != '|' && *d != '}') d++;
            if (*d == '|') d++;

            // If JSON value is not a string, walk past the variants and return Lax.
            int is_string_match_candidate = (jval && jval->type == JSON_STRING && jval->str_val);
            const char *js = is_string_match_candidate ? jval->str_val : NULL;
            // C18B-006 fix: use the parsed string's decoded byte length
            // rather than `strlen(js)` so embedded-NUL JSON inputs
            // (e.g. `"\u0000"` expanded by the parser) compare
            // correctly against variant names. Variant names cannot
            // themselves contain NUL (the descriptor format uses NUL
            // as a terminator upstream), so any JSON string whose
            // decoded length includes a NUL is a guaranteed mismatch
            // instead of a false positive truncation at the NUL.
            int js_len = is_string_match_candidate ? jval->str_len : 0;

            int matched = 0;
            taida_val ordinal = 0;
            int64_t current_ordinal = 0;

            // Parse variants one by one.
            while (*d && *d != '}') {
                // Read variant name until ',' or '}'.
                const char *vstart = d;
                while (*d && *d != ',' && *d != '}') d++;
                int vlen = (int)(d - vstart);

                if (!matched && js) {
                    // Compare: js[0..js_len] == vstart[0..vlen] exactly.
                    if (js_len == vlen && memcmp(js, vstart, (size_t)vlen) == 0) {
                        ordinal = (taida_val)current_ordinal;
                        matched = 1;
                    }
                }

                current_ordinal++;
                if (*d == ',') d++;
            }
            if (*d == '}') d++;
            *desc = d;

            if (matched) {
                return ordinal;
            }
            // Mismatch / non-string / missing -> Lax[Enum] with hasValue=false.
            // __value = __default = 0 (first variant ordinal) per C16 design.
            return taida_lax_empty(0);
        }
        default: {
            *desc = d + 1;
            return 0;
        }
    }
}

// Main entry point: JSON[raw, Schema]() -> Lax[T]
// raw_ptr: C string (the raw JSON)
// schema_ptr: C string (the schema descriptor)
// Returns: Lax BuchiPack (hasValue=true if parse succeeds, false on error)
taida_val taida_json_schema_cast(taida_val raw_ptr, taida_val schema_ptr) {
    const char *raw = (const char *)raw_ptr;
    const char *schema = (const char *)schema_ptr;

    if (!raw || !schema) {
        taida_val def = json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    // Parse JSON
    const char *p = raw;
    json_skip_ws(&p);
    if (!*p) {
        // Empty string -> parse error
        taida_val def = json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    const char *before_parse = p;
    json_val jval = json_parse_value(&p);

    // Detect parse error: if parser didn't advance, or the input wasn't
    // valid JSON (non-null value that didn't consume input)
    if (p == before_parse) {
        // Parser didn't consume anything -> parse error
        taida_val def = json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    // Check if there's trailing non-whitespace (malformed JSON)
    json_skip_ws(&p);
    if (*p != '\0') {
        // Trailing garbage -> parse error
        taida_val def = json_default_value_for_desc(schema);
        return taida_lax_empty(def);
    }

    // Apply schema
    const char *desc = schema;
    taida_val result = json_apply_schema(&jval, &desc);

    // Compute default for same schema
    taida_val def = json_default_value_for_desc(schema);

    return taida_lax_new(result, def);
}

// Legacy JSON functions (kept for backward compat with older tests)
taida_val taida_json_parse(taida_val str_ptr) {
    const char *src = (const char*)str_ptr;
    if (!src) src = "{}";
    size_t len = strlen(src);
    char *buf = (char*)TAIDA_MALLOC(len + 1, "json_parse");
    memcpy(buf, src, len + 1);
    return (taida_val)buf;
}

taida_val taida_json_empty(void) {
    char *buf = (char*)TAIDA_MALLOC(3, "json_empty");
    buf[0] = '{'; buf[1] = '}'; buf[2] = '\0';
    return (taida_val)buf;
}

taida_val taida_json_from_int(taida_val value) {
    char buf[32];
    snprintf(buf, sizeof(buf), "%" PRId64 "", value);
    size_t len = strlen(buf);
    char *result = (char*)TAIDA_MALLOC(len + 1, "json_from_int");
    memcpy(result, buf, len + 1);
    return (taida_val)result;
}

taida_val taida_json_from_str(taida_val str_ptr) {
    const char *src = (const char*)str_ptr;
    if (!src) src = "";
    size_t src_len = strlen(src);
    size_t new_len = src_len + 2;
    char *buf = (char*)TAIDA_MALLOC(new_len + 1, "json_from_str");
    buf[0] = '"';
    memcpy(buf + 1, src, src_len);
    buf[new_len - 1] = '"';
    buf[new_len] = '\0';
    return (taida_val)buf;
}

taida_val taida_json_unmold(taida_val json_ptr) {
    const char *src = (const char*)json_ptr;
    if (!src) { char *e = (char*)TAIDA_MALLOC(1, "json_unmold_empty"); e[0]='\0'; return (taida_val)e; }
    size_t len = strlen(src);
    char *buf = (char*)TAIDA_MALLOC(len + 1, "json_unmold");
    memcpy(buf, src, len + 1);
    return (taida_val)buf;
}

taida_val taida_json_stringify(taida_val json_ptr) {
    return taida_json_unmold(json_ptr);
}

taida_val taida_json_to_str(taida_val json_ptr) {
    return taida_json_unmold(json_ptr);
}

taida_val taida_json_to_int(taida_val json_ptr) {
    const char *data = (const char*)json_ptr;
    if (!data) return 0;
    return atol(data);
}

taida_val taida_json_size(taida_val json_ptr) {
    const char *data = (const char*)json_ptr;
    if (!data) return 0;
    return (taida_val)strlen(data);
}

taida_val taida_json_has(taida_val json_ptr, taida_val key_ptr) {
    const char *json_data = (const char*)json_ptr;
    const char *key_data = (const char*)key_ptr;
    if (!json_data || !key_data) return 0;
    return strstr(json_data, key_data) != NULL ? 1 : 0;
}

taida_val taida_debug_json(taida_val json_ptr) {
    const char *data = (const char*)json_ptr;
    if (data) printf("JSON(%s)\n", data);
    else printf("JSON(null)\n");
    return 0;
}

// ── stdlib math (native) ──────────────────────────────────
// Values may be integer (small values stored directly) or float (f64 bits in taida_val).
// We use a heuristic: the Taida lowering emits ConstFloat for known float literals
// and ConstInt for integer literals. Integer values in math context should be
// converted to double before computation.
//
// Convention: Math functions receive "tagged" longs. If the value was originally
// an integer (from ConstInt), the lowering inserts a taida_int_to_float call.
// For now, we use a bit-pattern heuristic as a fallback.

static double _l2d(taida_val v) { union { taida_val l; double d; } u; u.l = v; return u.d; }
static taida_val _d2l(double v) { union { taida_val l; double d; } u; u.d = v; return u.l; }

// Smart conversion: if the bit pattern represents a "reasonable" f64, use it as-is.
// If it looks like a small integer (-1M..1M), convert from integer.
// This heuristic handles both ConstFloat (bitcast) and ConstInt paths.
static double _to_double(taida_val v) {
    // If v is a small integer (common case for literals like 16, 100, etc.)
    // f64 encoding of small integers has specific bit patterns
    // Quick check: if |v| < 2^20 (about 1M), it's likely a plain integer
    if (v >= -1048576 && v <= 1048576) {
        return (double)v;
    }
    // Otherwise treat as f64 bit pattern
    return _l2d(v);
}

// taida_math_* functions removed (std dissolution)

// Float arithmetic (values stored as f64 bits in taida_val)
taida_val taida_float_add(taida_val a, taida_val b) { return _d2l(_to_double(a) + _to_double(b)); }
taida_val taida_float_sub(taida_val a, taida_val b) { return _d2l(_to_double(a) - _to_double(b)); }
taida_val taida_float_mul(taida_val a, taida_val b) { return _d2l(_to_double(a) * _to_double(b)); }
// taida_float_div removed — use Div[x, y]() mold instead

// ── Field Name Registry (for jsonEncode) ──────────────────
// Global hash -> name table for BuchiPack field name lookup.
// Populated by taida_register_field_name() calls emitted at compile time.

#define FIELD_REGISTRY_CAP 256
// type_tag: 0=unknown, 1=Int, 2=Float, 3=Str, 4=Bool, 5=Enum (C18-2)
// When type_tag == 5, `enum_desc` points to "VariantA,VariantB,..." so the
// encoder can emit the variant name Str for a given ordinal.
static struct {
    taida_val hash;
    const char *name;
    int type_tag;
    const char *enum_desc;
} __field_registry[FIELD_REGISTRY_CAP];
static int __field_registry_len = 0;

taida_val taida_register_field_name(taida_val hash, taida_val name_ptr) {
    // Check for duplicate
    for (int i = 0; i < __field_registry_len; i++) {
        if (__field_registry[i].hash == hash) return 0;
    }
    if (__field_registry_len < FIELD_REGISTRY_CAP) {
        __field_registry[__field_registry_len].hash = hash;
        __field_registry[__field_registry_len].name = (const char*)name_ptr;
        __field_registry[__field_registry_len].type_tag = 0;
        __field_registry[__field_registry_len].enum_desc = NULL;
        __field_registry_len++;
    }
    return 0;
}

// Extended version: register field with type tag
taida_val taida_register_field_type(taida_val hash, taida_val name_ptr, taida_val type_tag) {
    for (int i = 0; i < __field_registry_len; i++) {
        if (__field_registry[i].hash == hash) {
            __field_registry[i].type_tag = (int)type_tag;
            return 0;
        }
    }
    if (__field_registry_len < FIELD_REGISTRY_CAP) {
        __field_registry[__field_registry_len].hash = hash;
        __field_registry[__field_registry_len].name = (const char*)name_ptr;
        __field_registry[__field_registry_len].type_tag = (int)type_tag;
        __field_registry[__field_registry_len].enum_desc = NULL;
        __field_registry_len++;
    }
    return 0;
}

// C18-2: register a field as an Enum-typed field with variant descriptor.
// `variants_ptr` points to a comma-separated list of variant names
// (e.g. "Creating,Running,Stopped") emitted by the native lowering.
taida_val taida_register_field_enum(taida_val hash, taida_val name_ptr, taida_val variants_ptr) {
    for (int i = 0; i < __field_registry_len; i++) {
        if (__field_registry[i].hash == hash) {
            __field_registry[i].type_tag = 5;
            __field_registry[i].enum_desc = (const char*)variants_ptr;
            return 0;
        }
    }
    if (__field_registry_len < FIELD_REGISTRY_CAP) {
        __field_registry[__field_registry_len].hash = hash;
        __field_registry[__field_registry_len].name = (const char*)name_ptr;
        __field_registry[__field_registry_len].type_tag = 5;
        __field_registry[__field_registry_len].enum_desc = (const char*)variants_ptr;
        __field_registry_len++;
    }
    return 0;
}

static const char* taida_lookup_field_name(taida_val hash) {
    for (int i = 0; i < __field_registry_len; i++) {
        if (__field_registry[i].hash == hash) return __field_registry[i].name;
    }
    return NULL;
}

static const char* taida_lookup_field_enum_desc(taida_val hash) {
    for (int i = 0; i < __field_registry_len; i++) {
        if (__field_registry[i].hash == hash) return __field_registry[i].enum_desc;
    }
    return NULL;
}

// C18B-003 fix: per-pack-instance enum descriptor registry.
// Keyed by (pack_ptr, field_hash) so two packs that share a field name
// but carry different enums do not overwrite each other's descriptor.
//
// The table is bounded at `PACK_FIELD_ENUM_CAP`. When full we silently
// drop further registrations; `json_serialize_pack_fields` then falls
// back to the global `__field_registry.enum_desc`. The only observable
// consequence of table exhaustion is the pre-C18B-003 behaviour, so
// overflow cannot regress correctness below the already-released
// surface.
//
// `pack_ptr` is stored as `taida_val` (i64) for simple bitwise compare.
// The table is populated by `taida_register_pack_field_enum()` which
// is emitted once per `PackSet` of an Enum-typed field. Both fresh
// allocation and an existing-entry update path are provided so a pack
// that is reused across multiple PackSet calls (rare but possible via
// `taida_pack_set_val`) remains in sync with its last-written enum.
#define PACK_FIELD_ENUM_CAP 1024
static struct {
    taida_val pack_ptr;
    taida_val field_hash;
    const char *enum_desc;
} __pack_field_enum_registry[PACK_FIELD_ENUM_CAP];
static int __pack_field_enum_registry_len = 0;

taida_val taida_register_pack_field_enum(taida_val pack_ptr, taida_val field_hash, taida_val variants_ptr) {
    if (pack_ptr == 0) return 0;
    // Update existing entry if present (most recent write wins for a
    // given (pack, field) pair — mirrors the single-writer contract of
    // `PackSet`).
    for (int i = 0; i < __pack_field_enum_registry_len; i++) {
        if (__pack_field_enum_registry[i].pack_ptr == pack_ptr
            && __pack_field_enum_registry[i].field_hash == field_hash) {
            __pack_field_enum_registry[i].enum_desc = (const char*)variants_ptr;
            return 0;
        }
    }
    if (__pack_field_enum_registry_len < PACK_FIELD_ENUM_CAP) {
        __pack_field_enum_registry[__pack_field_enum_registry_len].pack_ptr = pack_ptr;
        __pack_field_enum_registry[__pack_field_enum_registry_len].field_hash = field_hash;
        __pack_field_enum_registry[__pack_field_enum_registry_len].enum_desc = (const char*)variants_ptr;
        __pack_field_enum_registry_len++;
    }
    return 0;
}

static const char* taida_lookup_pack_field_enum_desc(taida_val pack_ptr, taida_val field_hash) {
    if (pack_ptr == 0) return NULL;
    // Linear scan — tables are small (per-program, per-pack-instance).
    // Reverse order so the most recent registration wins if the same
    // (pack, field) is registered twice without the early-update path
    // firing (e.g. two distinct packs that happen to share a pointer
    // after an allocator reuse, in which case the last writer is the
    // live pack).
    for (int i = __pack_field_enum_registry_len - 1; i >= 0; i--) {
        if (__pack_field_enum_registry[i].pack_ptr == pack_ptr
            && __pack_field_enum_registry[i].field_hash == field_hash) {
            return __pack_field_enum_registry[i].enum_desc;
        }
    }
    return NULL;
}

static int taida_lookup_field_type(taida_val hash) {
    for (int i = 0; i < __field_registry_len; i++) {
        if (__field_registry[i].hash == hash) return __field_registry[i].type_tag;
    }
    return 0; // unknown
}

// ── jsonEncode / jsonPretty (native) ──────────────────────
// Recursive serialization of Taida values to JSON string.
// Uses runtime type detection (same heuristics as polymorphic dispatch).

static void json_append(char **buf, size_t *cap, size_t *len, const char *s) {
    size_t slen = strlen(s);
    while (*len + slen + 1 > *cap) {
        *cap *= 2;
        TAIDA_REALLOC(*buf, *cap, "json_stringify");
    }
    memcpy(*buf + *len, s, slen);
    *len += slen;
    (*buf)[*len] = '\0';
}

static void json_append_char(char **buf, size_t *cap, size_t *len, char c) {
    if (*len + 2 > *cap) {
        *cap *= 2;
        TAIDA_REALLOC(*buf, *cap, "json_stringify");
    }
    (*buf)[*len] = c;
    *len += 1;
    (*buf)[*len] = '\0';
}

// Escape a string for JSON output
static void json_append_escaped_str(char **buf, size_t *cap, size_t *len, const char *s) {
    json_append_char(buf, cap, len, '"');
    if (s) {
        for (const char *p = s; *p; p++) {
            switch (*p) {
                case '"':  json_append(buf, cap, len, "\\\""); break;
                case '\\': json_append(buf, cap, len, "\\\\"); break;
                case '\n': json_append(buf, cap, len, "\\n"); break;
                case '\r': json_append(buf, cap, len, "\\r"); break;
                case '\t': json_append(buf, cap, len, "\\t"); break;
                default:   json_append_char(buf, cap, len, *p); break;
            }
        }
    }
    json_append_char(buf, cap, len, '"');
}

// Forward declare: recursive serialization
// type_hint: 0=unknown, 1=Int, 2=Float, 3=Str, 4=Bool
static void json_serialize_typed(char **buf, size_t *cap, size_t *len, taida_val val, int indent, int depth, int type_hint);

// Append indentation (for pretty mode, indent > 0)
static void json_append_indent(char **buf, size_t *cap, size_t *len, int indent, int depth) {
    if (indent <= 0) return;
    json_append_char(buf, cap, len, '\n');
    for (int i = 0; i < indent * depth; i++) {
        json_append_char(buf, cap, len, ' ');
    }
}

// C18-2: Emit a variant-name Str for an Enum-typed field. `ordinal` is
// the Int(ordinal) stored in the BuchiPack field; `variants_csv` is the
// comma-separated variant list registered via `taida_register_field_enum`.
// Falls back to emitting the raw ordinal number when the descriptor is
// missing or the ordinal is out of range.
static void json_append_enum_variant(char **buf, size_t *cap, size_t *len, taida_val ordinal, const char *variants_csv) {
    if (!variants_csv) {
        char num[32];
        snprintf(num, sizeof(num), "%" PRId64 "", ordinal);
        json_append(buf, cap, len, num);
        return;
    }
    int64_t idx = 0;
    const char *start = variants_csv;
    const char *p = variants_csv;
    while (*p) {
        if (*p == ',') {
            if (idx == ordinal) {
                int vlen = (int)(p - start);
                char buf_name[128];
                int copy_len = vlen < (int)sizeof(buf_name) - 1 ? vlen : (int)sizeof(buf_name) - 1;
                memcpy(buf_name, start, (size_t)copy_len);
                buf_name[copy_len] = '\0';
                json_append_escaped_str(buf, cap, len, buf_name);
                return;
            }
            idx++;
            start = p + 1;
        }
        p++;
    }
    // Last variant (no trailing comma)
    if (idx == ordinal) {
        int vlen = (int)(p - start);
        char buf_name[128];
        int copy_len = vlen < (int)sizeof(buf_name) - 1 ? vlen : (int)sizeof(buf_name) - 1;
        memcpy(buf_name, start, (size_t)copy_len);
        buf_name[copy_len] = '\0';
        json_append_escaped_str(buf, cap, len, buf_name);
        return;
    }
    // Out of range — fall back to ordinal Int.
    char num[32];
    snprintf(num, sizeof(num), "%" PRId64 "", ordinal);
    json_append(buf, cap, len, num);
}

// Helper: serialize a BuchiPack's fields as JSON object
// Fields are sorted alphabetically (matching interpreter/JS behavior).
// All __ fields are skipped for user packs, BUT monadic packs (Lax /
// Gorillax / RelaxedGorillax / Result) expose `__value` / `__default` /
// `__error` / `__predicate` to match the interpreter's `jsonEncode`.
// `__type` is always skipped (internal metadata).
//
// C25B-028: detect monadic packs via `taida_monadic_field_count` on the
// pack pointer; when monadic, allow `__*` fields through (except
// `__type`). Non-monadic user BuchiPacks continue to skip all `__*` to
// preserve the pre-existing "hide internal state" contract.
static void json_serialize_pack_fields(char **buf, size_t *cap, size_t *len, taida_val *pack, taida_val fc, int indent, int depth) {
    int monadic_kind = taida_monadic_field_count((taida_val)(intptr_t)pack);
    int is_monadic = (monadic_kind == 3 || monadic_kind == 4);
    // C25B-028: for Lax (`monadic_kind == 4` with `__default` in slot 2)
    // the `__default` field carries the same T as `__value`; if `__value`
    // is tag-Int or heuristically an Int, force Int formatting on
    // `__default` too. This is the only reliable way to render
    // `Lax[N : Int]()` with `__default: 0` rather than `__default: {}`.
    // 0 = no override, 1 = Int, 3 = Str. Bool/Float omitted for now
    // (`Lax[true]()` / `Lax[1.0]()` produce true/0.0 which already round-
    // trip through the slot-tag path; no 0-default ambiguity).
    int lax_default_force_type = 0;
    if (is_monadic && monadic_kind == 4) {
        // Lax / Gorillax / RelaxedGorillax share the `hasValue` layout.
        // Distinguish Lax (has `__default`) vs Gorillax (has `__error`)
        // by checking slot 2's hash.
        if (fc >= 3 && pack[2 + 2 * 3] == (taida_val)HASH___DEFAULT) {
            taida_val value_slot_tag = pack[2 + 1 * 3 + 1];
            taida_val value_slot_val = pack[2 + 1 * 3 + 2];
            if (value_slot_tag == TAIDA_TAG_STR) {
                lax_default_force_type = 3;
            } else if (value_slot_tag == TAIDA_TAG_BOOL) {
                lax_default_force_type = 1; // Bool renders as 0/1 numerically here.
            } else if (value_slot_tag == TAIDA_TAG_INT) {
                // TAIDA_TAG_INT (0) is also the "untagged" sentinel.
                // Use the value shape to disambiguate:
                //   - int-shaped (small positive / any negative / zero) → Int
                //   - pointer-shaped (> 4096) with readable C-string →  Str
                //     (covers string literals stamped with no STR magic)
                size_t __slen = 0;
                if (value_slot_val > 0 &&
                    value_slot_val > 4096 &&
                    taida_read_cstr_len_safe((const char*)value_slot_val, 65536, &__slen)) {
                    lax_default_force_type = 3;
                } else if (value_slot_val == 0 ||
                           value_slot_val < 0 ||
                           (value_slot_val > 0 && value_slot_val < 4096)) {
                    lax_default_force_type = 1;
                }
            }
            // Other tags (Float/Pack/List/...) leave the default path
            // alone — either the slot tag picks up the type below, or
            // the heuristic in json_serialize_typed handles a pointer.
        }
    }
    // Collect visible fields: (name, val, type_hint, enum_desc, index for stable sort)
    typedef struct { const char *name; taida_val val; int type_hint; const char *enum_desc; } JsonField;
    JsonField fields[100];
    int nfields = 0;
    for (taida_val i = 0; i < fc && nfields < 100; i++) {
        taida_val field_hash = pack[2 + i * 3];
        taida_val field_val = pack[2 + i * 3 + 2];
        const char *fname = taida_lookup_field_name(field_hash);
        if (!fname) continue;
        // Always skip __type (internal metadata, not user data).
        if (strcmp(fname, "__type") == 0) continue;
        // Non-monadic packs hide all other __ fields.
        // Monadic packs (Lax/Gorillax/RelaxedGorillax/Result) expose them
        // so that jsonEncode output matches the interpreter.
        if (!is_monadic && fname[0] == '_' && fname[1] == '_') {
            continue;
        }
        int ftype = taida_lookup_field_type(field_hash);
        // C25B-028: pull per-slot tag from the pack layout so the
        // monadic payload uses the correct formatter (Int/Float/Str vs
        // pointer heuristic). Slot tag of 0 means TAIDA_TAG_INT /
        // untagged — the caller-level force_int_default_for_lax
        // compensates for Lax `__default`/`__value` ambiguity.
        if (ftype == 0 && is_monadic) {
            taida_val slot_tag = pack[2 + i * 3 + 1];
            if (slot_tag == TAIDA_TAG_STR) ftype = 3;
            else if (slot_tag == TAIDA_TAG_BOOL) ftype = 4;
            // TAIDA_TAG_INT (0) / other: leave as 0 and fall back to
            // heuristic inside json_serialize_typed.
        }
        // Lax payload: force formatting on `__value` and `__default`
        // based on the sibling slot's tag so `Value::Int(0)` doesn't
        // render as `{}` and `Value::Str("")` doesn't render as `{}`.
        if (lax_default_force_type != 0 && ftype == 0 &&
            (strcmp(fname, "__default") == 0 || strcmp(fname, "__value") == 0)) {
            ftype = lax_default_force_type;
            // Str-forced value with val=0: replace with an empty-string
            // pointer so json_serialize_typed doesn't deref NULL.
            if (ftype == 3 && field_val == 0) {
                field_val = (taida_val)"";
            }
        }
        // C18B-003 fix: prefer per-pack descriptor (keyed by pack_ptr +
        // field_hash) over the global field registry so two enums that
        // share the field name (`state`, `status`, `kind`, …) emit
        // their correct variant name. Fall back to the global table
        // when no per-pack entry exists (covers code paths that were
        // emitted before this fix and any future callers that skip the
        // `taida_register_pack_field_enum` emission).
        const char *enum_desc = taida_lookup_pack_field_enum_desc((taida_val)(intptr_t)pack, field_hash);
        // If we resolved a per-pack descriptor the field is Enum-typed
        // even if the global type tag never got promoted to 5 (e.g.
        // packs built by `taida_pack_new` outside of _taida_main).
        if (enum_desc != NULL) {
            ftype = 5;
        } else if (ftype == 5) {
            enum_desc = taida_lookup_field_enum_desc(field_hash);
        }
        fields[nfields].name = fname;
        fields[nfields].val = field_val;
        fields[nfields].type_hint = ftype;
        fields[nfields].enum_desc = enum_desc;
        nfields++;
    }
    // Sort fields alphabetically by name (insertion sort — nfields is small)
    for (int i = 1; i < nfields; i++) {
        JsonField tmp = fields[i];
        int j = i - 1;
        while (j >= 0 && strcmp(fields[j].name, tmp.name) > 0) {
            fields[j + 1] = fields[j];
            j--;
        }
        fields[j + 1] = tmp;
    }
    // Serialize
    json_append_char(buf, cap, len, '{');
    for (int i = 0; i < nfields; i++) {
        if (i > 0) json_append_char(buf, cap, len, ',');
        if (indent > 0) json_append_indent(buf, cap, len, indent, depth + 1);
        json_append_escaped_str(buf, cap, len, fields[i].name);
        json_append_char(buf, cap, len, ':');
        if (indent > 0) json_append_char(buf, cap, len, ' ');
        // C18-2: Enum-typed field → emit variant name Str via descriptor.
        if (fields[i].type_hint == 5) {
            json_append_enum_variant(buf, cap, len, fields[i].val, fields[i].enum_desc);
        } else {
            json_serialize_typed(buf, cap, len, fields[i].val, indent, depth + 1, fields[i].type_hint);
        }
    }
    if (indent > 0 && nfields > 0) json_append_indent(buf, cap, len, indent, depth);
    json_append_char(buf, cap, len, '}');
}

static void json_serialize_typed(char **buf, size_t *cap, size_t *len, taida_val val, int indent, int depth, int type_hint) {
    // Bool type hint: serialize 0/1 as false/true
    if (type_hint == 4) {
        json_append(buf, cap, len, val ? "true" : "false");
        return;
    }

    // Integer/Float hints take precedence over the val==0 Unit fallback
    // so `Value::Int(0)` → `"0"` and not `"{}"`. Matches interpreter's
    // `taida_value_to_json` which emits Int(0) as JSON number 0.
    if (type_hint == 1 || type_hint == 2) { // Int or Float
        char num[32];
        snprintf(num, sizeof(num), "%" PRId64 "", val);
        json_append(buf, cap, len, num);
        return;
    }

    // Null/Unit (untyped val==0 falls through to empty object, matching
    // interpreter's `Value::Unit` → `{}`).
    if (val == 0) {
        if (type_hint == 3) { // Str
            json_append(buf, cap, len, "\"\"");
        } else {
            json_append(buf, cap, len, "{}");
        }
        return;
    }
    // String hint: always treat as string pointer
    if (type_hint == 3) {
        const char *s = (const char*)val;
        json_append_escaped_str(buf, cap, len, s);
        return;
    }

    // No type hint (type_hint == 0): heuristic-based detection
    // Small integer (not a heap pointer)
    if (val > 0 && val < 4096) {
        char num[32];
        snprintf(num, sizeof(num), "%" PRId64 "", val);
        json_append(buf, cap, len, num);
        return;
    }
    if (val < 0) {
        char num[32];
        snprintf(num, sizeof(num), "%" PRId64 "", val);
        json_append(buf, cap, len, num);
        return;
    }

    // Check for HashMap
    if (taida_is_hashmap(val)) {
        taida_val *hm = (taida_val*)val;
        taida_val hm_cap = hm[1];
        json_append_char(buf, cap, len, '{');
        taida_val count = 0;
        // C23B-008 (2026-04-22): walk the insertion-order side-index so
        // JSON output mirrors interpreter / JS ordering.
        taida_val next_ord = hm[TAIDA_HM_ORD_HEADER_SLOT(hm_cap)];
        for (taida_val oi = 0; oi < next_ord; oi++) {
            taida_val slot = hm[TAIDA_HM_ORD_SLOT(hm_cap, oi)];
            if (slot < 0 || slot >= hm_cap) continue;
            taida_val slot_hash = hm[HM_HEADER + slot * 3];
            taida_val slot_key = hm[HM_HEADER + slot * 3 + 1];
            if (HM_SLOT_OCCUPIED(slot_hash, slot_key)) {
                if (count > 0) json_append_char(buf, cap, len, ',');
                if (indent > 0) json_append_indent(buf, cap, len, indent, depth + 1);
                const char *key_str = (const char*)slot_key;
                if (!key_str) key_str = "";
                json_append_escaped_str(buf, cap, len, key_str);
                json_append_char(buf, cap, len, ':');
                if (indent > 0) json_append_char(buf, cap, len, ' ');
                json_serialize_typed(buf, cap, len, hm[HM_HEADER + slot * 3 + 2], indent, depth + 1, 0);
                count++;
            }
        }
        if (indent > 0 && count > 0) json_append_indent(buf, cap, len, indent, depth);
        json_append_char(buf, cap, len, '}');
        return;
    }

    // Check for Set
    if (taida_is_set(val)) {
        taida_val *list = (taida_val*)val;
        taida_val list_len = list[2];
        json_append_char(buf, cap, len, '[');
        for (taida_val i = 0; i < list_len; i++) {
            if (i > 0) json_append_char(buf, cap, len, ',');
            if (indent > 0) json_append_indent(buf, cap, len, indent, depth + 1);
            json_serialize_typed(buf, cap, len, list[4 + i], indent, depth + 1, 0);
        }
        if (indent > 0 && list_len > 0) json_append_indent(buf, cap, len, indent, depth);
        json_append_char(buf, cap, len, ']');
        return;
    }

    // Check for BuchiPack (monadic types: Result, Lax)
    int fc = taida_monadic_field_count(val);
    if (fc > 0) {
        taida_val *pack = (taida_val*)val;
        // Use actual field_count from pack, not the type ID from monadic_field_count
        taida_val real_fc = pack[1];
        json_serialize_pack_fields(buf, cap, len, pack, real_fc, indent, depth);
        return;
    }

    // Check for List (before general BuchiPack since list detection is more specific)
    if (taida_is_list(val)) {
        taida_val *list = (taida_val*)val;
        taida_val list_len = list[2];
        json_append_char(buf, cap, len, '[');
        for (taida_val i = 0; i < list_len; i++) {
            if (i > 0) json_append_char(buf, cap, len, ',');
            if (indent > 0) json_append_indent(buf, cap, len, indent, depth + 1);
            json_serialize_typed(buf, cap, len, list[4 + i], indent, depth + 1, 0);
        }
        if (indent > 0 && list_len > 0) json_append_indent(buf, cap, len, indent, depth);
        json_append_char(buf, cap, len, ']');
        return;
    }

    // Check for BuchiPack (any size, including user-defined types)
    if (taida_is_buchi_pack(val)) {
        taida_val *obj = (taida_val*)val;
        taida_val obj_fc = obj[1];
        json_serialize_pack_fields(buf, cap, len, obj, obj_fc, indent, depth);
        return;
    }

    // Default: only serialize as string when safely readable.
    size_t str_len = 0;
    if (taida_read_cstr_len_safe((const char*)val, 65536, &str_len)) {
        json_append_escaped_str(buf, cap, len, (const char*)val);
    } else {
        // Not a safe C-string pointer — treat as integer
        char num[32];
        snprintf(num, sizeof(num), "%" PRId64 "", val);
        json_append(buf, cap, len, num);
    }
}

taida_val taida_json_encode(taida_val val) {
    size_t cap = 256;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "json_encode");
    buf[0] = '\0';
    json_serialize_typed(&buf, &cap, &len, val, 0, 0, 0);
    return (taida_val)buf;
}

taida_val taida_json_pretty(taida_val val) {
    size_t cap = 256;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "json_pretty");
    buf[0] = '\0';
    json_serialize_typed(&buf, &cap, &len, val, 2, 0, 0);
    return (taida_val)buf;
}

// ── stdlib I/O (native) ───────────────────────────────────

taida_val taida_time_now_ms(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
        return (taida_val)time(NULL) * 1000L;
    }
    int64_t ms = (int64_t)ts.tv_sec * 1000LL + (int64_t)(ts.tv_nsec / 1000000L);
    if (ms > INT64_MAX) return INT64_MAX;
    if (ms < INT64_MIN) return INT64_MIN;
    return (taida_val)ms;
}

static taida_val taida_time_sleep_task(taida_val ms) {
    struct timespec req;
    req.tv_sec = (time_t)(ms / 1000);
    req.tv_nsec = (taida_val)((ms % 1000) * 1000000L);
    while (nanosleep(&req, &req) == -1 && errno == EINTR) {
    }
    return taida_pack_new(0);
}

taida_val taida_time_sleep(taida_val ms) {
    const taida_val max_sleep_ms = 2147483647L;
    if (ms < 0 || ms > max_sleep_ms) {
        char msg[160];
        snprintf(msg, sizeof(msg), "sleep: ms must be in range 0..=%" PRId64 ", got %" PRId64 "", max_sleep_ms, ms);
        return taida_async_err(taida_make_error("RangeError", msg));
    }
    return taida_async_spawn((taida_val)taida_time_sleep_task, ms);
}

// ── SHA-256 prelude function (builtin, no external dependency) ─────────
typedef struct {
    uint32_t state[8];
    uint64_t total_len;
    unsigned char block[64];
    size_t block_len;
} taida_sha256_ctx;

static const uint32_t TAIDA_SHA256_K[64] = {
    0x428a2f98U, 0x71374491U, 0xb5c0fbcfU, 0xe9b5dba5U,
    0x3956c25bU, 0x59f111f1U, 0x923f82a4U, 0xab1c5ed5U,
    0xd807aa98U, 0x12835b01U, 0x243185beU, 0x550c7dc3U,
    0x72be5d74U, 0x80deb1feU, 0x9bdc06a7U, 0xc19bf174U,
    0xe49b69c1U, 0xefbe4786U, 0x0fc19dc6U, 0x240ca1ccU,
    0x2de92c6fU, 0x4a7484aaU, 0x5cb0a9dcU, 0x76f988daU,
    0x983e5152U, 0xa831c66dU, 0xb00327c8U, 0xbf597fc7U,
    0xc6e00bf3U, 0xd5a79147U, 0x06ca6351U, 0x14292967U,
    0x27b70a85U, 0x2e1b2138U, 0x4d2c6dfcU, 0x53380d13U,
    0x650a7354U, 0x766a0abbU, 0x81c2c92eU, 0x92722c85U,
    0xa2bfe8a1U, 0xa81a664bU, 0xc24b8b70U, 0xc76c51a3U,
    0xd192e819U, 0xd6990624U, 0xf40e3585U, 0x106aa070U,
    0x19a4c116U, 0x1e376c08U, 0x2748774cU, 0x34b0bcb5U,
    0x391c0cb3U, 0x4ed8aa4aU, 0x5b9cca4fU, 0x682e6ff3U,
    0x748f82eeU, 0x78a5636fU, 0x84c87814U, 0x8cc70208U,
    0x90befffaU, 0xa4506cebU, 0xbef9a3f7U, 0xc67178f2U
};

static uint32_t taida_sha256_rotr(uint32_t x, uint32_t n) {
    return (x >> n) | (x << (32 - n));
}

static void taida_sha256_init(taida_sha256_ctx *ctx) {
    ctx->state[0] = 0x6a09e667U;
    ctx->state[1] = 0xbb67ae85U;
    ctx->state[2] = 0x3c6ef372U;
    ctx->state[3] = 0xa54ff53aU;
    ctx->state[4] = 0x510e527fU;
    ctx->state[5] = 0x9b05688cU;
    ctx->state[6] = 0x1f83d9abU;
    ctx->state[7] = 0x5be0cd19U;
    ctx->total_len = 0;
    ctx->block_len = 0;
}

static void taida_sha256_transform(taida_sha256_ctx *ctx, const unsigned char block[64]) {
    uint32_t w[64];
    for (int i = 0; i < 16; i++) {
        int j = i * 4;
        w[i] = ((uint32_t)block[j] << 24) |
               ((uint32_t)block[j + 1] << 16) |
               ((uint32_t)block[j + 2] << 8) |
               (uint32_t)block[j + 3];
    }
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = taida_sha256_rotr(w[i - 15], 7) ^ taida_sha256_rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint32_t s1 = taida_sha256_rotr(w[i - 2], 17) ^ taida_sha256_rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }

    uint32_t a = ctx->state[0];
    uint32_t b = ctx->state[1];
    uint32_t c = ctx->state[2];
    uint32_t d = ctx->state[3];
    uint32_t e = ctx->state[4];
    uint32_t f = ctx->state[5];
    uint32_t g = ctx->state[6];
    uint32_t h = ctx->state[7];

    for (int i = 0; i < 64; i++) {
        uint32_t s1 = taida_sha256_rotr(e, 6) ^ taida_sha256_rotr(e, 11) ^ taida_sha256_rotr(e, 25);
        uint32_t ch = (e & f) ^ ((~e) & g);
        uint32_t temp1 = h + s1 + ch + TAIDA_SHA256_K[i] + w[i];
        uint32_t s0 = taida_sha256_rotr(a, 2) ^ taida_sha256_rotr(a, 13) ^ taida_sha256_rotr(a, 22);
        uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
        uint32_t temp2 = s0 + maj;

        h = g;
        g = f;
        f = e;
        e = d + temp1;
        d = c;
        c = b;
        b = a;
        a = temp1 + temp2;
    }

    ctx->state[0] += a;
    ctx->state[1] += b;
    ctx->state[2] += c;
    ctx->state[3] += d;
    ctx->state[4] += e;
    ctx->state[5] += f;
    ctx->state[6] += g;
    ctx->state[7] += h;
}

static void taida_sha256_update(taida_sha256_ctx *ctx, const unsigned char *data, size_t len) {
    if (!data || len == 0) return;
    ctx->total_len += (uint64_t)len;
    size_t pos = 0;
    while (pos < len) {
        size_t need = 64 - ctx->block_len;
        size_t take = (len - pos < need) ? (len - pos) : need;
        memcpy(ctx->block + ctx->block_len, data + pos, take);
        ctx->block_len += take;
        pos += take;
        if (ctx->block_len == 64) {
            taida_sha256_transform(ctx, ctx->block);
            ctx->block_len = 0;
        }
    }
}

static void taida_sha256_final(taida_sha256_ctx *ctx, unsigned char out[32]) {
    uint64_t bit_len = ctx->total_len * 8ULL;

    ctx->block[ctx->block_len++] = 0x80;
    if (ctx->block_len > 56) {
        while (ctx->block_len < 64) ctx->block[ctx->block_len++] = 0;
        taida_sha256_transform(ctx, ctx->block);
        ctx->block_len = 0;
    }
    while (ctx->block_len < 56) ctx->block[ctx->block_len++] = 0;

    for (int i = 0; i < 8; i++) {
        ctx->block[56 + i] = (unsigned char)(bit_len >> (56 - i * 8));
    }
    taida_sha256_transform(ctx, ctx->block);

    for (int i = 0; i < 8; i++) {
        out[i * 4] = (unsigned char)(ctx->state[i] >> 24);
        out[i * 4 + 1] = (unsigned char)(ctx->state[i] >> 16);
        out[i * 4 + 2] = (unsigned char)(ctx->state[i] >> 8);
        out[i * 4 + 3] = (unsigned char)(ctx->state[i]);
    }
}

static taida_val taida_sha256_hex_from_bytes(const unsigned char *data, size_t len) {
    taida_sha256_ctx ctx;
    unsigned char digest[32];
    static const char hex[] = "0123456789abcdef";
    char *out = taida_str_alloc(64);
    taida_sha256_init(&ctx);
    taida_sha256_update(&ctx, data, len);
    taida_sha256_final(&ctx, digest);
    for (int i = 0; i < 32; i++) {
        out[i * 2] = hex[(digest[i] >> 4) & 0x0f];
        out[i * 2 + 1] = hex[digest[i] & 0x0f];
    }
    return (taida_val)out;
}

taida_val taida_sha256(taida_val value) {
    if (TAIDA_IS_BYTES(value)) {
        taida_val len = taida_bytes_len(value);
        if (len <= 0) return taida_sha256_hex_from_bytes(NULL, 0);
        // M-08: Cap Bytes length to 256MB to prevent OOM from huge positive len.
        if (len > (taida_val)(256 * 1024 * 1024)) {
            return taida_sha256_hex_from_bytes(NULL, 0);
        }
        taida_val *bytes = (taida_val*)value;
        unsigned char *raw = (unsigned char*)TAIDA_MALLOC((size_t)len, "sha256_bytes");
        for (taida_val i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];
        taida_val out = taida_sha256_hex_from_bytes(raw, (size_t)len);
        free(raw);
        return out;
    }

    taida_val display = taida_value_to_display_string(value);
    const char *s = (const char*)display;
    size_t slen = 0;
    if (!taida_read_cstr_len_safe(s, 1 << 20, &slen)) {
        taida_str_release(display);
        return taida_sha256_hex_from_bytes(NULL, 0);
    }
    taida_val out = taida_sha256_hex_from_bytes((const unsigned char*)s, slen);
    taida_str_release(display);
    return out;
}

// C20-3 (ROOT-8): previously a fixed 4096-byte stack buffer truncated
// long paste input; the unread tail would bleed into the next `stdin`
// call and break 3-backend parity (Interpreter / JS both use dynamic
// buffers). Switch to a dynamic growth strategy: `getline(3)` on POSIX
// and a realloc-loop around `fgets` on Windows.
taida_val taida_io_stdin(taida_val prompt_ptr) {
    const char *prompt = (const char*)prompt_ptr;
    if (prompt && prompt[0] != '\0') {
        printf("%s", prompt);
        fflush(stdout);
    }
#if defined(_WIN32)
    // Windows fallback: realloc loop around fgets. `getline` is not
    // part of the Windows CRT. We grow the buffer until `\n` is seen
    // or EOF / error is reached.
    size_t cap = 1024;
    char *buf = (char*)TAIDA_MALLOC(cap, "stdin_buf");
    size_t len = 0;
    for (;;) {
        if (fgets(buf + len, (int)(cap - len), stdin) == NULL) {
            if (len == 0) {
                free(buf);
                return (taida_val)taida_str_alloc(0);
            }
            break; // EOF with data already read
        }
        len += strlen(buf + len);
        if (len > 0 && buf[len - 1] == '\n') break;
        if (cap - len <= 1) {
            cap *= 2;
            TAIDA_REALLOC(buf, cap, "stdin_buf");
        }
    }
    size_t ulen = len;
#else
    char *buf = NULL;
    size_t cap = 0;
    ssize_t got = getline(&buf, &cap, stdin);
    if (got < 0) {
        // EOF or error. `getline` may have allocated buf even on
        // failure — free it to avoid leaks.
        free(buf);
        return (taida_val)taida_str_alloc(0);
    }
    size_t ulen = (size_t)got;
#endif
    // Strip trailing \n / \r\n.
    if (ulen > 0 && buf[ulen - 1] == '\n') {
        ulen--;
        if (ulen > 0 && buf[ulen - 1] == '\r') ulen--;
    }
    char *r = taida_str_alloc(ulen);
    memcpy(r, buf, ulen);
    free(buf);
    return (taida_val)r;
}

// C12-5 (FB-18): stdout / stderr return the UTF-8 byte length of the payload
// as Int so that `n <= stdout("hi")` binds `n = 2`. The trailing newline added
// for display is NOT counted — callers see the payload size they supplied.
// Parity: interpreter and JS runtime use the same semantics (content length
// via Rust `String::len()` / JS UTF-8 byte length, newline excluded).
taida_val taida_io_stdout(taida_val val_ptr) {
    // For now, treat val as a string pointer
    const char *s = (const char*)val_ptr;
    if (s) {
        printf("%s\n", s);
        return (taida_val)strlen(s);
    }
    return 0;
}

// B11-2a: Type-tagged stdout — resolves Bool display parity (FB-3).
// When the compiler knows the argument type at emit time, it passes
// a compile-time tag so that Bool prints "true"/"false" instead of "1"/"0".
// Only Bool needs special handling; all other types (Str, Int, Float,
// Pack, List, etc.) are correctly handled by taida_polymorphic_to_string.
// C12-5: returns bytes written (Int), see taida_io_stdout above.
taida_val taida_io_stdout_with_tag(taida_val val, taida_val tag) {
    const char *s = NULL;
    char bool_buf[6];
    size_t bytes = 0;
    if ((int)tag == TAIDA_TAG_BOOL) {
        s = val ? "true" : "false";
        bytes = strlen(s);
        memcpy(bool_buf, s, bytes);
        bool_buf[bytes] = '\0';
        printf("%s\n", bool_buf);
        return (taida_val)bytes;
    }
    // C21B-seed-07: BuchiPack values (including Lax / Result / Gorillax and
    // user-defined packs) must route through `taida_stdout_display_string`
    // which uses `_full` rendering — interpreter parity requires showing
    // ALL fields including `__value` / `__default` / `__type`. Without this,
    // the FLOAT fast-path below would kick in for `stdout(Float[x]())`
    // (because `mold_returns.rs` historically claimed `Float[]()` returns
    // FLOAT even though it actually returns Lax) and the pack pointer would
    // be decoded as an f64 bit-pattern, printing subnormal garbage.
    // Detecting the pack at runtime defences against any other caller that
    // hands a Lax / user pack with a mis-stated tag (e.g. UNKNOWN = -1).
    if (val >= 4096 && taida_is_buchi_pack(val)) {
        taida_val str = taida_stdout_display_string(val);
        s = (const char*)str;
        if (s) {
            printf("%s\n", s);
            return (taida_val)strlen(s);
        }
        return 0;
    }
    // C21-4 / seed-03 / seed-05: FLOAT tag path — the boxed i64 carries
    // the f64 bit pattern, so decode via memcpy and format with the
    // Rust-display-compatible renderer `taida_float_to_str`. Without
    // this, the value falls through to `taida_polymorphic_to_string`
    // which has no tag context and the runtime prints the raw i64
    // (symptom: `stdout(triple(4.0))` → `4622382067542392832`).
    if ((int)tag == TAIDA_TAG_FLOAT) {
        double d;
        memcpy(&d, &val, sizeof(double));
        taida_val fstr = taida_float_to_str(d);
        s = (const char*)fstr;
        if (s) {
            printf("%s\n", s);
            return (taida_val)strlen(s);
        }
        return 0;
    }
    taida_val str = taida_polymorphic_to_string(val);
    s = (const char*)str;
    if (s) {
        printf("%s\n", s);
        return (taida_val)strlen(s);
    }
    return 0;
}

// C12-5: bytes written as Int.
taida_val taida_io_stderr(taida_val val_ptr) {
    const char *s = (const char*)val_ptr;
    if (s) {
        fprintf(stderr, "%s\n", s);
        return (taida_val)strlen(s);
    }
    return 0;
}

// B11-2a: Type-tagged stderr — mirrors taida_io_stdout_with_tag for stderr.
// C12-5: returns bytes written (Int).
taida_val taida_io_stderr_with_tag(taida_val val, taida_val tag) {
    const char *s = NULL;
    if ((int)tag == TAIDA_TAG_BOOL) {
        s = val ? "true" : "false";
        fprintf(stderr, "%s\n", s);
        return (taida_val)strlen(s);
    }
    // C21B-seed-07: symmetric with stdout — route buchi packs through
    // `taida_stdout_display_string` for full-form interpreter parity.
    if (val >= 4096 && taida_is_buchi_pack(val)) {
        taida_val str = taida_stdout_display_string(val);
        s = (const char*)str;
        if (s) {
            fprintf(stderr, "%s\n", s);
            return (taida_val)strlen(s);
        }
        return 0;
    }
    // C21-4: FLOAT tag path — same rationale as taida_io_stdout_with_tag.
    if ((int)tag == TAIDA_TAG_FLOAT) {
        double d;
        memcpy(&d, &val, sizeof(double));
        taida_val fstr = taida_float_to_str(d);
        s = (const char*)fstr;
        if (s) {
            fprintf(stderr, "%s\n", s);
            return (taida_val)strlen(s);
        }
        return 0;
    }
    taida_val str = taida_polymorphic_to_string(val);
    s = (const char*)str;
    if (s) {
        fprintf(stderr, "%s\n", s);
        return (taida_val)strlen(s);
    }
    return 0;
}

// ─── C20-2: stdinLine — UTF-8-aware Async[Lax[Str]] line editor ─────────
//
// Derived from linenoise (https://github.com/antirez/linenoise,
// BSD-2-Clause). Only the minimum subset needed to fix ROOT-7 (cooked-
// mode kernel Backspace corrupting multibyte UTF-8) is included:
//
//   * termios raw-mode TTY input on POSIX (linenoise's `enableRawMode`).
//   * UTF-8 codepoint-aware Backspace (linenoise's `linenoiseEditBackspace`
//     simplified to strip whole codepoints from the buffer in one step).
//   * ^C → treated as cancelled read, returns Lax failure with default "".
//   * ^D on empty line → EOF, returns Lax failure.
//   * Pipe / non-TTY input → falls back to getline (Phase 3's stdin path).
//
// Explicitly NOT implemented: history, tab completion, multi-line edit,
// arrow-key cursor movement, window-size tracking. Callers that want a
// full readline experience should install the future
// `taida-lang/readline` addon. Taida's `stdinLine` is intentionally a
// minimal surface so the 3-backend parity is auditable.
//
// License: BSD-2-Clause, see `LICENSES/linenoise.LICENSE` at the repo
// root for the full text and attribution.
#include <termios.h>
#include <unistd.h>

// Helper: wrap a Lax pack in a fulfilled Async. Taida surface is
// Async[Lax[Str]] across all 3 backends.
static taida_val taida_io_stdin_line_async_wrap(taida_val lax) {
    taida_val *obj = (taida_val*)TAIDA_MALLOC(7 * sizeof(taida_val), "stdinLine_async");
    obj[0] = TAIDA_ASYNC_MAGIC | 1;  // magic + refcount
    obj[1] = 1;                       // fulfilled
    obj[2] = lax;
    obj[3] = 0;                       // no error
    obj[4] = 0;                       // no thread
    obj[5] = TAIDA_TAG_PACK;          // Lax is a Pack
    obj[6] = TAIDA_TAG_UNKNOWN;
    return (taida_val)obj;
}

// Build Lax[Str].failure("") wrapped in Async (3-backend parity shape).
static taida_val taida_io_stdin_line_failure(void) {
    taida_val empty = (taida_val)taida_str_alloc(0);
    return taida_io_stdin_line_async_wrap(taida_lax_empty(empty));
}

// Build Lax[Str].success(line) wrapped in Async.
// `buf` is copied into a fresh taida_str so ownership semantics are clean.
static taida_val taida_io_stdin_line_success(const char *buf, size_t n) {
    char *r = taida_str_alloc(n);
    if (n > 0) memcpy(r, buf, n);
    return taida_io_stdin_line_async_wrap(taida_lax_new((taida_val)r, (taida_val)""));
}

// Fallback: non-TTY (pipe / redirect). Use getline semantics shared with
// `taida_io_stdin` so long lines and UTF-8 payloads survive intact. No
// in-band editing is performed (the kernel already buffered the line).
static taida_val taida_io_stdin_line_fallback(const char *prompt) {
    if (prompt && prompt[0] != '\0') { printf("%s", prompt); fflush(stdout); }
    char *buf = NULL;
    size_t cap = 0;
    ssize_t got = getline(&buf, &cap, stdin);
    if (got < 0) {
        free(buf);
        return taida_io_stdin_line_failure();
    }
    size_t ulen = (size_t)got;
    if (ulen > 0 && buf[ulen - 1] == '\n') {
        ulen--;
        if (ulen > 0 && buf[ulen - 1] == '\r') ulen--;
    }
    taida_val result = taida_io_stdin_line_success(buf, ulen);
    free(buf);
    return result;
}

// Find the start of the previous UTF-8 codepoint inside `buf[0..len)`.
// Returns the byte offset of the codepoint start, or 0 if the buffer
// is empty or all continuation bytes.
static size_t tl_utf8_prev_start(const char *buf, size_t len) {
    if (len == 0) return 0;
    size_t i = len - 1;
    while (i > 0 && ((unsigned char)buf[i] & 0xC0) == 0x80) {
        i--;
    }
    return i;
}

taida_val taida_io_stdin_line(taida_val prompt_ptr) {
    const char *prompt = (const char*)prompt_ptr;

    // Non-TTY (pipe / redirect): fall back to getline so that e.g.
    // `echo "..." | taida run` still works. Editing keys are irrelevant
    // when the kernel has already framed the line.
    if (!isatty(STDIN_FILENO) || !isatty(STDOUT_FILENO)) {
        return taida_io_stdin_line_fallback(prompt);
    }

    struct termios saved_termios, raw_termios;
    if (tcgetattr(STDIN_FILENO, &saved_termios) == -1) {
        // Cannot enter raw mode — still return a Lax shape.
        return taida_io_stdin_line_fallback(prompt);
    }
    raw_termios = saved_termios;
    // linenoise: disable canonical mode + echo so we own the edit loop.
    raw_termios.c_lflag &= ~(ECHO | ICANON | IEXTEN | ISIG);
    raw_termios.c_iflag &= ~(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
    raw_termios.c_cflag |= CS8;
    raw_termios.c_oflag &= ~(OPOST);
    raw_termios.c_cc[VMIN] = 1;
    raw_termios.c_cc[VTIME] = 0;
    if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw_termios) == -1) {
        return taida_io_stdin_line_fallback(prompt);
    }

    // Print prompt (raw mode means \r\n needs explicit \r).
    if (prompt && prompt[0] != '\0') {
        ssize_t pw = write(STDOUT_FILENO, prompt, strlen(prompt));
        (void)pw;
    }

    size_t cap = 256;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "stdinLine_edit");
    int completed = 0; // 1 = Enter pressed, 0 = EOF / Ctrl-C

    while (1) {
        unsigned char c;
        ssize_t nread = read(STDIN_FILENO, &c, 1);
        if (nread <= 0) {
            // EOF (pipe closure mid-read on a TTY is rare but possible).
            break;
        }

        if (c == 13 /* \r */ || c == 10 /* \n */) {
            // Enter: echo \r\n so the next shell prompt starts on a fresh
            // line, then break out of the edit loop.
            ssize_t ew = write(STDOUT_FILENO, "\r\n", 2); (void)ew;
            completed = 1;
            break;
        }
        if (c == 3 /* Ctrl-C */) {
            // Cancel — restore termios and return Lax failure.
            ssize_t ew = write(STDOUT_FILENO, "^C\r\n", 4); (void)ew;
            break;
        }
        if (c == 4 /* Ctrl-D */) {
            if (len == 0) {
                // EOF on empty line → Lax failure.
                ssize_t ew = write(STDOUT_FILENO, "\r\n", 2); (void)ew;
                break;
            }
            continue;
        }
        if (c == 127 /* DEL / Backspace */ || c == 8 /* BS */) {
            // UTF-8-aware Backspace: delete the last full codepoint.
            if (len > 0) {
                size_t prev = tl_utf8_prev_start(buf, len);
                len = prev;
                // Move cursor back 1 display column (approximation —
                // CJK / emoji are ≥ 2 columns but one codepoint, so
                // the visual blank may leave a stray space on wide
                // characters. Acceptable for a minimal editor.)
                ssize_t ew = write(STDOUT_FILENO, "\b \b", 3); (void)ew;
            }
            continue;
        }
        if (c == 21 /* Ctrl-U */) {
            // Clear the current line buffer, redraw prompt.
            while (len > 0) {
                ssize_t ew = write(STDOUT_FILENO, "\b \b", 3); (void)ew;
                len--;
            }
            continue;
        }
        if (c < 32) {
            // Ignore other control chars. Arrow keys arrive as 3-byte
            // ESC sequences — best-effort consume by reading the next
            // 2 bytes and discarding. History / cursor movement is
            // intentionally not implemented in this minimal editor.
            if (c == 27) {
                unsigned char seq[2];
                if (read(STDIN_FILENO, &seq[0], 1) == 1) {
                    ssize_t _r = read(STDIN_FILENO, &seq[1], 1);
                    (void)_r;
                }
            }
            continue;
        }

        // Regular byte (ASCII or UTF-8 codepoint start / continuation).
        // Buffer it, echo as-is. Terminal renders UTF-8 correctly so
        // multibyte characters appear as a single glyph.
        if (len + 1 >= cap) {
            cap *= 2;
            TAIDA_REALLOC(buf, cap, "stdinLine_edit");
        }
        buf[len++] = (char)c;
        ssize_t ew = write(STDOUT_FILENO, (const char*)&c, 1); (void)ew;
    }

    tcsetattr(STDIN_FILENO, TCSAFLUSH, &saved_termios);

    if (!completed) {
        free(buf);
        return taida_io_stdin_line_failure();
    }
    taida_val result = taida_io_stdin_line_success(buf, len);
    free(buf);
    return result;
}

