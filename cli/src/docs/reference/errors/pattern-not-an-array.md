---
title: An array pattern matches an array
message: '`{type}` is not an array'
---

```buri fail code=pattern-not-an-array
fn go(n: Int): Int {
    match (n) {
        [a] => a,
        _ => 0,
    }
}
```
