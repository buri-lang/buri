# `self` is the first parameter or nothing

```text
error: `self` may appear only as a function's first parameter [self-not-first]
```

## What to do

move it to the front, or rename it if this parameter is not the receiver

## A program that provokes it

```buri fail code=self-not-first use=errors
fn scaled(factor: Int, self: Square): Int {
  self.side * factor
}
```
