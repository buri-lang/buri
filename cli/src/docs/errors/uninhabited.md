# A type with no finite value cannot be constructed

```text
error: `Endless` can never be constructed [uninhabited]
```

## What to do

give `Endless` a variant that does not mention itself, the way `.None` terminates an `Option`

## Why

every variant recurses, so building one would need one already

## A program that provokes it

```buri fail code=uninhabited
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

enum Endless { Node(Endless) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `uninhabited` — so
this page cannot describe an error the compiler has stopped emitting.
