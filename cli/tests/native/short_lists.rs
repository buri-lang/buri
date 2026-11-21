//! Short lists built by pushes, natively, on every native backend built in:
//! what each holds at every length, beside lists that share a prefix, and what
//! building one costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// For each length from 1 to 12, a list of that many texts, one of that many
/// `Int`s and one of that many records of a text and four `Int`s, each pushed
/// one at a time onto the empty list. Then two lists
/// pushed from one shared prefix, one from a list literal, and one from a
/// prefix whose longer list died first. Prints each list.
const BUILT: &str = r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn words<C: Allocator>(ctx: C, i: Int, n: Int, acc: [Str]): [Str] {
  if (i == n) { acc } else { words(ctx, i + 1, n, acc.push(ctx, str.format(ctx, "w${i}"))) }
}

fn ints<C: Allocator>(ctx: C, i: Int, n: Int, acc: [Int]): [Int] {
  if (i == n) { acc } else { ints(ctx, i + 1, n, acc.push(ctx, i * 7)) }
}

struct Rec { name: Str, a: Int, b: Int, c: Int, d: Int }

fn recs<C: Allocator>(ctx: C, i: Int, n: Int, acc: [Rec]): [Rec] {
  if (i == n) { acc } else {
    recs(ctx, i + 1, n, acc.push(ctx, Rec { name: str.format(ctx, "r${i}"), a: i, b: i, c: i, d: i }))
  }
}

fn shownInts<C: Allocator>(ctx: C, xs: [Int]): Str {
  xs.foldCtx(ctx, fn(c, acc: Str, i) => acc.concat(c, str.format(c, "${i},")), "")
}

fn shownRecs<C: Allocator>(ctx: C, xs: [Rec]): Str {
  xs.foldCtx(ctx, fn(c, acc: Str, r) => acc.concat(c, str.format(c, "${r.name}=${r.a + r.b + r.c + r.d},")), "")
}

fn lines<C: Allocator>(ctx: C, n: Int, acc: Str): Str {
  if (n > 12) { acc } else {
    let line = str.format(ctx, "${n}: ${words(ctx, 0, n, []).join(ctx, ",")} ${shownInts(ctx, ints(ctx, 0, n, []))} ${shownRecs(ctx, recs(ctx, 0, n, []))}\n");
    lines(ctx, n + 1, acc.concat(ctx, line))
  }
}

fn longer<C: Allocator>(ctx: C, xs: [Str]): Int {
  xs.push(ctx, str.format(ctx, "gone")).length()
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let prefix = words(ctx, 0, 2, []);
  let left = prefix.push(ctx, str.format(ctx, "left"));
  let right = prefix.push(ctx, str.format(ctx, "right"));
  let literal = ["a", "b"].push(ctx, str.format(ctx, "c"));
  let after = words(ctx, 0, 1, []);
  let dead = longer(ctx, after);
  let kept = after.push(ctx, str.format(ctx, "kept"));
  let shared = str.format(ctx, "${left.join(ctx, ",")} ${right.join(ctx, ",")} ${prefix.join(ctx, ",")} ${literal.join(ctx, ",")} ${dead} ${kept.join(ctx, ",")}\n");
  io.print(ctx, lines(ctx, 1, "").concat(ctx, shared)).mapErr(fn(_e) => "stdout")
}
"#;

/// What [`BUILT`] prints, worked out by hand.
fn built() -> String {
    let line = |n: usize| {
        let words: Vec<String> = (0..n).map(|i| format!("w{i}")).collect();
        let ints: String = (0..n).map(|i| format!("{},", i * 7)).collect();
        let recs: String = (0..n).map(|i| format!("r{i}={},", i * 4)).collect();
        format!("{n}: {} {ints} {recs}\n", words.join(","))
    };
    let lines: String = (1..=12).map(line).collect();
    format!("{lines}w0,w1,left w0,w1,right w0,w1 a,b,c 2 w0,kept\n")
}

/// **A list built by pushes holds what was pushed, at every length.** The
/// first push onto the empty list leaves room for more, so the next few
/// pushes write in place. Texts, `Int`s and records, lengths 1 to 12, cover a
/// block's first room and past it, and the lists from one prefix, a literal and a
/// prefix a longer list wrote past each keep their own elements. Under the
/// heap check, so a text left in a block's room would be released twice.
#[test]
fn a_list_built_by_pushes_holds_what_was_pushed() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("short-lists-built", BUILT);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, built(), "{backend}");
    }
}

/// `main` takes a count `k`, builds `k` lists of three elements, each pushed
/// onto the empty list, and prints the sum of their lengths. `ELEMENT` is
/// what each push adds.
const THREES: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn three<C: Allocator>(ctx: C, k: Int): Int {
  [].push(ctx, ELEMENT).push(ctx, ELEMENT).push(ctx, ELEMENT).length()
}

fn rounds<C: Allocator>(ctx: C, k: Int, acc: Int): Int {
  if (k == 0) { acc } else { rounds(ctx, k - 1, acc + three(ctx, k)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(ctx, k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of lists [`THREES`] builds.
const THREE_COUNTS: (u64, u64) = (20_000, 120_000);

/// **A short list built by pushes is copied once, not twice.** The first push
/// onto the empty list made a block with room for exactly one element, so the
/// second push copied it to a bigger block, and a third of the pushes that
/// build short lists were copies. Then the first block got 64 bytes, which
/// hold one element of 48, so a list of three of those was still copied
/// twice. Now the first block has twice the room it needs, and at least 64
/// bytes.
///
/// The bounds are on the instructions one list of three costs, the loop's
/// step and the frees included. On macOS, with an exact first block, then
/// one of 64 bytes, then the room it has now:
///
/// | backend, element | exact | 64 bytes | now | bound |
/// | --- | --- | --- | --- | --- |
/// | stencil, `Int` | 545 | 371 | 376 | 458 |
/// | stencil, text | 1,173 | 915 | 919 | 1,044 |
/// | stencil, six `Int`s | | 760 | 599 | 680 |
/// | LLVM, `Int` | 466 | 296 | 300 | 381 |
/// | LLVM, text | 775 | 591 | 595 | 683 |
/// | LLVM, six `Int`s | | 650 | 493 | 571 |
///
/// Each bound sits halfway between the last two figures that differ, so a
/// copy coming back fails it by 11% or more, and the rest has the same room
/// to grow. The pushes are the runtime's, so every backend is bounded.
#[test]
fn a_short_list_built_by_pushes_is_copied_once() {
    let (fewer, more) = THREE_COUNTS;
    let lists = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let llvm = backend == "llvm";
        for (mode, element, bound) in [
            ("ints", "k", if llvm { 381.0 } else { 458.0 }),
            ("texts", "\"t\"", if llvm { 683.0 } else { 1_044.0 }),
            (
                "records",
                "(k, k, k, k, k, k)",
                if llvm { 571.0 } else { 680.0 },
            ),
        ] {
            let binary = build(
                &format!("short-lists-{mode}"),
                &THREES.replace("ELEMENT", element),
            );
            let n = steps_cost(backend, &binary, THREE_COUNTS, |k| format!("{}\n", k * 3));
            let per = n.map(|n| n as f64 / lists as f64);
            eprintln!("{backend}, {mode}: {per:?} instructions a list");
            if per.is_some_and(|per| per > bound) {
                failures.push(format!(
                    "{backend} {mode}: {per:?} instructions a list, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
