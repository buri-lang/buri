---
title: Everything in an `impl` takes `self`
message: `{name}` is in an `impl` block but takes no `self`
note: an `impl` block declares methods; a function with no receiver is declared at the top level
fix: give it a `self` parameter, or move it out of the `impl` block
---
# Everything in an `impl` takes `self`

```text
error: `unit` is in an `impl` block but takes no `self` [impl-fn-without-self]
```

## What to do

Give it a `self` parameter, or move it out of the `impl` block.

## Why

Method lookup goes through the receiver's type, so a function with no receiver
belongs at the top level.

## A program that provokes it

```buri fail code=impl-fn-without-self use=errors
impl Square {
    fn unit(): Square {
        Square { side: 1 }
    }
}
```
