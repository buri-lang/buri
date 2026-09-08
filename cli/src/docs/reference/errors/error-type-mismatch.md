---
title: "`?` does not convert the error type"
message: "`?` would propagate `{from}`, but this function returns `{to}`"
fix: "map the error first: `.mapErr(fn(e) => ...)?`, producing a `{to}` — there is no automatic error conversion"
---
```buri fail code=error-type-mismatch
# from "core/effect" import { Allocator };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };

fn load<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, Str> {
    let text = fs.readText(ctx, at)?;
    .Ok(text)
}
```
