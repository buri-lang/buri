//! `buri-rt-paint`'s root: the painter, on top of `lib.rs`'s crate.
//!
//! Only a program that takes a snapshot links the painter and its three
//! crates. The linker loads an archive member only for a symbol it needs, so
//! the painter sits in a crate of its own, and `lib.rs`'s crate never names
//! anything here (`design/PERFORMANCE.md` §6.143).
//!
//! Runtime tests build `lib.rs` as one crate with these modules in it, so a
//! module here reaches the rest as `crate::...`, and the `use` below gives
//! those paths the same meaning in this crate.

#![allow(clippy::missing_safety_doc)]

use buri_rt_core::{abort, host, sync, ui, value};

mod paint;
mod snapshot;
