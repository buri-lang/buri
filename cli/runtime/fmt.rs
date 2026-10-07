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
//! [`crate::ryu`] answers `(s, n)`: Ryū (Ulf Adams, PLDI 2018) finds the
//! shortest `s` and rounds it to nearest, ties to even, which is the property
//! word for word. This file writes the presentation rule around it.
//!
//! It used to take two `core::fmt` passes: the shortest formatter for `k`, then
//! the exact one at `k` digits for `s`. The second pass was needed because the
//! shortest formatter promises only digits that round-trip, not the *closest*
//! ones: it disagreed with V8 about once in twenty-five thousand
//! (`2181495296738027.3` where JavaScript says `2181495296738027.2`). Those two
//! passes and their buffers were most of the cost of a rendered float, and they
//! stay in this file's tests as the reference Ryū must match.
//!
//! The evidence is `cli/tests/native/float_parity.rs`, which renders a corpus of
//! **3,807,072** doubles — every corner case named in this file, a strided sweep
//! of the entire `f32` domain widened to `f64`, two million xorshift bit
//! patterns, every power of ten from `1e-320` to `1e308`, and the subnormals at
//! both ends — and compares each against `String(v)` under the JavaScript
//! engine the toolchain's own tests run. Zero disagreements.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here counts the decimal digits and exponent of one `u64` or `f64`, \
              and offsets into buffers sized to hold them"
)]

use crate::value::{str_of, BuriStr, BURI_RT_STR_LEN_MASK};

// ---------------------------------------------------------------------------
// ECMA-262 §6.1.6.1.20 — Number::toString(x, 10)
// ---------------------------------------------------------------------------

/// A rendering on the stack. Every float, integer and character this file
/// renders fits in [`Buf::CAP`] bytes, and building one here rather than in a
/// `String` saves a `malloc` and a `free` per value, which the platform
/// allocator zeroes on the way out.
pub(crate) struct Buf {
    bytes: [u8; Buf::CAP],
    len: usize,
}

impl Buf {
    const CAP: usize = 64;

    pub(crate) const fn new() -> Buf {
        Buf { bytes: [0; Buf::CAP], len: 0 }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    pub(crate) fn push_bytes(&mut self, b: &[u8]) {
        let end = self.len.saturating_add(b.len()).min(Buf::CAP);
        let n = end - self.len;
        if let (Some(dst), Some(src)) = (self.bytes.get_mut(self.len..end), b.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.len = end;
    }

    fn push(&mut self, b: u8) {
        self.push_bytes(&[b]);
    }

    fn push_zeros(&mut self, n: usize) {
        for _ in 0..n {
            self.push(b'0');
        }
    }

    /// `v` in decimal.
    pub(crate) fn push_u64(&mut self, v: u64) {
        let end = self.len + decimal_len(v);
        if let Some(dst) = self.bytes.get_mut(self.len..end) {
            write_decimal(v, dst);
            self.len = end;
        }
    }

    /// `v` in decimal, with a `-` if it is negative.
    #[cfg(test)]
    pub(crate) fn push_i64(&mut self, v: i64) {
        if v < 0 {
            self.push(b'-');
        }
        self.push_u64(v.unsigned_abs());
    }
}

impl std::fmt::Write for Buf {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if self.len.saturating_add(s.len()) > Buf::CAP {
            return Err(std::fmt::Error);
        }
        self.push_bytes(s.as_bytes());
        Ok(())
    }
}

/// Two digits at a time, `"00"` through `"99"`.
const PAIRS: &[u8; 200] = b"\
0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";

/// How many decimal digits `v` has.
pub(crate) fn decimal_len(v: u64) -> usize {
    #[expect(clippy::indexing_slicing, reason = "`i` counts from 1 to 19, inside the table")]
    const POW10: [u64; 20] = {
        let mut p = [1u64; 20];
        let mut i = 1;
        while i < 20 {
            p[i] = p[i - 1] * 10;
            i += 1;
        }
        p
    };
    // `bits * 1233 >> 12` is `floor(bits * log10(2))`, which is the digit
    // count less one or exactly it; one compare settles which.
    let bits = 64 - (v | 1).leading_zeros() as usize;
    let guess = (bits * 1233) >> 12;
    guess + usize::from(POW10.get(guess).is_some_and(|p| (v | 1) >= *p))
}

/// Write `v`'s decimal digits into `out`, which is [`decimal_len`]`(v)` long.
#[expect(
    clippy::indexing_slicing,
    reason = "`at` is checked against 2 or 1 before each write below it, and `pair` is at most 198"
)]
pub(crate) fn write_decimal(mut v: u64, out: &mut [u8]) {
    let mut at = out.len();
    while v >= 100 && at >= 2 {
        let pair = (v % 100) as usize * 2;
        v /= 100;
        at -= 2;
        out[at] = PAIRS[pair];
        out[at + 1] = PAIRS[pair + 1];
    }
    if v >= 10 && at >= 2 {
        let pair = v as usize * 2;
        out[at - 2] = PAIRS[pair];
        out[at - 1] = PAIRS[pair + 1];
    } else if at >= 1 {
        out[at - 1] = b'0' + v as u8;
    }
}

/// The shortest digit string of `x` and its decimal exponent, as ECMA-262's
/// `(s, n)`: `x == s * 10^(n - k)`, where `k` is `s.len()`.
///
/// `x` must be finite, non-zero and positive — the three cases the caller has
/// already peeled off.
fn shortest_digits(x: f64) -> (Buf, i32) {
    let (mut s, mut e) = crate::ryu::shortest(x);
    // A shortest answer has no trailing zero, so this never runs; it keeps
    // `k` honest if it ever did.
    while s >= 10 && s % 10 == 0 {
        s /= 10;
        e += 1;
    }
    let mut digits = Buf::new();
    digits.push_u64(s);
    // `x == s * 10^e`, and ECMA's `n` is where the point goes: `k` digits on.
    let k = digits.len as i32;
    (digits, e.saturating_add(k))
}

/// `Number::toString(x, 10)` — what JavaScript's `String(x)` produces.
///
/// `NaN`, `Infinity` and `-Infinity` come out with those spellings, which is
/// what ECMA-262 says; `$f64` never asks for them, because it renders the three
/// non-finite cases itself and this runtime follows it (see [`show_f64`]).
pub fn ecma_number(x: f64) -> String {
    let mut out = Buf::new();
    ecma_number_into(x, &mut out);
    String::from(out.as_str())
}

/// [`ecma_number`], onto the end of `out`.
fn ecma_number_into(x: f64, out: &mut Buf) {
    if x.is_nan() {
        return out.push_bytes(b"NaN");
    }
    // `-0` prints as `0`: ECMA-262 step 2 tests `x is either +0 or -0`.
    if x == 0.0 {
        return out.push(b'0');
    }
    if x < 0.0 {
        out.push(b'-');
        return ecma_number_into(-x, out);
    }
    if x.is_infinite() {
        return out.push_bytes(b"Infinity");
    }
    // An integer below 2^53 is its own shortest rendering: doubles there are
    // at most 1 apart, so no decimal with fewer significant digits reads back
    // as it, and `n <= 16` puts it in the first arm below.
    if x < EXACT_INTEGERS && x.fract() == 0.0 {
        return out.push_u64(x as u64);
    }
    let (digits, n) = shortest_digits(x);
    let digits = digits.as_bytes();
    let k = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if k <= n && n <= 21 {
        // `123` with `n == 5` is `12300`: the digits, then `n - k` zeros.
        out.push_bytes(digits);
        out.push_zeros(n.saturating_sub(k) as usize);
    } else if 0 < n && n <= 21 {
        // A point inside the digits: `1.5`.
        let at = n.clamp(0, k) as usize;
        let (head, tail) = digits.split_at(at.min(digits.len()));
        out.push_bytes(head);
        out.push(b'.');
        out.push_bytes(tail);
    } else if -6 < n && n <= 0 {
        // `0.` then `-n` zeros then the digits: `0.001`. The cut at `-6` is
        // ECMA's, and it is why `1e-7` is exponential while `1e-6` is not.
        out.push_bytes(b"0.");
        out.push_zeros(n.unsigned_abs() as usize);
        out.push_bytes(digits);
    } else {
        // Exponential. The exponent written is `n - 1`, and it always carries a
        // sign — `1e+21`, `1e-7`.
        let e = n.saturating_sub(1);
        let (head, tail) = digits.split_at(1.min(digits.len()));
        out.push_bytes(head);
        if !tail.is_empty() {
            out.push(b'.');
            out.push_bytes(tail);
        }
        out.push(b'e');
        out.push(if e < 0 { b'-' } else { b'+' });
        out.push_u64(u64::from(e.unsigned_abs()));
    }
}

/// 2^53: every integer below it is a double, and the doubles there are
/// integers at most 1 apart.
const EXACT_INTEGERS: f64 = 9_007_199_254_740_992.0;

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
    let mut out = Buf::new();
    show_f64_into(x, &mut out);
    String::from(out.as_str())
}

/// [`show_f64`], into `out`.
pub(crate) fn show_f64_into(x: f64, out: &mut Buf) {
    if x.is_nan() {
        return out.push_bytes(b"NaN");
    }
    if x.is_infinite() {
        return out.push_bytes(if x > 0.0 { b"inf" } else { b"-inf" });
    }
    if x.fract() == 0.0 && x.abs() < 1e21 {
        // `-0.0` is integral and `ecma_number` renders it `0`, so the sign is
        // put back by hand — `Object.is(n, -0)` on the JavaScript side.
        if x == 0.0 && x.is_sign_negative() {
            out.push(b'-');
        }
        ecma_number_into(x, out);
        return out.push_bytes(b".0");
    }
    ecma_number_into(x, out);
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
    let mut text = Buf::new();
    show_f64_into(x, &mut text);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(text.as_bytes())) }
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
    let mut text = Buf::new();
    let _ = std::fmt::Write::write_fmt(&mut text, format_args!("{v}"));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(text.as_bytes())) }
}

/// `show` of an unsigned 128-bit integer.
///
/// # Safety
/// As [`buri_rt_show_f64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_u128(lo: u64, hi: u64, out: *mut BuriStr) {
    let v = u128::from(lo) | (u128::from(hi) << 64);
    let mut text = Buf::new();
    let _ = std::fmt::Write::write_fmt(&mut text, format_args!("{v}"));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(text.as_bytes())) }
}

/// A `Char` as a one-scalar `Str` — a template hole at `Char`, and
/// `number.<T>.toChar` reaching a rendering.
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
    let mut quoted = [b'\''; 6];
    let n = ch.encode_utf8(&mut quoted[1..5]).len();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(quoted.get(..n + 2).unwrap_or(&[]))) }
}

/// `derive Show` of a `[T]`: `[` + the elements, already rendered, joined by
/// `, ` + `]`.
///
/// The one rendering entry whose argument is a list, and here for the reason
/// [`buri_rt_show_str`] is here: `$show`'s array arm is
/// `"[" + $showEach(v, d[1]) + "]"` (`runtime.js:273`) with `$showEach` joining
/// by `", "`, and a `show` that is not byte-identical across backends is a
/// failure report that depends on which backend built it (VALUE-MODEL.md §12
/// row 9). Two code generators spelling the brackets and the separator for
/// themselves would be two places for that to drift.
///
/// The elements arrive **already rendered** — `middle/derives.rs`'s
/// `deriveArrayShow` is `([T], fn(T) -> Str) -> Str`, and a backend calls that
/// function once per element into a scratch block of [`BuriStr`]s before this
/// runs. So this entry needs no element descriptor, which is what separates it
/// from the `list.*` entries `cli/runtime/list.rs`'s header holds back.
///
/// # Safety
/// `xs` points at `count` [`BuriStr`]s, each a live view, or is null with
/// `count == 0`. `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_show_list(xs: *const u8, count: u64, out: *mut BuriStr) {
    let stride = size_of::<BuriStr>();
    let mut built: Vec<u8> = Vec::new();
    built.push(b'[');
    for i in 0..count {
        if xs.is_null() {
            break;
        }
        if i > 0 {
            built.extend_from_slice(b", ");
        }
        // SAFETY: the caller promises `count` elements at `xs`.
        let element = unsafe { &*xs.add((i as usize).saturating_mul(stride)).cast::<BuriStr>() };
        // SAFETY: an element of a live `[Str]` is a live view.
        built.extend_from_slice(unsafe { element.bytes() });
    }
    built.push(b']');
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(&built)) }
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

    /// The rendering this file had before it moved onto the stack and took the
    /// integer shortcut, kept as the reference the new one must match.
    fn show_f64_by_strings(x: f64) -> String {
        fn digits(x: f64) -> (String, i32) {
            let short = format!("{x:e}");
            let mantissa = short.split('e').next().unwrap_or(&short);
            let k = mantissa.bytes().filter(u8::is_ascii_digit).count();
            let exact = format!("{:.*e}", k.saturating_sub(1), x);
            let (mantissa, exponent) = exact.split_once('e').unwrap();
            let exponent: i32 = exponent.parse().unwrap();
            let all: String = mantissa.chars().filter(char::is_ascii_digit).collect();
            let trimmed = all.trim_end_matches('0');
            let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
            (String::from(trimmed), exponent + 1)
        }
        fn ecma(x: f64) -> String {
            if x.is_nan() {
                return String::from("NaN");
            }
            if x == 0.0 {
                return String::from("0");
            }
            if x < 0.0 {
                return format!("-{}", ecma(-x));
            }
            if x.is_infinite() {
                return String::from("Infinity");
            }
            let (digits, n) = digits(x);
            let k = digits.len() as i32;
            if k <= n && n <= 21 {
                format!("{digits}{}", "0".repeat((n - k) as usize))
            } else if 0 < n && n <= 21 {
                let (head, tail) = digits.split_at(n as usize);
                format!("{head}.{tail}")
            } else if -6 < n && n <= 0 {
                format!("0.{}{digits}", "0".repeat((-n) as usize))
            } else {
                let e = n - 1;
                let (head, tail) = digits.split_at(1);
                let point = if tail.is_empty() { String::new() } else { format!(".{tail}") };
                format!("{head}{point}e{}{}", if e < 0 { '-' } else { '+' }, e.unsigned_abs())
            }
        }
        if x.is_nan() {
            return String::from("NaN");
        }
        if x.is_infinite() {
            return String::from(if x > 0.0 { "inf" } else { "-inf" });
        }
        if x.fract() == 0.0 && x.abs() < 1e21 {
            let sign = if x == 0.0 && x.is_sign_negative() { "-" } else { "" };
            return format!("{sign}{}.0", ecma(x));
        }
        ecma(x)
    }

    /// **The stack rendering is byte for byte the one it replaced**, over
    /// random bit patterns, every integer shape near 2^53, and powers of ten.
    #[test]
    fn the_stack_rendering_matches_the_string_one() {
        let mut inputs: Vec<f64> = Vec::new();
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..500_000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            inputs.push(f64::from_bits(s));
            inputs.push((s >> 11) as f64);
            inputs.push((s % 1_000_000) as f64 / 8.0);
        }
        for shift in 0..64 {
            let p = 2f64.powi(shift);
            inputs.extend([p - 2.0, p - 1.0, p, p + 1.0, p + 2.0, -p, p * 1.5]);
        }
        for e in -324..=308 {
            let p: f64 = format!("1e{e}").parse().unwrap();
            inputs.extend([p, -p, p * 3.0, p.next_up(), p.next_down()]);
        }
        inputs.extend([0.0, -0.0, f64::MAX, f64::MIN, f64::MIN_POSITIVE, f64::NAN, f64::INFINITY]);
        for x in inputs {
            assert_eq!(show_f64(x), show_f64_by_strings(x), "show({x:?}), bits {:#x}", x.to_bits());
        }
    }

    #[test]
    fn integers_render_as_to_string_does() {
        let mut s: u64 = 0x2545_F491_4F6C_DD1D;
        let check = |v: i64| {
            let mut b = Buf::new();
            b.push_i64(v);
            assert_eq!(b.as_bytes(), v.to_string().as_bytes());
            let mut out = crate::value::BuriStr::empty();
            // SAFETY: a writable, aligned destination.
            unsafe { crate::text::buri_rt_str_from_int(v, &raw mut out) };
            // SAFETY: the rendering just made.
            assert_eq!(unsafe { out.bytes() }, v.to_string().as_bytes());
            assert_ne!(out.len & crate::value::BURI_RT_STR_ASCII, 0);
            // SAFETY: the block the rendering allocated, held alone.
            unsafe { crate::memory::buri_rt_free(out.base) };
        };
        for v in [0, 1, -1, 9, 10, 99, 100, 101, i64::MAX, i64::MIN, i64::MIN + 1] {
            check(v);
        }
        for _ in 0..200_000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            check(s as i64);
            check((s >> (s % 64)) as i64);
        }
    }

    /// An `F32` renders as the double it widens to, not as the shortest `f32`.
    #[test]
    fn an_f32_renders_through_its_double() {
        assert_eq!(show_f64(f64::from(0.1f32)), "0.10000000149011612");
        assert_eq!(show_f64(f64::from(1.0f32)), "1.0");
    }

    /// `$show`'s array arm, spelled out: brackets always, `", "` between, and
    /// an empty list is `[]` rather than the empty string.
    #[test]
    fn a_list_renders_the_way_the_javascript_runner_renders_one() {
        let render = |items: &[&str]| {
            let strs: Vec<BuriStr> =
                items.iter().map(|s| BuriStr::copy_from(s.as_bytes())).collect();
            let mut out = BuriStr::empty();
            // SAFETY: `strs` is a live block of `BuriStr`s and `out` is a live
            // local. Every block here is this closure's, and each is given
            // back once it has been read.
            unsafe {
                buri_rt_show_list(strs.as_ptr().cast(), strs.len() as u64, &raw mut out);
                let text = String::from_utf8_lossy(out.bytes()).into_owned();
                crate::memory::buri_rt_decref(out.base, None);
                for s in &strs {
                    crate::memory::buri_rt_decref(s.base, None);
                }
                text
            }
        };
        assert_eq!(render(&[]), "[]");
        assert_eq!(render(&["1"]), "[1]");
        assert_eq!(render(&["1", "2", "3"]), "[1, 2, 3]");
        assert_eq!(render(&["\"a\"", "\"b\""]), "[\"a\", \"b\"]");
    }

    /// A null block with no elements is the empty `[T]` every backend answers
    /// for `list.empty` (VALUE-MODEL.md §4), and it renders as `[]`.
    #[test]
    fn a_null_block_renders_as_the_empty_list() {
        let mut out = BuriStr::empty();
        // SAFETY: `count` is zero, so `xs` is never dereferenced, and the
        // rendering is this test's to give back once it has been read.
        let text = unsafe {
            buri_rt_show_list(std::ptr::null(), 0, &raw mut out);
            let text = String::from_utf8_lossy(out.bytes()).into_owned();
            crate::memory::buri_rt_decref(out.base, None);
            text
        };
        assert_eq!(text, "[]");
    }
}
