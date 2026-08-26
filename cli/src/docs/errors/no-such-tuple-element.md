---
title: A tuple's elements are numbered from zero
message: a {arity}-tuple has no element {index}
fix: the elements are `.0` through `.{last}`
---

```buri fail code=no-such-tuple-element wrap=body
let pair = (1, 2);
let n = pair.5;
```
