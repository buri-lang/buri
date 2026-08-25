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

A literal is checked against the type it is pinned to rather than widened to
fit it, so the one class of overflow that is decidable at compile time is
decided there. `U64` holds 0 to 18446744073709551615.

## A program that provokes it

```buri fail code=literal-out-of-range wrap=body
let w: U64 = 18_446_744_073_709_551_616;
```
