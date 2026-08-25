---
title: There are no implicit conversions
message: expected `{expected}`, found `{actual}`
---
# There are no implicit conversions

```text
error: expected `I64`, found `I32` [type-mismatch]
```

## What to do

Convert explicitly. The diagnostic names the conversion: `.toI64()` is exact
for every `I32`, while a narrowing one returns a `Result` because not every
value fits.

## Why

No promotion of any kind, in either direction. A language that widened silently
would make the width of an arithmetic result a property of the expression's
shape rather than of what was written, and the one place it matters is the one
place nobody looks.

## A program that provokes it

```buri fail code=type-mismatch
fn widenByAccident(a: I32, b: I64): I64 {
  a + b
}
```
