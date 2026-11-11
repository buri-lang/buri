//! `core/math` — the twenty-two functions the runtime answers.
//!
//! `squareRoot`, `absoluteFloat`, the four rounding functions and the three
//! classifiers are exact: IEEE 754 fixes each answer, so `f64::sqrt` here and
//! `Math.sqrt` in JavaScript give the same bits everywhere.
//!
//! Twelve of the thirteen transcendentals call the platform's libm, which is what
//! JavaScript does too: Bun's `Math.sin` is the same `sin`. IEEE 754 doesn't fix
//! their last bit, so libms differ in it. Measured on 500,000 inputs:
//!
//! * macOS: every one of these matches Bun bit for bit.
//! * Linux: a native binary links musl and Bun links glibc, so up to 11% of
//!   `atan2`'s answers, and fewer of the others', differ by one or two units in
//!   the last place.
//! * Apple's libm disagrees with both Linux libms at common points:
//!   `tan(1.0)`, `asin(0.5)`, `atan(2.0)`.
//!
//! `power` is the exception: it is fdlibm's `pow` written out, in
//! [`buri_rt_math_power`] and again in `runtime.js`, so it answers the same
//! bits on every platform and every JavaScript engine.
//!
//! # `round` is not `f64::round`
//!
//! `Math.round` (ECMA-262 §21.3.2.28) rounds a tie **toward positive
//! infinity**: `Math.round(-0.5)` is `-0` and `Math.round(0.5)` is `1`. Rust's
//! `f64::round` rounds a tie *away from zero*, so it answers `-1`.

/// `math.squareRoot` — correctly rounded, by IEEE 754 §5.4.1.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_square_root(x: f64) -> f64 {
    x.sqrt()
}

/// `math.absoluteFloat`. `abs(-0.0)` is `0.0` and `abs(NaN)` is `NaN`, which is what
/// clearing the sign bit does and what `Math.abs` does.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_absolute_float(x: f64) -> f64 {
    x.abs()
}

/// `math.floor` — toward negative infinity.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_floor(x: f64) -> f64 {
    x.floor()
}

/// `math.ceiling` — toward positive infinity.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_ceiling(x: f64) -> f64 {
    x.ceil()
}

/// `math.truncate` — toward zero.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_truncate(x: f64) -> f64 {
    x.trunc()
}

/// `math.round` — ECMA-262 §21.3.2.28, tie toward **positive infinity**.
///
/// The specification's four early returns and then `floor(x + 0.5)`. The early
/// returns are not an optimization: without the third, `-0.4` would answer
/// `0` where JavaScript answers `-0`, and the two print differently
/// (`cli/runtime/fmt.rs` puts the sign back on `-0.0`).
///
/// `floor(x + 0.5)` is exact above `2^52`, where every double is already an
/// integer and adding a half rounds back to the value itself.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_round(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 || x.is_infinite() {
        return x;
    }
    if x > 0.0 && x < 0.5 {
        return 0.0;
    }
    if (-0.5..0.0).contains(&x) {
        return -0.0;
    }
    (x + 0.5).floor()
}

/// `math.isNan` — `Number.isNaN`, which is the value test and not the coercing
/// global `isNaN`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_is_nan(x: f64) -> u8 {
    u8::from(x.is_nan())
}

/// `math.isInfinite` — `x === Infinity || x === -Infinity`
/// (`runtime.js:910-912`), which is false for `NaN`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_is_infinite(x: f64) -> u8 {
    u8::from(x.is_infinite())
}

/// `math.isFinite` — `Number.isFinite`, false for both infinities and for
/// `NaN`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_is_finite(x: f64) -> u8 {
    u8::from(x.is_finite())
}

/// `math.cubeRoot` — libm's `cbrt`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_cube_root(x: f64) -> f64 {
    x.cbrt()
}

/// `math.exp` — libm's `exp`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_exp(x: f64) -> f64 {
    x.exp()
}

/// `math.ln` — libm's `log`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_ln(x: f64) -> f64 {
    x.ln()
}

/// `math.log10` — libm's `log10`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_log10(x: f64) -> f64 {
    x.log10()
}

/// `math.log2` — libm's `log2`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_log2(x: f64) -> f64 {
    x.log2()
}

/// `math.sin` — libm's `sin`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_sin(x: f64) -> f64 {
    x.sin()
}

/// `math.cos` — libm's `cos`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_cos(x: f64) -> f64 {
    x.cos()
}

/// `math.tan` — libm's `tan`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_tan(x: f64) -> f64 {
    x.tan()
}

/// `math.asin` — libm's `asin`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_asin(x: f64) -> f64 {
    x.asin()
}

/// `math.acos` — libm's `acos`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_acos(x: f64) -> f64 {
    x.acos()
}

/// `math.atan` — libm's `atan`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_atan(x: f64) -> f64 {
    x.atan()
}

/// `math.atan2` — libm's `atan2`, `y` first.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

/// A float's high word, signed, as fdlibm reads it.
fn high(x: f64) -> i32 {
    (x.to_bits() >> 32) as i32
}

fn low(x: f64) -> u32 {
    x.to_bits() as u32
}

fn words(high: i32, low: u32) -> f64 {
    f64::from_bits((u64::from(high as u32) << 32) | u64::from(low))
}

fn drop_low(x: f64) -> f64 {
    words(high(x), 0)
}

/// `x * 2^n`, as musl's `scalbn`.
fn scale(mut x: f64, mut n: i32) -> f64 {
    if n > 1023 {
        x *= 8.98846567431158e307;
        n -= 1023;
        if n > 1023 {
            x *= 8.98846567431158e307;
            n = (n - 1023).min(1023);
        }
    } else if n < -1022 {
        x *= 2.004168360008973e-292;
        n += 969;
        if n < -1022 {
            x *= 2.004168360008973e-292;
            n = (n + 969).max(-1022);
        }
    }
    x * words((0x3ff + n) << 20, 0)
}

/// `math.power` — fdlibm's `e_pow.c`, the same steps as `runtime.js`'s
/// `$math_power`.
///
/// Not libm's `pow`, nor `Math.pow`: their last bit differs between macOS,
/// musl, glibc, Bun and each node version. This uses IEEE arithmetic and bit
/// operations only, so every platform and every JavaScript engine gets the same
/// bits. The first line is ECMA-262's `power(1, NaN)` and `power(±1, ±∞)`,
/// which are `NaN` where fdlibm answers `1`.
#[unsafe(no_mangle)]
#[allow(clippy::excessive_precision, clippy::many_single_char_names)]
pub extern "C" fn buri_rt_math_power(x: f64, y: f64) -> f64 {
    if y.is_nan() || (x.abs() == 1.0 && y.is_infinite()) {
        return f64::NAN;
    }
    let (hx, lx, hy, ly) = (high(x), low(x), high(y), low(y));
    let mut ix = hx & 0x7fffffff;
    let iy = hy & 0x7fffffff;
    if iy as u32 | ly == 0 {
        return 1.0;
    }
    if hx == 0x3ff00000 && lx == 0 {
        return 1.0;
    }
    if ix > 0x7ff00000 || (ix == 0x7ff00000 && lx != 0) {
        return f64::NAN;
    }
    // Whether y is an odd whole number (1), an even one (2) or neither (0).
    let mut yisint = 0;
    if hx < 0 {
        if iy >= 0x43400000 {
            yisint = 2;
        } else if iy >= 0x3ff00000 {
            let k = (iy >> 20) - 0x3ff;
            if k > 20 {
                let j = ly >> (52 - k);
                if j << (52 - k) == ly {
                    yisint = 2 - (j & 1) as i32;
                }
            } else if ly == 0 {
                let j = iy >> (20 - k);
                if j << (20 - k) == iy {
                    yisint = 2 - (j & 1);
                }
            }
        }
    }
    if ly == 0 {
        if iy == 0x7ff00000 {
            if ix >= 0x3ff00000 {
                return if hy >= 0 { y } else { 0.0 };
            }
            return if hy < 0 { -y } else { 0.0 };
        }
        if iy == 0x3ff00000 {
            return if hy < 0 { 1.0 / x } else { x };
        }
        if hy == 0x40000000 {
            return x * x;
        }
        if hy == 0x3fe00000 && hx >= 0 {
            return x.sqrt();
        }
    }
    let mut ax = x.abs();
    if lx == 0 && (ix == 0x7ff00000 || ix == 0 || ix == 0x3ff00000) {
        let mut z = if hy < 0 { 1.0 / ax } else { ax };
        if hx < 0 {
            if (ix - 0x3ff00000) | yisint == 0 {
                z = f64::NAN;
            } else if yisint == 1 {
                z = -z;
            }
        }
        return z;
    }
    let mut n = (hx as u32 >> 31) as i32 - 1;
    if n | yisint == 0 {
        return f64::NAN;
    }
    let s = if n | (yisint - 1) == 0 { -1.0 } else { 1.0 };
    // log2(x) as t1 + t2, t1 with its low word cleared.
    let (t1, t2);
    if iy > 0x41e00000 {
        if iy > 0x43f00000 {
            if ix <= 0x3fefffff {
                return if hy < 0 { f64::INFINITY } else { 0.0 };
            }
            return if hy > 0 { f64::INFINITY } else { 0.0 };
        }
        if ix < 0x3fefffff {
            return s * if hy < 0 { f64::INFINITY } else { 0.0 };
        }
        if ix > 0x3ff00000 {
            return s * if hy > 0 { f64::INFINITY } else { 0.0 };
        }
        let t = ax - 1.0;
        let w = t * t * (0.5 - t * (3.3333333333333331e-1 - t * 0.25));
        let u = 1.44269502162933349609 * t;
        let v = t * 1.92596299112661746887e-8 - w * 1.442695040888963387;
        t1 = drop_low(u + v);
        t2 = v - (t1 - u);
    } else {
        n = 0;
        if ix < 0x00100000 {
            ax *= 9007199254740992.0;
            n -= 53;
            ix = high(ax);
        }
        n += (ix >> 20) - 0x3ff;
        let j = ix & 0x000fffff;
        ix = j | 0x3ff00000;
        let mut k = 0;
        if j > 0x3988e && j < 0xbb67a {
            k = 1;
        } else if j >= 0xbb67a {
            n += 1;
            ix -= 0x00100000;
        }
        ax = words(ix, low(ax));
        let (bp, dp_h, dp_l) = if k == 0 {
            (1.0, 0.0, 0.0)
        } else {
            (1.5, 5.84962487220764160156e-1, 1.35003920212974897128e-8)
        };
        let mut u = ax - bp;
        let mut v = 1.0 / (ax + bp);
        let ss = u * v;
        let s_h = drop_low(ss);
        let mut t_h = words(((ix >> 1) | 0x20000000) + 0x00080000 + (k << 18), 0);
        let mut t_l = ax - (t_h - bp);
        let s_l = v * (u - s_h * t_h - s_h * t_l);
        let mut s2 = ss * ss;
        let mut r = s2
            * s2
            * (5.99999999999994648725e-1
                + s2 * (4.28571428578550184252e-1
                    + s2 * (3.33333329818377432918e-1
                        + s2 * (2.72728123808534006489e-1
                            + s2 * (2.30660745775561754067e-1 + s2 * 2.06975017800338417784e-1)))));
        r += s_l * (s_h + ss);
        s2 = s_h * s_h;
        t_h = drop_low(3.0 + s2 + r);
        t_l = r - (t_h - 3.0 - s2);
        u = s_h * t_h;
        v = s_l * t_h + t_l * ss;
        let p_h = drop_low(u + v);
        let p_l = v - (p_h - u);
        let z_h = 9.61796700954437255859e-1 * p_h;
        let z_l = -7.02846165095275826516e-9 * p_h + p_l * 9.61796693925975554329e-1 + dp_l;
        let t = f64::from(n);
        t1 = drop_low(z_h + z_l + dp_h + t);
        t2 = z_l - (t1 - t - dp_h - z_h);
    }
    // y * log2(x) as p_h + p_l, then 2 to that.
    let y1 = drop_low(y);
    let p_l = (y - y1) * t1 + y * t2;
    let mut p_h = y1 * t1;
    let mut z = p_l + p_h;
    let j = high(z);
    let i = low(z);
    if j >= 0x40900000 {
        if (j - 0x40900000) as u32 | i != 0 || p_l + 8.0085662595372944372e-17 > z - p_h {
            return s * f64::INFINITY;
        }
    } else if j & 0x7fffffff >= 0x4090cc00
        && ((j as u32).wrapping_sub(0xc090cc00) | i != 0 || p_l <= z - p_h)
    {
        return s * 0.0;
    }
    let i = j & 0x7fffffff;
    let mut k = (i >> 20) - 0x3ff;
    n = 0;
    if i > 0x3fe00000 {
        n = j + (0x00100000 >> (k + 1));
        k = ((n & 0x7fffffff) >> 20) - 0x3ff;
        let t = words(n & !(0x000fffff >> k), 0);
        n = ((n & 0x000fffff) | 0x00100000) >> (20 - k);
        if j < 0 {
            n = -n;
        }
        p_h -= t;
    }
    let t = drop_low(p_l + p_h);
    let u = t * 6.93147182464599609375e-1;
    let v = (p_l - (t - p_h)) * 6.93147180559945286227e-1 + t * -1.90465429995776804525e-9;
    z = u + v;
    let w = v - (z - u);
    let t = z * z;
    let t1 = z - t
        * (1.66666666666666019037e-1
            + t * (-2.77777777770155933842e-3
                + t * (6.61375632143793436117e-5
                    + t * (-1.65339022054652515390e-6 + t * 4.13813679705723846039e-8))));
    let r = (z * t1) / (t1 - 2.0) - (w + z * w);
    z = 1.0 - (r - z);
    let j = high(z).wrapping_add(n << 20);
    z = if j >> 20 <= 0 {
        scale(z, n)
    } else {
        words(j, low(z))
    };
    s * z
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ties, at both signs — the whole reason `round` is written out rather
    /// than forwarded to `f64::round`.
    #[test]
    fn round_breaks_a_tie_toward_positive_infinity() {
        assert_eq!(buri_rt_math_round(0.5), 1.0);
        assert_eq!(buri_rt_math_round(1.5), 2.0);
        assert_eq!(buri_rt_math_round(-1.5), -1.0);
        assert_eq!(buri_rt_math_round(2.5), 3.0);
        // Rust's own `round` answers `-1.0` and `-2.0` for these two.
        assert_ne!(buri_rt_math_round(-0.5), (-0.5f64).round());
        assert_ne!(buri_rt_math_round(-1.5), (-1.5f64).round());
    }

    /// `-0` is not `0`: they print differently, so the sign has to survive.
    #[test]
    fn round_keeps_a_negative_zero() {
        assert!(buri_rt_math_round(-0.5).is_sign_negative());
        assert!(buri_rt_math_round(-0.4).is_sign_negative());
        assert!(buri_rt_math_round(-0.0).is_sign_negative());
        assert!(buri_rt_math_round(0.4).is_sign_positive());
    }

    #[test]
    fn the_non_finite_cases_pass_through() {
        assert!(buri_rt_math_round(f64::NAN).is_nan());
        assert_eq!(buri_rt_math_round(f64::INFINITY), f64::INFINITY);
        assert_eq!(buri_rt_math_is_nan(f64::NAN), 1);
        assert_eq!(buri_rt_math_is_infinite(f64::NAN), 0);
        assert_eq!(buri_rt_math_is_infinite(f64::NEG_INFINITY), 1);
        assert_eq!(buri_rt_math_is_finite(f64::INFINITY), 0);
        assert_eq!(buri_rt_math_is_finite(0.0), 1);
    }
}
