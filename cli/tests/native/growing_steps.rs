//! A string grown by short pieces, a list pushed onto or written in place, and
//! an ordered map inserted into or updated, natively, on every native backend
//! built in: what each step answers, owned and shared, and what an owned step
//! costs, counted rather than timed.
//!
//! Every program runs as it ships and once more under the heap check.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, ran_command};

/// Enough steps that the work dwarfs launching the process.
const STEPS: u64 = 1_000_000;

/// `main` takes the mode and the number of steps, and prints one number.
const STEPPED: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import { OrderedMap };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Wide { a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, k: Int }

fn grown<C: Allocator>(ctx: C, s: Str, piece: Str, i: Int, n: Int): Str {
  if (i >= n) { s } else { grown(ctx, s.concat(ctx, piece), piece, i + 1, n) }
}

fn read<C: Allocator>(ctx: C, s: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc + s.length() } else {
    let t = s.concat(ctx, "ab\u{e9}");
    let seen = if (t.charAt(t.length() / 2).isSome()) { 1 } else { 0 };
    read(ctx, t, i + 1, n, acc + seen)
  }
}

fn forked<C: Allocator>(ctx: C, s: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc + s.length() } else {
    let t = s.concat(ctx, "xy");
    forked(ctx, s.concat(ctx, "z"), i + 1, n, acc + t.length())
  }
}

fn pushed<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int): [Int] {
  if (i >= n) { xs } else { pushed(ctx, xs.push(ctx, i), i + 1, n) }
}

fn branched<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc + xs.length() } else {
    let ys = xs.push(ctx, i);
    branched(ctx, xs.concat(ctx, [i, i]), i + 1, n, acc + ys.length())
  }
}

fn replaced<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int): [Int] {
  if (i >= n) { xs } else { replaced(ctx, xs.replaceAt(ctx, i % 64, i), i + 1, n) }
}

fn kept<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc + xs.sum() } else {
    let ys = xs.replaceAt(ctx, i % 64, i);
    kept(ctx, ys, i + 1, n, acc + xs[i % 64].withDefault(0))
  }
}

fn inserted<C: Allocator>(ctx: C, m: OrderedMap<Int, Int>, i: Int, n: Int): OrderedMap<Int, Int> {
  if (i >= n) { m } else { inserted(ctx, m.insert(ctx, i * 7919 % 1000, i), i + 1, n) }
}

fn updated<C: Allocator>(ctx: C, m: OrderedMap<Int, Int>, i: Int, n: Int): OrderedMap<Int, Int> {
  if (i >= n) { m } else { updated(ctx, m.update(ctx, i * 7919 % 1000, fn(v) => v + i), i + 1, n) }
}

fn widened<C: Allocator>(ctx: C, m: OrderedMap<Int, Wide>, i: Int, n: Int): OrderedMap<Int, Wide> {
  if (i >= n) { m } else {
    widened(ctx, m.update(ctx, i * 7919 % 1000, fn(w) => Wide { a: w.a + i, b: w.b, c: w.c, d: w.d, e: w.e, f: w.f, g: w.g, h: w.h, k: w.k + 1 }), i + 1, n)
  }
}

fn persisted<C: Allocator>(ctx: C, m: OrderedMap<Int, Int>, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc + m.length() } else {
    let next = m.insert(ctx, i * 7919 % 1000, i);
    persisted(ctx, next, i + 1, n, acc + m.length())
  }
}

fn values(m: OrderedMap<Int, Int>): Int {
  m.fold(fn(acc, _k, v) => acc + v, 0)
}

fn keyed<C: Allocator>(ctx: C): OrderedMap<Int, Int> {
  orderedmap.of(ctx, list.range(ctx, 0, 1000).map(ctx, fn(i) => (i, 0)))
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let answer = match (args.first()) {
    .Some("concat") => grown(ctx, "", "abcdefgh", 0, n).length(),
    .Some("concat-wide") => grown(ctx, "", "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz", 0, n).length(),
    .Some("concat-indexed") => read(ctx, "", 0, n, 0),
    .Some("concat-shared") => forked(ctx, "", 0, n, 0),
    .Some("push") => pushed(ctx, [], 0, n).length(),
    .Some("push-shared") => branched(ctx, [], 0, n, 0),
    .Some("replace") => replaced(ctx, list.repeat(ctx, 0, 64), 0, n).sum(),
    .Some("replace-shared") => kept(ctx, list.repeat(ctx, 0, 64), 0, n, 0),
    .Some("insert") => values(inserted(ctx, orderedmap.empty(), 0, n)),
    .Some("update") => values(updated(ctx, keyed(ctx), 0, n)),
    .Some("update-wide") => {
      let wide = orderedmap.of(ctx, list.range(ctx, 0, 1000).map(ctx, fn(i) => (i, Wide { a: 0, b: 1, c: 2, d: 3, e: 4, f: 5, g: 6, h: 7, k: 0 })));
      widened(ctx, wide, 0, n).fold(fn(acc, _k, w) => acc + w.a + w.k + w.h, 0)
    },
    .Some("insert-shared") => persisted(ctx, orderedmap.empty(), 0, n, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// What each mode prints after `n` steps, worked out step by step.
fn answer(mode: &str, n: u64) -> u64 {
    let key = |i: u64| i * 7919 % 1000;
    match mode {
        "concat" => 8 * n,
        "concat-wide" => 78 * n,
        // Each step adds three scalars, and the middle one is always there.
        "concat-indexed" => n + 3 * n,
        "concat-shared" => (0..n).map(|i| i + 2).sum::<u64>() + n,
        "push" => n,
        "push-shared" => (0..n).map(|i| 2 * i + 1).sum::<u64>() + 2 * n,
        "replace" => (n.saturating_sub(64)..n).sum(),
        "replace-shared" => {
            let mut xs = [0u64; 64];
            let mut acc = 0;
            for i in 0..n {
                acc += xs[(i % 64) as usize];
                xs[(i % 64) as usize] = i;
            }
            acc + xs.iter().sum::<u64>()
        }
        "insert" => {
            let mut m = std::collections::BTreeMap::new();
            for i in 0..n {
                m.insert(key(i), i);
            }
            m.values().sum()
        }
        "update" => (0..n).sum(),
        "update-wide" => (0..n).sum::<u64>() + n + 7 * 1000,
        "insert-shared" => {
            let mut m = std::collections::BTreeSet::new();
            let mut acc = 0;
            for i in 0..n {
                acc += m.len() as u64;
                m.insert(key(i));
            }
            acc + m.len() as u64
        }
        _ => 0,
    }
}

/// **Every step answers what it did, whichever path the runtime took for it.**
/// A piece of 8 bytes or of 78, appended to a string that owns its block, one
/// whose scalars were just read, and one still held elsewhere; a push and a
/// concatenation onto a list owned and shared; `replaceAt` on a list owned and
/// shared; and an ordered map inserted into, updated with values of one word
/// and of nine, and inserted into while the map before is still read.
#[test]
fn growing_a_value_answers_the_same_owned_or_shared() {
    let n = 3000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("growing-steps", STEPPED);
        for mode in [
            "concat",
            "concat-wide",
            "concat-indexed",
            "concat-shared",
            "push",
            "push-shared",
            "replace",
            "replace-shared",
            "insert",
            "update",
            "update-wide",
            "insert-shared",
        ] {
            let ran = ran_command(heap_checked(
                std::process::Command::new(&binary).args([mode, &n.to_string()]),
            ));
            let want = format!("{}\n", answer(mode, n));
            if ran.status != 0 || ran.stdout != want {
                failures.push(format!(
                    "{backend} {mode}: exit {}, printed {:?}, want {want:?}\n{}",
                    ran.status, ran.stdout, ran.stderr
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// **An owned step calls nothing it doesn't need.** A string's append of a
/// short piece, a list's push and its `replaceAt` each have a path for a
/// block the step owns and has room in. Each of those paths once ran the whole
/// function's entry, saving every register any path used, and called out for
/// what only the other paths need: a scalar index's release, a long copy, a
/// check against blocks freed early. Those moved to functions of their own, so
/// the common path saves next to nothing.
///
/// In instructions a step on the copy-and-patch backend, an 8-byte append went
/// from 157 to 124, a push from 150 to 121, and a `replaceAt` from 102 to 91.
/// Each bound sits about halfway. LLVM open-codes an append and inlines the
/// rest, so only the copy-and-patch backend is bounded.
#[test]
fn an_owned_step_saves_next_to_nothing() {
    let steps = STEPS.to_string();
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("growing-steps", STEPPED);
        for (mode, bound) in [("concat", 140), ("push", 135), ("replace", 96)] {
            let want = format!("{}\n", answer(mode, STEPS));
            let (n, blocks) = per_op(backend, &binary, mode, &[&steps], STEPS, &want);
            eprintln!("{backend}, {mode}: {n:?} instructions a step, {blocks} blocks");
            let over = n.filter(|&n| backend == "stencil" && n > bound);
            if let Some(n) = over {
                failures.push(format!(
                    "{backend} {mode}: {n} instructions a step, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
