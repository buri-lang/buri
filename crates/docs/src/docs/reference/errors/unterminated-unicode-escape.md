---
title: A Unicode escape closes its brace
message: unterminated `\u{{...}}` escape
fix: close it with `}}`
---

```buri fail code=unterminated-unicode-escape wrap=body
let s = "\u{1F600";
```
