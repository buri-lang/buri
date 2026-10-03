---
title: A `context` is not exported from a test-only module
message: a `context` may be exported only from a test-only module
note: a test module is anything under a `testing` directory
fix: drop the `export`, or move it into a test-only module
---
```buri fail code=context-export
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

export context Fixture {
    Allocator: host.alloc,
    Stdout: host.stdout,
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = Fixture();
    let _ = io.println(ctx, "hi").ignore();
    .Ok(())
}
```

Moved into a test-only module, it's imported like anything else:

```buri ignore why="the fixture lives in a second module, and a doctest block is one file"
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

// test-only, because it sits under a `testonly` directory
from "//libs/testonly" import { Fixture };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = Fixture();
    let _ = io.println(ctx, "hi").ignore();
    .Ok(())
}
```
