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

use crate::shared::{probed, ran_checked, Ran, ALLOC_PROBE};
use std::path::PathBuf;

/// Builds a named program into an executable with [`ALLOC_PROBE`] linked in.
type Build = fn(&str, &str) -> PathBuf;

/// Each native backend built into this toolchain that can run here, by name,
/// with the function that builds a program on it.
fn backends() -> Vec<(&'static str, Build)> {
    let mut out: Vec<(&'static str, Build)> = Vec::new();
    #[cfg(feature = "backend-stencil")]
    if crate::stencil::supported() {
        out.push(("stencil", |name, source| {
            crate::stencil::build_with(&format!("{name}-stencil"), source, Some(ALLOC_PROBE))
        }));
    }
    #[cfg(feature = "backend-llvm")]
    if crate::llvm::can_execute().is_none_or(|why| !crate::ci::skipped("llvm", why)) {
        out.push(("llvm", |name, source| {
            crate::llvm::build_at(
                &format!("{name}-llvm"),
                source,
                Some(ALLOC_PROBE),
                buri::compiler::backend::Profile::Release,
            )
        }));
    }
    out
}

/// One program on every backend, under the heap check.
fn run_each(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    backends()
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
from "core/effect" import { Allocator };
from "core/host" import { stdout, alloc };
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

export fn main(): Result<(), Str> {
  let out = write(alloc, Out { pieces: [], at: 0 }, 2000);
  let p = prepare(alloc, Prep { tokens: [], docs: [], pending: 0 }, 2000);
  let o = nested(alloc, Outer { inner: Out { pieces: [], at: 0 }, n: 0 }, 2000);
  let last = match (p.docs.last()) { .Some(d) => d, .None => -1 };
  let _ = io.println(
    stdout,
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

/// What every update answers when its record, or a list in it, has a second
/// reader: the old value is unchanged and the new one is what a copy would
/// be. Each line is one way to have that second reader.
#[test]
fn a_field_grown_from_a_record_with_a_second_reader_leaves_the_old_value_alone() {
    let source = r#"
from "core/effect" import { Allocator };
from "core/host" import { stdout, alloc };
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

export fn main(): Result<(), Str> {
  let ctx = alloc;
  let base = raw(ctx, raw(ctx, Out { pieces: [], at: 0 }, "a"), "b");
  let next = raw(ctx, base, "c");
  let again = raw(ctx, base, "d");
  let inline = Out { ..base, pieces: base.pieces.push(ctx, "e"), at: 9 };
  let _ = io.println(
    stdout,
    "kept ${show(ctx, base.pieces)} ${show(ctx, next.pieces)} ${show(ctx, again.pieces)} ${show(ctx, inline.pieces)} ${base.at}",
  ).ignore();

  let pair = Two { left: grown(ctx, ["a"]), right: grown(ctx, ["b"]) };
  let once = swap(ctx, pair);
  let thrice = swap(ctx, swap(ctx, swap(ctx, pair)));
  let _ = io.println(
    stdout,
    "swap ${show(ctx, once.left)} ${show(ctx, once.right)} ${show(ctx, thrice.left)} ${show(ctx, thrice.right)} ${show(ctx, pair.left)} ${show(ctx, pair.right)}",
  ).ignore();

  let read = twice(ctx, twice(ctx, twice(ctx, Out { pieces: grown(ctx, ["x"]), at: 0 }, "y"), "z"), "w");
  let numbered = counted(ctx, counted(ctx, counted(ctx, Out { pieces: [], at: 0 })));
  let _ = io.println(stdout, "twice ${show(ctx, read.pieces)} ${read.at} ${show(ctx, numbered.pieces)}").ignore();

  let one = grown(ctx, ["s"]);
  let shared = raw(ctx, Out { pieces: one, at: 0 }, "t");
  let both = swap(ctx, Two { left: one, right: one });
  let _ = io.println(
    stdout,
    "shared ${show(ctx, one)} ${show(ctx, shared.pieces)} ${show(ctx, both.left)} ${show(ctx, both.right)}",
  ).ignore();

  let outer = deep(ctx, Outer { inner: Out { pieces: grown(ctx, ["o"]), at: 0 }, n: 0 }, "p");
  let deeper = deep(ctx, deep(ctx, outer, "q"), "r");
  let _ = io.println(
    stdout,
    "nested ${show(ctx, outer.inner.pieces)} ${show(ctx, deeper.inner.pieces)} ${deeper.n}",
  ).ignore();

  let h0 = Held { xs: [1].concat(ctx, [2]), size: fn() => 0 };
  let h1 = hold(ctx, h0, 3);
  let h2 = hold(ctx, h1, 4);
  let h3 = hold(ctx, h2, 5);
  let _ = io.println(
    stdout,
    "captured ${h1.size()} ${h2.size()} ${h3.size()} ${h3.xs.length()} ${h0.xs.length()}",
  ).ignore();

  let via = handed(ctx, handed(ctx, Out { pieces: grown(ctx, ["h"]), at: 0 }, "i"), "j");
  let _ = io.println(stdout, "handed ${show(ctx, via.pieces)}").ignore();

  let good = tryRaw(ctx, Out { pieces: grown(ctx, ["g"]), at: 0 }, "ok");
  let bad = tryRaw(ctx, Out { pieces: grown(ctx, ["g"]), at: 0 }, "bad");
  let said = match (good) { .Ok(o) => show(ctx, o.pieces), .Err(e) => e };
  let refused = match (bad) { .Ok(o) => show(ctx, o.pieces), .Err(e) => e };
  let _ = io.println(stdout, "escape ${said} ${refused}").ignore();
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
escape [g,ok] refused bad
";
    for (backend, r) in run_each("field-push-readers", source) {
        assert_eq!(r.stdout, expected, "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}
