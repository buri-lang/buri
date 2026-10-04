---
title: An enum value names a variant
message: '`{type}` is an enum; name a variant'
---

```buri fail code=enum-not-value
enum Colour {
    Red,
    Green,
}

fn go(): Colour {
    Colour { }
}
```
