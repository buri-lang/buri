---
title: A module path names exactly one file
message: {problem}
fix: create the file the path names, or correct the path — a module path maps to exactly one file, with no search
---
# A module path names exactly one file

```text
error: "//lib/nope" is in no package of this repository [module-not-found]
```

This resolves against the example monorepo in `cli/tests/example`, which has no
`lib/nope`:

```buri fail code=module-not-found repo=cli/tests/example
from "native" import { NativeHost };
from "//lib/nope" import { Nope };

export fn main(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```

There's no search path and no fallback, so a path never has two candidates.
