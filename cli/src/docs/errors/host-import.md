---
title: '`core/host` is not imported by a module that exports `main`'
message: '"core/host" is importable only from the module that exports `main`'
note: the context `main` builds is the program's complete effect budget; a module that could import `core/host` would be a second place authority enters
fix: take what you need as a `ctx` bound instead, and let `main` supply the implementation
---

```buri fail code=host-import
from "core/host" import * as host;
```
