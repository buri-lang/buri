---
title: A base prefix is followed by digits
message: '`{literal}` has no digits'
fix: write at least one digit after the base prefix, as in `0x1F`
---

```buri fail code=integer-without-digits wrap=body
let n = 0x;
```
