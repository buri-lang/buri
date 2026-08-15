# A module exports what it says it exports

```text
error: "core/list" does not export `notAThing` [no-such-export]
```

## What to do

add `export` to `notAThing`'s declaration in "core/list", or drop it from this list

## Why

a re-export may name only what its module path exports

## A program that provokes it

```buri fail code=no-such-export
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/list" export { notAThing };

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `no-such-export` — so
this page cannot describe an error the compiler has stopped emitting.
