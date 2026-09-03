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

The arrow is what tells the pattern from the expression, and it is the reason a
pattern and an expression may be spelled the same way. `1` on the left of the
arrow matches the value one; `1` on the right is the value one. Without the
arrow the two readings are the same tokens, and the parser would have to guess
where one stopped — so the arrow is required rather than inferred, and its
absence is this error instead of a mismatch three lines further on.

## A program that provokes it

```buri fail code=missing-arrow
fn pick(n: Int): Int {
  match (n) {
    1 1,
    _ => 0,
  }
}
```
