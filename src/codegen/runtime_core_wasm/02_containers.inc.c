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

/* B11B-015: Runtime type check for TypeIs with named types (WASM version).
   Gets __type from the BuchiPack and walks the inheritance chain.
   Returns 1 (true) or 0 (false). */
int64_t taida_typeis_named(int64_t val, int64_t expected_type_str) {
    if (!taida_is_buchi_pack(val)) return 0;
    int64_t type_str = 0;
    if (taida_pack_has_hash(val, WASM_HASH___TYPE)) {
        type_str = taida_pack_get(val, WASM_HASH___TYPE);
    }
    if (type_str == 0) return 0;
    /* Direct match */
    if (taida_str_eq(type_str, expected_type_str)) return 1;
    /* Walk inheritance chain */
    int64_t current = type_str;
    for (int i = 0; i < 64; i++) {
        int64_t parent = wasm_find_parent_type(current);
        if (parent == 0) break;
        if (taida_str_eq(parent, expected_type_str)) return 1;
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

/* C21B-seed-07: Register Lax's four field names so
   `_wasm_pack_to_string_full` can surface them in the interpreter-parity
   stdout form `@(hasValue <= …, __value <= …, __default <= …, __type <=
   "Lax")`. Without this, the lookup returns NULL and the field is skipped
   entirely — the symptom observed on wasm-wasi was `@()` for any Lax
   produced by `Int[x]()` / `Float[x]()` / `Bool[x]()` / `Str[x]()`. */
static int _wasm_lax_names_registered = 0;
static void _wasm_register_lax_field_names(void) {
    if (_wasm_lax_names_registered) return;
    _wasm_lax_names_registered = 1;
    taida_register_field_name(WASM_HASH_HAS_VALUE, (int64_t)(intptr_t)"hasValue");
    taida_register_field_name(WASM_HASH___VALUE,   (int64_t)(intptr_t)"__value");
    taida_register_field_name(WASM_HASH___DEFAULT, (int64_t)(intptr_t)"__default");
    taida_register_field_name(WASM_HASH___TYPE,    (int64_t)(intptr_t)"__type");
}

int64_t taida_lax_new(int64_t value, int64_t default_value) {
    _wasm_register_lax_field_names();
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
    /* Tag the __type slot as STR so `_wasm_pack_to_string_full` quotes it
       correctly (matches interpreter's `__type <= "Lax"`). */
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

int64_t taida_lax_empty(int64_t default_value) {
    _wasm_register_lax_field_names();
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
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
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

/* Forward declare: check if a value is a Lax pack.
   C24-A (2026-04-23): Gorillax now also uses `hash0 = HASH_HAS_VALUE`
   (previously `HASH_IS_OK`), so structural-only disambiguation by
   `p[0] == 4 && p[1] == HASH_HAS_VALUE` would match both Lax AND
   Gorillax / RelaxedGorillax. We disambiguate by the field-2 hash:
   - Lax:             slot-2 hash = WASM_HASH___DEFAULT
   - Gorillax / RelaxedGorillax: slot-2 hash = WASM_HASH___ERROR
   This avoids a pointer dereference + _looks_like_string call,
   keeping the wasm-min size gate green. The pack layout places
   field 2's hash at offset `1 + 2*3 = 7`. */
static int _wasm_is_lax(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] != 4 || p[1] != WASM_HASH_HAS_VALUE) return 0;
    /* Reject Gorillax / RelaxedGorillax (slot-2 hash == __error). */
    return p[1 + 2 * 3] == WASM_HASH___DEFAULT ? 1 : 0;
}

/* ── W-5: Gorillax (Result container) ── */
/* Gorillax: @(hasValue: Bool, __value: T, __error: Error, __type: "Gorillax")
   Using pack fields at fixed indices.

   C24-A (2026-04-23): unified Gorillax first-field name from `isOk` to
   `hasValue` so `Str[Gorillax[v]()]()` on wasm matches the interpreter /
   JS / native output byte-for-byte. The old `isOk` field name was WASM
   internal-only — no user-facing `.isOk()` method dispatches to this
   slot (that method lives on `Result`, routed through
   `taida_result_is_ok`). Disambiguation with Lax (which shares
   `hash0 = HASH_HAS_VALUE` after the rename) is performed on the
   `__type` first character by `_wasm_is_gorillax` / `_wasm_is_lax` in
   `01_core.inc.c` — Gorillax = 'G', RelaxedGorillax = 'R', Lax = 'L'. */

/* WASM_HASH_HAS_VALUE / __ERROR defined in W-5f monadic type hash section */

/* Idempotent registration of Gorillax field names into the global
   `_wasm_field_registry` so `_wasm_pack_to_string_full` can resolve the
   `__error` field (previously unregistered, which caused
   `Str[Gorillax[v]()]()` to silently skip the error slot). The other
   three fields (`hasValue`, `__value`, `__type`) are already registered
   by `_wasm_register_lax_field_names`, but we re-register them here as
   a defence-in-depth so the Gorillax path is self-sufficient. */
static void _wasm_register_gorillax_field_names(void) {
    taida_register_field_name(WASM_HASH_HAS_VALUE, (int64_t)(intptr_t)"hasValue");
    taida_register_field_name(WASM_HASH___VALUE,   (int64_t)(intptr_t)"__value");
    taida_register_field_name(WASM_HASH___ERROR,   (int64_t)(intptr_t)"__error");
    taida_register_field_name(WASM_HASH___TYPE,    (int64_t)(intptr_t)"__type");
}

int64_t taida_gorillax_new(int64_t value) {
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1); /* hasValue = true */
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    /* C24-A: store an empty pack pointer (not raw 0) so
       `_wasm_value_to_debug_string_full` renders `__error <= @()`
       instead of the `"0"` literal. Tagged as PACK so the display
       branch recurses into the full-form helper. Matches the native
       `taida_gorillax_new` tagging (TAIDA_TAG_PACK + raw 0 → `@()`
       via native's dedicated empty-pack branch). */
    taida_pack_set(pack, 2, taida_pack_new(0));
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Gorillax");
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

int64_t taida_gorillax_err(int64_t error) {
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0); /* hasValue = false */
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, 0);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, error);
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Gorillax");
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
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
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, taida_pack_get_idx(gx, 0));
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, taida_pack_get_idx(gx, 1));
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, taida_pack_get_idx(gx, 2));
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"RelaxedGorillax");
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

int64_t taida_relaxed_gorillax_new(int64_t value) {
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1);
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, taida_pack_new(0));
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"RelaxedGorillax");
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

int64_t taida_relaxed_gorillax_err(int64_t error) {
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, 0);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, error);
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"RelaxedGorillax");
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

/* ── W-5: Result[T, P] ── */
/* Result: @(__value: T, __predicate: P, throw: Error, __type: "Result")
   field 0: __value, field 1: __predicate, field 2: throw, field 3: __type */

/* WASM_HASH___PREDICATE, WASM_HASH_THROW defined in W-5f monadic type hash section */

/* C25B-028: register Result's `__predicate` / `throw` field names so
   jsonEncode (and any other name-lookup based renderer) can see them.
   Idempotent — mirrors the C23B-009 pattern used for Lax / Gorillax. */
static int _wasm_result_names_registered = 0;
static void _wasm_register_result_field_names(void) {
    if (_wasm_result_names_registered) return;
    _wasm_result_names_registered = 1;
    taida_register_field_name(WASM_HASH___VALUE,     (int64_t)(intptr_t)"__value");
    taida_register_field_name(WASM_HASH___PREDICATE, (int64_t)(intptr_t)"__predicate");
    taida_register_field_name(WASM_HASH_THROW,       (int64_t)(intptr_t)"throw");
    taida_register_field_name(WASM_HASH___TYPE,      (int64_t)(intptr_t)"__type");
}

int64_t taida_result_create(int64_t value, int64_t throw_val, int64_t predicate) {
    _wasm_register_result_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH___VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, WASM_HASH___PREDICATE);
    taida_pack_set(pack, 1, predicate);
    taida_pack_set_hash(pack, 2, WASM_HASH_THROW);
    taida_pack_set(pack, 2, throw_val);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, (int64_t)(intptr_t)"Result");
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

/* C25B-001: Stream[val]() — minimal wasm wrapper. Mirrors
   src/codegen/native_runtime/core.c::taida_stream_new. Produces a pack
   whose `Str[]()` rendering matches the interpreter's
   `Value::Stream -> "Stream[completed: N items]"` shape. Full lazy
   transform chain remains interpreter-only until a later Phase.

   Layout: fc=3 pack
     field 0: __stream_status = "completed" (Str)
     field 1: __stream_count  = 1 (Int)
     field 2: __type          = "Stream" (Str)
*/
#define WASM_HASH_STREAM_STATUS 0x6d32b928f2c5d8aeLL  /* FNV-1a("__stream_status") */
#define WASM_HASH_STREAM_COUNT  0x1c0dd3a9e6fd1178LL  /* FNV-1a("__stream_count") */

static int _wasm_stream_names_registered = 0;
static void _wasm_register_stream_field_names(void) {
    if (_wasm_stream_names_registered) return;
    _wasm_stream_names_registered = 1;
    taida_register_field_name(WASM_HASH_STREAM_STATUS, (int64_t)(intptr_t)"__stream_status");
    taida_register_field_name(WASM_HASH_STREAM_COUNT,  (int64_t)(intptr_t)"__stream_count");
}

int64_t taida_stream_new(int64_t inner_value) {
    (void)inner_value; /* Minimal impl: retain only the item count (=1). */
    _wasm_register_stream_field_names();
    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, WASM_HASH_STREAM_STATUS);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)"completed");
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    taida_pack_set_hash(pack, 1, WASM_HASH_STREAM_COUNT);
    taida_pack_set(pack, 1, 1);
    taida_pack_set_tag(pack, 1, WASM_TAG_INT);
    taida_pack_set_hash(pack, 2, WASM_HASH___TYPE);
    taida_pack_set(pack, 2, (int64_t)(intptr_t)"Stream");
    taida_pack_set_tag(pack, 2, WASM_TAG_STR);
    return pack;
}

/* C25B-001: forward declare `taida_str_alloc` so Stream display can
   use the bump allocator; the definition lives later in this file. */
int64_t taida_str_alloc(int64_t len_raw);

/* C25B-001: Stream pack detector (fc=3, hash0=HASH_STREAM_STATUS). */
static int _wasm_is_stream_pack(int64_t obj) {
    if (obj <= 0 || obj > 0xFFFFFFFF) return 0;
    unsigned int pages = __builtin_wasm_memory_size(0);
    unsigned int mem_size = pages * 65536;
    if ((unsigned int)obj + 24 > mem_size) return 0;
    int64_t *p = (int64_t *)(intptr_t)obj;
    if (p[0] != 3) return 0;
    return p[1] == WASM_HASH_STREAM_STATUS ? 1 : 0;
}

/* C25B-001: render Stream as `Stream[completed: N items]` (Str heap-
   allocated via _wasm_str_heap helper). */
static int64_t _wasm_stream_to_display_string(int64_t stream_ptr) {
    int64_t *p = (int64_t *)(intptr_t)stream_ptr;
    int64_t count = p[1 + 1 * 3 + 2]; /* slot 1 value */
    int64_t count_str = taida_int_to_str(count);
    /* Manual concatenation: "Stream[completed: " + count_str + " items]" */
    const char *prefix = "Stream[completed: ";
    const char *suffix = " items]";
    const char *cs = (const char *)(intptr_t)count_str;
    int plen = 0; while (prefix[plen]) plen++;
    int clen = 0; while (cs && cs[clen]) clen++;
    int slen = 0; while (suffix[slen]) slen++;
    int total = plen + clen + slen;
    /* Allocate through taida's bump allocator so the result is a
       well-formed Str pointer in linear memory. */
    char *buf = (char *)(intptr_t)taida_str_alloc(total);
    int off = 0;
    for (int i = 0; i < plen; i++) buf[off++] = prefix[i];
    for (int i = 0; i < clen; i++) buf[off++] = cs[i];
    for (int i = 0; i < slen; i++) buf[off++] = suffix[i];
    buf[off] = '\0';
    return (int64_t)(intptr_t)buf;
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
/*
 * C21B-seed-07: stamp the per-field primitive tag on `__value` (index 1)
 * and `__default` (index 2) so the new `_wasm_pack_to_display_string_full`
 * dispatcher can render Float / Str correctly in the interpreter-parity
 * full form. The Float and Bool molds already had this stamping (kept for
 * historical reasons); Int and Str now follow suit.
 */

static inline int64_t _lax_tag_vd(int64_t lax, int64_t tag) {
    taida_pack_set_tag(lax, 1, tag);
    taida_pack_set_tag(lax, 2, tag);
    return lax;
}

int64_t taida_str_mold_int(int64_t v) {
    return _lax_tag_vd(taida_lax_new(taida_int_to_str(v), (int64_t)(intptr_t)""), WASM_TAG_STR);
}

/* C23-4: Rust-`f64::to_string`-compatible formatter for `Str[Float]()`.
   The shared `taida_float_to_str` renders integer-form floats with a trailing
   `.0` to match `Value::to_display_string` (so `stdout(3.0)` prints `3.0`),
   but the interpreter's `Str[3.0]() -> Lax[Str]` stores `f.to_string()` which
   uses the shortest-round-trip form WITHOUT a trailing `.0` for integer-
   valued floats (`"3"` / `"-5"` / `"0"`). This local helper mirrors that
   contract by dropping the `.0` suffix produced by `fmt_g` on integer-valued
   floats, while keeping fractional / NaN / inf / exponential forms intact.
   See `src/interpreter/mold_eval.rs:2057` for the reference. */
static int64_t _taida_float_to_str_mold(int64_t val) {
    int64_t raw = taida_float_to_str(val);
    const char *s = (const char *)(intptr_t)raw;
    if (!s) return raw;
    int len = 0;
    while (s[len]) len++;
    /* Strip a trailing `.0` (but not `.14`, `.0e5`, `.1`, …) so that
       `3.0`/`-5.0`/`0.0` collapse to `3`/`-5`/`0`. NaN / inf / exponent forms
       have no trailing `.0` so they pass through unchanged. */
    if (len >= 3 && s[len - 2] == '.' && s[len - 1] == '0') {
        int keep = len - 2;
        char *buf = (char *)wasm_alloc(keep + 1);
        if (!buf) return raw;
        for (int i = 0; i < keep; i++) buf[i] = s[i];
        buf[keep] = '\0';
        return (int64_t)(intptr_t)buf;
    }
    return raw;
}

int64_t taida_str_mold_float(int64_t v) {
    return _lax_tag_vd(taida_lax_new(_taida_float_to_str_mold(v), (int64_t)(intptr_t)""), WASM_TAG_STR);
}

int64_t taida_str_mold_bool(int64_t v) {
    return _lax_tag_vd(taida_lax_new(taida_str_from_bool(v), (int64_t)(intptr_t)""), WASM_TAG_STR);
}

int64_t taida_str_mold_str(int64_t v) {
    return _lax_tag_vd(taida_lax_new(v, (int64_t)(intptr_t)""), WASM_TAG_STR);
}

/* C23-2: generic Str[x]() entry for non-primitive values (List/Pack/Lax/
   Result/…). Routes through `_wasm_stdout_display_string` so BuchiPacks
   render as full-form (`@(field <= value, …)` including `__`-prefixed
   internals), matching the interpreter's `format!("{}", other)` contract
   instead of the short-form `Lax(v)` / pointer-integer fallback used by
   `_wasm_value_to_display_string`. Symmetric with native's
   `taida_str_mold_any`. */
int64_t taida_str_mold_any(int64_t v) {
    int64_t str = _wasm_stdout_display_string(v);
    return _lax_tag_vd(taida_lax_new(str, (int64_t)(intptr_t)""), WASM_TAG_STR);
}

int64_t taida_int_mold_int(int64_t v) {
    return _lax_tag_vd(taida_lax_new(v, 0), WASM_TAG_INT);
}

int64_t taida_int_mold_float(int64_t v) {
    return _lax_tag_vd(taida_lax_new(taida_float_to_int(v), 0), WASM_TAG_INT);
}

int64_t taida_int_mold_bool(int64_t v) {
    return _lax_tag_vd(taida_lax_new(v != 0 ? 1 : 0, 0), WASM_TAG_INT);
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

/* C12-6c (FB-5) — wasm fallback stubs for the Str method Regex overloads.
 *
 * wasm profiles (min / wasi / full) do NOT link POSIX regex.h. To keep the
 * lowering signature stable across backends (lowered code calls the
 * `_poly` wrapper unconditionally), these stubs forward to the
 * fixed-string equivalents by treating the second argument as a
 * `const char*`. Passing an actual Regex pack here would read through
 * a BuchiPack header as if it were a C string — behaviour is
 * undefined. Parity tests for Regex overloads therefore run against
 * Interpreter / JS / Native only (design lock §C12-6); wasm targets
 * must stick to fixed-string usage.
 */
int64_t taida_str_split_poly(int64_t s_raw, int64_t sep_raw) {
    return taida_str_split(s_raw, sep_raw);
}
int64_t taida_str_replace_first_poly(int64_t s_raw, int64_t target_raw, int64_t rep_raw) {
    return taida_str_replace_first(s_raw, target_raw, rep_raw);
}
int64_t taida_str_replace_poly(int64_t s_raw, int64_t target_raw, int64_t rep_raw) {
    return taida_str_replace(s_raw, target_raw, rep_raw);
}
int64_t taida_str_match_regex(int64_t s_raw, int64_t regex_raw) {
    (void)s_raw; (void)regex_raw;
    /* No regex support — return 0 (Int 0 / empty pointer-like). Taida
     * programs on wasm should not rely on Regex methods. */
    return 0;
}
int64_t taida_str_search_regex(int64_t s_raw, int64_t regex_raw) {
    (void)s_raw; (void)regex_raw;
    return -1;
}
int64_t taida_regex_new(int64_t pattern_raw, int64_t flags_raw) {
    /* Build a minimal Regex "pack" that callers can inspect only if
     * they go back through the Regex-aware code paths (which don't
     * exist on wasm). Return the pattern pointer so that downstream
     * stubs at least don't crash. */
    (void)flags_raw;
    return pattern_raw;
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

