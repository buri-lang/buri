---
title: A default for an absent value is `withDefault`
message: '`??` is retired'
note: the operator was a second spelling of a method that already existed, and the only one that needed a right-associative rung of its own in the grammar
fix: write the default as a method call, as in `x.withDefault(0)`
---
# A default for an absent value is `withDefault`

```text
error: `??` is retired [retired-coalesce]
```

## What to do

Call `withDefault` on the value instead. `Option<T>` and `Result<T, E>` both
have it, and it takes the same default the operator's right-hand side was:

```buri
fn firstOr(xs: [Int], fallback: Int): Int {
    xs.get(0).withDefault(fallback)
}
```

A chain of them chains as method calls do —
`a.withDefault(b.withDefault(c))` where `a ?? b ?? c` used to be written, or
`a.or(b).withDefault(c)` when `a` and `b` are both `Option<T>`.

The one thing the operator did that the method does not is leave the default
*unevaluated* until it is needed. `withDefault` takes it as an argument, so it
is computed either way. Where that matters — a fallback that allocates, or a
call worth not making — write the `match` out, which is the only shape that
promises the absent branch is the one that runs it:

```buri
fn portOr(port: Option<Int>): Int {
    match (port) {
        .Some(p) => p,
        .None => expensiveDefault(),
    }
}

fn expensiveDefault(): Int {
    8080
}
```

## Why

`??` was a second way to say `withDefault`, and the two were not equally
reachable: the method is found by typing `.` after a value, and the operator had
to be learnt from the precedence table. It was also the only right-associative
rung in that table, so every reader of the grammar paid for it once and every
implementation of the parser paid for it again.

Removing it leaves one spelling, and the one that composes: a method sits in a
chain beside `map`, `filter` and `okOr`, where the operator had to interrupt one.

## A program that provokes it

```buri fail code=retired-coalesce
fn portOr(port: Option<Int>): Int {
    port ?? 8080
}
```
