//! Lists `list.range` and `reverse` fill, natively, on every native backend
//! built in: their elements at every length, stride and end of `Int`, and
//! what an element costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// `main` takes `answers`, or a count of ranges. `answers` prints ranges of
/// every length up to 19 from a negative start, empty and reversed ones, and
/// ranges ending at `Int`'s top. A count `n` fills `n` ranges of 10,000
/// elements and reads three elements of each.
const RANGES: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn shown<C: Allocator>(ctx: C, xs: [Int]): Str {
  xs.foldCtx(ctx, fn(c, acc: Str, i) => acc.concat(c, str.format(c, "${i},")), "")
}

fn lines<C: Allocator>(ctx: C, n: Int, acc: Str): Str {
  if (n > 19) { acc } else { lines(ctx, n + 1, acc.concat(ctx, str.format(ctx, "${shown(ctx, list.range(ctx, -7, n - 7))}\n"))) }
}

fn answers<C: Allocator>(ctx: C): Str {
  lines(ctx, 0, "")
    .concat(ctx, str.format(ctx, "${list.range(ctx, 5, 2).length()} ${list.range(ctx, 3, 3).length()}\n"))
    .concat(ctx, str.format(ctx, "${shown(ctx, list.range(ctx, 9223372036854775797, 9223372036854775807))}\n"))
}

fn fills<C: Allocator>(ctx: C, r: Int, n: Int, acc: Int): Int {
  if (r >= n) {
    acc
  } else {
    let xs = list.range(ctx, r, r + 10000);
    let read = xs.length() + xs.get(r % 10000).withDefault(0) + xs.last().withDefault(0);
    fills(ctx, r + 1, n, (acc + read) % 1000000007)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let answer = match (env.arguments(ctx).first()) {
    .Some("answers") => answers(ctx),
    .Some(s) => str.format(ctx, "${fills(ctx, 0, s.toInt().withDefault(0), 0)}\n"),
    .None => "0\n",
  };
  io.print(ctx, answer).mapErr(fn(_e) => "stdout")
}
"#;

/// What `answers` prints, worked out here.
fn answers() -> String {
    let shown = |xs: std::ops::Range<i64>| xs.map(|i| format!("{i},")).collect::<String>();
    let mut out = String::new();
    for n in 0..=19 {
        out += &format!("{}\n", shown(-7..n - 7));
    }
    out += "0 0\n";
    out += &format!("{}\n", shown(i64::MAX - 10..i64::MAX));
    out
}

/// **`list.range` holds every `Int` from its start up to its end.** Every
/// length up to 19, so each of a vector fill's widths and its leftovers,
/// from a negative start; an empty and a reversed range; and a range whose
/// last element is one below `Int`'s top. Under the heap check.
#[test]
fn a_range_holds_every_int_from_its_start_to_its_end() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-range-answers", RANGES);
        let ran = ran_command(heap_checked(
            std::process::Command::new(&binary).arg("answers"),
        ));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, answers(), "{backend}");
    }
}

/// What `fills` prints after `n` ranges.
fn fills_answer(n: u64) -> String {
    let acc = (0..n).fold(0u64, |acc, r| {
        (acc + 10_000 + (r + r % 10_000) + (r + 9_999)) % 1_000_000_007
    });
    format!("{acc}\n")
}

/// The two counts of ranges [`RANGES`] fills, 10,000 elements each.
const FILL_COUNTS: (u64, u64) = (100, 1_100);

/// **`list.range` fills several elements a loop step.** It wrote one
/// element a step, with a saturating multiply for its offset and a
/// saturating add for its value: 12 instructions an element. `start + i`
/// never passes the end, so the add can wrap and the loop fills several
/// elements a step: 1.2 instructions an element on macOS stencil.
///
/// The bound is half the old loop's 12, so it fails if the fill goes back to
/// one element a step, and five times the new fill's figure, so the noise a
/// loaded machine adds can't reach it. The fill is the runtime's, so every
/// backend is bounded.
#[test]
fn a_range_fills_several_elements_a_step() {
    const BOUND: u64 = 6;
    let (fewer, more) = FILL_COUNTS;
    let elements = (more - fewer) * 10_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-range-fills", RANGES);
        let n = steps_cost(backend, &binary, FILL_COUNTS, fills_answer);
        let each = n.map(|n| n as f64 / elements as f64);
        eprintln!("{backend}: {each:?} instructions an element");
        if n.is_some_and(|n| n > BOUND * elements) {
            failures.push(format!(
                "{backend}: {each:?} instructions an element, over {BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Lists of every length up to 9, each one reversed and each past its first
/// element: of `Int`s, of 32-byte records holding a `Str`, and of records too
/// long to copy inline.
const COPIES: &str = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

struct P { x: Int, name: Str }

struct Long { a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, name: Str }

fn people<C: Allocator>(ctx: C, n: Int): [P] {
  list.range(ctx, 0, n).mapCtx(ctx, fn(c, i) => P { x: i, name: str.fromInt(c, i * 7) })
}

fn ints<C: Allocator>(ctx: C, xs: [Int]): Str {
  xs.foldCtx(ctx, fn(c, acc: Str, i) => acc.concat(c, str.format(c, "${i},")), "")
}

fn named<C: Allocator>(ctx: C, ps: [P]): Str {
  ps.foldCtx(ctx, fn(c, acc: Str, p) => acc.concat(c, str.format(c, "${p.x}:${p.name},")), "")
}

fn longs<C: Allocator>(ctx: C, ls: [Long]): Str {
  ls.foldCtx(ctx, fn(c, acc: Str, l) => acc.concat(c, str.format(c, "${l.a}.${l.h}:${l.name},")), "")
}

fn line<C: Allocator>(ctx: C, n: Int): Str {
  let xs = list.range(ctx, 0, n);
  let ps = people(ctx, n);
  let ls = xs.mapCtx(ctx, fn(c, i) => Long { a: i, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, h: i * 3, name: str.fromInt(c, i * 11) });
  let reversed = str.format(ctx, "${ints(ctx, xs.reverse(ctx))} ${named(ctx, ps.reverse(ctx))} ${longs(ctx, ls.reverse(ctx))}");
  let sliced = str.format(ctx, "${ints(ctx, xs.slice(ctx, 1, n))} ${named(ctx, ps.slice(ctx, 1, n))} ${longs(ctx, ls.slice(ctx, 1, n))}");
  str.format(ctx, "${reversed} ${sliced} ${ints(ctx, xs)} ${named(ctx, ps)} ${longs(ctx, ls)}\n")
}

fn lines<C: Allocator>(ctx: C, n: Int, acc: Str): Str {
  if (n > 9) { acc } else { lines(ctx, n + 1, acc.concat(ctx, line(ctx, n))) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  io.print(ctx, lines(ctx, 0, "")).mapErr(fn(_e) => "stdout")
}
"#;

/// What [`COPIES`] prints, worked out here.
fn copied_answers() -> String {
    let ints = |xs: &[i64]| xs.iter().map(|i| format!("{i},")).collect::<String>();
    let named = |xs: &[i64]| {
        xs.iter()
            .map(|i| format!("{i}:{},", i * 7))
            .collect::<String>()
    };
    let longs = |xs: &[i64]| {
        xs.iter()
            .map(|i| format!("{i}.{}:{},", i * 3, i * 11))
            .collect::<String>()
    };
    let mut out = String::new();
    for n in 0..=9 {
        let xs: Vec<i64> = (0..n).collect();
        let rev: Vec<i64> = xs.iter().rev().copied().collect();
        let tail = xs.get(1..).unwrap_or_default();
        out += &format!(
            "{} {} {} {} {} {} {} {} {}\n",
            ints(&rev),
            named(&rev),
            longs(&rev),
            ints(tail),
            named(tail),
            longs(tail),
            ints(&xs),
            named(&xs),
            longs(&xs)
        );
    }
    out
}

/// **`reverse` turns a list around, `slice` copies part of it, and both leave
/// the original as it was.** Every length up to 9, of `Int`s, of records
/// holding a `Str`, and of records past the length copied inline, each
/// printed after its copies are. Under the heap check, which fails on a count
/// a copy took or dropped.
#[test]
fn a_reversed_list_holds_the_elements_backwards() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-copy-answers", COPIES);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, copied_answers(), "{backend}");
    }
}

/// One list of 2,000 records of six `Int`s, 48 bytes that hold no count,
/// reversed `n` times, reading two records of each.
const PLAIN_FLIPS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

struct Q { a: Int, b: Int, c: Int, d: Int, e: Int, f: Int }

fn flips<C: Allocator>(ctx: C, qs: [Q], r: Int, n: Int, acc: Int): Int {
  if (r >= n) {
    acc
  } else {
    let rev = qs.reverse(ctx);
    let read = rev.first().map(fn(q) => q.a).withDefault(0) + rev.get(r % 2000).map(fn(q) => q.f).withDefault(0);
    flips(ctx, qs, r + 1, n, (acc + read) % 1000000007)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let qs = list.range(ctx, 0, 2000).map(ctx, fn(i) => Q { a: i, b: 0, c: 0, d: 0, e: 0, f: i * 3 });
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${flips(ctx, qs, 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`PLAIN_FLIPS`] prints after `n` reverses.
fn plain_flips_answer(n: u64) -> String {
    let acc = (0..n).fold(0u64, |acc, r| {
        (acc + 1_999 + (1_999 - r % 2_000) * 3) % 1_000_000_007
    });
    format!("{acc}\n")
}

/// The two counts of reverses [`PLAIN_FLIPS`] makes, of 2,000 records each.
const FLIP_COUNTS: (u64, u64) = (100, 1_100);

/// **A reversed element is a few instructions past its copy.** `reverse`
/// multiplied out both offsets each element, with saturating multiplies, and
/// moved a 16-byte word through two general registers. Its pointers step now,
/// and a 16-byte word moves through one vector register.
///
/// In instructions a 48-byte record on macOS stencil, which includes the
/// program's own loop: 28.2 before, 21.2 with only the vector register, 19.2
/// with only the pointers stepping, and 11.2 with both. The bound sits about
/// halfway between both and either alone, so it fails if either change goes.
/// The copy is the runtime's, so every backend is bounded.
#[test]
fn a_reversed_element_is_a_few_instructions_past_its_copy() {
    const BOUND: u64 = 15;
    let (fewer, more) = FLIP_COUNTS;
    let elements = (more - fewer) * 2_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("list-reverse-costs", PLAIN_FLIPS);
        let n = steps_cost(backend, &binary, FLIP_COUNTS, plain_flips_answer);
        let each = n.map(|n| n as f64 / elements as f64);
        eprintln!("{backend}: {each:?} instructions a reversed record");
        if n.is_some_and(|n| n > BOUND * elements) {
            failures.push(format!(
                "{backend}: {each:?} instructions a reversed record, over {BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
