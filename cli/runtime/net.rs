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
//! feature brings `quinn`. This file names one type from each, which for three
//! of them — `hyper`, `tungstenite` and `quinn` — is still the *whole* of what
//! references them: no intrinsic key mangles to a symbol declared here
//! (`runtime_native::symbol_for` is the rule, and
//! `backend/runtime_table.rs` is the table); neither backend emits a call into
//! them; nothing in `core/` reaches them.
//!
//! **The acceptor below does not change that, and that is a decision** — one
//! taken twice now, because F3 was the day it was due to be taken again. A
//! server is the obvious first caller of `hyper`, and the acceptor frames
//! HTTP/1.1 itself instead, for `http.rs`'s reason read from the other side of
//! the wire: a synchronous exchange over one connection is the whole of what is
//! being asked for here, and reaching a framing layer through `hyper` would mean
//! standing up a reactor per connection to get at something two hundred lines
//! already do.
//!
//! What F2 wrote down as the day to re-take it was "the day handlers run on
//! tasks of their own — many connections in flight, h2 multiplexing, ALPN", and
//! that turned out to be three days rather than one. **Handlers run on tasks
//! now** and the framing did not move an inch: a worker per handler is a fan-out
//! over the same accept, the same head parser and the same response writer, all
//! of which were already one connection's worth of work with no state between
//! them. What is still ahead is the half that is genuinely `hyper`'s —
//! multiplexing two requests over one connection, and negotiating which protocol
//! that is — and it lands with TLS in F4, which is where `hyper` is linked and
//! where `.github/scripts/assert-runtime-archive.sh` moves it from its absent
//! list to its present one.
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

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::value::{BuriList, BuriStr, list_of_bytes, list_of_headers, str_of};

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

/// Whether **this acceptor** speaks a protocol the toolchain carries the code
/// for, which is a second and narrower question than [`serves`].
///
/// The two are different because a crate being linked and a server being
/// written against it are different facts. [`serves`] answers "does this
/// archive carry the code" — the `net-h3` feature, the QUIC stack — and is the
/// one C4 built. This answers "does the acceptor below drive it", and today the
/// acceptor is [`Listening`] and the workers on it: an HTTP/1.1 server that
/// frames its own messages, one request per connection.
///
/// Both are asked, in that order, so that a program asking for HTTP/3 on a
/// toolchain without `net-h3` gets the message that names the build switch
/// rather than this one — a user can act on the first and can only wait for the
/// second.
fn accepts(protocol: Protocol) -> Result<(), String> {
    serves(protocol).map_err(str::to_string)?;
    match protocol {
        Protocol::Http1 => Ok(()),
        Protocol::Http2 | Protocol::Http3 => Err(format!(
            "{} is not spoken by this runtime's acceptor: it frames HTTP/1.1, one request per \
             connection. Serve HTTP/1.1, or leave `protocols` unset",
            protocol.name()
        )),
    }
}

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
// **HTTP/1.1, framed here rather than by `hyper`.** `http.rs` made the same
// choice for the client and gives the reason: a synchronous exchange over one
// connection is the whole of what is being asked for, and reaching a framing
// layer through `hyper` would mean standing up a reactor per connection to get
// at something these two hundred lines already do. The day handlers run on
// tasks of their own — many connections in flight, h2 multiplexing, ALPN — is
// the day that decision is worth taking again, and it is the day the archive
// starts carrying `hyper`'s bytes.
//
// **One request per connection.** The response carries `connection: close` and
// the socket is dropped after it. Keep-alive is a multiplexing question and
// belongs with h2.
//
// **A worker per handler, and a connection identifier because of it.** The loop
// is still `core/net/server`'s and still in Buri; what F3 changed is that there
// are several of it, fanned out by `Tasks.parallel` onto carriers of their own,
// all accepting on the one listener. So "the request the accept last handed
// out" stopped naming exactly one connection: [`accept`] answers a connection
// id beside the request and [`respond`] takes that id instead of the listener's.
// Everything else about the division survived — the runtime still holds no Buri
// value between calls, still calls back into no generated code, and still does
// not know what a handler is.
//
// **Every wait is bounded except the one a server is for.** Reads and writes
// carry `SO_RCVTIMEO`/`SO_SNDTIMEO` from `headerTimeoutMillis`; the wait for a
// connection carries `idleTimeoutMillis` when the caller set one. A caller that
// sets neither gets a server that waits for a client indefinitely and reads
// with a thirty-second deadline, which is what a server is; a *test* sets both,
// and every test in this repository does.

/// How long one connection may take to send its request line, its headers and
/// its body, when the caller named no deadline. [`crate::http::DEADLINE`]'s
/// argument, from the other side of the wire.
const HEAD_DEADLINE: Duration = Duration::from_secs(30);

/// The largest request body read when the caller named no limit. A request
/// declaring more is answered `413` by the acceptor and never reaches a
/// handler: a limit that a handler had to enforce would be a limit every
/// handler had to remember.
const BODY_LIMIT: usize = 8 * 1024 * 1024;

/// How long to sleep between polls of a non-blocking accept.
///
/// Only reached when the caller set an idle deadline, because that is the only
/// case that needs the listener to be interruptible. Ten milliseconds costs a
/// hundred wakeups a second on an idle port and bounds the overshoot on the
/// deadline by the same amount, which is the right trade for a knob whose whole
/// purpose is that a test does not run forever.
const POLL: Duration = Duration::from_millis(10);

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
/// The day the acceptor is asynchronous — F4, which is the day `hyper` and its
/// reactor are linked — a waiting worker costs a mapping again and this is the
/// number that moves.
const MAX_HANDLERS: i64 = 64;

/// How long [`Listening::wake`] will wait for one of its own dials.
///
/// A bound rather than a timeout anything depends on: the connection is to a
/// port this process is holding open, on loopback, so it either succeeds at once
/// or the listener is already gone and there was nothing to wake.
const WAKE_DEADLINE: Duration = Duration::from_secs(1);

/// The listener's own state, and **the whole of what its workers share.**
///
/// Three questions under one lock, because they have to be answered together: a
/// worker that takes the last request has to mark the listener finished and read
/// how many workers are blocked in the same step. Split them and a worker that
/// checked `finished` before the mark and blocked after the read waits for a
/// connection nobody is going to make — the lost wakeup this lock rules out.
struct Gate {
    /// The listener will answer no more: its request limit is spent, or it has
    /// been closed. Once true, never false again.
    finished: bool,
    /// How many workers are blocked in [`wait_for_connection`] right now, which
    /// is exactly how many dials [`Listening::wake`] owes them.
    waiting: usize,
    /// Requests handed out, which is what `requestLimit` counts.
    ///
    /// **Counted at the accept and not at the response**, and the second worker
    /// is what forced that. With one worker the two were the same number; with
    /// many, a limit checked at the response lets every worker take a request
    /// before any of them answers one, and a server told to serve one request
    /// serves as many requests as it has handlers.
    served: u64,
}

/// An open port, and everything the workers on it share.
struct Listening {
    listener: TcpListener,
    plan: Plan,
    gate: Mutex<Gate>,
    /// **One worker inside the accept at a time.**
    ///
    /// Taking a connection off a listener is microseconds; answering it is the
    /// part that takes a second, and answering is what the workers do
    /// concurrently. So they queue here and take turns at the socket, which buys
    /// two things that matter more than an overlapped accept would.
    ///
    /// The first is the idle deadline. A listener with one is polled rather than
    /// blocked on (see [`POLL`]), and sixty-four workers polling the same socket
    /// is sixty-four hundred wakeups a second and a machine that has better
    /// things to do — which is measurable as a server that takes twenty seconds
    /// to notice a client. With the turn, exactly one of them polls.
    ///
    /// The second is [`Listening::wake`]. One blocked accept is one dial, so a
    /// shutdown costs a single loopback connection rather than one per worker,
    /// and the workers behind it are woken by the lock rather than by the
    /// network.
    turn: Mutex<()>,
    /// Where [`Listening::wake`] dials to interrupt a blocking accept: the bound
    /// address, with an unspecified host replaced by loopback, because `0.0.0.0`
    /// is an address to accept on and not one to connect to.
    wake_at: SocketAddr,
}

impl Listening {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        match self.gate.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// This worker's turn at the socket. See [`Listening::turn`].
    fn turn(&self) -> MutexGuard<'_, ()> {
        match self.turn.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Mark the listener finished, and answer how many workers were waiting on
    /// it — in one step, for [`Gate`]'s reason.
    fn finish(&self) -> usize {
        let mut gate = self.gate();
        gate.finished = true;
        gate.waiting
    }

    /// Wake `waiting` blocked accepts, by connecting to the port they are
    /// blocked on.
    ///
    /// **A dial rather than a shutdown**, because there is no portable way to
    /// make a blocking `accept(2)` return: closing the descriptor under a thread
    /// that is inside the call is a use-after-free waiting for the number to be
    /// reused, and the standard library gives a `TcpListener` no `shutdown`. One
    /// connection wakes the one blocked accept — there is only ever one, because
    /// of [`Listening::turn`] — and the worker that takes it reads `finished`,
    /// gives the port back, and the socket is dropped unanswered.
    ///
    /// Nothing is dialled where the listener is interruptible already: a caller
    /// that set an idle deadline made the accept a poll, and the poll reads
    /// `finished` every [`POLL`].
    fn wake(&self, waiting: usize) {
        if self.plan.idle.is_some() {
            return;
        }
        for _ in 0..waiting {
            let _ = TcpStream::connect_timeout(&self.wake_at, WAKE_DEADLINE);
        }
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

/// One accepted connection: the request already read off it, and the socket the
/// answer goes back on.
///
/// The request is held here between [`accept`] and [`request`] because those are
/// two calls — `effect Listen` argues why. What is held is a **Rust** value: the
/// runtime still builds no Buri value it does not hand over inside the same
/// call, which is the division that lets this acceptor be a blocking
/// implementation rather than a scheduler.
struct Pending {
    /// Which listener accepted it, so that closing a listener drops the
    /// connections no worker will be handed back.
    listener: i64,
    stream: TcpStream,
    /// `None` once `listenRequest` has taken it.
    request: Option<ServerRequest>,
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


/// `Listen::listenBind`, without the ABI around it.
pub fn bind(
    address: &str,
    port: i64,
    protocols: &[u8],
    limit: i64,
    idle_millis: i64,
) -> Result<(i64, u16, i64), ServeErr> {
    // C4's handoff, spent: the walk over `Server`'s `protocols` field, one
    // `serves` per named protocol, with the acceptor's own answer after it.
    // An empty list is `.None` — the caller chose nothing — and HTTP/1.1 is
    // what the acceptor picks, so there is nothing to refuse.
    for index in protocols.iter().map(|b| i64::from(*b)) {
        let Some(protocol) = Protocol::from_index(index) else {
            return Err(ServeErr::new(
                ServeFail::Unsupported,
                format!(
                    "this runtime does not know protocol {index}: the program and the runtime \
                     disagree about `Protocol`'s variants"
                ),
            ));
        };
        accepts(protocol).map_err(|detail| ServeErr::new(ServeFail::Unsupported, detail))?;
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
    };
    // Interruptible only where the caller asked for it: an idle deadline is the
    // one thing a blocking `accept` cannot answer, and polling for a server
    // that never wanted a deadline would be a hundred wakeups a second bought
    // for nothing. A blocking accept is interrupted by [`Listening::wake`]'s
    // dial instead, which costs nothing until there is something to interrupt.
    if plan.idle.is_some() {
        listener
            .set_nonblocking(true)
            .map_err(|e| ServeErr::io(&e, "making the listener interruptible"))?;
    }
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let listening = Listening {
        listener,
        plan,
        gate: Mutex::new(Gate { finished: false, waiting: 0, served: 0 }),
        turn: Mutex::new(()),
        wake_at: dial_address(local),
    };
    acceptors(|table| table.insert(handle, Arc::new(listening)));
    Ok((handle, bound, MAX_HANDLERS))
}

/// `Listen::listenAccept` — the next request, once one has arrived and been
/// read whole.
///
/// The loop is over *connections* and not over requests: a connection that
/// speaks nonsense, or asks for more body than the plan allows, is answered
/// here and the next one is waited for, so a handler is never handed a message
/// the acceptor could not make sense of.
pub fn accept(handle: i64) -> Result<i64, ServeErr> {
    let listening = listening(handle)?;
    loop {
        // One worker at a time from here to the accept — see `Listening::turn`.
        // A worker behind the turn is asleep on a lock rather than polling a
        // socket sixty-three others are already polling.
        let waited = {
            let _turn = listening.turn();
            {
                let mut gate = listening.gate();
                if gate.finished {
                    return Err(ServeErr::closed());
                }
                gate.waiting = gate.waiting.saturating_add(1);
            }
            let waited = wait_for_connection(&listening);
            {
                let mut gate = listening.gate();
                gate.waiting = gate.waiting.saturating_sub(1);
            }
            waited
        };
        let mut stream = waited?;
        // The connection may be [`Listening::wake`]'s own dial rather than a
        // client's: a worker that was blocked when the listener finished is
        // woken by one, and what it has to do with it is give the port back.
        if listening.gate().finished {
            return Err(ServeErr::closed());
        }
        if let Err(e) = deadlines(&stream, listening.plan.head) {
            let _ = refuse(&mut stream, 500, "");
            return Err(e);
        }
        let request = match read_request(&mut stream, listening.plan.body_limit) {
            Ok(request) => request,
            // A malformed or oversized request is the transport's problem: it
            // is answered here and the acceptor waits for the next connection.
            // Surfacing it would make every handler a parser's error path.
            Err(status) => {
                let _ = refuse(&mut stream, status, "");
                continue;
            }
        };
        // The request is claimed against the limit *here*, which is what makes
        // `requestLimit` mean the same thing with sixty-four workers as with
        // one — and the worker that takes the last one is the worker that ends
        // the server, so it wakes the others on its way past.
        let claimed = {
            let mut gate = listening.gate();
            if gate.finished {
                None
            } else {
                gate.served = gate.served.saturating_add(1);
                let spent = listening.plan.limit.is_some_and(|n| gate.served >= n);
                // Set, never cleared: `finished` is a one-way door, and a claim
                // that does not spend the limit must not reopen it.
                gate.finished = gate.finished || spent;
                Some(if spent { gate.waiting } else { 0 })
            }
        };
        let Some(waiting) = claimed else {
            // Another worker took the last slot while this one was reading, so
            // there is no handler for this request. The client is told rather
            // than left waiting for an answer nobody was handed.
            let _ = refuse(&mut stream, 503, "");
            return Err(ServeErr::closed());
        };
        listening.wake(waiting);
        let connection = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
        connections(|table| {
            table.insert(
                connection,
                Pending { listener: handle, stream, request: Some(request) },
            )
        });
        return Ok(connection);
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

/// Wait for one connection, honouring the idle deadline where there is one.
///
/// **The listener's table lock is not held across the wait.** It used to be,
/// when there was one worker and this wait was the only thing happening; a
/// worker waiting here would otherwise hold every other worker out of
/// `listenRespond` and `listenClose` too. The caller holds
/// [`Listening::turn`] instead, which is a lock over *this* socket and nothing
/// else.
fn wait_for_connection(listening: &Listening) -> Result<TcpStream, ServeErr> {
    let Some(idle) = listening.plan.idle else {
        // No deadline: an ordinary blocking accept, which is what a server that
        // did not ask to be interruptible is. [`Listening::wake`] is what gets
        // it back out.
        return listening
            .listener
            .accept()
            .map(|(s, _)| s)
            .map_err(|e| ServeErr::io(&e, "accepting"));
    };
    let until = Instant::now() + idle;
    loop {
        match listening.listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(ServeErr::io(&e, "accepting")),
        }
        // An interruptible listener needs no dial: this poll is the check, which
        // is why [`Listening::wake`] does nothing when there is a deadline.
        if listening.gate().finished {
            return Err(ServeErr::closed());
        }
        if Instant::now() >= until {
            return Err(ServeErr::new(
                ServeFail::Timeout,
                format!("no connection arrived within {} ms", idle.as_millis()),
            ));
        }
        std::thread::sleep(POLL.min(until.saturating_duration_since(Instant::now())));
    }
}

/// Both socket deadlines, from the plan's one number. Per step, as
/// `http.rs::DEADLINE` is and for its reason: `SO_RCVTIMEO` is per read.
fn deadlines(stream: &TcpStream, head: Duration) -> Result<(), ServeErr> {
    stream
        .set_read_timeout(Some(head))
        .and_then(|()| stream.set_write_timeout(Some(head)))
        .map_err(|e| ServeErr::io(&e, "setting the connection's deadlines"))
}

/// Read one request off a connection, or answer with the status it deserves.
///
/// The `Err` is a status code rather than a `ServeErr` on purpose: nothing here
/// is the *server's* failure, so none of it is a reason for `serve` to return.
fn read_request(stream: &mut TcpStream, body_limit: usize) -> Result<ServerRequest, u16> {
    let mut buffer = Vec::with_capacity(1024);
    let head = loop {
        if let Some(at) = find(&buffer, b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > 64 * 1024 {
            return Err(431);
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
fn refuse(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
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
    let Some(Pending { mut stream, .. }) = taken else { return Ok(()) };
    let status = status.clamp(100, 599) as u16;
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (name, value) in headers {
        // A header the acceptor writes for itself is not a header the caller
        // may write twice: framing is the transport's and a second
        // `content-length` is a request smuggling primitive.
        if name == "content-length" || name == "connection" || name == "transfer-encoding" {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", body.len()));
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .map_err(|e| ServeErr::io(&e, "writing the response"))
}

/// `Listen::listenClose`. Closing twice has nothing to do, which is what lets a
/// loop unwinding out of a failure tidy up without remembering how far it got.
///
/// **It is also how one worker stops the others**, and that is new. A worker
/// whose accept or respond failed calls this before it returns its error, so the
/// workers blocked beside it are woken and answered `.Closed` rather than
/// waiting out a server that has already failed. What makes it safe to call
/// while somebody is inside an accept is [`ACCEPTORS`]'s `Arc`: the entry leaves
/// the table now and the socket closes when the last worker leaves the call.
pub fn close(handle: i64) {
    let Some(listening) = acceptors(|table| table.remove(&handle)) else { return };
    let waiting = listening.finish();
    listening.wake(waiting);
    // A connection no worker will be handed back is a socket nobody will answer,
    // so it is dropped here rather than held until the process ends. The client
    // sees the connection close, which is what a server going down looks like
    // from the outside.
    connections(|table| table.retain(|_, pending| pending.listener != handle));
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
/// leaves, the port, the protocol list's `(ptr, len)`, two `Int` knobs, and
/// `Result`'s two out-pointers. An empty address, an empty protocol list and a
/// negative knob are all "the caller chose nothing" — the encoding
/// `core/net/server` renders a `Server`'s `.None` fields into, and the reason
/// [`HEAD_DEADLINE`] and [`BODY_LIMIT`] are this file's constants rather than
/// two more parameters.
///
/// # Safety
/// The address view and the `[Protocol]` must be live; both out-pointers
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
    let protocols: &[u8] = if pptr.is_null() || plen == 0 {
        &[]
    } else {
        // SAFETY: the caller promises `plen` live elements, and a `Protocol`'s
        // stride is one byte because its tag is one byte.
        unsafe { std::slice::from_raw_parts(pptr, plen as usize) }
    };
    // **A suspension point** (`rt.rs` §2), for `buri_rt_host_net_fetch`'s
    // reason: the bind is a syscall that can block on name resolution, and a
    // carrier is not the caller's to lose. The Buri blocks below are built
    // after the park returns, on the carrier and under the baton.
    let outcome = park(|| bind(&address, port, protocols, limit, idle));
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

// The three `Sockets` entries. Nothing hands out a socket yet — `Listen`
// performs no WebSocket upgrade — so every handle these can be given is one a
// program made up, and an unknown socket is one that has already gone away.
// That is the behaviour `effect Sockets` declares for a closed socket, which is
// what makes granting the authority beside `Listen` honest rather than a hole:
// the bodies are here, they are reached through the ordinary door, and what
// they do to a handle that names nothing is what they will do to a handle whose
// socket closed while a message was in flight.

/// `Sockets::socketSendText(socket, text)`.
///
/// # Safety
/// The text view must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_sockets_socket_send_text(
    _socket: i64,
    _base: *mut u8,
    _ptr: *const u8,
    _len: u64,
) {
}

/// `Sockets::socketSendBytes(socket, body)`.
///
/// # Safety
/// The `[U8]` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_sockets_socket_send_bytes(
    _socket: i64,
    _ptr: *const u8,
    _len: u64,
) {
}

/// `Sockets::socketClose(socket, code, reason)`.
///
/// # Safety
/// The reason view must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_sockets_socket_close(
    _socket: i64,
    _code: i64,
    _base: *mut u8,
    _ptr: *const u8,
    _len: u64,
) {
}

/// Run one blocking step off the run baton where there is a reactor to park on,
/// and inline where there is not.
///
/// The three waiting `Listen` entries all want the same two lines and the same
/// paragraph of reasoning (`host.rs`'s `buri_rt_host_net_fetch` is where it is
/// written out), so they say it once here. The transport underneath is
/// synchronous either way — the future completes on this thread — and what
/// `park_on` adds is that the carrier is not holding the baton while a server
/// waits for a client, which is the longest wait in this file.
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

    /// The acceptor speaks HTTP/1.1 and says so about the other two, and the
    /// two refusals are different sentences.
    ///
    /// [`serves`] is about the *archive* — was the QUIC code compiled in — and
    /// [`accepts`] is about this file. A user can act on the first (rebuild the
    /// toolchain) and can only wait for the second, so HTTP/3 on a toolchain
    /// without `net-h3` has to produce the first message and not the second.
    #[test]
    fn the_acceptor_speaks_http1_and_names_what_it_does_not() {
        assert_eq!(accepts(Protocol::Http1), Ok(()));
        let two = accepts(Protocol::Http2).expect_err("HTTP/2 is not framed here");
        assert!(two.contains("HTTP/2"), "{two}");
        assert!(two.contains("one request per connection"), "{two}");
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
        let request = read_request(&mut accepted, 1024).expect("a whole request");
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
        let status = read_request(&mut accepted, limit).unwrap_err();
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
            bind("127.0.0.1", 0, &[Protocol::Http1 as u8], 1, 10_000).expect("a bound port");
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
        let (handle, port, _handlers) = bind("127.0.0.1", 0, &[], -1, 60).expect("a bound port");
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

    /// A protocol the acceptor does not frame is refused **at the bind**, with
    /// a message rather than a tag, and no port is held.
    #[test]
    fn a_protocol_this_acceptor_does_not_frame_is_refused_at_the_bind() {
        let refused = bind("127.0.0.1", 0, &[Protocol::Http2 as u8], -1, 60)
            .expect_err("HTTP/2 is not framed here");
        assert_eq!(refused.cause, ServeFail::Unsupported);
        assert!(refused.detail.contains("HTTP/2"), "{}", refused.detail);
        // And an index no variant has is a program and a runtime disagreeing
        // about the enum, which is refused rather than transmuted.
        let unknown = bind("127.0.0.1", 0, &[9], -1, 60).expect_err("there is no ninth protocol");
        assert_eq!(unknown.cause, ServeFail::Unsupported);
        assert!(unknown.detail.contains("disagree"), "{}", unknown.detail);
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
        let (handle, _port, handlers) = bind("127.0.0.1", 0, &[], -1, 60).expect("a bound port");
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
            bind("127.0.0.1", 0, &[Protocol::Http1 as u8], WORKERS as i64, 20_000)
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

    /// **A blocking accept is interruptible**, which is what lets one worker
    /// stop the others.
    ///
    /// The listener has no idle deadline, so its workers are inside a blocking
    /// `accept(2)` with nothing to wake them; `close` from another thread has to
    /// get them out, and [`Listening::wake`]'s dial is how. Without it this test
    /// hangs, which is exactly the failure the wakeup exists to prevent — so the
    /// wait is bounded by a joined thread and a deadline of its own.
    #[test]
    fn closing_a_listener_wakes_the_workers_blocked_on_it() {
        let (handle, _port, _handlers) = bind("127.0.0.1", 0, &[], -1, -1).expect("a bound port");
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
            bind("127.0.0.1", 0, &[Protocol::Http1 as u8], 1, -1).expect("a bound port");
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
}
