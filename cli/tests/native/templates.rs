//! Interpolations of three parts or more, natively, on every native backend
//! built in: what the program prints, how many blocks a template takes, and
//! what a step costs, counted rather than timed.

use crate::collection_costs::steps_cost;
use crate::shared::{probed, ran_command};

/// `k` steps, step `i` interpolating one of three words twice between three
/// literals, and adding the result's length. `main` takes `k` and prints the
/// sum. The words are `Str` holes, so the template renders nothing of its own
/// and every block a step takes is a join's.
const WORDS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, i: Int, n: Int, words: [Str], acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let w = words.get(i % 3).withDefault("");
    let s = str.format(ctx, "<${w}|${w}>");
    rounds(ctx, i + 1, n, words, acc + s.length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(ctx, 0, k, ["ab", "été", "x𝄞"], 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`WORDS`] prints for `k` steps: each step's length in scalars, three
/// literals and its word twice, summed.
fn words_summed(k: u64) -> String {
    let sum: u64 = (0..k).map(|i| 3 + 2 * [2, 3, 2][(i % 3) as usize]).sum();
    format!("{sum}\n")
}

/// **A template allocates one block, however many parts it joins.** Every
/// part is rendered before the first join, so that join takes a block with
/// room for the parts still to come, and the later joins write into it. The
/// first join used to take a block of its own two parts and the second a
/// bigger one, so a step allocated two.
#[test]
fn a_template_of_five_parts_allocates_one_block() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("templates-words", WORDS);
        let blocks = |k: u64| {
            let r = ran_command(std::process::Command::new(&binary).arg(k.to_string()));
            assert_eq!(r.status, 0, "{backend} {k}: {}", r.stderr);
            assert_eq!(r.stdout, words_summed(k), "{backend} {k}: {}", r.stderr);
            probed(&r.stderr).0
        };
        // Both sums are past 9999, so printing each takes one block of its own.
        let (fewer, more) = (2_000, 4_000);
        let extra = blocks(more) - blocks(fewer);
        eprintln!("{backend}: {extra} blocks over {} steps", more - fewer);
        if extra != more - fewer {
            failures.push(format!(
                "{backend}: {extra} blocks over {} steps",
                more - fewer
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A template's text is its parts' text in order, whichever part is the first
/// to own a block: a literal, a borrowed `Str`, a rendered number, or an empty
/// part. Non-ASCII parts keep the answer's length in scalars.
#[test]
fn a_template_joins_its_parts_in_order() {
    let source = r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn show<C: Allocator + Stdout>(ctx: C, s: Str): Result<(), Str> {
  io.println(ctx, "${s} ${s.length()}").mapErr(fn(_e) => "stdout")
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let w = "été".concat(ctx, "!");
  let n = 123456;
  let e = "";
  let _ = show(ctx, str.format(ctx, "a-${n}-${w}-z"))?;
  let _ = show(ctx, str.format(ctx, "${w}:${n}:${w}"))?;
  let _ = show(ctx, str.format(ctx, "${n}${e}${n}${e}"))?;
  let _ = show(ctx, str.format(ctx, "${e}${e}${w}"))?;
  let _ = show(ctx, str.format(ctx, "𝄞${e}${e}𝄞"))?;
  let long = "0123456789".concat(ctx, "0123456789").concat(ctx, "0123456789");
  let _ = show(ctx, str.format(ctx, "${long}${long}${long}${long}${long}-${n}"))?;
  show(ctx, str.format(ctx, "[${long}]${long}"))
}
"#;
    let long = "012345678901234567890123456789";
    let want = format!(
        "a-123456-été!-z 15\nété!:123456:été! 16\n123456123456 12\nété! 4\n𝄞𝄞 2\n\
         {long}{long}{long}{long}{long}-123456 157\n[{long}]{long} 62\n"
    );
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("templates-parts", source);
        let r = ran_command(&mut std::process::Command::new(&binary));
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(r.stdout, want, "{backend}: {}", r.stderr);
        assert_eq!(
            probed(&r.stderr).1,
            0,
            "{backend}: blocks still live at exit"
        );
    }
}

/// `k` steps, step `i` interpolating two numbers of six digits between
/// literals, the strings bench's own template, and adding the length.
const NUMBERS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let j = 100000 + i % 300000;
    rounds(ctx, i + 1, n, acc + str.format(ctx, "item-${j}-${j * 2}").length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${rounds(ctx, 0, k, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// What [`NUMBERS`] prints for `k` steps.
fn numbers_summed(k: u64) -> String {
    let sum: u64 = (0..k)
        .map(|i| {
            let j = 100_000 + i % 300_000;
            format!("item-{j}-{}", j * 2).len() as u64
        })
        .sum();
    format!("{sum}\n")
}

/// The two counts of steps [`NUMBERS_BOUND`] is measured between, wide enough
/// that the difference runs to hundreds of millions of instructions.
const NUMBERS_STEPS: (u64, u64) = (100_000, 1_100_000);

/// What one step of [`NUMBERS`] costs on the copy-and-patch backend, in
/// instructions: halfway between [`NUMBERS_BEFORE`] and [`NUMBERS_AFTER`].
const NUMBERS_BOUND: f64 = NUMBERS_BEFORE - (NUMBERS_BEFORE - NUMBERS_AFTER) / 2.0;

/// [`NUMBERS`]'s step before, in instructions.
const NUMBERS_BEFORE: f64 = 601.1;

/// [`NUMBERS`]'s step after, in instructions.
const NUMBERS_AFTER: f64 = 575.1;

/// **The joins after a template's first write into its block, and its numbers
/// take no block of their own.** A step measured 875.2 instructions when the
/// second join allocated a doubled block and freed the first join's, and 736.2
/// once the first join had room for the whole template. Each number then still
/// took a block, freed after the joins; rendered into a digit slot the joins
/// copy, a step measured 601.1 (Apple M-series, lowest of three; LLVM went
/// from 571.2 to 448.1). Written right-aligned in its slot, a number's start
/// says how long it is, so nothing counts its digits first: 575.1. The bound
/// sits halfway, so either side of it is 13 instructions a step, 13 million
/// over [`NUMBERS_STEPS`], where two runs differ by well under a million.
#[test]
fn a_template_of_numbers_joins_without_a_second_block() {
    let (fewer, more) = NUMBERS_STEPS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("templates-numbers", NUMBERS);
        let n = steps_cost(backend, &binary, NUMBERS_STEPS, numbers_summed);
        let each = n.map(|n| n as f64 / steps as f64);
        eprintln!("{backend}: {each:?} instructions a step");
        if backend == "stencil" && each.is_some_and(|e| e > NUMBERS_BOUND) {
            failures.push(format!(
                "{backend}: {each:?} instructions a step, over {NUMBERS_BOUND}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// **A number a template renders takes no block of its own.** Its digits are
/// copied into the template's one block, so a [`NUMBERS`] step, two numbers
/// past 9999 between literals, allocates exactly that block. Each number took
/// a block, freed after the join, so a step allocated three.
#[test]
fn a_template_of_numbers_allocates_one_block() {
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("templates-numbers-blocks", NUMBERS);
        let blocks = |k: u64| {
            let r = ran_command(std::process::Command::new(&binary).arg(k.to_string()));
            assert_eq!(r.status, 0, "{backend} {k}: {}", r.stderr);
            assert_eq!(r.stdout, numbers_summed(k), "{backend} {k}: {}", r.stderr);
            probed(&r.stderr).0
        };
        let (fewer, more) = (2_000, 4_000);
        let extra = blocks(more) - blocks(fewer);
        eprintln!("{backend}: {extra} blocks over {} steps", more - fewer);
        if extra != more - fewer {
            failures.push(format!(
                "{backend}: {extra} blocks over {} steps",
                more - fewer
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A template's numbers are its own, whatever its holes run before the join:
/// a hole that calls a function rendering numbers in templates of its own,
/// a string kept from one template while the next renders, the widest and
/// narrowest integers, every part a number, and numbers beside empty parts.
#[test]
fn a_template_keeps_its_numbers_while_its_holes_render_others() {
    let source = r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

fn show<C: Allocator + Stdout>(ctx: C, s: Str): Result<(), Str> {
  io.println(ctx, "${s} ${s.length()}").mapErr(fn(_e) => "stdout")
}

fn inner<C: Allocator>(ctx: C, n: Int): Str {
  str.format(ctx, "<${n}|${n * 7}>")
}

fn width<C: Allocator>(ctx: C, n: Int): Int {
  str.format(ctx, "${n}-${n}-${n}").length()
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = 123456;
  let b = -98765432;
  let e = "";
  let lo = -9223372036854775807 - 1;
  let hi = 9223372036854775807;
  let x: I8 = -5;
  let u: U32 = 4294967295;
  let _ = show(ctx, str.format(ctx, "${a}-${inner(ctx, b)}-${a}"))?;
  let _ = show(ctx, str.format(ctx, "${a}:${width(ctx, 99999999)}:${b}"))?;
  let _ = show(ctx, str.format(ctx, "[${lo}][${hi}]"))?;
  let _ = show(ctx, str.format(ctx, "${x}/${u}/${a % 10}/${9999 + a - a}/${10000 + a - a}"))?;
  let _ = show(ctx, str.format(ctx, "${a}${b}${a}${b}${a}${b}${a}${b}${a}${b}${a}${b}${a}${b}${a}${b}"))?;
  let _ = show(ctx, str.format(ctx, "${e}${a}"))?;
  let _ = show(ctx, str.format(ctx, "${a}${e}"))?;
  let _ = show(ctx, str.format(ctx, "été${b}"))?;
  let kept = str.format(ctx, "${a}+${b}");
  let other = str.format(ctx, "${b}+${a}");
  let _ = show(ctx, kept)?;
  show(ctx, other)
}
"#;
    let ab = "123456-98765432".repeat(8);
    let want = format!(
        "123456-<-98765432|-691358024>-123456 36\n\
         123456:26:-98765432 19\n\
         [-9223372036854775808][9223372036854775807] 43\n\
         -5/4294967295/6/9999/10000 26\n\
         {ab} {}\n\
         123456 6\n\
         123456 6\n\
         été-98765432 12\n\
         123456+-98765432 16\n\
         -98765432+123456 16\n",
        ab.len()
    );
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("templates-own-numbers", source);
        let r = ran_command(&mut std::process::Command::new(&binary));
        assert_eq!(r.status, 0, "{backend}: {}", r.stderr);
        assert_eq!(r.stdout, want, "{backend}: {}", r.stderr);
        assert_eq!(
            probed(&r.stderr).1,
            0,
            "{backend}: blocks still live at exit"
        );
    }
}
