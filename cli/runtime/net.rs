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
//! **The acceptor below does not change that, and that is a decision.** A
//! server is the obvious first caller of `hyper`, and the acceptor frames
//! HTTP/1.1 itself instead — for `http.rs`'s reason, read from the other side
//! of the wire. A synchronous exchange over one connection at a time is the
//! whole of what is being asked for here, and reaching a framing layer through
//! `hyper` would mean standing up a reactor per connection to get at something
//! two hundred lines already do. The day handlers run on tasks of their own —
//! many connections in flight, h2 multiplexing, ALPN — is the day that decision
//! is worth taking again, and it is the day the archive starts carrying
//! `hyper`'s bytes and `.github/scripts/assert-runtime-archive.sh` moves
//! `hyper` from its absent list to its present one.
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
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
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
/// acceptor is [`Acceptor`]: an HTTP/1.1 server that frames its own messages,
/// one connection at a time.
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
            "{} is not spoken by this runtime's acceptor: it frames HTTP/1.1 and answers one \
             connection at a time. Serve HTTP/1.1, or leave `protocols` unset",
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
// the socket is dropped after it, which is what makes "the request `accept`
// last handed out" a well-defined thing to respond to without an identifier.
// Keep-alive is a multiplexing question and belongs with the tasks.
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

/// An open port, and the one connection it is in the middle of.
struct Acceptor {
    listener: TcpListener,
    plan: Plan,
    served: u64,
    /// The connection whose request `listenAccept` last handed out, waiting for
    /// its response. `None` between requests, which is what makes a second
    /// `listenRespond` a no-op rather than a write onto a dead socket.
    pending: Option<TcpStream>,
}

/// The open listeners, by handle.
///
/// A table rather than a pointer across the ABI: a handle is an `Int` in Buri
/// and an index here, so a program cannot hand the runtime an address, and a
/// stale one is a lookup that misses rather than a use-after-free.
static ACCEPTORS: Mutex<Option<HashMap<i64, Acceptor>>> = Mutex::new(None);

/// The next handle. Never reused, so a handle closed and a handle never opened
/// are the same answer — `.Err(.Closed)` — and neither is another server's.
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn acceptors<T>(with: impl FnOnce(&mut HashMap<i64, Acceptor>) -> T) -> T {
    let mut guard = match ACCEPTORS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    with(guard.get_or_insert_with(HashMap::new))
}

/// `Listen::listenBind`, without the ABI around it.
pub fn bind(
    address: &str,
    port: i64,
    protocols: &[u8],
    limit: i64,
    idle_millis: i64,
) -> Result<(i64, u16), ServeErr> {
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
    let bound = listener
        .local_addr()
        .map_err(|e| ServeErr::io(&e, "asking the socket which port it got"))?
        .port();
    let plan = Plan {
        limit: (limit >= 0).then_some(limit as u64),
        idle: (idle_millis >= 0).then_some(Duration::from_millis(idle_millis.max(0) as u64)),
        head: HEAD_DEADLINE,
        body_limit: BODY_LIMIT,
    };
    // Interruptible only where the caller asked for it: an idle deadline is the
    // one thing a blocking `accept` cannot answer, and polling for a server
    // that never wanted a deadline would be a hundred wakeups a second bought
    // for nothing.
    if plan.idle.is_some() {
        listener
            .set_nonblocking(true)
            .map_err(|e| ServeErr::io(&e, "making the listener interruptible"))?;
    }
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    acceptors(|table| table.insert(handle, Acceptor { listener, plan, served: 0, pending: None }));
    Ok((handle, bound))
}

/// `Listen::listenAccept` — the next request, once one has arrived and been
/// read whole.
///
/// The loop is over *connections* and not over requests: a connection that
/// speaks nonsense, or asks for more body than the plan allows, is answered
/// here and the next one is waited for, so a handler is never handed a message
/// the acceptor could not make sense of.
pub fn accept(handle: i64) -> Result<ServerRequest, ServeErr> {
    loop {
        let socket = acceptors(|table| {
            let Some(a) = table.get_mut(&handle) else { return Err(ServeErr::closed()) };
            if a.plan.limit.is_some_and(|n| a.served >= n) {
                return Err(ServeErr::closed());
            }
            Ok(())
        });
        socket?;
        let stream = wait_for_connection(handle)?;
        let plan = acceptors(|table| {
            table.get(&handle).map(|a| (a.plan.head, a.plan.body_limit)).ok_or_else(ServeErr::closed)
        })?;
        let mut stream = stream;
        if let Err(e) = deadlines(&stream, plan.0) {
            let _ = refuse(&mut stream, 500, "");
            return Err(e);
        }
        match read_request(&mut stream, plan.1) {
            Ok(request) => {
                acceptors(|table| {
                    if let Some(a) = table.get_mut(&handle) {
                        a.pending = Some(stream);
                    }
                });
                return Ok(request);
            }
            // A malformed or oversized request is the transport's problem: it
            // is answered here and the acceptor waits for the next connection.
            // Surfacing it would make every handler a parser's error path.
            Err(status) => {
                let _ = refuse(&mut stream, status, "");
            }
        }
    }
}

/// Wait for one connection, honouring the idle deadline where there is one.
fn wait_for_connection(handle: i64) -> Result<TcpStream, ServeErr> {
    let idle = acceptors(|table| {
        table.get(&handle).map(|a| a.plan.idle).ok_or_else(ServeErr::closed)
    })?;
    let Some(idle) = idle else {
        // No deadline: an ordinary blocking accept, which is what a server that
        // did not ask to be interruptible is.
        return acceptors(|table| {
            let a = table.get(&handle).ok_or_else(ServeErr::closed)?;
            a.listener.accept().map(|(s, _)| s).map_err(|e| ServeErr::io(&e, "accepting"))
        });
    };
    let until = Instant::now() + idle;
    loop {
        let attempt = acceptors(|table| {
            let a = table.get(&handle).ok_or_else(ServeErr::closed)?;
            match a.listener.accept() {
                Ok((s, _)) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(ServeErr::io(&e, "accepting")),
            }
        })?;
        if let Some(stream) = attempt {
            return Ok(stream);
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

/// `Listen::listenRespond` — the answer to the request `accept` last handed
/// out.
///
/// A response with no request outstanding is `.Ok(())` and writes nothing: the
/// pairing is positional, so "there is nothing to answer" is a state a loop can
/// be in without having done anything wrong.
pub fn respond(
    handle: i64,
    status: i64,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(), ServeErr> {
    let taken = acceptors(|table| {
        let Some(a) = table.get_mut(&handle) else { return Err(ServeErr::closed()) };
        a.served = a.served.saturating_add(1);
        Ok(a.pending.take())
    })?;
    let Some(mut stream) = taken else { return Ok(()) };
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
pub fn close(handle: i64) {
    acceptors(|table| table.remove(&handle));
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
// Four `Listen` entries and three `Sockets` ones, at `lib.rs` §1's naming rule
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

/// `Listener` — `{ handle: Int, port: Int }`.
#[repr(C)]
pub struct BuriListener {
    handle: i64,
    port: i64,
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
        Ok((handle, bound)) => {
            // SAFETY: the caller promises a writable destination.
            unsafe { out.write(BuriListener { handle, port: i64::from(bound) }) };
            crate::BURI_OK
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { err.write(BuriServeError::of(&e)) };
            0
        }
    }
}

/// `Listen::listenAccept(listener) -> Result<Request, ServeError>`.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_accept(
    handle: i64,
    out: *mut BuriRequest,
    err: *mut BuriServeError,
) -> i32 {
    // **A suspension point**, and the one that waits longest: this is where a
    // server sits between requests.
    match park(|| accept(handle)) {
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

/// `Listen::listenRespond(listener, response) -> Result<(), ServeError>`.
///
/// `Response`'s three fields arrive flattened, and `.Ok`'s payload is `()` — so
/// there is no out-pointer for it, on `lib.rs` §2.1's zero-sized rule.
///
/// # Safety
/// The `[Header]` and the `[U8]` must be live; `err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_listen_respond(
    handle: i64,
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
    match park(|| respond(handle, status, &fields, body)) {
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
/// The three `Listen` entries all want the same two lines and the same
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
        assert!(two.contains("one connection at a time"), "{two}");
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

        // `Listener { handle: Int, port: Int }`.
        assert_eq!(std::mem::size_of::<BuriListener>(), 16);
        assert_eq!(std::mem::offset_of!(BuriListener, port), 8);

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
        let (handle, port) =
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

        let request = accept(handle).expect("one request");
        assert_eq!(request.target, "/ping");
        respond(handle, 200, &[("x-from".into(), "buri".into())], b"pong").expect("a response");

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
        let (handle, port) = bind("127.0.0.1", 0, &[], -1, 60).expect("a bound port");
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
}
