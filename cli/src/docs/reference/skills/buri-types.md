---
name: buri-types
description: Use when working with Buri types, generics, traits, derives, effects, or contexts — including "why does this need a ctx", unsatisfied bounds, and the lambda capture rule.
---

# Buri: types, traits, effects, contexts

`buri docs language/types` and `buri docs language/effects` are the normative
text. **Look in the library before writing a helper.** `buri docs core/list`
renders one module, and bare `buri docs` lists every module.
`buri docs search <intent>` takes a phrase like "pad a string" or "group by
key", and prints each hit as the command that reads it.

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
distinct types**, so `Int` and `I64` interoperate with no conversion. There is
no `null`; absence is `Option<T>`. Everyday code writes `Int` and `Float`. Code
with a size on the wire writes `U8`, `I32`, `F32`.

## Composites

```buri
let pair: (Int, Str) = (1, "one");      // tuples have arity 2 or more
let first = pair.0;                     // nested access needs parens: (t.0).1
let xs: [Int] = [1, 2, 3];              // immutable, densely packed
let maybe = xs[0];                      // Option<Int>, never Int
```

- **There are no anonymous records.** Every product type is a named `struct`,
  and every type is nominal, trait conformance included, so two structs with
  identical fields are different types.
- Fields stay module-private unless you `export` them. Nobody outside the
  module can build a struct with a private field from scratch, but
  `{ ..u, name: "x" }` still works.
- A literal gives every *required* field a value. You may leave out a field
  whose **declared** type is `Option<...>`, and it comes out `.None`. The
  compiler judges the declaration, so `type Maybe = Option<Str>` counts and the
  `T` of a `struct S<T>` does not, even at `S<Option<Int>>`.
- An enum's variants carry no `export` of their own; they go out exactly when
  the enum does. To hide a representation, use a struct with a private field.
- An array literal is not an allocation. Any operation whose result length
  depends on runtime data — `map`, `filter`, `concat`, `sort`, `range` — needs
  `Alloc`.

`Option<T>`, `Result<T, E>` and `Order` are in the prelude. **You may not
discard a `Result`.** Consume it with `?`, `match`, `result.withDefault`, or
the greppable `result.ignore`. `Option` is not must-use.

## Generics

```buri
fn identity<T>(x: T): T { x }
fn largest<T: Ord>(xs: [T]): Option<T> { ... }
fn report<T: Ord + Show, C: Alloc>(ctx: C, xs: [T]): Str { ... }

let f = identity<Int>;                  // type arguments go on the expression
let e: [Int] = list.empty<Int>();
```

Inside such a function you may call **only the bound's methods** on the
parameter. Generic code that needs an operation no trait provides takes it as a
function argument: `sortBy(xs, cmp)`. There is one constraint mechanism:
`<T: Ord + Show>` and `<C: Alloc + FsRead>` are the same feature.

## Traits

A trait is an interface: a named set of method signatures.

```buri
trait Ord {
    fn compare(self, other: Self): Order;
}
trait Show {
    fn show<C: Alloc>(self, ctx: C): Str;
}
```

- **Conformance is nominal.** A type satisfies a trait only where an `impl` or
  a `derive` says so, and nothing follows from shape.
- An `impl` may appear only in its type's defining module, so you cannot
  implement a trait for somebody else's type.
- `impl Trait for Type { ... }` supplies conformance, and `impl Type { ... }`
  declares the type's own methods. They share a namespace and resolve the same
  way, and only the type's own methods take `export`.

### `derive`

```buri
derive Eq, Ord, Show for Version;
```

`derive` writes the methods structurally: fields and variants in declaration
order, recursing into field types. It fails to compile when a field type does
not satisfy the trait itself. You can derive `Eq`, `Ord`, `Show`, `Hash`,
`ToJson`, `FromJson` and the operator traits. `ToJson` and `FromJson` are
**derive-only**, and the compiler rejects a hand-written `impl` of either.

`assert.eq(a, b)` needs `Eq` for the comparison and `Show` for the failure
message, so `derive Eq, Show for YourType;` is usually what an
`unsatisfied-bound` on a test is asking for.

### Operators are trait methods

| Operator | Method |
|---|---|
| `a + b` `a - b` `-a` | `Add.add` `Sub.sub` `Neg.neg` |
| `a * b` `a / b` `a % b` | `Mul.mul` `Div.div` `Rem.rem` |
| `a == b` `a != b` | `Eq.eq` |
| `a < b` `a <= b` `a > b` `a >= b` | `Ord.compare` |

```buri
struct Meters(F64);
derive Add, Sub, Ord, Show for Meters;

let total = Meters(1.5) + Meters(2.0);     // Meters
// let bad = Meters(1.5) + 2.0;            // ERROR: F64 is not Meters
```

**An operator implementation cannot allocate or perform an effect**: `a + b`
has no argument position for a context. So `Matrix + Matrix` is not
expressible; matrix addition allocates, so it is `a.add(ctx, b)`.

Integer-specific behaviour is trait-shaped too: `Bounded`, `Checked`,
`Wrapping`, `Saturating`. Every built-in integer satisfies all four, and the
float types satisfy `Bounded` only.

### What traits deliberately lack

No blanket implementations, no associated types, no `where` clauses, no
supertraits, no trait objects, no dynamic dispatch.

## Method resolution

`x.f(...)` resolves in three steps, each a lookup:

1. a field named `f` on `x`'s type — call a field of function type as
   `(x.f)(...)`;
2. otherwise, for a concrete type, a method declared by an `impl` block in
   that type's **defining module**;
3. otherwise, for a type parameter, a method declared by one of its **bounds**.

| Type | Defining module |
|---|---|
| a `struct` or `enum` you declared | the module declaring it |
| `[T]` | `core/list` |
| `Str` `Char` `Bool` | `core/str` `core/character` `core/bool` |
| every integer and float type | `core/num` |
| `Option<T>` `Result<T, E>` | `core/option` `core/result` |
| tuples, function types, `Template` | none — no methods |

**You cannot extend a type's methods**: `impl Str { ... }` in your module is an
error, so write a free function. **Methods are not values**: `sq.area` is not
one, so wrap the call in a lambda. **The receiver's type must be known.** Where
two bounds declare the same method name, call the trait method as a function to
disambiguate: `Ord.compare(x, y)`.

## Effects

An **effect** is an interface declared with `effect` instead of `trait`, and
only platform modules may declare one. `core/effect` declares `Alloc`, `Net`,
`Clock`, `Rand`, `Env`, `Stdin`, `Stdout`, `Stderr`, `Proc`, `Tasks`, `Listen`
(`LINUX` and `MACOS`, where a program serves a page), and `Sockets` and
`WebSocketClient` (everywhere: a page dials a socket, and never accepts one).
`core/fs` is a platform module too, and it declares the filesystem's
two, `FsRead` and `FsWrite`: reading your configuration does not earn you the
right to delete it. Every method there names a `Path` (`core/path`), which
`core/fs` re-exports.

An effect is a trait in every other respect but three:

- an effect's implementors are **effect-carrying**, so you may pass one only as
  `self` or `ctx`;
- **no type may implement both an effect and a trait**, so an effect-carrying
  type satisfies no ordinary bound, which keeps `T: Ord` from being a context;
- **you perform an effect by handing the context to a function.**
  `ctx.println("hi")` is `io.println(ctx, "hi")`, and `ctx.readFile(p)` is
  `fs.readText(ctx, p)`. The doors are `core/alloc`, `core/io`, `core/fs`,
  `core/net/http`, `core/time`, `core/random`, `core/env`, `core/process`,
  `core/tasks`, `core/net/server` and `ui/signal`, and a method on the value is
  `effect-method-call`. Only `core/*` and an `impl` supplying an effect keep
  the method form, which lets a wrapper delegate with `self.0.readFile(path)`.
  Every `core/fs` function takes a `Path`, built once with
  `path.of(ctx, text)`. **A print returns `Result<(), IoError>`**, so drop one
  with `let _ = io.println(ctx, "hi").ignore();`, and `buri lint` reports it
  like any other drop.

### The `ctx` rule

**An effect-carrying parameter must be named `self` or `ctx`** — never any
other name, never any other position, at most one of each.

```buri
fn readText<C: Alloc + FsRead>(ctx: C, at: Path): Result<Str, IoError>  // ok
fn render<C: Alloc>(self, ctx: C): Str                        // ok
fn sneaky<C: FsRead>(a: Int, handle: C): Bool                           // ERROR
fn twoWorlds<A: FsRead, B: Net>(ctx: A, other: B): ()                   // ERROR
```

**Receiver first, context second, everything else after**, and the compiler
enforces it rather than leaving it to you.

> A function is effectful if and only if it has a `ctx` parameter or an
> effect-carrying `self`.

That is the purity theorem in usable form.

### The three tiers

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` | `xs.len()`, `s.trim()`, `xs.fold(f, z)` |
| **Deterministic** | `ctx` bounded by `Alloc` alone | `xs.map(ctx, f)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)` |

An operation with a fixed result size is pure; one whose result size depends on
runtime data names `Alloc`. Fixed-size construction — literals, tuples, enum
payloads, closures, `Template`s — never needs it.

### The capture rule

**A lambda may not capture an effect-carrying value.**

```buri
// ERROR: the lambda captures ctx
let texts = paths.map(ctx, fn(p) => fs.readText(ctx, p));

// Thread the context through a *Ctx combinator instead
let texts = paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p));
```

The library gives you `list.mapCtx`, `list.filterCtx`, `result.mapCtx`,
`result.andThenCtx` and friends, and explicit recursion always works. The rule
also catches a value whose type *could* be a context: an unbounded `T`, or one
bounded only by effects. So a closure-builder over a bare type parameter takes
the value as a parameter instead. A `T` with an ordinary trait bound is exempt,
and so is any function type.

## Contexts

A context binds each effect to a value implementing it. One form, used by both
`main` and a test.

```buri
let ctx = context { Alloc: host.alloc, Stdout: host.stdout, FsRead: host.fs };

context Fixture {
    Alloc: alloc(),
    FsRead: fs().files([("config.toml", "port=8080")]),
}
```

- You build a named context by **calling it**, `Fixture()`, and each call
  builds a fresh one.
- Either form may begin with a spread. A later binding replaces a spread one
  rather than duplicating it.
- Every left side must name a declared effect (import it!), and every right
  side must implement it. The result satisfies exactly the effects you bound,
  so a `<C: ...>` naming a subset accepts it and one naming more does not.
- A context's type never appears in source: the compiler generates it, unnamed.

**Where you may build a context:** an entry's body, a test source, or a test-only
module (a path with a `testing` segment). Never inside a lambda, and nowhere
else.

### Restricting what propagates

**Static confinement**: bound the callee to fewer effects. It receives the
same value, and it can neither use nor pass on anything its bounds do not name.
That holds transitively.

```buri
fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
    let _ = io.println(ctx, msg).ignore();
    // fs.readText(ctx, at)              // ERROR: C is not bounded by FsRead
}
```

**Attenuation**: wrap the context in a type satisfying fewer effects, so the
callee holds a value that genuinely lacks the rest. It narrows the whole
context, never one effect out of it. Reach for confinement by default and
attenuation at trust boundaries. You may import `core/alloc`'s
`GeneralPurpose`, `Arena` and `FixedBuffer` anywhere, because `Alloc` is the
one effect whose implementation grants nothing.
