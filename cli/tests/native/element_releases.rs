//! A list of records holding text, released natively, on every native
//! backend built in: what the program prints, and what releasing an element
//! costs, counted rather than timed.

use crate::collection_costs::steps_cost;

/// The elements each step's list holds.
const ELEMENTS: u64 = 1000;

/// `k` steps, each building a list of [`ELEMENTS`] copies of one record that
/// holds a counted `Str`, and dropping it. `main` takes `k` and prints the
/// sum of the lengths.
const RELEASES: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct P {
  x: Int,
  name: Str,
}

fn rounds<C: Allocator>(ctx: C, p: P, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let ps = list.repeat(ctx, p, 1000);
    rounds(ctx, p, i + 1, n, acc + ps.length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let p = P { x: k, name: str.fromInt(ctx, 1000000 + k).concat(ctx, "-held") };
  io.println(ctx, "${rounds(ctx, p, 0, k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`RELEASE_BOUND`] is measured between, wide
/// enough that the difference runs to billions of instructions.
const RELEASE_STEPS: (u64, u64) = (2_000, 42_000);

/// What one element of [`RELEASES`] costs on the copy-and-patch backend, in
/// instructions: its copy into the list, and its release. It measured 50.3
/// when the release glue copied each element into its frame and tested it
/// there, and 44.3 once it read only the `Str`'s block pointer, through the
/// element's address. The bound sits halfway, so the old code fails it by
/// 120 million instructions, where two runs differ by well under a million.
const RELEASE_BOUND: f64 = 47.3;

/// **A list's elements are released where they are.** A release reads only
/// the words it tests, through the element's address, rather than copying
/// the whole element into the glue's frame first.
#[test]
fn a_list_element_is_released_without_copying_it() {
    let (fewer, more) = RELEASE_STEPS;
    let elements = (more - fewer) * ELEMENTS;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("element-releases", RELEASES);
        let n = steps_cost(backend, &binary, RELEASE_STEPS, |k| {
            format!("{}\n", k * ELEMENTS)
        });
        let each = n.map(|n| n as f64 / elements as f64);
        eprintln!("{backend}: {each:?} instructions an element");
        if backend == "stencil" && each.is_some_and(|e| e > RELEASE_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions an element, over {RELEASE_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
