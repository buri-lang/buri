## 10. Effects and purity

This is the part of Buri that is not TypeScript and not Rust.

### 10.1 The model

An **effect** is an interface declared with `effect` instead of `trait`. Its
methods are the operations it grants:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// core/effect
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

`core/effect` declares `Alloc`, `Fs`, `Net`, `Clock`, `Rand`, `Env`, `Stdin`,
`Stdout`, `Stderr`, and `Proc`. **Only platform modules may declare effects**;
`effect` in ordinary code is a compile error, so the set of things a Buri program
can do to the world is fixed by its platform rather than open-ended.

An effect is a trait in every other respect — same declaration shape, same
nominal conformance, same `impl`, same bounds. Two rules separate them:

- an effect's implementors are **effect-carrying**, and so may be passed only as
  `self` or `ctx` (Section 10.2);
- **no type may implement both an effect and a trait.** A type is either part of
  the world or part of your data, and the boundary is checked rather than
  assumed. It holds for composites too: an effect-carrying type — one that
  merely *mentions* an effect, such as a `Holder<C>` storing a context —
  satisfies no ordinary bound either, whatever `impl`s its head constructor
  carries. That is what lets Section 10.6 conclude that a `T: Ord` is never a
  capability.

A function names the effects it needs as **bounds** on its context parameter:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs };
fn loadConfig<C: Alloc + Fs>(ctx: C, path: Str): Result<Config, ConfigError> {
  let text = fs.readText(ctx, path)?;
  parse(ctx, text)
}
```

There is one constraint mechanism in the language. `<T: Ord + Show>` and
`<C: Alloc + Fs>` are the same feature: a list of interfaces a type parameter
must satisfy.

### 10.2 The `ctx` rule

**An effect-carrying parameter must be `self` or `ctx`** — never any other
name, never any other position, and at most one of each:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError, Net, Region };
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>       // ok
fn render<C: Alloc>(self: Report, ctx: C): Str                            // ok
fn allocate(self: Self, bytes: Int): Region                               // ok
fn sneaky<C: Fs>(a: Int, handle: C): Bool                                 // ERROR
fn twoWorlds<A: Fs, B: Net>(ctx: A, other: B): {}                         // ERROR

enum Widget<C> { Press(fn(C, Int) => Str), Group([Widget<C>]) }
enum Boxed<C>  { Held(C) }

fn render<C: Alloc>(ctx: C, root: Widget<C>): Str                         // ok
fn peek<C: Alloc>(ctx: C, held: Boxed<C>): Int                            // ERROR
```

A type is **effect-carrying** if it is a type variable with an effect
bound, a type that implements an effect, or any type that can hand one of those
back — so a struct that stores a context is effect-carrying too.

**Position decides.** A *function type* is effect-carrying when its **result**
is: `fn(C, A) => B` merely accepts a context, which is the shape the `*Ctx`
combinators of Section 10.6 take, while `fn() => C` produces one. The same
reading applies to a type you declare, at each of its type arguments: an
argument counts only where the constructor can hand that argument back.

`Widget<C>` mentions `C`, but only where a *caller* must supply one: to get a
`C` out of a press handler you would have to pass one in first. `Boxed<C>`
stores its `C` and hands it straight back, so a `Boxed<C>` is a second context
under another name. A parameter that occurs in no field at all — a handle that
is phantom in it — is data for the same reason.

`self` has to be allowed because an effect's own methods take the
effect as their receiver (`fn allocate(self: Self, ...)`), and so do the
attenuation wrappers of Section 10.8. Outside those two places, effects arrive
through `ctx`.

There is exactly one construct in which more than one effect-carrying value may
appear, and it is the `context` expression of Section 11.3 — the place where a
context is assembled out of the implementations that make it up. Everywhere
else, effects travel through a single `ctx` parameter or an
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
`host.clock`, `host.rand`, `host.env`, `host.proc`, and on a platform with a
document `host.ui`, `host.watch`, `host.fetch` — and it is importable only from
the module that exports `main`. `main` assembles them into the one context
the program has:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, Stdout };
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

Note what is *not* claimed: an effect is an ordinary interface, so
anyone may write a type that satisfies it.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Stdout };
struct SilentOut {}
fn writeOut(self: SilentOut, text: Template): () { () }    // satisfies Stdout
```

That is not a forgery hole — a fake `Stdout` still cannot write anything. What is
unforgeable is the *platform's* implementation. The open interface is what makes
testing free (Section 10.8).

`Alloc` is the case where that openness is useful outside a test, because it is
the one effect whose implementation grants nothing: `allocate` answers a
`Region`, which is a number nothing reads. So `core/alloc` ships three
implementations — `generalPurpose()`, `arena()`, `fixedBuffer(n)` — and is
importable anywhere rather than only from `main`. Binding one is how a program
asks what it is spending, or refuses to spend more than a budget; it is not how
a program acquires an authority it was not given.

### 10.4 What "pure" means

> **Purity theorem.** If a function has no `ctx` parameter, no
> effect-carrying `self`, captures no effect-carrying value, and constructs no
> context, then any two evaluations on **identical** arguments that **terminate
> without aborting**, in the **absence of undefined behaviour**, produce
> identical results and perform no observable effect — and a call that
> terminates without aborting may be freely cached, reordered, or eliminated.

Each of those three qualifiers is load-bearing, and each is there because the
sentence without it is false:

- **Identical, not equal.** Equality is a trait, and function types do not have
  it (Section 5.11): `f == g` is not a question the language can ask, so "equal
  arguments" has no referent at a function type. The theorem quantifies over the
  *same* values, which is a form that means something at every type.
- **Terminating without aborting.** A pure function may abort — `100 / x` at
  `x = 0` does — and an abort is observable: a message on stderr and a non-zero
  exit status (Section 6.10). Eliminating a call that would have aborted turns
  an aborting program into a running one, which is not a refinement of it.
  Divergence has the same shape. So an implementation may drop a pure call only
  where it can also show the call returns.
- **In the absence of undefined behaviour.** Overflow is undefined (Section
  6.2), and on a target where every number is a double, `I64` arithmetic is
  undefined above 2^53 without overflowing the nominal type. Two evaluations
  agree only where the program's behaviour is defined at all.

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
something, but it is not observable. Every other effect in `core/effect` is
**observable**.

A function is **deterministic** if its only effect bound is `Alloc`.
`list.map(ctx, f)` is deterministic: it needs to allocate, but it is
referentially transparent. `time.now(ctx)` is not.

Tracking allocation is why `[T]`-returning combinators take a context at all, and
it is what makes "does no I/O" and "does not allocate" separately expressible:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError };
fn sum(self: [Int]): Int                                              // pure
fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B]         // deterministic
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>   // effectful
```

Fixed-size construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Alloc`. Only results whose size depends
on runtime data do.

### 10.6 The capture rule

**A lambda may not capture an effect-carrying value.** Effects travel
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

The rule reaches every binding a lambda could close over — parameters, `let`
bindings, names a pattern binds, and the parameters of an *enclosing* lambda.
The context a `*Ctx` combinator hands to `fn(c, p) => ...` is a binding like any
other, so a lambda written inside that one may not capture `c` either.

**And a lambda may not capture a value whose type could be a context.** This is
the same rule at a type parameter, where "carries an effect" cannot be read off
the type — so `fn wrap<T>(x: T, f: fn(T) => ()): fn() => () { fn() => f(x) }` is
rejected, on the capture of `x`.

Nothing in `wrap` mentions an effect. Its body is checked once, for every
instantiation at once (Section 13.5), so at the point the rule runs `T` is
opaque — and `wrap(ctx, fn(c) => c.println("hi"))` instantiates it at a context
type and returns a closure of type `fn() => ()` holding a capability. That is
exactly the smuggling the paragraph above rules out, arriving by the generic
route instead of the monomorphic one, and the predicate that only reads the
signature cannot see it. So a type parameter is treated as though it *were* a
context, unless one of two things says otherwise:

- **An ordinary trait bound.** A type is either part of the world or part of
  your data (Section 10.1), and that boundary is checked at every instantiation:
  an effect-carrying type satisfies no ordinary bound. So a `T: Eq` is never a
  context, and `xs.any(fn(x) => x == needle)` inside `impl<T: Eq> [T]` is fine.
  A `T` with no bounds, or one bounded only by effects, has no such guarantee.
- **A function type.** A closure holds exactly what it captured, and that is
  what this rule checks — so no closure holds a capability, and capturing one is
  safe whatever its type parameters are. `fn compose<A, B, C>(f: fn(A) => B, g:
  fn(B) => C): fn(A) => C { fn(x) => g(f(x)) }` is legal.

The cost is that a closure-builder over an unconstrained type parameter has to
take the value as a parameter rather than close over it. The alternative is a
purity theorem that is false, which is not an alternative.

The standard library provides `*Ctx` variants (`list.mapCtx`, `list.filterCtx`,
`result.andThenCtx`), and explicit recursion is always available when the
combinator does not fit. This is the sharpest trade-off in the language, and
Section 15 lists it as the first open question.

### 10.7 Calling convention

**receiver first, context second, everything else after** — which is now enforced
rather than merely conventional (Section 10.2):

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError };
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
# from "core/effect" import { Alloc, Fs, Stdout };
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
# from "core/effect" import { Alloc, Fs, IoError, Region };
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
# from "core/effect" import { Alloc, Fs, IoError };
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
