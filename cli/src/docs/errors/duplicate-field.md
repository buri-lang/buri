# A field name is used once

```text
error: `side` is already a field of `Square` [duplicate-field]
```

## What to do

rename the method, or rename the field

## Why

a `.` resolves to a field before a method, so the two may not share a name

## A program that provokes it

```buri fail code=duplicate-field
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct Square { export side: Int }

impl Square {
  fn side(self: Square): Int {
    self.side * 2
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let sq = Square { side: 3 };
  let _ = ctx.println("${sq.side}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `duplicate-field` — so
this page cannot describe an error the compiler has stopped emitting.
