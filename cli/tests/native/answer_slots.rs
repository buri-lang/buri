//! A `Str` or a list a runtime function answers, natively, on every native
//! backend built in: what the program prints, and what a step costs, counted
//! rather than timed.

use crate::collection_costs::steps_cost;

/// `k` steps, each asking a runtime function for a `Str` and one for a list,
/// both answered through the caller's slot. `main` takes `k` and prints the
/// sum of the lengths.
const ANSWERS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let text = str.fromInt(ctx, i % 10);
    let none = list.range(ctx, i, i);
    rounds(ctx, i + 1, n, acc + text.length() + none.length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(ctx, 0, k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`ANSWERS_BOUND`] is measured between, wide
/// enough that the difference runs to billions of instructions.
const ANSWERS_STEPS: (u64, u64) = (1_000_000, 11_000_000);

/// What one step of [`ANSWERS`] costs on the copy-and-patch backend, in
/// instructions. It measured 198.1 when each answer's slot was zeroed before
/// the call, three words for the `Str` and two for the list, and 193.1
/// without. The bound sits halfway, so the old code fails it by 25 million
/// instructions, many times the counter's noise between two runs.
const ANSWERS_BOUND: f64 = 195.6;

/// **A runtime function's `Str` or list answer goes straight into its slot.**
/// The function writes every word of it, so zeroing the slot before the call
/// is waste.
#[test]
fn a_text_or_list_answer_is_written_without_clearing_its_slot() {
    let (fewer, more) = ANSWERS_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("answer-slots", ANSWERS);
        let n = steps_cost(backend, &binary, ANSWERS_STEPS, |k| format!("{k}\n"));
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > ANSWERS_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {ANSWERS_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
