---
title: A tuple pattern matches a tuple of that arity
message: '`{type}` is not a {arity}-tuple'
---

```buri fail code=pattern-not-a-tuple
fn go(n: Int): Int {
    match (n) {
        (a, b) => a,
        _ => 0,
    }
}
```
