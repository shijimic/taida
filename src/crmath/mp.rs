//! Fixed-precision binary floating point for the slow path of the
//! correctly rounded math kernel.
//!
//! A value is `(-1)^neg * M * 2^exp` with a 352-bit integer mantissa `M`
//! held in 11 little-endian u32 limbs, normalized so that
//! `2^351 <= M < 2^352` (zero has every limb 0). Every operation computes
//! the exact result and truncates it to 352 bits, so each one is accurate
//! to better than 2^-350 relative; additions and subtractions are exact
//! before the truncation, so cancellation never loses more than the
//! cancelled bits. The kernel only needs `u32 * u32 -> u64` products, so
//! the C runtime can mirror it on wasm32.

pub(crate) const LIMBS: usize = 11;
const BITS: i32 = 32 * LIMBS as i32;
const WIDE: usize = 2 * LIMBS + 1;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Mp {
    pub(crate) neg: bool,
    pub(crate) exp: i32,
    pub(crate) m: [u32; LIMBS],
}

/// Shift the little-endian limb array `w` left by `n` bits (in place).
fn shl_wide(w: &mut [u32], n: u32) {
    let len = w.len();
    let limbs = (n / 32) as usize;
    let bits = n % 32;
    if limbs >= len {
        w.iter_mut().for_each(|x| *x = 0);
        return;
    }
    for i in (0..len).rev() {
        let src = i as isize - limbs as isize;
        let hi = if src >= 0 { w[src as usize] } else { 0 };
        let lo = if src >= 1 { w[src as usize - 1] } else { 0 };
        w[i] = if bits == 0 {
            hi
        } else {
            (hi << bits) | (lo >> (32 - bits))
        };
    }
}

/// Shift the little-endian limb array `w` right by `n` bits (in place,
/// truncating).
fn shr_wide(w: &mut [u32], n: u32) {
    let len = w.len();
    let limbs = (n / 32) as usize;
    let bits = n % 32;
    if limbs >= len {
        w.iter_mut().for_each(|x| *x = 0);
        return;
    }
    for i in 0..len {
        let src = i + limbs;
        let lo = if src < len { w[src] } else { 0 };
        let hi = if src + 1 < len { w[src + 1] } else { 0 };
        w[i] = if bits == 0 {
            lo
        } else {
            (lo >> bits) | (hi << (32 - bits))
        };
    }
}

/// The top `LIMBS` limbs of the nonzero wide integer `w` after shifting
/// its highest set bit to bit 351, and the exponent adjustment: the value
/// `w * 2^e` equals `M * 2^(e + adj)`.
fn normalize_wide(w: &mut [u32]) -> Option<([u32; LIMBS], i32)> {
    let len = w.len();
    let mut top = len;
    while top > 0 && w[top - 1] == 0 {
        top -= 1;
    }
    if top == 0 {
        return None;
    }
    // Bit index of the highest set bit.
    let high = (top as i32 - 1) * 32 + (31 - w[top - 1].leading_zeros() as i32);
    let target = BITS - 1; // bit 351 of the top LIMBS limbs
    // The result's low limb sits at limb index (len - LIMBS) once the high
    // bit is moved to bit (len*32 - 1).
    let lead = (len as i32) * 32 - 1 - high;
    shl_wide(w, lead as u32);
    let mut m = [0u32; LIMBS];
    m.copy_from_slice(&w[len - LIMBS..]);
    // w * 2^e = (w << lead) * 2^(e - lead); the top LIMBS limbs are
    // (w << lead) >> (32 * (len - LIMBS)).
    let adj = 32 * (len as i32 - LIMBS as i32) - lead;
    let _ = target;
    Some((m, adj))
}

impl Mp {
    pub(crate) const ZERO: Mp = Mp {
        neg: false,
        exp: 0,
        m: [0; LIMBS],
    };

    pub(crate) fn is_zero(&self) -> bool {
        self.m[LIMBS - 1] == 0
    }

    pub(crate) fn from_wide(neg: bool, w: &mut [u32], e: i32) -> Mp {
        match normalize_wide(w) {
            Some((m, adj)) => Mp {
                neg,
                exp: e + adj,
                m,
            },
            None => Mp::ZERO,
        }
    }

    pub(crate) fn from_u64(v: u64) -> Mp {
        let mut w = [0u32; LIMBS];
        w[0] = v as u32;
        w[1] = (v >> 32) as u32;
        Mp::from_wide(false, &mut w, 0)
    }

    pub(crate) fn from_i64(v: i64) -> Mp {
        let mut r = Mp::from_u64(v.unsigned_abs());
        r.neg = v < 0 && !r.is_zero();
        r
    }

    /// The exact value of the finite double `x` (a zero loses its sign).
    pub(crate) fn from_f64(x: f64) -> Mp {
        let bits = x.to_bits();
        let neg = bits >> 63 != 0;
        let biased = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & ((1u64 << 52) - 1);
        let (mant, e) = if biased == 0 {
            (frac, -1074)
        } else {
            (frac | (1u64 << 52), biased - 1075)
        };
        let mut w = [0u32; LIMBS];
        w[0] = mant as u32;
        w[1] = (mant >> 32) as u32;
        let mut r = Mp::from_wide(neg, &mut w, e);
        if r.is_zero() {
            r.neg = false;
        }
        r
    }

    pub(crate) fn neg(&self) -> Mp {
        let mut r = *self;
        if !r.is_zero() {
            r.neg = !r.neg;
        }
        r
    }

    pub(crate) fn abs(&self) -> Mp {
        let mut r = *self;
        r.neg = false;
        r
    }

    /// `self * 2^n`.
    pub(crate) fn ldexp(&self, n: i32) -> Mp {
        let mut r = *self;
        if !r.is_zero() {
            r.exp += n;
        }
        r
    }

    /// The base-2 exponent of the leading bit: `2^e <= |self| < 2^(e+1)`.
    pub(crate) fn top_exp(&self) -> i32 {
        self.exp + BITS - 1
    }

    /// Compare magnitudes.
    pub(crate) fn cmp_abs(a: &Mp, b: &Mp) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (a.is_zero(), b.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        if a.exp != b.exp {
            return a.exp.cmp(&b.exp);
        }
        for i in (0..LIMBS).rev() {
            if a.m[i] != b.m[i] {
                return a.m[i].cmp(&b.m[i]);
            }
        }
        Ordering::Equal
    }

    pub(crate) fn add(a: &Mp, b: &Mp) -> Mp {
        if a.is_zero() {
            return *b;
        }
        if b.is_zero() {
            return *a;
        }
        let (x, y) = if a.exp >= b.exp { (a, b) } else { (b, a) };
        let d = (x.exp as i64 - y.exp as i64) as u64;
        // y is below 2^-382 of x: it cannot change the truncated sum except
        // by borrowing one unit, which the 2^-350 error bound covers.
        if d > 32 * (WIDE - LIMBS) as u64 - 2 {
            return *x;
        }
        let mut wx = [0u32; WIDE];
        let mut wy = [0u32; WIDE];
        wx[WIDE - LIMBS..].copy_from_slice(&x.m);
        wy[WIDE - LIMBS..].copy_from_slice(&y.m);
        shr_wide(&mut wy, d as u32);
        let base = x.exp - 32 * (WIDE - LIMBS) as i32;
        if x.neg == y.neg {
            let mut carry = 0u64;
            let mut w = [0u32; WIDE + 1];
            for i in 0..WIDE {
                let s = wx[i] as u64 + wy[i] as u64 + carry;
                w[i] = s as u32;
                carry = s >> 32;
            }
            w[WIDE] = carry as u32;
            Mp::from_wide(x.neg, &mut w, base)
        } else {
            let mut gt = std::cmp::Ordering::Equal;
            for i in (0..WIDE).rev() {
                if wx[i] != wy[i] {
                    gt = wx[i].cmp(&wy[i]);
                    break;
                }
            }
            let (big, small, neg) = match gt {
                std::cmp::Ordering::Equal => return Mp::ZERO,
                std::cmp::Ordering::Greater => (&wx, &wy, x.neg),
                std::cmp::Ordering::Less => (&wy, &wx, y.neg),
            };
            let mut w = [0u32; WIDE];
            let mut borrow = 0i64;
            for i in 0..WIDE {
                let s = big[i] as i64 - small[i] as i64 - borrow;
                if s < 0 {
                    w[i] = (s + (1i64 << 32)) as u32;
                    borrow = 1;
                } else {
                    w[i] = s as u32;
                    borrow = 0;
                }
            }
            Mp::from_wide(neg, &mut w, base)
        }
    }

    pub(crate) fn sub(a: &Mp, b: &Mp) -> Mp {
        Mp::add(a, &b.neg())
    }

    pub(crate) fn mul(a: &Mp, b: &Mp) -> Mp {
        if a.is_zero() || b.is_zero() {
            return Mp::ZERO;
        }
        let mut w = [0u32; 2 * LIMBS];
        for i in 0..LIMBS {
            let mut carry = 0u64;
            let ai = a.m[i] as u64;
            for j in 0..LIMBS {
                let t = ai * b.m[j] as u64 + w[i + j] as u64 + carry;
                w[i + j] = t as u32;
                carry = t >> 32;
            }
            w[i + LIMBS] = carry as u32;
        }
        Mp::from_wide(a.neg != b.neg, &mut w, a.exp + b.exp)
    }

    pub(crate) fn div_u32(a: &Mp, k: u32) -> Mp {
        if a.is_zero() {
            return Mp::ZERO;
        }
        // (M * 2^64) / k keeps at least 352 + 32 significant bits.
        let mut num = [0u32; LIMBS + 2];
        num[2..].copy_from_slice(&a.m);
        let mut q = [0u32; LIMBS + 2];
        let mut rem = 0u64;
        for i in (0..LIMBS + 2).rev() {
            let cur = (rem << 32) | num[i] as u64;
            q[i] = (cur / k as u64) as u32;
            rem = cur % k as u64;
        }
        Mp::from_wide(a.neg, &mut q, a.exp - 64)
    }

    /// The leading bits of the value as a double (truncated), for initial
    /// guesses: `(d, e)` with `|self| ~= d * 2^e`, `0.5 <= d < 1`.
    fn approx_frac(&self) -> (f64, i32) {
        let hi = ((self.m[LIMBS - 1] as u64) << 32) | self.m[LIMBS - 2] as u64;
        // hi / 2^64 in [0.5, 1)
        let d = (hi >> 11) as f64 / (1u64 << 53) as f64;
        (d, self.exp + BITS)
    }

    /// The value as a double, truncated (for initial guesses and range
    /// checks; out of the double range it saturates to 0 or infinity).
    pub(crate) fn approx_f64(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let (d, e) = self.approx_frac();
        let v = d * 2f64.powi(e.clamp(-1100, 1100));
        if self.neg { -v } else { v }
    }

    /// `1 / self` (self nonzero).
    pub(crate) fn recip(&self) -> Mp {
        let (d, e) = self.approx_frac();
        // m = |self| / 2^e in [0.5, 1); y ~ 1/m.
        let m = Mp {
            neg: false,
            exp: self.exp - e,
            m: self.m,
        };
        let one = Mp::from_u64(1);
        let mut y = Mp::from_f64(1.0 / d);
        for _ in 0..3 {
            let t = Mp::sub(&one, &Mp::mul(&m, &y));
            y = Mp::add(&y, &Mp::mul(&y, &t));
        }
        let mut r = y.ldexp(-e);
        r.neg = self.neg;
        r
    }

    pub(crate) fn div(a: &Mp, b: &Mp) -> Mp {
        Mp::mul(a, &b.recip())
    }

    /// `sqrt(self)` (self >= 0).
    pub(crate) fn sqrt(&self) -> Mp {
        if self.is_zero() {
            return Mp::ZERO;
        }
        let (_, e) = self.approx_frac();
        // v = self / 2^(2k) in [0.25, 1)
        let k = if e % 2 == 0 { e / 2 } else { (e + 1) / 2 };
        let v = self.ldexp(-2 * k);
        let (vd, ve) = v.approx_frac();
        let vf = vd * (2.0f64).powi(ve);
        let mut y = Mp::from_f64(1.0 / vf.sqrt());
        let one = Mp::from_u64(1);
        for _ in 0..3 {
            // y += y * (1 - v*y^2) / 2
            let t = Mp::sub(&one, &Mp::mul(&v, &Mp::mul(&y, &y)));
            y = Mp::add(&y, &Mp::mul(&y, &t).ldexp(-1));
        }
        Mp::mul(&v, &y).ldexp(k)
    }

    /// The double nearest to the value taken as exact (ties to even).
    pub(crate) fn round_nearest(&self) -> f64 {
        self.round_impl(None)
            .expect("exact rounding always decides")
    }

    /// The double nearest to the value, when the value is known to within
    /// `2^-err_bits` of itself (relative); `None` when that is not enough
    /// to decide the rounding (the value is that close to a midpoint
    /// between two doubles).
    pub(crate) fn round_to_f64(&self, err_bits: u32) -> Option<f64> {
        self.round_impl(Some(err_bits))
    }

    fn round_impl(&self, err_bits: Option<u32>) -> Option<f64> {
        let sign = if self.neg { 1u64 << 63 } else { 0 };
        if self.is_zero() {
            return Some(f64::from_bits(sign));
        }
        let e_top = self.top_exp();
        if e_top >= 1024 {
            return Some(f64::from_bits(sign | 0x7ff0_0000_0000_0000));
        }
        // Below 2^-1076 the value is under half the smallest subnormal by
        // a wide margin: it rounds to zero.
        if e_top < -1076 {
            return Some(f64::from_bits(sign));
        }
        // Precision: 53 bits for normal results, fewer for subnormal ones.
        let p = if e_top >= -1022 { 53 } else { e_top + 1075 };
        // r low bits of M are rounded off (r may exceed 352).
        let r = BITS - p;
        // rem = M mod 2^r, half = 2^(r-1), compared in 13 limbs.
        let mut rem = [0u32; LIMBS + 2];
        rem[..LIMBS].copy_from_slice(&self.m);
        let kept: u64 = if r >= BITS {
            0
        } else {
            let mut k = [0u32; LIMBS];
            k.copy_from_slice(&self.m);
            shr_wide(&mut k, r as u32);
            // Clear the kept bits from rem.
            if r < BITS {
                let full = (r / 32) as usize;
                let part = r % 32;
                for (i, x) in rem.iter_mut().enumerate() {
                    if i > full || (i == full && part == 0) {
                        *x = 0;
                    } else if i == full {
                        *x &= (1u32 << part) - 1;
                    }
                }
            }
            (k[0] as u64) | ((k[1] as u64) << 32)
        };
        let mut half = [0u32; LIMBS + 2];
        let hb = (r - 1).max(0) as usize;
        if r >= 1 {
            half[hb / 32] |= 1u32 << (hb % 32);
        }
        // diff = |rem - half|
        let mut ge = true;
        for i in (0..LIMBS + 2).rev() {
            if rem[i] != half[i] {
                ge = rem[i] > half[i];
                break;
            }
        }
        let (big, small) = if ge { (&rem, &half) } else { (&half, &rem) };
        let mut diff = [0u32; LIMBS + 2];
        let mut borrow = 0i64;
        for i in 0..LIMBS + 2 {
            let s = big[i] as i64 - small[i] as i64 - borrow;
            if s < 0 {
                diff[i] = (s + (1i64 << 32)) as u32;
                borrow = 1;
            } else {
                diff[i] = s as u32;
                borrow = 0;
            }
        }
        let diff_zero = diff.iter().all(|&w| w == 0);
        let round_up = match err_bits {
            Some(err_bits) => {
                // err = 2^(352 - err_bits) units of M
                let eb = (BITS - err_bits as i32).max(0) as usize;
                let mut err = [0u32; LIMBS + 2];
                err[eb / 32] |= 1u32 << (eb % 32);
                let mut diff_le_err = true;
                for i in (0..LIMBS + 2).rev() {
                    if diff[i] != err[i] {
                        diff_le_err = diff[i] < err[i];
                        break;
                    }
                }
                if diff_le_err {
                    return None;
                }
                ge // rem > half (strictly, since diff > err >= 1)
            }
            // Exact: a tie goes to the even neighbour.
            None if diff_zero => kept & 1 == 1,
            None => ge,
        };
        let mut kept = kept + round_up as u64;
        // value = kept * 2^(exp + r)
        let mut q = self.exp + r;
        if p >= 1 && kept == 1u64 << p && e_top >= -1022 {
            kept >>= 1;
            q += 1;
        }
        if kept == 0 {
            return Some(f64::from_bits(sign));
        }
        if e_top < -1022 || (kept < 1u64 << 52) {
            // Subnormal (or rounded up into the smallest normal): the
            // representation is kept * 2^-1074 directly.
            debug_assert_eq!(q, -1074);
            return Some(f64::from_bits(sign | kept));
        }
        let biased = q + 52 + 1023;
        if biased >= 2047 {
            return Some(f64::from_bits(sign | 0x7ff0_0000_0000_0000));
        }
        Some(f64::from_bits(
            sign | ((biased as u64) << 52) | (kept - (1u64 << 52)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(x: f64) {
        let m = Mp::from_f64(x);
        assert_eq!(
            m.round_to_f64(300).map(f64::to_bits),
            Some(x.to_bits()),
            "{x:e}"
        );
    }

    #[test]
    fn doubles_round_trip() {
        for x in [
            1.0,
            -1.0,
            0.5,
            3.0,
            1e300,
            -1e-300,
            5e-324,
            2.2250738585072014e-308,
            1.7976931348623157e308,
            0.1,
            123.456,
        ] {
            rt(x);
        }
    }

    #[test]
    fn arithmetic_matches_doubles_when_exact() {
        let a = Mp::from_f64(1.5);
        let b = Mp::from_f64(2.25);
        assert_eq!(Mp::add(&a, &b).round_to_f64(300), Some(3.75));
        assert_eq!(Mp::sub(&a, &b).round_to_f64(300), Some(-0.75));
        assert_eq!(Mp::mul(&a, &b).round_to_f64(300), Some(3.375));
        assert_eq!(Mp::div(&b, &a).round_to_f64(300), Some(1.5));
        assert_eq!(Mp::div_u32(&b, 3).round_to_f64(300), Some(0.75));
        assert_eq!(
            Mp::from_f64(2.0).sqrt().round_to_f64(300),
            Some(std::f64::consts::SQRT_2)
        );
        assert_eq!(
            Mp::div(&Mp::from_u64(1), &Mp::from_u64(3)).round_to_f64(300),
            Some(1.0 / 3.0)
        );
    }
}
