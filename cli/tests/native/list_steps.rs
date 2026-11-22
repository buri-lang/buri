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
