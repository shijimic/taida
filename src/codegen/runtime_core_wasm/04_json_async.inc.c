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

/* C23B-005 / C23B-006 (2026-04-22): these detectors delegate to the
   `01_core.inc.c` implementations (`_is_wasm_hashmap` / `_is_wasm_set`)
   which enforce the dual-magic positive identification. Keeping the
   `_wc_*` wrappers makes it easy to audit all JSON / async call sites
   that relied on the old single-marker heuristic. */
static int _is_wasm_hashmap(int64_t ptr);
static int _is_wasm_set(int64_t ptr);

static int _wc_is_hashmap(int64_t ptr) {
    return _is_wasm_hashmap(ptr);
}

static int _wc_is_set(int64_t ptr) {
    return _is_wasm_set(ptr);
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
        /* C23B-008 (2026-04-22): insertion-order walk via the new
           side-index so JSON output matches interpreter / JS ordering.
           `WASM_HM_ORD_HEADER_SLOT` / `WASM_HM_ORD_SLOT` are defined in
           01_core.inc.c alongside `WASM_HM_HEADER`. */
        int64_t next_ord = hm[WASM_HM_ORD_HEADER_SLOT(cap)];
        for (int64_t oi = 0; oi < next_ord; oi++) {
            int64_t slot = hm[WASM_HM_ORD_SLOT(cap, oi)];
            if (slot < 0 || slot >= cap) continue;
            int64_t sh = hm[WASM_HM_HEADER + slot * 3];
            int64_t sk = hm[WASM_HM_HEADER + slot * 3 + 1];
            if (sh != 0 && !(sh == 1 && sk == 0)) {
                if (count > 0) _wc_jb_append_char(jb, ',');
                if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
                const char *key_str = (const char *)(intptr_t)sk;
                if (!key_str) key_str = "";
                _wc_jb_append_escaped_str(jb, key_str);
                _wc_jb_append_char(jb, ':');
                if (indent > 0) _wc_jb_append_char(jb, ' ');
                _wc_json_serialize_typed(jb, hm[WASM_HM_HEADER + slot * 3 + 2], indent, depth + 1, 0);
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
