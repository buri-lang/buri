---
title: Or-pattern alternatives bind the same names
message: or-pattern alternatives must bind the same names
fix: bind the same names in every alternative, or split this into separate arms
---
# Or-pattern alternatives bind the same names

```text
error: or-pattern alternatives must bind the same names [or-pattern-bindings]
```

## What to do

Bind the same names in every alternative, or split this into separate arms.

## Why

One arm has one body, checked once. The body can name only what every
alternative agrees to supply, and at the same type — so an alternative that
binds `y` where another binds `x` leaves the body with a name that is sometimes
not there.

## A program that provokes it

```buri fail code=or-pattern-bindings
enum Either {
    Left(Int),
    Right(Int),
}

fn value(e: Either): Int {
    match (e) {
        .Left(x) | .Right(y) => x,
    }
}
```
