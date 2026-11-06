//! Programs that allocate and free a block on every step, natively, on every
//! native backend built in: what a block costs, counted rather than timed, and
//! blocks that are built on one thread, freed on another and moved between
//! actors still reading right.
//!
//! Every program runs as it ships and once more under the heap check, which
//! takes the runtime's other path for every allocation and free.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, probed, ran_command};

/// Enough steps that the work, at least 110 M instructions, dwarfs the 15 M
/// that launching any process costs. What launching costs moves with the
/// machine's load: at 100,000 steps a 7% move in it read as 10 more
/// instructions a step.
const STEPS: u64 = 1_000_000;

/// One short-lived block a step, built two ways: a list from `list.range`, and
/// a list literal. `main` takes the mode and the number of steps.
const CHURN: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn ranges<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { ranges(ctx, i + 1, n, acc + list.range(ctx, i, i + 2).length()) }
}

fn literals<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { literals(ctx, i + 1, n, acc + [i, i + 1].length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let answer = match (args.first()) {
    .Some("range") => ranges(ctx, 0, n, 0),
    .Some("literal") => literals(ctx, 0, n, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A step that allocates a small block and frees it reads the thread's cache
/// without a call.** On macOS both read it through dyld's thread-local lookup,
/// about 35 instructions a pair. A `list.range` step went from 245 to 210
/// instructions on stencil and from 198 to 164 on LLVM; a list literal step
/// from 233 to 208 and from 141 to 109. Each bound sits about halfway.
#[test]
fn allocating_and_freeing_a_small_block_costs_a_few_dozen_instructions() {
    let steps = STEPS.to_string();
    let want = format!("{}\n", STEPS * 2);
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("alloc-churn", CHURN);
        for (mode, stencil, llvm) in [("range", 228, 182), ("literal", 220, 126)] {
            let (n, blocks) = per_op(backend, &binary, mode, &[&steps], STEPS, &want);
            eprintln!("{backend}, {mode}: {n:?} instructions a step, {blocks} blocks");
            assert!(
                blocks >= STEPS,
                "{backend} {mode}: {STEPS} steps made only {blocks} blocks"
            );
            let bound = if backend == "llvm" { llvm } else { stencil };
            if let Some(n) = n.filter(|&n| n > bound) {
                failures.push(format!(
                    "{backend} {mode}: {n} instructions a step, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Eight parallel steps a round build lists of strings, each on whichever
/// thread runs it, and the caller sends each list to an actor. The actor keeps
/// the last sixteen strings and frees the rest, so a block is often freed on a
/// thread other than the one that built it. 500 rounds; the answer is the sum
/// of the lengths the actor kept after each message.
const MOVERS: &str = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn words<C: Allocator>(ctx: C, i: Int): [Str] {
  list.range(ctx, 0, 1 + i % 5).mapCtx(ctx, fn(c, k) => str.format(c, "w${i}.${k}"))
}

fn kept<C: Allocator>(ctx: C, s: [Str], m: [Str]): [Str] {
  let grown = m.foldCtx(ctx, fn(c, acc: [Str], w) => acc.push(c, w), s);
  if (grown.length() > 16) { grown.drop(ctx, grown.length() - 16) } else { grown }
}

fn rounds<C: Allocator + Tasks>(ctx: C, address: Address<C, [Str], [Str], Int>, r: Int, n: Int, acc: Int): Int {
  if (r >= n) {
    acc
  } else {
    let batches = tasks.parallel(ctx, list.range(ctx, 0, 8), fn(c, _i, x) => words(c, r * 8 + x));
    let answered = batches.foldCtx(ctx, fn(c, a, b) => a + address.sendMessage(c, b).withDefault(0), 0);
    rounds(ctx, address, r + 1, n, (acc + answered) % 1000003)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let address = actor.start(ctx, Actor {
    state: list.empty(),
    step: fn(c, s, m) => {
      let next = kept(c, s, m);
      Stepped { state: next, answer: next.sumOf(fn(w) => w.length()) }
    },
  });
  let total = rounds(ctx, address, 0, 500, 0);
  let _ = address.stop(ctx).isOk();
  io.println(ctx, "${total}").mapErr(fn(_e) => "stdout")
}
"#;

/// Blocks built on one thread, freed on another and moved between actors read
/// right, and every one is freed by the end, as the program ships and under the
/// heap check.
#[test]
fn blocks_moved_between_threads_and_actors_read_right_and_are_all_freed() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("alloc-movers", MOVERS);
        for checked in [false, true] {
            let mut cmd = std::process::Command::new(&binary);
            if checked {
                heap_checked(&mut cmd);
            }
            let r = ran_command(&mut cmd);
            assert_eq!(r.status, 0, "{backend} checked={checked}: {}", r.stderr);
            assert_eq!(
                r.stdout, "429925\n",
                "{backend} checked={checked}: {}",
                r.stderr
            );
            let (blocks, live) = probed(&r.stderr);
            assert!(blocks > 20_000, "{backend}: only {blocks} blocks");
            assert_eq!(
                live, 0,
                "{backend} checked={checked}: blocks still live at exit"
            );
        }
    }
}
