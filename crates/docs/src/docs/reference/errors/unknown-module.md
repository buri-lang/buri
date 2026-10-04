---
title: A module path names exactly one file
message: '{problem}'
---
# A module path names exactly one file

```text
error: there is no module "core/lists" [unknown-module]
```

```buri fail code=unknown-module
from "core/lists" import * as lists;
```

```buri fail code=unknown-module repo=cli/tests/example
from "native" import { NativeHost };
from "//lib/nope" import { Nope };

export fn main(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```

There's no search path and no fallback, so a path never has two candidates.
