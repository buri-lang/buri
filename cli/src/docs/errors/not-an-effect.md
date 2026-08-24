# A context binds effects

```text
error: `Region` is not a declared effect [not-an-effect]
```

## What to do

name an effect the platform declares, as in `Alloc` or `Stdout`; `Region` is not one

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
