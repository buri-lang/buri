---
title: A statement's value is used or bound
message: 'this statement has type `{type}`, not `()`'
note: only an expression whose type is `()` may stand alone; bind anything else
fix: bind it with `let _ = ...;`
---

```buri fail code=statement-not-unit wrap=body
let n = 1;
n + 1;
```
