---
title: The bitwise operators are defined on integers
message: '`{operator}` is defined on integers, not `{type}`'
---

```buri fail code=bitwise-non-integer
fn go(b: Bool): Bool {
    ~b
}
```
