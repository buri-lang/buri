//! Scalar indices into long strings, natively, on the native backend this
//! toolchain has: copy-and-patch on the default leg, LLVM on the
//! `backend-llvm` one.
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

/// One program on whichever native backend this toolchain has, run under the
/// heap check, or `None` where none can run here. As in `e2e.rs`, the default
/// leg runs the copy-and-patch backend and the `backend-llvm` leg the release
/// one.
fn run(name: &str, source: &str) -> Option<Ran> {
    crate::e2e::built_probed(name, source, &scan_probe()).map(|binary| ran_checked(&binary))
}

/// A lexer's loop: a long string with one non-ASCII scalar in it, sliced once
/// per scalar from the front. Walking to each index from the start of the view
/// reads `n² / 2` bytes, which for `n` = 20,000 is two hundred million. The
/// index reads the string once to build and under two runs of 64 bytes per
/// question after that.
#[test]
fn slicing_a_long_string_once_per_scalar_reads_it_a_bounded_number_of_times() {
    let source = r#"
from "native" import { NativeHost };
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

export fn main(host: NativeHost): Result<(), Str> {
  let text = "é".concat(host.alloc, "abcdefghij".repeat(host.alloc, 2000));
  let n = text.length();
  let seen = scan(text, 0, n, 0);
  let tail = match (text.indexOf("jab")) { .Some(at) => at, .None => -1 };
  let _ = io.println(host.stdout, "${n} ${seen} ${tail}").ignore();
  .Ok(())
}
"#;
    let Some(r) = run("scan-per-scalar", source) else { return };
    assert_eq!(r.stdout, "20001 20002 10\n", "stderr: {}", r.stderr);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "blocks still live at exit");
    let bytes = 20_002u64;
    let read = scanned(&r.stderr);
    assert!(
        read < 200 * bytes,
        "twenty thousand slices of a {bytes}-byte string read {read} bytes to find their \
         scalars, which is a walk from the front per slice"
    );
}

/// [`ALLOC_PROBE`], plus a line with the bytes the program allocated in all.
fn bytes_probe() -> String {
    format!(
        "{ALLOC_PROBE}
__attribute__((destructor)) static void buri_bytes_probe(void) {{
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, \"allocated=%llu\\n\", (unsigned long long)s.total_bytes);
}}
"
    )
}

/// `normalize`, `caseFold` and `graphemes` on short strings, a hundred times
/// each. Their Unicode tables are about 86,000 characters of ASCII, and a call
/// reads the few fields it needs with `charAt`. Decoding the tables into
/// `[Char]`s on every call allocated over 150 KB a call, so this bound is the
/// difference between a lookup and a pass over the tables.
#[test]
fn a_short_string_normalizes_without_copying_the_unicode_tables() {
    let source = r#"
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
from "core/io" import * as io;

fn one(right: Bool): Int {
  if (right) { 1 } else { 0 }
}

fn round<C: Allocator>(ctx: C): Int {
  one("e\u{301}".normalize(ctx, .Nfc) == "\u{e9}")
    + one("\u{e9}".normalize(ctx, .Nfd) == "e\u{301}")
    + one("\u{fb01}".normalize(ctx, .Nfkc) == "fi")
    + one("\u{ac01}".normalize(ctx, .Nfkd) == "\u{1100}\u{1161}\u{11a8}")
    + one("\u{1100}\u{1161}\u{11a8}".normalize(ctx, .Nfc) == "\u{ac01}")
    + one("Stra\u{df}e".caseFold(ctx) == "strasse")
    + one("\u{1f44d}\u{1f3fd}!".graphemes(ctx).length() == 2)
}

fn rounds<C: Allocator>(ctx: C, left: Int, right: Int): Int {
  if (left == 0) { right } else { rounds(ctx, left - 1, right + round(ctx)) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc };
  let _ = io.println(host.stdout, "${rounds(ctx, 100, 0)}").ignore();
  .Ok(())
}
"#;
    let Some(binary) = crate::e2e::built_probed("normalize-short", source, &bytes_probe()) else {
        return;
    };
    let r = ran_checked(&binary);
    assert_eq!(r.stdout, "700\n", "stderr: {}", r.stderr);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "blocks still live at exit");
    let allocated: u64 = r
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("allocated="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {:?}", r.stderr))
        .trim()
        .parse()
        .unwrap();
    assert!(
        allocated < 1_000_000,
        "seven hundred calls on strings of one to three scalars allocated {allocated} bytes, \
         which is a copy of the Unicode tables per call"
    );
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
from "platform/effect" import { Allocator };
from "native" import { NativeHost };
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

export fn main(host: NativeHost): Result<(), Str> {
  let grown = longPrefix(host.alloc).concat(host.alloc, "abc".repeat(host.alloc, 100));
  let at = match (grown.indexOf("c")) { .Some(i) => i, .None => -1 };
  let _ = io.println(
    host.stdout,
    "${grown.length()} ${code(grown.charAt(9))},${code(grown.charAt(10))},${code(grown.charAt(11))},${code(grown.charAt(310))} ${grown.slice(10, 13)} ${grown.slice(307, 310)} ${at}",
  ).ignore();
  .Ok(())
}
"#;
    let Some(r) = run("grown-over-index", source) else { return };
    assert_eq!(r.stdout, "310 233,97,98,-1 abc abc 12\n", "stderr: {}", r.stderr);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "blocks still live at exit");
}
