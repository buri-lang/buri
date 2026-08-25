---
title: A rest pattern comes last
message: a rest pattern must come last
note: `[first, ..rest]` is legal; `[..init, last]` is not
fix: move `..` to the end, as in `[first, ..rest]`; matching a prefix is what an array pattern does
---
# A rest pattern comes last

```text
error: a rest pattern must come last [rest-pattern-not-last]
```

## What to do

Move `..` to the end: `[first, ..rest]` is legal, `[..init, last]` is not.

## Why

An array pattern matches a prefix and then binds the remainder. Allowing a rest
in the middle would make matching a search rather than a walk, and the cost
would be paid on every array pattern in the language.

## A program that provokes it

```buri fail code=rest-pattern-not-last
fn lastOf(xs: [Int]): Int {
  match (xs) {
    [..init, last] => last,
    _ => 0,
  }
}
```
