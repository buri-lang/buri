---
title: `load` takes a function name
message: '`load` takes the name of a function, and this is {got}'
label: nothing to move into a chunk
fix: 'name a function: `lazy.load(adminPage)`'
---
# `load` takes a function name

```text
error: `load` takes the name of a function, and this is Int [load-not-function]
```

```buri fail code=load-not-function
# from "core/lazy" import * as lazy;

fn eager(): Int {
    let f = lazy.load(3);
    f
}
```

Declare the function and hand `load` its name:

```buri
from "core/io" import * as io;
from "core/lazy" import * as lazy;
from "platform/effect" import { Stdout };

fn admin<C: Stdout>(ctx: C): () {
    io.println(ctx, "admin").ignore()
}

fn route<C: Stdout>(ctx: C): () {
    let page = lazy.load(admin);
    page(ctx)
}
```

`load` moves a function into its own chunk, so it needs a declared function to
move. A lambda at the call site is part of the surrounding body. A local holding
a function is a value, and the compiler can't see which body it holds. The
signature, `load<F>(f: F): F`, accepts anything, so this check does the
refusing.
