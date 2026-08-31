//! What more than one backend suite needs, and none of them owns.
//!
//! `llvm` and `stencil` compile the same programs through two pipelines and
//! assert the same things about what came out. Where the assertion is shared,
//! the machinery has to be too: two copies of an allocation probe cannot be
//! said to agree on an allocation count, and two copies of the corpus loader
//! are two chances for one backend to be reading a different repository.

// Which backends are built decides which of these are read.
#![allow(dead_code)]

use buri::build::workspace::Workspace;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A C shim linked beside the program, whose destructor reports the runtime's
/// allocation counters once `main` has returned.
///
/// `buri_rt_heap_stats` is not reachable from Buri and should not be, so an
/// assertion about *how many times a program allocated* has to be made from
/// outside it. A destructor rather than a wrapper around `main`: the emitted
/// entry point is the one `cli/runtime/lib.rs` §6 describes, and replacing it
/// would be measuring a different program.
pub const ALLOC_PROBE: &str = r#"
#include <stdio.h>
#include <stdint.h>
typedef struct { uint64_t live_blocks, live_bytes, total_blocks, total_bytes,
                          retained_bytes, decommitted_bytes,
                          arena_bytes, arena_released_bytes; } Stats;
extern void buri_rt_heap_stats(Stats *out);
__attribute__((destructor)) static void buri_probe(void) {
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, "blocks=%llu live=%llu\n",
          (unsigned long long)s.total_blocks, (unsigned long long)s.live_blocks);
}
"#;

/// `(total_blocks, live_blocks)` from an [`ALLOC_PROBE`]-linked run.
pub fn probed(stderr: &str) -> (u64, u64) {
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("blocks="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"));
    let (total, rest) = line.split_once(" live=").unwrap();
    (total.trim().parse().unwrap(), rest.trim().parse().unwrap())
}

/// What running one program produced.
pub struct Ran {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a linked executable and collects what it said.
pub fn ran(binary: &Path) -> Ran {
    let out = Command::new(binary).output().unwrap();
    Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

// ---------------------------------------------------------------------------
// A Buri server, and a client that is not one
// ---------------------------------------------------------------------------
//
// `effect Listen`'s acceptor is `cli/runtime/net.rs` and reaching it needs a
// *client*, which no Buri program can be on a native backend yet:
// `host.HostNet.fetch` has a body in the archive and no row in either runtime
// table, because `NetError` carries a payload on two of its variants and
// `lib.rs` §2.1's `Result` shape restricts the variant a discriminant names to
// carrying none. So the client here is Rust, on the far side of a loopback
// socket — which is `runtime.rs::the_network_effect_fetches` with the two ends
// swapped, and holds to the same rule: **every wait is bounded, and the thread
// is joined**, so a server that never comes up is a failing assertion rather
// than a suite that runs until CI kills it.
//
// The port is the server's own choice. A program binding `port: 0` is handed
// one by the operating system and writes it to a file, and the client polls for
// that file — rather than the test picking a port and hoping, which is a race
// between the pick and the bind that this arrangement does not have.

/// How long the client waits for the server to publish its port, and how long
/// any one step of the exchange may take.
pub const SERVER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// A Buri program that binds one port, publishes it on standard output,
/// answers one request and stops.
///
/// `requestLimit: 1` is what makes the server *finish*: the listener says
/// `.Closed` once the answer is written, `run` turns that into `.Ok(())`, and
/// the process falls off the end of `main` — which is the shape `effect Listen`
/// promises, and the thing a test could not assert if a server could only ever
/// be killed.
///
/// **The eight kilobytes of padding are load-bearing**, and their reason is a
/// gap rather than a trick. The port has to reach the test *while the server is
/// still running*, and a native Buri program has exactly one channel out of
/// itself: `cli/runtime/host.rs` buffers standard output until eight kilobytes
/// or exit. A file and an environment variable are both unavailable — neither
/// `host.HostFs.*` nor `host.HostEnv.*` has a row in either runtime table — so
/// filling that buffer is what makes the first line readable now rather than at
/// exit. The day one of those families gets a row this becomes two lines and a
/// path.
///
/// The alternative, letting the *test* pick a port and compiling it in, was
/// rejected: seconds pass between the pick and the bind, and a port taken in
/// between is a flake rather than a failure.
pub fn one_shot_server() -> String {
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
    requestLimit: .Some(1),
    idleTimeoutMillis: .Some(20000),
  }};
  match (server.bind(ctx, plan)) {{
    .Err(e) => .Err(server.errorText(e)),
    .Ok(listener) => {{
      let pad = "x".repeat(ctx, {pad});
      let _ = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
      match (server.run(ctx, listener, plan)) {{
        .Err(e) => .Err(server.errorText(e)),
        .Ok(_ok) => {{
          let _ = io.println(ctx, "served").ignore();
          .Ok(())
        }},
      }}
    }},
  }}
}}
"#,
        pad = STDOUT_BUFFER
    )
}

/// A server that answers `requests` requests, each handler sleeping for
/// `sleep_millis` before it answers.
///
/// **The sleep is the whole instrument.** A handler that computes proves nothing
/// about concurrency on a machine with one processor free, and a handler that
/// waits proves it on any machine: the acceptor's handlers overlapping take
/// about one sleep in total, and one at a time takes `requests` of them. The gap
/// between those two numbers is what `served_many`'s caller asserts inside.
///
/// How many handlers there are is not a parameter, because it is not a knob:
/// the acceptor answers with `net.rs`'s `MAX_HANDLERS` on `Listener.handlers`
/// and `run` fans out to it. That constant being a constant — sixty-four, and
/// not a function of this machine's processor count — is what makes the timing
/// assertion predictable, and `net.rs` says so where it is declared.
pub fn concurrent_server(requests: usize, sleep_millis: usize) -> String {
    format!(
        r#"from "core/effect" import {{ Alloc, Clock, Listen, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/time" import * as time;

export fn main(): Result<(), Str> {{
  let ctx = context {{
    Alloc: host.alloc,
    Clock: host.clock,
    Listen: host.listen,
    Stdout: host.stdout,
    Tasks: host.tasks,
  }};
  let plan = server.Server {{
    port: 0,
    onRequest: fn(c, request) => {{
      let _slept = time.sleepMs(c, {sleep});
      http.text(c, request.path())
    }},
    requestLimit: .Some({requests}),
    idleTimeoutMillis: .Some(60000),
  }};
  match (server.bind(ctx, plan)) {{
    .Err(e) => .Err(server.errorText(e)),
    .Ok(listener) => {{
      let pad = "x".repeat(ctx, {pad});
      let _ = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
      match (server.run(ctx, listener, plan)) {{
        .Err(e) => .Err(server.errorText(e)),
        .Ok(_ok) => {{
          let _ = io.println(ctx, "served").ignore();
          .Ok(())
        }},
      }}
    }},
  }}
}}
"#,
        requests = requests,
        sleep = sleep_millis,
        pad = STDOUT_BUFFER
    )
}

/// `cli/runtime/host.rs`'s `FLUSH_AT`, from the other side of the C ABI.
///
/// Transcribed rather than exported, because it is a buffering policy and not a
/// contract: a runtime that flushed on every newline would make the padding
/// above harmless rather than wrong, and one that buffered *more* would make
/// this test hang — which is why the wait for the first line is bounded.
const STDOUT_BUFFER: usize = 8 * 1024;

/// Run a server binary, take the port off its first line of output, make one
/// HTTP/1.1 request, and answer `(what the program said, what came back)`.
///
/// The server's `main` does not return until it has answered, so the two halves
/// have to overlap: the child's output is read on a thread of its own while
/// this one talks to it.
///
/// **Every wait is bounded and every thread is joined**, which is the rule
/// `cli/tests/native/runtime.rs` states about its own mirror of this: a test
/// that could wait forever for a server is a suite that runs until CI kills it,
/// and the failure that produced this rule took a CI job with it.
pub fn served(binary: &Path, target: &str) -> (Ran, String) {
    use std::io::{BufRead, Read, Write};
    let mut child = Command::new(binary)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {}: {e}", binary.display()));
    let stdout = child.stdout.take().expect("a piped stdout");
    let (announced, listening) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut first = String::new();
        let _ = reader.read_line(&mut first);
        let port = first
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p > 0);
        let _ = announced.send(port);
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
        (first, rest)
    });

    let port = listening
        .recv_timeout(SERVER_DEADLINE)
        .unwrap_or_else(|e| panic!("the server announced no port within {SERVER_DEADLINE:?}: {e}"))
        .expect("the server's first line did not carry a port");

    // The port is announced before `listenAccept` is entered, so the first
    // connect can lose the race with the `listen(2)` backlog on a loaded
    // machine. Retrying until the same deadline is what makes that a slow test
    // rather than a flaky one.
    let until = std::time::Instant::now() + SERVER_DEADLINE;
    let mut socket = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(socket) => break socket,
            Err(e) => {
                assert!(
                    std::time::Instant::now() < until,
                    "could not reach the server on 127.0.0.1:{port}: {e}"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    };
    socket.set_read_timeout(Some(SERVER_DEADLINE)).unwrap();
    socket.set_write_timeout(Some(SERVER_DEADLINE)).unwrap();
    socket
        .write_all(format!("GET {target} HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n").as_bytes())
        .unwrap();
    socket.flush().unwrap();
    let mut reply = Vec::new();
    let _ = socket.read_to_end(&mut reply);

    let status = child.wait().expect("the server exited");
    let (first, rest) = reader.join().expect("the reader thread finished");
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    let ran = Ran {
        status: status.code().unwrap_or(-1),
        // The first line's padding is dropped: it is the buffer's price and not
        // anything the program meant to say.
        stdout: format!("{}\n{rest}", first.split_whitespace().take(2).collect::<Vec<_>>().join(" ")),
        stderr,
    };
    (ran, String::from_utf8_lossy(&reply).to_string())
}

/// Run a server binary and make `requests` requests **at the same time**, one
/// thread each; answer what each client got back and how long the whole
/// exchange took.
///
/// The shape is `served`'s, with the one difference that matters: the clients
/// all connect before any of them is answered, so a server that answers one
/// connection at a time takes `requests` handler-sleeps and one that answers
/// `handlers` at a time takes about `requests / handlers` of them.
///
/// **Every wait is bounded and every thread is joined**, for the reason
/// `served` states: a test that could wait forever for a server is a suite that
/// runs until CI kills it.
pub fn served_many(binary: &Path, requests: usize) -> (Ran, Vec<String>, std::time::Duration) {
    use std::io::{BufRead, Read, Write};
    let mut child = Command::new(binary)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {}: {e}", binary.display()));
    let stdout = child.stdout.take().expect("a piped stdout");
    let (announced, listening) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut first = String::new();
        let _ = reader.read_line(&mut first);
        let port = first
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p > 0);
        let _ = announced.send(port);
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
        (first, rest)
    });

    let port = listening
        .recv_timeout(SERVER_DEADLINE)
        .unwrap_or_else(|e| panic!("the server announced no port within {SERVER_DEADLINE:?}: {e}"))
        .expect("the server's first line did not carry a port");

    let started = std::time::Instant::now();
    let clients: Vec<_> = (0..requests)
        .map(|i| {
            std::thread::spawn(move || {
                // The same retry `served` uses and for the same reason: the
                // port is announced before the first accept, so a connect can
                // lose the race with the backlog on a loaded machine.
                let until = std::time::Instant::now() + SERVER_DEADLINE;
                let mut socket = loop {
                    match std::net::TcpStream::connect(("127.0.0.1", port)) {
                        Ok(socket) => break socket,
                        Err(e) => {
                            assert!(
                                std::time::Instant::now() < until,
                                "could not reach the server on 127.0.0.1:{port}: {e}"
                            );
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                };
                socket.set_read_timeout(Some(SERVER_DEADLINE)).unwrap();
                socket.set_write_timeout(Some(SERVER_DEADLINE)).unwrap();
                socket
                    .write_all(
                        format!("GET /r{i} HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n").as_bytes(),
                    )
                    .unwrap();
                socket.flush().unwrap();
                let mut reply = Vec::new();
                let _ = socket.read_to_end(&mut reply);
                String::from_utf8_lossy(&reply).to_string()
            })
        })
        .collect();
    let replies: Vec<String> =
        clients.into_iter().map(|c| c.join().expect("a client finished")).collect();
    let elapsed = started.elapsed();

    let status = child.wait().expect("the server exited");
    let (first, rest) = reader.join().expect("the reader thread finished");
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    let ran = Ran {
        status: status.code().unwrap_or(-1),
        stdout: format!(
            "{}\n{rest}",
            first.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
        ),
        stderr,
    };
    (ran, replies, elapsed)
}

/// Every client got its own path back, and nobody got somebody else's.
///
/// **This is the ordering-independence claim.** The replies come back in
/// whatever order the handlers finished, so what is asserted is the pairing and
/// not the sequence: request `i` went out on connection `i` and `/ri` came back
/// on it. A server that crossed two answers over would fail here whatever the
/// timing said.
pub fn each_answered_its_own(replies: &[String]) {
    for (i, reply) in replies.iter().enumerate() {
        assert!(
            reply.starts_with("HTTP/1.1 200 OK\r\n"),
            "client {i} was not answered 200:\n{reply}"
        );
        assert!(
            reply.ends_with(&format!("\r\n\r\n/r{i}")),
            "client {i} was answered somebody else's request:\n{reply}"
        );
    }
}

/// The three things every backend's server row asserts about the answer.
pub fn served_the_path(reply: &str, path: &str) {
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "the server did not answer 200:\n{reply}");
    assert!(
        reply.contains("content-type: text/plain; charset=utf-8\r\n"),
        "`http.text`'s content type did not survive the wire:\n{reply}"
    );
    assert!(
        reply.ends_with(&format!("\r\n\r\n{path}")),
        "the handler was handed the wrong path:\n{reply}"
    );
}

/// The conformance corpus as the repository it is, opened once per process.
///
/// Eleven files import `//lib/<package>`, so a harness that compiled each as a
/// bare snippet would be refused by the *front end* and would say nothing
/// about a backend.
pub fn conformance_repository() -> Option<&'static Workspace> {
    static REPOSITORY: OnceLock<Option<Workspace>> = OnceLock::new();
    REPOSITORY.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
        let mut map = SourceMap::new();
        let mut diagnostics = Diagnostics::new();
        let workspace = Workspace::load(&root, &mut map, &mut diagnostics).ok()?;
        if diagnostics.has_errors() {
            return None;
        }
        Some(workspace)
    })
    .as_ref()
}

/// The corpus root the file walkers start from.
pub fn conformance_corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/lib")
}
