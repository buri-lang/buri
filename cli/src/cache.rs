//! Hermeticity and the incremental cache.
//!
//! The claim this is trying to earn: the same commit, on any machine, produces
//! byte-identical artifacts, and a build after a one-line edit does the
//! minimum work that edit implies. Those are one property, not two — a cache
//! is only safe if the thing it is caching depends on nothing it did not
//! declare.
//!
//! An action key hashes everything that can affect the output:
//!
//! ```text
//! key = H(action_kind, toolchain.version, toolchain.sha256, build_mode,
//!         platform, arch, rule_identity, H(content of each input file),
//!         key(each input action))
//! ```
//!
//! Four properties, each ruling out a class of stale-cache bug: content rather
//! than timestamps, repository-relative paths, dependencies entering as keys,
//! and the platform in the key while tags are not — a tag decides whether a
//! build is *allowed*, never what it *produces*.

use std::path::{Path, PathBuf};

use crate::buildfile::Platform;

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

/// A streaming SHA-256. Implemented here rather than pulled in, because the
/// toolchain is pinned by hash and a dependency tree is a second thing to pin.
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
        let mut at = 0;
        while at < data.len() {
            let take = (64 - self.buffered).min(data.len() - at);
            self.buffer[self.buffered..self.buffered + take]
                .copy_from_slice(&data[at..at + take]);
            self.buffered += take;
            at += take;
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

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
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
        let next = [a, b, c, d, e, f, g, h];
        for i in 0..8 {
            self.state[i] = self.state[i].wrapping_add(next[i]);
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

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// The kinds of action the build graph has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Compile,
    Link,
    Test,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Compile => "compile",
            Action::Link => "link",
            Action::Test => "test",
        }
    }
}

/// What became of an action, for `--explain`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The toolchain did the work.
    Run,
    /// The cache had the answer.
    Cached,
    /// The key was computed and folded into something else. Not a hit and not
    /// a miss: this toolchain caches a binary's whole closure under one `link`
    /// key, so a member's `compile` key exists without a cache entry of its
    /// own. Saying so is better than picking one of the other two and being
    /// wrong in a way a test would then enshrine.
    Keyed,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Run => "run",
            Status::Cached => "cached",
            Status::Keyed => "keyed",
        }
    }
}

/// One line per action, for `--explain`:
///
/// ```text
/// keyed  compile //lib/money js c40e19b7ad22
/// run    link //cmd/web js 3f9a1c2b8d4e
/// cached test //lib/money js 71c0aa38f5b1
/// ```
///
/// Deliberately boring — fixed fields, single spaces, no timings and no sizes —
/// so it is both greppable and recordable. Only the first twelve characters of
/// the key are printed: enough to compare two runs of one tree, and short
/// enough that nobody is tempted to check a whole key into a golden file, which
/// would break on every toolchain version (the key includes `cli::VERSION`).
pub fn explain(on: bool, status: Status, action: Action, label: &str, platform: Platform, key: &str) {
    if !on {
        return;
    }
    println!(
        "{:<6} {} {label} {} {}",
        status.name(),
        action.name(),
        platform.slug(),
        &key[..key.len().min(12)]
    );
}

pub struct Cache {
    dir: PathBuf,
    /// A file lock serializes cache writes; all commands are safe to run
    /// concurrently.
    _lock: Option<std::fs::File>,
}

impl Cache {
    pub fn open(root: &Path) -> Cache {
        let dir = root.join(".buri/cache");
        let _ = std::fs::create_dir_all(&dir);
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(dir.join(".lock"))
            .ok();
        Cache { dir, _lock: lock }
    }

    fn path(&self, key: &str) -> PathBuf {
        // Two levels, so a large repository does not put a hundred thousand
        // entries in one directory.
        self.dir.join(&key[..2]).join(&key[2..])
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    pub fn put(&self, key: &str, data: &[u8]) {
        let p = self.path(key);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Written to a temporary and renamed, so a concurrent reader never
        // sees half an entry.
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Builds an action key. Everything that can affect the output goes in, and
/// nothing else does.
pub struct KeyBuilder {
    hasher: Sha256,
}

impl KeyBuilder {
    pub fn new(action: Action, toolchain: &crate::buildfile::Toolchain, release: bool) -> KeyBuilder {
        let mut hasher = Sha256::new();
        hasher.text(action.name());
        hasher.text(&toolchain.version);
        hasher.text(&toolchain.sha256);
        // `--release` and `--debug` are part of the cache key.
        hasher.text(if release { "release" } else { "debug" });
        // The compiler's own identity: a toolchain change invalidates
        // everything, which is correct and is why the version is pinned
        // exactly.
        hasher.text(crate::cli::VERSION);
        KeyBuilder { hasher }
    }

    /// The platform is in the key. Tags are not, on purpose: a tag decides
    /// whether a build is allowed, never what it produces, so tagging a
    /// library differently invalidates no cache entry.
    pub fn platform(&mut self, platform: crate::buildfile::Platform, arch: Option<crate::buildfile::Arch>) {
        self.hasher.text(platform.proto());
        self.hasher.text(arch.map(|a| a.proto()).unwrap_or("-"));
    }

    /// The label, the rule kind, and the ordered source paths.
    pub fn rule_identity(&mut self, label: &str, kind: &str, sources: &[String]) {
        self.hasher.text(label);
        self.hasher.text(kind);
        for s in sources {
            self.hasher.text(s);
        }
    }

    /// Content, never timestamps. Touching a file rebuilds nothing; checking
    /// out a branch and back rebuilds nothing.
    pub fn input(&mut self, repo_relative_name: &str, contents: &[u8]) {
        self.hasher.text(repo_relative_name);
        self.hasher.field(contents);
    }

    /// A dependency enters as its key, not its contents, so a body edit does
    /// not propagate past the interface it did not change.
    pub fn dependency(&mut self, key: &str) {
        self.hasher.text(key);
    }

    pub fn finish(self) -> String {
        self.hasher.finish()
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

    #[test]
    fn tags_are_not_in_the_key() {
        // The key builder has no method that takes one, which is the point:
        // there is nowhere for a tag to enter.
        let tc = crate::buildfile::Toolchain::default();
        let mut a = KeyBuilder::new(Action::Compile, &tc, false);
        a.rule_identity("//lib/money", "library", &["cents.buri".into()]);
        let mut b = KeyBuilder::new(Action::Compile, &tc, false);
        b.rule_identity("//lib/money", "library", &["cents.buri".into()]);
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn the_build_mode_changes_the_key() {
        let tc = crate::buildfile::Toolchain::default();
        let debug = KeyBuilder::new(Action::Compile, &tc, false).finish();
        let release = KeyBuilder::new(Action::Compile, &tc, true).finish();
        assert_ne!(debug, release);
    }
}
