# `//!` documents the module, so it comes first

```text
error: `//!` documents the module, so it must come first [module-doc-not-first]
```

## What to do

Move it above the first declaration, or write `///` to document the declaration
below it.

## Why

`///` attaches downward, to the declaration beneath it. `//!` attaches upward,
to the thing that contains it — which, at the top of a file, is the module. One
written lower down has nothing above it to attach to except a declaration that
already has its own comment form, so it is a `///` typo far more often than it
is what was meant.

## A program that provokes it

```buri fail code=module-doc-not-first
export fn area(side: Int): Int { side * side }

//! This belongs at the top of the file, above everything.
export fn perimeter(side: Int): Int { side * 4 }
```
