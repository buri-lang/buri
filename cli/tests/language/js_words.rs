//! A rotate, a byte reversal or a bit count of a 64-bit word does no `BigInt`
//! arithmetic on the JavaScript backend.
//!
//! SipHash, which every `derive Hash` reaches, and SHA-512 are loops of
//! `rotateLeftU64` and `rotateRightU64`. A 64-bit integer is a `BigInt` on the
//! JavaScript backend, and the runtime used to rotate one with four shifts and
//! two `BigInt.asUintN` calls, each allocating; a crypto conformance run spent
//! nearly a third of its time there. A `DataView` reads the word as two 32-bit
//! halves that are `number`s and writes the turned halves back, with the same
//! bits.
//!
//! The test runs a loop of each with `BigInt` counted, so a regression to the
//! slow route is a failure and not a slower run. `numbers/test/bits.buri`
//! asserts the answers themselves on every backend.
use crate::harness::hang::Capped;
use crate::harness::*;

const PROGRAM: &str = r#"
from "platform/effect" import { Stdout };
from "node" import { NodeHost };
from "core/bits" import * as bits;
from "core/io" import * as io;

fn unsigned(x: U64, i: U32): U64 {
    if (i == 1000) {
        x
    } else {
        unsigned(bits.rotateLeftU64(x, 13) ^ bits.rotateRightU64(x, 41) ^ bits.byteSwapU64(x), i + 1)
    }
}

fn signed(x: Int, i: U32): Int {
    if (i == 1000) {
        x
    } else {
        signed(bits.rotateRight(bits.rotateLeft(x, 7), 32) ^ -3, i + 1)
    }
}

fn swapped(w: U32, i: U32): U32 {
    if (i == 1000) {
        w
    } else {
        swapped(bits.byteSwapU32(w).wrappingAdd(i), i + 1)
    }
}

/// How many of a walk's words have more than 32 bits set, a top bit set, and
/// a bottom bit set, as the three groups of three digits of one number.
fn counted(z: U64, i: U32, a: U32, b: U32, c: U32): U32 {
    if (i == 1000) {
        a * 1000000 + b * 1000 + c
    } else {
        let next = bits.rotateLeftU64(z, 1) ^ 11400714819323198485;
        counted(
            next,
            i + 1,
            if (bits.popCountU64(next) > 32) { a + 1 } else { a },
            if (bits.leadingZerosU64(next) == 0) { b + 1 } else { b },
            if (bits.trailingZerosU64(next) == 0) { c + 1 } else { c },
        )
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let u = unsigned(81985529216486895, 0);
    let s = signed(-81985529216486895, 0);
    let w = swapped(305419896, 0);
    let n = counted(81985529216486895, 0, 0, 0, 0);
    let _ = io.println(host.stdout, "${u} ${s} ${w} ${n}").ignore();
    .Ok(())
}
"#;

/// Runs the module with every way to build a `BigInt` but a literal counting
/// each call, and prints the count to stderr when the program exits. A
/// `DataView` read answers one without any of those ways, so what is counted
/// is `BigInt` arithmetic.
const COUNTING: &str = r#"
let count = 0;
const real = BigInt;
const counting = (v) => { count++; return real(v); };
counting.asIntN = (b, v) => { count++; return real.asIntN(b, v); };
counting.asUintN = (b, v) => { count++; return real.asUintN(b, v); };
globalThis.BigInt = counting;
process.on("exit", () => process.stderr.write("count " + count + "\n"));
const { pathToFileURL } = await import("node:url");
await import(pathToFileURL(process.argv[2]).href);
"#;

/// Rotates and shifts by counts written as literals, at every width, and one
/// by a `U32` count widened to `Int`, a thousand times each.
const LITERAL_COUNTS: &str = r#"
from "platform/effect" import { Stdout };
from "node" import { NodeHost };
from "core/bits" import * as bits;
from "core/io" import * as io;

fn narrow(w: U32, b: U8, i: U32): U32 {
    if (i == 1000) {
        w ^ b.toU32()
    } else {
        let turned = bits.rotateLeftU32(w, 7) ^ bits.rotateRightU32(w, 13) ^ bits.shiftRightU32(w, 3);
        let spun = bits.rotateLeftU8(b, 3) ^ bits.shiftLeftU8(b, 1);
        narrow(turned ^ bits.rotateLeftU32(w, (i % 32).toI64()), spun, i + 1)
    }
}

fn wide(x: U64, s: Int, i: U32): Int {
    if (i == 1000) {
        s ^ bits.popCountU64(x)
    } else {
        let turned = bits.rotateRightU64(x, 41) ^ bits.shiftLeftU64(x, 5);
        wide(turned, bits.rotateLeft(s, 13) ^ bits.shiftRight(s, 2), i + 1)
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let _ = io.println(host.stdout, "${narrow(305419896, 77, 0)} ${wide(81985529216486895, -81985529216486895, 0)}").ignore();
    .Ok(())
}
"#;

/// Runs the module with `Number` counting each call it is handed a `BigInt`.
const NUMBER_FROM_BIGINT: &str = r#"
let count = 0;
const real = Number;
const counting = function (v) { if (typeof v === "bigint") count++; return real(v); };
for (const k of Object.getOwnPropertyNames(real)) {
  if (k !== "length" && k !== "name") Object.defineProperty(counting, k, Object.getOwnPropertyDescriptor(real, k));
}
globalThis.Number = counting;
process.on("exit", () => process.stderr.write("count " + count + "\n"));
const { pathToFileURL } = await import("node:url");
await import(pathToFileURL(process.argv[2]).href);
"#;

/// A count is an `Int`, a `BigInt` here, and the runtime reads it as a
/// `number` with `Number`, which on a `BigInt` is a call into the engine. A
/// literal count, or a `U32` one widened, goes in as the `number` it already
/// is, so the 10,000 rotates and shifts this program makes convert none.
#[test]
fn a_rotate_or_shift_by_a_literal_count_converts_no_bigint() {
    let scratch = Scratch::repo("js-words-counts");
    scratch.binary_package("cmd/counts", LITERAL_COUNTS);
    scratch.run(&["build", "//cmd/counts", "--force"]).ok();
    let artifact = scratch.artifact_in("node", "cmd/counts");
    let preload = scratch.write("counting.mjs", NUMBER_FROM_BIGINT);
    let out = std::process::Command::new(js_runtime())
        .arg(&preload)
        .arg(&artifact)
        .capped_output()
        .expect("the javascript runtime runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the counts failed:\n{}",
        indent(&stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "92396152 -5337059898134375286\n"
    );
    let converted: usize = stderr
        .lines()
        .find_map(|l| l.strip_prefix("count "))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in:\n{}", indent(&stderr)));
    assert!(
        converted <= 16,
        "{converted} BigInt counts converted to numbers; a literal count should need none"
    );
}

/// The 1,024 small `Int`s the runtime builds once when it loads, which every
/// count answers from.
const AT_LOAD: usize = 1024;

/// The program makes 5,000 rotates, 2,000 byte reversals and 3,000 counts.
/// Each rotate used to build two `BigInt`s, and each reversal three or more.
#[test]
fn rotates_counts_and_byte_swaps_of_a_wide_word_do_no_bigint_arithmetic() {
    let scratch = Scratch::repo("js-words");
    scratch.binary_package("cmd/words", PROGRAM);
    scratch.run(&["build", "//cmd/words", "--force"]).ok();
    let artifact = scratch.artifact_in("node", "cmd/words");
    let preload = scratch.write("counting.mjs", COUNTING);
    let out = std::process::Command::new(js_runtime())
        .arg(&preload)
        .arg(&artifact)
        .capped_output()
        .expect("the javascript runtime runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the words failed:\n{}",
        indent(&stderr)
    );
    // Each chain a thousand times, computed outside Buri.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "409767520521109995 -3359843005175656717 2919639816 327547453\n"
    );
    let built: usize = stderr
        .lines()
        .find_map(|l| l.strip_prefix("count "))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in:\n{}", indent(&stderr)));
    assert!(
        built <= AT_LOAD + 16,
        "{built} BigInts built, {AT_LOAD} of them at load; the words should build none"
    );
}
