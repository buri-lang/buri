//! A small struct of several fields compared through its derived `Equal` and
//! `Ordered`, natively, on every native backend built in: what each way of
//! writing the operands answers, and what a step of such comparisons costs
//! next to comparing the fields by hand, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// Prints one line of answers, each operand written a different way: a struct
/// written out at the comparison, one held in a name, a field of a record, a
/// call's answer, a struct of such structs, a struct of one such struct, a
/// struct of floats, a tuple, and the derives of an option, a list and a sort
/// that compare such a struct inside them.
const ANSWERS: &str = r#"
from "core/io" import * as io;
from "core/option" import { Option };
from "core/order" import { Order };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

struct Key { name: Str, n: Int }
derive Equal, Ordered, Show for Key;
struct Two { k: Key, m: Key }
derive Equal, Ordered for Two;
struct Wrap(Key);
derive Equal, Ordered for Wrap;
struct Point { x: Float, y: Float }
derive Equal, Ordered for Point;
struct Holder { key: Key, tag: Int }

fn sign(o: Order): Str { match (o) { .Less => "<", .Equal => "=", .Greater => ">" } }
fn bit(b: Bool): Str { if (b) { "1" } else { "0" } }
fn pick(xs: [Key], i: Int): Key { xs.get(i).withDefault(Key { name: "", n: 0 }) }
fn said<C: Stdout>(ctx: C, line: Str, s: Str): Str { let _ = io.println(ctx, line).ignore(); s }
fn told<C: Stdout>(ctx: C, line: Str, xs: [Key], i: Int): Key { let _ = io.println(ctx, line).ignore(); pick(xs, i) }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = "a".concat(ctx, "b");
  let b = "a".concat(ctx, "c");
  let x = Key { name: a, n: 1 };
  let y = Key { name: a, n: 2 };
  let z = Key { name: b, n: 0 };
  let keys = [Key { name: b, n: 0 }, Key { name: a, n: 2 }, Key { name: a, n: 1 }];
  let h = Holder { key: y, tag: 3 };
  let g = Holder { key: z, tag: 3 };
  let nan = 0.0 / 0.0;
  let ox: Option<Key> = .Some(x);
  let oy: Option<Key> = .Some(y);
  let oa: Option<Key> = .Some(Key { name: a, n: 1 });
  let answers = [
    bit(Key { name: a, n: 1 } == Key { name: a, n: 1 }), sign(Key { name: a, n: 1 }.compare(Key { name: a, n: 2 })),
    bit(Key { name: b, n: 0 } == Key { name: a, n: 0 }), sign(Key { name: b, n: 0 }.compare(Key { name: a, n: 9 })),
    bit(x == y), sign(y.compare(x)), bit(x != z), bit(x < y), bit(z <= x),
    bit(h.key == y), sign(h.key.compare(g.key)),
    bit(pick(keys, 1) == y), sign(pick(keys, 0).compare(pick(keys, 2))),
    bit(told(ctx, "first", keys, 2) == Key { name: said(ctx, "second", a), n: 1 }),
    sign(told(ctx, "third", keys, 9).compare(Key { name: said(ctx, "fourth", b), n: 0 })),
    bit(Two { k: x, m: y } == Two { k: x, m: y }), sign(Two { k: x, m: z }.compare(Two { k: x, m: y })),
    bit(Wrap(x) == Wrap(y)), sign(Wrap(z).compare(Wrap(x))),
    bit(Point { x: nan, y: 1.0 } == Point { x: nan, y: 1.0 }),
    sign(Point { x: 0.5, y: 2.0 }.compare(Point { x: 0.5, y: nan })),
    sign(Point { x: -0.0, y: 0.0 }.compare(Point { x: 0.0, y: 0.0 })),
    bit((1, a) == (1, a)), bit((2, a) == (2, b)),
    bit(ox == oy), bit(ox == oa), sign([x, y].compare([x, z])),
  ].join(ctx, " ");
  io.println(ctx, "${answers} ${keys.sort(ctx)}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A small struct of several fields compares field by field, however the
/// operands are written.** The derived functions of such a struct are written
/// out at each comparison, with each operand that isn't a name named first,
/// so every way of writing one is here, once each. The calls that print show
/// each operand evaluated once, in order. Under the heap check, so a struct
/// written out at the comparison and never built still releases what it held.
#[test]
fn a_small_struct_compares_field_by_field() {
    let want = [
        "1 < 0 >",   // written out at the comparison
        "0 > 1 1 0", // held in names
        "1 <",       // fields of records
        "1 >",       // calls' answers
        "1 <",       // calls' answers beside structs built around calls
        "1 >",       // a struct of such structs
        "0 >",       // a struct of one such struct
        "1 < <",     // floats: `NaN` equal to itself, last in order, `-0.0` first
        "1 0",       // tuples
        "0 1 <",     // inside an option's and a list's derives
        r#"[Key { name: "ab", n: 1 }, Key { name: "ab", n: 2 }, Key { name: "ac", n: 0 }]"#,
    ]
    .join(" ")
        + "\n";
    // Each operand is evaluated once, first to last, as the call did.
    let want = format!("first\nsecond\nthird\nfourth\n{want}");
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("small-derives", ANSWERS);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, want, "{backend}");
    }
}

/// `main` takes a count `n` and makes four comparisons of three keys, each a
/// 13-byte text and a number, `n` times, and prints three for each step. The
/// texts are equal, so each comparison reads both fields. `EQ(p, q)` and
/// `CMP(p, q)` are either the derived functions or the fields compared by
/// hand ([`by_hand`]).
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/order" import { Order };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Key { name: Str, n: Int }
derive Equal, Ordered for Key;

fn rank(o: Order): Int { match (o) { .Less => 0, .Equal => 1, .Greater => 2 } }
fn one(b: Bool): Int { if (b) { 1 } else { 0 } }

fn steps(p: Key, q: Key, r: Key, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let k = rank(CMP(p, r)) + one(EQ(q, r)) + one(EQ(p, q)) + rank(CMP(r, p));
    steps(p, q, r, i + 1, n, acc + k)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let p = Key { name: "id-".concat(ctx, "0000000042"), n: 1 };
  let q = Key { name: "id-".concat(ctx, "0000000042"), n: 1 };
  let r = Key { name: "id-".concat(ctx, "0000000042"), n: 2 };
  io.println(ctx, "${steps(p, q, r, 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// [`STEPS`] with its comparisons the derived functions, `p == q` and
/// `p.compare(q)`.
fn derived() -> String {
    STEPS
        .replace("EQ(q, r)", "q == r")
        .replace("EQ(p, q)", "p == q")
        .replace("CMP(p, r)", "p.compare(r)")
        .replace("CMP(r, p)", "r.compare(p)")
}

/// [`STEPS`] with its comparisons written by hand, field by field, in the
/// order the derived functions compare them.
fn by_hand() -> String {
    let eq = |x: &str, y: &str| format!("({x}.name == {y}.name && {x}.n == {y}.n)");
    let cmp = |x: &str, y: &str| {
        format!("match ({x}.name.compare({y}.name)) {{ .Equal => {x}.n.compare({y}.n), o => o }}")
    };
    STEPS
        .replace("EQ(q, r)", &eq("q", "r"))
        .replace("EQ(p, q)", &eq("p", "q"))
        .replace("CMP(p, r)", &cmp("p", "r"))
        .replace("CMP(r, p)", &cmp("r", "p"))
}

/// The two counts of steps [`STEPS`] takes: two million steps apart, which is
/// over 400 million instructions between the two runs of either program.
const STEP_COUNTS: (u64, u64) = (100_000, 2_100_000);

/// **Comparing a small struct costs what comparing its fields by hand
/// costs.** Each comparison was a call to the derived function. In
/// instructions, a step of four comparisons cost 64 more than the fields
/// compared by hand on macOS copy-and-patch, now 8 fewer. The bound is 32
/// more, halfway, so it fails if the calls come back, and many times the
/// noise a difference of two million steps carries, which is under one a
/// step.
#[test]
fn a_small_struct_compares_at_its_fields_cost() {
    let (fewer, more) = STEP_COUNTS;
    for (backend, build) in crate::e2e::probed_backends() {
        let per = |name: &str, source: &str| {
            let binary = build(name, source);
            let n = steps_cost(backend, &binary, STEP_COUNTS, |n| format!("{}\n", n * 3));
            n.map(|n| n / (more - fewer))
        };
        let derived = per("small-derive-steps", &derived());
        let by_hand = per("small-by-hand-steps", &by_hand());
        eprintln!("{backend}: {derived:?} and {by_hand:?} instructions a step");
        if let Some((derived, by_hand)) = derived.zip(by_hand) {
            assert!(
                derived <= by_hand + BOUND,
                "{backend}: {derived} instructions a step through the derived functions, \
                 {by_hand} by hand, over {BOUND} apart"
            );
        }
    }
}

/// See [`a_small_struct_compares_at_its_fields_cost`].
const BOUND: u64 = 32;
