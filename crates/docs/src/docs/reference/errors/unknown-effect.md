---
title: A context binding names a declared effect
message: `{name}` is not a declared effect
fix: name an effect the platform declares, as in `Allocator` or `Stdout`; `{name}` is not one
---
# A context binding names a declared effect

```text
error: `Region` is not a declared effect [unknown-effect]
```

## A program that provokes it

```buri fail code=unknown-effect
# from "core/io" import * as io;
# from "native" import { NativeHost };
from "platform/effect" import { Allocator, Region, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Region: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```
