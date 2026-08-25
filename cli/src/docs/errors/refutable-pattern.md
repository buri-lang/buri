---
title: A `let` pattern must match every value
message: this pattern does not match every value of its type
note: a `let` binds unconditionally, so its pattern has to be irrefutable
fix: use `match`, which makes you say what the other cases do
---
# A `let` pattern must match every value

```text
error: this pattern does not match every value of its type [refutable-pattern]
```

## What to do

Use `match`, which makes you say what the other cases do.

## Why

A `let` binds unconditionally and there is no exception to throw when it does
not fit, so its pattern has to be one that cannot fail.

## A program that provokes it

```buri fail code=refutable-pattern
fn unwrap(o: Option<Int>): Int {
  let .Some(n) = o;
  n
}
```
