---
title: A literal must fit the type it is pinned to
message: {literal} is not representable in `{type}`
---
# A literal must fit the type it is pinned to

```text
error: 18_446_744_073_709_551_616 is not representable in `U64` [literal-out-of-range]
```

## What to do

Write a value inside the type's range, or annotate a wider type.

## Why

The compiler checks a literal against the type it is pinned to rather than
widening it to fit, so it settles the one class of overflow it can decide at
compile time. `U64` holds 0 to 18446744073709551615.

## A program that provokes it

```buri fail code=literal-out-of-range wrap=body
let w: U64 = 18_446_744_073_709_551_616;
```
