---
title: Comparison operators do not chain
message: comparison operators are non-associative
note: write `a < b && b < c` rather than `a < b < c`
fix: write `a < b && b < c` rather than `a < b < c`
---
# Comparison operators do not chain

```text
error: comparison operators are non-associative [chained-comparison]
```

## What to do

Write `a < b && b < c`.

## Why

Non-associativity is not a taste decision here. It is what makes `f<T>(x)`
readable as a call: under it, `(f < T) > (x)` is not a program either, so there
is no source the two readings both accept and disagree about. `a < b < c` is
therefore refused where a chaining language would have quietly parsed it as
`(a < b) < c`.

## A program that provokes it

```buri fail code=chained-comparison
fn between(a: Int, b: Int, c: Int): Bool {
  a < b < c
}
```
