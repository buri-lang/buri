---
title: A binding is given its value once
message: there is no assignment; a binding is given its value once, where it is declared
fix: bind the new value to a new name with `let`, or return it from the expression that computes it
---
# A binding is given its value once

```text
error: there is no assignment; a binding is given its value once, where it is declared [no-assignment]
```

## What to do

Write a new binding rather than overwriting the old one. Where the value is
built up in steps, each step is a `let` of its own, and where it is built up in
a loop, it is the value the recursion or the fold returns.

## Why

Every binding is final: there is no assignment operator, no `mut`, and no
interior mutability. A name therefore means one value everywhere it is in
scope, which is what lets a reader answer "what is this?" by finding the one
line that says so, and what lets the compiler move a value rather than copy it.

## A program that provokes it

```buri fail code=no-assignment
fn total(): Int {
  let n = 1;
  n = 2;
  n
}
```
