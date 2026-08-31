---
title: A context may only be built where authority enters
message: a context may not be constructed here
note: only in `main`'s body, in a test source, or in a test-only module — and never inside a lambda, since a closure that could mint authority would make the capture rule meaningless
fix: build it in `main` and pass it down as a `ctx` parameter, or make this a test source, where a context may be built per test
---
# A context may only be built where authority enters

```text
error: a context may not be constructed here [context-not-allowed]
```

## What to do

Build it in `main` and pass it down as a `ctx` parameter, or make this a test
source, where a context may be built per test.

## Why

Three places may mint authority: `main`'s body, a test source, and a test-only
module. A lambda is not one of them, and cannot be — a closure able to build a
context could hand one to a caller that never named an effect, which is exactly
what the capture rule exists to prevent.

## A program that provokes it

```buri fail code=context-not-allowed
# from "core/effect" import { Alloc, Stdout };
# from "core/host" import * as host;
# from "core/io" import * as io;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let make = fn(n: Int) => {
    let inner = context { Alloc: host.alloc, Stdout: host.stdout };
    n
  };
  let _ = io.println(ctx, "${make(1)}").ignore();
  .Ok(())
}
```
