//! TLS for `Net.fetch`, and the trust decision that comes with it.
//!
//! `http.rs` owns HTTP. This file owns exactly one question — *may this
//! connection be trusted* — and answers it with `rustls` over the `ring`
//! provider (`manifest.toml` argues both, and the price of the second one).
//! What `http.rs` gets back is a [`TlsStream`], which is a `Read + Write` over
//! the same `TcpStream` it opened, so the request writer, the response parser
//! and the dechunker are the *same code* on both schemes. That is not tidiness:
//! it is what makes "`http://` behaves exactly as it did" a property of the
//! design rather than a claim in a commit message.
//!
//! ## Where the trust anchors come from, and why not a crate
//!
//! **The host's own PEM bundle, and `SSL_CERT_FILE` overrides it.** There were
//! two candidates and the dependency bar in the root `Cargo.toml` decided
//! between them:
//!
//! * **`webpki-roots`** — Mozilla's root program, compiled in as data. It is a
//!   quarter of a megabyte in every binary this compiler produces, it pins the
//!   trust set to a crate version rather than to the machine, and — the part
//!   that matters — it is *data this repository would be shipping*, not a
//!   platform interface it could not reasonably write. A user who has added a
//!   corporate root to their machine has told their machine something, and a
//!   program compiled by this toolchain would not have heard it.
//! * **`rustls-native-certs`** — the platform stores, through
//!   `security-framework` on macOS and `schannel` on Windows. It reads the
//!   right thing, and it costs three transitive crates *and* a link-time
//!   dependency on `Security.framework`, which the archive cannot carry: this
//!   is a `staticlib` handed to `cc` by `build/link.rs`, so a framework it
//!   needs is a flag every artifact's link line would have to grow.
//!
//! What is left is [`BUNDLES`]: the file the platform already keeps, read and
//! parsed here in about forty lines. That clears the bar's first clause the
//! only way it can be cleared — by not being a dependency — and it is honest
//! about what it does not do, which is read the macOS keychain. A machine whose
//! roots are only in the keychain is exactly the machine [`CERT_FILE_ENV`] is
//! for, and it is the same variable `curl`, `git` and OpenSSL already honour,
//! so it is a spelling a user has met before.
//!
//! **Read on every fetch, not cached.** A `OnceLock` would save perhaps a
//! millisecond against a handshake that costs tens, and it would make the
//! answer to "which roots" a function of *when in the process* the first
//! request happened. Trust is not a thing to memoise by accident.
//!
//! ## The refusals
//!
//! Every *trust* failure below is a `NetError::Transport` carrying a sentence,
//! and each sentence names the trust source it used, because "this certificate
//! was refused" without "…and here is the set it was checked against" is the
//! failure a user cannot act on. The one failure that is not a sentence is a
//! handshake that ran out of time, which is a `NetError::Timeout`: it is the
//! same event a stalled read is, and it is the reason [`connect`] will not take
//! a socket without a deadline on it.
//!
//! `tests` below pins the refusals — an unknown issuer, a name the certificate
//! does not cover, a bundle with nothing in it, and a bundle that is not there
//! — against a server this file starts, with a certificate this repository
//! generated. Nothing in that test reaches the network, resolves a name off
//! this machine, or can wait forever for anything.

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::http::NetFail;

/// A TLS connection, owned end to end: `rustls` state and the socket under it.
///
/// `StreamOwned` rather than `rustls::Stream` because `http.rs` keeps the
/// connection in a local and hands out `&mut dyn Read`/`&mut dyn Write`; a
/// borrowed pair would put two lifetimes into `Transport` for nothing.
pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// The variable that replaces the trust anchors with a PEM bundle of the
/// caller's choosing.
///
/// The spelling is OpenSSL's, and `curl` and `git` honour the same one. It
/// **replaces** rather than adds, which is also what those three do: a user who
/// names a bundle is saying which roots to trust, not which extra ones.
pub const CERT_FILE_ENV: &str = "SSL_CERT_FILE";

/// The places a Unix host keeps its concatenated PEM bundle, in the order they
/// are tried.
///
/// The first that can be read wins. macOS keeps Apple's copy at the first
/// entry; the rest are the four spellings the mainstream Linux distributions
/// use. A host with none of them is not guessed at — it is told, by name, that
/// it has no trust anchors and which variable to set.
const BUNDLES: &[&str] = &[
    // macOS, and anywhere LibreSSL is the system TLS.
    "/etc/ssl/cert.pem",
    // Debian, Ubuntu, Alpine, Arch.
    "/etc/ssl/certs/ca-certificates.crt",
    // Fedora, RHEL, CentOS.
    "/etc/pki/tls/certs/ca-bundle.crt",
    // openSUSE.
    "/etc/ssl/ca-bundle.pem",
    // Older Red Hat derivatives.
    "/etc/ssl/certs/ca-bundle.crt",
];

/// The trust anchors, and the name of where they came from.
///
/// The source travels with the store because it is half of every refusal
/// message: a certificate that was not accepted is only actionable next to the
/// set it was checked against.
struct Roots {
    store: RootCertStore,
    source: String,
}

/// Open a TLS connection over an already-connected socket.
///
/// `host` is the authority's host part, which is both the SNI name sent to the
/// server and the name the certificate is checked against. An IP literal is a
/// valid answer here — `ServerName` carries one — and it is checked against the
/// certificate's IP SANs rather than its DNS ones, which is why
/// `https://127.0.0.1/` against a certificate for `localhost` is a refusal and
/// not a pass.
///
/// The handshake is completed **here**, before the caller writes a byte. A
/// lazily-handshaking stream would surface an untrusted certificate as an error
/// from `write_all` of the request line, which is the wrong error at the wrong
/// place; doing it here means [`connect`]'s `Err` is always about the peer's
/// identity or the transport, never about HTTP.
///
/// And it is completed **within a deadline**, whoever the caller is. `http.rs`
/// sets one on the socket before it hands it over — the deadlines it sets cover
/// the handshake for exactly that reason — but a handshake is a conversation
/// with a stranger, and a stranger who accepts a connection and then says
/// nothing must not be able to keep this thread. [`HANDSHAKE_DEADLINE`] is what
/// a socket that arrives here with no deadline of its own is given, so the
/// property belongs to this function rather than to the discipline of its
/// callers.
pub fn connect(sock: TcpStream, host: &str) -> Result<TlsStream, NetFail> {
    let Roots { store, source } = trust_anchors().map_err(NetFail::Transport)?;

    // `builder_with_provider` rather than `builder`: the latter reads a
    // *process-global* default provider, which is state a library in a static
    // archive has no business depending on. One provider is compiled in and it
    // is named here.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| {
            NetFail::Transport(format!("tls: this runtime's TLS configuration is not usable: {e}"))
        })?
        .with_root_certificates(store)
        .with_no_client_auth();

    let name = ServerName::try_from(host.to_string()).map_err(|_| {
        NetFail::Transport(format!(
            "tls: `{host}` is not a name a server certificate can be checked against"
        ))
    })?;
    let conn = ClientConnection::new(Arc::new(config), name).map_err(|e| {
        NetFail::Transport(format!("tls: the connection could not be started: {e}"))
    })?;

    bound(&sock);

    let mut stream = StreamOwned::new(conn, sock);
    // `complete_io` drives the handshake until it has nothing left to send or
    // to wait for. The bound is not expected to be reached — one call is
    // normally the whole handshake — and it is here because a peer that leaves
    // the connection wanting neither a read nor a write would otherwise be an
    // infinite loop rather than an error.
    let mut rounds = 0;
    while stream.conn.is_handshaking() {
        stream
            .conn
            .complete_io(&mut stream.sock)
            .map_err(|e| handshake_failure(&e, &source))?;
        rounds += 1;
        if rounds > 8 {
            return Err(NetFail::Transport(String::from("tls: the handshake did not finish")));
        }
    }
    Ok(stream)
}

/// What a handshake gets if its socket carries no deadline of its own.
///
/// The same thirty seconds `http.rs` uses, and stated separately rather than
/// imported from there because the two are answering different questions: that
/// one is this client's policy for a request, and this one is the floor under
/// *any* caller of [`connect`]. They would be free to differ.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);

/// Put a deadline on the socket if it has none.
///
/// **If**, and not unconditionally: a caller that set thirty seconds has said
/// what it wants and a caller that set three has said something this file has
/// no business overruling. What it refuses to allow is the third case, a socket
/// with no deadline at all, because a peer that completes the TCP handshake and
/// then sends nothing would hold the calling thread until the process ended.
///
/// The deadline stays on the socket afterwards. The stream is handed back for
/// the caller to read and write, and a peer that can stall a handshake can
/// stall the read after it just as easily; taking the bound off at the end
/// would be removing it exactly where it goes on being needed.
fn bound(sock: &TcpStream) {
    if matches!(sock.read_timeout(), Ok(None)) {
        let _ = sock.set_read_timeout(Some(HANDSHAKE_DEADLINE));
    }
    if matches!(sock.write_timeout(), Ok(None)) {
        let _ = sock.set_write_timeout(Some(HANDSHAKE_DEADLINE));
    }
}

/// The answer for a handshake that did not complete.
///
/// Three shapes, and the differences are the ones a caller acts on. A deadline
/// that ran out is a `Timeout` — the same answer a stalled read gets, because
/// it is the same event one layer down, and a program retrying on `Timeout`
/// should not have to know that this one happened during a handshake. A
/// certificate the verifier refused is a *trust* answer and gets the trust
/// source named beside it. Everything else — a protocol error, a closed socket,
/// a version mismatch — is a transport answer and gets the error as it stands.
///
/// `rustls` reports its own errors by wrapping them in an `io::Error`, so the
/// second distinction is a downcast rather than a string match; the first is
/// the socket's own deadline surfacing, so it is an `ErrorKind`.
fn handshake_failure(e: &std::io::Error, source: &str) -> NetFail {
    if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
        return NetFail::Timeout;
    }
    let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) else {
        return NetFail::Transport(format!("tls: the handshake failed: {e}"));
    };
    if matches!(inner, rustls::Error::InvalidCertificate(_)) {
        return NetFail::Transport(format!(
            "tls: the server's certificate was refused: {inner}. The trust anchors came from \
             {source}; set {CERT_FILE_ENV} to a PEM bundle of the roots this program should trust."
        ));
    }
    NetFail::Transport(format!("tls: the handshake failed: {inner}"))
}

/// Read the host's trust anchors, or say why there are none.
fn trust_anchors() -> Result<Roots, String> {
    let (pem, source) = match std::env::var_os(CERT_FILE_ENV) {
        Some(named) => {
            let path = PathBuf::from(named);
            let text = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "tls: {CERT_FILE_ENV} names {}, which could not be read: {e}",
                    path.display()
                )
            })?;
            (text, path.display().to_string())
        }
        None => {
            let found = BUNDLES
                .iter()
                .find_map(|c| std::fs::read_to_string(c).ok().map(|text| (text, (*c).to_string())));
            match found {
                Some(pair) => pair,
                None => {
                    return Err(format!(
                        "tls: this host has no certificate authorities to check a server against \
                         — none of {} could be read. Set {CERT_FILE_ENV} to a PEM bundle of the \
                         roots this program should trust.",
                        BUNDLES.join(", ")
                    ))
                }
            }
        }
    };

    let mut store = RootCertStore::empty();
    for der in certificates_in(&pem) {
        // A bundle a platform ships is a *historical* document: it holds
        // certificates with algorithms and encodings this verifier will refuse,
        // and one of them is not a reason to distrust the other two hundred.
        // The emptiness check below is what turns "all of them were refused"
        // into an error, which is the case that actually means something.
        let _ = store.add(CertificateDer::from(der));
    }
    if store.is_empty() {
        return Err(format!(
            "tls: {source} holds no certificate authority this runtime could use, so no server \
             certificate could be checked"
        ));
    }
    Ok(Roots { store, source })
}

/// Every `CERTIFICATE` block of a PEM document, decoded.
///
/// A hand-rolled reader rather than `rustls-pemfile`, for the reason the whole
/// of this file's trust story is hand-rolled: a bundle is base64 between two
/// marker lines, and a crate in every binary this compiler produces is a price
/// with a bar in front of it. Anything that is not a certificate block — a
/// private key, a comment, the human-readable dump `/etc/ssl/cert.pem` puts
/// above each entry — is skipped by construction, because only the text
/// between the two markers is ever looked at.
fn certificates_in(pem: &str) -> Vec<Vec<u8>> {
    blocks_in(pem, "CERTIFICATE")
}

/// Every block of a PEM document carrying one label, decoded.
///
/// The label is a parameter because a PEM document is *labelled* blocks and
/// reading one kind out of a file that holds several is the whole shape of the
/// format; the trust reader asks for `CERTIFICATE` and the test server, which
/// needs a key, asks for `PRIVATE KEY`.
fn blocks_in(pem: &str, label: &str) -> Vec<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    pem.split(begin.as_str())
        .skip(1)
        .filter_map(|block| block.split_once(end.as_str()).and_then(|(body, _)| base64(body)))
        .collect()
}

/// Standard-alphabet base64, whitespace ignored, padding ended.
///
/// `None` on any character that is not in the alphabet, which is what makes a
/// truncated or corrupted block a block that is skipped rather than a trust
/// anchor made of the wrong bytes.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == '=' {
            break;
        }
        let value = match c {
            'A'..='Z' => u32::from(c) - u32::from('A'),
            'a'..='z' => u32::from(c) - u32::from('a') + 26,
            '0'..='9' => u32::from(c) - u32::from('0') + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((accumulator >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    /// The test certificate authority, and the leaf it signed.
    ///
    /// Generated once, offline, by the commands below, and checked in as text
    /// because the alternative is a certificate *generator* — which is either a
    /// crate this repository will not admit for a test, or a hand-written X.509
    /// writer, which would be a second thing to get wrong beside the client
    /// under test.
    ///
    /// ```text
    /// openssl ecparam -name prime256v1 -genkey -noout -out ca.key
    /// openssl req -x509 -new -key ca.key -sha256 -days 8000 -out ca.crt \
    ///   -subj "/CN=buri runtime test CA" \
    ///   -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
    ///   -addext "keyUsage=critical,keyCertSign,cRLSign"
    /// openssl ecparam -name prime256v1 -genkey -noout -out leaf.key
    /// openssl pkcs8 -topk8 -nocrypt -in leaf.key -out leaf.pk8
    /// openssl req -new -key leaf.key -out leaf.csr -subj "/CN=localhost"
    /// openssl x509 -req -in leaf.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
    ///   -out leaf.crt -days 8000 -sha256 -extfile leaf.ext
    /// #   leaf.ext: basicConstraints=critical,CA:FALSE
    /// #             keyUsage=critical,digitalSignature
    /// #             extendedKeyUsage=serverAuth
    /// #             subjectAltName=DNS:localhost
    /// ```
    ///
    /// P-256 rather than RSA so the fixture is four lines rather than forty,
    /// and 8000 days rather than a century so the `notAfter` stays inside the
    /// UTCTime era every X.509 parser agrees about. It expires in **2048**; a
    /// maintainer who meets an `Expired` here should re-run the block above.
    const CA_PEM: &str = "\
-----BEGIN CERTIFICATE-----
MIIBpTCCAUygAwIBAgIUbhJr2chPv/c7SF7l4NzpYPs3TvIwCgYIKoZIzj0EAwIw
HzEdMBsGA1UEAwwUYnVyaSBydW50aW1lIHRlc3QgQ0EwHhcNMjYwODMwMTYzMDI1
WhcNNDgwNzI1MTYzMDI1WjAfMR0wGwYDVQQDDBRidXJpIHJ1bnRpbWUgdGVzdCBD
QTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABNzK2k8RMPmAbeUnOQN0XOeLbErK
jrxvaxKWSvHP1RvgWBP76WZFmDjJPnkMnrPPzgWKSH7aVXi4kuNYtAA8aGGjZjBk
MB0GA1UdDgQWBBTA1KvkoObvR+8An7VQ5rGc8ibMozAfBgNVHSMEGDAWgBTA1Kvk
oObvR+8An7VQ5rGc8ibMozASBgNVHRMBAf8ECDAGAQH/AgEAMA4GA1UdDwEB/wQE
AwIBBjAKBggqhkjOPQQDAgNHADBEAiBVHuidBPkVtHVAGk22n1tXvJJOWFus9Kev
u66hvyYsxgIgYiwfnji/XJr7G0G3su3bda6gySR7mwXtJaSJYsw0AQo=
-----END CERTIFICATE-----
";

    /// The leaf, `CN=localhost` with `subjectAltName = DNS:localhost` and no
    /// IP SAN — which is what makes the `127.0.0.1` case below a refusal.
    const LEAF_PEM: &str = "\
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
    const LEAF_KEY_PEM: &str = "\
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgVHRl2YgdCy3mjKbk
16fSBF1ppkf1hD5sToo8d62EJnuhRANCAAQOZuolOZh48E1a/BM/6evUztl8opNv
N36cRROHvFG5TJfrBSfH3IXkHfALHOC4nsMZgUIK1DDUYy/eh0P1jYuY
-----END PRIVATE KEY-----
";

    /// A second, unrelated authority, generated the same way and used for
    /// exactly one thing: being the trust set that does *not* contain the
    /// issuer of the certificate the server presents.
    ///
    /// The alternative — unsetting `SSL_CERT_FILE` and relying on the host's
    /// own bundle to lack our CA — would make the assertion depend on the
    /// machine, and would report "this host has no trust anchors" instead of
    /// "unknown issuer" on a host that has none.
    const OTHER_CA_PEM: &str = "\
-----BEGIN CERTIFICATE-----
MIIByTCCAXCgAwIBAgIUYC5rUXwfSFA+rQKR1zgldP6ZLFkwCgYIKoZIzj0EAwIw
MTEvMC0GA1UEAwwmYnVyaSBydW50aW1lIHRlc3QgQ0EgKGEgZGlmZmVyZW50IG9u
ZSkwHhcNMjYwODMwMTYzNTIyWhcNNDgwNzI1MTYzNTIyWjAxMS8wLQYDVQQDDCZi
dXJpIHJ1bnRpbWUgdGVzdCBDQSAoYSBkaWZmZXJlbnQgb25lKTBZMBMGByqGSM49
AgEGCCqGSM49AwEHA0IABAOaSzxRe2XbEVjyYA6Wu5dd1Og5v/BrkP8OatQ5Qk5w
tKGrWmYd+LwVJfu/GJDFAYCDegmf32plewF+FbAbOOujZjBkMB0GA1UdDgQWBBRn
ywuO5Y1fcWi7Fu54eb+Y8CGNhzAfBgNVHSMEGDAWgBRnywuO5Y1fcWi7Fu54eb+Y
8CGNhzASBgNVHRMBAf8ECDAGAQH/AgEAMA4GA1UdDwEB/wQEAwIBBjAKBggqhkjO
PQQDAgNHADBEAiBL3tGa29QwilgocWLjccWUaeBKL5DlkIxefn6hAjPPcgIgYqqf
YJlcERJ3qukVVHKAplDs77VXp3fy97GLt3F86A0=
-----END CERTIFICATE-----
";

    /// How long anything in these tests waits for anything.
    ///
    /// Every wait below is bounded by it: the accept, each read the server
    /// does, each write. Generous, because a loaded machine is not a failing
    /// one and the exchanges here take microseconds when they happen at all —
    /// and finite, because the alternative is the state this file was in, where
    /// a case that could not connect became a job that ran until CI killed it.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Loopback listeners on one port: `127.0.0.1` and `::1`, or whichever of
    /// the two this host has.
    ///
    /// **This is where the hang was.** Four of the five cases below reach the
    /// server by the name `localhost`, and which address that name answers with
    /// is the host's business — this machine offers `127.0.0.1` first and a
    /// great many offer `::1` first. A server on one family and a client on the
    /// other never meet: the connect is refused, or on a host with no route for
    /// the family it picked it is not answered at all. Either way the case's
    /// `fetch` failed, and the test thread then joined a server thread still
    /// sitting inside `accept` — with no deadline anywhere in the picture. That
    /// is a deterministic hang on every host that orders the two the other way
    /// round, and it is what a CI job's sixty-minute timeout was reporting.
    ///
    /// Binding both families dissolves the question instead of answering it,
    /// and does so without asking a resolver anything: the port comes from the
    /// v4 bind, and the v6 bind either takes the same port or the pair is
    /// dropped and another port tried. A host with no IPv6 loopback keeps the
    /// v4 listener alone, which is what it can use.
    fn loopback() -> (u16, Vec<TcpListener>) {
        use std::net::{Ipv4Addr, Ipv6Addr};
        for _ in 0..32 {
            let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback listener");
            let port = v4.local_addr().expect("the port it was given").port();
            let mut listeners = vec![v4];
            match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
                Ok(v6) => listeners.push(v6),
                // The same port is already taken on the other family. Drop both
                // and ask for another one rather than serving a port whose two
                // halves belong to two different tests.
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
                // No IPv6 loopback on this host at all.
                Err(_) => {}
            }
            return (port, listeners);
        }
        panic!("no port was free on both loopback families in thirty-two tries")
    }

    /// The first connection to arrive on any of the listeners, or `None` if
    /// none does before the deadline.
    ///
    /// Polled rather than blocked on, because "wait on two sockets" is `select`
    /// or a thread each, and both are more machinery than a millisecond of
    /// sleep buys back in a test whose connections arrive in microseconds. What
    /// the loop is *for* is the `None`: a server that gives up is a case that
    /// fails with a message, and a server that cannot give up is a suite that
    /// hangs.
    fn accept_within(listeners: &[TcpListener], patience: Duration) -> Option<TcpStream> {
        for listener in listeners {
            listener.set_nonblocking(true).ok()?;
        }
        let deadline = std::time::Instant::now() + patience;
        loop {
            for listener in listeners {
                match listener.accept() {
                    Ok((sock, _)) => {
                        sock.set_nonblocking(false).ok()?;
                        return Some(sock);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return None,
                }
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A rustls server on the loopback interface, serving the fixture
    /// certificate to exactly one connection, and answering the port and a join
    /// handle that yields whatever request text arrived.
    ///
    /// It is deliberately *tolerant*: four of the five cases below are
    /// refusals, and in a refusal the client hangs up mid-handshake. A server
    /// that unwrapped its way through that would turn every expected refusal
    /// into a panicking thread.
    ///
    /// And it is deliberately *impatient*: every wait it does has [`PATIENCE`]
    /// on it, so the handle it returns can always be joined. An empty string is
    /// what a case that never connected gets back, and an empty string fails an
    /// assertion — which is the whole difference between a test that reports a
    /// broken network and one that disappears into it.
    fn serve(response: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let der = blocks_in(LEAF_KEY_PEM, "PRIVATE KEY").pop().expect("the fixture key");
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(der.into());
        let chain: Vec<CertificateDer<'static>> =
            certificates_in(LEAF_PEM).into_iter().map(CertificateDer::from).collect();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();

        let (port, listeners) = loopback();
        let handle = std::thread::spawn(move || {
            let Some(sock) = accept_within(&listeners, PATIENCE) else { return String::new() };
            // The reads and the write below get the same bound the accept had,
            // so a client that connects and then stops talking — which is what
            // a refusal looks like from this side — cannot hold this thread
            // either.
            let _ = sock.set_read_timeout(Some(PATIENCE));
            let _ = sock.set_write_timeout(Some(PATIENCE));
            let Ok(conn) = rustls::ServerConnection::new(Arc::new(config)) else {
                return String::new();
            };
            let mut stream = StreamOwned::new(conn, sock);
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return String::from_utf8_lossy(&request).into_owned(),
                    Ok(n) => request.extend_from_slice(buffer.get(..n).unwrap_or(&[])),
                }
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            stream.conn.send_close_notify();
            let _ = stream.conn.complete_io(&mut stream.sock);
            String::from_utf8_lossy(&request).into_owned()
        });
        (port, handle)
    }

    /// A PEM file under the temporary directory, named for this process.
    fn bundle(name: &str, pem: &str) -> PathBuf {
        let path = std::env::temp_dir()
            .join(format!("buri-rt-tls-{}-{name}.pem", std::process::id()));
        std::fs::write(&path, pem).unwrap();
        path
    }

    /// Point the trust set at a file, for the duration of one case.
    ///
    /// `SSL_CERT_FILE` is process state, and `set_var` is `unsafe` in edition
    /// 2024 because `setenv` and `getenv` share one `environ` across threads.
    /// Two things bound that here, and neither is "it is fine":
    ///
    /// * **This is the only writer in the crate**, and all five cases are one
    ///   `#[test]` for exactly that reason — five would be five threads writing
    ///   the same variable.
    /// * The only other environment *reader* in the runtime is
    ///   `testing::resume_from`, a `OnceLock` over `BURI_TEST_FROM`, and
    ///   `host::buri_rt_host_env_get`, which no unit test in this crate calls.
    ///
    /// The alternative — a seam that let the test hand the trust source in
    /// directly — would have tested everything except the lines that decide
    /// *where the trust source comes from*, which is the decision this file is
    /// about.
    fn trust(path: &PathBuf) {
        unsafe { std::env::set_var(CERT_FILE_ENV, path) };
    }

    const RESPONSE: &str = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/plain\r\n",
        "Content-Length: 5\r\n",
        "Connection: close\r\n",
        "\r\n",
        "hello",
    );

    /// `https://` end to end against a server this test started, and the four
    /// refusals that are the honest half of it.
    ///
    /// Offline by construction: the listeners are on the loopback interface,
    /// the certificate was generated for this file, and the trust anchors are a
    /// file this function wrote. `localhost` is the only name in it and it is
    /// answered out of the host's own tables — there is no socket to anywhere
    /// but this machine, and [`loopback`] is what makes the answer to that name
    /// stop mattering.
    ///
    /// Bounded by construction too, which is the newer half: the server gives
    /// up after [`PATIENCE`], and the client's own dial is bounded by
    /// `http.rs`'s deadline. Every way this case can go wrong is a failing
    /// assertion in finite time; none of them is a job that has to be killed.
    #[test]
    fn https_is_checked_against_the_trust_anchors() {
        let started = Instant::now();
        let ours = bundle("ca", CA_PEM);
        let stranger = bundle("other", OTHER_CA_PEM);
        let empty = bundle("empty", "# a bundle with no certificate in it\n");

        // 1. The certificate's issuer is trusted and the name matches: a real
        //    HTTPS exchange, parsed by the same code `http://` is parsed by.
        trust(&ours);
        let (port, server) = serve(RESPONSE);
        let answer = crate::http::fetch(0, &format!("https://localhost:{port}/probe"), &[], b"");
        // The answer is read before the server is joined, so that a client that
        // failed is reported as the client failing. Joining first would report
        // it as whatever the server made of never being connected to — which,
        // before this file's server had a deadline, was nothing at all, forever.
        let response = match answer {
            Ok(r) => r,
            Err(e) => panic!("https fetch failed: tag {} {}", e.tag(), e.message()),
        };
        let request = server.join().unwrap();
        assert!(
            request.starts_with("GET /probe HTTP/1.1\r\n"),
            "the request the server saw was:\n{request}"
        );
        assert!(
            request.contains(&format!("Host: localhost:{port}\r\n")),
            "the request the server saw was:\n{request}"
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
        assert!(response.headers.iter().any(|(n, v)| n == "content-type" && v == "text/plain"));

        // 2. The same certificate, reached by an address it does not cover.
        //    `localhost` is a DNS SAN and `127.0.0.1` is not an IP SAN, so this
        //    is the "right server, wrong name" refusal.
        let (port, server) = serve(RESPONSE);
        let answer = crate::http::fetch(0, &format!("https://127.0.0.1:{port}/probe"), &[], b"");
        let _ = server.join();
        let message = answer.err().expect("a certificate for localhost is not one for 127.0.0.1");
        assert_eq!(
            message.message(),
            format!(
                "tls: the server's certificate was refused: invalid peer certificate: \
                 certificate not valid for name \"127.0.0.1\"; certificate is only valid for \
                 DnsName(\"localhost\"). The trust anchors came from {}; set SSL_CERT_FILE to a \
                 PEM bundle of the roots this program should trust.",
                ours.display()
            )
        );

        // 3. The name matches and the issuer is a stranger. This is the case
        //    the whole file exists for: a client that accepted here would be a
        //    client that accepts anyone.
        trust(&stranger);
        let (port, server) = serve(RESPONSE);
        let answer = crate::http::fetch(0, &format!("https://localhost:{port}/probe"), &[], b"");
        let _ = server.join();
        let message = answer.err().expect("an untrusted issuer is not a trusted one");
        assert_eq!(
            message.message(),
            format!(
                "tls: the server's certificate was refused: invalid peer certificate: \
                 UnknownIssuer. The trust anchors came from {}; set SSL_CERT_FILE to a PEM \
                 bundle of the roots this program should trust.",
                stranger.display()
            )
        );

        // 4. A bundle with nothing in it is not an empty trust set that refuses
        //    everything with `UnknownIssuer` — it is a misconfiguration, and it
        //    says which file it read and found nothing in.
        //
        //    A live server for this and for the case after it, even though
        //    neither gets as far as a handshake: `fetch` opens the socket
        //    before it wraps it, so a closed port would answer `Refused` and
        //    the message under test would never be reached.
        trust(&empty);
        let (port, server) = serve(RESPONSE);
        let answer = crate::http::fetch(0, &format!("https://localhost:{port}/probe"), &[], b"");
        let _ = server.join();
        let message = answer.err().expect("no anchors is no connection");
        assert_eq!(
            message.message(),
            format!(
                "tls: {} holds no certificate authority this runtime could use, so no server \
                 certificate could be checked",
                empty.display()
            )
        );

        // 5. And a bundle that is not there at all.
        let missing = std::env::temp_dir().join("buri-rt-tls-no-such-bundle.pem");
        let _ = std::fs::remove_file(&missing);
        trust(&missing);
        let (port, server) = serve(RESPONSE);
        let answer = crate::http::fetch(0, &format!("https://localhost:{port}/probe"), &[], b"");
        let _ = server.join();
        let message = answer.err().expect("an unreadable bundle is not a trust set");
        assert!(
            message
                .message()
                .starts_with(&format!("tls: SSL_CERT_FILE names {}, which could not be read", missing.display())),
            "the message was: {}",
            message.message()
        );

        unsafe { std::env::remove_var(CERT_FILE_ENV) };
        for path in [ours, stranger, empty] {
            let _ = std::fs::remove_file(path);
        }

        // Five loopback handshakes are microseconds of work, and this bound is
        // two orders of magnitude above them. It is not a performance
        // assertion: it is the one that fails, with a number, on the machine
        // where a step that used to be unbounded has started waiting again.
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the five cases took {:?}, which is long enough that something waited rather than \
             answered",
            started.elapsed()
        );
    }

    /// The PEM reader takes the certificates and leaves everything else.
    #[test]
    fn a_bundle_is_read_block_by_block() {
        let document = format!("{CA_PEM}\nsome human-readable text\n{OTHER_CA_PEM}");
        let found = certificates_in(&document);
        assert_eq!(found.len(), 2);
        // DER, so the first two octets are a SEQUENCE and its long-form length.
        assert_eq!(found[0].first(), Some(&0x30));
        assert_eq!(found[1].first(), Some(&0x30));
        // The private key is not a certificate.
        assert!(certificates_in(LEAF_KEY_PEM).is_empty());
        // A block that is not base64 is skipped rather than half-decoded.
        assert!(certificates_in(
            "-----BEGIN CERTIFICATE-----\nnot base64!\n-----END CERTIFICATE-----\n"
        )
        .is_empty());
    }

    /// Base64 against the three residues, because the tail is where a
    /// hand-rolled decoder goes wrong.
    #[test]
    fn base64_decodes_every_tail() {
        assert_eq!(base64("").unwrap(), b"");
        assert_eq!(base64("TQ==").unwrap(), b"M");
        assert_eq!(base64("TWE=").unwrap(), b"Ma");
        assert_eq!(base64("TWFu").unwrap(), b"Man");
        // Whitespace is a line break in a PEM file and nothing else.
        assert_eq!(base64("TW\nFu").unwrap(), b"Man");
        assert!(base64("TWF*").is_none());
    }
}
