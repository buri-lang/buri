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
