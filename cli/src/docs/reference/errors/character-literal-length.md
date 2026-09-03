---
title: A character literal holds one scalar value
message: a character literal holds exactly one Unicode scalar value
fix: use a string literal for more than one
---

```buri fail code=character-literal-length wrap=body
let c = 'ab';
```
