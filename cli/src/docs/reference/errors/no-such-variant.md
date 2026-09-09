---
title: A variant is named by the enum that declares it
message: '`{type}` has no variant `{variant}`'
fix: name a variant the enum declares
---

```buri fail code=no-such-variant
enum Colour {
    Red,
    Green,
}

fn go(): Colour {
    .Blue
}
```

Where the name is a near miss the fix names it; otherwise the enum's own
declaration is the list, and the diagnostic prints it.
