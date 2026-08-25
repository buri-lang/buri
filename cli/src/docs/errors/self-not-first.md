---
title: `self` is the first parameter or nothing
message: `self` may appear only as {position}
fix: move it to the front, or rename it if this parameter is not the receiver
---
# `self` is the first parameter or nothing

```text
error: `self` may appear only as a function's first parameter [self-not-first]
```

## What to do

Move it to the front, or rename it if this parameter is not the receiver.

## Why

`self` first and `ctx` immediately after is the whole calling convention, and
it is what lets a reader answer "what does this take, and what may it do?" from
the front of a signature.

## A program that provokes it

```buri fail code=self-not-first use=errors
fn scaled(factor: Int, self: Square): Int {
  self.side * factor
}
```
