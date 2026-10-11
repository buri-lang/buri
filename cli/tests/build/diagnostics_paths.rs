//! How the toolchain reports, whatever it is reporting about: messages from
//! an editor in JSON, a source file too big to hold, a mistake on a line too
//! long to print, and the profile switch.
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

/// A request id comes back as the server read it, so an id is a window on the
/// JSON reader: escapes, surrogate pairs, an unpaired surrogate, numbers too
/// big for an integer, and every literal.
#[test]
fn the_server_reads_every_json_spelling_of_an_id() {
    let scratch = Scratch::repo("diagnostics-json-ids");
    let ids = [
        r#""😀""#,
        r#""<\ud800x>""#,
        r#""<\ud800\n>""#,
        r#""é\t\/\b\f\r""#,
        "99999999999999999999",
        "-1.5e2",
        "[true,false,null]",
    ];
    let mut messages = vec![r#"{"id":0,"method":"initialize","params":{}}"#.to_string()];
    for id in ids {
        messages.push(format!(r#"{{"id":{id},"method":"buri/notAMethod"}}"#));
    }
    messages.push(r#"{"id":1,"method":"shutdown"}"#.to_string());
    messages.push(r#"{"method":"exit"}"#.to_string());
    let messages: Vec<&str> = messages.iter().map(String::as_str).collect();
    let run = scratch.run_with_stdin(&["lsp"], &framed(&messages));
    run.ok();
    assert_eq!(
        echoed_ids(&run.stdout),
        [
            "\"😀\"",
            "\"<\u{fffd}x>\"",
            "\"<\u{fffd}\\n>\"",
            "\"é\\t/\\u0008\\u000c\\r\"",
            "1e20",
            "-150.0",
            "[true,false,null]",
        ]
    );
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

/// A message that isn't JSON is answered with why, and the session goes on.
#[test]
fn a_message_that_is_not_json_says_why() {
    let scratch = Scratch::repo("diagnostics-json-malformed");
    let cases = [
        (r#"{"a":1} x"#, "trailing data at byte 8"),
        (r#"{"a" 1}"#, "expected `:` at byte 5"),
        ("nul", "expected `null` at byte 0"),
        (r#""\q""#, "unknown escape `\\q`"),
        (r#""\u12""#, "a \\u escape needs four digits, at byte 3"),
        (r#""\u123é""#, "a \\u escape needs four digits, at byte 3"),
        (r#""\uzzzz""#, "`zzzz` is not four hex digits, at byte 3"),
        ("[1 2]", "expected `,` or `]` at byte 3"),
        (r#"{"a":1 "b":2}"#, "expected `,` or `}` at byte 7"),
        (r#""abc"#, "a string is never closed"),
        (r#""\"#, "an escape is never completed"),
        ("1e999", "`1e999` is not a finite number"),
    ];
    let mut messages: Vec<&str> = cases.iter().map(|(m, _)| *m).collect();
    messages.push(r#"{"id":1,"method":"shutdown"}"#);
    messages.push(r#"{"method":"exit"}"#);
    let run = scratch.run_with_stdin(&["lsp"], &framed(&messages));
    run.ok();
    let said: Vec<&str> =
        run.stderr.lines().filter_map(|l| l.strip_prefix("buri lsp: unparseable message: ")).collect();
    let want: Vec<&str> = cases.iter().map(|(_, why)| *why).collect();
    assert_eq!(said, want);
}

/// A source file over four gigabytes is refused at the door, by name, in both
/// formats. Sparse, so the test writes one byte.
#[test]
fn a_source_file_too_big_to_hold_is_refused() {
    let scratch = Scratch::repo("diagnostics-huge-file");
    scratch.write("cmd/app/BUILD.buri", "binary {\n    outputs: [{ platform: \"node\" }]\n}\n");
    let file = std::fs::File::create(scratch.path("cmd/app/main.buri")).unwrap();
    file.set_len(5 << 30).unwrap();
    scratch.run(&["build", "//cmd/app"]).exits(1).says(
        "error: cannot read cmd/app/main.buri: the file is larger than 4294967295 bytes, which is the most one source file may hold",
    );
    let json = scratch.run(&["build", "//cmd/app", "--error-format=json"]);
    json.exits(1);
    assert_eq!(
        json.stdout.trim(),
        r#"{"severity":"error","message":"cannot read cmd/app/main.buri: the file is larger than 4294967295 bytes, which is the most one source file may hold","location":null,"fix":"check the file exists and is readable","notes":[],"related":[]}"#
    );
}

/// A mistake on a long line of two-byte characters is shown as a window cut
/// between characters, never through one.
#[test]
fn a_long_line_of_wide_characters_is_cut_between_them() {
    let scratch = Scratch::repo("diagnostics-wide-line");
    scratch.write("cmd/app/BUILD.buri", "binary {\n    outputs: [{ platform: \"node\" }]\n}\n");
    // The variable's name moves the cut by a byte at a time, so one of these
    // lands each edge of the window inside a character.
    let functions: Vec<String> = (1..=4)
        .map(|n| {
            format!(
                "fn f{n}(): Int {{ let a = \"{}\"; let {}: Int = \"x\"; let c = \"{}\"; 0 }}",
                "é".repeat(150),
                "b".repeat(n),
                "é".repeat(200)
            )
        })
        .collect();
    scratch.write("cmd/app/main.buri", &format!("{}\n", functions.join("\n\n")));
    let run = scratch.run(&["build", "//cmd/app"]);
    run.exits(1);
    let shown: Vec<&str> = run.stderr.lines().filter(|l| l.contains("Int = \"x\"")).collect();
    assert_eq!(shown.len(), 4, "{}", run.stderr);
    for line in shown {
        let text = line.split_once(" | ").unwrap().1;
        assert!(text.starts_with("…é") && text.ends_with("é…"), "{line}");
    }
}

/// An empty `BURI_PROFILE` is off, as `0` is.
#[test]
fn an_empty_profile_variable_prints_no_profile() {
    let scratch = Scratch::repo("diagnostics-empty-profile");
    scratch.write("cmd/app/BUILD.buri", "binary {\n    outputs: [{ platform: \"node\" }]\n}\n");
    scratch.write(
        "cmd/app/main.buri",
        "from \"node\" import { NodeHost };\n\nexport fn main(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n",
    );
    scratch.run_with_env(&["build", "//cmd/app"], &[("BURI_PROFILE", "")]).ok().silent_about("buri profile");
}

/// A native build's record whose link key was damaged is passed over: the
/// build still finds the executable and the program still runs.
#[test]
fn a_damaged_build_record_is_passed_over() {
    let scratch = Scratch::repo("diagnostics-damaged-record");
    scratch.write("cmd/app/BUILD.buri", "binary {\n    outputs: [{ platform: \"native\" }]\n}\n");
    scratch.write(
        "cmd/app/main.buri",
        "from \"core/io\" import * as io;\nfrom \"native\" import { NativeHost };\nfrom \"platform/effect\" import { Allocator, Stdout };\n\nexport fn main(host: NativeHost): Result<(), Str> {\n    let ctx = context {\n        Allocator: host.alloc,\n        Stdout: host.stdout,\n    };\n    io.println(ctx, \"native\").mapErr(fn(_e) => \"could not write\")\n}\n",
    );
    scratch.run(&["build", "//cmd/app"]).ok();
    // A record opens with its parts' count and the link key's length.
    let mut damaged = 0;
    for dir in std::fs::read_dir(scratch.path(".buri/cache")).unwrap().flatten() {
        let Ok(entries) = std::fs::read_dir(dir.path()) else { continue };
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            if let Some(rest) = text.strip_prefix("3\n64\n") {
                std::fs::write(entry.path(), format!("3\n63\n{}", &rest[1..])).unwrap();
                damaged += 1;
            }
        }
    }
    assert_eq!(damaged, 1, "no native build record was found to damage");
    scratch.run(&["build", "//cmd/app"]).ok().says("cached");
    scratch.run(&["run", "//cmd/app"]).ok().says("native");
}

/// The formatter keeps a literal's printable characters, wide ones too, and
/// writes an invisible one as an escape.
#[test]
fn a_literal_s_invisible_character_is_written_as_an_escape() {
    let scratch = Scratch::repo("diagnostics-literal-escape");
    scratch.write("lib/a/BUILD.buri", "library {}\n");
    scratch.write("lib/a/lib.buri", "export fn f(): Str {\n    \"café \u{200b}\"\n}\n");
    scratch.run(&["format", "lib/a/lib.buri"]).ok();
    assert_eq!(scratch.read("lib/a/lib.buri"), "export fn f(): Str {\n    \"café \\u{200b}\"\n}\n");
}
