---
title: A constructor is given the values it holds
message: '`{name}` holds {expected} values, but {given} were given'
fix: pass exactly {expected}
---

```buri fail code=wrong-value-count
struct Pair(Int, Int);

fn go(): Pair {
    Pair(1)
}
```
