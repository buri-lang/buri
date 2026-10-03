---
title: A context binds every effect its callee needs
message: '`{function}` needs `{effect}`, which this context does not bind'
note: the context binds {bound}
fix: 'bind `{effect}` in the `context {{ ... }}` this call is handed'
---
# A context binds every effect its callee needs

```text
error: `println` needs `Stdout`, which this context does not bind [unbound-effect]
```

```buri fail code=unbound-effect
# from "core/alloc" import * as alloc;
# from "core/io" import * as io;
# from "node" import { NodeHost };
# from "platform/effect" import { Allocator };

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
    };
    alloc.scoped(ctx, fn(c) => io.println(c, "hi")).mapErr(fn(_e) => "could not write")
}
```

`alloc.scoped` hands its body a `Scoped<C>`, which is a `Stdout` only when the
context it wraps is one. Type checking takes the wrapper at its word, so the
missing binding turns up when the program is compiled for the context it
actually got.
