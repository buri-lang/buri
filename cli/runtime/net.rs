//! The networking stack's seam — **the crates, and nothing that calls them.**
//!
//! `manifest.toml`'s `net` feature brings `tokio`, `hyper`, `rustls` and
//! `tungstenite` into the runtime's dependency tree. This file is the whole of
//! what references them, and what it does is name one type from each. No
//! intrinsic key mangles to a symbol declared here (`runtime_native::symbol_for`
//! is the rule, and `backend/runtime_table.rs` is the table); neither backend
//! emits a call into this file; nothing in `core/` reaches it.
//!
//! That is deliberate and it is the entire slice. Bringing four crates into
//! the archive that ships inside every `buri` binary is a decision with a
//! measurable price, and the price is worth measuring *before* anything
//! depends on the answer:
//!
//! ```text
//! aarch64-apple-darwin, libburi_rt.a
//!   net off   5 987 472 bytes
//!   net on    5 987 496 bytes      +24
//! ```
//!
//! Twenty-four bytes, because `lto = "fat"` is whole-program across the
//! dependency rlibs too and Rust code that nothing reaches does not reach the
//! archive. `.github/scripts/assert-runtime-archive.sh` holds that claim in CI
//! the direct way — it greps the archive's symbol table for the four crates'
//! names and requires **none** — so the first slice that genuinely links one of
//! them will have to move the assertion deliberately rather than discover the
//! growth in a binary size six months later.
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
/// TLS 1.2 and 1.3 — `rustls`. **The bit says the crate is linked, not that a
/// handshake would succeed:** the runtime carries no crypto provider yet
/// (`manifest.toml` says why, and where the decision lives), so a `rustls`
/// connection built today would fail for want of one. Nothing can build one,
/// because nothing calls into this file at all.
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
