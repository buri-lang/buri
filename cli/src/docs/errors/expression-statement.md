# An expression statement is legal only in a test

```text
error: an expression statement is legal only in a test source [expression-statement]
```

## What to do

Bind it — `let _ = ...;` — or make it the block's result expression.

## Why

A block is `let`s followed by a result expression, and there is no third
statement form. That is the hole that must-use closes: with no expression
statements, `let _ =` is the only way to discard a value, so `Result` cannot be
dropped by accident. A test source is the one exception, which is what lets
`assert.eq(...)` stand alone.

## A program that provokes it

```buri fail code=expression-statement wrap=body
ctx.println("ready");
```
