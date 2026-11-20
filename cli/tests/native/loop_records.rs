//! A record a tail-recursive loop carries and each step rebuilds from its own
//! fields, natively, on every native backend this toolchain has built in. The
//! copy-and-patch backend builds the new record over the old one, so a field
//! stored before another field read the old value would show here.

use crate::collection_costs::per_op;
use crate::shared::ran_built;

const ANSWERS: &str = r#"
from "native" import { NativeHost };
from "core/io" import * as io;

struct St { acc: Int, steps: Int, last: Int }

struct P { x: Int, y: Int }

struct W { p: P, q: P, n: Int }

fn step(s: St, i: Int): St {
  match (i % 7) {
    0 => St { ..s, acc: s.acc + i },
    1 => St { acc: s.last, steps: s.acc, last: s.steps },
    2 => St { ..s, acc: 1000 - s.acc, last: s.acc },
    3 => s,
    4 => St { acc: s.steps, steps: s.acc, last: s.last + 1 },
    5 => St { acc: (s.acc + s.steps) % 10007, steps: s.acc * 2 % 10007, last: s.last },
    _ => St { ..s, steps: s.steps + 1 },
  }
}

fn run(s: St, i: Int, n: Int): St {
  if (i == n) { s } else { run(step(s, i), i + 1, n) }
}

fn wide(w: W, i: Int, n: Int): W {
  if (i == n) { w } else { wide(W { p: w.q, q: w.p, n: w.n + i }, i + 1, n) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let s = run(St { acc: 1, steps: 2, last: 3 }, 0, 100);
  let w = wide(W { p: P { x: 1, y: 2 }, q: P { x: 3, y: 4 }, n: 0 }, 0, 7);
  let _ = io.println(host.stdout, "${s.acc} ${s.steps} ${s.last}").ignore();
  let _ = io.println(host.stdout, "${w.p.x} ${w.p.y} ${w.q.x} ${w.q.y} ${w.n}").ignore();
  .Ok(())
}
"#;

/// **Each step sees the record the step before it built**: fields rotated,
/// swapped, wide fields swapped, a field read after another took a new value,
/// a new value computed from a field another new value replaces, and the
/// record passed on unchanged.
#[test]
fn each_step_reads_the_record_before_it() {
    let expected = "17 4596 7029\n3 4 1 2 21\n";
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("loop-records", ANSWERS);
        let ran = ran_built(&binary, backend);
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, expected, "{backend}");
    }
}

/// A step's cost on the copy-and-patch backend, in instructions, over the
/// loop in [`STEPS`]. Measured at 27 with the record rebuilt over the loop's
/// own and each new field computed in its place, 30 with the new fields
/// computed aside and stored, and 42 when the record was built aside and
/// copied back.
const STEP_BOUND: u64 = 29;

/// A step matches an operation and rebuilds the loop's record, changing one
/// of its fields, all three, or none.
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

enum Op { Add(Int), Skip, Bump }

struct St { acc: Int, steps: Int, last: Int }

fn op(i: Int): Op {
  if (i % 3 == 0) { .Add(i % 13) } else if (i % 3 == 1) { .Skip } else { .Bump }
}

fn apply(s: St, o: Op): St {
  match (o) {
    .Add(k) => St { ..s, acc: s.acc + k },
    .Skip => s,
    .Bump => St { acc: s.acc + 1, steps: s.steps + 1, last: s.last + 2 },
  }
}

fn run(s: St, i: Int, n: Int): St {
  if (i == n) { s } else { run(apply(s, op(i)), i + 1, n) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = match (env.arguments(ctx).first().withDefault("none")) {
    "steps" => 200000,
    _ => 0,
  };
  let s = run(St { acc: 0, steps: 0, last: 0 }, 0, n);
  io.println(ctx, "${s.acc * 1000000000000 + s.steps * 1000000 + s.last}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A loop's record is rebuilt where it is** on the copy-and-patch backend,
/// its unchanged fields left in place, rather than built aside and copied
/// back each step.
#[test]
fn a_loop_record_is_rebuilt_in_place() {
    let steps = 200_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("loop-record-steps", STEPS);
        let (n, _) = per_op(
            backend,
            &binary,
            "steps",
            &[],
            steps,
            "466659066666133332\n",
        );
        eprintln!("{backend}: {n:?} instructions a step");
        if backend == "stencil" && n.is_some_and(|n| n > STEP_BOUND) {
            failures.push(format!(
                "{backend}: {n:?} instructions a step, over {STEP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
