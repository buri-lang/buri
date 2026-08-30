//! A minimal HTTP/1.1 client, for `Net::fetch`.
//!
//! Synchronous by necessity — Buri has no `async` in v0.3, so a request blocks,
//! exactly as `$host_HostNet_fetch` blocks on JavaScript (`runtime.js:1392-1404`,
//! which uses a synchronous `XMLHttpRequest` and says the same thing).
//!
//! ## Scope, stated honestly
//!
//! **`http://` only.** `https://` returns `NetError::Transport` with a message
//! saying so, rather than pretending. TLS is not something this repository can
//! reasonably write — the dependency bar in the workspace manifest exists for
//! exactly this shape of problem — and it is not something the `Net` effect can
//! be given half of: a client that silently downgraded to cleartext would be
//! worse than one that refuses.
//!
//! The growth path this header named — a cargo feature over `rustls` — **has
//! landed halfway**, and the half that landed is the dependency and not the
//! code. `rustls` is in `manifest.toml` behind the `net` feature, which is on
//! by default and which also carries `tokio`, `hyper` and `tungstenite`;
//! nothing references any of them (`net.rs`), the archive is twenty-four bytes
//! larger for it, and this file is unchanged. So the refusal above is still the
//! whole of what `https://` does, and it is now a refusal for want of *code*
//! rather than for want of a crate. Routing the client through hyper and rustls
//! — and choosing the crypto provider `manifest.toml` deliberately did not —
//! is the slice that deletes this section.
//!
//! What *is* here is complete for cleartext: absolute-URI parsing, `Host`,
//! request bodies with `Content-Length`, chunked and identity response bodies,
//! and a connect/read deadline.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// `NetError`'s variants, in declaration order in `effect.buri:91-96`. The index
/// is what crosses the ABI, so this order is the contract.
pub enum NetFail {
    Timeout,
    Refused,
    BadUrl(String),
    Transport(String),
}

impl NetFail {
    pub fn tag(&self) -> i32 {
        match self {
            NetFail::Timeout => 0,
            NetFail::Refused => 1,
            NetFail::BadUrl(_) => 2,
            NetFail::Transport(_) => 3,
        }
    }

    /// The payload of the two variants that carry one; empty for the two that
    /// do not, which is what the backend writes into an unused `Str` slot.
    pub fn message(&self) -> &str {
        match self {
            NetFail::Timeout | NetFail::Refused => "",
            NetFail::BadUrl(m) | NetFail::Transport(m) => m,
        }
    }
}

const DEADLINE: Duration = Duration::from_secs(30);

struct Url<'a> {
    authority: &'a str,
    host: &'a str,
    port: u16,
    target: &'a str,
}

fn parse(url: &str) -> Result<Url<'_>, NetFail> {
    let rest = match url.strip_prefix("http://") {
        Some(r) => r,
        None if url.starts_with("https://") => {
            return Err(NetFail::Transport(
                // It named a `net-tls` feature to build with until the crates
                // landed and the feature turned out to be called `net`, to be
                // on by default, and to not help: what is missing is the TLS
                // client, not the crate. A message naming a flag that changes
                // nothing is worse than one that says what is true.
                "https is not supported by the native runtime (its TLS client is not written \
                 yet; `Net.fetch` speaks cleartext http only)"
                    .to_string(),
            ))
        }
        None => return Err(NetFail::BadUrl(format!("not an absolute http URL: {url}"))),
    };
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
        None => (authority, 80),
    };
    Ok(Url { authority, host, port, target: if target.is_empty() { "/" } else { target } })
}

fn io_fail(e: &std::io::Error) -> NetFail {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => NetFail::Refused,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetFail::Timeout,
        _ => NetFail::Transport(e.to_string()),
    }
}

/// One request, one response. Returns `(status, body)`.
pub fn fetch(method: &str, url: &str, body: &str) -> Result<(i64, String), NetFail> {
    let url = parse(url)?;

    let mut addrs = (url.host, url.port)
        .to_socket_addrs()
        .map_err(|e| NetFail::Transport(format!("could not resolve {}: {e}", url.host)))?;
    let addr = addrs
        .next()
        .ok_or_else(|| NetFail::Transport(format!("no address for {}", url.host)))?;

    let mut sock = TcpStream::connect_timeout(&addr, DEADLINE).map_err(|e| io_fail(&e))?;
    sock.set_read_timeout(Some(DEADLINE)).map_err(|e| io_fail(&e))?;
    sock.set_write_timeout(Some(DEADLINE)).map_err(|e| io_fail(&e))?;
    // A request/response exchange is one write and one read, so Nagle can only
    // add a round trip's worth of delay to the front of it.
    let _ = sock.set_nodelay(true);

    let mut request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: buri\r\nAccept: */*\r\nConnection: close\r\n",
        method, url.target, url.authority
    );
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    sock.write_all(request.as_bytes()).map_err(|e| io_fail(&e))?;
    sock.flush().map_err(|e| io_fail(&e))?;

    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).map_err(|e| io_fail(&e))?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<(i64, String), NetFail> {
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
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }

    let bytes = if chunked {
        dechunk(body)?
    } else {
        match length {
            Some(n) => body.get(..n.min(body.len())).unwrap_or(body).to_vec(),
            None => body.to_vec(),
        }
    };
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
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
