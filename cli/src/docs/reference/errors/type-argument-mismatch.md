---
title: A call supplies the type arguments its function declares
message: expected {expected} type arguments, found {found}
fix: supply exactly {expected}
---

```buri fail code=type-argument-mismatch
fn identity<T>(x: T): T {
    x
}

fn go(): Int {
    identity<Int, Str>(1)
}
```
