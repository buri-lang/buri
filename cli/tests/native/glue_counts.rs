//! Records and enums of several strings and lists, retained and released
//! field by field: the values read back under the heap check on every
//! backend, and a release costs a bounded number of instructions a field.

use crate::collection_costs::per_op;

/// `Card`, a record of six `Str`s and two lists; `Shape`, an enum whose
/// variants hold several of each; `Deck`, a record nesting both; and `Tree`,
/// which reaches itself through records, enums and lists.
const SHAPES: &str = r#"
derive Equal, Show for Card;
struct Card { a: Str, b: Str, c: Str, d: Str, e: Str, f: Str, tags: [Str], ns: [Int] }

derive Equal, Show for Shape;
enum Shape {
    Empty,
    Two(Str, Str),
    Lists([Str], [Str], Str),
    Wide { a: Str, b: Str, c: Str, d: Str, e: [Str] },
}

derive Equal, Show for Deck;
struct Deck { name: Str, top: Card, shape: Shape, rest: [Card] }

derive Equal, Show for Tree;
enum Tree {
    Leaf(Str),
    Node { label: Str, left: Tree, right: Tree, notes: [Str] },
    Many([Tree], Str),
}

fn built<C: Allocator>(ctx: C, unit: Str, times: Int): Str {
    unit.repeat(ctx, times)
}

fn card<C: Allocator>(ctx: C, k: Int): Card {
    let s = built(ctx, "s", k % 4 + 1);
    Card {
        a: s, b: built(ctx, "b", 2), c: s, d: built(ctx, "d", k % 3 + 1), e: s, f: built(ctx, "f", 1),
        tags: [s, built(ctx, "t", 3), s],
        ns: [k, k + 1],
    }
}

fn shape<C: Allocator>(ctx: C, k: Int): Shape {
    let s = built(ctx, "h", k % 5 + 1);
    match (k % 4) {
        0 => .Empty,
        1 => .Two(s, built(ctx, "o", 2)),
        2 => .Lists([s, s], [built(ctx, "l", k)], s),
        _ => .Wide { a: s, b: built(ctx, "w", 3), c: s, d: s, e: [s, built(ctx, "e", 1)] },
    }
}

fn deck<C: Allocator>(ctx: C, k: Int): Deck {
    let top = card(ctx, k);
    Deck { name: built(ctx, "n", k % 2 + 1), top: top, shape: shape(ctx, k), rest: [top, card(ctx, k + 1), top] }
}

fn tree<C: Allocator>(ctx: C, d: Int): Tree {
    if (d == 0) {
        .Leaf(built(ctx, "leaf", 1))
    } else {
        let sub = tree(ctx, d - 1);
        match (d % 3) {
            0 => .Node { label: built(ctx, "x", d), left: sub, right: sub, notes: [built(ctx, "y", d)] },
            1 => .Many([sub, .Leaf(built(ctx, "z", d)), sub], built(ctx, "m", 2)),
            _ => .Node { label: built(ctx, "q", 1), left: .Leaf(built(ctx, "r", d)), right: sub, notes: [] },
        }
    }
}

fn size(t: Tree): Int {
    match (t) {
        .Leaf(s) => s.length(),
        .Node { label, left, right, notes } => label.length() + size(left) + size(right) + notes.length(),
        .Many(ts, s) => ts.fold(fn(a, x) => a + size(x), s.length()),
    }
}
"#;

const TESTS: &str = r#"
test "records of several strings and lists read back once shared and dropped" {
    let ctx = context { Allocator: testing.alloc() };
    let xs = list.range(ctx, 0, 40).mapCtx(ctx, fn(c, k) => card(c, k));
    let shared = xs.map(ctx, fn(x) => x);
    assert.equal(shared, list.range(ctx, 0, 40).mapCtx(ctx, fn(c, k) => card(c, k)));
    assert.equal(shared.get(5).map(fn(x) => x.tags), .Some(["ss", "ttt", "ss"]));
}

test "enums of several strings and lists read back once shared and dropped" {
    let ctx = context { Allocator: testing.alloc() };
    let xs = list.range(ctx, 0, 40).mapCtx(ctx, fn(c, k) => shape(c, k));
    let shared = xs.map(ctx, fn(x) => x);
    assert.equal(shared, list.range(ctx, 0, 40).mapCtx(ctx, fn(c, k) => shape(c, k)));
    assert.equal(shared.get(2), .Some(Shape.Lists(["hhh", "hhh"], ["ll"], "hhh")));
}

test "nested records and enums read back once copied out of a scope" {
    let ctx = context { Allocator: testing.alloc() };
    let out = alloc.scoped(ctx, fn(c) => list.range(c, 0, 12).mapCtx(c, fn(d, k) => deck(d, k)));
    assert.equal(out, list.range(ctx, 0, 12).mapCtx(ctx, fn(c, k) => deck(c, k)));
}

test "a recursive enum of records and lists reads back shared and copied" {
    let ctx = context { Allocator: testing.alloc() };
    let t = tree(ctx, 10);
    let out = alloc.scoped(ctx, fn(c) => tree(c, 10));
    assert.equal(out, t);
    assert.equal(size(Tree.Many([t, t], "")), 2 * size(t));
}

fn kept<C: Allocator>(ctx: C): [Deck] {
    let xs = list.range(ctx, 0, 12).mapCtx(ctx, fn(c, k) => deck(c, k));
    let ys = xs.map(ctx, fn(x) => x);
    ys
}

test "a shared value released last still reads back" {
    let ctx = context { Allocator: testing.alloc() };
    let ys = kept(ctx);
    assert.equal(ys, list.range(ctx, 0, 12).mapCtx(ctx, fn(c, k) => deck(c, k)));
    assert.equal(ys.get(3).map(fn(x) => x.top.a), .Some("ssss"));
}
"#;

/// `buri test` over the shapes above, on every backend under the heap check.
#[test]
fn records_and_enums_of_strings_and_lists_read_back_shared_and_released() {
    let test = format!(
        r#"from "core/alloc" import * as alloc;
from "core/list" import * as list;
from "core/testing/assert" import * as assert;
from "platform/effect" import {{ Allocator }};
from "platform/effect/testing" import * as testing;
{SHAPES}{TESTS}"#
    );
    crate::odd_sizes::passes("glue-counts", &test, 5);
}

/// The same shapes in a program that fans out, where every count is atomic:
/// each value goes through a task and back, and the originals are released
/// before the copies.
#[test]
fn records_and_enums_of_strings_and_lists_read_back_through_tasks() {
    let source = format!(
        r#"from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import {{ NativeHost }};
from "platform/effect" import {{ Allocator, Stdout, Tasks }};
{SHAPES}
fn through<C: Allocator + Tasks>(ctx: C): [Deck] {{
  let decks = list.range(ctx, 0, 24).mapCtx(ctx, fn(c, k) => deck(c, k));
  tasks.parallel(ctx, decks, fn(_c, _i, d) => d)
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks }};
  let back = through(ctx);
  let decks = back == list.range(ctx, 0, 24).mapCtx(ctx, fn(c, k) => deck(c, k));
  let t = tree(ctx, 8);
  let trees = tasks.parallel(ctx, [t, t, t], fn(_c, _i, x) => x);
  let sizes = trees.fold(fn(a, x) => a + size(x), 0) == 3 * size(t);
  io.println(ctx, "${{decks}} ${{sizes}}").mapErr(fn(_e) => "stdout")
}}
"#
    );
    for (backend, build) in crate::e2e::probed_backends() {
        let r = crate::shared::ran_checked(&build("glue-tasks", &source));
        assert_eq!(r.stdout, "true true\n", "{backend}: {}", r.stderr);
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
    }
}

const ELEMS: u64 = 2000 * 50;

/// A list of 2,000 records of `Str`s, shared element by element 50 times:
/// each share retains every field and each dropped copy releases them. `main`
/// takes the mode and the width, 1 or 8.
const SHARES: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct One { a: Str }
struct Eight { a: Str, b: Str, c: Str, d: Str, e: Str, f: Str, g: Str, h: Str }

fn times<C: Allocator, T>(ctx: C, k: Int, acc: Int, xs: [T]): Int {
  if (k == 0) { acc } else { times(ctx, k - 1, acc + xs.map(ctx, fn(x) => x).length(), xs) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let s = "s".repeat(ctx, 3);
  let eights = args.get(1) == .Some("8");
  let ones = list.range(ctx, 0, if (eights) { 0 } else { 2000 }).map(ctx, fn(_i) => One { a: s });
  let wide = list.range(ctx, 0, if (eights) { 2000 } else { 0 })
    .map(ctx, fn(_i) => Eight { a: s, b: s, c: s, d: s, e: s, f: s, g: s, h: s });
  let answer = match (args.first()) {
    .Some("share") => if (eights) { times(ctx, 50, 0, wide) } else { times(ctx, 50, 0, ones) },
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A field costs a handful of instructions to retain and release.** Each
/// field of a record wider than one load is retained and released on its own,
/// and that pair is counted here as the difference between a record of eight
/// `Str`s and a record of one, over seven.
#[test]
fn retaining_and_releasing_a_str_field_costs_a_handful_of_instructions() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("glue-shares", SHARES);
        let cost = |width: &str| {
            per_op(
                backend,
                &binary,
                "share",
                &[width],
                ELEMS,
                &format!("{ELEMS}\n"),
            )
            .0
        };
        let (Some(one), Some(eight)) = (cost("1"), cost("8")) else {
            continue;
        };
        let field = eight.saturating_sub(one) / 7;
        eprintln!("{backend}: {field} instructions to retain and release a field");
        if field > LIMIT {
            failures.push(format!(
                "{backend}: {field} instructions to retain and release a field, against {LIMIT}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LIMIT: u64 = 56;
