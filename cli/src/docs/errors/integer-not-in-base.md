---
title: An integer literal is written in the base its prefix names
message: '`{literal}` is not a valid base-{radix} integer, or does not fit in 128 bits'
fix: use digits base-{radix} admits, and a value inside 128 bits
---

```buri fail code=integer-not-in-base wrap=body
let n = 0b12;
```
