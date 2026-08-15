## 10. Effects and purity

This is the part of Buri that is not TypeScript and not Rust.

### 10.1 The model

An **effect** is an interface declared with `effect` instead of `trait`. Its
methods are the operations it grants:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// core/cap
export effect Alloc {
  fn allocate(self: Self, bytes: Int): Region;
}

export effect Stdout {
  fn print(self: Self, text: Template): ();
  fn println(self: Self, text: Template): ();
}

export effect Fs {
  fn readFile(self: Self, path: Str): Result<Str, IoError>;
  fn writeFile(self: Self, path: Str, body: Str): Result<(), IoError>;
}
```

`core/cap` declares `Alloc`, `Fs`, `Net`, `Clock`, `Rand`, `Env`, `Stdin`,
`Stdout`, `Stderr`, and `Proc`. **Only platform modules may declare effects**;
`effect` in ordinary code is a compile error, so the set of things a Buri program
can do to the world is fixed by its platform rather than open-ended.

An effect is a trait in every other respect — same declaration shape, same
nominal conformance, same `impl`, same bounds. Two rules separate them:

- an effect's implementors are **effect-carrying**, and so may be passed only as
  `self` or `ctx` (Section 10.2);
- **no type may implement both an effect and a trait.** A type is either part of
  the world or part of your data, and the boundary is checked rather than
  assumed.

A function names the effects it needs as **bounds** on its context parameter:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs };
fn loadConfig<C: Alloc + Fs>(ctx: C, path: Str): Result<Config, ConfigError> {
  let text = fs.readText(ctx, path)?;
  parse(ctx, text)
}
```

There is one constraint mechanism in the language. `<T: Ord + Show>` and
`<C: Alloc + Fs>` are the same feature: a list of interfaces a type parameter
must satisfy.

### 10.2 The `ctx` rule

**A effect-carrying parameter must be `self` or `ctx`** — never any other
name, never any other position, and at most one of each:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, IoError, Net, Region };
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>       // ok
fn render<C: Alloc>(self: Report, ctx: C): Str                            // ok
fn allocate(self: Self, bytes: Int): Region                               // ok
fn sneaky<C: Fs>(a: Int, handle: C): Bool                                 // ERROR
fn twoWorlds<A: Fs, B: Net>(ctx: A, other: B): {}                         // ERROR
```

A type is **effect-carrying** if it is a type variable with an effect
bound, or any type mentioning one — so a struct that stores a context is
effect-carrying too. A *function type* is effect-carrying when its **result**
is: `fn(C, A) => B` merely accepts a context, which is the shape the `*Ctx`
combinators of Section 10.6 take, while `fn() => C` produces one.

`self` has to be allowed because a effect's own methods take the
effect as their receiver (`fn allocate(self: Self, ...)`), and so do the
attenuation wrappers of Section 10.8. Outside those two places, effects arrive
through `ctx`.

There is exactly one construct in which more than one effect-carrying value may
appear, and it is the `context` expression of Section 11.3 — the place where a
context is assembled out of the implementations that make it up. Everywhere
else, capabilities travel through a single `ctx` parameter or an
effect-carrying `self`.

The rule costs a little flexibility — a function cannot take two independent
contexts; bundle them into one type instead — and buys the property the chapter
rests on:

> **A function is effectful if and only if it has a `ctx` parameter, or a
> effect-carrying `self`.**

Both are fixed positions with fixed names, so you read the first two parameters
and stop. You never scan a signature.

### 10.3 Where effects come from

The platform. `core/host` exports one value per effect the platform grants —
`host.alloc`, `host.stdout`, `host.stderr`, `host.stdin`, `host.fs`, `host.net`,
`host.clock`, `host.rand`, `host.env`, `host.proc` — and it is importable only
from the module that exports `main`. `main` assembles them into the one context
the program has:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
    Fs:     host.fs,
  };
  ...
}
```

The form is Section 11.3. What matters here is what it makes true: a program
that never names `host.net` cannot open a socket anywhere in its transitive call
graph — not in a dependency, not in a build script, not by accident, because
nothing anywhere can obtain a value bounded by `Net`. The effect budget is the
set of `host` members reachable from `main`'s context, and a platform that does
not grant an effect simply does not export it, so requesting one is an ordinary
unresolved-name error at the one line that asked for it.

Note what is *not* claimed: a effect is an ordinary interface, so
anyone may write a type that satisfies it.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Stdout };
struct SilentOut {}
fn writeOut(self: SilentOut, text: Template): () { () }    // satisfies Stdout
```

That is not a forgery hole — a fake `Stdout` still cannot write anything. What is
unforgeable is the *platform's* implementation. The open interface is what makes
testing free (Section 10.8).

### 10.4 What "pure" means

> **Purity theorem.** If a function has no `ctx` parameter, no
> effect-carrying `self`, captures no effect-carrying value, and constructs no
> context, then any two evaluations on equal arguments produce
> equal results, perform no observable effect, and may be freely cached,
> reordered, or eliminated.

Top-level functions capture nothing but other top-level declarations, which are
themselves effect-free, so for a top-level `fn` the theorem reduces to: *is
there a `ctx` parameter?*

The last clause exists because `main` has no parameters and is plainly not pure:
it builds a context and uses it. It is not a hole. A context may be constructed
only in `main`'s body, in a test source, or in a test-only module (Section
11.3), and none of those is a function anybody calls from library code — `main`
is the entry point, and a test source may not be imported. So in all ordinary
code the clause is vacuous, and the useful form of the theorem is unchanged.

Two consequences worth naming:

- Purity is not a keyword and not an effect annotation. It is the absence of one
  argument, in a fixed position, with a fixed name.
- The check is shallow and local. You never read a function body, or its
  callees' bodies, to know whether it can touch the world.

### 10.5 Determinism versus effects

`Alloc` is a **resource** effect: it can fail (out of memory) and it costs
something, but it is not observable. Every other effect in `core/cap` is
**observable**.

A function is **deterministic** if its only effect bound is `Alloc`.
`list.map(ctx, f)` is deterministic: it needs to allocate, but it is
referentially transparent. `time.now(ctx)` is not.

Tracking allocation is why `[T]`-returning combinators take a context at all, and
it is what makes "does no I/O" and "does not allocate" separately expressible:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, IoError };
fn sum(self: [Int]): Int                                              // pure
fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B]         // deterministic
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>   // effectful
```

Fixed-size construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Alloc`. Only results whose size depends
on runtime data do.

### 10.6 The capture rule

**A lambda may not capture a effect-carrying value.** Capabilities travel
through the `ctx` parameter only.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// ERROR: the lambda captures ctx
let texts = paths.map(ctx, fn(p) => fs.readText(ctx, p));

// Thread the context through a *Ctx combinator instead
let texts = paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p));
```

Without this rule, a value of type `fn(Str) => Str` could smuggle a file handle
past a signature with no `ctx` parameter, and the purity theorem would be false.
With it, a function type says everything about what its values can do.

The standard library provides `*Ctx` variants (`list.mapCtx`, `list.filterCtx`,
`result.andThenCtx`), and explicit recursion is always available when the
combinator does not fit. This is the sharpest trade-off in the language, and
Section 15 lists it as the first open question.

### 10.7 Calling convention

**receiver first, context second, everything else after** — which is now enforced
rather than merely conventional (Section 10.2):

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, IoError };
export fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B]
export fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>
```

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
xs.map(ctx, double)
lines.filter(ctx, isLong).sortBy(ctx, order.str)
```

### 10.8 Restricting what propagates

Two forms, giving different guarantees.

**Static confinement.** Bound the callee to fewer effects. It receives the
same value and cannot use, or pass on, anything its bounds do not name:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, Stdout };
fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
  let _ = io.println(ctx, msg);
  // fs.readText(ctx, "/etc/passwd")     // ERROR: C is not bounded by Fs
  // dangerous(ctx)                      // ERROR: dangerous needs C: Fs
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Fs: host.fs };
  let _ = logOnly(ctx, "starting");      // same value, confined by its bound
  .Ok(())
}
```

No copy and no ceremony. Confinement is transitive: `logOnly` cannot hand its
context to anything requiring more, because `C` is opaque at every call site
downstream.

**Attenuation.** Wrap the context in a type that satisfies fewer effects, so
the callee holds a value that genuinely lacks the rest:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, IoError, Region };
// module: safe/readonly
export struct ReadOnly<C>(C);

export fn readOnly<C>(ctx: C): ReadOnly<C> { ReadOnly(ctx) }

// Forwards Alloc...
impl<C: Alloc> Alloc for ReadOnly<C> {
  fn allocate(self: ReadOnly<C>, bytes: Int): Region { self.0.allocate(bytes) }
}

// ...and reading, but there is deliberately no `writeFile`, so ReadOnly<C>
// does not satisfy Fs no matter what C is.
export fn readFile<C: Fs>(self: ReadOnly<C>, path: Str): Result<Str, IoError> {
  self.0.readFile(path)
}
```

Static confinement is a fact about the type checker; attenuation is a fact about
the value, and survives anything that later escapes the type system. Use the
first by default and the second at trust boundaries.

Note that attenuation narrows the *context*, not one effect out of it. That
is what keeps the `ctx` rule satisfiable: there is still exactly one
effect-carrying parameter.

### 10.9 Testing

A pure function needs no harness. An effectful one is tested by building a
context out of different implementations — and because effects are ordinary
interfaces, writing one is writing a struct with methods. The call site does not
change, because there was never a global to stub.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/cap" import { Alloc, Fs, IoError };
struct FakeFs { export files: [(Str, Str)] }

impl Fs for FakeFs {
  fn readFile(self: FakeFs, path: Str): Result<Str, IoError> {
    match (self.files.find(fn(e) => e.0 == path)) {
      .Some(entry) => .Ok(entry.1),
      .None => .Err(.NotFound),
    }
  }
  fn writeFile(self: FakeFs, path: Str, body: Str): Result<(), IoError> {
    .Err(.ReadOnly)
  }
}

// context { Alloc: testing.alloc(), Fs: FakeFs { files: [...] } }
// loadConfig<C: Alloc + Fs> accepts it with no changes anywhere.
```

The harness around that — where tests live, how they are declared, and how they
build a context — is Sections 11.2 and 11.3.
