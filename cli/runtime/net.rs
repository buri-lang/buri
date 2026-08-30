//! The networking stack's seam — **the crates, and what does or does not call
//! them.**
//!
//! `manifest.toml`'s `net` feature brings `tokio`, `hyper`, `rustls`, `ring`
//! and `tungstenite` into the runtime's dependency tree. This file names one
//! type from each, which for three of them is still the *whole* of what
//! references them: no intrinsic key mangles to a symbol declared here
//! (`runtime_native::symbol_for` is the rule, and `backend/runtime_table.rs` is
//! the table); neither backend emits a call into this file; nothing in `core/`
//! reaches it.
//!
//! The crates landed a slice ahead of any code that uses them so that the price
//! could be measured *before* anything depended on the answer. The bill, on
//! aarch64-apple-darwin, and the second row is what `https://` cost:
//!
//! ```text
//! libburi_rt.a
//!   net off                                 6 052 464 bytes
//!   net on, tokio/hyper/rustls/tungstenite   6 052 488        +24
//!   net on, https:// through rustls + ring   7 857 056 +1 804 592
//! ```
//!
//! Twenty-four bytes for four unreferenced crates, because `lto = "fat"` is
//! whole-program across the dependency rlibs too and Rust code that nothing
//! reaches does not reach the archive. **1.72 MiB** for the two that are now
//! reached, about 845 KB of it `ring`'s native object code, which a `staticlib`
//! bundles whether the linker needs it or not and which therefore no amount of
//! LTO removes.
//!
//! `.github/scripts/assert-runtime-archive.sh` holds the remaining claim in CI
//! the direct way — it greps the archive's symbol table for `tokio`, `hyper`
//! and `tungstenite` and requires **none** — and `rustls` and `ring` left that
//! list in the commit that linked them, which is the assertion being moved
//! deliberately rather than the growth being discovered in a binary six months
//! later.
//!
//! ## What the two entries below are for
//!
//! They are not scaffolding in the sense of a stub to be filled in — the
//! carrier runtime is `rt.rs` (design/native, track B) and the HTTP client is
//! `http.rs`. They are the *question* "was this toolchain built with the
//! networking stack" made answerable **from a generated program**.
//!
//! The toolchain asks the same question and does **not** ask it here. Reading
//! this constant back out of the archive's symbol table was one of the three
//! candidates and it lost: it would make the compiler parse `ar` and a Mach-O
//! or ELF symbol table to learn something `cli/build.rs` already knew when it
//! ran the build. The script writes `libburi_rt.a.features` beside the archive
//! instead, and `runtime_native::net()` is a line of an `include_str!` of it —
//! `Backend::missing_intrinsics` reads that. So there are two answers to one
//! question, deliberately, on the two sides of the C ABI: this one is for a
//! program that has already been linked, and the file is for the compiler
//! deciding whether to link it at all.
//!
//! They obey `lib.rs` §1 and §2 like every other entry: `buri_rt_` plus
//! `snake_case`, `extern "C"`, scalar parameters and a scalar result. Nothing
//! here allocates, so §3's ownership rules have nothing to say about it.

/// The reactor, the timer wheel and the carrier pool — `tokio`.
pub const BURI_NET_TOKIO: i64 = 1 << 0;
/// HTTP/1.1 and HTTP/2 framing — `hyper`.
pub const BURI_NET_HYPER: i64 = 1 << 1;
/// TLS 1.2 and 1.3 — `rustls` over the `ring` provider. Unlike its three
/// neighbours this bit now means a working capability rather than a linked
/// crate: `tls.rs` builds a client configuration from it and `http.rs` reaches
/// that for every `https://` URL.
pub const BURI_NET_TLS: i64 = 1 << 2;
/// RFC 6455 framing and the handshake — `tungstenite`.
pub const BURI_NET_WEBSOCKET: i64 = 1 << 3;

/// The four bits this toolchain's runtime was built with.
///
/// A `const` rather than a function body so that the answer is folded at the
/// one place it is asked, and so that the `net`-off build has no branch and no
/// data for the four crates at all.
#[cfg(feature = "net")]
const LINKED: i64 = {
    // Naming a type is the whole reference. `size_of` is a constant, so this
    // costs no code and no data — what it buys is that removing a crate from
    // `manifest.toml` stops *compiling* rather than quietly leaving a feature
    // that claims a capability the archive no longer carries.
    let _reactor = size_of::<tokio::sync::Semaphore>();
    let _http = size_of::<hyper::Method>();
    let _tls = size_of::<rustls::ClientConfig>();
    // `ring` is reached through `rustls::crypto::ring` and never named in
    // `tls.rs`, so without this line the manifest could lose the entry and
    // nothing would stop compiling — while every binary would go on carrying
    // its object code through `rustls`'s own feature. It is declared directly
    // for `dependencies_stay_behind_the_bar` to see, and it is named here for
    // the same reason the other four are.
    let _provider = size_of::<ring::digest::Context>();
    let _websocket = size_of::<tungstenite::protocol::Role>();
    BURI_NET_TOKIO | BURI_NET_HYPER | BURI_NET_TLS | BURI_NET_WEBSOCKET
};

#[cfg(not(feature = "net"))]
const LINKED: i64 = 0;

/// Which halves of the networking stack this toolchain's runtime was built
/// with, as the bitmask above.
///
/// Four bits rather than one because they are four crates behind one feature
/// today and need not stay that way: `net-h3` is already a second feature in
/// the design (quinn, default off), and a caller asking "can this binary speak
/// WebSocket" should not have to ask "was the toolchain built with HTTP".
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_net_capabilities() -> i64 {
    LINKED
}

/// Whether this toolchain's runtime has a networking stack at all.
///
/// The counterpart of `runtime_native::AVAILABLE`, one level down: that one
/// answers "is there an archive", this one answers "does the archive speak the
/// network". Both are compile-time facts about how the toolchain was built,
/// and both are false in ways a program is entitled to be told about rather
/// than to discover as a link error.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_net_available() -> i32 {
    i32::from(LINKED != 0)
}
