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
