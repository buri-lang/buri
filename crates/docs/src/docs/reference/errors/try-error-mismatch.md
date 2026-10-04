---
title: "`?` does not convert the error type"
message: "`?` would propagate `{from}`, but this function returns `{to}`"
fix: "map the error first: `.mapErr(fn(e) => ...)?`, producing a `{to}` — there is no automatic error conversion"
---
```buri fail code=try-error-mismatch
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };
# from "platform/effect" import { Allocator };

fn load<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, Str> {
    let text = fs.readText(ctx, at)?;
    .Ok(text)
}
```
