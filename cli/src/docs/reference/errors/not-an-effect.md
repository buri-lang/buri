---
title: A context binds effects
message: `{name}` is not a declared effect
fix: name an effect the platform declares, as in `Allocator` or `Stdout`; `{name}` is not one
---
# A context binds effects

```text
error: `Region` is not a declared effect [not-an-effect]
```

## What to do

Every key in a context has to be an effect. The set is `core/effect`'s, plus
`ui/effect`'s where the platform grants them.

## A program that provokes it

```buri fail code=not-an-effect
from "core/effect" import { Allocator, Region, Stdout };
# from "core/host" import * as host;
# from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Region: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```
