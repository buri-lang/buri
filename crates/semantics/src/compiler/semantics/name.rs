//! Interned names.
//!
//! A [`Name`] is a `&'static str` from a process-wide table that holds each
//! distinct spelling once, so it is `Copy` and copying a typed body copies no
//! text. The checker names every local it makes, and monomorphization and
//! inlining copy bodies whole, so a `String` per local was an allocation at
//! each of those steps.
//!
//! Entries are never freed, the same bargain [`intern`](super::intern) makes
//! for types: they are bounded by the distinct identifiers a program spells,
//! not by how many times it spells them.
//!
//! The table is sharded by hash, like the type table, so two threads naming
//! locals contend only when both reach the same shard at once.

use std::hash::{BuildHasher, BuildHasherDefault};
use std::sync::{Mutex, PoisonError};

use crate::hash::{FxHasher, Set};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(&'static str);

impl Name {
    pub fn new(text: &str) -> Name {
        let hash = BuildHasherDefault::<FxHasher>::new().hash_one(text);
        let shard = (hash >> (u64::BITS - SHARD_BITS)) as usize;
        let Some(shard) = TABLE.get(shard) else { return Name("") };
        let mut shard = shard.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = shard.get(text) {
            return Name(found);
        }
        let leaked: &'static str = Box::leak(Box::from(text));
        shard.insert(leaked);
        Name(leaked)
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::ops::Deref for Name {
    type Target = str;

    fn deref(&self) -> &str {
        self.0
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

const SHARD_BITS: u32 = 6;
const SHARDS: usize = 1 << SHARD_BITS;

static TABLE: [Mutex<Set<&'static str>>; SHARDS] =
    [const { Mutex::new(Set::with_hasher(BuildHasherDefault::new())) }; SHARDS];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_spelling_is_one_entry() {
        let a = Name::new("total");
        let b = Name::new(&String::from("total"));
        assert_eq!(a, b);
        assert!(std::ptr::eq(a.as_str(), b.as_str()));
        assert_ne!(a, Name::new("count"));
        assert_eq!(a, "total");
    }
}
