//! SHA-256, in its own file so that **`cli/build.rs` can compile it too**.
//!
//! It is the toolchain's one hash: [`crate::build::cache`]'s action keys, the
//! `link` key's runtime term, and every `Backend::identity` that has bytes
//! rather than a version to name. It was written here rather than pulled in for
//! the reason the workspace manifest's dependency bar gives — an algorithm this
//! repository can write is not an admissible dependency — and it stays exactly
//! what it was; only the file changed.
//!
//! # Why a file of its own
//!
//! `cli/build.rs` produces two blobs the toolchain embeds — `libburi_rt.a` and
//! `stencils-<target>.bin` — and both of them enter a cache key as their own
//! digest. Hashing them at **run time** costs a SHA-256 pass over ten megabytes
//! in every `buri` process that reaches a native backend, once, before any
//! cache lookup can be made; hashing them at **build time** costs nothing at
//! all, because the bytes cannot change after the build script has written
//! them.
//!
//! A build script cannot use the crate it builds, so the code has to be
//! *shared* rather than called. `cli/build.rs` already shares the halves of
//! `backend/stencil` it needs the same way — `#[path = "src/…"] mod …` — and
//! this is that convention applied to the one thing both sides hash with. Sharing
//! the source rather than restating the algorithm is what keeps the digest the
//! build script bakes and the digest [`hash_bytes`] would have computed the
//! same string: `runtime_native::the_hash_is_of_the_bytes` asserts they are,
//! and it is the test that would notice if this file were ever forked.
//!
//! [`crate::build::cache`] re-exports both names, so every caller still spells
//! them `build::cache::{Sha256, hash_bytes}` and nothing outside this file and
//! that one moved.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is SHA-256's: fixed-width word mixing that is defined to wrap \
              and offsets within a 64-byte block. None of it takes a length or an offset from a \
              file the user wrote"
)]

// ---------------------------------------------------------------------------
// SHA-256
// ---------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// A streaming SHA-256. Implemented here rather than pulled in, because a
/// dependency tree is a second thing to audit for a compiler that hashes
/// everything it caches.
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                0x1f83d9ab, 0x5be0cd19,
            ],
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        let mut rest = data;
        while !rest.is_empty() {
            // "Fill the rest of the block, or consume the rest of the input,
            // whichever runs out first" is exactly what `zip` means, so it is
            // written as one rather than as two lengths and a `min` to check.
            // `buffered` is reset the moment it reaches 64, so the block always
            // has room and the loop always advances.
            let mut took = 0;
            for (slot, &byte) in self.buffer.iter_mut().skip(self.buffered).zip(rest) {
                *slot = byte;
                took += 1;
            }
            self.buffered += took;
            rest = rest.get(took..).unwrap_or(&[]);
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
    }

    /// A length-prefixed field, so that hashing `["ab", "c"]` and `["a", "bc"]`
    /// cannot collide.
    pub fn field(&mut self, data: &[u8]) {
        self.update(&(data.len() as u64).to_le_bytes());
        self.update(data);
    }

    pub fn text(&mut self, s: &str) {
        self.field(s.as_bytes());
    }

    /// One SHA-256 round over one 64-byte block, in the shape the standard
    /// states it.
    #[expect(
        clippy::indexing_slicing,
        reason = "`w`, `K` and `state` are fixed-size arrays and every index is a literal loop \
                  bound below their length — `i` runs under 64 and the largest lookback is \
                  `i - 16`. Nothing here is derived from an input length, and writing the \
                  standard's own indices is what makes this checkable against it"
    )]
    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        // `chunks_exact(4)` over 64 bytes is the first sixteen words, and the
        // fold spells the big-endian read without a fallible `try_into`.
        for (word, chunk) in w.iter_mut().zip(block.chunks_exact(4)) {
            *word = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, add) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(add);
        }
    }

    pub fn finish(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = String::with_capacity(64);
        for word in self.state {
            out.push_str(&format!("{word:08x}"));
        }
        out
    }
}

pub fn hash_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_known_vectors() {
        assert_eq!(
            hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_bytes(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn long_input_spans_blocks() {
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            hash_bytes(&data),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn fields_are_length_prefixed() {
        // Without a length prefix these would collide.
        let mut a = Sha256::new();
        a.text("ab");
        a.text("c");
        let mut b = Sha256::new();
        b.text("a");
        b.text("bc");
        assert_ne!(a.finish(), b.finish());
    }
}
