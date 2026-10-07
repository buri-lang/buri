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

/// One recorded exchange: whether `failure_list.txt` expected the test to fail,
/// its name, and the request and response bodies without the frame length.
struct Vector {
    fails: bool,
    name: String,
    request: Vec<u8>,
    response: Vec<u8>,
}

fn vectors() -> Vec<Vector> {
    let text = std::fs::read_to_string(proto_dir().join("vectors.txt"))
        .expect("cli/tests/proto/vectors.txt");
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(' ').collect();
        let [verdict, name, request, response] = parts[..] else {
            panic!("malformed vector: {line}");
        };
        assert!(verdict == "pass" || verdict == "fail", "malformed verdict: {line}");
        out.push(Vector {
            fails: verdict == "fail",
            name: name.to_string(),
            request: unhex(request),
            response: unhex(response),
        });
    }
    out
}

/// The test names `failure_list.txt` expects to fail.
fn listed() -> Vec<String> {
    std::fs::read_to_string(proto_dir().join("failure_list.txt"))
        .expect("cli/tests/proto/failure_list.txt")
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
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
    replay(&Scratch::copy_of("proto-vectors", &proto_dir().join("repo")));
}

/// The same exchanges, after `buri format` has laid out both vendored schemas:
/// formatting moves whitespace and comments, and changes no answer.
#[test]
fn formatting_the_schemas_changes_no_answer() {
    let scratch = Scratch::copy_of("proto-vectors-formatted", &proto_dir().join("repo"));
    let schema = "lib/conformance/test_messages_proto3.proto";
    let before = scratch.read(schema);
    scratch.run(&["format"]).ok();
    assert_ne!(scratch.read(schema), before, "formatting left {schema} as it was");
    // A fixed point: a second run finds nothing to change.
    scratch.run(&["format", "--check"]).ok();
    replay(&scratch);
}

fn replay(scratch: &Scratch) {
    let vectors = vectors();
    assert!(
        vectors.len() > 100,
        "only {} vectors; the file is not being read",
        vectors.len()
    );

    scratch.run(&["build", "//cmd/testee"]).ok();
    let artifact = scratch.path(".buri/out/node/cmd/testee/testee.mjs");
    assert!(artifact.is_file(), "the testee did not build");

    let mut stdin = Vec::new();
    for v in &vectors {
        frame(&v.request, &mut stdin);
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
    for (v, have) in vectors.iter().zip(got.iter()) {
        if &v.response != have {
            wrong.push(format!(
                "{} ({}):\n    request:  {}\n    recorded: {}\n    now:      {}",
                v.name,
                if v.fails { "listed as failing" } else { "passed the runner" },
                hex(&v.request),
                hex(&v.response),
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

/// **protobuf's own runner passes, with `failure_list.txt` applied.**
///
/// The runner is the ground truth the recording comes from: it fails on a test
/// that fails unlisted and on a listed test that passes. It is a C++ binary
/// `nix build .#conformance-runner` makes, so this runs where
/// `CONFORMANCE_TEST_RUNNER` names it, which CI's `protobuf conformance` job
/// does.
#[test]
fn the_conformance_runner_passes() {
    let Some(runner) = std::env::var_os("CONFORMANCE_TEST_RUNNER") else {
        ci::deferred_to(
            "vectors::proto",
            "protobuf conformance",
            "CONFORMANCE_TEST_RUNNER names no runner here",
        );
        return;
    };
    let scratch = Scratch::copy_of("proto-runner", &proto_dir().join("repo"));
    scratch.run(&["build", "//cmd/testee"]).ok();
    let artifact = scratch.path(".buri/out/node/cmd/testee/testee.mjs");
    let testee = scratch.path("testee.sh");
    std::fs::write(
        &testee,
        format!("#!/bin/sh\nexec '{}' '{}'\n", js_runtime(), artifact.display()),
    )
    .expect("writing the testee's script");
    std::fs::set_permissions(&testee, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("making the testee's script executable");
    let out = Command::new(&runner)
        .arg("--failure_list")
        .arg(proto_dir().join("failure_list.txt"))
        .arg("--output_dir")
        .arg(scratch.path(""))
        .arg(&testee)
        .output()
        .unwrap_or_else(|e| panic!("cannot start the conformance runner: {e}"));
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: Vec<&str> = report.lines().filter(|l| l.contains("CONFORMANCE SUITE")).collect();
    println!("{}", summary.join("\n"));
    assert!(
        out.status.success() && !summary.is_empty() && summary.iter().all(|l| l.contains("PASSED")),
        "the conformance runner failed:\n{}",
        indent(&report.lines().filter(|l| l.starts_with("ERROR")).collect::<Vec<_>>().join("\n"))
    );
}

/// **`failure_list.txt` is the set of recorded failures, exactly.**
///
/// `run.sh --record` writes a verdict beside every exchange, and records only a
/// run the runner passed with the list applied. So a name listed here and not
/// recorded as failing, or recorded as failing and no longer listed, is a list
/// edited without the runner. The replay above holds the answers; this holds
/// the list.
#[test]
fn the_failure_list_is_the_recorded_failures() {
    let recorded: std::collections::BTreeSet<String> =
        vectors().into_iter().filter(|v| v.fails).map(|v| v.name).collect();
    let listed: std::collections::BTreeSet<String> = listed().into_iter().collect();
    let unrecorded: Vec<_> = listed.difference(&recorded).collect();
    let unlisted: Vec<_> = recorded.difference(&listed).collect();
    assert!(
        unrecorded.is_empty() && unlisted.is_empty(),
        "failure_list.txt and vectors.txt disagree. Run cli/tests/proto/run.sh, fix \
         the list until the runner passes, then re-record with --record.\n  \
         listed, not recorded as failing: {unrecorded:?}\n  \
         recorded as failing, not listed: {unlisted:?}"
    );
}

/// **Every class of bug the conformance runner has found is in the recording.**
///
/// The replay only guards what was recorded, so the recording must reach each
/// of them: a recording narrowed by accident would pass every other test here.
#[test]
fn the_recording_covers_every_fixed_class_of_bug() {
    let names: std::collections::BTreeSet<String> =
        vectors().into_iter().filter(|v| !v.fails).map(|v| v.name).collect();
    let wanted = [
        // NaN and the infinities, as strings.
        "Required.Proto3.JsonInput.DoubleFieldNan.JsonOutput",
        "Required.Proto3.JsonInput.FloatFieldInfinity.JsonOutput",
        "Required.Proto3.JsonInput.DoubleFieldNegativeInfinity.JsonOutput",
        "Required.Proto3.ProtobufInput.DoubleFieldNormalizeSignalingNan.JsonOutput",
        // int64 at and past both ends.
        "Required.Proto3.JsonInput.Int64FieldMaxValue.JsonOutput",
        "Required.Proto3.JsonInput.Int64FieldMinValue.JsonOutput",
        "Required.Proto3.JsonInput.Int64FieldTooLarge",
        "Required.Proto3.JsonInput.Int64FieldTooSmall",
        "Required.Proto3.ProtobufInput.ValidDataScalar.INT64[2].ProtobufOutput",
        "Required.Proto3.ProtobufInput.ValidDataScalar.SINT64[3].ProtobufOutput",
        // uint64 and fixed64 past 2^63.
        "Required.Proto3.JsonInput.Uint64FieldMaxValue.JsonOutput",
        "Required.Proto3.JsonInput.Uint64FieldMaxValueNotQuoted.JsonOutput",
        "Required.Proto3.JsonInput.Uint64FieldTooLarge",
        "Required.Proto3.ProtobufInput.ValidDataScalar.UINT64[2].JsonOutput",
        "Required.Proto3.ProtobufInput.ValidDataScalar.FIXED64[2].JsonOutput",
        // A oneof set twice, a leading zero, and proto3's presence.
        "Required.Proto3.JsonInput.OneofFieldDuplicate",
        "Required.Proto3.JsonInput.Int32FieldLeadingZero",
        "Required.Proto3.JsonInput.SkipsDefaultPrimitive.Validator",
    ];
    let missing: Vec<_> = wanted.iter().filter(|w| !names.contains(**w)).collect();
    assert!(missing.is_empty(), "vectors.txt does not record these passing: {missing:?}");
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
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
