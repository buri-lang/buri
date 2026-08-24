# An expression statement is legal only in a test

```text
error: an expression statement is legal only in a test source [expression-statement]
```

## What to do

bind it: `let _ = ...;`, or make it the block's result expression

## Why

a block is `let`s followed by a result expression

## A program that provokes it

```buri fail code=expression-statement wrap=body
ctx.println("ready");
```
