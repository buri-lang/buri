---
title: A struct pattern mentions every field
message: this pattern does not mention {fields}
fix: match {fields} too, or end the pattern with `..` to ignore the rest
---
# A struct pattern mentions every field

```text
error: this pattern does not mention `y` [missing-field-pattern]
```

## What to do

Match `y` too, or end the pattern with `..` to ignore the rest.

## Why

Adding a field should be a compile error everywhere the type is taken apart, and
`..` is how a pattern opts out.

## A program that provokes it

```buri fail code=missing-field-pattern
struct Point {
    export x: Int,
    export y: Int,
}

fn xOf(p: Point): Int {
    let Point { x } = p;
    x
}
```
