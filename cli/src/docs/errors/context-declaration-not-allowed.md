---
title: context may be declared only in a main.buri, a test file, or a test-only module
message: a `context` declaration is not allowed here
note: context may be declared only in a main.buri, a test file, or a test-only module
fix: Accept a context variable as an argument, and pass it into the function from a main.buri, a test file, or a test-only module
---

```buri fail code=context-declaration-not-allowed
# //libs/print/lib.buri
from "core/effect" import { Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

context Console {
  Stdout: host.stdout,
}

export fn print(output: Str): () {
  io.println(Console(), output)
}
```

To fix, just accept the context as an argument:

```buri
from "core/effect" import { Stdout };
from "core/io" import * as io;

export fn print<C: Stdout>(ctx: C, output: Str): () {
  io.println(ctx, output)
}
```
