//! `core/orderedmap` grown and shrunk by a program, natively, on every native
//! backend this toolchain has built in.
//!
//! A node is a list of entries and a list of children, and an edit splices the
//! nodes on one path through `core/list`'s `insertAt`, `replaceAt` and
//! `removeAt`, which copy a node once, or write into it where nothing else
//! holds it. The work is counted rather than timed, through the allocation
//! probe, and every row runs under the heap check.

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
/// list a splice, and is 28,442 since a node keeps its keys and values in two
/// lists (#267).
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
