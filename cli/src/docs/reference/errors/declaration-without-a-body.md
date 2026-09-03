---
title: A declaration outside a trait or effect has a body
message: '`{name}` has no body'
note: a function declaration outside a trait or effect needs a block
fix: 'give it one: `{{ ... }}`. Only the bundled standard library declares a signature with no body, for an operation the runtime supplies'
---

```buri fail code=declaration-without-a-body
fn total(a: Int, b: Int): Int;
```
