---
title: Two tuple indices in a row lex as a float
message: '`.{literal}` lexes as a float, not two tuple indices'
fix: 'parenthesize the first index: `(t.0).1`'
---

```buri fail code=float-as-a-tuple-index wrap=body
let nested = ((1, 2), 3);
let n = nested.0.1;
```
