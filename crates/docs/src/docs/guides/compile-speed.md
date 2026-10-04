# How Buri compiles fast

Compile speed is a language design decision first. A checker is fast when
parsing never asks what a name means, when checking one function never depends
on checking another, and when resolving a bound is a lookup, not a search.

Each section below is a promise the language makes. Measure any proposed
language addition against them.

## Parsing depends on nothing

No grammar production consults name resolution or types. A file parses with no
knowledge of any other file, so every file parses in one pass, in parallel.

That is why `if` and `match` subjects are parenthesized, why there are no
expression statements, and why there is no cast operator.
`design/grammar-rationale.md` lists each such decision and its cost.

Anything that needs only a parse tree needs no build graph. `buri format` and
`buri lint` never check types, and the language server can answer about a file
whose dependencies it has never seen.

## Signatures are mandatory, so bodies check independently

Every top-level function writes its parameter and return types, so **no
inference crosses a function boundary.**

```buri
/// The signature is the whole contract: `total`'s body can't change what a
/// caller sees, and a caller can't change how this body checks.
fn total(xs: [Int]): Int {
    xs.fold(fn(n, x) => n + x, 0)
}
```

Bodies check in parallel, in any order, and editing one body never invalidates
another's check.

One consequence you'll meet: a type variable still unconstrained when a body
finishes checking becomes `()`. This is not the literal defaulting of
[`language/types.md` §5.1.1](../language/types.md). It shows up with
`assert.some(o)` on an `Option` whose payload the program never names: the
failure renders the `Option`, not the payload.

## Resolution and inference interleave, in a single traversal

Resolving `x.f()` requires knowing what `x` is, so name resolution and type
inference share one traversal. Four rules keep it from becoming a fixpoint:

- Method resolution needs only the receiver's **head type constructor**.
  `xs.first()` resolves in `core/list` whether `xs` is `[Int]` or `[T]` for an
  unresolved `T`.
- Type information flows **outside-in and left-to-right**. A lambda's parameter
  types come from the expected type at its call site, known before the body is
  visited.
- There is no overloading, so a name plus one type constructor selects exactly
  one definition.
- Conformance is nominal ([`language/types.md`
  §5.12.1](../language/types.md)), so a bound is a table lookup.

Overloading by argument types, return-type-directed dispatch, structural
conformance, or a method call before its receiver's type constructor is known
would each break this.

## A module's surface is exactly its exported declarations

Conformance is declared, not inferred from shape, so adding or removing a
private function can't change what another module sees. The compiler rechecks a
dependent only when a declaration it names changes, and editing a function body
recompiles that library and nothing upstream. That is the build system's
`interface` and `compile` split
([`build/hermeticity.md`](../reference/build/hermeticity.md)).

## Monomorphization is a codegen concern, not a checking one

A generic body checks once, polymorphically, and bounds are verified at each
call site, so checking is O(code), not O(code × instantiations). Generics are
still monomorphized, with no run-time dictionaries, but only after checking and
only for code the entry point reaches.

## Nothing in the checker requires a fixpoint

No recursive trait solving: no blanket implementations, no associated types, no
supertraits ([`language/types.md` §5.12.5](../language/types.md)). No variance
inference, because there is no subtyping. No effect inference, because effects
are declared. No cross-module exhaustiveness.

Those trait features are deferred *together*: each looks reasonable alone, but
together they turn a lookup into a search (`design/non-goals.md` keeps the
list).

## What this does not promise

Monomorphization and native optimization are real work, proportional to the
code the program reaches, and a cold build still builds the standard library.
The promise is that work is *proportional and parallel*: nothing in the front
end is superlinear in program size, and no file waits on another beyond the
dependency edges you declared.
[`build/hermeticity.md`](../reference/build/hermeticity.md) covers the cache,
and `design/PERFORMANCE.md` has the measurements.
