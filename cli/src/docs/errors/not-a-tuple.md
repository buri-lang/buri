---
title: A numeric field access indexes a tuple
message: '`{type}` is not a tuple'
fix: index a tuple or a tuple struct; name a field otherwise
---

```buri fail code=not-a-tuple
fn read(xs: [Int]): Int {
  xs.0
}
```
