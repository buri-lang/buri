//! A wrapping operation at 32 bits and under builds no `BigInt`.
//!
//! Hashes and checksums are loops of `wrappingMultiply` and `wrappingAdd`, and
//! on the JavaScript backend an integer up to 32 bits is a `number`. The
//! answer has to be the low bits of the exact result, which for a product is
//! beyond what a double holds, and the runtime used to get it by building two
//! `BigInt`s per operation. `Math.imul` and the bit operators give the same
//! bits without them, and a conformance run, which spends most of its time in
//! such loops, is faster by the difference.
//!
//! The test runs a loop of them with `BigInt` taken away, so a regression to
//! the slow route is a failure and not a slower run. The values are what
//! `numbers/test/integers.buri` also asserts, from a longer chain.
use crate::harness::*;

const PROGRAM: &str = r#"
from "platform/effect" import { Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

fn unsignedChain(h: U32, i: U32): U32 {
    if (i == 1000) {
        h
    } else {
        unsignedChain(h.wrappingMultiply(16777619).wrappingAdd(i).wrappingSubtract(7), i + 1)
    }
}

fn signedChain(g: I32, i: I32): I32 {
    if (i == 1000) {
        g
    } else {
        signedChain(g.wrappingMultiply(1664525).wrappingAdd(i).wrappingSubtract(7), i + 1)
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let u = unsignedChain(2166136261, 0);
    let s = signedChain(-5, 0);
    let _ = io.println(host.stdout, "${u} ${s} ${u.wrapToU8()} ${u.wrapToI16()}").ignore();
    .Ok(())
}
"#;

/// Runs the module after `BigInt` has been replaced by something that throws,
/// so a `BigInt` built anywhere on the way fails the run. Literals such as
/// `0n` are syntax and unaffected.
const WITHOUT_BIGINT: &str = r#"
const refuse = () => { throw new Error("a BigInt was built"); };
const none = Object.assign(refuse, { asIntN: refuse, asUintN: refuse });
globalThis.BigInt = none;
const { pathToFileURL } = await import("node:url");
await import(pathToFileURL(process.argv[2]).href);
"#;

#[test]
fn a_chain_of_narrow_wrapping_operations_builds_no_bigint() {
    let scratch = Scratch::repo("js-wrapping");
    scratch.binary_package("cmd/chain", PROGRAM);
    scratch.run(&["build", "//cmd/chain", "--force"]).ok();
    let artifact = scratch.artifact_in("node", "cmd/chain");
    let preload = scratch.write("without_bigint.mjs", WITHOUT_BIGINT);
    let out = std::process::Command::new(js_runtime())
        .arg(&preload)
        .arg(&artifact)
        .output()
        .expect("the javascript runtime runs");
    assert!(
        out.status.success(),
        "the chain failed without BigInt:\n{}",
        indent(&String::from_utf8_lossy(&out.stderr))
    );
    // 2166136261 and -5 chained a thousand times, computed outside Buri.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "3949928713 -1686102209 9 8457\n"
    );
}
