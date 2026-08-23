# Everything in an `impl` takes `self`

```text
error: `unit` is in an `impl` block but takes no `self` [impl-fn-without-self]
```

## What to do

give it a `self` parameter, or move it out of the `impl` block

## Why

an `impl` block declares methods; a function with no receiver is declared at the top level

## A program that provokes it

```buri fail code=impl-fn-without-self
// The converse rule: an `impl` block declares methods, so everything in one
// has a receiver. A constructor-shaped function is an ordinary declaration.
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

struct Square { export side: Int }

impl Square {
  fn unit(): Square { Square { side: 1 } }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `impl-fn-without-self` — so
this page cannot describe an error the compiler has stopped emitting.
