---
title: A Unicode escape names a scalar value
message: '`\u{{{digits}}}` is not a Unicode scalar value'
fix: a scalar value is at most 10FFFF and outside D800-DFFF
---

```buri fail code=invalid-unicode-escape wrap=body
let s = "\u{D800}";
```
