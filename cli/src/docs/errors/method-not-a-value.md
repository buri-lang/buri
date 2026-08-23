# A method is not a value

```text
error: `area` is a method, and a method is not a value [method-not-a-value]
```

## What to do

call it on a receiver: `x.area()`; to pass it on, wrap it in a lambda: `fn(x) => x.area()`

## A program that provokes it

```buri fail code=method-not-a-value
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

struct Square { export side: Int }

impl Square {
  fn area(self: Square): Int { self.side * self.side }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let sq = Square { side: 3 };
  let f = sq.area;
  let _ = ctx.println("${f()}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `method-not-a-value` — so
this page cannot describe an error the compiler has stopped emitting.
