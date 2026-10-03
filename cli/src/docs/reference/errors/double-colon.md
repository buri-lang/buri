---
title: A module's members are reached with `.`
message: '`::` is not an operator'
fix: a module's members are reached with `.`, as in `list.empty`
---

```buri fail code=double-colon wrap=body
let n = list::empty;
```
