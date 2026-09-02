//! Does a native `show(x)` of a `Float` print what a JavaScript one prints?
//!
//! VALUE-MODEL.md §12 asks that the two backends agree byte for byte, and this
//! is the row where agreement is hardest to get and easiest to lose. On
//! JavaScript, `show` of a `Float` is `$f64` (`runtime.js:84-90`), whose default
//! arm is `String(n)` — which is ECMA-262 §6.1.6.1.20, `Number::toString`, and
//! that is a *property* ("`k` as small as possible; among those, the closest to
//! `x`; ties to even") rather than an algorithm. `cli/runtime/fmt.rs` implements
//! the property, and this file is the evidence that it did.
//!
//! # What it does
//!
//! 1. Compiles a C driver against the embedded `libburi_rt.a`. The driver reads
//!    64-bit patterns as hexadecimal, one per line, and prints
//!    `<pattern> <buri_rt_show_f64 of it>`.
//! 2. Generates the corpus **here**, in Rust, so the two sides are reading the
//!    same bytes and not two independent ideas of "a corner case".
//! 3. Runs the same patterns through the JavaScript engine the rest of the
//!    suite uses, rendering each with `$f64`'s own source, and compares.
//!
//! The corpus is 3 807 072 doubles:
//!
//! | Part | Count | Why |
//! |---|---|---|
//! | Named corners | ~40 | The four presentation cases and both boundaries of each, `1e20`/`1e21`, `1e-6`/`1e-7`, `±0`, both infinities, `NaN` |
//! | Subnormals | 79 998 | Both ends of the pattern space, including `5e-324` |
//! | Powers of ten | 629 | `1e-320` through `1e308`, where the presentation rule switches |
//! | Every `f32`, strided | 2 106 231 | A whole domain swept, widened to `f64` — which is what `show` of an `F32` renders |
//! | Xorshift patterns | 1 500 000 | Uniform over the *bit pattern*, so the exponent range is covered rather than the value range |
//! | Small decimals | 120 000 | `n`, `n/10`, `n/1000` for `n` in `±20 000`: the values a program actually prints |
//!
//! Zero disagreements.
//!
//! # Why this is a module of its own
//!
//! It needs a JavaScript engine, which the backend suites beside it do not, and it
//! takes seconds rather than milliseconds. Both are reasons to be skippable on
//! their own terms rather than to make the fast suite slow, and a module is a
//! name prefix: `cargo test --test native -- --skip float_parity` leaves the
//! rest of the domain fast, and `--test native float_parity` runs only this.
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The driver: the ABI contract's `BuriStr` and one call per line of input.
///
/// Deliberately spelled as a C struct rather than as three loads at hand-written
/// offsets, because that struct *is* `cli/runtime/value.rs`'s `#[repr(C)]` one —
/// a disagreement about it is the kind of thing this file exists to catch.
const DRIVER: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct { unsigned char *base; const unsigned char *ptr; uint64_t len; } BuriStr;
#define STR_LEN_MASK 0x7fffffffffffffffULL

void buri_rt_show_f64(double x, BuriStr *out);
void buri_rt_show_f32(float x, BuriStr *out);
void buri_rt_argv_init(int argc, char **argv);
void buri_rt_flush(void);

/* Files rather than pipes, in both directions. A four-million-line corpus does
 * not fit in a pipe buffer, and a parent that writes the whole of it before
 * reading any of the answer deadlocks against a child doing the same — which is
 * a hang rather than a failure, and the worst way for a test to go wrong. */
int main(int argc, char **argv) {
    buri_rt_argv_init(argc, argv);
    if (argc < 3) return 2;
    FILE *in = fopen(argv[1], "r");
    FILE *out = fopen(argv[2], "w");
    if (!in || !out) return 3;
    char line[64];
    while (fgets(line, sizeof line, in)) {
        uint64_t bits = 0;
        if (sscanf(line, "%llx", (unsigned long long *)&bits) != 1) continue;
        double x;
        memcpy(&x, &bits, sizeof x);
        BuriStr s;
        buri_rt_show_f64(x, &s);
        fprintf(out, "%016llx ", (unsigned long long)bits);
        fwrite(s.ptr, 1, (size_t)(s.len & STR_LEN_MASK), out);
        fputc('\n', out);
    }
    fclose(in);
    fclose(out);
    buri_rt_flush();
    return 0;
}
"#;

/// `$f64`, lifted out of `backend/js/runtime.js` verbatim.
///
/// Copied rather than imported: the runtime is emitted into a generated program
/// and is not a module a script can `require`. The copy is three lines and is
/// checked against the original by [`the_javascript_side_is_the_runtimes_own`],
/// so it cannot drift into being a second opinion.
const CHECKER: &str = r#"
const fs = require("fs");
const buf = new ArrayBuffer(8);
const dv = new DataView(buf);

function $f64(n) {
  if (Number.isNaN(n)) return "NaN";
  if (n === Infinity) return "inf";
  if (n === -Infinity) return "-inf";
  if (Number.isInteger(n) && Math.abs(n) < 1e21) return (Object.is(n, -0) ? "-0" : n) + ".0";
  return String(n);
}

const lines = fs.readFileSync(process.argv[2], "utf8").split("\n");
let checked = 0;
let bad = 0;
for (const line of lines) {
  if (!line) continue;
  const at = line.indexOf(" ");
  const hex = line.slice(0, at);
  const got = line.slice(at + 1);
  dv.setBigUint64(0, BigInt("0x" + hex));
  const want = $f64(dv.getFloat64(0));
  checked++;
  if (want !== got) {
    bad++;
    if (bad <= 10) console.log("MISMATCH " + hex + " js=" + want + " native=" + got);
  }
}
console.log("checked " + checked + " mismatches " + bad);
"#;

/// A directory this *process* owns.
///
/// The process id is in the name, and it has to be: two overlapping
/// `cargo test` runs — routine while several agents are building the same tree
/// — otherwise share `float-parity/floats`, and the second overwrites the
/// binary the first is executing. On macOS that is not an error, it is a child
/// that never returns, and a full-suite run that never completes. The same
/// pattern `--check-reproducible` uses, for the same reason.
fn workspace() -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("float-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The JavaScript engine the rest of the suite runs, or `None`.
fn engine() -> Option<&'static str> {
    ["bun", "node"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn supported() -> bool {
    if !AVAILABLE {
        return !crate::ci::skipped(
            "float parity",
            "no runtime archive was built for this host, so there is nothing to render a float \
             with",
        );
    }
    true
}

/// Every bit pattern the corpus covers, in a fixed order.
///
/// Deterministic on purpose: a parity test whose corpus moves between runs
/// reports a failure nobody can reproduce. The generator is a plain xorshift
/// with a written-down seed for the same reason.
fn corpus() -> Vec<u64> {
    let mut bits: Vec<u64> = Vec::with_capacity(4_200_000);
    let named: &[f64] = &[
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.1,
        0.2,
        0.3,
        1.0 / 3.0,
        2.0 / 3.0,
        4.35,
        1.5,
        2.5,
        0.5,
        100.0,
        // The `k <= n <= 21` boundary, from both sides.
        1e20,
        1e21,
        1e22,
        123_456_789_012_345_678_901.0,
        1_234_567_890_123_456_789_012.0,
        // The `-6 < n` boundary, from both sides.
        1e-6,
        1e-7,
        0.000_001,
        0.000_000_1,
        1e100,
        1e-100,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        // The largest subnormal, as its bit pattern — a decimal literal for it
        // is longer than a double holds, and rounds to the smallest *normal*
        // one line above.
        f64::from_bits(0x000f_ffff_ffff_ffff),
        // And the smallest, at both signs.
        5e-324,
        -5e-324,
        // 2^53 and the first integer a double cannot represent.
        9_007_199_254_740_992.0,
        9_007_199_254_740_993.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for v in named {
        bits.push(v.to_bits());
    }
    // The subnormals at both ends of the pattern space, and their negatives.
    for i in 1u64..40_000 {
        bits.push(i);
        bits.push(u64::MAX - i);
    }
    for p in -320i32..=308 {
        bits.push(10f64.powi(p).to_bits());
    }
    // The whole `f32` domain, strided by a prime so the sweep is not aligned to
    // any exponent boundary. An `F32` renders as the double it widens to, so
    // this is the `show(F32)` corpus as well as part of the `show(F64)` one.
    let mut b: u32 = 0;
    loop {
        bits.push(f64::from(f32::from_bits(b)).to_bits());
        match b.checked_add(2039) {
            Some(next) => b = next,
            None => break,
        }
    }
    // Uniform over the *pattern*, which is uniform over the exponent — the
    // opposite of uniform over the value, and the one that reaches `1e-300`.
    let mut s: u64 = 0x243F_6A88_85A3_08D3;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for _ in 0..1_500_000 {
        bits.push(next());
    }
    // And the values a program actually prints.
    for i in -20_000i64..20_000 {
        let v = i as f64;
        bits.push(v.to_bits());
        bits.push((v / 10.0).to_bits());
        bits.push((v / 1000.0).to_bits());
    }
    bits
}

/// The corpus, rendered natively, checked against JavaScript.
#[test]
// COMPILED OUT, not ignored, on a host no runtime is built for. An `ignore`
// here reported one skipped test per such host for as long as the host
// existed, and a skipped test is a line in a summary that nobody ever acts on;
// a `cfg` says the same thing by the test not being there. macOS and Linux are
// every host `cli/build.rs` writes an archive for, and every host this
// workflow runs on, so nothing that CI could reach is removed by this.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn a_native_float_renders_as_javascript_renders_it() {
    if !supported() {
        return;
    }
    let Some(engine) = engine() else {
        crate::ci::skipped("float parity", "no JavaScript engine (`bun` or `node`) is on PATH");
        return;
    };
    let dir = workspace();
    let archive = dir.join(ARCHIVE_NAME);
    let source = dir.join("floats.c");
    let binary = dir.join("floats");
    std::fs::write(&archive, ARCHIVE).unwrap();
    std::fs::write(&source, DRIVER).unwrap();

    // `build/link.rs`'s own driver and trailing arguments
    // (`shared::product_cc`), because the archive on the far side of this link
    // is the product's — a musl one on Linux — and a harness that answered the
    // libc question differently would not link at all.
    let mut cc = crate::shared::product_cc();
    cc.arg("-std=c11").arg("-O1").arg("-o").arg(&binary).arg(&source).arg(&archive);
    cc.args(crate::shared::product_link_args());
    let built = cc.output().unwrap();
    assert!(
        built.status.success(),
        "the driver did not build:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let bits = corpus();
    let mut input = String::with_capacity(bits.len().saturating_mul(17));
    for b in &bits {
        input.push_str(&format!("{b:016x}\n"));
    }
    let corpus_path = dir.join("corpus.txt");
    let rendered = dir.join("rendered.txt");
    std::fs::write(&corpus_path, &input).unwrap();

    let out = Command::new(&binary).arg(&corpus_path).arg(&rendered).output().unwrap();
    assert!(out.status.success(), "the driver failed: {}", String::from_utf8_lossy(&out.stderr));

    let checker = dir.join("check.js");
    std::fs::write(&checker, CHECKER).unwrap();

    let checked = Command::new(engine).arg(&checker).arg(&rendered).output().unwrap();
    let report = String::from_utf8_lossy(&checked.stdout).to_string();
    assert!(
        checked.status.success(),
        "the checker failed: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let last = report.lines().last().unwrap_or_default();
    assert!(
        last.ends_with("mismatches 0"),
        "native and JavaScript float rendering disagree:\n{report}"
    );
    // A corpus that silently became empty would "pass", and so would a driver
    // that rendered half of it. The count is asserted against the corpus's own
    // length, which is the same reason `conformance_suite_passes` counts its
    // assertions.
    let count: usize = last
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert_eq!(count, bits.len(), "the driver did not render every value");
    eprintln!("float parity: {count} values, 0 mismatches");
    // The corpus and its rendering are about seventy megabytes together, and
    // the directory is this process's, so nothing else can be reading them.
    let _ = std::fs::remove_dir_all(&dir);
}

/// The copy of `$f64` this file checks against is the runtime's own.
///
/// Not a formality: the whole test is worthless if the JavaScript side drifts
/// into being a second implementation of the thing under test. Each clause is
/// looked for in `backend/js/runtime.js` itself.
#[test]
fn the_javascript_side_is_the_runtimes_own() {
    let js = include_str!("../../src/compiler/backend/js/runtime.js");
    for clause in [
        r#"if (Number.isNaN(n)) return "NaN";"#,
        r#"if (n === Infinity) return "inf";"#,
        r#"if (n === -Infinity) return "-inf";"#,
        r#"if (Number.isInteger(n) && Math.abs(n) < 1e21) return (Object.is(n, -0) ? "-0" : n) + ".0";"#,
        "return String(n);",
    ] {
        assert!(js.contains(clause), "`$f64` no longer contains `{clause}`");
        assert!(CHECKER.contains(clause), "this file's copy no longer contains `{clause}`");
    }
}

/// The runtime's own unit tests, run.
///
/// `cli/runtime` is not a cargo crate — `cli/build.rs` drives `rustc` directly
/// — so `cargo test` cannot reach the `#[cfg(test)]` modules inside it, and
/// until this existed they were written and never executed. One `rustc --test`
/// costs a few seconds and turns them back into tests.
///
/// It is here rather than in `native/runtime.rs` because that file's whole
/// argument is that what matters is the *C ABI*, and this is the opposite
/// claim: the algorithms inside — the ECMA-262 presentation rule, the UTF-16
/// hash, the JavaScript whitespace set — are worth testing in Rust, where a
/// failure names the line.
#[test]
// Compiled out rather than ignored, for the reason the corpus test above
// states: `cli/runtime` is written for macOS and Linux and a third host has no
// question to answer here, so it gets no test rather than a skipped one.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn the_runtimes_own_unit_tests_pass() {
    let dir = workspace();
    let binary = dir.join("runtime-tests");
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/lib.rs");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let built = Command::new(&rustc)
        .arg("--test")
        .args(["--edition", "2024"])
        .args(["-C", "opt-level=1"])
        .arg("-o")
        .arg(&binary)
        .arg(&lib)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "the runtime did not build as a test binary:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = Command::new(&binary).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "the runtime's own tests failed:\n{text}");
    let summary = text.lines().find(|l| l.starts_with("test result")).unwrap_or_default();
    // Zero tests would pass silently, and this file's whole point is that a
    // test nothing runs is not a test.
    //
    // The **count**, parsed, and not `text.contains("0 passed")` — which is
    // what this was and which is true of "30 passed" as well as of "0 passed".
    // It passed for as long as the runtime had between one and nine tests and
    // then between eleven and twenty-nine, and the tenth and the thirtieth
    // each broke it; a substring is not a number.
    let passed: usize = summary
        .split_whitespace()
        .zip(summary.split_whitespace().skip(1))
        .find(|(_, word)| *word == "passed;")
        .and_then(|(count, _)| count.parse().ok())
        .unwrap_or(0);
    assert!(passed > 0, "the runtime ran no tests:\n{text}");
    eprintln!("runtime unit tests: {summary}");
}
