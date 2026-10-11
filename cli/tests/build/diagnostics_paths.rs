//! How the toolchain reads and reports: messages from an editor in JSON.
use crate::harness::*;

/// Each message with the `Content-Length` framing the protocol uses.
fn framed(messages: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in messages {
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", m.len()).as_bytes());
        out.extend_from_slice(m.as_bytes());
    }
    out
}

/// The server's replies to requests for a method it doesn't have, each
/// naming the id it was sent back as the server read it.
fn echoed_ids(stdout: &str) -> Vec<String> {
    stdout
        .split("Content-Length")
        .filter(|m| m.contains("is not implemented"))
        .map(|m| {
            let at = m.find("\"id\":").unwrap() + 5;
            let rest = &m[at..];
            rest[..rest.find(",\"jsonrpc\"").unwrap()].to_string()
        })
        .collect()
}

/// An unpaired surrogate is replaced, and the escape after it is still read.
#[test]
fn an_escape_after_an_unpaired_surrogate_is_kept() {
    let scratch = Scratch::repo("diagnostics-json-surrogate");
    let run = scratch.run_with_stdin(
        &["lsp"],
        &framed(&[
            r#"{"id":0,"method":"initialize","params":{}}"#,
            r#"{"id":"<\ud800A>","method":"buri/notAMethod"}"#,
            r#"{"id":1,"method":"shutdown"}"#,
            r#"{"method":"exit"}"#,
        ]),
    );
    run.ok();
    assert_eq!(echoed_ids(&run.stdout), ["\"<\u{fffd}A>\""]);
}
