---
title: A module exports what it says it exports
message: "{path}" does not export `{name}`
---
# A module exports what it says it exports

```text
error: "core/list" does not export `notAThing` [no-such-export]
```

```buri fail code=no-such-export
from "core/list" export { notAThing };
```

A re-export names only what its module exports, so a library's surface is never
wider than the modules behind it.
