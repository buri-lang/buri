# Everything in an `impl` takes `self`

```text
error: `unit` is in an `impl` block but takes no `self` [impl-fn-without-self]
```

## What to do

give it a `self` parameter, or move it out of the `impl` block

## Why

an `impl` block declares methods; a function with no receiver is declared at the top level

## A program that provokes it

```buri fail code=impl-fn-without-self use=errors
impl Square {
  fn unit(): Square { Square { side: 1 } }
}
```
