# Buri

A strict, purely functional, statically typed language with TypeScript-shaped
syntax, Rust-shaped data declarations, and Roc-shaped ideas about platforms and
effects.

## Goals

**Safe, fast to run, fast to compile** — in that order when they conflict.
Secondarily, one language that targets both a native binary and JavaScript.

Those goals are why the design looks the way it does:

| Goal | What it bought, and what it cost |
|---|---|
| **Safe** | No `null`, no exceptions, no mutation, no aliasing. Exhaustive `match`. Indexing returns `Option`. `Result` is must-use. Effects require an effect value you were handed, in a parameter position the compiler enforces. Out-of-range numeric literals are compile errors. |
| **Fast to run** | Strict evaluation with fully specified order — no thunks, no space leaks. Monomorphized generics, no dictionaries. Guaranteed tail calls, lowered to loops where the host lacks them. Immutability lets the runtime reuse memory in place when a value is provably unshared. `Alloc` as an effect makes allocation visible at every call site that does it. |
| **Fast to compile** | An unambiguous LR(1) grammar with no name-resolution or type feedback into the parser, so parsing is one pass and trivially parallel across files. Mandatory top-level signatures make type inference local to each function body, so modules check independently and incrementally. No macros, no reflection, no overload resolution, no row unification. Conformance is nominal and declared in one module, so there is no coherence pass and no instance search — a bound is a table lookup. The one concession: method resolution needs the receiver's type, so name resolution and inference interleave. |
| **Binary and JS** | Nothing in the semantics assumes a machine word: `Int` is `I64` everywhere, integer overflow crashes rather than wrapping, and evaluation order is specified rather than left to the backend. The effect model maps onto a browser platform as cleanly as onto a POSIX one — a JS target simply exports a different `core/host`. |

Where they pull against each other, the compiler absorbs it rather than the
language: guaranteed tail calls become loops on a JS target, since no engine but
JavaScriptCore implements them natively ([SPEC.md §8.3.1](./SPEC.md)). `I64` on
JS is the one genuinely unresolved tension — see [SPEC.md §15](./SPEC.md).

[SPEC.md §13](./SPEC.md) states the invariants that make the compile-speed goal
reachable, so a future feature can be measured against them rather than
quietly eroding them.

**This repository is a specification, not an implementation.**

- [`SPEC.md`](./SPEC.md) — the language reference
- [`grammar.ebnf`](./grammar.ebnf) — the normative grammar, in extended BNF
- [`examples/`](./examples/) — twenty-two annotated example programs
- [`build-system/`](./build-system/) — the monorepo build system: `BUILD.buri`
  files, library and binary targets, visibility, tags, hermetic incremental
  builds, and one CLI

```buri
from "core/list" import * as list;
from "core/host" import * as host;

enum Shape {
  Circle(Float),
  Rect { width: Float, height: Float },
  Empty,
}

// No context parameter, so this cannot allocate, print, read a file, or open a
// socket. It is a mathematical function of its argument, and you can see that
// from the signature alone.
fn area(self: Shape): Float {
  match (self) {
    .Circle(r) => 3.14159 * r * r,
    .Rect { width, height } => width * height,
    .Empty => 0.0,
  }
}

// `main` takes no arguments. It builds the one context the program has, and
// those two bindings are the whole effect budget.
export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
  };

  let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
  let total = shapes.map(ctx, area).sum();
  let _ = ctx.println("total area: ${total}");
  .Ok(())
}
```

## Three ideas

**1. There is no mutation.** Every binding is final. No references, no borrowing,
no lifetimes, no aliasing hazards. "Updating" a value produces a new one, and the
runtime is expected to make that cheap through structural sharing and in-place
update when a value is provably unshared — an implementation strategy that is
never observable.

**2. Effects travel through arguments.** An **effect** is an interface — declared
with `effect` instead of `trait`, and only by platform modules — and a function
names the ones it needs as bounds on its context parameter:

```buri
effect Fs {
  fn readFile(self: Self, path: Str): Result<Str, IoError>;
  fn writeFile(self: Self, path: Str, body: Str): Result<(), IoError>;
}

fn loadUser<C: Alloc + Fs>(ctx: C, id: Str): Result<User, LoadError>
```

The compiler enforces where they may appear: **an effect-carrying parameter must
be `self` or `ctx`**, never any other name or position. So the question "can this
function touch the world?" is answered by reading the first two parameters and
stopping. No type may implement both an effect and a trait, so the boundary
between the world and your data is checked rather than assumed.

The platform supplies the implementations that really do anything, in a module
`core/host` that only the file exporting `main` may import. `main` takes no
parameters: it names the effects it wants, binds each to one of those
implementations, and passes the result down. A program whose `main` never names
`host.net` cannot open a socket anywhere in its transitive call graph — nothing
anywhere can obtain a value bounded by `Net`. A test builds its context the same
way, from the test runner's implementations instead.

Purity is therefore not a keyword, not an inferred effect row, and not something
to propagate through signatures. It is the *absence* of one argument:

> If a function has no `ctx` parameter, no effect-carrying `self`, captures no
> effect, and builds no context, then it is deterministic, effect-free, and
> freely cacheable.

The last clause covers `main` and a test, the only two places a context is
built. Neither is a function library code can call, so in ordinary code the
check is still just: *is there a `ctx` parameter?*

Three tiers fall out, and each is visible at a glance:

```buri
fn sum(self: [Int]): Int                                       // pure
fn map<A,B,C: Alloc>(self: [A], ctx: C, f: fn(A)=>B): [B]      // deterministic, allocates
fn readFile<C: Alloc + Fs>(ctx: C, p: Str): Result<Str, IoError>   // effectful
```

Allocation is tracked separately from I/O, so "does no I/O" and "does not
allocate" are separately expressible.

**3. The grammar is context-free and unambiguous.** Parsing never consults name
resolution or the type checker. That is a design constraint that cost real
ergonomics, and [SPEC.md §12](./SPEC.md#12-why-the-grammar-is-context-free-and-unambiguous)
lists all seventeen decisions with what each one gave up — parenthesized `if`
conditions, no record field shorthand, the turbofish, no `<<`/`>>` tokens,
dot-prefixed variants in patterns, and the rest.

## Numbers: two names, one set of types

Most code wants to say "a number." Some code — binary formats, checksums,
graphics, FFI — needs an exact width and wants the compiler to hold it there.
`Int` and `Float` are **aliases** (`I64`, `F64`), not a separate tier, so the two
kinds of code interoperate with no conversions at the boundary.

```buri
let a = 5;              // nothing pins it -> Int
let b: U8 = 200;        // the annotation pins it -> U8, not a conversion
let c: [F32] = [1.5];   // literals take their type from context
let bad: U8 = 300;      // compile error: 300 is not representable in U8
```

A numeric literal has no type until something constrains it, and only falls back
to `Int`/`Float` when nothing does — so out-of-range literals are caught at
compile time and there are no `5u8` suffixes to learn.

There is **no implicit promotion at all** (`1 + 1.0` is an error), and
conversions are ordinary methods rather than cast operators:

```buri
small.toI64()      // always exact — returns I64
big.toI32()        // may not fit  — returns Result<I32, RangeError>
big.wrapToU8()     // modular      — keeps the low bits, for wire formats
```

Whether a conversion can fail is visible in its return type rather than in the
choice of operator. Overflow crashes by default; `x.wrappingAdd(y)` and
`x.saturatingAdd(y)` are there when wrapping is the intent.

## Methods, and traits as interfaces

A function is a method **if and only if its first parameter is written `self`**
— a declaration, not a convention:

```buri
export fn area(self: Square): Int { self.height * self.width }
```

`x.f(a)` then calls `f(x, a)`, resolving `f` in the **defining module of x's
type**. No dispatch, no vtable, no impl block: `sq.area()` and `area(sq)` are the
same call. What it buys is that a type's operations travel with it —

```buri
from "lib/square" import { Square };     // the type — not `area`, not `scaled`
sq.scaled(2).area()                      // both resolve with no further imports
```

— and resolution stays one type, one module, one lookup: no candidate set, no
coherence check, no autoref, because there are no references.

A **trait is an interface**, and conformance is **nominal** — a type satisfies it
only where an `impl` or `derive` says so, never by accident of shape:

```buri
trait Ord { fn compare(self: Self, other: Self): Order; }

impl Ord for Version { ... }         // supplies the methods, checked against the trait
derive Eq, Ord, Show for Playlist;   // generates them structurally
```

Because a type has exactly one defining module and conformance is declared, there
is exactly one candidate per `(trait, type)`. Coherence, orphan rules, and
instance search aren't restricted — they're unrepresentable. It also keeps a
module's public API from implicitly including *which traits its types happen to
satisfy*, which is what would otherwise coarsen incremental rebuilds. Blanket impls, associated types, `where` clauses, supertraits,
and trait objects are all deliberately absent: each turns resolution from a
lookup into a search, and the search is the entire compile-time cost of a trait
system.

Operators are trait methods, which is what makes newtypes usable:

```buri
struct Meters(F64);
derive Add, Sub, Ord, Show for Meters;

let total = Meters(1.5) + Meters(2.0);   // Meters
// let bad = Meters(1.5) + 2.0;          // ERROR: F64 is not Meters
```

And an operator implementation **cannot allocate or perform an effect** — `a + b`
has no argument position for a context. You cannot write an expensive `+` in this
language, which is why operator overloading is safe here in a way it isn't
elsewhere.

## Restricting what propagates

Because effects are bounds, giving a callee less is just naming fewer of
them. It receives the same value and cannot use — or pass on — anything its
bounds omit:

```buri
fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
  let _ = ctx.println(msg);
  // ctx.readFile("/etc/passwd")       // ERROR: C is not bounded by Fs
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Fs: host.fs };
  let _ = logOnly(ctx, "starting");    // same value, confined by its bound
  .Ok(())
}
```

No copy, no wrapper, no runtime cost, and confinement is transitive — `C` is
opaque at every downstream call site. When you want the value itself to lack the
effect rather than merely be unable to name it, wrap the context in a type
that satisfies fewer traits ([SPEC.md §10.8](./SPEC.md)).

One more thing falls out of effects being ordinary interfaces: **a test double is
a struct with methods.** A test builds a context the same way `main` does, and
binds whichever implementations it wants — the runner's in-memory filesystem, or
its own:

```buri
test "falls back when the config is missing" {
  let ctx = context { ..Hermetic(), Fs: files([]) };
  assert.eq(loadConfig(ctx, "config.toml").withDefault(fallback()), fallback());
}
```

No mocking framework, and the call site does not change.

## Errors are not ignorable

`Result` is must-use: `let _ = fs.writeText(ctx, p, body);` does not compile.
Since Buri has no expression statements, `let _ =` is the only way to discard a
value, so the rule has no holes. Consume a `Result` with `?`, with `match`, with
`result.withDefault`, or — when you really mean it — with the explicit,
greppable `result.ignore`.

## Imports name the module first

```buri
from "core/list" import { map, filter };
from "core/list" import * as list;
```

The path leads so that an editor knows which module you mean before you open the
brace, and can complete the specifier list. A namespace import must be named:
bare `import *` is not derivable from the grammar, so no identifier ever enters a
module's scope without appearing in that module's own source.

## What's in v0.2

Primitives with explicit widths, arrays, tuples, structs (with per-field
visibility), enums, functions, methods, traits, and `effect` declarations.
Generics with trait bounds. The type system is nominal throughout — no records,
no structural conformance. Pattern matching with exhaustiveness checking. `Option`,
`Result`, `?`, `??`.

Methods and traits, neither of which introduces a runtime mechanism.

**Not present, deliberately:** classes, inheritance, dynamic dispatch, trait
objects, records, row polymorphism, cast operators, mutation, `null`, exceptions,
loops, the `|>` pipe operator, `return`, overloading, macros.

A `for`/`while` sugar was specified for this version and cut;
[SPEC.md §15.1](./SPEC.md) records the reasoning, since it constrains any future
attempt. Iteration is `fold` or explicit recursion, with tail calls guaranteed
eliminated.

**Deferred:** blanket impls, associated types, `where` clauses, supertraits,
foreign impls, dictionary literals, ranges, fixed-length array types, `async`.

## Status and open questions

This is a draft specification with no compiler behind it. The sharpest unresolved
trade-off is the rule that a lambda may not capture an effect
([SPEC.md §10.5](./SPEC.md)) — it is what makes the purity theorem hold
structurally, and it still forces any function that *stores* an effectful
callback to put the context in that callback's type. The runner-up is the
absence of `break`. [SPEC.md §15](./SPEC.md) lists those and five other questions
that want real programs before they can be settled.

## Naming

Búri, in Norse myth, is the first god — the one the others are descended from.
The language is named for the ambition, not the achievement.
