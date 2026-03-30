#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <dirent.h>
#include <sys/stat.h>
#include <unistd.h>
#include <errno.h>
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
// List state check methods
taida_val taida_list_last_index_of(taida_ptr list_ptr, taida_val item);
taida_val taida_list_any(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
taida_val taida_list_all(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
taida_val taida_list_none(taida_ptr list_ptr, taida_fn_ptr fn_ptr);
// Prelude functions
taida_ptr taida_io_stdin(taida_ptr prompt_ptr);
taida_ptr taida_sha256(taida_val value);
taida_val taida_time_now_ms(void);
taida_val taida_time_sleep(taida_val ms);
taida_ptr taida_json_encode(taida_val val);
taida_ptr taida_json_pretty(taida_val val);
taida_val taida_register_field_name(taida_val hash, taida_ptr name_ptr);
taida_val taida_register_field_type(taida_val hash, taida_ptr name_ptr, taida_val type_tag);
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
taida_val taida_typeof(taida_val val, taida_val tag);
taida_val taida_polymorphic_contains(taida_val obj, taida_val needle);
taida_val taida_polymorphic_index_of(taida_val obj, taida_val needle);
taida_val taida_polymorphic_last_index_of(taida_val obj, taida_val needle);
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

taida_val taida_debug_int(taida_val value) {
    printf("%" PRId64 "\n", value);
    return 0;
}

taida_val taida_debug_float(double value) {
    printf("%g\n", value);
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
static void taida_register_lax_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;
    taida_register_field_name((taida_val)HASH_HAS_VALUE, (taida_val)"hasValue");
    taida_register_field_name((taida_val)HASH___VALUE, (taida_val)"__value");
    taida_register_field_name((taida_val)HASH___DEFAULT, (taida_val)"__default");
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
    // __lax_type_str is static, not heap-allocated - leave as INT(0) to skip free
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
    // __lax_type_str is static - leave tag as INT(0)
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
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1);  // hasValue = true
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, value);
    // retain-on-store: value が Pack/List/Closure の場合 retain + tag 設定
    taida_retain_and_tag_field(pack, 1, value);
    taida_pack_set_hash(pack, 2, (taida_val)HASH___DEFAULT);  // reuse hash slot (field 2)
    taida_pack_set(pack, 2, 0);  // __error = Unit (no error)
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__gorillax_type_str);
    return pack;
}

taida_val taida_gorillax_err(taida_val error) {
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);  // hasValue = false
    taida_pack_set_tag(pack, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(pack, 1, (taida_val)HASH___VALUE);
    taida_pack_set(pack, 1, 0);  // __value = Unit
    taida_pack_set_hash(pack, 2, (taida_val)HASH___DEFAULT);  // reuse hash slot (field 2)
    taida_pack_set(pack, 2, error);  // __error may be a Pack
    taida_pack_set_tag(pack, 2, TAIDA_TAG_PACK);
    if (error != 0) taida_retain(error);  // retain-on-store: error pack child
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__gorillax_type_str);
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
    taida_pack_set_hash(pack, 2, (taida_val)HASH___DEFAULT);
    taida_pack_set(pack, 2, taida_pack_get_idx(ptr, 2));  // __error
    // QF-50: retain-on-store for __error (typically a Pack)
    taida_retain_and_tag_field(pack, 2, taida_pack_get_idx(ptr, 2));
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__relaxed_gorillax_type_str);
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

// Str[x]() — always succeeds
taida_val taida_str_mold_int(taida_val v) {
    return taida_lax_new(taida_str_from_int(v), (taida_val)"");
}
taida_val taida_str_mold_float(double v) {
    return taida_lax_new(taida_str_from_float(v), (taida_val)"");
}
taida_val taida_str_mold_bool(taida_val v) {
    return taida_lax_new(taida_str_from_bool(v), (taida_val)"");
}
taida_val taida_str_mold_str(taida_val v) {
    return taida_lax_new(v, (taida_val)"");
}

// Int[x]() — Str parse can fail
taida_val taida_int_mold_int(taida_val v) {
    return taida_lax_new(v, 0);
}
taida_val taida_int_mold_float(double v) {
    return taida_lax_new((taida_val)v, 0);
}
taida_val taida_int_mold_str(taida_val v) {
    const char *s = (const char *)v;
    if (!s || *s == '\0') return taida_lax_empty(0);
    char *end;
    taida_val result = strtol(s, &end, 10);
    if (*end != '\0') return taida_lax_empty(0);  // parse failed
    return taida_lax_new(result, 0);
}
taida_val taida_int_mold_bool(taida_val v) {
    return taida_lax_new(v ? 1 : 0, 0);
}

taida_val taida_int_mold_auto(taida_val v) {
    if (v == 0) return taida_int_mold_int(0);
    if (v < 0 || v < 4096) return taida_int_mold_int(v);

    if (taida_ptr_is_readable(v, sizeof(taida_val))) {
        taida_val tag = ((taida_val*)v)[0];
        if (taida_has_magic_header(tag)) {
            return taida_lax_empty(0);
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
    return taida_lax_new(_d2l(d), _d2l(0.0));
}
taida_val taida_float_mold_float(double v) {
    return taida_lax_new(_d2l(v), _d2l(0.0));
}
taida_val taida_float_mold_str(taida_val v) {
    const char *s = (const char *)v;
    if (!s || *s == '\0') return taida_lax_empty(_d2l(0.0));
    char *end;
    double result = strtod(s, &end);
    if (*end != '\0') return taida_lax_empty(_d2l(0.0));  // parse failed
    return taida_lax_new(_d2l(result), _d2l(0.0));
}
taida_val taida_float_mold_bool(taida_val v) {
    return taida_lax_new(_d2l(v ? 1.0 : 0.0), _d2l(0.0));
}

// Bool[x]() — Str accepts only "true"/"false"
taida_val taida_bool_mold_int(taida_val v) {
    return taida_lax_new(v != 0 ? 1 : 0, 0);
}
taida_val taida_bool_mold_float(double v) {
    return taida_lax_new(v != 0.0 ? 1 : 0, 0);
}
taida_val taida_bool_mold_str(taida_val v) {
    const char *s = (const char *)v;
    if (!s) return taida_lax_empty(0);
    if (strcmp(s, "true") == 0) return taida_lax_new(1, 0);
    if (strcmp(s, "false") == 0) return taida_lax_new(0, 0);
    return taida_lax_empty(0);  // not "true" or "false"
}
taida_val taida_bool_mold_bool(taida_val v) {
    return taida_lax_new(v, 0);
}

static int taida_char_to_digit(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'z') return c - 'a' + 10;
    if (c >= 'A' && c <= 'Z') return c - 'A' + 10;
    return -1;
}

taida_val taida_int_mold_str_base(taida_val v, taida_val base) {
    if (base < 2 || base > 36) return taida_lax_empty(0);
    const char *s = (const char*)v;
    size_t len = 0;
    if (!taida_read_cstr_len_safe(s, 4096, &len) || len == 0) return taida_lax_empty(0);

    int negative = 0;
    size_t i = 0;
    if (s[0] == '-') {
        negative = 1;
        i = 1;
        if (len == 1) return taida_lax_empty(0);
    }

    uint64_t acc = 0;
    uint64_t limit = negative ? ((uint64_t)INT64_MAX + 1ULL) : (uint64_t)INT64_MAX;
    for (; i < len; i++) {
        int d = taida_char_to_digit((unsigned char)s[i]);
        if (d < 0 || d >= base) return taida_lax_empty(0);
        if (acc > (limit - (uint64_t)d) / (uint64_t)base) return taida_lax_empty(0);
        acc = acc * (uint64_t)base + (uint64_t)d;
    }

    int64_t out = 0;
    if (negative) {
        if (acc == ((uint64_t)INT64_MAX + 1ULL)) out = INT64_MIN;
        else out = -(int64_t)acc;
    } else {
        out = (int64_t)acc;
    }
    return taida_lax_new((taida_val)out, 0);
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
static taida_val taida_pack_get_field_tag(taida_ptr pack_ptr, taida_val field_hash) {
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
    ((taida_val*)list_ptr)[3] = tag;
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
        // Split into chars
        taida_val len = (taida_val)strlen(s);
        for (taida_val i = 0; i < len; i++) {
            char *c = taida_str_alloc(1);
            c[0] = s[i];
            list = taida_list_push(list, (taida_val)c);
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
    char tmp[64];
    snprintf(tmp, sizeof(tmp), "%g", a);
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

taida_val taida_list_zip(taida_val list1, taida_val list2) {
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
    size_t slots = taida_safe_add((size_t)HM_HEADER, taida_safe_mul((size_t)cap, 3, "hm_new_with_cap slots"), "hm_new_with_cap total");
    size_t alloc_size = taida_safe_mul(slots, sizeof(taida_val), "hm_new_with_cap bytes");
    taida_val *hm = (taida_val*)calloc(1, alloc_size);
    if (!hm) { fprintf(stderr, "taida: out of memory (taida_hashmap_new_with_cap)\n"); exit(1); }
    hm[0] = TAIDA_HMAP_MAGIC | 1;  // Magic + refcount
    hm[1] = cap;  // capacity
    hm[2] = 0;    // length
    hm[3] = TAIDA_TAG_UNKNOWN;  // value_type_tag (unknown until set)
    return (taida_val)hm;
}

void taida_hashmap_set_value_tag(taida_val hm_ptr, taida_val tag) {
    ((taida_val*)hm_ptr)[3] = tag;
}

taida_val taida_hashmap_new(void) {
    return taida_hashmap_new_with_cap(16);
}

// Internal set used by resize (does not trigger resize)
static void taida_hashmap_set_internal(taida_val *hm, taida_val cap, taida_val key_hash, taida_val key_ptr, taida_val value) {
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
            return;
        }
        if (sh == key_hash && taida_hashmap_key_eq(sk, key_ptr)) {
            hm[HM_HEADER + slot * 3 + 2] = value;
            return;
        }
    }
}

// Resize the hashmap to new_cap (re-hash all occupied entries)
// This is a MOVE operation — entries transfer ownership from old to new.
// No retain/release needed; the old table's raw memory is freed.
static taida_val taida_hashmap_resize(taida_val hm_ptr, taida_val new_cap) {
    taida_val *old_hm = (taida_val*)hm_ptr;
    taida_val old_cap = old_hm[1];
    taida_val new_hm_ptr = taida_hashmap_new_with_cap(new_cap);
    taida_val *new_hm = (taida_val*)new_hm_ptr;
    // NO-1: propagate value_type_tag from old to new
    new_hm[3] = old_hm[3];
    for (taida_val i = 0; i < old_cap; i++) {
        taida_val sh = old_hm[HM_HEADER + i * 3];
        taida_val sk = old_hm[HM_HEADER + i * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_hashmap_set_internal(new_hm, new_cap, sh, sk, old_hm[HM_HEADER + i * 3 + 2]);
        }
    }
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
    for (taida_val i = 0; i < cap; i++) {
        taida_val sh = hm[HM_HEADER + i * 3];
        taida_val sk = hm[HM_HEADER + i * 3 + 1];
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
    for (taida_val i = 0; i < cap; i++) {
        taida_val sh = hm[HM_HEADER + i * 3];
        taida_val sk = hm[HM_HEADER + i * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val v = hm[HM_HEADER + i * 3 + 2];
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
static taida_val taida_hashmap_clone(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];
    taida_val val_tag = hm[3];  // value_type_tag
    // M-03: Guard against negative/overflow cap and NULL malloc result.
    if (cap < 0) {
        fprintf(stderr, "taida: invalid hashmap cap %" PRId64 " in taida_hashmap_clone\n", (int64_t)cap);
        exit(1);
    }
    size_t total = taida_safe_add((size_t)HM_HEADER, taida_safe_mul((size_t)cap, 3, "hm_clone slots"), "hm_clone total");
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

// Entries: returns list of BuchiPack @(key, value)
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
    for (taida_val i = 0; i < cap; i++) {
        taida_val sh = hm[HM_HEADER + i * 3];
        taida_val sk = hm[HM_HEADER + i * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val pair = taida_pack_new(2);
            taida_pack_set_hash(pair, 0, (taida_val)HASH_KEY);
            // NO-1: tag + retain key (Str) and value fields in pair pack
            taida_pack_set_tag(pair, 0, TAIDA_TAG_STR);
            taida_hashmap_key_retain(sk);
            taida_pack_set(pair, 0, sk);
            taida_pack_set_hash(pair, 1, (taida_val)HASH_VAL);
            taida_pack_set_tag(pair, 1, val_tag);
            taida_val v = hm[HM_HEADER + i * 3 + 2];
            taida_hashmap_val_retain(v, val_tag);
            taida_pack_set(pair, 1, v);
            list = taida_list_push(list, pair);
        }
    }
    return list;
}

// Merge two hashmaps (other overwrites this)
taida_val taida_hashmap_merge(taida_val hm_ptr, taida_val other_ptr) {
    taida_val new_hm = taida_hashmap_clone(hm_ptr);
    taida_val *other = (taida_val*)other_ptr;
    taida_val cap = other[1];
    for (taida_val i = 0; i < cap; i++) {
        taida_val sh = other[HM_HEADER + i * 3];
        taida_val sk = other[HM_HEADER + i * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            new_hm = taida_hashmap_set(new_hm, sh, sk, other[HM_HEADER + i * 3 + 2]);
        }
    }
    return new_hm;
}

// HashMap.toString() -> "HashMap({key1: val1, key2: val2})"
taida_val taida_hashmap_to_string(taida_val hm_ptr) {
    taida_val *hm = (taida_val*)hm_ptr;
    taida_val cap = hm[1];

    size_t buf_size = 256;
    char *buf = (char*)TAIDA_MALLOC(buf_size, "hm_to_string");
    // R-03: Use offset tracking instead of strcat (O(n) per call → O(1)).
    memcpy(buf, "HashMap({", 10); /* 9 chars + '\0' */
    size_t off = 9;
    taida_val count = 0;

    for (taida_val i = 0; i < cap; i++) {
        taida_val sh = hm[HM_HEADER + i * 3];
        taida_val sk = hm[HM_HEADER + i * 3 + 1];
        if (HM_SLOT_OCCUPIED(sh, sk)) {
            taida_val value = hm[HM_HEADER + i * 3 + 2];

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

// Result[value, predicate](throw <= error) — create Result with optional predicate
taida_val taida_result_create(taida_val value, taida_val throw_val, taida_val predicate) {
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
taida_val taida_lax_to_string(taida_val lax_ptr) {
    taida_val val = taida_pack_get_idx(lax_ptr, 1);
    taida_val def = taida_pack_get_idx(lax_ptr, 2);
    taida_val rendered = taida_pack_get_idx(lax_ptr, 0)
        ? taida_value_to_display_string(val)
        : taida_value_to_display_string(def);
    const char *rs = (const char*)rendered;
    size_t need = strlen(rs) + 24;
    char *buf = taida_str_alloc(need);
    if (taida_pack_get_idx(lax_ptr, 0)) {
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
        taida_val field_val = pack[2 + i * 3 + 2];
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
        // Check if field is Bool via registry
        int ftype = taida_lookup_field_type(field_hash);
        if (ftype == 4) {
            // Bool: display as true/false
            const char *bv = field_val ? "true" : "false";
            size_t sl = strlen(bv); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string"); } memcpy(buf + len, bv, sl); len += sl; buf[len] = '\0';
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
        taida_val field_val = pack[2 + i * 3 + 2];
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
        // Check if field is Bool via registry
        int ftype = taida_lookup_field_type(field_hash);
        if (ftype == 4) {
            const char *bv = field_val ? "true" : "false";
            size_t sl = strlen(bv); while (len + sl + 1 > cap) { cap *= 2; TAIDA_REALLOC(buf, cap, "to_string_full"); } memcpy(buf + len, bv, sl); len += sl; buf[len] = '\0';
        } else {
            taida_val val_str = taida_value_to_debug_string(field_val);
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
taida_val taida_stdout_display_string(taida_val obj) {
    if (obj == 0) return (taida_val)taida_str_new_copy("0");
    if (taida_is_buchi_pack(obj)) {
        return taida_pack_to_display_string_full(obj);
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
    char *str_val;        // for strings (heap-allocated)
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

static char *json_parse_string_raw(const char **p) {
    if (**p != '"') return NULL;
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
    return buf;
}

static json_val json_parse_string(const char **p) {
    json_val v;
    v.type = JSON_STRING;
    v.str_val = json_parse_string_raw(p);
    v.arr = NULL; v.obj = NULL;
    return v;
}

static json_val json_parse_number(const char **p) {
    json_val v;
    v.str_val = NULL; v.arr = NULL; v.obj = NULL;
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
    v.str_val = NULL; v.obj = NULL;
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
    v.str_val = NULL; v.arr = NULL;
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
    v.str_val = NULL; v.arr = NULL; v.obj = NULL;
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
static taida_val json_default_value_for_desc(const char *desc) {
    if (!desc || !*desc) return 0;
    switch (desc[0]) {
        case 'i': return 0;
        case 'f': return _d2l(0.0);
        case 's': {
            char *empty = (char*)TAIDA_MALLOC(1, "json_default_str");
            empty[0] = '\0';
            return (taida_val)empty;
        }
        case 'b': return 0;
        case 'T': {
            // Create default BuchiPack for TypeDef
            json_val null_val;
            null_val.type = JSON_NULL;
            null_val.str_val = NULL; null_val.arr = NULL; null_val.obj = NULL;
            return json_apply_schema(&null_val, &desc);
        }
        case 'L': {
            // Empty list
            return taida_list_new();
        }
        default: return 0;
    }
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
// type_tag: 0=unknown, 1=Int, 2=Float, 3=Str, 4=Bool
static struct { taida_val hash; const char *name; int type_tag; } __field_registry[FIELD_REGISTRY_CAP];
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

// Helper: serialize a BuchiPack's fields as JSON object
// Fields are sorted alphabetically (matching interpreter/JS behavior).
// All __ fields are skipped (__type, __value, __default, __entries, __items).
static void json_serialize_pack_fields(char **buf, size_t *cap, size_t *len, taida_val *pack, taida_val fc, int indent, int depth) {
    // Collect visible fields: (name, val, type_hint, index for stable sort)
    typedef struct { const char *name; taida_val val; int type_hint; } JsonField;
    JsonField fields[100];
    int nfields = 0;
    for (taida_val i = 0; i < fc && nfields < 100; i++) {
        taida_val field_hash = pack[2 + i * 3];
        taida_val field_val = pack[2 + i * 3 + 2];
        const char *fname = taida_lookup_field_name(field_hash);
        if (!fname) continue;
        // Skip all __ fields (__type, __value, __default, __entries, __items)
        if (fname[0] == '_' && fname[1] == '_') {
            continue;
        }
        int ftype = taida_lookup_field_type(field_hash);
        fields[nfields].name = fname;
        fields[nfields].val = field_val;
        fields[nfields].type_hint = ftype;
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
        json_serialize_typed(buf, cap, len, fields[i].val, indent, depth + 1, fields[i].type_hint);
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

    // Null/Unit
    if (val == 0) {
        if (type_hint == 3) { // Str
            json_append(buf, cap, len, "\"\"");
        } else {
            json_append(buf, cap, len, "{}");
        }
        return;
    }

    // Integer hints: always serialize as number
    if (type_hint == 1 || type_hint == 2) { // Int or Float
        char num[32];
        snprintf(num, sizeof(num), "%" PRId64 "", val);
        json_append(buf, cap, len, num);
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
        for (taida_val i = 0; i < hm_cap; i++) {
            taida_val slot_hash = hm[HM_HEADER + i * 3];
            taida_val slot_key = hm[HM_HEADER + i * 3 + 1];
            if (HM_SLOT_OCCUPIED(slot_hash, slot_key)) {
                if (count > 0) json_append_char(buf, cap, len, ',');
                if (indent > 0) json_append_indent(buf, cap, len, indent, depth + 1);
                const char *key_str = (const char*)slot_key;
                if (!key_str) key_str = "";
                json_append_escaped_str(buf, cap, len, key_str);
                json_append_char(buf, cap, len, ':');
                if (indent > 0) json_append_char(buf, cap, len, ' ');
                json_serialize_typed(buf, cap, len, hm[HM_HEADER + i * 3 + 2], indent, depth + 1, 0);
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

taida_val taida_io_stdin(taida_val prompt_ptr) {
    // Print prompt if provided
    const char *prompt = (const char*)prompt_ptr;
    if (prompt && prompt[0] != '\0') {
        printf("%s", prompt);
        fflush(stdout);
    }
    // Read a line from stdin
    char line[4096];
    if (fgets(line, sizeof(line), stdin) == NULL) {
        // EOF or error — return empty string
        return (taida_val)taida_str_alloc(0);
    }
    // Strip trailing newline
    size_t slen = strlen(line);
    if (slen > 0 && line[slen - 1] == '\n') {
        line[slen - 1] = '\0';
        slen--;
        if (slen > 0 && line[slen - 1] == '\r') {
            line[slen - 1] = '\0';
            slen--;
        }
    }
    char *r = taida_str_alloc(slen);
    memcpy(r, line, slen);
    return (taida_val)r;
}

taida_val taida_io_stdout(taida_val val_ptr) {
    // For now, treat val as a string pointer
    const char *s = (const char*)val_ptr;
    if (s) printf("%s\n", s);
    return 0;
}

taida_val taida_io_stderr(taida_val val_ptr) {
    const char *s = (const char*)val_ptr;
    if (s) fprintf(stderr, "%s\n", s);
    return 0;
}

// ── taida-lang/os package — Native runtime ────────────────

// Helper: build os Result success BuchiPack @(ok=true, code=0, message="")
static taida_val taida_os_result_success(taida_val inner) {
    return taida_result_create(inner, 0, 0);
}

// Helper: build os Result failure with IoError
static taida_val taida_os_result_failure(int err_code, const char *err_msg) {
    // inner = @(ok=false, code=errno, message=err_msg, kind=...)
    const char *message = err_msg ? err_msg : "unknown io error";
    const char *kind = taida_os_error_kind(err_code, message);
    taida_val inner = taida_pack_new(4);
    // ok field
    taida_val ok_hash = 0x08b05d07b5566befULL;  // FNV-1a("ok")
    taida_pack_set_hash(inner, 0, (taida_val)ok_hash);
    taida_pack_set(inner, 0, 0);  // false
    // code field
    taida_val code_hash = 0x0bb51791194b4414ULL;  // FNV-1a("code")
    taida_pack_set_hash(inner, 1, (taida_val)code_hash);
    taida_pack_set(inner, 1, (taida_val)err_code);
    // message field
    taida_val msg_hash = 0x546401b5d2a8d2a4ULL;   // FNV-1a("message")
    taida_pack_set_hash(inner, 2, (taida_val)msg_hash);
    char *msg_copy = taida_str_new_copy(message);
    taida_pack_set(inner, 2, (taida_val)msg_copy);
    // kind field
    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(inner, 3, kind_hash);
    char *kind_copy = taida_str_new_copy(kind);
    taida_pack_set(inner, 3, (taida_val)kind_copy);

    taida_val error = taida_make_io_error(err_code, message);
    return taida_result_create(inner, error, 0);
}

// Helper: build os ok inner @(ok=true, code=0, message="")
static taida_val taida_os_ok_inner(void) {
    taida_val inner = taida_pack_new(3);
    taida_val ok_hash = 0x08b05d07b5566befULL;
    taida_pack_set_hash(inner, 0, (taida_val)ok_hash);
    taida_pack_set(inner, 0, 1);  // true
    taida_val code_hash = 0x0bb51791194b4414ULL;
    taida_pack_set_hash(inner, 1, (taida_val)code_hash);
    taida_pack_set(inner, 1, 0);
    taida_val msg_hash = 0x546401b5d2a8d2a4ULL;
    taida_pack_set_hash(inner, 2, (taida_val)msg_hash);
    taida_pack_set(inner, 2, (taida_val)"");
    return inner;
}

// Helper: build process result inner @(stdout, stderr, code)
static taida_val taida_os_process_inner(const char *out, const char *err, taida_val code) {
    taida_val inner = taida_pack_new(3);
    // stdout
    taida_val stdout_hash = 0x42e6d785a74f8c66ULL;  // FNV-1a("stdout")
    taida_pack_set_hash(inner, 0, (taida_val)stdout_hash);
    char *out_copy = taida_str_new_copy(out);
    taida_pack_set(inner, 0, (taida_val)out_copy);
    // stderr
    taida_val stderr_hash = 0x104ce5858b0a80b5ULL;  // FNV-1a("stderr")
    taida_pack_set_hash(inner, 1, (taida_val)stderr_hash);
    char *err_copy = taida_str_new_copy(err);
    taida_pack_set(inner, 1, (taida_val)err_copy);
    // code
    taida_val code_hash = 0x0bb51791194b4414ULL;
    taida_pack_set_hash(inner, 2, (taida_val)code_hash);
    taida_pack_set(inner, 2, code);
    return inner;
}

// ── Read[path]() → Lax[Str] ──────────────────────────────
taida_val taida_os_read(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_lax_empty((taida_val)"");

    // Check file size (64MB limit)
    struct stat st;
    if (stat(path, &st) != 0) return taida_lax_empty((taida_val)"");
    if (st.st_size > 64 * 1024 * 1024) return taida_lax_empty((taida_val)"");

    FILE *f = fopen(path, "r");
    if (!f) return taida_lax_empty((taida_val)"");

    taida_val size = st.st_size;
    char *buf = taida_str_alloc(size);
    taida_val read_bytes = (taida_val)fread(buf, 1, size, f);
    fclose(f);
    buf[read_bytes] = '\0';

    return taida_lax_new((taida_val)buf, (taida_val)"");
}

// ── readBytes(path) → Lax[Bytes] ──────────────────────────
taida_val taida_os_read_bytes(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_lax_empty(taida_bytes_default_value());

    struct stat st;
    if (stat(path, &st) != 0) return taida_lax_empty(taida_bytes_default_value());
    if (st.st_size > 64 * 1024 * 1024) return taida_lax_empty(taida_bytes_default_value());

    FILE *f = fopen(path, "rb");
    if (!f) return taida_lax_empty(taida_bytes_default_value());

    taida_val size = st.st_size;
    unsigned char *buf = NULL;
    if (size > 0) {
        buf = (unsigned char*)malloc((size_t)size);
        if (!buf) {
            fclose(f);
            return taida_lax_empty(taida_bytes_default_value());
        }
    }

    size_t read_bytes = 0;
    if (size > 0) {
        read_bytes = fread(buf, 1, (size_t)size, f);
    }
    fclose(f);

    taida_val bytes = taida_bytes_from_raw(buf, (taida_val)read_bytes);
    free(buf);
    return taida_lax_new(bytes, taida_bytes_default_value());
}

// ── String comparator for qsort ──────────────────────────
static int taida_cmp_strings(const void *a, const void *b) {
    return strcmp(*(const char**)a, *(const char**)b);
}

// ── ListDir[path]() → Lax[@[Str]] ────────────────────────
taida_val taida_os_list_dir(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_lax_empty(taida_list_new());

    DIR *dir = opendir(path);
    if (!dir) return taida_lax_empty(taida_list_new());

    // Collect entries, then sort
    taida_val capacity = 64;
    taida_val count = 0;
    char **names = (char**)TAIDA_MALLOC(capacity * sizeof(char*), "listDir_init");

    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        // Skip . and ..
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
        if (count >= capacity) {
            // M-12: Guard against taida_val overflow on capacity *= 2.
            // capacity is int64_t; if it exceeds INT64_MAX/2, doubling would
            // overflow. In practice this is unreachable (>4 billion entries),
            // but the guard prevents undefined behavior.
            if (capacity > (taida_val)(INT64_MAX / 2)) {
                fprintf(stderr, "taida: directory entry count overflow in taida_os_list_dir\n");
                // Clean up already-collected names
                for (taida_val i = 0; i < count; i++) taida_str_release((taida_val)names[i]);
                free(names);
                closedir(dir);
                return taida_lax_empty(taida_list_new());
            }
            capacity *= 2;
            TAIDA_REALLOC(names, taida_safe_mul((size_t)capacity, sizeof(char*), "listDir_grow"), "listDir");
        }
        names[count] = taida_str_new_copy(entry->d_name);
        count++;
    }
    closedir(dir);

    // Sort alphabetically
    if (count > 1) {
        qsort(names, count, sizeof(char*), taida_cmp_strings);
    }

    taida_val list = taida_list_new();
    for (taida_val i = 0; i < count; i++) {
        list = taida_list_push(list, (taida_val)names[i]);
    }
    free(names);

    return taida_lax_new(list, taida_list_new());
}

// ── Stat[path]() → Lax[@(size: Int, modified: Str, isDir: Bool)] ──
taida_val taida_os_stat(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;

    // Build default stat pack
    taida_val default_pack = taida_pack_new(3);
    taida_val size_hash = 0x4dea9618e618ae3cULL;     // FNV-1a("size")
    taida_val modified_hash = 0xd381b19c7fd35852ULL;  // FNV-1a("modified")
    taida_val is_dir_hash = 0x641d9cfa1a584ee4ULL;    // FNV-1a("isDir")
    taida_pack_set_hash(default_pack, 0, (taida_val)size_hash);
    taida_pack_set(default_pack, 0, 0);
    taida_pack_set_hash(default_pack, 1, (taida_val)modified_hash);
    taida_pack_set(default_pack, 1, (taida_val)"");
    taida_pack_set_hash(default_pack, 2, (taida_val)is_dir_hash);
    taida_pack_set(default_pack, 2, 0);

    if (!path) return taida_lax_empty(default_pack);

    struct stat st;
    if (stat(path, &st) != 0) return taida_lax_empty(default_pack);

    // Format modified time as RFC3339/UTC
    struct tm tm_buf;
    struct tm *tm_utc = gmtime_r(&st.st_mtime, &tm_buf);
    char time_buf[32];
    if (tm_utc) {
        strftime(time_buf, sizeof(time_buf), "%Y-%m-%dT%H:%M:%SZ", tm_utc);
    } else {
        // R-11: memcpy for fixed-length literal (no format parsing overhead)
        memcpy(time_buf, "1970-01-01T00:00:00Z", 21); /* 20 chars + '\0' */
    }
    char *time_str = taida_str_new_copy(time_buf);

    taida_val stat_pack = taida_pack_new(3);
    taida_pack_set_hash(stat_pack, 0, (taida_val)size_hash);
    taida_pack_set(stat_pack, 0, (taida_val)st.st_size);
    taida_pack_set_hash(stat_pack, 1, (taida_val)modified_hash);
    taida_pack_set(stat_pack, 1, (taida_val)time_str);
    taida_pack_set_hash(stat_pack, 2, (taida_val)is_dir_hash);
    taida_pack_set(stat_pack, 2, S_ISDIR(st.st_mode) ? 1 : 0);

    return taida_lax_new(stat_pack, default_pack);
}

// ── Exists[path]() → Bool ─────────────────────────────────
taida_val taida_os_exists(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return 0;
    return access(path, F_OK) == 0 ? 1 : 0;
}

// ── EnvVar[name]() → Lax[Str] ─────────────────────────────
taida_val taida_os_env_var(taida_val name_ptr) {
    const char *name = (const char*)name_ptr;
    if (!name) return taida_lax_empty((taida_val)"");
    const char *val = getenv(name);
    if (!val) return taida_lax_empty((taida_val)"");
    char *copy = taida_str_new_copy(val);
    return taida_lax_new((taida_val)copy, (taida_val)"");
}

// ── writeFile(path, content) → Result ──────────────────────
taida_val taida_os_write_file(taida_val path_ptr, taida_val content_ptr) {
    const char *path = (const char*)path_ptr;
    const char *content = (const char*)content_ptr;
    if (!path || !content) return taida_os_result_failure(EINVAL, "writeFile: invalid arguments");

    FILE *f = fopen(path, "w");
    if (!f) return taida_os_result_failure(errno, strerror(errno));

    size_t len = strlen(content);
    size_t written = fwrite(content, 1, len, f);
    fclose(f);

    if (written != len) return taida_os_result_failure(errno, strerror(errno));
    return taida_os_result_success(taida_os_ok_inner());
}

// ── writeBytes(path, content) → Result ─────────────────────
taida_val taida_os_write_bytes(taida_val path_ptr, taida_val content_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "writeBytes: invalid arguments");

    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(content_ptr)) {
        taida_val *bytes = (taida_val*)content_ptr;
        taida_val len = bytes[1];
        if (len < 0) return taida_os_result_failure(EINVAL, "writeBytes: invalid bytes payload");
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) return taida_os_result_failure(EINVAL, "writeBytes: payload too large");
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "writeBytes_payload");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *content = (const char*)content_ptr;
        size_t content_len = 0;
        if (!taida_read_cstr_len_safe(content, 65536, &content_len)) {
            return taida_os_result_failure(EINVAL, "writeBytes: invalid data");
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(content_len, "writeBytes_payload");
        memcpy(payload_buf, content, content_len);
        payload_len = content_len;
    }

    FILE *f = fopen(path, "wb");
    if (!f) {
        free(payload_buf);
        return taida_os_result_failure(errno, strerror(errno));
    }

    size_t written = 0;
    if (payload_len > 0) {
        written = fwrite(payload_buf, 1, payload_len, f);
    }
    int saved_errno = errno;
    fclose(f);
    free(payload_buf);

    if (written != payload_len) return taida_os_result_failure(saved_errno, strerror(saved_errno));
    return taida_os_result_success(taida_os_ok_inner());
}

// ── appendFile(path, content) → Result ─────────────────────
taida_val taida_os_append_file(taida_val path_ptr, taida_val content_ptr) {
    const char *path = (const char*)path_ptr;
    const char *content = (const char*)content_ptr;
    if (!path || !content) return taida_os_result_failure(EINVAL, "appendFile: invalid arguments");

    FILE *f = fopen(path, "a");
    if (!f) return taida_os_result_failure(errno, strerror(errno));

    size_t len = strlen(content);
    size_t written = fwrite(content, 1, len, f);
    fclose(f);

    if (written != len) return taida_os_result_failure(errno, strerror(errno));
    return taida_os_result_success(taida_os_ok_inner());
}

// ── remove(path) → Result ──────────────────────────────────
// Recursive removal helper
static int taida_os_remove_recursive(const char *path) {
    struct stat st;
    if (lstat(path, &st) != 0) return -1;

    if (S_ISDIR(st.st_mode)) {
        DIR *dir = opendir(path);
        if (!dir) return -1;
        struct dirent *entry;
        while ((entry = readdir(dir)) != NULL) {
            if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
            size_t pathlen = strlen(path) + strlen(entry->d_name) + 2;
            char *child = (char*)TAIDA_MALLOC(pathlen, "remove_recursive");
            snprintf(child, pathlen, "%s/%s", path, entry->d_name);
            int r = taida_os_remove_recursive(child);
            free(child);
            if (r != 0) { closedir(dir); return -1; }
        }
        closedir(dir);
        return rmdir(path);
    } else {
        return unlink(path);
    }
}

taida_val taida_os_remove(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "remove: invalid arguments");

    if (taida_os_remove_recursive(path) != 0) {
        return taida_os_result_failure(errno, strerror(errno));
    }
    return taida_os_result_success(taida_os_ok_inner());
}

// ── createDir(path) → Result (mkdir -p) ────────────────────
static int taida_os_mkdir_p(const char *path) {
    size_t path_len = strlen(path);
    // M-14: Note: mkdir_p returns -1 on failure rather than aborting, so we
    // keep the manual malloc + NULL check pattern here (TAIDA_MALLOC would abort).
    char *tmp = (char*)malloc(path_len + 1);
    if (!tmp) return -1;
    memcpy(tmp, path, path_len + 1);
    for (char *p = tmp + 1; *p; p++) {
        if (*p == '/') {
            *p = '\0';
            if (mkdir(tmp, 0755) != 0 && errno != EEXIST) {
                free(tmp);
                return -1;
            }
            *p = '/';
        }
    }
    int r = mkdir(tmp, 0755);
    free(tmp);
    if (r != 0 && errno != EEXIST) return -1;
    return 0;
}

taida_val taida_os_create_dir(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "createDir: invalid arguments");

    if (taida_os_mkdir_p(path) != 0) {
        return taida_os_result_failure(errno, strerror(errno));
    }
    return taida_os_result_success(taida_os_ok_inner());
}

// ── rename(from, to) → Result ──────────────────────────────
taida_val taida_os_rename(taida_val from_ptr, taida_val to_ptr) {
    const char *from = (const char*)from_ptr;
    const char *to = (const char*)to_ptr;
    if (!from || !to) return taida_os_result_failure(EINVAL, "rename: invalid arguments");

    if (rename(from, to) != 0) {
        return taida_os_result_failure(errno, strerror(errno));
    }
    return taida_os_result_success(taida_os_ok_inner());
}

// ── run(program, args) → Gorillax[@(stdout, stderr, code)] ──
taida_val taida_os_run(taida_val program_ptr, taida_val args_list_ptr) {
    const char *program = (const char*)program_ptr;
    if (!program) return taida_gorillax_err(taida_make_io_error(EINVAL, "run: invalid arguments"));

    // Build argv from list
    taida_val *list = (taida_val*)args_list_ptr;
    taida_val argc = list ? list[2] : 0;

    // Create pipes for stdout and stderr
    int stdout_pipe[2], stderr_pipe[2];
    if (pipe(stdout_pipe) != 0 || pipe(stderr_pipe) != 0) {
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    pid_t pid = fork();
    if (pid < 0) {
        close(stdout_pipe[0]); close(stdout_pipe[1]);
        close(stderr_pipe[0]); close(stderr_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    if (pid == 0) {
        // Child
        close(stdout_pipe[0]);
        close(stderr_pipe[0]);
        dup2(stdout_pipe[1], STDOUT_FILENO);
        dup2(stderr_pipe[1], STDERR_FILENO);
        close(stdout_pipe[1]);
        close(stderr_pipe[1]);

        // Build argv: [program, arg0, arg1, ..., NULL]
        // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
        char **argv = (char**)TAIDA_MALLOC((argc + 2) * sizeof(char*), "exec_argv");
        argv[0] = (char*)program;
        for (taida_val i = 0; i < argc; i++) {
            argv[i + 1] = (char*)list[4 + i];
        }
        argv[argc + 1] = NULL;

        execvp(program, argv);
        // If exec fails
        _exit(127);
    }

    // Parent
    close(stdout_pipe[1]);
    close(stderr_pipe[1]);

    // Read stdout
    size_t out_cap = 4096, out_len = 0;
    char *out_buf = (char*)TAIDA_MALLOC(out_cap, "os_run_stdout");
    ssize_t n;
    while ((n = read(stdout_pipe[0], out_buf + out_len, out_cap - out_len - 1)) > 0) {
        out_len += n;
        if (out_len >= out_cap - 1) {
            out_cap *= 2;
            TAIDA_REALLOC(out_buf, out_cap, "os_run_stdout");
        }
    }
    out_buf[out_len] = '\0';
    close(stdout_pipe[0]);

    // Read stderr
    size_t err_cap = 4096, err_len = 0;
    char *err_buf = (char*)TAIDA_MALLOC(err_cap, "os_run_stderr");
    while ((n = read(stderr_pipe[0], err_buf + err_len, err_cap - err_len - 1)) > 0) {
        err_len += n;
        if (err_len >= err_cap - 1) {
            err_cap *= 2;
            TAIDA_REALLOC(err_buf, err_cap, "os_run_stderr");
        }
    }
    err_buf[err_len] = '\0';
    close(stderr_pipe[0]);

    int status;
    waitpid(pid, &status, 0);
    taida_val exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;

    if (exit_code == 0) {
        taida_val inner = taida_os_process_inner(out_buf, err_buf, exit_code);
        free(out_buf);
        free(err_buf);
        return taida_gorillax_new(inner);
    } else {
        free(out_buf);
        free(err_buf);
        char msg[256];
        snprintf(msg, sizeof(msg), "Process '%s' exited with code %" PRId64 "", program, exit_code);
        taida_val error = taida_make_error("ProcessError", msg);
        return taida_gorillax_err(error);
    }
}

// ── execShell(command) → Gorillax[@(stdout, stderr, code)] ──
taida_val taida_os_exec_shell(taida_val command_ptr) {
    const char *command = (const char*)command_ptr;
    if (!command) return taida_gorillax_err(taida_make_io_error(EINVAL, "execShell: invalid arguments"));

    // Use fork + sh -c to capture both stdout and stderr separately
    int stdout_pipe[2], stderr_pipe[2];
    if (pipe(stdout_pipe) != 0 || pipe(stderr_pipe) != 0) {
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    pid_t pid = fork();
    if (pid < 0) {
        close(stdout_pipe[0]); close(stdout_pipe[1]);
        close(stderr_pipe[0]); close(stderr_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    if (pid == 0) {
        close(stdout_pipe[0]);
        close(stderr_pipe[0]);
        dup2(stdout_pipe[1], STDOUT_FILENO);
        dup2(stderr_pipe[1], STDERR_FILENO);
        close(stdout_pipe[1]);
        close(stderr_pipe[1]);
        execl("/bin/sh", "sh", "-c", command, (char*)NULL);
        _exit(127);
    }

    close(stdout_pipe[1]);
    close(stderr_pipe[1]);

    size_t out_cap = 4096, out_len = 0;
    char *out_buf = (char*)TAIDA_MALLOC(out_cap, "execShell_stdout");
    ssize_t n;
    while ((n = read(stdout_pipe[0], out_buf + out_len, out_cap - out_len - 1)) > 0) {
        out_len += n;
        if (out_len >= out_cap - 1) { out_cap *= 2; TAIDA_REALLOC(out_buf, out_cap, "execShell_stdout"); }
    }
    out_buf[out_len] = '\0';
    close(stdout_pipe[0]);

    size_t err_cap = 4096, err_len = 0;
    char *err_buf = (char*)TAIDA_MALLOC(err_cap, "execShell_stderr");
    while ((n = read(stderr_pipe[0], err_buf + err_len, err_cap - err_len - 1)) > 0) {
        err_len += n;
        if (err_len >= err_cap - 1) { err_cap *= 2; TAIDA_REALLOC(err_buf, err_cap, "execShell_stderr"); }
    }
    err_buf[err_len] = '\0';
    close(stderr_pipe[0]);

    int status;
    waitpid(pid, &status, 0);
    taida_val exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;

    if (exit_code == 0) {
        taida_val inner = taida_os_process_inner(out_buf, err_buf, exit_code);
        free(out_buf);
        free(err_buf);
        return taida_gorillax_new(inner);
    } else {
        free(out_buf);
        free(err_buf);
        char msg[256];
        snprintf(msg, sizeof(msg), "Shell command exited with code %" PRId64 ": %s", exit_code, command);
        taida_val error = taida_make_error("ProcessError", msg);
        return taida_gorillax_err(error);
    }
}

// ── allEnv() → HashMap[Str, Str] ──────────────────────────
extern char **environ;

taida_val taida_os_all_env(void) {
    // F-24 fix: count env vars and set initial capacity accordingly
    taida_val env_count = 0;
    if (environ) {
        for (char **e = environ; *e; e++) env_count++;
    }
    // Capacity should be at least 2x entries for good load factor
    taida_val init_cap = 16;
    while (init_cap * 3 < env_count * 4) init_cap *= 2;  // ensure load < 0.75
    taida_val hm = taida_hashmap_new_with_cap(init_cap);
    // NO-1: allEnv returns HashMap[Str, Str] — set value_type_tag
    taida_hashmap_set_value_tag(hm, TAIDA_TAG_STR);
    if (!environ) return hm;
    for (char **env = environ; *env; env++) {
        char *eq = strchr(*env, '=');
        if (!eq) continue;
        size_t key_len = eq - *env;
        char *key = taida_str_alloc(key_len);
        memcpy(key, *env, key_len);
        char *val = taida_str_new_copy(eq + 1);
        taida_val key_hash = taida_str_hash((taida_val)key);
        hm = taida_hashmap_set(hm, key_hash, (taida_val)key, (taida_val)val);
    }
    return hm;
}

taida_val taida_os_argv(void) {
    taida_val list = taida_list_new();
    if (!taida_cli_argv || taida_cli_argc <= 1) return list;
    // Native binary mode: <program> [args...]
    for (int i = 1; i < taida_cli_argc; i++) {
        const char *arg = taida_cli_argv[i] ? taida_cli_argv[i] : "";
        list = taida_list_push(list, (taida_val)taida_str_new_copy(arg));
    }
    return list;
}

// ── Phase 2: Async OS APIs (pthread-based) ────────────────
// These APIs use pthread to run blocking operations in a background thread,
// returning an Async value that resolves when the thread completes.

#include <sys/socket.h>
#include <netdb.h>
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/time.h>
#include <sys/uio.h>  // NET3-5c: writev() for zero-copy chunk writes
#include <signal.h>   // NB3-5: SIGPIPE suppression for peer-close resilience
#include <dlfcn.h>    // NET5-4a: dlopen for OpenSSL TLS support

// ── NET5-4a: OpenSSL dlopen TLS support ─────────────────────────────
// Load libssl/libcrypto at runtime via dlopen — no compile-time headers needed.
// This avoids requiring libssl-dev at build time while still providing
// TLS server capability when OpenSSL shared libraries are installed.
//
// Opaque handle types — we only ever pass pointers through.
typedef void OSSL_SSL_CTX;
typedef void OSSL_SSL;
typedef void OSSL_SSL_METHOD;
typedef void OSSL_BIO;
typedef void OSSL_X509;
typedef void OSSL_EVP_PKEY;

// Function pointer table for the OpenSSL symbols we need.
static struct {
    int loaded;
    void *libssl_handle;
    void *libcrypto_handle;
    // libssl functions
    OSSL_SSL_METHOD *(*TLS_server_method)(void);
    OSSL_SSL_CTX *(*SSL_CTX_new)(const OSSL_SSL_METHOD *method);
    void (*SSL_CTX_free)(OSSL_SSL_CTX *ctx);
    int (*SSL_CTX_use_certificate_chain_file)(OSSL_SSL_CTX *ctx, const char *file);
    int (*SSL_CTX_use_PrivateKey_file)(OSSL_SSL_CTX *ctx, const char *file, int type);
    int (*SSL_CTX_check_private_key)(const OSSL_SSL_CTX *ctx);
    OSSL_SSL *(*SSL_new)(OSSL_SSL_CTX *ctx);
    void (*SSL_free)(OSSL_SSL *ssl);
    int (*SSL_set_fd)(OSSL_SSL *ssl, int fd);
    int (*SSL_accept)(OSSL_SSL *ssl);
    int (*SSL_read)(OSSL_SSL *ssl, void *buf, int num);
    int (*SSL_write)(OSSL_SSL *ssl, const void *buf, int num);
    int (*SSL_shutdown)(OSSL_SSL *ssl);
    int (*SSL_get_error)(const OSSL_SSL *ssl, int ret);
    long (*SSL_CTX_set_options)(OSSL_SSL_CTX *ctx, long options);
} taida_ossl = { 0, NULL, NULL };

// OpenSSL constants (stable ABI, unlikely to change).
#define TAIDA_SSL_FILETYPE_PEM 1
#define TAIDA_SSL_ERROR_NONE           0
#define TAIDA_SSL_ERROR_SSL            1
#define TAIDA_SSL_ERROR_WANT_READ      2
#define TAIDA_SSL_ERROR_WANT_WRITE     3
#define TAIDA_SSL_ERROR_SYSCALL        5
#define TAIDA_SSL_ERROR_ZERO_RETURN    6
// SSL_OP_NO_SSLv2 | SSL_OP_NO_SSLv3 | SSL_OP_NO_TLSv1 | SSL_OP_NO_TLSv1_1
// Only allow TLS 1.2+ for security.
#define TAIDA_SSL_OP_SECURE  (0x01000000L | 0x02000000L | 0x04000000L | 0x10000000L)

// Forward declaration.
static void taida_ossl_unload(void);

// Load OpenSSL shared libraries via dlopen. Returns 1 on success, 0 on failure.
static int taida_ossl_load(void) {
    if (taida_ossl.loaded) return 1;

    // Try common shared library names.
    taida_ossl.libssl_handle = dlopen("libssl.so.3", RTLD_LAZY);
    if (!taida_ossl.libssl_handle)
        taida_ossl.libssl_handle = dlopen("libssl.so", RTLD_LAZY);
    if (!taida_ossl.libssl_handle) return 0;

    taida_ossl.libcrypto_handle = dlopen("libcrypto.so.3", RTLD_LAZY);
    if (!taida_ossl.libcrypto_handle)
        taida_ossl.libcrypto_handle = dlopen("libcrypto.so", RTLD_LAZY);
    if (!taida_ossl.libcrypto_handle) {
        dlclose(taida_ossl.libssl_handle);
        taida_ossl.libssl_handle = NULL;
        return 0;
    }

    // Resolve symbols. Cast through void* to suppress -Wpedantic warnings.
    #define LOAD_SYM(lib, name) do { \
        *(void**)(&taida_ossl.name) = dlsym(taida_ossl.lib##_handle, #name); \
        if (!taida_ossl.name) { taida_ossl_unload(); return 0; } \
    } while(0)

    LOAD_SYM(libssl, TLS_server_method);
    LOAD_SYM(libssl, SSL_CTX_new);
    LOAD_SYM(libssl, SSL_CTX_free);
    LOAD_SYM(libssl, SSL_CTX_use_certificate_chain_file);
    LOAD_SYM(libssl, SSL_CTX_use_PrivateKey_file);
    LOAD_SYM(libssl, SSL_CTX_check_private_key);
    LOAD_SYM(libssl, SSL_new);
    LOAD_SYM(libssl, SSL_free);
    LOAD_SYM(libssl, SSL_set_fd);
    LOAD_SYM(libssl, SSL_accept);
    LOAD_SYM(libssl, SSL_read);
    LOAD_SYM(libssl, SSL_write);
    LOAD_SYM(libssl, SSL_shutdown);
    LOAD_SYM(libssl, SSL_get_error);
    LOAD_SYM(libssl, SSL_CTX_set_options);

    #undef LOAD_SYM

    taida_ossl.loaded = 1;
    return 1;
}

static void taida_ossl_unload(void) {
    if (taida_ossl.libssl_handle) { dlclose(taida_ossl.libssl_handle); taida_ossl.libssl_handle = NULL; }
    if (taida_ossl.libcrypto_handle) { dlclose(taida_ossl.libcrypto_handle); taida_ossl.libcrypto_handle = NULL; }
    taida_ossl.loaded = 0;
}

// Create an SSL_CTX for TLS server with cert/key PEM files.
// Returns non-NULL on success. On failure, writes error to errbuf and returns NULL.
static OSSL_SSL_CTX *taida_tls_create_ctx(const char *cert_path, const char *key_path, char *errbuf, size_t errbuf_sz) {
    OSSL_SSL_CTX *ctx = taida_ossl.SSL_CTX_new(taida_ossl.TLS_server_method());
    if (!ctx) {
        snprintf(errbuf, errbuf_sz, "httpServe: failed to create SSL context");
        return NULL;
    }
    // Only allow TLS 1.2+.
    taida_ossl.SSL_CTX_set_options(ctx, TAIDA_SSL_OP_SECURE);

    if (taida_ossl.SSL_CTX_use_certificate_chain_file(ctx, cert_path) != 1) {
        snprintf(errbuf, errbuf_sz, "httpServe: failed to load cert file '%s'", cert_path);
        taida_ossl.SSL_CTX_free(ctx);
        return NULL;
    }
    if (taida_ossl.SSL_CTX_use_PrivateKey_file(ctx, key_path, TAIDA_SSL_FILETYPE_PEM) != 1) {
        snprintf(errbuf, errbuf_sz, "httpServe: failed to load key file '%s'", key_path);
        taida_ossl.SSL_CTX_free(ctx);
        return NULL;
    }
    if (taida_ossl.SSL_CTX_check_private_key(ctx) != 1) {
        snprintf(errbuf, errbuf_sz, "httpServe: cert/key mismatch for '%s' / '%s'", cert_path, key_path);
        taida_ossl.SSL_CTX_free(ctx);
        return NULL;
    }
    return ctx;
}

// Thread-local: current SSL connection pointer for TLS-aware I/O.
// NULL = plaintext (v4 path), non-NULL = TLS connection.
static __thread OSSL_SSL *tl_ssl = NULL;

// ── TLS-aware I/O wrappers ──────────────────────────────────────────
// These check tl_ssl and route through SSL_read/SSL_write when active.

// TLS-aware recv: reads from SSL or raw fd. Returns bytes read, or <=0 on error/EOF.
static ssize_t taida_tls_recv(int fd, void *buf, size_t len) {
    if (tl_ssl) {
        int n = taida_ossl.SSL_read(tl_ssl, buf, (int)(len > INT_MAX ? INT_MAX : len));
        if (n <= 0) {
            int err = taida_ossl.SSL_get_error(tl_ssl, n);
            if (err == TAIDA_SSL_ERROR_ZERO_RETURN) return 0; // clean TLS shutdown
            if (err == TAIDA_SSL_ERROR_WANT_READ || err == TAIDA_SSL_ERROR_WANT_WRITE) {
                errno = EAGAIN;
                return -1;
            }
            errno = EIO;
            return -1;
        }
        return (ssize_t)n;
    }
    return recv(fd, buf, len, 0);
}

// TLS-aware send_all: writes all bytes through SSL or raw fd.
// Returns 0 on success, -1 on error.
static int taida_tls_send_all(int fd, const void *buf, size_t len) {
    const unsigned char *p = (const unsigned char*)buf;
    size_t remaining = len;
    if (tl_ssl) {
        while (remaining > 0) {
            int chunk = (int)(remaining > INT_MAX ? INT_MAX : remaining);
            int n = taida_ossl.SSL_write(tl_ssl, p, chunk);
            if (n <= 0) return -1;
            p += n;
            remaining -= (size_t)n;
        }
        return 0;
    }
    // Plaintext path: delegate to existing send_all.
    while (remaining > 0) {
        ssize_t n = send(fd, p, remaining, MSG_NOSIGNAL);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) return -1;
        p += (size_t)n;
        remaining -= (size_t)n;
    }
    return 0;
}

// TLS-aware writev_all: writes all iov buffers through SSL or raw fd.
// For TLS, we linearize the iovecs since SSL_write doesn't support scatter/gather.
// Returns 0 on success, -1 on error.
static int taida_tls_writev_all(int fd, struct iovec *iov, int iovcnt) {
    if (tl_ssl) {
        // SSL doesn't support writev — send each iovec buffer via SSL_write.
        for (int i = 0; i < iovcnt; i++) {
            if (iov[i].iov_len > 0) {
                if (taida_tls_send_all(fd, iov[i].iov_base, iov[i].iov_len) != 0)
                    return -1;
            }
        }
        return 0;
    }
    // Plaintext: use real writev.
    while (iovcnt > 0) {
        ssize_t n = writev(fd, iov, iovcnt);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) return -1;
        size_t written = (size_t)n;
        while (iovcnt > 0 && written >= iov[0].iov_len) {
            written -= iov[0].iov_len;
            iov++;
            iovcnt--;
        }
        if (iovcnt > 0 && written > 0) {
            iov[0].iov_base = (char*)iov[0].iov_base + written;
            iov[0].iov_len -= written;
        }
    }
    return 0;
}

// TLS-aware recv_exact: reads exactly `count` bytes. Returns bytes actually read.
static size_t taida_tls_recv_exact(int fd, unsigned char *out, size_t count) {
    size_t pos = 0;
    while (pos < count) {
        ssize_t n = taida_tls_recv(fd, out + pos, count - pos);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            return pos;
        }
        pos += (size_t)n;
    }
    return pos;
}

// Perform TLS handshake on an accepted fd. Returns SSL* on success, NULL on failure.
static OSSL_SSL *taida_tls_handshake(OSSL_SSL_CTX *ctx, int fd) {
    OSSL_SSL *ssl = taida_ossl.SSL_new(ctx);
    if (!ssl) return NULL;
    if (taida_ossl.SSL_set_fd(ssl, fd) != 1) {
        taida_ossl.SSL_free(ssl);
        return NULL;
    }
    int ret = taida_ossl.SSL_accept(ssl);
    if (ret != 1) {
        // Handshake failed — connection close per NET5-0c policy.
        taida_ossl.SSL_free(ssl);
        return NULL;
    }
    return ssl;
}

// TLS shutdown + free.
static void taida_tls_shutdown_free(OSSL_SSL *ssl) {
    if (!ssl) return;
    taida_ossl.SSL_shutdown(ssl);
    taida_ossl.SSL_free(ssl);
}

// Helper: create a resolved Async[value] (fulfilled)
// NO-3: auto-detect value type for ownership tracking
static taida_val taida_async_resolved(taida_val value) {
    taida_val vtag = taida_detect_value_tag(value);
    return taida_async_ok_tagged(value, vtag);
}

// ── ReadAsync[path]() → Async[Lax[Str]] ──────────────────
// Synchronous implementation wrapped in Async (pthread spawn for true async is future work)
taida_val taida_os_read_async(taida_val path_ptr) {
    // Reuse the sync Read implementation, wrap in Async
    taida_val lax_result = taida_os_read(path_ptr);
    return taida_async_resolved(lax_result);
}

// ── HTTP helpers (minimal HTTP/1.1 over raw TCP) ─────────
// FNV-1a hashes: "status", "body", "headers"
#define TAIDA_HTTP_STATUS_HASH  0xc4d5696d6cc12c2fULL
#define TAIDA_HTTP_BODY_HASH    0xcd4de79bc6c93295ULL
#define TAIDA_HTTP_HEADERS_HASH 0x8cc1ca917bac9b49ULL

static taida_val taida_os_http_default_response(void) {
    taida_val result = taida_pack_new(3);
    taida_pack_set_hash(result, 0, (taida_val)TAIDA_HTTP_STATUS_HASH);
    taida_pack_set(result, 0, 0);
    taida_pack_set_hash(result, 1, (taida_val)TAIDA_HTTP_BODY_HASH);
    taida_pack_set(result, 1, (taida_val)"");
    taida_pack_set_hash(result, 2, (taida_val)TAIDA_HTTP_HEADERS_HASH);
    taida_pack_set(result, 2, taida_pack_new(0));
    return result;
}

static taida_val taida_os_http_failure_lax(void) {
    return taida_lax_empty(taida_os_http_default_response());
}

static char *taida_os_http_headers_to_lines(taida_val headers_ptr) {
    if (!headers_ptr || !taida_is_buchi_pack(headers_ptr)) {
        char *empty = (char*)TAIDA_MALLOC(1, "http_headers_empty");
        empty[0] = '\0';
        return empty;
    }

    taida_val *pack = (taida_val*)headers_ptr;
    taida_val fc = pack[1];
    size_t cap = 128;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "http_headers");
    buf[0] = '\0';

    for (taida_val i = 0; i < fc; i++) {
        taida_val field_hash = pack[2 + i * 3];
        taida_val field_val = pack[2 + i * 3 + 2];
        const char *name = taida_lookup_field_name(field_hash);
        if (!name || !name[0]) continue;

        taida_val value_str_ptr = taida_value_to_display_string(field_val);
        const char *value_str = (const char*)value_str_ptr;
        if (!value_str) value_str = "";

        /* RCB-304: Strip CR/LF from header name and value to prevent CRLF injection */
        char *safe_name = strdup(name);
        char *safe_value = strdup(value_str);
        for (char *p = safe_name; *p; p++) { if (*p == '\r' || *p == '\n') *p = ' '; }
        for (char *p = safe_value; *p; p++) { if (*p == '\r' || *p == '\n') *p = ' '; }

        size_t need = strlen(safe_name) + strlen(safe_value) + 4;
        while (len + need + 1 > cap) {
            cap *= 2;
            TAIDA_REALLOC(buf, cap, "http_response");
        }

        int n = snprintf(buf + len, cap - len, "%s: %s\r\n", safe_name, safe_value);
        free(safe_name);
        free(safe_value);
        taida_str_release(value_str_ptr);
        if (n < 0) {
            free(buf);
            char *empty = (char*)TAIDA_MALLOC(1, "http_headers_err");
            empty[0] = '\0';
            return empty;
        }
        len += (size_t)n;
    }
    return buf;
}

static taida_val taida_os_http_parse_headers(const char *header_start, const char *header_end) {
    if (!header_start || !header_end || header_end <= header_start) return taida_pack_new(0);

    const char *lines_start = strstr(header_start, "\r\n");
    if (!lines_start || lines_start >= header_end) return taida_pack_new(0);
    lines_start += 2; // skip status line

    size_t header_count = 0;
    const char *scan = lines_start;
    while (scan < header_end) {
        const char *line_end = strstr(scan, "\r\n");
        if (!line_end || line_end > header_end) line_end = header_end;
        const char *colon = memchr(scan, ':', (size_t)(line_end - scan));
        if (colon) header_count++;
        if (line_end >= header_end) break;
        scan = line_end + 2;
    }

    taida_val headers_pack = taida_pack_new((taida_val)header_count);
    taida_val idx = 0;
    scan = lines_start;
    while (scan < header_end && idx < (taida_val)header_count) {
        const char *line_end = strstr(scan, "\r\n");
        if (!line_end || line_end > header_end) line_end = header_end;
        const char *colon = memchr(scan, ':', (size_t)(line_end - scan));
        if (colon) {
            size_t key_len = (size_t)(colon - scan);
            char *key = (char*)TAIDA_MALLOC(key_len + 1, "http_header_key");
            for (size_t i = 0; i < key_len; i++) {
                char c = scan[i];
                if (c >= 'A' && c <= 'Z') c = (char)(c + 32);
                key[i] = c;
            }
            key[key_len] = '\0';

            const char *value_start = colon + 1;
            while (value_start < line_end && (*value_start == ' ' || *value_start == '\t')) value_start++;
            size_t value_len = (size_t)(line_end - value_start);
            char *value = (char*)TAIDA_MALLOC(value_len + 1, "http_header_value");
            memcpy(value, value_start, value_len);
            value[value_len] = '\0';

            taida_val key_hash = taida_str_hash((taida_val)key);
            taida_register_field_name(key_hash, (taida_val)key);
            taida_register_field_type(key_hash, (taida_val)key, 3);
            taida_pack_set_hash(headers_pack, idx, key_hash);
            char *value_str = taida_str_new_copy(value);
            free(value);
            taida_pack_set(headers_pack, idx, (taida_val)value_str);
            taida_pack_set_tag(headers_pack, idx, TAIDA_TAG_STR);
            idx++;
        }

        if (line_end >= header_end) break;
        scan = line_end + 2;
    }

    return headers_pack;
}

static int taida_os_cmd_append(char **buf, size_t *cap, size_t *len, const char *chunk) {
    if (!chunk) return 1;
    size_t n = strlen(chunk);
    if (*len + n + 1 > *cap) {
        size_t new_cap = *cap;
        while (*len + n + 1 > new_cap) new_cap *= 2;
        char *new_buf = (char*)realloc(*buf, new_cap);
        if (!new_buf) return 0;
        *buf = new_buf;
        *cap = new_cap;
    }
    memcpy(*buf + *len, chunk, n);
    *len += n;
    (*buf)[*len] = '\0';
    return 1;
}

static char *taida_os_shell_quote(const char *s) {
    if (!s) s = "";
    size_t out_len = 2; // surrounding single quotes
    for (const char *p = s; *p; p++) {
        out_len += (*p == '\'') ? 4 : 1; // '\'' sequence for single quote
    }

    char *out = (char*)malloc(out_len + 1);
    if (!out) return NULL;
    char *w = out;
    *w++ = '\'';
    for (const char *p = s; *p; p++) {
        if (*p == '\'') {
            memcpy(w, "'\\''", 4);
            w += 4;
        } else {
            *w++ = *p;
        }
    }
    *w++ = '\'';
    *w = '\0';
    return out;
}

static taida_val taida_os_http_do_curl(const char *method, const char *url, taida_val headers_ptr, const char *body) {
    const char *method_str = (method && *method) ? method : "GET";
    const char *url_str = url ? url : "";
    const char *body_str = body ? body : "";

    char *q_method = taida_os_shell_quote(method_str);
    char *q_url = taida_os_shell_quote(url_str);
    if (!q_method || !q_url) {
        free(q_method);
        free(q_url);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    size_t cmd_cap = 1024;
    size_t cmd_len = 0;
    char *cmd = (char*)malloc(cmd_cap);
    if (!cmd) {
        free(q_method);
        free(q_url);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    cmd[0] = '\0';

    // RCB-306: Limit response size for HTTPS (curl) path — 100MB matches raw HTTP limit
    if (!taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, "curl -sS -i --max-time 30 --max-filesize 104857600 -X ")
        || !taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_method)
        || !taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, " ")
        || !taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_url)) {
        free(cmd);
        free(q_method);
        free(q_url);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    free(q_method);
    free(q_url);

    if (headers_ptr && taida_is_buchi_pack(headers_ptr)) {
        taida_val *pack = (taida_val*)headers_ptr;
        taida_val fc = pack[1];
        for (taida_val i = 0; i < fc; i++) {
            taida_val field_hash = pack[2 + i * 3];
            taida_val field_val = pack[2 + i * 3 + 2];
            const char *name = taida_lookup_field_name(field_hash);
            if (!name || !name[0]) continue;

            taida_val value_str_ptr = taida_value_to_display_string(field_val);
            const char *value_str = (const char*)value_str_ptr;
            if (!value_str) value_str = "";

            size_t hv_len = strlen(name) + strlen(value_str) + 2;
            char *header_pair = (char*)malloc(hv_len + 1);
            if (!header_pair) {
                taida_str_release(value_str_ptr);
                free(cmd);
                return taida_async_resolved(taida_os_http_failure_lax());
            }
            snprintf(header_pair, hv_len + 1, "%s: %s", name, value_str);
            taida_str_release(value_str_ptr);

            char *q_header = taida_os_shell_quote(header_pair);
            free(header_pair);
            if (!q_header) {
                free(cmd);
                return taida_async_resolved(taida_os_http_failure_lax());
            }

            int ok = taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, " -H ")
                && taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_header);
            free(q_header);
            if (!ok) {
                free(cmd);
                return taida_async_resolved(taida_os_http_failure_lax());
            }
        }
    }

    if (body_str[0] != '\0') {
        char *q_body = taida_os_shell_quote(body_str);
        if (!q_body) {
            free(cmd);
            return taida_async_resolved(taida_os_http_failure_lax());
        }
        int ok = taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, " --data-raw ")
            && taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_body);
        free(q_body);
        if (!ok) {
            free(cmd);
            return taida_async_resolved(taida_os_http_failure_lax());
        }
    }

    FILE *fp = popen(cmd, "r");
    free(cmd);
    if (!fp) return taida_async_resolved(taida_os_http_failure_lax());

    size_t resp_cap = 65536;
    size_t resp_len = 0;
    char *resp_buf = (char*)malloc(resp_cap);
    if (!resp_buf) {
        pclose(fp);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    size_t n;
    while ((n = fread(resp_buf + resp_len, 1, resp_cap - resp_len - 1, fp)) > 0) {
        resp_len += n;
        if (resp_len >= resp_cap - 1) {
            resp_cap *= 2;
            char *new_buf = (char*)realloc(resp_buf, resp_cap);
            if (!new_buf) {
                free(resp_buf);
                pclose(fp);
                return taida_async_resolved(taida_os_http_failure_lax());
            }
            resp_buf = new_buf;
        }
    }
    resp_buf[resp_len] = '\0';

    int status = pclose(fp);
    if (status == -1 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    char *header_end = strstr(resp_buf, "\r\n\r\n");
    if (!header_end) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    int status_code = 0;
    if (resp_len > 12 && resp_buf[0] == 'H') {
        char *sp = strchr(resp_buf, ' ');
        if (sp) status_code = atoi(sp + 1);
    }

    char *resp_body = header_end + 4;
    size_t resp_body_len = resp_len - (size_t)(resp_body - resp_buf);
    char *body_copy = (char*)malloc(resp_body_len + 1);
    if (!body_copy) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    memcpy(body_copy, resp_body, resp_body_len);
    body_copy[resp_body_len] = '\0';

    taida_val headers_pack = taida_os_http_parse_headers(resp_buf, header_end);

    taida_val result = taida_pack_new(3);
    taida_pack_set_hash(result, 0, (taida_val)TAIDA_HTTP_STATUS_HASH);
    taida_pack_set(result, 0, (taida_val)status_code);
    taida_pack_set_hash(result, 1, (taida_val)TAIDA_HTTP_BODY_HASH);
    taida_pack_set(result, 1, (taida_val)body_copy);
    taida_pack_set_hash(result, 2, (taida_val)TAIDA_HTTP_HEADERS_HASH);
    taida_pack_set(result, 2, headers_pack);

    free(resp_buf);
    return taida_async_resolved(taida_lax_new(result, taida_os_http_default_response()));
}

static taida_val taida_os_http_do(const char *method, const char *url, taida_val headers_ptr, const char *body) {
    if (!url) return taida_async_resolved(taida_os_http_failure_lax());

    const char *scheme_end = strstr(url, "://");
    int use_tls = 0;
    const char *host_start;
    if (scheme_end) {
        if (strncmp(url, "https", 5) == 0) use_tls = 1;
        host_start = scheme_end + 3;
    } else {
        host_start = url;
    }

    // HTTPS: route via curl TLS transport.
    if (use_tls) return taida_os_http_do_curl(method, url, headers_ptr, body);

    char host_buf[256] = {0};
    int port = 80;
    const char *path = "/";

    const char *slash = strchr(host_start, '/');
    const char *colon = strchr(host_start, ':');
    size_t host_len;

    if (slash) {
        path = slash;
        if (colon && colon < slash) {
            host_len = (size_t)(colon - host_start);
            port = atoi(colon + 1);
        } else {
            host_len = (size_t)(slash - host_start);
        }
    } else {
        if (colon) {
            host_len = (size_t)(colon - host_start);
            port = atoi(colon + 1);
        } else {
            host_len = strlen(host_start);
        }
    }

    if (host_len >= sizeof(host_buf)) host_len = sizeof(host_buf) - 1;
    memcpy(host_buf, host_start, host_len);
    host_buf[host_len] = '\0';

    // RCB-304: Reject URLs with CR/LF in host or path to prevent CRLF injection
    if (strchr(host_buf, '\r') || strchr(host_buf, '\n') ||
        strchr(path, '\r') || strchr(path, '\n')) {
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%d", port);
    if (getaddrinfo(host_buf, port_str, &hints, &res) != 0 || !res) {
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    int sockfd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
    if (sockfd < 0) {
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    if (connect(sockfd, res->ai_addr, res->ai_addrlen) < 0) {
        close(sockfd);
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    freeaddrinfo(res);

    char *header_lines = taida_os_http_headers_to_lines(headers_ptr);
    const char *method_str = (method && *method) ? method : "GET";
    const char *body_str = body ? body : "";
    size_t body_len = strlen(body_str);
    size_t header_lines_len = strlen(header_lines);
    size_t req_cap = strlen(method_str) + strlen(path) + strlen(host_buf) + header_lines_len + body_len + 256;
    char *request = (char*)TAIDA_MALLOC(req_cap, "http_request");

    int req_len;
    if (body_len > 0) {
        req_len = snprintf(
            request, req_cap,
            "%s %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\nContent-Length: %zu\r\nContent-Type: text/plain\r\n%s\r\n%s",
            method_str, path, host_buf, body_len, header_lines, body_str
        );
    } else {
        req_len = snprintf(
            request, req_cap,
            "%s %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n%s\r\n",
            method_str, path, host_buf, header_lines
        );
    }
    free(header_lines);

    if (req_len < 0 || (size_t)req_len >= req_cap) {
        free(request);
        close(sockfd);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    size_t sent_total = 0;
    while (sent_total < (size_t)req_len) {
        ssize_t sent = send(sockfd, request + sent_total, (size_t)req_len - sent_total, MSG_NOSIGNAL);
        if (sent <= 0) {
            free(request);
            close(sockfd);
            return taida_async_resolved(taida_os_http_failure_lax());
        }
        sent_total += (size_t)sent;
    }
    free(request);

    /* RCB-306: Limit HTTP response to 100 MB to prevent OOM */
    const size_t MAX_HTTP_RESPONSE = 100 * 1024 * 1024;
    size_t buf_cap = 65536;
    char *resp_buf = (char*)TAIDA_MALLOC(buf_cap, "http_recv");
    size_t resp_len = 0;
    ssize_t n;
    while ((n = recv(sockfd, resp_buf + resp_len, buf_cap - resp_len - 1, 0)) > 0) {
        resp_len += (size_t)n;
        if (resp_len > MAX_HTTP_RESPONSE) {
            close(sockfd);
            free(resp_buf);
            return taida_async_resolved(taida_os_http_failure_lax());
        }
        if (resp_len >= buf_cap - 1) {
            buf_cap *= 2;
            if (buf_cap > MAX_HTTP_RESPONSE + 1) buf_cap = MAX_HTTP_RESPONSE + 1;
            TAIDA_REALLOC(resp_buf, buf_cap, "tcp_recv");
        }
    }
    close(sockfd);
    resp_buf[resp_len] = '\0';

    char *header_end = strstr(resp_buf, "\r\n\r\n");
    if (!header_end) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    int status_code = 0;
    if (resp_len > 12 && resp_buf[0] == 'H') {
        char *sp = strchr(resp_buf, ' ');
        if (sp) status_code = atoi(sp + 1);
    }

    char *resp_body = header_end + 4;
    size_t resp_body_len = resp_len - (size_t)(resp_body - resp_buf);
    char *body_copy = (char*)TAIDA_MALLOC(resp_body_len + 1, "http_body");
    memcpy(body_copy, resp_body, resp_body_len);
    body_copy[resp_body_len] = '\0';

    taida_val headers_pack = taida_os_http_parse_headers(resp_buf, header_end);

    taida_val result = taida_pack_new(3);
    taida_pack_set_hash(result, 0, (taida_val)TAIDA_HTTP_STATUS_HASH);
    taida_pack_set(result, 0, (taida_val)status_code);
    taida_pack_set_hash(result, 1, (taida_val)TAIDA_HTTP_BODY_HASH);
    taida_pack_set(result, 1, (taida_val)body_copy);
    taida_pack_set_hash(result, 2, (taida_val)TAIDA_HTTP_HEADERS_HASH);
    taida_pack_set(result, 2, headers_pack);

    free(resp_buf);
    return taida_async_resolved(taida_lax_new(result, taida_os_http_default_response()));
}

taida_val taida_os_http_get(taida_val url_ptr) {
    return taida_os_http_do("GET", (const char*)url_ptr, 0, NULL);
}

taida_val taida_os_http_post(taida_val url_ptr, taida_val body_ptr) {
    return taida_os_http_do("POST", (const char*)url_ptr, 0, (const char*)body_ptr);
}

taida_val taida_os_http_request(taida_val method_ptr, taida_val url_ptr, taida_val headers_ptr, taida_val body_ptr) {
    const char *method = (const char*)method_ptr;
    if (!method || !*method) method = "GET";
    return taida_os_http_do(method, (const char*)url_ptr, headers_ptr, (const char*)body_ptr);
}

// ── TCP socket APIs ───────────────────────────────────────

static taida_val taida_os_network_timeout_ms(taida_val timeout_ms) {
    if (timeout_ms <= 0 || timeout_ms > 600000) return 30000;
    return timeout_ms;
}

static void taida_os_apply_socket_timeout(int fd, taida_val timeout_ms) {
    taida_val ms = taida_os_network_timeout_ms(timeout_ms);
    struct timeval tv;
    tv.tv_sec = (time_t)(ms / 1000);
    tv.tv_usec = (suseconds_t)((ms % 1000) * 1000);
    (void)setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    (void)setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
}

static taida_val taida_os_dns_failure(const char *op_name, int gai_code) {
    char msg[256];
    if (gai_code != 0) {
        snprintf(msg, sizeof(msg), "%s: %s", op_name, gai_strerror(gai_code));
    } else {
        snprintf(msg, sizeof(msg), "%s: DNS resolution failed", op_name);
    }
    return taida_async_resolved(taida_os_result_failure(EINVAL, msg));
}

taida_val taida_os_dns_resolve(taida_val host_ptr, taida_val timeout_ms) {
    (void)timeout_ms; // getaddrinfo timeout is not configurable per-call in this runtime path.

    const char *host = (const char*)host_ptr;
    if (!host || !host[0]) {
        return taida_async_resolved(taida_os_result_failure(EINVAL, "dnsResolve: invalid host"));
    }

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    int gai = getaddrinfo(host, NULL, &hints, &res);
    if (gai != 0 || !res) {
        return taida_os_dns_failure("dnsResolve", gai);
    }

    taida_val addresses = taida_list_new();
    for (struct addrinfo *it = res; it; it = it->ai_next) {
        char ip_buf[INET6_ADDRSTRLEN] = {0};
        const char *ip = NULL;

        if (it->ai_family == AF_INET) {
            struct sockaddr_in *addr4 = (struct sockaddr_in*)it->ai_addr;
            ip = inet_ntop(AF_INET, &addr4->sin_addr, ip_buf, sizeof(ip_buf));
        } else if (it->ai_family == AF_INET6) {
            struct sockaddr_in6 *addr6 = (struct sockaddr_in6*)it->ai_addr;
            ip = inet_ntop(AF_INET6, &addr6->sin6_addr, ip_buf, sizeof(ip_buf));
        }

        if (!ip || !ip[0]) continue;

        int exists = 0;
        taida_val len = taida_list_length(addresses);
        taida_val *list_ptr = (taida_val*)addresses;
        for (taida_val i = 0; i < len; i++) {
            const char *prev = (const char*)list_ptr[4 + i];
            if (prev && strcmp(prev, ip) == 0) {
                exists = 1;
                break;
            }
        }
        if (exists) continue;

        char *copy = taida_str_new_copy(ip);
        addresses = taida_list_push(addresses, (taida_val)copy);
    }
    freeaddrinfo(res);

    if (taida_list_length(addresses) <= 0) {
        return taida_os_dns_failure("dnsResolve", 0);
    }

    taida_val inner = taida_pack_new(1);
    taida_val addresses_hash = taida_str_hash((taida_val)"addresses");
    taida_pack_set_hash(inner, 0, addresses_hash);
    taida_pack_set(inner, 0, addresses);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_tcp_connect(taida_val host_ptr, taida_val port, taida_val timeout_ms) {
    const char *host = (const char*)host_ptr;
    if (!host) return taida_async_resolved(taida_os_result_failure(EINVAL, "tcpConnect: invalid host"));

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%" PRId64 "", port);
    int gai = getaddrinfo(host, port_str, &hints, &res);
    if (gai != 0 || !res) {
        return taida_os_dns_failure("tcpConnect", gai);
    }

    int sockfd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
    if (sockfd < 0) {
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(sockfd, timeout_ms);
    if (connect(sockfd, res->ai_addr, res->ai_addrlen) < 0) {
        close(sockfd);
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }
    freeaddrinfo(res);

    // Return @(socket: fd, host: host, port: port)
    taida_val inner = taida_pack_new(3);
    taida_val socket_hash = 0x10f2dcb841372d0cULL;
    taida_pack_set_hash(inner, 0, (taida_val)socket_hash);
    taida_pack_set(inner, 0, (taida_val)sockfd);
    taida_val host_hash = 0x4077f8cc7eaf4d6fULL;
    taida_pack_set_hash(inner, 1, (taida_val)host_hash);
    char *host_copy = taida_str_new_copy(host);
    taida_pack_set(inner, 1, (taida_val)host_copy);
    taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    taida_val port_hash = 0x8c2cdb0da8933fa6ULL;
    taida_pack_set_hash(inner, 2, (taida_val)port_hash);
    taida_pack_set(inner, 2, port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_tcp_listen(taida_val port, taida_val timeout_ms) {
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(sockfd, timeout_ms);

    int opt = 1;
    setsockopt(sockfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    /* RCB-305: Default to loopback (127.0.0.1) instead of all interfaces */
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    addr.sin_port = htons((unsigned short)port);

    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(sockfd);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    if (listen(sockfd, 128) < 0) {
        close(sockfd);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(2);
    taida_val listener_hash = 0x5a2d194b8a8ae591ULL;
    taida_pack_set_hash(inner, 0, (taida_val)listener_hash);
    taida_pack_set(inner, 0, (taida_val)sockfd);
    taida_val port_hash = 0x8c2cdb0da8933fa6ULL;
    taida_pack_set_hash(inner, 1, (taida_val)port_hash);
    taida_pack_set(inner, 1, port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_tcp_accept(taida_val listener_fd, taida_val timeout_ms) {
    struct sockaddr_in peer_addr;
    socklen_t peer_len = sizeof(peer_addr);
    taida_os_apply_socket_timeout((int)listener_fd, timeout_ms);
    int client_fd = accept((int)listener_fd, (struct sockaddr*)&peer_addr, &peer_len);
    if (client_fd < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(client_fd, timeout_ms);

    char host_buf[INET_ADDRSTRLEN] = {0};
    const char *peer_host = inet_ntop(AF_INET, &peer_addr.sin_addr, host_buf, sizeof(host_buf));
    if (!peer_host) peer_host = "";
    taida_val peer_port = (taida_val)ntohs(peer_addr.sin_port);

    taida_val inner = taida_pack_new(3);
    taida_val socket_hash = taida_str_hash((taida_val)"socket");
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_pack_set_hash(inner, 0, socket_hash);
    taida_pack_set(inner, 0, (taida_val)client_fd);
    taida_pack_set_hash(inner, 1, host_hash);
    taida_pack_set(inner, 1, (taida_val)taida_str_new_copy(peer_host));
    taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 2, port_hash);
    taida_pack_set(inner, 2, peer_port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_socket_send(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms) {
    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(data_ptr)) {
        taida_val *bytes = (taida_val*)data_ptr;
        taida_val len = bytes[1];
        if (len < 0) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSend: invalid data"));
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSend: payload too large"));
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "socketSend_bytes");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *data = (const char*)data_ptr;
        size_t data_len = 0;
        if (!taida_read_cstr_len_safe(data, 65536, &data_len)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSend: invalid data"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(data_len, "socketSend_str");
        memcpy(payload_buf, data, data_len);
        payload_len = data_len;
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t sent = send((int)socket_fd, payload_buf, payload_len, MSG_NOSIGNAL);
    free(payload_buf);
    if (sent < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = 0x08b05d07b5566befULL;
    taida_pack_set_hash(inner, 0, (taida_val)ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_val bytes_hash = 0x67ec7cd6a574048aULL;
    taida_pack_set_hash(inner, 1, (taida_val)bytes_hash);
    taida_pack_set(inner, 1, sent);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_socket_send_all(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms) {
    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(data_ptr)) {
        taida_val *bytes = (taida_val*)data_ptr;
        taida_val len = bytes[1];
        if (len < 0) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSendAll: invalid data"));
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSendAll: payload too large"));
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "socketSendAll_bytes");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *data = (const char*)data_ptr;
        size_t data_len = 0;
        if (!taida_read_cstr_len_safe(data, 65536, &data_len)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSendAll: invalid data"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(data_len, "socketSendAll_payload");
        memcpy(payload_buf, data, data_len);
        payload_len = data_len;
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    size_t sent_total = 0;
    while (sent_total < payload_len) {
        ssize_t sent = send((int)socket_fd, payload_buf + sent_total, payload_len - sent_total, MSG_NOSIGNAL);
        if (sent < 0) {
            if (errno == EINTR) continue;
            free(payload_buf);
            return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
        }
        if (sent == 0) {
            free(payload_buf);
            return taida_async_resolved(
                taida_os_result_failure(EPIPE, "socketSendAll: peer closed while sending")
            );
        }
        sent_total += (size_t)sent;
    }
    free(payload_buf);

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val bytes_hash = taida_str_hash((taida_val)"bytesSent");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_pack_set_hash(inner, 1, bytes_hash);
    taida_pack_set(inner, 1, (taida_val)sent_total);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_socket_recv(taida_val socket_fd, taida_val timeout_ms) {
    char buf[65536];
    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t n = recv((int)socket_fd, buf, sizeof(buf) - 1, 0);
    if (n <= 0) {
        return taida_async_resolved(taida_lax_empty((taida_val)""));
    }
    buf[n] = '\0';
    char *result = taida_str_new_copy(buf);
    return taida_async_resolved(taida_lax_new((taida_val)result, (taida_val)""));
}

taida_val taida_os_socket_send_bytes(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms) {
    return taida_os_socket_send(socket_fd, data_ptr, timeout_ms);
}

taida_val taida_os_socket_recv_bytes(taida_val socket_fd, taida_val timeout_ms) {
    unsigned char buf[65536];
    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t n = recv((int)socket_fd, buf, sizeof(buf), 0);
    if (n <= 0) {
        return taida_async_resolved(taida_lax_empty(taida_bytes_default_value()));
    }
    taida_val bytes = taida_bytes_from_raw(buf, (taida_val)n);
    return taida_async_resolved(taida_lax_new(bytes, taida_bytes_default_value()));
}

taida_val taida_os_socket_recv_exact(taida_val socket_fd, taida_val size, taida_val timeout_ms) {
    if (size < 0) {
        return taida_async_resolved(taida_lax_empty(taida_bytes_default_value()));
    }
    if (size == 0) {
        taida_val empty = taida_bytes_default_value();
        return taida_async_resolved(taida_lax_new(empty, empty));
    }
    // M-11: Cap recv size to 256MB to prevent unbounded malloc from user input.
    if (size > (taida_val)(256 * 1024 * 1024)) {
        return taida_async_resolved(taida_lax_empty(taida_bytes_default_value()));
    }

    unsigned char *buf = (unsigned char*)malloc((size_t)size);
    if (!buf) {
        return taida_async_resolved(taida_lax_empty(taida_bytes_default_value()));
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    size_t total = 0;
    while (total < (size_t)size) {
        ssize_t n = recv((int)socket_fd, buf + total, (size_t)size - total, 0);
        if (n == 0) {
            free(buf);
            return taida_async_resolved(taida_lax_empty(taida_bytes_default_value()));
        }
        if (n < 0) {
            if (errno == EINTR) continue;
            free(buf);
            return taida_async_resolved(taida_lax_empty(taida_bytes_default_value()));
        }
        total += (size_t)n;
    }

    taida_val bytes = taida_bytes_from_raw(buf, size);
    free(buf);
    return taida_async_resolved(taida_lax_new(bytes, taida_bytes_default_value()));
}

static taida_val taida_os_udp_default_payload(void) {
    taida_val payload = taida_pack_new(4);
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_val data_hash = taida_str_hash((taida_val)"data");
    taida_val truncated_hash = taida_str_hash((taida_val)"truncated");

    taida_pack_set_hash(payload, 0, host_hash);
    taida_pack_set(payload, 0, (taida_val)taida_str_new_copy(""));
    taida_pack_set_hash(payload, 1, port_hash);
    taida_pack_set(payload, 1, 0);
    taida_pack_set_hash(payload, 2, data_hash);
    taida_pack_set(payload, 2, taida_bytes_default_value());
    taida_pack_set_hash(payload, 3, truncated_hash);
    taida_pack_set(payload, 3, 0);
    return payload;
}

taida_val taida_os_udp_bind(taida_val host_ptr, taida_val port, taida_val timeout_ms) {
    const char *host = (const char*)host_ptr;
    if (!host) return taida_async_resolved(taida_os_result_failure(EINVAL, "udpBind: invalid host"));

    int sockfd = socket(AF_INET, SOCK_DGRAM, 0);
    if (sockfd < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(sockfd, timeout_ms);

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((unsigned short)port);

    if (host[0] == '\0' || strcmp(host, "0.0.0.0") == 0) {
        addr.sin_addr.s_addr = INADDR_ANY;
    } else if (inet_pton(AF_INET, host, &addr.sin_addr) != 1) {
        // Allow hostnames like "localhost" by resolving via DNS.
        struct addrinfo hints = {0}, *res = NULL;
        hints.ai_family = AF_INET;
        hints.ai_socktype = SOCK_DGRAM;
        int gai = getaddrinfo(host, NULL, &hints, &res);
        if (gai != 0 || !res) {
            close(sockfd);
            return taida_os_dns_failure("udpBind", gai);
        }
        struct sockaddr_in *resolved = (struct sockaddr_in*)res->ai_addr;
        addr.sin_addr = resolved->sin_addr;
        freeaddrinfo(res);
    }

    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(sockfd);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(3);
    taida_val socket_hash = taida_str_hash((taida_val)"socket");
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_pack_set_hash(inner, 0, socket_hash);
    taida_pack_set(inner, 0, (taida_val)sockfd);
    taida_pack_set_hash(inner, 1, host_hash);
    taida_pack_set(inner, 1, (taida_val)taida_str_new_copy(host));
    taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 2, port_hash);
    taida_pack_set(inner, 2, port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_udp_send_to(taida_val socket_fd, taida_val host_ptr, taida_val port, taida_val data_ptr, taida_val timeout_ms) {
    const char *host = (const char*)host_ptr;
    if (!host) {
        return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: invalid arguments"));
    }

    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(data_ptr)) {
        taida_val *bytes = (taida_val*)data_ptr;
        taida_val len = bytes[1];
        if (len < 0) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: invalid bytes payload"));
        }
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: payload too large"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "udpSendTo_bytes");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *data = (const char*)data_ptr;
        size_t data_len = 0;
        if (!taida_read_cstr_len_safe(data, 65536, &data_len)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: invalid data"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(data_len, "socketSend_payload");
        memcpy(payload_buf, data, data_len);
        payload_len = data_len;
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_DGRAM;
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%" PRId64 "", port);
    int gai = getaddrinfo(host, port_str, &hints, &res);
    if (gai != 0 || !res) {
        free(payload_buf);
        return taida_os_dns_failure("udpSendTo", gai);
    }

    ssize_t sent = sendto((int)socket_fd, payload_buf, payload_len, 0, res->ai_addr, res->ai_addrlen);
    freeaddrinfo(res);
    free(payload_buf);
    if (sent < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val bytes_hash = taida_str_hash((taida_val)"bytesSent");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_pack_set_hash(inner, 1, bytes_hash);
    taida_pack_set(inner, 1, sent);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_udp_recv_from(taida_val socket_fd, taida_val timeout_ms) {
    unsigned char buf[65508];
    struct sockaddr_in from_addr;
    socklen_t from_len = sizeof(from_addr);

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t n = recvfrom((int)socket_fd, buf, sizeof(buf), MSG_TRUNC, (struct sockaddr*)&from_addr, &from_len);
    if (n < 0) {
        return taida_async_resolved(taida_lax_empty(taida_os_udp_default_payload()));
    }
    taida_val copy_len = (taida_val)n;
    taida_val truncated = 0;
    if ((size_t)n > sizeof(buf)) {
        copy_len = (taida_val)sizeof(buf);
        truncated = 1;
    }

    char host_buf[INET_ADDRSTRLEN] = {0};
    const char *host = inet_ntop(AF_INET, &from_addr.sin_addr, host_buf, sizeof(host_buf));
    if (!host) host = "";
    taida_val peer_port = (taida_val)ntohs(from_addr.sin_port);

    taida_val payload = taida_pack_new(4);
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_val data_hash = taida_str_hash((taida_val)"data");
    taida_val truncated_hash = taida_str_hash((taida_val)"truncated");
    taida_pack_set_hash(payload, 0, host_hash);
    taida_pack_set(payload, 0, (taida_val)taida_str_new_copy(host));
    taida_pack_set_tag(payload, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(payload, 1, port_hash);
    taida_pack_set(payload, 1, peer_port);
    taida_pack_set_hash(payload, 2, data_hash);
    taida_pack_set(payload, 2, taida_bytes_from_raw(buf, copy_len));
    taida_pack_set_hash(payload, 3, truncated_hash);
    taida_pack_set(payload, 3, truncated);

    return taida_async_resolved(taida_lax_new(payload, taida_os_udp_default_payload()));
}

taida_val taida_os_socket_close(taida_val socket_fd) {
    if (close((int)socket_fd) < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }
    return taida_async_resolved(taida_os_result_success(taida_os_ok_inner()));
}

taida_val taida_os_listener_close(taida_val listener_fd) {
    if (close((int)listener_fd) < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }
    return taida_async_resolved(taida_os_result_success(taida_os_ok_inner()));
}

// ── taida-lang/pool package runtime ──────────────────────

#define TAIDA_POOL_MAX_STATES 4096

typedef struct {
    int open;
    taida_val max_size;
    taida_val max_idle;
    taida_val acquire_timeout_ms;
    taida_val next_token;
    size_t idle_len;
    size_t idle_cap;
    taida_val *idle_tokens;
    taida_val *idle_resources;
    size_t in_use_len;
    size_t in_use_cap;
    taida_val *in_use_tokens;
} taida_pool_state;

static taida_pool_state *taida_pool_states[TAIDA_POOL_MAX_STATES] = {0};
static taida_val taida_pool_next_id = 1;

static taida_val taida_pool_parse_handle(taida_val pool_or_pack) {
    taida_val pool_hash = taida_str_hash((taida_val)"pool");
    if (taida_is_buchi_pack(pool_or_pack) && taida_pack_has_hash(pool_or_pack, pool_hash)) {
        return taida_pack_get(pool_or_pack, pool_hash);
    }
    return pool_or_pack;
}

static taida_val taida_pool_io_error(const char *kind, const char *msg) {
    const char *message = msg ? msg : "pool error";
    const char *k = kind ? kind : "other";
    taida_val error = taida_pack_new(4);
    taida_pack_set_hash(error, 0, (taida_val)HASH_TYPE);
    taida_pack_set(error, 0, (taida_val)taida_str_new_copy("IoError"));
    taida_pack_set_tag(error, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(error, 1, (taida_val)HASH_MESSAGE);
    taida_pack_set(error, 1, (taida_val)taida_str_new_copy(message));
    taida_pack_set_tag(error, 1, TAIDA_TAG_STR);
    taida_val code_hash = taida_str_hash((taida_val)"code");
    taida_pack_set_hash(error, 2, code_hash);
    taida_pack_set(error, 2, -1);
    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(error, 3, kind_hash);
    taida_pack_set(error, 3, (taida_val)taida_str_new_copy(k));
    taida_pack_set_tag(error, 3, TAIDA_TAG_STR);
    return error;
}

static taida_val taida_pool_result_failure(const char *kind, const char *msg) {
    const char *message = msg ? msg : "pool error";
    const char *k = kind ? kind : "other";
    taida_val inner = taida_pack_new(4);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val code_hash = taida_str_hash((taida_val)"code");
    taida_val msg_hash = taida_str_hash((taida_val)"message");
    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 0);
    taida_pack_set_hash(inner, 1, code_hash);
    taida_pack_set(inner, 1, -1);
    taida_pack_set_hash(inner, 2, msg_hash);
    taida_pack_set(inner, 2, (taida_val)taida_str_new_copy(message));
    taida_pack_set_tag(inner, 2, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 3, kind_hash);
    taida_pack_set(inner, 3, (taida_val)taida_str_new_copy(k));
    taida_pack_set_tag(inner, 3, TAIDA_TAG_STR);
    return taida_result_create(inner, taida_pool_io_error(k, message), 0);
}

static int taida_pool_push_idle(taida_pool_state *st, taida_val token, taida_val resource) {
    if (st->idle_len >= st->idle_cap) {
        size_t new_cap = st->idle_cap == 0 ? 4 : st->idle_cap * 2;
        taida_val *new_tokens = (taida_val*)realloc(st->idle_tokens, sizeof(taida_val) * new_cap);
        taida_val *new_resources = (taida_val*)realloc(st->idle_resources, sizeof(taida_val) * new_cap);
        if (!new_tokens || !new_resources) {
            if (new_tokens) st->idle_tokens = new_tokens;
            if (new_resources) st->idle_resources = new_resources;
            return 0;
        }
        st->idle_tokens = new_tokens;
        st->idle_resources = new_resources;
        st->idle_cap = new_cap;
    }
    st->idle_tokens[st->idle_len] = token;
    st->idle_resources[st->idle_len] = resource;
    st->idle_len++;
    return 1;
}

static int taida_pool_push_in_use(taida_pool_state *st, taida_val token) {
    if (st->in_use_len >= st->in_use_cap) {
        size_t new_cap = st->in_use_cap == 0 ? 4 : st->in_use_cap * 2;
        taida_val *new_tokens = (taida_val*)realloc(st->in_use_tokens, sizeof(taida_val) * new_cap);
        if (!new_tokens) return 0;
        st->in_use_tokens = new_tokens;
        st->in_use_cap = new_cap;
    }
    st->in_use_tokens[st->in_use_len++] = token;
    return 1;
}

static taida_val taida_pool_find_in_use_idx(taida_pool_state *st, taida_val token) {
    for (size_t i = 0; i < st->in_use_len; i++) {
        if (st->in_use_tokens[i] == token) return (taida_val)i;
    }
    return -1;
}

static taida_val taida_pool_health_pack(taida_val open, taida_val idle, taida_val in_use, taida_val waiting) {
    taida_val pack = taida_pack_new(4);
    taida_val open_hash = taida_str_hash((taida_val)"open");
    taida_val idle_hash = taida_str_hash((taida_val)"idle");
    taida_val in_use_hash = taida_str_hash((taida_val)"inUse");
    taida_val waiting_hash = taida_str_hash((taida_val)"waiting");
    taida_pack_set_hash(pack, 0, open_hash);
    taida_pack_set(pack, 0, open ? 1 : 0);
    taida_pack_set_hash(pack, 1, idle_hash);
    taida_pack_set(pack, 1, idle);
    taida_pack_set_hash(pack, 2, in_use_hash);
    taida_pack_set(pack, 2, in_use);
    taida_pack_set_hash(pack, 3, waiting_hash);
    taida_pack_set(pack, 3, waiting);
    return pack;
}

taida_val taida_pool_create(taida_val config_ptr) {
    if (!taida_is_buchi_pack(config_ptr)) {
        return taida_pool_result_failure("invalid", "poolCreate: config must be a pack");
    }

    taida_val max_size = 10;
    taida_val max_idle = 10;
    taida_val acquire_timeout_ms = 30000;
    taida_val max_size_hash = taida_str_hash((taida_val)"maxSize");
    taida_val max_idle_hash = taida_str_hash((taida_val)"maxIdle");
    taida_val timeout_hash = taida_str_hash((taida_val)"acquireTimeoutMs");

    if (taida_pack_has_hash(config_ptr, max_size_hash)) {
        max_size = taida_pack_get(config_ptr, max_size_hash);
    }
    if (taida_pack_has_hash(config_ptr, max_idle_hash)) {
        max_idle = taida_pack_get(config_ptr, max_idle_hash);
    }
    if (taida_pack_has_hash(config_ptr, timeout_hash)) {
        acquire_timeout_ms = taida_pack_get(config_ptr, timeout_hash);
    }

    if (max_size <= 0) {
        return taida_pool_result_failure("invalid", "poolCreate: maxSize must be > 0");
    }
    if (max_idle < 0) {
        return taida_pool_result_failure("invalid", "poolCreate: maxIdle must be >= 0");
    }
    if (max_idle > max_size) max_idle = max_size;
    if (acquire_timeout_ms <= 0) {
        return taida_pool_result_failure("invalid", "poolCreate: acquireTimeoutMs must be > 0");
    }

    taida_val pool_id = taida_pool_next_id++;
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES) {
        return taida_pool_result_failure("other", "poolCreate: pool table exhausted");
    }

    taida_pool_state *st = (taida_pool_state*)calloc(1, sizeof(taida_pool_state));
    if (!st) return taida_pool_result_failure("other", "poolCreate: out of memory");
    st->open = 1;
    st->max_size = max_size;
    st->max_idle = max_idle;
    st->acquire_timeout_ms = acquire_timeout_ms;
    st->next_token = 1;
    taida_pool_states[pool_id] = st;

    taida_val inner = taida_pack_new(1);
    taida_val pool_hash = taida_str_hash((taida_val)"pool");
    taida_pack_set_hash(inner, 0, pool_hash);
    taida_pack_set(inner, 0, pool_id);
    return taida_os_result_success(inner);
}

taida_val taida_pool_acquire(taida_val pool_or_pack, taida_val timeout_ms) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        return taida_async_resolved(taida_pool_result_failure("invalid", "poolAcquire: unknown pool handle"));
    }

    taida_pool_state *st = taida_pool_states[pool_id];
    if (!st->open) {
        return taida_async_resolved(taida_pool_result_failure("closed", "poolAcquire: pool is closed"));
    }

    taida_val effective_timeout = timeout_ms > 0 ? timeout_ms : st->acquire_timeout_ms;
    if (effective_timeout <= 0) {
        return taida_async_resolved(taida_pool_result_failure("invalid", "poolAcquire: timeoutMs must be > 0"));
    }

    taida_val token = 0;
    taida_val resource = 0;  // Unit
    if (st->idle_len > 0) {
        st->idle_len--;
        token = st->idle_tokens[st->idle_len];
        resource = st->idle_resources[st->idle_len];
    } else if ((taida_val)st->in_use_len < st->max_size) {
        token = st->next_token++;
        resource = 0;
    } else {
        char msg[96];
        snprintf(msg, sizeof(msg), "poolAcquire: timed out after %" PRId64 "ms", effective_timeout);
        return taida_async_resolved(taida_pool_result_failure("timeout", msg));
    }

    if (!taida_pool_push_in_use(st, token)) {
        return taida_async_resolved(taida_pool_result_failure("other", "poolAcquire: out of memory"));
    }

    taida_val inner = taida_pack_new(2);
    taida_val resource_hash = taida_str_hash((taida_val)"resource");
    taida_val token_hash = taida_str_hash((taida_val)"token");
    taida_pack_set_hash(inner, 0, resource_hash);
    taida_pack_set(inner, 0, resource);
    taida_pack_set_hash(inner, 1, token_hash);
    taida_pack_set(inner, 1, token);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_pool_release(taida_val pool_or_pack, taida_val token, taida_val resource) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        return taida_pool_result_failure("invalid", "poolRelease: unknown pool handle");
    }

    taida_pool_state *st = taida_pool_states[pool_id];
    if (!st->open) {
        return taida_pool_result_failure("closed", "poolRelease: pool is closed");
    }

    taida_val idx = taida_pool_find_in_use_idx(st, token);
    if (idx < 0) {
        return taida_pool_result_failure("invalid", "poolRelease: token is not in-use");
    }
    st->in_use_tokens[idx] = st->in_use_tokens[st->in_use_len - 1];
    st->in_use_len--;

    taida_val reused = 0;
    if ((taida_val)st->idle_len < st->max_idle) {
        if (!taida_pool_push_idle(st, token, resource)) {
            return taida_pool_result_failure("other", "poolRelease: out of memory");
        }
        reused = 1;
    }

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val reused_hash = taida_str_hash((taida_val)"reused");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_pack_set_hash(inner, 1, reused_hash);
    taida_pack_set(inner, 1, reused);
    return taida_os_result_success(inner);
}

taida_val taida_pool_close(taida_val pool_or_pack) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        return taida_async_resolved(taida_pool_result_failure("invalid", "poolClose: unknown pool handle"));
    }
    taida_pool_state *st = taida_pool_states[pool_id];
    if (!st->open) {
        return taida_async_resolved(taida_pool_result_failure("closed", "poolClose: pool already closed"));
    }
    st->open = 0;
    st->idle_len = 0;
    st->in_use_len = 0;

    taida_val inner = taida_pack_new(1);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_pool_health(taida_val pool_or_pack) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        return taida_pool_health_pack(0, 0, 0, 0);
    }
    taida_pool_state *st = taida_pool_states[pool_id];
    return taida_pool_health_pack(st->open, (taida_val)st->idle_len, (taida_val)st->in_use_len, 0);
}

// ── taida-lang/net: HTTP v1 runtime ─────────────────────────────
// httpParseRequestHead, httpEncodeResponse, httpServe
// These are dedicated net runtime functions, not os wrappers.

// Forward declarations
taida_val taida_net_http_parse_request_head(taida_val input);
taida_val taida_net_http_encode_response(taida_val response);
taida_val taida_net_http_serve(taida_val port, taida_val handler, taida_val max_requests, taida_val timeout_ms, taida_val max_connections, taida_val tls, taida_val handler_type_tag, taida_val handler_arity);
taida_val taida_net_read_body(taida_val req);
// NET3-5b: v3 streaming API forward declarations
taida_val taida_net_start_response(taida_val writer, taida_val status, taida_val headers);
taida_val taida_net_write_chunk(taida_val writer, taida_val data);
taida_val taida_net_end_response(taida_val writer);
taida_val taida_net_sse_event(taida_val writer, taida_val event, taida_val data);
// NB4-6: v4 request body streaming + WebSocket API forward declarations
taida_val taida_net_read_body_chunk(taida_val req);
taida_val taida_net_read_body_all(taida_val req);
taida_val taida_net_ws_upgrade(taida_val req, taida_val writer);
taida_val taida_net_ws_send(taida_val ws, taida_val data);
taida_val taida_net_ws_receive(taida_val ws);
taida_val taida_net_ws_close(taida_val ws, taida_val code);
taida_val taida_net_ws_close_code(taida_val ws);
// v4: body stream request check (defined later, forward declared here for readBody delegation)
static int taida_net4_is_body_stream_request(taida_val req);

// Net result helpers (use HttpError instead of IoError)
static taida_val taida_net_result_ok(taida_val inner) {
    return taida_result_create(inner, 0, 0);
}

static taida_val taida_net_result_fail(const char *kind, const char *message) {
    // inner = @(ok: false, code: -1, message: msg, kind: kind)
    taida_val inner = taida_pack_new(4);
    taida_pack_set_hash(inner, 0, taida_str_hash((taida_val)"ok"));
    taida_pack_set(inner, 0, 0);  // false
    taida_pack_set_tag(inner, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(inner, 1, taida_str_hash((taida_val)"code"));
    taida_pack_set(inner, 1, -1);
    taida_pack_set_hash(inner, 2, taida_str_hash((taida_val)"message"));
    taida_pack_set(inner, 2, (taida_val)taida_str_new_copy(message));
    taida_pack_set_tag(inner, 2, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 3, taida_str_hash((taida_val)"kind"));
    taida_pack_set(inner, 3, (taida_val)taida_str_new_copy(kind));
    taida_pack_set_tag(inner, 3, TAIDA_TAG_STR);

    taida_val error = taida_make_error("HttpError", message);
    return taida_result_create(inner, error, 0);
}

// Helper: create span @(start: Int, len: Int)
static taida_val taida_net_make_span(taida_val start, taida_val len) {
    taida_val pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, taida_str_hash((taida_val)"start"));
    taida_pack_set(pack, 0, start);
    taida_pack_set_hash(pack, 1, taida_str_hash((taida_val)"len"));
    taida_pack_set(pack, 1, len);
    return pack;
}

// Status reason phrases (mirrors Interpreter status_reason)
static const char *taida_net_status_reason(int code) {
    switch (code) {
        case 100: return "Continue";
        case 101: return "Switching Protocols";
        case 200: return "OK";
        case 201: return "Created";
        case 202: return "Accepted";
        case 204: return "No Content";
        case 205: return "Reset Content";
        case 206: return "Partial Content";
        case 301: return "Moved Permanently";
        case 302: return "Found";
        case 304: return "Not Modified";
        case 307: return "Temporary Redirect";
        case 308: return "Permanent Redirect";
        case 400: return "Bad Request";
        case 401: return "Unauthorized";
        case 403: return "Forbidden";
        case 404: return "Not Found";
        case 405: return "Method Not Allowed";
        case 408: return "Request Timeout";
        case 409: return "Conflict";
        case 410: return "Gone";
        case 413: return "Content Too Large";
        case 415: return "Unsupported Media Type";
        case 418: return "I'm a Teapot";
        case 422: return "Unprocessable Content";
        case 429: return "Too Many Requests";
        case 500: return "Internal Server Error";
        case 502: return "Bad Gateway";
        case 503: return "Service Unavailable";
        case 504: return "Gateway Timeout";
        default:  return "";
    }
}

// ── httpParseRequestHead(bytes) ─────────────────────────────────
// Hand-written HTTP/1.1 request head parser (no external deps).
// Returns Result[@(complete, consumed, method, path, query, version, headers, bodyOffset, contentLength, chunked), _]
taida_val taida_net_http_parse_request_head(taida_val input) {
    // Extract raw bytes from Bytes or Str
    unsigned char *data = NULL;
    size_t data_len = 0;
    int free_data = 0;

    if (TAIDA_IS_BYTES(input)) {
        taida_val *bytes = (taida_val*)input;
        taida_val blen = bytes[1];
        if (blen < 0) blen = 0;
        data_len = (size_t)blen;
        data = (unsigned char*)TAIDA_MALLOC(data_len + 1, "net_parse_input");
        for (size_t i = 0; i < data_len; i++) data[i] = (unsigned char)bytes[2 + i];
        data[data_len] = 0;
        free_data = 1;
    } else {
        // Assume string
        size_t slen = 0;
        if (!taida_read_cstr_len_safe((const char*)input, 1048576, &slen)) {
            return taida_net_result_fail("ParseError", "httpParseRequestHead: argument must be Bytes or Str");
        }
        data = (unsigned char*)input;
        data_len = slen;
    }

    // Find \r\n\r\n (end of head)
    int head_end = -1;
    for (size_t i = 0; i + 3 < data_len; i++) {
        if (data[i] == '\r' && data[i+1] == '\n' && data[i+2] == '\r' && data[i+3] == '\n') {
            head_end = (int)i;
            break;
        }
    }

    int complete = (head_end >= 0);
    size_t consumed = complete ? (size_t)(head_end + 4) : 0;

    // We need at least a request line to parse
    // Find the first \r\n for request line
    int first_crlf = -1;
    size_t scan_limit = complete ? (size_t)head_end : data_len;
    for (size_t i = 0; i + 1 < scan_limit; i++) {
        if (data[i] == '\r' && data[i+1] == '\n') {
            first_crlf = (int)i;
            break;
        }
    }

    if (first_crlf < 0) {
        // No CRLF found at all — incomplete if no head_end, try to check for obvious malformed
        if (!complete) {
            // Could be incomplete — return incomplete result
            taida_val parsed = taida_pack_new(10);
            taida_pack_set_hash(parsed, 0, taida_str_hash((taida_val)"complete"));
            taida_pack_set(parsed, 0, 0);  // false
            taida_pack_set_tag(parsed, 0, TAIDA_TAG_BOOL);
            taida_pack_set_hash(parsed, 1, taida_str_hash((taida_val)"consumed"));
            taida_pack_set(parsed, 1, 0);
            taida_pack_set_hash(parsed, 2, taida_str_hash((taida_val)"method"));
            taida_pack_set(parsed, 2, taida_net_make_span(0, 0));
            taida_pack_set_tag(parsed, 2, TAIDA_TAG_PACK);
            taida_pack_set_hash(parsed, 3, taida_str_hash((taida_val)"path"));
            taida_pack_set(parsed, 3, taida_net_make_span(0, 0));
            taida_pack_set_tag(parsed, 3, TAIDA_TAG_PACK);
            taida_pack_set_hash(parsed, 4, taida_str_hash((taida_val)"query"));
            taida_pack_set(parsed, 4, taida_net_make_span(0, 0));
            taida_pack_set_tag(parsed, 4, TAIDA_TAG_PACK);
            taida_val ver = taida_pack_new(2);
            taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
            taida_pack_set(ver, 0, 1);
            taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
            taida_pack_set(ver, 1, 1);
            taida_pack_set_hash(parsed, 5, taida_str_hash((taida_val)"version"));
            taida_pack_set(parsed, 5, ver);
            taida_pack_set_tag(parsed, 5, TAIDA_TAG_PACK);
            taida_pack_set_hash(parsed, 6, taida_str_hash((taida_val)"headers"));
            taida_pack_set(parsed, 6, taida_list_new());
            taida_pack_set_tag(parsed, 6, TAIDA_TAG_LIST);
            taida_pack_set_hash(parsed, 7, taida_str_hash((taida_val)"bodyOffset"));
            taida_pack_set(parsed, 7, 0);
            taida_pack_set_hash(parsed, 8, taida_str_hash((taida_val)"contentLength"));
            taida_pack_set(parsed, 8, 0);
            taida_pack_set_hash(parsed, 9, taida_str_hash((taida_val)"chunked"));
            taida_pack_set(parsed, 9, 0);  // false
            taida_pack_set_tag(parsed, 9, TAIDA_TAG_BOOL);
            if (free_data) free(data);
            return taida_net_result_ok(parsed);
        }
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: no request line");
    }

    // Parse request line: METHOD SP PATH HTTP/x.y
    // Find first SP
    int method_end = -1;
    for (int i = 0; i < first_crlf; i++) {
        if (data[i] == ' ') { method_end = i; break; }
    }
    if (method_end <= 0) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid request line");
    }

    // Find last SP (before HTTP/x.y)
    int version_start = -1;
    for (int i = first_crlf - 1; i > method_end; i--) {
        if (data[i - 1] == ' ') { version_start = i; break; }
    }
    if (version_start < 0 || version_start <= method_end + 1) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid request line");
    }

    // Parse version: must be exactly "HTTP/x.y" where x,y are single ASCII digits
    // Strict: reject HTTP/a.b, HTTP/12.34, HTTP/1, etc. (parity with Interpreter/JS)
    int http_major = 1, http_minor = 1;
    int version_len = first_crlf - version_start;
    if (version_len == 8 &&
        data[version_start]   == 'H' && data[version_start+1] == 'T' &&
        data[version_start+2] == 'T' && data[version_start+3] == 'P' &&
        data[version_start+4] == '/' &&
        data[version_start+5] >= '0' && data[version_start+5] <= '9' &&
        data[version_start+6] == '.' &&
        data[version_start+7] >= '0' && data[version_start+7] <= '9') {
        http_major = data[version_start+5] - '0';
        http_minor = data[version_start+7] - '0';
        // NB-32: restrict to HTTP/1.0 and HTTP/1.1 only (parity with Interpreter/httparse)
        // Reject immediately once version is fully parsed, regardless of head completeness
        if (http_major != 1 || (http_minor != 0 && http_minor != 1)) {
            if (free_data) free(data);
            return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid HTTP version");
        }
    } else if (complete) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid HTTP version");
    }

    // Method span
    int method_start_idx = 0;
    int method_len = method_end;

    // Path + query: between first SP and last SP
    int uri_start = method_end + 1;
    int uri_end = version_start - 1;
    int uri_len = uri_end - uri_start;

    // Split path and query on '?'
    int path_start_idx = uri_start;
    int path_len = uri_len;
    int query_start_idx = 0;
    int query_len = 0;
    for (int i = uri_start; i < uri_end; i++) {
        if (data[i] == '?') {
            path_len = i - uri_start;
            query_start_idx = i + 1;
            query_len = uri_end - (i + 1);
            break;
        }
    }

    // Parse headers
    taida_val headers_list = taida_list_new();
    int64_t content_length = 0;
    int cl_count = 0;
    int has_te_chunked = 0;
    size_t pos = (size_t)(first_crlf + 2);  // after first \r\n

    int header_count = 0;
    while (pos < scan_limit) {
        // Find next \r\n
        size_t line_end = scan_limit;
        for (size_t j = pos; j + 1 < scan_limit; j++) {
            if (data[j] == '\r' && data[j+1] == '\n') {
                line_end = j;
                break;
            }
        }
        if (line_end == pos) break;  // empty line = end of headers

        // NB-4/NB-6: enforce max 64 headers (parity with Interpreter/httparse)
        header_count++;
        if (header_count > 64) {
            if (free_data) free(data);
            return taida_net_result_fail("ParseError", "Malformed HTTP request: too many headers");
        }

        // Find colon separator
        size_t colon = line_end;
        for (size_t j = pos; j < line_end; j++) {
            if (data[j] == ':') { colon = j; break; }
        }
        if (colon >= line_end) {
            // No colon found: if head is complete this is malformed, otherwise incomplete
            if (complete) {
                if (free_data) free(data);
                return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid header line");
            }
            break;  // incomplete — stop parsing headers
        }

        // Header name: pos..colon, value: after colon + OWS trimming
        size_t name_start = pos;
        size_t name_len = colon - pos;
        size_t val_start = colon + 1;
        // NB-34: Skip leading SP/HT and trim trailing SP/HT (parity with Interpreter/httparse)
        while (val_start < line_end && (data[val_start] == ' ' || data[val_start] == '\t')) val_start++;
        size_t val_end = line_end;
        while (val_end > val_start && (data[val_end - 1] == ' ' || data[val_end - 1] == '\t')) val_end--;
        size_t val_len = val_end - val_start;

        taida_val header_pack = taida_pack_new(2);
        taida_pack_set_hash(header_pack, 0, taida_str_hash((taida_val)"name"));
        taida_pack_set(header_pack, 0, taida_net_make_span((taida_val)name_start, (taida_val)name_len));
        taida_pack_set_tag(header_pack, 0, TAIDA_TAG_PACK);
        taida_pack_set_hash(header_pack, 1, taida_str_hash((taida_val)"value"));
        taida_pack_set(header_pack, 1, taida_net_make_span((taida_val)val_start, (taida_val)val_len));
        taida_pack_set_tag(header_pack, 1, TAIDA_TAG_PACK);
        headers_list = taida_list_push(headers_list, header_pack);

        // Check Content-Length (case-insensitive)
        if (name_len == 14) {
            // Check "content-length" case-insensitively
            const char *cl_expected = "content-length";
            int is_cl = 1;
            for (size_t k = 0; k < 14; k++) {
                char c = (char)data[name_start + k];
                if (c >= 'A' && c <= 'Z') c += 32;
                if (c != cl_expected[k]) { is_cl = 0; break; }
            }
            if (is_cl) {
                cl_count++;
                if (cl_count > 1) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: duplicate Content-Length header");
                }
                // Validate: trimmed value must be all digits
                // val_start..val_start+val_len (already trimmed leading spaces/tabs)
                // Also trim trailing spaces and tabs (parity with Interpreter's .trim() and JS's .trim())
                size_t cl_end = val_start + val_len;
                while (cl_end > val_start && (data[cl_end-1] == ' ' || data[cl_end-1] == '\t')) cl_end--;
                size_t cl_len = cl_end - val_start;
                if (cl_len == 0) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                }
                int all_digits = 1;
                for (size_t k = 0; k < cl_len; k++) {
                    if (data[val_start + k] < '0' || data[val_start + k] > '9') {
                        all_digits = 0;
                        break;
                    }
                }
                if (!all_digits) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                }
                // Parse digits
                int64_t cl_val = 0;
                for (size_t k = 0; k < cl_len; k++) {
                    int64_t digit = data[val_start + k] - '0';
                    // Overflow check
                    if (cl_val > (9007199254740991LL - digit) / 10) {
                        if (free_data) free(data);
                        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                    }
                    cl_val = cl_val * 10 + digit;
                }
                // Cap at Number.MAX_SAFE_INTEGER
                if (cl_val > 9007199254740991LL) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                }
                content_length = cl_val;
            }
        }

        // NET2-2a: Detect Transfer-Encoding: chunked (parity with Interpreter)
        if (name_len == 17) {
            const char *te_expected = "transfer-encoding";
            int is_te = 1;
            for (size_t k = 0; k < 17; k++) {
                char c = (char)data[name_start + k];
                if (c >= 'A' && c <= 'Z') c += 32;
                if (c != te_expected[k]) { is_te = 0; break; }
            }
            if (is_te) {
                // Scan comma-separated tokens for "chunked" (case-insensitive)
                size_t tk_start = val_start;
                while (tk_start < val_start + val_len) {
                    // Skip leading whitespace
                    while (tk_start < val_start + val_len && (data[tk_start] == ' ' || data[tk_start] == '\t')) tk_start++;
                    // Find comma or end
                    size_t tk_end = tk_start;
                    while (tk_end < val_start + val_len && data[tk_end] != ',') tk_end++;
                    // Trim trailing whitespace of token
                    size_t tk_trim = tk_end;
                    while (tk_trim > tk_start && (data[tk_trim - 1] == ' ' || data[tk_trim - 1] == '\t')) tk_trim--;
                    size_t tk_len = tk_trim - tk_start;
                    if (tk_len == 7) {
                        const char *chunked_str = "chunked";
                        int match = 1;
                        for (size_t m = 0; m < 7; m++) {
                            char c = (char)data[tk_start + m];
                            if (c >= 'A' && c <= 'Z') c += 32;
                            if (c != chunked_str[m]) { match = 0; break; }
                        }
                        if (match) has_te_chunked = 1;
                    }
                    tk_start = tk_end + 1;  // skip comma
                }
            }
        }

        pos = line_end + 2;  // skip \r\n
    }

    // NET2-2e: Reject Content-Length + Transfer-Encoding: chunked (RFC 7230 section 3.3.3)
    if (has_te_chunked && cl_count > 0) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: Content-Length and Transfer-Encoding: chunked are mutually exclusive");
    }

    // Build result pack
    taida_val parsed = taida_pack_new(10);
    taida_pack_set_hash(parsed, 0, taida_str_hash((taida_val)"complete"));
    taida_pack_set(parsed, 0, complete ? 1 : 0);
    taida_pack_set_tag(parsed, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(parsed, 1, taida_str_hash((taida_val)"consumed"));
    taida_pack_set(parsed, 1, (taida_val)consumed);
    taida_pack_set_hash(parsed, 2, taida_str_hash((taida_val)"method"));
    taida_pack_set(parsed, 2, taida_net_make_span((taida_val)method_start_idx, (taida_val)method_len));
    taida_pack_set_tag(parsed, 2, TAIDA_TAG_PACK);
    taida_pack_set_hash(parsed, 3, taida_str_hash((taida_val)"path"));
    taida_pack_set(parsed, 3, taida_net_make_span((taida_val)path_start_idx, (taida_val)path_len));
    taida_pack_set_tag(parsed, 3, TAIDA_TAG_PACK);
    taida_pack_set_hash(parsed, 4, taida_str_hash((taida_val)"query"));
    taida_pack_set(parsed, 4, taida_net_make_span((taida_val)query_start_idx, (taida_val)query_len));
    taida_pack_set_tag(parsed, 4, TAIDA_TAG_PACK);

    taida_val ver = taida_pack_new(2);
    taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
    taida_pack_set(ver, 0, (taida_val)http_major);
    taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
    taida_pack_set(ver, 1, (taida_val)http_minor);
    taida_pack_set_hash(parsed, 5, taida_str_hash((taida_val)"version"));
    taida_pack_set(parsed, 5, ver);
    taida_pack_set_tag(parsed, 5, TAIDA_TAG_PACK);

    taida_pack_set_hash(parsed, 6, taida_str_hash((taida_val)"headers"));
    taida_pack_set(parsed, 6, headers_list);
    taida_pack_set_tag(parsed, 6, TAIDA_TAG_LIST);

    taida_pack_set_hash(parsed, 7, taida_str_hash((taida_val)"bodyOffset"));
    taida_pack_set(parsed, 7, (taida_val)consumed);

    taida_pack_set_hash(parsed, 8, taida_str_hash((taida_val)"contentLength"));
    taida_pack_set(parsed, 8, (taida_val)content_length);

    taida_pack_set_hash(parsed, 9, taida_str_hash((taida_val)"chunked"));
    taida_pack_set(parsed, 9, has_te_chunked ? 1 : 0);
    taida_pack_set_tag(parsed, 9, TAIDA_TAG_BOOL);

    if (free_data) free(data);
    return taida_net_result_ok(parsed);
}

// ── httpEncodeResponse(response) ────────────────────────────────
// Encode response @(status, headers, body) into HTTP/1.1 wire bytes.
// Returns Result[@(bytes: Bytes), _]
taida_val taida_net_http_encode_response(taida_val response) {
    if (!taida_is_buchi_pack(response)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: argument must be a BuchiPack @(...)");
    }

    // Extract status (required, must be Int in 100-999)
    taida_val status_hash = taida_str_hash((taida_val)"status");
    if (!taida_pack_has_hash(response, status_hash)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: missing required field 'status'");
    }
    taida_val status = taida_pack_get(response, status_hash);
    // NB-14: Type check via field tag — status must be Int.
    // When tag is UNKNOWN, resolve via runtime detection to catch non-Int values
    // that the compiler couldn't type-check statically.
    {
        taida_val status_tag = taida_pack_get_field_tag(response, status_hash);
        if (status_tag == TAIDA_TAG_UNKNOWN) {
            status_tag = taida_runtime_detect_tag(status);
        }
        if (status_tag != TAIDA_TAG_INT) {
            char val_buf[64];
            taida_format_value(status_tag, status, val_buf, sizeof(val_buf));
            char err_msg[128];
            snprintf(err_msg, sizeof(err_msg),
                     "httpEncodeResponse: status must be Int, got %s",
                     val_buf);
            return taida_net_result_fail("EncodeError", err_msg);
        }
    }
    if (status < 100 || status > 999) {
        char err_msg[128];
        snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: status must be 100-999, got %d", (int)status);
        return taida_net_result_fail("EncodeError", err_msg);
    }

    // Extract headers (required, must be a List)
    taida_val headers_hash = taida_str_hash((taida_val)"headers");
    if (!taida_pack_has_hash(response, headers_hash)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: missing required field 'headers'");
    }
    taida_val headers_ptr = taida_pack_get(response, headers_hash);
    if (!taida_is_list(headers_ptr)) {
        // NB-21: Format actual value for parity with Interpreter/JS
        taida_val htag = taida_pack_get_field_tag(response, headers_hash);
        char val_buf[64];
        taida_format_value(htag, headers_ptr, val_buf, sizeof(val_buf));
        char err_msg[128];
        snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers must be a List, got %s",
                 val_buf);
        return taida_net_result_fail("EncodeError", err_msg);
    }

    // Extract body (required, must be Bytes or Str)
    taida_val body_hash = taida_str_hash((taida_val)"body");
    if (!taida_pack_has_hash(response, body_hash)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: missing required field 'body'");
    }
    taida_val body_ptr = taida_pack_get(response, body_hash);
    unsigned char *body_data = NULL;
    size_t body_len = 0;
    int free_body = 0;

    if (TAIDA_IS_BYTES(body_ptr)) {
        taida_val *bytes = (taida_val*)body_ptr;
        taida_val blen = bytes[1];
        if (blen < 0) blen = 0;
        body_len = (size_t)blen;
        body_data = (unsigned char*)TAIDA_MALLOC(body_len + 1, "net_encode_body");
        for (size_t i = 0; i < body_len; i++) body_data[i] = (unsigned char)bytes[2 + i];
        body_data[body_len] = 0;
        free_body = 1;
    } else {
        size_t slen = 0;
        if (taida_read_cstr_len_safe((const char*)body_ptr, 10485760, &slen)) {
            body_data = (unsigned char*)body_ptr;
            body_len = slen;
        } else {
            // NB-21: Format actual value for parity with Interpreter/JS
            taida_val btag = taida_pack_get_field_tag(response, body_hash);
            char val_buf[64];
            taida_format_value(btag, body_ptr, val_buf, sizeof(val_buf));
            char err_msg[128];
            snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: body must be Bytes or Str, got %s",
                     val_buf);
            return taida_net_result_fail("EncodeError", err_msg);
        }
    }

    // RFC 9110: 1xx, 204, 205, 304 MUST NOT contain a message body
    int no_body = (status >= 100 && status < 200) || status == 204 || status == 205 || status == 304;
    if (no_body && body_len > 0) {
        if (free_body) free(body_data);
        char err_msg[128];
        snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: status %d must not have a body", (int)status);
        return taida_net_result_fail("EncodeError", err_msg);
    }

    // Build HTTP response buffer
    size_t buf_cap = 512 + body_len;
    unsigned char *buf = (unsigned char*)TAIDA_MALLOC(buf_cap, "net_encode_buf");
    size_t buf_len = 0;

    // Status line
    const char *reason = taida_net_status_reason((int)status);
    buf_len += (size_t)snprintf((char*)buf + buf_len, buf_cap - buf_len,
                                 "HTTP/1.1 %d %s\r\n", (int)status, reason);

    // User headers
    int has_content_length = 0;
    taida_val name_hash = taida_str_hash((taida_val)"name");
    taida_val value_hash = taida_str_hash((taida_val)"value");

    {
        taida_val *hlist = (taida_val*)headers_ptr;
        taida_val hcount = hlist[2];
        for (taida_val i = 0; i < hcount; i++) {
            taida_val hdr = hlist[4 + i];
            if (!taida_is_buchi_pack(hdr)) {
                if (free_body) free(body_data);
                free(buf);
                char err_msg[128];
                snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d] must be @(name, value)", (int)i);
                return taida_net_result_fail("EncodeError", err_msg);
            }
            taida_val hname = taida_pack_get(hdr, name_hash);
            taida_val hvalue = taida_pack_get(hdr, value_hash);
            const char *hname_s = (const char*)hname;
            const char *hvalue_s = (const char*)hvalue;
            size_t hn_len = 0, hv_len = 0;
            if (!taida_read_cstr_len_safe(hname_s, 8192, &hn_len)) {
                if (free_body) free(body_data);
                free(buf);
                char err_msg[128];
                snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].name must be Str", (int)i);
                return taida_net_result_fail("EncodeError", err_msg);
            }
            if (!taida_read_cstr_len_safe(hvalue_s, 65536, &hv_len)) {
                if (free_body) free(body_data);
                free(buf);
                char err_msg[128];
                snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].value must be Str", (int)i);
                return taida_net_result_fail("EncodeError", err_msg);
            }

            // NB-13: Check for CRLF injection with index + name/value distinction (parity with Interpreter/JS)
            for (size_t k = 0; k < hn_len; k++) {
                if (hname_s[k] == '\r' || hname_s[k] == '\n') {
                    if (free_body) free(body_data);
                    free(buf);
                    char err_msg[128];
                    snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].name contains CR/LF", (int)i);
                    return taida_net_result_fail("EncodeError", err_msg);
                }
            }
            for (size_t k = 0; k < hv_len; k++) {
                if (hvalue_s[k] == '\r' || hvalue_s[k] == '\n') {
                    if (free_body) free(body_data);
                    free(buf);
                    char err_msg[128];
                    snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].value contains CR/LF", (int)i);
                    return taida_net_result_fail("EncodeError", err_msg);
                }
            }

            // Skip Content-Length for no-body statuses
            if (no_body && hn_len == 14) {
                const char *cl_expected = "content-length";
                int is_cl = 1;
                for (size_t k = 0; k < 14; k++) {
                    char c = hname_s[k];
                    if (c >= 'A' && c <= 'Z') c += 32;
                    if (c != cl_expected[k]) { is_cl = 0; break; }
                }
                if (is_cl) continue;
            }

            // Check if user provided Content-Length
            if (hn_len == 14) {
                const char *cl_expected = "content-length";
                int is_cl = 1;
                for (size_t k = 0; k < 14; k++) {
                    char c = hname_s[k];
                    if (c >= 'A' && c <= 'Z') c += 32;
                    if (c != cl_expected[k]) { is_cl = 0; break; }
                }
                if (is_cl) has_content_length = 1;
            }

            // Grow buffer if needed
            size_t needed = buf_len + hn_len + hv_len + 4;
            if (needed > buf_cap) {
                buf_cap = needed * 2;
                TAIDA_REALLOC(buf, buf_cap, "net_encode_headers");
            }
            memcpy(buf + buf_len, hname_s, hn_len); buf_len += hn_len;
            buf[buf_len++] = ':'; buf[buf_len++] = ' ';
            memcpy(buf + buf_len, hvalue_s, hv_len); buf_len += hv_len;
            buf[buf_len++] = '\r'; buf[buf_len++] = '\n';
        }
    }

    // Auto-append Content-Length for statuses that allow a body
    if (!no_body && !has_content_length) {
        char cl_hdr[64];
        int cl_len = snprintf(cl_hdr, sizeof(cl_hdr), "Content-Length: %zu\r\n", body_len);
        size_t needed = buf_len + (size_t)cl_len;
        if (needed > buf_cap) {
            buf_cap = needed * 2;
            TAIDA_REALLOC(buf, buf_cap, "net_encode_cl");
        }
        memcpy(buf + buf_len, cl_hdr, (size_t)cl_len);
        buf_len += (size_t)cl_len;
    }

    // End of headers
    size_t needed = buf_len + 2 + body_len;
    if (needed > buf_cap) {
        buf_cap = needed;
        TAIDA_REALLOC(buf, buf_cap, "net_encode_body");
    }
    buf[buf_len++] = '\r'; buf[buf_len++] = '\n';

    // Body
    if (!no_body && body_len > 0) {
        memcpy(buf + buf_len, body_data, body_len);
        buf_len += body_len;
    }

    if (free_body) free(body_data);

    // Convert to Bytes value
    taida_val result_bytes = taida_bytes_from_raw(buf, (taida_val)buf_len);
    free(buf);

    taida_val result = taida_pack_new(1);
    taida_pack_set_hash(result, 0, taida_str_hash((taida_val)"bytes"));
    taida_pack_set(result, 0, result_bytes);
    taida_pack_set_tag(result, 0, TAIDA_TAG_PACK);  // Bytes IS-A tagged ptr

    return taida_net_result_ok(result);
}

// ── net_send_all: short-write safe send helper ──────────────────
// Loops send() until all bytes are written or an error occurs.
// Returns 0 on success, -1 on error.
// NET5-4a: Routes through TLS when tl_ssl is active.
static int taida_net_send_all(int fd, const void *buf, size_t len) {
    return taida_tls_send_all(fd, buf, len);
}

// ── readBody(req) → Bytes ────────────────────────────────────────
// Extract body bytes from a request pack.
// req.raw (Bytes) + body span (start, len) → body slice as new Bytes.
// If body.len == 0 or body span is absent, returns empty Bytes.
taida_val taida_net_read_body(taida_val req) {
    if (!taida_is_buchi_pack(req)) {
        // Parity: Interpreter returns RuntimeError, JS throws __NativeError
        char val_buf[64];
        taida_val tag = taida_runtime_detect_tag(req);
        taida_format_value(tag, req, val_buf, sizeof(val_buf));
        char err_msg[256];
        snprintf(err_msg, sizeof(err_msg),
                 "readBody: argument must be a request pack @(...), got %s",
                 val_buf);
        return taida_throw(taida_make_error("TypeError", err_msg));
    }

    // v4: If the request has __body_stream sentinel (2-arg handler),
    // delegate to readBodyAll to stream from socket.
    if (taida_net4_is_body_stream_request(req)) {
        return taida_net_read_body_all(req);
    }

    // Extract raw: Bytes
    taida_val raw = taida_pack_get(req, taida_str_hash((taida_val)"raw"));
    if (!TAIDA_IS_BYTES(raw)) {
        return taida_throw(taida_make_error("TypeError",
            "readBody: request pack missing 'raw: Bytes' field"));
    }

    // Extract body: @(start: Int, len: Int)
    taida_val body_span = taida_pack_get(req, taida_str_hash((taida_val)"body"));
    taida_val body_start = 0;
    taida_val body_len = 0;
    if (body_span != 0 && taida_is_buchi_pack(body_span)) {
        body_start = taida_pack_get(body_span, taida_str_hash((taida_val)"start"));
        body_len = taida_pack_get(body_span, taida_str_hash((taida_val)"len"));
    }

    if (body_len <= 0) {
        return taida_bytes_new_filled(0, 0);
    }

    // raw layout: [magic+rc, length, b0, b1, ...]
    taida_val *raw_arr = (taida_val*)raw;
    taida_val raw_len = raw_arr[1];

    // Clamp to valid range
    if (body_start < 0) body_start = 0;
    if (body_start > raw_len) body_start = raw_len;
    taida_val end = body_start + body_len;
    if (end > raw_len) end = raw_len;
    taida_val actual_len = end - body_start;
    if (actual_len <= 0) {
        return taida_bytes_new_filled(0, 0);
    }

    // Copy body bytes into a new Bytes object
    taida_val out = taida_bytes_new_filled(actual_len, 0);
    taida_val *out_arr = (taida_val*)out;
    for (taida_val i = 0; i < actual_len; i++) {
        out_arr[2 + i] = raw_arr[2 + body_start + i];
    }
    return out;
}

// ── NET2-5a: Keep-Alive determination ──────────────────────────
// Determine whether the connection should be kept alive based on
// HTTP version and the Connection header value.
// Rules (RFC 7230 S6.1):
//   HTTP/1.1: keep-alive by default, Connection: close disables
//   HTTP/1.0: close by default, Connection: keep-alive enables
// raw is the wire bytes buffer, headers is the parsed header list.
static int taida_net_determine_keep_alive(
    const unsigned char *raw, size_t raw_len,
    taida_val headers, taida_val http_minor
) {
    int has_close = 0;
    int has_keep_alive = 0;

    if (!TAIDA_IS_LIST(headers)) {
        return (http_minor == 1) ? 1 : 0;
    }

    taida_val *hdr_list = (taida_val*)headers;
    taida_val hdr_count = hdr_list[2];  // list length at index 2 (layout: [magic+rc, capacity, length, elem_tag, ...])

    for (taida_val i = 0; i < hdr_count; i++) {
        taida_val header = hdr_list[4 + i];
        if (!taida_is_buchi_pack(header)) continue;

        // Get name span
        taida_val name_span = taida_pack_get(header, taida_str_hash((taida_val)"name"));
        if (!taida_is_buchi_pack(name_span)) continue;
        taida_val name_start = taida_pack_get(name_span, taida_str_hash((taida_val)"start"));
        taida_val name_len = taida_pack_get(name_span, taida_str_hash((taida_val)"len"));
        if (name_start < 0 || name_len <= 0) continue;
        if ((size_t)(name_start + name_len) > raw_len) continue;

        // Case-insensitive compare with "connection" (10 chars)
        if (name_len != 10) continue;
        const char *conn_str = "connection";
        int match = 1;
        for (int j = 0; j < 10; j++) {
            char c = (char)raw[name_start + j];
            if (c >= 'A' && c <= 'Z') c += 32;
            if (c != conn_str[j]) { match = 0; break; }
        }
        if (!match) continue;

        // Extract value span and scan comma-separated tokens
        taida_val val_span = taida_pack_get(header, taida_str_hash((taida_val)"value"));
        if (!taida_is_buchi_pack(val_span)) continue;
        taida_val val_start = taida_pack_get(val_span, taida_str_hash((taida_val)"start"));
        taida_val val_len = taida_pack_get(val_span, taida_str_hash((taida_val)"len"));
        if (val_start < 0 || val_len <= 0) continue;
        if ((size_t)(val_start + val_len) > raw_len) continue;

        // Scan tokens split by ','
        const unsigned char *vp = raw + val_start;
        size_t vl = (size_t)val_len;
        size_t tok_start = 0;
        for (size_t k = 0; k <= vl; k++) {
            if (k == vl || vp[k] == ',') {
                // Trim whitespace
                size_t ts = tok_start, te = k;
                while (ts < te && (vp[ts] == ' ' || vp[ts] == '\t')) ts++;
                while (te > ts && (vp[te-1] == ' ' || vp[te-1] == '\t')) te--;
                size_t tlen = te - ts;
                if (tlen == 5) {
                    // "close"
                    int mc = 1;
                    const char *cs = "close";
                    for (size_t m = 0; m < 5; m++) {
                        char c = (char)vp[ts + m];
                        if (c >= 'A' && c <= 'Z') c += 32;
                        if (c != cs[m]) { mc = 0; break; }
                    }
                    if (mc) has_close = 1;
                } else if (tlen == 10) {
                    // "keep-alive"
                    int mk = 1;
                    const char *ks = "keep-alive";
                    for (size_t m = 0; m < 10; m++) {
                        char c = (char)vp[ts + m];
                        if (c >= 'A' && c <= 'Z') c += 32;
                        if (c != ks[m]) { mk = 0; break; }
                    }
                    if (mk) has_keep_alive = 1;
                }
                tok_start = k + 1;
            }
        }
        // Don't break — merge multiple Connection headers
    }

    // RFC 7230 S6.1: close always wins
    if (has_close) return 0;
    if (http_minor == 1) return 1;  // HTTP/1.1 default keep-alive
    return has_keep_alive ? 1 : 0;  // HTTP/1.0 default close
}

// ── NET2-5b: Chunked in-place compaction ────────────────────────
// Result struct for chunked compaction
typedef struct {
    size_t body_len;       // compacted body length
    size_t wire_consumed;  // total bytes consumed from body_offset
} ChunkedCompactResult;

// Find the first CRLF in buf[0..len). Returns offset of '\r', or -1 if not found.
static int64_t taida_net_find_crlf(const unsigned char *data, size_t len) {
    if (len < 2) return -1;
    for (size_t i = 0; i + 1 < len; i++) {
        if (data[i] == '\r' && data[i + 1] == '\n') return (int64_t)i;
    }
    return -1;
}

// Check if a complete chunked body is available (read-only scan).
// Returns wire_consumed on success, -1 if incomplete, -2 if malformed.
static int64_t taida_net_chunked_body_complete(
    const unsigned char *buf, size_t total_len, size_t body_offset
) {
    size_t data_len = total_len - body_offset;
    size_t rp = 0;

    for (;;) {
        if (rp >= data_len) return -1; // incomplete

        int64_t crlf = taida_net_find_crlf(buf + body_offset + rp, data_len - rp);
        if (crlf < 0) return -1; // incomplete

        // Parse hex chunk-size, ignoring chunk-ext after ';'
        size_t hex_end = (size_t)crlf;
        for (size_t i = 0; i < hex_end; i++) {
            if (buf[body_offset + rp + i] == ';') { hex_end = i; break; }
        }
        // Trim whitespace
        size_t hs = 0, he = hex_end;
        while (hs < he && (buf[body_offset + rp + hs] == ' ' || buf[body_offset + rp + hs] == '\t')) hs++;
        while (he > hs && (buf[body_offset + rp + he - 1] == ' ' || buf[body_offset + rp + he - 1] == '\t')) he--;
        if (hs >= he) return -2; // empty chunk-size = malformed

        // Parse hex
        // NB2-5: Reject chunk-size with more than 15 hex digits (max safe: 0xFFFFFFFFFFFFFFF)
        // to prevent size_t overflow that silently wraps to 0 and accepts malformed input.
        if (he - hs > 15) return -2; // oversized chunk-size = malformed
        size_t chunk_size = 0;
        for (size_t i = hs; i < he; i++) {
            unsigned char c = buf[body_offset + rp + i];
            int digit = -1;
            if (c >= '0' && c <= '9') digit = c - '0';
            else if (c >= 'a' && c <= 'f') digit = 10 + c - 'a';
            else if (c >= 'A' && c <= 'F') digit = 10 + c - 'A';
            if (digit < 0) return -2; // invalid hex
            chunk_size = chunk_size * 16 + (size_t)digit;
        }

        rp += (size_t)crlf + 2; // skip "chunk-size\r\n"

        if (chunk_size == 0) {
            // Terminator chunk: skip trailers
            for (;;) {
                if (rp + 2 > data_len) return -1; // incomplete
                if (buf[body_offset + rp] == '\r' && buf[body_offset + rp + 1] == '\n') {
                    rp += 2;
                    return (int64_t)rp;
                }
                int64_t tc = taida_net_find_crlf(buf + body_offset + rp, data_len - rp);
                if (tc < 0) return -1; // incomplete
                rp += (size_t)tc + 2;
            }
        }

        // Check data + CRLF
        if (rp + chunk_size + 2 > data_len) return -1; // incomplete
        rp += chunk_size;
        if (buf[body_offset + rp] != '\r' || buf[body_offset + rp + 1] != '\n') return -2; // malformed
        rp += 2;
    }
}

// In-place compaction: remove chunk framing, compact data in-place using memmove.
// Returns 0 on success (result written to *out), -1 on error.
static int taida_net_chunked_in_place_compact(
    unsigned char *buf, size_t body_offset, ChunkedCompactResult *out
) {
    size_t rp = 0; // read position relative to body_offset
    size_t wp = 0; // write position relative to body_offset

    for (;;) {
        int64_t crlf = taida_net_find_crlf(buf + body_offset + rp, 1048576);
        if (crlf < 0) return -1;

        // Parse hex chunk-size, ignoring chunk-ext
        size_t hex_end = (size_t)crlf;
        for (size_t i = 0; i < hex_end; i++) {
            if (buf[body_offset + rp + i] == ';') { hex_end = i; break; }
        }
        size_t hs = 0, he = hex_end;
        while (hs < he && (buf[body_offset + rp + hs] == ' ' || buf[body_offset + rp + hs] == '\t')) hs++;
        while (he > hs && (buf[body_offset + rp + he - 1] == ' ' || buf[body_offset + rp + he - 1] == '\t')) he--;
        if (hs >= he) return -1;

        // NB2-5: Reject oversized chunk-size to prevent overflow (parity with body_complete)
        if (he - hs > 15) return -1;
        size_t chunk_size = 0;
        for (size_t i = hs; i < he; i++) {
            unsigned char c = buf[body_offset + rp + i];
            int digit = -1;
            if (c >= '0' && c <= '9') digit = c - '0';
            else if (c >= 'a' && c <= 'f') digit = 10 + c - 'a';
            else if (c >= 'A' && c <= 'F') digit = 10 + c - 'A';
            if (digit < 0) return -1;
            chunk_size = chunk_size * 16 + (size_t)digit;
        }

        rp += (size_t)crlf + 2; // skip "size\r\n"

        if (chunk_size == 0) {
            // Skip trailers until final CRLF
            for (;;) {
                if (buf[body_offset + rp] == '\r' && buf[body_offset + rp + 1] == '\n') {
                    rp += 2;
                    break;
                }
                int64_t tc = taida_net_find_crlf(buf + body_offset + rp, 1048576);
                if (tc < 0) return -1;
                rp += (size_t)tc + 2;
            }
            out->body_len = wp;
            out->wire_consumed = rp;
            return 0;
        }

        // In-place copy using memmove (safe for overlapping regions)
        if (wp != rp) {
            memmove(buf + body_offset + wp, buf + body_offset + rp, chunk_size);
        }
        wp += chunk_size;
        rp += chunk_size;

        // Validate trailing CRLF
        if (buf[body_offset + rp] != '\r' || buf[body_offset + rp + 1] != '\n') return -1;
        rp += 2;
    }
}

// ── NET2-5: httpServe helper — build request pack ────────────────
static taida_val taida_net_build_request_pack(
    const unsigned char *raw_data, size_t raw_len,
    size_t body_start, size_t body_len, int64_t content_length,
    int is_chunked, int keep_alive,
    const char *remote_host, int remote_port
) {
    taida_val raw_bytes = taida_bytes_from_raw(raw_data, (taida_val)raw_len);

    // Parse to get spans
    taida_val parse_result = taida_net_http_parse_request_head(raw_bytes);
    taida_val inner = taida_pack_get(parse_result, taida_str_hash((taida_val)"__value"));

    taida_val request = taida_pack_new(13);
    taida_pack_set_hash(request, 0, taida_str_hash((taida_val)"raw"));
    taida_pack_set(request, 0, raw_bytes);
    taida_pack_set_tag(request, 0, TAIDA_TAG_PACK);  // Bytes
    taida_retain(raw_bytes);

    if (inner != 0 && taida_is_buchi_pack(inner)) {
        taida_val method_v = taida_pack_get(inner, taida_str_hash((taida_val)"method"));
        taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
        taida_pack_set(request, 1, method_v);
        taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
        if (method_v > 4096) taida_retain(method_v);

        taida_val path_v = taida_pack_get(inner, taida_str_hash((taida_val)"path"));
        taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
        taida_pack_set(request, 2, path_v);
        taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
        if (path_v > 4096) taida_retain(path_v);

        taida_val query_v = taida_pack_get(inner, taida_str_hash((taida_val)"query"));
        taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
        taida_pack_set(request, 3, query_v);
        taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
        if (query_v > 4096) taida_retain(query_v);

        taida_val version_v = taida_pack_get(inner, taida_str_hash((taida_val)"version"));
        taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
        taida_pack_set(request, 4, version_v);
        taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
        if (version_v > 4096) taida_retain(version_v);

        taida_val headers_v = taida_pack_get(inner, taida_str_hash((taida_val)"headers"));
        taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
        taida_pack_set(request, 5, headers_v);
        taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
        if (headers_v > 4096) taida_retain(headers_v);
    } else {
        taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
        taida_pack_set(request, 1, taida_net_make_span(0, 0));
        taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
        taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
        taida_pack_set(request, 2, taida_net_make_span(0, 0));
        taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
        taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
        taida_pack_set(request, 3, taida_net_make_span(0, 0));
        taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
        taida_val ver = taida_pack_new(2);
        taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
        taida_pack_set(ver, 0, 1);
        taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
        taida_pack_set(ver, 1, 1);
        taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
        taida_pack_set(request, 4, ver);
        taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
        taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
        taida_pack_set(request, 5, taida_list_new());
        taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
    }

    taida_pack_set_hash(request, 6, taida_str_hash((taida_val)"body"));
    taida_pack_set(request, 6, taida_net_make_span((taida_val)body_start, (taida_val)body_len));
    taida_pack_set_tag(request, 6, TAIDA_TAG_PACK);

    taida_pack_set_hash(request, 7, taida_str_hash((taida_val)"bodyOffset"));
    taida_pack_set(request, 7, (taida_val)body_start);

    taida_pack_set_hash(request, 8, taida_str_hash((taida_val)"contentLength"));
    taida_pack_set(request, 8, (taida_val)content_length);

    taida_pack_set_hash(request, 9, taida_str_hash((taida_val)"remoteHost"));
    taida_pack_set(request, 9, (taida_val)taida_str_new_copy(remote_host));
    taida_pack_set_tag(request, 9, TAIDA_TAG_STR);

    taida_pack_set_hash(request, 10, taida_str_hash((taida_val)"remotePort"));
    taida_pack_set(request, 10, (taida_val)remote_port);

    taida_pack_set_hash(request, 11, taida_str_hash((taida_val)"keepAlive"));
    taida_pack_set(request, 11, keep_alive ? 1 : 0);
    taida_pack_set_tag(request, 11, TAIDA_TAG_BOOL);

    taida_pack_set_hash(request, 12, taida_str_hash((taida_val)"chunked"));
    taida_pack_set(request, 12, is_chunked ? 1 : 0);
    taida_pack_set_tag(request, 12, TAIDA_TAG_BOOL);

    return request;
}

// ── NET2-5: httpServe helper — send encoded response ─────────────
// NB2-20: Send directly from Bytes internal array — no extra malloc + byte-by-byte copy.
// Bytes layout: [header(magic+rc), length, byte0, byte1, ...] — each byte is a taida_val.
// We still need a contiguous buffer because taida_val slots are 8 bytes each (not 1 byte).
// Optimization: use stack buffer for small responses, heap only for large ones.
static void taida_net_send_response(int client_fd, taida_val encoded) {
    taida_val enc_throw = taida_pack_get(encoded, taida_str_hash((taida_val)"throw"));
    if (enc_throw == 0) {
        taida_val enc_inner = taida_pack_get(encoded, taida_str_hash((taida_val)"__value"));
        if (enc_inner != 0 && taida_is_buchi_pack(enc_inner)) {
            taida_val wire_bytes = taida_pack_get(enc_inner, taida_str_hash((taida_val)"bytes"));
            if (TAIDA_IS_BYTES(wire_bytes)) {
                taida_val *wb = (taida_val*)wire_bytes;
                taida_val wb_len = wb[1];
                // Use stack buffer for typical responses (< 4KB), heap for larger
                unsigned char stack_buf[4096];
                unsigned char *wb_buf;
                int heap_alloc = 0;
                if ((size_t)wb_len <= sizeof(stack_buf)) {
                    wb_buf = stack_buf;
                } else {
                    wb_buf = (unsigned char*)TAIDA_MALLOC((size_t)wb_len, "net_serve_send");
                    heap_alloc = 1;
                }
                for (taida_val i = 0; i < wb_len; i++) wb_buf[i] = (unsigned char)wb[2 + i];
                taida_net_send_all(client_fd, wb_buf, (size_t)wb_len);
                if (heap_alloc) free(wb_buf);
            }
        }
    } else {
        const char *fallback = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        taida_net_send_all(client_fd, fallback, strlen(fallback));
    }
}

// ── NET3-5a/5b/5c/5d/5e: v3 streaming writer state machine ─────────────
// Writer state: Idle(0) → HeadPrepared(1) → Streaming(2) → Ended(3)
// Thread-local context for v3 streaming API. Set in the worker thread
// before invoking a 2-arg handler; the v3 API functions (startResponse,
// writeChunk, endResponse, sseEvent) access it via these thread-locals.

#define NET3_STATE_IDLE         0
#define NET3_STATE_HEAD_PREPARED 1
#define NET3_STATE_STREAMING    2
#define NET3_STATE_ENDED        3
#define NET3_STATE_WEBSOCKET    4

// Maximum pending headers per streaming response
#define NET3_MAX_HEADERS 64

typedef struct {
    int state;               // NET3_STATE_*
    int pending_status;      // default 200
    int sse_mode;            // SSE auto-headers applied
    int header_count;        // number of pending headers
    // Stack-allocated header storage (no per-request malloc for headers)
    const char *header_names[NET3_MAX_HEADERS];
    const char *header_values[NET3_MAX_HEADERS];
} Net3WriterState;

// ── v4 Request Body Streaming State ──────────────────────────
// Per-request state for body-deferred 2-arg handlers.
// Lives on the worker stack; v4 API functions access it via thread-local.

#define NET4_CHUNKED_WAIT_SIZE    0
#define NET4_CHUNKED_READ_DATA    1
#define NET4_CHUNKED_WAIT_TRAILER 2
#define NET4_CHUNKED_DONE         3

typedef struct {
    int is_chunked;          // Transfer-Encoding: chunked?
    int64_t content_length;  // Content-Length from head (0 if absent/chunked)
    int64_t bytes_consumed;  // how many body bytes consumed so far (CL path)
    int fully_read;          // body fully consumed?
    int any_read_started;    // any readBodyChunk/readBodyAll call made?
    // Leftover bytes from head parsing that are body bytes already received.
    unsigned char *leftover;
    size_t leftover_len;
    size_t leftover_pos;     // current position within leftover
    // Chunked decoder state
    int chunked_state;       // NET4_CHUNKED_*
    size_t chunked_remaining;// bytes remaining in current chunk
    // Request-scoped identity token (NB4-7 parity)
    uint64_t request_token;
    // WebSocket close state
    int ws_closed;
    // NB4-10: Connection-scoped WebSocket token for identity verification.
    uint64_t ws_token;
    // v5: Received close code from peer's close frame (0 = not received).
    int64_t ws_close_code;
} Net4BodyState;

// Global monotonic counter for unique request tokens (NB4-7 parity).
static volatile uint64_t taida_net4_next_token = 1;
static uint64_t taida_net4_alloc_token(void) {
    return __atomic_fetch_add(&taida_net4_next_token, 1, __ATOMIC_RELAXED);
}

// NB4-10: Global monotonic counter for unique WebSocket connection tokens.
static volatile uint64_t taida_net4_next_ws_token = 1;
static uint64_t taida_net4_alloc_ws_token(void) {
    return __atomic_fetch_add(&taida_net4_next_ws_token, 1, __ATOMIC_RELAXED);
}

// Thread-local: current writer state and client fd for v3 streaming API.
// These are set/cleared around each 2-arg handler invocation.
static __thread Net3WriterState *tl_net3_writer = NULL;
static __thread int tl_net3_client_fd = -1;
// v4: per-request body streaming state for 2-arg handlers.
static __thread Net4BodyState *tl_net4_body = NULL;

// Forward declaration: writer token validation (defined after create_writer_token).
static void taida_net3_validate_writer(taida_val writer, const char *api_name);

// NET3-5c: writev()-based send helper. Sends all iov buffers, handling
// partial writes and EINTR. Returns 0 on success, -1 on error.
// NET5-4a: Routes through TLS when tl_ssl is active.
static int taida_net_writev_all(int fd, struct iovec *iov, int iovcnt) {
    return taida_tls_writev_all(fd, iov, iovcnt);
}

// Check if a status code forbids a message body (1xx, 204, 205, 304).
static int taida_net3_is_bodyless_status(int status) {
    return (status >= 100 && status <= 199) || status == 204 || status == 205 || status == 304;
}

// Build and send the streaming response head.
// Appends Transfer-Encoding: chunked for non-bodyless status codes.
// Uses stack buffer (no per-request malloc for typical headers).
// Returns 0 on success, -1 on send error, -2 on head overflow.
#define NET3_HEAD_BUF_SIZE 8192
static int taida_net3_commit_head(int fd, Net3WriterState *w) {
    char head_buf[NET3_HEAD_BUF_SIZE];
    size_t cap = sizeof(head_buf);
    size_t offset = 0;
    int n;

    const char *reason = taida_net_status_reason(w->pending_status);
    n = snprintf(head_buf, cap, "HTTP/1.1 %d %s\r\n", w->pending_status, reason);
    if (n < 0 || (size_t)n >= cap) goto overflow;
    offset += (size_t)n;

    for (int i = 0; i < w->header_count && i < NET3_MAX_HEADERS; i++) {
        size_t remaining = cap - offset;
        n = snprintf(head_buf + offset, remaining,
                     "%s: %s\r\n", w->header_names[i], w->header_values[i]);
        if (n < 0 || (size_t)n >= remaining) goto overflow;
        offset += (size_t)n;
    }
    if (!taida_net3_is_bodyless_status(w->pending_status)) {
        size_t remaining = cap - offset;
        n = snprintf(head_buf + offset, remaining, "Transfer-Encoding: chunked\r\n");
        if (n < 0 || (size_t)n >= remaining) goto overflow;
        offset += (size_t)n;
    }
    {
        size_t remaining = cap - offset;
        n = snprintf(head_buf + offset, remaining, "\r\n");
        if (n < 0 || (size_t)n >= remaining) goto overflow;
        offset += (size_t)n;
    }
    return taida_net_send_all(fd, head_buf, offset);

overflow:
    fprintf(stderr, "commit_head: response head exceeds %d bytes (too many or too large headers)\n",
            (int)NET3_HEAD_BUF_SIZE);
    return -2;
}

// Validate reserved headers (Content-Length, Transfer-Encoding) in streaming path.
// Returns 0 if valid, prints error to stderr and returns -1 if invalid.
static int taida_net3_validate_reserved_headers(taida_val headers, const char *api_name) {
    if (!TAIDA_IS_LIST(headers)) return 0;
    taida_val *list = (taida_val*)headers;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (!taida_is_buchi_pack(item)) continue;
        taida_val name_val = taida_pack_get(item, taida_str_hash((taida_val)"name"));
        if (name_val == 0) continue;
        const char *name_str = (const char*)name_val;
        size_t name_len = 0;
        if (!taida_read_cstr_len_safe(name_str, 256, &name_len)) continue;
        // Case-insensitive comparison
        if (name_len == 14) {
            // "content-length" (14 chars)
            char lower[15];
            for (size_t j = 0; j < name_len; j++) lower[j] = (char)((name_str[j] >= 'A' && name_str[j] <= 'Z') ? name_str[j] + 32 : name_str[j]);
            lower[name_len] = '\0';
            if (strcmp(lower, "content-length") == 0) {
                fprintf(stderr, "%s: 'Content-Length' is not allowed in streaming response headers. "
                        "The runtime manages Content-Length/Transfer-Encoding for streaming responses.\n", api_name);
                return -1;
            }
        }
        if (name_len == 17) {
            // "transfer-encoding" (17 chars)
            char lower[18];
            for (size_t j = 0; j < name_len; j++) lower[j] = (char)((name_str[j] >= 'A' && name_str[j] <= 'Z') ? name_str[j] + 32 : name_str[j]);
            lower[name_len] = '\0';
            if (strcmp(lower, "transfer-encoding") == 0) {
                fprintf(stderr, "%s: 'Transfer-Encoding' is not allowed in streaming response headers. "
                        "The runtime manages Transfer-Encoding for streaming responses.\n", api_name);
                return -1;
            }
        }
    }
    return 0;
}

// Extract headers from a taida list of @(name, value) packs into the writer state.
static void taida_net3_extract_headers(Net3WriterState *w, taida_val headers) {
    w->header_count = 0;
    if (!TAIDA_IS_LIST(headers)) return;
    taida_val *list = (taida_val*)headers;
    taida_val len = list[2];
    for (taida_val i = 0; i < len && w->header_count < NET3_MAX_HEADERS; i++) {
        taida_val item = list[4 + i];
        if (!taida_is_buchi_pack(item)) continue;
        taida_val name_val = taida_pack_get(item, taida_str_hash((taida_val)"name"));
        taida_val value_val = taida_pack_get(item, taida_str_hash((taida_val)"value"));
        if (name_val == 0 || value_val == 0) continue;
        w->header_names[w->header_count] = (const char*)name_val;
        w->header_values[w->header_count] = (const char*)value_val;
        w->header_count++;
    }
}

// NET3-5b: startResponse(writer, status, headers)
// Updates pending status/headers on the writer state. Does NOT commit to wire.
taida_val taida_net_start_response(taida_val writer, taida_val status, taida_val headers) {
    taida_net3_validate_writer(writer, "startResponse");
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "startResponse: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    // State check
    switch (w->state) {
        case NET3_STATE_IDLE: break;
        case NET3_STATE_HEAD_PREPARED:
            fprintf(stderr, "startResponse: already called. Cannot call startResponse twice.\n");
            exit(1);
        case NET3_STATE_STREAMING:
            fprintf(stderr, "startResponse: head already committed (chunks are being written). Cannot change status/headers after writeChunk.\n");
            exit(1);
        case NET3_STATE_ENDED:
            fprintf(stderr, "startResponse: response already ended.\n");
            exit(1);
    }
    // Validate status range
    if (status < 100 || status > 599) {
        fprintf(stderr, "startResponse: status must be 100-599, got %lld\n", (long long)status);
        exit(1);
    }
    // Validate reserved headers
    if (taida_net3_validate_reserved_headers(headers, "startResponse") < 0) {
        exit(1);
    }
    w->pending_status = (int)status;
    taida_net3_extract_headers(w, headers);
    w->state = NET3_STATE_HEAD_PREPARED;
    return 0; // Unit
}

// NET3-5b/5c/5d: writeChunk(writer, data)
// Sends one chunk of body data using chunked TE. Uses writev() for zero-copy.
// Bytes: extract from taida_val array to stack/stack-heap buffer, then writev.
// Str: use C string directly.
taida_val taida_net_write_chunk(taida_val writer, taida_val data) {
    taida_net3_validate_writer(writer, "writeChunk");
    Net3WriterState *w = tl_net3_writer;
    int fd = tl_net3_client_fd;
    if (!w) {
        fprintf(stderr, "writeChunk: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    if (w->state == NET3_STATE_ENDED) {
        fprintf(stderr, "writeChunk: response already ended.\n");
        exit(1);
    }

    // Extract payload pointer and length
    const unsigned char *payload = NULL;
    size_t payload_len = 0;
    // NET3-5d: For Bytes, we need to convert from taida_val array to contiguous bytes.
    // Use stack buffer for small payloads, heap only for large ones. No per-chunk persistent alloc.
    unsigned char stack_payload[4096];
    unsigned char *heap_payload = NULL;
    int is_bytes = 0;

    if (TAIDA_IS_BYTES(data)) {
        is_bytes = 1;
        taida_val *bytes = (taida_val*)data;
        taida_val blen = bytes[1];
        payload_len = (size_t)blen;
        if (payload_len == 0) return 0; // empty chunk is no-op
        if (payload_len <= sizeof(stack_payload)) {
            for (size_t i = 0; i < payload_len; i++) stack_payload[i] = (unsigned char)bytes[2 + i];
            payload = stack_payload;
        } else {
            heap_payload = (unsigned char*)TAIDA_MALLOC(payload_len, "net3_write_chunk_bytes");
            for (size_t i = 0; i < payload_len; i++) heap_payload[i] = (unsigned char)bytes[2 + i];
            payload = heap_payload;
        }
    } else {
        // Assume Str (C string)
        const char *str = (const char*)data;
        size_t slen = 0;
        if (!taida_read_cstr_len_safe(str, 16 * 1024 * 1024, &slen)) {
            fprintf(stderr, "writeChunk: data must be Bytes or Str\n");
            if (heap_payload) free(heap_payload);
            exit(1);
        }
        payload = (const unsigned char*)str;
        payload_len = slen;
        if (payload_len == 0) return 0; // empty chunk is no-op
    }

    // Bodyless status check
    if (taida_net3_is_bodyless_status(w->pending_status)) {
        fprintf(stderr, "writeChunk: status %d does not allow a message body\n", w->pending_status);
        if (heap_payload) free(heap_payload);
        exit(1);
    }

    // Commit head if not yet committed
    if (w->state == NET3_STATE_IDLE || w->state == NET3_STATE_HEAD_PREPARED) {
        if (taida_net3_commit_head(fd, w) != 0) {
            fprintf(stderr, "writeChunk: failed to commit response head\n");
            if (heap_payload) free(heap_payload);
            exit(1);
        }
        w->state = NET3_STATE_STREAMING;
    }

    // NET3-5c: Send chunk using writev() — zero-copy for payload.
    // Wire format: <hex-size>\r\n<payload>\r\n
    char hex_prefix[32];
    int hex_len = snprintf(hex_prefix, sizeof(hex_prefix), "%zx\r\n", payload_len);

    struct iovec iov[3];
    iov[0].iov_base = hex_prefix;
    iov[0].iov_len = (size_t)hex_len;
    iov[1].iov_base = (void*)payload;
    iov[1].iov_len = payload_len;
    iov[2].iov_base = (void*)"\r\n";
    iov[2].iov_len = 2;

    // NB3-5: Check writev_all return value for write errors (e.g. peer RST).
    if (taida_net_writev_all(fd, iov, 3) != 0) {
        if (heap_payload) free(heap_payload);
        fprintf(stderr, "writeChunk: failed to send chunk data\n");
        exit(1);
    }

    if (heap_payload) free(heap_payload);
    return 0; // Unit
}

// NET3-5b: endResponse(writer)
// Terminates the chunked response by sending 0\r\n\r\n.
// Idempotent: second call is a no-op.
taida_val taida_net_end_response(taida_val writer) {
    taida_net3_validate_writer(writer, "endResponse");
    Net3WriterState *w = tl_net3_writer;
    int fd = tl_net3_client_fd;
    if (!w) {
        fprintf(stderr, "endResponse: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    // Idempotent: no-op if already ended
    if (w->state == NET3_STATE_ENDED) return 0;

    // Commit head if not yet committed
    if (w->state == NET3_STATE_IDLE || w->state == NET3_STATE_HEAD_PREPARED) {
        if (taida_net3_commit_head(fd, w) != 0) {
            fprintf(stderr, "endResponse: failed to commit response head\n");
            exit(1);
        }
    }

    // Send chunked terminator — but only for non-bodyless status
    if (!taida_net3_is_bodyless_status(w->pending_status)) {
        taida_net_send_all(fd, "0\r\n\r\n", 5);
    }
    w->state = NET3_STATE_ENDED;
    return 0; // Unit
}

// NET3-5e: sseEvent(writer, event, data)
// SSE convenience API. Sends one Server-Sent Event.
// Auto-sets Content-Type and Cache-Control headers if not already set.
// Splits multiline data into data: lines.
taida_val taida_net_sse_event(taida_val writer, taida_val event, taida_val data) {
    taida_net3_validate_writer(writer, "sseEvent");
    Net3WriterState *w = tl_net3_writer;
    int fd = tl_net3_client_fd;
    if (!w) {
        fprintf(stderr, "sseEvent: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    // Validate event and data are strings.
    // NB3-8: Use taida_str_byte_len which reads heap string length from header
    // metadata instead of scanning for NUL. This is correct for non-ASCII
    // (multi-byte UTF-8) strings and avoids parity issues with Interpreter/JS.
    const char *event_str = (const char*)event;
    const char *data_str = (const char*)data;
    size_t event_len = 0, data_len = 0;
    if (!taida_str_byte_len(event_str, &event_len)) {
        fprintf(stderr, "sseEvent: event must be Str\n");
        exit(1);
    }
    if (!taida_str_byte_len(data_str, &data_len)) {
        fprintf(stderr, "sseEvent: data must be Str\n");
        exit(1);
    }

    if (w->state == NET3_STATE_ENDED) {
        fprintf(stderr, "sseEvent: response already ended.\n");
        exit(1);
    }
    if (taida_net3_is_bodyless_status(w->pending_status)) {
        fprintf(stderr, "sseEvent: status %d does not allow a message body\n", w->pending_status);
        exit(1);
    }

    // SSE auto-headers (once per writer)
    if (!w->sse_mode) {
        if (w->state == NET3_STATE_STREAMING) {
            // Head already committed — check if SSE headers were set
            int has_ct = 0, has_cc = 0;
            for (int i = 0; i < w->header_count; i++) {
                const char *n = w->header_names[i];
                size_t nlen = 0;
                if (!taida_read_cstr_len_safe(n, 256, &nlen)) continue;
                // Case-insensitive check
                if (nlen == 12) {
                    char lower[13];
                    for (size_t j = 0; j < nlen; j++) lower[j] = (char)((n[j] >= 'A' && n[j] <= 'Z') ? n[j] + 32 : n[j]);
                    lower[nlen] = '\0';
                    if (strcmp(lower, "content-type") == 0) {
                        const char *v = w->header_values[i];
                        size_t vlen = 0;
                        if (taida_read_cstr_len_safe(v, 256, &vlen)) {
                            char lv[256];
                            for (size_t j = 0; j < vlen && j < 255; j++) lv[j] = (char)((v[j] >= 'A' && v[j] <= 'Z') ? v[j] + 32 : v[j]);
                            lv[vlen < 255 ? vlen : 255] = '\0';
                            if (strstr(lv, "text/event-stream")) has_ct = 1;
                        }
                    }
                }
                if (nlen == 13) {
                    char lower[14];
                    for (size_t j = 0; j < nlen; j++) lower[j] = (char)((n[j] >= 'A' && n[j] <= 'Z') ? n[j] + 32 : n[j]);
                    lower[nlen] = '\0';
                    if (strcmp(lower, "cache-control") == 0) {
                        const char *v = w->header_values[i];
                        size_t vlen = 0;
                        if (taida_read_cstr_len_safe(v, 256, &vlen)) {
                            char lv[256];
                            for (size_t j = 0; j < vlen && j < 255; j++) lv[j] = (char)((v[j] >= 'A' && v[j] <= 'Z') ? v[j] + 32 : v[j]);
                            lv[vlen < 255 ? vlen : 255] = '\0';
                            if (strstr(lv, "no-cache")) has_cc = 1;
                        }
                    }
                }
            }
            if (!has_ct || !has_cc) {
                fprintf(stderr, "sseEvent: head already committed without SSE headers. "
                        "Call sseEvent before writeChunk, or use startResponse "
                        "with explicit Content-Type: text/event-stream and "
                        "Cache-Control: no-cache headers before writeChunk.\n");
                exit(1);
            }
            w->sse_mode = 1;
        } else {
            // Head not yet committed — safe to add auto-headers
            int has_ct = 0, has_cc = 0;
            for (int i = 0; i < w->header_count; i++) {
                const char *n = w->header_names[i];
                size_t nlen = 0;
                if (!taida_read_cstr_len_safe(n, 256, &nlen)) continue;
                char lower[256];
                for (size_t j = 0; j < nlen && j < 255; j++) lower[j] = (char)((n[j] >= 'A' && n[j] <= 'Z') ? n[j] + 32 : n[j]);
                lower[nlen < 255 ? nlen : 255] = '\0';
                if (strcmp(lower, "content-type") == 0) has_ct = 1;
                if (strcmp(lower, "cache-control") == 0) has_cc = 1;
            }
            if (!has_ct && w->header_count < NET3_MAX_HEADERS) {
                w->header_names[w->header_count] = "Content-Type";
                w->header_values[w->header_count] = "text/event-stream; charset=utf-8";
                w->header_count++;
            }
            if (!has_cc && w->header_count < NET3_MAX_HEADERS) {
                w->header_names[w->header_count] = "Cache-Control";
                w->header_values[w->header_count] = "no-cache";
                w->header_count++;
            }
            w->sse_mode = 1;
        }
    }

    // Commit head if not yet committed
    if (w->state == NET3_STATE_IDLE || w->state == NET3_STATE_HEAD_PREPARED) {
        if (taida_net3_commit_head(fd, w) != 0) {
            fprintf(stderr, "sseEvent: failed to commit response head\n");
            exit(1);
        }
        w->state = NET3_STATE_STREAMING;
    }

    // Build SSE event payload and compute total length.
    // Wire format:
    //   event: <event>\n      (omit if event is empty)
    //   data: <line1>\n
    //   data: <line2>\n       (for each line in data split by \n)
    //   \n                    (event terminator)

    // Count data lines
    int line_count = 1;
    for (size_t i = 0; i < data_len; i++) {
        if (data_str[i] == '\n') line_count++;
    }

    // Compute total payload length for chunk header
    size_t total_payload = 0;
    if (event_len > 0) {
        total_payload += 7 + event_len + 1; // "event: " + event + "\n"
    }
    // For each data line: "data: " + line + "\n"
    {
        const char *p = data_str;
        const char *end = data_str + data_len;
        while (p <= end) {
            const char *nl = p;
            while (nl < end && *nl != '\n') nl++;
            size_t line_len = (size_t)(nl - p);
            total_payload += 6 + line_len + 1; // "data: " + line + "\n"
            p = nl + 1;
            if (nl == end) break;
        }
    }
    total_payload += 1; // terminator "\n"

    // Build chunk: hex_prefix + SSE payload + chunk suffix
    char hex_prefix[32];
    int hex_len = snprintf(hex_prefix, sizeof(hex_prefix), "%zx\r\n", total_payload);

    // Use iov array. Max iovecs: 1(hex) + 3(event line) + 3*line_count(data lines) + 1(term) + 1(suffix)
    int max_iov = 1 + 3 + 3 * line_count + 1 + 1;
    // Use stack for small SSE events, heap for large
    struct iovec stack_iov[64];
    struct iovec *iov = (max_iov <= 64) ? stack_iov : (struct iovec*)TAIDA_MALLOC(sizeof(struct iovec) * (size_t)max_iov, "net3_sse_iov");
    int iov_count = 0;

    // hex prefix
    iov[iov_count].iov_base = hex_prefix;
    iov[iov_count].iov_len = (size_t)hex_len;
    iov_count++;

    // event: line
    if (event_len > 0) {
        iov[iov_count].iov_base = (void*)"event: ";
        iov[iov_count].iov_len = 7;
        iov_count++;
        iov[iov_count].iov_base = (void*)event_str;
        iov[iov_count].iov_len = event_len;
        iov_count++;
        iov[iov_count].iov_base = (void*)"\n";
        iov[iov_count].iov_len = 1;
        iov_count++;
    }

    // data: lines
    {
        const char *p = data_str;
        const char *end = data_str + data_len;
        while (p <= end) {
            const char *nl = p;
            while (nl < end && *nl != '\n') nl++;
            size_t line_len = (size_t)(nl - p);
            iov[iov_count].iov_base = (void*)"data: ";
            iov[iov_count].iov_len = 6;
            iov_count++;
            if (line_len > 0) {
                iov[iov_count].iov_base = (void*)p;
                iov[iov_count].iov_len = line_len;
                iov_count++;
            }
            iov[iov_count].iov_base = (void*)"\n";
            iov[iov_count].iov_len = 1;
            iov_count++;
            p = nl + 1;
            if (nl == end) break;
        }
    }

    // event terminator
    iov[iov_count].iov_base = (void*)"\n";
    iov[iov_count].iov_len = 1;
    iov_count++;

    // chunk suffix
    iov[iov_count].iov_base = (void*)"\r\n";
    iov[iov_count].iov_len = 2;
    iov_count++;

    // NB3-5: Check writev_all return value for write errors (e.g. peer RST).
    if (taida_net_writev_all(fd, iov, iov_count) != 0) {
        if (iov != stack_iov) free(iov);
        fprintf(stderr, "sseEvent: failed to send SSE chunk data\n");
        exit(1);
    }

    if (iov != stack_iov) free(iov);

    return 0; // Unit
}

// ── NET4-4: v4 Request Body Streaming + WebSocket — Native backend ──
//
// Phase 4: Full implementation of readBodyChunk, readBodyAll,
// wsUpgrade, wsSend, wsReceive, wsClose.
// Replaces NB4-6 stubs.

// ── SHA-1 implementation (RFC 3174, ~100 lines) ─────────────
// Used exclusively for WebSocket Sec-WebSocket-Accept calculation.
// Not for cryptographic purposes.

static void taida_sha1_transform(uint32_t state[5], const uint8_t block[64]) {
    uint32_t w[80];
    for (int i = 0; i < 16; i++) {
        w[i] = ((uint32_t)block[i*4] << 24) | ((uint32_t)block[i*4+1] << 16)
             | ((uint32_t)block[i*4+2] << 8) | (uint32_t)block[i*4+3];
    }
    for (int i = 16; i < 80; i++) {
        uint32_t t = w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16];
        w[i] = (t << 1) | (t >> 31);
    }
    uint32_t a = state[0], b = state[1], c = state[2], d = state[3], e = state[4];
    for (int i = 0; i < 80; i++) {
        uint32_t f, k;
        if (i < 20)      { f = (b & c) | ((~b) & d); k = 0x5A827999; }
        else if (i < 40) { f = b ^ c ^ d;             k = 0x6ED9EBA1; }
        else if (i < 60) { f = (b & c) | (b & d) | (c & d); k = 0x8F1BBCDC; }
        else              { f = b ^ c ^ d;             k = 0xCA62C1D6; }
        uint32_t temp = ((a << 5) | (a >> 27)) + f + e + k + w[i];
        e = d; d = c; c = (b << 30) | (b >> 2); b = a; a = temp;
    }
    state[0] += a; state[1] += b; state[2] += c; state[3] += d; state[4] += e;
}

// SHA-1 hash: input -> 20-byte digest.
static void taida_sha1(const uint8_t *data, size_t len, uint8_t digest[20]) {
    uint32_t state[5] = { 0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0 };
    size_t i;
    uint8_t block[64];
    size_t block_pos = 0;

    for (i = 0; i < len; i++) {
        block[block_pos++] = data[i];
        if (block_pos == 64) {
            taida_sha1_transform(state, block);
            block_pos = 0;
        }
    }

    // Padding
    block[block_pos++] = 0x80;
    if (block_pos > 56) {
        while (block_pos < 64) block[block_pos++] = 0;
        taida_sha1_transform(state, block);
        block_pos = 0;
    }
    while (block_pos < 56) block[block_pos++] = 0;

    // Length in bits (big-endian 64-bit)
    uint64_t bit_len = (uint64_t)len * 8;
    block[56] = (uint8_t)(bit_len >> 56);
    block[57] = (uint8_t)(bit_len >> 48);
    block[58] = (uint8_t)(bit_len >> 40);
    block[59] = (uint8_t)(bit_len >> 32);
    block[60] = (uint8_t)(bit_len >> 24);
    block[61] = (uint8_t)(bit_len >> 16);
    block[62] = (uint8_t)(bit_len >> 8);
    block[63] = (uint8_t)(bit_len);
    taida_sha1_transform(state, block);

    for (i = 0; i < 5; i++) {
        digest[i*4]   = (uint8_t)(state[i] >> 24);
        digest[i*4+1] = (uint8_t)(state[i] >> 16);
        digest[i*4+2] = (uint8_t)(state[i] >> 8);
        digest[i*4+3] = (uint8_t)(state[i]);
    }
}

// ── Base64 encode ──────────────────────────────────────────
static const char taida_b64_chars[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Base64 encode: input bytes -> null-terminated string (caller must free).
static char *taida_base64_encode(const uint8_t *data, size_t len) {
    size_t out_len = 4 * ((len + 2) / 3);
    char *out = (char*)TAIDA_MALLOC(out_len + 1, "net_base64");
    size_t j = 0;
    for (size_t i = 0; i < len; ) {
        uint32_t octet_a = i < len ? data[i++] : 0;
        uint32_t octet_b = i < len ? data[i++] : 0;
        uint32_t octet_c = i < len ? data[i++] : 0;
        uint32_t triple = (octet_a << 16) | (octet_b << 8) | octet_c;
        out[j++] = taida_b64_chars[(triple >> 18) & 0x3F];
        out[j++] = taida_b64_chars[(triple >> 12) & 0x3F];
        out[j++] = taida_b64_chars[(triple >> 6) & 0x3F];
        out[j++] = taida_b64_chars[triple & 0x3F];
    }
    // Padding
    size_t mod = len % 3;
    if (mod == 1) { out[j-1] = '='; out[j-2] = '='; }
    else if (mod == 2) { out[j-1] = '='; }
    out[j] = '\0';
    return out;
}

// NB4-11: Base64 decode for Sec-WebSocket-Key validation.
// Returns decoded length, or -1 on invalid input. Writes to `out` (must have enough space).
static int taida_base64_decode(const char *input, size_t input_len, uint8_t *out, size_t out_cap) {
    static const int8_t decode_table[256] = {
        [0 ... 255] = -1,
        ['A'] = 0, ['B'] = 1, ['C'] = 2, ['D'] = 3, ['E'] = 4, ['F'] = 5,
        ['G'] = 6, ['H'] = 7, ['I'] = 8, ['J'] = 9, ['K'] = 10, ['L'] = 11,
        ['M'] = 12, ['N'] = 13, ['O'] = 14, ['P'] = 15, ['Q'] = 16, ['R'] = 17,
        ['S'] = 18, ['T'] = 19, ['U'] = 20, ['V'] = 21, ['W'] = 22, ['X'] = 23,
        ['Y'] = 24, ['Z'] = 25,
        ['a'] = 26, ['b'] = 27, ['c'] = 28, ['d'] = 29, ['e'] = 30, ['f'] = 31,
        ['g'] = 32, ['h'] = 33, ['i'] = 34, ['j'] = 35, ['k'] = 36, ['l'] = 37,
        ['m'] = 38, ['n'] = 39, ['o'] = 40, ['p'] = 41, ['q'] = 42, ['r'] = 43,
        ['s'] = 44, ['t'] = 45, ['u'] = 46, ['v'] = 47, ['w'] = 48, ['x'] = 49,
        ['y'] = 50, ['z'] = 51,
        ['0'] = 52, ['1'] = 53, ['2'] = 54, ['3'] = 55, ['4'] = 56, ['5'] = 57,
        ['6'] = 58, ['7'] = 59, ['8'] = 60, ['9'] = 61,
        ['+'] = 62, ['/'] = 63
    };
    if (input_len % 4 != 0) return -1;
    size_t decoded_len = input_len / 4 * 3;
    if (input_len > 0 && input[input_len - 1] == '=') decoded_len--;
    if (input_len > 1 && input[input_len - 2] == '=') decoded_len--;
    if (decoded_len > out_cap) return -1;

    size_t j = 0;
    for (size_t i = 0; i < input_len; i += 4) {
        int8_t a = decode_table[(unsigned char)input[i]];
        int8_t b = (i + 1 < input_len) ? decode_table[(unsigned char)input[i + 1]] : -1;
        if (a < 0 || b < 0) return -1;
        uint32_t triple = ((uint32_t)a << 18) | ((uint32_t)b << 12);
        if (i + 2 < input_len && input[i + 2] != '=') {
            int8_t c = decode_table[(unsigned char)input[i + 2]];
            if (c < 0) return -1;
            triple |= ((uint32_t)c << 6);
        }
        if (i + 3 < input_len && input[i + 3] != '=') {
            int8_t d = decode_table[(unsigned char)input[i + 3]];
            if (d < 0) return -1;
            triple |= (uint32_t)d;
        }
        if (j < decoded_len) out[j++] = (uint8_t)(triple >> 16);
        if (j < decoded_len) out[j++] = (uint8_t)(triple >> 8);
        if (j < decoded_len) out[j++] = (uint8_t)triple;
    }
    return (int)decoded_len;
}

// ── Compute Sec-WebSocket-Accept (NET4-4b) ──────────────────
// SHA-1(key + GUID) -> Base64
static const char *WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

static char *taida_net4_compute_ws_accept(const char *key) {
    // Concatenate key + GUID
    size_t key_len = strlen(key);
    size_t guid_len = strlen(WS_GUID);
    size_t total = key_len + guid_len;
    uint8_t *combined = (uint8_t*)TAIDA_MALLOC(total + 1, "net_ws_accept");
    memcpy(combined, key, key_len);
    memcpy(combined + key_len, WS_GUID, guid_len);

    uint8_t digest[20];
    taida_sha1(combined, total, digest);
    free(combined);

    return taida_base64_encode(digest, 20);
}

// ── WebSocket constants ──────────────────────────────────────
#define WS_OPCODE_TEXT   0x1
#define WS_OPCODE_BINARY 0x2
#define WS_OPCODE_CLOSE  0x8
#define WS_OPCODE_PING   0x9
#define WS_OPCODE_PONG   0xA
#define WS_MAX_PAYLOAD   (16ULL * 1024 * 1024)  // 16 MiB

// ── v4 Body streaming helpers ────────────────────────────────

// Read exactly `count` bytes from fd. Returns bytes read, 0 on error/EOF.
// NET5-4a: Routes through TLS when tl_ssl is active.
static size_t taida_net4_recv_exact(int fd, unsigned char *out, size_t count) {
    return taida_tls_recv_exact(fd, out, count);
}

// Read up to `count` bytes from leftover then fd.
// Returns a new Bytes object (caller's ownership), or empty Bytes on EOF.
// NET5-4a: Routes through TLS when tl_ssl is active.
static size_t taida_net4_read_body_bytes(Net4BodyState *bs, int fd, unsigned char *out, size_t count) {
    size_t total = 0;
    // First, drain from leftover.
    while (total < count && bs->leftover_pos < bs->leftover_len) {
        out[total++] = bs->leftover[bs->leftover_pos++];
    }
    // Then read from socket (TLS-aware).
    while (total < count) {
        ssize_t n = taida_tls_recv(fd, out + total, count - total);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            break; // EOF or error
        }
        total += (size_t)n;
    }
    return total;
}

// Read a line (up to LF) from leftover then fd.
// Returns line in `out` (null-terminated). Max `cap` bytes including NUL.
// Returns length excluding NUL.
// NET5-4a: Routes through TLS when tl_ssl is active.
static size_t taida_net4_read_line(Net4BodyState *bs, int fd, char *out, size_t cap) {
    size_t pos = 0;
    // From leftover.
    while (pos < cap - 1 && bs->leftover_pos < bs->leftover_len) {
        unsigned char b = bs->leftover[bs->leftover_pos++];
        out[pos++] = (char)b;
        if (b == '\n') { out[pos] = '\0'; return pos; }
    }
    // From socket byte-by-byte (TLS-aware).
    while (pos < cap - 1) {
        unsigned char b;
        ssize_t n = taida_tls_recv(fd, &b, 1);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            break;
        }
        out[pos++] = (char)b;
        if (b == '\n') break;
    }
    out[pos] = '\0';
    return pos;
}

// Drain chunked trailers after terminal chunk (NB4-8 parity).
// Returns 0 on success, -1 on protocol error (missing final CRLF).
static int taida_net4_drain_chunked_trailers(Net4BodyState *bs, int fd) {
    char line[4096];
    for (int i = 0; i < 64; i++) {
        size_t len = taida_net4_read_line(bs, fd, line, sizeof(line));
        // NB4-18: EOF (0 raw bytes) != valid empty line ("\r\n").
        if (len == 0) {
            fprintf(stderr, "chunked body error: missing final CRLF after terminal chunk\n");
            return -1;
        }
        // Trim whitespace and check empty.
        size_t start = 0, end = len;
        while (start < end && (line[start] == ' ' || line[start] == '\t' || line[start] == '\r' || line[start] == '\n')) start++;
        while (end > start && (line[end-1] == ' ' || line[end-1] == '\t' || line[end-1] == '\r' || line[end-1] == '\n')) end--;
        if (start == end) return 0; // Empty line = trailers done.
    }
    return 0;
}

// Make Lax[Bytes] empty (parity with Interpreter: hasValue=false).
static taida_val taida_net4_make_lax_bytes_empty(void) {
    return taida_lax_empty(taida_bytes_default_value());
}

// Make Lax[Bytes] with value (parity with Interpreter: hasValue=true).
static taida_val taida_net4_make_lax_bytes_value(const unsigned char *data, size_t len) {
    taida_val bytes = taida_bytes_from_raw(data, (taida_val)len);
    return taida_lax_new(bytes, taida_bytes_default_value());
}

// Validate that req is a body-streaming request pack.
static int taida_net4_is_body_stream_request(taida_val req) {
    if (!taida_is_buchi_pack(req)) return 0;
    taida_val sentinel = taida_pack_get(req, taida_str_hash((taida_val)"__body_stream"));
    if (sentinel == 0) return 0;
    const char *s = (const char*)sentinel;
    size_t slen = 0;
    if (!taida_read_cstr_len_safe(s, 64, &slen)) return 0;
    return (slen == 16 && memcmp(s, "__v4_body_stream", 16) == 0);
}

// Extract __body_token from request pack.
static uint64_t taida_net4_extract_body_token(taida_val req) {
    return (uint64_t)taida_pack_get(req, taida_str_hash((taida_val)"__body_token"));
}

// ── readBodyChunk(req) → Lax[Bytes] ─────────────────────────
taida_val taida_net_read_body_chunk(taida_val req) {
    if (!taida_net4_is_body_stream_request(req)) {
        fprintf(stderr, "readBodyChunk: can only be called in a 2-argument httpServe handler. "
                "In a 1-argument handler, the request body is already fully read. "
                "Use readBody(req) instead.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (!bs) {
        fprintf(stderr, "readBodyChunk: no active body streaming state\n");
        exit(1);
    }

    // NB4-7: Verify request token.
    uint64_t tok = taida_net4_extract_body_token(req);
    if (tok != bs->request_token) {
        fprintf(stderr, "readBodyChunk: request pack does not match the current active request. "
                "The request may be stale or fabricated.\n");
        exit(1);
    }

    Net3WriterState *w = tl_net3_writer;
    if (w && w->state == NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "readBodyChunk: cannot read HTTP body after WebSocket upgrade.\n");
        exit(1);
    }

    int fd = tl_net3_client_fd;

    bs->any_read_started = 1;

    if (bs->fully_read) {
        return taida_net4_make_lax_bytes_empty();
    }

    if (bs->is_chunked) {
        // Chunked TE decode (parity with Interpreter).
        #define NET4_READ_BUF 8192
        char line_buf[4096];
        for (;;) {
            switch (bs->chunked_state) {
                case NET4_CHUNKED_DONE:
                    bs->fully_read = 1;
                    return taida_net4_make_lax_bytes_empty();

                case NET4_CHUNKED_WAIT_SIZE: {
                    size_t llen = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                    // Trim.
                    size_t s = 0, e = llen;
                    while (s < e && (line_buf[s]==' '||line_buf[s]=='\t'||line_buf[s]=='\r'||line_buf[s]=='\n')) s++;
                    while (e > s && (line_buf[e-1]==' '||line_buf[e-1]=='\t'||line_buf[e-1]=='\r'||line_buf[e-1]=='\n')) e--;
                    if (s == e) continue; // Empty line, try again.
                    // Parse hex chunk-size (strip chunk-extension after ';').
                    char hex_buf[64];
                    size_t hex_len = 0;
                    for (size_t i = s; i < e && line_buf[i] != ';' && hex_len < 63; i++) {
                        if (line_buf[i] != ' ' && line_buf[i] != '\t')
                            hex_buf[hex_len++] = line_buf[i];
                    }
                    hex_buf[hex_len] = '\0';
                    // NB4-18: Strict hex-only parse. Reject partial parse like '1g'.
                    for (size_t vi = 0; vi < hex_len; vi++) {
                        char c = hex_buf[vi];
                        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) {
                            fprintf(stderr, "readBodyChunk: invalid chunk-size '%s' in chunked body\n", hex_buf);
                            exit(1);
                        }
                    }
                    if (hex_len == 0) continue; // skip empty, retry
                    unsigned long chunk_size = strtoul(hex_buf, NULL, 16);
                    if (chunk_size == 0) {
                        bs->chunked_state = NET4_CHUNKED_DONE;
                        bs->fully_read = 1;
                        if (taida_net4_drain_chunked_trailers(bs, fd) < 0) {
                            bs->fully_read = 0;
                            fprintf(stderr, "readBodyChunk: chunked body protocol error\n");
                            exit(1);
                        }
                        return taida_net4_make_lax_bytes_empty();
                    }
                    bs->chunked_state = NET4_CHUNKED_READ_DATA;
                    bs->chunked_remaining = (size_t)chunk_size;
                    break;
                }

                case NET4_CHUNKED_READ_DATA: {
                    if (bs->chunked_remaining == 0) {
                        bs->chunked_state = NET4_CHUNKED_WAIT_TRAILER;
                        continue;
                    }
                    size_t to_read = bs->chunked_remaining;
                    if (to_read > NET4_READ_BUF) to_read = NET4_READ_BUF;
                    unsigned char tmp[NET4_READ_BUF];
                    size_t got = taida_net4_read_body_bytes(bs, fd, tmp, to_read);
                    // NB4-18: short read (EOF) in chunked data is a protocol error.
                    if (got == 0) {
                        fprintf(stderr, "readBodyChunk: truncated chunked body — expected %zu more chunk-data bytes but got EOF\n",
                                bs->chunked_remaining);
                        exit(1);
                    }
                    bs->chunked_remaining -= got;
                    bs->bytes_consumed += (int64_t)got;
                    return taida_net4_make_lax_bytes_value(tmp, got);
                }

                case NET4_CHUNKED_WAIT_TRAILER: {
                    // NB4-18: Read CRLF after chunk data and validate.
                    {
                        size_t tl_len = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                        if (tl_len == 0) {
                            fprintf(stderr, "readBodyChunk: missing CRLF after chunk data (unexpected EOF)\n");
                            exit(1);
                        }
                        // Trim and check empty.
                        size_t ts = 0, te = tl_len;
                        while (ts < te && (line_buf[ts]==' '||line_buf[ts]=='\t'||line_buf[ts]=='\r'||line_buf[ts]=='\n')) ts++;
                        while (te > ts && (line_buf[te-1]==' '||line_buf[te-1]=='\t'||line_buf[te-1]=='\r'||line_buf[te-1]=='\n')) te--;
                        if (ts != te) {
                            line_buf[tl_len < sizeof(line_buf)-1 ? tl_len : sizeof(line_buf)-1] = '\0';
                            fprintf(stderr, "readBodyChunk: malformed chunk trailer — expected CRLF after chunk data, got \"%s\"\n", line_buf);
                            exit(1);
                        }
                    }
                    bs->chunked_state = NET4_CHUNKED_WAIT_SIZE;
                    break;
                }
            }
        }
        #undef NET4_READ_BUF
    } else {
        // Content-Length path.
        int64_t remaining = bs->content_length - bs->bytes_consumed;
        if (remaining <= 0) {
            bs->fully_read = 1;
            return taida_net4_make_lax_bytes_empty();
        }
        size_t to_read = (size_t)remaining;
        if (to_read > 8192) to_read = 8192;
        unsigned char tmp[8192];
        size_t got = taida_net4_read_body_bytes(bs, fd, tmp, to_read);
        if (got == 0) {
            // NB4-18: EOF before Content-Length exhausted is a protocol error.
            fprintf(stderr, "readBodyChunk: truncated body — expected %" PRId64
                    " bytes (Content-Length) but got EOF after %" PRId64 " bytes\n",
                    bs->content_length, bs->bytes_consumed);
            exit(1);
        }
        bs->bytes_consumed += (int64_t)got;
        if (bs->bytes_consumed >= bs->content_length) {
            bs->fully_read = 1;
        }
        return taida_net4_make_lax_bytes_value(tmp, got);
    }
}

// ── readBodyAll(req) → Bytes ─────────────────────────────────
// The only aggregate path permitted by v4 contract.
taida_val taida_net_read_body_all(taida_val req) {
    if (!taida_net4_is_body_stream_request(req)) {
        fprintf(stderr, "readBodyAll: can only be called in a 2-argument httpServe handler. "
                "In a 1-argument handler, the request body is already fully read. "
                "Use readBody(req) instead.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (!bs) {
        fprintf(stderr, "readBodyAll: no active body streaming state\n");
        exit(1);
    }

    // NB4-7: Verify request token.
    uint64_t tok = taida_net4_extract_body_token(req);
    if (tok != bs->request_token) {
        fprintf(stderr, "readBodyAll: request pack does not match the current active request.\n");
        exit(1);
    }

    Net3WriterState *w = tl_net3_writer;
    if (w && w->state == NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "readBodyAll: cannot read HTTP body after WebSocket upgrade.\n");
        exit(1);
    }

    int fd = tl_net3_client_fd;

    bs->any_read_started = 1;

    if (bs->fully_read) {
        return taida_bytes_new_filled(0, 0);
    }

    // Aggregate all remaining body bytes (this is the only permitted aggregate path).
    size_t all_cap = 4096;
    size_t all_len = 0;
    unsigned char *all_buf = (unsigned char*)TAIDA_MALLOC(all_cap, "net_readBodyAll");

    if (bs->is_chunked) {
        char line_buf[4096];
        for (;;) {
            switch (bs->chunked_state) {
                case NET4_CHUNKED_DONE:
                    bs->fully_read = 1;
                    goto all_done;

                case NET4_CHUNKED_WAIT_SIZE: {
                    size_t llen = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                    size_t s = 0, e = llen;
                    while (s < e && (line_buf[s]==' '||line_buf[s]=='\t'||line_buf[s]=='\r'||line_buf[s]=='\n')) s++;
                    while (e > s && (line_buf[e-1]==' '||line_buf[e-1]=='\t'||line_buf[e-1]=='\r'||line_buf[e-1]=='\n')) e--;
                    if (s == e) continue;
                    char hex_buf[64];
                    size_t hex_len = 0;
                    for (size_t i = s; i < e && line_buf[i] != ';' && hex_len < 63; i++) {
                        if (line_buf[i] != ' ' && line_buf[i] != '\t')
                            hex_buf[hex_len++] = line_buf[i];
                    }
                    hex_buf[hex_len] = '\0';
                    // NB4-18: Strict hex-only parse. Reject partial parse like '1g'.
                    for (size_t vi = 0; vi < hex_len; vi++) {
                        char c = hex_buf[vi];
                        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) {
                            fprintf(stderr, "readBodyChunk: invalid chunk-size '%s' in chunked body\n", hex_buf);
                            exit(1);
                        }
                    }
                    if (hex_len == 0) continue; // skip empty, retry
                    unsigned long chunk_size = strtoul(hex_buf, NULL, 16);
                    if (chunk_size == 0) {
                        bs->chunked_state = NET4_CHUNKED_DONE;
                        bs->fully_read = 1;
                        if (taida_net4_drain_chunked_trailers(bs, fd) < 0) {
                            bs->fully_read = 0;
                            fprintf(stderr, "readBodyAll: chunked body protocol error\n");
                            exit(1);
                        }
                        goto all_done;
                    }
                    bs->chunked_state = NET4_CHUNKED_READ_DATA;
                    bs->chunked_remaining = (size_t)chunk_size;
                    break;
                }

                case NET4_CHUNKED_READ_DATA: {
                    if (bs->chunked_remaining == 0) {
                        bs->chunked_state = NET4_CHUNKED_WAIT_TRAILER;
                        continue;
                    }
                    // Ensure capacity.
                    while (all_len + bs->chunked_remaining > all_cap) {
                        all_cap *= 2;
                        TAIDA_REALLOC(all_buf, all_cap, "net_readBodyAll_grow");
                    }
                    size_t got = taida_net4_read_body_bytes(bs, fd, all_buf + all_len, bs->chunked_remaining);
                    // NB4-18: short read (EOF) in chunked data is a protocol error.
                    if (got == 0) {
                        fprintf(stderr, "readBodyAll: truncated chunked body — expected %zu more chunk-data bytes but got EOF\n",
                                bs->chunked_remaining);
                        free(all_buf);
                        exit(1);
                    }
                    all_len += got;
                    size_t new_rem = bs->chunked_remaining - got;
                    bs->chunked_remaining = new_rem;
                    break;
                }

                case NET4_CHUNKED_WAIT_TRAILER: {
                    // NB4-18: Read CRLF after chunk data and validate.
                    {
                        size_t tl_len2 = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                        if (tl_len2 == 0) {
                            fprintf(stderr, "readBodyAll: missing CRLF after chunk data (unexpected EOF)\n");
                            free(all_buf);
                            exit(1);
                        }
                        size_t ts2 = 0, te2 = tl_len2;
                        while (ts2 < te2 && (line_buf[ts2]==' '||line_buf[ts2]=='\t'||line_buf[ts2]=='\r'||line_buf[ts2]=='\n')) ts2++;
                        while (te2 > ts2 && (line_buf[te2-1]==' '||line_buf[te2-1]=='\t'||line_buf[te2-1]=='\r'||line_buf[te2-1]=='\n')) te2--;
                        if (ts2 != te2) {
                            line_buf[tl_len2 < sizeof(line_buf)-1 ? tl_len2 : sizeof(line_buf)-1] = '\0';
                            fprintf(stderr, "readBodyAll: malformed chunk trailer — expected CRLF after chunk data, got \"%s\"\n", line_buf);
                            free(all_buf);
                            exit(1);
                        }
                    }
                    bs->chunked_state = NET4_CHUNKED_WAIT_SIZE;
                    break;
                }
            }
        }
    } else {
        // Content-Length path.
        int64_t remaining = bs->content_length - bs->bytes_consumed;
        if (remaining > 0) {
            size_t to_read = (size_t)remaining;
            if (to_read > all_cap) {
                all_cap = to_read;
                TAIDA_REALLOC(all_buf, all_cap, "net_readBodyAll_cl");
            }
            size_t got = taida_net4_read_body_bytes(bs, fd, all_buf, to_read);
            // NB4-18: EOF before Content-Length exhausted is a protocol error.
            if (got == 0 && to_read > 0) {
                fprintf(stderr, "readBodyAll: truncated body — expected %" PRId64
                        " bytes (Content-Length) but got EOF after %" PRId64 " bytes\n",
                        bs->content_length, bs->bytes_consumed);
                free(all_buf);
                exit(1);
            }
            all_len = got;
            bs->bytes_consumed += (int64_t)got;
        }
        bs->fully_read = 1;
    }

all_done:;
    taida_val result = taida_bytes_from_raw(all_buf, (taida_val)all_len);
    free(all_buf);
    return result;
}

// ── WebSocket frame write (NET4-4c) ─────────────────────────
// Server->client: FIN=1, MASK=0. Header on stack, payload via writev.
static int taida_net4_write_ws_frame(int fd, uint8_t opcode, const unsigned char *payload, size_t payload_len) {
    unsigned char header[10];
    int header_len;
    header[0] = 0x80 | opcode; // FIN=1
    if (payload_len < 126) {
        header[1] = (uint8_t)payload_len;
        header_len = 2;
    } else if (payload_len <= 65535) {
        header[1] = 126;
        header[2] = (uint8_t)(payload_len >> 8);
        header[3] = (uint8_t)(payload_len & 0xFF);
        header_len = 4;
    } else {
        header[1] = 127;
        uint64_t len64 = (uint64_t)payload_len;
        header[2] = (uint8_t)(len64 >> 56);
        header[3] = (uint8_t)(len64 >> 48);
        header[4] = (uint8_t)(len64 >> 40);
        header[5] = (uint8_t)(len64 >> 32);
        header[6] = (uint8_t)(len64 >> 24);
        header[7] = (uint8_t)(len64 >> 16);
        header[8] = (uint8_t)(len64 >> 8);
        header[9] = (uint8_t)(len64);
        header_len = 10;
    }
    // Vectored write: header + payload (no aggregate buffer).
    struct iovec iov[2];
    iov[0].iov_base = header;
    iov[0].iov_len = (size_t)header_len;
    iov[1].iov_base = (void*)payload;
    iov[1].iov_len = payload_len;
    return taida_net_writev_all(fd, iov, payload_len > 0 ? 2 : 1);
}

// ── WebSocket frame read (NET4-4c) ──────────────────────────
// Frame types returned by read_ws_frame.
#define WS_FRAME_TEXT     1
#define WS_FRAME_BINARY   2
#define WS_FRAME_PING     3
#define WS_FRAME_PONG     4
#define WS_FRAME_CLOSE    5
#define WS_FRAME_ERROR    6

typedef struct {
    int type;                // WS_FRAME_*
    unsigned char *payload;  // heap-allocated payload (caller must free)
    size_t payload_len;
    uint8_t opcode;
} WsFrameResult;

static WsFrameResult taida_net4_read_ws_frame(int fd) {
    WsFrameResult result = { WS_FRAME_ERROR, NULL, 0, 0 };
    unsigned char hdr[2];
    if (taida_net4_recv_exact(fd, hdr, 2) != 2) {
        return result;
    }
    uint8_t byte0 = hdr[0], byte1 = hdr[1];
    int fin = (byte0 & 0x80) != 0;
    uint8_t rsv = byte0 & 0x70;
    uint8_t opcode = byte0 & 0x0F;
    int masked = (byte1 & 0x80) != 0;
    uint64_t payload_len7 = byte1 & 0x7F;

    // RSV must be 0.
    if (rsv != 0) { result.type = WS_FRAME_ERROR; return result; }

    // Fragmented frames not supported.
    if (!fin) { result.type = WS_FRAME_ERROR; return result; }

    // Continuation frame without fragmentation is error.
    if (opcode == 0x0) { result.type = WS_FRAME_ERROR; return result; }

    // NB4-11: Client-to-server frames MUST be masked (RFC 6455 Section 5.1).
    if (!masked) { result.type = WS_FRAME_ERROR; return result; }

    // Determine actual payload length.
    uint64_t payload_len;
    if (payload_len7 < 126) {
        payload_len = payload_len7;
    } else if (payload_len7 == 126) {
        unsigned char ext[2];
        if (taida_net4_recv_exact(fd, ext, 2) != 2) return result;
        payload_len = ((uint64_t)ext[0] << 8) | ext[1];
    } else { // 127
        unsigned char ext[8];
        if (taida_net4_recv_exact(fd, ext, 8) != 8) return result;
        payload_len = 0;
        for (int i = 0; i < 8; i++) payload_len = (payload_len << 8) | ext[i];
        if (payload_len >> 63) { result.type = WS_FRAME_ERROR; return result; }
    }

    // Oversized payload check.
    if (payload_len > WS_MAX_PAYLOAD) { result.type = WS_FRAME_ERROR; return result; }

    // Read masking key if masked.
    uint8_t mask_key[4] = {0};
    if (masked) {
        if (taida_net4_recv_exact(fd, mask_key, 4) != 4) return result;
    }

    // Read payload.
    unsigned char *payload = NULL;
    if (payload_len > 0) {
        payload = (unsigned char*)TAIDA_MALLOC((size_t)payload_len, "net_ws_frame_payload");
        if (taida_net4_recv_exact(fd, payload, (size_t)payload_len) != (size_t)payload_len) {
            free(payload);
            return result;
        }
        // Unmask in-place.
        if (masked) {
            for (size_t i = 0; i < (size_t)payload_len; i++) {
                payload[i] ^= mask_key[i % 4];
            }
        }
    }

    result.payload = payload;
    result.payload_len = (size_t)payload_len;
    result.opcode = opcode;

    switch (opcode) {
        case WS_OPCODE_TEXT:   result.type = WS_FRAME_TEXT; break;
        case WS_OPCODE_BINARY: result.type = WS_FRAME_BINARY; break;
        case WS_OPCODE_CLOSE:  result.type = WS_FRAME_CLOSE; break;
        case WS_OPCODE_PING:   result.type = WS_FRAME_PING; break;
        case WS_OPCODE_PONG:   result.type = WS_FRAME_PONG; break;
        default:               result.type = WS_FRAME_ERROR; break;
    }
    return result;
}

// ── NB4-10: Validate WsConn token — sentinel + connection-scoped token ──
static int taida_net4_validate_ws_token(taida_val ws) {
    if (!taida_is_buchi_pack(ws)) return 0;
    // Check sentinel.
    taida_val id_val = taida_pack_get(ws, taida_str_hash((taida_val)"__ws_id"));
    if (id_val == 0) return 0;
    const char *id_str = (const char*)id_val;
    size_t id_len = 0;
    if (!taida_read_cstr_len_safe(id_str, 64, &id_len)) return 0;
    if (id_len != 19 || memcmp(id_str, "__v4_websocket_conn", 19) != 0) return 0;
    // Verify connection-scoped token matches active ws_token.
    Net4BodyState *bs = tl_net4_body;
    if (!bs || bs->ws_token == 0) return 0;
    taida_val tok_val = taida_pack_get(ws, taida_str_hash((taida_val)"__ws_token"));
    if ((uint64_t)tok_val != bs->ws_token) return 0;
    return 1;
}

// Make Lax[@(ws: WsConn)] with value.
static taida_val taida_net4_make_lax_ws_value(taida_val ws_pack) {
    taida_val inner = taida_pack_new(1);
    taida_pack_set_hash(inner, 0, taida_str_hash((taida_val)"ws"));
    taida_pack_set(inner, 0, ws_pack);
    taida_pack_set_tag(inner, 0, TAIDA_TAG_PACK);
    taida_retain(ws_pack);
    return taida_lax_new(inner, taida_pack_new(0));
}

// Make Lax empty for failed wsUpgrade.
static taida_val taida_net4_make_lax_ws_empty(void) {
    return taida_lax_empty(taida_pack_new(0));
}

// Make Lax[@(type, data)] for wsReceive data frame.
static taida_val taida_net4_make_lax_ws_frame_value(const char *type_str, taida_val data_val) {
    taida_val inner = taida_pack_new(2);
    taida_pack_set_hash(inner, 0, taida_str_hash((taida_val)"type"));
    taida_pack_set(inner, 0, (taida_val)taida_str_new_copy(type_str));
    taida_pack_set_tag(inner, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 1, taida_str_hash((taida_val)"data"));
    taida_pack_set(inner, 1, data_val);
    // Tag the data field appropriately.
    if (TAIDA_IS_BYTES(data_val)) {
        taida_pack_set_tag(inner, 1, TAIDA_TAG_PACK); // Bytes is ptr
    } else {
        taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    }
    return taida_lax_new(inner, taida_pack_new(0));
}

// Make Lax empty for wsReceive close / end of stream.
static taida_val taida_net4_make_lax_ws_frame_empty(void) {
    return taida_lax_empty(taida_pack_new(0));
}

// ── Helper: extract header value from request pack (case-insensitive) ──
static int taida_net4_get_header_value(taida_val req, const unsigned char *raw, size_t raw_len,
                                        const char *target_name, char *out, size_t out_cap) {
    taida_val headers = taida_pack_get(req, taida_str_hash((taida_val)"headers"));
    if (!TAIDA_IS_LIST(headers)) return 0;
    taida_val *hdr_list = (taida_val*)headers;
    taida_val hdr_count = hdr_list[2];
    size_t target_len = strlen(target_name);

    for (taida_val i = 0; i < hdr_count; i++) {
        taida_val header = hdr_list[4 + i];
        if (!taida_is_buchi_pack(header)) continue;

        taida_val name_span = taida_pack_get(header, taida_str_hash((taida_val)"name"));
        if (!taida_is_buchi_pack(name_span)) continue;
        taida_val n_start = taida_pack_get(name_span, taida_str_hash((taida_val)"start"));
        taida_val n_len = taida_pack_get(name_span, taida_str_hash((taida_val)"len"));
        if (n_start < 0 || n_len <= 0 || (size_t)(n_start + n_len) > raw_len) continue;
        if ((size_t)n_len != target_len) continue;

        int match = 1;
        for (size_t j = 0; j < target_len; j++) {
            char c = (char)raw[n_start + j];
            if (c >= 'A' && c <= 'Z') c += 32;
            char t = target_name[j];
            if (t >= 'A' && t <= 'Z') t += 32;
            if (c != t) { match = 0; break; }
        }
        if (!match) continue;

        taida_val val_span = taida_pack_get(header, taida_str_hash((taida_val)"value"));
        if (!taida_is_buchi_pack(val_span)) continue;
        taida_val v_start = taida_pack_get(val_span, taida_str_hash((taida_val)"start"));
        taida_val v_len = taida_pack_get(val_span, taida_str_hash((taida_val)"len"));
        if (v_start < 0 || v_len <= 0 || (size_t)(v_start + v_len) > raw_len) continue;

        size_t copy_len = (size_t)v_len;
        if (copy_len >= out_cap) copy_len = out_cap - 1;
        memcpy(out, raw + v_start, copy_len);
        out[copy_len] = '\0';
        return 1;
    }
    return 0;
}

// ── Helper: extract method string from request ──
static int taida_net4_get_method(taida_val req, const unsigned char *raw, size_t raw_len, char *out, size_t out_cap) {
    taida_val method_span = taida_pack_get(req, taida_str_hash((taida_val)"method"));
    if (!taida_is_buchi_pack(method_span)) return 0;
    taida_val m_start = taida_pack_get(method_span, taida_str_hash((taida_val)"start"));
    taida_val m_len = taida_pack_get(method_span, taida_str_hash((taida_val)"len"));
    if (m_start < 0 || m_len <= 0 || (size_t)(m_start + m_len) > raw_len) return 0;
    size_t copy_len = (size_t)m_len;
    if (copy_len >= out_cap) copy_len = out_cap - 1;
    memcpy(out, raw + m_start, copy_len);
    out[copy_len] = '\0';
    return 1;
}

// Case-insensitive string compare.
static int taida_net4_strcasecmp(const char *a, const char *b) {
    while (*a && *b) {
        char ca = *a, cb = *b;
        if (ca >= 'A' && ca <= 'Z') ca += 32;
        if (cb >= 'A' && cb <= 'Z') cb += 32;
        if (ca != cb) return ca - cb;
        a++; b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}

// Check if a comma-separated header value contains a token (case-insensitive).
static int taida_net4_header_contains_token(const char *value, const char *token) {
    size_t token_len = strlen(token);
    const char *p = value;
    while (*p) {
        // Skip leading whitespace and commas.
        while (*p == ' ' || *p == '\t' || *p == ',') p++;
        if (!*p) break;
        const char *start = p;
        while (*p && *p != ',') p++;
        // Trim trailing whitespace.
        const char *end = p;
        while (end > start && (end[-1] == ' ' || end[-1] == '\t')) end--;
        size_t tlen = (size_t)(end - start);
        if (tlen == token_len) {
            int match = 1;
            for (size_t i = 0; i < tlen; i++) {
                char ca = start[i], cb = token[i];
                if (ca >= 'A' && ca <= 'Z') ca += 32;
                if (cb >= 'A' && cb <= 'Z') cb += 32;
                if (ca != cb) { match = 0; break; }
            }
            if (match) return 1;
        }
    }
    return 0;
}

// ── wsUpgrade(req, writer) → Lax[@(ws: WsConn)] (NET4-4b) ──
taida_val taida_net_ws_upgrade(taida_val req, taida_val writer) {
    // Must be inside 2-arg handler.
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsUpgrade: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    // Validate writer token.
    taida_net3_validate_writer(writer, "wsUpgrade");

    // NB4-10: Verify request token matches the active body state.
    {
        Net4BodyState *bs_check = tl_net4_body;
        if (bs_check) {
            uint64_t tok = taida_net4_extract_body_token(req);
            if (tok != bs_check->request_token) {
                fprintf(stderr, "wsUpgrade: request pack does not match the current active request. "
                        "The request may be stale or fabricated.\n");
                exit(1);
            }
        }
    }

    // State check: only valid in Idle state.
    switch (w->state) {
        case NET3_STATE_IDLE: break;
        case NET3_STATE_HEAD_PREPARED:
        case NET3_STATE_STREAMING:
            fprintf(stderr, "wsUpgrade: cannot upgrade after HTTP response has started. "
                    "wsUpgrade must be called before startResponse/writeChunk.\n");
            exit(1);
        case NET3_STATE_ENDED:
            fprintf(stderr, "wsUpgrade: cannot upgrade after HTTP response has ended.\n");
            exit(1);
        case NET3_STATE_WEBSOCKET:
            fprintf(stderr, "wsUpgrade: WebSocket upgrade already completed.\n");
            exit(1);
    }

    if (!taida_is_buchi_pack(req)) {
        return taida_net4_make_lax_ws_empty();
    }

    // Extract raw bytes for header value extraction.
    taida_val raw_val = taida_pack_get(req, taida_str_hash((taida_val)"raw"));
    if (!TAIDA_IS_BYTES(raw_val)) {
        return taida_net4_make_lax_ws_empty();
    }
    taida_val *raw_arr = (taida_val*)raw_val;
    taida_val raw_len = raw_arr[1];
    // Materialize raw bytes for C string comparison.
    unsigned char *raw = (unsigned char*)TAIDA_MALLOC((size_t)raw_len + 1, "net_ws_raw");
    for (taida_val i = 0; i < raw_len; i++) raw[i] = (unsigned char)raw_arr[2 + i];
    raw[raw_len] = 0;

    // Validate: must be GET.
    char method[16];
    if (!taida_net4_get_method(req, raw, (size_t)raw_len, method, sizeof(method)) ||
        taida_net4_strcasecmp(method, "GET") != 0) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Check: no body (Content-Length must be 0 or absent, not chunked).
    taida_val cl = taida_pack_get(req, taida_str_hash((taida_val)"contentLength"));
    taida_val chunked_val = taida_pack_get(req, taida_str_hash((taida_val)"chunked"));
    if (cl > 0 || chunked_val != 0) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Validate: Upgrade: websocket
    char hdr_buf[256];
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "upgrade", hdr_buf, sizeof(hdr_buf)) ||
        taida_net4_strcasecmp(hdr_buf, "websocket") != 0) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Validate: Connection contains "Upgrade"
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "connection", hdr_buf, sizeof(hdr_buf)) ||
        !taida_net4_header_contains_token(hdr_buf, "Upgrade")) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Validate: Sec-WebSocket-Version: 13
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "sec-websocket-version", hdr_buf, sizeof(hdr_buf))) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }
    // Trim whitespace.
    {
        char *p = hdr_buf;
        while (*p == ' ' || *p == '\t') p++;
        if (strcmp(p, "13") != 0) {
            free(raw);
            return taida_net4_make_lax_ws_empty();
        }
    }

    // Validate: Sec-WebSocket-Key (must be present and non-empty).
    char ws_key[256];
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "sec-websocket-key", ws_key, sizeof(ws_key))) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }
    // Trim key.
    {
        size_t ks = 0, ke = strlen(ws_key);
        while (ks < ke && (ws_key[ks] == ' ' || ws_key[ks] == '\t')) ks++;
        while (ke > ks && (ws_key[ke-1] == ' ' || ws_key[ke-1] == '\t')) ke--;
        if (ks > 0) memmove(ws_key, ws_key + ks, ke - ks);
        ws_key[ke - ks] = '\0';
    }
    if (ws_key[0] == '\0') {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }
    // NB4-11: RFC 6455: key must be 24 chars and decode to exactly 16 bytes.
    {
        size_t key_len = strlen(ws_key);
        if (key_len != 24) {
            free(raw);
            return taida_net4_make_lax_ws_empty();
        }
        uint8_t decoded[18]; // 16 bytes + margin
        int dec_len = taida_base64_decode(ws_key, key_len, decoded, sizeof(decoded));
        if (dec_len != 16) {
            free(raw);
            return taida_net4_make_lax_ws_empty();
        }
    }

    free(raw);

    // All validations passed. Compute accept and send 101 response.
    char *accept = taida_net4_compute_ws_accept(ws_key);

    int fd = tl_net3_client_fd;
    char response[512];
    int rlen = snprintf(response, sizeof(response),
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        "Sec-WebSocket-Accept: %s\r\n"
        "\r\n", accept);
    free(accept);

    if (rlen < 0 || (size_t)rlen >= sizeof(response)) {
        return taida_net4_make_lax_ws_empty();
    }
    taida_net_send_all(fd, response, (size_t)rlen);

    // Transition to WebSocket state.
    w->state = NET3_STATE_WEBSOCKET;

    // Mark body state and set ws token.
    Net4BodyState *bs = tl_net4_body;
    uint64_t ws_tok = taida_net4_alloc_ws_token();
    if (bs) {
        bs->ws_closed = 0;
        bs->ws_token = ws_tok;
    }

    // Create WsConn BuchiPack with identity token (NB4-10).
    taida_val ws_pack = taida_pack_new(2);
    taida_pack_set_hash(ws_pack, 0, taida_str_hash((taida_val)"__ws_id"));
    taida_pack_set(ws_pack, 0, (taida_val)"__v4_websocket_conn");
    taida_pack_set_tag(ws_pack, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(ws_pack, 1, taida_str_hash((taida_val)"__ws_token"));
    taida_pack_set(ws_pack, 1, (taida_val)ws_tok);
    taida_pack_set_tag(ws_pack, 1, TAIDA_TAG_INT);

    return taida_net4_make_lax_ws_value(ws_pack);
}

// ── wsSend(ws, data) → Unit (NET4-4d) ───────────────────────
taida_val taida_net_ws_send(taida_val ws, taida_val data) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsSend: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsSend: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsSend: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (bs && bs->ws_closed) {
        fprintf(stderr, "wsSend: WebSocket connection is already closed.\n");
        exit(1);
    }

    int fd = tl_net3_client_fd;

    // Determine opcode and payload.
    uint8_t opcode;
    const unsigned char *payload;
    size_t payload_len;
    unsigned char *temp_buf = NULL;

    if (TAIDA_IS_BYTES(data)) {
        opcode = WS_OPCODE_BINARY;
        taida_val *bytes = (taida_val*)data;
        taida_val blen = bytes[1];
        payload_len = (size_t)blen;
        temp_buf = (unsigned char*)TAIDA_MALLOC(payload_len + 1, "net_ws_send_bytes");
        for (taida_val i = 0; i < blen; i++) temp_buf[i] = (unsigned char)bytes[2 + i];
        payload = temp_buf;
    } else {
        // Assume Str -> text frame.
        opcode = WS_OPCODE_TEXT;
        const char *s = (const char*)data;
        size_t slen = 0;
        if (!taida_read_cstr_len_safe(s, 64 * 1024 * 1024, &slen)) {
            fprintf(stderr, "wsSend: data must be Str (text frame) or Bytes (binary frame)\n");
            exit(1);
        }
        payload = (const unsigned char*)s;
        payload_len = slen;
    }

    taida_net4_write_ws_frame(fd, opcode, payload, payload_len);
    if (temp_buf) free(temp_buf);

    return 0; // Unit
}

// ── wsReceive(ws) → Lax[@(type, data)] (NET4-4d) ────────────
taida_val taida_net_ws_receive(taida_val ws) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsReceive: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsReceive: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsReceive: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (bs && bs->ws_closed) {
        return taida_net4_make_lax_ws_frame_empty();
    }

    int fd = tl_net3_client_fd;

    // Loop to handle ping/pong transparently.
    for (;;) {
        WsFrameResult frame = taida_net4_read_ws_frame(fd);

        switch (frame.type) {
            case WS_FRAME_TEXT: {
                // Text frame: return data as Str (parity with Interpreter).
                char *text = NULL;
                if (frame.payload_len > 0) {
                    text = (char*)TAIDA_MALLOC(frame.payload_len + 1, "net_ws_text");
                    memcpy(text, frame.payload, frame.payload_len);
                    text[frame.payload_len] = '\0';
                } else {
                    text = taida_str_new_copy("");
                }
                free(frame.payload);
                taida_val data_val = (taida_val)text;
                return taida_net4_make_lax_ws_frame_value("text", data_val);
            }

            case WS_FRAME_BINARY: {
                taida_val bytes = taida_bytes_from_raw(frame.payload, (taida_val)frame.payload_len);
                free(frame.payload);
                return taida_net4_make_lax_ws_frame_value("binary", bytes);
            }

            case WS_FRAME_PING: {
                // Auto pong: send pong with same payload.
                taida_net4_write_ws_frame(fd, WS_OPCODE_PONG,
                    frame.payload ? frame.payload : (unsigned char*)"",
                    frame.payload_len);
                if (frame.payload) free(frame.payload);
                continue; // Next frame.
            }

            case WS_FRAME_PONG: {
                // Unsolicited pong: ignore.
                if (frame.payload) free(frame.payload);
                continue;
            }

            case WS_FRAME_CLOSE: {
                // v5 close code extraction (NET5-0d).
                if (frame.payload_len == 0) {
                    // No status code: reply with empty close payload.
                    if (bs && !bs->ws_closed) {
                        taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, (unsigned char*)"", 0);
                    }
                    if (bs) {
                        bs->ws_closed = 1;
                        bs->ws_close_code = 1005; // No Status Rcvd
                    }
                    if (frame.payload) free(frame.payload);
                    return taida_net4_make_lax_ws_frame_empty();
                } else if (frame.payload_len == 1) {
                    // 1-byte close payload is malformed.
                    unsigned char close_1002[2] = { 0x03, 0xEA };
                    taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_1002, 2);
                    if (bs) bs->ws_closed = 1;
                    if (frame.payload) free(frame.payload);
                    fprintf(stderr, "wsReceive: protocol error: malformed close frame (1-byte payload)\n");
                    exit(1);
                } else {
                    // 2+ bytes: first 2 bytes are the close code (big-endian).
                    uint16_t code = ((uint16_t)frame.payload[0] << 8) | (uint16_t)frame.payload[1];
                    // Validate close code (RFC 6455 Section 7.4).
                    // 1000-1003: standard, 1007-1014: IANA-registered,
                    // 3000-4999: reserved for libraries/apps/private use.
                    int valid_code = (code >= 1000 && code <= 1003) ||
                                     (code >= 1007 && code <= 1014) ||
                                     (code >= 3000 && code <= 4999);
                    if (!valid_code) {
                        unsigned char close_1002[2] = { 0x03, 0xEA };
                        taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_1002, 2);
                        if (bs) bs->ws_closed = 1;
                        free(frame.payload);
                        fprintf(stderr, "wsReceive: protocol error: invalid close code %u\n", (unsigned)code);
                        exit(1);
                    }
                    // Validate reason UTF-8 if present.
                    // Strict UTF-8 validation: reject overlong sequences, surrogate
                    // halves (U+D800..U+DFFF), and code points > U+10FFFF to match
                    // Interpreter (std::str::from_utf8) and JS (decode+re-encode).
                    if (frame.payload_len > 2) {
                        size_t rlen = frame.payload_len - 2;
                        unsigned char *reason = frame.payload + 2;
                        size_t i = 0;
                        int utf8_ok = 1;
                        while (i < rlen && utf8_ok) {
                            unsigned char c = reason[i];
                            if (c < 0x80) {
                                i++;
                            } else if ((c & 0xE0) == 0xC0) {
                                // 2-byte: must have 1 continuation, code point >= 0x80
                                if (i + 1 >= rlen || (reason[i+1] & 0xC0) != 0x80) { utf8_ok = 0; break; }
                                uint32_t cp = ((uint32_t)(c & 0x1F) << 6) | (uint32_t)(reason[i+1] & 0x3F);
                                if (cp < 0x80) { utf8_ok = 0; break; } // overlong
                                i += 2;
                            } else if ((c & 0xF0) == 0xE0) {
                                // 3-byte: must have 2 continuations, cp >= 0x800, not surrogate
                                if (i + 2 >= rlen || (reason[i+1] & 0xC0) != 0x80 || (reason[i+2] & 0xC0) != 0x80) { utf8_ok = 0; break; }
                                uint32_t cp = ((uint32_t)(c & 0x0F) << 12) | ((uint32_t)(reason[i+1] & 0x3F) << 6) | (uint32_t)(reason[i+2] & 0x3F);
                                if (cp < 0x800) { utf8_ok = 0; break; } // overlong
                                if (cp >= 0xD800 && cp <= 0xDFFF) { utf8_ok = 0; break; } // surrogate
                                i += 3;
                            } else if ((c & 0xF8) == 0xF0) {
                                // 4-byte: must have 3 continuations, cp >= 0x10000, cp <= 0x10FFFF
                                if (i + 3 >= rlen || (reason[i+1] & 0xC0) != 0x80 || (reason[i+2] & 0xC0) != 0x80 || (reason[i+3] & 0xC0) != 0x80) { utf8_ok = 0; break; }
                                uint32_t cp = ((uint32_t)(c & 0x07) << 18) | ((uint32_t)(reason[i+1] & 0x3F) << 12) | ((uint32_t)(reason[i+2] & 0x3F) << 6) | (uint32_t)(reason[i+3] & 0x3F);
                                if (cp < 0x10000) { utf8_ok = 0; break; } // overlong
                                if (cp > 0x10FFFF) { utf8_ok = 0; break; } // out of range
                                i += 4;
                            } else {
                                utf8_ok = 0; break; // invalid lead byte
                            }
                        }
                        if (!utf8_ok) {
                            unsigned char close_1002[2] = { 0x03, 0xEA };
                            taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_1002, 2);
                            if (bs) bs->ws_closed = 1;
                            free(frame.payload);
                            fprintf(stderr, "wsReceive: protocol error: invalid UTF-8 in close reason\n");
                            exit(1);
                        }
                    }
                    // Valid close: echo the code in the reply.
                    unsigned char reply[2] = { (unsigned char)(code >> 8), (unsigned char)(code & 0xFF) };
                    if (bs && !bs->ws_closed) {
                        taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, reply, 2);
                    }
                    if (bs) {
                        bs->ws_closed = 1;
                        bs->ws_close_code = (int64_t)code;
                    }
                    free(frame.payload);
                    return taida_net4_make_lax_ws_frame_empty();
                }
            }

            case WS_FRAME_ERROR:
            default: {
                if (frame.payload) free(frame.payload);
                // Send close frame with protocol error (1002).
                unsigned char close_payload[2] = { 0x03, 0xEA }; // 1002
                taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_payload, 2);
                if (bs) bs->ws_closed = 1;
                fprintf(stderr, "wsReceive: protocol error\n");
                exit(1);
            }
        }
    }
}

// ── wsClose(ws, code) → Unit (NET4-4d, v5 revision) ────────────────
// v5: wsClose(ws) or wsClose(ws, code) → Unit.
// 2nd arg (code): 0 = default 1000 (Normal Closure), otherwise explicit close code.
// Valid codes: 1000-4999 excluding reserved 1004, 1005, 1006, 1015.
taida_val taida_net_ws_close(taida_val ws, taida_val code_val) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsClose: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsClose: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsClose: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;

    // Idempotent: no-op if already closed.
    if (bs && bs->ws_closed) {
        return 0; // Unit
    }

    // v5: Determine close code from 2nd argument.
    // code_val is a raw Int (lowering passes 0 for default, or the literal value).
    int64_t close_code_i64 = (int64_t)code_val;

    uint16_t close_code;
    if (close_code_i64 == 0) {
        close_code = 1000; // default: Normal Closure
    } else {
        // Validate close code range.
        if (close_code_i64 < 1000 || close_code_i64 > 4999) {
            fprintf(stderr, "wsClose: close code must be 1000-4999, got %lld\n", (long long)close_code_i64);
            exit(1);
        }
        // Reserved codes that must not be sent.
        if (close_code_i64 == 1004 || close_code_i64 == 1005 || close_code_i64 == 1006 || close_code_i64 == 1015) {
            fprintf(stderr, "wsClose: close code %lld is reserved and cannot be sent\n", (long long)close_code_i64);
            exit(1);
        }
        close_code = (uint16_t)close_code_i64;
    }

    int fd = tl_net3_client_fd;

    // Send close frame with the specified close code.
    unsigned char close_payload[2] = { (unsigned char)(close_code >> 8), (unsigned char)(close_code & 0xFF) };
    taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_payload, 2);

    if (bs) bs->ws_closed = 1;

    return 0; // Unit
}

// v5: wsCloseCode(ws) → Int (NET5-0d)
// Returns the close code received from the peer's close frame.
// 0 = no close frame received yet, 1005 = no status code, 1000-4999 = peer code.
taida_val taida_net_ws_close_code(taida_val ws) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsCloseCode: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsCloseCode: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsCloseCode: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    int64_t code = (bs) ? bs->ws_close_code : 0;
    return (taida_val)code;
}

// Validate that the writer argument is a genuine BuchiPack token with
// __writer_id === "__v3_streaming_writer" (parity with Interpreter/JS).
static void taida_net3_validate_writer(taida_val writer, const char *api_name) {
    if (!taida_is_buchi_pack(writer)) {
        fprintf(stderr, "%s: first argument must be the writer provided by httpServe\n", api_name);
        exit(1);
    }
    taida_val id_val = taida_pack_get(writer, taida_str_hash((taida_val)"__writer_id"));
    if (id_val == 0) {
        fprintf(stderr, "%s: first argument must be the writer provided by httpServe\n", api_name);
        exit(1);
    }
    const char *id_str = (const char*)id_val;
    size_t id_len = 0;
    if (!taida_read_cstr_len_safe(id_str, 64, &id_len) ||
        id_len != 21 || memcmp(id_str, "__v3_streaming_writer", 21) != 0) {
        fprintf(stderr, "%s: first argument must be the writer provided by httpServe\n", api_name);
        exit(1);
    }
}

// Create a writer BuchiPack token for 2-arg handler.
// Contains __writer_id sentinel field (parity with Interpreter/JS).
static taida_val taida_net3_create_writer_token(void) {
    taida_val pack = taida_pack_new(1);
    taida_pack_set_hash(pack, 0, taida_str_hash((taida_val)"__writer_id"));
    taida_pack_set(pack, 0, (taida_val)"__v3_streaming_writer");
    taida_pack_set_tag(pack, 0, TAIDA_TAG_STR);
    return pack;
}

// ── NET2-5c: Thread pool structures ─────────────────────────────
// Shared state for the thread pool: a mutex-protected queue of client fds.
// Each worker thread pulls a client fd, processes the keep-alive loop, then
// returns to wait for the next fd.

typedef struct {
    int client_fd;
    struct sockaddr_in peer_addr;
} NetClientSlot;

typedef struct {
    // Shared mutable state (protected by mutex)
    pthread_mutex_t mutex;
    pthread_cond_t  cond_available;  // signal workers: new fd or shutdown
    pthread_cond_t  cond_done;       // signal main: a worker finished

    // Queue of pending client fds
    NetClientSlot *queue;
    int queue_cap;
    int queue_head;
    int queue_tail;
    int queue_count;

    // Global request counter (atomic via mutex)
    int64_t request_count;
    int64_t max_requests;

    // Active connection count (for maxConnections enforcement)
    int active_connections;

    // Shutdown flag
    int shutdown;

    // Handler and timeout
    taida_val handler;
    int64_t timeout_ms;

    // NET3-5a: handler arity (1 = one-shot, 2 = streaming, -1 = unknown/runtime detect)
    int handler_arity;

    // NET5-4a: TLS context (NULL = plaintext, non-NULL = TLS).
    OSSL_SSL_CTX *ssl_ctx;
} NetThreadPool;

static void net_pool_init(NetThreadPool *pool, int queue_cap, taida_val handler, int64_t max_requests, int64_t timeout_ms, int handler_arity) {
    pthread_mutex_init(&pool->mutex, NULL);
    pthread_cond_init(&pool->cond_available, NULL);
    pthread_cond_init(&pool->cond_done, NULL);
    pool->queue_cap = queue_cap;
    pool->queue = (NetClientSlot*)TAIDA_MALLOC(sizeof(NetClientSlot) * (size_t)queue_cap, "net_pool_queue");
    pool->queue_head = 0;
    pool->queue_tail = 0;
    pool->queue_count = 0;
    pool->request_count = 0;
    pool->max_requests = max_requests;
    pool->active_connections = 0;
    pool->shutdown = 0;
    pool->handler = handler;
    pool->timeout_ms = timeout_ms;
    pool->handler_arity = handler_arity;
    pool->ssl_ctx = NULL; // NET5-4a: set by httpServe if TLS configured
}

static void net_pool_destroy(NetThreadPool *pool) {
    pthread_mutex_destroy(&pool->mutex);
    pthread_cond_destroy(&pool->cond_available);
    pthread_cond_destroy(&pool->cond_done);
    free(pool->queue);
}

// Enqueue a client fd. Returns 0 on success, -1 if queue full.
static int net_pool_enqueue(NetThreadPool *pool, int fd, struct sockaddr_in addr) {
    if (pool->queue_count >= pool->queue_cap) return -1;
    pool->queue[pool->queue_tail].client_fd = fd;
    pool->queue[pool->queue_tail].peer_addr = addr;
    pool->queue_tail = (pool->queue_tail + 1) % pool->queue_cap;
    pool->queue_count++;
    return 0;
}

// Dequeue a client fd. Returns 0 on success, -1 if empty.
static int net_pool_dequeue(NetThreadPool *pool, NetClientSlot *out) {
    if (pool->queue_count <= 0) return -1;
    *out = pool->queue[pool->queue_head];
    pool->queue_head = (pool->queue_head + 1) % pool->queue_cap;
    pool->queue_count--;
    return 0;
}

// Check if the global request limit has been reached (call under mutex).
static int net_pool_requests_exhausted(NetThreadPool *pool) {
    return (pool->max_requests > 0 && pool->request_count >= pool->max_requests);
}

// ── NET2-5a/5b/5c: Worker thread — keep-alive loop per connection ──
static void *net_worker_thread(void *arg) {
    NetThreadPool *pool = (NetThreadPool*)arg;

    for (;;) {
        NetClientSlot slot;

        // Wait for a client fd or shutdown
        pthread_mutex_lock(&pool->mutex);
        while (pool->queue_count == 0 && !pool->shutdown) {
            pthread_cond_wait(&pool->cond_available, &pool->mutex);
        }
        if (pool->shutdown && pool->queue_count == 0) {
            pthread_mutex_unlock(&pool->mutex);
            break;
        }
        net_pool_dequeue(pool, &slot);
        pool->active_connections++;
        pthread_mutex_unlock(&pool->mutex);

        int client_fd = slot.client_fd;
        struct sockaddr_in peer_addr = slot.peer_addr;

        char host_buf[INET_ADDRSTRLEN] = {0};
        const char *peer_host = inet_ntop(AF_INET, &peer_addr.sin_addr, host_buf, sizeof(host_buf));
        if (!peer_host) peer_host = "";
        int peer_port = (int)ntohs(peer_addr.sin_port);

        // Set read timeout on client socket
        int64_t effective_timeout = (pool->timeout_ms > 0) ? pool->timeout_ms : 5000;
        {
            struct timeval tv;
            tv.tv_sec = (long)(effective_timeout / 1000);
            tv.tv_usec = (long)((effective_timeout % 1000) * 1000);
            setsockopt(client_fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
            // Also set write timeout for TLS handshake and writes.
            setsockopt(client_fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
        }

        // NET5-4a: TLS handshake if pool has SSL_CTX.
        OSSL_SSL *conn_ssl = NULL;
        if (pool->ssl_ctx) {
            conn_ssl = taida_tls_handshake(pool->ssl_ctx, client_fd);
            if (!conn_ssl) {
                // NET5-0c: handshake failure = close connection, don't call handler.
                close(client_fd);
                pthread_mutex_lock(&pool->mutex);
                pool->active_connections--;
                pthread_cond_signal(&pool->cond_done);
                pthread_mutex_unlock(&pool->mutex);
                continue;
            }
        }
        tl_ssl = conn_ssl;

        // Per-connection scratch buffer (allocated once, reused via advance)
        #define NET_MAX_REQUEST_BUF 1048576
        size_t buf_cap = 8192;
        unsigned char *buf = (unsigned char*)TAIDA_MALLOC(buf_cap, "net_worker_buf");
        size_t total_read = 0;

        // ── Keep-alive loop ──
        for (;;) {

            // Phase 1: Read until HTTP head is complete
            // NB2-19: Parse once, reuse result for keepAlive + request pack building.
            // NB2-9: Properly release parse_result / parse_bytes to prevent memory leak.
            int head_complete = 0;
            size_t head_consumed = 0;
            int64_t content_length = 0;
            int is_chunked = 0;
            int head_malformed = 0;
            taida_val parse_result = 0;  // retained across head+body for single-parse reuse
            taida_val parse_inner = 0;   // inner pack from parse_result

            while (total_read < NET_MAX_REQUEST_BUF) {
                // Try to parse what we have so far
                if (total_read > 3) {
                    int found_end = 0;
                    for (size_t i = 0; i + 3 < total_read; i++) {
                        if (buf[i] == '\r' && buf[i+1] == '\n' && buf[i+2] == '\r' && buf[i+3] == '\n') {
                            found_end = 1;
                            head_consumed = i + 4;
                            break;
                        }
                    }
                    if (found_end) {
                        head_complete = 1;
                        taida_val parse_bytes = taida_bytes_from_raw(buf, (taida_val)total_read);
                        parse_result = taida_net_http_parse_request_head(parse_bytes);
                        taida_val throw_val = taida_pack_get(parse_result, taida_str_hash((taida_val)"throw"));
                        if (throw_val != 0) {
                            head_malformed = 1;
                            taida_release(parse_bytes);
                            taida_release(parse_result);
                            parse_result = 0;
                            break;
                        }
                        parse_inner = taida_pack_get(parse_result, taida_str_hash((taida_val)"__value"));
                        if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                            content_length = taida_pack_get(parse_inner, taida_str_hash((taida_val)"contentLength"));
                            head_consumed = (size_t)taida_pack_get(parse_inner, taida_str_hash((taida_val)"consumed"));
                            taida_val chunked_val = taida_pack_get(parse_inner, taida_str_hash((taida_val)"chunked"));
                            is_chunked = (chunked_val != 0) ? 1 : 0;
                        }
                        taida_release(parse_bytes);
                        break;
                    }
                }

                // Read more data
                if (total_read >= buf_cap) {
                    size_t new_cap = buf_cap * 2;
                    if (new_cap > NET_MAX_REQUEST_BUF) new_cap = NET_MAX_REQUEST_BUF;
                    TAIDA_REALLOC(buf, new_cap, "net_worker_head");
                    buf_cap = new_cap;
                }
                ssize_t n = taida_tls_recv(client_fd, buf + total_read, buf_cap - total_read);
                if (n <= 0) {
                    // EOF or timeout — partial head gets 400 (parity with interpreter)
                    if (total_read > 0) {
                        const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, bad, strlen(bad));
                        pthread_mutex_lock(&pool->mutex);
                        if (!net_pool_requests_exhausted(pool)) {
                            pool->request_count++;
                        }
                        pthread_mutex_unlock(&pool->mutex);
                    }
                    goto conn_done;
                }
                total_read += (size_t)n;
            }

            if (!head_complete || head_malformed) {
                const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                taida_net_send_all(client_fd, bad, strlen(bad));
                pthread_mutex_lock(&pool->mutex);
                if (!net_pool_requests_exhausted(pool)) {
                    pool->request_count++;
                }
                pthread_mutex_unlock(&pool->mutex);
                break; // close connection
            }

            // Head is complete — this counts as a real request.
            pthread_mutex_lock(&pool->mutex);
            if (net_pool_requests_exhausted(pool)) {
                pthread_mutex_unlock(&pool->mutex);
                const char *unavail = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                taida_net_send_all(client_fd, unavail, strlen(unavail));
                if (parse_result) taida_release(parse_result);
                goto conn_done;
            }
            pool->request_count++;
            pthread_mutex_unlock(&pool->mutex);

            // NET4: Detect handler arity before body reading.
            // 2-arg handler = body-deferred (v4), 1-arg = eager body read (v2).
            int keep_alive = 1;
            size_t wire_consumed = head_consumed; // default for 2-arg deferred

            if (pool->handler_arity >= 2) {
                // ── v4 2-arg handler path: body-deferred ──
                // Do NOT eagerly read body. raw = head only.

                // Determine keep-alive from head.
                taida_val http_minor = 1;
                taida_val parsed_headers = 0;
                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val ver = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    if (ver != 0 && taida_is_buchi_pack(ver)) {
                        http_minor = taida_pack_get(ver, taida_str_hash((taida_val)"minor"));
                    }
                    parsed_headers = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                }
                keep_alive = taida_net_determine_keep_alive(buf, head_consumed, parsed_headers, http_minor);

                // Capture leftover body bytes already in buf (beyond head).
                size_t leftover_len = (total_read > head_consumed) ? (total_read - head_consumed) : 0;
                unsigned char *leftover = NULL;
                if (leftover_len > 0) {
                    leftover = (unsigned char*)TAIDA_MALLOC(leftover_len, "net_v4_leftover");
                    memcpy(leftover, buf + head_consumed, leftover_len);
                }

                // Create body streaming state.
                Net4BodyState body_state;
                memset(&body_state, 0, sizeof(body_state));
                body_state.is_chunked = is_chunked;
                body_state.content_length = content_length;
                body_state.bytes_consumed = 0;
                body_state.fully_read = (!is_chunked && content_length == 0) ? 1 : 0;
                body_state.any_read_started = 0;
                body_state.leftover = leftover;
                body_state.leftover_len = leftover_len;
                body_state.leftover_pos = 0;
                body_state.chunked_state = NET4_CHUNKED_WAIT_SIZE;
                body_state.chunked_remaining = 0;
                body_state.request_token = taida_net4_alloc_token();
                body_state.ws_closed = 0;
                body_state.ws_token = 0;
                body_state.ws_close_code = 0; // v5: no close frame received yet

                // Build request pack (head only, body = empty span).
                taida_val raw_bytes = taida_bytes_from_raw(buf, (taida_val)head_consumed);
                // 15 fields: raw, method, path, query, version, headers, body, bodyOffset,
                //            contentLength, remoteHost, remotePort, keepAlive, chunked,
                //            __body_stream, __body_token
                taida_val request = taida_pack_new(15);
                taida_pack_set_hash(request, 0, taida_str_hash((taida_val)"raw"));
                taida_pack_set(request, 0, raw_bytes);
                taida_pack_set_tag(request, 0, TAIDA_TAG_PACK);

                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val method_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"method"));
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, method_v);
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    if (method_v > 4096) taida_retain(method_v);

                    taida_val path_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"path"));
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, path_v);
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    if (path_v > 4096) taida_retain(path_v);

                    taida_val query_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"query"));
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, query_v);
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    if (query_v > 4096) taida_retain(query_v);

                    taida_val version_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, version_v);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    if (version_v > 4096) taida_retain(version_v);

                    taida_val headers_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, headers_v);
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                    if (headers_v > 4096) taida_retain(headers_v);
                } else {
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    taida_val ver = taida_pack_new(2);
                    taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
                    taida_pack_set(ver, 0, 1);
                    taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
                    taida_pack_set(ver, 1, 1);
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, ver);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, taida_list_new());
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                }
                // v4: body span is empty (body not yet read).
                taida_pack_set_hash(request, 6, taida_str_hash((taida_val)"body"));
                taida_pack_set(request, 6, taida_net_make_span(0, 0));
                taida_pack_set_tag(request, 6, TAIDA_TAG_PACK);
                taida_pack_set_hash(request, 7, taida_str_hash((taida_val)"bodyOffset"));
                taida_pack_set(request, 7, (taida_val)head_consumed);
                taida_pack_set_hash(request, 8, taida_str_hash((taida_val)"contentLength"));
                taida_pack_set(request, 8, (taida_val)content_length);
                taida_pack_set_hash(request, 9, taida_str_hash((taida_val)"remoteHost"));
                taida_pack_set(request, 9, (taida_val)taida_str_new_copy(peer_host));
                taida_pack_set_tag(request, 9, TAIDA_TAG_STR);
                taida_pack_set_hash(request, 10, taida_str_hash((taida_val)"remotePort"));
                taida_pack_set(request, 10, (taida_val)peer_port);
                taida_pack_set_hash(request, 11, taida_str_hash((taida_val)"keepAlive"));
                taida_pack_set(request, 11, keep_alive ? 1 : 0);
                taida_pack_set_tag(request, 11, TAIDA_TAG_BOOL);
                taida_pack_set_hash(request, 12, taida_str_hash((taida_val)"chunked"));
                taida_pack_set(request, 12, is_chunked ? 1 : 0);
                taida_pack_set_tag(request, 12, TAIDA_TAG_BOOL);
                // v4 sentinel + token.
                taida_pack_set_hash(request, 13, taida_str_hash((taida_val)"__body_stream"));
                taida_pack_set(request, 13, (taida_val)"__v4_body_stream");
                taida_pack_set_tag(request, 13, TAIDA_TAG_STR);
                taida_pack_set_hash(request, 14, taida_str_hash((taida_val)"__body_token"));
                taida_pack_set(request, 14, (taida_val)body_state.request_token);

                if (parse_result) { taida_release(parse_result); parse_result = 0; }

                // Create writer state.
                Net3WriterState writer_state;
                writer_state.state = NET3_STATE_IDLE;
                writer_state.pending_status = 200;
                writer_state.sse_mode = 0;
                writer_state.header_count = 0;

                // Set thread-local context.
                tl_net3_writer = &writer_state;
                tl_net3_client_fd = client_fd;
                tl_net4_body = &body_state;

                taida_val writer_token = taida_net3_create_writer_token();
                taida_val response = taida_invoke_callback2(pool->handler, request, writer_token);

                // Clear thread-local context.
                tl_net3_writer = NULL;
                tl_net3_client_fd = -1;
                tl_net4_body = NULL;

                // ── v4: WebSocket auto-close on handler return ──
                if (writer_state.state == NET3_STATE_WEBSOCKET) {
                    if (!body_state.ws_closed) {
                        unsigned char close_payload[2] = { 0x03, 0xE8 }; // 1000
                        taida_net4_write_ws_frame(client_fd, WS_OPCODE_CLOSE, close_payload, 2);
                    }
                    taida_release(request);
                    taida_release(writer_token);
                    taida_release(response);
                    if (leftover) free(leftover);
                    // WebSocket: never return to keep-alive.
                    // Check request limit.
                    pthread_mutex_lock(&pool->mutex);
                    int limit_hit = net_pool_requests_exhausted(pool);
                    pthread_mutex_unlock(&pool->mutex);
                    total_read = 0;
                    if (limit_hit) {
                        // Signal shutdown.
                        pthread_mutex_lock(&pool->mutex);
                        pool->shutdown = 1;
                        pthread_cond_broadcast(&pool->cond_available);
                        pthread_mutex_unlock(&pool->mutex);
                    }
                    break; // Close connection.
                }

                if (writer_state.state == NET3_STATE_IDLE) {
                    // One-shot fallback.
                    taida_val effective_response = response;
                    int need_default = 1;
                    if (response > 4096 && taida_is_buchi_pack(response)) {
                        taida_val status_val = taida_pack_get(response, taida_str_hash((taida_val)"status"));
                        taida_val body_val = taida_pack_get(response, taida_str_hash((taida_val)"body"));
                        if (status_val != 0 || body_val != 0) need_default = 0;
                    }
                    if (need_default && (response == 0 || !taida_is_buchi_pack(response))) {
                        effective_response = taida_pack_new(3);
                        taida_pack_set_hash(effective_response, 0, taida_str_hash((taida_val)"status"));
                        taida_pack_set(effective_response, 0, 200);
                        taida_pack_set_hash(effective_response, 1, taida_str_hash((taida_val)"headers"));
                        taida_pack_set(effective_response, 1, taida_list_new());
                        taida_pack_set_tag(effective_response, 1, TAIDA_TAG_LIST);
                        taida_pack_set_hash(effective_response, 2, taida_str_hash((taida_val)"body"));
                        taida_pack_set(effective_response, 2, (taida_val)"");
                        taida_pack_set_tag(effective_response, 2, TAIDA_TAG_STR);
                    }
                    taida_val encoded = taida_net_http_encode_response(effective_response);
                    taida_net_send_response(client_fd, encoded);
                    taida_release(encoded);
                    if (need_default && effective_response != response) taida_release(effective_response);
                } else {
                    // Streaming was started.
                    if (writer_state.state != NET3_STATE_ENDED) {
                        int auto_end_failed = 0;
                        if (writer_state.state == NET3_STATE_HEAD_PREPARED) {
                            if (taida_net3_commit_head(client_fd, &writer_state) != 0) {
                                fprintf(stderr, "httpServe: failed to commit response head during auto-end\n");
                                auto_end_failed = 1;
                            }
                        }
                        if (!auto_end_failed && !taida_net3_is_bodyless_status(writer_state.pending_status)) {
                            taida_net_send_all(client_fd, "0\r\n\r\n", 5);
                        }
                        writer_state.state = NET3_STATE_ENDED;
                        if (auto_end_failed) {
                            // Force connection close
                            keep_alive = 0;
                        }
                    }
                }

                taida_release(request);
                taida_release(writer_token);
                taida_release(response);
                if (leftover) free(leftover);

                // NET4-1g: If body not fully read, do NOT return to keep-alive.
                int body_done = body_state.fully_read || (!is_chunked && content_length == 0);
                if (!body_done) keep_alive = 0;

                // For 2-arg body-deferred path, stream read pointer is already
                // past the body (readBodyChunk consumed it). Reset buf.
                total_read = 0;
            } else {
                // ── v2/v3 1-arg handler path (unchanged eager body read) ──
                size_t body_start;
                size_t body_len;
                int64_t final_content_length;

                if (is_chunked) {
                    for (;;) {
                        int64_t check = taida_net_chunked_body_complete(buf, total_read, head_consumed);
                        if (check >= 0) break;
                        if (check == -2) {
                            const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            taida_net_send_all(client_fd, bad, strlen(bad));
                            if (parse_result) taida_release(parse_result);
                            goto conn_done;
                        }
                        if (total_read >= NET_MAX_REQUEST_BUF) {
                            const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            taida_net_send_all(client_fd, bad, strlen(bad));
                            if (parse_result) taida_release(parse_result);
                            goto conn_done;
                        }
                        if (total_read >= buf_cap) {
                            size_t new_cap = buf_cap * 2;
                            if (new_cap > NET_MAX_REQUEST_BUF) new_cap = NET_MAX_REQUEST_BUF;
                            TAIDA_REALLOC(buf, new_cap, "net_worker_chunked");
                            buf_cap = new_cap;
                        }
                        ssize_t n = taida_tls_recv(client_fd, buf + total_read, buf_cap - total_read);
                        if (n <= 0) {
                            const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            taida_net_send_all(client_fd, bad, strlen(bad));
                            if (parse_result) taida_release(parse_result);
                            goto conn_done;
                        }
                        total_read += (size_t)n;
                    }
                    ChunkedCompactResult compact;
                    if (taida_net_chunked_in_place_compact(buf, head_consumed, &compact) < 0) {
                        const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, bad, strlen(bad));
                        if (parse_result) taida_release(parse_result);
                        goto conn_done;
                    }
                    wire_consumed = head_consumed + compact.wire_consumed;
                    body_start = head_consumed;
                    body_len = compact.body_len;
                    final_content_length = (int64_t)compact.body_len;
                } else {
                    if (head_consumed + (size_t)content_length > NET_MAX_REQUEST_BUF) {
                        const char *too_large = "HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, too_large, strlen(too_large));
                        if (parse_result) taida_release(parse_result);
                        break;
                    }
                    size_t body_needed = head_consumed + (size_t)content_length;
                    while (total_read < body_needed && total_read < NET_MAX_REQUEST_BUF) {
                        if (total_read >= buf_cap) {
                            size_t new_cap = buf_cap * 2;
                            if (new_cap > NET_MAX_REQUEST_BUF) new_cap = NET_MAX_REQUEST_BUF;
                            TAIDA_REALLOC(buf, new_cap, "net_worker_body");
                            buf_cap = new_cap;
                        }
                        ssize_t n = taida_tls_recv(client_fd, buf + total_read, buf_cap - total_read);
                        if (n <= 0) break;
                        total_read += (size_t)n;
                    }
                    if (content_length > 0 && total_read < body_needed) {
                        const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, bad, strlen(bad));
                        if (parse_result) taida_release(parse_result);
                        break;
                    }
                    wire_consumed = body_needed;
                    body_start = head_consumed;
                    body_len = (size_t)content_length;
                    final_content_length = content_length;
                }

                size_t raw_len = is_chunked ? (head_consumed + body_len) : wire_consumed;
                taida_val http_minor = 1;
                taida_val parsed_headers = 0;
                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val ver = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    if (ver != 0 && taida_is_buchi_pack(ver)) {
                        http_minor = taida_pack_get(ver, taida_str_hash((taida_val)"minor"));
                    }
                    parsed_headers = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                }
                keep_alive = taida_net_determine_keep_alive(buf, raw_len, parsed_headers, http_minor);

                taida_val raw_bytes = taida_bytes_from_raw(buf, (taida_val)raw_len);
                taida_val request = taida_pack_new(13);
                taida_pack_set_hash(request, 0, taida_str_hash((taida_val)"raw"));
                taida_pack_set(request, 0, raw_bytes);
                taida_pack_set_tag(request, 0, TAIDA_TAG_PACK);

                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val method_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"method"));
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, method_v);
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    if (method_v > 4096) taida_retain(method_v);
                    taida_val path_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"path"));
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, path_v);
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    if (path_v > 4096) taida_retain(path_v);
                    taida_val query_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"query"));
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, query_v);
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    if (query_v > 4096) taida_retain(query_v);
                    taida_val version_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, version_v);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    if (version_v > 4096) taida_retain(version_v);
                    taida_val headers_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, headers_v);
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                    if (headers_v > 4096) taida_retain(headers_v);
                } else {
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    taida_val ver = taida_pack_new(2);
                    taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
                    taida_pack_set(ver, 0, 1);
                    taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
                    taida_pack_set(ver, 1, 1);
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, ver);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, taida_list_new());
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                }
                taida_pack_set_hash(request, 6, taida_str_hash((taida_val)"body"));
                taida_pack_set(request, 6, taida_net_make_span((taida_val)body_start, (taida_val)body_len));
                taida_pack_set_tag(request, 6, TAIDA_TAG_PACK);
                taida_pack_set_hash(request, 7, taida_str_hash((taida_val)"bodyOffset"));
                taida_pack_set(request, 7, (taida_val)body_start);
                taida_pack_set_hash(request, 8, taida_str_hash((taida_val)"contentLength"));
                taida_pack_set(request, 8, (taida_val)final_content_length);
                taida_pack_set_hash(request, 9, taida_str_hash((taida_val)"remoteHost"));
                taida_pack_set(request, 9, (taida_val)taida_str_new_copy(peer_host));
                taida_pack_set_tag(request, 9, TAIDA_TAG_STR);
                taida_pack_set_hash(request, 10, taida_str_hash((taida_val)"remotePort"));
                taida_pack_set(request, 10, (taida_val)peer_port);
                taida_pack_set_hash(request, 11, taida_str_hash((taida_val)"keepAlive"));
                taida_pack_set(request, 11, keep_alive ? 1 : 0);
                taida_pack_set_tag(request, 11, TAIDA_TAG_BOOL);
                taida_pack_set_hash(request, 12, taida_str_hash((taida_val)"chunked"));
                taida_pack_set(request, 12, is_chunked ? 1 : 0);
                taida_pack_set_tag(request, 12, TAIDA_TAG_BOOL);

                if (parse_result) { taida_release(parse_result); parse_result = 0; }

                // 1-arg handler: v2 one-shot response.
                taida_val response = taida_invoke_callback1(pool->handler, request);
                taida_val encoded = taida_net_http_encode_response(response);
                taida_net_send_response(client_fd, encoded);
                taida_release(request);
                taida_release(encoded);
                taida_release(response);
            }

            // request_count already reserved after head complete — check limit
            pthread_mutex_lock(&pool->mutex);
            int limit_hit = net_pool_requests_exhausted(pool);
            pthread_mutex_unlock(&pool->mutex);

            // Buffer advance: remove consumed bytes, keep any leftover
            if (wire_consumed < total_read) {
                memmove(buf, buf + wire_consumed, total_read - wire_consumed);
                total_read -= wire_consumed;
            } else {
                total_read = 0;
            }

            // Close if not keep-alive or limit reached
            if (!keep_alive || limit_hit) break;
        }

    conn_done:
        // NET5-4a: TLS shutdown before closing fd.
        if (conn_ssl) {
            taida_tls_shutdown_free(conn_ssl);
            conn_ssl = NULL;
            tl_ssl = NULL;
        }
        close(client_fd);
        free(buf);
        buf = NULL;
        total_read = 0;
        buf_cap = 8192;

        // Re-allocate buffer for next connection
        // (will be done at top of next keep-alive loop iteration)

        // Decrement active connections and signal main thread
        pthread_mutex_lock(&pool->mutex);
        pool->active_connections--;
        pthread_cond_signal(&pool->cond_done);
        pthread_mutex_unlock(&pool->mutex);

        #undef NET_MAX_REQUEST_BUF
    }

    return NULL;
}

// ── httpServe(port, handler, maxRequests, timeoutMs, maxConnections) ──
// HTTP/1.1 server v2+v3: keep-alive, chunked TE, pthread pool, maxConnections.
// NET3-5a: handler_arity added — 2 = streaming writer, 1 = one-shot, -1 = unknown.
// v5: tls parameter added. 0 = plaintext (v4 compat), non-zero = BuchiPack @(cert, key) = HTTPS via OpenSSL dlopen.
// Returns Async[Result[@(ok: Bool, requests: Int), _]]
taida_val taida_net_http_serve(taida_val port, taida_val handler, taida_val max_requests, taida_val timeout_ms, taida_val max_connections, taida_val tls, taida_val handler_type_tag, taida_val handler_arity) {
    // NB3-5: Suppress SIGPIPE process-wide. Without this, writev() or
    // send() on a peer-closed socket delivers SIGPIPE which terminates the
    // process before the return-value error path can execute. This is the
    // standard pattern for HTTP servers (nginx, Apache, Go net/http all do
    // the same). MSG_NOSIGNAL covers send() individually, but writev() has
    // no per-call flag — signal(SIGPIPE, SIG_IGN) is the only portable way.
    signal(SIGPIPE, SIG_IGN);

    // NB-2: port range validation (parity with Interpreter/JS)
    if (port < 0 || port > 65535) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: port must be 0-65535, got %lld", (long long)port);
        return taida_async_resolved(taida_net_result_fail("PortError", errbuf));
    }

    // NB-31: handler callable check using compile-time type tag.
    {
        int callable = 0;
        if (handler_type_tag == 6 || handler_type_tag == 10) {
            callable = 1;
        } else if (handler_type_tag == -1) {
            callable = TAIDA_IS_CALLABLE(handler);
        }
        if (!callable) {
            return taida_async_resolved(taida_net_result_fail("TypeError", "httpServe: handler must be a Function"));
        }
    }

    // NET5-4a: TLS configuration — replaced Phase 2 stub with actual implementation.
    // tls is a BuchiPack pointer (non-zero = object) or 0 (default = plaintext).
    // NB5-16: Non-zero non-BuchiPack tls must NOT silently fall back to plaintext.
    // Only 0 (default) and valid BuchiPack pointers are accepted.
    OSSL_SSL_CTX *ssl_ctx = NULL;
    if (tls != 0 && !TAIDA_IS_PACK(tls)) {
        // Non-BuchiPack non-zero value (e.g. tls=42) → reject.
        fprintf(stderr, "RuntimeError: httpServe: tls must be a BuchiPack @(cert: Str, key: Str) or @(), got non-pack value\n");
        fflush(stderr);
        exit(1);
    }
    if (tls != 0) {
        taida_val *pack = (taida_val *)tls;
        int64_t field_count = pack[1];
        if (field_count > 0) {
            // Non-empty tls pack → extract cert and key paths, initialize TLS.
            // Load OpenSSL via dlopen.
            if (!taida_ossl_load()) {
                return taida_async_resolved(taida_net_result_fail("TlsError",
                    "httpServe: TLS/HTTPS requires OpenSSL (libssl.so). "
                    "Install libssl3 or equivalent."));
            }
            // Extract cert and key fields from the BuchiPack.
            taida_val cert_val = taida_pack_get(tls, taida_str_hash((taida_val)"cert"));
            taida_val key_val = taida_pack_get(tls, taida_str_hash((taida_val)"key"));
            if (!cert_val || cert_val <= 4096) {
                return taida_async_resolved(taida_net_result_fail("TlsError",
                    "httpServe: tls config requires 'cert' field (path to PEM certificate file)"));
            }
            if (!key_val || key_val <= 4096) {
                return taida_async_resolved(taida_net_result_fail("TlsError",
                    "httpServe: tls config requires 'key' field (path to PEM private key file)"));
            }
            const char *cert_path = (const char *)cert_val;
            const char *key_path = (const char *)key_val;

            // Create SSL_CTX with cert/key.
            char tls_errbuf[512];
            ssl_ctx = taida_tls_create_ctx(cert_path, key_path, tls_errbuf, sizeof(tls_errbuf));
            if (!ssl_ctx) {
                return taida_async_resolved(taida_net_result_fail("TlsError", tls_errbuf));
            }
        }
        // else: empty @() pack → plaintext, fall through
    }

    // NET2-5d: maxConnections (default 128, <= 0 falls back to 128)
    int64_t max_conn = (max_connections > 0) ? max_connections : 128;

    // Bind to 127.0.0.1:port (v1 contract: always loopback)
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: failed to bind to 127.0.0.1:%d: %s", (int)port, strerror(errno));
        return taida_async_resolved(taida_net_result_fail("BindError", errbuf));
    }

    int opt = 1;
    setsockopt(sockfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    addr.sin_port = htons((unsigned short)port);

    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: failed to bind to 127.0.0.1:%d: %s", (int)port, strerror(errno));
        close(sockfd);
        return taida_async_resolved(taida_net_result_fail("BindError", errbuf));
    }

    if (listen(sockfd, 128) < 0) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: listen failed: %s", strerror(errno));
        close(sockfd);
        return taida_async_resolved(taida_net_result_fail("BindError", errbuf));
    }

    // NET2-5c: Create thread pool
    // Number of worker threads = min(maxConnections, 16) to avoid thread explosion.
    // Each worker handles one connection at a time with keep-alive loop.
    int num_workers = (int)max_conn;
    if (num_workers > 16) num_workers = 16;
    if (num_workers < 1) num_workers = 1;

    NetThreadPool pool;
    net_pool_init(&pool, (int)max_conn + 16, handler, max_requests, timeout_ms, (int)handler_arity);
    pool.ssl_ctx = ssl_ctx; // NET5-4a: NULL = plaintext, non-NULL = TLS

    pthread_t *workers = (pthread_t*)TAIDA_MALLOC(sizeof(pthread_t) * (size_t)num_workers, "net_workers");
    for (int i = 0; i < num_workers; i++) {
        pthread_create(&workers[i], NULL, net_worker_thread, &pool);
    }

    // Accept loop: accept connections and enqueue to worker pool
    for (;;) {
        // NB2-14: Single critical section for both request-limit check and maxConnections wait.
        // Eliminates TOCTOU window from the original unlock-relock pattern.
        pthread_mutex_lock(&pool.mutex);
        if (net_pool_requests_exhausted(&pool)) {
            pthread_mutex_unlock(&pool.mutex);
            break;
        }
        while (pool.active_connections + pool.queue_count >= (int)max_conn && !net_pool_requests_exhausted(&pool)) {
            pthread_cond_wait(&pool.cond_done, &pool.mutex);
        }
        if (net_pool_requests_exhausted(&pool)) {
            pthread_mutex_unlock(&pool.mutex);
            break;
        }
        pthread_mutex_unlock(&pool.mutex);

        // Set a short accept timeout so we can re-check request limits
        {
            struct timeval tv;
            tv.tv_sec = 0;
            tv.tv_usec = 100000;  // 100ms
            setsockopt(sockfd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
        }

        struct sockaddr_in peer_addr;
        socklen_t peer_len = sizeof(peer_addr);
        int client_fd = accept(sockfd, (struct sockaddr*)&peer_addr, &peer_len);
        if (client_fd < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                continue; // timeout or interrupt, re-check limits
            }
            // Fatal accept error
            break;
        }

        // Enqueue to worker pool
        pthread_mutex_lock(&pool.mutex);
        // NB2-10: Close fd if queue is full to prevent fd leak
        if (net_pool_enqueue(&pool, client_fd, peer_addr) < 0) {
            pthread_mutex_unlock(&pool.mutex);
            close(client_fd);
        } else {
            pthread_cond_signal(&pool.cond_available);
            pthread_mutex_unlock(&pool.mutex);
        }
    }

    // NB2-6: Shutdown — close server socket early, drain queued fds, signal workers.
    // Close the listening socket first so no new connections can arrive.
    close(sockfd);

    // Signal all workers to exit and drain any queued-but-unprocessed client fds.
    pthread_mutex_lock(&pool.mutex);
    pool.shutdown = 1;
    // Drain unprocessed queue entries to prevent fd leak
    {
        NetClientSlot drain_slot;
        while (net_pool_dequeue(&pool, &drain_slot) == 0) {
            close(drain_slot.client_fd);
        }
    }
    pthread_cond_broadcast(&pool.cond_available);
    pthread_mutex_unlock(&pool.mutex);

    // Workers currently in recv() will time out within SO_RCVTIMEO (effective_timeout ms).
    for (int i = 0; i < num_workers; i++) {
        pthread_join(workers[i], NULL);
    }

    int64_t final_count = pool.request_count;

    free(workers);
    net_pool_destroy(&pool);

    // NET5-4a: Free TLS context.
    if (ssl_ctx && taida_ossl.loaded) {
        taida_ossl.SSL_CTX_free(ssl_ctx);
    }

    // Server completed successfully
    taida_val ok_inner = taida_pack_new(2);
    taida_pack_set_hash(ok_inner, 0, taida_str_hash((taida_val)"ok"));
    taida_pack_set(ok_inner, 0, 1);  // true
    taida_pack_set_tag(ok_inner, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(ok_inner, 1, taida_str_hash((taida_val)"requests"));
    taida_pack_set(ok_inner, 1, (taida_val)final_count);

    return taida_async_resolved(taida_net_result_ok(ok_inner));
}

int main(int argc, char **argv) {
    taida_cli_argc = argc;
    taida_cli_argv = argv;
    taida_val result = _taida_main();
    return (int)result;
}
