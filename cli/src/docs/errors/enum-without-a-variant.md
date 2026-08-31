---
title: An enum is named through one of its variants
message: '`{type}` is an enum; name a variant'
---

```buri fail code=enum-without-a-variant
enum Colour {
    Red,
    Green,
}

fn go(): Colour {
    Colour { }
}
```
