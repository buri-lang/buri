# How Buri compiles fast

Compile speed is a language design decision before it is a compiler engineering
one. A checker is fast when parsing never has to ask what a name means, when
checking one function cannot depend on checking another, and when resolving a
bound is a lookup rather than a search.

Everything below is a promise the language makes, not an optimization the
compiler happens to have. Measure a proposed addition to the language against
them.

## Parsing depends on nothing

No production in the grammar consults name resolution or types. The compiler
parses a file with no knowledge of any other file, so parsing is one pass and
every file parses in parallel with no coordination.

That is why `if` and `match` subjects are parenthesized, why there are no
expression statements, and why there is no cast operator.
`design/grammar-rationale.md` lists every such decision with what it cost.

So anything that needs only a parse tree is fast and needs no build graph.
`buri format` and `buri lint` never check types, and the language server can
answer about a file whose dependencies it has never seen.

## Signatures are mandatory, so bodies check independently

Every top-level function writes its parameter types and its return type. That
annotation buys the rest of this page: **no inference crosses a function
boundary.**

```buri
/// The signature is the whole contract. Nothing about `total`'s body can
/// change what a caller sees, and nothing about a caller can change how this
/// body checks.
fn total(xs: [Int]): Int {
    xs.fold(fn(n, x) => n + x, 0)
}
```

Bodies therefore check in parallel, in any order, and editing one body can never
invalidate the check of another. A whole-program inference algorithm cannot make
that promise.

One consequence shows up in real programs. A type variable still unconstrained
when a body finishes checking stays unconstrained, because no other body and no
signature can reach it, so it becomes `()` — not the literal defaulting of
[`language/types.md` §5.1.1](../language/types.md). You meet it with
`assert.some(o)` on an `Option` whose payload the program never names: what it
renders is the `Option`, not the payload.

## Resolution and inference interleave, in a single traversal

Resolving `x.f()` requires knowing what `x` is, so name resolution and type
inference cannot be separate passes. Four things keep them a single traversal
rather than a fixpoint:

- Method resolution needs only the receiver's **head type constructor**, not its
  full type. `xs.first()` resolves in `core/list` whether `xs` is `[Int]` or
  `[T]` for an unresolved `T`.
- Type information flows **outside-in and left-to-right**. A lambda's parameter
  types come from the expected type at its call site, which is known before the
  body is visited.
- There is no overloading, so a name plus one type constructor selects exactly
  one definition.
- Conformance is nominal ([`language/types.md`
  §5.12.1](../language/types.md)), so a bound is a table lookup rather than a
  search that could need the rest of the program.

What would break it: overloading resolved by argument types, return-type-directed
dispatch, structural conformance, or any construct where a method call can appear
before its receiver's type constructor is determined.

## A module's surface is exactly its exported declarations

Because conformance is declared rather than inferred from shape, adding or
removing a private function cannot change what any other module sees. So
invalidation is precise: the compiler rechecks a dependent only when a
declaration it names actually changes, and editing a function body recompiles
that library and nothing upstream of it. That is the build system's split
between `interface` and `compile`
([`build/hermeticity.md`](../reference/build/hermeticity.md)).

## Monomorphization is a codegen concern, not a checking one

The compiler checks a generic body once, polymorphically, and verifies bounds at
each call site, so checking is O(code), not O(code × instantiations). Generics
are still monomorphized — no dictionaries at run time — but that happens after
everything has been checked, on the code the entry point reaches.

## Nothing in the checker requires a fixpoint

No recursive trait solving: no blanket implementations, no associated types, no
supertraits ([`language/types.md` §5.12.5](../language/types.md)). No variance
inference, because there is no subtyping. No effect inference, because effects
are declared rather than deduced. No cross-module exhaustiveness. Each absence
is a loop the compiler does not run to a fixed point.

Those trait features are deferred *together*: one at a time each looks
reasonable, and collectively they turn a lookup into a search
(`design/non-goals.md` keeps the list).

## What this does not promise

Monomorphization and the native backend's optimizations are real work,
proportional to the code the program reaches, and a cold build still builds the
standard library. What the invariants promise is that the work is *proportional
and parallel*: nothing in the front end is superlinear in the size of your
program, and nothing makes one file wait on another beyond the dependency edges
you declared. [`build/hermeticity.md`](../reference/build/hermeticity.md) covers
what the cache does with that, and `design/PERFORMANCE.md` has the
measurements.
