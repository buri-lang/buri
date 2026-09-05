# Effects and capabilities

A Buri signature says what a function may do to the world, and it says it in one
place. The mechanism is a parameter named `ctx` and the bounds written on its
type; everything below follows from those two things.

```buri
# from "core/effect" import { Alloc };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead, Path };

/// No `ctx`, so this cannot allocate, print, read a file, or open a socket —
/// and neither can anything it calls.
fn shortfall(score: Int, needed: Int): Int {
    needed - score
}

/// `Alloc + FsRead` is the whole of what this may do. `FsWrite` is not on the
/// list, so it cannot delete the file it just read; `Stdout` is not either, so
/// it cannot print, however much its caller can.
fn load<C: Alloc + FsRead>(ctx: C, at: Path): Result<Str, Str> {
    fs.readText(ctx, at).mapErr(fn(e) => "could not read the file")
}
```

## An effect is an interface, and the set of them is fixed

An **effect** is an interface declared with `effect` instead of `trait`, and its
methods are the operations it grants. `core/effect` declares most of them —
`Alloc`, `Net`, `Clock`, `Rand`, `Env`, `Stdin`, `Stdout`, `Stderr`, `Proc`,
`Tasks`, `Listen` and `Sockets` — and `core/fs`, a platform module too, declares
the filesystem's `FsRead` and `FsWrite`. **Only a platform module may declare an
effect**, so the set of things a Buri program can do to the world is closed.
Your own code cannot add to it.

The filesystem is two effects rather than one because it is two grants: a
program that reads its configuration has not thereby earned the right to delete
it. `<C: Alloc + FsRead>` is therefore a sentence the compiler holds a whole
call graph to — nothing that function passes `ctx` to can ask for `FsWrite` from
a context that does not bind it — and a program doing both binds both, which is
the price of the distinction being visible at all. The two live in `core/fs`
rather than in `core/effect` because every one of their methods names a `Path`,
and `core/path` names `Alloc`, so the declarations sit on the side of that
dependency where they can say what they mean.

Otherwise an effect is a trait: same declaration shape, same nominal
conformance, same `impl`, same bounds. `<C: Alloc + FsRead>` and `<T: Ord + Show>`
are the same feature, and the language has no second constraint mechanism. Two
rules keep effects and traits apart — an effect-carrying value may be passed
only as `self` or `ctx`, and no type may implement both an effect and a trait,
so a `T: Ord` is never secretly a context. Together they make one sentence true:
**a function is effectful if and only if it has a `ctx` parameter or an
effect-carrying `self`.** You never scan a signature to find out.

You also do not perform an effect *on* the context: `io.println(ctx, text)`
rather than `ctx.println(text)`. The operation is a free function in the module
that wraps the effect, which puts the authority where the reader is already
looking and splits *which* effect from *what* it does into two names.

## Authority starts at `core/host` and passes through `main`

The implementations that really do something live in `core/host`, which exports
one value per effect the platform grants — `host.alloc`, `host.stdout`,
`host.fs`, `host.net` and the rest — and **only the module that exports `main`
may import it**. `main` takes no parameters. It names the effects the program is
to have, binds each to an implementation, and hands the result down:

```buri
# from "core/effect" import { Alloc, Stdout };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead, Path };
from "core/host" import * as host;
# from "core/io" import * as io;
# from "core/path" import * as path;

# fn load<C: Alloc + FsRead>(ctx: C, at: Path): Result<Str, Str> {
#     fs.readText(ctx, at).mapErr(fn(e) => "could not read the file")
# }

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        FsRead: host.fs,
    };
    let text = load(ctx, path.of(ctx, "notes.txt"))?;
    io.println(ctx, text).mapErr(fn(e) => "could not print")
}
```

That `context` block is the program's entire effect budget, auditable by reading
it. It binds `FsRead` and not `FsWrite`, so this program cannot write a file
either — the half it was not given is as unreachable as the effects it never
mentioned. This program cannot open a socket — not in its own code, not in a
dependency, not in a build script — because nothing anywhere can obtain a value
bounded by `Net`, and there is no ambient `host` to reach for. A platform that
does not grant an effect does not export it at all, so asking for one you were
not given is a compile error on the line that asked — `effect-not-on-platform`,
reported while the file is being edited rather than when it is built.

## Giving a callee less is naming fewer bounds

Because effects are bounds, handing a callee less authority is naming fewer of
them. It receives the same value and cannot use — or pass on — anything its
bounds omit:

```buri
# from "core/effect" import { Alloc, Stdout };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead, Path };
# from "core/host" import * as host;
# from "core/io" import * as io;
# from "core/path" import * as path;

fn logOnly<C: Stdout>(ctx: C, msg: Str, at: Path): () {
    let _ = io.println(ctx, msg).ignore();
    let _f = fs.readText(ctx, at); // ERROR: `C` does not satisfy `FsRead`
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        FsRead: host.fs,
    };
    // same value, confined by its bound
    let _ = logOnly(ctx, "starting", path.of(ctx, "notes.txt"));
    .Ok(())
}
```

No copy, no wrapper, no runtime cost, and confinement is transitive — `C` is
opaque at every downstream call site, so `logOnly` cannot hand its context to
anything that asks for more than it has.

That is a fact about the type checker. When you want the *value* to lack the
effect rather than merely be unable to name it — at a trust boundary, where
something may later escape the type system — wrap the context in a type that
satisfies fewer effects:

```buri
# from "core/effect" import { Alloc, IoError, Region };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead, Path };

export struct ReadOnly<C>(C);

// Forwards allocation...
impl<C: Alloc> Alloc for ReadOnly<C> {
    fn allocate(self, bytes: Int): Region {
        self.0.allocate(bytes)
    }
}

// ...and reading, but there is deliberately no write here, and no
// `impl FsRead for ReadOnly<C>` anywhere, so a `ReadOnly<C>` satisfies neither
// half of the filesystem, whatever `C` is.
impl<C: Alloc + FsRead> ReadOnly<C> {
    export fn readText(self, at: Path): Result<Str, IoError> {
        fs.readText(self.0, at)
    }
}
```

Attenuation narrows the whole context rather than subtracting one effect from
it, which is what keeps the `ctx` rule satisfiable: the callee still holds
exactly one effect-carrying value. Use bounds by default, a wrapper at a
boundary.

## Test doubles fall out for free

An effect is an ordinary interface, so an implementation of one is a struct with
methods — and the standard library has already written the ones a test wants.
`core/host/testing` is `core/host`'s surface for a test source: `alloc()`,
`fs()`, `clock()`, `net()` and the rest, each real where it can be and hermetic
everywhere else. A test builds its context exactly the way `main` does, and the
code under test does not change, because there was never a global to stub:

```buri role=test
# from "core/effect" import { Alloc };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead, Path };
from "core/host/testing" import { alloc, fs as memory };
# from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn load<C: Alloc + FsRead>(ctx: C, at: Path): Result<Str, Str> {
#     fs.readText(ctx, at).mapErr(fn(e) => "could not read the file")
# }

test "load reads the file it is given" {
    let ctx = context {
        Alloc: alloc(),
        FsRead: memory().files([("notes.txt", "hello")]),
    };
    assert.eq(load(ctx, path.of(ctx, "notes.txt")), .Ok("hello"));
}
```

One `fs()` answers both `FsRead` and `FsWrite`, so a test that writes and then
reads back binds the *same* value under both names — `let disk = memory(); ...
FsRead: disk, FsWrite: disk` — because two calls would be two filesystems with
nothing in common.

There is no mocking framework, and nothing about `load` had to be written for
testability. [`reference/build/testing.md`](../reference/build/testing.md#the-runners-context)
has every double the runner ships and what each one does.

## `Alloc` is an effect, and that is the point

Allocation is on the same list as the filesystem, so "does no I/O" and "does not
allocate" are separately visible in a signature. A function whose only effect
bound is `Alloc` is **deterministic**: `xs.map(ctx, f)` allocates and is
otherwise referentially transparent, while `time.now(ctx)` is not. Only a result
whose size depends on runtime data needs it — struct literals, tuples, enum
payloads, array literals, closures and templates never do.

`Alloc` is also the one effect whose implementation grants nothing: `allocate`
answers a region, which is a number nothing reads. So `core/alloc` ships
`generalPurpose()`, `arena()` and `fixedBuffer(n)` and is importable anywhere
rather than only from `main`. Binding one is how a program says what it is
willing to spend, not how it acquires authority it was not given. Whether that
much bookkeeping is worth the guarantee is an open question, flagged as one in
`design/non-goals.md`.

## The exact rules

This page is the shape of the thing. [`language/effects.md`](../language/effects.md)
is the specification: what makes a type effect-carrying, why a lambda may not
capture one, the purity theorem and its three qualifiers, and the calling
convention every signature above follows.
