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
    char *buf = _wasm_str_alloc((unsigned int)(total + 1));
    int pos = 0;
    if (neg) buf[pos++] = '-';
    for (int i = len - 1; i >= 0; i--) buf[pos++] = tmp[i];
    buf[pos] = '\0';
    return buf;
}

/* ── Helper: double to string ── */
static char *_wc_double_to_str(double val) {
    if (val != val) {
        char *r = _wasm_str_alloc(4); r[0]='N'; r[1]='a'; r[2]='N'; r[3]='\0'; return r;
    }
    if (val > 1e18 || val < -1e18) {
        int neg = val < 0;
        if (neg) val = -val;
        int exp = 0;
        double v = val;
        while (v >= 10.0) { v /= 10.0; exp++; }
        char *buf = _wasm_str_alloc(32);
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
    char *buf = _wasm_str_alloc((unsigned int)(ilen + 18));
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

/* Delegate to the 01_core positive detectors (same pattern as
   _wc_is_hashmap/_wc_is_set below): the old local copies kept the
   pre-magic structural heuristics — a bare `cap/len` shape test for
   lists and a printable-byte test that accepted ANY pointer to a NUL
   byte as a string — long after every container and string carried a
   magic. An untagged Int whose value lands in the data segment could
   round-trip JSON as the wrong type through these. */
static int _looks_like_list(int64_t ptr);
static int _looks_like_string(int64_t val);

static int _wc_looks_like_list(int64_t ptr) {
    return _looks_like_list(ptr);
}

static int _wc_looks_like_string(int64_t val) {
    return _looks_like_string(val);
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
    if ((uint64_t)addr + min_bytes > (uint64_t)mem_size) return 0; /* no wrap-around */
    return 1;
}

static int _wc_is_lax(int64_t val) {
    if (!_wc_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if ((p[0] == 4 || p[0] == 5) && p[1] == WASM_HASH_HAS_VALUE) return 1;
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

/* The reader accepts exactly the documents the interpreter's reader
   (serde_json) accepts and decodes them to the same values — the same
   rules as the native runtime's JSON reader:
     - strict grammar: no trailing or missing `,` / `:`, no unclosed
       containers or strings, no leading zeros, `+`, bare `.` or hex
       numbers, no raw control characters or unknown escapes in strings,
       nothing but whitespace after the document;
     - `\uXXXX` escapes are decoded (a surrogate pair becomes one code
       point, a lone surrogate is an error); raw string bytes must be
       UTF-8;
     - integers keep the full Int range; fractions, exponents, `-0` and
       integers beyond the Int range become Floats computed the way
       serde_json computes them; a number past the Float range is an
       error;
     - at most _WC_JSON_MAX_NESTING containers nest;
     - a repeated object key keeps its last value.
   Every read is bounded by the end of the input. */

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
    int str_len;
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
    int key_len;
    wc_json_val value;
};

struct wc_json_obj {
    wc_json_obj_entry *entries;
    int count;
    int cap;
};

/* serde_json's recursion limit: the 128th nested container fails. */
#define _WC_JSON_MAX_NESTING 127
/* Integer limits (the freestanding build has no <stdint.h> limit macros). */
#define _WC_I32_MAX ((int32_t)TAIDA_WASM_I32_MAX)
#define _WC_I32_MIN (-_WC_I32_MAX - 1)
#define _WC_U64_MAX 0xFFFFFFFFFFFFFFFFULL

typedef struct {
    const unsigned char *p;
    const unsigned char *end;
    int nesting;
} _wc_json_reader;

/* Forward declarations */
static int _wc_json_parse_value(_wc_json_reader *r, wc_json_val *out);

static void _wc_json_val_clear(wc_json_val *v) {
    v->type = WC_JSON_NULL;
    v->int_val = 0;
    v->float_val = 0.0;
    v->str_val = (char *)0;
    v->str_len = 0;
    v->arr = (wc_json_array *)0;
    v->obj = (wc_json_obj *)0;
}

static void _wc_json_skip_ws(_wc_json_reader *r) {
    while (r->p < r->end
           && (*r->p == ' ' || *r->p == '\t' || *r->p == '\n' || *r->p == '\r')) {
        r->p++;
    }
}

static int _wc_json_at(_wc_json_reader *r, unsigned char c) {
    return r->p < r->end && *r->p == c;
}

static int _wc_json_at_digit(_wc_json_reader *r) {
    return r->p < r->end && *r->p >= '0' && *r->p <= '9';
}

/* Four hex digits of a `\u` escape, all before `limit`. */
static int _wc_json_read_hex4(_wc_json_reader *r, const unsigned char *limit, uint32_t *out) {
    if (limit - r->p < 4) return 0;
    uint32_t v = 0;
    for (int i = 0; i < 4; i++) {
        unsigned char c = r->p[i];
        uint32_t d;
        if (c >= '0' && c <= '9') d = (uint32_t)(c - '0');
        else if (c >= 'a' && c <= 'f') d = (uint32_t)(c - 'a' + 10);
        else if (c >= 'A' && c <= 'F') d = (uint32_t)(c - 'A' + 10);
        else return 0;
        v = (v << 4) | d;
    }
    r->p += 4;
    *out = v;
    return 1;
}

static int _wc_json_put_utf8(char *buf, int out, uint32_t cp) {
    if (cp < 0x80) {
        buf[out++] = (char)cp;
    } else if (cp < 0x800) {
        buf[out++] = (char)(0xC0 | (cp >> 6));
        buf[out++] = (char)(0x80 | (cp & 0x3F));
    } else if (cp < 0x10000) {
        buf[out++] = (char)(0xE0 | (cp >> 12));
        buf[out++] = (char)(0x80 | ((cp >> 6) & 0x3F));
        buf[out++] = (char)(0x80 | (cp & 0x3F));
    } else {
        buf[out++] = (char)(0xF0 | (cp >> 18));
        buf[out++] = (char)(0x80 | ((cp >> 12) & 0x3F));
        buf[out++] = (char)(0x80 | ((cp >> 6) & 0x3F));
        buf[out++] = (char)(0x80 | (cp & 0x3F));
    }
    return out;
}

/* Length of the well-formed UTF-8 sequence that starts with a non-ASCII
   byte at `p` and ends before `limit`, or 0 when it is malformed. */
static int _wc_json_utf8_seq_len(const unsigned char *p, const unsigned char *limit) {
    unsigned char c = p[0];
    int n;
    if (c >= 0xC2 && c <= 0xDF) n = 2;
    else if (c >= 0xE0 && c <= 0xEF) n = 3;
    else if (c >= 0xF0 && c <= 0xF4) n = 4;
    else return 0;
    if (limit - p < n) return 0;
    for (int i = 1; i < n; i++) {
        if ((p[i] & 0xC0) != 0x80) return 0;
    }
    if (c == 0xE0 && p[1] < 0xA0) return 0;
    if (c == 0xED && p[1] >= 0xA0) return 0;
    if (c == 0xF0 && p[1] < 0x90) return 0;
    if (c == 0xF4 && p[1] >= 0x90) return 0;
    return n;
}

/* Reads the string whose opening quote is at r->p into a Str buffer and
   stores its decoded length, or returns NULL on malformed input. */
static char *_wc_json_read_string(_wc_json_reader *r, int *out_len) {
    if (!_wc_json_at(r, '"')) return (char *)0;
    r->p++;
    /* Find the closing quote first (an escape always skips the byte after
       the backslash) to size the buffer: no escape decodes to more bytes
       than it occupies in the input. */
    const unsigned char *close = r->p;
    while (close < r->end && *close != '"') {
        if (*close == '\\') {
            close++;
            if (close >= r->end) break;
        }
        close++;
    }
    if (close >= r->end) return (char *)0;
    int64_t raw_len = (int64_t)(close - r->p);
    if (raw_len >= TAIDA_WASM_I32_MAX) return (char *)0;
    char *buf = _wasm_str_alloc((unsigned int)(raw_len + 1));
    int out = 0;
    while (r->p < close) {
        unsigned char c = *r->p;
        if (c == '\\') {
            r->p++;
            unsigned char e = *r->p++;
            switch (e) {
                case '"': buf[out++] = '"'; break;
                case '\\': buf[out++] = '\\'; break;
                case '/': buf[out++] = '/'; break;
                case 'b': buf[out++] = '\b'; break;
                case 'f': buf[out++] = '\f'; break;
                case 'n': buf[out++] = '\n'; break;
                case 'r': buf[out++] = '\r'; break;
                case 't': buf[out++] = '\t'; break;
                case 'u': {
                    uint32_t cp;
                    if (!_wc_json_read_hex4(r, close, &cp)) return (char *)0;
                    if (cp >= 0xDC00 && cp <= 0xDFFF) return (char *)0;
                    if (cp >= 0xD800 && cp <= 0xDBFF) {
                        uint32_t lo;
                        if (close - r->p < 2 || r->p[0] != '\\' || r->p[1] != 'u') return (char *)0;
                        r->p += 2;
                        if (!_wc_json_read_hex4(r, close, &lo)) return (char *)0;
                        if (lo < 0xDC00 || lo > 0xDFFF) return (char *)0;
                        cp = (((cp - 0xD800) << 10) | (lo - 0xDC00)) + 0x10000;
                    }
                    out = _wc_json_put_utf8(buf, out, cp);
                    break;
                }
                default:
                    return (char *)0;
            }
        } else if (c < 0x20) {
            return (char *)0;
        } else if (c < 0x80) {
            buf[out++] = (char)c;
            r->p++;
        } else {
            int n = _wc_json_utf8_seq_len(r->p, close);
            if (n == 0) return (char *)0;
            _wf_memcpy(buf + out, r->p, n);
            out += n;
            r->p += n;
        }
    }
    r->p = close + 1;  /* closing quote */
    buf[out] = '\0';
    if (out_len) *out_len = out;
    return buf;
}

/* Powers of ten used to scale a decimal significand, as serde_json does. */
static const double _wc_json_pow10[309] = {
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9,
    1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19,
    1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28, 1e29,
    1e30, 1e31, 1e32, 1e33, 1e34, 1e35, 1e36, 1e37, 1e38, 1e39,
    1e40, 1e41, 1e42, 1e43, 1e44, 1e45, 1e46, 1e47, 1e48, 1e49,
    1e50, 1e51, 1e52, 1e53, 1e54, 1e55, 1e56, 1e57, 1e58, 1e59,
    1e60, 1e61, 1e62, 1e63, 1e64, 1e65, 1e66, 1e67, 1e68, 1e69,
    1e70, 1e71, 1e72, 1e73, 1e74, 1e75, 1e76, 1e77, 1e78, 1e79,
    1e80, 1e81, 1e82, 1e83, 1e84, 1e85, 1e86, 1e87, 1e88, 1e89,
    1e90, 1e91, 1e92, 1e93, 1e94, 1e95, 1e96, 1e97, 1e98, 1e99,
    1e100, 1e101, 1e102, 1e103, 1e104, 1e105, 1e106, 1e107, 1e108, 1e109,
    1e110, 1e111, 1e112, 1e113, 1e114, 1e115, 1e116, 1e117, 1e118, 1e119,
    1e120, 1e121, 1e122, 1e123, 1e124, 1e125, 1e126, 1e127, 1e128, 1e129,
    1e130, 1e131, 1e132, 1e133, 1e134, 1e135, 1e136, 1e137, 1e138, 1e139,
    1e140, 1e141, 1e142, 1e143, 1e144, 1e145, 1e146, 1e147, 1e148, 1e149,
    1e150, 1e151, 1e152, 1e153, 1e154, 1e155, 1e156, 1e157, 1e158, 1e159,
    1e160, 1e161, 1e162, 1e163, 1e164, 1e165, 1e166, 1e167, 1e168, 1e169,
    1e170, 1e171, 1e172, 1e173, 1e174, 1e175, 1e176, 1e177, 1e178, 1e179,
    1e180, 1e181, 1e182, 1e183, 1e184, 1e185, 1e186, 1e187, 1e188, 1e189,
    1e190, 1e191, 1e192, 1e193, 1e194, 1e195, 1e196, 1e197, 1e198, 1e199,
    1e200, 1e201, 1e202, 1e203, 1e204, 1e205, 1e206, 1e207, 1e208, 1e209,
    1e210, 1e211, 1e212, 1e213, 1e214, 1e215, 1e216, 1e217, 1e218, 1e219,
    1e220, 1e221, 1e222, 1e223, 1e224, 1e225, 1e226, 1e227, 1e228, 1e229,
    1e230, 1e231, 1e232, 1e233, 1e234, 1e235, 1e236, 1e237, 1e238, 1e239,
    1e240, 1e241, 1e242, 1e243, 1e244, 1e245, 1e246, 1e247, 1e248, 1e249,
    1e250, 1e251, 1e252, 1e253, 1e254, 1e255, 1e256, 1e257, 1e258, 1e259,
    1e260, 1e261, 1e262, 1e263, 1e264, 1e265, 1e266, 1e267, 1e268, 1e269,
    1e270, 1e271, 1e272, 1e273, 1e274, 1e275, 1e276, 1e277, 1e278, 1e279,
    1e280, 1e281, 1e282, 1e283, 1e284, 1e285, 1e286, 1e287, 1e288, 1e289,
    1e290, 1e291, 1e292, 1e293, 1e294, 1e295, 1e296, 1e297, 1e298, 1e299,
    1e300, 1e301, 1e302, 1e303, 1e304, 1e305, 1e306, 1e307, 1e308,
};

/* significand * 10^exponent the way serde_json computes it: exact integer
   significand as f64, then one multiplication or division by a power of
   ten. Fails when the result overflows f64. */
static int _wc_json_f64_from_parts(int positive, uint64_t significand, int32_t exponent, double *out) {
    double f = (double)significand;
    for (;;) {
        uint32_t idx = exponent < 0 ? (uint32_t)0 - (uint32_t)exponent : (uint32_t)exponent;
        if (idx < 309) {
            if (exponent >= 0) {
                f *= _wc_json_pow10[idx];
                if (__builtin_isinf(f)) return 0;
            } else {
                f /= _wc_json_pow10[idx];
            }
            break;
        }
        if (f == 0.0) break;
        if (exponent >= 0) return 0;
        f /= 1e308;
        exponent += 308;
    }
    *out = positive ? f : -f;
    return 1;
}

static int _wc_json_read_exponent(_wc_json_reader *r, int positive, uint64_t significand,
                                  int32_t starting_exp, double *out) {
    r->p++;  /* 'e' / 'E' */
    int positive_exp = 1;
    if (_wc_json_at(r, '+')) {
        r->p++;
    } else if (_wc_json_at(r, '-')) {
        positive_exp = 0;
        r->p++;
    }
    if (!_wc_json_at_digit(r)) return 0;
    int32_t exp = (int32_t)(*r->p++ - '0');
    while (_wc_json_at_digit(r)) {
        int32_t digit = (int32_t)(*r->p - '0');
        if (exp >= _WC_I32_MAX / 10 && (exp > _WC_I32_MAX / 10 || digit > _WC_I32_MAX % 10)) {
            /* The exponent overflows i32: the value is 0 (or out of range). */
            if (significand != 0 && positive_exp) return 0;
            while (_wc_json_at_digit(r)) r->p++;
            *out = positive ? 0.0 : -0.0;
            return 1;
        }
        r->p++;
        exp = exp * 10 + digit;
    }
    int32_t final_exp;
    if (positive_exp) {
        final_exp = starting_exp > _WC_I32_MAX - exp ? _WC_I32_MAX : starting_exp + exp;
    } else {
        final_exp = starting_exp < _WC_I32_MIN + exp ? _WC_I32_MIN : starting_exp - exp;
    }
    return _wc_json_f64_from_parts(positive, significand, final_exp, out);
}

static int _wc_json_read_fraction(_wc_json_reader *r, int positive, uint64_t significand,
                                  int32_t exponent_before, double *out) {
    r->p++;  /* '.' */
    int32_t exponent_after = 0;
    while (_wc_json_at_digit(r)) {
        uint64_t digit = (uint64_t)(*r->p - '0');
        if (significand >= _WC_U64_MAX / 10
            && (significand > _WC_U64_MAX / 10 || digit > _WC_U64_MAX % 10)) {
            /* No room for more digits: the rest of the fraction is dropped. */
            int32_t exponent = exponent_before + exponent_after;
            while (_wc_json_at_digit(r)) r->p++;
            if (_wc_json_at(r, 'e') || _wc_json_at(r, 'E')) {
                return _wc_json_read_exponent(r, positive, significand, exponent, out);
            }
            return _wc_json_f64_from_parts(positive, significand, exponent, out);
        }
        r->p++;
        significand = significand * 10 + digit;
        if (exponent_after > _WC_I32_MIN) exponent_after--;
    }
    if (exponent_after == 0) return 0;  /* `.` needs a digit after it */
    int32_t exponent = exponent_before + exponent_after;
    if (_wc_json_at(r, 'e') || _wc_json_at(r, 'E')) {
        return _wc_json_read_exponent(r, positive, significand, exponent, out);
    }
    return _wc_json_f64_from_parts(positive, significand, exponent, out);
}

static int _wc_json_parse_number(_wc_json_reader *r, wc_json_val *v) {
    int positive = 1;
    if (_wc_json_at(r, '-')) {
        positive = 0;
        r->p++;
    }
    if (!_wc_json_at_digit(r)) return 0;
    uint64_t significand = 0;
    if (*r->p == '0') {
        r->p++;
        if (_wc_json_at_digit(r)) return 0;  /* leading zero */
    } else {
        while (_wc_json_at_digit(r)) {
            uint64_t digit = (uint64_t)(*r->p - '0');
            if (significand >= _WC_U64_MAX / 10
                && (significand > _WC_U64_MAX / 10 || digit > _WC_U64_MAX % 10)) {
                /* Past u64: the remaining integer digits only scale the
                   Float. */
                int32_t exponent = 0;
                while (_wc_json_at_digit(r)) {
                    r->p++;
                    if (exponent < _WC_I32_MAX) exponent++;
                }
                double d;
                int ok;
                if (_wc_json_at(r, '.')) ok = _wc_json_read_fraction(r, positive, significand, exponent, &d);
                else if (_wc_json_at(r, 'e') || _wc_json_at(r, 'E')) ok = _wc_json_read_exponent(r, positive, significand, exponent, &d);
                else ok = _wc_json_f64_from_parts(positive, significand, exponent, &d);
                if (!ok) return 0;
                v->type = WC_JSON_FLOAT;
                v->float_val = d;
                return 1;
            }
            r->p++;
            significand = significand * 10 + digit;
        }
    }
    if (_wc_json_at(r, '.') || _wc_json_at(r, 'e') || _wc_json_at(r, 'E')) {
        double d;
        int ok = _wc_json_at(r, '.')
            ? _wc_json_read_fraction(r, positive, significand, 0, &d)
            : _wc_json_read_exponent(r, positive, significand, 0, &d);
        if (!ok) return 0;
        v->type = WC_JSON_FLOAT;
        v->float_val = d;
        return 1;
    }
    if (positive) {
        if (significand <= (uint64_t)TAIDA_WASM_I64_MAX) {
            v->type = WC_JSON_INT;
            v->int_val = (int64_t)significand;
            v->float_val = (double)v->int_val;
        } else {
            /* Beyond Int: only a Float schema can take it. */
            v->type = WC_JSON_FLOAT;
            v->float_val = (double)significand;
        }
        return 1;
    }
    if (significand > (uint64_t)TAIDA_WASM_I64_MAX + 1 || significand == 0) {
        /* Below the Int range, or `-0`: a Float. */
        v->type = WC_JSON_FLOAT;
        v->float_val = -(double)significand;
        return 1;
    }
    v->type = WC_JSON_INT;
    v->int_val = significand == (uint64_t)TAIDA_WASM_I64_MAX + 1 ? TAIDA_WASM_I64_MIN : -(int64_t)significand;
    v->float_val = (double)v->int_val;
    return 1;
}

static int _wc_json_parse_literal(_wc_json_reader *r, const char *word, int len) {
    if (r->end - r->p < len || _wf_strncmp((const char *)r->p, word, len) != 0) return 0;
    r->p += len;
    return 1;
}

static int _wc_json_parse_array(_wc_json_reader *r, wc_json_val *v) {
    if (r->nesting >= _WC_JSON_MAX_NESTING) return 0;
    r->nesting++;
    r->p++;  /* '[' */
    v->type = WC_JSON_ARRAY;
    v->arr = (wc_json_array *)wasm_alloc(sizeof(wc_json_array));
    v->arr->count = 0;
    v->arr->cap = 4;
    v->arr->items = (wc_json_val *)wasm_alloc((unsigned int)(4 * sizeof(wc_json_val)));
    _wc_json_skip_ws(r);
    if (_wc_json_at(r, ']')) {
        r->p++;
        r->nesting--;
        return 1;
    }
    for (;;) {
        wc_json_val item;
        if (!_wc_json_parse_value(r, &item)) return 0;
        if (v->arr->count >= v->arr->cap) {
            int new_cap = v->arr->cap * 2;
            wc_json_val *new_items = (wc_json_val *)wasm_alloc(
                (unsigned int)(new_cap * sizeof(wc_json_val)));
            for (int i = 0; i < v->arr->count; i++) new_items[i] = v->arr->items[i];
            v->arr->items = new_items;
            v->arr->cap = new_cap;
        }
        v->arr->items[v->arr->count++] = item;
        _wc_json_skip_ws(r);
        if (_wc_json_at(r, ',')) {
            r->p++;
            continue;
        }
        if (_wc_json_at(r, ']')) {
            r->p++;
            break;
        }
        return 0;
    }
    r->nesting--;
    return 1;
}

static int _wc_json_parse_object(_wc_json_reader *r, wc_json_val *v) {
    if (r->nesting >= _WC_JSON_MAX_NESTING) return 0;
    r->nesting++;
    r->p++;  /* '{' */
    v->type = WC_JSON_OBJECT;
    v->obj = (wc_json_obj *)wasm_alloc(sizeof(wc_json_obj));
    v->obj->count = 0;
    v->obj->cap = 8;
    v->obj->entries = (wc_json_obj_entry *)wasm_alloc(
        (unsigned int)(8 * sizeof(wc_json_obj_entry)));
    _wc_json_skip_ws(r);
    if (_wc_json_at(r, '}')) {
        r->p++;
        r->nesting--;
        return 1;
    }
    for (;;) {
        _wc_json_skip_ws(r);
        int key_len = 0;
        char *key = _wc_json_read_string(r, &key_len);
        if (!key) return 0;
        _wc_json_skip_ws(r);
        if (!_wc_json_at(r, ':')) return 0;
        r->p++;
        wc_json_val val;
        if (!_wc_json_parse_value(r, &val)) return 0;
        if (v->obj->count >= v->obj->cap) {
            int new_cap = v->obj->cap * 2;
            wc_json_obj_entry *new_entries = (wc_json_obj_entry *)wasm_alloc(
                (unsigned int)(new_cap * sizeof(wc_json_obj_entry)));
            for (int i = 0; i < v->obj->count; i++) new_entries[i] = v->obj->entries[i];
            v->obj->entries = new_entries;
            v->obj->cap = new_cap;
        }
        v->obj->entries[v->obj->count].key = key;
        v->obj->entries[v->obj->count].key_len = key_len;
        v->obj->entries[v->obj->count].value = val;
        v->obj->count++;
        _wc_json_skip_ws(r);
        if (_wc_json_at(r, ',')) {
            r->p++;
            continue;
        }
        if (_wc_json_at(r, '}')) {
            r->p++;
            break;
        }
        return 0;
    }
    r->nesting--;
    return 1;
}

static int _wc_json_parse_value(_wc_json_reader *r, wc_json_val *out) {
    _wc_json_val_clear(out);
    _wc_json_skip_ws(r);
    if (r->p >= r->end) return 0;
    switch (*r->p) {
        case 'n':
            return _wc_json_parse_literal(r, "null", 4);
        case 't':
            if (!_wc_json_parse_literal(r, "true", 4)) return 0;
            out->type = WC_JSON_BOOL;
            out->int_val = 1;
            return 1;
        case 'f':
            if (!_wc_json_parse_literal(r, "false", 5)) return 0;
            out->type = WC_JSON_BOOL;
            out->int_val = 0;
            return 1;
        case '"': {
            int len = 0;
            char *s = _wc_json_read_string(r, &len);
            if (!s) return 0;
            out->type = WC_JSON_STRING;
            out->str_val = s;
            out->str_len = len;
            return 1;
        }
        case '[':
            return _wc_json_parse_array(r, out);
        case '{':
            return _wc_json_parse_object(r, out);
        default:
            if (*r->p == '-' || (*r->p >= '0' && *r->p <= '9')) {
                return _wc_json_parse_number(r, out);
            }
            return 0;
    }
}

/* A whole document: one value, surrounded by whitespace only. */
static int _wc_json_parse_document(const char *raw, wc_json_val *out) {
    _wc_json_reader r;
    r.p = (const unsigned char *)raw;
    r.end = r.p + _wf_strlen(raw);
    r.nesting = 0;
    if (!_wc_json_parse_value(&r, out)) return 0;
    _wc_json_skip_ws(&r);
    return r.p == r.end;
}

/* ── JSON object field lookup ── */
/* A repeated key keeps its last value, as in the interpreter. */
static wc_json_val *_wc_json_obj_get(wc_json_obj *obj, const char *key) {
    if (!obj) return (wc_json_val *)0;
    int key_len = _wf_strlen(key);
    for (int i = obj->count - 1; i >= 0; i--) {
        wc_json_obj_entry *e = &obj->entries[i];
        if (e->key_len == key_len && _wf_strncmp(e->key, key, key_len) == 0) {
            return &e->value;
        }
    }
    return (wc_json_val *)0;
}

static int64_t _wc_json_to_int(wc_json_val *jv);
static int64_t _wc_json_to_str(wc_json_val *jv);

static int _wc_json_schema_error = 0;
static const char *_wc_json_schema_error_message = (const char *)0;

static void _wc_json_schema_fail(const char *message) {
    if (!_wc_json_schema_error) {
        _wc_json_schema_error = 1;
        _wc_json_schema_error_message = message ? message : "JSON schema decode failed";
    }
}

static int64_t _wc_json_bytes_from_raw(const unsigned char *src, int len) {
    /* Bytes use the shared [TAIDBYT, len, byte...] layout (one int64_t per
       byte) so decoded values are real Bytes for every consumer
       (Utf8Decode / get / length / sha256 / display) — the list-shaped
       value this used to build was only readable by code that confused
       Bytes with @[Int]. */
    if (len < 0) len = 0;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((2 + (int64_t)len) * 8));
    if (!bytes) return 0;
    bytes[0] = TAIDA_WASM_BYTES_MAGIC;
    bytes[1] = len;
    for (int i = 0; i < len; i++) {
        bytes[2 + i] = src ? (int64_t)src[i] : 0;
    }
    return (int64_t)(intptr_t)bytes;
}

static int _wc_b64_value(char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return 26 + (c - 'a');
    if (c >= '0' && c <= '9') return 52 + (c - '0');
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

static int64_t _wc_json_bytes_from_base64_string(const char *src) {
    if (!src) return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
    int raw_len = _wf_strlen(src);
    char *clean = _wasm_str_alloc((unsigned int)(raw_len + 1));
    int clen = 0;
    for (int i = 0; i < raw_len; i++) {
        char c = src[i];
        if (c == ' ' || c == '\n' || c == '\r' || c == '\t') continue;
        if (c != '=' && _wc_b64_value(c) < 0) {
            _wc_json_schema_fail("JSON schema decode failed: invalid base64 character");
            return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
        }
        clean[clen++] = c;
    }
    clean[clen] = '\0';
    if ((clen % 4) != 0) {
        _wc_json_schema_fail("JSON schema decode failed: invalid base64 length");
        return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
    }

    unsigned char *out = (unsigned char *)wasm_alloc((unsigned int)((clen / 4) * 3 + 1));
    int opos = 0;
    for (int i = 0; i < clen; i += 4) {
        char c0 = clean[i];
        char c1 = clean[i + 1];
        char c2 = clean[i + 2];
        char c3 = clean[i + 3];
        if (c0 == '=' || c1 == '=') {
            _wc_json_schema_fail("JSON schema decode failed: invalid base64 padding");
            return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
        }
        int v0 = _wc_b64_value(c0);
        int v1 = _wc_b64_value(c1);
        int v2 = c2 == '=' ? 0 : _wc_b64_value(c2);
        int v3 = c3 == '=' ? 0 : _wc_b64_value(c3);
        if (v0 < 0 || v1 < 0 || v2 < 0 || v3 < 0) {
            _wc_json_schema_fail("JSON schema decode failed: invalid base64 character");
            return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
        }
        if (c2 == '=' && c3 != '=') {
            _wc_json_schema_fail("JSON schema decode failed: invalid base64 padding");
            return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
        }
        if ((c2 == '=' || c3 == '=') && i + 4 != clen) {
            _wc_json_schema_fail("JSON schema decode failed: invalid base64 padding");
            return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
        }
        out[opos++] = (unsigned char)((v0 << 2) | (v1 >> 4));
        if (c2 != '=') out[opos++] = (unsigned char)(((v1 & 15) << 4) | (v2 >> 2));
        if (c3 != '=') out[opos++] = (unsigned char)(((v2 & 3) << 6) | v3);
    }
    return _wc_json_bytes_from_raw(out, opos);
}

static int64_t _wc_json_make_name_value_pair(wc_json_val *jval) {
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, _wc_fnv1a("name", 4));
    taida_pack_set_hash(pack, 1, _wc_fnv1a("value", 5));
    wc_json_val *name_jv = (jval && jval->type == WC_JSON_OBJECT)
        ? _wc_json_obj_get(jval->obj, "name")
        : (wc_json_val *)0;
    wc_json_val *value_jv = (jval && jval->type == WC_JSON_OBJECT)
        ? _wc_json_obj_get(jval->obj, "value")
        : (wc_json_val *)0;
    taida_pack_set(pack, 0, _wc_json_to_str(name_jv));
    taida_pack_set(pack, 1, _wc_json_to_str(value_jv));
    return pack;
}

static int64_t _wc_json_name_value_list(wc_json_val *jval) {
    int64_t list = taida_list_new();
    taida_list_set_elem_tag(list, 4);
    if (jval && jval->type == WC_JSON_ARRAY && jval->arr) {
        for (int i = 0; i < jval->arr->count; i++) {
            list = taida_list_push(list, _wc_json_make_name_value_pair(&jval->arr->items[i]));
        }
    }
    return list;
}

static int64_t _wc_json_apply_web_request(wc_json_val *jval) {
    int64_t pack = taida_pack_new(7);
    taida_pack_set_hash(pack, 0, _wc_fnv1a("method", 6));
    taida_pack_set_hash(pack, 1, _wc_fnv1a("path", 4));
    taida_pack_set_hash(pack, 2, _wc_fnv1a("rawQuery", 8));
    taida_pack_set_hash(pack, 3, _wc_fnv1a("query", 5));
    taida_pack_set_hash(pack, 4, _wc_fnv1a("headers", 7));
    taida_pack_set_hash(pack, 5, _wc_fnv1a("body", 4));
    taida_pack_set_hash(pack, 6, _wc_fnv1a("__type", 6));

    wc_json_obj *obj = (jval && jval->type == WC_JSON_OBJECT) ? jval->obj : (wc_json_obj *)0;
    taida_pack_set(pack, 0, _wc_json_to_str(_wc_json_obj_get(obj, "method")));
    taida_pack_set(pack, 1, _wc_json_to_str(_wc_json_obj_get(obj, "path")));
    taida_pack_set(pack, 2, _wc_json_to_str(_wc_json_obj_get(obj, "rawQuery")));
    taida_pack_set(pack, 3, _wc_json_name_value_list(_wc_json_obj_get(obj, "query")));
    taida_pack_set(pack, 4, _wc_json_name_value_list(_wc_json_obj_get(obj, "headers")));
    wc_json_val *body = _wc_json_obj_get(obj, "bodyBase64");
    taida_pack_set(
        pack,
        5,
        body && body->type == WC_JSON_STRING
            ? _wc_json_bytes_from_base64_string(body->str_val)
            : _wc_json_bytes_from_raw((const unsigned char *)"", 0));
    taida_pack_set(pack, 6, WSTR("WebRequest"));
    return pack;
}

static int64_t _wc_json_apply_web_response(wc_json_val *jval) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, _wc_fnv1a("status", 6));
    taida_pack_set_hash(pack, 1, _wc_fnv1a("headers", 7));
    taida_pack_set_hash(pack, 2, _wc_fnv1a("body", 4));
    taida_pack_set_hash(pack, 3, _wc_fnv1a("__type", 6));

    wc_json_obj *obj = (jval && jval->type == WC_JSON_OBJECT) ? jval->obj : (wc_json_obj *)0;
    taida_pack_set(pack, 0, _wc_json_to_int(_wc_json_obj_get(obj, "status")));
    taida_pack_set(pack, 1, _wc_json_name_value_list(_wc_json_obj_get(obj, "headers")));
    wc_json_val *body = _wc_json_obj_get(obj, "bodyBase64");
    taida_pack_set(
        pack,
        2,
        body && body->type == WC_JSON_STRING
            ? _wc_json_bytes_from_base64_string(body->str_val)
            : _wc_json_bytes_from_raw((const unsigned char *)"", 0));
    taida_pack_set(pack, 3, WSTR("WebResponse"));
    return pack;
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

/* A self-referential type cannot be spelled out in full, so the compiler
   writes `N{Name}` where a type refers back to an enclosing
   `T{Name|...}` (the same scheme as the native runtime). The walkers keep
   a chain of the TypeDefs they are inside; `N{Name}` resolves to the
   nearest one with that name. `defaulting` marks a TypeDef being built
   from its defaults (the value is missing, null or not an object): a
   reference back to it from inside becomes the `@(__type <= Name)` stub,
   like the default of a self-referential type built anywhere else. */
typedef struct _wc_json_schema_frame {
    const char *type_desc;    /* the 'T' that opens the TypeDef */
    const char *name;
    int name_len;
    int defaulting;
    const struct _wc_json_schema_frame *parent;
} _wc_json_schema_frame;

static const _wc_json_schema_frame *_wc_json_schema_resolve_ref(
        const char **desc, const _wc_json_schema_frame *frames) {
    const char *d = *desc;
    if (d[1] != '{') {
        *desc = d + 1;
        return (const _wc_json_schema_frame *)0;
    }
    d += 2;
    const char *name = d;
    while (*d && *d != '}') d++;
    int name_len = (int)(d - name);
    if (*d == '}') d++;
    *desc = d;
    for (const _wc_json_schema_frame *f = frames; f; f = f->parent) {
        if (f->name_len == name_len && _wf_strncmp(f->name, name, name_len) == 0) return f;
    }
    return (const _wc_json_schema_frame *)0;
}

static const char *_wc_json_schema_enter_typedef(const char *d, _wc_json_schema_frame *frame,
                                                 const _wc_json_schema_frame *parent,
                                                 int defaulting) {
    frame->type_desc = d;
    frame->parent = parent;
    frame->defaulting = defaulting;
    d += 2;  /* skip "T{" */
    frame->name = d;
    while (*d && *d != '|' && *d != '}') d++;
    frame->name_len = (int)(d - frame->name);
    if (*d == '|') d++;
    return d;
}

static int64_t _wc_json_type_name_str(const _wc_json_schema_frame *frame) {
    char *s = _wasm_str_alloc((unsigned int)(frame->name_len + 1));
    _wf_memcpy(s, frame->name, frame->name_len);
    s[frame->name_len] = '\0';
    return (int64_t)(intptr_t)s;
}

/* `@(__type <= Name)`: the default of a self-referential type at the
   point where it refers back to itself. */
static int64_t _wc_json_self_ref_stub(const _wc_json_schema_frame *frame) {
    int64_t pack = taida_pack_new(1);
    taida_pack_set_hash(pack, 0, (int64_t)_wc_fnv1a("__type", 6));
    taida_pack_set(pack, 0, _wc_json_type_name_str(frame));
    taida_pack_set_tag(pack, 0, WASM_TAG_STR);
    return pack;
}

/* The rebuild hooks the compiler passes for TypeDefs with methods: a list
   `[TypeName, function, ...]`. The reader only fills in the data fields
   of a pack; the function turns that pack into the full instance
   (`TypeName(...)` with the same fields, methods included). */
static int64_t _wc_json_run_type_hook(int64_t hooks, const _wc_json_schema_frame *frame, int64_t pack) {
    if (!hooks) return pack;
    int64_t *list = (int64_t *)(intptr_t)hooks;
    int64_t len = list[1];
    for (int64_t i = 0; i + 1 < len; i += 2) {
        const char *name = (const char *)(intptr_t)list[WASM_LIST_ELEMS + i];
        if (!name) continue;
        if (_wf_strlen(name) == frame->name_len
            && _wf_strncmp(name, frame->name, frame->name_len) == 0) {
            return taida_invoke_callback1(list[WASM_LIST_ELEMS + i + 1], pack);
        }
    }
    return pack;
}

static void _wc_json_skip_schema_desc(const char **desc) {
    const char *d = *desc;
    if (!d || !*d) return;
    if ((d[0] == 'T' || d[0] == 'L' || d[0] == 'E' || d[0] == 'N') && d[1] == '{') {
        d += 2;
        d += _wc_schema_find_closing_brace(d);
        if (*d == '}') d++;
        *desc = d;
        return;
    }
    *desc = d + 1;
}

static int64_t _wc_json_apply_schema(wc_json_val *jval, const char **desc,
                                     const _wc_json_schema_frame *frames, int64_t hooks,
                                     int64_t *out_tag);

static int64_t _wc_json_default_value_for_desc(const char *desc, int64_t hooks) {
    if (!desc || !*desc) return 0;
    switch (desc[0]) {
        case 'i': return 0;
        case 'f': return _d2l(0.0);
        case 's': return taida_str_alloc(0);
        case 'b': return 0;
        case 'y': return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
        /* F62B-015: defaults are built through the MISSING-value path
           (NULL jval) — a synthesized WC_JSON_NULL would now trip the
           present-but-wrong-kind validation and fail every cast whose
           default is computed after schema application. */
        case 'Q': {
            return _wc_json_apply_web_request((wc_json_val *)0);
        }
        case 'R': {
            return _wc_json_apply_web_response((wc_json_val *)0);
        }
        case 'T': {
            int64_t tag;
            return _wc_json_apply_schema((wc_json_val *)0, &desc, (const _wc_json_schema_frame *)0, hooks, &tag);
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

/* Whether a JSON value has the kinds the schema asks for, like the
   interpreter's json_to_typed_value_checked: a present value of the wrong
   kind (a null list element included) fails the cast, while a missing or
   null object field just takes its default. */
static int _wc_json_matches_schema(wc_json_val *jval, const char **desc,
                                   const _wc_json_schema_frame *frames) {
    const char *d = *desc;
    if (!d || !*d) return 0;
    switch (d[0]) {
        case 'i':
            *desc = d + 1;
            return jval && jval->type == WC_JSON_INT;
        case 'f':
            *desc = d + 1;
            return jval && (jval->type == WC_JSON_INT || jval->type == WC_JSON_FLOAT);
        case 's':
            *desc = d + 1;
            return jval && jval->type == WC_JSON_STRING;
        case 'b':
            *desc = d + 1;
            return jval && jval->type == WC_JSON_BOOL;
        case 'T': {
            if (d[1] != '{') {
                *desc = d + 1;
                return 0;
            }
            _wc_json_schema_frame frame;
            d = _wc_json_schema_enter_typedef(d, &frame, frames, 0);
            int object_ok = (jval && jval->type == WC_JSON_OBJECT);
            int matches = object_ok;
            while (*d && *d != '}') {
                char fname[256];
                int fn_len = 0;
                while (*d && *d != ':' && *d != '}' && fn_len < 255) { fname[fn_len++] = *d; d++; }
                fname[fn_len] = '\0';
                if (*d == ':') d++;
                wc_json_val *field_jval = object_ok ? _wc_json_obj_get(jval->obj, fname) : (wc_json_val *)0;
                if (field_jval && field_jval->type != WC_JSON_NULL) {
                    if (!_wc_json_matches_schema(field_jval, &d, &frame)) matches = 0;
                } else {
                    _wc_json_skip_schema_desc(&d);
                }
                if (*d == ',') d++;
            }
            if (*d == '}') d++;
            *desc = d;
            return matches;
        }
        case 'N': {
            const _wc_json_schema_frame *target = _wc_json_schema_resolve_ref(desc, frames);
            if (!target) return 0;
            const char *type_desc = target->type_desc;
            return _wc_json_matches_schema(jval, &type_desc, frames);
        }
        case 'L': {
            if (d[1] != '{') {
                *desc = d + 1;
                return 0;
            }
            d += 2;
            int inner_len = _wc_schema_find_closing_brace(d);
            int matches = (jval && jval->type == WC_JSON_ARRAY && jval->arr);
            if (matches) {
                for (int i = 0; i < jval->arr->count; i++) {
                    const char *elem_desc = d;
                    if (!_wc_json_matches_schema(&jval->arr->items[i], &elem_desc, frames)) {
                        matches = 0;
                        break;
                    }
                }
            }
            d += inner_len;
            if (*d == '}') d++;
            *desc = d;
            return matches;
        }
        case 'E': {
            /* E{EnumName|Variant1,Variant2,...}: a string naming a variant */
            if (d[1] != '{') {
                *desc = d + 1;
                return 0;
            }
            d += 2;
            while (*d && *d != '|' && *d != '}') d++;
            if (*d == '|') d++;
            int matches = 0;
            int is_string = (jval && jval->type == WC_JSON_STRING && jval->str_val);
            while (*d && *d != '}') {
                const char *vstart = d;
                while (*d && *d != ',' && *d != '}') d++;
                int vlen = (int)(d - vstart);
                if (!matches && is_string && jval->str_len == vlen
                    && _wf_strncmp(jval->str_val, vstart, vlen) == 0) {
                    matches = 1;
                }
                if (*d == ',') d++;
            }
            if (*d == '}') d++;
            *desc = d;
            return matches;
        }
        default:
            /* Bytes / WebRequest / WebResponse report their own decode
               failures through _wc_json_schema_fail. */
            _wc_json_skip_schema_desc(desc);
            return 1;
    }
}

/* ── Apply schema descriptor to JSON value ── */
/* Builds the typed value; `*out_tag` receives its kind tag so containers
   can record it. A missing or null object field takes the type default.
   Whether the value's kinds matched is decided by _wc_json_matches_schema. */
static int64_t _wc_json_apply_schema(wc_json_val *jval, const char **desc,
                                     const _wc_json_schema_frame *frames, int64_t hooks,
                                     int64_t *out_tag) {
    /* WASM_TAG_UNKNOWN: no kind to record (the container keeps its
       unstamped slot, as before kinds were recorded here). */
    *out_tag = WASM_TAG_UNKNOWN;
    if (!desc || !*desc || !**desc) return 0;
    const char *d = *desc;

    switch (d[0]) {
        case 'i': {
            *desc = d + 1;
            *out_tag = WASM_TAG_INT;
            return (jval && jval->type == WC_JSON_INT) ? jval->int_val : 0;
        }
        case 'f': {
            *desc = d + 1;
            *out_tag = WASM_TAG_FLOAT;
            if (jval && jval->type == WC_JSON_INT) return _d2l((double)jval->int_val);
            if (jval && jval->type == WC_JSON_FLOAT) return _d2l(jval->float_val);
            return _d2l(0.0);
        }
        case 's': {
            *desc = d + 1;
            *out_tag = WASM_TAG_STR;
            if (jval && jval->type == WC_JSON_STRING && jval->str_val) {
                return taida_str_new_copy((int64_t)(intptr_t)jval->str_val);
            }
            return taida_str_alloc(0);
        }
        case 'b': {
            *desc = d + 1;
            *out_tag = WASM_TAG_BOOL;
            return (jval && jval->type == WC_JSON_BOOL && jval->int_val) ? 1 : 0;
        }
        case 'y': {
            *desc = d + 1;
            if (!jval || jval->type == WC_JSON_NULL) {
                return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
            }
            if (jval->type != WC_JSON_STRING) {
                _wc_json_schema_fail("JSON schema decode failed: Bytes value must be a base64 string");
                return _wc_json_bytes_from_raw((const unsigned char *)"", 0);
            }
            return _wc_json_bytes_from_base64_string(jval->str_val);
        }
        case 'Q': {
            *desc = d + 1;
            return _wc_json_apply_web_request(jval);
        }
        case 'R': {
            *desc = d + 1;
            return _wc_json_apply_web_response(jval);
        }
        case 'T': {
            if (d[1] != '{') { *desc = d + 1; return 0; }
            int is_object = (jval && jval->type == WC_JSON_OBJECT);
            _wc_json_schema_frame frame;
            d = _wc_json_schema_enter_typedef(d, &frame, frames, !is_object);

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
                if (is_object) {
                    field_jval = _wc_json_obj_get(jval->obj, fname);
                }
                /* Explicit JSON null is field absence (null-exclusion
                   philosophy): take the field default without failing. */
                if (field_jval && field_jval->type == WC_JSON_NULL) {
                    field_jval = (wc_json_val *)0;
                }

                int64_t field_tag;
                int64_t field_val = _wc_json_apply_schema(field_jval, &d, &frame, hooks, &field_tag);
                taida_pack_set(pack, idx, field_val);
                if (field_tag != WASM_TAG_UNKNOWN) taida_pack_set_tag(pack, idx, field_tag);
                idx++;

                if (*d == ',') d++;
            }
            if (*d == '}') d++;

            uint64_t type_hash = _wc_fnv1a("__type", 6);
            taida_pack_set_hash(pack, idx, (int64_t)type_hash);
            taida_pack_set(pack, idx, _wc_json_type_name_str(&frame));
            taida_pack_set_tag(pack, idx, WASM_TAG_STR);

            *desc = d;
            *out_tag = WASM_TAG_PACK;
            return _wc_json_run_type_hook(hooks, &frame, pack);
        }
        case 'N': {
            const _wc_json_schema_frame *target = _wc_json_schema_resolve_ref(desc, frames);
            if (!target) return 0;
            *out_tag = WASM_TAG_PACK;
            if (!jval && target->defaulting) return _wc_json_self_ref_stub(target);
            const char *type_desc = target->type_desc;
            return _wc_json_apply_schema(jval, &type_desc, frames, hooks, out_tag);
        }
        case 'L': {
            *out_tag = WASM_TAG_LIST;
            if (d[1] != '{') { *desc = d + 1; return taida_list_new(); }
            d += 2;
            int inner_len = _wc_schema_find_closing_brace(d);

            int64_t list = taida_list_new();
            if (jval && jval->type == WC_JSON_ARRAY && jval->arr) {
                for (int i = 0; i < jval->arr->count; i++) {
                    const char *elem_desc = d;
                    wc_json_val *item = &jval->arr->items[i];
                    int64_t elem_tag;
                    int64_t elem = _wc_json_apply_schema(
                        item->type == WC_JSON_NULL ? (wc_json_val *)0 : item,
                        &elem_desc, frames, hooks, &elem_tag);
                    if (elem_tag != WASM_TAG_UNKNOWN) taida_list_set_elem_tag(list, elem_tag);
                    list = taida_list_push(list, elem);
                }
            }

            d += inner_len;
            if (*d == '}') d++;
            *desc = d;
            return list;
        }
        case 'E': {
            /* E{EnumName|Variant1,Variant2,...}: a string naming a variant
               becomes its ordinal; anything else is the failed Lax[Enum]
               (value and default 0, the first variant). */
            if (d[1] != '{') {
                *desc = d + 1;
                *out_tag = WASM_TAG_PACK;
                return taida_lax_empty(0);
            }
            d += 2;
            while (*d && *d != '|' && *d != '}') d++;
            if (*d == '|') d++;
            int is_string = (jval && jval->type == WC_JSON_STRING && jval->str_val);
            int matched = 0;
            int64_t ordinal = 0;
            int64_t current = 0;
            while (*d && *d != '}') {
                const char *vstart = d;
                while (*d && *d != ',' && *d != '}') d++;
                int vlen = (int)(d - vstart);
                if (!matched && is_string && jval->str_len == vlen
                    && _wf_strncmp(jval->str_val, vstart, vlen) == 0) {
                    ordinal = current;
                    matched = 1;
                }
                current++;
                if (*d == ',') d++;
            }
            if (*d == '}') d++;
            *desc = d;
            if (matched) {
                *out_tag = WASM_TAG_INT;
                return ordinal;
            }
            *out_tag = WASM_TAG_PACK;
            return taida_lax_empty(0);
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

/* JSON[raw, Schema]() -> Lax[T]. `hooks` is 0 or the rebuild hooks for
   TypeDefs with methods (see _wc_json_run_type_hook). */
int64_t taida_json_schema_cast_hooked(int64_t raw_ptr, int64_t schema_ptr, int64_t hooks) {
    const char *raw = (const char *)(intptr_t)raw_ptr;
    const char *schema = (const char *)(intptr_t)schema_ptr;

    if (!raw || !schema) {
        int64_t def = _wc_json_default_value_for_desc(schema, hooks);
        return taida_lax_empty_error(def, taida_make_error_with_kind(WSTR("JsonError"), WSTR("JSON parse error: missing raw value or schema"), WSTR("parse")));
    }

    wc_json_val jval;
    if (!_wc_json_parse_document(raw, &jval)) {
        /* Every malformed document reports the same error, like the
           interpreter. */
        int64_t def = _wc_json_default_value_for_desc(schema, hooks);
        return taida_lax_empty_error(def, taida_make_error_with_kind(WSTR("JsonError"), WSTR("JSON parse error: invalid input"), WSTR("parse")));
    }

    const char *desc = schema;
    _wc_json_schema_error = 0;
    _wc_json_schema_error_message = (const char *)0;
    int64_t result_tag;
    int64_t result = _wc_json_apply_schema(&jval, &desc, (const _wc_json_schema_frame *)0, hooks, &result_tag);
    const char *match_desc = schema;
    int matches = _wc_json_matches_schema(&jval, &match_desc, (const _wc_json_schema_frame *)0);
    int64_t def = _wc_json_default_value_for_desc(schema, hooks);
    if (_wc_json_schema_error) {
        const char *message = _wc_json_schema_error_message
            ? _wc_json_schema_error_message
            : "JSON schema decode failed";
        /* `message` is a C string: copy it into a Taida Str so it reads as
           text, not as a pointer-valued integer. */
        return taida_lax_empty_error(def, taida_make_error_with_kind(WSTR("JsonError"), taida_str_new_copy((int64_t)(intptr_t)message), WSTR("schema")));
    }
    /* A kind mismatch keeps the typed value (fields that did match keep
       their values), like the interpreter. */
    if (!matches) return taida_lax_empty(result);
    return taida_lax_new(result, def);
}

int64_t taida_json_schema_cast(int64_t raw_ptr, int64_t schema_ptr) {
    return taida_json_schema_cast_hooked(raw_ptr, schema_ptr, 0);
}

/* taida_json_parse: copy raw JSON string */
int64_t taida_json_parse(int64_t str_ptr) {
    const char *src = (const char *)(intptr_t)str_ptr;
    if (!src) src = "{}";
    int len = _wf_strlen(src);
    char *buf = _wasm_str_alloc((unsigned int)(len + 1));
    _wf_memcpy(buf, src, len + 1);
    return (int64_t)(intptr_t)buf;
}

/* taida_json_empty: return "{}" */
int64_t taida_json_empty(void) {
    char *buf = _wasm_str_alloc(3);
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
    char *buf = _wasm_str_alloc((unsigned int)(new_len + 1));
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
    char *buf = _wasm_str_alloc((unsigned int)(len + 1));
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
    char *buf = _wasm_str_alloc((unsigned int)(total + 1));
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
    char *buf = _wasm_str_alloc((unsigned int)(len + 1));
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
    jb->buf = _wasm_str_alloc(jb->cap);
    jb->len = 0;
    if (jb->buf) jb->buf[0] = '\0';
}

static void _wc_jb_ensure(_wc_json_buf *jb, int needed) {
    if (jb->len + needed + 1 > jb->cap) {
        int new_cap = jb->cap;
        while (jb->len + needed + 1 > new_cap) new_cap *= 2;
        char *new_buf = _wasm_str_alloc((unsigned int)new_cap);
        if (!new_buf) return;
        for (int i = 0; i < jb->len; i++) new_buf[i] = jb->buf[i];
        new_buf[jb->len] = '\0';
        jb->buf = new_buf;
        jb->cap = new_cap;
    }
}

/* A finite Float in JSON (native twin: json_format_float): the shortest
   digits that read back as the same double, in plain notation when the
   decimal exponent is in -5..15 ("1.5", "100.0", "0.00015") and as
   "1.5e-7" / "1e+21" otherwise. Writes at most 26 bytes and a NUL. */
static void _wc_json_format_float(double d, char *out) {
    uint64_t b = _tf_bits(d);
    char dg[24];
    int ex = 0, n, e, i, pos = 0;
    if (b >> 63) out[pos++] = '-';
    if ((b << 1) == 0) {
        out[pos++] = '0'; out[pos++] = '.'; out[pos++] = '0'; out[pos] = '\0';
        return;
    }
    n = _tf_shortest(d, dg, &ex); /* |d| = 0.dg * 10^ex */
    e = ex - 1;                   /* |d| = d.g * 10^e */
    if (e >= -5 && e <= 15) {
        if (n - 1 <= e) {
            for (i = 0; i < n; i++) out[pos++] = dg[i];
            for (i = n; i <= e; i++) out[pos++] = '0';
            out[pos++] = '.'; out[pos++] = '0';
        } else if (e >= 0) {
            for (i = 0; i < n; i++) {
                out[pos++] = dg[i];
                if (i == e) out[pos++] = '.';
            }
        } else {
            out[pos++] = '0'; out[pos++] = '.';
            for (i = 1; i < -e; i++) out[pos++] = '0';
            for (i = 0; i < n; i++) out[pos++] = dg[i];
        }
        out[pos] = '\0';
        return;
    }
    out[pos++] = dg[0];
    if (n > 1) {
        out[pos++] = '.';
        for (i = 1; i < n; i++) out[pos++] = dg[i];
    }
    out[pos++] = 'e';
    out[pos++] = e >= 0 ? '+' : '-';
    if (e < 0) e = -e;
    if (e >= 100) out[pos++] = (char)('0' + e / 100);
    if (e >= 10) out[pos++] = (char)('0' + (e / 10) % 10);
    out[pos++] = (char)('0' + e % 10);
    out[pos] = '\0';
}

static void _wc_jb_append(_wc_json_buf *jb, const char *s) {
    int slen = _wf_strlen(s);
    _wc_jb_ensure(jb, slen);
    for (int i = 0; i < slen; i++) jb->buf[jb->len + i] = s[i];
    jb->len += slen;
    jb->buf[jb->len] = '\0';
}

static void _wc_jb_append_len(_wc_json_buf *jb, const char *s, int slen) {
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
            unsigned char c = (unsigned char)*p;
            switch (c) {
                case '"':  _wc_jb_append(jb, "\\\""); break;
                case '\\': _wc_jb_append(jb, "\\\\"); break;
                case '\n': _wc_jb_append(jb, "\\n"); break;
                case '\r': _wc_jb_append(jb, "\\r"); break;
                case '\t': _wc_jb_append(jb, "\\t"); break;
                case '\b': _wc_jb_append(jb, "\\b"); break;
                case '\f': _wc_jb_append(jb, "\\f"); break;
                default:
                    if (c < 0x20) {
                        /* Every other control character must be escaped to
                           keep the output valid JSON; `\u00XX` with
                           lowercase hex, like the reference encoder. */
                        char esc[7] = { '\\', 'u', '0', '0', 0, 0, 0 };
                        esc[4] = "0123456789abcdef"[c >> 4];
                        esc[5] = "0123456789abcdef"[c & 15];
                        _wc_jb_append_len(jb, esc, 6);
                    } else {
                        _wc_jb_append_char(jb, (char)c);
                    }
                    break;
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
static void _wc_json_serialize_desc(_wc_json_buf *jb, int64_t val, const char *desc, const char *root, int indent, int depth);
static const char *_wc_desc_field(const char *body, const char *name);

/* F62B-019: map a container element's recorded kind (WASM_TAG_* | aux) to
   the serializer's type_hint (0 none / 1 Int / 2 Float / 3 Str / 4 Bool) so
   list elements are rendered by their recorded kind instead of the pointer
   heuristics (a Split[] result's Str elements rendered as "{}" / raw ints
   without this). Pack/list/unknown kinds keep the structural walk. */
static int _wc_elem_hint_from_ekind(uint32_t ekind) {
    switch (ekind & 0xFFu) {
        case 0: return 1;  /* WASM_TAG_INT */
        case 1: return 2;  /* WASM_TAG_FLOAT */
        case 2: return 4;  /* WASM_TAG_BOOL */
        case 3: return 3;  /* WASM_TAG_STR */
        default: return 0;
    }
}

/* Write the quoted name of the value `ordinal` of the Enum the kind entry
   `ek` names (native twin: json_append_enum_name). 0 when that Enum's
   names were never registered (the caller writes the ordinal). */
static int _wc_jb_append_enum_name(_wc_json_buf *jb, uint32_t ek, int64_t ordinal) {
    uint32_t id = ek >> 8;
    if (id == 0 || id >= WASM_ENUM_NAMES_CAP || ordinal < 0) return 0;
    const char *p = _wasm_enum_names_by_id[id];
    if (!p) return 0;
    for (int64_t i = 0; i < ordinal; i++) {
        while (*p && *p != ',') p++;
        if (!*p) return 0;
        p++;
    }
    _wc_jb_append_char(jb, '"');
    while (*p && *p != ',') _wc_jb_append_char(jb, *p++);
    _wc_jb_append_char(jb, '"');
    return 1;
}

/* Helper: check whether a wasm value is a Gorillax / RelaxedGorillax
   (fc=4, hash0=HASH_HAS_VALUE, hash2=HASH___ERROR). Mirrors
   `_wasm_is_gorillax` in 01_core.inc.c but staying local to 04 to avoid
   a forward-declare edit in 01. */
static int _wc_is_gorillax_local(int64_t val) {
    if (!_wc_is_valid_ptr(val, 104)) return 0;
    int64_t *p = (int64_t *)(intptr_t)val;
    if (p[0] != 4 || p[1] != WASM_HASH_HAS_VALUE) return 0;
    return p[1 + 2 * 3] == WASM_HASH___ERROR ? 1 : 0;
}

/* Helper: check whether a wasm value is a monadic pack
   (Lax / Gorillax / RelaxedGorillax / Result) via structural probing.
   Used by C25B-028 to expand `__*` fields in jsonEncode output. */
static int _wc_is_monadic_pack(int64_t val) {
    return _wc_is_lax(val) || _wc_is_result(val) || _wc_is_gorillax_local(val);
}

/* Helper: check whether the pack is a Lax specifically
   (slot 2 hash = WASM_HASH___DEFAULT). */
static int _wc_is_lax_specifically(int64_t *pack) {
    if (pack[0] != 4 && pack[0] != 5) return 0;
    if (pack[1] != WASM_HASH_HAS_VALUE) return 0;
    return pack[1 + 2 * 3] == WASM_HASH___DEFAULT ? 1 : 0;
}

/* The name of variant `ordinal` in "A,B,C" as a JSON string; the
   ordinal itself when it is out of range. */
static void _wc_json_append_enum_variant(_wc_json_buf *jb, int64_t ordinal, const char *variants) {
    const char *start = variants;
    int64_t idx = 0;
    for (const char *p = variants;; p++) {
        if (*p == ',' || *p == '\0' || *p == '}') {
            if (idx == ordinal) {
                _wc_jb_append_char(jb, '"');
                _wc_jb_append_len(jb, start, (int)(p - start));
                _wc_jb_append_char(jb, '"');
                return;
            }
            if (*p == '\0' || *p == '}') break;
            idx++;
            start = p + 1;
        }
    }
    _wc_jb_append(jb, _wc_i64_to_str(ordinal));
}

/* Helper: serialize pack fields as JSON object (alphabetically sorted)
 *
 * C25B-028: monadic packs (Lax / Gorillax / RelaxedGorillax / Result)
 * expose `__value` / `__default` / `__error` / `__predicate` so the
 * jsonEncode output matches the interpreter. Non-monadic user BuchiPacks
 * continue to hide all `__*` fields. `__type` is always skipped.
 *
 * For Lax payloads whose slot-1 (`__value`) is Int-like or Str-like, the
 * mirror slot (`__default`) is forced to the same type_hint so a raw `0`
 * doesn't render as `{}` and an empty-string ptr (or unset) renders as
 * `""`. Matches the native runtime C25B-028 fix.
 */
static void _wc_json_serialize_pack_fields(_wc_json_buf *jb, int64_t *pack, int64_t fc, int indent, int depth, const char *fields_desc, const char *root) {
    int is_monadic = _wc_is_monadic_pack((int64_t)(intptr_t)pack);
    int lax_default_force_type = 0; /* 0 none, 1 Int, 3 Str */
    if (is_monadic && _wc_is_lax_specifically(pack)) {
        int64_t value_slot_tag = pack[1 + 1 * 3 + 1];
        int64_t value_slot_val = pack[1 + 1 * 3 + 2];
        if (value_slot_tag == WASM_TAG_STR) {
            lax_default_force_type = 3;
        } else if (value_slot_tag == WASM_TAG_BOOL) {
            lax_default_force_type = 1;
        } else if (value_slot_tag == WASM_TAG_INT) {
            /* WASM_TAG_INT (0) is the "untagged" sentinel as well. We
               cannot reliably separate Int(42) from a low-address Str
               literal at this point without lowering-side tagging
               (both satisfy `_wc_looks_like_string` on wasm linear
               memory). Default to Int — the common case for
               `Lax[<IntLit>]()`. String-typed Lax is tagged STR when
               constructed through `Str[x]()` / `CharAt[...]()` via
               `_lax_tag_vd`, so the first branch above wins for those
               paths. `Lax["literal"]()` without any Str mold is a
               known edge case; tracked as follow-up for fully-typed
               Lax payloads.
               TODO (D26+): propagate payload type from lowering so we
               can disambiguate Int vs Str in raw `Lax[val]()` too. */
            if (value_slot_val == 0 || value_slot_val < 0 ||
                (value_slot_val > 0 && value_slot_val < 0x100000000LL)) {
                lax_default_force_type = 1;
            }
        }
    }

    typedef struct { const char *name; int64_t val; int type_hint; const char *variants; const char *desc; } _WcJsonField;
    _WcJsonField fields[100];
    int nfields = 0;
    /* A Result given no error writes the empty Error its `throw` reads as. */
    int is_result = _wasm_is_result((int64_t)(intptr_t)pack);
    for (int64_t i = 0; i < fc && nfields < 100; i++) {
        int64_t field_hash = pack[1 + i * 3];
        int64_t field_val = pack[1 + i * 3 + 2];
        if (is_result && field_hash == WASM_HASH_THROW && field_val == 0) {
            field_val = taida_result_throw_field((int64_t)(intptr_t)pack);
        }
        int64_t fname_ptr = taida_lookup_field_name(field_hash);
        const char *fname = (const char *)(intptr_t)fname_ptr;
        if (!fname) continue;
        /* Always skip __type (internal metadata). */
        if (fname[0] == '_' && fname[1] == '_' && fname[2] == 't' &&
            fname[3] == 'y' && fname[4] == 'p' && fname[5] == 'e' &&
            fname[6] == '\0') continue;
        if (_wc_is_lax_specifically(pack) &&
            fname[0] == '_' && fname[1] == '_' && fname[2] == 'e' &&
            fname[3] == 'r' && fname[4] == 'r' && fname[5] == 'o' &&
            fname[6] == 'r' && fname[7] == '\0') continue;
        /* Non-monadic packs hide all __ fields. Monadic packs expose
           them so jsonEncode matches the interpreter. */
        if (!is_monadic && fname[0] == '_' && fname[1] == '_') continue;
        /* Methods belong to the type, not to the value. */
        if (_wasm_pack_field_is_method(pack, fc, fname)) continue;
        /* A function has no JSON form; the reference writes null. */
        if (pack[1 + i * 3 + 1] == WASM_TAG_CLOSURE || taida_is_closure_value(field_val)) {
            fields[nfields].name = fname;
            fields[nfields].val = 0;
            fields[nfields].type_hint = -1;
            fields[nfields].variants = 0;
            fields[nfields].desc = 0;
            nfields++;
            continue;
        }

        /* F62B-019: the per-slot tag wins over the global field-name
           registry. The registry keys on the FIELD NAME hash alone, so two
           pack types sharing a name with different types (`tags: Str` on a
           D1 row vs `tags: @[Str]` on the response shape) corrupted each
           other — a registry hit of Str stringified the list pointer. The
           registry survives only for slots whose tag is INT(0), which is
           indistinguishable from "untagged". */
        int64_t slot_tag = pack[1 + i * 3 + 1];
        int64_t ftype;
        if (slot_tag == WASM_TAG_STR) ftype = 3;
        else if (slot_tag == WASM_TAG_BOOL) ftype = 4;
        else if (slot_tag == WASM_TAG_FLOAT) ftype = 2;
        else if (slot_tag == WASM_TAG_PACK || slot_tag == 5 /* LIST */
                 || slot_tag == 6 /* CLOSURE */) {
            ftype = 0; /* structural walk */
        } else {
            ftype = taida_lookup_field_type(field_hash, 0);
            /* `_wasm_lookup_field_type` returns -1 (not 0) for unregistered
               hashes; normalise to 0 so downstream "unset" checks work. */
            if (ftype < 0) ftype = 0;
        }
        /* Lax payload force (see sibling comment above). Applies to
           `__value` and `__default` in Lax packs so both render with
           the same T-type (matching interpreter). */
        int is_lax_value_or_default =
            (lax_default_force_type != 0 &&
             fname[0] == '_' && fname[1] == '_' &&
             ((fname[2] == 'v' && fname[3] == 'a' && fname[4] == 'l' &&
               fname[5] == 'u' && fname[6] == 'e' && fname[7] == '\0') ||
              (fname[2] == 'd' && fname[3] == 'e' && fname[4] == 'f' &&
               fname[5] == 'a' && fname[6] == 'u' && fname[7] == 'l' &&
               fname[8] == 't' && fname[9] == '\0')));
        if (is_lax_value_or_default) {
            /* Override whatever the registry/slot-tag picked up: Lax's
               `__value` and `__default` share the same T and the
               override was derived from the `__value` slot's shape. */
            ftype = lax_default_force_type;
            if (ftype == 3 && field_val == 0) {
                field_val = WSTR("");
            }
        }
        /* An Enum-typed field holds the ordinal; the wire carries the
           variant name. The per-pack registration wins over the per-name
           one (see _wasm_lookup_pack_enum_variants). */
        const char *variants = _wasm_lookup_pack_enum_variants((int64_t)(intptr_t)pack, field_hash);
        if (!variants && ftype == WASM_FIELD_TYPE_ENUM)
            variants = _wasm_lookup_field_enum_variants(field_hash);
        fields[nfields].name = fname;
        fields[nfields].val = field_val;
        fields[nfields].type_hint = (int)ftype;
        fields[nfields].variants = variants;
        /* The field's static type, when the caller knows the pack's. */
        const char *fdesc = fields_desc ? _wc_desc_field(fields_desc, fname) : 0;
        fields[nfields].desc = (fdesc && fdesc[0] != 'a') ? fdesc : 0;
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
        if (fields[i].type_hint == -1) {
            _wc_jb_append(jb, "null");
        } else if (fields[i].desc) {
            _wc_json_serialize_desc(jb, fields[i].val, fields[i].desc, root, indent, depth + 1);
        } else if (fields[i].variants) {
            _wc_json_append_enum_variant(jb, fields[i].val, fields[i].variants);
        } else {
            _wc_json_serialize_typed(jb, fields[i].val, indent, depth + 1, fields[i].type_hint);
        }
    }
    if (indent > 0 && nfields > 0) _wc_jb_append_indent(jb, indent, depth);
    _wc_jb_append_char(jb, '}');
}

/* jsonEncode / jsonPretty stop past this many levels of nesting, with the
   message and exit status the native runtime gives (native twin:
   TAIDA_JSON_MAX_DEPTH): the writer recurses once per level, and a deeper
   value would run the stack out. */
#define TAIDA_WASM_JSON_MAX_DEPTH 128

static void _wc_json_depth_exceeded(void) {
    extern int fd_write(int fd, const void *iovs, int iovs_len, int *nwritten)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")));
    const char *msg = "taida: JSON nesting depth limit exceeded\n";
    int len = 0;
    while (msg[len]) len++;
    struct { const char *buf; int len; } iov = { msg, len };
    int nwritten;
    fd_write(2, &iov, 1, &nwritten);
#ifdef TAIDA_WASM_PROFILE_EDGE
    __builtin_trap();
#else
    extern void proc_exit(int code)
        __attribute__((import_module("wasi_snapshot_preview1"), import_name("proc_exit")));
    proc_exit(1);
#endif
}

static void _wc_json_serialize_typed(_wc_json_buf *jb, int64_t val, int indent, int depth, int type_hint) {
    if (depth > TAIDA_WASM_JSON_MAX_DEPTH) {
        _wc_json_depth_exceeded();
        return;
    }
    /* A function has no JSON form; the reference writes null. */
    if (type_hint == 0 && taida_is_closure_value(val)) {
        _wc_jb_append(jb, "null");
        return;
    }
    if (type_hint == 4) {
        _wc_jb_append(jb, val ? "true" : "false");
        return;
    }
    /* C25B-028: Int type hint takes precedence over the val==0
       Unit fallback so `Value::Int(0)` renders as `"0"` not `"{}"`.
       Matches native runtime's fix and interpreter `taida_value_to_json`. */
    if (type_hint == 1) {
        char *num = _wc_i64_to_str(val);
        _wc_jb_append(jb, num);
        return;
    }
    /* Float hint: the payload is an f64 bit pattern — PRId64-style
       output wrote the raw bits into the JSON. Render through the
       shared formatter so the output round-trips (native twin). */
    if (type_hint == 2) {
        /* JSON has no NaN / Infinity: non-finite values encode as `null`,
           as the reference encoder does. */
        double d = _l2d(val);
        if (d != d || d - d != 0.0) {
            _wc_jb_append(jb, "null");
            return;
        }
        char num[40];
        _wc_json_format_float(d, num);
        _wc_jb_append(jb, num);
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
    if (type_hint == 3) {
        const char *s = (const char *)(intptr_t)val;
        _wc_jb_append_escaped_str(jb, s);
        return;
    }

    /* F56: a sealed carrier (Moltenized/Secret) serializes to JSON `null` — the
       sealed value must never reach the wire (direct or nested). Matches the
       interpreter's `jsonEncode(Moltenized)` -> `null`. The checker rejects this
       at compile time ([E1534]); this is the fail-closed runtime layer. */
    if (_wasm_carrier_kind(val)) {
        _wc_jb_append(jb, "null");
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
        /* wire a homogeneous value tag
           into each value slot so direct Float/Bool values stop
           serializing as raw bits and match the interpreter's typed
           output. UNKNOWN(-1)/HETEROGENEOUS(-2) maps keep the heuristic
           walk (hint 0); per-entry kinds for heterogeneous maps are on
           the value-tag track. Layout: [0]=cap, [1]=len, [2]=value_tag,
           [3]=marker — same slot the `.set()` tag latch writes. */
        int value_hint = _wc_elem_hint_from_ekind((uint32_t)hm[2]);
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
                /* only Str keys have a
                   JSON object-key form. The interpreter skips non-Str keys
                   silently; skip here too instead of emitting whatever the
                   key slot holds as a char*. */
                uint32_t key_ek = _wasm_hm_key_ek(hm, cap, slot);
                if (WASM_EKIND_KIND(key_ek) != (uint32_t)WASM_TAG_STR) continue;
                if (!_wasm_is_string_ptr(sk)) continue;
                uint32_t value_ek = WASM_HM_VALUE_EK(hm[WASM_HM_KIND_SLOT(cap, slot)]);
                int entry_hint = WASM_EKIND_KIND(value_ek) == WASM_EKIND_UNKNOWN
                    ? value_hint : _wc_elem_hint_from_ekind(value_ek);
                if (count > 0) _wc_jb_append_char(jb, ',');
                if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
                _wc_jb_append_escaped_str(jb, (const char *)(intptr_t)sk);
                _wc_jb_append_char(jb, ':');
                if (indent > 0) _wc_jb_append_char(jb, ' ');
                int64_t value = hm[WASM_HM_HEADER + slot * 3 + 2];
                if (WASM_EKIND_KIND(value_ek) == (uint32_t)WASM_TAG_ENUM
                    && _wc_jb_append_enum_name(jb, value_ek, value)) {
                    count++;
                    continue;
                }
                _wc_json_serialize_typed(jb, value, indent, depth + 1, entry_hint);
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
            _wc_json_serialize_typed(jb, list[4 + i], indent, depth + 1, _wc_elem_hint_from_ekind(_wasm_elem_kind_at(list, i)));
        }
        if (indent > 0 && list_len > 0) _wc_jb_append_indent(jb, indent, depth);
        _wc_jb_append_char(jb, ']');
        return;
    }

    /* Check monadic types (Result, Lax, Gorillax / RelaxedGorillax).
       C25B-028: include Gorillax family so jsonEncode expands `__error`
       and normalises `has_value` through the monadic branch above. */
    if (_wc_is_result(val) || _wc_is_lax(val) || _wc_is_gorillax_local(val)) {
        int64_t *pack = (int64_t *)(intptr_t)val;
        int64_t fc = pack[0];
        _wc_json_serialize_pack_fields(jb, pack, fc, indent, depth, 0, 0);
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
            _wc_json_serialize_typed(jb, list[4 + i], indent, depth + 1, _wc_elem_hint_from_ekind(_wasm_elem_kind_at(list, i)));
        }
        if (indent > 0 && list_len > 0) _wc_jb_append_indent(jb, indent, depth);
        _wc_jb_append_char(jb, ']');
        return;
    }

    /* String pointer check moved BEFORE the
       BuchiPack heuristic. A static-literal string sits in the data
       segment right next to other literals; reading its first payload
       word as an fc and the next word as a hash made any string whose
       neighbour carried a large header magic (every WSTR literal does)
       serialize as an empty `{}` object. The string identification is
       positive-only (`_wasm_is_string_ptr` checks the hidden magic
       word), which makes a false positive on well-formed packs
       implausible rather than impossible — the pack heuristic below
       still has the final word for anything the string check lets
       through. */
    if (_wc_looks_like_string(val)) {
        _wc_jb_append_escaped_str(jb, (const char *)(intptr_t)val);
        return;
    }

    /* Check BuchiPack */
    if (_wc_is_valid_ptr(val, 8)) {
        int64_t *obj = (int64_t *)(intptr_t)val;
        int64_t fc = obj[0];
        /* C25B-028: recognise an explicit empty BuchiPack (fc=0,
           allocated via `taida_pack_new(0)`) as `{}`. Gorillax's
           `__error` slot stores such a pack on the success branch. */
        if (fc == 0) {
            _wc_jb_append(jb, "{}");
            return;
        }
        if (fc > 0 && fc < 200) {
            int64_t hash0 = obj[1];
            if (hash0 > 0x10000 || hash0 < 0) {
                _wc_json_serialize_pack_fields(jb, obj, fc, indent, depth, 0, 0);
                return;
            }
        }
    }

    /* Default: integer */
    char *num = _wc_i64_to_str(val);
    _wc_jb_append(jb, num);
}

/* ── jsonEncode / jsonPretty with the argument's static type ──
   The lowering passes the checked type as a schema descriptor (the JSON
   reader's format: i f s b y, L{..}, T{Name|field:desc,...}, N{Name},
   E{Name|A,B}; `a` where the kind is not known statically). Parts the
   descriptor names are written by that kind; the rest keep the
   value-based detection. */

/* One past the end of the descriptor starting at `d`. */
static const char *_wc_desc_end(const char *d) {
    if (d[0] && d[1] == '{') {
        int depth = 0;
        for (const char *p = d + 1; *p; p++) {
            if (*p == '{') depth++;
            else if (*p == '}' && --depth == 0) return p + 1;
        }
        return d + wasm_strlen(d);
    }
    return d[0] ? d + 1 : d;
}

/* The descriptor of field `name` in a `field:desc,...}` body. */
static const char *_wc_desc_field(const char *body, const char *name) {
    int name_len = (int)wasm_strlen(name);
    const char *p = body;
    while (*p && *p != '}') {
        const char *colon = p;
        while (*colon && *colon != ':' && *colon != '}') colon++;
        if (*colon != ':') return 0;
        const char *d = colon + 1;
        if ((int)(colon - p) == name_len && _wf_strncmp(p, name, name_len) == 0) return d;
        p = _wc_desc_end(d);
        if (*p == ',') p++;
    }
    return 0;
}

/* The `T{Name|...}` a `N{Name}` reference (name at `name`) points back to. */
static const char *_wc_desc_named(const char *root, const char *name) {
    int name_len = 0;
    while (name[name_len] && name[name_len] != '}') name_len++;
    for (const char *p = root; p && *p; p++) {
        if (p[0] == 'T' && p[1] == '{' && _wf_strncmp(p + 2, name, name_len) == 0
            && p[2 + name_len] == '|') return p;
    }
    return 0;
}

static void _wc_json_serialize_desc(_wc_json_buf *jb, int64_t val, const char *desc, const char *root, int indent, int depth) {
    if (depth > TAIDA_WASM_JSON_MAX_DEPTH) {
        _wc_json_depth_exceeded();
        return;
    }
    switch (desc[0]) {
        case 'i': _wc_json_serialize_typed(jb, val, indent, depth, 1); return;
        case 'f': _wc_json_serialize_typed(jb, val, indent, depth, 2); return;
        case 's':
            /* An absent Str is the empty string, not an object. A value that
               is not a string after all (a type name the checker resolved to
               another module's type) keeps the value-based detection. */
            if (!val) {
                _wc_json_serialize_typed(jb, WSTR(""), indent, depth, 3);
                return;
            }
            if (_wc_looks_like_string(val)) {
                _wc_json_serialize_typed(jb, val, indent, depth, 3);
                return;
            }
            break;
        case 'b': _wc_json_serialize_typed(jb, val, indent, depth, 4); return;
        /* Bytes have no JSON form; the reference writes null. */
        case 'y': _wc_jb_append(jb, "null"); return;
        case 'E': {
            const char *v = desc + 2;
            while (*v && *v != '|' && *v != '}') v++;
            if (desc[1] == '{' && *v == '|') {
                _wc_json_append_enum_variant(jb, val, v + 1);
                return;
            }
            break;
        }
        case 'L':
            if (desc[1] == '{' && _wc_looks_like_list(val) && !_wc_is_hashmap(val) && !_wc_is_set(val)) {
                int64_t *list = (int64_t *)(intptr_t)val;
                int64_t list_len = list[1];
                _wc_jb_append_char(jb, '[');
                for (int64_t i = 0; i < list_len; i++) {
                    if (i > 0) _wc_jb_append_char(jb, ',');
                    if (indent > 0) _wc_jb_append_indent(jb, indent, depth + 1);
                    _wc_json_serialize_desc(jb, list[WASM_LIST_ELEMS + i], desc + 2, root, indent, depth + 1);
                }
                if (indent > 0 && list_len > 0) _wc_jb_append_indent(jb, indent, depth);
                _wc_jb_append_char(jb, ']');
                return;
            }
            break;
        case 'T':
            if (desc[1] == '{' && _wc_is_valid_ptr(val, 8)) {
                int64_t *obj = (int64_t *)(intptr_t)val;
                int64_t fc = obj[0];
                if (fc > 0 && fc < 200) {
                    const char *body = desc + 2;
                    while (*body && *body != '|' && *body != '}') body++;
                    if (*body == '|') body++;
                    _wc_json_serialize_pack_fields(jb, obj, fc, indent, depth, body, root);
                    return;
                }
            }
            break;
        case 'N':
            if (desc[1] == '{') {
                const char *named = _wc_desc_named(root, desc + 2);
                if (named) {
                    _wc_json_serialize_desc(jb, val, named, root, indent, depth);
                    return;
                }
            }
            break;
        default:
            break;
    }
    _wc_json_serialize_typed(jb, val, indent, depth, 0);
}

int64_t taida_json_encode_desc(int64_t val, int64_t desc_ptr) {
    const char *desc = (const char *)(intptr_t)desc_ptr;
    _wc_json_buf jb;
    _wc_jb_init(&jb);
    _wc_json_serialize_desc(&jb, val, desc, desc, 0, 0);
    return (int64_t)(intptr_t)jb.buf;
}

int64_t taida_json_pretty_desc(int64_t val, int64_t desc_ptr) {
    const char *desc = (const char *)(intptr_t)desc_ptr;
    _wc_json_buf jb;
    _wc_jb_init(&jb);
    _wc_json_serialize_desc(&jb, val, desc, desc, 2, 0);
    return (int64_t)(intptr_t)jb.buf;
}

static int _wc_wire_schema_desc_len(const char *d) {
    int i = 0;
    int depth = 0;
    while (d[i]) {
        if (depth == 0 && (d[i] == ',' || d[i] == '}')) break;
        if (d[i] == '{') depth++;
        else if (d[i] == '}') depth--;
        i++;
    }
    return i;
}

static char *_wc_wire_schema_copy(const char *d, int len) {
    char *out = _wasm_str_alloc((unsigned int)(len + 1));
    _wf_memcpy(out, d, len);
    out[len] = '\0';
    return out;
}

static int _wc_wire_bytes_len(int64_t value) {
    /* Real Bytes ([TAIDBYT, len, byte...]) take priority; the string and
       list arms survive as wire-compat fallbacks for Str bodies and
       legacy @[Int] payloads. */
    if (_looks_like_bytes(value)) {
        int64_t len = ((int64_t *)(intptr_t)value)[1];
        if (len < 0) return 0;
        if (len > 0x1000000) return 0x1000000;
        return (int)len;
    }
    if (_wc_looks_like_string(value)) return _wf_strlen((const char *)(intptr_t)value);
    if (!_wc_looks_like_list(value)) return 0;
    int64_t *list = (int64_t *)(intptr_t)value;
    int64_t len = list[1];
    if (len < 0) return 0;
    if (len > 0x1000000) return 0x1000000;
    return (int)len;
}

static unsigned char _wc_wire_bytes_at(int64_t value, int idx) {
    if (_looks_like_bytes(value)) {
        return (unsigned char)(((int64_t *)(intptr_t)value)[2 + idx] & 0xff);
    }
    if (_wc_looks_like_string(value)) {
        const unsigned char *s = (const unsigned char *)(intptr_t)value;
        return s ? s[idx] : 0;
    }
    if (!_wc_looks_like_list(value)) return 0;
    int64_t *list = (int64_t *)(intptr_t)value;
    return (unsigned char)(list[WASM_LIST_ELEMS + idx] & 0xff);
}

static char _wc_b64_char(int n) {
    static const char table[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    return table[n & 63];
}

static void _wc_json_wire_append_base64(_wc_json_buf *jb, int64_t value) {
    int len = _wc_wire_bytes_len(value);
    for (int i = 0; i < len; i += 3) {
        int rem = len - i;
        unsigned int b0 = _wc_wire_bytes_at(value, i);
        unsigned int b1 = rem > 1 ? _wc_wire_bytes_at(value, i + 1) : 0;
        unsigned int b2 = rem > 2 ? _wc_wire_bytes_at(value, i + 2) : 0;
        char out[4];
        out[0] = _wc_b64_char((b0 >> 2) & 63);
        out[1] = _wc_b64_char(((b0 & 3) << 4) | ((b1 >> 4) & 15));
        out[2] = rem > 1 ? _wc_b64_char(((b1 & 15) << 2) | ((b2 >> 6) & 3)) : '=';
        out[3] = rem > 2 ? _wc_b64_char(b2 & 63) : '=';
        _wc_jb_append_len(jb, out, 4);
    }
}

static void _wc_json_wire_serialize(_wc_json_buf *jb, int64_t val, const char *schema);

static void _wc_json_wire_serialize_list(_wc_json_buf *jb, int64_t val, const char *inner) {
    _wc_jb_append_char(jb, '[');
    if (_wc_looks_like_list(val)) {
        int64_t *list = (int64_t *)(intptr_t)val;
        int64_t len = list[1];
        if (len < 0 || len > 0x1000000) len = 0;
        for (int64_t i = 0; i < len; i++) {
            if (i > 0) _wc_jb_append_char(jb, ',');
            _wc_json_wire_serialize(jb, list[WASM_LIST_ELEMS + i], inner);
        }
    }
    _wc_jb_append_char(jb, ']');
}

static void _wc_json_wire_serialize_pack(_wc_json_buf *jb, int64_t val, const char *schema) {
    const char *d = schema;
    if (d[0] != 'T' || d[1] != '{') {
        _wc_json_serialize_typed(jb, val, 0, 0, 0);
        return;
    }
    d += 2;
    while (*d && *d != '|') d++;
    if (*d == '|') d++;

    _wc_jb_append_char(jb, '{');
    int first = 1;
    while (*d && *d != '}') {
        char fname[256];
        int fn_len = 0;
        while (*d && *d != ':' && *d != '}' && fn_len < 255) {
            fname[fn_len++] = *d;
            d++;
        }
        fname[fn_len] = '\0';
        if (*d == ':') d++;
        int desc_len = _wc_wire_schema_desc_len(d);
        char *field_desc = _wc_wire_schema_copy(d, desc_len);
        int64_t field_val = taida_pack_get(val, (int64_t)_wc_fnv1a(fname, fn_len));
        if (!first) _wc_jb_append_char(jb, ',');
        first = 0;
        _wc_jb_append_escaped_str(jb, fname);
        _wc_jb_append_char(jb, ':');
        _wc_json_wire_serialize(jb, field_val, field_desc);
        d += desc_len;
        if (*d == ',') d++;
    }
    _wc_jb_append_char(jb, '}');
}

static void _wc_json_wire_serialize_web_request(_wc_json_buf *jb, int64_t val) {
    int64_t method = taida_pack_get(val, (int64_t)_wc_fnv1a("method", 6));
    int64_t path = taida_pack_get(val, (int64_t)_wc_fnv1a("path", 4));
    int64_t raw_query = taida_pack_get(val, (int64_t)_wc_fnv1a("rawQuery", 8));
    int64_t query = taida_pack_get(val, (int64_t)_wc_fnv1a("query", 5));
    int64_t headers = taida_pack_get(val, (int64_t)_wc_fnv1a("headers", 7));
    int64_t body = taida_pack_get(val, (int64_t)_wc_fnv1a("body", 4));

    _wc_jb_append(jb, "{\"method\":");
    _wc_json_wire_serialize(jb, method, "s");
    _wc_jb_append(jb, ",\"path\":");
    _wc_json_wire_serialize(jb, path, "s");
    _wc_jb_append(jb, ",\"rawQuery\":");
    _wc_json_wire_serialize(jb, raw_query, "s");
    _wc_jb_append(jb, ",\"query\":");
    _wc_json_wire_serialize(jb, query, "L{T{NameValue|name:s,value:s}}");
    _wc_jb_append(jb, ",\"headers\":");
    _wc_json_wire_serialize(jb, headers, "L{T{NameValue|name:s,value:s}}");
    _wc_jb_append(jb, ",\"bodyBase64\":\"");
    _wc_json_wire_append_base64(jb, body);
    _wc_jb_append(jb, "\"}");
}

static void _wc_json_wire_serialize_web_response(_wc_json_buf *jb, int64_t val) {
    int64_t status = taida_pack_get(val, (int64_t)_wc_fnv1a("status", 6));
    int64_t headers = taida_pack_get(val, (int64_t)_wc_fnv1a("headers", 7));
    int64_t body = taida_pack_get(val, (int64_t)_wc_fnv1a("body", 4));

    _wc_jb_append(jb, "{\"status\":");
    _wc_json_wire_serialize(jb, status, "i");
    _wc_jb_append(jb, ",\"headers\":");
    _wc_json_wire_serialize(jb, headers, "L{T{NameValue|name:s,value:s}}");
    _wc_jb_append(jb, ",\"bodyBase64\":\"");
    _wc_json_wire_append_base64(jb, body);
    _wc_jb_append(jb, "\"}");
}

static void _wc_json_wire_serialize(_wc_json_buf *jb, int64_t val, const char *schema) {
    if (!schema || !schema[0] || schema[0] == 'a') {
        _wc_json_serialize_typed(jb, val, 0, 0, 0);
        return;
    }
    switch (schema[0]) {
        case 'i': _wc_json_serialize_typed(jb, val, 0, 0, 1); return;
        case 'f': _wc_json_serialize_typed(jb, val, 0, 0, 2); return;
        case 's': _wc_json_serialize_typed(jb, val, 0, 0, 3); return;
        case 'b': _wc_json_serialize_typed(jb, val, 0, 0, 4); return;
        case 'y':
            _wc_jb_append_char(jb, '"');
            _wc_json_wire_append_base64(jb, val);
            _wc_jb_append_char(jb, '"');
            return;
        case 'Q':
            _wc_json_wire_serialize_web_request(jb, val);
            return;
        case 'R':
            _wc_json_wire_serialize_web_response(jb, val);
            return;
        case 'L': {
            if (schema[1] != '{') {
                _wc_json_wire_serialize_list(jb, val, "a");
                return;
            }
            const char *inner_start = schema + 2;
            int inner_len = _wc_schema_find_closing_brace(inner_start);
            char *inner = _wc_wire_schema_copy(inner_start, inner_len);
            _wc_json_wire_serialize_list(jb, val, inner);
            return;
        }
        case 'T':
            _wc_json_wire_serialize_pack(jb, val, schema);
            return;
        default:
            _wc_json_serialize_typed(jb, val, 0, 0, 0);
            return;
    }
}

int64_t taida_json_encode_wire(int64_t value, int64_t schema_ptr) {
    const char *schema = (const char *)(intptr_t)schema_ptr;
    _wc_json_buf jb;
    _wc_jb_init(&jb);
    _wc_json_wire_serialize(&jb, value, schema);
    return (int64_t)(intptr_t)jb.buf;
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

/* AsyncReject[x]() — a rejected Async holding `x` (native twin:
   taida_async_err_kinded). `ekind` is the static kind of `x`; its tag is
   kept so the payload is displayed and thrown as what it is. */
int64_t taida_async_err_kinded(int64_t error, int64_t ekind) {
    int64_t obj_ptr = taida_async_err(error);
    if (!obj_ptr) return 0;
    int64_t kind = ekind & 0xFF;
    int64_t tag = kind != 0xFF ? kind
                : (taida_is_buchi_pack(error) ? WASM_TAG_PACK : WASM_ASYNC_TAG_UNKNOWN);
    ((int64_t *)(intptr_t)obj_ptr)[6] = tag;
    return obj_ptr;
}

/* The text of a rejected Async's payload of kind `tag`, as the reference
   displays it (native twin: taida_async_payload_display). */
static int64_t _wasm_async_payload_display(int64_t payload, int64_t tag) {
    switch (tag) {
    case WASM_TAG_INT:
    case WASM_TAG_ENUM:
        return taida_str_from_int(payload);
    case WASM_TAG_FLOAT:
        return taida_str_from_float(payload);
    case WASM_TAG_BOOL:
        return taida_str_from_bool(payload);
    default:
        return _wasm_value_to_display_string(payload);
    }
}

/* The error a rejected Async throws (native twin:
   taida_async_rejection_error): a pack as it is; any other payload, such as
   the Str of `AsyncReject["timeout"]()`, as an AsyncError whose message is
   the payload's text, so an `|== e: Error` handler can read it. */
static int64_t _wasm_async_rejection_error(int64_t error, int64_t etag) {
    if (error == 0) {
        return taida_make_error(WSTR("AsyncError"), WSTR("Async rejected"));
    }
    if (etag != WASM_TAG_INT && etag != WASM_TAG_FLOAT && etag != WASM_TAG_BOOL
        && etag != WASM_TAG_ENUM && taida_is_buchi_pack(error)) {
        return error;
    }
    return taida_make_error(WSTR("AsyncError"), _wasm_async_payload_display(error, etag));
}

int64_t taida_async_pending_with_error(int64_t error) {
    int64_t *obj = (int64_t *)wasm_alloc(7 * 8);
    if (!obj) return 0;
    obj[0] = WASM_ASYNC_MAGIC;
    obj[1] = 0;  /* pending */
    obj[2] = 0;
    obj[3] = error;
    obj[4] = 0;
    obj[5] = WASM_ASYNC_TAG_UNKNOWN;
    obj[6] = WASM_TAG_PACK;
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
        /* rejected -> throw. A pack payload (an Error value of any
           subtype) is thrown as-is; other payloads become an AsyncError. */
        return taida_throw(_wasm_async_rejection_error(obj[3], obj[6]));
    }
    if (status == 0 && obj[3] != 0) {
        return taida_throw(obj[3]);
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
                return taida_throw(_wasm_async_rejection_error(aobj[3], aobj[6]));
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
    if (len == 0) {
        return taida_async_err(taida_make_error(
            WSTR("AsyncRaceError"),
            WSTR("Race requires at least one value")));
    }
    int64_t first = list[WASM_LIST_ELEMS];
    if (_wasm_is_async_obj(first)) {
        int64_t *aobj = (int64_t *)(intptr_t)first;
        if (aobj[1] == 2) {
            /* rejected -> throw (matches native behavior) */
            return taida_throw(_wasm_async_rejection_error(aobj[3], aobj[6]));
        }
        return taida_async_ok_tagged(aobj[2], aobj[5]);
    }
    return taida_async_ok_tagged(first, WASM_ASYNC_TAG_UNKNOWN);
}

static int64_t _wasm_async_task_error_from_caught(int64_t fallback_msg) {
    int64_t message = fallback_msg;
    if (__wasm_error_depth > 0) {
        int64_t error = __wasm_error_val[__wasm_error_depth - 1];
        if (_looks_like_pack(error) && taida_pack_has_hash(error, WASM_HASH_MESSAGE)) {
            int64_t caught_message = taida_pack_get(error, WASM_HASH_MESSAGE);
            if (caught_message) message = caught_message;
        }
    }
    return taida_make_error(WSTR("AsyncTaskError"), message);
}

int64_t taida_async_task_par(int64_t list_ptr) {
    if (!_looks_like_list(list_ptr)) {
        return taida_async_err(taida_make_error(
            WSTR("AsyncTaskError"),
            WSTR("Par expected a list of AsyncTask values")));
    }
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t result = taida_list_new();
    for (int64_t i = 0; i < len; i++) {
        int64_t task = _wasm_async_task_callable(list[WASM_LIST_ELEMS + i]);
        int64_t value = _wasm_invoke_callback0(task);
        if (__wasm_error_thrown) {
            int64_t error = _wasm_async_task_error_from_caught(WSTR("AsyncTask failed"));
            __wasm_error_thrown = 0;
            return taida_async_err(error);
        }
        result = taida_list_push(result, value);
    }
    return taida_async_ok_tagged(result, WASM_ASYNC_TAG_UNKNOWN);
}

int64_t taida_async_task_par_map(int64_t list_ptr, int64_t fn_ptr) {
    if (!_looks_like_list(list_ptr)) {
        return taida_async_err(taida_make_error(
            WSTR("AsyncTaskError"),
            WSTR("ParMap expected a list and a function")));
    }
    int64_t *list = (int64_t *)(intptr_t)list_ptr;
    int64_t len = list[1];
    int64_t result = taida_list_new();
    for (int64_t i = 0; i < len; i++) {
        int64_t value = _wasm_invoke_callback1(fn_ptr, list[WASM_LIST_ELEMS + i]);
        if (__wasm_error_thrown) {
            int64_t error = _wasm_async_task_error_from_caught(WSTR("ParMap task failed"));
            __wasm_error_thrown = 0;
            return taida_async_err(error);
        }
        result = taida_list_push(result, value);
    }
    return taida_async_ok_tagged(result, WASM_ASYNC_TAG_UNKNOWN);
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
/* The reason a rejected Async shows (native twin: taida_rejection_display):
   an error the runtime itself raised shows as `Error(type: message)`;
   anything else shows as a value. */
static int64_t _wasm_rejection_display(int64_t error) {
    if (_looks_like_pack(error)
        && taida_pack_has_hash(error, WASM_HASH_TYPE)
        && taida_pack_has_hash(error, WASM_HASH_MESSAGE)
        && taida_pack_has_hash(error, WASM_HASH___TYPE)) {
        int64_t type = taida_pack_get(error, WASM_HASH___TYPE);
        static const char *const runtime_types[] = {
            "AssertionError", "AsyncError", "AsyncRaceError", "AsyncStateError",
            "CancelledError", "HostCapabilityError", "HttpError", "ProcessError",
            "RangeError", "ResultError", "TimeoutError", "ValueError", 0,
        };
        int runtime = 0;
        if (_looks_like_string(type)) {
            const char *t = (const char *)(intptr_t)type;
            for (int i = 0; runtime_types[i]; i++) {
                if (_wf_strcmp(t, runtime_types[i]) == 0) { runtime = 1; break; }
            }
            if (_wf_strncmp(t, "Addon", 5) == 0) runtime = 1;
        }
        int64_t message = taida_pack_get(error, WASM_HASH_MESSAGE);
        if (runtime && _looks_like_string(message)) {
            _wasm_strbuf sb;
            _sb_init(&sb);
            _sb_append(&sb, "Error(");
            _sb_append(&sb, (const char *)(intptr_t)taida_pack_get(error, WASM_HASH_TYPE));
            _sb_append(&sb, ": ");
            _sb_append(&sb, (const char *)(intptr_t)message);
            _sb_append(&sb, ")");
            return _sb_finish(&sb);
        }
    }
    return _wasm_value_to_display_string(error);
}

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
        int64_t etag = obj[6];
        int64_t err_str = (etag == WASM_TAG_INT || etag == WASM_TAG_FLOAT
                           || etag == WASM_TAG_BOOL || etag == WASM_TAG_ENUM)
                              ? _wasm_async_payload_display(obj[3], etag)
                              : _wasm_rejection_display(obj[3]);
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
#ifdef TAIDA_PERF_COUNTERS
    wasm_perf_dump();
#endif
}
