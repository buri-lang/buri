//! A sort whose comparator compares two integers, natively, on every native
//! backend this toolchain has built in: what it answers at each width, signed
//! and unsigned, either way round, and what each element costs. Counted rather
//! than timed, as in [`crate::collection_costs`].

use crate::collection_costs::per_op;
use crate::shared::ran_built;

/// Lists of every kind of integer, sorted by `sort` and by `sortBy` with a
/// comparator of either order, and lists whose comparators compare something
/// else: a remainder, which `%` takes towards zero, and an element with
/// itself, which is always `Equal` and so keeps the list as it was. The
/// extremes are there for a compare at the wrong width or
/// signedness: `-128` is `128` unsigned, and `2^63` is negative signed.
const ANSWERS: &str = r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn down(a: Int, b: Int): Order { b.compare(a) }

export fn main(host: NativeHost): Result<(), Str> {
  let a = host.alloc;
  let ints = [5, -3, 9223372036854775807, 0, -9223372036854775808, 5, 2];
  let bytes: [I8] = [3, -128, 127, 0, -1];
  let ubytes: [U8] = [200, 3, 255, 0, 128];
  let words: [U64] = [9223372036854775808, 1, 18446744073709551615, 0, 9223372036854775807];
  let halves: [I32] = [-2147483648, 7, 2147483647, -1];
  let _ = io.println(host.stdout, "${ints.sort(a)}").ignore();
  let _ = io.println(host.stdout, "${ints.sortBy(a, fn(x, y) => y.compare(x))}").ignore();
  let _ = io.println(host.stdout, "${ints.sortBy(a, fn(x, y) => x.compare(y))}").ignore();
  let _ = io.println(host.stdout, "${ints.sortBy(a, down)}").ignore();
  let _ = io.println(host.stdout, "${ints.sortBy(a, fn(x, y) => (x % 3).compare(y % 3))}").ignore();
  let _ = io.println(host.stdout, "${ints.sortBy(a, fn(x, y) => x.compare(x))}").ignore();
  let _ = io.println(host.stdout, "${bytes.sort(a)} ${bytes.sortBy(a, fn(x, y) => y.compare(x))}").ignore();
  let _ = io.println(host.stdout, "${ubytes.sort(a)} ${ubytes.sortBy(a, fn(x, y) => y.compare(x))}").ignore();
  let _ = io.println(host.stdout, "${words.sort(a)} ${words.sortBy(a, fn(x, y) => y.compare(x))}").ignore();
  let _ = io.println(host.stdout, "${halves.sort(a)} ${halves.sortBy(a, fn(x, y) => y.compare(x))}").ignore();
  let _ = io.println(host.stdout, "${list.range(a, 0, 40).map(a, fn(i) => (i * 17) % 41).sort(a)}").ignore();
  .Ok(())
}
"#;

/// **A sort by an integer compare answers what the compare says**, at every
/// width, signed or unsigned, by `sort` and by a comparator of the program's
/// own either way round. The copy-and-patch backend tests the two integers in
/// place of calling such a comparator, so a test at the wrong width,
/// signedness or order would show here.
#[test]
fn a_sort_by_an_integer_compare_answers_its_order() {
    let ints = "[-9223372036854775808, -3, 0, 2, 5, 5, 9223372036854775807]";
    let down = "[9223372036854775807, 5, 5, 2, 0, -3, -9223372036854775808]";
    let expected = [
        ints.to_string(),
        down.to_string(),
        ints.to_string(),
        down.to_string(),
        "[-9223372036854775808, -3, 0, 9223372036854775807, 5, 5, 2]".to_string(),
        "[5, -3, 9223372036854775807, 0, -9223372036854775808, 5, 2]".to_string(),
        "[-128, -1, 0, 3, 127] [127, 3, 0, -1, -128]".to_string(),
        "[0, 3, 128, 200, 255] [255, 200, 128, 3, 0]".to_string(),
        "[0, 1, 9223372036854775807, 9223372036854775808, 18446744073709551615] \
         [18446744073709551615, 9223372036854775808, 9223372036854775807, 1, 0]"
            .to_string(),
        "[-2147483648, -1, 7, 2147483647] [2147483647, 7, -1, -2147483648]".to_string(),
        format!("{:?}", (0..41).filter(|i| *i != 24).collect::<Vec<_>>()),
    ]
    .join("\n")
        + "\n";
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("sort-integers", ANSWERS);
        let ran = ran_built(&binary, backend);
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, expected, "{backend}");
    }
}

/// 2,000 shuffled `Int`s sorted 50 times, by `sort` or by `sortBy` with a
/// comparator written as a lambda, or summed 50 times. `main` takes the mode.
const SORTS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn sorts<C: Allocator>(ctx: C, xs: [Int], k: Int, acc: Int): Int {
  if (k == 0) { acc } else { sorts(ctx, xs, k - 1, acc + xs.sort(ctx).first().withDefault(0) + 1) }
}

fn bys<C: Allocator>(ctx: C, xs: [Int], k: Int, acc: Int): Int {
  if (k == 0) {
    acc
  } else {
    let first = xs.sortBy(ctx, fn(a, b) => b.compare(a)).first().withDefault(0);
    bys(ctx, xs, k - 1, acc + first - 2001)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let mode = env.arguments(ctx).first().withDefault("none");
  let xs = list.range(ctx, 0, 2000).map(ctx, fn(i) => (i * 7919) % 2003);
  let answer = match (mode) {
    "sort" => sorts(ctx, xs, 50, 0),
    "sortBy" => bys(ctx, xs, 50, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// The most instructions an element [`SORTS`] may cost on the copy-and-patch
/// backend, either way.
const SORT_BOUND: u64 = 360;

/// **A sort's comparisons cost a compare, not a call.** `sort` hands
/// `sortBy` a comparator, and the copy-and-patch backend called it for every
/// comparison, then read the `Order` it returned: 546 instructions an
/// element, and 507 with a lambda of the program's own. Now a comparator
/// that compares its two integers is that test of them, and the sort's loops
/// call nothing: 407 and 381. Every loop of the sort keeps its variables in
/// registers, not only the smallest, and reads and writes an `Int` element
/// from one: 335 and 311.
#[test]
fn a_sorts_comparator_costs_a_compare() {
    let elems = 2000 * 50;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("small-comparator", SORTS);
        for mode in ["sort", "sortBy"] {
            let (n, _) = per_op(backend, &binary, mode, &[], elems, "50\n");
            eprintln!("{backend}, {mode}: {n:?} instructions an element");
            if backend == "stencil" && n.is_some_and(|n| n > SORT_BOUND) {
                failures.push(format!(
                    "{backend}: {mode} is {n:?} instructions an element, over {SORT_BOUND}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
