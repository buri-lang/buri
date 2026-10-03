---
title: A bound is named once
message: `{effect}` is bound twice
note: a spread's binding is replaced by an explicit one, but two explicit bindings of one effect are a mistake
fix: delete one of the two bindings
---
# A bound is named once

```text
error: `Allocator` is bound twice [duplicate-bound]
```

```buri fail code=duplicate-bound
# from "core/io" import * as io;
# from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```

Overriding a spread is how a test replaces a default:
`context { ..Fixture(), FileSystemRead: fs().files([]) }`. Two explicit
bindings have no such reading, so the later one doesn't silently win.
