//! `str.toInt`, natively, on every native backend built in: the number for
//! text with spaces, signs and characters that aren't ASCII, and what reading
//! one costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// The texts [`ANSWERS`] reads, in order.
const TEXTS: &[&str] = &[
    "42",
    "  -17  ",
    "+8",
    "\t\n\u{b}\u{c}\r 9 \r\n",
    "\u{a0}12\u{3000}",
    "\u{feff}5",
    "7\u{2028}",
    "\u{85}3",
    "",
    "   ",
    "+",
    "-",
    "--1",
    "1 2",
    "12a",
    "a12",
    "１２",
    "1é",
    "é1",
    "9223372036854775807",
    "-9223372036854775808",
    "9223372036854775808",
    "00000000000000000000000000000001",
    "999999999999999999",
    "-999999999999999999",
    "+000000000000000007",
    "1000000000000000000",
    "12345678901234567x",
    "-0",
];

/// Prints what `toInt` makes of each of [`TEXTS`], a line each.
fn answers_source() -> String {
    let reads: String = TEXTS
        .iter()
        .map(|text| {
            let escaped: String = text
                .chars()
                .map(|c| format!("\\u{{{:x}}}", c as u32))
                .collect();
            format!("    .concat(ctx, shown(ctx, \"{escaped}\"))\n")
        })
        .collect();
    format!(
        r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import {{ NativeHost }};
from "platform/effect" import {{ Allocator, Stdout }};

fn shown<C: Allocator>(ctx: C, s: Str): Str {{
  match (s.toInt()) {{
    .Some(n) => str.format(ctx, "${{n}}\n"),
    .None => "none\n",
  }}
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};
  let lines = ""
{reads};
  io.print(ctx, lines).mapErr(fn(_e) => "stdout")
}}
"#
    )
}

/// JavaScript's `String.trim` spaces: `WhiteSpace` and `LineTerminator`.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// What `toInt` makes of `text`: the trimmed text when it is an optional
/// sign and ASCII digits whose number fits in 64 bits.
fn read(text: &str) -> Option<i64> {
    let t = text.trim_matches(js_space);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

/// **`toInt` reads an optional sign and digits, between any spaces.** ASCII
/// and other spaces either side, signs, letters and digits that aren't ASCII,
/// and the ends of the 64-bit range. Under the heap check.
#[test]
fn to_int_reads_a_sign_and_digits_between_any_spaces() {
    let expected: String = TEXTS
        .iter()
        .map(|text| match read(text) {
            Some(n) => format!("{n}\n"),
            None => "none\n".to_string(),
        })
        .collect();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("int-answers", &answers_source());
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, expected, "{backend}");
    }
}

/// `main` takes a count `n`, asks `toInt` of a text `n` times, and prints the
/// sum of what it read, 1 for each text that isn't a number.
const READS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn ints(text: Str, r: Int, n: Int, acc: Int): Int {
  if (r >= n) { acc } else { ints(text, r + 1, n, acc + text.toInt().withDefault(1)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let text = TEXT;
  io.println(ctx, "${ints(text, 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of reads [`READS`] makes.
const READ_COUNTS: (u64, u64) = (10_000, 110_000);

/// **`toInt` stops at the first byte that can't be part of a number.** It
/// checked the whole text as UTF-8 twice, once to read it and once to trim
/// it, before looking at the first character, so a long text that isn't a
/// number cost a read of every byte. Now ASCII spaces, the sign and the
/// digits are read as bytes, and the text is decoded only where a byte that
/// isn't ASCII sits next to the spaces.
///
/// The bounds are on the instructions one read costs, the loop's step
/// included. On macOS stencil, a 4,000-byte text that starts with a letter
/// went from 4,251 to 131, and `"  -12345  "` from 515 to 319. Each bound sits
/// about halfway, which leaves the short read room to grow by a third before
/// it fails and still catches a decode of the long text coming back. The read
/// is the runtime's, so every backend is bounded.
#[test]
fn to_int_stops_at_the_first_byte_that_cannot_be_a_number() {
    let (fewer, more) = READ_COUNTS;
    let reads = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        for (mode, text, each, bound) in [
            ("letters", "\"x\".repeat(ctx, 4000)", 1, LETTERS_BOUND),
            ("number", "\"  -12345  \"", -12_345, NUMBER_BOUND),
        ] {
            let binary = build(
                &format!("int-read-{mode}"),
                &READS.replace("TEXT;", &format!("{text};")),
            );
            let answer = move |n: u64| format!("{}\n", n as i64 * each);
            let n = steps_cost(backend, &binary, READ_COUNTS, answer);
            let per = n.map(|n| n as f64 / reads as f64);
            eprintln!("{backend}, {mode}: {per:?} instructions a read");
            if per.is_some_and(|per| per > bound) {
                failures.push(format!(
                    "{backend} {mode}: {per:?} instructions a read, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// See [`to_int_stops_at_the_first_byte_that_cannot_be_a_number`].
const LETTERS_BOUND: f64 = 2_000.0;
/// See [`to_int_stops_at_the_first_byte_that_cannot_be_a_number`].
const NUMBER_BOUND: f64 = 415.0;

/// **`toInt` sums up to 18 digits as it checks them.** It checked that every
/// byte was a digit, then handed the text to Rust's parser, which checked
/// each byte again and guarded each step against overflow. Eighteen digits
/// can't overflow an `i64`, so they are checked and summed in one pass, and
/// only longer numbers still go to the parser.
///
/// The bounds are on the instructions one read costs, the loop's step
/// included. On macOS:
///
/// | backend, text | before | after | bound |
/// | --- | --- | --- | --- |
/// | stencil, `"  -12345  "` | 311 | 208 | 260 |
/// | stencil, 13 digits | 412 | 247 | 330 |
/// | LLVM, `"  -12345  "` | 280 | 178 | 229 |
/// | LLVM, 13 digits | 381 | 216 | 298 |
///
/// Each bound sits halfway, so the second pass coming back fails it by 19% or
/// more, and the read has the same room to grow. The read is the runtime's,
/// so every backend is bounded.
#[test]
fn to_int_sums_up_to_18_digits_as_it_checks_them() {
    let (fewer, more) = READ_COUNTS;
    let reads = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let llvm = backend == "llvm";
        for (mode, text, each, bound) in [
            (
                "short",
                "\"  -12345  \"",
                -12_345,
                if llvm { 229.0 } else { 260.0 },
            ),
            (
                "long",
                "\"1234567890123\"",
                1_234_567_890_123,
                if llvm { 298.0 } else { 330.0 },
            ),
        ] {
            let binary = build(
                &format!("int-digits-{mode}"),
                &READS.replace("TEXT;", &format!("{text};")),
            );
            let answer = move |n: u64| format!("{}\n", n as i64 * each);
            let n = steps_cost(backend, &binary, READ_COUNTS, answer);
            let per = n.map(|n| n as f64 / reads as f64);
            eprintln!("{backend}, {mode}: {per:?} instructions a read");
            if per.is_some_and(|per| per > bound) {
                failures.push(format!(
                    "{backend} {mode}: {per:?} instructions a read, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
