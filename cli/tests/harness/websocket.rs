//! A WebSocket **server**, hand-written, for the rows that dial one.
//!
//! `native/shared.rs`'s `Talking` is this file read the other way round: a
//! hand-written *client* for the rows where a Buri program holds the port. This
//! is the half those rows cannot be, because `core/net/websocket` is a client
//! and a client needs somebody to dial.
//!
//! **Hand-written for `Talking`'s reason**, which is
//! `language::corpus::dependencies_stay_behind_the_bar`: the only RFC 6455
//! implementation in this repository is inside the runtime archive, and a
//! `[dev-dependencies]` entry on `tungstenite` is a dependency the workspace may
//! not grow. So the handshake's one piece of cryptography — SHA-1 over the key
//! the client sent and RFC 6455 §1.3's constant — is [`sha1`] below, forty lines
//! and no crate, checked against the specification's own worked example in
//! [`the_accept_key_is_the_one_rfc_6455_works_through`].
//!
//! **What it exists to say that a Buri server cannot.** This repository's own
//! acceptor closes normally, answers pings itself, never fragments a message and
//! never writes a handshake that is not a `101`. Every one of those is a thing a
//! *client* has to cope with, so the far side of a client row has to be a
//! listener a test holds:
//!
//! * a close with each of the codes `CloseReason` names, with a code it does not
//!   name, with no code at all, and with no close frame at all;
//! * a message in three fragments, split mid-character, which reaches a program
//!   as one whole message;
//! * a ping, which reaches no program at all;
//! * text and binary payloads of nothing, of one octet, and of past the point
//!   where a frame's length stops fitting in two.
//!
//! Every read and write carries [`PATIENCE`], and the accept loop stops after
//! the sessions it was given, so a client that never dials is a joined thread
//! and a failing assertion rather than a job CI has to kill.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// How long any one read or write of this server may take.
///
/// The same twenty seconds `native/shared.rs`'s `SERVER_DEADLINE` gives a whole
/// exchange, written again here because that constant belongs to a different
/// binary and this file is included by two.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// One thing this server does, in the order a session's list gives them.
#[derive(Clone, Debug)]
pub enum Step {
    /// One text message, in one frame.
    Text(String),
    /// One binary message, in one frame.
    Binary(Vec<u8>),
    /// One text message, in as many frames as there are pieces — the first with
    /// `FIN` clear and opcode `text`, the rest continuations, the last with
    /// `FIN` set. A program is owed the whole message and never a piece.
    ///
    /// **Octets rather than strings, so a piece can end mid-character.** RFC
    /// 6455 §5.4 lets a UTF-8 sequence span a fragment boundary, and a
    /// reassembly that decoded each frame on its own would corrupt exactly
    /// there — which is the failure this variant exists to be able to write.
    Fragments(Vec<Vec<u8>>),
    /// A ping. Nothing above the transport may ever learn that one happened.
    Ping(Vec<u8>),
    /// Wait for one message from the client, and record it.
    Hear,
    /// A close frame carrying this code, or carrying no payload at all when it
    /// is `None` — which is RFC 6455's 1005, "the far side sent no code".
    Close(Option<u16>),
    /// Wait for the **client** to close, record it, and answer with a close
    /// frame carrying this code.
    ///
    /// The other direction of [`Step::Close`], and it is a step of its own
    /// because a JavaScript engine tells a program far more about a close it
    /// started than about one it was sent — `design/native/DECISIONS.md` has
    /// the row.
    Bye(u16),
    /// Drop the connection with no close frame: RFC 6455's 1006.
    Drop,
    /// Answer the handshake with this, rather than with a `101` this server
    /// signed. What it is for is the refusals — a status that is not `101`, a
    /// `101` signing another handshake's key — and it ends the session.
    Answer(String),
    /// Accept the connection and drop it without answering the handshake at
    /// all, which ends the session.
    Silence,
}

/// One thing this server was told, in the order it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    Text(String),
    Binary(Vec<u8>),
    /// The client closed, with the code it sent or `None` for an empty close.
    Closed(Option<u16>),
    /// The connection ended without a close frame.
    Gone,
}

/// A listening port and the thread behind it.
pub struct Serving {
    /// The port the kernel chose. Announced rather than picked, which is the
    /// rule every socket row in this repository keeps.
    pub port: u16,
    thread: std::thread::JoinHandle<Vec<Vec<Heard>>>,
}

impl Serving {
    /// What each session was told, oldest session first. Joins the thread, so
    /// a server that is still waiting for a dial fails here rather than
    /// outliving the test.
    pub fn heard(self) -> Vec<Vec<Heard>> {
        self.thread.join().expect("the serving thread finished")
    }
}

/// A server that answers `sessions.len()` dials, one after another, running
/// each session's steps in order.
///
/// **One connection at a time and in order**, because a reconnect loop is a
/// sequence: the second dial cannot start until the first socket has closed, so
/// a session's script is the script for that session and not a race between
/// them.
pub fn serving(sessions: Vec<Vec<Step>>) -> Serving {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a bound port");
    let port = listener.local_addr().expect("the bound port").port();
    let thread = std::thread::spawn(move || {
        let mut log = Vec::new();
        for session in sessions {
            let Ok((socket, _from)) = listener.accept() else {
                log.push(Vec::new());
                continue;
            };
            log.push(session_of(socket, &session));
        }
        log
    });
    Serving { port, thread }
}

/// One dial, from the handshake to whatever ended it.
fn session_of(socket: TcpStream, steps: &[Step]) -> Vec<Heard> {
    let _read = socket.set_read_timeout(Some(PATIENCE));
    let _written = socket.set_write_timeout(Some(PATIENCE));
    let mut peer = Peer { socket, over: Vec::new() };
    let Some(key) = peer.handshake() else { return Vec::new() };
    // A session that answers something other than a `101` is over as soon as it
    // has said it: there is no framing behind an answer that did not switch
    // protocols, and no bytes after it belong to anybody.
    match steps.first() {
        Some(Step::Silence) => return Vec::new(),
        Some(Step::Answer(answer)) => {
            let _sent = peer.socket.write_all(answer.as_bytes());
            let _flushed = peer.socket.flush();
            return Vec::new();
        }
        _ => {}
    }
    let accept = accept_key(&key);
    let answer = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
         sec-websocket-accept: {accept}\r\n\r\n"
    );
    if peer.socket.write_all(answer.as_bytes()).is_err() {
        return Vec::new();
    }
    let _flushed = peer.socket.flush();
    let mut heard = Vec::new();
    for step in steps {
        match step {
            Step::Text(text) => peer.send(true, 0x1, text.as_bytes()),
            Step::Binary(data) => peer.send(true, 0x2, data),
            Step::Ping(data) => peer.send(true, 0x9, data),
            Step::Fragments(pieces) => {
                for (at, piece) in pieces.iter().enumerate() {
                    let opcode = if at == 0 { 0x1 } else { 0x0 };
                    let last = at.saturating_add(1) == pieces.len();
                    peer.send(last, opcode, piece);
                }
            }
            Step::Hear => heard.push(peer.hear()),
            Step::Close(code) => {
                let payload = code.map(|c| c.to_be_bytes().to_vec()).unwrap_or_default();
                peer.send(true, 0x8, &payload);
                // Read to the end rather than dropping the socket here. A reset
                // can throw away bytes the peer has not read yet, and the close
                // frame just written is exactly such a byte.
                peer.drain();
                return heard;
            }
            Step::Bye(code) => {
                heard.push(peer.hear());
                peer.send(true, 0x8, &code.to_be_bytes());
                // This side closes the connection, because this side is the one
                // that answered — RFC 6455 §7.1.1. A client waiting for that is
                // waiting for the drop at the end of this function.
                peer.drain();
                return heard;
            }
            Step::Drop => return heard,
            // Already handled above, and only in first position.
            Step::Answer(_) | Step::Silence => return heard,
        }
    }
    peer.drain();
    heard
}

/// The socket, and whatever a read of it overshot into.
struct Peer {
    socket: TcpStream,
    over: Vec<u8>,
}

impl Peer {
    /// The request head, and the `sec-websocket-key` off it.
    fn handshake(&mut self) -> Option<String> {
        let mut head: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 512];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match self.socket.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(n) => head.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            }
            if head.len() > 64 * 1024 {
                return None;
            }
        }
        let at = head.windows(4).position(|w| w == b"\r\n\r\n")?;
        self.over = head.split_off(at.saturating_add(4));
        let text = String::from_utf8_lossy(&head).into_owned();
        text.lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("sec-websocket-key"))
            .map(|(_, value)| value.trim().to_string())
    }

    /// One frame out. A server never masks (RFC 6455 §5.1), so this is the
    /// header and the payload and nothing between them.
    fn send(&mut self, fin: bool, opcode: u8, payload: &[u8]) {
        let mut frame = vec![if fin { 0x80 | opcode } else { opcode }];
        let len = payload.len();
        if len < 126 {
            frame.push(len as u8);
        } else if len <= usize::from(u16::MAX) {
            frame.push(126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        let _written = self.socket.write_all(&frame);
        let _flushed = self.socket.flush();
    }

    /// The next whole message from the client, continuations assembled and
    /// control frames skipped.
    ///
    /// A pong is skipped rather than recorded, because a pong is the client's
    /// transport answering this server's ping and nothing a program did.
    fn hear(&mut self) -> Heard {
        let mut assembling: Option<(u8, Vec<u8>)> = None;
        loop {
            let Some((fin, opcode, payload)) = self.frame() else { return Heard::Gone };
            match opcode {
                0x0 => {
                    let Some((kind, mut so_far)) = assembling.take() else {
                        return Heard::Gone;
                    };
                    so_far.extend_from_slice(&payload);
                    if fin {
                        return finished(kind, so_far);
                    }
                    assembling = Some((kind, so_far));
                }
                0x1 | 0x2 => {
                    if fin {
                        return finished(opcode, payload);
                    }
                    assembling = Some((opcode, payload));
                }
                0x8 => {
                    return Heard::Closed(
                        payload.get(..2).map(|two| u16::from_be_bytes([two[0], two[1]])),
                    );
                }
                // A ping this server does not answer, or a pong for one it
                // sent. Neither is a message.
                _ => continue,
            }
        }
    }

    /// One frame, unmasked. A client must mask every frame (RFC 6455 §5.3), so
    /// a frame that is not masked is a client that is wrong and this says so.
    fn frame(&mut self) -> Option<(bool, u8, Vec<u8>)> {
        let first = self.exactly(2)?;
        let fin = first[0] & 0x80 != 0;
        let opcode = first[0] & 0x0f;
        assert_eq!(first[1] & 0x80, 0x80, "a client must mask every frame it sends");
        let len = match first[1] & 0x7f {
            126 => {
                let two = self.exactly(2)?;
                u64::from(u16::from_be_bytes([two[0], two[1]]))
            }
            127 => {
                let eight = self.exactly(8)?;
                u64::from_be_bytes(eight.try_into().ok()?)
            }
            short => u64::from(short),
        };
        let mask = self.exactly(4)?;
        let masked = self.exactly(len as usize)?;
        let payload = masked
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i % 4])
            .collect::<Vec<u8>>();
        Some((fin, opcode, payload))
    }

    fn exactly(&mut self, n: usize) -> Option<Vec<u8>> {
        while self.over.len() < n {
            let mut chunk = [0u8; 4096];
            match self.socket.read(&mut chunk) {
                Ok(0) => return None,
                Ok(read) => self.over.extend_from_slice(chunk.get(..read).unwrap_or(&[])),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        }
        Some(self.over.drain(..n).collect())
    }

    /// Take whatever the client still has to say, so that closing this side
    /// does not throw the last frame away.
    ///
    /// **Bounded by a quarter of a second rather than by [`PATIENCE`], and the
    /// difference is twenty seconds a session.** A close is a handshake: this
    /// side sends a close frame, the client answers with one, and then
    /// *somebody* closes the connection. `tungstenite` closes it; `node` waits
    /// for the server to, so a drain that read to end-of-file sat on its
    /// twenty-second deadline once per session before this file gave up first.
    /// What the read is for is only the reset — a `close(2)` with unread octets
    /// in the receive buffer sends `RST` and can discard the close frame that
    /// was just written — and a quarter of a second is far more than a loopback
    /// exchange needs for that.
    fn drain(&mut self) {
        let _brief = self.socket.set_read_timeout(Some(Duration::from_millis(250)));
        let mut chunk = [0u8; 4096];
        while matches!(self.socket.read(&mut chunk), Ok(n) if n > 0) {}
    }
}

fn finished(opcode: u8, payload: Vec<u8>) -> Heard {
    if opcode == 0x1 {
        Heard::Text(String::from_utf8_lossy(&payload).into_owned())
    } else {
        Heard::Binary(payload)
    }
}

// ---------------------------------------------------------------------------
// The one piece of cryptography a handshake needs
// ---------------------------------------------------------------------------

/// RFC 6455 §1.3's constant, appended to the client's key before it is hashed.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The `sec-websocket-accept` for a client's `sec-websocket-key`.
pub fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

/// SHA-1, as FIPS 180-4 writes it.
///
/// **Here rather than from a crate** because the dependency bar is a rule about
/// the workspace and not about how much code a test may hold, and because SHA-1
/// is forty lines. It is used for exactly one thing — the handshake's accept
/// key — and never for anything a security claim rests on.
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let bits = (message.len() as u64).wrapping_mul(8);
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bits.to_be_bytes());
    for block in padded.chunks(64) {
        let mut w = [0u32; 80];
        for (i, slot) in w.iter_mut().enumerate().take(16) {
            let at = i.saturating_mul(4);
            *slot = u32::from_be_bytes([
                block[at],
                block[at.saturating_add(1)],
                block[at.saturating_add(2)],
                block[at.saturating_add(3)],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (word, slot) in h.iter().zip(out.chunks_mut(4)) {
        slot.copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Standard-alphabet base64 with padding, on `cli/runtime/net.rs`'s terms: the
/// archive has a decoder and no encoder, and twenty lines beats a crate.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in bytes.chunks(3) {
        let mut block = [0u8; 3];
        for (slot, byte) in block.iter_mut().zip(group) {
            *slot = *byte;
        }
        let packed =
            (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for sextet in 0..4usize {
            let digit = if sextet <= group.len() {
                let shift = 18_u32.saturating_sub(6 * sextet as u32);
                ALPHABET[((packed >> shift) & 63) as usize]
            } else {
                b'='
            };
            out.push(char::from(digit));
        }
    }
    out
}

/// The worked example RFC 6455 §1.3 prints, which is what says the forty lines
/// above are SHA-1 and not something that agrees with itself.
///
/// A handshake this server signed would be accepted by a client that made the
/// same mistake, so the check has to come from outside this repository — and
/// the specification's own key and answer are exactly that.
#[test]
fn the_accept_key_is_the_one_rfc_6455_works_through() {
    assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    // And SHA-1's own two published vectors, so a key that happened to work is
    // not the only evidence the hash is right.
    assert_eq!(
        base64(&sha1(b"abc")),
        "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=",
        "SHA-1 of `abc` is a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        base64(&sha1(b"")),
        "2jmj7l5rSw0yVb/vlWAYkK/YBwk=",
        "SHA-1 of the empty message is da39a3ee5e6b4b0d3255bfef95601890afd80709"
    );
}
