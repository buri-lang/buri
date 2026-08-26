---
title: A function that needs an effect declares `ctx`
message: there is no `ctx` in scope
note: a function that needs an effect declares a `ctx` parameter bounded by the effects it needs
fix: add a `ctx` parameter bounded by the effects this function needs
---

```buri fail code=no-ctx-in-scope
fn go(): Int {
  let c = ctx;
  1
}
```
