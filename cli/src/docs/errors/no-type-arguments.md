---
title: A built-in type takes no type arguments
message: '`{name}` takes no type arguments'
fix: drop them
---

```buri fail code=no-type-arguments
fn width(n: Int<Str>): Int { 0 }
```
