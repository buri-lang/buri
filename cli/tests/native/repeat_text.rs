//! `str.repeat`, natively, on every native backend built in: the text at
//! every count, and what a byte of it costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{heap_checked, ran_command};

/// Prints each repeat and its length, a line each.
const ANSWERS: &str = r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn shown<C: Allocator>(ctx: C, s: Str, n: Int): Str {
  let r = s.repeat(ctx, n);
  str.format(ctx, "[${r}] ${r.length()}\n")
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let lines = shown(ctx, "ab", 0)
    .concat(ctx, shown(ctx, "ab", -3))
    .concat(ctx, shown(ctx, "", 5))
    .concat(ctx, shown(ctx, "ab", 1))
    .concat(ctx, shown(ctx, "abc", 2))
    .concat(ctx, shown(ctx, "x", 7))
    .concat(ctx, shown(ctx, "é€", 3))
    .concat(ctx, shown(ctx, "0123456789", 13))
    .concat(ctx, shown(ctx, "x", 70).toUpper(ctx));
  io.print(ctx, lines).mapErr(fn(_e) => "stdout")
}
"#;

/// What [`ANSWERS`] prints, worked out by hand.
fn answers() -> String {
    let line = |s: &str, n: usize| {
        let r = s.repeat(n);
        format!("[{r}] {}\n", r.chars().count())
    };
    [
        line("ab", 0),
        line("ab", 0),
        line("", 5),
        line("ab", 1),
        line("abc", 2),
        line("x", 7),
        line("é€", 3),
        line("0123456789", 13),
        line("x", 70).to_uppercase(),
    ]
    .concat()
}

/// **`str.repeat` gives the text `n` times, and none for a count of zero or
/// less.** An empty text, one byte, several, text that isn't ASCII, and a
/// result past 64 bytes. The last is upper-cased, which reads the result as
/// text again. Under the heap check.
#[test]
fn a_repeat_is_the_text_that_many_times() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("repeat-answers", ANSWERS);
        let ran = ran_command(heap_checked(&mut std::process::Command::new(&binary)));
        assert_eq!(ran.status, 0, "{backend}: {}", ran.stderr);
        assert_eq!(ran.stdout, answers(), "{backend}");
    }
}

/// `main` takes a count `n` and repeats `"ab"` 2,000 times, `n` times, and
/// prints the sum of the lengths.
const REPEATS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn repeats<C: Allocator>(ctx: C, r: Int, n: Int, acc: Int): Int {
  if (r >= n) { acc } else { repeats(ctx, r + 1, n, acc + "ab".repeat(ctx, 2000).length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${repeats(ctx, 0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of repeats [`REPEATS`] makes, of 4,000 bytes each.
const REPEAT_COUNTS: (u64, u64) = (100, 1_100);

/// **A repeat copies what it has built after itself, doubling.** It copied
/// the text once a time into a growing buffer, a `memcpy` call for two bytes,
/// then copied the whole buffer again into the string. In instructions a byte
/// on macOS stencil: 17.0 to 0.33. The bound sits about halfway, so it fails
/// if the repeat goes back to a copy a time, and leaves the allocation and the
/// loop room to grow. The repeat is the runtime's, so every backend is bounded.
#[test]
fn a_repeat_copies_what_it_built_after_itself() {
    let (fewer, more) = REPEAT_COUNTS;
    let bytes = (more - fewer) * 4_000;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("repeat-costs", REPEATS);
        let n = steps_cost(backend, &binary, REPEAT_COUNTS, |n| {
            format!("{}\n", n * 4_000)
        });
        let per = n.map(|n| n as f64 / bytes as f64);
        eprintln!("{backend}: {per:?} instructions a byte");
        if n.is_some_and(|n| n > BOUND.saturating_mul(bytes)) {
            failures.push(format!(
                "{backend}: {per:?} instructions a byte, over {BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// See [`a_repeat_copies_what_it_built_after_itself`].
const BOUND: u64 = 8;
