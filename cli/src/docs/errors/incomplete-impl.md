# An `impl` supplies every method of its trait

```text
error: `Bag`'s `impl Measurable` is missing `isEmptyThing` [incomplete-impl]
```

## What to do

add `isEmptyThing` to the block, with the signature `Measurable` declares

## Why

an `impl` supplies every method its trait declares

## A program that provokes it

```buri fail code=incomplete-impl
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

trait Measurable {
  fn size(self: Self): Int;
  fn isEmptyThing(self: Self): Bool;
}

struct Bag { export count: Int }

impl Measurable for Bag {
  fn size(self: Bag): Int { self.count }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${Bag { count: 1 }.size()}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `incomplete-impl` — so
this page cannot describe an error the compiler has stopped emitting.
