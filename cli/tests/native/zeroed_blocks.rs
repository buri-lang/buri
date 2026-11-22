//! Lists built in a zeroed block, natively, on every native backend built in:
//! what `filter` keeps at every length, in blocks that held other text
//! before, and what mapping a short list costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// For each length from 1 to 10, a list of that many texts, mapped and then
/// filtered to the even places, twice over, so the second time round each
/// block comes back from the allocator holding the first time's texts. Prints
/// what each filter kept.
const KEPT: &str = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn kept<C: Allocator>(ctx: C, n: Int): Str {
  let words = list.range(ctx, 0, n).mapCtx(ctx, fn(c, i) => str.format(c, "word-${i}"));
  let loud = words.mapCtx(ctx, fn(c, w) => w.toUpper(c));
  let even = loud.filter(ctx, fn(w) => w.endsWith("0") || w.endsWith("2") || w.endsWith("4") || w.endsWith("6") || w.endsWith("8"));
  str.format(ctx, "${n}: ${even.join(ctx, ",")}\n")
}

fn rounds<C: Allocator>(ctx: C, n: Int, acc: Str): Str {
  if (n > 10) { acc } else { rounds(ctx, n + 1, acc.concat(ctx, kept(ctx, n))) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let once = rounds(ctx, 1, "");
  let again = rounds(ctx, 1, "");
  io.print(ctx, once.concat(ctx, again)).mapErr(fn(_e) => "stdout")
}
"#;

/// What [`KEPT`] prints, worked out by hand.
fn kept() -> String {
    let line = |n: usize| {
        let even: Vec<String> = (0..n)
            .filter(|i| i % 2 == 0)
            .map(|i| format!("WORD-{i}"))
            .collect();
        format!("{n}: {}\n", even.join(","))
    };
    let once: String = (1..=10).map(line).collect();
    once.repeat(2)
}

/// **A filter keeps what it should at every length, in blocks that held
/// other text.** A list's block comes zeroed, because `filter` leaves the
/// places it rejects unwritten and the release walks the whole block. Lengths
/// 1 to 10 of 16-byte texts cover blocks of up to 64 bytes and past it. Under
/// the heap check, so a place left holding old text would be released twice.
#[test]
fn a_filter_keeps_what_it_should_in_a_block_that_held_other_text() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("zeroed-kept", KEPT);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, kept(), "{backend}");
    }
}

/// `main` takes a count `k` and maps a list of four `Int`s `k` times, and
/// prints the sum of the lengths.
const MAPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, xs: [Int], k: Int, acc: Int): Int {
  if (k == 0) { acc } else { rounds(ctx, xs, k - 1, acc + xs.map(ctx, fn(x) => x + k).length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(ctx, [1, 2, 3, 4], k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of maps [`MAPS`] makes.
const MAP_COUNTS: (u64, u64) = (100_000, 400_000);

/// **A short list's zeroed block is cleared without a call.** Each map of
/// four `Int`s allocated zeroed blocks, the list's 32 bytes among them, which
/// the allocator cleared with a call to `memset`. Then a block of up to 64
/// bytes was cleared with a few wide stores. Now the list's block, whose
/// every element the map stores, isn't cleared at all; the closure's still
/// is.
///
/// The bound is on the instructions one map costs: the allocations, their
/// clearing, four elements, the frees and a step of the loop. On macOS
/// stencil, 456 to 419, and 405 once the list's block wasn't cleared. The bound sits about halfway, so it fails if the
/// clearing goes back to a call, and leaves the rest about 4% to grow.
#[test]
fn a_short_lists_zeroed_block_is_cleared_without_a_call() {
    let (fewer, more) = MAP_COUNTS;
    let maps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("zeroed-maps", MAPS);
        let n = steps_cost(backend, &binary, MAP_COUNTS, |k| format!("{}\n", k * 4));
        let per = n.map(|n| n as f64 / maps as f64);
        eprintln!("{backend}: {per:?} instructions a map");
        if backend == "stencil" && per.is_some_and(|per| per > MAP_BOUND) {
            failures.push(format!(
                "{backend}: {per:?} instructions a map, over {MAP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// See [`a_short_lists_zeroed_block_is_cleared_without_a_call`].
const MAP_BOUND: f64 = 438.0;
