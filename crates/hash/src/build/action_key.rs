//! The name of one action-cache entry. Here rather than in `build::cache`
//! because a backend names what it emitted by one, and the backends sit below
//! the build system.

use super::sha256::{hash_bytes, Sha256};

/// A finished cache key: the hex SHA-256 a `KeyBuilder` produced.
///
/// A newtype rather than a `String` because `Cache::path` splits it at byte two
/// and every caller so far happened to hand it something 64 bytes long. There is
/// now no way to hand it anything else — every constructor hashes or checks,
/// and a hash is always 64 hex digits.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ActionKey(String);

impl ActionKey {
    /// The key for some bytes directly, with no action or toolchain folded in.
    /// Used where the content *is* the identity.
    pub fn of(bytes: &[u8]) -> ActionKey {
        ActionKey(hash_bytes(bytes))
    }

    /// The key a finished hasher names.
    pub fn of_hasher(hasher: Sha256) -> ActionKey {
        ActionKey(hasher.finish())
    }

    /// A key written down by [`ActionKey::as_str`] and read back, which is
    /// how one cache entry names another. `None` for anything that isn't 64
    /// hex digits.
    pub fn parse(text: &str) -> Option<ActionKey> {
        (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| ActionKey(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first twelve hex digits, which is what `--explain` prints.
    pub fn short(&self) -> &str {
        self.0.get(..12).unwrap_or(&self.0)
    }

    /// The key split the way `Cache::path` wants it: two hex digits of
    /// directory and the rest of the name.
    pub fn split(&self) -> (&str, &str) {
        self.0.split_at_checked(2).unwrap_or((&self.0, ""))
    }
}
