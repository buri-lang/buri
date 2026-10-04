//! The toolchain's hashes: [`hash`], the table hasher, and
//! [`build::sha256`], the one cryptographic hash, with the cache key built on
//! it. A crate of its own because the build scripts hash with them too.
//!
//! Module paths mirror where these files lived in `buri`, so a path like
//! `crate::build::sha256` reads the same in every crate.

pub mod hash;

pub mod build {
    pub mod action_key;
    pub mod sha256;
}
