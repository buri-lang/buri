//! A function with several loops, built by `buri test` on every backend. The
//! copy-and-patch backend keeps each loop's variables in registers inside that
//! loop and in their frame slots outside it, and reads and writes an `Int`
//! element from a register, so a loop that read another loop's register, or an
//! element read at the wrong width, would show here.

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
