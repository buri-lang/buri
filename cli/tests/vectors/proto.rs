//! The protobuf conformance suite, replayed without the protobuf conformance
//! suite.
//!
//! `cli/tests/proto/` holds a testee that speaks the runner's protocol, and
//! `run.sh` drives it with the real C++ `conformance_test_runner`. That tool is
//! not something `cargo test` can depend on — it is a C++ build of another
//! project — so this is the other half of the `vectors::lean` arrangement: the
//! external tool generates, a checked-in file replays, and the replay needs
//! nothing but a Buri toolchain and a JavaScript runtime.
//!
//! What it exercises is the whole pipeline and not a piece of it: the vendored
//! `.proto` schemas become modules, the generated codecs read the request and
//! write the response, and the framing is Buri too. A change anywhere in that
//! chain changes an answer here.
//!
//! ```text
//! BURI_KEEP=1 cargo test -p buri --test vectors proto::    # keep the scratch tree
//! ```
use crate::harness::*;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn proto_dir() -> PathBuf {
    tests_dir().join("proto")
}

/// `(request, response)` for each recorded exchange, bodies without the frame
/// length.
fn vectors() -> Vec<(Vec<u8>, Vec<u8>)> {
    let text = std::fs::read_to_string(proto_dir().join("vectors.txt"))
        .expect("cli/tests/proto/vectors.txt");
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (a, b) = line.split_once(' ').unwrap_or_else(|| panic!("malformed vector: {line}"));
        out.push((unhex(a), unhex(b)));
    }
    out
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex length");
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

fn frame(body: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
}

/// Splits a stream of length-prefixed frames.
fn unframe(mut b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while b.len() >= 4 {
        let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        if b.len() < 4 + n {
            break;
        }
        out.push(b[4..4 + n].to_vec());
        b = &b[4 + n..];
    }
    out
}

/// Every recorded request, answered the way it was answered when the reference
/// runner accepted the run.
///
/// One process for all of them, which is also what the runner does: the testee
/// is a loop over its own standard input, and running it once is the shape the
/// protocol has rather than a shortcut this test takes.
#[test]
fn the_recorded_exchanges_still_hold() {
    let vectors = vectors();
    assert!(
        vectors.len() > 100,
        "only {} vectors; the file is not being read",
        vectors.len()
    );

    let scratch = Scratch::copy_of("proto-vectors", &proto_dir().join("repo"));
    scratch.run(&["build", "//cmd/testee"]).ok();
    let artifact = scratch.path(".buri/out/js/cmd/testee/testee.mjs");
    assert!(artifact.is_file(), "the testee did not build");

    let mut stdin = Vec::new();
    for (request, _) in &vectors {
        frame(request, &mut stdin);
    }

    let runtime = js_runtime();
    let mut child = Command::new(&runtime)
        .arg(&artifact)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start `{runtime}`: {e}"));
    child.stdin.take().unwrap().write_all(&stdin).expect("writing the requests");
    let out = child.wait_with_output().expect("the testee did not finish");
    let got = unframe(&out.stdout);

    assert_eq!(
        got.len(),
        vectors.len(),
        "the testee answered {} of {} requests\nstderr:\n{}",
        got.len(),
        vectors.len(),
        indent(&String::from_utf8_lossy(&out.stderr))
    );

    let mut wrong = Vec::new();
    for (i, ((request, want), have)) in vectors.iter().zip(got.iter()).enumerate() {
        if want != have {
            wrong.push(format!(
                "vector {}:\n    request:  {}\n    recorded: {}\n    now:      {}",
                i + 1,
                hex(request),
                hex(want),
                hex(have)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} recorded exchanges changed:\n{}\n\nA change here is a change to what the \
         codecs answer. If it is intended, re-record with cli/tests/proto/run.sh --record and \
         re-run the conformance suite to confirm the new answers are still conformant.",
        wrong.len(),
        vectors.len(),
        wrong.join("\n")
    );
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The runtime the toolchain's own artifacts run on.
fn js_runtime() -> String {
    for candidate in ["bun", "node"] {
        if Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return candidate.to_string();
        }
    }
    panic!("neither `bun` nor `node` is on PATH; the emitted artifact needs one to run")
}

/// The framing is the one thing in the testee that is not generated, so it is
/// the one thing worth asserting on its own: a length that does not match its
/// body would make every vector above fail for one reason and say nothing about
/// which.
#[test]
fn the_framing_is_four_little_endian_bytes_and_a_body() {
    let mut buf = Vec::new();
    frame(b"", &mut buf);
    frame(b"abc", &mut buf);
    frame(&vec![7u8; 300], &mut buf);
    assert_eq!(&buf[..4], &[0, 0, 0, 0]);
    assert_eq!(&buf[4..8], &[3, 0, 0, 0]);
    assert_eq!(&buf[11..15], &[44, 1, 0, 0], "300 is 0x012c, low byte first");
    let frames = unframe(&buf);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].len(), 0);
    assert_eq!(frames[1], b"abc");
    assert_eq!(frames[2].len(), 300);
    // A truncated trailing frame is dropped rather than half-read, which is
    // what end of input looks like when the runner stops asking.
    assert_eq!(unframe(&[3, 0, 0, 0, 1, 2]).len(), 0);
}
