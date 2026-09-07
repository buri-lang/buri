---
title: A `match` covers every case
message: this `match` does not cover `{witness}`
label: not covered
---
# A `match` covers every case

```text
error: this `match` does not cover `.Empty` [match-not-exhaustive]
```

## What to do

Add an arm for the case the diagnostic names, or a `_` arm for everything left.

## Why

Exhaustiveness makes adding a variant a compile error at every place that has to
care. A `_` arm opts out of that for one `match`.

## A program that provokes it

```buri fail code=match-not-exhaustive
enum Shape {
    Circle(Int),
    Square(Int),
    Empty,
}

fn describe(s: Shape): Int {
    match (s) {
        .Circle(r) => r,
        .Square(n) => n,
    }
}
```
