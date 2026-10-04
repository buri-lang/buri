---
title: An expression statement is legal only in a test
message: an expression statement is legal only in a test source
note: a block is `let`s followed by a result expression
fix: bind it: `let _ = ...;`, or make it the block's result expression
---
# An expression statement is legal only in a test

```text
error: an expression statement is legal only in a test source [statement-outside-test]
```

```buri fail code=statement-outside-test wrap=body effects=Stdout,Allocator
io.println(ctx, "ready");
```

A test source is the one exception, which lets `assert.equal(...)` stand alone:
there any expression of type `()` may stand as a statement ending in `;`,
including a `match` or an `if` whose branches all assert.

A `Result` can't be dropped either way, per `result-discarded`, so a statement
of type `Result` needs both `.ignore()` and `let _ =`:

```buri role=entry
# from "core/io" import * as io;
# from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```
