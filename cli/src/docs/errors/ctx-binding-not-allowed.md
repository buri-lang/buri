---
title: `ctx` is bound where a context may be built
message: '`ctx` may be bound only where a context may be built'
note: that is `main`'s body, a test source, or a test-only module (SPEC 11.3)
fix: rename the binding, or move the construction into `main` or a test source
---

```buri fail code=ctx-binding-not-allowed
fn go(): Int {
  let ctx = 1;
  ctx
}
```
