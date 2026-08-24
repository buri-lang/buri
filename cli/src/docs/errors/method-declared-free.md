# A method is declared inside an `impl`

```text
error: `area` takes `self`, so it is a method [method-declared-free]
```

## What to do

move it into an `impl` block for its type, as in `impl Square { fn area(self: Square): Int { ... } }`

## Why

a method is found through its receiver's type, so it is declared with that type

## A program that provokes it

```buri fail code=method-declared-free use=errors
fn perimeter(self: Square): Int {
  self.side * 4
}
```
