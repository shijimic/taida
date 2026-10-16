//! Double-double arithmetic for the fast path.
//!
//! A value is the unevaluated sum `hi + lo` with `|lo| <= ulp(hi)/2`. The
//! building blocks are the error-free transformations (Knuth's two-sum,
//! Dekker's two-product with Veltkamp splitting, no FMA) and the accurate
//! double-word operations; each result is within a few units of 2^-106 of
//! the exact one, relative. Floating-point contraction must stay off (Rust
//! never contracts; the C mirror is compiled with contraction disabled).

#[derive(Clone, Copy, Debug)]
pub(crate) struct Dd {
    pub(crate) hi: f64,
    pub(crate) lo: f64,
}

/// s + e == a + b exactly.
#[inline]
pub(crate) fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bb = s - a;
    let e = (a - (s - bb)) + (b - bb);
    (s, e)
}

/// s + e == a + b exactly, for |a| >= |b| (or a == 0).
#[inline]
pub(crate) fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let e = b - (s - a);
    (s, e)
}

/// p + e == a * b exactly (no overflow or underflow in the pieces).
#[inline]
pub(crate) fn two_prod(a: f64, b: f64) -> (f64, f64) {
    const SPLIT: f64 = 134217729.0; // 2^27 + 1
    let t = SPLIT * a;
    let ah = t - (t - a);
    let al = a - ah;
    let t = SPLIT * b;
    let bh = t - (t - b);
    let bl = b - bh;
    let p = a * b;
    let e = ((ah * bh - p) + ah * bl + al * bh) + al * bl;
    (p, e)
}

impl Dd {
    #[inline]
    pub(crate) const fn new(hi: f64, lo: f64) -> Dd {
        Dd { hi, lo }
    }

    #[inline]
    pub(crate) fn from_f64(x: f64) -> Dd {
        Dd { hi: x, lo: 0.0 }
    }

    #[inline]
    pub(crate) fn neg(self) -> Dd {
        Dd {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    /// a + b (accurate double-word addition).
    #[inline]
    pub(crate) fn add(a: Dd, b: Dd) -> Dd {
        let (s, e) = two_sum(a.hi, b.hi);
        let (t, f) = two_sum(a.lo, b.lo);
        let e = e + t;
        let (s, e) = fast_two_sum(s, e);
        let e = e + f;
        let (hi, lo) = fast_two_sum(s, e);
        Dd { hi, lo }
    }

    #[inline]
    pub(crate) fn sub(a: Dd, b: Dd) -> Dd {
        Dd::add(a, b.neg())
    }

    /// a + b for a double b.
    #[inline]
    pub(crate) fn add_f(a: Dd, b: f64) -> Dd {
        let (s, e) = two_sum(a.hi, b);
        let e = e + a.lo;
        let (hi, lo) = fast_two_sum(s, e);
        Dd { hi, lo }
    }

    /// a * b.
    #[inline]
    pub(crate) fn mul(a: Dd, b: Dd) -> Dd {
        let (p, e) = two_prod(a.hi, b.hi);
        let e = e + (a.hi * b.lo + a.lo * b.hi);
        let (hi, lo) = fast_two_sum(p, e);
        Dd { hi, lo }
    }

    /// a * b for a double b.
    #[inline]
    pub(crate) fn mul_f(a: Dd, b: f64) -> Dd {
        let (p, e) = two_prod(a.hi, b);
        let e = e + a.lo * b;
        let (hi, lo) = fast_two_sum(p, e);
        Dd { hi, lo }
    }

    /// a / b.
    #[inline]
    pub(crate) fn div(a: Dd, b: Dd) -> Dd {
        let q1 = a.hi / b.hi;
        let r = Dd::sub(a, Dd::mul_f(b, q1));
        let q2 = r.hi / b.hi;
        let r = Dd::sub(r, Dd::mul_f(b, q2));
        let q3 = r.hi / b.hi;
        let (s, e) = fast_two_sum(q1, q2);
        Dd::add_f(Dd { hi: s, lo: e }, q3)
    }

    /// sqrt(a) for a > 0.
    #[inline]
    pub(crate) fn sqrt(a: Dd) -> Dd {
        let s = a.hi.sqrt();
        let (p, e) = two_prod(s, s);
        let r = ((a.hi - p) - e + a.lo) / (2.0 * s);
        let (hi, lo) = fast_two_sum(s, r);
        Dd { hi, lo }
    }

    /// The double nearest to the exact value, when the exact value is
    /// within `rel * |hi|` of `hi + lo` and that is enough to decide the
    /// rounding; `None` otherwise. The bound is doubled before the test so
    /// the two computed endpoints bracket the true interval.
    #[inline]
    pub(crate) fn round(self, rel: f64) -> Option<f64> {
        let err = 2.0 * rel * self.hi.abs();
        let a = self.hi + (self.lo - err);
        let b = self.hi + (self.lo + err);
        if a == b { Some(a) } else { None }
    }
}
