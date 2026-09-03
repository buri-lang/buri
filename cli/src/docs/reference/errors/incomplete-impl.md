---
title: An `impl` supplies every method of its trait
message: `{type}`'s `impl {trait}` is missing {methods}
note: an `impl` supplies every method its trait declares
fix: add {methods} to the block, with the signature `{trait}` declares
---
# An `impl` supplies every method of its trait

```text
error: `Bag`'s `impl Measurable` is missing `isEmptyThing` [incomplete-impl]
```

## What to do

Add `isEmptyThing` to the block, with the signature `Measurable` declares.

## Why

There are no default method bodies, so a trait's method list is the whole of
what an `impl` owes it. A partial conformance would be a value that satisfies a
bound and aborts when the bound is used.

## A program that provokes it

```buri fail code=incomplete-impl
trait Measurable {
    fn size(self): Int;
    fn isEmptyThing(self): Bool;
}

struct Bag {
    export count: Int,
}

impl Measurable for Bag {
    fn size(self): Int {
        self.count
    }
}
```
