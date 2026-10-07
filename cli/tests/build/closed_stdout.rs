//! `buri … | head -1`: a reader that leaves early ends the command, quietly.
//!
//! Every command dies the way a Unix tool does when its reader goes away: by
//! `SIGPIPE`, with nothing on stderr. It used to panic, and `buri test
//! --explain` then hung for ever with its workers waiting on a queue nobody
//! would close. The programs `buri` builds end promptly too.
//!
//! The hang cap is only the safety net here. What each row asserts is the
//! status and the silence.
use crate::harness::*;
use std::io::BufRead;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

const SIGPIPE: i32 = 13;

/// The worked monorepo, with one file out of format so `format --check` has
/// something to print.
fn example() -> Scratch {
    let scratch = Scratch::copy_of("closed-stdout", &example_repo());
    let lib = scratch.read("lib/money/lib.buri");
    scratch.write("lib/money/lib.buri", &format!("{lib}\n\n\n"));
    scratch
}

fn buri_in(scratch: &Scratch, args: &[&str]) -> Command {
    let mut cmd = buri_command();
    cmd.args(args).arg("--color=never").current_dir(&scratch.root);
    cmd
}

/// Runs `cmd` with stdout on a pipe, handing the read end to `reader`.
/// Returns the status and stderr.
fn piped(mut cmd: Command, what: &str, reader: impl FnOnce(std::io::PipeReader)) -> (ExitStatus, String) {
    let (read, write) = std::io::pipe().expect("a pipe");
    let mut child =
        cmd.stdin(Stdio::null()).stdout(write).stderr(Stdio::piped()).spawn().expect("the command runs");
    // The command holds the write end; ours has to go, or the reader never
    // sees the end of the stream.
    drop(cmd);
    reader(read);
    let (_, err) = hang::drain(&mut child);
    let status = hang::wait_capped(&mut child, what, hang::cap());
    (status, String::from_utf8_lossy(&err.take()).into_owned())
}

fn one_line(reader: std::io::PipeReader) -> String {
    let mut first = String::new();
    std::io::BufReader::new(reader).read_line(&mut first).expect("one line reads");
    first
}

fn quiet(what: &str, stderr: &str) {
    for said in ["panicked", "Broken pipe", "broken pipe"] {
        assert!(!stderr.contains(said), "`{what}` said {said:?}:\n{}", indent(stderr));
    }
}

/// One command three ways: a reader that is gone before it starts, one that
/// leaves after a line, and one that stays.
fn closes_quietly(args: &[&str]) {
    let scratch = example();
    let what = format!("buri {}", args.join(" "));

    // Gone before the first write, so the first write is the one that fails.
    let (status, stderr) = piped(buri_in(&scratch, args), &what, drop);
    assert_eq!(status.signal(), Some(SIGPIPE), "`{what}` ended {status} rather than by SIGPIPE:\n{}", indent(&stderr));
    quiet(&what, &stderr);

    // Leaves after one line, on a cold cache, so the command is still working
    // when it does.
    let mut first = String::new();
    let (left, left_stderr) = piped(buri_in(&scratch, args), &what, |r| first = one_line(r));
    quiet(&what, &left_stderr);
    assert!(!first.is_empty(), "`{what}` printed nothing, so this row proves nothing");

    // Stays to the end, on a warm cache: what a file would have held.
    scratch.run(args);
    let path = scratch.path("stdout.txt");
    let file = std::fs::File::create(&path).expect("a capture file");
    let mut child =
        buri_in(&scratch, args).stdin(Stdio::null()).stdout(file).stderr(Stdio::null()).spawn().expect("buri runs");
    let to_file = hang::wait_capped(&mut child, &what, hang::cap());
    let in_file = std::fs::read_to_string(&path).expect("the capture reads back");
    std::fs::remove_file(&path).expect("the capture is removed");
    let mut full = String::new();
    let (open, stderr) = piped(buri_in(&scratch, args), &what, |r| {
        std::io::Read::read_to_string(&mut { r }, &mut full).expect("stdout reads");
    });
    quiet(&what, &stderr);
    assert_eq!(open.code(), to_file.code(), "`{what}` exits differently on a pipe");
    assert_eq!(normalise(&full, &scratch.root), normalise(&in_file, &scratch.root), "`{what}` prints differently on a pipe");

    // Whether anything was written after the one line is a race, so that run
    // either finished or died by SIGPIPE.
    assert!(
        left.signal() == Some(SIGPIPE) || left.code() == open.code(),
        "`{what}` ended {left} after its reader left:\n{}",
        indent(&left_stderr)
    );
}

#[test]
fn test_with_explain() {
    closes_quietly(&["test", "//...", "--explain"]);
}

#[test]
fn test() {
    closes_quietly(&["test", "//..."]);
}

#[test]
fn build_with_explain() {
    closes_quietly(&["build", "//cmd/basket", "--explain"]);
}

#[test]
fn lint() {
    closes_quietly(&["lint", "//..."]);
}

#[test]
fn format_check() {
    closes_quietly(&["format", "--check"]);
}

#[test]
fn docs() {
    closes_quietly(&["docs"]);
}

#[test]
fn help() {
    closes_quietly(&["--help"]);
}

#[test]
fn version() {
    closes_quietly(&["version"]);
}

/// A program printing a million lines, stopping at the first failed print.
const PRINTER: &str = r#"from "core/io" import * as io;
from "HOST_MODULE" import { HOST };
from "platform/effect" import { Allocator, IoError, Stdout };

fn lines<C: Allocator + Stdout>(ctx: C, i: Int): Result<(), IoError> {
    if (i == 0) {
        .Ok(())
    } else {
        match (io.println(ctx, "line ${i}")) {
            .Ok(_) => lines(ctx, i - 1),
            .Err(e) => .Err(e),
        }
    }
}

export fn main(host: HOST): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    match (lines(ctx, 1000000)) {
        .Ok(_) => .Ok(()),
        .Err(_) => .Err("could not write to standard output"),
    }
}
"#;

/// A built program, either way its reader leaves: it ends promptly, by
/// `SIGPIPE` natively or through its own failed print on JavaScript.
fn program_closes(artifact: impl Fn() -> Command, what: &str) {
    for early in [true, false] {
        let (status, stderr) = piped(artifact(), what, |r| {
            if !early {
                assert_eq!(one_line(r), "line 1000000\n");
            }
        });
        let own_failure = status.code() == Some(1) && stderr.contains("could not write to standard output");
        assert!(
            status.signal() == Some(SIGPIPE) || own_failure,
            "`{what}` ended {status} after its reader left:\n{}",
            indent(&stderr)
        );
        quiet(what, &stderr);
    }
}

#[test]
fn a_native_program() {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    let scratch = Scratch::repo("closed-stdout-native");
    scratch.write("cmd/p/BUILD.buri", &format!("binary {{\n  outputs: [{{ platform: \"native\", variant: \"{os}-{arch}\" }}]\n}}\n"));
    scratch.write("cmd/p/main.buri", &PRINTER.replace("HOST_MODULE", "native").replace("HOST", "NativeHost"));
    scratch.run(&["build", "//cmd/p"]).ok();
    let exe = scratch.path(&format!(".buri/out/native/{os}-{arch}/cmd/p/p"));
    program_closes(
        || {
            let mut cmd = Command::new(&exe);
            heap_checked(&mut cmd);
            cmd
        },
        "the native program",
    );
}

#[test]
fn a_javascript_program() {
    let scratch = Scratch::repo("closed-stdout-js");
    scratch.binary_package("cmd/p", &PRINTER.replace("HOST_MODULE", "node").replace("HOST", "NodeHost"));
    scratch.run(&["build", "//cmd/p"]).ok();
    let artifact = scratch.artifact("cmd/p");
    program_closes(
        || {
            let mut cmd = Command::new(js_runtime());
            cmd.arg(&artifact).current_dir(&scratch.root);
            cmd
        },
        "the JavaScript program",
    );
}
