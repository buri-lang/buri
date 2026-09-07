---
title: A block-like expression is not the head of a postfix chain
message: {construct} is an operand, not the head of a `{token}`
fix: parenthesise it, or bind it with `let` and go on from the name
---
# A block-like expression is not the head of a postfix chain

```text
error: a `match` is an operand, not the head of a `.` [postfix-on-a-block]
```

## What to do

Put the block-like expression in parentheses, or — usually clearer — give it a
name with `let` and write the field access, call or index against the name.

## Why

`if`, `match`, `context` and a bare block are expressions, and they end with
`}`. If a `}` could be followed by `.`, `(` or `[`, then
`if (c) { a } else { b } { x: 1 }` would have two readings: a struct literal
headed by the `if`, or an `if` followed by a block. Neither you nor the parser
could tell which was meant (design/grammar-rationale.md 12.13).

## A program that provokes it

```buri fail code=postfix-on-a-block
struct Point { export x: Int, export y: Int }

fn xOf(p: Point): Int {
  match (p) {
    Point { x, y: _ } => Point { x, y: 0 },
  }.x
}
```
