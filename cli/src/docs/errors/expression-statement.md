---
title: An expression statement is legal only in a test
message: an expression statement is legal only in a test source
note: a block is `let`s followed by a result expression
fix: bind it: `let _ = ...;`, or make it the block's result expression
---
# An expression statement is legal only in a test

```text
error: an expression statement is legal only in a test source [expression-statement]
```

## What to do

Bind it — `let _ = ...;` — or make it the block's result expression.

## Why

A block is `let`s followed by a result expression, and there is no third
statement form. A test source is the one exception, which is what lets
`assert.eq(...)` stand alone — and there, any expression of type `()` may, a
`match` or an `if` whose branches all assert included, each terminated by `;`.

Between this rule and `result-discarded` there are exactly two places a value
can be thrown away — bound to a `_`, or left standing — and a `Result` is
refused in both, which is what makes must-use total rather than a convention.
So when the statement has type `Result`, the fix above says `.ignore()`
instead: `let _ = ...;` would only trade this error for that one.

## A program that provokes it

```buri fail code=expression-statement wrap=body
ctx.println("ready");
```
