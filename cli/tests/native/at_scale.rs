//! The programs the runtime's and the standard library's fast paths are for,
//! at the size they're for, on JavaScript and every native backend built in,
//! under the heap check.
//!
//! Each fast path has a cost bound elsewhere (`fan_out`, `map_keys`,
//! `collection_costs`), which counts work on the release backend or the
//! instructions where the kernel counts them. These rows ask only what a user
//! sees: the answer, the exit status, and every block given back. Each answer
//! is worked out here in Rust rather than read off a run.

use crate::agreement::{agree, skip_reason};

/// `(acc * 1000003 + x + 1) % 1000000007`, the positional digest every program
/// here folds its answers into. It fits an `I64` at every step.
fn mix(acc: i64, x: i64) -> i64 {
    (acc * 1_000_003 + x + 1) % 1_000_000_007
}

/// The same, as Buri.
const MIX: &str = "fn mix(acc: Int, x: Int): Int { (acc * 1000003 + x + 1) % 1000000007 }";

/// Digits in `n`'s decimal form, a minus sign included.
fn digits(n: i64) -> i64 {
    n.to_string().len() as i64
}

macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            crate::ci::skipped("backend agreement", &why);
            return;
        }
    };
}

/// **Fan-outs of every width answer in order** (PERFORMANCE.md §6.73–§6.75).
///
/// Thousands of rounds of 64 one-multiply steps, the shape whose wake-ups
/// were a broadcast and whose joiner now runs steps itself. Then widths either
/// side of the 64 stacks the pool keeps and of the 1,024-step window, up to
/// 3,000, where a step maps its stack on its first turn and the window refills
/// 64 at a time. Each wide fan-out answers a `Str` a step and hands it to a
/// second one, so a step dropped, run twice or answered out of order changes
/// the digest, and a count lost across threads is a leak or a use after free.
/// Last, eight steps that each fan out 200, which park rather than help.
#[test]
fn fan_outs_of_every_width_answer_in_order_on_every_backend() {
    rows_or_skip!();
    let widths = [1, 63, 64, 65, 1023, 1024, 1025, 1089, 3000];
    let tiny = (0..400).fold(0, |acc, _| mix(acc, (0..64).map(|i| 3 * i).sum()));
    let wide = |n: i64| {
        let again = (0..n).fold(0, |acc, i| mix(acc, digits(4 * i) + 1 + i));
        (0..n).fold(again, |acc, i| mix(acc, 4 * i))
    };
    let nested = (0..8).fold(0, |acc, x| mix(acc, (0..200).fold(0, |a, j| mix(a, x * 1000 + 2 * j))));
    let mut expected = format!("tiny {tiny}\n");
    for n in widths {
        expected += &format!("wide {n} {}\n", wide(n));
    }
    expected += &format!("nested {nested}\n");
    let widths = widths.map(|n| n.to_string()).join(", ");
    let source = format!(
        r#"
from "platform/effect" import {{ Allocator, Tasks }};
from "native" import {{ NativeHost }};
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

{MIX}

fn rounds<C: Allocator + Tasks>(ctx: C, k: Int, acc: Int): Int {{
  if (k == 0) {{
    acc
  }} else {{
    let ys = tasks.parallel(ctx, list.range(ctx, 0, 64), fn(c, i, x) => x * 2 + i);
    rounds(ctx, k - 1, mix(acc, ys.fold(fn(a, y) => a + y, 0)))
  }}
}}

fn wide<C: Allocator + Tasks>(ctx: C, n: Int): Int {{
  let words = tasks.parallel(ctx, list.range(ctx, 0, n), fn(c, i, x) => str.fromInt(c, x * 3 + i));
  let again = tasks.parallel(ctx, words, fn(c, i, w) => w.concat(c, "!").length() + i);
  words.fold(fn(a, w) => mix(a, w.toInt().withDefault(-1)), again.fold(fn(a, y) => mix(a, y), 0))
}}

fn nested<C: Allocator + Tasks>(ctx: C): Int {{
  let outer = tasks.parallel(ctx, list.range(ctx, 0, 8), fn(c, i, x) =>
    tasks.parallel(c, list.range(c, 0, 200), fn(d, j, y) => x * 1000 + y + j).fold(fn(a, y) => mix(a, y), 0));
  outer.fold(fn(a, y) => mix(a, y), 0)
}}

fn widths<C: Allocator + Tasks>(ctx: C, host: NativeHost, ns: [Int], at: Int): Int {{
  match (ns.get(at)) {{
    .Some(n) => {{
      let _ = io.println(host.stdout, "wide ${{n}} ${{wide(ctx, n)}}").ignore();
      widths(ctx, host, ns, at + 1)
    }},
    .None => at,
  }}
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Tasks: host.tasks }};
  let _ = io.println(host.stdout, "tiny ${{rounds(ctx, 400, 0)}}").ignore();
  let _ = widths(ctx, host, [{widths}], 0);
  let _ = io.println(host.stdout, "nested ${{nested(ctx)}}").ignore();
  .Ok(())
}}
"#
    );
    agree("fan-outs of every width", &source, &expected);
}

/// **Maps keyed by wide `Int`s, fractional floats and timestamps keep every
/// entry** (PERFORMANCE.md §6.64, §6.65). These keys all hashed alike once, so
/// a map of them was one collision list. Each shape builds a map of 4,000 keys
/// from one list, reads every key back, asks 4,000 keys it never held, removes
/// the half with an even value, and walks what's left.
#[test]
fn maps_keyed_by_wide_ints_fractions_and_timestamps_keep_every_entry_on_every_backend() {
    rows_or_skip!();
    let n: i64 = 4000;
    // (name, base, step) for `base + i * step`; a miss is `i + n`.
    let ints: [(&str, &str, &str); 6] = [
        ("low", "0", "1"),
        ("bit32", "0", "4294967296"),
        ("negative", "0 - 8589934592000", "4294967296"),
        ("bit40", "0", "1099511627776"),
        ("top", "4611686018427387904", "4294967296"),
        ("millis", "1700000000000", "1000"),
    ];
    // (name, base, scale) for `base + i * scale`; a miss is `i + 0.5`.
    let floats: [(&str, &str, &str); 6] = [
        ("whole", "0.0", "1.0"),
        ("fractions", "0.0", "1.0 / 16000.0"),
        ("negative fractions", "0.0 - 0.125", "0.0001"),
        ("wide floats", "0.0", "4294967296.0"),
        ("float millis", "1700000000000.0", "1.0"),
        ("seconds", "1700000000.0", "0.001"),
    ];
    let mut calls = String::new();
    let mut expected = String::new();
    let all: i64 = (0..n).sum();
    let odd: i64 = (0..n).filter(|i| i % 2 == 1).sum();
    for (name, base, step) in ints {
        calls += &format!(
            "  let _ = io.println(host.stdout, report(ctx, \"{name}\", ints(ctx, {n}, {base}, {step}, 0), ints(ctx, {n}, {base}, {step}, {n}))).ignore();\n"
        );
        expected += &format!("{name} {n} {all} 0 {all} {} {odd} {odd}\n", n / 2);
    }
    for (name, base, scale) in floats {
        calls += &format!(
            "  let _ = io.println(host.stdout, report(ctx, \"{name}\", floats(ctx, {n}, {base}, {scale}, 0.0), floats(ctx, {n}, {base}, {scale}, 0.5))).ignore();\n"
        );
        expected += &format!("{name} {n} {all} 0 {all} {} {odd} {odd}\n", n / 2);
    }
    let source = format!(
        r#"
from "native" import {{ NativeHost }};
from "core/io" import * as io;
from "core/list" import * as list;
from "core/map" import * as map;
from "core/map" import {{ Map }};
from "core/order" import {{ Equal, Hash }};
from "core/str" import * as str;
from "platform/effect" import {{ Allocator }};

fn ints<C: Allocator>(ctx: C, n: Int, base: Int, step: Int, offset: Int): [Int] {{
  list.range(ctx, 0, n).map(ctx, fn(i) => base + (i + offset) * step)
}}

fn floats<C: Allocator>(ctx: C, n: Int, base: F64, scale: F64, offset: F64): [F64] {{
  list.range(ctx, 0, n).map(ctx, fn(i) => base + (i.toF64() + offset) * scale)
}}

fn walked<K>(all: [(K, Int)]): Int {{
  all.fold(fn(a, e) => a + e.1, 0)
}}

/// Each key's value is how many keys came before it, so a key held twice
/// shows up in the size.
fn report<K: Hash + Equal, C: Allocator>(ctx: C, name: Str, keys: [K], misses: [K]): Str {{
  let m = keys.foldCtx(ctx, fn(c, acc: Map<K, Int>, k) => acc.insert(c, k, acc.size), map.empty());
  let hits = keys.fold(fn(a, k) => a + m.get(k).withDefault(0 - 1000000), 0);
  let missed = misses.fold(fn(a, k) => a + (if (m.has(k)) {{ 1 }} else {{ 0 }}), 0);
  let walk = walked(m.entries(ctx));
  let odd = keys.foldCtx(ctx, fn(c, acc: Map<K, Int>, k) => {{
    if (acc.get(k).withDefault(1) % 2 == 0) {{ acc.remove(c, k) }} else {{ acc }}
  }}, m);
  let after = keys.fold(fn(a, k) => a + odd.get(k).withDefault(0), 0);
  str.format(ctx, "${{name}} ${{m.size}} ${{hits}} ${{missed}} ${{walk}} ${{odd.size}} ${{after}} ${{walked(odd.entries(ctx))}}")
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc }};
{calls}  .Ok(())
}}
"#
    );
    agree("maps keyed by wide ints, fractions and timestamps", &source, &expected);
}

/// **An ordered map of wide values, and `filterMap` and `values` over records
/// that own strings, answer the same at every size** (PERFORMANCE.md §6.60,
/// §6.61). A lookup reads keys alone and copies only the value it answers, a
/// node splice writes in place where nothing else holds the node, `values`
/// flattens the leaves, and `filterMap` moves each payload into one block. A
/// count lost on any of those paths is a leak, a use after free or a wrong
/// digest.
#[test]
fn ordered_maps_of_wide_values_and_filter_map_answer_on_every_backend() {
    rows_or_skip!();
    let n: i64 = 5000;
    // The value under key `k` is element `e` with `e * 7 % n == k`.
    let e_of = |k: i64| (0..n).find(|e| e * 7 % n == k).unwrap();
    let by_key: Vec<i64> = (0..n).map(e_of).collect();
    let read = |e: i64| 3 * e + digits(2 * e) + 2;
    let gets = |removed: &dyn Fn(i64) -> bool| {
        (0..n).fold(0, |acc, k| mix(acc, if removed(k) { -1 } else { read(by_key[k as usize]) }))
    };
    let whole = gets(&|_| false);
    let thinned = gets(&|k| k % 3 == 0 && k < 3 * (n / 3));
    let flat_n: i64 = 3000;
    let expected = format!(
        "wide {n} {whole} {n}\n\
         values {n} {} {}\n\
         keys {n} {}\n\
         edits {n} {} {} {whole} {thinned} 199998\n\
         filterMap {} {} {}\n\
         filterMapCtx {} {}\n\
         ends 0 {n}\n\
         flat {flat_n} {} {} {flat_n}\n",
        by_key.iter().fold(0, |acc, &e| mix(acc, e)),
        (0..n).map(|e| 1 + digits(e)).sum::<i64>(),
        (0..n).fold(0, mix),
        n - n / 3,
        n - n / 3,
        (0..n).filter(|e| e % 3 == 0).count(),
        2 * (0..n).filter(|e| e % 3 == 0).count(),
        (0..n).filter(|e| e % 3 == 0).sum::<i64>(),
        (0..n).filter(|e| e % 2 == 1).count(),
        (0..n).filter(|e| e % 2 == 1).map(|e| 2 + digits(e) + digits(3 * e)).sum::<i64>(),
        (0..flat_n).map(|i| i + 63).sum::<i64>(),
        (0..flat_n).sum::<i64>(),
    );
    let fields: String = (0..64).map(|j| format!("  f{j}: Int,\n")).collect();
    let inits: Vec<String> = (0..64).map(|j| format!("f{j}: i + {j}")).collect();
    let inits = inits.join(", ");
    let source = format!(
        r#"
from "platform/effect" import {{ Allocator }};
from "native" import {{ NativeHost }};
from "core/io" import * as io;
from "core/list" import * as list;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import {{ OrderedMap }};
from "core/str" import * as str;

{MIX}

struct Wide {{ a: Str, b: Str, e: Int, f: Int, tags: [Str] }}

struct Flat {{
{fields}}}

fn wide<C: Allocator>(ctx: C, i: Int): Wide {{
  let s = str.format(ctx, "w${{i}}");
  Wide {{ a: s, b: str.fromInt(ctx, i * 2), e: i, f: i * 3, tags: [s, str.fromInt(ctx, i)] }}
}}

fn flat(i: Int): Flat {{ Flat {{ {inits} }} }}

fn gets(m: OrderedMap<Int, Wide>, k: Int, n: Int, acc: Int): Int {{
  if (k >= n) {{
    acc
  }} else {{
    gets(m, k + 1, n, mix(acc, m.get(k).map(fn(w) => w.f + w.b.length() + w.tags.length()).withDefault(0 - 1)))
  }}
}}

fn hases(m: OrderedMap<Int, Wide>, k: Int, n: Int, acc: Int): Int {{
  if (k >= n) {{ acc }} else {{ hases(m, k + 1, n, acc + (if (m.has(k)) {{ 1 }} else {{ 0 }})) }}
}}

fn flatGets(m: OrderedMap<Int, Flat>, k: Int, n: Int, acc: Int): Int {{
  if (k >= n) {{ acc }} else {{ flatGets(m, k + 1, n, acc + m.get(k).map(fn(v) => v.f63).withDefault(0)) }}
}}

fn flatHases(m: OrderedMap<Int, Flat>, k: Int, n: Int, acc: Int): Int {{
  if (k >= n) {{ acc }} else {{ flatHases(m, k + 1, n, acc + (if (m.has(k)) {{ 1 }} else {{ 0 }})) }}
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc }};
  let n = {n};
  let xs = list.range(ctx, 0, n).mapCtx(ctx, fn(c, i) => wide(c, i));
  let m = orderedmap.of(ctx, xs.map(ctx, fn(w) => (w.e * 7 % n, w)));
  let _ = io.println(host.stdout, "wide ${{m.length()}} ${{gets(m, 0, n, 0)}} ${{hases(m, 0 - 3, n + 3, 0)}}").ignore();
  let vs = m.values(ctx);
  let _ = io.println(host.stdout, "values ${{vs.length()}} ${{vs.fold(fn(a, w) => mix(a, w.e), 0)}} ${{vs.fold(fn(a, w) => a + w.a.length(), 0)}}").ignore();
  let ks = m.keys(ctx);
  let _ = io.println(host.stdout, "keys ${{ks.length()}} ${{ks.fold(fn(a, k) => mix(a, k), 0)}}").ignore();

  // Edits through a second name leave the first as it was.
  let fewer = list.range(ctx, 0, n / 3).foldCtx(ctx, fn(c, acc: OrderedMap<Int, Wide>, i) => acc.remove(c, i * 3), m);
  let more = fewer.insert(ctx, 1, wide(ctx, 99999));
  let replaced = more.get(1).map(fn(w) => w.b).withDefault("none");
  let _ = io.println(host.stdout, "edits ${{m.length()}} ${{fewer.length()}} ${{more.length()}} ${{gets(m, 0, n, 0)}} ${{gets(fewer, 0, n, 0)}} ${{replaced}}").ignore();

  let kept = xs.filterMap(ctx, fn(w) => if (w.e % 3 == 0) {{ .Some(w.tags) }} else {{ .None }});
  let tags = kept.fold(fn(a, t) => a + t.length(), 0);
  let seconds = kept.fold(fn(a, t) => a + t.get(1).andThen(fn(s) => s.toInt()).withDefault(0), 0);
  let _ = io.println(host.stdout, "filterMap ${{kept.length()}} ${{tags}} ${{seconds}}").ignore();
  let named = xs.filterMapCtx(ctx, fn(c, w) => if (w.e % 2 == 1) {{ .Some(str.format(c, "${{w.a}}-${{w.f}}")) }} else {{ .None }});
  let _ = io.println(host.stdout, "filterMapCtx ${{named.length()}} ${{named.fold(fn(a, s) => a + s.length(), 0)}}").ignore();
  let none = xs.filterMap(ctx, fn(w) => if (w.e < 0) {{ .Some(w) }} else {{ .None }});
  let all = xs.filterMap(ctx, fn(w) => .Some(w));
  let _ = io.println(host.stdout, "ends ${{none.length()}} ${{all.length()}}").ignore();

  let fm = orderedmap.of(ctx, list.range(ctx, 0, {flat_n}).map(ctx, fn(i) => (i, flat(i))));
  let f0 = fm.values(ctx).fold(fn(a, v) => a + v.f0, 0);
  let _ = io.println(host.stdout, "flat ${{fm.length()}} ${{flatGets(fm, 0, {flat_n}, 0)}} ${{f0}} ${{flatHases(fm, 0 - 3, {flat_n} + 3, 0)}}").ignore();
  .Ok(())
}}
"#
    );
    agree("ordered maps of wide values, filterMap and values", &source, &expected);
}

/// `total` of the tree `grow(depth, at)` builds in [`TREES`].
fn tree_total(depth: u32, at: i64) -> i64 {
    if depth == 0 {
        digits(at % 1000)
    } else {
        at % 7 + tree_total(depth - 1, at * 2) + tree_total(depth - 1, at * 2 + 1)
    }
}

/// Trees of up to 2¹⁷ leaves, each node a block and each leaf a `Str`.
const TREES: &str = r#"
from "platform/effect" import { Allocator, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

enum Tree { Leaf(Str), Branch(Fork) }
struct Fork { left: Tree, right: Tree, weight: Int }

fn grow<C: Allocator>(ctx: C, depth: Int, at: Int): Tree {
  if (depth == 0) {
    .Leaf(str.fromInt(ctx, at % 1000))
  } else {
    .Branch(Fork { left: grow(ctx, depth - 1, at * 2), right: grow(ctx, depth - 1, at * 2 + 1), weight: at % 7 })
  }
}

fn total(t: Tree): Int {
  match (t) {
    .Leaf(s) => s.length(),
    .Branch(f) => f.weight + total(f.left) + total(f.right),
  }
}

fn once<C: Allocator>(ctx: C, depth: Int): Int { total(grow(ctx, depth, 1)) }

fn many<C: Allocator>(ctx: C, k: Int, depth: Int, acc: Int): Int {
  if (k == 0) { acc } else { many(ctx, k - 1, depth, acc + once(ctx, depth)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Tasks: host.tasks };
  // Each big tree is freed before the next is built, past what the pool keeps.
  let _ = io.println(host.stdout, "big ${once(ctx, 17)} ${once(ctx, 17)} ${once(ctx, 16)}").ignore();
  let _ = io.println(host.stdout, "small ${many(ctx, 1000, 6, 0)}").ignore();
  // Built on the workers, read and dropped by the caller.
  let grown = tasks.parallel(ctx, list.range(ctx, 0, 8), fn(c, i, x) => grow(c, 12, x + 1));
  let _ = io.println(host.stdout, "grown ${grown.fold(fn(a, t) => a + total(t), 0)}").ignore();
  // Read on the workers while the caller holds them.
  let read = tasks.parallel(ctx, grown, fn(c, i, t) => total(t));
  let _ = io.println(host.stdout, "read ${read.fold(fn(a, y) => a + y, 0)}").ignore();
  // One tree held while others are built and freed beside it.
  let kept = grow(ctx, 15, 1);
  let churn = tasks.parallel(ctx, list.range(ctx, 0, 8), fn(c, i, x) => total(grow(c, 14, x + 1)));
  let _ = io.println(host.stdout, "kept ${total(kept)} ${churn.fold(fn(a, y) => a + y, 0)}").ignore();
  .Ok(())
}
"#;

/// **Deep trees built, freed and built again, on one thread and across
/// threads, keep every node** (PERFORMANCE.md §6.67). Blocks come from the
/// runtime's pages, a drained program gives pages past 8 MB back, and the
/// next tree takes them again. A 2¹⁷-leaf tree is about 12 MB of blocks.
///
/// The heap check takes freed blocks into its quarantine rather than the
/// cache, so the release that gives pages back happens only without it. The
/// native programs also run as they ship, with the allocation probe counting
/// what's live at exit.
#[test]
fn deep_trees_freed_and_built_again_keep_every_node_on_every_backend() {
    rows_or_skip!();
    let expected = format!(
        "big {} {} {}\nsmall {}\ngrown {}\nread {}\nkept {} {}\n",
        tree_total(17, 1),
        tree_total(17, 1),
        tree_total(16, 1),
        1000 * tree_total(6, 1),
        (1..=8).map(|x| tree_total(12, x)).sum::<i64>(),
        (1..=8).map(|x| tree_total(12, x)).sum::<i64>(),
        tree_total(15, 1),
        (1..=8).map(|x| tree_total(14, x)).sum::<i64>(),
    );
    agree("deep trees freed and built again", TREES, &expected);
    for (backend, build) in crate::e2e::probed_backends() {
        let r = crate::shared::ran_command(
            std::process::Command::new(build("at-scale-trees", TREES)).env_remove("BURI_RT_HEAP_CHECK"),
        );
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(r.stdout, expected, "{backend}: {}", r.stderr);
        let (_, live) = crate::shared::probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}

/// The key of element `i` of `n` in [`sorting_long_lists_agrees_on_every_backend`]'s
/// shapes: ascending, descending, shuffled, shuffled with ties, and ascending
/// with one more on the end.
fn long_sort_key(shape: i64, n: i64, i: i64) -> i64 {
    match shape {
        0 => i,
        1 => n - i,
        2 => (i * 7919) % 20011,
        3 => (i * 7919) % 20011 % 100,
        _ => {
            if i == n - 1 {
                0
            } else {
                i
            }
        }
    }
}

/// **Sorting 20,000 elements already in order, reversed or shuffled is stable
/// and gives back every element** (PERFORMANCE.md §6.66). A run in order is
/// copied, a run in reverse is copied from the end, and two runs in order
/// across a merge are copied or swapped rather than merged. Every element
/// holds a `Str`, so a lost or doubled count is a leak or a use after free.
#[test]
fn sorting_long_lists_agrees_on_every_backend() {
    rows_or_skip!();
    let n: i64 = 20_000;
    let mut expected = String::new();
    for shape in 0..5 {
        let items: Vec<(i64, i64)> = (0..n).map(|i| (long_sort_key(shape, n, i), i)).collect();
        let mut up = items.clone();
        up.sort_by_key(|&(k, _)| k);
        let mut down = items.clone();
        down.sort_by_key(|&(k, _)| std::cmp::Reverse(k));
        let mut tags: Vec<String> = items.iter().map(|&(k, _)| k.to_string()).collect();
        tags.sort();
        let digest = |xs: &[(i64, i64)]| xs.iter().fold(0, |acc, &(k, at)| mix(acc, k + at + digits(at)));
        let tag_digest = tags.iter().fold(0, |acc, t| mix(acc, t.parse::<i64>().unwrap()));
        expected += &format!("s{shape}: {} {} {tag_digest}\n", digest(&up), digest(&down));
    }
    let source = format!(
        r#"
from "native" import {{ NativeHost }};
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "platform/effect" import {{ Allocator }};

{MIX}

struct Item {{ key: Int, at: Int, tag: Str }}

fn keyOf(shape: Int, n: Int, i: Int): Int {{
  match (shape) {{
    0 => i,
    1 => n - i,
    2 => (i * 7919) % 20011,
    3 => (i * 7919) % 20011 % 100,
    _ => if (i == n - 1) {{ 0 }} else {{ i }},
  }}
}}

fn digest(xs: [Item]): Int {{ xs.fold(fn(acc, it) => mix(acc, it.key + it.at + it.tag.length()), 0) }}

fn line<C: Allocator>(ctx: C, shape: Int, n: Int): Str {{
  let xs = list.range(ctx, 0, n).mapCtx(ctx, fn(c, i) => Item {{ key: keyOf(shape, n, i), at: i, tag: str.fromInt(c, i) }});
  let up = xs.sortBy(ctx, fn(p, q) => p.key.compare(q.key));
  let down = xs.sortBy(ctx, fn(p, q) => q.key.compare(p.key));
  let tags = xs.mapCtx(ctx, fn(c, it) => str.fromInt(c, it.key)).sort(ctx);
  let tagged = tags.fold(fn(acc, s) => mix(acc, s.toInt().withDefault(-1)), 0);
  str.format(ctx, "s${{shape}}: ${{digest(up)}} ${{digest(down)}} ${{tagged}}")
}}

fn lines<C: Allocator>(ctx: C, host: NativeHost, shape: Int): Int {{
  if (shape == 5) {{
    shape
  }} else {{
    let _ = io.println(host.stdout, line(ctx, shape, {n})).ignore();
    lines(ctx, host, shape + 1)
  }}
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc }};
  let _ = lines(ctx, host, 0);
  .Ok(())
}}
"#
    );
    agree("sorting long lists", &source, &expected);
}

/// **Lengths, indices and ranges either side of 1,024 answer the same
/// everywhere** (PERFORMANCE.md §6.53). JavaScript reads a count below 1,024
/// from a table and makes one at or past it, and `range` counts in a `number`
/// while it answers `Int`s, however large.
#[test]
fn counts_either_side_of_1024_agree_on_every_backend() {
    rows_or_skip!();
    let mut expected = String::new();
    for n in [1023, 1024, 1025] {
        expected += &format!("{n}: {n} {} {} 0\n", n - 1, n - 1);
    }
    expected += "range 4611686018427387904,4611686018427387905,4611686018427387906\n";
    expected += "negative 10 -1030 -1021\n";
    agree(
        "counts either side of 1024",
        r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "platform/effect" import { Allocator };

fn line<C: Allocator>(ctx: C, n: Int): Str {
  let xs = list.range(ctx, 0, n);
  let found = xs.findIndex(fn(x) => x == n - 1).withDefault(-1);
  let at = xs.indexOf(n - 1).withDefault(-1);
  let last = xs.lastIndexOf(0).withDefault(-1);
  str.format(ctx, "${n}: ${xs.length()} ${found} ${at} ${last}")
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc };
  let _ = io.println(host.stdout, line(ctx, 1023)).ignore();
  let _ = io.println(host.stdout, line(ctx, 1024)).ignore();
  let _ = io.println(host.stdout, line(ctx, 1025)).ignore();
  let big = list.range(ctx, 4611686018427387904, 4611686018427387907);
  let _ = io.println(host.stdout, "range ${big.mapCtx(ctx, fn(c, x) => str.fromInt(c, x)).join(ctx, ",")}").ignore();
  let low = list.range(ctx, 0 - 1030, 0 - 1020);
  let _ = io.println(host.stdout, "negative ${low.length()} ${low.first().withDefault(0)} ${low.last().withDefault(0)}").ignore();
  .Ok(())
}
"#,
        &expected,
    );
}
