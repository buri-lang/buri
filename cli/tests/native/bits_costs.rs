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

/// `k` steps, each shifting a word by the literal counts 5 and 3 and by a
/// count read at run time. `main` takes `k` and prints the last word.
const SHIFTS: &str = r#"
from "core/bits" import * as bits;
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds(i: Int, n: Int, w: U64): U64 {
  if (i >= n) {
    w
  } else {
    let mixed = bits.shiftRightU64(w, 5) ^ bits.shiftLeftU64(w, 3) ^ i.wrapToU64();
    rounds(i + 1, n, mixed ^ bits.shiftRightU64(mixed, i % 61))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(0, k, 81985529216486895)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`SHIFTS`] prints for `k` steps.
fn shifts_answer(k: u64) -> String {
    let mut w = 81_985_529_216_486_895u64;
    for i in 0..k {
        let mixed = (w >> 5) ^ (w << 3) ^ i;
        w = mixed ^ (mixed >> (i % 61));
    }
    format!("{w}\n")
}

/// The two counts of steps [`SHIFTS_BOUND`] is measured between, wide
/// enough that the difference runs to hundreds of millions of instructions.
const SHIFTS_STEPS: (u64, u64) = (1_000_000, 11_000_000);

/// What one step of [`SHIFTS`] costs on the copy-and-patch backend, in
/// instructions. It measured 69.0 when every count was compared with the
/// width into a boolean and that boolean branched on, and 48.0 with the
/// literal counts unchecked and the other one a fused compare and branch. The
/// bound sits halfway, so the old code fails it by 110 million instructions,
/// far past the counter's noise of well under 1%.
const SHIFTS_BOUND: f64 = 58.0;

/// **A shift by a literal count in range has no range check, and a shift by
/// a count read at run time checks it with one compare and branch.** A
/// literal count is known in range when the program is built, so checking
/// it on every step is waste.
#[test]
fn a_shift_by_a_literal_count_skips_the_range_check() {
    let (fewer, more) = SHIFTS_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("bits-literal-shifts", SHIFTS);
        let n = crate::collection_costs::steps_cost(backend, &binary, SHIFTS_STEPS, shifts_answer);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > SHIFTS_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {SHIFTS_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `k` steps, each turning the low five bits of a word into a one-bit mask as
/// a map's branch does: `1 << (w & 31)`, with the `1` and the `31` named
/// constants. `main` takes `k` and prints the masks' wrapping sum.
const MASKS: &str = r#"
from "core/bits" import * as bits;
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

let ONE: U64 = 1;
let LOW: U64 = 31;

fn rounds(i: Int, n: Int, w: U64, acc: U64): U64 {
  if (i >= n) {
    acc
  } else {
    let bit = bits.shiftLeftU64(ONE, (w & LOW).wrapToI64());
    rounds(i + 1, n, bits.rotateRightU64(w, 5) ^ i.wrapToU64(), acc.wrappingAdd(bit))
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(0, k, 81985529216486895, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`MASKS`] prints for `k` steps.
fn masks_answer(k: u64) -> String {
    let (mut w, mut acc) = (81_985_529_216_486_895u64, 0u64);
    for i in 0..k {
        acc = acc.wrapping_add(1 << (w & 31));
        w = w.rotate_right(5) ^ i;
    }
    format!("{acc}\n")
}

/// The two counts of steps [`MASKS_BOUND`] is measured between.
const MASKS_STEPS: (u64, u64) = (1_000_000, 11_000_000);

/// What one step of [`MASKS`] costs on the copy-and-patch backend, in
/// instructions. It measured 41.0 when the masked count was checked against
/// the width and the `1` was stored to the frame and read back, 38.0 with the
/// check gone and 36.0 with the `1` the stencil's immediate too. The bound is
/// a whole instruction a step from each, ten million instructions, so the old
/// code and either change alone fail it, and the counter's noise (a few
/// thousandths of an instruction a step) is far inside it.
const MASKS_BOUND: f64 = 37.0;

/// **A shift by a count the build knows is in range has no range check, and
/// a literal shifted left is the shift's immediate.** `w & 31` is under 32
/// whatever `w` is, and still is once it is an `Int`, so checking it on
/// every step is waste; and the `1` need not be stored to be shifted.
#[test]
fn a_masked_shift_count_skips_the_range_check() {
    let (fewer, more) = MASKS_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("bits-masked-shifts", MASKS);
        let n = crate::collection_costs::steps_cost(backend, &binary, MASKS_STEPS, masks_answer);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > MASKS_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {MASKS_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
