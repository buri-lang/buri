//! **The end-to-end tier**: whole programs, a real backend, a real process, a
//! real socket, and a real signal.
//!
//! `cli/tests/README.md`'s "The trust ordering" is the argument for this file
//! existing; the short version is that every other tier here asserts about a
//! *part* of a program — a `Program` a test built by hand, a function called
//! from a suite, a listener in the runtime's own table — and each of them can
//! stay green while the thing a user runs stops working. Two slices' only
//! end-to-end goldens once collapsed into `1 failed to compile` and every unit
//! layer under them stayed green for two waves.
//!
//! ## What is here and what is next door
//!
//! `stencil.rs` and `llvm.rs` each carry the server rows that are about a
//! *backend* — where `.Ok`'s payload sits in a `Result`, where a `Str` sits in
//! a struct — and those are deliberately written twice, once per pipeline. The
//! rows here are about **behaviour**, which no backend decides, so each is
//! written once and built by whichever native backend this toolchain has:
//! stencil on a default build, LLVM under `--features backend-llvm`. Across
//! the two legs of `cli/tests/README.md`'s bar, both pipelines run every row.
//!
//! ## The rules every row here keeps
//!
//! * **Loopback and a port the program chose.** `port: 0`, the port printed on
//!   standard output, and the client dials `127.0.0.1`. A test that picks a
//!   port and hopes is a race between the pick and the bind.
//! * **Every wait is bounded and every thread is joined.** `shared`'s
//!   `SERVER_DEADLINE` on every connect, read and write; `shared::waited` on
//!   the child, which kills what it could not stop. A broken server is a
//!   failing test with a sentence, never a job CI has to kill. This is the
//!   `tls-hang-fix` doctrine, and it is not optional in this file.
//! * **A happy path and its signature failure.** Every row here that says a
//!   thing works has a sibling that says what happens when it does not — a
//!   plaintext client against a TLS port, a buffer that filled, a signal that
//!   came twice.
//!
//! ## What no row here can do, and why it is not a hole
//!
//! **Speak TLS or RFC 6455 with a library.** The only `rustls` and the only
//! `tungstenite` in this repository are inside the runtime archive, and a
//! dev-dependency on either from `cli/Cargo.toml` fails
//! `language::corpus::dependencies_stay_behind_the_bar`. So the clients here
//! are hand-written and deliberately minimal: [`client_hello`] is the smallest
//! TLS 1.2 `ClientHello` a `rustls` server will answer, and `shared::Talking`
//! is one text frame at a time with its mask. What they buy is the half only a
//! whole-program row can make — that a *Buri program's* `Server` opened the
//! port that answered.
//!
//! **Ask a toolchain built without networking what it says.**
//! `backend::networking_gap` reads a file baked into this binary, so the
//! `networking-not-available` refusal cannot be provoked by a program; it is
//! covered by [`the_refusal_a_toolchain_without_networking_names_what_a_real_server_reached`],
//! which drives the *real front end over real source* into the *real* refusal
//! and pins the page a user reads. That is as far as one toolchain reaches,
//! and the module doc of `backend::networking_gap_when` is the seam it uses.

use std::path::PathBuf;

#[cfg(feature = "backend-llvm")]
use buri::compiler::backend::Profile;

// ---------------------------------------------------------------------------
// Whichever native backend this toolchain has
// ---------------------------------------------------------------------------

/// Whether this host can build and run a native program at all.
///
/// The reason is printed rather than swallowed, and on a runner it panics —
/// `harness/ci.rs` reads `BURI_CI`, every job sets it, and every one of them
/// asserts the backend's own inputs are real bytes before the suite starts. A
/// guard firing there is a broken runner, not a modest host.
#[cfg(feature = "backend-llvm")]
fn ready() -> bool {
    match crate::llvm::can_execute() {
        Some(why) => !crate::ci::skipped("llvm", why),
        None => true,
    }
}

/// The same question of the copy-and-patch backend, which is what a default
/// build has.
#[cfg(all(not(feature = "backend-llvm"), feature = "backend-stencil"))]
fn ready() -> bool {
    crate::stencil::supported()
}

/// One program, through the whole pipeline, into an executable.
#[cfg(feature = "backend-llvm")]
fn built(name: &str, source: &str) -> PathBuf {
    crate::llvm::build_at(name, source, None, Profile::Release)
}

/// The same, on the backend a default build carries.
#[cfg(all(not(feature = "backend-llvm"), feature = "backend-stencil"))]
fn built(name: &str, source: &str) -> PathBuf {
    crate::stencil::build_with(name, source, None)
}

macro_rules! unless_ready {
    () => {
        if !ready() {
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// The programs
// ---------------------------------------------------------------------------

/// The eight kilobytes that make the port line readable *while the server is
/// still running*.
///
/// `cli/runtime/host.rs` buffers standard output until `FLUSH_AT` or exit, so a
/// fixture that has to be heard from *while it is still running* fills the
/// buffer. `shared`'s own fixtures carry the same padding and the same
/// argument.
///
/// Standard output is no longer the only channel out of a native program —
/// `host.HostFs.*` and `host.HostEnv.*` have rows in both runtime tables since
/// buri-lang/buri#36, and [`a_native_binary_touches_files_and_reads_its_own_arguments`]
/// is what says so — but it is still the only *unbuffered-on-demand* one, and a
/// server that has to announce a port before it blocks has nowhere else to put
/// the line.
fn padding() -> String {
    format!(r#"    let pad = "x".repeat(ctx, {});"#, crate::shared::STDOUT_BUFFER)
}

/// A server that asks for HTTP/3 on a toolchain whose runtime has no QUIC.
///
/// **The refusal is a run-time `.Err` and not a compile-time one, on purpose.**
/// `net-h3` is a field of a value rather than an intrinsic key, so nothing
/// about the program is wrong; refusing every program that mentions `serve`
/// would refuse every server that was only ever going to speak HTTP/1.1
/// (`backend/runtime_native.rs`'s `h3` paragraph). This row is the other half
/// of that decision: the program compiles, links, runs, and is told no in a
/// sentence naming the switch that would have said yes.
fn quic_server() -> String {
    String::from(
        r#"from "core/effect" import { Alloc, Listen, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Listen: host.listen,
        Stdout: host.stdout,
    };
    let plan = server.Server {
        port: 0,
        onRequest: fn(_c, _request) => http.status(204),
        protocols: .Some([.Http3]),
        idleTimeoutMillis: .Some(200),
    };
    match (server.bind(ctx, plan)) {
        .Err(e) => {
            let _refused = io.println(ctx, "h3 ${e.detail}").ignore();
            .Ok(())
        },
        .Ok(_listener) => .Err("a toolchain with no QUIC opened an HTTP/3 port"),
    }
}
"#,
    )
}

/// A TLS server that keeps its port open until it is told to stop.
///
/// No `requestLimit` and no `idleTimeoutMillis`, for `shared::draining_server`'s
/// reason: the probes below never complete a request — they cannot, there is no
/// TLS client here to complete one with — so a limit would never be spent and a
/// deadline would decide how long every row waited. A signal ends it, which
/// makes the row assert one more thing for free: **a TLS listener drains like
/// any other**.
fn tls_running_server(certificate: &std::path::Path, key: &std::path::Path) -> String {
    format!(
        r#"from "core/effect" import {{ Alloc, Listen, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Alloc: host.alloc,
        Listen: host.listen,
        Stdout: host.stdout,
        Tasks: host.tasks,
    }};
    let plan = server.Server {{
        port: 0,
        onRequest: fn(c, request) => http.text(c, request.path()),
        protocols: .Some([.Http2, .Http1]),
        tls: .Some(server.Tls {{ certificate: "{certificate}", key: "{key}" }}),
        drainMillis: .Some(5000),
    }};
    match (server.bind(ctx, plan)) {{
        .Err(e) => .Err(server.errorText(e)),
        .Ok(listener) => {{
{padding}
            let _announced = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
            match (server.run(ctx, listener, plan)) {{
                .Err(e) => .Err(server.errorText(e)),
                .Ok(_ok) => {{
                    let _done = io.println(ctx, "served").ignore();
                    .Ok(())
                }},
            }}
        }},
    }}
}}
"#,
        certificate = certificate.display(),
        key = key.display(),
        padding = padding(),
    )
}

/// A server with **no** `websocket` field, which is the whole of the
/// fall-through claim.
///
/// `Server.websocket` is an `Option` of the hooks rather than a `Bool` beside
/// them precisely so that this program has no branch in it: an upgrade request
/// reaches `onRequest` like any other request, and what a server that does not
/// do WebSockets answers is its own business.
fn no_hooks_server() -> String {
    format!(
        r#"from "core/effect" import {{ Alloc, Listen, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/str" import * as str;

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Alloc: host.alloc,
        Listen: host.listen,
        Stdout: host.stdout,
        Tasks: host.tasks,
    }};
    let plan = server.Server {{
        port: 0,
        onRequest: fn(c, request) => http.text(c, str.format(c, "no sockets here: ${{request.path()}}")),
        requestLimit: .Some(1),
        idleTimeoutMillis: .Some(20000),
    }};
    match (server.bind(ctx, plan)) {{
        .Err(e) => .Err(server.errorText(e)),
        .Ok(listener) => {{
{padding}
            let _announced = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
            match (server.run(ctx, listener, plan)) {{
                .Err(e) => .Err(server.errorText(e)),
                .Ok(_ok) => {{
                    let _done = io.println(ctx, "served").ignore();
                    .Ok(())
                }},
            }}
        }},
    }}
}}
"#,
        padding = padding(),
    )
}

/// A socket whose outbound buffer is one message deep, and a handler that
/// pushes far more than one.
///
/// **`send` never waits** — it takes a lock, pushes and returns — so a client
/// that reads more slowly than this server writes has to end somewhere, and
/// `Server.socketBuffer` is where. The flood is a tail call rather than a
/// literal list because what is being provoked is a *queue depth*: with a
/// bound of one, the second push that finds the first still queued is the
/// overflow, and a thousand of them in a row is a certainty rather than a race.
///
/// Three claims ride on one program, and each of them is a sentence on
/// standard output:
///
/// * `closed Overflow` — `onClose` ran, with the reason no peer can forge.
///   The far side is told 1011 and the hook is told `.Overflow`; the two are
///   different numbers on purpose.
/// * `after` — the hook's `send` *after* the close was accepted and dropped,
///   rather than aborting. "Did this arrive" was never a question this side
///   could answer.
/// * `served` — `run` still ended in `.Ok(())`, so an overflowing socket is a
///   socket ending and not a server falling over.
fn overflowing_socket_server(flood: usize) -> String {
    format!(
        r#"from "core/effect" import {{ Alloc, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/net/server" import {{ Socket }};

fn flood<C: Sockets>(ctx: C, socket: Socket, left: Int): Int {{
    if (left <= 0) {{
        0
    }} else {{
        let _pushed = socket.send(ctx, .Text("x"));
        flood(ctx, socket, left - 1)
    }}
}}

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Alloc: host.alloc,
        Listen: host.listen,
        Sockets: host.sockets,
        Stdout: host.stdout,
        Tasks: host.tasks,
    }};
    let plan = server.Server {{
        port: 0,
        onRequest: fn(_c, _request) => http.status(404),
        requestLimit: .Some(1),
        idleTimeoutMillis: .Some(20000),
        socketBuffer: .Some(1),
        websocket: .Some(server.WebSocket {{
            onOpen: fn(_c, _socket, _request) => 0,
            onMessage: fn(c, socket, sent, _message) => sent + flood(c, socket, {flood}),
            onClose: fn(c, socket, _sent, reason) => {{
                let _said = io.println(c, "closed ${{reason.show(c)}}").ignore();
                let _dropped = socket.send(c, .Text("after the close"));
                let _after = io.println(c, "after").ignore();
                ()
            }},
        }}),
    }};
    match (server.bind(ctx, plan)) {{
        .Err(e) => .Err(server.errorText(e)),
        .Ok(listener) => {{
{padding}
            let _announced = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
            match (server.run(ctx, listener, plan)) {{
                .Err(e) => .Err(server.errorText(e)),
                .Ok(_ok) => {{
                    let _done = io.println(ctx, "served").ignore();
                    .Ok(())
                }},
            }}
        }},
    }}
}}
"#,
        flood = flood,
        padding = padding(),
    )
}

/// **One handler, two worlds.**
///
/// `reply` is bounded by `Sockets` and nothing else, which is the whole point
/// of `effect Sockets` being separate from `Listen`: a function that pushes on
/// a socket never needs to have seen a listener. So it can be run twice in one
/// program — once against `Paper`, a `Sockets` this program writes, which
/// prints what a socket was handed instead of handing it to one; and once as a
/// real `onMessage` on a real acceptor, with a real client reading the frame.
///
/// **The two answers have to be the same string**, and that is what the row
/// asserts. It is the toolchain-side twin of
/// `conformance/lib/semantics/test/host_testing.buri`'s `sockets()` blocks: the
/// corpus says what the double does with no network at all, and this says the
/// same function does the same thing when there is one.
///
/// `Paper` prints through a context it holds rather than recording, for the
/// reason `conformance/lib/semantics/shapes.buri`'s `Scripted` does: an effect
/// method takes only `self`, `self` is immutable, and a hand-written double
/// that cannot reach runner-side state can record nothing at all.
fn both_worlds_server() -> String {
    format!(
        r#"from "core/effect" import {{ Alloc, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/net/server" import {{ Message, Socket }};
from "core/str" import * as str;

/// The answer, wherever it is asked for.
fn answer<C: Alloc>(ctx: C, question: Str): Str {{
    str.format(ctx, "you said ${{question}}")
}}

/// The hook, written once. `Alloc` for the answer, `Sockets` for the push, and
/// nothing about a listener anywhere in the bound.
fn reply<C: Alloc + Sockets>(ctx: C, socket: Socket, message: Message): Int {{
    match (message) {{
        .Text(text) => {{
            let _pushed = socket.send(ctx, .Text(answer(ctx, text)));
            1
        }},
        .Binary(_data) => {{
            let _pushed = socket.send(ctx, .Text(answer(ctx, "bytes")));
            1
        }},
    }}
}}

/// A `Sockets` with no network behind it: it says what it was handed.
struct Paper<C>(C);

impl<C: Alloc + Stdout> Sockets for Paper<C> {{
    fn socketSendText(self, _socket: Int, text: Str): () {{
        let _said = io.println(self.0, "paper ${{text}}").ignore();
        ()
    }}

    fn socketSendBytes(self, _socket: Int, _body: [U8]): () {{
        ()
    }}

    fn socketClose(self, _socket: Int, _code: Int, _reason: Str): () {{
        ()
    }}
}}

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Alloc: host.alloc,
        Listen: host.listen,
        Sockets: host.sockets,
        Stdout: host.stdout,
        Tasks: host.tasks,
    }};
    let plan = server.Server {{
        port: 0,
        onRequest: fn(_c, _request) => http.status(404),
        requestLimit: .Some(1),
        idleTimeoutMillis: .Some(20000),
        websocket: .Some(server.WebSocket {{
            onOpen: fn(_c, _socket, _request) => 0,
            // The second world: the same function, the same argument shapes,
            // and a client on the far side of a real socket.
            onMessage: fn(c, socket, said, message) => said + reply(c, socket, message),
            onClose: fn(_c, _socket, _said, _reason) => (),
        }}),
    }};
    match (server.bind(ctx, plan)) {{
        .Err(e) => .Err(server.errorText(e)),
        .Ok(listener) => {{
{padding}
            let _announced = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
            // The first world: no acceptor, no port, no client. A socket handle
            // this program invented, and a `Sockets` that writes down what it
            // was pushed. It runs after the port line because the port line is
            // the one the test reads first.
            let printing = context {{
                Alloc: host.alloc,
                Stdout: host.stdout,
            }};
            let onPaper = context {{
                Alloc: host.alloc,
                Sockets: Paper(printing),
            }};
            let _papered = reply(onPaper, Socket(1), .Text("hello"));
            match (server.run(ctx, listener, plan)) {{
                .Err(e) => .Err(server.errorText(e)),
                .Ok(_ok) => {{
                    let _done = io.println(ctx, "served").ignore();
                    .Ok(())
                }},
            }}
        }},
    }}
}}
"#,
        padding = padding(),
    )
}

// ---------------------------------------------------------------------------
// A TLS client that is not a TLS library
// ---------------------------------------------------------------------------

/// What a probe is waiting for.
///
/// **A byte count is not an answer, and this parameter used to be one.** It was
/// `want: usize`, the loop stopped as soon as that many bytes had arrived, and
/// every caller passed a number small enough that the first `read` satisfied it
/// — which meant each row asserted about *whatever the kernel had coalesced
/// into one segment*. On an idle machine that is the whole reply; under load
/// `hyper` writes the head and the body as two writes, they arrive as two
/// readable chunks, and the row saw a complete set of headers with no body
/// under them. CI caught it on both Linux jobs of run `33539837433`, and the
/// head it printed carried `content-length: 24` — the exact length of the body
/// the row said was missing, which is the failure diagnosing itself.
///
/// So a caller now says what a *complete* answer is, and the loop reads until
/// it has one. The two shapes are the two protocols these rows speak.
#[derive(Clone, Copy)]
enum Until {
    /// **The peer closed.** Right for an HTTP/1.1 exchange this server answers
    /// `connection: close`, and for a probe whose whole claim is about
    /// everything the far side had to say — nothing short of the end can settle
    /// "and it said nothing else either".
    Closed,
    /// **One whole TLS record**: its five-byte header, and the length that
    /// header declares. A record is the unit a `ServerHello` arrives in, so
    /// this is what "the handshake answered" means in bytes.
    ARecord,
}

impl Until {
    /// Whether what has arrived is already a complete answer.
    ///
    /// `Closed` is never satisfied by content, only by the read loop reaching
    /// end of file — which is the point of it.
    fn satisfied(self, back: &[u8]) -> bool {
        match self {
            Until::Closed => false,
            Until::ARecord => match back.get(3..5) {
                Some(two) => back.len() >= 5 + usize::from(u16::from_be_bytes([two[0], two[1]])),
                None => false,
            },
        }
    }
}

/// Dial a port and answer what came back, bounded on every step.
///
/// The connect retries until the deadline for `shared::served`'s reason: the
/// port is announced before the first accept, so the first connect can lose the
/// race with the listen backlog on a loaded machine. The read stops when
/// [`Until`] says the answer is whole, when the peer closes, or when the
/// deadline on the socket fires — never on a byte count.
fn dialled(port: u16, out: &[u8], until: Until) -> Vec<u8> {
    use std::io::{Read, Write};
    let deadline = crate::shared::SERVER_DEADLINE;
    let give_up_at = std::time::Instant::now() + deadline;
    let mut socket = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(socket) => break socket,
            Err(e) => {
                assert!(
                    std::time::Instant::now() < give_up_at,
                    "could not reach the server on 127.0.0.1:{port}: {e}"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    };
    socket.set_read_timeout(Some(deadline)).unwrap();
    socket.set_write_timeout(Some(deadline)).unwrap();
    socket.write_all(out).expect("the probe went out");
    socket.flush().expect("flush");
    let mut back = Vec::new();
    let mut chunk = [0u8; 4096];
    while !until.satisfied(&back) {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => back.extend_from_slice(&chunk[..read]),
        }
    }
    back
}

/// The body of an HTTP/1.1 reply, or `None` where the head has no blank line
/// after it.
///
/// Split out so that a row asserting about a body says so, and a row that read
/// only a head fails with *that* sentence rather than with "the text I was
/// looking for is not in this text".
fn body_of(reply: &str) -> Option<&str> {
    reply.split_once("\r\n\r\n").map(|(_head, body)| body)
}

/// The smallest TLS 1.2 `ClientHello` a `rustls` server will answer, offering
/// the two protocols this repository's servers are asked for.
///
/// **TLS 1.2 and not 1.3, and that is the whole reason this is writable.** In
/// TLS 1.3 the negotiated protocol travels inside `EncryptedExtensions`, so
/// reading it needs the key schedule and therefore a crypto library — which
/// `dependencies_stay_behind_the_bar` will not admit. In TLS 1.2 the server's
/// ALPN extension is in the clear in the `ServerHello`, so a hundred bytes of
/// hand-written record is enough to ask "which protocol did you pick" and read
/// the answer. `cli/runtime/tls.rs` builds its `ServerConfig` with
/// `with_safe_default_protocol_versions`, which is 1.2 and 1.3, so this is a
/// handshake the acceptor genuinely offers rather than one it was talked into.
///
/// Nothing here is a secret: the "random" is a constant, no key share is sent,
/// and the exchange stops at the `ServerHello`. What is being asked is which
/// protocol the server chose, and it says so before any of that matters.
fn client_hello(protocols: &[&str]) -> Vec<u8> {
    fn extension(id: u16, body: Vec<u8>) -> Vec<u8> {
        let mut out = id.to_be_bytes().to_vec();
        out.extend_from_slice(&u16::try_from(body.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&body);
        out
    }
    fn sized(body: Vec<u8>) -> Vec<u8> {
        let mut out = u16::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        out
    }

    let mut extensions = Vec::new();
    // server_name: `localhost`, which is the only name the fixture leaf carries.
    let mut name = vec![0x00u8];
    name.extend_from_slice(&sized(b"localhost".to_vec()));
    extensions.extend_from_slice(&extension(0x0000, sized(name)));
    // supported_groups: x25519 and secp256r1.
    extensions.extend_from_slice(&extension(0x000a, sized(vec![0x00, 0x1d, 0x00, 0x17])));
    // ec_point_formats: uncompressed.
    extensions.extend_from_slice(&extension(0x000b, vec![0x01, 0x00]));
    // signature_algorithms: the leaf is ECDSA over P-256, plus two RSA rows so
    // that a future fixture certificate does not silently stop being answered.
    extensions.extend_from_slice(&extension(
        0x000d,
        sized(vec![0x04, 0x03, 0x08, 0x04, 0x04, 0x01]),
    ));
    // extended_master_secret and session_ticket, both empty, both what an
    // ordinary client sends.
    extensions.extend_from_slice(&extension(0x0017, Vec::new()));
    extensions.extend_from_slice(&extension(0x0023, Vec::new()));
    // ALPN, which is what the row is here to ask about.
    let mut offered = Vec::new();
    for protocol in protocols {
        offered.push(u8::try_from(protocol.len()).unwrap());
        offered.extend_from_slice(protocol.as_bytes());
    }
    extensions.extend_from_slice(&extension(0x0010, sized(offered)));

    let mut body = vec![0x03u8, 0x03];
    body.extend_from_slice(&[0x5au8; 32]);
    body.push(0x00);
    body.extend_from_slice(&sized(vec![0xc0, 0x2b, 0xc0, 0x2c, 0xc0, 0x2f, 0xc0, 0x30]));
    body.extend_from_slice(&[0x01, 0x00]);
    body.extend_from_slice(&sized(extensions));

    let mut handshake = vec![0x01u8];
    let length = u32::try_from(body.len()).unwrap().to_be_bytes();
    handshake.extend_from_slice(&length[1..]);
    handshake.extend_from_slice(&body);

    let mut record = vec![0x16u8, 0x03, 0x01];
    record.extend_from_slice(&u16::try_from(handshake.len()).unwrap().to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

/// The protocol a `ServerHello` chose, read out of its ALPN extension.
///
/// `None` where the bytes are not a `ServerHello` at all — an alert, an HTTP
/// response, or nothing — which is what makes the assertion at the call site
/// name what actually came back rather than panicking inside a parser.
fn chose(back: &[u8]) -> Option<String> {
    // record header, handshake header, then the ServerHello body.
    let handshake = back.get(5..)?;
    if *handshake.first()? != 0x02 {
        return None;
    }
    let mut at = 4 + 2 + 32;
    let session = usize::from(*handshake.get(at)?);
    at = at + 1 + session + 2 + 1;
    let extensions = handshake.get(at + 2..)?;
    let mut cursor = 0usize;
    while cursor + 4 <= extensions.len() {
        let id = u16::from_be_bytes([extensions[cursor], extensions[cursor + 1]]);
        let length =
            usize::from(u16::from_be_bytes([extensions[cursor + 2], extensions[cursor + 3]]));
        let body = extensions.get(cursor + 4..cursor + 4 + length)?;
        if id == 0x0010 {
            // list length, then one length-prefixed name.
            let name = body.get(3..)?;
            return Some(String::from_utf8_lossy(name).to_string());
        }
        cursor += 4 + length;
    }
    None
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// **A protocol this toolchain's runtime was not built for is refused when the
/// port opens, in a sentence naming the switch.**
///
/// The happy path's negative twin, and the only one of the three protocols
/// whose refusal an ordinary toolchain can be asked for: `net-h3` is off unless
/// somebody asked for it, `net` is on unless something took it away. What the
/// row proves that the runtime's own test cannot is that the refusal survives
/// the whole toolchain — a `[Protocol]` rendered into a `[Serve]`, crossing the
/// C ABI at this backend's layout, and a `Str` coming back inside
/// `ServeError.detail`.
#[test]
fn a_protocol_this_runtime_was_not_built_for_is_refused_when_the_port_opens() {
    unless_ready!();
    let binary = built("e2e-http3", &quic_server());
    let out = crate::shared::ran(&binary);
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    let said = out
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("h3 "))
        .unwrap_or_else(|| panic!("the program printed no h3 line:\n{}", out.stdout));
    assert!(
        said.contains("HTTP/3"),
        "the refusal does not say which protocol was refused: {said}"
    );
    assert!(
        said.contains("BURI_RUNTIME_NET_H3"),
        "the refusal does not name the switch that would have said yes, so a reader is told \
         no and not told what to do about it: {said}"
    );
}

/// **A TLS port speaks TLS, chooses `h2` over ALPN, and has nothing to say to a
/// plaintext client** — one server, both halves, in one process.
///
/// The two probes are deliberately in one row and in this order. A plaintext
/// `GET` proving nothing came back would also pass against a server that had
/// fallen over, so the refusal is only evidence when the *same listener*, a
/// moment later, answers a real `ClientHello` — and the `ClientHello` is only
/// evidence of a Buri program's own port because a plaintext client to that
/// port got nothing.
///
/// It ends with a signal rather than a deadline, which asserts the third thing:
/// a **TLS** listener drains the way a cleartext one does, answers `.Ok(())`,
/// and lets `main` fall off its own end.
#[test]
fn a_tls_port_chooses_a_protocol_and_answers_a_plaintext_client_with_no_http_at_all() {
    unless_ready!();
    let (certificate, key, _absent) = crate::shared::tls_identity("e2e");
    let binary = built("e2e-tls", &tls_running_server(&certificate, &key));
    let running = crate::shared::announced(&binary);
    let port = running.2;

    let plaintext =
        dialled(port, b"GET /cleartext HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n", Until::Closed);
    assert!(
        !plaintext.starts_with(b"HTTP/"),
        "a plaintext client got an HTTP answer out of a TLS port: {}",
        String::from_utf8_lossy(&plaintext)
    );

    let hello = client_hello(&["h2", "http/1.1"]);
    let back = dialled(port, &hello, Until::ARecord);
    assert_eq!(
        back.first().copied(),
        Some(0x16),
        "the port answered a ClientHello with something that is not a TLS handshake record: {back:?}"
    );
    assert_eq!(
        chose(&back).as_deref(),
        Some("h2"),
        "the ServerHello did not choose h2 out of `h2, http/1.1`, which is what \
         `protocols: .Some([.Http2, .Http1])` asked the acceptor to offer"
    );

    crate::shared::signalling(&running.0, crate::shared::SIGTERM);
    let out = crate::shared::finished(running);
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    assert!(
        out.stdout.ends_with("served\n"),
        "a secured listener did not drain to `.Ok(())` the way a cleartext one does:\n{}",
        out.stdout
    );
}

/// **A second signal ends a server the first one asked to drain.**
///
/// F5's row is the drain; this is the escape hatch beside it, and it is what
/// stands between a handler that will not return and a process nobody can stop.
/// `cli/runtime/net.rs`'s handler restores the default disposition *first*,
/// before it writes into its self-pipe, so the second signal is the operating
/// system's rather than this runtime's.
///
/// The fixture's handler sleeps for far longer than the row is willing to wait,
/// so the drain is genuinely still in progress when the second signal lands —
/// a shorter sleep would be a race between the second signal and an ordinary
/// exit, and a passing run would say nothing.
///
/// Every wait is bounded and the child is killed on the way out if the second
/// signal did not do it, so a runtime that had made itself unkillable is a
/// failing test with a sentence rather than a process left behind.
#[test]
fn a_second_signal_ends_a_server_the_first_one_asked_to_drain() {
    unless_ready!();
    let binary = built("e2e-hard-stop", &crate::shared::draining_server(120_000));
    let stopped = crate::shared::signalled_twice(&binary, crate::shared::SIGTERM);
    assert_eq!(
        stopped.killed_by,
        Some(crate::shared::SIGTERM),
        "the second signal did not end the process: it exited with {:?} after saying:\n{}",
        stopped.code,
        stopped.said
    );
    assert!(
        stopped.said.contains("handling"),
        "the request never reached a handler, so the drain the second signal cut short had \
         not started:\n{}",
        stopped.said
    );
    assert!(
        !stopped.said.contains("served"),
        "the drain finished, so this row measured an ordinary shutdown and not a second \
         signal:\n{}",
        stopped.said
    );
}

/// **An upgrade request is an ordinary request on a server with no hooks.**
///
/// `Server.websocket` is an `Option` of the hooks rather than a `Bool` beside
/// them so that there is no third state to be in. This is the half of that
/// decision a program can be asked about: the same five header fields that
/// bring a socket into being on the server next door reach `onRequest` here,
/// and the handler that never heard of WebSockets answers them.
#[test]
fn an_upgrade_request_reaches_the_request_handler_when_a_server_has_no_hooks() {
    unless_ready!();
    let binary = built("e2e-fall-through", &no_hooks_server());
    let running = crate::shared::announced(&binary);
    let port = running.2;
    let back =
        dialled(port, crate::shared::upgrade_request("/socket").as_bytes(), Until::Closed);
    let reply = String::from_utf8_lossy(&back).to_string();
    assert!(
        reply.starts_with("HTTP/1.1 200 "),
        "an upgrade request to a server with no hooks was not answered as a request: {reply}"
    );
    assert!(
        !reply.contains("101 "),
        "a server with no `websocket` field switched protocols: {reply}"
    );
    // **The body, and not `contains` over the whole reply.** A `content-length`
    // in the head is the server's claim about a body; this is the body. The two
    // being separate assertions is what makes a reply whose head arrived and
    // whose body did not fail as "the body is empty" rather than as a missing
    // substring — which is the shape this row was first written in, and the
    // shape that made a read bug read like a platform divergence.
    let body = body_of(&reply)
        .unwrap_or_else(|| panic!("the reply has no blank line after its head: {reply}"));
    assert_eq!(
        body, "no sockets here: /socket",
        "the handler was not given the request's own path.\nthe whole reply was:\n{reply}"
    );
    let out = crate::shared::finished(running);
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
}

/// **A full outbound buffer closes the socket, and every part of that is
/// observable from outside the process.**
///
/// Four claims, and each of them is a different kind of evidence:
///
/// * the client is sent **1011**, which is what the far side is told;
/// * `onClose` is told **`.Overflow`**, which is what this end knows and no
///   peer can forge — a wire close code is two unsigned octets, so the negative
///   number the platform uses cannot have come off a socket;
/// * the hook's own `send`, made *after* the socket is gone, is **dropped**
///   rather than aborting; and
/// * `run` still answered `.Ok(())`.
///
/// The client says one thing and then reads to the end, so the frames it is
/// sent go into its receive buffer and the outbound queue is what fills. With
/// a bound of one, the flood is a certainty rather than a race.
#[test]
fn a_full_outbound_buffer_closes_the_socket_and_the_close_hook_is_told_why() {
    unless_ready!();
    let binary = built("e2e-overflow", &overflowing_socket_server(4_000));
    let running = crate::shared::announced(&binary);
    let mut client = crate::shared::Talking::to(running.2);
    client.say("go");
    let code = client.closed_with();
    let out = crate::shared::finished(running);
    assert_eq!(
        code,
        Some(1011),
        "the far side was not told 1011, which is the true sentence for an overflow from \
         where a client is standing.\nthe server said:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("closed .Overflow"),
        "`onClose` did not run with `.Overflow`:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("\nafter\n") || out.stdout.ends_with("after\n"),
        "the hook's send after the close did not return, so a message to a socket that has \
         gone is not being dropped:\n{}",
        out.stdout
    );
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    assert!(
        out.stdout.ends_with("served\n"),
        "an overflowing socket took the server with it:\n{}",
        out.stdout
    );
}

/// **The same handler, with a socket behind it and with nothing behind it,
/// answers the same string.**
///
/// A test double earns its keep only if a program that passes against it passes
/// against the thing it stands for. The corpus asserts what `sockets()` does
/// with no network at all
/// (`conformance/lib/semantics/test/host_testing.buri`); this asserts that the
/// function under both of them does not care which one it got.
#[test]
fn the_same_handler_answers_the_same_way_with_and_without_a_socket() {
    unless_ready!();
    let binary = built("e2e-both-worlds", &both_worlds_server());
    let running = crate::shared::announced(&binary);
    let mut client = crate::shared::Talking::to(running.2);
    client.say("hello");
    let heard = client.heard();
    client.hush();
    let out = crate::shared::finished(running);
    assert_eq!(
        heard.as_deref(),
        Some("you said hello"),
        "the real socket did not carry the handler's answer.\nthe server said:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("paper you said hello"),
        "the same handler answered something else when the `Sockets` behind it was one the \
         program wrote:\n{}",
        out.stdout
    );
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
}

// ---------------------------------------------------------------------------
// C3's refusal, over real source
// ---------------------------------------------------------------------------

/// The front end and `middle`, over one source, with no backend involved.
///
/// A copy of what `stencil.rs` and `llvm.rs` each do before they emit, minus
/// the emission — because the question below is asked of the `Program`, and a
/// `Program` is the same one whichever backend is about to read it.
fn compiled(source: &str) -> buri::compiler::middle::monomorphize::Program {
    use buri::compiler::modules::Role;
    use buri::compiler::{driver, middle};
    use buri::diagnostics::{Diagnostics, SourceMap};

    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::Entry);
    assert!(
        !analysis.diagnostics.has_errors(),
        "the source did not compile: {:?}",
        analysis.diagnostics.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let entry = analysis.checked.entry.expect("the source exports `main`");
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = middle::monomorphize::run(
        &analysis.checked,
        paths,
        &mut diagnostics,
        middle::monomorphize::Roots::Main(entry),
    );
    assert!(!diagnostics.has_errors(), "monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    program
}

/// **C3's missing test**: a real `Listen`-using program, compiled by the real
/// front end, and the refusal a toolchain without networking would print for
/// it — a diagnostic, and not a link error naming a symbol.
///
/// The close-out audit's F-4 recorded this as a hole: the design row named
/// *"a test compiling a `Listen`-using program with the feature off and
/// asserting the diagnostic"*, `grep -rl networking-not-available cli/tests/`
/// returned nothing, and the only coverage was over `Program`s a unit test had
/// built by hand out of a list of key strings. A hand-built `Program` cannot
/// answer the question the row was written for, which is **whether an ordinary
/// server program reaches the family at all** — a rename in `core/net/server`,
/// an intrinsic key that stopped being one, or a `serve` that grew a path
/// around `host.HostListen` would leave the unit rows green and this one red.
///
/// **Why it stops here and does not run a second toolchain.**
/// `runtime_native::net()` reads a file `cli/build.rs` writes beside the
/// archive and `include_str!` bakes into this binary, so the only way to *run*
/// the refusal is to build `buri` a second time with `BURI_RUNTIME_NET=0` —
/// minutes of `cargo`, inside a test, for an answer this row already gets from
/// the seam `networking_gap_when` exists to be. That is the honest boundary of
/// what one toolchain can assert, and `cli/tests/README.md`'s trust ordering
/// names it as the one refusal in the concurrency program with no whole-process
/// row of its own.
///
/// Three claims, and each is a different way the loop could break:
///
/// * the program a user writes reaches exactly the eight keys recorded below —
///   `serve`'s seven entries and the fan-out that drives them — so the family
///   is neither an empty set on real source nor a guess;
/// * everything the gap names is in the family and nothing else is — a
///   `list.map` in the same program is not swept up;
/// * the refusal names every one of them, carries the page's code and fix, and
///   does **not** say "report it" — a missing capability is a toolchain to
///   replace, not a bug to file.
#[test]
fn the_refusal_a_toolchain_without_networking_names_what_a_real_server_reached() {
    use buri::compiler::backend::{networking_gap, networking_gap_when, no_networking};
    use buri::diagnostics::Span;

    let program = compiled(&crate::shared::one_shot_server());

    // On this toolchain — which has networking — there is no gap at all, and
    // an ordinary build pays a walk of the function list and no diagnostic.
    assert!(
        networking_gap(&program).is_empty(),
        "this toolchain has networking, so a real server program owes it no refusal"
    );

    let gap = networking_gap_when(&program, false);
    // Recorded rather than sampled, and this is the list a hand-built `Program`
    // could not have produced: `serve`'s seven entries and the fan-out that
    // drives them. It is written out so that an entry `core/net/server` grows
    // or loses shows up here as a diff — a refusal that stopped naming half of
    // what a program reaches would leave the reader with half a link error.
    assert_eq!(
        gap,
        vec![
            "host.HostListen.listenAccept".to_string(),
            "host.HostListen.listenBind".to_string(),
            "host.HostListen.listenClose".to_string(),
            "host.HostListen.listenReceive".to_string(),
            "host.HostListen.listenRequest".to_string(),
            "host.HostListen.listenRespond".to_string(),
            "host.HostListen.listenUpgrade".to_string(),
            "host.HostTasks.parallel".to_string(),
        ],
        "an ordinary `bind`-and-`run` program does not reach the keys the refusal names"
    );
    assert!(
        gap.iter().all(|key| buri::compiler::backend::runtime_native::net_intrinsic(key)),
        "the gap claimed a key outside the networking family: {gap:?}"
    );
    assert!(
        !gap.iter().any(|key| key == "list.map" || key == "str.concat"),
        "the gap swept up an ordinary intrinsic the same program uses: {gap:?}"
    );

    let refusal = no_networking(&gap, Span::NONE);
    assert_eq!(refusal.code.as_deref(), Some("networking-not-available"));
    for key in &gap {
        assert!(
            refusal.message.contains(&format!("`{key}`")),
            "the refusal does not name `{key}`, which the program reaches: {}",
            refusal.message
        );
    }
    assert!(
        refusal.message.contains("without networking"),
        "the refusal does not say what is wrong with the toolchain: {}",
        refusal.message
    );
    let fix = refusal.fix.clone().expect("every diagnostic carries a fix");
    assert!(fix.contains("net"), "the fix does not name the feature: {fix}");
    assert!(
        !fix.contains("report it"),
        "a toolchain built without a capability is not a bug report: {fix}"
    );
}

/// The read loop's own tests, against a peer that answers in two writes.
///
/// **These exist because the read loop was the bug.** Every row above dials a
/// real Buri server, and against one on an idle machine a head and a body
/// arrive coalesced — so a loop that stopped at the first `read` passed on
/// every developer's machine and failed on both Linux jobs of CI run
/// `33539837433`, where the suite's own load made the two writes two segments.
/// A row that can only be made to fail by a loaded runner is a row nobody can
/// fix, so the failure mode is pinned here instead: a listener of this file's
/// own that answers in two writes with a pause between them, which is what a
/// loaded runner does and what an idle one does not.
///
/// It costs a millisecond and needs no backend, so it runs on every host,
/// including one with no `cc` and no stencil library — the rows above cannot
/// say that of themselves.
#[cfg(test)]
mod read_loop_tests {
    use super::{body_of, dialled, Until};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A listener on a port the operating system chose, answering `pieces` with
    /// a pause between each, then closing. Answers the port.
    ///
    /// The thread is detached rather than joined, and that is the one place
    /// this file departs from its own doctrine on purpose: it holds one
    /// connection, writes a few dozen bytes and returns, so there is nothing to
    /// wait for and nothing that can outlive the test. Every wait on the
    /// *client* side is still bounded by `dialled`'s own deadline.
    fn answering(pieces: &'static [&'static [u8]]) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        std::thread::spawn(move || {
            let Ok((mut socket, _from)) = listener.accept() else { return };
            // **Read what the client sent before answering anything**, which a
            // server does anyway and which one of these rows needs: a peer with
            // nothing to say would otherwise close while the client was still
            // writing, and the write would take `EPIPE`. That is a flake of
            // exactly the kind this module exists to stop, so it is closed here
            // rather than tolerated at the call site.
            let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut asked = [0u8; 1024];
            let _ = socket.read(&mut asked);
            for piece in pieces {
                if socket.write_all(piece).is_err() {
                    return;
                }
                let _ = socket.flush();
                // Long enough that the two writes cannot be coalesced into one
                // segment, which is the whole point: it makes the split that a
                // loaded runner produces by accident happen every time.
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        port
    }

    /// A head in one write and a body in the next is one reply.
    ///
    /// This is the exact failure CI saw, at the exact assertion that saw it:
    /// the head declares `content-length: 24` and carries no body, and the old
    /// loop stopped there and reported the body missing.
    #[test]
    fn a_head_and_a_body_in_two_writes_are_one_reply() {
        let port = answering(&[
            b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\n\
              content-length: 24\r\nconnection: close\r\n\r\n",
            b"no sockets here: /socket",
        ]);
        let back = dialled(port, b"GET /socket HTTP/1.1\r\n\r\n", Until::Closed);
        let reply = String::from_utf8_lossy(&back).to_string();
        assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
        assert_eq!(body_of(&reply), Some("no sockets here: /socket"), "{reply}");
    }

    /// And a TLS record split across its header and its payload is one record.
    ///
    /// `Until::ARecord` reads the length out of the header it has and then
    /// waits for that many bytes, so a peer that writes the two separately is
    /// not a short read. The old loop asked for five bytes and would have
    /// stopped on the header alone.
    #[test]
    fn a_record_split_from_its_header_is_one_record() {
        // A handshake record whose body is seven bytes: the header says so.
        let port = answering(&[b"\x16\x03\x03\x00\x07", b"\x02\x00\x00\x03abc"]);
        let back = dialled(port, b"hello", Until::ARecord);
        assert_eq!(back.len(), 12, "the record was not read whole: {back:?}");
        assert_eq!(back.first().copied(), Some(0x16));
    }

    /// A peer that closes with nothing to say is an answer too, and a bounded
    /// one — the loop ends at end of file rather than at its deadline.
    #[test]
    fn a_peer_that_says_nothing_and_closes_is_not_a_wait() {
        let port = answering(&[]);
        let started = std::time::Instant::now();
        let back = dialled(port, b"anything", Until::Closed);
        assert!(back.is_empty(), "{back:?}");
        assert!(
            started.elapsed() < crate::shared::SERVER_DEADLINE,
            "the loop waited for its deadline rather than for the close"
        );
    }
}

// ---------------------------------------------------------------------------
// The host surface: files, directories, arguments and variables
// ---------------------------------------------------------------------------

/// A program that uses every part of `Fs` a scratch directory needs, plus both
/// operations of `Env`.
///
/// **Every path is relative**, so the run below decides where the program
/// works by choosing its working directory rather than by baking one into the
/// source — which is what lets the fixture be a constant and the row be
/// hermetic.
///
/// It ends by removing what it made, which is the half `makeDir` had no
/// inverse for until buri-lang/buri#38: the last two lines are the assertion
/// that a program can leave the filesystem as it found it.
fn host_surface() -> String {
    String::from(
        r#"from "core/effect" import { Alloc, Env, Fs, Stdout };
from "core/env" import * as env;
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Env: host.env,
        Fs: host.fs,
        Stdout: host.stdout,
    };
    let _made = fs.makeDir(ctx, "scratch/run").mapErr(fn(_e) => "makeDir")?;
    let _wrote = fs.writeText(ctx, "scratch/run/note.txt", "hello").mapErr(fn(_e) => "write")?;
    let body = fs.readText(ctx, "scratch/run/note.txt").mapErr(fn(_e) => "read")?;
    let _p1 = io.println(ctx, "read ${body}").mapErr(fn(_e) => "print")?;
    let names = fs.listDir(ctx, "scratch/run").mapErr(fn(_e) => "listDir")?;
    let _p2 = io.println(ctx, "dir ${names.join(ctx, ",")}").mapErr(fn(_e) => "print")?;
    let args = env.args(ctx);
    let _p3 = io.println(ctx, "args ${args.join(ctx, ",")}").mapErr(fn(_e) => "print")?;
    let seen = match (env.get(ctx, "BURI_E2E_VARIABLE")) {
        .Some(value) => value,
        .None => "absent",
    };
    let _p4 = io.println(ctx, "var ${seen}").mapErr(fn(_e) => "print")?;
    let held = match (fs.removeDir(ctx, "scratch/run")) {
        .Ok(_gone) => "removed a directory that still held a file",
        .Err(_error) => "not empty",
    };
    let _p5 = io.println(ctx, "held ${held}").mapErr(fn(_e) => "print")?;
    let _gone = fs.remove(ctx, "scratch/run/note.txt").mapErr(fn(_e) => "remove")?;
    let _inner = fs.removeDir(ctx, "scratch/run").mapErr(fn(_e) => "removeDir run")?;
    let _outer = fs.removeDir(ctx, "scratch").mapErr(fn(_e) => "removeDir scratch")?;
    let left = fs.exists(ctx, "scratch");
    io.println(ctx, "left ${left}").mapErr(fn(_e) => "print")
}
"#,
    )
}

/// **A native binary reads and writes files, makes and removes directories,
/// and sees its own arguments and environment.**
///
/// buri-lang/buri#36 and buri-lang/buri#38 in one process. Before them, this
/// program did not compile at all for a native output: `buri build` refused it
/// with *"the stencil backend has no implementation of host.HostEnv.args,
/// host.HostFs.makeDir, …"* — nine operations in one line — while the same
/// source ran on JavaScript. `cli/runtime/host.rs` had a body for every one of
/// them; what was missing was the row, and behind the row the one shape
/// `Result<T, IoError>` needed (`cli/runtime/lib.rs` §2.1's message).
///
/// It is here rather than in `stencil.rs` or `llvm.rs` for this file's own
/// reason: what it asserts is *behaviour*, which no backend decides, so it is
/// written once and run by whichever native backend the toolchain has. And it
/// is a whole process rather than a unit row because nothing smaller can say
/// that a file was really written — a table with the right rows in it and an
/// archive that never opened the file would pass every other tier here.
#[test]
fn a_native_binary_touches_files_and_reads_its_own_arguments() {
    unless_ready!();
    let binary = built("e2e-host-surface", &host_surface());
    let dir = binary.parent().expect("the program is in a workspace of its own");
    let out = std::process::Command::new(&binary)
        .current_dir(dir)
        .args(["alpha", "beta"])
        .env("BURI_E2E_VARIABLE", "seen")
        .output()
        .expect("the program did not start");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the program failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            // `writeText` then `readText`: the octets went to a real file and
            // came back.
            "read hello",
            // `readDir` sees what `writeText` created, and nothing else.
            "dir note.txt",
            // `Env`'s two, in the order the effect declares them.
            "args alpha,beta",
            "var seen",
            // `removeDir` is `rmdir(2)` and not `rm -r`: a directory that still
            // holds a file is refused, which is the decision `core/fs`'s
            // `removeDir` argues and the reason there is no recursive form.
            "held not empty",
            // And once it is empty it goes, along with its parent — so the
            // program left the filesystem as it found it.
            "left false",
        ],
        "stderr:\n{stderr}"
    );
    assert!(
        !dir.join("scratch").exists(),
        "the program reported removing its scratch directory and it is still there"
    );
}
