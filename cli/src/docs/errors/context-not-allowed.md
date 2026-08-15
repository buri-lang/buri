# A context may only be built where authority enters

```text
error: a context may not be constructed here [context-not-allowed]
```

## What to do

build it in `main` and pass it down as a `ctx` parameter, or make this a test source, where a context may be built per test

## Why

only in `main`'s body, in a test source, or in a test-only module — and never inside a lambda, since a closure that could mint authority would make the capture rule meaningless

## A program that provokes it

```buri fail code=context-not-allowed
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let make = fn(n: Int) => {
    let inner = context { Alloc: host.alloc, Stdout: host.stdout };
    n
  };
  let _ = ctx.println("${make(1)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `context-not-allowed` — so
this page cannot describe an error the compiler has stopped emitting.
