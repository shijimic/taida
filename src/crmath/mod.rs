//! Correctly rounded double-precision math functions.
//!
//! Every function returns the double nearest to the exact value (ties to
//! even). A correctly rounded result is unique, so the interpreter (this
//! module) and the native and WASM runtimes (its C mirror,
//! `src/codegen/runtime_crmath.inc.c`) return the same bits on every CPU.
//! Special values follow C99 Annex F, as Rust's `f64` methods do.
//!
//! Each function tries the fast path (double-double with an error bound,
//! [`fast`]) and falls back to the slow path (352-bit arithmetic,
//! [`slow`]) when the bound does not decide the rounding.

mod dd;
mod exact;
mod fast;
mod mp;
mod slow;
mod tables;

use mp::Mp;
use slow::SLOW_ERR_BITS;

/// The double nearest to the slow path's value.
fn finish(v: &Mp) -> f64 {
    v.round_to_f64(SLOW_ERR_BITS)
        .unwrap_or_else(|| v.round_nearest())
}

fn with_sign(v: f64, neg: bool) -> f64 {
    if neg { -v } else { v }
}

pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.8 {
        return f64::INFINITY;
    }
    if x < -745.2 {
        return 0.0;
    }
    fast::exp(x).unwrap_or_else(|| finish(&slow::exp(&Mp::from_f64(x))))
}

pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return x;
    }
    if x == 1.0 {
        return 0.0;
    }
    fast::ln(x).unwrap_or_else(|| finish(&slow::ln(&Mp::from_f64(x))))
}

pub fn log2(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return x;
    }
    if x == 1.0 {
        return 0.0;
    }
    fast::log2(x)
        .unwrap_or_else(|| finish(&Mp::mul(&slow::ln(&Mp::from_f64(x)), &tables::MP_INV_LN2)))
}

pub fn log10(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return x;
    }
    if x == 1.0 {
        return 0.0;
    }
    fast::log10(x)
        .unwrap_or_else(|| finish(&Mp::mul(&slow::ln(&Mp::from_f64(x)), &tables::MP_INV_LN10)))
}

fn is_integer(y: f64) -> bool {
    y.is_finite() && y.fract() == 0.0
}

fn is_odd_integer(y: f64) -> bool {
    is_integer(y) && y.abs() < 9007199254740992.0 && (y as i64) & 1 == 1
}

pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let y_odd = is_odd_integer(y);
    if x == 0.0 {
        let neg = x.is_sign_negative() && y_odd;
        return with_sign(if y < 0.0 { f64::INFINITY } else { 0.0 }, neg);
    }
    if y.is_infinite() {
        let ax = x.abs();
        if ax == 1.0 {
            return 1.0;
        }
        return if (ax < 1.0) == (y < 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
    }
    if x.is_infinite() {
        let neg = x < 0.0 && y_odd;
        return with_sign(if y < 0.0 { 0.0 } else { f64::INFINITY }, neg);
    }
    if x < 0.0 && !is_integer(y) {
        return f64::NAN;
    }
    let neg = x < 0.0 && y_odd;
    let ax = x.abs();
    if ax == 1.0 {
        return with_sign(1.0, neg);
    }
    // |ln|x|| >= 2^-53 for |x| != 1, so |y| > 2^64 puts |z| above 2048.
    if y.abs() > 18446744073709551616.0 {
        return with_sign(
            if (ax > 1.0) == (y > 0.0) {
                f64::INFINITY
            } else {
                0.0
            },
            neg,
        );
    }
    // z = y ln|x| decides overflow and underflow: e^z overflows above
    // ln(DBL_MAX) = 709.7827 and rounds to zero below -745.1332 (half the
    // smallest subnormal); z is accurate to 2^-80, far inside the margins.
    let z = fast::pow_exponent(ax, y);
    if z > 709.79 {
        return with_sign(f64::INFINITY, neg);
    }
    if z < -745.14 {
        return with_sign(0.0, neg);
    }
    if let Some(r) = fast::pow_abs(ax, y) {
        return with_sign(r, neg);
    }
    let v = slow::pow_abs(ax, y);
    let r = match v.round_to_f64(SLOW_ERR_BITS) {
        Some(r) => r,
        None => exact::pow_exact(ax, y).unwrap_or_else(|| v.round_nearest()),
    };
    with_sign(r, neg)
}

pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        // |x| < 2^-30: sin x = x - x^3/6 rounds to x
        return x;
    }
    fast::sin(x).unwrap_or_else(|| finish(&slow::sin_cos(x).0))
}

pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return 1.0;
    }
    fast::cos(x).unwrap_or_else(|| finish(&slow::sin_cos(x).1))
}

pub fn tan(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return x;
    }
    if let Some(v) = fast::tan(x) {
        return v;
    }
    let (s, c) = slow::sin_cos(x);
    finish(&Mp::div(&s, &c))
}

pub fn asin(x: f64) -> f64 {
    if x.is_nan() || x.abs() > 1.0 {
        return f64::NAN;
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return x;
    }
    if x.abs() == 1.0 {
        return with_sign(tables::DD_PI_2.0, x < 0.0);
    }
    fast::asin(x).unwrap_or_else(|| finish(&slow::asin(&Mp::from_f64(x))))
}

pub fn acos(x: f64) -> f64 {
    if x.is_nan() || x.abs() > 1.0 {
        return f64::NAN;
    }
    if x == 1.0 {
        return 0.0;
    }
    if x == -1.0 {
        return tables::DD_PI.0;
    }
    fast::acos(x).unwrap_or_else(|| finish(&slow::acos(&Mp::from_f64(x))))
}

pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.is_infinite() {
        return with_sign(tables::DD_PI_2.0, x < 0.0);
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return x;
    }
    fast::atan(x).unwrap_or_else(|| finish(&slow::atan(&Mp::from_f64(x))))
}

pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let pi = tables::DD_PI.0;
    let pi_2 = tables::DD_PI_2.0;
    let yneg = y.is_sign_negative();
    if y == 0.0 {
        // atan2(+-0, x): +-0 for x > 0 or +0, +-pi for x < 0 or -0
        return if x > 0.0 || (x == 0.0 && !x.is_sign_negative()) {
            y
        } else {
            with_sign(pi, yneg)
        };
    }
    if x == 0.0 {
        return with_sign(pi_2, yneg);
    }
    if y.is_infinite() {
        let v = if x.is_infinite() {
            if x > 0.0 {
                tables::DD_PI_4.0
            } else {
                // 3pi/4, rounded once
                finish(&Mp::add(&tables::MP_PI_2, &tables::MP_PI_2.ldexp(-1)))
            }
        } else {
            pi_2
        };
        return with_sign(v, yneg);
    }
    if x.is_infinite() {
        return with_sign(if x > 0.0 { 0.0 } else { pi }, yneg);
    }
    fast::atan2(y, x).unwrap_or_else(|| finish(&slow::atan2(&Mp::from_f64(y), &Mp::from_f64(x))))
}

pub fn sinh(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.abs() > 710.5 {
        return with_sign(f64::INFINITY, x < 0.0);
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return x;
    }
    fast::sinh(x).unwrap_or_else(|| finish(&slow::sinh(&Mp::from_f64(x))))
}

pub fn cosh(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.abs() > 710.5 {
        return f64::INFINITY;
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return 1.0;
    }
    fast::cosh(x).unwrap_or_else(|| finish(&slow::cosh(&Mp::from_f64(x))))
}

pub fn tanh(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.abs() >= 22.0 {
        return with_sign(1.0, x < 0.0);
    }
    if x.abs() < f64::from_bits(0x3e10_0000_0000_0000) {
        return x;
    }
    fast::tanh(x).unwrap_or_else(|| finish(&slow::tanh(&Mp::from_f64(x))))
}

#[cfg(test)]
mod tests {
    // Expected values are the correctly rounded results (checked with
    // mpmath at 300 bits); the host libm returns a neighbour for several.
    #[test]
    fn known_values_are_correctly_rounded() {
        let cases: &[(&str, f64, f64)] = &[
            ("exp", 5.66, 287.1486425560543),
            ("exp", 13.08, 479260.70612947544),
            ("log10", 0.17, -0.769551078621726),
            ("sinh", 0.12, 0.12028820743110907),
            ("sinh", 2.0, 3.6268604078470186),
            ("cosh", 10.0, 11013.232920103323),
            ("tanh", 0.02, 0.019997333759930933),
            ("sin", 8.85, 0.5436484436660883),
            ("sin", 1e22, -0.8522008497671888),
            ("atan", 7.58, 1.4396276803928358),
        ];
        for &(f, x, want) in cases {
            let got = match f {
                "exp" => super::exp(x),
                "log10" => super::log10(x),
                "sinh" => super::sinh(x),
                "cosh" => super::cosh(x),
                "tanh" => super::tanh(x),
                "sin" => super::sin(x),
                _ => super::atan(x),
            };
            assert_eq!(got.to_bits(), want.to_bits(), "{f}({x})");
        }
        assert_eq!(super::pow(2.0, 0.5), std::f64::consts::SQRT_2);
        assert_eq!(super::pow(10.0, -3.0), 0.001);
    }

    #[test]
    fn special_values_follow_annex_f() {
        assert!(super::ln(-1.0).is_nan());
        assert_eq!(super::ln(0.0), f64::NEG_INFINITY);
        assert_eq!(super::exp(f64::NEG_INFINITY), 0.0);
        assert_eq!(super::pow(-1.0, f64::INFINITY), 1.0);
        assert_eq!(super::pow(f64::NAN, 0.0), 1.0);
        assert_eq!(super::pow(-0.0, -3.0), f64::NEG_INFINITY);
        assert_eq!(super::pow(-2.0, 1e300), f64::INFINITY);
        assert!(super::pow(-2.0, 0.5).is_nan());
        assert_eq!(
            super::atan2(-0.0, -1.0).to_bits(),
            (-std::f64::consts::PI).to_bits()
        );
        assert_eq!(super::sin(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(super::tanh(f64::INFINITY), 1.0);
        assert!(super::acos(1.5).is_nan());
    }

    /// The C mirror (shared by the native and WASM runtimes) returns the
    /// same bits as this module. Compiles the kernel with the host `cc`
    /// (skipped when there is none).
    #[test]
    fn the_c_mirror_returns_the_same_bits() {
        let dir = std::env::temp_dir().join(format!("taida_crmath_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("crm.c");
        let driver = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static double h(const char *s) { unsigned long long b = strtoull(s, 0, 16); double d; memcpy(&d, &b, 8); return d; }
int main(void) {
  char name[16], a[32], b[32];
  while (scanf("%15s %31s %31s", name, a, b) == 3) {
    double x = h(a), y = h(b), r;
    if (!strcmp(name, "exp")) r = _crm_exp(x); else if (!strcmp(name, "ln")) r = _crm_ln(x);
    else if (!strcmp(name, "log2")) r = _crm_log2(x); else if (!strcmp(name, "log10")) r = _crm_log10(x);
    else if (!strcmp(name, "pow")) r = _crm_pow(x, y); else if (!strcmp(name, "sin")) r = _crm_sin(x);
    else if (!strcmp(name, "cos")) r = _crm_cos(x); else if (!strcmp(name, "tan")) r = _crm_tan(x);
    else if (!strcmp(name, "asin")) r = _crm_asin(x); else if (!strcmp(name, "acos")) r = _crm_acos(x);
    else if (!strcmp(name, "atan")) r = _crm_atan(x); else if (!strcmp(name, "atan2")) r = _crm_atan2(x, y);
    else if (!strcmp(name, "sinh")) r = _crm_sinh(x); else if (!strcmp(name, "cosh")) r = _crm_cosh(x);
    else r = _crm_tanh(x);
    unsigned long long o; memcpy(&o, &r, 8);
    printf("%016llx\n", o);
  }
  return 0;
}
"#;
        let code = format!(
            "#include <stdint.h>\n{}{}{}",
            include_str!("../codegen/runtime_crmath_tables.inc.c"),
            include_str!("../codegen/runtime_crmath.inc.c"),
            driver
        );
        std::fs::write(&src, code).unwrap();
        let bin = dir.join("crm");
        let built = std::process::Command::new("cc")
            .args(["-O2", "-ffp-contract=off", "-o"])
            .arg(&bin)
            .arg(&src)
            .arg("-lm")
            .output();
        let Ok(built) = built else {
            return; // no C compiler
        };
        assert!(
            built.status.success(),
            "cc: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        // xorshift inputs across the interesting ranges of each function
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let names = [
            "exp", "ln", "log2", "log10", "pow", "sin", "cos", "tan", "asin", "acos", "atan",
            "atan2", "sinh", "cosh", "tanh",
        ];
        let mut input = String::new();
        let mut cases = Vec::new();
        for i in 0..30_000u64 {
            let name = names[(i % names.len() as u64) as usize];
            let u = (next() >> 11) as f64 / (1u64 << 53) as f64;
            let v = (next() >> 11) as f64 / (1u64 << 53) as f64;
            let wide = f64::from_bits(next() & 0x7fef_ffff_ffff_ffff);
            let (x, y) = match name {
                "exp" => (u * 1460.0 - 745.0, 0.0),
                "ln" | "log2" | "log10" => (if i % 2 == 0 { wide } else { 0.5 + u }, 0.0),
                "pow" => (0.1 + u * 10.0, v * 200.0 - 100.0),
                "asin" | "acos" => (u * 2.0 - 1.0, 0.0),
                "atan2" => (u * 20.0 - 10.0, v * 20.0 - 10.0),
                "sinh" | "cosh" | "tanh" => (u * 60.0 - 30.0, 0.0),
                _ => (if i % 3 == 0 { wide } else { u * 40.0 - 20.0 }, 0.0),
            };
            input.push_str(&format!(
                "{name} {:016x} {:016x}\n",
                x.to_bits(),
                y.to_bits()
            ));
            cases.push((name, x, y));
        }
        let input_path = dir.join("input.txt");
        std::fs::write(&input_path, input).unwrap();
        let out = std::process::Command::new(&bin)
            .stdin(std::fs::File::open(&input_path).unwrap())
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        let mut bad = Vec::new();
        for ((name, x, y), line) in cases.iter().zip(text.lines()) {
            let c = u64::from_str_radix(line, 16).unwrap();
            let r = match *name {
                "exp" => super::exp(*x),
                "ln" => super::ln(*x),
                "log2" => super::log2(*x),
                "log10" => super::log10(*x),
                "pow" => super::pow(*x, *y),
                "sin" => super::sin(*x),
                "cos" => super::cos(*x),
                "tan" => super::tan(*x),
                "asin" => super::asin(*x),
                "acos" => super::acos(*x),
                "atan" => super::atan(*x),
                "atan2" => super::atan2(*x, *y),
                "sinh" => super::sinh(*x),
                "cosh" => super::cosh(*x),
                _ => super::tanh(*x),
            };
            if r.to_bits() != c && !(r.is_nan() && f64::from_bits(c).is_nan()) {
                bad.push(format!(
                    "{name}({x:e}, {y:e}): rust {r:e} c {:e}",
                    f64::from_bits(c)
                ));
            }
        }
        assert_eq!(text.lines().count(), cases.len());
        assert!(bad.is_empty(), "{}", bad[..bad.len().min(10)].join("\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
