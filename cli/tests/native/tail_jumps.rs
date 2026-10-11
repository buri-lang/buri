//! A tail-recursive function that returns a value on one path and calls
//! itself on others, with the calls in more than one branch. The value
//! returned on the path that falls through still has to be released by
//! whoever matched it.
//!
//! Each run is a real `buri test` over a repository, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-tail-jumps-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

/// `buri test //lib` on every backend, with `test` as the one test source.
/// Each run must pass all `blocks` of it.
fn passes(name: &str, test: &str, blocks: usize) {
    let repo = workspace(name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/t.buri\"]\n    }\n}\n",
    );
    write(&repo.join("lib/lib.buri"), "export fn unused(): Int {\n    0\n}\n");
    write(&repo.join("lib/test/t.buri"), &format!("{PRELUDE}{test}"));
    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo).arg("test").args(flags).arg("//lib").arg("--force");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        let summary = format!("{blocks} passed, 0 failed");
        assert!(
            ran.status == 0 && ran.stdout.contains(&summary),
            "{backend}: status {}\n{}\n{}",
            ran.status,
            ran.stdout,
            ran.stderr
        );
    }
}

/// `next(ctx, n)` fails on a multiple of three and answers `"line${n}"`
/// otherwise, which is the file read of the issue without a file.
const PRELUDE: &str = r#"from "core/fs" import * as fs;
from "core/fs" import { FileSystemRead };
from "core/path" import * as path;
from "core/path" import { Path };
from "core/str" import * as str;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };
from "platform/effect/testing" import { fs as files };

fn next<C: Allocator>(ctx: C, n: Int): Result<Str, Str> {
    if (n % 3 == 0) { .Err(str.format(ctx, "e${n}")) } else { .Ok(str.format(ctx, "line${n}")) }
}
"#;

/// The issue's shape: an `if` in the `.Ok` arm, and both arms calling again.
#[test]
fn a_value_returned_beside_calls_in_two_arms_is_released() {
    let test = r#"
fn untilReady<C: Allocator>(ctx: C, n: Int): Str {
    match (next(ctx, n)) {
        .Ok(text) => if (n >= 5) { text } else { untilReady(ctx, n + 1) },
        .Err(_e) => untilReady(ctx, n + 1),
    }
}

test "the first ready line, past a failure and a line too early" {
    let ctx = context { Allocator: alloc() };
    assert.equal(untilReady(ctx, 1), "line5");
    assert.equal(untilReady(ctx, 5), "line5");
}
"#;
    passes("issue", test, 1);
}

/// The issue verbatim, through a file system.
#[test]
fn a_file_read_until_it_is_ready_is_released() {
    let test = r#"
fn untilReady<C: Allocator + FileSystemRead>(ctx: C, ready: Path): Str {
    match (fs.readText(ctx, ready)) {
        .Ok(text) => if (text.endsWith("\n")) { text } else { untilReady(ctx, ready) },
        .Err(_e) => untilReady(ctx, ready),
    }
}

test "a ready file is read once" {
    let ctx = context { Allocator: alloc(), FileSystemRead: files().files([("ready", "up\n")]) };
    assert.equal(untilReady(ctx, path.of(ctx, "ready")), "up\n");
}
"#;
    passes("file", test, 1);
}

/// One more `if` between the arm and the value.
#[test]
fn a_value_under_nested_ifs_is_released() {
    let test = r#"
fn untilReady<C: Allocator>(ctx: C, n: Int): Str {
    match (next(ctx, n)) {
        .Ok(text) => if (n >= 4) {
            if (n % 2 == 1) { text } else { untilReady(ctx, n + 1) }
        } else {
            untilReady(ctx, n + 1)
        },
        .Err(_e) => untilReady(ctx, n + 1),
    }
}

fn firstOdd<C: Allocator>(ctx: C, n: Int): Str {
    if (n >= 4) {
        match (next(ctx, n)) {
            .Ok(text) => if (n % 2 == 1) { text } else { firstOdd(ctx, n + 1) },
            .Err(_e) => firstOdd(ctx, n + 1),
        }
    } else {
        firstOdd(ctx, n + 1)
    }
}

test "the first odd line from four" {
    let ctx = context { Allocator: alloc() };
    assert.equal(untilReady(ctx, 1), "line5");
    assert.equal(firstOdd(ctx, 1), "line5");
}
"#;
    passes("nested", test, 1);
}

/// A call as the right operand of `||` is a tail call too, and the left
/// operand's answer falls through.
#[test]
fn a_call_behind_an_or_beside_another_arm_releases_the_scrutinee() {
    let test = r#"
fn anyFive<C: Allocator>(ctx: C, n: Int): Bool {
    match (next(ctx, n)) {
        .Ok(text) => text == "line5" || (n < 9 && anyFive(ctx, n + 1)),
        .Err(_e) => anyFive(ctx, n + 1),
    }
}

test "a line five is found and a line ten is not" {
    let ctx = context { Allocator: alloc() };
    assert.equal(anyFive(ctx, 1), true);
    assert.equal(anyFive(ctx, 7), false);
}
"#;
    passes("or", test, 1);
}

/// Three arms, two of them calling, and a list rather than a string.
#[test]
fn a_list_returned_beside_calls_in_two_of_three_arms_is_released() {
    let test = r#"
enum Step {
    Done([Int]),
    Skip(Str),
    Again([Int]),
}

fn step<C: Allocator>(ctx: C, n: Int): Step {
    if (n >= 6) {
        .Done([n, n + 1])
    } else if (n % 2 == 0) {
        .Skip(str.format(ctx, "skip${n}"))
    } else {
        .Again([n])
    }
}

fn run<C: Allocator>(ctx: C, n: Int): [Int] {
    match (step(ctx, n)) {
        .Done(xs) => if (xs.length() > 1) { xs } else { run(ctx, n + 1) },
        .Skip(_s) => run(ctx, n + 1),
        .Again(_xs) => run(ctx, n + 1),
    }
}

test "the first list of two" {
    let ctx = context { Allocator: alloc() };
    assert.equal(run(ctx, 1), [6, 7]);
}
"#;
    passes("list", test, 1);
}

/// Two functions calling each other, merged into one loop.
#[test]
fn a_value_returned_beside_calls_between_two_functions_is_released() {
    let test = r#"
fn ping<C: Allocator>(ctx: C, n: Int): Str {
    match (next(ctx, n)) {
        .Ok(text) => if (n >= 5) { text } else { pong(ctx, n + 1) },
        .Err(_e) => pong(ctx, n + 1),
    }
}

fn pong<C: Allocator>(ctx: C, n: Int): Str {
    match (next(ctx, n)) {
        .Ok(text) => if (n >= 5) { text } else { ping(ctx, n + 1) },
        .Err(_e) => ping(ctx, n + 1),
    }
}

test "the first ready line, from either side" {
    let ctx = context { Allocator: alloc() };
    assert.equal(ping(ctx, 1), "line5");
    assert.equal(pong(ctx, 1), "line5");
}
"#;
    passes("mutual", test, 1);
}
