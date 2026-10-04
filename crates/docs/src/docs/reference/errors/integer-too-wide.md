---
title: An integer literal fits in 128 bits
message: '`{literal}` does not fit in 128 bits'
fix: write a smaller value; 128 bits is the widest integer type
---

```buri fail code=integer-too-wide wrap=body
let n = 999999999999999999999999999999999999999999;
```
