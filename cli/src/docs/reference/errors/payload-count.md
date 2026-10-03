---
title: A constructor is given the values it holds
message: '`{name}` holds {expected} values, but {given} were given'
fix: 'pass `{name}` the following values: {shape}'
---
# A constructor is given the values it holds

```buri fail code=payload-count
struct Pair(Int, Int);

fn go(): Pair {
    Pair(1)
}
```
