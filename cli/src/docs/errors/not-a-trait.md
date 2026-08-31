---
title: A bound names a trait or an effect
message: `{name}` is not a trait or effect
---
# A bound names a trait or an effect

```text
error: `Bogus` is not a trait or effect [not-a-trait]
```

## What to do

Name a declared trait or effect, or declare the one you meant.

## Why

A bound is resolved to a declaration and then checked as a table lookup. There
are no `where` clauses and no structural constraints, so there is nothing a
bound could name except a trait or an effect.

## A program that provokes it

```buri fail code=not-a-trait
fn measure<T: Bogus>(x: T): Int {
    1
}
```
