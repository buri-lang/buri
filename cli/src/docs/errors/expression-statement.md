# An expression statement is legal only in a test

```text
error: an expression statement is legal only in a test source [expression-statement]
```

## What to do

bind it: `let _ = ...;`, or make it the block's result expression

## Why

a block is `let`s followed by a result expression

## A program that provokes it

```buri fail code=expression-statement
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `expression-statement` — so
this page cannot describe an error the compiler has stopped emitting.
