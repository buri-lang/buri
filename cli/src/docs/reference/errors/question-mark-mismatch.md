---
title: `?` propagates into a matching return type
message: `?` on {container} needs {container} return type, not `{type}`
---
# `?` propagates into a matching return type

```text
error: `?` on a `Result` needs a `Result` return type, not `I64` [question-mark-mismatch]
```

## What to do

Return a `Result` from this function, or handle the error here with `match` or
`withDefault`.

## Why

`?` is an early return of the error, so the function it sits in has to be able
to return one.

## A program that provokes it

```buri fail code=question-mark-mismatch
fn unwrap(r: Result<Int, Str>): Int {
    let n = r?;
    n
}
```
