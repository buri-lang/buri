---
title: `??` supplies a default for an absent or failed value
message: '`??` takes an `Option` or a `Result`, found `{type}`'
fix: '`??` supplies a default for an absent or failed value; this one is neither'
---

```buri fail code=coalesce-operand
fn go(n: Int): Int {
    n ?? 0
}
```
