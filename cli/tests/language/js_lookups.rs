//! The standard library's Unicode tables are read without building a `BigInt`
//! per digit, and without measuring a table again each time another is read.
//!
//! `isPunctuation`, `normalize`, `caseFold` and `graphemeCount` each look a
//! scalar up in tables of base-36 digits, a binary search that reads a few
//! dozen digits a lookup. An `Int` is a `BigInt` on the JavaScript backend, so
//! when the search counted in `Int` every digit it read became a `BigInt`, and
//! a conformance run spent most of its time there. The search counts in `U32`
//! now, which is a `number`.
//!
//! A `normalize` reads four or more tables for each scalar, and the runtime
//! used to remember only the last long string it had checked for astral
//! scalars, so each read checked its table again from the start.
//!
//! Each test runs a loop of lookups with something counted, so a regression
//! is a failure and not a slower run: the `BigInt`s built, and the times a
//! string is checked for astral scalars. The answers are what Python's
//! `unicodedata` gives, and what `characters.buri` asserts for the classifiers.
use crate::harness::hang::Capped;
use crate::harness::*;

const PROGRAM: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/str" import * as str;
from "core/str" import { Normalization };

fn alone<C: Allocator>(ctx: C, c: Char): Str {
    str.format(ctx, "${c}")
}

fn accented<C: Allocator>(ctx: C, c: Char): Str {
    str.format(ctx, "${c}MARK")
}

fn punctuation(cp: U32, end: U32, n: U32): U32 {
    if (cp == end) {
        n
    } else {
        let more = match (cp.toChar()) {
            .Ok(c) => if (c.isPunctuation()) { n + 1 } else { n },
            .Err(_e) => n,
        };
        punctuation(cp + 1, end, more)
    }
}

fn printable(cp: U32, end: U32, n: U32): U32 {
    if (cp == end) {
        n
    } else {
        let more = match (cp.toChar()) {
            .Ok(c) => if (c.isPrintable()) { n + 1 } else { n },
            .Err(_e) => n,
        };
        printable(cp + 1, end, more)
    }
}

/// The lengths of every scalar from `cp` to `end` under `form`, added up.
fn normalized<C: Allocator>(ctx: C, cp: U32, end: U32, form: Normalization, n: Int): Int {
    if (cp == end) {
        n
    } else {
        let more = match (cp.toChar()) {
            .Ok(c) => n + alone(ctx, c).normalize(ctx, form).length(),
            .Err(_e) => n,
        };
        normalized(ctx, cp + 1, end, form, more)
    }
}

/// How many scalars from `cp` to `end` decompose and compose back to themselves.
fn composedBack<C: Allocator>(ctx: C, cp: U32, end: U32, n: Int): Int {
    if (cp == end) {
        n
    } else {
        let more = match (cp.toChar()) {
            .Ok(c) => {
                let text = alone(ctx, c);
                if (text.normalize(ctx, .Nfd).normalize(ctx, .Nfc) == text) { n + 1 } else { n }
            },
            .Err(_e) => n,
        };
        composedBack(ctx, cp + 1, end, more)
    }
}

fn folded<C: Allocator>(ctx: C, cp: U32, end: U32, n: Int): Int {
    if (cp == end) {
        n
    } else {
        let more = match (cp.toChar()) {
            .Ok(c) => n + alone(ctx, c).caseFold(ctx).length(),
            .Err(_e) => n,
        };
        folded(ctx, cp + 1, end, more)
    }
}

/// The clusters in each scalar from `cp` to `end` with an acute accent on it.
fn clusters<C: Allocator>(ctx: C, cp: U32, end: U32, n: Int): Int {
    if (cp == end) {
        n
    } else {
        let more = match (cp.toChar()) {
            .Ok(c) => n + accented(ctx, c).graphemeCount(),
            .Err(_e) => n,
        };
        clusters(ctx, cp + 1, end, more)
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let p = punctuation(0, 0x30000, 0);
    let q = printable(0, 0x30000, 0);
    let d = normalized(ctx, 0xc0, 0x250, .Nfd, 0);
    let k = normalized(ctx, 0xc0, 0x250, .Nfkd, 0);
    let b = composedBack(ctx, 0xc0, 0x250, 0);
    let f = folded(ctx, 0xc0, 0x250, 0);
    let g = clusters(ctx, 0xc0, 0x250, 0);
    let _ = io.println(ctx, "${p} ${q} ${d} ${k} ${b} ${f} ${g}").ignore();
    .Ok(())
}
"#;

/// What every lookup above answers, computed outside Buri.
const ANSWERS: &str = "856 145944 674 694 400 404 400\n";

/// The lookups `PROGRAM` makes: two classifiers over 0x30000 scalars, and five
/// string operations over 400.
const LOOKUPS: usize = 2 * 0x30000 + 5 * 400;

/// The tables those lookups read: two range tables for the classifiers, four
/// each for canonical and compatibility decomposition and for folding, two for
/// composition, and the combining and grapheme classes.
const TABLES: usize = 2 + 4 + 4 + 4 + 2 + 2;

/// Runs the module with `counted` wrapped so that each call adds one to a
/// count, and prints the count to stderr when the program exits.
fn preload(counted: &str) -> String {
    format!(
        r#"
let count = 0;
{counted}
process.on("exit", () => process.stderr.write("count " + count + "\n"));
const {{ pathToFileURL }} = await import("node:url");
await import(pathToFileURL(process.argv[2]).href);
"#
    )
}

/// `BigInt`, `BigInt.asIntN` and `BigInt.asUintN`: every way to build one but
/// a literal.
const BIGINTS: &str = r#"
const real = BigInt;
const counting = (v) => { count++; return real(v); };
counting.asIntN = (b, v) => { count++; return real.asIntN(b, v); };
counting.asUintN = (b, v) => { count++; return real.asUintN(b, v); };
globalThis.BigInt = counting;
"#;

/// Every regular-expression test of a string as long as a table, which is how
/// the runtime checks one for astral scalars. A short string is checked each
/// time it is asked about, by design, and the program asks about thousands.
const SURROGATE_CHECKS: &str = r#"
const test = RegExp.prototype.test;
RegExp.prototype.test = function (s) {
  if (typeof s === "string" && s.length >= 256) count++;
  return test.call(this, s);
};
"#;

/// The program's answers, and the count `counted` gave.
fn counted_run(name: &str, counted: &str) -> usize {
    let (stdout, count) = counted_program(name, &PROGRAM.replace("MARK", "\u{301}"), counted);
    assert_eq!(stdout, ANSWERS);
    count
}

/// What `program` printed, and the count `counted` gave.
fn counted_program(name: &str, program: &str, counted: &str) -> (String, usize) {
    let scratch = Scratch::repo(name);
    scratch.binary_package("cmd/lookups", program);
    scratch.run(&["build", "//cmd/lookups", "--force"]).ok();
    let artifact = scratch.artifact_in("node", "cmd/lookups");
    let preload = scratch.write("counting.mjs", &preload(counted));
    let out = std::process::Command::new(js_runtime())
        .arg(&preload)
        .arg(&artifact)
        .capped_output()
        .expect("the javascript runtime runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the lookups failed:\n{}",
        indent(&stderr)
    );
    let count = stderr
        .lines()
        .find_map(|l| l.strip_prefix("count "))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in:\n{}", indent(&stderr)));
    (String::from_utf8_lossy(&out.stdout).into_owned(), count)
}

/// Counting a digit in `Int` builds at least one `BigInt` for each of the
/// dozens a lookup reads. About two a lookup are left, both turning the
/// table's length into the `U32` the search starts from.
#[test]
fn a_table_lookup_builds_no_bigint_per_digit() {
    let built = counted_run("js-lookups-bigint", BIGINTS);
    assert!(
        built <= 4 * LOOKUPS,
        "{built} BigInts built for {LOOKUPS} lookups; a lookup should build at most a few"
    );
}

/// Each lookup reads several tables, so remembering only the last string
/// checked had every read check its table again: 5,752 checks where there are
/// now 23.
#[test]
fn reading_tables_in_turn_checks_each_for_astral_scalars_once() {
    let checks = counted_run("js-lookups-surrogates", SURROGATE_CHECKS);
    assert!(
        checks <= 2 * TABLES,
        "{checks} checks of a long string for astral scalars, over {TABLES} tables"
    );
}

/// Counts the clusters and scalars of short strings, each built at run time.
/// `graphemeCount` reads its string one `charAt` at a time and looks each
/// scalar up in a table in between, so it asks about the two in turn.
const SCAN: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn counted<C: Allocator>(ctx: C, i: Int, end: Int, n: Int): Int {
    if (i == end) {
        n
    } else {
        let text = str.format(ctx, "${i} cafe\u{301} 😀 👍🏽 a\u{308}b ${i}");
        counted(ctx, i + 1, end, n + text.graphemeCount() + text.length())
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "${counted(ctx, 0, STRINGS, 0)}").ignore();
    .Ok(())
}
"#;

/// The strings `SCAN` counts.
const STRINGS: usize = 300;

/// Every regular-expression test of a string, whatever its length.
const EVERY_CHECK: &str = r#"
const test = RegExp.prototype.test;
RegExp.prototype.test = function (s) {
  if (typeof s === "string") count++;
  return test.call(this, s);
};
"#;

/// The runtime remembered only the one long string it last found, and never a
/// short one, so each `charAt` of a short string checked all of it for astral
/// scalars again. It remembers the last two strings asked about now, short or
/// long, so a scan checks its string once.
#[test]
fn a_scan_that_asks_about_two_strings_in_turn_checks_each_once() {
    let program = SCAN.replace("STRINGS", &STRINGS.to_string());
    let (stdout, checks) = counted_program("js-lookups-scan", &program, EVERY_CHECK);
    // Around its two numbers, each string is 13 clusters and 16 scalars; each
    // digit is one of both, and there are two of each number.
    let digits: usize = (0..STRINGS).map(|i| i.to_string().len()).sum();
    assert_eq!(stdout, format!("{}\n", STRINGS * (13 + 16) + 4 * digits));
    assert!(
        checks <= 3 * STRINGS,
        "{checks} checks of a string for astral scalars, for {STRINGS} strings"
    );
}

/// The JavaScript engine normalizes and classifies at the version the tables
/// are, so a program that does both ships none of their tables.
#[test]
fn a_javascript_artifact_carries_no_unicode_tables() {
    let scratch = Scratch::repo("js-lookups-size");
    scratch.binary_package("cmd/lookups", &PROGRAM.replace("MARK", "\u{301}"));
    scratch.run(&["build", "//cmd/lookups", "--release"]).ok();
    let size = std::fs::metadata(scratch.artifact_in("node", "cmd/lookups"))
        .expect("the artifact was written")
        .len();
    assert!(size < 64 * 1024, "the artifact is {size} bytes");
}
