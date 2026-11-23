//! Text trimmed natively, on every native backend built in: what the program
//! prints, and what a trim costs, counted rather than timed.

use crate::collection_costs::steps_cost;

/// `k` steps, each trimming the same short text with spaces at both ends.
/// `main` takes `k` and prints the sum of the trimmed lengths.
const TRIMS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds(s: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    rounds(s, i + 1, n, acc + s.trim().length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let s = "  held ".concat(ctx, str.fromInt(ctx, 7)).concat(ctx, " \t");
  io.println(ctx, "${rounds(s, 0, k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`TRIM_BOUND`] is measured between, wide enough
/// that the difference runs to hundreds of millions of instructions.
const TRIM_STEPS: (u64, u64) = (100_000, 2_100_000);

/// What one step of [`TRIMS`] costs on the copy-and-patch backend, in
/// instructions: the trim, the view it answers, and its length. It measured
/// 349 when the runtime checked the whole text was UTF-8 and decoded both
/// ends, and 190 once it read the ASCII spaces as bytes. The bound sits
/// halfway, so the old code fails it by 160 million instructions, where two
/// runs differ by well under a million.
const TRIM_BOUND: f64 = 269.5;

/// **A trim of ASCII text reads only the spaces it removes.** The runtime
/// walks each end a byte at a time while it is ASCII, rather than decoding
/// the whole text first.
#[test]
fn a_trim_of_ascii_text_reads_only_its_ends() {
    let (fewer, more) = TRIM_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("trims", TRIMS);
        let n = steps_cost(backend, &binary, TRIM_STEPS, |k| format!("{}\n", k * 6));
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > TRIM_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {TRIM_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What one step of [`TRIMS`] costs on the copy-and-patch backend once each
/// byte is tested against a word of space bits and the ASCII path calls
/// nothing that makes it save registers. It measured 190 before and 135
/// after; the bound sits halfway, so the old code fails it by 55 million
/// instructions over [`TRIM_STEPS`], where two runs differ by well under a
/// million.
const SPACE_BITS_BOUND: f64 = 162.5;

/// **A trim tests each space byte with one compare and one bit test.** The
/// ASCII walk at each end reads a byte, checks it against a word of space
/// bits, and leaves decoding to an out-of-line path that ASCII text never
/// takes.
#[test]
fn a_trim_tests_each_space_byte_against_a_word_of_bits() {
    let (fewer, more) = TRIM_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("trims-bits", TRIMS);
        let n = steps_cost(backend, &binary, TRIM_STEPS, |k| format!("{}\n", k * 6));
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > SPACE_BITS_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {SPACE_BITS_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
