//! A `match` that tests its value's variant one arm at a time, natively, on
//! every native backend this toolchain has built in: which arm each variant
//! takes, at every width of tag and through `Option`'s null pointer, and what
//! a step of such a match costs. Counted rather than timed, as in
//! [`crate::collection_costs`].

use crate::collection_costs::per_op;
use crate::shared::ran_built;

/// A program whose matches test one variant after another: arms with guards
/// and bound fields, which a match tests in order, over an enum with a byte
/// tag, one with 300 variants and so a two-byte tag, a variant past 127 of
/// the byte, `Option<Int>`, and `Option<Str>`, whose `.None` is a null
/// pointer.
fn answers() -> String {
    let variants: Vec<String> = (0..300)
        .map(|i| {
            if i % 50 == 0 {
                format!("V{i}(Int)")
            } else {
                format!("V{i}")
            }
        })
        .collect();
    let wide: Vec<String> = (0..260)
        .map(|i| {
            if i == 200 {
                format!("W{i}(Int)")
            } else {
                format!("W{i}")
            }
        })
        .collect();
    format!(
        r#"
from "native" import {{ NativeHost }};
from "core/io" import * as io;

enum Op {{ Add(Int), Neg, Pair(Int, Int) }}
enum Big {{ {big} }}
enum Wide {{ {byte} }}

fn op(o: Op): Int {{
  match (o) {{
    .Add(k) if k > 10 => 1,
    .Add(k) => k * 10,
    .Neg => 2,
    .Pair(a, b) if a > b => a - b,
    .Pair(a, b) => a * b,
  }}
}}

fn big(b: Big): Int {{
  match (b) {{
    .V0(k) if k > 0 => k,
    .V250(k) if k > 0 => k + 250,
    .V250(k) => 0 - k,
    .V299 => 299,
    .V150(k) => k + 150,
    _ => -1,
  }}
}}

fn byte(b: Wide): Int {{
  match (b) {{
    .W200(k) if k > 0 => k + 200,
    .W200(k) => 0 - k,
    .W255 => 255,
    .W1 => 1,
    _ => -1,
  }}
}}

fn int(o: Option<Int>, k: Int): Int {{
  match (o) {{
    .Some(x) if x > k => x,
    .Some(x) => 0 - x,
    .None => 7,
  }}
}}

fn text(o: Option<Str>, k: Int): Str {{
  match (o) {{
    .Some(s) if k > 0 => s,
    .Some(_s) => "small",
    .None => "none",
  }}
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let _ = io.println(host.stdout, "${{op(.Add(11))}} ${{op(.Add(3))}} ${{op(.Neg)}} ${{op(.Pair(5, 2))}} ${{op(.Pair(2, 5))}}").ignore();
  let _ = io.println(host.stdout, "${{big(.V0(4))}} ${{big(.V0(0))}} ${{big(.V250(1))}} ${{big(.V250(-3))}} ${{big(.V299)}} ${{big(.V150(1))}} ${{big(.V298)}} ${{big(.V1)}}").ignore();
  let _ = io.println(host.stdout, "${{byte(.W200(5))}} ${{byte(.W200(-5))}} ${{byte(.W255)}} ${{byte(.W1)}} ${{byte(.W0)}} ${{byte(.W128)}}").ignore();
  let _ = io.println(host.stdout, "${{int(.Some(9), 3)}} ${{int(.Some(2), 3)}} ${{int(.None, 3)}}").ignore();
  let _ = io.println(host.stdout, "${{text(.Some("given"), 1)}} ${{text(.Some("given"), 0)}} ${{text(.None, 1)}}").ignore();
  .Ok(())
}}
"#,
        big = variants.join(", "),
        byte = wide.join(", "),
    )
}

/// **Each variant takes its own arm**, in a match that tests its variants
/// one arm at a time, whatever the width of the tag and through `Option`'s
/// null pointer. The copy-and-patch backend reads such a test's tag in place
/// rather than through a slot, so a test at the wrong width, of the wrong
/// variant or the wrong way round would show here.
#[test]
fn each_variant_takes_its_own_arm() {
    let expected = "1 30 2 3 10\n\
                    4 -1 251 3 299 151 -1 -1\n\
                    205 5 255 1 -1 -1\n\
                    9 -2 7\n\
                    given small none\n";
    let program = answers();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("variant-tests", &program);
        let ran = ran_built(&binary, backend);
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, expected, "{backend}");
    }
}

/// A step's cost on the copy-and-patch backend, in instructions, over the
/// loop in [`STEPS`]. Measured at 52 once a payload's fields are read where
/// they are and `i % 5` and the tag are each read once for all their tests;
/// it was 58 with them read again for each test, and 64 with the tag stored
/// to a slot and read back for each arm's test.
const STEP_BOUND: u64 = 55;

/// The match of `design/PERFORMANCE.md` §9's `match` program: a step builds
/// one of five variants and a match tests it arm by arm, guards and all.
const STEPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

enum Op { Add(Int), Mul(Int), Neg, Skip, Pair(Int, Int) }

struct St { acc: Int, steps: Int, last: Int }

fn op(i: Int): Op {
  match (i % 5) {
    0 => .Add(i % 13),
    1 => .Mul(i % 3 + 1),
    2 => .Neg,
    3 => .Skip,
    _ => .Pair(i % 7, i % 11),
  }
}

fn apply(s: St, o: Op): St {
  match (o) {
    .Add(k) => St { ..s, acc: (s.acc + k) % 1000003, steps: s.steps + 1 },
    .Mul(k) => St { ..s, acc: s.acc * k % 1000003, steps: s.steps + 1 },
    .Neg => St { ..s, acc: 1000003 - s.acc, last: s.acc },
    .Skip => s,
    .Pair(a, b) if a > b => St { ..s, acc: (s.acc + a - b) % 1000003 },
    .Pair(a, b) => St { ..s, last: a * b },
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
  let s = run(St { acc: 1, steps: 0, last: 0 }, 0, n);
  io.println(ctx, "${if (n == 0) { 0 } else { s.acc + s.steps + s.last }}").mapErr(fn(_e) => "stdout")
}
"#;

/// **A match that tests its variants arm by arm costs a few instructions an
/// arm** on the copy-and-patch backend, its tag read in place for each test.
#[test]
fn a_variant_test_costs_a_compare() {
    let steps = 200_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("variant-steps", STEPS);
        let (n, _) = per_op(backend, &binary, "steps", &[], steps, "279050\n");
        eprintln!("{backend}: {n:?} instructions a step");
        if backend == "stencil" && n.is_some_and(|n| n > STEP_BOUND) {
            failures.push(format!(
                "{backend}: {n:?} instructions a step, over {STEP_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
