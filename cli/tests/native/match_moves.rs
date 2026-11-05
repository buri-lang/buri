//! A `match` on a value the function is done with: its arms keep some of what
//! it holds and let the rest go. What each arm answers, and that everything it
//! let go is released once, under the heap check, on JavaScript and on every
//! native backend this toolchain has built in; and what taking a value apart
//! costs against never having put it together.

use crate::collection_costs::per_op;
use std::path::Path;
use std::process::Command;

fn workspace(name: &str) -> crate::sweep::Owned {
    crate::sweep::owned("native-match-moves", name)
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

/// Every payload a heap `Str`, so a release too many frees one twice and a
/// release too few leaves one live at exit.
const PRELUDE: &str = r#"from "core/str" import * as str;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

enum Grown { Fit(Str), Split(Str, Str, Str) }

struct Pair { kept: Str, dropped: Str }

fn grow<C: Allocator>(ctx: C, n: Int): Grown {
    if (n % 2 == 0) {
        .Fit(str.format(ctx, "fit${n}"))
    } else {
        .Split(str.format(ctx, "l${n}"), str.format(ctx, "u${n}"), str.format(ctx, "r${n}"))
    }
}
"#;

/// An arm that keeps one payload and skips the others, by name and by `_`.
#[test]
fn an_arm_keeps_what_it_names_and_releases_the_rest() {
    let test = r#"
fn middle<C: Allocator>(ctx: C, n: Int): Str {
    let g = grow(ctx, n);
    match (g) {
        .Fit(one) => one,
        .Split(_, up, _) => up,
    }
}

fn outside<C: Allocator>(ctx: C, n: Int): Str {
    let g = grow(ctx, n);
    match (g) {
        .Fit(_one) => str.format(ctx, "none"),
        .Split(left, _up, right) => str.format(ctx, "${left}-${right}"),
    }
}

fn nothing<C: Allocator>(ctx: C, n: Int): Int {
    let g = grow(ctx, n);
    match (g) {
        .Fit(_) => 1,
        .Split(_, _, _) => 3,
    }
}

test "the payload an arm names" {
    let ctx = context { Allocator: alloc() };
    assert.equal(middle(ctx, 2), "fit2");
    assert.equal(middle(ctx, 3), "u3");
    assert.equal(outside(ctx, 2), "none");
    assert.equal(outside(ctx, 5), "l5-r5");
    assert.equal(nothing(ctx, 4) + nothing(ctx, 7), 4);
}
"#;
    passes("keeps", test, 1);
}

/// Payloads under payloads, a struct's fields, a tuple's places, a text
/// pattern, and the whole value under one name.
#[test]
fn nested_struct_tuple_and_text_patterns_take_a_value_apart() {
    let test = r#"
fn nested<C: Allocator>(ctx: C, n: Int): Str {
    let o: Option<Grown> = if (n > 0) { .Some(grow(ctx, n)) } else { .None };
    match (o) {
        .Some(.Split(left, _, right)) => str.format(ctx, "${right}${left}"),
        .Some(.Fit(one)) => one,
        .None => str.format(ctx, "empty"),
    }
}

fn kept<C: Allocator>(ctx: C, n: Int): Str {
    let p = Pair { kept: str.format(ctx, "k${n}"), dropped: str.format(ctx, "d${n}") };
    match (p) {
        Pair { kept, dropped: _ } => kept,
    }
}

fn second<C: Allocator>(ctx: C, n: Int): Str {
    let t = (str.format(ctx, "a${n}"), str.format(ctx, "b${n}"), n);
    match (t) {
        (_, b, 0) => b,
        (a, _, _) => a,
    }
}

fn spelled<C: Allocator>(ctx: C, n: Int): Str {
    let g = grow(ctx, n);
    match (g) {
        .Split(left, "u3", _) => left,
        .Split(_, up, _) => up,
        .Fit(one) => one,
    }
}

fn whole<C: Allocator>(ctx: C, n: Int): Grown {
    let g = grow(ctx, n);
    match (g) {
        all => all,
    }
}

test "every shape of pattern" {
    let ctx = context { Allocator: alloc() };
    assert.equal(nested(ctx, 3), "r3l3");
    assert.equal(nested(ctx, 2), "fit2");
    assert.equal(nested(ctx, 0), "empty");
    assert.equal(kept(ctx, 1), "k1");
    assert.equal(second(ctx, 0), "b0");
    assert.equal(second(ctx, 1), "a1");
    assert.equal(spelled(ctx, 3), "l3");
    assert.equal(spelled(ctx, 5), "u5");
    assert.equal(spelled(ctx, 6), "fit6");
    assert.equal(nothingOf(whole(ctx, 9)), "u9");
}

fn nothingOf(g: Grown): Str {
    match (g) {
        .Fit(one) => one,
        .Split(_, up, _) => up,
    }
}
"#;
    passes("shapes", test, 1);
}

/// A recursive enum boxes its own payload, an arm that reads the value it
/// took apart still has all of it, and an arm with a guard may not be the arm
/// that runs.
#[test]
fn boxed_payloads_rereads_and_guards_keep_the_whole_value() {
    let test = r#"
enum Tree { Leaf(Str), Fork(Tree, Str) }

fn deep<C: Allocator>(ctx: C, n: Int, acc: Tree): Tree {
    if (n == 0) { acc } else { deep(ctx, n - 1, .Fork(acc, str.format(ctx, "f${n}"))) }
}

fn top(t: Tree): Str {
    match (t) {
        .Leaf(s) => s,
        .Fork(_, s) => s,
    }
}

fn bottom(t: Tree): Str {
    match (t) {
        .Leaf(s) => s,
        .Fork(under, _) => bottom(under),
    }
}

fn again<C: Allocator>(ctx: C, n: Int): Str {
    let g = grow(ctx, n);
    match (g) {
        .Fit(_one) => nameOf(ctx, g),
        .Split(left, _, _) => left,
    }
}

fn nameOf<C: Allocator>(ctx: C, g: Grown): Str {
    match (g) {
        .Fit(one) => str.format(ctx, "fit:${one}"),
        .Split(_, up, _) => str.format(ctx, "split:${up}"),
    }
}

fn guarded<C: Allocator>(ctx: C, n: Int): Str {
    let g = grow(ctx, n);
    match (g) {
        .Split(left, _, _) if left.length() > 2 => left,
        .Split(_, _, right) => right,
        .Fit(one) => one,
    }
}

test "the parts a match keeps whole" {
    let ctx = context { Allocator: alloc() };
    assert.equal(top(deep(ctx, 3, .Leaf(str.format(ctx, "leaf")))), "f1");
    assert.equal(bottom(deep(ctx, 3, .Leaf(str.format(ctx, "leaf")))), "leaf");
    assert.equal(again(ctx, 2), "fit:fit2");
    assert.equal(again(ctx, 3), "l3");
    assert.equal(guarded(ctx, 3), "r3");
    assert.equal(guarded(ctx, 13), "l13");
    assert.equal(guarded(ctx, 4), "fit4");
}
"#;
    passes("whole", test, 1);
}

/// Two payloads swapped through a tail call, `ops` times, against the same two
/// carried as arguments: a value built only to be taken apart again.
const SWAPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

enum Pair { Two(Str, Str), Gone }

fn swapped(p: Pair, k: Int, acc: Int): Int {
  match (p) {
    .Two(a, b) => if (k == 0) { acc } else {
      let n = a.length();
      swapped(.Two(b, a), k - 1, acc + n)
    },
    .Gone => acc,
  }
}

fn carried(a: Str, b: Str, k: Int, acc: Int): Int {
  if (k == 0) { acc } else { carried(b, a, k - 1, acc + a.length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let ops = args[1].andThen(fn(s) => s.toInt()).withDefault(0);
  let a = "ab".repeat(ctx, 3);
  let b = "cd".repeat(ctx, 2);
  let mode = args.first().withDefault("none");
  let answer = if (mode == "swapped") {
    swapped(.Two(a, b), ops, 0)
  } else if (mode == "carried") {
    carried(a, b, ops, 0)
  } else {
    0
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **Taking apart a value the function is done with costs about what never
/// building it does.** Each arm took a count on every payload it named and
/// then released the whole value, which released the payloads again: two
/// retains and a release walk a step, for counts that end where they began:
/// 172 instructions a step against 97 on the copy-and-patch backend, and 46
/// against 11 on LLVM. Moving the payloads out instead is 109 and 13.
#[test]
fn taking_apart_a_dying_value_costs_about_what_carrying_its_parts_does() {
    const STEPS: u64 = 1_000_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("swaps", SWAPS);
        let steps = STEPS.to_string();
        let args = [steps.as_str()];
        let want = format!("{}\n", STEPS * 5);
        let (swapped, _) = per_op(backend, &binary, "swapped", &args, STEPS, &want);
        let (carried, _) = per_op(backend, &binary, "carried", &args, STEPS, &want);
        eprintln!("{backend}: swapped {swapped:?}, carried {carried:?} instructions a step");
        let (Some(swapped), Some(carried)) = (swapped, carried) else {
            continue;
        };
        if swapped * 10 > carried * 15 + 20 {
            failures.push(format!(
                "{backend}: a step through a value taken apart is {swapped} instructions \
                 against {carried} for carrying its parts"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
