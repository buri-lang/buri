//! Contexts handed from call to call: one with state read on every effect
//! call, and a test context of doubles passed down a recursion, twice to one
//! callee, and stored by its callee.
//!
//! One `buri test` over one repository, under the heap check, on every native
//! backend built in and on JavaScript.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-context-passing-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

const LIBRARY: &str = r#"from "core/fs" import * as fs;
from "core/fs" import { FileSystemRead };
from "core/path" import * as path;
from "core/random" import * as random;
from "platform/effect" import { Allocator, Random };

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

export fn climb<C: Random>(ctx: C, n: Int): Int {
    if (n == 0) { 0 } else { random.int(ctx, n, 1) + climb(ctx, n - 1) }
}

export fn fan<C: Allocator + FileSystemRead>(ctx: C, n: Int, at: Str): Int {
    if (n == 0) {
        if (fs.exists(ctx, path.of(ctx, at))) { 1 } else { 0 }
    } else {
        fan(ctx, n - 1, at) + fan(ctx, n - 1, at)
    }
}

export struct Kept<C> {
    export held: C,
    export n: Int,
}

export fn keep<C>(ctx: C, n: Int): Kept<C> {
    Kept { held: ctx, n: n }
}

export fn keepAll<C: Allocator>(ctx: C, n: Int): [Kept<C>] {
    if (n == 0) { [] } else { keepAll(ctx, n - 1).push(ctx, keep(ctx, n)) }
}

export fn has<C: Allocator + FileSystemRead>(ctx: C, at: Str): Bool {
    fs.exists(ctx, path.of(ctx, at))
}

/// Four words, one of them on the heap.
export struct Quad {
    export name: Str,
    export n: Int,
}

export fn pair(a: Quad, b: Quad): Int {
    a.n * 10 + b.n + a.name.length() * 1000 + b.name.length() * 100000
}
"#;

const TESTS: &str = r#"from "core/fs" import { FileSystemRead };
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator, Random };
from "platform/effect/testing" import { alloc, fs };
from "core/str" import * as str;
from "//lib/ctx" import { Quad, climb, fan, has, keep, keepAll, mul, named, pair, spin };

test "a context with state through many calls" {
    let ctx = context { Random: mul(31) };
    assert.equal(spin(ctx, 0, 100000, 1), 50276765289);
    assert.equal(climb(ctx, 2000), 62033000);
}

test "a context whose one field is on the heap" {
    let base = context { Allocator: alloc() };
    let ctx = context { Random: named(str.format(base, "seed ${12345}")) };
    assert.equal(spin(ctx, 0, 1000, 1), 506608599);
    assert.equal(climb(ctx, 1000), 5006000);
}

test "a test context with doubles down a recursion" {
    let ctx = context { Allocator: alloc(), FileSystemRead: fs().files([("a.txt", "hi")]) };
    assert.equal(fan(ctx, 8, "a.txt"), 256);
    assert.equal(fan(ctx, 3, "b.txt"), 0);
}

test "one aggregate passed twice" {
    let ctx = context { Allocator: alloc() };
    let q = Quad { name: str.format(ctx, "q${7}"), n: 3 };
    assert.equal(pair(q, q), 202033);
    assert.equal(pair(q, Quad { name: "four", n: 5 }), 402035);
}

test "a callee that stores the context it was passed" {
    let ctx = context { Allocator: alloc(), FileSystemRead: fs().files([("a.txt", "hi")]) };
    let one = keep(ctx, 7);
    assert.equal(has(one.held, "a.txt"), true);
    let empty = keep(context { Allocator: alloc(), FileSystemRead: fs() }, 0);
    assert.equal(has(empty.held, "a.txt"), false);
    let kept = keepAll(ctx, 50);
    assert.equal(kept.length(), 50);
    let total = kept.foldCtx(
        ctx,
        fn(c, acc, k) => acc + k.n + (if (has(k.held, "a.txt")) { 1 } else { 0 }),
        0,
    );
    assert.equal(total, 1325);
}
"#;

#[test]
fn contexts_passed_from_call_to_call() {
    let repo = workspace("passing");
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
        cmd.current_dir(&repo).arg("test").args(flags).arg("//lib/ctx");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert!(
            ran.status == 0 && ran.stdout.contains("5 passed, 0 failed"),
            "{backend}: contexts passed from call to call:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}
