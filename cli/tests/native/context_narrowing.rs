//! A function generic over its caller's context, called from contexts that
//! bind other effects as well, in other orders: directly, through a trait
//! method, from a callee that narrowed already, with a context read out of a
//! field, spawning into a scope its caller opened, and running an actor.
//!
//! One `buri test` over one repository, under the heap check, on every native
//! backend built in and on JavaScript.

use std::path::Path;
use std::process::Command;

fn workspace(name: &str) -> crate::sweep::Owned {
    crate::sweep::owned("native-context-narrowing", name)
}

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

const LIBRARY: &str = r#"from "core/actor" import * as actor;
from "core/actor" import { Actor, Stepped };
from "core/io" import * as io;
from "core/random" import * as random;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "core/tasks" import { Scope };
from "platform/effect" import { Allocator, Random, Stdout, Tasks };

/// One word of state.
export struct Mul(Int);

export fn mul(k: Int): Mul {
    Mul(k)
}

impl Random for Mul {
    fn nextInt(self, low: Int, high: Int): Int {
        (low * self.0 + high) % 1000003
    }

    fn nextFloat(self): Float {
        0.5
    }
}

/// One field, on the heap.
export struct Named(Str);

export fn named(name: Str): Named {
    Named(name)
}

impl Random for Named {
    fn nextInt(self, low: Int, high: Int): Int {
        (low * self.0.length() + high) % 1000003
    }

    fn nextFloat(self): Float {
        0.5
    }
}

export fn spin<C: Random>(ctx: C, i: Int, n: Int, acc: Int): Int {
    if (i == n) { acc } else { spin(ctx, i + 1, n, acc + random.int(ctx, i, acc)) }
}

export fn label<C: Random + Allocator>(ctx: C, n: Int): Str {
    str.format(ctx, "roll ${random.int(ctx, n, 7)}")
}

/// Hands its context, narrowed already, to callees that name fewer effects.
export fn both<C: Allocator + Random>(ctx: C, n: Int): Int {
    spin(ctx, 0, n, 1) + label(ctx, n).length()
}

export trait Die {
    fn roll<C: Random>(self, ctx: C): Int;
}

export struct D6(export Int);

impl Die for D6 {
    fn roll<C: Random>(self, ctx: C): Int {
        random.int(ctx, self.0, 6)
    }
}

export struct Held<C> {
    export held: C,
}

export fn hold<C>(ctx: C): Held<C> {
    Held { held: ctx }
}

/// Rolls any die at a context held in a field, so the roll's context is the
/// field's.
export fn rolled<C: Random, D: Die>(ctx: Held<C>, die: D): Int {
    die.roll(ctx.held)
}

/// Queues a task in a scope its caller opened, whose drain runs it.
export fn queue<C: Allocator + Stdout + Tasks>(ctx: C, here: Scope, line: Str): () {
    tasks.spawn(ctx, here, fn(c) => {
        let _ = io.println(c, line).ignore();
        ()
    })
}

/// Sums `1..=n` in an actor whose step prints through the context.
export fn tally<C: Allocator + Stdout + Tasks>(ctx: C, n: Int): Int {
    let address = actor.start(ctx, Actor {
        state: 0,
        step: fn(c, total, k) => {
            let _ = io.println(c, "add ${k}").ignore();
            Stepped { state: total + k, answer: total + k }
        },
        onStop: .None,
    });
    let last = sends(ctx, address, 1, n, 0);
    let _ = address.stop(ctx).isOk();
    last
}

fn sends<C: Allocator + Stdout + Tasks>(
    ctx: C,
    address: actor.Address<C, Int, Int, Int>,
    k: Int,
    n: Int,
    last: Int,
): Int {
    if (k > n) { last } else { sends(ctx, address, k + 1, n, address.sendMessage(ctx, k).withDefault(-1)) }
}

/// Prints through a context, beside tasks that print through it too.
export fn say<C: Stdout>(ctx: C, line: Str): () {
    let _ = io.println(ctx, line).ignore();
    ()
}
"#;

const TESTS: &str = r#"from "core/fs" import { FileSystemRead };
from "core/tasks" import * as tasks;
from "core/testing/assert" import * as assert;
from "core/str" import * as str;
from "platform/effect" import { Allocator, Clock, Random, Stdout, Tasks };
from "platform/effect/testing" import { alloc, clock, fs, stdout };
from "platform/effect/testing" import { tasks as taskDouble };
from "//lib/ctx" import { D6, Die, both, hold, label, mul, named, queue, rolled, say, spin, tally };

test "a context that binds the one effect called" {
    let ctx = context { Random: mul(31) };
    assert.equal(spin(ctx, 0, 1000, 1), spin(context { Random: mul(31) }, 0, 1000, 1));
    assert.equal(label(context { Random: mul(3), Allocator: alloc() }, 5), "roll 22");
}

test "contexts that bind other effects as well, in any order" {
    let one = spin(context { Random: mul(31) }, 0, 1000, 1);
    let wide = context { Allocator: alloc(), Random: mul(31), FileSystemRead: fs() };
    assert.equal(spin(wide, 0, 1000, 1), one);
    let other = context { Random: mul(31), FileSystemRead: fs(), Allocator: alloc() };
    assert.equal(spin(other, 0, 1000, 1), one);
    assert.equal(label(wide, 5), "roll 162");
    assert.equal(label(other, 5), "roll 162");
}

test "an implementation on the heap reached through a wider context" {
    let base = context { Allocator: alloc() };
    let seed = named(str.format(base, "seed ${12345}"));
    let narrow = spin(context { Random: seed }, 0, 1000, 1);
    let wide = context { FileSystemRead: fs(), Random: seed, Allocator: alloc() };
    assert.equal(spin(wide, 0, 1000, 1), narrow);
    assert.equal(label(wide, 2), "roll 27");
}

test "a callee that narrowed hands its context on" {
    let a = context { Allocator: alloc(), Random: mul(31) };
    let b = context { FileSystemRead: fs(), Random: mul(31), Allocator: alloc() };
    assert.equal(both(a, 100), both(b, 100));
    assert.equal(both(b, 0), 1 + "roll 7".length());
}

test "a trait method generic over its context" {
    let die = D6(4);
    assert.equal(die.roll(context { Random: mul(5) }), 26);
    assert.equal(die.roll(context { Allocator: alloc(), Random: mul(5) }), 26);
}

test "a trait method called on a generic receiver from a wider context" {
    let wide = hold(context { FileSystemRead: fs(), Random: mul(5), Allocator: alloc() });
    assert.equal(rolled(wide, D6(4)), 26);
    assert.equal(rolled(hold(context { Random: mul(5) }), D6(4)), 26);
}

test "a context read out of a field" {
    let kept = hold(context { Allocator: alloc(), Random: mul(31), FileSystemRead: fs() });
    assert.equal(spin(kept.held, 0, 1000, 1), spin(context { Random: mul(31) }, 0, 1000, 1));
    assert.equal(spin(hold(kept.held).held, 0, 10, 1), spin(context { Random: mul(31) }, 0, 10, 1));
}

test "a task queued from a wider context runs in its caller's scope" {
    let out = stdout();
    let ctx = context {
        Allocator: alloc(),
        Clock: clock(),
        Random: mul(3),
        Stdout: out,
        Tasks: taskDouble(),
    };
    let _ = tasks.scope(ctx, fn(c, here) => {
        let _ = queue(c, here, "queued a");
        let _ = tasks.spawn(c, here, fn(c2) => say(c2, "spawned"));
        let _ = queue(c, here, "queued b");
        say(c, "body")
    });
    assert.equal(out.captured(), "body\nqueued a\nspawned\nqueued b\n");
}

test "an actor run from a wider context" {
    let out = stdout();
    let ctx = context {
        Random: mul(3),
        Tasks: taskDouble(),
        Stdout: out,
        Clock: clock(),
        Allocator: alloc(),
    };
    assert.equal(tally(ctx, 3), 6);
    assert.equal(out.captured(), "add 1\nadd 2\nadd 3\n");
}
"#;

#[test]
fn a_context_narrowed_to_the_effects_a_callee_names() {
    let repo = workspace("narrowing");
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/ctx/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/ctx.buri\"]\n    }\n}\n",
    );
    write(&repo.join("lib/ctx/lib.buri"), LIBRARY);
    write(&repo.join("lib/ctx/test/ctx.buri"), TESTS);

    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo)
            .arg("test")
            .args(flags)
            .arg("//lib/ctx");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert!(
            ran.status == 0 && ran.stdout.contains("9 passed, 0 failed"),
            "{backend}: a context narrowed to the effects a callee names:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}
