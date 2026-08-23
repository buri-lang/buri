# `self` is the first parameter or nothing

```text
error: `self` may appear only as a function's first parameter [self-not-first]
```

## What to do

move it to the front, or rename it if this parameter is not the receiver

## A program that provokes it

```buri fail code=self-not-first
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

struct Square { export side: Int }

fn scaled(factor: Int, self: Square): Int {
  self.side * factor
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${scaled(2, Square { side: 3 })}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `self-not-first` — so
this page cannot describe an error the compiler has stopped emitting.
