---
title: A call passes the arguments the value's type declares
message: expected {expected} arguments, found {found}
fix: pass exactly {expected}
---

```buri fail code=argument-count-mismatch
fn go(): Int {
    let f = fn(a: Int): Int => a;
    f(1, 2)
}
```
