/* ── Numeric kernel: f64 <-> decimal text, and round ──
 *
 * The Float display, ToFixed and Float[Str] parsing on all wasm profiles
 * run on this kernel, so a wasm program prints and reads Float values as
 * the interpreter does (Rust's f64 formatting and parsing, which are exact:
 * shortest round-trip digits, exactly rounded fixed notation, correctly
 * rounded parsing).  The math molds use the shared correctly rounded kernel
 * (runtime_crmath.inc.c).
 *
 * Freestanding: <stdint.h> only; no libc, no heap.  The file is part of the
 * single runtime translation unit (see mod.rs); the entry points are
 * declared ahead of their callers in 01_core.inc.c:
 *
 *   _tf_display(x, out)     Taida Float display: integral finite values in
 *                           fixed notation with one decimal ("2.0", exact
 *                           digits), all others in the shortest positional
 *                           form ("0.1"; never an exponent).
 *   _tf_to_string(x, out)   the shortest positional form for every value
 *                           ("100", "0.1").
 *   _tf_fixed(x, d, out)    fixed notation with d decimals (0..20, clamped),
 *                           rounded half to even on the exact value; a
 *                           negative value keeps its "-" ("-0.00").
 *   _tf_parse(s, len, out)  [+-] digits [. digits] [(e|E) [+-] digits]
 *                           (a digit on at least one side of the point),
 *                           or inf / infinity / nan in any case;
 *                           correctly rounded.  Returns 0 for anything
 *                           else.
 *   _tf_shortest(x, dg, e)  the shortest digits that read back as |x|
 *                           (nonzero finite): |x| = 0.dg * 10^e.
 *
 * Methods (exact, on little-endian u32 big integers):
 *   shortest: x and the two ends of the interval of reals that read back
 *     as x are scaled to integers and written out in full in decimal; for
 *     n = 1, 2, ... the n-digit decimals just below and just above x are
 *     tested against the interval (its ends count when x's mantissa is
 *     even, as a tie then reads back as x).  The first n with a decimal
 *     inside wins; when both are inside, the one nearer to x (an exact tie:
 *     the larger).
 *   fixed: x * 10^d = m * 5^d * 2^(e+d), shifted with round-half-even.
 *   parse: up to 800 significant digits are kept (a further nonzero digit
 *     is remembered).  With 19 digits or fewer and a power of ten up to
 *     10^22 the value is formed in double or double-double arithmetic and
 *     its rounding decided; otherwise the result is found by binary search
 *     over the bit patterns, comparing the decimal value exactly with the
 *     midpoint above each candidate.  No such midpoint has more than 770
 *     significant digits, so the kept digits always decide.
 *
 * Buffers: _tf_display writes at most 331 bytes, _tf_fixed at most 331,
 * _tf_shortest at most 17 digits; no NUL is written.
 */

/* The runtime is linked without libc: functions that move overlapping
   ranges disable builtins so the moves cannot become memmove calls (the
   compiler may still emit memcpy and memset, which the runtime has). */
#if defined(__clang__)
#define _TF_FN static __attribute__((noinline, minsize))
#define _TF_API static __attribute__((unused, noinline, minsize))
#define _TF_NO_BUILTIN __attribute__((no_builtin))
#else
#define _TF_FN static __attribute__((noinline))
#define _TF_API static __attribute__((unused, noinline))
#define _TF_NO_BUILTIN
#endif

static uint64_t _tf_bits(double x)
{
    union { double d; uint64_t u; } v;
    v.d = x;
    return v.u;
}

static double _tf_from_bits(uint64_t u)
{
    union { double d; uint64_t u; } v;
    v.u = u;
    return v.d;
}

/* ------------------------------------------------------------------ */
/* Big unsigned integers                                              */
/* ------------------------------------------------------------------ */

#define _TF_CAP 240 /* 7680 bits */

typedef struct {
    int n; /* used limbs; 0 means zero */
    uint32_t w[_TF_CAP];
} _tf_big;

_TF_FN void _tf_big_trim(_tf_big *a)
{
    while (a->n > 0 && a->w[a->n - 1] == 0) a->n--;
}

_TF_FN void _tf_big_set(_tf_big *a, uint64_t v)
{
    a->w[0] = (uint32_t)v;
    a->w[1] = (uint32_t)(v >> 32);
    a->n = 2;
    _tf_big_trim(a);
}

_TF_FN void _tf_big_copy(_tf_big *r, const _tf_big *a)
{
    r->n = a->n;
    for (int i = 0; i < a->n; i++) r->w[i] = a->w[i];
}

/* a = a * k + add */
_TF_FN void _tf_big_muladd(_tf_big *a, uint32_t k, uint32_t add)
{
    uint64_t carry = add;
    for (int i = 0; i < a->n; i++) {
        uint64_t t = (uint64_t)a->w[i] * k + carry;
        a->w[i] = (uint32_t)t;
        carry = t >> 32;
    }
    if (carry && a->n < _TF_CAP) a->w[a->n++] = (uint32_t)carry;
}

/* a = a * 5^k */
_TF_FN void _tf_big_mul_pow5(_tf_big *a, int k)
{
    while (k >= 13) { _tf_big_muladd(a, 1220703125u, 0); k -= 13; }
    uint32_t p = 1;
    while (k-- > 0) p *= 5;
    if (p > 1) _tf_big_muladd(a, p, 0);
}

/* a = a * 10^k */
_TF_FN void _tf_big_mul_pow10(_tf_big *a, int k)
{
    while (k >= 9) { _tf_big_muladd(a, 1000000000u, 0); k -= 9; }
    uint32_t p = 1;
    while (k-- > 0) p *= 10;
    if (p > 1) _tf_big_muladd(a, p, 0);
}

/* r = a << s (r may alias a) */
_TF_FN _TF_NO_BUILTIN void _tf_big_shl(_tf_big *r, const _tf_big *a, int s)
{
    if (a->n == 0) { r->n = 0; return; }
    int limbs = s / 32, bits = s % 32;
    int an = a->n;
    int n = an + limbs + 1;
    if (n > _TF_CAP) n = _TF_CAP;
    for (int i = n - 1; i >= 0; i--) {
        int src = i - limbs;
        uint32_t hi = (src >= 0 && src < an) ? a->w[src] : 0;
        uint32_t lo = (src >= 1 && src - 1 < an) ? a->w[src - 1] : 0;
        r->w[i] = bits ? ((hi << bits) | (lo >> (32 - bits))) : hi;
    }
    r->n = n;
    _tf_big_trim(r);
}

/* r = a * b (r must not alias a or b) */
_TF_FN void _tf_big_mul(_tf_big *r, const _tf_big *a, const _tf_big *b)
{
    int n = a->n + b->n;
    if (n > _TF_CAP) n = _TF_CAP;
    for (int i = 0; i < n; i++) r->w[i] = 0;
    for (int i = 0; i < a->n; i++) {
        uint64_t carry = 0;
        for (int j = 0; j < b->n && i + j < n; j++) {
            uint64_t t = (uint64_t)a->w[i] * b->w[j] + r->w[i + j] + carry;
            r->w[i + j] = (uint32_t)t;
            carry = t >> 32;
        }
        if (i + b->n < n) r->w[i + b->n] = (uint32_t)carry;
    }
    r->n = n;
    _tf_big_trim(r);
}

_TF_FN int _tf_big_cmp(const _tf_big *a, const _tf_big *b)
{
    if (a->n != b->n) return a->n < b->n ? -1 : 1;
    for (int i = a->n - 1; i >= 0; i--)
        if (a->w[i] != b->w[i]) return a->w[i] < b->w[i] ? -1 : 1;
    return 0;
}

_TF_FN int _tf_big_bitlen(const _tf_big *a)
{
    if (a->n == 0) return 0;
    uint32_t t = a->w[a->n - 1];
    int b = 0;
    while (t) { b++; t >>= 1; }
    return (a->n - 1) * 32 + b;
}

_TF_FN int _tf_big_bit(const _tf_big *a, int i)
{
    int w = i / 32;
    return w < a->n ? (int)((a->w[w] >> (i % 32)) & 1u) : 0;
}

/* The decimal digits of a (destroyed) into out, most significant first;
   returns the count ("0" for zero). */
_TF_FN int _tf_big_to_dec(_tf_big *a, char *out)
{
    char tmp[2400];
    int n = 0;
    if (a->n == 0) { out[0] = '0'; return 1; }
    while (a->n > 0) {
        uint64_t rem = 0;
        for (int i = a->n - 1; i >= 0; i--) {
            uint64_t cur = (rem << 32) | a->w[i];
            a->w[i] = (uint32_t)(cur / 1000000000u);
            rem = cur % 1000000000u;
        }
        _tf_big_trim(a);
        for (int k = 0; k < 9; k++) { tmp[n++] = (char)('0' + rem % 10); rem /= 10; }
    }
    while (n > 1 && tmp[n - 1] == '0') n--;
    for (int i = 0; i < n; i++) out[i] = tmp[n - 1 - i];
    return n;
}

/* ------------------------------------------------------------------ */
/* Formatting                                                         */
/* ------------------------------------------------------------------ */

_TF_FN int _tf_copy(char *out, const char *s)
{
    int n = 0;
    while (s[n]) { out[n] = s[n]; n++; }
    return n;
}

/* Compare decimal digit strings (no leading zeros) as integers. */
_TF_FN int _tf_dec_cmp(const char *a, int an, const char *b, int bn)
{
    if (an != bn) return an < bn ? -1 : 1;
    for (int i = 0; i < an; i++)
        if (a[i] != b[i]) return a[i] < b[i] ? -1 : 1;
    return 0;
}

/* Is the decimal v inside [lo, hi] (the ends counted when incl)? */
_TF_FN int _tf_dec_inside(const char *v, int vn, const char *lo, int lon,
                          const char *hi, int hin, int incl)
{
    int a = _tf_dec_cmp(v, vn, lo, lon), b = _tf_dec_cmp(v, vn, hi, hin);
    return incl ? (a >= 0 && b <= 0) : (a > 0 && b < 0);
}

/* The decimal digits of f * k into out. */
_TF_FN int _tf_scaled_dec(const _tf_big *f, uint64_t k, char *out)
{
    static _tf_big kb, r;
    _tf_big_set(&kb, k);
    _tf_big_mul(&r, f, &kb);
    return _tf_big_to_dec(&r, out);
}

_TF_API _TF_NO_BUILTIN int _tf_shortest(double x, char *dg, int *ex)
{
    uint64_t b = _tf_bits(x);
    int biased = (int)((b >> 52) & 0x7ff);
    uint64_t frac = b & ((1ULL << 52) - 1);
    uint64_t m = biased ? (frac | (1ULL << 52)) : frac;
    int e = biased ? biased - 1075 : -1074;
    int asym = frac == 0 && biased > 1;
    int incl = (m & 1) == 0;
    /* X = 4m F, H = (4m + 2) F, L = (4m - 2) F (or (4m - 1) F when the gap
       below x is half the gap above), all divided by 10^scale: F = 2^(e-2),
       or F = 5^(2-e) with scale = 2 - e. */
    static _tf_big f;
    static char xs[800], hs[800], ls[800], v[810], up[810], twice[810];
    _tf_big_set(&f, 1);
    int scale = 0;
    if (e >= 2) {
        _tf_big_shl(&f, &f, e - 2);
    } else {
        _tf_big_mul_pow5(&f, 2 - e);
        scale = 2 - e;
    }
    int xn = _tf_scaled_dec(&f, 4 * m, xs);
    int hn = _tf_scaled_dec(&f, 4 * m + 2, hs);
    int ln = _tf_scaled_dec(&f, asym ? 4 * m - 1 : 4 * m - 2, ls);
    for (int n = 1; n <= xn; n++) {
        int drop = xn - n;
        /* below: the first n digits of X, then zeros */
        for (int i = 0; i < n; i++) v[i] = xs[i];
        for (int i = n; i < xn; i++) v[i] = '0';
        int down_in = _tf_dec_inside(v, xn, ls, ln, hs, hn, incl);
        /* above: the first n digits plus one (maybe a digit longer) */
        int un = xn, carry = 1;
        for (int i = xn - 1; i >= 0; i--) {
            if (i >= n) { up[i] = '0'; continue; }
            int d = (xs[i] - '0') + carry;
            carry = d / 10;
            up[i] = (char)('0' + d % 10);
        }
        if (carry) {
            for (int i = xn; i > 0; i--) up[i] = up[i - 1];
            up[0] = '1';
            un = xn + 1;
        }
        int up_in = _tf_dec_inside(up, un, ls, ln, hs, hn, incl);
        if (!down_in && !up_in) continue;
        int use_up = up_in;
        if (down_in && up_in) {
            /* the nearer one: 2 * (dropped digits of X) against 10^drop */
            int c = 0, carry2 = 0;
            for (int k = drop - 1; k >= 0; k--) {
                int d = (xs[n + k] - '0') * 2 + carry2;
                twice[k + 1] = (char)('0' + d % 10);
                carry2 = d / 10;
            }
            twice[0] = (char)('0' + carry2);
            for (int k = 0; k <= drop && c == 0; k++) {
                char want = k == 0 ? '1' : '0';
                if (twice[k] != want) c = twice[k] < want ? -1 : 1;
            }
            use_up = c >= 0;
        }
        const char *s = use_up ? up : v;
        int sn = use_up ? un : xn;
        int keep = sn;
        while (keep > 1 && s[keep - 1] == '0') keep--;
        for (int i = 0; i < keep; i++) dg[i] = s[i];
        /* value = s / 10^scale = 0.s * 10^(sn - scale) */
        *ex = sn - scale;
        return keep;
    }
    /* not reached: X itself lies in the interval */
    *ex = xn - scale;
    for (int i = 0; i < 17; i++) dg[i] = i < xn ? xs[i] : '0';
    return xn < 17 ? xn : 17;
}

/* The shortest positional form of the finite x ("0.001", "100"). */
_TF_FN int _tf_positional(double x, char *out)
{
    uint64_t b = _tf_bits(x);
    int pos = 0;
    if (b >> 63) out[pos++] = '-';
    if ((b << 1) == 0) { out[pos++] = '0'; return pos; }
    char dg[24];
    int ex;
    int n = _tf_shortest(x, dg, &ex);
    if (ex <= 0) {
        out[pos++] = '0';
        out[pos++] = '.';
        for (int i = 0; i < -ex; i++) out[pos++] = '0';
        for (int i = 0; i < n; i++) out[pos++] = dg[i];
    } else if (ex < n) {
        for (int i = 0; i < ex; i++) out[pos++] = dg[i];
        out[pos++] = '.';
        for (int i = ex; i < n; i++) out[pos++] = dg[i];
    } else {
        for (int i = 0; i < n; i++) out[pos++] = dg[i];
        for (int i = n; i < ex; i++) out[pos++] = '0';
    }
    return pos;
}

static int _tf_is_nonfinite(double x)
{
    return ((_tf_bits(x) >> 52) & 0x7ff) == 0x7ff;
}

_TF_FN int _tf_nonfinite(double x, char *out)
{
    uint64_t b = _tf_bits(x);
    if (b & ((1ULL << 52) - 1)) return _tf_copy(out, "NaN");
    return _tf_copy(out, (b >> 63) ? "-inf" : "inf");
}

_TF_API _TF_NO_BUILTIN int _tf_fixed(double x, int digits, char *out)
{
    if (digits < 0) digits = 0;
    if (digits > 20) digits = 20;
    if (_tf_is_nonfinite(x)) return _tf_nonfinite(x, out);
    uint64_t b = _tf_bits(x);
    int pos = 0;
    if (b >> 63) out[pos++] = '-';
    int biased = (int)((b >> 52) & 0x7ff);
    uint64_t frac = b & ((1ULL << 52) - 1);
    uint64_t m = biased ? (frac | (1ULL << 52)) : frac;
    int e = biased ? biased - 1075 : -1074;
    static _tf_big nb;
    static char ds[420];
    int dn;
    /* N = round_half_even(m * 5^d * 2^(e+d)) */
    _tf_big_set(&nb, m);
    _tf_big_mul_pow5(&nb, digits);
    int s = e + digits;
    if (nb.n == 0) {
        /* zero */
    } else if (s >= 0) {
        _tf_big_shl(&nb, &nb, s);
    } else {
        int k = -s;
        if (k > _tf_big_bitlen(&nb)) {
            nb.n = 0; /* below one half */
        } else {
            int half = _tf_big_bit(&nb, k - 1);
            int sticky = 0;
            for (int i = 0; i < k - 1 && !sticky; i++) sticky = _tf_big_bit(&nb, i);
            int limbs = k / 32, bits = k % 32;
            int n = nb.n - limbs;
            for (int i = 0; i < n; i++) {
                uint32_t lo = nb.w[i + limbs];
                uint32_t hi = i + limbs + 1 < nb.n ? nb.w[i + limbs + 1] : 0;
                nb.w[i] = bits ? ((lo >> bits) | (hi << (32 - bits))) : lo;
            }
            nb.n = n > 0 ? n : 0;
            _tf_big_trim(&nb);
            int odd = nb.n > 0 && (nb.w[0] & 1u);
            if (half && (sticky || odd)) _tf_big_muladd(&nb, 1, 1);
        }
    }
    dn = _tf_big_to_dec(&nb, ds);
    /* at least digits+1 digits; the point before the last `digits` */
    int pad = digits + 1 - dn;
    if (pad > 0) {
        for (int i = dn - 1; i >= 0; i--) ds[i + pad] = ds[i];
        for (int i = 0; i < pad; i++) ds[i] = '0';
        dn += pad;
    }
    for (int i = 0; i < dn - digits; i++) out[pos++] = ds[i];
    if (digits > 0) {
        out[pos++] = '.';
        for (int i = dn - digits; i < dn; i++) out[pos++] = ds[i];
    }
    return pos;
}

static int _tf_is_integral(double x)
{
    uint64_t b = _tf_bits(x);
    int biased = (int)((b >> 52) & 0x7ff);
    if (biased >= 1075) return 1; /* |x| >= 2^52 */
    if (biased < 1023) return (b << 1) == 0;
    return (b & ((1ULL << (1075 - biased)) - 1)) == 0;
}

_TF_API int _tf_display(double x, char *out)
{
    if (_tf_is_nonfinite(x)) return _tf_nonfinite(x, out);
    if (_tf_is_integral(x)) return _tf_fixed(x, 1, out);
    return _tf_positional(x, out);
}

_TF_API int _tf_to_string(double x, char *out)
{
    if (_tf_is_nonfinite(x)) return _tf_nonfinite(x, out);
    return _tf_positional(x, out);
}

/* ------------------------------------------------------------------ */
/* Parsing                                                            */
/* ------------------------------------------------------------------ */

#define _TF_MAX_DIGITS 800

_TF_FN int _tf_word(const char *s, int len, const char *w)
{
    int n = 0;
    while (w[n]) n++;
    if (len != n) return 0;
    for (int i = 0; i < n; i++) {
        int c = (unsigned char)s[i];
        if (c >= 'A' && c <= 'Z') c += 32;
        if (c != w[i]) return 0;
    }
    return 1;
}

static const double _tf_pow10_exact[23] = {
    1e0,  1e1,  1e2,  1e3,  1e4,  1e5,  1e6,  1e7,  1e8,  1e9,  1e10, 1e11,
    1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
};

/* Does the decimal value V round to a double at or below the positive
   double whose bits are cand?  V = a (e10 >= 0, a = digits * 10^e10) or
   V = a / p10 (e10 < 0); sticky: V is slightly above that. */
_TF_FN int _tf_rounds_at_or_below(uint64_t cand, const _tf_big *a, const _tf_big *p10,
                                  int e10, int sticky)
{
    if (cand >= 0x7ff0000000000000ULL) return 1; /* infinity */
    int biased = (int)((cand >> 52) & 0x7ff);
    uint64_t frac = cand & ((1ULL << 52) - 1);
    uint64_t mb = biased ? (frac | (1ULL << 52)) : frac;
    int eb = biased ? biased - 1075 : -1074;
    /* compare V with the midpoint above: (2 mb + 1) * 2^(eb - 1) */
    static _tf_big left, right, ub;
    _tf_big_set(&ub, 2 * mb + 1);
    if (e10 < 0) _tf_big_mul(&right, &ub, p10);
    else _tf_big_copy(&right, &ub);
    if (1 - eb > 0) _tf_big_shl(&left, a, 1 - eb);
    else _tf_big_copy(&left, a);
    if (eb - 1 > 0) _tf_big_shl(&right, &right, eb - 1);
    int c = _tf_big_cmp(&left, &right);
    if (c != 0) return c < 0;
    if (sticky) return 0;
    return (mb & 1) == 0; /* a tie goes to the even neighbour */
}

_TF_API int _tf_parse(const char *s, int len, double *out)
{
    int i = 0, neg = 0;
    if (i < len && (s[i] == '+' || s[i] == '-')) { neg = s[i] == '-'; i++; }
    if (_tf_word(s + i, len - i, "inf") || _tf_word(s + i, len - i, "infinity")) {
        *out = _tf_from_bits((neg ? (1ULL << 63) : 0) | 0x7ff0000000000000ULL);
        return 1;
    }
    if (_tf_word(s + i, len - i, "nan")) {
        *out = _tf_from_bits(0x7ff8000000000000ULL);
        return 1;
    }
    static char dg[_TF_MAX_DIGITS];
    int nd = 0, sticky = 0, any = 0;
    int64_t e10 = 0;
    while (i < len && s[i] >= '0' && s[i] <= '9') {
        any = 1;
        if (nd == 0 && s[i] == '0') { i++; continue; }
        if (nd < _TF_MAX_DIGITS) dg[nd++] = s[i];
        else { if (s[i] != '0') sticky = 1; e10++; }
        i++;
    }
    if (i < len && s[i] == '.') {
        i++;
        while (i < len && s[i] >= '0' && s[i] <= '9') {
            any = 1;
            if (nd == 0 && s[i] == '0') { e10--; i++; continue; }
            if (nd < _TF_MAX_DIGITS) { dg[nd++] = s[i]; e10--; }
            else if (s[i] != '0') sticky = 1;
            i++;
        }
    }
    if (!any) return 0;
    if (i < len && (s[i] == 'e' || s[i] == 'E')) {
        i++;
        int eneg = 0;
        if (i < len && (s[i] == '+' || s[i] == '-')) { eneg = s[i] == '-'; i++; }
        if (i >= len || s[i] < '0' || s[i] > '9') return 0;
        int64_t en = 0;
        while (i < len && s[i] >= '0' && s[i] <= '9') {
            if (en < 0x10000) en = en * 10 + (s[i] - '0');
            i++;
        }
        e10 += eneg ? -en : en;
    }
    if (i != len) return 0;
    uint64_t sign = neg ? (1ULL << 63) : 0;
    while (nd > 0 && dg[nd - 1] == '0') { nd--; e10++; }
    if (nd == 0) { *out = _tf_from_bits(sign); return 1; }
    if (nd + e10 > 310) { *out = _tf_from_bits(sign | 0x7ff0000000000000ULL); return 1; }
    if (nd + e10 < -324) { *out = _tf_from_bits(sign); return 1; }
    if (!sticky && nd <= 19 && e10 >= -22 && e10 <= 22) {
        uint64_t d = 0;
        for (int k = 0; k < nd; k++) d = d * 10 + (uint64_t)(dg[k] - '0');
        double p = _tf_pow10_exact[e10 >= 0 ? e10 : -e10];
        if (d <= (1ULL << 53)) {
            /* exact operands, one rounding */
            double v = (double)d;
            v = e10 >= 0 ? v * p : v / p;
            *out = neg ? -v : v;
            return 1;
        }
        /* d = hi + lo exactly; the product or quotient in double-double
           (relative error below 2^-103), rounded when that decides it */
        const double c = 134217729.0; /* 2^27 + 1 */
        double hi = (double)d;
        uint64_t hiu = (uint64_t)hi;
        double lo = hiu >= d ? -(double)(hiu - d) : (double)(d - hiu);
        double rh, rl;
        if (e10 >= 0) {
            double t = c * hi, ah = t - (t - hi), al = hi - ah;
            t = c * p;
            double bh = t - (t - p), bl = p - bh;
            double pp = hi * p;
            double pe = ((ah * bh - pp) + ah * bl + al * bh) + al * bl;
            pe += lo * p;
            rh = pp + pe;
            rl = pe - (rh - pp);
        } else {
            double q1 = hi / p;
            double t = c * q1, ah = t - (t - q1), al = q1 - ah;
            t = c * p;
            double bh = t - (t - p), bl = p - bh;
            double pp = q1 * p;
            double pe = ((ah * bh - pp) + ah * bl + al * bh) + al * bl;
            double r = ((hi - pp) - pe + lo) / p;
            rh = q1 + r;
            rl = r - (rh - q1);
        }
        double err = 0x1p-98 * (rh < 0 ? -rh : rh);
        double ra = rh + (rl - err), rb = rh + (rl + err);
        if (ra == rb) { *out = neg ? -ra : ra; return 1; }
    }
    /* exact: binary search over the positive bit patterns */
    static _tf_big a, p10, t;
    a.n = 0;
    for (int k = 0; k < nd; k++) _tf_big_muladd(&a, 10, (uint32_t)(dg[k] - '0'));
    _tf_big_set(&p10, 1);
    _tf_big_mul_pow10(&p10, (int)(e10 >= 0 ? e10 : -e10));
    if (e10 >= 0) { _tf_big_mul(&t, &a, &p10); _tf_big_copy(&a, &t); }
    uint64_t lo = 0, hi = 0x7ff0000000000000ULL;
    while (lo < hi) {
        uint64_t mid = lo + (hi - lo) / 2;
        if (_tf_rounds_at_or_below(mid, &a, &p10, (int)e10, sticky)) hi = mid;
        else lo = mid + 1;
    }
    *out = _tf_from_bits(sign | lo);
    return 1;
}

/*
 * round: to the nearest integer, ties away from zero (C round, Rust
 * f64::round).  Exact for every input; keeps the sign of zero.  (The
 * transcendental functions live in the shared correctly rounded kernel,
 * runtime_crmath.inc.c, concatenated after this fragment.)
 */
static double _tm_round(double x)
{
  union { double f; uint64_t i; } u = { x };
  uint32_t e = (uint32_t)(u.i >> 52) & 0x7ff;
  if (e >= 0x3ff + 52)
    return x; /* already integral, inf or nan */
  if (e < 0x3fe)
    return __builtin_copysign(0.0, x); /* |x| < 0.5 */
  double t = __builtin_trunc(x);
  if (__builtin_fabs(x - t) >= 0.5) /* x - t is exact */
    t += __builtin_copysign(1.0, x);
  return t;
}
