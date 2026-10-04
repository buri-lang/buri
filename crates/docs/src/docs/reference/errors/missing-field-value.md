---
title: A literal gives every required field a value
message: '`{name}` is missing {fields}'
---

```buri fail code=missing-field-value
struct Point {
    export x: Int,
    export y: Int,
    export label: Option<Str>,
}

fn go(): Point {
    Point { x: 1 }
}
```

An `Option<...>` field isn't required. Leaving it out writes `.None`, so the
message names only the other fields.
