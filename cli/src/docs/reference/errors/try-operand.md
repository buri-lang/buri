---
title: `?` propagates a failure
message: '`?` takes a `Result` or an `Option`, found `{type}`'
fix: '`?` propagates a failure; this value is neither a `Result` nor an `Option`'
---

```buri fail code=try-operand
fn go(n: Int): Option<Int> {
    let m = n?;
    Option.Some(m)
}
```
