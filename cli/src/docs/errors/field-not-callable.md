---
title: A field holding a value is not a method
message: 'field `{field}` has type `{type}`, which is not callable'
fix: this is a field access, not a method call
---

```buri fail code=field-not-callable
struct Holder { export n: Int }

fn go(h: Holder): Int {
  h.n()
}
```
