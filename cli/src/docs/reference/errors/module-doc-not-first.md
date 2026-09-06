---
title: `//!` documents the module, so it comes first
message: `//!` documents the module, so it must come first
fix: move it above the first declaration, or write `///` to document the declaration below it
---
# `//!` documents the module, so it comes first

```text
error: `//!` documents the module, so it must come first [module-doc-not-first]
```

## What to do

Move it above the first declaration, or write `///` to document the declaration
below it.

## Why

`///` attaches downward, to the declaration beneath it. `//!` attaches upward,
to whatever contains it, which at the top of a file is the module. Written lower
down it has nothing above it to attach to except a declaration that already has
its own comment form. So it is a `///` typo far more often than it is
deliberate.

## A program that provokes it

```buri fail code=module-doc-not-first
export fn area(side: Int): Int { side * side }

//! This belongs at the top of the file, above everything.
export fn perimeter(side: Int): Int { side * 4 }
```
