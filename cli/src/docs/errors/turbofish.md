---
title: Type arguments are written without `::`
message: type arguments in an expression are written without `::`
note: `::` was needed when `<` in expression position was always a comparison; it no longer is
fix: remove the `::`, as in `list.empty<Int>()`
---
# Type arguments are written without `::`

```text
error: type arguments in an expression are written without `::` [turbofish]
```

## What to do

Remove the `::`, as in `list.empty<Int>()`.

## Why

The turbofish exists in other languages because a bare `<` in expression
position is always a comparison there, so `f<A>(x)` cannot be told from
`(f < A) > (x)`. Buri's comparison operators are non-associative, so the second
reading is not a program either — there is no source the two readings both
accept and disagree about, and the `::` was the price of an ambiguity that
turned out not to be one.

Two spellings of one thing would be worse than either, so the old one is an
error rather than a second way to write it. The error carries the edit as
bytes, so `buri lint --fix` and an editor's quick fix migrate a file that still
has it.

## A program that provokes it

```buri fail code=turbofish
# from "core/list/lib.buri" import * as list;
fn empty(): [Int] {
  list.empty::<Int>()
}
```
