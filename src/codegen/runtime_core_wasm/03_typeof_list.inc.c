/* ── RC no-ops (wasm-min ではヒープなし) ── */

void taida_retain(int64_t val) { (void)val; }
void taida_release(int64_t val) { (void)val; }
void taida_str_retain(int64_t val) { (void)val; }

/* ── typeof: compile-time tag + runtime heuristic ── */

int64_t taida_typeof(int64_t val, int64_t tag) {
    if (val != 0 && val >= WASM_MIN_HEAP_ADDR) {
        if (_is_wasm_hashmap(val)) return WSTR("HashMap");
        if (_is_wasm_set(val)) return WSTR("Set");
        if (_wasm_is_result(val)) return WSTR("Result");
        if (_wasm_is_lax(val)) return WSTR("Lax");
        if (_looks_like_pack(val)) return WSTR("BuchiPack");
        if (_looks_like_list(val)) return WSTR("List");
        if (_looks_like_string(val)) return WSTR("Str");
    }
    switch (tag) {
        case 1: return WSTR("Float");
        case 2: return WSTR("Bool");
        case 3: return WSTR("Str");
        case 4: return WSTR("BuchiPack");
        case 5: return WSTR("List");
        case 6: return WSTR("Function");
        default: return WSTR("Int");
    }
}

int64_t taida_type_name(int64_t val, int64_t tag) {
    if (val != 0 && val >= WASM_MIN_HEAP_ADDR && _looks_like_pack(val)) {
        if (taida_pack_has_hash(val, WASM_HASH___TYPE)) {
            int64_t type_name = taida_pack_get(val, WASM_HASH___TYPE);
            if (type_name != 0) return type_name;
        }
        /* Plain packs intentionally have no class-like identity. If a future
           pack metadata field becomes public identity, add it before this return. */
        return WSTR("");
    }
    return taida_typeof(val, tag);
}

/* =========================================================================
 * WC-2a: Float mold functions (prelude — all profiles)
 * ========================================================================= */

/* The f64.floor / f64.ceil / f64.trunc opcodes: exact for every double,
   including magnitudes past the Int range, infinities and NaN (a round
   trip through `long long` is not). */
double __builtin_floor(double);
double __builtin_ceil(double);
double __builtin_trunc(double);

/// Floor[f]() -- floor(x), returns float (bit-punned int64_t)
int64_t taida_float_floor(int64_t a) {
    return _d2l(__builtin_floor(_to_double(a)));
}

/// Ceil[f]() -- ceil(x), returns float (bit-punned int64_t)
int64_t taida_float_ceil(int64_t a) {
    return _d2l(__builtin_ceil(_to_double(a)));
}

/// Round[f]() -- round(x) to nearest, ties away from zero. Exact for every
/// input (adding 0.5 first rounded 0.49999999999999994 up to 1).
int64_t taida_float_round(int64_t a) {
    return _d2l(_tm_round(_to_double(a)));
}

/// Abs[f]() -- absolute value of float
int64_t taida_float_abs(int64_t a) {
    /* Clear the sign bit: `-0.0 < 0.0` is false, and Abs[-0.0]() is 0.0. */
    return a & 0x7fffffffffffffffLL;
}

/// Clamp[f, lo, hi]() -- clamp float to range [lo, hi]
int64_t taida_float_clamp(int64_t a, int64_t lo, int64_t hi) {
    double da = _to_double(a);
    double dlo = _to_double(lo);
    double dhi = _to_double(hi);
    /* Reversed bounds (lo > hi) give lo, like the reference. */
    if (dlo > dhi || da < dlo) return lo;
    if (da > dhi) return hi;
    return a;
}

// ── Math mold family ─────────────────────────────────────
//
// The molds run on the math kernel (05_numeric.inc.c), which returns the
// reference libm's results: bit-identical for Exp / Ln / Log2 / Log10 /
// Log / Pow / Sinh / Cosh / Tanh, and the correctly rounded value for the
// trigonometric family (the reference is correctly rounded except on a
// small fraction of arguments). Sqrt is the exact `f64.sqrt` instruction.

double __builtin_sqrt(double);

int64_t taida_float_sqrt(int64_t a) {
    double d = _to_double(a);
    return _d2l(__builtin_sqrt(d));
}

int64_t taida_float_exp(int64_t a) { return _d2l(_crm_exp(_to_double(a))); }
int64_t taida_float_ln(int64_t a) { return _d2l(_crm_ln(_to_double(a))); }
int64_t taida_float_log2(int64_t a) { return _d2l(_crm_log2(_to_double(a))); }
int64_t taida_float_log10(int64_t a) { return _d2l(_crm_log10(_to_double(a))); }
int64_t taida_float_log(int64_t a, int64_t b) {
    return _d2l(_crm_ln(_to_double(a)) / _crm_ln(_to_double(b)));
}
int64_t taida_float_pow(int64_t a, int64_t b) {
    return _d2l(_crm_pow(_to_double(a), _to_double(b)));
}
int64_t taida_float_sin(int64_t a) { return _d2l(_crm_sin(_to_double(a))); }
int64_t taida_float_cos(int64_t a) { return _d2l(_crm_cos(_to_double(a))); }
int64_t taida_float_tan(int64_t a) { return _d2l(_crm_tan(_to_double(a))); }
int64_t taida_float_asin(int64_t a) { return _d2l(_crm_asin(_to_double(a))); }
int64_t taida_float_acos(int64_t a) { return _d2l(_crm_acos(_to_double(a))); }
int64_t taida_float_atan(int64_t a) { return _d2l(_crm_atan(_to_double(a))); }
int64_t taida_float_atan2(int64_t y_raw, int64_t x_raw) {
    return _d2l(_crm_atan2(_to_double(y_raw), _to_double(x_raw)));
}
int64_t taida_float_sinh(int64_t a) { return _d2l(_crm_sinh(_to_double(a))); }
int64_t taida_float_cosh(int64_t a) { return _d2l(_crm_cosh(_to_double(a))); }
int64_t taida_float_tanh(int64_t a) { return _d2l(_crm_tanh(_to_double(a))); }

/// ToFixed[f, digits]() -- Rust `format!("{:.digits$}", f)`: the exact
/// binary value rounded half-to-even, digits clamped to 0..=20.
int64_t taida_float_to_fixed(int64_t a, int64_t digits_raw) {
    int digits = digits_raw < 0 ? 0 : (digits_raw > 20 ? 20 : (int)digits_raw);
    char tmp[TAIDA_WASM_FLOAT_TEXT_MAX];
    int len = _tf_fixed(_to_double(a), digits, tmp);
    char *r = _wasm_str_alloc((unsigned int)(len + 1));
    if (!r) return 0;
    _wf_memcpy(r, tmp, len);
    r[len] = '\0';
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
    if (lo > hi || a < lo) return lo;
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

/// Int[str, base]() -- parse string in given base (native twin): one
/// optional sign, then digits of the base; a value past the Int range fails
/// and the smallest Int parses.
int64_t taida_int_mold_str_base(int64_t v, int64_t base) {
    if (base < 2 || base > 36) return taida_lax_empty(0);
    const char *s = (const char *)(intptr_t)v;
    if (!s || s[0] == '\0') return taida_lax_empty(0);
    int len = _wf_strlen(s);

    int negative = 0;
    int i = 0;
    if (s[0] == '-' || s[0] == '+') {
        negative = s[0] == '-';
        i = 1;
        if (len == 1) return taida_lax_empty(0);
    }

    uint64_t acc = 0;
    const uint64_t i64_max = 0x7fffffffffffffffULL;
    uint64_t limit = negative ? i64_max + 1ULL : i64_max;
    for (; i < len; i++) {
        int d = _wc_char_to_digit((unsigned char)s[i]);
        if (d < 0 || d >= (int)base) return taida_lax_empty(0);
        if (acc > (limit - (uint64_t)d) / (uint64_t)base) return taida_lax_empty(0);
        acc = acc * (uint64_t)base + (uint64_t)d;
    }

    int64_t out;
    if (negative) {
        out = acc == i64_max + 1ULL ? (int64_t)(i64_max + 1ULL) : -(int64_t)acc;
    } else {
        out = (int64_t)acc;
    }
    return taida_lax_new(out, 0);
}

/// ToRadix[value, base]() -- convert int to string in given base
int64_t taida_to_radix(int64_t value, int64_t base) {
    /* Lax[Str]: STR tags on value / default so display and jsonEncode
       render the digits and the "" default as strings, not addresses. */
    if (base < 2 || base > 36) return _wasm_str_lax_tagged(taida_lax_empty(WSTR("")));
    if (value == 0) {
        char *out = _wasm_str_alloc(2);
        out[0] = '0';
        out[1] = '\0';
        return _wasm_str_lax_tagged(taida_lax_new((int64_t)out, WSTR("")));
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

    char *out = _wasm_str_alloc((unsigned int)(pos + 1));
    for (int i = 0; i < pos; i++) {
        out[i] = tmp[pos - 1 - i];
    }
    out[pos] = '\0';
    return _wasm_str_lax_tagged(taida_lax_new((int64_t)out, WSTR("")));
}

/* ── WC-3d: Public callback invoke helpers (list HOF depends on these) ── */
/* These wrap the same logic as _wasm_invoke_callback1 (static, used by Cage)
   but with public linkage so runtime_full_wasm.c can also call them. */

int64_t taida_invoke_callback1(int64_t fn_ptr, int64_t arg0) {
    /* A pending throw ends the runtime loop that is calling back: no
       further user code runs before the ceiling. */
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

int64_t taida_invoke_callback2(int64_t fn_ptr, int64_t arg0, int64_t arg1) {
    if (__wasm_error_thrown) return 0;
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

/* Kind-supplying map (native mirror): codegen passes the callback's
   statically known return kind; UNKNOWN leaves the result kindless. */
int64_t taida_list_map_k(int64_t list_ptr, int64_t fn_ptr, int64_t ret_ekind) {
    int64_t result = taida_list_map(list_ptr, fn_ptr);
    uint32_t k = (uint32_t)ret_ekind & 0xFFu;
    if (k != WASM_EKIND_UNKNOWN) {
        int64_t *r = (int64_t *)(intptr_t)result;
        if (!_wasm_elem_slot_is_array(r[2])) r[2] = (int64_t)k;
    }
    return result;
}

int64_t taida_list_filter(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) {
            if (src_tagged) {
                new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
            } else {
                new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
            }
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
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        if (taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) {
            if (src_tagged) {
                new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
            } else {
                new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
            }
        } else {
            break;
        }
    }
    return new_list;
}

int64_t taida_list_drop_while(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    int64_t dropping = 1;
    for (int64_t i = 0; i < len; i++) {
        if (dropping && taida_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i])) {
            continue;
        }
        dropping = 0;
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
        } else {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
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

/* Ordering for Sort[] (native twin: taida_sort_gt): FLOAT kinds
   compare as f64, strings by byte content via their positive header,
   Int<->Float pairs cross over. Raw i64 order made negative-Float
   order invert and turned Sort[] on a Str list into pointer order. */
static int _wasm_sort_gt(int64_t a, uint32_t eka, int64_t b, uint32_t ekb) {
    int a_f = WASM_EKIND_KIND(eka) == WASM_TAG_FLOAT;
    int b_f = WASM_EKIND_KIND(ekb) == WASM_TAG_FLOAT;
    if (a_f || b_f) {
        double da, db;
        if (a_f) __builtin_memcpy(&da, &a, sizeof(double)); else da = (double)a;
        if (b_f) __builtin_memcpy(&db, &b, sizeof(double)); else db = (double)b;
        return da > db;
    }
    if (_wasm_is_string_ptr(a) && _wasm_is_string_ptr(b))
        return _wf_strcmp((const char *)(intptr_t)a, (const char *)(intptr_t)b) > 0;
    return a > b;
}

/* One element being sorted (native twin: taida_sort_entry): the value and
   its kind, and the key it is ordered by with the key's kind. */
typedef struct {
    int64_t item;
    int64_t key;
    uint32_t ek;
    uint32_t kek;
} _wasm_sort_entry;

static int _wasm_sort_key_is_nan(const _wasm_sort_entry *e) {
    if (WASM_EKIND_KIND(e->kek) != WASM_TAG_FLOAT) return 0;
    double d;
    __builtin_memcpy(&d, &e->key, sizeof(double));
    return d != d;
}

/* Sort[list](by, reverse), the reference's steps (native twin:
   taida_list_sort_kind): NaN keys go last in their original order in either
   direction; the rest are merge-sorted ascending (stable) and reversed for a
   descending sort. `key_fn` 0 orders by the elements themselves; otherwise
   `key_tag` is the kind of the keys it returns (-1 when unknown). */
int64_t taida_list_sort_kind(int64_t list_ptr, int64_t key_fn, int64_t key_tag, int64_t descending) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    if (len == 0) return new_list;
    uint32_t homog_ek = elem_tag >= 0 ? WASM_EKIND((uint32_t)elem_tag, 0) : WASM_EKIND_UNKNOWN;
    uint32_t key_ek = key_tag >= 0 ? WASM_EKIND((uint32_t)key_tag, 0) : WASM_EKIND_UNKNOWN;
    unsigned int bytes = (unsigned int)(len * sizeof(_wasm_sort_entry));
    _wasm_sort_entry *entries = (_wasm_sort_entry *)wasm_alloc(bytes);
    _wasm_sort_entry *tmp = (_wasm_sort_entry *)wasm_alloc(bytes);
    _wasm_sort_entry *nan_tail = (_wasm_sort_entry *)wasm_alloc(bytes);
    int64_t n_keep = 0, n_nan = 0;
    for (int64_t i = 0; i < len; i++) {
        _wasm_sort_entry e;
        e.item = list[WASM_LIST_ELEMS + i];
        e.ek = src_tagged ? _wasm_elem_kind_at(list, i) : homog_ek;
        if (key_fn) {
            e.key = taida_invoke_callback1(key_fn, e.item);
            e.kek = key_ek;
        } else {
            e.key = e.item;
            e.kek = e.ek;
        }
        if (_wasm_sort_key_is_nan(&e)) nan_tail[n_nan++] = e;
        else entries[n_keep++] = e;
    }
    /* Bottom-up merge sort (stable). */
    _wasm_sort_entry *src = entries, *dst = tmp;
    for (int64_t width = 1; width < n_keep; width *= 2) {
        for (int64_t lo = 0; lo < n_keep; lo += 2 * width) {
            int64_t mid = lo + width < n_keep ? lo + width : n_keep;
            int64_t hi = lo + 2 * width < n_keep ? lo + 2 * width : n_keep;
            int64_t i = lo, j = mid, k = lo;
            while (i < mid && j < hi) {
                if (_wasm_sort_gt(src[i].key, src[i].kek, src[j].key, src[j].kek)) dst[k++] = src[j++];
                else dst[k++] = src[i++];
            }
            while (i < mid) dst[k++] = src[i++];
            while (j < hi) dst[k++] = src[j++];
        }
        _wasm_sort_entry *swap = src;
        src = dst;
        dst = swap;
    }
    if (descending) {
        for (int64_t i = 0, j = n_keep - 1; i < j; i++, j--) {
            _wasm_sort_entry t = src[i];
            src[i] = src[j];
            src[j] = t;
        }
    }
    for (int64_t i = 0; i < n_keep + n_nan; i++) {
        _wasm_sort_entry *e = i < n_keep ? &src[i] : &nan_tail[i - n_keep];
        if (src_tagged) _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_list, e->ek);
        new_list = taida_list_push(new_list, e->item);
    }
    return new_list;
}

int64_t taida_list_sort(int64_t list_ptr) {
    return taida_list_sort_kind(list_ptr, 0, -1, 0);
}

int64_t taida_list_sort_desc(int64_t list_ptr) {
    return taida_list_sort_kind(list_ptr, 0, -1, 1);
}

/* Unique by key extraction function: fn_ptr maps each element to a key,
   then dedup by that key. Matches interpreter's Unique[list](by <= fn).
   E34B-020 (Codex review #16 follow-up): close the 4-backend parity gap
   where Native / WASM previously dropped the `by` callback. */
int64_t taida_list_unique_by(int64_t list_ptr, int64_t fn_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    if (len == 0) return new_list;
    /* F54B-016 (C2): dedup keys by STRUCTURE (was raw `==`), mirroring the
       interpreter Unique[](by) path: a fingerprint seen-set while keys are
       hashable, switching to a linear struct-eq scan once a non-hashable key
       appears. `seen_keys` retains every emitted key so the fallback scan still
       sees keys added during the hashable phase. Float/Bool key cross-type
       equality stays out of scope here (value-tag limitation, tracked
       separately). seen-set is bump-allocated (no free, matching seen_keys). */
    int64_t *seen_keys = (int64_t *)wasm_alloc((unsigned int)(len * 8));
    int64_t seen_count = 0;
    _wasm_seen seen;
    int use_hash = _wasm_seen_init(&seen, len);
    int fallback = 0;
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        int64_t key = taida_invoke_callback1(fn_ptr, item);
        int dup;
        if (!fallback && use_hash && _wasm_value_hashable(key)) {
            dup = !_wasm_seen_add(&seen, key);
        } else {
            fallback = 1;  /* a non-hashable key appeared: linear from now on */
            dup = 0;
            for (int64_t j = 0; j < seen_count; j++) {
                if (_wasm_value_eq(seen_keys[j], key)) { dup = 1; break; }
            }
        }
        if (!dup) {
            seen_keys[seen_count++] = key;
            if (src_tagged) {
                new_list = _wasm_list_project_push(new_list, item, list, i);
            } else {
                new_list = taida_list_push(new_list, item);
            }
        }
    }
    return new_list;
}

/* Sort by key extraction function: fn_ptr maps each element to a sort key,
   then sort ascending by key. Matches interpreter's Sort[list](by <= fn). */
int64_t taida_list_sort_by(int64_t list_ptr, int64_t fn_ptr) {
    return taida_list_sort_kind(list_ptr, fn_ptr, -1, 0);
}

int64_t taida_list_unique(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (_wasm_elem_slot_is_array(list[2])
        || _wasm_elem_tag_for_propagation(list) == WASM_TAG_FLOAT) {
        /* Kind-aware dedup over (value, recorded kind) pairs (mirror of
           native taida_list_unique). The result rebuilds its own kind
           entries for the surviving elements (and naturally re-homogenises
           when only one kind survives). The hash path engages only when
           every element kind is hashable — unknown kinds fall back
           to the linear pair scan. */
        int64_t new_list = taida_list_new();
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
                int64_t *nl = (int64_t *)(intptr_t)new_list;
                int64_t nlen = nl[1];
                dup = 0;
                for (int64_t j = 0; j < nlen; j++) {
                    if (_wasm_ekind_value_eq(nl[WASM_LIST_ELEMS + j], _wasm_elem_kind_at(nl, j), item, ek)) {
                        dup = 1;
                        break;
                    }
                }
            }
            if (!dup) {
                _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_list, ek);
                new_list = taida_list_push(new_list, item);
            }
        }
        return new_list;
    }
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    int64_t *nl_init = (int64_t *)(intptr_t)new_list;
    nl_init[2] = elem_tag;
    int raw = _wasm_tag_is_plain_number(elem_tag);
    _wasm_seen seen;
    int use_hash = (raw || _wasm_list_all_hashable(list)) && _wasm_seen_init(&seen, len);
    for (int64_t i = 0; i < len; i++) {
        int64_t item = list[WASM_LIST_ELEMS + i];
        int dup;
        if (use_hash) {
            dup = raw ? !_wasm_seen_add_raw(&seen, item) : !_wasm_seen_add(&seen, item);
        } else if (raw) {
            dup = _wasm_list_has_raw(new_list, item);
        } else {
            int64_t *nl = (int64_t *)(intptr_t)new_list;
            int64_t nlen = nl[1];
            dup = 0;
            for (int64_t j = 0; j < nlen; j++) {
                if (_wasm_value_eq(nl[WASM_LIST_ELEMS + j], item)) { dup = 1; break; }
            }
        }
        if (!dup) {
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
            /* Every inner element projects through the latch under its
               recorded kind — a single i==0 stamp can't represent
               cross-tagged sibling sublists, and the latch naturally
               keeps a same-kind result homogeneous (native mirror). */
            for (int64_t j = 0; j < slen; j++) {
                new_list = _wasm_list_project_push(new_list, sub[WASM_LIST_ELEMS + j], sub, j);
            }
        } else {
            /* Non-list element: project under its recorded kind (native
               mirror — always note, UNKNOWN included, so the latch sees
               every push). */
            _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_list,
                                         _wasm_elem_kind_at(list, i));
            new_list = taida_list_push(new_list, item);
        }
    }
    return new_list;
}

int64_t taida_list_reverse(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = len - 1; i >= 0; i--) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
        } else {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
    }
    return new_list;
}

/* Kind-aware element rendering for the list display paths. The list
   records each element's kind, but the display paths used to funnel
   every element through the tag-blind polymorphic heuristics — a Float
   element rendered as its raw f64 bit pattern and a Bool as 0/1,
   because neither payload can be identified from the value alone.
   Every other kind keeps the existing fallback. */
static int64_t _wasm_elem_to_string_kinded(int64_t item, uint32_t ek) {
    uint32_t k = WASM_EKIND_KIND(ek);
    if (k == WASM_TAG_INT) return taida_int_to_str(item);
    if (k == WASM_TAG_FLOAT) return taida_float_to_str(item);
    if (k == WASM_TAG_BOOL) {
        const char *s = item ? "true" : "false";
        unsigned int sl = item ? 4u : 5u;
        char *r = _wasm_str_alloc(sl + 1);
        _wf_memcpy(r, s, (int)(sl + 1));
        return (int64_t)r;
    }
    return taida_polymorphic_to_string(item);
}

int64_t taida_list_join(int64_t list_ptr, int64_t sep_raw) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_str_alloc(0);
    const char *sep = (const char *)(intptr_t)sep_raw;
    if (!sep) sep = "";
    int sep_len = _wf_strlen(sep);

    /* String elements (the dominant case: Split/Map outputs) are
       borrowed in place — materialising a per-element copy through
       polymorphic_to_string just to memcpy it once is pure allocation
       traffic. Non-string elements keep the shared toString path.
       Lengths are scanned once and reused for the copy pass. The
       total runs in 64 bits and oversize results are steered into the
       allocator's loud heap-ceiling trap rather than wrapping the
       32-bit allocation size. */
    const char **strs = (const char **)wasm_alloc((unsigned int)(len * sizeof(const char *)));
    int *lens = (int *)wasm_alloc((unsigned int)(len * sizeof(int)));
    int64_t total = 0;
    for (int64_t i = 0; i < len; i++) {
        int64_t elem = list[WASM_LIST_ELEMS + i];
        /* The element's recorded kind decides; only a Str or an element of
           unknown kind is looked at as a possible string. */
        uint32_t ek = _wasm_elem_kind_at(list, i);
        uint32_t k = WASM_EKIND_KIND(ek);
        if ((k == WASM_TAG_STR || k == WASM_EKIND_UNKNOWN) && _wasm_is_string_ptr(elem)) {
            strs[i] = (const char *)(intptr_t)elem;
        } else {
            strs[i] = (const char *)(intptr_t)_wasm_elem_to_string_kinded(elem, ek);
        }
        lens[i] = _wf_strlen(strs[i]);
        total += lens[i];
        if (i > 0) total += sep_len;
    }
    if (total + 1 > 0x7FFF0000LL) total = 0x7FFF0000LL; /* trap in the allocator */

    char *r = _wasm_str_alloc((unsigned int)(total + 1));
    char *dst = r;
    for (int64_t i = 0; i < len; i++) {
        if (i > 0 && sep_len > 0) { _wf_memcpy(dst, sep, sep_len); dst += sep_len; }
        _wf_memcpy(dst, strs[i], lens[i]);
        dst += lens[i];
    }
    *dst = '\0';
    return (int64_t)r;
}

int64_t taida_list_concat(int64_t list1, int64_t list2) {
    int64_t *l1 = (int64_t *)(intptr_t)list1;
    int64_t *l2 = (int64_t *)(intptr_t)list2;
    int64_t len1 = l1[1], len2 = l2[1];
    int src_tagged = _wasm_elem_slot_is_array(l1[2]) || _wasm_elem_slot_is_array(l2[2])
                  || _wasm_elem_tags_cross(l1, l2);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(l1);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = 0; i < len1; i++) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, l1[WASM_LIST_ELEMS + i], l1, i);
        } else {
            new_list = taida_list_push(new_list, l1[WASM_LIST_ELEMS + i]);
        }
    }
    for (int64_t i = 0; i < len2; i++) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, l2[WASM_LIST_ELEMS + i], l2, i);
        } else {
            new_list = taida_list_push(new_list, l2[WASM_LIST_ELEMS + i]);
        }
    }
    return new_list;
}

/* Record the kind of a value about to be pushed by an append (native
   twin: taida_list_note_appended_kind). Array carriers note the entry
   verbatim. Homogeneous-tag lists only act in one case: an EMPTY list
   with no established tag adopts the item's kind — this is how
   `Append[@[], 1.5]()` and the tail-recursive `build(n, @[])` pattern
   earn a Float tag; without it the result list stayed kindless and
   Float/Bool elements displayed as raw bits. A non-empty list keeps
   its tag untouched (the checker guarantees homogeneity; a kindless
   non-empty list has unknown provenance). Enum kinds keep the legacy
   untagged state — the homogeneous slot cannot carry the type-id aux. */
static void _wasm_list_note_appended_kind(int64_t *nl, uint32_t item_ek) {
    if (_wasm_elem_slot_is_array(nl[2])) {
        _wasm_elem_tags_note_push_ek(nl, item_ek);
        return;
    }
    uint32_t k = WASM_EKIND_KIND(item_ek);
    if (k == WASM_EKIND_UNKNOWN) return;
    if (nl[1] == 0 && nl[2] == WASM_TAG_UNKNOWN && k != WASM_TAG_ENUM) {
        nl[2] = (int64_t)k;
    }
}

/* Kind-supplying append: codegen passes the appended item's statically
   known kind (same encoding as taida_list_map_k). UNKNOWN leaves every
   tag exactly as the legacy entry point did. */
int64_t taida_list_append_k(int64_t list_ptr, int64_t item, int64_t item_ek) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = 0; i < len; i++) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
        } else {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
    }
    _wasm_list_note_appended_kind((int64_t *)(intptr_t)new_list, (uint32_t)item_ek);
    new_list = taida_list_push(new_list, item);
    return new_list;
}

int64_t taida_list_append(int64_t list_ptr, int64_t item) {
    return taida_list_append_k(list_ptr, item, WASM_EKIND_UNKNOWN);
}

/* Consume-variant Append — see the native twin for the ownership
   contract (owned=0 detaches via the copy variant; owned=1 pushes in
   place, the lowering having proven no other reference exists). */
int64_t taida_list_append_consume_k(int64_t list_ptr, int64_t item, int64_t item_ek,
                                    int64_t owned) {
    if (!owned) return taida_list_append_k(list_ptr, item, item_ek);
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    _wasm_list_note_appended_kind(list, (uint32_t)item_ek);
    return taida_list_push(list_ptr, item);
}

int64_t taida_list_append_consume(int64_t list_ptr, int64_t item, int64_t owned) {
    return taida_list_append_consume_k(list_ptr, item, WASM_EKIND_UNKNOWN, owned);
}

int64_t taida_list_prepend(int64_t list_ptr, int64_t item) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    if (src_tagged) _wasm_elem_tags_note_push_ek((int64_t *)(intptr_t)new_list, WASM_EKIND_UNKNOWN);
    new_list = taida_list_push(new_list, item);
    for (int64_t i = 0; i < len; i++) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
        } else {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
    }
    return new_list;
}

int64_t taida_list_take(int64_t list_ptr, int64_t n) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t take_n = n < len ? n : len;
    if (take_n < 0) take_n = 0;
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = 0; i < take_n; i++) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
        } else {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
    }
    return new_list;
}

int64_t taida_list_drop(int64_t list_ptr, int64_t n) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int src_tagged = _wasm_elem_slot_is_array(list[2]);
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t skip = n < len ? n : len;
    if (skip < 0) skip = 0;
    int64_t new_list = taida_list_new();
    if (!src_tagged) ((int64_t *)(intptr_t)new_list)[2] = elem_tag;
    for (int64_t i = skip; i < len; i++) {
        if (src_tagged) {
            new_list = _wasm_list_project_push(new_list, list[WASM_LIST_ELEMS + i], list, i);
        } else {
            new_list = taida_list_push(new_list, list[WASM_LIST_ELEMS + i]);
        }
    }
    return new_list;
}

/* C24-B (2026-04-23): Register zip / enumerate pair-pack field names
   into `_wasm_field_registry` so `_wasm_pack_to_string_full` resolves
   `first` / `second` / `index` / `value` (previously unregistered →
   NULL → every pair rendered as `@()`, which then trapped on the
   recursive full-form walk because the outer list's `elem_type_tag`
   = WASM_TAG_PACK forced the pair through tagged fast-path rendering).
   Idempotent — follows C23B-009's `taida_hashmap_entries` pattern
   (registers inside the helper body rather than at startup because
   the field names are only meaningful on the zip / enumerate path). */
static void _wasm_register_zip_enumerate_field_names(void) {
    taida_register_field_name((int64_t)WASM_HASH_FIRST,  WSTR("first"));
    taida_register_field_name((int64_t)WASM_HASH_SECOND, WSTR("second"));
    taida_register_field_name((int64_t)WASM_HASH_INDEX,  WSTR("index"));
    taida_register_field_name((int64_t)WASM_HASH_VALUE2, WSTR("value"));
}

/// range(start, end) -- @[start, ..., end - 1]; empty when end <= start.
/// More than a million items throws RangeError (the reference's limit,
/// with its item count saturated to the Int range in the message).
int64_t taida_range(int64_t start, int64_t end) {
    int64_t list = taida_list_new();
    taida_list_set_elem_tag(list, WASM_TAG_INT);
    if (end <= start) return list;
    uint64_t len = (uint64_t)end - (uint64_t)start;
    if (len > 1000000ULL) {
        if (len > 9223372036854775807ULL) len = 9223372036854775807ULL;
        int64_t msg = taida_str_concat(
            WSTR("range(start, end) would create "),
            taida_str_concat(taida_int_to_str((int64_t)len),
                             WSTR(" items; maximum is 1000000")));
        return taida_throw(taida_make_error(WSTR("RangeError"), msg));
    }
    for (int64_t v = start; v < end; v++) list = taida_list_push(list, v);
    return list;
}

int64_t taida_list_enumerate(int64_t list_ptr) {
    _wasm_register_zip_enumerate_field_names();
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    /* C24-B: propagate source list's elem_type_tag to the pair's value
       slot so primitives render through the tagged fast-path. The
       `index` slot is always INT (tag 0), no explicit stamping needed
       since `taida_pack_new` zero-initialises tags to INT. */
    int64_t elem_tag = _wasm_elem_tag_for_propagation(list);
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = WASM_TAG_PACK;  /* C24-B: enumerate produces Pack elements */
    for (int64_t i = 0; i < len; i++) {
        int64_t pair = taida_pack_new(2);
        taida_pack_set_hash(pair, 0, (int64_t)WASM_HASH_INDEX);
        taida_pack_set(pair, 0, i);
        taida_pack_set_tag(pair, 0, WASM_TAG_INT);
        taida_pack_set_hash(pair, 1, (int64_t)WASM_HASH_VALUE2);
        taida_pack_set(pair, 1, list[WASM_LIST_ELEMS + i]);
        taida_pack_set_tag(pair, 1, _wasm_ekind_to_pack_tag(_wasm_elem_kind_at(list, i), elem_tag));
        new_list = taida_list_push(new_list, pair);
    }
    return new_list;
}

int64_t taida_list_zip(int64_t list1, int64_t list2) {
    _wasm_register_zip_enumerate_field_names();
    int64_t *l1 = (int64_t *)(intptr_t)list1;
    int64_t *l2 = (int64_t *)(intptr_t)list2;
    int64_t len1 = l1[1], len2 = l2[1];
    int64_t min_len = len1 < len2 ? len1 : len2;
    /* C24-B: propagate each source list's elem_type_tag to its pair
       slot so primitives in either position render through tagged
       fast-path dispatch. */
    int64_t elem_tag1 = _wasm_elem_tag_for_propagation(l1);
    int64_t elem_tag2 = _wasm_elem_tag_for_propagation(l2);
    int64_t new_list = taida_list_new();
    int64_t *nl = (int64_t *)(intptr_t)new_list;
    nl[2] = WASM_TAG_PACK;  /* C24-B: zip produces Pack elements */
    for (int64_t i = 0; i < min_len; i++) {
        int64_t pair = taida_pack_new(2);
        taida_pack_set_hash(pair, 0, (int64_t)WASM_HASH_FIRST);
        taida_pack_set(pair, 0, l1[WASM_LIST_ELEMS + i]);
        taida_pack_set_tag(pair, 0, _wasm_ekind_to_pack_tag(_wasm_elem_kind_at(l1, i), elem_tag1));
        taida_pack_set_hash(pair, 1, (int64_t)WASM_HASH_SECOND);
        taida_pack_set(pair, 1, l2[WASM_LIST_ELEMS + i]);
        taida_pack_set_tag(pair, 1, _wasm_ekind_to_pack_tag(_wasm_elem_kind_at(l2, i), elem_tag2));
        new_list = taida_list_push(new_list, pair);
    }
    return new_list;
}

int64_t taida_list_to_display_string(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) {
        char *result = _wasm_str_alloc(4);
        _wf_memcpy(result, "@[]", 4);
        return (int64_t)result;
    }
    /* Build "@[elem, elem, ...]" — the total runs in 64 bits and an
       oversize result is steered into the allocator's loud
       heap-ceiling trap rather than wrapping the 32-bit size. */
    const char **strs = (const char **)wasm_alloc((unsigned int)(len * sizeof(const char *)));
    int64_t total = 3; /* "@[" + "]" */
    for (int64_t i = 0; i < len; i++) {
        strs[i] = (const char *)(intptr_t)_wasm_elem_to_string_kinded(
            list[WASM_LIST_ELEMS + i], _wasm_elem_kind_at(list, i));
        total += _wf_strlen(strs[i]);
        if (i > 0) total += 2; /* ", " */
    }
    if (total + 1 > 0x7FFF0000LL) total = 0x7FFF0000LL; /* trap in the allocator */
    char *r = _wasm_str_alloc((unsigned int)(total + 1));
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
    if (len == 0) return _wasm_list_lax(list, 0, 0, WASM_EKIND_UNKNOWN);
    return _wasm_list_lax(list, 1, list[WASM_LIST_ELEMS], _wasm_elem_kind_at(list, 0));
}

int64_t taida_list_last(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return _wasm_list_lax(list, 0, 0, WASM_EKIND_UNKNOWN);
    return _wasm_list_lax(list, 1, list[WASM_LIST_ELEMS + len - 1], _wasm_elem_kind_at(list, len - 1));
}

/* min/max order with the same kind-aware comparator as Sort[] and
   carry the winner's kind into the Lax (native twin has details). */
int64_t taida_list_min(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_lax_empty(0);
    int64_t min_val = list[WASM_LIST_ELEMS];
    uint32_t min_ek = _wasm_elem_kind_at(list, 0);
    for (int64_t i = 1; i < len; i++) {
        uint32_t ek = _wasm_elem_kind_at(list, i);
        if (_wasm_sort_gt(min_val, min_ek, list[WASM_LIST_ELEMS + i], ek)) {
            min_val = list[WASM_LIST_ELEMS + i];
            min_ek = ek;
        }
    }
    return _wasm_lax_new_k(min_val, 0, min_ek);
}

int64_t taida_list_max(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    if (len == 0) return taida_lax_empty(0);
    int64_t max_val = list[WASM_LIST_ELEMS];
    uint32_t max_ek = _wasm_elem_kind_at(list, 0);
    for (int64_t i = 1; i < len; i++) {
        uint32_t ek = _wasm_elem_kind_at(list, i);
        if (_wasm_sort_gt(list[WASM_LIST_ELEMS + i], ek, max_val, max_ek)) {
            max_val = list[WASM_LIST_ELEMS + i];
            max_ek = ek;
        }
    }
    return _wasm_lax_new_k(max_val, 0, max_ek);
}

int64_t taida_list_sum(int64_t list_ptr) {
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    /* A Float element's payload is its f64 bit pattern — a raw i64 +=
       would add the bit patterns as garbage integers (native twin has
       the same split). Any FLOAT-kind element switches the whole sum
       to f64; Int-only lists keep the exact i64 accumulation. */
    int has_float = 0;
    for (int64_t i = 0; i < len; i++) {
        if (WASM_EKIND_KIND(_wasm_elem_kind_at(list, i)) == WASM_TAG_FLOAT) {
            has_float = 1;
            break;
        }
    }
    if (has_float) {
        double dsum = 0.0;
        for (int64_t i = 0; i < len; i++) {
            int64_t v = list[WASM_LIST_ELEMS + i];
            if (WASM_EKIND_KIND(_wasm_elem_kind_at(list, i)) == WASM_TAG_FLOAT) {
                double d;
                __builtin_memcpy(&d, &v, sizeof(double));
                dsum += d;
            } else {
                dsum += (double)v;
            }
        }
        int64_t bits;
        __builtin_memcpy(&bits, &dsum, sizeof(double));
        return bits;
    }
    int64_t sum = 0;
    for (int64_t i = 0; i < len; i++) {
        sum += list[WASM_LIST_ELEMS + i];
    }
    return sum;
}

int64_t taida_list_contains(int64_t list_ptr, int64_t item) {
    /* F56: a sealed carrier is never "contained" (non-equal to everything,
       including the same pointer) — matching the interpreter and taida_list_index_of
       above, closing the `@[a].contains(a)` identity oracle. */
    if (_wasm_carrier_kind(item)) return 0;
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        /* Structural equality (native twin: taida_list_contains): raw `==`
           only matched by bit pattern, so a computed string or an equal
           pack/list was never "contained" — unlike `==` / setOf / interp. */
        if (_wasm_value_eq(list[WASM_LIST_ELEMS + i], item)) return 1;
    }
    return 0;
}

int64_t taida_list_index_of(int64_t list_ptr, int64_t item) {
    /* F56: a sealed carrier is never "found" (non-equal to everything, including
       the same pointer) — matching the interpreter and avoiding an identity-vs-
       value parity split via the raw `==` below. */
    if (_wasm_carrier_kind(item)) return -1;
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = 0; i < len; i++) {
        /* Structural equality — see taida_list_contains. */
        if (_wasm_value_eq(list[WASM_LIST_ELEMS + i], item)) return i;
    }
    return -1;
}

int64_t taida_list_last_index_of(int64_t list_ptr, int64_t item) {
    if (_wasm_carrier_kind(item)) return -1; /* F56: see taida_list_index_of. */
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    for (int64_t i = len - 1; i >= 0; i--) {
        /* Structural equality — see taida_list_contains. */
        if (_wasm_value_eq(list[WASM_LIST_ELEMS + i], item)) return i;
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
