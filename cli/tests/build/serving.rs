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
    complained: std::sync::mpsc::Receiver<String>,
    readers: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for Serving {
    fn drop(&mut self) {
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
    Serving { child, port, said, complained, readers: vec![reading_out, reading_err] }
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

/// The value of the first `name="..."` in a document: the address the browser
/// reads off the markup before it asks for anything.
fn attribute(html: &str, name: &str) -> String {
    let needle = format!("{name}=\"");
    let after = html
        .split_once(needle.as_str())
        .unwrap_or_else(|| panic!("no {name} in the shell:\n{}", indent(html)))
        .1;
    after
        .split_once('"')
        .unwrap_or_else(|| panic!("an unterminated {name} in the shell:\n{}", indent(html)))
        .0
        .to_string()
}

/// The address a browser asks for, having read `reference` in a document it
/// loaded from `base`.
///
/// The whole of the rule: a reference starting with `/` stands as it is, and
/// every other one hangs off the *directory* of the address the document came
/// from — not off the root. So a shell answered at `/a/b` that names
/// `./main.mjs` sends the browser to `/a/main.mjs`.
fn resolved(base: &str, reference: &str) -> String {
    if reference.starts_with('/') {
        return reference.to_string();
    }
    let relative = reference.strip_prefix("./").unwrap_or(reference);
    let directory = base.rsplit_once('/').map_or("/", |(head, _last)| head);
    format!("{directory}/{relative}")
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

/// **The shell a deep link answers asks for files that are there.**
///
/// A reader following a shared link arrives at `/components/button/states`,
/// and the browser resolves every address in the document it got against
/// *that* path rather than against the root. A shell naming `./main.mjs` sends
/// it to `/components/button/main.mjs`, which nothing wrote — so the module
/// never arrives, nothing mounts, and the page is blank with no error anywhere
/// a reader can see it. Deep links are the case the shell-for-every-path rule
/// exists for, so the shell has to name its two companions from the root.
///
/// The addresses are read off the markup rather than written out here, so this
/// asks the question a browser asks: whatever the shell says, is it there?
#[test]
fn a_deep_links_shell_asks_for_the_files_beside_it() {
    let scratch = page_repo("serving-deep");
    let server = serving(&scratch, "//cmd/site", &[]);

    let at = "/components/button/states";
    let shell = get(server.port, at).ok().body.clone();

    let module = resolved(at, &attribute(&shell, "src"));
    get(server.port, &module).ok().holds("the front page");

    let styles = resolved(at, &attribute(&shell, "href"));
    get(server.port, &styles).ok().holds("padding:1rem");
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
        .holds("<script type=\"module\" src=\"/main.mjs\"></script>");
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
