//! A list grown through a record's field, natively, on every native backend
//! this toolchain has built in.
//!
//! `Out { ..out, pieces: out.pieces.push(ctx, t) }` is how `core/buri/ast`
//! writes every piece it prints and every token it lexes. When `out` dies in
//! that update, the push must find the list's count at one and grow it in
//! place (MEMORY.md §5.3). When anything else still holds the record or the
//! list, the push must copy, and the old value must read as it did.
//!
//! The work is counted rather than timed, through the allocation probe: a push
//! that copies allocates a block per push, and one that grows in place
//! allocates a block per doubling. Every row runs under the heap check, so a
//! count taken and not given back, or a block freed while something still
//! reads it, fails the status.

use crate::shared::{probed, ran_checked, Ran};

/// One program on every backend, under the heap check.
fn run_each(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    crate::e2e::probed_backends()
        .into_iter()
        .map(|(backend, build)| (backend, ran_checked(&build(name, source))))
        .collect()
}

/// Two thousand pushes through a record's field, in three shapes: the
/// printer's (a list beside a number), the token preparer's (two lists grown
/// in one update, one of them from a third field), and a record nested in a
/// record. Each record dies in the update that grows it, so every push finds
/// its list unique and the six lists allocate a block per doubling.
///
/// The strings pushed are literals, which allocate nothing, so every block
/// counted is a list block.
#[test]
fn a_list_grown_through_a_dying_records_field_allocates_logarithmically() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Out { pieces: [Str], at: Int }

struct Prep { tokens: [Str], docs: [Int], pending: Int }

struct Outer { inner: Out, n: Int }

fn raw<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  Out { ..out, pieces: out.pieces.push(ctx, t), at: out.at + 1 }
}

fn write<C: Allocator>(ctx: C, out: Out, i: Int): Out {
  if (i == 0) { out } else { write(ctx, raw(ctx, out, "w"), i - 1) }
}

fn prepare<C: Allocator>(ctx: C, p: Prep, i: Int): Prep {
  if (i == 0) {
    p
  } else {
    prepare(
      ctx,
      Prep {
        ..p,
        tokens: p.tokens.push(ctx, "t"),
        docs: p.docs.push(ctx, p.pending),
        pending: p.pending + 1,
      },
      i - 1,
    )
  }
}

fn nested<C: Allocator>(ctx: C, o: Outer, i: Int): Outer {
  if (i == 0) {
    o
  } else {
    nested(ctx, Outer { ..o, inner: raw(ctx, o.inner, "n"), n: o.n + 1 }, i - 1)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let out = write(host.alloc, Out { pieces: [], at: 0 }, 2000);
  let p = prepare(host.alloc, Prep { tokens: [], docs: [], pending: 0 }, 2000);
  let o = nested(host.alloc, Outer { inner: Out { pieces: [], at: 0 }, n: 0 }, 2000);
  let last = match (p.docs.last()) { .Some(d) => d, .None => -1 };
  let _ = io.println(
    host.stdout,
    "${out.pieces.length()} ${out.at} ${p.tokens.length()} ${last} ${o.inner.pieces.length()} ${o.n}",
  ).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("field-push-loop", source) {
        assert_eq!(r.stdout, "2000 2000 2000 1999 2000 2000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 100,
            "{backend}: eight thousand pushes through record fields allocated {blocks} blocks: \
             the update kept a second count on the list, so every push copied it"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// Two more ways to write the update that grows a record's list, each two
/// thousand pushes. The list is named by a `let` before the update and grown
/// through the name, and it is handed to a function too large to inline from
/// both arms of an `if` inside the update. Either way the record dies in the
/// update, so each push finds its list unique. Found by the growth generator,
/// seed `0x67726f777468`, cases 138 and 61.
#[test]
fn a_list_named_by_a_let_or_handed_on_from_both_arms_of_an_update_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Acc { n: Int, items: [Int], tag: Int }

fn put<C: Allocator>(ctx: C, items: [Int], k: Int): [Int] {
  if (k < 0) { items } else { items.push(ctx, k) }
}

fn named<C: Allocator>(ctx: C, acc: Acc, i: Int): Acc {
  let held = acc.items;
  Acc { ..acc, n: acc.n + 1, items: held.push(ctx, held.length() + i) }
}

fn branched<C: Allocator>(ctx: C, acc: Acc, i: Int): Acc {
  Acc { ..acc, n: acc.n + 1, items: if (i % 2 == 0) { put(ctx, acc.items, i) } else { put(ctx, acc.items, i + 1) } }
}

fn byName<C: Allocator>(ctx: C, count: Int): Acc {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc, i) => named(c, acc, i), Acc { n: 0, items: [], tag: 7 })
}

fn byBranch<C: Allocator>(ctx: C, acc: Acc, left: Int): Acc {
  if (left == 0) { acc } else { byBranch(ctx, branched(ctx, acc, left), left - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let a = byName(host.alloc, 2000);
  let b = byBranch(host.alloc, Acc { n: 0, items: [], tag: 8 }, 2000);
  let _ = io.println(
    host.stdout,
    "${a.n} ${a.items.length()} ${a.tag} ${b.n} ${b.items.length()} ${b.tag}",
  ).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("field-push-named-branched", source) {
        assert_eq!(r.stdout, "2000 2000 7 2000 2000 8\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 60,
            "{backend}: four thousand pushes through record fields allocated {blocks} blocks: \
             the field kept a second count while it grew, so every push copied it"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// Two thousand pushes through a record nested in a record, where the
/// function that steps the inner record is called once and so is inlined. Its
/// parameter becomes a `let` naming `acc.inner`, and an update is written over
/// that name; the outer update moves `inner` out of the dying record, so the
/// inner update's base dies too and the push finds its list unique. Found by
/// the growth generator, seed `0x67726f777468`, case 90.
#[test]
fn a_list_in_a_record_nested_in_a_record_grows_in_place_through_an_inlined_step() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Inner { n: Int, items: [Int], tag: Int }

struct Outer { inner: Inner, k: Int }

fn grow<C: Allocator>(ctx: C, acc: Inner, i: Int): Inner {
  let seen = acc.items.length();
  Inner { ..acc, n: acc.n + 1, items: acc.items.push(ctx, i + seen) }
}

fn step<C: Allocator>(ctx: C, acc: Outer, i: Int): Outer {
  Outer { ..acc, k: acc.k + 1, inner: grow(ctx, acc.inner, i) }
}

fn run<C: Allocator>(ctx: C, acc: Outer, count: Int): Outer {
  list.range(ctx, 0, count).foldCtx(ctx, fn(c, acc, i) => step(c, acc, i), acc)
}

export fn main(host: NativeHost): Result<(), Str> {
  let o = run(host.alloc, Outer { inner: Inner { n: 0, items: [], tag: 5 }, k: 0 }, 2000);
  let _ = io.println(host.stdout, "${o.k} ${o.inner.n} ${o.inner.items.length()} ${o.inner.tag}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("field-push-nested-inlined", source) {
        assert_eq!(r.stdout, "2000 2000 2000 5\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 40,
            "{backend}: two thousand pushes allocated {blocks} blocks: the inner update was \
             written over a field path rather than a dying local, so every push copied"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// Two thousand pushes through a record nested in a record, stepped by a
/// helper that can fail: `Outer { ..acc, inner: grow(ctx, acc.inner, i)? }`.
/// The `?` leaves the function holding the dying record, so on that path the
/// record is released, without the field the helper was handed. On the other
/// the helper got the field's own count and pushes in place. Both paths run:
/// the first thousand steps succeed, and step 1000 fails. Found by the growth
/// generator exploring seed 2, case 271.
#[test]
fn a_field_handed_to_a_failing_helper_inside_an_update_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Inner { n: Int, items: [Str], tag: Str }

struct Outer { inner: Inner, k: Int, name: Str }

fn grow<C: Allocator>(ctx: C, acc: Inner, i: Int): Result<Inner, Str> {
  if (i == 1000) {
    .Err(str.format(ctx, "stopped at ${i}"))
  } else if (i < 0) {
    .Ok(acc)
  } else {
    .Ok(Inner { ..acc, n: acc.n + 1, items: acc.items.push(ctx, "x") })
  }
}

fn step<C: Allocator>(ctx: C, acc: Outer, i: Int): Result<Outer, Str> {
  .Ok(Outer { ..acc, k: acc.k + 1, inner: grow(ctx, acc.inner, i)? })
}

fn run<C: Allocator>(ctx: C, acc: Outer, count: Int): Result<Outer, Str> {
  list.range(ctx, 0, count).foldResultCtx(ctx, fn(c, acc, i) => step(c, acc, i), acc)
}

fn seed<C: Allocator>(ctx: C): Outer {
  Outer {
    inner: Inner { n: 0, items: [], tag: str.format(ctx, "t-${1}") },
    k: 0,
    name: str.format(ctx, "o-${2}"),
  }
}

fn shown<C: Allocator>(ctx: C, got: Result<Outer, Str>): Str {
  match (got) {
    .Ok(o) => str.format(ctx, "${o.k} ${o.inner.items.length()} ${o.inner.tag} ${o.name}"),
    .Err(e) => e,
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let done = shown(ctx, run(ctx, seed(ctx), 1000));
  let failed = shown(ctx, run(ctx, seed(ctx), 2000));
  let _ = io.println(host.stdout, "${done}, ${failed}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("field-push-failing-helper", source) {
        assert_eq!(r.stdout, "1000 1000 t-1 o-2, stopped at 1000\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 60,
            "{backend}: two thousand pushes allocated {blocks} blocks: the `?` in the update kept \
             the field counted twice, so every push copied it"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// The printer's other shape: a record handed on to the call that grows it,
/// and then read again for a number alone — `started.at` after `started` went
/// to `emit`. A number is a word of the record's own value, so reading it is no
/// second reader of the list, and the push must still find the list unique.
#[test]
fn a_number_read_from_a_record_after_it_was_handed_on_does_not_make_its_list_copy() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Out { pieces: [Str], at: Int, marks: [Int] }

fn raw<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  Out { ..out, pieces: out.pieces.push(ctx, t), at: out.at + 1 }
}

fn mark<C: Allocator>(ctx: C, out: Out, start: Int): Out {
  Out { ..out, marks: out.marks.push(ctx, start) }
}

fn line<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  let started = raw(ctx, out, "(");
  let written = raw(ctx, started, t);
  mark(ctx, written, started.at)
}

fn lines<C: Allocator>(ctx: C, out: Out, i: Int): Out {
  if (i == 0) { out } else { lines(ctx, line(ctx, out, "l"), i - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let out = lines(host.alloc, Out { pieces: [], at: 0, marks: [] }, 2000);
  let last = match (out.marks.last()) { .Some(m) => m, .None => -1 };
  let _ = io.println(host.stdout, "${out.pieces.length()} ${out.at} ${out.marks.length()} ${last}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("field-push-number-after", source) {
        assert_eq!(r.stdout, "4000 4000 2000 3999\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 60,
            "{backend}: six thousand pushes allocated {blocks} blocks: reading a number out of \
             the record kept it alive, so the call it was handed to got a second reference"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// The printer's third shape: a block's lines are a fold whose seed is
/// everything printed so far. The fold takes the seed over, so its first step
/// finds the list unique; lent, the first step of every fold copied it. Both
/// folds that take a seed and a context, one block of two lines per round.
#[test]
fn a_record_handed_to_a_fold_as_its_seed_grows_in_place() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Out { pieces: [Str], at: Int }

fn raw<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  Out { ..out, pieces: out.pieces.push(ctx, t), at: out.at + 1 }
}

fn block<C: Allocator>(ctx: C, out: Out, lines: [Str]): Out {
  lines.foldCtx(ctx, fn(c, acc: (Out, Bool), l) => { (raw(c, acc.0, l), true) }, (out, false)).0
}

fn checked<C: Allocator>(ctx: C, out: Out, lines: [Str]): Out {
  let done: Result<Out, Str> = lines.foldResultCtx(ctx, fn(c, acc: Out, l) => { .Ok(raw(c, acc, l)) }, out);
  match (done) { .Ok(o) => o, .Err(_e) => Out { pieces: [], at: -1 } }
}

fn blocks<C: Allocator>(ctx: C, out: Out, two: [Str], one: [Str], i: Int): Out {
  if (i == 0) { out } else { blocks(ctx, checked(ctx, block(ctx, out, two), one), two, one, i - 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let out = blocks(host.alloc, Out { pieces: [], at: 0 }, ["a", "b"], ["c"], 1000);
  let _ = io.println(host.stdout, "${out.pieces.length()} ${out.at}").ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("field-push-fold-seed", source) {
        assert_eq!(r.stdout, "3000 3000\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 60,
            "{backend}: two thousand folds allocated {blocks} blocks: the fold lent its seed to \
             the first step, so the step's first push copied the list"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// What every update answers when its record, or a list in it, has a second
/// reader: the old value is unchanged and the new one is what a copy would
/// be. Each line is one way to have that second reader.
#[test]
fn a_field_grown_from_a_record_with_a_second_reader_leaves_the_old_value_alone() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Out { pieces: [Str], at: Int }

struct Two { left: [Str], right: [Str] }

struct Outer { inner: Out, n: Int }

struct Held { xs: [Int], size: fn() => Int }

fn raw<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  Out { ..out, pieces: out.pieces.push(ctx, t), at: out.at + 1 }
}

fn swap<C: Allocator>(ctx: C, t: Two): Two {
  Two { ..t, left: t.right.push(ctx, "r"), right: t.left.push(ctx, "l") }
}

fn twice<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  Out { ..out, pieces: out.pieces.push(ctx, t), at: out.pieces.length() }
}

fn counted<C: Allocator>(ctx: C, out: Out): Out {
  Out { ..out, pieces: out.pieces.push(ctx, str.fromInt(ctx, out.pieces.length())) }
}

fn deep<C: Allocator>(ctx: C, o: Outer, t: Str): Outer {
  Outer { ..o, inner: Out { ..o.inner, pieces: o.inner.pieces.push(ctx, t) }, n: o.n + 1 }
}

fn hold<C: Allocator>(ctx: C, h: Held, x: Int): Held {
  Held { ..h, xs: h.xs.push(ctx, x), size: fn() => h.xs.length() }
}

fn appended<C: Allocator>(ctx: C, xs: [Str], t: Str): [Str] {
  xs.push(ctx, t)
}

fn handed<C: Allocator>(ctx: C, out: Out, t: Str): Out {
  Out { ..out, pieces: appended(ctx, out.pieces, t) }
}

fn checked<C: Allocator>(ctx: C, t: Str): Result<Str, Str> {
  if (t == "bad") { .Err(str.format(ctx, "refused ${t}")) } else { .Ok(t) }
}

fn tryRaw<C: Allocator>(ctx: C, out: Out, t: Str): Result<Out, Str> {
  .Ok(Out { ..out, pieces: out.pieces.push(ctx, checked(ctx, t)?), at: out.at + 1 })
}

fn show<C: Allocator>(ctx: C, xs: [Str]): Str {
  str.format(ctx, "[${xs.join(ctx, ",")}]")
}

fn grown<C: Allocator>(ctx: C, xs: [Str]): [Str] {
  xs.concat(ctx, list.empty<Str>())
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = host.alloc;
  let base = raw(ctx, raw(ctx, Out { pieces: [], at: 0 }, "a"), "b");
  let next = raw(ctx, base, "c");
  let again = raw(ctx, base, "d");
  let inline = Out { ..base, pieces: base.pieces.push(ctx, "e"), at: 9 };
  let _ = io.println(
    host.stdout,
    "kept ${show(ctx, base.pieces)} ${show(ctx, next.pieces)} ${show(ctx, again.pieces)} ${show(ctx, inline.pieces)} ${base.at}",
  ).ignore();

  let pair = Two { left: grown(ctx, ["a"]), right: grown(ctx, ["b"]) };
  let once = swap(ctx, pair);
  let thrice = swap(ctx, swap(ctx, swap(ctx, pair)));
  let _ = io.println(
    host.stdout,
    "swap ${show(ctx, once.left)} ${show(ctx, once.right)} ${show(ctx, thrice.left)} ${show(ctx, thrice.right)} ${show(ctx, pair.left)} ${show(ctx, pair.right)}",
  ).ignore();

  let read = twice(ctx, twice(ctx, twice(ctx, Out { pieces: grown(ctx, ["x"]), at: 0 }, "y"), "z"), "w");
  let numbered = counted(ctx, counted(ctx, counted(ctx, Out { pieces: [], at: 0 })));
  let _ = io.println(host.stdout, "twice ${show(ctx, read.pieces)} ${read.at} ${show(ctx, numbered.pieces)}").ignore();

  let one = grown(ctx, ["s"]);
  let shared = raw(ctx, Out { pieces: one, at: 0 }, "t");
  let both = swap(ctx, Two { left: one, right: one });
  let _ = io.println(
    host.stdout,
    "shared ${show(ctx, one)} ${show(ctx, shared.pieces)} ${show(ctx, both.left)} ${show(ctx, both.right)}",
  ).ignore();

  let outer = deep(ctx, Outer { inner: Out { pieces: grown(ctx, ["o"]), at: 0 }, n: 0 }, "p");
  let deeper = deep(ctx, deep(ctx, outer, "q"), "r");
  let _ = io.println(
    host.stdout,
    "nested ${show(ctx, outer.inner.pieces)} ${show(ctx, deeper.inner.pieces)} ${deeper.n}",
  ).ignore();

  let h0 = Held { xs: [1].concat(ctx, [2]), size: fn() => 0 };
  let h1 = hold(ctx, h0, 3);
  let h2 = hold(ctx, h1, 4);
  let h3 = hold(ctx, h2, 5);
  let _ = io.println(
    host.stdout,
    "captured ${h1.size()} ${h2.size()} ${h3.size()} ${h3.xs.length()} ${h0.xs.length()}",
  ).ignore();

  let via = handed(ctx, handed(ctx, Out { pieces: grown(ctx, ["h"]), at: 0 }, "i"), "j");
  let _ = io.println(host.stdout, "handed ${show(ctx, via.pieces)}").ignore();

  let src = raw(ctx, Out { pieces: grown(ctx, ["n"]), at: 5 }, "m");
  let onward = raw(ctx, src, "o");
  let _ = io.println(host.stdout, "number ${show(ctx, onward.pieces)} ${src.at} ${onward.at}").ignore();

  let seed = raw(ctx, Out { pieces: grown(ctx, ["f"]), at: 0 }, "g");
  let folded = ["h", "i"].foldCtx(ctx, fn(c, acc: Out, l) => raw(c, acc, l), seed);
  let empty = list.empty<Str>().foldCtx(ctx, fn(c, acc: Out, l) => raw(c, acc, l), seed);
  let _ = io.println(
    host.stdout,
    "seed ${show(ctx, seed.pieces)} ${show(ctx, folded.pieces)} ${show(ctx, empty.pieces)}",
  ).ignore();

  let good = tryRaw(ctx, Out { pieces: grown(ctx, ["g"]), at: 0 }, "ok");
  let bad = tryRaw(ctx, Out { pieces: grown(ctx, ["g"]), at: 0 }, "bad");
  let said = match (good) { .Ok(o) => show(ctx, o.pieces), .Err(e) => e };
  let refused = match (bad) { .Ok(o) => show(ctx, o.pieces), .Err(e) => e };
  let _ = io.println(host.stdout, "escape ${said} ${refused}").ignore();
  .Ok(())
}
"#;
    let expected = "\
kept [a,b] [a,b,c] [a,b,d] [a,b,e] 2
swap [b,r] [a,l] [b,r,l,r] [a,l,r,l] [a] [b]
twice [x,y,z,w] 3 [0,1,2]
shared [s] [s,t] [s,r] [s,l]
nested [o,p] [o,p,q,r] 3
captured 2 3 4 5 2
handed [h,i,j]
number [n,m,o] 6 7
seed [f,g] [f,g,h,i] [f,g]
escape [g,ok] refused bad
";
    for (backend, r) in run_each("field-push-readers", source) {
        assert_eq!(r.stdout, expected, "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}
