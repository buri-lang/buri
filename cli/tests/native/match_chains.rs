//! Tests of one value against literal after literal, natively, on every
//! native backend this toolchain has built in: a match on an integer, a
//! chain of `==` and `!=` tests, a match on a byte tag, and the payload each
//! arm reads. The copy-and-patch backend reads the value once and keeps it in
//! a register from one test to the next, so a test that compared a stale
//! register, or the register where another way into the test left something
//! else there, would show here.

use crate::collection_costs::{per_op, steps_cost};
use crate::shared::ran_built;

const ANSWERS: &str = r#"
from "native" import { NativeHost };
from "core/io" import * as io;

enum Shape { Dot, Line(Int), Box(Int, Int, Int), Mark(Float, Int), Empty }

fn int(i: Int): Int {
  match (i) {
    0 => 10,
    1 => 11,
    -5 => 12,
    1000000000000 => 13,
    -9223372036854775807 => 14,
    _ => 15,
  }
}

fn ne(i: Int): Int {
  if (i != 3) {
    if (i != 4) { if (i == 70000) { 1 } else { 2 } } else { 3 }
  } else { 4 }
}

fn joined(x: Int, y: Int): Int {
  let z = if (y == 1) { x } else { y };
  if (z == 5) { 50 } else if (z == 6) { 60 } else if (z == 1) { 10 } else { 0 }
}

fn both(x: Int, y: Int): Int {
  if (x == 1 && y == 5) { 1 } else if (x == 2) { 2 } else { 3 }
}

fn guarded(x: Int, y: Int): Int {
  match (x) {
    1 if y == 5 => 1,
    2 => 2,
    _ => 3,
  }
}

fn tagged(s: Shape, y: Int): Int {
  match (s) {
    .Dot if y == 1 => 1,
    .Line(_n) => 2,
    _ => 3,
  }
}

fn shape(s: Shape): Int {
  match (s) {
    .Dot => 1,
    .Line(n) => n,
    .Box(a, b, c) => a * 100 + b * 10 + c,
    .Mark(_f, k) => k + 1000,
    .Empty => 0,
  }
}

fn kind(s: Shape): Int {
  match (s) {
    .Empty => 5,
    .Mark(_f, _k) => 4,
    .Box(_a, _b, _c) => 3,
    .Line(_n) => 2,
    .Dot => 1,
  }
}

fn total(i: Int, acc: Int): Int {
  if (i == 0) { acc } else { total(i - 1, acc + int(i % 3) + ne(i % 5) + kind(.Line(i))) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let _ = io.println(host.stdout, "${int(0)} ${int(1)} ${int(-5)} ${int(1000000000000)} ${int(-9223372036854775807)} ${int(2)} ${int(1000000000001)}").ignore();
  let _ = io.println(host.stdout, "${ne(3)} ${ne(4)} ${ne(70000)} ${ne(5)}").ignore();
  let _ = io.println(host.stdout, "${joined(5, 1)} ${joined(9, 6)} ${joined(1, 1)} ${joined(6, 1)} ${joined(0, 2)}").ignore();
  let _ = io.println(host.stdout, "${both(1, 2)} ${both(1, 5)} ${both(2, 5)} ${both(3, 3)} ${guarded(1, 2)} ${guarded(1, 5)} ${guarded(2, 5)} ${guarded(3, 3)}").ignore();
  let _ = io.println(host.stdout, "${tagged(.Dot, 2)} ${tagged(.Dot, 0)} ${tagged(.Dot, 1)} ${tagged(.Line(1), 1)} ${tagged(.Empty, 1)}").ignore();
  let _ = io.println(host.stdout, "${shape(.Dot)} ${shape(.Line(7))} ${shape(.Box(1, 2, 3))} ${shape(.Mark(2.5, 9))} ${shape(.Empty)}").ignore();
  let _ = io.println(host.stdout, "${kind(.Dot)} ${kind(.Line(7))} ${kind(.Box(1, 2, 3))} ${kind(.Mark(2.5, 9))} ${kind(.Empty)}").ignore();
  let _ = io.println(host.stdout, "${total(30, 0)}").ignore();
  .Ok(())
}
"#;

/// **Each literal takes its own arm**, in a match on an integer, a chain of
/// `!=` tests, chains whose test is also reached from a second way in, where
/// the register holds another value, and
/// a match on a byte tag, with each arm reading its variant's fields.
#[test]
fn each_literal_takes_its_own_arm() {
    let expected = "10 11 12 13 14 15 15\n\
                    4 3 1 2\n\
                    50 60 10 60 0\n\
                    3 1 2 3 3 1 2 3\n\
                    3 3 1 2 3\n\
                    1 7 123 1009 0\n\
                    1 2 3 4 5\n\
                    498\n";
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("match-chains", ANSWERS);
        let ran = ran_built(&binary, backend);
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, expected, "{backend}");
    }
}

/// A step's cost on the copy-and-patch backend, in instructions, over the
/// loop in [`STEPS`]: a match of `i % 8` against seven literals, then of a
/// byte tag against five variants. Measured at 35 with each value read once
/// into a register for all its tests, and 34.6 since the remainder is computed
/// into that register, not stored and then read back.
///
/// The check was 34 after rounding down, which a run measuring 35.0 failed:
/// the cost sat 0.4 under the bound, and a loaded machine's extra kernel work
/// pushed it over. The figure is now the difference between two counts of
/// steps, so the setup and the reading of the argument cancel, and it is
/// compared unrounded with the same limit, 35.
const STEP_BOUND: u64 = 35;

/// The two counts of steps [`STEP_BOUND`] is measured between: two million
/// apart, so the noise left in each count is a small part of an instruction a
/// step.
const STEP_COUNTS: (u64, u64) = (1_000_000, 3_000_000);

/// A step tests `i % 8` against literal after literal and an enum's byte tag
/// against variant after variant, each value read once for all its tests.
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

enum Day { Mon, Tue, Wed, Thu, Fri, Sat }

fn day(i: Int): Day {
  match (i % 8) {
    0 => .Mon,
    1 => .Tue,
    2 => .Wed,
    3 => .Thu,
    4 => .Fri,
    5 => .Tue,
    6 => .Wed,
    _ => .Sat,
  }
}

fn hours(d: Day): Int {
  match (d) {
    .Sat => 0,
    .Fri => 6,
    .Thu => 7,
    .Wed => 9,
    .Tue => 8,
    .Mon => 5,
  }
}

fn run(acc: Int, i: Int, n: Int): Int {
  if (i == n) { acc } else { run(acc + hours(day(i)), i + 1, n) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${run(0, 0, n)}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A test against literal after literal costs a compare a literal** on the
/// copy-and-patch backend, the value read once rather than for each test.
#[test]
fn a_literal_test_costs_a_compare() {
    let (fewer, more) = STEP_COUNTS;
    let steps = more - fewer;
    // A day of each of the eight remainders is 52 hours.
    let hours = |n: u64| format!("{}\n", n / 8 * 52);
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("chain-steps", STEPS);
        let n = steps_cost(backend, &binary, STEP_COUNTS, hours);
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

/// A step's cost on the copy-and-patch backend, in instructions, over the
/// loop in [`NESTED_STEPS`]: a record built by one of an `if`'s two branches
/// inside a match arm, whose answer is the match's answer.
/// Measured at 52 with the record built in the match's place, and 58 when
/// the `if` built it aside and the match copied it; 37 since the match's
/// place is the loop's own record and each new field is computed in it.
const NESTED_STEP_BOUND: u64 = 40;

/// A step matches a `Pair` and builds a record in one of the two branches of
/// an `if` inside the arm, so the `if`'s answer is passed straight on as the
/// match's.
const NESTED_STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

enum Op { Pair(Int, Int), Skip }

struct St { acc: Int, steps: Int, last: Int }

fn op(i: Int): Op {
  if (i == -1) { .Skip } else { .Pair(i % 7, i % 11) }
}

fn apply(s: St, o: Op): St {
  match (o) {
    .Pair(a, b) => if (a > b) {
      St { ..s, acc: s.acc + a - b }
    } else {
      St { ..s, last: a * b, steps: s.steps + 1 }
    },
    .Skip => s,
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
  io.println(ctx, "${s.acc * 1000000 + s.steps * 100 + s.last}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A record built inside a match arm is built where the match answers** on
/// the copy-and-patch backend, rather than built aside and then copied there.
#[test]
fn a_record_built_inside_an_arm_is_not_copied_out() {
    let steps = 200_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("nested-steps", NESTED_STEPS);
        let (n, _) = per_op(backend, &binary, "steps", &[], steps, "145464545416\n");
        eprintln!("{backend}: {n:?} instructions a step");
        if backend == "stencil" && n.is_some_and(|n| n > NESTED_STEP_BOUND) {
            failures.push(format!(
                "{backend}: {n:?} instructions a step, over {NESTED_STEP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
