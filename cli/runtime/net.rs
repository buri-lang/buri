//! The networking stack's seam, and **the acceptor**.
//!
//! Two things live here and they are related the way a question is related to
//! the answer it made possible. The first half is the seam: which crates the
//! `net` and `net-h3` features bring in, which of them anything actually calls,
//! and the doors a linked program asks those questions through. The second half
//! is `effect Listen`'s implementation — the HTTP/1.1 server a Buri program
//! reaches through `core/net/server`.
//!
//! ## The crates, and what does or does not call them
//!
//! `manifest.toml`'s `net` feature brings `tokio`, `hyper`, `rustls`, `ring`
//! and `tungstenite` into the runtime's dependency tree, and its `net-h3`
//! feature brings `quinn`. This file names one type from each, which for two of
//! them — `tungstenite` and `quinn` — is still the *whole* of what references
//! them: no intrinsic key mangles to a symbol declared here
//! (`runtime_native::symbol_for` is the rule, and
//! `backend/runtime_table.rs` is the table); neither backend emits a call into
//! them; nothing in `core/` reaches them.
//!
//! **`hyper` left that list in F4, and this file is what took it off.** The
//! decision was taken three times and kept twice. F2 wrote the argument for
//! framing HTTP/1.1 by hand — `http.rs`'s reason read from the other side of
//! the wire, that a synchronous exchange over one connection is the whole of
//! what was being asked for, and that reaching a framing layer through `hyper`
//! would mean standing up a reactor per connection to get at something two
//! hundred lines already do. F3 re-took it when handlers moved onto tasks and
//! came out the same way: a worker per handler is a fan-out over the same
//! accept, the same head parser and the same response writer, none of which
//! held state between calls.
//!
//! What F2 and F3 both deferred by name was "multiplexing two requests over one
//! connection, and negotiating which protocol that is". That is this slice, and
//! it is where the argument runs out. HPACK is a compression context shared by
//! every stream on a connection, flow control is a windowed credit scheme in two
//! directions at two levels, and several requests being in flight on one socket
//! is the entire point of the protocol — none of which is framing this file
//! should write. So the acceptor keeps its hand-written HTTP/1.1 and hands an
//! ALPN-negotiated `h2` connection to `hyper`, which is the crate doing the one
//! job the dependency bar admitted it for.
//!
//! The crates landed a slice ahead of any code that uses them so that the price
//! could be measured *before* anything depended on the answer. Four of them
//! have since been linked, each in the commit that needed it and each with its
//! bytes written down: `tokio` by `rt.rs` (the reactor and its timer wheel,
//! +185 424 bytes when B6 measured it), `rustls` and `ring` by `tls.rs`, which
//! `http.rs` calls for every `https://` URL (+1 804 592 when C7 did), and
//! `hyper` by the HTTP/2 half below (+771 688 for the server, `h2`, `tokio-util`
//! and HPACK's static table). Where that leaves the archive, measured on
//! aarch64-apple-darwin on this tree:
//!
//! ```text
//! libburi_rt.a
//!   net off                                        6 304 448 bytes
//!   net on, the reactor, TLS and HTTP/2 linked     8 970 592   +2 666 144
//!   net-h3 on as well                              8 971 008        +416
//! ```
//!
//! All three re-measured on this tree, because the `net`-off figure had not
//! been since C7 and had moved on its own: this file's acceptor is compiled
//! into a `net`-off archive too, and three slices of it landed in between.
//!
//! The 9 MiB Darwin budget `the_runtime_archive_is_real` carries is **not** moved
//! by this slice, and that is the ratchet working rather than the ratchet being
//! ignored: 8 970 592 is under it by about 4.9 %, and a budget that moves when
//! it has not been hit is a budget. What it leaves is a thin margin, and the
//! slice that links `tungstenite` should expect to be the one that re-measures
//! it.
//!
//! **Linux is a bigger machine and its budget did move.** Every Linux figure
//! that script carried was a projection, and the first real measurement — taken
//! in the Linux container BUILD-AND-WATCH.md §3.3.1
//! describes, on
//! aarch64-unknown-linux-gnu — showed the projection had been wrong by 1.9 MB:
//!
//! ```text
//! libburi_rt.a, aarch64-unknown-linux-gnu, net on
//!   before this slice                             12 407 786 bytes
//!   after it                                      13 611 228   +1 203 442
//! ```
//!
//! So Linux had been at 98.6 % of its 12 MiB budget before HTTP/2 was linked at
//! all, and hyper's bytes are what took it over. The growth is the same code at
//! ELF's price: Linux is 1.51x this archive before the slice and hyper costs
//! 1.56x more there, which is a ratio and not a leak. 14 MiB is the
//! re-measurement, and the script's own comment is where that is argued.
//!
//! **`quinn` still costs the archive nothing**, which is the argument for
//! landing a crate a slice ahead of its caller all over again: turning `net-h3`
//! on changes the figure above by tens of bytes and not by a QUIC
//! implementation, because the whole crate is dropped by `lto = "fat"` while
//! nothing calls it. The price of HTTP/3 is a number the slice that drives it
//! will produce and not one this slice had to guess at.
//!
//! Twenty-four bytes was what four unreferenced crates cost, because
//! `lto = "fat"` is whole-program across the dependency rlibs and Rust code
//! that nothing reaches does not reach the archive. What LTO cannot touch is a
//! dependency's **native** object code: about 845 KB of the TLS figure is
//! `ring`'s C and AArch64 assembly, which a `staticlib` bundles whether the
//! linker needs it or not.
//!
//! `cli/tests/ci.rs::the_runtime_archive_is_real` holds the remaining claim in CI
//! the direct way — it greps the archive's symbol table, requires `tokio`,
//! `rustls`, `ring` and `hyper` to be **present** when the feature file says
//! `net`, and requires `tungstenite` and `quinn` to be **absent** on every leg,
//! h3 included. Each of the four linked crates crossed that line in the commit
//! that linked it, which is the assertion being moved deliberately rather than
//! the growth being discovered in a binary six months later, and the slice that
//! links one of the other two moves it again the same way.
//!
//! **A crate's name in a symbol is not the same as its code in the archive**,
//! and linking `hyper` is what made that difference visible. Rust's v0 mangling
//! ends a monomorphised generic with the crate that *instantiated* it, so
//! `alloc::raw_vec`'s `grow_one` over an `http::HeaderMap` bucket, kept from
//! tungstenite's codegen unit when LLVM deduplicated the copies, reads as a
//! symbol with `tungstenite` in its name and no tungstenite code behind it.
//! Which copy survives is arbitrary and differs by platform: three such symbols
//! appear on Linux and none on macOS. The script tells the two apart by
//! position — an instantiation tag is the last thing on its line and a path
//! never is — and it says so where it does it.
//!
//! It also greps for `aws_lc`, which was never a dependency: it is the *other*
//! provider `rustls` ships, and `quinn`'s own default features ask for it. They
//! are turned off in `manifest.toml` and `rustls-ring` is turned on instead, so
//! an h3 archive has one cryptography implementation in it rather than two —
//! and that grep is what says so when quinn's defaults next change.
//!
//! ## What the three doors are for
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
//! ## And one thing is not a door at all
//!
//! [`serves`] is a plain Rust function with no `no_mangle` on it, because its
//! caller is inside this crate: [`bind`] walks the protocol list a `Server`'s
//! `protocols` field became, and the loop body is `serves(protocol)`. That is
//! the whole of what the HTTP/3 slice built and this one spends — the feature,
//! the file beside the archive, the capability bit, the door and the sentence.
//!
//! It answers a `Result` rather than aborting, and the reason is `lib.rs` §5's
//! division: a toolchain built without a feature is a *configuration* a program
//! can report, retry or fall back from, while an abort is for an invariant that
//! is already broken. Asking a runtime with no QUIC in it for HTTP/3 is the
//! first of those. [`accepts`] asks the narrower question beside it — does the
//! acceptor in this file *drive* that protocol — and the two are separate
//! because a crate being linked and a server being written against it are two
//! different facts.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::value::{BuriList, BuriStr, list_of_bytes, list_of_headers, str_of};

/// The reactor, the timer wheel and the carrier pool — `tokio`. The one bit
/// whose crate is genuinely linked: `rt.rs` is what reaches it.
pub const BURI_NET_TOKIO: i64 = 1 << 0;
/// HTTP/2 framing — `hyper`. Like [`BURI_NET_TLS`] and unlike its two remaining
/// neighbours, this bit now means a working capability rather than a linked
/// crate: the acceptor below hands it every connection ALPN settled on `h2`.
/// HTTP/1.1 is framed in this file and needs no crate at all.
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

/// Whether **this acceptor** speaks a protocol the toolchain carries the code
/// for, which is a second and narrower question than [`serves`].
///
/// The two are different because a crate being linked and a server being
/// written against it are different facts. [`serves`] answers "does this
/// archive carry the code" — the `net-h3` feature, the QUIC stack — and is the
/// one C4 built. This answers "does the acceptor below drive it", and the
/// acceptor now drives two: HTTP/1.1, which it frames itself, and HTTP/2, which
/// is `hyper`'s over a TLS connection ALPN chose.
///
/// Both are asked, in that order, so that a program asking for HTTP/3 on a
/// toolchain without `net-h3` gets the message that names the build switch
/// rather than this one — a user can act on the first and can only wait for the
/// second.
fn accepts(protocol: Protocol) -> Result<(), String> {
    serves(protocol).map_err(str::to_string)?;
    match protocol {
        Protocol::Http1 => Ok(()),
        Protocol::Http2 if cfg!(feature = "net") => Ok(()),
        // A `net`-off toolchain has no `hyper` in it, and a program that got
        // this far on one is a program `runtime_native::net_intrinsic` has
        // already refused — so this arm is what an out-of-tree caller of the C
        // ABI is told rather than something a Buri program can reach.
        Protocol::Http2 => Err(String::from(
            "HTTP/2 is not spoken by this toolchain's native runtime: it was built without the \
             runtime's `net` feature, so it carries no HTTP/2 framing. Serve HTTP/1.1",
        )),
        Protocol::Http3 => Err(format!(
            "{} is not spoken by this runtime's acceptor: it frames HTTP/1.1 and negotiates \
             HTTP/2 over TLS. Serve one of those, or leave `protocols` unset",
            protocol.name()
        )),
    }
}

/// The sentence a server asking for HTTP/2 without a certificate is owed.
///
/// **h2 is negotiated and never assumed.** RFC 7540 §3.3 puts the choice in the
/// TLS handshake's ALPN extension, so a cleartext listener has nowhere to make
/// it: the prior-knowledge alternative (§3.4) asks every client to *guess* that
/// this port speaks HTTP/2, which is a thing to opt a whole port into and not a
/// thing to do to a program that asked for two protocols. So `protocols`
/// naming `.Http2` without a `tls` is refused at the bind rather than silently
/// answered in HTTP/1.1, because a server that quietly did not do what it was
/// configured to do is the failure that is found in production.
pub const H2_NEEDS_TLS: &str =
    "HTTP/2 is negotiated by ALPN inside the TLS handshake (RFC 7540 §3.3), so a server that asks \
     for `.Http2` needs a certificate: set `Server`'s `tls` field, or ask for `.Http1` alone";

// ---------------------------------------------------------------------------
// The acceptor
// ---------------------------------------------------------------------------
//
// `effect Listen` is four operations — bind, accept, respond, close — and the
// loop over the middle two is `core/net/server`'s, in Buri. What lives here is
// therefore not a server framework but the three steps a loop needs, and that
// division is the reason this file can be as small as it is: the runtime never
// holds a Buri value between calls, never calls back into generated code, and
// never has to know what a handler is.
//
// **HTTP/1.1 is framed here; HTTP/2 is `hyper`'s.** That split is F4's whole
// shape and it is the one the file was always going to reach. `http.rs`'s
// argument held for as long as a connection carried one message: a request
// line, some fields and a body is two hundred lines, and reaching a framing
// layer through `hyper` to get at them would have been a reactor bought for
// nothing. What HTTP/2 asks for is a different thing — HPACK, per-stream flow
// control, a settings negotiation and several requests interleaved on one
// socket — and none of that is framing a file can keep doing by hand. So this
// is the commit that links `hyper`, and it links it for exactly the half that
// is `hyper`'s: [`h2`] below is a `hyper::server::conn::http2` connection, and
// everything above it is the same hand-written HTTP/1.1 it was.
//
// **One request per HTTP/1.1 connection**, still: the response carries
// `connection: close` and the socket is dropped after it. HTTP/2 is where
// several messages share a connection, and it reaches that by multiplexing
// rather than by keep-alive.
//
// **TLS, and ALPN as the thing that chooses.** A listener with a certificate
// hands each accepted socket to `tls.rs` for the handshake, and what comes back
// carries the protocol the two ends agreed on: `h2` goes to `hyper`,
// `http/1.1` and no answer at all go to the framing below. A listener without
// one is cleartext HTTP/1.1 exactly as it was — the same accept, the same
// parser, the same writer — because the transport is a [`Wire`] and the framing
// never asks which kind it has.
//
// **A worker per handler, and a connection identifier because of it.** The loop
// is still `core/net/server`'s and still in Buri; what F3 changed is that there
// are several of it, fanned out by `Tasks.parallel` onto carriers of their own,
// all taking connections from the one listener. So "the request the accept last
// handed out" stopped naming exactly one connection: [`accept`] answers a
// connection id beside the request and [`respond`] takes that id instead of the
// listener's. Everything else about the division survived — the runtime still
// holds no Buri value between calls, still calls back into no generated code,
// and still does not know what a handler is.
//
// **The socket is accepted by a thread of the listener's own, and the workers
// take from a queue.** That is F4's second structural change and multiplexing
// is what forced it: an HTTP/2 connection produces requests whenever its client
// sends them, so a request can become ready while every worker is asleep inside
// `accept(2)` — and there is no portable way to make a blocking accept notice
// something that did not arrive on its socket. One acceptor thread per listener
// takes the connections, a short-lived thread per connection frames the first
// request off it, and [`Gate::ready`] is the one queue both protocols push
// on to. What it bought beside HTTP/2 is that a slow handshake or a slow client
// no longer occupies a *handler*: `MAX_HANDLERS` workers are now spent
// answering requests rather than waiting for them.
//
// **Every wait is bounded except the one a server is for.** Reads and writes
// carry `SO_RCVTIMEO`/`SO_SNDTIMEO` from `headerTimeoutMillis`; the wait for a
// connection carries `idleTimeoutMillis` when the caller set one. A caller that
// sets neither gets a server that waits for a client indefinitely and reads
// with a thirty-second deadline, which is what a server is; a *test* sets both,
// and every test in this repository does.
//
// **A socket option is not always the whole bound, and three places say so.**
// `SO_RCVTIMEO` bounds one `read(2)` and is restarted by the next, so a loop of
// reads is bounded in bytes and not in time — [`read_request`] therefore
// measures [`HEAD_DEADLINE`] end to end as well as putting it on the socket,
// which is what refuses a slowloris. The other two waits that no socket option
// reaches are HTTP/2's: [`answered_within`] bounds a stream waiting on a
// handler ([`ANSWER_DEADLINE`]), and [`h2`]'s connection task bounds itself
// once its GOAWAY has gone out, because a GOAWAY is a request and a client may
// decline it. The two named exemptions are unchanged and are the two a server
// is *for*: `listenAccept` and `listenReceive`.

/// How long one connection may take to send its request line, its headers and
/// its body, when the caller named no deadline. [`crate::http::DEADLINE`]'s
/// argument, from the other side of the wire.
///
/// **Both a per-read deadline and a budget across them**, which is two
/// mechanisms for one number because one of them cannot do the job alone.
/// [`deadlines`] puts it on the socket, where `SO_RCVTIMEO` bounds each
/// `read(2)`; [`read_request`] also measures it end to end, because a per-read
/// deadline the kernel restarts on every read is not a bound on the request at
/// all — a client sending a byte just inside it holds the connection for ever
/// without ever being late. The socket option is what stops one read hanging
/// and the budget is what stops the *sum* of them, and a server needs both.
const HEAD_DEADLINE: Duration = Duration::from_secs(30);

/// The largest request body read when the caller named no limit. A request
/// declaring more is answered `413` by the acceptor and never reaches a
/// handler: a limit that a handler had to enforce would be a limit every
/// handler had to remember.
const BODY_LIMIT: usize = 8 * 1024 * 1024;

/// How long a drain waits for the requests already in flight, when the plan
/// named no number of its own.
///
/// **What it bounds is the waiting and not the handlers.** A drain stops the
/// listener accepting, waits for every connection it has already taken to be
/// answered, and only then tells the workers there will be no more; a handler
/// that is still running when this expires still writes its response, because
/// the connection is still in [`CONNECTIONS`] and `listenClose` is still the
/// last thing `run` does. What expiry changes is that the workers waiting for a
/// *new* request stop waiting.
///
/// Ten seconds rather than thirty, which is the number the head deadline
/// carries. The two are answers to different questions: thirty seconds is how
/// long one client may take to say what it wants, and ten is how long a server
/// that has been asked to stop should keep a deployment waiting for a client
/// that is not going to finish. A program that knows its own handlers take
/// longer says so — `Server.drainMillis` is that sentence, and it is one
/// [`ServePlan`] variant rather than one more argument.
const DRAIN_DEADLINE: Duration = Duration::from_secs(10);

/// How long an HTTP/2 stream waits for the handler that took it.
///
/// **The one wait in this file whose other end is a Buri program**, which is
/// why it needs a number of its own. Everything else that waits here waits on
/// the network or on this file's own threads; an h2 stream waits on a handler,
/// and a handler that never calls `listenRespond` is a task, a stream and a
/// place under [`MAX_STREAMS`] held for the life of the process.
///
/// **HTTP/1.1 has no equivalent and needs none**, which is the asymmetry worth
/// naming rather than smoothing over. There the answer is bytes written on a
/// socket the connection owns, so a handler that never answers holds a socket
/// and nothing waits on it; here the answer is a value handed back to a task
/// that is parked until it arrives. The wait is the difference, and only a wait
/// needs a bound.
///
/// Thirty seconds, [`HEAD_DEADLINE`]'s number read the other way round: that is
/// how long a client may take to say what it wants, and this is how long the
/// server may take to say what it has. A `504` is what expiry answers, because
/// that is the status for a server that did not get its answer in time — and it
/// reaches the client, which is the whole difference between this and a stream
/// nobody ever hears about again. The handler is not cancelled and cannot be:
/// it is Buri code on a carrier, its `listenRespond` will find the connection
/// gone and answer `.Closed`, and that is the same thing a `listenClose` under
/// a running handler already does.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30);

/// How many messages one socket's outbound buffer holds when the plan named no
/// number of its own.
///
/// **A socket that fills it is closed**, which is the one answer that costs
/// neither of the two things the alternatives cost. A buffer that grew without
/// a bound would be a server whose memory is decided by its slowest client — a
/// client that stops reading is not an error condition, it is a laptop closing
/// — and a `send` that waited for room would be an actor stalled by a socket it
/// does not own, which is the whole reason `socketSendText` promises not to
/// wait.
///
/// Sixty-four rather than a number of bytes, because a message is what a
/// program enqueues and a byte count would bound a thousand small messages and
/// one large one the same way. A program that knows its own messages says so:
/// `Server.socketBuffer` is that sentence, and it is one [`ServePlan`] variant
/// rather than one more argument — which is exactly what F4 built the plan
/// for and named this as the second caller of.
const SOCKET_BUFFER: usize = 64;

/// How many accepted connections may be in flight — handshaking, being framed,
/// or waiting for a worker — before the acceptor thread stops taking more.
///
/// The queue exists because the acceptor no longer runs at the pace of the
/// workers, and a queue with no bound is a server whose memory is decided by
/// its clients. When it is full the acceptor thread simply stops accepting, so
/// the pressure lands in the kernel's own listen backlog and then on the
/// client's `connect` — which is where a server that is full should push back.
///
/// Twice `MAX_HANDLERS` and not equal to it, because a connection is in flight
/// from the moment it is accepted to the moment it is answered: one being
/// framed while another is being handled is the ordinary case and not
/// congestion.
const IN_FLIGHT: usize = 2 * MAX_HANDLERS as usize;

/// How many requests one HTTP/2 connection may have open at once.
///
/// The number `hyper` tells the client in the settings frame, and therefore the
/// bound on how many [`CONNECTIONS`] entries one socket can hold. It is
/// `MAX_HANDLERS` because that is how many can be *answered* at once: a client
/// allowed to open a thousand streams against sixty-four workers would be
/// buying itself a queue rather than any concurrency.
#[cfg(feature = "net")]
const MAX_STREAMS: u32 = MAX_HANDLERS as u32;

/// `ServeFailure`'s variants, in declaration order in `effect.buri`. The index
/// is what crosses the ABI, so this order is the contract — `NetFail`'s
/// comment, one file over, is the same sentence about the same rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeFail {
    AddressInUse,
    AddressNotAvailable,
    PermissionDenied,
    Unsupported,
    Timeout,
    Closed,
    Transport,
}

impl ServeFail {
    /// The variant index, which is what the `.Err` arm carries across.
    pub fn tag(&self) -> i8 {
        match self {
            ServeFail::AddressInUse => 0,
            ServeFail::AddressNotAvailable => 1,
            ServeFail::PermissionDenied => 2,
            ServeFail::Unsupported => 3,
            ServeFail::Timeout => 4,
            ServeFail::Closed => 5,
            ServeFail::Transport => 6,
        }
    }

    /// The failure an `io::Error` is, by kind. Anything the six named causes do
    /// not cover is `Transport` with the platform's own words, which is the
    /// same division `http.rs::io_fail` makes for the client.
    fn of(e: &std::io::Error) -> ServeFail {
        match e.kind() {
            std::io::ErrorKind::AddrInUse => ServeFail::AddressInUse,
            std::io::ErrorKind::AddrNotAvailable => ServeFail::AddressNotAvailable,
            std::io::ErrorKind::PermissionDenied => ServeFail::PermissionDenied,
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ServeFail::Timeout,
            _ => ServeFail::Transport,
        }
    }
}

/// A `ServeError` — the cause, and what the platform said.
#[derive(Debug)]
pub struct ServeErr {
    pub cause: ServeFail,
    pub detail: String,
}

impl ServeErr {
    fn new(cause: ServeFail, detail: impl Into<String>) -> ServeErr {
        ServeErr { cause, detail: detail.into() }
    }

    fn io(e: &std::io::Error, what: &str) -> ServeErr {
        ServeErr::new(ServeFail::of(e), format!("{what}: {e}"))
    }

    /// The listener will answer no more. Carries no detail: it is not a
    /// failure, and `core/net/server` turns it into `.Ok(())` without ever
    /// showing it to a program.
    fn closed() -> ServeErr {
        ServeErr::new(ServeFail::Closed, String::new())
    }
}

/// One request, as the acceptor read it off the wire.
#[derive(Debug)]
pub struct ServerRequest {
    /// A `Method` tag — the index into [`crate::http::METHODS`].
    pub method: i32,
    /// The request target, exactly as the client sent it: `/ping?x=1`. Not an
    /// absolute URL, because the client did not send one and reconstructing one
    /// would mean trusting `Host` to say where the request really went.
    /// `Request.path()` and `Request.query()` read it the same either way.
    pub target: String,
    /// Field names lowercased, as `Header` states they are.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Everything `listenBind` was asked for that outlives the bind, decoded.
///
/// The address and the port are not here: they are what the `TcpListener` was
/// built from and the socket is the one that remembers them, so a copy beside
/// it would be a second answer to a question `local_addr` already answers.
struct Plan {
    limit: Option<u64>,
    idle: Option<Duration>,
    head: Duration,
    body_limit: usize,
    /// How long [`drain`] waits for the connections this listener has already
    /// taken. The plan's own number where it named one, [`DRAIN_DEADLINE`]
    /// where it did not.
    ///
    /// **Two waiters, not one.** It bounds the `drained` call that `drain`
    /// itself makes, and — since the same number is the honest answer to "how
    /// long after a GOAWAY is a connection still this server's problem" — it
    /// also bounds [`h2`]'s connection task once that GOAWAY has gone out.
    drain: Duration,
    /// How long an HTTP/2 stream waits for its handler. [`ANSWER_DEADLINE`],
    /// with no `ServePlan` variant setting it yet — it is here rather than read
    /// from the constant at the wait so that the variant, when there is one, is
    /// a line in `bind` and nothing else.
    answer: Duration,
    /// How many messages one socket's outbound buffer holds before the socket
    /// is closed. The plan's own number where it named one,
    /// [`SOCKET_BUFFER`] where it did not.
    socket_buffer: usize,
}

/// The `[Serve]` list a `Server`'s protocol and TLS fields were rendered into,
/// decoded — **the bind's one extensible argument**.
///
/// F2 wrote down that `listenBind` was at the ten-integer budget a runtime call
/// has (`backend/stencil/abi.rs`'s `MAX_INT_ARGS`), and that the answer, when
/// one more knob had to cross, would be "an options struct behind one pointer
/// or a wider budget". A certificate and a key are two `Str`s and therefore six
/// integers, so F4 is where that bill came due — and the answer is neither of
/// the two that were costed, because a third one is free: the list argument
/// **already** crosses as a pointer and a length, and a runtime table describes
/// it as `Arg::List` whatever its elements are. So `protocols: [Protocol]`
/// became `plan: [Serve]`, a list of tagged options, and neither table has a
/// row that changed.
///
/// What that buys, beyond this slice: every knob that follows — F5's drain
/// deadline, F7's socket buffer bound, the `headerTimeoutMillis` and
/// `bodyLimitBytes` F2 could not carry — is a variant here rather than an
/// argument nobody has room for.
#[derive(Debug, Default)]
pub struct ServePlan {
    /// The protocols named, in the order they were named. Empty is "the caller
    /// chose nothing", which is HTTP/1.1.
    pub protocols: Vec<Protocol>,
    /// Where the server's certificate chain is read from, if it has one.
    pub certificate: Option<PathBuf>,
    /// Where its private key is read from.
    pub key: Option<PathBuf>,
    /// How long a shutdown waits for the requests already in flight. `None` is
    /// a caller who chose nothing, which is [`DRAIN_DEADLINE`].
    pub drain: Option<Duration>,
    /// How many messages one socket's outbound buffer holds. `None` is a
    /// caller who chose nothing, which is [`SOCKET_BUFFER`].
    pub socket_buffer: Option<usize>,
}

impl ServePlan {
    /// A plan that names protocols and no certificate — what a cleartext
    /// caller and most of the tests below want.
    pub fn speaking(protocols: &[Protocol]) -> ServePlan {
        ServePlan {
            protocols: protocols.to_vec(),
            certificate: None,
            key: None,
            drain: None,
            socket_buffer: None,
        }
    }

    /// The ALPN offer this plan makes, most-preferred first.
    ///
    /// **Only what was asked for.** `.None` protocols crosses as an empty list
    /// and means the caller chose nothing, which `core/net/server` documents as
    /// HTTP/1.1 — so an unconfigured TLS server offers `http/1.1` and does not
    /// quietly become an HTTP/2 server because it grew a certificate. Asking
    /// for h2 is a thing a program does on purpose.
    #[cfg(feature = "net")]
    fn alpn(&self) -> Vec<Vec<u8>> {
        let mut offer: Vec<Vec<u8>> = Vec::new();
        for protocol in &self.protocols {
            let name = match protocol {
                Protocol::Http2 => crate::tls::ALPN_H2,
                Protocol::Http1 => crate::tls::ALPN_HTTP1,
                Protocol::Http3 => continue,
            };
            if !offer.iter().any(|had| had == name) {
                offer.push(name.to_vec());
            }
        }
        if offer.is_empty() {
            offer.push(crate::tls::ALPN_HTTP1.to_vec());
        }
        offer
    }
}

/// How many handlers one listener hosts at once — the number `bind` answers on
/// `Listener.handlers`, and the number `core/net/server`'s `run` fans out to.
///
/// **A worker waiting for a connection holds a carrier.** The accept below is a
/// blocking call, and `rt.rs`'s `park_on` can only give a carrier back to a
/// future that answers `Pending` — so a worker between requests costs an OS
/// thread rather than a mapping. `rt::MAX_CARRIERS` is 256 and is what a whole
/// *program* may have blocked at once, so a server may not be allowed to spend
/// all of it: sixty-four leaves a program its own `parallel` and leaves the pool
/// room to run the work the handlers themselves hand on.
///
/// **A constant and not a function of the processor count**, which is a
/// deliberate choice about a number a program cannot ask for. A handler waits far
/// more than it computes, so the useful number was never the core count; and a
/// number nobody can pin should at least be one everybody can predict, including
/// a test asserting that fifty requests overlap. The day `listenBind`'s
/// ten-integer budget grows, a program says what it wants and this becomes the
/// ceiling rather than the answer.
///
/// F3 forecast that F4 would move it, on the reasoning that an asynchronous
/// acceptor lets a waiting worker park instead of block. **It did not move, and
/// the forecast was half right.** What became asynchronous is the *accepting* —
/// the socket, the handshake and the framing all left the workers — but a worker
/// still waits on a condition variable for the next ready request, and a
/// condition variable is not something `park_on` can hand a carrier back
/// through. What changed is what the sixty-four are spent on: they were the
/// bound on how many connections could be *in progress* and they are now the
/// bound on how many can be *answered at once*, with [`IN_FLIGHT`] carrying the
/// first question. Raising it is a benchmark rather than an edit, which is why
/// it is not in this slice.
const MAX_HANDLERS: i64 = 64;

/// How long [`Listening::wake`] will wait for its own dial.
///
/// A bound rather than a timeout anything depends on: the connection is to a
/// port this process is holding open, on loopback, so it either succeeds at once
/// or the listener is already gone and there was nothing to wake.
const WAKE_DEADLINE: Duration = Duration::from_secs(1);

/// The listener's own state, and **the whole of what its threads share.**
///
/// Four things under one lock, because they have to be answered together: the
/// connection that takes the last request has to mark the listener finished and
/// wake every worker in the same step. Split them and a worker that read
/// `finished` before the mark and went to sleep after the wake waits for a
/// request nobody is going to make — the lost wakeup this lock rules out.
struct Gate {
    /// The listener will answer no more: its request limit is spent, or it has
    /// been closed. Once true, never false again.
    finished: bool,
    /// The listener takes no more *connections*, and the requests it already
    /// took are being answered. Once true, never false again.
    ///
    /// **Two flags and not one, because a drain is the half-open state a
    /// shutdown is made of.** `finished` answers `listenAccept` with `.Closed`,
    /// which ends every worker; this one ends the acceptor thread and leaves
    /// the workers exactly as they were, so a request that has been framed is
    /// still handed out and a request that has been handed out is still
    /// answered. [`drain`] is what turns the first into the second, and the
    /// waiting in between is the whole of what "graceful" means here.
    draining: bool,
    /// Requests handed out, which is what `requestLimit` counts.
    ///
    /// **Counted when a request joins [`Gate::ready`]**, which is where "handed
    /// out" now happens. A limit checked at the *response* would let every
    /// worker take a request before any of them answered one, and a server told
    /// to serve one request would serve as many as it had handlers.
    served: u64,
    /// Connections accepted and not yet answered — the number [`IN_FLIGHT`]
    /// bounds. An HTTP/2 connection counts once however many streams it opens,
    /// because it is one socket and one `hyper` task; [`MAX_STREAMS`] is what
    /// bounds the streams on it.
    outstanding: usize,
    /// The connections whose request has been framed and is waiting for a
    /// worker. **The one queue both protocols push on to**, which is what makes
    /// [`accept`] the same three lines whether the request arrived on its own
    /// socket or as the second stream of an h2 connection.
    ready: VecDeque<i64>,
    /// The acceptor thread's own failure, waiting for a worker to ask.
    ///
    /// It exists because the accept moved off the workers and onto a thread
    /// with nobody to report to. `serve` used to answer an `.Err` when
    /// `accept(2)` failed, and it still does — the thread leaves the error
    /// here, and the first worker to ask takes it. **Taken and not copied**:
    /// `ServeErr` carries the platform's own words and there is one failure, so
    /// one worker reports it and the rest read the ordinary `.Closed`.
    failure: Option<ServeErr>,
}

/// An open port, and everything the threads on it share.
struct Listening {
    listener: TcpListener,
    plan: Plan,
    gate: Mutex<Gate>,
    /// A request joined [`Gate::ready`], or the listener finished. What the
    /// workers sleep on, and what replaced a blocking `accept(2)` as the thing
    /// they wait in.
    arrived: Condvar,
    /// A connection was answered, so there is room under [`IN_FLIGHT`] for
    /// another. What the acceptor thread sleeps on when the server is full.
    room: Condvar,
    /// Where [`Listening::wake`] dials to interrupt the acceptor thread's
    /// blocking accept: the bound address, with an unspecified host replaced by
    /// loopback, because `0.0.0.0` is an address to accept on and not one to
    /// connect to.
    wake_at: SocketAddr,
    /// The server's identity, if it has one. `None` is a cleartext listener,
    /// and the whole of the difference is [`serve_connection`]'s first branch.
    #[cfg(feature = "net")]
    tls: Option<Arc<rustls::ServerConfig>>,
    /// The drain has begun — told to the one part of this file that cannot be
    /// told with a condition variable.
    ///
    /// Every other thread on a listener is a blocking one and waits on
    /// [`Listening::arrived`] or [`Listening::room`]. An HTTP/2 connection is
    /// not a thread: it is a task on the reactor, and what it has to be told is
    /// "send your GOAWAY and finish the streams you have". A `Notify` is that
    /// condition variable's asynchronous twin, and [`h2`] is its one waiter.
    #[cfg(feature = "net")]
    stopping: tokio::sync::Notify,
}

impl Listening {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        match self.gate.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Mark the listener finished and wake everyone waiting on it, in one step,
    /// for [`Gate`]'s reason.
    fn finish(&self) {
        self.gate().finished = true;
        self.arrived.notify_all();
        self.room.notify_all();
    }

    /// Stop taking connections, and tell everything that is waiting.
    ///
    /// The first half of a drain, and the only half that has to happen at
    /// once: after this the acceptor thread is on its way out, an HTTP/2
    /// connection has been asked to send its GOAWAY, and every request already
    /// framed is still on [`Gate::ready`] waiting for a worker.
    fn begin_drain(&self) {
        let mut gate = self.gate();
        if gate.draining {
            return;
        }
        gate.draining = true;
        drop(gate);
        // The acceptor thread may be waiting for room under `IN_FLIGHT` rather
        // than inside the accept, and a drain that only dialled would leave it
        // there until a response it is no longer going to see.
        self.room.notify_all();
        self.arrived.notify_all();
        #[cfg(feature = "net")]
        self.stopping.notify_waiters();
        // And if it *is* inside the accept, this is what brings it out.
        self.wake();
    }

    /// Wait until every connection this listener took has been answered, or
    /// until `until`. `true` if it drained, `false` if the deadline came first.
    ///
    /// **[`Gate::outstanding`] reaching zero is the whole condition, and it
    /// covers the queue too.** A connection is counted from the moment it is
    /// accepted to the moment it is answered, and an HTTP/1.1 request waiting
    /// on [`Gate::ready`] for a worker is a connection in that interval; an
    /// HTTP/2 stream is not counted itself, but the socket it arrived on is,
    /// for as long as `hyper` is driving it. So there is no state in which the
    /// queue holds something and this number is zero.
    ///
    /// It also cannot reach zero while the acceptor thread is alive:
    /// [`accepting_on`] reserves its place *before* it accepts, which is what
    /// makes "the acceptor has stopped" part of what this waits for rather than
    /// something to check separately.
    fn drained(&self, until: Instant) -> bool {
        let mut gate = self.gate();
        while gate.outstanding > 0 {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            gate = match self.room.wait_timeout(gate, left) {
                Ok((g, _)) => g,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        true
    }

    /// Interrupt the acceptor thread's blocking accept, by connecting to the
    /// port it is blocked on.
    ///
    /// **A dial rather than a shutdown**, because there is no portable way to
    /// make a blocking `accept(2)` return: closing the descriptor under a thread
    /// that is inside the call is a use-after-free waiting for the number to be
    /// reused, and the standard library gives a `TcpListener` no `shutdown`.
    ///
    /// **One dial and not one per worker**, which is what the acceptor thread
    /// bought: there is exactly one thread inside an accept on any listener, so
    /// a shutdown costs one loopback connection however many handlers the
    /// server has. The thread that takes it reads `finished`, gives the port
    /// back, and the socket is dropped unanswered.
    fn wake(&self) {
        let _ = TcpStream::connect_timeout(&self.wake_at, WAKE_DEADLINE);
    }

    /// Claim one request against `requestLimit` and put it on the queue, in one
    /// step — or answer `false` because the listener is done.
    ///
    /// **One step, and that is load-bearing rather than tidy.** The claim, the
    /// limit's one-way door and the queue push all have to happen under the
    /// same lock: a claim that marked the listener finished and then queued
    /// would leave a window in which every worker is woken, reads `finished`
    /// with an empty queue, and answers `.Closed` — and the request the last
    /// claim was *for* would be handed to nobody. So the connection is minted
    /// inside the lock too, and a worker's first act is to look in the queue
    /// before it looks at `finished`.
    ///
    /// The connection is handed back on the refusal path, because the caller is
    /// the only one that knows how to say `503` on it — **boxed**, because a
    /// `Pending` is the widest thing in this file and an `Err` that carried one
    /// by value would make every call of this as wide as its rarest answer.
    /// The `Ok` is an `i64`. Registering it takes the
    /// connection table's lock while this holds the gate's, which is the only
    /// place the two are nested and is always in this order.
    fn hand_over(&self, pending: Pending) -> Result<i64, Box<Pending>> {
        let mut gate = self.gate();
        if gate.finished {
            return Err(Box::new(pending));
        }
        gate.served = gate.served.saturating_add(1);
        if self.plan.limit.is_some_and(|n| gate.served >= n) {
            // Set, never cleared: `finished` is a one-way door, and a claim
            // that does not spend the limit must not reopen it.
            gate.finished = true;
        }
        let connection = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
        connections(|table| table.insert(connection, pending));
        gate.ready.push_back(connection);
        drop(gate);
        // All of them and not one: the request goes to whichever asks first,
        // and the workers that find `finished` behind it are the ones this
        // wakes to tell.
        self.arrived.notify_all();
        self.room.notify_all();
        Ok(connection)
    }

    /// One more connection in flight, or `false` if the listener is done.
    ///
    /// A drain stops it as a close does, which is what ends the acceptor thread
    /// while the workers carry on: the difference between the two states is
    /// what [`Gate::draining`] is for.
    fn take_flight(&self) -> bool {
        let mut gate = self.gate();
        while !gate.finished && !gate.draining && gate.outstanding >= IN_FLIGHT {
            gate = match self.room.wait(gate) {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        if gate.finished || gate.draining {
            return false;
        }
        gate.outstanding = gate.outstanding.saturating_add(1);
        true
    }

    /// One fewer, and the acceptor thread may take another.
    ///
    /// **All of them and not one**, because there are now two kinds of waiter
    /// on [`Listening::room`]: the acceptor thread waiting for a place under
    /// `IN_FLIGHT`, and a drain waiting for the last one to be given back. At
    /// most one of each exists on any listener, so this is two wakeups on a
    /// response rather than one, and a `notify_one` that reached the wrong one
    /// would be a drain that waits out its deadline beside an idle server.
    fn land(&self) {
        let mut gate = self.gate();
        gate.outstanding = gate.outstanding.saturating_sub(1);
        drop(gate);
        self.room.notify_all();
    }
}

/// An address to *reach* a listener bound to `local`.
///
/// `0.0.0.0` and `::` are how a socket says "every interface", and they are not
/// addresses a client connects to — every platform this runtime supports treats
/// a connect to them as loopback, but writing that down is cheaper than relying
/// on it.
fn dial_address(local: SocketAddr) -> SocketAddr {
    match local.ip() {
        IpAddr::V4(host) if host.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), local.port())
        }
        IpAddr::V6(host) if host.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), local.port())
        }
        _ => local,
    }
}

/// The transport one HTTP/1.1 message is framed over.
///
/// **The framing never asks which kind it has.** `read_request`, `refuse` and
/// the response writer are `Read + Write` over this and nothing else, which is
/// what makes "TLS changed the transport and not the server" a property of the
/// code rather than a claim: there is one parser, one writer, and one place —
/// [`serve_connection`] — where the difference exists at all. `http.rs` reached
/// the same shape from the client's side and for the same reason.
enum Wire {
    Plain(TcpStream),
    /// Boxed because a `rustls` connection is two kilobytes of buffers and a
    /// session, and every cleartext connection would otherwise carry the size
    /// of one.
    #[cfg(feature = "net")]
    Tls(Box<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>),
}

impl Wire {
    /// The socket underneath, whichever transport is on top of it.
    ///
    /// One caller and one reason: a WebSocket waits on a *file descriptor*
    /// rather than on a condition variable, because what it waits for is either
    /// bytes from the far side or a message another task enqueued, and only one
    /// of those two can be a lock. The framing above still never asks which
    /// kind of wire it has.
    #[cfg(feature = "net")]
    fn stream(&self) -> &TcpStream {
        match self {
            Wire::Plain(s) => s,
            Wire::Tls(s) => &s.sock,
        }
    }
}

impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Wire::Plain(s) => s.read(buf),
            #[cfg(feature = "net")]
            Wire::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Wire::Plain(s) => s.write(buf),
            #[cfg(feature = "net")]
            Wire::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Wire::Plain(s) => s.flush(),
            #[cfg(feature = "net")]
            Wire::Tls(s) => s.flush(),
        }
    }
}

/// One response, on its way back to whoever is holding the other end.
#[cfg(feature = "net")]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Where a response goes, which is the one thing the two protocols do not
/// share.
///
/// HTTP/1.1 owns its socket, so the answer is bytes written on it. HTTP/2 does
/// not: the socket belongs to a `hyper` connection driving several streams at
/// once, and this stream's answer is a value handed back to the task holding
/// that stream open. Both are "the connection", and the id [`accept`] answers
/// with names either.
enum Answer {
    Wire(Wire),
    #[cfg(feature = "net")]
    Stream(tokio::sync::oneshot::Sender<Reply>),
}

/// One accepted connection: the request already read off it, and where the
/// answer goes back.
///
/// The request is held here between [`accept`] and [`request`] because those are
/// two calls — `effect Listen` argues why. What is held is a **Rust** value: the
/// runtime still builds no Buri value it does not hand over inside the same
/// call, which is the division that lets this acceptor keep its blocking
/// implementation rather than become a scheduler.
struct Pending {
    /// Which listener accepted it, so that closing a listener drops the
    /// connections no worker will be handed back.
    listener: i64,
    /// Whether answering this one gives a place in [`Gate::outstanding`] back.
    ///
    /// True for HTTP/1.1, where a connection and a request are the same thing;
    /// false for an HTTP/2 stream, where the *socket* is what was counted and
    /// what gives the place back is `hyper`'s connection task ending.
    counted: bool,
    /// `None` once the answer has been given, which is also what makes
    /// answering twice a no-op rather than a second response.
    answer: Option<Answer>,
    /// `None` once `listenRequest` has taken it.
    request: Option<ServerRequest>,
    /// The `sec-websocket-key` this request offered, when it is a WebSocket
    /// upgrade this acceptor could complete, and `None` when it is not.
    ///
    /// **Decided when the request is framed and kept beside it**, rather than
    /// asked of the request later, for one reason: `listenRequest` *takes* the
    /// request, so by the time `core/net/server` asks to upgrade there is no
    /// request left here to read. One `Option<String>` is also all an upgrade
    /// needs from the head — everything else about it is `tungstenite`'s.
    ///
    /// It is `None` for every HTTP/2 stream. RFC 9113 §8.5 has its own
    /// mechanism for this and it is not `upgrade:`, so an h2 client asking the
    /// HTTP/1.1 way is asking for something that is not there.
    upgrade: Option<String>,
}

/// The open listeners, by handle.
///
/// A table rather than a pointer across the ABI: a handle is an `Int` in Buri
/// and an index here, so a program cannot hand the runtime an address, and a
/// stale one is a lookup that misses rather than a use-after-free.
///
/// The value is an `Arc` rather than the [`Listening`] itself, and that is what
/// makes `listenClose` safe to call while a worker is blocked: the closing
/// worker takes the entry out of the table, the blocked one is still holding a
/// clone, and the socket outlives the close by exactly as long as somebody is
/// inside an accept on it.
static ACCEPTORS: Mutex<Option<HashMap<i64, Arc<Listening>>>> = Mutex::new(None);

/// The connections whose requests have been handed out and not yet answered.
///
/// **This table is why `listenRespond` names a connection and not a listener.**
/// With one worker, "the request the accept last handed out" named exactly one
/// connection and the listener was identifier enough; with a worker per handler
/// there are as many outstanding requests as there are handlers. The id is
/// minted at the accept, crosses to Buri inside `Accepted`, and comes back at
/// the respond — the smallest change that could carry the fan-out, and what
/// `Listener`'s handle being an opaque `Int` was for.
static CONNECTIONS: Mutex<Option<HashMap<i64, Pending>>> = Mutex::new(None);

/// The next handle. Never reused, so a handle closed and a handle never opened
/// are the same answer — `.Err(.Closed)` — and neither is another server's.
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

/// The next connection id, on [`NEXT_HANDLE`]'s terms and for its reason: a
/// connection already answered and one that never existed are both a lookup that
/// misses.
static NEXT_CONNECTION: AtomicI64 = AtomicI64::new(1);

fn acceptors<T>(with: impl FnOnce(&mut HashMap<i64, Arc<Listening>>) -> T) -> T {
    let mut guard = match ACCEPTORS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    with(guard.get_or_insert_with(HashMap::new))
}

fn connections<T>(with: impl FnOnce(&mut HashMap<i64, Pending>) -> T) -> T {
    let mut guard = match CONNECTIONS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    with(guard.get_or_insert_with(HashMap::new))
}

/// The listener behind a handle, or the answer for a handle that names nothing.
///
/// The `Arc` is cloned out of the table and held for the whole of the call, for
/// the reason [`ACCEPTORS`] states: a worker inside an accept keeps the socket
/// open even when another worker closes the listener underneath it.
fn listening(handle: i64) -> Result<Arc<Listening>, ServeErr> {
    acceptors(|table| table.get(&handle).map(Arc::clone).ok_or_else(ServeErr::closed))
}


/// The sentence a `net`-off toolchain owes a server that asked for TLS.
///
/// It is the shape `http.rs`'s `https://` refusal already has, from the other
/// side of the wire, and for its reason: with the feature off there is no
/// `rustls` in the archive, so the true answer is about how the toolchain was
/// built and not about what the acceptor can frame.
pub const TLS_OFF: &str =
    "TLS is not supported by this toolchain's native runtime: it was built without the runtime's \
     `net` feature, so it carries no TLS code. Serve cleartext by leaving `Server`'s `tls` field \
     unset";

/// The server's identity, read and checked at the bind.
///
/// **At the bind and not at the first handshake**, which is the decision here:
/// a certificate that is missing, unreadable or does not match its key is a
/// configuration mistake, and a configuration mistake should stop a program
/// starting rather than surface as one refused connection at a time.
#[cfg(feature = "net")]
fn identity(plan: &ServePlan) -> Result<Option<Arc<rustls::ServerConfig>>, ServeErr> {
    match (&plan.certificate, &plan.key) {
        (None, None) => Ok(None),
        (Some(certificate), Some(key)) => {
            let config = crate::tls::server_config(certificate, key, plan.alpn())
                .map_err(|detail| ServeErr::new(ServeFail::Transport, detail))?;
            Ok(Some(Arc::new(config)))
        }
        // Half a `Tls` is not a thing `core/net/server` can build — the struct
        // has both fields — so this is what an out-of-tree caller of the C ABI
        // is told rather than something a Buri program can reach.
        (certificate, _) => Err(ServeErr::new(
            ServeFail::Transport,
            format!(
                "tls: a {} with no {} is not an identity a server can present",
                if certificate.is_some() { "certificate" } else { "private key" },
                if certificate.is_some() { "private key" } else { "certificate" }
            ),
        )),
    }
}

#[cfg(not(feature = "net"))]
fn identity(plan: &ServePlan) -> Result<Option<()>, ServeErr> {
    if plan.certificate.is_some() || plan.key.is_some() {
        return Err(ServeErr::new(ServeFail::Unsupported, TLS_OFF));
    }
    Ok(None)
}

/// `Listen::listenBind`, without the ABI around it.
pub fn bind(
    address: &str,
    port: i64,
    plan: &ServePlan,
    limit: i64,
    idle_millis: i64,
) -> Result<(i64, u16, i64), ServeErr> {
    // C4's handoff, spent: the walk over `Server`'s `protocols` field, one
    // `serves` per named protocol, with the acceptor's own answer after it.
    // An empty list is `.None` — the caller chose nothing — and HTTP/1.1 is
    // what the acceptor picks, so there is nothing to refuse.
    for protocol in &plan.protocols {
        accepts(*protocol).map_err(|detail| ServeErr::new(ServeFail::Unsupported, detail))?;
    }
    let secure = identity(plan)?;
    // Asked for h2 and gave no certificate: refused rather than quietly
    // answered in HTTP/1.1. [`H2_NEEDS_TLS`] is where the reasoning is.
    if plan.protocols.contains(&Protocol::Http2) && secure.is_none() {
        return Err(ServeErr::new(ServeFail::Unsupported, H2_NEEDS_TLS));
    }
    let host = if address.is_empty() { "127.0.0.1" } else { address };
    let Ok(port) = u16::try_from(port.clamp(0, i64::from(u16::MAX))) else {
        return Err(ServeErr::new(ServeFail::AddressNotAvailable, format!("bad port: {port}")));
    };
    let listener = TcpListener::bind((host, port))
        .map_err(|e| ServeErr::io(&e, &format!("listening on {host}:{port}")))?;
    let local = listener
        .local_addr()
        .map_err(|e| ServeErr::io(&e, "asking the socket which port it got"))?;
    let bound = local.port();
    let plan = Plan {
        limit: (limit >= 0).then_some(limit as u64),
        idle: (idle_millis >= 0).then_some(Duration::from_millis(idle_millis.max(0) as u64)),
        head: HEAD_DEADLINE,
        body_limit: BODY_LIMIT,
        drain: plan.drain.unwrap_or(DRAIN_DEADLINE),
        answer: ANSWER_DEADLINE,
        socket_buffer: plan.socket_buffer.unwrap_or(SOCKET_BUFFER),
    };
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    // **The listener stays blocking, whatever the caller asked for.** It used
    // to be made non-blocking when there was an idle deadline, because the
    // worker that waited for a connection was the worker that had to notice the
    // deadline. The acceptor thread below does neither: the deadline is now the
    // *worker's* wait on [`Listening::arrived`], which a condition variable
    // times out natively, and the one thread inside `accept(2)` is interrupted
    // by [`Listening::wake`]'s dial. Polling is gone from this file.
    let listening = Arc::new(Listening {
        listener,
        plan,
        gate: Mutex::new(Gate {
            finished: false,
            draining: false,
            served: 0,
            outstanding: 0,
            ready: VecDeque::new(),
            failure: None,
        }),
        arrived: Condvar::new(),
        room: Condvar::new(),
        wake_at: dial_address(local),
        #[cfg(feature = "net")]
        tls: secure,
        #[cfg(feature = "net")]
        stopping: tokio::sync::Notify::new(),
    });
    let accepting = Arc::clone(&listening);
    let started = std::thread::Builder::new()
        .name(String::from("buri-acceptor"))
        .spawn(move || accepting_on(&accepting, handle));
    match started {
        Ok(_joining) => {}
        Err(e) => return Err(ServeErr::io(&e, "starting the acceptor")),
    }
    acceptors(|table| {
        table.insert(handle, listening);
        // Under the table's own lock, so that the disposition and the reason
        // for it cannot disagree: a listener is open exactly while a signal
        // means "drain", and [`shutdown::listen`] is where that is argued.
        shutdown::listen();
    });
    Ok((handle, bound, MAX_HANDLERS))
}

/// The acceptor thread: one per listener, and the only thing in this process
/// inside `accept(2)` on it.
///
/// It takes connections as fast as they arrive and hands each to a thread of
/// its own, so that a slow TLS handshake or a client that sends its request one
/// byte at a time costs a connection rather than a *handler*. [`IN_FLIGHT`] is
/// what stops that from being unbounded, and it is the reason the loop starts
/// by asking for room rather than by accepting.
fn accepting_on(listening: &Arc<Listening>, handle: i64) {
    while listening.take_flight() {
        let stream = match listening.listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) => {
                // The accept itself failed, and there is nobody on this thread
                // to report it to — so it is left in the gate for the first
                // worker to ask, which is the same `.Err` `serve` would have
                // returned when the workers did the accepting themselves.
                let mut gate = listening.gate();
                gate.outstanding = gate.outstanding.saturating_sub(1);
                gate.finished = true;
                gate.failure.get_or_insert_with(|| ServeErr::io(&e, "accepting"));
                drop(gate);
                listening.arrived.notify_all();
                return;
            }
        };
        // The connection may be [`Listening::wake`]'s own dial rather than a
        // client's, and what this thread does with one is give the port back.
        //
        // A drain ends this thread here too, and the connection it was holding
        // is dropped unanswered. That is the line a shutdown has to draw
        // somewhere: a request already framed is answered, and a socket the
        // kernel accepted a moment after the signal is not, because a server
        // that kept taking connections while it drained would never finish.
        let gate = listening.gate();
        if gate.finished || gate.draining {
            drop(gate);
            listening.land();
            return;
        }
        drop(gate);
        let connection = Arc::clone(listening);
        dispatch(move || serve_connection(&connection, handle, stream));
    }
}

/// Run one connection's framing somewhere that is not the acceptor thread.
///
/// `spawn_blocking` where there is a reactor, because its pool reuses threads
/// and bounds itself; a plain thread where there is not, which is a `net`-off
/// toolchain and therefore a program `runtime_native::net_intrinsic` refused
/// before code generation. Either way the work is blocking — a handshake and a
/// read with deadlines on them — and [`IN_FLIGHT`] is what bounds how many are
/// in progress.
fn dispatch(work: impl FnOnce() + Send + 'static) {
    #[cfg(feature = "net")]
    {
        crate::rt::handle().spawn_blocking(work);
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = std::thread::Builder::new().name(String::from("buri-connection")).spawn(work);
    }
}

/// What a socket turned out to be once TLS had its say.
enum Handshaken {
    /// HTTP/1.1, framed by this file, over cleartext or over TLS.
    Framing(Wire),
    /// HTTP/2: `hyper` owns the socket and the place in flight now.
    Multiplexed,
    /// Nothing to answer with — the handshake never completed.
    Refused,
}

/// Cleartext, TLS, or `hyper` — the one place in the acceptor where the
/// transport is a question.
#[cfg(feature = "net")]
fn handshake(listening: &Arc<Listening>, handle: i64, mut stream: TcpStream) -> Handshaken {
    let Some(config) = listening.tls.clone() else {
        return Handshaken::Framing(Wire::Plain(stream));
    };
    let conn = match crate::tls::accept(config, &mut stream) {
        Ok(conn) => conn,
        // A handshake that did not complete is a conversation that never
        // started: there is no HTTP layer to answer on, and a plaintext status
        // line written to a client expecting ciphertext is noise. The socket is
        // dropped, which is what every TLS server does with one.
        Err(_) => return Handshaken::Refused,
    };
    if conn.alpn_protocol() == Some(crate::tls::ALPN_H2) {
        return match h2(listening, handle, stream, conn) {
            Ok(()) => Handshaken::Multiplexed,
            Err(()) => Handshaken::Refused,
        };
    }
    Handshaken::Framing(Wire::Tls(Box::new(rustls::StreamOwned::new(conn, stream))))
}

#[cfg(not(feature = "net"))]
fn handshake(_listening: &Arc<Listening>, _handle: i64, stream: TcpStream) -> Handshaken {
    Handshaken::Framing(Wire::Plain(stream))
}

/// One connection, from the socket to a request a worker can be handed.
///
/// Everything slow about a connection happens here and not on a worker: the
/// deadline, the handshake, and the read of a request a client may be sending
/// slowly. What reaches [`Gate::ready`] is a message that has been framed
/// whole, which is what lets `listenRequest` wait on nothing.
fn serve_connection(listening: &Arc<Listening>, handle: i64, stream: TcpStream) {
    if deadlines(&stream, listening.plan.head).is_err() {
        listening.land();
        return;
    }
    let mut wire = match handshake(listening, handle, stream) {
        Handshaken::Framing(wire) => wire,
        // `hyper` has the socket, and its connection task is what gives the
        // place in flight back.
        Handshaken::Multiplexed => return,
        Handshaken::Refused => {
            listening.land();
            return;
        }
    };
    // The budget starts here and not at the accept: the handshake above has a
    // bound of its own (`tls.rs`'s round cap, and the socket deadlines this
    // function set), and [`HEAD_DEADLINE`] is written as what a client gets to
    // *say what it wants* rather than as the connection's whole life.
    let until = Instant::now() + listening.plan.head;
    let request = match read_request(&mut wire, listening.plan.body_limit, until) {
        Ok(request) => request,
        // A malformed or oversized request is the transport's problem: it is
        // answered here and never reaches a handler. Surfacing it would make
        // every handler a parser's error path.
        Err(status) => {
            let _ = refuse(&mut wire, status, "");
            listening.land();
            return;
        }
    };
    let upgrade = upgrade_key(&request);
    let pending = Pending {
        listener: handle,
        counted: true,
        answer: Some(Answer::Wire(wire)),
        request: Some(request),
        upgrade,
    };
    if let Err(unclaimed) = listening.hand_over(pending) {
        // The limit was spent while this request was being read, so there is no
        // handler for it. The client is told rather than left waiting for an
        // answer nobody was handed.
        if let Some(Answer::Wire(mut wire)) = unclaimed.answer {
            let _ = refuse(&mut wire, 503, "");
        }
        listening.land();
    }
}

/// `Listen::listenAccept` — the next request, once one has arrived and been
/// read whole.
///
/// **A queue rather than a socket**, and that is F4's change here. What a
/// worker waits for is a connection somebody else has already accepted,
/// handshaken and framed, which is what lets one worker be handed an HTTP/1.1
/// connection and the next be handed the third stream of an HTTP/2 one without
/// either of them knowing the difference. It is also what makes a malformed
/// request invisible from here: [`serve_connection`] answers it and never joins
/// the queue, so a handler is never handed a message the acceptor could not
/// make sense of.
pub fn accept(handle: i64) -> Result<i64, ServeErr> {
    let listening = listening(handle)?;
    let waiting_until = listening.plan.idle.map(|idle| (Instant::now() + idle, idle));
    let mut gate = listening.gate();
    loop {
        if let Some(connection) = gate.ready.pop_front() {
            return Ok(connection);
        }
        // Asked after the queue is drained: a request that was framed before
        // the failure is still a request somebody should answer.
        if let Some(e) = gate.failure.take() {
            return Err(e);
        }
        if gate.finished {
            return Err(ServeErr::closed());
        }
        let Some((deadline, idle)) = waiting_until else {
            gate = match listening.arrived.wait(gate) {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            continue;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ServeErr::new(
                ServeFail::Timeout,
                format!("no connection arrived within {} ms", idle.as_millis()),
            ));
        }
        gate = match listening.arrived.wait_timeout(gate, left) {
            Ok((g, _)) => g,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

/// `Listen::listenRequest` — the request read off a connection [`accept`]
/// answered with.
///
/// **Does not wait**, and is not a second accept: the request was framed and
/// read whole before the connection was handed out, so this is a table lookup
/// and a move. A connection nobody is holding — already read, already answered,
/// or its listener closed under it — is `.Closed`.
pub fn request(connection: i64) -> Result<ServerRequest, ServeErr> {
    connections(|table| {
        table
            .get_mut(&connection)
            .and_then(|pending| pending.request.take())
            .ok_or_else(ServeErr::closed)
    })
}

/// Both socket deadlines, from the plan's one number. Per step, as
/// `http.rs::DEADLINE` is and for its reason: `SO_RCVTIMEO` is per read.
///
/// **And the socket is made blocking first, which is not redundant.** POSIX
/// says an accepted socket does not inherit the listener's file status flags;
/// **BSD-derived kernels, macOS among them, say it does**, and a non-blocking
/// socket ignores `SO_RCVTIMEO` entirely — a read with nothing to find answers
/// `WouldBlock` at once instead of waiting out the deadline just set on it.
///
/// That combination was a real, intermittent, macOS-only bug and it is worth
/// naming: F3's acceptor made the *listener* non-blocking whenever the caller
/// set an idle deadline, which every test in this repository does. Every socket
/// it accepted was therefore non-blocking on macOS, and whenever an accept won
/// the race against its client's first write, `read_request`'s first read
/// answered `WouldBlock`, `read_request` read that as a stalled client, and the
/// acceptor answered `408` and dropped a connection nobody was ever handed. One
/// lost connection out of eight is a ninth accept that waits out the idle
/// deadline, which is how it presented:
/// `Timeout: no connection arrived within 20000 ms`, on CI, on macOS, about one
/// run in three.
///
/// F4 removed the cause — the listener is blocking now, always, because the
/// deadline moved to the worker's own wait (see [`bind`]) — so this line is a
/// belt rather than the fix. It is here because the *property* a read needs is
/// "this socket blocks", and a property should be established where it is
/// needed rather than inferred from a decision two functions away.
fn deadlines(stream: &TcpStream, head: Duration) -> Result<(), ServeErr> {
    stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_read_timeout(Some(head)))
        .and_then(|()| stream.set_write_timeout(Some(head)))
        .map_err(|e| ServeErr::io(&e, "setting the connection's deadlines"))
}

/// Read one request off a connection, or answer with the status it deserves.
///
/// The `Err` is a status code rather than a `ServeErr` on purpose: nothing here
/// is the *server's* failure, so none of it is a reason for `serve` to return.
fn read_request(
    stream: &mut impl Read,
    body_limit: usize,
    until: Instant,
) -> Result<ServerRequest, u16> {
    let mut buffer = Vec::with_capacity(1024);
    let head = loop {
        if let Some(at) = find(&buffer, b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > 64 * 1024 {
            return Err(431);
        }
        // **The aggregate, and it is what the socket options cannot do.**
        // `SO_RCVTIMEO` bounds one `read(2)` and the kernel restarts it on the
        // next, so a client sending one byte every twenty-nine seconds resets
        // the deadline for ever and never trips it — a slowloris, and the
        // oldest way there is to hold a server's connection open with almost no
        // traffic. Checked at the top of the loop rather than after the read,
        // so that a client which has already had its whole budget gets no
        // further read at all.
        if Instant::now() >= until {
            return Err(408);
        }
        let mut chunk = [0u8; 1024];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(n) => buffer.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            Err(_) => return Err(408),
        }
    };
    let Head { method, target, headers } = parse_head(buffer.get(..head).unwrap_or(&[]))?;
    let declared = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .map(|(_, v)| v.trim().parse::<usize>().map_err(|_| 400u16))
        .transpose()?
        .unwrap_or(0);
    if declared > body_limit {
        return Err(413);
    }
    // A `transfer-encoding` this acceptor does not frame is refused rather than
    // guessed at: a body read with the wrong framing is a body that belongs to
    // the next request.
    if headers.iter().any(|(n, v)| n == "transfer-encoding" && !v.trim().is_empty()) {
        return Err(501);
    }
    let mut body = buffer.split_off(head.saturating_add(4));
    while body.len() < declared {
        // The same aggregate, over the same budget: [`HEAD_DEADLINE`] is
        // documented as what one connection gets for "its request line, its
        // headers and its body", and a drip is a drip on either side of the
        // blank line.
        if Instant::now() >= until {
            return Err(408);
        }
        let want = declared.saturating_sub(body.len());
        let mut chunk = vec![0u8; want.min(64 * 1024)];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(n) => body.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            Err(_) => return Err(408),
        }
    }
    body.truncate(declared);
    Ok(ServerRequest { method, target, headers, body })
}

/// A request line and the header fields after it: everything a
/// [`ServerRequest`] is except its body, which is read afterwards and whose
/// length the fields decide.
struct Head {
    method: i32,
    target: String,
    headers: Vec<(String, String)>,
}

/// The request line and the header fields.
fn parse_head(head: &[u8]) -> Result<Head, u16> {
    let text = std::str::from_utf8(head).map_err(|_| 400u16)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(400u16)?;
    let mut parts = request_line.split(' ');
    let (verb, target, version) =
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(v), Some(t), Some(p), None) => (v, t, p),
            _ => return Err(400),
        };
    if !version.starts_with("HTTP/1.") {
        return Err(505);
    }
    // The wire spelling is the runtime's business and never Buri's: this is the
    // only place a server reads one, exactly as `METHODS` is the only place a
    // client writes one.
    let method = crate::http::METHODS
        .iter()
        .position(|m| *m == verb)
        .ok_or(501u16)
        .and_then(|i| i32::try_from(i).map_err(|_| 501u16))?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        // Lowercased on the way in, which is what `Header`'s declaration
        // promises a reader: `request.header("content-type")` is an exact
        // comparison and never a case-insensitive scan.
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(Head { method, target: target.to_string(), headers })
}

/// A status and nothing else, for a connection the acceptor is refusing.
fn refuse(stream: &mut impl Write, status: u16, body: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// `Listen::listenRespond` — the answer to one accepted request, named by the
/// connection id [`accept`] minted for it.
///
/// A connection nobody is holding is `.Ok(())` and writes nothing: it has been
/// answered already, or its listener was closed under it, and neither is
/// something the loop did wrong.
pub fn respond(
    connection: i64,
    status: i64,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(), ServeErr> {
    let taken = connections(|table| table.remove(&connection));
    let Some(pending) = taken else { return Ok(()) };
    let status = status.clamp(100, 599) as u16;
    // Whichever transport this was, the same three names are the transport's
    // own and not the caller's: framing is not a header a handler may write,
    // and a second `content-length` is a request smuggling primitive. HTTP/2
    // has no `connection` at all, so passing one on would be a protocol error
    // rather than a duplicate.
    let fields: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, _)| {
            name != "content-length" && name != "connection" && name != "transfer-encoding"
        })
        .cloned()
        .collect();
    let written = match pending.answer {
        Some(Answer::Wire(mut wire)) => write_response(&mut wire, status, &fields, body),
        #[cfg(feature = "net")]
        Some(Answer::Stream(back)) => {
            // The stream's own task is holding the other end; if it has gone —
            // the client reset the stream, or the connection closed — the send
            // fails and there is nothing to write it to, which is the `.Ok(())`
            // an already-answered connection gets for the same reason.
            let _ = back.send(Reply {
                status,
                headers: fields,
                body: body.to_vec(),
            });
            Ok(())
        }
        None => Ok(()),
    };
    if let (true, Ok(listening)) = (pending.counted, listening(pending.listener)) {
        listening.land();
    }
    written
}

/// The HTTP/1.1 response, on the wire.
fn write_response(
    wire: &mut Wire,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(), ServeErr> {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", body.len()));
    wire.write_all(head.as_bytes())
        .and_then(|()| wire.write_all(body))
        .and_then(|()| wire.flush())
        .map_err(|e| ServeErr::io(&e, "writing the response"))
}

/// `Listen::listenClose`. Closing twice has nothing to do, which is what lets a
/// loop unwinding out of a failure tidy up without remembering how far it got.
///
/// **It is also how one worker stops the others.** A worker whose accept or
/// respond failed calls this before it returns its error, so the workers waiting
/// beside it are woken and answered `.Closed` rather than waiting out a server
/// that has already failed. What makes it safe to call while the acceptor thread
/// is inside an `accept(2)` is [`ACCEPTORS`]'s `Arc`: the entry leaves the table
/// now, the dial below brings the thread out of the call, and the socket closes
/// when the last holder of a clone lets go.
pub fn close(handle: i64) {
    let Some(listening) = acceptors(|table| {
        let gone = table.remove(&handle);
        // The last port back is the last reason to hold the signals, and
        // giving them up under the table's lock is what keeps the two from
        // disagreeing — [`shutdown::listen`] takes them the same way.
        if table.is_empty() {
            shutdown::quiet();
        }
        gone
    }) else {
        return;
    };
    listening.finish();
    listening.wake();
    // A connection no worker will be handed back is an answer nobody will
    // write, so it is dropped here rather than held until the process ends. An
    // HTTP/1.1 client sees the socket close and an HTTP/2 stream sees its
    // `oneshot` sender go, which is what a server going down looks like from
    // the outside.
    //
    // **And a dropped connection has left flight**, which is why the count goes
    // with them. It matters because a drain waits on that number: a worker
    // whose loop failed closes the listener beside a shutdown that is draining
    // it, and a connection this dropped but still counted would make that drain
    // wait out its whole deadline for an answer nobody was ever going to write.
    // A worker that answers one of these afterwards finds nothing in the table
    // and returns without landing it a second time.
    let dropped = connections(|table| {
        let mut counted = 0usize;
        table.retain(|_, pending| {
            let ours = pending.listener == handle;
            if ours && pending.counted {
                counted += 1;
            }
            !ours
        });
        counted
    });
    if dropped > 0 {
        let mut gate = listening.gate();
        gate.outstanding = gate.outstanding.saturating_sub(dropped);
        drop(gate);
        listening.room.notify_all();
    }
    // And the sockets this listener accepted, for the connections' reason one
    // paragraph up: a socket whose worker has been told the listener is closed
    // is a socket nobody will ever read again, and its place in flight would
    // hold a drain of a *different* listener open for the whole of its
    // deadline. `close_all_on` is what gives both back.
    close_sockets_on(handle);
}

#[cfg(feature = "net")]
fn close_sockets_on(handle: i64) {
    sockets::close_all_on(handle);
}

#[cfg(not(feature = "net"))]
fn close_sockets_on(_handle: i64) {}

/// Every socket on a listener, told the server is going away — what a drain
/// means for the half of a server that a client *keeps*.
///
/// **A WebSocket is h2's problem again**, and it gets h2's answer. An HTTP/1.1
/// connection carries one message and ends, so a drain that stopped accepting
/// was already finished with it; an HTTP/2 connection and a WebSocket are both
/// things a client holds open on purpose, and a drain that could only wait
/// would wait out its whole deadline for every one of them. So a socket is
/// closed with 1001 and its worker runs `onClose` — which is more than the
/// GOAWAY does, because a WebSocket program has a hook to run and an h2 one
/// does not.
#[cfg(feature = "net")]
fn drain_sockets_on(handle: i64) {
    sockets::going_away(handle);
}

#[cfg(not(feature = "net"))]
fn drain_sockets_on(_handle: i64) {}

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------
//
// **What a shutdown has to interrupt is the accept, and F3 wrote that down two
// slices before there was anything to interrupt it with.** `listenAccept` is
// where a server spends its idle life: it is the one call in `effect Listen`
// that can wait forever, and every other call in this file is bounded by a
// deadline somebody set. So a server that is asked to stop is a server whose
// waiting workers have to be told, and the telling is [`Gate::finished`] — the
// one-way door that has been here since F3 and that `core/net/server`'s
// `answering` already turns into `.Ok(())`.
//
// What F5 adds is the half in front of that door: **stop taking connections,
// answer the ones already taken, and only then close it.** Three states rather
// than two, and the middle one is [`Gate::draining`]:
//
// ```text
//   serving    accepting, framing, answering
//      |
//      |  a signal, or `drain`
//      v
//   draining   the acceptor thread is gone; the queue is still handed out and
//      |       the requests in flight are still answered
//      |  every connection answered, or the drain deadline
//      v
//   finished   `listenAccept` says `.Closed`, every worker ends, `run` closes
//              the port and `serve` answers `.Ok(())`
// ```
//
// The difference between draining and closing is what makes this graceful, and
// it is one line: [`close`] drops the connections no worker will be handed
// back, and a drain waits for them instead.
//
// ## Why the signal is caught at all, and what a second one does
//
// The default disposition of `SIGINT` and `SIGTERM` is to terminate the
// process, which for a server means every request in flight is a client holding
// a socket that closed mid-response. Catching them is how a deployment gets to
// say "stop when you have finished what you are doing".
//
// It is caught **only while a port is open**, which is the answer to the
// obvious objection. A signal handler is process-wide state, and a runtime that
// installed one at startup would change what Ctrl-C does to every program this
// compiler builds — including the ones that are not servers, which would then
// have to be killed twice. So [`bind`] takes the disposition and the last
// [`close`] gives it back, both under [`ACCEPTORS`]'s own lock, and a program
// with no listener open is a program whose signals are the operating system's
// again.
//
// **The second signal is the operating system's too.** The handler restores the
// default disposition for the signal it was called with before it does anything
// else, so a second Ctrl-C during a slow drain terminates the process the
// ordinary way. That is the behaviour a person pressing it twice means, and it
// is also the reason the drain deadline is not the only thing standing between
// a hung handler and a process that will not stop.

/// Stop accepting, wait for the requests already in flight, and then say the
/// listener is closed. `true` if it drained within its deadline.
///
/// This is [`close`]'s gentler sibling and the two are deliberately different
/// verbs: `close` takes the port back *now* and drops whatever was in flight,
/// which is what a worker whose loop failed wants; this one answers what it
/// took first. Both end with `listenAccept` saying `.Closed`, which is what
/// `core/net/server` turns into `.Ok(())`, so a drained server and a closed one
/// look the same to a program.
///
/// The port itself is **not** given back here. `run` closes the listener when
/// its last worker returns, which is one line further along the path a request
/// limit already takes, and a drain that closed the port would be racing that.
pub fn drain(handle: i64) -> bool {
    let Ok(listening) = listening(handle) else { return true };
    listening.begin_drain();
    drain_sockets_on(handle);
    let drained = listening.drained(Instant::now() + listening.plan.drain);
    listening.finish();
    drained
}

/// Every open listener, drained at once — what a signal means.
///
/// **At once and not one after another**, which is what the shared deadline is:
/// the listeners are told to stop taking connections in one pass and then
/// waited for against a single instant, so a process holding four ports takes
/// one drain deadline to stop rather than four. The instant is the longest any
/// one of them asked for, because a deadline is a promise to the slowest
/// listener and not a budget divided between them.
fn drain_all() {
    let listeners: Vec<(i64, Arc<Listening>)> =
        acceptors(|table| table.iter().map(|(handle, open)| (*handle, Arc::clone(open))).collect());
    // No port open is a signal with nothing to drain, which is the race at the
    // very end of a server's life: the last `close` gave the signals back a
    // moment after this one was delivered. Nothing to do, and nothing lost —
    // the disposition is the operating system's again, so the next one
    // terminates the process the ordinary way.
    let Some(until) = listeners.iter().map(|(_, l)| l.plan.drain).max().map(|d| Instant::now() + d)
    else {
        return;
    };
    for (handle, listening) in &listeners {
        listening.begin_drain();
        drain_sockets_on(*handle);
    }
    for (_, listening) in &listeners {
        listening.drained(until);
    }
    for (_, listening) in &listeners {
        listening.finish();
    }
}

/// The two signals, the disposition they are held under, and the thread that
/// answers them.
///
/// # What may happen inside a signal handler
///
/// Almost nothing. A handler runs on whatever thread the kernel picked, at
/// whatever point that thread had reached — which on this runtime may be inside
/// `malloc`, inside the allocator's own lock, or part-way through B9's stack
/// switch with a stack pointer that belongs to neither carrier nor task. So the
/// only calls a handler may make are the ones POSIX lists as
/// async-signal-safe, and [`on_signal`] makes exactly three of them: it takes
/// the errno slot's address, it restores the default disposition, and it writes
/// one byte.
///
/// **The byte is the whole design.** Everything a shutdown actually has to do —
/// take locks, wait on condition variables, dial a socket — is done by
/// [`watching`], an ordinary thread blocked on an ordinary read, and the
/// self-pipe is how the handler reaches it. It is the oldest trick in this part
/// of the library and it is here for the reason it always is: the alternative
/// is a handler that takes a lock a signal may have interrupted.
///
/// **`errno` is saved and restored**, which is the part that is easy to leave
/// out. `write` sets it, and the thread this handler interrupted may be between
/// a failing call and its own `errno` read; a handler that clobbered it would
/// turn somebody else's `EAGAIN` into a success. Both platforms reach the slot
/// through a function of their own — `__error` on macOS, `__errno_location` on
/// Linux — and both are async-signal-safe, being a thread-local address and
/// nothing more.
///
/// # Why not `tokio::signal`
///
/// It is the same self-pipe with a reactor in front of it, and it costs the
/// `signal` feature in a crate that is linked into every native binary this
/// compiler produces. `manifest.toml`'s dependency set is closed by an exact
/// list for that reason, and the same argument that hand-wrote `mmap`'s three
/// declarations in `memory.rs` hand-writes these three. It would also be
/// unavailable exactly where the acceptor is not: a `net`-off runtime has no
/// reactor, and this file's acceptor is compiled into that archive too.
mod shutdown {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::sync::{Mutex, OnceLock};

    /// `SIGINT` — a person at a terminal. Two on both platforms this runtime
    /// is built for; `ARCHITECTURE.md` §9 admits Linux and macOS and no third.
    pub(super) const SIGINT: i32 = 2;
    /// `SIGTERM` — a supervisor, a container runtime, an `init`. Fifteen on
    /// both.
    pub(super) const SIGTERM: i32 = 15;

    /// The two, in the order they are installed and restored.
    const CAUGHT: [i32; 2] = [SIGINT, SIGTERM];

    /// `SIG_DFL`, which is the null handler pointer on both platforms.
    const SIG_DFL: usize = 0;
    /// `SIG_ERR`, which `signal` answers when it will not do what it was asked.
    const SIG_ERR: usize = usize::MAX;

    // The three calls this module makes into the C library, declared rather
    // than depended on — `memory.rs`'s `mmap` block, one file over, is the
    // precedent and carries the argument.
    //
    // `signal` rather than `sigaction` because the two platforms lay `struct
    // sigaction` out differently and nothing here needs a field of it: the
    // disposition is all that is being set, both platforms give `signal` BSD
    // semantics (the handler stays installed, and an interrupted syscall is
    // restarted rather than answered `EINTR`), and restarting is what this file
    // wants — the accept is interrupted by a dial and not by a signal, and a
    // read that answered `EINTR` because somebody pressed Ctrl-C would be a
    // client refused for the server's reasons.
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
        fn write(fd: i32, buf: *const core::ffi::c_void, count: usize) -> isize;
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        /// The address of this thread's `errno`.
        fn __error() -> *mut i32;
    }

    #[cfg(not(target_os = "macos"))]
    unsafe extern "C" {
        /// The same, spelled the way glibc and musl spell it.
        fn __errno_location() -> *mut i32;
    }

    fn errno_slot() -> *mut i32 {
        #[cfg(target_os = "macos")]
        // SAFETY: a thread-local address, and nothing else.
        unsafe {
            __error()
        }
        #[cfg(not(target_os = "macos"))]
        // SAFETY: as above.
        unsafe {
            __errno_location()
        }
    }

    /// The writing end of the self-pipe, as a raw descriptor a handler may
    /// read out of an atomic. `-1` until there is one.
    static WAKE: AtomicI32 = AtomicI32::new(-1);

    /// The reading end, held for the process's life so that the writing end is
    /// never a pipe with no reader.
    static PIPE: OnceLock<UnixStream> = OnceLock::new();

    /// Whether the disposition is currently ours. Under [`super::ACCEPTORS`]'s
    /// lock at every use, which is what makes it agree with the table.
    static HELD: AtomicBool = AtomicBool::new(false);

    /// The watching thread, started once and never stopped: a thread blocked
    /// on a read costs nothing, and a process that has served once may serve
    /// again.
    static WATCHER: Mutex<bool> = Mutex::new(false);

    /// The handler. **Everything it does is on POSIX's async-signal-safe list**
    /// and the module header says why that matters.
    extern "C" fn on_signal(sig: i32) {
        let slot = errno_slot();
        // SAFETY: `errno_slot` answers this thread's own `errno`.
        let saved = unsafe { slot.read() };
        // The default disposition back, first: from here on a second signal
        // terminates the process, which is what somebody pressing Ctrl-C twice
        // means and what stops a hung handler from being unkillable.
        // SAFETY: setting a disposition is async-signal-safe.
        unsafe { signal(sig, SIG_DFL) };
        let fd = WAKE.load(Ordering::Relaxed);
        if fd >= 0 {
            let byte = [sig as u8];
            // SAFETY: `fd` is the writing end of a pipe this process holds
            // both ends of, and one byte of a two-byte buffer is readable.
            // A short write and a failed write are the same thing here: the
            // reader is woken by any byte at all.
            unsafe { write(fd, byte.as_ptr().cast(), 1) };
        }
        // SAFETY: as above. Restored last, so that nothing between the two
        // reads a value this handler produced.
        unsafe { slot.write(saved) };
    }

    /// The thread the handler wakes: drain every open listener, then wait for
    /// the next signal.
    ///
    /// It loops rather than returning after one, because the handler restores
    /// the default disposition **per signal**: a `SIGINT` leaves `SIGTERM`
    /// caught, and a supervisor that sends both should get one drain apiece
    /// rather than one drain and one unnoticed signal.
    fn watching(mut pipe: UnixStream) {
        use std::io::Read;
        let mut byte = [0u8; 1];
        while matches!(pipe.read(&mut byte), Ok(1)) {
            super::drain_all();
        }
    }

    /// Take the two signals, if a listener has not taken them already.
    ///
    /// Called with [`super::ACCEPTORS`]'s lock held, which is what makes "the
    /// disposition is ours exactly while a port is open" true rather than
    /// nearly true.
    pub(super) fn listen() {
        if HELD.swap(true, Ordering::Relaxed) {
            return;
        }
        start();
        for sig in CAUGHT {
            // SAFETY: an ordinary `signal` call with a function this module
            // owns. `SIG_ERR` is what a signal that may not be caught answers,
            // and neither of these two is one — but a runtime that could not
            // take them should serve rather than refuse to start, so the
            // failure is left to the operating system's own default.
            let previous = unsafe { signal(sig, on_signal as *const () as usize) };
            debug_assert!(previous != SIG_ERR, "SIGINT and SIGTERM can be caught");
        }
    }

    /// Give them back. Called under the same lock, when the last port closes.
    pub(super) fn quiet() {
        if !HELD.swap(false, Ordering::Relaxed) {
            return;
        }
        for sig in CAUGHT {
            // SAFETY: as above. The handler may already have restored one of
            // the two, and setting a disposition that is already the default
            // is what it looks like when it has.
            unsafe { signal(sig, SIG_DFL) };
        }
    }

    /// The self-pipe and the thread that reads it, made once.
    fn start() {
        let mut made = match WATCHER.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if *made {
            return;
        }
        let Ok((reading, writing)) = UnixStream::pair() else { return };
        let fd = writing.as_raw_fd();
        // The writing end is kept alive by the `OnceLock` for as long as the
        // process is: a descriptor whose owner was dropped is a number the next
        // `open` may be given, and a signal handler writing one byte into an
        // unrelated file is the worst kind of bug to go looking for.
        if PIPE.set(writing).is_err() {
            return;
        }
        let started = std::thread::Builder::new()
            .name(String::from("buri-shutdown"))
            .spawn(move || watching(reading));
        if started.is_err() {
            return;
        }
        // Last, so that a handler cannot find a descriptor nobody is reading.
        WAKE.store(fd, Ordering::Relaxed);
        *made = true;
    }

    /// What [`on_signal`] is, as a number — for the one test that asserts the
    /// disposition really is this module's.
    #[cfg(test)]
    pub(super) fn handler_address() -> usize {
        on_signal as *const () as usize
    }
}

/// The reason phrase for a status, for the statuses this acceptor writes.
///
/// A phrase the client mostly ignores — HTTP/2 has none at all — so the list is
/// the ones written here plus a range fallback, rather than a table of every
/// code IANA has registered.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        505 => "HTTP Version Not Supported",
        // The class, for a status this list does not name. Guards rather than
        // ranges, because a range overlapping the arms above is a lint even
        // where the order settles it, and "the arms above win" is a claim
        // better made by the condition than by the ordering.
        s if s < 200 => "Informational",
        s if s < 300 => "OK",
        s if s < 400 => "Redirection",
        s if s < 500 => "Client Error",
        _ => "Server Error",
    }
}

/// The first occurrence of `needle` in `haystack`. `http.rs`'s, on this side of
/// the wire; two of them rather than one shared helper because the two files
/// are each other's mirror and neither owns the other's framing.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|i| haystack.get(*i..i + needle.len()) == Some(needle))
}

// ---------------------------------------------------------------------------
// HTTP/2 — the half that is `hyper`'s
// ---------------------------------------------------------------------------
//
// **This is where `hyper` is finally linked, and it is linked for the thing it
// is for.** F2 wrote the argument for framing HTTP/1.1 by hand and F3 re-took
// the decision and kept it: a request line, some fields and a body over one
// connection is two hundred lines, and reaching a framing layer through a
// crate to get at them would have been a reactor bought for nothing. None of
// that survives contact with HTTP/2. HPACK is a stateful compression context
// shared by every stream on a connection, flow control is a windowed credit
// scheme in two directions at two levels, and the whole point of the protocol
// is that several requests are in flight on one socket at once. That is not
// framing anybody should write twice, and it is what the dependency bar in the
// root manifest exists to admit.
//
// What it costs, and what it does not: the acceptor above is unchanged. The
// same [`Gate::ready`] queue, the same [`Pending`] table, the same
// `listenAccept`/`listenRequest`/`listenRespond`. An HTTP/2 *stream* becomes a
// connection id like any other, and a worker that is handed one cannot tell —
// which is the property that made this a slice rather than a second server.
//
// The one place the two differ is where the answer goes ([`Answer`]), because
// an h2 stream does not own its socket: `hyper` does, and this stream's
// response is a value handed back to the task holding the stream open.

/// Hand a handshaken h2 connection to `hyper`, on the process's own reactor.
///
/// The connection keeps its place in [`Gate::outstanding`] for as long as
/// `hyper` is driving it, and gives it back when the task ends — so a client
/// that opens a connection and holds it open counts once, however many requests
/// it sends on it.
///
/// **And that is exactly why a drain has to reach in here.** An HTTP/1.1
/// connection carries one message and ends; an HTTP/2 connection is a thing a
/// client keeps, so a shutdown that only stopped accepting would wait out its
/// whole deadline for a client doing nothing wrong. `hyper` has the right
/// answer already — `graceful_shutdown` sends the GOAWAY frame that says "no
/// new streams, finish the ones you have" — and what this had to grow is
/// somewhere to hear the drain from.
#[cfg(feature = "net")]
fn h2(
    listening: &Arc<Listening>,
    handle: i64,
    stream: TcpStream,
    conn: rustls::ServerConnection,
) -> Result<(), ()> {
    let io = crate::tls::AsyncTls::adopt(stream, conn).map_err(|_| ())?;
    let driving = Arc::clone(listening);
    crate::rt::handle().spawn(async move {
        let service = Multiplexed { listening: Arc::clone(&driving), handle };
        let connection = hyper::server::conn::http2::Builder::new(Spawn)
            .max_concurrent_streams(MAX_STREAMS)
            .serve_connection(io, service);
        let mut connection = std::pin::pin!(connection);
        let stopping = driving.stopping.notified();
        let mut stopping = std::pin::pin!(stopping);
        let mut asked = false;
        // Armed by the GOAWAY and not before. An h2 connection a client is
        // using is the wait a server is *for* and carries no deadline; a
        // connection that has been told to go and has not gone is a different
        // thing, and `net.rs`'s rule covers it.
        let mut giving_up: Option<Pin<Box<tokio::time::Sleep>>> = None;
        let patience = driving.plan.drain;
        // Hand-written rather than `tokio::select!`, which would be the
        // `macros` feature in a crate every native binary carries for the sake
        // of one two-armed wait. The arms are not symmetric anyway: the
        // connection is the future being driven and the notification is a thing
        // that happens to it once.
        //
        // The result is deliberately dropped. Every way an h2 connection can
        // end — the client went away, a stream was reset, the settings could
        // not be agreed — is that client's connection ending and not this
        // server's failure, and `serve` answers for the server.
        let _served = std::future::poll_fn(|cx| {
            if !asked {
                // Polled before the flag is read, and both are asked every
                // time: polling is what registers the waker, and the flag is
                // what catches a drain that began before this task existed.
                let woken = stopping.as_mut().poll(cx).is_ready();
                if woken || driving.gate().draining {
                    asked = true;
                    connection.as_mut().graceful_shutdown();
                    giving_up = Some(Box::pin(tokio::time::sleep(patience)));
                }
            }
            // **The bound `graceful_shutdown` does not carry.** GOAWAY is a
            // request: it says "no new streams, finish the ones you have", and
            // a client is free to hold its open streams for as long as it
            // likes. Until this arm existed, one that did held this task, its
            // socket and its place in [`Gate::outstanding`] for the life of the
            // process — `DRAIN_DEADLINE` bounded the *waiter* in [`drain`] and
            // nothing bounded this. The plan's drain number is the right one to
            // use twice: it is the answer the caller already gave to "how long
            // should a shutdown keep a deployment waiting", and a connection
            // still here after it is one the deployment has stopped counting
            // on.
            //
            // Dropping the connection is what ends it, and it is a TCP close
            // rather than a lost answer: every stream on it either was
            // answered, or is a handler whose `listenRespond` will find the
            // connection gone — which is exactly what `listenClose` under a
            // running handler already does.
            //
            // Asked **after** the connection has been polled and only while it
            // is still pending, so that a connection which was one poll away
            // from finishing cleanly finishes cleanly. The deadline is for a
            // connection that will not end, not for one that is ending.
            let served = connection.as_mut().poll(cx);
            if served.is_pending()
                && let Some(sleep) = giving_up.as_mut()
                && sleep.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Ok(()));
            }
            served
        })
        .await;
        driving.land();
    });
    Ok(())
}

/// Where `hyper` puts the tasks one connection needs.
///
/// `hyper`'s HTTP/2 server takes an executor rather than assuming one, which is
/// what lets this be three lines instead of `hyper-util`: the process already
/// has exactly one reactor and `rt.rs` owns it.
#[cfg(feature = "net")]
#[derive(Clone)]
struct Spawn;

#[cfg(feature = "net")]
impl<F> hyper::rt::Executor<F> for Spawn
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, future: F) {
        crate::rt::handle().spawn(future);
    }
}

/// One HTTP/2 connection's service: every stream on it becomes a connection id
/// on the listener's queue.
#[cfg(feature = "net")]
struct Multiplexed {
    listening: Arc<Listening>,
    handle: i64,
}

#[cfg(feature = "net")]
impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for Multiplexed {
    type Response = hyper::Response<Once>;
    /// Infallible in practice: every way a stream can fail to be answered is a
    /// status code, because a status code is a thing the client can read and a
    /// service error is a connection the client loses.
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, request: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let listening = Arc::clone(&self.listening);
        let handle = self.handle;
        Box::pin(async move { Ok(answering(listening, handle, request).await) })
    }
}

/// One HTTP/2 stream, from `hyper`'s request to the response a worker wrote.
#[cfg(feature = "net")]
async fn answering(
    listening: Arc<Listening>,
    handle: i64,
    request: hyper::Request<hyper::body::Incoming>,
) -> hyper::Response<Once> {
    let (parts, incoming) = request.into_parts();
    // The wire spelling is the runtime's business and never Buri's, exactly as
    // it is in `parse_head` one screen up: `METHODS` is the one table, and a
    // method it does not name is a `501` here as it is there.
    let Some(method) = crate::http::METHODS.iter().position(|m| *m == parts.method.as_str()) else {
        return only(501);
    };
    let Ok(method) = i32::try_from(method) else { return only(501) };
    // `:path` as the client sent it, which is what `Request.path()` and
    // `Request.query()` read. HTTP/2 sends the origin form in a pseudo-header,
    // so unlike HTTP/1.1 there is nothing to strip and nothing to reconstruct.
    let target = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), ToString::to_string);
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                // Already lowercase on the wire in HTTP/2 (RFC 9113 §8.2.1),
                // and lowercased anyway so that `Header`'s promise is this
                // file's and not the protocol's.
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = match collected(incoming, listening.plan.body_limit).await {
        Ok(body) => body,
        Err(status) => return only(status),
    };
    let (back, written) = tokio::sync::oneshot::channel();
    let pending = Pending {
        listener: handle,
        // The *socket* was counted, once, when it was accepted; a stream is not
        // a connection in flight, and `MAX_STREAMS` is what bounds how many of
        // these there can be.
        counted: false,
        answer: Some(Answer::Stream(back)),
        request: Some(ServerRequest { method, target, headers, body }),
        // RFC 9113 §8.5 replaced `upgrade:` outright, so an h2 stream asking
        // the HTTP/1.1 way is asking for a mechanism the protocol removed.
        upgrade: None,
    };
    if listening.hand_over(pending).is_err() {
        return only(503);
    }
    answered_within(written, listening.plan.answer).await
}

/// The handler's answer, or the status that says why there is not one.
///
/// **Bounded**, which is [`ANSWER_DEADLINE`]'s row: a `oneshot` receive with no
/// deadline waits on a Buri handler calling `listenRespond`, and escapes only
/// if the sender is dropped. A handler that loops for ever, or one that waits
/// on something that never arrives, would otherwise hold this task and its
/// stream until the process ended.
///
/// The deadline is a parameter for `http.rs`'s `fetch_within` reason: a test
/// that had to wait thirty seconds to watch the bound fire is a test nobody
/// runs, and this one is not otherwise reachable — it needs a handler that
/// never answers, which no other case in this file has.
#[cfg(feature = "net")]
async fn answered_within(
    written: tokio::sync::oneshot::Receiver<Reply>,
    deadline: Duration,
) -> hyper::Response<Once> {
    match tokio::time::timeout(deadline, written).await {
        Ok(Ok(reply)) => answered(reply),
        // The sender went without sending, which is `listenClose` having
        // dropped this connection: the server is going down under a request it
        // took, and the client is told so rather than left holding a stream.
        Ok(Err(_)) => only(503),
        // The handler took the request and has not answered. `504` rather than
        // `503`: the request was accepted and dispatched, and the thing that
        // did not happen in time is the answer.
        Err(_) => only(504),
    }
}

/// A request body, read whole and bounded by the same number HTTP/1.1's is.
///
/// Hand-rolled rather than `http-body-util`'s `BodyExt::collect`, for the
/// reason every other hand-rolled thing in this crate is hand-rolled: a crate
/// in every binary this compiler produces needs a better argument than fifteen
/// lines. The limit is checked as the frames arrive rather than at the end,
/// because a body that is refused after it has been buffered has already cost
/// what the limit exists to prevent.
#[cfg(feature = "net")]
async fn collected(body: hyper::body::Incoming, limit: usize) -> Result<Vec<u8>, u16> {
    use hyper::body::Body as _;

    let mut out: Vec<u8> = Vec::new();
    let mut body = std::pin::pin!(body);
    while let Some(frame) = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
        let Ok(frame) = frame else { return Err(400) };
        // Data frames are the body; a trailers frame is not, and this acceptor
        // has nowhere to put one — `Request` is four fields and none of them is
        // "the fields that came after the body".
        if let Ok(data) = frame.into_data() {
            if out.len().saturating_add(data.len()) > limit {
                return Err(413);
            }
            out.extend_from_slice(&data);
        }
    }
    Ok(out)
}

/// A response body of one piece, or of none.
///
/// `hyper::body::Body` is a stream of frames and a handler's answer is a `[U8]`
/// that already exists, so there is exactly one frame and then the end. It is
/// written out rather than reached for through `http-body-util`'s `Full` for
/// [`collected`]'s reason.
#[cfg(feature = "net")]
pub struct Once(Option<hyper::body::Bytes>);

#[cfg(feature = "net")]
impl hyper::body::Body for Once {
    type Data = hyper::body::Bytes;
    /// There is no way for this body to fail: the bytes are in memory before
    /// the response is built.
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.get_mut().0.take().map(|bytes| Ok(hyper::body::Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_none()
    }

    /// Exact, so that `hyper` can write a `content-length` and the client knows
    /// how much is coming. An unknown length would make every answer a chunked
    /// one for no reason.
    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::with_exact(self.0.as_ref().map_or(0, |b| b.len() as u64))
    }
}

/// A status and nothing else — [`refuse`]'s answer, in HTTP/2's shape.
#[cfg(feature = "net")]
fn only(status: u16) -> hyper::Response<Once> {
    let mut response = hyper::Response::new(Once(None));
    *response.status_mut() = hyper::StatusCode::from_u16(status)
        .unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    response
}

/// The handler's answer, in HTTP/2's shape.
#[cfg(feature = "net")]
fn answered(reply: Reply) -> hyper::Response<Once> {
    let mut response = hyper::Response::new(Once(Some(hyper::body::Bytes::from(reply.body))));
    *response.status_mut() = hyper::StatusCode::from_u16(reply.status)
        .unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in reply.headers {
        // A field name or value HTTP will not carry is dropped rather than
        // allowed to take the whole response down: a handler that wrote a
        // newline into a header has made one header wrong, not one server.
        let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::try_from(name),
            hyper::header::HeaderValue::try_from(value),
        ) else {
            continue;
        };
        response.headers_mut().append(name, value);
    }
    response
}

// ---------------------------------------------------------------------------
// WebSockets — the half `tungstenite` frames
// ---------------------------------------------------------------------------
//
// **This is the commit that links `tungstenite`, and it links it for the same
// reason F4 linked `hyper`: the part nobody should write twice.** RFC 6455 is a
// framing with a masking rule, a fragmentation rule, a control-frame rule, a
// close handshake and a conformance suite (Autobahn) that exists because
// implementations get it wrong. What this file keeps is what it has always
// kept — the HTTP/1.1 head that carries the upgrade, which is a status line and
// four fields — and what it hands over is every byte after the `101`.
//
// **The upgrade is a hundred and one and a hash.** `derive_accept_key` is the
// only thing this file borrows from the handshake half of that crate: it is
// SHA-1 over the client's key and a constant GUID, base64'd, and the response
// around it is four header fields written here. `tungstenite::accept` would
// have read the request off the socket itself, and the request was read three
// functions ago — a server that read it twice would be a server that framed it
// twice.
//
// ## Where a socket lives
//
// **On the worker that accepted it**, and that is the whole shape. F2 put the
// accept loop in Buri because the runtime cannot hold a Buri value between two
// calls of its own; F6 reached the same answer for an actor's state; and a
// socket's state is the same question a third time — `onOpen` answers a value,
// every `onMessage` answers the next one, and `onClose` sees the last. So
// `core/net/server` runs a socket's whole life on one worker, the state is a
// local threaded through a self tail call, and this file holds a queue and a
// framing rather than anything belonging to a program.
//
// Two things follow and both are worth stating:
//
// * **Hooks on one socket run in order by construction.** There is one caller
//   inside [`sockets::receive`] for any socket, so there is nothing to
//   serialise. The lock on the framing is there to make that a fact rather than
//   a promise, not to arbitrate between two workers that should not exist.
// * **A socket occupies a worker.** `MAX_HANDLERS` sockets is as many as a
//   server can hold while still accepting, which is F3's bound arriving in a
//   second place for F3's reason: `effect Tasks` has no detached spawn, so the
//   loop that reads a socket is a loop somebody is inside. It moves on the day
//   a task can outlive the call that started it, and nothing about this file's
//   surface moves with it.
//
// ## The one wait that is two waits
//
// A socket's loop waits for either of two things: bytes from the far side, or a
// message another task enqueued for it. The first is a file descriptor and the
// second is a lock, and there is no portable way to wait on both — which is why
// this is the one place in this file that reaches for `poll(2)` and a
// self-pipe. `Listening::wake` dials a port for the same reason one screen up;
// `shutdown` writes a byte into a pipe for the same reason one screen down.
//
// What that buys is that **`socketSendText` never waits and never spins**: it
// takes a lock, pushes onto a queue, writes one byte, and returns. The socket's
// own worker is out of `poll` before the caller's next instruction, and an idle
// socket costs nothing at all until somebody has something to say to it.

/// The `sec-websocket-key` a request offered, when it is an upgrade this
/// acceptor could complete, and `None` when it is not.
///
/// **Every clause RFC 6455 §4.2.1 requires**, and none of them is negotiable:
/// a `GET`, `upgrade: websocket`, `connection` containing `upgrade`,
/// `sec-websocket-version: 13`, and a key. A request missing any of them is not
/// an upgrade, and `core/net/server` hands it to the ordinary handler — which
/// is the same thing that happens to an upgrade request on a server with no
/// socket hooks, and is what makes the whole feature invisible to a program
/// that does not use it.
///
/// The two multi-valued fields are searched rather than compared. `connection`
/// is a comma-separated list and browsers send `keep-alive, Upgrade`; `upgrade`
/// is a list too, in principle. Both are matched case-insensitively because
/// RFC 9110 §7.6.1 says these tokens are, and a server that refused
/// `Connection: keep-alive, Upgrade` would be refusing correct clients.
fn upgrade_key(request: &ServerRequest) -> Option<String> {
    // `.Get` is index 0 in `Method`'s declaration order, which is
    // `crate::http::METHODS`'s order.
    if request.method != 0 {
        return None;
    }
    let field = |name: &str| {
        request.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.trim().to_string())
    };
    let names = |value: Option<String>, token: &str| {
        value.is_some_and(|v| v.split(',').any(|part| part.trim().eq_ignore_ascii_case(token)))
    };
    if !names(field("upgrade"), "websocket") || !names(field("connection"), "upgrade") {
        return None;
    }
    // The one version this protocol has had. A client asking for another is
    // owed a `sec-websocket-version: 13` response header by the RFC; it gets an
    // ordinary handler instead, which is a server that does not do WebSockets
    // as far as that client can tell, and is the honest answer from an acceptor
    // that speaks exactly one.
    if field("sec-websocket-version").as_deref() != Some("13") {
        return None;
    }
    field("sec-websocket-key").filter(|key| !key.is_empty())
}

/// The sentence a connection that was not an upgrade is refused with.
///
/// It is a refusal `core/net/server` never shows anybody: any `.Err` from
/// `listenUpgrade` means "this is an ordinary request", and the connection is
/// still there to be answered. The words are for whoever reads a `ServeError`
/// out of a hand-written loop.
pub const NOT_AN_UPGRADE: &str =
    "this request is not a WebSocket upgrade: an upgrade is a GET carrying `upgrade: websocket`, \
     `connection: upgrade`, `sec-websocket-version: 13` and a `sec-websocket-key`";

/// The sentence a `net`-off toolchain owes a program that asked for one.
pub const SOCKETS_OFF: &str =
    "WebSockets are not supported by this toolchain's native runtime: it was built without the \
     runtime's `net` feature, so it carries no RFC 6455 framing. Leave `Server`'s `websocket` \
     field unset and answer the request in `onRequest`";

/// What `listenReceive` answers with, before it becomes a Buri value.
///
/// The three fields a `Frame` does not name are empty, which is the contract
/// `Received`'s declaration states rather than something this struct enforces.
#[derive(Debug)]
pub struct Received {
    /// `Frame`'s variant index: 0 `.Text`, 1 `.Binary`, 2 `.Closed`.
    pub frame: i8,
    pub text: String,
    pub data: Vec<u8>,
    pub code: i64,
}

impl Received {
    fn text(text: &str) -> Received {
        Received { frame: 0, text: text.to_string(), data: Vec::new(), code: 0 }
    }

    fn binary(data: &[u8]) -> Received {
        Received { frame: 1, text: String::new(), data: data.to_vec(), code: 0 }
    }

    fn closed(code: i64) -> Received {
        Received { frame: 2, text: String::new(), data: Vec::new(), code }
    }
}

/// What a socket that filled its outbound buffer answers `Received.code` with.
///
/// **Negative, and that is the point.** A close code on the wire is unsigned
/// (RFC 6455 §5.5.1 makes it two octets), so no peer can send this number and
/// no peer can therefore claim that *this* server's buffer overflowed. It is
/// how a platform says a socket ended for a reason that was never on the wire,
/// and `core/net/server`'s `reasonOf` is the one place it is read.
const OVERFLOWED: i64 = -1;

/// What the far side is told when this side's buffer overflowed: 1011, "an
/// internal error", which is the true sentence from where the client is
/// standing.
const OVERFLOW_CODE: u16 = 1011;

/// What a socket open when a shutdown begins is closed with: 1001, "going
/// away".
///
/// **The WebSocket half of h2's GOAWAY**, and it is here for that decision's
/// reason. A WebSocket is a thing a client *keeps* — keeping it is what the
/// protocol is for — so a drain that only stopped accepting would wait out its
/// whole deadline for every open socket, which is not a corner case but the
/// ordinary shutdown of a server that has any. So a drain closes them, and
/// `onClose` still runs, so a program still gets to undo what `onOpen` did.
const GOING_AWAY: u16 = 1001;

/// What a socket that ended without a close frame answers with: 1006, which
/// RFC 6455 §7.4.1 reserves for exactly that and forbids on the wire.
const NO_CLOSE_FRAME: i64 = 1006;

/// What a close frame carrying no code answers with: 1005, reserved for "there
/// was no status code" and forbidden on the wire beside 1006.
const NO_CLOSE_CODE: i64 = 1005;

#[cfg(feature = "net")]
mod sockets {
    use std::collections::{HashMap, VecDeque};
    use std::io::{Read, Write};
    use std::os::unix::io::{AsRawFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use tungstenite::protocol::frame::coding::CloseCode;
    use tungstenite::protocol::{CloseFrame, Role, WebSocket, WebSocketConfig};

    use super::{
        Answer, GOING_AWAY, NOT_AN_UPGRADE, NO_CLOSE_CODE, NO_CLOSE_FRAME, OVERFLOWED,
        OVERFLOW_CODE, Received, ServeErr, ServeFail, Wire, connections, listening,
    };

    /// How long [`waiting`] sleeps before it looks again with nothing having
    /// woken it.
    ///
    /// **A backstop and not a poll**, and the difference is the number: every
    /// state change on a socket writes the wake byte under the same lock that
    /// made the change, so a socket that is doing nothing is woken by nothing
    /// and this is what happens when that reasoning is wrong. One second costs
    /// one wakeup per open socket per second — three orders of magnitude below
    /// the ten-millisecond poll F3 measured as a server that took twenty
    /// seconds to notice a client — and what it buys is that a lost wakeup is a
    /// one-second delay rather than a test suite that hangs.
    const BACKSTOP_MILLIS: i32 = 1_000;

    /// The read buffer `tungstenite` allocates per socket, eagerly.
    ///
    /// Eight kilobytes rather than the crate's own 128, because this runtime's
    /// sockets are bounded by `IN_FLIGHT` and not by one connection's
    /// throughput: a hundred and twenty-eight open sockets at the default would
    /// be sixteen megabytes of read buffers in a process that has not received
    /// a byte. A server that moves enough traffic for this to matter is a
    /// server that wants a knob, and the knob would be a `Serve` variant.
    const READ_BUFFER: usize = 8 * 1024;

    /// `POLLIN` — there is something to read, or the far side has gone. The
    /// same one on both platforms this runtime is built for.
    const POLLIN: i16 = 0x0001;
    /// `POLLOUT` — there is room to write. Asked for only when a flush has
    /// already answered `WouldBlock`.
    const POLLOUT: i16 = 0x0004;

    /// The two descriptors this waits on, as the width `poll` takes them in.
    const TWO: NFds = 2;

    /// `struct pollfd`, which is three fields in this order on both platforms.
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    /// `nfds_t` is an `unsigned int` on macOS and an `unsigned long` on Linux.
    /// Two lines rather than one, because a mismatched integer width in a C
    /// declaration is the kind of thing that works until it does not.
    #[cfg(target_os = "macos")]
    type NFds = u32;
    #[cfg(not(target_os = "macos"))]
    type NFds = u64;

    // The one call this module makes into the C library, declared rather than
    // depended on — `memory.rs`'s `mmap` block and `shutdown`'s `signal` block
    // are the precedents and carry the argument.
    //
    // `poll` rather than `select` because `select` cannot describe a descriptor
    // above `FD_SETSIZE` and a server holding a thousand connections has them;
    // `poll` rather than `kqueue`/`epoll` because those are two implementations
    // of one idea and this waits on exactly two descriptors.
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: NFds, timeout: i32) -> i32;
    }

    /// One message on its way out, before it is framed.
    enum Queued {
        Text(String),
        Binary(Vec<u8>),
    }

    /// How a socket is going to end, decided by this side.
    #[derive(Clone)]
    struct Ending {
        /// What the far side is sent in the close frame.
        code: u16,
        /// The words beside it.
        reason: String,
        /// What the socket's own loop is told, which is **not** always the
        /// same number: an overflow is [`OVERFLOWED`] here and
        /// [`OVERFLOW_CODE`] on the wire, because one is a fact about this
        /// server and the other is what the client can be told about it.
        told: i64,
    }

    /// What a socket has been handed and not yet written.
    struct Outbound {
        queue: VecDeque<Queued>,
        /// Set once, by whoever decides first. A second decision is dropped,
        /// which is what makes closing twice a no-op.
        ending: Option<Ending>,
    }

    /// One open socket: everything the tasks touching it share.
    ///
    /// **The framing and the queue are two locks and not one**, deliberately.
    /// The framing is held for as long as a `receive` is inside `poll`, which
    /// is most of a socket's life; a `send` that had to take that lock would be
    /// a `send` that waited for the far side to say something, which is exactly
    /// what `socketSendText` promises not to do. So a send touches the queue
    /// and the pipe, and neither is ever held across a wait.
    struct Socketed {
        /// Which listener accepted it, so a listener closing takes its sockets
        /// with it and so the place in flight goes back to the right gate.
        listener: i64,
        /// How many messages the queue holds before the socket is closed.
        bound: usize,
        out: Mutex<Outbound>,
        /// The framing. `None` once the socket has been retired, which is what
        /// makes a second `receive` `.Err` rather than a second close.
        framing: Mutex<Option<WebSocket<Wire>>>,
        /// The descriptor [`waiting`] watches for bytes.
        fd: RawFd,
        /// The write end of the self-pipe. One byte here brings the socket's
        /// own worker out of `poll`.
        wake: UnixStream,
        /// The read end, watched beside [`Socketed::fd`].
        woken: UnixStream,
        /// The listener took its sockets back while this one's worker was
        /// inside `poll`. Read on the way round.
        retired: AtomicBool,
        /// This socket's place in [`super::Gate::outstanding`] has been given
        /// back. Swapped rather than checked, so it happens exactly once
        /// however many paths reach it.
        landed: AtomicBool,
    }

    impl Socketed {
        fn out(&self) -> MutexGuard<'_, Outbound> {
            match self.out.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            }
        }

        fn framing(&self) -> MutexGuard<'_, Option<WebSocket<Wire>>> {
            match self.framing.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            }
        }

        /// One byte into the self-pipe. A failure is a pipe whose reader has
        /// gone, which is a socket already being retired.
        fn wake(&self) {
            let _woken = (&self.wake).write(&[1u8]);
        }
    }

    /// The open sockets, by handle — [`super::ACCEPTORS`]'s arrangement, for its
    /// reason: a handle is an `Int` in Buri and a key here, so a program cannot
    /// hand the runtime an address and a stale handle is a lookup that misses.
    static SOCKETS: Mutex<Option<HashMap<i64, Arc<Socketed>>>> = Mutex::new(None);

    /// The next socket. Never reused, so a socket closed and a socket never
    /// opened are the same answer.
    static NEXT_SOCKET: AtomicI64 = AtomicI64::new(1);

    fn table<T>(with: impl FnOnce(&mut HashMap<i64, Arc<Socketed>>) -> T) -> T {
        let mut guard = match SOCKETS.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        with(guard.get_or_insert_with(HashMap::new))
    }

    fn socketed(socket: i64) -> Option<Arc<Socketed>> {
        table(|open| open.get(&socket).map(Arc::clone))
    }

    /// Give this socket's place in flight back, at most once.
    ///
    /// A socket inherits the place its connection held from the moment it is
    /// upgraded — `listenRespond` is what would have given it back and an
    /// upgraded connection is never responded to — so a drain waits for open
    /// sockets exactly as it waits for requests in flight. What keeps that from
    /// being a wait nobody survives is [`GOING_AWAY`]: a drain closes them
    /// first.
    fn land(socket: &Socketed) {
        if socket.landed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(gate) = listening(socket.listener) {
            gate.land();
        }
    }

    /// Take a socket out of the table and let go of its framing, which is what
    /// closes the descriptor.
    fn retire(
        socket: i64,
        held: &mut Option<WebSocket<Wire>>,
        state: &Arc<Socketed>,
    ) {
        *held = None;
        table(|open| open.remove(&socket));
        land(state);
    }

    /// `Listen::listenUpgrade` — a connection becomes a socket, or says it is
    /// not one.
    ///
    /// **The connection is taken out of the table only when the upgrade is
    /// certain**, because every other answer has to leave it answerable: a
    /// request that was not an upgrade is one `core/net/server` hands to
    /// `onRequest`, and a connection that had been consumed would be a client
    /// left holding a socket nobody was going to write to.
    ///
    /// The one exception is a `101` that could not be written, and it is
    /// stated rather than hidden: the connection is gone by then, so the
    /// handler runs and its response goes nowhere. A socket whose first four
    /// lines did not reach the client is a socket that was never going to work.
    pub(super) fn upgrade(connection: i64) -> Result<i64, ServeErr> {
        let taken = connections(|table| {
            let ours = table.get(&connection).is_some_and(|pending| pending.upgrade.is_some());
            if ours { table.remove(&connection) } else { None }
        });
        let Some(pending) = taken else {
            return Err(ServeErr::new(ServeFail::Unsupported, NOT_AN_UPGRADE));
        };
        let listener = pending.listener;
        let refused = |detail: String| {
            // The connection has left the table, so its place in flight is this
            // function's to give back.
            if let Ok(gate) = listening(listener) {
                gate.land();
            }
            Err(ServeErr::new(ServeFail::Transport, detail))
        };
        let key = pending.upgrade.unwrap_or_default();
        let Some(Answer::Wire(mut wire)) = pending.answer else {
            return refused(String::from("this connection has already been answered"));
        };
        let open = listening(listener).ok();
        let limit = open.as_ref().map_or(0, |l| l.plan.body_limit);
        // The whole of the handshake this file writes: a status line and four
        // fields. `derive_accept_key` is SHA-1 over the client's key and RFC
        // 6455's constant GUID, base64'd, and it is the only part of it that
        // is anybody else's.
        let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
        let head = format!(
            "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-accept: {accept}\r\n\r\n"
        );
        if let Err(e) = wire.write_all(head.as_bytes()).and_then(|()| wire.flush()) {
            return refused(format!("writing the upgrade response: {e}"));
        }
        // **Non-blocking from here on**, which is what makes the wait below a
        // wait on two descriptors rather than a read that blocks: a framing
        // that answers `WouldBlock` is a framing that has told us everything it
        // has, and `poll` is what says when there is more.
        let stream = wire.stream();
        let fd = stream.as_raw_fd();
        if let Err(e) = stream.set_nonblocking(true) {
            return refused(format!("making the socket non-blocking: {e}"));
        }
        let Ok((woken, wake)) = UnixStream::pair() else {
            return refused(String::from("opening the socket's wakeup pipe"));
        };
        // Both ends, because the reader drains without blocking and the writer
        // must not block when nobody has drained yet — a full pipe means the
        // worker has already been woken and has not looked yet, which is a
        // wakeup that has done its job.
        if woken.set_nonblocking(true).is_err() || wake.set_nonblocking(true).is_err() {
            return refused(String::from("making the socket's wakeup pipe non-blocking"));
        }
        // Field by field rather than a struct literal: `WebSocketConfig` is
        // `#[non_exhaustive]`, which is the crate reserving the right to add a
        // knob without breaking this line.
        let mut config = WebSocketConfig::default();
        config.read_buffer_size = READ_BUFFER;
        // Zero, so a message is written to the stream as it is queued rather
        // than held for a flush that this loop would have to remember to make.
        // The buffering that matters is `Outbound`'s, which is the one a
        // program can bound.
        config.write_buffer_size = 0;
        config.max_message_size = (limit > 0).then_some(limit);
        let framing = WebSocket::from_raw_socket(wire, Role::Server, Some(config));
        let socket = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Socketed {
            listener,
            bound: open.as_ref().map_or(super::SOCKET_BUFFER, |l| l.plan.socket_buffer),
            out: Mutex::new(Outbound { queue: VecDeque::new(), ending: None }),
            framing: Mutex::new(Some(framing)),
            fd,
            wake,
            woken,
            retired: AtomicBool::new(false),
            landed: AtomicBool::new(false),
        });
        table(|open| open.insert(socket, state));
        Ok(socket)
    }

    /// `Listen::listenReceive` — the next thing to arrive on a socket.
    ///
    /// Three steps, in this order, and the order is the design: write what is
    /// queued, read what has arrived, and only then wait. A `receive` that
    /// waited first would be a socket whose outbound queue is delivered one
    /// message late, which for a broadcast is every subscriber waiting for the
    /// next thing anybody says.
    pub(super) fn receive(socket: i64) -> Result<Received, ServeErr> {
        let Some(state) = socketed(socket) else { return Err(ServeErr::closed()) };
        let mut held = state.framing();
        if held.is_none() {
            return Err(ServeErr::closed());
        }
        // Set when a flush has answered `WouldBlock`, so the wait asks about
        // room to write as well as bytes to read. Cleared as soon as one
        // succeeds.
        let mut blocked = false;
        loop {
            if state.retired.load(Ordering::SeqCst) {
                retire(socket, &mut held, &state);
                return Err(ServeErr::closed());
            }
            let Some(framing) = held.as_mut() else { return Err(ServeErr::closed()) };
            // 1. Everything that has been enqueued, and the close if one has
            //    been decided. Both are read under the queue's lock in one
            //    step, so a `send` that overflowed cannot have its message
            //    written and its close missed.
            let ending = {
                let mut out = state.out();
                while let Some(queued) = out.queue.pop_front() {
                    let message = match queued {
                        Queued::Text(text) => tungstenite::Message::Text(text.into()),
                        Queued::Binary(data) => tungstenite::Message::Binary(data.into()),
                    };
                    if framing.write(message).is_err() {
                        // The framing has gone. The close below is what the
                        // loop is told about it; the rest of the queue goes
                        // with the socket.
                        out.queue.clear();
                        out.ending.get_or_insert(Ending {
                            code: OVERFLOW_CODE,
                            reason: String::new(),
                            told: NO_CLOSE_FRAME,
                        });
                        break;
                    }
                }
                out.ending.clone()
            };
            blocked = flush(framing, blocked);
            if let Some(end) = ending {
                // The close frame is written and flushed on the way out, so a
                // client that is reading gets a reason rather than a socket
                // that stopped. `close` queues it and `flush` writes it; a
                // failure at either end is a client that has already gone.
                let _closed = framing.close(Some(CloseFrame {
                    code: CloseCode::from(end.code),
                    reason: end.reason.into(),
                }));
                let _flushed = framing.flush();
                retire(socket, &mut held, &state);
                return Ok(Received::closed(end.told));
            }
            // 2. Everything that has arrived. The loop is over `read` and not
            //    over `poll`, because `tungstenite` buffers: a second whole
            //    message may already be in its reader when the descriptor has
            //    nothing left to say.
            let mut arrived: Option<Received> = None;
            let mut ended: Option<i64> = None;
            loop {
                match framing.read() {
                    Ok(tungstenite::Message::Text(text)) => {
                        arrived = Some(Received::text(text.as_str()));
                        break;
                    }
                    Ok(tungstenite::Message::Binary(data)) => {
                        arrived = Some(Received::binary(&data));
                        break;
                    }
                    Ok(tungstenite::Message::Close(frame)) => {
                        ended = Some(
                            frame.map_or(NO_CLOSE_CODE, |f| i64::from(u16::from(f.code))),
                        );
                        break;
                    }
                    // **Ping and pong are the runtime's**, which is what this
                    // arm is: `tungstenite` has already queued the pong, the
                    // flush below writes it, and nothing above this line ever
                    // learns that a heartbeat happened.
                    Ok(_) => continue,
                    Err(tungstenite::Error::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        break;
                    }
                    // Every other way a read ends is this socket ending: the
                    // far side went without a close frame, or sent something
                    // that is not RFC 6455. Both are 1006 to the loop, which
                    // is `.Abnormal`.
                    Err(_) => {
                        ended = Some(NO_CLOSE_FRAME);
                        break;
                    }
                }
            }
            // The pong the read may have queued, and anything a `write` above
            // left buffered.
            blocked = flush(framing, blocked);
            if let Some(code) = ended {
                let _flushed = framing.flush();
                retire(socket, &mut held, &state);
                return Ok(Received::closed(code));
            }
            if let Some(received) = arrived {
                return Ok(received);
            }
            // 3. Nothing to write and nothing to read: wait for either.
            waiting(&state, blocked);
        }
    }

    /// Flush, and answer whether the stream is still refusing writes.
    ///
    /// A `WouldBlock` here is a client that is not reading, which is the
    /// ordinary state of a slow one: what it means is that the wait below has
    /// to ask about room to write as well as about bytes to read, and nothing
    /// more. The bytes stay in `tungstenite`'s own write buffer, which is why
    /// `Outbound`'s bound is the one a program sets — that queue is the one
    /// this runtime can see the depth of.
    fn flush(framing: &mut WebSocket<Wire>, blocked: bool) -> bool {
        match framing.flush() {
            Ok(()) => false,
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => true,
            Err(_) => blocked,
        }
    }

    /// Wait for bytes on the socket, for room to write on it, or for a byte on
    /// the self-pipe — whichever happens first.
    ///
    /// The pipe is drained here rather than counted: one byte and a hundred
    /// mean the same thing, which is "look again".
    fn waiting(state: &Socketed, blocked: bool) {
        let events = if blocked { POLLIN | POLLOUT } else { POLLIN };
        let mut fds = [
            PollFd { fd: state.fd, events, revents: 0 },
            PollFd { fd: state.woken.as_raw_fd(), events: POLLIN, revents: 0 },
        ];
        // SAFETY: two descriptors this process owns, in an array of two, with
        // a timeout in milliseconds. `poll` reads `nfds` entries and writes
        // `revents` into each, which is what a `&mut [PollFd; 2]` allows.
        let _ready = unsafe { poll(fds.as_mut_ptr(), TWO, BACKSTOP_MILLIS) };
        // The pipe is drained rather than counted: one byte and a hundred mean
        // the same thing, which is "look again".
        if fds.get(1).is_some_and(|pipe| pipe.revents & POLLIN != 0) {
            let mut sink = [0u8; 64];
            while let Ok(n) = (&state.woken).read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
        }
    }

    /// `Sockets::socketSendText` — a message onto the queue, and a byte into
    /// the pipe.
    ///
    /// **Never waits, and a handle that names nothing is a message dropped**,
    /// which are the same sentence from two directions: this side cannot tell a
    /// socket that closed a moment ago from one that never existed, and it was
    /// never in a position to answer "did this arrive" for either.
    fn send(socket: i64, message: Queued) {
        let Some(state) = socketed(socket) else { return };
        {
            let mut out = state.out();
            // A socket that is already ending takes nothing more: the close
            // frame is next on the wire, and a message queued behind it is one
            // the far side would never be told about anyway.
            if out.ending.is_some() {
                return;
            }
            if out.queue.len() >= state.bound {
                // **The overflow, and it closes the socket.** The message that
                // did not fit is dropped with the rest of the queue, because a
                // socket that is closing has nowhere to put them and a client
                // that receives half a stream is worse served than one that is
                // told the stream ended.
                out.queue.clear();
                out.ending = Some(Ending {
                    code: OVERFLOW_CODE,
                    reason: String::from("the outbound buffer overflowed"),
                    told: OVERFLOWED,
                });
            } else {
                out.queue.push_back(message);
            }
        }
        state.wake();
    }

    pub(super) fn send_text(socket: i64, text: &str) {
        send(socket, Queued::Text(text.to_string()));
    }

    pub(super) fn send_bytes(socket: i64, data: &[u8]) {
        send(socket, Queued::Binary(data.to_vec()));
    }

    /// `Sockets::socketClose` — the close this side decided on.
    ///
    /// The socket's own worker performs it, which is what keeps one thread
    /// inside the framing: this marks the queue and wakes it. The code the
    /// caller gave is both what the far side is sent and what `onClose` is
    /// told, so a program that closes with `.Normal` sees `.Normal`.
    pub(super) fn close(socket: i64, code: i64, reason: &str) {
        let Some(state) = socketed(socket) else { return };
        {
            let mut out = state.out();
            if out.ending.is_some() {
                return;
            }
            let wire = u16::try_from(code.clamp(1000, 4999)).unwrap_or(1000);
            out.ending =
                Some(Ending { code: wire, reason: reason.to_string(), told: i64::from(wire) });
        }
        state.wake();
    }

    /// Every socket on a listener, closed with 1001 — what a drain means.
    ///
    /// It does **not** wait: `Listening::drained` is already waiting on
    /// `Gate::outstanding`, and every socket gives its place back when its own
    /// worker performs the close. So a drain tells the sockets and then waits
    /// for the number it was already waiting for.
    pub(super) fn going_away(listener: i64) {
        for state in on(listener) {
            {
                let mut out = state.out();
                if out.ending.is_none() {
                    out.ending = Some(Ending {
                        code: GOING_AWAY,
                        reason: String::from("the server is shutting down"),
                        told: i64::from(GOING_AWAY),
                    });
                }
            }
            state.wake();
        }
    }

    /// Every socket on a listener, taken now — what `listenClose` means.
    ///
    /// `close` drops the connections nobody will answer and this drops the
    /// sockets nobody will read, which is the same sentence: a listener that
    /// has been closed is one whose workers are on their way out, and a socket
    /// waiting for one of them would wait forever. The place in flight goes
    /// back here rather than in a `receive` that may never happen again.
    pub(super) fn close_all_on(listener: i64) {
        for state in on(listener) {
            state.retired.store(true, Ordering::SeqCst);
            table(|open| {
                open.retain(|_, held| !Arc::ptr_eq(held, &state));
            });
            land(&state);
            state.wake();
        }
    }

    /// The sockets a listener accepted.
    fn on(listener: i64) -> Vec<Arc<Socketed>> {
        table(|open| {
            open.values().filter(|s| s.listener == listener).map(Arc::clone).collect()
        })
    }

    /// Whether **this** socket is still in the table — for the tests, which are
    /// the only thing that can ask.
    ///
    /// One socket and not a count, deliberately: the table is process-wide and
    /// the tests below share a process, so "the table is empty" would be a
    /// claim about every other case running beside this one. A test that
    /// asserted it would pass or fail depending on what the scheduler happened
    /// to be doing, which is the kind of row this repository fixes rather than
    /// retries.
    #[cfg(test)]
    pub(super) fn is_open(socket: i64) -> bool {
        table(|open| open.contains_key(&socket))
    }
}

// ---------------------------------------------------------------------------
// The C ABI
// ---------------------------------------------------------------------------
//
// Five `Listen` entries and three `Sockets` ones, at `lib.rs` §1's naming rule
// and §2's shapes. Three Buri values cross whole here — `Listener`, `Request`,
// `Response`, and `ServeError` beside them — and each is transcribed below as a
// `#[repr(C)]` struct rather than written field by field through separate
// out-pointers, for `BuriHeader`'s reason in `host.rs`: **the shape gets a name
// a reader can check against the Buri declaration.**

//
// What makes that legal is VALUE-MODEL.md §5, which lays a struct out as its
// fields back to back at their own alignments and never reorders them —
// `middle/layout.rs::record` is the function, and `#[repr(C)]` is the same
// rule. `the_transcribed_shapes_match_the_value_model` below asserts every size
// and offset, so a layout change that moved one of these is a failing unit test
// in this crate rather than a program reading the wrong bytes.

/// `Listener` — `{ handle: Int, port: Int, handlers: Int }`.
#[repr(C)]
pub struct BuriListener {
    handle: i64,
    port: i64,
    handlers: i64,
}

/// `Serve` — `enum { Speak(Protocol), Certificate(Str), PrivateKey(Str) }`,
/// and **the first payload-carrying enum this runtime reads.**
///
/// VALUE-MODEL.md §6 lays an enum out as `tag ++ payload`: the tag at offset
/// zero holding the variant's index in declaration order, and the payload area
/// starting at the first offset with the payload's own alignment
/// (`middle/layout.rs::record`'s sibling, the `Tagged` arm). The widest payload
/// here is a `Str`, which is three words and eight-aligned, so the tag is one
/// byte, the payload starts at **8**, and the whole is 32 —
/// `the_transcribed_shapes_match_the_value_model` asserts each of those.
///
/// A union rather than three structs because that is what the value model says
/// it is: one payload area, read according to the tag, with `Speak`'s single
/// byte occupying the first byte of the same area a `Str` would.
/// `ManuallyDrop` is Rust's rule for a union field and not a claim about
/// ownership — nothing here owns anything, exactly as `[Header]` does not.
#[repr(C)]
pub struct BuriServe {
    tag: i8,
    payload: BuriServePayload,
}

/// `Serve`'s payload area: a `Protocol`'s tag, an `Int`, or a `Str`.
#[repr(C)]
union BuriServePayload {
    /// `.Speak` — a payload-free enum is a bare integer (§6's first niche), so
    /// `Protocol`'s payload *is* its variant index in one byte.
    protocol: i8,
    /// `.DrainMillis` — an `Int`, which is eight bytes at the payload area's
    /// own offset. It changes nothing about the size: a `Str` was already the
    /// widest thing here, and F4's argument that the whole is 32 bytes is what
    /// a fifth variant would have to move rather than a fourth.
    millis: i64,
    /// `.Certificate` and `.PrivateKey`.
    text: std::mem::ManuallyDrop<BuriStr>,
}

/// The `[Serve]` list, decoded into the plan the acceptor keeps.
///
/// # Safety
/// `ptr` must address `len` live `Serve` values.
unsafe fn plan_of(ptr: *const u8, len: u64) -> Result<ServePlan, ServeErr> {
    let mut plan = ServePlan::default();
    if ptr.is_null() || len == 0 {
        return Ok(plan);
    }
    for index in 0..len as usize {
        // SAFETY: the caller promises `len` live elements, and the stride is
        // the one the layout gives `Serve`.
        let element = unsafe { &*ptr.cast::<BuriServe>().add(index) };
        match element.tag {
            // SAFETY: the tag says which arm of the union is live.
            0 => plan.protocols.push(protocol_of(i64::from(unsafe { element.payload.protocol }))?),
            // SAFETY: as above, and an element of a live list holds live `Str`
            // views.
            1 => {
                plan.certificate =
                    Some(PathBuf::from(unsafe { element.payload.text.as_str() }.into_owned()));
            }
            // SAFETY: as above.
            2 => {
                plan.key =
                    Some(PathBuf::from(unsafe { element.payload.text.as_str() }.into_owned()));
            }
            // SAFETY: the tag says the payload area holds an `Int`. A negative
            // number is `.None` reaching here the long way — nothing in
            // `core/net/server` renders one, since the field is an `Option` and
            // an absent option is an absent element — so it is read as "chose
            // nothing" rather than as a deadline in the past.
            3 => {
                let millis = unsafe { element.payload.millis };
                plan.drain = (millis >= 0).then(|| Duration::from_millis(millis as u64));
            }
            // SAFETY: the tag says the payload area holds an `Int`. Zero and
            // below are read as "chose nothing" for `.DrainMillis`'s reason and
            // for one of its own: a socket whose buffer holds no messages is a
            // socket that closes on its first `send`, which is a configuration
            // nobody means.
            4 => {
                let messages = unsafe { element.payload.millis };
                plan.socket_buffer = (messages > 0).then_some(messages as usize);
            }
            // Not defensive: a program this toolchain built cannot produce a
            // sixth tag, so this is what an older program linked against a
            // newer runtime — or the reverse — is told, and the answer to that
            // is a refusal rather than a guess.
            unknown => {
                return Err(ServeErr::new(
                    ServeFail::Unsupported,
                    format!(
                        "this runtime does not know serve option {unknown}: the program and the \
                         runtime disagree about `Serve`'s variants"
                    ),
                ));
            }
        }
    }
    Ok(plan)
}

/// A `Protocol`'s variant index, or the refusal for one no variant has.
fn protocol_of(index: i64) -> Result<Protocol, ServeErr> {
    Protocol::from_index(index).ok_or_else(|| {
        ServeErr::new(
            ServeFail::Unsupported,
            format!(
                "this runtime does not know protocol {index}: the program and the runtime \
                 disagree about `Protocol`'s variants"
            ),
        )
    })
}

/// `Request` — `{ method: Method, url: Str, headers: [Header], body: [U8] }`.
///
/// `Method` is a seven-variant enum with no payloads, which `middle/layout.rs`
/// gives a bare `i8` tag: the value *is* the index. `url` therefore starts at
/// offset 8 and not at 1, which is `#[repr(C)]`'s own padding rule and the
/// value model's alignment rule agreeing.
#[repr(C)]
pub struct BuriRequest {
    method: i8,
    url: BuriStr,
    headers: BuriList,
    body: BuriList,
}

/// `Response` — `{ status: Int, headers: [Header], body: [U8] }`.
#[repr(C)]
pub struct BuriResponse {
    status: i64,
    headers: BuriList,
    body: BuriList,
}

/// `ServeError` — `{ cause: ServeFailure, detail: Str }`, `Method`'s tag rule
/// applied to a seven-variant enum a second time.
///
/// It crosses as a value rather than as a discriminant because it is a
/// **struct**: `lib.rs` §2.1's second shape gives a non-enum error its own
/// out-pointer, so the platform's own words reach the program whole. That is
/// the whole reason `ServeError` is a struct wrapping an enum instead of an
/// enum with payloads, and `effect.buri` says so where it is declared.
#[repr(C)]
pub struct BuriServeError {
    cause: i8,
    detail: BuriStr,
}

impl BuriServeError {
    fn of(e: &ServeErr) -> BuriServeError {
        BuriServeError { cause: e.cause.tag(), detail: str_of(&e.detail) }
    }
}

/// `Listen::listenBind(…) -> Result<Listener, ServeError>`.
///
/// Ten integer parameters, which is exactly the budget a runtime call has
/// (`backend/stencil/abi.rs`'s `MAX_INT_ARGS`): the address's three `Str`
/// leaves, the port, the plan list's `(ptr, len)`, two `Int` knobs, and
/// `Result`'s two out-pointers. An empty address, an empty plan and a negative
/// knob are all "the caller chose nothing" — the encoding `core/net/server`
/// renders a `Server`'s `.None` fields into, and the reason [`HEAD_DEADLINE`]
/// and [`BODY_LIMIT`] are this file's constants rather than two more
/// parameters.
///
/// **The plan is a `[Serve]` and not a `[Protocol]`, and that is what let TLS
/// land without a table row.** A certificate and a key are two `Str`s and
/// therefore six more integers on a call that had none to give; the list
/// argument crosses as a pointer and a length whatever its elements are, so
/// making the elements tagged options bought six words of room at the cost of
/// a variant. [`ServePlan`] is where the rest of that argument is.
///
/// # Safety
/// The address view and the `[Serve]` must be live; both out-pointers
/// writable and aligned.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_listen_bind(
    _abase: *mut u8,
    aptr: *const u8,
    alen: u64,
    port: i64,
    pptr: *const u8,
    plen: u64,
    limit: i64,
    idle: i64,
    out: *mut BuriListener,
    err: *mut BuriServeError,
) -> i32 {
    // SAFETY: forwarded.
    let address = unsafe { crate::host::text(aptr, alen) };
    // SAFETY: forwarded — the caller promises `plen` live `Serve` values.
    let plan = match unsafe { plan_of(pptr, plen) } {
        Ok(plan) => plan,
        Err(e) => {
            // SAFETY: the caller promises a writable destination.
            unsafe { err.write(BuriServeError::of(&e)) };
            return 0;
        }
    };
    // **A suspension point** (`rt.rs` §2), for `buri_rt_host_net_fetch`'s
    // reason: the bind is a syscall that can block on name resolution, and a
    // carrier is not the caller's to lose. The Buri blocks below are built
    // after the park returns, on the carrier and under the baton.
    let outcome = park(|| bind(&address, port, &plan, limit, idle));
    match outcome {
        Ok((handle, bound, handlers)) => {
            // SAFETY: the caller promises a writable destination.
            unsafe {
                out.write(BuriListener { handle, port: i64::from(bound), handlers })
            };
            crate::BURI_OK
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

/// `Listen::listenAccept(listener) -> Result<Int, ServeError>`.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_accept(
    handle: i64,
    out: *mut i64,
    err: *mut BuriServeError,
) -> i32 {
    // **A suspension point**, and the one that waits longest: this is where a
    // worker sits between requests.
    match park(|| accept(handle)) {
        Ok(connection) => {
            // SAFETY: the caller promises a writable destination.
            unsafe { out.write(connection) };
            crate::BURI_OK
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

/// `Listen::listenRequest(connection) -> Result<Request, ServeError>`.
///
/// No `park`: the request was read off the socket before the connection was
/// handed out, so this waits on nothing. It is the one `Listen` entry that
/// allocates Buri blocks and does not wait.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_request(
    connection: i64,
    out: *mut BuriRequest,
    err: *mut BuriServeError,
) -> i32 {
    match request(connection) {
        Ok(request) => {
            let value = BuriRequest {
                method: i8::try_from(request.method).unwrap_or(0),
                url: str_of(&request.target),
                headers: list_of_headers(&request.headers),
                body: list_of_bytes(&request.body),
            };
            // SAFETY: the caller promises a writable destination.
            unsafe { out.write(value) };
            crate::BURI_OK
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

/// `Listen::listenRespond(connection, response) -> Result<(), ServeError>`.
///
/// `Response`'s three fields arrive flattened, and `.Ok`'s payload is `()` — so
/// there is no out-pointer for it, on `lib.rs` §2.1's zero-sized rule.
///
/// The first argument is the **connection** `listenAccept` answered with and no
/// longer the listener: with a worker per handler there is one outstanding
/// request per worker, so a listener no longer names one of them.
///
/// # Safety
/// The `[Header]` and the `[U8]` must be live; `err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_respond(
    connection: i64,
    status: i64,
    hptr: *const u8,
    hlen: u64,
    bptr: *const u8,
    blen: u64,
    err: *mut BuriServeError,
) -> i32 {
    // SAFETY: forwarded.
    let fields = unsafe { crate::host::headers(hptr, hlen) };
    let body: &[u8] = if bptr.is_null() || blen == 0 {
        &[]
    } else {
        // SAFETY: the caller promises `blen` readable bytes; a `[U8]`'s stride
        // is one, so the payload is the bytes themselves.
        unsafe { std::slice::from_raw_parts(bptr, blen as usize) }
    };
    // **A suspension point**: writing a response is a write to a socket the
    // peer may be reading slowly.
    match park(|| respond(connection, status, &fields, body)) {
        Ok(()) => crate::BURI_OK,
        Err(e) => {
            // SAFETY: the caller promises a writable destination.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

/// `Listen::listenClose(listener)`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_listen_close(handle: i64) {
    close(handle);
}

/// `Received` — `{ frame: Frame, text: Str, data: [U8], code: Int }`.
///
/// `Frame` is a three-variant enum with no payloads, which `middle/layout.rs`
/// gives a bare `i8` tag, so `text` starts at 8 and not at 1 — `BuriRequest`'s
/// rule one screen up, and `#[repr(C)]`'s own padding agreeing with the value
/// model's alignment. `the_transcribed_shapes_match_the_value_model` asserts
/// every offset of it.
#[repr(C)]
pub struct BuriReceived {
    frame: i8,
    text: BuriStr,
    data: BuriList,
    code: i64,
}

/// `Listen::listenUpgrade(connection) -> Result<Int, ServeError>`.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_upgrade(
    connection: i64,
    out: *mut i64,
    err: *mut BuriServeError,
) -> i32 {
    // **A suspension point**: the handshake response is a write to a socket
    // whose peer may be reading slowly, on `listenRespond`'s reasoning.
    match park(|| upgraded(connection)) {
        Ok(socket) => {
            // SAFETY: the caller promises a writable destination.
            unsafe { out.write(socket) };
            crate::BURI_OK
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

#[cfg(feature = "net")]
fn upgraded(connection: i64) -> Result<i64, ServeErr> {
    sockets::upgrade(connection)
}

/// With the feature off there is no RFC 6455 framing in the archive, so the
/// true answer is about how the toolchain was built. It reaches
/// `core/net/server` as "this is not an upgrade" either way — every `.Err` from
/// this call does — so a `net`-off server answers the request in `onRequest`,
/// which is the same thing a server with no hooks does.
#[cfg(not(feature = "net"))]
fn upgraded(_connection: i64) -> Result<i64, ServeErr> {
    Err(ServeErr::new(ServeFail::Unsupported, SOCKETS_OFF))
}

/// `Listen::listenReceive(socket) -> Result<Received, ServeError>`.
///
/// **A suspension point, and the second one in this file that can wait
/// forever.** A socket between messages is a worker between messages, exactly
/// as a listener between connections is, and `park_on` is what keeps it from
/// being a carrier between messages too.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_receive(
    socket: i64,
    out: *mut BuriReceived,
    err: *mut BuriServeError,
) -> i32 {
    match park(|| received(socket)) {
        Ok(event) => {
            let value = BuriReceived {
                frame: event.frame,
                text: str_of(&event.text),
                data: list_of_bytes(&event.data),
                code: event.code,
            };
            // SAFETY: the caller promises a writable destination.
            unsafe { out.write(value) };
            crate::BURI_OK
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

#[cfg(feature = "net")]
fn received(socket: i64) -> Result<Received, ServeErr> {
    sockets::receive(socket)
}

#[cfg(not(feature = "net"))]
fn received(_socket: i64) -> Result<Received, ServeErr> {
    Err(ServeErr::closed())
}

// The three `Sockets` entries. A handle that names no open socket is one that
// has already gone away — which is the behaviour `effect Sockets` declares for
// a closed socket, so a handle a program made up and a handle it has spent are
// deliberately the same answer, and all three are total.
//
// None of them touches the socket itself. A message goes onto the socket's own
// outbound queue and a byte goes into its wakeup pipe, and the socket's own
// worker does the writing — which is what `socketSendText`'s "never waits"
// means and what keeps one thread inside one framing.

/// `Sockets::socketSendText(socket, text)`.
///
/// # Safety
/// The text view must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_sockets_socket_send_text(
    socket: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded.
    let text = unsafe { crate::host::text(ptr, len) };
    enqueue_text(socket, &text);
}

/// `Sockets::socketSendBytes(socket, body)`.
///
/// # Safety
/// The `[U8]` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_sockets_socket_send_bytes(
    socket: i64,
    ptr: *const u8,
    len: u64,
) {
    let body: &[u8] = if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller promises `len` readable bytes; a `[U8]`'s stride
        // is one, so the payload is the bytes themselves.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    };
    enqueue_bytes(socket, body);
}

/// `Sockets::socketClose(socket, code, reason)`.
///
/// # Safety
/// The reason view must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_sockets_socket_close(
    socket: i64,
    code: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded.
    let reason = unsafe { crate::host::text(ptr, len) };
    finish(socket, code, &reason);
}

#[cfg(feature = "net")]
fn enqueue_text(socket: i64, text: &str) {
    sockets::send_text(socket, text);
}

#[cfg(feature = "net")]
fn enqueue_bytes(socket: i64, body: &[u8]) {
    sockets::send_bytes(socket, body);
}

#[cfg(feature = "net")]
fn finish(socket: i64, code: i64, reason: &str) {
    sockets::close(socket, code, reason);
}

// With the feature off nothing can have minted a socket — `listenUpgrade`
// refuses — so every handle these can be given is one a program made up, and
// dropping what they are handed *is* the declared behaviour for a socket that
// has gone rather than a stub.
#[cfg(not(feature = "net"))]
fn enqueue_text(_socket: i64, _text: &str) {}

#[cfg(not(feature = "net"))]
fn enqueue_bytes(_socket: i64, _body: &[u8]) {}

#[cfg(not(feature = "net"))]
fn finish(_socket: i64, _code: i64, _reason: &str) {}

/// Run one blocking step inside the reactor's context where there is one, and
/// inline where there is not.
///
/// The five `Listen` entries that reach a blocking call all want the same two
/// lines, so they say them once here.
///
/// **It does not park, and the name is older than the mechanism.** `work` is a
/// synchronous call wrapped in an `async` block, so the first poll runs it to
/// completion and answers `Ready` — and `rt::park_on` only gives a carrier back
/// to a future that answers `Pending`. The carrier is therefore held for the
/// whole of the blocking `accept(2)` or `Condvar::wait` underneath. That is not
/// an oversight and [`MAX_HANDLERS`] is the number that prices it: sixty-four
/// carriers may be in exactly this state at once, which is the reason that
/// ceiling is a constant a program can predict rather than a function of the
/// machine, and it is stated there in as many words.
///
/// **What routing through `park_on` buys is the reactor and the seam.** The
/// work runs under `Handle::enter` on a carrier and under `Handle::block_on`
/// off one, so a body that reaches for a tokio resource without naming a handle
/// finds a runtime; and this is the one function — five callers, two lines —
/// where a real suspension goes on the day these entries stop being blocking
/// calls, with no caller moving. Neither is load-bearing today: everything
/// below names [`crate::rt::handle`] explicitly, so what this is at present is
/// the seam and an honest name for where the waiting happens.
///
/// The doc this replaces said `park_on` kept the carrier from "holding the
/// baton while a server waits for a client". There is no baton — G3 deleted it
/// (`rt.rs` §1) — and on the mechanism that replaced it the sentence was the
/// opposite of what these two lines do.
fn park<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    #[cfg(feature = "net")]
    {
        crate::rt::park_on(async { work() })
    }
    #[cfg(not(feature = "net"))]
    {
        work()
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

    /// The acceptor speaks HTTP/1.1 and HTTP/2 and says so about the third.
    ///
    /// [`serves`] is about the *archive* — was the QUIC code compiled in — and
    /// [`accepts`] is about this file. A user can act on the first (rebuild the
    /// toolchain) and can only wait for the second, so HTTP/3 on a toolchain
    /// without `net-h3` has to produce the first message and not the second.
    ///
    /// **HTTP/2 moved across this line in F4** and its remaining condition is
    /// not a protocol question but a transport one: `accepts` says yes, and
    /// `bind` is where a plan asking for it without a certificate is refused.
    #[test]
    fn the_acceptor_speaks_http1_and_http2_and_names_what_it_does_not() {
        assert_eq!(accepts(Protocol::Http1), Ok(()));
        if cfg!(feature = "net") {
            assert_eq!(accepts(Protocol::Http2), Ok(()), "HTTP/2 is spoken where `hyper` is");
        } else {
            let two = accepts(Protocol::Http2).expect_err("no `hyper`, no HTTP/2");
            assert!(two.contains("`net` feature"), "{two}");
        }
        let three = accepts(Protocol::Http3).expect_err("HTTP/3 is not framed here either");
        if cfg!(feature = "net-h3") {
            assert!(three.contains("HTTP/3"), "{three}");
        } else {
            assert!(
                three.contains("BURI_RUNTIME_NET_H3=1"),
                "an h3 refusal on a toolchain with no QUIC should name the switch: {three}"
            );
        }
    }

    /// **The transcribed shapes are the value model's**, checked one size and
    /// one offset at a time.
    ///
    /// `BuriRequest`, `BuriResponse`, `BuriListener` and `BuriServeError` are
    /// hand-written transcriptions of four Buri declarations, and the C ABI has
    /// no way to notice a disagreement — a runtime writing a `Str` eight bytes
    /// early is a program reading a pointer out of a tag. So the numbers are
    /// asserted here, on this side, against what VALUE-MODEL.md §5's layout
    /// gives those declarations: fields back to back at their own alignments,
    /// in declaration order, with a payload-free enum occupying its `i8` tag.
    /// The compiler states the same numbers from the other side by *being* the
    /// layout, and `cli/tests/native/` runs a request through both.
    #[test]
    fn the_transcribed_shapes_match_the_value_model() {
        let str_bytes = std::mem::size_of::<BuriStr>();
        let list_bytes = std::mem::size_of::<BuriList>();
        assert_eq!(str_bytes, 24, "a `Str` is `{{ base, ptr, len }}` (§3)");
        assert_eq!(list_bytes, 16, "a `[T]` is `{{ ptr, len }}` (§4)");

        // `Request { method: Method, url: Str, headers: [Header], body: [U8] }`.
        // `Method` has seven payload-free variants, so its layout is a bare
        // `i8` tag — one byte, aligned to one — and `url` starts at the next
        // multiple of eight rather than at offset 1.
        assert_eq!(std::mem::size_of::<BuriRequest>(), 64);
        assert_eq!(std::mem::align_of::<BuriRequest>(), 8);
        assert_eq!(std::mem::offset_of!(BuriRequest, method), 0);
        assert_eq!(std::mem::offset_of!(BuriRequest, url), 8);
        assert_eq!(std::mem::offset_of!(BuriRequest, headers), 32);
        assert_eq!(std::mem::offset_of!(BuriRequest, body), 48);

        // `Response { status: Int, headers: [Header], body: [U8] }`.
        assert_eq!(std::mem::size_of::<BuriResponse>(), 40);
        assert_eq!(std::mem::offset_of!(BuriResponse, status), 0);
        assert_eq!(std::mem::offset_of!(BuriResponse, headers), 8);
        assert_eq!(std::mem::offset_of!(BuriResponse, body), 24);

        // `Listener { handle: Int, port: Int, handlers: Int }`.
        assert_eq!(std::mem::size_of::<BuriListener>(), 24);
        assert_eq!(std::mem::offset_of!(BuriListener, port), 8);
        assert_eq!(std::mem::offset_of!(BuriListener, handlers), 16);

        // `ServeError { cause: ServeFailure, detail: Str }`, `Method`'s tag
        // rule a second time.
        assert_eq!(std::mem::size_of::<BuriServeError>(), 32);
        assert_eq!(std::mem::offset_of!(BuriServeError, cause), 0);
        assert_eq!(std::mem::offset_of!(BuriServeError, detail), 8);

        // `Received { frame: Frame, text: Str, data: [U8], code: Int }`.
        // `Frame` has three payload-free variants, so `BuriRequest`'s tag rule
        // a third time: one byte at zero, and `text` at 8 rather than at 1.
        assert_eq!(std::mem::size_of::<BuriReceived>(), 56);
        assert_eq!(std::mem::align_of::<BuriReceived>(), 8);
        assert_eq!(std::mem::offset_of!(BuriReceived, frame), 0);
        assert_eq!(std::mem::offset_of!(BuriReceived, text), 8);
        assert_eq!(std::mem::offset_of!(BuriReceived, data), 32);
        assert_eq!(std::mem::offset_of!(BuriReceived, code), 48);

        // `Serve { Speak(Protocol), Certificate(Str), PrivateKey(Str),
        // DrainMillis(Int), SocketBuffer(Int) }` — §6's `tag ++ payload`, and
        // the first
        // payload-carrying enum this runtime reads. The tag is one byte because
        // four variants fit in one; the payload area starts at 8 because a
        // `Str` is eight-aligned; the whole is 32 because that is
        // `align_up(8 + 24, 8)`. A `[Serve]`'s stride is that size, so getting
        // it wrong reads every element after the first from the wrong address.
        //
        // **F5 added a fourth variant and F7 a fifth, and none of those
        // numbers moved**, which is what the extension point being free means:
        // an `Int` payload is eight bytes in an area that already held
        // twenty-four, and a fifth tag is a fifth value of a byte that already
        // had two hundred and fifty-six. Two knobs that could not have crossed
        // the bind's ten-integer budget at all have now cost nothing between
        // them.
        assert_eq!(std::mem::size_of::<BuriServe>(), 32);
        assert_eq!(std::mem::align_of::<BuriServe>(), 8);
        assert_eq!(std::mem::offset_of!(BuriServe, tag), 0);
        assert_eq!(std::mem::offset_of!(BuriServe, payload), 8);
        // And the two arms of the payload area start at the same place, which
        // is what "one payload area, read according to the tag" means.
        assert_eq!(std::mem::size_of::<BuriServePayload>(), str_bytes);
        assert_eq!(std::mem::align_of::<BuriServePayload>(), 8);
        assert_eq!(std::mem::offset_of!(BuriServe, payload) + std::mem::size_of::<i64>(), 16);
    }

    /// A `[Serve]` decodes into the plan the acceptor keeps, tag by tag.
    ///
    /// Built here as the bytes a generated program would pass rather than
    /// through a constructor, because what is under test is the *transcription*:
    /// the offsets above are only right if reading them back gives the values
    /// that were written.
    #[test]
    fn a_serve_plan_crosses_as_a_list_of_tagged_options() {
        let certificate = String::from("/etc/ssl/leaf.pem");
        let key = String::from("/etc/ssl/leaf.key");
        let items = [
            BuriServe { tag: 0, payload: BuriServePayload { protocol: Protocol::Http1 as i8 } },
            BuriServe { tag: 0, payload: BuriServePayload { protocol: Protocol::Http2 as i8 } },
            BuriServe {
                tag: 1,
                payload: BuriServePayload {
                    text: std::mem::ManuallyDrop::new(borrowed(&certificate)),
                },
            },
            BuriServe {
                tag: 2,
                payload: BuriServePayload { text: std::mem::ManuallyDrop::new(borrowed(&key)) },
            },
            BuriServe { tag: 3, payload: BuriServePayload { millis: 2_500 } },
            BuriServe { tag: 4, payload: BuriServePayload { millis: 8 } },
        ];
        // SAFETY: six live elements, at the stride asserted above.
        let plan = unsafe { plan_of(items.as_ptr().cast(), items.len() as u64) }
            .expect("a plan this runtime knows every tag of");
        assert_eq!(plan.protocols, vec![Protocol::Http1, Protocol::Http2]);
        assert_eq!(plan.certificate.as_deref(), Some(std::path::Path::new(&certificate)));
        assert_eq!(plan.key.as_deref(), Some(std::path::Path::new(&key)));
        // The `Int` arm of the same payload area, read back at the same offset
        // a `Str` is read at — which is the claim the union makes and the one
        // that would fail silently if it were wrong.
        assert_eq!(plan.drain, Some(Duration::from_millis(2_500)));
        // The fifth variant, at the same offset again.
        assert_eq!(plan.socket_buffer, Some(8));
        // An empty plan is a caller who chose nothing, and that includes the
        // drain: `None` here is `DRAIN_DEADLINE` at the bind, not zero.
        let empty = unsafe { plan_of(std::ptr::null(), 0) }.expect("nothing is a plan too");
        assert!(empty.protocols.is_empty());
        assert_eq!(empty.drain, None);
        assert_eq!(empty.socket_buffer, None);
        // A negative deadline is `.None` arriving the long way round, and it
        // reads as "chose nothing" rather than as a deadline already past.
        let negative = [BuriServe { tag: 3, payload: BuriServePayload { millis: -1 } }];
        // SAFETY: one live element.
        let negative = unsafe { plan_of(negative.as_ptr().cast(), 1) }.expect("a plan");
        assert_eq!(negative.drain, None);
        // A buffer of zero is read the same way and for a reason of its own: a
        // socket whose queue holds nothing would close on its first `send`,
        // which is not a configuration anybody means.
        let none = [BuriServe { tag: 4, payload: BuriServePayload { millis: 0 } }];
        // SAFETY: one live element.
        let none = unsafe { plan_of(none.as_ptr().cast(), 1) }.expect("a plan");
        assert_eq!(none.socket_buffer, None);
        // The ALPN offer follows the protocols and their order, most preferred
        // first — which is the whole of how a client comes to be speaking h2 —
        // and a caller who chose nothing offers HTTP/1.1. There is no offer to
        // make on a `net`-off toolchain, which has no TLS to make it in.
        #[cfg(feature = "net")]
        {
            assert_eq!(
                plan.alpn(),
                vec![crate::tls::ALPN_HTTP1.to_vec(), crate::tls::ALPN_H2.to_vec()]
            );
            assert_eq!(empty.alpn(), vec![crate::tls::ALPN_HTTP1.to_vec()]);
        }

        // A tag no variant has is the program and the runtime disagreeing, and
        // is refused rather than read as one of the three.
        let unknown =
            [BuriServe { tag: 7, payload: BuriServePayload { protocol: 0 } }];
        // SAFETY: one live element.
        let refused = unsafe { plan_of(unknown.as_ptr().cast(), 1) }
            .expect_err("there is no eighth serve option");
        assert_eq!(refused.cause, ServeFail::Unsupported);
        assert!(refused.detail.contains("disagree"), "{}", refused.detail);
    }

    /// **A drain answers what the listener already took, and only then closes
    /// it** — which is the whole of what "graceful" means here.
    ///
    /// The instrument is the ordering: the request is accepted *before* the
    /// drain begins and answered *after* it, so a drain that behaved like
    /// [`close`] would have dropped the connection and the client would read an
    /// empty reply. What it must do instead is wait, and the wait is what the
    /// drain thread's `true` reports.
    ///
    /// The second client is the other half of the same claim: once the drain
    /// has returned, the acceptor thread is gone — it cannot be otherwise,
    /// since it reserves its place in `outstanding` before it accepts and the
    /// drain waited for that number to reach zero — so a connection made
    /// afterwards is never answered.
    #[test]
    fn a_drain_answers_the_request_it_took_and_then_stops_the_listener() {
        let plan = ServePlan {
            protocols: vec![Protocol::Http1],
            certificate: None,
            key: None,
            drain: Some(Duration::from_secs(10)),
            socket_buffer: None,
        };
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.write_all(b"GET /inflight HTTP/1.1\r\nhost: h\r\n\r\n").expect("request");
            socket.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = socket.read_to_end(&mut reply);
            reply
        });

        let connection = accept(handle).expect("one connection");
        // From here the listener is being drained on another thread, and this
        // one is the request in flight it has to wait for.
        let draining = std::thread::spawn(move || drain(handle));
        let read = request(connection).expect("the request survives the drain");
        assert_eq!(read.target, "/inflight");
        respond(connection, 200, &[], b"finished").expect("a response");
        assert!(
            draining.join().expect("the drain finished"),
            "the drain timed out rather than waiting for the request it had taken"
        );

        let reply =
            String::from_utf8_lossy(&client.join().expect("the client finished")).to_string();
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.ends_with("\r\n\r\nfinished"), "{reply}");

        // And now the listener says it will answer no more, which is the
        // `.Closed` `core/net/server`'s `run` turns into `.Ok(())`.
        assert_eq!(accept(handle).expect_err("drained").cause, ServeFail::Closed);

        // A client that arrives after the drain is not answered. The port is
        // still open — `run` closes it, one line further along — so the connect
        // succeeds and nothing comes back.
        let mut late = TcpStream::connect(("127.0.0.1", port)).expect("the port is still open");
        late.set_read_timeout(Some(Duration::from_millis(500))).expect("a deadline");
        late.set_write_timeout(Some(Duration::from_millis(500))).expect("a deadline");
        let _ = late.write_all(b"GET /late HTTP/1.1\r\nhost: h\r\n\r\n");
        let mut nothing = Vec::new();
        let _ = late.read_to_end(&mut nothing);
        assert!(nothing.is_empty(), "a drained listener answered a new connection: {nothing:?}");

        close(handle);
    }

    /// **The drain deadline bounds the waiting, and the waiting only.**
    ///
    /// A request that is accepted and never answered is the worst case a
    /// shutdown has — a handler that will not return — and this is what it
    /// costs: the drain gives up after the number the plan named, the workers
    /// are told the listener is closed, and the process is free to end.
    ///
    /// A hundred and fifty milliseconds so that the row is fast, and the
    /// assertion on the elapsed time is two-sided: it waited at least its
    /// deadline (a drain that returned at once would not have waited for
    /// anything) and it stopped well inside a bound (a drain that ignored its
    /// deadline would sit here until the head deadline thirty seconds away).
    #[test]
    fn a_drain_gives_up_on_a_request_nobody_answers() {
        let plan = ServePlan {
            protocols: vec![Protocol::Http1],
            certificate: None,
            key: None,
            drain: Some(Duration::from_millis(150)),
            socket_buffer: None,
        };
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.write_all(b"GET /abandoned HTTP/1.1\r\nhost: h\r\n\r\n").expect("request");
            socket.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = socket.read_to_end(&mut reply);
            reply
        });

        let connection = accept(handle).expect("one connection");
        assert_eq!(request(connection).expect("its request").target, "/abandoned");
        // And nothing answers it.
        let started = Instant::now();
        assert!(!drain(handle), "the drain claimed to have drained a request nobody answered");
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(150), "the drain did not wait: {waited:?}");
        assert!(waited < Duration::from_secs(10), "the deadline did not bound it: {waited:?}");

        assert_eq!(accept(handle).expect_err("drained").cause, ServeFail::Closed);
        // Giving the port back is what drops the connection nobody answered,
        // and the client sees the socket close rather than a response.
        close(handle);
        let reply = client.join().expect("the client finished");
        assert!(reply.is_empty(), "a request nobody answered was answered anyway: {reply:?}");
    }

    /// **`SIGINT` and `SIGTERM` are this runtime's exactly while a port is
    /// open.**
    ///
    /// The disposition is process-wide state, so what can be asserted here
    /// without racing every other row in this file is the half that holds a
    /// listener open across it: while *this* test's listener is in the table
    /// the table is not empty, so nothing else can give the signals back.
    ///
    /// It is asserted as an **equality with the handler's own address** rather
    /// than as "not the default", which is what makes it a test of the
    /// disposition rather than of the query: a `sigaction` read at the wrong
    /// offset would answer something that is also not `SIG_DFL`.
    ///
    /// The other half — that the last `close` gives them back — cannot be
    /// asserted from here, because another row's listener may be open at the
    /// same moment. It is asserted where a process is the unit:
    /// `cli/tests/native`'s signal rows.
    #[test]
    fn a_bound_listener_holds_the_shutdown_signals() {
        let (handle, _port, _handlers) =
            bind("127.0.0.1", 0, &ServePlan::default(), -1, 60).expect("a bound port");
        let ours = shutdown::handler_address();
        assert_eq!(disposition(shutdown::SIGTERM), ours, "SIGTERM is not this runtime's");
        assert_eq!(disposition(shutdown::SIGINT), ours, "SIGINT is not this runtime's");
        close(handle);
    }

    /// A signal's current handler, read without changing it.
    ///
    /// `sigaction` with a null `act` is the only query POSIX gives — `signal`
    /// answers the previous disposition but sets one on the way past, which two
    /// rows binding at once would race on. The struct's layout differs between
    /// the two platforms in everything *after* the first field, and the first
    /// field is the handler on both, so what is read here is one pointer out of
    /// a buffer that is comfortably larger than either.
    fn disposition(sig: i32) -> usize {
        #[repr(C, align(16))]
        struct Sigaction([u8; 256]);

        unsafe extern "C" {
            fn sigaction(sig: i32, act: *const u8, old: *mut u8) -> i32;
        }

        let mut out = Sigaction([0u8; 256]);
        // SAFETY: a null `act` reads without writing, and the destination is
        // larger than `struct sigaction` on either platform.
        let rc = unsafe { sigaction(sig, std::ptr::null(), (&raw mut out.0).cast()) };
        assert_eq!(rc, 0, "sigaction could not be asked about signal {sig}");
        // SAFETY: `sa_handler` is the first member on both platforms, and the
        // buffer is aligned for it.
        unsafe { (&raw const out.0).cast::<usize>().read() }
    }

    /// A `Str` view over a Rust `String` that owns nothing — what a generated
    /// program's `Str` looks like to a runtime that only reads it.
    fn borrowed(text: &str) -> BuriStr {
        BuriStr {
            base: std::ptr::null_mut(),
            ptr: text.as_ptr(),
            len: text.len() as u64,
        }
    }

    /// `ServeFailure`'s variant indices, which are what the `.Err` arm carries
    /// across, in `effect.buri`'s declaration order.
    ///
    /// Written out rather than derived, because the order *is* the contract and
    /// a derivation would agree with whatever it was derived from.
    #[test]
    fn a_serve_failure_tag_is_its_declaration_order() {
        assert_eq!(ServeFail::AddressInUse.tag(), 0);
        assert_eq!(ServeFail::AddressNotAvailable.tag(), 1);
        assert_eq!(ServeFail::PermissionDenied.tag(), 2);
        assert_eq!(ServeFail::Unsupported.tag(), 3);
        assert_eq!(ServeFail::Timeout.tag(), 4);
        assert_eq!(ServeFail::Closed.tag(), 5);
        assert_eq!(ServeFail::Transport.tag(), 6);
    }

    /// A request line, its headers and its body, off the wire and back.
    ///
    /// The parser is fed a whole request through a loopback socket rather than
    /// a `&[u8]`, because what is being checked is the *read loop* as much as
    /// the parse: a head that arrives in two writes, a `content-length` body
    /// that arrives after it, and a deadline on both.
    #[test]
    fn a_request_is_read_off_a_socket_whole() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        // Every wait here is bounded, and the test joins the writer, so a
        // client that never connects is a failing assertion rather than a suite
        // that runs until CI kills it.
        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_write_timeout(Some(Duration::from_secs(10))).expect("a deadline");
            socket.write_all(b"POST /a/b?x=1 HTTP/1.1\r\nHost: h\r\nContent-Length").expect("head");
            socket.write_all(b": 5\r\nX-Mixed-Case: Yes\r\n\r\nhello").expect("rest");
            socket.flush().expect("flush");
            socket
        });
        let (mut accepted, _) = listener.accept().expect("accept");
        deadlines(&accepted, Duration::from_secs(10)).expect("deadlines");
        let request = read_request(&mut accepted, 1024, Instant::now() + Duration::from_secs(10))
            .expect("a whole request");
        let _client = client.join().expect("the writer finished");

        assert_eq!(crate::http::method_name(request.method), "POST");
        assert_eq!(request.target, "/a/b?x=1");
        assert_eq!(request.body, b"hello");
        // Lowercased on the way in, which is what `Header`'s declaration
        // promises: `request.header("x-mixed-case")` is an exact comparison.
        assert!(
            request.headers.iter().any(|(n, v)| n == "x-mixed-case" && v == "Yes"),
            "{:?}",
            request.headers
        );
    }

    /// Four shapes the acceptor answers itself, so that a handler is never
    /// handed a message the framing could not make sense of.
    fn refused_status(head: &[u8], limit: usize) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        let head = head.to_vec();
        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_write_timeout(Some(Duration::from_secs(10))).expect("a deadline");
            let _ = socket.write_all(&head);
            let _ = socket.flush();
            socket
        });
        let (mut accepted, _) = listener.accept().expect("accept");
        deadlines(&accepted, Duration::from_secs(10)).expect("deadlines");
        let status =
            read_request(&mut accepted, limit, Instant::now() + Duration::from_secs(10))
                .unwrap_err();
        let _client = client.join().expect("the writer finished");
        status
    }

    #[test]
    fn the_acceptor_answers_a_request_it_cannot_serve() {
        assert_eq!(refused_status(b"not a request line\r\n\r\n", 1024), 400);
        assert_eq!(refused_status(b"GET / HTTP/2.0\r\n\r\n", 1024), 505);
        assert_eq!(refused_status(b"BREW / HTTP/1.1\r\n\r\n", 1024), 501);
        assert_eq!(refused_status(b"POST / HTTP/1.1\r\ncontent-length: 99\r\n\r\n", 8), 413);
        // A framing this acceptor does not drive is refused rather than
        // guessed at: a body read with the wrong framing belongs to the next
        // request.
        assert_eq!(
            refused_status(b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n", 1024),
            501
        );
    }

    /// A client that sends one byte at a time and never stops.
    ///
    /// The sleep is what makes it a *slowloris* rather than a fast loop: each
    /// read succeeds, well inside any `SO_RCVTIMEO`, so the socket's own
    /// deadline is restarted by every one of them and never fires. `head`
    /// is handed over whole first, so the same reader drips a request line or
    /// drips a body depending on what it is given.
    struct Drip {
        head: Vec<u8>,
        every: Duration,
    }

    impl Read for Drip {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.head.is_empty() {
                let n = self.head.len().min(buffer.len());
                buffer.get_mut(..n).unwrap_or(&mut []).copy_from_slice(&self.head[..n]);
                self.head.drain(..n);
                return Ok(n);
            }
            std::thread::sleep(self.every);
            // A header byte, so the buffer grows and the terminator never
            // arrives — the shape a slowloris actually sends.
            if let Some(first) = buffer.first_mut() {
                *first = b'x';
            }
            Ok(1)
        }
    }

    /// How long a bounded read may take before the bound is what failed.
    const PROMPTLY: Duration = Duration::from_secs(5);

    /// **A drip is bounded by the budget, and nothing else here bounds it.**
    ///
    /// Two cases in one, because the head loop and the body loop are two loops
    /// with the same hole in them.
    ///
    /// The instrument is the arithmetic. `SO_RCVTIMEO` bounds one `read(2)`;
    /// what bounds the *loop* without a budget is the byte cap, and the byte
    /// caps are 64 KiB of header and 8 MiB of body. At a byte every two
    /// milliseconds that is two minutes and four and a half hours
    /// respectively, and at a byte every twenty-nine seconds — which is what a
    /// client that has read `HEAD_DEADLINE` would send — it is three weeks and
    /// seven years. So this case fails the way an unbounded wait fails: it does
    /// not come back. With the budget it is a `408`, which is the status that
    /// was already the answer to "this client stalled".
    #[test]
    fn a_client_that_drips_is_refused_at_the_budget_and_not_at_the_byte_cap() {
        let budget = Duration::from_millis(120);

        let mut head = Drip { head: Vec::new(), every: Duration::from_millis(2) };
        let started = Instant::now();
        let status = read_request(&mut head, 8 * 1024, Instant::now() + budget)
            .expect_err("a request that never ended was accepted");
        assert_eq!(status, 408, "a drip is a stalled client");
        assert!(started.elapsed() < PROMPTLY, "the head drip ran {:?}", started.elapsed());

        // And the same again after a complete, well-formed head: a declared
        // body that arrives one byte at a time is the second loop.
        let mut body = Drip {
            head: b"POST /slow HTTP/1.1\r\nhost: h\r\ncontent-length: 4096\r\n\r\n".to_vec(),
            every: Duration::from_millis(2),
        };
        let started = Instant::now();
        let status = read_request(&mut body, 8 * 1024, Instant::now() + budget)
            .expect_err("a body that never ended was accepted");
        assert_eq!(status, 408);
        assert!(started.elapsed() < PROMPTLY, "the body drip ran {:?}", started.elapsed());
    }

    /// The whole of `bind`, `accept`, `respond` and `close`, against a real
    /// client on a loopback port.
    ///
    /// Every wait is bounded — an idle deadline on the accept, socket
    /// deadlines on the exchange, and a joined writer thread — for the reason
    /// `cli/tests/native/runtime.rs` states about its own mirror of this: a
    /// server test that could hang is a suite that runs until CI kills it.
    #[test]
    fn a_bound_listener_answers_one_request_and_then_says_it_is_closed() {
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &ServePlan::speaking(&[Protocol::Http1]), 1, 10_000).expect("a bound port");
        assert!(port > 0, "port 0 should have been replaced by the one the OS chose");

        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_read_timeout(Some(Duration::from_secs(10))).expect("a deadline");
            socket.set_write_timeout(Some(Duration::from_secs(10))).expect("a deadline");
            socket.write_all(b"GET /ping HTTP/1.1\r\nhost: h\r\n\r\n").expect("request");
            socket.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = socket.read_to_end(&mut reply);
            reply
        });

        let connection = accept(handle).expect("one connection");
        assert!(connection > 0, "a connection id is minted for every accepted request");
        let read = request(connection).expect("the request on it");
        assert_eq!(read.target, "/ping");
        // Read once: the second call is a connection nobody is holding a
        // request for, which is the same `.Closed` a stale connection gets.
        assert_eq!(request(connection).expect_err("read twice").cause, ServeFail::Closed);
        respond(connection, 200, &[("x-from".into(), "buri".into())], b"pong")
            .expect("a response");
        // Answering the same connection twice writes nothing and is not an
        // error: it has been answered, which is a state a loop can be in
        // without having done anything wrong.
        respond(connection, 200, &[], b"again").expect("a second answer is a no-op");

        let reply = String::from_utf8_lossy(&client.join().expect("the client finished")).to_string();
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.contains("x-from: buri\r\n"), "{reply}");
        assert!(reply.contains("content-length: 4\r\n"), "{reply}");
        assert!(reply.ends_with("\r\n\r\npong"), "{reply}");

        // The limit is spent, so the listener says it will answer no more —
        // which is the ordinary end of a bounded server and what
        // `core/net/server`'s `run` turns into `.Ok(())`.
        let closed = accept(handle).expect_err("the request limit is spent");
        assert_eq!(closed.cause, ServeFail::Closed);

        close(handle);
        // And a handle that names nothing is the same answer rather than a
        // crash: closing twice has nothing to do, and a stale handle is a
        // lookup that misses.
        close(handle);
        assert_eq!(accept(handle).expect_err("closed").cause, ServeFail::Closed);
    }

    /// The idle deadline fires, and it is a `Timeout` rather than a hang.
    ///
    /// This is the knob every test in this repository sets, and it is the one
    /// that makes a server testable at all: without it the accept below waits
    /// for a client that is never coming.
    #[test]
    fn an_idle_listener_gives_up_when_it_was_told_to() {
        let (handle, port, _handlers) = bind("127.0.0.1", 0, &ServePlan::default(), -1, 60).expect("a bound port");
        assert!(port > 0);
        let started = Instant::now();
        let timed_out = accept(handle).expect_err("nobody connected");
        assert_eq!(timed_out.cause, ServeFail::Timeout);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline did not bound the wait: {:?}",
            started.elapsed()
        );
        assert!(timed_out.detail.contains("60"), "the message names no deadline: {}", timed_out.detail);
        close(handle);
    }

    /// **A client that connects and then thinks about it is still served**, and
    /// this is the regression test for a real flake.
    ///
    /// The three hundred milliseconds are the whole instrument: they guarantee
    /// the accept wins the race against the client's first write, which is the
    /// state F3's acceptor could not survive on macOS. It made the listener
    /// non-blocking whenever the caller set an idle deadline; a BSD kernel hands
    /// an accepted socket the listener's `O_NONBLOCK`; a non-blocking socket
    /// ignores `SO_RCVTIMEO`; so the first read answered `WouldBlock`,
    /// `read_request` read that as a stalled client, and the connection was
    /// answered `408` and dropped. On CI that presented as *another* accept
    /// waiting out its twenty-second idle deadline, about one macOS run in
    /// three, with nothing in the log connecting the two.
    ///
    /// It is written as a *sleep* rather than as a hammer because the race it
    /// pins has a side that always loses: with the pause, the old code fails
    /// every time and the new code passes every time. [`deadlines`] is where the
    /// property is established and where the whole of that story is written
    /// down.
    #[test]
    fn a_client_that_pauses_before_writing_is_still_framed() {
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &ServePlan::speaking(&[Protocol::Http1]), 1, 20_000)
                .expect("a bound port");
        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            // Long enough that the acceptor is certainly waiting on this socket
            // before a byte of the request exists.
            std::thread::sleep(Duration::from_millis(300));
            socket.write_all(b"GET /late HTTP/1.1\r\nhost: h\r\n\r\n").expect("request");
            socket.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = socket.read_to_end(&mut reply);
            String::from_utf8_lossy(&reply).into_owned()
        });

        let connection = accept(handle).expect("the connection, framed after the pause");
        let read = request(connection).expect("the request on it");
        assert_eq!(read.target, "/late", "a paused client was answered somebody else's request");
        respond(connection, 200, &[], b"late").expect("a response");
        let reply = client.join().expect("the client finished");
        assert!(
            reply.starts_with("HTTP/1.1 200 OK\r\n"),
            "a client that paused before writing was refused rather than read:\n{reply}"
        );
        assert!(reply.ends_with("\r\n\r\nlate"), "{reply}");
        close(handle);
    }

    /// A protocol the acceptor does not frame is refused **at the bind**, with
    /// a message rather than a tag, and no port is held.
    #[test]
    fn a_protocol_this_acceptor_does_not_frame_is_refused_at_the_bind() {
        let refused = bind("127.0.0.1", 0, &ServePlan::speaking(&[Protocol::Http3]), -1, 60)
            .expect_err("HTTP/3 is not framed here");
        assert_eq!(refused.cause, ServeFail::Unsupported);
        if cfg!(feature = "net-h3") {
            assert!(refused.detail.contains("HTTP/3"), "{}", refused.detail);
        } else {
            // A toolchain with no QUIC in it owes the message that names the
            // build switch, not the one about this file — a user can act on the
            // first and can only wait for the second.
            assert!(refused.detail.contains("BURI_RUNTIME_NET_H3=1"), "{}", refused.detail);
        }
        // And an index no variant has is a program and a runtime disagreeing
        // about the enum, which is refused rather than transmuted.
        let unknown = protocol_of(9).expect_err("there is no tenth protocol");
        assert_eq!(unknown.cause, ServeFail::Unsupported);
        assert!(unknown.detail.contains("disagree"), "{}", unknown.detail);
        assert!(protocol_of(-1).is_err(), "a negative index is not a protocol either");
    }

    /// **HTTP/2 without a certificate is refused at the bind**, and not quietly
    /// answered in HTTP/1.1.
    ///
    /// h2 is chosen inside the TLS handshake by ALPN, so a cleartext listener
    /// has nowhere to make the choice. Answering HTTP/1.1 anyway would be a
    /// server not doing what it was configured to do, which is the failure that
    /// gets found in production; the refusal names the field that would fix it.
    ///
    /// `net` only: without the feature there is no HTTP/2 at all, and the
    /// refusal a plan asking for it gets is the one that names the build switch
    /// — which is `the_acceptor_speaks_http1_and_http2_and_names_what_it_does_not`'s
    /// claim rather than this one's.
    #[cfg(feature = "net")]
    #[test]
    fn http2_without_a_certificate_is_refused_at_the_bind() {
        let refused = bind("127.0.0.1", 0, &ServePlan::speaking(&[Protocol::Http2]), -1, 60)
            .expect_err("h2 is negotiated inside a handshake there is none of");
        assert_eq!(refused.cause, ServeFail::Unsupported);
        assert!(refused.detail.contains("ALPN"), "{}", refused.detail);
        assert!(refused.detail.contains("tls"), "{}", refused.detail);
        // And HTTP/1.1 beside it does not rescue it: the plan asked for two
        // protocols and one of them is unreachable.
        let both = ServePlan::speaking(&[Protocol::Http1, Protocol::Http2]);
        assert!(bind("127.0.0.1", 0, &both, -1, 60).is_err(), "one bad protocol is a bad plan");
    }

    /// The bind answers how many handlers it will host, and it is a number a
    /// test can predict.
    ///
    /// The prediction is the point: a program cannot ask for a different one —
    /// `listenBind` has no argument left to carry a preference — so the value of
    /// this constant being a *constant* is what lets a test assert that fifty
    /// requests overlap. [`MAX_HANDLERS`] says why it is sixty-four.
    #[test]
    fn a_bind_answers_the_handler_count_it_will_host() {
        let (handle, _port, handlers) = bind("127.0.0.1", 0, &ServePlan::default(), -1, 60).expect("a bound port");
        assert_eq!(handlers, MAX_HANDLERS);
        assert!(handlers >= 1, "a listener with no handlers is a port nobody answers");
        close(handle);
    }

    /// **Many workers, many connections in flight** — the slice, in the runtime
    /// crate.
    ///
    /// Eight workers accept on one listener at the same time; eight clients
    /// connect at once; every worker holds its request without answering until
    /// all eight have been handed out. A one-connection-at-a-time acceptor
    /// cannot get past the first, so the barrier below is the assertion: it is
    /// only released when the eighth request has been accepted.
    ///
    /// Every wait is bounded — an idle deadline on the listener, socket
    /// deadlines on every exchange, a bounded barrier, and every thread joined —
    /// which is the rule this file's other server cases already state.
    #[test]
    fn eight_workers_hold_eight_requests_at_once() {
        const WORKERS: usize = 8;
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &ServePlan::speaking(&[Protocol::Http1]), WORKERS as i64, 20_000)
                .expect("a bound port");

        let clients: Vec<_> = (0..WORKERS)
            .map(|i| {
                std::thread::spawn(move || {
                    let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
                    socket.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
                    socket.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
                    socket
                        .write_all(format!("GET /{i} HTTP/1.1\r\nhost: h\r\n\r\n").as_bytes())
                        .expect("request");
                    socket.flush().expect("flush");
                    let mut reply = Vec::new();
                    let _ = socket.read_to_end(&mut reply);
                    String::from_utf8_lossy(&reply).to_string()
                })
            })
            .collect();

        // Every worker accepts and *keeps* its request. Nothing is answered
        // until all eight are in hand, so this only completes if eight
        // connections are genuinely in flight.
        let held: Vec<_> = (0..WORKERS)
            .map(|_| {
                let started = Instant::now();
                let connection = accept(handle).expect("a connection");
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "an accept outlasted the listener's own idle deadline"
                );
                let read = request(connection).expect("the request on it");
                (connection, read)
            })
            .collect();
        let paths: std::collections::BTreeSet<String> =
            held.iter().map(|(_, request)| request.target.clone()).collect();
        assert_eq!(paths.len(), WORKERS, "two workers were handed the same request: {paths:?}");

        // **Ordering independence**: the answers go out in the reverse of the
        // order the requests came in, and every client still gets its own.
        for (connection, request) in held.into_iter().rev() {
            let body = request.target.clone().into_bytes();
            respond(connection, 200, &[], &body).expect("a response");
        }

        for (i, client) in clients.into_iter().enumerate() {
            let reply = client.join().expect("the client finished");
            assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
            assert!(
                reply.ends_with(&format!("\r\n\r\n/{i}")),
                "client {i} was answered somebody else's request:\n{reply}"
            );
        }
        close(handle);
    }

    /// **A waiting worker is interruptible**, which is what lets one worker stop
    /// the others.
    ///
    /// The listener has no idle deadline, so its workers are asleep on
    /// [`Listening::arrived`] with no request coming; `close` from another
    /// thread has to get them out. Without the `notify_all` in
    /// [`Listening::finish`] this test hangs, which is exactly the failure that
    /// wakeup exists to prevent — so the wait is bounded by joined threads.
    ///
    /// It exercises [`Listening::wake`]'s dial too, one layer down: the
    /// acceptor thread is inside a blocking `accept(2)` on this listener, and
    /// nothing but a connection brings it out. That half is not asserted here
    /// because a thread that never exits is a leak rather than a failure; what
    /// asserts it is `a_bound_listener_answers_one_request_and_then_says_it_is_closed`
    /// finishing under a test binary that joins its threads at exit.
    #[test]
    fn closing_a_listener_wakes_the_workers_blocked_on_it() {
        let (handle, _port, _handlers) = bind("127.0.0.1", 0, &ServePlan::default(), -1, -1).expect("a bound port");
        let workers: Vec<_> = (0..3)
            .map(|_| std::thread::spawn(move || accept(handle).err().map(|e| e.cause)))
            .collect();
        // Give the three time to reach the accept. A worker that has not blocked
        // yet reads `finished` under the same lock instead, so this sleep is
        // what makes the test exercise the *dial* rather than the check.
        std::thread::sleep(Duration::from_millis(200));
        close(handle);
        for worker in workers {
            let cause = worker.join().expect("a worker finished");
            assert_eq!(
                cause,
                Some(ServeFail::Closed),
                "a worker blocked in accept was not woken by the close"
            );
        }
    }

    /// The request limit counts requests **handed out**, so a limit of one is
    /// one request however many workers are waiting for it.
    ///
    /// This is the case the counter moved for: while `served` was incremented at
    /// the response, four workers could each take a request before any of them
    /// answered, and a server told to answer one question answered four.
    #[test]
    fn a_limit_of_one_hands_out_one_request_however_many_workers_wait() {
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &ServePlan::speaking(&[Protocol::Http1]), 1, -1).expect("a bound port");
        let workers: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(move || accept(handle)))
            .collect();
        std::thread::sleep(Duration::from_millis(200));
        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            socket.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            socket.write_all(b"GET /once HTTP/1.1\r\nhost: h\r\n\r\n").expect("request");
            socket.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = socket.read_to_end(&mut reply);
            String::from_utf8_lossy(&reply).to_string()
        });

        let mut served = Vec::new();
        let mut closed = 0;
        for worker in workers {
            match worker.join().expect("a worker finished") {
                Ok(connection) => served.push(connection),
                Err(e) => {
                    assert_eq!(e.cause, ServeFail::Closed, "{}", e.detail);
                    closed += 1;
                }
            }
        }
        assert_eq!(served.len(), 1, "the limit of one was spent more than once");
        assert_eq!(closed, 3, "the other workers were not told the listener was finished");
        let connection = served.pop().expect("the one connection");
        let read = request(connection).expect("the request on it");
        assert_eq!(read.target, "/once");
        respond(connection, 200, &[], b"ok").expect("a response");
        let reply = client.join().expect("the client finished");
        assert!(reply.ends_with("\r\n\r\nok"), "{reply}");
        close(handle);
    }

    // -----------------------------------------------------------------------
    // TLS, ALPN and HTTP/2
    // -----------------------------------------------------------------------
    //
    // Three cases, all against a server this file starts on loopback with the
    // certificate `cli/runtime/tls.rs` generated — the same leaf for
    // `localhost`, the same test CA, and the same `openssl` commands recorded
    // beside them. Nothing here reaches the network, resolves a name off this
    // machine, or waits without a bound: every socket carries a deadline, the
    // listener carries an idle deadline, and every thread is joined.
    //
    // They live in this crate for C7's reason, unchanged: a TLS *client* needs
    // `rustls`, the only `rustls` in this repository is inside the runtime
    // archive, and a dev-dependency on it in `cli/Cargo.toml` fails
    // `dependencies_stay_behind_the_bar`. `cli/tests/native/` asserts what it
    // can from the other side of the C ABI — that a Buri `Server` with a `tls`
    // field binds, and that a certificate it cannot read stops it — and the
    // handshake itself is asserted here.

    /// How long anything in the TLS cases waits for anything. `tls.rs`'s own
    /// `PATIENCE`, and its argument: generous because a loaded machine is not a
    /// failing one, finite because the alternative is a job CI has to kill.
    #[cfg(feature = "net")]
    const PATIENCE: Duration = Duration::from_secs(30);

    /// The server's identity, written where the acceptor can read it.
    ///
    /// Two files rather than one, because that is the shape `Server`'s `tls`
    /// field has and the shape a deployment has.
    ///
    /// **Named for the case as well as the process**, which is not decoration:
    /// `cargo test` runs these on threads of one process, `std::fs::write`
    /// truncates before it writes, and three cases sharing one path is one case
    /// reading a file another is halfway through — which presented, under a
    /// parallel hammer, as `holds no PRIVATE KEY … block this runtime could
    /// read` about one run in eight. A file two tests share is a file two tests
    /// race on.
    #[cfg(feature = "net")]
    fn identity_files(row: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("buri-rt-net-{}-{row}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory of this case's own");
        let certificate = dir.join("leaf.pem");
        let key = dir.join("leaf.key");
        std::fs::write(&certificate, crate::tls::tests::LEAF_PEM).expect("the leaf");
        std::fs::write(&key, crate::tls::tests::LEAF_KEY_PEM).expect("the key");
        (certificate, key)
    }

    /// A client that trusts the test CA and offers exactly the protocols named.
    #[cfg(feature = "net")]
    fn client_config(alpn: Vec<Vec<u8>>) -> rustls::ClientConfig {
        let mut roots = rustls::RootCertStore::empty();
        for der in crate::tls::blocks_in(crate::tls::tests::CA_PEM, "CERTIFICATE") {
            roots
                .add(rustls::pki_types::CertificateDer::from(der))
                .expect("the test CA is a certificate this verifier can use");
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("a usable configuration")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn;
        config
    }

    /// Connect, handshake, and answer with what ALPN chose.
    ///
    /// The name is `localhost` and the address is `127.0.0.1`, which is not a
    /// contradiction: the SNI name and the name the certificate is checked
    /// against are the *authority*, and the address is how the socket gets
    /// there. `tls.rs`'s own cases pin the other half — that
    /// `https://127.0.0.1/` against this leaf is a refusal.
    #[cfg(feature = "net")]
    fn handshaken(port: u16, alpn: Vec<Vec<u8>>) -> (rustls::ClientConnection, TcpStream) {
        let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        sock.set_read_timeout(Some(PATIENCE)).expect("a deadline");
        sock.set_write_timeout(Some(PATIENCE)).expect("a deadline");
        let name = rustls::pki_types::ServerName::try_from("localhost").expect("a server name");
        let mut conn =
            rustls::ClientConnection::new(Arc::new(client_config(alpn)), name).expect("a client");
        let mut rounds = 0;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).expect("the handshake");
            rounds += 1;
            assert!(rounds <= 8, "the handshake did not finish");
        }
        (conn, sock)
    }

    /// **ALPN chooses, and HTTP/1.1 over TLS is the same server it was.**
    ///
    /// The client offers `http/1.1` alone against a listener that offers both,
    /// so the negotiation has a real choice to make and makes the one the
    /// client left it. What follows is byte for byte the exchange
    /// `a_bound_listener_answers_one_request_and_then_says_it_is_closed` has
    /// over cleartext, which is the assertion: the transport changed and the
    /// framing did not.
    #[cfg(feature = "net")]
    #[test]
    fn alpn_chooses_http1_and_the_acceptor_frames_it_over_tls() {
        let (certificate, key) = identity_files("http1");
        let plan = ServePlan {
            protocols: vec![Protocol::Http1, Protocol::Http2],
            certificate: Some(certificate),
            key: Some(key),
            drain: None,
            socket_buffer: None,
        };
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, 1, 30_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let (conn, sock) = handshaken(port, vec![crate::tls::ALPN_HTTP1.to_vec()]);
            let chosen = conn.alpn_protocol().map(<[u8]>::to_vec);
            let mut stream = rustls::StreamOwned::new(conn, sock);
            stream.write_all(b"GET /over-tls HTTP/1.1\r\nhost: localhost\r\n\r\n").expect("write");
            stream.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = stream.read_to_end(&mut reply);
            (chosen, String::from_utf8_lossy(&reply).into_owned())
        });

        let connection = accept(handle).expect("one connection");
        let read = request(connection).expect("the request on it");
        assert_eq!(read.target, "/over-tls");
        // The handler is handed nothing about the transport, which is the point
        // of the `Wire`: a `Request` is four fields and none of them is "how it
        // arrived".
        assert_eq!(read.headers.iter().find(|(n, _)| n == "host").map(|(_, v)| v.as_str()),
                   Some("localhost"));
        respond(connection, 200, &[("x-from".into(), "buri".into())], b"pong")
            .expect("a response");

        let (chosen, reply) = client.join().expect("the client finished");
        assert_eq!(
            chosen.as_deref(),
            Some(crate::tls::ALPN_HTTP1),
            "a client that offered only http/1.1 was answered something else"
        );
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.contains("x-from: buri\r\n"), "{reply}");
        assert!(reply.ends_with("\r\n\r\npong"), "{reply}");
        close(handle);
    }

    /// **ALPN chooses `h2`, and two requests are in flight on one socket.**
    ///
    /// The multiplexing is what is asserted and it is asserted the only way it
    /// can be: the server takes *both* requests before it answers *either*. A
    /// connection that carried one message at a time deadlocks on the second
    /// accept, so the test either proves the property or fails on its own
    /// deadline rather than passing weakly.
    ///
    /// The client is `hyper`'s own h2 client over the same [`crate::tls::AsyncTls`]
    /// the server reads through, which is deliberate: an adapter that only ever
    /// drove one side would be a test asserting against itself.
    #[cfg(feature = "net")]
    #[test]
    fn an_h2_client_multiplexes_two_requests_over_one_connection() {
        let (certificate, key) = identity_files("h2");
        let plan = ServePlan {
            protocols: vec![Protocol::Http2, Protocol::Http1],
            certificate: Some(certificate),
            key: Some(key),
            drain: None,
            socket_buffer: None,
        };
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, 2, 30_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let (conn, sock) =
                handshaken(port, vec![crate::tls::ALPN_H2.to_vec(), crate::tls::ALPN_HTTP1.to_vec()]);
            let chosen = conn.alpn_protocol().map(<[u8]>::to_vec);
            assert_eq!(
                chosen.as_deref(),
                Some(crate::tls::ALPN_H2),
                "a listener that asked for HTTP/2 did not negotiate it"
            );
            crate::rt::handle().block_on(async move {
                let io = crate::tls::AsyncTls::adopt(sock, conn).expect("an async connection");
                let (send, driving) = hyper::client::conn::http2::handshake::<_, _, Once>(Spawn, io)
                    .await
                    .expect("an h2 handshake");
                crate::rt::handle().spawn(async move {
                    let _driven = driving.await;
                });
                let asking = |path: &'static str| {
                    let mut send = send.clone();
                    crate::rt::handle().spawn(async move {
                        let request = hyper::Request::builder()
                            .method("GET")
                            .uri(format!("https://localhost{path}"))
                            .body(Once(None))
                            .expect("a request");
                        let response = send.send_request(request).await.expect("a response");
                        let status = response.status().as_u16();
                        let body = collected(response.into_body(), 64 * 1024)
                            .await
                            .expect("a body");
                        (status, String::from_utf8_lossy(&body).into_owned())
                    })
                };
                // Both requests are issued before either is answered, which is
                // the client's half of the multiplexing claim.
                let one = asking("/one");
                let two = asking("/two");
                let one = one.await.expect("the first stream finished");
                let two = two.await.expect("the second stream finished");
                (chosen, one, two)
            })
        });

        // The server's half: two connections in hand at once, off one socket.
        let first = accept(handle).expect("the first stream");
        let second = accept(handle).expect("the second stream");
        assert_ne!(first, second, "two streams were given one connection id");
        let mut answered: Vec<(i64, String)> = Vec::new();
        for connection in [first, second] {
            let read = request(connection).expect("the request on it");
            answered.push((connection, read.target));
        }
        // Answered in the reverse of the order they arrived, because a stream's
        // answer belongs to that stream and to no other.
        for (connection, target) in answered.into_iter().rev() {
            let body = target.trim_start_matches('/').to_string().into_bytes();
            respond(connection, 200, &[("x-stream".into(), target)], &body).expect("a response");
        }

        let (chosen, one, two) = client.join().expect("the client finished");
        assert_eq!(chosen.as_deref(), Some(crate::tls::ALPN_H2));
        assert_eq!(one, (200, String::from("one")));
        assert_eq!(two, (200, String::from("two")));
        close(handle);
    }

    /// **A drain sends an idle HTTP/2 connection on its way, rather than
    /// waiting for it.**
    ///
    /// This is the case HTTP/2 makes and HTTP/1.1 does not. A cleartext
    /// connection here carries one message and ends, so a drain that only
    /// stopped accepting would already be finished; an h2 connection is a thing
    /// a client *keeps*, and a client holding an idle one open is doing exactly
    /// what the protocol is for. Without `hyper`'s `graceful_shutdown` the drain
    /// below would wait out its whole deadline for a client that has done
    /// nothing wrong.
    ///
    /// So the instrument is the clock and the deadline together: fifteen
    /// seconds asked for, and the assertion is that it took under five. A drain
    /// that could not reach into the connection answers `false` after fifteen,
    /// which fails on the line above rather than on the timing.
    #[cfg(feature = "net")]
    #[test]
    fn a_drain_sends_an_idle_h2_connection_on_its_way() {
        let (certificate, key) = identity_files("h2-drain");
        let plan = ServePlan {
            protocols: vec![Protocol::Http2],
            certificate: Some(certificate),
            key: Some(key),
            drain: Some(Duration::from_secs(15)),
            socket_buffer: None,
        };
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 30_000).expect("a bound port");

        let (answered, was_answered) = std::sync::mpsc::channel();
        let client = std::thread::spawn(move || {
            let (conn, sock) = handshaken(port, vec![crate::tls::ALPN_H2.to_vec()]);
            crate::rt::handle().block_on(async move {
                let io = crate::tls::AsyncTls::adopt(sock, conn).expect("an async connection");
                let (mut send, driving) =
                    hyper::client::conn::http2::handshake::<_, _, Once>(Spawn, io)
                        .await
                        .expect("an h2 handshake");
                let driven = crate::rt::handle().spawn(async move {
                    let _driven = driving.await;
                });
                let request = hyper::Request::builder()
                    .method("GET")
                    .uri("https://localhost/keep")
                    .body(Once(None))
                    .expect("a request");
                let response = send.send_request(request).await.expect("a response");
                let status = response.status().as_u16();
                let body =
                    collected(response.into_body(), 64 * 1024).await.expect("a body");
                let _ = answered.send(());
                // And now the client does nothing at all, holding its
                // connection open. What ends this wait is the server's GOAWAY —
                // **and the deadline is what stops it being a hang** when the
                // GOAWAY does not come. Under a drain that cannot reach into
                // `hyper` this is the wait that would otherwise run until CI
                // killed the job; with the bound it is a `false` on the line
                // below instead. That rule has a name in this repository and it
                // is the tls-hang incident.
                let ended = tokio::time::timeout(PATIENCE, driven).await.is_ok();
                (status, String::from_utf8_lossy(&body).into_owned(), ended)
            })
        });

        let connection = accept(handle).expect("the one stream");
        assert_eq!(request(connection).expect("its request").target, "/keep");
        respond(connection, 200, &[], b"kept").expect("a response");
        was_answered.recv_timeout(PATIENCE).expect("the client read its answer");

        let started = Instant::now();
        assert!(drain(handle), "the drain gave up on an h2 connection it should have ended");
        let waited = started.elapsed();
        assert!(waited < Duration::from_secs(5), "the GOAWAY did not end it: {waited:?}");

        let (status, body, ended) = client.join().expect("the client finished");
        assert_eq!((status, body.as_str()), (200, "kept"));
        assert!(ended, "the client's connection was never sent a GOAWAY");
        assert_eq!(accept(handle).expect_err("drained").cause, ServeFail::Closed);
        close(handle);
    }

    /// **A handler that never answers is a `504`, not a stream held for ever.**
    ///
    /// The sender is alive for the whole of this wait, which is what separates
    /// the two failures: a *dropped* sender is `listenClose` taking the
    /// connection out from under a handler and has always answered `503`, and
    /// a sender that is simply never used is a handler that did not finish.
    /// Before [`ANSWER_DEADLINE`] the second had no answer at all — the receive
    /// waited, and the task, the stream and its place under [`MAX_STREAMS`]
    /// waited with it until the process ended.
    ///
    /// Asserted at the wait rather than through a server, because the case
    /// needs a handler that never responds and there is no other way to write
    /// one: every acceptance case in this file answers what it accepts.
    #[cfg(feature = "net")]
    #[test]
    fn a_stream_whose_handler_never_answers_is_told_so_at_the_deadline() {
        let (back, written) = tokio::sync::oneshot::channel::<Reply>();
        let started = Instant::now();
        let response =
            crate::rt::handle().block_on(answered_within(written, Duration::from_millis(80)));
        let waited = started.elapsed();
        assert_eq!(response.status().as_u16(), 504, "a handler that never answered was not a 504");
        assert!(waited >= Duration::from_millis(60), "{waited:?} is not a wait at all");
        assert!(waited < Duration::from_secs(5), "the stream waited {waited:?}");
        // Held across the wait on purpose: this is the deadline arm and not the
        // dropped-sender one.
        drop(back);
    }

    /// **A drain ends an HTTP/2 connection whose stream will not finish.**
    ///
    /// The companion to `a_drain_sends_an_idle_h2_connection_on_its_way`, and
    /// the case that one cannot reach. There the client is idle, so the GOAWAY
    /// is the whole answer and the connection ends at once. Here a stream is
    /// open and unanswered — the handler took the request and never responded —
    /// so `graceful_shutdown` does exactly what it promises and *waits* for it.
    /// GOAWAY is a request, not a close, and a client with an open stream is
    /// entitled to hold the connection; until this bound existed, holding it
    /// meant holding a task, a socket and a place in `Gate::outstanding` for
    /// the life of the process. `DRAIN_DEADLINE` bounded the caller of `drain`
    /// and nothing bounded the connection.
    ///
    /// So the instrument is the **client's own connection future**, and only
    /// that. Without the bound `ended` is `false` after `PATIENCE`; with it the
    /// socket closes half a second after the GOAWAY and `ended` is `true`.
    ///
    /// `drain`'s own boolean is deliberately *not* asserted, and the reason is
    /// worth writing down rather than discovering twice: the plan's one drain
    /// number is now the deadline for both waiters — `drain` waiting for the
    /// connections and the connection waiting to be let go — so the two expire
    /// at the same instant and either may observe the other first. That is
    /// harmless in a server, where both endings are "the drain is over", and it
    /// is a coin flip in a test. What is asserted about `drain` is the half
    /// that is not a race: that it comes back, and comes back promptly.
    #[cfg(feature = "net")]
    #[test]
    fn a_drain_ends_an_h2_connection_whose_stream_will_not_finish() {
        let (certificate, key) = identity_files("h2-stuck");
        let plan = ServePlan {
            protocols: vec![Protocol::Http2],
            certificate: Some(certificate),
            key: Some(key),
            drain: Some(Duration::from_millis(500)),
            socket_buffer: None,
        };
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 30_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let (conn, sock) = handshaken(port, vec![crate::tls::ALPN_H2.to_vec()]);
            crate::rt::handle().block_on(async move {
                let io = crate::tls::AsyncTls::adopt(sock, conn).expect("an async connection");
                let (mut send, driving) =
                    hyper::client::conn::http2::handshake::<_, _, Once>(Spawn, io)
                        .await
                        .expect("an h2 handshake");
                let driven = crate::rt::handle().spawn(async move {
                    let _driven = driving.await;
                });
                let request = hyper::Request::builder()
                    .method("GET")
                    .uri("https://localhost/stuck")
                    .body(Once(None))
                    .expect("a request");
                // Issued and never finished: this is the stream the server
                // will be asked to give up on.
                let asking = crate::rt::handle().spawn(async move {
                    let _ = send.send_request(request).await;
                });
                let ended = tokio::time::timeout(PATIENCE, driven).await.is_ok();
                asking.abort();
                ended
            })
        });

        let connection = accept(handle).expect("the one stream");
        assert_eq!(request(connection).expect("its request").target, "/stuck");
        // And no `respond`. The handler has the request and never answers.

        let started = Instant::now();
        let _either_way = drain(handle);
        let waited = started.elapsed();
        assert!(waited < Duration::from_secs(5), "the drain itself ran {waited:?}");

        let ended = client.join().expect("the client finished");
        assert!(ended, "the connection outlived its GOAWAY and its drain deadline");
        close(handle);
    }

    /// A certificate the runtime cannot read stops the **bind**, and says which
    /// file and why.
    ///
    /// At the bind rather than at the first handshake, because a certificate
    /// that is not there is a configuration mistake and a configuration mistake
    /// should stop a program starting.
    #[cfg(feature = "net")]
    #[test]
    fn a_certificate_the_runtime_cannot_read_stops_the_bind() {
        let missing = std::env::temp_dir()
            .join(format!("buri-rt-net-{}-absent.pem", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        let (_certificate, key) = identity_files("refused");
        let plan = ServePlan {
            protocols: vec![Protocol::Http1],
            certificate: Some(missing.clone()),
            key: Some(key.clone()),
            drain: None,
            socket_buffer: None,
        };
        let refused = bind("127.0.0.1", 0, &plan, -1, 60).expect_err("there is no such file");
        assert_eq!(refused.cause, ServeFail::Transport);
        assert!(refused.detail.contains(&missing.display().to_string()), "{}", refused.detail);

        // A file that exists and holds no certificate is the other half of the
        // same question, and gets a different sentence rather than the same one.
        let empty =
            std::env::temp_dir().join(format!("buri-rt-net-{}-empty.pem", std::process::id()));
        std::fs::write(&empty, "not a certificate\n").expect("a file with no PEM in it");
        let plan = ServePlan {
            protocols: vec![Protocol::Http1],
            certificate: Some(empty.clone()),
            key: Some(key),
            drain: None,
            socket_buffer: None,
        };
        let refused = bind("127.0.0.1", 0, &plan, -1, 60).expect_err("no identity to present");
        assert_eq!(refused.cause, ServeFail::Transport);
        assert!(refused.detail.contains("CERTIFICATE"), "{}", refused.detail);
        let _ = std::fs::remove_file(&empty);
    }
    // -----------------------------------------------------------------------
    // WebSockets
    // -----------------------------------------------------------------------
    //
    // Every one of these binds `127.0.0.1:0`, puts a deadline on every socket,
    // bounds the listener's own wait, and joins every thread it starts. The
    // client is `tungstenite`'s own — which this crate has because the server
    // half is `tungstenite`'s too — so what is under test is this file's
    // upgrade, queue and close and not a hand-written client's idea of RFC
    // 6455.

    /// How long a socket test waits for an event before it calls the socket
    /// broken. Not a measurement — nothing here asserts how fast a socket
    /// answers — so it is generous, and what it buys is a failing test with a
    /// sentence instead of a job CI has to kill.
    #[cfg(feature = "net")]
    const SOCKET_DEADLINE: Duration = Duration::from_secs(20);

    /// `receive`, with a deadline, on a thread of its own.
    ///
    /// **`listenReceive` is the second call in this file that may wait
    /// forever**, and a test that waited forever is a suite CI has to kill —
    /// which is the rule the tls-hang fix left behind and every row here obeys.
    /// So the wait happens on a thread and this one holds a deadline; the way
    /// out on expiry is to close the listener, which retires the socket and
    /// brings the thread home with an `.Err` rather than leaving it detached.
    ///
    /// The deadline is generous because it is not a measurement: nothing here
    /// asserts how *fast* a socket answers, only that it does, so this is the
    /// difference between a failing test with a sentence and a hang.
    #[cfg(feature = "net")]
    fn received_within(socket: i64, handle: i64, within: Duration) -> Result<Received, ServeErr> {
        let (said, heard) = std::sync::mpsc::channel();
        let waiting = std::thread::spawn(move || {
            let _sent = said.send(sockets::receive(socket));
        });
        let answered = heard.recv_timeout(within);
        let late = answered.is_err();
        if late {
            close(handle);
        }
        let outcome = match answered {
            Ok(outcome) => outcome,
            Err(_) => heard
                .recv_timeout(within)
                .expect("the receive ended once the listener was closed"),
        };
        waiting.join().expect("the receiving thread finished");
        assert!(!late, "listenReceive answered nothing within {within:?}");
        outcome
    }

    /// The plan a socket test binds with: HTTP/1.1, no identity, and whatever
    /// outbound bound the case is about.
    #[cfg(feature = "net")]
    fn socket_plan(buffer: Option<usize>) -> ServePlan {
        ServePlan {
            protocols: vec![Protocol::Http1],
            certificate: None,
            key: None,
            drain: Some(Duration::from_secs(10)),
            socket_buffer: buffer,
        }
    }

    /// Take the one connection a client made and turn it into a socket.
    #[cfg(feature = "net")]
    fn upgraded_socket(handle: i64) -> i64 {
        let connection = accept(handle).expect("one connection");
        let read = request(connection).expect("the upgrade request");
        assert_eq!(read.target, "/socket");
        sockets::upgrade(connection).expect("an upgrade this acceptor can complete")
    }

    /// **A message each way, and the send is flushed by the next receive.**
    ///
    /// The ordering is the claim rather than the round trip: `send_text` only
    /// queues, so the `pong` reaches the wire when the socket's own loop next
    /// asks what arrived — which is exactly what `socketSendText`'s "never
    /// waits" costs and what makes it safe to call from a task that holds no
    /// listener. A `send` that wrote to the socket itself would pass this too;
    /// what would fail is a `receive` that did not flush, and the client's read
    /// deadline is what says so.
    #[cfg(feature = "net")]
    #[test]
    fn a_socket_carries_a_message_each_way_and_then_closes() {
        let plan = socket_plan(None);
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            stream.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            let (mut socket, response) =
                tungstenite::client(format!("ws://127.0.0.1:{port}/socket"), stream)
                    .expect("the handshake");
            assert_eq!(response.status().as_u16(), 101);
            socket.send(tungstenite::Message::text("ping")).expect("a message out");
            let back = socket.read().expect("a message back");
            socket
                .close(Some(tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                    reason: "done".into(),
                }))
                .expect("a close");
            // Driven to completion, which is what leaves the server's own read
            // with a close frame rather than a dropped socket.
            while socket.read().is_ok() {}
            back
        });

        let socket = upgraded_socket(handle);
        let arrived = received_within(socket, handle, SOCKET_DEADLINE)
            .expect("the client's message");
        assert_eq!(arrived.frame, 0, "a text frame arrived as frame {}", arrived.frame);
        assert_eq!(arrived.text, "ping");
        sockets::send_text(socket, "pong");
        // The second receive is what writes it, and then waits for the close.
        let closed =
            received_within(socket, handle, SOCKET_DEADLINE).expect("the client's close");
        assert_eq!(closed.frame, 2, "the socket's last event is its close");
        assert_eq!(closed.code, 1000, "a clean close is 1000");

        let back = client.join().expect("the client finished");
        assert_eq!(back, tungstenite::Message::text("pong"));

        // **`.Closed` is the last answer a socket gives**: it is gone from the
        // table, a message to it is dropped, and asking again is `.Err`.
        sockets::send_text(socket, "nobody is there");
        assert_eq!(sockets::receive(socket).expect_err("spent").cause, ServeFail::Closed);
        assert!(!sockets::is_open(socket), "the socket was not retired");

        close(handle);
    }

    /// **An overflow closes the socket, and the loop is told what a peer could
    /// never have said.**
    ///
    /// The buffer is one message deep and nothing is flushing it, so the second
    /// `send` is the overflow. What the socket's own loop reads is a close
    /// carrying a *negative* code — [`OVERFLOWED`] — which is unreachable from
    /// the wire, so `core/net/server` can map it to `.Overflow` without a
    /// client being able to claim the same thing.
    ///
    /// The far side is told 1011 instead, because "this end broke" is the true
    /// sentence from where the client is standing.
    #[cfg(feature = "net")]
    #[test]
    fn a_full_outbound_buffer_closes_the_socket_and_says_so() {
        let plan = socket_plan(Some(1));
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            stream.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            let (mut socket, _response) =
                tungstenite::client(format!("ws://127.0.0.1:{port}/socket"), stream)
                    .expect("the handshake");
            // Reads until the close frame the overflow produced, and answers
            // with the code it carried.
            let mut code = 0u16;
            while let Ok(message) = socket.read() {
                if let tungstenite::Message::Close(frame) = message {
                    code = frame.map_or(0, |f| u16::from(f.code));
                    break;
                }
            }
            code
        });

        let socket = upgraded_socket(handle);
        // Nothing has flushed the queue, so the second of these is the one that
        // does not fit.
        sockets::send_text(socket, "one");
        sockets::send_text(socket, "two");
        let closed =
            received_within(socket, handle, SOCKET_DEADLINE).expect("the overflow, as a close");
        assert_eq!(closed.frame, 2);
        assert_eq!(
            closed.code, OVERFLOWED,
            "an overflow is the platform's own reason and not a wire code"
        );
        assert!(closed.code < 0, "a wire code is unsigned, so this one cannot be forged");

        assert_eq!(
            client.join().expect("the client finished"),
            OVERFLOW_CODE,
            "the far side is told 1011, which is the true sentence from there"
        );
        assert!(!sockets::is_open(socket), "the socket was not retired");

        close(handle);
    }

    /// **A request that is not an upgrade is left exactly as it was**, which is
    /// what lets `core/net/server` ask about every request and hand the ones it
    /// is refused to `onRequest`.
    ///
    /// Two halves, and the second is the one that would fail silently: the
    /// refusal names what an upgrade is, *and* the connection is still there to
    /// be responded to. A `listenUpgrade` that consumed what it refused would
    /// be a client left holding a socket nobody was going to write to.
    #[cfg(feature = "net")]
    #[test]
    fn a_request_that_is_not_an_upgrade_is_refused_and_still_answerable() {
        let plan = socket_plan(None);
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            stream.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            stream.write_all(b"GET /ordinary HTTP/1.1\r\nhost: h\r\n\r\n").expect("request");
            stream.flush().expect("flush");
            let mut reply = Vec::new();
            let _ = stream.read_to_end(&mut reply);
            reply
        });

        let connection = accept(handle).expect("one connection");
        let read = request(connection).expect("the request");
        assert_eq!(read.target, "/ordinary");
        let refused = sockets::upgrade(connection).expect_err("this was not an upgrade");
        assert_eq!(refused.cause, ServeFail::Unsupported);
        assert!(refused.detail.contains("upgrade: websocket"), "{}", refused.detail);
        // Still answerable, which is the half that matters.
        respond(connection, 200, &[], b"handled").expect("a response");

        let reply =
            String::from_utf8_lossy(&client.join().expect("the client finished")).to_string();
        assert!(reply.ends_with("\r\n\r\nhandled"), "{reply}");

        close(handle);
    }

    /// **A drain sends an open socket on its way**, which is h2's GOAWAY
    /// argument reaching the other thing a client keeps open on purpose.
    ///
    /// Two-sided, as every drain row here is: the socket's loop is told 1001
    /// (so the close *happened*), and the drain returns inside its own deadline
    /// (so it did not wait the socket out). Without the close it would sit for
    /// the whole ten seconds, which is the failure this row would otherwise be
    /// unable to tell from a pass.
    #[cfg(feature = "net")]
    #[test]
    fn a_drain_sends_an_open_socket_on_its_way() {
        let plan = socket_plan(None);
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            stream.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            let (mut socket, _response) =
                tungstenite::client(format!("ws://127.0.0.1:{port}/socket"), stream)
                    .expect("the handshake");
            // Idles, which is what a WebSocket client does and what a drain
            // that could only wait would wait out.
            let mut code = 0u16;
            while let Ok(message) = socket.read() {
                if let tungstenite::Message::Close(frame) = message {
                    code = frame.map_or(0, |f| u16::from(f.code));
                    break;
                }
            }
            code
        });

        let socket = upgraded_socket(handle);
        let began = Instant::now();
        let draining = std::thread::spawn(move || drain(handle));
        let closed =
            received_within(socket, handle, SOCKET_DEADLINE).expect("the drain's close");
        assert_eq!(closed.frame, 2);
        assert_eq!(closed.code, i64::from(GOING_AWAY), "a drained socket is going away");
        assert!(
            draining.join().expect("the drain finished"),
            "the drain timed out rather than closing the socket it was holding"
        );
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the drain waited the socket out rather than closing it: {:?}",
            began.elapsed()
        );
        assert_eq!(client.join().expect("the client finished"), GOING_AWAY);

        assert_eq!(accept(handle).expect_err("drained").cause, ServeFail::Closed);
        close(handle);
    }

    /// **Closing a listener takes its sockets with it**, and gives their places
    /// in flight back — which is what stops a drain of a *second* listener from
    /// waiting out a socket nobody is reading.
    #[cfg(feature = "net")]
    #[test]
    fn closing_a_listener_takes_its_sockets_with_it() {
        let plan = socket_plan(None);
        let (handle, port, _handlers) =
            bind("127.0.0.1", 0, &plan, -1, 20_000).expect("a bound port");

        let client = std::thread::spawn(move || {
            let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            stream.set_write_timeout(Some(Duration::from_secs(20))).expect("a deadline");
            let (mut socket, _response) =
                tungstenite::client(format!("ws://127.0.0.1:{port}/socket"), stream)
                    .expect("the handshake");
            while socket.read().is_ok() {}
        });

        let socket = upgraded_socket(handle);
        close(handle);
        // The socket is gone with the listener, so its own loop is told the
        // ordinary thing a spent handle is told and runs its close hook.
        assert_eq!(sockets::receive(socket).expect_err("gone").cause, ServeFail::Closed);
        assert!(!sockets::is_open(socket), "the listener did not take its socket with it");
        client.join().expect("the client finished");
    }

    /// A handle that names no socket is one that has already gone, which is the
    /// behaviour `effect Sockets` declares for a socket somebody closed — so a
    /// forged handle and a spent one are deliberately the same answer, and all
    /// four entries are total.
    #[cfg(feature = "net")]
    #[test]
    fn a_socket_handle_that_names_nothing_is_answered_and_not_aborted() {
        for made_up in [-1, 0, i64::MAX] {
            sockets::send_text(made_up, "into the void");
            sockets::send_bytes(made_up, &[1, 2, 3]);
            sockets::close(made_up, 1000, "bye");
            assert_eq!(
                sockets::receive(made_up).expect_err("no such socket").cause,
                ServeFail::Closed
            );
        }
    }

    /// Every clause RFC 6455 §4.2.1 requires, and none of them optional.
    ///
    /// The two multi-valued fields are searched rather than compared, because a
    /// browser sends `Connection: keep-alive, Upgrade` and a server that
    /// refused it would be refusing correct clients.
    #[test]
    fn an_upgrade_request_is_the_five_things_rfc_6455_asks_for() {
        let head = |extra: &[(&str, &str)]| {
            let mut headers: Vec<(String, String)> = vec![
                (String::from("upgrade"), String::from("websocket")),
                (String::from("connection"), String::from("Upgrade")),
                (String::from("sec-websocket-version"), String::from("13")),
                (String::from("sec-websocket-key"), String::from("dGhlIHNhbXBsZSBub25jZQ==")),
            ];
            for (name, value) in extra {
                headers.retain(|(had, _)| had != name);
                if !value.is_empty() {
                    headers.push(((*name).to_string(), (*value).to_string()));
                }
            }
            ServerRequest {
                method: 0,
                target: String::from("/socket"),
                headers,
                body: Vec::new(),
            }
        };
        assert_eq!(upgrade_key(&head(&[])).as_deref(), Some("dGhlIHNhbXBsZSBub25jZQ=="));
        // A list, matched token by token and case-insensitively, which is what
        // every browser actually sends.
        assert!(upgrade_key(&head(&[("connection", "keep-alive, Upgrade")])).is_some());
        assert!(upgrade_key(&head(&[("upgrade", "WebSocket")])).is_some());
        // And each clause, missing.
        assert!(upgrade_key(&head(&[("upgrade", "")])).is_none());
        assert!(upgrade_key(&head(&[("connection", "")])).is_none());
        assert!(upgrade_key(&head(&[("connection", "keep-alive")])).is_none());
        assert!(upgrade_key(&head(&[("sec-websocket-version", "8")])).is_none());
        assert!(upgrade_key(&head(&[("sec-websocket-version", "")])).is_none());
        assert!(upgrade_key(&head(&[("sec-websocket-key", "")])).is_none());
        // A POST is not an upgrade. `.Get` is index 0 in `Method`'s declaration
        // order, so anything else is a method that cannot carry one.
        let mut posted = head(&[]);
        posted.method = 2;
        assert!(upgrade_key(&posted).is_none());
    }
}
