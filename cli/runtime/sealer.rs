//! `buri-rt-crypto`'s root: `core/crypto`'s sealing and signature checks, on
//! top of `lib.rs`'s crate.
//!
//! Only a program that seals, opens or checks a signature links this crate and
//! `ring`. The linker loads an archive member only for a symbol it needs, so
//! the checks sit in a crate of their own, and `lib.rs`'s crate never names
//! anything here (`design/PERFORMANCE.md` §6.144).
//!
//! Runtime tests build `lib.rs` as one crate with this module in it, so the
//! module reaches the rest as `crate::...`, and the `use` below gives those
//! paths the same meaning in this crate.

#![allow(clippy::missing_safety_doc)]

#[cfg(feature = "crypto")]
use buri_rt_core::{BURI_OK, abort, value};

#[cfg(feature = "crypto")]
mod crypto;
