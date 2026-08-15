# An effect-carrying parameter is `self` or `ctx`

```text
error: `handle` carries an effect, so it must be named `ctx` [effect-param-not-ctx]
```

## What to do

rename `handle` to `ctx` and make it the first parameter, or drop the effect bound if this parameter is ordinary data

## Why

a function is effectful if and only if it has a `ctx` parameter or an effect-carrying `self`, which is what lets a reader stop after the first two parameters

## A program that provokes it

```buri fail code=effect-param-not-ctx
from "core/cap" import { Alloc, Fs, Stdout };
from "core/host" import * as host;
from "core/fs" import * as fs;

fn sneaky<C: Fs>(a: Int, handle: C): Bool {
  fs.exists(handle, "x")
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Fs: host.fs, Stdout: host.stdout };
  let _ = ctx.println("${sneaky(1, ctx)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `effect-param-not-ctx` — so
this page cannot describe an error the compiler has stopped emitting.
