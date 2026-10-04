//! SHA-256, in the `buri-hash` crate so that **build scripts can use it too**.
//!
//! It is the toolchain's one hash: `build::cache`'s action keys, the `link`
//! key's runtime term, and every `Backend::identity` that has bytes rather than
//! a version to name. It was written here rather than pulled in for the reason
//! the workspace manifest's dependency bar gives — an algorithm this repository
//! can write is not an admissible dependency. Where the processor has SHA-256
//! instructions it uses them (`accelerated`), and the portable compression
//! stays as the fallback and as the reference the tests hold the instructions
//! to.
//!
//! # Why a crate of its own
//!
//! `cli/build.rs` and `crates/stencil/build.rs` produce the blobs the toolchain
//! embeds — `libburi_rt.a` and `stencils-<target>.bin` — and each enters a
//! cache key as its own digest. Hashing them at **run time** costs a SHA-256
//! pass over ten megabytes in every `buri` process that reaches a native
//! backend, once, before any cache lookup can be made; hashing them at **build
//! time** costs nothing at all, because the bytes cannot change after the build
//! script has written them.
//!
//! Both build scripts list this crate as a build dependency, so the digest a
//! script bakes and the digest [`hash_bytes`] computes at run time come from
//! one function: `runtime_native::the_hash_is_of_the_bytes` asserts they are
//! the same string. `build::cache` re-exports both names, so callers still
//! spell them `build::cache::{Sha256, hash_bytes}`.
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
    compress: Compress,
}

/// Runs the compression function over whole 64-byte blocks. The length of the
/// slice is a multiple of 64; a remainder would be ignored.
type Compress = fn(&mut [u32; 8], &[u8]);

impl Default for Sha256 {
    fn default() -> Self {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256::with(accelerated().unwrap_or(compress_portable))
    }

    fn with(compress: Compress) -> Sha256 {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                0x1f83d9ab, 0x5be0cd19,
            ],
            buffer: [0; 64],
            buffered: 0,
            length: 0,
            compress,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        let mut rest = data;
        if self.buffered > 0 {
            // "Fill the rest of the block, or consume the rest of the input,
            // whichever runs out first" is exactly what `zip` means, so it is
            // written as one rather than as two lengths and a `min` to check.
            let mut took = 0;
            for (slot, &byte) in self.buffer.iter_mut().skip(self.buffered).zip(rest) {
                *slot = byte;
                took += 1;
            }
            self.buffered += took;
            rest = rest.get(took..).unwrap_or(&[]);
            if self.buffered < 64 {
                return;
            }
            (self.compress)(&mut self.state, &self.buffer);
            self.buffered = 0;
        }
        // Every whole block straight from the input, in one call, so that an
        // accelerated compression keeps the state in registers across them.
        let (blocks, tail) = rest.split_at(rest.len() - rest.len() % 64);
        if !blocks.is_empty() {
            (self.compress)(&mut self.state, blocks);
        }
        for (slot, &byte) in self.buffer.iter_mut().zip(tail) {
            *slot = byte;
        }
        self.buffered = tail.len();
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

/// Every compression this file has is the same function: the standard's,
/// written out ([`compress_portable`]), and the one the processor carries as
/// instructions — the SHA-2 extension on `aarch64`, SHA-NI on `x86_64`. A
/// machine without them, and every other architecture, takes the portable one.
/// They are asked for at run time rather than at compile time, because the
/// toolchain is built once and run on whatever machine installs it.
///
/// The instructions run the same rounds on the same words; the speed comes
/// from doing two or four of them per instruction. The tests below hold every
/// accelerated path to the portable one over random inputs of every length a
/// block boundary can fall in.
fn accelerated() -> Option<Compress> {
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("sha2") {
        return Some(arm::compress);
    }
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sha")
        && is_x86_feature_detected!("sse2")
        && is_x86_feature_detected!("ssse3")
        && is_x86_feature_detected!("sse4.1")
    {
        return Some(x86::compress);
    }
    None
}

/// The SHA-256 compression over each 64-byte block, in the shape the standard
/// states it.
#[expect(
    clippy::indexing_slicing,
    reason = "`w`, `K` and `state` are fixed-size arrays and every index is a literal loop \
              bound below their length — `i` runs under 64 and the largest lookback is \
              `i - 16`. Nothing here is derived from an input length, and writing the \
              standard's own indices is what makes this checkable against it"
)]
fn compress_portable(state: &mut [u32; 8], blocks: &[u8]) {
    for block in blocks.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        // A block's sixty-four bytes are the first sixteen big-endian words.
        for (word, chunk) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
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
        for (slot, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(add);
        }
    }
}

/// The ARMv8 SHA-2 extension: `sha256h` and `sha256h2` run four rounds on the
/// two halves of the state, and `sha256su0` and `sha256su1` extend the message
/// schedule four words at a time.
#[cfg(target_arch = "aarch64")]
mod arm {
    use super::K;
    use core::arch::aarch64::*;

    pub(super) fn compress(state: &mut [u32; 8], blocks: &[u8]) {
        // SAFETY: only ever handed out by `accelerated`, after the processor
        // said it has the extension.
        unsafe { rounds(state, blocks) }
    }

    #[target_feature(enable = "sha2")]
    unsafe fn rounds(state: &mut [u32; 8], blocks: &[u8]) {
        // SAFETY (the whole body): every load and store is sixteen bytes
        // inside an array or a block whose length is fixed — the state's
        // eight words, `K`'s sixty-four at offsets up to 60, a block's
        // sixty-four bytes at offsets up to 48.
        let mut abcd = vld1q_u32(state.as_ptr());
        let mut efgh = vld1q_u32(state.as_ptr().add(4));
        for block in blocks.as_chunks::<64>().0 {
            let (abcd_was, efgh_was) = (abcd, efgh);
            let at = block.as_ptr();
            // The words are big-endian in the block.
            let mut m0 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(at)));
            let mut m1 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(at.add(16))));
            let mut m2 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(at.add(32))));
            let mut m3 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(at.add(48))));
            // Four rounds over four schedule words and their constants.
            macro_rules! quad {
                ($m:expr, $k:expr) => {{
                    let wk = vaddq_u32($m, vld1q_u32(K.as_ptr().add($k)));
                    let abcd_in = abcd;
                    abcd = vsha256hq_u32(abcd, efgh, wk);
                    efgh = vsha256h2q_u32(efgh, abcd_in, wk);
                }};
            }
            // The next four schedule words, from the sixteen before them.
            macro_rules! extend {
                ($a:ident, $b:ident, $c:ident, $d:ident) => {
                    $a = vsha256su1q_u32(vsha256su0q_u32($a, $b), $c, $d);
                };
            }
            quad!(m0, 0);
            quad!(m1, 4);
            quad!(m2, 8);
            quad!(m3, 12);
            for k in [16, 32, 48] {
                extend!(m0, m1, m2, m3);
                quad!(m0, k);
                extend!(m1, m2, m3, m0);
                quad!(m1, k + 4);
                extend!(m2, m3, m0, m1);
                quad!(m2, k + 8);
                extend!(m3, m0, m1, m2);
                quad!(m3, k + 12);
            }
            abcd = vaddq_u32(abcd, abcd_was);
            efgh = vaddq_u32(efgh, efgh_was);
        }
        vst1q_u32(state.as_mut_ptr(), abcd);
        vst1q_u32(state.as_mut_ptr().add(4), efgh);
    }
}

/// The x86 SHA extensions: `sha256rnds2` runs two rounds on the state kept as
/// `ABEF` and `CDGH`, and `sha256msg1` and `sha256msg2` extend the message
/// schedule four words at a time.
#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::K;
    use core::arch::x86_64::*;

    pub(super) fn compress(state: &mut [u32; 8], blocks: &[u8]) {
        // SAFETY: only ever handed out by `accelerated`, after the processor
        // said it has every feature `rounds` enables.
        unsafe { rounds(state, blocks) }
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    unsafe fn rounds(state: &mut [u32; 8], blocks: &[u8]) {
        // SAFETY (the whole body): every load and store is an unaligned
        // sixteen bytes inside an array or a block whose length is fixed —
        // the state's eight words, `K`'s sixty-four at offsets up to 60, a
        // block's sixty-four bytes at offsets up to 48.
        //
        // `swap` reverses the bytes of each word: they are big-endian in the
        // block.
        let swap = _mm_set_epi64x(0x0c0d_0e0f_0809_0a0b, 0x0405_0607_0001_0203);
        let dcba = _mm_loadu_si128(state.as_ptr().cast());
        let hgfe = _mm_loadu_si128(state.as_ptr().add(4).cast());
        let cdab = _mm_shuffle_epi32(dcba, 0xb1);
        let efgh = _mm_shuffle_epi32(hgfe, 0x1b);
        let mut abef = _mm_alignr_epi8(cdab, efgh, 8);
        let mut cdgh = _mm_blend_epi16(efgh, cdab, 0xf0);
        for block in blocks.chunks_exact(64) {
            let (abef_was, cdgh_was) = (abef, cdgh);
            let at: *const __m128i = block.as_ptr().cast();
            let mut m0 = _mm_shuffle_epi8(_mm_loadu_si128(at), swap);
            let mut m1 = _mm_shuffle_epi8(_mm_loadu_si128(at.add(1)), swap);
            let mut m2 = _mm_shuffle_epi8(_mm_loadu_si128(at.add(2)), swap);
            let mut m3 = _mm_shuffle_epi8(_mm_loadu_si128(at.add(3)), swap);
            // Four rounds over four schedule words and their constants: two
            // `sha256rnds2`, the second on the upper pair.
            macro_rules! quad {
                ($m:expr, $k:expr) => {{
                    let wk = _mm_add_epi32($m, _mm_loadu_si128(K.as_ptr().add($k).cast()));
                    cdgh = _mm_sha256rnds2_epu32(cdgh, abef, wk);
                    abef = _mm_sha256rnds2_epu32(abef, cdgh, _mm_shuffle_epi32(wk, 0x0e));
                }};
            }
            // The next four schedule words, from the sixteen before them:
            // `msg1` adds σ0, the shifted pair brings in w[i-7], `msg2` adds σ1.
            macro_rules! extend {
                ($a:ident, $b:ident, $c:ident, $d:ident) => {
                    $a = _mm_sha256msg2_epu32(
                        _mm_add_epi32(_mm_sha256msg1_epu32($a, $b), _mm_alignr_epi8($d, $c, 4)),
                        $d,
                    );
                };
            }
            quad!(m0, 0);
            quad!(m1, 4);
            quad!(m2, 8);
            quad!(m3, 12);
            for k in [16, 32, 48] {
                extend!(m0, m1, m2, m3);
                quad!(m0, k);
                extend!(m1, m2, m3, m0);
                quad!(m1, k + 4);
                extend!(m2, m3, m0, m1);
                quad!(m2, k + 8);
                extend!(m3, m0, m1, m2);
                quad!(m3, k + 12);
            }
            abef = _mm_add_epi32(abef, abef_was);
            cdgh = _mm_add_epi32(cdgh, cdgh_was);
        }
        let feba = _mm_shuffle_epi32(abef, 0x1b);
        let dchg = _mm_shuffle_epi32(cdgh, 0xb1);
        let dcba = _mm_blend_epi16(feba, dchg, 0xf0);
        let hgfe = _mm_alignr_epi8(dchg, feba, 8);
        _mm_storeu_si128(state.as_mut_ptr().cast(), dcba);
        _mm_storeu_si128(state.as_mut_ptr().add(4).cast(), hgfe);
    }
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

    /// Every compression this machine can run: the portable one, and the
    /// accelerated one where the processor has it.
    fn engines() -> Vec<(&'static str, Compress)> {
        let mut out: Vec<(&'static str, Compress)> = vec![("portable", compress_portable)];
        if let Some(fast) = accelerated() {
            out.push(("accelerated", fast));
        }
        out
    }

    fn digest(compress: Compress, data: &[u8]) -> String {
        let mut h = Sha256::with(compress);
        h.update(data);
        h.finish()
    }

    #[test]
    fn every_engine_matches_the_known_vectors() {
        let million = vec![b'a'; 1_000_000];
        let vectors: [(&[u8], &str); 4] = [
            (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (&million, "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"),
        ];
        for (name, compress) in engines() {
            for (data, expected) in vectors {
                assert_eq!(digest(compress, data), expected, "{name}, {} bytes", data.len());
            }
        }
    }

    /// The accelerated path against the portable one over random bytes of
    /// every length from empty to past four blocks, and some long ones, fed
    /// both whole and in random pieces — so a block boundary falls everywhere
    /// in the buffer and in the input. On a machine with no accelerated path
    /// this compares the portable one with itself, which still holds the
    /// streaming to the one-shot hash.
    #[test]
    fn the_accelerated_path_is_the_portable_one() {
        // xorshift64: a fixed seed, so a failure names an input that comes
        // back the next run.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let fast = accelerated().unwrap_or(compress_portable);
        let lengths = (0..=300usize).chain([1_000, 4_095, 4_096, 4_097, 65_536, 100_003]);
        for len in lengths {
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let expected = digest(compress_portable, &data);
            assert_eq!(digest(fast, &data), expected, "{len} bytes in one piece");
            let mut pieces = Sha256::with(fast);
            let mut rest = &data[..];
            while !rest.is_empty() {
                let take = (next() as usize % 150).min(rest.len());
                pieces.update(&rest[..take]);
                rest = &rest[take..];
            }
            assert_eq!(pieces.finish(), expected, "{len} bytes in random pieces");
        }
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
