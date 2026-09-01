//! A minimal HTTP/1.1 client, for `Net::fetch`.
//!
//! Synchronous, and now alone in it. A request made through this client blocks
//! the thread it was made on; the JavaScript half no longer does, because
//! `$host_HostNet_fetch` awaits the platform's own `fetch` (`runtime.js`). What
//! makes the difference is a runtime that can suspend a call, which the native
//! backends do not yet have — not the shape of the request, which is the same
//! `Request` on both sides.
//!
//! ## Scope
//!
//! **`http://` and `https://`.** Absolute-URI parsing, `Host`, the caller's own
//! request headers, octet request bodies with `Content-Length`, chunked and
//! identity response bodies, the response's header fields, and a deadline on
//! every step of the exchange — over cleartext, or over TLS 1.2 and 1.3 with
//! the server's certificate checked against the host's trust anchors.
//!
//! **Every step**, and the word is load-bearing: the name lookup, the connect,
//! the TLS handshake, each write and each read. [`DEADLINE`] says what that is
//! worth and [`within`] is how the one step the socket options cannot bound —
//! `getaddrinfo` — is bounded anyway. A `fetch` that could hang forever would
//! hang a carrier forever, and a carrier is not the caller's to lose.
//!
//! **The scheme changes one thing and one thing only: whether the socket is
//! wrapped.** [`Transport`] is the seam, `tls.rs` is what fills it, and
//! everything from the request line to the dechunker is the same code on both
//! sides. That is deliberate and it is a *testable* claim rather than an
//! intention: the cleartext path could not have regressed when `https://`
//! landed, because there is no cleartext path to regress — there is one path
//! and a wrapper.
//!
//! What is *not* here is HTTP/2. `hyper` is in the runtime's manifest and this
//! client does not use it: a synchronous exchange over one connection is the
//! whole of what `Net.fetch` is until the carrier runtime exists (design/native
//! track B), and until then a `hyper` client would mean standing up a `tokio`
//! reactor per request in order to reach a framing layer this file already has.
//! The day `fetch` can suspend, that decision is worth taking again.
//!
//! `tls.rs` owns the other half of the story — which certificates are trusted,
//! where they come from, and what every refusal says.
//!
//! ## The two enums that cross as indices
//!
//! [`NetFail`] and [`METHODS`] are transcriptions of `NetError` and `Method` in
//! `effect.buri`. Neither is a copy of a *name*: what crosses the ABI is the
//! variant index, so what has to agree is the order. `GET` appears in
//! [`METHODS`] and nowhere else in the runtime, and nowhere at all in Buri —
//! the same rule `CloseReason`'s wire codes follow.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// `NetError`'s variants, in declaration order in `effect.buri`. The index is
/// what crosses the ABI, so this order is the contract.
///
/// `Aborted` has no producer here: nothing in this runtime can call a request
/// off yet. It is in the enum because the *index* is the contract — a variant
/// appended later is free, one inserted in the middle silently renumbers the
/// rest — so the tag it will use is claimed now.
pub enum NetFail {
    Timeout,
    Refused,
    BadUrl(String),
    Transport(String),
    #[allow(dead_code)]
    Aborted,
}

impl NetFail {
    pub fn tag(&self) -> i32 {
        match self {
            NetFail::Timeout => 0,
            NetFail::Refused => 1,
            NetFail::BadUrl(_) => 2,
            NetFail::Transport(_) => 3,
            NetFail::Aborted => 4,
        }
    }

    /// The payload of the two variants that carry one; empty for the three
    /// that do not, which is what the backend writes into an unused `Str`
    /// slot.
    pub fn message(&self) -> &str {
        match self {
            NetFail::Timeout | NetFail::Refused | NetFail::Aborted => "",
            NetFail::BadUrl(m) | NetFail::Transport(m) => m,
        }
    }
}

/// `Method`'s variants, in declaration order in `effect.buri`. The index is
/// what crosses the ABI, and this table is the only place in the native
/// runtime where a wire spelling is written down — `CloseReason`'s philosophy,
/// applied to the request line.
pub const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// The wire spelling of a `Method` tag. An index this runtime does not know is
/// unreachable from generated code — the enum is closed and the compiler
/// writes the tag — and answering `GET` is a refusal to invent a verb rather
/// than a claim it cannot happen.
pub fn method_name(tag: i32) -> &'static str {
    METHODS.get(tag.max(0) as usize).copied().unwrap_or("GET")
}

/// A whole HTTP response — `Response`'s three fields.
pub struct HttpResponse {
    pub status: i64,
    /// Field names lowercased, as `Header` states they are.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// How long any one step of a request may take before it is a `Timeout`.
///
/// **Every** step: the name lookup, the connect, the handshake, each write and
/// each read. That is what makes "a `Net.fetch` returns" a property of this
/// client rather than a property of the network it is pointed at — a peer that
/// accepts and says nothing, a route that swallows the SYN, a resolver that
/// never answers, all end here with the same answer instead of holding the
/// calling thread for as long as the process lives.
///
/// It is per step and not a budget across them, which is the shape the socket
/// options have — `SO_RCVTIMEO` is per read — and making the whole exchange
/// share one deadline would mean re-deriving the remainder before every call.
/// The bound that matters is that there *is* one.
///
/// **With one exception, and it is the step that is a loop.** Every other step
/// here happens once, so bounding the step bounds it; reading the response does
/// not. `SO_RCVTIMEO` is restarted by every successful read, so a server
/// sending a byte just inside the deadline is never late and never finishes,
/// and per-step is then not a bound at all. [`read_to_end`] therefore carries
/// this number a second way — as a budget across the whole read — beside
/// [`RESPONSE_LIMIT`], which is the same hole closed in bytes. `net.rs`'s
/// `HEAD_DEADLINE` states the identical pair from the server's side of the
/// wire.
const DEADLINE: Duration = Duration::from_secs(30);

/// The largest response this client will read.
///
/// **The server side has had one since F2 and the client side had none**:
/// `net.rs`'s `BODY_LIMIT` refuses an over-large request body with a `413`, and
/// nothing here refused an over-large *response*. A peer that answers with an
/// endless body is not exotic — a misconfigured stream, a proxy error page that
/// never ends, a host that is not the host it was supposed to be — and without
/// a cap the only thing that ends the read is the process running out of
/// memory.
///
/// The same eight mebibytes `BODY_LIMIT` names, because the two are the same
/// question asked in the two directions and a client that would not *serve* a
/// larger body has no argument for accepting one. A caller that wants a large
/// response wants streaming, which is a `Response` shape this client does not
/// have rather than a bigger number.
const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;

struct Url<'a> {
    authority: &'a str,
    host: &'a str,
    port: u16,
    target: &'a str,
    /// Whether the socket is wrapped before a byte of HTTP crosses it. The
    /// *only* thing the scheme changes: everything below this line writes and
    /// reads the same request and the same response either way.
    tls: bool,
}

fn parse(url: &str) -> Result<Url<'_>, NetFail> {
    let (rest, tls) = match (url.strip_prefix("http://"), url.strip_prefix("https://")) {
        (Some(r), _) => (r, false),
        (_, Some(r)) => (r, true),
        _ => return Err(NetFail::BadUrl(format!("not an absolute http URL: {url}"))),
    };
    // A toolchain built without the runtime's `net` feature has no TLS code in
    // it at all, and the refusal names the feature because with the feature off
    // that is the true and complete reason. It is a *run-time* refusal rather
    // than the compile-time one `Backend::missing_intrinsics` gives the server
    // and task intrinsics, and deliberately: the cleartext half of this client
    // needs no crate and goes on working, so refusing every program that
    // mentions `Net.fetch` would be refusing programs that were never going to
    // need TLS.
    #[cfg(not(feature = "net"))]
    if tls {
        return Err(NetFail::Transport(
            "https is not supported by this toolchain's native runtime: it was built without the \
             runtime's `net` feature, so it carries no TLS code. `Net.fetch` speaks cleartext \
             http only"
                .to_string(),
        ));
    }
    let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, target) = rest.split_at(split);
    if authority.is_empty() {
        return Err(NetFail::BadUrl(format!("no host in URL: {url}")));
    }
    // A userinfo section is not supported and is not silently dropped: an
    // implementation that ignored credentials would send an unauthenticated
    // request and report the server's refusal, which is the wrong error.
    if authority.contains('@') {
        return Err(NetFail::BadUrl(format!("userinfo in a URL is not supported: {url}")));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(n) => (h, n),
            Err(_) => return Err(NetFail::BadUrl(format!("bad port in URL: {url}"))),
        },
        None => (authority, if tls { 443 } else { 80 }),
    };
    Ok(Url { authority, host, port, target: if target.is_empty() { "/" } else { target }, tls })
}

/// The socket, and — for `https://` — the TLS session over it.
///
/// One `Read + Write` either way, which is the whole point: the request writer
/// and the response reader below take a `&mut Transport` and have no scheme in
/// them. The `Box` is because a `rustls` connection is a few kilobytes of
/// buffers and this enum is a local in [`fetch`].
enum Transport {
    Plain(TcpStream),
    #[cfg(feature = "net")]
    Tls(Box<crate::tls::TlsStream>),
}

impl Read for Transport {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(buffer),
            #[cfg(feature = "net")]
            Transport::Tls(s) => s.read(buffer),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(buffer),
            #[cfg(feature = "net")]
            Transport::Tls(s) => s.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            #[cfg(feature = "net")]
            Transport::Tls(s) => s.flush(),
        }
    }
}

fn io_fail(e: &std::io::Error) -> NetFail {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => NetFail::Refused,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetFail::Timeout,
        _ => NetFail::Transport(e.to_string()),
    }
}

/// The address to dial, found within the deadline.
///
/// **A name lookup is the one step of a dial that the socket options cannot
/// bound.** `connect_timeout` takes a `Duration`, `SO_RCVTIMEO` and
/// `SO_SNDTIMEO` bound the reads and the writes, and `getaddrinfo` — which is
/// what `to_socket_addrs` is on every host this runtime targets — takes as long
/// as the resolver takes. On a machine whose DNS is answered that is a
/// millisecond; on one whose packets to the resolver are dropped it is minutes,
/// and on one whose resolver is gone it can be *never*. A `Net.fetch` that
/// never returns is not a slow program, it is a stuck one, and the calling
/// thread is a carrier that nothing can take back.
///
/// So the lookup happens on a thread of its own and this one waits for it with
/// [`within`]. An address literal — which is what every loopback probe in this
/// repository and every URL with an IP in it is — skips the whole arrangement:
/// there is nothing to ask anybody.
fn resolve(host: &str, port: u16, deadline: Duration) -> Result<SocketAddr, NetFail> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let name = host.to_string();
    let found = within(deadline, "the name lookup", move || {
        (name.as_str(), port)
            .to_socket_addrs()
            .map(|addrs| addrs.collect::<Vec<_>>())
            .map_err(|e| e.to_string())
    })?;
    match found {
        Err(e) => Err(NetFail::Transport(format!("could not resolve {host}: {e}"))),
        Ok(addrs) => addrs
            .into_iter()
            .next()
            .ok_or_else(|| NetFail::Transport(format!("no address for {host}"))),
    }
}

/// Run `work` on a thread of its own and wait `deadline` for its answer.
///
/// The deadline is on the *wait*, not on the work: nothing here can cancel a
/// `getaddrinfo` that is already in the kernel, and pretending otherwise would
/// be a worse lie than the hang. What it can do is stop the caller waiting on
/// it, which is the difference between a request that fails and a process that
/// has to be killed. The abandoned thread holds a `Sender` and a name, finishes
/// whenever the resolver lets it, and sends into a channel nobody is listening
/// to; a process that exits before then takes it with it.
///
/// `work` is a parameter rather than the lookup written inline because that is
/// what makes the bound testable — a test hands it something that never
/// finishes and gets its answer in milliseconds, which is precisely the case
/// the network cannot be asked to reproduce on demand.
fn within<T: Send + 'static>(
    deadline: Duration,
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, NetFail> {
    let (answer, wait) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(String::from("buri-rt-net"))
        .spawn(move || {
            let _ = answer.send(work());
        })
        .map_err(|e| {
            NetFail::Transport(format!("{what} needs a thread, and one could not be started: {e}"))
        })?;
    match wait.recv_timeout(deadline) {
        Ok(value) => Ok(value),
        Err(RecvTimeoutError::Timeout) => Err(NetFail::Timeout),
        // The thread ended without sending, which for a closure that cannot
        // return early means it panicked. Its own message has already been
        // printed by the panic hook; this is the caller being told that the
        // step has no answer rather than being told to keep waiting.
        Err(RecvTimeoutError::Disconnected) => {
            Err(NetFail::Transport(format!("{what} ended without an answer")))
        }
    }
}

/// One request, one response.
///
/// `method` is a `Method` tag, `headers` the request's own fields — sent after
/// the four this client writes for itself, so a caller may add and may not
/// silently replace — and `body` its octets.
pub fn fetch(
    method: i32,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<HttpResponse, NetFail> {
    fetch_within(DEADLINE, method, url, headers, body)
}

/// [`fetch`], with the deadline as a parameter.
///
/// The parameter exists so that "the dial is bounded" can be *asserted* rather
/// than read: a test that wanted to see [`DEADLINE`] fire would have to wait
/// thirty seconds for it, and a test nobody runs is the state the hang below
/// was found in. The public entry passes the constant, so there is exactly one
/// policy and one caller of it.
fn fetch_within(
    deadline: Duration,
    method: i32,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<HttpResponse, NetFail> {
    let url = parse(url)?;

    let addr = resolve(url.host, url.port, deadline)?;

    let sock = TcpStream::connect_timeout(&addr, deadline).map_err(|e| io_fail(&e))?;
    sock.set_read_timeout(Some(deadline)).map_err(|e| io_fail(&e))?;
    sock.set_write_timeout(Some(deadline)).map_err(|e| io_fail(&e))?;
    // A request/response exchange is one write and one read, so Nagle can only
    // add a round trip's worth of delay to the front of it.
    let _ = sock.set_nodelay(true);
    // The deadlines are set on the `TcpStream` *before* it is wrapped, so they
    // cover the handshake as well as the exchange: a peer that accepts the
    // connection and then says nothing is a `Timeout` rather than a hang.
    let mut sock = wrap(sock, &url)?;

    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: buri\r\nAccept: */*\r\nConnection: close\r\n",
        method_name(method),
        url.target,
        url.authority
    );
    if !body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (name, value) in headers {
        // A field with a newline in it would be a second header, or a second
        // request. Refused rather than sanitized: a caller who wrote one meant
        // something, and quietly sending a different thing is worse than not
        // sending it.
        if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            return Err(NetFail::Transport(format!("header `{name}` is not a header field")));
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut request = head.into_bytes();
    request.extend_from_slice(body);
    sock.write_all(&request).map_err(|e| io_fail(&e))?;
    sock.flush().map_err(|e| io_fail(&e))?;

    let mut raw = Vec::new();
    // The budget starts at the read rather than at the dial: every step before
    // this one has already had its own [`DEADLINE`], and a response is not late
    // because the name lookup was slow.
    read_to_end(&mut sock, &mut raw, Instant::now() + deadline, RESPONSE_LIMIT)?;
    parse_response(&raw)
}

/// Wrap the socket if the URL asked for TLS, and hand it back untouched if it
/// did not.
///
/// The `cfg` is on the *arm* rather than on the function: with the `net`
/// feature off, [`parse`] has already refused every `https://` URL by name, so
/// this branch is unreachable and says so rather than being a second refusal
/// with a second message to keep in step.
fn wrap(sock: TcpStream, url: &Url<'_>) -> Result<Transport, NetFail> {
    if !url.tls {
        return Ok(Transport::Plain(sock));
    }
    #[cfg(feature = "net")]
    {
        // `tls::connect` answers a `NetFail` rather than a sentence, because a
        // handshake has two ways to fail that a caller answers differently: a
        // certificate that did not check out is a `Transport` carrying the
        // refusal, and a peer that stopped talking part way through it is the
        // same `Timeout` a stalled read is. A `String` could only have been the
        // first of those.
        let stream = crate::tls::connect(sock, url.host)?;
        Ok(Transport::Tls(Box::new(stream)))
    }
    #[cfg(not(feature = "net"))]
    unreachable!("parse() refuses https:// when the runtime has no TLS code")
}

/// Read until the peer stops talking.
///
/// `Read::read_to_end` in all but one respect, and the one respect is why it is
/// written out: a TLS peer that closes the TCP connection **without** sending
/// `close_notify` makes `rustls` answer `UnexpectedEof`, and a great many HTTP
/// servers closing a `Connection: close` exchange do exactly that. The body has
/// already arrived by then — it is delimited by `Content-Length` or by the
/// terminating chunk — so treating that as the end of the message is right, and
/// treating it as an error would refuse a response the caller can see is whole.
///
/// For a plain `TcpStream` the case cannot arise, so `http://` reads exactly as
/// it always did.
///
/// **The two bounds are on the loop and not on the read**, which is the whole
/// reason they are parameters. Each `read` is bounded already by the socket's
/// `SO_RCVTIMEO`; what is not bounded by that is how many times round this
/// loop goes, and the answer without `until` is "until the peer stops", which a
/// peer choosing to drip need never do. [`DEADLINE`] and [`RESPONSE_LIMIT`]
/// argue the two numbers. They are parameters rather than the constants for
/// `fetch_within`'s reason: a test that had to wait thirty seconds to watch a
/// bound fire is a test nobody runs.
///
/// `&mut impl Read` rather than `&mut Transport` for the same reason — a
/// [`Transport`] is a socket or a TLS session over one, and neither is a thing
/// a test can make drip on demand.
fn read_to_end(
    sock: &mut impl Read,
    out: &mut Vec<u8>,
    until: Instant,
    limit: usize,
) -> Result<(), NetFail> {
    let mut buffer = [0_u8; 8192];
    loop {
        if Instant::now() >= until {
            return Err(NetFail::Timeout);
        }
        if out.len() > limit {
            return Err(NetFail::Transport(format!(
                "the response is larger than this client will read ({limit} bytes)"
            )));
        }
        match sock.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => out.extend_from_slice(buffer.get(..n).unwrap_or(&[])),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(io_fail(&e)),
        }
    }
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, NetFail> {
    let split = find(raw, b"\r\n\r\n")
        .ok_or_else(|| NetFail::Transport("truncated response: no header terminator".to_string()))?;
    let (head, rest) = raw.split_at(split);
    let body = rest.get(4..).unwrap_or(&[]);
    let head = String::from_utf8_lossy(head);
    let mut lines = head.split("\r\n");

    let status_line = lines
        .next()
        .ok_or_else(|| NetFail::Transport("empty response".to_string()))?;
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| NetFail::Transport(format!("bad status line: {status_line}")))?;

    let mut length = None;
    let mut chunked = false;
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
        // Lowercased on the way in, which is what `Header` promises a program
        // reading one back. A server may send any casing it likes.
        fields.push((name.trim().to_ascii_lowercase(), value.to_string()));
    }

    let bytes = if chunked {
        dechunk(body)?
    } else {
        match length {
            Some(n) => body.get(..n.min(body.len())).unwrap_or(body).to_vec(),
            None => body.to_vec(),
        }
    };
    Ok(HttpResponse { status, headers: fields, body: bytes })
}

fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, NetFail> {
    let mut out = Vec::new();
    loop {
        let end = find(body, b"\r\n")
            .ok_or_else(|| NetFail::Transport("truncated chunked body".to_string()))?;
        let header = String::from_utf8_lossy(body.get(..end).unwrap_or(&[]));
        let size_text = header.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| NetFail::Transport(format!("bad chunk size: {size_text}")))?;
        let after = body.get(end + 2..).unwrap_or(&[]);
        if size == 0 {
            return Ok(out);
        }
        let chunk = after
            .get(..size)
            .ok_or_else(|| NetFail::Transport("truncated chunk".to_string()))?;
        out.extend_from_slice(chunk);
        body = after.get(size + 2..).unwrap_or(&[]);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    (0..=last).find(|i| haystack.get(*i..*i + needle.len()) == Some(needle))
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a bounded step is allowed to take before the claim this file
    /// makes — that it is bounded — is the thing that failed.
    ///
    /// Two orders of magnitude above the deadlines the cases below set, so a
    /// loaded machine is not a failing one, and three below the sixty-minute
    /// job timeout that used to be how a hang here was reported.
    const SOON: Duration = Duration::from_secs(5);

    /// A step that never finishes ends at the deadline rather than at the end
    /// of the process.
    ///
    /// The work here is a sleep rather than a name lookup because a lookup that
    /// hangs is a property of the *machine* — it is what a CI runner with its
    /// egress blackholed has and a developer's laptop does not — and the bound
    /// has to be provable on both. What is under test is [`within`]: the caller
    /// stops waiting even though the work has not stopped working.
    #[test]
    fn a_step_that_never_finishes_ends_at_the_deadline() {
        let start = Instant::now();
        let answer = within(Duration::from_millis(50), "the test's own step", || {
            std::thread::sleep(Duration::from_secs(60));
            0_u8
        });
        assert!(matches!(answer, Err(NetFail::Timeout)), "a step that never answers is a Timeout");
        assert!(start.elapsed() < SOON, "the wait took {:?}", start.elapsed());
    }

    /// An address literal is dialled, not looked up.
    ///
    /// A deadline of nothing at all is the assertion: any lookup would time out
    /// against it, so an answer proves no lookup happened.
    #[test]
    fn an_address_literal_is_not_looked_up() {
        assert_eq!(
            resolve("127.0.0.1", 443, Duration::ZERO).ok(),
            Some(SocketAddr::from(([127, 0, 0, 1], 443)))
        );
        assert_eq!(
            resolve("::1", 8443, Duration::ZERO).ok(),
            Some(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 8443)))
        );
    }

    /// A peer that keeps talking and never stops.
    ///
    /// `every` is what decides which bound it runs into: a sleep makes it a
    /// drip and the budget catches it, no sleep makes it a firehose and the
    /// byte cap does. One reader, because they are one hole — a loop with no
    /// bound on how many times it goes round.
    struct Endless {
        every: Duration,
    }

    impl Read for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.every.is_zero() {
                std::thread::sleep(self.every);
                return Ok(buffer.first_mut().map_or(0, |b| {
                    *b = b'.';
                    1
                }));
            }
            buffer.fill(b'.');
            Ok(buffer.len())
        }
    }

    /// **A server that drips the response ends at the budget.**
    ///
    /// The socket's own `SO_RCVTIMEO` cannot catch this and that is the point:
    /// every read here *succeeds*, so the deadline it carries is restarted
    /// before it can fire and the peer is never late. What ends the read is the
    /// budget across the loop, and without one this case does not fail — it
    /// runs until the byte cap, which at a byte per millisecond is over two
    /// hours.
    #[test]
    fn a_response_that_drips_ends_at_the_budget() {
        let mut peer = Endless { every: Duration::from_millis(1) };
        let mut out = Vec::new();
        let started = Instant::now();
        let answer = read_to_end(
            &mut peer,
            &mut out,
            Instant::now() + Duration::from_millis(100),
            RESPONSE_LIMIT,
        );
        assert!(matches!(answer, Err(NetFail::Timeout)), "a drip is a Timeout");
        assert!(started.elapsed() < SOON, "the read ran {:?}", started.elapsed());
    }

    /// **A response larger than this client will read is refused, and says so.**
    ///
    /// The other half of the same loop: a peer that answers as fast as the
    /// socket allows is bounded by neither a socket deadline nor a budget, only
    /// by how much memory the process has. `net.rs` has had `BODY_LIMIT` on the
    /// serving side since F2; this is that cap, in the direction that did not
    /// have one.
    #[test]
    fn a_response_larger_than_the_cap_is_refused_and_says_so() {
        let mut peer = Endless { every: Duration::ZERO };
        let mut out = Vec::new();
        let started = Instant::now();
        let answer =
            read_to_end(&mut peer, &mut out, Instant::now() + Duration::from_secs(60), 64 * 1024);
        match answer {
            Err(NetFail::Transport(said)) => {
                assert!(said.contains("larger than"), "{said}");
                assert!(said.contains("65536"), "the refusal names the cap it hit: {said}");
            }
            Err(other) => panic!("an endless response was refused as {}", other.message()),
            Ok(()) => panic!("an endless response was read to its end"),
        }
        assert!(started.elapsed() < SOON, "the read ran {:?}", started.elapsed());
    }

    /// A dial to an address nothing answers for ends at the deadline.
    ///
    /// `192.0.2.1` is TEST-NET-1 (RFC 5737): an address reserved for
    /// documentation, which no host on the internet is allowed to be and which
    /// nothing on the way there is allowed to answer for. What it does with a
    /// SYN depends on the network the test is run on — dropped on the floor
    /// here, "no route to host" there — and this case asserts the half that has
    /// to be true on all of them: `fetch` comes back, and it comes back when
    /// this client's deadline says so rather than when the kernel's own connect
    /// timeout does, which on this platform is over a minute.
    #[test]
    fn a_dial_that_goes_nowhere_ends_at_the_deadline() {
        let start = Instant::now();
        let answer =
            fetch_within(Duration::from_millis(250), 0, "http://192.0.2.1:81/probe", &[], b"");
        assert!(answer.is_err(), "TEST-NET-1 answered an HTTP request");
        assert!(start.elapsed() < SOON, "the dial took {:?}", start.elapsed());
    }
}
