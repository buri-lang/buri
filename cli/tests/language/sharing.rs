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
//!    than a time, between two programs doing the same *total* number of
//!    pushes in runs of ten thousand and of a hundred thousand: linear work
//!    makes those equal, and the copying they replaced makes the second ten
//!    times the first.
//!
//! ```text
//! cargo test -p buri --test language sharing::
//! ```

use std::process::Command;
use std::time::Instant;

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

/// A list grown in a loop, with the size and the number of runs traded against
/// each other so that both programs push the same number of elements.
const GROW: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/list" import * as list;

struct State { total: Int, items: [Int] }

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

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${buildRuns(ctx, 0, RUNS, SIZE, 0)} ${foldRuns(ctx, 0, RUNS, SIZE, 0)}");
  .Ok(())
}
"#;

/// The fastest of three runs, because the slow ones are the machine's and the
/// fast one is the program's.
fn milliseconds(scratch: &Scratch, package: &str, expected: &str) -> u128 {
    let mut best = u128::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let run = scratch.exec_js(package);
        let took = start.elapsed().as_millis();
        assert_eq!(run.stdout.trim(), expected, "{}\n{}", run.what, run.all());
        best = best.min(took);
    }
    best
}

/// Both programs push two million elements. Linear growth makes them cost the
/// same; the copy they replaced makes the hundred-thousand one ten times the
/// ten-thousand one, because each of its pushes copies ten times as much.
///
/// The bound is four rather than two: the point is the *shape* of the curve,
/// and a bound tight enough to catch a constant-factor regression would be one
/// that fails on a loaded machine.
#[test]
fn growing_a_list_in_a_loop_is_linear() {
    let scratch = Scratch::repo("js-sharing-linearity");
    for (package, size, runs) in [("cmd/small", 10_000, 200), ("cmd/large", 100_000, 20)] {
        let source = GROW.replace("SIZE", &size.to_string()).replace("RUNS", &runs.to_string());
        scratch.write(&format!("{package}/BUILD.buri"), JS_BINARY);
        scratch.write(&format!("{package}/main.buri"), &source);
        scratch.run(&["build", &format!("//{package}"), "--force"]).ok();
    }
    let small = milliseconds(&scratch, "cmd/small", "2000000 2000000");
    let large = milliseconds(&scratch, "cmd/large", "2000000 2000000");
    assert!(
        large <= small.saturating_mul(4).max(200),
        "growing a list is not linear: two million pushes in runs of ten thousand \
         took {small} ms and the same two million in runs of a hundred thousand \
         took {large} ms"
    );
}
