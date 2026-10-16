/* ── Correctly rounded math kernel ──────────────────────────────────────
 *
 * Every function returns the double nearest to the exact value (ties to
 * even).  A correctly rounded result is unique, so the interpreter (the
 * Rust mirror of this file, src/crmath/), the native runtime and every
 * WASM profile return the same bits on every CPU.  Special values follow
 * C99 Annex F.
 *
 * Two paths (Ziv's strategy):
 *   fast: double-double evaluation with a relative error bound; the result
 *         is returned when the bound decides the rounding;
 *   slow: 352-bit evaluation (u32 limbs, u32*u32->u64 products only),
 *         accurate to far better than 2^-250, which decides the rounding
 *         of these transcendental values; pow additionally checks its
 *         exact and midpoint cases exactly.
 *
 * Freestanding: <stdint.h> only (no libm, no long double, no __int128, no
 * FMA instruction); sqrt is IEEE-exact.  Floating-point contraction must
 * stay off: the error-free transformations below depend on it.  The build
 * passes -ffp-contract=off; clang also honours the pragma below (GCC does
 * not know it and would warn).
 *
 * The tables (runtime_crmath_tables.inc.c, generated with mpmath) are
 * concatenated in front of this file.
 */

#if defined(__clang__)
#pragma STDC FP_CONTRACT OFF
#endif

/* The WASM runtime is linked without libc: the in-place limb shifts
   disable builtins so they cannot become memmove calls. */
#if defined(__clang__)
#define _CRM_NO_BUILTIN __attribute__((no_builtin))
#else
#define _CRM_NO_BUILTIN
#endif

#define _CRM_LIMBS 11
#define _CRM_BITS 352
#define _CRM_WIDE (2 * _CRM_LIMBS + 1)

typedef struct {
  double hi, lo;
} _crm_dd;

static inline uint64_t _crm_bits(double x)
{
  union { double f; uint64_t i; } u;
  u.f = x;
  return u.i;
}

static inline double _crm_from_bits(uint64_t i)
{
  union { uint64_t i; double f; } u;
  u.i = i;
  return u.f;
}

#define _CRM_INF _crm_from_bits(0x7ff0000000000000ULL)
#define _CRM_NAN _crm_from_bits(0x7ff8000000000000ULL)

static inline double _crm_fabs(double x) { return _crm_from_bits(_crm_bits(x) & 0x7fffffffffffffffULL); }
static inline int _crm_isnan(double x) { return x != x; }
static inline int _crm_isinf(double x) { return (_crm_bits(x) & 0x7fffffffffffffffULL) == 0x7ff0000000000000ULL; }
static inline int _crm_signbit(double x) { return (int)(_crm_bits(x) >> 63); }
static inline double _crm_with_sign(double v, int neg) { return neg ? -v : v; }

/* 2^m for -1022 <= m <= 1023. */
static inline double _crm_pow2(int m) { return _crm_from_bits((uint64_t)(m + 1023) << 52); }

/* y == an integer (finite y). */
static int _crm_is_integer(double y)
{
  double a = _crm_fabs(y);
  if (a >= 4503599627370496.0) return !_crm_isinf(y) && !_crm_isnan(y);  /* 2^52 */
  return (double)(int64_t)y == y;
}

static int _crm_is_odd_integer(double y)
{
  if (!_crm_is_integer(y) || _crm_fabs(y) >= 9007199254740992.0) return 0;  /* 2^53 */
  return (int)((int64_t)y & 1);
}

/* Round to a nearest integer (any tie choice). */
static inline int64_t _crm_round_i(double v) { return (int64_t)(v >= 0 ? v + 0.5 : v - 0.5); }

/* ------------------------------------------------------------------------ */
/* 352-bit arithmetic                                                        */
/* ------------------------------------------------------------------------ */

static _CRM_NO_BUILTIN void _crm_shl_wide(uint32_t *w, int len, uint32_t n)
{
  int limbs = (int)(n / 32);
  uint32_t bits = n % 32;
  if (limbs >= len) {
    for (int i = 0; i < len; i++) w[i] = 0;
    return;
  }
  for (int i = len - 1; i >= 0; i--) {
    int src = i - limbs;
    uint32_t hi = src >= 0 ? w[src] : 0;
    uint32_t lo = src >= 1 ? w[src - 1] : 0;
    w[i] = bits == 0 ? hi : ((hi << bits) | (lo >> (32 - bits)));
  }
}

static _CRM_NO_BUILTIN void _crm_shr_wide(uint32_t *w, int len, uint32_t n)
{
  int limbs = (int)(n / 32);
  uint32_t bits = n % 32;
  if (limbs >= len) {
    for (int i = 0; i < len; i++) w[i] = 0;
    return;
  }
  for (int i = 0; i < len; i++) {
    int src = i + limbs;
    uint32_t lo = src < len ? w[src] : 0;
    uint32_t hi = src + 1 < len ? w[src + 1] : 0;
    w[i] = bits == 0 ? lo : ((lo >> bits) | (hi << (32 - bits)));
  }
}

static inline int _crm_clz32(uint32_t v)
{
  int n = 0;
  if (v == 0) return 32;
  while (!(v & 0x80000000u)) { v <<= 1; n++; }
  return n;
}

static const _crm_mp _CRM_MP_ZERO = { 0, 0, { 0 } };

static inline int _crm_mp_is_zero(const _crm_mp *a) { return a->m[_CRM_LIMBS - 1] == 0; }

/* The value w * 2^e (w little-endian, len >= LIMBS limbs, clobbered). */
static _crm_mp _crm_mp_from_wide(int neg, uint32_t *w, int len, int32_t e)
{
  int top = len;
  while (top > 0 && w[top - 1] == 0) top--;
  if (top == 0) return _CRM_MP_ZERO;
  int high = (top - 1) * 32 + (31 - _crm_clz32(w[top - 1]));
  int lead = len * 32 - 1 - high;
  _crm_shl_wide(w, len, (uint32_t)lead);
  _crm_mp r;
  r.neg = neg;
  r.exp = e + 32 * (len - _CRM_LIMBS) - lead;
  for (int i = 0; i < _CRM_LIMBS; i++) r.m[i] = w[len - _CRM_LIMBS + i];
  return r;
}

static _crm_mp _crm_mp_from_u64(uint64_t v)
{
  uint32_t w[_CRM_LIMBS] = { 0 };
  w[0] = (uint32_t)v;
  w[1] = (uint32_t)(v >> 32);
  return _crm_mp_from_wide(0, w, _CRM_LIMBS, 0);
}

static _crm_mp _crm_mp_from_i64(int64_t v)
{
  _crm_mp r = _crm_mp_from_u64(v < 0 ? (uint64_t)0 - (uint64_t)v : (uint64_t)v);
  r.neg = v < 0 && !_crm_mp_is_zero(&r);
  return r;
}

/* The exact value of the finite double x (a zero loses its sign). */
static _crm_mp _crm_mp_from_f64(double x)
{
  uint64_t b = _crm_bits(x);
  int neg = (int)(b >> 63);
  int biased = (int)((b >> 52) & 0x7ff);
  uint64_t frac = b & ((1ULL << 52) - 1);
  uint64_t mant;
  int32_t e;
  if (biased == 0) { mant = frac; e = -1074; } else { mant = frac | (1ULL << 52); e = biased - 1075; }
  uint32_t w[_CRM_LIMBS] = { 0 };
  w[0] = (uint32_t)mant;
  w[1] = (uint32_t)(mant >> 32);
  _crm_mp r = _crm_mp_from_wide(neg, w, _CRM_LIMBS, e);
  if (_crm_mp_is_zero(&r)) r.neg = 0;
  return r;
}

static inline _crm_mp _crm_mp_neg(_crm_mp a)
{
  if (!_crm_mp_is_zero(&a)) a.neg = !a.neg;
  return a;
}

static inline _crm_mp _crm_mp_abs(_crm_mp a)
{
  a.neg = 0;
  return a;
}

static inline _crm_mp _crm_mp_ldexp(_crm_mp a, int32_t n)
{
  if (!_crm_mp_is_zero(&a)) a.exp += n;
  return a;
}

static inline int32_t _crm_mp_top_exp(const _crm_mp *a) { return a->exp + _CRM_BITS - 1; }

/* Compare magnitudes: -1, 0, 1. */
static int _crm_mp_cmp_abs(const _crm_mp *a, const _crm_mp *b)
{
  int az = _crm_mp_is_zero(a), bz = _crm_mp_is_zero(b);
  if (az && bz) return 0;
  if (az) return -1;
  if (bz) return 1;
  if (a->exp != b->exp) return a->exp < b->exp ? -1 : 1;
  for (int i = _CRM_LIMBS - 1; i >= 0; i--)
    if (a->m[i] != b->m[i]) return a->m[i] < b->m[i] ? -1 : 1;
  return 0;
}

static _crm_mp _crm_mp_add(const _crm_mp *a, const _crm_mp *b)
{
  if (_crm_mp_is_zero(a)) return *b;
  if (_crm_mp_is_zero(b)) return *a;
  const _crm_mp *x = a->exp >= b->exp ? a : b;
  const _crm_mp *y = a->exp >= b->exp ? b : a;
  int64_t d = (int64_t)x->exp - (int64_t)y->exp;
  if (d > 32 * (_CRM_WIDE - _CRM_LIMBS) - 2) return *x;
  uint32_t wx[_CRM_WIDE] = { 0 }, wy[_CRM_WIDE] = { 0 };
  for (int i = 0; i < _CRM_LIMBS; i++) {
    wx[_CRM_WIDE - _CRM_LIMBS + i] = x->m[i];
    wy[_CRM_WIDE - _CRM_LIMBS + i] = y->m[i];
  }
  _crm_shr_wide(wy, _CRM_WIDE, (uint32_t)d);
  int32_t base = x->exp - 32 * (_CRM_WIDE - _CRM_LIMBS);
  if (x->neg == y->neg) {
    uint32_t w[_CRM_WIDE + 1];
    uint64_t carry = 0;
    for (int i = 0; i < _CRM_WIDE; i++) {
      uint64_t s = (uint64_t)wx[i] + wy[i] + carry;
      w[i] = (uint32_t)s;
      carry = s >> 32;
    }
    w[_CRM_WIDE] = (uint32_t)carry;
    return _crm_mp_from_wide(x->neg, w, _CRM_WIDE + 1, base);
  }
  int gt = 0;
  for (int i = _CRM_WIDE - 1; i >= 0; i--)
    if (wx[i] != wy[i]) { gt = wx[i] > wy[i] ? 1 : -1; break; }
  if (gt == 0) return _CRM_MP_ZERO;
  const uint32_t *big = gt > 0 ? wx : wy;
  const uint32_t *small = gt > 0 ? wy : wx;
  int neg = gt > 0 ? x->neg : y->neg;
  uint32_t w[_CRM_WIDE];
  int64_t borrow = 0;
  for (int i = 0; i < _CRM_WIDE; i++) {
    int64_t s = (int64_t)big[i] - (int64_t)small[i] - borrow;
    if (s < 0) { w[i] = (uint32_t)(s + ((int64_t)1 << 32)); borrow = 1; }
    else { w[i] = (uint32_t)s; borrow = 0; }
  }
  return _crm_mp_from_wide(neg, w, _CRM_WIDE, base);
}

static inline _crm_mp _crm_mp_sub(const _crm_mp *a, const _crm_mp *b)
{
  _crm_mp nb = _crm_mp_neg(*b);
  return _crm_mp_add(a, &nb);
}

static _crm_mp _crm_mp_mul(const _crm_mp *a, const _crm_mp *b)
{
  if (_crm_mp_is_zero(a) || _crm_mp_is_zero(b)) return _CRM_MP_ZERO;
  uint32_t w[2 * _CRM_LIMBS] = { 0 };
  for (int i = 0; i < _CRM_LIMBS; i++) {
    uint64_t carry = 0;
    uint64_t ai = a->m[i];
    for (int j = 0; j < _CRM_LIMBS; j++) {
      uint64_t t = ai * b->m[j] + w[i + j] + carry;
      w[i + j] = (uint32_t)t;
      carry = t >> 32;
    }
    w[i + _CRM_LIMBS] = (uint32_t)carry;
  }
  return _crm_mp_from_wide(a->neg != b->neg, w, 2 * _CRM_LIMBS, a->exp + b->exp);
}

static _crm_mp _crm_mp_div_u32(const _crm_mp *a, uint32_t k)
{
  if (_crm_mp_is_zero(a)) return _CRM_MP_ZERO;
  uint32_t num[_CRM_LIMBS + 2] = { 0 };
  uint32_t q[_CRM_LIMBS + 2];
  for (int i = 0; i < _CRM_LIMBS; i++) num[i + 2] = a->m[i];
  uint64_t rem = 0;
  for (int i = _CRM_LIMBS + 1; i >= 0; i--) {
    uint64_t cur = (rem << 32) | num[i];
    q[i] = (uint32_t)(cur / k);
    rem = cur % k;
  }
  return _crm_mp_from_wide(a->neg, q, _CRM_LIMBS + 2, a->exp - 64);
}

/* The leading bits as d * 2^e, 0.5 <= d < 1 (truncated). */
static double _crm_mp_approx_frac(const _crm_mp *a, int32_t *e)
{
  uint64_t hi = ((uint64_t)a->m[_CRM_LIMBS - 1] << 32) | a->m[_CRM_LIMBS - 2];
  *e = a->exp + _CRM_BITS;
  return (double)(hi >> 11) / 9007199254740992.0;
}

/* d * 2^e for the approximations below (saturating, inexact is fine). */
static double _crm_scale_approx(double d, int32_t e)
{
  if (e > 1100) e = 1100;
  if (e < -1100) e = -1100;
  while (e > 0) { int s = e > 1000 ? 1000 : e; d *= _crm_pow2(s); e -= s; }
  while (e < 0) { int s = -e > 1000 ? 1000 : -e; d *= _crm_pow2(-s); e += s; }
  return d;
}

static double _crm_mp_approx_f64(const _crm_mp *a)
{
  if (_crm_mp_is_zero(a)) return 0.0;
  int32_t e;
  double d = _crm_mp_approx_frac(a, &e);
  double v = _crm_scale_approx(d, e);
  return a->neg ? -v : v;
}

static _crm_mp _crm_mp_recip(const _crm_mp *a)
{
  int32_t e;
  double d = _crm_mp_approx_frac(a, &e);
  _crm_mp m = *a;
  m.neg = 0;
  m.exp = a->exp - e;
  _crm_mp one = _crm_mp_from_u64(1);
  _crm_mp y = _crm_mp_from_f64(1.0 / d);
  for (int i = 0; i < 3; i++) {
    _crm_mp my = _crm_mp_mul(&m, &y);
    _crm_mp t = _crm_mp_sub(&one, &my);
    _crm_mp yt = _crm_mp_mul(&y, &t);
    y = _crm_mp_add(&y, &yt);
  }
  _crm_mp r = _crm_mp_ldexp(y, -e);
  r.neg = a->neg;
  return r;
}

static _crm_mp _crm_mp_div(const _crm_mp *a, const _crm_mp *b)
{
  _crm_mp r = _crm_mp_recip(b);
  return _crm_mp_mul(a, &r);
}

static _crm_mp _crm_mp_sqrt(const _crm_mp *a)
{
  if (_crm_mp_is_zero(a)) return _CRM_MP_ZERO;
  int32_t e;
  (void)_crm_mp_approx_frac(a, &e);
  int32_t k = (e % 2 == 0) ? e / 2 : (e + 1) / 2;
  _crm_mp v = _crm_mp_ldexp(*a, -2 * k);
  int32_t ve;
  double vd = _crm_mp_approx_frac(&v, &ve);
  double vf = ve == 0 ? vd : vd * 0.5;
  _crm_mp y = _crm_mp_from_f64(1.0 / __builtin_sqrt(vf));
  _crm_mp one = _crm_mp_from_u64(1);
  for (int i = 0; i < 3; i++) {
    _crm_mp yy = _crm_mp_mul(&y, &y);
    _crm_mp vyy = _crm_mp_mul(&v, &yy);
    _crm_mp t = _crm_mp_sub(&one, &vyy);
    _crm_mp yt = _crm_mp_ldexp(_crm_mp_mul(&y, &t), -1);
    y = _crm_mp_add(&y, &yt);
  }
  _crm_mp r = _crm_mp_mul(&v, &y);
  return _crm_mp_ldexp(r, k);
}

/* The double nearest to the value.  err_bits >= 0: the value is known to
   within 2^-err_bits (relative); returns 0 (and leaves *out) when that
   does not decide the rounding.  err_bits < 0: the value is exact (ties to
   even).  Returns 1 on success. */
static int _crm_mp_round(const _crm_mp *a, int err_bits, double *out)
{
  uint64_t sign = a->neg ? (1ULL << 63) : 0;
  if (_crm_mp_is_zero(a)) { *out = _crm_from_bits(sign); return 1; }
  int32_t e_top = _crm_mp_top_exp(a);
  if (e_top >= 1024) { *out = _crm_from_bits(sign | 0x7ff0000000000000ULL); return 1; }
  if (e_top < -1076) { *out = _crm_from_bits(sign); return 1; }
  int32_t p = e_top >= -1022 ? 53 : e_top + 1075;
  int32_t r = _CRM_BITS - p;
  uint32_t rem[_CRM_LIMBS + 2] = { 0 };
  for (int i = 0; i < _CRM_LIMBS; i++) rem[i] = a->m[i];
  uint64_t kept = 0;
  if (r < _CRM_BITS) {
    uint32_t k[_CRM_LIMBS];
    for (int i = 0; i < _CRM_LIMBS; i++) k[i] = a->m[i];
    _crm_shr_wide(k, _CRM_LIMBS, (uint32_t)r);
    int full = r / 32, part = r % 32;
    for (int i = 0; i < _CRM_LIMBS + 2; i++) {
      if (i > full || (i == full && part == 0)) rem[i] = 0;
      else if (i == full) rem[i] &= (1u << part) - 1;
    }
    kept = (uint64_t)k[0] | ((uint64_t)k[1] << 32);
  }
  uint32_t half[_CRM_LIMBS + 2] = { 0 };
  if (r >= 1) {
    int hb = r - 1;
    half[hb / 32] |= 1u << (hb % 32);
  }
  int ge = 1;
  for (int i = _CRM_LIMBS + 1; i >= 0; i--)
    if (rem[i] != half[i]) { ge = rem[i] > half[i]; break; }
  const uint32_t *big = ge ? rem : half;
  const uint32_t *small = ge ? half : rem;
  uint32_t diff[_CRM_LIMBS + 2];
  int64_t borrow = 0;
  int diff_zero = 1;
  for (int i = 0; i < _CRM_LIMBS + 2; i++) {
    int64_t s = (int64_t)big[i] - (int64_t)small[i] - borrow;
    if (s < 0) { diff[i] = (uint32_t)(s + ((int64_t)1 << 32)); borrow = 1; }
    else { diff[i] = (uint32_t)s; borrow = 0; }
    if (diff[i]) diff_zero = 0;
  }
  int round_up;
  if (err_bits >= 0) {
    int eb = _CRM_BITS - err_bits;
    if (eb < 0) eb = 0;
    uint32_t err[_CRM_LIMBS + 2] = { 0 };
    err[eb / 32] |= 1u << (eb % 32);
    int le = 1;
    for (int i = _CRM_LIMBS + 1; i >= 0; i--)
      if (diff[i] != err[i]) { le = diff[i] < err[i]; break; }
    if (le) return 0;
    round_up = ge;
  } else {
    round_up = diff_zero ? (int)(kept & 1) : ge;
  }
  kept += (uint64_t)round_up;
  int32_t q = a->exp + r;
  if (p >= 1 && kept == (1ULL << p) && e_top >= -1022) { kept >>= 1; q += 1; }
  if (kept == 0) { *out = _crm_from_bits(sign); return 1; }
  if (e_top < -1022 || kept < (1ULL << 52)) { *out = _crm_from_bits(sign | kept); return 1; }
  int32_t biased = q + 52 + 1023;
  if (biased >= 2047) { *out = _crm_from_bits(sign | 0x7ff0000000000000ULL); return 1; }
  *out = _crm_from_bits(sign | ((uint64_t)biased << 52) | (kept - (1ULL << 52)));
  return 1;
}

static inline double _crm_mp_round_nearest(const _crm_mp *a)
{
  double v = 0.0;
  (void)_crm_mp_round(a, -1, &v);
  return v;
}

/* ------------------------------------------------------------------------ */
/* Slow path: the functions in 352-bit arithmetic                            */
/* ------------------------------------------------------------------------ */

#define _CRM_SLOW_ERR_BITS 250
#define _CRM_SERIES_BITS 360

static inline _crm_mp _crm_one(void) { return _crm_mp_from_u64(1); }

static inline int _crm_negligible(const _crm_mp *term, const _crm_mp *sum)
{
  return _crm_mp_is_zero(term)
      || (!_crm_mp_is_zero(sum) && _crm_mp_top_exp(term) < _crm_mp_top_exp(sum) - _CRM_SERIES_BITS);
}

static double _crm_finish(const _crm_mp *v)
{
  double r;
  if (_crm_mp_round(v, _CRM_SLOW_ERR_BITS, &r)) return r;
  return _crm_mp_round_nearest(v);
}

static _crm_mp _crm_slow_exp(const _crm_mp *x)
{
  int64_t k = _crm_round_i(_crm_mp_approx_f64(x) * 1.4426950408889634);
  _crm_mp kk = _crm_mp_from_i64(k);
  _crm_mp kl = _crm_mp_mul(&kk, &_CRM_MP_LN2);
  _crm_mp r = _crm_mp_sub(x, &kl);
  _crm_mp t = _crm_mp_ldexp(r, -12);
  _crm_mp sum = _crm_one(), term = _crm_one();
  for (uint32_t n = 1; n < 200; n++) {
    _crm_mp tt = _crm_mp_mul(&term, &t);
    term = _crm_mp_div_u32(&tt, n);
    if (_crm_negligible(&term, &sum)) break;
    sum = _crm_mp_add(&sum, &term);
  }
  for (int i = 0; i < 12; i++) sum = _crm_mp_mul(&sum, &sum);
  return _crm_mp_ldexp(sum, (int32_t)k);
}

static _crm_mp _crm_slow_ln(const _crm_mp *x)
{
  int32_t e = _crm_mp_top_exp(x);
  _crm_mp m = _crm_mp_ldexp(*x, -e);
  if (_crm_mp_approx_f64(&m) > 1.4142135623730951) { m = _crm_mp_ldexp(m, -1); e += 1; }
  _crm_mp one = _crm_one();
  _crm_mp num = _crm_mp_sub(&m, &one);
  _crm_mp den = _crm_mp_add(&m, &one);
  _crm_mp t = _crm_mp_div(&num, &den);
  _crm_mp t2 = _crm_mp_mul(&t, &t);
  _crm_mp sum = t, pw = t;
  for (uint32_t n = 1; n < 400; n++) {
    pw = _crm_mp_mul(&pw, &t2);
    _crm_mp term = _crm_mp_div_u32(&pw, 2 * n + 1);
    if (_crm_negligible(&term, &sum)) break;
    sum = _crm_mp_add(&sum, &term);
  }
  _crm_mp ee = _crm_mp_from_i64(e);
  _crm_mp el = _crm_mp_mul(&ee, &_CRM_MP_LN2);
  _crm_mp s2 = _crm_mp_ldexp(sum, 1);
  return _crm_mp_add(&el, &s2);
}

static uint32_t _crm_two_over_pi_bit(int i)
{
  int w = (i - 1) / 32;
  int b = 31 - ((i - 1) % 32);
  return (_CRM_TWO_OVER_PI_BITS[w] >> b) & 1u;
}

/* x = q pi/2 + r with |r| <= pi/4 (q mod 4), finite x. */
static void _crm_reduce_pio2(double x, uint32_t *qout, _crm_mp *rout)
{
  double ax = _crm_fabs(x);
  uint64_t q;
  _crm_mp r;
  if (ax < 1048576.0) {
    int64_t qi = _crm_round_i(ax * 0.6366197723675814);
    _crm_mp qq = _crm_mp_from_i64(qi);
    _crm_mp qp = _crm_mp_mul(&qq, &_CRM_MP_PI_2);
    _crm_mp xa = _crm_mp_from_f64(ax);
    r = _crm_mp_sub(&xa, &qp);
    q = (uint64_t)qi;
  } else {
    uint64_t b = _crm_bits(ax);
    uint64_t mant = (b & ((1ULL << 52) - 1)) | (1ULL << 52);
    int64_t e = (int64_t)((b >> 52) & 0x7ff) - 1075;
    int i0 = (int)(e - 1 > 1 ? e - 1 : 1);
    int i1 = (int)(e + 53 + 450);
    int nbits = i1 - i0 + 1;
    int nlimbs = (nbits + 31) / 32 + 1;
    uint32_t wv[20] = { 0 };
    uint32_t p[22] = { 0 };
    uint32_t frac[22];
    for (int k = 0, i = i0; i <= i1; i++, k++) {
      int pos = nbits - 1 - k;
      wv[pos / 32] |= _crm_two_over_pi_bit(i) << (pos % 32);
    }
    uint32_t mw[2] = { (uint32_t)mant, (uint32_t)(mant >> 32) };
    for (int j = 0; j < 2; j++) {
      uint64_t carry = 0;
      for (int i = 0; i < nlimbs; i++) {
        uint64_t t = (uint64_t)mw[j] * wv[i] + p[i + j] + carry;
        p[i + j] = (uint32_t)t;
        carry = t >> 32;
      }
      int idx = nlimbs + j;
      while (carry != 0) {
        uint64_t t = (uint64_t)p[idx] + carry;
        p[idx] = (uint32_t)t;
        carry = t >> 32;
        idx++;
      }
    }
    int plen = nlimbs + 2;
    int s = (int)(i1 - e);
    uint64_t int2 = (uint64_t)(p[s / 32] >> (s % 32));
    if (s % 32 > 30 && s / 32 + 1 < plen) int2 |= (uint64_t)p[s / 32 + 1] << (32 - s % 32);
    int2 &= 3;
    for (int idx = 0; idx < plen; idx++) {
      frac[idx] = p[idx];
      if (idx * 32 >= s) frac[idx] = 0;
      else if (idx * 32 + 32 > s) frac[idx] &= (1u << (s - idx * 32)) - 1;
    }
    _crm_mp f = _crm_mp_from_wide(0, frac, plen, -s);
    q = int2;
    if (_crm_mp_top_exp(&f) >= -1) {
      _crm_mp one = _crm_one();
      f = _crm_mp_sub(&f, &one);
      q += 1;
    }
    r = _crm_mp_mul(&f, &_CRM_MP_PI_2);
  }
  uint32_t qq = (uint32_t)(q & 3);
  if (x < 0.0) { *qout = (4 - qq) & 3; *rout = _crm_mp_neg(r); }
  else { *qout = qq; *rout = r; }
}

static void _crm_slow_sin_cos_small(const _crm_mp *r, _crm_mp *sout, _crm_mp *cout)
{
  _crm_mp r2 = _crm_mp_mul(r, r);
  _crm_mp s = *r, term = *r;
  for (uint32_t n = 1; n < 200; n++) {
    _crm_mp tt = _crm_mp_mul(&term, &r2);
    term = _crm_mp_neg(_crm_mp_div_u32(&tt, (2 * n) * (2 * n + 1)));
    if (_crm_negligible(&term, &s)) break;
    s = _crm_mp_add(&s, &term);
  }
  _crm_mp c = _crm_one();
  term = _crm_one();
  for (uint32_t n = 1; n < 200; n++) {
    _crm_mp tt = _crm_mp_mul(&term, &r2);
    term = _crm_mp_neg(_crm_mp_div_u32(&tt, (2 * n - 1) * (2 * n)));
    if (_crm_negligible(&term, &c)) break;
    c = _crm_mp_add(&c, &term);
  }
  *sout = s;
  *cout = c;
}

static void _crm_slow_sin_cos(double x, _crm_mp *sout, _crm_mp *cout)
{
  uint32_t q;
  _crm_mp r, s, c;
  _crm_reduce_pio2(x, &q, &r);
  _crm_slow_sin_cos_small(&r, &s, &c);
  switch (q) {
  case 0: *sout = s; *cout = c; break;
  case 1: *sout = c; *cout = _crm_mp_neg(s); break;
  case 2: *sout = _crm_mp_neg(s); *cout = _crm_mp_neg(c); break;
  default: *sout = _crm_mp_neg(c); *cout = s; break;
  }
}

static _crm_mp _crm_slow_atan(const _crm_mp *x)
{
  if (_crm_mp_is_zero(x)) return _CRM_MP_ZERO;
  _crm_mp one = _crm_one();
  _crm_mp ax = _crm_mp_abs(*x);
  int invert = _crm_mp_cmp_abs(&ax, &one) > 0;
  _crm_mp t = invert ? _crm_mp_recip(&ax) : ax;
  for (int i = 0; i < 3; i++) {
    _crm_mp tt = _crm_mp_mul(&t, &t);
    _crm_mp u = _crm_mp_add(&one, &tt);
    _crm_mp sq = _crm_mp_sqrt(&u);
    _crm_mp d = _crm_mp_add(&one, &sq);
    t = _crm_mp_div(&t, &d);
  }
  _crm_mp t2 = _crm_mp_mul(&t, &t);
  _crm_mp sum = t, pw = t;
  for (uint32_t n = 1; n < 400; n++) {
    pw = _crm_mp_neg(_crm_mp_mul(&pw, &t2));
    _crm_mp term = _crm_mp_div_u32(&pw, 2 * n + 1);
    if (_crm_negligible(&term, &sum)) break;
    sum = _crm_mp_add(&sum, &term);
  }
  _crm_mp a = _crm_mp_ldexp(sum, 3);
  if (invert) a = _crm_mp_sub(&_CRM_MP_PI_2, &a);
  return x->neg ? _crm_mp_neg(a) : a;
}

static _crm_mp _crm_slow_asin(const _crm_mp *x)
{
  _crm_mp one = _crm_one();
  _crm_mp half = _crm_mp_ldexp(one, -1);
  _crm_mp ax = _crm_mp_abs(*x);
  _crm_mp a;
  if (_crm_mp_cmp_abs(&ax, &half) <= 0) {
    _crm_mp xx = _crm_mp_mul(&ax, &ax);
    _crm_mp u = _crm_mp_sub(&one, &xx);
    _crm_mp d = _crm_mp_sqrt(&u);
    _crm_mp q = _crm_mp_div(&ax, &d);
    a = _crm_slow_atan(&q);
  } else {
    _crm_mp u = _crm_mp_ldexp(_crm_mp_sub(&one, &ax), -1);
    _crm_mp s = _crm_mp_sqrt(&u);
    _crm_mp ss = _crm_mp_mul(&s, &s);
    _crm_mp v = _crm_mp_sub(&one, &ss);
    _crm_mp d = _crm_mp_sqrt(&v);
    _crm_mp inner = _CRM_MP_ZERO;
    if (!_crm_mp_is_zero(&s)) {
      _crm_mp q = _crm_mp_div(&s, &d);
      inner = _crm_slow_atan(&q);
    }
    _crm_mp in2 = _crm_mp_ldexp(inner, 1);
    a = _crm_mp_sub(&_CRM_MP_PI_2, &in2);
  }
  return x->neg ? _crm_mp_neg(a) : a;
}

static _crm_mp _crm_slow_acos(const _crm_mp *x)
{
  _crm_mp one = _crm_one();
  _crm_mp half = _crm_mp_ldexp(one, -1);
  if (_crm_mp_cmp_abs(x, &half) <= 0) {
    _crm_mp as = _crm_slow_asin(x);
    return _crm_mp_sub(&_CRM_MP_PI_2, &as);
  }
  _crm_mp ax = _crm_mp_abs(*x);
  _crm_mp u = _crm_mp_ldexp(_crm_mp_sub(&one, &ax), -1);
  _crm_mp s = _crm_mp_sqrt(&u);
  _crm_mp two_asin = _crm_mp_ldexp(_crm_slow_asin(&s), 1);
  return x->neg ? _crm_mp_sub(&_CRM_MP_PI, &two_asin) : two_asin;
}

static _crm_mp _crm_slow_atan2(const _crm_mp *y, const _crm_mp *x)
{
  _crm_mp q = _crm_mp_div(y, x);
  _crm_mp a = _crm_slow_atan(&q);
  if (!x->neg) return a;
  if (!y->neg) return _crm_mp_add(&a, &_CRM_MP_PI);
  return _crm_mp_sub(&a, &_CRM_MP_PI);
}

static _crm_mp _crm_slow_sinh(const _crm_mp *x)
{
  if (_crm_mp_is_zero(x)) return _CRM_MP_ZERO;
  if (_crm_mp_top_exp(x) < -3) {
    _crm_mp x2 = _crm_mp_mul(x, x);
    _crm_mp s = *x, term = *x;
    for (uint32_t n = 1; n < 200; n++) {
      _crm_mp tt = _crm_mp_mul(&term, &x2);
      term = _crm_mp_div_u32(&tt, (2 * n) * (2 * n + 1));
      if (_crm_negligible(&term, &s)) break;
      s = _crm_mp_add(&s, &term);
    }
    return s;
  }
  _crm_mp e = _crm_slow_exp(x);
  _crm_mp ei = _crm_mp_recip(&e);
  return _crm_mp_ldexp(_crm_mp_sub(&e, &ei), -1);
}

static _crm_mp _crm_slow_cosh(const _crm_mp *x)
{
  _crm_mp e = _crm_slow_exp(x);
  _crm_mp ei = _crm_mp_recip(&e);
  return _crm_mp_ldexp(_crm_mp_add(&e, &ei), -1);
}

static _crm_mp _crm_slow_tanh(const _crm_mp *x)
{
  if (_crm_mp_is_zero(x)) return _CRM_MP_ZERO;
  _crm_mp one = _crm_one();
  if (_crm_mp_top_exp(x) < -3) {
    _crm_mp s = _crm_slow_sinh(x);
    _crm_mp ss = _crm_mp_mul(&s, &s);
    _crm_mp u = _crm_mp_add(&one, &ss);
    _crm_mp c = _crm_mp_sqrt(&u);
    return _crm_mp_div(&s, &c);
  }
  _crm_mp ax = _crm_mp_abs(*x);
  _crm_mp m2 = _crm_mp_neg(_crm_mp_ldexp(ax, 1));
  _crm_mp e = _crm_slow_exp(&m2);
  _crm_mp num = _crm_mp_sub(&one, &e);
  _crm_mp den = _crm_mp_add(&one, &e);
  _crm_mp t = _crm_mp_div(&num, &den);
  return x->neg ? _crm_mp_neg(t) : t;
}

static _crm_mp _crm_slow_pow_abs(double x, double y)
{
  _crm_mp xa = _crm_mp_from_f64(_crm_fabs(x));
  _crm_mp l = _crm_slow_ln(&xa);
  _crm_mp yy = _crm_mp_from_f64(y);
  _crm_mp z = _crm_mp_mul(&yy, &l);
  return _crm_slow_exp(&z);
}

/* ------------------------------------------------------------------------ */
/* Exact pow: |x|^y when it is a dyadic rational (a double or a midpoint    */
/* candidate), rounded exactly.                                              */
/* ------------------------------------------------------------------------ */

#define _CRM_BIG_LIMBS 220

typedef struct {
  int len;
  uint32_t w[_CRM_BIG_LIMBS];
} _crm_big;

static void _crm_big_set(_crm_big *b, uint64_t v)
{
  b->w[0] = (uint32_t)v;
  b->w[1] = (uint32_t)(v >> 32);
  b->len = b->w[1] ? 2 : 1;
}

/* r = a * c (r must not alias). */
static int _crm_big_mul(_crm_big *r, const _crm_big *a, const _crm_big *c)
{
  int n = a->len + c->len;
  if (n > _CRM_BIG_LIMBS) return 0;
  for (int i = 0; i < n; i++) r->w[i] = 0;
  for (int i = 0; i < a->len; i++) {
    uint64_t carry = 0;
    for (int j = 0; j < c->len; j++) {
      uint64_t t = (uint64_t)a->w[i] * c->w[j] + r->w[i + j] + carry;
      r->w[i + j] = (uint32_t)t;
      carry = t >> 32;
    }
    r->w[i + c->len] = (uint32_t)carry;
  }
  while (n > 1 && r->w[n - 1] == 0) n--;
  r->len = n;
  return 1;
}

static int _crm_big_bit_len(const _crm_big *b)
{
  uint32_t top = b->w[b->len - 1];
  return top == 0 ? 0 : (b->len - 1) * 32 + (32 - _crm_clz32(top));
}

static int _crm_big_bit(const _crm_big *b, int i)
{
  int w = i / 32;
  return w < b->len && ((b->w[w] >> (i % 32)) & 1u);
}

static int _crm_big_any_below(const _crm_big *b, int i)
{
  for (int k = 0; k < i; k++)
    if (_crm_big_bit(b, k)) return 1;
  return 0;
}

/* The double nearest to n * 2^e, n > 0. */
static double _crm_round_big(const _crm_big *n, int64_t e)
{
  int64_t len = _crm_big_bit_len(n);
  int64_t top = len - 1 + e;
  if (top >= 1024) return _CRM_INF;
  if (top < -1076) return 0.0;
  int normal = top >= -1022;
  int64_t q = normal ? top - 52 : -1074;
  int64_t shift = q - e;
  uint64_t k = 0;
  if (shift <= 0) {
    for (int64_t b = len - 1; b >= 0; b--) k = (k << 1) | (uint64_t)_crm_big_bit(n, (int)b);
    k <<= (int)(-shift);
  } else {
    for (int64_t b = len - 1; b >= shift; b--) k = (k << 1) | (uint64_t)_crm_big_bit(n, (int)b);
    int half = _crm_big_bit(n, (int)(shift - 1));
    int sticky = _crm_big_any_below(n, (int)(shift - 1));
    if (half && (sticky || (k & 1))) k += 1;
  }
  if (normal && k == (1ULL << 53)) { k >>= 1; q += 1; }
  if (k == 0) return 0.0;
  if (!normal || k < (1ULL << 52)) return _crm_from_bits(k);
  int64_t biased = q + 52 + 1023;
  if (biased >= 2047) return _CRM_INF;
  return _crm_from_bits(((uint64_t)biased << 52) | (k - (1ULL << 52)));
}

static uint64_t _crm_isqrt(uint64_t n)
{
  uint64_t r = (uint64_t)__builtin_sqrt((double)n);
  while (r * r > n) r--;
  while ((r + 1) * (r + 1) <= n) r++;
  return r;
}

/* |x|^y exactly rounded when it is a dyadic rational; 0 otherwise. */
static int _crm_pow_exact(double x, double y, double *out)
{
  uint64_t b = _crm_bits(x);
  int biased = (int)((b >> 52) & 0x7ff);
  uint64_t frac = b & ((1ULL << 52) - 1);
  uint64_t m;
  int64_t e;
  if (biased == 0) { m = frac; e = -1074; } else { m = frac | (1ULL << 52); e = biased - 1075; }
  while ((m & 1) == 0) { m >>= 1; e += 1; }
  uint32_t k = 0;
  double yy = y;
  while (!_crm_is_integer(yy)) {
    yy *= 2.0;
    k += 1;
    if (k > 60) return 0;
  }
  for (uint32_t i = 0; i < k; i++) {
    if (e % 2 != 0) return 0;
    uint64_t r = _crm_isqrt(m);
    if (r * r != m) return 0;
    m = r;
    e /= 2;
  }
  if (_crm_fabs(yy) > 1100.0) return 0;
  int64_t ni = (int64_t)yy;
  _crm_big acc, base, tmp;
  if (ni < 0) {
    if (m != 1) return 0;
    _crm_big_set(&acc, 1);
    *out = _crm_round_big(&acc, e * ni);
    return 1;
  }
  if (m > 1 && ni > 64) return 0;
  _crm_big_set(&acc, 1);
  _crm_big_set(&base, m);
  uint64_t p = (uint64_t)ni;
  while (p > 0) {
    if (p & 1) {
      if (!_crm_big_mul(&tmp, &acc, &base)) return 0;
      acc = tmp;
    }
    p >>= 1;
    if (p > 0) {
      if (!_crm_big_mul(&tmp, &base, &base)) return 0;
      base = tmp;
    }
  }
  *out = _crm_round_big(&acc, e * ni);
  return 1;
}

/* ------------------------------------------------------------------------ */
/* Double-double arithmetic (fast path)                                     */
/* ------------------------------------------------------------------------ */

static inline _crm_dd _crm_dd_make(double hi, double lo) { _crm_dd r; r.hi = hi; r.lo = lo; return r; }

static inline void _crm_two_sum(double a, double b, double *s, double *e)
{
  double x = a + b;
  double bb = x - a;
  *s = x;
  *e = (a - (x - bb)) + (b - bb);
}

static inline void _crm_fast_two_sum(double a, double b, double *s, double *e)
{
  double x = a + b;
  *s = x;
  *e = b - (x - a);
}

static inline void _crm_two_prod(double a, double b, double *p, double *e)
{
  const double c = 134217729.0; /* 2^27 + 1 */
  double t = c * a;
  double ah = t - (t - a);
  double al = a - ah;
  t = c * b;
  double bh = t - (t - b);
  double bl = b - bh;
  double pp = a * b;
  *p = pp;
  *e = ((ah * bh - pp) + ah * bl + al * bh) + al * bl;
}

static inline _crm_dd _crm_dd_neg(_crm_dd a) { return _crm_dd_make(-a.hi, -a.lo); }

static inline _crm_dd _crm_dd_add(_crm_dd a, _crm_dd b)
{
  double s, e, t, f;
  _crm_two_sum(a.hi, b.hi, &s, &e);
  _crm_two_sum(a.lo, b.lo, &t, &f);
  e = e + t;
  _crm_fast_two_sum(s, e, &s, &e);
  e = e + f;
  _crm_fast_two_sum(s, e, &s, &e);
  return _crm_dd_make(s, e);
}

static inline _crm_dd _crm_dd_sub(_crm_dd a, _crm_dd b) { return _crm_dd_add(a, _crm_dd_neg(b)); }

static inline _crm_dd _crm_dd_add_f(_crm_dd a, double b)
{
  double s, e;
  _crm_two_sum(a.hi, b, &s, &e);
  e = e + a.lo;
  _crm_fast_two_sum(s, e, &s, &e);
  return _crm_dd_make(s, e);
}

static inline _crm_dd _crm_dd_mul(_crm_dd a, _crm_dd b)
{
  double p, e;
  _crm_two_prod(a.hi, b.hi, &p, &e);
  e = e + (a.hi * b.lo + a.lo * b.hi);
  _crm_fast_two_sum(p, e, &p, &e);
  return _crm_dd_make(p, e);
}

static inline _crm_dd _crm_dd_mul_f(_crm_dd a, double b)
{
  double p, e;
  _crm_two_prod(a.hi, b, &p, &e);
  e = e + a.lo * b;
  _crm_fast_two_sum(p, e, &p, &e);
  return _crm_dd_make(p, e);
}

static inline _crm_dd _crm_dd_div(_crm_dd a, _crm_dd b)
{
  double q1 = a.hi / b.hi;
  _crm_dd r = _crm_dd_sub(a, _crm_dd_mul_f(b, q1));
  double q2 = r.hi / b.hi;
  r = _crm_dd_sub(r, _crm_dd_mul_f(b, q2));
  double q3 = r.hi / b.hi;
  double s, e;
  _crm_fast_two_sum(q1, q2, &s, &e);
  return _crm_dd_add_f(_crm_dd_make(s, e), q3);
}

static inline _crm_dd _crm_dd_sqrt(_crm_dd a)
{
  double s = __builtin_sqrt(a.hi);
  double p, e;
  _crm_two_prod(s, s, &p, &e);
  double r = ((a.hi - p) - e + a.lo) / (2.0 * s);
  double hi, lo;
  _crm_fast_two_sum(s, r, &hi, &lo);
  return _crm_dd_make(hi, lo);
}

/* The double nearest to the exact value when it lies within rel*|hi| of
   hi+lo and that decides the rounding: returns 1 and sets *out. */
static inline int _crm_dd_round(_crm_dd v, double rel, double *out)
{
  double err = 2.0 * rel * _crm_fabs(v.hi);
  double a = v.hi + (v.lo - err);
  double b = v.hi + (v.lo + err);
  if (a == b) { *out = a; return 1; }
  return 0;
}

static inline _crm_dd _crm_dd_tab(const double t[2]) { return _crm_dd_make(t[0], t[1]); }

static inline _crm_dd _crm_dd_scale(_crm_dd v, int m)
{
  double s = _crm_pow2(m);
  return _crm_dd_make(v.hi * s, v.lo * s);
}

/* ------------------------------------------------------------------------ */
/* Fast path                                                                */
/* ------------------------------------------------------------------------ */

#define _CRM_SHIFT 6755399441055744.0 /* 1.5 * 2^52 */
#define _CRM_EXP_ERR 0x1p-95
#define _CRM_LN_ERR 0x1p-92
#define _CRM_LOG_ERR 0x1p-91
#define _CRM_POW_ERR 0x1p-80
#define _CRM_TRIG_ERR 0x1p-95
#define _CRM_TAN_ERR 0x1p-94
#define _CRM_INV_TRIG_ERR 0x1p-93
#define _CRM_HYP_ERR 0x1p-93

/* e^(xh + xl) = v * 2^m, |xh| < 745, relative error below 2^-102. */
static _crm_dd _crm_exp_core(double xh, double xl, int *mout)
{
  double kf = (xh * _CRM_INV_LN2_128 + _CRM_SHIFT) - _CRM_SHIFT;
  int64_t k = (int64_t)kf;
  double r1 = xh - kf * _CRM_LN2_128[0];
  double s, e, p3, e3;
  _crm_two_sum(r1, -kf * _CRM_LN2_128[1], &s, &e);
  _crm_two_prod(kf, _CRM_LN2_128[2], &p3, &e3);
  _crm_dd r = _crm_dd_add(_crm_dd_make(s, e), _crm_dd_make(-p3, -e3));
  r = _crm_dd_add_f(r, xl);
  double rh = r.hi;
  double q = _CRM_INV_FACT[10][0];
  q = q * rh + _CRM_INV_FACT[9][0];
  q = q * rh + _CRM_INV_FACT[8][0];
  q = q * rh + _CRM_INV_FACT[7][0];
  q = q * rh + _CRM_INV_FACT[6][0];
  q = q * rh + _CRM_INV_FACT[5][0];
  double r2 = rh * rh;
  q = q * (r2 * r2 * rh);
  _crm_dd h = _crm_dd_tab(_CRM_INV_FACT[4]);
  h = _crm_dd_add(_crm_dd_mul(h, r), _crm_dd_tab(_CRM_INV_FACT[3]));
  h = _crm_dd_add_f(_crm_dd_mul(h, r), 0.5);
  h = _crm_dd_add_f(_crm_dd_mul(h, r), 1.0);
  h = _crm_dd_mul(h, r);
  _crm_dd p = _crm_dd_add_f(h, q);
  int j = (int)(k & 127);
  *mout = (int)(k >> 7);
  _crm_dd t = _crm_dd_tab(_CRM_EXP2_TAB[j]);
  return _crm_dd_add(t, _crm_dd_mul(t, p));
}

static int _crm_fast_exp(double x, double *out)
{
  if (!(x > -708.0 && x < 709.7)) return 0;
  int m;
  _crm_dd v = _crm_exp_core(x, 0.0, &m);
  double r;
  if (!_crm_dd_round(v, _CRM_EXP_ERR, &r)) return 0;
  *out = r * _crm_pow2(m);
  return 1;
}

static _crm_dd _crm_log1p_series(_crm_dd r, int dd_terms, int total)
{
  double rh = r.hi;
  double t = 0.0;
  for (int n = total; n >= dd_terms + 1; n--)
    t = t * rh + ((n % 2 == 1) ? 1.0 : -1.0) * _CRM_INV_N[n - 1][0];
  double pw = rh;
  for (int i = 0; i < dd_terms; i++) pw *= rh;
  double tail = t * pw;
  double sg = (dd_terms % 2 == 1) ? 1.0 : -1.0;
  _crm_dd h = _crm_dd_make(sg * _CRM_INV_N[dd_terms - 1][0], sg * _CRM_INV_N[dd_terms - 1][1]);
  for (int n = dd_terms - 1; n >= 1; n--) {
    double sn = (n % 2 == 1) ? 1.0 : -1.0;
    h = _crm_dd_add(_crm_dd_mul(h, r), _crm_dd_make(sn * _CRM_INV_N[n - 1][0], sn * _CRM_INV_N[n - 1][1]));
  }
  return _crm_dd_add_f(_crm_dd_mul(h, r), tail);
}

/* ln x for finite x > 0, x != 1, relative error below 2^-96. */
static _crm_dd _crm_ln_dd(double x)
{
  double xx = x;
  int eadj = 0;
  if (x < 2.2250738585072014e-308) { xx = x * 18014398509481984.0; eadj = -54; }
  if (xx > 0.9921875 && xx < 1.0078125)
    return _crm_log1p_series(_crm_dd_make(xx - 1.0, 0.0), 8, 16);
  uint64_t b = _crm_bits(xx);
  int e = (int)((b >> 52) & 0x7ff) - 1023 + eadj;
  double m = _crm_from_bits((b & ((1ULL << 52) - 1)) | (1023ULL << 52));
  int j = (int)((b >> 45) & 127);
  double ph, pl, rh, rl;
  _crm_two_prod(m, _CRM_LN_INV[j], &ph, &pl);
  _crm_fast_two_sum(ph - 1.0, pl, &rh, &rl);
  _crm_dd l1p = _crm_log1p_series(_crm_dd_make(rh, rl), 6, 13);
  double eh, el;
  _crm_two_prod((double)e, _CRM_DD_LN2_HI, &eh, &el);
  _crm_dd ell = _crm_dd_make(eh, el + (double)e * _CRM_DD_LN2_LO);
  return _crm_dd_add(_crm_dd_add(ell, _crm_dd_tab(_CRM_LN_TAB[j])), l1p);
}

static int _crm_fast_pow_abs(double ax, double y, double *out)
{
  _crm_dd l = _crm_ln_dd(ax);
  _crm_dd z = _crm_dd_mul_f(l, y);
  if (!(z.hi > -708.0 && z.hi < 709.7)) return 0;
  int m;
  _crm_dd v = _crm_exp_core(z.hi, z.lo, &m);
  double r;
  if (!_crm_dd_round(v, _CRM_POW_ERR, &r)) return 0;
  *out = r * _crm_pow2(m);
  return 1;
}

/* x = k pi/2 + r for |x| < 2^20 (refused when |r| < 2^-15 and k != 0). */
static int _crm_fast_reduce_pio2(double x, int64_t *kout, _crm_dd *rout)
{
  double kf = (x * _CRM_TWO_OVER_PI + _CRM_SHIFT) - _CRM_SHIFT;
  if (kf == 0.0) { *kout = 0; *rout = _crm_dd_make(x, 0.0); return 1; }
  double r1 = x - kf * _CRM_PIO2[0];
  double s, e, p4, e4;
  _crm_two_sum(r1, -kf * _CRM_PIO2[1], &s, &e);
  _crm_dd r = _crm_dd_add_f(_crm_dd_make(s, e), -kf * _CRM_PIO2[2]);
  _crm_two_prod(kf, _CRM_PIO2[3], &p4, &e4);
  r = _crm_dd_add(r, _crm_dd_make(-p4, -e4));
  if (_crm_fabs(r.hi) < 3.0517578125e-05) return 0;
  *kout = (int64_t)kf;
  *rout = r;
  return 1;
}

/* (sin r, cos r) for 0 <= r <= pi/4 + 2^-20. */
static void _crm_sin_cos_dd(_crm_dd r, _crm_dd *sout, _crm_dd *cout)
{
  int j = (int)(r.hi * 64.0 + 0.5);
  double a = (double)j / 64.0;
  double th, tl;
  _crm_fast_two_sum(r.hi - a, r.lo, &th, &tl);
  _crm_dd t = _crm_dd_make(th, tl);
  _crm_dd t2 = _crm_dd_mul(t, t);
  double u = t2.hi;
  double d = -_CRM_INV_FACT[7][0] + u * (_CRM_INV_FACT[9][0] - u * _CRM_INV_FACT[11][0]);
  _crm_dd h = _crm_dd_add(_crm_dd_tab(_CRM_INV_FACT[5]), _crm_dd_mul_f(t2, d));
  h = _crm_dd_add(_crm_dd_mul(t2, h), _crm_dd_neg(_crm_dd_tab(_CRM_INV_FACT[3])));
  h = _crm_dd_add_f(_crm_dd_mul(t2, h), 1.0);
  _crm_dd st = _crm_dd_mul(t, h);
  d = -_CRM_INV_FACT[6][0] + u * (_CRM_INV_FACT[8][0] + u * (-_CRM_INV_FACT[10][0] + u * _CRM_INV_FACT[12][0]));
  h = _crm_dd_add(_crm_dd_tab(_CRM_INV_FACT[4]), _crm_dd_mul_f(t2, d));
  h = _crm_dd_add_f(_crm_dd_mul(t2, h), -0.5);
  _crm_dd ct = _crm_dd_add_f(_crm_dd_mul(t2, h), 1.0);
  _crm_dd sa = _crm_dd_make(_CRM_SINCOS_TAB[j][0], _CRM_SINCOS_TAB[j][1]);
  _crm_dd ca = _crm_dd_make(_CRM_SINCOS_TAB[j][2], _CRM_SINCOS_TAB[j][3]);
  *sout = _crm_dd_add(_crm_dd_mul(sa, ct), _crm_dd_mul(ca, st));
  *cout = _crm_dd_sub(_crm_dd_mul(ca, ct), _crm_dd_mul(sa, st));
}

static int _crm_fast_sin_cos_x(double x, _crm_dd *sout, _crm_dd *cout)
{
  int64_t k;
  _crm_dd r, s, c;
  if (!_crm_fast_reduce_pio2(x, &k, &r)) return 0;
  int neg = r.hi < 0.0;
  _crm_dd ra = neg ? _crm_dd_neg(r) : r;
  _crm_sin_cos_dd(ra, &s, &c);
  if (neg) s = _crm_dd_neg(s);
  switch (k & 3) {
  case 0: *sout = s; *cout = c; break;
  case 1: *sout = c; *cout = _crm_dd_neg(s); break;
  case 2: *sout = _crm_dd_neg(s); *cout = _crm_dd_neg(c); break;
  default: *sout = _crm_dd_neg(c); *cout = s; break;
  }
  return 1;
}

/* atan(u) for a double-double u >= 0 (u.hi normal). */
static _crm_dd _crm_atan_dd(_crm_dd u)
{
  int inv = u.hi > 1.0;
  _crm_dd v = inv ? _crm_dd_div(_crm_dd_make(1.0, 0.0), u) : u;
  int j = (int)(v.hi * 64.0 + 0.5);
  _crm_dd t;
  if (j == 0) {
    t = v;
  } else {
    double c = (double)j / 64.0;
    double nh, nl;
    _crm_fast_two_sum(v.hi - c, v.lo, &nh, &nl);
    _crm_dd den = _crm_dd_add_f(_crm_dd_mul_f(v, c), 1.0);
    t = _crm_dd_div(_crm_dd_make(nh, nl), den);
  }
  _crm_dd t2 = _crm_dd_mul(t, t);
  double w = t2.hi;
  double d = _CRM_INV_N[8][0] + w * (-_CRM_INV_N[10][0] + w * (_CRM_INV_N[12][0] - w * _CRM_INV_N[14][0]));
  _crm_dd h = _crm_dd_add(_crm_dd_neg(_crm_dd_tab(_CRM_INV_N[6])), _crm_dd_mul_f(t2, d));
  h = _crm_dd_add(_crm_dd_mul(t2, h), _crm_dd_tab(_CRM_INV_N[4]));
  h = _crm_dd_add(_crm_dd_mul(t2, h), _crm_dd_neg(_crm_dd_tab(_CRM_INV_N[2])));
  h = _crm_dd_add_f(_crm_dd_mul(t2, h), 1.0);
  _crm_dd a = _crm_dd_add(_crm_dd_tab(_CRM_ATAN_TAB[j]), _crm_dd_mul(t, h));
  return inv ? _crm_dd_sub(_crm_dd_make(_CRM_DD_PI_2_HI, _CRM_DD_PI_2_LO), a) : a;
}

static _crm_dd _crm_sinh_series(double x)
{
  _crm_dd x2 = _crm_dd_mul(_crm_dd_make(x, 0.0), _crm_dd_make(x, 0.0));
  double u = x2.hi;
  double d = _CRM_INV_FACT[11][0]
      + u * (_CRM_INV_FACT[13][0] + u * (_CRM_INV_FACT[15][0] + u * (_CRM_INV_FACT[17][0] + u * _CRM_INV_FACT[19][0])));
  _crm_dd h = _crm_dd_add(_crm_dd_tab(_CRM_INV_FACT[9]), _crm_dd_mul_f(x2, d));
  h = _crm_dd_add(_crm_dd_mul(x2, h), _crm_dd_tab(_CRM_INV_FACT[7]));
  h = _crm_dd_add(_crm_dd_mul(x2, h), _crm_dd_tab(_CRM_INV_FACT[5]));
  h = _crm_dd_add(_crm_dd_mul(x2, h), _crm_dd_tab(_CRM_INV_FACT[3]));
  h = _crm_dd_add_f(_crm_dd_mul(x2, h), 1.0);
  return _crm_dd_mul_f(h, x);
}

/* ------------------------------------------------------------------------ */
/* Entry points                                                              */
/* ------------------------------------------------------------------------ */

#define _CRM_TINY 0x1p-30

static double _crm_exp(double x)
{
  double r;
  if (_crm_isnan(x)) return _CRM_NAN;
  if (x > 709.8) return _CRM_INF;
  if (x < -745.2) return 0.0;
  if (_crm_fast_exp(x, &r)) return r;
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_exp(&xm);
  return _crm_finish(&v);
}

/* kind: 0 ln, 1 log2, 2 log10 */
static double _crm_log_kind(double x, int kind)
{
  double r;
  if (_crm_isnan(x) || x < 0.0) return _CRM_NAN;
  if (x == 0.0) return -_CRM_INF;
  if (_crm_isinf(x)) return x;
  if (x == 1.0) return 0.0;
  _crm_dd l = _crm_ln_dd(x);
  if (kind == 0) {
    if (_crm_dd_round(l, _CRM_LN_ERR, &r)) return r;
  } else {
    _crm_dd c = kind == 1 ? _crm_dd_make(_CRM_DD_INV_LN2_HI, _CRM_DD_INV_LN2_LO)
                          : _crm_dd_make(_CRM_DD_INV_LN10_HI, _CRM_DD_INV_LN10_LO);
    if (_crm_dd_round(_crm_dd_mul(l, c), _CRM_LOG_ERR, &r)) return r;
  }
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_ln(&xm);
  if (kind == 1) v = _crm_mp_mul(&v, &_CRM_MP_INV_LN2);
  else if (kind == 2) v = _crm_mp_mul(&v, &_CRM_MP_INV_LN10);
  return _crm_finish(&v);
}

static double _crm_ln(double x) { return _crm_log_kind(x, 0); }
static double _crm_log2(double x) { return _crm_log_kind(x, 1); }
static double _crm_log10(double x) { return _crm_log_kind(x, 2); }

static double _crm_pow(double x, double y)
{
  if (y == 0.0 || x == 1.0) return 1.0;
  if (_crm_isnan(x) || _crm_isnan(y)) return _CRM_NAN;
  int y_odd = _crm_is_odd_integer(y);
  if (x == 0.0) {
    int neg = _crm_signbit(x) && y_odd;
    return _crm_with_sign(y < 0.0 ? _CRM_INF : 0.0, neg);
  }
  if (_crm_isinf(y)) {
    double ax = _crm_fabs(x);
    if (ax == 1.0) return 1.0;
    return ((ax < 1.0) == (y < 0.0)) ? _CRM_INF : 0.0;
  }
  if (_crm_isinf(x)) {
    int neg = x < 0.0 && y_odd;
    return _crm_with_sign(y < 0.0 ? 0.0 : _CRM_INF, neg);
  }
  if (x < 0.0 && !_crm_is_integer(y)) return _CRM_NAN;
  int neg = x < 0.0 && y_odd;
  double ax = _crm_fabs(x);
  if (ax == 1.0) return _crm_with_sign(1.0, neg);
  if (_crm_fabs(y) > 18446744073709551616.0)
    return _crm_with_sign(((ax > 1.0) == (y > 0.0)) ? _CRM_INF : 0.0, neg);
  double z = _crm_dd_mul_f(_crm_ln_dd(ax), y).hi;
  if (z > 709.79) return _crm_with_sign(_CRM_INF, neg);
  if (z < -745.14) return _crm_with_sign(0.0, neg);
  double r;
  if (_crm_fast_pow_abs(ax, y, &r)) return _crm_with_sign(r, neg);
  _crm_mp v = _crm_slow_pow_abs(ax, y);
  if (!_crm_mp_round(&v, _CRM_SLOW_ERR_BITS, &r)) {
    if (!_crm_pow_exact(ax, y, &r)) r = _crm_mp_round_nearest(&v);
  }
  return _crm_with_sign(r, neg);
}

static double _crm_sin(double x)
{
  double r;
  _crm_dd s, c;
  if (_crm_isnan(x) || _crm_isinf(x)) return _CRM_NAN;
  if (_crm_fabs(x) < _CRM_TINY) return x;
  if (_crm_fabs(x) < 1048576.0 && _crm_fast_sin_cos_x(x, &s, &c) && _crm_dd_round(s, _CRM_TRIG_ERR, &r)) return r;
  _crm_mp sm, cm;
  _crm_slow_sin_cos(x, &sm, &cm);
  return _crm_finish(&sm);
}

static double _crm_cos(double x)
{
  double r;
  _crm_dd s, c;
  if (_crm_isnan(x) || _crm_isinf(x)) return _CRM_NAN;
  if (_crm_fabs(x) < _CRM_TINY) return 1.0;
  if (_crm_fabs(x) < 1048576.0 && _crm_fast_sin_cos_x(x, &s, &c) && _crm_dd_round(c, _CRM_TRIG_ERR, &r)) return r;
  _crm_mp sm, cm;
  _crm_slow_sin_cos(x, &sm, &cm);
  return _crm_finish(&cm);
}

static double _crm_tan(double x)
{
  double r;
  _crm_dd s, c;
  if (_crm_isnan(x) || _crm_isinf(x)) return _CRM_NAN;
  if (_crm_fabs(x) < _CRM_TINY) return x;
  if (_crm_fabs(x) < 1048576.0 && _crm_fast_sin_cos_x(x, &s, &c) && _crm_dd_round(_crm_dd_div(s, c), _CRM_TAN_ERR, &r)) return r;
  _crm_mp sm, cm;
  _crm_slow_sin_cos(x, &sm, &cm);
  _crm_mp q = _crm_mp_div(&sm, &cm);
  return _crm_finish(&q);
}

static double _crm_asin(double x)
{
  double r;
  if (_crm_isnan(x) || _crm_fabs(x) > 1.0) return _CRM_NAN;
  double ax = _crm_fabs(x);
  if (ax < _CRM_TINY) return x;
  if (ax == 1.0) return _crm_with_sign(_CRM_DD_PI_2_HI, x < 0.0);
  _crm_dd d;
  if (ax >= 0.5) {
    double bh, bl;
    _crm_two_sum(1.0, ax, &bh, &bl);
    d = _crm_dd_mul_f(_crm_dd_make(bh, bl), 1.0 - ax);
  } else {
    double p, e;
    _crm_two_prod(ax, ax, &p, &e);
    d = _crm_dd_sub(_crm_dd_make(1.0, 0.0), _crm_dd_make(p, e));
  }
  _crm_dd q = _crm_dd_div(_crm_dd_make(ax, 0.0), _crm_dd_sqrt(d));
  if (_crm_dd_round(_crm_atan_dd(q), _CRM_INV_TRIG_ERR, &r)) return _crm_with_sign(r, x < 0.0);
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_asin(&xm);
  return _crm_finish(&v);
}

static double _crm_acos(double x)
{
  double r;
  if (_crm_isnan(x) || _crm_fabs(x) > 1.0) return _CRM_NAN;
  if (x == 1.0) return 0.0;
  if (x == -1.0) return _CRM_DD_PI_HI;
  _crm_dd v;
  if (_crm_fabs(x) <= 0.5) {
    double p, e;
    _crm_two_prod(x, x, &p, &e);
    _crm_dd d = _crm_dd_sub(_crm_dd_make(1.0, 0.0), _crm_dd_make(p, e));
    _crm_dd a = x == 0.0 ? _crm_dd_make(0.0, 0.0)
                         : _crm_atan_dd(_crm_dd_div(_crm_dd_make(_crm_fabs(x), 0.0), _crm_dd_sqrt(d)));
    _crm_dd as = x < 0.0 ? _crm_dd_neg(a) : a;
    v = _crm_dd_sub(_crm_dd_make(_CRM_DD_PI_2_HI, _CRM_DD_PI_2_LO), as);
  } else if (x > 0.5) {
    double dh, dl;
    _crm_two_sum(1.0, x, &dh, &dl);
    _crm_dd t = _crm_dd_sqrt(_crm_dd_div(_crm_dd_make(1.0 - x, 0.0), _crm_dd_make(dh, dl)));
    v = _crm_dd_mul_f(_crm_atan_dd(t), 2.0);
  } else {
    double dh, dl;
    _crm_two_sum(1.0, -x, &dh, &dl);
    _crm_dd t = _crm_dd_sqrt(_crm_dd_div(_crm_dd_make(1.0 + x, 0.0), _crm_dd_make(dh, dl)));
    v = _crm_dd_sub(_crm_dd_make(_CRM_DD_PI_HI, _CRM_DD_PI_LO), _crm_dd_mul_f(_crm_atan_dd(t), 2.0));
  }
  if (_crm_dd_round(v, _CRM_INV_TRIG_ERR, &r)) return r;
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp s = _crm_slow_acos(&xm);
  return _crm_finish(&s);
}

static double _crm_atan(double x)
{
  double r;
  if (_crm_isnan(x)) return _CRM_NAN;
  if (_crm_isinf(x)) return _crm_with_sign(_CRM_DD_PI_2_HI, x < 0.0);
  double ax = _crm_fabs(x);
  if (ax < _CRM_TINY) return x;
  if (ax > 1.152921504606847e18) return _crm_with_sign(_CRM_DD_PI_2_HI, x < 0.0);
  if (_crm_dd_round(_crm_atan_dd(_crm_dd_make(ax, 0.0)), _CRM_INV_TRIG_ERR, &r)) return _crm_with_sign(r, x < 0.0);
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_atan(&xm);
  return _crm_finish(&v);
}

static double _crm_atan2(double y, double x)
{
  double r;
  if (_crm_isnan(x) || _crm_isnan(y)) return _CRM_NAN;
  int yneg = _crm_signbit(y);
  if (y == 0.0) {
    if (x > 0.0 || (x == 0.0 && !_crm_signbit(x))) return y;
    return _crm_with_sign(_CRM_DD_PI_HI, yneg);
  }
  if (x == 0.0) return _crm_with_sign(_CRM_DD_PI_2_HI, yneg);
  if (_crm_isinf(y)) {
    double v;
    if (_crm_isinf(x)) {
      if (x > 0.0) {
        v = _CRM_DD_PI_4_HI;
      } else {
        _crm_mp h = _crm_mp_ldexp(_CRM_MP_PI_2, -1);
        _crm_mp t = _crm_mp_add(&_CRM_MP_PI_2, &h);
        v = _crm_finish(&t);
      }
    } else {
      v = _CRM_DD_PI_2_HI;
    }
    return _crm_with_sign(v, yneg);
  }
  if (_crm_isinf(x)) return _crm_with_sign(x > 0.0 ? 0.0 : _CRM_DD_PI_HI, yneg);
  int ey = (int)((_crm_bits(y) >> 52) & 0x7ff);
  int ex = (int)((_crm_bits(x) >> 52) & 0x7ff);
  int de = ey - ex;
  if (ey != 0 && ex != 0 && de <= 900 && de >= -900) {
    int emax = ey > ex ? ey : ex;
    int k = 1023 - emax;
    if (k < -1022) k = -1022;
    if (k > 1023) k = 1023;
    double sc = _crm_pow2(k);
    _crm_dd q = _crm_dd_div(_crm_dd_make(_crm_fabs(y) * sc, 0.0), _crm_dd_make(_crm_fabs(x) * sc, 0.0));
    _crm_dd a = _crm_atan_dd(q);
    _crm_dd v = x > 0.0 ? a : _crm_dd_sub(_crm_dd_make(_CRM_DD_PI_HI, _CRM_DD_PI_LO), a);
    if (_crm_dd_round(v, _CRM_INV_TRIG_ERR, &r)) return _crm_with_sign(r, y < 0.0);
  }
  _crm_mp ym = _crm_mp_from_f64(y), xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_atan2(&ym, &xm);
  return _crm_finish(&v);
}

static double _crm_sinh(double x)
{
  double r;
  if (_crm_isnan(x)) return _CRM_NAN;
  double ax = _crm_fabs(x);
  if (ax > 710.5) return _crm_with_sign(_CRM_INF, x < 0.0);
  if (ax < _CRM_TINY) return x;
  if (ax < 0.125) {
    if (_crm_dd_round(_crm_sinh_series(ax), _CRM_HYP_ERR, &r)) return _crm_with_sign(r, x < 0.0);
  } else if (ax <= 709.0) {
    int m;
    _crm_dd v = _crm_exp_core(ax, 0.0, &m);
    if (m > 60) {
      if (_crm_dd_round(v, _CRM_HYP_ERR, &r)) return _crm_with_sign(r * _crm_pow2(m - 1), x < 0.0);
    } else {
      _crm_dd inv = _crm_dd_div(_crm_dd_make(1.0, 0.0), v);
      _crm_dd s = _crm_dd_sub(_crm_dd_scale(v, m), _crm_dd_scale(inv, -m));
      if (_crm_dd_round(_crm_dd_make(s.hi * 0.5, s.lo * 0.5), _CRM_HYP_ERR, &r)) return _crm_with_sign(r, x < 0.0);
    }
  }
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_sinh(&xm);
  return _crm_finish(&v);
}

static double _crm_cosh(double x)
{
  double r;
  if (_crm_isnan(x)) return _CRM_NAN;
  double ax = _crm_fabs(x);
  if (ax > 710.5) return _CRM_INF;
  if (ax < _CRM_TINY) return 1.0;
  if (ax <= 709.0) {
    int m;
    _crm_dd v = _crm_exp_core(ax, 0.0, &m);
    if (m > 60) {
      if (_crm_dd_round(v, _CRM_HYP_ERR, &r)) return r * _crm_pow2(m - 1);
    } else {
      _crm_dd inv = _crm_dd_div(_crm_dd_make(1.0, 0.0), v);
      _crm_dd c = _crm_dd_add(_crm_dd_scale(v, m), _crm_dd_scale(inv, -m));
      if (_crm_dd_round(_crm_dd_make(c.hi * 0.5, c.lo * 0.5), _CRM_HYP_ERR, &r)) return r;
    }
  }
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp v = _crm_slow_cosh(&xm);
  return _crm_finish(&v);
}

static double _crm_tanh(double x)
{
  double r;
  if (_crm_isnan(x)) return _CRM_NAN;
  double ax = _crm_fabs(x);
  if (ax >= 22.0) return _crm_with_sign(1.0, x < 0.0);
  if (ax < _CRM_TINY) return x;
  _crm_dd v;
  if (ax < 0.125) {
    _crm_dd s = _crm_sinh_series(ax);
    _crm_dd c = _crm_dd_sqrt(_crm_dd_add_f(_crm_dd_mul(s, s), 1.0));
    v = _crm_dd_div(s, c);
  } else {
    int m;
    _crm_dd ev = _crm_exp_core(-2.0 * ax, 0.0, &m);
    _crm_dd e = _crm_dd_scale(ev, m);
    v = _crm_dd_div(_crm_dd_add_f(_crm_dd_neg(e), 1.0), _crm_dd_add_f(e, 1.0));
  }
  if (_crm_dd_round(v, _CRM_HYP_ERR, &r)) return _crm_with_sign(r, x < 0.0);
  _crm_mp xm = _crm_mp_from_f64(x);
  _crm_mp t = _crm_slow_tanh(&xm);
  return _crm_finish(&t);
}
