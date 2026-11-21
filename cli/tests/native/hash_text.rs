//! A `Str` hashed, natively, on every native backend built in: the number at
//! every length and for text that isn't ASCII, and what one hash costs,
//! counted rather than timed.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, ran_command};

/// `main` takes the mode and a count. `answers` prints the hashes of text of
/// every length up to 25 bytes and of three that aren't ASCII; `hash` hashes
/// a nine-byte key a step and prints how many hashes were odd.
const HASH: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn digits<C: Allocator>(ctx: C, i: Int, n: Int, acc: Str): Str {
  if (i >= n) { acc } else { digits(ctx, i + 1, n, acc.concat(ctx, str.fromInt(ctx, i % 10))) }
}

fn answers<C: Allocator>(ctx: C): Str {
  let counted = list.range(ctx, 0, 26).foldCtx(ctx, fn(c, acc: Str, n) => acc.concat(c, str.format(c, "${digits(c, 0, n, "").hash()}\n")), "");
  counted
    .concat(ctx, str.format(ctx, "${"héllo wörld".hash()}\n"))
    .concat(ctx, str.format(ctx, "${"a😀b".hash()}\n"))
    .concat(ctx, str.format(ctx, "${"ascii then é".hash()}\n"))
}

fn odd(i: Int): Int {
  let key = if (i % 2 == 0) { "key-12345" } else { "key-54321" };
  if (key.hash() % 2 == 1) { 1 } else { 0 }
}

fn hashes(i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { hashes(i + 1, n, acc + odd(i)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let answer = match (args.first()) {
    .Some("answers") => answers(ctx),
    .Some("hash") => str.format(ctx, "${hashes(0, n, 0)}"),
    _ => "0",
  };
  io.println(ctx, answer).mapErr(fn(_e) => "stdout")
}
"#;

/// FNV-1a over the text's UTF-16 code units, which is what JavaScript's
/// `$hashInto` mixes.
fn fnv(s: &str) -> u64 {
    let h = s.encode_utf16().fold(0x811c_9dc5u32, |h, unit| {
        (h ^ u32::from(unit)).wrapping_mul(0x0100_0193)
    });
    u64::from(h)
}

/// What `answers` prints, worked out here.
fn answers() -> String {
    let mut out = String::new();
    for n in 0..26u8 {
        let s: String = (0..n).map(|i| char::from(b'0' + i % 10)).collect();
        out += &format!("{}\n", fnv(&s));
    }
    for s in ["héllo wörld", "a😀b", "ascii then é"] {
        out += &format!("{}\n", fnv(s));
    }
    out
}

/// **A `Str` hashes to JavaScript's number at every length.** Every length up
/// to 25 bytes, on both sides of each eight-byte word, and text with two-byte
/// scalars and an astral one, whose halves mix one at a time. Under the heap
/// check.
#[test]
fn a_str_hashes_to_the_same_number_at_every_length() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("hash-text-answers", HASH);
        let ran = ran_command(heap_checked(
            std::process::Command::new(&binary).args(["answers", "0"]),
        ));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, format!("{}\n", answers()), "{backend}");
    }
}

/// How many of `steps` hashes `hashes` finds odd.
fn odd_count(steps: u64) -> u64 {
    let odd = |s: &str| u64::from(fnv(s) % 2 == 1);
    steps.div_ceil(2) * odd("key-12345") + steps / 2 * odd("key-54321")
}

/// **An ASCII `Str` hashes eight bytes a loop step, with no frame.** A hash
/// mixed one byte a loop step, and saved two register pairs for the UTF-16
/// path even on ASCII text. That path is out of line now.
///
/// In instructions a step on the copy-and-patch backend, a nine-byte key
/// went from 114 to 99. The bound sits about halfway. Only the
/// copy-and-patch backend is bounded, as in `growing_steps.rs`.
#[test]
fn an_ascii_str_hashes_eight_bytes_a_loop_step() {
    const STEPS: u64 = 200_000;
    const BOUND: u64 = 107;
    let steps = STEPS.to_string();
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("hash-text-costs", HASH);
        let want = format!("{}\n", odd_count(STEPS));
        let (n, _) = per_op(backend, &binary, "hash", &[&steps], STEPS, &want);
        eprintln!("{backend}, hash: {n:?} instructions a step");
        if let Some(n) = n.filter(|&n| backend == "stencil" && n > BOUND) {
            failures.push(format!(
                "{backend} hash: {n} instructions a step, over {BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
