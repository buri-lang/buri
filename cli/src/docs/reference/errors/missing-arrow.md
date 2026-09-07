---
title: A match arm is `pattern => expression`
message: a match arm is `pattern => expression`
fix: write `=>` here
---
# A match arm is `pattern => expression`

```text
error: a match arm is `pattern => expression` [missing-arrow]
```

## What to do

Write the `=>` between the pattern and the arm's body. The error carries the
edit as bytes, so an editor's quick fix writes it for you.

## Why

The arrow tells the pattern from the expression, which is why the two may be
spelled the same way. `1` on the left matches the value one; `1` on the right
*is* the value one. Without the arrow the parser would have to guess where the
pattern stopped.

## A program that provokes it

```buri fail code=missing-arrow
fn pick(n: Int): Int {
  match (n) {
    1 1,
    _ => 0,
  }
}
```
