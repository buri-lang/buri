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

/// Strings a template or a derived `Show` writes, passed straight to a
/// function and given back after it: to a callee that keeps them, to one that
/// hands them on to itself, read before the call too, and list items and
/// values of every width passed to closures.
#[test]
fn written_strings_and_closure_arguments_arrive_whole() {
    passes(
        "written-arguments",
        r#"from "core/testing/assert" import * as assert;
from "core/str" import * as str;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Equal, Show for Job;
enum Job {
    Running(Int),
    Done,
}

struct Pair {
    a: Int,
    b: Str,
}

fn size(s: Str): Int {
    s.length()
}

fn both(a: Str, n: Int, b: Str): Int {
    a.length() * 100 + n * 10 + b.length()
}

fn again(s: Str, n: Int): Int {
    if (n <= 0) { s.length() } else { again(s, n - 1) }
}

fn keep(s: Str): Pair {
    Pair { a: s.length(), b: s }
}

test "templates passed to functions" {
    let ctx = context { Allocator: alloc() };
    let n = 42;
    assert.equal(size(str.format(ctx, "n=${n}")), 4);
    assert.equal(both(str.format(ctx, "${n}"), size(str.format(ctx, "${n}!")), str.format(ctx, "<${n}>")), 234);
    assert.equal(again(str.format(ctx, "${n}${n}"), 3), 4);
    assert.equal(keep(str.format(ctx, "${n}-")).b, "42-");
}

test "derived shows passed to functions" {
    let ctx = context { Allocator: alloc() };
    assert.equal(size(Job.Running(size("abc")).show(ctx)), 11);
    assert.equal(both(Job.Done.show(ctx), 1, Job.Running(7).show(ctx)), 521);
    assert.equal(keep(Job.Running(2).show(ctx)).b, ".Running(2)");
}

test "a string read before the call it is passed to" {
    let ctx = context { Allocator: alloc() };
    let s = str.format(ctx, "${size("ab")}x");
    assert.equal(s.length() + size(s), 4);
    let t = Job.Done.show(ctx);
    assert.equal(both(t, t.length(), t), 555);
}

test "list items and every width passed to closures" {
    let ctx = context { Allocator: alloc() };
    let xs = ["a", "bb", str.format(ctx, "${size("ccc")}")];
    assert.equal(xs.map(ctx, fn(s) => s.length()), [1, 2, 1]);
    assert.equal(xs.map(ctx, fn(s) => size(s) + s.length()), [2, 4, 2]);
    let top: I128 = 170141183460469231731687303715884105727;
    let f = fn(w: I128, p: Pair, k: Bool, s: Str) => if (k && p.a == s.length()) { w - 1 } else { w - 2 };
    assert.equal(f(top, Pair { a: 39, b: "z" }, true, str.format(ctx, "${top}")), top - 1);
    assert.equal(f(top, keep("abc"), true, "q"), top - 2);
    let g = fn(x: Int, s: Str) => x * 2 + s.length();
    assert.equal(g(g(3, Job.Done.show(ctx)), str.format(ctx, "${top}")), 61);
}

struct Held {
    run: fn(I128, Bool, Str, Int) => I128,
}

fn through(h: Held, n: Int): I128 {
    let run = h.run;
    run(170141183460469231731687303715884105727, n > 0, "four", n)
}

test "constants and written strings passed to a held closure" {
    let ctx = context { Allocator: alloc() };
    let h = Held { run: fn(w, k, s, n) => if (k && s.length() == n) { w - 1 } else { w - 2 } };
    let top: I128 = 170141183460469231731687303715884105727;
    assert.equal(through(h, 4), top - 1);
    assert.equal(through(h, 3), top - 2);
    let run = h.run;
    assert.equal(run(top, true, str.format(ctx, "${size("abc")}"), 1), top - 1);
    assert.equal(run(7, true, Job.Done.show(ctx), 5), 6);
}
"#,
        5,
    );
}

/// Records and enums built straight into a call's argument list: from other
/// calls' answers, from fields loaded out of another record, past more calls
/// made for the same arguments, wide ones, nested ones, and two in one call.
#[test]
fn records_built_as_arguments_arrive_whole() {
    passes(
        "record-arguments",
        r#"from "core/testing/assert" import * as assert;

struct Pair {
    a: Int,
    b: Int,
}

struct Wide {
    a: Int,
    b: Int,
    c: Int,
    d: Int,
    e: Int,
    f: Int,
}

struct Outer {
    p: Pair,
    q: Pair,
}

enum Step {
    By(Int),
    Both(Int, Int),
    Stay,
}

fn down(n: Int): Int {
    if (n <= 0) { 0 } else { 1 + down(n - 1) }
}

fn diff(p: Pair): Int {
    down(p.a) - p.b
}

fn walk(p: Pair, s: Step): Int {
    match (s) {
        .By(k) => diff(p) * 10 + k,
        .Both(k, j) => diff(p) * 100 + k * 10 + j,
        .Stay => diff(p),
    }
}

fn sum(w: Wide): Int {
    w.a + w.b * 10 + w.c * 100 + w.d * 1000 + w.e * 10000 + w.f * 100000
}

fn outer(o: Outer): Int {
    diff(o.p) * 10 + diff(o.q)
}

fn swap(p: Pair): Pair {
    Pair { a: p.b, b: p.a }
}

test "from other calls' answers" {
    assert.equal(diff(Pair { a: down(5), b: down(2) }), 3);
    assert.equal(walk(Pair { a: down(4) + 1, b: down(1) }, Step.By(down(3))), 43);
    assert.equal(walk(Pair { a: 9, b: down(3) }, Step.Both(down(1), down(2))), 612);
    assert.equal(walk(Pair { a: down(2), b: 2 }, Step.Stay), 0);
}

test "from fields loaded out of another record" {
    let p = swap(Pair { a: 2, b: 7 });
    assert.equal(diff(Pair { a: p.a, b: p.b }), 5);
    assert.equal(diff(Pair { a: p.a, b: down(p.b) - 1 }), 6);
}

test "wide and nested" {
    assert.equal(sum(Wide { a: 1, b: down(2), c: 3, d: down(4), e: 5, f: down(6) }), 654321);
    let q = Pair { a: down(8), b: 3 };
    assert.equal(outer(Outer { p: Pair { a: down(9), b: down(1) }, q: q }), 85);
}

test "a record passed alongside another call's answer" {
    assert.equal(walk(Pair { a: 6, b: 1 }, Step.By(walk(Pair { a: 3, b: 3 }, Step.Stay))), 50);
}
"#,
        4,
    );
}

/// One call's answer passed straight to another, at every width: nested three
/// deep, beside arguments built before and after it, to a closure and from
/// one, from a callee handed constants and strings it drops, and down a
/// recursion three thousand calls deep.
#[test]
fn answers_passed_straight_to_calls_arrive_whole() {
    passes(
        "answer-arguments",
        r#"from "core/testing/assert" import * as assert;
from "core/str" import * as str;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Equal, Show for Pair;
struct Pair {
    a: Int,
    b: Str,
}

derive Equal, Show for Job;
enum Job {
    Running(Int, I128),
    Done,
}

fn flag(n: Int): Bool { n > 2 }
fn small(n: Int): I8 { if (n > 0) { 1 } else { 3 } }
fn wide(n: Int): I128 { if (n > 0) { 1 } else { 170141183460469231731687303715884105727 } }
fn half(n: Int): Float { if (n > 0) { 1.5 } else { 0.5 } }
fn text(n: Int): Str { if (n > 0) { "abc" } else { "" } }
fn pair(n: Int): Pair { Pair { a: n, b: "p" } }
fn job(n: Int): Job { if (n > 0) { Job.Running(n, 7) } else { Job.Done } }

fn ofFlag(b: Bool): Int { if (b) { 1 } else { 0 } }
fn ofSmall(x: I8): Int { if (x == 3) { 3 } else { 0 } }
fn ofWide(w: I128): Int { if (w > 0) { 4 } else { 0 } }
fn ofHalf(f: Float): Int { if (f == 0.5) { 5 } else { 0 } }
fn ofText(s: Str): Int { s.length() }
fn ofPair(p: Pair): Int { p.a + p.b.length() }
fn ofJob(j: Job): Int { match (j) { .Running(n, w) => if (w == 7) { n } else { 0 }, .Done => 0 } }

fn inc(n: Int): Int { n + 1 }
fn mix(a: Int, b: Int, c: Int, d: Int): Int { a * 1000 + b * 100 + c * 10 + d }
fn two(a: Pair, b: Pair): Int { a.a * 10 + b.a }
fn keep(s: Str, n: Int): Str { if (n > 0) { s } else { "none" } }
fn drop(s: Str, w: I128): Int { if (w > 0) { s.length() } else { 0 } }

fn sum(n: Int): Int {
    if (n == 0) { 0 } else { mix(0, 0, 0, n % 2) + sum(n - 1) }
}

fn down(n: Int): Int {
    if (n == 0) { 0 } else { inc(down(n - 1)) }
}

test "every width" {
    assert.equal(ofFlag(flag(3)), 1);
    assert.equal(ofSmall(small(0)), 3);
    assert.equal(ofWide(wide(0)), 4);
    assert.equal(ofHalf(half(0)), 5);
    assert.equal(ofText(text(1)), 3);
    assert.equal(ofPair(pair(4)), 5);
    assert.equal(ofJob(job(6)), 6);
    assert.equal(ofJob(job(0)), 0);
}

test "nested and beside other arguments" {
    assert.equal(inc(inc(inc(1))), 4);
    assert.equal(ofText(keep(text(1), ofPair(pair(2)))), 3);
    let x = 3;
    assert.equal(mix(inc(1), x + 1, inc(x), 9), 2449);
    assert.equal(mix(1, inc(1), inc(2), inc(3)), 1234);
    assert.equal(mix(1, mix(2, 3, 4, 5), 6, 7), 235567);
    assert.equal(two(pair(3), pair(inc(4))), 35);
    let t = inc(5);
    assert.equal(mix(t, t, inc(t), t), 6676);
}

test "constants and dropped strings" {
    let ctx = context { Allocator: alloc() };
    assert.equal(inc(drop(str.format(ctx, "${x()}!"), 170141183460469231731687303715884105727)), 3);
    assert.equal(ofText(keep(str.format(ctx, "<${x()}>"), 1)), 3);
    assert.equal(ofText(keep("lit", 0)), 4);
    assert.equal(ofPair(Pair { a: inc(drop(Job.Done.show(ctx), 1)), b: "q" }), 7);
}

test "answers dropped after the call they are passed to" {
    let ctx = context { Allocator: alloc() };
    assert.equal(ofText(built(ctx, 12)), 3);
    assert.equal(ofPair(Pair { a: 1, b: built(ctx, 4) }), 3);
    assert.equal(again(built(ctx, 345), 3), 4);
    assert.equal(keep(built(ctx, 6), 1), "6!");
    assert.equal(ofJob(job(inc(4))) + ofText(built(ctx, ofText(built(ctx, 1)))), 7);
}

fn built<C: Allocator>(ctx: C, n: Int): Str { str.format(ctx, "${n}!") }

fn again(s: Str, n: Int): Int {
    if (n <= 0) { s.length() } else { again(s, n - 1) }
}

fn x(): Int { 1 }

struct Held {
    run: fn(Int, Pair) => Int,
    both: fn(Int, Int) => Int,
}

test "closures" {
    let h = Held { run: fn(n, p) => n * 10 + p.a, both: fn(a, b) => a * 10 + b };
    let run = h.run;
    assert.equal(run(inc(1), pair(3)), 23);
    let f = fn(n: Int) => n * 3;
    let g = fn(s: Str) => s.length();
    assert.equal(f(g(text(1))), 9);
    assert.equal(run(f(2), pair(f(1))), 63);
    let both = h.both;
    let t = f(2);
    assert.equal(both(t, t), 66);
}

test "deep recursions" {
    assert.equal(sum(3000), 1500);
    assert.equal(down(3000), 3000);
}
"#,
        6,
    );
}

/// Records and enums of several widths, met by function after function of one
/// unit: built, read field by field, matched and passed on. Every function
/// lays out the same types again, so each must find the layout the first one
/// found.
#[test]
fn types_met_by_many_functions_of_one_unit_read_back() {
    passes(
        "shared-layouts",
        r#"from "core/testing/assert" import * as assert;

struct Narrow { a: Bool, b: Bool, c: Bool }

struct Wide { x: Int, y: Int, z: Int, w: Int }

struct Mixed { n: Narrow, at: Int, wide: Wide }

enum Shape {
    Dot,
    Line(Int),
    Box(Wide),
    Tagged(Narrow, Int),
}

fn narrow(i: Int): Narrow { Narrow { a: i > 0, b: i > 1, c: i > 2 } }

fn wide(i: Int): Wide { Wide { x: i, y: i * 2, z: i * 3, w: i * 4 } }

fn mixed(i: Int): Mixed { Mixed { n: narrow(i), at: i, wide: wide(i) } }

fn bits(n: Narrow): Int {
    (if (n.a) { 1 } else { 0 }) + (if (n.b) { 2 } else { 0 }) + (if (n.c) { 4 } else { 0 })
}

fn total(w: Wide): Int { w.x + w.y + w.z + w.w }

fn shape(i: Int): Shape {
    match (i % 4) {
        0 => .Dot,
        1 => .Line(i),
        2 => .Box(wide(i)),
        _ => .Tagged(narrow(i), i),
    }
}

fn measure(s: Shape): Int {
    match (s) {
        .Dot => 0,
        .Line(n) => n,
        .Box(w) => total(w),
        .Tagged(n, k) => bits(n) * 100 + k,
    }
}

fn of(m: Mixed): Int { bits(m.n) * 1000 + m.at * 100 + total(m.wide) }

fn swap(m: Mixed): Mixed {
    Mixed { n: narrow(3 - bits(m.n) % 4), at: m.wide.x, wide: wide(m.at + 1) }
}

fn sum(i: Int, acc: Int): Int {
    if (i == 0) { acc } else { sum(i - 1, acc + measure(shape(i)) + of(mixed(i % 3))) }
}

test "records read field by field" {
    assert.equal(bits(narrow(2)), 3);
    assert.equal(total(wide(5)), 50);
    assert.equal(of(mixed(2)), 3220);
    assert.equal(of(swap(mixed(1))), 3120);
}

test "enums matched by payload" {
    assert.equal(measure(shape(4)), 0);
    assert.equal(measure(shape(5)), 5);
    assert.equal(measure(shape(6)), 60);
    assert.equal(measure(shape(7)), 707);
}

test "every type met again in a loop" {
    assert.equal(sum(12, 0), 19636);
}
"#,
        3,
    );
}
