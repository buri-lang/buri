# A `Result` may not be discarded

```text
error: a `Result` may not be discarded [result-discarded]
```

## What to do

consume it: `?` to propagate, `match` to handle both cases, `result.withDefault` to supply one — or, when you really mean to drop it, the explicit and greppable `result.ignore`

## A program that provokes it

```buri fail code=result-discarded
from "core/cap" import { Alloc, Fs, Stdout };
from "core/host" import * as host;
from "core/fs" import * as fs;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Fs: host.fs, Stdout: host.stdout };
  let _ = fs.readText(ctx, "config.toml");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `result-discarded` — so
this page cannot describe an error the compiler has stopped emitting.
