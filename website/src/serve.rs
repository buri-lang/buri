//! The preview server: `cargo run -p website -- serve`.
//!
//! A hundred lines of `std::net`, because HTTP for one reader on localhost is
//! a request line, a few headers, and a file. It listens on 127.0.0.1 only —
//! this is a preview of a static site, not a web server, and it should not be
//! reachable from the network the machine is on.
//!
//! `--watch` polls the modification times of the files the site was generated
//! from, every half second, and rebuilds when one of them moves. Polling
//! rather than watching the filesystem for the reason the build system gives
//! for the same choice (`design/native/BUILD-AND-WATCH.md` §1.2): the declared
//! source set is known, it is a few hundred files, and a watcher is a
//! dependency.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "what the server says about itself is this command's output"
)]

use crate::pages;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// How often `--watch` asks the filesystem whether anything moved.
const POLL: Duration = Duration::from_millis(500);

/// Serves `out` until the process is interrupted.
pub fn serve(root: &Path, out: &Path, port: u16, watch: bool) -> i32 {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("cannot listen on 127.0.0.1:{port}: {why}");
            return 1;
        }
    };
    // Held while a rebuild is writing and while a response is reading, so a
    // reader never gets half a file.
    let building = Arc::new(Mutex::new(()));
    println!("serving {} on http://127.0.0.1:{port}/", out.display());
    if watch {
        println!("watching the documentation; edit a page and reload");
        spawn_watcher(root.to_path_buf(), out.to_path_buf(), Arc::clone(&building));
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let out = out.to_path_buf();
        let building = Arc::clone(&building);
        std::thread::spawn(move || {
            let _ = respond(stream, &out, &building);
        });
    }
    0
}

fn spawn_watcher(root: PathBuf, out: PathBuf, building: Arc<Mutex<()>>) {
    std::thread::spawn(move || {
        let mut stamps = Vec::new();
        loop {
            std::thread::sleep(POLL);
            let Ok(site) = pages::read(&root) else { continue };
            let now = modification_times(&site.sources());
            if stamps.is_empty() {
                stamps = now;
                continue;
            }
            if now == stamps {
                continue;
            }
            stamps = now;
            let Ok(guard) = building.lock() else { return };
            match crate::build(&site, &out) {
                Ok(built) => println!("rebuilt {} pages", built.len()),
                Err(why) => eprintln!("rebuild failed: {why}"),
            }
            drop(guard);
        }
    });
}

fn modification_times(paths: &[PathBuf]) -> Vec<SystemTime> {
    paths
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .and_then(|data| data.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
        .collect()
}

fn respond(stream: TcpStream, out: &Path, building: &Mutex<()>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // The headers are read and discarded: nothing here varies by them, and a
    // connection whose body is left unread is one the browser reports as
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
        return send(stream, "405 Method Not Allowed", "text/plain; charset=utf-8", b"", false);
    }

    let path = target.split(['?', '#']).next().unwrap_or("/");
    let Some(file) = resolve(out, path) else {
        return send(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found\n",
            method == "HEAD",
        );
    };
    let body = {
        let _guard = building.lock().map_err(|_| std::io::Error::other("the builder panicked"))?;
        let mut body = Vec::new();
        std::fs::File::open(&file)?.read_to_end(&mut body)?;
        body
    };
    send(stream, "200 OK", content_type(&file), &body, method == "HEAD")
}

/// The file a request path names, or `None` when it names nothing this server
/// will hand out. A path with a `..` in it is refused rather than normalized:
/// the site never writes one, so a request carrying one is not a reader.
fn resolve(out: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_decoded(path);
    let mut file = out.to_path_buf();
    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\\') {
            return None;
        }
        file.push(segment);
    }
    if file.is_dir() {
        file.push("index.html");
    }
    file.is_file().then_some(file)
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

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn send(
    mut stream: TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_path_climbing_out_of_the_output_is_refused() {
        let out = Path::new("/nowhere");
        assert!(resolve(out, "/../../etc/passwd").is_none());
        assert!(resolve(out, "/%2e%2e/etc/passwd").is_none());
    }

    #[test]
    fn a_percent_escape_is_decoded_and_a_stray_one_is_kept() {
        assert_eq!(percent_decoded("/a%20b"), "/a b");
        assert_eq!(percent_decoded("/100%"), "/100%");
    }

    /// Every file the generator writes today is a page — the stylesheet is
    /// inside each of them — so the rest of the table is what the server would
    /// say about an asset the site does not yet have.
    #[test]
    fn a_page_is_served_as_html() {
        assert_eq!(content_type(Path::new("a/index.html")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("assets/anything.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new(".website-generated")), "application/octet-stream");
    }
}
