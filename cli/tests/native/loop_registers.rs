//! A function with several loops, built by `buri test` on every backend. The
//! copy-and-patch backend keeps each loop's variables in registers inside that
//! loop and in their frame slots outside it, and reads and writes an `Int`
//! element from a register, so a loop that read another loop's register, or an
//! element read at the wrong width, would show here. And what a step of a
//! loop costs where it keeps its variables in registers, counted rather than
//! timed.

/// **Each loop reads its own variables**: sorted lists either way, at a
/// word's width and a byte's, a filter and a map over a sorted list, and a
/// tail-recursive loop that runs a list walk each round.
#[test]
fn each_loop_reads_its_own_variables() {
    let test = r#"
from "core/list" import * as list;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

fn total(xs: [Int], i: Int, acc: Int): Int {
    if (i == xs.length()) { acc } else { total(xs, i + 1, acc + xs.get(i).withDefault(0) * (i + 1)) }
}

fn rounds(xs: [Int], k: Int, acc: Int): Int {
    if (k == 0) { acc } else { rounds(xs, k - 1, acc * 31 % 1000003 + total(xs, 0, k)) }
}

test "loops" {
    let ctx = context { Allocator: alloc() };
    let xs = list.range(ctx, 0, 300).map(ctx, fn(i) => (i * 7919) % 307);
    let ys = xs.sort(ctx);
    let zs = ys.filter(ctx, fn(x) => x % 3 != 0).map(ctx, fn(x) => x * 2 - 1);
    let back = xs.sortBy(ctx, fn(p, q) => q.compare(p));
    let bytes: [I8] = [3, -128, 127, 0, -1, 5];
    assert.equal(ys.first().withDefault(-1), 0);
    assert.equal(ys.get(299).withDefault(-1), 306);
    assert.equal(back.first().withDefault(-1), 306);
    assert.equal(zs.length(), 201);
    assert.equal(total(zs, 0, 0), 8298507);
    assert.equal(rounds(ys, 20, 1), 9845006);
    let sorted = bytes.sort(ctx);
    assert.equal(sorted.get(0).withDefault(0), -128);
    assert.equal(sorted.get(1).withDefault(0), -1);
    assert.equal(sorted.get(4).withDefault(0), 5);
    assert.equal(sorted.get(5).withDefault(0), 127);
}
"#;
    crate::odd_sizes::passes("loop-registers", test, 1);
}

/// **A loop hands each variable the value meant for it**: two that trade
/// places every round, as integers and as floats, and one that takes a
/// record's field.
#[test]
fn each_loop_variable_gets_its_own_next_value() {
    let test = r#"
from "core/testing/assert" import * as assert;

struct R { k: Int, j: Int }

fn swaps(a: Int, b: Int, n: Int, acc: Int): Int {
    if (n == 0) { acc } else { swaps(b, a, n - 1, acc + a * 3 - b) }
}

fn fields(i: Int, s: Int, r: R): Int {
    if (i > 1000) { s * 10000 + i } else { fields(i + s % 7 + 1, r.k, r) }
}

fn mixed(a: Int, b: Int, x: Float, y: Float, n: Int): Float {
    if (n <= 0) {
        x * 3.0 - y
    } else if (a > b && x > y) {
        mixed(b, a, y, x, n - 1)
    } else {
        mixed(b, a, y, x, n - 2)
    }
}

test "next values" {
    assert.equal(swaps(5, 2, 11, 0), 83);
    assert.equal(swaps(-4, 9, 10, 1), 51);
    assert.equal(fields(0, 3, R { k: 12, j: 0 }), 121006);
    assert.equal(mixed(1, 2, 0.5, 4.0, 3), -2.5);
    assert.equal(mixed(1, 2, 0.5, 4.0, 4), 11.5);
    assert.equal(mixed(3, 2, 0.5, 4.0, 4), -2.5);
    assert.equal(mixed(3, 2, 0.5, 4.0, 6), 11.5);
}
"#;
    crate::odd_sizes::passes("loop-swaps", test, 1);
}

/// **A loop's next value is read where it was computed**: `i + 1` read again
/// before the loop takes it, by arithmetic and by a record, with the loop's
/// own `i` read once it ends.
#[test]
fn a_next_value_read_before_the_loop_takes_it() {
    let test = r#"
from "core/testing/assert" import * as assert;

struct R { k: Int, j: Int }

fn step(i: Int, n: Int, acc: Int): Int {
    if (i >= n) { acc * 7 + i } else {
        let i1 = i + 1;
        step(i1, n, acc + i1 * 3 % 11)
    }
}

fn kept(i: Int, n: Int, r: R): Int {
    if (i >= n) { r.k * 5 + r.j + i } else {
        let i1 = i + 1;
        kept(i1, n, R { k: i1, j: r.j + i1 % 4 })
    }
}

test "next values read again" {
    assert.equal(step(0, 1000, 0), 36035);
    assert.equal(step(5, 3, 2), 19);
    assert.equal(kept(0, 999, R { k: 0, j: 0 }), 7494);
}
"#;
    crate::odd_sizes::passes("loop-next-read", test, 1);
}

/// **A next value computed before a branch reaches every way back**: `k + 1`
/// computed before the loop picks a way and handed on by each way, by ways
/// nested two deep, and also read on one way. And ways back each behind a
/// test whose other way out reads the loop's own `i`, not `i + 1`.
#[test]
fn a_next_value_computed_before_a_branch_reaches_every_way_back() {
    let test = r#"
from "core/testing/assert" import * as assert;

fn two(i: Int, j: Int, k: Int, n: Int, acc: Int): Int {
    if (k >= n) { acc * 1000 + i * 10 + j } else {
        let k1 = k + 1;
        if ((i * 7 + j * 3) % 5 < 2) { two(i + 1, j, k1, n, acc + k * i) } else { two(i, j + 1, k1, n, acc - k + j) }
    }
}

fn read(i: Int, k: Int, n: Int, acc: Int): Int {
    if (k >= n) { acc * 100 + i } else {
        let k1 = k * 3 - 1;
        if (i % 3 == 0) { read(i + 1, k1, n, acc + k1 % 7) } else { read(i + 2, k1, n, acc) }
    }
}

fn deep(i: Int, k: Int, n: Int, acc: Int): Int {
    if (k >= n) { acc + i } else {
        let k1 = k + 2;
        if (i % 2 == 0) {
            if (i % 3 == 0) { deep(i + 1, k1, n, acc + 1) } else { deep(i + 3, k1, n, acc + 2) }
        } else if (i % 5 == 0) {
            if (i % 7 == 0) { deep(i + 5, k1, n, acc * 2 % 1009) } else { deep(i + 7, k1, n, acc + 3) }
        } else {
            deep(i + 1, k1, n, acc - 1)
        }
    }
}

fn out(i: Int, n: Int, acc: Int): Int {
    if (i >= 1000000) { acc } else {
        let j = i + 1;
        if (acc % 3 == 0) {
            let a2 = acc + 2;
            if (a2 > n) { a2 * 7 + i } else { out(j, n, a2) }
        } else {
            let a3 = acc + 1;
            if (a3 > n) { a3 * 5 + i } else { out(j, n, a3) }
        }
    }
}

test "next values before a branch" {
    assert.equal(out(0, 5000, 0), 28338);
    assert.equal(out(10, 77, 1), 451);
    assert.equal(out(999990, 1000000000000, 0), 15);
    assert.equal(two(0, 0, 0, 100, 0), 79575550);
    assert.equal(two(3, 1, 5, 4, 9), 9031);
    assert.equal(two(2, 5, 0, 57, -3), 16004334);
    assert.equal(read(0, 1, 100000, 0), 1018);
    assert.equal(read(1, 2, 1000000000, 5), 2030);
    assert.equal(deep(0, 0, 1000, 0), 1767);
    assert.equal(deep(1, 0, 333, 7), 1294);
}
"#;
    crate::odd_sizes::passes("loop-next-branch", test, 1);
}

/// **A loop's variables are read after it ends**: totals read by the code
/// after the loop, an `Int` and a `Float`, a loop with two ways out reading
/// different totals, and an inner loop's total read before the outer loop
/// goes round again. And loops left into a block that only tests, left from
/// a block that has just computed the next value, and left two ways into one
/// block, by an `Int` and a `Float` handed back unchanged. And a `match` whose
/// two patterns leave the loop into one arm, reading a `Float` handed back
/// unchanged and one stepped by it or by a constant.
#[test]
fn a_loops_variables_are_read_after_it_ends() {
    let test = r#"
from "core/testing/assert" import * as assert;

fn fsum(i: Int, n: Int, x: Float): Float {
    if (i >= n) { x * 2.0 } else { fsum(i + 1, n, x + 0.5) }
}

fn exits(i: Int, a: Int, b: Int): Int {
    if (a > 5000) { a * 3 + i } else if (b > 7000) { b - i } else { exits(i + 1, a + i % 11, b + i % 13) }
}

fn inner(j: Int, m: Int, acc: Int): Int {
    if (j >= m) { acc } else { inner(j + 1, m, acc + j * m % 17) }
}

fn outer(i: Int, n: Int, acc: Int): Int {
    if (i >= n) { acc } else {
        let t = inner(0, i, 0);
        outer(i + 1, n, acc * 3 % 100003 + t)
    }
}

fn tested(i: Int, n: Int, acc: Int): Int {
    if (i >= n) { if (acc > 5) { acc } else { 0 - acc } } else { tested(i + 1, n, acc + i % 7) }
}

fn moved(i: Int, n: Int, acc: Int): Int {
    let j = i + 1;
    if (j >= n) { acc + i } else { moved(j, n, acc + j % 5) }
}

fn joined(i: Int, n: Int, k: Int): Int {
    if (i >= n || k > 1000000) { k + n + i } else { joined(i + 1, n, k) }
}

fn fjoined(i: Int, n: Int, x: Float): Float {
    if (i >= n || x > 1000.0) { x * 2.0 } else { fjoined(i + 1, n, x) }
}

fn either(i: Int, n: Int, k: Float, x: Float): Float {
    match ((i >= n, x > 1000.0)) {
        (true, _) | (_, true) => x + k,
        _ => either(i + 1, n, k, x + k),
    }
}

fn halves(i: Int, n: Int, k: Float, x: Float): Float {
    match ((i >= n, x > 1000.0)) {
        (true, _) | (_, true) => x + k,
        _ => halves(i + 1, n, k, x + 0.5),
    }
}

test "read after the loop" {
    assert.equal(either(0, 40, 2.0, 1.25), 83.25);
    assert.equal(either(0, 1000, 3.0, 0.5), 1005.5);
    assert.equal(halves(0, 40, 2.0, 1.25), 23.25);
    assert.equal(halves(0, 5000, 2.0, 0.0), 1002.5);
    assert.equal(tested(0, 100, 0), 295);
    assert.equal(tested(0, 2, 0), -1);
    assert.equal(moved(0, 100, 0), 299);
    assert.equal(moved(0, 1, 3), 3);
    assert.equal(joined(0, 50, 7), 107);
    assert.equal(joined(0, 50, 2000000), 2000050);
    assert.equal(fjoined(0, 40, 1.25), 2.5);
    assert.equal(fjoined(0, 40, 2000.0), 4000.0);
    assert.equal(fsum(0, 1000, 0.25), 1000.5);
    assert.equal(exits(0, 0, 0), 16016);
    assert.equal(exits(50, 4990, 0), 15061);
    assert.equal(exits(3, 0, 6990), 6996);
    assert.equal(outer(0, 60, 1), 96863);
}
"#;
    crate::odd_sizes::passes("loop-read-after", test, 1);
}

/// **A sort by a comparator it calls keeps every element in order**: lists of
/// every length up to 69, sorted by a function of the program's own over a
/// record, so the sort's merge calls it for each element and its loop hands
/// each next output index on both on a way back and on a way out.
#[test]
fn a_sort_by_a_called_comparator_keeps_every_element_in_order() {
    let test = r#"
from "core/list" import * as list;
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

struct F { n: Int, name: Str }

fn cmp(a: F, b: F): Order {
    if (a.n == b.n) { a.name.compare(b.name) } else if (a.n < b.n) { .Less } else { .Greater }
}

fn sorted(xs: [F], i: Int): Bool {
    if (i + 1 >= xs.length()) { true } else {
        let a = xs.get(i).withDefault(F { n: 0, name: "" });
        let b = xs.get(i + 1).withDefault(F { n: 0, name: "" });
        if (cmp(a, b) == .Greater) { false } else { sorted(xs, i + 1) }
    }
}

fn total(xs: [F], i: Int, acc: Int): Int {
    if (i >= xs.length()) { acc } else { total(xs, i + 1, acc + xs.get(i).withDefault(F { n: 0, name: "" }).n) }
}

fn wrong<C: Allocator>(ctx: C, m: Int): Bool {
    let xs = list.range(ctx, 0, m).map(ctx, fn(i) => F { n: (i * 7919) % 13, name: if (i % 2 == 0) { "a" } else { "b" } });
    let ys = xs.sortBy(ctx, cmp);
    !sorted(ys, 0) || ys.length() != m || total(ys, 0, 0) != total(xs, 0, 0)
}

fn count<C: Allocator>(ctx: C, m: Int, acc: Int): Int {
    if (m >= 70) { acc } else { count(ctx, m + 1, if (wrong(ctx, m)) { acc + 1 } else { acc }) }
}

test "called comparator" {
    let ctx = context { Allocator: alloc() };
    assert.equal(count(ctx, 0, 0), 0);
}
"#;
    crate::odd_sizes::passes("loop-called-sort", test, 1);
}

/// `main` takes a count `n` and runs a loop of `n` steps, then prints what
/// it answers when it ends, `EXIT`: either something the code after the loop
/// reads both variables for, or nothing that reads them.
const READ_AFTER: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Environment, Stdout };

fn sum(i: Int, n: Int, acc: Int): Int {
  if (i >= n) { EXIT } else { sum(i + 1, n, acc + i % 7) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Environment: host.env, Stdout: host.stdout };
  let n = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${sum(0, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The two counts of steps [`READ_AFTER`] is measured between: 30 million
/// apart, which is about 360 million instructions between the two runs on
/// copy-and-patch.
const READ_AFTER_STEPS: (u64, u64) = (10_000_000, 40_000_000);

/// **A loop read after it ends costs a step what one not read after does.**
/// The copy-and-patch backend wrote each variable the code after the loop
/// reads to its frame slot on every way back, a store a variable a step. Now
/// it writes each once, where the loop is left. In instructions a step, the
/// loop read after cost this much more than the one not read after: 1.0 on
/// macOS, now 0.0. The bound is 0.5, halfway, many times the noise of a
/// difference of 30 million steps, which is under 0.001 a step.
#[test]
fn a_loop_read_after_it_ends_keeps_its_variables_in_registers() {
    let (fewer, more) = READ_AFTER_STEPS;
    let steps = more - fewer;
    for (backend, build) in crate::e2e::probed_backends() {
        // LLVM drops a loop whose answer is not read.
        if backend != "stencil" {
            continue;
        }
        let per = |name: &str, exit: &str, answer: fn(u64) -> String| {
            let binary = build(name, &READ_AFTER.replace("EXIT", exit));
            crate::collection_costs::steps_cost(backend, &binary, READ_AFTER_STEPS, answer)
        };
        let read = per("loop-read-after-steps", "acc * 3 + i", |n| {
            let acc: u64 = (0..n).map(|i| i % 7).sum();
            format!("{}\n", acc * 3 + n)
        });
        let unread = per("loop-unread-after-steps", "0", |_| String::from("0\n"));
        eprintln!("{backend}: {read:?} and {unread:?} instructions");
        if let Some((read, unread)) = read.zip(unread) {
            // In tenths of an instruction a step.
            let over = read.saturating_sub(unread) * 10 / steps;
            assert!(
                over < READ_AFTER_BOUND_TENTHS,
                "{backend}: {read} instructions read after, {unread} not, \
                 {over} tenths a step apart"
            );
        }
    }
}

/// See [`a_loop_read_after_it_ends_keeps_its_variables_in_registers`].
const READ_AFTER_BOUND_TENTHS: u64 = 5;
