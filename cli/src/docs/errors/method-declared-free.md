# A method is declared inside an `impl`

```text
error: `area` takes `self`, so it is a method [method-declared-free]
```

## What to do

move it into an `impl` block for its type, as in `impl Square { fn area(self: Square): Int { ... } }`

## Why

a method is found through its receiver's type, so it is declared with that type

## A program that provokes it

```buri fail code=method-declared-free
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

Compiled by the test suite, which checks that it still produces `method-declared-free` — so
this page cannot describe an error the compiler has stopped emitting.
