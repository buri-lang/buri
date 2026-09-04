//! **The sticky shared bit**: the two claims that make writing into a list
//! legal, and the one that makes it worth doing.
//!
//! `design/native/MEMORY.md` §5.5 is the design. What is asserted here is what
//! the design has to be true for:
//!
//!  * **Provenance.** A JavaScript array this backend did not allocate carries
//!    no bit, and absence reads as *shared*, so it is copied and never written
//!    to — and no mark is ever written onto it either. The `core/list` surface
//!    is asked this directly, because the host boundary is a property of the
//!    runtime rather than of any program.
//!  * **Aliasing.** Every case where two names reach one list is in the
//!    conformance corpus (`conformance/lib/data/test/lists.buri`, "aliasing"),
//!    where both backends run it and the answers have to match.
//!  * **Cost.** Growing a list in a loop is linear. Asserted as a ratio rather
//!    than a time, between the same *total* number of pushes taken in runs of
//!    ten thousand and in runs of a hundred thousand: linear work makes those
//!    equal, and the copying they replaced makes the second ten times the
//!    first. One program times both, back to back, reading the clock around
//!    its own pushing — so what is compared is the work rather than a
//!    process's wall time, and the two halves of a ratio are always answers
//!    about the same machine a moment apart. Two dozen such pairs, and the
//!    median is the answer: what load does to a pair, it does to both halves,
//!    and a ratio divides it out.
//!
//! ```text
//! cargo test -p buri --test language sharing::
//! ```

use std::process::Command;

use crate::harness::{JS_BINARY, Scratch, js_runtime};

/// The runtime, plus a script that asks it the questions a Buri program
/// cannot: what a host array is, and what a mark is written on.
///
/// Every claim is a `throw`, so the assertion is the exit code and the message
/// is the thrown one.
const HOST_ARRAY_PROBE: &str = r#"
function check(what, ok) {
  if (!ok) throw new Error(what);
}

// An array from the host: no `$u`, so `$list_push` must copy it.
const host = [1, 2, 3];
const grown = $list_push(host, null, 4);
check("a host array was written through", host.length === 3);
check("a host array's copy is wrong", grown.length === 4 && grown[3] === 4);
check("a host array was marked", !Object.prototype.hasOwnProperty.call(host, "$u"));

// The copy is ours, so the next push writes into it.
const again = $list_push(grown, null, 5);
check("the copy of a host array was copied again", again === grown);

// Marking a host array must write nothing at all onto it.
const before = Object.getOwnPropertyNames(host).join(",");
$share(host);
check("$share wrote onto a host array", Object.getOwnPropertyNames(host).join(",") === before);
check("a marked host array became writable", $list_push(host, null, 9) !== host);
check("a marked host array was written through", host.length === 3);

// Two names for one of ours: the mark is what separates them.
const ours = $list_empty();
const marked = $share(ours);
check("$share answers its argument", marked === ours);
const left = $list_push(ours, null, 1);
const right = $list_push(ours, null, 2);
check("a marked list was written through", ours.length === 0);
check("two pushes onto a marked list agree", left[0] === 1 && right[0] === 2);

// Sticky: nothing puts a list back.
$list_push(left, null, 9);
check("a copy that was marked came back unmarked", left.$u === true);
$share(left);
$share(left);
check("a mark was undone", left.$u === false);

// The other five all copy a host array and write into one of ours.
for (const [name, call] of [
  ["concat", (xs) => $list_concat(xs, null, [9])],
  ["reverse", (xs) => $list_reverse(xs, null)],
  ["take", (xs) => $list_take(xs, null, 2)],
  ["drop", (xs) => $list_drop(xs, null, 1)],
  ["slice", (xs) => $list_slice(xs, null, 0, 2)],
]) {
  const from_host = [1, 2, 3];
  const out = call(from_host);
  check(name + " wrote through a host array", from_host.length === 3);
  check(name + " wrote through a host array", from_host[0] === 1 && from_host[2] === 3);
  check(name + " did not take ownership of its answer", out.$u === true);
}

console.log("ok");
"#;

/// A host array is copied, never written to, and never marked.
#[test]
fn a_host_array_is_never_written_through() {
    let scratch = Scratch::empty("js-sharing-host-array");
    let probe = scratch.write(
        "probe.mjs",
        &format!("{}\n{HOST_ARRAY_PROBE}", buri::compiler::backend::js::runtime_source()),
    );
    let out = Command::new(js_runtime()).arg(&probe).output().expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.trim() == "ok",
        "the runtime's list surface does not hold the host boundary:\n{stdout}{stderr}"
    );
}

/// How many pairs of measurements one launch takes, and how many launches
/// there are.
///
/// Twelve pairs a launch and two launches is twenty-four ratios, and the answer
/// is the median of them. Two launches rather than one because a JIT that
/// settles somewhere unlucky settles there for a whole process, and that is the
/// one source of error a second sample inside the same process cannot reach.
const PAIRS: usize = 12;
const LAUNCHES: usize = 2;

/// A list grown in a loop, timing **both** sizes back to back and printing one
/// `<small ms> <large ms> <small pushes> <large pushes>` line per pair.
///
/// **The clock is inside the program, and the two sizes are measured next to
/// each other.** Those are one decision. What this file asserts is a ratio, and
/// a ratio is only about the curve if its two halves are answers about the same
/// machine; every way this test has gone flaky has been a way of getting two
/// answers about two machines. Process wall time is mostly starting a
/// JavaScript runtime and warming a JIT — tens of milliseconds that are on
/// neither curve and that a loaded box is worst at — so the program reads the
/// clock around its own pushing instead. And the two sizes are timed thirty
/// milliseconds apart in one process rather than in two processes seconds
/// apart, so a burst of load is something a pair lives through together.
///
/// The order inside a pair alternates, so that nothing periodic in the
/// machine can settle into always landing on the same half.
const GROW: &str = r#"
from "core/effect" import { Alloc, Clock, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/time" import * as time;

struct State { total: Int, items: [Int] }

struct Timing { millis: Int, pushed: Int }

fn build<C: Alloc>(ctx: C, i: Int, n: Int, acc: [Int]): [Int] {
  if (i >= n) { acc } else { build(ctx, i + 1, n, acc.push(ctx, i)) }
}

fn fold<C: Alloc>(ctx: C, i: Int, n: Int, s: State): State {
  if (i >= n) {
    s
  } else {
    fold(ctx, i + 1, n, State { ..s, total: s.total + i, items: s.items.push(ctx, i) })
  }
}

fn buildRuns<C: Alloc>(ctx: C, k: Int, runs: Int, n: Int, acc: Int): Int {
  if (k >= runs) {
    acc
  } else {
    buildRuns(ctx, k + 1, runs, n, acc + build(ctx, 0, n, list.empty<Int>()).len())
  }
}

fn foldRuns<C: Alloc>(ctx: C, k: Int, runs: Int, n: Int, acc: Int): Int {
  if (k >= runs) {
    acc
  } else {
    foldRuns(
      ctx,
      k + 1,
      runs,
      n,
      acc + fold(ctx, 0, n, State { total: 0, items: list.empty<Int>() }).items.len(),
    )
  }
}

/// One size, timed: `runs` runs of `n` pushes through each of the two shapes.
fn timed<C: Alloc + Clock>(ctx: C, runs: Int, n: Int): Timing {
  let started = time.now(ctx);
  let pushed = buildRuns(ctx, 0, runs, n, 0) + foldRuns(ctx, 0, runs, n, 0);
  let took = time.since(ctx, started);
  Timing { millis: took.millis(), pushed: pushed }
}

fn say<C: Alloc + Stdout>(ctx: C, small: Timing, large: Timing): () {
  io.println(
    ctx,
    "${small.millis} ${large.millis} ${small.pushed} ${large.pushed}",
  ).ignore()
}

fn pairs<C: Alloc + Clock + Stdout>(ctx: C, k: Int, count: Int): () {
  if (k >= count) {
    ()
  } else {
    let _ = if (k % 2 == 0) {
      let small = timed(ctx, SMALL_RUNS, SMALL_SIZE);
      let large = timed(ctx, LARGE_RUNS, LARGE_SIZE);
      say(ctx, small, large)
    } else {
      let large = timed(ctx, LARGE_RUNS, LARGE_SIZE);
      let small = timed(ctx, SMALL_RUNS, SMALL_SIZE);
      say(ctx, small, large)
    };
    pairs(ctx, k + 1, count)
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Clock: host.clock, Stdout: host.stdout };
  let _ = pairs(ctx, 0, PAIRS);
  .Ok(())
}
"#;

/// One pair: the two sizes as the program timed them, in milliseconds.
struct Pair {
    small: u64,
    large: u64,
}

impl Pair {
    fn ratio(&self) -> f64 {
        self.large as f64 / self.small as f64
    }
}

/// Every pair the program reported, over every launch.
///
/// The launches are what is left of the old best-of-three: three runs of one
/// program and then three of the other were two samples taken at two different
/// moments, and a burst of load landing inside one of them moved the ratio for
/// a reason that had nothing to do with the curve. That is how this row went
/// flaky twice — the second time 115 ms against 784 ms, a ratio of 6.8 on a
/// bound of 4, from JavaScript the run before had passed on. Nothing about a
/// sample *count* fixes it; only measuring the two sizes together does, which
/// is what the program now does. The launches remain because a process is also
/// a JIT, and two of them are two opinions about the same code.
fn pairs(scratch: &Scratch, package: &str, pushes: &str) -> Vec<Pair> {
    let artifact = scratch.artifact(package);
    let what = format!("{} {}", js_runtime(), artifact.display());
    let mut all = Vec::new();
    for _ in 0..LAUNCHES {
        let out = Command::new(js_runtime())
            .arg(&artifact)
            .output()
            .unwrap_or_else(|e| panic!("`{what}` did not run: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let both = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "`{what}` failed:\n{both}");
        let mut seen = 0;
        for line in stdout.lines() {
            let field: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(
                field.len(),
                4,
                "`{what}` printed {line:?}, not four numbers:\n{both}"
            );
            let number = |at: usize| -> u64 {
                field[at].parse().unwrap_or_else(|_| {
                    panic!("`{what}` printed {:?} where a number belongs:\n{both}", field[at])
                })
            };
            for at in [2, 3] {
                assert_eq!(
                    field[at], pushes,
                    "a repetition pushed the wrong number of elements:\n{both}"
                );
            }
            let (small, large) = (number(0), number(1));
            assert!(
                small > 0 && large > 0,
                "`{what}` timed a repetition at zero milliseconds, which the clock \
                 cannot resolve; raise the runs per repetition:\n{both}"
            );
            all.push(Pair { small, large });
            seen += 1;
        }
        assert_eq!(seen, PAIRS, "`{what}` reported {seen} pairs rather than {PAIRS}:\n{both}");
    }
    all
}

/// The median of what the pairs say, which is the number the bound is on.
///
/// A median and not a minimum, because pairing has already done what a minimum
/// used to be for. A minimum throws away every sample a burst of load touched,
/// and that was the only defence available while the two halves of the ratio
/// were measured seconds apart; it also has a bias nobody wants — the shorter
/// of two tasks is likelier to find an uninterrupted slice on a contended box,
/// so a minimum quietly flatters the small one. Inside a pair, load lands on
/// both halves and divides out, so what is wanted is the *typical* pair rather
/// than the luckiest one, and half the pairs have to be wrong in the same
/// direction to move a median.
fn median_ratio(pairs: &[Pair]) -> f64 {
    let mut ratios: Vec<f64> = pairs.iter().map(Pair::ratio).collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).expect("a ratio is a number"));
    ratios[ratios.len() / 2]
}

/// Both sizes push six hundred thousand elements per timed repetition. Linear
/// growth makes them cost the same; the copy they replaced makes the
/// hundred-thousand one ten times the ten-thousand one, because each of its
/// pushes copies ten times as much.
///
/// The bound is four rather than two: the point is the *shape* of the curve,
/// and a bound tight enough to catch a constant-factor regression would be one
/// that fails on a loaded machine. Four is not a guess at where the noise is,
/// though — it sits in a gap that has been measured from both sides. Linear
/// growth scores **1.2** — on an idle box, and on one carrying two whole
/// parallel copies of this suite at a load average of three hundred on ten
/// cores, thirty runs of which put the median between 0.92 and 1.41. That the
/// two are the same number is the whole point of pairing. Copying scores
/// **10.3**: this test, run unchanged against a `$list_push` with its in-place
/// branch deleted and at a tenth of these sizes so that a quadratic program
/// finishes at all, put every one of its twenty-four pairs between 9.2 and
/// 10.6. There is a factor of eight between the two answers and the bound is
/// in the middle of it.
#[test]
fn growing_a_list_in_a_loop_is_linear() {
    let scratch = Scratch::repo("js-sharing-linearity");
    let source = GROW
        .replace("PAIRS", &PAIRS.to_string())
        .replace("SMALL_RUNS", "30")
        .replace("SMALL_SIZE", "10_000")
        .replace("LARGE_RUNS", "3")
        .replace("LARGE_SIZE", "100_000");
    scratch.write("cmd/grow/BUILD.buri", JS_BINARY);
    scratch.write("cmd/grow/main.buri", &source);
    scratch.run(&["build", "//cmd/grow", "--force"]).ok();

    let measured = pairs(&scratch, "cmd/grow", "600000");
    let ratio = median_ratio(&measured);

    // A repetition too short to resolve is a measurement, not a claim: the
    // clock reports whole milliseconds, and at five of them a rounding is
    // already a fifth of the reading. A host fast enough to trip this wants
    // more runs per repetition, not a looser bound.
    let mut short: Vec<u64> = measured.iter().map(|p| p.small).collect();
    short.sort_unstable();
    let typical = short[short.len() / 2];
    assert!(
        typical >= 5,
        "the typical repetition took {typical} ms, which a whole-millisecond clock \
         cannot resolve; raise the runs per repetition"
    );
    assert!(
        ratio <= 4.0,
        "growing a list is not linear: over {} pairs of six hundred thousand pushes, \
         the median run in runs of a hundred thousand cost {ratio:.1} times the same \
         work in runs of ten thousand, where linear growth scores 1.2 and copying \
         scores about 10. The pairs, as `<ten-thousand ms> <hundred-thousand ms>`: {}",
        measured.len(),
        measured
            .iter()
            .map(|p| format!("{}/{}", p.small, p.large))
            .collect::<Vec<_>>()
            .join(" "),
    );
}
