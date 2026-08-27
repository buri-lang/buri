---
name: buri-types
description: Use when working with Buri types, generics, traits, derives, effects, or contexts — including "why does this need a ctx", unsatisfied bounds, and the lambda capture rule.
---

# Buri: types, traits, effects, contexts

`buri docs lang/types` and `buri docs lang/effects` are the normative text;
`buri docs core/list` renders a standard library module from the source the
compiler checked.

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
distinct types** — a function declared with `Int` and one declared with `I64`
interoperate with no conversion. There is no `null`; absence is `Option<T>`.

Everyday code writes `Int` and `Float`. Code with a size on the wire writes
`U8`, `I32`, `F32` and gets exactly that. There is no third category and no
numeric tower.

## Composites

```buri
let pair: (Int, Str) = (1, "one");      // tuples have arity 2 or more
let first = pair.0;                     // nested access needs parens: (t.0).1
let xs: [Int] = [1, 2, 3];              // immutable, densely packed
let maybe = xs[0];                      // Option<Int>, never Int
```

- **There are no anonymous records.** Every product type is a named `struct`,
  and every type in the language is nominal — including trait conformance.
- Two structs with identical fields are different types.
- Fields are module-private unless `export`ed. A struct with any private field
  cannot be constructed from scratch elsewhere, but `{ ..u, name: "x" }` still
  works, because it never names the hidden fields.
- An enum's variants carry no `export` of their own: they are exported exactly
  when the enum is. Hiding a representation is a struct with a private field.
- An array literal is not an allocation. Any operation whose result length
  depends on runtime data — `map`, `filter`, `concat`, `sort`, `range` —
  requires `Alloc`.

`Option<T>`, `Result<T, E>` and `Order` are in the prelude. **A `Result` may
not be discarded**; consume it with `?`, `match`, `result.withDefault`, or the
greppable `result.ignore`. `Option` is not must-use.

## Generics

```buri
fn identity<T>(x: T): T { x }
fn largest<T: Ord>(xs: [T]): Option<T> { ... }
fn report<T: Ord + Show, C: Alloc>(ctx: C, xs: [T]): Str { ... }

let f = identity<Int>;                  // type arguments go on the expression
let e: [Int] = list.empty<Int>();
```

Inside such a function, **only the bound's methods are callable** on the
parameter, and nothing else. Generic code needing an operation no trait
provides takes it as a function argument: `sortBy(xs, cmp)`.

There is one constraint mechanism. `<T: Ord + Show>` and `<C: Alloc + Fs>` are
the same feature.

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
  a `derive` says so; nothing is inferred from shape. Checking `T: Ord` is one
  lookup keyed by `(trait, type)`.
- An `impl` may appear only in the defining module of its type, so you cannot
  implement a trait for someone else's type — and there is no coherence pass,
  no orphan rule, no instance search.
- `impl Trait for Type { ... }` supplies conformance; `impl Type { ... }`
  declares the type's own methods. Same namespace, same resolution.
- A method of the type's own may be `export`ed; a method supplied to a trait
  may not.

### `derive`

```buri
derive Eq, Ord, Show for Version;
```

Generates methods structurally — struct fields in declaration order, enum
variants in declaration order, recursing into field types — and fails to
compile if any field type does not itself satisfy the trait.

Derivable: `Eq`, `Ord`, `Show`, `Hash`, `ToJson`, `FromJson`, and the operator
traits. `ToJson`/`FromJson` are **only ever derived**; a hand-written `impl` of
either is rejected.

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

**An operator implementation cannot allocate or perform an effect** — there is
no argument position in `a + b` for a context — so `Matrix + Matrix` is not
expressible. Matrix addition allocates, so it is `a.add(ctx, b)`, which says so.

Integer-specific behaviour is likewise trait-shaped: `Bounded`, `Checked`,
`Wrapping`, `Saturating`. Every built-in integer satisfies all four; the float
types satisfy `Bounded` only.

### What traits deliberately lack

No blanket implementations, no associated types, no `where` clauses, no
supertraits, no trait objects, no dynamic dispatch. Generic code is
monomorphized; a generic body is typechecked once, polymorphically, with bounds
verified at the call site.

## Method resolution

`x.f(...)` resolves in three steps, each a lookup:

1. a field named `f` on `x`'s type (a field of function type is called
   `(x.f)(...)`);
2. otherwise, for a concrete type, a method declared by an `impl` block in
   that type's **defining module**;
3. otherwise, for a type parameter, a method declared by one of its **bounds**.

| Type | Defining module |
|---|---|
| a `struct` or `enum` you declared | the module declaring it |
| `[T]` | `core/list` |
| `Str` `Char` `Bool` | `core/str` `core/char` `core/bool` |
| every integer and float type | `core/num` |
| `Option<T>` `Result<T, E>` | `core/option` `core/result` |
| tuples, function types, `Template` | none — no methods |

Consequences: **methods are not extensible** (`impl Str { ... }` in your module
is an error — write a free function); **methods are not values** (`sq.area` is
not one, wrap the call in a lambda); **the receiver's type must be known**.
Where two bounds declare the same method name, disambiguate by calling the
trait method as a function: `Ord.compare(x, y)`.

## Effects

An **effect** is an interface declared with `effect` instead of `trait`. Only
platform modules may declare one. `core/effect` declares `Alloc`, `Fs`, `Net`,
`Clock`, `Rand`, `Env`, `Stdin`, `Stdout`, `Stderr`, `Proc`.

An effect is a trait in every other respect. Two rules separate them:

- an effect's implementors are **effect-carrying**, and may be passed only as
  `self` or `ctx`;
- **no type may implement both an effect and a trait**, so an effect-carrying
  type satisfies no ordinary bound — which is what makes a `T: Ord` provably
  not a context.

### The `ctx` rule

**An effect-carrying parameter must be named `self` or `ctx`** — never any
other name, never any other position, at most one of each.

```buri
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>   // ok
fn render<C: Alloc>(self, ctx: C): Str                        // ok
fn sneaky<C: Fs>(a: Int, handle: C): Bool                             // ERROR
fn twoWorlds<A: Fs, B: Net>(ctx: A, other: B): ()                     // ERROR
```

The convention is enforced, not merely followed: **receiver first, context
second, everything else after**.

> A function is effectful if and only if it has a `ctx` parameter or an
> effect-carrying `self`.

That is the purity theorem in usable form. Purity is not a keyword — it is the
absence of one argument, in a fixed position, with a fixed name, and the check
never reads a function body.

### The three tiers

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` | `xs.len()`, `s.trim()`, `xs.fold(f, z)` |
| **Deterministic** | `ctx` bounded by `Alloc` alone | `xs.map(ctx, f)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)` |

The rule that decides it: an operation whose result size is fixed is pure, and
one whose result size depends on runtime data names `Alloc`. Fixed-size
construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Alloc`.

### The capture rule

**A lambda may not capture an effect-carrying value.**

```buri
// ERROR: the lambda captures ctx
let texts = paths.map(ctx, fn(p) => fs.readText(ctx, p));

// Thread the context through a *Ctx combinator instead
let texts = paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p));
```

The library provides `list.mapCtx`, `list.filterCtx`, `result.andThenCtx` and
friends; explicit recursion always works. The rule also reaches a value whose
type *could* be a context — an unbounded `T`, or one bounded only by effects —
so a closure-builder over a bare type parameter has to take the value as a
parameter rather than close over it. A `T` with an ordinary trait bound, and
any function type, are exempt.

## Contexts

A context binds each effect to a value implementing it. One form, used by both
`main` and a test.

```buri
let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Fs: host.fs,
};

context Fixture {
    ..Hermetic(),
    Fs: files([("config.toml", "port=8080")]),
}
```

- A named context is **constructed by calling it** — `Fixture()` — and each
  call builds a fresh one.
- Either form may begin with a spread; a later binding replaces a spread one
  rather than duplicating it.
- Every left side must name a declared effect (import it!), every right side
  must implement it, and the result satisfies exactly the effects bound — so
  it is accepted by any `<C: ...>` naming a subset and rejected by any naming
  more.
- A context's type is generated, unnamed, and never written down.

**Where a context may be built:** in `main`'s body, in a test source, or in a
test-only module (a path with a `testing` segment) — and never inside a lambda.
Nowhere else, which is why the purity theorem holds in ordinary code.

### Restricting what propagates

**Static confinement** — bound the callee to fewer effects. It receives the
same value and cannot use or pass on anything its bounds do not name, and that
is transitive because `C` stays opaque downstream.

```buri
fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
    let _ = io.println(ctx, msg);
    // fs.readText(ctx, "/etc/passwd")   // ERROR: C is not bounded by Fs
}
```

**Attenuation** — wrap the context in a type satisfying fewer effects, so the
callee holds a value that genuinely lacks the rest. Attenuation narrows the
whole context, never one effect out of it, which is what keeps the `ctx` rule
satisfiable.

Use confinement by default and attenuation at trust boundaries.
`core/alloc`'s `GeneralPurpose`, `Arena` and `FixedBuffer` are importable
anywhere, because `Alloc` is the one effect whose implementation grants
nothing.
