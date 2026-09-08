//! The local server `buri run` starts for a page.
//!
//! A hundred lines of `std::net`, because HTTP for one reader on localhost is a
//! request line, a few headers and a file. It listens on `127.0.0.1` only: this
//! is a page you are working on, not a web server, and it has no business being
//! reachable from the network the machine is on.
//!
//! **The rule it exists for.** A page routes on `web.route(ctx)` — the address
//! bar — so a reader who asks for `/components/button` has to arrive at the
//! page with `/components/button` still in the address bar. A static server
//! answering `/main.html` hands the router `/main.html` and the page renders
//! its not-found route; a plain one answers 404, because there is no file
//! there. So: **a path that names a file is that file, and every other path is
//! the shell.** What separates the two is the last segment having an extension,
//! rather than whether the file happens to exist — a browser told that the
//! stylesheet it asked for is HTML has been lied to, and the mistake shows up
//! as a page with no styles rather than as a 404 anybody can read.
//!
//! `website/src/serve.rs` is the same shape for the documentation site, and the
//! two are not shared: `website` is the site generator, it reads `cli/src/docs`,
//! and the toolchain does not depend on it. What is duplicated is sixty lines
//! of `std::net`; what would be shared is a dependency edge the wrong way round.
#![allow(
    clippy::print_stdout,
    reason = "the address a reader is meant to open is this command's own output; \
              every diagnostic still leaves through `Session::emit`"
)]

use std::io::{BufRead as _, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Where a page listens when nobody said.
///
/// Stable, so the tab you left open is still the tab: a port the toolchain
/// picked afresh each run would mean a new address every time. It is not 3000,
/// which is what `core/net/server`'s own guide binds, so a page and the server
/// it talks to can both be up.
pub const DEFAULT_PORT: u16 = 4000;

/// The artifact directory a page is answered out of, and the shell within it.
#[derive(Debug)]
pub struct Page {
    /// Everything under here is answered as itself. It is the directory the
    /// build wrote the module into, so the stylesheet, the chunks and anything
    /// else beside it are reachable by the names the shell uses.
    dir: PathBuf,
    /// The file every path that names no file is answered with.
    shell: PathBuf,
    /// Held while a rebuild is writing and while a response is reading, so a
    /// reader is never handed half a file.
    building: Mutex<()>,
}

impl Page {
    /// The page beside a built module, or the reason there is none.
    ///
    /// A build that succeeded has written the shell, so the failure here is not
    /// a build failure — it is the artifact directory having been emptied under
    /// the command. Serving anyway would answer every route with nothing at
    /// all, which reads as a broken page rather than as a missing file.
    pub fn beside(module: &Path) -> Result<Page, String> {
        let shell = shell_beside(module);
        if !shell.is_file() {
            return Err(format!("{} is not there", shell.display()));
        }
        let dir = module.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(Page { dir, shell, building: Mutex::new(()) })
    }

    /// Held for as long as a rebuild is writing into the artifact directory.
    pub fn building(&self) -> MutexGuard<'_, ()> {
        // A panicking responder cannot leave a page unservable: what the lock
        // guards is a moment in time rather than an invariant over data.
        self.building.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The file a request path is answered with, or `None` for the one shape
    /// this server has nothing for: a path that names a file that is not there.
    fn answer(&self, path: &str) -> Option<PathBuf> {
        let mut file = self.dir.clone();
        let decoded = percent_decoded(path);
        let mut last = "";
        for segment in decoded.split('/') {
            if segment.is_empty() || segment == "." {
                continue;
            }
            // Refused rather than normalised: no name the compiler writes is
            // reached through one, so a request carrying one is not a reader.
            if segment == ".." || segment.contains('\\') {
                return None;
            }
            file.push(segment);
            last = segment;
        }
        if file.is_file() {
            return Some(file);
        }
        // A last segment with an extension asked for a file; anything else
        // asked for a route, and every route is the page.
        if last.contains('.') {
            return None;
        }
        Some(self.shell.clone())
    }
}

/// Where the entry shell sits: beside the module, under the same name.
///
/// `build/actions.rs`'s `web_companions` writes it there, and this is the same
/// convention read from the other end — one agreement, and the file name is the
/// whole of it.
pub fn shell_beside(module: &Path) -> PathBuf {
    module.with_extension("html")
}

/// Binds loopback, or says why it could not.
pub fn bind(port: u16) -> Result<TcpListener, String> {
    TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))
}

/// Prints the address, once, before anything blocks.
///
/// It is the whole of the handshake: a reader opens it, and
/// `cli/tests/build/serving.rs` reads the port off it. `println!` writes through
/// a `LineWriter`, so the newline is the flush whether standard output is a
/// terminal or a pipe.
pub fn announce(label: &str, port: u16) {
    println!("serving {label} on http://127.0.0.1:{port}/");
    let _ = std::io::stdout().flush();
}

/// Answers requests until the process is stopped.
///
/// A connection at a time on a thread of its own, and `Connection: close` on
/// every answer: a browser opens several at once, and a page that is being
/// worked on is worth no more machinery than that.
pub fn serve(listener: &TcpListener, page: &Arc<Page>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let page = Arc::clone(page);
        std::thread::spawn(move || {
            let _ = respond(&stream, &page);
        });
    }
}

fn respond(stream: &TcpStream, page: &Page) -> std::io::Result<()> {
    let mut reader = std::io::BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // The headers are read and discarded: nothing here varies by one, and a
    // connection whose request is left unread is one a browser reports as
    // reset.
    let mut header = String::new();
    loop {
        header.clear();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
    }

    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return send(stream, "405 Method Not Allowed", "text/plain; charset=utf-8", b"", true);
    }
    // A query string and a fragment are not part of a path, which is what
    // `Request.path` and the address bar both say — so `/notes?page=2` is the
    // same page as `/notes`.
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let Some(file) = page.answer(path) else {
        return send(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found\n",
            method == "HEAD",
        );
    };
    let body = {
        let _guard = page.building();
        let mut body = Vec::new();
        std::fs::File::open(&file)?.read_to_end(&mut body)?;
        body
    };
    send(stream, "200 OK", content_type(&file), &body, method == "HEAD")
}

/// What the browser is told a file is.
///
/// Seven types and a fallback, which is what a page built by this toolchain can
/// hold: the three the compiler writes, and the four a `data` directory brings
/// with it. A type nothing here can name would be a guess.
fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("mjs" | "js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn send(
    mut stream: &TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    // `no-store` because the whole point of the command is that a rebuild is
    // what the next reload shows. A cached answer would make `--watch` a lie.
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn percent_decoded(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    while let Some(byte) = bytes.get(at).copied() {
        if byte == b'%' {
            let digits = path.get(at.saturating_add(1)..at.saturating_add(3));
            if let Some(value) = digits.and_then(|d| u8::from_str_radix(d, 16).ok()) {
                out.push(value);
                at = at.saturating_add(3);
                continue;
            }
        }
        out.push(byte);
        at = at.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory holding a shell, a module and a stylesheet — the three files
    /// a WEB output is.
    fn artifact(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buri-serve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("main.html"), "<!doctype html>\n");
        let _ = std::fs::write(dir.join("main.mjs"), "export {};\n");
        let _ = std::fs::write(dir.join("main.css"), ".p-r1{padding:1rem}\n");
        dir
    }

    /// The rule the command exists for: a file is itself, and everything else
    /// is the shell, so the page's own router sees the address that was typed.
    #[test]
    fn a_file_is_itself_and_every_other_path_is_the_shell() {
        let dir = artifact("routes");
        let page = Page::beside(&dir.join("main.mjs")).expect("a page");

        assert_eq!(page.answer("/"), Some(dir.join("main.html")));
        assert_eq!(page.answer("/components/button"), Some(dir.join("main.html")));
        assert_eq!(page.answer("/main.css"), Some(dir.join("main.css")));
        assert_eq!(page.answer("/main.mjs"), Some(dir.join("main.mjs")));
        // The shell by its own name is the shell, which is what an address bar
        // holding `/main.html` after a reload has to keep working.
        assert_eq!(page.answer("/main.html"), Some(dir.join("main.html")));

        // A path that named a file this page does not have is missing. Handing
        // back the shell here is what turns a mistyped asset into a page with
        // no styles and nothing said about it.
        assert_eq!(page.answer("/theme.css"), None);
        assert_eq!(page.answer("/assets/logo.png"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A path climbing out of the artifact directory is refused rather than
    /// normalised, in both spellings.
    #[test]
    fn a_path_climbing_out_of_the_artifact_directory_is_refused() {
        let dir = artifact("climbing");
        let page = Page::beside(&dir.join("main.mjs")).expect("a page");
        assert_eq!(page.answer("/../../etc/passwd"), None);
        assert_eq!(page.answer("/%2e%2e/etc/passwd"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A page with no shell beside it is a refusal naming the file, rather than
    /// a server that answers every route with nothing.
    #[test]
    fn a_module_with_no_shell_beside_it_is_not_a_page() {
        let dir = artifact("no-shell");
        let refusal = Page::beside(&dir.join("gone.mjs")).expect_err("no shell");
        assert!(refusal.contains("gone.html"), "the refusal does not name the shell: {refusal}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the browser is told each of a page's three files is, and what it is
    /// told about a name this table has nothing for.
    #[test]
    fn every_file_a_page_is_made_of_has_a_type() {
        assert_eq!(content_type(Path::new("main.html")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("main.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new("main.mjs")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("main.0.js")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("data.json")), "application/json");
        assert_eq!(content_type(Path::new("logo.png")), "image/png");
        assert_eq!(content_type(Path::new("logo.svg")), "image/svg+xml");
        assert_eq!(content_type(Path::new("text.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("LICENSE")), "application/octet-stream");
    }

    #[test]
    fn a_percent_escape_is_decoded_and_a_stray_one_is_kept() {
        assert_eq!(percent_decoded("/a%20b"), "/a b");
        assert_eq!(percent_decoded("/100%"), "/100%");
    }

    /// The shell is the module's own name with the extension swapped, which is
    /// the one agreement between what the build writes and what this reads.
    #[test]
    fn the_shell_sits_beside_the_module_under_the_same_name() {
        assert_eq!(
            shell_beside(Path::new("/repo/.buri/out/web/apps/design/main.mjs")),
            PathBuf::from("/repo/.buri/out/web/apps/design/main.html")
        );
    }
}
