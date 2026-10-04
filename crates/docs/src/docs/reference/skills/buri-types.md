---
name: buri-types
description: Use when working with Buri types, generics, traits, derives, effects, or contexts — including "why does this need a ctx", unsatisfied bounds, and the lambda capture rule.
---

# Buri: types, traits, effects, contexts

The full rules are `buri docs language/types` and `buri docs language/effects`.
**Check the library before writing a helper:** bare `buri docs` lists every
module, `buri docs core/list` renders one, and `buri docs search <intent>` takes
a phrase like "pad a string" or "group by key".

## Primitives

| Type | Meaning |
|---|---|
| `Bool` | `true` / `false` |
| `I8` `I16` `I32` `I64` `I128` | signed two's-complement integers |
| `U8` `U16` `U32` `U64` `U128` | unsigned integers |
| `F32` `F64` | IEEE-754 binary32 / binary64 |
| `Char` | a Unicode scalar value |
| `Str` | an immutable UTF-8 string |
| `Template` | an interpolated string literal |

`Int = I64`, `Float = F64`, `Uint = U64`, `Byte = U8` are **aliases, not
distinct types**, so `Int` and `I64` mix freely. Everyday code writes `Int` and
`Float`; code with a size on the wire writes `U8`, `I32`, `F32`. There's no
`null`; absence is `Option<T>`.

## Composites

```buri
let pair: (Int, Str) = (1, "one");      // tuples have arity 2 or more
let first = pair.0;                     // nested access needs parens: (t.0).1
let xs: [Int] = [1, 2, 3];              // immutable, densely packed
let maybe = xs[0];                      // Option<Int>, never Int
```

- **No anonymous records.** Every product type is a named `struct`, and every
  type is nominal, so two structs with identical fields are different types.
- Fields are private unless exported. Outside the module, you can't build a
  struct with a private field from scratch, but `{ ..u, name: "x" }` works.
- A literal must set every *required* field. A field whose **declared** type is
  `Option<...>` may be left out and becomes `.None`. The declaration is what
  counts: `type Maybe = Option<Str>` qualifies, but the `T` of a `struct S<T>`
  doesn't, even at `S<Option<Int>>`.
- Enum variants are exported exactly when the enum is. To hide a
  representation, use a struct with a private field.
- An array literal doesn't allocate. Anything whose result length depends on
  runtime data — `map`, `filter`, `concat`, `sort`, `range` — needs `Allocator`.

`Option<T>`, `Result<T, E>` and `Order` are in the prelude. **You may not
discard a `Result`**: consume it with `?`, `match`, `result.withDefault`, or
the greppable `result.ignore`. `Option` is not must-use.

## Generics

```buri
fn identity<T>(x: T): T { x }
fn largest<T: Ordered>(xs: [T]): Option<T> { ... }
fn report<T: Ordered + Show, C: Allocator>(ctx: C, xs: [T]): Str { ... }

let f = identity<Int>;                  // type arguments go on the expression
let e: [Int] = list.empty<Int>();
```

On a type parameter you can call **only its bounds' methods**. If you need an
operation no trait provides, take it as a function argument: `sortBy(xs, cmp)`.
`<T: Ordered + Show>` and `<C: Allocator + FileSystemRead>` are the same
feature.

## Traits

```buri
trait Ordered {
    fn compare(self, other: Self): Order;
}
trait Show {
    fn show<C: Allocator>(self, ctx: C): Str;
}
```

- **Conformance is nominal.** A type satisfies a trait only where an `impl` or
  `derive` says so.
- An `impl` may appear only in its type's module, so you can't implement a
  trait for someone else's type.
- `impl Trait for Type { ... }` supplies conformance; `impl Type { ... }`
  declares the type's own methods. They share a namespace, and only the type's
  own methods take `export`.

### `derive`

```buri
derive Equal, Ordered, Show for Version;
```

`derive` writes the methods structurally, in declaration order, recursing into
field types; it fails if a field type doesn't satisfy the trait. Derivable:
`Equal`, `Ordered`, `Show`, `Hash`, `ToJson`, `FromJson` and the operator
traits. `ToJson` and `FromJson` are **derive-only**; a hand-written `impl` is
rejected.

`assert.equal(a, b)` needs `Equal` and `Show`, so an `missing-impl` in a
test usually wants `derive Equal, Show for YourType;`.

### Operators are trait methods

| Operator | Method |
|---|---|
| `a + b` `a - b` `-a` | `Add.add` `Subtract.subtract` `Negate.negate` |
| `a * b` `a / b` `a % b` | `Multiply.multiply` `Divide.divide` `Remainder.remainder` |
| `a == b` `a != b` | `Equal.equal` |
| `a < b` `a <= b` `a > b` `a >= b` | `Ordered.compare` |

```buri
struct Meters(F64);
derive Add, Subtract, Ordered, Show for Meters;

let total = Meters(1.5) + Meters(2.0);     // Meters
// let bad = Meters(1.5) + 2.0;            // ERROR: F64 is not Meters
```

**An operator can't allocate or perform an effect**, since `a + b` has nowhere
to pass a context. Matrix addition allocates, so it's `a.add(ctx, b)`, not
`a + b`.

Every built-in integer satisfies `Bounded`, `Checked`, `Wrapping` and
`Saturating`; floats satisfy `Bounded` only.

Traits have no blanket implementations, associated types, `where` clauses,
supertraits, trait objects, or dynamic dispatch.

## Method resolution

`x.f(...)` resolves to the first match:

1. a field named `f` on `x`'s type — call a function-typed field as
   `(x.f)(...)`;
2. for a concrete type, a method from an `impl` in that type's **defining
   module**;
3. for a type parameter, a method from one of its **bounds**.

| Type | Defining module |
|---|---|
| a `struct` or `enum` you declared | the module declaring it |
| `[T]` | `core/list` |
| `Str` `Char` `Bool` | `core/str` `core/character` `core/bool` |
| every integer and float type | `core/number` |
| `Option<T>` `Result<T, E>` | `core/option` `core/result` |
| tuples, function types, `Template` | none — no methods |

- **You can't add methods to a type** from another module: `impl Str { ... }`
  is an error, so write a free function.
- **Methods aren't values**: `sq.area` isn't one, so wrap the call in a lambda.
- **The receiver's type must be known.**
- When two bounds declare the same method name, call it as a function:
  `Ordered.compare(x, y)`.

## Effects

An **effect** is an interface declared with `effect` instead of `trait`.
`platform/effect` declares the bundled ones: `Allocator`, `Network`, `Clock`,
`Random`, `Environment`, `Stdin`, `Stdout`, `Stderr`, `Process`, `Tasks`,
`Listen` and `Tcp` (`native` only), `Sockets` and `WebSocketClient` (a page
dials a socket but never accepts one), and `Ui`, `Watch` and `Location` (`web`
of the bundled ones).
A repository declares its own only in an effect package under
`//platform/effect/`. `core/fs` declares the filesystem's two, `FileSystemRead` and
`FileSystemWrite`, so reading your configuration doesn't grant deleting it.
Every `core/fs` function takes a `Path`, which `core/fs` re-exports from
`core/path`; build one with `path.of(ctx, text)`.

An effect differs from a trait in three ways:

- Its implementors are **effect-carrying**, so you may pass one only as `self`
  or `ctx`.
- **No type may implement both an effect and a trait**, so an effect-carrying
  value never satisfies a bound like `T: Ordered`.
- **You perform an effect by passing the context to a function**:
  `io.println(ctx, "hi")`, not `ctx.println("hi")`; `fs.readText(ctx, p)`, not
  `ctx.readFile(p)`. The doors are `core/alloc`, `core/io`, `core/fs`,
  `core/net/http`, `core/time`, `core/random`, `core/env`, `core/process`,
  `core/tasks`, `core/net/server` and `ui/signal`. Calling the method on the
  value is `effect-method-call`, except in `core/*` and in an `impl E for T`
  supplying an effect. An inherent `impl` calls `fs.readText(self.0, p)`. **A print
  returns `Result<(), IoError>`**: drop one with
  `let _ = io.println(ctx, "hi").ignore();`, which `buri lint` reports like any
  other drop.

### The `ctx` rule

**An effect-carrying parameter must be named `self` or `ctx`**, at most one of
each, with the receiver first, the context second, and everything else after.

```buri
fn readText<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, IoError>  // ok
fn render<C: Allocator>(self, ctx: C): Str                        // ok
fn sneaky<C: FileSystemRead>(a: Int, handle: C): Bool                           // ERROR
fn twoWorlds<A: FileSystemRead, B: Network>(ctx: A, other: B): ()                   // ERROR
```

> A function is effectful if and only if it has a `ctx` parameter or an
> effect-carrying `self`.

### The three tiers

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` | `xs.length()`, `s.trim()`, `xs.fold(f, z)` |
| **Deterministic** | `ctx` bounded by `Allocator` alone | `xs.map(ctx, f)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)` |

An operation whose result size depends on runtime data needs `Allocator`.
Fixed-size construction — literals, tuples, enum payloads, closures,
`Template`s — never does.

### The capture rule

**A lambda may not capture an effect-carrying value.**

```buri
// ERROR: the lambda captures ctx
let texts = paths.map(ctx, fn(p) => fs.readText(ctx, p));

// Thread the context through a *Ctx combinator instead
let texts = paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p));
```

Use `list.mapCtx`, `list.filterCtx`, `result.mapCtx`, `result.andThenCtx` and
friends, or explicit recursion. The rule also catches any value whose type
*could* be a context: an unbounded `T`, or one bounded only by effects. A
closure-builder over a bare type parameter must take the value as a parameter
instead. A `T` with an ordinary trait bound is exempt, as is any function type.

## Contexts

A context binds each effect to a value implementing it. `main` and tests use the
same form.

```buri
let ctx = context { Allocator: host.alloc, Stdout: host.stdout, FileSystemRead: host.fs };

context Fixture {
    Allocator: alloc(),
    FileSystemRead: fs().files([("config.toml", "port=8080")]),
}
```

- Build a named context by **calling it**: `Fixture()` makes a fresh one each
  time.
- Either form may start with a spread; a later binding replaces the spread one.
- Every left side must name a declared effect (import it!), and every right side
  must implement it. The result satisfies exactly the bound effects, so a
  `<C: ...>` naming a subset accepts it and one naming more doesn't.
- A context's type is generated and unnamed; it never appears in source.

**You may build a context only** in an entry's body, a test source, or a
test-only module (a path with a `testing` segment) — never inside a lambda.

### Restricting what propagates

**Static confinement**: bound the callee to fewer effects. It gets the same
value but can't use or pass on anything its bounds don't name, transitively.

```buri
fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
    let _ = io.println(ctx, msg).ignore();
    // fs.readText(ctx, at)              // ERROR: C is not bounded by FileSystemRead
}
```

**Attenuation**: wrap the context in a type satisfying fewer effects, so the
callee's value genuinely lacks the rest. It narrows the whole context, never one
effect inside it. Use confinement by default and attenuation at trust
boundaries. `core/alloc`'s `GeneralPurpose`, `Arena` and `FixedBuffer` can be
imported anywhere, because `Allocator` is the one effect that grants nothing.
