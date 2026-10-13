//! What a value taken apart, handed on, or read by a guard owes, natively, on
//! every native backend this toolchain has built in.
//!
//! A tuple or record taken apart by a pattern, or a list a fold's step hands to
//! the function that grows it: when nothing reads the whole again, the push
//! must find the list's count at one and grow it in place (MEMORY.md §5.3). A
//! match arm whose guard reads a heap value it bound and then fails must leave
//! that value for the arm the match falls through to.
//! A match that rebuilds a value nothing reads again allocates nothing new,
//! and gives back the old value's hold on the payloads it moved.
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
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
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
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// Two thousand pushes by a function too large to inline, handed the list a
/// step took out of its tuple or record accumulator with a destructuring
/// `let`. The function may hand the list back untouched, so it owns it; the
/// step must give it the accumulator's count rather than a second one, or every
/// push copies. Found by the growth generator, seed `0x67726f777468`, case 147.
#[test]
fn a_list_a_let_took_apart_and_handed_to_an_owning_function_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Acc { n: Int, items: [Int] }

fn put<C: Allocator>(ctx: C, items: [Int], k: Int): [Int] {
  if (k < 0) { items } else { items.push(ctx, k) }
}

fn tupled<C: Allocator>(ctx: C, acc: (Int, [Int]), i: Int): (Int, [Int]) {
  let (n, items) = acc;
  (n + 1, if (i % 2 == 0) { put(ctx, items, i) } else { put(ctx, items, i + 1) })
}

fn recorded<C: Allocator>(ctx: C, acc: Acc, i: Int): Acc {
  let Acc { n, items } = acc;
  Acc { n: n + 1, items: if (i % 2 == 0) { put(ctx, items, i) } else { put(ctx, items, i + 1) } }
}

fn byTuple<C: Allocator>(ctx: C, count: Int): Int {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc, i) => tupled(c, acc, i), (0, list.empty<Int>())).1.length()
}

fn byRecord<C: Allocator>(ctx: C, acc: Acc, left: Int): Int {
  if (left == 0) { acc.items.length() } else { byRecord(ctx, recorded(ctx, acc, left), left - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let a = byTuple(ctx, 2000);
  let b = byRecord(ctx, Acc { n: 0, items: list.empty<Int>() }, 2000);
  let _ = io.println(host.stdout, "${a} ${b}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("let-apart-owning-call", source) {
        assert_eq!(r.stdout, "2000 2000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 80,
            "{backend}: four thousand pushes allocated {blocks} blocks: the step kept a count on \
             the accumulator while the function it handed the list to pushed, so every push copied"
        );
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// Two thousand pushes by a one-line function a fold's step hands the list in
/// its tuple accumulator to, as `acc.1`. The inliner pastes the function in
/// and binds its parameter with a `let`; the step was lent its accumulator, so
/// the name needs no count of its own, and with one the push copied. Found by
/// the growth generator, seed `0x67726f777468`, case 289.
#[test]
fn a_list_read_out_of_a_lent_tuple_into_an_inlined_function_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn add<C: Allocator>(ctx: C, items: [Int], k: Int): [Int] { items.push(ctx, k) }

fn step<C: Allocator>(ctx: C, acc: (Int, [Int]), i: Int): (Int, [Int]) {
  (acc.0 + 1, add(ctx, acc.1, i))
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let acc = list.range(ctx, 0, 2000).foldCtx(ctx, fn(c, acc, i) => step(c, acc, i), (0, list.empty<Int>()));
  let _ = io.println(host.stdout, "${acc.0} ${acc.1.length()}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("inlined-tuple-field", source) {
        assert_eq!(r.stdout, "2000 2000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 40,
            "{backend}: two thousand pushes allocated {blocks} blocks: the inlined function's \
             parameter took a count of its own on the lent list, so every push copied it"
        );
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// The same, from a step that can fail, with the list named by a `let` first
/// and the name handed on: the inliner binds the function's parameter to the
/// name in a block inside, so forwarding the outer name has to reach the inner
/// `let` too. Found by the growth generator, seed `0x67726f777468`, case 184.
#[test]
fn a_list_named_and_then_handed_to_an_inlined_function_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn add<C: Allocator>(ctx: C, items: [Int], k: Int): [Int] { items.push(ctx, k) }

fn gate(i: Int): Result<Int, Str> {
  if (i < 0) { .Err("negative") } else { .Ok(i % 5) }
}

fn step<C: Allocator>(ctx: C, acc: (Int, [Int]), i: Int): Result<(Int, [Int]), Str> {
  let q = gate(i)?;
  let held = acc.1;
  .Ok((acc.0 + 1, add(ctx, held, i + q)))
}

fn run<C: Allocator>(ctx: C, acc: (Int, [Int]), count: Int): Result<(Int, [Int]), Str> {
  if (count == 0) { .Ok(acc) } else { step(ctx, run(ctx, acc, count - 1)?, count - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let acc = run(host.alloc, (0, list.empty<Int>()), 2000)?;
  let _ = io.println(host.stdout, "${acc.0} ${acc.1.length()}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("named-inlined-tuple-field", source) {
        assert_eq!(r.stdout, "2000 2000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 40,
            "{backend}: two thousand pushes allocated {blocks} blocks: the inlined function's \
             parameter took a count of its own on the list, so every push copied it"
        );
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// Two thousand steps that each push two elements onto the list a fold lends
/// them, as a chain, through a `let`, and out of a tuple. The first push grows the
/// list in place, so its answer is a second count on the block the fold still
/// holds; the step has to take the list for the second push to find it unique.
/// Found by the growth generator, seed `0x67726f777468`, case 309, and
/// exploring seed 5, case 302.
#[test]
fn a_list_a_fold_step_pushes_twice_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn chained<C: Allocator>(ctx: C, count: Int): Int {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc: [Int], i) => acc.push(c, i).push(c, i + 1), list.empty<Int>()).length()
}

fn named<C: Allocator>(ctx: C, count: Int): Int {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc: [Int], i) => {
    let once = acc.push(c, i);
    once.push(c, i + 1)
  }, list.empty<Int>()).length()
}

fn tupled<C: Allocator>(ctx: C, count: Int): Int {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc: (Int, [Int]), i) => (acc.0 + 1, acc.1.push(c, i).push(c, i + 1)), (0, list.empty<Int>())).1.length()
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let _ = io.println(host.stdout, "${chained(ctx, 2000)} ${named(ctx, 2000)} ${tupled(ctx, 2000)}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("fold-step-pushes-twice", source) {
        assert_eq!(r.stdout, "4000 4000 4000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 80,
            "{backend}: twelve thousand pushes allocated {blocks} blocks: the step's second push \
             found the list still lent to it, so every step copied it"
        );
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
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

/// Two arms on one variant, each binding both of its `Int` payloads, the first
/// guarded by comparing them (buri-lang/buri#239). The guard and the arm it
/// falls through to must both read the second payload as it was built.
#[test]
fn a_guard_comparing_a_variants_two_payloads_reads_both() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

enum Span {
  Numbers(Int, Int),
  Name(Str),
}

fn shown<C: Allocator>(ctx: C, span: Span): Str {
  match (span) {
    .Numbers(first, last) if first == last => str.format(ctx, "${first}"),
    .Numbers(first, last) => str.format(ctx, "${first} to ${last}"),
    .Name(name) => str.format(ctx, "'${name}'"),
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let name = str.format(ctx, "n-${7}");
  let line = "${shown(ctx, Span.Numbers(9, 9))}, ${shown(ctx, Span.Numbers(4, 6))}, ${shown(ctx, Span.Name(name))}";
  let _ = io.println(host.stdout, line).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("guard-two-payloads", source) {
        assert_eq!(r.stdout, "9, 4 to 6, 'n-7'\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}

/// A `..rest` binding is a list the arm allocates before its guard runs. When
/// the guard is false the match moves on to the next arm, and the list must be
/// released on the way, whether the guard read it, the body reads it, or
/// neither does.
#[test]
fn a_rest_binding_is_released_when_its_arms_guard_fails() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

fn unread(items: [Str], flag: Bool): Str {
  match (items) {
    [_, second, ..rest] if flag => second,
    _ => "unread",
  }
}

fn guardOnly(items: [Str]): Str {
  match (items) {
    [_, ..rest] if rest.length() > 5 => "long",
    _ => "short",
  }
}

fn both<C: Allocator>(ctx: C, items: [Str]): Str {
  match (items) {
    [first, ..rest] if !rest.isEmpty() && first == "z" => str.format(ctx, "${rest.length()}"),
    _ => "both",
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let items = list.empty<Str>().push(ctx, "a").push(ctx, "b").push(ctx, "c");
  let line = "${unread(items, false)} ${guardOnly(items)} ${both(ctx, items)} ${unread(items, true)}";
  let _ = io.println(host.stdout, line).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("rest-guard-fails", source) {
        assert_eq!(r.stdout, "unread short both b\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}

/// A `..rest` binding followed by a test that fails, in the same pattern: the
/// match moves on to the next arm having allocated nothing it keeps.
#[test]
fn a_rest_binding_before_a_failing_test_leaves_nothing_behind() {
    let source = r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn pick(p: ([Str], Str)): Str {
  match (p) {
    ([_, ..rest], "x") => "x",
    ([first, ..rest], _) => first,
    _ => "other",
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let items = list.empty<Str>().push(ctx, "a").push(ctx, "b").push(ctx, "c");
  let line = "${pick((items, "y"))} ${pick((items, "x"))} ${pick((list.empty<Str>(), "y"))}";
  let _ = io.println(host.stdout, line).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("rest-then-test", source) {
        assert_eq!(r.stdout, "a x other\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}

/// Two thousand of each of `replaceAt`, `insertAt` and `removeAt` on a list
/// threaded through a loop, so each step owns it outright (#251). Each wrote a
/// whole copy, about 11 ns an element for a `[Str]`, where `push` on the same
/// list reused it.
///
/// Every element is a literal, which allocates nothing, so every block counted
/// is a list block.
#[test]
fn a_list_the_caller_owns_is_spliced_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn replaced<C: Allocator>(ctx: C, xs: [Str], left: Int): [Str] {
  if (left == 0) { xs } else { replaced(ctx, xs.replaceAt(ctx, 16, "r"), left - 1) }
}

fn moved<C: Allocator>(ctx: C, xs: [Str], left: Int): [Str] {
  if (left == 0) { xs } else { moved(ctx, xs.insertAt(ctx, 3, "i").removeAt(ctx, 20), left - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let a = replaced(ctx, list.repeat(ctx, "s", 32), 2000);
  let b = moved(ctx, list.repeat(ctx, "s", 32), 2000);
  let _ = io.println(host.stdout, "${a.length()} ${a.get(16).withDefault("")} ${b.length()} ${b.get(3).withDefault("")}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("owned-splice-loop", source) {
        assert_eq!(r.stdout, "32 r 32 i\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 50,
            "{backend}: six thousand splices of an owned list allocated {blocks} blocks: \
             every splice copied its list"
        );
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// A helper that recurses on the right of a short-circuit nested in another one
/// — `a && (b || self(...))` — and owns a string only `b` reads (#303). Each
/// shape is run down every path: `a` false, `b` true, and the recursion.
///
/// The string was released once by `b` and again after the whole expression,
/// so the program read a freed block and, in a test binary, died with
/// `SIGSEGV` in whichever test ran next.
#[test]
fn a_helper_recursing_inside_a_nested_short_circuit_releases_once() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn andOr<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n >= 0 && (k == "k0" || andOr(ctx, n - 1))
}

fn orAnd<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n < 0 || (k != "k0" && orAnd(ctx, n - 1))
}

fn andAnd<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n >= 0 && (k != "k0" && andAnd(ctx, n - 1))
}

fn deeper<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n >= 0 && (k == "k0" || (n > 0 && deeper(ctx, n - 1)))
}

fn lent<C: Allocator>(ctx: C, label: Str, n: Int): Bool {
  n >= 0 && (label == "k0" || lent(ctx, str.format(ctx, "k${n - 1}"), n - 1))
}

fn carried<C: Allocator>(ctx: C, label: Str, n: Int): Bool {
  let k = str.format(ctx, "${label}");
  n >= 0 && (k == "k0" || carried(ctx, k, n - 1))
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let a = str.format(ctx, "${andOr(ctx, 5)} ${andOr(ctx, -1)}");
  let b = str.format(ctx, "${orAnd(ctx, 5)} ${orAnd(ctx, -1)}");
  let c = str.format(ctx, "${andAnd(ctx, 5)} ${andAnd(ctx, -1)}");
  let d = str.format(ctx, "${deeper(ctx, 5)} ${deeper(ctx, -1)}");
  let e = str.format(ctx, "${lent(ctx, "k5", 5)} ${lent(ctx, "x", -1)}");
  let f = str.format(ctx, "${carried(ctx, "k0", 3)} ${carried(ctx, "x", 3)}");
  let _ = io.println(host.stdout, "${a} | ${b} | ${c} | ${d} | ${e} | ${f}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("short-circuit-recursion", source) {
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(
            r.stdout,
            "true false | false true | false false | true false | true false | true false\n",
            "{backend}: {}",
            r.stderr
        );
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// The same recursion one construct further in: the inner `||` is a `match`
/// arm, an `if` branch or a block's result, and the `&&` around it is what
/// keeps the string alive (#303).
#[test]
fn a_helper_recursing_under_a_branch_inside_a_short_circuit_releases_once() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn inArm<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n >= 0 && match (n) {
    99 => false,
    _ => k == "k0" || inArm(ctx, n - 1),
  }
}

fn inBranch<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n >= 0 && (if (n == 99) { false } else { k == "k0" || inBranch(ctx, n - 1) })
}

fn inBlock<C: Allocator>(ctx: C, n: Int): Bool {
  let k = str.format(ctx, "k${n}");
  n >= 0 && {
    let next = n - 1;
    k == "k0" || inBlock(ctx, next)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let a = str.format(ctx, "${inArm(ctx, 5)} ${inArm(ctx, -1)}");
  let b = str.format(ctx, "${inBranch(ctx, 5)} ${inBranch(ctx, -1)}");
  let c = str.format(ctx, "${inBlock(ctx, 5)} ${inBlock(ctx, -1)}");
  let _ = io.println(host.stdout, "${a} | ${b} | ${c}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("short-circuit-recursion-nested", source) {
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(
            r.stdout, "true false | true false | true false\n",
            "{backend}: {}",
            r.stderr
        );
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(
            live, 0,
            "{backend}: {blocks} blocks allocated and {live} still live at exit"
        );
    }
}

/// A match that takes apart a value nothing reads again and builds a new one
/// from its payloads, two thousand times over. The swapped value is a register
/// or stack value, so swapping it allocates nothing: the two `Str`s are the
/// only blocks the swaps touch, and they move from each value to the next.
#[test]
fn a_value_a_match_takes_apart_and_rebuilds_allocates_nothing_new() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

enum Pair { One(Str), Two(Str, Str) }

fn swap(p: Pair): Pair {
  match (p) {
    .One(a) => .One(a),
    .Two(a, b) => .Two(b, a),
  }
}

fn spin(p: Pair, left: Int): Pair {
  if (left == 0) { p } else { spin(swap(p), left - 1) }
}

fn first(p: Pair): Str {
  match (p) {
    .One(a) => a,
    .Two(a, _b) => a,
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let two = spin(Pair.Two(str.format(ctx, "left-${1}"), str.format(ctx, "right-${2}")), 2001);
  let one = spin(Pair.One(str.format(ctx, "only-${3}")), 2000);
  let _ = io.println(host.stdout, "${first(two)} ${first(one)}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("match-rebuilds-dying-value", source) {
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(r.stdout, "right-2 only-3\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 50,
            "{backend}: four thousand swaps allocated {blocks} blocks: a rebuilt value took a \
             block of its own"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// A thousand fresh values, each of heap `Str`s, each taken apart and rebuilt
/// by a match that is the last thing to read it. The payloads move into the
/// new value and the old value's hold on them is given back, so every block
/// is freed by exit.
#[test]
fn a_value_a_match_takes_apart_and_rebuilds_gives_its_payloads_back() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

enum Pair { One(Str), Two(Str, Str) }

fn swap(p: Pair): Pair {
  match (p) {
    .One(a) => .One(a),
    .Two(a, b) => .Two(b, a),
  }
}

fn fresh<C: Allocator>(ctx: C, i: Int): Pair {
  if (i % 2 == 0) {
    .Two(str.format(ctx, "a-${i}"), str.format(ctx, "bb-${i}"))
  } else {
    .One(str.format(ctx, "ccc-${i}"))
  }
}

fn churn<C: Allocator>(ctx: C, i: Int, seen: Int, last: Str): (Int, Str) {
  if (i == 0) {
    (seen, last)
  } else {
    let (n, head) = match (swap(fresh(ctx, i))) {
      .One(a) => (a.length(), a),
      .Two(a, b) => (a.length() + b.length(), a),
    };
    churn(ctx, i - 1, seen + n, head)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let (total, last) = churn(host.alloc, 1000, 0, "none");
  let _ = io.println(host.stdout, "${total} ${last}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("match-rebuild-releases", source) {
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(r.stdout, "8841 ccc-1\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// A match rebuilds a value and the old value is **read again afterwards**, so
/// it isn't dying where the new one is built. Both must read as they were
/// built, a thousand times over, with each round's blocks freed and handed to
/// the next round's allocations.
#[test]
fn a_value_read_after_a_match_rebuilt_it_keeps_its_payloads() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

enum Pair { One(Str), Two(Str, Str) }

fn first(p: Pair): Str {
  match (p) {
    .One(a) => a,
    .Two(a, _b) => a,
  }
}

fn both<C: Allocator>(ctx: C, i: Int): Str {
  let p = Pair.Two(str.format(ctx, "a-${i}"), str.format(ctx, "b-${i}"));
  let q = match (p) {
    .One(a) => Pair.One(a),
    .Two(a, b) => Pair.Two(b, a),
  };
  str.format(ctx, "${first(q)}/${first(p)}")
}

fn rounds<C: Allocator>(ctx: C, i: Int, bad: Int, last: Str): (Int, Str) {
  if (i == 0) {
    (bad, last)
  } else {
    let shown = both(ctx, i);
    let ok = shown == str.format(ctx, "b-${i}/a-${i}");
    rounds(ctx, i - 1, if (ok) { bad } else { bad + 1 }, shown)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let (bad, last) = rounds(host.alloc, 1000, 0, "none");
  let _ = io.println(host.stdout, "${bad} ${last}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("match-rebuild-read-after", source) {
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(r.stdout, "0 b-1/a-1\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}
