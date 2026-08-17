//! `core/math` — the half of it that has one right answer.
//!
//! # Why this file is nine functions and not twenty-two
//!
//! `core/math` declares twenty-two entries and this implements nine. The split
//! is not effort, it is **whether the answer is specified**:
//!
//! * `sqrt`, `abs`, `floor`, `ceil`, `trunc`, `round`, `isNan`, `isInfinite`
//!   and `isFinite` are exactly determined. IEEE 754 requires `sqrt` to be
//!   correctly rounded, the four rounding functions are integer selections, and
//!   the three predicates are classifications. Every conforming implementation
//!   agrees on every input, so `f64::sqrt` here and `Math.sqrt` there are the
//!   same bits.
//!
//! * `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `ln`,
//!   `log10`, `log2`, `pow` and `cbrt` are **not**. IEEE 754 recommends
//!   correctly-rounded transcendentals and requires nothing, so the answer is
//!   the implementation's. V8 uses its own port of fdlibm
//!   (`src/base/ieee754.cc`); `f64::sin` calls the platform's libm, which is
//!   Apple's on macOS and glibc's on Linux — three implementations that agree
//!   to within an ulp and not to the bit.
//!
//! VALUE-MODEL.md §12 asks for *identical* output, and `show(math.sin(x))`
//! prints seventeen significant digits. So implementing the second group with
//! libm would put a divergence into the toolchain that shows up on one input in
//! a few thousand, which is the hardest possible bug to find and the easiest to
//! ship. They are a named gap instead: both backends report them through
//! `missing_intrinsics`, and closing it means porting fdlibm into this runtime
//! — a real piece of work, and one whose correctness `cli/tests/` can check the
//! same way it checks the float formatter.
//!
//! # `round` is not `f64::round`
//!
//! The one function here whose obvious implementation is wrong. `Math.round`
//! (ECMA-262 §21.3.2.28) rounds a tie **toward positive infinity**:
//! `Math.round(-0.5)` is `-0` and `Math.round(0.5)` is `1`. Rust's
//! `f64::round` rounds a tie *away from zero*, so it answers `-1`. Half the
//! ties disagree, and `-0.5` is not an exotic input.

/// `math.sqrt` — correctly rounded, by IEEE 754 §5.4.1.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// `math.absFloat`. `abs(-0.0)` is `0.0` and `abs(NaN)` is `NaN`, which is what
/// clearing the sign bit does and what `Math.abs` does.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_abs_float(x: f64) -> f64 {
    x.abs()
}

/// `math.floor` — toward negative infinity.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_floor(x: f64) -> f64 {
    x.floor()
}

/// `math.ceil` — toward positive infinity.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_ceil(x: f64) -> f64 {
    x.ceil()
}

/// `math.trunc` — toward zero.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_math_trunc(x: f64) -> f64 {
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
    if x < 0.0 && x >= -0.5 {
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
