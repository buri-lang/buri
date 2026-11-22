//! Two texts compared, natively, on every native backend built in: every way
//! a program asks, at every length that takes its own path, and what a step
//! of comparisons costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// The texts [`ANSWERS`] compares, each against each. Lengths 0 to 36, pairs
/// that differ only in their last byte, prefixes of each other, and bytes
/// over 127, which order after every ASCII byte.
const TEXTS: [&str; 24] = [
    "",
    "a",
    "b",
    "z",
    "ab",
    "abc",
    "abd",
    "abcd",
    "abce",
    "abcdefg",
    "abcdefh",
    "abcdefgh",
    "abcdefgi",
    "abcdefghi",
    "abcdefghijklmno",
    "abcdefghijklmnop",
    "abcdefghijklmnoq",
    "abcdefghijklmnopq",
    "abcdefghijklmnopqrstuvwxyz0123456789",
    "abcdefghijklmnopqrstuvwxyz0123456788",
    "abcdefghijklmnopqrstuvwxyz012345678",
    "é",
    "é€",
    "e€",
];

/// Prints a line for each pair of [`TEXTS`], the first against a copy of the
/// second in a block of its own, then against the second itself.
const ANSWERS: &str = r#"
from "core/io" import * as io;
from "core/order" import { Order };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

struct Id(Str);
derive Equal, Ordered for Id;

fn sign(o: Order): Str { match (o) { .Less => "<", .Equal => "=", .Greater => ">" } }
fn bit(b: Bool): Str { if (b) { "1" } else { "0" } }

fn line<C: Allocator>(ctx: C, a: Str, b: Str): Str {
  bit(a == b).concat(ctx, bit(a != b)).concat(ctx, bit(a < b)).concat(ctx, bit(a <= b))
    .concat(ctx, bit(a > b)).concat(ctx, bit(a >= b)).concat(ctx, sign(a.compare(b)))
    .concat(ctx, bit(Id(a) == Id(b))).concat(ctx, sign(Id(a).compare(Id(b)))).concat(ctx, "\n")
}

fn copy<C: Allocator>(ctx: C, s: Str): Str {
  s.concat(ctx, "!").slice(0, s.length())
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let xs = [TEXTS];
  let lines = xs.mapCtx(ctx, fn(c, a) => xs.mapCtx(c, fn(c2, b) => line(c2, a, copy(c2, b)).concat(c2, line(c2, a, b))).join(c, "")).join(ctx, "");
  io.print(ctx, lines).mapErr(fn(_e) => "stdout")
}
"#;

/// [`ANSWERS`] with [`TEXTS`] put in.
fn answers_program() -> String {
    let texts: Vec<String> = TEXTS.iter().map(|t| format!("{t:?}")).collect();
    ANSWERS.replace("TEXTS", &texts.join(", "))
}

/// What [`ANSWERS`] prints, from Rust's byte order of the same texts.
fn answers() -> String {
    let bit = |b: bool| if b { "1" } else { "0" };
    let sign = |o: std::cmp::Ordering| match o {
        std::cmp::Ordering::Less => "<",
        std::cmp::Ordering::Equal => "=",
        std::cmp::Ordering::Greater => ">",
    };
    let line = |a: &str, b: &str| {
        let o = a.as_bytes().cmp(b.as_bytes());
        [
            bit(a == b),
            bit(a != b),
            bit(a < b),
            bit(a <= b),
            bit(a > b),
            bit(a >= b),
            sign(o),
            bit(a == b),
            sign(o),
            "\n",
        ]
        .concat()
    };
    let mut out = String::new();
    for a in TEXTS {
        for b in TEXTS {
            out += &line(a, b);
            out += &line(a, b);
        }
    }
    out
}

/// **Two texts compare by their bytes, whichever way a program asks.** `==`,
/// `!=`, the four orderings, `compare`, and a struct of one text's derived
/// `Equal` and `Ordered`, for each pair of [`TEXTS`]: against a copy, whose
/// bytes are somewhere else, and against the text itself. Under the heap
/// check.
#[test]
fn texts_compare_by_their_bytes_whichever_way_a_program_asks() {
    let want = answers();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("text-compares", &answers_program());
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, want, "{backend}");
    }
}

/// `main` takes a count `n` and makes five comparisons of three 13-byte texts
/// `n` times: two through operators, two through a struct's derived `Equal`
/// and `Ordered`, and one `compare`. It prints three for each step.
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/order" import { Order };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Id(Str);
derive Equal, Ordered for Id;

fn rank(o: Order): Int { match (o) { .Less => 0, .Equal => 1, .Greater => 2 } }
fn one(b: Bool): Int { if (b) { 1 } else { 0 } }

fn steps(a: Str, b: Str, c: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let k = one(a == b) + 2 * one(a < c) + rank(Id(a).compare(Id(c))) + rank(b.compare(c)) + one(Id(b) == Id(c));
    steps(a, b, c, i + 1, n, acc + k)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let a = "id-".concat(ctx, "0000000042");
  let b = "id-".concat(ctx, "0000000042");
  let c = "id-".concat(ctx, "0000000043");
  io.println(ctx, "${steps(a, b, c, 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`STEPS`] takes.
const STEP_COUNTS: (u64, u64) = (1_000, 21_000);

/// **A comparison of two short texts costs no call into the runtime on the
/// copy-and-patch backend.** Each comparison was a call of six arguments with
/// the registers saved around it, and is now one stencil that compares the
/// words itself. In instructions a step of five comparisons on macOS stencil:
/// 456 to 378. The bound sits halfway, 39 a step from each, about three
/// times the worst macOS noise of the new figure, so it fails if the calls
/// come back and leaves the rest of the step room to move. Only this backend
/// changed, so only it is bounded.
#[test]
fn a_short_text_compare_costs_no_runtime_call() {
    let (fewer, more) = STEP_COUNTS;
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("text-compare-steps", STEPS);
        let n = steps_cost(backend, &binary, STEP_COUNTS, |n| format!("{}\n", n * 3));
        let per = n.map(|n| n / (more - fewer));
        eprintln!("{backend}: {per:?} instructions a step");
        if backend == "stencil" {
            assert!(
                per.is_none_or(|p| p <= BOUND),
                "{backend}: {per:?} instructions a step, over {BOUND}"
            );
        }
    }
}

/// See [`a_short_text_compare_costs_no_runtime_call`].
const BOUND: u64 = 417;
