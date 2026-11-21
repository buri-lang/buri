//! A record a tail-recursive loop carries and each step rebuilds from its own
//! fields, natively, on every native backend this toolchain has built in. The
//! copy-and-patch backend builds the new record over the old one, so a field
//! stored before another field read the old value would show here.

use crate::collection_costs::steps_cost;
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

/// The two counts of steps [`STEPS`] is measured between.
const STEP_COUNTS: (u64, u64) = (1_000_000, 3_000_000);

/// What [`STEPS`] prints after `n` steps.
fn steps_answer(n: u64) -> String {
    let (mut acc, mut steps, mut last) = (0u64, 0u64, 0u64);
    for i in 0..n {
        match i % 3 {
            0 => acc += i % 13,
            1 => {}
            _ => (acc, steps, last) = (acc + 1, steps + 1, last + 2),
        }
    }
    format!("{}\n", acc * 1_000_000_000_000 + steps * 1_000_000 + last)
}

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
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let s = run(St { acc: 0, steps: 0, last: 0 }, 0, n);
  io.println(ctx, "${s.acc * 1000000000000 + s.steps * 1000000 + s.last}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A loop's record is rebuilt where it is** on the copy-and-patch backend,
/// its unchanged fields left in place, rather than built aside and copied
/// back each step.
#[test]
fn a_loop_record_is_rebuilt_in_place() {
    let (fewer, more) = STEP_COUNTS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("loop-record-steps", STEPS);
        let n = steps_cost(backend, &binary, STEP_COUNTS, steps_answer);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && n.is_some_and(|n| n > STEP_BOUND * steps) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {STEP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// **A record keeps its unchanged fields through a step**, built by `buri
/// test` on every backend: a field kept between two new ones, the last of them
/// computed before the first, and a record built from a field of another kind
/// of record.
#[test]
fn a_step_keeps_the_fields_it_does_not_change() {
    let test = r#"
struct St { acc: Int, steps: Int, last: Int }

struct P { x: Int, y: Int }

struct A { pad: Int, p: P }

struct B { p: P, k: Int }

fn tick(s: St, i: Int, n: Int): St {
    if (i == n) {
        s
    } else {
        let last = s.last * 3 % 101;
        tick(St { acc: s.acc + i, steps: s.steps, last: last }, i + 1, n)
    }
}

fn made(i: Int): A {
    A { pad: i, p: P { x: i + 1, y: i + 2 } }
}

fn converted(i: Int): B {
    let a = made(i);
    B { p: a.p, k: a.pad }
}

fn walk(b: B, i: Int, n: Int): B {
    if (i == n) { b } else { walk(B { p: converted(i).p, k: b.k + b.p.x }, i + 1, n) }
}

test "kept fields" {
    let t = tick(St { acc: 1, steps: 2, last: 3 }, 0, 10);
    assert.equal(t.acc, 46);
    assert.equal(t.steps, 2);
    assert.equal(t.last, 94);
    let b = walk(converted(0), 0, 5);
    assert.equal(b.p.x, 5);
    assert.equal(b.p.y, 6);
    assert.equal(b.k, 11);
}
"#;
    crate::match_moves::passes("loop-record-kept", test, 1);
}

/// **A constant less a value answers the difference**, built by `buri test` on
/// every backend: less a field a loop carries, less an argument, less a sum
/// computed just before, and less a value below zero.
#[test]
fn a_constant_less_a_value_is_the_difference() {
    let test = r#"
struct St { acc: Int, steps: Int, last: Int }

fn flip(s: St, i: Int, n: Int): St {
    if (i == n) { s } else { flip(St { ..s, acc: 1000003 - s.acc, last: s.acc + i }, i + 1, n) }
}

fn less(x: Int): Int {
    7 - x
}

fn lessSum(x: Int, y: Int): Int {
    100 - (x + y)
}

test "constant first" {
    let s = flip(St { acc: 5, steps: 2, last: 0 }, 0, 7);
    assert.equal(s.acc, 999998);
    assert.equal(s.steps, 2);
    assert.equal(s.last, 11);
    assert.equal(less(10), -3);
    assert.equal(less(-9223372036854775800), 9223372036854775807);
    assert.equal(lessSum(30, 12), 58);
}
"#;
    crate::match_moves::passes("constant-first", test, 1);
}

/// A step's cost on the copy-and-patch backend, in instructions, over the
/// loop in [`FLIPS`]. Exactly 15 with the constant an operand of the
/// subtraction, and 16 when it was put in a frame slot first. A step may
/// cost up to half an instruction more than this, since macOS's counter
/// reads a little over or under the cost and 15 is the cost itself.
const FLIP_BOUND: u64 = 15;

/// The two counts of steps [`FLIPS`] is measured between. A run's count
/// is off by up to about 60,000 instructions on macOS, so ten million steps
/// put that under a hundredth of an instruction a step.
const FLIP_COUNTS: (u64, u64) = (5_000_000, 15_000_000);

/// A step takes a field from a constant, as a `match` arm negating a
/// value modulo a prime does.
const FLIPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct St { acc: Int, steps: Int, last: Int }

fn flip(s: St, i: Int, n: Int): St {
  if (i == n) { s } else { flip(St { ..s, acc: 1000003 - s.acc, last: s.acc }, i + 1, n) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let s = flip(St { acc: 0, steps: 0, last: 0 }, 0, n);
  io.println(ctx, "${s.acc + s.last}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A constant less a field costs one subtraction**: the constant is the
/// stencil's operand rather than a frame slot written each step and read back.
#[test]
fn a_constant_less_a_field_costs_one_subtraction() {
    let (fewer, more) = FLIP_COUNTS;
    let steps = more - fewer;
    // Each pair of flips puts `acc` back, and `acc + last` is the constant
    // after any flip.
    let answer = |n: u64| format!("{}\n", if n == 0 { 0 } else { 1000003 });
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("constant-first-steps", FLIPS);
        let n = steps_cost(backend, &binary, FLIP_COUNTS, answer);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && n.is_some_and(|n| n > FLIP_BOUND * steps + steps / 2) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {FLIP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
