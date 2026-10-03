---
title: Conformance is declared, never inferred
message: `{type}` does not implement `{trait}`
---
# Conformance is declared, never inferred

```text
error: `HostStdout` does not implement `Allocator` [missing-conformance]
```

```buri fail code=missing-conformance
# from "core/io" import * as io;
# from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.stdout,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```

Bind a value whose type has an `impl` of the trait. Having the right methods
isn't enough: a type conforms only once an `impl` says so. That's why a test
double is a struct plus an `impl` block.
