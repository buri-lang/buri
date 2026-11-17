//! `libburi_rt.a`'s root: the network modules, on top of `lib.rs`'s crate.
//!
//! A program that never touches the network links none of TLS, HTTP/2 or
//! WebSockets. The linker loads an archive member only for a symbol it needs,
//! so the network code sits in a crate of its own, and `lib.rs`'s crate never
//! names anything here (`design/PERFORMANCE.md` §6.142).
//!
//! Runtime tests build `lib.rs` as one crate with these modules in it, so a
//! module here reaches the rest as `crate::...`, and the `use`s below give
//! those paths the same meaning in this crate.

#![allow(clippy::missing_safety_doc)]

#[cfg(feature = "net")]
use buri_rt_core::{memory, rt};
use buri_rt_core::{BURI_OK, allocator, host, sync, value};

mod http;
mod net;
mod tcp;
/// Rustls's client and server configurations.
#[cfg(feature = "net")]
mod tls;

pub use net::*;
