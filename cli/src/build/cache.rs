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
//! key = H(action_kind, toolchain_version, build_mode,
//!         platform, arch, rule_identity, H(content of each input file),
//!         key(each input action))
//! ```
//!
//! Four properties, each ruling out a class of stale-cache bug: content rather
//! than timestamps, repository-relative paths, dependencies entering as keys,
//! and the platform in the key while tags are not — a tag decides whether a
//! build is *allowed*, never what it *produces*.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is SHA-256's: fixed-width word mixing that is defined to wrap, \
              offsets within a 64-byte block, and one deadline ten seconds out. None of it takes \
              a length or an offset from a file the user wrote"
)]

use std::path::{Path, PathBuf};

use crate::build::buildfile::{self, Platform};

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

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// The kinds of action the build graph has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Turning a `.proto` schema into a module. Keyed on the contents of every
    /// schema the rule declares, which is the whole of what the generated
    /// modules depend on — a schema may only import another in the same rule,
    /// because the modules they become import each other and that import is
    /// subject to the library boundary like any other.
    Proto,
    Compile,
    /// Turning one codegen unit's lowered IR into object bytes. One action per
    /// unit, where a unit is the set of monomorphized functions whose
    /// declaration came from one source module.
    ///
    /// Its key is content-addressed **on the IR**, not on source files, and
    /// that is the decision the whole incremental story rests on. Keying a unit
    /// on the sources of the module it came from is wrong in both directions:
    /// *unsound*, because a monomorphized unit contains instantiations
    /// requested by other modules — `core/list`'s object for a program depends
    /// on which types that program maps over — and *imprecise*, because
    /// reformatting a comment changes a file's bytes and not one instruction of
    /// its IR.
    ///
    /// See `design/native/ARCHITECTURE.md` §6. Wave 2c stores entries under it;
    /// wave 0 adds the variant so that no later wave has to edit this enum.
    Codegen,
    Link,
    Test,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Proto => "proto",
            Action::Compile => "compile",
            Action::Codegen => "codegen",
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
/// would break on every toolchain version (the key includes `arguments::VERSION`).
#[expect(
    clippy::print_stdout,
    reason = "this is `buri build --explain`'s own output — the record of what the build did — \
              rather than a diagnostic, which still leaves through Session::emit"
)]
pub fn explain(
    on: bool,
    status: Status,
    action: Action,
    label: &str,
    platform: Platform,
    key: &ActionKey,
) {
    if !on {
        return;
    }
    println!(
        "{:<6} {} {label} {} {}",
        status.name(),
        action.name(),
        platform.slug(),
        key.short()
    );
}

/// A finished cache key: the hex SHA-256 a [`KeyBuilder`] produced.
///
/// A newtype rather than a `String` because `Cache::path` splits it at byte two
/// and every caller so far happened to hand it something 64 bytes long. There is
/// now no way to hand it anything else — the only two constructors both hash,
/// and a hash is always 64 hex digits.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ActionKey(String);

impl ActionKey {
    /// The key for some bytes directly, with no action or toolchain folded in.
    /// Used where the content *is* the identity.
    pub fn of(bytes: &[u8]) -> ActionKey {
        ActionKey(hash_bytes(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first twelve hex digits, which is what `--explain` prints.
    fn short(&self) -> &str {
        self.0.get(..12).unwrap_or(&self.0)
    }

    /// The key split the way [`Cache::path`] wants it: two hex digits of
    /// directory and the rest of the name.
    fn split(&self) -> (&str, &str) {
        self.0.split_at_checked(2).unwrap_or((&self.0, ""))
    }
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn open(root: &Path) -> Cache {
        let dir = root.join(".buri/cache");
        let _ = std::fs::create_dir_all(&dir);
        Cache { dir }
    }

    fn path(&self, key: &ActionKey) -> PathBuf {
        // Two levels, so a large repository does not put a hundred thousand
        // entries in one directory. An `ActionKey` is a whole SHA-256 by
        // construction, so the split always has both halves.
        let (prefix, rest) = key.split();
        self.dir.join(prefix).join(rest)
    }

    /// Reads without the lock, on purpose.
    ///
    /// An entry appears whole or not at all — `put` renames it into place — so
    /// a reader has nothing to wait for. Taking the lock here would make every
    /// cache *hit* serialize behind every cache *write*, which is the opposite
    /// of what a cache is for.
    pub fn get(&self, key: &ActionKey) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    /// "All commands are safe to run concurrently; a file lock serializes cache
    /// writes" (CLI.md). This is that lock.
    pub fn put(&self, key: &ActionKey, data: &[u8]) {
        let p = self.path(key);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Either outcome writes: the lock is an optimisation for the rename,
        // not a correctness requirement (see [`Lock`]). Naming both is what
        // makes that a decision rather than a field nobody looks at.
        let _guard = match Lock::acquire(&self.dir) {
            LockOutcome::Held(lock) => Some(lock),
            LockOutcome::ProceedUnlocked => None,
        };
        // Written to a temporary and renamed, so a concurrent reader never
        // sees half an entry. The temporary is named for this process as well
        // as for the key, so that two writers of one key never share a file
        // even in the window where the lock has been abandoned.
        let tmp = p.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The file lock that serializes cache writes.
///
/// `create_new` on a lock file, which is one atomic operation on every
/// filesystem the toolchain runs on and needs nothing from libc. Held for the
/// length of one write and no longer, because what has to be serialized is the
/// write and not the build — two `buri build` processes on one repository
/// should overlap, and only meet at the moment they both have an entry to
/// store.
///
/// Two ways out other than success, and both of them are deliberate:
///
/// - **A lock older than [`STALE`] is stolen.** A process killed mid-write
///   leaves its lock file behind, and a repository that can be wedged by one
///   `^C` is a repository nobody trusts. Stealing is safe because the write it
///   interrupts is a rename of a content-addressed name: the loser's bytes and
///   the winner's bytes are the same bytes.
/// - **After [`PATIENCE`] the write proceeds unlocked.** The lock is an
///   optimisation for the rename, not a correctness requirement — a build that
///   hangs waiting for one would be a worse failure than a build that writes an
///   entry two processes agree about.
pub struct Lock {
    path: PathBuf,
}

/// What [`Lock::acquire`] came back with. The two are different values because
/// they are different situations: `ProceedUnlocked` carries nothing to drop and
/// nothing to release, and making the caller name it is what stops "the lock
/// timed out" from being a `Lock` whose type says it is held.
#[must_use]
pub enum LockOutcome {
    Held(Lock),
    ProceedUnlocked,
}

/// How long to wait for another process's write.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
/// After this, a lock file is a crashed process's rather than a live one's.
const STALE: std::time::Duration = std::time::Duration::from_secs(30);

impl Lock {
    pub fn acquire(dir: &Path) -> LockOutcome {
        let path = dir.join(".lock");
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            if std::fs::OpenOptions::new().create_new(true).write(true).open(&path).is_ok() {
                return LockOutcome::Held(Lock { path });
            }
            let abandoned = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|age| age > STALE);
            if abandoned {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if std::time::Instant::now() >= deadline {
                return LockOutcome::ProceedUnlocked;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Builds an action key. Everything that can affect the output goes in, and
/// nothing else does.
pub struct KeyBuilder {
    hasher: Sha256,
}

impl KeyBuilder {
    pub fn new(action: Action, mode: crate::commands::arguments::BuildMode) -> KeyBuilder {
        let mut hasher = Sha256::new();
        hasher.text(action.name());
        // `--release` and `--debug` are part of the cache key.
        hasher.text(mode.name());
        // The compiler's own identity: an artifact built by a different
        // compiler is a different artifact, so a release moves every key in
        // every repository at once.
        //
        // It is `CARGO_PKG_VERSION` — a version, not a hash of the running
        // executable. Two `buri` binaries built from different source at the
        // same version therefore compute the same keys and share a cache, and
        // nothing here can tell them apart. That is a hazard for whoever
        // rebuilds this toolchain and then compares one repository's artifacts
        // across the rebuild; `buri docs build/hermeticity`, "The toolchain in
        // the key", says what to do about it. Closing it would mean hashing the
        // binary on every key — or a build-script fingerprint over `cli/src`,
        // which `cli/build.rs` deliberately keeps out of its rerun set so that
        // an edit to the compiler does not re-invoke `rustc` on the runtime.
        // What a *user* can vary is caught: `Backend::identity` carries the
        // LLVM the binary was linked against, and `Linker::version` the linker
        // it found.
        hasher.text(crate::commands::arguments::VERSION);
        KeyBuilder { hasher }
    }

    /// The platform is in the key. Tags are not, on purpose: a tag decides
    /// whether a build is allowed, never what it produces, so tagging a
    /// library differently invalidates no cache entry.
    pub fn platform(&mut self, platform: Platform, arch: Option<buildfile::Arch>) {
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
    pub fn dependency(&mut self, key: &ActionKey) {
        self.hasher.text(key.as_str());
    }

    /// Which backend produced the bytes, and the identity of everything
    /// outside the program that they depend on.
    ///
    /// Two fields rather than one because they answer different questions.
    /// `name` is which of `js`/`cranelift`/`llvm` ran, which a key that did not
    /// name it could not tell apart. `identity` is what neither the name nor
    /// the toolchain version catches: `llvm-sys` links against whatever
    /// `llvm-config` found at build time, so two `buri` binaries with
    /// identical Rust source and the same version can have
    /// different LLVM underneath, and `Profile::Release` on LLVM 20 must not
    /// share a cache entry with `Profile::Release` on LLVM 21. The build system
    /// has no way to ask, so the backend answers (`Backend::identity`).
    pub fn backend(&mut self, name: &str, identity: &str) {
        self.hasher.text(name);
        self.hasher.text(identity);
    }

    /// The linker's identity, for the same reason [`KeyBuilder::backend`]
    /// exists: `ld64` and `mold` do not produce the same bytes.
    pub fn linker(&mut self, name: &str, version: &str) {
        self.hasher.text(name);
        self.hasher.text(version);
    }

    pub fn finish(self) -> ActionKey {
        ActionKey(self.hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::arguments::BuildMode;

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
        let mut a = KeyBuilder::new(Action::Compile, BuildMode::Debug);
        a.rule_identity("//lib/money", "library", &["cents.buri".into()]);
        let mut b = KeyBuilder::new(Action::Compile, BuildMode::Debug);
        b.rule_identity("//lib/money", "library", &["cents.buri".into()]);
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn the_build_mode_changes_the_key() {
        let debug = KeyBuilder::new(Action::Compile, BuildMode::Debug).finish();
        let release = KeyBuilder::new(Action::Compile, BuildMode::Release).finish();
        assert_ne!(debug, release);
    }

    /// The toolchain's identity is in every key, and since `REPO.buri` stopped
    /// naming a toolchain it is `arguments::VERSION` and nothing else. A
    /// release moves every key in every repository, which is the row
    /// HERMETICITY-AND-CACHING.md promises and what the pin used to carry.
    ///
    /// It is asserted by rebuilding the key field by field rather than by
    /// moving the version, because there is nothing left in a repository to
    /// move: the version is a constant compiled into this binary, so varying it
    /// would take a second binary to compare against. What can be held is that
    /// it is in there, in a key that holds nothing else — a version dropped
    /// from the key, or a fourth field slipped in beside it, fails here.
    #[test]
    fn the_toolchain_version_is_in_every_key() {
        let mut expected = Sha256::new();
        expected.text("compile");
        expected.text("debug");
        expected.text(crate::commands::arguments::VERSION);
        assert_eq!(
            KeyBuilder::new(Action::Compile, BuildMode::Debug).finish(),
            ActionKey(expected.finish()),
            "the key a build starts from is no longer the action, the mode and the version"
        );

        // The negative twin: the same key with any other version in it is a
        // different key, so a release cannot be served a cache entry another
        // toolchain wrote.
        let mut other = Sha256::new();
        other.text("compile");
        other.text("debug");
        other.text("0.0.0-some-other-toolchain");
        assert_ne!(
            KeyBuilder::new(Action::Compile, BuildMode::Debug).finish(),
            ActionKey(other.finish())
        );
    }

    // -----------------------------------------------------------------------
    // Key composition
    // -----------------------------------------------------------------------
    //
    // The four properties HERMETICITY-AND-CACHING.md names, each asserted on
    // the builder rather than through a build, because "the platform is in the
    // key" is a claim about the key and a build can only show its shadow.

    /// The platform and the arch are the only things a build varies along, and
    /// both are in the key. The same library built for `linux/x86_64` and for
    /// `js` is two entries, and nothing is reused between them.
    #[test]
    fn the_platform_and_the_arch_are_in_the_key() {
        let key = |p: Platform, a: Option<buildfile::Arch>| {
            let mut k = KeyBuilder::new(Action::Compile, BuildMode::Debug);
            k.platform(p, a);
            k.rule_identity("//lib/money", "library", &["cents.buri".into()]);
            k.finish()
        };
        let js = key(Platform::Js, None);
        let linux = key(Platform::Linux, None);
        let macos = key(Platform::Macos, None);
        assert_ne!(js, linux, "js and linux share a key");
        assert_ne!(linux, macos, "linux and macos share a key");

        let x86 = key(Platform::Linux, Some(buildfile::Arch::X86_64));
        let arm = key(Platform::Linux, Some(buildfile::Arch::Arm64));
        assert_ne!(x86, arm, "two architectures share a key");
        assert_ne!(x86, linux, "naming an arch and leaving it out share a key");
    }

    /// Rule identity — the label, the rule kind, and the ordered source paths —
    /// is in the key independently of what the sources contain. Two rules whose
    /// files hold the same bytes are still two rules.
    #[test]
    fn rule_identity_is_in_the_key() {
        let key = |label: &str, kind: &str, sources: &[&str]| {
            let mut k = KeyBuilder::new(Action::Compile, BuildMode::Debug);
            let sources: Vec<String> = sources.iter().map(|s| (*s).to_string()).collect();
            k.rule_identity(label, kind, &sources);
            // The same bytes under every spelling, so nothing below can be
            // explained by the contents having moved.
            for s in &sources {
                k.input(s, b"the same bytes");
            }
            k.finish()
        };
        let base = key("//lib/money", "library", &["cents.buri"]);
        assert_ne!(base, key("//lib/ledger", "library", &["cents.buri"]), "the label is not in the key");
        assert_ne!(base, key("//lib/money", "binary", &["cents.buri"]), "the rule kind is not in the key");
        assert_ne!(base, key("//lib/money", "library", &["pence.buri"]), "a source's path is not in the key");
        assert_ne!(
            base,
            key("//lib/money", "library", &["cents.buri", "extra.buri"]),
            "adding a source did not change the key"
        );
    }

    /// A dependency enters as its key, never as its contents. What follows is
    /// the property that makes incrementality work at all: two dependencies
    /// that hash to one key are one dependency as far as a dependent is
    /// concerned, whatever they hold.
    #[test]
    fn a_dependency_enters_as_its_key_and_not_its_contents() {
        let dependent = |dep: &[u8]| {
            let mut k = KeyBuilder::new(Action::Link, BuildMode::Debug);
            k.rule_identity("//cmd/web", "binary", &["main.buri".into()]);
            k.input("cmd/web/main.buri", b"the binary's own source");
            k.dependency(&ActionKey::of(dep));
            k.finish()
        };
        // One dependency, two states of its source tree that hash the same
        // because nothing output-determining moved.
        assert_eq!(dependent(b"aaaa"), dependent(b"aaaa"));
        assert_ne!(dependent(b"aaaa"), dependent(b"bbbb"), "a dependency's key is not in the key");

        // And the negative twin: the builder has no way to fold a dependency's
        // *contents* in, so there is nowhere for a body edit to enter except
        // through the key it did or did not change.
        let mut by_key = KeyBuilder::new(Action::Link, BuildMode::Debug);
        by_key.dependency(&ActionKey::of(b"aaaa"));
        let mut by_content = KeyBuilder::new(Action::Link, BuildMode::Debug);
        by_content.input("aaaa", b"");
        assert_ne!(by_key.finish(), by_content.finish());
    }

    /// The lock is held for a write and released, so a second acquisition in
    /// the same process is immediate rather than a deadlock.
    #[test]
    fn the_write_lock_is_released_when_the_write_finishes() {
        let dir = std::env::temp_dir().join(format!("buri-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let held = Lock::acquire(&dir);
            assert!(matches!(held, LockOutcome::Held(_)), "the lock was not taken");
            assert!(dir.join(".lock").exists(), "the lock file was not created");
        }
        assert!(!dir.join(".lock").exists(), "the lock outlived the write");
        let started = std::time::Instant::now();
        drop(Lock::acquire(&dir));
        assert!(started.elapsed() < PATIENCE, "a released lock was waited for");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two writers of one key agree, because the key names the bytes.
    #[test]
    fn a_second_writer_of_one_key_leaves_the_entry_intact() {
        let root = std::env::temp_dir().join(format!("buri-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = Cache::open(&root);
        let key = ActionKey::of(b"an entry");
        cache.put(&key, b"an entry");
        cache.put(&key, b"an entry");
        assert_eq!(cache.get(&key).as_deref(), Some(&b"an entry"[..]));
        // Nothing half-written is left where a reader could find it.
        let (prefix, _) = key.split();
        let leftovers: Vec<String> = std::fs::read_dir(root.join(".buri/cache").join(prefix))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporaries left behind: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
