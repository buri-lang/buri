//! A `Str` split and searched, natively, on every native backend built in:
//! the pieces and positions at every count and needle length, and what one
//! split or search costs, counted rather than timed.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, ran_command};

/// `main` takes the mode and a count. `answers` prints splits and searches;
/// `split`, `index` and `contains` do one of each a step and print the sum of
/// what they found.
const SPLIT: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn joined<C: Allocator>(ctx: C, xs: [Str], i: Int, acc: Str): Str {
  match (xs[i]) {
    .None => acc,
    .Some(x) => joined(ctx, xs, i + 1, acc.concat(ctx, str.format(ctx, "[${x}:${x.length()}]"))),
  }
}

fn commas<C: Allocator>(ctx: C, i: Int, n: Int, acc: Str): Str {
  if (i >= n) { acc } else { commas(ctx, i + 1, n, acc.concat(ctx, str.fromInt(ctx, i)).concat(ctx, ",")) }
}

fn cut<C: Allocator>(ctx: C, s: Str, sep: Str): Str {
  let parts = s.split(ctx, sep);
  str.format(ctx, "${parts.length()}:${joined(ctx, parts, 0, "")}\n")
}

fn found<C: Allocator>(ctx: C, s: Str, needle: Str): Str {
  str.format(ctx, "${s.indexOf(needle).withDefault(-1)} ${s.contains(needle)}\n")
}

fn answers<C: Allocator>(ctx: C): Str {
  let long = "abcdefghijklmnopqrst";
  let counts = list.range(ctx, 0, 41).foldCtx(ctx, fn(c, acc: Str, n) => acc.concat(c, cut(c, commas(c, 0, n, ""), ",")), "");
  counts
    .concat(ctx, cut(ctx, "a--b-c---d--", "--"))
    .concat(ctx, cut(ctx, "x", "xy"))
    .concat(ctx, cut(ctx, "", ","))
    .concat(ctx, cut(ctx, "-a-", "-"))
    .concat(ctx, cut(ctx, "--------x---------", "-"))
    .concat(ctx, cut(ctx, "abcdefg-ijklmno-qrstuvw-", "-"))
    .concat(ctx, cut(ctx, "ä-ö--ü-", "-"))
    .concat(ctx, cut(ctx, "no separator here, at all", "-"))
    .concat(ctx, found(ctx, "a9 9a 99", "99"))
    .concat(ctx, found(ctx, "a9 9a 9", "99"))
    .concat(ctx, found(ctx, "abcdefghijklmnopqrsX abcdefghijklmnopqrst", long))
    .concat(ctx, found(ctx, "abcdefghijklmnopqrsX abcdefghijklmnopqrsT", long))
    .concat(ctx, found(ctx, "abcdefghijklmnopq", "abcdefghijklmnopq"))
    .concat(ctx, found(ctx, "abcdefghijklmnopQ abcdefghijklmnopq.", "abcdefghijklmnopq."))
    .concat(ctx, found(ctx, "ä-ö-ü", "ö"))
    .concat(ctx, found(ctx, "ä-ö-ü", "-"))
}

fn splits<C: Allocator>(ctx: C, s: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { splits(ctx, s, i + 1, n, acc + s.split(ctx, "-").length()) }
}

fn indexes(s: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { indexes(s, i + 1, n, acc + s.indexOf("-").withDefault(0)) }
}

fn containing(s: Str, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let t = if (s.contains("99")) { 1 } else { 2 };
    containing(s, i + 1, n, acc + t)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let s = "item-919293-949596";
  let answer = match (args.first()) {
    .Some("answers") => answers(ctx),
    .Some("split") => str.format(ctx, "${splits(ctx, s, 0, n, 0)}"),
    .Some("index") => str.format(ctx, "${indexes(s, 0, n, 0)}"),
    .Some("contains") => str.format(ctx, "${containing(s, 0, n, 0)}"),
    _ => "0",
  };
  io.println(ctx, answer).mapErr(fn(_e) => "stdout")
}
"#;

/// What `answers` prints, worked out here.
fn answers() -> String {
    let mut out = String::new();
    let cut = |s: &str, sep: &str| {
        let parts: Vec<&str> = s.split(sep).collect();
        let joined: String = parts
            .iter()
            .map(|p| format!("[{p}:{}]", p.chars().count()))
            .collect();
        format!("{}:{joined}\n", parts.len())
    };
    for n in 0..41 {
        let s: String = (0..n).map(|i| format!("{i},")).collect();
        out += &cut(&s, ",");
    }
    out += &cut("a--b-c---d--", "--");
    out += &cut("x", "xy");
    out += &cut("", ",");
    for s in [
        "-a-",
        "--------x---------",
        "abcdefg-ijklmno-qrstuvw-",
        "ä-ö--ü-",
        "no separator here, at all",
    ] {
        out += &cut(s, "-");
    }
    let long = "abcdefghijklmnopqrst";
    for (s, needle) in [
        ("a9 9a 99", "99"),
        ("a9 9a 9", "99"),
        ("abcdefghijklmnopqrsX abcdefghijklmnopqrst", long),
        ("abcdefghijklmnopqrsX abcdefghijklmnopqrsT", long),
        ("abcdefghijklmnopq", "abcdefghijklmnopq"),
        ("abcdefghijklmnopQ abcdefghijklmnopq.", "abcdefghijklmnopq."),
        ("ä-ö-ü", "ö"),
        ("ä-ö-ü", "-"),
    ] {
        let at = s.find(needle).map_or(-1, |i| s[..i].chars().count() as i64);
        out += &format!("{at} {}\n", at >= 0);
    }
    out
}

/// **A split cuts every piece, at any count and separator length, with its
/// length in scalars, and a search finds the first whole match past every
/// near one.** Up to forty pieces, past the sixteen a split keeps on the
/// stack; a one-byte separator first, last, repeated, on either side of an
/// eight-byte boundary, between non-ASCII scalars and absent; a separator
/// whose first byte recurs; a separator longer than the string; and needles
/// of 2, 17, 18 and 20 bytes whose first bytes match early and whose ends
/// don't, and a search past non-ASCII scalars. Under the heap check.
#[test]
fn a_split_cuts_every_piece_and_a_search_finds_the_first_match() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("split-text-answers", SPLIT);
        let ran = ran_command(heap_checked(
            std::process::Command::new(&binary).args(["answers", "0"]),
        ));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, format!("{}\n", answers()), "{backend}");
    }
}

/// **A split and a search compare a short needle without a call.** Each
/// compared the needle's tail through slice equality, a call to `bcmp`, and a
/// split zeroed sixteen stack slots for its pieces before cutting three.
///
/// In instructions a step on the copy-and-patch backend: a split of
/// `item-919293-949596` on `-` went from 668 to 608, `indexOf("-")` from 146
/// to 131 and `contains("99")`, past six `9`s, from 392 to 295.
///
/// **A split records each piece without a call.** Saving a piece's range was
/// a call that spilled four registers to write two words, three times a
/// split of three pieces. Only the seventeenth piece on goes out of line. A
/// split went from 608 to 561.
///
/// **A split on one byte writes each piece straight into the list.** It
/// counts the separators first, so the list is allocated at its size, then
/// writes each piece as it finds it, with no ranges kept on the stack. An
/// ASCII `indexOf` saves no registers for counting scalars. A split went from
/// 561 to 445, and `indexOf("-")` from 131 to 120.
///
/// Each bound sits about halfway. Only the copy-and-patch backend is bounded,
/// as in `growing_steps.rs`.
#[test]
fn a_split_and_a_search_compare_a_short_needle_without_a_call() {
    const STEPS: u64 = 200_000;
    let steps = STEPS.to_string();
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("split-text-costs", SPLIT);
        for (mode, each, bound) in [("split", 3, 503), ("index", 4, 126), ("contains", 2, 343)] {
            let want = format!("{}\n", each * STEPS);
            let (n, _) = per_op(backend, &binary, mode, &[&steps], STEPS, &want);
            eprintln!("{backend}, {mode}: {n:?} instructions a step");
            if let Some(n) = n.filter(|&n| backend == "stencil" && n > bound) {
                failures.push(format!(
                    "{backend} {mode}: {n} instructions a step, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
