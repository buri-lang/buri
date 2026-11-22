//! A struct of one field compared through its derived `Equal` and `Ordered`,
//! natively, on every native backend built in: what each way of writing the
//! operands answers, and what a step of such comparisons costs next to
//! comparing the fields themselves, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// Prints one line of answers, each operand written a different way: a struct
/// written out at the comparison, one held in a name, a field of a record, a
/// call's answer, a struct of such a struct, a struct of a number, and the
/// derives of a record, an option and a sort that compare such a struct inside
/// them.
const ANSWERS: &str = r#"
from "core/io" import * as io;
from "core/option" import { Option };
from "core/order" import { Order };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

struct Id(Str);
derive Equal, Ordered, Show for Id;
struct Outer(Id);
derive Equal, Ordered for Outer;
struct Count(Int);
derive Equal, Ordered for Count;
struct Ratio(Float);
derive Equal, Ordered for Ratio;
struct Row { id: Id, n: Int }
derive Equal, Ordered for Row;

fn sign(o: Order): Str { match (o) { .Less => "<", .Equal => "=", .Greater => ">" } }
fn bit(b: Bool): Str { if (b) { "1" } else { "0" } }
fn pick(xs: [Id], i: Int): Id { xs.get(i).withDefault(Id("")) }
fn said<C: Stdout>(ctx: C, line: Str, s: Str): Str { let _ = io.println(ctx, line).ignore(); s }
fn told<C: Stdout>(ctx: C, line: Str, xs: [Id], i: Int): Id { let _ = io.println(ctx, line).ignore(); pick(xs, i) }

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = "a".concat(ctx, "b");
  let b = "a".concat(ctx, "c");
  let x = Id(a);
  let y = Id(b);
  let ids = [Id(a), Id(b), Id(a)];
  let r = Row { id: Id(b), n: 1 };
  let s = Row { id: Id(a), n: 2 };
  let nan = 0.0 / 0.0;
  let ox: Option<Id> = .Some(x);
  let oy: Option<Id> = .Some(y);
  let oa: Option<Id> = .Some(Id(a));
  let answers = [
    bit(Id(a) == Id(b)), sign(Id(a).compare(Id(b))), bit(Id(a) == Id(a)), sign(Id(b).compare(Id(a))),
    bit(x == y), sign(y.compare(x)), bit(x != y), bit(x < y),
    bit(r.id == s.id), sign(r.id.compare(s.id)),
    bit(pick(ids, 0) == pick(ids, 2)), sign(pick(ids, 0).compare(pick(ids, 1))),
    bit(told(ctx, "first", ids, 0) == Id(said(ctx, "second", b))),
    sign(told(ctx, "third", ids, 1).compare(Id(said(ctx, "fourth", a)))),
    bit(Outer(x) == Outer(Id(a))), sign(Outer(y).compare(Outer(x))),
    bit(Count(3) == Count(4)), sign(Count(-5).compare(Count(4))),
    bit(Ratio(nan) == Ratio(nan)), bit(nan == nan), sign(Ratio(1.5).compare(Ratio(0.5))),
    bit(r == s), sign(r.compare(s)), bit(ox == oy), bit(ox == oa),
  ].join(ctx, " ");
  io.println(ctx, "${answers} ${ids.sort(ctx)}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A struct of one field compares as its field does, however the operands
/// are written.** The derived functions of such a struct are written out at
/// each comparison, and an operand that is costly to read the field of, such
/// as a call's answer, is named first, so every way of writing one is here,
/// once each. The calls that print show each operand evaluated once, in
/// order. Under the heap check, so a struct written out at the comparison and
/// never built still releases what it held.
#[test]
fn a_struct_of_one_field_compares_as_its_field_does() {
    let want = [
        "0 < 1 >", // written out at the comparison
        "0 > 1 1", // held in names
        "0 >",     // fields of records
        "1 <",     // calls' answers
        "0 >",     // calls' answers beside structs built around calls
        "1 >",     // a struct of a struct
        "0 <",     // a struct of an `Int`
        "1 1 >",   // a struct of a `Float`, `NaN` equal to itself either way (SPEC 7.2)
        "0 > 0 1", // inside a record's and an option's derives
        r#"[Id("ab"), Id("ab"), Id("ac")]"#,
    ]
    .join(" ")
        + "\n";
    // Each operand is evaluated once, first to last, as the call did.
    let want = format!("first\nsecond\nthird\nfourth\n{want}");
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("one-field-derives", ANSWERS);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, want, "{backend}");
    }
}

/// `main` takes a count `n` and makes four comparisons of three 13-byte texts
/// `n` times, each through `COMPARE`, and prints three for each step.
/// `COMPARE` is either a struct of one text written out around each operand,
/// or nothing, which compares the texts themselves.
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
    let k = rank(COMPARE(a).compare(COMPARE(c))) + one(COMPARE(b) == COMPARE(c)) + one(COMPARE(a) == COMPARE(b)) + rank(COMPARE(c).compare(COMPARE(a)));
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

/// The two counts of steps [`STEPS`] takes: two million steps apart, which is
/// over 400 million instructions between the two runs of either program.
const STEP_COUNTS: (u64, u64) = (100_000, 2_100_000);

/// **Comparing a struct of one field costs what comparing the field costs.**
/// Each comparison was a call to the derived function, and the struct was
/// built to pass to it. In instructions a step of four comparisons more than
/// the texts themselves cost: 272 copy-and-patch and 127 LLVM release on arm64
/// Linux, and 232 copy-and-patch on macOS, now 0 on each. The bound is 40,
/// over three times under the smallest of the old figures, and many times the
/// noise a difference of two million steps carries, which is under one a
/// step.
#[test]
fn a_struct_of_one_field_compares_at_its_fields_cost() {
    let (fewer, more) = STEP_COUNTS;
    for (backend, build) in crate::e2e::probed_backends() {
        let per = |name: &str, wrap: &str| {
            let binary = build(name, &STEPS.replace("COMPARE", wrap));
            let n = steps_cost(backend, &binary, STEP_COUNTS, |n| format!("{}\n", n * 3));
            n.map(|n| n / (more - fewer))
        };
        let wrapped = per("one-field-derive-steps", "Id");
        let direct = per("one-field-direct-steps", "");
        eprintln!("{backend}: {wrapped:?} and {direct:?} instructions a step");
        if let Some((wrapped, direct)) = wrapped.zip(direct) {
            assert!(
                wrapped <= direct + BOUND,
                "{backend}: {wrapped} instructions a step through the struct, \
                 {direct} without, over {BOUND} apart"
            );
        }
    }
}

/// See [`a_struct_of_one_field_compares_at_its_fields_cost`].
const BOUND: u64 = 40;

/// `main` takes a count `n` and makes two comparisons of calls' answers, each
/// a struct of one text, `n` times, and prints one for each step. `STEP` is
/// the step: the calls made at the comparisons, or the answers named first.
/// `pick` is too big to inline, so each is a call either way.
const CALLED: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/order" import { Order };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Id(Str);
derive Equal, Ordered for Id;

fn rank(o: Order): Int { match (o) { .Less => 0, .Equal => 1, .Greater => 2 } }
fn one(b: Bool): Int { if (b) { 1 } else { 0 } }
fn pick(xs: [Id], i: Int): Id { xs.get(i % 3).withDefault(Id("")) }

fn steps(xs: [Id], i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    STEP
    steps(xs, i + 1, n, acc + k)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let a = "id-".concat(ctx, "0000000042");
  let b = "id-".concat(ctx, "0000000043");
  io.println(ctx, "${steps([Id(a), Id(b), Id(a)], 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// [`CALLED`]'s step with the calls made at the comparisons.
const AT_THE_CALL: &str =
    "let k = one(pick(xs, 0) == pick(xs, 2)) + rank(pick(xs, 0).compare(pick(xs, 1)));";

/// [`CALLED`]'s step with the answers named first.
const NAMED: &str = "let p = pick(xs, 0); let q = pick(xs, 2); let r = pick(xs, 0); \
     let t = pick(xs, 1); let k = one(p == q) + rank(r.compare(t));";

/// **Comparing calls' answers costs what comparing named answers costs.** A
/// struct of one field that a call answered was compared through a call to
/// the derived function. Now each answer is named first, its field is read
/// with no count of its own, and it is released once. In instructions, a
/// step of two such comparisons cost this much more than the same step with
/// the answers named: 11 copy-and-patch on macOS and 10 on arm64 Linux, now
/// 0 on each. The bound is 5, halfway, so it fails if the call comes back,
/// and is many times the noise a difference of two million steps carries,
/// which is under one a step.
#[test]
fn comparing_calls_answers_costs_what_comparing_named_ones_does() {
    let (fewer, more) = STEP_COUNTS;
    for (backend, build) in crate::e2e::probed_backends() {
        let per = |name: &str, step: &str| {
            let binary = build(name, &CALLED.replace("STEP", step));
            let n = steps_cost(backend, &binary, STEP_COUNTS, |n| format!("{n}\n"));
            n.map(|n| n / (more - fewer))
        };
        let at_the_call = per("one-field-called-steps", AT_THE_CALL);
        let named = per("one-field-named-steps", NAMED);
        eprintln!("{backend}: {at_the_call:?} and {named:?} instructions a step");
        if let Some((at_the_call, named)) = at_the_call.zip(named) {
            assert!(
                at_the_call <= named + CALLED_BOUND,
                "{backend}: {at_the_call} instructions a step comparing calls' answers, \
                 {named} with them named, over {CALLED_BOUND} apart"
            );
        }
    }
}

/// See [`comparing_calls_answers_costs_what_comparing_named_ones_does`].
const CALLED_BOUND: u64 = 5;
