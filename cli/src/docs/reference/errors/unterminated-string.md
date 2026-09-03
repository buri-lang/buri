---
title: A string literal closes on the line it opens
message: unterminated string literal
fix: close it with `"`; a string literal does not span a line break
---

```buri fail code=unterminated-string wrap=body
let s = "unclosed;
```
