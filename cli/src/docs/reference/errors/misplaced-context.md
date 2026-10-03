---
title: A context is built only in an entry or a test
message: a context may not be constructed here
note: only in an entry's body, in a test source, or in a test-only module — and never inside a lambda, since a closure that could mint authority would make the capture rule meaningless
fix: build it in the entry and pass it down as a `ctx` parameter, or make this a test source, where a context may be built per test
---
# A context is built only in an entry or a test

```text
error: a context may not be constructed here [misplaced-context]
```

```buri fail code=misplaced-context
# from "core/io" import * as io;
# from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let make = fn(n: Int) => {
        let inner = context {
            Allocator: host.alloc,
            Stdout: host.stdout,
        };
        n
    };
    let _ = io.println(ctx, "${make(1)}").ignore();
    .Ok(())
}
```

A closure that could build a context could hand one to a caller that never
named an effect, which is what the capture rule prevents.

An entry is a function an `outputs` entry names, `main` unless the build file
says otherwise. A binary with a page and a worker has two, and each builds its
own context.
