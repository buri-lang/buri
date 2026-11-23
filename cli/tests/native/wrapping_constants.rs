//! A wrapping operation on a constant, natively, on every native backend
//! built in: what the program prints, and what a step costs, counted rather
//! than timed.

use crate::collection_costs::steps_cost;

/// `k` steps, each taking one off a `U64` with `wrappingSubtract`, by a
/// constant `let` as `core/map` writes it, and adding one back with
/// `wrappingAdd` and a literal. `main` takes `k` and prints what is left of
/// a start that runs below zero and wraps.
const WRAPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Environment, Stdout };

let ONE: U64 = 1;

fn rounds(i: Int, n: Int, acc: U64): U64 {
  if (i >= n) {
    acc
  } else {
    rounds(i + 1, n, acc.wrappingSubtract(ONE).wrappingSubtract(ONE).wrappingAdd(1))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(0, k, 3)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`WRAP_BOUND`] is measured between, wide enough
/// that the difference runs to hundreds of millions of instructions.
const WRAP_STEPS: (u64, u64) = (1_000_000, 51_000_000);

/// What one step of [`WRAPS`] costs on the copy-and-patch backend, in
/// instructions: three wrapping operations and the loop around them. It
/// measured 26.0 when each operation read its constant from a frame slot it
/// was stored to first, and 17.0 once the constant was the stencil's
/// immediate. The bound sits halfway, so the old code fails it by 225 million
/// instructions, where two runs differ by well under a million.
const WRAP_BOUND: f64 = 21.5;

/// What `k` steps leave of 3: one less a step, wrapped at 2^64.
fn left(k: u64) -> String {
    format!("{}\n", 3u64.wrapping_sub(k))
}

/// **A wrapping operation reads a constant as an immediate.** The constant
/// needs no frame slot, and the operation is the stencil with the constant
/// in it, as the operator `-` would be.
#[test]
fn a_wrapping_operation_on_a_constant_reads_it_as_an_immediate() {
    let (fewer, more) = WRAP_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("wrapping-constants", WRAPS);
        let n = steps_cost(backend, &binary, WRAP_STEPS, left);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > WRAP_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {WRAP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
