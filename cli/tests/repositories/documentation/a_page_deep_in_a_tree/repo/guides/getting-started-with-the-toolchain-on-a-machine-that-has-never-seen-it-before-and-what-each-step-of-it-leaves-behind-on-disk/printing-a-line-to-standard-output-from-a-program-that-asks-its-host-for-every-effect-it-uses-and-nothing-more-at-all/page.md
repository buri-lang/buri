# Printing

A program prints through the host it is handed.

```buri run
from "core/io" import * as io;
from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "a latte").mapErr(fn(e) => "no")
}
```

```stdout
a latte
```
