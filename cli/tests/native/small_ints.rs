//! `str.fromInt` of numbers below 10000, natively, on every native backend
//! built in: what the program prints, and what a step costs, counted rather
//! than timed.

use crate::collection_costs::steps_cost;

/// `k` steps, step `i` rendering `i % 10000` and taking its length. `main`
/// takes `k` and prints the sum of the lengths.
const SMALL_INTS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    rounds(ctx, i + 1, n, acc + str.fromInt(ctx, i % 10000).length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(ctx, 0, k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`SMALL_INTS`] prints for `k` steps: the digits of `i % 10000`,
/// summed.
fn digits_summed(k: u64) -> String {
    let sum: u64 = (0..k).map(|i| (i % 10_000).to_string().len() as u64).sum();
    format!("{sum}\n")
}

/// The two counts of steps [`SMALL_INT_BOUND`] is measured between, wide
/// enough that the difference runs to hundreds of millions of instructions.
const SMALL_INT_STEPS: (u64, u64) = (100_000, 2_100_000);

/// What one step of [`SMALL_INTS`] costs on the copy-and-patch backend, in
/// instructions: the number's text, its length, and the step's arithmetic.
/// It measured 174 when each number's digits went into a fresh block that
/// was freed again, and 58 once a number below 10000 is a view of static
/// digits. The bound sits halfway, so the old code fails it by 116 million
/// instructions over [`SMALL_INT_STEPS`], where two runs differ by well under
/// a million.
const SMALL_INT_BOUND: f64 = 115.9;

/// **A number below 10000 renders without allocating.** Its text is a view
/// of static digits, as a literal is, so there is no block to allocate and
/// none to free.
#[test]
fn a_number_below_ten_thousand_renders_without_a_block() {
    let (fewer, more) = SMALL_INT_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("small-ints", SMALL_INTS);
        let n = steps_cost(backend, &binary, SMALL_INT_STEPS, digits_summed);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > SMALL_INT_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {SMALL_INT_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
