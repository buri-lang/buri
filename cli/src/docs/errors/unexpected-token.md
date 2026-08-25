---
title: The grammar expected something else here
message: expected {expected}, found {found}
---
# The grammar expected something else here

```text
error: expected a pattern, found `..` [unexpected-token]
```

## What to do

Write what the message names. This is the parser's catch-all, so the useful
half of it is always the "expected" part.

## A program that provokes it

```buri fail code=unexpected-token
fn middle(xs: [Int]): Int {
  match (xs) {
    [..a, m, ..b] => m,
    _ => 0,
  }
}
```
