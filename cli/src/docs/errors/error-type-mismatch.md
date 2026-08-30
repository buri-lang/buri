---
title: "`?` does not convert the error type"
message: "`?` would propagate `{from}`, but this function returns `{to}`"
fix: "map the error first: `.mapErr(fn(e) => ...)?`, producing a `{to}` — there is no automatic error conversion"
---
```buri fail code=error-type-mismatch
# from "core/effect/lib.buri" import { Alloc, Fs };
# from "core/fs/lib.buri" import * as fs;
fn load<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, Str> {
  let text = fs.readText(ctx, path)?;
  .Ok(text)
}
```
