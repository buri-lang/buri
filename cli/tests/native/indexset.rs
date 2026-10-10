//! What a `core/indexset` set costs, natively, on every native backend this
//! toolchain has built in: the bytes a moving window allocates, and the
//! instructions `has` and the set operations take against the set's size.
//!
//! The bytes and blocks are read through the runtime's own heap statistics, and
//! every run is under the heap check.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, probed, ran_command, ALLOC_PROBE};

/// [`ALLOC_PROBE`], plus a line with every byte the run allocated.
fn bytes_probe() -> String {
    format!(
        "{ALLOC_PROBE}
__attribute__((destructor)) static void buri_bytes_probe(void) {{
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, \"bytes=%llu\\n\", (unsigned long long)s.total_bytes);
}}
"
    )
}

/// The `bytes=` line a [`bytes_probe`]-linked run printed.
fn bytes(stderr: &str) -> u64 {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("bytes="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"))
        .trim()
        .parse()
        .unwrap()
}

/// `main <start> <steps>`: 128 members from `start`, then `steps` times the
/// next one in and the oldest out. It prints how many members are left.
const WINDOW: &str = r#"
from "core/env" import * as env;
from "core/indexset" import * as indexSet;
from "core/indexset" import { Index, IndexSet };
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

derive Index for Seq;
struct Seq(U64);

fn slide<C: Allocator>(ctx: C, s: IndexSet<Seq>, next: U64, steps: Int): IndexSet<Seq> {
  if (steps == 0) {
    s
  } else {
    slide(ctx, s.insert(ctx, Seq(next)).removeBelow(ctx, Seq(next - 127)), next + 1, steps - 1)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let start = args.get(0).andThen(fn(s) => s.toInt()).withDefault(0).wrapToU64();
  let steps = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let first = indexSet.range(ctx, Seq(start), Seq(start + 128));
  let last = slide(ctx, first, start + 128, steps);
  io.println(ctx, "${last.count()}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A window that moves stays as small as what it covers** (#296). The set
/// keeps its words from the word of its smallest member, so a window of 128
/// numbers near 2^62 allocates what one near zero does, byte for byte. A set
/// that kept its words from zero would need 2^56 of them.
///
/// Nothing else holds the set, so a step writes its words in place: every 64
/// steps one word arrives at the end and one leaves the front, a block each.
#[test]
fn a_moving_window_allocates_the_same_bytes_wherever_it_is() {
    let probe = bytes_probe();
    for (backend, build) in crate::e2e::probed_builds(&probe) {
        let binary = build("indexset-window", WINDOW);
        let run = |start: &str, steps: &str| {
            let mut cmd = std::process::Command::new(&binary);
            cmd.args([start, steps]);
            let ran = ran_command(heap_checked(&mut cmd));
            assert_eq!(ran.status, 0, "{backend} {start} {steps}: {}", ran.stderr);
            assert_eq!(ran.stdout, "128\n", "{backend} {start} {steps}: {}", ran.stderr);
            assert_eq!(probed(&ran.stderr).1, 0, "{backend} {start} {steps}: blocks still live at exit");
            (probed(&ran.stderr).0, bytes(&ran.stderr))
        };
        let blocks = ["0640", "1280", "1920"].map(|steps| {
            let (near, far) = (run("0000000000000000000", steps), run("4611686018427387904", steps));
            assert_eq!(near, far, "{backend}, {steps} steps: {near:?} blocks and bytes near zero, {far:?} near 2^62");
            near.0
        });
        let [a, b, c] = blocks;
        assert_eq!((b - a, c - b), (20, 20), "{backend}: 640 steps took {} blocks, then {}", b - a, c - b);
    }
}

/// `main <mode> <size> <ops>`: two sets of `size` members over the same span,
/// and `mode` asked of them `ops` times. `none` answers 0 after the same setup.
const COSTS: &str = r#"
from "core/env" import * as env;
from "core/indexset" import * as indexSet;
from "core/indexset" import { Index, IndexSet };
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

derive Index for Slot;
struct Slot(U32);

fn hases(s: IndexSet<Slot>, i: Int, n: Int, span: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let hit = if (s.has(Slot((i * 7 % span).wrapToU32()))) { 1 } else { 0 };
    hases(s, i + 1, n, span, acc + hit)
  }
}

fn times<C: Allocator>(ctx: C, k: Int, acc: Int, f: fn(C) => Int): Int {
  if (k == 0) { acc } else { times(ctx, k - 1, acc + f(ctx), f) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let ops = args.get(2).andThen(fn(s) => s.toInt()).withDefault(0);
  let evens = indexSet.of(ctx, list.range(ctx, 0, size).map(ctx, fn(i) => Slot((i * 2).wrapToU32())));
  let all = indexSet.range(ctx, Slot(0), Slot((size * 2).wrapToU32()));
  let answer = match (args.first()) {
    .Some("has") => hases(evens, 0, ops, size * 2, 0),
    .Some("union") => times(ctx, ops, 0, fn(c) => if (evens.union(c, all).isEmpty()) { 0 } else { 1 }),
    .Some("intersect") => times(ctx, ops, 0, fn(c) => if (evens.intersect(c, all).isEmpty()) { 0 } else { 1 }),
    .Some("difference") => times(ctx, ops, 0, fn(c) => if (all.difference(c, evens).isEmpty()) { 0 } else { 1 }),
    .Some("isSubsetOf") => times(ctx, ops, 0, fn(c) => if (evens.isSubsetOf(all)) { 1 } else { 0 }),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// How many times each operation runs: enough that the setup's own noise is
/// a fraction of an instruction an operation.
const LOOKUPS: u64 = 200_000;
const OPS: u64 = 2000;

/// **`has` reads one word, and a set operation walks the words** (#296).
/// Against a set 64 times larger, `has` costs the same instructions, and a set
/// operation the same instructions a word. `has` and `isSubsetOf` allocate
/// nothing, and each of the others one list, whatever the size. Instructions
/// are counted where the kernel counts them, and blocks everywhere.
#[test]
fn has_is_constant_and_set_operations_are_linear_in_words() {
    let (small, large) = (1024u64, 65536u64);
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("indexset-costs", COSTS);
        // The probes that land on an even number, which are the members.
        let hits = |n: u64| (0..LOOKUPS).filter(|i| (i * 7 % (n * 2)).is_multiple_of(2)).count();
        let mut cost = std::collections::BTreeMap::new();
        for size in [small, large] {
            let at = size.to_string();
            let lookups = LOOKUPS.to_string();
            let (has, has_blocks) =
                per_op(backend, &binary, "has", &[&at, &lookups], LOOKUPS, &format!("{}\n", hits(size)));
            assert_eq!(has_blocks, 0, "{backend}, {size}: `has` allocated");
            cost.insert(("has", size), has);
            for (mode, blocks) in [("union", 1), ("intersect", 1), ("difference", 1), ("isSubsetOf", 0)] {
                let (n, got) = per_op(backend, &binary, mode, &[&at, &OPS.to_string()], OPS, &format!("{OPS}\n"));
                // The one block over is the closure `main` hands `times`.
                assert_eq!(got / OPS, blocks, "{backend}, {size}: {OPS} of `{mode}` took {got} blocks");
                // Per word: `size` evens over twice the span is `size / 32` words.
                cost.insert((mode, size), n.map(|n| n * 32 / size));
            }
        }
        for mode in ["has", "union", "intersect", "difference", "isSubsetOf"] {
            let (Some(Some(a)), Some(Some(b))) = (cost.get(&(mode, small)), cost.get(&(mode, large))) else { continue };
            eprintln!("{backend}, {mode}: {a} then {b} instructions");
            if b * 10 > a * 12 + 20 {
                failures.push(format!("{backend}: `{mode}` costs {a} at {small} members and {b} at {large}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const REPORTS: &str = r#"from "core/indexset" import * as indexSet;
from "core/indexset" import { Index, IndexSet };
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Index, Equal, Show for Slot;
struct Slot(U32);

// No `Show`, so the report walks the set itself.
derive Index, Equal for Bare;
struct Bare(U64);

test "a set" {
    let ctx = context { Allocator: alloc() };
    assert.equal(indexSet.of(ctx, [Slot(70), Slot(3)]), indexSet.of(ctx, [Slot(3)]));
}

test "no Show" {
    let ctx = context { Allocator: alloc() };
    assert.equal(indexSet.of(ctx, [Bare(64)]), indexSet.empty<Bare>());
}

test "a list of sets" {
    let ctx = context { Allocator: alloc() };
    assert.equal([indexSet.of(ctx, [Slot(1)])], []);
}
"#;

/// **A failing assertion prints a set as its members in ascending order**, on
/// JavaScript and every native backend, through the set's `Show` where the
/// member has one and through the report's own walk where it does not.
#[test]
fn a_failing_assert_prints_a_set_as_its_members_on_every_backend() {
    let repo = crate::hand_written_impls::repository("indexset-reports", REPORTS);
    let expected = [
        "    actual:   [Slot(3), Slot(70)]\n    expected: [Slot(3)]\n",
        "    actual:   [Bare(64)]\n    expected: []\n",
        "    actual:   [[Slot(1)]]\n    expected: []\n",
    ];
    for (backend, status, stdout, stderr) in crate::hand_written_impls::every_backend(&repo) {
        let context = format!("{backend}:\n{stdout}\n{stderr}");
        assert_eq!(status, 1, "{context}");
        assert!(stdout.contains("0 passed, 3 failed"), "{context}");
        for line in expected {
            assert!(stdout.contains(line), "{backend}: missing\n{line}\nin\n{stdout}\n{stderr}");
        }
    }
}
