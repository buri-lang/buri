//! `core/bytes` — the six entries `bytes.buri` declares as intrinsics.
//!
//! The rest of that module is written in Buri: hexadecimal, base64, varints and
//! zigzag are all arithmetic over a `[U8]` and a backend has nothing to add to
//! them. What is here is the two conversions whose answer belongs to the
//! *platform's* representation rather than to arithmetic, and `bytes.buri`'s own
//! headers say so in both cases:
//!
//!   * **UTF-8** — "a string is UTF-16 on this backend, and the encoding is the
//!     platform's business rather than something to rebuild on top of
//!     `charAt`";
//!   * **the IEEE 754 byte patterns** — "the bit pattern of a `Float` belongs to
//!     the platform, and reconstructing it from arithmetic would be a second
//!     definition of the same thing".
//!
//! ## `toUtf8` is a copy, and that is the whole of it
//!
//! `runtime.js`'s `$bytes_toUtf8` is a fourteen-line encoder because a
//! JavaScript string is UTF-16 and has to be encoded. A native `Str` **is**
//! UTF-8 (VALUE-MODEL.md §3), so the encoder here would be a decode followed by
//! the encode that undid it. The bytes are already the answer.
//!
//! The two are the same function on every input, and the reason is worth stating
//! rather than assuming: `$bytes_toUtf8` iterates `for (const ch of s)`, which
//! is code points, and re-encodes each — so it produces exactly the UTF-8 of the
//! string's scalar sequence, which is exactly what a native `Str` holds. The one
//! input where they could part company is a JavaScript string containing a lone
//! surrogate, and this language has no way to write one: `Char` is a scalar
//! value and `u32.toChar()` refuses the surrogate range.
//!
//! ## `fromUtf8` is strict, and the offset is part of the answer
//!
//! `Utf8Error(Int)` carries *where* the decoding stopped making sense, and the
//! JavaScript side returns `$err([i])` at five distinct places. Every one of
//! them is transcribed, with `i` being the index of the byte that **began** the
//! bad sequence rather than the byte that was wrong — which is a distinction a
//! reimplementation gets wrong by default and which
//! `conformance/lib/proto/test/failures.buri` asserts on.
//!
//! `std::str::from_utf8` would answer the same *verdict* and a different
//! *offset* on a truncated sequence, and it has no error at all for a value
//! this decoder rejects for a different reason. It is not used.
//!
//! ## Ownership
//!
//! `lib.rs` §3. Every result is a fresh block; every argument is borrowed.

use crate::value::{list_of_bytes, str_of, BuriList, BuriStr};
use crate::BURI_OK;

/// Borrow a `[U8]` argument. `lib.rs` §2 rule 1 flattens it to `ptr` and `len`,
/// and `U8`'s stride is one byte, so the block *is* the slice.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with a zero length.
unsafe fn octets<'a>(ptr: *const u8, len: u64) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        return &[];
    }
    // SAFETY: the caller promises `len` readable bytes.
    unsafe { std::slice::from_raw_parts(ptr, len as usize) }
}

/// `bytes.toUtf8(ctx, s) -> [U8]` — the string's own bytes, copied.
///
/// # Safety
/// `ptr`/`len` describe a readable range; `out` is writable and aligned for a
/// [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_bytes_to_utf8(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) {
    // The length field carries the ASCII flag `str.len` reads, so it is masked
    // before it is used as a byte count — the same mask `text.rs` applies.
    let n = len & crate::value::BURI_RT_STR_LEN_MASK;
    // SAFETY: the caller promises the range.
    let bytes = unsafe { octets(ptr, n) };
    let value = list_of_bytes(bytes);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
}

/// `bytes.fromUtf8(ctx, b) -> Result<Str, Utf8Error>`.
///
/// `lib.rs` §2.1's second shape: `Utf8Error` is a **struct** and not an enum, so
/// there is no variant index to name it with and the error value crosses through
/// its own out-pointer. The discriminant is [`BURI_OK`] or `0`, and `0` here
/// means "the error is at `err`" rather than "error variant zero".
///
/// # Safety
/// `ptr`/`len` describe a readable range; `out` is writable and aligned for a
/// [`BuriStr`]; `err` is writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_bytes_from_utf8(
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
    err: *mut i64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let b = unsafe { octets(ptr, len) };
    let mut text = String::with_capacity(b.len());
    let mut i = 0usize;
    // `$bytes_fromUtf8`'s loop, byte for byte. The five `return $err([i])` sites
    // are the five `fail` arms below and they report the same `i`.
    let fail = |err: *mut i64, at: usize| -> i32 {
        // SAFETY: the caller promises a writable, aligned word.
        unsafe { err.write(at as i64) };
        0
    };
    while i < b.len() {
        let c = *b.get(i).unwrap_or(&0);
        let (mut cp, n) = if c < 0x80 {
            (u32::from(c), 0usize)
        } else if c & 0xE0 == 0xC0 {
            (u32::from(c & 0x1F), 1)
        } else if c & 0xF0 == 0xE0 {
            (u32::from(c & 0x0F), 2)
        } else if c & 0xF8 == 0xF0 {
            (u32::from(c & 0x07), 3)
        } else {
            return fail(err, i);
        };
        // `i + n >= b.length` and not `>`: the continuation bytes are at
        // `i + 1 ..= i + n`, so the last one has to be *inside* the input.
        if n > 0 && i.saturating_add(n) >= b.len() {
            return fail(err, i);
        }
        for k in 1..=n {
            let cc = *b.get(i.saturating_add(k)).unwrap_or(&0);
            if cc & 0xC0 != 0x80 {
                return fail(err, i);
            }
            cp = (cp << 6) | u32::from(cc & 0x3F);
        }
        let min = match n {
            0 => 0,
            1 => 0x80,
            2 => 0x800,
            _ => 0x1_0000,
        };
        if cp < min || cp > 0x10_FFFF || (0xD800..=0xDFFF).contains(&cp) {
            return fail(err, i);
        }
        match core::char::from_u32(cp) {
            Some(ch) => text.push(ch),
            // Unreachable: the three tests above are exactly the ones
            // `from_u32` makes. Reported rather than silently dropped, so a
            // change to either stays a diagnosable disagreement.
            None => return fail(err, i),
        }
        i = i.saturating_add(n).saturating_add(1);
    }
    let value = str_of(&text);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// The one NaN. A payload is not part of a `Float`'s value — SPEC 6.2 rules
/// every NaN `==` every other, regardless of sign or payload — and the only way
/// to construct one is to decode it out of bytes, which is where this stands.
///
/// Without it the two backends disagree on a pure byte-level round trip: a
/// `Float` on JavaScript is a `number`, and moving a NaN through one is what
/// canonicalizes it, so the payload cannot survive there whatever the decoder
/// does. Native is the side that moves. VALUE-MODEL.md §12 row 16.
fn canonical(x: f64) -> f64 {
    if x.is_nan() {
        f64::NAN
    } else {
        x
    }
}

/// `bytes.f64ToBytes(ctx, x) -> [U8]` — eight octets, **little-endian**, which
/// is `setFloat64(0, x, true)`.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_bytes_f64_to_bytes(x: f64, out: *mut BuriList) {
    let value = list_of_bytes(&x.to_bits().to_le_bytes());
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
}

/// `bytes.f32ToBytes(ctx, x) -> [U8]` — four octets, little-endian.
///
/// The argument is a `Float`, which is an `F64`, so the rounding to binary32 is
/// part of the operation and not a coincidence of the cast: `setFloat32` rounds
/// to nearest-even, and so does `as f32`.
///
/// # Safety
/// As [`buri_rt_bytes_f64_to_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_bytes_f32_to_bytes(x: f64, out: *mut BuriList) {
    let value = list_of_bytes(&(x as f32).to_bits().to_le_bytes());
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
}

/// `bytes.f64FromBytes(b, at) -> Option<Float>` — `lib.rs` §2 rule 3.
///
/// `.None` for a negative `at` and for an input too short to hold eight octets
/// from there, which is `$bytes_f64FromBytes`'s one guard. It is **not** an
/// abort: a short input is a value this function has an answer for.
///
/// A NaN comes back **canonical**, payload and sign discarded, which is what
/// [`canonical`] is for. VALUE-MODEL.md §12 row 16.
///
/// # Safety
/// `ptr`/`len` describe a readable range; `out` is writable and aligned for an
/// `f64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_bytes_f64_from_bytes(
    ptr: *const u8,
    len: u64,
    at: i64,
    out: *mut f64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let b = unsafe { octets(ptr, len) };
    let Ok(start) = usize::try_from(at) else { return 0 };
    let Some(window) = b.get(start..start.saturating_add(8)) else { return 0 };
    let mut raw = [0u8; 8];
    raw.copy_from_slice(window);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(canonical(f64::from_bits(u64::from_le_bytes(raw)))) };
    BURI_OK
}

/// `bytes.f32FromBytes(b, at) -> Option<Float>` — four octets, widened.
///
/// The answer is a `Float`, so the `f32` is promoted, which is exact. A NaN is
/// canonicalized as it is at eight octets: promotion carries a payload across
/// and there is nothing on the far side that could read it.
///
/// # Safety
/// As [`buri_rt_bytes_f64_from_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_bytes_f32_from_bytes(
    ptr: *const u8,
    len: u64,
    at: i64,
    out: *mut f64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let b = unsafe { octets(ptr, len) };
    let Ok(start) = usize::try_from(at) else { return 0 };
    let Some(window) = b.get(start..start.saturating_add(4)) else { return 0 };
    let mut raw = [0u8; 4];
    raw.copy_from_slice(window);
    let widened = f64::from(f32::from_bits(u32::from_le_bytes(raw)));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(canonical(widened)) };
    BURI_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Result<String, i64> {
        let mut out = BuriStr::empty();
        let mut err = 0i64;
        // SAFETY: both destinations are live locals, and the slice is live.
        let disc = unsafe {
            buri_rt_bytes_from_utf8(bytes.as_ptr(), bytes.len() as u64, &raw mut out, &raw mut err)
        };
        if disc == BURI_OK {
            // SAFETY: the entry wrote a live `Str`.
            Ok(unsafe { out.as_str() }.into_owned())
        } else {
            Err(err)
        }
    }

    #[test]
    fn decoding_accepts_what_the_javascript_decoder_accepts() {
        assert_eq!(decode(b"").unwrap(), "");
        assert_eq!(decode(b"hi").unwrap(), "hi");
        assert_eq!(decode(&[0xC3, 0xA9]).unwrap(), "é");
        assert_eq!(decode(&[0xE6, 0xBC, 0xA2]).unwrap(), "漢");
        assert_eq!(decode(&[0xF0, 0x9F, 0x98, 0x80]).unwrap(), "😀");
    }

    /// The five `return $err([i])` sites, and the offset each reports: the byte
    /// that **began** the sequence, not the byte that was wrong.
    #[test]
    fn decoding_refuses_what_the_javascript_decoder_refuses() {
        // A continuation byte with nothing in front of it.
        assert_eq!(decode(&[0x80]), Err(0));
        assert_eq!(decode(&[0xF8]), Err(0));
        // Truncated: two bytes of a three-byte sequence.
        assert_eq!(decode(&[b'a', 0xE6, 0xBC]), Err(1));
        // A continuation byte that is not one.
        assert_eq!(decode(&[0xE6, 0x41, 0xA2]), Err(0));
        // Overlong: `/` written in two bytes.
        assert_eq!(decode(&[0xC0, 0xAF]), Err(0));
        // A surrogate, which has a UTF-8 spelling and is not a scalar value.
        assert_eq!(decode(&[0xED, 0xA0, 0x80]), Err(0));
        // Above U+10FFFF.
        assert_eq!(decode(&[0xF7, 0xBF, 0xBF, 0xBF]), Err(0));
        // The offset is where the bad sequence starts, after good text.
        assert_eq!(decode(&[b'o', b'k', 0xC0, 0xAF]), Err(2));
    }

    #[test]
    fn the_float_patterns_are_little_endian() {
        let mut list = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `list` is a live local.
        unsafe { buri_rt_bytes_f64_to_bytes(1.0, &raw mut list) };
        // SAFETY: the entry wrote a live block of eight octets.
        let bytes = unsafe { std::slice::from_raw_parts(list.ptr, list.len as usize) };
        assert_eq!(bytes, &[0, 0, 0, 0, 0, 0, 0xF0, 0x3F]);

        let mut back = 0f64;
        // SAFETY: the slice and the destination are live.
        let disc = unsafe {
            buri_rt_bytes_f64_from_bytes(bytes.as_ptr(), 8, 0, &raw mut back)
        };
        assert_eq!(disc, BURI_OK);
        assert!((back - 1.0).abs() < f64::EPSILON);

        // Short, and negative: both are `.None` rather than an abort.
        // SAFETY: as above.
        assert_eq!(unsafe { buri_rt_bytes_f64_from_bytes(bytes.as_ptr(), 8, 1, &raw mut back) }, 0);
        // SAFETY: as above.
        assert_eq!(unsafe { buri_rt_bytes_f64_from_bytes(bytes.as_ptr(), 8, -1, &raw mut back) }, 0);
    }

    #[test]
    fn a_thirty_two_bit_pattern_is_four_octets_and_widens_back() {
        let mut list = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `list` is a live local.
        unsafe { buri_rt_bytes_f32_to_bytes(-2.5, &raw mut list) };
        // SAFETY: the entry wrote a live block of four octets.
        let bytes = unsafe { std::slice::from_raw_parts(list.ptr, list.len as usize) };
        assert_eq!(bytes, &[0, 0, 0x20, 0xC0]);
        let mut back = 0f64;
        // SAFETY: the slice and the destination are live.
        let disc = unsafe {
            buri_rt_bytes_f32_from_bytes(bytes.as_ptr(), 4, 0, &raw mut back)
        };
        assert_eq!(disc, BURI_OK);
        assert!((back + 2.5).abs() < f64::EPSILON);
    }

    /// The payload and the sign are dropped on the way in, so the round trip
    /// answers `[0, 0, 0, 0, 0, 0, 248, 127]` here as it does on JavaScript.
    #[test]
    fn a_nan_payload_does_not_survive_the_round_trip() {
        let payloads: [[u8; 8]; 3] = [
            [1, 0, 0, 0, 0, 0, 0xF8, 0x7F],
            [2, 0, 0, 0, 0, 0, 0xF8, 0x7F],
            // Signalling, and negative.
            [1, 0, 0, 0, 0, 0, 0xF0, 0xFF],
        ];
        for raw in payloads {
            let mut back = 0f64;
            // SAFETY: the slice and the destination are live.
            let disc = unsafe { buri_rt_bytes_f64_from_bytes(raw.as_ptr(), 8, 0, &raw mut back) };
            assert_eq!(disc, BURI_OK);
            assert_eq!(back.to_bits(), f64::NAN.to_bits());

            let mut list = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: `list` is a live local.
            unsafe { buri_rt_bytes_f64_to_bytes(back, &raw mut list) };
            // SAFETY: the entry wrote a live block of eight octets.
            let bytes = unsafe { std::slice::from_raw_parts(list.ptr, list.len as usize) };
            assert_eq!(bytes, &[0, 0, 0, 0, 0, 0, 0xF8, 0x7F]);
        }
    }

    /// Four octets go the same way: promotion carries a payload across, so the
    /// canonicalization has to happen before it.
    #[test]
    fn a_thirty_two_bit_nan_payload_does_not_survive_either() {
        let raw: [u8; 4] = [1, 0, 0xC0, 0x7F];
        let mut back = 0f64;
        // SAFETY: the slice and the destination are live.
        let disc = unsafe { buri_rt_bytes_f32_from_bytes(raw.as_ptr(), 4, 0, &raw mut back) };
        assert_eq!(disc, BURI_OK);
        assert_eq!(back.to_bits(), f64::NAN.to_bits());

        let mut list = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `list` is a live local.
        unsafe { buri_rt_bytes_f32_to_bytes(back, &raw mut list) };
        // SAFETY: the entry wrote a live block of four octets.
        let bytes = unsafe { std::slice::from_raw_parts(list.ptr, list.len as usize) };
        assert_eq!(bytes, &[0, 0, 0xC0, 0x7F]);
    }
}
