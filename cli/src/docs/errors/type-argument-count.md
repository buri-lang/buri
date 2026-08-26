---
title: A type is written with the arguments its declaration takes
message: '`{type}` takes {expected} type arguments, but {given} were given'
fix: supply exactly {expected}
---

```buri fail code=type-argument-count
struct Pair<A, B> { export a: A, export b: B }

fn take(p: Pair<Int>): Int { 0 }
```
