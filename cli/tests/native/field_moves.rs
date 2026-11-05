//! A field taken out of a record its function is done with: what it answers,
//! and that every field is released once, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in; and
//! what a step costs against taking the record apart with a match.

use crate::collection_costs::per_op;
use crate::match_moves::passes;

/// A field handed to a function that keeps it, out of a record nothing reads
/// again: as a parameter, a `let`, a record with a boxed field, beside a read
/// of another field, and before a `?` that fails.
#[test]
fn a_field_taken_out_of_a_dying_record_is_released_once() {
    let test = r#"
struct Held { name: Str, tag: Str, n: Int }

struct Chain { name: Str, next: Option<Chain> }

fn held<C: Allocator>(ctx: C, n: Int): Held {
    Held { name: str.format(ctx, "n${n}"), tag: str.format(ctx, "t${n}"), n: n }
}

fn keep(s: Str): Str {
    s
}

fn joined<C: Allocator>(ctx: C, a: Str, b: Str): Str {
    str.format(ctx, "${a}+${b}")
}

fn named(h: Held): Str {
    keep(h.name)
}

fn bound<C: Allocator>(ctx: C, h: Held): Str {
    let s = h.name;
    joined(ctx, s, "x")
}

fn beside<C: Allocator>(ctx: C, h: Held): Str {
    str.format(ctx, "${h.tag}/${keep(h.name)}")
}

fn first(c: Chain): Str {
    keep(c.name)
}

fn small(n: Int): Result<Str, Str> {
    if (n < 5) { .Ok("small") } else { .Err("big") }
}

fn checked<C: Allocator>(ctx: C, h: Held): Result<Str, Str> {
    .Ok(joined(ctx, h.name, small(h.n)?))
}

fn either<C: Allocator>(ctx: C, h: Held, left: Bool): Str {
    if (left) { keep(h.name) } else { keep(h.tag) }
}

test "the field each function keeps" {
    let ctx = context { Allocator: alloc() };
    assert.equal(named(held(ctx, 1)), "n1");
    assert.equal(bound(ctx, held(ctx, 2)), "n2+x");
    assert.equal(beside(ctx, held(ctx, 3)), "t3/n3");
    let tail = Chain { name: str.format(ctx, "b"), next: .None };
    assert.equal(first(Chain { name: str.format(ctx, "a"), next: .Some(tail) }), "a");
    assert.equal(checked(ctx, held(ctx, 4)), .Ok("n4+small"));
    assert.equal(checked(ctx, held(ctx, 6)), .Err("big"));
    assert.equal(either(ctx, held(ctx, 7), true), "n7");
    assert.equal(either(ctx, held(ctx, 8), false), "t8");
}
"#;
    passes("field-moves", test, 1);
}

/// A field past the sixty-fourth, taken out of a record nothing reads again.
#[test]
fn a_field_past_the_sixty_fourth_is_released_once() {
    let fields: Vec<String> = (0..65).map(|i| format!("p{i}: Str")).collect();
    let values: Vec<String> = (0..65)
        .map(|i| format!("p{i}: str.format(ctx, \"v{i}\")"))
        .collect();
    let test = format!(
        r#"
struct Wide {{ {} }}

fn wide<C: Allocator>(ctx: C): Wide {{
    Wide {{ {} }}
}}

fn keep(s: Str): Str {{
    s
}}

fn last(w: Wide): Str {{
    keep(w.p64)
}}

test "the last field" {{
    let ctx = context {{ Allocator: alloc() }};
    assert.equal(last(wide(ctx)), "v64");
}}
"#,
        fields.join(", "),
        values.join(", ")
    );
    passes("wide-field-moves", &test, 1);
}

/// One step: a record whose field is handed on, by a read of the field or by
/// a match that takes the record apart.
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Carry { kept: Str, left: Str, n: Int }

fn carry(s: Str, other: Str, k: Int): Carry {
  Carry { kept: s, left: other, n: k }
}

fn read(c: Carry, other: Str, k: Int): Int {
  if (k == 0) { c.kept.length() } else { read(carry(c.kept, other, k), other, k - 1) }
}

fn matched(c: Carry, other: Str, k: Int): Int {
  match (c) {
    Carry { kept, left: _, n: _ } => if (k == 0) { kept.length() } else { matched(carry(kept, other, k), other, k - 1) },
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let ops = args[1].andThen(fn(s) => s.toInt()).withDefault(0);
  let mode = args.first().withDefault("none");
  let x = "ab".repeat(ctx, 2);
  let y = "cd".repeat(ctx, 2);
  let answer = if (mode == "read") {
    read(carry(x, y, 0), y, ops)
  } else if (mode == "matched") {
    matched(carry(x, y, 0), y, ops)
  } else {
    0
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A field read out of a dying record costs what taking it apart does.**
/// The field took a count of its own and the whole record was released
/// after, a retain and a release of the field a step more than the match:
/// 87 instructions against 59 on the copy-and-patch backend, and 31 against
/// 19 on LLVM. Moved, it is 57 and 18.
#[test]
fn a_field_read_out_of_a_dying_record_costs_what_taking_it_apart_does() {
    const OPS: u64 = 500_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("field-steps", STEPS);
        let n = OPS.to_string();
        let args = [n.as_str()];
        let (read, _) = per_op(backend, &binary, "read", &args, OPS, "4\n");
        let (matched, _) = per_op(backend, &binary, "matched", &args, OPS, "4\n");
        eprintln!("{backend}: read {read:?}, matched {matched:?} instructions a step");
        if let (Some(read), Some(matched)) = (read, matched) {
            if read > matched + 10 {
                failures.push(format!(
                    "{backend}: reading the field is {read} instructions a step against {matched}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
