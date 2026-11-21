//! `core/bits` called in a loop: what it answers on every native backend
//! built in, under the heap check, and what a step costs, counted rather than
//! timed.

/// `k` steps, each folding seven `core/bits` operations over a word that is
/// both an argument and the answer of the step before. `main` takes `k` and
/// prints the sum of the counts.
const FOLDS: &str = r#"
from "core/bits" import * as bits;
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds(i: Int, n: Int, w: U64, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let c = i % 61;
    let mixed = bits.rotateLeftU64(w, c) ^ bits.shiftLeftU64(i.wrapToU64(), c);
    let next = bits.byteSwapU64(mixed) ^ bits.shiftRightU64(mixed, 7);
    let counts = bits.popCountU64(next) + bits.leadingZerosU64(next) + bits.trailingZerosU64(next);
    rounds(i + 1, n, next, acc + counts)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(0, k, 81985529216486895, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`FOLDS`] prints for `k` steps.
fn folds_answer(k: u64) -> String {
    let (mut w, mut acc) = (81_985_529_216_486_895u64, 0u64);
    for i in 0..k {
        let c = (i % 61) as u32;
        let mixed = w.rotate_left(c) ^ (i << c);
        w = mixed.swap_bytes() ^ (mixed >> 7);
        acc += u64::from(w.count_ones() + w.leading_zeros() + w.trailing_zeros());
    }
    format!("{acc}\n")
}

/// The two counts of steps [`FOLDS_BOUND`] is measured between.
const FOLDS_STEPS: (u64, u64) = (100_000, 400_000);

/// What one step of [`FOLDS`] costs on the copy-and-patch backend, in
/// instructions. It measured 164.0 when each operation was a call
/// to its `core/bits` function, and 87.8 with each one's stencil in
/// the caller. The bound sits between the two, so the old code fails it, with
/// room for the counter's noise of well under 1%.
const FOLDS_BOUND: f64 = 126.0;

/// **A `core/bits` operation is its stencil in the caller, not a call.** Each
/// is one or two machine instructions, so a call around it costs several
/// times the operation itself.
#[test]
fn a_bits_operation_runs_in_its_caller_without_a_call() {
    let (fewer, more) = FOLDS_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("bits-folds", FOLDS);
        let n = crate::collection_costs::steps_cost(backend, &binary, FOLDS_STEPS, folds_answer);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > FOLDS_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {FOLDS_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
