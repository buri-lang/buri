//! Values at the sizes a native backend takes a different path for: list
//! elements three, five, six and seven bytes wide, which no single load reads,
//! a string literal longer than sixteen mebibytes, past what a Mach-O
//! instruction relocation's addend can reach, and records at each width a
//! copy splits differently at.
//!
//! Each run is a real `buri test` over a repository, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in.

use std::path::Path;
use std::process::Command;

fn workspace(name: &str) -> crate::sweep::Owned {
    crate::sweep::owned("native-odd-sizes", name)
}

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

/// `buri test //lib` on every backend, with `test` as the one test source.
/// Each run must pass all `blocks` of it.
pub(crate) fn passes(name: &str, test: &str, blocks: usize) {
    let repo = workspace(name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/t.buri\"]\n    }\n}\n",
    );
    write(
        &repo.join("lib/lib.buri"),
        "export fn unused(): Int {\n    0\n}\n",
    );
    write(&repo.join("lib/test/t.buri"), test);
    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo)
            .arg("test")
            .args(flags)
            .arg("//lib")
            .arg("--force");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        let summary = format!("{blocks} passed, 0 failed");
        assert!(
            ran.status == 0 && ran.stdout.contains(&summary),
            "{backend}: status {}\n{}\n{}",
            ran.status,
            ran.stdout,
            ran.stderr
        );
    }
}

/// Structs of three, five, six and seven `Bool`s, read out of a list by a
/// pattern and by `map`.
#[test]
fn elements_of_three_five_six_and_seven_bytes_read_back() {
    passes(
        "narrow-elements",
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Equal, Show for Three;
struct Three {
    a: Bool,
    b: Bool,
    c: Bool,
}

derive Equal, Show for Five;
struct Five {
    a: Bool,
    b: Bool,
    c: Bool,
    d: Bool,
    e: Bool,
}

derive Equal, Show for Six;
struct Six {
    a: Bool,
    b: Bool,
    c: Bool,
    d: Bool,
    e: Bool,
    f: Bool,
}

derive Equal, Show for Seven;
struct Seven {
    a: Bool,
    b: Bool,
    c: Bool,
    d: Bool,
    e: Bool,
    f: Bool,
    g: Bool,
}

fn second<T>(xs: [T], none: T): T {
    match (xs) {
        [_, x, ..] => x,
        _ => none,
    }
}

test "three bytes" {
    let ctx = context {
        Allocator: alloc(),
    };
    let off = Three { a: false, b: false, c: false };
    let on = Three { a: true, b: true, c: true };
    let xs = [on, Three { ..off, b: true }, on];
    assert.equal(second(xs, off), Three { ..off, b: true });
    assert.equal(xs.map(ctx, fn(t) => t.b && !t.c), [false, true, false]);
}

test "five bytes" {
    let off = Five { a: false, b: false, c: false, d: false, e: false };
    let on = Five { a: true, b: true, c: true, d: true, e: true };
    assert.equal(second([on, Five { ..off, c: true }, on], on), Five { ..off, c: true });
}

test "six bytes" {
    let off = Six { a: false, b: false, c: false, d: false, e: false, f: false };
    let on = Six { a: true, b: true, c: true, d: true, e: true, f: true };
    assert.equal(second([on, Six { ..off, f: true }, on], on), Six { ..off, f: true });
}

test "seven bytes" {
    let off = Seven { a: false, b: false, c: false, d: false, e: false, f: false, g: false };
    let on = Seven { a: true, b: true, c: true, d: true, e: true, f: true, g: true };
    assert.equal(second([on, Seven { ..off, a: true }, on], on), Seven { ..off, a: true });
}
"#,
        4,
    );
}

/// An element with padding after it, and a three-byte value a closure captures
/// and reads on every call.
#[test]
fn a_padded_element_and_a_three_byte_capture_read_back() {
    passes(
        "padded-elements",
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Equal, Show for Padded;
struct Padded {
    n: I32,
    b: Bool,
}

derive Equal, Show for Three;
struct Three {
    a: Bool,
    b: Bool,
    c: Bool,
}

fn second<T>(xs: [T], none: T): T {
    match (xs) {
        [_, x, ..] => x,
        _ => none,
    }
}

fn padded(n: I32): [Padded] {
    [Padded { n: n, b: false }, Padded { n: n + 1, b: true }, Padded { n: n + 2, b: false }]
}

test "an element with padding after it" {
    let none = Padded { n: 0, b: false };
    assert.equal(second(padded(7), none), Padded { n: 8, b: true });
}

test "a three-byte capture" {
    let ctx = context {
        Allocator: alloc(),
    };
    let t = second([Three { a: false, b: false, c: false }, Three { a: true, b: false, c: true }], Three { a: false, b: true, c: false });
    let f = fn(x: Int) => if (t.a && !t.b && t.c) { x } else { 0 - x };
    assert.equal([1, 2, 3].map(ctx, f), [1, 2, 3]);
}
"#,
        2,
    );
}

/// A literal of sixteen mebibytes and eight bytes, with a short one after it,
/// both read back.
#[test]
fn a_literal_past_sixteen_mebibytes_builds_and_reads_back() {
    let n = (16 << 20) + 8;
    let test = format!(
        r#"from "core/testing/assert" import * as assert;

test "a long literal and a short one after it" {{
    let long = "{}";
    let short = "after";
    assert.equal(long.length(), {n});
    assert.equal(short, "after");
    assert.equal(long.slice(0, 4), "abab");
}}
"#,
        "ab".repeat(n / 2)
    );
    passes("long-literal", &test, 1);
}

/// Records from 96 bytes to over a kilobyte, each passed down a recursion that
/// rebuilds it and handed back up, so every width a copy splits differently at
/// is copied on a call, into a literal and on a return: under 128 bytes, at it,
/// past it with each tail, at the loop's 256 and past it, and over a kilobyte.
#[test]
fn records_of_every_copy_width_survive_calls_and_returns() {
    let widths = [11usize, 15, 16, 17, 23, 31, 32, 33, 47, 48, 63, 140];
    let mut test = String::from("from \"core/testing/assert\" import * as assert;\n");
    for n in widths {
        let fields: String = (0..n).map(|i| format!("    a{i}: Int,\n")).collect();
        let made: Vec<String> = (0..n).map(|i| format!("a{i}: {}", i * 3)).collect();
        let (mid, last) = (n / 2, n - 1);
        test.push_str(&format!(
            "\nstruct W{n} {{\n{fields}    flag: Bool,\n}}\n\n\
             fn keep{n}(w: W{n}, k: Int): W{n} {{\n    \
             if (k <= 0) {{ w }} else {{ keep{n}(W{n} {{ ..w, a0: w.a0 + 1 }}, k - 1) }}\n}}\n\n\
             test \"a record of {n} words and a flag\" {{\n    \
             let w = W{n} {{ {made}, flag: true }};\n    \
             let v = keep{n}(w, 4);\n    \
             assert.equal(v.a0, 4);\n    \
             assert.equal(v.a{mid}, {mid3});\n    \
             assert.equal(v.a{last}, {last3});\n    \
             assert.isTrue(v.flag);\n    \
             assert.equal(w.a0, 0);\n}}\n",
            made = made.join(", "),
            mid3 = mid * 3,
            last3 = last * 3,
        ));
    }
    passes("copy-widths", &test, widths.len());
}

/// Records of one, three, forty and seventy words turned round a loop: a tail
/// call passes three of them on rotated and two more swapped, so each pass is
/// two cycles of copies on the loop's way back. Forty and seventy words are
/// wider than the scratch a cycle turns through.
#[test]
fn records_rotated_round_a_loop_come_back_in_order() {
    let widths = [1usize, 3, 40, 70];
    let mut test = String::from("from \"core/testing/assert\" import * as assert;\n");
    for n in widths {
        let fields: String = (0..n).map(|i| format!("    a{i}: Int,\n")).collect();
        let make = |r: usize| {
            let made: Vec<String> = (0..n).map(|i| format!("a{i}: {}", r * 1000 + i)).collect();
            format!("W{n} {{ {} }}", made.join(", "))
        };
        let last = n - 1;
        // Seven passes turn the three once and swap the two.
        let want: usize = [2usize, 3, 1, 5, 4]
            .iter()
            .zip([100_000, 10_000, 1_000, 100, 1])
            .map(|(r, m)| (r * 1000 + last) * m)
            .sum();
        test.push_str(&format!(
            "\nstruct W{n} {{\n{fields}}}\n\n\
             fn turn{n}(a: W{n}, b: W{n}, c: W{n}, x: W{n}, y: W{n}, k: Int): W{n} {{\n    \
             if (k <= 0) {{\n        \
             W{n} {{ ..a, a{last}: a.a{last} * 100000 + b.a{last} * 10000 + c.a{last} * 1000 \
             + x.a{last} * 100 + y.a{last} }}\n    \
             }} else {{\n        turn{n}(b, c, a, y, x, k - 1)\n    }}\n}}\n\n\
             test \"records of {n} words turned round a loop\" {{\n    \
             let v = turn{n}({a}, {b}, {c}, {x}, {y}, 7);\n    \
             assert.equal(v.a{last}, {want});\n    \
             assert.equal(v.a0, {first});\n}}\n",
            a = make(1),
            b = make(2),
            c = make(3),
            x = make(4),
            y = make(5),
            first = if last == 0 { want } else { 2000 },
        ));
    }
    passes("rotated", &test, widths.len());
}

/// Records of one, three, seventeen and forty words and a string, returned
/// from both arms of an `if`, from each arm of a `match`, and past an `if`
/// whose arms only check something, all come back whole.
#[test]
fn records_returned_from_arms_come_back_whole() {
    let widths = [1usize, 3, 17, 40];
    let mut test = String::from("from \"core/testing/assert\" import * as assert;\n");
    for n in widths {
        let fields: String = (0..n).map(|i| format!("    a{i}: Int,\n")).collect();
        let make = |base: &str| {
            let made: Vec<String> = (0..n).map(|i| format!("a{i}: {base} + {i}")).collect();
            format!("W{n} {{ {}, name: \"w{n}\" }}", made.join(", "))
        };
        let last = n - 1;
        test.push_str(&format!(
            "\nstruct W{n} {{\n{fields}    name: Str,\n}}\n\n\
             fn either{n}(k: Int): W{n} {{\n    \
             if (k > 0) {{ {pos} }} else {{ {neg} }}\n}}\n\n\
             fn pick{n}(k: Int): W{n} {{\n    \
             match (k) {{\n        0 => {zero},\n        1 => {one},\n        _ => {many},\n    }}\n}}\n\n\
             fn past{n}(w: W{n}, k: Int): W{n} {{\n    \
             let _ = if (k > 0) {{ assert.isTrue(k > 0) }} else {{ assert.isTrue(k <= 0) }};\n    \
             w\n}}\n\n\
             test \"records of {n} words returned from arms\" {{\n    \
             assert.equal(either{n}(5).a{last}, {e5});\n    \
             assert.equal(either{n}(-5).a0, -501);\n    \
             assert.equal(pick{n}(0).a{last}, {last});\n    \
             assert.equal(pick{n}(1).a0, 10);\n    \
             assert.equal(pick{n}(7).a{last}, {p7});\n    \
             assert.equal(pick{n}(7).name, \"w{n}\");\n    \
             let w = past{n}(either{n}(2), 3);\n    \
             assert.equal(w.a{last}, {e2});\n    \
             assert.equal(past{n}(w, -1).name, \"w{n}\");\n}}\n",
            pos = make("k * 100"),
            neg = make("k * 100 - 1"),
            zero = make("0"),
            one = make("10"),
            many = make("k * 1000"),
            e5 = 500 + last,
            p7 = 7000 + last,
            e2 = 200 + last,
        ));
    }
    passes("returned", &test, widths.len());
}

/// Constants of every width passed straight to a function: past another call
/// made for the same call's arguments, named and passed twice, kept by the
/// callee, and the literals a derived `Show` writes between its fields.
#[test]
fn constants_passed_to_functions_arrive_whole() {
    passes(
        "constant-arguments",
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Equal, Show for Job;
enum Job {
    Running(Int),
    Failed { code: Int, message: Str },
    Done,
}

derive Equal, Show for Two;
struct Two {
    a: Str,
    b: Str,
}

fn join(a: Str, n: Int, b: Str, wa: Str, wb: Str): Int {
    if (a == wa && b == wb) { n } else { -1 }
}

fn next(n: Int): Int {
    n + 1
}

fn mixed(a: Bool, c: Char, f: Float, s: Str): Str {
    if (a && c == 'x' && f > 2.0) { s } else { "no" }
}

fn wide(w: I128, k: I128): I128 {
    w - k
}

fn seven(): I128 {
    7
}

fn twice(s: Str): Two {
    Two { a: s, b: s }
}

test "literals passed past a call" {
    assert.equal(join("<", next(next(1)), ">", "<", ">"), 3);
    assert.equal(join("(", join("[", next(5), "]", "[", "]"), ")", "(", ")"), 6);
}

test "a literal named and passed twice" {
    let s = "kept";
    assert.equal(join(s, next(0), s, "kept", s), 1);
    assert.equal(join(s, 2, "kept", s, s), 2);
}

test "constants of every width" {
    assert.equal(mixed(true, 'x', 2.5, "y"), "y");
    assert.equal(mixed(true, 'x', 1.5, "y"), "no");
    let top: I128 = 170141183460469231731687303715884105727;
    assert.equal(wide(170141183460469231731687303715884105727, seven()), top - 7);
}

test "a literal the callee keeps" {
    assert.equal(twice("z"), Two { a: "z", b: "z" });
    assert.equal(twice("z").b, "z");
}

test "the literals a derived Show writes" {
    let ctx = context {
        Allocator: alloc(),
    };
    let failed = Job.Failed { code: next(1), message: "no" };
    assert.equal(failed.show(ctx), ".Failed { code: 2, message: \"no\" }");
    assert.equal(Job.Running(next(2)).show(ctx), ".Running(3)");
    assert.equal(Job.Done.show(ctx), ".Done");
}
"#,
        5,
    );
}
