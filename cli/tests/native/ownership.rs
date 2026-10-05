//! What a value taken apart, handed on, or read by a guard owes, natively, on
//! every native backend this toolchain has built in.
//!
//! A tuple or record taken apart by a pattern, or a list a fold's step hands to
//! the function that grows it: when nothing reads the whole again, the push
//! must find the list's count at one and grow it in place (MEMORY.md §5.3). A
//! match arm whose guard reads a heap value it bound and then fails must leave
//! that value for the arm the match falls through to.
//!
//! The work is counted rather than timed, through the allocation probe: a push
//! that copies allocates a block per push, and one that grows in place
//! allocates a block per doubling. Every row runs under the heap check.

use crate::shared::{probed, ran_checked, Ran};

/// One program on every backend, under the heap check.
fn run_each(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    crate::e2e::probed_backends()
        .into_iter()
        .map(|(backend, build)| (backend, ran_checked(&build(name, source))))
        .collect()
}

/// Two thousand pushes each through a tuple taken apart by `let`, by `match`,
/// and a record taken apart by `let`, both driven by a fold and by recursion
/// (buri-lang/buri#236). The whole is never read after the pattern, so each
/// push finds its list unique.
///
/// The strings pushed are literals, which allocate nothing, so every block
/// counted is a list block.
#[test]
fn a_list_taken_out_of_a_dying_tuple_by_a_pattern_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Acc { n: Int, items: [Str] }

fn step<C: Allocator>(ctx: C, acc: (Int, [Str]), _i: Int): (Int, [Str]) {
  let (n, items) = acc;
  (n + 1, items.push(ctx, "t"))
}

fn matched<C: Allocator>(ctx: C, acc: (Int, [Str]), _i: Int): (Int, [Str]) {
  match (acc) {
    (n, items) => (n + 1, items.push(ctx, "m")),
  }
}

fn record<C: Allocator>(ctx: C, acc: Acc, _i: Int): Acc {
  let Acc { n, items } = acc;
  Acc { n: n + 1, items: items.push(ctx, "r") }
}

fn folded<C: Allocator>(ctx: C, count: Int): Int {
  let (_n, items) = list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc, i) => step(c, acc, i), (0, list.empty<Str>()));
  items.length()
}

fn recursed<C: Allocator>(ctx: C, acc: (Int, [Str]), left: Int): Int {
  if (left == 0) { acc.1.length() } else { recursed(ctx, step(ctx, acc, left), left - 1) }
}

fn byMatch<C: Allocator>(ctx: C, acc: (Int, [Str]), left: Int): Int {
  if (left == 0) { acc.1.length() } else { byMatch(ctx, matched(ctx, acc, left), left - 1) }
}

fn byRecord<C: Allocator>(ctx: C, acc: Acc, left: Int): Int {
  if (left == 0) { acc.items.length() } else { byRecord(ctx, record(ctx, acc, left), left - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let a = folded(ctx, 2000);
  let b = recursed(ctx, (0, list.empty<Str>()), 2000);
  let c = byMatch(ctx, (0, list.empty<Str>()), 2000);
  let d = byRecord(ctx, Acc { n: 0, items: list.empty<Str>() }, 2000);
  let _ = io.println(host.stdout, "${a} ${b} ${c} ${d}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("pattern-push-loop", source) {
        assert_eq!(r.stdout, "2000 2000 2000 2000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 150,
            "{backend}: eight thousand pushes through taken-apart tuples and records allocated \
             {blocks} blocks: every push copied its list"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// Two thousand pushes by a function a fold's step hands its list accumulator
/// to (buri-lang/buri#237). The step never reads the list again, so each push
/// finds it unique.
#[test]
fn a_list_a_fold_step_hands_to_a_function_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn addOne<C: Allocator>(ctx: C, items: [Str]): [Str] {
  items.push(ctx, "item")
}

fn viaCall<C: Allocator>(ctx: C, count: Int): Int {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, items, _i) => addOne(c, items), list.empty<Str>()).length()
}

export fn main(host: NativeHost): Result<(), Str> {
  let _ = io.println(host.stdout, "${viaCall(host.alloc, 2000)}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("fold-step-hands-list", source) {
        assert_eq!(r.stdout, "2000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 60,
            "{backend}: two thousand pushes allocated {blocks} blocks: the fold's step kept a \
             second reference to the list it handed on, so every push copied it"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// A match arm binds a record holding a heap `Str`, its guard reads the record
/// and fails, and the match falls through to the wildcard (buri-lang/buri#231).
/// The value must be released once, by the arm that ran: released by the guard
/// as well, it was freed twice and the next allocations reused it.
#[test]
fn a_guard_that_fails_on_a_bound_heap_value_falls_through_cleanly() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

struct Stamp { at: Int, node: Str }

fn decoded<C: Allocator>(ctx: C, at: Int): Option<Stamp> {
  .Some(Stamp { at, node: str.format(ctx, "node-${at}") })
}

fn later<C: Allocator>(ctx: C, stamp: Option<Stamp>, limit: Int): [Str] {
  match (stamp) {
    .Some(found) if found.at > limit => [],
    _ => [str.format(ctx, "a-${limit}"), str.format(ctx, "b-${limit}")],
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let total = [1, 2, 3, 4, 5, 6, 7, 8].foldCtx(ctx, fn(c, sum, i) => sum + later(c, decoded(c, i), 100).length(), 0);
  let node = match (decoded(ctx, 2)) { .Some(s) => s.node, .None => "none" };
  let _ = io.println(host.stdout, "${total} ${node}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("guard-falls-through", source) {
        assert_eq!(r.stdout, "16 node-2\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}
