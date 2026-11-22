//! Lists whose every element is stored as they are made, and lists that keep
//! only some, natively, on every native backend built in: what each holds,
//! and what making one of wide records costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// Prints one line for each way of making a list: a literal, `map`, `zip`,
/// `flatten`, `show`, and `sort` of numbers and of texts, which store every
/// element, and `filter` and `filterMap`, which keep only some. The elements
/// are numbers, texts and records holding texts, so the lists' releases walk
/// what they hold.
const MADE: &str = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

struct Row { name: Str, n: Int, tag: Str }

fn line<C: Allocator>(ctx: C, xs: [Str]): Str { xs.join(ctx, ",") }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let ns = list.range(ctx, 0, 6);
  let ts = ns.mapCtx(ctx, fn(c, i) => str.format(c, "t${5 - i}"));
  let rows = ns.mapCtx(ctx, fn(c, i) => Row { name: str.format(c, "r${i}"), n: i * 10, tag: "x" });
  let literal = [ts.get(0).withDefault(""), "b", ts.get(5).withDefault("")];
  let pairs = ts.zip(ctx, ns).mapCtx(ctx, fn(c, p) => str.format(c, "${p.0}${p.1}"));
  let flat = [ts, literal, []].flatten(ctx);
  let kept = rows.filter(ctx, fn(r) => r.n % 20 == 0).mapCtx(ctx, fn(c, r) => r.name);
  let some = ts.filterMap(ctx, fn(t) => if (t == "t2") { .None } else { .Some(t) });
  let lines = [
    line(ctx, literal),
    line(ctx, ns.mapCtx(ctx, fn(c, i) => str.format(c, "${i * i}"))),
    line(ctx, rows.mapCtx(ctx, fn(c, r) => str.format(c, "${r.name}:${r.n}"))),
    line(ctx, pairs),
    line(ctx, flat),
    str.format(ctx, "${literal}"),
    line(ctx, ns.map(ctx, fn(i) => (i * 5 + 2) % 6).sort(ctx).mapCtx(ctx, fn(c, i) => str.format(c, "${i}"))),
    line(ctx, ts.sort(ctx)),
    line(ctx, kept),
    line(ctx, some),
  ];
  io.println(ctx, lines.join(ctx, "\n")).mapErr(fn(_e) => "stdout")
}
"#;

/// **Every way of making a list holds what it was given.** A list whose every
/// element is stored as it is made, a literal or a `map`'s, is no longer
/// cleared first on the copy-and-patch backend, and one that keeps only some,
/// a `filter`'s, still is. Under the heap check, so each list's release walks
/// exactly what it holds.
#[test]
fn every_way_of_making_a_list_holds_what_it_was_given() {
    let want = [
        "t5,b,t0",
        "0,1,4,9,16,25",
        "r0:0,r1:10,r2:20,r3:30,r4:40,r5:50",
        "t50,t41,t32,t23,t14,t05",
        "t5,t4,t3,t2,t1,t0,t5,b,t0",
        r#"["t5", "b", "t0"]"#,
        "0,1,2,3,4,5",
        "t0,t1,t2,t3,t4,t5",
        "r0,r2,r4",
        "t5,t4,t3,t1,t0",
    ]
    .join("\n")
        + "\n";
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("filled-lists", MADE);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, want, "{backend}");
    }
}

/// `main` takes a count `n` and maps a list of 64 numbers `n` times, each
/// element to `MADE`: a record of eight numbers, or the number itself. It
/// prints the sum of the answers' lengths.
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Row { a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int }

fn steps<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { steps(ctx, xs, i + 1, n, acc + xs.map(ctx, fn(x) => MADE).length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${steps(ctx, list.range(ctx, 0, 64), 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`STEPS`] takes: 400,000 steps apart, which is
/// over 780 million instructions between the two runs of either program on
/// copy-and-patch.
const STEP_COUNTS: (u64, u64) = (20_000, 420_000);

/// **Mapping to a wide record isn't cleared first.** A `map`'s block was
/// cleared before the loop stored every element over it. In instructions, a
/// step of 64 records of eight numbers costs this much more than a step of 64
/// numbers on macOS copy-and-patch: 1,406 before, 1,126 now. The bound is
/// halfway, 140 from each, so it fails if the clearing comes back, and leaves
/// the stores about 12% to grow. The noise a difference of 400,000 steps
/// carries is under one a step.
#[test]
fn mapping_to_a_wide_record_does_not_clear_its_block_first() {
    let (fewer, more) = STEP_COUNTS;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let per = |name: &str, made: &str| {
            let binary = build(name, &STEPS.replace("MADE", made));
            let n = steps_cost(backend, &binary, STEP_COUNTS, |n| format!("{}\n", n * 64));
            n.map(|n| n / (more - fewer))
        };
        let rows = per(
            "filled-rows-steps",
            "Row { a: x, b: i, c: x, d: i, e: x, f: i, g: x, h: i }",
        );
        let ints = per("filled-ints-steps", "x + i");
        let (Some(rows), Some(ints)) = (rows, ints) else {
            continue;
        };
        eprintln!("{backend}: rows {rows}, numbers {ints} a step");
        if backend == "stencil" && rows.saturating_sub(ints) > WIDER_BOUND {
            failures.push(format!(
                "{backend}: records cost {} a step more than numbers, over {WIDER_BOUND}",
                rows.saturating_sub(ints)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// See [`mapping_to_a_wide_record_does_not_clear_its_block_first`].
const WIDER_BOUND: u64 = 1266;
