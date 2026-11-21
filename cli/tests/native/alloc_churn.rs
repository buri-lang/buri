//! Programs that allocate and free a block on every step, natively, on every
//! native backend built in: what a block costs, counted rather than timed, and
//! blocks that are built on one thread, freed on another and moved between
//! actors still reading right.
//!
//! Every program runs as it ships and once more under the heap check, which
//! takes the runtime's other path for every allocation and free.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, probed, ran_command};

/// Enough steps that the work, at least 110 M instructions, dwarfs the 15 M
/// that launching any process costs. What launching costs moves with the
/// machine's load: at 100,000 steps a 7% move in it read as 10 more
/// instructions a step.
const STEPS: u64 = 1_000_000;

/// One short-lived block a step, built three ways: a list from `list.range`, a
/// list literal, and a literal of [`LARGE`] `Int`s. `main` takes the mode and
/// the number of steps.
fn churn() -> String {
    CHURN.replace("LARGE", &vec!["i"; LARGE as usize].join(", "))
}

/// The elements of a list one more than the small-block cache keeps: 1,032
/// bytes against 1,024.
const LARGE: u64 = 129;

/// [`churn`]'s program, with `LARGE` for the large literal's elements.
const CHURN: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn ranges<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { ranges(ctx, i + 1, n, acc + list.range(ctx, i, i + 2).length()) }
}

fn literals<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { literals(ctx, i + 1, n, acc + [i, i + 1].length()) }
}

fn larges<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { larges(ctx, i + 1, n, acc + [LARGE].length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let answer = match (args.first()) {
    .Some("range") => ranges(ctx, 0, n, 0),
    .Some("literal") => literals(ctx, 0, n, 0),
    .Some("large") => larges(ctx, 0, n, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A step that allocates a small block and frees it reads the thread's cache
/// without a call.** On macOS both read it through dyld's thread-local lookup,
/// about 35 instructions a pair. A `list.range` step went from 245 to 210
/// instructions on stencil and from 198 to 164 on LLVM; a list literal step
/// from 233 to 208 and from 141 to 109.
///
/// **A block too large for that cache asks nothing twice.** Its allocation
/// misses the cache and its free can't go in, so both go past the cache's
/// paths, which once asked again every question those had just answered. It
/// is kept for large blocks, so no step reaches the platform allocator. A large
/// literal step went from 829 to 782 instructions on stencil and from 315 to
/// 261 on LLVM.
///
/// **The fast paths test one gate byte, and a free counts down two words.**
/// The heap check, scopes and indexed blocks each once had a flag of their
/// own that every allocation or free loaded, and a push added to the bytes
/// held and compared the sum with a limit. A `list.range` step went from 210
/// to 187 instructions on stencil and from 164 to 154 on LLVM; a list literal
/// step from 208 to 177 and from 109 to 99; a large literal step from 782 to
/// 757 and from 261 to 255.
///
/// **A cache hit counts itself with one add.** A pop or push once added to
/// four heap counts; it now adds one to its own size's count, and a reader
/// works the bytes out from the sizes. A `list.range` step went from 187 to
/// 180 instructions on stencil, a list literal step from 177 to 170 and a
/// large literal step from 756 to 747.
///
/// **A miss calls nothing on its way.** Past the cache, a large block came
/// from a second thread-local lookup behind a call, and every miss saved
/// registers for the tally's rare settling. The block now comes from the
/// cache's own thread-local, and the settling is out of line. A large literal
/// step went from 747 to 728 instructions on stencil.
///
/// Each bound sits about halfway.
#[test]
fn allocating_and_freeing_a_block_costs_a_few_dozen_instructions() {
    let steps = STEPS.to_string();
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("alloc-churn", &churn());
        for (mode, length, stencil, llvm) in [
            ("range", 2, 183, 159),
            ("literal", 2, 173, 104),
            ("large", LARGE, 737, 258),
        ] {
            let want = format!("{}\n", STEPS * length);
            let (n, blocks) = per_op(backend, &binary, mode, &[&steps], STEPS, &want);
            eprintln!("{backend}, {mode}: {n:?} instructions a step, {blocks} blocks");
            assert!(
                blocks >= STEPS,
                "{backend} {mode}: {STEPS} steps made only {blocks} blocks"
            );
            let bound = if backend == "llvm" { llvm } else { stencil };
            if let Some(n) = n.filter(|&n| n > bound) {
                failures.push(format!(
                    "{backend} {mode}: {n} instructions a step, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Eight parallel steps a round build lists of strings, each on whichever
/// thread runs it, and the caller sends each list to an actor. The actor keeps
/// the last sixteen strings and frees the rest, so a block is often freed on a
/// thread other than the one that built it. 500 rounds; the answer is the sum
/// of the lengths the actor kept after each message.
const MOVERS: &str = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn words<C: Allocator>(ctx: C, i: Int): [Str] {
  list.range(ctx, 0, 1 + i % 5).mapCtx(ctx, fn(c, k) => str.format(c, "w${i}.${k}"))
}

fn kept<C: Allocator>(ctx: C, s: [Str], m: [Str]): [Str] {
  let grown = m.foldCtx(ctx, fn(c, acc: [Str], w) => acc.push(c, w), s);
  if (grown.length() > 16) { grown.drop(ctx, grown.length() - 16) } else { grown }
}

fn rounds<C: Allocator + Tasks>(ctx: C, address: Address<C, [Str], [Str], Int>, r: Int, n: Int, acc: Int): Int {
  if (r >= n) {
    acc
  } else {
    let batches = tasks.parallel(ctx, list.range(ctx, 0, 8), fn(c, _i, x) => words(c, r * 8 + x));
    let answered = batches.foldCtx(ctx, fn(c, a, b) => a + address.sendMessage(c, b).withDefault(0), 0);
    rounds(ctx, address, r + 1, n, (acc + answered) % 1000003)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let address = actor.start(ctx, Actor {
    state: list.empty(),
    step: fn(c, s, m) => {
      let next = kept(c, s, m);
      Stepped { state: next, answer: next.sumOf(fn(w) => w.length()) }
    },
  });
  let total = rounds(ctx, address, 0, 500, 0);
  let _ = address.stop(ctx).isOk();
  io.println(ctx, "${total}").mapErr(fn(_e) => "stdout")
}
"#;

/// Blocks built on one thread, freed on another and moved between actors read
/// right, and every one is freed by the end, as the program ships and under the
/// heap check.
#[test]
fn blocks_moved_between_threads_and_actors_read_right_and_are_all_freed() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("alloc-movers", MOVERS);
        for checked in [false, true] {
            let mut cmd = std::process::Command::new(&binary);
            if checked {
                heap_checked(&mut cmd);
            }
            let r = ran_command(&mut cmd);
            assert_eq!(r.status, 0, "{backend} checked={checked}: {}", r.stderr);
            assert_eq!(
                r.stdout, "429925\n",
                "{backend} checked={checked}: {}",
                r.stderr
            );
            let (blocks, live) = probed(&r.stderr);
            assert!(blocks > 20_000, "{backend}: only {blocks} blocks");
            assert_eq!(
                live, 0,
                "{backend} checked={checked}: blocks still live at exit"
            );
        }
    }
}

/// Four parallel tasks each make a list and a string of a different size each
/// step, 1 to 1,112 bytes, so some sizes fit the small-block cache and some
/// don't. The answer is the sum of their lengths.
const SIZES: &str = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn sizes<C: Allocator>(ctx: C, k: Int, n: Int, acc: Int): Int {
  if (k >= n) {
    acc
  } else {
    let xs = list.range(ctx, 0, k % 140);
    sizes(ctx, k + 1, n, acc + xs.length() + str.fromInt(ctx, k * 7919).length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let counts = tasks.parallel(ctx, list.range(ctx, 0, 4), fn(c, _i, x) => sizes(c, x, 20000 + x, 0));
  io.println(ctx, "${counts.foldCtx(ctx, fn(_c, a, b) => a + b, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// [`crate::shared::ALLOC_PROBE`], plus a line with every heap count.
fn counts_probe() -> String {
    format!(
        "{}
__attribute__((destructor)) static void buri_counts_probe(void) {{
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, \"counts=%llu %llu %llu %llu\\n\",
          (unsigned long long)s.total_blocks, (unsigned long long)s.total_bytes,
          (unsigned long long)s.live_blocks, (unsigned long long)s.live_bytes);
}}
",
        crate::shared::ALLOC_PROBE
    )
}

/// **The heap counts are the same whether a block comes from the cache or
/// not.** A block the small-block cache hands out or takes back is counted
/// by its size's own pair of counts, and the rest in the thread's totals, so
/// the totals are read by adding the two. Under the heap check no block goes
/// near the cache, so both runs must count every block and byte alike, with
/// nothing live at exit.
#[test]
fn the_heap_counts_are_the_same_whether_a_block_comes_from_the_cache_or_not() {
    let probe = counts_probe();
    for (backend, build) in crate::e2e::probed_builds(&probe) {
        let binary = build("alloc-sizes", SIZES);
        let counts = |checked: bool| {
            let mut cmd = std::process::Command::new(&binary);
            if checked {
                heap_checked(&mut cmd);
            }
            let r = ran_command(&mut cmd);
            assert_eq!(r.status, 0, "{backend} checked={checked}: {}", r.stderr);
            assert_eq!(r.stdout, "6219814\n", "{backend} checked={checked}");
            r.stderr
                .lines()
                .find_map(|l| l.strip_prefix("counts="))
                .unwrap_or_else(|| panic!("the probe printed nothing: {}", r.stderr))
                .to_string()
        };
        let (shipped, checked) = (counts(false), counts(true));
        assert_eq!(
            shipped, checked,
            "{backend}: total blocks, bytes, live blocks, bytes"
        );
        assert!(
            shipped.ends_with(" 0 0"),
            "{backend}: {shipped} still live at exit"
        );
    }
}

/// Small blocks made and dropped, then long non-ASCII strings of one length
/// sliced, each dropped before the next is made, then a scope opened, with
/// more churn after each. The slices build scalar indexes, and those and the
/// scope each turn on something every later free or allocation has to ask
/// about. The strings take turns between two layouts with their `é`s in
/// different places. `main` prints the churn sums and each string's 201st
/// scalar.
const LATCHES: &str = r#"
from "core/alloc" import * as alloc;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn churn<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let xs = list.range(ctx, 0, i % 9);
    churn(ctx, i + 1, n, acc + xs.length() + str.fromInt(ctx, i).length())
  }
}

fn picks<C: Allocator>(ctx: C, k: Int, n: Int, acc: Str): Str {
  if (k >= n) {
    acc
  } else {
    let s = if (k % 2 == 0) {
      str.format(ctx, "${"é".repeat(ctx, 150)}${"b".repeat(ctx, 150)}")
    } else {
      str.format(ctx, "${"a".repeat(ctx, 300)}${"é".repeat(ctx, 75)}")
    };
    picks(ctx, k + 1, n, acc.concat(ctx, s.slice(200, 201)))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let before = churn(ctx, 0, 5000, 0);
  let picked = picks(ctx, 0, 300, "");
  let indexed = churn(ctx, 0, 5000, 0);
  let scoped = alloc.scoped(ctx, fn(s) => churn(s, 0, 5000, 0) + "z".repeat(s, 50).length());
  let after = churn(ctx, 0, 5000, 0);
  io.println(ctx, "${before} ${picked} ${indexed} ${scoped} ${after}").mapErr(fn(_e) => "stdout")
}
"#;

/// **Turning on a scalar index or a scope partway through sends every later
/// block the right way.** On Linux the cache's fast paths find the thread's
/// heap and ask the gate in one load, so each change to the gate has to reach
/// that word too. A free that missed it would keep a string's index for the
/// next string given the same block, and slice that one wrong. An allocation
/// that missed it would hand a scope a cached block. The counts must match
/// the heap check's, which never uses the cache, with nothing live at exit.
#[test]
fn blocks_go_the_right_way_after_an_index_or_a_scope_turns_on_partway() {
    let probe = counts_probe();
    let picked = "ba".repeat(150);
    let want = format!("38880 {picked} 38880 38930 38880\n");
    for (backend, build) in crate::e2e::probed_builds(&probe) {
        let binary = build("alloc-latches", LATCHES);
        let counts = |checked: bool| {
            let mut cmd = std::process::Command::new(&binary);
            if checked {
                heap_checked(&mut cmd);
            }
            let r = ran_command(&mut cmd);
            assert_eq!(r.status, 0, "{backend} checked={checked}: {}", r.stderr);
            assert_eq!(r.stdout, want, "{backend} checked={checked}");
            r.stderr
                .lines()
                .find_map(|l| l.strip_prefix("counts="))
                .unwrap_or_else(|| panic!("the probe printed nothing: {}", r.stderr))
                .to_string()
        };
        let (shipped, checked) = (counts(false), counts(true));
        assert_eq!(
            shipped, checked,
            "{backend}: total blocks, bytes, live blocks, bytes"
        );
        assert!(
            shipped.ends_with(" 0 0"),
            "{backend}: {shipped} still live at exit"
        );
    }
}

/// Rounds of 400 lists of `Int`s, 0 to 140 elements long, so each round's
/// lists fill cache slots up to 1,120 bytes: a size of a page's class, one
/// past the classes but in the cache, or past both. Each round's sizes differ
/// from the last's, so the cache's sweeps give back the slots that went idle.
/// `fanned` builds four batches a round on other threads and sums them here,
/// so a swept block is often one another thread's page owns. The answer is
/// both sums.
const SWEPT: &str = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn batch<C: Allocator>(ctx: C, r: Int): [[Int]] {
  list.range(ctx, 0, 400).mapCtx(ctx, fn(c, i) => list.range(c, i, i + (i * 37 + r * 11) % 141))
}

fn total(xss: [[Int]]): Int {
  xss.fold(fn(a, xs) => a + xs.fold(fn(b, x) => b + x, 0), 0)
}

fn rounds<C: Allocator>(ctx: C, r: Int, n: Int, acc: Int): Int {
  if (r >= n) { acc } else { rounds(ctx, r + 1, n, acc + total(batch(ctx, r))) }
}

fn fanned<C: Allocator + Tasks>(ctx: C, r: Int, n: Int, acc: Int): Int {
  if (r >= n) {
    acc
  } else {
    let made = tasks.parallel(ctx, list.range(ctx, 0, 4), fn(c, _i, x) => batch(c, r * 4 + x));
    fanned(ctx, r + 1, n, acc + made.fold(fn(a, xss) => a + total(xss), 0))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  io.println(ctx, "${rounds(ctx, 0, 200, 0)} ${fanned(ctx, 0, 50, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// [`SWEPT`]'s answer: the sum of every list's elements, a batch `r` at a time.
fn swept_answer() -> String {
    let batch = |r: u64| -> u64 {
        (0..400u64)
            .map(|i| (i..i + (i * 37 + r * 11) % 141).sum::<u64>())
            .sum()
    };
    let rounds: u64 = (0..200).map(batch).sum();
    let fanned: u64 = (0..200).map(batch).sum();
    format!("{rounds} {fanned}\n")
}

/// **Blocks the cache's sweep gives back are used again intact, at every size
/// the cache keeps.** A sweep hands a slot's whole list to the allocator at
/// once, which finds the size's class and the thread's heap once for the lot.
/// Lists of 127 and 128 elements are cached but past the allocator's classes,
/// so those go to the system, and blocks from other threads' pages go back to
/// those pages. A block given back to the wrong place would turn up in two
/// lists and change a sum. The counts must match the heap check's, which
/// never uses the cache, with nothing live at exit.
#[test]
fn blocks_a_sweep_gives_back_are_used_again_intact() {
    let probe = counts_probe();
    let want = swept_answer();
    for (backend, build) in crate::e2e::probed_builds(&probe) {
        let binary = build("alloc-swept", SWEPT);
        let counts = |checked: bool| {
            let mut cmd = std::process::Command::new(&binary);
            if checked {
                heap_checked(&mut cmd);
            }
            let r = ran_command(&mut cmd);
            assert_eq!(r.status, 0, "{backend} checked={checked}: {}", r.stderr);
            assert_eq!(r.stdout, want, "{backend} checked={checked}");
            r.stderr
                .lines()
                .find_map(|l| l.strip_prefix("counts="))
                .unwrap_or_else(|| panic!("the probe printed nothing: {}", r.stderr))
                .to_string()
        };
        let (shipped, checked) = (counts(false), counts(true));
        assert_eq!(
            shipped, checked,
            "{backend}: total blocks, bytes, live blocks, bytes"
        );
        assert!(
            shipped.ends_with(" 0 0"),
            "{backend}: {shipped} still live at exit"
        );
    }
}
