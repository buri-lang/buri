# Everything in an `impl` takes `self`

```text
error: `unit` is in an `impl` block but takes no `self` [impl-fn-without-self]
```

## What to do

Give it a `self` parameter, or move it out of the `impl` block.

## Why

An `impl` block declares methods, and a method is found through its receiver's
type. A constructor-shaped function has no receiver, so it is an ordinary
top-level declaration.

## A program that provokes it

```buri fail code=impl-fn-without-self use=errors
impl Square {
  fn unit(): Square { Square { side: 1 } }
}
```
