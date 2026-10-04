---
title: `ctx` comes first, or immediately after `self`
message: `ctx` must come first, or immediately after `self`
note: the calling convention is receiver first, context second, everything else after
fix: move `ctx` to that position
---
# `ctx` comes first, or immediately after `self`

```text
error: `ctx` must come first, or immediately after `self` [ctx-not-first]
```

```buri fail code=ctx-not-first
# from "core/io" import * as io;
# from "platform/effect" import { Stdout };

fn shout<C: Stdout>(times: Int, ctx: C): () {
    io.println(ctx, "loud").ignore()
}
```

The fixed position lets you answer "can this function touch the world?" from
the first two parameters.
