//! What building and reading a `core/map` costs against which bits its `Int`
//! and `F64` keys differ in, natively, on every native backend built in.
//!
//! The work is counted rather than timed: instructions retired where the kernel
//! counts them (macOS on hardware), each run against one that does less, so the
//! difference is the work alone. Every run also goes once under the heap check.
//!
//! The count includes starting the process, which on a busy machine costs 1–2 M
//! more instructions one run in ten. So each run does enough work, [`INSERTS`]
//! inserts and [`LOOKUPS`] lookups, that 4 M more moves a figure a few percent.
//! Where nothing is counted, the hash values themselves are what
//! `agreement::wide_integers_hash_every_bit` and
//! `agreement::floats_hash_every_bit` pin.

use crate::hang::CappedChild;
use crate::shared::{exited_instructions, heap_checked, ran_command, Ran};
use std::path::Path;

/// The lookups a `get` run does.
const LOOKUPS: u64 = 500_000;

/// The inserts a `build` run does, over as many maps as it takes.
const INSERTS: u64 = 64_000;

/// `main <mode> <size> <rounds> <step>` builds a map of `size` keys `i * step`,
/// `rounds` times. `build` answers the sizes' sum, `get` also reads the last
/// map [`LOOKUPS`] times, and `none` does neither.
const PROGRAM: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/map" import * as map;
from "core/map" import { Map };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn built<C: Allocator>(ctx: C, n: Int, step: Int): Map<Int, Int> {
  list.range(ctx, 0, n).foldCtx(ctx, fn(c, acc: Map<Int, Int>, i) => acc.insert(c, i * step, i), map.empty())
}

fn rebuilt<C: Allocator>(ctx: C, n: Int, step: Int, rounds: Int, acc: Int): Int {
  if (rounds <= 0) { acc } else { rebuilt(ctx, n, step, rounds - 1, acc + built(ctx, n, step).size) }
}

fn gets(m: Map<Int, Int>, step: Int, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { gets(m, step, i + 1, n, acc + m.get(i * 7 % m.size * step).withDefault(0)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let rounds = args.get(2).andThen(fn(s) => s.toInt()).withDefault(1);
  let step = args.get(3).andThen(fn(s) => s.toInt()).withDefault(1);
  let answer = match (args.first()) {
    .Some("build") => rebuilt(ctx, size, step, rounds, 0),
    .Some("get") => rebuilt(ctx, size, step, rounds - 1, 0) + gets(built(ctx, size, step), step, 0, LOOKUPS, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// [`PROGRAM`] over `F64` keys `start + i * scale`, with `scale` and `start`
/// the arguments after `rounds`.
const FLOATS: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/map" import * as map;
from "core/map" import { Map };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn built<C: Allocator>(ctx: C, n: Int, scale: F64, start: F64): Map<F64, Int> {
  list.range(ctx, 0, n).foldCtx(ctx, fn(c, acc: Map<F64, Int>, i) => acc.insert(c, start + i.toF64() * scale, i), map.empty())
}

fn rebuilt<C: Allocator>(ctx: C, n: Int, scale: F64, start: F64, rounds: Int, acc: Int): Int {
  if (rounds <= 0) { acc } else { rebuilt(ctx, n, scale, start, rounds - 1, acc + built(ctx, n, scale, start).size) }
}

fn gets(m: Map<F64, Int>, scale: F64, start: F64, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { gets(m, scale, start, i + 1, n, acc + m.get(start + (i * 7 % m.size).toF64() * scale).withDefault(0)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let rounds = args.get(2).andThen(fn(s) => s.toInt()).withDefault(1);
  let scale = args.get(3).andThen(fn(s) => s.toFloat()).withDefault(1.0);
  let start = args.get(4).andThen(fn(s) => s.toFloat()).withDefault(0.0);
  let answer = match (args.first()) {
    .Some("build") => rebuilt(ctx, size, scale, start, rounds, 0),
    .Some("get") => rebuilt(ctx, size, scale, start, rounds - 1, 0) + gets(built(ctx, size, scale, start), scale, start, 0, LOOKUPS, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// The fewer instructions of two runs, since a short process's count only
/// gains noise (`design/PERFORMANCE.md` §8). `None` where the kernel counts
/// nothing.
fn measured(backend: &str, binary: &Path, args: &[&str], expected: &str) -> Option<u64> {
    let once = || {
        let child = std::process::Command::new(binary)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let instructions = exited_instructions(child.id());
        let out = child.capped_wait_with_output().unwrap();
        let ran = Ran {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        };
        (ran, instructions)
    };
    let (ran, first) = once();
    let (_, second) = once();
    let checked = ran_command(heap_checked(std::process::Command::new(binary).args(args)));
    for r in [&ran, &checked] {
        assert_eq!(r.status, 0, "{backend} {args:?}: {}", r.stderr);
        assert_eq!(r.stdout, expected, "{backend} {args:?}: {}", r.stderr);
    }
    first.zip(second).map(|(a, b)| a.min(b)).filter(|&n| n > 0)
}

/// Instructions per insert and per lookup, with `keys` the arguments after the
/// rounds.
fn costs(backend: &str, binary: &Path, size: u64, keys: &[&str]) -> Option<(u64, u64)> {
    let rounds = INSERTS / size;
    let (n, r) = (size.to_string(), rounds.to_string());
    let run = |mode: &str, expected: &str| {
        let args: Vec<&str> = [mode, n.as_str(), r.as_str()]
            .into_iter()
            .chain(keys.iter().copied())
            .collect();
        measured(backend, binary, &args, expected)
    };
    let idle = run("none", "0\n");
    let built = run("build", &format!("{}\n", size * rounds));
    let got: u64 = (0..LOOKUPS).map(|i| i * 7 % size).sum::<u64>() + size * (rounds - 1);
    let read = run("get", &format!("{got}\n"));
    let (idle, built, read) = (idle?, built?, read?);
    Some((
        built.saturating_sub(idle) / (size * rounds),
        read.saturating_sub(built) / LOOKUPS,
    ))
}

/// **Keys that differ only above bit 32 cost what keys that differ in their
/// low bits cost** (buri-lang/buri#273). `Int`'s hash mixed only its low 32
/// bits, so `i * 2^32` all hashed alike and shared one collision list, which
/// every insert walked and every lookup scanned.
#[test]
fn a_map_of_keys_that_differ_only_above_bit_32_costs_what_any_other_does() {
    const HIGH: &str = "4294967296";
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build(
            "map-keys",
            &PROGRAM.replace("LOOKUPS", &LOOKUPS.to_string()),
        );
        let small = costs(backend, &binary, 1000, &[HIGH]);
        let low = costs(backend, &binary, 8000, &["1"]);
        let high = costs(backend, &binary, 8000, &[HIGH]);
        eprintln!("{backend}: (insert, lookup) instructions, low {low:?}, high {high:?}, high at 1,000 {small:?}");
        let (Some(small), Some(low), Some(high)) = (small, low, high) else {
            continue;
        };
        let mut over = |what: &str, got: u64, against: u64| {
            if got * 4 > against * 5 {
                failures.push(format!(
                    "{backend}: {what} is {got} instructions against {against}"
                ));
            }
        };
        over("an insert of a key above bit 32", high.0, low.0);
        over("a lookup of a key above bit 32", high.1, low.1);
        // Linear growth, with room for a deeper trie. One collision list grew
        // 7.6x.
        if high.0 > small.0 * 3 {
            failures.push(format!(
                "{backend}: an insert into 8,000 keys above bit 32 is {} instructions against {} into 1,000",
                high.0, small.0
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// **Fractional `F64` keys, and ones that agree mod `2^32`, cost what keys
/// `0.0, 1.0, 2.0, ...` cost** (follows buri-lang/buri#273). A float's hash
/// mixed `ToUint32(trunc(x))`, so every key in `(0, 1)` hashed as `0`, and so
/// did every multiple of `2^32`.
///
/// Millisecond timestamps vary only in their middle bits, so they catch a hash
/// that mixes whole words, whose low bits see only the words' low bits.
#[test]
fn a_map_of_fractional_float_keys_costs_what_whole_ones_do() {
    // `i / 16,000`, `i * 2^32`, and a second apart from 2023-11-14.
    const FRACTION: &[&str] = &["0.0000625"];
    const HIGH: &[&str] = &["4294967296"];
    const STAMPS: &[&str] = &["1000", "1700000000000"];
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build(
            "map-float-keys",
            &FLOATS.replace("LOOKUPS", &LOOKUPS.to_string()),
        );
        let whole = costs(backend, &binary, 8000, &["1"]);
        let small = costs(backend, &binary, 1000, FRACTION);
        let fraction = costs(backend, &binary, 8000, FRACTION);
        let high = costs(backend, &binary, 8000, HIGH);
        let stamps = costs(backend, &binary, 8000, STAMPS);
        eprintln!(
            "{backend}: (insert, lookup) instructions, whole {whole:?}, fraction {fraction:?}, \
             above 2^32 {high:?}, timestamps {stamps:?}, fraction at 1,000 {small:?}"
        );
        let (Some(whole), Some(small), Some(fraction), Some(high), Some(stamps)) =
            (whole, small, fraction, high, stamps)
        else {
            continue;
        };
        let mut over = |what: &str, got: u64, against: u64| {
            if got * 4 > against * 5 {
                failures.push(format!(
                    "{backend}: {what} is {got} instructions against {against}"
                ));
            }
        };
        over("an insert of a fractional key", fraction.0, whole.0);
        over("a lookup of a fractional key", fraction.1, whole.1);
        over("an insert of a key above 2^32", high.0, whole.0);
        over("a lookup of a key above 2^32", high.1, whole.1);
        over("an insert of a timestamp", stamps.0, whole.0);
        over("a lookup of a timestamp", stamps.1, whole.1);
        // Linear growth, as for `Int` keys: one collision list grew 7.6x.
        if fraction.0 > small.0 * 3 {
            failures.push(format!(
                "{backend}: an insert into 8,000 fractional keys is {} instructions against {} into 1,000",
                fraction.0, small.0
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
