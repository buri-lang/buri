# A bound is named once

```text
error: `Alloc` is bound twice [duplicate-bound]
```

## What to do

delete one of the two bindings

## Why

a spread's binding is replaced by an explicit one, but two explicit bindings of one effect are a mistake

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
