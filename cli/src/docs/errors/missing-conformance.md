---
title: Conformance is declared, never inferred
message: `{type}` does not implement `{trait}`
---
# Conformance is declared, never inferred

```text
error: `HostStdout` does not implement `Alloc` [missing-conformance]
```

## What to do

Bind a value whose type has `impl Alloc for ...`.

## Why

Conformance is declared and never inferred, so a type with all the right
methods still does not satisfy an effect until an `impl` says it does. An
effect is an ordinary interface, which is why a test double is a struct with
those methods and an `impl` block.

## A program that provokes it

```buri fail code=missing-conformance
# from "core/effect/lib.buri" import { Alloc, Stdout };
# from "core/host/lib.buri" import * as host;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.stdout, Stdout: host.stdout };
  let _ = ctx.println("ready");
  .Ok(())
}
```
