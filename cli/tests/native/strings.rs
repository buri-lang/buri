//! Scalar indices into long strings, natively, on every native backend this
//! toolchain has.
//!
//! `core/str` indexes in Unicode scalars and a native `Str` is UTF-8, so
//! `slice`, `charAt` and `indexOf` turn a scalar index into a byte offset. On a
//! long non-ASCII string the runtime answers that from a kept table rather than
//! from a walk over the string (`cli/runtime/scalars.rs`). The answers are
//! pinned in the conformance corpus (`lib/data/test/strings.buri`, "Scalar
//! indices into long strings"), which `native::conformance` runs on the
//! copy-and-patch backend. This file holds what that corpus cannot say:
//!
//! * **the work is linear**, counted rather than timed, through the runtime's
//!   `buri_rt_str_scanned_bytes`; and
//! * **the answers hold on the release backend too**, whose `str.concat` is
//!   open-coded rather than a runtime call and so drops a block's table from a
//!   call site of its own.
//!
//! Each row runs under the heap check, so a table that kept a block alive, or a
//! block freed under one, fails the status.

use crate::shared::{probed, ran_checked, Ran, ALLOC_PROBE};
use std::path::PathBuf;

/// [`ALLOC_PROBE`], plus a line with the bytes the runtime read to turn scalar
/// indices into byte offsets over the whole run.
fn scan_probe() -> String {
    format!(
        "{ALLOC_PROBE}
extern uint64_t buri_rt_str_scanned_bytes(void);
__attribute__((destructor)) static void buri_scan_probe(void) {{
  fprintf(stderr, \"scanned=%llu\\n\", (unsigned long long)buri_rt_str_scanned_bytes());
}}
"
    )
}

/// The `scanned=` line a [`scan_probe`]-linked run printed.
fn scanned(stderr: &str) -> u64 {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("scanned="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"))
        .trim()
        .parse()
        .unwrap()
}

/// A backend's name, and how it builds `(name, source, probe)` into a binary.
type Backend = (&'static str, fn(&str, &str, &str) -> PathBuf);

/// Every native backend built into this toolchain that can run here.
fn backends() -> Vec<Backend> {
    let mut out: Vec<Backend> = Vec::new();
    #[cfg(feature = "backend-stencil")]
    if crate::stencil::supported() {
        out.push(("stencil", |name, source, probe| {
            crate::stencil::build_with(&format!("{name}-stencil"), source, Some(probe))
        }));
    }
    #[cfg(feature = "backend-llvm")]
    if crate::llvm::can_execute().is_none_or(|why| !crate::ci::skipped("llvm", why)) {
        out.push(("llvm", |name, source, probe| {
            crate::llvm::build_at(
                &format!("{name}-llvm"),
                source,
                Some(probe),
                buri::compiler::backend::Profile::Release,
            )
        }));
    }
    out
}

/// One program on every backend, each run under the heap check.
fn run_everywhere(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    let probe = scan_probe();
    backends()
        .into_iter()
        .map(|(backend, build)| (backend, ran_checked(&build(name, source, &probe))))
        .collect()
}

/// A lexer's loop: a long string with one non-ASCII scalar in it, sliced once
/// per scalar from the front. Walking to each index from the start of the view
/// reads `n² / 2` bytes, which for `n` = 20,000 is two hundred million. The
/// index reads the string once to build and under two runs of 64 bytes per
/// question after that.
#[test]
fn slicing_a_long_string_once_per_scalar_reads_it_a_bounded_number_of_times() {
    let source = r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

fn scan(text: Str, i: Int, n: Int, seen: Int): Int {
  if (i >= n) {
    seen
  } else {
    let one = text.slice(i, i + 1);
    let found = match (text.charAt(i)) { .Some(c) => if (c == 'é') { 1 } else { 0 }, .None => 0 };
    scan(text, i + 1, n, seen + one.length() + found)
  }
}

export fn main(): Result<(), Str> {
  let text = "é".concat(alloc, "abcdefghij".repeat(alloc, 2000));
  let n = text.length();
  let seen = scan(text, 0, n, 0);
  let tail = match (text.indexOf("jab")) { .Some(at) => at, .None => -1 };
  let _ = io.println(stdout, "${n} ${seen} ${tail}").ignore();
  .Ok(())
}
"#;
    let runs = run_everywhere("scan-per-scalar", source);
    for (backend, r) in &runs {
        assert_eq!(r.stdout, "20001 20002 10\n", "{backend}: stderr: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
        let bytes = 20_002u64;
        let read = scanned(&r.stderr);
        assert!(
            read < 200 * bytes,
            "{backend}: twenty thousand slices of a {bytes}-byte string read {read} bytes to \
             find their scalars, which is a walk from the front per slice"
        );
    }
}

/// MEMORY.md §5.3's in-place concatenation can write a short string's tail
/// over bytes a longer, dead view of the same block was indexed over. The
/// answers about the grown string must be about its own bytes.
///
/// `long` is indexed by the `charAt`, `prefix` is a view of its first ten
/// scalars, and `long` dies with `longPrefix`'s frame, which leaves `prefix`
/// the block's only owner. The concatenation then writes `abc…` from byte 20,
/// where the old index says `é` continues.
#[test]
fn a_string_grown_in_place_over_an_indexed_block_reads_its_own_scalars() {
    let source = r#"
from "core/effect" import { Allocator };
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

fn longPrefix<C: Allocator>(ctx: C): Str {
  let long = "é".repeat(ctx, 300);
  match (long.charAt(299)) {
    .Some(_c) => long.slice(0, 10),
    .None => "",
  }
}

fn code(c: Option<Char>): Int {
  match (c) { .Some(x) => x.toU32().toI64(), .None => -1 }
}

export fn main(): Result<(), Str> {
  let grown = longPrefix(alloc).concat(alloc, "abc".repeat(alloc, 100));
  let at = match (grown.indexOf("c")) { .Some(i) => i, .None => -1 };
  let _ = io.println(
    stdout,
    "${grown.length()} ${code(grown.charAt(9))},${code(grown.charAt(10))},${code(grown.charAt(11))},${code(grown.charAt(310))} ${grown.slice(10, 13)} ${grown.slice(307, 310)} ${at}",
  ).ignore();
  .Ok(())
}
"#;
    for (backend, r) in run_everywhere("grown-over-index", source) {
        assert_eq!(r.stdout, "310 233,97,98,-1 abc abc 12\n", "{backend}: stderr: {}", r.stderr);
        let (_, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: blocks still live at exit");
    }
}
