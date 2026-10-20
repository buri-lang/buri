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

```text
error: `//lib/money` has no lib/money/lib.buri, the file a library is rooted at [unknown-module]
 --> cmd/app/BUILD.buri:2:20
```

Each rule is rooted at a file: a binary at `main.buri`, a library at
`lib.buri`, a tool at `tool.buri` and a platform at `platform.buri`. A missing
one is reported where a build file names the rule, or at a binary's own rule.
