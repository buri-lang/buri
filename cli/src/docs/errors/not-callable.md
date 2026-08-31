---
title: A call names a function or a lambda
message: '`{type}` is not callable'
fix: call a function, a lambda, or a field holding one — `(x.f)(...)` for a field
---

```buri fail code=not-callable
fn go(n: Int): Int {
    n(1)
}
```
