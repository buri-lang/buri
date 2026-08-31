//! The networking stack's seam — **the crates, and what does or does not call
//! them.**
//!
//! `manifest.toml`'s `net` feature brings `tokio`, `hyper`, `rustls`, `ring`
//! and `tungstenite` into the runtime's dependency tree, and its `net-h3`
//! feature brings `quinn`. This file names one type from each, which for three
//! of them — `hyper`, `tungstenite` and `quinn` — is still the *whole* of what
//! references them: no intrinsic key mangles to a symbol declared here
//! (`runtime_native::symbol_for` is the rule, and
//! `backend/runtime_table.rs` is the table); neither backend emits a call into
//! this file; nothing in `core/` reaches it.
//!
//! The crates landed a slice ahead of any code that uses them so that the price
//! could be measured *before* anything depended on the answer. Three of them
//! have since been linked, each in the commit that needed it and each with its
//! bytes written down: `tokio` by `rt.rs` (the reactor and its timer wheel,
//! +185 424 bytes when B6 measured it), and `rustls` and `ring` by `tls.rs`,
//! which `http.rs` calls for every `https://` URL (+1 804 592 when C7 did).
//! Where that leaves the archive, measured on aarch64-apple-darwin on this
//! tree:
//!
//! ```text
//! libburi_rt.a
//!   net off                                       6 130 608 bytes
//!   net on, the reactor and the TLS client linked 8 199 032   +2 068 424
//!   net-h3 on as well                             8 198 992         -40
//! ```
//!
//! **The third row is the argument for landing `quinn` a slice ahead of F2 all
//! over again.** A QUIC implementation costs the archive *nothing* while
//! nothing calls it — forty bytes *less* than the row above, which is the
//! HTTP/3 refusal string an h3 build has no use for. The whole crate is dropped
//! by `lto = "fat"`, exactly as four unreferenced crates were when they landed,
//! so the price of HTTP/3 is a number F2 will produce and not one this slice
//! had to guess at.
//!
//! Twenty-four bytes was what four unreferenced crates cost, because
//! `lto = "fat"` is whole-program across the dependency rlibs and Rust code
//! that nothing reaches does not reach the archive. What LTO cannot touch is a
//! dependency's **native** object code: about 845 KB of the TLS figure is
//! `ring`'s C and AArch64 assembly, which a `staticlib` bundles whether the
//! linker needs it or not.
//!
//! `.github/scripts/assert-runtime-archive.sh` holds the remaining claim in CI
//! the direct way — it greps the archive's symbol table, requires `tokio`,
//! `rustls` and `ring` to be **present** when the feature file says `net`, and
//! requires `hyper`, `tungstenite` and `quinn` to be **absent** on every leg,
//! h3 included. Each of the three linked crates crossed that line in the commit
//! that linked it, which is the assertion being moved deliberately rather than
//! the growth being discovered in a binary six months later, and the slice that
//! links one of the other three moves it again the same way.
//!
//! It also greps for `aws_lc`, which was never a dependency: it is the *other*
//! provider `rustls` ships, and `quinn`'s own default features ask for it. They
//! are turned off in `manifest.toml` and `rustls-ring` is turned on instead, so
//! an h3 archive has one cryptography implementation in it rather than two —
//! and that grep is what says so when quinn's defaults next change.
//!
//! ## What the entries below are for
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
//!
//! ## And one thing below is not a door at all
//!
//! [`serves`] is a plain Rust function with no `no_mangle` on it, because its
//! caller is inside this crate: F2's `serve` walks `Server`'s `protocols` field
//! and its loop body is `net::serves(protocol)?`. That is the whole of C4's
//! handoff — the feature, the file beside the archive, the capability bit, the
//! door and the sentence all land here, and the slice that adds a server spends
//! them in one line.
//!
//! It answers a `Result` rather than aborting, and the reason is `lib.rs` §5's
//! division: a toolchain built without a feature is a *configuration* a program
//! can report, retry or fall back from, while an abort is for an invariant that
//! is already broken. Asking a runtime with no QUIC in it for HTTP/3 is the
//! first of those.

/// The reactor, the timer wheel and the carrier pool — `tokio`. The one bit
/// whose crate is genuinely linked: `rt.rs` is what reaches it.
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
/// QUIC, and therefore HTTP/3 — `quinn`. **The only bit behind a feature that
/// is off by default**, which is why it is the only one a program has a reason
/// to ask about before it asks for anything: see [`buri_rt_net_h3_available`].
pub const BURI_NET_H3: i64 = 1 << 4;

/// The bits this toolchain's runtime was built with.
///
/// A `const` rather than a function body so that the answer is folded at the
/// one place it is asked, and so that the `net`-off build has no branch and no
/// data for the six crates at all.
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
    // the same reason the other five are.
    let _provider = size_of::<ring::digest::Context>();
    let _websocket = size_of::<tungstenite::protocol::Role>();
    BURI_NET_TOKIO | BURI_NET_HYPER | BURI_NET_TLS | BURI_NET_WEBSOCKET | H3
};

#[cfg(not(feature = "net"))]
const LINKED: i64 = 0;

/// The `net-h3` half of [`LINKED`], separated because it is the one feature a
/// user has to ask for.
///
/// `net-h3` implies `net` in `manifest.toml`, so this is only ever folded into
/// a mask that already has the other four bits — but it is written as its own
/// constant rather than as a `cfg` on the expression above, because "which
/// crate does this bit stand for" is a question each of the five answers the
/// same way and it should keep answering it the same way when there are six.
#[cfg(feature = "net-h3")]
const H3: i64 = {
    let _quic = size_of::<quinn::Endpoint>();
    BURI_NET_H3
};

#[cfg(all(feature = "net", not(feature = "net-h3")))]
const H3: i64 = 0;

/// Which halves of the networking stack this toolchain's runtime was built
/// with, as the bitmask above.
///
/// Bits rather than one flag because they were four crates behind one feature
/// and are now six behind two: `net-h3` is the second, it is **off by
/// default**, and a caller asking "can this binary speak HTTP/3" must not have
/// to ask "was the toolchain built with the network".
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

/// Whether this toolchain's runtime can speak HTTP/3.
///
/// The third door in this file and the only one whose answer is `0` on an
/// ordinary toolchain: `net` is on by default and `net-h3` is not, because
/// `quinn` is a QUIC implementation and the note's gate was "behind
/// configuration until the crate is trusted". `manifest.toml`'s feature block
/// is where that is argued.
///
/// It is a door of its own rather than a bit a caller masks out of
/// [`buri_rt_net_capabilities`] because it is the one a *program* has a reason
/// to ask: everything else in the mask is on whenever the network is, so
/// asking is only worth a call for the half that might not be.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_net_h3_available() -> i32 {
    i32::from(LINKED & BURI_NET_H3 != 0)
}

/// The application protocols a server can be asked to speak — `Server`'s
/// `protocols` field, one variant at a time.
///
/// The discriminants are the **variant indices** of the Buri enum F2 declares,
/// because that is what a generated program passes across the C ABI: the same
/// convention `Method`'s `.Get` already travels under in `http.rs`, where the
/// wire spelling is the runtime's and never the caller's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Protocol {
    /// `.Http1` — what `hyper` speaks, and what a `net` runtime always can.
    Http1 = 0,
    /// `.Http2` — the same connection, the same crate, ALPN-negotiated.
    Http2 = 1,
    /// `.Http3` — QUIC, and the one that is not always there.
    Http3 = 2,
}

impl Protocol {
    /// The variant index back to the variant, and `None` for an index no
    /// variant has.
    ///
    /// The `None` case is not defensive: a program built by this toolchain
    /// cannot produce a fourth index, so it is what an *older* program linked
    /// against a *newer* runtime — or the reverse — would produce, and the
    /// answer to that is a refusal rather than a transmute.
    pub fn from_index(index: i64) -> Option<Self> {
        match index {
            0 => Some(Self::Http1),
            1 => Some(Self::Http2),
            2 => Some(Self::Http3),
            _ => None,
        }
    }

    /// What the refusals call it, which is what a user reading one will
    /// recognise from the specification rather than from this enum.
    pub fn name(self) -> &'static str {
        match self {
            Self::Http1 => "HTTP/1.1",
            Self::Http2 => "HTTP/2",
            Self::Http3 => "HTTP/3",
        }
    }
}

/// The sentence a toolchain without `net-h3` owes a program that asked for
/// HTTP/3.
///
/// It names the feature, the flag that turns it on and the two protocols that
/// are there, because "unsupported" on its own is a failure the user cannot
/// act on — the same standard `tls.rs`'s refusals are held to.
pub const H3_UNSUPPORTED: &str = "HTTP/3 is not supported by this toolchain's native runtime: it \
                                  was built without the runtime's `net-h3` feature, so it \
                                  carries no QUIC code. Rebuild the toolchain with \
                                  `BURI_RUNTIME_NET_H3=1`, or serve HTTP/1.1 or HTTP/2";

/// Whether this runtime can serve a protocol — **the refusal, as a `Result`.**
///
/// This is the whole of what C4 built and F2 spends: `serve` walks `Server`'s
/// `protocols` field and the loop body is `net::serves(protocol)?`, so the
/// `.Err(Unsupported)` the design row asks for is one line at the call site and
/// a value everywhere else. It is a `Result` rather than an `abort` on purpose,
/// and the reason is the same one `NetError` exists for: asking for HTTP/3 on a
/// toolchain that has none is a *configuration* the program can report, retry
/// or fall back from, not a broken invariant — and `lib.rs` §5 reserves
/// aborting for the second.
///
/// **The `net` question is deliberately not asked here.** `host.HostListen.*`
/// is in `runtime_native::net_intrinsic`'s family, so a program that reaches
/// `serve` at all has already been refused before code generation on a
/// `net`-off toolchain (`networking-not-available`, C3). `net-h3` is *not* in
/// that family and must not be: refusing at compile time would refuse every
/// program that mentions `serve`, including every one that was only ever going
/// to ask for HTTP/1.1. That asymmetry is exactly the one `HostNet.fetch` and
/// `https://` already carry, argued in `runtime_native.rs` and `lib.rs` §8.
pub fn serves(protocol: Protocol) -> Result<(), &'static str> {
    match protocol {
        Protocol::Http1 | Protocol::Http2 => Ok(()),
        Protocol::Http3 if LINKED & BURI_NET_H3 != 0 => Ok(()),
        Protocol::Http3 => Err(H3_UNSUPPORTED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag round-trips: the feature the crate was compiled with, the bit
    /// in the mask, and the door a linked program asks through are one answer.
    ///
    /// `cfg!` rather than a constant of this module, so the assertion is
    /// against Cargo's own view of the build rather than against the same
    /// expression under test. The other half of the round trip — the feature
    /// list `cli/build.rs` writes beside the archive, and
    /// `runtime_native::h3()` reading it back — is asserted from the other side
    /// of the C ABI in `cli/tests/native/runtime.rs`, because the two paths are
    /// independent and their agreement is the property worth holding.
    #[test]
    fn the_h3_flag_round_trips() {
        let built = cfg!(feature = "net-h3");
        assert_eq!(buri_rt_net_h3_available() == 1, built);
        assert_eq!(buri_rt_net_capabilities() & BURI_NET_H3 != 0, built);
        // And h3 is a *half* of the network rather than an alternative to it:
        // `net-h3` implies `net` in the manifest, so the door cannot be open
        // while the stack as a whole is shut.
        assert!(!built || buri_rt_net_available() == 1);
        // The bit is its own, and it is above the four `net` ones.
        assert_eq!(BURI_NET_H3, 1 << 4);
        assert_eq!(
            BURI_NET_H3 & (BURI_NET_TOKIO | BURI_NET_HYPER | BURI_NET_TLS | BURI_NET_WEBSOCKET),
            0
        );
    }

    /// The variant index a generated program sends is the variant this file
    /// reads back, and a fourth index is refused rather than guessed at.
    #[test]
    fn a_protocol_index_round_trips() {
        for protocol in [Protocol::Http1, Protocol::Http2, Protocol::Http3] {
            assert_eq!(Protocol::from_index(protocol as i64), Some(protocol));
        }
        assert_eq!(Protocol::from_index(0), Some(Protocol::Http1));
        assert_eq!(Protocol::from_index(2), Some(Protocol::Http3));
        assert_eq!(Protocol::from_index(3), None);
        assert_eq!(Protocol::from_index(-1), None);
        assert_eq!(Protocol::Http3.name(), "HTTP/3");
    }

    /// **The refusal is a value.** The test runs to its end on a toolchain
    /// without `net-h3`, which is the assertion: an `abort` would take the
    /// process with it and there would be no `Err` to look at.
    #[test]
    fn asking_for_http3_answers_a_result() {
        assert_eq!(serves(Protocol::Http1), Ok(()));
        assert_eq!(serves(Protocol::Http2), Ok(()));
        if cfg!(feature = "net-h3") {
            assert_eq!(serves(Protocol::Http3), Ok(()));
        } else {
            let refused = serves(Protocol::Http3);
            assert!(refused.is_err(), "HTTP/3 was allowed by a runtime with no QUIC in it");
            let message = refused.unwrap_err();
            assert!(message.contains("net-h3"), "the refusal does not name the feature: {message}");
            assert!(
                message.contains("BURI_RUNTIME_NET_H3=1"),
                "the refusal does not say how to turn it on: {message}"
            );
        }
    }
}
