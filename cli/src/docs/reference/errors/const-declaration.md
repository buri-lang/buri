---
title: A module-level binding is written with `let`
message: there is no `const` declaration; a module-level binding is written with `let`
note: one keyword binds everywhere, and a module-level `let` still writes its type
fix: write `let` in place of `const`
---

```buri fail code=const-declaration
const MAX_RETRIES: Int = 3;
```
