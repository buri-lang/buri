---
title: A `Result` may not be discarded
message: a `Result` may not be discarded
fix: consume it: `?` to propagate, `match` to handle both cases, `.withDefault(...)` to supply one — or, when you really mean to drop it, the explicit and greppable `.ignore()`
---
# A `Result` may not be discarded

```text
error: a `Result` may not be discarded [result-discarded]
```

```buri fail code=result-discarded
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead };
# from "core/path" import * as path;
# from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        FileSystemRead: host.fs,
        Stdout: host.stdout,
    };
    let _ = fs.readText(ctx, path.of(ctx, "config.toml"));
    .Ok(())
}
```

A `Result` can be thrown away in only two places: bound to `_` in a `let`, or
left as an expression statement. Both are this error, so `.ignore()` is the one
spelling of a deliberate drop, and `buri lint` reports it as `ignored-result`.

A `_` anywhere in the pattern counts, so `let (count, _) = (1, mayFail());` is
this error too.
