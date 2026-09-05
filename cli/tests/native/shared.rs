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
///
/// **Under the heap check**, because every caller of this function runs a
/// whole Buri program to completion and that is exactly the population the
/// runtime's exit audit is a question about. A program that gave back every
/// block is unchanged by it; one that did not exits
/// [`HEAP_CHECK_STATUS`] with the runtime's own line on standard error, which
/// is a failure in whatever the caller was already asserting about the status.
pub fn ran(binary: &Path) -> Ran {
    ran_checked(binary)
}

/// The same without the check, for a caller that is deliberately measuring the
/// program the product ships rather than the program a harness runs.
pub fn ran_unchecked(binary: &Path) -> Ran {
    ran_command(&mut Command::new(binary))
}

/// [`ran`] for a command a caller has already configured.
pub fn ran_command(cmd: &mut Command) -> Ran {
    let out = cmd.output().unwrap();
    Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

// ---------------------------------------------------------------------------
// The universal heap invariant
// ---------------------------------------------------------------------------
//
// **Every program this domain runs is asked the same question on its way out:
// did it give back every block it took?** The counters have been in
// `cli/runtime/memory.rs` all along and a handful of suites read them through
// a linked C probe; what was missing is that a probe is opt-in, so the
// invariant held for the programs somebody remembered rather than for the
// corpus. `middle::rc.rs` is a whole-program analysis and its failures are
// silent, so "the programs somebody remembered" is exactly the wrong sample.
//
// The mode is the runtime's, described where it is implemented. Here it is one
// environment variable set on every child this domain spawns, and the two
// helpers that read the answer back.

/// The JavaScript engine this domain runs the reference backend under, or
/// `None`.
///
/// `BURI_JS` first, so every suite here answers the same question
/// `tests/harness/mod.rs` does and a machine that has configured one engine
/// does not silently get another. One copy, because `agreement.rs` and
/// `differential.rs` compare a native answer against *the* reference answer,
/// and two ideas of which engine that is would be two references.
pub fn js_engine() -> Option<String> {
    let configured = std::env::var("BURI_JS").ok();
    let candidates: Vec<String> = match configured {
        Some(js) => vec![js],
        None => vec![String::from("bun"), String::from("node")],
    };
    candidates.into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// The exit status the runtime's heap check stops a program with.
///
/// It is not 1 — an ordinary abort's — and it is not a signal, so a status of
/// exactly this means the program finished and *then* failed the invariant.
/// `cli/runtime/memory.rs`'s `BURI_RT_HEAP_CHECK_STATUS` is the other side.
pub const HEAP_CHECK_STATUS: i32 = 97;

/// The environment a Buri artifact is run under, everywhere in this domain.
///
/// `1` is the audit **and** the quarantine: freed blocks are poisoned and held
/// rather than recycled, and a reference operation that reaches one stops the
/// process. The quarantine is what makes an *over*-decrement deterministic —
/// without it a released block is recycled and the symptom is a wrong answer
/// somewhere else, on the runs where the recycling happened to matter.
pub fn heap_checked(cmd: &mut Command) -> &mut Command {
    cmd.env("BURI_RT_HEAP_CHECK", "1")
}

/// Run a linked executable under the heap check.
pub fn ran_checked(binary: &Path) -> Ran {
    let mut cmd = Command::new(binary);
    heap_checked(&mut cmd);
    ran_command(&mut cmd)
}

/// What the heap check said about a run, if it said anything.
///
/// The whole line, so that a caller reporting a failure reports the number of
/// blocks and the reason rather than "it exited 97". Taken as the status and
/// the stream rather than as a `Ran`, because every suite here has its own
/// idea of what a run is and none of them should have to convert.
pub fn heap_check_failure(status: i32, stderr: &str) -> Option<&str> {
    if status != HEAP_CHECK_STATUS {
        return None;
    }
    Some(stderr.lines().find(|l| l.starts_with("buri heap check:")).unwrap_or(
        "the program exited with the heap-check status and said nothing, which is a \
         harness bug rather than a program's",
    ))
}

/// How many blocks a run leaked, where the failure was a leak.
///
/// `None` for a clean run and for a *use after free*, which is the other thing
/// the check reports and is not a number.
pub fn leaked_blocks(status: i32, stderr: &str) -> Option<u64> {
    let line = heap_check_failure(status, stderr)?;
    let rest = line.strip_prefix("buri heap check: leak: ")?;
    let (blocks, _) = rest.split_once(" block(s)")?;
    blocks.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// The product's link line
// ---------------------------------------------------------------------------
//
// **A harness that links more permissively than the product is a harness that
// cannot see the product's bugs**, and one that links *differently* from it
// measures a toolchain nobody ships. `build/link.rs` says the same sentence
// over `product_link_args`, and this is the pair of functions that carries its
// answer into a `Command`.
//
// Every suite in this binary used to spell the flags out again — `-dead_strip`
// on macOS, `-Wl,--gc-sections -lpthread -ldl -lm` on Linux — and the Linux
// half of that spelling stopped being true the day the Linux artifact became a
// static-PIE musl one: musl folds pthread, dl and m into `libc.a` and ships no
// `libpthread.a` stub, so the old line is now `cannot find -lpthread` against
// the sysroot the product links with. There is nothing to keep in sync here
// any more; there is one call.

/// The driver `build/link.rs` would spawn, in the directory its arguments are
/// relative to.
///
/// **Both halves matter.** The driver, because the `musl-clang` tier answers
/// "which libc" with the *program*: a harness that kept spawning `$CC` there
/// would link glibc while the product linked musl. The directory, because the
/// arguments name `musl/lib` relatively — that is the product's own
/// reproducibility discipline (ARCHITECTURE.md §7), and a link run somewhere
/// else would not find the staged sysroot at all.
///
/// Every other path a harness passes — the objects, the archive, `-o` — is
/// absolute, which is what lets one staging directory serve every link in the
/// process instead of one per program. That is not a micro-optimisation: the
/// sysroot is eleven files and about 6.6 MB, and the corpus census links
/// thirty-six programs while the fuzzer links as many as its budget buys.
pub fn product_cc() -> Command {
    let mut cmd = Command::new(
        buri::build::link::product_link_driver()
            .unwrap_or_else(|| PathBuf::from(std::env::var("CC").unwrap_or_else(|_| "cc".into()))),
    );
    cmd.current_dir(&staged().0);
    cmd
}

/// The `--target=` a *compile* needs to agree with the link below it.
///
/// `build/link.rs::product_compile_args` is the answer and this is the memo
/// for it: the two `-c` probe compiles in this binary would otherwise be
/// compiled for the driver's default target — glibc on a Debian host — and
/// linked against the baked musl sysroot. Empty everywhere except the baked
/// tier, where it is one flag; the reasoning for that scoping is over there,
/// beside the link flags it has to agree with.
pub fn product_compile_args() -> &'static [String] {
    static ARGS: OnceLock<Vec<String>> = OnceLock::new();
    ARGS.get_or_init(buri::build::link::product_compile_args)
}

/// The arguments the product passes **after** the objects and the archive.
///
/// Order is part of the claim: `-lunwind` resolves against what precedes it,
/// so a harness that put these first would be linking a different command
/// line. Empty on a host that cannot link at all — no `cc`, or the hermetic
/// refusal — which is not permissiveness, because the link that follows is
/// about to fail for that same reason.
pub fn product_link_args() -> &'static [String] {
    &staged().1
}

/// The staging directory and the arguments for it, once per process.
///
/// Named for the process like every other tree in here, and swept by the next
/// run for the reason `harness/sweep.rs` gives: 6.6 MB that nothing deletes is
/// a disk in two heavy sessions.
fn staged() -> &'static (PathBuf, Vec<String>) {
    static STAGED: OnceLock<(PathBuf, Vec<String>)> = OnceLock::new();
    STAGED.get_or_init(|| {
        crate::sweep::once();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("native-link-flags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let args = buri::build::link::product_link_args(&dir);
        (dir, args)
    })
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
pub const STDOUT_BUFFER: usize = 8 * 1024;

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
        .env("BURI_RT_HEAP_CHECK", "1")
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
        .env("BURI_RT_HEAP_CHECK", "1")
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

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------

/// `SIGTERM`, which is what a supervisor sends. Fifteen on both platforms this
/// suite runs on; `cli/runtime/net.rs`'s `shutdown` module is where the same
/// two numbers are written on the other side of the C ABI.
pub const SIGTERM: i32 = 15;

/// `SIGINT`, which is what a person at a terminal sends.
pub const SIGINT: i32 = 2;

// The one call this file makes into the C library, declared rather than
// depended on — `cli/runtime/memory.rs`'s `mmap` block is the precedent, and
// the argument is the same one: a dependency for a single declaration is a
// dependency.
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// A Buri server that **cannot stop on its own**, so that the only thing that
/// can end it is a signal.
///
/// No `requestLimit` and no `idleTimeoutMillis`: the two fields that let every
/// other server fixture in this file finish are deliberately absent, so a
/// `.Ok(())` out of `serve` — which is what "served" on the last line reports —
/// can only have come from the drain. A shutdown that did not work leaves a
/// process running forever, which is why every wait in [`signalled`] is bounded
/// and why it kills what it could not stop.
///
/// **The handler announces itself before it sleeps**, and that is the whole
/// instrument. The signal has to arrive while a request is genuinely in flight,
/// and "sleep a bit after writing and hope" is a race rather than a test. So the
/// handler prints a line, fills the output buffer to flush it (the same eight
/// kilobytes and the same reason as the port above), and only then sleeps: when
/// the test sees that line, the request is provably inside a handler.
pub fn draining_server(sleep_millis: usize) -> String {
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
      let _handling = io.println(c, "handling").ignore();
      let flush = "x".repeat(c, {pad});
      let _flushed = io.println(c, flush).ignore();
      let _slept = time.sleepMs(c, {sleep});
      http.text(c, request.path())
    }},
    drainMillis: .Some(10000),
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
        sleep = sleep_millis,
        pad = STDOUT_BUFFER
    )
}

/// Run a server binary, put one request into a handler, **signal the child
/// while that request is in flight**, and answer what the client got back.
///
/// This is F5's fixture and its shape is the claim: the signal is not sent
/// until the child has said its handler is running, so what is being asserted
/// is that a request already in a handler is answered *after* the process was
/// told to stop — and that the process then ends by itself, with `serve`
/// answering `.Ok(())` and `main` falling off its end.
///
/// **Every wait is bounded, including the one on the child.** `Child::wait` has
/// no deadline, and a shutdown test that could wait forever for a process that
/// will not stop is the failure this repository already had once. So the wait
/// is a bounded `try_wait` loop and a `SIGKILL` on the way out: a broken drain
/// is a failing test with a message rather than a job CI has to kill.
pub fn signalled(binary: &Path, signal: i32) -> (Ran, String) {
    use std::io::{BufRead, Read, Write};
    let mut child = Command::new(binary)
        .env("BURI_RT_HEAP_CHECK", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {}: {e}", binary.display()));
    let stdout = child.stdout.take().expect("a piped stdout");
    let (said, saying) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        let mut lines: Vec<String> = Vec::new();
        for line in reader.lines() {
            let Ok(line) = line else { break };
            // The padding is the output buffer's price and not anything the
            // program meant to say, so it is dropped here rather than carried
            // through every assertion below.
            let short: String = line
                .split_whitespace()
                .take(2)
                .filter(|token| !token.starts_with("xxxx"))
                .collect::<Vec<_>>()
                .join(" ");
            if short.is_empty() {
                continue;
            }
            let _ = said.send(short.clone());
            lines.push(short);
        }
        lines
    });

    // **One deadline for the whole wait and not one per line**, which is the
    // difference between a bound and a bound per iteration: a child printing a
    // line a second would otherwise renew its own reprieve forever.
    let port = saying_until(&saying, SERVER_DEADLINE, |line| {
        line.strip_prefix("port ").and_then(|p| p.parse::<u16>().ok())
    });
    let port = port.unwrap_or_else(|| {
        let _ = child.kill();
        panic!("the server announced no port within {SERVER_DEADLINE:?}");
    });

    // The same retry `served` uses and for the same reason: the port is
    // announced before the first accept, so a connect can lose the race with
    // the backlog on a loaded machine.
    let until = std::time::Instant::now() + SERVER_DEADLINE;
    let mut socket = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(socket) => break socket,
            Err(e) => {
                if std::time::Instant::now() >= until {
                    let _ = child.kill();
                    panic!("could not reach the server on 127.0.0.1:{port}: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    };
    socket.set_read_timeout(Some(SERVER_DEADLINE)).unwrap();
    socket.set_write_timeout(Some(SERVER_DEADLINE)).unwrap();
    socket
        .write_all(b"GET /in-flight HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n")
        .unwrap();
    socket.flush().unwrap();

    // And now wait until the handler says it is running, which is what makes
    // the signal land on a request that is genuinely in flight rather than on
    // one that might be.
    let handling = saying_until(&saying, SERVER_DEADLINE, |line| (line == "handling").then_some(()));
    if handling.is_none() {
        let _ = child.kill();
        panic!("the server never entered its handler within {SERVER_DEADLINE:?}");
    }

    let pid = i32::try_from(child.id()).expect("a process id");
    // SAFETY: an ordinary `kill` on this test's own child.
    let sent = unsafe { kill(pid, signal) };
    assert_eq!(sent, 0, "could not signal the server with {signal}");

    let mut reply = Vec::new();
    let _ = socket.read_to_end(&mut reply);

    let status = waited(&mut child, SERVER_DEADLINE);
    let lines = reader.join().expect("the reader thread finished");
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    let ran = Ran {
        status: status.code().unwrap_or(-1),
        stdout: format!("{}\n", lines.join("\n")),
        stderr,
    };
    (ran, String::from_utf8_lossy(&reply).to_string())
}

/// What became of a child that was signalled more than once.
///
/// `code` and `killed_by` are the two halves of a Unix exit status and exactly
/// one of them is ever `Some`: a process that returned from `main` has a code,
/// and one the kernel ended has a signal number. A row that read only the code
/// would see `-1` for both and could not tell a drain that failed from a
/// process that was killed, which is the whole distinction being asserted.
pub struct Stopped {
    /// Every line the child said, in order, with the output buffer's padding
    /// dropped.
    pub said: String,
    /// The status `main` returned with, where it returned at all.
    pub code: Option<i32>,
    /// The signal that ended it, where one did.
    pub killed_by: Option<i32>,
}

/// Put a request into a handler, signal the child, and **keep signalling it**
/// until it stops or the deadline runs out.
///
/// **Written beside [`signalled`] rather than sharing its body**, which is the
/// argument [`announced`] makes about itself one section down: what the two
/// have in common is a shape and not a dependency, and rewriting a working
/// shutdown row to prove it is churn. What differs is the whole point — this
/// one does not wait for the drain, it interrupts it.
///
/// **The signal is repeated rather than sent exactly twice**, and that is a
/// deliberate strengthening rather than a weakening. `cli/runtime/net.rs`'s
/// handler restores the default disposition *first*, before it writes into its
/// self-pipe, so the second signal to arrive is the operating system's — but
/// "arrive" is the kernel's business and not this test's, and a fixed sleep
/// between two `kill`s would be a race with the delivery of the first. Sending
/// until the child is gone makes a runtime that restored the disposition stop
/// at once, and makes one that had made itself unkillable fail here at the
/// deadline with a sentence, which is the only other thing it could be.
///
/// Every wait is bounded and the child is killed on the way out, for
/// [`waited`]'s reason.
pub fn signalled_twice(binary: &Path, signal: i32) -> Stopped {
    use std::io::{BufRead, Read, Write};
    use std::os::unix::process::ExitStatusExt;

    let mut child = Command::new(binary)
        .env("BURI_RT_HEAP_CHECK", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {}: {e}", binary.display()));
    let stdout = child.stdout.take().expect("a piped stdout");
    let (says, saying) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        let mut lines: Vec<String> = Vec::new();
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let short: String = line
                .split_whitespace()
                .take(2)
                .filter(|token| !token.starts_with("xxxx"))
                .collect::<Vec<_>>()
                .join(" ");
            if short.is_empty() {
                continue;
            }
            let _ = says.send(short.clone());
            lines.push(short);
        }
        lines
    });

    let port = saying_until(&saying, SERVER_DEADLINE, |line| {
        line.strip_prefix("port ").and_then(|p| p.parse::<u16>().ok())
    });
    let port = port.unwrap_or_else(|| {
        let _ = child.kill();
        panic!("the server announced no port within {SERVER_DEADLINE:?}");
    });

    let until = std::time::Instant::now() + SERVER_DEADLINE;
    let mut socket = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(socket) => break socket,
            Err(e) => {
                if std::time::Instant::now() >= until {
                    let _ = child.kill();
                    panic!("could not reach the server on 127.0.0.1:{port}: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    };
    socket.set_read_timeout(Some(SERVER_DEADLINE)).unwrap();
    socket.set_write_timeout(Some(SERVER_DEADLINE)).unwrap();
    socket.write_all(b"GET /in-flight HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n").unwrap();
    socket.flush().unwrap();

    // The handler says so before it sleeps, so from here the request is
    // provably inside one rather than probably.
    let handling = saying_until(&saying, SERVER_DEADLINE, |line| (line == "handling").then_some(()));
    if handling.is_none() {
        let _ = child.kill();
        panic!("the server never entered its handler within {SERVER_DEADLINE:?}");
    }

    signalling(&child, signal);
    let until = std::time::Instant::now() + SERVER_DEADLINE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => panic!("could not ask whether the server had exited: {e}"),
        }
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the server was still running {SERVER_DEADLINE:?} after being signalled with \
                 {signal} over and over, so this runtime cannot be stopped"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        signalling(&child, signal);
    };

    let lines = reader.join().expect("the reader thread finished");
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    let _ = stderr;
    Stopped {
        said: format!("{}\n", lines.join("\n")),
        code: status.code(),
        killed_by: status.signal(),
    }
}

/// The first line the child says that `read` makes something of, within one
/// deadline for the whole wait.
fn saying_until<T>(
    saying: &std::sync::mpsc::Receiver<String>,
    within: std::time::Duration,
    read: impl Fn(&str) -> Option<T>,
) -> Option<T> {
    let until = std::time::Instant::now() + within;
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        let Ok(line) = saying.recv_timeout(left) else { return None };
        if let Some(found) = read(&line) {
            return Some(found);
        }
    }
}

/// Send one signal to a child of this test.
///
/// The one call into the C library this file makes is declared above rather
/// than depended on; this is the wrapper the rows outside `signalled` reach
/// for, so that `kill` itself stays private to the module that declares it.
pub fn signalling(child: &std::process::Child, signal: i32) {
    let pid = i32::try_from(child.id()).expect("a process id");
    // SAFETY: an ordinary `kill` on a child this suite started.
    let sent = unsafe { kill(pid, signal) };
    assert_eq!(sent, 0, "could not signal the server with {signal}");
}

/// Wait for a child, with a deadline — `Child::wait` has none.
///
/// A process that will not stop is killed rather than waited on forever, and
/// the panic says which of the two happened. That is the whole of the rule this
/// file states about every other wait, applied to the one std does not bound.
pub fn waited(child: &mut std::process::Child, within: std::time::Duration) -> std::process::ExitStatus {
    let until = std::time::Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {}
            Err(e) => panic!("could not ask whether the server had exited: {e}"),
        }
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the server did not stop within {within:?} of being signalled");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// What every backend's shutdown row asserts about what came back.
pub fn drained_gracefully(out: &Ran, reply: &str) {
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    // The request that was in a handler when the signal arrived was answered
    // whole — a status line, `http.text`'s content type, and the path the
    // handler was given.
    served_the_path(reply, "/in-flight");
    // And `serve` answered `.Ok(())`, which is the last line `main` prints.
    // This fixture has no request limit and no idle deadline, so there is no
    // other way for that line to have been reached.
    assert!(
        out.stdout.ends_with("served\n"),
        "the server did not run to its own end after the signal:\n{}",
        out.stdout
    );
}

// ---------------------------------------------------------------------------
// TLS
// ---------------------------------------------------------------------------
//
// **What a native row can assert about TLS, and what it deliberately cannot.**
// A test here can build a Buri program, run it, and read what it printed; it
// cannot *talk* TLS to it, because a TLS client needs `rustls` and the only
// `rustls` in this repository is inside the runtime archive — a dev-dependency
// on it in `cli/Cargo.toml` fails `dependencies_stay_behind_the_bar`, which is
// the wall C7 met and wrote down.
//
// So the handshake, ALPN and HTTP/2's multiplexing are asserted where both ends
// are reachable, in `cli/runtime/net.rs`'s own tests, and what these rows
// assert is the half only they can: that a `Server`'s `tls` field survives the
// whole toolchain — the `[Serve]` plan `bind` renders, the list of tagged
// options crossing the C ABI at each backend's own layout, and the acceptor
// reading a certificate off the path it was given. A refusal naming the path is
// proof the `Str` payload arrived intact; a successful bind is proof the
// certificate was read and `rustls` accepted it.

/// The certificate the fixture below presents, and the key that goes with it.
///
/// **A copy of `cli/runtime/tls.rs`'s test fixture, and the copy is deliberate.**
/// That module is the source of truth — it records the `openssl` commands that
/// produced these, and the leaf's `DNS:localhost` and 2048 expiry are argued
/// there — and it is `#[cfg(test)]` inside a package this suite cannot import
/// from. The two corpora are independent by construction (C7 §3), so the fixture
/// is duplicated rather than shared, and this comment is where a maintainer
/// regenerating one is told to regenerate the other.
pub const TLS_LEAF_PEM: &str = "\
-----BEGIN CERTIFICATE-----
MIIBwzCCAWigAwIBAgIUB7wmYz/reLH8tSqdnHa/BR/gebcwCgYIKoZIzj0EAwIw
HzEdMBsGA1UEAwwUYnVyaSBydW50aW1lIHRlc3QgQ0EwHhcNMjYwODMwMTYzMDM4
WhcNNDgwNzI1MTYzMDM4WjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjO
PQIBBggqhkjOPQMBBwNCAAQOZuolOZh48E1a/BM/6evUztl8opNvN36cRROHvFG5
TJfrBSfH3IXkHfALHOC4nsMZgUIK1DDUYy/eh0P1jYuYo4GMMIGJMAwGA1UdEwEB
/wQCMAAwDgYDVR0PAQH/BAQDAgeAMBMGA1UdJQQMMAoGCCsGAQUFBwMBMBQGA1Ud
EQQNMAuCCWxvY2FsaG9zdDAdBgNVHQ4EFgQUJheVI1V2yv/bAsVsv7BpVcbp/Hgw
HwYDVR0jBBgwFoAUwNSr5KDm70fvAJ+1UOaxnPImzKMwCgYIKoZIzj0EAwIDSQAw
RgIhAKAUNh0Y9nOCGTYhCwKgc68ih70uKRmbikS+DOzEicJcAiEA+hYUHrQ/rPHi
f5ZwnkOLTUiDfd4nyoY9skQKNg8V9CU=
-----END CERTIFICATE-----
";

/// The leaf's private key, PKCS#8. A test fixture and nothing else: it has
/// signed one certificate, for `localhost`, that no trust store on earth
/// carries.
pub const TLS_LEAF_KEY_PEM: &str = "\
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgVHRl2YgdCy3mjKbk
16fSBF1ppkf1hD5sToo8d62EJnuhRANCAAQOZuolOZh48E1a/BM/6evUztl8opNv
N36cRROHvFG5TJfrBSfH3IXkHfALHOC4nsMZgUIK1DDUYy/eh0P1jYuY
-----END PRIVATE KEY-----
";

/// The two PEM files a fixture names, written beside the binary that will read
/// them, and a third path that is deliberately not there.
///
/// Named for the row that asked, so two rows running at once do not share a
/// file — and written under the process's own temporary directory rather than
/// the repository, because a certificate is not source.
pub fn tls_identity(row: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("buri-tls-{}-{row}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory for the certificate");
    let certificate = dir.join("leaf.pem");
    let key = dir.join("leaf.key");
    let absent = dir.join("nothing-here.pem");
    std::fs::write(&certificate, TLS_LEAF_PEM).expect("the certificate");
    std::fs::write(&key, TLS_LEAF_KEY_PEM).expect("the key");
    let _ = std::fs::remove_file(&absent);
    (certificate, key, absent)
}

/// A program that asks for TLS three ways and prints what it was told.
///
/// It never serves a request, and that is what makes it a *bind* fixture: every
/// claim it makes is about the plan reaching the acceptor and the acceptor's
/// answer coming back, which is exactly the half `cli/runtime/net.rs`'s own
/// cases cannot reach through the toolchain.
///
/// Three lines out, in order:
///
/// * `h2 <sentence>` — `.Http2` with no certificate, refused at the bind. This
///   one proves a `.Speak` variant crossed: nothing else in the plan could have
///   produced the refusal.
/// * `missing <sentence>` — a certificate path that is not a file. The sentence
///   carries the path, which is a `Str` payload of a `.Certificate` making the
///   round trip and coming back inside a `ServeError.detail`.
/// * `opened <port>` — a real certificate and a real key, bound. A port above
///   zero is the acceptor having read both files and `rustls` having accepted
///   the pair.
pub fn tls_server(certificate: &Path, key: &Path, absent: &Path) -> String {
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
  let h2 = server.Server {{
    port: 0,
    onRequest: fn(_c, _request) => http.status(204),
    protocols: .Some([.Http2]),
    idleTimeoutMillis: .Some(200),
  }};
  let _h2 = match (server.bind(ctx, h2)) {{
    .Err(e) => io.println(ctx, "h2 ${{e.detail}}").ignore(),
    .Ok(_listener) => io.println(ctx, "h2 bound, which it should not have").ignore(),
  }};
  let missing = server.Server {{
    port: 0,
    onRequest: fn(_c, _request) => http.status(204),
    tls: .Some(server.Tls {{ certificate: "{absent}", key: "{key}" }}),
    idleTimeoutMillis: .Some(200),
  }};
  let _missing = match (server.bind(ctx, missing)) {{
    .Err(e) => io.println(ctx, "missing ${{e.detail}}").ignore(),
    .Ok(_listener) => io.println(ctx, "missing bound, which it should not have").ignore(),
  }};
  let secured = server.Server {{
    port: 0,
    onRequest: fn(_c, _request) => http.status(204),
    protocols: .Some([.Http1, .Http2]),
    tls: .Some(server.Tls {{ certificate: "{certificate}", key: "{key}" }}),
    idleTimeoutMillis: .Some(200),
  }};
  match (server.bind(ctx, secured)) {{
    .Err(e) => .Err(server.errorText(e)),
    .Ok(listener) => {{
      let _opened = io.println(ctx, "opened ${{listener.port}}").ignore();
      // The idle deadline is what ends this, and `run` gives the port back on
      // its way out — so the program finishes rather than being killed.
      let _served = server.run(ctx, listener, secured);
      .Ok(())
    }},
  }}
}}
"#,
        certificate = certificate.display(),
        key = key.display(),
        absent = absent.display(),
    )
}

/// The three claims [`tls_server`] prints, asserted the same way on every
/// backend.
pub fn tls_bind_answers(stdout: &str, absent: &Path) {
    let h2 = stdout
        .lines()
        .find(|line| line.starts_with("h2 "))
        .unwrap_or_else(|| panic!("the program printed no h2 line:\n{stdout}"));
    assert!(
        h2.contains("ALPN"),
        "`.Http2` without a certificate was not refused for the reason it should be:\n{h2}"
    );
    let missing = stdout
        .lines()
        .find(|line| line.starts_with("missing "))
        .unwrap_or_else(|| panic!("the program printed no missing line:\n{stdout}"));
    assert!(
        missing.contains(&absent.display().to_string()),
        "the refusal does not name the certificate it could not read, so the `Str` payload of a \
         `Serve::Certificate` did not survive the round trip:\n{missing}"
    );
    let opened = stdout
        .lines()
        .find_map(|line| line.strip_prefix("opened "))
        .unwrap_or_else(|| panic!("the program printed no opened line:\n{stdout}"));
    let port: u32 = opened.trim().parse().unwrap_or_else(|_| panic!("a port, not {opened:?}"));
    assert!(port > 0, "a secured listener was not given a port: {port}");
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

// -----------------------------------------------------------------------------
// WebSockets
// -----------------------------------------------------------------------------

/// The note's counter-per-socket example, as a program that finishes.
///
/// **This is the design row's first named test**, and it is here rather than in
/// the conformance corpus because the middle of it — a message arriving on a
/// socket and an answer going back — needs an acceptor, and the only acceptor
/// is the one in the runtime archive. What the corpus can say about a socket
/// with no network behind it, it says.
///
/// `requestLimit: 1` is what makes it finish, exactly as it is for
/// [`one_shot_server`]: the upgrade is the one request this listener hands out,
/// so once the socket closes the worker's next `listenAccept` is `.Closed` and
/// `main` falls off its own end.
///
/// The socket's state is its counter's address, which is the note's own
/// sentence: `onOpen` starts an actor, every `onMessage` is handed the address
/// the last one answered, and `onClose` stops it. Nothing keys anything by
/// socket.
pub fn counting_socket_server() -> String {
    format!(
        r#"from "core/actor" import * as actor;
from "core/actor" import {{ Actor, Reply }};
from "core/effect" import {{ Alloc, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/str" import * as str;

enum Counting {{
  Increment,
  Get(Reply<Int>),
}}

fn counter<C: Alloc + Tasks>(): Actor<C, Int, Counting> {{
  Actor {{
    state: 0,
    step: fn(c, count, message) => {{
      match (message) {{
        .Increment => count + 1,
        .Get(reply) => {{
          let _answered = reply.answer(c, count);
          count
        }},
      }}
    }},
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
      path: "/socket",
      onOpen: fn(c, _socket, _request) => actor.start(c, counter()),
      onMessage: fn(c, socket, counted, _message) => {{
        let _posted = counted.send(c, .Increment);
        let count = counted.ask(c, fn(reply) => .Get(reply)).withDefault(0);
        let said = str.format(c, "messages so far: ${{count}}");
        let _sent = socket.send(c, .Text(said));
        counted
      }},
      onClose: fn(c, _socket, counted, reason) => {{
        let _stopped = counted.stop(c);
        let _said = io.println(c, "closed ${{reason.show(c)}}").ignore();
        ()
      }},
    }}),
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

/// The note's broadcast-actor example, as a program that finishes.
///
/// **The design row's second named test, and F6's forecast spent** — with one
/// correction to it, which is this slice's own finding and is worth the
/// paragraph.
///
/// F6 wrote that "a socket hook that `send`s into an actor is the *driver* of
/// that actor for the duration of its own call, so a broadcast happens when
/// somebody publishes". The first half is right and the second is not, and the
/// reason is in `core/actor`'s `send`: it posts and returns, and it drives the
/// mailbox down only when the post found it **crowded**. So a room driven by
/// `send` alone steps nothing until something else drives it — `ask`, `stop`,
/// or a sixty-fourth message — and a broadcast that used the note's literal
/// spelling would arrive when the server shut down. That is F6's own stated
/// cost ("an actor does not run *between* the calls that drive it") reaching
/// its first caller, and it is not a bug in either module.
///
/// So `Publish` carries a `Reply<Int>` and the hook **asks**: the answer is how
/// many sockets the message was pushed to, the ask is what runs the mailbox
/// down, and the publisher gets a receipt instead of a promise. Everything else
/// is the note's — an actor holding `[Socket]`, sent to from a socket hook,
/// pushing to sockets its own worker does not own. The day an actor gets a task
/// of its own, `send` is the spelling again and nothing else here moves.
///
/// It needs two sockets open at once, so it needs two workers, so it is an
/// LLVM row rather than a pair: the frame-threaded backend runs a `parallel`
/// in index order because a program it builds has one Buri data stack, and
/// worker zero holding a socket is worker zero never returning.
pub fn broadcasting_socket_server(members: usize) -> String {
    format!(
        r#"from "core/actor" import * as actor;
from "core/actor" import {{ Actor, Reply }};
from "core/effect" import {{ Alloc, Listen, Sockets, Stdout, Tasks }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/net/server" import {{ Message, Socket }};

enum Room {{
  Joined(Socket),
  Left(Socket),
  Publish(Message, Reply<Int>),
}}

fn room<C: Alloc + Sockets + Tasks>(): Actor<C, [Socket], Room> {{
  Actor {{
    state: [],
    step: fn(c, members, message) => {{
      match (message) {{
        .Joined(socket) => members.push(c, socket),
        .Left(socket) => members.filter(c, fn(m) => m != socket),
        .Publish(m, reply) => {{
          let _pushed = members.foldCtx(
            c,
            fn(inner, _sofar, socket) => socket.send(inner, m),
            (),
          );
          let _answered = reply.answer(c, members.len());
          members
        }},
      }}
    }},
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
  let members = actor.start(ctx, room());
  let plan = server.Server {{
    port: 0,
    onRequest: fn(_c, _request) => http.status(404),
    requestLimit: .Some({members}),
    idleTimeoutMillis: .Some(20000),
    websocket: .Some(server.WebSocket {{
      path: "/socket",
      onOpen: fn(c, socket, _request) => {{
        let _joined = members.send(c, .Joined(socket));
        socket
      }},
      onMessage: fn(c, _socket, mine, message) => {{
        let reached = members
          .ask(c, fn(reply) => .Publish(message, reply))
          .withDefault(0);
        let _said = io.println(c, "published to ${{reached}}").ignore();
        mine
      }},
      onClose: fn(c, _socket, mine, _reason) => {{
        let _left = members.send(c, .Left(mine));
        ()
      }},
    }}),
  }};
  match (server.bind(ctx, plan)) {{
    .Err(e) => .Err(server.errorText(e)),
    .Ok(listener) => {{
      let pad = "x".repeat(ctx, {pad});
      let _ = io.println(ctx, "port ${{listener.port}} ${{pad}}").ignore();
      let outcome = server.run(ctx, listener, plan);
      let _stopped = members.stop(ctx);
      match (outcome) {{
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
        members = members,
        pad = STDOUT_BUFFER
    )
}

/// A running server binary, its output reader, and the port it announced.
///
/// The three lines every server row in this file opens with, factored out
/// because the socket rows need them twice and their preamble is otherwise
/// exactly [`served`]'s. The existing rows are left as they are: what they
/// share is a shape rather than a dependency, and rewriting four working tests
/// to prove that is churn.
type Announced = (std::process::Child, std::thread::JoinHandle<(String, String)>, u16);

/// Start a server binary and read the port off its first line, bounded.
pub fn announced(binary: &Path) -> Announced {
    use std::io::{BufRead, Read};
    let mut child = Command::new(binary)
        .env("BURI_RT_HEAP_CHECK", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {}: {e}", binary.display()));
    let stdout = child.stdout.take().expect("a piped stdout");
    let (says, listening) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut first = String::new();
        let _ = reader.read_line(&mut first);
        let port = first
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p > 0);
        let _ = says.send(port);
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
        (first, rest)
    });
    let port = listening
        .recv_timeout(SERVER_DEADLINE)
        .unwrap_or_else(|e| panic!("the server announced no port within {SERVER_DEADLINE:?}: {e}"))
        .expect("the server's first line did not carry a port");
    (child, reader, port)
}

/// Wait for an announced server to exit, bounded, and collect what it said.
pub fn finished(announced: Announced) -> Ran {
    use std::io::Read;
    let (mut child, reader, _port) = announced;
    let status = waited(&mut child, SERVER_DEADLINE);
    let (first, rest) = reader.join().expect("the reader thread finished");
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    Ran {
        status: status.code().unwrap_or(-1),
        stdout: format!(
            "{}\n{rest}",
            first.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
        ),
        stderr,
    }
}

/// A WebSocket client, hand-written.
///
/// **Because there is nowhere to get one from.** The only RFC 6455
/// implementation in this repository is inside the runtime archive, and a
/// dev-dependency on `tungstenite` from `cli/Cargo.toml` fails
/// `dependencies_stay_behind_the_bar` — which is C7's wall, in its third
/// appearance, and it draws the same line it drew for TLS: the framing is
/// asserted where both ends are reachable (`cli/runtime/net.rs`'s own tests,
/// which may use the crate) and the toolchain rows assert the half only they
/// can, which is that a *Buri program* holds the socket.
///
/// It is deliberately the smallest client that is correct for what these rows
/// send: one text frame at a time, no fragmentation, and a close. Masking is
/// not optional — RFC 6455 §5.3 requires a client to mask every frame and this
/// acceptor enforces it — so the mask is here rather than skipped.
pub struct Talking {
    socket: std::net::TcpStream,
    /// Bytes read past the end of the handshake, or past the end of a frame.
    over: Vec<u8>,
    /// The code on the close frame the server sent, once one has been read.
    ///
    /// Kept rather than answered from [`Talking::heard`], because a reader that
    /// stops at the close still wants to say *why* it stopped: `.Overflow` on
    /// this side of the wire is 1011, and a row that could only report "the
    /// socket ended" could not tell an overflow from a normal close.
    closed: Option<u16>,
}

impl Talking {
    /// Connect, upgrade, and check the `101`.
    ///
    /// The connect retries until the deadline for [`served`]'s reason: the port
    /// is announced before `listenAccept` is entered, so the first connect can
    /// lose the race with the listen backlog on a loaded machine.
    pub fn to(port: u16) -> Talking {
        Talking::to_at(port, "/socket")
    }

    /// The same, asking for a path of the row's own choosing.
    pub fn to_at(port: u16, target: &str) -> Talking {
        use std::io::{Read, Write};
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
            .write_all(upgrade_request(target).as_bytes())
            .expect("the upgrade request");
        socket.flush().expect("flush");
        // The head, up to and including its blank line. Anything after it is a
        // frame that arrived in the same read and is kept.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let read = socket.read(&mut byte).expect("the upgrade response");
            assert_eq!(read, 1, "the server closed during the handshake");
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            assert!(head.len() < 8 * 1024, "no blank line in the upgrade response");
        }
        let head = String::from_utf8_lossy(&head).to_string();
        assert!(
            head.starts_with("HTTP/1.1 101 "),
            "the server did not switch protocols: {head}"
        );
        assert!(
            head.to_ascii_lowercase().contains("sec-websocket-accept:"),
            "the 101 carried no accept key: {head}"
        );
        Talking { socket, over: Vec::new(), closed: None }
    }

    /// The close code the server sent, once [`Talking::heard`] has answered
    /// `None`. `None` here is a socket that ended without a close frame.
    pub fn closed_with(&mut self) -> Option<u16> {
        while self.closed.is_none() && self.heard().is_some() {}
        self.closed
    }

    /// One masked text frame.
    pub fn say(&mut self, text: &str) {
        self.frame(0x1, text.as_bytes());
    }

    /// A masked close, code 1000.
    pub fn hush(&mut self) {
        self.frame(0x8, &[0x03, 0xe8]);
    }

    fn frame(&mut self, opcode: u8, payload: &[u8]) {
        use std::io::Write;
        // A fixed mask rather than a random one: masking exists to keep an
        // intermediary from being confused by attacker-chosen bytes, and there
        // is no intermediary between two threads of one test.
        let mask = [0x21u8, 0x43, 0x65, 0x87];
        let mut frame = vec![0x80 | opcode];
        if payload.len() < 126 {
            frame.push(0x80 | payload.len() as u8);
        } else {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (i, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask[i % 4]);
        }
        self.socket.write_all(&frame).expect("a frame out");
        self.socket.flush().expect("flush");
    }

    /// The next text message, or `None` once the socket has closed.
    ///
    /// Ping and pong are skipped rather than answered, because this acceptor
    /// answers pings itself and a client that had to would be testing its own
    /// heartbeat rather than the server's.
    pub fn heard(&mut self) -> Option<String> {
        loop {
            let first = self.exactly(2)?;
            let opcode = first[0] & 0x0f;
            // A server never masks (RFC 6455 §5.1), so the length is the low
            // seven bits and there is no mask to skip.
            assert_eq!(first[1] & 0x80, 0, "the server masked a frame");
            let len = match first[1] & 0x7f {
                126 => {
                    let two = self.exactly(2)?;
                    u64::from(u16::from_be_bytes([two[0], two[1]]))
                }
                127 => {
                    let eight = self.exactly(8)?;
                    u64::from_be_bytes(eight.try_into().expect("eight bytes"))
                }
                short => u64::from(short),
            };
            let payload = self.exactly(len as usize)?;
            match opcode {
                0x1 => return Some(String::from_utf8_lossy(&payload).to_string()),
                0x8 => {
                    self.closed = payload
                        .get(..2)
                        .map(|two| u16::from_be_bytes([two[0], two[1]]));
                    return None;
                }
                // A continuation, a binary frame, a ping or a pong: not what
                // these rows send, and not something to fail on.
                _ => continue,
            }
        }
    }

    /// Exactly `n` bytes, or `None` if the socket ended first.
    fn exactly(&mut self, n: usize) -> Option<Vec<u8>> {
        use std::io::Read;
        while self.over.len() < n {
            let mut chunk = [0u8; 1024];
            match self.socket.read(&mut chunk) {
                Ok(0) => return None,
                Ok(read) => self.over.extend_from_slice(&chunk[..read]),
                Err(_) => return None,
            }
        }
        Some(self.over.drain(..n).collect())
    }
}

/// The upgrade request [`Talking`] sends, and the one the fall-through row
/// sends to a server that has no hooks to answer it with.
///
/// Every clause RFC 6455 §4.2.1 asks for, including the multi-valued
/// `connection` a browser actually sends — `cli/runtime/net.rs`'s
/// `an_upgrade_request_is_the_five_things_rfc_6455_asks_for` is the row that
/// says a naive comparison would refuse it.
pub fn upgrade_request(target: &str) -> String {
    format!(
        "GET {target} HTTP/1.1\r\n\
         host: 127.0.0.1\r\n\
         upgrade: websocket\r\n\
         connection: keep-alive, Upgrade\r\n\
         sec-websocket-version: 13\r\n\
         sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
}
