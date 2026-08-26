---
title: A type parameter stands for one type
message: '`{name}` is a type parameter and takes no type arguments'
fix: drop the arguments; a type parameter stands for one type already
---

```buri fail code=type-parameter-with-arguments
fn first<T>(xs: T<Int>): Int { 0 }
```
