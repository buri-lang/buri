---
title: There are no implicit conversions
message: expected {expected}, found {found}
---
# There are no implicit conversions

```text
error: expected `I64`, found `I32` [type-mismatch]
```

## What to do

Convert explicitly. The diagnostic names the conversion: `.toI64()` is exact for
every `I32`, while a narrowing one returns a `Result` because not every value
fits.

A bare numeric literal is reported as `Int` or `Float`, the type it takes when
nothing pins it. It is not held to that: annotate it — `let x: F64 = 1.0`, or
`let x: U8 = 1` — and it becomes any type of the same kind.

## Why

There is no promotion of any kind, in either direction. A language that widened
silently would make the width of an arithmetic result depend on the shape of the
expression rather than on what you wrote.

## A program that provokes it

```buri fail code=type-mismatch
fn widenByAccident(a: I32, b: I64): I64 {
    a + b
}
```
