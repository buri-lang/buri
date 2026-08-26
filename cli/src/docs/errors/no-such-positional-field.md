---
title: A tuple struct's fields are numbered from zero
message: '`{type}` has no field {index}'
fix: '`{type}` has {count} fields'
---

```buri fail code=no-such-positional-field
struct Wrapper(Int);

fn read(w: Wrapper): Int {
  w.3
}
```
