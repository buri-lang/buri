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

Use `match`. There is no exception to throw when the value does not fit, so the
other cases have to be written out.

## A program that provokes it

```buri fail code=refutable-pattern
fn unwrap(o: Option<Int>): Int {
    let .Some(n) = o;
    n
}
```
