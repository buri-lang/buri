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
`??`.

## Why

`?` is an early return of the error, so the function it appears in has to be
able to return one. A version that aborted instead would make every `?` a
possible crash, which is the property this language does not want.

## A program that provokes it

```buri fail code=question-mark-mismatch
fn unwrap(r: Result<Int, Str>): Int {
    let n = r?;
    n
}
```
