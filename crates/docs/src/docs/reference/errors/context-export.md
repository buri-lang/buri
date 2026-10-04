---
title: A `context` is exported only from a test-only module
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

In a test-only module, such as `lib/ledger/testing/fixtures.buri`, it compiles,
and any test may import it:

```buri repo=cli/tests/example package=//lib/ledger role=testing
from "platform/effect" import { Allocator, Stdout };
from "platform/effect/testing" import { alloc, stdout };

export context Fixture {
    Allocator: alloc(),
    Stdout: stdout(),
}
```
