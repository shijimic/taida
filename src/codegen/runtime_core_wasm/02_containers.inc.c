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
/* Non-static: the generated C reads it after every call that can throw
   and returns at once, so no statement runs between a throw and its
   ceiling (the interpreter / native unwind the same way). */
int __wasm_error_thrown = 0;

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
    /* A throw is already on its way to the ceiling: this one sits on the
       dead path (e.g. a Result predicate threw and the unmold then
       reports the failed predicate). The first throw wins, as it does
       when the stack unwinds. */
    if (__wasm_error_thrown) return 0;
    if (__wasm_error_depth > 0) {
        int depth = __wasm_error_depth - 1;
        __wasm_error_val[depth] = error_val;
        __wasm_error_thrown = 1;
        return 0; /* caller should check __wasm_error_thrown */
    }
    /* No error ceiling: match the interpreter's unhandled report —
       `Runtime error: Unhandled error: ...` on stderr, exit 1; packs
       render as `Error[type]: message`. A missing type is AnonymousError;
       an omitted message shows the type name, an empty one stays empty. */
    {
        wasi_ciovec iov[6];
        int n = 0;
        const char *head = "Runtime error: Unhandled error: ";
        iov[n].buf = (int32_t)(intptr_t)head; iov[n].len = wasm_strlen(head); n++;
        if (taida_is_buchi_pack(error_val)) {
            const char *ty = "AnonymousError";
            const char *m = (const char *)0;
            if (taida_pack_has_hash(error_val, WASM_HASH_TYPE)) {
                int64_t tv = taida_pack_get(error_val, WASM_HASH_TYPE);
                if (tv && _looks_like_string(tv)
                    && ((const char *)(intptr_t)tv)[0] != '\0')
                    ty = (const char *)(intptr_t)tv;
            }
            if (_wf_strcmp(ty, "AnonymousError") == 0
                && taida_pack_has_hash(error_val, WASM_HASH___TYPE)) {
                int64_t tv = taida_pack_get(error_val, WASM_HASH___TYPE);
                if (tv && _looks_like_string(tv)
                    && ((const char *)(intptr_t)tv)[0] != '\0')
                    ty = (const char *)(intptr_t)tv;
            }
            if (taida_pack_has_hash(error_val, WASM_HASH_MESSAGE)) {
                int64_t mv = taida_pack_get(error_val, WASM_HASH_MESSAGE);
                if (mv && _looks_like_string(mv))
                    m = (const char *)(intptr_t)mv;
            }
            if (!m) m = ty;
            iov[n].buf = (int32_t)(intptr_t)"Error["; iov[n].len = 6; n++;
            iov[n].buf = (int32_t)(intptr_t)ty; iov[n].len = wasm_strlen(ty); n++;
            iov[n].buf = (int32_t)(intptr_t)"]: "; iov[n].len = 3; n++;
            iov[n].buf = (int32_t)(intptr_t)m; iov[n].len = wasm_strlen(m); n++;
        } else if (error_val && _looks_like_string(error_val)) {
            const char *ms = (const char *)(intptr_t)error_val;
            iov[n].buf = (int32_t)(intptr_t)ms; iov[n].len = wasm_strlen(ms); n++;
        } else {
            /* Bare scalar throw (`throw(42)`): a local decimal render
               keeps this cold path from pulling the whole polymorphic
               display machinery into every binary (the size gate caught
               exactly that — +16KB on pi.wasm). */
            static char dec[24];
            int64_t v = error_val;
            int neg = v < 0;
            uint64_t u = neg ? (uint64_t)(-(v + 1)) + 1ULL : (uint64_t)v;
            int p = 23;
            dec[p] = '\0';
            do { dec[--p] = (char)('0' + (u % 10)); u /= 10; } while (u && p > 1);
            if (neg && p > 0) dec[--p] = '-';
            iov[n].buf = (int32_t)(intptr_t)(dec + p); iov[n].len = wasm_strlen(dec + p); n++;
        }
        iov[n].buf = (int32_t)(intptr_t)"\n"; iov[n].len = 1; n++;
        /* One iovec per call — the single-iovec shape is the only one
           this runtime has ever exercised against wasmtime. */
        for (int i = 0; i < n; i++) {
            int32_t nwritten;
            __wasi_fd_write(2, &iov[i], 1, &nwritten);
        }
#ifdef TAIDA_WASM_PROFILE_EDGE
        /* The edge profile runs as an exported handler under a host that
           provides no WASI proc_exit; a trap surfaces to the host as a
           catchable WebAssembly.RuntimeError, which is that runtime's
           uncaught-throw contract. */
        __builtin_trap();
#else
        extern void proc_exit(int code)
            __attribute__((import_module("wasi_snapshot_preview1"), import_name("proc_exit")));
        proc_exit(1);
#endif
    }
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

/* Type declaration tags (mirror of the native runtime): one Str per
   declared type and the name it is shown under, also naming the
   declaration, so same-named types of two modules are told apart in `|==`,
   `TypeIs` and the parent registry. A name the program did not declare
   here (a built-in error type) compares by name. Tags are made by module
   code before it runs and are never reclaimed. */
static int64_t *__wasm_type_tag_tags = 0;
static const char **__wasm_type_tag_displays = 0;
static const char **__wasm_type_tag_keys = 0;
static int __wasm_type_tag_count = 0;
static int __wasm_type_tag_cap = 0;

int64_t taida_str_new_copy(int64_t src_raw); /* fwd */

/* The tag of the declaration `key` shown as `display`. */
int64_t taida_type_tag(int64_t display, int64_t key) {
    const char *name = (const char *)(intptr_t)display;
    const char *decl = (const char *)(intptr_t)key;
    if (!name || !decl) return display;
    for (int i = 0; i < __wasm_type_tag_count; i++) {
        if (_wf_strcmp(__wasm_type_tag_displays[i], name) == 0
            && _wf_strcmp(__wasm_type_tag_keys[i], decl) == 0) {
            return __wasm_type_tag_tags[i];
        }
    }
    if (__wasm_type_tag_count >= __wasm_type_tag_cap) {
        int new_cap = __wasm_type_tag_cap == 0 ? 32 : __wasm_type_tag_cap * 2;
        int64_t *tags = (int64_t *)wasm_alloc(sizeof(int64_t) * new_cap);
        const char **displays = (const char **)wasm_alloc(sizeof(const char *) * new_cap);
        const char **keys = (const char **)wasm_alloc(sizeof(const char *) * new_cap);
        if (!tags || !displays || !keys) return display;
        for (int i = 0; i < __wasm_type_tag_count; i++) {
            tags[i] = __wasm_type_tag_tags[i];
            displays[i] = __wasm_type_tag_displays[i];
            keys[i] = __wasm_type_tag_keys[i];
        }
        __wasm_type_tag_tags = tags;
        __wasm_type_tag_displays = displays;
        __wasm_type_tag_keys = keys;
        __wasm_type_tag_cap = new_cap;
    }
    int64_t tag = taida_str_new_copy(display);
    __wasm_type_tag_tags[__wasm_type_tag_count] = tag;
    __wasm_type_tag_displays[__wasm_type_tag_count] = (const char *)(intptr_t)tag;
    __wasm_type_tag_keys[__wasm_type_tag_count] = (const char *)(intptr_t)taida_str_new_copy(key);
    __wasm_type_tag_count++;
    return tag;
}

/* The declaration a type tag names, or 0 for a name the program did not
   declare. */
static const char *_wasm_type_decl_key(int64_t tag) {
    for (int i = 0; i < __wasm_type_tag_count; i++) {
        if (__wasm_type_tag_tags[i] == tag) return __wasm_type_tag_keys[i];
    }
    return 0;
}

/* Whether the type names `a` and `b` stand for the same type: two declared
   tags by their declaration, anything else by name. */
static int _wasm_type_tags_same(int64_t a, int64_t b) {
    if (a == b) return 1;
    const char *ka = _wasm_type_decl_key(a);
    const char *kb = ka ? _wasm_type_decl_key(b) : 0;
    if (ka && kb) return _wf_strcmp(ka, kb) == 0;
    return taida_str_eq(a, b) ? 1 : 0;
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

/* A declared tag finds only its own declaration's parent; a name the
   program did not declare finds the first parent registered under it. */
static int64_t wasm_find_parent_type(int64_t child_str) {
    const char *child_key = _wasm_type_decl_key(child_str);
    for (int i = 0; i < __wasm_type_parent_count; i++) {
        int64_t registered = __wasm_type_parent_child[i];
        int same;
        if (child_key) {
            const char *key = registered == child_str ? child_key : _wasm_type_decl_key(registered);
            same = key && _wf_strcmp(key, child_key) == 0;
        } else {
            same = taida_str_eq(registered, child_str) ? 1 : 0;
        }
        if (same) return __wasm_type_parent_parent[i];
    }
    return 0;
}

__attribute__((weak)) int32_t taida_abi_web_is_host_call_pending_error(int64_t error) {
    (void)error;
    return 0;
}

int64_t taida_error_type_matches(int64_t error_val, int64_t handler_type_str) {
    if (taida_abi_web_is_host_call_pending_error(error_val)) return 0;

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
        if (_wasm_type_tags_same(current, handler_type_str)) return 1;
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
    if (_wasm_type_tags_same(type_str, expected_type_str)) return 1;
    /* Walk inheritance chain */
    int64_t current = type_str;
    for (int i = 0; i < 64; i++) {
        int64_t parent = wasm_find_parent_type(current);
        if (parent == 0) break;
        if (_wasm_type_tags_same(parent, expected_type_str)) return 1;
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
#define WASM_HASH_KIND      0xef9c96d721673243LL  /* FNV-1a("kind") */

static void _wasm_register_builtin_error_field_names(void) {
    static int registered = 0;
    if (registered) return;
    registered = 1;

    taida_register_field_name(WASM_HASH_TYPE, WSTR("type"));
    taida_register_field_name(WASM_HASH_MESSAGE, WSTR("message"));
    taida_register_field_name(WASM_HASH_FIELD, WSTR("field"));
    taida_register_field_name(WASM_HASH_CODE, WSTR("code"));
    taida_register_field_name(WASM_HASH_KIND, WSTR("kind"));
}

int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr) {
    _wasm_register_builtin_error_field_names();
    /* The errors the reference raises from its own runtime are caught as
       `@(type, message, kind, code, __type)`; build them in that form
       (native twin: taida_make_error). */
    static const char *const canonical_types[] = {
        "AssertionError", "AsyncError", "AsyncRaceError", "AsyncStateError",
        "CancelledError", "HostCapabilityError", "HttpError", "ProcessError",
        "RangeError", "ResultError", "TimeoutError", "ValueError", 0,
    };
    const char *t = (const char *)(intptr_t)type_ptr;
    if (t) {
        for (int i = 0; canonical_types[i]; i++) {
            if (_wf_strcmp(t, canonical_types[i]) == 0) {
                return taida_make_error_with_kind_code(type_ptr, msg_ptr, type_ptr, 0);
            }
        }
        if (_wf_strncmp(t, "Addon", 5) == 0) {
            return taida_make_error_with_kind_code(type_ptr, msg_ptr, type_ptr, 0);
        }
    }

    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, WASM_HASH_TYPE);
    taida_pack_set(pack, 0, type_ptr);
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    taida_pack_set_hash(pack, 1, WASM_HASH_MESSAGE);
    taida_pack_set(pack, 1, msg_ptr);
    taida_pack_set_tag(pack, 1, WASM_TAG_STR);
    /* RCB-101 fix: Set __type field so error type matching works.
       Without this, taida_error_type_matches falls through to catch-all
       because it looks for __type, not type. */
    taida_pack_set_hash(pack, 2, WASM_HASH___TYPE);
    taida_pack_set(pack, 2, type_ptr);
    taida_pack_set_tag(pack, 2, WASM_TAG_STR);
    return pack;
}

int64_t taida_make_error_with_kind(int64_t type_ptr, int64_t msg_ptr, int64_t kind_ptr) {
    return taida_make_error_with_kind_code(type_ptr, msg_ptr, kind_ptr, 0);
}

int64_t taida_make_error_with_kind_code(int64_t type_ptr, int64_t msg_ptr, int64_t kind_ptr, int64_t code) {
    _wasm_register_builtin_error_field_names();

    int64_t pack = taida_pack_new(5);
    taida_pack_set_hash(pack, 0, WASM_HASH_TYPE);
    taida_pack_set(pack, 0, type_ptr);
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    taida_pack_set_hash(pack, 1, WASM_HASH_MESSAGE);
    taida_pack_set(pack, 1, msg_ptr);
    taida_pack_set_tag(pack, 1, WASM_TAG_STR);
    taida_pack_set_hash(pack, 2, WASM_HASH_KIND);
    taida_pack_set(pack, 2, kind_ptr);
    taida_pack_set_tag(pack, 2, WASM_TAG_STR);
    taida_pack_set_hash(pack, 3, WASM_HASH_CODE);
    taida_pack_set(pack, 3, code);
    taida_pack_set_hash(pack, 4, WASM_HASH___TYPE);
    taida_pack_set(pack, 4, type_ptr);
    taida_pack_set_tag(pack, 4, WASM_TAG_STR);
    return pack;
}

/* catch-site canonicalization — a caught error
   always exposes the full ErrorInfo field set (type/message/kind/code)
   inside the handler scope. Runtime-raised errors already carry kind/code
   so handlers can read `err.kind`. A
   user throw like `Error(type <= "X", message <= "y").throw()` builds a
   bare @(type, message) pack, so `.kind` used to crash only on that path.
   Mirrors the interpreter's canonicalize_caught_error and the native
   runtime's catch-site normalization. */
int64_t taida_canonicalize_caught_error(int64_t error_val) {
    if (!_looks_like_pack(error_val)) return error_val;
    if (taida_pack_has_hash(error_val, WASM_HASH_KIND)
        && taida_pack_has_hash(error_val, WASM_HASH_CODE)
        && taida_pack_has_hash(error_val, WASM_HASH___TYPE)) {
        return error_val; /* already canonical — display / re-throw unchanged */
    }
    int64_t type_ptr = 0;
    if (taida_pack_has_hash(error_val, WASM_HASH_TYPE)) {
        type_ptr = taida_pack_get(error_val, WASM_HASH_TYPE);
    } else if (taida_pack_has_hash(error_val, WASM_HASH___TYPE)) {
        type_ptr = taida_pack_get(error_val, WASM_HASH___TYPE);
    }
    if (type_ptr == 0 || !_wasm_is_string_ptr(type_ptr)) type_ptr = WSTR("Error");
    /* The thrown pack's OWN fields must survive canonicalization: a user
       subtype like L2Error(l1, l2) keeps its declared slots in the handler
       scope. Canonicalization only ADDS the missing ErrorInfo slots — it
       never rebuilds the pack. Mirrors the interpreter's BuchiPack arm. */
    int64_t *src = (int64_t *)(intptr_t)error_val;
    int64_t src_fc = src[0];
    int has_kind_slot = taida_pack_has_hash(error_val, WASM_HASH_KIND);
    int has_code_slot = taida_pack_has_hash(error_val, WASM_HASH_CODE);
    int has_type_slot = taida_pack_has_hash(error_val, WASM_HASH___TYPE);
    int64_t extra = (int64_t)(!has_kind_slot) + (int64_t)(!has_code_slot)
                  + (int64_t)(!has_type_slot);
    int64_t canon = taida_pack_new(src_fc + extra);
    for (int64_t i = 0; i < src_fc; i++) {
        /* Copy each slot verbatim — hash, tag, and value. The bump
           allocator never frees, so no retain is needed on this profile. */
        taida_pack_set_hash(canon, i, src[1 + i * 3]);
        taida_pack_set_tag(canon, i, src[1 + i * 3 + 1]);
        taida_pack_set(canon, i, src[1 + i * 3 + 2]);
    }
    int64_t next = src_fc;
    if (!has_kind_slot) {
        /* Stamp per-slot value tags : the full-form pack
           renderer renders tag-0 slots through its explicit INT branch, so
           the Str payloads must be tagged or they surface as raw heap
           addresses. */
        taida_pack_set_hash(canon, next, WASM_HASH_KIND);
        taida_pack_set(canon, next, type_ptr);
        taida_pack_set_tag(canon, next, WASM_TAG_STR);
        next++;
    }
    if (!has_code_slot) {
        taida_pack_set_hash(canon, next, WASM_HASH_CODE);
        taida_pack_set(canon, next, 0);
        taida_pack_set_tag(canon, next, WASM_TAG_INT);
        next++;
    }
    if (!has_type_slot) {
        taida_pack_set_hash(canon, next, WASM_HASH___TYPE);
        taida_pack_set(canon, next, type_ptr);
        taida_pack_set_tag(canon, next, WASM_TAG_STR);
        next++;
    }
    return canon;
}

/* ── W-5: Lax[T] runtime ────────────────────────────────── */
/* Lax is a BuchiPack @(has_value: Bool, __value: T, __default: T, __type: Str)
   Layout: 4-field pack using same hash constants as native. */

/* WASM_HASH_HAS_VALUE, __VALUE, __DEFAULT, __TYPE defined in W-5f monadic type hash section */

/* C21B-seed-07: Register Lax's four field names so
   `_wasm_pack_to_string_full` can surface them in the interpreter-parity
   stdout form `@(has_value <= …, __value <= …, __default <= …, __type <=
   "Lax")`. Without this, the lookup returns NULL and the field is skipped
   entirely — the symptom observed on wasm-wasi was `@()` for any Lax
   produced by `Int[x]()` / `Float[x]()` / `Bool[x]()` / `Str[x]()`. */
static int _wasm_lax_names_registered = 0;
static void _wasm_register_lax_field_names(void) {
    if (_wasm_lax_names_registered) return;
    _wasm_lax_names_registered = 1;
    taida_register_field_name(WASM_HASH_HAS_VALUE, WSTR("has_value"));
    taida_register_field_name(WASM_HASH___VALUE,   WSTR("__value"));
    taida_register_field_name(WASM_HASH___DEFAULT, WSTR("__default"));
    taida_register_field_name(WASM_HASH___TYPE,    WSTR("__type"));
}

int64_t taida_lax_new(int64_t value, int64_t default_value) {
    _wasm_register_lax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1);  /* has_value = true */
    taida_pack_set_tag(pack, 0, 2); /* BOOL tag */
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    taida_pack_set_hash(pack, 2, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, WSTR("Lax"));
    /* Tag the __type slot as STR so `_wasm_pack_to_string_full` quotes it
       correctly (matches interpreter's `__type <= "Lax"`). */
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

int64_t taida_lax_empty(int64_t default_value) {
    _wasm_register_lax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);  /* has_value = false */
    taida_pack_set_tag(pack, 0, 2); /* BOOL tag */
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, default_value);
    taida_pack_set_hash(pack, 2, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, WSTR("Lax"));
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

/* A lookup that found nothing takes the default of the element's static
   type the lowering built: `__default` always, `__value` too when the Lax
   is empty (native twin: taida_lax_fill_default). */
int64_t taida_lax_fill_default(int64_t lax, int64_t def, int64_t tag) {
    if (!_looks_like_pack(lax)) return lax;
    int64_t *p = (int64_t *)(intptr_t)lax;
    if (p[0] < 3) return lax;
    if (p[1 + 0 * 3 + 2] == 0) {
        taida_pack_set(lax, 1, def);
        taida_pack_set_tag(lax, 1, tag);
    }
    taida_pack_set(lax, 2, def);
    taida_pack_set_tag(lax, 2, tag);
    return lax;
}

int64_t taida_lax_empty_error(int64_t default_value, int64_t error) {
    _wasm_register_lax_field_names();
    int64_t pack = taida_pack_new(5);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0);
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, default_value);
    taida_pack_set_hash(pack, 2, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 2, default_value);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, WSTR("Lax"));
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    taida_pack_set_hash(pack, 4, WASM_HASH___ERROR);
    taida_pack_set(pack, 4, error);
    taida_pack_set_tag(pack, 4, WASM_TAG_PACK);
    return pack;
}

int64_t taida_lax_has_value(int64_t lax_ptr) {
    return taida_pack_get_idx(lax_ptr, 0);  /* has_value field */
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

static int64_t _wasm_error_info_pack(int64_t type_name, int64_t message, int64_t kind, int64_t code) {
    _wasm_register_builtin_error_field_names();
    _wasm_register_lax_field_names();
    int64_t pack = taida_pack_new(5);
    taida_pack_set_hash(pack, 0, WASM_HASH_TYPE);
    taida_pack_set(pack, 0, type_name);
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    taida_pack_set_hash(pack, 1, WASM_HASH_MESSAGE);
    taida_pack_set(pack, 1, message);
    taida_pack_set_tag(pack, 1, WASM_TAG_STR);
    taida_pack_set_hash(pack, 2, WASM_HASH_KIND);
    taida_pack_set(pack, 2, kind);
    taida_pack_set_tag(pack, 2, WASM_TAG_STR);
    taida_pack_set_hash(pack, 3, WASM_HASH_CODE);
    taida_pack_set(pack, 3, code);
    taida_pack_set_tag(pack, 3, WASM_TAG_INT);
    taida_pack_set_hash(pack, 4, WASM_HASH___TYPE);
    taida_pack_set(pack, 4, WSTR("ErrorInfo"));
    taida_pack_set_tag(pack, 4, WASM_TAG_STR);
    return pack;
}

static int64_t _wasm_error_info_default(void) {
    return _wasm_error_info_pack(
        WSTR(""),
        WSTR(""),
        WSTR(""),
        0
    );
}

static int _wasm_is_gorillax_like_pack(int64_t ptr) {
    if (!_looks_like_pack(ptr)) return 0;
    int64_t *pack = (int64_t *)(intptr_t)ptr;
    if (pack[0] < 4) return 0;
    return pack[1] == WASM_HASH_HAS_VALUE && pack[1 + 2 * 3] == WASM_HASH___ERROR;
}

static int _wasm_is_error_info_source_pack(int64_t ptr) {
    return _looks_like_pack(ptr)
        && taida_pack_has_hash(ptr, WASM_HASH___TYPE)
        && taida_pack_has_hash(ptr, WASM_HASH_TYPE)
        && taida_pack_has_hash(ptr, WASM_HASH_MESSAGE);
}

static int64_t _wasm_error_info_from_error(int64_t error) {
    int64_t type_name = WSTR("Error");
    int64_t message = WSTR("");
    int64_t kind = 0;
    int64_t code = 0;
    if (_looks_like_pack(error)) {
        if (taida_pack_has_hash(error, WASM_HASH_TYPE)) {
            type_name = taida_pack_get(error, WASM_HASH_TYPE);
        } else if (taida_pack_has_hash(error, WASM_HASH___TYPE)) {
            type_name = taida_pack_get(error, WASM_HASH___TYPE);
        }
        if (taida_pack_has_hash(error, WASM_HASH_MESSAGE)) {
            message = taida_pack_get(error, WASM_HASH_MESSAGE);
        }
        if (taida_pack_has_hash(error, WASM_HASH_KIND)) {
            kind = taida_pack_get(error, WASM_HASH_KIND);
        }
        if (taida_pack_has_hash(error, WASM_HASH_CODE)) {
            code = taida_pack_get(error, WASM_HASH_CODE);
        }
    }
    if (kind == 0) kind = type_name;
    return _wasm_error_info_pack(type_name, message, kind, code);
}

int64_t taida_error_info(int64_t source) {
    int64_t def = _wasm_error_info_default();
    if (_wasm_is_gorillax_like_pack(source)) {
        if (taida_pack_get_idx(source, 0)) return taida_lax_empty(def);
        return taida_lax_new(_wasm_error_info_from_error(taida_pack_get_idx(source, 2)), def);
    }
    if (_wasm_is_lax(source)) {
        if (taida_pack_get_idx(source, 0)) return taida_lax_empty(def);
        if (!taida_pack_has_hash(source, WASM_HASH___ERROR)) return taida_lax_empty(def);
        return taida_lax_new(_wasm_error_info_from_error(taida_pack_get(source, WASM_HASH___ERROR)), def);
    }
    if (source == 0) return taida_lax_empty(def);
    if (!_wasm_is_error_info_source_pack(source)) return taida_lax_empty(def);
    return taida_lax_new(_wasm_error_info_from_error(source), def);
}

/* ── W-5: generic_unmold — now Lax-aware ── */
/* Override the simplified version from W-1. When the value is a Lax pack
   (detected by field count == 4 and has_value field), extract the value;
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
    if ((p[0] != 4 && p[0] != 5) || p[1] != WASM_HASH_HAS_VALUE) return 0;
    /* Reject Gorillax / RelaxedGorillax (slot-2 hash == __error). */
    return p[1 + 2 * 3] == WASM_HASH___DEFAULT ? 1 : 0;
}

/* ── W-5: Gorillax (Result container) ── */
/* Gorillax: @(has_value: Bool, __value: T, __error: Error, __type: "Gorillax")
   Using pack fields at fixed indices.

   C24-A (2026-04-23): unified Gorillax first-field name from `isOk` to
   `has_value` so `Str[Gorillax[v]()]()` on wasm matches the interpreter /
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
   three fields (`has_value`, `__value`, `__type`) are already registered
   by `_wasm_register_lax_field_names`, but we re-register them here as
   a defence-in-depth so the Gorillax path is self-sufficient. */
static void _wasm_register_gorillax_field_names(void) {
    taida_register_field_name(WASM_HASH_HAS_VALUE, WSTR("has_value"));
    taida_register_field_name(WASM_HASH___VALUE,   WSTR("__value"));
    taida_register_field_name(WASM_HASH___ERROR,   WSTR("__error"));
    taida_register_field_name(WASM_HASH___TYPE,    WSTR("__type"));
}

int64_t taida_gorillax_new(int64_t value) {
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 1); /* has_value = true */
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
    taida_pack_set(pack, 3, WSTR("Gorillax"));
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

int64_t taida_gorillax_err(int64_t error) {
    _wasm_register_gorillax_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_HAS_VALUE);
    taida_pack_set(pack, 0, 0); /* has_value = false */
    taida_pack_set_tag(pack, 0, WASM_TAG_BOOL);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, 0);
    taida_pack_set_hash(pack, 2, WASM_HASH___ERROR);
    taida_pack_set(pack, 2, error);
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, WSTR("Gorillax"));
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
    taida_pack_set(pack, 3, WSTR("RelaxedGorillax"));
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
    taida_pack_set(pack, 3, WSTR("RelaxedGorillax"));
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
    taida_pack_set(pack, 3, WSTR("RelaxedGorillax"));
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
    taida_register_field_name(WASM_HASH___VALUE,     WSTR("__value"));
    taida_register_field_name(WASM_HASH___PREDICATE, WSTR("__predicate"));
    taida_register_field_name(WASM_HASH_THROW,       WSTR("throw"));
    taida_register_field_name(WASM_HASH___TYPE,      WSTR("__type"));
}

int64_t taida_result_create(int64_t value, int64_t throw_val, int64_t predicate) {
    _wasm_register_result_field_names();
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH___VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, WASM_HASH___PREDICATE);
    taida_pack_set(pack, 1, predicate);
    taida_pack_set_tag(pack, 1, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 2, WASM_HASH_THROW);
    taida_pack_set(pack, 2, throw_val);
    taida_pack_set_tag(pack, 2, WASM_TAG_PACK);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, WSTR("Result"));
    taida_pack_set_tag(pack, 3, WASM_TAG_STR);
    return pack;
}

static int64_t _wasm_result_get_value(int64_t result) {
    return taida_pack_get(result, WASM_HASH___VALUE);
}

static int64_t _wasm_result_get_predicate(int64_t result) {
    return taida_pack_get(result, WASM_HASH___PREDICATE);
}

static int64_t _wasm_result_get_throw(int64_t result) {
    return taida_pack_get(result, WASM_HASH_THROW);
}

/* The Error a Result given no error has as its `throw` (native twin:
   taida_empty_error_new): `@(type <= "", message <= "")` of type Error. */
static int64_t _wasm_empty_error_new(void) {
    _wasm_register_builtin_error_field_names();
    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, WASM_HASH_TYPE);
    taida_pack_set(pack, 0, WSTR(""));
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    taida_pack_set_hash(pack, 1, WASM_HASH_MESSAGE);
    taida_pack_set(pack, 1, WSTR(""));
    taida_pack_set_tag(pack, 1, WASM_TAG_STR);
    taida_pack_set_hash(pack, 2, WASM_HASH___TYPE);
    taida_pack_set(pack, 2, WSTR("Error"));
    taida_pack_set_tag(pack, 2, WASM_TAG_STR);
    return pack;
}

/* `result.throw`: the error the Result was given, or the empty Error. */
int64_t taida_result_throw_field(int64_t result) {
    int64_t thrown = _wasm_result_get_throw(result);
    return thrown != 0 ? thrown : _wasm_empty_error_new();
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
    taida_register_field_name(WASM_HASH_STREAM_STATUS, WSTR("__stream_status"));
    taida_register_field_name(WASM_HASH_STREAM_COUNT,  WSTR("__stream_count"));
}

int64_t taida_stream_new(int64_t inner_value) {
    (void)inner_value; /* Minimal impl: retain only the item count (=1). */
    _wasm_register_stream_field_names();
    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, WASM_HASH_STREAM_STATUS);
    taida_pack_set(pack, 0, WSTR("completed"));
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    taida_pack_set_hash(pack, 1, WASM_HASH_STREAM_COUNT);
    taida_pack_set(pack, 1, 1);
    taida_pack_set_tag(pack, 1, WASM_TAG_INT);
    taida_pack_set_hash(pack, 2, WASM_HASH___TYPE);
    taida_pack_set(pack, 2, WSTR("Stream"));
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
    if ((uint64_t)obj + 24 > (uint64_t)mem_size) return 0; /* no wrap-around */
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
    int64_t throw_val = _wasm_result_get_throw(result);
    int64_t pred = _wasm_result_get_predicate(result);
    int64_t value = _wasm_result_get_value(result);

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
    int64_t throw_val = _wasm_result_get_throw(result);
    /* Pass the throw payload `P` directly to the mapper so the runtime
       matches the type-checker contract
       `mapError(fn: P -> Q) -> Result[T, Q]`. */
    int64_t mapped = taida_invoke_callback1(fn_ptr, throw_val);
    /* Snapshot the callback return tag immediately. */
    int64_t mapped_tag = taida_get_return_tag();
    /* Direct-store applies to Error-derived BuchiPacks — those that
       carry WASM_HASH___TYPE = "__type", which the user-defined
       `Error => Foo = @(...)` form always emits. The predicate is
       deliberately the same shape as the Native and Interpreter
       paths so all four backends agree. */
    if (taida_is_buchi_pack(mapped)
        && taida_pack_has_hash(mapped, WASM_HASH___TYPE)) {
        return taida_result_create(0, mapped, 0);
    }
    /* Anything else (anonymous pack, primitive) is wrapped in a generic
       ResultError. Untagged scalars must be rendered via the tag the
       callback advertised; otherwise primitives surface as their raw
       64-bit representation (Bool=1, Float=IEEE-754 bit pattern) and
       diverge from the Interpreter / JS output. */
    int64_t display;
    if (mapped_tag == WASM_TAG_BOOL) {
        display = taida_str_from_bool(mapped);
    } else if (mapped_tag == WASM_TAG_FLOAT) {
        display = taida_float_to_str(mapped);
    } else if (mapped_tag == WASM_TAG_INT) {
        display = taida_int_to_str(mapped);
    } else if (mapped_tag == WASM_TAG_STR) {
        display = mapped;
    } else {
        display = taida_polymorphic_to_string(mapped);
    }
    int64_t new_error = taida_make_error(
        WSTR("ResultError"), display);
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
        if (taida_pack_has_hash(lax_ptr, WASM_HASH___ERROR)) {
            return taida_lax_empty_error(def, taida_pack_get(lax_ptr, WASM_HASH___ERROR));
        }
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
        if (taida_pack_has_hash(lax_ptr, WASM_HASH___ERROR)) {
            return taida_lax_empty_error(def, taida_pack_get(lax_ptr, WASM_HASH___ERROR));
        }
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
    if (!_wasm_result_is_error_check(result)) return _wasm_result_get_value(result);
    return def;
}

/// Result.map(fn)
int64_t taida_result_map(int64_t result, int64_t fn_ptr) {
    if (_wasm_result_is_error_check(result)) return result;
    int64_t value = _wasm_result_get_value(result);
    int64_t new_val = taida_invoke_callback1(fn_ptr, value);
    return taida_result_create(new_val, 0, 0);
}

/// Result.flatMap(fn)
int64_t taida_result_flat_map(int64_t result, int64_t fn_ptr) {
    if (_wasm_result_is_error_check(result)) return result;
    int64_t value = _wasm_result_get_value(result);
    return taida_invoke_callback1(fn_ptr, value);
}

/// Result.getOrThrow()
int64_t taida_result_get_or_throw(int64_t result) {
    if (!_wasm_result_is_error_check(result)) {
        return _wasm_result_get_value(result);
    }
    int64_t throw_val = _wasm_result_get_throw(result);
    if (taida_can_throw_payload(throw_val)) {
        return taida_throw(throw_val);
    }
    return taida_throw(_wasm_result_predicate_error(result));
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
    int64_t info = _wasm_error_info_from_error(taida_pack_get_idx(ptr, 2));
    int64_t kind = WSTR("RelaxedGorillaEscaped");
    int64_t code = 0;
    if (_looks_like_pack(info)) {
        if (taida_pack_has_hash(info, WASM_HASH_KIND)) kind = taida_pack_get(info, WASM_HASH_KIND);
        if (taida_pack_has_hash(info, WASM_HASH_CODE)) code = taida_pack_get(info, WASM_HASH_CODE);
    }
    int64_t error = taida_make_error_with_kind_code(
        WSTR("RelaxedGorillaEscaped"),
        WSTR("Relaxed gorilla escaped"),
        kind,
        code);
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
        if (taida_result_is_ok(obj)) return taida_pack_get(obj, WASM_HASH___VALUE);
        int64_t throw_val = taida_pack_get(obj, WASM_HASH_THROW);
        if (taida_can_throw_payload(throw_val)) return taida_throw(throw_val);
        return taida_throw(_wasm_result_predicate_error(obj));
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
    /* A pending throw ends the runtime loop that is calling back (Map,
       Filter, ...): no further user code runs before the ceiling. */
    if (__wasm_error_thrown) return 0;
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

static int64_t _wasm_invoke_callback0(int64_t fn_ptr) {
    if (__wasm_error_thrown) return 0;
    if (taida_is_closure_value(fn_ptr)) {
        int64_t *closure = (int64_t *)(intptr_t)fn_ptr;
        typedef int64_t (*closure_fn0_t)(int64_t);
        closure_fn0_t func = (closure_fn0_t)(intptr_t)closure[1];
        return func(closure[2]);
    }
    typedef int64_t (*fn_t)(void);
    fn_t func = (fn_t)(intptr_t)fn_ptr;
    return func();
}

int64_t taida_async_task_new(int64_t fn_ptr) {
    taida_register_field_name(WASM_HASH_TODO_TASK, WSTR("task"));
    taida_register_field_name(WASM_HASH___TYPE, WSTR("__type"));
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, WASM_HASH_TODO_TASK);
    taida_pack_set(pack, 0, fn_ptr);
    taida_pack_set_hash(pack, 1, WASM_HASH___TYPE);
    taida_pack_set(pack, 1, WSTR("AsyncTask"));
    taida_pack_set_tag(pack, 1, WASM_TAG_STR);
    return pack;
}

static int64_t _wasm_async_task_callable(int64_t task) {
    if (_looks_like_pack(task)
        && taida_pack_has_hash(task, WASM_HASH_TODO_TASK)
        && taida_pack_has_hash(task, WASM_HASH___TYPE)) {
        int64_t type_name = taida_pack_get(task, WASM_HASH___TYPE);
        if (taida_str_eq(type_name, WSTR("AsyncTask"))) {
            return taida_pack_get(task, WASM_HASH_TODO_TASK);
        }
    }
    return task;
}

/* RCB-101: Check error type and re-throw if it does not match.
   If the type matches, returns the error_val unchanged.
   If it does not match, calls taida_throw(error_val) which sets the error flag (never returns normally). */
int64_t taida_error_type_check_or_rethrow(int64_t error_val, int64_t handler_type_str) {
    if (taida_error_type_matches(error_val, handler_type_str)) {
        /* the handler variable always sees the full ErrorInfo
           field set (type/message/kind/code), regardless of how the error
           was thrown. */
        return taida_canonicalize_caught_error(error_val);
    }
    /* Re-throw: sets __wasm_error_thrown flag */
    taida_throw(error_val);
    return 0; /* caller should check __wasm_error_thrown */
}

/* ── W-5: Molten/Stub/Todo stubs ── */

int64_t taida_molten_new(void) {
    int64_t pack = taida_pack_new(1);
    taida_pack_set_hash(pack, 0, WASM_HASH___TYPE);
    taida_pack_set(pack, 0, WSTR("Molten"));
    return pack;
}

/* F56: opaque secret carriers (Moltenized / Secret). fc=2 pack with an extra
   __value slot. Checker sink matrix rejects display / JSON / concat / unmold;
   this is the value-construction half of the contract.

   INVARIANT: the __type slot MUST store the shared `__wasm_moltenized_str` /
   `__wasm_secret_str` statics (defined in 01_core.inc.c). `_wasm_carrier_kind`
   classifies a carrier by *pointer identity* against those exact addresses (a
   content compare faulted out of bounds on magic-tagged AsyncTask/Par packs).
   All `.inc.c` fragments compile as one translation unit, so the addresses
   match. Any future carrier producer must use these same statics — a distinct
   "Moltenized" literal would fail closed in display (renders the pack) but
   fail OPEN in the detector, so do not inline the string here. (Native's
   `taida_is_moltenized` additionally falls back to a bounded content compare,
   so the native/wasm guards are intentionally asymmetric.) */
int64_t taida_moltenize_new(int64_t value) {
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, WASM_HASH___TYPE);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)__wasm_moltenized_str);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    return pack;
}

int64_t taida_secret_new(int64_t value) {
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, WASM_HASH___TYPE);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)__wasm_secret_str);
    taida_pack_set_hash(pack, 1, WASM_HASH___VALUE);
    taida_pack_set(pack, 1, value);
    return pack;
}

int64_t taida_redact(int64_t carrier) {
    (void)carrier;
    return taida_str_new_copy(WSTR("***"));
}

int64_t taida_stub_new(int64_t message) {
    (void)message;
    return taida_molten_new();
}

/* classify a TODO field value so the
   pack renderers' explicit INT branch (tag 0) never swallows it. Str
   payloads used to surface as raw heap addresses (`id <= 4649840`).
   Structural kinds get their heap tag so the renderer takes the recursive
   full-form path; scalars stay INT (Bool is indistinguishable from Int at
   the value level — the legacy ftype-4 registry hint keeps Bool fields
   rendering as true/false). Float payloads have no unboxed runtime
   signature and remain on the per-entry value-tag track. */
static int _wasm_todo_field_tag(int64_t v) {
    if (_wasm_is_string_ptr(v)) return WASM_TAG_STR;
    if (taida_is_buchi_pack(v)) return WASM_TAG_PACK;
    if (taida_is_list(v)) return WASM_TAG_LIST;
    if (taida_is_hashmap(v)) return WASM_TAG_HMAP;
    if (taida_is_set(v)) return WASM_TAG_SET;
    return WASM_TAG_INT;
}

int64_t taida_todo_new(int64_t id, int64_t task, int64_t sol, int64_t unm) {
    /* BE-WASM-1: proper TODO pack matching native_runtime.c layout.
       Fields: id(0), task(1), sol(2), unm(3), __value(4), __default(5), __type(6) */
    int64_t pack = taida_pack_new(7);
    int id_tag = _wasm_todo_field_tag(id);
    int task_tag = _wasm_todo_field_tag(task);
    int sol_tag = _wasm_todo_field_tag(sol);
    int unm_tag = _wasm_todo_field_tag(unm);
    taida_pack_set_hash(pack, 0, WASM_HASH_TODO_ID);
    taida_pack_set(pack, 0, id);
    taida_pack_set_tag(pack, 0, id_tag);
    taida_pack_set_hash(pack, 1, WASM_HASH_TODO_TASK);
    taida_pack_set(pack, 1, task);
    taida_pack_set_tag(pack, 1, task_tag);
    taida_pack_set_hash(pack, 2, WASM_HASH_TODO_SOL);
    taida_pack_set(pack, 2, sol);
    taida_pack_set_tag(pack, 2, sol_tag);
    taida_pack_set_hash(pack, 3, WASM_HASH_TODO_UNM);
    taida_pack_set(pack, 3, unm);
    taida_pack_set_tag(pack, 3, unm_tag);
    /* __value / __default mirror the sol / unm channels. */
    taida_pack_set_hash(pack, 4, WASM_HASH___VALUE);
    taida_pack_set(pack, 4, sol);
    taida_pack_set_tag(pack, 4, sol_tag);
    taida_pack_set_hash(pack, 5, WASM_HASH___DEFAULT);
    taida_pack_set(pack, 5, unm);
    taida_pack_set_tag(pack, 5, unm_tag);
    taida_pack_set_hash(pack, 6, WASM_HASH___TYPE);
    taida_pack_set(pack, 6, WSTR("TODO"));
    taida_pack_set_tag(pack, 6, WASM_TAG_STR);
    return pack;
}

/* BE-WASM-2: Gorilla literal — immediate crash (matching native exit(1)).
   In WASM, __builtin_trap() produces an unreachable instruction that
   terminates the module, which is the WASM equivalent of exit(). */
void taida_gorilla(void) {
    /* `><` ends the program with exit code 1 and no output, like the
       interpreter / native. The edge handler host provides no WASI
       proc_exit; a trap is that runtime's termination (see taida_exit). */
#ifdef TAIDA_WASM_PROFILE_EDGE
    __builtin_trap();
#else
    extern void proc_exit(int code)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("proc_exit")));
    proc_exit(1);
    __builtin_trap(); /* unreachable */
#endif
}

/* F62B-030: exit(code) prelude builtin. WASI profiles terminate through
   proc_exit so the code reaches the host; the edge handler host provides
   no WASI proc_exit, so a trap surfaces as that runtime's uncaught-throw
   contract (the code is not observable there). */
int64_t taida_exit(int64_t code) {
#ifdef TAIDA_WASM_PROFILE_EDGE
    (void)code;
    __builtin_trap();
#else
    extern void proc_exit(int code)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("proc_exit")));
    proc_exit((int)code);
    __builtin_trap(); /* unreachable */
#endif
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
    return _lax_tag_vd(taida_lax_new(taida_int_to_str(v), WSTR("")), WASM_TAG_STR);
}

/* `Str[Float]()` stores Rust's `f.to_string()`: the shortest round-trip
   digits with no `.0` for integral values (`3`, `-5`, `1e23` ->
   `100000000000000000000000`), unlike the Float display `3.0`. */
static int64_t _taida_float_to_str_mold(int64_t val) {
    char tmp[TAIDA_WASM_FLOAT_TEXT_MAX];
    int len = _tf_to_string(_l2d(val), tmp);
    char *buf = _wasm_str_alloc((unsigned int)(len + 1));
    if (!buf) return 0;
    for (int i = 0; i < len; i++) buf[i] = tmp[i];
    buf[len] = '\0';
    return (int64_t)(intptr_t)buf;
}

int64_t taida_str_mold_float(int64_t v) {
    return _lax_tag_vd(taida_lax_new(_taida_float_to_str_mold(v), WSTR("")), WASM_TAG_STR);
}

int64_t taida_str_mold_bool(int64_t v) {
    return _lax_tag_vd(taida_lax_new(taida_str_from_bool(v), WSTR("")), WASM_TAG_STR);
}

int64_t taida_str_mold_str(int64_t v) {
    return _lax_tag_vd(taida_lax_new(v, WSTR("")), WASM_TAG_STR);
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
    return _lax_tag_vd(taida_lax_new(str, WSTR("")), WASM_TAG_STR);
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
    /* Rust `str::parse::<f64>` grammar and correctly rounded value (a
       digit-by-digit accumulation was off by an ulp for inputs as plain
       as "0.3"). */
    const char *s = (const char *)(intptr_t)v;
    if (!s) return _float_lax_empty();
    double d;
    if (!_tf_parse(s, wasm_strlen(s), &d)) return _float_lax_empty();
    return _float_lax_new(_d2l(d));
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
    /* A pack payload is thrown as-is, whatever its field order: an Error
       subtype (`Error => E1 = @(a: Int)`) stores its own fields before
       `type` / `message`, and must still reach a `|== e: E1` ceiling
       with its type and fields. Str payloads are thrown as-is too, like
       the interpreter / native; other values fall back to the caller's
       generic error. */
    return taida_is_buchi_pack(val) || _wasm_is_string_ptr(val);
}

/* =========================================================================
 * WC-1b: String mold functions (prelude — all profiles)
 * ========================================================================= */

/// Allocate a NUL-terminated string buffer of `len` bytes (+ 1 for NUL).
/// Uses bump allocator. No hidden header needed (no RC in WASM).
int64_t taida_str_alloc(int64_t len_raw) {
    int len = (int)len_raw;
    if (len < 0) len = 0;
    char *buf = _wasm_str_alloc((unsigned int)(len + 1));
    if (!buf) return 0;
    buf[len] = '\0';
    return (int64_t)buf;
}

/// Copy a NUL-terminated string into a newly allocated buffer.
int64_t taida_str_new_copy(int64_t src_raw) {
    const char *src = (const char *)src_raw;
    if (!src) {
        char *r = _wasm_str_alloc(1);
        r[0] = '\0';
        return (int64_t)r;
    }
    int len = _wf_strlen(src);
    char *r = _wasm_str_alloc((unsigned int)(len + 1));
    _wf_memcpy(r, src, len);
    r[len] = '\0';
    return (int64_t)r;
}

/// Release a string. No-op in WASM (bump allocator, no free).
void taida_str_release(int64_t s) {
    (void)s;
}

/* ── taida-lang/crypto: sha256 (pure, all WASM profiles) ───────────── */
#define TAIDA_WASM_SHA256_MAX_INPUT_BYTES (256LL * 1024LL * 1024LL)
#define TAIDA_WASM_BYTES_MAGIC 0x5441494442595400LL  /* "TAIDBYT\0" */

/* F54B-016 (G4): structural Bytes identity + content access for Set /
   list.unique. A Bytes value is laid out as [magic, len, byte0, byte1, ...]
   with one int64_t per byte (the same layout taida_wasm_sha256_bytes_input
   parses). The Bytes constructor mold is implemented on wasm-full only;
   wasm-min / wasm-wasi never materialise a Bytes value, so these helpers
   matter only where Bytes can actually exist. Mirrors the native
   taida_value_kind / taida_value_struct_eq Bytes path so all four backends
   agree on structural dedup -- the interpreter ValueKey treats Bytes as a
   key-eligible, content-compared value. Forward-declared in 01_core. */
static int _looks_like_bytes(int64_t val) {
    if (!_wasm_is_valid_ptr(val, 16)) return 0;  /* need magic + len slots */
    unsigned int addr = (unsigned int)(uint64_t)val;
    if ((addr & 7u) != 0) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if ((p[0] & 0xFFFFFFFFFFFFFF00LL) != TAIDA_WASM_BYTES_MAGIC) return 0;
    int64_t len = p[1];
    if (len < 0) return 0;
    unsigned int mem_size = (unsigned int)__builtin_wasm_memory_size(0) * 65536u;
    if ((uint64_t)addr + (uint64_t)(2 + len) * 8ULL > (uint64_t)mem_size) return 0;
    return 1;
}
static int64_t _wasm_bytes_len(int64_t val) { return ((int64_t *)(intptr_t)val)[1]; }
static unsigned char _wasm_bytes_at(int64_t val, int64_t i) {
    return (unsigned char)(((int64_t *)(intptr_t)val)[2 + i] & 0xFF);
}

typedef struct {
    uint32_t state[8];
    uint64_t total_len;
    unsigned char block[64];
    int block_len;
} taida_sha256_ctx;

static const uint32_t TAIDA_WASM_SHA256_K[64] = {
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

static uint32_t taida_wasm_sha256_rotr(uint32_t x, uint32_t n) {
    return (x >> n) | (x << (32 - n));
}

static void taida_wasm_sha256_trap(void) {
    __builtin_trap();
}

static int taida_wasm_sha256_ptr_readable(int64_t ptr, uint64_t bytes) {
    if (ptr <= 0 || ptr > 0xFFFFFFFFLL) return 0;
    unsigned int addr = (unsigned int)(uint64_t)ptr;
    uint64_t mem_size = (uint64_t)__builtin_wasm_memory_size(0) * 65536ULL;
    return (uint64_t)addr + bytes <= mem_size;
}

static int taida_wasm_sha256_bounded_strlen(const char *s, int64_t max_len, int64_t *out_len) {
    if (!s) {
        if (out_len) *out_len = 0;
        return 1;
    }
    int64_t ptr = (int64_t)(intptr_t)s;
    if (!taida_wasm_sha256_ptr_readable(ptr, 1)) return 0;
    uint64_t addr = (uint64_t)(unsigned int)(uint64_t)ptr;
    uint64_t mem_size = (uint64_t)__builtin_wasm_memory_size(0) * 65536ULL;
    uint64_t available = mem_size > addr ? mem_size - addr : 0;
    uint64_t limit = (uint64_t)max_len < available ? (uint64_t)max_len : available;
    for (uint64_t i = 0; i <= limit; i++) {
        if (i == available) break;
        if (((const unsigned char *)s)[i] == 0) {
            if (out_len) *out_len = (int64_t)i;
            return 1;
        }
    }
    return 0;
}

static int taida_wasm_sha256_list_input(int64_t value, int64_t **out_list, int64_t *out_len) {
    if (!taida_wasm_sha256_ptr_readable(value, 32)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)value;
    if ((addr & 7u) != 0) return 0;
    int64_t *list = (int64_t *)(intptr_t)value;
    if (list[3] != WASM_LIST_MAGIC && list[3] != WASM_SET_MAGIC) return 0;
    int64_t cap = list[0];
    int64_t len = list[1];
    if (cap < 0 || cap > TAIDA_WASM_SHA256_MAX_INPUT_BYTES) return -1;
    if (len < 0 || len > cap || len > TAIDA_WASM_SHA256_MAX_INPUT_BYTES) return -1;
    uint64_t total_bytes = (uint64_t)(WASM_LIST_ELEMS + cap + 1) * 8ULL;
    if (!taida_wasm_sha256_ptr_readable(value, total_bytes)) return -1;
    if (list[WASM_LIST_ELEMS + cap] != list[3]) return -1;
    if (out_list) *out_list = list;
    if (out_len) *out_len = len;
    return 1;
}

static int taida_wasm_sha256_bytes_input(int64_t value, int64_t **out_bytes, int64_t *out_len) {
    if (!taida_wasm_sha256_ptr_readable(value, 16)) return 0;
    unsigned int addr = (unsigned int)(uint64_t)value;
    if ((addr & 7u) != 0) return 0;
    int64_t *bytes = (int64_t *)(intptr_t)value;
    if ((bytes[0] & 0xFFFFFFFFFFFFFF00LL) != TAIDA_WASM_BYTES_MAGIC) return 0;
    int64_t len = bytes[1];
    if (len < 0 || len > TAIDA_WASM_SHA256_MAX_INPUT_BYTES) return -1;
    uint64_t total_bytes = (uint64_t)(2 + len) * 8ULL;
    if (!taida_wasm_sha256_ptr_readable(value, total_bytes)) return -1;
    if (out_bytes) *out_bytes = bytes;
    if (out_len) *out_len = len;
    return 1;
}

static void taida_wasm_sha256_init(taida_sha256_ctx *ctx) {
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

static void taida_wasm_sha256_transform(taida_sha256_ctx *ctx, const unsigned char block[64]) {
    uint32_t w[64];
    for (int i = 0; i < 16; i++) {
        int j = i * 4;
        w[i] = ((uint32_t)block[j] << 24) |
               ((uint32_t)block[j + 1] << 16) |
               ((uint32_t)block[j + 2] << 8) |
               (uint32_t)block[j + 3];
    }
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = taida_wasm_sha256_rotr(w[i - 15], 7) ^
                      taida_wasm_sha256_rotr(w[i - 15], 18) ^
                      (w[i - 15] >> 3);
        uint32_t s1 = taida_wasm_sha256_rotr(w[i - 2], 17) ^
                      taida_wasm_sha256_rotr(w[i - 2], 19) ^
                      (w[i - 2] >> 10);
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
        uint32_t s1 = taida_wasm_sha256_rotr(e, 6) ^
                      taida_wasm_sha256_rotr(e, 11) ^
                      taida_wasm_sha256_rotr(e, 25);
        uint32_t ch = (e & f) ^ ((~e) & g);
        uint32_t temp1 = h + s1 + ch + TAIDA_WASM_SHA256_K[i] + w[i];
        uint32_t s0 = taida_wasm_sha256_rotr(a, 2) ^
                      taida_wasm_sha256_rotr(a, 13) ^
                      taida_wasm_sha256_rotr(a, 22);
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

static void taida_wasm_sha256_update_byte(taida_sha256_ctx *ctx, unsigned char byte) {
    if (ctx->block_len < 0 || ctx->block_len >= 64) taida_wasm_sha256_trap();
    ctx->block[ctx->block_len++] = byte;
    ctx->total_len++;
    if (ctx->block_len == 64) {
        taida_wasm_sha256_transform(ctx, ctx->block);
        ctx->block_len = 0;
    }
}

static void taida_wasm_sha256_update(taida_sha256_ctx *ctx, const unsigned char *data, int64_t len) {
    if (!data || len <= 0) return;
    for (int64_t i = 0; i < len; i++) {
        taida_wasm_sha256_update_byte(ctx, data[i]);
    }
}

static int64_t taida_wasm_sha256_finish_hex(taida_sha256_ctx *ctx) {
    uint64_t bit_len = ctx->total_len * 8ULL;
    unsigned char digest[32];
    static const char hex[] = "0123456789abcdef";

    /* Invariant: update_byte transforms immediately at 64 bytes, so finish
       only accepts a pending block length in 0..63. */
    if (ctx->block_len < 0 || ctx->block_len >= 64) taida_wasm_sha256_trap();
    ctx->block[ctx->block_len++] = 0x80;
    if (ctx->block_len > 56) {
        while (ctx->block_len < 64) ctx->block[ctx->block_len++] = 0;
        taida_wasm_sha256_transform(ctx, ctx->block);
        ctx->block_len = 0;
    }
    while (ctx->block_len < 56) ctx->block[ctx->block_len++] = 0;

    for (int i = 0; i < 8; i++) {
        ctx->block[56 + i] = (unsigned char)(bit_len >> (56 - i * 8));
    }
    taida_wasm_sha256_transform(ctx, ctx->block);

    for (int i = 0; i < 8; i++) {
        digest[i * 4] = (unsigned char)(ctx->state[i] >> 24);
        digest[i * 4 + 1] = (unsigned char)(ctx->state[i] >> 16);
        digest[i * 4 + 2] = (unsigned char)(ctx->state[i] >> 8);
        digest[i * 4 + 3] = (unsigned char)(ctx->state[i]);
    }

    char *out = (char *)(intptr_t)taida_str_alloc(64);
    if (!out) taida_wasm_sha256_trap();
    for (int i = 0; i < 32; i++) {
        out[i * 2] = hex[(digest[i] >> 4) & 0x0f];
        out[i * 2 + 1] = hex[digest[i] & 0x0f];
    }
    out[64] = '\0';
    return (int64_t)(intptr_t)out;
}

int64_t taida_sha256(int64_t value) {
    taida_sha256_ctx ctx;
    taida_wasm_sha256_init(&ctx);

    int64_t len = 0;
    int64_t *list = (int64_t *)0;
    int list_status = taida_wasm_sha256_list_input(value, &list, &len);
    if (list_status < 0) taida_wasm_sha256_trap();
    if (list_status > 0) {
        for (int64_t i = 0; i < len; i++) {
            taida_wasm_sha256_update_byte(
                &ctx,
                (unsigned char)(list[WASM_LIST_ELEMS + i] & 0xff));
        }
        return taida_wasm_sha256_finish_hex(&ctx);
    }

    int64_t *bytes = (int64_t *)0;
    int bytes_status = taida_wasm_sha256_bytes_input(value, &bytes, &len);
    if (bytes_status < 0) taida_wasm_sha256_trap();
    if (bytes_status > 0) {
        for (int64_t i = 0; i < len; i++) {
            taida_wasm_sha256_update_byte(&ctx, (unsigned char)(bytes[2 + i] & 0xff));
        }
        return taida_wasm_sha256_finish_hex(&ctx);
    }

    const char *s = (const char *)(intptr_t)value;
    if (!taida_wasm_sha256_bounded_strlen(s, TAIDA_WASM_SHA256_MAX_INPUT_BYTES, &len)) {
        taida_wasm_sha256_trap();
    }
    taida_wasm_sha256_update(&ctx, (const unsigned char *)s, len);
    return taida_wasm_sha256_finish_hex(&ctx);
}

/* ── F55 S4: extended crypto surface (all WASM profiles) ─────────────── */
/* SHA-512 / 384 / 224, HMAC-SHA256, constant-time equality, hex/base64    */
/* encode. These return Str / Bool only, so they need no Bytes constructor */
/* and work on every profile (like sha256). hexDecode / base64Decode /     */
/* randomBytes (which produce Bytes) live in runtime_wasi_io.c and are     */
/* profile-gated to wasm-wasi / wasm-full.                                 */

/* Materialize a Str|Bytes|List value into a freshly wasm_alloc'd byte
   buffer. Returns 1 and sets *out + *out_len on success; traps on a
   structurally invalid Bytes/List; returns 0 (with empty result) for a
   value that is neither Str, Bytes, nor List. */
static int taida_wasm_crypto_materialize(int64_t value, unsigned char **out, int64_t *out_len) {
    int64_t len = 0;
    int64_t *list = (int64_t *)0;
    int list_status = taida_wasm_sha256_list_input(value, &list, &len);
    if (list_status < 0) taida_wasm_sha256_trap();
    if (list_status > 0) {
        unsigned char *buf = (unsigned char *)(intptr_t)wasm_alloc((unsigned int)(len > 0 ? len : 1));
        for (int64_t i = 0; i < len; i++) buf[i] = (unsigned char)(list[WASM_LIST_ELEMS + i] & 0xff);
        *out = buf; *out_len = len; return 1;
    }
    int64_t *bytes = (int64_t *)0;
    int bytes_status = taida_wasm_sha256_bytes_input(value, &bytes, &len);
    if (bytes_status < 0) taida_wasm_sha256_trap();
    if (bytes_status > 0) {
        unsigned char *buf = (unsigned char *)(intptr_t)wasm_alloc((unsigned int)(len > 0 ? len : 1));
        for (int64_t i = 0; i < len; i++) buf[i] = (unsigned char)(bytes[2 + i] & 0xff);
        *out = buf; *out_len = len; return 1;
    }
    const char *s = (const char *)(intptr_t)value;
    if (!taida_wasm_sha256_bounded_strlen(s, TAIDA_WASM_SHA256_MAX_INPUT_BYTES, &len)) {
        taida_wasm_sha256_trap();
    }
    unsigned char *buf = (unsigned char *)(intptr_t)wasm_alloc((unsigned int)(len > 0 ? len : 1));
    for (int64_t i = 0; i < len; i++) buf[i] = (unsigned char)((const unsigned char *)s)[i];
    *out = buf; *out_len = len; return 1;
}

static int64_t taida_wasm_crypto_hex_str(const unsigned char *digest, int n) {
    static const char hex[] = "0123456789abcdef";
    char *out = (char *)(intptr_t)taida_str_alloc((int64_t)(n * 2));
    if (!out) taida_wasm_sha256_trap();
    for (int i = 0; i < n; i++) {
        out[i * 2] = hex[(digest[i] >> 4) & 0x0f];
        out[i * 2 + 1] = hex[digest[i] & 0x0f];
    }
    out[n * 2] = '\0';
    return (int64_t)(intptr_t)out;
}

/* SHA-512 / 384 core (64-bit words, 1024-bit blocks). */
typedef struct {
    uint64_t state[8];
    uint64_t total_lo;
    unsigned char block[128];
    int block_len;
} taida_wasm_sha512_ctx;

static const uint64_t TAIDA_WASM_SHA512_K[80] = {
    0x428a2f98d728ae22ULL, 0x7137449123ef65cdULL, 0xb5c0fbcfec4d3b2fULL, 0xe9b5dba58189dbbcULL,
    0x3956c25bf348b538ULL, 0x59f111f1b605d019ULL, 0x923f82a4af194f9bULL, 0xab1c5ed5da6d8118ULL,
    0xd807aa98a3030242ULL, 0x12835b0145706fbeULL, 0x243185be4ee4b28cULL, 0x550c7dc3d5ffb4e2ULL,
    0x72be5d74f27b896fULL, 0x80deb1fe3b1696b1ULL, 0x9bdc06a725c71235ULL, 0xc19bf174cf692694ULL,
    0xe49b69c19ef14ad2ULL, 0xefbe4786384f25e3ULL, 0x0fc19dc68b8cd5b5ULL, 0x240ca1cc77ac9c65ULL,
    0x2de92c6f592b0275ULL, 0x4a7484aa6ea6e483ULL, 0x5cb0a9dcbd41fbd4ULL, 0x76f988da831153b5ULL,
    0x983e5152ee66dfabULL, 0xa831c66d2db43210ULL, 0xb00327c898fb213fULL, 0xbf597fc7beef0ee4ULL,
    0xc6e00bf33da88fc2ULL, 0xd5a79147930aa725ULL, 0x06ca6351e003826fULL, 0x142929670a0e6e70ULL,
    0x27b70a8546d22ffcULL, 0x2e1b21385c26c926ULL, 0x4d2c6dfc5ac42aedULL, 0x53380d139d95b3dfULL,
    0x650a73548baf63deULL, 0x766a0abb3c77b2a8ULL, 0x81c2c92e47edaee6ULL, 0x92722c851482353bULL,
    0xa2bfe8a14cf10364ULL, 0xa81a664bbc423001ULL, 0xc24b8b70d0f89791ULL, 0xc76c51a30654be30ULL,
    0xd192e819d6ef5218ULL, 0xd69906245565a910ULL, 0xf40e35855771202aULL, 0x106aa07032bbd1b8ULL,
    0x19a4c116b8d2d0c8ULL, 0x1e376c085141ab53ULL, 0x2748774cdf8eeb99ULL, 0x34b0bcb5e19b48a8ULL,
    0x391c0cb3c5c95a63ULL, 0x4ed8aa4ae3418acbULL, 0x5b9cca4f7763e373ULL, 0x682e6ff3d6b2b8a3ULL,
    0x748f82ee5defb2fcULL, 0x78a5636f43172f60ULL, 0x84c87814a1f0ab72ULL, 0x8cc702081a6439ecULL,
    0x90befffa23631e28ULL, 0xa4506cebde82bde9ULL, 0xbef9a3f7b2c67915ULL, 0xc67178f2e372532bULL,
    0xca273eceea26619cULL, 0xd186b8c721c0c207ULL, 0xeada7dd6cde0eb1eULL, 0xf57d4f7fee6ed178ULL,
    0x06f067aa72176fbaULL, 0x0a637dc5a2c898a6ULL, 0x113f9804bef90daeULL, 0x1b710b35131c471bULL,
    0x28db77f523047d84ULL, 0x32caab7b40c72493ULL, 0x3c9ebe0a15c9bebcULL, 0x431d67c49c100d4cULL,
    0x4cc5d4becb3e42b6ULL, 0x597f299cfc657e2aULL, 0x5fcb6fab3ad6faecULL, 0x6c44198c4a475817ULL
};

static uint64_t taida_wasm_sha512_rotr(uint64_t x, unsigned n) {
    return (x >> n) | (x << (64 - n));
}

static void taida_wasm_sha512_transform(taida_wasm_sha512_ctx *ctx, const unsigned char block[128]) {
    uint64_t w[80];
    for (int i = 0; i < 16; i++) {
        int j = i * 8;
        w[i] = ((uint64_t)block[j] << 56) | ((uint64_t)block[j+1] << 48) |
               ((uint64_t)block[j+2] << 40) | ((uint64_t)block[j+3] << 32) |
               ((uint64_t)block[j+4] << 24) | ((uint64_t)block[j+5] << 16) |
               ((uint64_t)block[j+6] << 8) | (uint64_t)block[j+7];
    }
    for (int i = 16; i < 80; i++) {
        uint64_t s0 = taida_wasm_sha512_rotr(w[i-15], 1) ^ taida_wasm_sha512_rotr(w[i-15], 8) ^ (w[i-15] >> 7);
        uint64_t s1 = taida_wasm_sha512_rotr(w[i-2], 19) ^ taida_wasm_sha512_rotr(w[i-2], 61) ^ (w[i-2] >> 6);
        w[i] = w[i-16] + s0 + w[i-7] + s1;
    }
    uint64_t a = ctx->state[0], b = ctx->state[1], c = ctx->state[2], d = ctx->state[3];
    uint64_t e = ctx->state[4], f = ctx->state[5], g = ctx->state[6], h = ctx->state[7];
    for (int i = 0; i < 80; i++) {
        uint64_t s1 = taida_wasm_sha512_rotr(e, 14) ^ taida_wasm_sha512_rotr(e, 18) ^ taida_wasm_sha512_rotr(e, 41);
        uint64_t ch = (e & f) ^ ((~e) & g);
        uint64_t temp1 = h + s1 + ch + TAIDA_WASM_SHA512_K[i] + w[i];
        uint64_t s0 = taida_wasm_sha512_rotr(a, 28) ^ taida_wasm_sha512_rotr(a, 34) ^ taida_wasm_sha512_rotr(a, 39);
        uint64_t maj = (a & b) ^ (a & c) ^ (b & c);
        uint64_t temp2 = s0 + maj;
        h = g; g = f; f = e; e = d + temp1; d = c; c = b; b = a; a = temp1 + temp2;
    }
    ctx->state[0] += a; ctx->state[1] += b; ctx->state[2] += c; ctx->state[3] += d;
    ctx->state[4] += e; ctx->state[5] += f; ctx->state[6] += g; ctx->state[7] += h;
}

static void taida_wasm_sha512_update(taida_wasm_sha512_ctx *ctx, const unsigned char *data, int64_t len) {
    if (!data || len <= 0) return;
    ctx->total_lo += (uint64_t)len;
    int64_t pos = 0;
    while (pos < len) {
        int need = 128 - ctx->block_len;
        int64_t take = (len - pos < need) ? (len - pos) : need;
        for (int64_t i = 0; i < take; i++) ctx->block[ctx->block_len + (int)i] = data[pos + i];
        ctx->block_len += (int)take;
        pos += take;
        if (ctx->block_len == 128) {
            taida_wasm_sha512_transform(ctx, ctx->block);
            ctx->block_len = 0;
        }
    }
}

static void taida_wasm_sha512_final(taida_wasm_sha512_ctx *ctx, unsigned char *out, int out_len) {
    uint64_t bit_len = ctx->total_lo * 8ULL;
    if (ctx->block_len < 0 || ctx->block_len >= 128) taida_wasm_sha256_trap();
    ctx->block[ctx->block_len++] = 0x80;
    if (ctx->block_len > 112) {
        while (ctx->block_len < 128) ctx->block[ctx->block_len++] = 0;
        taida_wasm_sha512_transform(ctx, ctx->block);
        ctx->block_len = 0;
    }
    while (ctx->block_len < 112) ctx->block[ctx->block_len++] = 0;
    for (int i = 0; i < 8; i++) ctx->block[112 + i] = 0;
    for (int i = 0; i < 8; i++) ctx->block[120 + i] = (unsigned char)(bit_len >> (56 - i * 8));
    taida_wasm_sha512_transform(ctx, ctx->block);
    unsigned char full[64];
    for (int i = 0; i < 8; i++) {
        full[i*8]   = (unsigned char)(ctx->state[i] >> 56);
        full[i*8+1] = (unsigned char)(ctx->state[i] >> 48);
        full[i*8+2] = (unsigned char)(ctx->state[i] >> 40);
        full[i*8+3] = (unsigned char)(ctx->state[i] >> 32);
        full[i*8+4] = (unsigned char)(ctx->state[i] >> 24);
        full[i*8+5] = (unsigned char)(ctx->state[i] >> 16);
        full[i*8+6] = (unsigned char)(ctx->state[i] >> 8);
        full[i*8+7] = (unsigned char)(ctx->state[i]);
    }
    for (int i = 0; i < out_len; i++) out[i] = full[i];
}

static int64_t taida_wasm_sha512_family(int64_t value, const uint64_t iv[8], int out_len) {
    unsigned char *raw = (unsigned char *)0;
    int64_t len = 0;
    taida_wasm_crypto_materialize(value, &raw, &len);
    taida_wasm_sha512_ctx ctx;
    for (int i = 0; i < 8; i++) ctx.state[i] = iv[i];
    ctx.total_lo = 0;
    ctx.block_len = 0;
    taida_wasm_sha512_update(&ctx, raw, len);
    unsigned char digest[64];
    taida_wasm_sha512_final(&ctx, digest, out_len);
    return taida_wasm_crypto_hex_str(digest, out_len);
}

int64_t taida_crypto_sha512(int64_t value) {
    static const uint64_t iv[8] = {
        0x6a09e667f3bcc908ULL, 0xbb67ae8584caa73bULL, 0x3c6ef372fe94f82bULL, 0xa54ff53a5f1d36f1ULL,
        0x510e527fade682d1ULL, 0x9b05688c2b3e6c1fULL, 0x1f83d9abfb41bd6bULL, 0x5be0cd19137e2179ULL
    };
    return taida_wasm_sha512_family(value, iv, 64);
}

int64_t taida_crypto_sha384(int64_t value) {
    static const uint64_t iv[8] = {
        0xcbbb9d5dc1059ed8ULL, 0x629a292a367cd507ULL, 0x9159015a3070dd17ULL, 0x152fecd8f70e5939ULL,
        0x67332667ffc00b31ULL, 0x8eb44a8768581511ULL, 0xdb0c2e0d64f98fa7ULL, 0x47b5481dbefa4fa4ULL
    };
    return taida_wasm_sha512_family(value, iv, 48);
}

int64_t taida_crypto_sha224(int64_t value) {
    /* SHA-224 = SHA-256 32-bit core with the SHA-224 IV, digest truncated
       to 28 bytes. Reuses the wasm sha256 byte-feed + transform helpers. */
    unsigned char *raw = (unsigned char *)0;
    int64_t len = 0;
    taida_wasm_crypto_materialize(value, &raw, &len);
    taida_sha256_ctx ctx;
    ctx.state[0] = 0xc1059ed8U; ctx.state[1] = 0x367cd507U;
    ctx.state[2] = 0x3070dd17U; ctx.state[3] = 0xf70e5939U;
    ctx.state[4] = 0xffc00b31U; ctx.state[5] = 0x68581511U;
    ctx.state[6] = 0x64f98fa7U; ctx.state[7] = 0xbefa4fa4U;
    ctx.total_len = 0;
    ctx.block_len = 0;
    for (int64_t i = 0; i < len; i++) taida_wasm_sha256_update_byte(&ctx, raw[i]);
    /* Finalize manually (sha256 padding) to obtain the 32-byte digest. */
    uint64_t bit_len = ctx.total_len * 8ULL;
    if (ctx.block_len < 0 || ctx.block_len >= 64) taida_wasm_sha256_trap();
    ctx.block[ctx.block_len++] = 0x80;
    if (ctx.block_len > 56) {
        while (ctx.block_len < 64) ctx.block[ctx.block_len++] = 0;
        taida_wasm_sha256_transform(&ctx, ctx.block);
        ctx.block_len = 0;
    }
    while (ctx.block_len < 56) ctx.block[ctx.block_len++] = 0;
    for (int i = 0; i < 8; i++) ctx.block[56 + i] = (unsigned char)(bit_len >> (56 - i * 8));
    taida_wasm_sha256_transform(&ctx, ctx.block);
    unsigned char digest[32];
    for (int i = 0; i < 8; i++) {
        digest[i*4]   = (unsigned char)(ctx.state[i] >> 24);
        digest[i*4+1] = (unsigned char)(ctx.state[i] >> 16);
        digest[i*4+2] = (unsigned char)(ctx.state[i] >> 8);
        digest[i*4+3] = (unsigned char)(ctx.state[i]);
    }
    return taida_wasm_crypto_hex_str(digest, 28);
}

/* HMAC-SHA256 (RFC 2104), block size 64, reuses the sha256 core. */
int64_t taida_crypto_hmac_sha256(int64_t key_val, int64_t data_val) {
    unsigned char *key = (unsigned char *)0; int64_t key_len = 0;
    unsigned char *data = (unsigned char *)0; int64_t data_len = 0;
    taida_wasm_crypto_materialize(key_val, &key, &key_len);
    taida_wasm_crypto_materialize(data_val, &data, &data_len);
    unsigned char key_block[64];
    for (int i = 0; i < 64; i++) key_block[i] = 0;
    if (key_len > 64) {
        taida_sha256_ctx kc;
        taida_wasm_sha256_init(&kc);
        for (int64_t i = 0; i < key_len; i++) taida_wasm_sha256_update_byte(&kc, key[i]);
        /* finalize manually into 32-byte digest */
        uint64_t bl = kc.total_len * 8ULL;
        kc.block[kc.block_len++] = 0x80;
        if (kc.block_len > 56) { while (kc.block_len < 64) kc.block[kc.block_len++] = 0; taida_wasm_sha256_transform(&kc, kc.block); kc.block_len = 0; }
        while (kc.block_len < 56) kc.block[kc.block_len++] = 0;
        for (int i = 0; i < 8; i++) kc.block[56 + i] = (unsigned char)(bl >> (56 - i * 8));
        taida_wasm_sha256_transform(&kc, kc.block);
        for (int i = 0; i < 8; i++) {
            key_block[i*4]   = (unsigned char)(kc.state[i] >> 24);
            key_block[i*4+1] = (unsigned char)(kc.state[i] >> 16);
            key_block[i*4+2] = (unsigned char)(kc.state[i] >> 8);
            key_block[i*4+3] = (unsigned char)(kc.state[i]);
        }
    } else {
        for (int64_t i = 0; i < key_len; i++) key_block[i] = key[i];
    }
    unsigned char ipad[64], opad[64];
    for (int i = 0; i < 64; i++) { ipad[i] = key_block[i] ^ 0x36; opad[i] = key_block[i] ^ 0x5c; }
    /* inner = SHA256(ipad || data) */
    taida_sha256_ctx ic; taida_wasm_sha256_init(&ic);
    for (int i = 0; i < 64; i++) taida_wasm_sha256_update_byte(&ic, ipad[i]);
    for (int64_t i = 0; i < data_len; i++) taida_wasm_sha256_update_byte(&ic, data[i]);
    uint64_t bl1 = ic.total_len * 8ULL;
    ic.block[ic.block_len++] = 0x80;
    if (ic.block_len > 56) { while (ic.block_len < 64) ic.block[ic.block_len++] = 0; taida_wasm_sha256_transform(&ic, ic.block); ic.block_len = 0; }
    while (ic.block_len < 56) ic.block[ic.block_len++] = 0;
    for (int i = 0; i < 8; i++) ic.block[56 + i] = (unsigned char)(bl1 >> (56 - i * 8));
    taida_wasm_sha256_transform(&ic, ic.block);
    unsigned char inner[32];
    for (int i = 0; i < 8; i++) {
        inner[i*4]   = (unsigned char)(ic.state[i] >> 24);
        inner[i*4+1] = (unsigned char)(ic.state[i] >> 16);
        inner[i*4+2] = (unsigned char)(ic.state[i] >> 8);
        inner[i*4+3] = (unsigned char)(ic.state[i]);
    }
    /* outer = SHA256(opad || inner) */
    taida_sha256_ctx oc; taida_wasm_sha256_init(&oc);
    for (int i = 0; i < 64; i++) taida_wasm_sha256_update_byte(&oc, opad[i]);
    for (int i = 0; i < 32; i++) taida_wasm_sha256_update_byte(&oc, inner[i]);
    uint64_t bl2 = oc.total_len * 8ULL;
    oc.block[oc.block_len++] = 0x80;
    if (oc.block_len > 56) { while (oc.block_len < 64) oc.block[oc.block_len++] = 0; taida_wasm_sha256_transform(&oc, oc.block); oc.block_len = 0; }
    while (oc.block_len < 56) oc.block[oc.block_len++] = 0;
    for (int i = 0; i < 8; i++) oc.block[56 + i] = (unsigned char)(bl2 >> (56 - i * 8));
    taida_wasm_sha256_transform(&oc, oc.block);
    unsigned char outer[32];
    for (int i = 0; i < 8; i++) {
        outer[i*4]   = (unsigned char)(oc.state[i] >> 24);
        outer[i*4+1] = (unsigned char)(oc.state[i] >> 16);
        outer[i*4+2] = (unsigned char)(oc.state[i] >> 8);
        outer[i*4+3] = (unsigned char)(oc.state[i]);
    }
    return taida_wasm_crypto_hex_str(outer, 32);
}

/* constantTimeEquals: length mismatch -> false, full-length walk of a. */
int64_t taida_crypto_constant_time_equals(int64_t a_val, int64_t b_val) {
    unsigned char *a = (unsigned char *)0; int64_t a_len = 0;
    unsigned char *b = (unsigned char *)0; int64_t b_len = 0;
    taida_wasm_crypto_materialize(a_val, &a, &a_len);
    taida_wasm_crypto_materialize(b_val, &b, &b_len);
    unsigned char diff = (a_len != b_len) ? 1 : 0;
    for (int64_t i = 0; i < a_len; i++) {
        unsigned char bb = (b_len == 0) ? 0 : b[i % b_len];
        diff |= a[i] ^ bb;
    }
    return (int64_t)(diff == 0 ? 1 : 0);
}

/* F56 Phase 4: secret-aware consumers. Reveal the sealed secret's inner value
   (pack index 1 = __value) and feed it to the crypto primitive; the result
   (MAC hex / bool) is public. (Level 0 on WASM: the inner value lives in the
   pack — see the F56 backend guarantee matrix.) */
int64_t taida_hmac_sha256_secret(int64_t secret_ptr, int64_t msg_val) {
    /* F56-FB-002: reject a non-sealed first argument (parity with the interpreter
       and JS, which throw). Without this guard WASM silently MAC'd the plain value
       under `--no-check`; the checker [E1506] gates it normally. */
    if (!_wasm_carrier_kind(secret_ptr)) {
        return taida_throw(taida_make_error(
            WSTR("TypeError"),
            WSTR("HmacSha256 expects a sealed Secret as its first argument — seal it with MoltenizeSecret[...] or read it via MoltenizeSecretFromEnv / MoltenizeSecretFromFile")));
    }
    int64_t inner = taida_pack_get_idx(secret_ptr, 1);
    return taida_crypto_hmac_sha256(inner, msg_val);
}

int64_t taida_constant_time_eq_secret(int64_t secret_ptr, int64_t cand_val) {
    if (!_wasm_carrier_kind(secret_ptr)) { /* F56-FB-002: see taida_hmac_sha256_secret. */
        return taida_throw(taida_make_error(
            WSTR("TypeError"),
            WSTR("ConstantTimeEq expects a sealed Secret as its first argument — seal it with MoltenizeSecret[...] or read it via MoltenizeSecretFromEnv / MoltenizeSecretFromFile")));
    }
    int64_t inner = taida_pack_get_idx(secret_ptr, 1);
    return taida_crypto_constant_time_equals(inner, cand_val);
}

/* hexEncode: Str|Bytes -> lower-hex Str. */
int64_t taida_crypto_hex_encode(int64_t value) {
    unsigned char *raw = (unsigned char *)0; int64_t len = 0;
    taida_wasm_crypto_materialize(value, &raw, &len);
    static const char hex[] = "0123456789abcdef";
    char *out = (char *)(intptr_t)taida_str_alloc(len * 2);
    if (!out) taida_wasm_sha256_trap();
    for (int64_t i = 0; i < len; i++) {
        out[i*2] = hex[(raw[i] >> 4) & 0x0f];
        out[i*2+1] = hex[raw[i] & 0x0f];
    }
    out[len * 2] = '\0';
    return (int64_t)(intptr_t)out;
}

static const char TAIDA_WASM_B64_ALPHABET[64] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/* base64Encode: Str|Bytes -> Str (RFC 4648, padded). */
int64_t taida_crypto_base64_encode(int64_t value) {
    unsigned char *raw = (unsigned char *)0; int64_t len = 0;
    taida_wasm_crypto_materialize(value, &raw, &len);
    int64_t out_len = ((len + 2) / 3) * 4;
    char *out = (char *)(intptr_t)taida_str_alloc(out_len);
    if (!out) taida_wasm_sha256_trap();
    int64_t oi = 0, i = 0;
    while (i + 3 <= len) {
        uint32_t n = ((uint32_t)raw[i] << 16) | ((uint32_t)raw[i+1] << 8) | (uint32_t)raw[i+2];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 18) & 0x3f];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 12) & 0x3f];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 6) & 0x3f];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[n & 0x3f];
        i += 3;
    }
    int64_t rem = len - i;
    if (rem == 1) {
        uint32_t n = (uint32_t)raw[i] << 16;
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 18) & 0x3f];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 12) & 0x3f];
        out[oi++] = '=';
        out[oi++] = '=';
    } else if (rem == 2) {
        uint32_t n = ((uint32_t)raw[i] << 16) | ((uint32_t)raw[i+1] << 8);
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 18) & 0x3f];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 12) & 0x3f];
        out[oi++] = TAIDA_WASM_B64_ALPHABET[(n >> 6) & 0x3f];
        out[oi++] = '=';
    }
    out[out_len] = '\0';
    return (int64_t)(intptr_t)out;
}

// ── Unicode case tables (generated; tests/f65_case_tables.rs checks them) ──
// Generated from the interpreter's Unicode tables: Upper / Lower follow
// Rust's char::to_uppercase / to_lowercase, and the final-sigma rule of
// str::to_lowercase reads the Cased / Case_Ignorable sets. Unicode 17.0.0.
static const int32_t TAIDA_CASE_UPPER_RUNS[][4] = {
    {0x61, 0x7A, -32, 1},
    {0xB5, 0xB5, 743, 1},
    {0xE0, 0xF6, -32, 1},
    {0xF8, 0xFE, -32, 1},
    {0xFF, 0xFF, 121, 1},
    {0x101, 0x12F, -1, 2},
    {0x131, 0x131, -232, 1},
    {0x133, 0x137, -1, 2},
    {0x13A, 0x148, -1, 2},
    {0x14B, 0x177, -1, 2},
    {0x17A, 0x17E, -1, 2},
    {0x17F, 0x17F, -300, 1},
    {0x180, 0x180, 195, 1},
    {0x183, 0x185, -1, 2},
    {0x188, 0x188, -1, 1},
    {0x18C, 0x18C, -1, 1},
    {0x192, 0x192, -1, 1},
    {0x195, 0x195, 97, 1},
    {0x199, 0x199, -1, 1},
    {0x19A, 0x19A, 163, 1},
    {0x19B, 0x19B, 42561, 1},
    {0x19E, 0x19E, 130, 1},
    {0x1A1, 0x1A5, -1, 2},
    {0x1A8, 0x1A8, -1, 1},
    {0x1AD, 0x1AD, -1, 1},
    {0x1B0, 0x1B0, -1, 1},
    {0x1B4, 0x1B6, -1, 2},
    {0x1B9, 0x1B9, -1, 1},
    {0x1BD, 0x1BD, -1, 1},
    {0x1BF, 0x1BF, 56, 1},
    {0x1C5, 0x1C5, -1, 1},
    {0x1C6, 0x1C6, -2, 1},
    {0x1C8, 0x1C8, -1, 1},
    {0x1C9, 0x1C9, -2, 1},
    {0x1CB, 0x1CB, -1, 1},
    {0x1CC, 0x1CC, -2, 1},
    {0x1CE, 0x1DC, -1, 2},
    {0x1DD, 0x1DD, -79, 1},
    {0x1DF, 0x1EF, -1, 2},
    {0x1F2, 0x1F2, -1, 1},
    {0x1F3, 0x1F3, -2, 1},
    {0x1F5, 0x1F5, -1, 1},
    {0x1F9, 0x21F, -1, 2},
    {0x223, 0x233, -1, 2},
    {0x23C, 0x23C, -1, 1},
    {0x23F, 0x240, 10815, 1},
    {0x242, 0x242, -1, 1},
    {0x247, 0x24F, -1, 2},
    {0x250, 0x250, 10783, 1},
    {0x251, 0x251, 10780, 1},
    {0x252, 0x252, 10782, 1},
    {0x253, 0x253, -210, 1},
    {0x254, 0x254, -206, 1},
    {0x256, 0x257, -205, 1},
    {0x259, 0x259, -202, 1},
    {0x25B, 0x25B, -203, 1},
    {0x25C, 0x25C, 42319, 1},
    {0x260, 0x260, -205, 1},
    {0x261, 0x261, 42315, 1},
    {0x263, 0x263, -207, 1},
    {0x264, 0x264, 42343, 1},
    {0x265, 0x265, 42280, 1},
    {0x266, 0x266, 42308, 1},
    {0x268, 0x268, -209, 1},
    {0x269, 0x269, -211, 1},
    {0x26A, 0x26A, 42308, 1},
    {0x26B, 0x26B, 10743, 1},
    {0x26C, 0x26C, 42305, 1},
    {0x26F, 0x26F, -211, 1},
    {0x271, 0x271, 10749, 1},
    {0x272, 0x272, -213, 1},
    {0x275, 0x275, -214, 1},
    {0x27D, 0x27D, 10727, 1},
    {0x280, 0x280, -218, 1},
    {0x282, 0x282, 42307, 1},
    {0x283, 0x283, -218, 1},
    {0x287, 0x287, 42282, 1},
    {0x288, 0x288, -218, 1},
    {0x289, 0x289, -69, 1},
    {0x28A, 0x28B, -217, 1},
    {0x28C, 0x28C, -71, 1},
    {0x292, 0x292, -219, 1},
    {0x29D, 0x29D, 42261, 1},
    {0x29E, 0x29E, 42258, 1},
    {0x345, 0x345, 84, 1},
    {0x371, 0x373, -1, 2},
    {0x377, 0x377, -1, 1},
    {0x37B, 0x37D, 130, 1},
    {0x3AC, 0x3AC, -38, 1},
    {0x3AD, 0x3AF, -37, 1},
    {0x3B1, 0x3C1, -32, 1},
    {0x3C2, 0x3C2, -31, 1},
    {0x3C3, 0x3CB, -32, 1},
    {0x3CC, 0x3CC, -64, 1},
    {0x3CD, 0x3CE, -63, 1},
    {0x3D0, 0x3D0, -62, 1},
    {0x3D1, 0x3D1, -57, 1},
    {0x3D5, 0x3D5, -47, 1},
    {0x3D6, 0x3D6, -54, 1},
    {0x3D7, 0x3D7, -8, 1},
    {0x3D9, 0x3EF, -1, 2},
    {0x3F0, 0x3F0, -86, 1},
    {0x3F1, 0x3F1, -80, 1},
    {0x3F2, 0x3F2, 7, 1},
    {0x3F3, 0x3F3, -116, 1},
    {0x3F5, 0x3F5, -96, 1},
    {0x3F8, 0x3F8, -1, 1},
    {0x3FB, 0x3FB, -1, 1},
    {0x430, 0x44F, -32, 1},
    {0x450, 0x45F, -80, 1},
    {0x461, 0x481, -1, 2},
    {0x48B, 0x4BF, -1, 2},
    {0x4C2, 0x4CE, -1, 2},
    {0x4CF, 0x4CF, -15, 1},
    {0x4D1, 0x52F, -1, 2},
    {0x561, 0x586, -48, 1},
    {0x10D0, 0x10FA, 3008, 1},
    {0x10FD, 0x10FF, 3008, 1},
    {0x13F8, 0x13FD, -8, 1},
    {0x1C80, 0x1C80, -6254, 1},
    {0x1C81, 0x1C81, -6253, 1},
    {0x1C82, 0x1C82, -6244, 1},
    {0x1C83, 0x1C84, -6242, 1},
    {0x1C85, 0x1C85, -6243, 1},
    {0x1C86, 0x1C86, -6236, 1},
    {0x1C87, 0x1C87, -6181, 1},
    {0x1C88, 0x1C88, 35266, 1},
    {0x1C8A, 0x1C8A, -1, 1},
    {0x1D79, 0x1D79, 35332, 1},
    {0x1D7D, 0x1D7D, 3814, 1},
    {0x1D8E, 0x1D8E, 35384, 1},
    {0x1E01, 0x1E95, -1, 2},
    {0x1E9B, 0x1E9B, -59, 1},
    {0x1EA1, 0x1EFF, -1, 2},
    {0x1F00, 0x1F07, 8, 1},
    {0x1F10, 0x1F15, 8, 1},
    {0x1F20, 0x1F27, 8, 1},
    {0x1F30, 0x1F37, 8, 1},
    {0x1F40, 0x1F45, 8, 1},
    {0x1F51, 0x1F57, 8, 2},
    {0x1F60, 0x1F67, 8, 1},
    {0x1F70, 0x1F71, 74, 1},
    {0x1F72, 0x1F75, 86, 1},
    {0x1F76, 0x1F77, 100, 1},
    {0x1F78, 0x1F79, 128, 1},
    {0x1F7A, 0x1F7B, 112, 1},
    {0x1F7C, 0x1F7D, 126, 1},
    {0x1FB0, 0x1FB1, 8, 1},
    {0x1FBE, 0x1FBE, -7205, 1},
    {0x1FD0, 0x1FD1, 8, 1},
    {0x1FE0, 0x1FE1, 8, 1},
    {0x1FE5, 0x1FE5, 7, 1},
    {0x214E, 0x214E, -28, 1},
    {0x2170, 0x217F, -16, 1},
    {0x2184, 0x2184, -1, 1},
    {0x24D0, 0x24E9, -26, 1},
    {0x2C30, 0x2C5F, -48, 1},
    {0x2C61, 0x2C61, -1, 1},
    {0x2C65, 0x2C65, -10795, 1},
    {0x2C66, 0x2C66, -10792, 1},
    {0x2C68, 0x2C6C, -1, 2},
    {0x2C73, 0x2C73, -1, 1},
    {0x2C76, 0x2C76, -1, 1},
    {0x2C81, 0x2CE3, -1, 2},
    {0x2CEC, 0x2CEE, -1, 2},
    {0x2CF3, 0x2CF3, -1, 1},
    {0x2D00, 0x2D25, -7264, 1},
    {0x2D27, 0x2D27, -7264, 1},
    {0x2D2D, 0x2D2D, -7264, 1},
    {0xA641, 0xA66D, -1, 2},
    {0xA681, 0xA69B, -1, 2},
    {0xA723, 0xA72F, -1, 2},
    {0xA733, 0xA76F, -1, 2},
    {0xA77A, 0xA77C, -1, 2},
    {0xA77F, 0xA787, -1, 2},
    {0xA78C, 0xA78C, -1, 1},
    {0xA791, 0xA793, -1, 2},
    {0xA794, 0xA794, 48, 1},
    {0xA797, 0xA7A9, -1, 2},
    {0xA7B5, 0xA7C3, -1, 2},
    {0xA7C8, 0xA7CA, -1, 2},
    {0xA7CD, 0xA7DB, -1, 2},
    {0xA7F6, 0xA7F6, -1, 1},
    {0xAB53, 0xAB53, -928, 1},
    {0xAB70, 0xABBF, -38864, 1},
    {0xFF41, 0xFF5A, -32, 1},
    {0x10428, 0x1044F, -40, 1},
    {0x104D8, 0x104FB, -40, 1},
    {0x10597, 0x105A1, -39, 1},
    {0x105A3, 0x105B1, -39, 1},
    {0x105B3, 0x105B9, -39, 1},
    {0x105BB, 0x105BC, -39, 1},
    {0x10CC0, 0x10CF2, -64, 1},
    {0x10D70, 0x10D85, -32, 1},
    {0x118C0, 0x118DF, -32, 1},
    {0x16E60, 0x16E7F, -32, 1},
    {0x16EBB, 0x16ED3, -27, 1},
    {0x1E922, 0x1E943, -34, 1},
};
static const uint32_t TAIDA_CASE_UPPER_SPECIAL[][4] = {
    {0xDF, 0x53, 0x53, 0x0},
    {0x149, 0x2BC, 0x4E, 0x0},
    {0x1F0, 0x4A, 0x30C, 0x0},
    {0x390, 0x399, 0x308, 0x301},
    {0x3B0, 0x3A5, 0x308, 0x301},
    {0x587, 0x535, 0x552, 0x0},
    {0x1E96, 0x48, 0x331, 0x0},
    {0x1E97, 0x54, 0x308, 0x0},
    {0x1E98, 0x57, 0x30A, 0x0},
    {0x1E99, 0x59, 0x30A, 0x0},
    {0x1E9A, 0x41, 0x2BE, 0x0},
    {0x1F50, 0x3A5, 0x313, 0x0},
    {0x1F52, 0x3A5, 0x313, 0x300},
    {0x1F54, 0x3A5, 0x313, 0x301},
    {0x1F56, 0x3A5, 0x313, 0x342},
    {0x1F80, 0x1F08, 0x399, 0x0},
    {0x1F81, 0x1F09, 0x399, 0x0},
    {0x1F82, 0x1F0A, 0x399, 0x0},
    {0x1F83, 0x1F0B, 0x399, 0x0},
    {0x1F84, 0x1F0C, 0x399, 0x0},
    {0x1F85, 0x1F0D, 0x399, 0x0},
    {0x1F86, 0x1F0E, 0x399, 0x0},
    {0x1F87, 0x1F0F, 0x399, 0x0},
    {0x1F88, 0x1F08, 0x399, 0x0},
    {0x1F89, 0x1F09, 0x399, 0x0},
    {0x1F8A, 0x1F0A, 0x399, 0x0},
    {0x1F8B, 0x1F0B, 0x399, 0x0},
    {0x1F8C, 0x1F0C, 0x399, 0x0},
    {0x1F8D, 0x1F0D, 0x399, 0x0},
    {0x1F8E, 0x1F0E, 0x399, 0x0},
    {0x1F8F, 0x1F0F, 0x399, 0x0},
    {0x1F90, 0x1F28, 0x399, 0x0},
    {0x1F91, 0x1F29, 0x399, 0x0},
    {0x1F92, 0x1F2A, 0x399, 0x0},
    {0x1F93, 0x1F2B, 0x399, 0x0},
    {0x1F94, 0x1F2C, 0x399, 0x0},
    {0x1F95, 0x1F2D, 0x399, 0x0},
    {0x1F96, 0x1F2E, 0x399, 0x0},
    {0x1F97, 0x1F2F, 0x399, 0x0},
    {0x1F98, 0x1F28, 0x399, 0x0},
    {0x1F99, 0x1F29, 0x399, 0x0},
    {0x1F9A, 0x1F2A, 0x399, 0x0},
    {0x1F9B, 0x1F2B, 0x399, 0x0},
    {0x1F9C, 0x1F2C, 0x399, 0x0},
    {0x1F9D, 0x1F2D, 0x399, 0x0},
    {0x1F9E, 0x1F2E, 0x399, 0x0},
    {0x1F9F, 0x1F2F, 0x399, 0x0},
    {0x1FA0, 0x1F68, 0x399, 0x0},
    {0x1FA1, 0x1F69, 0x399, 0x0},
    {0x1FA2, 0x1F6A, 0x399, 0x0},
    {0x1FA3, 0x1F6B, 0x399, 0x0},
    {0x1FA4, 0x1F6C, 0x399, 0x0},
    {0x1FA5, 0x1F6D, 0x399, 0x0},
    {0x1FA6, 0x1F6E, 0x399, 0x0},
    {0x1FA7, 0x1F6F, 0x399, 0x0},
    {0x1FA8, 0x1F68, 0x399, 0x0},
    {0x1FA9, 0x1F69, 0x399, 0x0},
    {0x1FAA, 0x1F6A, 0x399, 0x0},
    {0x1FAB, 0x1F6B, 0x399, 0x0},
    {0x1FAC, 0x1F6C, 0x399, 0x0},
    {0x1FAD, 0x1F6D, 0x399, 0x0},
    {0x1FAE, 0x1F6E, 0x399, 0x0},
    {0x1FAF, 0x1F6F, 0x399, 0x0},
    {0x1FB2, 0x1FBA, 0x399, 0x0},
    {0x1FB3, 0x391, 0x399, 0x0},
    {0x1FB4, 0x386, 0x399, 0x0},
    {0x1FB6, 0x391, 0x342, 0x0},
    {0x1FB7, 0x391, 0x342, 0x399},
    {0x1FBC, 0x391, 0x399, 0x0},
    {0x1FC2, 0x1FCA, 0x399, 0x0},
    {0x1FC3, 0x397, 0x399, 0x0},
    {0x1FC4, 0x389, 0x399, 0x0},
    {0x1FC6, 0x397, 0x342, 0x0},
    {0x1FC7, 0x397, 0x342, 0x399},
    {0x1FCC, 0x397, 0x399, 0x0},
    {0x1FD2, 0x399, 0x308, 0x300},
    {0x1FD3, 0x399, 0x308, 0x301},
    {0x1FD6, 0x399, 0x342, 0x0},
    {0x1FD7, 0x399, 0x308, 0x342},
    {0x1FE2, 0x3A5, 0x308, 0x300},
    {0x1FE3, 0x3A5, 0x308, 0x301},
    {0x1FE4, 0x3A1, 0x313, 0x0},
    {0x1FE6, 0x3A5, 0x342, 0x0},
    {0x1FE7, 0x3A5, 0x308, 0x342},
    {0x1FF2, 0x1FFA, 0x399, 0x0},
    {0x1FF3, 0x3A9, 0x399, 0x0},
    {0x1FF4, 0x38F, 0x399, 0x0},
    {0x1FF6, 0x3A9, 0x342, 0x0},
    {0x1FF7, 0x3A9, 0x342, 0x399},
    {0x1FFC, 0x3A9, 0x399, 0x0},
    {0xFB00, 0x46, 0x46, 0x0},
    {0xFB01, 0x46, 0x49, 0x0},
    {0xFB02, 0x46, 0x4C, 0x0},
    {0xFB03, 0x46, 0x46, 0x49},
    {0xFB04, 0x46, 0x46, 0x4C},
    {0xFB05, 0x53, 0x54, 0x0},
    {0xFB06, 0x53, 0x54, 0x0},
    {0xFB13, 0x544, 0x546, 0x0},
    {0xFB14, 0x544, 0x535, 0x0},
    {0xFB15, 0x544, 0x53B, 0x0},
    {0xFB16, 0x54E, 0x546, 0x0},
    {0xFB17, 0x544, 0x53D, 0x0},
};
static const int32_t TAIDA_CASE_LOWER_RUNS[][4] = {
    {0x41, 0x5A, 32, 1},
    {0xC0, 0xD6, 32, 1},
    {0xD8, 0xDE, 32, 1},
    {0x100, 0x12E, 1, 2},
    {0x132, 0x136, 1, 2},
    {0x139, 0x147, 1, 2},
    {0x14A, 0x176, 1, 2},
    {0x178, 0x178, -121, 1},
    {0x179, 0x17D, 1, 2},
    {0x181, 0x181, 210, 1},
    {0x182, 0x184, 1, 2},
    {0x186, 0x186, 206, 1},
    {0x187, 0x187, 1, 1},
    {0x189, 0x18A, 205, 1},
    {0x18B, 0x18B, 1, 1},
    {0x18E, 0x18E, 79, 1},
    {0x18F, 0x18F, 202, 1},
    {0x190, 0x190, 203, 1},
    {0x191, 0x191, 1, 1},
    {0x193, 0x193, 205, 1},
    {0x194, 0x194, 207, 1},
    {0x196, 0x196, 211, 1},
    {0x197, 0x197, 209, 1},
    {0x198, 0x198, 1, 1},
    {0x19C, 0x19C, 211, 1},
    {0x19D, 0x19D, 213, 1},
    {0x19F, 0x19F, 214, 1},
    {0x1A0, 0x1A4, 1, 2},
    {0x1A6, 0x1A6, 218, 1},
    {0x1A7, 0x1A7, 1, 1},
    {0x1A9, 0x1A9, 218, 1},
    {0x1AC, 0x1AC, 1, 1},
    {0x1AE, 0x1AE, 218, 1},
    {0x1AF, 0x1AF, 1, 1},
    {0x1B1, 0x1B2, 217, 1},
    {0x1B3, 0x1B5, 1, 2},
    {0x1B7, 0x1B7, 219, 1},
    {0x1B8, 0x1B8, 1, 1},
    {0x1BC, 0x1BC, 1, 1},
    {0x1C4, 0x1C4, 2, 1},
    {0x1C5, 0x1C5, 1, 1},
    {0x1C7, 0x1C7, 2, 1},
    {0x1C8, 0x1C8, 1, 1},
    {0x1CA, 0x1CA, 2, 1},
    {0x1CB, 0x1DB, 1, 2},
    {0x1DE, 0x1EE, 1, 2},
    {0x1F1, 0x1F1, 2, 1},
    {0x1F2, 0x1F4, 1, 2},
    {0x1F6, 0x1F6, -97, 1},
    {0x1F7, 0x1F7, -56, 1},
    {0x1F8, 0x21E, 1, 2},
    {0x220, 0x220, -130, 1},
    {0x222, 0x232, 1, 2},
    {0x23A, 0x23A, 10795, 1},
    {0x23B, 0x23B, 1, 1},
    {0x23D, 0x23D, -163, 1},
    {0x23E, 0x23E, 10792, 1},
    {0x241, 0x241, 1, 1},
    {0x243, 0x243, -195, 1},
    {0x244, 0x244, 69, 1},
    {0x245, 0x245, 71, 1},
    {0x246, 0x24E, 1, 2},
    {0x370, 0x372, 1, 2},
    {0x376, 0x376, 1, 1},
    {0x37F, 0x37F, 116, 1},
    {0x386, 0x386, 38, 1},
    {0x388, 0x38A, 37, 1},
    {0x38C, 0x38C, 64, 1},
    {0x38E, 0x38F, 63, 1},
    {0x391, 0x3A1, 32, 1},
    {0x3A3, 0x3AB, 32, 1},
    {0x3CF, 0x3CF, 8, 1},
    {0x3D8, 0x3EE, 1, 2},
    {0x3F4, 0x3F4, -60, 1},
    {0x3F7, 0x3F7, 1, 1},
    {0x3F9, 0x3F9, -7, 1},
    {0x3FA, 0x3FA, 1, 1},
    {0x3FD, 0x3FF, -130, 1},
    {0x400, 0x40F, 80, 1},
    {0x410, 0x42F, 32, 1},
    {0x460, 0x480, 1, 2},
    {0x48A, 0x4BE, 1, 2},
    {0x4C0, 0x4C0, 15, 1},
    {0x4C1, 0x4CD, 1, 2},
    {0x4D0, 0x52E, 1, 2},
    {0x531, 0x556, 48, 1},
    {0x10A0, 0x10C5, 7264, 1},
    {0x10C7, 0x10C7, 7264, 1},
    {0x10CD, 0x10CD, 7264, 1},
    {0x13A0, 0x13EF, 38864, 1},
    {0x13F0, 0x13F5, 8, 1},
    {0x1C89, 0x1C89, 1, 1},
    {0x1C90, 0x1CBA, -3008, 1},
    {0x1CBD, 0x1CBF, -3008, 1},
    {0x1E00, 0x1E94, 1, 2},
    {0x1E9E, 0x1E9E, -7615, 1},
    {0x1EA0, 0x1EFE, 1, 2},
    {0x1F08, 0x1F0F, -8, 1},
    {0x1F18, 0x1F1D, -8, 1},
    {0x1F28, 0x1F2F, -8, 1},
    {0x1F38, 0x1F3F, -8, 1},
    {0x1F48, 0x1F4D, -8, 1},
    {0x1F59, 0x1F5F, -8, 2},
    {0x1F68, 0x1F6F, -8, 1},
    {0x1F88, 0x1F8F, -8, 1},
    {0x1F98, 0x1F9F, -8, 1},
    {0x1FA8, 0x1FAF, -8, 1},
    {0x1FB8, 0x1FB9, -8, 1},
    {0x1FBA, 0x1FBB, -74, 1},
    {0x1FBC, 0x1FBC, -9, 1},
    {0x1FC8, 0x1FCB, -86, 1},
    {0x1FCC, 0x1FCC, -9, 1},
    {0x1FD8, 0x1FD9, -8, 1},
    {0x1FDA, 0x1FDB, -100, 1},
    {0x1FE8, 0x1FE9, -8, 1},
    {0x1FEA, 0x1FEB, -112, 1},
    {0x1FEC, 0x1FEC, -7, 1},
    {0x1FF8, 0x1FF9, -128, 1},
    {0x1FFA, 0x1FFB, -126, 1},
    {0x1FFC, 0x1FFC, -9, 1},
    {0x2126, 0x2126, -7517, 1},
    {0x212A, 0x212A, -8383, 1},
    {0x212B, 0x212B, -8262, 1},
    {0x2132, 0x2132, 28, 1},
    {0x2160, 0x216F, 16, 1},
    {0x2183, 0x2183, 1, 1},
    {0x24B6, 0x24CF, 26, 1},
    {0x2C00, 0x2C2F, 48, 1},
    {0x2C60, 0x2C60, 1, 1},
    {0x2C62, 0x2C62, -10743, 1},
    {0x2C63, 0x2C63, -3814, 1},
    {0x2C64, 0x2C64, -10727, 1},
    {0x2C67, 0x2C6B, 1, 2},
    {0x2C6D, 0x2C6D, -10780, 1},
    {0x2C6E, 0x2C6E, -10749, 1},
    {0x2C6F, 0x2C6F, -10783, 1},
    {0x2C70, 0x2C70, -10782, 1},
    {0x2C72, 0x2C72, 1, 1},
    {0x2C75, 0x2C75, 1, 1},
    {0x2C7E, 0x2C7F, -10815, 1},
    {0x2C80, 0x2CE2, 1, 2},
    {0x2CEB, 0x2CED, 1, 2},
    {0x2CF2, 0x2CF2, 1, 1},
    {0xA640, 0xA66C, 1, 2},
    {0xA680, 0xA69A, 1, 2},
    {0xA722, 0xA72E, 1, 2},
    {0xA732, 0xA76E, 1, 2},
    {0xA779, 0xA77B, 1, 2},
    {0xA77D, 0xA77D, -35332, 1},
    {0xA77E, 0xA786, 1, 2},
    {0xA78B, 0xA78B, 1, 1},
    {0xA78D, 0xA78D, -42280, 1},
    {0xA790, 0xA792, 1, 2},
    {0xA796, 0xA7A8, 1, 2},
    {0xA7AA, 0xA7AA, -42308, 1},
    {0xA7AB, 0xA7AB, -42319, 1},
    {0xA7AC, 0xA7AC, -42315, 1},
    {0xA7AD, 0xA7AD, -42305, 1},
    {0xA7AE, 0xA7AE, -42308, 1},
    {0xA7B0, 0xA7B0, -42258, 1},
    {0xA7B1, 0xA7B1, -42282, 1},
    {0xA7B2, 0xA7B2, -42261, 1},
    {0xA7B3, 0xA7B3, 928, 1},
    {0xA7B4, 0xA7C2, 1, 2},
    {0xA7C4, 0xA7C4, -48, 1},
    {0xA7C5, 0xA7C5, -42307, 1},
    {0xA7C6, 0xA7C6, -35384, 1},
    {0xA7C7, 0xA7C9, 1, 2},
    {0xA7CB, 0xA7CB, -42343, 1},
    {0xA7CC, 0xA7DA, 1, 2},
    {0xA7DC, 0xA7DC, -42561, 1},
    {0xA7F5, 0xA7F5, 1, 1},
    {0xFF21, 0xFF3A, 32, 1},
    {0x10400, 0x10427, 40, 1},
    {0x104B0, 0x104D3, 40, 1},
    {0x10570, 0x1057A, 39, 1},
    {0x1057C, 0x1058A, 39, 1},
    {0x1058C, 0x10592, 39, 1},
    {0x10594, 0x10595, 39, 1},
    {0x10C80, 0x10CB2, 64, 1},
    {0x10D50, 0x10D65, 32, 1},
    {0x118A0, 0x118BF, 32, 1},
    {0x16E40, 0x16E5F, 32, 1},
    {0x16EA0, 0x16EB8, 27, 1},
    {0x1E900, 0x1E921, 34, 1},
};
static const uint32_t TAIDA_CASE_LOWER_SPECIAL[][4] = {
    {0x130, 0x69, 0x307, 0x0},
};
static const uint32_t TAIDA_CASED_RANGES[][2] = {
    {0x41, 0x5A},
    {0x61, 0x7A},
    {0xAA, 0xAA},
    {0xB5, 0xB5},
    {0xBA, 0xBA},
    {0xC0, 0xD6},
    {0xD8, 0xF6},
    {0xF8, 0x1BA},
    {0x1BC, 0x1BF},
    {0x1C4, 0x293},
    {0x296, 0x2AF},
    {0x370, 0x373},
    {0x376, 0x377},
    {0x37B, 0x37D},
    {0x37F, 0x37F},
    {0x386, 0x386},
    {0x388, 0x38A},
    {0x38C, 0x38C},
    {0x38E, 0x3A1},
    {0x3A3, 0x3F5},
    {0x3F7, 0x481},
    {0x48A, 0x52F},
    {0x531, 0x556},
    {0x560, 0x588},
    {0x10A0, 0x10C5},
    {0x10C7, 0x10C7},
    {0x10CD, 0x10CD},
    {0x10D0, 0x10FA},
    {0x10FD, 0x10FF},
    {0x13A0, 0x13F5},
    {0x13F8, 0x13FD},
    {0x1C80, 0x1C8A},
    {0x1C90, 0x1CBA},
    {0x1CBD, 0x1CBF},
    {0x1D00, 0x1D2B},
    {0x1D6B, 0x1D77},
    {0x1D79, 0x1D9A},
    {0x1E00, 0x1F15},
    {0x1F18, 0x1F1D},
    {0x1F20, 0x1F45},
    {0x1F48, 0x1F4D},
    {0x1F50, 0x1F57},
    {0x1F59, 0x1F59},
    {0x1F5B, 0x1F5B},
    {0x1F5D, 0x1F5D},
    {0x1F5F, 0x1F7D},
    {0x1F80, 0x1FB4},
    {0x1FB6, 0x1FBC},
    {0x1FBE, 0x1FBE},
    {0x1FC2, 0x1FC4},
    {0x1FC6, 0x1FCC},
    {0x1FD0, 0x1FD3},
    {0x1FD6, 0x1FDB},
    {0x1FE0, 0x1FEC},
    {0x1FF2, 0x1FF4},
    {0x1FF6, 0x1FFC},
    {0x2102, 0x2102},
    {0x2107, 0x2107},
    {0x210A, 0x2113},
    {0x2115, 0x2115},
    {0x2119, 0x211D},
    {0x2124, 0x2124},
    {0x2126, 0x2126},
    {0x2128, 0x2128},
    {0x212A, 0x212D},
    {0x212F, 0x2134},
    {0x2139, 0x2139},
    {0x213C, 0x213F},
    {0x2145, 0x2149},
    {0x214E, 0x214E},
    {0x2160, 0x217F},
    {0x2183, 0x2184},
    {0x24B6, 0x24E9},
    {0x2C00, 0x2C7B},
    {0x2C7E, 0x2CE4},
    {0x2CEB, 0x2CEE},
    {0x2CF2, 0x2CF3},
    {0x2D00, 0x2D25},
    {0x2D27, 0x2D27},
    {0x2D2D, 0x2D2D},
    {0xA640, 0xA66D},
    {0xA680, 0xA69B},
    {0xA722, 0xA76F},
    {0xA771, 0xA787},
    {0xA78B, 0xA78E},
    {0xA790, 0xA7DC},
    {0xA7F5, 0xA7F6},
    {0xA7FA, 0xA7FA},
    {0xAB30, 0xAB5A},
    {0xAB60, 0xAB68},
    {0xAB70, 0xABBF},
    {0xFB00, 0xFB06},
    {0xFB13, 0xFB17},
    {0xFF21, 0xFF3A},
    {0xFF41, 0xFF5A},
    {0x10400, 0x1044F},
    {0x104B0, 0x104D3},
    {0x104D8, 0x104FB},
    {0x10570, 0x1057A},
    {0x1057C, 0x1058A},
    {0x1058C, 0x10592},
    {0x10594, 0x10595},
    {0x10597, 0x105A1},
    {0x105A3, 0x105B1},
    {0x105B3, 0x105B9},
    {0x105BB, 0x105BC},
    {0x10C80, 0x10CB2},
    {0x10CC0, 0x10CF2},
    {0x10D50, 0x10D65},
    {0x10D70, 0x10D85},
    {0x118A0, 0x118DF},
    {0x16E40, 0x16E7F},
    {0x16EA0, 0x16EB8},
    {0x16EBB, 0x16ED3},
    {0x1D400, 0x1D454},
    {0x1D456, 0x1D49C},
    {0x1D49E, 0x1D49F},
    {0x1D4A2, 0x1D4A2},
    {0x1D4A5, 0x1D4A6},
    {0x1D4A9, 0x1D4AC},
    {0x1D4AE, 0x1D4B9},
    {0x1D4BB, 0x1D4BB},
    {0x1D4BD, 0x1D4C3},
    {0x1D4C5, 0x1D505},
    {0x1D507, 0x1D50A},
    {0x1D50D, 0x1D514},
    {0x1D516, 0x1D51C},
    {0x1D51E, 0x1D539},
    {0x1D53B, 0x1D53E},
    {0x1D540, 0x1D544},
    {0x1D546, 0x1D546},
    {0x1D54A, 0x1D550},
    {0x1D552, 0x1D6A5},
    {0x1D6A8, 0x1D6C0},
    {0x1D6C2, 0x1D6DA},
    {0x1D6DC, 0x1D6FA},
    {0x1D6FC, 0x1D714},
    {0x1D716, 0x1D734},
    {0x1D736, 0x1D74E},
    {0x1D750, 0x1D76E},
    {0x1D770, 0x1D788},
    {0x1D78A, 0x1D7A8},
    {0x1D7AA, 0x1D7C2},
    {0x1D7C4, 0x1D7CB},
    {0x1DF00, 0x1DF09},
    {0x1DF0B, 0x1DF1E},
    {0x1DF25, 0x1DF2A},
    {0x1E900, 0x1E943},
    {0x1F130, 0x1F149},
    {0x1F150, 0x1F169},
    {0x1F170, 0x1F189},
};
static const uint32_t TAIDA_CASE_IGNORABLE_RANGES[][2] = {
    {0x27, 0x27},
    {0x2E, 0x2E},
    {0x3A, 0x3A},
    {0x5E, 0x5E},
    {0x60, 0x60},
    {0xA8, 0xA8},
    {0xAD, 0xAD},
    {0xAF, 0xAF},
    {0xB4, 0xB4},
    {0xB7, 0xB8},
    {0x2B0, 0x36F},
    {0x374, 0x375},
    {0x37A, 0x37A},
    {0x384, 0x385},
    {0x387, 0x387},
    {0x483, 0x489},
    {0x559, 0x559},
    {0x55F, 0x55F},
    {0x591, 0x5BD},
    {0x5BF, 0x5BF},
    {0x5C1, 0x5C2},
    {0x5C4, 0x5C5},
    {0x5C7, 0x5C7},
    {0x5F4, 0x5F4},
    {0x600, 0x605},
    {0x610, 0x61A},
    {0x61C, 0x61C},
    {0x640, 0x640},
    {0x64B, 0x65F},
    {0x670, 0x670},
    {0x6D6, 0x6DD},
    {0x6DF, 0x6E8},
    {0x6EA, 0x6ED},
    {0x70F, 0x70F},
    {0x711, 0x711},
    {0x730, 0x74A},
    {0x7A6, 0x7B0},
    {0x7EB, 0x7F5},
    {0x7FA, 0x7FA},
    {0x7FD, 0x7FD},
    {0x816, 0x82D},
    {0x859, 0x85B},
    {0x888, 0x888},
    {0x890, 0x891},
    {0x897, 0x89F},
    {0x8C9, 0x902},
    {0x93A, 0x93A},
    {0x93C, 0x93C},
    {0x941, 0x948},
    {0x94D, 0x94D},
    {0x951, 0x957},
    {0x962, 0x963},
    {0x971, 0x971},
    {0x981, 0x981},
    {0x9BC, 0x9BC},
    {0x9C1, 0x9C4},
    {0x9CD, 0x9CD},
    {0x9E2, 0x9E3},
    {0x9FE, 0x9FE},
    {0xA01, 0xA02},
    {0xA3C, 0xA3C},
    {0xA41, 0xA42},
    {0xA47, 0xA48},
    {0xA4B, 0xA4D},
    {0xA51, 0xA51},
    {0xA70, 0xA71},
    {0xA75, 0xA75},
    {0xA81, 0xA82},
    {0xABC, 0xABC},
    {0xAC1, 0xAC5},
    {0xAC7, 0xAC8},
    {0xACD, 0xACD},
    {0xAE2, 0xAE3},
    {0xAFA, 0xAFF},
    {0xB01, 0xB01},
    {0xB3C, 0xB3C},
    {0xB3F, 0xB3F},
    {0xB41, 0xB44},
    {0xB4D, 0xB4D},
    {0xB55, 0xB56},
    {0xB62, 0xB63},
    {0xB82, 0xB82},
    {0xBC0, 0xBC0},
    {0xBCD, 0xBCD},
    {0xC00, 0xC00},
    {0xC04, 0xC04},
    {0xC3C, 0xC3C},
    {0xC3E, 0xC40},
    {0xC46, 0xC48},
    {0xC4A, 0xC4D},
    {0xC55, 0xC56},
    {0xC62, 0xC63},
    {0xC81, 0xC81},
    {0xCBC, 0xCBC},
    {0xCBF, 0xCBF},
    {0xCC6, 0xCC6},
    {0xCCC, 0xCCD},
    {0xCE2, 0xCE3},
    {0xD00, 0xD01},
    {0xD3B, 0xD3C},
    {0xD41, 0xD44},
    {0xD4D, 0xD4D},
    {0xD62, 0xD63},
    {0xD81, 0xD81},
    {0xDCA, 0xDCA},
    {0xDD2, 0xDD4},
    {0xDD6, 0xDD6},
    {0xE31, 0xE31},
    {0xE34, 0xE3A},
    {0xE46, 0xE4E},
    {0xEB1, 0xEB1},
    {0xEB4, 0xEBC},
    {0xEC6, 0xEC6},
    {0xEC8, 0xECE},
    {0xF18, 0xF19},
    {0xF35, 0xF35},
    {0xF37, 0xF37},
    {0xF39, 0xF39},
    {0xF71, 0xF7E},
    {0xF80, 0xF84},
    {0xF86, 0xF87},
    {0xF8D, 0xF97},
    {0xF99, 0xFBC},
    {0xFC6, 0xFC6},
    {0x102D, 0x1030},
    {0x1032, 0x1037},
    {0x1039, 0x103A},
    {0x103D, 0x103E},
    {0x1058, 0x1059},
    {0x105E, 0x1060},
    {0x1071, 0x1074},
    {0x1082, 0x1082},
    {0x1085, 0x1086},
    {0x108D, 0x108D},
    {0x109D, 0x109D},
    {0x10FC, 0x10FC},
    {0x135D, 0x135F},
    {0x1712, 0x1714},
    {0x1732, 0x1733},
    {0x1752, 0x1753},
    {0x1772, 0x1773},
    {0x17B4, 0x17B5},
    {0x17B7, 0x17BD},
    {0x17C6, 0x17C6},
    {0x17C9, 0x17D3},
    {0x17D7, 0x17D7},
    {0x17DD, 0x17DD},
    {0x180B, 0x180F},
    {0x1843, 0x1843},
    {0x1885, 0x1886},
    {0x18A9, 0x18A9},
    {0x1920, 0x1922},
    {0x1927, 0x1928},
    {0x1932, 0x1932},
    {0x1939, 0x193B},
    {0x1A17, 0x1A18},
    {0x1A1B, 0x1A1B},
    {0x1A56, 0x1A56},
    {0x1A58, 0x1A5E},
    {0x1A60, 0x1A60},
    {0x1A62, 0x1A62},
    {0x1A65, 0x1A6C},
    {0x1A73, 0x1A7C},
    {0x1A7F, 0x1A7F},
    {0x1AA7, 0x1AA7},
    {0x1AB0, 0x1ADD},
    {0x1AE0, 0x1AEB},
    {0x1B00, 0x1B03},
    {0x1B34, 0x1B34},
    {0x1B36, 0x1B3A},
    {0x1B3C, 0x1B3C},
    {0x1B42, 0x1B42},
    {0x1B6B, 0x1B73},
    {0x1B80, 0x1B81},
    {0x1BA2, 0x1BA5},
    {0x1BA8, 0x1BA9},
    {0x1BAB, 0x1BAD},
    {0x1BE6, 0x1BE6},
    {0x1BE8, 0x1BE9},
    {0x1BED, 0x1BED},
    {0x1BEF, 0x1BF1},
    {0x1C2C, 0x1C33},
    {0x1C36, 0x1C37},
    {0x1C78, 0x1C7D},
    {0x1CD0, 0x1CD2},
    {0x1CD4, 0x1CE0},
    {0x1CE2, 0x1CE8},
    {0x1CED, 0x1CED},
    {0x1CF4, 0x1CF4},
    {0x1CF8, 0x1CF9},
    {0x1D2C, 0x1D6A},
    {0x1D78, 0x1D78},
    {0x1D9B, 0x1DFF},
    {0x1FBD, 0x1FBD},
    {0x1FBF, 0x1FC1},
    {0x1FCD, 0x1FCF},
    {0x1FDD, 0x1FDF},
    {0x1FED, 0x1FEF},
    {0x1FFD, 0x1FFE},
    {0x200B, 0x200F},
    {0x2018, 0x2019},
    {0x2024, 0x2024},
    {0x2027, 0x2027},
    {0x202A, 0x202E},
    {0x2060, 0x2064},
    {0x2066, 0x206F},
    {0x2071, 0x2071},
    {0x207F, 0x207F},
    {0x2090, 0x209C},
    {0x20D0, 0x20F0},
    {0x2C7C, 0x2C7D},
    {0x2CEF, 0x2CF1},
    {0x2D6F, 0x2D6F},
    {0x2D7F, 0x2D7F},
    {0x2DE0, 0x2DFF},
    {0x2E2F, 0x2E2F},
    {0x3005, 0x3005},
    {0x302A, 0x302D},
    {0x3031, 0x3035},
    {0x303B, 0x303B},
    {0x3099, 0x309E},
    {0x30FC, 0x30FE},
    {0xA015, 0xA015},
    {0xA4F8, 0xA4FD},
    {0xA60C, 0xA60C},
    {0xA66F, 0xA672},
    {0xA674, 0xA67D},
    {0xA67F, 0xA67F},
    {0xA69C, 0xA69F},
    {0xA6F0, 0xA6F1},
    {0xA700, 0xA721},
    {0xA770, 0xA770},
    {0xA788, 0xA78A},
    {0xA7F1, 0xA7F4},
    {0xA7F8, 0xA7F9},
    {0xA802, 0xA802},
    {0xA806, 0xA806},
    {0xA80B, 0xA80B},
    {0xA825, 0xA826},
    {0xA82C, 0xA82C},
    {0xA8C4, 0xA8C5},
    {0xA8E0, 0xA8F1},
    {0xA8FF, 0xA8FF},
    {0xA926, 0xA92D},
    {0xA947, 0xA951},
    {0xA980, 0xA982},
    {0xA9B3, 0xA9B3},
    {0xA9B6, 0xA9B9},
    {0xA9BC, 0xA9BD},
    {0xA9CF, 0xA9CF},
    {0xA9E5, 0xA9E6},
    {0xAA29, 0xAA2E},
    {0xAA31, 0xAA32},
    {0xAA35, 0xAA36},
    {0xAA43, 0xAA43},
    {0xAA4C, 0xAA4C},
    {0xAA70, 0xAA70},
    {0xAA7C, 0xAA7C},
    {0xAAB0, 0xAAB0},
    {0xAAB2, 0xAAB4},
    {0xAAB7, 0xAAB8},
    {0xAABE, 0xAABF},
    {0xAAC1, 0xAAC1},
    {0xAADD, 0xAADD},
    {0xAAEC, 0xAAED},
    {0xAAF3, 0xAAF4},
    {0xAAF6, 0xAAF6},
    {0xAB5B, 0xAB5F},
    {0xAB69, 0xAB6B},
    {0xABE5, 0xABE5},
    {0xABE8, 0xABE8},
    {0xABED, 0xABED},
    {0xFB1E, 0xFB1E},
    {0xFBB2, 0xFBC2},
    {0xFE00, 0xFE0F},
    {0xFE13, 0xFE13},
    {0xFE20, 0xFE2F},
    {0xFE52, 0xFE52},
    {0xFE55, 0xFE55},
    {0xFEFF, 0xFEFF},
    {0xFF07, 0xFF07},
    {0xFF0E, 0xFF0E},
    {0xFF1A, 0xFF1A},
    {0xFF3E, 0xFF3E},
    {0xFF40, 0xFF40},
    {0xFF70, 0xFF70},
    {0xFF9E, 0xFF9F},
    {0xFFE3, 0xFFE3},
    {0xFFF9, 0xFFFB},
    {0x101FD, 0x101FD},
    {0x102E0, 0x102E0},
    {0x10376, 0x1037A},
    {0x10780, 0x10785},
    {0x10787, 0x107B0},
    {0x107B2, 0x107BA},
    {0x10A01, 0x10A03},
    {0x10A05, 0x10A06},
    {0x10A0C, 0x10A0F},
    {0x10A38, 0x10A3A},
    {0x10A3F, 0x10A3F},
    {0x10AE5, 0x10AE6},
    {0x10D24, 0x10D27},
    {0x10D4E, 0x10D4E},
    {0x10D69, 0x10D6D},
    {0x10D6F, 0x10D6F},
    {0x10EAB, 0x10EAC},
    {0x10EC5, 0x10EC5},
    {0x10EFA, 0x10EFF},
    {0x10F46, 0x10F50},
    {0x10F82, 0x10F85},
    {0x11001, 0x11001},
    {0x11038, 0x11046},
    {0x11070, 0x11070},
    {0x11073, 0x11074},
    {0x1107F, 0x11081},
    {0x110B3, 0x110B6},
    {0x110B9, 0x110BA},
    {0x110BD, 0x110BD},
    {0x110C2, 0x110C2},
    {0x110CD, 0x110CD},
    {0x11100, 0x11102},
    {0x11127, 0x1112B},
    {0x1112D, 0x11134},
    {0x11173, 0x11173},
    {0x11180, 0x11181},
    {0x111B6, 0x111BE},
    {0x111C9, 0x111CC},
    {0x111CF, 0x111CF},
    {0x1122F, 0x11231},
    {0x11234, 0x11234},
    {0x11236, 0x11237},
    {0x1123E, 0x1123E},
    {0x11241, 0x11241},
    {0x112DF, 0x112DF},
    {0x112E3, 0x112EA},
    {0x11300, 0x11301},
    {0x1133B, 0x1133C},
    {0x11340, 0x11340},
    {0x11366, 0x1136C},
    {0x11370, 0x11374},
    {0x113BB, 0x113C0},
    {0x113CE, 0x113CE},
    {0x113D0, 0x113D0},
    {0x113D2, 0x113D2},
    {0x113E1, 0x113E2},
    {0x11438, 0x1143F},
    {0x11442, 0x11444},
    {0x11446, 0x11446},
    {0x1145E, 0x1145E},
    {0x114B3, 0x114B8},
    {0x114BA, 0x114BA},
    {0x114BF, 0x114C0},
    {0x114C2, 0x114C3},
    {0x115B2, 0x115B5},
    {0x115BC, 0x115BD},
    {0x115BF, 0x115C0},
    {0x115DC, 0x115DD},
    {0x11633, 0x1163A},
    {0x1163D, 0x1163D},
    {0x1163F, 0x11640},
    {0x116AB, 0x116AB},
    {0x116AD, 0x116AD},
    {0x116B0, 0x116B5},
    {0x116B7, 0x116B7},
    {0x1171D, 0x1171D},
    {0x1171F, 0x1171F},
    {0x11722, 0x11725},
    {0x11727, 0x1172B},
    {0x1182F, 0x11837},
    {0x11839, 0x1183A},
    {0x1193B, 0x1193C},
    {0x1193E, 0x1193E},
    {0x11943, 0x11943},
    {0x119D4, 0x119D7},
    {0x119DA, 0x119DB},
    {0x119E0, 0x119E0},
    {0x11A01, 0x11A0A},
    {0x11A33, 0x11A38},
    {0x11A3B, 0x11A3E},
    {0x11A47, 0x11A47},
    {0x11A51, 0x11A56},
    {0x11A59, 0x11A5B},
    {0x11A8A, 0x11A96},
    {0x11A98, 0x11A99},
    {0x11B60, 0x11B60},
    {0x11B62, 0x11B64},
    {0x11B66, 0x11B66},
    {0x11C30, 0x11C36},
    {0x11C38, 0x11C3D},
    {0x11C3F, 0x11C3F},
    {0x11C92, 0x11CA7},
    {0x11CAA, 0x11CB0},
    {0x11CB2, 0x11CB3},
    {0x11CB5, 0x11CB6},
    {0x11D31, 0x11D36},
    {0x11D3A, 0x11D3A},
    {0x11D3C, 0x11D3D},
    {0x11D3F, 0x11D45},
    {0x11D47, 0x11D47},
    {0x11D90, 0x11D91},
    {0x11D95, 0x11D95},
    {0x11D97, 0x11D97},
    {0x11DD9, 0x11DD9},
    {0x11EF3, 0x11EF4},
    {0x11F00, 0x11F01},
    {0x11F36, 0x11F3A},
    {0x11F40, 0x11F40},
    {0x11F42, 0x11F42},
    {0x11F5A, 0x11F5A},
    {0x13430, 0x13440},
    {0x13447, 0x13455},
    {0x1611E, 0x16129},
    {0x1612D, 0x1612F},
    {0x16AF0, 0x16AF4},
    {0x16B30, 0x16B36},
    {0x16B40, 0x16B43},
    {0x16D40, 0x16D42},
    {0x16D6B, 0x16D6C},
    {0x16F4F, 0x16F4F},
    {0x16F8F, 0x16F9F},
    {0x16FE0, 0x16FE1},
    {0x16FE3, 0x16FE4},
    {0x16FF2, 0x16FF3},
    {0x1AFF0, 0x1AFF3},
    {0x1AFF5, 0x1AFFB},
    {0x1AFFD, 0x1AFFE},
    {0x1BC9D, 0x1BC9E},
    {0x1BCA0, 0x1BCA3},
    {0x1CF00, 0x1CF2D},
    {0x1CF30, 0x1CF46},
    {0x1D167, 0x1D169},
    {0x1D173, 0x1D182},
    {0x1D185, 0x1D18B},
    {0x1D1AA, 0x1D1AD},
    {0x1D242, 0x1D244},
    {0x1DA00, 0x1DA36},
    {0x1DA3B, 0x1DA6C},
    {0x1DA75, 0x1DA75},
    {0x1DA84, 0x1DA84},
    {0x1DA9B, 0x1DA9F},
    {0x1DAA1, 0x1DAAF},
    {0x1E000, 0x1E006},
    {0x1E008, 0x1E018},
    {0x1E01B, 0x1E021},
    {0x1E023, 0x1E024},
    {0x1E026, 0x1E02A},
    {0x1E030, 0x1E06D},
    {0x1E08F, 0x1E08F},
    {0x1E130, 0x1E13D},
    {0x1E2AE, 0x1E2AE},
    {0x1E2EC, 0x1E2EF},
    {0x1E4EB, 0x1E4EF},
    {0x1E5EE, 0x1E5EF},
    {0x1E6E3, 0x1E6E3},
    {0x1E6E6, 0x1E6E6},
    {0x1E6EE, 0x1E6EF},
    {0x1E6F5, 0x1E6F5},
    {0x1E6FF, 0x1E6FF},
    {0x1E8D0, 0x1E8D6},
    {0x1E944, 0x1E94B},
    {0x1F3FB, 0x1F3FF},
    {0xE0001, 0xE0001},
    {0xE0020, 0xE007F},
    {0xE0100, 0xE01EF},
};
// ── end of Unicode case tables ──

// Whether `cp` lies in one of the `n` sorted, disjoint ranges of `set`.
static int taida_case_in_set(const uint32_t (*set)[2], int64_t n, uint32_t cp) {
    int64_t lo = 0, hi = n;
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        if (cp < set[mid][0]) {
            hi = mid;
        } else if (cp > set[mid][1]) {
            lo = mid + 1;
        } else {
            return 1;
        }
    }
    return 0;
}

// The code points `cp` maps to in upper case (`upper`) or lower case, as
// Rust's char::to_uppercase / to_lowercase give them: 1 to 3 of them, in
// `out`. Returns how many.
static int64_t taida_case_map_cp(uint32_t cp, int upper, uint32_t out[3]) {
    const uint32_t (*special)[4] = upper ? TAIDA_CASE_UPPER_SPECIAL : TAIDA_CASE_LOWER_SPECIAL;
    int64_t nspecial = upper
        ? (int64_t)(sizeof(TAIDA_CASE_UPPER_SPECIAL) / sizeof(TAIDA_CASE_UPPER_SPECIAL[0]))
        : (int64_t)(sizeof(TAIDA_CASE_LOWER_SPECIAL) / sizeof(TAIDA_CASE_LOWER_SPECIAL[0]));
    int64_t lo = 0, hi = nspecial;
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        if (cp < special[mid][0]) {
            hi = mid;
        } else if (cp > special[mid][0]) {
            lo = mid + 1;
        } else {
            int64_t n = 0;
            while (n < 3 && special[mid][1 + n] != 0) {
                out[n] = special[mid][1 + n];
                n++;
            }
            return n;
        }
    }
    const int32_t (*runs)[4] = upper ? TAIDA_CASE_UPPER_RUNS : TAIDA_CASE_LOWER_RUNS;
    int64_t nruns = upper
        ? (int64_t)(sizeof(TAIDA_CASE_UPPER_RUNS) / sizeof(TAIDA_CASE_UPPER_RUNS[0]))
        : (int64_t)(sizeof(TAIDA_CASE_LOWER_RUNS) / sizeof(TAIDA_CASE_LOWER_RUNS[0]));
    lo = 0;
    hi = nruns;
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        if ((int64_t)cp < runs[mid][0]) {
            hi = mid;
        } else if ((int64_t)cp > runs[mid][1]) {
            lo = mid + 1;
        } else {
            if (((int64_t)cp - runs[mid][0]) % runs[mid][3] == 0) {
                out[0] = (uint32_t)((int64_t)cp + runs[mid][2]);
                return 1;
            }
            break;
        }
    }
    out[0] = cp;
    return 1;
}

// Decode the UTF-8 character at s[i] (before `len`) into `*cp`; returns its
// byte length. A byte that does not start a valid sequence is taken alone
// and reported as 0xFFFFFFFF, to be copied unchanged.
static int64_t taida_case_utf8_decode(const unsigned char *s, int64_t len, int64_t i, uint32_t *cp) {
    unsigned char c = s[i];
    int64_t need = c < 0x80 ? 1 : (c >= 0xC2 && c <= 0xDF) ? 2 : (c >= 0xE0 && c <= 0xEF) ? 3
                 : (c >= 0xF0 && c <= 0xF4) ? 4 : 0;
    if (need == 0 || i + need > len) {
        *cp = 0xFFFFFFFFu;
        return 1;
    }
    uint32_t v = need == 1 ? c : need == 2 ? (uint32_t)(c & 0x1F) : need == 3 ? (uint32_t)(c & 0x0F) : (uint32_t)(c & 0x07);
    for (int64_t k = 1; k < need; k++) {
        unsigned char cc = s[i + k];
        if ((cc & 0xC0) != 0x80) {
            *cp = 0xFFFFFFFFu;
            return 1;
        }
        v = (v << 6) | (uint32_t)(cc & 0x3F);
    }
    *cp = v;
    return need;
}

// Write `cp` as UTF-8 at `out` (when not NULL); returns its byte length.
static int64_t taida_case_utf8_put(uint32_t cp, unsigned char *out) {
    if (cp < 0x80) {
        if (out) out[0] = (unsigned char)cp;
        return 1;
    }
    if (cp < 0x800) {
        if (out) {
            out[0] = (unsigned char)(0xC0 | (cp >> 6));
            out[1] = (unsigned char)(0x80 | (cp & 0x3F));
        }
        return 2;
    }
    if (cp < 0x10000) {
        if (out) {
            out[0] = (unsigned char)(0xE0 | (cp >> 12));
            out[1] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
            out[2] = (unsigned char)(0x80 | (cp & 0x3F));
        }
        return 3;
    }
    if (out) {
        out[0] = (unsigned char)(0xF0 | (cp >> 18));
        out[1] = (unsigned char)(0x80 | ((cp >> 12) & 0x3F));
        out[2] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        out[3] = (unsigned char)(0x80 | (cp & 0x3F));
    }
    return 4;
}

// Whether the next character from s[i] going forward (`step` 1) or before
// s[i] going backward (`step` -1), skipping case-ignorable ones, is cased.
static int taida_case_cased_beyond(const unsigned char *s, int64_t len, int64_t i, int step) {
    int64_t nset_cased = (int64_t)(sizeof(TAIDA_CASED_RANGES) / sizeof(TAIDA_CASED_RANGES[0]));
    int64_t nset_ign = (int64_t)(sizeof(TAIDA_CASE_IGNORABLE_RANGES) / sizeof(TAIDA_CASE_IGNORABLE_RANGES[0]));
    while (step > 0 ? i < len : i > 0) {
        int64_t start = i;
        if (step < 0) {
            start = i - 1;
            while (start > 0 && (s[start] & 0xC0) == 0x80 && i - start < 4) start--;
        }
        uint32_t cp;
        int64_t n = taida_case_utf8_decode(s, len, start, &cp);
        if (step < 0 && start + n != i) {
            // Not a whole character ending at i: take the last byte alone.
            start = i - 1;
            cp = 0xFFFFFFFFu;
            n = 1;
        }
        if (cp == 0xFFFFFFFFu || !taida_case_in_set(TAIDA_CASE_IGNORABLE_RANGES, nset_ign, cp)) {
            return cp != 0xFFFFFFFFu && taida_case_in_set(TAIDA_CASED_RANGES, nset_cased, cp);
        }
        i = step > 0 ? start + n : start;
    }
    return 0;
}

// Upper / Lower of the `len` bytes at `s`, as Rust's str::to_uppercase /
// to_lowercase give them (a capital sigma ending a word lowers to the final
// form). Writes the result at `out` when not NULL; returns its byte length.
static int64_t taida_case_convert(const unsigned char *s, int64_t len, int upper, unsigned char *out) {
    int64_t o = 0;
    int64_t i = 0;
    while (i < len) {
        uint32_t cp;
        int64_t n = taida_case_utf8_decode(s, len, i, &cp);
        if (cp == 0xFFFFFFFFu) {
            if (out) out[o] = s[i];
            o += 1;
            i += 1;
            continue;
        }
        uint32_t mapped[3];
        int64_t m;
        if (!upper && cp == 0x3A3) {
            int final_sigma = taida_case_cased_beyond(s, len, i, -1)
                && !taida_case_cased_beyond(s, len, i + n, 1);
            mapped[0] = final_sigma ? 0x3C2 : 0x3C3;
            m = 1;
        } else {
            m = taida_case_map_cp(cp, upper, mapped);
        }
        for (int64_t k = 0; k < m; k++) {
            o += taida_case_utf8_put(mapped[k], out ? out + o : (unsigned char *)0);
        }
        i += n;
    }
    return o;
}

/* Upper / Lower: the whole Unicode case mapping, as the reference has it
   (native twin: taida_str_case). */
static int64_t _wasm_str_case(int64_t s_raw, int upper) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int64_t len = _wf_strlen(s);
    int64_t n = taida_case_convert((const unsigned char *)s, len, upper, (unsigned char *)0);
    char *r = _wasm_str_alloc((unsigned int)(n + 1));
    taida_case_convert((const unsigned char *)s, len, upper, (unsigned char *)r);
    r[n] = '\0';
    return (int64_t)r;
}

/// Upper[str]()
int64_t taida_str_to_upper(int64_t s_raw) {
    return _wasm_str_case(s_raw, 1);
}

/// Lower[str]()
int64_t taida_str_to_lower(int64_t s_raw) {
    return _wasm_str_case(s_raw, 0);
}

/// Trim[str]() -- strip leading and trailing whitespace
/* Byte length of the whitespace character at s[i] (before `len`), or 0.
   Whitespace is Unicode's White_Space set — what the interpreter's
   `str::trim` strips: TAB..CR, space, NEL, NBSP, U+1680, U+2000..U+200A,
   U+2028, U+2029, U+202F, U+205F and U+3000. */
static int _wc_trim_ws_len(const unsigned char *s, int i, int len) {
    unsigned char c = s[i];
    if (c == ' ' || (c >= 0x09 && c <= 0x0D)) return 1;
    if (c == 0xC2 && i + 1 < len && (s[i + 1] == 0x85 || s[i + 1] == 0xA0)) return 2;
    if (i + 2 >= len) return 0;
    unsigned char c1 = s[i + 1], c2 = s[i + 2];
    if (c == 0xE1 && c1 == 0x9A && c2 == 0x80) return 3;
    if (c == 0xE2 && c1 == 0x80
        && ((c2 >= 0x80 && c2 <= 0x8A) || c2 == 0xA8 || c2 == 0xA9 || c2 == 0xAF)) return 3;
    if (c == 0xE2 && c1 == 0x81 && c2 == 0x9F) return 3;
    if (c == 0xE3 && c1 == 0x80 && c2 == 0x80) return 3;
    return 0;
}

static int _wc_trim_start_pos(const unsigned char *s, int len) {
    int start = 0;
    int n;
    while (start < len && (n = _wc_trim_ws_len(s, start, len)) > 0) start += n;
    return start;
}

static int _wc_trim_end_pos(const unsigned char *s, int start, int end) {
    while (end > start) {
        /* start of the last character (at most 3 continuation bytes back) */
        int cs = end - 1;
        while (cs > start && (s[cs] & 0xC0) == 0x80 && end - cs < 4) cs--;
        if (_wc_trim_ws_len(s, cs, end) != end - cs) break;
        end = cs;
    }
    return end;
}

int64_t taida_str_trim(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int start = _wc_trim_start_pos((const unsigned char *)s, len);
    int end = _wc_trim_end_pos((const unsigned char *)s, start, len);
    int slen = end - start;
    char *r = _wasm_str_alloc((unsigned int)(slen + 1));
    _wf_memcpy(r, s + start, slen);
    r[slen] = '\0';
    return (int64_t)r;
}

/// TrimStart[str]() -- strip leading whitespace
int64_t taida_str_trim_start(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int start = _wc_trim_start_pos((const unsigned char *)s, len);
    int slen = len - start;
    char *r = _wasm_str_alloc((unsigned int)(slen + 1));
    _wf_memcpy(r, s + start, slen);
    r[slen] = '\0';
    return (int64_t)r;
}

/// TrimEnd[str]() -- strip trailing whitespace
int64_t taida_str_trim_end(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    int len = _wf_strlen(s);
    int end = _wc_trim_end_pos((const unsigned char *)s, 0, len);
    char *r = _wasm_str_alloc((unsigned int)(end + 1));
    _wf_memcpy(r, s, end);
    r[end] = '\0';
    return (int64_t)r;
}

/// Split[str, sep]() -- split string by separator, return list of strings.
/// If sep is empty, splits into individual characters.
static int _wc_utf8_decode_one(const unsigned char *buf, int len, int *consumed, uint32_t *out_cp); /* fwd */

int64_t taida_str_split(int64_t s_raw, int64_t sep_raw) {
    const char *s = (const char *)s_raw;
    const char *sep = (const char *)sep_raw;
    /* F62B-019: fragments are Str — record the elem kind so display /
       jsonEncode render them as strings instead of probing pointers. */
    if (!s) {
        int64_t empty = taida_list_new();
        taida_list_set_elem_tag(empty, 3 /* STR */);
        return empty;
    }
    int64_t list = taida_list_new();
    taida_list_set_elem_tag(list, 3 /* STR */);
    if (!sep || _wf_strlen(sep) == 0) {
        /* Locked split("") semantics (B11 method lock, matches the
           interpreter / native / JS): chars split with no empty
           fragments, empty input gives the empty list. CODEPOINT-wise:
           the previous per-BYTE walk tore multibyte UTF-8 apart,
           diverging from native/interp on non-ASCII input. */
        int len = _wf_strlen(s);
        int off = 0;
        while (off < len) {
            int consumed = 0;
            uint32_t cp = 0;
            if (!_wc_utf8_decode_one((const unsigned char *)s + off, len - off, &consumed, &cp)
                || consumed <= 0) {
                consumed = 1; /* invalid byte: keep it as a single fragment */
            }
            char *c = _wasm_str_alloc((unsigned int)(consumed + 1));
            _wf_memcpy(c, s + off, consumed);
            c[consumed] = '\0';
            list = taida_list_push(list, (int64_t)c);
            off += consumed;
        }
        return list;
    }
    int sep_len = _wf_strlen(sep);
    const char *p = s;
    while (1) {
        const char *found = _wf_strstr(p, sep);
        if (!found) {
            int slen = _wf_strlen(p);
            char *part = _wasm_str_alloc((unsigned int)(slen + 1));
            _wf_memcpy(part, p, slen);
            part[slen] = '\0';
            list = taida_list_push(list, (int64_t)part);
            break;
        }
        int plen = (int)(found - p);
        char *part = _wasm_str_alloc((unsigned int)(plen + 1));
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
    char *r = _wasm_str_alloc((unsigned int)(new_len + 1));
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
    char *r = _wasm_str_alloc((unsigned int)(new_len + 1));
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
/* E32B-022 (Lock-N): wasm Regex stub — same no-op semantics as
 * `taida_str_search_regex`, surfaced as a has_value=false Lax[Int] so the
 * caller path matches Interpreter / Native / JS shape. */
int64_t taida_str_search_regex_lax(int64_t s_raw, int64_t regex_raw) {
    (void)s_raw; (void)regex_raw;
    return taida_lax_empty(0);
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
    int byte_len = _wf_strlen(s);
    /* Indices are code points (mirrors native taida_str_slice).
       INT64_MIN marks an omitted end ("to the end"); an explicit
       negative end clamps to 0 like the reference. */
    int64_t start = start_raw;
    int64_t end = end_raw;
    if (end == (-9223372036854775807LL - 1)) end = _wasm_utf8_count(s, byte_len);
    if (end < 0) end = 0;
    if (start < 0) start = 0;
    if (start >= end) { return taida_str_alloc(0); }
    int b0 = _wasm_utf8_cp_to_byte(s, byte_len, start);
    int b1 = _wasm_utf8_cp_to_byte(s, byte_len, end);
    if (b0 >= b1) { return taida_str_alloc(0); }
    int slen = b1 - b0;
    char *r = _wasm_str_alloc((unsigned int)(slen + 1));
    _wf_memcpy(r, s + b0, slen);
    r[slen] = '\0';
    return (int64_t)r;
}

/// CharAt[str, index]() -- extract single character (code point) at index
int64_t taida_str_char_at(int64_t s_raw, int64_t idx_raw) {
    const char *s = (const char *)s_raw;
    if (!s || idx_raw < 0) { return taida_str_alloc(0); }
    int byte_len = _wf_strlen(s);
    int off = _wasm_utf8_cp_to_byte(s, byte_len, idx_raw);
    if (off >= byte_len) { return taida_str_alloc(0); }
    int cl = _wasm_utf8_cp_len_at(s, byte_len, off);
    char *r = _wasm_str_alloc((unsigned int)(cl + 1));
    _wf_memcpy(r, s + off, cl);
    r[cl] = '\0';
    return (int64_t)r;
}

/* Strict UTF-8 validator (one scalar): rejects overlongs, surrogates and
   out-of-range scalars — mirrors the native taida_utf8_decode_one and the
   rt_wasi twin. Lives in core so Bytes readers (Utf8Decode) work on every
   profile now that Bytes share the TAIDBYT layout. */
static int _core_utf8_decode_one(const unsigned char *buf, int len, int *consumed, uint32_t *out_cp) {
    if (len <= 0) return 0;
    unsigned char b0 = buf[0];
    if (b0 < 0x80) { *consumed = 1; *out_cp = b0; return 1; }
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

/// Utf8Decode[bytes]() -> Lax[Str]. Invalid UTF-8 -> empty Lax.
/// Reads the shared [TAIDBYT, len, byte...] layout; available on every
/// WASM profile (moved from rt_wasi after the Bytes layout unification).
int64_t taida_utf8_decode_mold(int64_t value) {
    if (!_looks_like_bytes(value)) return taida_lax_empty(taida_str_alloc(0));
    int64_t *bytes = (int64_t *)(intptr_t)value;
    int64_t len = bytes[1];
    unsigned char *raw = (unsigned char *)_wasm_str_alloc((unsigned int)(len > 0 ? len : 1));
    for (int64_t i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];
    int pos = 0;
    while (pos < (int)len) {
        int consumed = 0;
        uint32_t cp = 0;
        if (!_core_utf8_decode_one(raw + pos, (int)len - pos, &consumed, &cp)) {
            return taida_lax_empty(taida_str_alloc(0));
        }
        pos += consumed;
    }
    char *out = (char *)_wasm_str_alloc((unsigned int)(len + 1));
    for (int64_t i = 0; i < len; i++) out[i] = (char)raw[i];
    out[len] = '\0';
    return taida_lax_new((int64_t)(intptr_t)out, taida_str_alloc(0));
}

/* Empty Bytes for Lax defaults (core-local; the public
   taida_bytes_default_value lives in the profile runtimes). */
static int64_t _core_bytes_empty(void) {
    int64_t *bytes = (int64_t *)wasm_alloc(2 * 8);
    if (!bytes) return 0;
    bytes[0] = TAIDA_WASM_BYTES_MAGIC;
    bytes[1] = 0;
    return (int64_t)(intptr_t)bytes;
}

/// Utf8Encode[str]() -> Lax[Bytes]. Core twin of the rt_wasi original:
/// Bytes use the shared TAIDBYT layout, so the constructor is core-safe.
int64_t taida_utf8_encode_mold(int64_t value) {
    const char *s = (const char *)(intptr_t)value;
    if (!s || !_looks_like_string(value)) {
        return taida_lax_empty(_core_bytes_empty());
    }
    int slen = _wf_strlen(s);
    int64_t len = (int64_t)slen;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    if (!bytes) return taida_lax_empty(_core_bytes_empty());
    bytes[0] = TAIDA_WASM_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = (int64_t)(unsigned char)s[i];
    return taida_lax_new((int64_t)(intptr_t)bytes, _core_bytes_empty());
}

/* ── Bytes constructors and codec molds, bitwise / shift molds, Float
   Div / Mod molds ──
   Pure computations on the shared [TAIDBYT, len, byte...] Bytes layout
   and on Int / Float values: no OS access, so every profile provides
   them (wasm-min and wasm-edge used to reject them at build time). */

/* ─── Bytes constructors ─── */

int64_t taida_bytes_default_value(void) {
    return _core_bytes_empty();
}

int64_t taida_bytes_len(int64_t bytes_ptr) {
    if (!_looks_like_bytes(bytes_ptr)) return 0;
    return ((int64_t *)(intptr_t)bytes_ptr)[1];
}

int64_t taida_bytes_new_filled(int64_t len, int64_t fill) {
    if (len < 0) len = 0;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    bytes[0] = TAIDA_WASM_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = fill;
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_from_raw(int64_t ptr, int64_t len) {
    if (len < 0) len = 0;
    const unsigned char *data = (const unsigned char *)(intptr_t)ptr;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    bytes[0] = TAIDA_WASM_BYTES_MAGIC;
    bytes[1] = len;
    for (int64_t i = 0; i < len; i++) bytes[2 + i] = (int64_t)data[i];
    return (int64_t)(intptr_t)bytes;
}

int64_t taida_bytes_clone(int64_t bytes_ptr) {
    if (!_looks_like_bytes(bytes_ptr)) return taida_bytes_default_value();
    int64_t *src = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = src[1];
    int64_t *dst = (int64_t *)wasm_alloc((unsigned int)((2 + len) * 8));
    for (int64_t i = 0; i < 2 + len; i++) dst[i] = src[i];
    return (int64_t)(intptr_t)dst;
}

int64_t taida_bytes_get_lax(int64_t bytes_ptr, int64_t idx) {
    if (!_looks_like_bytes(bytes_ptr)) return taida_lax_empty(0);
    int64_t len = ((int64_t *)(intptr_t)bytes_ptr)[1];
    if (idx < 0 || idx >= len) return taida_lax_empty(0);
    int64_t val = ((int64_t *)(intptr_t)bytes_ptr)[2 + idx];
    return taida_lax_new(val, 0);
}

int64_t taida_bytes_set(int64_t bytes_ptr, int64_t idx, int64_t val) {
    if (!_looks_like_bytes(bytes_ptr)) return taida_lax_empty(taida_bytes_default_value());
    int64_t len = ((int64_t *)(intptr_t)bytes_ptr)[1];
    if (idx < 0 || idx >= len) return taida_lax_empty(taida_bytes_default_value());
    if (val < 0 || val > 255) return taida_lax_empty(taida_bytes_default_value());
    int64_t out = taida_bytes_clone(bytes_ptr);
    ((int64_t *)(intptr_t)out)[2 + idx] = val;
    return taida_lax_new(out, taida_bytes_default_value());
}

int64_t taida_bytes_to_list(int64_t bytes_ptr) {
    int64_t list = taida_list_new();
    if (!_looks_like_bytes(bytes_ptr)) return list;
    int64_t *bytes = (int64_t *)(intptr_t)bytes_ptr;
    int64_t len = bytes[1];
    for (int64_t i = 0; i < len; i++) {
        list = taida_list_push(list, bytes[2 + i]);
    }
    return list;
}

int64_t taida_bytes_to_display_string(int64_t bytes_ptr) {
    if (!_looks_like_bytes(bytes_ptr)) bytes_ptr = _core_bytes_empty();
    return _wasm_bytes_to_display_string(bytes_ptr);
}

/* A Lax over Bytes records the payload's kind in both slots (native twin:
   taida_bytes_lax). */
static int64_t _wasm_bytes_lax(int64_t lax) {
    taida_pack_set_tag(lax, 1, WASM_TAG_BYTES);
    taida_pack_set_tag(lax, 2, WASM_TAG_BYTES);
    return lax;
}

int64_t taida_bytes_mold(int64_t value, int64_t fill) {
    if (_looks_like_bytes(value)) {
        int64_t cloned = taida_bytes_clone(value);
        return _wasm_bytes_lax(taida_lax_new(cloned, taida_bytes_default_value()));
    }
    if (_looks_like_list(value)) {
        int64_t *list = (int64_t *)(intptr_t)value;
        int64_t len = list[1];
        int64_t out = taida_bytes_new_filled(len, 0);
        int64_t *ob = (int64_t *)(intptr_t)out;
        for (int64_t i = 0; i < len; i++) {
            int64_t item = list[WASM_LIST_ELEMS + i];
            if (item < 0 || item > 255) {
                return _wasm_bytes_lax(taida_lax_empty(taida_bytes_default_value()));
            }
            ob[2 + i] = item;
        }
        return _wasm_bytes_lax(taida_lax_new(out, taida_bytes_default_value()));
    }
    if (_wasm_is_string_ptr(value)) {
        const char *s = (const char *)(intptr_t)value;
        int slen = wasm_strlen(s);
        int64_t out = taida_bytes_from_raw((int64_t)(intptr_t)s, (int64_t)slen);
        return _wasm_bytes_lax(taida_lax_new(out, taida_bytes_default_value()));
    }
    int64_t len = value;
    if (len < 0 || len > 10000000) return _wasm_bytes_lax(taida_lax_empty(taida_bytes_default_value()));
    if (fill < 0 || fill > 255) return _wasm_bytes_lax(taida_lax_empty(taida_bytes_default_value()));
    int64_t out = taida_bytes_new_filled(len, (int64_t)(unsigned char)fill);
    return _wasm_bytes_lax(taida_lax_new(out, taida_bytes_default_value()));
}

/* ─── Bytes cursor ─── */
/* Hash constants are FNV-1a of the actual user-facing field names. The
 * code generator's `simple_hash` (`src/codegen/lower/mod.rs`) hashes the
 * field name verbatim, so e.g. `cursor.bytes` hashes "bytes", not
 * "__bytes"; the matching names below keep `step.value`, `step.cursor`,
 * `cursor.bytes`, etc. resolvable from generated wasm code. */
#define WASM_HASH_CURSOR_BYTES   0x2f2ec0474f1c4fe4ULL  /* FNV-1a("bytes") */
#define WASM_HASH_CURSOR_OFFSET  0x0268b0f8129435caULL  /* FNV-1a("offset") */
#define WASM_HASH_CURSOR_LENGTH  0xea11573f1af59eb5ULL  /* FNV-1a("length") */
#define WASM_HASH_STEP_VALUE     0x7ce4fd9430e80ceaULL  /* FNV-1a("value") */
#define WASM_HASH_STEP_CURSOR    0xf927453fbe6252efULL  /* FNV-1a("cursor") */

static int _wasm_bytes_cursor_unpack(int64_t cursor_ptr, int64_t *bytes_out, int64_t *offset_out) {
    if (!_wasm_is_valid_ptr(cursor_ptr, 8)) return 0;
    int64_t *pack = (int64_t *)(intptr_t)cursor_ptr;
    int64_t fc = pack[0];
    if (fc < 2) return 0;
    int64_t bytes_ptr = pack[1 + 0 * 3 + 2];
    int64_t offset = pack[1 + 1 * 3 + 2];
    if (!_looks_like_bytes(bytes_ptr)) return 0;
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    *bytes_out = bytes_ptr;
    *offset_out = offset;
    return 1;
}

static int64_t _wasm_bytes_cursor_step(int64_t value, int64_t cursor) {
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, WASM_HASH_STEP_VALUE);
    taida_pack_set(pack, 0, value);
    taida_pack_set_hash(pack, 1, WASM_HASH_STEP_CURSOR);
    taida_pack_set(pack, 1, cursor);
    return pack;
}

int64_t taida_bytes_cursor_new(int64_t bytes_ptr, int64_t offset) {
    if (!_looks_like_bytes(bytes_ptr)) bytes_ptr = taida_bytes_default_value();
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset < 0) offset = 0;
    if (offset > len) offset = len;
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, WASM_HASH_CURSOR_BYTES);
    taida_pack_set(pack, 0, bytes_ptr);
    taida_pack_set_hash(pack, 1, WASM_HASH_CURSOR_OFFSET);
    taida_pack_set(pack, 1, offset);
    taida_pack_set_hash(pack, 2, WASM_HASH_CURSOR_LENGTH);
    taida_pack_set(pack, 2, len);
    taida_pack_set_hash(pack, 3, WASM_HASH___TYPE);
    taida_pack_set(pack, 3, WSTR("BytesCursor"));
    taida_pack_set_tag(pack, 3, 3 /* STR */);
    return pack;
}

int64_t taida_bytes_cursor_u8(int64_t cursor_ptr) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wasm_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        int64_t empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        return taida_lax_empty(_wasm_bytes_cursor_step(0, empty_cursor));
    }
    int64_t cur = taida_bytes_cursor_new(bytes_ptr, offset);
    int64_t def_step = _wasm_bytes_cursor_step(0, cur);
    int64_t len = taida_bytes_len(bytes_ptr);
    if (offset >= len) return taida_lax_empty(def_step);
    int64_t val = ((int64_t *)(intptr_t)bytes_ptr)[2 + offset];
    int64_t next = taida_bytes_cursor_new(bytes_ptr, offset + 1);
    return taida_lax_new(_wasm_bytes_cursor_step(val, next), def_step);
}

int64_t taida_bytes_cursor_take(int64_t cursor_ptr, int64_t size) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wasm_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        int64_t empty_cursor = taida_bytes_cursor_new(taida_bytes_default_value(), 0);
        return taida_lax_empty(_wasm_bytes_cursor_step(taida_bytes_default_value(), empty_cursor));
    }
    int64_t cur = taida_bytes_cursor_new(bytes_ptr, offset);
    int64_t def_step = _wasm_bytes_cursor_step(taida_bytes_default_value(), cur);
    if (size < 0) return taida_lax_empty(def_step);
    int64_t len = taida_bytes_len(bytes_ptr);
    /* `offset + size` could wrap for a huge size; compare against what
       is left instead. */
    if (offset < 0 || offset > len || size > len - offset) return taida_lax_empty(def_step);
    int64_t *src = (int64_t *)(intptr_t)bytes_ptr;
    int64_t out = taida_bytes_new_filled(size, 0);
    int64_t *dst = (int64_t *)(intptr_t)out;
    for (int64_t i = 0; i < size; i++) dst[2 + i] = src[2 + offset + i];
    int64_t next = taida_bytes_cursor_new(bytes_ptr, offset + size);
    return taida_lax_new(_wasm_bytes_cursor_step(out, next), def_step);
}

int64_t taida_bytes_cursor_step(int64_t cursor_ptr, int64_t n) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wasm_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) {
        return taida_bytes_cursor_new(taida_bytes_default_value(), 0);
    }
    return taida_bytes_cursor_new(bytes_ptr, offset + n);
}

int64_t taida_bytes_cursor_remaining(int64_t cursor_ptr) {
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wasm_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
    return taida_bytes_len(bytes_ptr) - offset;
}

int64_t taida_bytes_cursor_unpack(int64_t cursor_ptr, int64_t schema) {
    (void)schema;
    int64_t bytes_ptr = 0, offset = 0;
    if (!_wasm_bytes_cursor_unpack(cursor_ptr, &bytes_ptr, &offset)) return 0;
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
    if (!_looks_like_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) != 2) return taida_lax_empty(0); /* exactly 2, as native */
    int64_t *b = (int64_t *)(intptr_t)value;
    uint16_t n = (uint16_t)(((unsigned)b[2] << 8) | (unsigned)b[3]);
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u16le_decode_mold(int64_t value) {
    if (!_looks_like_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) != 2) return taida_lax_empty(0); /* exactly 2, as native */
    int64_t *b = (int64_t *)(intptr_t)value;
    uint16_t n = (uint16_t)((unsigned)b[2] | ((unsigned)b[3] << 8));
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u32be_decode_mold(int64_t value) {
    if (!_looks_like_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) != 4) return taida_lax_empty(0); /* exactly 4, as native */
    int64_t *b = (int64_t *)(intptr_t)value;
    uint32_t n = ((uint32_t)b[2] << 24) | ((uint32_t)b[3] << 16) | ((uint32_t)b[4] << 8) | (uint32_t)b[5];
    return taida_lax_new((int64_t)n, 0);
}

int64_t taida_u32le_decode_mold(int64_t value) {
    if (!_looks_like_bytes(value)) return taida_lax_empty(0);
    if (taida_bytes_len(value) != 4) return taida_lax_empty(0); /* exactly 4, as native */
    int64_t *b = (int64_t *)(intptr_t)value;
    uint32_t n = (uint32_t)b[2] | ((uint32_t)b[3] << 8) | ((uint32_t)b[4] << 16) | ((uint32_t)b[5] << 24);
    return taida_lax_new((int64_t)n, 0);
}

static int64_t _wasm_strtol_prefix(const char *s, const char **end) {
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
    if (_wasm_is_string_ptr(v)) {
        const char *s = (const char *)(intptr_t)v;
        const char *cend;
        parsed = _wasm_strtol_prefix(s, &cend);
        if (*cend != '\0') parsed = v;
    }
    if (parsed < 0 || parsed > 255) return taida_lax_empty(0);
    return taida_lax_new(parsed, 0);
}

int64_t taida_uint8_mold_float(int64_t v) {
    double d = _to_double(v);
    if (d != d) return taida_lax_empty(0); /* NaN */
    if (d < 0.0 || d > 255.0) return taida_lax_empty(0);
    double fl = (double)(int64_t)d;
    if (fl != d) return taida_lax_empty(0);
    return taida_lax_new((int64_t)d, 0);
}

/* ─── UTF-8 encode/decode molds ─── */

/* taida_utf8_encode_mold moved to runtime_core_wasm/02_containers.inc.c:
   Bytes use the shared TAIDBYT layout, so the constructor is core-safe and
   the mold is available on every WASM profile. */

/* taida_utf8_decode_mold moved to runtime_core_wasm/02_containers.inc.c:
   it only READS Bytes (shared TAIDBYT layout) and returns Lax[Str], so it
   is available on every WASM profile. */

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
    if (!_looks_like_bytes(v)) return taida_lax_empty(0);
    int64_t *bytes = (int64_t *)(intptr_t)v;
    int64_t len = bytes[1];
    unsigned char raw[4];
    int avail = (int)(len < 4 ? len : 4);
    for (int i = 0; i < avail; i++) raw[i] = (unsigned char)bytes[2 + i];
    int consumed = 0;
    uint32_t cp = 0;
    if (!_core_utf8_decode_one(raw, avail, &consumed, &cp)) return taida_lax_empty(0);
    return taida_lax_new((int64_t)cp, 0);
}

int64_t taida_utf8_single_scalar(int64_t v) {
    /* Decode a Bytes value that must contain exactly one UTF-8 scalar. */
    if (!_looks_like_bytes(v)) return taida_lax_empty(0);
    int64_t *bytes = (int64_t *)(intptr_t)v;
    int64_t len = bytes[1];
    if (len < 1 || len > 4) return taida_lax_empty(0);
    unsigned char raw[4];
    for (int64_t i = 0; i < len; i++) raw[i] = (unsigned char)bytes[2 + i];
    int consumed = 0;
    uint32_t cp = 0;
    if (!_core_utf8_decode_one(raw, (int)len, &consumed, &cp)) return taida_lax_empty(0);
    if (consumed != (int)len) return taida_lax_empty(0);
    return taida_lax_new((int64_t)cp, 0);
}

/* ── Bitwise / shift / Float Div+Mod molds ──
 *
 * `taida_bit_*` / `taida_shift_*`: pure 64-bit ops.
 *
 * `taida_div_mold_f` / `taida_mod_mold_f`: Float div/mod via the
 *   boxed-double bitcast; a zero divisor gives an empty Lax (the native
 *   runtime's taida_div_mold_f contract). The Lax's __value / __default
 *   carry the FLOAT tag (see _float_lax_new) so display reads them as
 *   f64 bit patterns.
 */

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

/* int64 ABI: a_bits, b_bits are bit-patterns of f64 (or small ints
 * widened by the lowering via `taida_int_to_float`).  Returns Lax pack
 * with FLOAT-tagged __value/__default. */
int64_t taida_div_mold_f(int64_t a_bits, int64_t b_bits) {
    double a = _to_double(a_bits);
    double b = _to_double(b_bits);
    if (b == 0.0) return _float_lax_empty();
    return _float_lax_new(_d2l(a / b));
}

/* Exact IEEE-754 fmod.
 *
 * A single-pass `(int64_t)q; a - (double)qi * b` lost
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
 *   - b == 0.0          -> Lax empty
 *   - NaN / Inf inputs  -> see the NaN / Inf block in the body
 *   - |a| < |b|         -> result == a   (preserves sign, including
 *                         signed zero)
 */
static int _wasm_is_finite(double x) {
    /* True iff x is neither NaN nor +/-Inf. NaN: x != x. Inf: |x| ==
     * +Inf, detected by checking the f64 exponent field is not all-1s. */
    union { double d; int64_t l; } u; u.d = x;
    int64_t exp = (u.l >> 52) & 0x7FF;
    return exp != 0x7FF;
}

int64_t taida_mod_mold_f(int64_t a_bits, int64_t b_bits) {
    double a = _to_double(a_bits);
    double b = _to_double(b_bits);
    if (b == 0.0) return _float_lax_empty();
    /* NaN / Inf semantics follow libc fmod (the native runtime's
     * taida_mod_mold_f):
     *
     *  - fmod(NaN, b) = NaN
     *  - fmod(a,   NaN) = NaN
     *  - fmod(±Inf, finite) = NaN  (cannot produce a meaningful remainder)
     *  - fmod(finite, ±Inf) = finite (preserves sign of dividend)
     *
     * The b==0 case stays an empty Lax.
     */
    /* NaN propagation: any NaN input -> NaN-valued Lax. */
    if (a != a || b != b) {
        /* IEEE-754 canonical NaN bit-pattern (`0.0/0.0`). */
        union { double d; int64_t l; } nu;
        nu.d = 0.0; nu.d = nu.d / nu.d; /* generate canonical NaN */
        return _float_lax_new(nu.l);
    }
    int a_finite = _wasm_is_finite(a);
    int b_finite = _wasm_is_finite(b);
    if (!a_finite && !b_finite) {
        /* fmod(±Inf, ±Inf) = NaN */
        union { double d; int64_t l; } nu;
        nu.d = 0.0; nu.d = nu.d / nu.d;
        return _float_lax_new(nu.l);
    }
    if (!a_finite) {
        /* fmod(±Inf, finite) = NaN */
        union { double d; int64_t l; } nu;
        nu.d = 0.0; nu.d = nu.d / nu.d;
        return _float_lax_new(nu.l);
    }
    if (!b_finite) {
        /* fmod(finite, ±Inf) = finite (preserves sign of dividend). */
        return _float_lax_new(a_bits);
    }
    /* Preserve sign of `a` (IEEE 754 fmod sign rule). */
    int sign_a = (a < 0.0) || (a == 0.0 && _d2l(a) < 0);
    double aa = sign_a ? -a : a;
    double bb = (b < 0.0) ? -b : b;
    if (aa < bb) {
        /* |a| < |b|: remainder is a itself (preserves signed zero). */
        return _float_lax_new(_d2l(a));
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
    return _float_lax_new(_d2l(result));
}

/// Chars[str]() -- split into code-point strings (native mirror).
int64_t taida_str_chars(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    int64_t list = taida_list_new();
    taida_list_set_elem_tag(list, 3 /* STR */);
    if (!s) return list;
    int byte_len = _wf_strlen(s);
    int off = 0;
    while (off < byte_len) {
        int cl = _wasm_utf8_cp_len_at(s, byte_len, off);
        if (cl <= 0) cl = 1;
        char *piece = _wasm_str_alloc((unsigned int)(cl + 1));
        _wf_memcpy(piece, s + off, cl);
        piece[cl] = '\0';
        list = taida_list_push(list, (int64_t)(intptr_t)piece);
        off += cl;
    }
    return list;
}

/// Repeat[str, n]() -- repeat string n times
int64_t taida_str_repeat(int64_t s_raw, int64_t n_raw) {
    const char *s = (const char *)s_raw;
    if (!s || n_raw <= 0 || n_raw > TAIDA_WASM_I32_MAX) { return taida_str_alloc(0); }
    int n = (int)n_raw;
    int slen = _wf_strlen(s);
    if (slen == 0) { return taida_str_alloc(0); }
    if (n > (TAIDA_WASM_I32_MAX - 1) / slen) { return taida_str_alloc(0); }
    int total = slen * n;
    char *r = _wasm_str_alloc((unsigned int)(total + 1));
    if (!r) { return taida_str_alloc(0); }
    for (int i = 0; i < n; i++) {
        _wf_memcpy(r + i * slen, s, slen);
    }
    r[total] = '\0';
    return (int64_t)r;
}

// The bytes s[start..end) as UTF-8 text, each maximal invalid sequence
// turned into U+FFFD as Rust's String::from_utf8_lossy does. Writes at
// `out` when not NULL; returns the byte length of the text.
static int64_t taida_utf8_lossy_copy(const unsigned char *s, int64_t start, int64_t end, unsigned char *out) {
    int64_t o = 0;
    int64_t i = start;
    while (i < end) {
        unsigned char c = s[i];
        int64_t need = c < 0x80 ? 1 : (c >= 0xC2 && c <= 0xDF) ? 2 : (c >= 0xE0 && c <= 0xEF) ? 3
                     : (c >= 0xF0 && c <= 0xF4) ? 4 : 0;
        int64_t k = 0;
        while (need > 1 && k < need - 1 && i + 1 + k < end) {
            unsigned char cc = s[i + 1 + k];
            unsigned char lo = 0x80, hi = 0xBF;
            if (k == 0) {
                if (c == 0xE0) lo = 0xA0;
                else if (c == 0xED) hi = 0x9F;
                else if (c == 0xF0) lo = 0x90;
                else if (c == 0xF4) hi = 0x8F;
            }
            if (cc < lo || cc > hi) break;
            k++;
        }
        if (need == 1 || (need > 1 && k == need - 1)) {
            if (out) {
                for (int64_t j = 0; j < need; j++) out[o + j] = s[i + j];
            }
            o += need;
            i += need;
        } else {
            if (out) {
                out[o] = 0xEF;
                out[o + 1] = 0xBF;
                out[o + 2] = 0xBD;
            }
            o += 3;
            i += 1 + k;
        }
    }
    return o;
}

/* The bytes ByteLength / ByteAt / ByteSlice read: a Str's UTF-8 bytes, or
   the bytes of a Bytes value (kept one per slot, so copied out first). */
static const unsigned char *_wasm_byte_mold_bytes(int64_t v, int64_t *len) {
    if (v == 0) {
        *len = 0;
        return (const unsigned char *)"";
    }
    if (_looks_like_bytes(v)) {
        int64_t n = _wasm_bytes_len(v);
        unsigned char *buf = (unsigned char *)wasm_alloc((unsigned int)(n > 0 ? n : 1));
        for (int64_t i = 0; i < n; i++) buf[i] = _wasm_bytes_at(v, i);
        *len = n;
        return buf;
    }
    const unsigned char *s = (const unsigned char *)(intptr_t)v;
    *len = wasm_strlen((const char *)s);
    return s;
}

/// ByteLength[x]() -- a Str's UTF-8 byte count, or a Bytes value's length.
int64_t taida_str_byte_length(int64_t s_raw) {
    if (s_raw != 0 && _looks_like_bytes(s_raw)) return _wasm_bytes_len(s_raw);
    const char *s = (const char *)(intptr_t)s_raw;
    return s ? (int64_t)wasm_strlen(s) : 0;
}

/// Byte at `idx`, or -1 out of range (the lowering pairs it with the Lax
/// form below).
int64_t taida_str_byte_at(int64_t s_raw, int64_t idx) {
    if (s_raw != 0 && _looks_like_bytes(s_raw)) {
        if (idx < 0 || idx >= _wasm_bytes_len(s_raw)) return -1;
        return (int64_t)_wasm_bytes_at(s_raw, idx);
    }
    const char *s = (const char *)(intptr_t)s_raw;
    if (!s) return -1;
    int64_t len = wasm_strlen(s);
    if (idx < 0 || idx >= len) return -1;
    return (int64_t)(unsigned char)s[idx];
}

/// ByteAt[x, idx]() -> Lax[Int]
int64_t taida_str_byte_at_lax(int64_t s_raw, int64_t idx, int64_t default_value) {
    int64_t b = taida_str_byte_at(s_raw, idx);
    if (b < 0) return taida_lax_empty(default_value);
    return taida_lax_new(b, default_value);
}

/// ByteSlice[x, start, end]() -- the bytes [start, end) of a Str or Bytes
/// (clamped, swapped when reversed) as a Str. A cut through a UTF-8
/// sequence turns each maximal invalid subsequence into U+FFFD, like the
/// reference's `String::from_utf8_lossy`.
int64_t taida_str_byte_slice(int64_t s_raw, int64_t start, int64_t end) {
    int64_t len = 0;
    const unsigned char *s = _wasm_byte_mold_bytes(s_raw, &len);
    if (start < 0) start = 0;
    if (start > len) start = len;
    if (end < 0) end = 0;
    if (end > len) end = len;
    if (end < start) { int64_t t = start; start = end; end = t; }
    int64_t n = taida_utf8_lossy_copy(s, start, end, (unsigned char *)0);
    char *out = _wasm_str_alloc((unsigned int)(n + 1));
    taida_utf8_lossy_copy(s, start, end, (unsigned char *)out);
    out[n] = '\0';
    return (int64_t)(intptr_t)out;
}

/// StringRepeatJoin[str, n, sep]() -- n copies of str joined by sep;
/// "" for n <= 0 or a result past the Str length limit.
int64_t taida_str_repeat_join(int64_t s_raw, int64_t n, int64_t sep_raw) {
    const char *s = (const char *)(intptr_t)s_raw;
    const char *sep = (const char *)(intptr_t)sep_raw;
    if (n <= 0 || n > TAIDA_WASM_I32_MAX) return taida_str_alloc(0);
    if (!s) s = "";
    if (!sep) sep = "";
    int64_t slen = wasm_strlen(s), seplen = wasm_strlen(sep);
    int64_t total = slen * n + seplen * (n - 1);
    if (total > TAIDA_WASM_I32_MAX - 1) return taida_str_alloc(0);
    char *r = _wasm_str_alloc((unsigned int)(total + 1));
    if (!r) return taida_str_alloc(0);
    int64_t pos = 0;
    for (int64_t i = 0; i < n; i++) {
        if (i > 0) { _wf_memcpy(r + pos, sep, (int)seplen); pos += seplen; }
        _wf_memcpy(r + pos, s, (int)slen);
        pos += slen;
    }
    r[total] = '\0';
    return (int64_t)(intptr_t)r;
}

/// Reverse[str]() -- reverse characters
int64_t taida_str_reverse(int64_t s_raw) {
    const char *s = (const char *)s_raw;
    if (!s) { return taida_str_alloc(0); }
    /* Reverse code points, not bytes (mirrors native). */
    int byte_len = _wf_strlen(s);
    char *r = _wasm_str_alloc((unsigned int)(byte_len + 1));
    int w = byte_len;
    int i = 0;
    while (i < byte_len) {
        int cl = _wasm_utf8_cp_len_at(s, byte_len, i);
        w -= cl;
        _wf_memcpy(r + w, s + i, cl);
        i += cl;
    }
    r[byte_len] = '\0';
    return (int64_t)r;
}

/// Pad[str, target_len](padChar, padEnd) -- pad string to target length
int64_t taida_str_pad(int64_t s_raw, int64_t target_len_raw, int64_t pad_char_raw, int64_t pad_end_raw) {
    const char *s = (const char *)s_raw;
    const char *pad_char = (const char *)pad_char_raw;
    int pad_end = (int)pad_end_raw;
    if (!s) { return taida_str_alloc(0); }
    /* measure characters (code points), not bytes, and repeat the
     * whole pad string -- matching the interpreter's char-based behavior. */
    int sbytes = _wf_strlen(s);
    int slen = _wasm_utf8_count(s, sbytes);
    if (slen >= target_len_raw || target_len_raw > TAIDA_WASM_I32_MAX - 1) {
        return taida_str_new_copy(s_raw);
    }
    const char *pc = (pad_char && pad_char[0] != '\0') ? pad_char : " ";
    int pc_bytes = _wf_strlen(pc);
    int pad_count = (int)(target_len_raw - slen);
    if (pad_count > (TAIDA_WASM_I32_MAX - 1 - sbytes) / pc_bytes) {
        return taida_str_new_copy(s_raw);
    }
    int pad_bytes = pad_count * pc_bytes;
    char *r = _wasm_str_alloc((unsigned int)(sbytes + pad_bytes + 1));
    if (!r) { return taida_str_new_copy(s_raw); }
    if (pad_end) {
        _wf_memcpy(r, s, sbytes);
        for (int i = 0; i < pad_count; i++) _wf_memcpy(r + sbytes + i * pc_bytes, pc, pc_bytes);
    } else {
        for (int i = 0; i < pad_count; i++) _wf_memcpy(r + i * pc_bytes, pc, pc_bytes);
        _wf_memcpy(r + pad_bytes, s, sbytes);
    }
    r[sbytes + pad_bytes] = '\0';
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

/// str.get(index) -- get character (code point) at index as Lax[Str]
static int64_t _wasm_str_lax_tagged(int64_t lax) {
    /* STR tags on value/default so the Lax pack display renders the
       slots as strings instead of pointer numbers. */
    taida_pack_set_tag(lax, 1, WASM_TAG_STR);
    taida_pack_set_tag(lax, 2, WASM_TAG_STR);
    return lax;
}
int64_t taida_str_get(int64_t s_raw, int64_t idx_raw) {
    const char *s = (const char *)s_raw;
    if (!s || idx_raw < 0) return _wasm_str_lax_tagged(taida_lax_empty(WSTR("")));
    int byte_len = _wf_strlen(s);
    int off = _wasm_utf8_cp_to_byte(s, byte_len, idx_raw);
    if (off >= byte_len) return _wasm_str_lax_tagged(taida_lax_empty(WSTR("")));
    int cl = _wasm_utf8_cp_len_at(s, byte_len, off);
    char *r = _wasm_str_alloc((unsigned int)(cl + 1));
    _wf_memcpy(r, s + off, cl);
    r[cl] = '\0';
    return _wasm_str_lax_tagged(taida_lax_new((int64_t)r, WSTR("")));
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

/* F62B-017: lexicographic ordering for Str operands of < / > / >= (native
   mirror). UTF-8 byte order equals code-point order. */
int64_t taida_str_lt(int64_t a_raw, int64_t b_raw) {
    return _wf_strcmp((const char *)a_raw ? (const char *)a_raw : "",
                      (const char *)b_raw ? (const char *)b_raw : "") < 0 ? 1 : 0;
}
int64_t taida_str_gt(int64_t a_raw, int64_t b_raw) {
    return _wf_strcmp((const char *)a_raw ? (const char *)a_raw : "",
                      (const char *)b_raw ? (const char *)b_raw : "") > 0 ? 1 : 0;
}
int64_t taida_str_gte(int64_t a_raw, int64_t b_raw) {
    return _wf_strcmp((const char *)a_raw ? (const char *)a_raw : "",
                      (const char *)b_raw ? (const char *)b_raw : "") >= 0 ? 1 : 0;
}

/// Slice mold -- polymorphic slice for Str, List, Bytes
int64_t taida_slice_mold(int64_t value, int64_t start_raw, int64_t end_raw) {
    if (_looks_like_list(value)) {
        int64_t *list = (int64_t *)(intptr_t)value;
        int64_t len = list[1];
        int64_t s = start_raw;
        int64_t e = (end_raw == (-9223372036854775807LL - 1)) ? len : end_raw;
        if (s < 0) s = 0;
        if (s > len) s = len;
        if (e < 0) e = 0;
        if (e > len) e = len;
        if (e < s) e = s;
        int64_t out = taida_list_new();
        for (int64_t i = s; i < e; i++) {
            _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)out,
                                         _wasm_elem_kind_at(list, i));
            out = taida_list_push(out, list[WASM_LIST_ELEMS + i]);
        }
        return out;
    }
    if (_looks_like_bytes(value)) {
        /* Bytes slice by byte position, bounds as for a list (native twin). */
        int64_t len = taida_bytes_len(value);
        int64_t s = start_raw;
        int64_t e = (end_raw == (-9223372036854775807LL - 1)) ? len : end_raw;
        if (s < 0) s = 0;
        if (s > len) s = len;
        if (e < 0) e = 0;
        if (e > len) e = len;
        if (e < s) e = s;
        int64_t *src = (int64_t *)(intptr_t)value;
        int64_t out = taida_bytes_new_filled(e - s, 0);
        int64_t *dst = (int64_t *)(intptr_t)out;
        for (int64_t i = s; i < e; i++) dst[2 + i - s] = src[2 + i];
        return out;
    }
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
    char *out = _wasm_str_alloc((unsigned int)(out_len + 1));
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
