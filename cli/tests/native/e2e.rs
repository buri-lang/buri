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
/// `host.HostFileSystem.*` and `host.HostEnvironment.*` have rows in both runtime tables since
/// buri-lang/buri#36, and [`a_native_binary_touches_files_and_reads_its_own_arguments`]
/// is what says so — but it is still the only *unbuffered-on-demand* one, and a
/// server that has to announce a port before it blocks has nowhere else to put
/// the line.
fn padding() -> String {
    format!(r#"    let pad = "x".repeat(ctx, {});"#, crate::shared::STDOUT_BUFFER)
}

/// A program that dials a socket, writes to it, reads until the answer is
/// whole, and closes it.
///
/// The port arrives on the command line rather than in the source, for the same
/// reason every server row here prints one: a port written into a fixture is a
/// race between the write and the bind.
///
/// The read is a **loop**, and that is the half of `core/net/tcp`'s contract
/// this fixture exists to exercise. `Stream.read` answers at most what it was
/// asked for and waits for at least one byte, so a client that stopped at the
/// first answer would be asserting about whatever the kernel coalesced into one
/// segment — which is `cli/tests/README.md`'s read-loop rule seen from the
/// program's side. The peer below answers in two writes so that the loop
/// happens every time rather than on a loaded machine only.
fn tcp_client() -> String {
    String::from(
        r#"from "core/bytes" import * as bytes;
from "core/effect" import { Allocator, Environment, Stdout, Tcp };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/tcp" import * as tcp;
from "core/net/tcp" import { Stream };

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        Stdout: host.stdout,
        Tcp: host.tcp,
    };
    let port = env.withArguments(ctx).first().andThen(fn(a) => a.toInt()).withDefault(0);
    match (tcp.connect(ctx, "127.0.0.1", port)) {
        .Err(_e) => .Err("the dial failed"),
        .Ok(stream) => exchange(ctx, stream),
    }
}

fn exchange<C: Allocator + Stdout + Tcp>(ctx: C, stream: Stream): Result<(), Str> {
    match (stream.write(ctx, bytes.toUtf8(ctx, "ping\n"))) {
        .Err(_e) => .Err("the write failed"),
        .Ok(_sent) => {
            match (whole(ctx, stream, [])) {
                .Err(why) => .Err(why),
                .Ok(answer) => said(ctx, stream, answer),
            }
        },
    }
}

/// Reads until a newline has arrived, however many segments it took.
fn whole<C: Allocator + Tcp>(ctx: C, stream: Stream, sofar: [U8]): Result<[U8], Str> {
    if (sofar.contains(10)) {
        .Ok(sofar)
    } else {
        match (stream.read(ctx, 64)) {
            .Err(_e) => .Err("the read failed"),
            .Ok(more) => {
                if (more.isEmpty()) {
                    .Err("the peer closed before it answered")
                } else {
                    whole(ctx, stream, sofar.concat(ctx, more))
                }
            },
        }
    }
}

fn said<C: Allocator + Stdout + Tcp>(ctx: C, stream: Stream, answer: [U8]): Result<(), Str> {
    let text = bytes.fromUtf8(ctx, answer).withDefault("<not utf-8>");
    let _shown = io.println(ctx, "got ${text.trim()}").ignore();
    let _closed = stream.close(ctx);
    // And a stream that has been closed is one that names nothing, which is the
    // signature failure beside the exchange above.
    match (stream.read(ctx, 8)) {
        .Ok(_more) => .Err("a closed stream still read"),
        .Err(e) => {
            let _refused = io.println(ctx, "after close ${e.show(ctx)}").ignore();
            .Ok(())
        },
    }
}
"#,
    )
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
        r#"from "core/effect" import { Allocator, Listen, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
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
        r#"from "core/effect" import {{ Allocator, Listen, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
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

/// A server whose hooks are served at **one** path, and whose handler answers
/// every other one.
///
/// `WebSocket.path` is a plain required field, so this program cannot be
/// written without saying where the socket lives — and having said it, the
/// server is an ordinary one everywhere else. `requestLimit: 2` is what makes
/// it finish: the ordinary request is the first, the upgrade is the second, and
/// once the socket closes the next `listenAccept` is `.Closed`.
fn path_scoped_socket_server() -> String {
    format!(
        r#"from "core/effect" import {{ Allocator, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/str" import * as str;

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Listen: host.listen,
        Sockets: host.sockets,
        Stdout: host.stdout,
        Tasks: host.tasks,
    }};
    let plan = server.Server {{
        port: 0,
        onRequest: fn(c, request) => http.text(c, str.format(c, "handled ${{request.path()}}")),
        requestLimit: .Some(2),
        idleTimeoutMillis: .Some(20000),
        websocket: .Some(server.WebSocket {{
            path: "/socket",
            onOpen: fn(c, _socket, request) => {{
                let _said = io.println(c, "opened ${{request.path()}}").ignore();
                0
            }},
            onMessage: fn(c, socket, seen, message) => {{
                match (message) {{
                    .Text(text) => {{
                        let said = str.format(c, "echo ${{text}}");
                        let _sent = socket.send(c, .Text(said));
                        seen + 1
                    }},
                    .Binary(_data) => seen,
                }}
            }},
            onClose: fn(_c, _socket, _seen, _reason) => (),
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
        r#"from "core/effect" import {{ Allocator, Listen, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/str" import * as str;

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
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
        r#"from "core/effect" import {{ Allocator, Listen, Sockets, Stdout, Tasks }};
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
        Allocator: host.alloc,
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
            path: "/socket",
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
        r#"from "core/effect" import {{ Allocator, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/net/server" import {{ Message, Socket }};
from "core/str" import * as str;

/// The answer, wherever it is asked for.
fn answer<C: Allocator>(ctx: C, question: Str): Str {{
    str.format(ctx, "you said ${{question}}")
}}

/// The hook, written once. `Allocator` for the answer, `Sockets` for the push, and
/// nothing about a listener anywhere in the bound.
fn reply<C: Allocator + Sockets>(ctx: C, socket: Socket, message: Message): Int {{
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

impl<C: Allocator + Stdout> Sockets for Paper<C> {{
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
        Allocator: host.alloc,
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
            path: "/socket",
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
                Allocator: host.alloc,
                Stdout: host.stdout,
            }};
            let onPaper = context {{
                Allocator: host.alloc,
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

/// **A WebSocket is served at the path it names, and nowhere else.**
///
/// One server, one binary, two clients, and the two halves of the rule:
///
/// * an upgrade request to `/elsewhere` — every RFC 6455 header on it — is
///   answered by `onRequest` with a `200` and the handler's own body, which is
///   character for character what the same request gets from a server with no
///   `websocket` at all; and
/// * an upgrade request to `/socket`, the path the hooks named, becomes a
///   socket: `onOpen` runs and `onMessage` answers on the wire.
///
/// The two are asserted against *one* server on purpose. Either half alone is
/// satisfiable by a server that is broken in the other direction — a server
/// that upgraded nothing would pass the first, and the old server that upgraded
/// everything would pass the second — and only both together say that the path
/// is what chose.
#[test]
fn a_websocket_is_served_only_at_the_path_its_hooks_name() {
    unless_ready!();
    let binary = built("e2e-socket-path", &path_scoped_socket_server());
    let running = crate::shared::announced(&binary);
    let port = running.2;

    // The wrong path, asked for with a complete upgrade request.
    let back =
        dialled(port, crate::shared::upgrade_request("/elsewhere").as_bytes(), Until::Closed);
    let reply = String::from_utf8_lossy(&back).to_string();
    assert!(
        !reply.contains("101 "),
        "an upgrade request to a path the hooks do not name was upgraded: {reply}"
    );
    assert!(
        reply.starts_with("HTTP/1.1 200 "),
        "an upgrade request to another path was not answered as a request: {reply}"
    );
    let body = body_of(&reply)
        .unwrap_or_else(|| panic!("the reply has no blank line after its head: {reply}"));
    assert_eq!(
        body, "handled /elsewhere",
        "the handler did not answer the request the socket declined.\nthe whole reply was:\n{reply}"
    );

    // The declared path, on the same server.
    let mut client = crate::shared::Talking::to_at(port, "/socket");
    client.say("hi");
    let heard = client.heard();
    client.hush();
    let out = crate::shared::finished(running);
    assert_eq!(
        heard.as_deref(),
        Some("echo hi"),
        "the socket at the declared path did not carry the hook's answer.\nthe server said:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("opened /socket"),
        "`onOpen` did not run for the request to the declared path:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("opened /elsewhere"),
        "`onOpen` ran for a path the hooks do not name:\n{}",
        out.stdout
    );
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

/// A program that uses every part of the filesystem a scratch directory needs,
/// plus both operations of `Environment`.
///
/// **Every path is relative**, so the run below decides where the program
/// works by choosing its working directory rather than by baking one into the
/// source — which is what lets the fixture be a constant and the row be
/// hermetic. They are also `Path` values rather than strings: `core/fs` takes
/// nothing else, and `path.of` is where the text becomes one. That is what
/// makes this row a check on the *native* half of the change too — a `Path` is
/// a one-field struct, so it flattens to the three C parameters a `Str` was,
/// and if that were wrong every call below would read a different file.
///
/// It binds `FileSystemRead` and `FileSystemWrite` separately, which is what the split costs a
/// program that does both — and buys the one below it, which binds neither.
///
/// It ends by removing what it made, which is the half `makeDir` had no
/// inverse for until buri-lang/buri#38: the last two lines are the assertion
/// that a program can leave the filesystem as it found it.
fn host_surface() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Environment, Stdout };
from "core/env" import * as env;
from "core/fs" import { FileSystemRead, FileSystemWrite };
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/path" import * as filepath;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        FileSystemRead: host.fs,
        FileSystemWrite: host.fs,
        Stdout: host.stdout,
    };
    let run = filepath.of(ctx, "scratch/run");
    let note = run.join(ctx, "note.txt");
    let _made = fs.makeDir(ctx, run).mapErr(fn(_e) => "makeDir")?;
    let _wrote = fs.writeText(ctx, note, "hello").mapErr(fn(_e) => "write")?;
    let body = fs.readText(ctx, note).mapErr(fn(_e) => "read")?;
    let _p1 = io.println(ctx, "read ${body}").mapErr(fn(_e) => "print")?;
    let names = fs.listDir(ctx, run).mapErr(fn(_e) => "listDir")?;
    let _p2 = io.println(ctx, "dir ${names.join(ctx, ",")}").mapErr(fn(_e) => "print")?;
    let args = env.withArguments(ctx);
    let _p3 = io.println(ctx, "args ${args.join(ctx, ",")}").mapErr(fn(_e) => "print")?;
    let seen = match (env.get(ctx, "BURI_E2E_VARIABLE")) {
        .Some(value) => value,
        .None => "absent",
    };
    let _p4 = io.println(ctx, "var ${seen}").mapErr(fn(_e) => "print")?;
    let held = match (fs.removeDir(ctx, run)) {
        .Ok(_gone) => "removed a directory that still held a file",
        .Err(_error) => "not empty",
    };
    let _p5 = io.println(ctx, "held ${held}").mapErr(fn(_e) => "print")?;
    let _gone = fs.remove(ctx, note).mapErr(fn(_e) => "remove")?;
    let _inner = fs.removeDir(ctx, run).mapErr(fn(_e) => "removeDir run")?;
    let scratch = match (run.parent()) {
        .Some(up) => up,
        .None => filepath.of(ctx, "scratch"),
    };
    let _outer = fs.removeDir(ctx, scratch).mapErr(fn(_e) => "removeDir scratch")?;
    let left = fs.exists(ctx, scratch);
    io.println(ctx, "left ${left}").mapErr(fn(_e) => "print")
}
"#,
    )
}

/// The same filesystem, read-only: one binding, and the writing half of
/// `core/fs` is unreachable from anywhere this program can go.
///
/// It exists to run the *other* signature natively. `writeAtomic` and
/// `readBytesIfExists` are written out of the effect methods rather than being
/// methods of their own, so what a native run adds over the fake is that the
/// four calls an atomic write is really happened against a real filesystem —
/// including the `sync` of the directory, which the in-memory double has
/// nothing to flush for.
fn read_only_surface() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Stdout };
from "core/fs" import { FileSystemRead, FileSystemWrite, Path };
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/path" import * as filepath;
from "core/str" import * as str;

/// Names `FileSystemRead` and nothing else: no call this makes can write, and the
/// compiler is what says so.
fn describe<C: Allocator + FileSystemRead>(ctx: C, at: Path): Str {
    match (fs.readBytesIfExists(ctx, at)) {
        .Err(_e) => "unreadable",
        .Ok(.None) => "absent",
        .Ok(.Some(body)) => str.format(ctx, "${body.length()} octets"),
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        FileSystemRead: host.fs,
        FileSystemWrite: host.fs,
        Stdout: host.stdout,
    };
    let db = filepath.of(ctx, "atomic.db");
    let before = describe(ctx, db);
    let _p1 = io.println(ctx, "before ${before}").mapErr(fn(_e) => "print")?;
    let _wrote = fs.writeAtomic(ctx, db, [1, 2, 3]).mapErr(fn(_e) => "writeAtomic")?;
    let after = describe(ctx, db);
    let _p2 = io.println(ctx, "after ${after}").mapErr(fn(_e) => "print")?;
    let temporary = match (db.withSuffix(ctx, ".tmp")) {
        .Some(t) => fs.exists(ctx, t),
        .None => true,
    };
    let _p3 = io.println(ctx, "temporary ${temporary}").mapErr(fn(_e) => "print")?;
    let _gone = fs.remove(ctx, db).mapErr(fn(_e) => "remove")?;
    io.println(ctx, "stem ${db.stem().withDefault("?")}").mapErr(fn(_e) => "print")
}
"#,
    )
}

/// **`writeAtomic` and `readBytesIfExists` do on a real filesystem what the
/// in-memory double says they do.**
///
/// The fake answers the same four calls in `conformance/lib/semantics/test/
/// effects.buri`; what this adds is that `rename(2)` and the two `fsync`s
/// really ran, and that the temporary is not left behind on the way.
#[test]
fn a_native_binary_writes_atomically_and_reads_what_may_not_be_there() {
    unless_ready!();
    let binary = built("e2e-atomic-surface", &read_only_surface());
    let dir = binary.parent().expect("the program is in a workspace of its own");
    let out = std::process::Command::new(&binary)
        .current_dir(dir)
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
            // `.NotFound` on the read became `.None` rather than an error.
            "before absent",
            "after 3 octets",
            // The sibling temporary was renamed away rather than left.
            "temporary false",
            // A pure `Path` method, answered natively.
            "stem atomic",
        ],
        "stderr:\n{stderr}"
    );
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
/// **A Buri program dialled a socket and spoke over it.** `Tcp`, end to end.
///
/// The peer is this test: a loopback listener, one connection, and an answer
/// written in **two** pieces. Two rather than one is the point — it is the
/// read-loop rule from the program's side, and with one write the fixture would
/// pass whether or not `Stream.read` were allowed to answer short.
///
/// Nothing here can hang. The accept has a deadline and drops the listener when
/// it expires, so a program that dialled late is refused rather than left
/// waiting; the peer's own reads and writes carry `SERVER_DEADLINE`; and a
/// panic in the peer closes the socket, which the program reads as an empty
/// answer and reports. The child goes through `shared::waited`, which kills what
/// it could not stop.
///
/// It is here rather than in `stencil.rs` or `llvm.rs` for this file's reason:
/// what it asserts is behaviour, which no backend decides.
#[test]
fn a_native_binary_speaks_over_a_socket_it_dialled() {
    unless_ready!();
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    listener.set_nonblocking(true).expect("a listener that can be polled");

    let peer = std::thread::spawn(move || {
        let until = std::time::Instant::now() + crate::shared::SERVER_DEADLINE;
        let mut accepted = None;
        while std::time::Instant::now() < until {
            match listener.accept() {
                Ok((socket, _)) => {
                    accepted = Some(socket);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => panic!("the loopback listener failed: {e}"),
            }
        }
        // Dropping the listener without accepting is what a program that never
        // dialled gets: a refusal rather than a wait.
        let Some(mut socket) = accepted else {
            return String::from("<nobody dialled>");
        };
        socket.set_nonblocking(false).expect("a blocking socket");
        socket.set_read_timeout(Some(crate::shared::SERVER_DEADLINE)).expect("a read deadline");
        socket.set_write_timeout(Some(crate::shared::SERVER_DEADLINE)).expect("a write deadline");
        let mut asked = Vec::new();
        let mut byte = [0_u8; 1];
        while socket.read(&mut byte).expect("the request") == 1 {
            asked.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        // Two writes, so the program's read loop runs at least twice.
        socket.write_all(b"po").expect("the first half of the answer");
        socket.flush().expect("the first half flushed");
        std::thread::sleep(std::time::Duration::from_millis(20));
        socket.write_all(b"ng\n").expect("the second half of the answer");
        socket.flush().expect("the second half flushed");
        String::from_utf8_lossy(&asked).into_owned()
    });

    let binary = built("e2e-tcp-client", &tcp_client());
    let mut child = std::process::Command::new(&binary)
        .arg(port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the program did not start");
    let status = crate::shared::waited(&mut child, crate::shared::SERVER_DEADLINE);
    let mut stdout = String::new();
    child.stdout.take().expect("a piped stdout").read_to_string(&mut stdout).expect("stdout");
    let mut stderr = String::new();
    child.stderr.take().expect("a piped stderr").read_to_string(&mut stderr).expect("stderr");

    // The client's answer is read before the peer is joined, so a client that
    // failed reports as the client failing.
    assert_eq!(
        status.code(),
        Some(0),
        "the program failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            // Both halves of the answer, so the read loop ran.
            "got pong",
            // And the stream is gone once it is closed: a handle that names
            // nothing is one already closed, which `core/effect` promises and
            // `IoError.NotFound` is how it says so.
            "after close .NotFound",
        ],
        "stderr:\n{stderr}"
    );
    assert_eq!(peer.join().expect("the peer thread"), "ping\n", "the program sent something else");
}

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
            // `Environment`'s two, in the order the effect declares them.
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


/// A program that builds a real tree in a real temporary directory and asks
/// the filesystem about it.
///
/// **`makeTemporaryDirectory` is what makes the row hermetic**: the name comes
/// from `TMPDIR` and eight octets of the operating system's own entropy, so two
/// runs of this suite at once do not meet. It removes the tree with
/// `removeTree` at the end, which is the other half — and the assertion the
/// double cannot make, because a flat map has no directory to leave behind.
///
/// Five things here need the *harness*, and every one of them is something
/// `core/fs` cannot make: a symbolic link, a link that points at nothing, a
/// chain of two, a directory holding a link to its own parent, and a directory
/// this process may not read. `metadata` not following a link is the decision
/// `EntryKind` exists for, and a walk that followed one would not come back.
fn real_tree() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Entropy, Environment, IoError, Stdout };
from "core/env" import * as env;
from "core/fs" import { EntryKind, FileSystemRead, FileSystemWrite };
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/path" import * as path;
from "core/path" import { Path };
from "core/str" import * as str;
from "core/time" import { Instant };

fn named(kind: EntryKind): Str {
    match (kind) {
        .File => "file",
        .Directory => "directory",
        .Symlink => "symlink",
        .Other => "other",
    }
}

/// An `IoError` as one word, so a row can name the one it expects.
fn why(error: IoError): Str {
    match (error) {
        .NotFound => "NotFound",
        .PermissionDenied => "PermissionDenied",
        .ReadOnly => "ReadOnly",
        .AlreadyExists => "AlreadyExists",
        .NotADirectory => "NotADirectory",
        .CrossDevice => "CrossDevice",
        .Other(_message) => "Other",
    }
}

/// What a call that must fail said — or that it did not fail.
fn refused<T>(answer: Result<T, IoError>): Str {
    match (answer) {
        .Ok(_it) => "no refusal",
        .Err(error) => why(error),
    }
}

/// The entries as `name:kind`, sorted — `readDir` answers in the operating
/// system's own order, which is not an order a test may depend on.
fn listed<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, Str> {
    let entries = fs.listDirectoryEntries(ctx, at).mapErr(fn(_e) => "listDirectoryEntries")?;
    let shown = entries.mapCtx(ctx, fn(c, entry) => str.format(c, "${entry.0}:${named(entry.1)}"));
    .Ok(shown.sort(ctx).join(ctx, " "))
}

/// Every path under `root`, written from it and sorted, for `listed`'s reason.
fn walked<C: Allocator + FileSystemRead>(ctx: C, root: Path): Result<Str, Str> {
    let found = fs.walk(ctx, root).mapErr(fn(_e) => "walk")?;
    let shown = found.mapCtx(
        ctx,
        fn(c, p) => match (p.relativeTo(c, root)) {
            .Some(under) => under.text(),
            .None => p.text(),
        },
    );
    .Ok(shown.sort(ctx).join(ctx, " "))
}

/// The links and the two directories the harness made beside the binary:
/// nothing in `core/fs` creates a symbolic link or takes a permission away, and
/// both are what `EntryKind` and the walk's refusals are about.
fn harnessMade<C: Allocator + Environment + FileSystemRead + FileSystemWrite + Stdout>(ctx: C, here: Path): Result<(), Str> {
    // A link `metadata` must not follow, and one that points at nothing: both
    // are `.Symlink`, and neither is what it points at.
    let link = here.join(ctx, "pointer");
    let linkInfo = fs.metadata(ctx, link).mapErr(fn(_e) => "metadata link")?;
    let _p1 = io.println(ctx, "link ${named(linkInfo.kind)}").mapErr(fn(_e) => "print")?;
    let dangling = here.join(ctx, "dangling");
    let danglingInfo = fs.metadata(ctx, dangling).mapErr(fn(_e) => "metadata dangling")?;
    let _p2 = io
        .println(
            ctx,
            "dangling ${named(danglingInfo.kind)} ${fs.exists(ctx, dangling)} ${refused(fs.readText(ctx, dangling))} ${refused(fs.canonicalize(ctx, dangling))}",
        )
        .mapErr(fn(_e) => "print")?;
    let _p3 = io
        .println(ctx, "gone ${refused(fs.metadata(ctx, here.join(ctx, "no-such-entry")))}")
        .mapErr(fn(_e) => "print")?;

    // A link to a link to a file: only the filesystem can follow the chain.
    let chained = fs.canonicalize(ctx, here.join(ctx, "chain")).mapErr(fn(_e) => "chain")?;
    let _p4 = io
        .println(ctx, "chain ${chained.fileName().withDefault("?")}")
        .mapErr(fn(_e) => "print")?;

    // A directory holding a link to its own parent. A walk lists it and does
    // not follow it, which is the whole reason `metadata` does not either.
    let looped = here.join(ctx, "loopdir");
    let _p5 = io.println(ctx, "loop ${walked(ctx, looped)?}").mapErr(fn(_e) => "print")?;

    // A tree with a link out of it: the link goes and what it pointed at stays.
    let tree = here.join(ctx, "tree");
    let _gone = fs.removeTree(ctx, tree).mapErr(fn(_e) => "removeTree tree")?;
    let _p6 = io
        .println(
            ctx,
            "outside ${fs.exists(ctx, tree)} ${fs.exists(ctx, here.join(ctx, "outside.txt"))}",
        )
        .mapErr(fn(_e) => "print")?;

    // A directory this process may not read. The harness took the permission
    // away and knows whether the operating system honoured it.
    let closed = here.join(ctx, "closed");
    io
        .println(
            ctx,
            "closed ${refused(fs.walk(ctx, closed))} ${refused(fs.removeTree(ctx, closed))}",
        )
        .mapErr(fn(_e) => "print")
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Entropy: host.entropy,
        Environment: host.env,
        FileSystemRead: host.fs,
        FileSystemWrite: host.fs,
        Stdout: host.stdout,
    };

    // Where the process is, and what it is running on.
    let here = env.currentDirectory(ctx);
    let _p1 = io
        .println(ctx, "cwd ${here.fileName().withDefault("?")}")
        .mapErr(fn(_e) => "print")?;
    let _p2 = io.println(ctx, "os ${env.operatingSystem(ctx)}").mapErr(fn(_e) => "print")?;
    let seen = env.all(ctx).any(fn(pair) => pair.0 == "BURI_E2E_VARIABLE" && pair.1 == "seen");
    let _p3 = io.println(ctx, "inherited ${seen}").mapErr(fn(_e) => "print")?;
    let home = match (env.homeDirectory(ctx)) {
        .Some(_at) => "some",
        .None => "none",
    };
    let under = env.temporaryDirectory(ctx);
    let _p4 = io
        .println(ctx, "home ${home} tmproot ${under.text()}")
        .mapErr(fn(_e) => "print")?;

    let _made = harnessMade(ctx, here)?;

    // A real tree, in a real temporary directory.
    let root = fs.makeTemporaryDirectory(ctx, "buri-e2e-").mapErr(fn(_e) => "temporary")?;
    let _p5 = io
        .println(ctx, "temporary ${root.startsWith(under)}")
        .mapErr(fn(_e) => "print")?;

    let deep = root.join(ctx, "deep");
    let _made2 = fs.makeDir(ctx, deep).mapErr(fn(_e) => "makeDir")?;
    let note = root.join(ctx, "note.txt");
    let _wrote = fs.writeBytes(ctx, note, [104, 101, 108, 108, 111]).mapErr(fn(_e) => "write")?;
    let inner = deep.join(ctx, "b.bin");
    let _wrote2 = fs.writeBytes(ctx, inner, [1, 2]).mapErr(fn(_e) => "write inner")?;

    let info = fs.metadata(ctx, note).mapErr(fn(_e) => "metadata")?;
    let _p6 = io
        .println(ctx, "note ${named(info.kind)} ${info.size}")
        .mapErr(fn(_e) => "print")?;
    // A real clock is behind this, so what is assertable is that it is after a
    // date this toolchain did not exist on.
    let _p7 = io
        .println(ctx, "modified ${info.modified.hasPassed(Instant(1_500_000_000_000))}")
        .mapErr(fn(_e) => "print")?;
    let _p8 = io
        .println(ctx, "kinds ${fs.isFile(ctx, note)} ${fs.isDirectory(ctx, deep)}")
        .mapErr(fn(_e) => "print")?;

    let entries = listed(ctx, root)?;
    let _p9 = io.println(ctx, "entries ${entries}").mapErr(fn(_e) => "print")?;
    let tree = walked(ctx, root)?;
    let _p10 = io.println(ctx, "walk ${tree}").mapErr(fn(_e) => "print")?;

    // An empty directory lists nothing and is not a directory that is missing,
    // which is the one thing an in-memory map cannot tell apart. A file is not
    // a directory at all.
    let empty = root.join(ctx, "empty");
    let _made3 = fs.makeDir(ctx, empty).mapErr(fn(_e) => "makeDir empty")?;
    let _p11 = io
        .println(
            ctx,
            "empty ${fs.walk(ctx, empty).map(fn(found) => found.length()).withDefault(0 - 1)} ${refused(fs.walk(ctx, root.join(ctx, "nowhere")))} ${refused(fs.walk(ctx, note))}",
        )
        .mapErr(fn(_e) => "print")?;

    // A window into the file: its middle, the octet at its end, past the end,
    // none of it at all, and a count no read can have.
    let head = fs.readRange(ctx, note, 1, 3).mapErr(fn(_e) => "readRange")?;
    let _p12 = io.println(ctx, "range ${head == [101, 108, 108]}").mapErr(fn(_e) => "print")?;
    let past = fs.readRange(ctx, note, 99, 4).mapErr(fn(_e) => "readRange past")?;
    let last = fs.readRange(ctx, note, 4, 9).mapErr(fn(_e) => "readRange last")?;
    let none = fs.readRange(ctx, note, 1, 0).mapErr(fn(_e) => "readRange none")?;
    let _p13 = io
        .println(
            ctx,
            "window ${past.length()} ${last.length()} ${none.length()} ${refused(fs.readRange(ctx, note, 0 - 1, 2))} ${refused(fs.readRange(ctx, note, 0, 0 - 2))}",
        )
        .mapErr(fn(_e) => "print")?;

    // A copy of the whole of it, over itself, over a file that is there, into a
    // directory that is not, and of a directory, which is not a file.
    let twin = root.join(ctx, "note.copy");
    let _wrote3 = fs.writeBytes(ctx, twin, [55, 55, 55, 55, 55, 55, 55]).mapErr(fn(_e) => "write twin")?;
    let _copied = fs.copy(ctx, note, twin).mapErr(fn(_e) => "copy")?;
    let back = fs.readBytes(ctx, twin).mapErr(fn(_e) => "read copy")?;
    let _self = fs.copy(ctx, note, note).mapErr(fn(_e) => "copy onto itself")?;
    let itself = fs.readBytes(ctx, note).mapErr(fn(_e) => "read after self copy")?;
    let _p14 = io
        .println(
            ctx,
            "copy ${back.length()} ${itself.length()} ${refused(fs.copy(ctx, note, root.join(ctx, "nowhere/x")))} ${refused(fs.copy(ctx, deep, root.join(ctx, "deep.copy")))}",
        )
        .mapErr(fn(_e) => "print")?;

    // `..` and `.` are the filesystem's to resolve, and this is the call that
    // asks it.
    let wound = fs.canonicalize(ctx, root.join(ctx, "deep/../note.txt"))
        .mapErr(fn(_e) => "canonicalize")?;
    let straight = fs.canonicalize(ctx, note).mapErr(fn(_e) => "canonicalize note")?;
    let dot = fs.canonicalize(ctx, path.of(ctx, ".")).mapErr(fn(_e) => "canonicalize dot")?;
    let up = fs.canonicalize(ctx, path.of(ctx, "..")).mapErr(fn(_e) => "canonicalize up")?;
    let _p15 = io
        .println(
            ctx,
            "canonical ${wound == straight} ${dot.isAbsolute()} ${dot.startsWith(up)} ${refused(fs.canonicalize(ctx, root.join(ctx, "nope")))}",
        )
        .mapErr(fn(_e) => "print")?;

    // A second scratch directory is a second directory, and a prefix with a
    // separator in it names one further down.
    let second = fs.makeTemporaryDirectory(ctx, "buri-e2e-").mapErr(fn(_e) => "temporary twice")?;
    let nested = fs
        .makeTemporaryDirectory(ctx, "buri-e2e-nest/inner-")
        .mapErr(fn(_e) => "temporary nested")?;
    let _p16 = io
        .println(
            ctx,
            "twice ${root == second} ${fs.isDirectory(ctx, second)} ${fs.isDirectory(ctx, nested)} ${nested.startsWith(under)}",
        )
        .mapErr(fn(_e) => "print")?;
    let _gone2 = fs.removeTree(ctx, second).mapErr(fn(_e) => "removeTree second")?;
    let _gone3 = fs
        .removeTree(ctx, nested.parent().withDefault(nested))
        .mapErr(fn(_e) => "removeTree nested")?;

    // And the tree goes, which `removeDir` alone could not do.
    let _gone = fs.removeTree(ctx, root).mapErr(fn(_e) => "removeTree")?;
    io
        .println(
            ctx,
            "left ${fs.exists(ctx, root)} ${refused(fs.removeTree(ctx, root))}",
        )
        .mapErr(fn(_e) => "print")
}
"#,
    )
}

/// **A native binary asks a real filesystem what is there, and takes a real
/// tree away again.**
///
/// The conformance package for `core/fs` runs every one of these calls against
/// the in-memory double; what a whole process adds is the half a map cannot
/// have — a symbolic link that `metadata` refuses to follow, a `..` that only
/// `realpath(3)` can resolve, a modification time from a real clock, an empty
/// directory that is not a missing one, and a directory that is really gone at
/// the end.
///
/// It runs twice: once with an environment, and once with `HOME` and `TMPDIR`
/// taken out of it, which is the only way to ask what a scrubbed environment
/// answers.
#[test]
fn a_native_binary_reads_a_real_tree_and_removes_it() {
    unless_ready!();
    let binary = built("e2e-real-tree", &real_tree());
    let dir = binary.parent().expect("the program is in a workspace of its own").to_path_buf();

    // Laid out again before each run: the program removes the tree it is given,
    // which is the point of that half of the row.
    let run = |scrubbed: bool| -> (String, bool) {
        let unreadable = laid_out(&dir);
        let mut command = std::process::Command::new(&binary);
        command.current_dir(&dir).env("BURI_E2E_VARIABLE", "seen");
        if scrubbed {
            command.env_remove("HOME").env_remove("TMPDIR");
        }
        let out = command.output().expect("the program did not start");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(
            out.status.code(),
            Some(0),
            "the program failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        (stdout, unreadable)
    };

    let (stdout, unreadable) = run(false);
    let lines: Vec<&str> = stdout.lines().collect();
    let said = |prefix: &str| -> String {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` line:\n{stdout}"))
            .to_string()
    };
    let expected = dir.file_name().expect("a directory has a name").to_string_lossy().to_string();
    assert_eq!(said("cwd "), expected, "the program is not where it was started");
    assert!(
        said("os ") == "macos" || said("os ") == "linux",
        "the operating system reported itself as `{}`",
        said("os ")
    );
    assert_eq!(said("inherited "), "true", "the child did not see the variable it was given");

    // The links, the loop and the tree the harness laid out.
    assert_eq!(said("link "), "symlink", "`metadata` followed the link instead of reporting it");
    assert_eq!(
        said("dangling "),
        "symlink false NotFound NotFound",
        "a link to nothing is not a link that is not there"
    );
    assert_eq!(said("gone "), "NotFound", "a path naming nothing is not `.NotFound`");
    assert_eq!(said("chain "), "target.txt", "a chain of two links did not resolve");
    assert_eq!(
        said("loop "),
        "self",
        "a walk followed a link to the directory's own parent"
    );
    assert_eq!(
        said("outside "),
        "false true",
        "`removeTree` followed a link out of the tree it was given"
    );
    // A process that may read anything — root in a container — is told the
    // same thing the operating system told the harness.
    let expected_refusal = if unreadable {
        "PermissionDenied PermissionDenied"
    } else {
        "no refusal no refusal"
    };
    assert_eq!(
        said("closed "),
        expected_refusal,
        "the walk, the removal and the operating system disagree about a directory with \
         no permission"
    );

    // The tree the program made itself.
    assert_eq!(said("temporary "), "true", "the scratch directory is not under `TMPDIR`");
    assert_eq!(said("note "), "file 5", "the file's kind or size is wrong");
    assert_eq!(said("modified "), "true", "the file's timestamp is not a real one");
    assert_eq!(said("kinds "), "true true");
    assert_eq!(said("entries "), "deep:directory note.txt:file");
    assert_eq!(
        said("walk "),
        "deep deep/b.bin note.txt",
        "the walk is not the tree, depth first"
    );
    assert_eq!(
        said("empty "),
        "0 NotFound NotADirectory",
        "an empty directory, a missing one and a file are not three answers"
    );
    assert_eq!(said("range "), "true", "`readRange` answered the wrong octets");
    assert_eq!(
        said("window "),
        "0 1 0 Other Other",
        "a window past the end, at the end, of nothing, or of a negative count"
    );
    assert_eq!(
        said("copy "),
        "5 5 NotFound Other",
        "a copy over a file, onto itself, into nowhere, or of a directory"
    );
    assert_eq!(
        said("canonical "),
        "true true true NotFound",
        "`..` and `.` were not resolved by the filesystem"
    );
    assert_eq!(
        said("twice "),
        "false true true true",
        "two temporary directories are not two directories"
    );
    assert_eq!(
        said("left "),
        "false NotFound",
        "`removeTree` left the tree behind, or removing it twice is not `.NotFound`"
    );

    // The same program, with nothing in the environment to read.
    let (scrubbed, _again) = run(true);
    let home = scrubbed
        .lines()
        .find_map(|l| l.strip_prefix("home "))
        .unwrap_or_else(|| panic!("no `home` line:\n{scrubbed}"));
    assert_eq!(
        home, "none tmproot /tmp",
        "a scrubbed environment still answered a home directory or a `TMPDIR`"
    );
    let given = stdout
        .lines()
        .find_map(|l| l.strip_prefix("home "))
        .expect("the first run says where home is");
    assert!(
        given.starts_with("some tmproot /"),
        "an inherited environment has a home directory and a temporary one: {given}"
    );
}

/// The five things `core/fs` cannot make, laid out beside the binary.
///
/// Answers whether the operating system really refuses this process the
/// unreadable directory: a container that runs as root refuses nothing, and a
/// row that asserted a refusal there would be a row that fails on one machine
/// and passes on another.
fn laid_out(dir: &std::path::Path) -> bool {
    let make = |name: &str| dir.join(name);
    let _ = std::fs::remove_dir_all(make("loopdir"));
    let _ = std::fs::remove_dir_all(make("tree"));
    let _ = std::fs::set_permissions(make("closed"), permissions(0o755));
    let _ = std::fs::remove_dir_all(make("closed"));
    for name in ["pointer", "dangling", "chain", "chain2", "target.txt", "outside.txt"] {
        let _ = std::fs::remove_file(make(name));
    }

    std::fs::write(make("target.txt"), "a real file").expect("the harness could not write");
    std::fs::write(make("outside.txt"), "outside the tree").expect("the harness could not write");
    std::fs::create_dir_all(make("loopdir")).expect("the harness could not make a directory");
    std::fs::create_dir_all(make("tree")).expect("the harness could not make a directory");
    std::fs::create_dir_all(make("closed")).expect("the harness could not make a directory");
    std::fs::write(dir.join("closed").join("secret.txt"), "unreadable")
        .expect("the harness could not write");
    #[cfg(unix)]
    {
        let link = |from: &str, to: &std::path::Path| {
            std::os::unix::fs::symlink(from, to).expect("the harness could not make a link")
        };
        link("target.txt", &make("pointer"));
        link("no-such-file", &make("dangling"));
        link("chain2", &make("chain"));
        link("target.txt", &make("chain2"));
        // A directory that holds a link to its own parent: the loop a walk
        // would take if `metadata` followed links.
        link("..", &dir.join("loopdir").join("self"));
        // And a link out of a tree that is about to be removed.
        link("../outside.txt", &dir.join("tree").join("out"));
    }
    #[cfg(unix)]
    std::fs::set_permissions(make("closed"), permissions(0o000))
        .expect("the harness could not take a permission away");
    std::fs::read_dir(make("closed")).is_err()
}

/// A mode, as a `Permissions`. Unix only, which every row in this file is.
#[cfg(unix)]
fn permissions(mode: u32) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(mode)
}

/// A program that runs real children, reads its own standard input, and looks
/// programs up on `PATH`.
///
/// `true`, `false`, `cat`, `env`, `pwd` and `sh` are the ones every unix has and
/// the ones whose whole interface is an exit code, a stream and an environment.
/// `which` finds each of them, which is the other half of the row: a path this
/// program invented would prove nothing about `PATH`.
///
/// Its first argument picks what it does, because four claims here need four
/// processes and only one of them needs a build: the children, the two
/// whole-input readers — a stream is lines or octets and never both — and a
/// `PATH` the harness chose.
fn child_processes() -> String {
    String::from(
        r#"from "core/bytes" import * as bytes;
from "core/effect" import { Allocator, Environment, IoError, Stdin, Stdout };
from "core/env" import * as env;
from "core/fs" import { FileSystemRead };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/path" import { Path };
from "core/process" import * as process;
from "core/process" import { Command, Spawn };
from "core/str" import * as str;

fn text<C: Allocator>(ctx: C, body: [U8]): Str {
    match (bytes.fromUtf8(ctx, body)) {
        .Ok(said) => said.trim(),
        .Err(_e) => "?",
    }
}

/// An `IoError` as one word, so a row can name the one it expects.
fn why(error: IoError): Str {
    match (error) {
        .NotFound => "NotFound",
        .PermissionDenied => "PermissionDenied",
        .ReadOnly => "ReadOnly",
        .AlreadyExists => "AlreadyExists",
        .NotADirectory => "NotADirectory",
        .CrossDevice => "CrossDevice",
        .Other(_message) => "Other",
    }
}

fn refused<T>(answer: Result<T, IoError>): Str {
    match (answer) {
        .Ok(_it) => "no refusal",
        .Err(error) => why(error),
    }
}

fn found<C: Allocator + Environment + FileSystemRead>(ctx: C, program: Str): Result<Path, Str> {
    match (process.which(ctx, program)) {
        .Some(at) => .Ok(at),
        .None => .Err(str.format(ctx, "no `${program}` on PATH")),
    }
}

fn shown(at: Option<Path>): Str {
    match (at) {
        .Some(p) => p.text(),
        .None => "none",
    }
}

/// Everything on standard input, as text and as octets. Two modes rather than
/// one call: a stream is lines or octets and never both, which `Stdin` states.
fn filtered<C: Allocator + Stdin + Stdout>(ctx: C, mode: Str): Result<(), Str> {
    if (mode == "read") {
        let whole = io.readAll(ctx);
        io
            .println(ctx, "readAll ${whole.length()} ${whole.replace(ctx, "\n", "|")}")
            .mapErr(fn(_e) => "print")
    } else {
        let body = io.readAllBytes(ctx);
        let sum = body.fold(fn(total, b) => total + b.toI64(), 0);
        io
            .println(ctx, "readAllBytes ${body.length()} ${sum}")
            .mapErr(fn(_e) => "print")
    }
}

/// Where `PATH` says these are, under whatever `PATH` this run was given.
fn lookups<C: Allocator + Environment + FileSystemRead + Stdout>(ctx: C): Result<(), Str> {
    io
        .println(
            ctx,
            "which ${shown(process.which(ctx, "helper"))} ${shown(process.which(ctx, "./helper"))} ${shown(process.which(ctx, "buri-no-such-program"))}",
        )
        .mapErr(fn(_e) => "print")
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        FileSystemRead: host.fs,
        Spawn: host.spawn,
        Stdin: host.stdin,
        Stdout: host.stdout,
    };

    // The harness runs this binary four times: once for the children, twice for
    // the two whole-input readers, and once with a `PATH` of its own.
    let mode = env.withArguments(ctx).get(0).withDefault("");
    if (mode == "read" || mode == "bytes") {
        filtered(ctx, mode)
    } else if (mode == "path") {
        lookups(ctx)
    } else {
        children(ctx)
    }
}

fn children<C: Allocator + Environment + FileSystemRead + Spawn + Stdout>(ctx: C): Result<(), Str> {
    let yes = found(ctx, "true")?;
    let no = found(ctx, "false")?;
    let cat = found(ctx, "cat")?;
    let _p1 = io
        .println(ctx, "which ${yes.isAbsolute()} ${shown(process.which(ctx, yes.text()))}")
        .mapErr(fn(_e) => "print")?;

    let ran = process.run(ctx, process.command(yes.text(), [])).mapErr(fn(_e) => "true")?;
    let _p2 = io
        .println(ctx, "true ${ran.code} ${ran.stdout.length()} ${ran.stderr.length()}")
        .mapErr(fn(_e) => "print")?;

    let failed = process.run(ctx, process.command(no.text(), [])).mapErr(fn(_e) => "false")?;
    let _p3 = io.println(ctx, "false ${failed.code}").mapErr(fn(_e) => "print")?;

    // Standard input reaches the child, and what it writes reaches back.
    let fed = Command {
        program: cat.text(),
        arguments: [],
        workingDirectory: .None,
        environment: .None,
        stdin: .Some([104, 101, 108, 108, 111]),
    };
    let echoed = process.run(ctx, fed).mapErr(fn(_e) => "cat")?;
    let _p4 = io
        .println(ctx, "cat ${echoed.code} ${text(ctx, echoed.stdout)}")
        .mapErr(fn(_e) => "print")?;

    // More than a pipe holds, both ways at once. A run that read its input only
    // after the child exited would stop here rather than answer.
    let large = list.generate(ctx, 300_000, fn(_i) => 65);
    let flooded = Command {
        program: cat.text(),
        arguments: [],
        workingDirectory: .None,
        environment: .None,
        stdin: .Some(large),
    };
    let all = process.run(ctx, flooded).mapErr(fn(_e) => "cat large")?;
    let _p5 = io
        .println(ctx, "large ${all.code} ${all.stdout.length()}")
        .mapErr(fn(_e) => "print")?;

    // A `cat` of a path that is not there writes to standard error and exits
    // non-zero: a child that ran and failed is `.Ok`, not `.Err`.
    let complained = process
        .run(ctx, process.command(cat.text(), ["no-such-file-here"]))
        .mapErr(fn(_e) => "cat missing")?;
    let _p6 = io
        .println(ctx, "missing ${complained.code != 0} ${complained.stderr.length() > 0}")
        .mapErr(fn(_e) => "print")?;

    // A program that is not there never ran at all, and neither did one whose
    // working directory is not there.
    let nowhere = Command {
        program: yes.text(),
        arguments: [],
        workingDirectory: .Some(yes.parent().withDefault(yes).join(ctx, "no-such-directory")),
        environment: .None,
        stdin: .None,
    };
    // The same again with a working directory that is a *file*, which is a
    // different refusal and the one a mistyped path usually gets.
    let notAtAll = Command {
        program: yes.text(),
        arguments: [],
        workingDirectory: .Some(yes.join(ctx, "under-a-file")),
        environment: .None,
        stdin: .None,
    };
    let _p7 = io
        .println(
            ctx,
            "absent ${refused(process.run(ctx, process.command("buri-no-such-program", [])))} ${refused(process.run(ctx, process.command("./buri-no-such-program", [])))} ${refused(process.run(ctx, nowhere))} ${refused(process.run(ctx, notAtAll))}",
        )
        .mapErr(fn(_e) => "print")?;

    // The child's whole environment, replaced — with a value holding a space
    // and an `=`, which is one value and not two variables. An empty list is a
    // child with no environment at all.
    let printer = found(ctx, "env")?;
    let scrubbed = Command {
        program: printer.text(),
        arguments: [],
        workingDirectory: .None,
        environment: .Some([("BURI_CHILD", "one two=three")]),
        stdin: .None,
    };
    let listed = process.run(ctx, scrubbed).mapErr(fn(_e) => "env")?;
    let _p8 = io
        .println(ctx, "env ${text(ctx, listed.stdout)}")
        .mapErr(fn(_e) => "print")?;
    let bare = Command {
        program: printer.text(),
        arguments: [],
        workingDirectory: .None,
        environment: .Some([]),
        stdin: .None,
    };
    let nothing = process.run(ctx, bare).mapErr(fn(_e) => "env none")?;
    let _p9 = io
        .println(ctx, "envnone ${nothing.code} ${text(ctx, nothing.stdout).contains("PATH=")}")
        .mapErr(fn(_e) => "print")?;

    // Both streams at once, from one child: two pipes drained while it runs,
    // and neither one of them is the other.
    let shell = found(ctx, "sh")?;
    let both = process
        .run(ctx, process.command(shell.text(), ["-c", "echo out; echo err >&2; exit 4"]))
        .mapErr(fn(_e) => "sh both")?;
    let _p10 = io
        .println(
            ctx,
            "both ${both.code} ${text(ctx, both.stdout)} ${text(ctx, both.stderr)}",
        )
        .mapErr(fn(_e) => "print")?;

    // A child killed by a signal reports `128 + signal`, which is the number a
    // shell reports for it.
    let killed = process
        .run(ctx, process.command(shell.text(), ["-c", "kill -9 $$"]))
        .mapErr(fn(_e) => "sh")?;
    let _p11 = io.println(ctx, "signal ${killed.code}").mapErr(fn(_e) => "print")?;

    // And the directory it runs in.
    let pwd = found(ctx, "pwd")?;
    let moved = Command {
        program: pwd.text(),
        arguments: [],
        workingDirectory: .Some(yes.parent().withDefault(pwd)),
        environment: .None,
        stdin: .None,
    };
    let told = process.run(ctx, moved).mapErr(fn(_e) => "pwd")?;
    io.println(ctx, "cwd ${text(ctx, told.stdout)}").mapErr(fn(_e) => "print")
}
"#,
    )
}

/// **A native binary starts a real child, feeds it, waits for it, and reads
/// back everything it wrote.**
///
/// The conformance package for `core/process` runs every one of these through
/// the double, which records the command and answers what the test wrote down.
/// What a whole process adds is the half a double cannot have: a real `fork`
/// and `exec`, a real pipe with more octets in it than a pipe holds, a real
/// exit code, a real signal, and a real `PATH` lookup that found the program on
/// this machine.
#[test]
fn a_native_binary_runs_a_real_child_and_reads_what_it_wrote() {
    unless_ready!();
    let binary = built("e2e-child-processes", &child_processes());
    let dir = binary.parent().expect("the program is in a workspace of its own").to_path_buf();
    std::fs::write(dir.join("helper"), "a file with a name to look up")
        .expect("the harness could not write");

    // Every wait here is bounded and both streams are drained while the program
    // runs — this file's own rule, and the one a child that writes more than a
    // pipe holds is about. The writer and the two readers are threads, so the
    // only thing left to bound is the exit, which `shared::waited` does.
    let ran = |arguments: &[&str], input: &[u8], path: Option<&str>| -> String {
        use std::io::{Read, Write};
        let mut command = std::process::Command::new(&binary);
        command
            .current_dir(&dir)
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(path) = path {
            command.env("PATH", path);
        }
        let mut child = command.spawn().expect("the program did not start");
        let mut pipe = child.stdin.take().expect("the child was given a pipe");
        let body = input.to_vec();
        let feeding = std::thread::spawn(move || {
            let _ = pipe.write_all(&body);
        });
        let drain = |mut stream: std::process::ChildStdout| {
            std::thread::spawn(move || {
                let mut said = Vec::new();
                let _ = stream.read_to_end(&mut said);
                said
            })
        };
        let reading = drain(child.stdout.take().expect("the child was given a pipe"));
        let mut errors = child.stderr.take().expect("the child was given a pipe");
        let complaining = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = errors.read_to_end(&mut said);
            said
        });
        let status = crate::shared::waited(&mut child, crate::shared::SERVER_DEADLINE);
        let stdout = String::from_utf8_lossy(&reading.join().expect("the reader thread")).to_string();
        let stderr =
            String::from_utf8_lossy(&complaining.join().expect("the reader thread")).to_string();
        feeding.join().expect("the writer thread");
        assert_eq!(
            status.code(),
            Some(0),
            "the program failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        stdout
    };

    let stdout = ran(&[], b"", None);
    let lines: Vec<&str> = stdout.lines().collect();
    let said = |prefix: &str| -> String {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` line:\n{stdout}"))
            .to_string()
    };
    let yes = said("which ");
    assert!(
        yes.starts_with("true /") && yes.ends_with("true"),
        "`which` answered a relative path, or refused the absolute one it had just given: {yes}"
    );
    assert_eq!(said("true "), "0 0 0", "`true` did not exit 0 with nothing to say");
    assert_eq!(said("false "), "1", "`false` did not exit 1");
    assert_eq!(said("cat "), "0 hello", "standard input did not reach the child");
    assert_eq!(
        said("large "),
        "0 300000",
        "a child fed more than a pipe holds did not get all of it back"
    );
    assert_eq!(
        said("missing "),
        "true true",
        "a child that ran and failed did not report its own failure"
    );
    assert_eq!(
        said("absent "),
        "NotFound NotFound NotFound NotADirectory",
        "a program that is not there, a directory that is not, and one that is a file"
    );
    assert_eq!(
        said("env "),
        "BURI_CHILD=one two=three",
        "the child's environment was added to rather than replaced"
    );
    // Not "nothing at all": macOS adds a variable of its own to an empty
    // environment. The claim is that nothing the *parent* had survived.
    assert_eq!(
        said("envnone "),
        "0 false",
        "a child given an empty environment kept the parent's"
    );
    assert_eq!(
        said("both "),
        "4 out err",
        "a child that wrote to both streams was not read from both"
    );
    assert_eq!(said("signal "), "137", "a child killed by a signal is not `128 + signal`");
    // The directory `true` sits in, whatever that is on this machine — and
    // resolved, because `pwd` reports the physical path.
    let told = said("cwd ");
    assert!(
        !told.is_empty() && told.starts_with('/'),
        "the child did not run in the directory it was given: {told}"
    );

    // Standard input, read to its end: with no newline after the last line,
    // with nothing on it at all, and as octets that are not text.
    assert_eq!(
        ran(&["read"], b"one\ntwo", None).trim_end(),
        "readAll 7 one|two",
        "the last line without a newline after it did not come back"
    );
    assert_eq!(
        ran(&["read"], b"", None).trim_end(),
        "readAll 0",
        "an empty stream is not the empty string"
    );
    assert_eq!(
        ran(&["bytes"], &[0u8, 255, 10, 0, 65], None).trim_end(),
        "readAllBytes 5 330",
        "a NUL, a high octet and a newline did not survive the stream"
    );

    // `PATH` the program cannot set for itself: an empty one is one empty
    // entry, which is the directory the process is standing in.
    assert_eq!(
        ran(&["path"], b"", Some("")).trim_end(),
        // `./helper` comes back as `helper`: a `Path` is normalized once, when
        // the text becomes one.
        "which helper helper none",
        "an empty `PATH` is not the current directory"
    );
    assert_eq!(
        ran(&["path"], b"", Some("/nowhere-at-all")).trim_end(),
        "which none helper none",
        "a `PATH` naming nothing found something, or a relative name stopped resolving"
    );
}

// ---------------------------------------------------------------------------
// The two shapes that leaked a block the program could still name
// ---------------------------------------------------------------------------

/// One program, run under the runtime's own exit audit, with its output.
///
/// `BURI_RT_HEAP_REPORT` is the receipt: a clean run and a run in which the
/// check was never switched on are otherwise the same silence, so the rows
/// below assert on the line the runtime prints rather than on the absence of
/// one.
fn heap_checked(name: &str, source: &str) -> (Vec<String>, String) {
    let binary = built(name, source);
    let out = std::process::Command::new(&binary)
        .env("BURI_RT_HEAP_CHECK", "1")
        .env("BURI_RT_HEAP_REPORT", "1")
        .output()
        .expect("the program did not start");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the program failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("buri heap check: ok"),
        "the heap audit did not report a clean exit.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout.lines().map(str::to_string).collect(), stderr)
}

/// A functional update that **replaces** a field holding a counted value.
///
/// Every payload is built at run time. A literal lives in the artifact's
/// constant pool and is `IMMORTAL` (VALUE-MODEL.md §5.2), so a leaked one would
/// still balance and this row would assert nothing.
///
/// Three updates, because the leak is per replaced field rather than per
/// update, and because the third is the shape a decoder is written in — the
/// replacement *reads the field it replaces*, which is what
/// `cli/src/build/protogen.rs` generates for a repeated field and what
/// `proto/binary.buri` leaked eight blocks of.
fn replaced_fields() -> String {
    String::from(
        r#"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct Basket {
    export label: Str,
    export items: [Str],
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let base = Basket {
        label: "b".repeat(ctx, 9000),
        items: ["i".repeat(ctx, 8000)],
    };
    // The old `items` is carried by nothing the update built.
    let swapped = Basket { ..base, items: ["j".repeat(ctx, 7000)] };
    // The label as well as the list, so the row is not about one repr.
    let renamed = Basket { ..base, label: "r".repeat(ctx, 6000) };
    // And the accumulating shape: the replacement is grown out of the field it
    // replaces, which a unique list may answer by writing in place.
    let grown = Basket { ..base, items: base.items.concat(ctx, swapped.items) };
    let _ = io.println(ctx, "${base.items.length()} ${swapped.items.length()}").ignore();
    let _ = io.println(ctx, "${renamed.label.length()} ${grown.items.length()}").ignore();
    .Ok(())
}
"#,
    )
}

/// **A functional update gives back the value of every field it replaces.**
///
/// `S { ..base, f: v }` takes `..base` as a whole — a construction owns what it
/// is handed — and builds a struct carrying only the fields it did not replace.
/// The old value of a replaced field was therefore counted for by nobody: one
/// leaked block per replaced counted field, whether or not `base` outlives the
/// update. `data/lists.buri` and `proto/binary.buri` both had a ledger row for
/// it, the second because generated decoders accumulate a repeated field by
/// updating the message once per element.
#[test]
fn a_functional_update_releases_the_field_it_replaced() {
    unless_ready!();
    let (stdout, stderr) = heap_checked("e2e-struct-update-replaced", &replaced_fields());
    assert_eq!(stdout, vec!["1 1", "6000 2"], "stderr:\n{stderr}");
}

/// A `?` whose operand fails while the function still owns something.
///
/// `step` owns its parameter — the tail hands it on into the answer — and reads
/// it *after* the `?`, so on the escape path there is a live `Str` and no
/// continuation left to release it. This is `core/bytes`'s `Reader.takeVarint`
/// with the names changed, which is the row `text/bytes.buri` carried.
///
/// Both arms are exercised: `ok` takes the continuation and `bad` takes the
/// escape, so the audit is of a program in which the early return really ran.
fn early_return() -> String {
    String::from(
        r#"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct Frame {
    export body: Str,
    export at: Int,
}

fn parse(at: Int): Result<Int, Str> {
    if (at > 1) { .Err("short") } else { .Ok(at) }
}

fn step(frame: Frame): Result<(Int, Frame), Str> {
    let next = parse(frame.at)?;
    .Ok((next, frame))
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let ok = step(Frame { body: "o".repeat(ctx, 9000), at: 0 });
    let bad = step(Frame { body: "z".repeat(ctx, 6000), at: 7 });
    let _ = io.println(ctx, "${ok.isOk()} ${bad.isOk()}").ignore();
    .Ok(())
}
"#,
    )
}

/// **A `?` that leaves the function releases what the function still owns.**
///
/// The early return is a branch whose arm runs no more of the body, so every
/// drop the rest of the body would have performed is a drop that never happens
/// — and an owned value live across the `?` is leaked exactly when the `?`
/// fails. It is a *branch*, so the fix is the one a branch already gets:
/// `middle::rc` balances the escape against the continuation, and
/// `middle::lower` puts those drops in the block the early return is built in.
#[test]
fn an_early_return_releases_what_the_function_still_owns() {
    unless_ready!();
    let (stdout, stderr) = heap_checked("e2e-try-escape", &early_return());
    assert_eq!(stdout, vec!["true false"], "stderr:\n{stderr}");
}



// ---------------------------------------------------------------------------
// Everything an actor's messages and answers carried
// ---------------------------------------------------------------------------

/// A program that sends messages carrying **program-built** strings to an actor
/// that answers with built strings of its own, and then stops it.
///
/// Three things are load-bearing.
///
/// * **The payloads are built rather than written.** A literal lives in the
///   artifact's constant pool and is `IMMORTAL` (VALUE-MODEL.md 5.2), so a
///   payload nobody released would still balance. `repeat` allocates.
/// * **Every crossing is here.** The state `start` moves in, the message
///   `sendMessage` posts, the state the step puts back, and the answer that
///   comes out through a reply slot — four blocks per send, all of them the
///   runtime's between two calls.
/// * **The state the stop hands back is a built string too**, so the closing
///   path is in the count as well as the sending one.
fn actor_payloads() -> String {
    String::from(
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Stepped, Stopped };
from "core/effect" import { Allocator, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;

enum Note {
    Put(Str),
    Get,
}

enum Noted {
    Stored,
    Held(Str),
}

fn keeper<C: Allocator + Tasks>(initial: Str): Actor<C, Str, Note, Noted> {
    Actor {
        state: initial,
        step: fn(c, held, message) => {
            match (message) {
                .Put(next) => Stepped { state: next, answer: .Held(held) },
                .Get => Stepped { state: held, answer: .Held(held) },
            }
        },
    }
}

fn size(answered: Result<Noted, Stopped>): Int {
    match (answered) {
        .Ok(.Held(s)) => s.length(),
        .Ok(.Stored) => -1,
        .Err(_gone) => -1,
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let address = actor.start(ctx, keeper("s".repeat(ctx, 7000)));
    // The answer carries the state back out through a reply slot.
    let _ = io.println(ctx, "delivered ${size(address.sendMessage(ctx, .Get))}").ignore();
    // Four more, each one a block in and the one it replaced back out.
    let _ = address.sendMessage(ctx, .Put("a".repeat(ctx, 70000))).ignore();
    let _ = address.sendMessage(ctx, .Put("b".repeat(ctx, 60000))).ignore();
    let _ = address.sendMessage(ctx, .Put("c".repeat(ctx, 50000))).ignore();
    let _ = address.sendMessage(ctx, .Put("e".repeat(ctx, 40000))).ignore();
    let _ = io.println(ctx, "held ${size(address.sendMessage(ctx, .Get))}").ignore();
    let _ = io.println(ctx, "stopped ${address.stop(ctx).isOk()}").ignore();
    .Ok(())
}
"#,
    )
}

/// **An actor gives back everything its messages, its answers and its state
/// were carrying.**
///
/// Four blocks cross per send and the runtime holds each of them past the call
/// that handed it over, so a release the compiler generated at the wrong type
/// frees the block and lets go of nothing inside it. That is a real defect this
/// row caught: `stop` popped the mailbox at a type nothing determined — a
/// discard loop never looks inside what it drops — and every undelivered
/// message's payload leaked. A single-driver program can no longer leave a
/// message waiting for `stop` to discard, because `sendMessage` runs the
/// mailbox down before it answers, so what this row now counts is the four
/// crossings a send makes and the state the stop hands back.
///
/// It is here rather than beside a backend because what it asserts is
/// behaviour: both native pipelines run this row, and the runtime's own audit
/// is what answers it. `BURI_RT_HEAP_REPORT` is the audit saying so out loud —
/// a silent pass and a heap check that never ran look the same otherwise.
#[test]
fn an_actor_leaks_none_of_what_its_messages_and_answers_carried() {
    unless_ready!();
    let binary = built("e2e-actor-payloads", &actor_payloads());
    let out = std::process::Command::new(&binary)
        .env("BURI_RT_HEAP_CHECK", "1")
        .env("BURI_RT_HEAP_REPORT", "1")
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
        vec!["delivered 7000", "held 40000", "stopped true"],
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("buri heap check: ok"),
        "the heap audit did not report a clean exit.\nstderr:\n{stderr}"
    );
}



// ---------------------------------------------------------------------------
// A projection off a value that arrives through a tail
// ---------------------------------------------------------------------------

/// A **chain** of field reads off a generic call's result, where the field in
/// the middle of the chain holds a list of built strings.
///
/// `middle::inline` replaces `identity(outer(ctx, 3))` with the callee's body,
/// and a body is a `Block` — so `.inner` is a projection off a value that
/// arrives through a tail. `middle::rc` owns such a base rather than borrowing
/// it, which means the projection increfs the field it hands on: what `.inner`
/// produces is an owned reference with **no name**, and the `.lines` read one
/// step further along is the only thing that ever looks at it.
///
/// Both payloads are built rather than written. A literal lives in the
/// artifact's constant pool and is `IMMORTAL` (VALUE-MODEL.md 5.2), so a
/// string nobody released would still balance; `repeat` and `map` allocate, and
/// three strings behind one list is a block per element plus the list.
fn chained_projection() -> String {
    String::from(
        r#"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct Inner { lines: [Str] }
struct Outer { inner: Inner, tag: Str }

fn outer<C: Allocator>(ctx: C, n: Int): Outer {
    Outer {
        inner: Inner { lines: [1, 2, 3].mapCtx(ctx, fn(c, i) => "line".repeat(c, n + i)) },
        tag: "t".repeat(ctx, n),
    }
}

fn identity<T>(value: T): T { value }

export fn main(): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let held = identity(outer(ctx, 3)).inner.lines.length();
    let joined = identity(outer(ctx, 4)).inner.lines.join(ctx, ",").length();
    let _ = io.println(ctx, "held ${held} joined ${joined}").ignore();
    .Ok(())
}
"#,
    )
}

/// The same projection, reached through the **standard library** instead of a
/// `let`: `Option.withDefault` is a `match` the inliner pastes in, so
/// `held.withDefault(fallback).octets` is a field read off a tail too.
///
/// Both arms are exercised and both allocate. `.Some` answers the payload and
/// throws the fallback away; `.None` answers the fallback — and the fallback
/// here holds a **built** list rather than `list.empty<U8>()`, so the arm that
/// costs no block in `native::agreement`'s row costs one here.
fn defaulted_projection() -> String {
    String::from(
        r#"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct Wrapper { octets: [U8] }

fn payload<C: Allocator>(ctx: C, n: Int): [U8] {
    [1, 2, 3].map(ctx, fn(i) => (i + n).wrapToU8())
}

fn size(held: Option<Wrapper>, fallback: Wrapper): Int {
    held.withDefault(fallback).octets.length()
}

export fn main(): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let present = size(
        .Some(Wrapper { octets: payload(ctx, 10) }),
        Wrapper { octets: payload(ctx, 20) },
    );
    let absent = size(.None, Wrapper { octets: payload(ctx, 30) });
    let _ = io.println(ctx, "present ${present} absent ${absent}").ignore();
    .Ok(())
}
"#,
    )
}

/// **A projection off a value that arrives through a tail gives its base
/// back.**
///
/// This is the under-decrement half of issue #33, and it is one defect behind
/// two shapes. Owning a tail-shaped base is what stopped the enclosing block
/// from releasing its own binding underneath the field read; it also made the
/// projection take a count of its own, and `rc::fresh` — the function that says
/// which values are temporaries nobody named — did not report a projection off
/// a `Block` as one. So the count went out and never came back, at one block a
/// call.
///
/// It is here rather than beside a backend because what it asserts is
/// behaviour: both native pipelines run this row, and the runtime's own audit
/// is what answers it. `BURI_RT_HEAP_REPORT` is the audit saying so out loud —
/// a silent pass and a heap check that never ran look the same otherwise.
#[test]
fn a_projection_off_an_inlined_calls_result_leaks_nothing() {
    unless_ready!();
    heap_is_clean(
        "e2e-projection-chained",
        &chained_projection(),
        &["held 3 joined 74"],
    );
}

/// The same defect through `Option.withDefault`, which is the spelling a
/// program is far likelier to write than a hand-rolled `identity`.
#[test]
fn a_projection_off_a_defaulted_option_leaks_nothing() {
    unless_ready!();
    heap_is_clean(
        "e2e-projection-defaulted",
        &defaulted_projection(),
        &["present 3 absent 3"],
    );
}

/// Builds a program, runs it under the runtime's exit audit, and requires that
/// it printed those lines, exited zero, and gave every block back.
fn heap_is_clean(name: &str, source: &str, lines: &[&str]) {
    let binary = built(name, source);
    let out = std::process::Command::new(&binary)
        .env("BURI_RT_HEAP_CHECK", "1")
        .env("BURI_RT_HEAP_REPORT", "1")
        .output()
        .expect("the program did not start");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the program failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.lines().collect::<Vec<_>>(), lines.to_vec(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("buri heap check: ok"),
        "the heap audit did not report a clean exit.\nstderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Two more ways a program lost a block it was still holding
// ---------------------------------------------------------------------------
//
// Both were found by the heap check over `cli/tests/conformance`, and neither
// is about the file whose ledger row named it: `semantics/effects.buri` and
// `semantics/host_testing.buri` are the rows, and both root causes are in
// `middle::rc`. Payloads are built at run time throughout, for the reason
// [`replaced_fields`] states: a literal is `IMMORTAL` and would balance while
// leaking.

/// A `?` inside a **`match` arm**, where what leaks is not a local.
///
/// [`early_return`] is the half of this that a *name* covers: an owned local
/// live across the `?`, released on the escape path with the rest of what the
/// abandoned continuation would have released. This is the other half. A
/// `match` on a scrutinee it built itself has no binding to release it by, so
/// the drop is keyed on the `match`'s own [`rc::Position::After`] — and an arm
/// that escapes through a `?` never reaches it. `core/fs`'s `writeAtomic` is
/// the shape: `path.withSuffix(ctx, ".tmp")` is matched, the arm writes and
/// syncs and renames through three `?`s, and the temporary path leaked once per
/// write that failed.
///
/// Two runs, because the point is the difference between them: `wrote` takes
/// the continuation and `failed` takes the escape, so the audit is of a program
/// in which the early return really ran.
fn early_return_out_of_an_arm() -> String {
    String::from(
        r#"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

fn refused(at: Int): Result<Int, Str> {
    if (at > 1) { .Err("refused") } else { .Ok(at) }
}

fn suffixed<C: Allocator>(ctx: C, seed: Str): Option<Str> {
    .Some(seed.repeat(ctx, 30000))
}

/// `temp` points into an `Option` this `match` built, and the release of that
/// `Option` sits after the arms — past the `?`.
fn through_an_arm<C: Allocator>(ctx: C, seed: Str, at: Int): Result<Int, Str> {
    match (suffixed(ctx, seed)) {
        .None => .Err("none"),
        .Some(temp) => {
            let n = refused(at)?;
            .Ok(n + temp.length())
        },
    }
}

export fn main(): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let wrote = through_an_arm(ctx, "a", 0);
    let failed = through_an_arm(ctx, "b", 7);
    let _ = io.println(ctx, "wrote ${wrote.isOk()} failed ${failed.isOk()}").ignore();
    .Ok(())
}
"#,
    )
}

/// A value a statement **discards**: `let _ = f(ctx);`.
///
/// The pattern binds nothing, so there was no name for the "bound and never
/// read" drop to hang on and the initializer's value had no owner at all. It is
/// the shape a discarded `assert.ok(…)` in a test source is written in, which
/// is how nineteen of `core/host/testing`'s answers went missing.
fn discarded_bindings() -> String {
    String::from(
        r#"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

fn made<C: Allocator>(ctx: C, seed: Str): Str {
    seed.repeat(ctx, 50000)
}

/// `assert.ok`'s shape: a `match` that consumes the value and hands out a piece
/// of it, whose caller then throws the piece away.
fn taken(r: Result<Str, Str>, fallback: Str): Str {
    match (r) {
        .Ok(v) => v,
        .Err(_e) => fallback,
    }
}

export fn main(): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    // Discarded outright.
    let _ = made(ctx, "a");
    // And discarded after a `match` handed it out of a value it consumed.
    let _ = taken(.Ok(made(ctx, "b")), "");
    let kept = made(ctx, "c");
    let _ = io.println(ctx, "kept ${kept.length()}").ignore();
    .Ok(())
}
"#,
    )
}

/// **An escaping `?` gives back what the construct around it would have.**
///
/// The drops a `?` skips are not only the last-use drops of locals: they are
/// every drop this pass placed after an *enclosing* node, and the value a
/// `match` built to scrutinise is the one with no name to be found by.
#[test]
fn an_early_return_out_of_a_match_arm_releases_what_the_match_built() {
    unless_ready!();
    heap_is_clean(
        "e2e-early-return-in-an-arm",
        &early_return_out_of_an_arm(),
        &["wrote true failed false"],
    );
}

/// **A statement that discards a value releases it.**
///
/// `semantics/host_testing.buri` was the ledger row: nineteen strings and byte
/// lists the test platform's doubles had answered with, every one of them
/// thrown away by a `let _ = …;` that bound nothing.
#[test]
fn a_discarded_binding_releases_what_it_discarded() {
    unless_ready!();
    heap_is_clean("e2e-discarded-bindings", &discarded_bindings(), &["kept 50000"]);
}

// ---------------------------------------------------------------------------
// A Buri client against a Buri server
// ---------------------------------------------------------------------------
//
// Every socket row above is a *Buri server* answering a hand-written client,
// because until now that was the only client there was. `core/net/websocket`
// is the other end, so these two rows are the first in this file where both
// sides of the wire are programs this toolchain built: one binds a port and
// upgrades, the other dials it, and neither of them is `shared::Talking`.
//
// That is worth its own pair of rows rather than a variant of the ones above.
// The handshake a client writes and the handshake a server answers are two
// different pieces of code that have never met — `Talking` was written to what
// the acceptor does, so a mistake shared by both would have been invisible in
// it — and RFC 6455 masks a client's frames and not a server's, so a client is
// not a server with the arguments swapped.

/// A server that upgrades one connection at `/socket`, echoes what it is sent,
/// and stops.
///
/// `requestLimit: .Some(1)` is what makes it end on its own: the upgrade spends
/// the limit, the socket runs to its close on that worker, and the next accept
/// answers `.Closed`. So the row waits for a process to exit rather than
/// signalling one.
fn echoing_socket_server() -> String {
    echoing_socket_server_for(1)
}

/// The same server, with a request limit a row chooses.
///
/// One upgrade spends one request, so the number is how many sockets this
/// server will serve before its accept loop is done — which is what the
/// reconnect row needs two of.
fn echoing_socket_server_for(sockets: u32) -> String {
    format!(
        r#"from "core/effect" import {{ Allocator, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/str" import * as str;

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Listen: host.listen,
        Sockets: host.sockets,
        Stdout: host.stdout,
        Tasks: host.tasks,
    }};
    let plan = server.Server {{
        port: 0,
        onRequest: fn(_c, _request) => http.status(404),
        requestLimit: .Some({sockets}),
        idleTimeoutMillis: .Some(20000),
        websocket: .Some(server.WebSocket {{
            path: "/socket",
            onOpen: fn(c, _socket, _request) => {{
                let _said = io.println(c, "server opened").ignore();
                0
            }},
            onMessage: fn(c, socket, seen, message) => {{
                match (message) {{
                    .Text(text) => {{
                        let said = str.format(c, "echo ${{text}}");
                        let _sent = socket.send(c, .Text(said));
                        seen + 1
                    }},
                    .Binary(_data) => seen,
                }}
            }},
            onClose: fn(c, _socket, seen, _reason) => {{
                let _said = io.println(c, "server closed after ${{seen}}").ignore();
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
        padding = padding(),
        sockets = sockets,
    )
}

/// The client half: dial the port on the command line, say one thing, print
/// what came back, close, and print how the socket ended.
///
/// It takes the port as an argument rather than baking one in, for the reason
/// every row in this file binds `port: 0`: a test that picks a port races the
/// pick against the bind.
///
/// `Environment` is in the context and `Listen` is not, which is the shape of the claim
/// — this program has no authority to accept anything, and it does not need
/// one to hold a socket.
fn dialling_client() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        Sockets: host.sockets,
        Stdout: host.stdout,
        WebSocketClient: host.websocketClient,
    };
    let port = env.withArguments(ctx).first().withDefault("0");
    let dialled = websocket.connect(ctx, Client {
        url: str.format(ctx, "ws://127.0.0.1:${port}/socket"),
        onOpen: fn(c, socket, response) => {
            let _said = io.println(c, "client opened ${response.status}").ignore();
            let _sent = socket.send(c, .Text("hi"));
            0
        },
        onMessage: fn(c, socket, seen, message) => {
            match (message) {
                .Text(text) => {
                    let _said = io.println(c, "client heard ${text}").ignore();
                    let _closed = socket.close(c, .Normal);
                    seen + 1
                },
                .Binary(_data) => seen,
            }
        },
        onClose: fn(c, _socket, seen, reason) => {
            let _said = io.println(c, "client closed after ${seen} ${reason}").ignore();
            ()
        },
    });
    match (dialled) {
        .Err(e) => {
            let _said = io.println(
                ctx,
                "client refused: ${e.cause} ${websocket.errorText(e)} ${e.detail}",
            ).ignore();
            .Ok(())
        },
        .Ok(reason) => {
            let _said = io.println(ctx, "client ended ${reason}").ignore();
            .Ok(())
        },
    }
}
"#,
    )
}

/// Runs a linked client with one argument, under the heap check every program
/// this domain runs answers.
fn dialled_client(binary: &std::path::Path, port: u16) -> crate::shared::Ran {
    crate::shared::ran_command(
        std::process::Command::new(binary)
            .arg(port.to_string())
            .env("BURI_RT_HEAP_CHECK", "1"),
    )
}

/// **A Buri client and a Buri server carry a message both ways over a real
/// socket.**
///
/// Two processes, two binaries this toolchain built, one loopback port and no
/// hand-written framing anywhere in the row. Both ends are asserted, and each
/// says something the other cannot:
///
/// * the **client** prints the `101` it was handed, the echo it read back, and
///   the `.Normal` it closed with — so `connect` ran all three hooks, the
///   response reached `onOpen`, and the close reason survived the round trip;
/// * the **server** prints that its own `onOpen` ran and that its `onClose`
///   saw the one message — so what the client sent was framed the way this
///   acceptor reads a client's frames, mask and all.
///
/// The server ends on its own: its request limit is one, the upgrade spends it,
/// and the accept after the socket closes answers `.Closed`.
#[test]
fn a_buri_client_and_a_buri_server_carry_a_message_both_ways() {
    unless_ready!();
    // A name per row and not a name per program: `built` writes into a
    // directory named for its first argument, and two rows here run at the same
    // time.
    let server = built("e2e-client-server-both-ways", &echoing_socket_server());
    let client = built("e2e-client-dial-both-ways", &dialling_client());
    let running = crate::shared::announced(&server);
    let port = running.2;
    let said = dialled_client(&client, port);
    let out = crate::shared::finished(running);
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    for line in ["client opened 101", "client heard echo hi", "client ended .Normal"] {
        assert!(
            said.stdout.contains(line),
            "the client never said `{line}`.\nit said:\n{}\nthe server said:\n{}",
            said.stdout,
            out.stdout
        );
    }
    assert!(
        said.stdout.contains("client closed after 1 .Normal"),
        "the client's close hook did not run with the state its message left.\nit said:\n{}",
        said.stdout
    );
    assert!(
        out.stdout.contains("server opened"),
        "the server never upgraded the client's request.\nit said:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("server closed after 1"),
        "the server did not read exactly the one message the client sent.\nit said:\n{}",
        out.stdout
    );
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
}

/// **A client that dials a port nobody holds says so, and the hooks never
/// run.**
///
/// The signature failure beside the row above, and the reason `connect` answers
/// a `Result` at all: everything after the handshake is an `.Ok` carrying a
/// close reason, so the only thing an `.Err` can mean is that there was never a
/// socket. The evidence is both halves — the refusal is printed, and no hook
/// printed anything.
///
/// The port is one the operating system just gave back. `announced` starts the
/// server, `finished` waits for it to exit, and the port it held is then a port
/// with nothing behind it — which is a stronger arrangement than picking a
/// number and hoping, because a number nobody bound may be bound by anything on
/// a shared machine.
#[test]
fn a_client_that_dials_a_port_nobody_holds_says_so() {
    unless_ready!();
    let server = built("e2e-client-server-refused", &echoing_socket_server());
    let client = built("e2e-client-dial-refused", &dialling_client());
    // A port this machine really did hand out a moment ago, and then took back.
    let running = crate::shared::announced(&server);
    let port = running.2;
    let mut child = running.0;
    crate::shared::signalling(&child, crate::shared::SIGTERM);
    let _stopped = crate::shared::waited(&mut child, crate::shared::SERVER_DEADLINE);
    let _reader = running.1.join();

    let said = dialled_client(&client, port);
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    assert!(
        said.stdout.contains("client refused:"),
        "dialling a port nobody holds did not answer `.Err`.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        !said.stdout.contains("client opened"),
        "`onOpen` ran for a socket that never opened.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        !said.stdout.contains("client closed"),
        "`onClose` ran for a socket that never opened.\nthe client said:\n{}",
        said.stdout
    );
}

/// A one-shot server that answers one connection with `answer` and stops.
///
/// **What no Buri server can be made to say.** The two refusals below are a
/// handshake this repository's own acceptor would never write — a status that
/// is not `101`, and a `101` signing somebody else's key — so the far side has
/// to be a listener this test holds. It reads the request head first, because a
/// server that answered before reading would be testing this client's patience
/// rather than its answer.
///
/// Every wait is bounded by `shared::SERVER_DEADLINE`, so a client that never
/// dials is a joined thread and a failing assertion rather than a job CI has to
/// kill.
fn one_answer(answer: &'static str) -> (u16, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a bound port");
    let port = listener.local_addr().expect("the bound port").port();
    let serving = std::thread::spawn(move || {
        let deadline = crate::shared::SERVER_DEADLINE;
        let Ok((mut socket, _from)) = listener.accept() else { return };
        let _read = socket.set_read_timeout(Some(deadline));
        let _written = socket.set_write_timeout(Some(deadline));
        let mut head: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 512];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            }
        }
        let _sent = socket.write_all(answer.as_bytes());
        let _flushed = socket.flush();
    });
    (port, serving)
}

/// **A server that does not switch protocols is a socket that never opened, and
/// the program is told which status it got.**
///
/// The second of the three ways a dial can fail, as a whole program: the port
/// answers, the connection is made, and what comes back is an ordinary `404`.
/// `connect` answers `.Err(.Transport)` carrying a sentence with the status in
/// it, and neither hook runs — a socket that never opened has no state for
/// `onClose` to be handed.
#[test]
fn a_client_told_something_other_than_101_says_which_status_it_got() {
    unless_ready!();
    let client = built("e2e-client-dial-not-101", &dialling_client());
    let (port, serving) = one_answer(
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
    );
    let said = dialled_client(&client, port);
    serving.join().expect("the one-shot server finished");
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    assert!(
        said.stdout.contains("client refused: .Transport"),
        "a 404 to a handshake was not a transport failure.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        said.stdout.contains("404"),
        "the refusal never named the status the server answered.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        !said.stdout.contains("client opened") && !said.stdout.contains("client closed"),
        "a hook ran for a socket that never opened.\nthe client said:\n{}",
        said.stdout
    );
}

/// **A `101` that signs the wrong key is refused, and the program is told which
/// check failed.**
///
/// The third way, and the one the handshake exists for. `sec-websocket-accept`
/// is SHA-1 over the key this client sent and RFC 6455's constant, so a server
/// answering `101` with a signature for somebody else's key did not read this
/// request — a cache, a proxy, or an answer meant for another client. The value
/// below is the RFC's own example, which signs `dGhlIHNhbXBsZSBub25jZQ==` and
/// never the sixteen random octets this program offered.
#[test]
fn a_client_handed_a_signature_for_another_handshake_refuses_it() {
    unless_ready!();
    let client = built("e2e-client-dial-bad-accept", &dialling_client());
    let (port, serving) = one_answer(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
         sec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
    );
    let said = dialled_client(&client, port);
    serving.join().expect("the one-shot server finished");
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    assert!(
        said.stdout.contains("client refused: .Transport"),
        "a signature for another handshake was not a transport failure.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        said.stdout.contains("sec-websocket-accept"),
        "the refusal never named the check that failed.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        !said.stdout.contains("client opened") && !said.stdout.contains("client closed"),
        "a hook ran for a socket that never opened.\nthe client said:\n{}",
        said.stdout
    );
}

/// A client that dials twice, sleeping between the two, and prints what each
/// session heard.
///
/// **The loop `core/net/websocket` documents instead of a knob.** `connect`
/// returns when the socket closes, so a second socket is a second call — with
/// `time.sleepMs` between the tries, which is the whole of what a backoff is
/// here. The session number is threaded through the recursion, so the two lines
/// out say which session heard what.
fn reconnecting_client() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Clock, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/server" import { CloseReason };
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;
from "core/time" import * as time;

fn saying<C: Allocator + Sockets + Stdout + WebSocketClient>(
    url: Str,
    session: Int,
    word: Str,
): Client<C, Int> {
    Client {
        url: url,
        onOpen: fn(c, socket, _response) => {
            let _sent = socket.send(c, .Text(word));
            0
        },
        onMessage: fn(c, socket, seen, message) => {
            match (message) {
                .Text(text) => {
                    let _said = io.println(c, "session ${session} ${text}").ignore();
                    let _closed = socket.close(c, .Normal);
                    seen + 1
                },
                .Binary(_data) => seen,
            }
        },
        onClose: fn(_c, _socket, _seen, _reason) => (),
    }
}

/// Dial, and when the socket has closed, sleep and dial again.
fn following<C: Allocator + Clock + Sockets + Stdout + WebSocketClient>(
    ctx: C,
    url: Str,
    session: Int,
    left: Int,
): Int {
    let word = if (session == 1) { "one" } else { "two" };
    match (websocket.connect(ctx, saying(url, session, word))) {
        .Err(e) => {
            let _said = io.println(ctx, "client refused: ${e.detail}").ignore();
            session - 1
        },
        .Ok(_reason) => {
            if (left <= 1) {
                session
            } else {
                let _slept = time.sleepMs(ctx, 50);
                following(ctx, url, session + 1, left - 1)
            }
        },
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Environment: host.env,
        Sockets: host.sockets,
        Stdout: host.stdout,
        WebSocketClient: host.websocketClient,
    };
    let port = env.withArguments(ctx).first().withDefault("0");
    let url = str.format(ctx, "ws://127.0.0.1:${port}/socket");
    let sessions = following(ctx, url, 1, 2);
    let _said = io.println(ctx, "reconnected ${sessions}").ignore();
    .Ok(())
}
"#,
    )
}

/// **Reconnecting is a loop around `connect`, and the second session is a
/// second socket.**
///
/// Two sessions over one port, each with its own upgrade, its own three hooks
/// and its own close, from a program that names no reconnect field because there
/// is none: it calls `connect` again.
///
/// The server's request limit is two, so it ends on its own once the second
/// socket has closed, and the row waits for a process to exit rather than
/// signalling one.
#[test]
fn a_client_reconnects_by_calling_connect_again() {
    unless_ready!();
    let server = built("e2e-client-server-reconnect", &echoing_socket_server_for(2));
    let client = built("e2e-client-dial-reconnect", &reconnecting_client());
    let running = crate::shared::announced(&server);
    let port = running.2;
    let said = dialled_client(&client, port);
    let out = crate::shared::finished(running);
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    for line in ["session 1 echo one", "session 2 echo two"] {
        assert!(
            said.stdout.contains(line),
            "the client never said `{line}`.\nit said:\n{}\nthe server said:\n{}",
            said.stdout,
            out.stdout
        );
    }
    assert!(
        said.stdout.contains("reconnected 2"),
        "the loop did not run twice.\nthe client said:\n{}",
        said.stdout
    );
    // Two upgrades on the server's side: a second `connect` is a second socket
    // rather than the first one carried on with.
    assert_eq!(
        out.stdout.matches("server opened").count(),
        2,
        "the server did not upgrade twice.\nit said:\n{}",
        out.stdout
    );
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
}

// ---------------------------------------------------------------------------
// A Buri client against a server no Buri server can be
// ---------------------------------------------------------------------------
//
// The rows above put a Buri client on one end of the wire and a Buri server on
// the other, which is the strongest thing this file can say about a socket that
// behaves. What they cannot say anything about is a socket that does something
// *this repository's own acceptor never does*: closing with a code other than
// the one its program chose, fragmenting a message, sending a ping, or dropping
// the connection with no close frame at all. Every one of those is a thing a
// client has to cope with, and none is reachable from a `core/net/server`
// program.
//
// So the far side of these rows is `harness::websocket`, a hand-written
// listener — hand-written for `shared::Talking`'s reason, which is that the
// only RFC 6455 implementation here lives inside the runtime archive and the
// workspace may not grow a second one.

/// A client that dials the port on its command line as many times as its second
/// argument says, and prints how each socket ended.
///
/// One program and one line per session, which is what makes the whole of
/// `CloseReason` one row: the server closes each session with a different code,
/// and this prints the reason the program was handed for it.
fn rounds_client() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

/// Hooks that say nothing, so what this program prints is the ending and only
/// the ending.
fn quiet<C: Allocator + Sockets + Stdout + WebSocketClient>(url: Str): Client<C, Int> {
    Client {
        url: url,
        onOpen: fn(_c, _socket, _response) => 0,
        onMessage: fn(_c, _socket, seen, _message) => seen + 1,
        onClose: fn(_c, _socket, _seen, _reason) => (),
    }
}

/// Dial, print how it ended, and dial again. A self tail call, so a hundred
/// sessions would cost one frame.
fn dialling<C: Allocator + Sockets + Stdout + WebSocketClient>(ctx: C, url: Str, left: Int): () {
    if (left <= 0) {
        ()
    } else {
        let _said = match (websocket.connect(ctx, quiet(url))) {
            .Err(e) => io.println(ctx, "refused ${e.cause}").ignore(),
            .Ok(reason) => io.println(ctx, "ended ${reason}").ignore(),
        };
        dialling(ctx, url, left - 1)
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        Sockets: host.sockets,
        Stdout: host.stdout,
        WebSocketClient: host.websocketClient,
    };
    let args = env.withArguments(ctx);
    let port = args.get(0).withDefault("0");
    let rounds = args.get(1).withDefault("0").toInt().withDefault(0);
    let _ran = dialling(ctx, str.format(ctx, "ws://127.0.0.1:${port}/socket"), rounds);
    .Ok(())
}
"#,
    )
}

/// Runs a linked client with two arguments, under the heap check every program
/// in this domain answers.
fn dialled_client_rounds(binary: &std::path::Path, port: u16, rounds: usize) -> crate::shared::Ran {
    crate::shared::ran_command(
        std::process::Command::new(binary)
            .arg(port.to_string())
            .arg(rounds.to_string())
            .env("BURI_RT_HEAP_CHECK", "1"),
    )
}

/// **Every close a far side can send reaches the program as the reason it
/// names.**
///
/// `core/net/websocket` maps a wire code to a `CloseReason` in nine arms, and
/// this is all of them over a real socket: the seven codes the enum names, one
/// it does not, a close frame carrying no code at all, and a connection dropped
/// without one. The last three are `.Abnormal` — the enum's catch-all — by
/// three different routes, so a mapping that handled only one of them would
/// show up here as one line out of ten.
///
/// **A Buri server cannot be the far side of this row.** `server.run` closes a
/// socket with what its own program asked for, and nothing in it will drop a
/// connection mid-flight or send a code this language has no name for. So the
/// listener is `harness::websocket`, one session per ending, in order.
#[test]
fn a_client_reads_every_close_a_far_side_can_send() {
    unless_ready!();
    use crate::websocket::Step;
    // Seven names, one number the enum does not name, an empty close frame, and
    // no close frame at all.
    let endings: Vec<(Step, &str)> = vec![
        (Step::Close(Some(1000)), "ended .Normal"),
        (Step::Close(Some(1001)), "ended .GoingAway"),
        (Step::Close(Some(1002)), "ended .ProtocolError"),
        (Step::Close(Some(1003)), "ended .Unsupported"),
        (Step::Close(Some(1008)), "ended .Policy"),
        (Step::Close(Some(1009)), "ended .TooLarge"),
        (Step::Close(Some(1011)), "ended .InternalError"),
        (Step::Close(Some(4000)), "ended .Abnormal"),
        (Step::Close(None), "ended .Abnormal"),
        (Step::Drop, "ended .Abnormal"),
    ];
    let client = built("e2e-client-close-codes", &rounds_client());
    let serving = crate::websocket::serving(
        endings.iter().map(|(step, _)| vec![step.clone()]).collect(),
    );
    let port = serving.port;
    let said = dialled_client_rounds(&client, port, endings.len());
    let sessions = serving.heard();
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    let lines: Vec<&str> = said.stdout.lines().filter(|l| l.starts_with("ended ")).collect();
    let wanted: Vec<&str> = endings.iter().map(|(_, line)| *line).collect();
    assert_eq!(
        lines, wanted,
        "a close code reached the program as the wrong reason.\nthe client said:\n{}\nthe \
         server heard:\n{sessions:?}",
        said.stdout
    );
}

/// The client for the sizes row: say six things of its own, then print one line
/// per thing it is told, and one for the ending.
///
/// **Both directions in one program**, because a framing that is wrong is
/// usually wrong in only one of them: a client masks every frame it writes and
/// a server masks none, and a length that stops fitting in one octet — or in
/// two — is a different header on each side of the wire.
fn sizing_client() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

fn feed<C: Allocator + Sockets + Stdout + WebSocketClient>(url: Str, large: Int): Client<C, Int> {
    Client {
        url: url,
        onOpen: fn(c, socket, _response) => {
            // Nothing, one octet, and past both length boundaries, in both
            // framings. Six messages, well inside the outbound bound.
            let _empty = socket.send(c, .Text(""));
            let _one = socket.send(c, .Text("x"));
            let _many = socket.send(c, .Text("a".repeat(c, large)));
            let _none = socket.send(c, .Binary([]));
            let _byte = socket.send(c, .Binary([7]));
            let _bytes = socket.send(c, .Binary(list.repeat(c, 7, large)));
            0
        },
        onMessage: fn(c, socket, seen, message) => {
            let _said = match (message) {
                .Text(text) => io.println(c, "text ${text.length()} ${text}").ignore(),
                .Binary(data) => io.println(c, "binary ${data.length()}").ignore(),
            };
            let next = seen + 1;
            // The seventh is the last the far side sends, and this end hangs up
            // on it — with a reason that is not the default one, so the number
            // that goes out is a number this program chose.
            let _closed = if (next == 7) { socket.close(c, .GoingAway) } else { () };
            next
        },
        onClose: fn(c, _socket, seen, reason) => {
            io.println(c, "closed after ${seen} ${reason}").ignore()
        },
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        Sockets: host.sockets,
        Stdout: host.stdout,
        WebSocketClient: host.websocketClient,
    };
    let args = env.withArguments(ctx);
    let port = args.get(0).withDefault("0");
    // Past 125 and past 65535, which are the two points where a frame's length
    // stops fitting where it was. Given on the command line so the number is
    // written down once, in the test that also reads it back.
    let large = args.get(1).withDefault("0").toInt().withDefault(0);
    let url = str.format(ctx, "ws://127.0.0.1:${port}/socket");
    match (websocket.connect(ctx, feed(url, large))) {
        .Err(e) => .Err(e.detail),
        .Ok(reason) => {
            let _said = io.println(ctx, "ended ${reason}").ignore();
            .Ok(())
        },
    }
}
"#,
    )
}

/// **A message of nothing, of one octet and of seventy kilobytes crosses whole,
/// in both framings and in both directions — and a ping and a fragmentation
/// never reach the program at all.**
///
/// Six claims about size and two about what a program is *not* told, in one
/// exchange:
///
/// * **Nothing, one, and many.** A payload's length lives in the frame header
///   in one of three widths — under 126 inline, up to 65535 in two octets,
///   larger in eight — and 70000 is past both boundaries. An empty message is
///   the other end of the same header: a frame whose length is zero is a
///   message and not the absence of one, and a client that dropped it would
///   lose a keep-alive a program really did send.
/// * **Both framings.** `.Text` and `.Binary` are two opcodes, and a `[U8]` of
///   nothing is not a `Str` of nothing.
/// * **Both directions.** The server records what it was told and this row
///   asserts that list too, because a length read correctly one way is not
///   thereby read correctly the other.
/// * **A ping is the transport's.** `Frame` has three variants rather than
///   five, and this is the row that says so over a real socket: the server
///   pings, the program is told nothing, and the socket goes on working — which
///   the message *after* the ping is the evidence for.
/// * **A fragmented message is one message.** Three frames go out — one text
///   with `FIN` clear and two continuations, cut in the middle of two
///   characters — and `onMessage` runs once with the whole of it. A reassembly
///   that decoded each frame on its own would print something else.
/// * **A close this end started carries the code this end chose.** The program
///   hangs up with `.GoingAway`, the far side reads 1001 off the wire, and
///   `onClose` is handed `.GoingAway` back. The row above is every close that
///   comes *in*; this is the one that goes out.
#[test]
fn a_client_carries_every_size_and_is_told_nothing_of_a_ping() {
    unless_ready!();
    use crate::websocket::{Heard, Step};
    const LARGE: usize = 70000;
    /// Sixteen octets and twelve characters, two of which take more than one.
    const SPANS: &[u8] = "h\u{e9}llo \u{1f30a} done".as_bytes();
    let client = built("e2e-client-sizes", &sizing_client());
    let serving = crate::websocket::serving(vec![vec![
        // What the client said in `onOpen`, in the order it said it.
        Step::Hear,
        Step::Hear,
        Step::Hear,
        Step::Hear,
        Step::Hear,
        Step::Hear,
        // And the same six shapes back the other way.
        Step::Text(String::new()),
        Step::Text(String::from("x")),
        Step::Text("a".repeat(LARGE)),
        Step::Binary(Vec::new()),
        Step::Binary(vec![7]),
        Step::Binary(vec![7; LARGE]),
        // A heartbeat no program may see, and a message the transport has to
        // put back together.
        Step::Ping(b"beat".to_vec()),
        // Cut at octet 2 and octet 9, which is the middle of the `\u{e9}` and
        // the middle of the `\u{1f30a}`: neither piece is text on its own.
        Step::Fragments(
            [&SPANS[..2], &SPANS[2..9], &SPANS[9..]]
                .iter()
                .map(|piece| piece.to_vec())
                .collect(),
        ),
        // The program hangs up on the seventh message; this reads the close it
        // sent and answers it.
        Step::Bye(1000),
    ]]);
    let port = serving.port;
    let said = dialled_client_rounds(&client, port, LARGE);
    let sessions = serving.heard();
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    let large = "a".repeat(LARGE);
    // `Str::len` counts Unicode scalar values, so the fragmented message is
    // twelve of them and not the sixteen octets it took on the wire — and the
    // three fragments cut two of those characters in half.
    let wanted = format!(
        "text 0 \ntext 1 x\ntext {LARGE} {large}\nbinary 0\nbinary 1\nbinary {LARGE}\n\
         text 12 h\u{e9}llo \u{1f30a} done\nclosed after 7 .GoingAway\nended .GoingAway\n"
    );
    assert_eq!(
        said.stdout, wanted,
        "a message did not cross whole, or a ping reached the program.\nstderr:\n{}",
        said.stderr
    );
    // And the other direction, as the far side actually read it off the wire.
    assert_eq!(
        sessions,
        vec![vec![
            Heard::Text(String::new()),
            Heard::Text(String::from("x")),
            Heard::Text("a".repeat(LARGE)),
            Heard::Binary(Vec::new()),
            Heard::Binary(vec![7]),
            Heard::Binary(vec![7; LARGE]),
            // **The close code a program chose, as the far side read it.**
            // `.GoingAway` is 1001, and a native client writes the number
            // whole — which is the half of the mapping the row above cannot
            // see, because there every close came *in*.
            Heard::Closed(Some(1001)),
        ]],
        "what the client wrote is not what the far side read.\nthe client said:\n{}",
        said.stdout
    );
}

/// **A server that accepts the connection and then says nothing is a socket
/// that never opened, and the sentence says the server went.**
///
/// The fourth way a dial fails, beside the three rows above: not a port with
/// nobody on it, not a status that is not `101`, and not a signature for
/// somebody else's handshake — a listener that took the connection and closed
/// it without answering at all. A client that read that as an empty response
/// head would say "this is not HTTP"; what it owes the program is that the
/// server went away before it answered.
///
/// **The read deadline itself has no row here and cannot have one.** A server
/// that accepts and then *holds* the connection open is refused after thirty
/// seconds, and thirty seconds of wall clock is both a large share of this
/// suite's five-minute bar and exactly the kind of timing verdict
/// `cli/tests/README.md` forbids deciding a test with. What is asserted instead
/// is the same read's other exit — the one that answers zero — which is what a
/// real broken server actually produces.
#[test]
fn a_server_that_answers_nothing_at_all_is_a_socket_that_never_opened() {
    unless_ready!();
    let client = built("e2e-client-dial-silent", &dialling_client());
    let serving = crate::websocket::serving(vec![vec![crate::websocket::Step::Silence]]);
    let port = serving.port;
    let said = dialled_client(&client, port);
    let _sessions = serving.heard();
    assert_eq!(
        said.status, 0,
        "the client exited {}.\nstdout:\n{}\nstderr:\n{}",
        said.status, said.stdout, said.stderr
    );
    assert!(
        said.stdout.contains("client refused: .Transport"),
        "a server that answered nothing was not a transport failure.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        said.stdout.contains("before answering the handshake"),
        "the refusal never said the server went without answering.\nthe client said:\n{}",
        said.stdout
    );
    assert!(
        !said.stdout.contains("client opened") && !said.stdout.contains("client closed"),
        "a hook ran for a socket that never opened.\nthe client said:\n{}",
        said.stdout
    );
}

/// A client whose `onOpen` divides by zero.
///
/// The socket opens, the hook runs, and the hook aborts — which is the only way
/// a Buri hook can fail, because the language has no exceptions and nothing can
/// catch an abort.
fn aborting_client() -> String {
    String::from(
        r#"from "core/effect" import { Allocator, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Environment: host.env,
        Sockets: host.sockets,
        Stdout: host.stdout,
        WebSocketClient: host.websocketClient,
    };
    let port = env.withArguments(ctx).first().withDefault("0");
    let zero = port.length() - port.length();
    let dialled = websocket.connect(ctx, Client {
        url: str.format(ctx, "ws://127.0.0.1:${port}/socket"),
        onOpen: fn(c, _socket, _response) => {
            let _said = io.println(c, "the hook ran").ignore();
            // Not a literal, so it is a division the front end cannot fold
            // away: the port's own length, minus itself.
            1 / zero
        },
        onMessage: fn(_c, _socket, seen, _message) => seen + 1,
        onClose: fn(c, _socket, _seen, _reason) => io.println(c, "closed").ignore(),
    });
    let _said = match (dialled) {
        .Err(e) => io.println(ctx, "refused ${e.cause}").ignore(),
        .Ok(reason) => io.println(ctx, "ended ${reason}").ignore(),
    };
    .Ok(())
}
"#,
    )
}

/// **A hook that aborts aborts the program, and `connect` does not swallow
/// it.**
///
/// The negative twin of every row above, and the only failure a *hook* has:
/// the language has no exceptions and nothing catches an abort, so a hook that
/// divides by zero is a process that stops with the message
/// `cli/tests/crash/` pins for that operation and a status that is not zero.
///
/// It matters that this is asserted rather than assumed, because `connect` is a
/// loop with the hooks inside it: a runtime that ran a hook behind a boundary
/// which turned a fault into a value would print `ended` and exit `0`, and
/// every row above would still be green.
#[test]
fn a_hook_that_aborts_stops_the_program_rather_than_the_socket() {
    unless_ready!();
    let client = built("e2e-client-hook-aborts", &aborting_client());
    let serving = crate::websocket::serving(vec![vec![
        crate::websocket::Step::Text(String::from("never read")),
        crate::websocket::Step::Close(Some(1000)),
    ]]);
    let port = serving.port;
    let said = dialled_client(&client, port);
    let _sessions = serving.heard();
    assert_ne!(said.status, 0, "an abort inside a hook exited 0.\nstdout:\n{}", said.stdout);
    assert!(
        said.stderr.contains("division by zero"),
        "the abort did not name what went wrong.\nstderr:\n{}",
        said.stderr
    );
    assert!(
        said.stdout.contains("the hook ran"),
        "the hook never ran, so this row proved nothing about a hook.\nstdout:\n{}",
        said.stdout
    );
    assert!(
        !said.stdout.contains("ended") && !said.stdout.contains("closed"),
        "`connect` answered for a program that had already stopped.\nstdout:\n{}",
        said.stdout
    );
}
