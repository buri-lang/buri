---
title: A literal gives every required field a value
message: '`{name}` is missing {fields}'
---

A field whose declared type is `Option<...>` is not required. Leaving it out
writes `.None` for it, so the message names only the other fields.

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
