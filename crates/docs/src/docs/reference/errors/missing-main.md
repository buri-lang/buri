---
title: A binary exports the entry its output fills
message: '{package} exports no `{entry}`'
fix: 'add `export fn {entry}(...)` to its `main.buri`, with the signature its platform declares'
reproduction: none
---
# A binary exports the entry its output fills

```text
error: //cmd/app exports no `main` [missing-main]
```

An output with no `entries` is filled by the function named after the
platform's entry: `main` for the bundled platforms, or the name a repository
platform's `entry` gives, such as `fetch`.

```buri
from "node" import { NodeHost };

export fn main(host: NodeHost): Result<(), Str> {
    .Ok(())
}
```
