---
title: A `//` path needs a repository to be relative to
message: "{path}" is outside any repository
fix: import from `"core/..."` or from a `//...` path in this repository
---

```buri fail code=module-outside-repository
from "//lib/ledger" import { Entry };
```
