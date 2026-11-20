//! An `Int` and a `Float` rendered as text, natively, on every native backend
//! built in: the text at each length and sign, and what one rendering costs,
//! counted rather than timed.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, ran_command};

/// `main` takes the mode and a count. `digits` and `floats` print each value
/// through `str.fromInt` or `str.fromFloat` and through interpolation;
/// `render` and `render-float` render `n` numbers and print the length of all
/// of them.
const RENDERED: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn powers<C: Allocator>(ctx: C, p: Int, k: Int, acc: [Int]): [Int] {
  let more = acc.concat(ctx, [p - 1, p, 0 - p, 1 - p]);
  if (k == 1) { more } else { powers(ctx, p * 10, k - 1, more) }
}

fn line<C: Allocator>(ctx: C, xs: [Int], i: Int, acc: Str): Str {
  match (xs[i]) {
    .None => acc,
    .Some(x) => line(ctx, xs, i + 1, acc.concat(ctx, str.fromInt(ctx, x)).concat(ctx, str.format(ctx, "=${x} "))),
  }
}

fn floatLine<C: Allocator>(ctx: C, xs: [Float], i: Int, acc: Str): Str {
  match (xs[i]) {
    .None => acc,
    .Some(x) => floatLine(ctx, xs, i + 1, acc.concat(ctx, str.fromFloat(ctx, x)).concat(ctx, str.format(ctx, "=${x} "))),
  }
}

fn rendered<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { rendered(ctx, i + 1, n, acc + str.fromInt(ctx, 1000 + i % 9000).length()) }
}

fn renderedFloats<C: Allocator>(ctx: C, x: Float, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { renderedFloats(ctx, x, i + 1, n, acc + str.fromFloat(ctx, x).length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let answer = match (args.first()) {
    .Some("digits") => {
      let xs = powers(ctx, 10, 18, [0, 1, -1, 9223372036854775807, -9223372036854775807 - 1]);
      "${line(ctx, xs, 0, "")}"
    },
    .Some("floats") => {
      let xs = [0.1, 1.5, 2.0, -1.0 * 0.0, -2.5, 123456.789, 0.000001, 0.0000001, 0.1 + 0.2,
        100000000000000000000.0, 1000000000000000000000.0];
      "${floatLine(ctx, xs, 0, "")}"
    },
    .Some("render") => "${rendered(ctx, 0, n, 0)}",
    .Some("render-float") => "${renderedFloats(ctx, 0.1 + 0.2, 0, n, 0)}",
    _ => "0",
  };
  io.println(ctx, answer).mapErr(fn(_e) => "stdout")
}
"#;

/// What `digits` prints, worked out here.
fn digits() -> String {
    let mut xs: Vec<i64> = vec![0, 1, -1, i64::MAX, i64::MIN];
    let mut p: i64 = 10;
    for _ in 0..18 {
        xs.extend([p - 1, p, -p, 1 - p]);
        p = p.wrapping_mul(10);
    }
    let line: String = xs.iter().map(|x| format!("{x}={x} ")).collect();
    format!("{line}\n")
}

/// What `floats` prints: JavaScript's `String(x)`, with `.0` on an integral
/// value under 10²¹.
const FLOATS: &[&str] = &[
    "0.1",
    "1.5",
    "2.0",
    "-0.0",
    "-2.5",
    "123456.789",
    "0.000001",
    "1e-7",
    "0.30000000000000004",
    "100000000000000000000.0",
    "1e+21",
];

/// Runs `mode` as it ships and under the heap check, which allocates the long
/// way round, and returns what it printed, the same both ways.
fn printed(backend: &str, binary: &std::path::Path, mode: &str) -> String {
    let run = |checked: bool| {
        let mut cmd = std::process::Command::new(binary);
        cmd.args([mode, "0"]);
        if checked {
            heap_checked(&mut cmd);
        }
        let ran = ran_command(&mut cmd);
        assert_eq!(ran.status, 0, "{backend} {mode}: {}", ran.stderr);
        ran.stdout
    };
    let shipped = run(false);
    assert_eq!(shipped, run(true), "{backend} {mode}");
    shipped
}

/// **A number renders as its text at every length and sign.** For integers,
/// each power of ten from 10 to 10¹⁸, one less, and their negations, with
/// zero, one, minus one and both ends of `Int`. For floats, one to 23
/// characters, with and without an exponent. Each goes through `str.fromInt`
/// or `str.fromFloat` and through interpolation.
#[test]
fn a_number_renders_as_its_text_at_every_length() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("number-text-answers", RENDERED);
        assert_eq!(printed(backend, &binary, "digits"), digits(), "{backend}");
        let floats: String = FLOATS.iter().map(|x| format!("{x}={x} ")).collect();
        assert_eq!(
            printed(backend, &binary, "floats"),
            format!("{floats}\n"),
            "{backend}"
        );
    }
}

/// **Rendering a number copies its text without a call.** An integer's
/// digits went through a call that checked each byte against a slice it was
/// handed, which cost more than the digits did. A float's text went from its
/// stack buffer to the heap through `memcpy`, a piece at a time. Both write
/// inline now.
///
/// In instructions a step on the copy-and-patch backend, which allocates and
/// frees what it renders: four digits went from 236 to 199, and
/// `0.30000000000000004` from 745 to 596.
///
/// **An integer's text needs no stack frame when the cache has a block for
/// it.** A miss is a tail call to a second copy that allocates the long way,
/// so a hit saves no registers. Four digits went from 192 to 183.
///
/// Each bound sits about halfway. Only the copy-and-patch backend is bounded,
/// as in `growing_steps.rs`.
#[test]
fn rendering_a_number_copies_its_text_without_a_call() {
    const RENDERS: u64 = 200_000;
    let steps = RENDERS.to_string();
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("number-text-costs", RENDERED);
        for (mode, width, bound) in [("render", 4, 188), ("render-float", 19, 670)] {
            let want = format!("{}\n", width * RENDERS);
            let (n, _) = per_op(backend, &binary, mode, &[&steps], RENDERS, &want);
            eprintln!("{backend}, {mode}: {n:?} instructions a step");
            if let Some(n) = n.filter(|&n| backend == "stencil" && n > bound) {
                failures.push(format!(
                    "{backend} {mode}: {n} instructions a step, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
