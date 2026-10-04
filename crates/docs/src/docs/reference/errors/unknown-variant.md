---
title: A variant is one its enum declares
message: '`{type}` has no variant `{variant}`'
fix: name a variant the enum declares
---

```buri fail code=unknown-variant
enum Colour {
    Red,
    Green,
}

fn go(): Colour {
    .Blue
}
```

The fix names a near miss when there is one. Otherwise the diagnostic prints
the enum's declaration.
