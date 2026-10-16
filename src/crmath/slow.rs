//! The slow path: every function evaluated in 352-bit arithmetic.
//!
//! Each result is accurate to far better than 2^-250 relative (the series
//! run to 2^-360 and the few hundred operations each lose at most a few
//! bits), which [`SLOW_ERR_BITS`] states. The values of these functions at
//! rational arguments (other than the trivial ones the callers handle) are
//! transcendental, so they are never a double or a midpoint between two
//! doubles, and the known hardest-to-round arguments need well under 200
//! bits: at this accuracy the rounding is always decided. `pow` is the
//! exception (its value can be a double or a midpoint); its caller checks
//! those cases exactly.

use super::mp::Mp;
use super::tables::*;
use std::cmp::Ordering;

/// The relative accuracy every slow result is rounded with.
pub(crate) const SLOW_ERR_BITS: u32 = 250;

/// Series stop once a term is this far below the running sum.
const SERIES_BITS: i32 = 360;

fn one() -> Mp {
    Mp::from_u64(1)
}

fn negligible(term: &Mp, sum: &Mp) -> bool {
    term.is_zero() || (!sum.is_zero() && term.top_exp() < sum.top_exp() - SERIES_BITS)
}

/// e^x for an x of moderate size (|x| < 2^20).
pub(crate) fn exp(x: &Mp) -> Mp {
    let k = (x.approx_f64() * std::f64::consts::LOG2_E).round() as i64;
    let r = Mp::sub(x, &Mp::mul(&Mp::from_i64(k), &MP_LN2));
    // |r| <= ~0.35; t = r / 2^12, e^r = (e^t)^(2^12).
    const SQUARINGS: i32 = 12;
    let t = r.ldexp(-SQUARINGS);
    let mut sum = one();
    let mut term = one();
    for n in 1..200u32 {
        term = Mp::div_u32(&Mp::mul(&term, &t), n);
        if negligible(&term, &sum) {
            break;
        }
        sum = Mp::add(&sum, &term);
    }
    for _ in 0..SQUARINGS {
        sum = Mp::mul(&sum, &sum);
    }
    sum.ldexp(k as i32)
}

/// ln(x) for x > 0.
pub(crate) fn ln(x: &Mp) -> Mp {
    // x = m * 2^e with m in [sqrt(2)/2, sqrt(2)).
    let mut e = x.top_exp();
    let mut m = x.ldexp(-e);
    if m.approx_f64() > std::f64::consts::SQRT_2 {
        m = m.ldexp(-1);
        e += 1;
    }
    // ln(m) = 2 atanh(t), t = (m - 1) / (m + 1), |t| <= 0.172.
    let t = Mp::div(&Mp::sub(&m, &one()), &Mp::add(&m, &one()));
    let t2 = Mp::mul(&t, &t);
    let mut sum = t;
    let mut pw = t;
    for n in 1..400u32 {
        pw = Mp::mul(&pw, &t2);
        let term = Mp::div_u32(&pw, 2 * n + 1);
        if negligible(&term, &sum) {
            break;
        }
        sum = Mp::add(&sum, &term);
    }
    Mp::add(&Mp::mul(&Mp::from_i64(e as i64), &MP_LN2), &sum.ldexp(1))
}

/// Bit `i` (1-based, after the binary point) of 2/pi.
fn two_over_pi_bit(i: usize) -> u32 {
    let w = (i - 1) / 32;
    let b = 31 - ((i - 1) % 32);
    (TWO_OVER_PI_BITS[w] >> b) & 1
}

/// x = q * pi/2 + r with |r| <= pi/4 (q taken modulo 4), for finite x.
pub(crate) fn reduce_pio2(x: f64) -> (u32, Mp) {
    let ax = x.abs();
    let (q, r) = if ax < 1048576.0 {
        let q = (ax * std::f64::consts::FRAC_2_PI).round() as i64;
        let r = Mp::sub(&Mp::from_f64(ax), &Mp::mul(&Mp::from_i64(q), &MP_PI_2));
        (q as u64, r)
    } else {
        // Payne-Hanek: ax = mant * 2^e (mant a 53-bit integer). Bits of 2/pi
        // before position e-1 contribute multiples of 4 to ax*2/pi.
        let bits = ax.to_bits();
        let mant = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        let e = ((bits >> 52) & 0x7ff) as i64 - 1075;
        let i0 = (e - 1).max(1) as usize;
        let i1 = (e + 53 + 450) as usize;
        // W = bits i0..=i1 of 2/pi as an integer (big-endian bit order).
        let nbits = i1 - i0 + 1;
        let nlimbs = nbits.div_ceil(32) + 1;
        let mut wv = vec![0u32; nlimbs];
        for (k, i) in (i0..=i1).enumerate() {
            // bit i has weight 2^(i1 - i) in W
            let pos = nbits - 1 - k;
            wv[pos / 32] |= two_over_pi_bit(i) << (pos % 32);
        }
        // P = mant * W
        let mut p = vec![0u32; nlimbs + 2];
        for (j, m) in [mant as u32, (mant >> 32) as u32].iter().enumerate() {
            let mut carry = 0u64;
            for i in 0..nlimbs {
                let t = *m as u64 * wv[i] as u64 + p[i + j] as u64 + carry;
                p[i + j] = t as u32;
                carry = t >> 32;
            }
            let mut idx = nlimbs + j;
            while carry != 0 {
                let t = p[idx] as u64 + carry;
                p[idx] = t as u32;
                carry = t >> 32;
                idx += 1;
            }
        }
        // ax*2/pi = P * 2^(e - i1): the point sits `s` bits up.
        let s = (i1 as i64 - e) as usize;
        let int2 = ((p[s / 32] >> (s % 32)) as u64
            | if s % 32 > 30 && s / 32 + 1 < p.len() {
                (p[s / 32 + 1] as u64) << (32 - s % 32)
            } else {
                0
            })
            & 3;
        // fraction: P mod 2^s, scaled to an Mp in [0, 1)
        let mut frac = p.clone();
        for (idx, w) in frac.iter_mut().enumerate() {
            if idx * 32 >= s {
                *w = 0;
            } else if idx * 32 + 32 > s {
                *w &= (1u32 << (s - idx * 32)) - 1;
            }
        }
        let mut f = Mp::from_wide(false, &mut frac, -(s as i32));
        let mut q = int2;
        // f in [0,1): move to [-1/2, 1/2)
        if f.top_exp() >= -1 {
            f = Mp::sub(&f, &one());
            q += 1;
        }
        (q, Mp::mul(&f, &MP_PI_2))
    };
    let q = (q & 3) as u32;
    if x < 0.0 {
        ((4 - q) & 3, r.neg())
    } else {
        (q, r)
    }
}

/// (sin r, cos r) for |r| <= pi/4.
pub(crate) fn sin_cos_small(r: &Mp) -> (Mp, Mp) {
    let r2 = Mp::mul(r, r);
    let mut s = *r;
    let mut term = *r;
    for n in 1..200u32 {
        term = Mp::div_u32(&Mp::mul(&term, &r2), (2 * n) * (2 * n + 1)).neg();
        if negligible(&term, &s) {
            break;
        }
        s = Mp::add(&s, &term);
    }
    let mut c = one();
    let mut term = one();
    for n in 1..200u32 {
        term = Mp::div_u32(&Mp::mul(&term, &r2), (2 * n - 1) * (2 * n)).neg();
        if negligible(&term, &c) {
            break;
        }
        c = Mp::add(&c, &term);
    }
    (s, c)
}

/// (sin x, cos x) for finite x.
pub(crate) fn sin_cos(x: f64) -> (Mp, Mp) {
    let (q, r) = reduce_pio2(x);
    let (s, c) = sin_cos_small(&r);
    match q {
        0 => (s, c),
        1 => (c, s.neg()),
        2 => (s.neg(), c.neg()),
        _ => (c.neg(), s),
    }
}

/// atan(x).
pub(crate) fn atan(x: &Mp) -> Mp {
    if x.is_zero() {
        return Mp::ZERO;
    }
    let ax = x.abs();
    let invert = Mp::cmp_abs(&ax, &one()) == Ordering::Greater;
    let mut t = if invert { ax.recip() } else { ax };
    // atan(t) = 2 atan(t / (1 + sqrt(1 + t^2))), three times.
    for _ in 0..3 {
        let d = Mp::add(&one(), &Mp::add(&one(), &Mp::mul(&t, &t)).sqrt());
        t = Mp::div(&t, &d);
    }
    let t2 = Mp::mul(&t, &t);
    let mut sum = t;
    let mut pw = t;
    for n in 1..400u32 {
        pw = Mp::mul(&pw, &t2).neg();
        let term = Mp::div_u32(&pw, 2 * n + 1);
        if negligible(&term, &sum) {
            break;
        }
        sum = Mp::add(&sum, &term);
    }
    let mut a = sum.ldexp(3);
    if invert {
        a = Mp::sub(&MP_PI_2, &a);
    }
    if x.neg { a.neg() } else { a }
}

/// asin(x) for |x| <= 1.
pub(crate) fn asin(x: &Mp) -> Mp {
    let ax = x.abs();
    let half = one().ldexp(-1);
    let a = if Mp::cmp_abs(&ax, &half) != Ordering::Greater {
        // atan(x / sqrt(1 - x^2))
        let d = Mp::sub(&one(), &Mp::mul(&ax, &ax)).sqrt();
        atan(&Mp::div(&ax, &d))
    } else {
        // pi/2 - 2 asin(sqrt((1 - x) / 2))
        let s = Mp::sub(&one(), &ax).ldexp(-1).sqrt();
        let d = Mp::sub(&one(), &Mp::mul(&s, &s)).sqrt();
        let inner = if s.is_zero() {
            Mp::ZERO
        } else {
            atan(&Mp::div(&s, &d))
        };
        Mp::sub(&MP_PI_2, &inner.ldexp(1))
    };
    if x.neg { a.neg() } else { a }
}

/// acos(x) for |x| <= 1.
pub(crate) fn acos(x: &Mp) -> Mp {
    let half = one().ldexp(-1);
    if Mp::cmp_abs(x, &half) != Ordering::Greater {
        return Mp::sub(&MP_PI_2, &asin(x));
    }
    // s = sqrt((1 - |x|) / 2); acos(|x|) = 2 asin(s)
    let ax = x.abs();
    let s = Mp::sub(&one(), &ax).ldexp(-1).sqrt();
    let two_asin = asin(&s).ldexp(1);
    if x.neg {
        Mp::sub(&MP_PI, &two_asin)
    } else {
        two_asin
    }
}

/// atan2(y, x) for finite nonzero x and y.
pub(crate) fn atan2(y: &Mp, x: &Mp) -> Mp {
    let a = atan(&Mp::div(y, x));
    if !x.neg {
        a
    } else if !y.neg {
        Mp::add(&a, &MP_PI)
    } else {
        Mp::sub(&a, &MP_PI)
    }
}

/// sinh(x) (|x| < 2^20).
pub(crate) fn sinh(x: &Mp) -> Mp {
    if x.is_zero() {
        return Mp::ZERO;
    }
    if x.top_exp() < -3 {
        let x2 = Mp::mul(x, x);
        let mut s = *x;
        let mut term = *x;
        for n in 1..200u32 {
            term = Mp::div_u32(&Mp::mul(&term, &x2), (2 * n) * (2 * n + 1));
            if negligible(&term, &s) {
                break;
            }
            s = Mp::add(&s, &term);
        }
        return s;
    }
    let e = exp(x);
    Mp::sub(&e, &e.recip()).ldexp(-1)
}

/// cosh(x) (|x| < 2^20).
pub(crate) fn cosh(x: &Mp) -> Mp {
    let e = exp(x);
    Mp::add(&e, &e.recip()).ldexp(-1)
}

/// tanh(x).
pub(crate) fn tanh(x: &Mp) -> Mp {
    if x.is_zero() {
        return Mp::ZERO;
    }
    if x.top_exp() < -3 {
        // cosh = sqrt(1 + sinh^2)
        let s = sinh(x);
        let c = Mp::add(&one(), &Mp::mul(&s, &s)).sqrt();
        return Mp::div(&s, &c);
    }
    // (1 - E) / (1 + E), E = e^(-2|x|)
    let ax = x.abs();
    let e = exp(&ax.ldexp(1).neg());
    let t = Mp::div(&Mp::sub(&one(), &e), &Mp::add(&one(), &e));
    if x.neg { t.neg() } else { t }
}

/// |x|^y = e^(y ln|x|) for finite |x| > 0 and finite y, with |y ln|x|| <
/// 2^20.
pub(crate) fn pow_abs(x: f64, y: f64) -> Mp {
    let l = ln(&Mp::from_f64(x.abs()));
    exp(&Mp::mul(&Mp::from_f64(y), &l))
}
