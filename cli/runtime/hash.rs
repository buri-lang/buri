//! `derivePrimHash`: FNV-1a, exactly as `$mix` and `$hashInto` compute it.
//!
//! `Hash` returns a `U64` and every value this produces fits in 32 bits, with
//! the top half always zero. That is not a native decision — it is
//! `runtime.js:143-147`, where the accumulator is a double and 32 bits is what
//! one holds exactly through `Math.imul`. VALUE-MODEL.md §12 asks for the same
//! number on both backends, and `x.hash()` is a number a program can print, so
//! the width has to be JavaScript's rather than the machine's.
//!
//! One arm per shape `$hashInto` has for a primitive:
//!
//! | Buri type | JavaScript value | What is mixed |
//! |---|---|---|
//! | `Bool`, an integer up to 32 bits | `number` | `ToUint32(Math.trunc(x) \|\| 0)` — every bit |
//! | `I64`, `U64`, `I128`, `U128` | `bigint` | its fewest two's-complement 32-bit words, low first |
//! | `F32`, `F64` | `number` | as the integer, when an `I32` or a `U32` holds it; else its eight bytes, low first |
//!
//! The float row keeps `Equal` and `Hash` agreeing. `-0.0 == 0.0`, and both
//! are the integer `0`. SPEC 7.2 makes `NaN == NaN` true, so every NaN, of
//! either sign and any payload, mixes the quiet NaN's bytes.
//! | `Char`, `Str` | `string` | one mix per **UTF-16 code unit** |
//!
//! The last row is the one that cannot be guessed. `$hashInto` walks a string
//! with `charCodeAt`, which yields code *units*: an astral character is two
//! mixes of its surrogate halves, not one of its scalar value. A native hasher
//! that mixed scalars would agree with JavaScript on every ASCII string and
//! disagree on every emoji, which is the worst possible place to differ.
//!
//! A `Char` is a one-character string on JavaScript, so it takes the string
//! path too — [`buri_rt_hash_char`] and not [`buri_rt_mix`].

use crate::value::{BURI_RT_STR_ASCII, BURI_RT_STR_LEN_MASK};

/// The FNV-1a offset basis, and the seed `$hash` starts from.
pub const BURI_RT_HASH_SEED: u64 = 0x811c_9dc5;

/// The FNV-1a prime.
const PRIME: u32 = 0x0100_0193;

/// One 32-bit mix — `$mix(h, x)`.
///
/// `h` is a `U64` because `Hash` is declared over `U64`, and only its low 32
/// bits are ever significant; the multiply wraps, which is what `Math.imul`
/// does and what `>>> 0` then keeps.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_mix(h: u64, x: u32) -> u64 {
    let mixed = (h as u32) ^ x;
    u64::from(mixed.wrapping_mul(PRIME))
}

/// `$hashInto` at an `I64`: one mix when it fits an `I32`, which is what a
/// narrower integer holding the same value mixes, and two when it does not.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_i64(h: u64, x: i64) -> u64 {
    match i32::try_from(x) {
        Ok(narrow) => buri_rt_mix(h, narrow as u32),
        Err(_) => signed_words(h, i128::from(x)),
    }
}

/// `$hashInto` at a `U64`. From `2^63` up a third word of zeros keeps it
/// apart from the negative `I64` with the same bits, as the `bigint` loop does.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_u64(h: u64, x: u64) -> u64 {
    match i32::try_from(x) {
        Ok(narrow) => buri_rt_mix(h, narrow as u32),
        Err(_) => unsigned_words(h, u128::from(x)),
    }
}

/// `$hashInto` at an `I128`, from its low and high halves.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_i128(h: u64, lo: u64, hi: u64) -> u64 {
    signed_words(h, ((u128::from(hi) << 64) | u128::from(lo)) as i128)
}

/// `$hashInto` at a `U128`, from its low and high halves.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_u128(h: u64, lo: u64, hi: u64) -> u64 {
    unsigned_words(h, (u128::from(hi) << 64) | u128::from(lo))
}

/// `$hashInto`'s `bigint` loop: mix the low 32-bit word and shift it out,
/// until what is left is the sign that word already carries.
fn signed_words(mut h: u64, mut x: i128) -> u64 {
    loop {
        let w = x as u32;
        h = buri_rt_mix(h, w);
        x >>= 32;
        if x == if w >> 31 == 1 { -1 } else { 0 } {
            return h;
        }
    }
}

/// The same loop over a value no `i128` holds, such as `U128`'s maximum.
fn unsigned_words(mut h: u64, mut x: u128) -> u64 {
    loop {
        let w = x as u32;
        h = buri_rt_mix(h, w);
        x >>= 32;
        if x == 0 && w >> 31 == 0 {
            return h;
        }
    }
}

/// `$hashInto` at a float. One that an `I32` or a `U32` holds is one word, as
/// that integer is, because JavaScript can't tell the two apart. Any other
/// float mixes its eight bytes, low first, so that every bit reaches the low
/// bits `core/map` branches on first. Every NaN mixes the quiet NaN's.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_f64(h: u64, x: f64) -> u64 {
    // `-0.0` passes, and is the integer `0`.
    if x.trunc() == x && (-2_147_483_648.0..4_294_967_296.0).contains(&x) {
        return buri_rt_mix(h, x as i64 as u32);
    }
    let bits = if x.is_nan() { 0x7ff8_0000_0000_0000 } else { x.to_bits() };
    bits.to_le_bytes().iter().fold(h, |h, &b| buri_rt_mix(h, u32::from(b)))
}

/// `$hashInto` at a `Str`: one mix per UTF-16 code unit.
///
/// # Safety
/// `ptr` must point at `len & BURI_RT_STR_LEN_MASK` readable bytes, or be null
/// with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_hash_str(
    h: u64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> u64 {
    let n = (len & BURI_RT_STR_LEN_MASK) as usize;
    if ptr.is_null() || n == 0 {
        return h;
    }
    // SAFETY: the caller promises `n` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, n) };
    // ASCII is one code unit per byte, and so is a run of it in any string.
    if len & BURI_RT_STR_ASCII != 0 || bytes.is_ascii() {
        return bytes.iter().fold(h, |acc, &b| buri_rt_mix(acc, u32::from(b)));
    }
    let text = String::from_utf8_lossy(bytes);
    let mut acc = h;
    for unit in text.encode_utf16() {
        acc = buri_rt_mix(acc, u32::from(unit));
    }
    acc
}

/// `$hashInto` at a `Char`, which is a one-character string on JavaScript —
/// so an astral scalar is **two** mixes, of its surrogate halves.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_char(h: u64, c: u32) -> u64 {
    let ch = char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut buf = [0u16; 2];
    let mut acc = h;
    for unit in ch.encode_utf16(&mut buf) {
        acc = buri_rt_mix(acc, u32::from(*unit));
    }
    acc
}

/// `Ordered` on a `Char`, as the `Order` tag `0 | 1 | 2`.
///
/// **Scalar value order**, which is what `Char` *is*: the comparison is on the
/// code points, and VALUE-MODEL.md §1 already said so.
///
/// It transcoded to UTF-16 first and compared the units, to match a JavaScript
/// backend where a `Char` is a one-character string and `<` on one is UTF-16.
/// That put every character in U+E000..U+FFFF above every astral one, which is
/// not what a code point says and is not what the native backends' own
/// `compare_prim` — an integer comparison on the scalar — was answering. So the
/// parity it bought was against `$cmp` only, `$cmp` now routes text through
/// `$str_compare`, and this is the order both sides mean.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_char_compare(a: u32, b: u32) -> i32 {
    match a.cmp(&b) {
        std::cmp::Ordering::Less => 0,
        std::cmp::Ordering::Equal => 1,
        std::cmp::Ordering::Greater => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seed and one mix, against the numbers `$mix` produces. Computed by
    /// hand from the definition rather than copied from a run, so this is a
    /// check of the arithmetic and not of itself.
    #[test]
    fn one_mix_is_fnv_1a() {
        // (0x811c9dc5 ^ 0) * 0x01000193 mod 2^32.
        let want = u64::from(0x811c_9dc5u32.wrapping_mul(PRIME));
        assert_eq!(buri_rt_mix(BURI_RT_HASH_SEED, 0), want);
        // The accumulator is used 32 bits wide, so a set top half is ignored.
        assert_eq!(buri_rt_mix(BURI_RT_HASH_SEED | (1 << 40), 0), want);
        // And the answer never has one.
        assert_eq!(buri_rt_mix(BURI_RT_HASH_SEED, u32::MAX) >> 32, 0);
    }

    /// A wide integer mixes its fewest 32-bit words, low first, and one that
    /// fits an `I32` mixes as a narrower integer holding it does.
    #[test]
    fn a_wide_integer_mixes_every_word() {
        let s = BURI_RT_HASH_SEED;
        let mix = buri_rt_mix;
        assert_eq!(buri_rt_hash_i64(s, -1), mix(s, u32::MAX));
        assert_eq!(buri_rt_hash_i64(s, 1 << 32), mix(mix(s, 0), 1));
        assert_eq!(buri_rt_hash_i64(s, 1 << 31), mix(mix(s, 1 << 31), 0));
        assert_eq!(buri_rt_hash_i64(s, -(1 << 31) - 1), mix(mix(s, i32::MAX as u32), u32::MAX));
        assert_eq!(buri_rt_hash_u64(s, 7), mix(s, 7));
        assert_eq!(buri_rt_hash_u64(s, u64::MAX), mix(mix(mix(s, u32::MAX), u32::MAX), 0));
        assert_eq!(buri_rt_hash_i128(s, u64::MAX, u64::MAX), mix(s, u32::MAX));
        assert_eq!(buri_rt_hash_i128(s, 0, 1), mix(mix(mix(s, 0), 0), 1));
        let all = (0..4).fold(s, |h, _| mix(h, u32::MAX));
        assert_eq!(buri_rt_hash_u128(s, u64::MAX, u64::MAX), mix(all, 0));
    }

    /// A float an `I32` or a `U32` holds mixes as that integer, through
    /// `ToUint32`, and `-0.0` is `0`.
    #[test]
    fn a_float_hashes_through_to_uint32() {
        let s = BURI_RT_HASH_SEED;
        assert_eq!(buri_rt_hash_f64(s, 0.0), buri_rt_mix(s, 0));
        assert_eq!(buri_rt_hash_f64(s, -0.0), buri_rt_mix(s, 0));
        assert_eq!(buri_rt_hash_f64(s, -1.0), buri_rt_mix(s, u32::MAX));
        assert_eq!(buri_rt_hash_f64(s, -2_147_483_648.0), buri_rt_mix(s, 1 << 31));
        assert_eq!(buri_rt_hash_f64(s, 4_294_967_295.0), buri_rt_mix(s, u32::MAX));
    }

    /// Any other float mixes its eight bytes, low first, and every NaN mixes
    /// the quiet NaN's.
    #[test]
    fn a_float_mixes_every_bit() {
        let s = BURI_RT_HASH_SEED;
        let bytes = |x: f64| (0..8).fold(s, |h, i| buri_rt_mix(h, (x.to_bits() >> (8 * i)) as u32 & 255));
        for x in [0.5, 1.9, -2.75, 4_294_967_296.0, -2_147_483_649.0, f64::INFINITY, f64::MIN_POSITIVE] {
            assert_eq!(buri_rt_hash_f64(s, x), bytes(x), "{x}");
        }
        let nan = bytes(f64::from_bits(0x7ff8_0000_0000_0000));
        for bits in [0x7ff8_0000_0000_0000u64, 0xfff8_0000_0000_0000, 0x7ff0_0000_0000_0001, u64::MAX] {
            assert_eq!(buri_rt_hash_f64(s, f64::from_bits(bits)), nan, "{bits:#x}");
        }
        assert_ne!(buri_rt_hash_f64(s, 0.1), buri_rt_hash_f64(s, 0.2));
    }

    /// An astral character is two mixes, because JavaScript sees two code
    /// units. This is the row of the table that cannot be guessed.
    #[test]
    fn an_astral_char_is_two_code_units() {
        let one = buri_rt_hash_char(BURI_RT_HASH_SEED, 'a' as u32);
        assert_eq!(one, buri_rt_mix(BURI_RT_HASH_SEED, 0x61));
        let astral = buri_rt_hash_char(BURI_RT_HASH_SEED, 0x1_0000);
        let by_hand = buri_rt_mix(buri_rt_mix(BURI_RT_HASH_SEED, 0xD800), 0xDC00);
        assert_eq!(astral, by_hand);
    }

    /// The row that discriminates the two candidate orders. It asserted the
    /// UTF-16 one — U+FFFD *after* U+10000, because a surrogate pair begins at
    /// `0xD800` — and the language's order is the scalar one, so the
    /// expectation is inverted rather than the case dropped.
    #[test]
    fn characters_order_by_scalar_value() {
        assert_eq!(buri_rt_char_compare('a' as u32, 'b' as u32), 0);
        assert_eq!(buri_rt_char_compare('a' as u32, 'a' as u32), 1);
        assert_eq!(buri_rt_char_compare(0xFFFD, 0x1_0000), 0);
        assert_eq!(buri_rt_char_compare(0x1_0000, 0xFFFD), 2);
        // The issue's pair, as characters rather than as strings.
        assert_eq!(buri_rt_char_compare(0x1_F600, 0xE000), 2);
    }

    #[test]
    fn a_string_hashes_unit_by_unit() {
        let s = "ab";
        // SAFETY: `s` outlives the call.
        let got = unsafe {
            buri_rt_hash_str(BURI_RT_HASH_SEED, std::ptr::null_mut(), s.as_ptr(), 2)
        };
        let want = buri_rt_mix(buri_rt_mix(BURI_RT_HASH_SEED, 0x61), 0x62);
        assert_eq!(got, want);
    }

    /// The ASCII shortcut mixes what the code-unit walk mixes, flagged or not.
    #[test]
    fn every_string_hashes_by_its_utf16_units() {
        for text in ["", "a", "key-123", "héllo", "日本語", "a😀b", "\u{7f}\u{80}"] {
            let by_units = text.encode_utf16().fold(BURI_RT_HASH_SEED, |h, u| buri_rt_mix(h, u32::from(u)));
            let n = text.len() as u64;
            let flag = if text.is_ascii() { BURI_RT_STR_ASCII } else { 0 };
            for len in [n, n | flag] {
                // SAFETY: `text` covers `n` bytes.
                let got = unsafe { buri_rt_hash_str(BURI_RT_HASH_SEED, std::ptr::null_mut(), text.as_ptr(), len) };
                assert_eq!(got, by_units, "{text:?}");
            }
        }
    }
}
