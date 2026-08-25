---
title: A `context` declaration lives where a context may be built
message: a `context` declaration is not legal here
note: a context may be declared only in the module exporting `main`, in a test source, or in a test-only module
fix: move it into the module that exports `main`, or into a test-only module
---
```buri fail code=context-declaration-not-allowed
# from "core/effect" import { Alloc, Stdout };
# from "core/host" import * as host;
context Program {
  Alloc: host.alloc,
  Stdout: host.stdout,
}
```
