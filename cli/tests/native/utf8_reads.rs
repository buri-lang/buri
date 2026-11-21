//! Text read as UTF-8, natively, on every native backend built in: where
//! `bytes.fromUtf8` stops on bytes that aren't text, and what decoding,
//! `toInt` and `chars` cost a byte, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// Prints what `bytes.fromUtf8` makes of each byte list, a line each: the
/// text, or `err` and the offset where it stopped.
const DECODES: &str = r#"
from "core/bytes" import * as bytes;
from "core/bytes" import { Utf8Error };
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn shown<C: Allocator>(ctx: C, raw: [U8]): Str {
  match (bytes.fromUtf8(ctx, raw)) {
    .Ok(s) => str.format(ctx, "ok ${s} ${s.length()}"),
    .Err(Utf8Error(at)) => str.format(ctx, "err ${at}"),
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let cases: [[U8]] = [
    [104, 105],
    [0xc3, 0xa9, 0xe2, 0x82, 0xac, 0xf0, 0x9f, 0x98, 0x80],
    [97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 0xc3, 0xa9, 113],
    [0xf4, 0x90, 0x80, 0x80],
    [0xf5, 0x80, 0x80, 0x80],
    [0xe0, 0x80, 0x80],
    [0xc3, 0xa9, 0xe2, 0x82],
    [0xe2, 0x28, 0xa1],
    [0xf0, 0x9f, 0x98, 0x80, 0x80],
    [97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 0xff],
    [0xed, 0x9f, 0xbf, 0xed, 0xa0, 0x80],
    [0xf8, 0x88, 0x80, 0x80, 0x80],
  ];
  let lines = cases.foldCtx(ctx, fn(c, acc: Str, raw) => acc.concat(c, shown(c, raw).concat(c, "\n")), "");
  io.print(ctx, lines).mapErr(fn(_e) => "stdout")
}
"#;

/// What [`DECODES`] prints, worked out by hand.
const DECODED: &str = "ok hi 2
ok \u{e9}\u{20ac}\u{1f600} 3
ok abcdefghijklmnop\u{e9}q 18
err 0
err 0
err 0
err 2
err 0
err 4
err 17
err 3
err 0
";

/// **`bytes.fromUtf8` stops where the first sequence that isn't text
/// starts.** Past U+10FFFF, a lead byte no sequence starts with, an
/// overlong form, a sequence cut short, a bad continuation, a stray
/// continuation, a bad byte after a run of ASCII, and a surrogate after a
/// valid character. Under the heap check, which fails on a count the decode
/// took or dropped.
#[test]
fn decoding_stops_where_the_bytes_stop_being_text() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("utf8-decodes", DECODES);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, DECODED, "{backend}");
    }
}

/// `main` takes a count `n` and makes `READ` `n` times: [`DECODE`] or
/// [`INT`].
const READS: &str = r#"
from "core/bytes" import * as bytes;
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn decodes<C: Allocator>(ctx: C, raw: [U8], r: Int, n: Int, acc: Int): Int {
  if (r >= n) { acc } else { decodes(ctx, raw, r + 1, n, acc + bytes.fromUtf8(ctx, raw).map(fn(s) => s.length()).withDefault(0)) }
}

fn ints(text: Str, r: Int, n: Int, acc: Int): Int {
  if (r >= n) { acc } else { ints(text, r + 1, n, acc + text.toInt().withDefault(1)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let text = "x".repeat(ctx, 4000);
  let answer = READ;
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// One 4,000-byte list of ASCII turned into text.
const DECODE: &str = "decodes(ctx, bytes.toUtf8(ctx, text), 0, n, 0)";

/// `toInt` asked of a 4,000-byte string that isn't a number.
const INT: &str = "ints(text, 0, n, 0)";

/// The two counts of reads [`READS`] makes, of 4,000 bytes each.
const READ_COUNTS: (u64, u64) = (100, 1_100);

/// **Text is checked as UTF-8 a word at a time.** `bytes.fromUtf8` decoded
/// a scalar at a time into a fresh string and copied that, and `toInt` read
/// its text through `from_utf8_lossy` twice, which walks every byte. Both now
/// ask the standard validator, which checks ASCII a word at a time. In
/// instructions a byte on macOS stencil: 36.5 to 0.95 for a decode, 12.1 to
/// 1.06 for `toInt`. Each bound sits about halfway. The reads are the
/// runtime's, so every backend is bounded.
#[test]
fn text_is_checked_as_utf8_a_word_at_a_time() {
    let (fewer, more) = READ_COUNTS;
    let read = (more - fewer) * 4_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        for (mode, read_expr, each, bound) in [("decode", DECODE, 4_000, 18), ("int", INT, 1, 6)] {
            let binary = build(
                &format!("utf8-read-{mode}"),
                &READS.replace("READ;", &format!("{read_expr};")),
            );
            let answer = move |n: u64| format!("{}\n", n * each);
            let n = steps_cost(backend, &binary, READ_COUNTS, answer);
            let per = n.map(|n| n as f64 / read as f64);
            eprintln!("{backend}, {mode}: {per:?} instructions a byte");
            if n.is_some_and(|n| n > bound * read) {
                failures.push(format!(
                    "{backend} {mode}: {per:?} instructions a byte, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
