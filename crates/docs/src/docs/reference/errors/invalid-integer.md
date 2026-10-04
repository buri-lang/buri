---
title: An integer's digits match its base prefix
message: '`{literal}` is not a valid base-{radix} integer, or does not fit in 128 bits'
fix: use digits base-{radix} admits, and a value inside 128 bits
---

```buri fail code=invalid-integer wrap=body
let n = 0b12;
```
