# A context binds effects

```text
error: `Region` is not a declared effect [not-an-effect]
```

## What to do

Name an effect the platform declares, as in `Alloc` or `Stdout`.

## Why

A context binds effects to implementations, so each key has to be one — and the
set of them is `core/effect`'s, plus `ui/effect`'s where the platform grants
them.

## A program that provokes it

```buri fail code=not-an-effect
# from "core/host" import * as host;
from "core/effect" import { Alloc, Region, Stdout };

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Region: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("ready");
  .Ok(())
}
```
