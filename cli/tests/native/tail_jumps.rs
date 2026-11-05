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
    write(
        &repo.join("lib/lib.buri"),
        "export fn unused(): Int {\n    0\n}\n",
    );
    write(&repo.join("lib/test/t.buri"), &format!("{PRELUDE}{test}"));
    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo)
            .arg("test")
            .args(flags)
            .arg("//lib")
            .arg("--force");
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

/// A tree whose labels are long enough to live on the heap, three levels
/// deep, for the descents below. None of them hands back the node it was
/// given, so each only reads it.
const TREE: &str = r#"
struct Node {
    label: Str,
    kids: [Node],
}

fn grow<C: Allocator>(ctx: C, depth: Int, at: Int): Node {
    let label = str.format(ctx, "a label long enough for the heap ${depth}.${at}");
    if (depth == 0) {
        Node { label: label, kids: [] }
    } else {
        Node { label: label, kids: [grow(ctx, depth - 1, 0), grow(ctx, depth - 1, 1)] }
    }
}
"#;

/// A descent that hands each step a piece of the node it was given: the
/// tree it walks is the caller's, and it is whole afterwards.
#[test]
fn a_descent_into_pieces_of_its_argument_leaves_the_tree_whole() {
    let test = format!(
        "{TREE}{}",
        r#"
fn reaches(n: Node, path: [Int], at: Int, label: Str): Bool {
    match (path[at]) {
        .None => n.label == label,
        .Some(i) => match (n.kids[i]) {
            .Some(child) => reaches(child, path, at + 1, label),
            .None => n.label == label,
        },
    }
}

test "the label at the end of a path, twice, then the root" {
    let ctx = context { Allocator: alloc() };
    let tree = grow(ctx, 3, 0);
    assert.isTrue(reaches(tree, [1, 0, 1], 0, "a label long enough for the heap 0.1"));
    assert.isTrue(reaches(tree, [1, 0, 1], 0, "a label long enough for the heap 0.1"));
    assert.isTrue(reaches(tree, [0, 5], 0, "a label long enough for the heap 2.0"));
    assert.isFalse(reaches(tree, [0, 0], 0, "a label long enough for the heap 2.0"));
    assert.equal(tree.label, "a label long enough for the heap 3.0");
}
"#
    );
    passes("descent", &test, 1);
}

/// The same descent, keeping each node it passes as well as stepping into
/// it, and a piece of a piece.
#[test]
fn a_descent_that_keeps_what_it_steps_into_leaves_the_tree_whole() {
    let test = format!(
        "{TREE}{}",
        r#"
fn trail<C: Allocator>(ctx: C, n: Node, acc: [Node]): [Node] {
    match (n.kids[1]) {
        .Some(child) => trail(ctx, child, acc.push(ctx, child)),
        .None => acc,
    }
}

fn leftmost(n: Node): [Node] {
    match (n.kids[0]) {
        .Some(child) => if (child.kids.isEmpty()) { [child] } else { leftmost(child) },
        .None => [],
    }
}

fn grandchild(n: Node, label: Str): Bool {
    match (n.kids[0]) {
        .Some(child) => match (child.kids[1]) {
            .Some(g) => grandchild(g, label),
            .None => child.label == label,
        },
        .None => n.label == label,
    }
}

test "every node down the right side, the leftmost leaf, and every other one down the left" {
    let ctx = context { Allocator: alloc() };
    let tree = grow(ctx, 3, 0);
    let kept = trail(ctx, tree, []);
    assert.equal(kept.length(), 3);
    assert.equal(kept[2].map(fn(k) => k.label), .Some("a label long enough for the heap 0.1"));
    assert.equal(leftmost(tree).map(ctx, fn(k) => k.label), ["a label long enough for the heap 0.0"]);
    assert.isTrue(grandchild(tree, "a label long enough for the heap 0.0"));
    assert.equal(tree.label, "a label long enough for the heap 3.0");
}
"#
    );
    passes("kept", &test, 1);
}

/// A descent split between two functions, each stepping into a piece of
/// what the other handed it.
#[test]
fn a_descent_between_two_functions_leaves_the_tree_whole() {
    let test = format!(
        "{TREE}{}",
        r#"
fn right(n: Node, label: Str): Bool {
    match (n.kids[1]) {
        .Some(child) => left(child, label),
        .None => n.label == label,
    }
}

fn left(n: Node, label: Str): Bool {
    match (n.kids[0]) {
        .Some(child) => right(child, label),
        .None => n.label == label,
    }
}

test "a zigzag from either side" {
    let ctx = context { Allocator: alloc() };
    let tree = grow(ctx, 3, 0);
    assert.isTrue(right(tree, "a label long enough for the heap 0.1"));
    assert.isTrue(left(tree, "a label long enough for the heap 0.0"));
    assert.equal(tree.label, "a label long enough for the heap 3.0");
}
"#
    );
    passes("zigzag", &test, 1);
}

/// A descent into a piece of something the iteration built rather than of
/// what it was given: the piece outlives the iteration that built it.
#[test]
fn a_descent_into_a_piece_of_what_it_built_keeps_the_piece() {
    let test = format!(
        "{TREE}{}",
        r#"
fn regrow<C: Allocator>(ctx: C, n: Node, depth: Int, label: Str): Bool {
    if (depth == 0) {
        n.label == label
    } else {
        let fresh = grow(ctx, depth, 7);
        match (fresh.kids[1]) {
            .Some(child) => regrow(ctx, child, depth - 1, label),
            .None => n.label == label,
        }
    }
}

test "the label of the last tree grown" {
    let ctx = context { Allocator: alloc() };
    let tree = grow(ctx, 3, 0);
    assert.isTrue(regrow(ctx, tree, 2, "a label long enough for the heap 0.1"));
    assert.equal(tree.label, "a label long enough for the heap 3.0");
}
"#
    );
    passes("built", &test, 1);
}
