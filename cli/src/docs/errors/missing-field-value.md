---
title: A literal gives every field a value
message: '`{name}` is missing {fields}'
---

```buri fail code=missing-field-value
struct Point { export x: Int, export y: Int }

fn go(): Point { Point { x: 1 } }
```
