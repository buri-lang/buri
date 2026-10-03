//! A list grown in a program whose values may cross tasks, natively, on every
//! native backend this toolchain has built in.
//!
//! A program that reaches `core/actor`, `core/tasks` or `Tasks.parallel` marks
//! every block it allocates, and counts the marked ones atomically (MEMORY.md
//! §5.1). Issue #222: a marked list was never treated as unique, so every
//! `push` in such a program copied the whole list, even one no other task
//! could reach. Building a list was quadratic again, for `[Int]` as well as
//! `[Str]`, because the program started one actor somewhere.
//!
//! The work is counted rather than timed, through the allocation probe: a push
//! that copies allocates a block per push, and one that grows in place
//! allocates a block per doubling. Every row runs under the heap check.
//!
//! The other rows share a list between tasks and then grow it on more than one
//! of them at once. Each task's answer must be the old list and its own
//! element, and the old list must read as it did. On the development backend
//! the tasks run one after another, and the rows reach the same answers with
//! no contention.

use crate::shared::{probed, ran_checked, Ran};

/// One program on every backend, under the heap check.
fn run_each(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    crate::e2e::probed_backends()
        .into_iter()
        .map(|(backend, build)| (backend, ran_checked(&build(name, source))))
        .collect()
}

/// Two thousand pushes each onto an `[Int]`, a `[Str]` and a `[(Str, Int)]`,
/// then two thousand more inside an actor's step, in a program that starts the
/// actor only after the first three loops are done.
///
/// No list here is shared with anything, so every push finds its list unique
/// and the four lists allocate a block per doubling. The strings pushed are
/// literals, which allocate nothing.
#[test]
fn a_unique_push_loop_in_a_program_that_starts_an_actor_allocates_logarithmically() {
    let source = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Stepped };
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn ints<C: Allocator>(ctx: C, xs: [Int], i: Int): [Int] {
  if (i == 0) { xs } else { ints(ctx, xs.push(ctx, i), i - 1) }
}

fn words<C: Allocator>(ctx: C, xs: [Str], i: Int): [Str] {
  if (i == 0) { xs } else { words(ctx, xs.push(ctx, "w"), i - 1) }
}

fn pairs<C: Allocator>(ctx: C, xs: [(Str, Int)], i: Int): [(Str, Int)] {
  if (i == 0) { xs } else { pairs(ctx, xs.push(ctx, ("p", i)), i - 1) }
}

fn builder<C: Allocator + Tasks>(): Actor<C, Int, Int, Int> {
  Actor {
    state: 0,
    step: fn(c, built, n) => {
      let made = words(c, [], n);
      Stepped { state: built + made.length(), answer: made.length() }
    },
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let ns = ints(ctx, [], 2000);
  let ws = words(ctx, [], 2000);
  let ps = pairs(ctx, [], 2000);
  let last = match (ps.last()) { .Some(p) => p.1, .None => -1 };
  let address = actor.start(ctx, builder());
  let stepped = match (address.sendMessage(ctx, 2000)) {
    .Ok(n) => n,
    .Err(_gone) => -1,
  };
  let _ = address.stop(ctx).ignore();
  let _ = io.println(ctx, "${ns.length()} ${ws.length()} ${ps.length()} ${last} ${stepped}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("marked-push-loop", source) {
        assert_eq!(r.stdout, "2000 2000 2000 1 2000\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 150,
            "{backend}: eight thousand pushes in a program with an actor allocated {blocks} \
             blocks: a marked list was copied on every push"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// One list, held by one closure, grown by eight `Tasks.parallel` steps at
/// once, two hundred times over.
///
/// The closure's environment holds the list's only reference, and every step
/// borrows it, so every step reads a count of one off the same block. At most
/// one of them may write past the list's end. Each answer must be the list and
/// the step's own index, so a step that wrote into another step's answer shows
/// as a wrong last element. The list has headroom, because it was built by
/// pushes, so writing in place is always tempting.
///
/// Both element kinds, because they take different paths: an `[Int]` writes
/// any slot past the end, and a `[Str]` only an all-zero one.
#[test]
fn parallel_steps_growing_one_shared_list_each_see_only_their_own_element() {
    let source = r#"
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

fn ints<C: Allocator>(ctx: C, xs: [Int], i: Int): [Int] {
  if (i == 0) { xs } else { ints(ctx, xs.push(ctx, 7), i - 1) }
}

fn words<C: Allocator>(ctx: C, xs: [Str], i: Int): [Str] {
  if (i == 0) { xs } else { words(ctx, xs.push(ctx, str.fromInt(ctx, 7)), i - 1) }
}

fn intsRight<C: Allocator>(ctx: C, answers: [[Int]]): Int {
  answers.mapIndexed(ctx, fn(i, xs) => {
    let prefix = xs.count(fn(x) => x == 7) == 40;
    let own = xs.last() == .Some(i + 100);
    if (prefix && own && xs.length() == 41) { 1 } else { 0 }
  }).sum()
}

fn wordsRight<C: Allocator>(ctx: C, answers: [[Str]]): Int {
  answers.mapIndexedCtx(ctx, fn(c, i, xs) => {
    let prefix = xs.count(fn(x) => x == "7") == 40;
    let own = xs.last() == .Some(str.fromInt(c, i + 100));
    if (prefix && own && xs.length() == 41) { 1 } else { 0 }
  }).sum()
}

fn round<C: Allocator + Tasks>(ctx: C, k: Int, right: Int): Int {
  if (k == 0) {
    right
  } else {
    let xs = ints(ctx, [], 40);
    let ws = words(ctx, [], 40);
    let steps = [0, 1, 2, 3, 4, 5, 6, 7];
    let grownInts = tasks.parallel(ctx, steps, fn(c, i, _s) => xs.push(c, i + 100));
    let grownWords = tasks.parallel(ctx, steps, fn(c, i, _s) => ws.push(c, str.fromInt(c, i + 100)));
    round(ctx, k - 1, right + intsRight(ctx, grownInts) + wordsRight(ctx, grownWords))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let _ = io.println(ctx, "right ${round(ctx, 200, 0)} of 3200").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("marked-parallel-push", source) {
        assert_eq!(r.stdout, "right 3200 of 3200\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
    }
}

/// Lists shared with an actor through its state, its messages and its
/// answers, and grown on both sides after the sharing.
///
/// The actor keeps a `[Str]` and a `[Int]`. A message carries a list the sender
/// goes on growing; the step grows the state, and answers it; the sender grows
/// the answer. Every value read back afterwards must be the one it was when it
/// was shared.
#[test]
fn lists_shared_with_an_actor_keep_their_values_when_either_side_grows_them() {
    let source = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Stepped };
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Kept { words: [Str], counts: [Int] }

enum Keep {
  Add([Str]),
  Get,
}

fn keeper<C: Allocator + Tasks>(): Actor<C, Kept, Keep, Kept> {
  Actor {
    state: Kept { words: [], counts: [] },
    step: fn(c, kept, message) => {
      match (message) {
        .Add(more) => {
          let grown = Kept {
            words: kept.words.concat(c, more).push(c, "|"),
            counts: kept.counts.push(c, more.length()),
          };
          Stepped { state: grown, answer: grown }
        },
        .Get => Stepped { state: kept, answer: kept },
      }
    },
  }
}

fn joined<C: Allocator>(ctx: C, k: Kept): Str {
  str.format(ctx, "${k.words.join(ctx, "")}/${k.counts.mapCtx(ctx, fn(c, n) => str.fromInt(c, n)).join(ctx, ",")}")
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let address = actor.start(ctx, keeper());
  let sent = ["a", "b"];
  let first = match (address.sendMessage(ctx, .Add(sent))) {
    .Ok(k) => k,
    .Err(_gone) => Kept { words: [], counts: [] },
  };
  // The sender grows the list it sent, and the answer it got back.
  let sentMore = sent.push(ctx, "x");
  let firstMore = Kept { words: first.words.push(ctx, "y"), counts: first.counts.push(ctx, 9) };
  // The actor grows its state, which the first answer still shares.
  let second = match (address.sendMessage(ctx, .Add(["c"]))) {
    .Ok(k) => k,
    .Err(_gone) => Kept { words: [], counts: [] },
  };
  let now = match (address.sendMessage(ctx, .Get)) {
    .Ok(k) => k,
    .Err(_gone) => Kept { words: [], counts: [] },
  };
  let _ = address.stop(ctx).ignore();
  let _ = io.println(ctx, "sent ${sent.join(ctx, "")} ${sentMore.join(ctx, "")}").ignore();
  let _ = io.println(ctx, "first ${joined(ctx, first)} ${joined(ctx, firstMore)}").ignore();
  let _ = io.println(ctx, "second ${joined(ctx, second)} now ${joined(ctx, now)}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("marked-actor-sharing", source) {
        assert_eq!(
            r.stdout,
            "sent ab abx\nfirst ab|/2 ab|y/2,9\nsecond ab|c|/2,1 now ab|c|/2,1\n",
            "{backend}: {}",
            r.stderr
        );
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
    }
}

/// One list, held by one closure, grown by tasks spawned into a scope and by
/// the scope's body at the same time. In a `--release` build the tasks run
/// beside the body.
///
/// A spawned task returns nothing, so each one posts what it built to an actor.
/// Every list the actor holds afterwards must be the shared list and that
/// task's own element.
#[test]
fn scope_tasks_growing_one_shared_list_each_see_only_their_own_element() {
    let source = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "core/tasks" import { Scope };

enum Note {
  Built([Int]),
  Get,
}

fn notes<C: Allocator + Tasks>(): Actor<C, [[Int]], Note, [[Int]]> {
  Actor {
    state: [],
    step: fn(c, held, message) => {
      match (message) {
        .Built(xs) => Stepped { state: held.push(c, xs), answer: [] },
        .Get => Stepped { state: held, answer: held },
      }
    },
  }
}

fn ints<C: Allocator>(ctx: C, xs: [Int], i: Int): [Int] {
  if (i == 0) { xs } else { ints(ctx, xs.push(ctx, 7), i - 1) }
}

fn spawning<C: Allocator + Tasks>(
  ctx: C,
  here: Scope,
  kept: Address<C, [[Int]], Note, [[Int]]>,
  add: fn(C, Int) => [Int],
  i: Int,
): () {
  if (i == 0) {
    ()
  } else {
    let _ = tasks.spawn(ctx, here, fn(c) => {
      let grow = add;
      let _ = kept.sendMessage(c, .Built(grow(c, i + 100))).ignore();
      ()
    });
    spawning(ctx, here, kept, add, i - 1)
  }
}

fn right(all: [[Int]]): Int {
  all.count(fn(xs) => {
    let prefix = xs.count(fn(x) => x == 7) == 40;
    let own = match (xs.last()) { .Some(n) => n > 100 && n <= 108, .None => false };
    prefix && own && xs.length() == 41
  })
}

fn distinct(all: [[Int]]): Int {
  [101, 102, 103, 104, 105, 106, 107, 108]
    .count(fn(n) => all.any(fn(xs) => xs.last() == .Some(n)))
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let kept = actor.start(ctx, notes());
  let xs = ints(ctx, [], 40);
  let add = fn(c, n) => xs.push(c, n);
  let mine = tasks.scope(ctx, fn(c, here) => {
    let _ = spawning(c, here, kept, add, 8);
    let grow = add;
    grow(c, 99)
  });
  let all = match (kept.sendMessage(ctx, .Get)) {
    .Ok(held) => held,
    .Err(_gone) => [],
  };
  let _ = kept.stop(ctx).ignore();
  let _ = io.println(ctx, "tasks ${right(all)} of ${all.length()}, distinct ${distinct(all)}").ignore();
  let _ = io.println(ctx, "body ${mine.length()} ${mine.last().withDefault(-1)}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("marked-scope-push", source) {
        assert_eq!(
            r.stdout,
            "tasks 8 of 8, distinct 8\nbody 41 99\n",
            "{backend}: {}",
            r.stderr
        );
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
    }
}
