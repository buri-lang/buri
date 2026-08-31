---
title: A bound is named once
message: `{effect}` is bound twice
note: a spread's binding is replaced by an explicit one, but two explicit bindings of one effect are a mistake
fix: delete one of the two bindings
---
# A bound is named once

```text
error: `Alloc` is bound twice [duplicate-bound]
```

## What to do

Delete one of the two bindings.

## Why

A spread's binding is replaced by an explicit one — `context { ..Fixture(),
Fs: fs().files([]) }` is how a test overrides a default — but two explicit
bindings of one effect have no such reading, so the later one is not a silent
winner.

## A program that provokes it

```buri fail code=duplicate-bound
# from "core/effect" import { Alloc, Stdout };
# from "core/host" import * as host;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("ready");
  .Ok(())
}
```
