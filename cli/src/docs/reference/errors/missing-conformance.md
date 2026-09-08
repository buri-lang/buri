---
title: Conformance is declared, never inferred
message: `{type}` does not implement `{trait}`
---
# Conformance is declared, never inferred

```text
error: `HostStdout` does not implement `Allocator` [missing-conformance]
```

## What to do

Bind a value whose type has `impl Allocator for ...`.

## Why

You declare conformance; the compiler never infers it. A type with all the right
methods still does not satisfy an effect until an `impl` says so — which is why
a test double is a struct with those methods and an `impl` block.

## A program that provokes it

```buri fail code=missing-conformance
# from "core/effect" import { Allocator, Stdout };
# from "core/host" import * as host;
# from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.stdout,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```
