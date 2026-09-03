---
title: `if` is an expression, so it needs an `else`
message: `if` requires an `else` branch
note: `if` is an expression, so both branches must produce a value of the same type
fix: add `else {{ ... }}`; an `if` is an expression, so it has a value either way
---
# `if` is an expression, so it needs an `else`

```text
error: `if` requires an `else` branch [if-without-else]
```

## What to do

Add `else { ... }`.

## Why

There is no statement form of `if` to fall back on. An `if` has a value, so it
has one on both paths, and a missing `else` is a value the language would have
to invent.

## A program that provokes it

```buri fail code=if-without-else
fn sign(n: Int): Int {
  let label = if (n > 0) { 1 };
  label
}
```
