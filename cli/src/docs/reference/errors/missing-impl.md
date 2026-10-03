---
title: A type implements the traits its uses need
message: '`{type}` does not implement `{trait}`'
---
# A type implements the traits its uses need

```text
error: `HostStdout` does not implement `Allocator` [missing-impl]
```

```buri fail code=missing-impl
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

```buri fail code=missing-impl
struct Point {
    export x: Int,
    export y: Int,
}

fn same<T: Equal>(a: T, b: T): Bool {
    a == b
}

fn check(p: Point): Bool {
    same(p, p)
}
```

A context binding and a bound both ask for an `impl`. Having the right methods
isn't enough: a type conforms only once an `impl` says so. That's why a test
double is a struct plus an `impl` block.
