//! The native runtime, driven the way a generated program drives it.
//!
//! What matters here is the **C ABI**: whether a caller that knows only
//! `cli/runtime/lib.rs`'s contract gets the answers the contract promises. A
//! `#[test]` inside the runtime cannot answer that — it agrees with the runtime
//! about Rust's own layout by construction, which is the thing under test.
//!
//! `cli/runtime` *is* a cargo package (BUILD-AND-WATCH.md §2.2), and it does
//! have unit tests of its own — for the float formatter, the UTF-16
//! comparison, the handle table, and since `https://` for TLS, all of which are
//! questions about Rust code rather than about a C ABI. Nothing ran them until
//! `the_runtime_crate_answers_its_own_tests` at the bottom of this file, which
//! is the second seam and the reason both are named.
//!
//! So the suite compiles a C driver against the embedded archive with `cc` and
//! runs it. `cc` is not a new requirement: the link step already drives the
//! platform C compiler (CODEGEN-STENCIL.md §12), so a machine that can build
//! a Buri artifact can build this driver.
//!
//! The four things it proves:
//!
//! 1. **The archive links.** A `buri_rt_*` symbol that is missing, or that has
//!    the wrong arity, is a link error here rather than a miscompile later.
//! 2. **The header is the header.** `rc`, `cap`, 16-byte alignment, the
//!    `IMMORTAL` sentinel, drop-glue dispatch, and a heap that returns to its
//!    starting size (MEMORY.md §2's leak property, on a corpus of one).
//! 3. **The abort messages match JavaScript byte for byte**, which
//!    `cli/tests/crash/` pins for the JavaScript backend and nothing pinned for
//!    the native one until now (VALUE-MODEL.md §12 row 14).
//! 4. **The host capabilities work**, including the byte forms, the write
//!    ordering between the buffered text stream and `writeBytes`, and the
//!    append/rename/sync sequence a write-ahead log commits through.
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE, h3, net};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const DRIVER: &str = include_str!("driver.c");

/// A per-run directory under `CARGO_TARGET_TMPDIR`, so nothing is written
/// inside a checked-in tree.
///
/// Named by the process id, because the driver below is built once and then
/// executed by every test in this file: two `cargo test` runs in two shells
/// sharing the path would have one `cc` overwriting the binary the other is
/// executing, which on macOS is a child that never returns rather than an
/// error.
fn workspace() -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("runtime-native-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build the driver once, and hand every test the path to it.
///
/// `OnceLock` rather than a per-test build: `cc` on the driver plus a 6 MB
/// archive is about a second, and paying it once for the suite is the
/// difference between a test file that is worth running and one that is not.
fn driver() -> &'static Path {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = workspace();
        let archive = dir.join(ARCHIVE_NAME);
        let source = dir.join("driver.c");
        let binary = dir.join("driver");
        std::fs::write(&archive, ARCHIVE).unwrap();
        std::fs::write(&source, DRIVER).unwrap();

        // `build/link.rs`'s own driver and trailing arguments
        // (`shared::product_cc`). The archive this driver.c links against is
        // the one `cli/build.rs` built, and on Linux that is a musl archive —
        // so the old `-lpthread -ldl -lm` is not merely stale here, it is a
        // link against the wrong libc.
        let mut cc = crate::shared::product_cc();
        cc.arg("-std=c11").arg("-O1").arg("-o").arg(&binary).arg(&source).arg(&archive);
        cc.args(crate::shared::product_link_args());
        let out = cc.output().unwrap();
        assert!(
            out.status.success(),
            "cc failed to link the driver against {ARCHIVE_NAME}:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        binary
    })
}

fn run(args: &[&str]) -> Output {
    run_with(args, &[], "")
}

fn run_with(args: &[&str], env: &[(&str, &str)], stdin: &str) -> Output {
    let mut cmd = Command::new(driver());
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    {
        use std::io::Write;
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(stdin.as_bytes()).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Every host we build a runtime for runs these; the rest have nothing to run.
fn skip() -> bool {
    if !AVAILABLE {
        return crate::ci::skipped(
            "runtime_native",
            "no runtime archive was built for this host, so there is nothing to link a driver \
             against",
        );
    }
    false
}

// ---------------------------------------------------------------------------

/// The header, the counts, the drop glue, and the leak property.
#[test]
fn the_memory_contract_holds() {
    if skip() {
        return;
    }
    let out = run(&["memory"]);
    assert_eq!(
        stdout(&out).trim_end(),
        "rc=1 cap=100 aligned=1 \
         after-incref=2 after-decref=1 \
         dropped=1 freed=1 \
         immortal-survives=1 \
         realloc-keeps-rc=1 realloc-cap=200 \
         leaked=0",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `Str`'s ASCII flag and the scalar count it stands in for
/// (VALUE-MODEL.md §3.1), and `[T]` construction.
#[test]
fn the_value_contract_holds() {
    if skip() {
        return;
    }
    let out = run(&["values"]);
    assert_eq!(
        stdout(&out).trim_end(),
        "ascii bytes=5 flag=1 scalars=5 \
         utf8 bytes=6 flag=0 scalars=5 \
         empty bytes=0 flag=1 \
         list len=4 cap=32 \
         divmod 142857 1 -142857 -1 \
         udivmod-high 6148914691236517205 21 1",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// The rendering surface — `cli/runtime/fmt.rs` and `hash.rs`.
///
/// Every string and every number here is what the JavaScript runtime produces
/// for the same input, which VALUE-MODEL.md §12 asks for and which this test is
/// what pins natively. The float corpus lives next door in
/// `native/float_parity.rs`, where four million values are checked against a
/// JavaScript engine; these are the ones a reader would look up.
#[test]
fn the_rendering_contract_matches_javascript() {
    if skip() {
        return;
    }
    let out = run(&["render"]);
    assert_eq!(
        stdout(&out).trim_end(),
        concat!(
            // `$f64`'s four rows: the default arm, the integral one with its
            // `.0`, the sign `Object.is(n, -0)` puts back, and the `1e21` cut
            // above which the value is already exponential.
            "f64 0.1\n",
            "int 1.0\n",
            "negzero -0.0\n",
            "big 1e+21\n",
            "denormal 5e-324\n",
            // An `F32` is a double on JavaScript, so it renders as the double
            // it widens to and not as the shortest `f32`.
            "f32 0.10000000149011612\n",
            "i128 -1\n",
            "u128 18446744073709551616\n",
            // `$show`'s `\"c\"` arm quotes and its `\"s\"` arm is
            // `JSON.stringify`; `$str` does neither.
            "char 'a'\n",
            "charstr a\n",
            "quoted \"a\\\"b\\n\"\n",
            "fromint -42\n",
            // `$hash(7)`, `$hash(\"ab\")`, `$hash('a')` and `$hash(NaN)`.
            "hash-int 34363494\n",
            "hash-str 1294271946\n",
            "hash-char 3826002220\n",
            "hash-nan 84696351",
        ),
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `core/str` at the C boundary — `cli/runtime/text.rs`.
///
/// Each line is a rule that would be easy to get subtly wrong, and each is
/// `backend/js/runtime.js`'s answer: scalar indices instead of byte offsets,
/// views instead of copies, JavaScript's whitespace set instead of Unicode's,
/// and the `Option` discriminant of `cli/runtime/lib.rs` §2 rule 3 — `-1`
/// present, `0` absent.
///
/// "JavaScript's answer" is not the same as "what a JavaScript operator does",
/// and `compare` is where the two came apart. It used to be UTF-16 order here
/// because `$str_compare` was JavaScript's `<`; the order is the language's
/// rather than the host's, `$str_compare` spells the scalar order out, and this
/// side is a plain byte comparison. buri-lang/buri#35.
#[test]
fn the_string_surface_matches_javascript() {
    if skip() {
        return;
    }
    let out = run(&["text"]);
    assert_eq!(
        stdout(&out).trim_end(),
        concat!(
            "len 3\n",
            "slice é\n",
            // Scalar 2 of \"aé漢\" is U+6F22.
            "charat -1 28450\n",
            "charat-past 0\n",
            // U+FEFF is JavaScript whitespace and is not Unicode `White_Space`.
            "trim [x]\n",
            "starts 1 ends 1 contains 1\n",
            // A *scalar* index, so the two-byte and three-byte prefixes count
            // as one each.
            "indexof -1 3\n",
            "indexof-none 0\n",
            // The empty first half is what a null `ptr` would misreport as
            // `.None`, which is why the runtime's empty string has an address.
            "splitonce -1 [][b]\n",
            "splitonce-none 0\n",
            // `Less = 0`, `Equal = 1`, `Greater = 2`. The third pair is the one
            // that discriminates the candidate orders: it answered `2` while
            // this compared UTF-16 code units, where a surrogate pair sorts
            // below U+FFFD, and answers `0` now that it compares scalar values
            // — which for valid UTF-8 is the bytes. buri-lang/buri#35.
            "compare 0 1 0\n",
            "eq 1 0\n",
            "toint -1 42\n",
            "toint-wide -1 9223372036854775807\n",
            "toint-past 0\n",
            "tofloat -1 15\n",
            "tofloat-bad 0\n",
            "split 3 a b c\n",
            "join a-b-c\n",
            "lines 3\n",
            "splitany 3\n",
            "replace baNANA\n",
            "repeat ababab\n",
            "repeat-none []\n",
            "upper AÉ\n",
            "lower aé\n",
            "padstart 007\n",
            "padend 700\n",
            "chars 2 97 233\n",
            "fromchars aé",
        ),
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `core/list`'s block-copying half — `cli/runtime/list.rs`.
///
/// Including the retain glue, which is the part with no counterpart on the
/// JavaScript side at all: a copied `[Str]` has to take a count on every string
/// block it now names, and `repeat 3 3` is three elements and three retains.
#[test]
fn the_list_surface_copies_and_retains() {
    if skip() {
        return;
    }
    let out = run(&["list"]);
    assert_eq!(
        stdout(&out).trim_end(),
        concat!(
            "get -1 30\n",
            "get-past 0\n",
            "get-negative 0\n",
            "concat 4 10 40\n",
            "push 5 99\n",
            "reverse 40 10\n",
            // Clamped at both ends rather than aborting, as `xs.slice(a, b)` is.
            "slice 4\n",
            "repeat 3 3\n",
            "range 3 4\n",
            // An empty `[T]` allocates nothing, which is what makes
            // `list.empty` free.
            "range-empty 0 1",
        ),
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// The four messages `cli/tests/crash/` pins, byte for byte against
/// `runtime.js`, plus the exit status `generate.rs:336` gives them.
///
/// A change to any of these on either backend breaks the other's corpus, which
/// is the point: VALUE-MODEL.md §12 row 14 says the message and the status must
/// agree, and this is the native half of the agreement.
#[test]
fn abort_messages_match_the_javascript_backend() {
    if skip() {
        return;
    }
    for (mode, message) in [
        ("abort-div", "division by zero"),
        ("abort-shift", "shift out of range"),
        ("abort-random", "random range is empty"),
        ("abort-entropy-count", "entropy count is negative"),
    ] {
        let out = run(&[mode]);
        assert_eq!(stderr(&out), format!("{message}\n"), "mode {mode}");
        assert_eq!(out.status.code(), Some(1), "mode {mode} must exit 1");
    }
}

/// The aborts the JavaScript backend has no counterpart for. Nothing pins the
/// wording, so this is what pins it.
#[test]
fn the_unpinned_aborts_say_what_they_mean() {
    if skip() {
        return;
    }
    let out = run(&["abort-bounds"]);
    assert_eq!(stderr(&out), "index out of bounds: the length is 3 but the index is 7\n");
    assert_eq!(out.status.code(), Some(1));

    let out = run(&["abort-budget"]);
    assert_eq!(
        stderr(&out),
        "allocation budget exhausted: 4096 bytes requested against a budget of 1024\n"
    );
    assert_eq!(out.status.code(), Some(1));
}

/// Buffered output is flushed before an abort, so the last thing a program
/// printed is above the reason it stopped — `generate.rs:337`'s ordering.
#[test]
fn an_abort_flushes_what_was_printed() {
    if skip() {
        return;
    }
    let out = run(&["abort-after-print"]);
    assert_eq!(stdout(&out), "printed before the abort\n");
    assert_eq!(stderr(&out), "division by zero\n");
    assert_eq!(out.status.code(), Some(1));
}

/// The text stream, the byte stream, and the ordering between them.
///
/// `writeBytes` flushes the text buffer first, for the same reason
/// `$host_HostStdout_writeBytes` does: the two orderings a program can see are
/// the one it wrote.
#[test]
fn the_streams_interleave_as_written() {
    if skip() {
        return;
    }
    let out = run(&["streams"]);
    assert_eq!(stdout(&out), "one two\nthree\nfour");
    assert_eq!(stderr(&out), "err one\nerr two");
    assert!(out.status.success());
}

/// `Fs`, end to end, including the two error shapes a program can match on.
#[test]
fn the_filesystem_effect_works() {
    if skip() {
        return;
    }
    let dir = workspace().join("fs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = run(&["fs", dir.to_str().unwrap()]);
    assert_eq!(
        stdout(&out).trim_end(),
        "write=ok exists=1 read=hello utf8=héllo \
         readdir=2 missing=0 notdir=4 exists-missing=0",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// The write-ahead log the seven new `Fs` operations exist for, against a real
/// filesystem.
///
/// `conformance/lib/semantics/test/effects.buri` runs the same sequence against
/// the `fs()` double on both backends. Two implementations of one story is the whole
/// argument for a fake: a divergence is a failure in one of them rather than a
/// difference between two sets of assertions.
///
/// It ends on `removeDir` (buri-lang/buri#38), which is where the story could
/// not end before: the mode's own `mkdir` had no inverse, so the scratch
/// directory it opened with stayed. `rmdir-held=6` is `IoError`'s `.Other` and
/// `rmdir-said=1` is that it came with a sentence — `lib.rs` §2.1's message,
/// read back through the C boundary rather than through a program, which is the
/// only tier that can see the out-pointer at all.
#[test]
fn a_write_ahead_log_commits_through_append_sync_and_rename() {
    if skip() {
        return;
    }
    let dir = workspace().join("wal");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = run(&["wal", dir.to_str().unwrap()]);
    assert_eq!(
        stdout(&out).trim_end(),
        "mkdir=ok,ok append=ok,ok sync=ok,ok log=1.10.2.20 \
         write=ok synctmp=ok rename=ok syncdir=ok tmp-gone=1 checkpoint=30 \
         remove=ok remove-again=0 sync-missing=0 \
         rmdir-held=6 rmdir-said=1 drop=ok rmdir=ok root-gone=1",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `Env`, both halves — and the argument vector the entry point hands over.
#[test]
fn the_environment_effect_works() {
    if skip() {
        return;
    }
    let out = run_with(&["env", "alpha", "beta"], &[("BURI_RT_TEST", "set")], "");
    assert_eq!(
        stdout(&out).trim_end(),
        "var=set missing=none args=3:env,alpha,beta",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `Clock` and `Rand`. Neither has a fixed answer, so what is asserted is the
/// range each one promises.
#[test]
fn the_clock_and_random_effects_work() {
    if skip() {
        return;
    }
    let out = run(&["clock-rand"]);
    assert_eq!(
        stdout(&out).trim_end(),
        "now-after-2020=1 slept=1 int-in-range=1000 float-in-range=1000 varies=1",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `Entropy`. Its answer is the one thing in this file that cannot be written
/// down — that is what the effect is *for* — so what is pinned is everything
/// around it: the empty request, the length, that the buffer was written at
/// all, that two draws differ, and that a request past 65536 octets is filled
/// to its end rather than to whatever one call of the platform's generator
/// would give.
#[test]
fn the_entropy_effect_answers_octets_nobody_wrote() {
    if skip() {
        return;
    }
    let out = run(&["entropy"]);
    assert_eq!(
        stdout(&out).trim_end(),
        "empty=0 len=64 differ=1 nonzero=1 big=70000 tail=1",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// `Stdin`, both forms: lines to end of input, and exactly `n` octets.
#[test]
fn the_standard_input_effect_works() {
    if skip() {
        return;
    }
    let out = run_with(&["stdin-lines"], &[], "alpha\nbeta\n");
    assert_eq!(stdout(&out).trim_end(), "line=alpha line=beta end", "stderr:\n{}", stderr(&out));

    let out = run_with(&["stdin-bytes"], &[], "abcdef");
    assert_eq!(stdout(&out).trim_end(), "got=4:abcd then=2:ef then=none");

    let out = run_with(&["stdin-lines"], &[], "");
    assert_eq!(stdout(&out).trim_end(), "end");
}

/// `Proc::exitWith` flushes and does not return.
#[test]
fn the_process_effect_exits() {
    if skip() {
        return;
    }
    let out = run(&["exit"]);
    assert_eq!(stdout(&out), "buffered, and flushed by the exit\n");
    assert_eq!(out.status.code(), Some(7));
}

/// `Net::fetch` against a socket this test owns.
///
/// A real HTTP server rather than a mock: the point of the client is that it
/// speaks the protocol to something that did not come out of the same file.
///
/// **`http://`, and it is unchanged by TLS landing** — which is the assertion
/// this test now carries as well as its own. `http.rs` wraps the socket for
/// `https://` and changes nothing else, so the request line, the four headers
/// the client writes for itself, the caller's own header, the `Content-Length`
/// an octet body earns and the dechunked response below are the same bytes they
/// were before there was a TLS client at all.
#[test]
fn the_network_effect_fetches() {
    if skip() {
        return;
    }
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        // Every wait this thread does is bounded, because the test joins it: a
        // driver that never connects, or that connects and stops talking, has
        // to become a failing assertion here rather than a suite that runs
        // until CI kills it. `cli/runtime/tls.rs`'s own server was the same
        // shape and was the hang that made the point.
        let patience = std::time::Duration::from_secs(60);
        let deadline = std::time::Instant::now() + patience;
        listener.set_nonblocking(true).unwrap();
        let mut sock = loop {
            match listener.accept() {
                Ok((sock, _)) => break sock,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the driver did not connect within {patience:?}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => panic!("the probe listener could not accept: {e}"),
            }
        };
        sock.set_nonblocking(false).unwrap();
        sock.set_read_timeout(Some(patience)).unwrap();
        sock.set_write_timeout(Some(patience)).unwrap();
        let mut request = Vec::new();
        let mut buf = [0_u8; 1024];
        // Read until the headers are complete *and* the four-octet body the
        // driver sends after them has arrived.
        loop {
            let n = sock.read(&mut buf).unwrap();
            request.extend_from_slice(&buf[..n]);
            let end = request.windows(4).position(|w| w == b"\r\n\r\n");
            if n == 0 || end.is_some_and(|at| request.len() >= at + 4 + 4) {
                break;
            }
        }
        let response = concat!(
            "HTTP/1.1 201 Created\r\n",
            "Content-Type: text/plain\r\n",
            "Transfer-Encoding: chunked\r\n",
            "\r\n",
            // Two chunks, so the client's dechunker is what assembled the body.
            "5\r\nhello\r\n",
            "4\r\n you\r\n",
            "0\r\n\r\n",
        );
        sock.write_all(response.as_bytes()).unwrap();
        sock.flush().unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });

    let out = run(&["net", &format!("http://127.0.0.1:{port}/probe")]);
    let request = server.join().unwrap();
    // `.Get` is variant 0 and the driver sends the index; `GET` is written in
    // `http.rs`'s `METHODS` and nowhere else, so this line is the assertion
    // that the index reached the request line.
    assert!(request.starts_with("GET /probe HTTP/1.1\r\n"), "request was:\n{request}");
    assert!(request.contains(&format!("Host: 127.0.0.1:{port}\r\n")), "request was:\n{request}");
    // The caller's own header, and the `Content-Length` its octet body earns.
    assert!(request.contains("x-probe: buri\r\n"), "request was:\n{request}");
    assert!(request.contains("Content-Length: 4\r\n"), "request was:\n{request}");
    assert!(request.ends_with("\u{1f44b}"), "request was:\n{request}");
    // Two header fields come back, lowercased, and the body is the two chunks
    // joined — the response's three fields, all three read back.
    assert_eq!(
        stdout(&out).trim_end(),
        "status=201 headers=2 body=hello you content-type=text/plain \
         transfer-encoding=chunked",
        "stderr:\n{}",
        stderr(&out)
    );
    assert!(out.status.success());
}

/// What `https://` does through the C ABI, in both of the runtime's feature
/// states, and the refusal that is not about TLS at all.
///
/// The *contents* of the TLS story — a real handshake against a real server, a
/// certificate refused for an unknown issuer, one refused for the wrong name —
/// are in the runtime crate's own tests, because a TLS server has to come from
/// somewhere and the only `rustls` in this repository is inside the archive.
/// `the_runtime_crate_answers_its_own_tests` below is what runs them. What
/// *this* test is for is the seam: that the scheme is no longer refused by name
/// on a `net` toolchain, and that it is refused with the right sentence on one
/// without.
///
/// The probe is a **closed port on the loopback interface**, so it is offline
/// and it is deterministic: with TLS compiled in, `https://` gets as far as the
/// socket and answers `Refused`; without it, the scheme never reaches a socket
/// at all.
#[test]
fn the_network_effect_answers_https_according_to_its_features() {
    if skip() {
        return;
    }
    // A port nothing is listening on: bound, read back, and dropped.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let out = run(&["net", &format!("https://127.0.0.1:{port}/probe")]);
    if net() {
        assert_eq!(
            stdout(&out).trim_end(),
            "err=1 message=",
            "`https://` reached the socket and found nothing there, which is `Refused`; \
             stderr:\n{}",
            stderr(&out)
        );
    } else {
        assert_eq!(
            stdout(&out).trim_end(),
            "err=3 message=https is not supported by this toolchain's native runtime: it was \
             built without the runtime's `net` feature, so it carries no TLS code. `Net.fetch` \
             speaks cleartext http only"
        );
    }

    let out = run(&["net", "not-a-url"]);
    assert_eq!(stdout(&out).trim_end(), "err=2 message=not an absolute http URL: not-a-url");
}

/// **The `net-h3` flag round-trips**, across the C ABI and back.
///
/// There are two independent paths from "which features did Cargo build this
/// archive with" to an answer, and this is the assertion that they agree:
///
/// * the **archive's**, which is `#[cfg(feature = "net-h3")]` folded into
///   `net.rs`'s `LINKED` constant and exported as `buri_rt_net_h3_available`,
///   read here by linking the archive and calling it; and
/// * the **toolchain's**, which is `cli/build.rs` writing `net-h3` into
///   `libburi_rt.a.features` and `runtime_native::h3()` reading that line back.
///
/// Neither can see the other. A build script that wrote the wrong feature line,
/// a `--features` flag that did not reach the nested `cargo`, or a stale
/// `OUT_DIR` pairing one build's bytes with another's answer all show up here
/// as a disagreement, and nowhere else.
///
/// The same holds for `net` beside it, which is why both are checked: `net-h3`
/// contains `net` as a substring, so a features file read by anything other
/// than whole lines would report an h3-only archive as a networking one and
/// this is where that would surface.
#[test]
fn the_networking_features_agree_across_the_abi() {
    if skip() {
        return;
    }
    let out = run(&["net-features"]);
    let line = stdout(&out);
    let line = line.trim_end();
    let bit = |name: &str| -> i64 {
        line.split_whitespace()
            .find_map(|field| field.strip_prefix(name))
            .unwrap_or_else(|| panic!("the driver printed no `{name}` field: {line}"))
            .parse()
            .unwrap_or_else(|e| panic!("`{name}` is not a number in {line}: {e}"))
    };
    assert_eq!(bit("net=") == 1, net(), "the archive and its feature file disagree about `net`");
    assert_eq!(bit("h3=") == 1, h3(), "the archive and its feature file disagree about `net-h3`");
    // The capability mask is the same fact a third way: bit 4 is `net-h3`, and
    // it is above the four `net` ones so that neither can be read as the other.
    let caps = bit("caps=");
    assert_eq!(caps & (1 << 4) != 0, h3(), "the capability mask disagrees with the h3 door");
    assert_eq!(caps != 0, net(), "the capability mask disagrees with the `net` door");
    // And the implication the manifest states: `net-h3 = ["net", "dep:quinn"]`.
    assert!(!h3() || net(), "this archive claims QUIC without a networking stack under it");
    assert!(out.status.success());
}

/// The runtime crate's own unit tests — including the `https://` exchange
/// against a locally-served `rustls` endpoint — run.
///
/// **They were written and never run.** `cli/runtime` is a cargo package that
/// is deliberately not a workspace member and whose manifest is deliberately
/// not called `Cargo.toml` (`cli/build.rs`'s header says why), so
/// `cargo test -p buri` cannot reach inside it and no CI step did either. Fifty
/// assertions about the float formatter, the UTF-16 comparison, the handle
/// table and the allocator were dead. This is the seam that runs them, and the
/// path comes from the build script — `BURI_RT_PKG` — because the package is
/// *assembled* in `OUT_DIR` and only the script knows where.
///
/// # Two ways to run them, and this test is both
///
/// The nested `cargo` below cold-compiles tokio, hyper and rustls the first
/// time it is asked, which is a minute of `cc` and `rustc` **inside one test**
/// — invisible in a test report, and invisible to every cache CI has, because
/// the target directory is under `CARGO_TARGET_TMPDIR` and `harness/sweep.rs`
/// collects it. On a laptop that is a fair trade: it is paid once per checkout
/// and it needs no workflow to have been written. On a runner it was sixty
/// seconds a leg of unattributable time.
///
/// So on a runner the *step* runs them —
/// `.github/scripts/test-runtime-crate.sh`, which is the same `cargo test` with
/// a cache key and a line in the run summary — and this test asserts the step
/// ran. Not that it is configured: that the stamp it writes on success is on
/// disk. Delete the step from a job and this fails; leave `BURI_CI=1` set with
/// no step and this fails; and nothing about it can pass while the runtime's
/// tests have not run.
///
/// **No platform loses coverage.** Off a runner (`BURI_CI` unset) the nested
/// `cargo` runs exactly as it always did, on every host that has a runtime —
/// which is every host `AVAILABLE` is true on.
///
/// The nested `cargo` gets the same treatment `cli/build.rs`'s does and for the
/// same reasons: every `CARGO_*` but `CARGO_HOME` removed, so the outer
/// invocation's target-directory lock and jobserver are not inherited, and an
/// emptied `RUSTFLAGS`. `--offline`, `--no-default-features` and
/// `--features net-h3` mirror whatever the archive beside this binary was
/// actually built with, so this runs the tests of *this* runtime rather than of
/// a differently-featured one. All three states are reachable from the outside:
/// the default, `BURI_RUNTIME_NET=0`, and `BURI_RUNTIME_NET_H3=1`. The script
/// makes the same two decisions from the same two files.
#[test]
fn the_runtime_crate_answers_its_own_tests() {
    if skip() {
        return;
    }
    if crate::ci::on() {
        the_ci_step_ran_them();
        return;
    }
    let pkg = Path::new(env!("BURI_RT_PKG"));
    assert!(
        pkg.join("Cargo.toml").is_file(),
        "the build script reported the runtime package at {} and there is no manifest there, so \
         this test would have asserted nothing",
        pkg.display()
    );

    let mut cargo =
        Command::new(std::env::var("CARGO").unwrap_or_else(|_| String::from("cargo")));
    for (name, _) in std::env::vars() {
        if name.starts_with("CARGO_") && name != "CARGO_HOME" {
            cargo.env_remove(&name);
        }
    }
    cargo.env_remove("CARGO");
    cargo.env("RUSTFLAGS", "");
    cargo.arg("test").arg("--locked");
    if env!("BURI_RT_OFFLINE") == "1" {
        cargo.arg("--offline");
    }
    if !net() {
        cargo.arg("--no-default-features");
    }
    if h3() {
        cargo.args(["--features", "net-h3"]);
    }
    cargo.arg("--manifest-path").arg(pkg.join("Cargo.toml"));
    // Beside the other native scratch trees, and *not* named for the process:
    // the dependency tree is a minute of `cc` and `rustc` the first time, and
    // paying that once per checkout rather than once per run is the difference
    // between a test that is run and one that is skipped. `harness/sweep.rs`
    // collects it after two idle hours like everything else here.
    cargo.arg("--target-dir").arg(Path::new(env!("CARGO_TARGET_TMPDIR")).join("runtime-crate"));

    let out = cargo.output().expect("cargo could not be run for the runtime package");
    assert!(
        out.status.success(),
        "the runtime crate's own tests did not pass:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The runner's half: the hoisted step ran, and it passed.
///
/// `BURI_RT_TESTS_STAMP` is set in the workflow's `env:` block and the stamp is
/// written by `.github/scripts/test-runtime-crate.sh` only after its `cargo
/// test` exits zero, so its presence is the step's own report rather than a
/// claim about the workflow's text. Both halves of that are failures here: a
/// job with the variable and no step leaves no file, and a job with neither
/// leaves no variable.
///
/// The stamp's contents are read, not just its existence — a `mkdir` or a
/// truncated write would otherwise satisfy this — and what is checked is a
/// **digest, not a path**. A target directory can hold several assembled
/// packages (a restored cache's, a `--features` build's), only one of which is
/// the one this binary was compiled against, and the file paths do not say
/// which. `libburi_rt.a.sha256` does: the script records the digest of every
/// distinct runtime it tested, `archive_hash()` is the digest of the archive
/// baked into this binary, and the question "were the tests that ran the tests
/// of THIS runtime" is exactly whether the second is among the first.
fn the_ci_step_ran_them() {
    let stamp = std::env::var("BURI_RT_TESTS_STAMP").unwrap_or_else(|_| {
        panic!(
            "BURI_CI=1 and BURI_RT_TESTS_STAMP is unset. On a runner this test does not shell a \
             nested `cargo` — `.github/scripts/test-runtime-crate.sh` runs the runtime crate's \
             tests as a step, and this variable is how the step and this test agree on where the \
             step's stamp goes. Set it in the workflow's `env:` block, beside BURI_CI."
        )
    });
    let text = std::fs::read_to_string(&stamp).unwrap_or_else(|e| {
        panic!(
            "BURI_CI=1 and there is no stamp at {stamp} ({e}). The runtime crate's own tests are \
             run by `.github/scripts/test-runtime-crate.sh` on a runner, and that script writes \
             this file only after its `cargo test` exits zero — so either the step is missing \
             from this job or it has not run yet. Add it after the runtime-archive assertion and \
             before the suite."
        )
    });
    assert!(
        text.starts_with("ok\n"),
        "the stamp at {stamp} does not say the runtime crate's tests passed:\n{text}"
    );
    let tested: Vec<&str> =
        text.lines().filter_map(|line| line.strip_prefix("runtime: ")).collect();
    assert!(!tested.is_empty(), "the stamp at {stamp} names no runtime it tested:\n{text}");
    let ours = buri::compiler::backend::runtime_native::archive_hash();
    assert!(
        tested.iter().any(|digest| *digest == ours),
        "the step tested {} runtime(s) and none of them is the one in this binary. Tested:\n  \
         {}\nThis binary's `libburi_rt.a`: {ours}\n\nThe usual cause is a stale `$OUT_DIR` under \
         the target directory — a restored cache's, or a differently-featured build's — that the \
         script found and this binary was not compiled against. It tests every DISTINCT runtime \
         it finds, so a miss here means the one in this binary was not on disk when the step ran.",
        tested.len(),
        tested.join("\n  ")
    );
    eprintln!("runtime crate: tested by the CI step ({stamp})\n{text}");
}

// ---------------------------------------------------------------------------
// B9: the three machine-stack switches
// ---------------------------------------------------------------------------

/// The three hand-written blocks, by the `StencilTarget` each belongs to.
///
/// `include_str!` rather than a read at run time: the test is a statement
/// about the files this toolchain was **built** from, and a path read at run
/// time would pass against a working tree the archive does not contain.
const SWITCH_BLOCKS: [(&str, &str, &str); 3] = [
    ("macos-arm64", "arm64-apple-darwin", include_str!("../../runtime/switch_macos_arm64.s")),
    (
        "linux-arm64",
        "aarch64-unknown-linux-musl",
        include_str!("../../runtime/switch_linux_arm64.s"),
    ),
    (
        "linux-x86_64",
        "x86_64-unknown-linux-musl",
        include_str!("../../runtime/switch_linux_x86_64.s"),
    ),
];

/// **Every switch block assembles for the machine it is written for**, and the
/// object says which machine that was.
///
/// `cli/build.rs` builds the runtime archive for the **host triple alone**, so
/// two of the three blocks in `cli/runtime/` are never compiled by anything
/// this repository runs — which is exactly the shape of a file that rots. The
/// stencil library has the same problem and answers it the same way, by
/// building all three targets from one host with `cc --target=`; this is that
/// assertion for the switch.
///
/// Three things per block, and the second is the one that would catch a
/// copy-and-paste: it assembles, the object it produces is **for the right
/// machine** (read out of the container's own header rather than trusted), and
/// both of its symbols are in it under the spelling that platform uses.
///
/// A host whose `cc` cannot target one of the triples skips *that* triple with
/// a line on standard error, exactly as `sources::can_build` does — a Nix
/// cross-wrapper and a bare Linux box are both real, and neither is a reason
/// to fail a test about three files.
#[test]
fn the_three_switch_blocks_assemble_for_their_targets() {
    let dir = workspace().join("switch");
    std::fs::create_dir_all(&dir).unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let mut built = 0;

    for (slug, triple, source) in SWITCH_BLOCKS {
        let src = dir.join(format!("{slug}.s"));
        let obj = dir.join(format!("{slug}.o"));
        std::fs::write(&src, source).unwrap();
        let _ = std::fs::remove_file(&obj);

        let out = Command::new(&cc)
            .args(["-c", "-x", "assembler"])
            .arg(format!("--target={triple}"))
            .arg("-o")
            .arg(&obj)
            .arg(&src)
            .output()
            .unwrap_or_else(|e| panic!("could not run {cc}: {e}"));
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            // A clang with no back end for this triple is a host limitation
            // and not a broken block; anything else is the block.
            let unsupported = stderr.contains("unknown target")
                || stderr.contains("unsupported")
                || stderr.contains("No available targets");
            assert!(unsupported, "{slug} did not assemble for {triple}:\n{stderr}");
            eprintln!("switch: this cc cannot target {triple}, skipping {slug}");
            continue;
        }

        let bytes = std::fs::read(&obj).unwrap();
        assert!(!bytes.is_empty(), "{slug} assembled to an empty object");
        assert_eq!(
            container_machine(&bytes),
            Some(slug_machine(slug)),
            "{slug} produced an object for the wrong machine",
        );
        // The symbol table is plain text in both containers, and the spelling
        // is the platform's: Darwin puts a leading underscore on a C symbol
        // and ELF does not.
        let text = String::from_utf8_lossy(&bytes);
        let lead = if slug == "macos-arm64" { "_" } else { "" };
        for name in ["buri_rt_task_switch", "buri_rt_task_launch", "buri_rt_task_main"] {
            assert!(
                text.contains(&format!("{lead}{name}")),
                "{slug}'s object does not name {lead}{name}",
            );
        }
        built += 1;
    }

    assert!(built > 0, "not one of the three switch blocks could be assembled on this host");
}

/// What machine an object file says it is for: `(container, machine)`.
///
/// Read out of the two headers by hand, which is nine lines and no dependency.
/// ELF: `e_machine` at byte 18, little-endian. Mach-O 64: `cputype` at byte 4.
fn container_machine(bytes: &[u8]) -> Option<(&'static str, u32)> {
    if bytes.len() < 20 {
        return None;
    }
    if &bytes[..4] == b"\x7fELF" {
        return Some(("elf", u32::from(u16::from_le_bytes([bytes[18], bytes[19]]))));
    }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic == 0xfeed_facf {
        return Some(("macho", u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])));
    }
    None
}

/// The `(container, machine)` each block must produce.
fn slug_machine(slug: &str) -> (&'static str, u32) {
    match slug {
        // `CPU_TYPE_ARM64` = `CPU_TYPE_ARM | CPU_ARCH_ABI64`.
        "macos-arm64" => ("macho", 0x0100_000c),
        // `EM_AARCH64`.
        "linux-arm64" => ("elf", 183),
        // `EM_X86_64`.
        "linux-x86_64" => ("elf", 62),
        other => panic!("no machine for {other}"),
    }
}

/// **The two AArch64 blocks are one body.**
///
/// They are the same instructions under two object formats, and the whole of
/// the difference is meant to be the leading underscore and ELF's `.type` /
/// `.size` / `.note.GNU-stack` directives. A fix applied to one and not the
/// other is the failure this catches, and it is the one a reader of two nearly
/// identical files will not notice.
///
/// Compared as *instructions*: every line that is not blank, not a comment and
/// not a directive, with a Darwin label's underscore removed.
#[test]
fn the_two_aarch64_switch_blocks_are_one_body() {
    fn instructions(source: &str, lead: &str) -> Vec<String> {
        source
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('.'))
            .map(|l| {
                let l = l.trim();
                match l.strip_prefix(lead) {
                    Some(rest) if !lead.is_empty() && l.ends_with(':') => rest.to_string(),
                    _ => l.replace(&format!(" {lead}buri_rt"), " buri_rt"),
                }
            })
            .collect()
    }
    let darwin = instructions(SWITCH_BLOCKS[0].2, "_");
    let linux = instructions(SWITCH_BLOCKS[1].2, "");
    assert!(!darwin.is_empty(), "the darwin block has no instructions");
    assert_eq!(darwin, linux, "the two AArch64 switch blocks have drifted apart");
}
