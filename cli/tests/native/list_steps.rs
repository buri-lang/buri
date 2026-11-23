//! A list loop's step, natively, on every native backend built in: what each
//! kind of step answers, and what an element of a `map` through a small lambda
//! costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// Prints one line of answers, each from a list loop whose step is written a
/// different way: arithmetic, a record built and read, a capture, a function
/// passed by name, the element itself, a body too big to copy, a function of
/// another module, a body that branches, texts read, retained and joined, a
/// fold, a test, a sort and a list of lists.
const ANSWERS: &str = r#"
from "core/bytes" import * as bytes;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

struct R { a: Int, b: Int }

fn tenfold(x: Int): Int { x * 10 }

fn big(x: Int): Int {
  let a = x * 3 + 1;
  let b = a * a - x;
  let c = b % 7 + a;
  let d = c * c + b * a;
  let e = d - c + b - a;
  e % 1000 + (a + b + c) % 13 + d % 11 + e % 17
}

fn ints<C: Allocator>(ctx: C, xs: [Int]): Str { xs.mapCtx(ctx, fn(c, x) => x.show(c)).join(ctx, ",") }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let xs = [3, 1, 2];
  let k = 5;
  let words = ["ab".concat(ctx, "c"), "d".concat(ctx, "ef"), "g".concat(ctx, "")];
  let answers = [
    ints(ctx, xs.map(ctx, fn(x) => x * 2 + 1)),
    ints(ctx, xs.map(ctx, fn(x) => R { a: x, b: x + 1 }).map(ctx, fn(r) => r.a * r.b)),
    ints(ctx, xs.map(ctx, fn(x) => x + k)),
    ints(ctx, xs.map(ctx, tenfold)), ints(ctx, xs.filter(ctx, fn(x) => x > 1).map(ctx, tenfold)),
    ints(ctx, xs.map(ctx, fn(x) => x)),
    ints(ctx, xs.map(ctx, big)),
    ints(ctx, xs.map(ctx, bytes.zigzag)),
    ints(ctx, xs.map(ctx, fn(x) => if (x > 1) { x } else { 0 - x })),
    ints(ctx, words.map(ctx, fn(w) => w.length())),
    words.map(ctx, fn(w) => [w, w]).mapCtx(ctx, fn(c, p) => p.join(c, "")).join(ctx, ","),
    words.mapCtx(ctx, fn(c, w) => w.concat(c, "!")).join(ctx, ","),
    words.filter(ctx, fn(w) => w.length() > 1).join(ctx, ","),
    xs.fold(fn(acc, x) => acc * 10 + x, 0).show(ctx),
    ints(ctx, [xs.any(fn(x) => x == 2), xs.all(fn(x) => x > 1)].map(ctx, fn(b) => if (b) { 1 } else { 0 })),
    xs.count(fn(x) => x != 1).show(ctx),
    ints(ctx, xs.sortBy(ctx, fn(a, b) => b.compare(a))),
    ints(ctx, [xs, [4]].map(ctx, fn(l) => l.length())),
  ].join(ctx, " ");
  io.println(ctx, answers).mapErr(fn(_e) => "stdout")
}
"#;

/// **Each kind of list step answers as it did.** A step whose body is one
/// short block is written out in its loop, and the rest stay calls, so each
/// kind of step is here, once each: one that builds, reads, captures,
/// branches, retains or joins texts, one passed by name, one of another
/// module, and one too big to write out. Under the heap check, so a step
/// written out in its loop still releases what the call did.
#[test]
fn each_kind_of_list_step_answers_as_it_did() {
    let want = [
        "7,3,5",            // arithmetic
        "12,2,6",           // a record built, then read
        "8,6,7",            // a capture
        "30,10,20",         // a function passed by name
        "30,20",            // a filter, then that function again
        "3,1,2",            // the element itself
        "313,116,510",      // a body too big to write out
        "6,2,4",            // a function of another module
        "3,-1,2",           // a body that branches
        "3,3,1",            // texts read
        "abcabc,defdef,gg", // texts retained in a list, then joined
        "abc!,def!,g!",     // texts joined, with the context
        "abc,def",          // texts kept
        "312",              // a fold
        "1,0",              // tests
        "2",                // a count
        "3,2,1",            // a sort
        "3,1",              // a list of lists
    ]
    .join(" ")
        + "\n";
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-steps", ANSWERS);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, want, "{backend}");
    }
}

/// `main` takes a count `n`, maps 64 numbers through `STEP` `n` times and
/// prints the sum of the last list's numbers.
const MAPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int, last: [Int]): [Int] {
  if (i >= n) { last } else { rounds(ctx, xs, i + 1, n, xs.map(ctx, fn(x) => x * 3 + i)) }
}

fn sum(xs: [Int]): Int { xs.fold(fn(acc, x) => acc + x, 0) }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let xs = list.range(ctx, 0, 64);
  io.println(ctx, "${sum(rounds(ctx, xs, 0, n, []))}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`MAPS`] prints after `n` rounds.
fn sum_after(n: u64) -> String {
    let i = n.saturating_sub(1);
    let sum = if n == 0 {
        0
    } else {
        (0..64u64).map(|x| x * 3 + i).sum()
    };
    format!("{sum}\n")
}

/// The two counts of rounds [`MAPS`] takes: 600 thousand rounds apart, which
/// is 38 million elements and hundreds of millions of instructions between
/// the two runs.
const ROUNDS: (u64, u64) = (100_000, 700_000);

/// **A `map` through a small lambda costs no call an element.** The lambda
/// was called for each element. Now its body is written out in the loop, and
/// on the copy-and-patch backend the loop keeps its index in a register. In
/// instructions an element on macOS copy-and-patch: 32.6 before, 17.6 now.
/// The bound is 25.1, halfway, many times the noise of a difference of 38
/// million elements, which is under a hundredth of an instruction an element.
#[test]
fn a_map_through_a_small_lambda_costs_no_call_an_element() {
    let (fewer, more) = ROUNDS;
    let elements = (more - fewer) * 64;
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-steps-map", MAPS);
        let n = steps_cost(backend, &binary, ROUNDS, sum_after);
        // In tenths of an instruction an element.
        let tenths = n.map(|n| n * 10 / elements);
        eprintln!("{backend}: {tenths:?} tenths of an instruction an element");
        if backend == "stencil" {
            if let Some(tenths) = tenths {
                assert!(
                    tenths < MAP_BOUND_TENTHS,
                    "{backend}: {tenths} tenths of an instruction an element"
                );
            }
        }
    }
}

/// See [`a_map_through_a_small_lambda_costs_no_call_an_element`].
const MAP_BOUND_TENTHS: u64 = 251;

/// **A step that makes a record besides the one it answers answers its
/// own.** `buri test` on every backend: the record the step answers is stored
/// in its element, and the spare one made after it is not.
#[test]
fn a_step_that_makes_a_spare_record_answers_its_own() {
    let test = r#"
from "core/list" import * as list;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

struct Twice { a: Int, b: Int }

test "spare record" {
    let ctx = context { Allocator: alloc() };
    let rs = list.range(ctx, 0, 4).map(ctx, fn(x) => {
        let t = Twice { a: x, b: x + 1 };
        let other = Twice { a: t.b, b: x };
        t
    });
    let sums = rs.map(ctx, fn(r) => r.a * 10 + r.b);
    assert.equal(sums.length(), 4);
    assert.equal(sums.get(0).withDefault(-1), 1);
    assert.equal(sums.get(1).withDefault(-1), 12);
    assert.equal(sums.get(3).withDefault(-1), 34);
}
"#;
    crate::odd_sizes::passes("list-steps-spare", test, 1);
}

/// **A step that captures several values answers as it did**: `buri test` on
/// every backend, with steps that capture three values each. One builds a
/// record of what it captured, one adds them up, one divides, one branches,
/// and one is too long to write out in its loop, so the first two are called
/// directly and written out in their loops and the rest go through their
/// closures.
#[test]
fn a_step_that_captures_several_values_answers_as_it_did() {
    let test = r#"
from "core/list" import * as list;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

struct Pair { a: Int, b: Int }
struct Held { n: Int, name: Str, f: Float, p: Pair }

test "wide captures" {
    let ctx = context { Allocator: alloc() };
    let xs = list.range(ctx, 0, 4);
    let name = "n".concat(ctx, "4");
    let f = 1.5;
    let p = Pair { a: 7, b: xs.length() };
    let a = xs.length() - 1;
    let b = a + 2;
    let c = a + 4;
    let held = xs.map(ctx, fn(x) => Held { n: x, name: name, f: f, p: p });
    let sums = xs.map(ctx, fn(x) => x * a + b - c);
    let quotients = xs.map(ctx, fn(x) => (x + a + b) / c);
    let branches = xs.map(ctx, fn(x) => if (x > a - 1) { b } else { c });
    let long = xs.map(ctx, fn(x) => x * a + b * x + c * x + a * b + b * c + c * a + x * x + a + b + c);
    assert.equal(held.length(), 4);
    assert.equal(held.get(3).map(fn(h) => h.n + h.p.a + h.p.b).withDefault(-1), 14);
    assert.equal(held.get(2).map(fn(h) => h.name.length()).withDefault(-1), 2);
    assert.equal(held.get(1).map(fn(h) => h.f).withDefault(0.0), 1.5);
    assert.equal(sums.get(3).withDefault(-1), 7);
    assert.equal(quotients.get(3).withDefault(-1), 1);
    assert.equal(branches.get(0).withDefault(-1), 7);
    assert.equal(branches.get(3).withDefault(-1), 5);
    assert.equal(long.get(2).withDefault(-1), 120);
}
"#;
    crate::odd_sizes::passes("list-steps-wide", test, 1);
}

/// [`MAPS`], with a step that captures three values: the round and two
/// numbers `main` was handed.
const WIDE_MAPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, xs: [Int], a: Int, b: Int, i: Int, n: Int, last: [Int]): [Int] {
  if (i >= n) { last } else { rounds(ctx, xs, a, b, i + 1, n, xs.map(ctx, fn(x) => x * a + b + i)) }
}

fn sum(xs: [Int]): Int { xs.fold(fn(acc, x) => acc + x, 0) }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let xs = list.range(ctx, 0, 64);
  let a = xs.length() / 21;
  io.println(ctx, "${sum(rounds(ctx, xs, a, a - 3, 0, n, []))}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A `map` through a lambda that captures several values costs no call an
/// element.** A step whose environment is wider than a word went through its
/// closure. Now it is called directly, so its body is written out in the loop
/// as [`a_map_through_a_small_lambda_costs_no_call_an_element`]'s is. Rounds
/// and elements are [`ROUNDS`]'s, so the noise is as small.
#[test]
fn a_map_through_a_lambda_that_captures_several_values_costs_no_call_an_element() {
    let (fewer, more) = ROUNDS;
    let elements = (more - fewer) * 64;
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-steps-wide-map", WIDE_MAPS);
        let n = steps_cost(backend, &binary, ROUNDS, sum_after);
        let tenths = n.map(|n| n * 10 / elements);
        eprintln!("{backend}: {tenths:?} tenths of an instruction an element");
        if backend == "stencil" {
            if let Some(tenths) = tenths {
                assert!(
                    tenths < WIDE_BOUND_TENTHS,
                    "{backend}: {tenths} tenths of an instruction an element"
                );
            }
        }
    }
}

/// See [`a_map_through_a_lambda_that_captures_several_values_costs_no_call_an_element`].
const WIDE_BOUND_TENTHS: u64 = 362;

/// `main` takes a count `n`, maps 64 numbers to their text through a step
/// that asks the runtime for it, `n` times, and prints the last texts'
/// lengths added up. The text comes back through an out-pointer.
const TEXTS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, xs: [Int], i: Int, n: Int, last: [Str]): [Str] {
  if (i >= n) { last } else { rounds(ctx, xs, i + 1, n, xs.mapCtx(ctx, fn(c, x) => str.fromInt(c, x + i))) }
}

fn total(xs: [Str]): Int { xs.fold(fn(acc, s) => acc + s.length(), 0) }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let xs = list.range(ctx, 0, 64);
  io.println(ctx, "${total(rounds(ctx, xs, 0, n, []))}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`TEXTS`] prints after `n` rounds.
fn lengths_after(n: u64) -> String {
    let i = n.saturating_sub(1);
    let total: usize = if n == 0 {
        0
    } else {
        (0..64u64).map(|x| (x + i).to_string().len()).sum()
    };
    format!("{total}\n")
}

/// The two counts of rounds [`TEXTS`] takes: 150 thousand rounds apart, which
/// is 9.6 million elements and over a billion instructions between the runs.
const TEXT_ROUNDS: (u64, u64) = (50_000, 200_000);

/// **A runtime call's out-pointer is passed without a trip through the
/// frame.** The step's `str.fromInt` answers through the address of the
/// step's slot, which was written to the frame by one stencil and read back
/// by the call's; the call's stencil now computes it. In instructions an
/// element on macOS copy-and-patch: 193.4 before, 191.5 now, the same to a
/// thousandth in two runs. The bound is 192.4, halfway: nine million
/// instructions from each, where the runs differ by none.
#[test]
fn a_runtime_answer_is_written_through_its_slot_address() {
    let (fewer, more) = TEXT_ROUNDS;
    let elements = (more - fewer) * 64;
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-steps-texts", TEXTS);
        let n = steps_cost(backend, &binary, TEXT_ROUNDS, lengths_after);
        // In tenths of an instruction an element.
        let tenths = n.map(|n| n * 10 / elements);
        eprintln!("{backend}: {tenths:?} tenths of an instruction an element");
        if backend == "stencil" {
            if let Some(tenths) = tenths {
                assert!(
                    tenths < TEXT_BOUND_TENTHS,
                    "{backend}: {tenths} tenths of an instruction an element"
                );
            }
        }
    }
}

/// See [`a_runtime_answer_is_written_through_its_slot_address`].
const TEXT_BOUND_TENTHS: u64 = 1924;
