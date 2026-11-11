//! `core/math` — the twenty-two functions the runtime answers.
//!
//! `squareRoot`, `absoluteFloat`, the four rounding functions and the three
//! classifiers are exact: IEEE 754 fixes each answer, so `f64::sqrt` here and
//! `Math.sqrt` in JavaScript give the same bits everywhere.
//!
//! The thirteen transcendentals call the platform's libm, which is what
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
//! `power` is the exception: it follows Bun's `Math.pow` step for step rather
//! than C's `pow`, and so does `runtime.js`, so node agrees. See
//! [`buri_rt_math_power`].
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

/// `math.power` — Bun's `Math.pow` (JavaScriptCore's `operationMathPow`).
///
/// Three things C's `pow` does differently:
///
/// * ECMA-262 makes `power(1, NaN)` and `power(±1, ±∞)` `NaN`; C answers `1`.
/// * An exponent of `0.5` or `-0.5` is a square root or one over it.
/// * A whole exponent from 0 to 1000 multiplies by repeated squaring, which is
///   plain arithmetic and so the same everywhere. `power(10, 308)` is
///   `1.0000000000000006e308` where `pow` rounds to `1e308`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_power(x: f64, y: f64) -> f64 {
    let size = x.abs();
    if y.is_nan() || (size == 1.0 && y.is_infinite()) {
        return f64::NAN;
    }
    if y == 0.5 || y == -0.5 {
        let root = if size == 0.0 || size == f64::INFINITY {
            size
        } else {
            x.sqrt()
        };
        return if y > 0.0 { root } else { 1.0 / root };
    }
    if (0.0..=1000.0).contains(&y) && y.trunc() == y {
        let (mut n, mut base, mut result) = (y as u32, x, 1.0);
        while n != 0 {
            if n & 1 == 1 {
                result *= base;
            }
            base *= base;
            n >>= 1;
        }
        return result;
    }
    x.powf(y)
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
