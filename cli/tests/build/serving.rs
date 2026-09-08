//! `buri run` on a page: the server it starts, and what a reader gets back.
//!
//! A page is the one output `run` cannot hand to a process, so what it does
//! instead is build the artifact and serve it. That is a listener, which is why
//! this is a module of its own rather than steps in
//! `repositories/serving/a_page_is_served/CASE.textproto`: a manifest step runs
//! a command to completion, and this command runs until it is stopped. The
//! fixture is shared — the manifest pins the build and the refusals, and this
//! file spawns `buri run` against the same `repo/` and talks to it.
//!
//! Every row here holds to the rules `cli/tests/README.md` states about a test
//! with a socket in it:
//!
//!   * **the port is the server's own.** Every row asks for `--port=0` and
//!     reads the address off the line the command prints. A test that picks a
//!     port and hopes is racing the pick against the bind.
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
struct Serving {
    child: std::process::Child,
    port: u16,
    said: std::sync::mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        while self.said.try_recv().is_ok() {}
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
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
        .stderr(std::process::Stdio::inherit());
    let mut child = command.spawn().expect("buri starts");
    let stdout = child.stdout.take().expect("a piped stdout");
    let (sending, said) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if sending.send(line).is_err() {
                return;
            }
        }
    });

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
    Serving { child, port, said, reader: Some(reader) }
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
