# Effects and capabilities

A Buri signature says what a function may do to the world, in one place: a
parameter named `ctx`, and the bounds written on its type.

```buri
# from "core/effect" import { Allocator };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };

/// No `ctx`, so this cannot allocate, print, read a file, or open a socket —
/// and neither can anything it calls.
fn shortfall(score: Int, needed: Int): Int {
    needed - score
}

/// `Allocator + FileSystemRead` is the whole of what this may do. `FileSystemWrite` is not on the
/// list, so it cannot delete the file it just read; `Stdout` is not either, so
/// it cannot print, however much its caller can.
fn load<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, Str> {
    fs.readText(ctx, at).mapErr(fn(e) => "could not read the file")
}
```

## An effect is an interface, and the set of them is fixed

An **effect** is an interface declared with `effect` instead of `trait`, and its
methods are the operations it grants. `core/effect` declares most of them:
`Allocator`, `Network`, `Clock`, `Random`, `Environment`, `Stdin`, `Stdout`, `Stderr`, `Process`,
`Tasks`, `Listen`, `Sockets` and `WebSocketClient`. `core/fs` is a platform
module too, and it declares the filesystem's `FileSystemRead` and `FileSystemWrite`. **Only a platform module may
declare an effect**, so the set of things a Buri program can do to the world is
closed.

The filesystem is two effects because it is two grants: a program that reads its
configuration has not thereby earned the right to delete it. Nothing a function
passes `ctx` to can ask for `FileSystemWrite` from a context that does not bind it.

Otherwise an effect is a trait: same declaration shape, same nominal
conformance, same `impl`, same bounds. Two rules keep the two apart. You may
pass an effect-carrying value only as `self` or `ctx`, and no type may implement
both an effect and a trait, so a `T: Ord` is never secretly a context. Together
they make one sentence true: **a function is effectful if and only if it has a
`ctx` parameter or an effect-carrying `self`.**

You do not perform an effect *on* the context: `io.println(ctx, text)` rather
than `ctx.println(text)`. The operation is a free function in the module that
wraps the effect, which splits *which* effect from *what* it does.

## Authority starts at `core/host` and passes through `main`

The implementations that really do something live in `core/host`. It exports one
value per effect the platform grants — `host.alloc`, `host.stdout`, `host.fs`,
`host.net` and the rest — and **only the module that exports `main` may import
it**. `main` takes no parameters. It names the effects the program is to have,
binds each to an implementation, and hands the result down:

```buri
# from "core/effect" import { Allocator, Stdout };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };
from "core/host" import * as host;
# from "core/io" import * as io;
# from "core/path" import * as path;

# fn load<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, Str> {
#     fs.readText(ctx, at).mapErr(fn(e) => "could not read the file")
# }

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        FileSystemRead: host.fs,
    };
    let text = load(ctx, path.of(ctx, "notes.txt"))?;
    io.println(ctx, text).mapErr(fn(e) => "could not print")
}
```

That `context` block is the program's entire effect budget, and you audit it by
reading it. It binds `FileSystemRead` and not `FileSystemWrite`, so this program cannot write a
file, and it cannot open a socket in its own code, a dependency or a build
script, because nothing anywhere can obtain a value bounded by `Network`. A platform
that does not grant an effect does not export it, so asking for one is a compile
error on the line that asked: `effect-not-on-platform`.

## Giving a callee less is naming fewer bounds

You hand a callee less authority by naming fewer bounds. It receives the same
value, and cannot use or pass on anything its bounds omit:

```buri
# from "core/effect" import { Allocator, Stdout };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };
# from "core/host" import * as host;
# from "core/io" import * as io;
# from "core/path" import * as path;

fn logOnly<C: Stdout>(ctx: C, msg: Str, at: Path): () {
    let _ = io.println(ctx, msg).ignore();
    let _f = fs.readText(ctx, at); // ERROR: `C` does not satisfy `FileSystemRead`
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        FileSystemRead: host.fs,
    };
    // same value, confined by its bound
    let _ = logOnly(ctx, "starting", path.of(ctx, "notes.txt"));
    .Ok(())
}
```

No copy, no wrapper, no runtime cost. The confinement is transitive: `C` is
opaque at every downstream call site, so `logOnly` cannot hand its context to
anything that asks for more than it has.

At a trust boundary you may want the *value* to lack the effect rather than
merely be unable to name it. Then wrap the context in a type that satisfies
fewer effects:

```buri
# from "core/effect" import { Allocator, IoError, Region };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };

export struct ReadOnly<C>(C);

// Forwards allocation...
impl<C: Allocator> Allocator for ReadOnly<C> {
    fn allocate(self, bytes: Int): Region {
        self.0.allocate(bytes)
    }
}

// ...and reading. Nothing anywhere implements `FileSystemRead` for `ReadOnly<C>`, so it
// satisfies neither half of the filesystem, whatever `C` is.
impl<C: Allocator + FileSystemRead> ReadOnly<C> {
    export fn readText(self, at: Path): Result<Str, IoError> {
        fs.readText(self.0, at)
    }
}
```

Attenuation narrows the whole context rather than subtracting one effect, so the
callee still holds exactly one effect-carrying value. Use bounds by default, a
wrapper at a boundary.

## Test doubles fall out for free

An effect is an ordinary interface, so an implementation is a struct with
methods, and the standard library has written the ones a test wants.
`core/host/testing` is `core/host`'s surface for a test source: `alloc()`,
`fs()`, `clock()`, `net()` and the rest, each real where it can be and hermetic
everywhere else. A test builds its context the way `main` does, and the code
under test does not change:

```buri role=test
# from "core/effect" import { Allocator };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };
from "core/host/testing" import { alloc, fs as memory };
# from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn load<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, Str> {
#     fs.readText(ctx, at).mapErr(fn(e) => "could not read the file")
# }

test "load reads the file it is given" {
    let ctx = context {
        Allocator: alloc(),
        FileSystemRead: memory().files([("notes.txt", "hello")]),
    };
    assert.eq(load(ctx, path.of(ctx, "notes.txt")), .Ok("hello"));
}
```

One `fs()` answers both `FileSystemRead` and `FileSystemWrite`, so a test that writes and then
reads back binds the *same* value under both names — `let disk = memory(); ...
FileSystemRead: disk, FileSystemWrite: disk` — because two calls would be two filesystems with
nothing in common.

[`reference/build/testing.md`](../reference/build/testing.md#the-runners-context)
has every double the runner ships and what each one does.

## `Allocator` is an effect, and that is the point

Allocation is on the same list as the filesystem, so "does no I/O" and "does not
allocate" are separately visible in a signature. A function whose only effect
bound is `Allocator` is **deterministic**: `xs.map(ctx, f)` allocates and is
otherwise referentially transparent, while `time.now(ctx)` is not. Only a result
whose size depends on runtime data needs it. Struct literals, tuples, enum
payloads, array literals, closures and templates never do.

`Allocator` is the one effect whose implementation grants nothing: `allocate`
answers a region, which is a number nothing reads. So `core/alloc` ships
`generalPurpose()`, `arena()` and `fixedBuffer(n)`, and you may import it
anywhere rather than only from `main`. Binding one says what a program is
willing to spend, not what authority it holds.

## The exact rules

[`language/effects.md`](../language/effects.md) is the specification: what makes
a type effect-carrying, why a lambda may not capture one, the purity theorem and
its three qualifiers, and the calling convention every signature above
follows.
