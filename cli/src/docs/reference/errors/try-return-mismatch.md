---
title: `?` needs a matching return type
message: `?` on {container} needs {container} return type, not `{type}`
---
# `?` needs a matching return type

```text
error: `?` on a `Result` needs a `Result` return type, not `I64` [try-return-mismatch]
```

```buri fail code=try-return-mismatch
fn unwrap(r: Result<Int, Str>): Int {
    let n = r?;
    n
}
```

`?` returns the error early, so the function has to be able to return one.
Return a `Result`, or handle the error in place with `match` or `withDefault`.
