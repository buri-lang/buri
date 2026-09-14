---
title: A module path names a module that exists
message: there is no module "{path}"
fix: check the path; the standard library's modules are all {roots}
---
# A module path names a module that exists

```text
error: there is no module "core/lists" [no-such-module]
```

## A program that provokes it

```buri fail code=no-such-module
from "core/lists" import * as lists;
```
