//! What `core/orderedmap`'s lookups and `values`, and `core/list`'s
//! `filterMap`, cost against the shape of what they hold, natively, on every
//! native backend this toolchain has built in.
//!
//! Each program runs once doing nothing past its setup and once doing the work,
//! so the difference is the work alone. The work is counted rather than timed:
//! instructions retired where the kernel counts them (macOS on hardware), and
//! blocks through the allocation probe everywhere. The counted runs are the
//! program as it ships, and one more of each runs under the heap check.

use crate::hang::CappedChild;
use crate::shared::{exited_instructions, heap_checked, probed, ran_command, Ran};
use std::path::Path;

/// One run, with the fewest instructions of three, because a short process's
/// count only ever gains noise (`design/PERFORMANCE.md` §8). An idle run's
/// noise takes away from the work's count, and under memory pressure two runs
/// in a row both gained it. `None` where the kernel counts nothing, such as
/// CI's virtual machines.
fn measured(binary: &Path, args: &[&str]) -> (Ran, Option<u64>) {
    let once = || {
        let mut cmd = std::process::Command::new(binary);
        cmd.args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd.spawn().unwrap();
        let instructions = exited_instructions(child.id());
        let out = child.capped_wait_with_output().unwrap();
        let ran = Ran {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        };
        (ran, instructions)
    };
    let (ran, first) = once();
    let (_, second) = once();
    let (_, third) = once();
    let fewest = first.zip(second).zip(third);
    (
        ran,
        fewest.map(|((a, b), c)| a.min(b).min(c)).filter(|&n| n > 0),
    )
}

/// The instructions one operation costs, from `ops` of them against none, and
/// the blocks all of them allocated. `args` are the program's after its mode.
pub(crate) fn per_op(
    backend: &str,
    binary: &Path,
    mode: &str,
    args: &[&str],
    ops: u64,
    expected: &str,
) -> (Option<u64>, u64) {
    let (idle, idle_n) = measured(binary, &[&["none"], args].concat());
    let (busy, busy_n) = measured(binary, &[&[mode], args].concat());
    let checked = ran_command(heap_checked(
        std::process::Command::new(binary).args([&[mode], args].concat()),
    ));
    for (r, want) in [(&idle, "0\n"), (&busy, expected), (&checked, expected)] {
        assert_eq!(r.status, 0, "{backend} {mode} {args:?}: {}", r.stderr);
        assert_eq!(r.stdout, want, "{backend} {mode} {args:?}: {}", r.stderr);
    }
    let (idle_blocks, idle_live) = probed(&idle.stderr);
    let (busy_blocks, busy_live) = probed(&busy.stderr);
    assert_eq!(
        (idle_live, busy_live),
        (0, 0),
        "{backend} {mode}: blocks still live at exit"
    );
    let instructions = idle_n.zip(busy_n).map(|(a, b)| b.saturating_sub(a) / ops);
    (instructions, busy_blocks.saturating_sub(idle_blocks))
}

const LOOKUPS: u64 = 100_000;

/// A map of `size` `Int` keys to values of type `Value`, and `mode` asked of it
/// 100,000 times: `get` reads one field of what it finds, and `has` asks only
/// whether there is anything. `main` takes the mode and the size.
fn lookups(value: &str, make: &str, read: &str) -> String {
    format!(
        r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import {{ OrderedMap }};
from "core/str" import * as str;
from "native" import {{ NativeHost }};
from "platform/effect" import {{ Allocator, Environment, Stdout }};

{value}

fn make<C: Allocator>(ctx: C, i: Int): Value {{
  {make}
}}

fn gets(m: OrderedMap<Int, Value>, i: Int, n: Int, acc: Int): Int {{
  if (i >= n) {{ acc }} else {{ gets(m, i + 1, n, acc + m.get(i * 3 % m.size).map(fn(v) => {read}).withDefault(0)) }}
}}

fn hases(m: OrderedMap<Int, Value>, i: Int, n: Int, acc: Int): Int {{
  if (i >= n) {{ acc }} else {{ hases(m, i + 1, n, acc + (if (m.has(i * 3 % m.size)) {{ 1 }} else {{ 0 }})) }}
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Environment: host.env, Stdout: host.stdout }};
  let args = env.arguments(ctx);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let m = orderedmap.of(ctx, list.range(ctx, 0, size).mapCtx(ctx, fn(c, i) => (i, make(c, i))));
  let answer = match (args.first()) {{
    .Some("get") => gets(m, 0, {LOOKUPS}, 0),
    .Some("has") => hases(m, 0, {LOOKUPS}, 0),
    _ => 0,
  }};
  io.println(ctx, "${{answer}}").mapErr(fn(_e) => "stdout")
}}
"#
    )
}

/// **A key-only lookup costs the same whatever the value type is**
/// (buri-lang/buri#267). Each node held its entries as one list of `(K, V)`,
/// so the search copied every entry it compared a key against, value and all,
/// and retained each counted field in it.
///
/// Keys sit in a list of their own now, so `has` reads no value. `get` copies
/// the one it answers, and that copy is the same however deep the key sits.
#[test]
fn an_ordered_map_lookup_costs_the_same_for_any_value_type() {
    let fields =
        |n: usize, f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<Vec<_>>().join(", ");
    let shapes = [
        (
            "one Int",
            fields(1, &|i| format!("f{i}: Int")),
            fields(1, &|i| format!("f{i}: i")),
            "v.f0",
        ),
        (
            "64 Ints",
            fields(64, &|i| format!("f{i}: Int")),
            fields(64, &|i| format!("f{i}: i")),
            "v.f0",
        ),
        (
            "eight Strs",
            fields(8, &|i| format!("f{i}: Str")),
            fields(8, &|i| format!("f{i}: str.format(ctx, \"x${{i}}\")")),
            "v.f0.length()",
        ),
    ];
    // One leaf, and a tree three levels deep.
    let sizes = [7u64, 2000];
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        // (shape, size) -> (get, has)
        let mut costs = std::collections::BTreeMap::new();
        for (i, (name, value, make, read)) in shapes.iter().enumerate() {
            let source = lookups(
                &format!("struct Value {{ {value} }}"),
                &format!("Value {{ {make} }}"),
                read,
            );
            let binary = build(&format!("orderedmap-lookup-{i}"), &source);
            for size in sizes {
                let keys = (0..LOOKUPS).map(|j| j * 3 % size);
                let got: u64 = if i == 2 {
                    keys.map(|k| 1 + k.to_string().len() as u64).sum()
                } else {
                    keys.sum()
                };
                let at = size.to_string();
                let (get, get_blocks) = per_op(
                    backend,
                    &binary,
                    "get",
                    &[&at],
                    LOOKUPS,
                    &format!("{got}\n"),
                );
                let (has, has_blocks) = per_op(
                    backend,
                    &binary,
                    "has",
                    &[&at],
                    LOOKUPS,
                    &format!("{LOOKUPS}\n"),
                );
                assert_eq!(
                    (get_blocks, has_blocks),
                    (0, 0),
                    "{backend}, {name}: a lookup allocated"
                );
                eprintln!("{backend}, {name}, {size} keys: get {get:?}, has {has:?} instructions");
                costs.insert((i, size), (get, has));
            }
        }
        for size in sizes {
            let Some((_, Some(has1))) = costs.get(&(0, size)).copied() else {
                continue;
            };
            for (i, (name, ..)) in shapes.iter().enumerate().skip(1) {
                let Some((_, Some(has))) = costs.get(&(i, size)).copied() else {
                    continue;
                };
                if has * 10 > has1 * 12 {
                    failures.push(format!(
                        "{backend}, {size} keys: `has` on {name} is {has} instructions against {has1} on one Int"
                    ));
                }
            }
        }
        // What `get` adds over `has` is the copy of the value it answers, once.
        for (i, (name, ..)) in shapes.iter().enumerate() {
            let value = |size| match costs.get(&(i, size)).copied() {
                Some((Some(get), Some(has))) => Some(get.saturating_sub(has)),
                _ => None,
            };
            let (Some(shallow), Some(deep)) = (value(7), value(2000)) else {
                continue;
            };
            if deep * 10 > shallow * 12 + 500 {
                failures.push(format!(
                    "{backend}: `get` on {name} adds {deep} instructions over `has` three levels deep, \
                     and {shallow} in one leaf"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same `size` `Str` keys in an ordered map and in a sorted list, and
/// whether each holds a key asked 100,000 times: of the map with `has`, and of
/// the list with a binary search. `main` takes the mode and the size.
const STR_LOOKUPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import { OrderedMap };
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn holds(ks: [Str], k: Str, lo: Int, hi: Int): Bool {
  if (lo >= hi) { false } else {
    let mid = (lo + hi) / 2;
    match (ks[mid]) {
      .Some(m) => if (m == k) { true } else if (m < k) { holds(ks, k, mid + 1, hi) } else { holds(ks, k, lo, mid) },
      .None => false,
    }
  }
}

fn mapped(m: OrderedMap<Str, Int>, ks: [Str], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let k = ks[i % ks.length()].withDefault("");
    mapped(m, ks, i + 1, n, acc + (if (m.has(k)) { 1 } else { 0 }))
  }
}

fn searched(sorted: [Str], ks: [Str], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let k = ks[i % ks.length()].withDefault("");
    searched(sorted, ks, i + 1, n, acc + (if (holds(sorted, k, 0, sorted.length())) { 1 } else { 0 }))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let size = args[1].andThen(fn(s) => s.toInt()).withDefault(1);
  let ks = list.range(ctx, 0, size).mapCtx(ctx, fn(c, i) => str.format(c, "key-${i * 7919 % size}"));
  let m = orderedmap.of(ctx, ks.mapCtx(ctx, fn(_c, k) => (k, k.length())));
  let sorted = ks.sort(ctx);
  let answer = match (args.first()) {
    .Some("map") => mapped(m, ks, 0, 100000, 0),
    .Some("list") => searched(sorted, ks, 0, 100000, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **An ordered lookup over `Str` keys costs about what a binary search over a
/// sorted list does.** Each node read its keys and children with `list.get`,
/// which retains the element it answers, so the search retained and released
/// every key it compared and every node it passed through: 2,000 keys cost
/// 1.72 times the binary search on the copy-and-patch backend. Read by index,
/// a key or a child is a word of its node, and that's 1.31.
#[test]
fn an_ordered_lookup_over_str_keys_costs_about_what_a_binary_search_does() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("str-lookups", STR_LOOKUPS);
        let want = format!("{LOOKUPS}\n");
        let (map, _) = per_op(backend, &binary, "map", &["2000"], LOOKUPS, &want);
        let (list, _) = per_op(backend, &binary, "list", &["2000"], LOOKUPS, &want);
        eprintln!("{backend}: map {map:?}, list {list:?} instructions a lookup");
        let (Some(map), Some(list)) = (map, list) else {
            continue;
        };
        if map * 10 > list * 14 {
            failures.push(format!(
                "{backend}: an ordered lookup over 2,000 `Str` keys is {map} instructions \
                 against {list} for a binary search"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A chain of `depth` nodes below a root, each the one child of the one
/// above, and `mode` walked over it until about `ops` steps have been taken:
/// `down` steps from each node into its child, and `across` reads each node's
/// child from a list of every node instead. `main` takes the mode, the depth
/// and the steps.
const DESCENTS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Node { at: Int, kids: [Node] }

fn chain<C: Allocator>(ctx: C, n: Int, acc: Node): Node {
  if (n == 0) { acc } else { chain(ctx, n - 1, Node { at: n, kids: [acc] }) }
}

fn down(n: Node, steps: Int): Int {
  match (n.kids[0]) {
    .Some(child) => down(child, steps + 1),
    .None => steps + n.at,
  }
}

fn across(nodes: [Node], i: Int, steps: Int): Int {
  match (nodes[i]) {
    .Some(n) => match (n.kids[0]) {
      .Some(_child) => across(nodes, i + 1, steps + 1),
      .None => steps + n.at,
    },
    .None => steps,
  }
}

fn spine<C: Allocator>(ctx: C, n: Node, acc: [Node]): [Node] {
  match (n.kids[0]) {
    .Some(child) => spine(ctx, child, acc.push(ctx, n)),
    .None => acc.push(ctx, n),
  }
}

fn rounds(root: Node, nodes: [Node], mode: Str, k: Int, acc: Int): Int {
  if (k == 0) { acc } else {
    let got = if (mode == "down") { down(root, 0) } else { across(nodes, 0, 0) };
    rounds(root, nodes, mode, k - 1, acc + got)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let depth = args[1].andThen(fn(s) => s.toInt()).withDefault(1);
  let ops = args[2].andThen(fn(s) => s.toInt()).withDefault(0);
  let root = chain(ctx, depth, Node { at: 0, kids: [] });
  let nodes = spine(ctx, root, []);
  let mode = args.first().withDefault("none");
  let answer = if (mode == "none") { 0 } else { rounds(root, nodes, mode, ops / depth, 0) };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **Stepping down a chain it was lent costs less than reading each node's
/// child out of a list.** The child a step matched out of its node was the
/// next step's own, so each step retained it and released the node before it:
/// 69 instructions a step against 70 across the list on the copy-and-patch
/// backend, and 27 against 16 on LLVM. The caller's count holds the whole
/// chain for the whole walk, so the walk borrows all the way down, and that's
/// 36 and 10.
#[test]
fn stepping_down_a_lent_chain_costs_less_than_reading_it_from_a_list() {
    const DEPTH: u64 = 1000;
    const STEPS: u64 = 1_000_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("descents", DESCENTS);
        let (depth, steps) = (DEPTH.to_string(), STEPS.to_string());
        let args = [depth.as_str(), steps.as_str()];
        let want = format!("{STEPS}\n");
        let (down, _) = per_op(backend, &binary, "down", &args, STEPS, &want);
        let (across, _) = per_op(backend, &binary, "across", &args, STEPS, &want);
        eprintln!("{backend}: down {down:?}, across {across:?} instructions a step");
        let (Some(down), Some(across)) = (down, across) else {
            continue;
        };
        if down * 10 > across * 8 {
            failures.push(format!(
                "{backend}: a step down a chain is {down} instructions against {across} \
                 for reading the same child from a list"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// 2,000 records of four `Str`s, four `Int`s and a `[Str]`, #266's shape, and
/// `mode` run over them 50 times: `map` and `filterMap` copy the list through
/// a lambda that keeps every element, and `values` reads them back out of a
/// map.
const PROJECTIONS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/orderedmap" import * as orderedmap;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Wide {
  a: Str,
  b: Str,
  c: Str,
  d: Str,
  e: Int,
  f: Int,
  g: Int,
  h: Int,
  tags: [Str],
}

fn wide<C: Allocator>(ctx: C, i: Int): Wide {
  let s = str.format(ctx, "w${i}");
  Wide { a: s, b: s, c: s, d: s, e: i, f: i, g: i, h: i, tags: [s] }
}

fn times<C: Allocator>(ctx: C, k: Int, acc: Int, f: fn(C) => Int): Int {
  if (k == 0) { acc } else { times(ctx, k - 1, acc + f(ctx), f) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let xs = list.range(ctx, 0, 2000).mapCtx(ctx, fn(c, i) => wide(c, i));
  let m = orderedmap.of(ctx, xs.map(ctx, fn(w) => (w.e, w)));
  let answer = match (env.arguments(ctx).first()) {
    .Some("map") => times(ctx, 50, 0, fn(c) => xs.map(c, fn(w) => w).length()),
    .Some("filterMap") => times(ctx, 50, 0, fn(c) => xs.filterMap(c, fn(w) => .Some(w)).length()),
    .Some("values") => times(ctx, 50, 0, fn(c) => m.values(c).length()),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **`filterMap` and `OrderedMap.values` cost about what `map` costs**
/// (buri-lang/buri#266). `filterMap` was a fold through a closure that owned
/// each element it was handed and pushed what it kept. The push outgrew its
/// block again and again, and each move to a bigger block retained every
/// element already there. `values` appended each node's values to one list
/// the same way. Against `map`, in instructions an element, `filterMap` was 3
/// and 5 times on the copy-and-patch and LLVM backends, and `values` 2.2 and 4
/// times.
///
/// `filterMap` is a loop like `filter` now, writing into a block the length of
/// the list. Every value sits in a leaf, and `values` flattens the leaves' own
/// lists into one block sized once.
#[test]
fn filter_map_and_values_cost_about_what_map_costs() {
    let elems = 2000 * 50;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-projections", PROJECTIONS);
        let mut cost = std::collections::BTreeMap::new();
        for mode in ["map", "filterMap", "values"] {
            let (n, _blocks) = per_op(backend, &binary, mode, &[], elems, "100000\n");
            eprintln!("{backend}, {mode}: {n:?} instructions an element");
            cost.insert(mode, n);
        }
        let Some(Some(map)) = cost.get("map").copied() else {
            continue;
        };
        for (mode, times) in [("filterMap", 13), ("values", 13)] {
            let Some(Some(n)) = cost.get(mode).copied() else {
                continue;
            };
            if n * 10 > map * times + 200 {
                failures.push(format!(
                    "{backend}: {mode} is {n} instructions an element against map's {map}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// 2,000 `Int`s in `mode`'s order, sorted 50 times. `main` takes the mode.
const SORTS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn times<C: Allocator>(ctx: C, xs: [Int], k: Int, acc: Int): Int {
  if (k == 0) { acc } else { times(ctx, xs, k - 1, acc + xs.sort(ctx).first().withDefault(0) + 1) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let mode = env.arguments(ctx).first().withDefault("none");
  let xs = list.range(ctx, 0, 2000).map(ctx, fn(i) => match (mode) {
    "ascending" => i,
    "descending" => 2000 - i,
    _ => (i * 7919) % 2003,
  });
  let answer = if (mode == "none") { 0 } else { times(ctx, xs, 50, 0) };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **Sorting a list that is already in order, either way, costs a fraction of
/// sorting a shuffled one.** `sortBy` was a bottom-up merge from runs of one,
/// so it made the same eleven passes over 2,000 elements whatever their order.
/// An ascending list cost 0.83 times a shuffled one on LLVM and 0.73 on the
/// copy-and-patch backend, and a descending one 0.94 and 0.77. Now a scan
/// finds both, and they cost under a tenth.
#[test]
fn sorting_an_ordered_list_costs_a_fraction_of_a_shuffled_one() {
    let elems = 2000 * 50;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-sorts", SORTS);
        let mut cost = std::collections::BTreeMap::new();
        for (mode, answer) in [
            ("shuffled", "50\n"),
            ("ascending", "50\n"),
            ("descending", "100\n"),
        ] {
            let (n, _blocks) = per_op(backend, &binary, mode, &[], elems, answer);
            eprintln!("{backend}, {mode}: {n:?} instructions an element");
            cost.insert(mode, n);
        }
        let Some(Some(shuffled)) = cost.get("shuffled").copied() else {
            continue;
        };
        for mode in ["ascending", "descending"] {
            let Some(Some(n)) = cost.get(mode).copied() else {
                continue;
            };
            if n * 4 > shuffled {
                failures.push(format!("{backend}: {mode} is {n} instructions an element against shuffled's {shuffled}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Trees built and dropped until about a million nodes have been, `depth`
/// levels each. `parallel` builds eight a round in a fan-out and drops them in
/// the caller, so most blocks are freed on a thread that didn't allocate them.
const TREES: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout, Tasks };

enum Tree { Leaf, Node(Tree, Int, Tree) }

fn build<C: Allocator>(ctx: C, d: Int, v: Int): Tree {
  if (d == 0) { .Leaf } else { .Node(build(ctx, d - 1, v * 2), v, build(ctx, d - 1, v * 2 + 1)) }
}

fn sum(t: Tree): Int {
  match (t) { .Leaf => 0, .Node(l, v, r) => sum(l) + v % 7 + sum(r) }
}

fn pow2(d: Int): Int {
  if (d == 0) { 1 } else { 2 * pow2(d - 1) }
}

fn rounds<C: Allocator>(ctx: C, d: Int, k: Int, acc: Int): Int {
  if (k == 0) { acc } else { rounds(ctx, d, k - 1, acc + sum(build(ctx, d, 1))) }
}

fn fanned<C: Allocator + Tasks>(ctx: C, d: Int, k: Int, acc: Int): Int {
  if (k == 0) {
    acc
  } else {
    let trees = tasks.parallel(ctx, list.range(ctx, 0, 8), fn(c, _i, _x) => build(c, d, 1));
    fanned(ctx, d, k - 1, acc + trees.fold(fn(a, t) => a + sum(t), 0))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout, Tasks: host.tasks };
  let args = env.arguments(ctx);
  let depth = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let answer = match (args.first()) {
    .Some("trees") => rounds(ctx, depth, 1048576 / pow2(depth), 0),
    .Some("parallel") => fanned(ctx, depth, 131072 / pow2(depth), 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A tree too big for the block cache costs about what a small one does, a
/// node.** Every node is two blocks, and once a tree outgrew the cache each
/// was a `malloc` and a `free` from the system allocator. A 2¹⁶-node tree cost
/// 2.71 times a 63-node one a node on LLVM, and 2.16 times on the
/// copy-and-patch backend. Blocks up to 1 KiB now come from the runtime's own
/// pages, and that's 1.47 and 1.37.
#[test]
fn a_big_tree_costs_about_what_a_small_one_does_a_node() {
    let total = |depth: u32| {
        let nodes = (1u64 << depth) - 1;
        let one: u64 = (1..=nodes).map(|i| i % 7).sum();
        (nodes, one)
    };
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("trees", TREES);
        let mut cost = std::collections::BTreeMap::new();
        for depth in [6u32, 16] {
            let (nodes, one) = total(depth);
            let trees = (1u64 << 20) >> depth;
            let at = depth.to_string();
            let (n, _blocks) = per_op(
                backend,
                &binary,
                "trees",
                &[&at],
                trees * nodes,
                &format!("{}\n", one * trees),
            );
            eprintln!("{backend}, depth {depth}: {n:?} instructions a node");
            cost.insert(depth, n);
        }
        // Blocks freed on threads that didn't allocate them.
        let (nodes, one) = total(12);
        let rounds = (1u64 << 17) >> 12;
        per_op(
            backend,
            &binary,
            "parallel",
            &["12"],
            rounds * 8 * nodes,
            &format!("{}\n", one * 8 * rounds),
        );
        let (Some(Some(small)), Some(Some(big))) = (cost.get(&6).copied(), cost.get(&16).copied())
        else {
            continue;
        };
        if big * 10 > small * 18 {
            failures.push(format!("{backend}: a 2^16-node tree is {big} instructions a node against {small} for 63 nodes"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const NAMED_COMPARES: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/order" import { Order };
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Name(Str);

derive Equal, Ordered for Name;

fn named(ks: [Name], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let at = i % 1000;
    let after = match ((ks[at], ks[at + 1])) {
      (.Some(a), .Some(b)) => if (a.compare(b) == Order.Less) { 0 } else { 1 },
      _ => 0,
    };
    named(ks, i + 1, n, acc + after)
  }
}

fn plain(ks: [Str], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let at = i % 1000;
    let after = match ((ks[at], ks[at + 1])) {
      (.Some(a), .Some(b)) => if (a.compare(b) == Order.Less) { 0 } else { 1 },
      _ => 0,
    };
    plain(ks, i + 1, n, acc + after)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let ks = list.range(ctx, 0, 1001).mapCtx(ctx, fn(c, i) => str.format(c, "users.name.${20000 - i}").padStart(c, 24, 'x'));
  let names = ks.map(ctx, fn(k) => Name(k));
  let answer = match (args.first()) {
    .Some("named") => named(names, 0, 100000, 0),
    .Some("plain") => plain(ks, 0, 100000, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A derived order over a `Str` field costs about what comparing the `Str`
/// does.** The derived `compare` asked whether one field was less than the
/// other and then whether it was greater, and each question compared the two
/// strings from their first byte, so a field that wasn't less cost two
/// comparisons. One comparison answers both.
#[test]
fn a_derived_order_over_text_compares_the_text_once() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("named-compares", NAMED_COMPARES);
        let want = format!("{LOOKUPS}\n");
        let (named, _) = per_op(backend, &binary, "named", &[], LOOKUPS, &want);
        let (plain, _) = per_op(backend, &binary, "plain", &[], LOOKUPS, &want);
        eprintln!("{backend}: named {named:?}, plain {plain:?} instructions a compare");
        let (Some(named), Some(plain)) = (named, plain) else {
            continue;
        };
        if named * 10 > plain * 13 {
            failures.push(format!(
                "{backend}: a derived order over a `Str` is {named} instructions a compare \
                 against {plain} for the `Str` itself"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `mode` asked 100,000 times of a 16-element list, alternating its last
/// element (an equal copy) and one it doesn't hold, through `contains` or a
/// scan written by hand.
const SCANS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Name(Str);

derive Equal for Name;

fn has<T: Equal>(xs: [T], x: T, at: Int): Bool {
  match (xs[at]) {
    .None => false,
    .Some(y) => if (y == x) { true } else { has(xs, x, at + 1) },
  }
}

fn listed<T: Equal>(xs: [T], needles: [T], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let found = match (needles[i % 2]) {
      .Some(x) => if (xs.contains(x)) { 1 } else { 0 },
      .None => 0,
    };
    listed(xs, needles, i + 1, n, acc + found)
  }
}

fn handwritten<T: Equal>(xs: [T], needles: [T], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let found = match (needles[i % 2]) {
      .Some(x) => if (has(xs, x, 0)) { 1 } else { 0 },
      .None => 0,
    };
    handwritten(xs, needles, i + 1, n, acc + found)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let keys = list.range(ctx, 0, 17).mapCtx(ctx, fn(c, i) => str.format(c, "users.name.${i}").padStart(c, 24, 'x'));
  let names = keys.take(ctx, 16).map(ctx, fn(k) => Name(k));
  let copy = match (names.last()) { .Some(k) => Name(str.format(ctx, "${k.0}")), .None => Name("") };
  let missing = match (keys.last()) { .Some(k) => Name(k), .None => Name("") };
  let ints = list.range(ctx, 0, 16);
  let n = 100000;
  let answer = match (args.first()) {
    .Some("names") => listed(names, [copy, missing], 0, n, 0),
    .Some("handNames") => handwritten(names, [copy, missing], 0, n, 0),
    .Some("ints") => listed(ints, [15, 16], 0, n, 0),
    .Some("handInts") => handwritten(ints, [15, 16], 0, n, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **`contains` costs about what a scan written by hand does, and allocates
/// no more than it.** It asked `any` with a closure over the needle, so every call
/// allocated that closure and called it through a pointer for each element.
#[test]
fn contains_costs_what_a_scan_written_by_hand_does() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("scans", SCANS);
        let want = format!("{}\n", LOOKUPS / 2);
        for (mode, hand) in [("names", "handNames"), ("ints", "handInts")] {
            let (listed, blocks) = per_op(backend, &binary, mode, &[], LOOKUPS, &want);
            let (handwritten, by_hand) = per_op(backend, &binary, hand, &[], LOOKUPS, &want);
            eprintln!(
                "{backend} {mode}: contains {listed:?}, by hand {handwritten:?} instructions, \
                 {blocks} blocks"
            );
            if blocks > by_hand {
                failures.push(format!(
                    "{backend} {mode}: {LOOKUPS} `contains` allocated {blocks} blocks against \
                     {by_hand} for a scan written by hand"
                ));
            }
            let (Some(listed), Some(handwritten)) = (listed, handwritten) else {
                continue;
            };
            if listed * 10 > handwritten * 13 {
                failures.push(format!(
                    "{backend} {mode}: `contains` is {listed} instructions against \
                     {handwritten} for a scan written by hand"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `equal` asks 100,000 times whether a `Str` equals the very same `Str`,
/// held in a list so each comparison reads it afresh. The second argument is
/// its length in bytes.
const SAME_TEXT: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn equal(xs: [Str], x: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else {
    let same = match (xs[i % 2]) { .Some(y) => if (y == x) { 1 } else { 0 }, .None => 0 };
    equal(xs, x, i + 1, n, acc + same)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let text = "x".repeat(ctx, size);
  let answer = match (args.first()) {
    .Some("equal") => equal([text, text], text, 0, 100000, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A `Str` asked whether it equals itself costs the same at any length.**
/// `==` read every byte of the two, even when they were the same bytes.
#[test]
fn text_equal_to_itself_costs_the_same_at_any_length() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("same-text", SAME_TEXT);
        let want = format!("{LOOKUPS}\n");
        let (short, _) = per_op(backend, &binary, "equal", &["8"], LOOKUPS, &want);
        let (long, _) = per_op(backend, &binary, "equal", &["4096"], LOOKUPS, &want);
        eprintln!("{backend}: 8 bytes {short:?}, 4096 bytes {long:?} instructions");
        let (Some(short), Some(long)) = (short, long) else {
            continue;
        };
        if long * 10 > short * 13 {
            failures.push(format!(
                "{backend}: 4096 bytes against themselves are {long} instructions against \
                 {short} for 8"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
