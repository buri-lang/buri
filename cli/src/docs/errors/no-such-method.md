# A method is looked up in its type's defining module

```text
error: `Square` has no method `area` [no-such-method]
```

## What to do

check the spelling, or declare it in `impl Square { ... }` in that type's own module — a method may not be added from anywhere else

## A program that provokes it

```buri fail code=no-such-method
// A method is declared inside an `impl` block for its type. Taking `self` at
// the top level names the shape of a method in a place that has no receiver
// type to attach it to.
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct Square { export side: Int }

fn area(self: Square): Int {
  self.side * self.side
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${Square { side: 3 }.area()}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `no-such-method` — so
this page cannot describe an error the compiler has stopped emitting.
