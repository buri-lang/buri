//! `core/orderedmap` grown and shrunk by a program, natively, on every native
//! backend this toolchain has built in.
//!
//! A node is a list of entries and a list of children, and an edit splices the
//! nodes on one path through `core/list`'s `insertAt`, `replaceAt` and
//! `removeAt`, which copy a node once, or write into it where nothing else
//! holds it. The work is counted rather than timed, through the allocation
//! probe, and every row runs under the heap check. The same splices on plain
//! lists of every element width check the runtime's copies.

use crate::shared::{probed, ran_checked, Ran};

/// One program on every backend, under the heap check.
fn run_each(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    crate::e2e::probed_backends()
        .into_iter()
        .map(|(backend, build)| (backend, ran_checked(&build(name, source))))
        .collect()
}

const PRELUDE: &str = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import { OrderedMap };
from "core/str" import * as str;

fn grow<C: Allocator>(ctx: C, m: OrderedMap<Int, Int>, i: Int, n: Int): OrderedMap<Int, Int> {
  if (i == n) { m } else { grow(ctx, m.insert(ctx, i * 7 % n, i), i + 1, n) }
}

fn shrink<C: Allocator>(ctx: C, m: OrderedMap<Int, Int>, i: Int, n: Int): OrderedMap<Int, Int> {
  if (i == n) { m } else { shrink(ctx, m.remove(ctx, i * 3 % n), i + 1, n) }
}

fn total(m: OrderedMap<Int, Int>, i: Int, n: Int, acc: Int): Int {
  if (i == n) { acc } else { total(m, i + 1, n, acc + m.get(i).withDefault(0)) }
}
"#;

/// Two thousand inserts and two thousand removes, each a walk down a tree of
/// seven-entry nodes. A splice that built its node with `take`, `push`,
/// `concat` and `drop` allocated three lists for the one it kept, which put
/// this at 58,289 blocks on the copy-and-patch backend. It was 22,317 with one
/// list a splice, and is 26,444 since a node keeps its keys and values in two
/// lists and every value in a leaf (#267, #266).
#[test]
fn an_ordered_map_edits_a_node_with_one_splice() {
    let source = format!(
        "{PRELUDE}{}",
        r#"
export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc };
  let full = grow(ctx, orderedmap.empty(), 0, 2000);
  let sum = total(full, 0, 2000, 0);
  let empty = shrink(ctx, full, 0, 2000);
  let _ = io.println(host.stdout, "${sum} ${empty.length()}").ignore();
  .Ok(())
}
"#
    );
    for (backend, r) in run_each("orderedmap-splices", &source) {
        assert_eq!(r.stdout, "1999000 0\n", "{backend}: stderr: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
        assert!(
            blocks < 30_000,
            "{backend}: four thousand edits of an ordered map allocated {blocks} blocks, \
             which is three lists a splice"
        );
    }
}

/// **An edit leaves every other name for the map as it was.** A splice writes
/// into a node nothing else holds, so each shape here is one where something
/// does: a second name, a map in a list, and an edit of an edit.
#[test]
fn an_ordered_map_edited_through_one_name_is_unchanged_through_another() {
    let source = format!(
        "{PRELUDE}{}",
        r#"
fn show<C: Allocator>(ctx: C, m: OrderedMap<Int, Int>): Str {
  str.format(ctx, "${m.length()} ${total(m, 0, 600, 0)} ${m.get(70).withDefault(-1)} ${m.get(700).withDefault(-1)}")
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc };
  let m = grow(ctx, orderedmap.empty(), 0, 600);
  let a = m.insert(ctx, 70, 1000);
  let b = m.insert(ctx, 700, 5);
  let c = m.remove(ctx, 140);
  let d = a.insert(ctx, 77, 9).remove(ctx, 0);
  let versions = [m, a];
  let e = shrink(ctx, versions.get(1).withDefault(m), 0, 300);
  let _ = io.println(host.stdout, "m ${show(ctx, m)}").ignore();
  let _ = io.println(host.stdout, "a ${show(ctx, a)}").ignore();
  let _ = io.println(host.stdout, "b ${show(ctx, b)}").ignore();
  let _ = io.println(host.stdout, "c ${show(ctx, c)}").ignore();
  let _ = io.println(host.stdout, "d ${show(ctx, d)}").ignore();
  let _ = io.println(host.stdout, "e ${show(ctx, e)}").ignore();
  .Ok(())
}
"#
    );
    for (backend, r) in run_each("orderedmap-aliasing", &source) {
        assert_eq!(
            r.stdout,
            "m 600 179700 10 -1\n\
             a 600 180690 1000 -1\n\
             b 601 179700 10 5\n\
             c 599 179680 10 -1\n\
             d 599 180688 1000 -1\n\
             e 500 152940 1000 -1\n",
            "{backend}: stderr: {}",
            r.stderr
        );
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}

const SPLICED: &str = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn grown<C: Allocator, T>(ctx: C, xs: [T], i: Int, make: fn(Int) => T): [T] {
  if (i == 80) { xs } else { grown(ctx, xs.insertAt(ctx, i / 2, make(i)), i + 1, make) }
}

fn replaced<C: Allocator, T>(ctx: C, xs: [T], i: Int, make: fn(Int) => T): [T] {
  if (i >= xs.length()) { xs } else { replaced(ctx, xs.replaceAt(ctx, i, make(100 + i)), i + 2, make) }
}

fn shrunk<C: Allocator, T>(ctx: C, xs: [T], left: Int): [T] {
  if (left == 0) { xs } else { shrunk(ctx, xs.removeAt(ctx, xs.length() / 3), left - 1) }
}

fn pushed<C: Allocator, T>(ctx: C, xs: [T], i: Int, make: fn(Int) => T): [T] {
  if (i == 20) { xs } else { pushed(ctx, xs.push(ctx, make(200 + i)), i + 1, make) }
}

fn digest<C: Allocator, T>(ctx: C, xs: [T], show: fn(T) => Int): Str {
  str.format(ctx, "${xs.length()} ${xs.fold(fn(h, x) => (h * 31 + show(x)) % 1000000007, 0)}")
}

// The first `replaceAt` copies, because `xs` is read again at the end.
fn report<C: Allocator, T>(ctx: C, name: Str, make: fn(Int) => T, show: fn(T) => Int): Str {
  let xs = grown(ctx, [], 0, make);
  let ys = replaced(ctx, xs, 0, make);
  let zs = shrunk(ctx, ys, 50);
  let ws = pushed(ctx, zs, 0, make);
  let cs = zs.concat(ctx, ws);
  let sl = cs.slice(ctx, 3, 13);
  let digests = [ys, zs, ws, cs, sl, xs].mapCtx(ctx, fn(c, l) => digest(c, l, show));
  str.format(ctx, "${name} ${digests.join(ctx, " ")}")
}

fn letters(): Str { "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-abcdefgh" }

fn joined<C: Allocator>(ctx: C, acc: Str, i: Int): Str {
  if (i == 72) { acc } else { joined(ctx, acc.concat(ctx, letters().slice(0, i)), i + 1) }
}

fn pairs<C: Allocator>(ctx: C, acc: Str, n: Int): Str {
  if (n == 72) { acc } else {
    let pair = letters().slice(0, n).concat(ctx, letters().slice(0, 71 - n));
    pairs(ctx, acc.concat(ctx, pair).concat(ctx, ","), n + 1)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc };
  let lines = [
    report(ctx, "u8", fn(g) => g.wrapToU8(), fn(x) => x.toI64()),
    report(ctx, "u32", fn(g) => (g * 65599).wrapToU32(), fn(x) => x.toI64()),
    report(ctx, "w1", fn(g) => g, fn(x) => x),
    report(ctx, "w2", fn(g) => (g, g + 1), fn(x) => x.0 * 7 + x.1),
    report(ctx, "w3", fn(g) => (g, g + 1, g + 2), fn(x) => (x.0 * 7 + x.1) * 7 + x.2),
    report(ctx, "w4", fn(g) => (g, g + 1, g + 2, g + 3), fn(x) => ((x.0 * 7 + x.1) * 7 + x.2) * 7 + x.3),
    report(ctx, "w5", fn(g) => (g, g + 1, g + 2, g + 3, g + 4),
      fn(x) => (((x.0 * 7 + x.1) * 7 + x.2) * 7 + x.3) * 7 + x.4),
    report(ctx, "w8", fn(g) => (g, g + 1, g + 2, g + 3, g + 4, g + 5, g + 6, g + 7),
      fn(x) => ((((((x.0 * 7 + x.1) * 7 + x.2) * 7 + x.3) * 7 + x.4) * 7 + x.5) * 7 + x.6) * 7 + x.7),
    report(ctx, "w9", fn(g) => (g, g + 1, g + 2, g + 3, g + 4, g + 5, g + 6, g + 7, g + 8),
      fn(x) => (((((((x.0 * 7 + x.1) * 7 + x.2) * 7 + x.3) * 7 + x.4) * 7 + x.5) * 7 + x.6) * 7 + x.7) * 7 + x.8),
    joined(ctx, "", 0),
    pairs(ctx, "", 0),
  ];
  let _ = io.println(host.stdout, lines.join(ctx, "\n")).ignore();
  .Ok(())
}
"#;

/// What [`SPLICED`] prints for one element type, with `show` the number an
/// element made from `g` reads back as.
fn spliced(name: &str, show: impl Fn(i64) -> i64) -> String {
    let digest = |v: &[i64]| {
        let h = v.iter().fold(0, |h, g| (h * 31 + show(*g)) % 1_000_000_007);
        format!("{} {h}", v.len())
    };
    let mut xs = Vec::new();
    for i in 0..80 {
        xs.insert(i as usize / 2, i);
    }
    let mut ys = xs.clone();
    for i in (0..ys.len()).step_by(2) {
        ys[i] = 100 + i as i64;
    }
    let mut zs = ys.clone();
    for _ in 0..50 {
        zs.remove(zs.len() / 3);
    }
    let mut ws = zs.clone();
    ws.extend(200..220);
    let cs = [zs.as_slice(), &ws].concat();
    let lists = [&ys, &zs, &ws, &cs, &cs[3..13].to_vec(), &xs].map(|v| digest(v));
    format!("{name} {}", lists.join(" "))
}

/// **A list keeps its elements through every splice at every element width.**
/// The runtime copies 64 bytes or less in a few word moves rather than a call,
/// and a size class it got wrong would show here: one- and four-byte elements
/// and tuples of one to nine `Int`s, through `insertAt`, `replaceAt`,
/// `removeAt`, `push`, `concat` and `slice`. Strings of every length up to 71
/// are joined the same way, onto a string nothing else holds and into a fresh
/// one.
#[test]
fn a_list_spliced_at_every_width_keeps_its_elements() {
    let ints = |fields: i64| move |g: i64| (0..fields).fold(0, |h, k| h * 7 + g + k);
    let mut want = vec![
        spliced("u8", |g| g),
        spliced("u32", |g| g * 65599),
        spliced("w1", ints(1)),
        spliced("w2", ints(2)),
        spliced("w3", ints(3)),
        spliced("w4", ints(4)),
        spliced("w5", ints(5)),
        spliced("w8", ints(8)),
        spliced("w9", ints(9)),
    ];
    let letters = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-abcdefgh";
    want.push((0..72).map(|i| &letters[..i]).collect());
    want.push(
        (0..72)
            .map(|n| format!("{}{},", &letters[..n], &letters[..71 - n]))
            .collect(),
    );
    let want = want.join("\n") + "\n";
    for (backend, r) in run_each("spliced-widths", SPLICED) {
        assert_eq!(r.stdout, want, "{backend}: stderr: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}

/// **An element whose first word is zero still lets go of what it holds.**
/// The runtime asks a live element's first word before scanning the rest for a
/// reference, and an `(Int, Str)` whose `Int` is 0 has one only further on:
/// each `replaceAt` here releases such an element, and so does the list's end.
#[test]
fn an_element_whose_first_word_is_zero_is_released() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn filled<C: Allocator>(ctx: C, xs: [(Int, Str)], i: Int): [(Int, Str)] {
  if (i == 50) { xs } else { filled(ctx, xs.push(ctx, (0, str.format(ctx, "s${i}"))), i + 1) }
}

fn swapped<C: Allocator>(ctx: C, xs: [(Int, Str)], i: Int): [(Int, Str)] {
  if (i == 50) { xs } else { swapped(ctx, xs.replaceAt(ctx, i, (0, str.format(ctx, "t${i}"))), i + 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc };
  let xs = swapped(ctx, filled(ctx, [(0, "")], 0), 0).push(ctx, (0, ""));
  let text = xs.mapCtx(ctx, fn(c, x) => x.1).join(ctx, "");
  let _ = io.println(host.stdout, str.format(ctx, "${xs.length()} ${text.length()}")).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_each("zero-first-word", source) {
        assert_eq!(r.stdout, "52 143\n", "{backend}: stderr: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}
