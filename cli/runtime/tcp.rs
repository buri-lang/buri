//! `Tcp` — a connection dialled out, with bytes each way and nothing over them.
//!
//! `net.rs` is the accepting half and `http.rs` is one request and one answer.
//! This is the layer under both, and it is deliberately the smallest file in
//! the runtime that opens a socket: `std::net::TcpStream`, a table of handles,
//! and four entries.
//!
//! ## What is bounded, and what is not
//!
//! **The dial is**, on `http.rs`'s argument and with `http.rs`'s machinery: a
//! name lookup is the one step socket options cannot bound, so it happens on a
//! thread of its own and this one waits [`DIAL`] for it. A `Tcp.tcpConnect`
//! that never returned would hang a carrier, and a carrier is not the caller's
//! to lose.
//!
//! **A read and a write are not.** That is the difference from every other
//! socket in this runtime and it is the point of the effect: a client waiting
//! on a subscription, a long poll or a `BLPOP` is waiting on purpose, and a
//! deadline this file chose would break it at a number nobody asked for. What a
//! caller that wants one does instead is close the stream from another task —
//! `Stream` is an inert handle, so it can go anywhere — and the blocked read
//! comes back an error.
//!
//! ## No TLS
//!
//! `tls.rs` is behind feature `net`, and this file is not: a cleartext socket
//! needs no crate at all, so a toolchain built without networking still answers
//! `Tcp` exactly as it answers `http://`. Wrapping one is `core/net/http`'s job
//! and `core/net/server`'s.
//!
//! ## The handle table
//!
//! A handle is a number a program holds, so it has to be meaningless to
//! anything but this table: **handles are never reused**, because a spent one
//! that came back would let a program that closed a stream write to somebody
//! else's. The counter only goes up. A handle naming nothing is treated as one
//! already closed — `IoError.NotFound` from a read or a write, and nothing at
//! all from a close — which is what `core/effect` promises.

use crate::value::{list_of_bytes, str_of, BuriList, BuriStr};
use crate::BURI_OK;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Mutex;
use std::time::Duration;

/// How long a dial may take, all of it: the name lookup and the connect.
///
/// `http.rs`'s [`DEADLINE`][d] is the same number for the same reason, and the
/// two are separate constants rather than one shared because they bound two
/// different things — that one bounds every step of an exchange, and this one
/// bounds only getting there.
///
/// [d]: crate::http
const DIAL: Duration = Duration::from_secs(30);

/// Every open stream, by the handle a program holds.
static STREAMS: Mutex<Option<HashMap<i64, TcpStream>>> = Mutex::new(None);

/// The next handle. Starts at one so that zero is never a live stream.
static NEXT: AtomicI64 = AtomicI64::new(1);

/// `IoError`'s variants as `host.rs` numbers them, for the failures a socket
/// has. The four a dial and a transfer actually produce are named; everything
/// else is `.Other` with the platform's own sentence, which is the same
/// division `host.rs`'s `io_error` makes for the filesystem.
fn classify(e: &std::io::Error) -> (i32, String) {
    match e.kind() {
        // A name that resolves to nothing, and a handle this table does not
        // hold: both are "there is nothing there".
        std::io::ErrorKind::NotFound => (0, String::new()),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ConnectionRefused => {
            (1, String::new())
        }
        _ => (6, e.to_string()),
    }
}

/// Write the error arm and return its tag — `host.rs`'s `fail`, on this file's
/// classification.
///
/// # Safety
/// `out_err` must be writable and aligned for a [`BuriStr`].
unsafe fn fail(e: &std::io::Error, out_err: *mut BuriStr) -> i32 {
    let (tag, message) = classify(e);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out_err.write(str_of(&message)) };
    tag
}

/// The same, for a failure this file states rather than one the platform
/// reported.
///
/// # Safety
/// As [`fail`].
unsafe fn refuse(tag: i32, message: &str, out_err: *mut BuriStr) -> i32 {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out_err.write(str_of(message)) };
    tag
}

/// The address to dial, found within the deadline.
///
/// `http.rs`'s `resolve`, and the argument is that file's in full: `getaddrinfo`
/// is the one step of a dial the socket options cannot bound, so it runs on a
/// thread of its own and this one waits. An address literal skips the whole
/// arrangement — there is nobody to ask.
fn resolve(host: &str, port: u16) -> Result<SocketAddr, (i32, String)> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let name = host.to_string();
    let (answer, wait) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(String::from("buri-rt-tcp"))
        .spawn(move || {
            let found = (name.as_str(), port)
                .to_socket_addrs()
                .map(|addrs| addrs.collect::<Vec<_>>())
                .map_err(|e| e.to_string());
            let _ = answer.send(found);
        })
        .map_err(|e| (6, format!("a name lookup needs a thread, and one could not start: {e}")))?;
    match wait.recv_timeout(DIAL) {
        Ok(Ok(addrs)) => addrs.into_iter().next().ok_or((0, String::new())),
        Ok(Err(e)) => Err((0, format!("could not resolve {host}: {e}"))),
        Err(RecvTimeoutError::Timeout) => {
            Err((6, format!("the name lookup for {host} did not answer in {DIAL:?}")))
        }
        // The thread ended without sending, which for a closure that cannot
        // return early means it panicked; its own message is already printed.
        Err(RecvTimeoutError::Disconnected) => {
            Err((6, format!("the name lookup for {host} ended without an answer")))
        }
    }
}

/// Do `f` with the stream `handle` names, or answer `None` for a handle this
/// table does not hold.
///
/// The stream is **taken out of the table for the duration**, so two carriers
/// reading one handle do not hold the lock across a syscall that waits — which
/// is what a read on this effect does on purpose. A handle whose stream is out
/// looks closed to anybody else, and `core/effect` says a handle that names no
/// open stream counts as one already closed.
fn with_stream<T>(handle: i64, f: impl FnOnce(&mut TcpStream) -> T) -> Option<T> {
    let mut taken = {
        let mut table = STREAMS.lock().ok()?;
        table.as_mut()?.remove(&handle)?
    };
    let answer = f(&mut taken);
    if let Ok(mut table) = STREAMS.lock() {
        if let Some(map) = table.as_mut() {
            map.insert(handle, taken);
        }
    }
    Some(answer)
}

/// `Tcp::tcpConnect(host, port) -> Result<Int, IoError>`.
///
/// `host` is a `Str`, so it arrives as its three leaves (`lib.rs` §2 rule 1);
/// `.Ok`'s payload is one scalar and leaves through the out-pointer, and the
/// message goes through the second (§2.1's message shape).
///
/// A port outside 0 to 65535 is `.Other` naming it rather than a wrap: a
/// program that computed a port badly should hear about it.
///
/// # Safety
/// The host view must be live; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_tcp_connect(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    port: i64,
    out_ok: *mut i64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let host = unsafe { crate::host::text(ptr, len) };
    let Ok(port) = u16::try_from(port) else {
        // SAFETY: the caller promises a writable destination.
        return unsafe { refuse(6, &format!("{port} is not a port number"), out_err) };
    };
    let address = match resolve(&host, port) {
        Ok(address) => address,
        // SAFETY: as above.
        Err((tag, message)) => return unsafe { refuse(tag, &message, out_err) },
    };
    // `host::about_to_block`'s rule: a name lookup and a dial both wait, and
    // these entries wait on the descriptor rather than on the reactor, so they
    // say it for themselves.
    crate::host::about_to_block();
    let stream = match TcpStream::connect_timeout(&address, DIAL) {
        Ok(stream) => stream,
        // SAFETY: as above.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    // Nagle off. A protocol client writes a command and waits for the answer,
    // which is exactly the shape delayed acknowledgement and Nagle interact
    // badly on; a caller that wants coalescing builds one buffer and writes it.
    let _ = stream.set_nodelay(true);
    let handle = NEXT.fetch_add(1, Ordering::SeqCst);
    match STREAMS.lock() {
        Ok(mut table) => {
            table.get_or_insert_with(HashMap::new).insert(handle, stream);
        }
        // SAFETY: as above.
        Err(_) => return unsafe { refuse(6, "the stream table is poisoned", out_err) },
    }
    // SAFETY: the caller promises a writable destination.
    unsafe { out_ok.write(handle) };
    BURI_OK
}

/// `Tcp::tcpRead(handle, limit) -> Result<[U8], IoError>`.
///
/// Waits for at least one byte and answers at most `limit`. The empty list is
/// the far side closing cleanly, which is the one answer that is neither bytes
/// nor an error. A `limit` at or below zero answers the empty list without
/// waiting, so a caller cannot ask this to block for nothing.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_tcp_read(
    handle: i64,
    limit: i64,
    out_ok: *mut BuriList,
    out_err: *mut BuriStr,
) -> i32 {
    if limit <= 0 {
        // SAFETY: the caller promises a writable destination.
        unsafe { out_ok.write(list_of_bytes(&[])) };
        return BURI_OK;
    }
    let want = usize::try_from(limit).unwrap_or(usize::MAX).min(1 << 20);
    // `host::about_to_block`'s rule: this waits for at least one byte, and a
    // client that printed what it asked for prints it before it waits.
    crate::host::about_to_block();
    let read = with_stream(handle, |stream| {
        let mut buffer = vec![0_u8; want];
        stream.read(&mut buffer).map(|n| {
            buffer.truncate(n);
            buffer
        })
    });
    match read {
        Some(Ok(bytes)) => {
            let value = list_of_bytes(&bytes);
            // SAFETY: the caller promises a writable destination.
            unsafe { out_ok.write(value) };
            BURI_OK
        }
        // SAFETY: as above.
        Some(Err(e)) => unsafe { fail(&e, out_err) },
        // SAFETY: as above.
        None => unsafe { refuse(0, "", out_err) },
    }
}

/// `Tcp::tcpWrite(handle, body) -> Result<(), IoError>`.
///
/// `write_all`, so there is no short write: `.Ok` means every byte went. `.Ok`'s
/// payload is `()` and so has no out-pointer at all (`lib.rs` §2.1's zero-sized
/// rule).
///
/// # Safety
/// `ptr`/`len` must be a readable range or null with a zero length; `out_err`
/// writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_tcp_write(
    handle: i64,
    ptr: *const u8,
    len: u64,
    out_err: *mut BuriStr,
) -> i32 {
    let body: &[u8] = if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller promises `len` readable bytes; a `[U8]`'s stride is
        // one, so the payload is the octets themselves.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    };
    match with_stream(handle, |stream| stream.write_all(body).and_then(|()| stream.flush())) {
        Some(Ok(())) => BURI_OK,
        // SAFETY: the caller promises a writable destination.
        Some(Err(e)) => unsafe { fail(&e, out_err) },
        // SAFETY: as above.
        None => unsafe { refuse(0, "", out_err) },
    }
}

/// `Tcp::tcpClose(handle) -> ()`.
///
/// Dropping the `TcpStream` closes the descriptor. A handle this table does not
/// hold is one already closed, and closing one twice is nothing happening the
/// second time — which is what `core/effect` promises and why this returns
/// nothing at all.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_tcp_close(handle: i64) {
    let Ok(mut table) = STREAMS.lock() else { return };
    if let Some(map) = table.as_mut() {
        map.remove(&handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A loopback peer that echoes what it is sent, once, and then closes.
    fn echo_once() -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("the bound address").port();
        let thread = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().expect("a connection");
            let mut buffer = [0_u8; 64];
            let n = peer.read(&mut buffer).expect("a request");
            peer.write_all(&buffer[..n]).expect("an answer");
        });
        (port, thread)
    }

    /// A `BuriStr` for the host, and the two out-pointers a connect writes.
    fn dial(host: &str, port: i64) -> (i32, i64, String) {
        let mut handle = 0_i64;
        let mut message = str_of("");
        let tag = unsafe {
            buri_rt_host_tcp_connect(
                std::ptr::null_mut(),
                host.as_ptr(),
                host.len() as u64,
                port,
                &raw mut handle,
                &raw mut message,
            )
        };
        (tag, handle, unsafe { message.as_str() }.into_owned())
    }

    #[test]
    fn a_dial_writes_reads_and_closes() {
        let (port, thread) = echo_once();
        let (tag, handle, message) = dial("127.0.0.1", i64::from(port));
        assert_eq!(tag, BURI_OK, "the dial failed: {message}");

        let body = b"hello";
        let mut error = str_of("");
        let wrote = unsafe {
            buri_rt_host_tcp_write(handle, body.as_ptr(), body.len() as u64, &raw mut error)
        };
        assert_eq!(wrote, BURI_OK, "the write failed: {}", unsafe { error.as_str() }.into_owned());

        let mut got = list_of_bytes(&[]);
        let read =
            unsafe { buri_rt_host_tcp_read(handle, 64, &raw mut got, &raw mut error) };
        assert_eq!(read, BURI_OK, "the read failed: {}", unsafe { error.as_str() }.into_owned());
        // SAFETY: the entry answered a live `[U8]`.
        let answer = unsafe { std::slice::from_raw_parts(got.ptr, got.len as usize) };
        assert_eq!(answer, body);

        buri_rt_host_tcp_close(handle);
        thread.join().expect("the peer thread");
    }

    /// The signature failure beside the happy path: a handle nothing holds.
    #[test]
    fn a_handle_that_names_nothing_is_a_stream_already_closed() {
        let mut error = str_of("");
        let mut got = list_of_bytes(&[]);
        let read = unsafe { buri_rt_host_tcp_read(0, 8, &raw mut got, &raw mut error) };
        assert_eq!(read, 0, "a made-up handle read something");

        let body = b"x";
        let wrote = unsafe {
            buri_rt_host_tcp_write(0, body.as_ptr(), 1, &raw mut error)
        };
        assert_eq!(wrote, 0, "a made-up handle was written to");

        // And closing one is nothing happening rather than a failure.
        buri_rt_host_tcp_close(0);
    }

    /// A closed stream is closed: the handle is not reused, and reading it back
    /// is the same refusal a made-up one gets.
    #[test]
    fn a_closed_handle_never_comes_back() {
        let (port, thread) = echo_once();
        let (tag, first, _) = dial("127.0.0.1", i64::from(port));
        assert_eq!(tag, BURI_OK);
        buri_rt_host_tcp_close(first);

        let mut error = str_of("");
        let mut got = list_of_bytes(&[]);
        let read = unsafe { buri_rt_host_tcp_read(first, 8, &raw mut got, &raw mut error) };
        assert_eq!(read, 0, "a closed handle still reads");

        let (_, second, _) = dial("127.0.0.1", i64::from(port));
        assert_ne!(second, first, "a spent handle came back");
        buri_rt_host_tcp_close(second);
        // The peer is waiting on a read that will never come; closing both ends
        // is what lets it finish.
        drop(thread);
    }

    #[test]
    fn a_port_that_is_not_one_is_refused_by_number() {
        let (tag, _, message) = dial("127.0.0.1", 70000);
        assert_eq!(tag, 6, "70000 was accepted as a port");
        assert!(message.contains("70000"), "{message} does not name the port");
    }

    /// Nothing is listening on a port nothing bound, so the dial is refused
    /// rather than hanging.
    #[test]
    fn a_dial_nobody_answers_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("the bound address").port();
        drop(listener);
        let (tag, _, _) = dial("127.0.0.1", i64::from(port));
        assert_eq!(tag, 1, "a dial to a closed port was not refused");
    }

    /// A read of nothing answers nothing, and does not wait to do it.
    #[test]
    fn a_limit_of_zero_answers_the_empty_list() {
        let mut error = str_of("");
        let mut got = list_of_bytes(&[1]);
        let read = unsafe { buri_rt_host_tcp_read(0, 0, &raw mut got, &raw mut error) };
        assert_eq!(read, BURI_OK);
        assert_eq!(got.len, 0);
    }
}
