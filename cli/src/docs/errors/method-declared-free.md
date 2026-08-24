# A method is declared inside an `impl`

```text
error: `area` takes `self`, so it is a method [method-declared-free]
```

## What to do

Move it into an `impl` block for its type.

## Why

A method is found through its receiver's type, so it is declared with that
type — in an `impl` block, in the module that declares the type. Taking `self`
at the top level names the shape of a method in a place that has no receiver
type to attach it to.

## A program that provokes it

```buri fail code=method-declared-free use=errors
fn perimeter(self: Square): Int {
  self.side * 4
}
```
