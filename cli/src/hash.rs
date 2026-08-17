//! The hasher the compiler's tables use.
//!
//! `std`'s default is SipHash-1-3, which is chosen to survive an adversary
//! who picks the keys. Nothing here has one: the keys are a program's own
//! symbol names, type ids and module paths, all of them produced by the
//! compiler from a file the person running it wrote. What that costs is real —
//! SipHash spends longer initialising its four-word state than this spends
//! hashing a whole key, and it is too large to inline into the lookup.
//!
//! So the tables use a multiplicative hash instead: `hash = (hash + word) * K`
//! per word, with a rotate at the end to move the entropy the multiply pushed
//! into the high bits back down where a hash table takes its bucket index
//! from. This is the function rustc uses, for the same reason, and the
//! constants and the byte path are theirs — the multiplier is from Steele and
//! Vigna's spectral-test tables, the seeds are digits of pi. rustc's own
//! measurement of the alternative is the argument for it: an experiment that
//! put SipHash back in place of this made the compiler 4% to 84% slower.
//!
//! Two consequences worth stating, since neither is about speed:
//!
//! * **Iteration order becomes deterministic.** `RandomState` seeds itself
//!   per process, so two runs of the compiler on one input walked its tables
//!   in two different orders. Nothing was allowed to depend on that — build
//!   artifacts are compared byte for byte — but "nothing depends on it" was a
//!   property maintained by care rather than by construction. With a fixed
//!   seed a mistake of that kind is at least reproducible, and
//!   `buri build --check-reproducible` can see it.
//! * **This is not a hash for untrusted keys.** If something here ever hashes
//!   input from a network, it wants `RandomState` and should say so at the
//!   declaration.
//!
//! The test vectors at the bottom are rustc-hash's own, so this file can be
//! checked against the thing it is a copy of rather than only against itself.

use std::hash::{BuildHasherDefault, Hasher};

/// A `HashMap` using [`FxHasher`]. Same API as `std`'s.
pub type Map<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
/// A `HashSet` using [`FxHasher`].
pub type Set<T> = std::collections::HashSet<T, BuildHasherDefault<FxHasher>>;

/// The multiplier: an MCG constant from Steele and Vigna, "Computationally
/// Easy, Spectrally Good Multipliers for Congruential Pseudorandom Number
/// Generators".
const K: u64 = 0xf135_7aea_2e62_a9c5;

// Seeds for the byte path, and the constant that keeps a run of zero bytes
// from collapsing the state to zero. All three are digits of pi.
const SEED1: u64 = 0x243f_6a88_85a3_08d3;
const SEED2: u64 = 0x1319_8a2e_0370_7344;
const PREVENT_TRIVIAL_ZERO_COLLAPSE: u64 = 0xa409_3822_299f_31d0;

#[derive(Clone, Copy, Default)]
pub struct FxHasher {
    hash: u64,
}

/// The wide product's two halves folded together. The middle bits of a 64x64
/// multiply are the ones that depend on the most input bits, and this is what
/// brings them into range.
#[inline]
fn multiply_mix(x: u64, y: u64) -> u64 {
    let full = (x as u128).wrapping_mul(y as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

/// The first eight bytes of `b`, or zero if there are not eight. Every caller
/// has already established the length; the fallback is there so that a load can
/// be a value rather than a bounds check.
#[inline]
fn le_u64(b: &[u8]) -> u64 {
    match b.first_chunk::<8>() {
        Some(c) => u64::from_le_bytes(*c),
        None => 0,
    }
}

/// The first four bytes of `b`, or zero if there are not four.
#[inline]
fn le_u32(b: &[u8]) -> u32 {
    match b.first_chunk::<4>() {
        Some(c) => u32::from_le_bytes(*c),
        None => 0,
    }
}

/// The *last* eight bytes of `b`, or zero if there are not eight.
#[inline]
fn le_u64_tail(b: &[u8]) -> u64 {
    match b.last_chunk::<8>() {
        Some(c) => u64::from_le_bytes(*c),
        None => 0,
    }
}

/// A wyhash-style hash of a byte string, folded into the state as one word.
///
/// The short cases read the first and last chunk, which overlap for a length
/// between the two — that is deliberate, and it is what lets every length
/// below 16 be handled with two loads and no loop.
fn hash_bytes(bytes: &[u8]) -> u64 {
    let len = bytes.len();
    let mut s0 = SEED1;
    let mut s1 = SEED2;

    if len <= 16 {
        if len >= 8 {
            s0 ^= le_u64(bytes);
            s1 ^= le_u64_tail(bytes);
        } else if len >= 4 {
            s0 ^= le_u32(bytes) as u64;
            s1 ^= match bytes.last_chunk::<4>() {
                Some(c) => u32::from_le_bytes(*c) as u64,
                None => 0,
            };
        } else {
            // The one-to-three-byte case reads the first, last and middle
            // byte, which for these lengths is every byte there is. `first`
            // being `None` is the empty string, which keeps the seeds.
            if let (Some(&first), Some(&last), Some(&middle)) =
                (bytes.first(), bytes.last(), bytes.get(len / 2))
            {
                s0 ^= first as u64;
                s1 ^= ((last as u64) << 8) | middle as u64;
            }
        }
    } else {
        // One byte short of the whole, so that the tail below always overlaps
        // the bulk rather than starting after it — every byte is then covered
        // by a full 16-byte read.
        let bulk = bytes.split_last().map_or(bytes, |(_, rest)| rest);
        for chunk in bulk.chunks_exact(16) {
            let x = le_u64(chunk);
            let y = le_u64_tail(chunk);
            // Two streams rather than one, so the multiplies pipeline instead
            // of forming a single dependency chain.
            let t = multiply_mix(s0 ^ x, PREVENT_TRIVIAL_ZERO_COLLAPSE ^ y);
            s0 = s1;
            s1 = t;
        }
        s0 ^= match bytes.last_chunk::<16>() {
            Some(c) => le_u64(c),
            None => 0,
        };
        s1 ^= le_u64_tail(bytes);
    }

    multiply_mix(s0, s1) ^ (len as u64)
}

impl FxHasher {
    #[inline]
    fn add(&mut self, i: u64) {
        self.hash = self.hash.wrapping_add(i).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.add(hash_bytes(bytes));
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.add(i as u64);
        self.add((i >> 64) as u64);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    /// The rotate is here rather than in `add` because a multiply leaves its
    /// best bits at the top and a hash table indexes from the bottom. Doing it
    /// once, at the end, keeps the per-word step a single instruction.
    #[inline]
    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_of(f: impl FnOnce(&mut FxHasher)) -> u64 {
        let mut h = FxHasher::default();
        f(&mut h);
        h.finish()
    }

    /// rustc-hash's own vectors. They are here so that this file is checkable
    /// against the implementation it was copied from, and so that a "harmless"
    /// edit to the constants above cannot pass silently.
    #[test]
    fn matches_the_reference_implementation() {
        assert_eq!(hash_of(|h| h.write_u64(0)), 0);
        assert_eq!(hash_of(|h| h.write_u64(1)), 12157901119326311915);
        assert_eq!(hash_of(|h| h.write_u64(100)), 16751747135202103309);
        assert_eq!(hash_of(|h| h.write_u64(u64::MAX)), 6288842954450348564);

        assert_eq!(hash_of(|h| h.write(b"")), 17606491139363777937);
        assert_eq!(hash_of(|h| h.write(&[1])), 5922447956811044110);
        assert_eq!(hash_of(|h| h.write(b"uwu")), 7168164714682931527);
        assert_eq!(
            hash_of(|h| h.write(b"These are some bytes for testing rustc_hash.")),
            2349210501944688211
        );
    }

    /// Every byte matters at every length, including the lengths where the
    /// first and last read overlap.
    #[test]
    fn every_length_is_sensitive_to_every_byte() {
        for len in 1..64usize {
            let base: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let h = hash_bytes(&base);
            for i in 0..len {
                let mut other = base.clone();
                other[i] ^= 0x5a;
                assert_ne!(hash_bytes(&other), h, "length {len}, byte {i} did not matter");
            }
        }
    }

    /// A run of zeroes must not collapse the state — the failure mode the
    /// third constant exists to prevent.
    #[test]
    fn zeroes_do_not_collapse() {
        let mut seen = std::collections::HashSet::new();
        for len in 0..64usize {
            assert!(seen.insert(hash_bytes(&vec![0u8; len])), "zeroes of length {len} collided");
        }
    }
}
