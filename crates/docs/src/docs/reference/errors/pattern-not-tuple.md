---
title: A tuple pattern matches a tuple of its length
message: '`{type}` is not a {arity}-tuple'
---

```buri fail code=pattern-not-tuple
fn go(n: Int): Int {
    match (n) {
        (a, b) => a,
        _ => 0,
    }
}
```
