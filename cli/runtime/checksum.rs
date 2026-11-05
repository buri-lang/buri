//! `core/hash`'s `crc32c`: Castagnoli's CRC, reflected, starting and ending
//! with all ones.
//!
//! `hash.buri` declares the function and this answers it, because the
//! processor has the arithmetic built in: ARMv8's `crc32c` instructions and
//! SSE 4.2's `crc32` fold eight octets in one instruction. A bit at a time in
//! Buri was 10% of the slowest test suite's run.
//!
//! The instructions are optional on x86-64 and on Linux arm64, so they are
//! asked for at run time. Without them the answer comes from slicing-by-8
//! tables, which fold the same eight octets with eight lookups. Both give the
//! same number as the definition; `runtime.js` computes it a table at a time.

/// The reflected Castagnoli polynomial.
const POLY: u32 = 0x82f6_3b78;

/// `crc32c(b) -> U32`.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_hash_crc32c(ptr: *const u8, len: u64) -> u32 {
    let b = if ptr.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: the caller promises `len` readable bytes.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    };
    !update(!0, b)
}

/// The register after `b`, from `crc`, with neither inversion applied.
fn update(crc: u32, b: &[u8]) -> u32 {
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("crc") {
        // SAFETY: the feature is present.
        return unsafe { arm64(crc, b) };
    }
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: the feature is present.
        return unsafe { x86_64(crc, b) };
    }
    sliced(crc, b)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
unsafe fn arm64(mut crc: u32, b: &[u8]) -> u32 {
    use std::arch::aarch64::{__crc32cb, __crc32cd};
    let (words, rest) = b.as_chunks::<8>();
    for &w in words {
        crc = __crc32cd(crc, u64::from_le_bytes(w));
    }
    for &o in rest {
        crc = __crc32cb(crc, o);
    }
    crc
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn x86_64(crc: u32, b: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};
    let mut crc = u64::from(crc);
    let (words, rest) = b.as_chunks::<8>();
    for &w in words {
        crc = _mm_crc32_u64(crc, u64::from_le_bytes(w));
    }
    let mut crc = crc as u32;
    for &o in rest {
        crc = _mm_crc32_u8(crc, o);
    }
    crc
}

/// `TABLES[k][x]` is the register after octet `x` and then `k` zero octets.
static TABLES: [[u32; 256]; 8] = tables();

#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "constant evaluation: every index is below its bound, checked at compile time"
)]
const fn tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut x = 0;
    while x < 256 {
        let mut crc = x as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (POLY & (crc & 1).wrapping_neg());
            bit += 1;
        }
        t[0][x] = crc;
        x += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut x = 0;
        while x < 256 {
            let prev = t[k - 1][x];
            t[k][x] = (prev >> 8) ^ t[0][(prev & 0xff) as usize];
            x += 1;
        }
        k += 1;
    }
    t
}

/// Slicing-by-8: eight octets a step, each looked up as far from the end of
/// the word as it sits.
#[expect(
    clippy::indexing_slicing,
    reason = "a table has 256 entries and every index is one octet"
)]
fn sliced(mut crc: u32, b: &[u8]) -> u32 {
    let (words, rest) = b.as_chunks::<8>();
    for &[o0, o1, o2, o3, o4, o5, o6, o7] in words {
        let lo = crc ^ u32::from_le_bytes([o0, o1, o2, o3]);
        crc = TABLES[7][(lo & 0xff) as usize]
            ^ TABLES[6][((lo >> 8) & 0xff) as usize]
            ^ TABLES[5][((lo >> 16) & 0xff) as usize]
            ^ TABLES[4][(lo >> 24) as usize]
            ^ TABLES[3][usize::from(o4)]
            ^ TABLES[2][usize::from(o5)]
            ^ TABLES[1][usize::from(o6)]
            ^ TABLES[0][usize::from(o7)];
    }
    for &o in rest {
        crc = (crc >> 8) ^ TABLES[0][((crc ^ u32::from(o)) & 0xff) as usize];
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitwise(b: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &o in b {
            crc ^= u32::from(o);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (POLY & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }

    fn crc(b: &[u8]) -> u32 {
        // SAFETY: a slice is readable for its length.
        unsafe { buri_rt_hash_crc32c(b.as_ptr(), b.len() as u64) }
    }

    #[test]
    fn the_check_value_and_the_empty_input() {
        assert_eq!(crc(b"123456789"), 0xe306_9283);
        assert_eq!(crc(&[]), 0);
        // SAFETY: a null pointer with a zero length is allowed.
        assert_eq!(unsafe { buri_rt_hash_crc32c(std::ptr::null(), 0) }, 0);
    }

    #[test]
    fn the_tables_and_the_instructions_agree_with_the_definition() {
        let source: Vec<u8> = (0..4200u32).map(|i| (i * 167 + 13) as u8).collect();
        for start in 0..9 {
            for n in (0..41).chain([63, 64, 65, 255, 4096, 4097]) {
                let w = &source[start..start + n];
                assert_eq!(!sliced(!0, w), bitwise(w), "tables, {start}+{n}");
                assert_eq!(crc(w), bitwise(w), "{start}+{n}");
            }
        }
    }
}
