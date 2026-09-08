---
title: A `context` is not exported from a test-only module
message: a `context` may be exported only from a test-only module
note: a test module is anything under a `testing` directory
fix: drop the `export`, or move it into a test-only module
---

```buri fail code=context-export
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export context Fixture {
    Allocator: host.alloc,
    Stdout: host.stdout,
}

export fn main(): Result<(), Str> {
    let ctx = Fixture();
    let _ = io.println(ctx, "hi").ignore();
    .Ok(())
}
```

To fix, drop the `export`, or move the context into a test-only module:

```buri ignore why="the fixture lives in a second module, and a doctest block is one file"
from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

// define the context in a test only module instead
// this module is "test only" because it has a "testonly" directory
from "//libs/testonly" import { Fixture };

export fn main(): Result<(), Str> {
    let ctx = Fixture();
    let _ = io.println(ctx, "hi").ignore();
    .Ok(())
}
```
