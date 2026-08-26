---
title: A tuple index is a plain decimal number
message: '`{literal}` is not a tuple index'
fix: a tuple index is a plain decimal number, as in `pair.0`
---

```buri fail code=not-a-tuple-index wrap=body
let pair = (1, 2);
let n = pair.99999999999;
```
