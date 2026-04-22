/// runtime_full_wasm.c -- wasm-full extended runtime (non-prelude functions)
///
/// This file contains functions that are NOT part of the Taida prelude and
/// therefore only available in the wasm-full profile. All prelude functions
/// live in runtime_core_wasm.c (linked by all profiles).
///
/// Contents:
///   - Full-specific overrides (#define redirect targets)
///   - Shadow field registry (for Bytes cursor hash lookups)
///   - Bitwise operations (7 functions)
///   - Global get/set (2 functions)
///   - Bytes runtime (21 functions)
///   - UTF-8 encode/decode molds (5 functions)
///
/// This file references functions from runtime_core_wasm.c via extern.
/// runtime_core_wasm.c is NOT #included; wasm-ld resolves symbols.

#include <stdint.h>

// WCR-2: Minimum heap address — values below this are small integers, not pointers.
// Used by _wasm_is_valid_ptr and type detection heuristics.
#define WASM_MIN_HEAP_ADDR 4096

// ---------------------------------------------------------------------------
// Forward declarations from runtime_core_wasm.c
// (linked via wasm-ld, not #include)
// Only functions actually called from this file are declared here.
// ---------------------------------------------------------------------------
extern void *wasm_alloc(unsigned int size);
extern int64_t taida_lax_new(int64_t value, int64_t default_value);
extern int64_t taida_lax_empty(int64_t default_value);
extern int64_t taida_list_new(void);
extern int64_t taida_list_push(int64_t list_ptr, int64_t item);
extern int64_t taida_pack_new(int64_t field_count);
extern int64_t taida_pack_set(int64_t pack_ptr, int64_t index, int64_t value);
extern int64_t taida_pack_get_idx(int64_t pack_ptr, int64_t index);
extern int64_t taida_pack_set_hash(int64_t pack_ptr, int64_t index, int64_t hash);
extern int64_t taida_hashmap_get_lax(int64_t hm, int64_t kh, int64_t kp);
extern int64_t taida_value_hash(int64_t val);
extern int64_t taida_register_field_name(int64_t hash, int64_t name_ptr);
extern int64_t taida_polymorphic_to_string(int64_t obj);
extern int64_t taida_str_alloc(int64_t len_raw);
extern int64_t taida_str_new_copy(int64_t src_raw);
extern int64_t taida_str_get(int64_t s_raw, int64_t idx_raw);
extern int64_t taida_codepoint_mold_str(int64_t value);
extern int64_t taida_result_is_error(int64_t result);
extern int64_t taida_monadic_to_string(int64_t obj);

// ---------------------------------------------------------------------------
// Float bit-punning helper
// ---------------------------------------------------------------------------
static inline double _to_double(int64_t v) {
    union { int64_t i; double d; } u;
    u.i = v;
    return u.d;
}

// ---------------------------------------------------------------------------
// Local string helpers (no libc available in wasm32)
// Only helpers still needed by remaining full-only functions are kept.
// ---------------------------------------------------------------------------

// WCR-4: Use core's wasm_strlen instead of duplicating
extern int32_t wasm_strlen(const char *s);
extern void *memcpy(void *dest, const void *src, unsigned long n);
#define _wf_strlen(s) ((int)wasm_strlen((s) ? (s) : ""))
#define _wf_memcpy(d,s,n) memcpy((d),(s),(n))

// ---------------------------------------------------------------------------
// Type/pointer detection helpers (used by full-specific overrides and Bytes)
// ---------------------------------------------------------------------------

/* C23B-005 (2026-04-22) — magic constants + validator forward decl so
   the detector helpers can reference them without reordering the
   file. The actual definitions live further down; these must stay in
   sync with the `01_core.inc.c` stamps. */
#define WF_HM_MAGIC_FWD  0x54414944484D5000LL  /* "TAIDHMP\0" */
#define WF_SET_MAGIC_FWD 0x5441494453455400LL  /* "TAIDSET\0" */
#define WF_LIST_MAGIC_FWD 0x544149444C535400LL /* "TAIDLST\0" */
static int _wf_is_valid_ptr(int64_t val, unsigned int min_bytes);

/// Helper: check if a pointer is a positively-identified WASM list.
/// C23B-005 (2026-04-22): matches the `WASM_LIST_MAGIC` stamp that
/// `taida_list_new` writes at `data[3]`. The previous cap/len range
/// heuristic false-positived on any untagged large Int that aliased a
/// heap allocation. Stays in sync with `_looks_like_list` in
/// `src/codegen/runtime_core_wasm/01_core.inc.c`.
static int _wf_looks_like_list(int64_t ptr) {
    if (!_wf_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WF_LIST_MAGIC_FWD || data[3] == WF_SET_MAGIC_FWD;
}

/// Helper: check if a value looks like a valid string pointer in WASM linear memory.
static int _wf_looks_like_string(int64_t val) {
    if (val == 0) return 0;
    if (val < 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr >= mem_size) return 0;
    const char *s = (const char *)(intptr_t)val;
    /* Empty string is a valid string */
    if (s[0] == '\0') return 1;
    /* Verify first few bytes are valid UTF-8/ASCII (not random garbage) */
    for (int i = 0; i < 8 && s[i]; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c < 0x20 && c != '\t' && c != '\n' && c != '\r') return 0;
    }
    return 1;
}

#define WF_LIST_ELEMS 4
/* C23B-005 / C23B-006 (2026-04-22): wide 8-byte printable-ASCII magic
   sentinels matched to the `01_core.inc.c` allocation stamps. The
   previous 4-byte markers false-positived on any int64_t whose memory
   at offset 24 happened to alias those 32-bit values. These constants
   MUST stay in sync with `WASM_HM_MAGIC` / `WASM_SET_MAGIC` /
   `WASM_LIST_MAGIC` in `src/codegen/runtime_core_wasm/01_core.inc.c`. */
#define WF_HM_MARKER_VAL  0x54414944484D5000LL  /* "TAIDHMP\0" */
#define WF_SET_MARKER_VAL 0x5441494453455400LL  /* "TAIDSET\0" */
#define WF_LIST_MARKER_VAL 0x544149444C535400LL /* "TAIDLST\0" */
#define WF_HASH_HAS_VALUE   0x9e9c6dc733414d60LL
#define WF_HASH___VALUE     0x0a7fc9f13472bbe0LL
#define WF_HASH_THROW       0x5a5fe3720c9584cfLL

static int _wf_is_valid_ptr(int64_t val, unsigned int min_bytes) {
    if (val <= 0 || val > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    unsigned int addr = (unsigned int)val;
    if (addr + min_bytes > mem_size) return 0;
    return 1;
}

static int _wf_is_hashmap(int64_t ptr) {
    /* C23B-005 / C23B-006: positive identification with bounds + alignment
       checks. The 8-byte magic (matched against the `01_core.inc.c`
       allocation stamp) is essentially collision-free, so alignment +
       bounds suffice as provenance. */
    if (!_wf_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WF_HM_MARKER_VAL;
}

static int _wf_is_set(int64_t ptr) {
    if (!_wf_is_valid_ptr(ptr, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    if ((addr & 7u) != 0) return 0;
    int64_t *data = (int64_t *)(intptr_t)ptr;
    return data[3] == WF_SET_MARKER_VAL;
}

static int _wf_is_result(int64_t val) {
    if (!_wf_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] == 4 && p[1] == WF_HASH___VALUE) {
        int64_t hash2 = p[1 + 2 * 3];
        if (hash2 == WF_HASH_THROW) return 1;
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Numeric helpers (no libc: manual strtol / i64_to_str)
// Only helpers still needed by remaining full-only functions are kept.
// ---------------------------------------------------------------------------

/// Manual string-to-long (base 10). Returns parsed value and advances *end.
static int64_t _wf_strtol(const char *s, const char **end) {
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

/// Manual int64_t to string. Returns bump-allocated string.
static char *_wf_i64_to_str(int64_t val) {
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

// ---------------------------------------------------------------------------
// FNV-1a hash (kept in full for Bytes cursor etc.)
// ---------------------------------------------------------------------------
static uint64_t _wf_fnv1a(const char *s, int len) {
    uint64_t hash = 0xcbf29ce484222325ULL;
    for (int i = 0; i < len; i++) {
        hash ^= (unsigned char)s[i];
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

// ===========================================================================
// Full-specific overrides (#define redirect targets)
// ===========================================================================

/// Int[str]() -- parse string to int, returning Lax[Int]
/// This overrides core's version which returns raw value (no Lax wrapper).
/// Linked via #define redirect: taida_int_mold_str -> _full.
int64_t taida_int_mold_str_full(int64_t v) {
    const char *s = (const char *)(intptr_t)v;
    if (!s || s[0] == '\0') return taida_lax_empty(0);
    int neg = 0;
    int i = 0;
    if (s[0] == '-') { neg = 1; i = 1; }
    else if (s[0] == '+') { i = 1; }
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
    return taida_lax_empty(0); // parse failed
}

/// Polymorphic .get(key_or_index) -- wasm-full override.
/// The core version returns raw values for List.get(); this version wraps in Lax
/// to match native semantics (OOB returns empty Lax, in-bounds returns Lax[value]).
/// Linked via #define redirect in generated C: taida_collection_get -> _full.
int64_t taida_collection_get_full(int64_t ptr, int64_t item) {
    if (_wf_is_hashmap(ptr)) {
        int64_t key_hash = taida_value_hash(item);
        return taida_hashmap_get_lax(ptr, key_hash, item);
    }
    /* List: index-based access returning Lax (matching native) */
    if (_wf_looks_like_list(ptr)) {
        int64_t *list = (int64_t *)(intptr_t)ptr;
        int64_t len = list[1];
        if (item < 0 || item >= len) return taida_lax_empty(0);
        return taida_lax_new(list[WF_LIST_ELEMS + item], 0);
    }
    /* String: char-at returning Lax */
    if (_wf_looks_like_string(ptr)) {
        return taida_str_get(ptr, item);
    }
    return taida_lax_empty(0);
}

/// Polymorphic .isEmpty() -- wasm-full override.
/// The core version (runtime_core_wasm.c) only handles List/Set/HashMap/String.
/// This version adds Lax, Gorillax, and Result support.
/// Linked via #define redirect in generated C: taida_polymorphic_is_empty -> _full.
int64_t taida_polymorphic_is_empty_full(int64_t ptr) {
    if (ptr == 0) return 1;
    if (!_wf_is_valid_ptr(ptr, 8)) return 0;
    int64_t *p = (int64_t *)(intptr_t)ptr;
    // fc=4 monadic types: Lax/Gorillax/RelaxedGorillax/Result
    // field 0 = hasValue/isOk; isEmpty = !field0
    if (p[0] == 4) {
        if (_wf_is_result(ptr)) return taida_result_is_error(ptr);
        return taida_pack_get_idx(ptr, 0) ? 0 : 1; // Lax/Gorillax
    }
    // HashMap/Set
    if (_wf_is_hashmap(ptr)) {
        int64_t *data = (int64_t *)(intptr_t)ptr;
        return data[1] == 0 ? 1 : 0;
    }
    if (_wf_is_set(ptr)) {
        int64_t *data = (int64_t *)(intptr_t)ptr;
        return data[1] == 0 ? 1 : 0;
    }
    // List
    if (_wf_looks_like_list(ptr)) {
        int64_t *data = (int64_t *)(intptr_t)ptr;
        return data[1] == 0 ? 1 : 0;
    }
    // String
    if (_wf_looks_like_string(ptr)) {
        const char *s = (const char *)(intptr_t)ptr;
        return s[0] == '\0' ? 1 : 0;
    }
    return 0;
}

/// Detect Gorillax type: 0 = unknown, 1 = Gorillax, 2 = RelaxedGorillax
/// Unlike core's version, this does not use > 4096 threshold for the __type string.
/// In wasm-full, data section strings can be at any address.
static int _wf_gorillax_type(int64_t gx) {
    int64_t *p = (int64_t *)(intptr_t)gx;
    // __type field is at index 3: p[1 + 3*3 + 2] = p[12]
    int64_t type_str = p[1 + 3 * 3 + 2];
    if (type_str > 0 && _wf_looks_like_string(type_str)) {
        const char *s = (const char *)(intptr_t)type_str;
        if (s[0] == 'R') return 2; // "RelaxedGorillax"
        if (s[0] == 'G') return 1; // "Gorillax"
    }
    return 1; // default
}

/// Gorillax/RelaxedGorillax toString with proper type detection
static int64_t _wf_gorillax_to_str(int64_t gx) {
    int64_t is_ok = taida_pack_get_idx(gx, 0);
    int gtype = _wf_gorillax_type(gx);
    const char *prefix = (gtype == 2) ? "RelaxedGorillax(" : "Gorillax(";
    int prefix_len = (gtype == 2) ? 16 : 9;
    if (is_ok) {
        int64_t value = taida_pack_get_idx(gx, 1);
        int64_t value_str = taida_polymorphic_to_string(value);
        const char *vs = (const char *)(intptr_t)value_str;
        int vlen = _wf_strlen(vs);
        int need = prefix_len + vlen + 2;
        char *buf = (char *)wasm_alloc((unsigned int)(need));
        _wf_memcpy(buf, prefix, prefix_len);
        _wf_memcpy(buf + prefix_len, vs, vlen);
        buf[prefix_len + vlen] = ')';
        buf[prefix_len + vlen + 1] = '\0';
        return (int64_t)(intptr_t)buf;
    }
    if (gtype == 2) {
        return (int64_t)(intptr_t)"RelaxedGorillax(escaped)";
    }
    return (int64_t)(intptr_t)"Gorillax(><)";
}

/// Polymorphic .toString() -- wasm-full override.
/// Fixes Gorillax/RelaxedGorillax type detection that fails in core due to
/// the > 4096 address threshold for data section strings.
/// Linked via #define redirect: taida_polymorphic_to_string -> _full.
int64_t taida_polymorphic_to_string_full(int64_t obj) {
    if (obj == 0) return (int64_t)(intptr_t)"0";
    // Check Gorillax BEFORE delegating to core (core's gorillax type detection
    // has the > 4096 threshold issue)
    if (_wf_is_valid_ptr(obj, 104)) {
        int64_t *p = (int64_t *)(intptr_t)obj;
        if (p[0] == 4 && p[1] == 0x6550c1c5b98b56bfLL) { // WASM_HASH_IS_OK
            return _wf_gorillax_to_str(obj);
        }
    }
    // Delegate everything else to core
    return taida_polymorphic_to_string(obj);
}

// ===========================================================================
// Shadow field registry (for wasm-full overrides)
// ===========================================================================

#define WF_FIELD_REGISTRY_MAX 256

static struct {
    int64_t hash;
    const char *name;
    int64_t type_tag;
} _wf_field_registry[WF_FIELD_REGISTRY_MAX];
static int _wf_field_registry_count = 0;

/// Register field name+type into full's shadow registry, then delegate to core.
int64_t taida_register_field_name_full(int64_t hash, int64_t name_ptr) {
    // Store in shadow registry
    for (int i = 0; i < _wf_field_registry_count; i++) {
        if (_wf_field_registry[i].hash == hash) goto skip_add;
    }
    if (_wf_field_registry_count < WF_FIELD_REGISTRY_MAX) {
        _wf_field_registry[_wf_field_registry_count].hash = hash;
        _wf_field_registry[_wf_field_registry_count].name = (const char *)(intptr_t)name_ptr;
        _wf_field_registry[_wf_field_registry_count].type_tag = -1;
        _wf_field_registry_count++;
    }
skip_add:
    // Delegate to core's original
    return taida_register_field_name(hash, name_ptr);
}

int64_t taida_register_field_type_full(int64_t hash, int64_t name_ptr, int64_t type_tag) {
    // Update shadow registry
    for (int i = 0; i < _wf_field_registry_count; i++) {
        if (_wf_field_registry[i].hash == hash) {
            _wf_field_registry[i].type_tag = type_tag;
            goto do_core;
        }
    }
    if (_wf_field_registry_count < WF_FIELD_REGISTRY_MAX) {
        _wf_field_registry[_wf_field_registry_count].hash = hash;
        _wf_field_registry[_wf_field_registry_count].name = (const char *)(intptr_t)name_ptr;
        _wf_field_registry[_wf_field_registry_count].type_tag = type_tag;
        _wf_field_registry_count++;
    }
do_core:;
    // Core's register_field_type is defined in runtime_core_wasm.c (non-static)
    extern int64_t taida_register_field_type(int64_t, int64_t, int64_t);
    return taida_register_field_type(hash, name_ptr, type_tag);
}

// ===========================================================================
// Bitwise operations (7 functions)
// ===========================================================================

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

// ===========================================================================
// Global get/set (2 functions)
// ===========================================================================

#define WF_GLOBAL_TABLE_SIZE 64
static int64_t _wf_global_keys[WF_GLOBAL_TABLE_SIZE];
static int64_t _wf_global_vals[WF_GLOBAL_TABLE_SIZE];
static int _wf_global_used[WF_GLOBAL_TABLE_SIZE];

int64_t taida_global_set(int64_t key, int64_t val) {
    unsigned int idx = (unsigned int)((uint64_t)key % WF_GLOBAL_TABLE_SIZE);
    for (int i = 0; i < WF_GLOBAL_TABLE_SIZE; i++) {
        unsigned int slot = (idx + (unsigned int)i) % WF_GLOBAL_TABLE_SIZE;
        if (!_wf_global_used[slot] || _wf_global_keys[slot] == key) {
            _wf_global_keys[slot] = key;
            _wf_global_vals[slot] = val;
            _wf_global_used[slot] = 1;
            return 0;
        }
    }
    return 0;
}

int64_t taida_global_get(int64_t key) {
    unsigned int idx = (unsigned int)((uint64_t)key % WF_GLOBAL_TABLE_SIZE);
    for (int i = 0; i < WF_GLOBAL_TABLE_SIZE; i++) {
        unsigned int slot = (idx + (unsigned int)i) % WF_GLOBAL_TABLE_SIZE;
        if (!_wf_global_used[slot]) return 0;
        if (_wf_global_keys[slot] == key) return _wf_global_vals[slot];
    }
    return 0;
}

// ===========================================================================
// Bytes runtime (21 functions)
// ===========================================================================

// Bytes layout (WASM): [BYTES_MAGIC, len, byte0, byte1, ...]
// No RC header in WASM -- bump allocator.
#define WF_BYTES_MAGIC 0x5441494442595400LL  // "TAIDBYT\0"

static int _wf_is_bytes(int64_t val) {
    if (!_wf_is_valid_ptr(val, 16)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    return (p[0] & 0xFFFFFFFFFFFFFF00LL) == WF_BYTES_MAGIC;
}

int64_t taida_bytes_default_value(void) {
    int64_t *bytes = (int64_t *)wasm_alloc(3 * 8);
    bytes[0] = WF_BYTES_MAGIC;
    bytes[1] = 0;  // len
    bytes[2] = 0;
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_len(int64_t bytes_ptr) {
    if (!_wf_is_bytes(bytes_ptr)) return 0;
    return ((int64_t *)(intptr_t)bytes_ptr)[1];
}

int64_t taida_bytes_new_filled(int64_t len, int64_t fill) {
    if (len < 0) len = 0;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    bytes[0] = WF_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = fill;
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_from_raw(int64_t ptr, int64_t len) {
    if (len < 0) len = 0;
    const unsigned char *data = (const unsigned char *)(intptr_t)ptr;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    bytes[0] = WF_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = (int64_t)data[i];
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_clone(int64_t bytes_ptr) {
    if (!_wf_is_bytes(bytes_ptr)) return taida_bytes_default_value();
    int64_t *src = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = src[1];
    int64_t *dst = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    for (int64_t i = 0; i < 2 + len; i++) dst[i] = src[i];
    return (int64_t)(intptr_t)dst;
}

int64_t taida_bytes_get_lax(int64_t bytes_ptr, int64_t idx) {
    if (!_wf_is_bytes(bytes_ptr)) return taida_lax_empty(0);
    int64_t len = ((int64_t *)(intptr_t)bytes_ptr)[1];
    if (idx < 0 || idx >= len) return taida_lax_empty(0);
    int64_t val = ((int64_t *)(intptr_t)bytes_ptr)[2 + idx];
    return taida_lax_new(val, 0);
}

int64_t taida_bytes_set(int64_t bytes_ptr, int64_t idx, int64_t val) {
    if (!_wf_is_bytes(bytes_ptr)) return taida_lax_empty(taida_bytes_default_value());
    int64_t len = ((int64_t *)(intptr_t)bytes_ptr)[1];
    if (idx < 0 || idx >= len) return taida_lax_empty(taida_bytes_default_value());
    if (val < 0 || val > 255) return taida_lax_empty(taida_bytes_default_value());
    int64_t out = taida_bytes_clone(bytes_ptr);
    ((int64_t *)(intptr_t)out)[2 + idx] = val;
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_bytes_to_list(int64_t bytes_ptr) {
    int64_t list = taida_list_new();
    if (!_wf_is_bytes(bytes_ptr)) return list;
    int64_t *bytes = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = bytes[1];
    for (int64_t i = 0; i < len; i++) {
        list = taida_list_push(list, bytes[2 + i]);
    }
    return list;
}

/* Minimal string buffer for Bytes display */
typedef struct { char *buf; int len; int cap; } _wf_strbuf;
static void _wf_sb_init(_wf_strbuf *sb) {
    sb->cap = 256; sb->buf = (char *)wasm_alloc(sb->cap); sb->len = 0;
    if (sb->buf) sb->buf[0] = '\0';
}
static void _wf_sb_append(_wf_strbuf *sb, const char *s) {
    int slen = _wf_strlen(s);
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
    if (!_wf_is_bytes(bytes_ptr)) {
        return taida_str_new_copy((int64_t)(intptr_t)"Bytes()");
    }
    int64_t *bytes = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = bytes[1];
    _wf_strbuf jb;
    _wf_sb_init(&jb);
    _wf_sb_append(&jb, "Bytes([");
    for (int64_t i = 0; i < len; i++) {
        if (i > 0) _wf_sb_append(&jb, ", ");
        char *s = _wf_i64_to_str(bytes[2 + i]);
        _wf_sb_append(&jb, s);
    }
    _wf_sb_append(&jb, "])");
    return (int64_t)(intptr_t)jb.buf;
}

int64_t taida_bytes_mold(int64_t value, int64_t fill) {
    if (_wf_is_bytes(value)) {
        int64_t cloned = taida_bytes_clone(value);
        return taida_lax_new(cloned, taida_bytes_default_value());
    }
    if (_wf_looks_like_list(value)) {
        int64_t *list = (int64_t *)(intptr_t)value;
        int64_t len = list[1];
        int64_t out = taida_bytes_new_filled(len, 0);
        int64_t *ob = (int64_t *)(intptr_t)out;
        for (int64_t i = 0; i < len; i++) {
            int64_t item = list[4 + i];
            if (item < 0 || item > 255) {
                return taida_lax_empty(taida_bytes_default_value());
            }
            ob[2 + i] = item;
        }
        return taida_lax_new(out, taida_bytes_default_value());
    }
    // String
    if (_wf_looks_like_string(value)) {
        const char *s = (const char *)(intptr_t)value;
        int slen = _wf_strlen(s);
        int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)s, (int64_t)slen);
        return taida_lax_new(out, taida_bytes_default_value());
    }
    // Integer length
    int64_t len = value;
    if (len < 0 || len > 10000000) return taida_lax_empty(taida_bytes_default_value());
    if (fill < 0 || fill > 255) return taida_lax_empty(taida_bytes_default_value());
    int64_t out = taida_bytes_new_filled(len, (int64_t)(unsigned char)fill);
    return taida_lax_new(out, taida_bytes_default_value());
}

// --- Bytes cursor ---
#define WF_HASH_CURSOR_BYTES   0xb66db66da4a4c5a2LL
#define WF_HASH_CURSOR_OFFSET  0x1234567890abcdefLL
#define WF_HASH_CURSOR_LENGTH  0xfedcba0987654321LL
#define WF_HASH_STEP_VALUE     0x0a7fc9f13472bbe0LL  /* FNV-1a("__value") */
#define WF_HASH_STEP_CURSOR    0xb66db66da4a4c5a3LL

static int _wf_bytes_cursor_unpack(int64_t cursor_ptr, int64_t *bytes_out, int64_t *offset_out) {
    // Cursor is a Pack with fields: [bytes, offset, length, __type]
    if (!_wf_is_valid_ptr(cursor_ptr, 8)) return 0;
    int64_t *pack = (int64_t *)(intptr_t)cursor_ptr;
    int64_t fc = pack[0];
    if (fc < 2) return 0;
    int64_t bytes_ptr = pack[1 + 0 * 3 + 2]; // field 0 value
    int64_t offset = pack[1 + 1 * 3 + 2];     // field 1 value
    if (!_wf_is_bytes(bytes_ptr)) return 0;
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    *bytes_out = bytes_ptr;
    *offset_out = offset;
    return 1;
}

static int64_t _wf_bytes_cursor_step(int64_t value, int64_t cursor) {
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, WF_HASH_STEP_VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, WF_HASH_STEP_CURSOR);
    taida_pack_set(pack, 1, cursor);
    return pack;
}

int64_t taida_bytes_cursor_new(int64_t bytes_ptr, int64_t offset) {
    if (!_wf_is_bytes(bytes_ptr)) bytes_ptr = taida_bytes_default_value();
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WF_HASH_CURSOR_BYTES);
    taida_pack_set(pack, 0, bytes_ptr);
    taida_pack_set_hash(pack, 1, WF_HASH_CURSOR_OFFSET);
    taida_pack_set(pack, 1, offset);
    taida_pack_set_hash(pack, 2, WF_HASH_CURSOR_LENGTH);
    taida_pack_set(pack, 2, len);
    uint64_t type_hash = _wf_fnv1a("__type", 6);
    taida_pack_set_hash(pack, 3, (int64_t)type_hash);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"BytesCursor");
    return pack;
}

int64_t taida_bytes_cursor_u8(int64_t cursor_ptr) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wf_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        int64_t empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        return taida_lax_empty(_wf_bytes_cursor_step(0, empty_cursor));
    }
    int64_t cur = taida_bytes_cursor_new(bytes_ptr, offset);
    int64_t def_step = _wf_bytes_cursor_step(0, cur);
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset >= len) return taida_lax_empty(def_step);
    int64_t val = ((int64_t *)(intptr_t)bytes_ptr)[2 + offset];
    int64_t next = taida_bytes_cursor_new(bytes_ptr, offset + 1);
    return taida_lax_new(_wf_bytes_cursor_step(val, next), def_step);
}

int64_t taida_bytes_cursor_take(int64_t cursor_ptr, int64_t size) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wf_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        int64_t empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        return taida_lax_empty(_wf_bytes_cursor_step(taida_bytes_default_value(), empty_cursor));
    }
    int64_t cur = taida_bytes_cursor_new(bytes_ptr, offset);
    int64_t def_step = _wf_bytes_cursor_step(taida_bytes_default_value(), cur);
    if (size < 0) return taida_lax_empty(def_step);
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset + size > len) return taida_lax_empty(def_step);
    int64_t *src = (int64_t *)(intptr_t)bytes_ptr;
    int64_t out = taida_bytes_new_filled(size, 0);
    int64_t *dst = (int64_t *)(intptr_t)out;
    for (int64_t i = 0; i < size; i++) dst[2 + i] = src[2 + offset + i];
    int64_t next = taida_bytes_cursor_new(bytes_ptr, offset + size);
    return taida_lax_new(_wf_bytes_cursor_step(out, next), def_step);
}

int64_t taida_bytes_cursor_step(int64_t cursor_ptr, int64_t n) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wf_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        return taida_bytes_cursor_new(taida_bytes_default_value(), 0);
    }
    return taida_bytes_cursor_new(bytes_ptr, offset + n);
}

int64_t taida_bytes_cursor_remaining(int64_t cursor_ptr) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wf_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
    return taida_bytes_len(bytes_ptr) - offset;
}

int64_t taida_bytes_cursor_unpack(int64_t cursor_ptr, int64_t schema) {
    (void)schema;
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wf_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
    return bytes_ptr;
}

// --- u16/u32 encode/decode molds ---
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
    if (!_wf_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 2) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint16_t n = (uint16_t)(((unsigned)b[2] << 8) | (unsigned)b[3]);
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u16le_decode_mold(int64_t value) {
    if (!_wf_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 2) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint16_t n = (uint16_t)((unsigned)b[2] | ((unsigned)b[3] << 8));
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u32be_decode_mold(int64_t value) {
    if (!_wf_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 4) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint32_t n = ((uint32_t)b[2] << 24) | ((uint32_t)b[3] << 16) | ((uint32_t)b[4] << 8) | (uint32_t)b[5];
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u32le_decode_mold(int64_t value) {
    if (!_wf_is_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) < 4) return taida_lax_empty(0);
    int64_t *b = (int64_t *)(intptr_t)value;
    uint32_t n = (uint32_t)b[2] | ((uint32_t)b[3] << 8) | ((uint32_t)b[4] << 16) | ((uint32_t)b[5] << 24);
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_uint8_mold(int64_t v) {
    int64_t parsed = v;
    if (_wf_looks_like_string(v)) {
        const char *s = (const char *)(intptr_t)v;
        const char *end;
        parsed = _wf_strtol(s, &end);
        if (*end != '\0') parsed = v;
    }
    if (parsed < 0 || parsed > 255) return taida_lax_empty(0);
    return taida_lax_new(parsed, 0);
}

int64_t taida_uint8_mold_float(int64_t v) {
    double d = _to_double(v);
    if (d != d) return taida_lax_empty(0); // NaN
    if (d < 0.0 || d > 255.0) return taida_lax_empty(0);
    // Check integer
    double fl = (double)(int64_t)d;
    if (fl != d) return taida_lax_empty(0);
    return taida_lax_new((int64_t)d, 0);
}

// ===========================================================================
// UTF-8 encode/decode molds (5 functions)
// ===========================================================================

static int _wf_utf8_decode_one(const unsigned char *buf, int len, int *consumed, uint32_t *out_cp) {
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
    if (!s || !_wf_looks_like_string(value)) {
        return taida_lax_empty(taida_bytes_default_value());
    }
    int slen = _wf_strlen(s);
    int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)s, (int64_t)slen);
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_utf8_decode_mold(int64_t value) {
    if (!_wf_is_bytes(value)) return taida_lax_empty(taida_str_alloc(0));
    int64_t *bytes = (int64_t *)(intptr_t)value;
    int64_t len = bytes[1];
    // Extract raw bytes
    unsigned char *raw = (unsigned char *)wasm_alloc((unsigned int)len);
    for (int64_t i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];
    // Validate UTF-8
    int pos = 0;
    while (pos < (int)len) {
        int consumed = 0;
        uint32_t cp = 0;
        if (!_wf_utf8_decode_one(raw + pos, (int)len - pos, &consumed, &cp)) {
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
    return taida_utf8_encode_mold(v);
}

int64_t taida_utf8_decode_one(int64_t v) {
    return taida_utf8_decode_mold(v);
}

int64_t taida_utf8_single_scalar(int64_t v) {
    return taida_codepoint_mold_str(v);
}
