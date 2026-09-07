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
to whatever contains it — at the top of a file, the module. Lower down it has
nothing to attach to, so it is almost always a `///` typo.

## A program that provokes it

```buri fail code=module-doc-not-first
export fn area(side: Int): Int { side * side }

//! This belongs at the top of the file, above everything.
export fn perimeter(side: Int): Int { side * 4 }
```
