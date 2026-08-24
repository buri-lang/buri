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
  fn size(self: Self): Int;
  fn isEmptyThing(self: Self): Bool;
}

struct Bag { export count: Int }

impl Measurable for Bag {
  fn size(self: Bag): Int { self.count }
}
```
