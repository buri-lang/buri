//! What `core/orderedmap`'s lookups and `values`, and `core/list`'s
//! `filterMap`, cost against the shape of what they hold, natively, on every
//! native backend this toolchain has built in.
//!
//! Each program runs once doing nothing past its setup and once doing the work,
//! so the difference is the work alone. The work is counted rather than timed:
//! instructions retired where the kernel counts them (macOS on hardware), and
//! blocks through the allocation probe everywhere. The counted runs are the
//! program as it ships, and one more of each runs under the heap check.

use crate::shared::{exited_instructions, heap_checked, probed, ran_command, Ran};
use std::path::Path;

/// One run, with the fewer instructions of two, because a short process's
/// count only ever gains noise (`design/PERFORMANCE.md` §8). `None` where the
/// kernel counts nothing, such as CI's virtual machines.
fn measured(binary: &Path, args: &[&str]) -> (Ran, Option<u64>) {
    let once = || {
        let mut cmd = std::process::Command::new(binary);
        cmd.args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd.spawn().unwrap();
        let instructions = exited_instructions(child.id());
        let out = child.wait_with_output().unwrap();
        let ran = Ran {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        };
        (ran, instructions)
    };
    let (ran, first) = once();
    let (_, second) = once();
    (ran, first.zip(second).map(|(a, b)| a.min(b)).filter(|&n| n > 0))
}

/// The instructions one operation costs, from `ops` of them against none, and
/// the blocks all of them allocated. `args` are the program's after its mode.
fn per_op(backend: &str, binary: &Path, mode: &str, args: &[&str], ops: u64, expected: &str) -> (Option<u64>, u64) {
    let (idle, idle_n) = measured(binary, &[&["none"], args].concat());
    let (busy, busy_n) = measured(binary, &[&[mode], args].concat());
    let checked = ran_command(heap_checked(std::process::Command::new(binary).args([&[mode], args].concat())));
    for (r, want) in [(&idle, "0\n"), (&busy, expected), (&checked, expected)] {
        assert_eq!(r.status, 0, "{backend} {mode} {args:?}: {}", r.stderr);
        assert_eq!(r.stdout, want, "{backend} {mode} {args:?}: {}", r.stderr);
    }
    let (idle_blocks, idle_live) = probed(&idle.stderr);
    let (busy_blocks, busy_live) = probed(&busy.stderr);
    assert_eq!((idle_live, busy_live), (0, 0), "{backend} {mode}: blocks still live at exit");
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
    let fields = |n: usize, f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<Vec<_>>().join(", ");
    let shapes = [
        ("one Int", fields(1, &|i| format!("f{i}: Int")), fields(1, &|i| format!("f{i}: i")), "v.f0"),
        ("64 Ints", fields(64, &|i| format!("f{i}: Int")), fields(64, &|i| format!("f{i}: i")), "v.f0"),
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
            let source = lookups(&format!("struct Value {{ {value} }}"), &format!("Value {{ {make} }}"), read);
            let binary = build(&format!("orderedmap-lookup-{i}"), &source);
            for size in sizes {
                let keys = (0..LOOKUPS).map(|j| j * 3 % size);
                let got: u64 = if i == 2 { keys.map(|k| 1 + k.to_string().len() as u64).sum() } else { keys.sum() };
                let at = size.to_string();
                let (get, get_blocks) = per_op(backend, &binary, "get", &[&at], LOOKUPS, &format!("{got}\n"));
                let (has, has_blocks) = per_op(backend, &binary, "has", &[&at], LOOKUPS, &format!("{LOOKUPS}\n"));
                assert_eq!((get_blocks, has_blocks), (0, 0), "{backend}, {name}: a lookup allocated");
                eprintln!("{backend}, {name}, {size} keys: get {get:?}, has {has:?} instructions");
                costs.insert((i, size), (get, has));
            }
        }
        for size in sizes {
            let Some((_, Some(has1))) = costs.get(&(0, size)).copied() else { continue };
            for (i, (name, ..)) in shapes.iter().enumerate().skip(1) {
                let Some((_, Some(has))) = costs.get(&(i, size)).copied() else { continue };
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
            let (Some(shallow), Some(deep)) = (value(7), value(2000)) else { continue };
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
        let Some(Some(map)) = cost.get("map").copied() else { continue };
        for (mode, times) in [("filterMap", 13), ("values", 13)] {
            let Some(Some(n)) = cost.get(mode).copied() else { continue };
            if n * 10 > map * times + 200 {
                failures.push(format!("{backend}: {mode} is {n} instructions an element against map's {map}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
