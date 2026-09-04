//! `derivePrimHash`: FNV-1a, exactly as `$mix` and `$hashInto` compute it.
//!
//! `Hash` returns a `U64` and every value this produces fits in 32 bits, with
//! the top half always zero. That is not a native decision — it is
//! `runtime.js:143-147`, where the accumulator is a double and 32 bits is what
//! one holds exactly through `Math.imul`. VALUE-MODEL.md §12 asks for the same
//! number on both backends, and `x.hash()` is a number a program can print, so
//! the width has to be JavaScript's rather than the machine's.
//!
//! Three arms, because `$hashInto` has three shapes for a primitive:
//!
//! | Buri type | JavaScript value | What is mixed |
//! |---|---|---|
//! | `Bool`, every integer | `number` | `ToUint32(Math.trunc(x) \|\| 0)` — the low 32 bits |
//! | `F32`, `F64` | `number` | the same, so `1.9` and `1.0` collide, and `NaN` hashes as `0` |
//!
//! The float row is what keeps `Eq` and `Hash` agreeing. SPEC 7.2 makes
//! `NaN == NaN` true, so every `NaN` has to hash alike — and it does, on both
//! backends, because `|| 0` maps every payload and both signs to zero before
//! anything is mixed. A hasher that mixed the bit pattern would give two equal
//! values two hashes, and a `Map` key would be lost the moment it collided.
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

use crate::value::BURI_RT_STR_LEN_MASK;

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

/// `$hashInto` at a float: `Math.trunc(x) || 0`, then `ToUint32`.
///
/// `|| 0` is JavaScript's falsiness, and it catches three values: `NaN`, `+0`
/// and `-0` all mix as zero. `ToUint32` of a non-finite is zero as well
/// (ECMA-262 §7.1.7 step 3), so an infinity mixes as zero rather than
/// saturating.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_hash_f64(h: u64, x: f64) -> u64 {
    buri_rt_mix(h, to_uint32(x))
}

/// `ToUint32` — ECMA-262 §7.1.7, applied to the truncated value.
fn to_uint32(x: f64) -> u32 {
    let t = x.trunc();
    if !t.is_finite() || t == 0.0 {
        return 0;
    }
    // `rem_euclid` on a value that is already integral is exact for every
    // magnitude a double can hold, so this is the modulo the specification
    // asks for and not an approximation of it.
    let m = t.rem_euclid(4_294_967_296.0);
    m as u32
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

/// `Ord` on a `Char`, as the `Order` tag `0 | 1 | 2`.
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

    /// `Math.trunc(x) || 0` collapses three values to zero, and a negative
    /// number's `ToUint32` is its two's complement.
    #[test]
    fn a_float_hashes_through_to_uint32() {
        assert_eq!(to_uint32(f64::NAN), 0);
        assert_eq!(to_uint32(0.0), 0);
        assert_eq!(to_uint32(-0.0), 0);
        assert_eq!(to_uint32(f64::INFINITY), 0);
        assert_eq!(to_uint32(1.9), 1);
        assert_eq!(to_uint32(-1.0), u32::MAX);
        // Past 32 bits it wraps rather than saturating.
        assert_eq!(to_uint32(4_294_967_297.0), 1);
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
}
