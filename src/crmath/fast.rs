//! The fast path: double-double evaluation with an error bound, rounded
//! only when the bound decides the rounding (Ziv's strategy). A `None`
//! sends the call to the slow path.
//!
//! Each function states the relative error bound its rounding test uses;
//! the evaluations are accurate to about 2^-102 or better, so the bounds
//! leave a margin of at least 2^7 (the error terms are listed with each
//! function).

use super::dd::{Dd, fast_two_sum, two_prod, two_sum};
use super::tables::*;

/// 1.5 * 2^52: `(v + SHIFT) - SHIFT` rounds v (|v| < 2^51) to an integer.
const SHIFT: f64 = 6755399441055744.0;

const EXP_ERR: f64 = 2.524354896707238e-29; // 2^-95
const LN_ERR: f64 = 2.0194839173657902e-28; // 2^-92
const LOG_ERR: f64 = 4.0389678347315804e-28; // 2^-91
const POW_ERR: f64 = 8.271806125530277e-25; // 2^-80
const TRIG_ERR: f64 = 2.524354896707238e-29; // 2^-95
const TAN_ERR: f64 = 5.048709793414476e-29; // 2^-94
const INV_TRIG_ERR: f64 = 1.0097419586828951e-28; // 2^-93
const HYP_ERR: f64 = 1.0097419586828951e-28; // 2^-93

#[inline]
fn dd(t: (f64, f64)) -> Dd {
    Dd::new(t.0, t.1)
}

/// 2^m for -1022 <= m <= 1023.
#[inline]
fn pow2(m: i32) -> f64 {
    f64::from_bits(((m + 1023) as u64) << 52)
}

#[inline]
fn scale(v: Dd, m: i32) -> Dd {
    let s = pow2(m);
    Dd::new(v.hi * s, v.lo * s)
}

#[inline]
fn with_sign(v: f64, neg: bool) -> f64 {
    if neg { -v } else { v }
}

// ---------------------------------------------------------------------
// exp
// ---------------------------------------------------------------------

/// e^(xh + xl) = v * 2^m for |xh| < 745, v within [0.99, 2.01] and a
/// relative error below 2^-102:
///   x = k ln2/128 + r, the reduction exact to 2^-150 (the three pieces of
///   ln2/128, k < 2^18); 2^(j/128) from the table (2^-106); e^r - 1 by its
///   Taylor series to r^10 (truncation 2^-107, |r| < 2^-8.5), the r^5..r^10
///   terms in double (2^-101.5 of a term below 2^-49.4); a dozen
///   double-double operations of about 2^-105 each.
pub(crate) fn exp_core(xh: f64, xl: f64) -> (Dd, i32) {
    let kf = (xh * INV_LN2_128 + SHIFT) - SHIFT;
    let k = kf as i64;
    let r1 = xh - kf * LN2_128[0];
    let (s, e) = two_sum(r1, -kf * LN2_128[1]);
    let (p3, e3) = two_prod(kf, LN2_128[2]);
    let r = Dd::add(Dd::new(s, e), Dd::new(-p3, -e3));
    let r = Dd::add_f(r, xl);
    let rh = r.hi;
    // r^5/5! + ... + r^10/10! in double
    let q = INV_FACT[10].0;
    let q = q * rh + INV_FACT[9].0;
    let q = q * rh + INV_FACT[8].0;
    let q = q * rh + INV_FACT[7].0;
    let q = q * rh + INV_FACT[6].0;
    let q = q * rh + INV_FACT[5].0;
    let r2 = rh * rh;
    let q = q * (r2 * r2 * rh);
    // r + r^2/2 + r^3/6 + r^4/24 in double-double
    let mut h = dd(INV_FACT[4]);
    h = Dd::add(Dd::mul(h, r), dd(INV_FACT[3]));
    h = Dd::add_f(Dd::mul(h, r), 0.5);
    h = Dd::add_f(Dd::mul(h, r), 1.0);
    h = Dd::mul(h, r);
    let p = Dd::add_f(h, q);
    let j = (k & 127) as usize;
    let m = (k >> 7) as i32;
    let t = dd(EXP2_TAB[j]);
    (Dd::add(t, Dd::mul(t, p)), m)
}

pub(crate) fn exp(x: f64) -> Option<f64> {
    if !(x > -708.0 && x < 709.7) {
        return None;
    }
    let (v, m) = exp_core(x, 0.0);
    Some(v.round(EXP_ERR)? * pow2(m))
}

// ---------------------------------------------------------------------
// ln, log2, log10
// ---------------------------------------------------------------------

/// The series of ln(1 + r): `dd_terms` terms in double-double, the rest up
/// to `total` in double.
#[inline]
fn log1p_series(r: Dd, dd_terms: usize, total: usize) -> Dd {
    let rh = r.hi;
    let sgn = |n: usize| if n % 2 == 1 { 1.0 } else { -1.0 };
    let mut t = 0.0;
    for n in (dd_terms + 1..=total).rev() {
        t = t * rh + sgn(n) * INV_N[n - 1].0;
    }
    let mut pw = rh;
    for _ in 0..dd_terms {
        pw *= rh;
    }
    let tail = t * pw;
    let mut h = Dd::new(
        sgn(dd_terms) * INV_N[dd_terms - 1].0,
        sgn(dd_terms) * INV_N[dd_terms - 1].1,
    );
    for n in (1..dd_terms).rev() {
        let c = Dd::new(sgn(n) * INV_N[n - 1].0, sgn(n) * INV_N[n - 1].1);
        h = Dd::add(Dd::mul(h, r), c);
    }
    Dd::add_f(Dd::mul(h, r), tail)
}

/// ln x for finite x > 0, x != 1, relative error below 2^-96:
///   near 1 (|x - 1| < 2^-7): r = x - 1 exactly, the series to r^16
///   (truncation 2^-116, double terms from r^9 at 2^-119);
///   elsewhere x = 2^e m, m/c_j = 1 + r with |r| < 2^-7.99 exactly (24-bit
///   1/c_j times m in double-double), ln(1+r) to r^13 (2^-107), -ln(1/c_j)
///   from the table (2^-106), e ln2 in double-double. The sum cancels by at
///   most 7 bits (x in [1/2, 1 - 2^-7]), so 2^-104 * 2^7 bounds it.
pub(crate) fn ln_dd(x: f64) -> Dd {
    let (xx, eadj) = if x < 2.2250738585072014e-308 {
        (x * 18014398509481984.0, -54) // 2^54
    } else {
        (x, 0)
    };
    if xx > 0.9921875 && xx < 1.0078125 {
        return log1p_series(Dd::from_f64(xx - 1.0), 8, 16);
    }
    let bits = xx.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32 - 1023 + eadj;
    let m = f64::from_bits((bits & ((1u64 << 52) - 1)) | (1023u64 << 52));
    let j = ((bits >> 45) & 127) as usize;
    let (ph, pl) = two_prod(m, LN_INV[j]);
    let (rh, rl) = fast_two_sum(ph - 1.0, pl);
    let l1p = log1p_series(Dd::new(rh, rl), 6, 13);
    let (eh, el) = two_prod(e as f64, DD_LN2.0);
    let el = Dd::new(eh, el + e as f64 * DD_LN2.1);
    Dd::add(Dd::add(el, dd(LN_TAB[j])), l1p)
}

pub(crate) fn ln(x: f64) -> Option<f64> {
    ln_dd(x).round(LN_ERR)
}

pub(crate) fn log2(x: f64) -> Option<f64> {
    Dd::mul(ln_dd(x), dd(DD_INV_LN2)).round(LOG_ERR)
}

pub(crate) fn log10(x: f64) -> Option<f64> {
    Dd::mul(ln_dd(x), dd(DD_INV_LN10)).round(LOG_ERR)
}

// ---------------------------------------------------------------------
// pow
// ---------------------------------------------------------------------

/// |x|^y for finite |x| > 0 and finite y with the result in the normal
/// range: z = y ln|x| carries the 2^-96 relative error of ln, an absolute
/// error below 709 * 2^-96 < 2^-86 that e^z turns into a relative one; the
/// bound 2^-80 leaves room for the rest.
/// y ln|x| (its leading double).
pub(crate) fn pow_exponent(ax: f64, y: f64) -> f64 {
    Dd::mul_f(ln_dd(ax), y).hi
}

pub(crate) fn pow_abs(ax: f64, y: f64) -> Option<f64> {
    let l = ln_dd(ax);
    let z = Dd::mul_f(l, y);
    if !(z.hi > -708.0 && z.hi < 709.7) {
        return None;
    }
    let (v, m) = exp_core(z.hi, z.lo);
    Some(v.round(POW_ERR)? * pow2(m))
}

// ---------------------------------------------------------------------
// sin, cos, tan
// ---------------------------------------------------------------------

/// x = k pi/2 + r for |x| < 2^20, with r in double-double. pi/2 is split
/// into 33, 33, 33 and 53 bits (k pi/2 exact to 2^-132); the reduction is
/// refused when |r| < 2^-15 and k != 0, where the cancellation would eat
/// into the 2^-104 budget.
#[inline]
fn reduce_pio2(x: f64) -> Option<(i64, Dd)> {
    let kf = (x * TWO_OVER_PI + SHIFT) - SHIFT;
    if kf == 0.0 {
        return Some((0, Dd::from_f64(x)));
    }
    let r1 = x - kf * PIO2[0];
    let (s, e) = two_sum(r1, -kf * PIO2[1]);
    let r = Dd::add_f(Dd::new(s, e), -kf * PIO2[2]);
    let (p4, e4) = two_prod(kf, PIO2[3]);
    let r = Dd::add(r, Dd::new(-p4, -e4));
    if r.hi.abs() < 3.0517578125e-05 {
        return None;
    }
    Some((kf as i64, r))
}

/// (sin r, cos r) for 0 <= r <= pi/4 + 2^-20, relative errors below
/// 2^-102: r = j/64 + t (|t| <= 1/128), the table values of sin and cos at
/// j/64 (2^-106), the Taylor series of sin t to t^11 and cos t to t^12
/// (truncation 2^-116), the addition formulas (cancellation at most one
/// bit).
#[inline]
fn sin_cos_dd(r: Dd) -> (Dd, Dd) {
    let j = (r.hi * 64.0 + 0.5) as usize;
    let a = j as f64 / 64.0;
    let (th, tl) = fast_two_sum(r.hi - a, r.lo);
    let t = Dd::new(th, tl);
    let t2 = Dd::mul(t, t);
    let u = t2.hi;
    // sin t = t (1 + t2 (-1/3! + t2 (1/5! + t2 d)))
    let d = -INV_FACT[7].0 + u * (INV_FACT[9].0 - u * INV_FACT[11].0);
    let mut h = Dd::add(dd(INV_FACT[5]), Dd::mul_f(t2, d));
    h = Dd::add(Dd::mul(t2, h), dd(INV_FACT[3]).neg());
    h = Dd::add_f(Dd::mul(t2, h), 1.0);
    let st = Dd::mul(t, h);
    // cos t = 1 + t2 (-1/2 + t2 (1/4! + t2 d))
    let d = -INV_FACT[6].0 + u * (INV_FACT[8].0 + u * (-INV_FACT[10].0 + u * INV_FACT[12].0));
    let mut h = Dd::add(dd(INV_FACT[4]), Dd::mul_f(t2, d));
    h = Dd::add_f(Dd::mul(t2, h), -0.5);
    let ct = Dd::add_f(Dd::mul(t2, h), 1.0);
    let ((sh, sl), (ch, cl)) = SINCOS_TAB[j];
    let sa = Dd::new(sh, sl);
    let ca = Dd::new(ch, cl);
    let s = Dd::add(Dd::mul(sa, ct), Dd::mul(ca, st));
    let c = Dd::sub(Dd::mul(ca, ct), Dd::mul(sa, st));
    (s, c)
}

/// (sin x, cos x) in double-double for 2^-30 <= |x| < 2^20.
#[inline]
fn sin_cos_x(x: f64) -> Option<(Dd, Dd)> {
    let (k, r) = reduce_pio2(x)?;
    let neg = r.hi < 0.0;
    let ra = if neg { r.neg() } else { r };
    let (s, c) = sin_cos_dd(ra);
    let s = if neg { s.neg() } else { s };
    Some(match k & 3 {
        0 => (s, c),
        1 => (c, s.neg()),
        2 => (s.neg(), c.neg()),
        _ => (c.neg(), s),
    })
}

pub(crate) fn sin(x: f64) -> Option<f64> {
    if x.abs() >= 1048576.0 {
        return None;
    }
    sin_cos_x(x)?.0.round(TRIG_ERR)
}

pub(crate) fn cos(x: f64) -> Option<f64> {
    if x.abs() >= 1048576.0 {
        return None;
    }
    sin_cos_x(x)?.1.round(TRIG_ERR)
}

pub(crate) fn tan(x: f64) -> Option<f64> {
    if x.abs() >= 1048576.0 {
        return None;
    }
    let (s, c) = sin_cos_x(x)?;
    Dd::div(s, c).round(TAN_ERR)
}

// ---------------------------------------------------------------------
// atan, atan2, asin, acos
// ---------------------------------------------------------------------

/// atan(u) for a double-double u >= 0 (u.hi normal), relative error below
/// 2^-101: u > 1 goes through pi/2 - atan(1/u); v = j/64 + ...,
/// atan(v) = atan(j/64) + atan(t), t = (v - c)/(1 + v c), |t| <= 2^-7;
/// the series of atan t to t^15 (truncation 2^-116).
#[inline]
fn atan_dd(u: Dd) -> Dd {
    let inv = u.hi > 1.0;
    let v = if inv {
        Dd::div(Dd::from_f64(1.0), u)
    } else {
        u
    };
    let j = (v.hi * 64.0 + 0.5) as usize;
    let t = if j == 0 {
        v
    } else {
        let c = j as f64 / 64.0;
        let (nh, nl) = fast_two_sum(v.hi - c, v.lo);
        let den = Dd::add_f(Dd::mul_f(v, c), 1.0);
        Dd::div(Dd::new(nh, nl), den)
    };
    let t2 = Dd::mul(t, t);
    let w = t2.hi;
    // atan t = t (1 + t2 (-1/3 + t2 (1/5 + t2 (-1/7 + t2 d))))
    let d = INV_N[8].0 + w * (-INV_N[10].0 + w * (INV_N[12].0 - w * INV_N[14].0));
    let mut h = Dd::add(dd(INV_N[6]).neg(), Dd::mul_f(t2, d));
    h = Dd::add(Dd::mul(t2, h), dd(INV_N[4]));
    h = Dd::add(Dd::mul(t2, h), dd(INV_N[2]).neg());
    h = Dd::add_f(Dd::mul(t2, h), 1.0);
    let a = Dd::add(dd(ATAN_TAB[j]), Dd::mul(t, h));
    if inv { Dd::sub(dd(DD_PI_2), a) } else { a }
}

pub(crate) fn atan(x: f64) -> Option<f64> {
    let ax = x.abs();
    if ax > 1.152921504606847e18 {
        // |x| > 2^60: pi/2 - 1/|x| rounds to RN(pi/2)
        return Some(with_sign(DD_PI_2.0, x < 0.0));
    }
    Some(with_sign(
        atan_dd(Dd::from_f64(ax)).round(INV_TRIG_ERR)?,
        x < 0.0,
    ))
}

/// atan2 for finite nonzero x and y.
pub(crate) fn atan2(y: f64, x: f64) -> Option<f64> {
    let ey = ((y.to_bits() >> 52) & 0x7ff) as i32;
    let ex = ((x.to_bits() >> 52) & 0x7ff) as i32;
    if ey == 0 || ex == 0 || (ey - ex).abs() > 900 {
        return None;
    }
    // Only the ratio matters: scale both so the larger is near 1, keeping
    // the products inside the division far from underflow and overflow.
    let sc = pow2((1023 - ey.max(ex)).clamp(-1022, 1023));
    let q = Dd::div(Dd::from_f64(y.abs() * sc), Dd::from_f64(x.abs() * sc));
    let a = atan_dd(q);
    let r = if x > 0.0 { a } else { Dd::sub(dd(DD_PI), a) };
    Some(with_sign(r.round(INV_TRIG_ERR)?, y < 0.0))
}

/// asin for 2^-30 <= |x| < 1: atan(x / sqrt(1 - x^2)), 1 - x^2 formed
/// exactly enough ((1 - |x|)(1 + |x|) above 1/2).
pub(crate) fn asin(x: f64) -> Option<f64> {
    let ax = x.abs();
    let d = if ax >= 0.5 {
        let (bh, bl) = two_sum(1.0, ax);
        Dd::mul_f(Dd::new(bh, bl), 1.0 - ax)
    } else {
        let (p, e) = two_prod(ax, ax);
        Dd::sub(Dd::from_f64(1.0), Dd::new(p, e))
    };
    let q = Dd::div(Dd::from_f64(ax), Dd::sqrt(d));
    Some(with_sign(atan_dd(q).round(INV_TRIG_ERR)?, x < 0.0))
}

/// acos for |x| < 1.
pub(crate) fn acos(x: f64) -> Option<f64> {
    let r = if x.abs() <= 0.5 {
        let (p, e) = two_prod(x, x);
        let d = Dd::sub(Dd::from_f64(1.0), Dd::new(p, e));
        let a = if x == 0.0 {
            Dd::from_f64(0.0)
        } else {
            atan_dd(Dd::div(Dd::from_f64(x.abs()), Dd::sqrt(d)))
        };
        let asin = if x < 0.0 { a.neg() } else { a };
        Dd::sub(dd(DD_PI_2), asin)
    } else if x > 0.5 {
        let (dh, dl) = two_sum(1.0, x);
        let t = Dd::sqrt(Dd::div(Dd::from_f64(1.0 - x), Dd::new(dh, dl)));
        Dd::mul_f(atan_dd(t), 2.0)
    } else {
        let (dh, dl) = two_sum(1.0, -x);
        let t = Dd::sqrt(Dd::div(Dd::from_f64(1.0 + x), Dd::new(dh, dl)));
        Dd::sub(dd(DD_PI), Dd::mul_f(atan_dd(t), 2.0))
    };
    r.round(INV_TRIG_ERR)
}

// ---------------------------------------------------------------------
// sinh, cosh, tanh
// ---------------------------------------------------------------------

/// sinh x for 0 <= x < 1/8 by its Taylor series to x^19 (truncation
/// 2^-110), the x^11 and later terms in double.
#[inline]
fn sinh_series(x: f64) -> Dd {
    let x2 = Dd::from_f64(x);
    let x2 = Dd::mul(x2, x2);
    let u = x2.hi;
    let d = INV_FACT[11].0
        + u * (INV_FACT[13].0 + u * (INV_FACT[15].0 + u * (INV_FACT[17].0 + u * INV_FACT[19].0)));
    let mut h = Dd::add(dd(INV_FACT[9]), Dd::mul_f(x2, d));
    h = Dd::add(Dd::mul(x2, h), dd(INV_FACT[7]));
    h = Dd::add(Dd::mul(x2, h), dd(INV_FACT[5]));
    h = Dd::add(Dd::mul(x2, h), dd(INV_FACT[3]));
    h = Dd::add_f(Dd::mul(x2, h), 1.0);
    Dd::mul_f(h, x)
}

/// cosh for 2^-30 <= |x| <= 709.
pub(crate) fn cosh(x: f64) -> Option<f64> {
    let ax = x.abs();
    if ax > 709.0 {
        return None;
    }
    let (v, m) = exp_core(ax, 0.0);
    if m > 60 {
        // e^-|x| is below 2^-120 of e^|x|
        return Some(v.round(HYP_ERR)? * pow2(m - 1));
    }
    let inv = Dd::div(Dd::from_f64(1.0), v);
    let c = Dd::add(scale(v, m), scale(inv, -m));
    Dd::new(c.hi * 0.5, c.lo * 0.5).round(HYP_ERR)
}

/// sinh for 2^-30 <= |x| <= 709.
pub(crate) fn sinh(x: f64) -> Option<f64> {
    let ax = x.abs();
    let r = if ax < 0.125 {
        sinh_series(ax)
    } else if ax <= 709.0 {
        let (v, m) = exp_core(ax, 0.0);
        if m > 60 {
            return Some(with_sign(v.round(HYP_ERR)? * pow2(m - 1), x < 0.0));
        }
        let inv = Dd::div(Dd::from_f64(1.0), v);
        let s = Dd::sub(scale(v, m), scale(inv, -m));
        Dd::new(s.hi * 0.5, s.lo * 0.5)
    } else {
        return None;
    };
    Some(with_sign(r.round(HYP_ERR)?, x < 0.0))
}

/// tanh for 2^-30 <= |x| < 22.
pub(crate) fn tanh(x: f64) -> Option<f64> {
    let ax = x.abs();
    let r = if ax < 0.125 {
        let s = sinh_series(ax);
        let c = Dd::sqrt(Dd::add_f(Dd::mul(s, s), 1.0));
        Dd::div(s, c)
    } else {
        let (v, m) = exp_core(-2.0 * ax, 0.0);
        let e = scale(v, m);
        Dd::div(Dd::add_f(e.neg(), 1.0), Dd::add_f(e, 1.0))
    };
    Some(with_sign(r.round(HYP_ERR)?, x < 0.0))
}
