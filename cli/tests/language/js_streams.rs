//! What a program's last moment does with what it printed.
//!
//! `buri docs language/programs` makes two promises about the end of a run —
//! `.Err(msg)` prints `msg` on stderr and exits 1, and `proc.exit(n)` exits
//! `n` — and both were kept about the *status* while losing the *output*. The
//! JavaScript backend buffers its streams and empties them on the exit path,
//! and every asynchronous writer a JavaScript host offers hands the text to
//! the event loop and answers before it has landed. `process.exit` does not
//! wait for the loop, so a flush the runtime had already reported as done was
//! discarded: buri-lang/buri#42 for the failing return, buri-lang/buri#37 for
//! `proc.exit`.
//!
//! **Both the file and the pipe are asserted, and neither is redundant.** The
//! two hosts this suite runs on lost different halves of it: `bun` dropped a
//! whole flush written to a *file* and kept the same flush written to a pipe,
//! and `node` wrote a file synchronously and truncated a pipe at the first
//! 64KiB. A case that captured only one of them would pass on one host while
//! the defect was still there on the other, which is why the programs below
//! print past a pipe buffer and are run both ways.
use crate::harness::*;

/// Sixty lines of two thousand characters, which is one flush of a hundred and
/// twenty thousand bytes: past what a pipe takes in a single write, and under
/// the runtime's own buffer limit of sixty-five entries, so the exit path is
/// what writes it and nothing before it has.
const LINES: usize = 60;
const WIDTH: usize = 2000;

fn expected_stdout() -> String {
    let line = "x".repeat(WIDTH);
    (0..LINES).map(|_| format!("{line}\n")).collect()
}

/// A program printing that much on stdout, one line on stderr, and then ending
/// the way `ending` says.
fn program(ending: &str) -> String {
    format!(
        "\
from \"core/effect\" import {{ Alloc, Proc, Stderr, Stdout }};
from \"core/host\" import * as host;
from \"core/io\" import * as io;
from \"core/proc\" import * as proc;

// Prints `line` `n` times. Recursive, so nothing here folds it away.
fn shout<C: Alloc + Stdout>(ctx: C, line: Str, n: Int): Int {{
    if (n <= 0) {{
        0
    }} else {{
        let _ = io.println(ctx, line).ignore();
        shout(ctx, line, n - 1)
    }}
}}

export fn main(): Result<(), Str> {{
    let ctx = context {{
        Alloc: host.alloc,
        Proc: host.proc,
        Stderr: host.stderr,
        Stdout: host.stdout,
    }};
    let line = \"x\".repeat(ctx, {WIDTH});
    let _ = shout(ctx, line, {LINES});
    let _ = io.eprintln(ctx, \"on stderr\").ignore();
    {ending}
}}
"
    )
}

/// Asserts on one run: the status, and both streams whole.
fn check(run: &Run, code: i32, stderr: &str) {
    let stdout = expected_stdout();
    assert_eq!(run.code, code, "{}: the exit status", run.what);
    assert_eq!(
        run.stdout.len(),
        stdout.len(),
        "{}: {} bytes of stdout reached the stream, and the program printed {}",
        run.what,
        run.stdout.len(),
        stdout.len()
    );
    assert_eq!(run.stdout, stdout, "{}: what reached stdout", run.what);
    assert_eq!(run.stderr, stderr, "{}: what reached stderr", run.what);
}

/// buri-lang/buri#42: a `main` answering `.Err(msg)` prints `msg` on stderr,
/// exits 1, and everything it printed before is still there. The `.Ok` path was
/// never the broken one, and it is asserted beside it so that a fix which
/// merely stopped exiting could not pass.
#[test]
fn a_failing_main_keeps_both_streams() {
    let scratch = Scratch::repo("js-streams-err");
    scratch.binary_package("cmd/fails", &program(".Err(\"the program failed\")"));
    scratch.binary_package("cmd/works", &program(".Ok(())"));
    scratch.run(&["build", "//cmd/fails", "--force"]).ok();
    scratch.run(&["build", "//cmd/works", "--force"]).ok();

    let failed = "on stderr\nthe program failed\n";
    check(&scratch.exec_js_to_files("cmd/fails"), 1, failed);
    check(&scratch.exec_js("cmd/fails"), 1, failed);
    check(&scratch.exec_js_to_files("cmd/works"), 0, "on stderr\n");
    check(&scratch.exec_js("cmd/works"), 0, "on stderr\n");
}

/// buri-lang/buri#37: `proc.exit` carries the status a program chose, and
/// everything printed before it reaches the stream — whether the stream is a
/// terminal, a pipe or a file. A status that is neither 0 nor 1 is the only
/// reason to call it, and a failing run is exactly the one whose output a
/// caller captures.
#[test]
fn an_exit_keeps_what_was_printed_before_it() {
    let scratch = Scratch::repo("js-streams-exit");
    scratch.binary_package(
        "cmd/exits",
        &program("let _ = proc.exit(ctx, 3);\n    .Ok(())"),
    );
    scratch.run(&["build", "//cmd/exits", "--force"]).ok();

    check(&scratch.exec_js_to_files("cmd/exits"), 3, "on stderr\n");
    check(&scratch.exec_js("cmd/exits"), 3, "on stderr\n");
}

/// The same two claims about the artifact a user ships. `--release` renames
/// every global and drops what the program does not reach, and the exit path
/// is reached from a statement rather than from a call — which is exactly the
/// shape dead-code elimination has to be told about.
#[test]
fn the_release_artifact_flushes_too() {
    let scratch = Scratch::repo("js-streams-release");
    scratch.binary_package("cmd/fails", &program(".Err(\"the program failed\")"));
    scratch.binary_package(
        "cmd/exits",
        &program("let _ = proc.exit(ctx, 3);\n    .Ok(())"),
    );
    scratch.run(&["build", "//cmd/fails", "--release", "--force"]).ok();
    scratch.run(&["build", "//cmd/exits", "--release", "--force"]).ok();

    check(&scratch.exec_js_to_files("cmd/fails"), 1, "on stderr\nthe program failed\n");
    check(&scratch.exec_js("cmd/fails"), 1, "on stderr\nthe program failed\n");
    check(&scratch.exec_js_to_files("cmd/exits"), 3, "on stderr\n");
    check(&scratch.exec_js("cmd/exits"), 3, "on stderr\n");
}

/// An abort is the third way a run ends, and it is the one a user reads when
/// nothing else explains the failure: the message has to arrive, and so does
/// whatever the program printed before it.
#[test]
fn an_abort_keeps_what_was_printed_before_it() {
    let scratch = Scratch::repo("js-streams-abort");
    // The divisor is the length the runtime answered minus the length asked
    // for, so it is zero and nothing folds the division away beforehand.
    let ending = format!(
        "let zero = line.len() - {WIDTH};\n    \
         let _ = io.println(ctx, \"${{10 / zero}}\").ignore();\n    .Ok(())"
    );
    scratch.binary_package("cmd/aborts", &program(&ending));
    scratch.run(&["build", "//cmd/aborts", "--force"]).ok();

    let stdout = expected_stdout();
    for run in [scratch.exec_js_to_files("cmd/aborts"), scratch.exec_js("cmd/aborts")] {
        assert_eq!(run.code, 1, "{}: an abort exits 1", run.what);
        assert_eq!(
            run.stdout.len(),
            stdout.len(),
            "{}: {} bytes of stdout reached the stream, and the program printed {}",
            run.what,
            run.stdout.len(),
            stdout.len()
        );
        assert_eq!(run.stdout, stdout, "{}: what reached stdout", run.what);
        assert!(
            run.stderr.starts_with("on stderr\n"),
            "{}: the abort's own message displaced what the program printed:\n{}",
            run.what,
            indent(&run.stderr)
        );
        assert!(
            run.stderr.len() > "on stderr\n".len(),
            "{}: an abort says nothing of its own on stderr",
            run.what
        );
    }
}
