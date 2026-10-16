//! Exact evaluation of `pow` for the arguments whose value can be a double
//! or a midpoint between two doubles (the multiprecision value cannot tell
//! which side of such a point the rounding goes).

/// A little-endian big unsigned integer.
#[derive(Clone)]
struct Big(Vec<u32>);

impl Big {
    fn from_u64(v: u64) -> Big {
        let mut b = Big(vec![v as u32, (v >> 32) as u32]);
        b.trim();
        b
    }

    fn trim(&mut self) {
        while self.0.len() > 1 && *self.0.last().unwrap() == 0 {
            self.0.pop();
        }
    }

    fn mul(&self, o: &Big) -> Big {
        let mut w = vec![0u32; self.0.len() + o.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.0.iter().enumerate() {
                let t = a as u64 * b as u64 + w[i + j] as u64 + carry;
                w[i + j] = t as u32;
                carry = t >> 32;
            }
            w[i + o.0.len()] = carry as u32;
        }
        let mut r = Big(w);
        r.trim();
        r
    }

    fn bit_len(&self) -> u32 {
        let top = *self.0.last().unwrap();
        if top == 0 {
            0
        } else {
            (self.0.len() as u32 - 1) * 32 + (32 - top.leading_zeros())
        }
    }

    fn bit(&self, i: u32) -> bool {
        let w = (i / 32) as usize;
        w < self.0.len() && (self.0[w] >> (i % 32)) & 1 == 1
    }

    fn any_below(&self, i: u32) -> bool {
        (0..i).any(|b| self.bit(b))
    }
}

/// The double nearest to `n * 2^e` (ties to even), `n > 0`.
fn round_big(n: &Big, e: i64) -> f64 {
    let len = n.bit_len() as i64;
    // value in [2^top, 2^(top+1))
    let top = len - 1 + e;
    if top >= 1024 {
        return f64::INFINITY;
    }
    if top < -1076 {
        return 0.0;
    }
    // value ~= k * 2^q with q the exponent of the last kept bit
    let normal = top >= -1022;
    let mut q = if normal { top - 52 } else { -1074 };
    let shift = q - e; // bits of n rounded off (negative: none, shift left)
    let mut k: u64 = if shift <= 0 {
        // n has fewer bits than kept: exact
        let mut v: u64 = 0;
        for b in (0..len).rev() {
            v = (v << 1) | n.bit(b as u32) as u64;
        }
        v << (-shift)
    } else {
        let mut v: u64 = 0;
        for b in (shift..len).rev() {
            v = (v << 1) | n.bit(b as u32) as u64;
        }
        let half = n.bit(shift as u32 - 1);
        let sticky = n.any_below(shift as u32 - 1);
        if half && (sticky || v & 1 == 1) {
            v += 1;
        }
        v
    };
    if normal && k == 1u64 << 53 {
        k >>= 1;
        q += 1;
    }
    if k == 0 {
        return 0.0;
    }
    if !normal || k < 1u64 << 52 {
        // subnormal (k * 2^-1074; k == 2^52 is the smallest normal)
        return f64::from_bits(k);
    }
    let biased = q + 52 + 1023;
    if biased >= 2047 {
        return f64::INFINITY;
    }
    f64::from_bits(((biased as u64) << 52) | (k - (1u64 << 52)))
}

/// (m, e) with x = m * 2^e and m odd (x > 0 finite).
fn decompose(x: f64) -> (u64, i64) {
    let bits = x.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i64;
    let frac = bits & ((1u64 << 52) - 1);
    let (mut m, mut e) = if biased == 0 {
        (frac, -1074)
    } else {
        (frac | (1u64 << 52), biased - 1075)
    };
    let tz = m.trailing_zeros();
    m >>= tz;
    e += tz as i64;
    (m, e)
}

fn isqrt(n: u64) -> u64 {
    let mut r = (n as f64).sqrt() as u64;
    while r * r > n {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= n {
        r += 1;
    }
    r
}

/// |x|^y exactly rounded when it is a dyadic rational (it could then be a
/// double or a midpoint), `None` otherwise. x > 0 finite, y finite.
pub(crate) fn pow_exact(x: f64, y: f64) -> Option<f64> {
    let (mut m, mut e) = decompose(x);
    // y = n / 2^k with n an odd integer (or y an integer).
    let mut k = 0u32;
    let mut yy = y;
    while yy.fract() != 0.0 {
        yy *= 2.0;
        k += 1;
        if k > 60 {
            return None;
        }
    }
    // take 2^k-th roots of m * 2^e
    for _ in 0..k {
        if e % 2 != 0 {
            return None;
        }
        let r = isqrt(m);
        if r * r != m {
            return None;
        }
        m = r;
        e /= 2;
    }
    let n = yy;
    if n.abs() > 1100.0 {
        return None;
    }
    let ni = n as i64;
    if ni < 0 {
        // m^-|n|: dyadic only for m == 1
        if m != 1 {
            return None;
        }
        return Some(round_big(&Big::from_u64(1), e * ni));
    }
    // m^n with m odd: more than 54 bits means it is no tie candidate, but
    // the exact rounding is still right.
    let mut acc = Big::from_u64(1);
    let mut base = Big::from_u64(m);
    let mut p = ni as u64;
    if m > 1 && ni > 64 {
        return None;
    }
    while p > 0 {
        if p & 1 == 1 {
            acc = acc.mul(&base);
        }
        base = base.mul(&base);
        p >>= 1;
    }
    Some(round_big(&acc, e * ni))
}
