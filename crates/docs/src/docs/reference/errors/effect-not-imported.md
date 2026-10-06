---
title: A context binding names an effect this file imports
message: `{name}` is an effect, but this file does not import it
fix: import it: `{import}`
---
# A context binding names an effect this file imports

```text
error: `Allocator` is an effect, but this file does not import it [effect-not-imported]
```

```buri fail code=effect-not-imported
# from "core/io" import * as io;
# from "native" import { NativeHost };
from "platform/effect" import { Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```

The name on the left of a binding is resolved like any other name, so an
effect has to be imported before a context can bind it — even when the value on
the right, `host.alloc` or a test's `alloc()`, comes from a module that does.

The binding is still checked as the effect it names, so this is the one error
reported: a call that needs that effect from the context isn't reported as
well, as `a context` not implementing it.
