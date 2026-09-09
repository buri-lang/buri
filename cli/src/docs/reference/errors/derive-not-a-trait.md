---
title: A `derive` names a declared trait
message: '`{name}` is not a trait'
fix: name a declared trait; `derive` generates a trait's methods
---

```buri fail code=derive-not-a-trait
derive Bogus for Point;
struct Point {
    export x: Int,
}
```

A trait the standard library renamed is answered with what it is called now:
`Eq` is `Equal`, and the fix says to write it.

```buri fail code=derive-not-a-trait
derive Eq for Point;
struct Point {
    export x: Int,
}
```
