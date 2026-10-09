//! A `?` whose payload goes straight into something that only reads it: a
//! method's receiver, a field, an index, a call's argument, a `match`
//! (buri-lang/buri#276). The payload has no count of its own, so whatever owns
//! the operand has to outlive that reader.
//!
//! Each run is a real `buri test` over a repository, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-lent-try-{}", std::process::id()))
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

/// `buri test //lib` on every backend, where `//lib` is `lib` with `test` as
/// its one test source. Each run must pass all `blocks` of it.
fn passes(name: &str, lib: &str, test: &str, blocks: usize) {
    let repo = workspace(name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/t.buri\"]\n    }\n}\n",
    );
    write(&repo.join("lib/lib.buri"), lib);
    write(&repo.join("lib/test/t.buri"), test);
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

/// `lib/lib.buri` for the tests that keep everything in their test source.
const NO_LIB: &str = "export fn unused(): Int {\n    0\n}\n";

/// What every test source but the issue's opens with.
const PRELUDE: &str = r#"from "core/list" import * as list;
from "core/str" import * as str;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

fn lists<C: Allocator>(ctx: C, n: Int): [[Int]] {
    list.range(ctx, 0, n).mapCtx(ctx, fn(c, i) => list.range(c, i, i + 2))
}

fn some<C: Allocator, T>(ctx: C, skip: Bool, v: T): Option<T> {
    if (skip) { .None } else { .Some(v) }
}

fn okOr<C: Allocator, T: Show>(ctx: C, fail: Bool, v: T): Result<T, Str> {
    if (fail) { .Err(str.format(ctx, "no ${v.show(ctx)}")) } else { .Ok(v) }
}
"#;

fn with_prelude(name: &str, body: &str, blocks: usize) {
    passes(name, NO_LIB, &format!("{PRELUDE}{body}"), blocks);
}

/// The issue's program and test, verbatim.
#[test]
fn a_fold_called_on_a_question_mark_reads_the_unwrapped_list() {
    let lib = r#"from "platform/effect" import { Allocator };

export fn total<C: Allocator>(ctx: C, items: [([Int], Int)], skip: Bool): Option<[[Int]]> {
    let kept = if (skip) { Option.None } else { .Some(items) };
    let rows = kept?.foldCtx(
        ctx,
        fn(c, acc: Option<[[Int]]>, one) => {
            match (acc) {
                .None => Option.None,
                .Some(sofar) => .Some(sofar.push(c, one.0.push(c, one.1))),
            }
        },
        .Some([[100]]),
    )?;
    .Some(rows)
}
"#;
    let test = r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };
from "//lib" import { total };

test "a ? before a method call yields the unwrapped value's result" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(total(ctx, [([1], 2), ([3], 4)], false), .Some([[100], [1, 2], [3, 4]]));
    assert.equal(total(ctx, [([1], 2)], true), .None);
}
"#;
    passes("issue", lib, test, 1);
}

/// The issue's follow-up: a list accumulator, which first showed freed memory.
#[test]
fn a_fold_on_a_question_mark_grows_a_list_accumulator() {
    with_prelude(
        "fold",
        r#"
fn grown<C: Allocator>(ctx: C, skip: Bool): Option<[[Int]]> {
    let kept = some(ctx, skip, lists(ctx, 3));
    .Some(kept?.foldCtx(ctx, fn(c, acc: [[Int]], one) => acc.push(c, one.push(c, 9)), [[100]]))
}

test "grown" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(grown(ctx, false), .Some([[100], [0, 1, 9], [1, 2, 9], [2, 3, 9]]));
    assert.equal(grown(ctx, true), .None);
}
"#,
        1,
    );
}

#[test]
fn a_method_without_a_closure_on_a_question_mark_reads_the_unwrapped_value() {
    with_prelude(
        "no-closure",
        r#"
fn read<C: Allocator>(ctx: C, skip: Bool): Option<(Int, Option<[Int]>, [[Int]])> {
    let kept = some(ctx, skip, lists(ctx, 3));
    let other = some(ctx, skip, lists(ctx, 2));
    let s = some(ctx, skip, str.format(ctx, "abc${skip}"));
    .Some((kept?.length() + s?.length(), other?.get(1), other?.concat(ctx, [[7]])))
}

test "read" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(read(ctx, false), .Some((11, .Some([1, 2]), [[0, 1], [1, 2], [7]])));
    assert.equal(read(ctx, true), .None);
}
"#,
        1,
    );
}

#[test]
fn a_closure_method_on_a_question_mark_reads_every_element() {
    with_prelude(
        "closures",
        r#"
fn read<C: Allocator>(ctx: C, skip: Bool): Option<([Int], [[Int]], Bool, Int)> {
    let a = some(ctx, skip, lists(ctx, 3));
    let b = some(ctx, skip, lists(ctx, 3));
    let c = some(ctx, skip, lists(ctx, 3));
    let d = some(ctx, skip, lists(ctx, 3));
    .Some((
        a?.map(ctx, fn(xs: [Int]) => xs.length() + xs[0].withDefault(0)),
        b?.filter(ctx, fn(xs: [Int]) => xs[0].withDefault(0) > 0),
        c?.any(fn(xs: [Int]) => xs[1].withDefault(0) == 3),
        d?.fold(fn(acc: Int, xs: [Int]) => acc + xs[1].withDefault(0), 0),
    ))
}

test "read" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(read(ctx, false), .Some(([2, 3, 4], [[1, 2], [2, 3]], true, 6)));
    assert.equal(read(ctx, true), .None);
}
"#,
        1,
    );
}

/// A struct with counted fields, and a list of lists.
#[test]
fn a_field_or_an_index_of_a_question_mark_reads_the_unwrapped_value() {
    with_prelude(
        "fields",
        r#"
struct Rec {
    name: Str,
    items: [Int],
}

fn read<C: Allocator>(ctx: C, skip: Bool): Option<(Str, Int, Option<[Int]>)> {
    let r = some(ctx, skip, Rec { name: str.format(ctx, "rec${skip}"), items: list.range(ctx, 0, 4) });
    let q = some(ctx, skip, Rec { name: str.format(ctx, "q${skip}"), items: list.range(ctx, 5, 9) });
    let xs = some(ctx, skip, lists(ctx, 3));
    .Some((
        str.format(ctx, "[${r?.name}]"),
        q?.items.foldCtx(ctx, fn(_c, acc: Int, i) => acc + i, 0),
        xs?[1],
    ))
}

test "read" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(read(ctx, false), .Some(("[recfalse]", 26, .Some([1, 2]))));
    assert.equal(read(ctx, true), .None);
}
"#,
        1,
    );
}

#[test]
fn chained_question_marks_read_each_unwrapped_value() {
    with_prelude(
        "chained",
        r#"
fn read<C: Allocator>(ctx: C, skip: Bool, k: Int): Option<[Int]> {
    let all = some(ctx, skip, lists(ctx, 3));
    .Some(all?.get(k)?.mapCtx(ctx, fn(c, i) => i * 10))
}

test "read" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(read(ctx, false, 2), .Some([20, 30]));
    assert.equal(read(ctx, false, 5), .None);
    assert.equal(read(ctx, true, 0), .None);
}
"#,
        1,
    );
}

#[test]
fn a_question_mark_in_an_argument_lives_through_the_call() {
    with_prelude(
        "argument",
        r#"
fn total(xs: [[Int]], extra: [Int]): Int {
    xs.fold(fn(acc: Int, x) => acc + x.length(), 0) + extra.length()
}

fn read<C: Allocator>(ctx: C, skip: Bool): Option<Int> {
    let kept = some(ctx, skip, lists(ctx, 3));
    let extra = list.range(ctx, 0, 2);
    .Some(total(kept?, extra) + total(lists(ctx, 1), some(ctx, skip, list.range(ctx, 0, 5))?))
}

fn sizes<C: Allocator>(ctx: C, all: Option<[[Int]]>): [Int] {
    all.withDefault([]).mapCtx(ctx, fn(_c, xs) => xs.length())
}

// The operand's last mention is a later argument.
fn sibling<C: Allocator>(ctx: C, skip: Bool): Option<Int> {
    let kept = some(ctx, skip, lists(ctx, 3));
    .Some(total(kept?, sizes(ctx, kept)))
}

test "read" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(read(ctx, false), .Some(15));
    assert.equal(read(ctx, true), .None);
}

test "sibling" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(sibling(ctx, false), .Some(9));
    assert.equal(sibling(ctx, true), .None);
}
"#,
        2,
    );
}

/// The error handed back is the operand's, whether or not the operand is read
/// again after the `?`.
#[test]
fn a_question_mark_on_a_result_hands_back_its_error() {
    with_prelude(
        "results",
        r#"
fn dead<C: Allocator>(ctx: C, fail: Bool): Result<[[Int]], Str> {
    let kept = okOr(ctx, fail, lists(ctx, 2));
    .Ok(kept?.foldCtx(ctx, fn(c, acc: [[Int]], one) => acc.push(c, one), [[100]]))
}

fn live<C: Allocator>(ctx: C, fail: Bool): Result<(Int, Bool), Str> {
    let kept = okOr(ctx, fail, lists(ctx, 2));
    let n = kept?.foldCtx(ctx, fn(c, acc: Int, one) => acc + one.length(), 0);
    .Ok((n, kept.isOk()))
}

test "dead" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(dead(ctx, false), .Ok([[100], [0, 1], [1, 2]]));
    assert.equal(dead(ctx, true), .Err("no [[0, 1], [1, 2]]"));
}

test "live" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(live(ctx, false), .Ok((4, true)));
    assert.equal(live(ctx, true), .Err("no [[0, 1], [1, 2]]"));
}
"#,
        2,
    );
}

#[test]
fn a_method_taking_ctx_or_a_generic_method_on_a_question_mark() {
    with_prelude(
        "methods",
        r#"
struct Rec {
    name: Str,
    items: [Int],
}

impl Rec {
    fn render<C: Allocator>(self, ctx: C): Str {
        str.format(ctx, "${self.name}:${self.items.length()}")
    }

    fn pick<T>(self, choose: fn([Int]) => T): T {
        choose(self.items)
    }
}

struct Holder<T> {
    value: T,
    tags: [Str],
}

impl<T> Holder<T> {
    fn apply<C: Allocator, U>(self, ctx: C, f: fn(C, T, [Str]) => U): U {
        f(ctx, self.value, self.tags)
    }
}

fn read<C: Allocator>(ctx: C, skip: Bool): Option<(Str, Int, Str)> {
    let r = some(ctx, skip, Rec { name: str.format(ctx, "m${skip}"), items: list.range(ctx, 0, 3) });
    let p = some(ctx, skip, Rec { name: str.format(ctx, "p${skip}"), items: list.range(ctx, 0, 6) });
    let h = some(ctx, skip, Holder { value: lists(ctx, 2), tags: ["a", str.format(ctx, "b${skip}")] });
    .Some((
        r?.render(ctx),
        p?.pick(fn(xs) => xs.length()),
        h?.apply(ctx, fn(c, v, tags) => str.format(c, "${v.length()} ${tags.join(c, ",")}")),
    ))
}

test "read" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(read(ctx, false), .Some(("mfalse:3", 6, "2 a,bfalse")));
    assert.equal(read(ctx, true), .None);
}
"#,
        1,
    );
}

/// The operand as a parameter, a field, a branch and a `match`'s scrutinee.
#[test]
fn a_question_mark_on_any_operand_keeps_it_for_its_reader() {
    with_prelude(
        "operands",
        r#"
struct Box {
    res: Result<[Str], Str>,
}

fn param(r: Result<[Str], Str>): Result<Int, Str> {
    .Ok(r?.fold(fn(acc: Int, s: Str) => acc + s.length(), 0))
}

fn field(b: Box): Result<Int, Str> {
    .Ok(b.res?.fold(fn(acc: Int, s: Str) => acc + s.length(), 0))
}

fn branch<C: Allocator>(ctx: C, fail: Bool, which: Bool): Result<Int, Str> {
    let a = okOr(ctx, fail, [str.format(ctx, "aa${fail}")]);
    let b = okOr(ctx, fail, [str.format(ctx, "b${fail}")]);
    .Ok((if (which) { a } else { b })?.fold(fn(acc: Int, s: Str) => acc + s.length(), 0))
}

fn scrutinee<C: Allocator>(ctx: C, fail: Bool): Result<Str, Str> {
    let a = okOr(ctx, fail, [str.format(ctx, "x${fail}"), str.format(ctx, "y${fail}")]);
    match (a?) {
        [first, ..] => .Ok(str.format(ctx, "${first}!")),
        [] => .Err("empty"),
    }
}

test "operands" {
    let ctx = context {
        Allocator: alloc(),
    };
    let ok: Result<[Str], Str> = .Ok([str.format(ctx, "abc${1}"), "de"]);
    let bad: Result<[Str], Str> = .Err(str.format(ctx, "bad${1}"));
    assert.equal(param(ok), .Ok(6));
    assert.equal(param(bad), .Err("bad1"));
    assert.equal(field(Box { res: ok }), .Ok(6));
    assert.equal(field(Box { res: bad }), .Err("bad1"));
    assert.equal(branch(ctx, false, true), .Ok(7));
    assert.equal(branch(ctx, false, false), .Ok(6));
    assert.equal(branch(ctx, true, true), .Err("no [\"aatrue\"]"));
    assert.equal(scrutinee(ctx, false), .Ok("xfalse!"));
    assert.equal(scrutinee(ctx, true), .Err("no [\"xtrue\", \"ytrue\"]"));
    assert.equal(param(ok), .Ok(6));
    assert.equal(param(bad), .Err("bad1"));
}
"#,
        1,
    );
}
