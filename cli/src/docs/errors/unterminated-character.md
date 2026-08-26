---
title: A character literal closes its quote
message: unterminated character literal
fix: close it with `'`
---

```buri fail code=unterminated-character wrap=body
let c = '
```
