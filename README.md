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
| **Safe** | No `null`, no exceptions, no mutation, no aliasing. Exhaustive `match`. Indexing returns `Option`. `Result` is must-use. Effects require a capability you were handed, in a parameter position the compiler enforces. Out-of-range numeric literals are compile errors. |
| **Fast to run** | Strict evaluation with fully specified order — no thunks, no space leaks. Monomorphized generics, no dictionaries. Guaranteed tail calls. Immutability lets the runtime reuse memory in place when a value is provably unshared. `Alloc` as a capability makes allocation visible at every call site that does it. |
| **Fast to compile** | An unambiguous LR(1) grammar with no name-resolution or type feedback into the parser, so parsing is one pass and trivially parallel across files. Mandatory top-level signatures make type inference local to each function body, so modules check independently and incrementally. No macros, no reflection, no overload resolution. Traits are structural and satisfied by one module, so there is no coherence pass and no instance search — a bound is a lookup. The one concession: method resolution needs the receiver's type, but it is a single lookup in one known module. |
| **Binary and JS** | Nothing in the semantics assumes a machine word: `Int` is `I64` everywhere, integer overflow crashes rather than wrapping, and evaluation order is specified rather than left to the backend. The capability model maps onto a browser platform as cleanly as onto a POSIX one — a JS target simply grants a different context to `main`. |

The one place these pull against each other is `I64` on the JS target, which has
no native 64-bit integer; see [SPEC.md §14](./SPEC.md).

**This repository is a specification, not an implementation.**

- [`SPEC.md`](./SPEC.md) — the language reference
- [`grammar.ebnf`](./grammar.ebnf) — the normative grammar, in extended BNF
- [`examples/`](./examples/) — twenty-two annotated example programs

```buri
from "core/io" import * as io;
from "core/list" import * as list;
from "core/cap" import { Alloc, Stdout };

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

export fn main<C: Alloc + Stdout>(ctx: C): Result<{}, Str> {
  let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
  let total = shapes.map(ctx, area).sum();
  let _ = io.println(ctx, "total area: ${total}");
  .Ok({})
}
```

## Three ideas

**1. There is no mutation.** Every binding is final. No references, no borrowing,
no lifetimes, no aliasing hazards. "Updating" a value produces a new one, and the
runtime is expected to make that cheap through structural sharing and in-place
update when a value is provably unshared — an implementation strategy that is
never observable.

**2. Effects travel through arguments.** A capability is a **trait** — allocating,
reading a file, opening a socket — and a function names the ones it needs as
bounds on its context parameter:

```buri
capability trait Fs {
  fn readFile(self: Self, path: Str): Result<Str, IoError>;
  fn writeFile(self: Self, path: Str, body: Str): Result<{}, IoError>;
}

fn loadUser<C: Alloc + Fs>(ctx: C, id: Str): Result<User, LoadError>
```

The compiler enforces where they may appear: **a capability-carrying parameter
must be `self` or `ctx`**, never any other name or position. So the question "can
this function touch the world?" is answered by reading the first two parameters
and stopping.

The platform supplies the one implementation that really does anything, and hands
it to `main`. A program whose `main` never names `Net` in its bounds cannot open
a socket anywhere in its transitive call graph — nothing anywhere can obtain a
value bounded by `Net`.

Purity is therefore not a keyword, not an inferred effect row, and not something
to propagate through signatures. It is the *absence* of one argument:

> If a function has no `ctx` parameter, no capability-carrying `self`, and
> captures no capability, then it is deterministic, effect-free, and freely
> cacheable.

Three tiers fall out, and each is visible at a glance:

```buri
fn sum(self: [Int]): Int                                       // pure
fn map<A,B,C: Alloc>(self: [A], ctx: C, f: fn(A)=>B): [B]      // deterministic, allocates
fn readText<C: Alloc + Fs>(ctx: C, p: Str): Result<Str, IoError>   // effectful
```

Allocation is tracked separately from I/O, so "does no I/O" and "does not
allocate" are separately expressible.

**3. The grammar is context-free and unambiguous.** Parsing never consults name
resolution or the type checker. That is a design constraint that cost real
ergonomics, and [SPEC.md §12](./SPEC.md#12-why-the-grammar-is-context-free-and-unambiguous)
lists all sixteen decisions with what each one gave up — parenthesized `if`
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

There is **no implicit promotion at all** (`1 + 1.0` is an error), and three
conversion operators that differ only in what happens when the value does not
fit:

```buri
x as I64      // lossless — compile error if the conversion could lose anything
x as? I32     // checked  — Option<I32>
x as% U8      // modular  — keeps the low bits, for checksums and wire formats
```

Overflow crashes by default; `x.wrappingAdd(y)` and `x.saturatingAdd(y)` are
there when wrapping is the intent. Conversions are operators rather than library
functions because a function cannot be generic over its *source* type.

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

A **trait is an interface**, and a type satisfies it when its defining module
declares matching methods:

```buri
trait Ord { fn compare(self: Self, other: Self): Order; }

impl Ord for Version { ... }     // states the intent, and is checked
derive Eq, Ord, Show for Playlist;   // generates it structurally
```

Because a type has exactly one defining module, it has exactly one candidate per
trait. Coherence, orphan rules, and instance search aren't restricted — they're
unrepresentable. Blanket impls, associated types, `where` clauses, supertraits,
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

Because capabilities are bounds, giving a callee less is just naming fewer of
them. It receives the same value and cannot use — or pass on — anything its
bounds omit:

```buri
fn logOnly<C: Stdout>(ctx: C, msg: Str): {} {
  let _ = io.println(ctx, msg);
  // fs.readText(ctx, "/etc/passwd")   // ERROR: C is not bounded by Fs
}

export fn main<C: Alloc + Stdout + Fs>(ctx: C): Result<{}, Str> {
  let _ = logOnly(ctx, "starting");    // same value, confined by its bound
  .Ok({})
}
```

No copy, no wrapper, no runtime cost, and confinement is transitive — `C` is
opaque at every downstream call site. When you want the value itself to lack the
capability rather than merely be unable to name it, wrap the context in a type
that satisfies fewer traits ([SPEC.md §10.8](./SPEC.md)).

One more thing falls out of capabilities being ordinary interfaces: **a test
double is a struct with methods.** No mocking framework, and the call site does
not change.

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

Primitives with explicit widths, arrays, tuples, records, structs (with private
fields), enums, functions, methods, and traits — including `capability trait`,
which is how effects are declared. Generics with trait bounds. Pattern matching with exhaustiveness checking. `Option`,
`Result`, `?`, `??`.

Methods and traits, neither of which introduces a runtime mechanism.

**Not present, deliberately:** classes, inheritance, dynamic dispatch, trait
objects, mutation, `null`, exceptions, loops, the `|>` pipe operator, `return`,
overloading, macros.

A `for`/`while` sugar was specified for this version and cut;
[SPEC.md §14.1](./SPEC.md) records the reasoning, since it constrains any future
attempt. Iteration is `fold` or explicit recursion, with tail calls guaranteed
eliminated.

**Deferred:** blanket impls, associated types, `where` clauses, supertraits,
foreign impls, dictionary literals, ranges, fixed-length array types, `async`.

## Status and open questions

This is a draft specification with no compiler behind it. The sharpest unresolved
trade-off is the rule that a lambda may not capture a capability
([SPEC.md §10.5](./SPEC.md)) — it is what makes the purity theorem hold
structurally, and it still forces any function that *stores* an effectful
callback to put the context in that callback's type. The runner-up is the
absence of `break`. [SPEC.md §14](./SPEC.md) lists those and five other questions
that want real programs before they can be settled.

## Naming

Búri, in Norse myth, is the first god — the one the others are descended from.
The language is named for the ambition, not the achievement.
