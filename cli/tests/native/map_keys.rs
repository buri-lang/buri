//! What building and reading a `core/map` costs against which bits its `Int`
//! keys differ in, natively, on every native backend built in.
//!
//! The work is counted rather than timed: instructions retired where the kernel
//! counts them (macOS on hardware), each run against one that does less, so the
//! difference is the work alone. Every run also goes once under the heap check.
//! Where nothing is counted, the hash values themselves are what
//! `agreement::wide_integers_hash_every_bit` pins.

use crate::shared::{exited_instructions, heap_checked, ran_command, Ran};
use std::path::Path;

const LOOKUPS: u64 = 100_000;

/// `main <mode> <size> <step>` builds a map of `size` keys `i * step`. `build`
/// answers its size, `get` also reads it [`LOOKUPS`] times, and `none` does
/// neither.
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

fn gets(m: Map<Int, Int>, step: Int, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { gets(m, step, i + 1, n, acc + m.get(i * 7 % m.size * step).withDefault(0)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(1);
  let step = args.get(2).andThen(fn(s) => s.toInt()).withDefault(1);
  let answer = match (args.first()) {
    .Some("build") => built(ctx, size, step).size,
    .Some("get") => gets(built(ctx, size, step), step, 0, LOOKUPS, 0),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// The fewest instructions of five runs, since a short process's count only
/// gains noise. `None` where the kernel counts nothing.
fn measured(backend: &str, binary: &Path, args: &[&str], expected: &str) -> Option<u64> {
    let once = || {
        let child = std::process::Command::new(binary)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let instructions = exited_instructions(child.id());
        let out = child.wait_with_output().unwrap();
        let ran = Ran {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        };
        (ran, instructions)
    };
    let (ran, first) = once();
    let rest: Vec<Option<u64>> = (0..4).map(|_| once().1).collect();
    let checked = ran_command(heap_checked(std::process::Command::new(binary).args(args)));
    for r in [&ran, &checked] {
        assert_eq!(r.status, 0, "{backend} {args:?}: {}", r.stderr);
        assert_eq!(r.stdout, expected, "{backend} {args:?}: {}", r.stderr);
    }
    rest.into_iter().try_fold(first?, |a, b| b.map(|b| a.min(b))).filter(|&n| n > 0)
}

/// Instructions per insert and per lookup.
fn costs(backend: &str, binary: &Path, size: u64, step: u64) -> Option<(u64, u64)> {
    let (n, s) = (size.to_string(), step.to_string());
    let idle = measured(backend, binary, &["none", &n, &s], "0\n");
    let built = measured(backend, binary, &["build", &n, &s], &format!("{size}\n"));
    let got: u64 = (0..LOOKUPS).map(|i| i * 7 % size).sum();
    let read = measured(backend, binary, &["get", &n, &s], &format!("{got}\n"));
    let (idle, built, read) = (idle?, built?, read?);
    Some((built.saturating_sub(idle) / size, read.saturating_sub(built) / LOOKUPS))
}

/// **Keys that differ only above bit 32 cost what keys that differ in their
/// low bits cost** (buri-lang/buri#273). `Int`'s hash mixed only its low 32
/// bits, so `i * 2^32` all hashed alike and shared one collision list, which
/// every insert walked and every lookup scanned.
#[test]
fn a_map_of_keys_that_differ_only_above_bit_32_costs_what_any_other_does() {
    const HIGH: u64 = 1 << 32;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("map-keys", &PROGRAM.replace("LOOKUPS", &LOOKUPS.to_string()));
        let small = costs(backend, &binary, 1000, HIGH);
        let low = costs(backend, &binary, 8000, 1);
        let high = costs(backend, &binary, 8000, HIGH);
        eprintln!("{backend}: (insert, lookup) instructions, low {low:?}, high {high:?}, high at 1,000 {small:?}");
        let (Some(small), Some(low), Some(high)) = (small, low, high) else { continue };
        let mut over = |what: &str, got: u64, against: u64| {
            if got * 4 > against * 5 {
                failures.push(format!("{backend}: {what} is {got} instructions against {against}"));
            }
        };
        over("an insert of a key above bit 32", high.0, low.0);
        over("a lookup of a key above bit 32", high.1, low.1);
        // Linear growth, with room for a deeper trie and the noise a short run
        // of 1,000 inserts carries (up to 2x measured). One collision list grew
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
