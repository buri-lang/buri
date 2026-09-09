//! `buri run` on something that keeps running: what it serves, and what
//! stopping it stops.
//!
//! A page is the one output `run` cannot hand to a process, so what it does
//! instead is build the artifact and serve it. That is a listener, which is why
//! this is a module of its own rather than steps in
//! `repositories/serving/a_page_is_served/CASE.textproto`: a manifest step runs
//! a command to completion, and this command runs until it is stopped. The
//! fixture is shared — the manifest pins the build and the refusals, and this
//! file spawns `buri run` against the same `repo/` and talks to it.
//!
//! The last section is the other half of "runs until it is stopped": a native
//! binary is a *child* of `buri run`, so stopping the command has to stop the
//! program too. Its rows own a socket for the same reason the page rows do — a
//! port that answers after the wrapper is gone is what an orphan looks like
//! from outside.
//!
//! Every row here holds to the rules `cli/tests/README.md` states about a test
//! with a socket in it:
//!
//!   * **the port is the server's own.** A page row asks for `--port=0` and a
//!     program row binds `port: 0`; either way the number is read off the line
//!     the process printed. A test that picks a port and hopes is racing the
//!     pick against the bind.
//!   * **every wait has a deadline, and what is waited for is what was read.**
//!     The announcement, the connect, the request and the reply each carry one,
//!     and the `--watch` row waits for the *rebuilt bytes* to appear rather
//!     than for a length of time somebody guessed at.
//!   * **the reply is read until it is whole.** The server answers
//!     `Connection: close`, so the end of the answer is the peer closing, and
//!     nothing here stops at a byte count the kernel happened to coalesce.
//!   * **a child that stopped is not waited for.** The announcement is waited
//!     on until the child's own standard output closes, so a `buri run` that
//!     refused the invocation fails the row with its status instead of sitting
//!     out the deadline.
//!   * **the child is always stopped.** [`Serving`] kills it on the way out of
//!     the row, including the way out a panic takes, so a failing assertion is
//!     a red test rather than a `buri` left listening.
use crate::harness::*;

use std::io::{BufRead as _, Read as _, Write as _};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

/// How long any one step may take: the announcement after a cold build, a
/// connect, a request, a reply, or a rebuild landing on disk.
///
/// One number rather than several, and a generous one, because none of these
/// is a measurement — every row asserts on what it read, and the deadline is
/// only there so that a server which never comes up is a failing assertion
/// instead of a job CI has to kill.
const DEADLINE: Duration = Duration::from_secs(180);

/// The fixture: the repository the manifest case builds, copied where this
/// process can edit it.
fn page_repo(name: &str) -> Scratch {
    Scratch::copy_of(name, &tests_dir().join("repositories/serving/a_page_is_served/repo"))
}

// ---------------------------------------------------------------------------
// A `buri run` that is listening
// ---------------------------------------------------------------------------

/// A running `buri run`, with the port it announced.
///
/// The child is killed in `Drop` rather than at the end of each row: a row that
/// fails an assertion leaves through a panic, and a panic that leaked a
/// listening `buri` would take the port with it.
///
/// **`own_group` is what makes that true of a `run` that started a program of
/// its own.** The program inherits these pipes, so a wrapper killed on its own
/// leaves a grandchild holding the writing ends and the reader threads below
/// never finish — a failing row would hang instead of reporting. `running` puts
/// the command in a process group of its own and `Drop` ends that group, which
/// reaches every process this row started and nothing else.
struct Serving {
    child: std::process::Child,
    port: u16,
    own_group: bool,
    said: std::sync::mpsc::Receiver<String>,
    complained: std::sync::mpsc::Receiver<String>,
    readers: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        if self.own_group {
            if let Ok(pid) = i32::try_from(self.child.id()) {
                // SAFETY: an ordinary `kill` on the process group `running`
                // made for this row.
                unsafe { kill(-pid, SIGKILL) };
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        while self.said.try_recv().is_ok() {}
        while self.complained.try_recv().is_ok() {}
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Serving {
    /// Waits for a line on the child's standard error holding `needle`.
    ///
    /// This is how a row waits for a *pass* rather than for a length of time.
    /// A rebuild that fails writes nothing, so there is no file whose bytes a
    /// row could poll for; what says the pass ran is the diagnostic it printed.
    /// Standard error closing means the process ended, which is a failing row
    /// here rather than a wait that runs out the deadline.
    fn complained_about(&self, needle: &str) {
        let mut seen: Vec<String> = Vec::new();
        let found = until(DEADLINE, || loop {
            match self.complained.try_recv() {
                Ok(line) => {
                    let hit = line.contains(needle);
                    seen.push(line);
                    if hit {
                        return Some(true);
                    }
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(false),
            }
        });
        match found {
            Some(true) => {}
            Some(false) => panic!(
                "the server stopped without saying {needle:?}; it said:\n{}",
                indent(&seen.join("\n"))
            ),
            None => panic!(
                "nothing on standard error held {needle:?} within {DEADLINE:?}; it said:\n{}",
                indent(&seen.join("\n"))
            ),
        }
    }
}

/// Reads a pipe a line at a time onto a channel.
///
/// Every line is echoed as it is read, because standard error used to be
/// inherited and a failing row is still owed what the child said about itself.
fn lines(
    pipe: impl std::io::Read + Send + 'static,
) -> (std::sync::mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
    let (sending, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(pipe).lines() {
            let Ok(line) = line else { return };
            eprintln!("{line}");
            if sending.send(line).is_err() {
                return;
            }
        }
    });
    (received, reader)
}

/// Spawns `buri run <target> --port=0 [extra]` in `scratch` and waits for the
/// line it prints when it is listening.
///
/// The line is the whole of the handshake, and it is the command's contract:
/// the address is printed once, on standard output, before anything blocks.
/// Under `--watch` it is printed after the declared set has been stamped, so a
/// row that edits a file the moment it has read this line is editing on the far
/// side of that stamp rather than racing it.
fn serving(scratch: &Scratch, target: &str, extra: &[&str]) -> Serving {
    let mut command = buri_command();
    command
        .current_dir(&scratch.root)
        .arg("run")
        .arg(target)
        .arg("--port=0")
        .args(extra)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("buri starts");
    let (said, reading_out) = lines(child.stdout.take().expect("a piped stdout"));
    let (complained, reading_err) = lines(child.stderr.take().expect("a piped stderr"));

    // Every line the child wrote reaches the channel before the sender is
    // dropped, so `Disconnected` means the process closed its output with
    // nothing left to say — which is what a refused invocation looks like from
    // here, and there is no point waiting out the deadline for it.
    let announced = until(DEADLINE, || match said.try_recv() {
        Ok(line) => Some(Some(line)),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(None),
    });
    let announced = match announced {
        Some(Some(line)) => line,
        Some(None) => {
            let status = child.wait().map(|s| s.to_string()).unwrap_or_default();
            panic!("`buri run {target}` stopped without announcing an address ({status})")
        }
        None => panic!("`buri run {target}` announced no address within {DEADLINE:?}"),
    };
    let port = port_in(&announced);
    Serving {
        child,
        port,
        own_group: false,
        said,
        complained,
        readers: vec![reading_out, reading_err],
    }
}

/// The port out of `serving //cmd/site on http://127.0.0.1:<port>/`.
///
/// Parsed rather than matched loosely, so a line that stopped carrying an
/// address fails here with the line in the message.
fn port_in(line: &str) -> u16 {
    let address = line
        .rsplit_once("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("no loopback address in the announcement: {line:?}"))
        .1;
    let digits: String = address.chars().take_while(char::is_ascii_digit).collect();
    let port: u16 =
        digits.parse().unwrap_or_else(|e| panic!("no port in the announcement {line:?}: {e}"));
    assert!(port > 0, "the announcement carries port 0, which is what was asked for: {line:?}");
    port
}

/// Polls `read` until it answers, or the deadline passes.
fn until<T>(within: Duration, mut read: impl FnMut() -> Option<T>) -> Option<T> {
    let stop = Instant::now() + within;
    loop {
        if let Some(found) = read() {
            return Some(found);
        }
        if Instant::now() >= stop {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// A client that is not a browser
// ---------------------------------------------------------------------------

/// What came back: the status line, the headers, and the body.
struct Reply {
    status: String,
    headers: String,
    body: String,
}

impl Reply {
    fn ok(&self) -> &Reply {
        assert!(self.status.starts_with("HTTP/1.1 200"), "not a 200: {}", self.status);
        self
    }

    fn missing(&self) -> &Reply {
        assert!(self.status.starts_with("HTTP/1.1 404"), "not a 404: {}", self.status);
        self
    }

    fn header(&self, name: &str) -> String {
        self.headers
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with(&name.to_ascii_lowercase()))
            .unwrap_or_else(|| panic!("no {name} header in:\n{}", indent(&self.headers)))
            .trim()
            .to_string()
    }

    fn holds(&self, needle: &str) -> &Reply {
        assert!(
            self.body.contains(needle),
            "the body does not hold {needle:?}:\n{}",
            indent(&self.body)
        );
        self
    }
}

/// One `GET`, read until the peer closes.
fn get(port: u16, path: &str) -> Reply {
    let stop = Instant::now() + DEADLINE;
    let mut socket = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(socket) => break socket,
            Err(e) => {
                assert!(Instant::now() < stop, "could not reach 127.0.0.1:{port}: {e}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    socket.set_read_timeout(Some(DEADLINE)).unwrap();
    socket.set_write_timeout(Some(DEADLINE)).unwrap();
    socket.write_all(format!("GET {path} HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
    socket.flush().unwrap();
    // To the close rather than to a length: the server says `Connection:
    // close`, so the end of the answer is the end of the stream.
    let mut raw = Vec::new();
    socket.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let (status, headers) = head.split_once("\r\n").unwrap_or((head, ""));
    Reply { status: status.to_string(), headers: headers.to_string(), body: body.to_string() }
}

// ---------------------------------------------------------------------------
// What a reader gets
// ---------------------------------------------------------------------------

/// The whole of what a page needs from a local server, in one process: the
/// shell for the address the reader typed, the files beside it as themselves,
/// and a 404 for an asset that is not there.
///
/// **`/` and `/components/button` answer the same shell.** That is the point of
/// the command rather than an implementation detail: the page routes on
/// `web.route(ctx)`, so a deep link has to arrive with the address the reader
/// typed still in the address bar. A server that answered `/main.html` would
/// hand the router `/main.html`, and a plain static one would 404 where there
/// is no file.
///
/// **`/main.css` is itself.** The shell links it by name, so a server that
/// answered the shell for *everything* would hand a browser HTML where it asked
/// for a stylesheet — which is why the rule is about what the path names rather
/// than about what happens to be missing.
#[test]
fn a_page_is_served_with_the_shell_for_every_route() {
    let scratch = page_repo("serving-routes");
    let server = serving(&scratch, "//cmd/site", &[]);

    let shell = scratch.read(".buri/out/web/cmd/site/main.html");
    let front = get(server.port, "/");
    front.ok();
    assert_eq!(front.body, shell, "`/` is not the shell on disk");
    assert_eq!(front.header("content-type"), "Content-Type: text/html; charset=utf-8");
    // A rebuild the browser cannot see is a rebuild that did not happen.
    assert_eq!(front.header("cache-control"), "Cache-Control: no-store");

    let deep = get(server.port, "/components/button");
    deep.ok();
    assert_eq!(deep.body, shell, "a deep link is not the shell");

    // A query string is not part of a path, which is what `Request.path` and
    // the address bar both say, so it is the same page.
    assert_eq!(get(server.port, "/components/button?tab=api").ok().body, shell);

    let styles = get(server.port, "/main.css");
    styles.ok().holds("padding:1rem");
    assert_eq!(styles.header("content-type"), "Content-Type: text/css; charset=utf-8");

    let module = get(server.port, "/main.mjs");
    module.ok().holds("the front page");
    assert_eq!(module.header("content-type"), "Content-Type: text/javascript; charset=utf-8");

    // An asset that is not there is missing, not a page. A browser told the
    // stylesheet it asked for is HTML has been lied to.
    get(server.port, "/theme.css").missing();

    // And a path climbing out of the artifact directory is refused rather than
    // normalised: nothing the compiler writes is reached through one.
    get(server.port, "/../../../etc/passwd").missing();
}

/// `--watch` rebuilds on an edit to a declared input, and the next request is
/// answered from what the rebuild wrote.
///
/// The edit is made after the address is announced, which is the boundary the
/// whole loop turns on: under `--watch` the command prints the address once the
/// declared set has been stamped, so an edit on the far side of that line wakes
/// the next pass rather than being absorbed by the stamp.
///
/// What it asserts is the *rebuilt bytes*, polled for until they arrive. The
/// shell is a fixed function of the artifact's name, so what moves when a
/// page's source moves is the stylesheet and the module — both are read back
/// here, along with the shell, which has to keep being answered across a
/// rebuild.
#[test]
fn a_watching_run_serves_what_the_rebuild_wrote() {
    let scratch = page_repo("serving-watch");
    let server = serving(&scratch, "//cmd/site", &["--watch"]);

    get(server.port, "/main.css").ok().holds("padding:1rem");
    get(server.port, "/main.mjs").ok().holds("the front page");

    // One write, not two: the loop coalesces a burst into one pass, and two
    // writes could also be two passes — the first of which would have rebuilt
    // half the edit, which is a race between what is asserted below.
    let source = scratch.read("cmd/site/main.buri");
    let edited = source.replace(".Rem(1.0)", ".Rem(2.0)").replace("the front page", "the rebuilt front page");
    assert_ne!(edited, source, "the edit changed nothing");
    scratch.write("cmd/site/main.buri", &edited);

    let rebuilt = until(DEADLINE, || {
        let styles = get(server.port, "/main.css");
        styles.body.contains("padding:2rem").then_some(())
    });
    assert!(rebuilt.is_some(), "the stylesheet was not rebuilt within {DEADLINE:?}");
    get(server.port, "/main.mjs").ok().holds("the rebuilt front page");
    // The same server: a route with no file behind it is still the shell.
    get(server.port, "/components/button")
        .ok()
        .holds("<script type=\"module\" src=\"./main.mjs\"></script>");
}

/// A rebuild that fails leaves the page that was working where it was, and the
/// loop goes on watching the file it could not build.
///
/// This is the signature failure beside
/// [`a_watching_run_serves_what_the_rebuild_wrote`], and it is the half a
/// reader notices: a page you can still reload is better than a blank one, so
/// a broken save may not take the last good build down with it. The two ways
/// to get that wrong are a command that exits — the reader's tab now refuses
/// the connection — and a server left answering out of a directory the failed
/// pass had emptied.
///
/// **The diagnostic is what says the pass ran.** A failed rebuild writes
/// nothing, so there are no new bytes to poll for; the row waits for the error
/// on standard error instead, and everything asserted after it is asserted
/// about a rebuild that has been tried and has failed rather than one that has
/// not started.
///
/// The repair at the end is the other half of "still watching": a loop that
/// dropped the broken file from its input set would never wake for the save
/// that fixes it, and the page would stay at the last good build for ever with
/// nothing said about it.
#[test]
fn a_watching_run_that_cannot_rebuild_keeps_serving_the_last_page() {
    let scratch = page_repo("serving-watch-broken");
    let server = serving(&scratch, "//cmd/site", &["--watch"]);

    let shell = get(server.port, "/").ok().body.clone();
    get(server.port, "/main.mjs").ok().holds("the front page");

    // A name that is not in scope: the file still parses, so the pass gets as
    // far as the checker and the failure is a diagnostic about the program
    // rather than a build file the loop could not read.
    let source = scratch.read("cmd/site/main.buri");
    let broken = source.replace("\"the front page\"", "theFrontPage");
    assert_ne!(broken, source, "the edit changed nothing");
    scratch.write("cmd/site/main.buri", &broken);
    server.complained_about("unresolved-name");

    get(server.port, "/main.mjs").ok().holds("the front page");
    assert_eq!(get(server.port, "/").ok().body, shell, "the shell the reader had is gone");

    let repaired = source.replace("the front page", "the repaired front page");
    assert_ne!(repaired, source, "the repair changed nothing");
    scratch.write("cmd/site/main.buri", &repaired);
    let served = until(DEADLINE, || {
        get(server.port, "/main.mjs").body.contains("the repaired front page").then_some(())
    });
    assert!(served.is_some(), "the repair was not served within {DEADLINE:?}");
}

/// A binary that declares a page *and* a worker serves the page.
///
/// A worker is called by its platform, once per request, so there is nothing
/// for `run` to start — and a page is the half a person can look at. This is
/// what the `run` row in `repositories/build-files/several_entries` used to
/// assert by exiting 0 with nothing to show for it; here the claim is the
/// answer that came back over a socket.
///
/// What it does not do is put the worker in front of the page. `buri run`
/// answers the shell the compiler wrote rather than the document `fetch`
/// renders, which is why `cmd/both`'s page mounts instead of resuming.
#[test]
fn a_binary_with_a_worker_beside_its_page_serves_the_page() {
    let scratch = page_repo("serving-both");
    let server = serving(&scratch, "//cmd/both", &[]);

    let shell = scratch.read(".buri/out/web/cmd/both/main.html");
    assert_eq!(get(server.port, "/").ok().body, shell, "`/` is not the page's shell");
    get(server.port, "/main.mjs").ok().holds("both halves, one tree");
}

/// A port something else is already on is a refusal naming it, rather than a
/// command that came up somewhere the reader was not told about.
#[test]
fn a_port_already_taken_is_refused_by_name() {
    let scratch = page_repo("serving-taken");
    let held = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port");
    let port = held.local_addr().expect("a bound address").port();

    let run = scratch.run(&["run", "//cmd/site", &format!("--port={port}")]);
    run.exits(2).says(&format!("cannot listen on 127.0.0.1:{port}")).says("--port=0");
    drop(held);
}

// ---------------------------------------------------------------------------
// Stopping what `buri run` started
// ---------------------------------------------------------------------------

/// `SIGINT` — a person at a terminal — and `SIGTERM` — a supervisor, a
/// container runtime, an `init`. Two and fifteen on both platforms this suite
/// runs on; `cli/runtime/net.rs`'s `shutdown` module writes the same two
/// numbers on the other side of the C ABI.
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
/// `SIGKILL`, which is how [`Serving`] ends a row's process group whatever
/// state the row left it in.
const SIGKILL: i32 = 9;

// The two calls this file makes into the C library, declared rather than
// depended on — `cli/tests/native/shared.rs`'s `kill` block is the precedent,
// and the argument is the same one: a dependency for a declaration is a
// dependency.
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn setpgid(pid: i32, pgid: i32) -> i32;
}

/// A server that cannot stop on its own, so the only thing that can end it is a
/// signal.
///
/// No `requestLimit` and no `idleTimeout`: the two fields that let
/// `native::shared`'s fixtures finish are deliberately absent, so a `main` that
/// returned `.Ok(())` can only have come from the drain a signal started. The
/// port is printed before `run` blocks, which is the runtime's promise rather
/// than this fixture's trick.
const SERVER: &str = r#"from "core/effect" import { Allocator, Listen, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/time" import * as time;

export fn main(): Result<(), Str> {
  let ctx = context {
    Allocator: host.alloc,
    Listen: host.listen,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };
  let plan = server.Server {
    port: 0,
    onRequest: fn(c, request) => http.text(c, request.path()),
    drain: .Some(time.milliseconds(10000)),
  };
  match (server.bind(ctx, plan)) {
    .Err(e) => .Err(server.errorText(e)),
    .Ok(listener) => {
      let _ = io.println(ctx, "port ${listener.port}").ignore();
      match (server.run(ctx, listener, plan)) {
        .Err(e) => .Err(server.errorText(e)),
        .Ok(_ok) => .Ok(()),
      }
    },
  }
}
"#;

/// A program that catches nothing, so a signal it is sent ends it the operating
/// system's way. It opens no port, and `up` is how a row knows it is running.
const SLEEPER: &str = r#"from "core/effect" import { Clock, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/time" import * as time;

fn forever<C: Clock>(ctx: C): () {
    let _ = time.sleep(ctx, time.seconds(1));
    forever(ctx)
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Clock: host.clock,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "up").ignore();
    let _ = forever(ctx);
    .Ok(())
}
"#;

/// The platform a binary here declares, named rather than left to the default:
/// a binary that declares no output builds for JavaScript, and a JavaScript
/// artifact is a module `bun` runs rather than a process `run` starts.
fn native_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "MACOS"
    } else {
        "LINUX"
    }
}

/// A one-package repository holding `source`, built once so that the rows below
/// spawn a command that only has to start.
///
/// `None` is a host with no native backend — no C toolchain, or a triple no
/// stencil library is built for. That is a host's answer and not a runner's:
/// `ci::skipped` prints it here and panics under `BURI_CI=1`, where
/// `cli/tests/ci.rs` has already asserted the backend's inputs are real bytes.
fn native_repo(name: &str, source: &str) -> Option<Scratch> {
    let scratch = Scratch::repo(name);
    scratch.write(
        "cmd/program/BUILD.buri",
        &format!("binary {{\n    outputs: [{{ platform: {} }}]\n}}\n", native_platform()),
    );
    scratch.write("cmd/program/main.buri", source);
    let built = scratch.run(&["build", "//cmd/program"]);
    if built.code != 0 {
        ci::skipped(
            "build::serving",
            &format!(
                "`buri build //cmd/program` could not produce a native artifact:\n{}",
                indent(&built.all())
            ),
        );
        return None;
    }
    Some(scratch)
}

/// Where a row aims its signal: at `buri run` itself, or at the whole process
/// group it was started in — which is what a terminal does when somebody
/// presses Ctrl-C.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Aim {
    AtTheWrapper,
    AtTheGroup,
}

/// Spawns `buri run //cmd/program` in `scratch` and waits for the first line
/// the program prints, which is how a row knows it is up.
///
/// The command gets a **process group of its own**, always: it is what lets a
/// row aim at a group holding nothing but processes this test started, and it
/// is what lets [`Serving`] clean up after a row that failed.
fn running(scratch: &Scratch) -> (Serving, String) {
    use std::os::unix::process::CommandExt as _;

    let mut command = buri_command();
    command
        .current_dir(&scratch.root)
        .arg("run")
        .arg("//cmd/program")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // SAFETY: `setpgid` is on POSIX's async-signal-safe list, which is the
    // whole of what a `pre_exec` closure may call.
    unsafe {
        command.pre_exec(|| {
            setpgid(0, 0);
            Ok(())
        })
    };
    let mut child = command.spawn().expect("buri starts");
    let (said, reading_out) = lines(child.stdout.take().expect("a piped stdout"));
    let (complained, reading_err) = lines(child.stderr.take().expect("a piped stderr"));

    let first = until(DEADLINE, || match said.try_recv() {
        Ok(line) => Some(Some(line)),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(None),
    });
    let first = match first {
        Some(Some(line)) => line,
        Some(None) => {
            let status = child.wait().map(|s| s.to_string()).unwrap_or_default();
            panic!("`buri run //cmd/program` stopped without starting the program ({status})")
        }
        None => panic!("`buri run //cmd/program` said nothing within {DEADLINE:?}"),
    };
    let serving = Serving {
        child,
        port: 0,
        own_group: true,
        said,
        complained,
        readers: vec![reading_out, reading_err],
    };
    (serving, first)
}

/// The port out of the program's own `port <n>` line.
fn announced_port(line: &str) -> u16 {
    line.strip_prefix("port ")
        .and_then(|digits| digits.trim().parse().ok())
        .unwrap_or_else(|| panic!("no port in the program's first line: {line:?}"))
}

/// Sends one signal to a process this test started, and to nothing else.
///
/// A negative pid is the process group of that number, which is how a terminal
/// signals a job — and the group is one [`running`] made, so it holds this
/// row's `buri run` and its child and nothing besides.
fn signal(server: &Serving, aim: Aim, sig: i32) {
    let pid = i32::try_from(server.child.id()).expect("a process id");
    let target = if aim == Aim::AtTheGroup { -pid } else { pid };
    // SAFETY: an ordinary `kill` on this row's own child, or on the process
    // group `running` put it in.
    let sent = unsafe { kill(target, sig) };
    assert_eq!(sent, 0, "could not signal {target} with {sig}");
}

/// Waits for `buri run` to stop, and answers the status it stopped with.
///
/// Bounded and killed on the way out, for the reason every wait in this file
/// is: a wrapper that will not stop is a failing row with a sentence rather
/// than a job CI has to kill.
fn stopped(server: &mut Serving) -> std::process::ExitStatus {
    let status = until(DEADLINE, || server.child.try_wait().ok().flatten());
    status.unwrap_or_else(|| {
        let _ = server.child.kill();
        panic!("`buri run` was still running {DEADLINE:?} after the signal")
    })
}

/// Nothing is listening there any more.
///
/// Asked once, after `buri run` has been reaped, and that ordering is the
/// assertion: a wrapper that waits for its child before exiting cannot leave a
/// bound port behind it.
fn refused(port: u16) {
    if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        panic!(
            "127.0.0.1:{port} still answers after `buri run` exited, so the program it started \
             is still running and still holding the port"
        );
    }
}

/// **Stopping `buri run` stops the program it started**, and stops it the way a
/// server wants to be stopped.
///
/// The wrapper executes a native binary as a child, so a `SIGTERM` that reaches
/// only the wrapper leaves a server reparented to `init`, still bound to its
/// port and never told to drain (buri-lang/buri#91). What this row reads is
/// exactly that from outside: the command is reaped, and *then* the port is
/// dialled — a connection that succeeds is an orphan.
///
/// The status is the other half. A drained server returns `.Ok(())`, so `buri
/// run` has a child that exited 0 to report, and reporting anything else would
/// make a clean stop look like a failure to every script that reads it.
#[test]
fn a_signalled_run_stops_the_server_it_started() {
    let Some(scratch) = native_repo("run-signal", SERVER) else { return };
    let (mut server, first) = running(&scratch);
    let port = announced_port(&first);

    signal(&server, Aim::AtTheWrapper, SIGTERM);
    let status = stopped(&mut server);
    assert_eq!(
        status.code(),
        Some(0),
        "`buri run` did not answer with the status its child drained with: {status}"
    );
    refused(port);
}

/// The signature failure beside it: a signal aimed at the whole process group
/// reaches the program **once**.
///
/// This is Ctrl-C. A terminal sends the signal to every process in the
/// foreground group, so a wrapper that also forwarded it would deliver a second
/// one — and the second is the operating system's, because the runtime restores
/// the default disposition before it drains (`design/native/DECISIONS.md`). A
/// server would then be killed by the keystroke that was meant to start its
/// drain, which is the thing this row exists to catch: `.Ok(())` and exit 0 can
/// only come from a drain that ran.
#[test]
fn a_signal_to_the_whole_group_reaches_the_program_once() {
    let Some(scratch) = native_repo("run-signal-group", SERVER) else { return };
    let (mut server, first) = running(&scratch);
    let port = announced_port(&first);

    signal(&server, Aim::AtTheGroup, SIGINT);
    let status = stopped(&mut server);
    assert_eq!(
        status.code(),
        Some(0),
        "the program was killed rather than drained, so it was signalled twice: {status}"
    );
    refused(port);
}

/// A program that catches nothing is ended by the signal, and `buri run` says
/// so the way a shell does: 128 plus the number.
///
/// `ExitStatus::code` is `None` for a process a signal killed, so a wrapper
/// that passed the code straight through would report the same status for a
/// program that was killed and one that chose to fail.
#[test]
fn a_program_a_signal_ends_is_reported_as_the_signal() {
    let Some(scratch) = native_repo("run-signal-uncaught", SLEEPER) else { return };
    let (mut server, first) = running(&scratch);
    assert_eq!(first, "up", "the program did not start");

    signal(&server, Aim::AtTheWrapper, SIGTERM);
    let status = stopped(&mut server);
    assert_eq!(
        status.code(),
        Some(128 + SIGTERM),
        "`buri run` did not report the signal that ended its child: {status}"
    );
}

/// And the page server gives its port back on the same signal.
///
/// It has no child and nothing to drain, so stopping is all there is to it —
/// but a port left bound is the same complaint whichever half of `run` was
/// holding it, and this is the row that says so.
#[test]
fn a_signalled_page_server_gives_up_its_port() {
    let scratch = page_repo("serving-signal");
    let mut server = serving(&scratch, "//cmd/site", &[]);
    let port = server.port;
    get(port, "/").ok();

    signal(&server, Aim::AtTheWrapper, SIGTERM);
    let _ = stopped(&mut server);
    refused(port);
}
