//! Rendering: floats, 128-bit integers, characters, and quoted strings.
//!
//! VALUE-MODEL.md §12 row 9 asks that `show` produce the same bytes on both
//! backends. On JavaScript `show` of a `Float` is `$f64` (`runtime.js:84-90`),
//! which is `String(n)` with a `.0` stuck on the integral cases — and
//! `String(n)` is `Number::toString(n, 10)`, ECMA-262 §6.1.6.1.20. So the
//! headline obligation of this file is that obligation: **a native `show(0.1)`
//! and a JavaScript one are byte-identical**, and so are `show(1e21)`,
//! `show(5e-324)`, `show(f64::MAX)` and every value in between.
//!
//! # How the shortest representation is found
//!
//! ECMA-262 §6.1.6.1.20 does not name an algorithm; it states a property. Let
//! `n`, `k` and `s` be integers with `k >= 1`, `10^(k-1) <= s < 10^k` and
//! `s * 10^(n-k) == x`, with **`k` as small as possible**; among the `s` that
//! achieve that `k`, the one whose value is **closest to `x`**, and on a tie the
//! **even** one. Then a fixed presentation rule turns `(n, k, s)` into digits.
//!
//! Two halves, and they are found separately here:
//!
//! 1. **`k`, the shortest length**, comes from Rust's own shortest formatter —
//!    `format!("{:e}", x)`, which is Grisu3 with an exact Dragon4 fallback and
//!    is documented to produce the shortest digit string that round-trips.
//! 2. **`s`, the digits**, comes from `format!("{:.*e}", k - 1, x)`, Rust's
//!    *exact* formatter at `k` significant digits, which is correctly rounded to
//!    nearest with ties to even — which is precisely the second and third
//!    clauses of the property above.
//!
//! Step 2 is not redundant, and finding that out is what this file is for. The
//! shortest formatter promises only that its digits round-trip, not that they
//! are the *closest* `k`-digit decimal: over a corpus of this size it disagrees
//! with V8 on the last digit about once in twenty-five thousand
//! (`2181495296738027.3` where JavaScript says `2181495296738027.2`; both read
//! back as the same double, and only one of them is closer). Re-rounding at the
//! length the first step found fixes every one of those, and cannot lengthen the
//! answer: if some `k`-digit decimal is within half an ulp of `x`, the *closest*
//! `k`-digit decimal is too.
//!
//! The evidence is `cli/tests/native/float_parity.rs`, which renders a corpus of
//! **3,807,072** doubles — every corner case named in this file, a strided sweep
//! of the entire `f32` domain widened to `f64`, two million xorshift bit
//! patterns, every power of ten from `1e-320` to `1e308`, and the subnormals at
//! both ends — and compares each against `String(v)` under the JavaScript
//! engine the toolchain's own tests run. Zero disagreements.
//!
//! # Why this rather than a hand-rolled Ryū
//!
//! (Adams' paper is vendored at `reference/ryu-float-to-string.pdf`.)
//!
//! The dependency bar (workspace `Cargo.toml`) is about *crates*, and `std` is
//! not one: this runtime already uses `String::from_utf8_lossy`, `std::alloc`
//! and `std::io`. A hand-written Ryū would be four hundred lines of table-driven
//! integer arithmetic reproducing something `core::fmt` already does correctly,
//! and its bugs would be exactly the ones that are hardest to find — a wrong
//! digit on one input in a million. What is genuinely *not* in `std` is the
//! ECMA-262 presentation rule, and that is what is written out below.
//!
//! The cost is two formatting calls per rendered float. That is the right trade
//! for a v1 whose correctness is checked against another implementation and
//! whose allocator is still one `malloc` per value (`lib.rs` §5); a Ryū fast
//! path can be dropped in under this same test corpus when a profile asks for
//! one.

use crate::value::{str_of, BuriStr, BURI_RT_STR_LEN_MASK};

// ---------------------------------------------------------------------------
// ECMA-262 §6.1.6.1.20 — Number::toString(x, 10)
// ---------------------------------------------------------------------------

/// The shortest digit string of `x` and its decimal exponent, as ECMA-262's
/// `(s, n)`: `x == s * 10^(n - k)`, where `k` is `s.len()`.
///
/// `x` must be finite, non-zero and positive — the three cases the caller has
/// already peeled off.
fn shortest_digits(x: f64) -> (String, i32) {
    // Step 1: the *length*. Rust's shortest formatter answers `d.ddde<exp>`,
    // and the count is of the **mantissa's** digits: the exponent has digits
    // too, and counting those made `1.0 / 3.0` seventeen significant figures
    // instead of sixteen — which is a wrong answer, not a rounding one.
    let short = format!("{x:e}");
    let mantissa = short.split('e').next().unwrap_or(&short);
    let k = mantissa.bytes().filter(u8::is_ascii_digit).count();
    // Step 2: the *digits*, correctly rounded at that length. `k >= 1` always,
    // because a finite non-zero float has at least one significant digit.
    let exact = format!("{:.*e}", k.saturating_sub(1), x);
    let Some((mantissa, exponent)) = exact.split_once('e') else {
        // `{:e}` always emits an `e`. Answering `0` here rather than reaching
        // for a panic keeps the promise that no input panics the runtime.
        return (String::from("0"), 0);
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // Re-rounding can turn `9.99` into `10.0`; the trailing zero is not a
    // significant digit and `k` shrinks by one, which the presentation rule
    // below reads off `digits.len()` rather than from step 1.
    let trimmed = digits.trim_end_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    // `{:e}` writes `d.ddd`, so the value is `mantissa * 10^exponent` and
    // ECMA's `n` — the position of the decimal point — is one further right.
    (String::from(trimmed), exponent.saturating_add(1))
}

/// `Number::toString(x, 10)` — what JavaScript's `String(x)` produces.
///
/// `NaN`, `Infinity` and `-Infinity` come out with those spellings, which is
/// what ECMA-262 says; `$f64` never asks for them, because it renders the three
/// non-finite cases itself and this runtime follows it (see [`show_f64`]).
pub fn ecma_number(x: f64) -> String {
    if x.is_nan() {
        return String::from("NaN");
    }
    // `-0` prints as `0`: ECMA-262 step 2 tests `x is either +0 or -0`.
    if x == 0.0 {
        return String::from("0");
    }
    if x < 0.0 {
        return format!("-{}", ecma_number(-x));
    }
    if x.is_infinite() {
        return String::from("Infinity");
    }
    let (digits, n) = shortest_digits(x);
    let k = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let mut out = String::with_capacity(digits.len().saturating_add(8));
    if k <= n && n <= 21 {
        // `123` with `n == 5` is `12300`: the digits, then `n - k` zeros.
        out.push_str(&digits);
        for _ in 0..n.saturating_sub(k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        // A point inside the digits: `1.5`.
        let at = n.clamp(0, k) as usize;
        let (head, tail) = digits.split_at(at.min(digits.len()));
        out.push_str(head);
        out.push('.');
        out.push_str(tail);
    } else if -6 < n && n <= 0 {
        // `0.` then `-n` zeros then the digits: `0.001`. The cut at `-6` is
        // ECMA's, and it is why `1e-7` is exponential while `1e-6` is not.
        out.push_str("0.");
        for _ in 0..-n {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        // Exponential. The exponent written is `n - 1`, and it always carries a
        // sign — `1e+21`, `1e-7`.
        let e = n.saturating_sub(1);
        let (head, tail) = digits.split_at(1.min(digits.len()));
        out.push_str(head);
        if !tail.is_empty() {
            out.push('.');
            out.push_str(tail);
        }
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.unsigned_abs().to_string());
    }
    out
}

/// `$f64` — how a `Float` renders in a template hole and in a derived `Show`.
///
/// `runtime.js:84-90`, clause for clause:
///
/// | Input | Output | Why |
/// |---|---|---|
/// | `NaN` | `NaN` | |
/// | `+inf`, `-inf` | `inf`, `-inf` | Buri's spelling, not JavaScript's `Infinity` |
/// | integral, `abs < 1e21` | `42.0`, `-0.0` | a float always shows a point, so `1.0` does not read as an integer |
/// | anything else | `Number::toString` | `0.1`, `1e+21`, `5e-324` |
///
/// The `1e21` cut is not arbitrary: it is where [`ecma_number`] itself switches
/// to exponential notation, so above it a `.0` would be appended to something
/// that already has an `e` in it.
pub fn show_f64(x: f64) -> String {
    if x.is_nan() {
        return String::from("NaN");
    }
    if x.is_infinite() {
        return String::from(if x > 0.0 { "inf" } else { "-inf" });
    }
    if x.fract() == 0.0 && x.abs() < 1e21 {
        // `-0.0` is integral and `ecma_number` renders it `0`, so the sign is
        // put back by hand — `Object.is(n, -0)` on the JavaScript side.
        let sign = if x == 0.0 && x.is_sign_negative() { "-" } else { "" };
        return format!("{sign}{}.0", ecma_number(x));
    }
    ecma_number(x)
}

// ---------------------------------------------------------------------------
// The exported renderings
// ---------------------------------------------------------------------------

/// `show` of an `F64`, and `str.fromFloat`.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_f64(x: f64, out: *mut BuriStr) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&show_f64(x))) }
}

/// `show` of an `F32`.
///
/// Widened to `f64` first, and rendered as that double: an `F32` is stored as a
/// double on JavaScript (`$convF32` rounds through `Math.fround`), so
/// `show(0.1f32)` is `0.10000000149011612` there. Rendering the shortest `f32`
/// digits instead would print `0.1`, which is a *different string on the two
/// backends* — and this file exists so that cannot happen.
///
/// # Safety
/// As [`buri_rt_show_f64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_f32(x: f32, out: *mut BuriStr) {
    // SAFETY: forwarded.
    unsafe { buri_rt_show_f64(f64::from(x), out) }
}

/// `show` of a signed 128-bit integer — decimal, with a leading `-`.
///
/// Deliberately not the 64-bit renderer applied to the low half: a `show` that
/// silently truncated would be a wrong answer where an unimplemented one used to
/// be a diagnostic. The operand is a **pair of `u64`s, low half first**, for the
/// reason `buri_rt_i128_divmod` states: `lib.rs` §2's first rule says a
/// parameter is a scalar leaf, and a 128-bit value is not one.
///
/// # Safety
/// As [`buri_rt_show_f64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_i128(lo: u64, hi: u64, out: *mut BuriStr) {
    let v = (u128::from(lo) | (u128::from(hi) << 64)) as i128;
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&v.to_string())) }
}

/// `show` of an unsigned 128-bit integer.
///
/// # Safety
/// As [`buri_rt_show_f64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_u128(lo: u64, hi: u64, out: *mut BuriStr) {
    let v = u128::from(lo) | (u128::from(hi) << 64);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&v.to_string())) }
}

/// A `Char` as a one-scalar `Str` — a template hole at `Char`, and
/// `num.<T>.toChar` reaching a rendering.
///
/// `$str` of a `Char` on JavaScript is the character itself, because a `Char`
/// *is* a one-character string there. An unpaired or out-of-range scalar
/// renders as U+FFFD, matching `String::from_utf8_lossy`'s treatment of every
/// other malformed input this runtime sees.
///
/// # Safety
/// As [`buri_rt_show_f64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_char_to_str(c: u32, out: *mut BuriStr) {
    let mut buf = [0u8; 4];
    let text = char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut buf);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(text)) }
}

/// `derive Show` of a `Char`: the character in single quotes.
///
/// `$show`'s `"c"` arm (`runtime.js:206`) is `"'" + v + "'"` — no escaping at
/// all, including for `'` itself, which is the JavaScript backend's behaviour
/// and therefore this one's.
///
/// # Safety
/// As [`buri_rt_show_f64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_char(c: u32, out: *mut BuriStr) {
    let ch = char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&format!("'{ch}'"))) }
}

/// `derive Show` of a `Str`: `JSON.stringify`, which is quoting and escaping.
///
/// `$show`'s `"s"` arm (`runtime.js:205`). ECMA-262 `QuoteJSONString`: the two
/// structural characters, the five short escapes, and `\u00XX` for everything
/// else below U+0020. Nothing above U+007F is escaped — the output is UTF-8 and
/// `JSON.stringify` emits those characters literally too.
///
/// # Safety
/// `ptr` must point at `len` readable bytes, or be null with `len == 0`. `out`
/// must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_str(ptr: *const u8, len: u64, out: *mut BuriStr) {
    // The **stored** length, flag and all: bit 63 of a `Str`'s length is the
    // ASCII flag (VALUE-MODEL.md §3.1), and every entry in `text.rs` masks it
    // off before using the value as a byte count. This one did not, and a
    // backend that passed the word unmasked asked for a slice of `2^63 + n`
    // bytes. Masking here rather than at two call sites is what makes the rule
    // "a runtime entry taking a `Str` length takes the stored word", which is
    // the rule the rest of the runtime already follows.
    let n = (len & BURI_RT_STR_LEN_MASK) as usize;
    let src = if ptr.is_null() || n == 0 {
        &[][..]
    } else {
        // SAFETY: the caller promises `n` readable bytes.
        unsafe { std::slice::from_raw_parts(ptr, n) }
    };
    let text = String::from_utf8_lossy(src);
    let mut quoted = String::with_capacity(text.len().saturating_add(2));
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\u{8}' => quoted.push_str("\\b"),
            '\u{c}' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if (c as u32) < 0x20 => quoted.push_str(&format!("\\u{:04x}", c as u32)),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&quoted)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows of `$f64`'s table, and the boundaries between the four
    /// presentation cases of ECMA-262 §6.1.6.1.20.
    #[test]
    fn the_named_corners_render_as_javascript_does() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (0.1, "0.1"),
            (0.2, "0.2"),
            (1.0 / 3.0, "0.3333333333333333"),
            // The `n <= 21` boundary: the last integral value written out in
            // full, and the first written in exponential form.
            (1e20, "100000000000000000000.0"),
            (1e21, "1e+21"),
            (1e-6, "0.000001"),
            // The `-6 < n` boundary.
            (1e-7, "1e-7"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::MIN, "-1.7976931348623157e+308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            // The smallest subnormal, at both signs.
            (5e-324, "5e-324"),
            (-5e-324, "-5e-324"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            // The one where a shortest formatter that does not re-round
            // disagrees with V8.
            (f64::from_bits(0x431f_003b_d0f7_0bad), "2181495296738027.2"),
        ];
        for (input, want) in cases {
            assert_eq!(&show_f64(*input), want, "show({input:?})");
        }
    }

    /// `ecma_number` is `String(x)`, which differs from `show` in exactly two
    /// places: no `.0` on an integer, and the JavaScript spellings of the
    /// non-finite values.
    #[test]
    fn ecma_number_is_the_javascript_spelling() {
        assert_eq!(ecma_number(1.0), "1");
        assert_eq!(ecma_number(-0.0), "0");
        assert_eq!(ecma_number(f64::INFINITY), "Infinity");
        assert_eq!(ecma_number(f64::NAN), "NaN");
        assert_eq!(ecma_number(1e21), "1e+21");
    }

    /// Every finite double must round-trip through its own rendering: that is
    /// the property the shortest representation is *for*, and it is the one a
    /// re-rounding step could break.
    #[test]
    fn every_rendering_reads_back_as_itself() {
        let mut s: u64 = 0x243F_6A88_85A3_08D3;
        for _ in 0..100_000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let x = f64::from_bits(s);
            if !x.is_finite() {
                continue;
            }
            let text = ecma_number(x);
            let back: f64 = text.parse().unwrap_or(f64::NAN);
            assert_eq!(back.to_bits(), x.to_bits(), "{text} did not read back as {x:?}");
        }
    }

    /// An `F32` renders as the double it widens to, not as the shortest `f32`.
    #[test]
    fn an_f32_renders_through_its_double() {
        assert_eq!(show_f64(f64::from(0.1f32)), "0.10000000149011612");
        assert_eq!(show_f64(f64::from(1.0f32)), "1.0");
    }
}
