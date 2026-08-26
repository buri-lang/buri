---
title: A `derive` names a declared trait
message: '`{name}` is not a trait'
fix: name a declared trait; `derive` generates a trait's methods
---

```buri fail code=derive-not-a-trait
struct Point { export x: Int }

derive Bogus for Point;
```
