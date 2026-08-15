# `main` has one shape

```text
error: `main` declares no generic parameters [main-signature]
```

## What to do

drop them: `main` is called by the runtime, so there is nothing to infer them from

## A program that provokes it

```buri fail code=main-signature
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main<T>(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `main-signature` — so
this page cannot describe an error the compiler has stopped emitting.
