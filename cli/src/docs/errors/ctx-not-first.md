# `ctx` comes first, or immediately after `self`

```text
error: `ctx` must come first, or immediately after `self` [ctx-not-first]
```

## What to do

move `ctx` to that position

## Why

the calling convention is receiver first, context second, everything else after

## A program that provokes it

```buri fail code=ctx-not-first
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

fn shout<C: Stdout>(times: Int, ctx: C): () {
  io.println(ctx, "loud")
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = shout(2, ctx);
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `ctx-not-first` — so
this page cannot describe an error the compiler has stopped emitting.
