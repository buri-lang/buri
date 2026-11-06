//! A `context { ... }` built in a generic function, binding a value of the
//! function's type parameter: the effect it supplies is the value handed in,
//! whatever type each call hands.
//!
//! Each run is a real `buri test` over a repository, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in.

use crate::hand_written_impls::{every_backend, repository};

const BOUND: &str = r#"from "core/fs" import { FileSystemRead };
from "core/list" import * as list;
from "core/str" import * as str;
from "core/testing/assert" import * as assert;
from "core/time" import * as time;
from "platform/effect" import { Allocator, Clock };
from "platform/effect/testing" import { alloc, clock, fs };

context Fixture {
    Allocator: alloc(),
    Clock: clock().at(1_000),
    FileSystemRead: fs(),
}

fn elapsed<C: Clock>(ctx: C): Int {
    time.now(ctx).duration(time.now(clock())).milliseconds()
}

fn rebound<C: Clock>(ctx: C): Int {
    elapsed(context { ..Fixture(), Clock: ctx })
}

fn named<C: Allocator + Clock + FileSystemRead>(ctx: C, n: Int): Str {
    let at = elapsed(ctx);
    str.format(ctx, "${n} at ${at}")
}

fn allocating<C: Allocator>(ctx: C, n: Int): Str {
    let at = context {
        ..Fixture(),
        Allocator: ctx,
        Clock: clock().at(2_000),
    };
    named(at, n)
}

test "a clock bound from a type parameter is the one read" {
    assert.equal(rebound(clock().at(7)), 7);
    assert.equal(rebound(context { Clock: clock().at(9) }), 9);
    assert.equal(rebound(Fixture()), 1_000);
}

test "an allocator bound from a type parameter allocates" {
    assert.equal(allocating(alloc(), 1), "1 at 2000");
    assert.equal(allocating(Fixture(), 2), "2 at 2000");
    let ctx = Fixture();
    assert.equal(
        list.range(ctx, 0, 3).mapCtx(ctx, fn(c, k) => allocating(c, k)),
        ["0 at 2000", "1 at 2000", "2 at 2000"],
    );
}
"#;

/// A clock and an allocator, each bound in a context from a value of a type
/// parameter: a lone clock, a context of one, a whole fixture, and the context
/// a step of `mapCtx` is handed.
#[test]
fn a_context_binding_a_type_parameter_supplies_the_value_handed_in_on_every_backend() {
    let repo = repository("bound", BOUND);
    for (backend, status, stdout, stderr) in every_backend(&repo) {
        let context = format!("{backend}:\n{stdout}\n{stderr}");
        assert_eq!(status, 0, "{context}");
        assert!(stdout.contains("2 passed, 0 failed"), "{context}");
    }
}
