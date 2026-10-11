# Page

A snippet may export a function besides `main`.

```buri
from "core/io" import * as io;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };

export fn helper(): I64 {
    1
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "${helper()}").mapErr(fn(e) => "no")
}
```
