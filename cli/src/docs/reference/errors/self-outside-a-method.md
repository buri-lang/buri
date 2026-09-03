---
title: `self` is legal only in a method body
message: '`self` is legal only in a method body'
fix: name a parameter instead, or move this into an `impl` block
---

```buri fail code=self-outside-a-method
fn go(): Int {
    self
}
```
